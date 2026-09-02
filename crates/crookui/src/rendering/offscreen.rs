//! Rendering a [`Scene`] to a texture instead of a window.
//!
//! This is a first-class path, not a test hack. It is how the renderer is
//! verified on a machine with no display and in CI: build a scene, render it,
//! assert on pixels. It shares every pipeline, buffer and shader with the
//! window path — the only differences are that the color target is a texture
//! this module owns rather than a swapchain image, and that the device is
//! opened without a compatible surface.
//!
//! The format is always `Rgba8Unorm`, so the bytes that come back need no
//! channel swizzle. A window's swapchain is frequently BGRA; a caller reading
//! *that* back would have to swap bytes 0 and 2.

use anyhow::{Context, Result};
use crookui_core::geometry::Vector2F;
use crookui_core::platform::FontDb;
use crookui_core::scene::Scene;

use super::frame::Renderer;
use super::resources::Resources;

/// The pixel format every offscreen render produces.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Renders `scene` into a fresh texture and reads it back as RGBA8.
///
/// `size` is the *logical* size to render at, the same number a window of that
/// size would report; the texture is `size * scene.scale_factor()` physical
/// pixels, and that is the size returned alongside the bytes. Rows come back
/// tightly packed at four bytes per pixel, top row first.
///
/// This opens a device per call, which costs tens of milliseconds. Use
/// [`Offscreen`] directly to render many scenes against one.
pub fn render_scene_to_rgba(
    scene: &Scene,
    size: Vector2F,
    font_db: &dyn FontDb,
) -> Result<(Vec<u8>, u32, u32)> {
    let physical = physical_size(size, scene.scale_factor());
    let mut offscreen = Offscreen::new(physical)?;
    let pixels = offscreen.render(scene, font_db)?;

    Ok((pixels, physical.0, physical.1))
}

/// A device, a renderer and one texture to draw into, reusable across scenes.
pub struct Offscreen {
    resources: Resources,
    renderer: Renderer,
    texture: wgpu::Texture,
    size: (u32, u32),
}

impl Offscreen {
    /// Opens a device with no surface and allocates a `size` physical-pixel
    /// render target.
    ///
    /// Both dimensions are raised to at least one: a zero-sized texture is a
    /// validation error on every backend.
    pub fn new(size: (u32, u32)) -> Result<Self> {
        let size = (size.0.max(1), size.1.max(1));
        let resources = Resources::new(None)?;
        let renderer = Renderer::new(&resources, FORMAT);

        let texture = resources.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Offscreen target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        Ok(Self {
            resources,
            renderer,
            texture,
            size,
        })
    }

    /// The target's size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Draws `scene` and reads the result back as tightly packed RGBA8.
    ///
    /// The scene's own scale factor is used, so a scene built for a 2x display
    /// must be rendered into a target twice its logical size or it will be
    /// clipped.
    pub fn render(&mut self, scene: &Scene, font_db: &dyn FontDb) -> Result<Vec<u8>> {
        let (width, height) = self.size;
        let device = &self.resources.device;

        let frame = self
            .renderer
            .build_frame(scene, &self.resources, font_db)
            .context("failed to build the frame's instance buffers")?;

        let view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Offscreen encoder"),
        });
        frame.draw(
            &self.resources,
            &mut encoder,
            &view,
            Vector2F::new(width as f32, height as f32),
        );

        // A texture-to-buffer copy must pad each row up to a 256-byte boundary,
        // so the staging buffer is wider than the image and the padding is
        // stripped on the way out.
        let unpadded_bytes_per_row = width * 4;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Offscreen readback"),
            size: (padded_bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.resources.queue.submit(Some(encoder.finish()));

        let (sender, receiver) = std::sync::mpsc::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .context("the device stopped responding while reading the frame back")?;
        receiver
            .recv()
            .context("the buffer mapping callback never ran")?
            .context("failed to map the readback buffer")?;

        let mapped = staging
            .slice(..)
            .get_mapped_range()
            .context("failed to read the mapped readback buffer")?;
        let mut pixels = Vec::with_capacity((unpadded_bytes_per_row * height) as usize);
        for row in 0..height {
            let start = (row * padded_bytes_per_row) as usize;
            pixels.extend_from_slice(&mapped[start..start + unpadded_bytes_per_row as usize]);
        }
        drop(mapped);
        staging.unmap();

        Ok(pixels)
    }
}

fn physical_size(logical: Vector2F, scale_factor: f32) -> (u32, u32) {
    (
        (logical.x() * scale_factor).round().max(1.) as u32,
        (logical.y() * scale_factor).round().max(1.) as u32,
    )
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, bail};
    use crookui_core::fonts::{
        FamilyId, FontId, GlyphId, GlyphKey, Metrics, RasterBounds, RasterizedGlyph,
        SubpixelAlignment,
    };
    use crookui_core::geometry::{Color, RectF, vec2f};
    use crookui_core::scene::Radius;

    use super::*;

    /// A font backend for scenes that draw no text.
    ///
    /// Every method fails loudly rather than returning something plausible: if
    /// one of them is ever reached, the test is drawing a glyph it did not mean
    /// to and should say so instead of rendering a blank.
    struct NoFonts;

    impl FontDb for NoFonts {
        fn load_family_from_bytes(&mut self, name: &str, _: Vec<Vec<u8>>) -> Result<FamilyId> {
            bail!("this test draws no text, but a font family named {name} was loaded")
        }

        fn font_metrics(&self, font_id: FontId) -> Metrics {
            panic!("this test draws no text, but metrics for {font_id:?} were asked for")
        }

        fn glyph_advance(&self, _: FontId, glyph_id: GlyphId) -> Result<Vector2F> {
            bail!("this test draws no text, but the advance of glyph {glyph_id} was asked for")
        }

        fn glyph_raster_bounds(&self, key: GlyphKey, _: Vector2F) -> Result<RasterBounds> {
            bail!("this test draws no text, but glyph {key:?} reached the atlas")
        }

        fn rasterize_glyph(
            &self,
            key: GlyphKey,
            _: Vector2F,
            _: SubpixelAlignment,
            _: crookui_core::fonts::RasterFormat,
        ) -> Result<RasterizedGlyph> {
            bail!("this test draws no text, but glyph {key:?} was rasterized")
        }
    }

    /// A font backend whose every glyph is an opaque square of coverage.
    ///
    /// Synthetic on purpose: it exercises the atlas, the bind group and the
    /// glyph shader with pixels the test can predict exactly, and it does it
    /// without a font file, a shaper or a rasterizer.
    struct SolidGlyphs;

    /// The edge length of every glyph `SolidGlyphs` produces, in pixels.
    const GLYPH_SIZE: u32 = 8;

    impl FontDb for SolidGlyphs {
        fn load_family_from_bytes(&mut self, name: &str, _: Vec<Vec<u8>>) -> Result<FamilyId> {
            bail!("SolidGlyphs has no families to load, but {name} was requested")
        }

        fn font_metrics(&self, _: FontId) -> Metrics {
            Metrics {
                units_per_em: 1000,
                ascent: 800,
                descent: -200,
                line_gap: 0,
            }
        }

        fn glyph_advance(&self, _: FontId, _: GlyphId) -> Result<Vector2F> {
            Ok(Vector2F::new(GLYPH_SIZE as f32, 0.))
        }

        fn glyph_raster_bounds(&self, _: GlyphKey, _: Vector2F) -> Result<RasterBounds> {
            // Sitting on the baseline: `top` counts upwards, so the bitmap
            // spans from one glyph-height above the baseline down to it.
            Ok(RasterBounds {
                left: 0,
                top: GLYPH_SIZE as i32,
                width: GLYPH_SIZE,
                height: GLYPH_SIZE,
            })
        }

        fn rasterize_glyph(
            &self,
            _: GlyphKey,
            _: Vector2F,
            _: SubpixelAlignment,
            _: crookui_core::fonts::RasterFormat,
        ) -> Result<RasterizedGlyph> {
            Ok(RasterizedGlyph {
                canvas: crookui_core::fonts::Canvas {
                    // Full coverage in every channel, which is what the atlas
                    // expects of a mask: the shader reads it from red.
                    pixels: vec![255; (GLYPH_SIZE * GLYPH_SIZE * 4) as usize],
                    size: (GLYPH_SIZE, GLYPH_SIZE),
                    row_stride: (GLYPH_SIZE * 4) as usize,
                    format: crookui_core::fonts::RasterFormat::Rgba32,
                },
                is_emoji: false,
            })
        }
    }

    fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * width + x) * 4) as usize;
        pixels[start..start + 4].try_into().expect("four channels")
    }

    #[test]
    fn a_rect_scene_renders_to_the_pixels_it_describes() {
        let mut scene = Scene::new(1.);
        scene
            .draw_rect_with_hit_recording(RectF::new(vec2f(10., 10.), vec2f(20., 20.)))
            .with_background(Color::rgb(255, 0, 0))
            .with_corner_radius(crookui_core::scene::CornerRadius::with_all(Radius::Pixels(
                4.,
            )));

        let Ok((pixels, width, height)) = render_scene_to_rgba(&scene, vec2f(40., 40.), &NoFonts)
        else {
            // No GPU here — a headless container, or a machine with no adapter
            // for any enabled backend. Everything else in this crate is testable
            // without one; this is the one test that is not.
            log::warn!("skipping the offscreen render test: no usable GPU adapter");
            return;
        };

        assert_eq!((width, height), (40, 40));
        assert_eq!(pixels.len(), 40 * 40 * 4);

        assert_eq!(
            pixel(&pixels, width, 20, 20),
            [255, 0, 0, 255],
            "the middle of the rect should be opaque red"
        );
        assert_eq!(
            pixel(&pixels, width, 2, 2),
            [0, 0, 0, 0],
            "outside the rect should be left transparent by the clear"
        );
        assert_eq!(
            pixel(&pixels, width, 10, 10),
            [0, 0, 0, 0],
            "the corner radius should have cut the rect's own corner away"
        );
    }

    #[test]
    fn a_solid_scene_reads_back_the_exact_bytes_it_was_given() {
        let mut scene = Scene::new(1.);
        scene
            .draw_rect_without_hit_recording(RectF::new(Vector2F::zero(), vec2f(2., 2.)))
            .with_background(Color::rgb(64, 128, 192));

        let Ok((pixels, width, height)) = render_scene_to_rgba(&scene, vec2f(2., 2.), &NoFonts)
        else {
            log::warn!("skipping the solid colour test: no usable GPU adapter");
            return;
        };

        assert_eq!((width, height), (2, 2));
        for y in 0..height {
            for x in 0..width {
                // Byte-identical in and out. The target is `Rgba8Unorm`, not
                // `Rgba8UnormSrgb`, so nothing anywhere applies a transfer
                // function: 64 would come back as 137 if it did.
                assert_eq!(
                    pixel(&pixels, width, x, y),
                    [64, 128, 192, 255],
                    "pixel ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn a_translucent_fill_lands_premultiplied_over_the_transparent_clear() {
        let mut scene = Scene::new(1.);
        scene
            .draw_rect_without_hit_recording(RectF::new(Vector2F::zero(), vec2f(2., 2.)))
            .with_background(Color::rgba(255, 0, 0, 128));

        let Ok((pixels, width, _)) = render_scene_to_rgba(&scene, vec2f(2., 2.), &NoFonts) else {
            log::warn!("skipping the alpha test: no usable GPU adapter");
            return;
        };

        // The shader emits straight alpha and the pipeline blends with
        // `ALPHA_BLENDING`, so `src * src_alpha + dst * (1 - src_alpha)` over a
        // transparent clear leaves the *buffer* premultiplied: red comes back
        // at 128, not 255. A reader compositing these bytes onto something else
        // must not multiply by alpha a second time.
        let [r, g, b, a] = pixel(&pixels, width, 0, 0);
        assert!(
            r.abs_diff(128) <= 1,
            "red should be 255 * (128/255), got {r}"
        );
        assert_eq!((g, b), (0, 0));
        assert!(
            a.abs_diff(128) <= 1,
            "alpha should survive unchanged, got {a}"
        );
    }

    #[test]
    fn a_glyph_lands_where_its_raster_bounds_say_it_should() {
        let mut scene = Scene::new(1.);
        scene.draw_glyph(vec2f(10., 20.), 1, FontId(0), 10., Color::WHITE);

        let Ok((pixels, width, _)) = render_scene_to_rgba(&scene, vec2f(40., 40.), &SolidGlyphs)
        else {
            log::warn!("skipping the offscreen glyph test: no usable GPU adapter");
            return;
        };

        // The quad's origin is the baseline position plus the raster bounds'
        // offset, so an 8px glyph drawn on a baseline at y = 20 covers
        // y = 12..20 and x = 10..18.
        assert_eq!(
            pixel(&pixels, width, 14, 16),
            [255, 255, 255, 255],
            "the middle of the glyph should be fully covered"
        );
        assert_eq!(
            pixel(&pixels, width, 14, 24),
            [0, 0, 0, 0],
            "below the baseline is outside a glyph that sits on it"
        );
        assert_eq!(
            pixel(&pixels, width, 4, 16),
            [0, 0, 0, 0],
            "left of the glyph's origin is outside it"
        );
    }
}
