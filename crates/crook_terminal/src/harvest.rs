//! Taking a finished block's rows out of the emulator and keeping them.
//!
//! When a command finishes, its rows stop being the emulator's problem: they
//! are copied out of the grid into a [`BlockRows`] the block owns forever.
//! That is what makes a block survive `clear`, survive the scrollback filling
//! up and evicting its lines, and survive a resize reflowing everything above
//! it — none of which an approach that remembered "block seven is lines 412 to
//! 480" can do, because alacritty's history evicts from the top and reflows on
//! resize and exposes no monotonic count of what it dropped.
//!
//! # Why not `Vec<SnapshotCell>`
//!
//! A [`SnapshotCell`] is twelve bytes. Ten thousand rows of two hundred
//! columns is 24 MB — per pane, forever, with nothing evicting it. That is the
//! difference between this feature shipping and not, so rows are stored the
//! way the text actually is: the characters, with trailing blanks trimmed off,
//! and a run-length description of how they are painted. Ordinary shell output
//! is one run a row, so a row costs its bytes plus about twenty, and a
//! screenful lands at a few percent of the cell array it came from.
//!
//! Everything is flat. One `String` holds every row's text end to end, one
//! `Vec<StyleRun>` holds every row's runs, and a prefix of byte offsets says
//! where each row starts in each. A block of a thousand rows is five
//! allocations rather than three thousand.
//!
//! # Why exactly one way out
//!
//! [`BlockRows::materialise`] fills a caller-supplied `Vec<SnapshotCell>` with
//! one row. That signature is the whole point: the renderer already has a fast
//! path that walks a row of `SnapshotCell` and draws one glyph per cell with a
//! fixed advance, and a harvested row goes through it unchanged, from a scratch
//! buffer reused down the whole list. A store that handed back its own row type
//! would have grown a second copy of that painter.

use std::ops::Range;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::Term;

use crate::snapshot::{self, CellFlags, Palette, Rgb, SnapshotCell};

/// A run of neighbouring cells in a harvested row that are painted the same
/// way.
///
/// Ordinary output is one of these per row: the whole row in the default
/// colours, with the characters carried separately.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct StyleRun {
    /// First column of the run.
    pub start: u16,
    /// How many columns it covers. Never zero.
    pub len: u16,
    /// The resolved colour of the characters in it.
    pub foreground: Rgb,
    /// The resolved colour behind them.
    pub background: Rgb,
    /// What else to do when drawing them.
    pub flags: CellFlags,
}

/// The zero-width characters stacked on one cell of a harvested row.
///
/// A combining accent, a virama, a variation selector: characters that occupy
/// no column of their own and belong to the character before them. Kept beside
/// the row rather than in it for the same reason [`SnapshotCell`] keeps them
/// out — nearly every row has none.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RowCombining {
    /// Which cell of the row they belong to.
    pub column: usize,
    /// The characters, in the order the child sent them.
    pub characters: Box<[char]>,
}

/// The rows of one finished block, owned and compact.
///
/// Built by harvesting a range of the emulator's grid; read one row at a time
/// with [`Self::materialise`] for painting or [`Self::text`] for copying.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockRows {
    /// Columns each row was harvested at. Rows are stored at the width the
    /// grid had, so a run always covers exactly this many columns.
    columns: u16,
    /// Every row's characters end to end, one `char` per cell, with each row's
    /// trailing blanks dropped.
    text: String,
    /// Byte offset into `text` where each row starts, plus a final entry for
    /// the end. Empty when there are no rows at all.
    text_starts: Vec<u32>,
    /// Every row's runs end to end.
    runs: Vec<StyleRun>,
    /// Index into `runs` where each row starts, plus a final entry.
    run_starts: Vec<u32>,
    /// Every row's zero-width characters end to end. Usually empty.
    combining: Vec<RowCombining>,
    /// Index into `combining` where each row starts, plus a final entry.
    combining_starts: Vec<u32>,
}

impl BlockRows {
    /// How many rows the block holds.
    pub fn rows(&self) -> usize {
        self.text_starts.len().saturating_sub(1)
    }

    /// How wide the grid was when the block was harvested.
    ///
    /// Every row materialises to exactly this many cells, whatever the pane
    /// has been resized to since.
    pub fn columns(&self) -> usize {
        usize::from(self.columns)
    }

    /// Whether the block has no rows.
    pub fn is_empty(&self) -> bool {
        self.rows() == 0
    }

    /// Whether every row is blank.
    ///
    /// What separates a block worth keeping from the one-blank-row leftover a
    /// shell prints between a command finishing and its next prompt.
    pub fn is_blank(&self) -> bool {
        (0..self.rows()).all(|row| self.text(row).is_empty())
    }

    /// Fills `cells` with one row, ready for the renderer's cell path, and
    /// returns the zero-width characters that belong to it — an empty slice
    /// for nearly every row.
    ///
    /// `cells` is cleared first and left holding exactly [`Self::columns`]
    /// cells, or none at all when `row` is out of range. Pass the same `Vec`
    /// down the whole list: after the first row it never allocates again.
    pub fn materialise<'rows>(
        &'rows self,
        row: usize,
        cells: &mut Vec<SnapshotCell>,
    ) -> &'rows [RowCombining] {
        cells.clear();
        let Some(runs) = self.runs(row) else {
            return &[];
        };

        cells.reserve(self.columns());
        let mut characters = self.text(row).chars();
        for run in runs {
            // A trimmed row runs out of characters before it runs out of
            // columns; the cells past the end are blanks that still carry the
            // row's background, which is what keeps a coloured `\e[K` tail.
            let (foreground, background, flags) = (run.foreground, run.background, run.flags);
            for _ in 0..run.len {
                cells.push(SnapshotCell {
                    c: characters.next().unwrap_or(' '),
                    foreground,
                    background,
                    flags,
                });
            }
        }
        self.combining(row)
    }

    /// One row's characters, with trailing blanks already gone. Empty when
    /// `row` is out of range.
    ///
    /// One `char` per cell, so the trailing half of a double-width character
    /// contributes a space here exactly as it does on screen. A caller
    /// building text for a human — a copy, a log — should skip those the way
    /// [`crate::Snapshot::text`] does.
    pub fn text(&self, row: usize) -> &str {
        let Some(&start) = self.text_starts.get(row) else {
            return "";
        };
        let Some(&end) = self.text_starts.get(row + 1) else {
            return "";
        };
        &self.text[start as usize..end as usize]
    }

    /// How many columns of `row` were printed into.
    ///
    /// Everything past this is blank, which is why the store does not keep it
    /// and why a copy stops here. See [`Self::text`].
    pub fn line_length(&self, row: usize) -> usize {
        self.text(row).chars().count()
    }

    /// Whether the terminal folded a line too long for the grid here, so the
    /// row below continues this one rather than starting a new one.
    ///
    /// The flag is on the row's last cell, which is where the emulator puts
    /// it, and the runs always reach that cell however short the text is.
    pub fn wraps(&self, row: usize) -> bool {
        self.runs(row)
            .and_then(<[StyleRun]>::last)
            .is_some_and(|run| run.flags.contains(CellFlags::WRAPLINE))
    }

    /// One cell of a row, or `None` when either index is out of range.
    ///
    /// For the caller that wants a cell rather than a row — a highlight
    /// deciding whether it is standing on half of a double-width character.
    /// Painting a row goes through [`Self::materialise`] instead.
    pub fn cell(&self, row: usize, column: usize) -> Option<SnapshotCell> {
        let run = self
            .runs(row)?
            .iter()
            .find(|run| column < usize::from(run.start) + usize::from(run.len))
            .filter(|run| column >= usize::from(run.start))?;
        Some(SnapshotCell {
            c: self.text(row).chars().nth(column).unwrap_or(' '),
            foreground: run.foreground,
            background: run.background,
            flags: run.flags,
        })
    }

    /// The zero-width characters stacked on one cell of a row.
    pub fn zerowidth(&self, row: usize, column: usize) -> &[char] {
        self.combining(row)
            .iter()
            .find(|marks| marks.column == column)
            .map_or(&[], |marks| &marks.characters)
    }

    /// Every row, joined by newlines: what the block *looks* like, one line
    /// per row of the grid.
    ///
    /// The same picture [`crate::Snapshot::text`] gives of a live grid, and
    /// the counterpart of it for a block that has been harvested — which is
    /// what makes the two comparable. The trailing column of a double-width
    /// character contributes nothing, because the grid holds a space there and
    /// copying it puts one inside every CJK word and after every emoji, and
    /// the zero-width characters stacked on a cell follow the character they
    /// belong to instead of being dropped.
    ///
    /// **Not what a copy of the block hands the clipboard.** A row the
    /// terminal folded because the line was too long for the pane gets a
    /// newline here and does not get one there: a copy is the text the shell
    /// printed, and this is the shape it was printed into. The clipboard is
    /// served by one region resolved over the block, in `app`'s `selection`,
    /// so that a drag across a block and its copy control cannot disagree.
    pub fn to_text(&self) -> String {
        let mut text = String::with_capacity(self.text.len() + self.rows());
        for row in 0..self.rows() {
            if row > 0 {
                text.push('\n');
            }
            self.write_row(row, &mut text);
        }
        text
    }

    /// Appends the characters of `columns` on one row to `out`, as a person
    /// would read them.
    pub fn write(&self, row: usize, columns: Range<usize>, out: &mut String) {
        let combining = self.combining(row);
        // Runs cover every column of the row where the characters stop at the
        // last one that was printed, so the zip ends with the text and the
        // flags stay in step with it.
        let spacers = self.runs(row).unwrap_or_default().iter().flat_map(|run| {
            std::iter::repeat_n(
                run.flags.contains(CellFlags::WIDE_SPACER),
                usize::from(run.len),
            )
        });

        for (column, (character, spacer)) in self
            .text(row)
            .chars()
            .zip(spacers)
            .enumerate()
            .skip(columns.start)
        {
            if column >= columns.end {
                break;
            }
            if spacer {
                continue;
            }
            out.push(character);
            if let Some(marks) = combining.iter().find(|marks| marks.column == column) {
                out.extend(marks.characters.iter());
            }
        }
    }

    /// Appends one row's characters to `out`, as a person would read them.
    fn write_row(&self, row: usize, out: &mut String) {
        self.write(row, 0..self.columns(), out);
    }

    /// Roughly how many bytes of heap the block's rows occupy.
    ///
    /// For comparing against `rows * columns * size_of::<SnapshotCell>()`,
    /// which is what this type exists not to be.
    pub fn memory_usage(&self) -> usize {
        let combining: usize = self
            .combining
            .iter()
            .map(|entry| size_of_val(&*entry.characters))
            .sum();
        self.text.capacity()
            + self.text_starts.capacity() * size_of::<u32>()
            + self.runs.capacity() * size_of::<StyleRun>()
            + self.run_starts.capacity() * size_of::<u32>()
            + self.combining.capacity() * size_of::<RowCombining>()
            + self.combining_starts.capacity() * size_of::<u32>()
            + combining
    }

    /// One row's style runs, or `None` when `row` is out of range.
    pub fn runs(&self, row: usize) -> Option<&[StyleRun]> {
        let start = *self.run_starts.get(row)? as usize;
        let end = *self.run_starts.get(row + 1)? as usize;
        Some(&self.runs[start..end])
    }

    /// One row's zero-width characters.
    fn combining(&self, row: usize) -> &[RowCombining] {
        let (Some(&start), Some(&end)) = (
            self.combining_starts.get(row),
            self.combining_starts.get(row + 1),
        ) else {
            return &[];
        };
        &self.combining[start as usize..end as usize]
    }

    /// Starts a store for rows of the given width.
    fn with_columns(columns: usize) -> Self {
        Self {
            columns: u16::try_from(columns).unwrap_or(u16::MAX),
            ..Self::default()
        }
    }

    /// Appends one row: its cells in order, and the zero-width characters
    /// found on any of them.
    ///
    /// Runs are merged as they are pushed, so a row painted in one style
    /// arrives as one run however wide the grid is.
    fn push_row(&mut self, cells: &[SnapshotCell], combining: &mut Vec<RowCombining>) {
        if self.text_starts.is_empty() {
            self.text_starts.push(0);
            self.run_starts.push(0);
            self.combining_starts.push(0);
        }

        let trimmed = cells
            .iter()
            .rposition(|cell| cell.c != ' ')
            .map_or(0, |at| at + 1);
        self.text.extend(cells[..trimmed].iter().map(|cell| cell.c));

        for (column, cell) in cells.iter().enumerate() {
            let extends = self.runs.last().is_some_and(|run| {
                usize::from(run.start) + usize::from(run.len) == column
                    && run.foreground == cell.foreground
                    && run.background == cell.background
                    && run.flags == cell.flags
            });
            match self.runs.last_mut() {
                Some(run) if extends => run.len += 1,
                _ => self.runs.push(StyleRun {
                    start: column as u16,
                    len: 1,
                    foreground: cell.foreground,
                    background: cell.background,
                    flags: cell.flags,
                }),
            }
        }

        self.combining.append(combining);
        self.text_starts.push(self.text.len() as u32);
        self.run_starts.push(self.runs.len() as u32);
        self.combining_starts.push(self.combining.len() as u32);
    }
}

/// Copies grid lines `top..=bottom` out of the emulator, resolving every
/// colour through `palette` exactly as [`crate::Snapshot`] does.
///
/// The range is clamped to what the grid actually holds, so a `top` that has
/// already scrolled past the oldest line of the history yields the block from
/// the oldest line it still has rather than nothing — a block that swallowed a
/// row of the one before it is a merge, and merges are recoverable where a
/// split is not.
pub(crate) fn harvest<T>(term: &Term<T>, palette: &Palette, top: i32, bottom: i32) -> BlockRows {
    let grid = term.grid();
    let columns = grid.columns();
    let mut rows = BlockRows::with_columns(columns);

    let oldest = -(grid.history_size() as i32);
    let newest = grid.screen_lines() as i32 - 1;
    let (first, last) = (top.max(oldest), bottom.min(newest));
    if first > last {
        return rows;
    }

    let overrides = term.colors();
    let mut cells = Vec::with_capacity(columns);
    let mut combining = Vec::new();
    for line in first..=last {
        cells.clear();
        for (column, cell) in grid[Line(line)][..].iter().enumerate() {
            if let Some(marks) = cell.zerowidth().filter(|marks| !marks.is_empty()) {
                combining.push(RowCombining {
                    column,
                    characters: marks.into(),
                });
            }
            cells.push(snapshot::convert(cell, palette, overrides));
        }
        // `push_row` drains `combining`, so its allocation is reused by every
        // row that needs one.
        rows.push_row(&cells, &mut combining);
    }
    rows
}

#[cfg(test)]
#[path = "harvest_tests.rs"]
mod tests;
