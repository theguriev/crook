//! The window itself: what it can be asked to do, and where a surface may float.
//!
//! # Every one of Crook's own commands, by name
//!
//! Thirteen of these were variants of a closed enum reachable only by a chord
//! this file's `input_keys` table knew about. They are the same thirteen — the
//! handlers come back through [`Workspace::command`], so a palette entry and a
//! chord cannot drift apart — with names on them, which is what lets a person
//! bind one in `keymap.json`, another plugin invoke one, and a palette list
//! them.
//!
//! They are registered as *commands* rather than as bare actions: each has a
//! title, because a list of `crook/window/move-tab-left` is not a list a
//! person reads.
//!
//! # The overlay slot
//!
//! [`WINDOW_OVERLAY`] is where anything that floats over the whole window
//! goes. A `List`, not a `Single`: two plugins may each have a surface, and
//! only one of them will be showing at a time — a contribution that has
//! nothing to say returns [`Empty`] and costs a layout of nothing.
//!
//! It is declared here rather than in `workspace::view` for the reason every
//! slot is declared where it is: the plugin that owns a place in the interface
//! is the one that can say what belongs there, and "the whole window" is this
//! one's.

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Cardinality, Manifest, PluginId, SlotId, Tier};

use crate::input_keys::Binding;
use crate::workspace::{WindowAction, Workspace, WorkspaceAction};

use crate::plugin::{BuildError, Host, Plugin};

/// Anything that floats over the whole window: a palette, a modal.
pub const WINDOW_OVERLAY: SlotId = SlotId::new("window.overlay");

/// The plugin that owns the window's own commands.
pub struct Window;

impl Plugin for Window {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.declare_slot(WINDOW_OVERLAY, Cardinality::List);

        // The order is the order the settings page's Keys section lists the
        // bindings in, so a person reading one and the other is reading the
        // same order twice.
        for (name, title, binding) in [
            ("new-tab", "New agent tab", Binding::NewTab),
            ("close-pane", "Close the focused pane", Binding::ClosePane),
            ("split-right", "Split to the right", Binding::SplitRight),
            ("split-down", "Split downwards", Binding::SplitDown),
            ("previous-tab", "Previous tab", Binding::PreviousTab),
            ("next-tab", "Next tab", Binding::NextTab),
            ("move-tab-left", "Move the tab left", Binding::MoveTabLeft),
            (
                "move-tab-right",
                "Move the tab right",
                Binding::MoveTabRight,
            ),
            ("open-settings", "Settings", Binding::OpenSettings),
            ("zoom-in", "Make the text bigger", Binding::ZoomIn),
            ("zoom-out", "Make the text smaller", Binding::ZoomOut),
            ("zoom-reset", "Reset the text size", Binding::ZoomReset),
        ] {
            host.register_command(
                action(name),
                title,
                dispatches(move |workspace| workspace.command(binding)),
            );
        }

        // The three that are not bindings, because a window is not a view and
        // there is no chord for them: they are what the title bar's buttons
        // send.
        for (name, title, action_value) in [
            ("minimise", "Minimise the window", WindowAction::Minimize),
            (
                "toggle-maximised",
                "Maximise the window",
                WindowAction::ToggleMaximized,
            ),
            ("close-window", "Close the window", WindowAction::Close),
        ] {
            host.register_command(
                action(name),
                title,
                dispatches(move |_| Some(WorkspaceAction::Window(action_value))),
            );
        }

        Ok(())
    }
}

/// `crook/window/<name>`.
fn action(name: &str) -> ActionName {
    ActionName::parse(&format!("crook/window/{name}")).expect("a name built from a literal")
}

/// A handler that works out an action and hands it to the workspace.
///
/// Through [`Workspace::handle_action`] rather than through the method behind
/// whichever arm it lands in, so that everything a chord does on the way — the
/// menu closing, the quit on the last pane — happens here too.
fn dispatches(
    what: impl Fn(&Workspace) -> Option<WorkspaceAction> + 'static,
) -> impl Fn(&mut Workspace, &mut ViewContext<Workspace>) + 'static {
    move |workspace, ctx| {
        if let Some(action) = what(workspace) {
            workspace.handle_action(&action, ctx);
        }
    }
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/window").expect("a literal that parses"),
        name: "Window commands",
        description: "Tabs, panes, splits, the layout and the text size, each reachable by name.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
    })
}
