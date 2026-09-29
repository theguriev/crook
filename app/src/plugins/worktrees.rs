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
use crate::workspace::tab_context_menu::submenu_entry;
use crate::workspace::{Workspace, WorkspaceAction, WorktreeAction};

use super::tabs::TAB_MENU_ENTRIES;

/// Where the branch a new worktree is being named lives.
///
/// Named rather than handed over, because the thing that *draws* it is
/// `workspace::tab_menu` — which this plugin does not own yet — and a field is
/// found by name the way an action is.
pub const BRANCH_FIELD: &str = "crook/worktrees/branch";

/// Where the prompt a new worktree's agent is started on is typed, found by
/// name for the branch field's reason.
pub const PROMPT_FIELD: &str = "crook/worktrees/prompt";

/// The plugin that puts the worktree menu on a tab.
pub struct Worktrees;

impl Plugin for Worktrees {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::GitBranch)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // The branch field, which the worktree menu's creator is typed into.
        // The *field* has moved here even though the popup that draws it has
        // not: `Workspace::sync_input_keys` used to name it in source, which
        // was the last field in the window that belonged to a feature rather
        // than to the window. It now goes out with this plugin when it is
        // switched off, like everything else it registers.
        host.claim_field("branch", Workspace::worktree_branch_has_keys);
        // The creator's second field, which is there once an agent is picked.
        // At most one of the two has the keyboard, and the creator says which:
        // a press on a field, or Tab. Neither does while the arrows walk the
        // agents with no prompt to type into.
        host.claim_field("prompt", Workspace::worktree_prompt_has_keys);

        // A command like every other entry, and it was not always: the
        // objection was that a palette row which opened a submenu would be
        // opening it *behind* the palette. It would not — the palette takes
        // itself down before it runs anything, on purpose — and until this was
        // a command the worktree list was the one part of the window nothing
        // but a secondary press could reach.
        // "Worktree list" rather than the plugin's own name: with Finish and
        // Discard beside it the plugin has a heading of its own on the
        // Keyboard Shortcuts page, and a row reading "Worktrees" under a
        // heading reading "Worktrees" says one thing twice.
        let opens = host.register_command(action("menu"), "Worktree list", |workspace, ctx| {
            let Some((tab, _)) = workspace.menu_target() else {
                return;
            };
            workspace.handle_action(
                &WorkspaceAction::Worktree(WorktreeAction::OpenMenu(tab)),
                ctx,
            );
        });

        // The same creator, opened on the task rather than on the list: the
        // first agent found picked and the keyboard in its prompt, so that a
        // task is a sentence and Enter. On the tab the palette would act on,
        // which is the one a person is looking at. No chord ships for it —
        // a chord is a key taken from every shell — and the README suggests
        // one for the people who start many.
        host.register_command(action("new-task"), "New task…", |workspace, ctx| {
            let Some((tab, _)) = workspace.menu_target() else {
                return;
            };
            workspace.handle_action(
                &WorkspaceAction::Worktree(WorktreeAction::NewTask(tab)),
                ctx,
            );
        });

        // Band 5: alone, after the hairline that follows "Close tab". A
        // submenu is its own kind of row and does not belong in a group with
        // things that happen when you press them.
        host.contribute(TAB_MENU_ENTRIES, "menu", 500, move |workspace, app| {
            // Not lit while the menu is asking about a task: that question
            // hangs where the list would, and it is the task's row that
            // opened it.
            let open =
                workspace.worktree_menu_is_open() && workspace.worktree_menu_task().is_none();
            if !open && !workspace.menu_tab_is_in_a_repository(app) {
                return None;
            }
            Some(submenu_entry(
                workspace,
                "crook/worktrees/menu",
                "Worktrees",
                open,
                WorkspaceAction::Run(opens),
            ))
        });

        // The end of a task, on the tab doing it: Finish, which deletes the
        // branch only when its work is proved to be on the base, and Discard,
        // which deletes it anyway after a second question naming what goes.
        // Offered only in a checkout Crook made — one somebody made by hand
        // elsewhere is theirs to finish — and in the same band as the list,
        // because each opens a question where the list would hang.
        for (name, title, discard, order) in [
            ("finish-task", "Finish task", false, 510),
            ("discard-task", "Discard task", true, 520),
        ] {
            let asks = host.register_command(action(name), title, move |workspace, ctx| {
                let Some((_, pane)) = workspace.menu_target() else {
                    return;
                };
                workspace.handle_action(
                    &WorkspaceAction::Worktree(WorktreeAction::AskFinish { pane, discard }),
                    ctx,
                );
            });
            host.contribute(TAB_MENU_ENTRIES, name, order, move |workspace, _| {
                let open = workspace.worktree_menu_task() == Some(discard);
                if !open && !workspace.menu_pane_is_in_a_crook_checkout() {
                    return None;
                }
                Some(submenu_entry(
                    workspace,
                    &format!("crook/worktrees/{name}"),
                    title,
                    open,
                    WorkspaceAction::Run(asks),
                ))
            });
        }

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
