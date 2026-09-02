//! The carrier that tells a [`Stack`](super::Stack) a child is anchored.

use std::any::Any;

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Point, Vector2F};
use crate::presenter::{EventContext, LayoutContext, PaintContext};

use super::AnchorTo;

/// Forwards everything to its child and reports an [`AnchorTo`] as parent
/// data.
///
/// That is the whole element. A [`Stack`](super::Stack) recognises an anchored
/// child by downcasting [`Element::parent_data`], so the anchor has to hang off
/// a *direct* child of the stack — which is why it gets a wrapper of its own
/// rather than living on the element being positioned.
pub(super) struct Positioned {
    child: Box<dyn Element>,
    anchor: AnchorTo,
}

impl Positioned {
    /// Marks `child` as anchored rather than stacked.
    pub(super) fn new(child: Box<dyn Element>, anchor: AnchorTo) -> Self {
        Self { child, anchor }
    }
}

impl Element for Positioned {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
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

    fn parent_data(&self) -> Option<&dyn Any> {
        Some(&self.anchor)
    }
}
