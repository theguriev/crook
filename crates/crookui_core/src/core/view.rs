//! Views: the things that render.

use std::any::Any;

use crate::core::{Action, AppContext, Entity, EntityId, ViewContext, WindowId};
use crate::element::Element;

/// Why [`View::on_focus`] is being called.
pub enum FocusContext {
    /// This view itself gained focus.
    SelfFocused,
    /// A descendant gained focus. Carries which one.
    DescendentFocused(EntityId),
}

impl FocusContext {
    /// Whether this view itself gained focus.
    pub fn is_self_focused(&self) -> bool {
        matches!(self, Self::SelfFocused)
    }
}

/// Why [`View::on_blur`] is being called.
pub enum BlurContext {
    /// This view itself lost focus.
    SelfBlurred,
    /// A descendant lost focus. Carries which one.
    DescendentBlurred(EntityId),
}

impl BlurContext {
    /// Whether this view itself lost focus.
    pub fn is_self_blurred(&self) -> bool {
        matches!(self, Self::SelfBlurred)
    }
}

/// An interactive, renderable piece of UI.
///
/// A `View` is roughly a React component: instance state plus a `render` that
/// turns it into a tree of primitives. Two properties follow from that and are
/// worth stating outright.
///
/// First, `render` takes `&self`. It cannot mutate, and it produces a brand new
/// [`Element`] tree every time — there is no diffing, no keys and no
/// reconciliation. Any interaction state an element seems to have (hover,
/// pressed) actually lives in the view and is handed down.
///
/// Second, nothing re-renders on its own. A view is re-rendered when, and only
/// when, someone calls [`ViewContext::notify`]. Mutating a field without
/// notifying leaves stale pixels on screen and reports no error.
///
/// # Example
///
/// ```
/// use crookui_core::prelude::*;
/// use crookui_core::elements::Empty;
///
/// struct Chip;
///
/// impl Entity for Chip {
///     type Event = ();
/// }
///
/// impl View for Chip {
///     fn ui_name() -> &'static str {
///         "Chip"
///     }
///
///     fn render(&self, _: &AppContext) -> Box<dyn Element> {
///         Empty::new().finish()
///     }
/// }
/// ```
pub trait View: Entity {
    /// A stable name for this view type, used in logs and diagnostics.
    fn ui_name() -> &'static str;

    /// Builds this view's element tree.
    fn render(&self, app: &AppContext) -> Box<dyn Element>;

    /// Called when this view or a descendant gains focus.
    fn on_focus(&mut self, _: &FocusContext, _: &mut ViewContext<Self>) {}

    /// Called when this view or a descendant loses focus.
    fn on_blur(&mut self, _: &BlurContext, _: &mut ViewContext<Self>) {}
}

/// A view that handles one type of action.
///
/// Give each view its *own* action enum. Dispatch walks the responder chain
/// from the leaf up and stops at the first view registered for the action's
/// type, so an action belonging to the workspace passes straight through every
/// tab view in between — which is exactly how an event bubbles here.
pub trait TypedActionView {
    /// The action type this view handles.
    type Action: Action;

    /// Handles an action dispatched from this view or a descendant.
    fn handle_action(&mut self, _: &Self::Action, _: &mut ViewContext<Self>) {}
}

/// The object-safe erasure of [`View`], as stored in a window.
///
/// The blanket impl below is the only place a `&mut AppContext` plus a pair of
/// ids is turned back into a typed `ViewContext<T>`, which is why user code
/// never handles `dyn Any` itself.
pub trait AnyView {
    /// This view as `&dyn Any`, for downcasting on the read path.
    fn as_any(&self) -> &dyn Any;

    /// This view as `&mut dyn Any`, for downcasting on the update path.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// The view type's name.
    fn ui_name(&self) -> &'static str;

    /// Builds the view's element tree.
    fn render(&self, app: &AppContext) -> Box<dyn Element>;

    /// Delivers a focus notification, rebuilding the typed context first.
    fn on_focus(
        &mut self,
        focus_ctx: &FocusContext,
        window_id: WindowId,
        view_id: EntityId,
        app: &mut AppContext,
    );

    /// Delivers a blur notification, rebuilding the typed context first.
    fn on_blur(
        &mut self,
        blur_ctx: &BlurContext,
        window_id: WindowId,
        view_id: EntityId,
        app: &mut AppContext,
    );
}

impl<T: View> AnyView for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn ui_name(&self) -> &'static str {
        T::ui_name()
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        View::render(self, app)
    }

    fn on_focus(
        &mut self,
        focus_ctx: &FocusContext,
        window_id: WindowId,
        view_id: EntityId,
        app: &mut AppContext,
    ) {
        let mut ctx = ViewContext::new(app, window_id, view_id);
        View::on_focus(self, focus_ctx, &mut ctx);
    }

    fn on_blur(
        &mut self,
        blur_ctx: &BlurContext,
        window_id: WindowId,
        view_id: EntityId,
        app: &mut AppContext,
    ) {
        let mut ctx = ViewContext::new(app, window_id, view_id);
        View::on_blur(self, blur_ctx, &mut ctx);
    }
}
