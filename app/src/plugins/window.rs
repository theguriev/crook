//! The window itself: what it can be asked to do, and where a surface may float.
//!
//! # Every one of Crook's own commands, by name
//!
//! These began as variants of a closed enum reachable only by a chord a table
//! in `input_keys` knew about. They are the same handlers — they come back
//! through [`Workspace::command`], so a palette entry and a chord cannot drift
//! apart — with names on them, which is what lets a person bind one in
//! `keybindings.json`, another plugin invoke one, and a palette list them.
//! [`COMMANDS`] is that list, and the shipped keybindings bind it by name like
//! anything else.
//!
//! **Most of them ship with no chord**, and that is the arrangement rather
//! than an omission. A name is what makes something reachable from the
//! keyboard at all — the palette runs it, and the Keyboard Shortcuts page
//! binds it to whatever a person likes — so a shipped chord is only spent on
//! what is pressed often enough to be worth taking a key away from the shell
//! for. See [`crate::keybindings::DEFAULTS_MAC`].
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
use crate::tab::Direction;
use crate::workspace::{BlockEdge, BlockPart};
use crate::workspace::{WindowAction, Workspace, WorkspaceAction};

use crate::plugin::{BuildError, Host, Plugin};

/// Anything that floats over the whole window: a palette, a modal.
pub const WINDOW_OVERLAY: SlotId = SlotId::new("window.overlay");

/// Every command the window has of its own, by name.
///
/// One table, read three ways: `build` registers each of them under its name
/// and its title, [`binding_for`] turns a name back into the thing the
/// workspace does, and the shipped keybindings in
/// [`crate::keybindings`] name them. Three tables would be three places for a
/// command to go missing from — a chord bound to nothing, a palette entry with
/// no key, a name the settings page cannot print a title for.
///
/// The order is the order the shipped keybindings are written in, so a person
/// reading one and the other is reading the same order twice.
pub const COMMANDS: [(&str, &str, Binding); 49] = [
    ("new-tab", "New agent tab", Binding::NewTab),
    ("close-pane", "Close the focused pane", Binding::ClosePane),
    ("split-right", "Split to the right", Binding::SplitRight),
    ("split-down", "Split downwards", Binding::SplitDown),
    ("split-left", "Split to the left", Binding::SplitLeft),
    ("split-up", "Split upwards", Binding::SplitUp),
    ("previous-tab", "Previous tab", Binding::PreviousTab),
    ("next-tab", "Next tab", Binding::NextTab),
    ("move-tab-left", "Move the tab left", Binding::MoveTabLeft),
    (
        "move-tab-right",
        "Move the tab right",
        Binding::MoveTabRight,
    ),
    // Nine and a last, which is every browser's arrangement and the one every
    // terminal copied from them: the ninth key is the end of the list rather
    // than the ninth tab, because a person with twenty tabs pressing it wants
    // the one they just opened.
    (
        "select-tab-1",
        "Select the first tab",
        Binding::SelectTab(0),
    ),
    (
        "select-tab-2",
        "Select the second tab",
        Binding::SelectTab(1),
    ),
    (
        "select-tab-3",
        "Select the third tab",
        Binding::SelectTab(2),
    ),
    (
        "select-tab-4",
        "Select the fourth tab",
        Binding::SelectTab(3),
    ),
    (
        "select-tab-5",
        "Select the fifth tab",
        Binding::SelectTab(4),
    ),
    (
        "select-tab-6",
        "Select the sixth tab",
        Binding::SelectTab(5),
    ),
    (
        "select-tab-7",
        "Select the seventh tab",
        Binding::SelectTab(6),
    ),
    (
        "select-tab-8",
        "Select the eighth tab",
        Binding::SelectTab(7),
    ),
    (
        "select-last-tab",
        "Select the last tab",
        Binding::SelectLastTab,
    ),
    // The four directions a split can be walked, and the two ways round it.
    // A group is one vector along one axis, so two of the four always decline
    // — which is what leaves the other two keys to the shell.
    (
        "focus-pane-left",
        "Focus the pane to the left",
        Binding::FocusPane(Direction::Left),
    ),
    (
        "focus-pane-right",
        "Focus the pane to the right",
        Binding::FocusPane(Direction::Right),
    ),
    (
        "focus-pane-up",
        "Focus the pane above",
        Binding::FocusPane(Direction::Up),
    ),
    (
        "focus-pane-down",
        "Focus the pane below",
        Binding::FocusPane(Direction::Down),
    ),
    (
        "focus-next-pane",
        "Focus the next pane",
        Binding::CyclePane { forward: true },
    ),
    (
        "focus-previous-pane",
        "Focus the previous pane",
        Binding::CyclePane { forward: false },
    ),
    ("grow-pane", "Give the pane more room", Binding::GrowPane),
    (
        "shrink-pane",
        "Give the pane less room",
        Binding::ShrinkPane,
    ),
    ("even-panes", "Even out the split", Binding::EvenPanes),
    ("search-tabs", "Search the tabs", Binding::SearchTabs),
    ("find", "Find in output", Binding::FindInOutput),
    (
        "select-block-up",
        "Select the block above",
        Binding::SelectBlockUp,
    ),
    (
        "select-block-down",
        "Select the block below",
        Binding::SelectBlockDown,
    ),
    // The scrollback, over both surfaces: a list of blocks moves its own
    // offset and a full-screen program's grid moves the emulator's history.
    // One command either way, because a person pressing Page Up is not asking
    // which of the two they are looking at.
    (
        "page-up",
        "Scroll up a screenful",
        Binding::Page { down: false },
    ),
    (
        "page-down",
        "Scroll down a screenful",
        Binding::Page { down: true },
    ),
    (
        "scroll-to-top",
        "Scroll to the oldest output",
        Binding::ScrollToTop,
    ),
    (
        "scroll-to-bottom",
        "Scroll to the newest output",
        Binding::ScrollToBottom,
    ),
    // Everything the menu on a block offers, as a name each. They act on the
    // block the menu is up on, or — with no menu — on the one the keyboard has
    // selected, so each of these is a chord as well as a row.
    (
        "copy-block",
        "Copy the block",
        Binding::CopyBlock(BlockPart::Whole),
    ),
    (
        "copy-block-command",
        "Copy the block's command",
        Binding::CopyBlock(BlockPart::Command),
    ),
    (
        "copy-block-output",
        "Copy the block's output",
        Binding::CopyBlock(BlockPart::Output),
    ),
    (
        "copy-block-directory",
        "Copy the block's working directory",
        Binding::CopyBlock(BlockPart::Directory),
    ),
    (
        "copy-block-branch",
        "Copy the block's branch",
        Binding::CopyBlock(BlockPart::Branch),
    ),
    (
        "rerun-block",
        "Put the block's command back in the composer",
        Binding::RerunBlock,
    ),
    (
        "open-block-menu",
        "Open the selected block's menu",
        Binding::OpenBlockMenu,
    ),
    (
        "scroll-to-block-top",
        "Bring the block's first line to the top",
        Binding::ScrollToBlock(BlockEdge::Top),
    ),
    (
        "scroll-to-block-bottom",
        "Bring the block's last line to the bottom",
        Binding::ScrollToBlock(BlockEdge::Bottom),
    ),
    ("open-settings", "Settings", Binding::OpenSettings),
    ("zoom-in", "Make the text bigger", Binding::ZoomIn),
    ("zoom-out", "Make the text smaller", Binding::ZoomOut),
    ("zoom-reset", "Reset the text size", Binding::ZoomReset),
];

/// What one of the window's own commands does, by name.
///
/// `None` for every other name, which is every plugin's: those are dispatched
/// through the host like any other action. This exists because the window's
/// commands can decline — closing a pane when there is no focused one — and a
/// chord that declines goes on to the shell, which a handler dispatched
/// through the host has no way to say.
pub fn binding_for(name: &ActionName) -> Option<Binding> {
    let (owner, rest) = name.as_str().rsplit_once('/')?;
    if owner != "crook/window" {
        return None;
    }
    COMMANDS
        .iter()
        .find(|(command, _, _)| *command == rest)
        .map(|(_, _, binding)| *binding)
}

/// The plugin that owns the window's own commands.
pub struct Window;

impl Plugin for Window {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::AppWindow)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.declare_slot(WINDOW_OVERLAY, Cardinality::List);
        // The sidebar is the window's, so the slot its sections go in is too.
        host.declare_sidebar_slot();

        for (name, title, binding) in COMMANDS {
            host.register_command(
                action(name),
                title,
                dispatches(move |workspace| workspace.command(binding)),
            );
        }

        // The three that are not bindings, because a window is not a view and
        // there is no chord for them. Now that the title bar draws no buttons,
        // these commands and the desktop's own shortcuts are the whole of how
        // a window is minimised, maximised and closed by hand.
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
        capabilities: &[],
    })
}
