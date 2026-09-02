//! Font identity, style and rasterization types.
//!
//! This module is deliberately data-only: it names fonts and glyphs and
//! describes how they should be drawn, but it loads nothing. The crate that
//! owns a font backend implements [`crate::platform::FontDb`] over these types.
//!
//! Warp's equivalent (`warpui_core/src/fonts.rs`) also carries a memoizing
//! `Cache` in front of the backend. Caching belongs next to the backend that
//! knows what is expensive, so it lives in the platform crate here.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::geometry::Color;

/// A font family: a name like "Hack", covering every weight and style in it.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct FamilyId(pub usize);

impl FamilyId {
    /// Mints a globally-unique family id.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// One concrete face: a family resolved through a weight and a style.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct FontId(pub usize);

impl FontId {
    /// Mints a globally-unique face id.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// A glyph index within a single face. Not a character: shaping maps a run of
/// characters to a run of these, and the mapping is neither 1:1 nor stable
/// across faces.
pub type GlyphId = u32;

/// Whether a face is upright or slanted.
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
pub enum Style {
    /// Upright.
    #[default]
    Normal,
    /// Slanted.
    Italic,
}

/// The nine CSS-style weights a family may provide.
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub enum Weight {
    /// 100.
    Thin,
    /// 200.
    ExtraLight,
    /// 300.
    Light,
    /// 400.
    #[default]
    Normal,
    /// 500.
    Medium,
    /// 600.
    Semibold,
    /// 700.
    Bold,
    /// 800.
    ExtraBold,
    /// 900.
    Black,
}

impl Weight {
    /// The numeric weight, for backends that speak in CSS units.
    pub fn to_number(self) -> u16 {
        match self {
            Self::Thin => 100,
            Self::ExtraLight => 200,
            Self::Light => 300,
            Self::Normal => 400,
            Self::Medium => 500,
            Self::Semibold => 600,
            Self::Bold => 700,
            Self::ExtraBold => 800,
            Self::Black => 900,
        }
    }
}

/// What to ask a family for when selecting a face.
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Properties {
    /// Upright or italic.
    pub style: Style,
    /// How heavy.
    pub weight: Weight,
}

impl Properties {
    /// The face selection for bold text of an otherwise default style.
    pub fn bold() -> Self {
        Self {
            weight: Weight::Bold,
            ..Default::default()
        }
    }

    /// The face selection for italic text of an otherwise default style.
    pub fn italic() -> Self {
        Self {
            style: Style::Italic,
            ..Default::default()
        }
    }
}

/// Font-wide metrics, in font units.
///
/// Scale to pixels with `point_size / units_per_em`.
#[derive(Copy, Clone, Debug)]
pub struct Metrics {
    /// Font units per em; the denominator for every other field here.
    pub units_per_em: u32,

    /// How far the face rises above the baseline.
    pub ascent: i16,

    /// How far the face descends below the baseline.
    ///
    /// Typically negative, matching `sTypoDescender` in the OpenType `OS/2`
    /// table — the opposite sign from what the Windows and macOS APIs report.
    pub descent: i16,

    /// Extra leading the designer asks for between baselines.
    pub line_gap: i16,
}

/// How a line of text is sized vertically.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LineStyle {
    /// Em size in logical pixels.
    pub font_size: f32,

    /// Line height as a multiple of `font_size`.
    pub line_height_ratio: f32,

    /// Where the baseline sits within the line, as a fraction of the space
    /// above and below the text box.
    pub baseline_ratio: f32,

    /// Tab stop width in spaces, for fully monospaced text. `None` leaves tab
    /// handling to the backend.
    pub fixed_width_tab_size: Option<u8>,
}

/// Line height as a multiple of the font size, matching Warp's UI default.
pub const DEFAULT_UI_LINE_HEIGHT_RATIO: f32 = 1.2;

/// The default split of leftover line height above versus below the text box.
pub const DEFAULT_TOP_BOTTOM_RATIO: f32 = 0.8;

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            font_size: 14.,
            line_height_ratio: DEFAULT_UI_LINE_HEIGHT_RATIO,
            baseline_ratio: DEFAULT_TOP_BOTTOM_RATIO,
            fixed_width_tab_size: None,
        }
    }
}

/// How a span of text should be painted.
///
/// Warp carries syntax colors, borders, error underlines, strikethroughs and
/// hyperlink ids here too. Those all belong to a text editor; a tab title and a
/// usage chip need three colors.
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextStyle {
    /// Glyph color. `None` means "use the color the element was given".
    pub foreground_color: Option<Color>,

    /// A rectangle painted behind the run before its glyphs.
    pub background_color: Option<Color>,

    /// Draws an underline under the run in this color.
    pub underline_color: Option<Color>,
}

/// A style plus the family it applies to, as handed to the shaper per range.
///
/// The family — not a [`FontId`] — is what is recorded, because resolving a
/// family to a face is the shaper's job: it may need a different face per
/// character to cover the string.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub struct StyleAndFont {
    /// Which family to shape with.
    pub font_family: FamilyId,
    /// Which face within the family.
    pub properties: Properties,
    /// How to paint the resulting glyphs.
    pub style: TextStyle,
}

impl StyleAndFont {
    /// Pairs a family, a face selection and a paint style.
    pub fn new(font_family: FamilyId, properties: Properties, style: TextStyle) -> Self {
        Self {
            font_family,
            properties,
            style,
        }
    }
}

/// Everything needed to rasterize one glyph, and the key it is cached under.
///
/// `font_size` participates in equality by its bit pattern, which is exactly
/// what a cache key wants: two sizes that are not bitwise identical rasterize
/// differently anyway.
#[derive(Copy, Clone, Debug)]
pub struct GlyphKey {
    /// Which glyph.
    pub glyph_id: GlyphId,
    /// Which face.
    pub font_id: FontId,
    /// At what em size, in logical pixels.
    pub font_size: f32,
}

impl PartialEq for GlyphKey {
    fn eq(&self, other: &Self) -> bool {
        self.glyph_id == other.glyph_id
            && self.font_id == other.font_id
            && self.font_size.to_bits() == other.font_size.to_bits()
    }
}

impl Eq for GlyphKey {}

impl Hash for GlyphKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.glyph_id.hash(state);
        self.font_id.hash(state);
        self.font_size.to_bits().hash(state);
    }
}

/// The horizontal subpixel position a glyph was rasterized at.
///
/// Only the horizontal component is quantized, because glyphs are snapped to
/// whole pixels vertically by the renderer. Three steps per pixel is enough to
/// keep advance rounding invisible while tripling, not quintupling, the atlas.
#[derive(Copy, Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SubpixelAlignment(u8);

impl SubpixelAlignment {
    /// How many subdivisions a pixel is sliced into.
    const STEPS: u8 = 3;

    /// Quantizes the horizontal component of `glyph_position` to the nearest
    /// subpixel step.
    ///
    /// This is semantically a modulus: a fraction such as 0.95 rounds up and
    /// around to step 0.
    pub fn new(glyph_position: crate::geometry::Vector2F) -> Self {
        let scaled = glyph_position.x().fract() * Self::STEPS as f32;
        Self(scaled.round() as u8 % Self::STEPS)
    }

    /// The horizontal offset within the pixel this alignment stands for.
    pub fn to_offset(self) -> crate::geometry::Vector2F {
        crate::geometry::vec2f(self.0 as f32 / Self::STEPS as f32, 0.)
    }
}

/// The pixel format of a rasterized glyph.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum RasterFormat {
    /// Premultiplied R8G8B8A8, little-endian. Color glyphs (emoji).
    Rgba32,
    /// R8G8B8, little-endian.
    Rgb24,
    /// A single coverage byte per pixel. The usual format for text.
    A8,
}

impl RasterFormat {
    /// Bytes per pixel in this format.
    pub fn bytes_per_pixel(self) -> u8 {
        match self {
            Self::Rgba32 => 4,
            Self::Rgb24 => 3,
            Self::A8 => 1,
        }
    }
}

/// Where a rasterized glyph lands relative to the point where its left edge
/// meets the baseline, and how big it is.
///
/// This is the shape every rasterizer reports placement in: signed offsets
/// (a glyph routinely starts left of and above its origin) and unsigned
/// extents. Keeping it integral keeps atlas allocation exact.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct RasterBounds {
    /// Pixels right of the origin where the bitmap starts; often negative.
    pub left: i32,
    /// Pixels above the baseline where the bitmap starts; positive is up.
    pub top: i32,
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
}

impl RasterBounds {
    /// Whether the glyph rasterizes to nothing, as a space does.
    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// An in-memory bitmap holding one rasterized glyph.
#[derive(Clone, Debug)]
pub struct Canvas {
    /// The pixels, `row_stride` bytes per row.
    pub pixels: Vec<u8>,
    /// Width and height in pixels.
    pub size: (u32, u32),
    /// Bytes between the starts of successive rows, which may exceed
    /// `width * bytes_per_pixel` when the backend pads rows.
    pub row_stride: usize,
    /// How to interpret `pixels`.
    pub format: RasterFormat,
}

/// A rasterized glyph and whether it carries its own color.
#[derive(Clone, Debug)]
pub struct RasterizedGlyph {
    /// The pixels.
    pub canvas: Canvas,
    /// Color glyphs are blitted as-is; monochrome ones are tinted by the
    /// text color at draw time.
    pub is_emoji: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::vec2f;

    #[test]
    fn subpixel_alignment_wraps_around_the_pixel() {
        assert_eq!(SubpixelAlignment::new(vec2f(1.0, 0.)).to_offset().x(), 0.);
        assert_eq!(
            SubpixelAlignment::new(vec2f(1.95, 0.)).to_offset().x(),
            0.,
            "0.95 rounds up to a full pixel, which is step 0 of the next one"
        );
        assert_eq!(
            SubpixelAlignment::new(vec2f(0.34, 0.)),
            SubpixelAlignment::new(vec2f(7.33, 0.)),
            "only the fractional part matters"
        );
    }
}
