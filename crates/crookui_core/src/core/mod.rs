//! The application core: entities, handles, contexts and the effect queue.
//!
//! The whole design rests on one decision — there is exactly one `RefCell` in
//! the application, wrapping [`AppContext`], and it is entered only at the
//! edges. Inside it everything is a plain `&mut AppContext`: no interior
//! mutability, no locks, no `Rc<RefCell<MyView>>` in user code.
//!
//! Two consequences shape everything else here. Entities cannot hold
//! references to each other, so they hold [`ModelHandle`]/[`ViewHandle`] ids
//! and resolve them against a context. And nothing may re-enter the app while
//! an entity is checked out, so every method that would — notify, emit, focus,
//! dispatch — pushes an [`Effect`] that runs once the stack unwinds.
//!
//! There are two notification channels here, deliberately not unified. *Events*
//! (`emit`/`subscribe_to_*`) carry a typed payload to explicitly registered
//! listeners and never bubble. *Invalidations* (`notify`/`observe`) carry no
//! data at all: for a model they run observer callbacks, and for a view they
//! do nothing but mark it dirty, because the renderer is the only observer a
//! view has.

mod action;
mod app;
mod context;
mod entity;
mod handle;
mod refcount;
mod view;
mod window;

use std::any::Any;

pub use action::Action;
pub(crate) use action::{ActionType, ViewType};
pub use app::{App, AppContext, WeakApp};
pub use context::{ModelContext, ViewContext};
pub use entity::{
    AddSingletonModel, Entity, EntityId, EntityIdMap, EntityIdSet, GetSingletonModelHandle,
    SingletonEntity,
};
pub(crate) use handle::AnyViewHandle;
pub use handle::{
    AnyModelHandle, ModelAsRef, ModelHandle, ReadModel, ReadView, UpdateModel, UpdateView,
    ViewAsRef, ViewHandle, WeakModelHandle, WeakViewHandle,
};
pub use view::{AnyView, BlurContext, FocusContext, TypedActionView, View};
pub(crate) use window::Window;
pub use window::WindowId;

#[cfg(test)]
mod tests;

/// The object-safe erasure of a model.
///
/// Models have no behaviour the core calls into — unlike views they do not
/// render or take focus — so this is only what is needed to downcast one back
/// to its concrete type.
pub(crate) trait AnyModel {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Entity> AnyModel for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Work the app owes itself, queued rather than done inline.
///
/// Every context method that would re-enter the app pushes one of these; they
/// run in `flush_effects` once every entity has been checked back in.
pub(crate) enum Effect {
    /// An entity emitted an event for its subscribers.
    Event {
        entity_id: EntityId,
        payload: Box<dyn Any>,
    },
    /// A model changed; run its observers.
    ModelNotification { model_id: EntityId },
    /// A view changed; mark it for re-render.
    ViewNotification {
        window_id: WindowId,
        view_id: EntityId,
    },
    /// Move focus.
    Focus {
        window_id: WindowId,
        view_id: EntityId,
    },
    /// Dispatch an action up a view's responder chain.
    TypedAction {
        window_id: WindowId,
        view_id: EntityId,
        action: Box<dyn Action>,
    },
}

pub(crate) type SubscriptionFromModelCallback =
    dyn FnMut(&mut dyn Any, &dyn Any, EntityId, &mut AppContext);

pub(crate) type SubscriptionFromViewCallback =
    dyn FnMut(&mut dyn Any, &dyn Any, WindowId, EntityId, &mut AppContext);

pub(crate) type ObservationFromModelCallback =
    dyn FnMut(&mut dyn Any, EntityId, EntityId, &mut AppContext);

pub(crate) type ObservationFromViewCallback =
    dyn FnMut(&mut dyn Any, EntityId, WindowId, EntityId, &mut AppContext);

pub(crate) type TypedActionCallback =
    dyn FnMut(&mut dyn Any, &dyn Any, WindowId, EntityId, &mut AppContext);

pub(crate) type InvalidationCallback = dyn FnMut(WindowId, &mut AppContext);

/// Who is listening to an entity's events.
///
/// The subscriber is identified by id, never by reference: it has to be
/// checked out of the app before its callback can run.
pub(crate) enum Subscription {
    FromModel {
        model_id: EntityId,
        callback: Box<SubscriptionFromModelCallback>,
    },
    FromView {
        window_id: WindowId,
        view_id: EntityId,
        callback: Box<SubscriptionFromViewCallback>,
    },
}

/// Who is watching an entity for changes.
pub(crate) enum Observation {
    FromModel {
        model_id: EntityId,
        callback: Box<ObservationFromModelCallback>,
    },
    FromView {
        window_id: WindowId,
        view_id: EntityId,
        callback: Box<ObservationFromViewCallback>,
    },
}

/// What changed in a window since the renderer last looked.
#[derive(Clone, Debug, Default)]
pub struct WindowInvalidation {
    /// Views that must be re-rendered.
    pub updated: EntityIdSet,

    /// Views that are gone and whose element trees should be dropped.
    pub removed: EntityIdSet,

    /// Whether the frame must be rebuilt even if no view changed — a resize,
    /// for instance.
    pub redraw_requested: bool,
}

impl WindowInvalidation {
    /// Whether there is nothing to do.
    pub fn is_empty(&self) -> bool {
        self.updated.is_empty() && self.removed.is_empty() && !self.redraw_requested
    }
}
