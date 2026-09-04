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

use crate::plugin::{ActionName, Showing};

use crate::clipboard::Clipboard;
use crate::text_input::TextInput;
use crate::workspace::Fonts;

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
}

impl Palette {
    /// A palette that is not showing.
    pub(super) fn new(showing: Showing, clipboard: Clipboard, fonts: Fonts) -> Self {
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
        }
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

    /// Moves it, wrapping at both ends over a list of `len` rows.
    ///
    /// Wrapping because the list is short and a person holding Down expects to
    /// come back to the top, which is what the Themes panel does with the same
    /// keys.
    pub(super) fn move_selection(&self, by: isize, len: usize) {
        if len == 0 {
            self.selected.set(0);
            return;
        }
        let index = self.selected.get() as isize;
        self.selected
            .set((index + by).rem_euclid(len as isize) as usize);
    }

    /// Puts the selection somewhere that exists, given what is being shown.
    ///
    /// Two different situations and they want different answers. A query that
    /// has *changed* puts the keyboard back on the first row, because the
    /// first row is the best match for what was just typed and leaving the
    /// selection four rows down a list somebody is still typing is how a
    /// palette runs the wrong thing. A list that changed for any other reason
    /// — a plugin disabled while the palette is up — only clamps, so the
    /// selection stays where the person put it.
    pub(super) fn settle(&self, query: &str, len: usize) {
        let changed = {
            let mut last = self.last_query.borrow_mut();
            let changed = *last != query;
            if changed {
                *last = query.to_owned();
            }
            changed
        };

        if changed {
            self.selected.set(0);
            self.scroll.lock().scroll_to_top();
            return;
        }

        if len == 0 {
            self.selected.set(0);
        } else if self.selected.get() >= len {
            self.selected.set(len - 1);
        }
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

/// One row: a command, and what to call it.
pub(super) struct Command {
    /// The name, printed on the right of the row.
    ///
    /// Because it is the only way a person finds out what to write in their
    /// keybindings file, and because two plugins may reasonably both offer "Refresh" —
    /// the name is what tells them apart.
    pub(super) action: ActionName,
    /// What it is called.
    pub(super) title: String,
}
