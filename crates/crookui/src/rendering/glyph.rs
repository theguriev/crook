//! The glyph pipeline: one instanced quad per glyph, sampling an atlas.
//!
//! Ported from Warp's `crates/warpui/src/rendering/wgpu/renderer/glyph.rs` and
//! `texture_with_bind_group.rs` (MIT), minus the horizontal fade — Crook
//! truncates overflowing text with an ellipsis, which is a layout decision that
//! needs nothing from the shader.
//!
//! Pictures are drawn here too, as the same quad with the emoji flag set: the
//! shader already passes a colour texel through untouched for an emoji, and a
//! picture is nothing but colour texels. What they need that a glyph does not
//! is atlases of their own, so [`Sheet`] says which set a batch samples from.
//!
//! The instance layout here is the Rust half of the contract in
//! `src/shaders/README.md`; the two must be changed together.

use std::mem;

use crookui_core::fonts::{Canvas, SubpixelAlignment};
use crookui_core::geometry::{Color, vec2f};
use crookui_core::platform::FontDb;
use crookui_core::scene::Layer;
use wgpu::util::BufferInitDescriptor;

use super::Error;
use super::atlas::{AllocatedRegion, TextureId};
use super::glyph_cache::GlyphCache;
use super::resources::{QUAD_INDICES, Resources, quad_vertex_layout};
use super::util::create_buffer_init;

/// What minting and filling an atlas texture takes.
///
/// Bundled because the glyph cache needs all four together and none of them
/// separately, and because it keeps the cache's signature to five parameters
/// instead of eight.
pub struct AtlasContext<'a> {
    /// The device that owns the atlas textures.
    pub device: &'a wgpu::Device,
    /// The queue glyph uploads are written on.
    pub queue: &'a wgpu::Queue,
    /// The layout of bind group 1: the atlas texture and its sampler.
    pub bind_group_layout: &'a wgpu::BindGroupLayout,
    /// The sampler shared by every atlas.
    pub sampler: &'a wgpu::Sampler,
}

/// One atlas texture and the bind group that binds it.
///
/// The bind group is built once with the texture rather than once per frame,
/// which is the whole reason these two live in the same struct.
pub struct AtlasTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

impl AtlasTexture {
    /// Allocates a `size` × `size` RGBA8 atlas and its bind group.
    pub fn new(size: u32, ctx: &AtlasContext<'_>) -> Self {
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Glyph atlas"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Glyph atlas bind group"),
            layout: ctx.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(ctx.sampler),
                },
            ],
        });

        Self {
            texture,
            bind_group,
        }
    }

    /// Uploads one rasterized bitmap into `region`.
    ///
    /// A glyph, a colour emoji or an icon: by the time anything reaches here
    /// it is pixels in the atlas's own format, and the atlas does not care
    /// which of the three it was.
    pub fn insert(&mut self, region: AllocatedRegion, canvas: &Canvas, queue: &wgpu::Queue) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: region.pixel_region.x,
                    y: region.pixel_region.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &canvas.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                // The rasterizer is free to pad its rows, so the stride it
                // reports is the authority, not width times four.
                bytes_per_row: Some(canvas.row_stride as u32),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: region.pixel_region.width,
                height: region.pixel_region.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// The bind group holding this atlas and its sampler.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }
}

/// The glyph pipeline, its atlas cache and its sampler.
pub(super) struct Pipeline {
    glyph_cache: GlyphCache,
    render_pipeline: wgpu::RenderPipeline,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

/// Every glyph in the frame, in one buffer.
#[derive(Default)]
pub(super) struct PerFrameState {
    data: Vec<GlyphInstanceData>,
    buffer: Option<wgpu::Buffer>,
}

/// One layer's pictures and glyphs, grouped into a draw per atlas texture.
pub(super) struct LayerState {
    textures: Vec<PerTextureState>,
}

/// Which atlas a batch samples: a glyph atlas or an image atlas.
///
/// The two sets are numbered independently, so a texture id alone is
/// ambiguous and the batch has to say which set it means.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum Sheet {
    /// A glyph atlas: masks, colour emoji and icons.
    Marks(TextureId),
    /// An image atlas: pictures only.
    Images(TextureId),
}

struct PerTextureState {
    sheet: Sheet,
    start_offset: usize,
    len: usize,
}

impl Pipeline {
    /// Builds the pipeline against a color target of `format`.
    pub(super) fn new(resources: &Resources, format: wgpu::TextureFormat) -> Self {
        let device = &resources.device;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Glyph shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/glyph_shader.wgsl").into()),
        });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Glyph atlas bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        // Filtering, to match `filterable: true` above.
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Glyph pipeline layout"),
            bind_group_layouts: &[
                Some(resources.uniform_bind_group_layout()),
                Some(&texture_bind_group_layout),
            ],
            immediate_size: 0,
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Glyph pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    Some(quad_vertex_layout()),
                    Some(GlyphInstanceData::layout()),
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // The shader emits straight, not premultiplied, alpha.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::all(),
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            // Desktop drivers keep their own shader caches, so a pipeline cache
            // would buy nothing on any platform Crook targets.
            cache: None,
        });

        // Linear filtering is load-bearing: it is what turns a glyph's
        // horizontal subpixel offset into smooth positioning instead of a jump.
        // It is also why the atlas allocator pads between entries.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Glyph atlas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            glyph_cache: GlyphCache::new(),
            render_pipeline,
            texture_bind_group_layout,
            sampler,
        }
    }

    /// Lets the cache drop its image atlases if they have grown past their
    /// budget. Run once per frame, before any layer is walked.
    pub(super) fn sweep_images(&mut self) {
        self.glyph_cache.sweep_images();
    }

    /// Appends `layer`'s pictures and glyphs to the frame's instance data.
    ///
    /// Returns `None` when the layer draws neither, which keeps a zero-length
    /// draw — and, on some backends, a zero-sized buffer — out of the frame.
    pub(super) fn initialize_for_layer(
        &mut self,
        layer: &Layer,
        scale_factor: f32,
        resources: &Resources,
        font_db: &dyn FontDb,
        per_frame_state: &mut PerFrameState,
    ) -> Option<LayerState> {
        if layer.images.is_empty() && layer.glyphs.is_empty() && layer.icons.is_empty() {
            return None;
        }

        let atlas = AtlasContext {
            device: &resources.device,
            queue: &resources.queue,
            bind_group_layout: &self.texture_bind_group_layout,
            sampler: &self.sampler,
        };

        // Grouped in first-seen order rather than through a hash map, so a
        // frame drawn twice produces the same command stream — which is what
        // makes the offscreen path comparable against a golden image.
        let mut batches: Vec<(Sheet, Vec<GlyphInstanceData>)> = Vec::new();
        let mut batch = |sheet, instance| match batches.iter_mut().find(|(seen, _)| *seen == sheet)
        {
            Some((_, instances)) => instances.push(instance),
            None => batches.push((sheet, vec![instance])),
        };

        // Pictures first, so a caption drawn over one in the same layer wins.
        // A picture is an emoji as far as the shader knows: its texel is
        // emitted unchanged and the tint is ignored. The quad is the drawn
        // device size; the region is normally the same size, resampled to it
        // on the CPU, and only past the atlas ceiling is the sampler asked to
        // stretch a smaller region up.
        for image in &layer.images {
            let origin = image.bounds.origin() * scale_factor;
            let size = image.bounds.size() * scale_factor;
            let drawn = (
                size.x().round().max(0.) as u32,
                size.y().round().max(0.) as u32,
            );

            let placed = match self.glyph_cache.get_image(&image.bitmap, drawn, &atlas) {
                Ok(Some(placed)) => placed,
                Ok(None) => continue,
                Err(error) => {
                    log::warn!("could not place image {:?}: {error:#}", image.bitmap);
                    continue;
                }
            };

            let region = placed.allocated_region.pixel_region;
            let uv_bounds = if (region.width, region.height) == drawn {
                rect_to_array(placed.allocated_region.uv_region)
            } else {
                inset_half_texel(placed.allocated_region)
            };
            let instance = GlyphInstanceData {
                bounds: [
                    origin.x().round(),
                    origin.y().round(),
                    drawn.0 as f32,
                    drawn.1 as f32,
                ],
                uv_bounds,
                color: Color::WHITE.to_f32_array(),
                is_emoji: true as i32,
            };
            batch(Sheet::Images(placed.texture_id), instance);
        }

        for glyph in &layer.glyphs {
            let position = glyph.position * scale_factor;
            let subpixel_alignment = SubpixelAlignment::new(position);

            let placed = match self.glyph_cache.get(
                glyph.glyph_key,
                scale_factor,
                subpixel_alignment,
                font_db,
                &atlas,
            ) {
                Ok(Some(placed)) => placed,
                Ok(None) => continue,
                Err(error) => {
                    log::warn!("could not place glyph {:?}: {error:#}", glyph.glyph_key);
                    continue;
                }
            };

            // The glyph was rasterized already shifted into its subpixel
            // bucket, so the fraction of a pixel that bucket stands for has to
            // come back off the position or it would be applied twice.
            let origin = position - subpixel_alignment.to_offset()
                + vec2f(
                    placed.raster_bounds.left as f32,
                    // `top` counts upwards from the baseline; screen y counts down.
                    -(placed.raster_bounds.top as f32),
                );

            let region = placed.allocated_region.pixel_region;
            let instance = GlyphInstanceData {
                bounds: [
                    origin.x(),
                    origin.y(),
                    region.width as f32,
                    region.height as f32,
                ],
                uv_bounds: rect_to_array(placed.allocated_region.uv_region),
                color: glyph.color.to_f32_array(),
                is_emoji: placed.is_emoji as i32,
            };
            batch(Sheet::Marks(placed.texture_id), instance);
        }

        // Icons after glyphs, into the same batches: an icon is a mask in the
        // same atlas, drawn by the same shader with the same tint, and the
        // only thing it does differently is that it is placed by its box
        // rather than by a baseline and snapped to whole pixels on both axes.
        for icon in &layer.icons {
            let placed = match self
                .glyph_cache
                .get_icon(icon.icon_key, scale_factor, &atlas)
            {
                Ok(Some(placed)) => placed,
                Ok(None) => continue,
                Err(error) => {
                    log::warn!("could not place icon {:?}: {error:#}", icon.icon_key);
                    continue;
                }
            };

            let position = icon.bounds.origin() * scale_factor;
            let region = placed.allocated_region.pixel_region;
            let instance = GlyphInstanceData {
                bounds: [
                    position.x().round(),
                    position.y().round(),
                    region.width as f32,
                    region.height as f32,
                ],
                uv_bounds: rect_to_array(placed.allocated_region.uv_region),
                color: icon.color.to_f32_array(),
                is_emoji: false as i32,
            };
            batch(Sheet::Marks(placed.texture_id), instance);
        }

        if batches.is_empty() {
            return None;
        }

        let mut start_offset = per_frame_state.data.len();
        let textures = batches
            .into_iter()
            .map(|(sheet, mut instances)| {
                let len = instances.len();
                per_frame_state.data.append(&mut instances);
                let state = PerTextureState {
                    sheet,
                    start_offset,
                    len,
                };
                start_offset += len;
                state
            })
            .collect();

        Some(LayerState { textures })
    }

    /// Uploads the whole frame's glyphs as one buffer.
    pub(super) fn finalize_per_frame_state(
        per_frame_state: &mut PerFrameState,
        resources: &Resources,
    ) -> Result<(), Error> {
        if per_frame_state.data.is_empty() {
            return Ok(());
        }

        per_frame_state.buffer = Some(create_buffer_init(
            &resources.device,
            &resources.device_lost,
            &BufferInitDescriptor {
                label: Some("Glyph instance buffer"),
                contents: bytemuck::cast_slice(&per_frame_state.data),
                usage: wgpu::BufferUsages::VERTEX,
            },
        )?);

        Ok(())
    }

    /// Draws one layer's glyphs: one bind-group switch and one draw per atlas.
    pub(super) fn draw(
        &self,
        render_pass: &mut wgpu::RenderPass<'_>,
        layer_state: &LayerState,
        per_frame_state: &PerFrameState,
    ) {
        let Some(buffer) = per_frame_state.buffer.as_ref() else {
            return;
        };

        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_vertex_buffer(1, buffer.slice(..));

        for state in &layer_state.textures {
            let texture = match state.sheet {
                Sheet::Marks(texture_id) => self.glyph_cache.texture(texture_id),
                Sheet::Images(texture_id) => self.glyph_cache.image_texture(texture_id),
            };
            let Some(texture) = texture else {
                log::warn!(
                    "a layer referenced atlas {:?}, which does not exist",
                    state.sheet
                );
                continue;
            };

            render_pass.set_bind_group(1, texture.bind_group(), &[]);
            let end = state.start_offset + state.len;
            render_pass.draw_indexed(
                0..QUAD_INDICES.len() as u32,
                0,
                state.start_offset as u32..end as u32,
            );
        }
    }
}

fn rect_to_array(rect: crookui_core::geometry::RectF) -> [f32; 4] {
    [
        rect.origin().x(),
        rect.origin().y(),
        rect.size().x(),
        rect.size().y(),
    ]
}

/// `region`'s UV rect pulled in by half a texel on every side, for a quad
/// that is not the region's size.
///
/// A quad drawn one to one samples every texel at its centre and never
/// touches the padding around the region. A stretched quad does not: its
/// outermost fragments land within half a texel of the region's edge, and the
/// linear filter blends the transparent padding in, fringing the picture. With
/// the edges pulled in, the outermost fragments sample the outermost texel
/// centres instead. Not applied one to one, where it would put every sample
/// between two texels and blur the whole picture.
fn inset_half_texel(region: AllocatedRegion) -> [f32; 4] {
    let uv = region.uv_region;
    let pixels = region.pixel_region;
    let half_x = uv.width() / pixels.width as f32 * 0.5;
    let half_y = uv.height() / pixels.height as f32 * 0.5;
    [
        uv.origin().x() + half_x,
        uv.origin().y() + half_y,
        uv.width() - 2. * half_x,
        uv.height() - 2. * half_y,
    ]
}

/// One glyph, as vertex slot 1 sees it.
///
/// Field order and types are the contract in `src/shaders/README.md`: four
/// attributes at locations 1 through 4, stride 52.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct GlyphInstanceData {
    /// Quad origin in physical pixels, and the glyph's size *in the atlas*.
    bounds: [f32; 4],
    /// The atlas region in UV space.
    uv_bounds: [f32; 4],
    /// Text color, for mask glyphs.
    color: [f32; 4],
    /// 1 for a color glyph or a picture, whose own pixels are used instead.
    is_emoji: i32,
}

impl GlyphInstanceData {
    const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        1 => Float32x4, // bounds
        2 => Float32x4, // uv_bounds
        3 => Float32x4, // color
        4 => Sint32,    // is_emoji
    ];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

#[cfg(test)]
mod tests {
    use crookui_core::geometry::RectF;

    use super::super::atlas::PixelRect;
    use super::*;

    #[test]
    fn the_instance_layout_matches_the_shader_contract() {
        assert_eq!(mem::size_of::<GlyphInstanceData>(), 52);
        assert_eq!(GlyphInstanceData::layout().array_stride, 52);
    }

    #[test]
    fn a_stretched_quad_samples_from_half_a_texel_inside_its_region() {
        // A 4 × 2 region in a 16 px atlas: a texel is a sixteenth of UV space,
        // so each edge moves in by a thirty-second.
        let region = AllocatedRegion {
            uv_region: RectF::new(vec2f(0., 0.5), vec2f(0.25, 0.125)),
            pixel_region: PixelRect {
                x: 0,
                y: 8,
                width: 4,
                height: 2,
            },
        };

        assert_eq!(
            inset_half_texel(region),
            [1. / 32., 0.5 + 1. / 32., 0.25 - 1. / 16., 0.125 - 1. / 16.]
        );
    }
}
