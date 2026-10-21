//! The open block's rows that leave the grid for the store a finished
//! block's rows live in: see [`BlockTracker::spilled`].
//!
//! Most of these feed two emulators the same bytes — one whose grid keeps a
//! few lines of the scrollback and spills the rest, and one whose grid keeps
//! all of it, the way every emulator did before there was a spill — and hold
//! that nothing a caller can read tells them apart: the screen at every
//! offset, the scrollback a copy and the find bar read, the open block's rows
//! and pictures, the blocks it finishes as.

use base64::Engine as _;

use super::*;

/// OSC 133 prompt start, prompt end, output start.
const A: &str = "\x1b]133;A\x07";
const B: &str = "\x1b]133;B\x07";
const C: &str = "\x1b]133;C\x07";

/// How much of the scrollback the spilling twin's grid keeps: a few screens
/// of [`ROWS`], small enough that a few hundred lines spill most of it.
const GRID: usize = 12;
const COLUMNS: u16 = 20;
const ROWS: u16 = 5;

/// Two emulators fed alike, the first spilling past [`GRID`] lines.
struct Twins {
    spilling: Emulator,
    whole: Emulator,
}

impl Twins {
    fn new(scrollback: usize) -> Self {
        let size = TerminalSize::new(COLUMNS, ROWS).with_cell_size(8, 16);
        Self {
            spilling: Emulator::with_grid_lines(size, scrollback, GRID, Palette::default()),
            whole: Emulator::with_grid_lines(size, scrollback, scrollback, Palette::default()),
        }
    }

    fn feed(&mut self, bytes: impl AsRef<[u8]>) {
        self.spilling.advance(bytes.as_ref());
        self.whole.advance(bytes.as_ref());
    }

    fn scroll(&mut self, delta: i32) {
        self.spilling.scroll_lines(delta);
        self.whole.scroll_lines(delta);
    }

    fn resize(&mut self, columns: u16, rows: u16) {
        let size = TerminalSize::new(columns, rows).with_cell_size(8, 16);
        self.spilling.resize(size);
        self.whole.resize(size);
    }

    /// Holds that nothing tells the two apart, and that the spilling one
    /// keeps no more than [`GRID`] lines in its grid while doing it.
    fn alike(&mut self, what: &str) {
        let grid = self.spilling.term.grid().history_size();
        assert!(
            grid <= GRID,
            "{what}: the grid holds {grid} lines of history"
        );

        let (spilling, whole) = (self.spilling.snapshot(), self.whole.snapshot());
        assert_eq!(whole.text(), spilling.text(), "{what}: the screen");
        assert_eq!(whole.cells, spilling.cells, "{what}: the screen's cells");
        assert_eq!(whole.combining, spilling.combining, "{what}: the marks");
        assert_eq!(whole.history_len, spilling.history_len, "{what}: history");
        assert_eq!(
            whole.display_offset, spilling.display_offset,
            "{what}: the view"
        );
        assert_eq!(whole.cursor, spilling.cursor, "{what}: the cursor");
        assert_eq!(
            alike_live(&whole.live_block),
            alike_live(&spilling.live_block),
            "{what}: the open block"
        );

        let last = whole.history_len + whole.rows;
        let (rows, at) = self.whole.harvest_rows(0, last);
        let (spilled, spilled_at) = self.spilling.harvest_rows(0, last);
        assert_eq!(rows.to_text(), spilled.to_text(), "{what}: the scrollback");
        assert_eq!(rows, spilled, "{what}: the scrollback's cells");
        assert_eq!(at, spilled_at, "{what}: where the scrollback starts");
        // And read a few rows at a time, the way the find bar reads it,
        // across the seam between the store and the grid.
        for first in (0..last).step_by(7) {
            assert_eq!(
                self.whole.harvest_rows(first, first + 6),
                self.spilling.harvest_rows(first, first + 6),
                "{what}: rows {first} to {}",
                first + 6
            );
        }

        assert_eq!(
            finished(&self.whole),
            finished(&self.spilling),
            "{what}: the finished blocks"
        );
    }
}

/// The open block as a caller sees it, without the moment its command
/// started, which the two read off the clock apart, and with its pictures
/// where they are drawn rather than which picture each is: every picture
/// has a number of its own, the twins' too.
fn alike_live(live: &LiveBlock) -> (LiveBlock, Vec<(i32, usize, usize, usize)>) {
    let pictures = live
        .images
        .iter()
        .map(|image| (image.row, image.column, image.columns, image.rows))
        .collect();
    let live = LiveBlock {
        started_at: None,
        images: Vec::new(),
        ..live.clone()
    };
    (live, pictures)
}

/// A finished block's command, rows, output start and pictures' rows.
type Finished = (Option<String>, BlockRows, Option<usize>, Vec<i32>);

/// What a finished block came to, without its clock readings.
fn finished(emulator: &Emulator) -> Vec<Finished> {
    emulator
        .blocks()
        .iter()
        .map(|block| {
            (
                block.command.clone(),
                block.rows.clone(),
                block.output_from,
                block.images.iter().map(|image| image.row).collect(),
            )
        })
        .collect()
}

/// Output the way a busy program prints it: plain lines, coloured ones, a
/// coloured row the program erased to its end, wide characters and accents,
/// and lines too long for the grid.
fn output(from: usize, to: usize) -> String {
    let mut out = String::new();
    for n in from..to {
        match n % 7 {
            0 => out.push_str(&format!("\x1b[31mred {n}\x1b[0m\r\n")),
            1 => out.push_str(&format!("\x1b[44m\x1b[Kblue {n}\x1b[0m\r\n")),
            2 => out.push_str(&format!("日本語 {n} e\u{301}\r\n")),
            3 => out.push_str(&format!("{n} {}\r\n", "long ".repeat(9))),
            4 => out.push_str(&format!("{n}\t{n}\r\n")),
            _ => out.push_str(&format!("line {n}\r\n")),
        }
    }
    out
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes
}

/// What `imgcat` prints for a picture two cells wide and two rows tall.
fn picture() -> String {
    let file = base64::engine::general_purpose::STANDARD.encode(png(16, 32));
    format!("\x1b]1337;File=inline=1:{file}\x07")
}

#[test]
fn an_open_block_past_the_grid_keeps_its_scrollback_in_the_store() {
    // A pane with no shell integration is one open block for its whole
    // life, and so is an agent's session: every line it ever printed is the
    // open block's, and only the last few stay in the grid.
    let mut twins = Twins::new(1000);
    for chunk in 0..6 {
        twins.feed(output(chunk * 40, chunk * 40 + 40));
        twins.alike(&format!("after chunk {chunk}"));
    }
    let spilled = twins.spilling.blocks.spilled().rows();
    assert!(spilled > 200, "only {spilled} rows were spilled");
    assert_eq!(
        twins.whole.history_len(),
        twins.spilling.history_len(),
        "the scrollback is as long"
    );
}

#[test]
fn the_view_scrolls_up_into_the_spilled_rows_and_back() {
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 120));
    twins.alike("at the bottom");
    // A line at a time across the seam, a page at a time, to the top, and
    // past it, which stops there.
    for step in [
        1,
        1,
        3,
        GRID as i32,
        5,
        40,
        1000,
        -1,
        -7,
        -(GRID as i32),
        -200,
        3,
        -1000,
    ] {
        twins.scroll(step);
        twins.alike(&format!("after scrolling {step}"));
    }
    twins.scroll(1000);
    let top = twins.spilling.snapshot();
    assert_eq!(top.display_offset, top.history_len);
    assert!(
        top.text().starts_with("red 0"),
        "the top of the scrollback is the first line printed: {:?}",
        top.text()
    );
}

#[test]
fn output_arriving_under_a_view_in_the_spilled_rows_leaves_the_view_on_them() {
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 100));
    twins.scroll(60);
    let before = twins.spilling.snapshot().text();
    for chunk in 0..5 {
        twins.feed(output(100 + chunk * 9, 109 + chunk * 9));
        twins.alike(&format!("after chunk {chunk}"));
    }
    assert_eq!(
        before,
        twins.spilling.snapshot().text(),
        "the reader was moved off what they were reading"
    );
}

#[test]
fn a_block_finished_past_the_grid_keeps_every_row_it_printed() {
    let mut twins = Twins::new(1000);
    twins.feed(format!("{A}$ {B}make\r\n{C}"));
    twins.feed(output(0, 150));
    twins.alike("while it runs");
    twins.feed(format!("\x1b]133;D;0\x07{A}$ {B}"));
    twins.alike("once it has finished");

    let block = twins.spilling.blocks().last().expect("a finished block");
    assert_eq!(Some("make"), block.command.as_deref());
    let text = block.rows.to_text();
    assert!(text.starts_with("$ make\nred 0"), "{text:?}");
    assert!(text.ends_with("日本語 149 e\u{301}"), "{text:?}");
    assert!(
        twins.spilling.blocks.spilled().is_empty(),
        "the spill went with the block"
    );
    assert_eq!(
        Some(1),
        block.output_from,
        "the output starts under the command"
    );
}

#[test]
fn rows_above_the_open_blocks_first_are_not_spilled_into_it() {
    // A block opens on a screen the one before it was erased from; the
    // erased rows scroll into the history ahead of the block's own, and are
    // nobody's. The grid keeps them until its history is cut, and the spill
    // does not take them: a column change, which starts the block at the
    // first row spilled, would make them the block's.
    let mut twins = Twins::new(1000);
    twins.feed(format!("{A}$ {B}one\r\n{C}first\r\n\x1b]133;D;0\x07"));
    twins.feed(format!("{A}$ {B}two\r\n{C}"));
    twins.feed(output(0, 80));
    let (rows, _) = twins.spilling.harvest_rows(0, 0);
    assert_eq!(
        "$ two",
        rows.to_text(),
        "the scrollback starts with the block"
    );
    twins.resize(17, 5);
    twins.feed(format!("\x1b]133;D;0\x07{A}$ {B}"));
    twins.alike("two blocks on");
    let second = &twins.spilling.blocks()[1];
    assert!(
        second.rows.to_text().starts_with("$ two\n"),
        "{:?}",
        second.rows.to_text()
    );
}

#[test]
fn the_scrollback_stops_at_its_length_and_the_oldest_spilled_rows_go() {
    let mut twins = Twins::new(60);
    for chunk in 0..10 {
        twins.feed(output(chunk * 50, chunk * 50 + 50));
    }
    let history = twins.spilling.history_len();
    assert!(
        (60..=60 + crate::blocks::HISTORY_HEADROOM).contains(&history),
        "the scrollback is {history} lines"
    );
    let (rows, _) = twins.spilling.harvest_rows(0, history + 10);
    let text = rows.to_text();
    assert!(!text.contains("line 5\n"), "the oldest are kept: {text:?}");
    assert!(text.contains("blue 498"), "the newest are lost: {text:?}");
    // Past its length the two drop their oldest at different moments, the
    // grid a line at a time and the store a batch at a time, so what they
    // hold is not compared; where the store's batch has just gone, it holds
    // exactly the scrollback.
}

#[test]
fn a_picture_scrolled_up_into_the_spilled_rows_stays_on_its_rows() {
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 3));
    twins.feed(picture());
    twins.feed("\r\n");
    twins.feed(output(3, 60));
    twins.alike("with the picture spilled");
    assert_eq!(
        1,
        twins.spilling.live_block().images.len(),
        "it is still there"
    );
    twins.scroll(1000);
    twins.alike("at the top, with the picture on screen");
    let top = twins.spilling.live_block();
    assert!(
        (0..i32::from(ROWS)).contains(&top.images[0].row),
        "the picture is on the screen: {:?}",
        top.images[0].row
    );
}

#[test]
fn a_column_change_lays_the_spilled_rows_out_again_at_the_new_width() {
    let mut twins = Twins::new(1000);
    // The long lines are far up, so the line that crosses from the store
    // into the grid is a short one, which a reflow leaves alone either way.
    twins.feed(output(0, 60));
    twins.feed((0..40).map(|n| format!("tail {n}\r\n")).collect::<String>());
    twins.alike("before the resize");
    for (columns, rows) in [(13, 5), (31, 5), (20, 7), (9, 4)] {
        twins.resize(columns, rows);
        twins.alike(&format!("at {columns}x{rows}"));
        twins.scroll(1000);
        twins.alike(&format!("at {columns}x{rows}, at the top"));
        twins.scroll(-1000);
        twins.feed(output(500, 520));
        twins.alike(&format!("at {columns}x{rows}, after more output"));
    }
}

#[test]
fn a_full_screen_program_hides_the_spilled_rows_and_gives_them_back() {
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 80));
    twins.feed("\x1b[?1049h\x1b[Hvim");
    twins.alike("under vim");
    assert_eq!(0, twins.spilling.history_len());
    twins.feed("\x1b[?1049l");
    twins.alike("after vim");
    assert!(twins.spilling.history_len() > GRID);
}

#[test]
fn erasing_the_scrollback_erases_the_spilled_rows() {
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 80));
    twins.scroll(50);
    twins.feed("\x1b[3J");
    twins.alike("after CSI 3 J");
    assert!(twins.spilling.blocks.spilled().is_empty());
    twins.feed(output(80, 120));
    twins.alike("after more output");
}

#[test]
fn clearing_the_pane_lets_go_of_the_spilled_rows() {
    let mut emulator = Emulator::with_grid_lines(
        TerminalSize::new(COLUMNS, ROWS),
        1000,
        GRID,
        Palette::default(),
    );
    emulator.advance(output(0, 80).as_bytes());
    assert!(!emulator.blocks.spilled().is_empty(), "the premise");
    emulator.clear_blocks();
    assert!(emulator.blocks.spilled().is_empty());
    assert_eq!(0, emulator.history_len());
}

#[test]
fn the_grid_of_an_open_block_stays_a_few_screens_deep_by_default() {
    // The default grid, at the default scrollback: ten thousand lines
    // printed into one open block leave a thousand of them in the grid,
    // where a cell is 24 bytes, and every one of them readable.
    let size = TerminalSize::new(200, 50);
    let mut emulator = Emulator::new(size, 10_000, Palette::default());
    let lines: String = (0..10_000).map(|n| format!("line {n}\r\n")).collect();
    emulator.advance(lines.as_bytes());

    assert_eq!(GRID_LINES, emulator.term.grid().history_size());
    assert_eq!(10_000 - 49, emulator.history_len());
    let (rows, at) = emulator.harvest_rows(0, 2);
    assert_eq!(0, at);
    assert_eq!("line 0\nline 1\nline 2", rows.to_text());
    // The spilled rows are their text and a run each, not 200 cells.
    let spilled = emulator.blocks.spilled();
    assert!(
        spilled.memory_usage() < spilled.rows() * 128,
        "{} bytes for {} rows",
        spilled.memory_usage(),
        spilled.rows()
    );
}

#[test]
fn a_line_folded_across_the_seam_is_laid_out_again_whole() {
    // A line too long for the grid, whose head has been spilled and whose
    // rest is still the grid's oldest rows: neither half can be laid out at
    // the new width on its own, so the rest goes up to the head first.
    let mut across = 0;
    for pad in 0..24 {
        let mut twins = Twins::new(1000);
        twins.feed(
            (0..30)
                .map(|n| format!("short {n}\r\n"))
                .collect::<String>(),
        );
        twins.feed(format!("{}\r\n", "folded ".repeat(13)));
        twins.feed((0..pad).map(|n| format!("pad {n}\r\n")).collect::<String>());
        let spill = twins.spilling.blocks.spilled();
        if spill.is_empty() || !spill.wraps(spill.rows() - 1) {
            continue;
        }
        across += 1;
        twins.resize(13, 5);
        twins.alike(&format!("{pad} lines on, at 13 columns"));
        twins.resize(29, 5);
        twins.alike(&format!("{pad} lines on, at 29 columns"));
    }
    assert!(across > 0, "the premise: no line was ever across the seam");
}

#[test]
fn a_synchronized_update_that_scrolls_past_the_headroom_loses_no_line() {
    // A frame held back and let go of reaches the grid all at once, however
    // many lines it scrolls: Claude Code draws every frame that way. The
    // grid's history goes past its share of the scrollback by all of them
    // before anything can cut it back, and none of them may go uncounted.
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 40));
    let lines: String = (0..crate::blocks::HISTORY_HEADROOM + 100)
        .map(|n| format!("frame {n}\r\n"))
        .collect();
    twins.feed(format!("\x1b[?2026h{lines}\x1b[?2026l"));
    twins.alike("after the update");
    let (rows, _) = twins.spilling.harvest_rows(0, 1);
    assert_eq!("red 0\nblue 1", rows.to_text());
}
