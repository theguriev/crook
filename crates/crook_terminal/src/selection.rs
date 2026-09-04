//! The vocabulary a pointer selects text with.
//!
//! The selection itself is not here, and is not in this crate at all. A pane's
//! output is a list of blocks, and every block but the one still open was
//! harvested out of the emulator when its command ended — so a selection
//! anchored to a cell of the grid could only ever cover the newest of them.
//! It is anchored to a block and a row of that block instead, one level up,
//! where the list that draws them is: see `crook::selection`.
//!
//! What is left here is the two words that side of the seam and this one both
//! have to use: what a gesture is asking for, and which gap between two cells
//! a pointer is in. They are in this crate because they are what
//! [`Rows`](crate::Rows) is addressed with, and because a renderer that named
//! an alacritty type would be a renderer that could not change emulator.

/// What one gesture selects at a time.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    /// Exactly the cells between the two ends, wrapping at the end of a row.
    /// What a plain drag selects.
    Simple,
    /// The rectangle between the two ends, which is how a column is taken out
    /// of aligned output.
    Block,
    /// Whole words. What a double click selects.
    Semantic,
    /// Whole lines, including the rows a long line wrapped onto. What a triple
    /// click selects.
    Lines,
}

/// Which half of a cell a pointer is on.
///
/// A selection ends *between* two cells, not on one, and this is which side of
/// the cell under the pointer that gap is on. It is what makes dragging left
/// from the middle of a character take that character, and dragging right from
/// the same place leave it — the behaviour every text selection has and the one
/// nobody notices until it is missing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CellSide {
    /// The gap before the cell.
    Left,
    /// The gap after it.
    Right,
}
