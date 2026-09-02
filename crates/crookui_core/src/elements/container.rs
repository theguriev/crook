//! The one element that draws a box.

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Color, Point, RectF, Vector2F, vec2f};
use crate::presenter::{EventContext, LayoutContext, PaintContext};
use crate::scene::{Border, CornerRadius, Fill};

/// Space outside a container's box.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Margin {
    /// Space above.
    pub top: f32,
    /// Space to the left.
    pub left: f32,
    /// Space below.
    pub bottom: f32,
    /// Space to the right.
    pub right: f32,
}

impl Margin {
    /// The same margin on all four sides.
    pub const fn uniform(margin: f32) -> Self {
        Self {
            top: margin,
            left: margin,
            bottom: margin,
            right: margin,
        }
    }
}

/// Space between a container's box and its child.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Padding {
    /// Space above.
    pub top: f32,
    /// Space to the left.
    pub left: f32,
    /// Space below.
    pub bottom: f32,
    /// Space to the right.
    pub right: f32,
}

impl Padding {
    /// The same padding on all four sides.
    pub const fn uniform(padding: f32) -> Self {
        Self {
            top: padding,
            left: padding,
            bottom: padding,
            right: padding,
        }
    }
}

/// Wraps a child in a box model: margin, border, padding, background.
///
/// This is the only element that paints a rectangle, and it paints exactly
/// one. Everything else is either pure layout or pure content, so styling is
/// composed rather than cascaded — there is no style trait and no inheritance.
/// A tab and the usage chip are both a `Container` around a
/// [`Flex`](super::Flex).
pub struct Container {
    margin: Margin,
    padding: Padding,
    background: Fill,
    border: Border,
    corner_radius: CornerRadius,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Container {
    /// Wraps `child` with no styling at all.
    pub fn new(child: Box<dyn Element>) -> Self {
        Self {
            margin: Margin::default(),
            padding: Padding::default(),
            background: Fill::None,
            border: Border::default(),
            corner_radius: CornerRadius::default(),
            child,
            size: None,
            origin: None,
        }
    }

    /// Sets every padding edge.
    pub fn with_uniform_padding(self, padding: f32) -> Self {
        self.with_padding(Padding::uniform(padding))
    }

    /// Sets all four padding edges at once.
    pub fn with_padding(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    /// Sets the left and right padding.
    pub fn with_horizontal_padding(mut self, padding: f32) -> Self {
        self.padding.left = padding;
        self.padding.right = padding;
        self
    }

    /// Sets the top and bottom padding.
    pub fn with_vertical_padding(mut self, padding: f32) -> Self {
        self.padding.top = padding;
        self.padding.bottom = padding;
        self
    }

    /// Sets the top padding.
    pub fn with_padding_top(mut self, padding: f32) -> Self {
        self.padding.top = padding;
        self
    }

    /// Sets the left padding.
    pub fn with_padding_left(mut self, padding: f32) -> Self {
        self.padding.left = padding;
        self
    }

    /// Sets the bottom padding.
    pub fn with_padding_bottom(mut self, padding: f32) -> Self {
        self.padding.bottom = padding;
        self
    }

    /// Sets the right padding.
    pub fn with_padding_right(mut self, padding: f32) -> Self {
        self.padding.right = padding;
        self
    }

    /// Sets every margin edge.
    pub fn with_uniform_margin(self, margin: f32) -> Self {
        self.with_margin(Margin::uniform(margin))
    }

    /// Sets all four margin edges at once.
    pub fn with_margin(mut self, margin: Margin) -> Self {
        self.margin = margin;
        self
    }

    /// Sets the left and right margin.
    pub fn with_horizontal_margin(mut self, margin: f32) -> Self {
        self.margin.left = margin;
        self.margin.right = margin;
        self
    }

    /// Sets the top and bottom margin.
    pub fn with_vertical_margin(mut self, margin: f32) -> Self {
        self.margin.top = margin;
        self.margin.bottom = margin;
        self
    }

    /// Sets the top margin.
    pub fn with_margin_top(mut self, margin: f32) -> Self {
        self.margin.top = margin;
        self
    }

    /// Sets the left margin.
    pub fn with_margin_left(mut self, margin: f32) -> Self {
        self.margin.left = margin;
        self
    }

    /// Sets the bottom margin.
    pub fn with_margin_bottom(mut self, margin: f32) -> Self {
        self.margin.bottom = margin;
        self
    }

    /// Sets the right margin.
    pub fn with_margin_right(mut self, margin: f32) -> Self {
        self.margin.right = margin;
        self
    }

    /// Fills the box with a flat color.
    pub fn with_background_color(self, color: Color) -> Self {
        self.with_background(color)
    }

    /// Fills the box.
    pub fn with_background(mut self, background: impl Into<Fill>) -> Self {
        self.background = background.into();
        self
    }

    /// Strokes the box. The stroke is inset, so it takes layout space.
    pub fn with_border(mut self, border: impl Into<Border>) -> Self {
        self.border = border.into();
        self
    }

    /// Rounds the box's corners.
    pub fn with_corner_radius(mut self, corner_radius: CornerRadius) -> Self {
        self.corner_radius = corner_radius;
        self
    }

    /// How much bigger this container is than its child.
    fn size_buffer(&self) -> Vector2F {
        vec2f(
            self.margin.left
                + self.margin.right
                + self.padding.left
                + self.padding.right
                + self.border.left_width()
                + self.border.right_width(),
            self.margin.top
                + self.margin.bottom
                + self.padding.top
                + self.padding.bottom
                + self.border.top_width()
                + self.border.bottom_width(),
        )
    }
}

impl Element for Container {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size_buffer = self.size_buffer();
        let max = (constraint.max - size_buffer).max(Vector2F::zero());
        let child_constraint = SizeConstraint {
            // Clamping the minimum against the maximum matters when a padded
            // container sits in a tight parent: without it the child would be
            // handed a constraint it cannot satisfy.
            min: (constraint.min - size_buffer)
                .max(Vector2F::zero())
                .min(max),
            max,
        };

        // The child was measured against a maximum this container's own box
        // model had already been taken out of, so the sum only exceeds the
        // constraint when there was less room than the margin, border and
        // padding need. Reporting that surplus would push every later sibling
        // along by it — a strip of narrow tabs would run off the end of the
        // header and under the controls to its right. The box shrinks instead,
        // and a subtree that must not spill out of it says so with a
        // [`Clipped`](super::Clipped).
        let size =
            (self.child.layout(child_constraint, ctx, app) + size_buffer).min(constraint.max);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let size = self
            .size
            .expect("a container was painted before it was laid out");
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));

        let box_origin = origin + vec2f(self.margin.left, self.margin.top);
        let box_size = size
            - vec2f(
                self.margin.left + self.margin.right,
                self.margin.top + self.margin.bottom,
            );

        ctx.scene
            .draw_rect_with_hit_recording(RectF::new(box_origin, box_size))
            .with_background(self.background)
            .with_border(self.border)
            .with_corner_radius(self.corner_radius);

        let child_origin = box_origin
            + vec2f(
                self.padding.left + self.border.left_width(),
                self.padding.top + self.border.top_width(),
            );
        self.child.paint(child_origin, ctx, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
