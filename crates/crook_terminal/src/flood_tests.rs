//! Output that writes the whole screen, or a long run of it, a few bytes at a
//! time — DECALN, the erases, RIS, line moves, scrolls, REP — and what keeps a
//! flood of it from holding the pane: see [`OscWatcher::spend`].
//!
//! Every test that feeds a stream holds the emulator to a `Term` given the same
//! bytes with nothing between them: the same cells on the screen, the cursor
//! in the same place, the same modes. The work budget may end a slice of
//! output early and a REP may be cut to a count that leaves the same screen,
//! and neither may leave a screen other than the one the program drew.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::index::{Column, Line};

use super::*;

/// What a screen shows, as far as a program can tell: every cell, where the
/// cursor is, and the modes.
#[derive(Debug, PartialEq, Eq)]
struct Picture {
    rows: Vec<String>,
    cursor: String,
    modes: String,
}

fn picture<T>(term: &Term<T>) -> Picture {
    let grid = term.grid();
    let rows = (0..grid.screen_lines())
        .map(|line| {
            (0..grid.columns())
                .map(|column| {
                    let cell = &grid[Line(line as i32)][Column(column)];
                    format!(
                        "{:?}{:?}{:?}{:?}{:?}|",
                        cell.c,
                        cell.fg,
                        cell.bg,
                        cell.flags,
                        cell.zerowidth()
                    )
                })
                .collect()
        })
        .collect();
    Picture {
        rows,
        cursor: format!(
            "{:?} wrap {}",
            grid.cursor.point, grid.cursor.input_needs_wrap
        ),
        modes: format!("{:?}", term.mode()),
    }
}

/// One thing fed: output, or a resize.
enum Step {
    Read(Vec<u8>),
    Resize(TerminalSize),
}

/// A scrollback long enough that nothing these tests print leaves it.
const LONG: usize = 100_000;

/// What `steps` leave on the screen of a `Term` that nothing stands between
/// and the bytes.
///
/// Not the history under it: what Crook keeps of that is its own, and leaves
/// out the blank rows above a block found again after an erase or a reset.
/// Two emulators are held to the same history: see [`kept_after`].
fn untouched_steps(size: TerminalSize, steps: &[Step]) -> Picture {
    let config = Config {
        scrolling_history: LONG,
        kitty_keyboard: true,
        ..Config::default()
    };
    let mut term = Term::new(config, &size, VoidListener);
    let mut parser: Processor = Processor::new();
    for step in steps {
        match step {
            Step::Read(read) => parser.advance(&mut term, read),
            Step::Resize(size) => term.resize(*size),
        }
    }
    picture(&term)
}

/// An emulator keeping `scrollback` lines, fed `steps` a slice at a time
/// the way the reader feeds it.
fn emulator_after(size: TerminalSize, scrollback: usize, steps: &[Step]) -> Emulator {
    emulator_cutting(size, scrollback, steps, true)
}

/// [`emulator_after`], cutting REPs or not.
fn emulator_cutting(size: TerminalSize, scrollback: usize, steps: &[Step], cut: bool) -> Emulator {
    let mut emulator = Emulator::new(size, scrollback, Palette::default());
    emulator.osc_watcher.whole_repeats = !cut;
    for step in steps {
        match step {
            Step::Read(read) => {
                let mut rest = &read[..];
                while !rest.is_empty() {
                    rest = &rest[emulator.advance_some(rest)..];
                }
            }
            Step::Resize(size) => emulator.resize(*size),
        }
    }
    emulator
}

/// What an emulator fed `steps` leaves on its screen.
fn emulated_steps(size: TerminalSize, steps: &[Step]) -> Picture {
    picture(&emulator_after(size, LONG, steps).term)
}

/// What an emulator keeping `scrollback` lines fed `steps` leaves on its
/// screen and in its history, every row of it, cutting REPs or not.
fn kept_after(
    size: TerminalSize,
    scrollback: usize,
    steps: &[Step],
    cut: bool,
) -> (Picture, usize, String) {
    let emulator = emulator_cutting(size, scrollback, steps, cut);
    let history = emulator.history_len();
    let (rows, _) = emulator.harvest_rows(0, history);
    (picture(&emulator.term), history, format!("{rows:?}"))
}

/// Holds that a REP cut leaves an emulator keeping `scrollback` lines just
/// as the whole run does: the screen, the history row by row, and both after
/// whatever resizes `steps` has.
fn cut_alike(size: TerminalSize, scrollback: usize, steps: &[Step], what: &str) {
    let (cut, whole) = (
        kept_after(size, scrollback, steps, true),
        kept_after(size, scrollback, steps, false),
    );
    assert!(cut.0 == whole.0, "{what}: the screens differ");
    assert_eq!(cut.1, whole.1, "{what}: the history's length");
    assert!(cut.2 == whole.2, "{what}: the history's rows differ");
}

/// [`untouched_steps`] for reads alone, under a long scrollback.
fn untouched(size: TerminalSize, reads: &[Vec<u8>]) -> Picture {
    let steps: Vec<Step> = reads.iter().cloned().map(Step::Read).collect();
    untouched_steps(size, &steps)
}

/// [`emulated_steps`] for reads alone, under a long scrollback.
fn emulated(size: TerminalSize, reads: &[Vec<u8>]) -> Picture {
    let steps: Vec<Step> = reads.iter().cloned().map(Step::Read).collect();
    emulated_steps(size, &steps)
}

/// `stream` cut into reads of `size` bytes.
fn reads(stream: &[u8], size: usize) -> Vec<Vec<u8>> {
    stream.chunks(size).map(<[u8]>::to_vec).collect()
}

/// Holds that `reads` leave the emulator's screen as they leave an untouched
/// `Term`'s.
fn alike(size: TerminalSize, reads: &[Vec<u8>], what: &str) {
    assert!(
        untouched(size, reads) == emulated(size, reads),
        "{what} at {}x{}: the screens differ",
        size.columns,
        size.rows
    );
}

/// Holds that `stream` leaves the same screen in a single read, in reads of
/// every size in `sizes`, and wrapped in a synchronized update.
fn alike_however_read(size: TerminalSize, frames: &[Vec<u8>], sizes: &[usize], what: &str) {
    const BSU: &[u8] = b"\x1b[?2026h";
    const ESU: &[u8] = b"\x1b[?2026l";
    let plain: Vec<u8> = frames.concat();
    let mut whole = BSU.to_vec();
    whole.extend_from_slice(&plain);
    whole.extend_from_slice(ESU);
    let framed: Vec<u8> = frames
        .iter()
        .flat_map(|frame| [BSU, frame, ESU].concat())
        .collect();
    for (how, stream) in [
        ("plain", &plain),
        ("in one update", &whole),
        ("an update a frame", &framed),
    ] {
        for read in sizes {
            alike(
                size,
                &reads(stream, *read),
                &format!("{what}, {how}, {read}-byte reads"),
            );
        }
    }
}

/// A seeded generator, so that a stream that fails is the same stream again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

/// `for i in $(seq N); do clear; seq $((RANDOM%20+1)); done; clear; echo
/// summary`: a screen erased and printed shorter and longer in turn.
fn clear_loop(frames: usize) -> Vec<Vec<u8>> {
    let mut rng = Rng(0x5eed_c1ea);
    let mut out: Vec<Vec<u8>> = (0..frames)
        .map(|_| {
            let mut frame = b"\x1b[H\x1b[2J\x1b[3J".to_vec();
            for n in 1..=rng.below(20) + 1 {
                frame.extend_from_slice(format!("{n}\r\n").as_bytes());
            }
            frame
        })
        .collect();
    out.push(b"\x1b[H\x1b[2J\x1b[3Jsummary\r\n".to_vec());
    out
}

/// A `printf '\e[H\e[2J'` loop over a few lines each time.
fn printf_loop(frames: usize) -> Vec<Vec<u8>> {
    let mut rng = Rng(0x9e37_79b9);
    (0..frames)
        .map(|frame| {
            let mut out = b"\x1b[H\x1b[2J".to_vec();
            for line in 0..rng.below(15) + 1 {
                out.extend_from_slice(format!("line {line} of {frame}\r\n").as_bytes());
            }
            out
        })
        .collect()
}

/// Three stars a frame, each frame on an erased screen.
fn stars(frames: usize, size: TerminalSize) -> Vec<Vec<u8>> {
    let mut rng = Rng(0x0001_57a5);
    (0..frames)
        .map(|frame| {
            let mut out = b"\x1b[H\x1b[2J".to_vec();
            for _ in 0..3 {
                let row = rng.below(usize::from(size.rows)) + 1;
                let column = rng.below(usize::from(size.columns)) + 1;
                out.extend_from_slice(format!("\x1b[{row};{column}H*").as_bytes());
            }
            out.extend_from_slice(format!("\x1b[1;1Hframe {frame}").as_bytes());
            out
        })
        .collect()
}

/// A progress display of a dozen lines that shrinks as its tasks end, drawn
/// again each step by going up over itself and erasing below.
fn progress(steps: usize) -> Vec<Vec<u8>> {
    let mut drawn = 0;
    let mut out: Vec<Vec<u8>> = (0..steps)
        .map(|step| {
            let mut frame = Vec::new();
            if drawn > 0 {
                frame.extend_from_slice(format!("\x1b[{drawn}A\x1b[J").as_bytes());
            }
            let live = 12usize.saturating_sub(step / 150).max(1);
            for task in 0..live {
                let bar = "#".repeat((step + task) % 30);
                frame.extend_from_slice(format!("[{bar:30}] task {task} {step}\r\n").as_bytes());
            }
            drawn = live;
            frame
        })
        .collect();
    out.push(b"all done\r\n".to_vec());
    out
}

#[test]
fn programs_that_erase_and_draw_less_each_time_leave_what_they_drew() {
    // Each of these erases the screen, or the part it drew, and draws less
    // on it than the time before: an erase that is not done leaves the last
    // frame's lines under this one's, and they stay after the program has
    // gone. However the output is read.
    for size in [TerminalSize::new(20, 5), TerminalSize::new(80, 24)] {
        for (what, frames) in [
            ("a clear loop", clear_loop(40)),
            ("a printf loop", printf_loop(60)),
            ("stars", stars(80, size)),
            ("a progress display", progress(200)),
        ] {
            alike_however_read(size, &frames, &[64, 1000, 16 * 1024], what);
        }
    }
}

#[test]
#[ignore = "slow in a debug build; run it in release mode"]
fn programs_that_erase_and_draw_less_each_time_leave_what_they_drew_at_every_size() {
    for size in [
        TerminalSize::new(20, 5),
        TerminalSize::new(80, 24),
        TerminalSize::new(200, 50),
        TerminalSize::new(300, 100),
    ] {
        for (what, frames) in [
            ("a clear loop", clear_loop(300)),
            ("a printf loop", printf_loop(3000)),
            ("stars", stars(600, size)),
            ("a progress display", progress(2000)),
        ] {
            alike_however_read(size, &frames, &[1, 7, 64, 512, 1024, 4096, 16 * 1024], what);
        }
    }
}

#[test]
#[ignore = "needs recordings: CROOK_RECORDINGS=<a directory of `name-COLSxROWS.rec`>"]
fn recorded_programs_leave_what_they_drew() {
    // `script` recordings of nvim, less, btop, top, watch, fzf, tmux, mc and
    // shell loops, kept out of the repository because what they show is a
    // machine's processes and files.
    let Ok(directory) = std::env::var("CROOK_RECORDINGS") else {
        return;
    };
    let mut seen = 0;
    for entry in std::fs::read_dir(&directory).expect("a directory of recordings") {
        let path = entry.expect("an entry").path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some(geometry) = name
            .strip_suffix(".rec")
            .and_then(|stem| stem.rsplit_once('-'))
            .map(|(_, geometry)| geometry)
        else {
            continue;
        };
        let Some((columns, rows)) = geometry
            .split_once('x')
            .and_then(|(columns, rows)| Some((columns.parse().ok()?, rows.parse().ok()?)))
        else {
            continue;
        };
        let stream = std::fs::read(&path).expect("a readable recording");
        alike_however_read(
            TerminalSize::new(columns, rows),
            &[stream],
            &[1, 64, 1024, 4096, 16 * 1024],
            &name,
        );
        seen += 1;
    }
    assert!(seen > 0, "no recordings in {directory}");
}

/// A random stream of everything the budget counts and everything that
/// changes what those escapes do: text, narrow and wide, moves, erases,
/// alignments, line moves and scrolls, REPs, colours, scroll regions,
/// synchronized updates, the alternate screen, resets, insert mode and
/// autowrap.
///
/// It ends with the end of a synchronized update, so that one still open is
/// played: one that was let go of early has been drawn by then, and until
/// it ends the two screens show different moments of the same stream.
fn fuzzed(rng: &mut Rng, size: TerminalSize, ops: usize) -> Vec<u8> {
    let mut stream = fuzzed_ops(rng, size, ops).concat().into_bytes();
    stream.extend_from_slice(b"\x1b[?2026l");
    stream
}

/// [`fuzzed`], an escape or a run of text at a time.
fn fuzzed_ops(rng: &mut Rng, size: TerminalSize, ops: usize) -> Vec<String> {
    let (columns, rows) = (usize::from(size.columns), usize::from(size.rows));
    let mut out = Vec::new();
    for _ in 0..ops {
        let op = match rng.below(20) {
            0..=3 => {
                let text: String = (0..rng.below(30) + 1)
                    .map(|_| ['a', 'b', 'x', ' ', '\u{4e2d}', '#'][rng.below(6)])
                    .collect();
                text
            }
            4 => ["\r\n", "\n", "\r", "\t", "\x08"][rng.below(5)].to_owned(),
            5 => format!("\x1b[{};{}H", rng.below(rows + 2), rng.below(columns + 2)),
            6 => format!("\x1b[{}J", rng.below(4)),
            7 => "\x1b#8".to_owned(),
            8 => {
                let count = [1, 2, rows - 1, rows, rows + 1, 999][rng.below(6)];
                format!("\x1b[{count}{}", ['L', 'M', 'S', 'T'][rng.below(4)])
            }
            9 | 10 => {
                let cells = columns * rows;
                let count = [
                    1,
                    2,
                    columns - 1,
                    columns,
                    columns + 1,
                    cells - 1,
                    cells,
                    cells + columns + 1,
                    500,
                    65535,
                ][rng.below(10)];
                format!("\x1b[{count}b")
            }
            11 => format!("\x1b[{}m", [0, 41, 44, 47, 49, 31][rng.below(6)]),
            12 => {
                if rng.below(3) == 0 {
                    "\x1b[r".to_owned()
                } else {
                    let top = rng.below(rows) + 1;
                    format!("\x1b[{top};{}r", top + rng.below(rows - top + 1))
                }
            }
            13 => ["\x1b[?2026h", "\x1b[?2026l"][rng.below(2)].to_owned(),
            14 => ["\x1b[?1049h", "\x1b[?1049l"][rng.below(2)].to_owned(),
            15 => "\x1bc".to_owned(),
            16 => ["\x1b[4h", "\x1b[4l"][rng.below(2)].to_owned(),
            17 => ["\x1b[?7h", "\x1b[?7l"][rng.below(2)].to_owned(),
            18 => format!(
                "\x1b[{}{}",
                [1, 5, 999][rng.below(3)],
                ['X', '@', 'P'][rng.below(3)]
            ),
            _ => "\x1b[3J".to_owned(),
        };
        out.push(op);
    }
    out
}

/// Feeds `seeds` random streams, each cut into reads at random, and says
/// which of them left the emulator's screen other than an untouched `Term`'s.
fn fuzz(seeds: std::ops::Range<u64>, ops: usize) -> Vec<(u64, TerminalSize)> {
    let sizes = [
        TerminalSize::new(20, 5),
        TerminalSize::new(21, 5),
        TerminalSize::new(13, 7),
    ];
    let mut differ = Vec::new();
    for seed in seeds {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let size = sizes[rng.below(3)];
        let stream = fuzzed(&mut rng, size, ops);
        // Cut into reads at random, with the screen changing size between
        // some of them.
        let mut steps = Vec::new();
        let mut rest = &stream[..];
        while !rest.is_empty() {
            let take = (rng.below(600) + 1).min(rest.len());
            steps.push(Step::Read(rest[..take].to_vec()));
            rest = &rest[take..];
            // With any update ended first: one let go of early has been drawn
            // at the size it was let go of at, where one held is played at
            // the size the screen has when it ends — which a REP cut, by the
            // work it saves, moves.
            if rng.below(6) == 0 {
                steps.push(Step::Read(b"\x1b[?2026l".to_vec()));
                steps.push(Step::Resize(sizes[rng.below(3)]));
            }
        }
        // The screen as an untouched `Term` leaves it, without the resizes:
        // Crook lays out again only the rows its grid holds, and the rest as
        // a store of its own, which reflows lines a screen long at the seam
        // otherwise than `Term`. And the screen and the history as the whole
        // of every REP leaves them, resizes and all, over a scrollback short
        // enough for a REP to be cut past it.
        let reads: Vec<Step> = steps
            .iter()
            .filter_map(|step| match step {
                Step::Read(read) => Some(Step::Read(read.clone())),
                Step::Resize(_) => None,
            })
            .collect();
        let screen = untouched_steps(size, &reads) == emulated_steps(size, &reads);
        let cut = kept_after(size, 12, &steps, true) == kept_after(size, 12, &steps, false);
        if !(screen && cut) {
            differ.push((seed, size));
        }
    }
    differ
}

#[test]
fn random_floods_leave_what_they_drew() {
    assert_eq!(Vec::<(u64, TerminalSize)>::new(), fuzz(0..60, 300));
}

#[test]
#[ignore = "slow in a debug build; run it in release mode"]
fn random_floods_leave_what_they_drew_many_times_over() {
    assert_eq!(Vec::<(u64, TerminalSize)>::new(), fuzz(0..5000, 600));
}

#[test]
fn a_cut_repeat_leaves_the_screen_and_the_cursor_the_whole_run_would() {
    // A REP is cut to about a screenful, which is all a run can leave on the
    // screen: from wherever and however the run starts.
    let openings: &[&str] = &[
        "",
        "x",
        "ab\x1b[3;4H",
        "x\n",
        "x\t",
        "one\r\ntwo\r\nthree\r\nfour\x1b[2;5H",
        "full\r\nscreen\r\nof\r\nlines\r\nand\r\nmore\r\nstill\x1b[1;1H",
        "\x1b[4hinsert\r\nmode\x1b[1;3H",
        "\x1b[4h",
        "\x1b[4hab\x1b[2;3Hcd\x1b[1;1H",
        "\x1b[4h\u{4e2d}x\u{4e2d}\x1b[3;2H",
        "\x1b[31mred\x1b[44m",
        "\x1b[2;4r\x1b[3;2H",
        "\x1b[2;4r\x1b[6;2H",
        "\x1b[?7lno wrap",
        "\x1b[1;20H",
        "\x1b[1;19H",
        "abcdefghijklmnopqrst",
    ];
    let counts = [
        1, 5, 19, 20, 21, 40, 119, 120, 121, 125, 141, 150, 160, 161, 179, 180, 181, 241, 1000,
        65535, 99999, 239, 240, 241, 259, 260, 261, 280, 281,
    ];
    for size in [TerminalSize::new(20, 6), TerminalSize::new(21, 5)] {
        for character in ["x", "\u{4e2d}"] {
            for opening in openings {
                for count in counts {
                    // On the alternate screen, which keeps no history, the
                    // cut is two screenfuls: held to an untouched `Term`.
                    let what = format!("{opening:?} {character} x {count}");
                    let run = format!("\x1b[?1049h{opening}{character}\x1b[{count}bY");
                    alike(size, &[run.into_bytes()], &what);
                    // On the shell's, with a scrollback short enough to cut
                    // past: held to the whole run, history and all, and
                    // after the screen changes size both ways.
                    let run = format!("{opening}{character}\x1b[{count}bY").into_bytes();
                    let steps = [
                        Step::Read(run),
                        Step::Resize(TerminalSize::new(13, 7)),
                        Step::Read(b"Z".to_vec()),
                        Step::Resize(size),
                    ];
                    cut_alike(size, 12, &steps, &what);
                }
            }
        }
    }
}

#[test]
fn repeats_in_insert_mode_and_after_repeats_leave_what_the_whole_runs_would() {
    for (size, stream) in [
        (TerminalSize::new(80, 24), "\x1b[4hx\x1b[65535bY"),
        (
            TerminalSize::new(21, 5),
            "\x1b[4h\u{4e2d}\x1b[119b\x1b[3;5H\x1b[200bY",
        ),
        (TerminalSize::new(21, 5), "\u{4e2d}\x1b[200b\x1b[r\x1b[119b"),
        (
            TerminalSize::new(20, 5),
            "x\x1b[100b\x1b[100b\x1b[100b\x1b[7bY",
        ),
        (TerminalSize::new(20, 5), "xxxx\x1b[150bxx\x1b[65535bY"),
        (
            TerminalSize::new(21, 5),
            "\u{4e2d}\x1b[50b\u{4e2d}\x1b[65535b\x1b[3bZ",
        ),
    ] {
        alike(size, &[stream.as_bytes().to_vec()], stream);
    }
}

#[test]
fn repeats_of_one_character_one_after_another_are_cut_as_one_run() {
    // Each a row and a bit, and so none past a screenful on its own:
    // together a run as long as all of them, and once that is past two
    // screenfuls each is cut to the bit, which leaves the screen the whole
    // run does. Cut, they cost a slice what as many characters do; whole,
    // the slice would end every few of them.
    let size = TerminalSize::new(20, 5);
    let flood = format!("\x1b[?1049hx{}Y", "\x1b[199b".repeat(2000));
    alike(size, &[flood.clone().into_bytes()], "a run of REPs");
    let mut emulator = Emulator::new(size, 100, Palette::default());
    assert_eq!(flood.len(), emulator.advance_some(flood.as_bytes()));
}

#[test]
fn a_cut_repeat_leaves_the_scrollback_a_column_change_reflows_alike() {
    // Cut to two screenfuls, a REP left fewer rows of history than the whole
    // run, and a column change reflowed them into another screen. Cut past
    // the whole scrollback, the history is every row the run leaves too.
    // On the alternate screen, which a resize does not reflow, a run the
    // screen changes size in the middle of is two runs.
    let size = TerminalSize::new(20, 5);
    for (narrower, then) in [(7, "\x1b[65535bY"), (9, "\x1b[61bY"), (13, "\x1b[300bY")] {
        let steps = [
            Step::Read(b"\x1b[?1049hx\x1b[65535b".to_vec()),
            Step::Resize(TerminalSize::new(narrower, 5)),
            Step::Read(then.as_bytes().to_vec()),
        ];
        assert!(
            untouched_steps(size, &steps) == emulated_steps(size, &steps),
            "{narrower} columns then {then:?}"
        );
    }
    for (size, steps) in [
        (
            TerminalSize::new(13, 7),
            vec![
                Step::Read(b"ba  b xbx\x1b[500b".to_vec()),
                Step::Resize(TerminalSize::new(21, 5)),
            ],
        ),
        (
            TerminalSize::new(20, 5),
            vec![
                Step::Read("\u{4e2d}\x1b[65535b".as_bytes().to_vec()),
                Step::Resize(TerminalSize::new(13, 9)),
                Step::Read(b"x\x1b[65535b".to_vec()),
                Step::Resize(TerminalSize::new(30, 3)),
            ],
        ),
        // A run the screen changes size in the middle of, which is two runs.
        (
            TerminalSize::new(20, 5),
            vec![
                Step::Read(b"x\x1b[65535b".to_vec()),
                Step::Resize(TerminalSize::new(7, 5)),
                Step::Read(b"\x1b[65535b".to_vec()),
                Step::Resize(TerminalSize::new(20, 5)),
            ],
        ),
    ] {
        for scrollback in [0, 1, 12, 100] {
            cut_alike(
                size,
                scrollback,
                &steps,
                &format!("a scrollback of {scrollback}"),
            );
        }
    }
}

#[test]
fn a_reset_inside_an_update_finds_the_block_on_the_screen_the_update_leaves() {
    // The watcher reads a reset as it arrives, and `vte` holds it with the
    // update it is in. The block found again then is found on the screen as
    // it was before the update, below the rows the update prints from the
    // top, which were nobody's.
    let mut emulator = Emulator::new(TerminalSize::new(20, 6), 1000, Palette::default());
    emulator.advance(b"a\r\nb\r\nc\r\nd");
    emulator.advance(b"\x1b[?2026h\x1bcfirst\r\nsecond\x1b[?2026l");
    let live = emulator.live_block();
    assert_eq!((0, 1), (live.top_row, live.bottom_row), "{live:?}");
}

#[test]
fn a_cut_repeat_split_across_reads_is_cut_alike() {
    let size = TerminalSize::new(21, 5);
    let run = "ab\x1b[2;3H\u{4e2d}\x1b[65535bY".as_bytes();
    for first in 1..run.len() {
        for second in first..run.len() {
            let reads = [
                run[..first].to_vec(),
                run[first..second].to_vec(),
                run[second..].to_vec(),
            ];
            alike(size, &reads, &format!("cut at {first} and {second}"));
        }
    }
}

#[test]
fn a_repeat_of_more_than_a_screenful_repeats_about_a_screenful() {
    // Eight bytes asked `vte` for 65535 characters, every one of them a
    // character printed. On the alternate screen, which keeps no history,
    // two screenfuls and two rows are as many as leave the screen alike.
    let mut emulator = Emulator::new(TerminalSize::new(20, 5), 100, Palette::default());
    emulator.advance(b"\x1b[?1049hx");
    let before = emulator.osc_watcher.work;
    emulator.advance(b"\x1b[65535b");
    let spent = before + 8 * 8 - emulator.osc_watcher.work;
    assert!(spent <= 20 * (2 * 5 + 2), "{spent} repeated");
    // On the shell's, the history is a row of them too, as many rows as it
    // keeps: nothing short of a scrollback and two screenfuls is cut.
    let mut emulator = Emulator::new(TerminalSize::new(20, 5), 100, Palette::default());
    emulator.advance(b"x\x1b[65535b");
    // The run is the `x` and the REP: the fewest past the floor, less the
    // `x`, that end in the column 65535 would.
    let floor = (100 + HISTORY_HEADROOM + 2 * 5 + 1) * 20;
    let column = 65535 % 20;
    let cut = column + (floor - 1 - column).div_ceil(20) * 20;
    assert_eq!(Some(Cut::Repeat(cut)), {
        let mut watcher = OscWatcher {
            screen: (20, 5),
            scrollback: 100 + HISTORY_HEADROOM,
            ..OscWatcher::default()
        };
        watcher.slice(9, false, false);
        let mut parser = Parser::new();
        let _ = parser.advance_until_terminated(&mut watcher, b"x\x1b[65535b");
        watcher.cut
    });
}

#[test]
fn a_combining_mark_is_repeated_as_often_as_a_grapheme_carries_them() {
    // A mark goes on the cell before it, and so does every copy of it: as
    // many as asked up to sixteen, and sixteen past that, rather than 65535
    // on one cell for eight bytes.
    let size = TerminalSize::new(20, 5);
    for count in [1, 2, 7, 15] {
        let stream = format!("e\u{301}\x1b[{count}bz").into_bytes();
        alike(size, &[stream], &format!("a mark repeated {count} times"));
    }
    // The one place a REP leaves a screen other than its whole run: past
    // sixteen copies of a mark, sixteen.
    for count in [16, 17, 100, 65535] {
        let mut emulator = Emulator::new(size, 100, Palette::default());
        emulator.advance(format!("e\u{301}\x1b[{count}b").as_bytes());
        let marks = emulator.term.grid()[Line(0)][Column(0)]
            .zerowidth()
            .map_or(0, <[char]>::len);
        assert_eq!(1 + ZERO_WIDTH_REPEATS, marks, "{count}");
    }
}

#[test]
fn escapes_a_second_of_changes_nothing_leave_what_both_would() {
    // The ones the watcher does not do twice: a DECALN or an erase right
    // after one, which changes nothing — unless the pen has a background,
    // where a second erase pushes the painted screen into the history — and
    // a reset or an erase of the history after one.
    let size = TerminalSize::new(20, 6);
    for stream in [
        "\x1b#8\x1b#8\x1b#8",
        "\x1b#8x\x1b#8y",
        "ab\x1b#8\x1b[3;3Hc\x1b#8\x1b[1;1Hd",
        "one\r\ntwo\x1b[2J\x1b[2J\x1b[J\x1b[1J",
        "one\x1b[2;3H\x1b[J\x1b[J\x1b[1J\x1b[1J",
        "\x1b[44mone\r\ntwo\x1b[2J\x1b[2J\x1b[J",
        "\x1b[44m\x1b[2J\x1b[2J",
        "\x1b#8\x1b[2J\x1b#8\x1b[2J",
        "one\x1bc\x1bc\x1b[3J\x1b[3Jtwo",
        "a\r\nb\r\nc\x1b[2;1H\x1b[2L\x1b[M\x1b[3S\x1b[T",
        "\x1b[?2026h\x1b#8\x1b#8x\x1b[2J\x1b[?2026l",
    ] {
        alike(size, &[stream.as_bytes().to_vec()], stream);
    }
    // And the history: a second erase under a pen with a background pushes
    // the painted screen into it, as the same erase does with an escape
    // between the two that changes nothing.
    let history = |stream: &str| {
        let mut emulator = Emulator::new(size, 100_000, Palette::default());
        emulator.advance(stream.as_bytes());
        emulator.history_len()
    };
    assert_eq!(
        history("\x1b[44mone\r\ntwo\x1b[2J\x1b[44m\x1b[2J"),
        history("\x1b[44mone\r\ntwo\x1b[2J\x1b[2J")
    );
    // Inside an update too, where `Term` has not been given the colour yet.
    assert_eq!(
        history("one\r\ntwo\x1b[?2026h\x1b[44m\x1b[2J\x1b[44m\x1b[2J\x1b[?2026l"),
        history("one\r\ntwo\x1b[?2026h\x1b[44m\x1b[2J\x1b[2J\x1b[?2026l")
    );
}

/// What the watcher stops on in `bytes`, in order: a reset, an erase of the
/// history, and an escape it does some other way.
fn stops(bytes: &[u8]) -> Vec<(bool, bool, Option<Cut>)> {
    let mut watcher = OscWatcher {
        screen: (20, 5),
        ..OscWatcher::default()
    };
    watcher.slice(bytes.len(), false, false);
    let mut parser = Parser::new();
    let mut stops = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let consumed = parser.advance_until_terminated(&mut watcher, rest);
        rest = &rest[consumed..];
        stops.push((
            std::mem::take(&mut watcher.reset),
            std::mem::take(&mut watcher.history_cleared),
            watcher.cut.take(),
        ));
    }
    stops
}

#[test]
fn a_reset_or_an_erase_with_nothing_left_to_do_is_not_stopped_on() {
    let resets = |bytes: &[u8]| stops(bytes).iter().filter(|stop| stop.0).count();
    let erases = |bytes: &[u8]| stops(bytes).iter().filter(|stop| stop.1).count();
    assert_eq!(1, resets(b"\x1bc\x1bc\x1bc"));
    assert_eq!(2, resets(b"\x1bcx\x1bc"));
    assert_eq!(1, erases(b"\x1b[3J\x1b[3J"));
    assert_eq!(0, erases(b"\x1bc\x1b[3J"));
    assert_eq!(2, erases(b"\x1b[3Jx\x1b[3J"));
    let skipped = |bytes: &[u8]| stops(bytes).iter().filter(|stop| stop.2.is_some()).count();
    assert_eq!(2, skipped(b"\x1b#8\x1b#8\x1b#8"));
    assert_eq!(0, skipped(b"\x1b#8x\x1b#8"));
    assert_eq!(2, skipped(b"\x1b[2J\x1b[J\x1b[1J"));
    assert_eq!(0, skipped(b"\x1b[J\x1b[1J"));
}

#[test]
fn a_slice_of_screen_writes_ends_where_its_work_runs_out() {
    // A read of DECALNs with a character after each: every one of them is
    // done, a slice of work at a time, and the slices together leave the
    // screen the whole read does.
    let size = TerminalSize::new(80, 24);
    let mut flood = Vec::new();
    for n in 0..4000 {
        flood.extend_from_slice(format!("\x1b#8\x1b[{};{}Hx", n % 24 + 1, n % 80 + 1).as_bytes());
    }
    let mut emulator = Emulator::new(size, 1000, Palette::default());
    let first = emulator.advance_some(&flood);
    assert!(first < flood.len(), "the whole read was one slice");
    let mut slices = 1;
    let mut rest = &flood[first..];
    while !rest.is_empty() {
        rest = &rest[emulator.advance_some(rest)..];
        slices += 1;
    }
    assert!(slices > 10, "{slices} slices");
    assert!(untouched(size, &[flood]) == picture(&emulator.term));
    // A read of scrolls too, which ends at the end of a slice the feed
    // would feed whole rather than at the scroll the work ran out on.
    let scrolls = "\x1b[24S".repeat(4000);
    let mut scrolling = Emulator::new(size, 10_000, Palette::default());
    let first = scrolling.advance_some(scrolls.as_bytes());
    assert!(
        first < scrolls.len(),
        "the whole read of scrolls was one slice"
    );
    // And text costs nothing: a read of it is one slice.
    let text = "abc ".repeat(4000);
    assert_eq!(text.len(), emulator.advance_some(text.as_bytes()));
}

#[test]
fn an_update_whose_work_runs_out_is_let_go_of_and_drawn_as_it_comes() {
    // `vte` holds an update back and plays it in one go, so the slices it
    // spans could not each let the lock go: once its work is spent, it is
    // let go of as one that ran out of time is.
    let size = TerminalSize::new(80, 24);
    let mut flood = b"\x1b[?2026h".to_vec();
    for n in 0..4000 {
        flood.extend_from_slice(format!("\x1b#8\x1b[{};{}Hx", n % 24 + 1, n % 80 + 1).as_bytes());
    }
    flood.extend_from_slice(b"\x1b[?2026l");
    let mut emulator = Emulator::new(size, 1000, Palette::default());
    let first = emulator.advance_some(&flood);
    assert!(first < flood.len(), "the whole update was one slice");
    assert!(!emulator.updating(), "the update is still held");
    let mut rest = &flood[first..];
    while !rest.is_empty() {
        rest = &rest[emulator.advance_some(rest)..];
    }
    assert!(untouched(size, &[flood]) == picture(&emulator.term));
}

/// Release-mode timings of output a program floods with, for the pull
/// request: `cargo test --release -p crook_terminal --lib bench_flood --
/// --ignored --nocapture`. Fed the way the reader feeds it: 16 KiB reads,
/// each a slice of work at a time, the lock let go between slices. The
/// whole feed, and the longest slice, which is how long the pane's lock is
/// held at a time: as plain output, and as one synchronized update.
#[test]
#[ignore = "a benchmark; run it in release mode"]
fn bench_flooded_output() {
    use std::time::Instant;
    let floods: Vec<(&str, &[u8], &[u8])> = vec![
        ("text", b"", b"x"),
        ("REP 65535", b"", b"x\x1b[65535b"),
        ("REP of the screen", b"", b"x\x1b[10000b"),
        ("DECALN", b"", b"\x1b#8"),
        ("DECALN and a character", b"", b"\x1b#8x"),
        ("ED 2", b"", b"\x1b[2J"),
        ("a character and ED 2", b"", b"x\x1b[2J"),
        ("clear", b"", b"\x1b[H\x1b[2J\x1b[3Jx\r\n"),
        ("RIS", b"", b"\x1bc"),
        ("RIS and a character", b"", b"\x1bcx"),
        ("IL 999", b"", b"\x1b[999L"),
        ("DL 999", b"", b"\x1b[999M"),
        ("SU 999", b"", b"\x1b[999S"),
        ("SD 999", b"", b"\x1b[999T"),
        ("LF in a region", b"\x1b[2;49r\x1b[49H", b"\n"),
        ("RI in a region", b"\x1b[2;49r\x1b[2H", b"\x1bM"),
        ("ICH 9999", b"", b"\x1b[9999@"),
        ("ECH 9999", b"", b"\x1b[9999X"),
        ("DECCOLM", b"", b"\x1b[?3h"),
        ("alt screen in and out", b"", b"\x1b[?1049h\x1b[?1049l"),
    ];
    for (name, setup, unit) in floods {
        for synchronized in [false, true] {
            let size = if synchronized { 2_000_000 } else { 300_000 };
            let mut bytes = setup.to_vec();
            if synchronized {
                bytes.extend_from_slice(b"\x1b[?2026h");
            }
            while bytes.len() < size {
                bytes.extend_from_slice(unit);
            }
            if synchronized {
                bytes.extend_from_slice(b"\x1b[?2026l");
            }
            let mut emulator =
                Emulator::new(TerminalSize::new(200, 50), 10_000, Palette::default());
            let mut worst = 0f64;
            let start = Instant::now();
            for read in bytes.chunks(16 * 1024) {
                let mut rest = read;
                while !rest.is_empty() {
                    let at = Instant::now();
                    rest = &rest[emulator.advance_some(rest)..];
                    worst = worst.max(at.elapsed().as_secs_f64() * 1000.);
                }
            }
            println!(
                "{name}{} ({} KB) at 200x50: {:.0} ms, longest slice {worst:.1} ms",
                if synchronized { ", one update" } else { "" },
                bytes.len() / 1000,
                start.elapsed().as_secs_f64() * 1000.
            );
        }
    }
}
