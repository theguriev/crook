//! Boundaries in a `&str`: graphemes, words and lines — and the one transform
//! that keeps text inside them, [`printable`].
//!
//! Every function here is a pure query over borrowed text, which is why they
//! live apart from [`Editor`](super::Editor): the rules for "where does the
//! previous word start" are worth reading and testing on their own, and none
//! of them needs to know that a caret or a selection exists.
//!
//! Two invariants hold throughout. Offsets are byte offsets, and every offset
//! returned lands on a grapheme cluster boundary — a caret in the middle of a
//! multi-byte character is a panic waiting for the next `&text[..offset]`.
//! Callers pass offsets that are already on a boundary; [`snap`] is how an
//! offset from outside — a hit test, a stored position after the text changed
//! — becomes one.

use std::borrow::Cow;
use std::ops::Range;

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

/// `text` with everything that has no cell to sit in taken out of it.
///
/// A command line is a run of characters the field draws one to a cell and the
/// shell then reads as a line, and three kinds of character fit neither:
///
/// * `\r`, on its own or before a `\n`. Text copied from a Windows program or
///   a web page carries it, and a stray carriage return left in the buffer is
///   a character every line query here would have to know about, that nothing
///   on screen can show, and that breaks the grapheme-boundary invariant
///   outright — UAX #29 makes `\r\n` one cluster while the line helpers split
///   on the `\n` byte, so a caret could land between the two.
/// * a tab, which becomes one space. Its width is a property of the terminal
///   it is going to rather than of the text, and a cell that drew nothing
///   would make the caret sit somewhere the characters do not.
/// * every other control character, which is dropped. A command copied off a
///   web page can carry an embedded escape or a BEL, and a field that showed a
///   clean-looking line and then wrote an escape sequence to the shell on
///   Enter would be worse than one that showed nothing at all. Only the
///   control character goes: what is left of `ESC [ 3 1 m` is `[31m`, which is
///   visible, selectable and deletable — the field shows exactly what Enter
///   will send, which is the property worth keeping.
///
/// Borrowed when there is nothing to take out, which is every keystroke and
/// nearly every paste.
pub fn printable(text: &str) -> Cow<'_, str> {
    let clean = |character: char| character == '\n' || !character.is_control();
    if text.chars().all(clean) {
        return Cow::Borrowed(text);
    }

    let mut printable = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                characters.next_if_eq(&'\n');
                printable.push('\n');
            }
            '\t' => printable.push(' '),
            '\n' => printable.push('\n'),
            _ if character.is_control() => {}
            _ => printable.push(character),
        }
    }
    Cow::Owned(printable)
}

/// The offset one grapheme cluster before `offset`, or 0 at the start.
///
/// A cluster, not a `char`: `é` written as `e` plus a combining acute is two
/// `char`s and one thing a person would call a character, and the flag emoji
/// is two more.
pub fn prev_grapheme(text: &str, offset: usize) -> usize {
    let mut cursor = GraphemeCursor::new(offset, text.len(), true);
    cursor.prev_boundary(text, 0).ok().flatten().unwrap_or(0)
}

/// The offset one grapheme cluster after `offset`, or the end of the text.
pub fn next_grapheme(text: &str, offset: usize) -> usize {
    let mut cursor = GraphemeCursor::new(offset, text.len(), true);
    cursor
        .next_boundary(text, 0)
        .ok()
        .flatten()
        .unwrap_or(text.len())
}

/// `offset` moved back to the nearest grapheme boundary at or before it, and
/// clamped into the text.
pub fn snap(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }

    let mut cursor = GraphemeCursor::new(offset, text.len(), true);
    if cursor.is_boundary(text, 0).unwrap_or(true) {
        offset
    } else {
        prev_grapheme(text, offset)
    }
}

/// The start of the word before `offset`, or 0 when there is none.
///
/// This is a text field's idea of a word, not a shell's: the separators
/// between words are skipped rather than treated as words of their own, so
/// from the end of `git commit -m` one step left lands on the `m` and the next
/// on the `c` of `commit`. From inside a word it lands on that word's start.
pub fn prev_word(text: &str, offset: usize) -> usize {
    let mut start = 0;
    for (index, segment) in text.split_word_bound_indices() {
        if index >= offset {
            break;
        }
        if is_word(segment) {
            start = index;
        }
    }
    start
}

/// The end of the word after `offset`, or the end of the text when there is
/// none. The mirror of [`prev_word`], and it stops at the end of the word it
/// is already inside.
pub fn next_word(text: &str, offset: usize) -> usize {
    for (index, segment) in text.split_word_bound_indices() {
        let end = index + segment.len();
        if end > offset && is_word(segment) {
            return end;
        }
    }
    text.len()
}

/// The offset just after the newline that precedes `offset`, or 0.
pub fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

/// The offset of the newline that follows `offset`, or the end of the text.
pub fn line_end(text: &str, offset: usize) -> usize {
    text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index)
}

/// How many grapheme clusters `offset` sits past the start of its line.
///
/// Graphemes rather than bytes because this is what up and down aim at, and a
/// column measured in bytes would drift on any line holding a character wider
/// than ASCII.
pub fn column(text: &str, offset: usize) -> usize {
    text[line_start(text, offset)..offset]
        .graphemes(true)
        .count()
}

/// The offset `column` clusters into the line starting at `start`, clamped to
/// that line's end so a short line cannot swallow the next one.
pub fn offset_at_column(text: &str, start: usize, column: usize) -> usize {
    let line = &text[start..line_end(text, start)];
    line.grapheme_indices(true)
        .nth(column)
        .map_or(start + line.len(), |(index, _)| start + index)
}

/// The range a double click selects: the word-bound segment holding `offset`.
///
/// Whatever the segment is — a word, a run of spaces, a punctuation mark — is
/// what gets selected, which is what a text field does. Past the last
/// character the last segment is selected, so a click in the empty space after
/// a line takes the word that ends it.
pub fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    let mut last = 0..0;
    for (index, segment) in text.split_word_bound_indices() {
        last = index..index + segment.len();
        if offset < last.end {
            break;
        }
    }
    last
}

/// The range a triple click selects: the line holding `offset`, without its
/// newline.
pub fn line_range_at(text: &str, offset: usize) -> Range<usize> {
    line_start(text, offset)..line_end(text, offset)
}

/// Whether a word-bound segment is a word rather than the space or punctuation
/// between two.
fn is_word(segment: &str) -> bool {
    segment.chars().any(char::is_alphanumeric)
}
