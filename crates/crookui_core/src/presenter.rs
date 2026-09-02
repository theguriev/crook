//! The frame driver: renders dirty views, lays out, paints, dispatches events.
//!
//! One [`Presenter`] per window. It owns one element tree per view — not one
//! per frame — and replaces exactly the trees whose views were invalidated,
//! leaving the rest of last frame's tree in place. Layout and paint then walk
//! all of them.
//!
//! The presenter is also where the view hierarchy is *discovered*. Nothing
//! declares that view B is inside view A; laying out a
//! [`ChildView`](crate::elements::ChildView) is what proves it, and the batch
//! of relationships found during layout is reported back to the app, which
//! walks it for responder chains and focus propagation.

use std::rc::Rc;
use std::sync::Arc;

use crate::core::{
    Action, AppContext, EntityId, EntityIdMap, EntityIdSet, WindowId, WindowInvalidation,
};
use crate::element::{Element, SizeConstraint};
use crate::event::{DispatchedEvent, Event};
use crate::geometry::{Point, RectF, Vector2F};
use crate::platform::TextLayoutSystem;
use crate::scene::Scene;

/// Renders one window.
pub struct Presenter {
    window_id: WindowId,
    scene: Option<Rc<Scene>>,
    rendered_views: EntityIdMap<Box<dyn Element>>,
    text_layout: Arc<dyn TextLayoutSystem>,
}

impl Presenter {
    /// A presenter for `window_id` that shapes text with `text_layout`.
    pub fn new(window_id: WindowId, text_layout: Arc<dyn TextLayoutSystem>) -> Self {
        Self {
            window_id,
            scene: None,
            rendered_views: EntityIdMap::default(),
            text_layout,
        }
    }

    /// The window this presenter draws.
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// The scene built by the last [`Self::build_scene`], if any.
    pub fn scene(&self) -> Option<&Rc<Scene>> {
        self.scene.as_ref()
    }

    /// Re-renders the views that changed and forgets the ones that are gone.
    pub fn invalidate(&mut self, invalidation: WindowInvalidation, app: &AppContext) {
        // A view can be both updated and removed in one batch — it notified and
        // was then dropped. Removal wins; rendering it would resurrect a tree
        // for a view that no longer exists.
        for view_id in invalidation.updated.difference(&invalidation.removed) {
            match app.render_view(self.window_id, *view_id) {
                Some(element) => {
                    self.rendered_views.insert(*view_id, element);
                }
                None => log::warn!("view {view_id} could not be rendered and was skipped"),
            }
        }

        for view_id in invalidation.removed {
            self.rendered_views.remove(&view_id);
        }
    }

    /// Lays out and paints the window, producing the frame to hand the GPU.
    ///
    /// The root is laid out against `0..window_size` and painted at the origin;
    /// every other position in the frame follows from that.
    pub fn build_scene(
        &mut self,
        window_size: Vector2F,
        scale_factor: f32,
        ctx: &mut AppContext,
    ) -> Rc<Scene> {
        let mut embeddings = EntityIdMap::default();
        if let Some(root_view_id) = ctx.root_view_id(self.window_id) {
            let mut layout_ctx = LayoutContext {
                rendered_views: &mut self.rendered_views,
                parents: &mut embeddings,
                view_stack: Vec::new(),
                text_layout: self.text_layout.as_ref(),
                window_size,
            };
            layout_ctx.layout(
                root_view_id,
                SizeConstraint::new(Vector2F::zero(), window_size),
                ctx,
            );
        }
        // Reported as a batch because the layout walk only has `&AppContext`.
        ctx.report_view_embeddings(self.window_id, embeddings);

        let mut scene = Scene::new(scale_factor);
        if let Some(root_view_id) = ctx.root_view_id(self.window_id) {
            let mut paint_ctx = PaintContext {
                rendered_views: &mut self.rendered_views,
                scene: &mut scene,
                window_size,
            };
            paint_ctx.paint(root_view_id, Vector2F::zero(), ctx);
        }

        let scene = Rc::new(scene);
        self.scene = Some(scene.clone());
        scene
    }

    /// Walks the element tree with an event, collecting what it asked for.
    ///
    /// Nothing is applied here: an element only has `&AppContext` while it
    /// handles an event, so the actions and notifications it produced are
    /// returned for the caller to apply against `&mut AppContext`. See
    /// [`AppContext::dispatch_window_event`].
    pub fn dispatch_event(&mut self, event: Event, app: &AppContext) -> DispatchResult {
        let mut event_ctx = EventContext {
            rendered_views: &mut self.rendered_views,
            scene: self.scene.clone(),
            view_stack: Vec::new(),
            actions: Vec::new(),
            notified: EntityIdSet::default(),
        };

        let handled = app
            .root_view_id(self.window_id)
            .is_some_and(|root_view_id| {
                event_ctx.dispatch_event_on_view(root_view_id, &DispatchedEvent::from(event), app)
            });

        DispatchResult {
            handled,
            actions: event_ctx.actions,
            notified: event_ctx.notified,
        }
    }
}

/// What an event dispatch asked the app to do.
#[derive(Default)]
pub struct DispatchResult {
    /// Whether any element claimed the event.
    pub handled: bool,
    /// Actions to dispatch, each tagged with the view it came from.
    pub actions: Vec<DispatchedAction>,
    /// Views that asked to be re-rendered.
    pub notified: EntityIdSet,
}

/// An action an element fired, and the view whose tree it was fired from.
pub struct DispatchedAction {
    /// The origin of the responder chain this action should walk.
    pub view_id: EntityId,
    /// The action itself.
    pub action: Box<dyn Action>,
}

/// What an element is given during layout.
pub struct LayoutContext<'a> {
    rendered_views: &'a mut EntityIdMap<Box<dyn Element>>,
    parents: &'a mut EntityIdMap<EntityId>,
    view_stack: Vec<EntityId>,

    /// The shaper, for elements that measure text.
    pub text_layout: &'a dyn TextLayoutSystem,

    /// The window's size in logical pixels, for elements that size themselves
    /// against the viewport rather than their parent.
    pub window_size: Vector2F,
}

impl LayoutContext<'_> {
    /// Lays out another view's element tree, recording where it was embedded.
    pub fn layout(
        &mut self,
        view_id: EntityId,
        constraint: SizeConstraint,
        app: &AppContext,
    ) -> Vector2F {
        // A view with no tree is one whose render failed or that was dropped
        // between invalidation and layout. It takes up no space.
        let Some(mut rendered_view) = self.rendered_views.remove(&view_id) else {
            return Vector2F::zero();
        };

        if let Some(parent_id) = self.view_stack.last() {
            self.parents.insert(view_id, *parent_id);
        }

        self.view_stack.push(view_id);
        let size = rendered_view.layout(constraint, self, app);
        self.view_stack.pop();

        self.rendered_views.insert(view_id, rendered_view);
        size
    }
}

/// What an element is given during paint.
pub struct PaintContext<'a> {
    rendered_views: &'a mut EntityIdMap<Box<dyn Element>>,

    /// The draw list being built.
    pub scene: &'a mut Scene,

    /// The window's size in logical pixels.
    pub window_size: Vector2F,
}

impl PaintContext<'_> {
    /// Paints another view's element tree at `origin`.
    pub fn paint(&mut self, view_id: EntityId, origin: Vector2F, app: &AppContext) {
        if let Some(mut rendered_view) = self.rendered_views.remove(&view_id) {
            rendered_view.paint(origin, self, app);
            self.rendered_views.insert(view_id, rendered_view);
        }
    }
}

/// What an element is given while handling an event.
pub struct EventContext<'a> {
    rendered_views: &'a mut EntityIdMap<Box<dyn Element>>,
    scene: Option<Rc<Scene>>,
    view_stack: Vec<EntityId>,
    actions: Vec<DispatchedAction>,
    notified: EntityIdSet,
}

impl EventContext<'_> {
    /// Hands the event to another view's element tree.
    pub fn dispatch_event_on_view(
        &mut self,
        view_id: EntityId,
        event: &DispatchedEvent,
        app: &AppContext,
    ) -> bool {
        let Some(mut rendered_view) = self.rendered_views.remove(&view_id) else {
            return false;
        };

        self.view_stack.push(view_id);
        let handled = rendered_view.dispatch_event(event, self, app);
        self.view_stack.pop();

        self.rendered_views.insert(view_id, rendered_view);
        handled
    }

    /// Fires an action from the view whose tree is currently being walked.
    pub fn dispatch_typed_action<A: Action>(&mut self, action: A) {
        self.actions.push(DispatchedAction {
            view_id: self.current_view_id(),
            action: Box::new(action),
        });
    }

    /// Marks the view whose tree is currently being walked as dirty.
    pub fn notify(&mut self) {
        let view_id = self.current_view_id();
        self.notified.insert(view_id);
    }

    /// Whether `position` is covered by something painted above it.
    pub fn is_covered(&self, position: Point) -> bool {
        self.scene
            .as_ref()
            .is_some_and(|scene| scene.is_covered(position))
    }

    /// The visible part of a rect, given the clip of the layer it painted into.
    pub fn visible_rect(&self, origin: Point, size: Vector2F) -> Option<RectF> {
        self.scene
            .as_ref()
            .and_then(|scene| scene.visible_rect(origin, size))
    }

    fn current_view_id(&self) -> EntityId {
        *self
            .view_stack
            .last()
            .expect("an element can only act while its own view is being walked")
    }
}

impl AppContext {
    /// Dispatches a window event through `presenter` and applies what it asked
    /// for.
    ///
    /// This is the whole of the input path: the element tree decides what an
    /// event means while it holds `&AppContext`, and the resulting actions and
    /// invalidations are applied here, where `&mut AppContext` is available.
    pub fn dispatch_window_event(
        &mut self,
        window_id: WindowId,
        event: Event,
        presenter: &mut Presenter,
    ) -> bool {
        let result = presenter.dispatch_event(event, self);

        for view_id in result.notified {
            self.notify_view_observers(window_id, view_id);
        }

        for dispatched in result.actions {
            self.dispatch_typed_action_for_view(
                window_id,
                dispatched.view_id,
                dispatched.action.as_ref(),
            );
        }

        result.handled
    }
}
