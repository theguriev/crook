//! The Changes column, as a command and an entry of a tab's menu.
//!
//! The column itself — its state, its reads, its rows — is
//! `workspace::changes_panel`, drawn by the workspace for the reason every
//! surface is: an element tree is native work, and the column is composed
//! beside the work where the Themes panel is. What is here is how a person
//! reaches it: `crook/changes/toggle`, which the palette lists and a chord
//! can be bound to, and a row on a tab's menu that runs the same command.
//! Switching this plugin off takes both away and leaves the column
//! unreachable, which is what switching a feature off has to mean.

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::plugins::tabs::TAB_MENU_ENTRIES;
use crate::tab::TabAction;
use crate::workspace::tab_context_menu::entry;
use crate::workspace::{Workspace, WorkspaceAction};

/// Where the entry goes in a tab's menu: band 2, after the three copies.
/// They are what the tab *says* — its title, its directory, its branch — and
/// this is the rest of that sentence: what it did.
const MENU_ORDER: i32 = 250;

/// The plugin that puts the Changes column within reach.
pub struct Changes;

impl Plugin for Changes {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Diff)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // One command for both directions, the way "Show or hide the tabs
        // panel" is one: a person pressing it is not looking at the column to
        // know which of the two they need.
        let toggles = host.register_command(action("toggle"), "Show changes", |workspace, ctx| {
            // From a tab's menu the column is about the tab the menu was
            // opened on, which need not be the one in front: it is brought
            // forward first, so the column and the pane it describes are on
            // screen together. From the palette or a chord this is the
            // focused pane already, and focusing it again changes nothing.
            let target = workspace.menu_target();
            workspace.close_tab_context_menu(ctx);
            if !workspace.is_changes_panel_open()
                && let Some((_, pane)) = target
            {
                workspace.handle_action(&WorkspaceAction::Tab(TabAction::FocusPane(pane)), ctx);
            }
            workspace.toggle_changes_panel(ctx);
        });

        // Only in a repository, like the worktrees' entry: outside one there
        // is nothing for the column to say, and a row that opened a column
        // saying so would be a row offering nothing.
        host.contribute(
            TAB_MENU_ENTRIES,
            "toggle",
            MENU_ORDER,
            move |workspace, app| {
                if !workspace.menu_tab_is_in_a_repository(app) {
                    return None;
                }
                // The row says what pressing it will do, as "Pin tab" does.
                let label = if workspace.is_changes_panel_open() {
                    "Hide changes"
                } else {
                    "Show changes"
                };
                Some(entry(
                    workspace,
                    "crook/changes/toggle",
                    label,
                    WorkspaceAction::Run(toggles),
                ))
            },
        );

        Ok(())
    }
}

/// `crook/changes/<name>`.
fn action(name: &str) -> ActionName {
    ActionName::parse(&format!("crook/changes/{name}")).expect("a name built from a literal")
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/changes").expect("a literal that parses"),
        name: "Changes",
        description: "What the agent in a tab changed: its commits, its files and their diffs, \
                      read-only, in a column beside it.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}
