//! Turning a glyph into pixels, with swash.
//!
//! A port of Warp's `crates/warpui/src/windowing/winit/fonts/swash_rasterizer.rs`
//! — the whole of its cross-platform rasterizer, and proof that rasterization
//! is not the hard part of a font stack. Two entry points, both routed into
//! cosmic-text's swash scaler:
//!
//! * [`CosmicFontDb::raster_bounds`] answers "how big, and where relative to
//!   the origin" without handing back pixels. That is what lets the glyph atlas
//!   reserve space for a glyph before it has one.
//! * [`CosmicFontDb::rasterize`] answers with the pixels, in a bitmap that
//!   exactly fills the reserved space.
//!
//! Both go through `get_image_uncached`: swash caches nothing here on purpose,
//! because the renderer's atlas already caches the result and a second copy of
//! every glyph bitmap buys nothing.
//!
//! # The bin-size problem, and why bounds are a union
//!
//! A glyph is rasterized three times, once per horizontal subpixel bin, and
//! **the three bitmaps are not the same size.** Shifting an outline by a third
//! of a pixel routinely makes it cover one more or one fewer pixel column: over
//! a sweep of ASCII at six sizes and two scales, roughly a third of the
//! (glyph, bin) pairs differ from the bin-zero measurement, in both directions.
//!
//! That matters because [`crookui_core::platform::FontDb`] splits measuring
//! from rasterizing and only the second half is told which bin it is drawing.
//! An atlas that allocates from a bin-zero measurement and then uploads a
//! bin-two bitmap has reserved the wrong rectangle: too small and the upload
//! runs past its region, too large and the quad — whose size, per the glyph
//! shader's contract, is the *atlas region* — samples padding as if it were
//! glyph. Warp inherits the same split and papers over it with a `vec2i(1, 1)`
//! inflation in its `font-kit` path.
//!
//! So [`CosmicFontDb::raster_bounds`] reports the union of all three bins, and
//! [`CosmicFontDb::rasterize`] composites its bitmap into a canvas of exactly
//! that size. Bounds and pixels then agree for every bin by construction, the
//! atlas allocation is exact, and the extra area is transparent. The union is
//! memoized, so a glyph is measured three times once and never again.
//!
//! # Two further divergences from the Warp original
//!
//! Warp sets `Canvas::row_stride` from the *source* format's bytes per pixel
//! while writing pixels in the *destination* format, so an expanded A8 mask
//! claims a quarter of its real stride. Nothing catches it because Warp's
//! uploader recomputes the stride and ignores the field. This sets the true
//! stride; an uploader that trusts the field gets correctly-shaped glyphs
//! rather than diagonally sheared ones.
//!
//! Warp also casts a `u32` glyph id straight to `u16`. Faces with more than
//! 65,535 glyphs exist. This refuses the cast rather than wrapping it.

use std::sync::OnceLock;

use anyhow::Result;
use cosmic_text::{CacheKey, CacheKeyFlags, SwashContent, SwashImage};
use crookui_core::fonts::{
    Canvas, GlyphKey, RasterBounds, RasterFormat, RasterizedGlyph, SubpixelAlignment,
};
use crookui_core::geometry::{Vector2F, vec2f};

use super::{CosmicFontDb, to_swash_glyph_id};

impl CosmicFontDb {
    /// Where a glyph's bitmap lands relative to the point where its left edge
    /// meets the baseline, and how big it is, at `scale` device pixels per
    /// logical pixel.
    ///
    /// Large enough for the glyph at every subpixel alignment; see the module
    /// docs. A glyph that draws nothing — a space, or a character the face maps
    /// to an outline-less glyph — reports empty bounds rather than an error,
    /// because "there is nothing to put in the atlas" is an ordinary answer.
    pub(super) fn raster_bounds(
        &self,
        glyph_key: GlyphKey,
        scale: Vector2F,
    ) -> Result<RasterBounds> {
        let cache_key = (glyph_key, scale.x().to_bits(), scale.y().to_bits());
        if let Some(bounds) = self.store.raster_bounds.read().get(&cache_key) {
            return Ok(*bounds);
        }

        let mut bounds = RasterBounds::default();
        for offset in subpixel_offsets() {
            if let Some(image) = self.raster_image(glyph_key, scale, *offset)? {
                bounds = union(bounds, placement_bounds(&image));
            }
        }

        self.store.raster_bounds.write().insert(cache_key, bounds);
        Ok(bounds)
    }

    /// Rasterizes a glyph at `scale` device pixels per logical pixel, shifted
    /// horizontally within the pixel grid by `subpixel_alignment`.
    ///
    /// The returned canvas is always exactly the size
    /// [`CosmicFontDb::raster_bounds`] reports for the same key and scale.
    ///
    /// `format` is honoured for coverage masks, which is what text is. A color
    /// glyph — an emoji — carries its own pixels and always comes back as
    /// [`RasterFormat::Rgba32`]; read [`Canvas::format`] rather than assuming
    /// the requested one.
    pub(super) fn rasterize(
        &self,
        glyph_key: GlyphKey,
        scale: Vector2F,
        subpixel_alignment: SubpixelAlignment,
        format: RasterFormat,
    ) -> Result<RasterizedGlyph> {
        let bounds = self.raster_bounds(glyph_key, scale)?;
        if bounds.is_empty() {
            return Ok(blank_glyph(bounds, format));
        }

        let offset = subpixel_alignment.to_offset().x();
        let Some(image) = self.raster_image(glyph_key, scale, offset)? else {
            return Ok(blank_glyph(bounds, format));
        };

        let (source_format, is_emoji) = match image.content {
            SwashContent::Mask => (RasterFormat::A8, false),
            SwashContent::SubpixelMask => (RasterFormat::Rgba32, false),
            SwashContent::Color => (RasterFormat::Rgba32, true),
        };

        // Only an A8 coverage mask can be re-expressed; the other two are
        // already four bytes per pixel and carry color a narrower format would
        // throw away.
        let format = if source_format == RasterFormat::A8 {
            format
        } else {
            source_format
        };

        let canvas = composite(&image, source_format, bounds, format);
        Ok(RasterizedGlyph { canvas, is_emoji })
    }

    /// Runs the swash scaler over one glyph at one subpixel offset.
    ///
    /// `None` when the face produces no bitmap at this size, which is the
    /// normal answer for whitespace.
    fn raster_image(
        &self,
        glyph_key: GlyphKey,
        scale: Vector2F,
        subpixel_offset: f32,
    ) -> Result<Option<SwashImage>> {
        let face = self.store.face(glyph_key.font_id)?;
        let glyph_id = to_swash_glyph_id(glyph_key.glyph_id)?;

        // `CacheKey::new` returns the key plus the integral part of the
        // position it binned away; only the key is wanted, because the caller
        // positions the quad itself from the raster bounds.
        let cache_key = CacheKey::new(
            face.id,
            glyph_id,
            glyph_key.font_size * scale.x(),
            (subpixel_offset, 0.),
            face.weight,
            CacheKeyFlags::empty(),
        )
        .0;

        // The swash scaler is taken before the font system, always, so this and
        // the font store's own single-lock discipline cannot interleave into a
        // cycle.
        let mut swash = self.swash.lock();
        let mut font_system = self.store.font_system.write();
        Ok(swash.get_image_uncached(&mut font_system, cache_key))
    }
}

/// Every distinct horizontal offset [`SubpixelAlignment`] can quantize to.
///
/// Recovered by sweeping one pixel and deduplicating rather than restated as a
/// constant here, because the step count belongs to `SubpixelAlignment` and a
/// copy of it in this module would be a copy that can drift.
fn subpixel_offsets() -> &'static [f32] {
    static OFFSETS: OnceLock<Vec<f32>> = OnceLock::new();

    OFFSETS.get_or_init(|| {
        const SAMPLES: u32 = 256;

        let mut offsets: Vec<_> = (0..SAMPLES)
            .map(|sample| {
                SubpixelAlignment::new(vec2f(sample as f32 / SAMPLES as f32, 0.))
                    .to_offset()
                    .x()
            })
            .collect();
        offsets.sort_by(f32::total_cmp);
        offsets.dedup();
        offsets
    })
}

/// swash reports placement with `top` measured upwards from the baseline and
/// `left` rightwards from the origin, which is exactly what [`RasterBounds`]
/// documents. No sign flip: Warp flips `top` only because its rectangle type
/// puts y downwards.
fn placement_bounds(image: &SwashImage) -> RasterBounds {
    RasterBounds {
        left: image.placement.left,
        top: image.placement.top,
        width: image.placement.width,
        height: image.placement.height,
    }
}

/// The smallest bounds containing both. An empty operand contributes nothing,
/// so this folds cleanly from [`RasterBounds::default`].
fn union(a: RasterBounds, b: RasterBounds) -> RasterBounds {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }

    let left = a.left.min(b.left);
    let top = a.top.max(b.top);
    let right = (a.left + a.width as i32).max(b.left + b.width as i32);
    let bottom = (a.top - a.height as i32).min(b.top - b.height as i32);

    RasterBounds {
        left,
        top,
        width: (right - left).max(0) as u32,
        height: (top - bottom).max(0) as u32,
    }
}

/// Places a rasterized bitmap into a canvas the size of `bounds`.
///
/// Everything `bounds` covers that the bitmap does not stays zero, which for
/// both a coverage mask and a premultiplied color bitmap reads as transparent.
fn composite(
    image: &SwashImage,
    source_format: RasterFormat,
    bounds: RasterBounds,
    format: RasterFormat,
) -> Canvas {
    let bytes_per_pixel = format.bytes_per_pixel() as usize;
    let row_stride = bounds.width as usize * bytes_per_pixel;
    let mut pixels = vec![0u8; row_stride * bounds.height as usize];

    // The bitmap sits inside the union by construction, so both deltas are
    // non-negative; they are clamped anyway because a font that reports
    // inconsistent placements must not be able to panic the renderer.
    let left_offset = (image.placement.left - bounds.left).max(0) as usize;
    let top_offset = (bounds.top - image.placement.top).max(0) as usize;

    let source_bytes_per_pixel = source_format.bytes_per_pixel() as usize;
    let source_stride = image.placement.width as usize * source_bytes_per_pixel;
    let copied_width = (image.placement.width as usize).min(
        (bounds.width as usize)
            .checked_sub(left_offset)
            .unwrap_or_default(),
    );

    for row in 0..image.placement.height as usize {
        let target_row = row + top_offset;
        if target_row >= bounds.height as usize {
            break;
        }

        let source = &image.data[row * source_stride..][..copied_width * source_bytes_per_pixel];
        let target_start = target_row * row_stride + left_offset * bytes_per_pixel;
        let target = &mut pixels[target_start..][..copied_width * bytes_per_pixel];

        if source_format == format {
            target.copy_from_slice(source);
        } else {
            // The only conversion that reaches here: a one-byte coverage mask
            // widened by replicating the byte into every channel. Replication
            // rather than "coverage in alpha alone" because the glyph shader
            // reads coverage from the red channel of an RGBA atlas — one
            // texture and one format serve masks and emoji alike, and the
            // shader picks between them with a flag, not a second sampler.
            for (pixel, coverage) in target.chunks_exact_mut(bytes_per_pixel).zip(source) {
                pixel.fill(*coverage);
            }
        }
    }

    Canvas {
        pixels,
        size: (bounds.width, bounds.height),
        row_stride,
        format,
    }
}

/// A canvas the size of `bounds` with nothing drawn in it.
fn blank_glyph(bounds: RasterBounds, format: RasterFormat) -> RasterizedGlyph {
    let row_stride = bounds.width as usize * format.bytes_per_pixel() as usize;

    RasterizedGlyph {
        canvas: Canvas {
            pixels: vec![0u8; row_stride * bounds.height as usize],
            size: (bounds.width, bounds.height),
            row_stride,
            format,
        },
        is_emoji: false,
    }
}
