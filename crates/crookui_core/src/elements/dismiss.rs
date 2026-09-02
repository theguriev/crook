//! Closing a popup by clicking anywhere else.

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::{DispatchedEvent, Event, MouseButton};
use crate::geometry::{Point, RectF, Vector2F};
use crate::presenter::{EventContext, LayoutContext, PaintContext};
use crate::scene::ClipBounds;

/// What to run when the popup should close.
type DismissHandler = Box<dyn FnMut(&mut EventContext, &AppContext)>;

/// Wraps a popup and reports the clicks that landed outside it.
///
/// "Outside" is never measured. The child is painted one layer above this
/// element, so a press over the popup is *covered* — from down here it does not
/// exist — while a press anywhere else arrives intact. That is the whole test,
/// and it stays right for a menu with a ragged edge, a gap between two rows, or
/// a submenu of its own, none of which a rectangle test would survive.
///
/// [`Self::modal`] additionally freezes the rest of the window while the popup
/// is up.
pub struct Dismiss {
    child: Box<dyn Element>,
    on_dismiss: Option<DismissHandler>,
    origin: Option<Point>,
    modal: bool,
}

impl Dismiss {
    /// Wraps `child`, which is the popup itself.
    pub fn new(child: Box<dyn Element>) -> Self {
        Self {
            child,
            on_dismiss: None,
            origin: None,
            modal: false,
        }
    }

    /// Runs `handler` when a press lands outside the popup.
    ///
    /// The handler is expected to take the popup down; nothing here does that
    /// on its own, because what "closing" means belongs to the view that
    /// decided to open it.
    pub fn on_dismiss<F>(mut self, handler: F) -> Self
    where
        F: 'static + FnMut(&mut EventContext, &AppContext),
    {
        self.on_dismiss = Some(Box::new(handler));
        self
    }

    /// Freezes the rest of the window while the popup is up: nothing painted
    /// below it hovers, scrolls or reacts to a click until it is dismissed.
    pub fn modal(mut self) -> Self {
        self.modal = true;
        self
    }
}

impl Element for Dismiss {
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

        if self.modal {
            // An invisible hit rect over the whole window, in *this* layer.
            // Everything painted below is covered by it, which is what stops
            // the frame underneath reacting to the click that dismisses.
            ctx.scene
                .draw_rect_with_hit_recording(RectF::new(Vector2F::zero(), ctx.window_size));
        }

        // The child goes one layer up either way — Warp skips this in the
        // modal case and pays for it with an event barrier wrapped around
        // every modal popup, because without the extra layer a press on the
        // popup's own padding is indistinguishable from a press on the window.
        ctx.scene.start_layer(ClipBounds::ActiveLayer);
        self.child.paint(origin, ctx, app);
        ctx.scene.stop_layer();
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        if self.child.dispatch_event(event, ctx, app) {
            return true;
        }

        let Some(z_index) = self.z_index() else {
            return false;
        };

        match (self.on_dismiss.as_mut(), event.at_z_index(z_index, ctx)) {
            // Reaching this element's own layer means the press was not
            // covered by the child painted above it: it landed outside.
            (
                Some(handler),
                Some(Event::MouseDown {
                    button: MouseButton::Left,
                    ..
                }),
            ) => {
                handler(ctx, app);
                self.modal
            }
            (
                None,
                Some(Event::MouseDown {
                    button: MouseButton::Left,
                    ..
                }),
            ) => {
                log::warn!("a dismiss underlay was clicked with no handler attached");
                self.modal
            }
            // A modal swallows every other positional event that got past the
            // popup, so the window beneath neither hovers nor scrolls. Keys
            // are left alone: they are not aimed at a place on screen.
            (
                _,
                Some(
                    Event::MouseDown { .. }
                    | Event::MouseUp { .. }
                    | Event::MouseDragged { .. }
                    | Event::MouseMoved { .. }
                    | Event::ScrollWheel { .. },
                ),
            ) => self.modal,
            _ => false,
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
