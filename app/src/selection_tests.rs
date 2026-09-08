//! What a selection covers and what it copies, over blocks a real emulator
//! made.
//!
//! Built by feeding OSC 133 at an [`Emulator`] rather than by hand, so that
//! the blocks under test are the blocks the shell integration actually
//! produces — the harvested rows, the widths they were harvested at, and the
//! fold flags the terminal put on them.

use std::sync::Arc;

use crook_terminal::{Emulator, Palette, TerminalSize};

use crate::pane_selection::PaneSelection;

use super::*;

/// OSC 133 prompt start.
const A: &str = "\x1b]133;A\x07";
/// OSC 133 prompt end.
const B: &str = "\x1b]133;B\x07";
/// OSC 133 output start.
const C: &str = "\x1b]133;C\x07";
/// OSC 133 command finished, successfully.
const D: &str = "\x1b]133;D;0\x07";

/// A session: the blocks its commands left behind, and the grid the open one
/// is still in.
struct Session {
    finished: BlockHistory,
    snapshot: Arc<Snapshot>,
}

impl Session {
    /// The blocks a selection in it addresses.
    fn blocks(&self) -> Blocks<'_> {
        Blocks::list(&self.finished, &self.snapshot)
    }

    /// The id of the block at `index`, the open one being last.
    fn id(&self, index: usize) -> BlockId {
        self.blocks()
            .item(index)
            .unwrap_or_else(|| panic!("there is no block {index}"))
            .id
    }

    /// What a drag from one cell to another would copy.
    fn dragged(&self, from: (usize, usize, usize), to: (usize, usize, usize)) -> Option<String> {
        self.taken(SelectionKind::Simple, from, to)
    }

    /// What a gesture of `kind` from one cell to another would copy.
    ///
    /// Each end is a block index, a row of it and a column: the drag starts on
    /// the left of the first and finishes on the right of the last, which is
    /// what a pointer dragged across the two characters does.
    fn taken(
        &self,
        kind: SelectionKind,
        from: (usize, usize, usize),
        to: (usize, usize, usize),
    ) -> Option<String> {
        let selection = Selection::new(
            kind,
            Anchor::new(self.id(from.0), from.1, from.2, CellSide::Left),
            Anchor::new(self.id(to.0), to.1, to.2, CellSide::Right),
        );
        selection.text(&self.blocks())
    }
}

/// Runs `commands` through an emulator that reports command boundaries, and
/// hands back the session they left.
fn session(columns: u16, rows: u16, commands: &[&str]) -> Session {
    let mut emulator = Emulator::new(TerminalSize::new(columns, rows), 1000, Palette::default());
    emulator.advance(A.as_bytes());
    for command in commands {
        emulator.advance(format!("{B}{command}\r\n{C}").as_bytes());
        emulator.advance(command_output(command).as_bytes());
        emulator.advance(format!("{D}{A}$ ").as_bytes());
    }

    Session {
        finished: BlockHistory::new(emulator.blocks().iter().cloned().map(Arc::new).collect(), 0),
        snapshot: emulator.snapshot(),
    }
}

/// What a command in these tests prints: its own name in capitals, once per
/// line, so that every block is telling you which one it is.
fn command_output(command: &str) -> String {
    format!(
        "{} one\r\n{} two\r\n",
        command.to_uppercase(),
        command.to_uppercase()
    )
}

#[test]
fn test_a_drag_across_a_finished_block_takes_the_cells_it_covered() {
    // **The bug this whole module exists for.** A command finishes, its rows
    // are harvested out of the emulator, and a drag across them still selects
    // them — because a block's rows are addressed as a block's rows rather
    // than as cells of a grid they are no longer in.
    let session = session(40, 12, &["first", "second"]);
    assert_eq!(
        3,
        session.blocks().len(),
        "two commands and the open prompt"
    );

    // Row 0 of a block is the prompt and the command line it was typed on;
    // rows 1 and 2 are what the command printed.
    assert_eq!(
        Some("FIRST".to_owned()),
        session.dragged((0, 1, 0), (0, 1, 4))
    );
    assert_eq!(
        Some("SECOND one".to_owned()),
        session.dragged((1, 1, 0), (1, 1, 9))
    );
}

#[test]
fn test_a_selection_spanning_three_blocks_takes_the_middle_one_whole() {
    // The ends partially, the interior entirely, joined with a single newline
    // — and nothing at all from the padding, the dividers or the prompts
    // between them beyond the one break.
    let session = session(40, 20, &["one", "two", "three"]);
    let copied = session
        .dragged((0, 2, 0), (2, 1, 8))
        .expect("three blocks are selected");

    assert_eq!(
        "ONE two\n$ two\nTWO one\nTWO two\n$ three\nTHREE one", copied,
        "the ends partially, the block between them whole"
    );
}

#[test]
fn test_a_selection_inside_the_open_block_still_works() {
    // The open block is the last item of the same list, with its rows read out
    // of the snapshot rather than out of a store. No second code path, and no
    // second behaviour.
    let session = session(40, 12, &["only"]);
    let open = session.blocks().len() - 1;
    assert_eq!(
        Some("$".to_owned()),
        session.dragged((open, 0, 0), (open, 0, 0))
    );
}

#[test]
fn test_a_word_and_a_line_can_be_taken_at_a_block_s_first_and_last_row() {
    // A double and a triple click, at both edges of a block — where an
    // expansion that ran off the end of one would reach into the next.
    let session = session(40, 12, &["alpha"]);
    let last = session.blocks().item(0).expect("a block").rows.count() - 1;

    assert_eq!(
        Some("alpha".to_owned()),
        session.taken(SelectionKind::Semantic, (0, 0, 3), (0, 0, 3)),
        "the command on the block's first row"
    );
    assert_eq!(
        Some("ALPHA two".to_owned()),
        session.taken(SelectionKind::Lines, (0, last, 2), (0, last, 2)),
        "the whole of the block's last row"
    );
    assert_eq!(
        Some("ALPHA".to_owned()),
        session.taken(SelectionKind::Semantic, (0, last, 2), (0, last, 2)),
        "and one word of it"
    );
}

#[test]
fn test_trailing_blanks_do_not_come_along() {
    // A row is as long as what was printed into it; the rest is a grid having
    // to hold something. Dragging to the end of a short line in a wide pane
    // must not paste forty spaces.
    let session = session(40, 12, &["short"]);
    let copied = session
        .dragged((0, 1, 0), (0, 1, 39))
        .expect("the whole row is selected");
    assert_eq!("SHORT one", copied);
}

#[test]
fn test_a_folded_line_copies_back_as_the_one_line_it_is() {
    // A line too long for the pane is one line of text on two rows, and the
    // fold is a place the terminal put it rather than something the shell
    // printed. This has to stay true now that the rows are in a store rather
    // than in the emulator that folded them.
    let mut emulator = Emulator::new(TerminalSize::new(20, 8), 1000, Palette::default());
    let long = "abcdefghijklmnopqrstuvwxyz";
    emulator.advance(format!("{A}${B}long\r\n{C}{long}\r\n{D}{A}$ ").as_bytes());
    let session = Session {
        finished: BlockHistory::new(emulator.blocks().iter().cloned().map(Arc::new).collect(), 0),
        snapshot: emulator.snapshot(),
    };

    // Row 1 is the folded line and row 2 is what it folded onto.
    assert_eq!(Some(long.to_owned()), session.dragged((0, 1, 0), (0, 2, 5)));
}

#[test]
fn test_a_wide_character_copies_as_one_character() {
    // Its glyph is drawn once across two columns, and the grid holds a space
    // in the second of them. Copying that space would put one inside every CJK
    // word and after every emoji.
    let mut emulator = Emulator::new(TerminalSize::new(20, 8), 1000, Palette::default());
    emulator.advance(format!("{A}${B}wide\r\n{C}a漢b\r\n{D}{A}$ ").as_bytes());
    let session = Session {
        finished: BlockHistory::new(emulator.blocks().iter().cloned().map(Arc::new).collect(), 0),
        snapshot: emulator.snapshot(),
    };

    // Reaching either of the character's two columns takes all of it, once.
    assert_eq!(
        Some("a漢".to_owned()),
        session.dragged((0, 1, 0), (0, 1, 1))
    );
    assert_eq!(
        Some("a漢".to_owned()),
        session.dragged((0, 1, 0), (0, 1, 2))
    );
    assert_eq!(
        Some("漢b".to_owned()),
        session.dragged((0, 1, 1), (0, 1, 3))
    );
    assert_eq!(
        Some("a".to_owned()),
        session.dragged((0, 1, 0), (0, 1, 0)),
        "a selection that stops before it must not drag it in"
    );
}

#[test]
fn test_a_press_that_never_moved_selects_nothing() {
    let session = session(40, 12, &["click"]);
    let at = Anchor::new(session.id(0), 1, 3, CellSide::Left);
    assert_eq!(
        None,
        Selection::new(SelectionKind::Simple, at, at).region(&session.blocks())
    );
    assert_eq!(
        None,
        Selection::new(SelectionKind::Block, at, at).region(&session.blocks())
    );
}

#[test]
fn test_a_selection_dragged_backwards_covers_the_same_cells() {
    let session = session(40, 12, &["back", "wards"]);
    let forwards = Selection::new(
        SelectionKind::Simple,
        Anchor::new(session.id(0), 1, 0, CellSide::Left),
        Anchor::new(session.id(1), 1, 4, CellSide::Right),
    );
    let backwards = Selection::new(SelectionKind::Simple, forwards.head, forwards.anchor);
    assert_eq!(
        forwards.text(&session.blocks()),
        backwards.text(&session.blocks())
    );
}

#[test]
fn test_a_block_selection_takes_a_column_out_of_every_row_it_crosses() {
    // Alt-drag: the same columns out of aligned output, without the rest of
    // every line coming with it.
    let session = session(40, 12, &["column"]);
    assert_eq!(
        Some("COL\nCOL".to_owned()),
        session.taken(SelectionKind::Block, (0, 1, 0), (0, 2, 2))
    );
}

#[test]
fn test_a_selection_holds_still_while_the_next_command_runs() {
    // Nothing about a block moves when another one is added after it, so the
    // very same anchors copy the very same text.
    let before = session(40, 20, &["kept"]);
    let taken = before.dragged((0, 1, 0), (0, 1, 8));

    let after = session(40, 20, &["kept", "and", "more"]);
    assert_eq!(taken, after.dragged((0, 1, 0), (0, 1, 8)));
    assert_eq!(Some("KEPT one".to_owned()), taken);
}

#[test]
fn test_text_can_be_found_by_what_it_says() {
    // What `--select-output` aims with, and it has to walk the finished blocks
    // as well as the open one — there is nothing to aim a pointer with in a
    // headless run.
    let session = session(40, 20, &["find", "me"]);
    let blocks = session.blocks();

    let found = blocks.find("FIND two").expect("the output shows it");
    assert_eq!(Some("FIND two".to_owned()), found.text(&blocks));

    // Across a block boundary, which is where the newline the walk inserts has
    // to be the newline a copy would produce.
    let across = blocks.find("FIND two\n$ me").expect("it spans two blocks");
    assert_eq!(Some("FIND two\n$ me".to_owned()), across.text(&blocks));
    assert_eq!(None, blocks.find("not on this screen"));
    assert_eq!(None, blocks.find(""));
}

// ---------------------------------------------------------------------------
// What moves under an anchor.
//
// The address space promises that an anchor names the same characters after
// the pane has been scrolled, resized, overflowed, taken over by a full-screen
// program, or has evicted what is above it. These are that promise, and the
// one place it is knowingly not kept.

/// A grid with `lines` lines already scrolled through it, and nothing else.
fn scrolled_grid(columns: u16, rows: u16, scrollback: usize, lines: usize) -> Emulator {
    let mut emulator = Emulator::new(
        TerminalSize::new(columns, rows),
        scrollback,
        Palette::default(),
    );
    for line in 0..lines {
        emulator.advance(format!("line{line:02}\r\n").as_bytes());
    }
    emulator
}

/// What a copy off the grid takes: the rows read back out of the emulator, the
/// way [`crate::workspace::pane_output::Output::selected_text`] reads them.
fn copied_off_the_grid(emulator: &mut Emulator, selection: Selection) -> Option<String> {
    let snapshot = emulator.snapshot();
    let slack = snapshot.rows;
    let (first, last) = (
        selection
            .anchor
            .row
            .min(selection.head.row)
            .saturating_sub(slack),
        selection.anchor.row.max(selection.head.row) + slack,
    );
    let (rows, at) = emulator.harvest_rows(first, last);
    selection.text(&Blocks::one(
        selection.anchor.block,
        Rows::Stored(&rows),
        at,
    ))
}

#[test]
fn a_double_click_on_the_grid_survives_scrolling_back_to_the_bottom() {
    // `grid_first_row` is `history_len - display_offset`, so scrolling back
    // down *raises* the number the top row answers to while the anchor keeps
    // the one it was made at — until the row it names is above the viewport
    // altogether. Painting it must say "not on screen" rather than subtract
    // its way off the end of the grid.
    let mut emulator = scrolled_grid(20, 4, 1000, 20);
    emulator.scroll_lines(5);
    let scrolled = emulator.snapshot();
    let id = scrolled.live_block.id;
    let row = grid_first_row(&scrolled);

    let selection = Selection::new(
        SelectionKind::Semantic,
        Anchor::new(id, row, 1, CellSide::Left),
        Anchor::new(id, row, 1, CellSide::Left),
    );
    let word = selection
        .text(&Blocks::grid(&scrolled, id))
        .expect("a word under the pointer");
    assert_eq!("line12", word);

    emulator.scroll_to_bottom();
    let bottom = emulator.snapshot();
    // The highlight is off the top of the screen, so there is nothing to
    // paint and nothing to grow it against — and asking is not a crash.
    let _ = selection.text(&Blocks::grid(&bottom, id));
    // The copy does not read the screen, so it still has the whole word.
    assert_eq!(
        Some(word),
        copied_off_the_grid(&mut emulator, selection),
        "the wheel moves the viewport, not the selection"
    );
}

#[test]
fn a_double_click_survives_the_pane_getting_shorter() {
    // A height-only resize re-wraps nothing, so `PaneSelection::reflowed`
    // rightly keeps the selection — and leaves an anchor on a row the open
    // block no longer has. Every reader has to answer that rather than index
    // the snapshot with it.
    let mut emulator = Emulator::new(TerminalSize::new(40, 12), 1000, Palette::default());
    emulator.advance(A.as_bytes());
    emulator.advance(format!("{B}live\r\n{C}").as_bytes());
    for line in 0..8 {
        emulator.advance(format!("row{line}\r\n").as_bytes());
    }

    let tall = emulator.snapshot();
    let id = tall.live_block.id;
    let none = BlockHistory::default();
    let count = Blocks::list(&none, &tall)
        .item(0)
        .expect("the open block")
        .rows
        .count();
    let selection = Selection::new(
        SelectionKind::Semantic,
        Anchor::new(id, count - 1, 1, CellSide::Left),
        Anchor::new(id, count - 1, 1, CellSide::Left),
    );

    emulator.resize(TerminalSize::new(40, 5));
    let short = emulator.snapshot();
    let _ = selection.text(&Blocks::list(&none, &short));
}

#[test]
fn a_selection_made_on_the_list_is_never_resolved_against_the_grid() {
    // A full-screen program takes the pane to the grid surface. If the anchors
    // were re-read there, a finished block's rows and the program's screen
    // would be one address space and the copy would hand over whatever `vim`
    // had on row one.
    let session = session(40, 12, &["first", "second"]);
    let block = session.finished.get(0).expect("a finished block").clone();
    let selection = Selection::new(
        SelectionKind::Simple,
        Anchor::new(block.id, 1, 0, CellSide::Left),
        Anchor::new(block.id, 1, 4, CellSide::Right),
    );
    assert_eq!(Some("FIRST".to_owned()), selection.text(&session.blocks()));

    let pane = PaneSelection::new();
    pane.select(selection, 40, Cells::List);
    assert!(
        pane.selection_in(Cells::Grid).is_none(),
        "the grid cannot read a selection the list's rows were numbered for"
    );
    assert!(pane.resurfaced(Cells::Grid), "so the pane lets go of it");
    assert!(!pane.has_selection());
}

#[test]
fn a_selection_holds_its_block_while_the_open_one_overflows_the_viewport() {
    // The everyday route to the grid surface: the running command prints past
    // the top of the screen. The blocks above it are still in the store, and a
    // selection in one of them still names the same characters — which is what
    // makes letting go of it on the surface change a policy rather than a
    // rescue.
    let mut emulator = Emulator::new(TerminalSize::new(40, 8), 1000, Palette::default());
    emulator.advance(A.as_bytes());
    emulator.advance(format!("{B}first\r\n{C}").as_bytes());
    emulator.advance(b"FIRST one\r\nFIRST two\r\n");
    emulator.advance(format!("{D}{A}{B}long\r\n{C}").as_bytes());

    let finished = BlockHistory::new(emulator.blocks().iter().cloned().map(Arc::new).collect(), 0);
    let block = finished.get(0).expect("a finished block").clone();
    let selection = Selection::new(
        SelectionKind::Simple,
        Anchor::new(block.id, 1, 0, CellSide::Left),
        Anchor::new(block.id, 1, 8, CellSide::Right),
    );

    for line in 0..30 {
        emulator.advance(format!("noise{line:02}\r\n").as_bytes());
    }
    let after = emulator.snapshot();
    assert!(after.live_block.top_row < 0, "the pane is on the grid now");
    assert_eq!(
        Some("FIRST one".to_owned()),
        selection.text(&Blocks::list(&finished, &after)),
        "the block the selection is in did not go anywhere"
    );
}

#[test]
fn a_grid_selection_slides_when_a_full_scrollback_evicts() {
    // **A known limit, pinned here so that it changes on purpose.** Grid rows
    // are numbered from the oldest line the history holds, and a history at
    // its cap drops its oldest line for every new one — which renumbers every
    // row under the selection. Counting the dropped lines is not something the
    // emulator underneath can answer; see `grid_first_row`.
    let mut emulator = scrolled_grid(20, 4, 5, 9);
    let before = emulator.snapshot();
    let id = before.live_block.id;
    let row = grid_first_row(&before);
    let selection = Selection::new(
        SelectionKind::Simple,
        Anchor::new(id, row, 0, CellSide::Left),
        Anchor::new(id, row, 5, CellSide::Right),
    );
    assert_eq!(
        Some("line06".to_owned()),
        selection.text(&Blocks::grid(&before, id))
    );

    for line in 9..13 {
        emulator.advance(format!("line{line:02}\r\n").as_bytes());
    }
    let after = emulator.snapshot();
    assert_eq!(
        Some("line10".to_owned()),
        selection.text(&Blocks::grid(&after, id)),
        "four lines evicted, four rows of drift — and the highlight moved with it"
    );
}

#[test]
fn a_wide_character_on_a_scrolled_grid_is_still_copied_whole() {
    // `columns_on` is handed the row in the *anchor's* numbering and the item
    // that says what that numbering is, so the cell it grows against is the
    // cell it highlights — on a grid with a scrollback above it as much as in
    // a block whose rows start at zero.
    let mut emulator = Emulator::new(TerminalSize::new(20, 4), 100, Palette::default());
    for line in 0..8 {
        emulator.advance(format!("line{line}\r\n").as_bytes());
    }
    emulator.advance("\u{4e2d}\u{6587} ok\r\n".as_bytes());

    let snapshot = emulator.snapshot();
    let id = snapshot.live_block.id;
    let row = grid_first_row(&snapshot) + 2;
    // Column 1 is the trailing half of the first double-width character.
    let selection = Selection::new(
        SelectionKind::Simple,
        Anchor::new(id, row, 1, CellSide::Left),
        Anchor::new(id, row, 1, CellSide::Right),
    );
    assert_eq!(
        Some("\u{4e2d}".to_owned()),
        selection.text(&Blocks::grid(&snapshot, id)),
        "reaching either half of a wide character reaches all of it"
    );
}

#[test]
fn an_evicted_block_leaves_the_highlight_and_the_copy_agreeing() {
    // The store caps blocks and drops from the front. What is still in the
    // list is still selected, still painted as selected, and still copied.
    let session = session(40, 20, &["one", "two", "three"]);
    let first = session.finished.get(0).expect("block one").id;
    let last = session.finished.get(2).expect("block three").clone();
    let selection = Selection::new(
        SelectionKind::Simple,
        Anchor::new(first, 1, 0, CellSide::Left),
        Anchor::new(last.id, 1, 4, CellSide::Right),
    );

    let kept = BlockHistory::new(session.finished.iter().skip(1).cloned().collect(), 1);
    let evicted = Blocks::list(&kept, &session.snapshot);
    let region = selection.region(&evicted).expect("a region over the rest");
    let item = Item {
        id: last.id,
        rows: Rows::Stored(&last.rows),
        first: 0,
    };
    assert!(region.columns_on(&item, 1).is_some());
    assert_eq!(
        Some("$ two\nTWO one\nTWO two\n$ three\nTHREE".to_owned()),
        selection.text(&evicted),
        "what is painted as selected is what a copy takes"
    );
}

#[test]
fn find_all_finds_every_occurrence_across_blocks_and_folds_ascii_case() {
    // Two commands, each printing its own name in capitals twice. The word
    // "one" appears once per command's output, so a search for it finds two,
    // one in each block, and the search is ASCII case-insensitive.
    let session = session(40, 12, &["alpha", "beta"]);

    let matches = session.blocks().find_all("one");
    assert_eq!(
        2,
        matches.len(),
        "`one` ends each block's first output line"
    );
    assert!(
        matches[0].anchor.order() < matches[1].anchor.order(),
        "matches come back in reading order, oldest block first"
    );

    // What each match covers is the word, wherever it fell.
    for found in &matches {
        assert_eq!(Some("one".to_owned()), found.text(&session.blocks()));
    }

    // Case folds over ASCII: a block's first row is its prompt and command
    // line, so `alpha` matches once there and once per upper-cased output
    // line — three times, all in the one block whose command it was — and the
    // query's own case does not change the count.
    assert_eq!(3, session.blocks().find_all("alpha").len());
    assert_eq!(3, session.blocks().find_all("beta").len());
    assert_eq!(3, session.blocks().find_all("BETA").len());

    // Nothing matches nothing, and an empty needle finds nothing rather than
    // everything.
    assert!(session.blocks().find_all("nowhere").is_empty());
    assert!(session.blocks().find_all("").is_empty());
}
