//! The two entity contexts: what an entity is handed while it runs.
//!
//! A context is a fat pointer and two ids — `&mut AppContext` plus "which
//! entity is speaking". It is created and dropped constantly, and it is the
//! only way an entity gets to act on anything outside itself.
//!
//! Both contexts `Deref` to [`AppContext`], so every app-wide method is in
//! scope on `ctx`. That is convenient and it is a real trade: it also puts
//! methods in scope that will panic given the current checked-out state, such
//! as reading the very entity whose update is running.

use std::future::Future;
use std::marker::PhantomData;

use crate::core::{
    Action, AppContext, Effect, Entity, EntityId, GetSingletonModelHandle, ModelAsRef, ModelHandle,
    Observation, ReadModel, ReadView, SingletonEntity, Subscription, TypedActionView, UpdateModel,
    UpdateView, View, ViewAsRef, ViewHandle, WeakModelHandle, WeakViewHandle, WindowId,
};
use crate::executor::Task;

/// What a view is handed while it runs.
pub struct ViewContext<'a, T: ?Sized> {
    app: &'a mut AppContext,
    window_id: WindowId,
    view_id: EntityId,
    view_type: PhantomData<T>,
}

impl<'a, T: Entity> ViewContext<'a, T> {
    pub(in crate::core) fn new(
        app: &'a mut AppContext,
        window_id: WindowId,
        view_id: EntityId,
    ) -> Self {
        Self {
            app,
            window_id,
            view_id,
            view_type: PhantomData,
        }
    }

    /// A weak handle to this view.
    ///
    /// Always weak: a view holding a strong handle to itself is a cycle the
    /// app can never collect. Spawned work re-upgrades against the live app.
    pub fn handle(&self) -> WeakViewHandle<T> {
        WeakViewHandle::new(self.window_id, self.view_id)
    }

    /// The window this view lives in.
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// This view's id.
    pub fn view_id(&self) -> EntityId {
        self.view_id
    }

    /// Creates a model.
    pub fn add_model<S, F>(&mut self, build_model: F) -> ModelHandle<S>
    where
        S: Entity,
        F: FnOnce(&mut ModelContext<S>) -> S,
    {
        self.app.add_model(build_model)
    }

    /// Creates a child view in the same window.
    ///
    /// The new view records this one as its parent immediately, rather than
    /// waiting for a layout pass to discover the relationship. That matters
    /// for a view that is created but not always rendered: without creation
    /// time parentage its responder chain would be just itself, and actions
    /// dispatched from it would never reach this view.
    pub fn add_view<V, F>(&mut self, build_view: F) -> ViewHandle<V>
    where
        V: TypedActionView + View,
        F: FnOnce(&mut ViewContext<V>) -> V,
    {
        self.app
            .add_view_with_parent(self.window_id, self.view_id, build_view)
    }

    /// Marks this view dirty, so it re-renders on the next frame.
    ///
    /// Dirtiness applies to this view alone, never to its children: each view
    /// that changed must say so itself.
    pub fn notify(&mut self) {
        self.app
            .pending_effects
            .push_back(Effect::ViewNotification {
                window_id: self.window_id,
                view_id: self.view_id,
            });
    }

    /// Emits an event to this view's subscribers.
    pub fn emit(&mut self, payload: T::Event) {
        self.app.pending_effects.push_back(Effect::Event {
            entity_id: self.view_id,
            payload: Box::new(payload),
        });
    }

    /// Moves focus to another view.
    pub fn focus<S: Entity>(&mut self, handle: &ViewHandle<S>) {
        self.app.pending_effects.push_back(Effect::Focus {
            window_id: handle.window_id(),
            view_id: handle.id(),
        });
    }

    /// Moves focus to this view.
    pub fn focus_self(&mut self) {
        self.app.pending_effects.push_back(Effect::Focus {
            window_id: self.window_id,
            view_id: self.view_id,
        });
    }

    /// Whether this view holds focus.
    pub fn is_self_focused(&self) -> bool {
        self.app.focused_view_id(self.window_id) == Some(self.view_id)
    }

    /// Whether this view or one of its descendants holds focus.
    ///
    /// This is how a tab bar knows which tab is active without polling.
    pub fn is_self_or_child_focused(&self) -> bool {
        self.app
            .is_view_or_descendant_focused(self.window_id, self.view_id)
    }

    /// Runs `callback` whenever `handle`'s model notifies.
    ///
    /// Observation carries no payload — it means "I changed, look again". A
    /// view that observes a model and calls [`Self::notify`] is the canonical
    /// "model changed, redraw this chip" bridge.
    ///
    /// There is no `observe` for views, by design: view-to-view reactivity
    /// goes through a shared model, or through
    /// [`Self::subscribe_to_view`] with an explicit event.
    pub fn observe<S, F>(&mut self, handle: &ModelHandle<S>, mut callback: F)
    where
        S: Entity,
        F: 'static + FnMut(&mut T, ModelHandle<S>, &mut ViewContext<T>),
    {
        self.app
            .observations
            .entry(handle.id())
            .or_default()
            .push(Observation::FromView {
                window_id: self.window_id,
                view_id: self.view_id,
                callback: Box::new(move |view, observed_id, window_id, view_id, app| {
                    let observed = ModelHandle::new(observed_id, &app.ref_counts);
                    let view = view.downcast_mut().expect("keyed by view type");
                    let mut ctx = ViewContext::new(app, window_id, view_id);
                    callback(view, observed, &mut ctx);
                }),
            });
    }

    /// Runs `callback` for every event a model emits.
    pub fn subscribe_to_model<S, F>(&mut self, handle: &ModelHandle<S>, mut callback: F)
    where
        S: Entity,
        F: 'static + FnMut(&mut T, ModelHandle<S>, &S::Event, &mut ViewContext<T>),
    {
        let emitter = handle.downgrade();
        self.app
            .subscriptions
            .entry(handle.id())
            .or_default()
            .push(Subscription::FromView {
                window_id: self.window_id,
                view_id: self.view_id,
                callback: Box::new(move |view, payload, window_id, view_id, app| {
                    if let Some(emitter) = emitter.upgrade(app) {
                        let view = view.downcast_mut().expect("keyed by view type");
                        let payload = payload.downcast_ref().expect("keyed by emitter");
                        let mut ctx = ViewContext::new(app, window_id, view_id);
                        callback(view, emitter, payload, &mut ctx);
                    }
                }),
            });
    }

    /// Runs `callback` for every event another view emits.
    pub fn subscribe_to_view<V, F>(&mut self, handle: &ViewHandle<V>, mut callback: F)
    where
        V: Entity,
        F: 'static + FnMut(&mut T, ViewHandle<V>, &V::Event, &mut ViewContext<T>),
    {
        let emitter = handle.downgrade();
        self.app
            .subscriptions
            .entry(handle.id())
            .or_default()
            .push(Subscription::FromView {
                window_id: self.window_id,
                view_id: self.view_id,
                callback: Box::new(move |view, payload, window_id, view_id, app| {
                    if let Some(emitter) = emitter.upgrade(app) {
                        let view = view.downcast_mut().expect("keyed by view type");
                        let payload = payload.downcast_ref().expect("keyed by emitter");
                        let mut ctx = ViewContext::new(app, window_id, view_id);
                        callback(view, emitter, payload, &mut ctx);
                    }
                }),
            });
    }

    /// Cancels this view's subscriptions to `handle`'s events.
    pub fn unsubscribe_from_view<V: Entity>(&mut self, handle: &ViewHandle<V>) {
        self.app.unsubscribe_view(handle.id(), self.view_id);
    }

    /// Cancels this view's subscriptions to `handle`'s events.
    pub fn unsubscribe_from_model<S: Entity>(&mut self, handle: &ModelHandle<S>) {
        self.app.unsubscribe_view(handle.id(), self.view_id);
    }

    /// Dispatches an action up this view's responder chain, immediately.
    ///
    /// Not usable from inside an action handler for the same action type: the
    /// handler table for that type is checked out for the duration of a
    /// dispatch, so a re-entrant dispatch finds no handlers. Use
    /// [`Self::dispatch_typed_action_deferred`] there.
    pub fn dispatch_typed_action(&mut self, action: &dyn Action) {
        self.app
            .dispatch_typed_action_for_view(self.window_id, self.view_id, action);
    }

    /// Queues an action to be dispatched once the current work settles.
    pub fn dispatch_typed_action_deferred<A: Action>(&mut self, action: A) {
        self.app.pending_effects.push_back(Effect::TypedAction {
            window_id: self.window_id,
            view_id: self.view_id,
            action: Box::new(action),
        });
    }

    /// Runs `future` on the main thread, then applies `callback` to this view.
    ///
    /// The view is re-acquired by id when the future completes, so a view that
    /// was dropped in the meantime simply does not get the callback. To do
    /// real work off the main thread, spawn it on
    /// [`AppContext::background`](crate::AppContext::background) and await the
    /// resulting task here.
    ///
    /// Dropping the returned task cancels the future.
    pub fn spawn<Fut, F>(&mut self, future: Fut, callback: F) -> Task<()>
    where
        Fut: 'static + Future,
        F: 'static + FnOnce(&mut T, Fut::Output, &mut ViewContext<T>),
    {
        let handle = self.handle();
        let app = self.app.weak_app();
        self.app.foreground.clone().spawn(async move {
            let output = future.await;
            if let Some(mut app) = app.upgrade() {
                app.update(|ctx| {
                    if let Some(handle) = handle.upgrade(ctx) {
                        handle.update(ctx, |view, ctx| callback(view, output, ctx));
                    }
                });
            }
        })
    }
}

impl<T> std::ops::Deref for ViewContext<'_, T> {
    type Target = AppContext;

    fn deref(&self) -> &AppContext {
        self.app
    }
}

impl<T> std::ops::DerefMut for ViewContext<'_, T> {
    fn deref_mut(&mut self) -> &mut AppContext {
        self.app
    }
}

/// What a model is handed while it runs.
///
/// The same shape as [`ViewContext`] minus everything window-shaped: a model
/// has no place on screen, no focus and no responder chain.
pub struct ModelContext<'a, T: ?Sized> {
    app: &'a mut AppContext,
    model_id: EntityId,
    model_type: PhantomData<T>,
}

impl<'a, T: Entity> ModelContext<'a, T> {
    pub(in crate::core) fn new(app: &'a mut AppContext, model_id: EntityId) -> Self {
        Self {
            app,
            model_id,
            model_type: PhantomData,
        }
    }

    /// A weak handle to this model.
    pub fn handle(&self) -> WeakModelHandle<T> {
        WeakModelHandle::new(self.model_id)
    }

    /// This model's id.
    pub fn model_id(&self) -> EntityId {
        self.model_id
    }

    /// Creates another model.
    pub fn add_model<S, F>(&mut self, build_model: F) -> ModelHandle<S>
    where
        S: Entity,
        F: FnOnce(&mut ModelContext<S>) -> S,
    {
        self.app.add_model(build_model)
    }

    /// Emits an event to this model's subscribers.
    pub fn emit(&mut self, payload: T::Event) {
        self.app.pending_effects.push_back(Effect::Event {
            entity_id: self.model_id,
            payload: Box::new(payload),
        });
    }

    /// Tells this model's observers that it changed.
    pub fn notify(&mut self) {
        // Collapsing a repeat of the effect already at the back of the queue
        // keeps a loop that mutates a model N times from running every
        // observer N times.
        if let Some(Effect::ModelNotification { model_id }) = self.app.pending_effects.back()
            && *model_id == self.model_id
        {
            return;
        }

        self.app
            .pending_effects
            .push_back(Effect::ModelNotification {
                model_id: self.model_id,
            });
    }

    /// Runs `callback` whenever another model notifies.
    pub fn observe<S, F>(&mut self, handle: &ModelHandle<S>, mut callback: F)
    where
        S: Entity,
        F: 'static + FnMut(&mut T, ModelHandle<S>, &mut ModelContext<T>),
    {
        self.app
            .observations
            .entry(handle.id())
            .or_default()
            .push(Observation::FromModel {
                model_id: self.model_id,
                callback: Box::new(move |model, observed_id, model_id, app| {
                    let observed = ModelHandle::new(observed_id, &app.ref_counts);
                    let model = model.downcast_mut().expect("keyed by model type");
                    let mut ctx = ModelContext::new(app, model_id);
                    callback(model, observed, &mut ctx);
                }),
            });
    }

    /// Runs `callback` for every event another model emits.
    ///
    /// A model may not subscribe to itself: emitting checks the subscriber out
    /// of the app before running its callback, so the handle it would be given
    /// could not be upgraded and the callback would silently never run.
    pub fn subscribe_to_model<S, F>(&mut self, handle: &ModelHandle<S>, mut callback: F)
    where
        S: Entity,
        F: 'static + FnMut(&mut T, ModelHandle<S>, &S::Event, &mut ModelContext<T>),
    {
        debug_assert_ne!(
            handle.id(),
            self.model_id,
            "a model must not subscribe to its own events"
        );

        let emitter = handle.downgrade();
        self.app
            .subscriptions
            .entry(handle.id())
            .or_default()
            .push(Subscription::FromModel {
                model_id: self.model_id,
                callback: Box::new(move |model, payload, model_id, app| {
                    if let Some(emitter) = emitter.upgrade(app) {
                        let model = model.downcast_mut().expect("keyed by model type");
                        let payload = payload.downcast_ref().expect("keyed by emitter");
                        let mut ctx = ModelContext::new(app, model_id);
                        callback(model, emitter, payload, &mut ctx);
                    }
                }),
            });
    }

    /// Cancels this model's subscriptions to `handle`'s events.
    pub fn unsubscribe_from_model<S: Entity>(&mut self, handle: &ModelHandle<S>) {
        self.app.unsubscribe_model(handle.id(), self.model_id);
    }

    /// Runs `future` on the main thread, then applies `callback` to this model.
    ///
    /// Dropping the returned task cancels the future.
    pub fn spawn<Fut, F>(&mut self, future: Fut, callback: F) -> Task<()>
    where
        Fut: 'static + Future,
        F: 'static + FnOnce(&mut T, Fut::Output, &mut ModelContext<T>),
    {
        let handle = self.handle();
        let app = self.app.weak_app();
        self.app.foreground.clone().spawn(async move {
            let output = future.await;
            if let Some(mut app) = app.upgrade() {
                app.update(|ctx| {
                    if let Some(handle) = handle.upgrade(ctx) {
                        handle.update(ctx, |model, ctx| callback(model, output, ctx));
                    }
                });
            }
        })
    }
}

impl<T> std::ops::Deref for ModelContext<'_, T> {
    type Target = AppContext;

    fn deref(&self) -> &AppContext {
        self.app
    }
}

impl<T> std::ops::DerefMut for ModelContext<'_, T> {
    fn deref_mut(&mut self) -> &mut AppContext {
        self.app
    }
}

macro_rules! impl_entity_access {
    ($context:ident) => {
        impl<V> ModelAsRef for $context<'_, V> {
            fn model<T: Entity>(&self, handle: &ModelHandle<T>) -> &T {
                self.app.model(handle)
            }
        }

        impl<V> ReadModel for $context<'_, V> {
            fn read_model<T, F, S>(&self, handle: &ModelHandle<T>, read: F) -> S
            where
                T: Entity,
                F: FnOnce(&T, &AppContext) -> S,
            {
                self.app.read_model(handle, read)
            }
        }

        impl<V> UpdateModel for $context<'_, V> {
            fn update_model<T, F, S>(&mut self, handle: &ModelHandle<T>, update: F) -> S
            where
                T: Entity,
                F: FnOnce(&mut T, &mut ModelContext<T>) -> S,
            {
                self.app.update_model(handle, update)
            }
        }

        impl<V> ViewAsRef for $context<'_, V> {
            fn view<T: 'static>(&self, handle: &ViewHandle<T>) -> &T {
                self.app.view(handle)
            }

            fn try_view<T: 'static>(&self, handle: &ViewHandle<T>) -> Option<&T> {
                self.app.try_view(handle)
            }
        }

        impl<V> ReadView for $context<'_, V> {
            fn read_view<T, F, S>(&self, handle: &ViewHandle<T>, read: F) -> S
            where
                T: 'static,
                F: FnOnce(&T, &AppContext) -> S,
            {
                self.app.read_view(handle, read)
            }
        }

        impl<V> UpdateView for $context<'_, V> {
            fn update_view<T, F, S>(&mut self, handle: &ViewHandle<T>, update: F) -> S
            where
                T: Entity,
                F: FnOnce(&mut T, &mut ViewContext<T>) -> S,
            {
                self.app.update_view(handle, update)
            }
        }

        impl<V> GetSingletonModelHandle for $context<'_, V> {
            fn get_singleton_model_handle<T: SingletonEntity>(&self) -> ModelHandle<T> {
                self.app.get_singleton_model_handle()
            }
        }
    };
}

impl_entity_access!(ViewContext);
impl_entity_access!(ModelContext);
