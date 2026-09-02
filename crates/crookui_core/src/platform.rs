//! The seam between this crate and whatever draws for it.
//!
//! Two traits, and no implementations. Everything above this line — elements,
//! views, the scene — is written against them, so this crate compiles with no
//! font library, no GPU and no window system, and the platform crate is free to
//! change any of the three without touching a view.
//!
//! Crook has exactly one implementation of each today. They stay traits anyway:
//! together they are forty lines, and they are the line along which a headless
//! renderer, a second shaper or a test double gets inserted later.

use std::ops::Range;

use anyhow::Result;

use crate::fonts::{
    FamilyId, FontId, GlyphId, GlyphKey, LineStyle, Metrics, RasterBounds, RasterFormat,
    RasterizedGlyph, StyleAndFont, SubpixelAlignment,
};
use crate::geometry::Vector2F;
use crate::text_layout::Line;

/// Turns a string into positioned glyphs.
///
/// `Send + Sync` because shaping is the expensive half of laying out text and
/// belongs on a background thread once there is enough of it to matter.
pub trait TextLayoutSystem: 'static + Send + Sync {
    /// Shapes `text` as a single line.
    ///
    /// `style_runs` assigns a family, face and paint style to byte ranges of
    /// `text`; ranges must be ascending and non-overlapping, and any byte not
    /// covered by one takes the style of the preceding run. The shaper may
    /// still split a run further when font fallback needs a second face.
    ///
    /// `max_width` is advisory: the returned [`Line`] may be wider, and the
    /// caller decides whether to clip. Nothing here wraps — a line is a line.
    fn layout_line(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
    ) -> Line;
}

/// Loads fonts and rasterizes glyphs.
///
/// Deliberately not `Send`/`Sync`: font backends keep mutable native state, and
/// everything a background thread needs from text goes through
/// [`TextLayoutSystem`] instead. Callers are expected to cache what they ask
/// for — none of these methods promises to be cheap.
pub trait FontDb: 'static {
    /// Registers a family from font file bytes, one entry per face.
    ///
    /// `name` is the family name callers will look the result up by. Returns
    /// an error when no byte slice parses as a font.
    fn load_family_from_bytes(&mut self, name: &str, bytes: Vec<Vec<u8>>) -> Result<FamilyId>;

    /// The font-wide metrics of a face, in font units.
    fn font_metrics(&self, font_id: FontId) -> Metrics;

    /// A glyph's advance in font units — scale by `font_size / units_per_em`.
    fn glyph_advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Vector2F>;

    /// The pixel bounds a glyph would rasterize into at `scale`, relative to
    /// where its left edge meets the baseline.
    ///
    /// Answering this without rasterizing is what lets an atlas allocate space
    /// for a glyph before it has the pixels.
    fn glyph_raster_bounds(&self, glyph_key: GlyphKey, scale: Vector2F) -> Result<RasterBounds>;

    /// Rasterizes a glyph.
    ///
    /// `subpixel_alignment` shifts the outline horizontally within the pixel
    /// grid before rasterizing; the caller keys its cache on it and positions
    /// the result at the matching fraction of a pixel.
    fn rasterize_glyph(
        &self,
        glyph_key: GlyphKey,
        scale: Vector2F,
        subpixel_alignment: SubpixelAlignment,
        format: RasterFormat,
    ) -> Result<RasterizedGlyph>;
}
