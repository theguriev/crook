//! What a find bar keeps of a search between frames, over blocks a real
//! emulator made: that a finished block is read once, that what is kept is
//! what a fresh walk of the same blocks would find, and that the query is
//! compiled once for the search, the bar's word on it and the filter.
//!
//! The grid's half of the find bar, over grids a real emulator made: reading
//! the scrollback back in pieces, matching it by the list's rules, and the
//! arithmetic between a match's row and the screen.

use std::sync::Arc;

use crook_terminal::{Emulator, Palette, TerminalSize};

use super::*;
use crate::selection::{patterns_compiled, stored_rows_searched};

/// OSC 133 prompt start.
const A: &str = "\x1b]133;A\x07";
/// OSC 133 prompt end.
const B: &str = "\x1b]133;B\x07";
/// OSC 133 output start.
const C: &str = "\x1b]133;C\x07";
/// OSC 133 command finished, successfully.
const D: &str = "\x1b]133;D;0\x07";

/// A pane whose shell reports where its commands start and end.
struct Pane {
    emulator: Emulator,
}

impl Pane {
    /// A pane at its first prompt.
    fn new() -> Self {
        let mut emulator = Emulator::new(TerminalSize::new(10, 8), 1000, Palette::default());
        emulator.advance(format!("{A}$ ").as_bytes());
        Self { emulator }
    }

    /// Runs `command`, which prints `output`, through to the next prompt.
    fn run(&mut self, command: &str, output: &str) {
        self.emulator
            .advance(format!("{B}{command}\r\n{C}{output}\r\n{D}{A}$ ").as_bytes());
    }

    /// The finished blocks as the window holds them, `evicted` of the oldest
    /// left out the way the store drops them.
    fn history(&self, evicted: usize) -> BlockHistory {
        BlockHistory::new(
            self.emulator
                .blocks()
                .iter()
                .skip(evicted)
                .cloned()
                .map(Arc::new)
                .collect(),
            evicted,
        )
    }
}

/// A find bar with `query` typed into it.
fn bar(query: &str) -> PaneFind {
    let find = PaneFind::new();
    find.open(false);
    find.input().edit(|editor| editor.set_text(query));
    find
}

/// What `find` matches in `pane`, and how many finished rows it read to say.
fn found(find: &PaneFind, pane: &mut Pane, evicted: usize) -> (Matches, usize) {
    let history = pane.history(evicted);
    let snapshot = pane.emulator.snapshot();
    let before = stored_rows_searched();
    let matches = find.matches(&history, &snapshot);
    (matches, stored_rows_searched() - before)
}

/// The text of every match, in order, as a copy of it would read.
fn texts(matches: &Matches, pane: &mut Pane) -> Vec<String> {
    let history = pane.history(0);
    let snapshot = pane.emulator.snapshot();
    let blocks = Blocks::list(&history, &snapshot);
    (0..matches.len())
        .filter_map(|index| matches.get(index))
        .filter_map(|region| region.text(&blocks))
        .collect()
}

#[test]
fn a_finished_block_is_read_once_however_many_frames_ask() {
    let mut pane = Pane::new();
    pane.run("a", "one two");
    pane.run("b", "two one");
    let find = bar("one");

    let (first, read) = found(&find, &mut pane, 0);
    assert_eq!(2, first.len());
    assert!(read > 0, "the first ask reads the finished blocks");

    for _ in 0..3 {
        let (again, read) = found(&find, &mut pane, 0);
        assert_eq!(0, read, "a frame over the same blocks read them again");
        assert_eq!(2, again.len(), "and still has every match");
    }
}

#[test]
fn only_a_block_that_finished_since_is_read() {
    let mut pane = Pane::new();
    pane.run("a", "one");
    let find = bar("one");
    let (_, read) = found(&find, &mut pane, 0);
    assert!(read > 0);

    pane.run("b", "one one");
    let (matches, read) = found(&find, &mut pane, 0);
    let new = pane.history(0).get(1).expect("the new block").rows.rows();
    assert_eq!(new, read, "the new block's rows, and none of the old one's");
    assert_eq!(3, matches.len());
    assert_eq!(vec!["one"; 3], texts(&matches, &mut pane));
}

#[test]
fn another_query_or_a_switch_is_a_new_search() {
    let mut pane = Pane::new();
    pane.run("a", "one ONE");
    let find = bar("one");
    let (matches, _) = found(&find, &mut pane, 0);
    assert_eq!(2, matches.len(), "folded");

    find.toggle(false);
    let (matches, read) = found(&find, &mut pane, 0);
    assert!(read > 0, "matched another way is searched again");
    assert_eq!(vec!["one"], texts(&matches, &mut pane), "told apart");

    find.input().edit(|editor| editor.set_text("ONE"));
    let (matches, read) = found(&find, &mut pane, 0);
    assert!(read > 0, "another query is searched again");
    assert_eq!(vec!["ONE"], texts(&matches, &mut pane));

    find.input().edit(|editor| editor.set_text(""));
    assert!(
        found(&find, &mut pane, 0).0.is_empty(),
        "nothing finds nothing"
    );
}

#[test]
fn a_bad_pattern_is_said_by_the_search_that_compiled_it_once() {
    // The bar compiled the query again on every frame to say whether it was a
    // bad pattern, beside the copy the search keeps, and without the search's
    // case folding — so it could call a pattern fine that the search itself
    // could not compile, and count it "No results". It asks the search now.
    let mut pane = Pane::new();
    pane.run("a", "one");
    let find = bar("(one");
    assert!(!find.pattern_is_bad(), "as text, no query is a bad pattern");

    find.toggle(true);
    let before = patterns_compiled();
    for _ in 0..3 {
        assert!(find.pattern_is_bad(), "an unclosed group");
        assert!(
            found(&find, &mut pane, 0).0.is_empty(),
            "and it finds nothing"
        );
    }
    assert_eq!(
        1,
        patterns_compiled() - before,
        "three frames of the bar and the search compiled it more than once"
    );

    find.input().edit(|editor| editor.set_text("o.e"));
    assert!(!find.pattern_is_bad());
    assert_eq!(vec!["one"], texts(&found(&find, &mut pane, 0).0, &mut pane));

    // Two thousand Greek small letters in a row fit the size limit, but not
    // once folding has made each one a class of its capital too: the search
    // cannot compile that, so the bar says so.
    find.input().edit(|editor| editor.set_text("[α-ω]{2000}"));
    assert!(find.pattern_is_bad(), "folded, it is past the size limit");
    find.toggle(false);
    assert!(!find.pattern_is_bad(), "told apart, it compiles");
}

#[test]
fn the_filter_is_handed_the_pattern_the_search_compiled() {
    // The filter compiled the query for itself on every frame that had no
    // answer kept, which for a pattern that does not compile was every frame.
    let mut pane = Pane::new();
    pane.run("a", "one");
    let find = bar("o.e");
    find.toggle(true);

    let before = patterns_compiled();
    let first = find.matcher().expect("it compiles");
    found(&find, &mut pane, 0);
    let again = find.matcher().expect("it still compiles");
    assert!(Rc::ptr_eq(&first, &again), "the filter compiled its own");
    assert!(first.matches("one") && !first.matches("two"));
    assert_eq!(1, patterns_compiled() - before, "one search, one compile");

    find.input().edit(|editor| editor.set_text("(one"));
    let before = patterns_compiled();
    for _ in 0..3 {
        assert!(find.matcher().is_none(), "an unclosed group");
    }
    assert_eq!(
        1,
        patterns_compiled() - before,
        "a pattern that does not compile was compiled for every ask"
    );
}

#[test]
fn a_block_dropped_off_the_front_takes_its_matches_with_it() {
    let mut pane = Pane::new();
    pane.run("a", "one");
    pane.run("b", "one");
    pane.run("c", "one");
    let find = bar("one");
    assert_eq!(3, found(&find, &mut pane, 0).0.len());

    let (matches, read) = found(&find, &mut pane, 1);
    assert_eq!(0, read, "what is left was already read");
    assert_eq!(2, matches.len());
    let kept = pane.history(1);
    assert_eq!(
        kept.get(0).map(|block| block.id),
        matches.get(0).map(|found| found.start.block),
        "the first match is in the oldest block still held"
    );
}

#[test]
fn the_open_block_is_searched_again_once_it_has_changed() {
    let mut pane = Pane::new();
    pane.run("a", "one");
    let find = bar("one");
    assert_eq!(1, found(&find, &mut pane, 0).0.len());

    // A command still running, printing a match of its own: the snapshot is
    // a new one, so the open block is searched in it.
    pane.emulator
        .advance(format!("{B}b\r\n{C}one\r\n").as_bytes());
    let (matches, read) = found(&find, &mut pane, 0);
    assert_eq!(0, read, "the finished block was not read again");
    assert_eq!(2, matches.len());
    assert_eq!(vec!["one", "one"], texts(&matches, &mut pane));

    pane.emulator.advance(b"one\r\n");
    assert_eq!(3, found(&find, &mut pane, 0).0.len(), "and again");
}

#[test]
fn what_is_kept_across_frames_is_what_one_search_of_every_block_finds() {
    // Gathered a block at a time as the commands finish, folds and wide
    // characters and the open block included, the kept matches are the ones a
    // single search of all of it finds: the same ones, in the same order,
    // over the same cells.
    let mut pane = Pane::new();
    let find = bar("one");
    pane.run("a", "x one 漢one");
    found(&find, &mut pane, 0);
    pane.run("b", "aaaaaaaaa one");
    found(&find, &mut pane, 0);
    pane.emulator
        .advance(format!("{B}c\r\n{C}one\r\n").as_bytes());
    let (matches, _) = found(&find, &mut pane, 0);

    let history = pane.history(0);
    let snapshot = pane.emulator.snapshot();
    let blocks = Blocks::list(&history, &snapshot);
    let once = blocks.find_all("one");
    assert_eq!(4, once.len(), "two in the first block, one in each after");
    assert_eq!(once.len(), matches.len());
    for (index, selection) in once.iter().enumerate() {
        assert_eq!(
            selection.region(&blocks),
            matches.get(index),
            "match {index}"
        );
    }
}

#[test]
fn a_row_asks_only_for_the_matches_on_it() {
    // Ten columns: the second `one` starts in the last column and runs on
    // across the fold, so it is on both rows of the line and the first is on
    // only the first.
    let mut pane = Pane::new();
    pane.run("a", "one aaaaaone");
    let find = bar("one");
    let (matches, _) = found(&find, &mut pane, 0);
    let history = pane.history(0);
    let block = history.get(0).expect("a finished block").id;

    let rows: Vec<Vec<usize>> = (0..4)
        .map(|row| {
            matches
                .on_row(block, row)
                .map(|found| found.start.column)
                .collect()
        })
        .collect();
    assert_eq!(
        vec![vec![], vec![0, 9], vec![9], vec![]],
        rows,
        "the command line, then the output's two rows"
    );
}

/// A grid four rows tall with `lines` lines printed through it, so that all
/// but the last few are in the scrollback.
fn printed(columns: u16, lines: &[&str]) -> Emulator {
    let mut emulator = Emulator::new(TerminalSize::new(columns, 4), 1000, Palette::default());
    for line in lines {
        emulator.advance(format!("{line}\r\n").as_bytes());
    }
    emulator
}

/// Every row of the grid, as one harvest reads it: the scrollback, and a
/// screen of fewer rows than the thousand past it that are asked for.
fn whole(emulator: &Emulator) -> BlockRows {
    emulator.harvest_rows(0, emulator.history_len() + 1000).0
}

/// Where each match starts, as (row, column).
fn starts(matches: &[Selection]) -> Vec<(usize, usize)> {
    matches
        .iter()
        .map(|found| (found.anchor.row, found.anchor.column))
        .collect()
}

#[test]
fn a_grid_read_in_pieces_is_the_grid_read_whole() {
    // Thirty lines through a four-row grid, read seven rows at a time: the
    // pieces end inside the scrollback, at its edge with the screen and on the
    // screen, and the store they make has to be the one a single read makes.
    let lines: Vec<String> = (0..30).map(|line| format!("line {line}")).collect();
    let emulator = printed(20, &lines.iter().map(String::as_str).collect::<Vec<_>>());
    let mut reads = 0;
    let pieces = harvest_grid(
        |first, last| {
            reads += 1;
            emulator.harvest_rows(first, last).0
        },
        7,
    );

    let expected = whole(&emulator);
    assert_eq!(expected.rows(), pieces.rows());
    assert_eq!(expected.to_text(), pieces.to_text());
    assert_eq!(
        expected.rows().div_ceil(7),
        reads,
        "the last piece, short of seven rows, is the one that ends the read"
    );
}

#[test]
fn the_grid_is_found_by_the_rules_the_list_is_found_by() {
    // The fold of capitals in every script, the case switch, the pattern
    // switch, and rows numbered from the oldest line of the scrollback — the
    // first match is far above the four rows on screen.
    let mut emulator = printed(
        20,
        &[
            "Ошибка one",
            "fine",
            "ошибка two",
            "x",
            "x",
            "x",
            "x",
            "ОШИБКА",
        ],
    );
    let rows = whole(&emulator);
    let id = emulator.snapshot().live_block.id;

    let folded = find_in_grid(&rows, id, "ошибка", FindOptions::default());
    assert_eq!(vec![(0, 0), (2, 0), (7, 0)], starts(&folded));
    assert!(folded.iter().all(|found| found.anchor.block == id));

    let told_apart = FindOptions {
        case_sensitive: true,
        regex: false,
    };
    assert_eq!(
        vec![(2, 0)],
        starts(&find_in_grid(&rows, id, "ошибка", told_apart))
    );

    let pattern = FindOptions {
        case_sensitive: false,
        regex: true,
    };
    assert_eq!(
        vec![(0, 7), (2, 7)],
        starts(&find_in_grid(&rows, id, "(ONE|t[w]o)", pattern))
    );
    assert!(
        find_in_grid(&rows, id, "(unclosed", pattern).is_empty(),
        "a pattern that does not compile finds nothing, and the bar says why"
    );
}

#[test]
fn a_line_the_terminal_folded_is_found_across_the_fold() {
    // Ten columns: the line goes over two rows, and a query that straddles the
    // fold is still in it, the way the list finds one.
    let emulator = printed(10, &["abcdefghijKLMNOP", "x", "x", "x", "x"]);
    let rows = whole(&emulator);
    let id = emulator.live_block().id;

    let found = find_in_grid(&rows, id, "hijkl", FindOptions::default());
    assert_eq!(1, found.len());
    assert_eq!((0, 7), (found[0].anchor.row, found[0].anchor.column));
    assert_eq!((1, 1), (found[0].head.row, found[0].head.column));
}

#[test]
fn only_the_matches_on_screen_are_handed_to_the_frame() {
    let lines: Vec<String> = (0..20).map(|line| format!("hit {line}")).collect();
    let mut emulator = printed(20, &lines.iter().map(String::as_str).collect::<Vec<_>>());
    let rows = whole(&emulator);
    let id = emulator.snapshot().live_block.id;
    let matches = find_in_grid(&rows, id, "hit", FindOptions::default());
    assert_eq!(20, matches.len());

    // At the bottom, the screen is the last three lines printed and the blank
    // row the cursor is on.
    let bottom = emulator.snapshot();
    let showing = on_screen(&matches, &bottom);
    assert_eq!(17..20, showing);

    emulator.scroll_lines(10);
    let scrolled = emulator.snapshot();
    let top = grid_first_row(&scrolled);
    let showing = on_screen(&matches, &scrolled);
    assert_eq!(top..top + 4, showing, "four rows, one match each");
    for found in &matches[showing] {
        let line = emulator_line(&scrolled, found.anchor.row - top);
        assert!(line.starts_with("hit"), "{line:?} is not a match's row");
    }
}

/// One viewport row of a snapshot, as text.
fn emulator_line(snapshot: &Snapshot, row: usize) -> String {
    snapshot
        .text()
        .lines()
        .nth(row)
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_match_off_screen_is_brought_a_third_of_the_way_down() {
    let lines: Vec<String> = (0..40).map(|line| format!("line {line}")).collect();
    let mut emulator = Emulator::new(TerminalSize::new(20, 9), 1000, Palette::default());
    for line in &lines {
        emulator.advance(format!("{line}\r\n").as_bytes());
    }
    let bottom = emulator.snapshot();
    let history = bottom.history_len;

    // On screen: nothing moves.
    assert_eq!(None, scroll_to_show(&bottom, history + 2));

    // Above it: back into the scrollback, until the row is three rows down a
    // nine-row screen.
    let lines_back = scroll_to_show(&bottom, 5).expect("row five is far above the screen");
    assert!(lines_back > 0, "back into the scrollback is positive");
    emulator.scroll_lines(lines_back);
    let scrolled = emulator.snapshot();
    assert_eq!(
        5 - 3,
        grid_first_row(&scrolled),
        "a third of the screen above it"
    );
    assert_eq!(Some("line 5"), scrolled.text().lines().nth(3));

    // Below it, from up there: forwards again, and a row on the live screen
    // takes the screen back to the bottom rather than past it.
    let forwards = scroll_to_show(&scrolled, history + 8).expect("the last row is off screen");
    assert!(forwards < 0, "towards the live output is negative");
    emulator.scroll_lines(forwards);
    assert_eq!(0, emulator.snapshot().display_offset);

    // The oldest row cannot be put a third of the way down; it goes to the top.
    let to_the_top = scroll_to_show(&emulator.snapshot(), 0).expect("row zero is off screen");
    emulator.scroll_lines(to_the_top);
    assert_eq!(0, grid_first_row(&emulator.snapshot()));
}

/// A question about a picture of the grid, with the bar's own switches.
fn asked(find: &PaneFind, snapshot: &Snapshot) -> GridAsk {
    GridAsk {
        query: find.query(),
        options: find.options(),
        revision: snapshot.revision,
        columns: snapshot.columns,
    }
}

#[test]
fn the_grid_is_searched_once_at_a_time_and_a_stale_answer_is_dropped() {
    let mut emulator = printed(20, &["zqvx"]);
    let one = emulator.snapshot();
    emulator.advance(b"more\r\n");
    let two = emulator.snapshot();
    assert_ne!(one.revision, two.revision);

    let find = PaneFind::new();
    find.open(false);
    find.input().edit(|editor| editor.set_text("qvx"));
    assert_eq!(
        None,
        find.grid_matches("qvx", 20),
        "nothing has been found yet"
    );

    let first = asked(&find, &one);
    assert!(find.ask_grid(&first), "the first question starts a search");
    assert!(
        !find.ask_grid(&first),
        "the same one on its way is not asked twice"
    );
    // Another question waits for the one on its way, whose landing asks again.
    let later = asked(&find, &two);
    assert!(!find.ask_grid(&later));

    // An answer nobody is waiting for is dropped.
    assert!(!find.land_grid(later.clone(), two.clone(), Arc::default(), Vec::new()));
    assert!(find.land_grid(first.clone(), one.clone(), Arc::default(), Vec::new()));
    assert_eq!(
        Some(0),
        find.grid_matches("qvx", 20).map(|found| found.len())
    );
    assert!(
        !find.ask_grid(&first),
        "a question the last answer was to is not asked again"
    );
    assert!(
        find.grid_rows(one.revision).is_some(),
        "the rows read for the first picture are kept"
    );
    assert!(
        find.grid_rows(two.revision).is_none(),
        "and are not taken for another picture"
    );

    // A different query, or the same one matched another way, is another
    // question: the answer to the last one is not its answer.
    assert_eq!(None, find.grid_matches("qv", 20));
    find.toggle(false);
    assert_eq!(None, find.grid_matches("qvx", 20));
    find.toggle(false);
    assert!(find.grid_matches("qvx", 20).is_some());
    // And a grid of another width has been folded again since: the rows the
    // answer names hold other text now.
    assert_eq!(None, find.grid_matches("qvx", 30));

    // Closing lets go of all of it, and an answer still on its way lands on
    // nothing.
    assert!(find.ask_grid(&later));
    find.close();
    assert!(!find.land_grid(later, two, Arc::default(), Vec::new()));
    assert_eq!(None, find.grid_matches("qvx", 20));
    assert!(find.grid_rows(one.revision).is_none());
}

#[test]
fn a_picture_whose_text_did_not_change_keeps_the_answer_found_in_it() {
    // A working agent animates its title several times a second, and every
    // frame of that is a new revision. Searching again for each one read the
    // whole scrollback for an answer that could not have changed.
    let mut emulator = printed(20, &["zqvx", "x", "x", "x", "x"]);
    let read_for = emulator.snapshot();
    let rows = Arc::new(whole(&emulator));
    let id = read_for.live_block.id;

    let find = PaneFind::new();
    find.open(false);
    find.input().edit(|editor| editor.set_text("qvx"));
    let first = asked(&find, &read_for);
    assert!(find.ask_grid(&first));
    let matches = find_in_grid(&rows, id, "qvx", find.options());
    assert_eq!(1, matches.len());
    assert!(find.land_grid(first, read_for.clone(), rows, matches));

    // The title, then the cursor, and nothing else.
    emulator.advance(b"\x1b]0;working\x07");
    let titled = emulator.snapshot();
    assert_ne!(
        read_for.revision, titled.revision,
        "a title is a new revision"
    );
    assert!(same_text(&read_for, &titled));
    find.carry_over(&titled);
    assert!(
        !find.ask_grid(&asked(&find, &titled)),
        "a picture that only changed its title is answered already"
    );
    assert!(find.grid_rows(titled.revision).is_some());

    emulator.advance(b"\x1b[1;1H");
    let moved = emulator.snapshot();
    assert_ne!(titled.revision, moved.revision);
    assert!(same_text(&titled, &moved), "the cursor is not text");
    find.carry_over(&moved);
    assert!(!find.ask_grid(&asked(&find, &moved)));

    // A letter printed is text, and is searched for.
    emulator.advance(b"q");
    let typed = emulator.snapshot();
    assert!(!same_text(&moved, &typed));
    find.carry_over(&typed);
    assert!(
        find.grid_rows(typed.revision).is_none(),
        "the rows read before the letter are not this picture's"
    );
    assert!(find.ask_grid(&asked(&find, &typed)));

    // Nor is a scroll the same picture: the screen shows other rows.
    let bottom = emulator.snapshot();
    emulator.scroll_lines(1);
    assert!(!same_text(&bottom, &emulator.snapshot()));
}

#[test]
fn a_picture_scrolled_back_is_not_taken_for_the_one_before_it() {
    // Scrolled back, the bottom of the live screen is under the viewport, and
    // a program can rewrite it in place — an agent's status line — without
    // adding a line. With its title ticking the revision moves, and every
    // cell a snapshot holds is as it was: the text found before was carried
    // over to a screen that now says something else.
    let mut emulator = printed(20, &["x", "x", "x", "x", "x", "x"]);
    emulator.scroll_lines(2);
    let before = emulator.snapshot();
    assert_eq!(2, before.display_offset, "the viewport is scrolled back");
    emulator.advance(b"\x1b]0;working\x07\x1b[4;1Hzqvx");
    let after = emulator.snapshot();
    assert_ne!(before.revision, after.revision);
    assert_eq!(
        (before.history_len, before.display_offset),
        (after.history_len, after.display_offset),
        "no line was added, and the viewport stayed where it was"
    );
    assert!(
        before
            .cells
            .iter()
            .zip(&after.cells)
            .all(|(a, b)| a.c == b.c),
        "the rewritten row is not in view"
    );
    let rows = whole(&emulator);
    assert!(
        (0..rows.rows()).any(|row| rows.text(row).contains("zqvx")),
        "the screen under the viewport says something else now"
    );
    assert!(!same_text(&before, &after));

    // So a bar over it reads the grid again rather than keeping its answer.
    let find = PaneFind::new();
    find.open(false);
    find.input().edit(|editor| editor.set_text("qvx"));
    let first = asked(&find, &before);
    assert!(find.ask_grid(&first));
    assert!(find.land_grid(first, before, Arc::new(BlockRows::default()), Vec::new()));
    find.carry_over(&after);
    assert!(find.grid_rows(after.revision).is_none());
    assert!(find.ask_grid(&asked(&find, &after)));
}

/// A bar with an answer to `qvx` in the first of two pictures of a grid, and
/// a read of the second waiting out its pause: the grid moved under the
/// answer on screen. The pause is handed back for the test to wait on, as the
/// worker that reads would.
fn paused() -> (PaneFind, Arc<Snapshot>, GridAsk, GridPause) {
    let mut emulator = printed(20, &["zqvx"]);
    let one = emulator.snapshot();
    emulator.advance(b"more\r\n");
    let two = emulator.snapshot();

    let find = PaneFind::new();
    find.open(false);
    find.input().edit(|editor| editor.set_text("qvx"));
    let first = asked(&find, &one);
    assert!(find.ask_grid(&first));
    assert!(find.land_grid(first, one, Arc::default(), Vec::new()));

    let moved = asked(&find, &two);
    assert!(find.ask_grid(&moved), "the grid moved, and is read again");
    let pause = find.pause_grid();
    (find, two, moved, pause)
}

/// How long a pause that something should have ended is waited on before a
/// test calls it unended. Far past any scheduler's hiccup, and short of a
/// test that hangs.
const UNENDED: Duration = Duration::from_secs(30);

#[test]
fn a_changed_question_takes_the_place_of_a_read_waiting_out_its_pause() {
    // A program printing without stop keeps a paused read on its way nearly
    // all the time, and a letter typed into the bar used to wait out the rest
    // of that pause, and its read, before its own search could start: the
    // count went blank for up to a quarter of a second a letter.
    let (find, two, moved, pause) = paused();

    // The grid moving again is the same question, and waits its turn.
    let again = GridAsk {
        revision: two.revision + 1,
        ..moved.clone()
    };
    assert!(!find.ask_grid(&again));

    // A letter typed is not.
    find.input().edit(|editor| editor.set_text("qvxm"));
    let typed = asked(&find, &two);
    assert!(
        find.ask_grid(&typed),
        "a changed question waited out a pause that was for the grid's sake"
    );
    // The paused read is not made, and lets its worker go at once.
    let started = std::time::Instant::now();
    assert!(!pause.wait(UNENDED), "the paused read went ahead as well");
    assert!(
        started.elapsed() < UNENDED / 2,
        "the paused read held its worker for the rest of the pause"
    );
    // And what it would have found is nobody's answer.
    assert!(!find.land_grid(moved, two.clone(), Arc::default(), Vec::new()));
    assert!(find.land_grid(typed, two, Arc::default(), Vec::new()));
}

#[test]
fn a_switch_flipped_or_a_resize_takes_the_place_of_a_paused_read_too() {
    let (find, two, _, _pause) = paused();
    find.toggle(false);
    assert!(
        find.ask_grid(&asked(&find, &two)),
        "a switch flipped waited"
    );

    let (find, two, moved, _pause) = paused();
    let wider = GridAsk {
        columns: two.columns + 10,
        ..moved
    };
    assert!(find.ask_grid(&wider), "a resize waited");
}

#[test]
fn a_read_that_has_begun_is_never_run_beside_another() {
    // The pause is over and the worker is reading: a letter typed now waits
    // for that read to land, which asks again, rather than reading the
    // scrollback a second time beside it.
    let (find, two, _, pause) = paused();
    assert!(
        pause.wait(Duration::ZERO),
        "the read goes ahead after its pause"
    );
    find.input().edit(|editor| editor.set_text("qvxm"));
    assert!(!find.ask_grid(&asked(&find, &two)));
}

#[test]
fn a_search_let_go_of_leaves_the_answer_and_frees_the_question() {
    // A search that comes back to a pane nobody can see any more is let go
    // of. Declining what it found was not enough on its own: the search still
    // counted as on its way, so the pane coming back on screen asked nothing
    // and the count stayed where it was.
    let (find, two, moved, _pause) = paused();
    find.drop_grid(&moved);
    assert!(
        !find.land_grid(moved.clone(), two, Arc::default(), Vec::new()),
        "what a search let go of found was taken in"
    );
    assert_eq!(
        Some(0),
        find.grid_matches("qvx", 20).map(|found| found.len()),
        "the answer on screen went with it"
    );
    assert!(
        find.ask_grid(&moved),
        "the same question waited for a search that had been let go of"
    );

    // Only the search being waited for: one that set out before is not it.
    let (find, _, moved, _pause) = paused();
    find.drop_grid(&GridAsk {
        query: "qv".to_owned(),
        ..moved.clone()
    });
    assert!(
        !find.ask_grid(&moved),
        "a search still on its way was let go of for another"
    );
}

#[test]
fn closing_the_bar_ends_a_pause_at_once() {
    let (find, _, _, pause) = paused();
    find.close();
    let started = std::time::Instant::now();
    assert!(
        !pause.wait(UNENDED),
        "a read went ahead for a bar that closed"
    );
    assert!(started.elapsed() < UNENDED / 2);
}
