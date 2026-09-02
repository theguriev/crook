// Every glyph Crook draws: tab titles, the token count in the usage chip, and eventually a
// terminal grid. One instanced unit quad per glyph, sampling an RGBA8 atlas that holds both
// alpha-coverage masks (byte-replicated into all four channels) and full-color emoji bitmaps.
//
// Ported from Warp's `crates/warpui/src/rendering/wgpu/shaders/glyph_shader.wgsl` (MIT).
// The horizontal-fade attributes are absent — Crook truncates overflowing text with an ellipsis
// instead. See src/shaders/README.md.

// Brightness-scaled contrast enhancement for glyph alpha masks.
//
// Linear sRGB blending makes light-on-dark text appear too thin because AA fringe
// pixels blend perceptually darker than expected. Dark-on-light text has the opposite
// problem — it already looks heavier than its geometric coverage.
//
// To compensate, we compute the text color's brightness (k) and use it to boost the
// glyph alpha through enhance_contrast(). Brighter text gets a stronger boost;
// dark text is left unchanged.
//
// enhance_contrast() adapted from DWrite_EnhanceContrast in Windows Terminal's DirectWrite shader:
// https://github.com/microsoft/terminal/blob/1283c0f5b99a2961673249fa77c6b986efb5086c/src/renderer/atlas/dwrite.hlsl
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
fn glyph_color_brightness(color: vec3<f32>) -> f32 {
    // REC. 601 luminance coefficients for perceived brightness.
    return dot(color, vec3<f32>(0.30, 0.59, 0.11));
}

fn enhance_contrast(alpha: f32, k: f32) -> f32 {
    return alpha * (k + 1.0) / (alpha * k + 1.0);
}

struct Uniforms {
    // Surface size in PHYSICAL pixels.
    viewport_size: vec2<f32>,
    // Some wgpu backends (webgl, and gles drivers generally) require a uniform buffer binding to
    // be a multiple of 16 bytes, so the struct is padded up to that on both sides of the wire.
    padding: vec2<f32>
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@group(1) @binding(0) var glyphAtlasTexture: texture_2d<f32>;
@group(1) @binding(1) var glyphAtlasSampler: sampler;

struct GlyphVertexShaderInput {
    // Corner of the shared unit quad, in [0,1]^2 — NOT normalized device coordinates.
    @location(0) vertex_position: vec2<f32>,
    // Quad bounds in physical pixels: origin in `xy`, size in `zw`. The size must be the glyph's
    // size in the ATLAS, not its raster bounds — a smaller size makes the sample below read a
    // sub-region of what was uploaded and fringes the glyph.
    @location(1) bounds: vec4<f32>,
    // Atlas region in UV space: origin in `xy`, size in `zw`.
    @location(2) uv_bounds: vec4<f32>,
    // Text color for mask glyphs; ignored for emoji, which carry their own color.
    @location(3) color: vec4<f32>,
    @location(4) is_emoji: i32,
}

struct GlyphVertexShaderOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texture_coordinate: vec2<f32>,
    @location(1) color: vec4<f32>,
    // Flat, because a per-glyph branch selector must not be interpolated across the quad.
    @location(2) @interpolate(flat) is_emoji: i32,
}

@vertex
fn vs_main(
    glyph: GlyphVertexShaderInput,
) -> GlyphVertexShaderOutput {
    var out: GlyphVertexShaderOutput;
    var origin: vec2<f32> = glyph.bounds.xy;
    var size: vec2<f32> = glyph.bounds.zw;
    var pixel_pos: vec2<f32> = glyph.vertex_position * size + origin;

    // Use floor here to vertically align the glyph to the pixel grid.
    // If it's not aligned to the grid, the fragment shader will do its
    // own interpolation, which makes it so we don't use the anti-aliasing
    // from the rasterizer, which is what we want.  We don't force the glyph to a
    // horizontal pixel position because we rasterize the glyph at multiple
    // subpixel positions, and so the very slight linear interpolation here
    // won't produce a fuzzy glyph, just a correctly-positioned one.
    pixel_pos = vec2(pixel_pos.x, floor(pixel_pos.y));

    // Screen space has its origin at the top-left with y growing downwards; NDC has its origin at
    // the center with y growing upwards. The vec2(2.0, -2.0) scale is what flips y.
    var device_pos: vec2<f32> = pixel_pos / uniforms.viewport_size * vec2(2.0, -2.0) + vec2(-1.0, 1.0);

    var texture_coordinate: vec2<f32> = glyph.uv_bounds.xy + glyph.vertex_position * glyph.uv_bounds.zw;

    out.position = vec4<f32>(device_pos, 0.0, 1.0);
    out.texture_coordinate = texture_coordinate;
    out.color = glyph.color;
    out.is_emoji = glyph.is_emoji;
    return out;
}

@fragment
fn fs_main(in: GlyphVertexShaderOutput) -> @location(0) vec4<f32> {
    // Sample the texture to obtain a color.
    var tex_color: vec4<f32> = textureSample(glyphAtlasTexture, glyphAtlasSampler, in.texture_coordinate);
    // Use the input color for non-emoji, and the sampled color for emoji.
    var color: vec4<f32> = mix(in.color, tex_color, f32(in.is_emoji));

    // Scale contrast boost by text brightness:
    // light text (white=1) gets full boost; dark text (black=0) gets none.
    // A mask's coverage lives in .r because A8 rasterizer output is byte-replicated across RGBA
    // on upload; the max() then bypasses the whole curve for emoji, whose alpha is already final.
    let k = glyph_color_brightness(color.rgb);
    let contrasted = enhance_contrast(tex_color.r, k);
    color.a *= max(contrasted, f32(in.is_emoji));

    return color;
}
