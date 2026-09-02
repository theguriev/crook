//! Window identity and the per-window slice of application state.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use crate::core::{AnyView, AnyViewHandle, EntityId, EntityIdMap};

/// A unique identifier for a window.
///
/// Globally unique and never reused, so a stale id is always wrong rather than
/// accidentally right.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId(usize);

impl WindowId {
    /// Mints a globally-unique window id.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The application state that belongs to one window.
///
/// Crook opens a single window today. The id is threaded through every
/// signature anyway — through effects, callbacks, contexts and the parent map —
/// because a terminal grows a second window early, and retrofitting the id
/// afterwards means touching all of them at once.
#[derive(Default)]
pub(crate) struct Window {
    /// Every view in this window, by id.
    pub(crate) views: EntityIdMap<Box<dyn AnyView>>,

    /// The top of the view hierarchy. Holding a strong handle here is what
    /// keeps the root view (and therefore the tree it owns) alive.
    pub(crate) root_view: Option<AnyViewHandle>,

    /// Which view has keyboard focus, if any.
    pub(crate) focused_view: Option<EntityId>,
}
