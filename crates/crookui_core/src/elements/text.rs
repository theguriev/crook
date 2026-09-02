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

/// Draws one line of text.
///
/// Shaping happens during layout, through the platform's text layout system,
/// and the shaped [`Line`] is kept for paint to walk. Text that does not fit
/// the width it was given is cut off at the edge rather than wrapped: this is
/// a tab title and a chip label, not a paragraph.
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
            line: None,
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
        let style_runs = [(
            0..self.text.len(),
            StyleAndFont::new(self.font_family, self.properties, TextStyle::default()),
        )];

        let line = ctx.text_layout.layout_line(
            &self.text,
            LineStyle {
                font_size: self.font_size,
                line_height_ratio: self.line_height_ratio,
                baseline_ratio: DEFAULT_TOP_BOTTOM_RATIO,
                fixed_width_tab_size: None,
            },
            &style_runs,
            constraint.max.x(),
        );

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
