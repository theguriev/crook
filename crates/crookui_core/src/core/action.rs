//! Typed actions: the whole type system for "something happened, handle it
//! somewhere up the view hierarchy".
//!
//! An action is any `'static + Debug + Send + Sync` value. There is no derive,
//! no registry and no string name — a blanket impl makes every such type an
//! action, and dispatch keys on its [`TypeId`]. In practice each view declares
//! one action enum, and firing a variant of some *other* view's enum is how an
//! event bubbles past every view in between.

use std::any::{Any, TypeId};
use std::fmt::Debug;

/// A dispatchable action.
///
/// `Any` so dispatch can downcast it back to the concrete type; `Debug` so it
/// can be logged as it travels; `Send + Sync` so it can be queued as a deferred
/// effect or handed across a spawn boundary.
pub trait Action: Any + Debug + Send + Sync {
    /// This action as `&dyn Any`.
    ///
    /// Needed because trait upcasting from `&dyn Action` to `&dyn Any` is what
    /// the handler's downcast requires, and the compiler will not do it for us.
    fn as_any(&self) -> &dyn Any;

    /// The action's type name, for logs.
    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

impl<T> Action for T
where
    T: Any + Debug + Send + Sync,
{
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The first half of a handler key: which action type this is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ActionType(TypeId);

impl ActionType {
    pub(crate) fn of<T: ?Sized + 'static>() -> Self {
        Self(TypeId::of::<T>())
    }
}

impl From<&dyn Action> for ActionType {
    fn from(action: &dyn Action) -> Self {
        Self(action.type_id())
    }
}

/// The second half of a handler key: which view type registered it.
///
/// Keying on both is what makes the two downcasts inside a handler sound: it
/// only ever runs when the stored view really is a `V` and the action really
/// is a `V::Action`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ViewType(TypeId);

impl ViewType {
    pub(crate) fn of<T: ?Sized + 'static>() -> Self {
        Self(TypeId::of::<T>())
    }

    /// The key for a view that has already been type-erased.
    pub(crate) fn of_value(view: &dyn Any) -> Self {
        Self(view.type_id())
    }
}
