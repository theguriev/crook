//! The text editor model behind a pane's command input.
//!
//! A pane's terminal grid shows what the shell wrote; the input below it is
//! where the next command is composed, and this module is everything that
//! input knows: what the text is, where the caret is, what is selected, and
//! what each editing operation does to all three.
//!
//! # What is deliberately not here
//!
//! Nothing draws, nothing touches a clipboard, and nothing knows a keystroke.
//! [`Editor::copy`] hands back a `String` and [`Editor::paste`] takes one, so
//! the system clipboard stays in the layer that owns the window; movements
//! arrive as an already-resolved [`Motion`], so whether "previous word" was
//! typed as Alt-Left or Ctrl-Left is settled before it gets here. The result
//! is that the entire behaviour of the input — every movement from every
//! position, undo grouping, history, grapheme-aware deletion — is testable in
//! microseconds with no window, no GPU and no fonts.
//!
//! # Positions
//!
//! A position is a byte offset into [`Editor::text`], always on a grapheme
//! cluster boundary. Offsets arriving from outside — a hit test, a saved
//! position — go through [`text::snap`] on the way in, so no offset the
//! renderer computes can ever put the caret inside a character.
//!
//! # What can be in the buffer
//!
//! Printable characters and `\n`, and nothing else: every way text gets in —
//! typing, pasting, [`Editor::set_text`] — goes through [`text::printable`],
//! which turns `\r\n` and `\r` into `\n`, a tab into a space, and drops the
//! rest. Two things depend on it. A stray `\r` would break the grapheme
//! boundary invariant above, because UAX #29 binds it to the `\n` after it
//! while every line query here splits on the `\n` byte; and a control
//! character that draws nothing would make the field show a clean-looking
//! command and then hand the shell something else on Enter.
//!
//! # Lines
//!
//! Up and down move by *line*, and a line here is a run of text between
//! newlines. The input does not wrap: it grows downwards as newlines are
//! added, so a logical line and a visual line are the same thing, and this
//! module stays independent of any width. If wrapping ever arrives, [`Motion`]
//! keeps its meaning and only the line helpers in [`text`] change.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use self::history::History;
use self::undo::{Revision, UndoStack};

mod history;
mod text;
mod undo;

#[cfg(test)]
mod tests;

/// What is selected, as the end that stays put and the end that moves.
///
/// Not a `Range`, because a range cannot say which end grows: shift-extending
/// left and then back right has to shrink the same selection rather than start
/// a new one, and only an anchor and a head express that. The caret is always
/// at `head`, and an empty selection is a bare caret.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// The end that a shift-extension leaves where it is.
    pub anchor: usize,
    /// The end that moves, and where the caret is drawn.
    pub head: usize,
}

impl Selection {
    /// A selection of nothing: a caret at `offset`.
    pub fn caret(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    /// A selection from `anchor` to `head`, in either direction.
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    /// Whether nothing is selected.
    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    /// The lower of the two ends.
    pub fn start(self) -> usize {
        self.anchor.min(self.head)
    }

    /// The higher of the two ends.
    pub fn end(self) -> usize {
        self.anchor.max(self.head)
    }

    /// The selected span, ordered.
    pub fn range(self) -> Range<usize> {
        self.start()..self.end()
    }
}

/// A caret movement, already resolved from whatever key produced it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Motion {
    /// One grapheme cluster back.
    Left,
    /// One grapheme cluster forward.
    Right,
    /// One line up, keeping the goal column.
    Up,
    /// One line down, keeping the goal column.
    Down,
    /// To the start of the previous word.
    WordLeft,
    /// To the end of the next word.
    WordRight,
    /// To the first character of the current line.
    LineStart,
    /// To the last character of the current line.
    LineEnd,
    /// To the very start of the text.
    BufferStart,
    /// To the very end of the text.
    BufferEnd,
}

impl Motion {
    /// Whether this motion runs towards the start of the text, which is also
    /// the end of a selection it collapses to.
    fn is_backward(self) -> bool {
        matches!(
            self,
            Self::Left | Self::Up | Self::WordLeft | Self::LineStart | Self::BufferStart
        )
    }
}

/// A command line being composed: its text, its caret, its selection, its
/// undo history and the lines submitted before it.
#[derive(Debug, Default)]
pub struct Editor {
    /// The whole text, as one `String` with byte offsets into it.
    ///
    /// A command line is tens of characters, occasionally hundreds. At that
    /// size the memmove an insert costs is unmeasurable next to the keystroke
    /// that caused it, and a rope would buy nothing but a second structure to
    /// keep in agreement with the offsets everything else holds.
    text: String,
    selection: Selection,
    /// The column up and down aim for, kept across a run of them so that
    /// passing over a short line does not permanently narrow the caret's path.
    /// Any other movement, and any edit, forgets it.
    goal_column: Option<usize>,
    revisions: UndoStack,
    history: History,
}

impl Editor {
    /// An empty editor.
    pub fn new() -> Self {
        Self::default()
    }

    // --- Reading -------------------------------------------------------------

    /// The text being composed.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether there is nothing to submit.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// What is selected, and which end of it the caret is on.
    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// Where the caret is.
    pub fn caret(&self) -> usize {
        self.selection.head
    }

    /// The selected text, empty when nothing is selected.
    pub fn selected_text(&self) -> &str {
        &self.text[self.selection.range()]
    }

    /// How many lines the text occupies. Always at least one, and one more
    /// than the number of newlines — a trailing newline opens a line that the
    /// caret can sit on.
    pub fn line_count(&self) -> usize {
        self.text.matches('\n').count() + 1
    }

    /// The caret as a zero-based line and grapheme column, which is what a
    /// renderer needs to put a bar on screen.
    pub fn caret_line_column(&self) -> (usize, usize) {
        let caret = self.selection.head;
        (
            self.text[..caret].matches('\n').count(),
            text::column(&self.text, caret),
        )
    }

    /// The offset at a zero-based line and grapheme column, clamped into the
    /// text: what a click resolves to once the renderer has turned a point
    /// into a row and a column.
    pub fn offset_at(&self, line: usize, column: usize) -> usize {
        let start = if line == 0 {
            0
        } else {
            self.text
                .match_indices('\n')
                .nth(line - 1)
                .map_or(self.text.len(), |(index, _)| index + 1)
        };
        text::offset_at_column(&self.text, start, column)
    }

    /// The lines submitted this session, oldest first.
    pub fn history(&self) -> &[String] {
        self.history.entries()
    }

    // --- Placing the caret ---------------------------------------------------

    /// Puts the caret at `offset`, snapped onto a grapheme boundary and
    /// clamped into the text, and drops any selection.
    pub fn set_caret(&mut self, offset: usize) {
        self.set_selection(Selection::caret(offset));
    }

    /// Selects `selection`, with both ends snapped onto grapheme boundaries
    /// and clamped into the text.
    pub fn set_selection(&mut self, selection: Selection) {
        self.selection = Selection::new(
            text::snap(&self.text, selection.anchor),
            text::snap(&self.text, selection.head),
        );
        self.goal_column = None;
        self.revisions.break_run();
    }

    /// Selects everything.
    pub fn select_all(&mut self) {
        self.set_selection(Selection::new(0, self.text.len()));
    }

    /// Selects the word-bound segment holding `offset`: a double click.
    pub fn select_word_at(&mut self, offset: usize) {
        let range = text::word_range_at(&self.text, text::snap(&self.text, offset));
        self.set_selection(Selection::new(range.start, range.end));
    }

    /// Selects the line holding `offset`, without its newline: a triple click.
    pub fn select_line_at(&mut self, offset: usize) {
        let range = text::line_range_at(&self.text, text::snap(&self.text, offset));
        self.set_selection(Selection::new(range.start, range.end));
    }

    // --- Moving the caret ----------------------------------------------------

    /// Moves the caret, dropping any selection.
    pub fn move_caret(&mut self, motion: Motion) {
        let from = if motion.is_backward() {
            self.selection.start()
        } else {
            self.selection.end()
        };

        // Collapsing a selection is the whole of what Left and Right do to
        // one: from a selected word, Left puts the caret before it rather than
        // one character further back. Every other motion runs from the edge it
        // collapsed to.
        let collapsing =
            !self.selection.is_empty() && matches!(motion, Motion::Left | Motion::Right);
        let (target, goal) = if collapsing {
            (from, None)
        } else {
            self.resolve(from, motion)
        };

        self.selection = Selection::caret(target);
        self.goal_column = goal;
        self.revisions.break_run();
    }

    /// Moves the head of the selection, leaving the anchor: the same movement
    /// with Shift held.
    pub fn extend_selection(&mut self, motion: Motion) {
        let (target, goal) = self.resolve(self.selection.head, motion);
        self.selection.head = target;
        self.goal_column = goal;
        self.revisions.break_run();
    }

    /// What the Up key does: a line up, or the previous history entry when the
    /// caret is already on the first line.
    pub fn up(&mut self) {
        // From the caret rather than from the edge of a selection: with the
        // whole of a two-line command selected, Up moves the caret up a line
        // — a selection is not a reason to throw the line away for a history
        // entry.
        let on_first_line = text::line_start(&self.text, self.selection.head) == 0;
        if on_first_line && self.history_previous() {
            return;
        }
        self.move_caret(Motion::Up);
    }

    /// What the Down key does: a line down, or the next history entry when the
    /// caret is already on the last line.
    pub fn down(&mut self) {
        // The caret's line, for the reason [`Self::up`] gives.
        let on_last_line = text::line_end(&self.text, self.selection.head) == self.text.len();
        if on_last_line && self.history_next() {
            return;
        }
        self.move_caret(Motion::Down);
    }

    // --- Editing -------------------------------------------------------------

    /// Inserts text at the caret, replacing the selection if there is one.
    ///
    /// What goes in is [`text::printable`]: a keystroke's text is already, and
    /// a caller handing over a line of its own — `--type`, a test — must not
    /// be able to put a character in the buffer that no cell can hold.
    pub fn insert(&mut self, insertion: &str) {
        let insertion = text::printable(insertion);
        if insertion.is_empty() && self.selection.is_empty() {
            return;
        }

        let before = self.revision();
        // One grapheme is somebody typing, and typing coalesces into runs.
        // Anything longer arrived some other way and is a step of its own.
        if is_one_grapheme(&insertion) {
            let whitespace = insertion.chars().all(char::is_whitespace);
            if self.selection.is_empty() {
                self.revisions.push_typing(before, whitespace);
            } else {
                self.revisions.start_typing(before, whitespace);
            }
        } else {
            self.revisions.push(before);
        }
        self.replace_range(self.selection.range(), &insertion);
    }

    /// Inserts a line break. The input is multi-line; Enter is the caller's to
    /// interpret, and it calls this when it means a newline rather than a
    /// submission.
    pub fn insert_newline(&mut self) {
        self.edit(self.selection.range(), "\n");
    }

    /// Deletes the selection, or the grapheme cluster before the caret.
    pub fn backspace(&mut self) {
        if !self.selection.is_empty() {
            self.edit(self.selection.range(), "");
            return;
        }

        let head = self.selection.head;
        if head == 0 {
            return;
        }
        let before = self.revision();
        self.revisions.push_deleting(before);
        self.replace_range(text::prev_grapheme(&self.text, head)..head, "");
    }

    /// Deletes the selection, or the grapheme cluster after the caret.
    pub fn delete_forward(&mut self) {
        if !self.selection.is_empty() {
            self.edit(self.selection.range(), "");
            return;
        }

        let head = self.selection.head;
        if head == self.text.len() {
            return;
        }
        let before = self.revision();
        self.revisions.push_deleting(before);
        self.replace_range(head..text::next_grapheme(&self.text, head), "");
    }

    /// Deletes the selection, or back to the start of the previous word.
    pub fn delete_word_left(&mut self) {
        let range = if self.selection.is_empty() {
            text::prev_word(&self.text, self.selection.head)..self.selection.head
        } else {
            self.selection.range()
        };
        self.edit(range, "");
    }

    /// Deletes the selection, or forward to the end of the next word.
    pub fn delete_word_right(&mut self) {
        let range = if self.selection.is_empty() {
            self.selection.head..text::next_word(&self.text, self.selection.head)
        } else {
            self.selection.range()
        };
        self.edit(range, "");
    }

    /// Deletes the selection, or from the caret back to the start of its line.
    pub fn delete_to_line_start(&mut self) {
        let head = self.selection.head;
        let range = if self.selection.is_empty() {
            text::line_start(&self.text, head)..head
        } else {
            self.selection.range()
        };
        self.edit(range, "");
    }

    /// Deletes the selection, or from the caret forward to the end of its line.
    pub fn delete_to_line_end(&mut self) {
        let head = self.selection.head;
        let range = if self.selection.is_empty() {
            head..text::line_end(&self.text, head)
        } else {
            self.selection.range()
        };
        self.edit(range, "");
    }

    /// Replaces everything, putting the caret at the end.
    ///
    /// Through [`text::printable`] like every other way text gets in.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.edit(0..self.text.len(), &text::printable(&text.into()));
    }

    /// Throws the line away, and with it any history walk in progress. What
    /// abandoning a half-typed command does.
    pub fn clear(&mut self) {
        self.history.reset();
        self.edit(0..self.text.len(), "");
    }

    // --- Clipboard -----------------------------------------------------------

    /// The selected text, for the caller to put on the system clipboard, or
    /// `None` when nothing is selected and there is nothing to copy.
    pub fn copy(&self) -> Option<String> {
        (!self.selection.is_empty()).then(|| self.selected_text().to_string())
    }

    /// The selected text, removed. `None` when nothing is selected, and then
    /// nothing is removed either.
    pub fn cut(&mut self) -> Option<String> {
        let cut = self.copy()?;
        self.edit(self.selection.range(), "");
        Some(cut)
    }

    /// Inserts clipboard text at the caret, replacing the selection.
    ///
    /// Through [`text::printable`], which is what a clipboard makes
    /// unavoidable: what is on it was written by something else entirely, and
    /// a command copied off a web page carries `\r\n`, tabs, and sometimes an
    /// escape sequence that Enter would hand straight to the shell.
    pub fn paste(&mut self, pasted: &str) {
        let pasted = text::printable(pasted);
        if pasted.is_empty() && self.selection.is_empty() {
            return;
        }
        self.edit(self.selection.range(), &pasted);
    }

    // --- Undo ----------------------------------------------------------------

    /// Steps back one edit, returning whether there was one.
    pub fn undo(&mut self) -> bool {
        let current = self.revision();
        let Some(revision) = self.revisions.undo(current) else {
            return false;
        };
        self.restore(revision);
        true
    }

    /// Steps forward one undone edit, returning whether there was one.
    pub fn redo(&mut self) -> bool {
        let current = self.revision();
        let Some(revision) = self.revisions.redo(current) else {
            return false;
        };
        self.restore(revision);
        true
    }

    // --- History -------------------------------------------------------------

    /// Takes the composed line, records it in the history and empties the
    /// editor. What Enter does, with the caller sending the result to the pty.
    pub fn submit(&mut self) -> String {
        let line = std::mem::take(&mut self.text);
        self.history.push(&line);
        self.selection = Selection::default();
        self.goal_column = None;
        self.revisions.clear();
        line
    }

    /// Shows the previous history entry, keeping the line in progress so
    /// [`history_next`](Self::history_next) can put it back. Returns whether
    /// there was an earlier entry to show.
    pub fn history_previous(&mut self) -> bool {
        let Some(entry) = self.history.previous(&self.text) else {
            return false;
        };
        self.show(entry);
        true
    }

    /// Shows the next history entry, or the line that was in progress when the
    /// walk started. Returns whether a walk was in progress at all.
    pub fn history_next(&mut self) -> bool {
        let Some(entry) = self.history.next() else {
            return false;
        };
        self.show(entry);
        true
    }

    // --- Internals -----------------------------------------------------------

    /// Where `motion` lands from `from`, and the goal column to remember.
    fn resolve(&self, from: usize, motion: Motion) -> (usize, Option<usize>) {
        match motion {
            Motion::Left => (text::prev_grapheme(&self.text, from), None),
            Motion::Right => (text::next_grapheme(&self.text, from), None),
            Motion::Up | Motion::Down => {
                let goal = self
                    .goal_column
                    .unwrap_or_else(|| text::column(&self.text, from));
                let up = motion == Motion::Up;
                // Off either end, the caret goes as far as it can in the
                // direction asked, which is what a text field does — and it
                // leaves the goal column standing, so coming back lands on it.
                let fallback = if up { 0 } else { self.text.len() };
                (
                    self.vertical(from, goal, up).unwrap_or(fallback),
                    Some(goal),
                )
            }
            Motion::WordLeft => (text::prev_word(&self.text, from), None),
            Motion::WordRight => (text::next_word(&self.text, from), None),
            Motion::LineStart => (text::line_start(&self.text, from), None),
            Motion::LineEnd => (text::line_end(&self.text, from), None),
            Motion::BufferStart => (0, None),
            Motion::BufferEnd => (self.text.len(), None),
        }
    }

    /// The offset one line above or below `from` at `goal`, or `None` when
    /// there is no such line.
    fn vertical(&self, from: usize, goal: usize, up: bool) -> Option<usize> {
        let start = if up {
            let start = text::line_start(&self.text, from);
            text::line_start(&self.text, start.checked_sub(1)?)
        } else {
            let end = text::line_end(&self.text, from);
            if end == self.text.len() {
                return None;
            }
            end + 1
        };
        Some(text::offset_at_column(&self.text, start, goal))
    }

    /// Replaces `range` as one undo step of its own.
    fn edit(&mut self, range: Range<usize>, replacement: &str) {
        if range.is_empty() && replacement.is_empty() {
            return;
        }
        let before = self.revision();
        self.revisions.push(before);
        self.replace_range(range, replacement);
    }

    /// Rewrites the text and leaves the caret after what was written. The one
    /// place the text is mutated, so no edit can forget the caret or the goal
    /// column.
    fn replace_range(&mut self, range: Range<usize>, replacement: &str) {
        let caret = range.start + replacement.len();
        self.text.replace_range(range, replacement);
        self.selection = Selection::caret(caret);
        self.goal_column = None;
    }

    /// Puts a recalled history entry on screen.
    fn show(&mut self, line: String) {
        // Undo is per line: a recalled entry is a new editing session, not
        // something the previous line's steps can reach back into.
        self.revisions.clear();
        self.text = line;
        self.selection = Selection::caret(self.text.len());
        self.goal_column = None;
    }

    fn revision(&self) -> Revision {
        Revision {
            text: self.text.clone(),
            selection: self.selection,
        }
    }

    fn restore(&mut self, revision: Revision) {
        self.text = revision.text;
        self.selection = revision.selection;
        self.goal_column = None;
    }
}

/// Whether `text` is exactly one grapheme cluster, which is what a key press
/// produces and a paste generally does not.
fn is_one_grapheme(text: &str) -> bool {
    let mut graphemes = text.graphemes(true);
    graphemes.next().is_some() && graphemes.next().is_none()
}
