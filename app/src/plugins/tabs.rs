//! The tabs, and the menu their secondary press opens.
//!
//! This plugin owns [`TAB_MENU_ENTRIES`] — the place a tab's context menu is
//! made of — and contributes the entries that are about a tab as such: putting
//! it in a group, copying what it says, closing it. The popup those rows land
//! in is drawn by [`workspace::tab_context_menu`](crate::workspace), for the
//! reason every surface in this application is drawn by the workspace: an
//! element tree is native work. What is owned *here* is the slot, and owning
//! it is what makes a tab's menu something a stranger's plugin can put a row
//! in — the same move `crook/header` made for the header's right-hand side.
//!
//! # Why these entries are commands
//!
//! Every one of them is registered with
//! [`register_command`](crate::plugin::Host::register_command) rather than
//! wired straight to a click, so the palette lists them and a person can bind
//! a chord to one. That is not generosity; it is what an entry *is* once the
//! menu is a slot. A row that dispatched a `WorkspaceAction` of its own would
//! be reachable by exactly one gesture, and the enum would grow an arm per row
//! — which is the arrangement `WorkspaceAction::Run` was built to end.
//!
//! # What a command acts on when no menu is up
//!
//! [`Workspace::menu_target`](crate::workspace::Workspace::menu_target): the
//! tab and pane the menu was opened over, and failing that the active tab's
//! focused pane. A command reached from the palette or from a chord therefore
//! means the tab a person is looking at, which is the only thing it could
//! sensibly mean, and the same handler serves both without a branch.

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Cardinality, Manifest, PluginId, SlotId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::tab::TabAction;
use crate::workspace::tab_context_menu::{entry, inert_entry};
use crate::workspace::{Workspace, WorkspaceAction};

/// Every row in the menu a tab's secondary press opens.
///
/// A list rather than a single, and ordered in bands of
/// [`BAND`](crate::workspace::tab_context_menu::BAND) — see that module for
/// what a band buys and why it is a division of `order` rather than a field.
pub const TAB_MENU_ENTRIES: SlotId = SlotId::new("tab.menu.entries");

/// The plugin that owns the tabs' menu.
pub struct Tabs;

impl Plugin for Tabs {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.declare_slot(TAB_MENU_ENTRIES, Cardinality::List);

        host.register_command(
            action("new-group-with-tab"),
            "New group with tab",
            |workspace, ctx| {
                let Some((tab, _)) = workspace.menu_target() else {
                    return;
                };
                workspace.close_tab_context_menu(ctx);
                workspace.handle_action(&WorkspaceAction::Tab(TabAction::NewInGroupOf(tab)), ctx);
            },
        );

        host.register_command(
            action("copy-pane-title"),
            "Copy pane title",
            |workspace, ctx| {
                let Some(text) = workspace.menu_pane_title() else {
                    return;
                };
                workspace.close_tab_context_menu(ctx);
                workspace.clipboard().write(&text);
            },
        );

        host.register_command(
            action("copy-working-directory"),
            "Copy working directory",
            |workspace, ctx| {
                let Some(directory) = workspace.menu_pane_directory() else {
                    return;
                };
                workspace.close_tab_context_menu(ctx);
                workspace.clipboard().write(&directory.to_string_lossy());
            },
        );

        host.register_command(action("close-tab"), "Close tab", |workspace, ctx| {
            let Some((tab, _)) = workspace.menu_target() else {
                return;
            };
            workspace.close_tab_context_menu(ctx);
            workspace.handle_action(&WorkspaceAction::Tab(TabAction::Close(tab)), ctx);
        });

        // Band 0: what this tab is *with*. Band 1: what it says, as text.
        // Band 2: it goes away. The gap between 2 and the worktree menu's 3 is
        // deliberate — a destructive entry sits alone, so the pointer on its
        // way down the column has a hairline to stop at before it.
        contribute(host, "new-group-with-tab", 0, |workspace| {
            workspace.menu_target().is_some()
        });
        contribute(host, "copy-pane-title", 100, |workspace| {
            workspace.menu_pane_title().is_some()
        });
        contribute(host, "copy-working-directory", 101, |workspace| {
            workspace.menu_pane_directory().is_some()
        });
        contribute(host, "close-tab", 200, |workspace| {
            workspace.menu_target().is_some()
        });

        Ok(())
    }
}

/// Contributes one entry that runs this plugin's action of the same name.
///
/// The label is the command's own title, read back off the host rather than
/// written twice: the palette and the menu say the same words about the same
/// action because there is one place the words are.
///
/// `live` decides whether the row can be pressed. These four are about the tab
/// the menu is on and a tab always has them, so a false answer draws an *inert*
/// row rather than none — a directory the shell has not reported yet is a copy
/// that cannot happen this frame, not a menu with a hole in it.
fn contribute(
    host: &mut Host,
    name: &'static str,
    order: i32,
    live: impl Fn(&Workspace) -> bool + 'static,
) {
    let label = host.title_of(&action(name)).unwrap_or(name).to_owned();
    let id = host.action(&action(name));
    let key = format!("crook/tabs/{name}");

    host.contribute(TAB_MENU_ENTRIES, name, order, move |workspace, _| {
        let Some(id) = id.filter(|_| live(workspace)) else {
            return inert_entry(workspace, &key, label.clone());
        };
        entry(workspace, &key, label.clone(), WorkspaceAction::Run(id))
    });
}

/// `crook/tabs/<name>`.
fn action(name: &str) -> ActionName {
    ActionName::parse(&format!("crook/tabs/{name}")).expect("a name built from a literal")
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/tabs").expect("a literal that parses"),
        name: "Tabs",
        description: "The list of what is being worked on, and the menu a tab opens.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}
