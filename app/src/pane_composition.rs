//! What an input method is composing into the program in one pane, kept
//! between the frames that draw it.
//!
//! The composer has its own: a preedit beside its editor, drawn into the line
//! being written (see [`TextInput`](crate::text_input::TextInput)). A pane
//! with no composer — vim, `less`, Claude Code, anything that has taken the
//! keyboard — has nowhere like that, and a person writing Japanese, Chinese or
//! Korean into it used to see nothing at all until the word was committed,
//! with the input method's list of candidates hanging wherever the prompt had
//! been when the program started. So the pane keeps the composition here and
//! the output draws it, underlined, at the program's cursor, which is where
//! every terminal draws one.
//!
//! **Nothing here reaches the program.** A preedit is not text: it is replaced
//! wholesale by the next one and can be abandoned without a trace, so it is
//! drawn over the grid and never written to the pty. Only the commit is, by
//! [`Output`](crate::workspace::Output), exactly as before there was anything
//! to draw.
//!
//! It is one of these per pane, held by the workspace and handed to the
//! element that draws the output on every frame, for the reason
//! [`PaneSelection`](crate::pane_selection::PaneSelection) is: the element
//! tree is thrown away and rebuilt on every render, and a composition spans
//! many of them. The element also writes back where it drew the input
//! method's caret, because that is where the window puts the candidate list
//! and nothing but the paint path knows it.
//!
//! # Who clears it
//!
//! A composition belongs to the keyboard, and goes when the keyboard does: a
//! commit or an input method giving up ends it, and so does the pane losing
//! the keyboard — focus moving to another pane or another tab, a menu, a find
//! bar or a box beside the pane taking the keys, a section of the sidebar
//! replacing the panes, a composer coming back under a command that finished —
//! and the window going behind another. The workspace says which of those
//! happened through [`PaneComposition::listen`]; the output asks
//! [`PaneComposition::is_listening`] before it takes an event, because the
//! element a keystroke reaches was built a frame before whatever moved the
//! focus.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use crookui_core::geometry::RectF;
use unicode_width::UnicodeWidthChar;

/// What an input method has composed so far, and where its caret is in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preedit {
    /// The string being composed. Never empty: an empty one is no composition.
    text: String,
    /// The range the input method's own caret or selection covers, in bytes
    /// of [`Self::text`], on character boundaries. `None` is the input method
    /// asking for no caret to be drawn.
    cursor: Option<(usize, usize)>,
}

impl Preedit {
    /// The range the input method's caret or selection covers, in bytes, when
    /// it asked for one to be drawn.
    pub fn cursor(&self) -> Option<(usize, usize)> {
        self.cursor
    }

    /// Where the input method's caret is, in bytes: the start of its range,
    /// or the end of the text when it asked for none — which is where the
    /// composer puts it too, and where the next character would go.
    pub fn caret(&self) -> usize {
        self.cursor.map_or(self.text.len(), |(start, _)| start)
    }

    /// Where each character goes on a row `columns` cells wide, for a
    /// composition that starts at the program's cursor in column `at`.
    ///
    /// A character takes the cells the emulator would give it — two for `日`,
    /// none for a combining mark, which is drawn over the one before it — so
    /// the composition is exactly as wide as what committing it will print.
    /// It is kept on the row: one that would run past the end starts further
    /// left instead, as far as the row allows, and one wider than the whole
    /// row keeps its end, which is where the input method's caret usually is.
    pub fn lay_out(&self, at: usize, columns: usize) -> Laid {
        // One entry per character: its byte offset, the character, and how
        // many cells it takes. A mark with nothing before it to sit on takes a
        // cell of its own rather than being drawn over nothing.
        let mut cells: Vec<(usize, char, usize)> = Vec::new();
        for (offset, character) in self.text.char_indices() {
            let width = match character.width() {
                Some(0) if !cells.is_empty() => 0,
                Some(0) | None => 1,
                Some(width) => width,
            };
            cells.push((offset, character, width));
        }

        // Dropped from the front, a character at a time with the marks on it,
        // until what is left fits the row.
        let mut width: usize = cells.iter().map(|(_, _, width)| width).sum();
        let mut skip = 0;
        while width > columns && skip < cells.len() {
            width -= cells[skip].2;
            skip += 1;
            while skip < cells.len() && cells[skip].2 == 0 {
                skip += 1;
            }
        }
        let kept = &cells[skip..];

        let start = at.min(columns.saturating_sub(width));
        let caret_offset = self.caret();
        let mut glyphs = Vec::with_capacity(kept.len());
        let mut column = start;
        let mut caret = start;
        for &(offset, character, width) in kept {
            // A mark is drawn from the column of the character it sits on,
            // which is the column before the pen moved on past it.
            let drawn_at = if width == 0 {
                glyphs.last().map_or(column, |&(at, _)| at)
            } else {
                column
            };
            glyphs.push((drawn_at, character));
            if offset < caret_offset {
                caret = column + width;
            }
            column += width;
        }

        Laid {
            glyphs,
            span: start..start + width,
            caret,
        }
    }
}

/// A composition placed on the cursor's row. See [`Preedit::lay_out`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Laid {
    /// Each character that fits, and the column it is drawn from.
    pub glyphs: Vec<(usize, char)>,
    /// The columns the composition covers.
    pub span: Range<usize>,
    /// The column the input method's caret is in: before the character it
    /// stands in front of, or just past the end.
    pub caret: usize,
}

/// One pane's input method composition.
///
/// Cheap to clone — it is an [`Rc`] — because the element that draws the
/// pane's output takes one every frame.
#[derive(Clone, Default)]
pub struct PaneComposition(Rc<Inner>);

#[derive(Default)]
struct Inner {
    /// Whether the pane's output, rather than a composer, is where its typing
    /// goes. Set by the workspace; see the module's notes on clearing.
    listening: Cell<bool>,
    /// What is being composed, if anything.
    preedit: RefCell<Option<Preedit>>,
    /// Where the output last drew the cell an input method's caret is in, in
    /// window coordinates: the program's cursor, or a place in the
    /// composition drawn over it.
    caret_rect: Cell<Option<RectF>>,
}

impl PaneComposition {
    /// A pane that is composing nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Says whether the pane's output is where its typing goes.
    ///
    /// Saying it is not lets go of the composition and of where its caret
    /// was, because both belong to a keyboard this pane no longer has. Reports
    /// whether a composition was let go of, which is whether the frame changed.
    pub fn listen(&self, listening: bool) -> bool {
        self.0.listening.set(listening);
        if listening {
            return false;
        }
        self.0.caret_rect.set(None);
        self.clear()
    }

    /// Whether the pane's output is where its typing goes, as of the last
    /// thing the workspace said.
    pub fn is_listening(&self) -> bool {
        self.0.listening.get()
    }

    /// Replaces the composition with what an input method just said,
    /// reporting whether anything changed.
    ///
    /// An empty `text` is the input method clearing it without committing.
    /// The range is pulled onto character boundaries of `text`, because an
    /// input method reporting one this build does not understand must not be
    /// able to make the paint path slice a character in half.
    pub fn set(&self, text: &str, cursor: Option<(usize, usize)>) -> bool {
        if text.is_empty() {
            return self.clear();
        }
        let boundary = |offset: usize| {
            let mut offset = offset.min(text.len());
            while !text.is_char_boundary(offset) {
                offset -= 1;
            }
            offset
        };
        let replacement = Preedit {
            text: text.to_owned(),
            cursor: cursor.map(|(start, end)| {
                let start = boundary(start);
                (start, boundary(end).max(start))
            }),
        };
        let mut preedit = self.0.preedit.borrow_mut();
        if preedit.as_ref() == Some(&replacement) {
            return false;
        }
        *preedit = Some(replacement);
        true
    }

    /// Abandons the composition, reporting whether there was one.
    pub fn clear(&self) -> bool {
        self.0.preedit.borrow_mut().take().is_some()
    }

    /// What is being composed, if anything.
    pub fn preedit(&self) -> Option<Preedit> {
        self.0.preedit.borrow().clone()
    }

    /// Where the output last drew the cell an input method's caret is in.
    pub fn caret_rect(&self) -> Option<RectF> {
        self.0.caret_rect.get()
    }

    /// Records where the output has just drawn that cell, or that it drew
    /// none — a cursor scrolled out of view — so the window can put the
    /// candidate list beside it.
    pub fn set_caret_rect(&self, rect: Option<RectF>) {
        self.0.caret_rect.set(rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composing(text: &str, cursor: Option<(usize, usize)>) -> Preedit {
        Preedit {
            text: text.to_owned(),
            cursor,
        }
    }

    #[test]
    fn a_composition_starts_at_the_cursor_and_a_wide_character_takes_two_cells() {
        let laid = composing("日本a", Some((6, 6))).lay_out(3, 20);
        assert_eq!(vec![(3, '日'), (5, '本'), (7, 'a')], laid.glyphs);
        assert_eq!(3..8, laid.span);
        assert_eq!(7, laid.caret, "the caret stands before the `a`");
    }

    #[test]
    fn with_no_caret_asked_for_it_is_reckoned_from_the_end() {
        let laid = composing("かな", None).lay_out(0, 20);
        assert_eq!(4, laid.caret);
    }

    #[test]
    fn a_combining_mark_is_drawn_over_the_character_before_it() {
        let laid = composing("e\u{301}x", None).lay_out(0, 20);
        assert_eq!(vec![(0, 'e'), (0, '\u{301}'), (1, 'x')], laid.glyphs);
        assert_eq!(0..2, laid.span);
    }

    #[test]
    fn a_composition_that_would_run_off_the_row_starts_further_left() {
        // A cursor near the right edge, in a narrow split: the whole word is
        // shown, over the cells before the cursor, rather than its first
        // character and nothing after it.
        let laid = composing("日本語", None).lay_out(8, 10);
        assert_eq!(4..10, laid.span);
        assert_eq!(vec![(4, '日'), (6, '本'), (8, '語')], laid.glyphs);
        assert_eq!(10, laid.caret);
    }

    #[test]
    fn a_composition_wider_than_the_row_keeps_its_end() {
        let laid = composing("日本語", None).lay_out(0, 5);
        assert_eq!(vec![(0, '本'), (2, '語')], laid.glyphs);
        assert_eq!(0..4, laid.span);
        assert_eq!(4, laid.caret);
    }

    #[test]
    fn a_row_of_no_columns_lays_out_nothing() {
        let laid = composing("日", None).lay_out(0, 0);
        assert!(laid.glyphs.is_empty());
        assert!(laid.span.is_empty());
    }

    #[test]
    fn a_range_off_a_character_boundary_is_pulled_back_onto_one() {
        let composition = PaneComposition::new();
        assert!(composition.set("日本", Some((1, 4))));
        let preedit = composition.preedit().expect("something is composed");
        assert_eq!(Some((0, 3)), preedit.cursor());
        assert_eq!(0, preedit.caret());
    }

    #[test]
    fn an_empty_preedit_is_the_composition_abandoned() {
        let composition = PaneComposition::new();
        composition.set("か", None);
        assert!(composition.set("", None), "the frame changed");
        assert_eq!(None, composition.preedit());
        assert!(
            !composition.set("", None),
            "and the next one changes nothing"
        );
    }

    #[test]
    fn losing_the_keyboard_lets_go_of_the_composition_and_its_caret() {
        let composition = PaneComposition::new();
        composition.listen(true);
        composition.set("か", None);
        composition.set_caret_rect(Some(RectF::default()));

        // A clone, as the element holds: one composition, not two.
        let held = composition.clone();
        assert!(composition.listen(false), "a composition was let go of");
        assert!(!held.is_listening());
        assert_eq!(None, held.preedit());
        assert_eq!(None, held.caret_rect());
    }
}
