//! One block's rows, whichever of the two places its cells live in.
//!
//! A pane's output is a list of blocks, and exactly one of them — the open one
//! — still has its cells in the emulator's grid. Every other block's were
//! copied out into a [`BlockRows`] when its command ended, and a pane that
//! draws one grid instead of a list has no finished blocks at all.
//!
//! Everything that reads text out of a block has to work on both stores, and
//! writing it twice is precisely how the two come to disagree — a selection
//! that highlights one thing and copies another. So this is the one type that
//! knows the difference, and it is small enough to read in a sitting: five
//! questions, answered twice each.

use std::ops::Range;

use crate::harvest::BlockRows;
use crate::snapshot::{CellFlags, Snapshot, SnapshotCell};

/// The rows of one block.
#[derive(Copy, Clone, Debug)]
pub enum Rows<'a> {
    /// A finished block's, out of the store its command was harvested into.
    Stored(&'a BlockRows),
    /// A window of the viewport: the open block's rows, or — where a pane
    /// draws one grid rather than a list — every row of it.
    Live {
        /// The grid they are drawn from.
        snapshot: &'a Snapshot,
        /// The viewport row this block's row zero is drawn on.
        top: usize,
        /// How many rows it has.
        count: usize,
    },
}

impl<'a> Rows<'a> {
    /// How many rows the block holds.
    pub fn count(&self) -> usize {
        match self {
            Self::Stored(rows) => rows.rows(),
            Self::Live {
                snapshot,
                top,
                count,
            } => (*count).min(snapshot.rows.saturating_sub(*top)),
        }
    }

    /// How many columns each of them has.
    ///
    /// A finished block keeps the width it was harvested at, whatever the pane
    /// has been resized to since; the open one has the width the grid has now.
    /// That is why a selection's columns belong to the block they were taken
    /// from rather than to the pane.
    pub fn columns(&self) -> usize {
        match self {
            Self::Stored(rows) => rows.columns(),
            Self::Live { snapshot, .. } => snapshot.columns,
        }
    }

    /// One cell, or `None` when either index is outside the block.
    pub fn cell(&self, row: usize, column: usize) -> Option<SnapshotCell> {
        if row >= self.count() {
            return None;
        }
        match self {
            Self::Stored(rows) => rows.cell(row, column),
            Self::Live { snapshot, top, .. } => snapshot.cell(top + row, column).copied(),
        }
    }

    /// The zero-width characters stacked on one cell — an accent, a virama, a
    /// variation selector — which belong to the character before them.
    pub fn zerowidth(&self, row: usize, column: usize) -> &'a [char] {
        if row >= self.count() {
            return &[];
        }
        match self {
            Self::Stored(rows) => rows.zerowidth(row, column),
            Self::Live { snapshot, top, .. } => snapshot.zerowidth(top + row, column),
        }
    }

    /// How many columns of `row` were printed into.
    ///
    /// Everything past this is blank, and blanks at the end of a row are not
    /// text anybody typed: they are the rest of a grid that has to hold
    /// something. A copy stops here, which is what keeps a selection dragged
    /// past the end of a line from bringing eighty spaces with it.
    ///
    /// A row the block does not have is empty rather than a panic, as it is
    /// for every other question here: the viewport this is asked about shrinks
    /// under a resize and slides under a scroll, and a selection that named a
    /// row before either is still a selection.
    pub fn line_length(&self, row: usize) -> usize {
        if row >= self.count() {
            return 0;
        }
        match self {
            Self::Stored(rows) => rows.line_length(row),
            Self::Live { snapshot, top, .. } => snapshot
                .row(top + row)
                .iter()
                .rposition(|cell| cell.c != ' ')
                .map_or(0, |at| at + 1),
        }
    }

    /// Whether the terminal folded a line too long for the grid here, so the
    /// row below continues this one rather than starting a new one.
    ///
    /// Never true of a block's last row, whatever flag the emulator left on
    /// it. What follows a block is a different command, and a copy that ran
    /// the two together into one line because one of them happened to fill its
    /// last row exactly would be inventing a line nobody printed.
    pub fn wraps(&self, row: usize) -> bool {
        if row + 1 >= self.count() {
            return false;
        }
        match self {
            Self::Stored(rows) => rows.wraps(row),
            Self::Live { .. } => self
                .cell(row, self.columns().saturating_sub(1))
                .is_some_and(|cell| cell.flags.contains(CellFlags::WRAPLINE)),
        }
    }

    /// `columns` grown to cover whole characters.
    ///
    /// A double-width character is one character in two columns, drawn once
    /// across both, so reaching either of them reaches all of it. Highlighting
    /// and copying both ask this, which is what stops a highlight cutting a
    /// glyph down the middle or a copy losing the character under it.
    pub fn whole_characters(&self, row: usize, columns: Range<usize>) -> Range<usize> {
        let last = self.columns();
        let start = match self.cell(row, columns.start) {
            Some(cell) if cell.flags.contains(CellFlags::WIDE_SPACER) => {
                columns.start.saturating_sub(1)
            }
            _ => columns.start,
        };
        let end = match columns.end.checked_sub(1).and_then(|at| self.cell(row, at)) {
            Some(cell) if cell.flags.contains(CellFlags::WIDE) => (columns.end + 1).min(last),
            _ => columns.end.min(last),
        };
        start..end
    }

    /// Appends the characters of `columns` on one row to `out`, as a person
    /// reads them.
    ///
    /// The trailing column of a double-width character contributes nothing —
    /// the grid holds a space there, and copying it would put one inside every
    /// CJK word and after every emoji — and the zero-width characters stacked
    /// on a cell follow the character they belong to.
    pub fn write(&self, row: usize, columns: Range<usize>, out: &mut String) {
        if row >= self.count() {
            return;
        }
        match self {
            Self::Stored(rows) => rows.write(row, columns, out),
            Self::Live { snapshot, top, .. } => {
                let cells = snapshot.row(top + row);
                let end = columns.end.min(cells.len());
                for (column, cell) in cells[..end].iter().enumerate().skip(columns.start) {
                    if cell.flags.contains(CellFlags::WIDE_SPACER) {
                        continue;
                    }
                    out.push(cell.c);
                    out.extend(snapshot.zerowidth(top + row, column));
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "rows_tests.rs"]
mod tests;
