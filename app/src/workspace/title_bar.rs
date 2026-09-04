//! What makes the header a title bar rather than a row that happens to be at
//! the top: something to pick the window up by.
//!
//! Crook opens a client-decorated window
//! ([`WINDOW_CHROME`](crate::WINDOW_CHROME)), which means the window has no
//! title bar of its own and has to be moved by the application. [`draggable`]
//! wraps a row and turns a press on its *empty space* into a window drag, and
//! a double click there into maximise — the gesture every desktop gives a
//! title bar.
//!
//! "Empty" is not a list of rectangles kept in step with the header's layout.
//! It is whatever the row's own children did not claim: the children see every
//! event first, and a press that a tab or the usage chip handled never reaches
//! the drag. Add a control to the header and it stops being draggable there on
//! the same frame, with nothing to remember.
//!
//! # There are no caption buttons
//!
//! Crook draws no minimise, maximise or close controls of its own on any
//! platform. macOS's traffic lights are AppKit's and are painted over this
//! header — [`platform_insets`](crate::platform_insets) reserves the corner
//! they land in — and on Windows and Linux the window is closed, minimised and
//! maximised the way the desktop closes, minimises and maximises any other
//! window: its own shortcuts, its own gestures, and the commands the window
//! plugin registers. A row that also drew three buttons was a second title bar
//! inside the one the desktop already provides.

use crookui_core::event::{DispatchedEvent, Event, MouseButton};
use crookui_core::geometry::{Point, ZIndex};
use crookui_core::prelude::*;
use crookui_core::presenter::{LayoutContext, PaintContext};

use crate::platform_insets::WindowChrome;

use super::action::{WindowAction, WorkspaceAction};
use super::view::Workspace;

/// Picks the window up by `row`'s empty space.
///
/// `row` unchanged under native chrome: there the window manager draws a title
/// bar of its own and moves the window by it, and a header that also moved it
/// would be a second title bar with the tabs inside it.
pub(super) fn draggable(workspace: &Workspace, row: Box<dyn Element>) -> Box<dyn Element> {
    if matches!(workspace.window_chrome(), WindowChrome::Native) {
        return row;
    }

    DragToMove {
        child: row,
        origin: None,
        child_max_z_index: None,
    }
    .finish()
}

/// A row whose empty space moves the window.
///
/// Not a [`Hoverable`]: this needs the press itself, and it needs it only when
/// nothing else wanted it. `Hoverable` answers a *click*, which is a press and
/// a release in the same place — by which time a window drag has already had
/// to start — and it claims a press whether or not its child took it first.
struct DragToMove {
    child: Box<dyn Element>,
    origin: Option<Point>,
    /// The topmost layer the child painted into, so a press on something drawn
    /// *over* the header — the options menu, a tooltip — is not a press on the
    /// header.
    child_max_z_index: Option<ZIndex>,
}

impl Element for DragToMove {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.child.layout(constraint, ctx, app)
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
        self.child_max_z_index = Some(ctx.scene.max_active_z_index());
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        // The children first, and their answer is what "empty space" means.
        if self.child.dispatch_event(event, ctx, app) {
            return true;
        }

        let (Some(origin), Some(size), Some(z_index)) =
            (self.origin, self.child.size(), self.child_max_z_index)
        else {
            // Before the first paint there is nothing to pick up.
            return false;
        };

        let Some(Event::MouseDown {
            button: MouseButton::Left,
            position,
            click_count,
            ..
        }) = event.at_z_index(z_index, ctx)
        else {
            return false;
        };

        if !ctx
            .visible_rect(origin, size)
            .is_some_and(|visible| visible.contains_point(*position))
        {
            return false;
        }

        // The second press of a series and only the second. `click_count` goes
        // on counting for as long as the presses stay inside half a second and
        // four pixels of each other, so `>= 2` made every press after a double
        // click another maximise: reach straight for the title bar to move the
        // window you have just maximised and it would restore and stay put,
        // because no drag was ever started.
        ctx.dispatch_typed_action(WorkspaceAction::Window(if *click_count == 2 {
            WindowAction::ToggleMaximized
        } else {
            WindowAction::Drag
        }));
        true
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
