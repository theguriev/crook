//! The box above the tabs, and what it hides.
//!
//! Telegram's shape and Telegram's rules, which is what was asked for. The
//! field is **its own full-width row under the control bar** rather than a
//! control inside it: that bar is also the window's title bar — it is the
//! top-left corner, it carries the traffic lights on macOS and it is what a
//! person picks the window up by — and a field wide enough to type a path into
//! would leave the gear, the `+` and the drag surface fighting over 248
//! pixels. Telegram makes the same call for the same reason: title row, then
//! search, then the list.
//!
//! # What it does to the list
//!
//! It filters it, and filters *nothing else*. The strip is untouched: the
//! active tab stays active, `cmd-alt-left/right` still walks every tab there
//! is, and a filtered-out tab goes on printing into its pane. This is a person
//! finding a row in a column of forty, not a mode the window enters — which is
//! also why the query is thrown away the moment it has been used.
//!
//! # Three keys, and why they are not the field's
//!
//! [`TextField`] answers Escape by emptying itself and Enter by doing nothing,
//! which is right for the settings rail: that field is the only thing on its
//! screen that takes a key, so there is nowhere for the keyboard to go back
//! to. Here there is — the shell in the pane behind it — and a box that keeps
//! the keyboard after a person has finished with it is a box that eats the
//! next command they type. So both keys are claimed by
//! [`Workspace::action_for`](super::super::view::Workspace::action_for),
//! before the tree sees them:
//!
//! - **Escape** clears the query *and* hands the keyboard back to the pane, in
//!   one press rather than two. The filter cannot be left behind, because the
//!   thing it filters is the list a person navigates by.
//! - **Enter** selects the top match and then does exactly what Escape does.
//!   Telegram's rule, and the one that makes the box worth a chord: type three
//!   letters, press Enter, and you are in that tab with the keyboard back in
//!   the shell.
//! - **cmd+k**, `ctrl+shift+k` off macOS, is what puts the keyboard *in* the
//!   box, from anywhere in the window — including from another section of the
//!   sidebar, which it switches away from first. See
//!   [`Binding::SearchTabs`](crate::input_keys::Binding::SearchTabs).
//!
//! # What "matches" means
//!
//! [`Query`] and [`Words`], which is the settings page's search — the same
//! substring rule over the same shape of row, so the two boxes in this window
//! cannot come to disagree about what typing two words means. What a tab
//! offers it is what its rows print: the session's title, the working
//! directory as the row abbreviates it, the branch, and the name of the tab
//! the row sits under.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::prelude::*;

use crate::git::{self, Head};
use crate::input_keys::Platform;
use crate::settings::Granularity;
use crate::tab::{Pane, PaneId, Tab, TabId};
use crate::text_input::TextInput;

use super::super::action::{SearchAction, WorkspaceAction};
use super::super::settings_page::search::{Query, Words};
use super::super::text_field::TextField;
use super::super::view::Workspace;

/// The inset around the field: the control bar's own, and a gap under it.
const PADDING: f32 = 8.;

/// The gap between the box and the first row of the list.
const BOTTOM_GAP: f32 = 6.;

/// What the box says while nothing has been typed.
///
/// The chord is *in* the placeholder, which is Telegram's "Search (⌘K)" and is
/// the only place a chord for this can be discovered: the box has no label and
/// the panel has no menu. Spelled the way every other chord in Crook is
/// spelled — the notation `keybindings.json` uses, which is the one the
/// Keyboard Shortcuts page prints — rather than in Mac symbols, and spelled
/// per platform because the two keymaps are not one chord with a modifier
/// swapped.
fn placeholder() -> &'static str {
    match Platform::current() {
        Platform::Mac => "Search (cmd+k)",
        Platform::Other => "Search (ctrl+shift+k)",
    }
}

/// The box: what has been typed into it, and whether it is being typed into.
///
/// Not one of the workspace's per-section
/// [`fields`](super::super::view::Workspace::field), and that is the whole
/// difference between this box and the settings rail's. A section's field has
/// the keyboard because its section is showing — there is no pane on screen to
/// compete with. This one shares its screen with a shell, so it takes the
/// keyboard only when a person puts it there and gives it back at the first
/// opportunity.
#[derive(Default)]
pub(crate) struct SearchState {
    /// What has been typed, with its own caret, selection and undo stack.
    input: TextInput,
    /// What the mouse is doing to the box.
    hover: MouseStateHandle,
    /// Whether the keyboard is the box's rather than the pane's.
    ///
    /// A wish rather than the answer: whether it *actually* has the keyboard
    /// is [`Workspace::search_takes_keys`](super::super::view::Workspace), which
    /// also asks whether the box is on screen at all.
    focused: bool,
}

impl SearchState {
    /// The editor behind the box.
    pub(crate) fn input(&self) -> &TextInput {
        &self.input
    }

    /// What has been typed, ready to match against.
    pub(crate) fn query(&self) -> Query {
        let editor = self.input.editor();
        Query::new(editor.text())
    }

    /// Whether nothing has been typed.
    pub(crate) fn is_empty(&self) -> bool {
        self.input.editor().is_empty()
    }

    /// Whether the keyboard has been put in the box.
    pub(crate) fn is_focused(&self) -> bool {
        self.focused
    }

    /// Puts it there, or takes it away.
    pub(crate) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Empties the box, and forgets that the pointer was ever on it.
    ///
    /// Both halves: the box is about to stop being drawn or stop being typed
    /// into, and a control that came back believing it was hovered would be
    /// outlined with the pointer somewhere else entirely.
    pub(crate) fn clear(&mut self) {
        self.input.edit(crate::editor::Editor::clear);
        self.hover.lock().reset_interaction_state();
    }
}

/// The box itself, between the control bar and the list.
pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let state = workspace.panel_search();

    Container::new(
        TextField::new(
            state.input.clone(),
            workspace.clipboard().clone(),
            workspace.fonts(),
            state.hover.clone(),
            placeholder(),
        )
        .with_icon(Lucide::Search)
        // A press is the other way into the box, and it says only that: what a
        // press means is the workspace's, exactly as it is for every field a
        // section brings with it.
        .with_focus(WorkspaceAction::Search(SearchAction::Focus))
        .finish(),
    )
    .with_padding(Padding {
        top: 0.,
        left: PADDING,
        bottom: BOTTOM_GAP,
        right: PADDING,
    })
    .finish()
}

/// Whether the row for `pane` under `tab` survives the filter.
///
/// The granularity decides *what a row stands for*, so it has to decide what a
/// row is searched by. A `Tabs` row stands for the whole tab and lists only
/// its focused pane, so it is kept when **any** of the tab's panes matches —
/// otherwise searching for a command running in the other half of a split
/// would empty the list of the tab that is running it. A `Panes` row stands
/// for one pane and is kept on its own words, plus the name of the tab it sits
/// under: searching a tab's name keeps all of its rows, which is what a person
/// who typed it meant.
pub(super) fn keeps(
    workspace: &Workspace,
    app: &AppContext,
    query: &Query,
    granularity: Granularity,
    tab: TabId,
    pane: PaneId,
) -> bool {
    if query.is_empty() {
        return true;
    }
    let Some(data) = workspace.tabs().get(tab) else {
        return false;
    };

    match granularity {
        Granularity::Tabs => data
            .panes()
            .iter()
            .any(|pane| matches(workspace, app, query, data, pane)),
        Granularity::Panes => data
            .panes()
            .get(pane)
            .is_some_and(|pane| matches(workspace, app, query, data, pane)),
    }
}

/// The tab Enter opens: the first one with anything in it that matches.
///
/// In the strip's order rather than the list's, and they are the same order —
/// the list is the strip filtered, never reordered, so "the top match" is a
/// thing a person can point at.
///
/// Granularity-free on purpose: which rows a filtered list happens to be
/// showing is a fact about a setting, and Enter has to mean the same thing in
/// both. A tab with a match in any of its panes is a tab worth opening.
pub(crate) fn first_match(workspace: &Workspace, app: &AppContext) -> Option<TabId> {
    let query = workspace.panel_search().query();
    if query.is_empty() {
        return None;
    }

    workspace
        .tabs()
        .iter()
        .find(|tab| {
            tab.panes()
                .iter()
                .any(|pane| matches(workspace, app, &query, tab, pane))
        })
        .map(Tab::id)
}

/// Whether one pane answers the query.
///
/// The words are the row's own, resolved the way the row resolves them: the
/// path is abbreviated against the home directory, because `~/Work/crook` is
/// what is printed and therefore what somebody types. It is *not* cut to the
/// row's width — a search that could not find a directory because the row was
/// too narrow to print its middle would be a search nobody trusts twice.
fn matches(workspace: &Workspace, app: &AppContext, query: &Query, tab: &Tab, pane: &Pane) -> bool {
    let session = pane.session();
    let mut words = Words::new(session.display_title());

    if let Some(directory) = session.working_directory.as_deref() {
        words = words.with_description(git::user_friendly_path(directory, workspace.home()));
    }
    // A keyword rather than a line of its own, because it is not a line of its
    // own: a row prints the branch only under one "Pane title as", and a row
    // that cannot be found by the branch it is standing on would be a surprise
    // in the other two.
    let branch = workspace
        .git_facts(session, app)
        .and_then(|facts| facts.branch.as_ref())
        .map(Head::label);
    if let Some(branch) = branch {
        words = words.with_keywords(&[branch]);
    }

    query.matches(&words, &[tab.name()])
}
