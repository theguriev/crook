//! Positioning a small child inside a large space.

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Point, Vector2F, vec2f};
use crate::presenter::{EventContext, LayoutContext, PaintContext};

/// Takes all the room it is offered and places its child within it.
///
/// This is also how a child is made to *fill* a space: a
/// [`Text`](super::Text) inside an [`Expanded`](super::Expanded) still
/// measures to its own width, but an `Align` between them takes the whole
/// share.
pub struct Align {
    child: Box<dyn Element>,
    /// Where the child sits, per axis, from `-1.` (leading) through `0.`
    /// (centered) to `1.` (trailing).
    alignment: Vector2F,
    size: Option<Vector2F>,
}

impl Align {
    /// Wraps `child`, centered.
    pub fn new(child: Box<dyn Element>) -> Self {
        Self {
            child,
            alignment: Vector2F::zero(),
            size: None,
        }
    }

    /// Aligns the child to the left, centered vertically.
    pub fn left(mut self) -> Self {
        self.alignment = vec2f(-1., 0.);
        self
    }

    /// Aligns the child to the right, centered vertically.
    pub fn right(mut self) -> Self {
        self.alignment = vec2f(1., 0.);
        self
    }

    /// Aligns the child to the top, centered horizontally.
    pub fn top(mut self) -> Self {
        self.alignment = vec2f(0., -1.);
        self
    }

    /// Aligns the child to the bottom, centered horizontally.
    pub fn bottom(mut self) -> Self {
        self.alignment = vec2f(0., 1.);
        self
    }

    /// Aligns the child to the top-left corner.
    pub fn top_left(mut self) -> Self {
        self.alignment = vec2f(-1., -1.);
        self
    }

    /// Aligns the child to the top-right corner.
    pub fn top_right(mut self) -> Self {
        self.alignment = vec2f(1., -1.);
        self
    }

    /// Aligns the child to the bottom-left corner.
    pub fn bottom_left(mut self) -> Self {
        self.alignment = vec2f(-1., 1.);
        self
    }

    /// Aligns the child to the bottom-right corner.
    pub fn bottom_right(mut self) -> Self {
        self.alignment = vec2f(1., 1.);
        self
    }
}

impl Element for Align {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let child_size = self.child.layout(
            SizeConstraint::new(Vector2F::zero(), constraint.max),
            ctx,
            app,
        );

        // Taking the maximum is the whole point; an unbounded axis has no
        // maximum to take, so fall back to the child on that axis alone.
        let mut size = constraint.max;
        if size.x().is_infinite() {
            size.set_x(child_size.x().max(constraint.min.x()));
        }
        if size.y().is_infinite() {
            size.set_y(child_size.y().max(constraint.min.y()));
        }

        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let size = self
            .size
            .expect("an align was painted before it was laid out");
        let child_size = self
            .child
            .size()
            .expect("an align's child was painted before it was laid out");

        let self_center = size / 2.;
        let self_target = self_center + self_center * self.alignment;
        let child_center = child_size / 2.;
        // A child larger than its parent would otherwise be pushed off the
        // leading edge; clamping keeps it flush with it instead.
        let child_target = (child_center + child_center * self.alignment).min(self_target);

        self.child
            .paint(origin - (child_target - self_target), ctx, app);
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
        self.child.origin()
    }
}
