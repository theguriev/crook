//! The glyph pipeline: one instanced quad per glyph, sampling an atlas.
//!
//! Ported from Warp's `crates/warpui/src/rendering/wgpu/renderer/glyph.rs` and
//! `texture_with_bind_group.rs` (MIT), minus the horizontal fade — Crook
//! truncates overflowing text with an ellipsis, which is a layout decision that
//! needs nothing from the shader.
//!
//! The instance layout here is the Rust half of the contract in
//! `src/shaders/README.md`; the two must be changed together.

use std::mem;

use crookui_core::fonts::{RasterizedGlyph, SubpixelAlignment};
use crookui_core::geometry::vec2f;
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

    /// Uploads a rasterized glyph into `region`.
    pub fn insert_glyph(
        &mut self,
        region: AllocatedRegion,
        glyph: &RasterizedGlyph,
        queue: &wgpu::Queue,
    ) {
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
            &glyph.canvas.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                // The rasterizer is free to pad its rows, so the stride it
                // reports is the authority, not width times four.
                bytes_per_row: Some(glyph.canvas.row_stride as u32),
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

/// One layer's glyphs, grouped into a draw per atlas texture.
pub(super) struct LayerState {
    textures: Vec<PerTextureState>,
}

struct PerTextureState {
    texture_id: TextureId,
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

    /// Appends `layer`'s glyphs to the frame's instance data.
    ///
    /// Returns `None` when the layer draws no glyphs, which keeps a zero-length
    /// draw — and, on some backends, a zero-sized buffer — out of the frame.
    pub(super) fn initialize_for_layer(
        &mut self,
        layer: &Layer,
        scale_factor: f32,
        resources: &Resources,
        font_db: &dyn FontDb,
        per_frame_state: &mut PerFrameState,
    ) -> Option<LayerState> {
        if layer.glyphs.is_empty() {
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
        let mut batches: Vec<(TextureId, Vec<GlyphInstanceData>)> = Vec::new();

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

            match batches
                .iter_mut()
                .find(|(texture_id, _)| *texture_id == placed.texture_id)
            {
                Some((_, instances)) => instances.push(instance),
                None => batches.push((placed.texture_id, vec![instance])),
            }
        }

        if batches.is_empty() {
            return None;
        }

        let mut start_offset = per_frame_state.data.len();
        let textures = batches
            .into_iter()
            .map(|(texture_id, mut instances)| {
                let len = instances.len();
                per_frame_state.data.append(&mut instances);
                let state = PerTextureState {
                    texture_id,
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
            let Some(texture) = self.glyph_cache.texture(state.texture_id) else {
                log::warn!(
                    "a layer referenced atlas {:?}, which does not exist",
                    state.texture_id
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
    /// 1 for a color glyph, whose own pixels are used instead.
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
    use super::*;

    #[test]
    fn the_instance_layout_matches_the_shader_contract() {
        assert_eq!(mem::size_of::<GlyphInstanceData>(), 52);
        assert_eq!(GlyphInstanceData::layout().array_stride, 52);
    }
}
