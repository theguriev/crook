//! A box no bigger than the room the window leaves.
//!
//! A popup is built to a width — a card to 320, a menu to 200 — and to
//! whatever height its rows add up to, and both are decided before anything
//! knows how big the window is. The layout knows: [`LayoutContext`] carries
//! the window's size for exactly the element that sizes itself against the
//! viewport rather than its parent. This is that element, for an overlay.
//!
//! It takes the room off the *constraint* and lets the child do what it does
//! with less: a card's texts are cut with a mark where they run out, and a
//! menu's rows scroll behind a [`Scrollable`](crookui_core::elements::Scrollable)
//! the menu puts inside. What it never does is grow the child — an overlay
//! that took the whole window would be a page, not a popup.

use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::DispatchedEvent;
use crookui_core::geometry::{Point, Vector2F};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};

/// Narrows or shortens its child to the room the window leaves.
pub(crate) struct WindowRoom {
    child: Box<dyn Element>,
    /// What is taken off the window's width before the child gets the rest,
    /// and the least it may be narrowed to.
    width: Option<(f32, f32)>,
    /// The inset kept above and below the child, and the least it may be
    /// shortened to: the window's height less twice the inset is what it gets.
    height: Option<(f32, f32)>,
}

impl WindowRoom {
    /// Wraps `child`, taking nothing off it until asked.
    pub(crate) fn new(child: Box<dyn Element>) -> Self {
        Self {
            child,
            width: None,
            height: None,
        }
    }

    /// Narrows the child to the room right of `from_left`, never under
    /// `least`.
    ///
    /// The least is where narrowing stops helping: a card cut to nothing
    /// says nothing a row does not, and the clip off the window's edge is
    /// then the lesser evil.
    pub(crate) fn with_width_right_of(mut self, from_left: f32, least: f32) -> Self {
        self.width = Some((from_left, least));
        self
    }

    /// Narrows the child to the window's width less `inset` at each side,
    /// never under `least`.
    ///
    /// For a surface centred on the window: a card built to one width in a
    /// window narrower than it used to run off the right edge, since the row
    /// that centres it measures it free.
    pub(crate) fn with_width_inset(mut self, inset: f32, least: f32) -> Self {
        self.width = Some((inset * 2., least));
        self
    }

    /// Shortens the child to the window's height less `inset` at each end,
    /// never under `least`.
    ///
    /// For a menu whose rows scroll: a window shorter than the menu used to
    /// leave its last rows past the bottom edge, unreachable, since the
    /// anchor slides a popup on screen but cannot make it fit.
    pub(crate) fn with_height_inset(mut self, inset: f32, least: f32) -> Self {
        self.height = Some((inset, least));
        self
    }
}

impl Element for WindowRoom {
    fn layout(
        &mut self,
        mut constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        if let Some((taken, least)) = self.width {
            let room = (ctx.window_size.x() - taken).max(least);
            constraint.max.set_x(constraint.max.x().min(room));
            constraint.min.set_x(constraint.min.x().min(room));
        }
        if let Some((inset, least)) = self.height {
            let room = (ctx.window_size.y() - inset * 2.).max(least);
            constraint.max.set_y(constraint.max.y().min(room));
            constraint.min.set_y(constraint.min.y().min(room));
        }
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
