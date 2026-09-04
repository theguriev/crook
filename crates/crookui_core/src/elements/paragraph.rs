//! Several lines of text, broken to the width they are given.
//!
//! [`Text`](super::Text) is one line and is cut off at the edge, which is
//! right for a tab title and a chip label. A description is not either of
//! those, and until this existed every paragraph in Crook was broken by
//! *counting characters* — a constant per surface, guessed against the width
//! that surface happened to have, and wrong the moment the surface could be
//! two widths.
//!
//! # It measures rather than counts
//!
//! The whole string is shaped once against an unbounded width, which gives the
//! x of every character; the breaks are then chosen by arithmetic on those,
//! and each line is shaped again for painting. So a paragraph is `1 + lines`
//! shaping calls per layout, and it fits the box it is actually in rather than
//! the box somebody expected.
//!
//! Greedy and word-based: the longest run of words that fits goes on the line.
//! No hyphenation and no penalty function — this is a settings description,
//! not a book.

use std::borrow::Cow;

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::fonts::{
    DEFAULT_TOP_BOTTOM_RATIO, DEFAULT_UI_LINE_HEIGHT_RATIO, FamilyId, LineStyle, Properties,
    StyleAndFont, TextStyle,
};
use crate::geometry::{Color, Point, RectF, Vector2F, vec2f};
use crate::presenter::{EventContext, LayoutContext, PaintContext};
use crate::text_layout::Line;

/// Draws text over as many lines as it takes.
pub struct Paragraph {
    text: Cow<'static, str>,
    font_family: FamilyId,
    font_size: f32,
    line_height_ratio: f32,
    properties: Properties,
    color: Color,
    lines: Vec<Line>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Paragraph {
    /// Text in `font_family` at `font_size` logical pixels.
    pub fn new(text: impl Into<Cow<'static, str>>, font_family: FamilyId, font_size: f32) -> Self {
        Self {
            text: text.into(),
            font_family,
            font_size,
            line_height_ratio: DEFAULT_UI_LINE_HEIGHT_RATIO,
            properties: Properties::default(),
            color: Color::WHITE,
            lines: Vec::new(),
            size: None,
            origin: None,
        }
    }

    /// Sets the glyph color.
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Sets the weight and slant to select within the family.
    pub fn with_style(mut self, properties: Properties) -> Self {
        self.properties = properties;
        self
    }

    /// Sets the line height as a multiple of the font size.
    ///
    /// A paragraph wants more of it than a label: lines set solid are lines
    /// the eye loses its place in.
    pub fn with_line_height_ratio(mut self, line_height_ratio: f32) -> Self {
        self.line_height_ratio = line_height_ratio;
        self
    }

    /// Shapes one run of text against an unbounded width.
    fn shape(&self, text: &str, ctx: &mut LayoutContext) -> Line {
        let style_runs = [(
            0..text.len(),
            StyleAndFont::new(self.font_family, self.properties, TextStyle::default()),
        )];
        ctx.text_layout.layout_line(
            text,
            LineStyle {
                font_size: self.font_size,
                line_height_ratio: self.line_height_ratio,
                baseline_ratio: DEFAULT_TOP_BOTTOM_RATIO,
                fixed_width_tab_size: None,
            },
            &style_runs,
            f32::INFINITY,
        )
    }
}

/// Where each line of `text` starts and ends, given the x of every character.
///
/// Separated from the element so that what "breaking" means can be tested
/// without a layout pass: it is arithmetic on measurements, and the
/// measurements are the only thing that needs a shaper.
///
/// A word wider than `width` gets a line of its own and overflows it — the
/// alternative is breaking inside a word, which for a path or a plugin id is
/// worse than a line that runs long.
fn break_lines(text: &str, width: f32, x_of: impl Fn(usize) -> f32) -> Vec<std::ops::Range<usize>> {
    let words: Vec<(usize, usize)> = text
        .split_whitespace()
        .map(|word| {
            let start = word.as_ptr() as usize - text.as_ptr() as usize;
            (start, start + word.len())
        })
        .collect();

    let mut lines: Vec<std::ops::Range<usize>> = Vec::new();
    let Some(&(first, first_end)) = words.first() else {
        // Nothing but whitespace still makes one line, so a column does not
        // jump when a description arrives.
        lines.push(0..text.len());
        return lines;
    };

    let mut start = first;
    let mut end = first_end;
    for &(word_start, word_end) in &words[1..] {
        if x_of(word_end) - x_of(start) <= width {
            end = word_end;
            continue;
        }
        lines.push(start..end);
        start = word_start;
        end = word_end;
    }
    lines.push(start..end);
    lines
}

impl Element for Paragraph {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        // Once, unbounded, to find out where every character is. `Wrap::None`
        // is what the shaper does with a finite width, so a bounded call would
        // hand back the same single line and no more information.
        let measured = self.shape(&self.text, ctx);
        let ranges = break_lines(&self.text, constraint.max.x(), |index| {
            measured.x_for_index(index)
        });

        self.lines = ranges
            .into_iter()
            .map(|range| {
                let text = self.text[range].to_owned();
                self.shape(&text, ctx)
            })
            .collect();

        let width = self.lines.iter().map(|line| line.width).fold(0., f32::max);
        let height = self.lines.iter().map(Line::height).sum();
        let size = constraint.apply(vec2f(width, height));
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        let size = self
            .size
            .expect("a paragraph was painted before it was laid out");
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));

        let mut top = origin.y();
        for line in &self.lines {
            let height = line.height();
            line.paint(
                RectF::new(vec2f(origin.x(), top), vec2f(size.x(), height)),
                self.color,
                ctx.scene,
            );
            top += height;
        }
    }

    fn dispatch_event(
        &mut self,
        _: &DispatchedEvent,
        _: &mut EventContext,
        _: &AppContext,
    ) -> bool {
        false
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

#[cfg(test)]
mod tests {
    use super::break_lines;

    /// The stub shaper's arithmetic: every character is half the font size
    /// wide, so a character is one unit here.
    fn x_of(text: &str) -> impl Fn(usize) -> f32 + '_ {
        move |index| text[..index.min(text.len())].chars().count() as f32
    }

    fn lines(text: &str, width: f32) -> Vec<&str> {
        break_lines(text, width, x_of(text))
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    #[test]
    fn the_longest_run_of_words_that_fits_goes_on_the_line() {
        assert_eq!(
            lines("one two three four", 8.),
            ["one two", "three", "four"]
        );
    }

    #[test]
    fn a_word_wider_than_the_line_gets_one_of_its_own_and_overflows_it() {
        // Breaking inside a word is worse than a line that runs long: a path
        // or a plugin id broken in half is a thing nobody can copy.
        assert_eq!(
            lines("a supercalifragilistic b", 8.),
            ["a", "supercalifragilistic", "b"]
        );
    }

    #[test]
    fn text_that_fits_is_one_line() {
        assert_eq!(lines("one two", 100.), ["one two"]);
    }

    #[test]
    fn nothing_is_one_empty_line_rather_than_no_lines() {
        // A paragraph with no words still has a height, which is what keeps a
        // column from jumping when a description arrives.
        assert_eq!(lines("", 10.), [""]);
        assert_eq!(lines("   ", 10.), ["   "]);
    }

    #[test]
    fn the_space_between_two_words_is_not_carried_to_the_next_line() {
        let text = "one two three";
        let broken = break_lines(text, 8., x_of(text));

        assert_eq!(broken, [0..7, 8..13]);
    }
}
