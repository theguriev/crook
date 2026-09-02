//! The rect pipeline: one instanced quad per rectangle.
//!
//! Every non-text pixel goes through here — tab backgrounds, the usage chip,
//! dividers, focus rings, and the background and underline rects that
//! [`Line::paint`](crookui_core::text_layout::Line::paint) emits for styled
//! text. Rounded corners, gradient fills and per-side gradient borders are all
//! evaluated in the fragment shader, so the CPU side is nothing but a struct
//! per rect.
//!
//! Ported from Warp's `crates/warpui/src/rendering/wgpu/renderer/rect.rs`
//! (MIT), minus drop shadows and dashed borders, which the ported shader does
//! not implement either.
//!
//! The instance layout here is the Rust half of the contract in
//! `src/shaders/README.md`; the two must be changed together.

use std::mem;

use crookui_core::geometry::{Color, RectF};
use crookui_core::scene::{Border, CornerRadius, Fill, Layer, Radius};
use wgpu::util::BufferInitDescriptor;

use super::Error;
use super::resources::{QUAD_INDICES, Resources, quad_vertex_layout};
use super::util::create_buffer_init;

/// The rect pipeline.
pub(super) struct Pipeline {
    render_pipeline: wgpu::RenderPipeline,
}

/// Every rect in the frame, in one buffer.
#[derive(Default)]
pub(super) struct PerFrameState {
    data: Vec<RectData>,
    buffer: Option<wgpu::Buffer>,
}

/// One layer's slice of that buffer.
pub(super) struct LayerState {
    start_offset: usize,
    len: usize,
}

impl Pipeline {
    /// Builds the pipeline against a color target of `format`.
    pub(super) fn new(resources: &Resources, format: wgpu::TextureFormat) -> Self {
        let device = &resources.device;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Rect shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/rect_shader.wgsl").into()),
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Rect pipeline layout"),
            bind_group_layouts: &[Some(resources.uniform_bind_group_layout())],
            immediate_size: 0,
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Rect pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(quad_vertex_layout()), Some(RectData::layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("rect_fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // The shader's border-over-background composite divides the
                    // summed color back out by alpha, so what it emits is
                    // straight alpha. Premultiplied blending here would wash
                    // out everything with a semi-transparent border.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::all(),
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self { render_pipeline }
    }

    /// Appends `layer`'s rects to the frame's instance data.
    ///
    /// Returns `None` for a layer with no rects, so the frame never issues a
    /// zero-instance draw.
    pub(super) fn initialize_for_layer(
        &self,
        layer: &Layer,
        scale_factor: f32,
        per_frame_state: &mut PerFrameState,
    ) -> Option<LayerState> {
        if layer.rects.is_empty() {
            return None;
        }

        let start_offset = per_frame_state.data.len();
        per_frame_state.data.extend(layer.rects.iter().map(|rect| {
            let bounds = scale_rect(rect.bounds, scale_factor);
            RectData::new(
                bounds,
                rect.background,
                rect.border,
                rect.corner_radius,
                scale_factor,
            )
        }));

        Some(LayerState {
            start_offset,
            len: layer.rects.len(),
        })
    }

    /// Uploads the whole frame's rects as one buffer.
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
                label: Some("Rect instance buffer"),
                contents: bytemuck::cast_slice(&per_frame_state.data),
                usage: wgpu::BufferUsages::VERTEX,
            },
        )?);

        Ok(())
    }

    /// Draws one layer's rects as a single instanced draw.
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

        let end = layer_state.start_offset + layer_state.len;
        render_pass.draw_indexed(
            0..QUAD_INDICES.len() as u32,
            0,
            layer_state.start_offset as u32..end as u32,
        );
    }
}

fn scale_rect(rect: RectF, scale_factor: f32) -> RectF {
    RectF::new(
        rect.origin().scale(scale_factor),
        rect.size().scale(scale_factor),
    )
}

/// One rect, as vertex slot 1 sees it.
///
/// Field order and types are the contract in `src/shaders/README.md`: eleven
/// attributes at locations 1 through 11, stride 144.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct RectData {
    /// Origin and size in physical pixels.
    bounds: [f32; 4],
    background_start: [f32; 2],
    background_start_color: [f32; 4],
    background_end: [f32; 2],
    background_end_color: [f32; 4],
    /// Top, right, bottom, left, in physical pixels.
    border_width: [f32; 4],
    border_start: [f32; 2],
    border_start_color: [f32; 4],
    border_end: [f32; 2],
    border_end_color: [f32; 4],
    /// Top-left, top-right, bottom-left, bottom-right — not the CSS order.
    corner_radius: [f32; 4],
}

impl RectData {
    const ATTRIBUTES: [wgpu::VertexAttribute; 11] = wgpu::vertex_attr_array![
        // Location 0 is the shared quad's vertex position.
        1 => Float32x4,  // bounds
        2 => Float32x2,  // background_start
        3 => Float32x4,  // background_start_color
        4 => Float32x2,  // background_end
        5 => Float32x4,  // background_end_color
        6 => Float32x4,  // border_width
        7 => Float32x2,  // border_start
        8 => Float32x4,  // border_start_color
        9 => Float32x2,  // border_end
        10 => Float32x4, // border_end_color
        11 => Float32x4, // corner_radius
    ];

    fn new(
        bounds: RectF,
        background: Fill,
        border: Border,
        corner_radius: CornerRadius,
        scale_factor: f32,
    ) -> Self {
        let min_dimension = bounds.width().min(bounds.height());
        let radius = |radius: Radius| {
            let pixels = match radius {
                Radius::Pixels(pixels) => pixels * scale_factor,
                Radius::Percentage(percent) => percent / 100. * min_dimension,
            };
            // Nothing downstream clamps this. Past half the shorter side the
            // inner and outer signed-distance fields cross and the border turns
            // itself inside out.
            pixels.clamp(0., min_dimension / 2.)
        };

        Self {
            bounds: [
                bounds.origin().x(),
                bounds.origin().y(),
                bounds.size().x(),
                bounds.size().y(),
            ],
            background_start: to_array2(background.start()),
            background_start_color: color(background.start_color()),
            background_end: to_array2(background.end()),
            background_end_color: color(background.end_color()),
            border_width: [
                border.top_width() * scale_factor,
                border.right_width() * scale_factor,
                border.bottom_width() * scale_factor,
                border.left_width() * scale_factor,
            ],
            border_start: to_array2(border.color.start()),
            border_start_color: color(border.color.start_color()),
            border_end: to_array2(border.color.end()),
            border_end_color: color(border.color.end_color()),
            corner_radius: [
                radius(corner_radius.get_top_left()),
                radius(corner_radius.get_top_right()),
                radius(corner_radius.get_bottom_left()),
                radius(corner_radius.get_bottom_right()),
            ],
        }
    }

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

fn to_array2(vector: crookui_core::geometry::Vector2F) -> [f32; 2] {
    [vector.x(), vector.y()]
}

fn color(color: Color) -> [f32; 4] {
    color.to_f32_array()
}

#[cfg(test)]
mod tests {
    use crookui_core::geometry::vec2f;

    use super::*;

    fn rect_data(bounds: RectF, corner_radius: CornerRadius, scale_factor: f32) -> RectData {
        RectData::new(
            scale_rect(bounds, scale_factor),
            Fill::Solid(Color::WHITE),
            Border::default(),
            corner_radius,
            scale_factor,
        )
    }

    #[test]
    fn the_instance_layout_matches_the_shader_contract() {
        assert_eq!(mem::size_of::<RectData>(), 144);
        assert_eq!(RectData::layout().array_stride, 144);
    }

    #[test]
    fn a_percentage_radius_is_half_the_shorter_side_at_fifty_percent() {
        let data = rect_data(
            RectF::new(vec2f(0., 0.), vec2f(80., 20.)),
            CornerRadius::with_all(Radius::Percentage(50.)),
            1.,
        );

        assert_eq!(data.corner_radius, [10., 10., 10., 10.]);
    }

    #[test]
    fn a_pixel_radius_scales_and_clamps_to_half_the_shorter_side() {
        let data = rect_data(
            RectF::new(vec2f(0., 0.), vec2f(40., 10.)),
            CornerRadius::with_all(Radius::Pixels(4.)),
            2.,
        );

        // 4 logical px at 2x is 8 physical px, which is half of the 20px height.
        assert_eq!(data.corner_radius, [8., 8., 8., 8.]);

        let clamped = rect_data(
            RectF::new(vec2f(0., 0.), vec2f(40., 10.)),
            CornerRadius::with_all(Radius::Pixels(30.)),
            1.,
        );
        assert_eq!(clamped.corner_radius, [5., 5., 5., 5.]);
    }

    #[test]
    fn border_widths_are_zero_on_the_sides_that_are_off() {
        let data = RectData::new(
            RectF::new(vec2f(0., 0.), vec2f(40., 20.)),
            Fill::None,
            Border::top(2.).with_border_color(Color::BLACK),
            CornerRadius::default(),
            1.,
        );

        assert_eq!(data.border_width, [2., 0., 0., 0.]);
    }

    #[test]
    fn a_flat_fill_still_gets_a_non_degenerate_gradient_axis() {
        let data = rect_data(
            RectF::new(vec2f(0., 0.), vec2f(40., 20.)),
            CornerRadius::default(),
            1.,
        );

        // `derive_color` divides by the squared axis length, so start and end
        // must never coincide even when the two colors do.
        assert_ne!(data.background_start, data.background_end);
        assert_eq!(data.background_start_color, data.background_end_color);
    }
}
