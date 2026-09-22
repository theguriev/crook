//! The arithmetic the list is built on: how tall an item is, and which rows of
//! the open block belong to it.
//!
//! Painting, hovering and copying need a window to run in and are exercised
//! against a real shell in `workspace::tests::shells::blocks`. What is here is
//! the part that decides where everything lands, driven by a real emulator fed
//! real OSC 133 marks — because a hand-built block would be a block this
//! module invented rather than one the emulator makes.

use std::time::Instant;

use crook_terminal::{Block, Emulator, Palette, TerminalSize};
use crookui_core::event::Modifiers;

use super::*;
use crate::pane_link::LinkCells;

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
    let (first, last) = snapshot.live_rows().expect("the new prompt is on screen");
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
        snapshot.live_rows(),
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
    let (first, last) = snapshot.live_rows().expect("something is on screen");
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

/// What `inline_start` is asked, for a pane that is drawing blocks with a
/// composer under it and is scrolled to its own end.
fn inline(emulator: &mut Emulator) -> Option<usize> {
    inline_at(emulator, false)
}

/// The same, saying whether the list has output cut off below the fold.
fn inline_at(emulator: &mut Emulator, cut_off: bool) -> Option<usize> {
    let snapshot = emulator.snapshot();
    let surface = pane_surface::of(&snapshot, Instant::now());
    inline_start(&snapshot, surface, cut_off)
}

#[test]
fn the_composer_starts_one_column_after_the_prompt_s_last_cell() {
    // The whole feature: the shell said where its prompt ended, so the line
    // being typed continues that row instead of starting one below it.
    let mut emulator = emulator();
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");

    assert_eq!(
        inline(&mut emulator),
        Some(2),
        "`$ ` is two cells, so the third is where typing goes"
    );

    // And it is the same cell the shell itself would use, which is what makes
    // the composed line land under the echoed one.
    let snapshot = emulator.snapshot();
    let (_, last) = snapshot.live_rows().expect("the prompt is on screen");
    assert_eq!(
        snapshot.live_block.prompt_end.map(|end| end.row),
        Some(last as i32),
        "the prompt has to be on the row the list draws last"
    );
}

#[test]
fn a_shell_that_reports_nothing_keeps_the_composer_on_its_own_row() {
    // The fallback, and the reason there is no third case: with no mark there
    // is nothing but the screen to go on, and no pattern in a prompt to find.
    let mut emulator = emulator();
    emulator.advance(b"$ ");

    assert_eq!(inline(&mut emulator), None);
    // The list is unchanged by that: one open block holding everything, drawn
    // exactly as it was, with the composer under its last row.
    let snapshot = emulator.snapshot();
    assert_eq!(snapshot.live_rows(), Some((0, 0)));
    assert_eq!(live_height(&snapshot, true), PADDING_TOP + 1.);
}

#[test]
fn a_list_scrolled_off_its_own_bottom_gives_the_composer_a_row_back() {
    // The row above the composer is then whatever the wheel left there, and a
    // line typed onto it would land on somebody's output. It is the same
    // condition the rule above the composer is drawn on, so the frame that
    // gains the seam is the frame the composer takes its own row.
    let mut emulator = emulator();
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");

    assert_eq!(inline_at(&mut emulator, false), Some(2));
    assert_eq!(inline_at(&mut emulator, true), None);
}

#[test]
fn a_prompt_the_shell_has_printed_past_is_not_continued() {
    // A mark on a row that is no longer the block's last one. A `B` a file
    // printed looks exactly like this, and so does a prompt something scrolled
    // away from.
    let mut emulator = emulator();
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");
    assert_eq!(inline(&mut emulator), Some(2));

    emulator.advance(b"\r\nnoise");
    assert_eq!(
        inline(&mut emulator),
        None,
        "the prompt's row has output under it now"
    );
}

#[test]
fn there_is_nothing_to_continue_where_there_is_no_composer() {
    // Both of the surfaces that take the field away. A full-screen program
    // owns the grid, and a command that has been running longer than a blink
    // has taken the space the field was in — in neither case is there a line
    // being typed to place.
    let mut alt = emulator();
    alt.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07vim\r\n\x1b]133;C\x07");
    alt.advance(b"\x1b[?1049h~");
    assert_eq!(inline(&mut alt), None, "the alternate screen");

    let mut running = emulator();
    running.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07sleep 9\r\n\x1b]133;C\x07");
    let snapshot = running.snapshot();
    let later = Instant::now() + pane_surface::LONG_RUNNING;
    assert_eq!(
        inline_start(&snapshot, pane_surface::of(&snapshot, later), false),
        None,
        "a long-running command"
    );
}

/// A command-less block — the pre-prompt banner a shell-integrated session
/// closes into its first block — is drawn with no top padding, and its links
/// have to be hit-tested against that. Built here rather than in the real-shell
/// suite because a shell the harness starts prints no banner to make one, and
/// the bug this guards is pure row arithmetic: `link_at` once figured the first
/// row from the constant `PADDING_TOP` instead of the block-aware `padding_top`,
/// so on such a block the top row's links were dead and every other row opened
/// the URL from the row above.
#[test]
fn links_on_a_command_less_block_land_on_their_own_rows() {
    use crookui_core::geometry::{Point, ZIndex, vec2f};
    use std::sync::Arc;

    let mut emulator = emulator();
    // Two lines of pre-prompt output, a distinct URL on each, closed into a
    // command-less block by the first prompt mark.
    emulator.advance(b"https://a.test\r\nhttps://b.test\r\n");
    emulator.advance(b"\x1b]133;A\x07$ ");

    let block = emulator
        .blocks()
        .first()
        .expect("the banner closed into a block");
    assert!(
        block.command.is_none(),
        "the banner block carries no command"
    );

    let history = Arc::new(BlockHistory::new(
        emulator.blocks().iter().cloned().map(Arc::new).collect(),
        0,
    ));
    let font = crate::terminal_font::CellFont::headless(12.);
    let metrics = font.metrics();
    let mut list = BlockList::new(history, emulator.snapshot(), font, PaneBlocks::new());

    // Lay the list out in a window big enough to hold it. `size` and `origin`
    // are what `layout` and `paint` set; a test sets them so `bounds` answers.
    let size = vec2f(400., 300.);
    list.size = Some(size);
    list.origin = Some(Point::from_vec2f(vec2f(0., 0.), ZIndex::Normal(0)));
    list.measure(size);

    let item = list
        .window
        .first()
        .copied()
        .expect("the banner is on screen");
    assert_eq!(item.index, 0, "the banner is the first item");
    let origin = list.bounds().expect("the list has bounds").origin();
    let follow = follow();
    assert!(terminal_element::opens_links(follow));

    // The centre of each row lands on that row's own URL — not None, and not
    // the row above's.
    for (row, uri) in [(0usize, "https://a.test"), (1, "https://b.test")] {
        let at = origin
            + vec2f(
                GUTTER + 0.5 * metrics.width,
                item.top + (row as f32 + 0.5) * metrics.height,
            );
        let span = list
            .link_at(at, follow)
            .unwrap_or_else(|| panic!("row {row} has no link under the pointer"));
        assert_eq!(span.uri, uri, "row {row} opened the wrong URL");
        assert!(
            matches!(
                span.cells.as_slice(),
                [LinkCells { row: LinkRow::Block { index: 0, row: r }, .. }] if *r == row
            ),
            "row {row} was hit as {:?}",
            span.cells
        );
    }
}

/// The modifier that follows a link: the platform's — Command on macOS,
/// Control off it — the same split `opens_links` makes, and the tests run on
/// both. Hardcoding Control passed on Linux and failed on the macOS runner.
fn follow() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            cmd: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }
}

/// A URL longer than the pane is wide, printed by a command that finished, is
/// one link on the two rows the terminal folded it onto — found whole from a
/// pointer on either, and underlined on both. The block is a harvested one,
/// so this is the stored rows' fold flag rather than the grid's.
#[test]
fn a_folded_url_in_a_finished_block_is_one_link_on_both_of_its_rows() {
    use crookui_core::geometry::{Point, ZIndex, vec2f};
    use std::sync::Arc;

    let mut emulator = emulator();
    // Thirty-six characters into a twenty-column grid.
    let url = "https://example.com/abcdefghijklmnop";
    cycle(&mut emulator, "cat log", url, 0);
    let block = finished(&emulator);
    assert!(block.rows.wraps(1), "the emulator folded the URL");

    let history = Arc::new(BlockHistory::new(
        emulator.blocks().iter().cloned().map(Arc::new).collect(),
        0,
    ));
    let font = crate::terminal_font::CellFont::headless(12.);
    let metrics = font.metrics();
    let mut list = BlockList::new(history, emulator.snapshot(), font, PaneBlocks::new());
    let size = vec2f(400., 300.);
    list.size = Some(size);
    list.origin = Some(Point::from_vec2f(vec2f(0., 0.), ZIndex::Normal(0)));
    list.measure(size);

    let item = list
        .window
        .first()
        .copied()
        .expect("the block is on screen");
    assert_eq!(item.index, 0);
    let origin = list.bounds().expect("the list has bounds").origin();
    let rows_top = item.top + list.padding_top(item.index) * metrics.height;
    let follow = follow();

    // Row 0 is the prompt line; the URL is rows 1 and 2.
    let whole = vec![
        LinkCells {
            row: LinkRow::Block { index: 0, row: 1 },
            start: 0,
            len: 20,
        },
        LinkCells {
            row: LinkRow::Block { index: 0, row: 2 },
            start: 0,
            len: 16,
        },
    ];
    for (row, column) in [(1usize, 5usize), (2, 3)] {
        let at = origin
            + vec2f(
                GUTTER + (column as f32 + 0.5) * metrics.width,
                rows_top + (row as f32 + 0.5) * metrics.height,
            );
        let span = list
            .link_at(at, follow)
            .unwrap_or_else(|| panic!("row {row} has no link under the pointer"));
        assert_eq!(span.uri, url, "row {row} found a cut-off URL");
        assert_eq!(
            span.cells, whole,
            "row {row} did not map the link onto both rows"
        );
    }
}
