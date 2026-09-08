//! One pane's find bar: whether it is open, what is being looked for, and
//! which match a person is standing on.
//!
//! Per pane, and kept on [`PaneInteraction`](crate::workspace) beside the
//! selection, for the reason the selection is kept there: the element tree
//! that draws the bar is rebuilt every frame, so anything a keystroke changes
//! has to outlive it. What the bar is *about* — the matches themselves — is
//! not here. Those are a walk of the pane's output against the query, and the
//! output changes under the bar as a command prints, so they are recomputed
//! from the query each frame rather than kept and invalidated. This holds only
//! the two things that are not derivable from the output: whether the bar is
//! open, and which of however many matches is the current one.
//!
//! It shares the shape the search box has — a [`TextInput`], a wish to have
//! the keyboard, a hover state — and differs in the one way that matters: the
//! search box has one instance because there is one tab list, and this has one
//! per pane because a person can be searching the output of one pane while
//! another prints. Which pane's bar has the keyboard is the focused pane's,
//! decided in `Workspace::sync_input_keys` exactly as the search box's wish is.

use std::cell::RefCell;
use std::rc::Rc;

use crookui_core::elements::MouseStateHandle;

use crate::text_input::TextInput;

/// One pane's find bar.
#[derive(Clone)]
pub struct PaneFind(Rc<RefCell<State>>);

/// What the bar keeps between frames.
struct State {
    /// Whether it is on screen at all.
    open: bool,
    /// What is being looked for, with its own caret, selection and undo.
    input: TextInput,
    /// What the pointer is doing to the field, so its focus press has
    /// somewhere to remember it was hovered.
    field: MouseStateHandle,
    /// The two buttons that step between matches: previous, then next.
    steps: [MouseStateHandle; 2],
    /// The button that closes the bar.
    close_button: MouseStateHandle,
    /// Which match is current, counted from zero.
    ///
    /// Kept even while the bar is closed and even when there are no matches:
    /// a person who typed a query, stepped to the third match and cleared the
    /// field is at nothing, and the number is meaningless until a match
    /// exists again — at which point [`Self::clamped`] brings it back inside
    /// the count rather than starting over at the top.
    current: usize,
}

impl Default for PaneFind {
    fn default() -> Self {
        Self::new()
    }
}

impl PaneFind {
    /// A pane whose find bar is closed and empty.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(State {
            open: false,
            input: TextInput::new(),
            field: MouseStateHandle::default(),
            steps: Default::default(),
            close_button: MouseStateHandle::default(),
            current: 0,
        })))
    }

    /// Opens the bar, reporting whether it was closed.
    ///
    /// Does not touch the query: opening the bar a second time on the same
    /// pane comes back to what was being looked for, which is what a person
    /// pressing the chord again to get back to their search means.
    pub fn open(&self) -> bool {
        let mut state = self.0.borrow_mut();
        !std::mem::replace(&mut state.open, true)
    }

    /// Closes the bar, reporting whether it was open. Leaves the query where
    /// it was, for the same reason [`Self::open`] does not clear it.
    pub fn close(&self) -> bool {
        let mut state = self.0.borrow_mut();
        std::mem::replace(&mut state.open, false)
    }

    /// Whether the bar is on screen.
    pub fn is_open(&self) -> bool {
        self.0.borrow().open
    }

    /// The editor behind the field.
    pub fn input(&self) -> TextInput {
        self.0.borrow().input.clone()
    }

    /// The field's hover state.
    pub fn field(&self) -> MouseStateHandle {
        self.0.borrow().field.clone()
    }

    /// The hover state of one of the two step buttons: `0` is previous, `1`
    /// is next.
    pub fn step_state(&self, forward: bool) -> MouseStateHandle {
        self.0.borrow().steps[usize::from(forward)].clone()
    }

    /// The close button's hover state.
    pub fn close_state(&self) -> MouseStateHandle {
        self.0.borrow().close_button.clone()
    }

    /// What is being looked for.
    pub fn query(&self) -> String {
        self.0.borrow().input.editor().text().to_owned()
    }

    /// The current match, brought inside a count of `total` matches.
    ///
    /// `None` when there are none: the field is empty, or nothing in the
    /// output matches what is in it. Otherwise the stored index wrapped into
    /// range, so a match count that shrank under the cursor — a command that
    /// printed over the line a match was on — leaves the cursor on a match
    /// that still exists rather than off the end.
    pub fn clamped(&self, total: usize) -> Option<usize> {
        (total > 0).then(|| self.0.borrow().current % total)
    }

    /// Steps `delta` matches from the current one, wrapping at both ends, and
    /// returns where it landed — or `None` when there is nothing to step to.
    ///
    /// The step is taken from the *clamped* current, so stepping forward from
    /// a cursor left past the end by a shrinking match count goes to the
    /// second match rather than to wherever the raw index modulo the count
    /// happened to fall.
    pub fn step(&self, delta: isize, total: usize) -> Option<usize> {
        if total == 0 {
            return None;
        }
        let total_signed = total as isize;
        let here = (self.0.borrow().current % total) as isize;
        let next = (here + delta).rem_euclid(total_signed) as usize;
        self.0.borrow_mut().current = next;
        Some(next)
    }

    /// Puts the cursor on a particular match, which is what a fresh search
    /// does: the first match after the caret is the one to stand on.
    pub fn set_current(&self, current: usize) {
        self.0.borrow_mut().current = current;
    }
}
