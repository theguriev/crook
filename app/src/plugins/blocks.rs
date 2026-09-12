//! The menu a block opens, and the one place anything may be put in it.
//!
//! The blocks themselves are not a plugin and are not going to be: one address
//! space is shared by the marks, the selection, the copy and the paint, and
//! `docs/plugins.md` lists the block machinery among the things that are the
//! core. What this plugin owns is the *menu* — [`BLOCK_MENU`], the four groups
//! Crook puts in it, and therefore the answer to "where does a plugin put
//! something that is about one command?"
//!
//! That question had no answer before. A plugin could pin a chip to the header
//! and add a page to the settings, and neither of those knows what a person is
//! looking at; this slot is handed the block a menu was opened on, which is the
//! first surface in the application that is about the work rather than about
//! the window.
//!
//! [`Cardinality::List`], where the header is [`Single`](Cardinality::Single),
//! and the difference is the surface again: a menu is a list by construction —
//! it is read top to bottom and it can be as long as it is useful — where the
//! header is a row that is always on screen and a competition nobody wins.
//!
//! **A contribution is a group of entries, not one entry.** The menu draws a
//! hairline between contributions, so what a plugin adds arrives together and
//! under a rule of its own rather than interleaved with Crook's, where it would
//! read as something the terminal does. The drawing is
//! [`workspace::block_menu`](crate::workspace::block_menu)'s, exactly as the
//! header row is still drawn by `workspace::header_toolbar`: what a plugin owns
//! is the slot.

use crookui_core::prelude::*;

use crook_plugin::{Cardinality, Manifest, PluginId, SlotId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::{Workspace, block_menu};

/// Where an entry of a block's menu goes.
///
/// Declared by `crook/blocks`, which draws the menu; contributed to by
/// anything that has something to say about one command.
pub const BLOCK_MENU: SlotId = SlotId::new("block.menu");

/// The plugin that owns the block menu.
pub struct Blocks;

impl Plugin for Blocks {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Blocks)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.declare_slot(BLOCK_MENU, Cardinality::List);

        // Ten apart, so that a plugin can land between two of Crook's own
        // groups rather than only at the ends. Nothing here relies on the
        // numbers: they are the order the groups read in.
        host.contribute(BLOCK_MENU, "copy", 0, |workspace, _| {
            block_menu::copy_group(workspace)
        });
        host.contribute(BLOCK_MENU, "facts", 10, |workspace, _| {
            block_menu::facts_group(workspace)
        });
        host.contribute(BLOCK_MENU, "run", 20, |workspace, _| {
            block_menu::run_group(workspace)
        });
        host.contribute(BLOCK_MENU, "scroll", 30, |workspace, _| {
            block_menu::scroll_group(workspace)
        });
        Ok(())
    }
}

/// Built once and leaked, because a manifest outlives everything that reads it
/// and `PluginId` cannot be constructed in a `const`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/blocks").expect("a literal that parses"),
        name: "Blocks",
        description: "The menu a command opens, and what is in it.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}
