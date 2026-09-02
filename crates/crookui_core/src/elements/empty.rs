//! Nothing, sized.

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Point, Vector2F, vec2f};
use crate::presenter::{EventContext, LayoutContext, PaintContext};

/// Draws nothing and takes all the room it is offered.
///
/// Its use is as a flex spacer: `Expanded::new(1., Empty::new().finish())`
/// between two groups pushes them to opposite ends.
#[derive(Default)]
pub struct Empty {
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Empty {
    /// A new empty element.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Element for Empty {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        // An unbounded axis has no maximum to fill, and returning infinity
        // would poison every size computed from it, so fall back to the
        // minimum there.
        let size = vec2f(
            if constraint.max.x().is_infinite() {
                constraint.min.x()
            } else {
                constraint.max.x()
            },
            if constraint.max.y().is_infinite() {
                constraint.min.y()
            } else {
                constraint.max.y()
            },
        );

        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
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
