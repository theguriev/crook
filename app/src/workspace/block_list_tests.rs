//! The arithmetic the list is built on: how tall an item is, and which rows of
//! the open block belong to it.
//!
//! Painting, hovering and copying need a window to run in and are exercised
//! against a real shell in `workspace::tests::shells::blocks`. What is here is
//! the part that decides where everything lands, driven by a real emulator fed
//! real OSC 133 marks — because a hand-built block would be a block this
//! module invented rather than one the emulator makes.

use crook_terminal::{Block, Emulator, Palette, TerminalSize};

use super::*;

/// A grid wide enough for the marks below and short enough to reason about.
const GRID: TerminalSize = TerminalSize::new(20, 6);

/// An emulator with nothing in it.
fn emulator() -> Emulator {
    Emulator::new(GRID, 100, Palette::default())
}

/// One integrated command cycle: a prompt, a command line, its output, and a
/// completion carrying `exit`.
fn cycle(emulator: &mut Emulator, command: &str, output: &str, exit: i32) {
    emulator.advance(b"\x1b]133;A\x07$ ");
    emulator.advance(b"\x1b]133;B\x07");
    emulator.advance(command.as_bytes());
    emulator.advance(b"\r\n\x1b]133;C\x07");
    for line in output.lines() {
        emulator.advance(line.as_bytes());
        emulator.advance(b"\r\n");
    }
    emulator.advance(format!("\x1b]133;D;{exit}\x07").as_bytes());
}

/// The one finished block an emulator holds.
fn finished(emulator: &Emulator) -> &Block {
    emulator.blocks().first().expect("a command finished")
}

#[test]
fn a_block_is_its_rows_with_a_command_padding_above_and_below() {
    let mut emulator = emulator();
    cycle(&mut emulator, "echo hi", "hi", 0);

    let block = finished(&emulator);
    let rows = block.rows.rows() as f32;
    assert!(rows >= 2., "the prompt line and the output are both in it");
    assert_eq!(block_height(block), PADDING_TOP + rows + PADDING_BOTTOM);
}

#[test]
fn a_block_with_no_command_collapses_to_its_rows() {
    // What a shell prints between commands, and what a session restores into.
    // Two lines of padding around it would be chrome claiming a boundary
    // nobody reported.
    let mut emulator = emulator();
    cycle(&mut emulator, "echo hi", "hi", 0);

    let bare = Block {
        command: None,
        ..finished(&emulator).clone()
    };
    assert_eq!(block_height(&bare), bare.rows.rows() as f32);
}

#[test]
fn the_open_block_is_the_rows_its_anchor_names_and_no_others() {
    // The rows above it are stale copies of a block that has already been
    // harvested into the store, and drawing them would show the same output
    // twice.
    let mut emulator = emulator();
    cycle(&mut emulator, "echo hi", "hi", 0);
    emulator.advance(b"\x1b]133;A\x07$ ");

    let snapshot = emulator.snapshot();
    let (first, last) = live_rows(&snapshot).expect("the new prompt is on screen");
    assert!(
        first > 0,
        "the finished block is above the open one, at rows the list must not draw"
    );
    assert_eq!(first, last, "one prompt row, and nothing under it yet");
    assert_eq!(live_height(&snapshot, true), PADDING_TOP + 1.);
}

#[test]
fn an_open_block_that_has_printed_nothing_takes_no_room_at_all() {
    // Otherwise a prompt that has not arrived leaves two lines of empty
    // padding above the composer, which is a gap with nothing in it.
    let mut emulator = emulator();
    cycle(&mut emulator, "echo hi", "hi", 0);

    let snapshot = emulator.snapshot();
    assert_eq!(
        live_rows(&snapshot),
        None,
        "the block closed and the next one has printed nothing"
    );
    assert_eq!(live_height(&snapshot, true), 0.);
}

#[test]
fn the_open_block_of_a_shell_with_no_marks_is_the_whole_screen() {
    // The honest rendering of an un-integrated shell: one block, no chrome
    // claiming boundaries nobody reported.
    let mut emulator = emulator();
    emulator.advance(b"one\r\ntwo\r\nthree");

    let snapshot = emulator.snapshot();
    let (first, last) = live_rows(&snapshot).expect("something is on screen");
    assert_eq!((first, last), (0, 2));
    assert_eq!(live_height(&snapshot, true), PADDING_TOP + 3.);
    assert_eq!(
        live_height(&snapshot, false),
        PADDING_TOP + 3. + RUNNING_PADDING_BOTTOM,
        "with no composer under it the block keeps its last row off the edge"
    );
}

#[test]
fn a_failed_command_reads_as_failed_and_an_interrupted_one_does_not() {
    // Ctrl-C's 130 and SIGPIPE's 141 are how a command *ends*, not how it
    // fails, and a session full of red every time somebody pressed Ctrl-C
    // would make the colour mean nothing.
    let mut emulator = emulator();
    for exit in [0, 1, 130, 141] {
        cycle(&mut emulator, "run", "out", exit);
    }

    let verdicts: Vec<_> = emulator.blocks().iter().map(verdict_of).collect();
    assert_eq!(
        verdicts,
        vec![
            Verdict::Plain,
            Verdict::Failed,
            Verdict::Plain,
            Verdict::Plain
        ]
    );
}

#[test]
fn a_block_copies_back_as_exactly_its_own_text() {
    // The thing scrollback cannot do: the rows were harvested when the command
    // ended, so this is that command and its output with no neighbour's text
    // and no trailing blank rows.
    let mut emulator = emulator();
    cycle(&mut emulator, "first", "ONE", 0);
    cycle(&mut emulator, "second", "TWO", 0);

    let second = emulator.blocks()[1].rows.to_text();
    let second = second.trim_end();
    assert!(second.contains("second"), "its own command: {second:?}");
    assert!(second.contains("TWO"), "its own output: {second:?}");
    assert!(!second.contains("first"), "the block before it: {second:?}");
    assert!(!second.contains("ONE"), "that block's output: {second:?}");
}
