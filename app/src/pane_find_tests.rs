//! What a find bar keeps of a search between frames, over blocks a real
//! emulator made: that a finished block is read once, and that what is kept
//! is what a fresh walk of the same blocks would find.

use std::sync::Arc;

use crook_terminal::{Emulator, Palette, TerminalSize};

use super::*;
use crate::selection::stored_rows_searched;

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
    find.open();
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
