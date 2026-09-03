//! The contract between the emulator and the renderer.
//!
//! A [`Snapshot`] is a flat, owned picture of the visible grid: one
//! [`SnapshotCell`] per column per row, every colour already resolved to
//! concrete RGB. The renderer holds one for as long as it likes and paints from
//! it without touching — and so without locking — the emulator, which is the
//! whole reason this type exists rather than the renderer walking the grid
//! itself.
//!
//! Two properties matter more than anything else here:
//!
//! * **Nothing left to look up.** A palette index, a "default foreground", a
//!   dim or inverse attribute — all of it is applied while the snapshot is
//!   built. The renderer reads `cell.foreground` and draws it. It never needs a
//!   theme, and it can never disagree with the emulator about what a colour
//!   means.
//! * **A revision that only moves on a real change.** The emulator hands back
//!   the same `Arc<Snapshot>`, with the same [`Snapshot::revision`], until the
//!   drawn content actually differs. A frame that changed nothing costs the UI
//!   an integer comparison.
//!
//! Cells are stored row-major in one `Vec`, so a 200x50 grid is a single 120 KB
//! allocation the renderer walks in order — [`Snapshot::iter_rows`] hands out
//! the rows as slices.

use std::ops::{BitOr, BitOrAssign};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::{self, Colors};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color, CursorShape as VteCursorShape, NamedColor, Rgb as VteRgb,
};

/// How much of a colour survives the dim attribute.
///
/// The ANSI dim attribute has no defined intensity, so terminals pick one; two
/// thirds is the conventional choice and the one this crate makes when the
/// palette has no dedicated dim entry.
const DIM_FACTOR: f32 = 0.66;

/// A colour the renderer can use without translating anything.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// A colour from its three channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// A colour from a `0xRRGGBB` literal, which is how palettes are written.
    pub const fn hex(value: u32) -> Self {
        Self::new((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }

    /// The same hue at a fraction of the intensity, used for the dim attribute.
    pub fn scaled(self, factor: f32) -> Self {
        let scale = |channel: u8| (f32::from(channel) * factor).clamp(0., 255.) as u8;
        Self::new(scale(self.r), scale(self.g), scale(self.b))
    }
}

impl From<VteRgb> for Rgb {
    fn from(value: VteRgb) -> Self {
        Self::new(value.r, value.g, value.b)
    }
}

impl From<Rgb> for VteRgb {
    fn from(value: Rgb) -> Self {
        Self {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}

/// The attributes of a cell that survive colour resolution.
///
/// Inverse, dim and hidden are deliberately absent: they only ever affect which
/// colours a cell is drawn in, and the snapshot has already applied them. What
/// is left is what the renderer must act on — a different font face, an extra
/// line, or a column it must not draw into.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellFlags(u8);

impl CellFlags {
    /// No attributes.
    pub const NONE: Self = Self(0);
    /// Draw with a bold face.
    pub const BOLD: Self = Self(1 << 0);
    /// Draw with an italic face.
    pub const ITALIC: Self = Self(1 << 1);
    /// Draw a line under the cell. Alacritty's four underline styles all
    /// collapse to this one, because a single rule is all the renderer draws.
    pub const UNDERLINE: Self = Self(1 << 2);
    /// Draw a line through the cell.
    pub const STRIKEOUT: Self = Self(1 << 3);
    /// A double-width character: its glyph spans this column and the next.
    pub const WIDE: Self = Self(1 << 4);
    /// The second column of a double-width character. Its background belongs to
    /// the pair; its character must not be drawn.
    pub const WIDE_SPACER: Self = Self(1 << 5);

    /// Whether every flag in `other` is set here.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no flag at all is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The raw bits, for a renderer that wants to key a cache on them.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl BitOr for CellFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for CellFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// The zero-width characters stacked on top of one cell.
///
/// A combining accent, a Devanagari virama, a variation selector, a
/// zero-width joiner: characters that occupy no column of their own and belong
/// to the character before them. They are kept beside the grid rather than in
/// [`SnapshotCell`] so that a cell stays twelve bytes and stays [`Copy`] —
/// they are rare enough that most snapshots carry none at all, and a screen
/// that does carry some carries a handful.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CellCombining {
    /// Which cell they belong to: an index into [`Snapshot::cells`].
    pub cell: usize,
    /// The characters, in the order the child sent them.
    pub characters: Box<[char]>,
}

/// One drawn cell: a character and the two colours it is drawn in.
///
/// Zero-width characters stacked on this cell are not here; see
/// [`Snapshot::zerowidth`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SnapshotCell {
    /// The character occupying the cell. A blank cell holds a space, never a
    /// `\0`, so a renderer can draw every cell unconditionally.
    pub c: char,
    /// The resolved colour of the character.
    pub foreground: Rgb,
    /// The resolved colour behind it.
    pub background: Rgb,
    /// What else to do when drawing it.
    pub flags: CellFlags,
}

/// The shape the child process asked the cursor to take.
///
/// Hidden is not a shape: a hidden cursor is [`Snapshot::cursor`] being `None`,
/// which is also what a cursor scrolled out of the viewport looks like, so the
/// renderer has exactly one case to handle.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum CursorShape {
    /// A filled cell.
    Block,
    /// A rule along the bottom of the cell.
    Underline,
    /// A rule down the left of the cell.
    Beam,
    /// An outlined cell, conventionally drawn when the terminal is unfocused.
    HollowBlock,
}

impl CursorShape {
    /// The renderable shapes, dropping alacritty's hidden state.
    fn from_vte(shape: VteCursorShape) -> Option<Self> {
        match shape {
            VteCursorShape::Block => Some(Self::Block),
            VteCursorShape::Underline => Some(Self::Underline),
            VteCursorShape::Beam => Some(Self::Beam),
            VteCursorShape::HollowBlock => Some(Self::HollowBlock),
            VteCursorShape::Hidden => None,
        }
    }
}

/// Where the cursor is and what it looks like, in viewport coordinates.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cursor {
    /// Row within the viewport, counting down from the top.
    pub row: usize,
    /// Column within the viewport.
    pub column: usize,
    /// What to draw.
    pub shape: CursorShape,
    /// The resolved cursor colour.
    pub color: Rgb,
}

/// The size of the grid, in cells and — where the caller knows it — in pixels.
///
/// The pixel dimensions are the renderer's business, but they belong here
/// because they travel to the same two places the cell counts do: the pty, so
/// full-screen programs can size their own graphics, and the emulator, which
/// answers the child's text-area-size queries with them. Zero means unknown,
/// which is what every terminal reports until it has laid out a font.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerminalSize {
    /// Columns of text.
    pub columns: u16,
    /// Rows of text.
    pub rows: u16,
    /// Width of one cell in pixels, or zero if unknown.
    pub cell_width: u16,
    /// Height of one cell in pixels, or zero if unknown.
    pub cell_height: u16,
}

impl TerminalSize {
    /// The smallest grid a terminal may be. Anything narrower cannot hold a
    /// double-width character, and alacritty's grid refuses to go below it.
    const MIN_COLUMNS: u16 = 2;
    /// The smallest grid a terminal may be, vertically.
    const MIN_ROWS: u16 = 1;

    /// A grid of the given size whose pixel dimensions are not yet known.
    pub const fn new(columns: u16, rows: u16) -> Self {
        Self {
            columns,
            rows,
            cell_width: 0,
            cell_height: 0,
        }
    }

    /// The same grid with the pixel size of one cell filled in.
    pub const fn with_cell_size(self, cell_width: u16, cell_height: u16) -> Self {
        Self {
            cell_width,
            cell_height,
            ..self
        }
    }
}

impl Default for TerminalSize {
    /// The size every terminal starts at when nobody has said otherwise.
    fn default() -> Self {
        Self::new(80, 24)
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        usize::from(self.rows.max(Self::MIN_ROWS))
    }

    fn columns(&self) -> usize {
        usize::from(self.columns.max(Self::MIN_COLUMNS))
    }
}

/// The colours a terminal draws with before the child process overrides any.
///
/// The emulator resolves every cell through this, so replacing it is how the
/// app applies a theme. Overrides the child sets with OSC 4, 10, 11 and 12 take
/// precedence over the corresponding entries here, which is what those escapes
/// are for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The 256 indexed colours: 16 ANSI, a 6x6x6 cube, then a grey ramp.
    pub ansi: [Rgb; 256],
    /// Default text colour.
    pub foreground: Rgb,
    /// Default background colour, also used for the untouched part of the grid.
    pub background: Rgb,
    /// The cursor's colour.
    pub cursor: Rgb,
    /// Text colour for the bright-foreground named colour.
    pub bright_foreground: Rgb,
    /// Text colour for the dim-foreground named colour.
    pub dim_foreground: Rgb,
}

/// The xterm defaults for the sixteen ANSI colours.
const ANSI_DEFAULTS: [Rgb; 16] = [
    Rgb::hex(0x000000),
    Rgb::hex(0xcd0000),
    Rgb::hex(0x00cd00),
    Rgb::hex(0xcdcd00),
    Rgb::hex(0x0000ee),
    Rgb::hex(0xcd00cd),
    Rgb::hex(0x00cdcd),
    Rgb::hex(0xe5e5e5),
    Rgb::hex(0x7f7f7f),
    Rgb::hex(0xff0000),
    Rgb::hex(0x00ff00),
    Rgb::hex(0xffff00),
    Rgb::hex(0x5c5cff),
    Rgb::hex(0xff00ff),
    Rgb::hex(0x00ffff),
    Rgb::hex(0xffffff),
];

impl Default for Palette {
    fn default() -> Self {
        // The 216-colour cube steps through these six levels per channel, and
        // the grey ramp runs 8, 18, ... 238. Both are fixed by the xterm 256
        // colour scheme; every terminal agrees on them, so they are generated
        // rather than written out.
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

        let mut ansi = [Rgb::default(); 256];
        ansi[..ANSI_DEFAULTS.len()].copy_from_slice(&ANSI_DEFAULTS);
        for (index, entry) in ansi[16..232].iter_mut().enumerate() {
            *entry = Rgb::new(
                LEVELS[index / 36],
                LEVELS[(index / 6) % 6],
                LEVELS[index % 6],
            );
        }
        for (index, entry) in ansi[232..].iter_mut().enumerate() {
            let level = 8 + index as u8 * 10;
            *entry = Rgb::new(level, level, level);
        }

        let foreground = Rgb::hex(0xe5e5e5);
        Self {
            ansi,
            foreground,
            background: Rgb::hex(0x111111),
            cursor: foreground,
            bright_foreground: Rgb::hex(0xffffff),
            dim_foreground: foreground.scaled(DIM_FACTOR),
        }
    }
}

// Named colours are addressed by their discriminant in the same space as the
// indexed ones, so resolution is a single `match` over a `usize`. Pattern
// position needs constants rather than `as` casts.
const FOREGROUND: usize = NamedColor::Foreground as usize;
const BACKGROUND: usize = NamedColor::Background as usize;
const CURSOR: usize = NamedColor::Cursor as usize;
const DIM_BLACK: usize = NamedColor::DimBlack as usize;
const DIM_WHITE: usize = NamedColor::DimWhite as usize;
const BRIGHT_FOREGROUND: usize = NamedColor::BrightForeground as usize;
const DIM_FOREGROUND: usize = NamedColor::DimForeground as usize;

impl Palette {
    /// The concrete colour at a terminal colour index, honouring any override
    /// the child process has installed with OSC 4, 10, 11 or 12.
    ///
    /// Indices below 256 are the indexed palette; the rest are the named slots
    /// alacritty keeps above it (foreground, background, cursor, and the dim
    /// and bright variants).
    ///
    /// Crate-private because `overrides` belongs to a running emulator: by the
    /// time a snapshot reaches the renderer there is nothing left to resolve.
    pub(crate) fn color_at(&self, index: usize, overrides: &Colors) -> Rgb {
        if index < color::COUNT
            && let Some(rgb) = overrides[index]
        {
            return rgb.into();
        }

        match index {
            FOREGROUND => self.foreground,
            BACKGROUND => self.background,
            CURSOR => self.cursor,
            BRIGHT_FOREGROUND => self.bright_foreground,
            DIM_FOREGROUND => self.dim_foreground,
            // No palette carries the eight dim ANSI colours, so they are the
            // ordinary ones held back.
            index @ DIM_BLACK..=DIM_WHITE => self.ansi[index - DIM_BLACK].scaled(DIM_FACTOR),
            index => self.ansi.get(index).copied().unwrap_or(self.foreground),
        }
    }

    /// The concrete colour of one of the named slots.
    pub(crate) fn named(&self, name: NamedColor, overrides: &Colors) -> Rgb {
        self.color_at(name as usize, overrides)
    }

    /// The concrete colour a cell asked for, whatever form it asked in.
    fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb.into(),
            Color::Named(name) => self.named(name, overrides),
            Color::Indexed(index) => self.color_at(usize::from(index), overrides),
        }
    }

    /// The same colour held back for the dim attribute.
    ///
    /// A named or low indexed colour resolves through the palette's own dim
    /// slot — which is what makes [`Palette::dim_foreground`] and an OSC 4
    /// override of index 256..268 mean anything, and what alacritty does — and
    /// anything else is simply held back by [`DIM_FACTOR`], because a 24-bit
    /// colour and a cube entry have no dim counterpart to look up.
    fn resolve_dim(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Named(name) => self.named(name.to_dim(), overrides),
            Color::Indexed(index @ 0..8) => {
                self.color_at(DIM_BLACK + usize::from(index), overrides)
            }
            Color::Indexed(index @ 8..16) => self.color_at(usize::from(index) - 8, overrides),
            color => self.resolve(color, overrides).scaled(DIM_FACTOR),
        }
    }
}

/// The visible grid, and everything needed to draw it.
///
/// Compared with `==`, two snapshots are equal only when they are the same
/// version; use [`Snapshot::same_content`] to ask whether they would be drawn
/// identically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Bumped every time the drawn content changes, and never otherwise. Two
    /// snapshots with the same revision came from the same emulator and are
    /// identical.
    pub revision: u64,
    /// Cells per row.
    pub columns: usize,
    /// Rows in the viewport.
    pub rows: usize,
    /// `rows * columns` cells, row-major, top-left first.
    pub cells: Vec<SnapshotCell>,
    /// The zero-width characters that belong to cells, ordered by cell index
    /// and empty for the overwhelming majority of screens. Read it with
    /// [`Snapshot::zerowidth`].
    pub combining: Vec<CellCombining>,
    /// The cursor, or `None` when it is hidden or scrolled out of view.
    pub cursor: Option<Cursor>,
    /// The resolved default text colour, for anything the renderer draws around
    /// the grid.
    pub foreground: Rgb,
    /// The resolved default background, which the renderer should clear to.
    pub background: Rgb,
    /// How many lines above the bottom the viewport is showing; zero when the
    /// terminal is scrolled to the live output.
    pub display_offset: usize,
    /// How many lines of scrollback exist above the viewport.
    pub history_len: usize,
    /// Whether the alternate screen is active. Full-screen programs run here,
    /// and there is no scrollback while it is.
    pub alt_screen: bool,
    /// The title the child process last asked for, if any.
    pub title: Option<String>,
}

impl Snapshot {
    /// The cells of one row. Panics if `row` is outside the viewport.
    pub fn row(&self, row: usize) -> &[SnapshotCell] {
        let columns = self.columns.max(1);
        let start = row * columns;
        &self.cells[start..start + columns]
    }

    /// One cell, or `None` outside the viewport.
    pub fn cell(&self, row: usize, column: usize) -> Option<&SnapshotCell> {
        if column >= self.columns || row >= self.rows {
            return None;
        }
        self.cells.get(row * self.columns.max(1) + column)
    }

    /// Every row, top to bottom, as a slice each.
    pub fn iter_rows(&self) -> impl Iterator<Item = &[SnapshotCell]> {
        self.cells.chunks_exact(self.columns.max(1))
    }

    /// Whether any cell carries zero-width characters.
    ///
    /// The answer is no for nearly every screen, which is why it is worth
    /// asking before walking cells looking for them.
    pub fn has_combining(&self) -> bool {
        !self.combining.is_empty()
    }

    /// The zero-width characters stacked on one cell — an accent, a virama, a
    /// variation selector — which are drawn over the cell's own character and
    /// belong to it when the screen is copied.
    pub fn zerowidth(&self, row: usize, column: usize) -> &[char] {
        if self.combining.is_empty() || column >= self.columns || row >= self.rows {
            return &[];
        }
        let cell = row * self.columns.max(1) + column;
        match self
            .combining
            .binary_search_by_key(&cell, |entry| entry.cell)
        {
            Ok(at) => &self.combining[at].characters,
            Err(_) => &[],
        }
    }

    /// Whether the two snapshots would be drawn identically — everything except
    /// the revision they carry.
    pub fn same_content(&self, other: &Self) -> bool {
        self.columns == other.columns
            && self.rows == other.rows
            && self.cursor == other.cursor
            && self.foreground == other.foreground
            && self.background == other.background
            && self.display_offset == other.display_offset
            && self.history_len == other.history_len
            && self.alt_screen == other.alt_screen
            && self.title == other.title
            && self.cells == other.cells
            && self.combining == other.combining
    }

    /// The visible text, one line per row with trailing blanks removed.
    ///
    /// This is for tests, logs and "copy the screen"; it drops every colour and
    /// attribute, so it is not a rendering path.
    pub fn text(&self) -> String {
        let mut text = String::with_capacity(self.cells.len() + self.rows);
        for (row, cells) in self.iter_rows().enumerate() {
            let end = cells
                .iter()
                .rposition(|cell| cell.c != ' ')
                .map_or(0, |at| at + 1);
            for (column, cell) in cells[..end].iter().enumerate() {
                // The trailing half of a double-width character has no
                // character of its own; its neighbour already contributed one.
                if cell.flags.contains(CellFlags::WIDE_SPACER) {
                    continue;
                }
                text.push(cell.c);
                text.extend(self.zerowidth(row, column));
            }
            text.push('\n');
        }
        text
    }
}

/// Turns one grid cell into a drawable one, applying every attribute that only
/// changes which colours are used.
fn convert(cell: &Cell, palette: &Palette, overrides: &Colors) -> SnapshotCell {
    let mut foreground = if cell.flags.contains(Flags::DIM) {
        palette.resolve_dim(cell.fg, overrides)
    } else {
        palette.resolve(cell.fg, overrides)
    };
    let mut background = palette.resolve(cell.bg, overrides);

    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut foreground, &mut background);
    }
    // Concealed text keeps its cell and its background; only the glyph goes.
    if cell.flags.contains(Flags::HIDDEN) {
        foreground = background;
    }

    let mut flags = CellFlags::NONE;
    if cell.flags.contains(Flags::BOLD) {
        flags |= CellFlags::BOLD;
    }
    if cell.flags.contains(Flags::ITALIC) {
        flags |= CellFlags::ITALIC;
    }
    if cell.flags.intersects(Flags::ALL_UNDERLINES) {
        flags |= CellFlags::UNDERLINE;
    }
    if cell.flags.contains(Flags::STRIKEOUT) {
        flags |= CellFlags::STRIKEOUT;
    }
    if cell.flags.contains(Flags::WIDE_CHAR) {
        flags |= CellFlags::WIDE;
    }
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
    {
        flags |= CellFlags::WIDE_SPACER;
    }

    SnapshotCell {
        c: cell.c,
        foreground,
        background,
        flags,
    }
}

/// Where the cursor should be drawn, in viewport coordinates.
fn cursor_of<T>(
    term: &Term<T>,
    palette: &Palette,
    rows: usize,
    display_offset: usize,
) -> Option<Cursor> {
    let shape = CursorShape::from_vte(term.cursor_style().shape)?;
    if !term.mode().contains(TermMode::SHOW_CURSOR) {
        return None;
    }

    let grid = term.grid();
    let mut point = grid.cursor.point;
    // A cursor sitting on the tail of a double-width character belongs to the
    // pair, and the pair is drawn from its first column.
    if point.column.0 > 0
        && point.column.0 < grid.columns()
        && grid[point].flags.contains(Flags::WIDE_CHAR_SPACER)
    {
        point.column -= 1;
    }

    // Scrolling back far enough puts the cursor above the viewport, where there
    // is nothing to draw.
    let row = usize::try_from(point.line.0 + display_offset as i32).ok()?;
    (row < rows).then(|| Cursor {
        row,
        column: point.column.0,
        shape,
        color: palette.named(NamedColor::Cursor, term.colors()),
    })
}

/// Builds the snapshot the renderer draws from the emulator's current state.
pub(crate) fn build<T>(
    term: &Term<T>,
    palette: &Palette,
    revision: u64,
    title: Option<&str>,
) -> Snapshot {
    let grid = term.grid();
    let overrides = term.colors();
    let columns = grid.columns();
    let rows = grid.screen_lines();
    let display_offset = grid.display_offset();

    let mut cells = Vec::with_capacity(columns * rows);
    // Nearly always empty, so it starts that way and only allocates for a
    // screen that actually has an accent on it.
    let mut combining = Vec::new();
    for row in 0..rows {
        // Viewport row 0 is `display_offset` lines above the live bottom line.
        let line = Line(row as i32 - display_offset as i32);
        for (column, cell) in grid[line][..].iter().enumerate() {
            // Pushed in cell order, which is what lets `zerowidth` binary
            // search rather than scan.
            if let Some(zerowidth) = cell.zerowidth().filter(|marks| !marks.is_empty()) {
                combining.push(CellCombining {
                    cell: row * columns + column,
                    characters: zerowidth.into(),
                });
            }
            cells.push(convert(cell, palette, overrides));
        }
    }
    debug_assert_eq!(columns * rows, cells.len());

    Snapshot {
        revision,
        columns,
        rows,
        cells,
        combining,
        cursor: cursor_of(term, palette, rows, display_offset),
        foreground: palette.named(NamedColor::Foreground, overrides),
        background: palette.named(NamedColor::Background, overrides),
        display_offset,
        history_len: grid.history_size(),
        alt_screen: term.mode().contains(TermMode::ALT_SCREEN),
        title: title.map(str::to_owned),
    }
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
