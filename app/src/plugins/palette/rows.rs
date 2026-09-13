//! What the palette is showing, and where the keyboard may be in it.
//!
//! # One box, five lists
//!
//! The palette is one surface with five lists in it, and which one it is
//! showing is worked out from the query rather than held in a flag — see
//! [`Mode`]. The default is all of them at once: what there is to run, the
//! tabs that are open, and the rows of the settings, each under a heading of
//! its own. That is the whole point of the surface — somebody who knows one
//! chord for "find something" should not have to know first which of three
//! registries the thing they want lives in — and a sigil at the front narrows
//! it back down to one list when they do know: `>` for the commands, `@` for
//! the tabs, `#` for the settings, `?` for the keys.
//!
//! Keys mode is the keymap. It holds the commands grouped by the plugin that
//! registered them, then every key bound to something the command list does
//! not hold, then the keys the program in a pane answers to — because
//! «на какую кнопку забиндено» is a question about the keyboard, and a list
//! that answered it for commands only would be silent about `ctrl+c`.
//!
//! # A heading is drawn only when there is something to tell apart
//!
//! In the everything list a section's heading appears when **more than one**
//! section has rows in it. A query that only commands answer is therefore the
//! flat, title-sorted launcher that shipped, in a card three rows taller; the
//! headings arrive at the moment they are carrying information, which is when
//! a person has to know why a row about a directory is sitting under a row
//! about a command. A lone heading over the only list there is would be a
//! fence, which is the same argument [`GROUP_MINIMUM`] makes about a plugin
//! with one command.
//!
//! # A row that is not a command is still run by name
//!
//! A tab and a settings row have no command of their own, so they carry a
//! [`Target`]: an action, and what is said to it before it runs. That is the
//! idiom the rest of the window already uses to hand an argumentless action
//! its subject — [`Host::say`](crate::plugin::Host::say) — and using it here
//! is what keeps Enter, a click and a chord one dispatch path rather than
//! three. The palette owns those two actions for the same reason it owns its
//! own arrows: they are what a row of *this* surface does, and a list whose
//! first rows were "go to a tab (which one?)" is a list nobody reads.
//!
//! # One index space
//!
//! There is one index space and it is the position in the flattened display
//! list: the selection is a place in [`Rows::rows`], the renderer's counter is
//! a place in it, a hover handle is keyed on a place in it, and the scroll
//! offset is a sum over it. The bug this shape exists to make impossible is
//! the other arrangement — a selection counted in commands and a list drawn in
//! rows, where running the selected row is off by the number of headings above
//! it and runs the wrong thing with no symptom at all.
//!
//! Only [`Row::Command`] can be landed on. A heading is not a row, and neither
//! is a [`Row::Fact`]: Enter must mean one thing, and on a key a pane eats
//! there is nothing for it to mean.
//!
//! # A row's height is a function of its kind
//!
//! That is the invariant to keep. [`scroll_into_view`] is a prefix sum over
//! [`height`], and `ScrollState` carries no per-row rectangles to correct it
//! with — the day a title wraps or a row grows a second line the arithmetic
//! dies. That is why a title clips rather than wraps.

use std::collections::HashMap;

use crookui_core::prelude::AppContext;

use crook_plugin::{ActionName, PluginId};

use crate::git;
use crate::keybindings::Rule;
use crate::plugin::{ActionId, Host};
use crate::plugins::shortcuts::{self, NOT_BOUND, PANE_KEYS, PaneKey};
use crate::tab::Tab;
use crate::workspace::Workspace;
use crate::workspace::settings_page::search::Query;
use crate::workspace::tabs_panel::search as tab_search;

use super::state::{Goto, Palette};
use super::{chord, list};

/// How many commands a plugin registers before its name is worth a heading.
///
/// Seven plugins register exactly one command each — the worktrees menu, the
/// store, this plugin, and the four `Settings: <page>` commands every settings
/// page gets for free. Seven 28px headings over seven rows is a fence rather
/// than a list, and one of them would read `Command palette` directly above a
/// row reading `Command palette`.
const GROUP_MINIMUM: usize = 3;

/// The heading a plugin too small for one of its own goes under.
const ELSEWHERE: &str = "Elsewhere";

/// The heading over keys bound to something that is not a command.
const OTHER_BINDINGS: &str = "Other bindings";

/// The heading over the keys the program in the pane answers to.
const IN_A_PANE: &str = "In a pane";

/// The heading over what there is to run, in the list that holds more.
const COMMANDS: &str = "Commands";

/// The heading over the tabs that are open.
const TABS: &str = "Tabs";

/// The heading over the rows of the settings.
const SETTINGS: &str = "Settings";

/// What the row for the tab a person is already in says.
///
/// The one fact that makes a list of tabs worth reading rather than counting:
/// where you are in it. Printed as a note, in the muted colour a note is
/// printed in, because it is something about the row rather than a column of
/// its own — the same slot `nothing answers to this` uses.
const HERE: &str = "you are here";

/// What a binding naming a command this machine does not have says.
///
/// The one thing a keybindings file cannot tell the person who wrote it: the
/// line is fine and the plugin it names is not installed.
const NOTHING_ANSWERS: &str = "nothing answers to this";

/// Which of the lists the query asks for, and what is left to search with.
///
/// Worked out from the query every frame rather than held in a `Cell`, because
/// a mode kept beside the field is a second source of truth that a paste, an
/// undo or a select-all-delete can put out of step with what a person can see.
/// It also makes the switch a text edit, which is what lets a search survive
/// it: `split`, `>split` and `?split` are one question asked of three lists.
///
/// The sigils are the characters no title, no path and no chord starts with,
/// and each of them is somebody's convention already: `>` is what VSCode's own
/// palette narrows to commands with, `@` is a place, `#` a setting, `?` a
/// question about the keyboard.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    /// Every list at once, under a heading each. What the chord opens.
    Everything,
    /// Only what there is to run, flat and by title. Yesterday's launcher.
    Commands,
    /// Only the tabs that are open, in the order the strip holds them.
    Tabs,
    /// Only the rows of the settings, in the order the rail holds them.
    Settings,
    /// Everything there is to press, grouped by what owns it.
    Keys,
}

impl Mode {
    /// The lists a person can be put in, in the order `tab` walks them.
    ///
    /// The everything list is first because it is what the chord opens. The
    /// **keys are second**, which is the one place this order is not the order
    /// the sections are drawn in, and it is deliberate: the keys are the list
    /// a person turns to *about the row they are looking at* — "and what is
    /// this bound to?" — so it has to be one press from the list they are
    /// looking at, and the row they had walked to has to survive the press.
    /// Two lists of commands side by side is what makes that possible; a tab
    /// or a settings row in between would drop the row on the way through.
    const ORDER: [Self; 5] = [
        Self::Everything,
        Self::Keys,
        Self::Commands,
        Self::Tabs,
        Self::Settings,
    ];

    /// Which list `query` asks for, and what of it is left to search with.
    pub(super) fn of(query: &str) -> (Self, &str) {
        let mut characters = query.chars();
        match characters.next().and_then(Self::asked_by) {
            Some(mode) => (mode, characters.as_str()),
            None => (Self::Everything, query),
        }
    }

    /// The list one character at the front of a query asks for.
    fn asked_by(sigil: char) -> Option<Self> {
        Self::ORDER
            .into_iter()
            .find(|mode| mode.sigil() == Some(sigil))
    }

    /// The character that asks for this list, and none for the one a query
    /// with no sigil on it gets.
    pub(super) fn sigil(self) -> Option<char> {
        match self {
            Self::Everything => None,
            Self::Commands => Some('>'),
            Self::Tabs => Some('@'),
            Self::Settings => Some('#'),
            Self::Keys => Some('?'),
        }
    }

    /// The list `tab` turns the card over to, wrapping at the end.
    ///
    /// A cycle rather than a toggle, and that is the whole of how the sigils
    /// are discovered: pressing it puts the next one *in the box*, where it
    /// can be read, deleted and typed again on purpose. A footer that listed
    /// four sigils would be teaching a syntax; a key that spells one is
    /// showing it.
    pub(super) fn next(self) -> Self {
        let at = Self::ORDER
            .into_iter()
            .position(|mode| mode == self)
            .unwrap_or_default();
        Self::ORDER[(at + 1) % Self::ORDER.len()]
    }

    /// What this list is called, for the line at the bottom of the card.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Everything => "everything",
            Self::Commands => "the commands",
            Self::Tabs => "the tabs",
            Self::Settings => "the settings",
            Self::Keys => "the keys",
        }
    }

    /// `seed` behind this list's sigil, which is what the box then holds.
    pub(super) fn seeded(self, seed: &str) -> String {
        match self.sigil() {
            Some(sigil) => format!("{sigil}{seed}"),
            None => seed.to_owned(),
        }
    }
}

/// One key that reaches something, as the row prints it.
pub(super) struct Chord {
    /// As a person would write it in `keybindings.json`, which is the string
    /// the cap carries and the reason the cap is allowed to be a cap: the
    /// right-hand half of a row exists to be copied into a file, and `⌃⇧P`
    /// would be the palette teaching a syntax that does not parse.
    pub(super) text: String,
    /// The context keys its clause names, ready to print, or `None` for a key
    /// that is always in force — which is `None` the moment *any* rule
    /// spelling this key is unconditional, because that is the one `resolve`
    /// falls back to.
    pub(super) only_when: Option<String>,
}

/// One line about a key: what it does, and every key that does it.
///
/// The action name is `None` for a key a pane answers to, and that is the
/// whole of the difference: a name is what you write in a keybindings file,
/// and a key the program in the pane eats is not something a keybindings file
/// can reach.
pub(super) struct Entry {
    /// The name, printed on the right of the row: the only way a person finds
    /// out what to write in their keybindings file, and what tells two
    /// plugins' "Refresh" apart.
    pub(super) action: Option<ActionName>,
    /// What it is called. For a row under [`OTHER_BINDINGS`] there is no human
    /// name, so this is the action name itself and the row prints no
    /// right-hand column — the same string in both columns is a stutter, not a
    /// column.
    pub(super) title: String,
    /// What is printed on the right instead of an action name: where a tab is
    /// working, or the page and category a settings row lives on.
    ///
    /// A row that is not a command has no name to print — there is no
    /// keybindings file that can reach one tab — and the column is worth more
    /// said than left empty: two tabs called `crook` are told apart by their
    /// directories and by nothing else on the row.
    pub(super) detail: Option<String>,
    /// Every key that reaches it, weakest layer first. Empty is the ordinary
    /// case: 52 window commands stand against 43 shipped rules, most of which
    /// are second spellings of one key.
    pub(super) chords: Vec<Chord>,
    /// What is wrong with this row, when something is — today only
    /// [`NOTHING_ANSWERS`]. Printed muted, after the title.
    pub(super) note: Option<&'static str>,
}

/// What Enter does to a row: an action, and what is said to it first.
///
/// Every row runs a *named action*, whether it is a command somebody
/// registered or a tab that has no name at all, because that is the one
/// dispatch path a chord, a click and Enter can share — see this module's own
/// doc. The subject is [`Host::say`](crate::plugin::Host::say)'s: left for the
/// action to take when it runs, which is safe because a dispatched action is
/// applied after the whole tree has seen the event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    /// What runs.
    pub(super) action: ActionId,
    /// What it is told first, for the actions that act on something.
    pub(super) subject: Option<String>,
}

impl Target {
    /// A row that runs a command, which needs to be told nothing.
    pub(super) fn plain(action: ActionId) -> Self {
        Self {
            action,
            subject: None,
        }
    }

    /// A row that runs an action about something.
    fn about(action: ActionId, subject: impl Into<String>) -> Self {
        Self {
            action,
            subject: Some(subject.into()),
        }
    }
}

/// One line of the list as it is drawn.
pub(super) enum Row {
    /// A heading: the plugin whose commands are under it, or one of the blocks
    /// that belong to no plugin — the fold, the keys bound to something that
    /// is not a command, and the keys a pane eats. Never selectable, never
    /// hoverable, and drawn with no background — so that nothing which is not
    /// a row looks like one, and so that `palette_selected_row` keeps finding
    /// exactly one wide `overlay_2` band per frame.
    Group(String),
    /// Something Enter can run. The only variant the keyboard lands on.
    Command(Entry, Target),
    /// Something that only explains itself: a key a pane eats, or a binding
    /// naming a command this machine does not have. Skipped by the arrows for
    /// the same reason a heading is.
    Fact(Entry),
}

/// Everything the palette is showing, and the only place that knows which of
/// it the keyboard may land on.
///
/// **Invariant:** after the selection has been settled, the selected line is a
/// [`Row::Command`], or there is no line the keyboard can land on and the
/// selection is `0` — the list `?sigint` leaves, all heading and fact.
pub(super) struct Rows {
    mode: Mode,
    rows: Vec<Row>,
}

impl Rows {
    /// What was worked out, under the mode it was worked out for.
    pub(super) fn new(mode: Mode, rows: Vec<Row>) -> Self {
        Self { mode, rows }
    }

    /// Which list this is, for the field's icon and the footer's line.
    pub(super) fn mode(&self) -> Mode {
        self.mode
    }

    /// The lines, in the order they are drawn.
    pub(super) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Whether there is nothing to show, which is what the empty state asks.
    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Whether the keyboard may land on the line at `at`.
    fn is_command(&self, at: usize) -> bool {
        matches!(self.rows.get(at), Some(Row::Command(..)))
    }

    /// What the line at `at` is, if it is a command at all.
    ///
    /// A match, not arithmetic: this is the one call running the selection
    /// makes, and an off-by-headings mistake here would run the wrong command
    /// with no visible symptom.
    pub(super) fn command_at(&self, at: usize) -> Option<(&Entry, &Target)> {
        match self.rows.get(at) {
            Some(Row::Command(entry, target)) => Some((entry, target)),
            _ => None,
        }
    }

    /// The first line the keyboard can land on.
    ///
    /// What a query that has changed puts the selection back to, and not line
    /// zero, which is a heading. This is what keeps "open, type three letters,
    /// press Enter" the same three keystrokes it was before there were
    /// headings.
    pub(super) fn first_command(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Command(..)))
    }

    /// Where a command with this name ended up, for the switch that keeps the
    /// row a person was on.
    pub(super) fn find(&self, action: &ActionName) -> Option<usize> {
        self.rows.iter().position(|row| match row {
            Row::Command(entry, _) => entry.action.as_ref() == Some(action),
            Row::Group(_) | Row::Fact(_) => false,
        })
    }

    /// `by` steps from `at`, wrapping, over the lines that can be landed on.
    ///
    /// Bounded by one pass over the lines, so a list with nothing to land on —
    /// which `?sigint` leaves, a heading and the keys a pane eats and no
    /// command anywhere — comes back `None` rather than not coming back.
    pub(super) fn stepped(&self, at: usize, by: isize) -> Option<usize> {
        let len = self.rows.len();
        if len == 0 {
            return None;
        }

        let mut index = at.min(len - 1) as isize;
        for _ in 0..len {
            index = (index + by).rem_euclid(len as isize);
            if self.is_command(index as usize) {
                return Some(index as usize);
            }
        }
        None
    }

    /// The nearest landable line at or before `at`, and failing that at or
    /// after.
    ///
    /// Backwards first, so a list that shrank under the selection keeps a
    /// person near where they were rather than jumping forward past a heading.
    /// Clamps an out-of-range `at` first, which is the case this exists for.
    pub(super) fn settled(&self, at: usize) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        let at = at.min(self.rows.len() - 1);
        (0..=at)
            .rev()
            .find(|&k| self.is_command(k))
            .or_else(|| (at + 1..self.rows.len()).find(|&k| self.is_command(k)))
    }

    /// How far down the list the line at `at` starts.
    fn top(&self, at: usize) -> f32 {
        self.rows[..at.min(self.rows.len())]
            .iter()
            .map(height)
            .sum()
    }

    /// How much of the heading above `at` to bring into view with it.
    ///
    /// A row at the head of its group is shown with the heading that names it:
    /// arriving at the top of `Tabs` and not being told it is Tabs is arriving
    /// nowhere. A [`Row::Fact`] above the selection reveals nothing extra,
    /// which is right — it is a row, not a label for one.
    fn reveal(&self, at: usize) -> f32 {
        match at.checked_sub(1).and_then(|above| self.rows.get(above)) {
            Some(Row::Group(_)) => list::GROUP_HEIGHT,
            _ => 0.,
        }
    }
}

/// How tall one line is drawn, which is the only thing its offset depends on.
fn height(row: &Row) -> f32 {
    match row {
        Row::Group(_) => list::GROUP_HEIGHT,
        Row::Command(..) | Row::Fact(_) => list::ROW_HEIGHT,
    }
}

/// Keeps the line the keyboard is on inside the list.
///
/// A prefix sum rather than `selected * ROW_HEIGHT`, which is wrong the moment
/// a 28px heading precedes the selection and wrong by more with every heading
/// after that.
///
/// Four assumptions, all of them still true: heights are constants and never
/// measured; the list's content starts at content-y 0, because the
/// `Scrollable`'s child is the row column with no header and no padding of its
/// own; `offset` is last frame's, which is harmless because opening the
/// palette scrolls to the top and a query that changed does the same; and
/// `scroll_to` clamps into `[0, max_offset()]`, which absorbs both a negative
/// target and an overshoot — against last frame's extent, so a switch into the
/// taller list can be held short until the next arrow moves it.
///
/// The height the row has to fit inside of is the smaller of two: the one
/// **this mode** caps the list at, and the one the last layout measured. Not
/// the cap alone — a window too short for the card gives the list less, and
/// a row scrolled into a 408px window that is really a 250px one is below
/// the fold. Not the measurement alone either: `tab` changes the cap and
/// calls this in the same turn, so the measured viewport is still the list
/// that was just left, and scrolling the kept row into a 408px window that
/// is about to be a 306px one leaves it below the fold too. The smaller of
/// the two is right in a tall window and in a short one, and in the one
/// case it is short — the taller list entered from the shorter — it scrolls a
/// row further up than it had to, which the next arrow undoes. Before any
/// layout the measurement is zero and says nothing, so the cap stands alone.
pub(super) fn scroll_into_view(palette: &Palette, rows: &Rows) {
    let at = palette.selected();
    let top = rows.top(at);
    let own = rows.rows().get(at).map_or(list::ROW_HEIGHT, height);
    let reveal = rows.reveal(at);
    let cap = list::list_height(rows.mode());

    let scroll = palette.scroll();
    let mut scroll = scroll.lock();
    let measured = scroll.viewport();
    let viewport = if measured > 0. {
        cap.min(measured)
    } else {
        cap
    };
    let offset = scroll.offset();
    if top - reveal < offset {
        scroll.scroll_to(top - reveal);
    } else if top + own > offset + viewport {
        scroll.scroll_to(top + own - viewport);
    }
}

/// Everything the palette is showing right now, in the order it is drawn.
///
/// Worked out every frame rather than cached, because the things it depends on
/// — the query, which plugins are loaded, what the keybindings say, which tabs
/// are open and what the settings hold — all change without this plugin being
/// told.
///
/// The rules are walked **once** for the whole list rather than once per
/// command: `chords_for` rebuilds `effective()` on every call, and a list that
/// searches chord text has to know every command's keys before it can filter
/// any of them out. That makes this an order of magnitude cheaper than the
/// ~92 `effective()` walks a frame it replaces, which is what pays for chord
/// search.
///
/// Only the lists the mode asks for are built at all. That is what keeps `@`
/// off the settings pages — building those means building their *elements*,
/// which is what the settings rail already pays per keystroke while it is
/// being searched, and there is no reason to pay it for a query that cannot
/// show a settings row.
///
/// Settles the selection against what it worked out, so that the invariant
/// [`Rows`] states — the selected line is a command, or there is nothing to
/// select — holds from the moment the list exists rather than from whenever a
/// caller remembers to ask for it.
pub(super) fn showing(workspace: &Workspace, palette: &Palette, app: &AppContext) -> Rows {
    let host = workspace.host();
    let text = palette.query().editor().text().to_lowercase();
    let (mode, needle) = Mode::of(&text);
    let terms: Vec<&str> = needle.split_whitespace().collect();
    let goto = palette.goto();

    let rows = match mode {
        Mode::Commands => flat(runnable(host, &standing(workspace), &terms)),
        Mode::Tabs => tabs(workspace, app, needle, goto),
        Mode::Settings => settings(workspace, app, needle, goto, true),
        Mode::Keys => keys(workspace, &terms),
        Mode::Everything => everything(workspace, app, needle, &terms, goto),
    };

    let rows = Rows::new(mode, rows);
    // The selection is put back here rather than by each caller, because the
    // ordering is the subtle part and there should be one place that gets it
    // right: `settle` remembers the query it was handed, so the *first* build
    // of a frame is the one that sees a query which changed and every later
    // build sees one that did not.
    palette.settle(&text, &rows);
    rows
}

/// Every rule that stands, gathered by the command it names.
fn standing(workspace: &Workspace) -> HashMap<&ActionName, Vec<&Rule>> {
    let mut by_command: HashMap<&ActionName, Vec<&Rule>> = HashMap::new();
    for rule in workspace.keybindings().standing() {
        by_command.entry(&rule.command).or_default().push(rule);
    }
    by_command
}

/// The commands a query is looking for, and who registered each.
fn runnable<'a>(
    host: &'a Host,
    by_command: &HashMap<&ActionName, Vec<&Rule>>,
    terms: &[&str],
) -> Vec<(&'a PluginId, Entry, Target)> {
    let mut matched: Vec<(&PluginId, Entry, Target)> = Vec::new();
    for (owner, action, title) in host.commands() {
        let chords = chord::collapse(by_command.get(action).map_or(&[][..], Vec::as_slice));
        if !command_matches(title, action, owner, &chords, terms) {
            continue;
        }

        // Only what is registered *now*. A command whose plugin has been
        // disabled since it was registered has no id, and a row that did
        // nothing when it was clicked would be worse than no row.
        let Some(id) = host.action(action) else {
            continue;
        };
        matched.push((
            owner,
            Entry {
                action: Some(action.clone()),
                title: title.clone(),
                detail: None,
                chords,
                note: None,
            },
            Target::plain(id),
        ));
    }
    matched
}

/// The keymap: the commands under the plugin that registered them, then the
/// keys that reach something else, then the keys no keybindings file can
/// reach.
fn keys(workspace: &Workspace, terms: &[&str]) -> Vec<Row> {
    let host = workspace.host();
    let by_command = standing(workspace);

    // How many commands each plugin has *before* any filtering, so that which
    // group a row is under does not move as somebody types.
    let mut sizes: HashMap<&PluginId, usize> = HashMap::new();
    for (owner, _, _) in host.commands() {
        *sizes.entry(owner).or_default() += 1;
    }

    let mut rows = grouped(host, &sizes, runnable(host, &by_command, terms));
    rows.extend(other_bindings(host, &by_command, terms));
    rows.extend(in_a_pane(terms));
    rows
}

/// The everything list: the three lists that are about *doing* something, each
/// under a heading — and under none at all when only one of them answered.
///
/// The commands come first and that is not alphabetical accident: "open it,
/// type three letters, press Enter" has to go on landing on a command, because
/// that is the palette people already have in their fingers. A tab or a
/// settings row is found by typing what it is called, which is a thing nobody
/// does by accident.
fn everything(
    workspace: &Workspace,
    app: &AppContext,
    needle: &str,
    terms: &[&str],
    goto: Goto,
) -> Vec<Row> {
    let host = workspace.host();
    let sections = [
        (COMMANDS, flat(runnable(host, &standing(workspace), terms))),
        (TABS, tabs(workspace, app, needle, goto)),
        // With nothing typed there is no question for a settings row to be the
        // answer to, and forty switches under the tabs would be the palette
        // opening on its own noise. `#` is how somebody asks to see them all.
        (SETTINGS, settings(workspace, app, needle, goto, false)),
    ];

    let headed = sections.iter().filter(|(_, rows)| !rows.is_empty()).count() > 1;

    let mut all: Vec<Row> = Vec::new();
    for (heading, rows) in sections {
        if rows.is_empty() {
            continue;
        }
        if headed {
            all.push(Row::Group(heading.to_owned()));
        }
        all.extend(rows);
    }
    all
}

/// Every open tab a query is looking for, in the order the strip holds them.
///
/// The strip's order and not a sort of this module's own: the panel a person
/// reads all day is that order, and a list of the same tabs in a different one
/// is a list they have to re-read rather than recognise.
///
/// What a tab is *found* by is [`tab_search::whole`], which is the rule the
/// panel's own search box uses — the title, the directory as the row
/// abbreviates it, the branch, and the tab's name.
fn tabs(workspace: &Workspace, app: &AppContext, needle: &str, goto: Goto) -> Vec<Row> {
    let query = Query::new(needle);
    let here = workspace.tabs().active_id();

    workspace
        .tabs()
        .iter()
        .filter(|tab| tab_search::whole(workspace, app, &query, tab))
        .map(|tab| {
            Row::Command(
                Entry {
                    // No name column: no keybindings file can reach one tab,
                    // so there is nothing here for a person to write down.
                    action: None,
                    title: called(tab),
                    detail: where_it_is(workspace, tab),
                    chords: Vec::new(),
                    note: (tab.id() == here).then_some(HERE),
                },
                Target::about(goto.tab, tab.id().as_u64().to_string()),
            )
        })
        .collect()
}

/// What a tab is called, as its row in the panel calls it.
///
/// The focused pane's title and **not** [`Tab::name`], which is the one thing
/// about this row that is copied rather than reasoned about: a tab's name is
/// what it was opened as — `agent 3` — and its row in the panel says what the
/// session in it is *doing*. Two surfaces calling one tab two different things
/// is a person looking for a row they can see and not finding it. The name is
/// searched all the same, because [`tab_search::whole`] counts it as a word of
/// the tab's.
fn called(tab: &Tab) -> String {
    // The focused pane, or the first one when focus names a pane that has
    // gone — which is the fallback `tabs_tab` states for the same row.
    tab.panes()
        .focused()
        .or_else(|| tab.panes().iter().next())
        .map_or_else(
            || tab.name().to_owned(),
            |pane| pane.session().display_title().to_owned(),
        )
}

/// Where a tab's focused pane is working, as its row in the panel prints it.
fn where_it_is(workspace: &Workspace, tab: &Tab) -> Option<String> {
    let directory = tab
        .panes()
        .focused()?
        .session()
        .working_directory
        .as_deref();
    Some(git::user_friendly_path(directory?, workspace.home()))
}

/// Every settings row a query is looking for, in the order the rail holds
/// them.
///
/// `all` is what `#` asks for: with nothing typed the everything list leaves
/// these out, and the scoped list shows the lot.
///
/// **The Keyboard Shortcuts page is not one of them.** Its rows are the
/// commands, one per row, which are already the first section of this list and
/// the whole of the `?` one — searching `split` would otherwise answer twice,
/// with the same words, and a search that answers twice teaches a person that
/// one of the answers is noise.
///
/// Building a page means building its elements, which are thrown away here.
/// That is what the settings rail itself pays on every keystroke while its own
/// box has something in it — four pages of about thirty rows — and it is the
/// only way to ask a page what it holds: a page is a function, and the words
/// its rows are found by come back with the rows.
fn settings(
    workspace: &Workspace,
    app: &AppContext,
    needle: &str,
    goto: Goto,
    all: bool,
) -> Vec<Row> {
    let query = Query::new(needle);
    if query.is_empty() && !all {
        return Vec::new();
    }

    let host = workspace.host();
    let mut rows: Vec<Row> = Vec::new();
    for (page, title) in host.settings_pages() {
        if title == shortcuts::PAGE_TITLE {
            continue;
        }
        let Some(key) = host.settings_page_key(page).map(str::to_owned) else {
            continue;
        };
        let Some(categories) = host.build_settings_page(page, workspace, app) else {
            continue;
        };

        for category in categories {
            for entry in category.entries {
                // A note has no words and is not a row: a paragraph of prose
                // is not something Enter can take you to.
                let Some(words) = &entry.words else {
                    continue;
                };
                if !entry.matches(&query, &[&title, &category.title]) {
                    continue;
                }
                let label = words.label.clone();
                rows.push(Row::Command(
                    Entry {
                        action: None,
                        title: label.clone(),
                        detail: Some(format!("{title} \u{b7} {}", category.title)),
                        chords: Vec::new(),
                        note: None,
                    },
                    Target::about(goto.setting, format!("{key}\t{label}")),
                ));
            }
        }
    }
    rows
}

/// Whether every term is somewhere in what a command row says.
///
/// They need not match the same thing: this is a person narrowing a list, not
/// writing a pattern. The chord text is in the haystack in **every** list a
/// command appears in, because typing `cmd+k` and being told it searches the
/// tabs is a win on the fast path too — and it is the half of "what is bound
/// to what" that a list of titles cannot answer.
fn command_matches(
    title: &str,
    action: &ActionName,
    owner: &PluginId,
    chords: &[Chord],
    terms: &[&str],
) -> bool {
    terms.iter().all(|term| {
        title.to_lowercase().contains(term)
            || action.as_str().contains(term)
            || owner.name().contains(term)
            || chords.iter().any(|chord| chord.text.contains(term))
            // So that `unbound` is a query, which is the list of things worth
            // binding. Both words, because only one of them is on the row.
            || (chords.is_empty() && (NOT_BOUND.contains(term) || *term == "unbound"))
    })
}

/// The commands, flat and by title: no headings, and the launcher's ordering
/// exactly.
///
/// `title.cmp` and not the lowercased comparison a group uses, because this is
/// the launcher's hot path and nothing about it has changed.
fn flat(mut matched: Vec<(&PluginId, Entry, Target)>) -> Vec<Row> {
    matched.sort_by(|(_, left, _), (_, right, _)| left.title.cmp(&right.title));
    matched
        .into_iter()
        .map(|(_, entry, target)| Row::Command(entry, target))
        .collect()
}

/// Keys mode: the commands, under the plugin that registered them.
///
/// Groups appear in first-seen order over `host.commands()`, which is
/// registration order, which is load order, which is the order the sidebar and
/// the settings rail are already in — two views of one list should have one
/// shape. A plugin with fewer than [`GROUP_MINIMUM`] commands goes into one
/// trailing [`ELSEWHERE`] instead of claiming a heading over a single row.
fn grouped(
    host: &Host,
    sizes: &HashMap<&PluginId, usize>,
    matched: Vec<(&PluginId, Entry, Target)>,
) -> Vec<Row> {
    let mut owners: Vec<&PluginId> = Vec::new();
    let mut buckets: Vec<Vec<(Entry, Target)>> = Vec::new();
    let mut elsewhere: Vec<(Entry, Target)> = Vec::new();

    for (owner, entry, target) in matched {
        if sizes.get(owner).copied().unwrap_or_default() < GROUP_MINIMUM {
            elsewhere.push((entry, target));
            continue;
        }
        match owners.iter().position(|known| *known == owner) {
            Some(at) => buckets[at].push((entry, target)),
            None => {
                owners.push(owner);
                buckets.push(vec![(entry, target)]);
            }
        }
    }

    let mut rows: Vec<Row> = Vec::new();
    for (owner, bucket) in owners.into_iter().zip(buckets) {
        rows.extend(group(host.name_of(owner), bucket));
    }
    rows.extend(group(ELSEWHERE.to_owned(), elsewhere));
    rows
}

/// A heading and the rows under it, or nothing at all when there are none.
///
/// A query that empties a group takes the heading with it. An owner's bucket
/// cannot be empty — it is made by putting a row in it — but the fold's is
/// emitted whether or not anything landed in it, and a heading over nothing is
/// a fence.
///
/// Bound first, then alphabetical, which in the 52-row window group puts the
/// rows that *are* the keyboard in one block at the top and the unbound below
/// — the survey the whole list exists to be. Lowercased, unlike the launcher's
/// raw `String::cmp`, so that `Zoom` does not sort before `add`.
fn group(name: String, mut entries: Vec<(Entry, Target)>) -> Vec<Row> {
    if entries.is_empty() {
        return Vec::new();
    }

    entries.sort_by_key(|(entry, _)| (entry.chords.is_empty(), entry.title.to_lowercase()));

    let mut rows = vec![Row::Group(name)];
    rows.extend(
        entries
            .into_iter()
            .map(|(entry, target)| Row::Command(entry, target)),
    );
    rows
}

/// Every key bound to something `host.commands()` does not hold.
///
/// Two kinds live here and what tells them apart is whether anything answers.
/// A plain action somebody bound — `crook/palette/next`,
/// `crook/shortcuts/rebind` — is runnable and is a [`Row::Command`]. A line in
/// somebody's `keybindings.json` naming a command this machine does not have
/// is not, and is a [`Row::Fact`] carrying [`NOTHING_ANSWERS`].
///
/// This is an unconditional heading rather than part of [`ELSEWHERE`]: the
/// fold exists to stop seven one-row plugins each claiming a heading, and this
/// is the answer to a different question, which has to be findable as a block.
fn other_bindings(
    host: &Host,
    by_command: &HashMap<&ActionName, Vec<&Rule>>,
    terms: &[&str],
) -> Vec<Row> {
    let mut bound: Vec<(&ActionName, &[&Rule])> = by_command
        .iter()
        .filter(|(name, _)| {
            !host
                .commands()
                .iter()
                .any(|(_, command, _)| command == **name)
        })
        .map(|(name, rules)| (*name, rules.as_slice()))
        .collect();

    // Alphabetically: there is no title to sort by, and the order the rules
    // happened to be written in means nothing to somebody reading them.
    bound.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));

    let mut rows: Vec<Row> = Vec::new();
    for (name, rules) in bound {
        let chords = chord::collapse(rules);
        let id = host.action(name);
        let note = id.is_none().then_some(NOTHING_ANSWERS);
        if !binding_matches(name, &chords, note, terms) {
            continue;
        }

        let entry = Entry {
            action: Some(name.clone()),
            title: name.to_string(),
            detail: None,
            chords,
            note,
        };
        rows.push(match id {
            Some(id) => Row::Command(entry, Target::plain(id)),
            None => Row::Fact(entry),
        });
    }

    if rows.is_empty() {
        return Vec::new();
    }
    let mut all = vec![Row::Group(OTHER_BINDINGS.to_owned())];
    all.extend(rows);
    all
}

/// Whether every term is somewhere in what a binding's row says.
///
/// The action name is the title here, so that is one field and not two.
fn binding_matches(
    name: &ActionName,
    chords: &[Chord],
    note: Option<&str>,
    terms: &[&str],
) -> bool {
    terms.iter().all(|term| {
        name.as_str().contains(term)
            || chords.iter().any(|chord| chord.text.contains(term))
            || note.is_some_and(|note| note.contains(term))
    })
}

/// The keys that belong to the program in the pane, out of the table the
/// Keyboard Shortcuts page prints.
///
/// Every one is a [`Row::Fact`]: there is nothing to run and nothing to
/// rebind, and a keybindings file cannot reach any of them. Shared with that
/// page rather than copied, because a second copy of the table is how the two
/// surfaces would come to disagree about what ctrl-c does.
fn in_a_pane(terms: &[&str]) -> Vec<Row> {
    let rows: Vec<Row> = PANE_KEYS
        .iter()
        .filter(|key| pane_matches(key, terms))
        .map(|key| {
            Row::Fact(Entry {
                action: None,
                title: key.label.to_owned(),
                detail: None,
                chords: parts(key.keys()),
                note: None,
            })
        })
        .collect();

    if rows.is_empty() {
        return Vec::new();
    }
    let mut all = vec![Row::Group(IN_A_PANE.to_owned())];
    all.extend(rows);
    all
}

/// One [`Chord`] per ` / `-separated part of what a pane key answers to.
///
/// A part is not always a chord: `ctrl+c / ctrl+z / ctrl+d` is three of them,
/// but `right / alt+right for one word` is a key and then a phrase, and
/// `hover it, then click` is a sentence. Splitting is all this does — which
/// parts are drawn as a cap and which as words is the row's decision, and it
/// turns on whether a part has a space in it.
fn parts(keys: &str) -> Vec<Chord> {
    keys.split(" / ")
        .map(|part| Chord {
            text: part.to_owned(),
            only_when: None,
        })
        .collect()
}

/// Whether every term is somewhere in what a pane key's row says, or in what
/// it is searchable by.
///
/// The keywords are why `sigint`, `ctrl-c` written with a hyphen and `yank`
/// find rows whose visible text contains none of them. The settings page
/// already searches them, and this is the same table.
fn pane_matches(key: &PaneKey, terms: &[&str]) -> bool {
    terms.iter().all(|term| {
        key.label.to_lowercase().contains(term)
            || key.keywords.iter().any(|keyword| keyword.contains(term))
            || key.keys().contains(term)
    })
}

#[cfg(test)]
#[path = "rows_tests.rs"]
mod tests;
