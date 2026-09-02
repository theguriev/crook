// Every non-text pixel Crook draws goes through this shader: tab backgrounds, the tab bar,
// the token-usage chip, dividers, focus rings. One instanced unit quad per rect; the rounded
// corners, the gradient fill and the per-side borders are all fragment-shader SDF math over it.
//
// Ported from Warp's `crates/warpui/src/rendering/wgpu/shaders/rect_shader.wgsl` (MIT).
// The drop-shadow half (rounded_box_shadow / rounded_box_shadow_x / erf / gaussian) and the
// dashed-border block are deliberately absent — see src/shaders/README.md.

struct Uniforms {
    // Surface size in PHYSICAL pixels.
    viewport_size: vec2<f32>,
    // Some wgpu backends (webgl, and gles drivers generally) require a uniform buffer binding to
    // be a multiple of 16 bytes, so the struct is padded up to that on both sides of the wire.
    padding: vec2<f32>
}

const EPSILON: f32 = 0.0000001;

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct RectVertexShaderInput {
    // Corner of the shared unit quad, in [0,1]^2 — NOT normalized device coordinates.
    @location(0) vertex_position: vec2<f32>,
    // Rect bounds in physical pixels: origin in `xy`, size in `zw`.
    @location(1) bounds: vec4<f32>,
    // Gradient endpoints are given in [0,1] relative to the rect, so the CPU never has to know
    // the rect's pixel size to express "left edge to right edge".
    @location(2) background_start: vec2<f32>,
    @location(3) background_start_color: vec4<f32>,
    @location(4) background_end: vec2<f32>,
    @location(5) background_end_color: vec4<f32>,
    // Border widths in physical pixels, in the order top, right, bottom, left.
    @location(6) border_width: vec4<f32>,
    @location(7) border_start: vec2<f32>,
    @location(8) border_start_color: vec4<f32>,
    @location(9) border_end: vec2<f32>,
    @location(10) border_end_color: vec4<f32>,
    // Corner radii in physical pixels, in the order top_left, top_right, bottom_left,
    // bottom_right. Note this is NOT the CSS order, which puts bottom_right third.
    @location(11) corner_radius: vec4<f32>,
};

struct RectVertexShaderOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) background_start: vec2<f32>,
    @location(1) background_start_color: vec4<f32>,
    @location(2) background_end: vec2<f32>,
    @location(3) background_end_color: vec4<f32>,
    @location(4) border_width: vec4<f32>,
    @location(5) border_start: vec2<f32>,
    @location(6) border_start_color: vec4<f32>,
    @location(7) border_end: vec2<f32>,
    @location(8) border_end_color: vec4<f32>,
    @location(9) rect_corner: vec2<f32>,
    @location(10) rect_center: vec2<f32>,
    @location(11) corner_radius: vec4<f32>,
};

@vertex
fn vs_main(
    in: RectVertexShaderInput,
) -> RectVertexShaderOutput {
    var out: RectVertexShaderOutput;
    var origin: vec2<f32> = in.bounds.xy;
    var size: vec2<f32> = in.bounds.zw;
    var pixel_pos: vec2<f32> = in.vertex_position * size + origin;
    // Screen space has its origin at the top-left with y growing downwards; NDC has its origin at
    // the center with y growing upwards. The vec2(2.0, -2.0) scale is what flips y.
    var ndc_position: vec2<f32> = pixel_pos / uniforms.viewport_size * vec2(2.0, -2.0) + vec2(-1.0, 1.0);

    out.position = vec4<f32>(ndc_position, 0.0, 1.0);
    // Gradient endpoints are resolved to pixel space here so the fragment stage can project the
    // fragment's own pixel position onto the gradient axis with a plain dot product.
    out.background_start = in.background_start * size + origin;
    out.background_start_color = in.background_start_color;
    out.background_end = in.background_end * size + origin;
    out.background_end_color = in.background_end_color;
    out.border_start = in.border_start * size + origin;
    out.border_start_color = in.border_start_color;
    out.border_end = in.border_end * size + origin;
    out.border_end_color = in.border_end_color;
    out.border_width = in.border_width;
    out.corner_radius = in.corner_radius;
    out.rect_corner = size / 2.;
    out.rect_center = origin + out.rect_corner;

    return out;
}

@fragment
fn rect_fs_main(in: RectVertexShaderOutput) -> @location(0) vec4<f32> {
    var background_color: vec4<f32> = derive_color(
        in.position.xy,
        in.background_start,
        in.background_end,
        in.background_start_color,
        in.background_end_color
    );
    var border_color: vec4<f32> = derive_color(
        in.position.xy,
        in.border_start,
        in.border_end,
        in.border_start_color,
        in.border_end_color
    );

    // There are actually two different radii at play here - the inner
    // (background) and outer (shape) radii.  The inner radius is equal to the
    // outer radius minus the border width, in order for the two curves to
    // maintain a constant distance from each other.
    var inner_corner_radius: f32;
    var outer_corner_radius: f32;

    var border_inner_corner: vec2<f32> = in.rect_corner;
    if in.position.y >= in.rect_center.y {
        // Bottom half
        border_inner_corner.y -= in.border_width.z;
        if in.position.x >= in.rect_center.x {
          // Bottom right quadrant
            border_inner_corner.x -= in.border_width.y;
            outer_corner_radius = in.corner_radius.w;
            inner_corner_radius = max(0.0, outer_corner_radius - in.border_width.z);
        } else {
          // Bottom left quadrant
            border_inner_corner.x -= in.border_width.w;
            outer_corner_radius = in.corner_radius.z;
            inner_corner_radius = max(0.0, outer_corner_radius - in.border_width.z);
        }
    } else {
        // Top half
        border_inner_corner.y -= in.border_width.x;
        if in.position.x >= in.rect_center.x {
          // Top right quadrant
            border_inner_corner.x -= in.border_width.y;
            outer_corner_radius = in.corner_radius.y;
            inner_corner_radius = max(0.0, outer_corner_radius - in.border_width.x);
        } else {
          // Top left quadrant
            border_inner_corner.x -= in.border_width.w;
            outer_corner_radius = in.corner_radius.x;
            inner_corner_radius = max(0.0, outer_corner_radius - in.border_width.x);
        }
    }

    var outer_distance: f32 = distance_from_rect(in.position.xy, in.rect_center, in.rect_corner, outer_corner_radius);
    var inner_distance: f32 = distance_from_rect(in.position.xy, in.rect_center, border_inner_corner, inner_corner_radius);

    // Adjust the opacity of the border color based on where the pixel lies
    // between the background and the border_width.
    border_color.a *= saturate(inner_distance + 0.5);

    // Force the alpha value to 0 (fully transparent) if the pixel is
    // outside the border_width.
    //
    // When we are outside the border, outer_distance is a larger positive
    // value than inner_distance.  When we are inside the border itself,
    // outer_distance is negative and inner_distance is positive.  When we
    // are inside the inner border edge, outer_distance is more negative
    // than inner_distance.
    border_color.a *= f32(inner_distance > outer_distance);

    // Perform proper alpha blending on the two colors, avoiding a
    // divide-by-zero if both colors are fully transparent.
    //
    // See formula for "over" compositing here: https://en.wikipedia.org/wiki/Alpha_compositing#Alpha_blending
    var alpha: f32 = border_color.a + background_color.a * (1.0 - border_color.a);
    var new_background_color: vec3<f32> = (border_color.rgb * border_color.a + background_color.rgb * background_color.a * (1.0 - border_color.a)) / (alpha + EPSILON);
    background_color = vec4(new_background_color, alpha);

    // If there's a corner radius we need to do some anti aliasing to smooth out the rounded corner effect.
    if outer_corner_radius > 0. {
        background_color.a *= 1.0 - saturate(outer_distance + 0.5);
    }

    return background_color;
}

// Projects `position` onto the start->end axis and interpolates. `h` is deliberately
// unclamped: a gradient axis shorter than the rect keeps extending past its endpoints, and
// `mix` with h outside [0,1] extrapolates, which is what makes a two-stop gradient placed on
// part of a rect still cover the whole rect.
fn derive_color(
    position: vec2<f32>,
    start: vec2<f32>,
    end: vec2<f32>,
    start_color: vec4<f32>,
    end_color: vec4<f32>
) -> vec4<f32> {
    var adjusted_end: vec2<f32> = end - start;
    var h: f32 = dot(position - start, adjusted_end) / dot(adjusted_end, adjusted_end);
    return mix(start_color, end_color, h);
}

// Signed distance to a rounded rect: negative inside, positive outside, and (crucially) in units
// of pixels near the edge, which is what lets the caller antialias with a single saturate().
fn distance_from_rect(pixel_pos: vec2<f32>, rect_center: vec2<f32>, rect_corner: vec2<f32>, corner_radius: f32) -> f32 {
    var p: vec2<f32> = pixel_pos - rect_center;
    var q: vec2<f32> = abs(p) - rect_corner + corner_radius;
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - corner_radius;
}
