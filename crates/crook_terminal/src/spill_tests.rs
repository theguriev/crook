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
        Self::with_grid(scrollback, GRID)
    }

    /// Twins whose spilling one keeps `grid` lines in its grid.
    fn with_grid(scrollback: usize, grid: usize) -> Self {
        let size = TerminalSize::new(COLUMNS, ROWS).with_cell_size(8, 16);
        Self {
            spilling: Emulator::with_grid_lines(size, scrollback, grid, Palette::default()),
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

    fn bottom(&mut self) {
        self.spilling.scroll_to_bottom();
        self.whole.scroll_to_bottom();
    }

    fn resize(&mut self, columns: u16, rows: u16) {
        let size = TerminalSize::new(columns, rows).with_cell_size(8, 16);
        self.spilling.resize(size);
        self.whole.resize(size);
    }

    /// Holds that nothing tells the two apart, and that the spilling one
    /// keeps no more than its share of lines in its grid while doing it.
    fn alike(&mut self, what: &str) {
        let grid = self.spilling.term.grid().history_size();
        assert!(
            grid <= self.spilling.grid_lines,
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
                (*block.rows).clone(),
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

#[test]
fn lines_printed_after_an_erase_in_the_same_read_are_kept() {
    // `clear` and Claude Code's redraw erase the scrollback and print on in
    // one write, and a pty hands the whole of it over in one read: the
    // erase is read before every line after it, and drops only what came
    // before it.
    let lines = || output(1000, 1100);
    let shapes = [
        ("CSI 3 J", format!("\x1b[H\x1b[2J\x1b[3J{}", lines())),
        ("ESC c", format!("\x1bc{}", lines())),
        (
            "a synchronized redraw",
            format!("\x1b[?2026h\x1b[H\x1b[2J\x1b[3J{}\x1b[?2026l", lines()),
        ),
    ];
    for (shape, bytes) in shapes {
        let mut twins = Twins::new(1000);
        twins.feed(output(0, 80));
        twins.feed(&bytes);
        twins.alike(&format!("after {shape}"));
        let (rows, _) = twins.spilling.harvest_rows(0, 0);
        assert!(
            rows.to_text().starts_with("line 1000"),
            "{shape}: the scrollback starts at {:?}",
            rows.to_text()
        );
    }
}

#[test]
fn a_taller_or_shorter_screen_leaves_a_view_in_the_spilled_rows_where_it_was() {
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 120));
    twins.scroll(40);
    twins.alike("scrolled up");
    for rows in [8, 3, 5, 9, 2] {
        twins.resize(COLUMNS, rows);
        twins.alike(&format!("at {rows} rows"));
    }
}

#[test]
fn a_view_in_the_spilled_rows_is_where_it_was_after_a_full_screen_program() {
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 120));
    twins.scroll(40);
    twins.alike("scrolled up");
    twins.feed("\x1b[?1049h\x1b[Hvim");
    twins.alike("under vim");
    twins.feed("\x1b[?1049l");
    twins.alike("after vim");
}

#[test]
fn a_column_change_leaves_a_view_in_the_spilled_rows_on_the_line_it_showed() {
    let mut emulator =
        Emulator::with_grid_lines(TerminalSize::new(30, 5), 1000, GRID, Palette::default());
    let lines: String = (0..200)
        .map(|n| format!("entry {n} {}\r\n", "x".repeat(n % 40)))
        .collect();
    emulator.advance(lines.as_bytes());
    for (scroll, columns) in [(60, 17), (30, 41), (90, 23), (45, 30)] {
        emulator.scroll_to_bottom();
        emulator.scroll_lines(scroll);
        let before = emulator.snapshot().text();
        let first = before.lines().find(|line| line.starts_with("entry"));
        let Some(first) = first.and_then(|line| line.split(' ').nth(1)) else {
            continue;
        };
        let first = format!("entry {first} ");
        emulator.resize(TerminalSize::new(columns, 5));
        let after = emulator.snapshot();
        assert!(
            after.display_offset > emulator.term.grid().history_size(),
            "at {columns} columns the view left the spilled rows"
        );
        assert!(
            after.text().contains(&first),
            "at {columns} columns the view moved off {first:?}: {:?}, was {before:?}",
            after.text()
        );
    }
}

/// Log lines the way a build or a server prints them: a timestamp, a
/// coloured level, and a message, in pty-sized reads.
fn log_reads(lines: usize) -> Vec<Vec<u8>> {
    let mut text = Vec::new();
    for n in 0..lines {
        let level = if n % 5 == 0 {
            "\x1b[33mWARN\x1b[0m"
        } else {
            "\x1b[32mINFO\x1b[0m"
        };
        text.extend_from_slice(
            format!(
                "2026-10-21T12:{:02}:{:02}Z {level} crook::server: request {n} handled in {}ms\r\n",
                n / 60 % 60,
                n % 60,
                n % 97
            )
            .as_bytes(),
        );
    }
    text.chunks(4096).map(<[u8]>::to_vec).collect()
}

/// Release-mode timings, for the pull request:
/// `cargo test --release -p crook_terminal --lib bench_ -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark; run it in release mode"]
fn bench_feeding_and_resizing_a_long_open_block() {
    use std::time::Instant;
    let reads = log_reads(200_000);
    for columns in [80, 200] {
        let size = TerminalSize::new(columns, 50);
        for (name, grid) in [("grid keeps all", 10_000), ("spilling", GRID_LINES)] {
            let mut best = f64::MAX;
            for _ in 0..3 {
                let mut emulator =
                    Emulator::with_grid_lines(size, 10_000, grid, Palette::default());
                let start = Instant::now();
                for read in &reads {
                    emulator.advance(read);
                }
                best = best.min(start.elapsed().as_secs_f64() * 1000.);
            }
            println!("feed 200k lines at {columns} columns, {name}: {best:.0} ms");
        }
    }
    for (name, grid) in [("grid keeps all", 12_000), ("spilling", GRID_LINES)] {
        let size = TerminalSize::new(200, 50);
        let mut emulator = Emulator::with_grid_lines(size, 12_000, grid, Palette::default());
        for read in log_reads(12_100) {
            emulator.advance(&read);
        }
        let start = Instant::now();
        for step in 0..40u16 {
            emulator.resize(TerminalSize::new(200 - step % 2, 50));
        }
        println!(
            "column change at 12k rows x 200, {name}: {:.2} ms each",
            start.elapsed().as_secs_f64() * 1000. / 40.
        );
    }
    // A line of 51 KB, 256 rows at 200 columns, across the seam: the
    // whole of its rest in the grid is gathered up on every column change.
    for (name, grid) in [("grid keeps all", 12_000), ("spilling", GRID_LINES)] {
        let size = TerminalSize::new(200, 50);
        let mut emulator = Emulator::with_grid_lines(size, 12_000, grid, Palette::default());
        for read in log_reads(12_000) {
            emulator.advance(&read);
        }
        let line: String = (0..12_800).map(|n| format!("{:04}", n % 10_000)).collect();
        emulator.advance(format!("{line}\r\n").as_bytes());
        for read in log_reads(300) {
            emulator.advance(&read);
        }
        let start = Instant::now();
        for step in 0..40u16 {
            emulator.resize(TerminalSize::new(200 - step % 2, 50));
        }
        println!(
            "column change with a 51 KB line across the seam, {name}: {:.2} ms each",
            start.elapsed().as_secs_f64() * 1000. / 40.
        );
    }
}

/// Release-mode timings of what costs the spill the most against a grid
/// that keeps the whole scrollback: lines much shorter than the pane, and
/// escapes that scroll a screenful in a few bytes. Best of three, and the
/// slowest single 4 KiB read of the best run, which is how long the reader
/// holds the terminal's lock at worst:
/// `cargo test --release -p crook_terminal --lib bench_short -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark; run it in release mode"]
fn bench_short_lines_and_screenful_scrolls() {
    use std::time::Instant;
    let seq: Vec<u8> = (1..=200_000)
        .flat_map(|n| format!("{n}\r\n").into_bytes())
        .collect();
    let hello: Vec<u8> = (1..=200_000)
        .flat_map(|n| format!("hello world line {n}\r\n").into_bytes())
        .collect();
    let scrolls = "\x1b[65535S".repeat(10_000).into_bytes();
    let cases: [(&str, &[u8], u16, u16, usize); 6] = [
        ("seq 200k", &seq, 80, 24, 10_000),
        ("seq 200k", &seq, 200, 50, 10_000),
        ("hello world 200k", &hello, 80, 24, 10_000),
        ("hello world 200k", &hello, 200, 50, 10_000),
        ("10k x CSI 65535 S", &scrolls, 200, 50, 10_000),
        (
            "10k x CSI 65535 S, scrollback 1000",
            &scrolls,
            200,
            50,
            1000,
        ),
    ];
    for (what, bytes, columns, rows, scrollback) in cases {
        for (name, grid) in [("grid keeps all", scrollback), ("spilling", GRID_LINES)] {
            let (mut best, mut worst_read) = (f64::MAX, 0.);
            for _ in 0..3 {
                let mut emulator = Emulator::with_grid_lines(
                    TerminalSize::new(columns, rows),
                    scrollback,
                    grid,
                    Palette::default(),
                );
                let mut worst = 0f64;
                let start = Instant::now();
                for read in bytes.chunks(4096) {
                    let at = Instant::now();
                    emulator.advance(read);
                    worst = worst.max(at.elapsed().as_secs_f64() * 1000.);
                }
                let took = start.elapsed().as_secs_f64() * 1000.;
                if took < best {
                    (best, worst_read) = (took, worst);
                }
            }
            println!(
                "{what} at {columns}x{rows}, {name}: {best:.1} ms, worst read {worst_read:.2} ms"
            );
        }
    }
}

/// Release-mode timings of floods of screenful scrolls across screen
/// heights and scrollbacks, spilling: best of three, and the slowest 4 KiB
/// read of the best run:
/// `cargo test --release -p crook_terminal --lib bench_scroll_floods -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark; run it in release mode"]
fn bench_scroll_floods_across_screens_and_scrollbacks() {
    use std::time::Instant;
    let su = "\x1b[65535S".repeat(10_000).into_bytes();
    let su999 = "\x1b[999S".repeat(20_000).into_bytes();
    let cases: [(&str, &[u8], u16, u16, usize); 10] = [
        ("10k x CSI 65535 S", &su, 80, 24, 2000),
        ("10k x CSI 65535 S", &su, 200, 50, 1000),
        ("10k x CSI 65535 S", &su, 200, 50, 2000),
        ("10k x CSI 65535 S", &su, 200, 50, 10_000),
        ("10k x CSI 65535 S", &su, 200, 100, 2000),
        ("10k x CSI 65535 S", &su, 200, 100, 10_000),
        ("10k x CSI 65535 S", &su, 200, 135, 10_000),
        ("20k x CSI 999 S", &su999, 200, 200, 2000),
        ("10k x CSI 65535 S", &su, 300, 400, 10_000),
        ("20k x CSI 999 S", &su999, 300, 400, 10_000),
    ];
    for (what, bytes, columns, rows, scrollback) in cases {
        let (mut best, mut worst_read) = (f64::MAX, 0.);
        for _ in 0..3 {
            let mut emulator = Emulator::new(
                TerminalSize::new(columns, rows),
                scrollback,
                Palette::default(),
            );
            let mut worst = 0f64;
            let start = Instant::now();
            for read in bytes.chunks(4096) {
                let at = Instant::now();
                emulator.advance(read);
                worst = worst.max(at.elapsed().as_secs_f64() * 1000.);
            }
            let took = start.elapsed().as_secs_f64() * 1000.;
            if took < best {
                (best, worst_read) = (took, worst);
            }
        }
        println!(
            "{what} at {columns}x{rows}, scrollback {scrollback}: {best:.0} ms, worst read {worst_read:.1} ms"
        );
    }
}

/// Ten thousand rows of what an agent's session prints: prose wrapped at
/// the pane, coloured tool calls, box-drawn panels and diff lines.
fn agent_session(rows: usize) -> String {
    let mut out = String::new();
    for n in 0..rows {
        match n % 6 {
            0 => out.push_str(&format!(
                "\x1b[38;5;174m●\x1b[0m \x1b[1mBash\x1b[0m(cargo test -p crook_terminal spill {n})\r\n"
            )),
            1 => out.push_str("  ⎿  \x1b[2mRunning 461 tests … ok\x1b[0m\r\n"),
            2 => out.push_str(&format!(
                "\x1b[38;5;246m╭{}╮\x1b[0m\r\n",
                "─".repeat(60)
            )),
            3 => out.push_str(&format!(
                "\x1b[48;5;22m+ let spilled = self.blocks.spilled_rows(&self.term); // {n}\x1b[0m\r\n"
            )),
            4 => out.push_str(&format!(
                "The grid keeps a cell of 24 bytes for every column of every row, printed into or not, row {n}.\r\n"
            )),
            _ => out.push_str("\r\n"),
        }
    }
    out
}

/// This process's resident set, in KiB.
fn rss_kib() -> usize {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|statm| statm.split_whitespace().nth(1)?.parse::<usize>().ok())
        .map_or(0, |pages| pages * 4)
}

/// Resident memory of ten 200x50 panes, each one open block of ten
/// thousand agent rows. One configuration a process, for the allocator's
/// sake:
/// `cargo test --release -p crook_terminal --lib bench_rss -- --ignored --nocapture --exact emulator::spill_tests::bench_rss_spilling`
/// and the same with `bench_rss_grid_keeps_all`.
fn rss_of_ten_panes(grid: usize) {
    let session = agent_session(10_000);
    let before = rss_kib();
    let panes: Vec<Emulator> = (0..10)
        .map(|_| {
            let mut emulator = Emulator::with_grid_lines(
                TerminalSize::new(200, 50),
                10_000,
                grid,
                Palette::default(),
            );
            for read in session.as_bytes().chunks(4096) {
                emulator.advance(read);
            }
            emulator
        })
        .collect();
    let after = rss_kib();
    let spilled: usize = panes
        .iter()
        .map(|pane| pane.blocks.spilled().memory_usage())
        .sum();
    let rows: usize = panes.iter().map(|pane| pane.blocks.spilled().rows()).sum();
    println!(
        "grid {grid}: {:.1} MiB a pane resident; spilled {} rows at {} B a row",
        (after - before) as f64 / 1024. / 10.,
        rows / 10,
        spilled.checked_div(rows).unwrap_or(0)
    );
}

#[test]
#[ignore = "a measurement; run it in release mode, alone"]
fn bench_rss_spilling() {
    rss_of_ten_panes(GRID_LINES);
}

#[test]
#[ignore = "a measurement; run it in release mode, alone"]
fn bench_rss_grid_keeps_all() {
    rss_of_ten_panes(10_000);
}

/// Resident memory of a pane's finished blocks — two hundred commands of
/// two thousand log lines at 120 columns — alone, with a list of them held
/// the way the application holds one, and with the copy of every block the
/// application used to make:
/// `cargo test --release -p crook_terminal --lib bench_rss_finished -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement; run it in release mode, alone"]
fn bench_rss_finished_blocks_and_a_list_of_them() {
    let mut emulator = Emulator::with_grid_lines(
        TerminalSize::new(120, 40),
        10_000,
        GRID_LINES,
        Palette::default(),
    );
    let before = rss_kib();
    let lines = log_reads(2000).concat();
    for _ in 0..200 {
        emulator.advance(format!("{A}$ {B}make\r\n{C}").as_bytes());
        emulator.advance(&lines);
        emulator.advance(b"\x1b]133;D;0\x07");
    }
    let alone = rss_kib();
    let rows: usize = emulator
        .blocks()
        .iter()
        .map(|block| block.rows.rows())
        .sum();
    let bytes: usize = emulator
        .blocks()
        .iter()
        .map(|block| block.rows.memory_usage())
        .sum();
    let shared = emulator.blocks().to_vec();
    let with_list = rss_kib();
    let copied: Vec<Block> = emulator
        .blocks()
        .iter()
        .map(|block| Block {
            rows: Arc::new((*block.rows).clone()),
            ..(**block).clone()
        })
        .collect();
    let with_copies = rss_kib();
    println!(
        "{} blocks, {rows} rows, {:.1} MiB of rows; resident {:.0} MB alone, {:.0} MB with a list of them, {:.0} MB with a copy of every block",
        shared.len(),
        bytes as f64 / 1048576.,
        (alone - before) as f64 / 1024.,
        (with_list - before) as f64 / 1024.,
        (with_copies - before) as f64 / 1024.,
    );
    drop((shared, copied));
}

#[test]
fn a_picture_spilled_with_its_rows_stays_on_them_through_a_long_read() {
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 30));
    twins.feed(picture());
    twins.feed("\r\nBELOW\r\n");
    let read: String = (0..crate::blocks::HISTORY_HEADROOM * 3)
        .map(|line| format!("more {line}\r\n"))
        .collect();
    twins.feed(&read);
    twins.alike("after the read");
    assert_eq!(1, twins.spilling.live_block().images.len());
}

#[test]
fn a_view_in_the_spilled_rows_outlasts_a_full_screen_program_of_many_reads() {
    // vim draws over many reads, and the pane is resized under it.
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 120));
    twins.scroll(40);
    twins.feed("\x1b[?1049h\x1b[Hvim");
    for key in ["j", "jj", "\x1b[2;1Hdd", "y"] {
        // A key typed into the pane takes the view on show to the bottom,
        // and that is vim's.
        twins.bottom();
        twins.scroll(3);
        twins.feed(key);
        twins.alike(&format!("under vim, after {key:?}"));
    }
    twins.feed("\x1b[?1049l");
    twins.alike("after vim");

    // And across a column change under it, on the line it showed.
    let mut emulator = twins.spilling;
    emulator.scroll_to_bottom();
    emulator.scroll_lines(40);
    let before = emulator.snapshot().text();
    let head = before
        .lines()
        .next()
        .unwrap_or_default()
        .split(' ')
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    emulator.advance(b"\x1b[?1049h\x1b[Hvim");
    emulator.advance(b"j");
    emulator.resize(TerminalSize::new(27, ROWS).with_cell_size(8, 16));
    emulator.advance(b"j");
    emulator.advance(b"\x1b[?1049l");
    let after = emulator.snapshot();
    assert!(
        after.display_offset > emulator.grid_history(),
        "the view left the spilled rows across a column change under vim"
    );
    assert!(
        after.text().contains(&head),
        "the view moved off {head:?} under vim: {:?}, was {before:?}",
        after.text()
    );
}

#[test]
fn a_read_that_scrolls_the_screen_by_its_height_at_a_time_leaves_no_hole() {
    // `CSI n S` scrolls a screenful in five bytes, and a slice is cut for
    // a line a byte: a read of them can scroll the history past alacritty's
    // room before anything cuts it back, and the lines it evicts were never
    // spilled. The scrollback must not lose lines from its middle.
    for (scrollback, rows) in [(5000, 50), (10_000, 100), (10_000, 200)] {
        let size = TerminalSize::new(20, rows).with_cell_size(8, 16);
        let mut spilling = Emulator::new(size, scrollback, Palette::default());
        let lines: String = (0..1200).map(|n| format!("L{n}\r\n")).collect();
        spilling.advance(lines.as_bytes());
        // As many as one slice of output holds.
        let scroll = format!("\x1b[{}S", rows - 1);
        let burst = scroll.repeat(500 / scroll.len());
        spilling.advance(burst.as_bytes());
        spilling.advance(b"after\r\n");
        let history = spilling.history_len();
        let (kept, _) = spilling.harvest_rows(0, history + usize::from(rows));
        let numbers: Vec<usize> = kept
            .to_text()
            .lines()
            .filter_map(|line| line.strip_prefix('L')?.parse().ok())
            .collect();
        assert!(
            numbers.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "{scrollback} lines, {rows} rows: a hole in {numbers:?}"
        );
        assert!(
            numbers.last().is_none_or(|last| *last == 1199),
            "{scrollback} lines, {rows} rows: the newest went: {numbers:?}"
        );
        assert!(
            history >= scrollback,
            "{scrollback} lines, {rows} rows: only {history} kept"
        );
    }
}

#[test]
fn a_line_longer_than_the_headroom_across_the_seam_stays_one_line() {
    // Minified code, base64, a long JSON document: one line over hundreds
    // of rows, its head spilled and the rest of it the grid's whole history.
    // A column change lays it out again whole, so a copy of it has no line
    // break in it.
    let mut twins = Twins::with_grid(2000, 400);
    twins.feed(output(0, 20));
    let line: String = (0..3500).map(|n| format!("{:04}", n % 10_000)).collect();
    twins.feed(format!("{line}\r\n"));
    // Off the screen, so that what the grid holds of it is all history.
    twins.feed(output(20, 26));
    let spill = twins.spilling.blocks.spilled();
    assert!(
        !spill.is_empty() && spill.wraps(spill.rows() - 1),
        "the premise: the line crosses the seam"
    );
    assert!(
        twins.spilling.grid_history() > crate::blocks::HISTORY_HEADROOM,
        "the premise: more of it is in the grid than the headroom"
    );
    twins.resize(13, ROWS);
    twins.alike("at 13 columns");
}

#[test]
fn a_much_taller_screen_pulls_its_rows_back_from_the_spill() {
    // A taller screen takes its new rows from the history, and the grid has
    // to hold enough of it for them: its share follows the screen's height.
    let mut twins = Twins::new(1000);
    twins.feed(output(0, 300));
    for rows in [9, 17, 33, 60, 7] {
        twins.resize(COLUMNS, rows);
        twins.alike(&format!("at {rows} rows"));
        twins.feed(output(1000, 1040));
        twins.alike(&format!("at {rows} rows, after more output"));
    }
}

#[test]
fn a_column_change_leaves_a_view_in_the_grids_history_on_the_line_it_showed() {
    let mut emulator =
        Emulator::with_grid_lines(TerminalSize::new(30, 5), 1000, 100, Palette::default());
    let lines: String = (0..300)
        .map(|n| format!("entry {n} {}\r\n", "x".repeat(n % 40)))
        .collect();
    emulator.advance(lines.as_bytes());
    for (scroll, columns) in [(30, 17), (12, 41), (20, 23), (25, 30)] {
        emulator.scroll_to_bottom();
        emulator.scroll_lines(scroll);
        let before = emulator.snapshot();
        assert!(
            before.display_offset <= emulator.grid_history(),
            "the premise: {} in {}",
            before.display_offset,
            emulator.grid_history()
        );
        let first = before
            .text()
            .lines()
            .find(|line| line.starts_with("entry"))
            .map(str::to_owned);
        let Some(first) = first.and_then(|line| line.split(' ').nth(1).map(str::to_owned)) else {
            continue;
        };
        let first = format!("entry {first} ");
        emulator.resize(TerminalSize::new(columns, 5));
        let after = emulator.snapshot().text();
        assert!(
            after.contains(&first),
            "at {columns} columns the view moved off {first:?}: {after:?}"
        );
    }
}

#[test]
fn an_update_that_scrolls_the_shell_past_its_room_under_a_program_leaves_no_hole() {
    // A synchronized update can put the shell's screen up, scroll it by more
    // than alacritty has room for, and take it down again, all at once; and a
    // taller screen then pulls rows back out of the history, which hides
    // that it ever went past its room. The spilled rows above what alacritty
    // evicted must not be kept as though they joined on.
    let size = |rows| TerminalSize::new(20, rows).with_cell_size(8, 16);
    let mut emulator = Emulator::new(size(50), 10_000, Palette::default());
    let lines: String = (0..1200).map(|n| format!("L{n}\r\n")).collect();
    emulator.advance(lines.as_bytes());
    emulator.advance(b"\x1b[?1049hvim");
    let blank: String = (0..9_850).map(|n| format!("B{n}\r\n")).collect();
    emulator.advance(format!("\x1b[?2026h\x1b[?1049l{blank}\x1b[?1049h\x1b[?2026l").as_bytes());
    emulator.resize(size(120));
    emulator.advance(b"\x1b[?1049l");
    emulator.advance(b"after\r\n");
    let (kept, _) = emulator.harvest_rows(0, emulator.history_len() + 120);
    let text = kept.to_text();
    for prefix in ['L', 'B'] {
        let numbers: Vec<usize> = text
            .lines()
            .filter_map(|line| line.strip_prefix(prefix)?.parse().ok())
            .collect();
        assert!(
            numbers.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "a hole in the {prefix} lines: {numbers:?}"
        );
    }
    let last_l = text.lines().rfind(|line| line.starts_with('L'));
    assert!(
        last_l.is_none_or(|line| line == "L1199"),
        "older lines kept above a hole, the last of them {last_l:?}"
    );
}

#[test]
fn a_program_that_switches_screens_in_an_update_and_a_taller_pane_keep_the_scrollback() {
    // Neovim draws inside synchronized updates, its own screen switches
    // among them, and `:!cmd` puts the shell's screen up for a line in one.
    // Neither scrolls the shell's history anywhere near its room, and a
    // taller pane under the program afterwards loses nothing.
    let size = |rows| TerminalSize::new(40, rows).with_cell_size(8, 16);
    let shapes = [
        (
            "an update that goes up",
            "\x1b[?2026h\x1b[?1049h\x1b[Hnvim\x1b[?2026l".to_owned(),
        ),
        (
            "`:!cmd` inside one",
            "\x1b[?1049h\x1b[Hnvim\x1b[?2026h\x1b[?1049l$ ls\r\nfile\r\n\x1b[?1049h\x1b[?2026l"
                .to_owned(),
        ),
    ];
    for (shape, bytes) in shapes {
        let mut emulator = Emulator::new(size(50), 10_000, Palette::default());
        let lines: String = (0..12_000).map(|n| format!("L{n}\r\n")).collect();
        emulator.advance(lines.as_bytes());
        let before = emulator.history_len();
        emulator.advance(bytes.as_bytes());
        emulator.resize(size(60));
        emulator.advance(b"\x1b[?1049l");
        assert!(
            emulator.history_len() + 50 >= before,
            "{shape}: the scrollback went from {before} to {} lines",
            emulator.history_len()
        );
    }
}

#[test]
fn a_column_change_under_a_program_leaves_the_shells_view_on_its_line() {
    for grid in [12, 100, 500] {
        let mut twins = Twins::with_grid(3000, grid);
        // Lines that fold at the narrower width, so that the grid's history
        // grows in the reflow under the program.
        twins.feed(output(0, 1400));
        twins.scroll(1500);
        twins.alike("scrolled up");
        let top = twins
            .whole
            .snapshot()
            .text()
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        twins.feed("\x1b[?1049h\x1b[Hvim");
        twins.resize(13, ROWS);
        twins.feed("j");
        twins.feed("\x1b[?1049l");
        let shown = twins.spilling.snapshot().text();
        assert!(
            shown.lines().next() == Some(top.as_str()),
            "grid {grid}: the view moved off {top:?} to {:?}",
            shown.lines().next()
        );
        // Output in the same read as the switches moves nothing either.
        let mut twins = Twins::with_grid(3000, grid);
        twins.feed(output(0, 1400));
        twins.scroll(1500);
        twins.feed("\x1b[?1049h\x1b[Hvim");
        twins.feed("\x1b[?1049lback\r\nmore\r\n\x1b[?1049h\x1b[Hvim\x1b[?1049l");
        twins.alike(&format!(
            "grid {grid}: after output between switches in one read"
        ));
    }
}

/// What [`fuzz`] found over its seeds: checks made, checks where the
/// spilling twin kept fewer of the lines printed than the other, by more
/// than a taller screen can pull out of a full history, and checks where it was missing a
/// numbered line the other kept, between two it kept itself.
#[derive(Debug, Default)]
struct Fuzzed {
    checks: usize,
    short: usize,
    holes: usize,
}

/// Feeds both twins the same random session — numbered lines, bursts of
/// `CSI n S`, full-screen programs entered and left plainly and inside
/// synchronized updates, the shell's screen put up for a line inside one,
/// erases, scrolls and resizes of both kinds — and compares the two after
/// every step.
fn fuzz(seeds: std::ops::Range<u64>, scrollback: usize, grid: usize) -> Fuzzed {
    let mut found = Fuzzed::default();
    for seed in seeds {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut next = |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below.max(1)
        };
        let mut twins = Twins::with_grid(scrollback, grid);
        let (mut line, mut up, mut rows) = (0u64, false, ROWS);
        for _ in 0..60 {
            let bytes = match next(10) {
                0..=2 => {
                    let lines: String = (0..1 + next(400))
                        .map(|_| {
                            line += 1;
                            format!("N{line}\r\n")
                        })
                        .collect();
                    lines
                }
                3 => format!("\x1b[{}S", 1 + next(u64::from(rows))).repeat(1 + next(200) as usize),
                4 if up => {
                    up = false;
                    "\x1b[?1049l".to_owned()
                }
                4 => {
                    up = true;
                    "\x1b[?1049h\x1b[Hvim".to_owned()
                }
                5 if up => {
                    // `:!cmd`, mostly short, sometimes long enough to scroll
                    // the shell's history past its room in one go.
                    let count = if next(4) == 0 {
                        (scrollback - grid) as u64 + next(800)
                    } else {
                        1 + next(3)
                    };
                    let lines: String = (0..count)
                        .map(|_| {
                            line += 1;
                            format!("N{line}\r\n")
                        })
                        .collect();
                    format!("\x1b[?2026h\x1b[?1049l{lines}\x1b[?1049h\x1b[?2026l")
                }
                5 => {
                    up = true;
                    "\x1b[?2026h\x1b[?1049h\x1b[Hnvim\x1b[?2026l".to_owned()
                }
                6 => {
                    rows = 2 + next(60) as u16;
                    twins.resize(COLUMNS, rows);
                    String::new()
                }
                7 => {
                    twins.resize(10 + next(30) as u16, rows);
                    String::new()
                }
                8 => {
                    twins.scroll(next(2000) as i32 - 1000);
                    String::new()
                }
                _ if next(8) == 0 => "\x1b[3J".to_owned(),
                // A key for the program, which the shell's screen never sees.
                _ if up => "y".to_owned(),
                _ => String::new(),
            };
            twins.feed(&bytes);
            if up {
                continue;
            }
            found.checks += 1;
            let (spilling, whole) = (twins.spilling.history_len(), twins.whole.history_len());
            // A line the other twin still has, between the oldest and the
            // newest line this one has, missing here: a hole. The session
            // can leave gaps of its own — a cursor a full-screen program
            // gave back above lines printed under it — and those are in both.
            let numbers = |emulator: &Emulator, history: usize| {
                let (kept, _) = emulator.harvest_rows(0, history + 100);
                kept.to_text()
                    .lines()
                    .filter_map(|text| text.strip_prefix('N')?.parse().ok())
                    .collect::<std::collections::BTreeSet<u64>>()
            };
            let (here, there) = (
                numbers(&twins.spilling, spilling),
                numbers(&twins.whole, whole),
            );
            // Short: fewer of the lines printed kept than the other twin
            // keeps, by more than a taller screen can pull out of a full
            // history. The blank rows a block opened on above its first are
            // not kept on purpose, and are not counted.
            if here.len() + 60 < there.len() {
                found.short += 1;
            }
            if let (Some(first), Some(last)) = (here.first(), here.last())
                && there
                    .range(first..=last)
                    .any(|number| !here.contains(number))
            {
                found.holes += 1;
            }
        }
    }
    found
}

#[test]
fn random_sessions_keep_the_scrollback_whole_and_long() {
    for (scrollback, grid) in [(600, GRID), (3000, 100)] {
        let found = fuzz(0..20, scrollback, grid);
        assert!(
            found.short == 0 && found.holes == 0,
            "scrollback {scrollback}, grid {grid}: {found:?}"
        );
    }
}

/// The same, over many more seeds:
/// `cargo test --release -p crook_terminal --lib bench_fuzz -- --ignored --nocapture`.
#[test]
#[ignore = "a long run; run it in release mode"]
fn bench_fuzz_random_sessions() {
    for (scrollback, grid) in [(10_000, GRID_LINES), (600, GRID), (3000, 100)] {
        let found = fuzz(0..400, scrollback, grid);
        println!("fuzz, scrollback {scrollback}, grid {grid}: {found:?}");
    }
}

#[test]
fn a_taller_and_shorter_pane_in_turn_under_a_program_keep_the_scrollback() {
    // A pane resized back and forth under a program pulls rows out of the
    // shell's history and pushes them back as often: what it is short of
    // full by is what it pulled less what it pushed, not every row it ever
    // pulled.
    let size = |rows| TerminalSize::new(20, rows).with_cell_size(8, 16);
    let mut emulator = Emulator::with_grid_lines(size(5), 600, GRID, Palette::default());
    let lines: String = (0..1000).map(|n| format!("L{n}\r\n")).collect();
    emulator.advance(lines.as_bytes());
    let before = emulator.history_len();
    emulator.advance(b"\x1b[?1049h\x1b[Hvim\x1b[?2026h\x1b[?1049l$ ls\r\n\x1b[?1049h\x1b[?2026l");
    for _ in 0..30 {
        emulator.resize(size(55));
        emulator.resize(size(5));
    }
    emulator.advance(b"\x1b[?1049l");
    assert!(
        emulator.history_len() + 10 >= before,
        "the scrollback went from {before} to {} lines",
        emulator.history_len()
    );
}

#[test]
fn an_overrun_reaches_the_block_top_however_the_block_was_found() {
    // A block whose first row a column change found again while its history
    // was blank, so that it is anchored to a line of the screen: an update
    // that scrolls the shell past its room then puts that anchor below the
    // lines it scrolled, and the next cut must not take them for rows above
    // the block.
    let mut twins = Twins::with_grid(10_000, 500);
    twins.feed("\x1b[4S".repeat(123));
    twins.resize(19, ROWS);
    twins.feed("\x1b[?1049h\x1b[Hvim");
    let lines: String = (0..10_000).map(|n| format!("N{n}\r\n")).collect();
    twins.feed(format!(
        "\x1b[?2026h\x1b[?1049l{lines}\x1b[?1049h\x1b[?2026l"
    ));
    twins.resize(19, 21);
    twins.feed("\x1b[?1049l");
    let numbered = |emulator: &Emulator| {
        let (kept, _) = emulator.harvest_rows(0, emulator.history_len() + 30);
        kept.to_text()
            .lines()
            .filter(|line| line.starts_with('N'))
            .count()
    };
    let (here, there) = (numbered(&twins.spilling), numbered(&twins.whole));
    assert!(here + 60 >= there, "{here} lines kept against {there}");
}

#[test]
fn harvesting_past_the_room_leaves_what_harvesting_and_dropping_would() {
    // Every layout of pieces a cut can meet, every cut and every room, done
    // the long way — harvest everything, drop the oldest pieces — and the
    // short way, which harvests none of the rows the drop would let go of.
    let size = TerminalSize::new(30, 6);
    let mut emulator = Emulator::with_grid_lines(size, 4000, 4000, Palette::default());
    emulator.advance(output(0, 1500).as_bytes());
    let (term, palette) = (&emulator.term, &emulator.palette);
    let oldest = -(term.grid().history_size() as i32);
    let mut cases = 0;
    for held in [0, 1, 7, 8, 9, 23, 40] {
        for piece in [1, 3, 8, 64] {
            for cut in [1, 5, 8, 31, 100, 400] {
                for keep in [1, 4, 8, 17, 64, 300] {
                    let mut long = crate::spill::Spill::default();
                    long.harvest(term, palette, (oldest, oldest + held - 1), piece);
                    let mut short = crate::spill::Spill::default();
                    short.harvest(term, palette, (oldest, oldest + held - 1), piece);
                    let range = (oldest + held, oldest + held + cut - 1);
                    long.harvest(term, palette, range, piece);
                    let dropped = long.drop_oldest(keep);
                    let what = format!("held {held}, piece {piece}, cut {cut}, keep {keep}");
                    assert_eq!(
                        dropped,
                        short.harvest_keeping(term, palette, range, piece, keep),
                        "{what}: rows dropped"
                    );
                    assert_eq!(long.piece_sizes(), short.piece_sizes(), "{what}: pieces");
                    assert_eq!(long.rows(), short.rows(), "{what}: rows");
                    assert_eq!(long.take(), short.take(), "{what}: the rows kept");
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 7 * 4 * 6 * 6);
}

#[test]
fn a_cut_longer_than_the_scrollback_harvests_only_the_rows_it_keeps() {
    // A short scrollback and a read of `CSI n S` that scrolls ten times as
    // many lines as it holds: the rows that would be dropped as soon as they
    // were harvested are not harvested.
    let mut twins = Twins::new(60);
    twins.feed(output(0, 40));
    let before = twins.spilling.blocks.spilled().harvested;
    let burst = format!("\x1b[{ROWS}S").repeat(120);
    twins.feed(&burst);
    twins.feed(output(40, 45));
    twins.alike("after the burst");
    let harvested = twins.spilling.blocks.spilled().harvested - before;
    let scrolled = 120 * usize::from(ROWS);
    assert!(
        harvested < scrolled / 4,
        "{harvested} rows harvested of the {scrolled} scrolled, for a scrollback of 60"
    );
}

#[test]
fn a_flood_of_screenful_scrolls_is_fed_in_slices_that_scroll_no_more_than_a_cut_keeps() {
    // `CSI 65535 S` scrolls a screenful in nine bytes. Fed in slices cut for
    // a line a byte, each slice scrolls thousands of lines into the grid's
    // history, and cutting them back makes alacritty give back the rows
    // past its thousand spare and allocate them again for the next slice,
    // every slice: sixty times what the same flood costs a grid keeping
    // the whole scrollback. After the first slice the rest are shorter.
    let mut emulator = Emulator::new(TerminalSize::new(200, 50), 10_000, Palette::default());
    emulator.advance(b"before\r\n");
    for read in "\x1b[65535S".repeat(10_000).as_bytes().chunks(4096) {
        emulator.advance(read);
    }
    emulator.advance(b"after\r\n");
    assert!(
        emulator.overgrown_slices <= 1,
        "{} slices scrolled past alacritty's spare rows",
        emulator.overgrown_slices
    );
    assert!(emulator.history_len() >= 10_000);
    assert!(emulator.snapshot().text().contains("after"));
}

#[test]
fn a_flood_on_a_tall_screen_over_a_short_scrollback_is_fed_in_whole_slices() {
    // `CSI 999 S` on two hundred rows scrolls nearly thirty lines a byte,
    // and a whole slice of it ten times the rows a scrollback of two
    // thousand keeps. Short slices would harvest every one of those rows;
    // a whole one harvests the ones kept, and that is cheaper than the rows
    // alacritty frees and allocates again for it. A whole slice scrolls to
    // the end of alacritty's room and counts less than it scrolled, which
    // must not be taken for a slower flood, or the slices go short and
    // whole by turns.
    let mut emulator = Emulator::new(TerminalSize::new(200, 200), 2000, Palette::default());
    let flood = "\x1b[999S".repeat(20_000);
    for read in flood.as_bytes().chunks(4096) {
        emulator.advance(read);
    }
    let whole = flood.len().div_ceil(511);
    assert!(
        emulator.fed_slices * 2 <= whole * 3,
        "{} slices for {} bytes, {whole} of them whole",
        emulator.fed_slices,
        flood.len()
    );
}

#[test]
fn a_flood_a_whole_slice_of_which_scrolls_about_twice_what_is_kept_is_fed_short() {
    // On fifty rows over a scrollback of two thousand, a whole slice of
    // `CSI 65535 S` scrolls about twice the rows the spill keeps: few enough
    // that harvesting them all costs less than alacritty's churn, which
    // short slices are spared. The first few slices measure what the rest
    // are cut to.
    let mut emulator = Emulator::new(TerminalSize::new(200, 50), 2000, Palette::default());
    for read in "\x1b[65535S".repeat(3000).as_bytes().chunks(4096) {
        emulator.advance(read);
    }
    assert!(
        emulator.overgrown_slices <= 8,
        "{} slices scrolled past alacritty's spare rows",
        emulator.overgrown_slices
    );
}
