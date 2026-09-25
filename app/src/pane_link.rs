//! The link under the pointer in one pane's output.
//!
//! Kept by the workspace, per pane, for exactly the reason
//! [`PaneSelection`](crate::pane_selection::PaneSelection) is: the element tree
//! is thrown away and rebuilt on every frame, and this is a fact about where
//! the pointer is that has to survive between two of them. The move that finds
//! a link and the frame that underlines it are different frames.
//!
//! # Why a link is only live while a modifier is held
//!
//! Because the pointer is already spoken for. Dragging across the output
//! selects it, and a terminal where a click on a URL opened a browser instead
//! of placing a selection would be a terminal you cannot copy a URL out of.
//! Every terminal resolves this the same way: hold the platform's own chord
//! key and links light up; let go and the pointer goes back to selecting. So
//! this is set only while that key is down, and cleared the moment it comes up
//! — which is what makes the underline appear and disappear under a pointer
//! that never moved.
//!
//! # A folded line is one link
//!
//! [`find`] is the scan itself, and both surfaces go through it. The URL a
//! build tool prints is routinely longer than the pane is wide, and the
//! terminal folds it onto the next row without the program ever printing a
//! newline — the emulator marks the fold with [`CellFlags::WRAPLINE`], and a
//! copy already reads that flag to hand back the one line the shell printed.
//! A scan of a single row would find the link only as far as the fold, and
//! open a URL cut off at the pane's edge. So the row under the pointer is
//! joined with the rows the fold connects it to, above and below, and the span
//! that comes back is mapped onto every row it crosses. A hard line break —
//! two URLs printed on adjacent rows — carries no flag and is not joined.
//!
//! [`CellFlags::WRAPLINE`]: crook_terminal::CellFlags::WRAPLINE

use std::cell::RefCell;
use std::rc::Rc;

use crook_terminal::Rows;
use crook_terminal::url;
use unicode_width::UnicodeWidthChar;

/// A link under the pointer: the URL, and the cells it covers.
///
/// One range of cells per row, top row first, because a line the terminal
/// folded puts one link on two rows — and the underline and the click have to
/// agree that both of them are it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    /// The cells it covers, one range per row it crosses, in row order. Never
    /// empty.
    pub cells: Vec<LinkCells>,
    /// The URL itself, which is what a click opens.
    pub uri: String,
}

/// The cells of one row a link occupies, in the coordinates of the surface
/// that found it.
///
/// The two surfaces number their rows differently — a grid row is a row of the
/// viewport, a block row is a row of one finished command — so the row is
/// stored beside a tag saying which it is. Without it, scrolling the list would
/// leave an underline on whichever row happened to inherit the number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkCells {
    /// Which surface, and which of its rows.
    pub row: LinkRow,
    /// The first cell of the row the link occupies.
    pub start: usize,
    /// How many cells it covers.
    pub len: usize,
}

/// Which row of which surface a link was found on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinkRow {
    /// A row of the live viewport, as the grid draws it.
    Viewport(usize),
    /// A row of one finished block: its index in the list, and the row within
    /// it.
    Block {
        /// Which block, by its position in the pane's history.
        index: usize,
        /// Which of that block's rows.
        row: usize,
    },
}

/// The link under the pointer in one pane, if there is one.
///
/// Cheap to clone — it is an [`Rc`] — because the elements that draw a pane's
/// output take one every frame.
#[derive(Clone, Default)]
pub struct PaneLink(Rc<RefCell<Option<LinkSpan>>>);

impl PaneLink {
    /// A pane with no link under the pointer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The link under the pointer, if there is one.
    pub fn get(&self) -> Option<LinkSpan> {
        self.0.borrow().clone()
    }

    /// The cells of the link under the pointer that lie on a given row, if
    /// any do.
    ///
    /// What the paint path asks: it is drawing one row and wants to know
    /// whether any of its cells are underlined. A link on a folded line
    /// answers for each row it crosses.
    pub fn on(&self, row: LinkRow) -> Option<LinkCells> {
        self.0
            .borrow()
            .as_ref()
            .and_then(|link| link.cells.iter().find(|cells| cells.row == row))
            .cloned()
    }

    /// The URL a click would open.
    pub fn uri(&self) -> Option<String> {
        self.0.borrow().as_ref().map(|link| link.uri.clone())
    }

    /// Puts a link under the pointer, reporting whether the frame changed.
    ///
    /// `None` clears it. The answer is what decides whether a pointer move is
    /// worth a repaint — a pointer travelling along one link produces a move
    /// per pixel and exactly one frame.
    pub fn set(&self, link: Option<LinkSpan>) -> bool {
        let mut held = self.0.borrow_mut();
        if *held == link {
            return false;
        }
        *held = link;
        true
    }

    /// Whether there is a link under the pointer at all.
    pub fn is_some(&self) -> bool {
        self.0.borrow().is_some()
    }

    /// The cells of the link that are on viewport rows this grid still draws,
    /// with the row each range is on.
    ///
    /// The bound matters: a grid that shrank between the move that found the
    /// link and the frame that draws it would otherwise underline a row
    /// outside its own box.
    pub fn on_viewport_rows(&self, rows: usize) -> Vec<(usize, LinkCells)> {
        self.0
            .borrow()
            .as_ref()
            .map(|link| {
                link.cells
                    .iter()
                    .filter_map(|cells| match cells.row {
                        LinkRow::Viewport(row) if row < rows => Some((row, cells.clone())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The link under a cell of a block's rows, or `None` when the cell is not
/// part of one.
///
/// `row` and `column` are the cell under the pointer, in the block's own
/// numbering; `place` names each row of the block on the surface the caller
/// draws, which is what the cells that come back are tagged with.
///
/// The rows the terminal folded around `row` are scanned as the one line they
/// are: every row contributes exactly its width, one `char` per cell, so a
/// cell of the joined text is a row and a column by arithmetic alone. Nothing
/// is trimmed on the way in — a blank cell ends a URL exactly as it does on
/// one row, and a line that folded right after a space stays two words rather
/// than one. The price is a double-width character that would not fit at the
/// margin: the blank it leaves there ends the link, so a URL with such a
/// character exactly at the fold is found only up to it.
pub fn find(
    rows: &Rows<'_>,
    row: usize,
    column: usize,
    place: impl Fn(usize) -> LinkRow,
) -> Option<LinkSpan> {
    let columns = rows.columns();
    if row >= rows.count() || column >= columns {
        return None;
    }
    let mut first = row;
    while first > 0 && rows.wraps(first - 1) {
        first -= 1;
    }
    // Bounded by the block: `wraps` is never true of its last row.
    let mut last = row;
    while rows.wraps(last) {
        last += 1;
    }

    let mut text = String::with_capacity((last - first + 1) * columns);
    for folded in first..=last {
        let mut cells = vec![' '; columns];
        // The marks stacked on a cell arrive after it, at its column, and the
        // scan counts cells: the cell's own character is the one that stands
        // for it. A wide character's spacer column is visited by nothing, and
        // is given a stand-in that continues a URL — left blank, as it is on
        // the grid, it ended one, and `https://ja.wikipedia.org/wiki/東京`
        // was a link to `…/wiki/東`.
        let mut filled = None;
        rows.visit_line(folded, columns, |character, at| {
            if filled != Some(at) {
                cells[at] = character;
                filled = Some(at);
                if character.width() == Some(2)
                    && let Some(spacer) = cells.get_mut(at + 1)
                {
                    *spacer = SPACER;
                }
            }
        });
        text.extend(cells);
    }

    let url = url::at(&text, (row - first) * columns + column)?;
    let cells = (first..=last)
        .filter_map(|folded| {
            let row_start = (folded - first) * columns;
            let start = url.start.max(row_start);
            let end = url.end().min(row_start + columns);
            (start < end).then(|| LinkCells {
                row: place(folded),
                start: start - row_start,
                len: end - start,
            })
        })
        .collect();
    Some(LinkSpan {
        cells,
        uri: url.uri.replace(SPACER, ""),
    })
}

/// What a wide character's spacer column reads as while a line is scanned
/// for a URL: a character from the private use area, which nothing prints
/// and nothing ends a URL on. Taken out of the address before it is handed
/// back; the cells it stood for stay, so the underline covers the whole
/// character.
const SPACER: char = '\u{e000}';

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crook_terminal::{Emulator, Palette, Snapshot, TerminalSize};

    use super::*;

    fn span(row: LinkRow) -> LinkSpan {
        LinkSpan {
            cells: vec![LinkCells {
                row,
                start: 4,
                len: 19,
            }],
            uri: "https://example.com".to_owned(),
        }
    }

    /// A grid twenty columns wide holding whatever `bytes` drew — narrow
    /// enough that one URL folds, so the fold is the emulator's own rather
    /// than a flag a test set by hand.
    fn grid(bytes: &str) -> Arc<Snapshot> {
        let mut emulator = Emulator::new(TerminalSize::new(20, 6), 100, Palette::default());
        emulator.advance(bytes.as_bytes());
        emulator.snapshot()
    }

    /// The whole grid as the rows of one block.
    fn rows(snapshot: &Snapshot) -> Rows<'_> {
        Rows::Live {
            snapshot,
            top: 0,
            count: snapshot.rows,
        }
    }

    /// The link under a cell of the grid, tagged with viewport rows.
    fn link_at(snapshot: &Snapshot, row: usize, column: usize) -> Option<LinkSpan> {
        find(&rows(snapshot), row, column, LinkRow::Viewport)
    }

    fn cells(row: usize, start: usize, len: usize) -> LinkCells {
        LinkCells {
            row: LinkRow::Viewport(row),
            start,
            len,
        }
    }

    #[test]
    fn a_url_with_wide_characters_in_it_is_one_link() {
        // Each of these is two columns, and the column after it is a spacer
        // the scan used to read as a blank — the end of the link, one
        // character into the path.
        let snapshot = grid("https://x.jp/\u{6771}\u{4eac}");
        let link = link_at(&snapshot, 0, 2).expect("the row is a link");
        assert_eq!(link.uri, "https://x.jp/\u{6771}\u{4eac}");
        assert_eq!(link.cells, vec![cells(0, 0, 17)]);

        // From the second half of a wide character as well as the first.
        let link = link_at(&snapshot, 0, 16).expect("the spacer is part of it");
        assert_eq!(link.uri, "https://x.jp/\u{6771}\u{4eac}");
    }

    #[test]
    fn a_url_the_terminal_folded_is_one_link_from_either_row() {
        // Thirty-six characters into twenty columns: the emulator folds it
        // after the slash, and nothing was printed at the fold.
        let snapshot = grid("https://example.com/abcdefghijklmnop");
        assert!(rows(&snapshot).wraps(0), "the emulator folded it");
        let whole = LinkSpan {
            cells: vec![cells(0, 0, 20), cells(1, 0, 16)],
            uri: "https://example.com/abcdefghijklmnop".to_owned(),
        };

        assert_eq!(
            link_at(&snapshot, 0, 5),
            Some(whole.clone()),
            "from the top row"
        );
        assert_eq!(
            link_at(&snapshot, 1, 3),
            Some(whole),
            "from the row it folded onto"
        );
        assert_eq!(
            link_at(&snapshot, 1, 16),
            None,
            "the blank after it is not part of it"
        );
    }

    #[test]
    fn two_urls_on_adjacent_rows_stay_two_links() {
        // The first fills its row exactly, which is the case that looks most
        // like a fold — but the newline was printed, so there is no flag and
        // no join.
        let snapshot = grid("https://a.test/aaaaa\r\nhttps://b.test");
        assert!(!rows(&snapshot).wraps(0), "a printed newline is not a fold");

        let top = link_at(&snapshot, 0, 2).expect("the top row's link");
        assert_eq!(top.uri, "https://a.test/aaaaa");
        assert_eq!(top.cells, vec![cells(0, 0, 20)]);

        let below = link_at(&snapshot, 1, 2).expect("the second row's link");
        assert_eq!(below.uri, "https://b.test");
        assert_eq!(below.cells, vec![cells(1, 0, 14)]);
    }

    #[test]
    fn a_line_folded_right_after_a_space_does_not_run_the_link_into_the_next_row() {
        // The space is the twentieth cell, and the word after it folds. A copy
        // trims that space away and runs the two together; a link must not,
        // because the URL ended where the space was.
        let snapshot = grid("https://example.com next");
        assert!(rows(&snapshot).wraps(0), "the emulator folded it");

        let link = link_at(&snapshot, 0, 4).expect("the link on the top row");
        assert_eq!(link.uri, "https://example.com");
        assert_eq!(link.cells, vec![cells(0, 0, 19)]);
        assert_eq!(link_at(&snapshot, 1, 1), None, "the word after the fold");
    }

    #[test]
    fn a_folded_link_after_wide_characters_lands_on_the_cells_it_was_drawn_in() {
        // Two double-width characters take four columns, so the link starts
        // at the sixth cell and folds fifteen characters in. Counting
        // characters instead of cells would put the underline two cells
        // left of the text on the top row and split the fold in the wrong
        // place.
        let snapshot = grid("漢字 https://example.com/abcdefg");
        assert!(rows(&snapshot).wraps(0), "the emulator folded it");
        let whole = LinkSpan {
            cells: vec![cells(0, 5, 15), cells(1, 0, 12)],
            uri: "https://example.com/abcdefg".to_owned(),
        };

        assert_eq!(link_at(&snapshot, 0, 7), Some(whole.clone()));
        assert_eq!(link_at(&snapshot, 1, 2), Some(whole));
        assert_eq!(
            link_at(&snapshot, 0, 1),
            None,
            "the wide character before it"
        );
    }

    #[test]
    fn a_link_on_a_folded_line_is_offered_to_every_row_it_crosses() {
        let link = PaneLink::new();
        link.set(Some(LinkSpan {
            cells: vec![cells(2, 3, 17), cells(3, 0, 8)],
            uri: "https://example.com/abcde".to_owned(),
        }));

        assert_eq!(link.on(LinkRow::Viewport(2)), Some(cells(2, 3, 17)));
        assert_eq!(link.on(LinkRow::Viewport(3)), Some(cells(3, 0, 8)));
        assert_eq!(link.on(LinkRow::Viewport(4)), None);
        // A grid that shrank to three rows between the move and the frame
        // draws the top half and not the row it no longer has.
        assert_eq!(link.on_viewport_rows(3), vec![(2, cells(2, 3, 17))]);
    }

    #[test]
    fn a_link_is_only_offered_to_the_row_it_was_found_on() {
        // The two surfaces number their rows differently. Without the tag, a
        // list scrolled by one row would underline whichever row inherited the
        // number.
        let link = PaneLink::new();
        link.set(Some(span(LinkRow::Viewport(3))));

        assert!(link.on(LinkRow::Viewport(3)).is_some());
        assert!(link.on(LinkRow::Viewport(4)).is_none());
        assert!(
            link.on(LinkRow::Block { index: 0, row: 3 }).is_none(),
            "row 3 of a block is not row 3 of the viewport"
        );
    }

    #[test]
    fn setting_the_same_link_again_is_not_a_repaint() {
        // A pointer travelling along one link produces a move per pixel. Every
        // one of them finds the same link, and exactly one of them is worth a
        // frame.
        let link = PaneLink::new();

        assert!(link.set(Some(span(LinkRow::Viewport(1)))));
        assert!(!link.set(Some(span(LinkRow::Viewport(1)))));
        assert!(link.set(Some(span(LinkRow::Viewport(2)))));
        assert!(link.set(None));
        assert!(!link.set(None));
    }

    #[test]
    fn the_element_that_is_handed_a_clone_is_looking_at_the_same_link() {
        let kept = PaneLink::new();
        let handed_to_the_element = kept.clone();

        handed_to_the_element.set(Some(span(LinkRow::Viewport(0))));

        assert!(kept.is_some());
        assert_eq!(kept.uri().as_deref(), Some("https://example.com"));
    }
}
