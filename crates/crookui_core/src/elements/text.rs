//! A single line of text.

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

/// The mark a line that did not fit ends in.
const ELLIPSIS: char = '\u{2026}';

/// Draws one line of text.
///
/// Shaping happens during layout, through the platform's text layout system,
/// and the shaped [`Line`] is kept for paint to walk. Text that does not fit
/// the width it was given is cut off at the edge rather than wrapped: this is
/// a tab title and a chip label, not a paragraph. Ask for
/// [`with_ellipsis`](Self::with_ellipsis) and the cut is made here, at the
/// last glyph that leaves room for an ellipsis, rather than by the edge.
///
/// A `Text` records no hit rect, so it is never clickable on its own. Wrap it
/// in a [`Container`](super::Container) or a [`Hoverable`](super::Hoverable)
/// to give it one.
pub struct Text {
    text: Cow<'static, str>,
    font_family: FamilyId,
    font_size: f32,
    line_height_ratio: f32,
    properties: Properties,
    color: Color,
    ellipsis: bool,
    line: Option<Line>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Text {
    /// Text in `font_family` at `font_size` logical pixels.
    pub fn new(text: impl Into<Cow<'static, str>>, font_family: FamilyId, font_size: f32) -> Self {
        Self {
            text: text.into(),
            font_family,
            font_size,
            line_height_ratio: DEFAULT_UI_LINE_HEIGHT_RATIO,
            properties: Properties::default(),
            color: Color::WHITE,
            ellipsis: false,
            line: None,
            size: None,
            origin: None,
        }
    }

    /// Ends a line that does not fit its width in an ellipsis, where the
    /// glyphs stop, rather than letting the edge cut a word wherever it falls.
    ///
    /// Measured, not counted: a budget of characters is a guess at a width,
    /// and in a proportional face `eugen@omarchy` and `~/.local/sh` are the
    /// same count and not the same width. The cut is found from the shaped
    /// glyphs' own advances, so it lands exactly where the room runs out.
    pub fn with_ellipsis(mut self) -> Self {
        self.ellipsis = true;
        self
    }

    /// Shapes `text` the way [`Element::layout`] does.
    fn shape(&self, text: &str, ctx: &mut LayoutContext, max_width: f32) -> Line {
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
            max_width,
        )
    }

    /// The longest start of the text that, followed by an ellipsis, fits in
    /// `max_width` — shaped, so the answer is a line and not a guess.
    ///
    /// The first cut is read off the full line's glyph advances: the ellipsis
    /// takes the room from the end, and the glyphs that no longer fit name
    /// the byte the text is cut at. Cutting at the *lowest* such byte is what
    /// keeps a ligature or a bidi run whole — a glyph's byte index is neither
    /// injective nor monotonic, and a cut inside one would draw half of it.
    /// Reshaping can then come out a hair wider than the sum of the parts
    /// (kerning against the ellipsis), so the cut backs off a character at a
    /// time until the shaped line fits. Three tries has never been too few;
    /// past that the ellipsis alone is drawn, which is still a whole mark.
    fn cut(&self, line: &Line, ctx: &mut LayoutContext, max_width: f32) -> Line {
        let ellipsis = ELLIPSIS.to_string();
        let room = max_width - self.shape(&ellipsis, ctx, f32::INFINITY).width;
        if room <= 0. {
            return self.shape(&ellipsis, ctx, max_width);
        }

        let mut cut = line
            .runs
            .iter()
            .flat_map(|run| &run.glyphs)
            .filter(|glyph| glyph.position_along_baseline.x() + glyph.width > room)
            .map(|glyph| glyph.index)
            .min()
            .unwrap_or(self.text.len());

        for _ in 0..3 {
            let shorter = format!("{}{ellipsis}", &self.text[..cut]);
            let line = self.shape(&shorter, ctx, max_width);
            if line.width <= max_width || cut == 0 {
                return line;
            }
            cut = self.text[..cut]
                .char_indices()
                .next_back()
                .map_or(0, |(index, _)| index);
        }
        self.shape(&ellipsis, ctx, max_width)
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
    pub fn with_line_height_ratio(mut self, line_height_ratio: f32) -> Self {
        self.line_height_ratio = line_height_ratio;
        self
    }
}

impl Element for Text {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        let max_width = constraint.max.x();
        let mut line = self.shape(&self.text, ctx, max_width);
        // Only a line that overran a finite width is cut: an unbounded axis
        // is a flex measuring, and a line that fits is left as it was shaped.
        if self.ellipsis && max_width.is_finite() && line.width > max_width {
            line = self.cut(&line, ctx, max_width);
        }

        let size = constraint.apply(vec2f(line.width, line.height()));
        self.line = Some(line);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        let size = self.size.expect("text was painted before it was laid out");
        let line = self
            .line
            .as_ref()
            .expect("text was painted before it was laid out");

        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        line.paint(RectF::new(origin, size), self.color, ctx.scene);
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
