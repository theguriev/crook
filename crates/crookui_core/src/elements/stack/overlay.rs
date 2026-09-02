//! The layer jump behind every popup.

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Point, Vector2F};
use crate::presenter::{EventContext, LayoutContext, PaintContext};
use crate::scene::ClipBounds;

/// Paints its child into an overlay layer.
///
/// Everything else forwards: this element exists for the two lines in
/// [`Element::paint`]. Its child lands above every normal layer in the frame
/// and outside every ancestor's clip, which is what turns a subtree emitted
/// inside the tab strip into a menu floating over the whole window.
///
/// Reachable only through [`Stack`](super::Stack), because an overlay that
/// nothing positions would paint at whatever origin its parent happened to
/// hand it.
pub(super) struct Overlay {
    child: Box<dyn Element>,
}

impl Overlay {
    /// Wraps `child`, lifting it into an overlay layer of its own.
    pub(super) fn new(child: Box<dyn Element>) -> Self {
        Self { child }
    }
}

impl Element for Overlay {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.child.layout(constraint, ctx, app)
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        // Unclipped: a menu is free of the scissor rect of whatever panel
        // spawned it, exactly as it is free of that panel's depth.
        ctx.scene.start_overlay_layer(ClipBounds::None);
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
        self.child.size()
    }

    /// The child's, not one of its own: this element is a layer, not a box, so
    /// everything above it must see straight through to what it wraps.
    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }
}
