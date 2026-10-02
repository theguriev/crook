//! The panel's right edge, which is a handle: dragged, it makes the panel
//! wider or narrower, and double-clicked it puts the width back.
//!
//! The panel's own element rather than a divider between it and the work,
//! because a sibling would not be enough: every child of a row sees every
//! press, so a handle beside the panel would share its press with whatever
//! row was under it — and a row pressed and dragged is a tab being moved.
//! Wrapping the panel is what lets the handle keep the press from the rows.

use std::cell::Cell;
use std::rc::Rc;

use crookui_core::element::SizeConstraint;
use crookui_core::event::{DispatchedEvent, Event, MouseButton};
use crookui_core::geometry::{Point, Vector2F};
use crookui_core::prelude::*;
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};

use super::PANEL_WIDTH;
use crate::workspace::action::WorkspaceAction;

/// How far in from the panel's right edge a press takes the handle.
///
/// Inside the panel only: the pane to its right hit-tests its own bounds and
/// would start a selection under a handle that reached into it.
const GRAB: f32 = 6.;

/// The panel's width being dragged, shared between the frames of one drag.
///
/// One per window and kept by it, since the element that starts a drag is
/// thrown away with the frame that drew it.
#[derive(Clone, Default)]
pub(crate) struct PanelResize(Rc<Cell<Option<Resize>>>);

/// One drag of the edge.
#[derive(Copy, Clone, Debug)]
struct Resize {
    /// Where the pointer went down, across the window.
    anchor: f32,
    /// The width the panel had then.
    from: f32,
    /// The width the drag last asked for, which is what a release keeps.
    to: f32,
}

impl PanelResize {
    /// A window whose panel edge nobody is dragging.
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// The panel, with its right edge made a handle.
pub(super) struct PanelEdge {
    child: Box<dyn Element>,
    resize: PanelResize,
    width: f32,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl PanelEdge {
    /// `panel` drawn `width` wide, its edge dragged through `resize`.
    pub(super) fn new(panel: Box<dyn Element>, width: f32, resize: PanelResize) -> Self {
        Self {
            child: panel,
            resize,
            width,
            size: None,
            origin: None,
        }
    }

    /// Whether a press at `position` is on the handle.
    fn on_handle(&self, position: Vector2F) -> bool {
        self.bounds().is_some_and(|bounds| {
            bounds.contains_point(position) && position.x() >= bounds.max_x() - GRAB
        })
    }

    fn ask(width: f32, save: bool, ctx: &mut EventContext) {
        ctx.dispatch_typed_action(WorkspaceAction::ResizeTabsPanel { width, save });
    }
}

impl Element for PanelEdge {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        // The rest of a drag is not hit-tested, as a divider's is not: the
        // pointer has left the edge by the first pixel of it. Nor do the rows
        // hear it, since none of them was pressed.
        if let Some(resize) = self.resize.0.get() {
            match event.raw_event() {
                Event::MouseDragged {
                    button: MouseButton::Left,
                    position,
                    ..
                } => {
                    let to = resize.from + position.x() - resize.anchor;
                    self.resize.0.set(Some(Resize { to, ..resize }));
                    Self::ask(to, false, ctx);
                    return true;
                }
                Event::MouseUp {
                    button: MouseButton::Left,
                    ..
                } => {
                    self.resize.0.set(None);
                    Self::ask(resize.to, true, ctx);
                    return true;
                }
                _ => {}
            }
        }

        if let Some(z_index) = self.z_index()
            && let Some(Event::MouseDown {
                button: MouseButton::Left,
                position,
                click_count,
                ..
            }) = event.at_z_index(z_index, ctx)
            && self.on_handle(*position)
        {
            // A double click puts the width back, which is the only way to
            // the default once it has been dragged and what a split's
            // divider does with its own.
            if *click_count >= 2 {
                Self::ask(PANEL_WIDTH, true, ctx);
            } else {
                self.resize.0.set(Some(Resize {
                    anchor: position.x(),
                    from: self.width,
                    to: self.width,
                }));
            }
            return true;
        }

        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
