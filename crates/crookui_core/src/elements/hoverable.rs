//! Hover and click, with state that outlives the element tree.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::{DispatchedEvent, Event, MouseButton};
use crate::geometry::{Point, Vector2F, ZIndex};
use crate::presenter::{EventContext, LayoutContext, PaintContext};

/// What the mouse is doing to one element.
///
/// This lives in the *view*, not in the element: the element tree is thrown
/// away and rebuilt on every re-render, so anything stored in it is lost the
/// moment hovering causes a redraw. The view keeps a
/// [`MouseStateHandle`] per interactive region and hands it back to
/// [`Hoverable::new`] each time it renders.
#[derive(Clone, Debug, Default)]
pub struct MouseState {
    click_count: Option<u32>,
    is_hovered: bool,
    last_hover_change_was_synthetic: bool,
}

impl MouseState {
    /// Whether the pointer is over the element.
    pub fn is_hovered(&self) -> bool {
        self.is_hovered
    }

    /// Whether the element is being pressed right now.
    pub fn is_clicked(&self) -> bool {
        self.click_count.is_some()
    }

    /// How many clicks the press in progress is part of.
    pub fn click_count(&self) -> Option<u32> {
        self.click_count
    }

    /// Forgets the press and the hover.
    ///
    /// Call this when a click takes the element away — closing the tab that
    /// was clicked, say. Without it the replacement element inherits a hover
    /// it never received, because the synthetic move that follows a relayout
    /// would arrive at an element that already believes it is hovered.
    pub fn reset_interaction_state(&mut self) {
        self.click_count = None;
        self.is_hovered = false;
        self.last_hover_change_was_synthetic = true;
    }
}

/// Shared ownership of a [`MouseState`], held by a view across renders.
pub type MouseStateHandle = Arc<Mutex<MouseState>>;

type ClickHandler = Box<dyn FnMut(Vector2F, &mut EventContext, &AppContext)>;
type HoverHandler = Box<dyn FnMut(bool, Vector2F, &mut EventContext, &AppContext)>;

/// Makes a subtree respond to the mouse.
pub struct Hoverable {
    child: Box<dyn Element>,
    state: MouseStateHandle,
    origin: Option<Point>,
    child_max_z_index: Option<ZIndex>,
    hover_handler: Option<HoverHandler>,
    click_handler: Option<ClickHandler>,
    middle_click_handler: Option<ClickHandler>,
}

impl Hoverable {
    /// Wraps the element `build_child` produces from the current mouse state.
    ///
    /// The builder is handed the state so the child can be styled by it — a
    /// hovered tab is a different `Container` than an unhovered one, built
    /// right here.
    pub fn new<F>(state: MouseStateHandle, build_child: F) -> Self
    where
        F: FnOnce(&MouseState) -> Box<dyn Element>,
    {
        let child = build_child(&state.lock());
        Self {
            child,
            state,
            origin: None,
            child_max_z_index: None,
            hover_handler: None,
            click_handler: None,
            middle_click_handler: None,
        }
    }

    /// Runs `handler` on a left press and release that both land inside.
    pub fn on_click<F>(mut self, handler: F) -> Self
    where
        F: 'static + FnMut(Vector2F, &mut EventContext, &AppContext),
    {
        self.click_handler = Some(Box::new(handler));
        self
    }

    /// Runs `handler` on a middle press, which conventionally closes a tab.
    pub fn on_middle_click<F>(mut self, handler: F) -> Self
    where
        F: 'static + FnMut(Vector2F, &mut EventContext, &AppContext),
    {
        self.middle_click_handler = Some(Box::new(handler));
        self
    }

    /// Runs `handler` when the pointer enters or leaves.
    pub fn on_hover<F>(mut self, handler: F) -> Self
    where
        F: 'static + FnMut(bool, Vector2F, &mut EventContext, &AppContext),
    {
        self.hover_handler = Some(Box::new(handler));
        self
    }

    /// Whether `position` is over this element and not covered by anything.
    fn is_mouse_over(&self, position: Vector2F, ctx: &EventContext) -> bool {
        let (Some(origin), Some(size), Some(z_index)) =
            (self.origin, self.size(), self.child_max_z_index)
        else {
            // Before the first paint there is nothing to hit.
            return false;
        };

        ctx.visible_rect(origin, size)
            .is_some_and(|visible| visible.contains_point(position))
            && !ctx.is_covered(Point::from_vec2f(position, z_index))
    }

    fn handle_mouse_moved(
        &mut self,
        position: Vector2F,
        is_synthetic: bool,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let is_hovered = self.is_mouse_over(position, ctx);

        let was_synthetic = {
            let mut state = self.state.lock();
            if state.is_hovered == is_hovered {
                return false;
            }
            state.is_hovered = is_hovered;
            std::mem::replace(&mut state.last_hover_change_was_synthetic, is_synthetic)
        };

        // Two synthetic hover changes in a row mean the redraw caused by the
        // first produced the second: a child whose size depends on hover moved
        // out from under the pointer. Breaking the chain here is what keeps
        // that from looping forever.
        if was_synthetic && is_synthetic {
            log::warn!("ignoring a synthetic hover change that followed another one");
            return false;
        }

        if let Some(handler) = self.hover_handler.as_mut() {
            handler(is_hovered, position, ctx, app);
        }

        ctx.notify();
        true
    }
}

impl Element for Hoverable {
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

        // Hit testing uses the topmost layer the child reached, not the layer
        // this element painted into, so a child drawn above still counts as
        // part of the region rather than as something covering it.
        self.child_max_z_index = Some(ctx.scene.max_active_z_index());
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let handled = self.child.dispatch_event(event, ctx, app);
        if self.origin.is_none() {
            return handled;
        }

        if !matches!(event.raw_event(), Event::MouseMoved { .. }) {
            self.state.lock().last_hover_change_was_synthetic = false;
        }

        // A press somewhere else means whatever this element was tracking is
        // over: the release will not be coming here.
        if let Some(position) = event.raw_event().mouse_down_position()
            && !self.is_mouse_over(position, ctx)
        {
            let mut state = self.state.lock();
            state.click_count = None;
            state.is_hovered = false;
            return handled;
        }

        match event.raw_event() {
            Event::MouseDown {
                button: MouseButton::Middle,
                position,
                ..
            } => {
                if let Some(handler) = self.middle_click_handler.as_mut() {
                    handler(*position, ctx, app);
                    ctx.notify();
                    return true;
                }
            }

            Event::MouseDown {
                button: MouseButton::Left,
                click_count,
                ..
            } => {
                self.state.lock().click_count = Some(*click_count);
                // Claiming the press is what makes the release meaningful: a
                // click is the pair, and only this element is tracking it.
                if self.click_handler.is_some() {
                    ctx.notify();
                    return true;
                }
            }

            Event::MouseUp {
                button: MouseButton::Left,
                position,
                ..
            } => {
                let was_pressed = self.state.lock().click_count.take().is_some();
                if !self.is_mouse_over(*position, ctx) {
                    return handled;
                }

                if was_pressed && let Some(handler) = self.click_handler.as_mut() {
                    handler(*position, ctx, app);
                    ctx.notify();
                    return true;
                }
            }

            Event::MouseMoved {
                position,
                is_synthetic,
                ..
            } => {
                if self.handle_mouse_moved(*position, *is_synthetic, ctx, app) {
                    return true;
                }
            }

            _ => {}
        }

        handled
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
