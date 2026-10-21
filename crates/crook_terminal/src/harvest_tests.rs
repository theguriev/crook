use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::Config;

use super::*;
use crate::blocks::LiveBlock;
use crate::snapshot::TerminalSize;

/// A terminal holding whatever the bytes drew, ready to harvest out of.
fn term(size: TerminalSize, bytes: &str) -> Term<VoidListener> {
    let mut term = Term::new(Config::default(), &size, VoidListener);
    let mut parser = alacritty_terminal::vte::ansi::Processor::<
        alacritty_terminal::vte::ansi::StdSyncHandler,
    >::new();
    parser.advance(&mut term, bytes.as_bytes());
    term
}

/// Every row of a store, materialised through the one method that hands rows
/// to the renderer.
fn cells(rows: &BlockRows) -> Vec<Vec<SnapshotCell>> {
    let mut scratch = Vec::new();
    (0..rows.rows())
        .map(|row| {
            rows.materialise(row, &mut scratch);
            scratch.clone()
        })
        .collect()
}

#[test]
fn test_a_harvested_row_materialises_to_the_cells_it_came_from() {
    let size = TerminalSize::new(20, 4);
    let palette = Palette::default();
    let term = term(size, "\x1b[31mred\x1b[0m plain\r\n\x1b[1;4mbold\x1b[0m");

    let rows = harvest(&term, &palette, 0, 1);
    assert_eq!(2, rows.rows());
    assert_eq!(20, rows.columns());

    let grid = term.grid();
    for (row, materialised) in cells(&rows).iter().enumerate() {
        assert_eq!(20, materialised.len(), "row {row} lost its width");
        for (column, cell) in grid[Line(row as i32)][..].iter().enumerate() {
            assert_eq!(
                snapshot::convert(cell, &palette, term.colors()),
                materialised[column],
                "row {row} column {column}"
            );
        }
    }
}

#[test]
fn test_ordinary_output_is_one_run_a_row() {
    // The claim the whole storage decision rests on: a row nobody coloured
    // costs its bytes and a single twelve-byte run, whatever the grid's width.
    let rows = harvest(
        &term(TerminalSize::new(200, 4), "an ordinary line of output"),
        &Palette::default(),
        0,
        0,
    );
    assert_eq!(1, rows.runs(0).unwrap().len());
    assert_eq!("an ordinary line of output", rows.text(0));
}

#[test]
fn test_a_coloured_tail_survives_the_trailing_blank_trim() {
    // `\e[K` after setting a background paints the rest of the row, and the
    // cells it painted are blanks. Trimming the *text* must not trim them:
    // a status line that lost its bar is a rendering bug people report.
    let size = TerminalSize::new(20, 2);
    let palette = Palette::default();
    let term = term(size, "\x1b[41mbar\x1b[K");
    let rows = harvest(&term, &palette, 0, 0);

    assert_eq!("bar", rows.text(0), "the text keeps no trailing blanks");
    let mut scratch = Vec::new();
    rows.materialise(0, &mut scratch);
    assert_eq!(20, scratch.len());
    assert!(
        scratch
            .iter()
            .all(|cell| cell.background == palette.ansi[1]),
        "the painted tail was trimmed away with the text"
    );
    assert_eq!(' ', scratch[19].c);
}

#[test]
fn test_zero_width_characters_come_back_with_the_row_they_belong_to() {
    let size = TerminalSize::new(20, 2);
    let term = term(size, "e\u{301}f");
    let rows = harvest(&term, &Palette::default(), 0, 0);

    let mut scratch = Vec::new();
    let combining = rows.materialise(0, &mut scratch);
    assert_eq!('e', scratch[0].c);
    assert_eq!('f', scratch[1].c);
    assert_eq!(1, combining.len());
    assert_eq!(0, combining[0].column);
    assert_eq!(&['\u{301}'], &*combining[0].characters);

    // And a row with none says so without allocating anything to say it.
    assert!(rows.materialise(1, &mut scratch).is_empty());
}

#[test]
fn test_a_double_width_character_keeps_both_of_its_columns() {
    let size = TerminalSize::new(20, 2);
    let palette = Palette::default();
    let term = term(size, "\u{6f22}x");
    let rows = harvest(&term, &palette, 0, 0);

    let mut scratch = Vec::new();
    rows.materialise(0, &mut scratch);
    assert_eq!('\u{6f22}', scratch[0].c);
    assert!(scratch[0].flags.contains(CellFlags::WIDE));
    assert!(scratch[1].flags.contains(CellFlags::WIDE_SPACER));
    assert_eq!('x', scratch[2].c);
    // One `char` per cell, so the spacer's blank is in the text too.
    assert_eq!("\u{6f22} x", rows.text(0));
}

#[test]
fn test_a_range_outside_the_grid_yields_no_rows() {
    let size = TerminalSize::new(20, 4);
    let palette = Palette::default();
    let term = term(size, "one\r\ntwo");

    assert!(harvest(&term, &palette, 2, 1).is_empty());
    assert!(harvest(&term, &palette, 40, 80).is_empty());
    // Clamped rather than refused: a top that has scrolled out of the history
    // gives the rows that are left, which merges rather than splits.
    assert_eq!(4, harvest(&term, &palette, -50, 50).rows());
}

#[test]
fn test_blank_rows_are_recognisable() {
    let size = TerminalSize::new(20, 4);
    let palette = Palette::default();

    assert!(harvest(&term(size, ""), &palette, 0, 3).is_blank());
    assert!(!harvest(&term(size, "\r\n\r\nx"), &palette, 0, 3).is_blank());
}

#[test]
fn test_the_text_of_a_block_is_its_rows_joined_by_newlines() {
    let size = TerminalSize::new(20, 4);
    let rows = harvest(
        &term(size, "first\r\nsecond\r\n\r\nfourth"),
        &Palette::default(),
        0,
        3,
    );
    assert_eq!("first\nsecond\n\nfourth", rows.to_text());
}

#[test]
fn test_a_screenful_costs_a_fraction_of_the_cells_it_came_from() {
    // The reason this type exists. A screenful of `Vec<SnapshotCell>` is
    // twelve bytes a cell; ten thousand rows of it is 24 MB per pane, forever,
    // with nothing evicting it.
    let (columns, rows) = (200, 50);
    let size = TerminalSize::new(columns, rows);
    let mut output = String::new();
    for line in 0..rows {
        output.push_str(&format!(
            "drwxr-xr-x  7 user  staff  224 Feb 3 file-{line}\r\n"
        ));
    }

    let harvested = harvest(
        &term(size, &output),
        &Palette::default(),
        0,
        i32::from(rows) - 1,
    );
    assert_eq!(usize::from(rows), harvested.rows());

    let as_cells = harvested.rows() * harvested.columns() * size_of::<SnapshotCell>();
    let used = harvested.memory_usage();
    assert!(
        used * 8 < as_cells,
        "{used} bytes is not a small fraction of the {as_cells} a cell array would cost"
    );
}

#[test]
fn test_copied_text_drops_wide_spacers_and_keeps_combining_marks() {
    // What the clipboard gets when a block is copied, and it has to be what a
    // selection over the same rows would give: the grid holds a space in the
    // trailing column of every double-width character, and copying that puts
    // one inside every CJK word and after every emoji. The accents live beside
    // the row and are dropped by anything that reads the characters alone.
    let size = TerminalSize::new(20, 4);
    let term = term(size, "a\u{65e5}b\r\ne\u{301}f");
    let rows = harvest(&term, &Palette::default(), 0, 1);

    assert_eq!(
        "a\u{65e5} b",
        rows.text(0),
        "the grid's own row is unchanged"
    );
    assert_eq!("a\u{65e5}b\ne\u{301}f", rows.to_text());
    assert_eq!(
        snapshot::build(
            &term,
            &Palette::default(),
            0,
            None,
            LiveBlock::default(),
            false,
            snapshot::Spilled::none(&crate::spill::Spill::default()),
        )
        .text()
        .trim_end()
        .to_owned(),
        rows.to_text(),
        "a copied block and a copied selection disagree"
    );
}

#[test]
fn a_command_kept_without_its_rows_is_laid_out_the_way_the_grid_drew_it() {
    // Wide characters, one that does not fit before the edge, and an accent
    // that stacks on the letter before it: the row kept for a wiped block
    // has to be the row the grid would have had.
    let palette = Palette::default();
    for (columns, text) in [
        (4, "日本語"),
        (5, "日本語x"),
        (6, "cafe\u{301} ok"),
        (3, "abcdefg"),
    ] {
        let kept = BlockRows::of_line(columns.into(), text, &palette);
        let drawn = term(TerminalSize::new(columns, 6), text);
        let harvested = harvest(&drawn, &palette, 0, kept.rows() as i32 - 1);

        assert_eq!(
            cells(&kept),
            cells(&harvested),
            "{text:?} in {columns} columns"
        );
        let marks = |rows: &BlockRows| {
            let mut scratch = Vec::new();
            (0..rows.rows())
                .map(|row| rows.materialise(row, &mut scratch).to_vec())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            marks(&kept),
            marks(&harvested),
            "{text:?} in {columns} columns"
        );
    }
}

#[test]
fn a_harvest_taken_in_pieces_is_the_harvest_taken_whole() {
    // A grid's scrollback is read a few hundred rows at a time, so that the
    // reader is not kept off the lock for the whole of it. Put back together,
    // the pieces have to be the very store one harvest makes: the text, the
    // runs that colour it and the accents stacked on it, each row where it was.
    let size = TerminalSize::new(12, 6);
    let palette = Palette::default();
    let term = term(
        size,
        "plain\r\n\x1b[31mred\x1b[0m and not\r\n\u{65e5}\u{672c}x\r\ne\u{301}f\r\n\x1b[42mgreen\x1b[K\x1b[0m\r\nlast",
    );
    let whole = harvest(&term, &palette, 0, 5);

    for cut in 1..5 {
        let mut pieces = harvest(&term, &palette, 0, cut - 1);
        assert!(pieces.append(harvest(&term, &palette, cut, 5)));
        assert_eq!(whole.rows(), pieces.rows(), "cut before row {cut}");
        assert_eq!(cells(&whole), cells(&pieces), "cut before row {cut}");
        assert_eq!(whole.to_text(), pieces.to_text(), "cut before row {cut}");
        for row in 0..whole.rows() {
            assert_eq!(
                whole.zerowidth(row, 0),
                pieces.zerowidth(row, 0),
                "row {row}, cut before row {cut}"
            );
        }
    }

    // An empty store takes the first piece as it is, and an empty piece adds
    // nothing.
    let mut started = BlockRows::default();
    assert!(started.append(harvest(&term, &palette, 0, 5)));
    assert!(started.append(BlockRows::default()));
    assert_eq!(cells(&whole), cells(&started));
}

#[test]
fn a_piece_of_another_width_is_refused() {
    // A resize between two pieces: the second's runs cover another number of
    // columns, and a store that took it would paint those rows misaligned.
    let palette = Palette::default();
    let mut narrow = harvest(&term(TerminalSize::new(10, 2), "one"), &palette, 0, 0);
    let wide = harvest(&term(TerminalSize::new(20, 2), "two"), &palette, 0, 0);

    assert!(!narrow.append(wide));
    assert_eq!(1, narrow.rows(), "the refused piece was left out");
    assert_eq!("one", narrow.text(0));
}

/// What `bytes` leave on a screen `columns` wide, and tall enough for all of
/// it, once it has been made `resized` wide: harvested down to the last row
/// anything was printed on.
fn laid_out(columns: u16, resized: u16, bytes: &str) -> BlockRows {
    let mut term = term(TerminalSize::new(columns, 40), bytes);
    term.resize(TerminalSize::new(resized, 40));
    // A reflow that makes more rows of them pushes the first into history.
    let history = term.grid().history_size() as i32;
    let rows = harvest(&term, &Palette::default(), -history, 39);
    let printed = (0..rows.rows())
        .rposition(|row| !rows.text(row).is_empty())
        .map_or(0, |row| row + 1);
    rows.slice(0..printed)
}

/// A cell nothing was printed into, in the default palette's colours.
fn blank() -> SnapshotCell {
    let palette = Palette::default();
    SnapshotCell {
        c: ' ',
        foreground: palette.foreground,
        background: palette.background,
        flags: CellFlags::NONE,
    }
}

/// Lines a column change has something to do with: one folded over rows,
/// a double-width character the edge falls inside, a combining accent,
/// a coloured erase to the end of the row, a tab, and blank lines.
const TEXT: &str = "short\r\n\
    a line long enough to fold over several rows of a narrow grid\r\n\
    \x1b[44m\x1b[Kblue to the edge\x1b[0m\r\n\
    \r\n\
    日本語の文章が折り返される e\u{301}\u{301} done\r\n\
    tab\there\r\n";

#[test]
fn rows_laid_out_again_at_a_width_are_the_rows_the_grid_reflows_them_into() {
    // Against the grid's own reflow rather than against the text printed
    // at the new width: the two differ where a coloured erase reached the
    // old edge, which the reflow carries and a fresh print never made.
    for (from, to) in [(20, 13), (13, 31), (31, 7), (7, 20), (20, 20)] {
        let rewrapped = laid_out(from, from, TEXT).rewrapped(usize::from(to), blank());
        let grid = laid_out(from, to, TEXT);
        assert_eq!(
            grid.to_text(),
            rewrapped.to_text(),
            "from {from} columns to {to}"
        );
        assert_eq!(grid, rewrapped, "the cells, from {from} columns to {to}");
    }
}

#[test]
fn a_line_whose_rows_go_on_past_the_store_ends_where_the_store_does() {
    // The head of a folded line, the way the grid's oldest rows cut off it
    // leave it: its last row still says the line goes on.
    let head = laid_out(10, 10, "0123456789abcdefghijklmn").slice(0..2);
    assert!(
        head.wraps(1),
        "the premise: the head is folded into its rest"
    );
    let rewrapped = head.rewrapped(7, blank());
    assert_eq!("0123456\n789abcd\nefghij", rewrapped.to_text());
    assert!(
        !rewrapped.wraps(rewrapped.rows() - 1),
        "a fold into rows that are not there"
    );
}

#[test]
fn a_slice_and_a_dropped_front_keep_the_rows_they_name() {
    let rows = laid_out(20, 20, TEXT);
    let all = rows.rows();
    assert_eq!(rows, {
        let mut joined = rows.slice(0..3);
        assert!(joined.append(rows.slice(3..all)));
        joined
    });
    let mut dropped = rows.clone();
    dropped.drop_front(2);
    assert_eq!(rows.slice(2..all), dropped);
    assert_eq!(rows.text(4), dropped.text(2));
    assert_eq!(rows.zerowidth(6, 15), dropped.zerowidth(4, 15));
    dropped.drop_front(all);
    assert!(dropped.is_empty());
    assert_eq!(20, dropped.columns(), "the width stays");
    let (from, to) = (5, 2);
    assert!(
        rows.slice(from..to).is_empty(),
        "a range that ends before it starts"
    );
    assert_eq!(rows.slice(all - 1..all + 10), rows.slice(all - 1..all));
}

#[test]
fn rows_laid_out_from_their_runs_are_the_rows_laid_out_a_cell_at_a_time() {
    // Coloured runs across folds, a fold whose row ends in spaces, a
    // coloured erase past the text, inverse and bold runs, non-ASCII
    // single-width text, and lines with nothing in them, at widths that cut
    // inside runs and between them.
    let text = "\x1b[31mred\x1b[0m and \x1b[1;44mbold on blue\x1b[0m and plain text going on\r\n\
        ends in spaces          \r\n\
        \x1b[7minverse\x1b[0m \x1b[42m\x1b[Kgreen tail\x1b[0m\r\n\
        ─── box ─── drawing ─── café ── naïve ─────────────────────\r\n\
        \r\n\
        \r\n\
        last";
    for from in [7u16, 13, 20, 31] {
        let rows = laid_out(from, from, text);
        for to in [1, 2, 5, 7, 12, 13, 19, 20, 33, 80] {
            assert_eq!(
                rows.rewrapped_by_cells(to, blank()),
                rows.rewrapped(to, blank()),
                "from {from} columns to {to}"
            );
        }
        // And a store that ends inside a folded line.
        let head = rows.slice(0..2);
        assert_eq!(
            head.rewrapped_by_cells(9, blank()),
            head.rewrapped(9, blank()),
            "the head of a line, from {from} columns"
        );
    }
}
