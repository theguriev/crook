//! The output of shaping a string, and how it reaches the [`Scene`].
//!
//! Shaping itself needs a font backend, so it happens in the platform crate
//! behind [`crate::platform::TextLayoutSystem`]. What lives here is the half
//! that does not: the positioned-glyph data a shaper produces, and the walk
//! that turns it into draw commands.
//!
//! The contract between the two halves is that a [`Line`] is self-contained —
//! it carries its own ascent, descent and font size, so painting it needs no
//! font metrics and no cache lookup.

use crate::fonts::{FontId, GlyphId, TextStyle};
use crate::geometry::{Color, RectF, Vector2F, vec2f};
use crate::scene::Scene;

/// One positioned glyph within a [`Run`].
#[derive(Clone, Debug)]
pub struct Glyph {
    /// The glyph index in the run's face.
    pub id: GlyphId,

    /// Where the glyph's left edge meets the baseline, relative to the line's
    /// origin.
    pub position_along_baseline: Vector2F,

    /// Byte offset into the source string where the character this glyph came
    /// from starts. Neither injective nor monotonic: ligatures collapse
    /// several characters into one glyph, and bidi reorders them.
    pub index: usize,

    /// The glyph's advance in logical pixels.
    pub width: f32,
}

/// Consecutive glyphs that share one face and one style.
///
/// A run breaks whenever either changes — which is why the shaper, not the
/// caller, decides where runs begin: font fallback can split a single styled
/// range across several faces.
#[derive(Clone, Debug)]
pub struct Run {
    /// The face every glyph in this run came from.
    pub font_id: FontId,
    /// The glyphs, in visual order.
    pub glyphs: Vec<Glyph>,
    /// How the run should be painted.
    pub styles: TextStyle,
    /// The sum of the run's advances.
    pub width: f32,
}

/// A single shaped line of text.
#[derive(Clone, Debug)]
pub struct Line {
    /// The total advance width in logical pixels.
    pub width: f32,
    /// The runs making up the line, in visual order.
    pub runs: Vec<Run>,
    /// The em size the line was shaped at.
    pub font_size: f32,
    /// Line height as a multiple of `font_size`.
    pub line_height_ratio: f32,
    /// Where the baseline sits within the leftover line height.
    pub baseline_ratio: f32,
    /// Maximum rise above the baseline, in logical pixels.
    pub ascent: f32,
    /// Maximum drop below the baseline, in logical pixels, as a positive
    /// number — the opposite sign convention from [`crate::fonts::Metrics`].
    pub descent: f32,
}

impl Default for Line {
    fn default() -> Self {
        Self {
            width: 0.,
            runs: Vec::new(),
            font_size: 0.,
            line_height_ratio: crate::fonts::DEFAULT_UI_LINE_HEIGHT_RATIO,
            baseline_ratio: crate::fonts::DEFAULT_TOP_BOTTOM_RATIO,
            ascent: 0.,
            descent: 0.,
        }
    }
}

/// Where the baseline sits inside a line box, measured from its top.
///
/// The text box (ascent plus descent) is centered in the line box, and the
/// baseline is one ascent below the top of that.
pub fn default_compute_baseline_position(
    font_size: f32,
    line_height_ratio: f32,
    ascent: f32,
    descent: f32,
) -> f32 {
    let line_height = font_size * line_height_ratio;
    let text_height = ascent + descent;
    let padding_top = (line_height - text_height) / 2.;
    padding_top + ascent
}

/// How thick an underline is drawn, as a fraction of the font size.
const UNDERLINE_THICKNESS_RATIO: f32 = 0.06;

/// How far below the baseline an underline is drawn, as a fraction of the
/// descent.
const UNDERLINE_OFFSET_RATIO: f32 = 0.4;

impl Line {
    /// The height of the line box in logical pixels.
    pub fn height(&self) -> f32 {
        self.font_size * self.line_height_ratio
    }

    /// Where the baseline sits, measured down from the top of the line box.
    pub fn baseline_position(&self) -> f32 {
        default_compute_baseline_position(
            self.font_size,
            self.line_height_ratio,
            self.ascent,
            self.descent,
        )
    }

    /// The x offset of the glyph at `index` characters into the line, or the
    /// line's width when `index` is past the end.
    pub fn x_for_index(&self, index: usize) -> f32 {
        self.runs
            .iter()
            .flat_map(|run| run.glyphs.iter())
            .find(|glyph| glyph.index >= index)
            .map(|glyph| glyph.position_along_baseline.x())
            .unwrap_or(self.width)
    }

    /// Pushes this line's glyphs into `scene`, laid out inside `bounds`.
    ///
    /// `bounds` is the *line box*: its origin is the top-left of the line, not
    /// the baseline, and its width is how much room the glyphs have. Glyphs
    /// that would overflow are dropped rather than drawn outside — a tab title
    /// too long for its tab stops at the edge.
    ///
    /// `default_color` paints every run that does not name its own foreground.
    pub fn paint(&self, bounds: RectF, default_color: Color, scene: &mut Scene) {
        let line_origin = bounds.origin() + vec2f(0., self.baseline_position());
        let available_width = bounds.width();

        for run in &self.runs {
            let color = run.styles.foreground_color.unwrap_or(default_color);

            let visible_width = run
                .glyphs
                .iter()
                .take_while(|glyph| Self::fits(glyph, available_width))
                .map(|glyph| glyph.width)
                .sum::<f32>();
            if visible_width <= 0. {
                continue;
            }

            let run_start = run
                .glyphs
                .first()
                .map(|glyph| glyph.position_along_baseline.x())
                .unwrap_or_default();

            // Backgrounds and underlines are rects behind and under the run, so
            // they have to be drawn before its glyphs to avoid covering them.
            if let Some(background) = run.styles.background_color {
                scene
                    .draw_rect_without_hit_recording(RectF::new(
                        bounds.origin() + vec2f(run_start, 0.),
                        vec2f(visible_width, self.height()),
                    ))
                    .with_background(background);
            }

            if let Some(underline) = run.styles.underline_color {
                let offset = self.descent * UNDERLINE_OFFSET_RATIO;
                let thickness = (self.font_size * UNDERLINE_THICKNESS_RATIO).max(1.);
                scene
                    .draw_rect_without_hit_recording(RectF::new(
                        line_origin + vec2f(run_start, offset),
                        vec2f(visible_width, thickness),
                    ))
                    .with_background(underline);
            }

            for glyph in &run.glyphs {
                if !Self::fits(glyph, available_width) {
                    break;
                }
                scene.draw_glyph(
                    line_origin + glyph.position_along_baseline,
                    glyph.id,
                    run.font_id,
                    self.font_size,
                    color,
                );
            }
        }
    }

    fn fits(glyph: &Glyph, available_width: f32) -> bool {
        // Half a pixel of slack, so a line painted at exactly the width it
        // measured does not lose its last glyph to accumulated float error.
        const TOLERANCE: f32 = 0.5;

        glyph.position_along_baseline.x() + glyph.width <= available_width + TOLERANCE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::GlyphKey;
    use crate::geometry::Point;
    use crate::scene::ClipBounds;

    fn line(glyph_widths: &[f32], styles: TextStyle) -> Line {
        let mut x = 0.;
        let glyphs = glyph_widths
            .iter()
            .enumerate()
            .map(|(index, width)| {
                let glyph = Glyph {
                    id: index as u32,
                    position_along_baseline: vec2f(x, 0.),
                    index,
                    width: *width,
                };
                x += width;
                glyph
            })
            .collect();

        Line {
            width: x,
            runs: vec![Run {
                font_id: FontId(0),
                glyphs,
                styles,
                width: x,
            }],
            font_size: 10.,
            ascent: 8.,
            descent: 2.,
            ..Default::default()
        }
    }

    fn painted_glyphs(line: &Line, bounds: RectF) -> Vec<GlyphKey> {
        let mut scene = Scene::new(1.);
        line.paint(bounds, Color::WHITE, &mut scene);
        scene
            .layers()
            .flat_map(|layer| layer.glyphs.iter())
            .map(|glyph| glyph.glyph_key)
            .collect()
    }

    #[test]
    fn glyphs_sit_on_the_baseline_below_the_line_top() {
        let line = line(&[5.], TextStyle::default());
        let mut scene = Scene::new(1.);
        line.paint(
            RectF::new(vec2f(100., 200.), vec2f(50., 12.)),
            Color::WHITE,
            &mut scene,
        );

        let glyph = scene.layers().flat_map(|l| l.glyphs.iter()).next().unwrap();
        assert_eq!(glyph.position.x(), 100.);
        // Line box 12px, text box 10px, so 1px of padding above the ascent.
        assert_eq!(glyph.position.y(), 200. + 1. + 8.);
    }

    #[test]
    fn glyphs_past_the_available_width_are_dropped() {
        let line = line(&[5., 5., 5.], TextStyle::default());
        assert_eq!(
            painted_glyphs(&line, RectF::new(Vector2F::zero(), vec2f(11., 12.))).len(),
            2
        );
        assert_eq!(
            painted_glyphs(&line, RectF::new(Vector2F::zero(), vec2f(15., 12.))).len(),
            3
        );
    }

    #[test]
    fn a_backgrounded_run_draws_a_rect_that_is_click_through() {
        let styles = TextStyle {
            background_color: Some(Color::BLACK),
            ..Default::default()
        };
        let mut scene = Scene::new(1.);
        let under_the_text = Point::from_vec2f(vec2f(1., 1.), scene.z_index());
        scene.start_layer(ClipBounds::None);
        line(&[5., 5.], styles).paint(
            RectF::new(Vector2F::zero(), vec2f(20., 12.)),
            Color::WHITE,
            &mut scene,
        );
        scene.stop_layer();

        assert_eq!(scene.layers().flat_map(|l| l.rects.iter()).count(), 1);
        assert!(
            !scene.is_covered(under_the_text),
            "text decoration must not swallow clicks meant for what is under it"
        );
    }
}

/// `text` shaped as one line that reads left to right whatever letters it
/// holds: every character in the order it comes, but for the letters of
/// each right-to-left run, which read right to left within it as they are
/// written. With its glyphs' [`Glyph::index`] into `text`, and `style_runs`
/// as [`TextLayoutSystem::layout_line`] takes them, into `text`.
///
/// A shaper takes a line's direction from its first strong letter and
/// reorders the neutral characters between right-to-left letters with
/// them, so `: ع ; rm -rf $HOME # ع ; echo done` is drawn
/// `echo done ; ع # rm -rf $HOME ; ع :` — the `rm` behind a `#`, where a
/// shell runs it. For text drawn so that a person can tell what will run,
/// that order is the one thing that may not change. A left-to-right mark
/// first sets the line's direction, and one after each right-to-left run
/// — its combining marks with it — leaves nothing for a neutral to be
/// reordered with.
///
/// [`TextLayoutSystem::layout_line`]: crate::platform::TextLayoutSystem::layout_line
pub fn layout_left_to_right(
    text_layout: &dyn crate::platform::TextLayoutSystem,
    text: &str,
    line_style: crate::fonts::LineStyle,
    style_runs: &[(std::ops::Range<usize>, crate::fonts::StyleAndFont)],
    max_width: f32,
) -> Line {
    let (marked, at) = left_to_right(text);
    let runs: Vec<_> = style_runs
        .iter()
        .map(|(range, style)| (at[range.start]..at[range.end], *style))
        .collect();
    let mut line = text_layout.layout_line(&marked, line_style, &runs, max_width);
    // Back to `text`'s offsets: a byte of `marked` is the byte of `text` it
    // came from, and an inserted mark the one after it.
    for glyph in line.runs.iter_mut().flat_map(|run| run.glyphs.iter_mut()) {
        glyph.index = at.partition_point(|&shifted| shifted < glyph.index);
    }
    line
}

/// LRM, U+200E: a strong left-to-right character that draws as nothing.
const LEFT_TO_RIGHT_MARK: char = '\u{200E}';

/// `text`, ready to be drawn as one line that reads left to right: as it
/// is when it has nothing that reads right to left in it, and otherwise
/// with the marks [`left_to_right`] puts in. For a single line of one
/// style — a label, a row's summary — whose offsets nobody maps back.
pub fn reads_left_to_right(text: &str) -> std::borrow::Cow<'_, str> {
    use unicode_bidi::{BidiClass, bidi_class};
    if text.chars().any(|character| {
        matches!(
            bidi_class(character),
            BidiClass::R | BidiClass::AL | BidiClass::AN
        )
    }) {
        std::borrow::Cow::Owned(left_to_right(text).0)
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// `text` with a left-to-right mark first and after each right-to-left
/// run, and where each byte offset of `text`, its end included, lands in it.
pub fn left_to_right(text: &str) -> (String, Vec<usize>) {
    use unicode_bidi::{BidiClass, bidi_class};
    // A right-to-left letter, or an Arabic-Indic digit: one of those the
    // algorithm counts as right to left when it orders what lies between
    // them, so that `٣# ;: ٤` would be drawn `٤ :; #٣`.
    let right_to_left = |character: char| {
        matches!(
            bidi_class(character),
            BidiClass::R | BidiClass::AL | BidiClass::AN
        )
    };
    let mut marked = String::with_capacity(text.len() + 6);
    let mut at = vec![0; text.len() + 1];
    marked.push(LEFT_TO_RIGHT_MARK);
    let mut in_run = false;
    for (start, character) in text.char_indices() {
        let goes_on =
            right_to_left(character) || (in_run && bidi_class(character) == BidiClass::NSM);
        if in_run && !goes_on {
            marked.push(LEFT_TO_RIGHT_MARK);
        }
        in_run = goes_on;
        let shift = marked.len();
        for (byte, lands) in at[start..start + character.len_utf8()]
            .iter_mut()
            .enumerate()
        {
            *lands = shift + byte;
        }
        marked.push(character);
    }
    at[text.len()] = marked.len();
    if in_run {
        marked.push(LEFT_TO_RIGHT_MARK);
    }
    (marked, at)
}

#[cfg(test)]
mod left_to_right_tests {
    use super::left_to_right;

    /// `text` with its marks shown as `|`.
    fn marked(text: &str) -> String {
        left_to_right(text).0.replace('\u{200e}', "|")
    }

    #[test]
    fn a_mark_starts_the_line_and_closes_each_right_to_left_run() {
        assert_eq!(marked("echo hi"), "|echo hi");
        assert_eq!(marked(": ع ; rm"), "|: ع| ; rm");
        assert_eq!(marked("مرحبا بالعالم"), "|مرحبا| بالعالم|");
        // A number after a right-to-left letter is its own, in its order.
        assert_eq!(marked("\"ع1\";ع2\""), "|\"ع|1\";ع|2\"");
        // Nothing comes between letters that join, nor between a letter and
        // its marks: lam-alef and harakat stay one shape.
        assert_eq!(marked("لا"), "|لا|");
        assert_eq!(marked("بَيْت x"), "|بَيْت| x");
        assert_eq!(marked("שָׁלוֹם"), "|שָׁלוֹם|");
    }

    #[test]
    fn arabic_indic_digits_are_a_run_of_their_own() {
        // Counted as right to left between them, they reorder what lies
        // between: `٣# ;: ٤` drawn `٤ :; #٣`.
        assert_eq!(marked("true ٣# ;: ٤ ; rm"), "|true ٣|# ;: ٤| ; rm");
        assert_eq!(marked("cat ٣ > ٤"), "|cat ٣| > ٤|");
        assert_eq!(
            marked("a \u{10e60} | \u{10e61}"),
            "|a \u{10e60}| | \u{10e61}|"
        );
        assert_eq!(marked("1\u{66b}2"), "|1\u{66b}|2");
    }

    /// Where each character of `text` is drawn, left to right, by the
    /// Unicode bidirectional algorithm as cosmic-text applies it: the line's
    /// direction from its first strong letter, each level's run reversed
    /// when it is right to left, as offsets into `text`.
    fn drawn_order(text: &str) -> Vec<usize> {
        let bidi = unicode_bidi::BidiInfo::new(text, None);
        let mut order = Vec::new();
        for paragraph in &bidi.paragraphs {
            let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
            for run in runs {
                let mut offsets: Vec<usize> = text[run.clone()]
                    .char_indices()
                    .map(|(at, _)| run.start + at)
                    .collect();
                if levels[run.start].is_rtl() {
                    offsets.reverse();
                }
                order.extend(offsets);
            }
        }
        order
    }

    #[test]
    fn no_ascii_character_is_ever_drawn_out_of_its_order() {
        // What a shell reads is ASCII. Whatever else a line holds — letters
        // of any direction, digits of any kind, brackets, marks — its ASCII
        // characters are drawn in the order they run, so the next class of
        // character that reorders a line is caught here and not by a reader.
        let pool: Vec<char> = concat!(
            "abz09 #'\"`;|&$<>()[]{}\\/*?!~=-+.,:%_",
            "عبتمرحلا",
            "אבגש",
            "٠١٢٣٩",
            "۰۱۹",
            "\u{66b}\u{66c}\u{66a}\u{60c}\u{61b}",
            "\u{10e60}\u{10e6a}\u{10e7e}",
            "\u{64e}\u{651}\u{5b8}\u{301}",
            "漢字αβЖ",
            "（）［］｛｝«»‹›",
            "\u{2212}\u{b0}\u{20ac}\u{a3}\u{2030}",
            "\u{7c0}\u{7ca}\u{800}\u{840}\u{1e900}",
            "ހށܐܒ",
        )
        .chars()
        .collect();
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below as u64) as usize
        };
        for _ in 0..50_000 {
            let length = 1 + next(24);
            let text: String = (0..length).map(|_| pool[next(pool.len())]).collect();
            let (marked, at) = left_to_right(&text);
            let ascii: Vec<usize> = text
                .char_indices()
                .filter(|(_, character)| character.is_ascii())
                .map(|(offset, _)| at[offset])
                .collect();
            let drawn: Vec<usize> = drawn_order(&marked)
                .into_iter()
                .filter(|offset| ascii.contains(offset))
                .collect();
            assert_eq!(drawn, ascii, "{text:?} drawn out of order as {marked:?}");
        }
    }

    #[test]
    fn every_offset_lands_on_its_character() {
        let text = "a ع b";
        let (marked, at) = left_to_right(text);
        for (offset, character) in text.char_indices() {
            assert_eq!(marked[at[offset]..].chars().next(), Some(character));
        }
        assert_eq!(at[text.len()], marked.len());
        let (marked, at) = left_to_right("a ع");
        assert_eq!(at["a ع".len()], marked.len() - '\u{200e}'.len_utf8());
    }
}
