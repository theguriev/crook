//! [`App`], the one shared-mutability point, and [`AppContext`], everything it
//! guards.

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::rc::{self, Rc};
use std::sync::Arc;

use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::core::refcount::RefCounts;
use crate::core::{
    Action, ActionType, AddSingletonModel, AnyModel, AnyModelHandle, AnyView, BlurContext, Effect,
    Entity, EntityId, EntityIdMap, FocusContext, GetSingletonModelHandle, InvalidationCallback,
    ModelAsRef, ModelContext, ModelHandle, Observation, ReadModel, ReadView, SingletonEntity,
    Subscription, TypedActionCallback, TypedActionView, UpdateModel, UpdateView, View, ViewAsRef,
    ViewContext, ViewHandle, ViewType, Window, WindowId, WindowInvalidation,
};
use crate::element::Element;
use crate::executor::{Background, Foreground, LocalQueue};

/// The application.
///
/// A cheap clonable handle to the one `RefCell` in the design. Every method
/// here takes that borrow for the duration of one closure and drops it again,
/// which is why nothing below this line needs interior mutability.
#[derive(Clone)]
pub struct App(Rc<RefCell<AppContext>>);

/// A weak [`App`], for code that must not keep the application alive.
///
/// This is what a spawned task holds: it re-enters the app when it completes,
/// and finds nothing if the app shut down first.
#[derive(Clone)]
pub struct WeakApp(rc::Weak<RefCell<AppContext>>);

impl WeakApp {
    /// The app, if it still exists.
    pub fn upgrade(&self) -> Option<App> {
        self.0.upgrade().map(App)
    }
}

impl App {
    /// Builds an application around the executors it will run on.
    pub fn new(foreground: Rc<Foreground>, background: Arc<Background>) -> Self {
        let app = Self(Rc::new(RefCell::new(AppContext::new(
            foreground, background,
        ))));
        app.0.borrow_mut().weak_self = Rc::downgrade(&app.0);
        app
    }

    /// Runs `test` against a fresh app on the current thread.
    ///
    /// The returned future is driven by a local queue that also pumps anything
    /// the app spawns on the foreground, so a test can await spawned work
    /// without a platform event loop.
    pub fn test<T, Fut>(test: impl FnOnce(App) -> Fut) -> T
    where
        Fut: 'static + Future<Output = T>,
    {
        let queue = LocalQueue::new();
        let app = Self::new(queue.foreground(), Arc::new(Background::new(1)));
        queue.block_on(test(app))
    }

    /// A weak reference to this app.
    pub fn downgrade(&self) -> WeakApp {
        WeakApp(Rc::downgrade(&self.0))
    }

    /// Reads application state.
    pub fn read<T>(&self, read: impl FnOnce(&AppContext) -> T) -> T {
        read(&self.0.borrow())
    }

    /// Mutates application state, then flushes the effects it queued.
    pub fn update<T>(&mut self, update: impl FnOnce(&mut AppContext) -> T) -> T {
        let mut ctx = self.0.borrow_mut();
        ctx.pending_flushes += 1;
        let result = update(&mut ctx);
        ctx.flush_effects();
        result
    }

    /// Opens a window and builds its root view.
    pub fn add_window<V, F>(&mut self, build_root_view: F) -> (WindowId, ViewHandle<V>)
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.update(|ctx| ctx.add_window(build_root_view))
    }

    /// Creates a model.
    pub fn add_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: Entity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        self.update(|ctx| ctx.add_model(build_model))
    }

    /// Creates a view with no parent.
    pub fn add_view<V, F>(&mut self, window_id: WindowId, build_view: F) -> ViewHandle<V>
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.update(|ctx| ctx.add_view(window_id, build_view))
    }

    /// Dispatches an action along an explicit responder chain.
    pub fn dispatch_typed_action(
        &mut self,
        window_id: WindowId,
        responder_chain: &[EntityId],
        action: &dyn Action,
    ) -> bool {
        self.update(|ctx| ctx.dispatch_typed_action(window_id, responder_chain, action))
    }

    /// Which view holds focus in a window.
    pub fn focused_view_id(&self, window_id: WindowId) -> Option<EntityId> {
        self.read(|ctx| ctx.focused_view_id(window_id))
    }

    /// Registers the callback that redraws a window when its views change.
    pub fn on_window_invalidated<F>(&mut self, window_id: WindowId, callback: F)
    where
        F: 'static + FnMut(WindowId, &mut AppContext),
    {
        self.update(|ctx| ctx.on_window_invalidated(window_id, callback));
    }
}

/// Everything the application owns.
pub struct AppContext {
    pub(crate) models: EntityIdMap<Box<dyn AnyModel>>,
    pub(crate) windows: HashMap<WindowId, Window>,

    /// Which window each view lives in.
    ///
    /// Handles carry a copy of this, but a dropped handle is exactly when the
    /// copy is out of reach, so removal reads it from here.
    pub(crate) view_to_window: EntityIdMap<WindowId>,

    /// Child view to parent view, per window: the responder chain.
    ///
    /// Two things write here. Creating a view through a
    /// [`ViewContext`](crate::ViewContext) records parentage immediately, and
    /// laying out a [`ChildView`](crate::elements::ChildView) reports it after
    /// the fact. The second is what makes the chain follow what is actually on
    /// screen; the first is what gives a view that is never rendered an
    /// ancestor anyway.
    pub(crate) view_parents: HashMap<WindowId, EntityIdMap<EntityId>>,

    pub(crate) ref_counts: Arc<Mutex<RefCounts>>,
    pub(crate) subscriptions: EntityIdMap<Vec<Subscription>>,
    pub(crate) observations: EntityIdMap<Vec<Observation>>,

    /// One action handler per (action type, view type) pair — per *type*, not
    /// per instance, since the handler is the same closure for every instance.
    typed_actions: HashMap<ActionType, HashMap<ViewType, Box<TypedActionCallback>>>,

    window_invalidations: HashMap<WindowId, WindowInvalidation>,
    invalidation_callbacks: HashMap<WindowId, Box<InvalidationCallback>>,

    pub(crate) pending_effects: VecDeque<Effect>,
    pending_flushes: usize,
    flushing_effects: bool,

    singleton_models: FxHashMap<TypeId, AnyModelHandle>,

    weak_self: rc::Weak<RefCell<Self>>,
    pub(crate) foreground: Rc<Foreground>,
    background: Arc<Background>,
}

impl AppContext {
    fn new(foreground: Rc<Foreground>, background: Arc<Background>) -> Self {
        Self {
            models: EntityIdMap::default(),
            windows: HashMap::new(),
            view_to_window: EntityIdMap::default(),
            view_parents: HashMap::new(),
            ref_counts: Arc::new(Mutex::new(RefCounts::default())),
            subscriptions: EntityIdMap::default(),
            observations: EntityIdMap::default(),
            typed_actions: HashMap::new(),
            window_invalidations: HashMap::new(),
            invalidation_callbacks: HashMap::new(),
            pending_effects: VecDeque::new(),
            pending_flushes: 0,
            flushing_effects: false,
            singleton_models: FxHashMap::default(),
            weak_self: rc::Weak::new(),
            foreground,
            background,
        }
    }

    /// A weak reference to the owning [`App`].
    pub fn weak_app(&self) -> WeakApp {
        WeakApp(self.weak_self.clone())
    }

    /// The main-thread executor.
    pub fn foreground(&self) -> &Rc<Foreground> {
        &self.foreground
    }

    /// The worker pool for work that must not block a frame.
    pub fn background(&self) -> &Arc<Background> {
        &self.background
    }

    /// How many nested updates are in flight. Effects run when this hits zero.
    pub fn pending_flushes(&self) -> usize {
        self.pending_flushes
    }

    // ---------------------------------------------------------------- windows

    /// Opens a window and builds its root view.
    ///
    /// The window is registered *before* the root view is built, because that
    /// view's constructor may add child views and they need somewhere to live.
    pub fn add_window<V, F>(&mut self, build_root_view: F) -> (WindowId, ViewHandle<V>)
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        let window_id = WindowId::new();
        self.windows.insert(window_id, Window::default());

        self.pending_flushes += 1;
        let root_view = self.add_view(window_id, build_root_view);
        self.windows
            .get_mut(&window_id)
            .expect("just inserted")
            .root_view = Some((&root_view).into());
        self.focus(window_id, root_view.id());
        self.flush_effects();

        (window_id, root_view)
    }

    /// Whether a window is open.
    pub fn is_window_open(&self, window_id: WindowId) -> bool {
        self.windows.contains_key(&window_id)
    }

    /// The id of a window's root view.
    pub fn root_view_id(&self, window_id: WindowId) -> Option<EntityId> {
        self.windows
            .get(&window_id)
            .and_then(|window| window.root_view.as_ref())
            .map(|root_view| root_view.id())
    }

    /// A handle to a window's root view, if it is a `T`.
    pub fn root_view<T: Entity>(&self, window_id: WindowId) -> Option<ViewHandle<T>> {
        self.windows
            .get(&window_id)
            .and_then(|window| window.root_view.clone())
            .and_then(|root_view| root_view.downcast())
    }

    /// Which view holds focus in a window.
    pub fn focused_view_id(&self, window_id: WindowId) -> Option<EntityId> {
        self.windows
            .get(&window_id)
            .and_then(|window| window.focused_view)
    }

    pub(crate) fn stored_view(
        &self,
        window_id: WindowId,
        view_id: EntityId,
    ) -> Option<&dyn AnyView> {
        self.windows
            .get(&window_id)?
            .views
            .get(&view_id)
            .map(|view| &**view)
    }

    /// The view type's name, for logs and diagnostics.
    pub fn view_name(&self, window_id: WindowId, view_id: EntityId) -> Option<&'static str> {
        self.stored_view(window_id, view_id)
            .map(|view| view.ui_name())
    }

    /// Renders one view's element tree.
    ///
    /// Returns `None` when the view is gone or is currently checked out — both
    /// of which happen legitimately, one frame after a view is dropped.
    pub(crate) fn render_view(
        &self,
        window_id: WindowId,
        view_id: EntityId,
    ) -> Option<Box<dyn Element>> {
        Some(self.stored_view(window_id, view_id)?.render(self))
    }

    // ----------------------------------------------------------------- models

    /// Creates a model.
    pub fn add_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: Entity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        self.pending_flushes += 1;
        let model_id = EntityId::new();
        let mut ctx = ModelContext::new(self, model_id);
        let model = build_model(&mut ctx);
        self.models.insert(model_id, Box::new(model));
        self.flush_effects();
        ModelHandle::new(model_id, &self.ref_counts)
    }

    /// Whether the singleton model of type `T` has been registered.
    pub fn has_singleton_model<T: SingletonEntity>(&self) -> bool {
        self.singleton_models.contains_key(&TypeId::of::<T>())
    }

    pub(crate) fn get_singleton_model_as_ref<T: SingletonEntity>(&self) -> &T {
        self.singleton_models
            .get(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("no singleton model of type {}", std::any::type_name::<T>()))
            .downcast_ref(self)
            .expect("the singleton table is keyed by the model's own type")
    }

    // ------------------------------------------------------------------ views

    /// Creates a view with no parent.
    ///
    /// Its responder chain is itself until a layout pass discovers where it was
    /// embedded, so prefer [`ViewContext::add_view`], which records parentage
    /// at creation time.
    pub fn add_view<V, F>(&mut self, window_id: WindowId, build_view: F) -> ViewHandle<V>
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.add_view_internal(window_id, None, build_view)
    }

    pub(crate) fn add_view_with_parent<V, F>(
        &mut self,
        window_id: WindowId,
        parent_view_id: EntityId,
        build_view: F,
    ) -> ViewHandle<V>
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.add_view_internal(window_id, Some(parent_view_id), build_view)
    }

    fn add_view_internal<V, F>(
        &mut self,
        window_id: WindowId,
        parent_view_id: Option<EntityId>,
        build_view: F,
    ) -> ViewHandle<V>
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.pending_flushes += 1;

        let view_id = EntityId::new();
        let mut ctx = ViewContext::new(self, window_id, view_id);
        let view = build_view(&mut ctx);
        self.windows
            .get_mut(&window_id)
            .expect("cannot add a view to a window that does not exist")
            .views
            .insert(view_id, Box::new(view));

        self.view_to_window.insert(view_id, window_id);
        if let Some(parent_view_id) = parent_view_id {
            self.record_view_parent(window_id, view_id, parent_view_id);
        }
        self.add_typed_action::<V>();
        self.window_invalidations
            .entry(window_id)
            .or_default()
            .updated
            .insert(view_id);

        let handle = ViewHandle::new(window_id, view_id, &self.ref_counts);
        self.flush_effects();
        handle
    }

    // --------------------------------------------------------- responder chain

    /// Records that `view_id` is embedded in `parent_view_id`.
    pub(crate) fn record_view_parent(
        &mut self,
        window_id: WindowId,
        view_id: EntityId,
        parent_view_id: EntityId,
    ) {
        self.view_parents
            .entry(window_id)
            .or_default()
            .insert(view_id, parent_view_id);
    }

    /// Records a batch of child-to-parent relationships discovered by layout.
    pub fn report_view_embeddings(
        &mut self,
        window_id: WindowId,
        embeddings: EntityIdMap<EntityId>,
    ) {
        self.view_parents
            .entry(window_id)
            .or_default()
            .extend(embeddings);
    }

    /// The chain from the window's root down to `view_id`, inclusive.
    pub fn view_ancestors(&self, window_id: WindowId, mut view_id: EntityId) -> Vec<EntityId> {
        let mut chain = vec![view_id];
        if let Some(parents) = self.view_parents.get(&window_id) {
            while let Some(parent_id) = parents.get(&view_id) {
                if chain.contains(parent_id) {
                    log::error!("cycle in the view hierarchy at view {parent_id}");
                    break;
                }
                view_id = *parent_id;
                chain.push(view_id);
            }
        }
        chain.reverse();
        chain
    }

    /// Whether `view_id` or anything below it holds focus.
    pub fn is_view_or_descendant_focused(&self, window_id: WindowId, view_id: EntityId) -> bool {
        self.focused_view_id(window_id).is_some_and(|focused_id| {
            self.view_ancestors(window_id, focused_id)
                .contains(&view_id)
        })
    }

    // ---------------------------------------------------------------- actions

    /// Registers the action handler for a view type, once per type.
    ///
    /// The handler downcasts both the view and the action. That is sound
    /// because the table is keyed by both types, so it can only be reached
    /// when both downcasts are known to succeed.
    fn add_typed_action<V: TypedActionView + Entity>(&mut self) {
        let handler = Box::new(
            |view: &mut dyn Any,
             action: &dyn Any,
             window_id: WindowId,
             view_id: EntityId,
             app: &mut AppContext| {
                let action = action.downcast_ref().expect("keyed by action type");
                let view = view.downcast_mut().expect("keyed by view type");
                let mut ctx = ViewContext::new(app, window_id, view_id);
                V::handle_action(view, action, &mut ctx);
            },
        );

        self.typed_actions
            .entry(ActionType::of::<V::Action>())
            .or_default()
            .entry(ViewType::of::<V>())
            .or_insert(handler);
    }

    /// Dispatches an action up the responder chain of one view.
    pub fn dispatch_typed_action_for_view(
        &mut self,
        window_id: WindowId,
        view_id: EntityId,
        action: &dyn Action,
    ) -> bool {
        if !self.is_window_open(window_id) {
            return false;
        }
        let responder_chain = self.view_ancestors(window_id, view_id);
        self.dispatch_typed_action(window_id, &responder_chain, action)
    }

    /// Dispatches an action along an explicit responder chain.
    ///
    /// The chain is walked leaf to root and stops at the **first** view
    /// registered for this action's type — typed actions do not keep bubbling
    /// once handled. Views in between that handle other action types are
    /// simply passed over, which is how an action reaches an ancestor several
    /// levels up.
    pub fn dispatch_typed_action(
        &mut self,
        window_id: WindowId,
        responder_chain: &[EntityId],
        action: &dyn Action,
    ) -> bool {
        let action_type = ActionType::from(action);

        // Taking the whole handler table for this action type out means a
        // handler that dispatches the same action type re-entrantly finds
        // nothing. That is why deferred dispatch exists.
        let Some(mut handlers) = self.typed_actions.remove(&action_type) else {
            log::warn!("no view handles {}", action.type_name());
            return false;
        };

        self.pending_flushes += 1;

        let handled = responder_chain.iter().rev().any(|view_id| {
            let Some(mut view) = self
                .windows
                .get_mut(&window_id)
                .and_then(|window| window.views.remove(view_id))
            else {
                return false;
            };

            let view_type = ViewType::of_value(view.as_any());
            let handled = match handlers.get_mut(&view_type) {
                Some(handler) => {
                    handler(
                        view.as_any_mut(),
                        action.as_any(),
                        window_id,
                        *view_id,
                        self,
                    );
                    true
                }
                None => false,
            };

            if let Some(window) = self.windows.get_mut(&window_id) {
                window.views.insert(*view_id, view);
            }
            handled
        });

        self.typed_actions.insert(action_type, handlers);

        if !handled {
            log::warn!("{action:?} was dispatched, but no view in the chain handled it");
        }

        self.flush_effects();
        handled
    }

    // ---------------------------------------------------- events and observers

    /// Cancels a view's subscriptions to one entity.
    pub(crate) fn unsubscribe_view(&mut self, entity_id: EntityId, subscriber_id: EntityId) {
        self.unsubscribe(entity_id, |subscription| match subscription {
            Subscription::FromView { view_id, .. } => *view_id != subscriber_id,
            Subscription::FromModel { .. } => true,
        });
    }

    /// Cancels a model's subscriptions to one entity.
    pub(crate) fn unsubscribe_model(&mut self, entity_id: EntityId, subscriber_id: EntityId) {
        self.unsubscribe(entity_id, |subscription| match subscription {
            Subscription::FromModel { model_id, .. } => *model_id != subscriber_id,
            Subscription::FromView { .. } => true,
        });
    }

    fn unsubscribe(&mut self, entity_id: EntityId, keep: impl Fn(&Subscription) -> bool) {
        if let Some(subscriptions) = self.subscriptions.get_mut(&entity_id) {
            subscriptions.retain(keep);
            if subscriptions.is_empty() {
                self.subscriptions.remove(&entity_id);
            }
        }
    }

    fn emit_event(&mut self, entity_id: EntityId, payload: Box<dyn Any>) {
        let Some(subscriptions) = self.subscriptions.remove(&entity_id) else {
            return;
        };

        let mut to_reinsert = Vec::new();
        for mut subscription in subscriptions {
            // A subscriber that cannot be checked out is either gone or in the
            // middle of its own update. Either way its subscription is dropped
            // here rather than re-inserted: it will never fire again.
            let alive = match &mut subscription {
                Subscription::FromModel { model_id, callback } => {
                    match self.models.remove(model_id) {
                        Some(mut model) => {
                            callback(model.as_any_mut(), payload.as_ref(), *model_id, self);
                            self.models.insert(*model_id, model);
                            true
                        }
                        None => false,
                    }
                }
                Subscription::FromView {
                    window_id,
                    view_id,
                    callback,
                } => {
                    match self
                        .windows
                        .get_mut(window_id)
                        .and_then(|window| window.views.remove(view_id))
                    {
                        Some(mut view) => {
                            callback(
                                view.as_any_mut(),
                                payload.as_ref(),
                                *window_id,
                                *view_id,
                                self,
                            );
                            match self.windows.get_mut(window_id) {
                                Some(window) => {
                                    window.views.insert(*view_id, view);
                                    true
                                }
                                None => false,
                            }
                        }
                        None => false,
                    }
                }
            };

            if alive {
                to_reinsert.push(subscription);
            }
        }

        // Subscriptions added by the callbacks above go in front of the ones
        // that were already there, so a callback that subscribes during an
        // event is called before the older subscription on the next one.
        let mut surviving = self.subscriptions.remove(&entity_id).unwrap_or_default();
        surviving.extend(to_reinsert);
        if !surviving.is_empty() {
            self.subscriptions.insert(entity_id, surviving);
        }
    }

    fn notify_model_observers(&mut self, observed_id: EntityId) {
        let Some(observations) = self.observations.remove(&observed_id) else {
            return;
        };
        if !self.models.contains_key(&observed_id) {
            return;
        }

        for mut observation in observations {
            let alive = match &mut observation {
                Observation::FromModel { model_id, callback } => {
                    match self.models.remove(model_id) {
                        Some(mut model) => {
                            callback(model.as_any_mut(), observed_id, *model_id, self);
                            self.models.insert(*model_id, model);
                            true
                        }
                        None => false,
                    }
                }
                Observation::FromView {
                    window_id,
                    view_id,
                    callback,
                } => {
                    match self
                        .windows
                        .get_mut(window_id)
                        .and_then(|window| window.views.remove(view_id))
                    {
                        Some(mut view) => {
                            callback(view.as_any_mut(), observed_id, *window_id, *view_id, self);
                            if let Some(window) = self.windows.get_mut(window_id) {
                                window.views.insert(*view_id, view);
                            }
                            true
                        }
                        None => false,
                    }
                }
            };

            if alive {
                self.observations
                    .entry(observed_id)
                    .or_default()
                    .push(observation);
            }
        }
    }

    /// Marks a view dirty. This is all a view's `notify` does: the renderer is
    /// the only observer a view has.
    pub(crate) fn notify_view_observers(&mut self, window_id: WindowId, view_id: EntityId) {
        self.window_invalidations
            .entry(window_id)
            .or_default()
            .updated
            .insert(view_id);
    }

    // ------------------------------------------------------------------ focus

    /// Moves focus within a window.
    ///
    /// The newly focused and newly blurred views hear about themselves, and
    /// then every ancestor of each hears that a descendant changed. That
    /// second signal is what lets a tab bar restyle itself when focus moves
    /// into one of its tabs.
    pub fn focus(&mut self, window_id: WindowId, focused_id: EntityId) {
        if self.focused_view_id(window_id) == Some(focused_id) {
            return;
        }

        self.pending_flushes += 1;

        let blurred = self.windows.get_mut(&window_id).and_then(|window| {
            let blurred_id = window.focused_view.replace(focused_id);
            blurred_id.and_then(|id| window.views.remove(&id).map(|view| (id, view)))
        });

        if let Some((blurred_id, mut blurred)) = blurred {
            blurred.on_blur(&BlurContext::SelfBlurred, window_id, blurred_id, self);
            self.check_in_view(window_id, blurred_id, blurred);
            self.notify_ancestors(window_id, blurred_id, |view, window_id, view_id, app| {
                view.on_blur(
                    &BlurContext::DescendentBlurred(blurred_id),
                    window_id,
                    view_id,
                    app,
                );
            });
        }

        if let Some(mut focused) = self
            .windows
            .get_mut(&window_id)
            .and_then(|window| window.views.remove(&focused_id))
        {
            focused.on_focus(&FocusContext::SelfFocused, window_id, focused_id, self);
            self.check_in_view(window_id, focused_id, focused);
            self.notify_ancestors(window_id, focused_id, |view, window_id, view_id, app| {
                view.on_focus(
                    &FocusContext::DescendentFocused(focused_id),
                    window_id,
                    view_id,
                    app,
                );
            });
        }

        self.flush_effects();
    }

    /// Runs `notify` on every ancestor of `view_id`, nearest first.
    fn notify_ancestors(
        &mut self,
        window_id: WindowId,
        view_id: EntityId,
        notify: impl Fn(&mut dyn AnyView, WindowId, EntityId, &mut AppContext),
    ) {
        // Skipping one entry drops `view_id` itself, which was told directly.
        for ancestor_id in self
            .view_ancestors(window_id, view_id)
            .into_iter()
            .rev()
            .skip(1)
        {
            if let Some(mut ancestor) = self
                .windows
                .get_mut(&window_id)
                .and_then(|window| window.views.remove(&ancestor_id))
            {
                notify(ancestor.as_mut(), window_id, ancestor_id, self);
                self.check_in_view(window_id, ancestor_id, ancestor);
            }
        }
    }

    fn check_in_view(&mut self, window_id: WindowId, view_id: EntityId, view: Box<dyn AnyView>) {
        // The window can be gone: a callback is allowed to close it.
        if let Some(window) = self.windows.get_mut(&window_id) {
            window.views.insert(view_id, view);
        }
    }

    // ------------------------------------------------------------ invalidation

    /// Registers the callback that redraws a window when its views change.
    pub fn on_window_invalidated<F>(&mut self, window_id: WindowId, callback: F)
    where
        F: 'static + FnMut(WindowId, &mut AppContext),
    {
        self.invalidation_callbacks
            .insert(window_id, Box::new(callback));
    }

    /// Whether a window has pending invalidations.
    pub fn has_window_invalidations(&self, window_id: WindowId) -> bool {
        self.window_invalidations
            .get(&window_id)
            .is_some_and(|invalidation| !invalidation.is_empty())
    }

    /// Takes everything that changed in a window since the last frame.
    pub fn take_all_invalidations_for_window(&mut self, window_id: WindowId) -> WindowInvalidation {
        self.window_invalidations
            .remove(&window_id)
            .unwrap_or_default()
    }

    /// Asks a window to redraw even though no view changed.
    pub fn request_redraw(&mut self, window_id: WindowId) {
        self.window_invalidations
            .entry(window_id)
            .or_default()
            .redraw_requested = true;
    }

    // ----------------------------------------------------------------- effects

    /// Runs queued effects, once the outermost update has finished.
    ///
    /// Nested updates each decrement the counter on the way out; only the last
    /// one drains, at which point every entity is checked back in and it is
    /// safe for a callback to reach anything.
    pub(crate) fn flush_effects(&mut self) {
        self.pending_flushes -= 1;
        if self.flushing_effects || self.pending_flushes != 0 {
            return;
        }

        self.flushing_effects = true;
        self.remove_dropped_items();

        while let Some(effect) = self.pending_effects.pop_front() {
            match effect {
                Effect::Event { entity_id, payload } => self.emit_event(entity_id, payload),
                Effect::ModelNotification { model_id } => self.notify_model_observers(model_id),
                Effect::ViewNotification { window_id, view_id } => {
                    self.notify_view_observers(window_id, view_id)
                }
                Effect::Focus { window_id, view_id } => self.focus(window_id, view_id),
                Effect::TypedAction {
                    window_id,
                    view_id,
                    action,
                } => {
                    self.dispatch_typed_action_for_view(window_id, view_id, action.as_ref());
                }
            }

            // Every effect can drop the last handle to something — including,
            // via a view's own callback, the view that owned the effect.
            self.remove_dropped_items();
        }

        self.flushing_effects = false;
        self.update_windows();
    }

    /// Removes entities whose last handle went away.
    ///
    /// Loops because dropping an entity drops the handles it owned, which can
    /// drop more entities.
    fn remove_dropped_items(&mut self) {
        loop {
            let dropped = self.ref_counts.lock().take_dropped();
            if dropped.is_empty() {
                break;
            }

            for model_id in dropped.models {
                self.models.remove(&model_id);
                self.subscriptions.remove(&model_id);
                self.observations.remove(&model_id);
            }

            for view_id in dropped.views {
                let Some(window_id) = self.view_to_window.remove(&view_id) else {
                    continue;
                };

                // Focus cannot be left pointing at a removed view, or the
                // window would stop routing keystrokes anywhere at all.
                if self.focused_view_id(window_id) == Some(view_id)
                    && let Some(root_view_id) = self.root_view_id(window_id)
                {
                    self.focus(window_id, root_view_id);
                }

                self.subscriptions.remove(&view_id);
                self.observations.remove(&view_id);
                if let Some(window) = self.windows.get_mut(&window_id) {
                    window.views.remove(&view_id);
                }
                if let Some(parents) = self.view_parents.get_mut(&window_id) {
                    parents.remove(&view_id);
                }
                self.window_invalidations
                    .entry(window_id)
                    .or_default()
                    .removed
                    .insert(view_id);
            }
        }
    }

    /// Fires the invalidation callback of every window that changed.
    fn update_windows(&mut self) {
        let invalidated: Vec<_> = self.window_invalidations.keys().copied().collect();
        for window_id in invalidated {
            // Taking the callback out avoids handing it a borrow of the map it
            // lives in; it is put back whether or not it panicked its way out.
            if let Some(mut callback) = self.invalidation_callbacks.remove(&window_id) {
                callback(window_id, self);
                self.invalidation_callbacks.insert(window_id, callback);
            }
        }
    }
}

impl ModelAsRef for AppContext {
    fn model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T {
        self.models
            .get(&handle.id())
            .unwrap_or_else(|| {
                panic!(
                    "circular model reference for {}",
                    std::any::type_name::<T>()
                )
            })
            .as_any()
            .downcast_ref()
            .expect("a handle's type is the model's type")
    }
}

impl ReadModel for AppContext {
    fn read_model<T, F, S>(&self, handle: &ModelHandle<T>, read: F) -> S
    where
        T: Entity,
        F: FnOnce(&T, &AppContext) -> S,
    {
        read(self.model(handle), self)
    }
}

impl UpdateModel for AppContext {
    fn update_model<T, F, S>(&mut self, handle: &ModelHandle<T>, update: F) -> S
    where
        T: Entity,
        F: FnOnce(&mut T, &mut ModelContext<T>) -> S,
    {
        let mut model = self
            .models
            .remove(&handle.id())
            .expect("circular model update");

        self.pending_flushes += 1;
        let mut ctx = ModelContext::new(self, handle.id());
        let result = update(
            model
                .as_any_mut()
                .downcast_mut()
                .expect("a handle's type is the model's type"),
            &mut ctx,
        );
        self.models.insert(handle.id(), model);
        self.flush_effects();
        result
    }
}

impl ViewAsRef for AppContext {
    fn view<T: 'static>(&self, handle: &ViewHandle<T>) -> &T {
        self.try_view(handle)
            .unwrap_or_else(|| panic!("circular view reference for {}", std::any::type_name::<T>()))
    }

    fn try_view<T: 'static>(&self, handle: &ViewHandle<T>) -> Option<&T> {
        self.stored_view(handle.window_id(), handle.id())?
            .as_any()
            .downcast_ref()
    }
}

impl ReadView for AppContext {
    fn read_view<T, F, S>(&self, handle: &ViewHandle<T>, read: F) -> S
    where
        T: 'static,
        F: FnOnce(&T, &AppContext) -> S,
    {
        read(self.view(handle), self)
    }
}

impl UpdateView for AppContext {
    fn update_view<T, F, S>(&mut self, handle: &ViewHandle<T>, update: F) -> S
    where
        T: Entity,
        F: FnOnce(&mut T, &mut ViewContext<T>) -> S,
    {
        let window_id = handle.window_id();
        let mut view = self
            .windows
            .get_mut(&window_id)
            .expect("cannot update a view in a window that does not exist")
            .views
            .remove(&handle.id())
            .expect("circular view update");

        self.pending_flushes += 1;
        let mut ctx = ViewContext::new(self, window_id, handle.id());
        let result = update(
            view.as_any_mut()
                .downcast_mut()
                .expect("a handle's type is the view's type"),
            &mut ctx,
        );
        self.check_in_view(window_id, handle.id(), view);
        self.flush_effects();
        result
    }
}

impl GetSingletonModelHandle for AppContext {
    fn get_singleton_model_handle<T: SingletonEntity>(&self) -> ModelHandle<T> {
        self.singleton_models
            .get(&TypeId::of::<T>())
            .unwrap_or_else(|| panic!("no singleton model of type {}", std::any::type_name::<T>()))
            .clone()
            .downcast()
            .expect("a registered singleton always has an outstanding handle")
    }
}

impl AddSingletonModel for AppContext {
    fn add_singleton_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: SingletonEntity,
        F: FnOnce(&mut ModelContext<T>) -> T,
    {
        let handle = self.add_model(build_model);
        let previous = self
            .singleton_models
            .insert(TypeId::of::<T>(), handle.clone().into());
        debug_assert!(
            previous.is_none(),
            "a second singleton model of type {} was registered",
            std::any::type_name::<T>()
        );
        handle
    }
}

impl ModelAsRef for App {
    /// Always panics. A reference handed out here would outlive the `RefCell`
    /// borrow it came from; `App::read` scopes that borrow to a closure, which
    /// is why it is the only way to reach an entity from outside the app.
    fn model<T: Entity>(&self, _: &ModelHandle<T>) -> &T {
        unimplemented!("borrow the model inside App::read instead")
    }
}

impl ReadModel for App {
    fn read_model<T, F, S>(&self, handle: &ModelHandle<T>, read: F) -> S
    where
        T: Entity,
        F: FnOnce(&T, &AppContext) -> S,
    {
        self.read(|ctx| ctx.read_model(handle, read))
    }
}

impl UpdateModel for App {
    fn update_model<T, F, S>(&mut self, handle: &ModelHandle<T>, update: F) -> S
    where
        T: Entity,
        F: FnOnce(&mut T, &mut ModelContext<T>) -> S,
    {
        self.update(|ctx| ctx.update_model(handle, update))
    }
}

impl ViewAsRef for App {
    /// Always panics, for the same reason as [`ModelAsRef::model`] on `App`.
    fn view<T: 'static>(&self, _: &ViewHandle<T>) -> &T {
        unimplemented!("borrow the view inside App::read instead")
    }

    /// Always panics, for the same reason as [`ModelAsRef::model`] on `App`.
    fn try_view<T: 'static>(&self, _: &ViewHandle<T>) -> Option<&T> {
        unimplemented!("borrow the view inside App::read instead")
    }
}

impl ReadView for App {
    fn read_view<T, F, S>(&self, handle: &ViewHandle<T>, read: F) -> S
    where
        T: 'static,
        F: FnOnce(&T, &AppContext) -> S,
    {
        self.read(|ctx| ctx.read_view(handle, read))
    }
}

impl UpdateView for App {
    fn update_view<T, F, S>(&mut self, handle: &ViewHandle<T>, update: F) -> S
    where
        T: Entity,
        F: FnOnce(&mut T, &mut ViewContext<T>) -> S,
    {
        self.update(|ctx| ctx.update_view(handle, update))
    }
}
