//! Turning a [`Scene`] into one render pass.
//!
//! Two passes over the layer list, and the split between them is the whole
//! design. The first ([`Renderer::build_frame`]) walks every layer and appends
//! instance data into one `Vec` per pipeline, then uploads each `Vec` as one
//! buffer. The second ([`Frame::draw`]) opens a single render pass and walks
//! the layers again, this time setting a scissor rect from the layer's clip and
//! issuing draws that are nothing but index ranges into those two buffers.
//!
//! Doing it in that order is deliberate: building can fail — a lost device, a
//! rejected buffer — and it fails before a swapchain texture has been acquired,
//! so a failed frame never strands one.
//!
//! Ported from Warp's `crates/warpui/src/rendering/wgpu/renderer/frame.rs` and
//! `renderer.rs` (MIT), minus the image pipeline and frame capture.

use crookui_core::geometry::{RectF, Vector2F};
use crookui_core::platform::FontDb;
use crookui_core::scene::{Layer, Scene};

use super::resources::{Resources, WindowResources};
use super::util::with_error_scope;
use super::{Error, glyph, rect};

const ENCODER_DESCRIPTOR: wgpu::CommandEncoderDescriptor = wgpu::CommandEncoderDescriptor {
    label: Some("Frame encoder"),
};

/// The two pipelines, and the glyph atlas that outlives every frame.
pub struct Renderer {
    rect_pipeline: rect::Pipeline,
    glyph_pipeline: glyph::Pipeline,
}

impl Renderer {
    /// Builds both pipelines against a color target of `format`.
    ///
    /// `format` must match whatever the frame will actually be drawn into — the
    /// swapchain's format for a window, the texture's for an offscreen render.
    pub fn new(resources: &Resources, format: wgpu::TextureFormat) -> Self {
        Self {
            rect_pipeline: rect::Pipeline::new(resources, format),
            glyph_pipeline: glyph::Pipeline::new(resources, format),
        }
    }

    /// Builds every instance buffer `scene` needs.
    ///
    /// This is where glyphs are rasterized and uploaded to the atlas, so it is
    /// also the only part of drawing that touches `font_db`.
    pub fn build_frame<'a>(
        &'a mut self,
        scene: &'a Scene,
        resources: &Resources,
        font_db: &dyn FontDb,
    ) -> Result<Frame<'a>, Error> {
        let Self {
            rect_pipeline,
            glyph_pipeline,
        } = self;

        let (frame, error) = with_error_scope(&resources.device, || {
            let scale_factor = scene.scale_factor();
            let mut rect_state = rect::PerFrameState::default();
            let mut glyph_state = glyph::PerFrameState::default();

            let layers = scene
                .layers()
                .map(|layer| LayerState {
                    rect: rect_pipeline.initialize_for_layer(layer, scale_factor, &mut rect_state),
                    glyph: glyph_pipeline.initialize_for_layer(
                        layer,
                        scale_factor,
                        resources,
                        font_db,
                        &mut glyph_state,
                    ),
                    layer,
                })
                .collect();

            rect::Pipeline::finalize_per_frame_state(&mut rect_state, resources)?;
            glyph::Pipeline::finalize_per_frame_state(&mut glyph_state, resources)?;

            Ok(Frame {
                scale_factor,
                layers,
                rect_state,
                glyph_state,
                rect_pipeline,
                glyph_pipeline,
            })
        });

        match error {
            Some(error) => Err(error),
            None => frame,
        }
    }

    /// Draws `scene` into `window`'s next swapchain texture and presents it.
    ///
    /// `pre_present` runs after the work is submitted and before it is
    /// presented; on the winit path that is where `pre_present_notify` goes, so
    /// the compositor is told a frame is coming at the last possible moment.
    pub fn render(
        &mut self,
        scene: &Scene,
        window: &WindowResources,
        font_db: &dyn FontDb,
        pre_present: impl FnOnce(),
    ) -> Result<(), Error> {
        let size = window.size();
        if size.x() <= 0. || size.y() <= 0. {
            return Ok(());
        }

        let resources = &window.resources;
        let frame = self.build_frame(scene, resources, font_db)?;

        let surface_texture = window.get_surface_texture()?;
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                format: Some(surface_texture.texture.format()),
                ..Default::default()
            });

        let mut encoder = resources.device.create_command_encoder(&ENCODER_DESCRIPTOR);
        let (_, error) = with_error_scope(&resources.device, || {
            frame.draw(resources, &mut encoder, &view, size);
            resources.queue.submit(Some(encoder.finish()));
        });

        pre_present();

        match error {
            Some(error) => Err(error),
            // Presenting after a failed submit makes wgpu complain about
            // presenting without having submitted any work, which buries the
            // error that actually mattered.
            None => match with_error_scope(&resources.device, || {
                resources.queue.present(surface_texture);
            }) {
                (_, None) => Ok(()),
                (_, Some(error)) => Err(error),
            },
        }
    }
}

/// One frame's instance buffers, ready to be drawn.
pub struct Frame<'a> {
    scale_factor: f32,
    layers: Vec<LayerState<'a>>,
    rect_state: rect::PerFrameState,
    glyph_state: glyph::PerFrameState,
    rect_pipeline: &'a rect::Pipeline,
    glyph_pipeline: &'a glyph::Pipeline,
}

struct LayerState<'a> {
    layer: &'a Layer,
    rect: Option<rect::LayerState>,
    glyph: Option<glyph::LayerState>,
}

impl Frame<'_> {
    /// Encodes the whole frame as one render pass into `view`.
    ///
    /// `target_size` is the view's size in physical pixels; it becomes the
    /// shaders' viewport and the bound the scissor rects are clipped against,
    /// so it must be the texture's real size and not the window's logical one.
    pub fn draw(
        self,
        resources: &Resources,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target_size: Vector2F,
    ) {
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Frame pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        resources.configure_render_pass(&mut render_pass, target_size);

        let target_bounds = RectF::new(Vector2F::zero(), target_size);

        for state in &self.layers {
            match state.layer.clip_bounds {
                Some(bounds) => {
                    let scaled = RectF::new(
                        bounds.origin().scale(self.scale_factor),
                        bounds.size().scale(self.scale_factor),
                    );
                    // A layer whose clip is entirely off-screen draws nothing
                    // at all, rather than drawing unclipped.
                    let Some(visible) = scaled.intersection(target_bounds) else {
                        continue;
                    };
                    set_scissor_rect(&mut render_pass, visible);
                }
                None => set_scissor_rect(&mut render_pass, target_bounds),
            }

            // Fixed order within a layer: text is always drawn over the
            // rectangles of its own layer, including the background and
            // underline rects a styled run emits.
            if let Some(rect_state) = &state.rect {
                self.rect_pipeline
                    .draw(&mut render_pass, rect_state, &self.rect_state);
            }
            if let Some(glyph_state) = &state.glyph {
                self.glyph_pipeline
                    .draw(&mut render_pass, glyph_state, &self.glyph_state);
            }
        }
    }
}

/// Sets a scissor rect, rounding in the one way that cannot overflow the target.
fn set_scissor_rect(render_pass: &mut wgpu::RenderPass<'_>, bounds: RectF) {
    // Round the two corners independently and derive the extent from them.
    // Rounding the origin and the size separately can push the far edge past
    // the surface when both round up, which wgpu rejects.
    let x = bounds.min_x().round() as u32;
    let y = bounds.min_y().round() as u32;
    let width = (bounds.max_x().round() as u32).saturating_sub(x);
    let height = (bounds.max_y().round() as u32).saturating_sub(y);

    // A zero-dimension scissor rect trips a runtime assertion inside wgpu
    // rather than clipping everything away. See gfx-rs/wgpu#1750.
    if width != 0 && height != 0 {
        render_pass.set_scissor_rect(x, y, width, height);
    }
}

#[cfg(test)]
mod tests {
    use crookui_core::geometry::vec2f;

    use super::*;

    /// A stand-in for `set_scissor_rect`'s arithmetic, so the rounding rule can
    /// be checked without a GPU.
    fn scissor(bounds: RectF) -> (u32, u32, u32, u32) {
        let x = bounds.min_x().round() as u32;
        let y = bounds.min_y().round() as u32;
        (
            x,
            y,
            (bounds.max_x().round() as u32).saturating_sub(x),
            (bounds.max_y().round() as u32).saturating_sub(y),
        )
    }

    #[test]
    fn rounding_corners_independently_keeps_the_rect_inside_the_target() {
        // Origin 0.6 and size 99.6 round to 1 and 100, which would reach 101 on
        // a 100px surface. Rounding the far corner instead gives 100.
        let bounds = RectF::new(vec2f(0.6, 0.), vec2f(99.6, 50.));
        let (x, _, width, _) = scissor(bounds);

        assert_eq!(x, 1);
        assert_eq!(x + width, 100);
    }
}
