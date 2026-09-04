//! A group of tabs: what Warp folds several tabs under one heading with.
//!
//! **This is not [`PaneGroup`](super::PaneGroup), and the difference is the
//! whole point of the module.** A pane group is one tab's split — several
//! terminals sharing a rectangle, side by side, all visible at once. A tab
//! group is several *tabs* sharing a heading in the panel — one visible at a
//! time, exactly as before, with a fold above them saying they belong
//! together. A worktree opened from a tab wants the second and never the
//! first: two agents working two checkouts of one repository are two places to
//! be, not two halves of one screen.
//!
//! The membership lives on the [`Tab`](super::Tab) — `group: Option<GroupId>`
//! — rather than as a list of ids here, and that is Warp's shape
//! (`TabData::group_id`) for Warp's reason: the tabs are already an ordered
//! vector, and a second ordered list of the same tabs is a second answer to
//! "what order are they in" that nothing stops from disagreeing with the
//! first. What this type holds is only what a group has *of its own*: a name,
//! and whether it is folded away.
//!
//! **One invariant, kept by [`TabStrip`](super::TabStrip) and nowhere else: a
//! group's members are contiguous in the strip.** It is what makes a group
//! drawable at all — a heading with its members under it cannot be drawn from
//! tabs scattered through the list — and it is why every move that touches
//! membership goes through one clamp rather than trusting its caller.

use std::sync::atomic::{AtomicU64, Ordering};

/// A group's identity, stable for as long as the group exists.
///
/// Minted from a process-wide counter and never reused, exactly like
/// [`TabId`](super::TabId), so an action naming a group that has since been
/// pruned resolves to nothing rather than to whichever group took its place.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(u64);

impl GroupId {
    /// Mints an id no other group in this process will ever have.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// One group: an identity, a name, and whether its members are folded away.
///
/// No colour, no pinning and no multi-selection, which Warp's has. Those are
/// three separate features hanging off the same struct there; what is here is
/// what a heading in the panel needs to draw itself.
#[derive(Clone, Debug)]
pub struct TabGroup {
    id: GroupId,
    name: String,
    collapsed: bool,
}

impl TabGroup {
    /// A fresh, expanded group called `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: GroupId::next(),
            name: name.into(),
            collapsed: false,
        }
    }

    /// This group's identity, for as long as it holds a tab.
    pub fn id(&self) -> GroupId {
        self.id
    }

    /// What the heading says.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Renames it. Crate-private: the strip is what a rename goes through.
    pub(crate) fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Whether the members are folded away behind the heading.
    pub fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    /// Folds the members away, or shows them again.
    pub(crate) fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }
}
