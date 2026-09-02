//! Fixed and bounded sizes.

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Point, Vector2F};
use crate::presenter::{EventContext, LayoutContext, PaintContext};

/// Narrows the constraint its child is laid out against.
///
/// Otherwise completely transparent: it reports its child's size and origin as
/// its own, and forwards every event. This is how a tab gets a fixed width and
/// the header a fixed height.
pub struct ConstrainedBox {
    child: Box<dyn Element>,
    constraint: SizeConstraint,
}

impl ConstrainedBox {
    /// Wraps `child`, constraining nothing yet.
    pub fn new(child: Box<dyn Element>) -> Self {
        Self {
            child,
            constraint: SizeConstraint::new(Vector2F::zero(), Vector2F::splat(f32::INFINITY)),
        }
    }

    /// Fixes the width.
    pub fn with_width(mut self, width: f32) -> Self {
        self.constraint.min.set_x(width);
        self.constraint.max.set_x(width);
        self
    }

    /// Fixes the height.
    pub fn with_height(mut self, height: f32) -> Self {
        self.constraint.min.set_y(height);
        self.constraint.max.set_y(height);
        self
    }

    /// Sets the widest the child may be.
    pub fn with_max_width(mut self, max_width: f32) -> Self {
        self.constraint.max.set_x(max_width);
        self
    }

    /// Sets the narrowest the child may be.
    pub fn with_min_width(mut self, min_width: f32) -> Self {
        self.constraint.min.set_x(min_width);
        self
    }

    /// Sets the tallest the child may be.
    pub fn with_max_height(mut self, max_height: f32) -> Self {
        self.constraint.max.set_y(max_height);
        self
    }

    /// Sets the shortest the child may be.
    pub fn with_min_height(mut self, min_height: f32) -> Self {
        self.constraint.min.set_y(min_height);
        self
    }
}

impl Element for ConstrainedBox {
    fn layout(
        &mut self,
        mut constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        constraint.min = constraint.min.max(self.constraint.min);
        constraint.max = constraint.max.min(self.constraint.max);
        // A parent's maximum wins over this box's minimum: asking for a width
        // of 200 inside a 100-wide parent yields 100, not an unsatisfiable
        // constraint.
        constraint.min = constraint.min.min(constraint.max);

        self.child.layout(constraint, ctx, app)
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app);
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
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }
}
