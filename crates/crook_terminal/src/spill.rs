//! The open block's rows that have left the grid: see
//! [`crate::blocks::BlockTracker::spilled`].
//!
//! Held in pieces of a few hundred rows rather than as one [`BlockRows`], for
//! the two things a long-lived open block does to them all the time. Rows
//! arrive at the end, a slice of output's worth at a time, and go into the
//! last piece where they are harvested, with no store built for them on the
//! way. And once the scrollback is full the oldest go, a piece at a time,
//! which drops the piece and moves nothing else.

use std::collections::VecDeque;
use std::ops::Range;

use alacritty_terminal::term::Term;

use crate::harvest::{self, BlockRows, RowCombining};
use crate::snapshot::{Palette, SnapshotCell};

/// The most rows one piece holds.
pub(crate) const PIECE_ROWS: usize = 512;

/// The open block's rows past the grid, oldest first, in pieces of one width.
#[derive(Debug, Default)]
pub(crate) struct Spill {
    pieces: VecDeque<BlockRows>,
    rows: usize,
    /// The most rows a piece is filled to: see [`Self::harvest`].
    piece_rows: usize,
}

impl Spill {
    /// How many rows there are.
    pub(crate) fn rows(&self) -> usize {
        self.rows
    }

    /// Whether there are none.
    pub(crate) fn is_empty(&self) -> bool {
        self.rows == 0
    }

    /// How wide the rows are, or `None` when there are none.
    pub(crate) fn columns(&self) -> Option<usize> {
        self.pieces.front().map(BlockRows::columns)
    }

    /// Lets go of every row.
    pub(crate) fn clear(&mut self) {
        self.pieces.clear();
        self.rows = 0;
    }

    /// The piece `row` is in, and its row there.
    fn locate(&self, mut row: usize) -> Option<(&BlockRows, usize)> {
        for piece in &self.pieces {
            if row < piece.rows() {
                return Some((piece, row));
            }
            row -= piece.rows();
        }
        None
    }

    /// One row, as [`BlockRows::materialise`] hands it out.
    pub(crate) fn materialise<'rows>(
        &'rows self,
        row: usize,
        cells: &mut Vec<SnapshotCell>,
    ) -> &'rows [RowCombining] {
        match self.locate(row) {
            Some((piece, row)) => piece.materialise(row, cells),
            None => {
                cells.clear();
                &[]
            }
        }
    }

    /// Whether the terminal folded the line `row` is on into the row below.
    pub(crate) fn wraps(&self, row: usize) -> bool {
        self.locate(row)
            .is_some_and(|(piece, row)| piece.wraps(row))
    }

    /// A copy of the rows in `range`, clamped to the rows there are.
    pub(crate) fn slice(&self, range: Range<usize>) -> BlockRows {
        let mut slice = BlockRows::with_columns(self.columns().unwrap_or(0));
        let mut at = 0;
        for piece in &self.pieces {
            let (from, to) = (at, at + piece.rows());
            at = to;
            if to <= range.start {
                continue;
            }
            if from >= range.end {
                break;
            }
            let part = piece.slice(range.start.saturating_sub(from)..range.end.min(to) - from);
            slice.append(part);
        }
        slice
    }

    /// Every row, as one store, leaving none.
    pub(crate) fn take(&mut self) -> BlockRows {
        let mut pieces = std::mem::take(&mut self.pieces);
        self.rows = 0;
        // One store, of every piece, which none of them is laid out for.
        let mut rows = pieces.pop_front().unwrap_or_default();
        for piece in pieces {
            rows.append(piece);
        }
        rows
    }

    /// Harvests grid lines `top..=bottom` onto the end, the way
    /// [`harvest::harvest`] would into a store of their own, in pieces of at
    /// most `piece_rows`. Rows of another width than the ones held start
    /// over: the spill has to end where the grid begins, so it is the older
    /// that go.
    pub(crate) fn harvest<T>(
        &mut self,
        term: &Term<T>,
        palette: &Palette,
        (top, bottom): (i32, i32),
        piece_rows: usize,
    ) {
        let piece_rows = piece_rows.clamp(1, PIECE_ROWS);
        self.piece_rows = piece_rows;
        let columns = alacritty_terminal::grid::Dimensions::columns(term.grid());
        if self.columns().is_some_and(|held| held != columns) {
            self.clear();
        }
        let mut line = top;
        while line <= bottom {
            let room = match self.pieces.back() {
                Some(piece) if piece.rows() < piece_rows => piece_rows - piece.rows(),
                _ => {
                    if let Some(full) = self.pieces.back_mut() {
                        full.shrink_to_fit();
                    }
                    self.pieces.push_back(BlockRows::with_columns(columns));
                    piece_rows
                }
            };
            let to = bottom.min(line + room as i32 - 1);
            let piece = self.pieces.back_mut().expect("a piece was just made");
            let before = piece.rows();
            harvest::harvest_into(term, palette, line, to, piece);
            let added = piece.rows() - before;
            self.rows += added;
            if added == 0 {
                break;
            }
            line = to + 1;
        }
    }

    /// Lets go of the oldest pieces for as long as at least `keep` rows are
    /// left: the scrollback is full, and they are the furthest up. Answers
    /// how many rows went.
    pub(crate) fn drop_oldest(&mut self, keep: usize) -> usize {
        let before = self.rows;
        while let Some(front) = self.pieces.front() {
            if self.rows - front.rows() < keep {
                break;
            }
            self.rows -= front.rows();
            self.pieces.pop_front();
        }
        before - self.rows
    }

    /// Lays the rows out again at `columns`: see [`BlockRows::rewrapped_tracking`].
    /// Answers which row the row `track` was is now, the first one of the
    /// rows its cells went to.
    pub(crate) fn rewrap(
        &mut self,
        columns: usize,
        blank: SnapshotCell,
        track: Option<usize>,
    ) -> Option<usize> {
        if self.is_empty() || self.columns() == Some(columns) {
            return track;
        }
        let piece_rows = self.piece_rows.clamp(1, PIECE_ROWS);
        let (rows, tracked) = self.take().rewrapped_tracking(columns, blank, track);
        self.piece_rows = piece_rows;
        let mut at = 0;
        while at < rows.rows() {
            let mut piece = rows.slice(at..at + piece_rows);
            piece.shrink_to_fit();
            at += piece.rows();
            self.rows += piece.rows();
            self.pieces.push_back(piece);
        }
        tracked
    }

    /// Roughly how many bytes of heap the rows occupy: see
    /// [`BlockRows::memory_usage`].
    #[cfg(test)]
    pub(crate) fn memory_usage(&self) -> usize {
        self.pieces.iter().map(BlockRows::memory_usage).sum()
    }
}
