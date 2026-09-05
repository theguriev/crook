//! The worktree menu, as one entry of the menu a tab opens.
//!
//! # Why this is a plugin, when the menu it opens is not yet
//!
//! `docs/plugins.md` has had "still to move: the worktree menu" on its list
//! since Phase 1 opened, and this is half of that move rather than the whole
//! of it. What has moved is the *claim*: the worktree menu no longer owns a
//! tab's secondary press, it asks `crook/tabs` for a row like anything else,
//! and it can be switched off on the Plugins page — after which a tab's menu
//! is four entries and nothing anywhere says a fifth is missing. What has not
//! moved is the popup's state and rendering, which are still
//! `workspace::tab_menu`, because that column holds a text field, a background
//! git read and a two-step confirmation, and moving those is a change to how
//! a plugin owns state rather than a change to what owns this menu.
//!
//! Drawing the seam first and moving the code behind it afterwards is the
//! order this repository has used every other time — the usage chip was a
//! plugin owning a struct in the binary before it was a `.wasm` file in a
//! directory — and it is the order that keeps each step reviewable.
//!
//! # One entry, absent rather than dead
//!
//! Outside a git repository there is no row at all. That is the promise the
//! menu made when it was the whole of a tab's secondary press — "a gesture
//! that does nothing where there is nothing to say" — and it survives the move
//! intact, except that now the gesture still opens a menu and it is one row
//! shorter. Which is strictly better: the rule that used to cost a person the
//! entire menu now costs them the entry it was ever about.

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::tab_context_menu::{nothing, submenu_entry};
use crate::workspace::{Workspace, WorkspaceAction, WorktreeAction};

use super::tabs::TAB_MENU_ENTRIES;

/// Where the branch a new worktree is being named lives.
///
/// Named rather than handed over, because the thing that *draws* it is
/// `workspace::tab_menu` — which this plugin does not own yet — and a field is
/// found by name the way an action is.
pub const BRANCH_FIELD: &str = "crook/worktrees/branch";

/// The plugin that puts the worktree menu on a tab.
pub struct Worktrees;

impl Plugin for Worktrees {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // The branch field, which the worktree menu's creator is typed into.
        // The *field* has moved here even though the popup that draws it has
        // not: `Workspace::sync_input_keys` used to name it in source, which
        // was the last field in the window that belonged to a feature rather
        // than to the window. It now goes out with this plugin when it is
        // switched off, like everything else it registers.
        host.claim_field("branch", Workspace::worktree_menu_is_creating);

        // Not a command, and this is the one entry in the menu that is not.
        // The others *do* something and are worth a chord; this one opens a
        // submenu, which is a thing to look at rather than a thing to run, and
        // a palette row that answered "a popup is now open behind the palette"
        // would be a row nobody could use.
        let opens = host.register_action(action("menu"), |workspace, ctx| {
            let Some((tab, _)) = workspace.menu_target() else {
                return;
            };
            workspace.handle_action(
                &WorkspaceAction::Worktree(WorktreeAction::OpenMenu(tab)),
                ctx,
            );
        });

        // Band 3: alone, after the hairline that follows "Close tab". A
        // submenu is its own kind of row and does not belong in a group with
        // things that happen when you press them.
        host.contribute(TAB_MENU_ENTRIES, "menu", 300, move |workspace, app| {
            let open = workspace.worktree_menu_is_open();
            if !open && !workspace.menu_tab_is_in_a_repository(app) {
                return nothing();
            }
            submenu_entry(
                workspace,
                "crook/worktrees/menu",
                "Worktrees",
                open,
                WorkspaceAction::Run(opens),
            )
        });

        Ok(())
    }
}

/// `crook/worktrees/<name>`.
fn action(name: &str) -> ActionName {
    ActionName::parse(&format!("crook/worktrees/{name}")).expect("a name built from a literal")
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/worktrees").expect("a literal that parses"),
        name: "Worktrees",
        description: "One agent, one checkout: the repository's worktrees, on a tab's own menu.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}
