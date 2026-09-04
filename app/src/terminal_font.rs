//! How big a terminal cell is, and which glyph goes in it.
//!
//! This is the monospace fast path the renderer was built for, and it is the
//! reason Crook owns its glyph pipeline instead of using a shaping library: a
//! grid needs one glyph record per cell, each in its own colour, thousands of
//! times a frame. Shaping is for a tab title. A cell is
//! [`CosmicGlyphs::glyph_for_char`] and a fixed advance.
//!
//! Everything expensive happens once, here, at startup:
//!
//! * the four faces the grid can draw in — regular, bold, italic and both —
//!   are selected from the monospace family and kept as [`FontId`]s, so a bold
//!   cell costs no lookup;
//! * the cell is measured as the advance of `m` in that family, which is what
//!   makes it a *monospace* cell rather than an average of something;
//! * the baseline within the line box is computed with the same rule
//!   [`crookui_core::text_layout`] uses for a shaped line, so terminal text
//!   sits where UI text of the same size would.
//!
//! What is left on the paint path is a hash lookup per cell, memoized by
//! `(face, character)`. A character the shell has never printed costs one call
//! into the font backend; every later occurrence of it costs the lookup.
//!
//! The one thing that lookup does not assume is that the monospace family can
//! draw what the shell printed. A grid is not Latin: `npm` spins in braille,
//! prompts are built out of powerline separators, `ls` prints whatever the
//! filenames are, and build scripts print emoji. A character the family lacks
//! resolves through [`CosmicGlyphs::fallback_glyph`] to a face that has it, and
//! the answer — face included — is memoized like any other.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use crook_terminal::CellFlags;
use crookui::CosmicGlyphs;
use crookui_core::fonts::{
    DEFAULT_UI_LINE_HEIGHT_RATIO, FamilyId, FontId, GlyphId, Properties, Style, Weight,
};
use crookui_core::text_layout::default_compute_baseline_position;

/// The em size a terminal grid is set at, in logical pixels.
///
/// The size the body panel already printed its one monospace line at, so a pane
/// that now runs a shell is set in the same type it used to describe one.
pub const CELL_FONT_SIZE: f32 = 12.5;

/// How much a face's ascent and descent have to add up to before they are
/// believed.
///
/// A face that reports nothing vertical would put every baseline at the top of
/// its line box. The proportions of a typical text face stand in instead, which
/// is the same choice the font backend makes for a face with no readable
/// metrics at all.
const FALLBACK_ASCENT_RATIO: f32 = 0.8;

/// The four faces a cell can be drawn in, indexed by bold and italic.
const FACE_COUNT: usize = 4;

/// One cell of the grid, in logical pixels.
///
/// Fractional widths are deliberate: the renderer positions glyphs at thirds of
/// a pixel horizontally, so rounding the advance would trade a real subpixel
/// pipeline for accumulated drift across a hundred columns. The *height* is
/// rounded, because the renderer snaps glyphs to whole pixels vertically and a
/// fractional row pitch would make some rows a pixel taller than others.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CellMetrics {
    /// The advance of one column.
    pub width: f32,
    /// The pitch of one row: the line box a cell's glyph sits in.
    pub height: f32,
    /// The em size the glyphs are drawn at.
    pub font_size: f32,
    /// Where the baseline sits, measured down from the top of the cell.
    pub baseline: f32,
}

impl CellMetrics {
    /// How many whole columns and rows fit in `width` by `height` logical
    /// pixels.
    ///
    /// Never below two columns or one row. A pane can be dragged smaller than a
    /// cell, and neither end of a terminal will accept a grid that small: a pty
    /// refuses no columns at all, and the emulator's grid clamps to two — the
    /// narrowest a double-width character fits in. Clamping to the same floor
    /// here is what keeps the size the pty was told from disagreeing with the
    /// size the snapshot comes back at.
    pub fn grid_for(self, width: f32, height: f32) -> (u16, u16) {
        const MIN_COLUMNS: f32 = 2.;
        const MIN_ROWS: f32 = 1.;

        let columns = (width / self.width)
            .floor()
            .clamp(MIN_COLUMNS, f32::from(u16::MAX)) as u16;
        let rows = (height / self.height)
            .floor()
            .clamp(MIN_ROWS, f32::from(u16::MAX)) as u16;
        (columns, rows)
    }
}

/// The faces and the measurements a terminal grid draws with.
///
/// Cheap to clone — it is an [`Rc`] — because every pane's element takes one
/// each frame. Resolve it once, at startup, and hand it down.
#[derive(Clone)]
pub struct CellFont(Rc<Inner>);

struct Inner {
    /// The font backend and the family it was resolved from, or `None` for a
    /// grid measured without one.
    ///
    /// The family is kept so that [`CellFont::resized`] can re-measure the
    /// same faces at a new size. Re-resolving it from a name would let a font
    /// uninstalled mid-session turn one press of the zoom chord into a change
    /// of typeface.
    source: Option<(CosmicGlyphs, FamilyId)>,
    /// Regular, bold, italic, bold italic — see [`CellFont::face`].
    faces: [FontId; FACE_COUNT],
    metrics: CellMetrics,
    /// Memoized glyph lookup, fallback included. `None` is cached too: a
    /// character no installed font can draw will not become drawable, and the
    /// fallback search is the expensive one to repeat.
    cache: RefCell<HashMap<(FontId, char), Option<DrawnGlyph>>>,
}

/// A glyph and the face it is actually drawn from, which is not always the face
/// that was asked for — see [`CellFont::glyph`].
type DrawnGlyph = (FontId, GlyphId);

impl CellFont {
    /// Resolves the faces and measures the cell of `family` at `font_size`.
    ///
    /// Fails when the family was never registered, or when its `m` has no
    /// advance — either way there is no monospace cell to be had from it.
    pub fn new(glyphs: CosmicGlyphs, family: FamilyId, font_size: f32) -> Result<Self> {
        Self::build(Some((glyphs, family)), font_size)
    }

    /// The same faces, measured at a different size.
    ///
    /// What zooming is. The family is remembered rather than re-resolved,
    /// which matters for more than tidiness: a name resolves to whatever is
    /// installed *now*, and a font uninstalled mid-session would otherwise
    /// make one press of the zoom chord silently change the typeface.
    ///
    /// A headless font resizes too, and is the only case that cannot fail.
    pub fn resized(&self, font_size: f32) -> Result<Self> {
        match self.0.source.clone() {
            Some((glyphs, family)) => Self::build(Some((glyphs, family)), font_size),
            None => Ok(Self::headless(font_size)),
        }
    }

    /// The size this font is set at.
    pub fn font_size(&self) -> f32 {
        self.0.metrics.font_size
    }

    /// Resolves the faces and measures the cell, for a real family or for
    /// nothing at all.
    fn build(source: Option<(CosmicGlyphs, FamilyId)>, font_size: f32) -> Result<Self> {
        let Some((glyphs, family)) = source else {
            return Ok(Self::headless(font_size));
        };
        let width = glyphs
            .em_width(family, font_size)
            .with_context(|| format!("{family:?} cannot measure a monospace cell"))?;
        if !(width.is_finite() && width > 0.) {
            bail!("{family:?} reports a cell {width} logical pixels wide");
        }

        let mut faces = [FontId(0); FACE_COUNT];
        for (index, face) in faces.iter_mut().enumerate() {
            let properties = properties_for(index);
            *face = glyphs
                .select_font(family, properties)
                .with_context(|| format!("{family:?} was never registered"))?;
        }

        let metrics = glyphs.font_metrics(faces[0]);
        let scale = font_size / metrics.units_per_em.max(1) as f32;
        let (ascent, descent) = (
            f32::from(metrics.ascent) * scale,
            f32::from(-metrics.descent) * scale,
        );

        Ok(Self::assemble(
            Some((glyphs, family)),
            faces,
            font_size,
            width,
            ascent,
            descent,
        ))
    }

    /// A grid measured with no font backend at all.
    ///
    /// The headless tests use this: a character stands in for its own glyph id,
    /// and a cell is half the font size wide, which is exactly what the stub
    /// shaper those tests lay text out with reports. It renders nothing a person
    /// would want to look at, and it is the only way to assert what a grid draws
    /// without making the assertion depend on the fonts installed on the machine
    /// running it.
    pub fn headless(font_size: f32) -> Self {
        Self::assemble(
            None,
            [FontId(0); FACE_COUNT],
            font_size,
            font_size * 0.5,
            font_size * FALLBACK_ASCENT_RATIO,
            font_size * (1. - FALLBACK_ASCENT_RATIO),
        )
    }

    /// The cell this font measures.
    pub fn metrics(&self) -> CellMetrics {
        self.0.metrics
    }

    /// The face plain text is drawn in.
    ///
    /// What the command input asks for: a field is not a grid, so it has no
    /// [`CellFlags`] to resolve, and every cell of it is regular weight.
    pub fn regular(&self) -> FontId {
        self.face(CellFlags::NONE)
    }

    /// The face a cell with these attributes is drawn in.
    ///
    /// Underline and strikeout are rules the grid draws itself, and every other
    /// flag on a [`CellFlags`] has already been resolved into a colour, so bold
    /// and italic are the only two that choose a face.
    pub fn face(&self, flags: CellFlags) -> FontId {
        let bold = usize::from(flags.contains(CellFlags::BOLD));
        let italic = usize::from(flags.contains(CellFlags::ITALIC));
        self.0.faces[bold | (italic << 1)]
    }

    /// The face and glyph `character` is drawn with, or `None` when no font on
    /// this machine can draw it.
    ///
    /// The face that comes back is usually the one that went in, and is another
    /// one when the monospace family does not cover the character: a spinner's
    /// braille frames, a powerline separator, a CJK filename, an emoji a build
    /// script printed. A grid that dropped those would draw a blank column and
    /// keep advancing over it, so the line would be silently short — and
    /// copying the screen would disagree with it.
    ///
    /// Memoized both ways round, misses included. The paint path calls this
    /// once per cell, and the miss — a charmap parse, and then a shaping call
    /// that may walk every installed face — has to happen once per distinct
    /// character and never again.
    pub fn glyph(&self, face: FontId, character: char) -> Option<DrawnGlyph> {
        if let Some(glyph) = self.0.cache.borrow().get(&(face, character)) {
            return *glyph;
        }

        let glyph = match self.0.source.as_ref().map(|(glyphs, _)| glyphs) {
            Some(glyphs) => glyphs
                .glyph_for_char(face, character)
                .map(|glyph| (face, glyph))
                .or_else(|| glyphs.fallback_glyph(face, character)),
            // No backend: the character is its own glyph, which is the identity
            // the stub shaper uses and the only thing a test can assert on.
            None => Some((face, character as GlyphId)),
        };
        self.0.cache.borrow_mut().insert((face, character), glyph);
        glyph
    }

    fn assemble(
        source: Option<(CosmicGlyphs, FamilyId)>,
        faces: [FontId; FACE_COUNT],
        font_size: f32,
        width: f32,
        ascent: f32,
        descent: f32,
    ) -> Self {
        // A face whose metrics are missing or nonsensical would otherwise put
        // every glyph on the top edge of its cell.
        let (ascent, descent) = if ascent + descent > 0. {
            (ascent, descent)
        } else {
            (
                font_size * FALLBACK_ASCENT_RATIO,
                font_size * (1. - FALLBACK_ASCENT_RATIO),
            )
        };

        let height = (font_size * DEFAULT_UI_LINE_HEIGHT_RATIO).ceil().max(1.);
        let baseline =
            default_compute_baseline_position(font_size, height / font_size, ascent, descent);

        Self(Rc::new(Inner {
            source,
            faces,
            metrics: CellMetrics {
                width,
                height,
                font_size,
                baseline,
            },
            cache: RefCell::new(HashMap::new()),
        }))
    }
}

/// The face selection at each index of [`Inner::faces`].
fn properties_for(index: usize) -> Properties {
    Properties {
        weight: if index & 1 == 0 {
            Weight::Normal
        } else {
            Weight::Bold
        },
        style: if index & 2 == 0 {
            Style::Normal
        } else {
            Style::Italic
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_headless_cell_is_the_stub_shapers_advance() {
        // The tests draw against a shaper that reports half the font size per
        // character; a grid that measured itself differently would put its
        // columns somewhere the rest of the frame does not agree with.
        let font = CellFont::headless(12.);
        assert_eq!(font.metrics().width, 6.);
        assert_eq!(font.metrics().height, 15.);
    }

    #[test]
    fn the_baseline_sits_inside_the_cell() {
        let font = CellFont::headless(12.);
        let metrics = font.metrics();
        assert!(metrics.baseline > 0. && metrics.baseline < metrics.height);
    }

    #[test]
    fn a_grid_never_has_a_zero_dimension() {
        // A pane can be dragged narrower than a cell, and neither a pty nor the
        // emulator's grid will accept what that measures.
        let metrics = CellFont::headless(12.).metrics();
        assert_eq!(metrics.grid_for(0., 0.), (2, 1));
        assert_eq!(metrics.grid_for(60., 75.), (10, 5));
        assert_eq!(
            metrics.grid_for(65., 79.),
            (10, 5),
            "a part of a cell is not a cell"
        );
    }

    #[test]
    fn bold_and_italic_pick_different_faces_of_the_same_family() {
        let font = CellFont::headless(12.);
        // Headless, every face is the same id — what is being asserted is the
        // indexing, which is what would silently draw italics in bold.
        assert_eq!(font.face(CellFlags::NONE), font.face(CellFlags::UNDERLINE));
        assert_eq!(properties_for(0), Properties::default());
        assert_eq!(properties_for(1), Properties::bold());
        assert_eq!(properties_for(2), Properties::italic());
        assert_eq!(
            properties_for(3),
            Properties {
                weight: Weight::Bold,
                style: Style::Italic
            }
        );
    }

    #[test]
    fn a_glyph_is_looked_up_once_and_remembered() {
        let font = CellFont::headless(12.);
        let face = font.face(CellFlags::NONE);

        assert_eq!(font.glyph(face, 'q'), Some((face, u32::from('q'))));
        assert_eq!(font.glyph(face, 'q'), Some((face, u32::from('q'))));
        assert_eq!(font.0.cache.borrow().len(), 1);
    }
}
