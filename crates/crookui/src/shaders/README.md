# Shaders

Two WGSL shaders, both ported from Warp's MIT-licensed `warpui` crate. They are the entire
rendering surface of Crook v1: `rect_shader.wgsl` draws tabs and the token-usage chip,
`glyph_shader.wgsl` draws the text inside them.

| file | origin | lines |
| --- | --- | --- |
| `rect_shader.wgsl` | `warp/crates/warpui/src/rendering/wgpu/shaders/rect_shader.wgsl` (291) | 200 |
| `glyph_shader.wgsl` | `warp/crates/warpui/src/rendering/wgpu/shaders/glyph_shader.wgsl` (123) | 114 |

Both target wgpu 30 / naga. Neither uses any construct that was not already in the Warp
originals.

---

## Coordinate space

One convention runs through both shaders, and getting it wrong is the most expensive mistake
available here.

**Screen space is physical pixels, origin at the top-left of the surface, `+x` right, `+y` down.**

- The `Scene` stores *logical* coordinates. The CPU multiplies by `scale_factor` exactly once,
  when it builds instance data. Everything that reaches the GPU is already physical.
- `Uniforms.viewport_size` is the **surface texture size in physical pixels** — the same value the
  swapchain was configured with, not the window's logical size. It reaches the shader through a
  16-byte uniform buffer at `@group(0) @binding(0)`, rewritten with `queue.write_buffer` once per
  frame just before the render pass binds group 0.
- Both vertex shaders convert with the same line:

  ```wgsl
  pixel_pos / uniforms.viewport_size * vec2(2.0, -2.0) + vec2(-1.0, 1.0)
  ```

  The `-2.0` on y is the entire y-flip. There is no projection matrix anywhere in this renderer.
- In the **fragment** stage, `@builtin(position).xy` is the fragment's window-space coordinate,
  again in physical pixels with `+y` down, sampled at pixel centres (so `x.5`, `y.5`). That is why
  `rect_fs_main` can compare it directly against `rect_center`, the gradient endpoints and the
  border insets without any transform. If you ever add a second render target with a different
  size, this identity breaks.

`vertex_position` at `@location(0)` is a corner of the shared unit quad in `[0,1]²`. It is **not**
NDC, despite the comment in the Warp original saying so. Both shaders map it into the instance's
`bounds` rect; that is the whole geometry story.

---

## Shared vertex buffer (slot 0)

One static buffer for the whole app, bound once per frame.

| field | WGSL type | location | wgpu format | offset |
| --- | --- | --- | --- | --- |
| `position` | `vec2<f32>` | 0 | `Float32x2` | 0 |

`array_stride: 8`, `step_mode: Vertex`. Vertices `[(0,0), (1,0), (0,1), (1,1)]`, indices
`[0, 1, 2, 2, 3, 1]` as `Uint16`. Every draw is
`draw_indexed(0..6, 0, start_instance..end_instance)`.

---

## `rect_shader.wgsl`

Antialiased rounded rect: linear-gradient fill, linear-gradient border with independent per-side
widths, four independent corner radii. Entry points `vs_main` / `rect_fs_main`.

### Dropped from the Warp original

- **Drop shadows** — `rounded_box_shadow`, `rounded_box_shadow_x`, `erf`, `gaussian`, the
  `drop_shadow_data` attribute and the whole `if drop_shadow_sigma > 0.0` branch in the fragment
  shader (~75 lines). Nothing in v1 casts a shadow. Removing the branch is what let the border
  compositing un-indent by one level; that block is otherwise unchanged.
- **Dashed borders** — the `dashed_border_data` attribute and the `is_horizontal_dash` /
  `is_vertical_dash` masking block (~15 lines). Restoring it also means restoring Warp's
  `get_best_dash_gap`, which picks a gap length that divides evenly into each side so dashes don't
  break at the corners.
- **`select_border_radius`** — dead in Warp too: defined, never called. Its doc comment claimed
  CSS radius order, which contradicts the order the live code actually uses. Deleting it removes a
  trap rather than a feature.
- **`PI`** — only `gaussian` used it.
- **`rect_origin`** — only the shadow and dash blocks used it.

Everything else — `distance_from_rect`, `derive_color`, the quadrant selection, the "over"
composite, the one-line corner antialias — is character-for-character Warp.

### Instance buffer (slot 1), `step_mode: Instance`

11 attributes, `array_stride: 144`. Field order is stable; the Rust `#[repr(C)]` struct must
declare these fields in exactly this sequence with no padding (every field is 4-byte aligned, so
`repr(C)` and `wgpu::vertex_attr_array!` agree naturally).

| field | WGSL type | location | wgpu format | offset | size |
| --- | --- | --- | --- | --- | --- |
| `bounds` | `vec4<f32>` | 1 | `Float32x4` | 0 | 16 |
| `background_start` | `vec2<f32>` | 2 | `Float32x2` | 16 | 8 |
| `background_start_color` | `vec4<f32>` | 3 | `Float32x4` | 24 | 16 |
| `background_end` | `vec2<f32>` | 4 | `Float32x2` | 40 | 8 |
| `background_end_color` | `vec4<f32>` | 5 | `Float32x4` | 48 | 16 |
| `border_width` | `vec4<f32>` | 6 | `Float32x4` | 64 | 16 |
| `border_start` | `vec2<f32>` | 7 | `Float32x2` | 80 | 8 |
| `border_start_color` | `vec4<f32>` | 8 | `Float32x4` | 88 | 16 |
| `border_end` | `vec2<f32>` | 9 | `Float32x2` | 104 | 8 |
| `border_end_color` | `vec4<f32>` | 10 | `Float32x4` | 112 | 16 |
| `corner_radius` | `vec4<f32>` | 11 | `Float32x4` | 128 | 16 |

Semantics:

- `bounds` — `xy` = origin, `zw` = size, physical pixels.
- `background_*` / `border_*` — a two-stop linear gradient. The two endpoints are in `[0,1]`
  *relative to the rect*, so the CPU expresses "left edge to right edge" as `(0,0)` → `(1,0)`
  without knowing the pixel size. A flat colour is start == end colour with any non-degenerate
  axis; `(0,0)` → `(1,0)` is what Warp uses. **The axis must not be zero-length** — `derive_color`
  divides by `dot(axis, axis)`.
- `border_width` — **top, right, bottom, left**, physical pixels. Warp's WGSL comment said
  "top, left, right, bottom" and was simply wrong: its Rust builds `vec4f(top, right, bottom,
  left)` and the fragment shader reads `.x`=top, `.y`=right, `.z`=bottom, `.w`=left. Corrected
  here.
- `corner_radius` — **top_left, top_right, bottom_left, bottom_right**, physical pixels. This is
  *not* the CSS order (CSS puts bottom_right third). Warp's Rust
  `CornerRadius::from_ui_corner_radius` produces this order and the fragment shader consumes it.

### Bind groups

| group | binding | resource | visibility |
| --- | --- | --- | --- |
| 0 | 0 | `var<uniform> uniforms: Uniforms` (16 bytes) | `VERTEX` |

That is all. No textures, no samplers.

### Things a future reader will otherwise break

1. **The output is straight (non-premultiplied) alpha.** The pipeline must use
   `BlendState::ALPHA_BLENDING`. The manual "over" composite of border-over-background divides the
   summed colour back out by `alpha + EPSILON` for exactly this reason — that division is not a
   numerical nicety, it is what un-premultiplies the result. If you switch the blend state to
   `PREMULTIPLIED_ALPHA_BLENDING`, delete the division too, or everything with a semi-transparent
   border washes out.
2. **Nothing clamps `corner_radius`.** Not the shader, and not Warp's CPU side either. A radius
   larger than half the shorter side makes the inner and outer SDFs cross and the border inverts.
   Clamp to `min(width, height) / 2` when you build the instance.
3. **The corner antialias only runs when `outer_corner_radius > 0`.** A square rect gets no AA at
   all. That is correct for axis-aligned rects on integer pixel boundaries and wrong for anything
   else — if a rect ever lands on a fractional physical pixel and looks hard-edged, this branch is
   why.
4. **The inner corner radius is derived from the top/bottom border width only**, never left/right
   (see the four quadrant cases). Uniform-width borders are exact; a rect with a 1px top and an 8px
   left border gets a slightly wrong inner curve. This matches Warp and is invisible at the widths
   a tab or chip uses.

---

## `glyph_shader.wgsl`

Samples an RGBA8 glyph atlas that holds both alpha-coverage masks and full-colour emoji bitmaps.
Entry points `vs_main` / `fs_main`.

### Dropped from the Warp original

- **`GlyphFade`** — the `fade_start` / `fade_end` attributes, the `fade_alpha` varying, the
  direction-detecting block in the vertex shader and the final `color.a *= saturate(fade_alpha)`.
  Warp fades a tab title out horizontally when it overflows; Crook truncates with an ellipsis
  instead, which is a layout-side decision and needs nothing from the shader.
- **`rect_center` / `rect_corner` varyings** — computed by Warp's vertex shader and never read by
  its fragment shader. Dead in the original.

Kept **exactly**, per the port brief: `glyph_color_brightness`, `enhance_contrast`, and the
`pixel_pos = vec2(pixel_pos.x, floor(pixel_pos.y))` vertical snap. The only edit inside those
regions is one word in a comment ("core text" → "the rasterizer", since Crook rasterizes with
swash on every platform). Together these two things are most of the difference between text that
looks like a terminal and text that looks like a game engine drawing text — do not "clean them
up".

### Instance buffer (slot 1), `step_mode: Instance`

4 attributes, `array_stride: 52`. Locations were renumbered after the fade attributes were
removed, so they are contiguous.

| field | WGSL type | location | wgpu format | offset | size |
| --- | --- | --- | --- | --- | --- |
| `bounds` | `vec4<f32>` | 1 | `Float32x4` | 0 | 16 |
| `uv_bounds` | `vec4<f32>` | 2 | `Float32x4` | 16 | 16 |
| `color` | `vec4<f32>` | 3 | `Float32x4` | 32 | 16 |
| `is_emoji` | `i32` | 4 | `Sint32` | 48 | 4 |

Semantics:

- `bounds` — `xy` = the glyph quad's origin in physical pixels, which is
  `glyph_position - subpixel_alignment.to_offset() + raster_bounds.origin()`.
  `zw` = the glyph's size **in the atlas**.
- `uv_bounds` — `xy` = the atlas region's UV origin, `zw` = its UV size.
- `color` — straight-alpha RGBA, used for mask glyphs and ignored for emoji.
- `is_emoji` — 0 or 1. Declared `i32` / `Sint32`, and flat-interpolated in the varying struct.

### Bind groups

| group | binding | resource | visibility |
| --- | --- | --- | --- |
| 0 | 0 | `var<uniform> uniforms: Uniforms` (16 bytes) | `VERTEX` |
| 1 | 0 | `texture_2d<f32>` — the atlas, `Rgba8Unorm` | `FRAGMENT` |
| 1 | 1 | `sampler` — `SamplerBindingType::Filtering` | `FRAGMENT` |

One bind group per atlas texture, so the draw loop is one `set_bind_group(1, …)` +
`draw_indexed` per (layer, atlas) pair. Group 0 is declared but only the vertex stage reads it;
`ShaderStages::VERTEX` is the correct visibility, and the binding must still appear in the
pipeline layout.

The sampler must be `FilterMode::Linear` for both `mag_filter` and `min_filter`, with the default
`ClampToEdge` address modes. Linear filtering is load-bearing: it is what turns the horizontal
subpixel offset into smooth positioning rather than a jump. It is also why the atlas allocator
must keep its 1px padding between entries — without it the filter bleeds a neighbouring glyph in
at the edges.

### Things a future reader will otherwise break

1. **`bounds.zw` must be the atlas region size, not the raster bounds.** They differ. If you pass
   the smaller raster bounds, the shader samples a sub-region of what was uploaded and every glyph
   gets a clipped edge. Warp carries a code comment about this at `glyph.rs:191`; it is the single
   most-reported way to get this pipeline subtly wrong.
2. **Snap y, never x.** `floor(pixel_pos.y)` puts the glyph on the pixel grid vertically so the
   sampler never interpolates the antialiasing ramp across scanlines. x is deliberately left
   subpixel, because the glyph was rasterized into one of three horizontal subpixel buckets and
   the CPU already subtracted `SubpixelAlignment::to_offset()` from the position. Snapping x too
   would undo the bucketing and make text shimmer as it scrolls; not snapping y makes it fuzzy.
3. **Mask coverage is read from `tex_color.r`.** The atlas is always `Rgba8Unorm`, so an 8-bit
   coverage mask must be byte-replicated into all four channels at upload time. Costs 4× the
   memory for masks; buys one texture format, one pipeline and one atlas for both masks and emoji.
4. **`enhance_contrast`'s `k` comes from the composited colour, not the input colour.** For masks
   those are identical. For emoji, `k` is computed from the sampled texel and then thrown away by
   the `max(contrasted, f32(in.is_emoji))` — emoji alpha is already final and must not be pushed
   through a contrast curve. Do not "simplify" the `max` away.

---

## Device limits

Both shaders are written to stay inside `wgpu::Limits::downlevel_webgl2_defaults()`, which is what
keeps the same WGSL running on Metal, Vulkan, D3D12 and GL:

| limit | budget | rect | glyph |
| --- | --- | --- | --- |
| `max_vertex_attributes` | 16 | 12 | 5 |
| `max_vertex_buffer_array_stride` | 255 | 144 | 52 |
| `max_inter_stage_shader_variables` | 15 | 12 | 3 |

Warp raises `max_inter_stage_shader_variables` to 15 explicitly because its rect shader uses 14
varyings. The trimmed shader uses 12, so Crook needs no limits bump at all — request
`downlevel_webgl2_defaults().using_resolution(adapter.limits())` and leave it alone.

The stride budget is the tight one. `RectData` has 111 bytes of headroom under the downlevel
255-byte vertex-buffer stride limit, which is roughly seven more `vec4<f32>` attributes. Warp
packs `(sigma, padding_factor)` into one `vec2` and `(dash_length, gap_x, gap_y)` into one `vec3`
to stay under the *attribute* count; if you add drop shadows back, copy that packing rather than
adding two attributes.
