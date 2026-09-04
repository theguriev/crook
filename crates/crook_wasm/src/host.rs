//! What the guest may call, and what it leaves behind when it does.

use std::cell::RefCell;
use std::rc::Rc;

use crook_plugin_api::{Action, Contribution, Registered};

/// What a plugin registered while it was building.
///
/// Shared between the host functions — which are called from inside the guest
/// and hold no `&mut` to anything of the host's — and the [`Sandbox`] that
/// reads it afterwards.
///
/// [`Sandbox`]: crate::Sandbox
#[derive(Clone, Default)]
pub struct Registry(Rc<RefCell<Registered>>);

impl Registry {
    /// An empty one.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a contribution.
    pub(crate) fn contribute(&self, slot: String, entry: String, order: i32) {
        self.0
            .borrow_mut()
            .contributions
            .push(Contribution { slot, entry, order });
    }

    /// Records an action.
    pub(crate) fn register_action(&self, name: String, title: Option<String>) {
        self.0.borrow_mut().actions.push(Action { name, title });
    }

    /// Everything recorded so far.
    pub fn taken(&self) -> Registered {
        self.0.borrow().clone()
    }

    /// Forgets it, which is what a build that failed leaves behind.
    pub(crate) fn clear(&self) {
        *self.0.borrow_mut() = Registered::default();
    }
}
