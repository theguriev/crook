use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::{Config, Term};

use super::*;
use crate::blocks::LiveBlock;
use crate::harvest::harvest;
use crate::snapshot::{Palette, TerminalSize};

/// A terminal holding whatever the bytes drew.
fn term(size: TerminalSize, bytes: &str) -> Term<VoidListener> {
    let mut term = Term::new(Config::default(), &size, VoidListener);
    let mut parser = alacritty_terminal::vte::ansi::Processor::<
        alacritty_terminal::vte::ansi::StdSyncHandler,
    >::new();
    parser.advance(&mut term, bytes.as_bytes());
    term
}

/// The same rows twice: once still in the grid, once harvested out of it.
///
/// **Every test here runs against both**, because the whole reason this type
/// exists is that the two must answer identically — a highlight drawn from one
/// and a copy taken from the other is exactly the bug the block list had.
fn both(bytes: &str, rows: usize, columns: usize) -> (Snapshot, BlockRows) {
    let size = TerminalSize::new(columns as u16, rows as u16);
    let term = term(size, bytes);
    let palette = Palette::default();
    let snapshot = crate::snapshot::build(&term, &palette, 0, None, LiveBlock::default());
    let stored = harvest(&term, &palette, 0, rows as i32 - 1);
    (snapshot, stored)
}

/// Runs `check` against a live window over the whole grid and against the
/// store the same grid was harvested into.
fn for_each(bytes: &str, rows: usize, columns: usize, check: impl Fn(Rows<'_>, &str)) {
    let (snapshot, stored) = both(bytes, rows, columns);
    check(
        Rows::Live {
            snapshot: &snapshot,
            top: 0,
            count: rows,
        },
        "live",
    );
    check(Rows::Stored(&stored), "stored");
}

/// One row's text, as a copy would take it.
fn text(rows: Rows<'_>, row: usize) -> String {
    let mut out = String::new();
    rows.write(row, 0..rows.line_length(row), &mut out);
    out
}

#[test]
fn test_both_stores_report_the_same_shape() {
    for_each("one\r\ntwo", 3, 10, |rows, from| {
        assert_eq!(3, rows.count(), "{from}");
        assert_eq!(10, rows.columns(), "{from}");
    });
}

#[test]
fn test_trailing_blanks_are_not_part_of_a_row() {
    // A row is as long as what was printed into it. The rest is a grid having
    // to hold something, and a copy that brought it along would paste eighty
    // spaces after every line.
    for_each("hello\r\n\r\nx", 3, 20, |rows, from| {
        assert_eq!(5, rows.line_length(0), "{from}");
        assert_eq!(0, rows.line_length(1), "{from}: an empty row");
        assert_eq!("hello", text(rows, 0), "{from}");
        assert_eq!("", text(rows, 1), "{from}");
    });
}

#[test]
fn test_a_folded_line_says_the_row_below_continues_it() {
    // Twenty-six characters into a twenty-column grid: the emulator folds it,
    // and the fold is a place the terminal put the text rather than something
    // the child printed.
    for_each("abcdefghijklmnopqrstuvwxyz", 3, 20, |rows, from| {
        assert!(rows.wraps(0), "{from}: the folded row");
        assert!(!rows.wraps(1), "{from}: the row it folded onto");
        assert_eq!("abcdefghijklmnopqrst", text(rows, 0), "{from}");
        assert_eq!("uvwxyz", text(rows, 1), "{from}");
    });
}

#[test]
fn test_the_last_row_of_a_block_never_continues_into_the_next_one() {
    // A row that fills its width exactly carries the emulator's fold flag, and
    // the block after it is a different command. Running the two together
    // would invent a line nobody printed.
    for_each("abcdefghij", 1, 10, |rows, from| {
        assert!(!rows.wraps(0), "{from}");
    });
}

#[test]
fn test_a_double_width_character_is_one_character_in_two_columns() {
    for_each("a漢b", 1, 10, |rows, from| {
        assert!(
            rows.cell(0, 1)
                .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE)),
            "{from}"
        );
        assert!(
            rows.cell(0, 2)
                .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER)),
            "{from}"
        );

        // Reaching either column reaches the whole character, and the copy
        // takes it once rather than as a character and a space.
        assert_eq!(1..3, rows.whole_characters(0, 1..2), "{from}");
        assert_eq!(1..3, rows.whole_characters(0, 2..3), "{from}");
        let mut out = String::new();
        rows.write(0, 1..3, &mut out);
        assert_eq!("漢", out, "{from}");
    });
}

#[test]
fn test_zero_width_characters_follow_the_character_they_belong_to() {
    // `e` with a combining acute: two chars, one cell, and a copy that dropped
    // the accent would hand back a different word.
    for_each("e\u{301}x", 1, 10, |rows, from| {
        assert_eq!(&['\u{301}'], rows.zerowidth(0, 0), "{from}");
        assert_eq!("e\u{301}x", text(rows, 0), "{from}");
    });
}

#[test]
fn test_a_window_can_start_part_way_down_the_grid() {
    // What the open block is: rows `top..` of the viewport and no others,
    // because the rows above them belong to blocks already harvested.
    let (snapshot, _) = both("one\r\ntwo\r\nthree", 3, 10);
    let rows = Rows::Live {
        snapshot: &snapshot,
        top: 1,
        count: 2,
    };
    assert_eq!(2, rows.count());
    assert_eq!("two", text(rows, 0));
    assert_eq!("three", text(rows, 1));
}

#[test]
fn test_a_row_the_block_does_not_have_is_answered_rather_than_indexed() {
    // A selection is anchored to a row, and the window under it moves: the
    // wheel takes rows off the top of a grid and a shorter pane takes them off
    // the bottom of the open block. Every question here has to answer for a
    // row that is no longer there — `line_length` was the one that indexed the
    // snapshot with it and panicked on the paint path.
    for_each("one\r\ntwo", 2, 10, |rows, from| {
        assert_eq!(0, rows.line_length(9), "{from}");
        assert_eq!(None, rows.cell(9, 0), "{from}");
        assert!(rows.zerowidth(9, 0).is_empty(), "{from}");
        assert!(!rows.wraps(9), "{from}");
        assert_eq!("", text(rows, 9), "{from}");
    });
}
