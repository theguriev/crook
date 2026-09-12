//! The pane's own places, and the one thing a plugin may put in one.
//!
//! # The chips
//!
//! [`PANE_CHIPS`] is a row of small things about the pane a person is working
//! in: where it is, what branch it is on, what a chord would do. Warp draws
//! exactly this, and where it draws it is the interesting part — the chips sit
//! *with the prompt* while there is one, and move to the bottom corner of the
//! screen while a program has taken it. Both are the same statement about the
//! same pane, so both are this one slot, and which of the two a contribution
//! lands in is not something it is told: a chip that had to know whether an
//! agent was running would be a chip with an opinion about the terminal.
//!
//! A `List`, not a `Single`. The header takes one item because it is also the
//! window's title bar and a row of competing chips across it is how a status
//! bar becomes a place nobody reads. This row is the opposite case: it is
//! *about* one pane, it is next to the thing it describes, and three of them —
//! a directory, a branch, a hint — is the arrangement being copied.
//!
//! # Where they are drawn
//!
//! `workspace::body`, in two places, and it is worth saying why the slot is
//! declared here rather than there. The plugin that owns a place in the
//! interface is the one that can say what belongs in it; the body is the
//! surface that happens to paint it. That is the same split `header.right` and
//! `header_toolbar` are on either side of.
//!
//! Only the **focused** pane draws them. A chip says what the *active* pane is
//! doing — which is the same pane [`Capability::ReadWorkingDirectory`] lets a
//! plugin ask about — and a split with four copies of the row in it would be
//! four plugins polling for four answers, three of which nobody is looking at.
//!
//! [`Capability::ReadWorkingDirectory`]: crook_plugin_api::Capability::ReadWorkingDirectory

use crookui_core::prelude::*;

use crook_plugin::{Cardinality, Manifest, PluginId, SlotId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;

/// The row of chips about the focused pane.
pub const PANE_CHIPS: SlotId = SlotId::new("pane.chips");

/// The plugin that owns the pane's slots.
pub struct PaneSlots;

impl Plugin for PaneSlots {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Tag)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.declare_slot(PANE_CHIPS, Cardinality::List);
        Ok(())
    }
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/pane").expect("a literal that parses"),
        name: "Pane chips",
        description: "The row of chips about the pane being worked in: beside the prompt, or over the corner of a screen a program has taken.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}
