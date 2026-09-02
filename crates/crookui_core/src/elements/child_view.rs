//! Embedding one view's element tree inside another's.

use std::marker::PhantomData;

use crate::AppContext;
use crate::core::{EntityId, View, ViewHandle};
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Point, Vector2F};
use crate::presenter::{EventContext, LayoutContext, PaintContext};

/// The element that stands in for another view.
///
/// Every method here delegates to the presenter, which looks the view's own
/// element tree up by id. That indirection is what keeps views independent:
/// the parent never touches the child's tree, and the child re-renders on its
/// own schedule.
///
/// Laying one of these out is also how the app *learns* that one view is
/// inside another — see [`AppContext::view_ancestors`].
pub struct ChildView<T> {
    view_id: EntityId,
    size: Option<Vector2F>,
    origin: Option<Point>,
    view_type: PhantomData<T>,
}

impl<T: View> ChildView<T> {
    /// Embeds the view behind `handle`.
    pub fn new(handle: &ViewHandle<T>) -> Self {
        Self::with_id(handle.id())
    }

    /// Embeds a view by id, for callers that only kept the id.
    pub fn with_id(view_id: EntityId) -> Self {
        Self {
            view_id,
            size: None,
            origin: None,
            view_type: PhantomData,
        }
    }
}

impl<T> Element for ChildView<T> {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = ctx.layout(self.view_id, constraint, app);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        ctx.paint(self.view_id, origin, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        ctx.dispatch_event_on_view(self.view_id, event, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
