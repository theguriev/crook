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
        snapshot::build(&term, &Palette::default(), 0, None, LiveBlock::default())
            .text()
            .trim_end()
            .to_owned(),
        rows.to_text(),
        "a copied block and a copied selection disagree"
    );
}
