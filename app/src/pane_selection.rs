//! What one pane has selected in its output, and the press that is taking it,
//! kept between the frames that draw them.
//!
//! **The selection lives here and nowhere else.** It used to live in the
//! emulator, because that was the only place that could keep it anchored to
//! its text while the shell printed underneath — and that stopped being true
//! the moment a finished command's rows were harvested out of the grid into a
//! block of their own. A selection anchored to a cell of the grid could then
//! only ever cover the newest block, which is the one thing nobody wants to
//! select: everything a person has already run had become unselectable. So the
//! anchors moved up here, into the address space the list draws — a block, a
//! row of that block, a column — where nothing the shell does moves them. See
//! [`crate::selection`].
//!
//! There are two facts, and they are deliberately separate.
//!
//! * **The press** is one end and a kind, from the moment the button goes down
//!   until it comes up. It is not a selection: a click that is never dragged
//!   anywhere selects nothing at all, which is what makes a plain click on the
//!   output *clear* the last selection rather than leave a one-cell highlight.
//! * **The selection** is only ever one that covers cells. Whoever moves it
//!   has the blocks to resolve it against and passes the answer in, so that
//!   "is anything selected?" — the question `ctrl-c` is settled by — is a
//!   `bool` read rather than a walk. It is asked *of a surface*
//!   ([`PaneSelection::has_selection_in`]), because a selection taken on the
//!   other one is a highlight nobody can see, and a highlight nobody can see
//!   must not be what an interrupt is spent on.
//!
//! It is one of these per pane, held by the workspace and handed to the
//! element that draws the output on every frame, exactly as
//! [`TextInput`](crate::text_input::TextInput) and
//! [`PaneBlocks`](crate::pane_blocks::PaneBlocks) are — because the element
//! tree is thrown away and rebuilt on every render, and a press is only half a
//! gesture. A pane that closes takes its selection with it.

use std::cell::RefCell;
use std::rc::Rc;

use crook_terminal::SelectionKind;

use crate::selection::{Anchor, Cells, Selection};

/// One pane's output selection.
#[derive(Clone, Default)]
pub struct PaneSelection(Rc<RefCell<State>>);

/// What a pane keeps between frames.
#[derive(Default)]
struct State {
    /// The press that is open, and what it is taking at a time.
    pressed: Option<(SelectionKind, Anchor)>,
    /// What is selected — never a selection that covers no cells.
    selected: Option<Selection>,
    /// How wide the grid was when it was made.
    ///
    /// A resize that changes the column count reflows every row the open block
    /// holds, so the cells a selection named hold other text afterwards. There
    /// is no honest way to re-anchor that, and a highlight left over a screen
    /// that has been re-wrapped under it is a copy of something nobody
    /// selected — so it is let go of instead.
    columns: usize,
    /// Which address space its rows are numbered in.
    ///
    /// A pane draws either a list of blocks or one grid, and a row number
    /// means a different row in each. So the space is part of the selection,
    /// and a pane that changes surface under one lets go of it for the same
    /// reason a resize does: the picture it was drawn on is gone. See
    /// [`crate::selection::Cells`].
    cells: Cells,
    /// Whether the button that is down was given to a program reading the
    /// mouse rather than to a selection.
    ///
    /// **A third state beside the two above, not a flag on the press.** A
    /// press on a pane means one of two entirely different things depending on
    /// what is running in it: with nothing reading the mouse it starts a
    /// selection, and with `vim`, `htop` or `tmux` in the pane it is a click
    /// the program takes, whose moves and release are reports rather than a
    /// drag.
    ///
    /// Which it was is decided **once, when the button goes down**, and
    /// remembered. Asking the terminal again on every move would be asking a
    /// question whose answer can change mid-drag: a program that turned mouse
    /// reporting off while a button was held would leave the release
    /// unreported and half a selection dragged out of a screen nobody selected
    /// in.
    reporting: bool,
}

impl PaneSelection {
    /// A pane with nothing selected and no press open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Says a press landed on this pane's output, and what it selects on its
    /// own: a word for a double click, a line for a triple, nothing at all for
    /// a single one.
    ///
    /// Whatever was selected before is let go of either way. Reports whether
    /// anything on screen changed.
    pub fn press(
        &self,
        kind: SelectionKind,
        at: Anchor,
        covers: bool,
        columns: usize,
        cells: Cells,
    ) -> bool {
        let mut state = self.0.borrow_mut();
        let was = state.selected;
        state.pressed = Some((kind, at));
        state.selected = covers.then(|| Selection::new(kind, at, at));
        state.columns = columns;
        state.cells = cells;
        was != state.selected
    }

    /// Moves the open end of the press to `at`, reporting whether that changed
    /// what is selected.
    ///
    /// `covers` is whether the selection this makes covers any cells, which
    /// only the caller can know: it has the blocks, and this does not.
    pub fn drag(&self, at: Anchor, covers: bool) -> bool {
        let mut state = self.0.borrow_mut();
        let Some((kind, anchor)) = state.pressed else {
            return false;
        };
        let was = state.selected;
        state.selected = covers.then(|| Selection::new(kind, anchor, at));
        was != state.selected
    }

    /// Selects a region outright, with no gesture behind it.
    ///
    /// What `--select-output` and the tests aim with, since neither has a
    /// pointer to hold down.
    pub fn select(&self, selection: Selection, columns: usize, cells: Cells) {
        let mut state = self.0.borrow_mut();
        state.selected = Some(selection);
        state.columns = columns;
        state.cells = cells;
    }

    /// The end a drag would move, when a press is open.
    pub fn pressed(&self) -> Option<(SelectionKind, Anchor)> {
        self.0.borrow().pressed
    }

    /// Whether a press on this pane's output has not been released yet.
    pub fn is_dragging(&self) -> bool {
        self.0.borrow().pressed.is_some()
    }

    /// Ends the press, reporting whether there was one to end.
    ///
    /// The answer is what makes a release meaningful: only the pane the press
    /// landed on has anything to do with the button coming up again.
    pub fn release(&self) -> bool {
        self.0.borrow_mut().pressed.take().is_some()
    }

    /// Says the press that just landed was given to a program reading the
    /// mouse, so nothing that follows it is a selection.
    ///
    /// Whatever was selected is let go of, exactly as an ordinary press lets
    /// go of it: the pointer now belongs to the program, and a highlight left
    /// behind under it would be one nobody could extend or clear.
    pub fn begin_reporting(&self) -> bool {
        let mut state = self.0.borrow_mut();
        let was = state.selected;
        state.pressed = None;
        state.selected = None;
        state.reporting = true;
        was.is_some()
    }

    /// Whether the button that is down belongs to a program reading the mouse.
    pub fn is_reporting(&self) -> bool {
        self.0.borrow().reporting
    }

    /// Ends a reported press, reporting whether there was one to end.
    pub fn end_reporting(&self) -> bool {
        std::mem::take(&mut self.0.borrow_mut().reporting)
    }

    /// What is selected, or `None` when nothing is.
    ///
    /// Whichever space it was made in, so this is for the caller that is about
    /// to resolve it in that space. Anything painting or copying against one
    /// surface asks [`Self::selection_in`] instead.
    pub fn selection(&self) -> Option<Selection> {
        self.0.borrow().selected
    }

    /// The space its rows are numbered in.
    pub fn cells(&self) -> Cells {
        self.0.borrow().cells
    }

    /// What is selected, if it was selected in `cells`.
    ///
    /// The guard on every reader that has one surface's blocks in its hand: a
    /// row number from the other space names a row, and resolving it would
    /// paint and copy text nobody selected rather than fail.
    pub fn selection_in(&self, cells: Cells) -> Option<Selection> {
        let state = self.0.borrow();
        state.selected.filter(|_| state.cells == cells)
    }

    /// Whether anything is selected — the question `ctrl-c` is settled by.
    pub fn has_selection(&self) -> bool {
        self.0.borrow().selected.is_some()
    }

    /// Whether anything is selected in `cells`.
    ///
    /// What the copy chord is actually settled by, because a selection the
    /// surface on screen cannot draw is one the person cannot see: letting it
    /// claim `ctrl-c` would spend the interrupt on a highlight that is not
    /// there. See [`Self::resurfaced`], which is what stops one lasting longer
    /// than the frame that changed surface.
    pub fn has_selection_in(&self, cells: Cells) -> bool {
        self.selection_in(cells).is_some()
    }

    /// Lets go of what is selected, reporting whether there was anything to
    /// let go of.
    pub fn clear(&self) -> bool {
        self.0.borrow_mut().selected.take().is_some()
    }

    /// Lets go of a selection made at a different width, reporting whether it
    /// did.
    ///
    /// Called from layout, where the width is known. See [`State::columns`].
    pub fn reflowed(&self, columns: usize) -> bool {
        let stale = {
            let state = self.0.borrow();
            state.selected.is_some() && state.columns != columns
        };
        stale && self.clear()
    }

    /// Lets go of a selection made on the other surface, reporting whether it
    /// did.
    ///
    /// Called from layout by whichever element is drawing the output, which is
    /// the one thing that knows which surface the pane is on now. A command
    /// that grows past the top of the viewport takes its pane from the list to
    /// the grid mid-drag, and a full-screen program does it without any output
    /// arriving at all; the selection underneath is in rows the new surface
    /// numbers differently, so it goes — highlight, press and all — rather
    /// than being read against the wrong picture. The press goes too because
    /// half a gesture in one space and half in the other is a region with one
    /// end in each.
    pub fn resurfaced(&self, cells: Cells) -> bool {
        let mut state = self.0.borrow_mut();
        if state.cells == cells {
            return false;
        }
        state.cells = cells;
        state.pressed = None;
        state.selected.take().is_some()
    }
}

#[cfg(test)]
#[path = "pane_selection_tests.rs"]
mod tests;
