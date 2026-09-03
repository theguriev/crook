//! Selecting text on the grid.
//!
//! The selection itself is not modelled here. `alacritty_terminal` already has
//! it — `Selection`, `SelectionType`, `Selection::rotate`, `Term::selection`,
//! `Term::selection_to_string` — and, crucially, [`Term`] *drives* it: every
//! path that scrolls the grid rotates the selection with the text before it
//! moves a single cell, so a selection made against `line 40 of 200` still
//! covers `line 40 of 200` after another screenful has been printed
//! underneath it. Re-implementing that on top of the emulator would mean
//! re-implementing exactly the part that is hard to get right.
//!
//! What this module is, then, is a *vocabulary*: the four types the rest of
//! Crook needs to ask for a selection without ever naming an alacritty type.
//! The renderer never links against the emulator's index types, and a change
//! of emulator is a change to two `From` impls rather than to every element
//! that draws a highlight.
//!
//! # Two kinds of point, and why they are two types
//!
//! [`ViewportPoint`] is a cell of what is *on screen*: row zero is the top row
//! being drawn, whatever the terminal is scrolled to. [`GridPoint`] is a cell
//! of the *text*: line zero is the first line of the live screen and negative
//! lines are scrollback, so a given [`GridPoint`] names the same characters
//! however far the viewport has moved since.
//!
//! A mouse produces the first and a selection is stored in the second, and
//! confusing them is the whole bug class this feature has: a selection made
//! four screens back in the scrollback that lands on the live output instead.
//! They are separate types so that the conversion — which needs the display
//! offset, and so needs the emulator — cannot be skipped by accident.

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{SelectionRange, SelectionType};

/// What one gesture selects at a time.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    /// Exactly the cells between the two ends, wrapping at the end of a row.
    /// What a plain drag selects.
    Simple,
    /// The rectangle between the two ends, which is how a column is taken out
    /// of aligned output.
    Block,
    /// Whole words, using the emulator's own word rules. What a double click
    /// selects.
    Semantic,
    /// Whole lines, including the rows a long line wrapped onto. What a triple
    /// click selects.
    Lines,
}

impl From<SelectionKind> for SelectionType {
    fn from(kind: SelectionKind) -> Self {
        match kind {
            SelectionKind::Simple => Self::Simple,
            SelectionKind::Block => Self::Block,
            SelectionKind::Semantic => Self::Semantic,
            SelectionKind::Lines => Self::Lines,
        }
    }
}

/// Which half of a cell a pointer is on.
///
/// A selection ends *between* two cells, not on one, and this is which side of
/// the cell under the pointer that gap is on. It is what makes dragging left
/// from the middle of a character take that character, and dragging right from
/// the same place leave it — the behaviour every text selection has and the one
/// nobody notices until it is missing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum CellSide {
    /// The gap before the cell.
    Left,
    /// The gap after it.
    Right,
}

impl From<CellSide> for Side {
    fn from(side: CellSide) -> Self {
        match side {
            CellSide::Left => Self::Left,
            CellSide::Right => Self::Right,
        }
    }
}

/// A cell of what is currently on screen.
///
/// Row zero is the top drawn row. Turn one into a [`GridPoint`] with
/// [`crate::Emulator::grid_point`], which is the only thing that knows how far
/// the viewport has been scrolled.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewportPoint {
    /// Row within the viewport, counting down from the top.
    pub row: usize,
    /// Column within the row.
    pub column: usize,
}

impl ViewportPoint {
    /// A cell of the viewport.
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// A cell of the terminal's text, scrollback included.
///
/// Line zero is the first line of the live screen; negative lines are history
/// above it. This is the coordinate a selection is *stored* in, which is what
/// lets it stay on the same characters while output scrolls underneath.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GridPoint {
    /// Line of the grid: zero is the top of the live screen, negative is
    /// scrollback.
    pub line: i32,
    /// Column within the line.
    pub column: usize,
}

impl GridPoint {
    /// A cell of the grid.
    pub const fn new(line: i32, column: usize) -> Self {
        Self { line, column }
    }
}

impl From<GridPoint> for Point {
    fn from(point: GridPoint) -> Self {
        Self::new(Line(point.line), Column(point.column))
    }
}

impl From<Point> for GridPoint {
    fn from(point: Point) -> Self {
        Self::new(point.line.0, point.column.0)
    }
}

/// The selected region, as the renderer is given it.
///
/// Inclusive at both ends: `start` is the first selected cell and `end` is the
/// last, in reading order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SelectionSpan {
    /// The first selected cell.
    pub start: GridPoint,
    /// The last selected cell.
    pub end: GridPoint,
    /// Whether this is a rectangle rather than a run of text. A block
    /// selection takes the same columns out of every line it covers; every
    /// other kind runs to the end of a line and continues on the next.
    pub block: bool,
}

impl SelectionSpan {
    /// Whether a cell is inside the span.
    ///
    /// The same rule `SelectionRange::contains` states, and it has to stay the
    /// same rule: this is what the renderer highlights and that is what
    /// `Term::selection_to_string` copies, so a disagreement between them is a
    /// person copying something other than what they can see.
    pub fn contains(&self, point: GridPoint) -> bool {
        self.start.line <= point.line
            && self.end.line >= point.line
            && (self.start.column <= point.column || (self.start.line != point.line && !self.block))
            && (self.end.column >= point.column || (self.end.line != point.line && !self.block))
    }
}

impl From<SelectionRange> for SelectionSpan {
    fn from(range: SelectionRange) -> Self {
        Self {
            start: range.start.into(),
            end: range.end.into(),
            block: range.is_block,
        }
    }
}

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
