//! Entity identity: what a handle points at.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::core::{AppContext, ModelContext, ModelHandle};

/// A unique identifier for a view or a model.
///
/// Views and models share one namespace on purpose: subscriptions,
/// observations and the effect queue key on an entity without caring which
/// kind it is, and that only works if the ids cannot collide.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityId(usize);

/// A hash map keyed by [`EntityId`].
pub type EntityIdMap<V> = FxHashMap<EntityId, V>;

/// A hash set of [`EntityId`]s.
pub type EntityIdSet = FxHashSet<EntityId>;

impl EntityId {
    /// Mints a globally-unique entity id.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// Something the app owns and that can emit events.
///
/// Both models and views are entities. `Event` is the payload type subscribers
/// receive; `()` is the right answer for an entity nothing listens to.
pub trait Entity: 'static {
    /// What this entity emits.
    type Event;
}

/// An entity the application holds exactly one of.
///
/// The point is reachability without plumbing: any view can get to the token
/// usage model with `TokenUsage::handle(ctx)` instead of being handed a copy
/// of the handle at construction time by every view above it.
pub trait SingletonEntity: Entity + Sized {
    /// The handle to the single instance of this type.
    ///
    /// Panics if no instance was registered with
    /// [`AppContext::add_singleton_model`](crate::AppContext::add_singleton_model).
    fn handle<T: GetSingletonModelHandle>(ctx: &T) -> ModelHandle<Self> {
        ctx.get_singleton_model_handle()
    }

    /// The single instance of this type, borrowed.
    ///
    /// Panics if no instance was registered, or if the instance is currently
    /// checked out by an update.
    fn as_ref(ctx: &AppContext) -> &Self {
        ctx.get_singleton_model_as_ref()
    }
}

/// Implemented by every context that can reach the singleton table.
pub trait GetSingletonModelHandle {
    /// The handle to the single model of type `T`.
    fn get_singleton_model_handle<T: SingletonEntity>(&self) -> ModelHandle<T>;
}

/// Implemented by every context that can register a singleton.
pub trait AddSingletonModel {
    /// Creates the single model of type `T` and registers it.
    fn add_singleton_model<T, F>(&mut self, build_model: F) -> ModelHandle<T>
    where
        T: SingletonEntity,
        F: FnOnce(&mut ModelContext<T>) -> T;
}
