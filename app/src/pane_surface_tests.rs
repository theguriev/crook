//! The four answers, one per rule, against hand-built snapshots.

use crook_terminal::{BlockState, LiveBlock, Rgb};

use super::*;

/// A snapshot with nothing in it but the state these rules read.
fn snapshot(live_block: LiveBlock, alt_screen: bool) -> Snapshot {
    Snapshot {
        revision: 1,
        columns: 0,
        rows: 0,
        cells: Vec::new(),
        combining: Vec::new(),
        cursor: None,
        foreground: Rgb::new(0, 0, 0),
        background: Rgb::new(0, 0, 0),
        display_offset: 0,
        history_len: 0,
        alt_screen,
        live_block,
        title: None,
    }
}

/// An open block that has been running for `age`.
fn running(age: Duration, now: Instant) -> LiveBlock {
    LiveBlock {
        state: BlockState::Executing,
        started_at: now.checked_sub(age),
        ..LiveBlock::default()
    }
}

#[test]
fn an_idle_shell_gets_the_block_list_and_a_composer() {
    let now = Instant::now();
    let surface = of(&snapshot(LiveBlock::default(), false), now);

    assert_eq!(surface.surface, Surface::Blocks);
    assert!(surface.composer, "there is a line to compose");
    assert!(!surface.output_owns_caret(), "the field has the caret");
}

#[test]
fn the_alternate_screen_takes_the_pane_and_the_keyboard() {
    let now = Instant::now();
    let surface = of(&snapshot(LiveBlock::default(), true), now);

    assert_eq!(surface.surface, Surface::Grid);
    assert!(!surface.composer, "a full-screen program reads no line");
    assert!(surface.output_owns_caret());
}

#[test]
fn a_running_command_takes_the_composer_s_space_but_not_the_block_list() {
    let now = Instant::now();

    let quick = of(&snapshot(running(Duration::ZERO, now), false), now);
    assert!(
        quick.composer,
        "an `ls` must not flick the composer away and back"
    );

    let slow = of(&snapshot(running(LONG_RUNNING, now), false), now);
    assert!(!slow.composer, "there is nothing to compose under `less`");
    assert!(slow.output_owns_caret(), "so the program's cursor is drawn");
    assert_eq!(
        slow.surface,
        Surface::Blocks,
        "a minute of `cargo build` is still one command among the ones before it"
    );

    // And a program that really does own the screen reaches the grid by
    // outgrowing the viewport, which is the honest way to get there.
    let owning = LiveBlock {
        top_row: -1,
        ..running(LONG_RUNNING, now)
    };
    assert_eq!(of(&snapshot(owning, false), now).surface, Surface::Grid);
}

#[test]
fn an_open_block_that_has_scrolled_off_the_top_is_drawn_as_the_grid() {
    // The un-integrated shell: one block holding everything, most of it in the
    // emulator's history rather than in the snapshot. Drawing it as a block
    // would show only the screenful the snapshot holds.
    let now = Instant::now();
    let overflowing = LiveBlock {
        top_row: -1,
        bottom_row: 23,
        ..LiveBlock::default()
    };
    let surface = of(&snapshot(overflowing, false), now);

    assert_eq!(surface.surface, Surface::Grid);
    assert!(
        !surface.composer,
        "the pty is sized from the pane, so a grid squeezed above a field loses its last rows \
         under it"
    );
    assert!(surface.output_owns_caret(), "and typing goes to the pty");
}

#[test]
fn a_line_no_shell_has_answered_never_takes_the_composer_away() {
    // A shell with no integration — `ssh` to a host that has none, a
    // container, the opt-out — answers a submitted line with no mark at all,
    // so its open block stays `Submitted` for the rest of the session. If that
    // counted as running, the first Enter would take the field away and
    // nothing would ever give it back.
    let now = Instant::now();
    let sent = LiveBlock {
        state: BlockState::Submitted,
        started_at: now.checked_sub(LONG_RUNNING * 60),
        ..LiveBlock::default()
    };

    let surface = of(&snapshot(sent.clone(), false), now);
    assert!(surface.composer, "the field went with the unanswered line");
    assert_eq!(surface.surface, Surface::Blocks);
    assert_eq!(until_long_running(&snapshot(sent, false), now), None);
}

#[test]
fn the_repaint_that_hides_the_composer_is_scheduled_from_the_start_of_the_command() {
    let now = Instant::now();

    assert_eq!(
        until_long_running(&snapshot(LiveBlock::default(), false), now),
        None,
        "nothing is running, so nothing has to be redrawn"
    );

    let waiting = until_long_running(&snapshot(running(Duration::ZERO, now), false), now)
        .expect("a command that just started is on its way to long-running");
    assert!(waiting <= LONG_RUNNING && waiting > Duration::ZERO);

    assert_eq!(
        until_long_running(&snapshot(running(LONG_RUNNING, now), false), now),
        None,
        "already long-running: the frame that hides the composer has been drawn"
    );
}
