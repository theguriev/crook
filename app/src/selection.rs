//! Selecting text out of a pane's output: one selection, across every block in
//! it.
//!
//! # The address space
//!
//! A pane's output is a list of blocks, and only the last of them still has
//! its cells in the emulator — everything above it was harvested out when its
//! command ended. So an anchor is **not** a cell of the grid. It is a block, a
//! row of that block, a column, and which side of the cell the pointer is on.
//!
//! That is what makes a selection hold still. A block's id is handed out once
//! and never reused, and its rows never move within it: output arriving below
//! adds a block rather than shifting one, the emulator scrolling changes which
//! viewport row the open block's row seven is drawn on but not that it is row
//! seven, and a command finishing copies its rows into the store in the order
//! they were already in. A selection made before any of those is the same
//! selection after all of them.
//!
//! **The open block is not a special case.** It is the last item of the list
//! with its rows read out of the [`Snapshot`] instead of out of a store, which
//! is the one difference and the one [`Rows`] exists to absorb. There is no
//! second code path for it, because two implementations of a selection is
//! exactly how a terminal comes to highlight one thing and copy another.
//!
//! # The grid is a list of one block
//!
//! A pane that draws one grid rather than a list — the alternate screen, a
//! block that has outgrown the viewport — has no finished blocks to address.
//! It is still the same address space: one item, whose rows are numbered from
//! the oldest line the scrollback holds so that they do not move under a
//! viewport that scrolls, and whose cells are read back through
//! [`Terminal::harvest_rows`](crook_terminal::Terminal::harvest_rows) into the
//! very store a finished block uses. Same anchors, same region, same copy.
//!
//! # The two numberings do not mix
//!
//! A block's rows are numbered from its own first row and a grid's from the
//! scrollback, so the *same* pair of numbers names different characters on the
//! two surfaces. Which one an anchor was minted in is therefore part of it —
//! [`Cells`] — and a selection is only ever resolved against the blocks of the
//! space it was made in. A pane that changes surface under a selection lets go
//! of it, exactly as one that is resized under it does: the picture the
//! highlight was drawn on is gone, and re-reading the anchors against the new
//! one is how a terminal comes to copy text nobody selected. See
//! [`PaneSelection::resurfaced`](crate::pane_selection::PaneSelection::resurfaced).
//!
//! # What a word is
//!
//! The emulator is no longer answering that, so the rule is stated here and it
//! is the composer's: a word is one [UAX #29] word-bound segment — a run of
//! letters and digits, a run of spaces, or a run of punctuation — taken from
//! [`crate::editor::text::word_range_at`], the same function the field below
//! the output uses for its own double click. One rule in one place, so that
//! double-clicking a path in the output and double-clicking it in the line
//! being composed select the same thing.
//!
//! A word does cross the fold in a line too long for the pane, because that
//! fold is a place the terminal put the text rather than something the shell
//! printed. It never crosses a block, because that is a different command.
//!
//! # A find is a line at a time
//!
//! What a find bar looks for is matched against one logical line at a time —
//! the rows a fold joins, with the blanks the fold trimmed put back — and never
//! against the output as one string. So a pattern's `^` and `$` are the start
//! and end of a line, where grep puts them, and a fold is neither; a match
//! never runs out of one line into the next, let alone into the next command;
//! and what a block matches depends on nothing outside it, which is what lets
//! the bar keep a finished block's matches rather than search the whole
//! history again every frame. See [`Item::find_with`].
//!
//! [UAX #29]: https://unicode.org/reports/tr29/

use std::ops::Range;

use crook_terminal::{Block, BlockId, CellFlags, CellSide, Rows, SelectionKind, Snapshot};

use crate::editor::text;
use crate::terminal_model::BlockHistory;

/// Which of the two address spaces an anchor's numbers are in.
///
/// The one thing the two surfaces do not share, and it is not a difference in
/// what a selection *is*: the same anchors, the same region and the same walk
/// either way. It is which list of blocks they mean — and because a row number
/// means a different row in each, it is carried with the selection rather than
/// guessed from whatever the pane happens to be drawing when a copy is asked
/// for.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Cells {
    /// The list's blocks: the commands that have finished, then the open one.
    /// Rows are numbered from each block's own first row.
    #[default]
    List,
    /// One grid, which addresses as a list of a single block — and whose rows
    /// reach back into a scrollback the snapshot does not hold, so they are
    /// numbered from the oldest line of the history and read out of the
    /// emulator into the very store a finished block uses.
    Grid,
}

/// One end of a selection.
///
/// Which block, which of that block's rows, which column, and which of the two
/// gaps beside that column the pointer is in — because a selection ends
/// *between* two cells, and that is what makes dragging left from the middle
/// of a character take it and dragging right from the same place leave it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// The block it is in.
    pub block: BlockId,
    /// The row of that block, counting from its first.
    pub row: usize,
    /// The column of that row.
    pub column: usize,
    /// Which side of the column.
    pub side: CellSide,
}

impl Anchor {
    /// An end of a selection at one cell.
    pub const fn new(block: BlockId, row: usize, column: usize, side: CellSide) -> Self {
        Self {
            block,
            row,
            column,
            side,
        }
    }

    /// What orders two ends of a gesture: reading order, with the gap before a
    /// cell coming before the gap after it.
    fn order(self) -> (BlockId, usize, usize, CellSide) {
        (self.block, self.row, self.column, self.side)
    }
}

/// A selection in a pane's output, as the gesture that made it left it.
///
/// Kept as the two ends and the kind rather than as a resolved region, for the
/// same reason the emulator kept a `Selection` rather than a range: what a
/// double click covers depends on the text under it, and the text under it
/// changes. Resolving happens on demand, in [`Self::region`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// What the gesture is taking at a time.
    pub kind: SelectionKind,
    /// The end that stays where the press landed.
    pub anchor: Anchor,
    /// The end that follows the pointer.
    pub head: Anchor,
}

impl Selection {
    /// A selection from the press at `anchor` to wherever the pointer is now.
    pub const fn new(kind: SelectionKind, anchor: Anchor, head: Anchor) -> Self {
        Self { kind, anchor, head }
    }

    /// The cells it covers, or `None` when it covers none — a press nobody
    /// dragged anywhere, or a drag that came back to where it started.
    ///
    /// That is what makes a plain click on the output *clear* the last
    /// selection rather than leave a one-cell highlight behind.
    pub fn region(&self, blocks: &Blocks<'_>) -> Option<Region> {
        let (from, to) = if self.anchor.order() <= self.head.order() {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };

        match self.kind {
            SelectionKind::Simple => simple(from, to, blocks),
            SelectionKind::Block => rectangle(from, to),
            SelectionKind::Semantic => grown(from, to, blocks, word_at),
            SelectionKind::Lines => grown(from, to, blocks, line_at),
        }
    }

    /// What a copy of it would put on the clipboard, or `None` when it covers
    /// no cells.
    pub fn text(&self, blocks: &Blocks<'_>) -> Option<String> {
        self.text_with(blocks, None)
    }

    /// [`Self::text`] of a list one block of which is shown filtered: the
    /// rows the filter hides are left out, and a selection every row of which
    /// it hides is `None`, as one that covers no cells is. See
    /// [`Region::text_with`].
    pub fn text_with(&self, blocks: &Blocks<'_>, filter: Option<Filter<'_>>) -> Option<String> {
        self.region(blocks)?.text_with(blocks, filter)
    }
}

/// One block shown filtered: which block, and which of its rows are drawn, in
/// the order they are drawn.
///
/// What a copy has to be told and a region cannot work out. A drag over a
/// filtered block is made against the block's own rows — the third line on
/// screen is whatever row `kept[2]` names — so the rows the filter hides
/// between the two ends are inside the region, as rows of the block always
/// are. They are not painted, and a copy that took them would hand over lines
/// nobody saw.
#[derive(Copy, Clone, Debug)]
pub struct Filter<'a> {
    /// The block that is filtered.
    pub block: BlockId,
    /// Its rows that are shown, in the numbering anchors use, as the filter
    /// gave them.
    pub kept: &'a [usize],
}

impl<'a> Filter<'a> {
    /// The rows of `id` that are shown, or `None` when `id` is not the block
    /// this filters and all of its rows are.
    fn kept_of(&self, id: BlockId) -> Option<&'a [usize]> {
        (self.block == id).then_some(self.kept)
    }
}

/// One cell of the address space: a block, one of its rows, and a column.
///
/// Ordered the way the list is drawn, which is why the whole of "is this row
/// inside the selection" is one comparison of two of these. Block ids are
/// handed out in order and never reused, so ordering by id *is* ordering by
/// position in the list, and the open block — always the newest — always
/// sorts last.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Place {
    /// The block it is in.
    pub block: BlockId,
    /// The row of that block.
    pub row: usize,
    /// The column of that row.
    pub column: usize,
}

impl Place {
    /// A cell of the address space.
    pub const fn new(block: BlockId, row: usize, column: usize) -> Self {
        Self { block, row, column }
    }

    /// Which row it is on, for the comparisons that do not care about columns.
    fn line(self) -> (BlockId, usize) {
        (self.block, self.row)
    }
}

/// The cells a selection covers, resolved against the blocks it crosses.
///
/// Inclusive at both ends, in reading order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Region {
    /// The first selected cell.
    pub start: Place,
    /// The last selected cell.
    pub end: Place,
    /// Whether it is a rectangle rather than a run of text. A rectangle takes
    /// the same columns out of every row it covers; everything else runs to
    /// the end of a row and continues on the next.
    pub block: bool,
}

impl Region {
    /// Which columns of one row are inside it, or `None` when that row is not.
    ///
    /// The one answer both the highlight and the copy are drawn from, which is
    /// what stops them disagreeing about what is selected. Grown to whole
    /// characters, so a double-width glyph is never cut down the middle by
    /// either of them.
    ///
    /// `row` is in the numbering the anchors use, and the item is what says
    /// what that numbering is — which is why the item is the argument rather
    /// than its rows. A store is indexed from its own first row, and handing
    /// it a grid's row number is how a highlight comes to be grown against one
    /// line and painted over another.
    pub fn columns_on(&self, item: &Item<'_>, row: usize) -> Option<Range<usize>> {
        let line = (item.id, row);
        if line < self.start.line() || line > self.end.line() {
            return None;
        }
        let local = item.local(row)?;
        let last = item.rows.columns().checked_sub(1)?;

        let (first, end) = if self.block {
            (self.start.column, self.end.column)
        } else {
            (
                if line == self.start.line() {
                    self.start.column
                } else {
                    0
                },
                if line == self.end.line() {
                    self.end.column
                } else {
                    last
                },
            )
        };
        let (first, end) = (first.min(last), end.min(last));
        (first <= end).then(|| item.rows.whole_characters(local, first..end + 1))
    }

    /// The text it covers, or `None` when there are no blocks to read it out
    /// of at all.
    ///
    /// **What survives is what is copied.** A block dropped off the front of
    /// the list — ten thousand commands ago — takes its own rows with it and
    /// nothing else: the blocks after it are still selected, still painted as
    /// selected, and still copied. A copy that gave up because the block a
    /// drag *started* in had been evicted would hand back nothing while the
    /// screen showed a highlight over three blocks that are still there.
    ///
    /// **The blocks between the two ends are taken whole and the two ends
    /// partially, and the pieces are joined with a single newline.** The
    /// padding, the dividers and the copy controls between two blocks
    /// contribute nothing: you get the text you saw, without the gutter.
    ///
    /// Two rules inside a block, and both of them are about what a line
    /// actually is. A row too long for the pane was folded onto the row below
    /// by the terminal, not broken by the shell, so the two come back as the
    /// one line they are. And the blanks at the end of a row are the rest of a
    /// grid rather than anything anybody typed, so they stay behind.
    pub fn text(&self, blocks: &Blocks<'_>) -> Option<String> {
        self.text_with(blocks, None)
    }

    /// [`Self::text`], leaving out the rows of the filtered block that the
    /// filter does not show.
    ///
    /// The rows it does show are walked in the order it shows them, by the
    /// same two rules. A fold still joins a row to the one it folded onto when
    /// the filter shows both, which is a long line kept whole; when the row it
    /// folded onto is hidden, the next row shown is another line, and it gets
    /// a line break of its own rather than being run on to a half line.
    ///
    /// `None` when a filter is given and the region holds none of the rows
    /// it shows, as for a selection that covers no cells. The region is
    /// worked out on the block's own rows, so it outlives the rows it was
    /// drawn over: narrow the query after a drag, or let go of a drag in the
    /// rows hidden between two shown ones, and the selection still holds with
    /// no highlight drawn. An empty string for it would go onto the clipboard
    /// over whatever the person last copied, for a highlight they could not
    /// see.
    pub fn text_with(&self, blocks: &Blocks<'_>, filter: Option<Filter<'_>>) -> Option<String> {
        let last = blocks.len().checked_sub(1)?;
        // An end whose block is gone clamps to the surviving list rather than
        // giving up: everything before the first block left is what was
        // evicted, and `columns_on` decides row by row anyway.
        let (first, last) = (
            blocks.index_of(self.start.block).unwrap_or(0),
            blocks.index_of(self.end.block).unwrap_or(last),
        );

        let mut text = String::new();
        let mut written = false;
        let mut folded = false;
        for index in first..=last {
            let Some(item) = blocks.item(index) else {
                continue;
            };
            // The rows the list draws of this item, in the order it draws
            // them: every one of them, or the ones the filter keeps.
            let kept = filter.and_then(|filter| filter.kept_of(item.id));
            let shown = kept.map_or(item.rows.count(), <[usize]>::len);
            for place in 0..shown {
                let row = kept.map_or(item.first + place, |kept| kept[place]);
                let Some(local) = item.local(row) else {
                    continue;
                };
                let Some(columns) = self.columns_on(&item, row) else {
                    continue;
                };
                if written && !folded {
                    text.push('\n');
                }
                let end = columns.end.min(item.rows.line_length(local));
                item.rows
                    .write(local, columns.start..end.max(columns.start), &mut text);
                // A fold runs on only into the row it folded onto, which a
                // filter may have hidden.
                let continued = kept.is_none_or(|kept| kept.get(place + 1) == Some(&(row + 1)));
                // A rectangle is columns cut out of the rows it crosses, so
                // every one of them is its own line however the terminal
                // folded them.
                folded = !self.block
                    && continued
                    && columns.end >= item.rows.columns()
                    && item.rows.wraps(local);
                if folded {
                    let blanks = blanks_before_a_fold(&item.rows, local)
                        .filter(|column| *column >= columns.start)
                        .count();
                    text.extend(std::iter::repeat_n(' ', blanks));
                }
                written = true;
            }
        }
        (written || filter.is_none()).then_some(text)
    }
}

/// One block of the list a selection addresses.
#[derive(Copy, Clone, Debug)]
pub struct Item<'a> {
    /// What it is called, for as long as the session lives.
    pub id: BlockId,
    /// Its cells, out of whichever store holds them.
    pub rows: Rows<'a>,
    /// The number its first row answers to.
    ///
    /// Zero for a block, because a block's rows are its own. Not zero for the
    /// grid, whose rows are numbered from the oldest line of the scrollback so
    /// that scrolling the viewport does not renumber them.
    pub first: usize,
}

impl Item<'_> {
    /// The rows it holds, in the numbering anchors use.
    fn range(&self) -> Range<usize> {
        self.first..self.first + self.rows.count()
    }

    /// Where an anchor's row is in the store, or `None` when this item does
    /// not hold that row.
    ///
    /// The one place the two numberings meet. A grid's window slides — over
    /// the scrollback as the wheel turns, and over its own rows as the pane is
    /// made shorter — so an anchor perfectly well formed a moment ago names a
    /// row outside it, and every reader has to be able to say so rather than
    /// subtract its way off the end of the array.
    fn local(&self, row: usize) -> Option<usize> {
        row.checked_sub(self.first)
            .filter(|local| *local < self.rows.count())
    }

    /// Every place `matcher` matches this item's text, as the cells each match
    /// covers, appended to `found` in reading order.
    ///
    /// One logical line at a time — see the module docs — so the regions it
    /// adds are in order and none overlaps another, which is what lets
    /// [`regions_on`] answer for one row with a binary search.
    pub fn find_with(&self, matcher: &LineMatcher, found: &mut Vec<Region>) {
        #[cfg(test)]
        if let Rows::Stored(rows) = self.rows {
            STORED_ROWS_SEARCHED.with(|read| read.set(read.get() + rows.rows()));
        }

        // One line as the matcher reads it, and for each of its characters
        // where in the line it starts and the cell it was drawn in: what takes
        // a match, which is bytes of the line, back to the cells it covers.
        // Kept from one line to the next, so that a block of fifty thousand
        // rows costs two buffers rather than a hundred thousand.
        let mut line = String::new();
        let mut cells: Vec<(usize, usize, usize)> = Vec::new();
        let count = self.rows.count();
        let mut local = 0;
        while local < count {
            line.clear();
            cells.clear();
            loop {
                let row = self.first + local;
                self.rows
                    .visit_line(local, self.rows.line_length(local), |character, column| {
                        cells.push((line.len(), row, column));
                        line.push(matcher.searched(character));
                    });
                for column in blanks_before_a_fold(&self.rows, local) {
                    cells.push((line.len(), row, column));
                    line.push(' ');
                }
                let folds = self.rows.wraps(local);
                local += 1;
                if !folds {
                    break;
                }
            }

            matcher.each_in(&line, |start, end| {
                // Both ends of a match are on character boundaries, so the
                // first character is the one starting at `start` and the last
                // is the one before `end`.
                let first = cells.partition_point(|cell| cell.0 < start);
                let last = cells.partition_point(|cell| cell.0 < end).checked_sub(1);
                if let (Some(first), Some(last)) =
                    (cells.get(first), last.and_then(|last| cells.get(last)))
                {
                    found.push(Region {
                        start: Place::new(self.id, first.1, first.2),
                        end: Place::new(self.id, last.1, last.2),
                        block: false,
                    });
                }
            });
        }
    }
}

/// Every block a selection can address, in the order a pane draws them.
///
/// One list whichever surface the pane is showing: a grid is a list of exactly
/// one block.
#[derive(Copy, Clone, Debug)]
pub enum Blocks<'a> {
    /// The commands that have finished, then the one still open.
    List {
        /// The finished blocks, oldest first.
        finished: &'a BlockHistory,
        /// The open block, when it has printed anything at all.
        open: Option<Item<'a>>,
    },
    /// One grid, which addresses like a list of one block.
    One(Item<'a>),
}

impl<'a> Blocks<'a> {
    /// A pane's block list: the finished commands with the open one after
    /// them.
    pub fn list(finished: &'a BlockHistory, snapshot: &'a Snapshot) -> Self {
        Self::List {
            finished,
            open: snapshot.live_rows().map(|(top, bottom)| Item {
                id: snapshot.live_block.id,
                rows: Rows::Live {
                    snapshot,
                    top,
                    count: bottom - top + 1,
                },
                first: 0,
            }),
        }
    }

    /// A pane's grid: one block, under the name `id`, whose rows are numbered
    /// from the oldest line the scrollback still holds.
    ///
    /// The name is the caller's to give because a grid has exactly one item
    /// and its identity is therefore whatever the anchors already call it —
    /// which is what stops a block closing under a grid that is still up from
    /// throwing the selection away.
    pub fn grid(snapshot: &'a Snapshot, id: BlockId) -> Self {
        Self::One(Item {
            id,
            rows: Rows::Live {
                snapshot,
                top: 0,
                count: snapshot.rows,
            },
            first: grid_first_row(snapshot),
        })
    }

    /// One block, out of the store it was read back into. What a copy off the
    /// grid walks, once the rows it named have been harvested out of the
    /// emulator.
    pub fn one(id: BlockId, rows: Rows<'a>, first: usize) -> Self {
        Self::One(Item { id, rows, first })
    }

    /// How many blocks there are.
    pub fn len(&self) -> usize {
        match self {
            Self::List { finished, open } => finished.len() + usize::from(open.is_some()),
            Self::One(_) => 1,
        }
    }

    /// Whether there is nothing to select at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One block by its position in the list.
    pub fn item(&self, index: usize) -> Option<Item<'a>> {
        match self {
            Self::List { finished, open } => match finished.get(index) {
                Some(block) => Some(stored(block)),
                None => open.filter(|_| index == finished.len()),
            },
            Self::One(item) => (index == 0).then_some(*item),
        }
    }

    /// Where a block is in the list, or `None` once it has been evicted.
    ///
    /// A binary search, because ids are handed out in order: a copy taken
    /// across a session with ten thousand blocks behind it costs a handful of
    /// comparisons rather than a walk.
    pub fn index_of(&self, id: BlockId) -> Option<usize> {
        match self {
            Self::List { finished, open } => {
                if open.is_some_and(|open| open.id == id) {
                    return Some(finished.len());
                }
                finished.position(id)
            }
            Self::One(item) => (item.id == id).then_some(0),
        }
    }

    /// The block an anchor names, with its rows.
    fn holding(&self, id: BlockId) -> Option<Item<'a>> {
        self.item(self.index_of(id)?)
    }

    /// Where `text` is in the output, as the selection that covers it.
    ///
    /// A selection is two points somebody aimed a pointer at, and there is
    /// nothing to aim one with in a headless run or a test — this is how those
    /// name a region instead. It walks every block, so it is for the caller
    /// that has one thing to find rather than for a frame.
    pub fn find(&self, text: &str) -> Option<Selection> {
        if text.is_empty() {
            return None;
        }

        let mut showing = String::new();
        let mut at = Vec::new();
        self.walk(|character, place| {
            showing.push(character);
            at.push(place);
        });

        // `at` has one entry per character, and `find` answers in bytes.
        let byte = showing.find(text)?;
        let first = showing[..byte].chars().count();
        let last = first + text.chars().count() - 1;
        let (from, to) = (at.get(first)?, at.get(last)?);
        Some(Selection::new(
            SelectionKind::Simple,
            Anchor::new(from.block, from.row, from.column, CellSide::Left),
            Anchor::new(to.block, to.row, to.column, CellSide::Right),
        ))
    }

    /// Every place `needle` occurs in the output, each as the selection that
    /// covers it, in reading order.
    ///
    /// What a find bar highlights and steps through. Case-insensitive — a
    /// `grep` for `error` finds `Error`, and `ошибка` finds `Ошибка` — one
    /// character for one: see [`fold`]. Found one logical line at a time,
    /// folds and all, and never across two: see the module docs.
    pub fn find_all(&self, needle: &str) -> Vec<Selection> {
        self.find_all_with(needle, FindOptions::default())
            .unwrap_or_default()
    }

    /// [`Self::find_all`], matched as `options` says: telling capitals apart,
    /// or reading `needle` as a regular expression. `None` for a pattern that
    /// is not one, which a find bar says rather than showing "no results".
    ///
    /// A regular expression is matched a line at a time too, so its `^` and
    /// `$` are the start and end of a line, and a match of nothing — `a*`
    /// before every character — is no match: there is nothing to highlight
    /// in it.
    ///
    /// It walks every block, as [`Self::find`] does. A find bar asks the
    /// blocks one at a time instead and keeps what each one said; see
    /// [`PaneFind::matches`](crate::pane_find::PaneFind::matches).
    pub fn find_all_with(&self, needle: &str, options: FindOptions) -> Option<Vec<Selection>> {
        if needle.is_empty() {
            return Some(Vec::new());
        }
        let matcher = LineMatcher::new(needle, options)?;
        let mut found = Vec::new();
        for index in 0..self.len() {
            if let Some(item) = self.item(index) {
                item.find_with(&matcher, &mut found);
            }
        }
        Some(
            found
                .iter()
                .map(|region| {
                    Selection::new(
                        SelectionKind::Simple,
                        Anchor::new(
                            region.start.block,
                            region.start.row,
                            region.start.column,
                            CellSide::Left,
                        ),
                        Anchor::new(
                            region.end.block,
                            region.end.row,
                            region.end.column,
                            CellSide::Right,
                        ),
                    )
                })
                .collect(),
        )
    }

    /// The selection that runs from the start of the first `from` to the end
    /// of the first `to`.
    ///
    /// Two markers rather than one string, so that a region crossing a block
    /// boundary can be named without spelling out the prompt between them —
    /// which is whatever `PS1` was. See `--select-output` and
    /// `--select-through`.
    pub fn find_through(&self, from: &str, to: &str) -> Option<Selection> {
        let start = self.find(from)?;
        let end = self.find(to)?;
        Some(Selection::new(
            SelectionKind::Simple,
            start.anchor,
            end.head,
        ))
    }

    /// Every character in the output in reading order, with the cell it was
    /// drawn in, laid out exactly as a copy of the whole thing would be.
    ///
    /// One walk, so that "what does it say" and "where does it say it" cannot
    /// disagree about which cell a character came from.
    fn walk(&self, mut visit: impl FnMut(char, Place)) {
        // Where the last row ended, which is the cell a line break belongs to:
        // a match that ends at a break ends at the end of that line.
        let mut ended: Option<Place> = None;
        let mut folded = false;

        for index in 0..self.len() {
            let Some(item) = self.item(index) else {
                continue;
            };
            for local in 0..item.rows.count() {
                let row = item.first + local;
                if let Some(place) = ended.filter(|_| !folded) {
                    visit('\n', place);
                }

                let length = item.rows.line_length(local);
                let id = item.id;
                item.rows.visit_line(local, length, |character, column| {
                    visit(character, Place::new(id, row, column));
                });
                for column in blanks_before_a_fold(&item.rows, local) {
                    visit(' ', Place::new(id, row, column));
                }
                folded = item.rows.wraps(local);
                ended = Some(Place::new(
                    item.id,
                    row,
                    length.saturating_sub(1).min(item.rows.columns()),
                ));
            }
        }
    }
}

/// One query, ready to ask of a line at a time: whether the line holds it,
/// and where.
///
/// What filtering a block's output asks — which of its lines to keep — and
/// what a find asks of every line, with the same rules for both: folded or
/// told apart, text or a pattern. A find bar compiles one when its search
/// starts and keeps it, and its filter matches with that same one, rather
/// than either compiling the query again for every frame drawn while the bar
/// is up: see [`PaneFind::matcher`](crate::pane_find::PaneFind::matcher).
pub enum LineMatcher {
    /// Text, already folded when case does not matter.
    Text {
        /// The needle, as compared.
        needle: String,
        /// Whether lines are folded before they are searched.
        folded: bool,
    },
    /// A compiled pattern.
    Pattern(regex::Regex),
}

impl LineMatcher {
    /// A matcher for `needle`, or `None` for a pattern that does not compile.
    ///
    /// The one place in the workspace allowed to compile a regular
    /// expression, so that the count of compiles the find tests read sees
    /// every compile there is: `.clippy.toml` bans the `regex` constructors
    /// everywhere else.
    #[allow(
        clippy::disallowed_methods,
        reason = "this function is the replacement those lints point at"
    )]
    pub fn new(needle: &str, options: FindOptions) -> Option<Self> {
        if options.regex {
            #[cfg(test)]
            PATTERNS_COMPILED.with(|compiled| compiled.set(compiled.get() + 1));
            return regex::RegexBuilder::new(needle)
                .case_insensitive(!options.case_sensitive)
                .size_limit(1 << 20)
                .build()
                .ok()
                .map(Self::Pattern);
        }
        let folded = !options.case_sensitive;
        Some(Self::Text {
            needle: if folded {
                needle.chars().map(fold).collect()
            } else {
                needle.to_owned()
            },
            folded,
        })
    }

    /// Whether `line` holds the query.
    pub fn matches(&self, line: &str) -> bool {
        match self {
            Self::Text { needle, folded } if *folded => line
                .chars()
                .map(fold)
                .collect::<String>()
                .contains(needle.as_str()),
            Self::Text { needle, .. } => line.contains(needle.as_str()),
            Self::Pattern(pattern) => pattern.is_match(line),
        }
    }

    /// What filtering one block for the query keeps of it, when its output
    /// starts at row `from`: every row before that, and every row of each
    /// output line that holds the query.
    ///
    /// A line, not a row. A line too long for the pane is folded over
    /// several rows, and each read on its own holds a piece of it: a query
    /// that crossed the fold matched none of them while find found it, and
    /// one that matched kept its row and hid the rest of the line. So the
    /// rows are read back into lines exactly as [`Blocks::find_all`] reads
    /// them, blanks before a fold and all, and a line is kept or dropped
    /// whole — and counted once.
    pub fn keep(&self, rows: Rows<'_>, from: usize) -> Filtered {
        let count = rows.count();
        let from = from.min(count);
        let mut filtered = Filtered {
            rows: (0..from).collect(),
            ..Filtered::default()
        };
        // One buffer down the whole block rather than a string a line: the
        // block can be fifty thousand lines, read again at every change to
        // the query.
        let mut line = String::new();
        let mut starts = from;
        for row in from..count {
            rows.visit_line(row, rows.line_length(row), |character, _| {
                line.push(character);
            });
            line.extend(blanks_before_a_fold(&rows, row).map(|_| ' '));
            // Never true of the block's last row, so its last line is closed.
            if rows.wraps(row) {
                continue;
            }
            filtered.lines += 1;
            if self.matches(&line) {
                filtered.shown += 1;
                filtered.rows.extend(starts..=row);
            }
            line.clear();
            starts = row + 1;
        }
        filtered
    }
}

/// What a block's filter keeps of it: see [`LineMatcher::keep`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filtered {
    /// The block's own rows that stay on screen, in order: rows, because a
    /// row is what the list draws and what a press on it is about.
    pub rows: Vec<usize>,
    /// How many of the output's lines hold the query.
    pub shown: usize,
    /// How many lines the output has, a line folded over three rows being
    /// one: what a person reading the bar's count sees as lines.
    pub lines: usize,
}

/// How a find matches its query against the output.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FindOptions {
    /// Capitals told apart from small letters, rather than folded together.
    pub case_sensitive: bool,
    /// The query read as a regular expression, rather than as text.
    pub regex: bool,
}

/// `character` as a find compares it: its lowercase, when that is one
/// character, and itself when it is not.
///
/// Every script's case, not only ASCII's: output in Russian or Greek is
/// searched the way English is. One for one, because a find maps each
/// character it matched back to the cell it was drawn in, and a fold that
/// grew a character — `İ` lowercases to two — would put every highlight after
/// it one cell off the text. Those few keep their own case and match only
/// themselves.
fn fold(character: char) -> char {
    let mut lower = character.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(only), None) => only,
        _ => character,
    }
}

impl LineMatcher {
    /// `character` as [`Self::each_in`] wants the line it searches written:
    /// folded when the needle was, and as it is otherwise — a pattern folds
    /// for itself.
    fn searched(&self, character: char) -> char {
        match self {
            Self::Text { folded: true, .. } => fold(character),
            _ => character,
        }
    }

    /// Every place the query is in `line` — a line written through
    /// [`Self::searched`] — as the byte range it covers, in order, none of
    /// them overlapping and none of them empty.
    fn each_in(&self, line: &str, mut found: impl FnMut(usize, usize)) {
        match self {
            Self::Text { needle, .. } => {
                if needle.is_empty() {
                    return;
                }
                let mut from = 0;
                // Past the whole match each time, so overlapping runs of a
                // repeated needle are counted once each rather than at every
                // offset.
                while let Some(relative) = line[from..].find(needle.as_str()) {
                    let start = from + relative;
                    from = start + needle.len();
                    found(start, from);
                }
            }
            Self::Pattern(pattern) => {
                for matched in pattern.find_iter(line).filter(|found| !found.is_empty()) {
                    found(matched.start(), matched.end());
                }
            }
        }
    }
}

/// The regions of `found` that cover any of row `row` of block `block`, in
/// reading order.
///
/// `found` is what a find found: in reading order and none overlapping
/// another, which makes this two binary searches rather than a walk. A frame
/// asks it for every row it draws, of however many thousand matches the
/// history holds.
pub fn regions_on(found: &[Region], block: BlockId, row: usize) -> &[Region] {
    let line = (block, row);
    let from = found.partition_point(|region| region.end.line() < line);
    let to = from + found[from..].partition_point(|region| region.start.line() <= line);
    &found[from..to]
}

/// The columns of a folded row past its last printed character, which were
/// spaces the program printed rather than the rest of the grid.
///
/// A row only folds once something is written past its last column, so
/// every cell of it was printed into — but both stores trim the blanks at the
/// end of a row, and a fold puts no line break there to stand for them. So
/// `aaaaaaaaa bbb` folded at ten columns read as `aaaaaaaaabbb`: a search
/// for `a bbb` missed it, `abbb` matched text that was never there, and a
/// copy of the line lost the space. The blank a wide character leaves when
/// it does not fit in the last column is not one of these: it is where the
/// character would have gone, not a space anybody printed.
fn blanks_before_a_fold<'a>(rows: &'a Rows<'_>, row: usize) -> impl Iterator<Item = usize> + 'a {
    let from = if rows.wraps(row) {
        rows.line_length(row)
    } else {
        rows.columns()
    };
    (from..rows.columns()).filter(move |column| {
        !rows
            .cell(row, *column)
            .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER))
    })
}

#[cfg(test)]
thread_local! {
    /// See [`stored_rows_searched`].
    static STORED_ROWS_SEARCHED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many rows of finished blocks a find on this thread has read, so far.
///
/// What a test of "a frame with the find bar open reads none of them again"
/// counts. Thread-local because the window's test harness draws on the test's
/// own thread and the suite runs its tests side by side.
#[cfg(test)]
pub(crate) fn stored_rows_searched() -> usize {
    STORED_ROWS_SEARCHED.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// See [`patterns_compiled`].
    static PATTERNS_COMPILED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many regular expressions [`LineMatcher::new`] has compiled on this
/// thread, so far, whether they compiled or not.
///
/// What a test of "a frame with a pattern in the find bar does not compile
/// it again" counts, on the same terms as [`stored_rows_searched`].
#[cfg(test)]
pub(crate) fn patterns_compiled() -> usize {
    PATTERNS_COMPILED.with(std::cell::Cell::get)
}

/// The item a finished block is.
fn stored(block: &Block) -> Item<'_> {
    Item {
        id: block.id,
        rows: Rows::Stored(&block.rows),
        first: 0,
    }
}

/// The row of the grid the top of the viewport is showing, numbered from the
/// oldest line the scrollback holds.
///
/// The numbering is what keeps a selection on the grid still while the wheel
/// moves the viewport over it: the top row of the screen is a different one of
/// these after every scroll, and a selection is stored in these rather than in
/// screen rows.
///
/// **It stops being still once the scrollback is full.** From then on every
/// new line drops the oldest one, the line the numbering counts from is a
/// different line, and a grid selection slides one row up the text per line
/// printed. Nothing here can compensate: the count of lines that have been
/// dropped is not something the emulator underneath keeps, and the same limit
/// is why a block's own anchor clamps at the oldest line
/// (`crook_terminal::blocks`). It takes a command printing past the whole
/// scrollback — ten thousand lines by default — *while* a selection is being
/// held on the grid, which is the one case in this module that is known to be
/// wrong rather than merely unimplemented.
pub fn grid_first_row(snapshot: &Snapshot) -> usize {
    snapshot.history_len.saturating_sub(snapshot.display_offset)
}

/// A plain drag: everything between the two gaps, wrapping at the end of a row.
fn simple(from: Anchor, to: Anchor, blocks: &Blocks<'_>) -> Option<Region> {
    if from == to {
        return None;
    }
    let mut start = Place::new(from.block, from.row, from.column);
    let mut end = Place::new(to.block, to.row, to.column);

    // The gesture ends in the gaps, and the cells are what is between them: a
    // drag that finished on the *left* of a cell did not reach that cell, and
    // one that began on the *right* of a cell started after it.
    if to.side == CellSide::Left {
        end = before(end, blocks)?;
    }
    if from.side == CellSide::Right {
        start = after(start, blocks)?;
    }
    (start <= end).then_some(Region {
        start,
        end,
        block: false,
    })
}

/// An alt-drag: the rectangle between the two gaps, which is how a column is
/// taken out of aligned output without the rest of every line coming with it.
fn rectangle(from: Anchor, to: Anchor) -> Option<Region> {
    if from == to {
        return None;
    }
    let (left, right) = if from.column <= to.column {
        ((from.column, from.side), (to.column, to.side))
    } else {
        ((to.column, to.side), (from.column, from.side))
    };
    let first = left.0 + usize::from(left.1 == CellSide::Right);
    let last = right
        .0
        .checked_sub(usize::from(right.1 == CellSide::Left))?;
    if first > last {
        return None;
    }

    Some(Region {
        start: Place::new(from.block, from.row, first),
        end: Place::new(to.block, to.row, last),
        block: true,
    })
}

/// A double or triple click: both ends grown to whole words or whole lines.
fn grown(
    from: Anchor,
    to: Anchor,
    blocks: &Blocks<'_>,
    unit: impl Fn(Place, &Blocks<'_>) -> Option<(Place, Place)>,
) -> Option<Region> {
    let (start, _) = unit(Place::new(from.block, from.row, from.column), blocks)?;
    let (_, end) = unit(Place::new(to.block, to.row, to.column), blocks)?;
    (start <= end).then_some(Region {
        start,
        end,
        block: false,
    })
}

/// The cell before this one, stepping back a row and then a block at the ends.
fn before(at: Place, blocks: &Blocks<'_>) -> Option<Place> {
    if at.column > 0 {
        return Some(Place::new(at.block, at.row, at.column - 1));
    }
    let index = blocks.index_of(at.block)?;
    let item = blocks.item(index)?;
    if at.row > item.first {
        return Some(Place::new(
            at.block,
            at.row - 1,
            item.rows.columns().saturating_sub(1),
        ));
    }
    // Past a block with no rows in it — a command that printed nothing — and
    // on to one that has some, because there is no cell in an empty one to
    // land on.
    let previous = (0..index)
        .rev()
        .filter_map(|index| blocks.item(index))
        .find(|item| item.rows.count() > 0)?;
    Some(Place::new(
        previous.id,
        previous.range().end - 1,
        previous.rows.columns().saturating_sub(1),
    ))
}

/// The cell after this one, stepping on a row and then a block at the ends.
fn after(at: Place, blocks: &Blocks<'_>) -> Option<Place> {
    let index = blocks.index_of(at.block)?;
    let item = blocks.item(index)?;
    if at.column + 1 < item.rows.columns() {
        return Some(Place::new(at.block, at.row, at.column + 1));
    }
    if at.row + 1 < item.range().end {
        return Some(Place::new(at.block, at.row + 1, 0));
    }
    let next = (index + 1..blocks.len())
        .filter_map(|index| blocks.item(index))
        .find(|item| item.rows.count() > 0)?;
    Some(Place::new(next.id, next.first, 0))
}

/// The first and last row of the line `row` is part of: the rows the terminal
/// folded a line too long for the pane onto, and nothing beyond the block.
///
/// `None` for a row the item is not holding — one the wheel has taken off the
/// screen, one a shorter pane no longer has. There is nothing to read a word
/// or a line off there, and the caller says what to do about it.
fn folded_line(item: &Item<'_>, row: usize) -> Option<(usize, usize)> {
    let local = item.local(row)?;
    let mut first = local;
    while first > 0 && item.rows.wraps(first - 1) {
        first -= 1;
    }
    let mut last = local;
    while item.rows.wraps(last) {
        last += 1;
    }
    Some((item.first + first, item.first + last))
}

/// The whole line under a cell, which is what a triple click takes.
///
/// A cell the item is not holding grows to itself. A double or triple click is
/// kept as the two ends and the kind rather than as the region it resolved to,
/// so a line scrolled out of the viewport is ungrown *while it is out of
/// sight* and is a whole line again the moment it comes back — and the copy,
/// which reads the rows out of the emulator rather than off the screen, has it
/// whole either way.
fn line_at(at: Place, blocks: &Blocks<'_>) -> Option<(Place, Place)> {
    let item = blocks.holding(at.block)?;
    let Some((first, last)) = folded_line(&item, at.row) else {
        return Some((at, at));
    };
    Some((
        Place::new(at.block, first, 0),
        Place::new(at.block, last, item.rows.columns().saturating_sub(1)),
    ))
}

/// The word under a cell, which is what a double click takes.
///
/// Read off the whole folded line rather than off the row, so that a path or a
/// URL too long for the pane is one word rather than two halves. The rule for
/// what a word is comes from the composer's editor — see the module docs.
fn word_at(at: Place, blocks: &Blocks<'_>) -> Option<(Place, Place)> {
    let item = blocks.holding(at.block)?;
    let Some((first, last)) = folded_line(&item, at.row) else {
        return Some((at, at));
    };

    let mut line = String::new();
    let mut cells = Vec::new();
    for row in first..=last {
        let local = row - item.first;
        // One pass per row, not a `write` per column — the same O(length²) a
        // long folded line double-clicked would otherwise pay that
        // [`Blocks::walk`] shed. The byte offset a `cells` entry needs is the
        // line's length when a column's first character arrives; a wide
        // character's trailing column yields nothing and has no offset of its
        // own, the character it belongs to already having one.
        let mut wrote = None;
        item.rows
            .visit_line(local, item.rows.line_length(local), |character, column| {
                if wrote != Some(column) {
                    cells.push((line.len(), row, column));
                    wrote = Some(column);
                }
                line.push(character);
            });
        // The blanks a fold trimmed off the end of the row, as the copy and
        // the find put them back: a space in the last column is a space, and
        // without it `aaaaaaaaa bbb` folded there read as one word, so a
        // double click on `bbb` took the `a`s with it.
        for column in blanks_before_a_fold(&item.rows, local) {
            cells.push((line.len(), row, column));
            line.push(' ');
        }
    }

    // Past the last character of the line — a click in the blank tail — takes
    // the word that ends it, which is what a text field does.
    let offset = cells
        .iter()
        .rev()
        .find(|(_, row, column)| (*row, *column) <= (at.row, at.column))
        .map_or(0, |(offset, _, _)| *offset);
    let word = text::word_range_at(&line, offset);

    let start = cells
        .iter()
        .find(|(offset, _, _)| *offset >= word.start)
        .or_else(|| cells.first())?;
    let end = cells
        .iter()
        .rev()
        .find(|(offset, _, _)| *offset < word.end)
        .or_else(|| cells.last())?;
    Some((
        Place::new(at.block, start.1, start.2),
        Place::new(at.block, end.1, end.2),
    ))
}

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
