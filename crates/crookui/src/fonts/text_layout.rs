//! Shaping: a string and its style runs in, positioned glyphs out.
//!
//! The body of [`CosmicTextLayout::layout_line`] is Warp's
//! `winit/fonts.rs:1305-1360` with the bidi paragraph walk, the caret table and
//! the clip configuration removed — build a [`cosmic_text::AttrsList`] from the
//! style runs, hand it to [`cosmic_text::ShapeLine`], hand the resulting
//! [`cosmic_text::LayoutGlyph`]s to a [`RunBuilder`]. The interesting parts are
//! all in what has to be carried across cosmic-text's API:
//!
//! * **Styles ride in a `usize`.** cosmic-text lets an attribute span carry one
//!   machine word of caller metadata and nothing else, so [`TextStylesMap`]
//!   parks each [`TextStyle`] in a vector and sends the index. Warp's trick,
//!   kept whole.
//! * **Runs are discovered, not declared.** The caller says which family and
//!   style each byte range wants; where the *runs* break is the shaper's
//!   answer, because font fallback can split one styled range across three
//!   faces to cover its characters. [`RunBuilder`] starts a new run whenever
//!   either the face or the style changes.
//! * **Offsets are translated exactly once.** cosmic-text reports byte offsets
//!   into the string it was given, which is not the string the caller gave;
//!   see [`super::str_index_map`].

use std::ops::Range;
use std::sync::Arc;

use cosmic_text::{
    Align, Attrs, AttrsList, Hinting, LayoutGlyph, LayoutLine, ShapeLine, Shaping, Wrap, fontdb,
};
use crookui_core::fonts::{FontId, LineStyle, StyleAndFont, TextStyle};
use crookui_core::geometry::vec2f;
use crookui_core::platform;
use crookui_core::text_layout::{Glyph, Line, Run};

use super::str_index_map::StrIndexMap;
use super::{DEFAULT_TAB_WIDTH, FontStore};

/// Crook's shaper.
///
/// Obtained from [`CosmicFontDb::text_layout`], which it shares a font store
/// with. `Send + Sync`, and cheap to clone.
///
/// [`CosmicFontDb::text_layout`]: super::CosmicFontDb::text_layout
#[derive(Clone)]
pub struct CosmicTextLayout {
    store: Arc<FontStore>,
}

impl CosmicTextLayout {
    pub(super) fn new(store: Arc<FontStore>) -> Self {
        Self { store }
    }

    /// Resolves each style run to a concrete face and parks its paint style,
    /// producing the attribute list the shaper reads.
    ///
    /// Face selection happens here rather than being left to cosmic-text
    /// because cosmic-text does not implement CSS font matching: asked for a
    /// weight or slant a family does not have, it can fail outright. Resolving
    /// through the font database first and then naming the *resolved* face's
    /// own weight and style means the shaper is only ever asked for a
    /// combination that exists.
    fn build_attrs_list(
        &self,
        indices: &StrIndexMap<'_>,
        style_runs: &[(Range<usize>, StyleAndFont)],
        styles: &mut TextStylesMap,
    ) -> AttrsList {
        let text_len = indices.shaped_text().len();

        // Selection takes the store's locks, so it is finished before the font
        // database is opened for reading below.
        let resolved: Vec<_> = style_runs
            .iter()
            .filter_map(|(range, style_and_font)| {
                let start = indices.shaped_index(range.start).min(text_len);
                let end = indices.shaped_index(range.end).min(text_len);
                if start >= end {
                    return None;
                }

                let font_id = self
                    .store
                    .select_font(style_and_font.font_family, style_and_font.properties)?;
                let face = self.store.face(font_id).ok()?;
                Some((start..end, face.id, styles.insert(style_and_font.style)))
            })
            .collect();

        let mut attrs_list = AttrsList::new(&Attrs::new());
        let font_system = self.store.font_system.read();
        for (range, face_id, metadata) in resolved {
            let Some(face) = font_system.db().face(face_id) else {
                continue;
            };
            let Some((family, _)) = face.families.first() else {
                continue;
            };

            attrs_list.add_span(
                range,
                &Attrs::new()
                    .family(fontdb::Family::Name(family.as_str()))
                    .style(face.style)
                    .weight(face.weight)
                    .metadata(metadata),
            );
        }
        attrs_list
    }

    /// Ascent and descent for a line with no glyphs in it.
    ///
    /// An empty tab title still occupies a line box and still has a baseline,
    /// and the shaper reports neither for a string it produced nothing from.
    /// The first style run names the family the text would have used, which is
    /// the right face to take the answer from.
    fn empty_line_extents(
        &self,
        style_runs: &[(Range<usize>, StyleAndFont)],
        font_size: f32,
    ) -> (f32, f32) {
        let Some((_, style_and_font)) = style_runs.first() else {
            return (0., 0.);
        };
        let Some(font_id) = self
            .store
            .select_font(style_and_font.font_family, style_and_font.properties)
        else {
            return (0., 0.);
        };

        let metrics = self.store.font_metrics(font_id);
        let scale = font_size / metrics.units_per_em as f32;

        // `Line::descent` is a positive drop below the baseline; `Metrics`
        // spells the same quantity with the opposite sign.
        (
            f32::from(metrics.ascent) * scale,
            f32::from(-metrics.descent) * scale,
        )
    }

    fn build_line(
        &self,
        layout_line: LayoutLine,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        styles: &TextStylesMap,
        indices: &StrIndexMap<'_>,
    ) -> Line {
        let Some(first_glyph) = layout_line.glyphs.first() else {
            return self.empty_line(line_style, style_runs);
        };

        let mut runs = RunBuilder::new(
            &self.store,
            styles,
            indices,
            self.store.font_id_for_face(first_glyph.font_id),
            layout_line.glyphs.len(),
        );
        for glyph in layout_line.glyphs {
            runs.push_glyph(glyph);
        }

        Line {
            width: layout_line.w,
            runs: runs.build(),
            font_size: line_style.font_size,
            line_height_ratio: line_style.line_height_ratio,
            baseline_ratio: line_style.baseline_ratio,
            ascent: layout_line.max_ascent,
            descent: layout_line.max_descent,
        }
    }

    fn empty_line(
        &self,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
    ) -> Line {
        let (ascent, descent) = self.empty_line_extents(style_runs, line_style.font_size);
        Line {
            width: 0.,
            runs: Vec::new(),
            font_size: line_style.font_size,
            line_height_ratio: line_style.line_height_ratio,
            baseline_ratio: line_style.baseline_ratio,
            ascent,
            descent,
        }
    }
}

impl platform::TextLayoutSystem for CosmicTextLayout {
    fn layout_line(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
    ) -> Line {
        let indices = StrIndexMap::new(text);
        let mut styles = TextStylesMap::new();
        let attrs_list = self.build_attrs_list(&indices, style_runs, &mut styles);

        let tab_width = line_style
            .fixed_width_tab_size
            .map_or(DEFAULT_TAB_WIDTH, u16::from);

        // The shaped line owns its spans, so the font system is only borrowed
        // for shaping and is free again before layout runs.
        let shape_line = {
            let mut font_system = self.store.font_system.write();
            ShapeLine::new(
                &mut font_system,
                indices.shaped_text(),
                &attrs_list,
                Shaping::Advanced,
                tab_width,
            )
        };

        let layout = shape_line.layout(
            line_style.font_size,
            // A non-finite width reaches cosmic-text's fitting arithmetic
            // directly; an element that has not been given a constraint yet
            // routinely passes one.
            max_width.is_finite().then_some(max_width),
            // A line is a line. Wrapping is `layout_text`'s job, and Crook does
            // not have one yet.
            Wrap::None,
            Some(Align::Left),
            None,
            // Glyphs keep their subpixel x positions: they are rasterized into
            // three horizontal subpixel bins, and snapping x during layout
            // would throw that away.
            Hinting::Disabled,
        );

        debug_assert!(
            layout.len() <= 1,
            "Wrap::None must produce at most one line, got {}",
            layout.len()
        );
        let Some(layout_line) = layout.into_iter().next() else {
            return self.empty_line(line_style, style_runs);
        };
        self.build_line(layout_line, line_style, style_runs, &styles, &indices)
    }
}

/// Assembles [`Run`]s out of a stream of shaped glyphs.
///
/// A port of Warp's `winit/fonts/text_layout.rs`. A run ends when the face or
/// the paint style changes, and the face is a proxy for weight and slant, so
/// this single test covers "the caller asked for bold here" and "the shaper had
/// to reach for another font to cover this character" alike.
struct RunBuilder<'a> {
    store: &'a FontStore,
    styles: &'a TextStylesMap,
    indices: &'a StrIndexMap<'a>,
    runs: Vec<Run>,
    current_font: FontId,
    current_style: TextStyle,
    current_width: f32,
    current_glyphs: Vec<Glyph>,
}

impl<'a> RunBuilder<'a> {
    fn new(
        store: &'a FontStore,
        styles: &'a TextStylesMap,
        indices: &'a StrIndexMap<'a>,
        initial_font: FontId,
        glyph_count: usize,
    ) -> Self {
        Self {
            store,
            styles,
            indices,
            runs: Vec::new(),
            current_font: initial_font,
            current_style: TextStyle::default(),
            current_width: 0.,
            current_glyphs: Vec::with_capacity(glyph_count),
        }
    }

    fn push_glyph(&mut self, glyph: LayoutGlyph) {
        let font_id = self.store.font_id_for_face(glyph.font_id);
        let style = self.styles.get(glyph.metadata);

        if font_id != self.current_font || style != self.current_style {
            self.flush();
            self.current_font = font_id;
            self.current_style = style;
            self.current_width = 0.;
        }

        self.current_glyphs.push(Glyph {
            id: glyph.glyph_id.into(),
            position_along_baseline: vec2f(glyph.x, glyph.y),
            index: self.indices.source_index(glyph.start),
            width: glyph.w,
        });
        self.current_width += glyph.w;
    }

    /// Closes the run in progress. A run with no glyphs in it is dropped rather
    /// than pushed, so a style change before the first glyph costs nothing.
    fn flush(&mut self) {
        if self.current_glyphs.is_empty() {
            return;
        }

        self.runs.push(Run {
            font_id: self.current_font,
            glyphs: std::mem::take(&mut self.current_glyphs),
            styles: self.current_style,
            width: self.current_width,
        });
    }

    fn build(mut self) -> Vec<Run> {
        self.flush();
        self.runs
    }
}

/// Smuggles a [`TextStyle`] through cosmic-text's `usize` span metadata.
///
/// A `Vec` behind a newtype rather than a bare `Vec`, because the index *is*
/// the key: anything that reorders or removes entries silently repaints text in
/// another run's colors.
struct TextStylesMap {
    styles: Vec<TextStyle>,
}

impl TextStylesMap {
    /// Index zero is the default style and is never handed out.
    ///
    /// Text no style run covers keeps the attribute list's default metadata,
    /// which is zero. Warp's version starts numbering at zero and so paints
    /// uncovered text in the *first* run's colors; reserving the slot costs one
    /// entry and makes uncovered text look uncovered.
    fn new() -> Self {
        Self {
            styles: vec![TextStyle::default()],
        }
    }

    /// Parks a style and returns the metadata value that retrieves it.
    fn insert(&mut self, style: TextStyle) -> usize {
        self.styles.push(style);
        self.styles.len() - 1
    }

    /// The style at `index`, or the default when nothing is parked there.
    fn get(&self, index: usize) -> TextStyle {
        self.styles.get(index).copied().unwrap_or_default()
    }
}
