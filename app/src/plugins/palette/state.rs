//! What the palette is doing, between one frame and the next.
//!
//! Shared by the contribution that draws it and by the handlers that change
//! it, so every field is behind interior mutability: a contribution holds
//! `&Workspace` and a handler is an `Fn`, and neither of them can hold a `&mut`
//! to this. That is the same arrangement `SettingsState` reaches for and for
//! the same reason.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crookui_core::elements::{MouseStateHandle, ScrollStateHandle};

use crate::plugin::{ActionId, Showing, Voice};

use crate::clipboard::Clipboard;
use crate::text_input::TextInput;
use crate::workspace::Fonts;

use super::rows::{Mode, Rows};

/// The two actions the rows that are not commands run.
///
/// Resolved once, when the plugin builds, rather than looked up by name on
/// every frame: `showing` runs on every keystroke and every arrow, and an
/// `ActionName` is a string that would be parsed and searched for each time.
#[derive(Copy, Clone)]
pub(super) struct Goto {
    /// Makes a tab the active one, told which through
    /// [`Host::say`](crate::plugin::Host::say).
    pub(super) tab: ActionId,
    /// Opens the settings at the page a row lives on, with the row's own words
    /// in the rail's box so that the row is on screen when it arrives.
    pub(super) setting: ActionId,
}

/// One clickable thing in the palette.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Control {
    /// The field at the top.
    Query,
    /// One row of the list, by its place in what is being shown.
    ///
    /// By position rather than by name, because the list is re-filtered on
    /// every keystroke and a handle kept per name would be a map that only
    /// grows. The cost is that a row's hover state belongs to the *slot*, and
    /// the pointer stays over the same slot while the list changes underneath
    /// it — which is what a person sees anyway.
    Row(usize),
}

/// The palette.
pub(super) struct Palette {
    /// Whether it is up, and the flag the workspace reads to take the keyboard
    /// away from the pane under it. One state, kept in two places because one
    /// of them has to be reachable from a render that holds no plugin.
    open: Cell<bool>,
    showing: Showing,
    /// Which row the keyboard is on, as a place in what is being shown.
    selected: Cell<usize>,
    /// What has been typed.
    query: TextInput,
    scroll: ScrollStateHandle,
    controls: RefCell<HashMap<Control, MouseStateHandle>>,
    /// What the query was the last time the list was worked out, so that a
    /// query which has changed can put the selection back. Held here rather
    /// than compared against the field, because the field is edited by an
    /// element and nothing tells this when it was.
    last_query: RefCell<String>,
    clipboard: Clipboard,
    fonts: Fonts,
    /// What a row leaves for the action it is about to run.
    ///
    /// Held here because the thing that says it is often a *click handler*,
    /// which holds no workspace and no host — only what it captured. See
    /// [`Voice`].
    voice: Voice,
    goto: Goto,
}

impl Palette {
    /// A palette that is not showing.
    pub(super) fn new(
        showing: Showing,
        clipboard: Clipboard,
        fonts: Fonts,
        voice: Voice,
        goto: Goto,
    ) -> Self {
        Self {
            open: Cell::new(false),
            showing,
            selected: Cell::new(0),
            query: TextInput::new(),
            scroll: ScrollStateHandle::default(),
            controls: RefCell::new(HashMap::new()),
            last_query: RefCell::new(String::new()),
            clipboard,
            fonts,
            voice,
            goto,
        }
    }

    /// The two actions the rows that are not commands run.
    pub(super) fn goto(&self) -> Goto {
        self.goto
    }

    /// The handle a row uses to tell the action it runs what it is about.
    pub(super) fn voice(&self) -> Voice {
        self.voice.clone()
    }

    /// Whether it is up.
    pub(super) fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Puts it up, on an empty query with the first row selected.
    ///
    /// Opening it again while it is up is a fresh palette rather than a
    /// no-op: the chord is what a person presses when they want to type
    /// something, and finding yesterday's query in the box is not that.
    pub(super) fn open(&self) {
        self.open.set(true);
        self.showing.set(true);
        self.selected.set(0);
        self.query.edit(crate::editor::Editor::clear);
        self.query.set_has_keys(true);
        self.last_query.replace(String::new());
        self.scroll.lock().scroll_to_top();
    }

    /// Puts it up on one of the lists, with `seed` in the box after the sigil.
    ///
    /// The seed is what `--action "crook/palette/keys split"` sends through
    /// [`Host::said`](crate::plugin::Host::said), which is how a row anywhere
    /// else hands an argumentless action its subject — and the only way these
    /// surfaces can be pictured with `--snapshot`.
    pub(super) fn open_in(&self, mode: Mode, seed: &str) {
        self.open();
        self.query.edit(|editor| editor.set_text(mode.seeded(seed)));
    }

    /// Puts the next list's sigil on the front of the query, in place of
    /// whichever one is there.
    ///
    /// A text edit rather than a flag, which is the whole argument for keeping
    /// the mode in the query: a search survives the switch, so `split`,
    /// `>split` and `?split` are one question asked of three lists. The caret
    /// lands at the end, because `Editor::set_text` puts it there — caret
    /// arithmetic on a keystroke that is about the list rather than about the
    /// text would be buying very little.
    pub(super) fn next_mode(&self) {
        self.query.edit(|editor| {
            let (mode, rest) = Mode::of(editor.text());
            let turned = mode.next().seeded(rest);
            editor.set_text(turned);
        });
    }

    /// Takes it down.
    pub(super) fn close(&self) {
        self.open.set(false);
        self.showing.set(false);
        self.query.set_has_keys(false);
    }

    /// What has been typed into it.
    pub(super) fn query(&self) -> &TextInput {
        &self.query
    }

    /// Which row the keyboard is on.
    pub(super) fn selected(&self) -> usize {
        self.selected.get()
    }

    /// Puts it on a line somebody else worked out, for the switch that keeps
    /// the row a person was on when the list under it changed.
    pub(super) fn select(&self, at: usize) {
        self.selected.set(at);
    }

    /// Moves it, wrapping at both ends over the lines it may land on.
    ///
    /// Wrapping because the list is short and a person holding Down expects to
    /// come back to the top, which is what the Themes panel does with the same
    /// keys. Which lines those are is [`Rows`]'s to say — a heading is stepped
    /// over, and so is a key a pane eats — and a list with nothing to land on
    /// leaves the selection alone rather than putting it on a heading.
    pub(super) fn move_selection(&self, by: isize, rows: &Rows) {
        if let Some(at) = rows.stepped(self.selected.get(), by) {
            self.selected.set(at);
        }
    }

    /// Puts the selection somewhere that exists, given what is being shown.
    ///
    /// Two different situations and they want different answers. A query that
    /// has *changed* puts the keyboard back on the first row, because the
    /// first row is the best match for what was just typed and leaving the
    /// selection four rows down a list somebody is still typing is how a
    /// palette runs the wrong thing. A list that changed for any other reason
    /// — a plugin disabled while the palette is up — only settles, so the
    /// selection stays near where the person put it.
    ///
    /// `query` is the *whole* of what has been typed, the mode sigil
    /// included: turning the card over changes the list under the selection
    /// exactly the way typing does, so it counts as a change. The row a person
    /// was on is put back afterwards by name, by the handler that turned it —
    /// see `crook/palette/mode`.
    pub(super) fn settle(&self, query: &str, rows: &Rows) {
        let changed = {
            let mut last = self.last_query.borrow_mut();
            let changed = *last != query;
            if changed {
                *last = query.to_owned();
            }
            changed
        };

        if changed {
            // The first *command*, not line zero, which in the list of keys is
            // a heading. This is what keeps "open it, type three letters,
            // press Enter" the three keystrokes it was before there were
            // headings to step over.
            self.selected.set(rows.first_command().unwrap_or(0));
            self.scroll.lock().scroll_to_top();
            return;
        }

        self.selected
            .set(rows.settled(self.selected.get()).unwrap_or(0));
    }

    /// The list's scroll position.
    pub(super) fn scroll(&self) -> ScrollStateHandle {
        self.scroll.clone()
    }

    /// The clipboard the field pastes from.
    pub(super) fn clipboard(&self) -> &Clipboard {
        &self.clipboard
    }

    /// The families it draws in.
    pub(super) fn fonts(&self) -> Fonts {
        self.fonts
    }

    /// The mouse state for one control, made the first time it is drawn.
    pub(super) fn control(&self, control: Control) -> MouseStateHandle {
        self.controls
            .borrow_mut()
            .entry(control)
            .or_default()
            .clone()
    }
}
