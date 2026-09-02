//! Confining a subtree to the box it was given.

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Point, RectF, Vector2F};
use crate::presenter::{EventContext, LayoutContext, PaintContext};
use crate::scene::ClipBounds;

/// Paints its child into a clipped layer of its own.
///
/// The box protocol lets a child paint outside the space it was allocated: a
/// [`Flex`](super::Flex) whose children need more room than it has lays them
/// out at their own sizes and paints them past its edge. Nothing about that is
/// visual only — [`Hoverable`](super::Hoverable) hit-tests against the
/// geometry it *painted*, so an overflowing button is clickable over whatever
/// it spilled onto.
///
/// Wrapping the subtree here fixes both halves at once. The scene layer this
/// starts carries a scissor rect the renderer honours, and the same bounds
/// narrow [`EventContext::visible_rect`], which is what every hit test is
/// resolved against. A control that spilled out of its parent is therefore
/// neither drawn nor clickable outside it.
pub struct Clipped {
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Clipped {
    /// Wraps `child`, clipping it to whatever size layout settles on.
    pub fn new(child: Box<dyn Element>) -> Self {
        Self {
            child,
            size: None,
            origin: None,
        }
    }
}

impl Element for Clipped {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        // Clipped to the space this element was *given*, not to the space its
        // child asked for: a child that overflows would otherwise widen the
        // very box it is supposed to be confined to.
        let size = self.child.layout(constraint, ctx, app).min(constraint.max);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let size = self
            .size
            .expect("a clip was painted before it was laid out");

        // Intersected with the enclosing clip rather than replacing it, so
        // nesting one of these inside another narrows the region instead of
        // widening it back out.
        ctx.scene
            .start_layer(ClipBounds::BoundedByActiveLayerAnd(RectF::new(
                origin, size,
            )));

        // Recorded inside the new layer: this element's content lives there,
        // so that is the layer its bounds must be resolved against.
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);

        ctx.scene.stop_layer();
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
