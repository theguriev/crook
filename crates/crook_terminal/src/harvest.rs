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

use crate::rows::TabPadding;
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

        let mut padding = TabPadding::default();
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
            if spacer || padding.skips(column, character) {
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

    /// Visits each character of a row with the column it sits on, left to right
    /// in one pass — the base character, then any combining marks stacked on
    /// it, all at that column; a wide character's spacer column yields nothing.
    /// `length` bounds it the way a caller walking `0..length` with
    /// [`Self::write`] one column at a time would.
    ///
    /// That caller is why this exists: [`Self::write`] reaches its start column
    /// by `.skip`-ing a `Chars` iterator, which has no O(1) `nth`, so a column
    /// at a time is O(length²) per row. One left-to-right pass is O(length).
    pub fn visit_line(&self, row: usize, length: usize, mut visit: impl FnMut(char, usize)) {
        let combining = self.combining(row);
        let spacers = self.runs(row).unwrap_or_default().iter().flat_map(|run| {
            std::iter::repeat_n(
                run.flags.contains(CellFlags::WIDE_SPACER),
                usize::from(run.len),
            )
        });
        let mut padding = TabPadding::default();
        for (column, (character, spacer)) in self.text(row).chars().zip(spacers).enumerate() {
            if column >= length {
                break;
            }
            if spacer || padding.skips(column, character) {
                continue;
            }
            visit(character, column);
            if let Some(marks) = combining.iter().find(|marks| marks.column == column) {
                for mark in marks.characters.iter() {
                    visit(*mark, column);
                }
            }
        }
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

    /// Puts `other`'s rows after this store's last, reporting whether it could.
    ///
    /// For a harvest taken in pieces: a grid's whole scrollback read a few
    /// hundred rows at a time, so that the lock the pty's reader parses under
    /// is let go between the pieces rather than held for the whole of it. The
    /// pieces must have been harvested at the same width, since every row's
    /// runs cover exactly [`Self::columns`] cells; a piece of another width —
    /// a resize that landed between two of them — is refused and left out.
    pub fn append(&mut self, other: BlockRows) -> bool {
        if other.is_empty() {
            return true;
        }
        if self.is_empty() {
            *self = other;
            return true;
        }
        if other.columns != self.columns {
            return false;
        }
        // Each list of starts opens with a zero for its first row, and this
        // store's own closing entry already says where that row now begins.
        let (text, runs, combining) = (
            self.text.len() as u32,
            self.runs.len() as u32,
            self.combining.len() as u32,
        );
        self.text.push_str(&other.text);
        self.text_starts
            .extend(other.text_starts[1..].iter().map(|start| start + text));
        self.runs.extend_from_slice(&other.runs);
        self.run_starts
            .extend(other.run_starts[1..].iter().map(|start| start + runs));
        self.combining.extend(other.combining);
        self.combining_starts.extend(
            other.combining_starts[1..]
                .iter()
                .map(|start| start + combining),
        );
        true
    }

    /// One row's style runs, or `None` when `row` is out of range.
    pub fn runs(&self, row: usize) -> Option<&[StyleRun]> {
        let start = *self.run_starts.get(row)? as usize;
        let end = *self.run_starts.get(row + 1)? as usize;
        Some(&self.runs[start..end])
    }

    /// A copy of the rows in `rows`, clamped to the rows there are, at this
    /// store's width.
    pub(crate) fn slice(&self, rows: Range<usize>) -> BlockRows {
        let end = rows.end.min(self.rows());
        let start = rows.start.min(end);
        let mut slice = Self::with_columns(self.columns());
        if start == end {
            return slice;
        }
        let (text, run, combining) = (
            self.text_starts[start],
            self.run_starts[start],
            self.combining_starts[start],
        );
        slice.text = self.text[text as usize..self.text_starts[end] as usize].to_owned();
        slice.text_starts = self.text_starts[start..=end]
            .iter()
            .map(|at| at - text)
            .collect();
        slice.runs = self.runs[run as usize..self.run_starts[end] as usize].to_vec();
        slice.run_starts = self.run_starts[start..=end]
            .iter()
            .map(|at| at - run)
            .collect();
        slice.combining =
            self.combining[combining as usize..self.combining_starts[end] as usize].to_vec();
        slice.combining_starts = self.combining_starts[start..=end]
            .iter()
            .map(|at| at - combining)
            .collect();
        slice
    }

    /// Lets go of the first `count` rows, or of every row when there are no
    /// more than that. The width stays.
    pub(crate) fn drop_front(&mut self, count: usize) {
        if count > 0 {
            *self = self.slice(count..self.rows());
        }
    }

    /// The same rows laid out again at `columns`, the way the grid lays out
    /// the lines it holds when its width changes.
    ///
    /// A line the terminal folded is one line again, read off the rows it was
    /// folded over, and folded anew where the new width ends: a double-width
    /// character that would straddle the edge starts the next row and leaves
    /// a spacer behind, and each row a line goes on past is marked folded.
    /// What a line never printed into — the cells after its last one that are
    /// `blank` — is not carried, so a narrower width does not turn a short
    /// line into a row of text and a row of nothing; a wider one pads each
    /// row out with `blank`.
    ///
    /// The last line may be one whose rows go on below this store — the
    /// store holds the head of a block and the grid the rest of it — and its
    /// fold into the rows below is the one thing a store cannot lay out
    /// again. It ends on its own row instead, unfolded: the line is cut there,
    /// which is what the grid does to a line whose head it no longer holds.
    ///
    /// A line with no double-width character and no zero-width one, which is
    /// nearly every line, is laid out from its text and its runs, a run at a
    /// time; the rest a cell at a time.
    ///
    /// Answers also which row `track` is laid out on: the row its first cell
    /// went to.
    pub(crate) fn rewrapped_tracking(
        &self,
        columns: usize,
        blank: SnapshotCell,
        track: Option<usize>,
    ) -> (BlockRows, Option<usize>) {
        self.rewrap(columns, blank, track, true)
    }

    /// Tests: [`Self::rewrapped_tracking`]'s rows.
    #[cfg(test)]
    pub(crate) fn rewrapped(&self, columns: usize, blank: SnapshotCell) -> BlockRows {
        self.rewrapped_tracking(columns, blank, None).0
    }

    /// [`Self::rewrapped_tracking`], a run at a time where it can be when
    /// `by_runs`, and a cell at a time everywhere when not.
    fn rewrap(
        &self,
        columns: usize,
        blank: SnapshotCell,
        track: Option<usize>,
        by_runs: bool,
    ) -> (BlockRows, Option<usize>) {
        let columns = columns.max(1);
        let mut laid = Self::with_columns(columns);
        let mut tracked = None;
        let mut scratch = Rewrap::default();
        let mut row = 0;
        while row < self.rows() {
            let mut end = row;
            while end + 1 < self.rows() && self.wraps(end) {
                end += 1;
            }
            let first = laid.rows();
            let cells_before = if by_runs && self.is_plain(row..end + 1) {
                laid.push_plain_line(self, row..end + 1, columns, blank, track, &mut scratch)
            } else {
                laid.push_line_by_cells(self, row..end + 1, columns, blank, track, &mut scratch)
            };
            if let Some(before) = cells_before {
                tracked = Some((first + before / columns).min(laid.rows().saturating_sub(1)));
            }
            row = end + 1;
        }
        (laid, tracked)
    }

    /// Tests: [`Self::rewrapped_tracking`] a cell at a time on every line, which the
    /// way it lays out most lines has to agree with.
    #[cfg(test)]
    pub(crate) fn rewrapped_by_cells(
        &self,
        columns: usize,
        blank: SnapshotCell,
        track: Option<usize>,
    ) -> (BlockRows, Option<usize>) {
        self.rewrap(columns, blank, track, false)
    }

    /// Whether `rows` have no double-width character and no zero-width one.
    fn is_plain(&self, rows: Range<usize>) -> bool {
        let wide = CellFlags::WIDE | CellFlags::WIDE_SPACER;
        self.combining_starts[rows.start] == self.combining_starts[rows.end]
            && self.runs[self.run_starts[rows.start] as usize..self.run_starts[rows.end] as usize]
                .iter()
                .all(|run| run.flags.without(wide) == run.flags)
    }

    /// Lays out the line on `rows` of `source` a cell at a time, answering
    /// how many of its cells come before `track`'s first, when `track` is
    /// one of them.
    fn push_line_by_cells(
        &mut self,
        source: &BlockRows,
        rows: Range<usize>,
        columns: usize,
        blank: SnapshotCell,
        track: Option<usize>,
        scratch: &mut Rewrap,
    ) -> Option<usize> {
        let Rewrap { cells, line, .. } = scratch;
        line.clear();
        let mut marks: Vec<RowCombining> = Vec::new();
        let mut before = None;
        for row in rows {
            if Some(row) == track {
                before = Some(line.len());
            }
            let combining = source.materialise(row, cells);
            let folds = source.wraps(row);
            let length = if folds {
                // A double-width character that did not fit left a spacer in
                // the last column, which is the row's and not the line's.
                let leading_spacer = cells
                    .last()
                    .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER))
                    && !cells
                        .len()
                        .checked_sub(2)
                        .is_some_and(|at| cells[at].flags.contains(CellFlags::WIDE));
                cells.len() - usize::from(leading_spacer)
            } else {
                cells
                    .iter()
                    .rposition(|cell| *cell != blank)
                    .map_or(0, |at| at + 1)
            };
            let base = line.len();
            line.extend(cells[..length].iter().map(|cell| SnapshotCell {
                flags: cell.flags.without(CellFlags::WRAPLINE),
                ..*cell
            }));
            marks.extend(
                combining
                    .iter()
                    .filter(|marks| marks.column < length)
                    .map(|marks| RowCombining {
                        column: base + marks.column,
                        characters: marks.characters.clone(),
                    }),
            );
        }
        self.push_line(line, &mut marks, columns, blank);
        before
    }

    /// Lays out the line on `rows` of `source` from its text and its runs,
    /// which [`Self::is_plain`] has said hold no character wider or narrower
    /// than a cell: the same rows [`Self::push_line_by_cells`] lays out, cut
    /// where the columns end rather than walked a cell at a time.
    fn push_plain_line(
        &mut self,
        source: &BlockRows,
        rows: Range<usize>,
        columns: usize,
        blank: SnapshotCell,
        track: Option<usize>,
        scratch: &mut Rewrap,
    ) -> Option<usize> {
        let Rewrap {
            text,
            runs,
            offsets,
            ..
        } = scratch;
        text.clear();
        runs.clear();
        let mut cells = 0;
        let mut before = None;
        let last = rows.end - 1;
        for row in rows {
            if Some(row) == track {
                before = Some(cells);
            }
            let row_text = source.text(row);
            let row_runs = source.runs(row).unwrap_or_default();
            let printed = if row_text.is_ascii() {
                row_text.len()
            } else {
                row_text.chars().count()
            };
            // Every cell of a row the line goes on past is the line's; the
            // last row's end where nothing past is printed or painted.
            let length = if row < last || source.wraps(row) {
                source.columns()
            } else {
                row_runs
                    .iter()
                    .rev()
                    .find(|run| (run.foreground, run.background, run.flags) != style_of(&blank))
                    .map_or(0, |run| usize::from(run.start) + usize::from(run.len))
                    .max(printed)
            };
            text.push_str(row_text);
            text.extend(std::iter::repeat_n(' ', length.saturating_sub(printed)));
            for run in row_runs {
                let start = usize::from(run.start);
                let end = (start + usize::from(run.len)).min(length);
                if start >= end {
                    break;
                }
                let style = (
                    run.foreground,
                    run.background,
                    run.flags.without(CellFlags::WRAPLINE),
                );
                // Two runs painted alike are joined where they are pushed.
                runs.push((cells + start, end - start, style));
            }
            cells += length;
        }

        // Where each cell's character starts in `text`, which is one byte a
        // cell for a line of ASCII and needs counting for any other.
        let ascii = text.is_ascii();
        if !ascii {
            offsets.clear();
            offsets.extend(text.char_indices().map(|(at, _)| at));
            offsets.push(text.len());
        }
        let byte = |cell: usize| if ascii { cell } else { offsets[cell] };

        let blank_style = style_of(&blank);
        let mut from = 0;
        let mut run = 0;
        loop {
            let to = (from + columns).min(cells);
            let folds = to < cells;
            if self.text_starts.is_empty() {
                self.text_starts.push(0);
                self.run_starts.push(0);
                self.combining_starts.push(0);
            }
            self.text
                .push_str(text[byte(from)..byte(to)].trim_end_matches(' '));
            let row_start = self.runs.len();
            while run < runs.len() && runs[run].0 + runs[run].1 <= from {
                run += 1;
            }
            let mut next = run;
            while next < runs.len() && runs[next].0 < to {
                let (at, len, style) = runs[next];
                let (start, end) = (at.max(from), (at + len).min(to));
                self.push_styled(row_start, start - from, end - start, style);
                next += 1;
            }
            if folds {
                // The fold is the last cell's, and only its.
                let last = self.runs.last_mut().expect("a folded row has cells");
                let style = (
                    last.foreground,
                    last.background,
                    last.flags | CellFlags::WRAPLINE,
                );
                if last.len > 1 {
                    last.len -= 1;
                    let start = usize::from(last.start + last.len);
                    self.runs.push(run_of(start, 1, style));
                } else {
                    last.flags = style.2;
                }
            } else {
                self.push_styled(row_start, to - from, columns - (to - from), blank_style);
            }
            self.text_starts.push(self.text.len() as u32);
            self.run_starts.push(self.runs.len() as u32);
            self.combining_starts.push(self.combining.len() as u32);
            if !folds {
                break;
            }
            from = to;
        }
        before
    }

    /// Paints `count` cells from `column` of the row whose runs start at
    /// `row_start` in `style`, onto the row's last run when that one is
    /// painted the same way.
    fn push_styled(&mut self, row_start: usize, column: usize, count: usize, style: Style) {
        if count == 0 {
            return;
        }
        let (foreground, background, flags) = style;
        self.push_cells(
            row_start,
            column,
            &SnapshotCell {
                c: ' ',
                foreground,
                background,
                flags,
            },
            count,
        );
    }

    /// Appends one line of cells, folded over as many rows of `columns` as it
    /// takes: see [`Self::rewrapped_tracking`]. `marks` are the line's zero-width
    /// characters, at cells of the line and in order, and it is left empty.
    fn push_line(
        &mut self,
        line: &[SnapshotCell],
        marks: &mut Vec<RowCombining>,
        columns: usize,
        blank: SnapshotCell,
    ) {
        let mut row = Vec::with_capacity(columns);
        let mut row_marks = Vec::new();
        let mut pending = marks.drain(..).peekable();
        let mut at = 0;
        while at < line.len() {
            let wide = line[at].flags.contains(CellFlags::WIDE) && columns > 1;
            // The cell, and the spacer that is a wide one's second column.
            let take = if wide && at + 1 < line.len() { 2 } else { 1 };
            if row.len() + take > columns {
                if wide && row.len() < columns {
                    // Where the grid leaves a spacer for a wide character
                    // that starts the next row.
                    row.push(SnapshotCell {
                        flags: CellFlags::WIDE_SPACER,
                        ..blank
                    });
                }
                row.resize(columns, blank);
                if let Some(last) = row.last_mut() {
                    last.flags |= CellFlags::WRAPLINE;
                }
                self.push_row(&row, &mut row_marks);
                row.clear();
            }
            for (cell, content) in line.iter().enumerate().skip(at).take(take) {
                while let Some(marked) = pending.next_if(|marks| marks.column <= cell) {
                    if marked.column == cell {
                        row_marks.push(RowCombining {
                            column: row.len(),
                            characters: marked.characters,
                        });
                    }
                }
                row.push(*content);
            }
            at += take;
        }
        row.resize(columns, blank);
        self.push_row(&row, &mut row_marks);
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
    pub(crate) fn with_columns(columns: usize) -> Self {
        Self {
            columns: u16::try_from(columns).unwrap_or(u16::MAX),
            ..Self::default()
        }
    }

    /// One row of plain text at the grid's default colours, for a block
    /// whose rows the grid no longer had.
    ///
    /// `clear` wipes the screen the command line was on before the block
    /// closes, and a harvest then copies out nothing. The command itself is
    /// known — it was read when the shell accepted it — so the block keeps
    /// that as its one row rather than being a gap with chrome around it.
    /// Plain, because the prompt that stood before it is gone with the rest.
    pub(crate) fn of_line(columns: usize, text: &str, palette: &Palette) -> Self {
        use unicode_width::UnicodeWidthChar as _;

        let columns = columns.max(1);
        let mut rows = Self::with_columns(columns);
        let blank = SnapshotCell {
            c: ' ',
            foreground: palette.foreground,
            background: palette.background,
            flags: CellFlags::NONE,
        };
        let mut cells = vec![blank; columns];
        let mut combining: Vec<RowCombining> = Vec::new();
        // Laid out the way the grid lays out what a shell echoes, so the row
        // is the command as it was drawn: a wide character takes two cells,
        // the second a spacer; a zero-width one stacks on the character
        // before it; and a row wraps at the grid's width in *cells*. One
        // character a cell drew `日本語` overlapping itself and wrapped a long
        // command at twice the width it had.
        let mut column = 0;
        let mut last = None;
        let mut pending = false;
        for character in text.chars() {
            let width = character.width().unwrap_or(1);
            if width == 0 {
                if let Some(at) = last {
                    match combining.last_mut() {
                        Some(marks) if marks.column == at => {
                            let mut stacked = marks.characters.to_vec();
                            stacked.push(character);
                            marks.characters = stacked.into();
                        }
                        _ => combining.push(RowCombining {
                            column: at,
                            characters: Box::new([character]),
                        }),
                    }
                }
                continue;
            }
            let wide = width == 2 && columns > 1;
            if column + if wide { 2 } else { 1 } > columns {
                // A wide character with one cell left before the edge leaves
                // that cell as a spacer and starts the next row, as the grid
                // does.
                if wide && column < columns {
                    cells[column].flags = CellFlags::WIDE_SPACER;
                }
                // Marked as folded, the way the grid marks the last cell of a
                // row it wrapped, so a copy of the command is one line again.
                cells[columns - 1].flags |= CellFlags::WRAPLINE;
                rows.push_row(&cells, &mut combining);
                cells.fill(blank);
                column = 0;
            }
            cells[column].c = character;
            if wide {
                cells[column].flags = CellFlags::WIDE;
                cells[column + 1].flags = CellFlags::WIDE_SPACER;
            }
            last = Some(column);
            column += if wide { 2 } else { 1 };
            pending = true;
        }
        if pending {
            rows.push_row(&cells, &mut combining);
        }
        rows
    }

    /// Appends one row: its cells in order, and the zero-width characters
    /// found on any of them.
    ///
    /// Runs are merged as they are pushed, so a row painted in one style
    /// arrives as one run however wide the grid is.
    fn push_row(&mut self, cells: &[SnapshotCell], combining: &mut Vec<RowCombining>) {
        self.push_row_and_blanks(cells, combining, None);
    }

    /// [`Self::push_row`], with `blanks` more cells after `cells`, every one
    /// of them `blank`: what a grid row nothing was printed into the end of
    /// costs to store without being read a cell at a time.
    fn push_row_and_blanks(
        &mut self,
        cells: &[SnapshotCell],
        combining: &mut Vec<RowCombining>,
        blanks: Option<(SnapshotCell, usize)>,
    ) {
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

        let row_start = self.runs.len();
        for (column, cell) in cells.iter().enumerate() {
            self.push_cells(row_start, column, cell, 1);
        }
        if let Some((blank, count)) = blanks.filter(|(_, count)| *count > 0) {
            self.push_cells(row_start, cells.len(), &blank, count);
        }

        self.combining.append(combining);
        self.text_starts.push(self.text.len() as u32);
        self.run_starts.push(self.runs.len() as u32);
        self.combining_starts.push(self.combining.len() as u32);
    }

    /// Paints `count` cells from `column` of the row whose runs start at
    /// `row_start` the way `cell` is painted, onto the row's last run when
    /// that one is painted the same way.
    fn push_cells(&mut self, row_start: usize, column: usize, cell: &SnapshotCell, count: usize) {
        let extends = self.runs.len() > row_start
            && self.runs.last().is_some_and(|run| {
                usize::from(run.start) + usize::from(run.len) == column
                    && run.foreground == cell.foreground
                    && run.background == cell.background
                    && run.flags == cell.flags
            });
        match self.runs.last_mut() {
            Some(run) if extends => run.len += count as u16,
            _ => self.runs.push(StyleRun {
                start: column as u16,
                len: count as u16,
                foreground: cell.foreground,
                background: cell.background,
                flags: cell.flags,
            }),
        }
    }

    /// Gives back what the store's allocations hold beyond its rows, for a
    /// store that will not grow again.
    pub(crate) fn shrink_to_fit(&mut self) {
        self.text.shrink_to_fit();
        self.text_starts.shrink_to_fit();
        self.runs.shrink_to_fit();
        self.run_starts.shrink_to_fit();
        self.combining.shrink_to_fit();
        self.combining_starts.shrink_to_fit();
    }
}

/// How a cell is painted: its two colours and its flags.
type Style = (Rgb, Rgb, CellFlags);

/// How `cell` is painted.
fn style_of(cell: &SnapshotCell) -> Style {
    (cell.foreground, cell.background, cell.flags)
}

/// A run of `len` cells from `start`, painted in `style`.
fn run_of(start: usize, len: usize, (foreground, background, flags): Style) -> StyleRun {
    StyleRun {
        start: start as u16,
        len: len as u16,
        foreground,
        background,
        flags,
    }
}

/// What laying rows out again reuses from one line to the next.
#[derive(Default)]
struct Rewrap {
    /// One row's cells, for a line laid out a cell at a time.
    cells: Vec<SnapshotCell>,
    /// The line's cells, laid out a cell at a time.
    line: Vec<SnapshotCell>,
    /// The line's characters, laid out a run at a time.
    text: String,
    /// The line's runs: where each starts in the line, how long, its style.
    runs: Vec<(usize, usize, Style)>,
    /// Where each of the line's cells starts in `text`, when it is not ASCII.
    offsets: Vec<usize>,
}

/// How many of `row`'s cells come before the ones nothing was printed into:
/// the cells after the last one that is not `Cell::default()`, which is what
/// a grid erases to. Not `Cell::is_empty`, which takes a tab, and bold.
///
/// Eight cells at a time from the end, with nothing to stop on inside the
/// eight: the blank end of a wide row is most of what a harvest reads.
fn printed(row: &[alacritty_terminal::term::cell::Cell]) -> usize {
    use alacritty_terminal::vte::ansi::{Color, NamedColor};

    let untouched = |cell: &alacritty_terminal::term::cell::Cell| {
        (cell.c == ' ')
            & cell.flags.is_empty()
            & matches!(cell.bg, Color::Named(NamedColor::Background))
            & matches!(cell.fg, Color::Named(NamedColor::Foreground))
            & cell.extra.is_none()
    };
    let mut end = row.len();
    while end >= 8
        && row[end - 8..end]
            .iter()
            .fold(true, |all, cell| all & untouched(cell))
    {
        end -= 8;
    }
    row[..end]
        .iter()
        .rposition(|cell| !untouched(cell))
        .map_or(0, |at| at + 1)
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
    let mut rows = BlockRows::with_columns(term.grid().columns());
    harvest_into(term, palette, top, bottom, &mut rows);
    rows
}

/// [`harvest`], onto the end of `rows`, which must be as wide as the grid.
///
/// Read the way the open block's rows are cut out of the grid a slice of
/// output at a time, so it is cheap where most of a row is: the cells after
/// the last one anything was printed into are counted rather than read, and
/// a cell painted the way the one before it was is not resolved again.
pub(crate) fn harvest_into<T>(
    term: &Term<T>,
    palette: &Palette,
    top: i32,
    bottom: i32,
    rows: &mut BlockRows,
) {
    use alacritty_terminal::term::cell::Cell;

    let grid = term.grid();
    let columns = grid.columns();
    debug_assert!(rows.is_empty() || rows.columns() == columns);
    let oldest = -(grid.history_size() as i32);
    let newest = grid.screen_lines() as i32 - 1;
    let (first, last) = (top.max(oldest), bottom.min(newest));
    if first > last {
        return;
    }

    let overrides = term.colors();
    let default = Cell::default();
    let blank = snapshot::convert(&default, palette, overrides);
    // A cell nothing was printed into: what `Cell::default` is, as a grid
    // erases to it. `Cell::is_empty` is not that — it takes a tab, and bold.
    for line in first..=last {
        let row = &grid[Line(line)][..];
        let printed = printed(row);
        if rows.text_starts.is_empty() {
            rows.text_starts.push(0);
            rows.run_starts.push(0);
            rows.combining_starts.push(0);
        }
        // Straight into the store: a run is grown a cell at a time while the
        // cells are painted alike, and a cell is resolved through the
        // palette only where the painting changes. What `push_row` would
        // make of the same cells, without a row of them made first.
        let row_start = rows.runs.len();
        let text_start = rows.text.len();
        let mut column = 0;
        while column < printed {
            // The cells from here painted as this one is: one resolve, one
            // run, and their characters.
            let first = &row[column];
            let key = (first.fg, first.bg, first.flags);
            let mut end = column + 1;
            let mut extra = first.extra.is_some();
            rows.text.push(first.c);
            for cell in &row[end..printed] {
                if (cell.fg, cell.bg, cell.flags) != key {
                    break;
                }
                rows.text.push(cell.c);
                extra |= cell.extra.is_some();
                end += 1;
            }
            let converted = snapshot::convert(first, palette, overrides);
            rows.push_cells(row_start, column, &converted, end - column);
            if extra {
                for (at, cell) in row[column..end].iter().enumerate() {
                    if let Some(marks) = cell.zerowidth().filter(|marks| !marks.is_empty()) {
                        rows.combining.push(RowCombining {
                            column: column + at,
                            characters: marks.into(),
                        });
                    }
                }
            }
            column = end;
        }
        let kept = rows.text[text_start..].trim_end_matches(' ').len();
        rows.text.truncate(text_start + kept);
        if printed < columns {
            rows.push_cells(row_start, printed, &blank, columns - printed);
        }
        rows.text_starts.push(rows.text.len() as u32);
        rows.run_starts.push(rows.runs.len() as u32);
        rows.combining_starts.push(rows.combining.len() as u32);
    }
}

#[cfg(test)]
#[path = "harvest_tests.rs"]
mod tests;
