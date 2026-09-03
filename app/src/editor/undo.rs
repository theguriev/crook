//! Undo and redo, and the rule that makes a run of typing one step.
//!
//! The stack holds whole snapshots rather than diffs. A command line is a few
//! hundred bytes and there are at most [`DEPTH`] of them, so the entire history
//! of an editing session is smaller than one frame of the terminal grid, and a
//! diff-based stack would only add bookkeeping that can disagree with the text.
//!
//! What actually needs care is *grouping*. Thirty keystrokes must undo as the
//! two or three words they spelled, not as thirty steps, and the boundaries
//! between those groups are the whole design — see [`UndoStack::push_typing`].

use super::Selection;

/// How many steps back an editing session can reach.
///
/// A pane lives for hours and every edit that is not part of a run pushes a
/// snapshot, so this is bounded rather than left to grow for the life of the
/// process. Deep enough that no one types their way past it in one line.
const DEPTH: usize = 256;

/// The editor's whole state at one point in time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    /// The text.
    pub text: String,
    /// Where the caret and selection were, so undo puts them back too.
    pub selection: Selection,
}

/// What the last edit was, for deciding whether the next one joins it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Run {
    /// Characters typed one at a time, the last of them whitespace or not.
    Typing {
        /// Whether the last grapheme typed was whitespace.
        whitespace: bool,
    },
    /// Backspace or Delete pressed repeatedly.
    Deleting,
}

/// The states an editing session can go back to, and the ones it came from.
#[derive(Debug, Default)]
pub struct UndoStack {
    past: Vec<Revision>,
    future: Vec<Revision>,
    run: Option<Run>,
}

impl UndoStack {
    /// Records the state before a typed grapheme, joining the run in progress
    /// when there is one.
    ///
    /// A run breaks at a word boundary: the whitespace after a word starts a
    /// new group, so undo peels the line back one word at a time. It also
    /// breaks whenever the caret moves ([`break_run`](Self::break_run), which
    /// every movement calls) and whenever an edit that is not a typed grapheme
    /// happens — a delete, a paste, or typing over a selection, each of which
    /// reaches [`push`](Self::push) instead.
    pub fn push_typing(&mut self, before: Revision, whitespace: bool) {
        let joins =
            matches!(self.run, Some(Run::Typing { whitespace: last }) if last || !whitespace);
        self.record(before, Some(Run::Typing { whitespace }), joins);
    }

    /// Records the state before a typed grapheme that ends the run in
    /// progress and starts one of its own.
    ///
    /// What typing over a selection does: the replacement cannot join what was
    /// typed before it, but the typing that follows joins the replacement —
    /// swapping a word out and typing the new one is one thing a person did,
    /// and one thing to undo.
    pub fn start_typing(&mut self, before: Revision, whitespace: bool) {
        self.record(before, Some(Run::Typing { whitespace }), false);
    }

    /// Records the state before a one-grapheme delete, joining a run of them.
    pub fn push_deleting(&mut self, before: Revision) {
        let joins = self.run == Some(Run::Deleting);
        self.record(before, Some(Run::Deleting), joins);
    }

    /// Records the state before an edit that is a step of its own.
    pub fn push(&mut self, before: Revision) {
        self.record(before, None, false);
    }

    /// Ends any run, so the next edit starts a new step. Every caret movement
    /// calls this: coming back to type somewhere else is a new group even when
    /// the caret lands where it left off.
    pub fn break_run(&mut self) {
        self.run = None;
    }

    /// Forgets everything. Undo is per line, the way readline's is: submitting
    /// or recalling a line starts an editing session that the previous one's
    /// steps have no business reaching into.
    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
        self.run = None;
    }

    /// The state to go back to, given the state to come back to.
    pub fn undo(&mut self, current: Revision) -> Option<Revision> {
        let previous = self.past.pop()?;
        self.future.push(current);
        self.run = None;
        Some(previous)
    }

    /// The state to go forward to, given the state to come back to.
    pub fn redo(&mut self, current: Revision) -> Option<Revision> {
        let next = self.future.pop()?;
        self.past.push(current);
        self.run = None;
        Some(next)
    }

    fn record(&mut self, before: Revision, run: Option<Run>, joins: bool) {
        if !joins {
            if self.past.len() == DEPTH {
                self.past.remove(0);
            }
            self.past.push(before);
        }
        self.run = run;
        // Editing after an undo abandons what was undone; there is no tree.
        self.future.clear();
    }
}
