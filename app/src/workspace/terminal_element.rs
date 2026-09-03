//! The grid: a [`Snapshot`] painted as glyph records, one per cell.
//!
//! This is the element the renderer was designed around. Everywhere else in
//! Crook, text goes through the shaper: a string is measured, shaped into runs,
//! and painted from a [`Line`]. A terminal cannot work that way — a full screen
//! is thousands of cells, each with its own foreground, and re-shaping them
//! sixty times a second would be the whole frame. So the grid bypasses shaping
//! entirely: [`CellFont::glyph`] maps a character to a glyph id, the column
//! index times a fixed advance says where it goes, and the record goes straight
//! into the [`Scene`].
//!
//! Three things keep an idle screen close to free:
//!
//! * a blank cell draws no glyph, and a cell whose background is the grid's own
//!   draws no rectangle — an untouched screen is one rectangle in total;
//! * adjacent cells sharing a background become one rectangle, and so do
//!   adjacent underlines and strikeouts, so a coloured `ls` line and a
//!   full-width underlined line of `man` both cost a rectangle per run rather
//!   than per column;
//! * the snapshot arrives as an `Arc` the emulator only replaces when the drawn
//!   content changed, so a frame spent on an unchanged grid never happens.
//!
//! What the grid does *not* assume is that the monospace family covers what the
//! shell printed. A character it lacks — a spinner's braille, a powerline
//! separator, a CJK filename — is drawn from a face that has it, resolved once
//! per distinct character by [`CellFont::glyph`]. A blank column would be worse
//! than wrong: the row would be silently short, and copying the screen would
//! disagree with looking at it.
//!
//! # When a pane draws this rather than a list of blocks
//!
//! Three cases, and [`crate::pane_surface`] states all three: a full-screen
//! program on the alternate screen, a command that has been running on the
//! primary screen for longer than a blink, and a shell with no command marks
//! whose one open block has already grown past the top of the viewport. The
//! first two get the whole pane and every key; the third keeps its composer
//! and scrolls through the emulator's own history, exactly as every pane did
//! before blocks existed.
//!
//! **The pty is not resized here.** It is resized from the pane's rectangle,
//! by `body::PaneSizer`, because this element's box moves whenever the
//! composer appears or hides — see that type for the whole argument. What is
//! left here is the consequence: a snapshot measured for the grid the last
//! frame had is refreshed during layout rather than painted into the new box.
//!
//! # Selecting the output
//!
//! The grid is the other selectable surface in a pane. A press starts a
//! selection at the cell under it — a character, a word on the second click, a
//! line on the third, a block with Alt — a drag takes it out, and a drag that
//! leaves the top or the bottom edge scrolls the viewport under the pointer so
//! that a selection can run past the screen it began on.
//!
//! **None of the selection is kept here.** The cells belong to the emulator,
//! which is what keeps them anchored to their text while the shell prints more
//! underneath — see [`crook_terminal::selection`] — and the *gesture* belongs
//! to the workspace, because a press and the drag that answers it are separated
//! by every frame the pointer takes to move and this element is thrown away on
//! each of them. What is here is the arithmetic between a pixel and a cell, the
//! rectangles the highlight is drawn as, and the four rules that let go of a
//! selection: typing, clicking into the field, clicking somewhere else in the
//! output, and the pane closing. The fifth thing that could and must not is the
//! shell printing, which is why nothing in the paint path touches it.
//!
//! [`Line`]: crookui_core::text_layout::Line

use std::sync::Arc;

use crook_terminal::{
    CellFlags, CellSide, Cursor, CursorShape, Rgb, RowCombining, Snapshot, SnapshotCell,
    ViewportPoint,
};
use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::{DispatchedEvent, Event, Modifiers, MouseButton};
use crookui_core::geometry::{Color, Point, RectF, Vector2F, vec2f};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};
use crookui_core::scene::Scene;

use crate::clipboard::Clipboard;
use crate::pane_input::PaneInput;
use crate::pane_selection::PaneSelection;
use crate::pane_surface;
use crate::tab::PaneId;
use crate::terminal_font::{CellFont, CellMetrics};
use crate::terminal_model::TerminalHandle;
use crate::theme::theme;

use super::pane_output::{Keys, Output, Typed, selection_kind};

/// How thick the rules a cell can carry are, as a fraction of the font size.
///
/// The same ratio a shaped line underlines itself with, so a link underlined by
/// a shell and a label underlined by the UI are the same weight.
const RULE_THICKNESS_RATIO: f32 = 0.06;

/// How far below the baseline an underline sits, as a fraction of the space
/// between the baseline and the bottom of the cell.
const UNDERLINE_OFFSET_RATIO: f32 = 0.4;

/// Where a strikeout crosses the cell, measured up from the baseline as a
/// fraction of the font size.
const STRIKEOUT_HEIGHT_RATIO: f32 = 0.28;

/// How wide the beam cursor and the hollow block's outline are drawn.
const CURSOR_STROKE: f32 = 2.;

/// One pane's terminal grid.
pub struct TerminalElement {
    /// The grid to paint. Replaced during layout when the pane resized the pty
    /// under it, so a frame that changed the column count draws the new grid
    /// rather than the old one stretched over it.
    snapshot: Arc<Snapshot>,
    font: CellFont,

    /// The keyboard and the selection, which a block list answers exactly the
    /// same way. See [`Output`].
    output: Output,

    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl TerminalElement {
    /// A grid that paints `snapshot` and is driven by nothing.
    pub fn new(snapshot: Arc<Snapshot>, font: CellFont) -> Self {
        Self {
            snapshot,
            font,
            output: Output::detached(),
            size: None,
            origin: None,
        }
    }

    /// Attaches the terminal the snapshot came from, so the grid can be typed
    /// into as far as `keys` allows.
    pub fn with_terminal(mut self, handle: TerminalHandle, keys: Keys) -> Self {
        self.output = self.output.with_terminal(handle, keys);
        self
    }

    /// Attaches the field under this grid, whose line decides what Ctrl-D
    /// means.
    pub fn with_input(mut self, input: PaneInput) -> Self {
        self.output = self.output.with_input(input);
        self
    }

    /// Makes the output selectable: `gesture` is the press this pane has open,
    /// which outlives the frame, `clipboard` is where a copy goes, and `pane`
    /// is who the release is dispatched for.
    pub fn with_selection(
        mut self,
        pane: PaneId,
        gesture: PaneSelection,
        clipboard: Clipboard,
    ) -> Self {
        self.output = self.output.with_selection(pane, gesture, clipboard);
        self
    }

    /// The typed keystroke, if this pane is the one that should have it and
    /// the shell is the half of the pane it belongs to.
    fn type_key(&self, event: &Event, ctx: &mut EventContext) -> bool {
        self.output.type_key(event, self.snapshot.alt_screen, ctx) != Typed::Ignored
    }

    /// Starts a selection where a press landed.
    fn press(
        &self,
        position: Vector2F,
        click_count: u32,
        modifiers: Modifiers,
        ctx: &mut EventContext,
    ) -> bool {
        let Some((at, side)) = self.cell_at(position) else {
            return false;
        };
        self.output
            .press(at, side, selection_kind(click_count, modifiers.alt), ctx)
    }

    /// Drags the open end of the selection to the pointer, scrolling the
    /// viewport when the pointer has left the grid.
    fn drag(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
        // Deliberately not hit-tested: dragging *past* the pane is how a
        // selection is taken to the end of a line, and how it is taken past
        // the end of the screen. What keeps this pane's grid out of a drag
        // that began in the field, or in the pane beside it, is that only the
        // pane the press landed on has a gesture open.
        if !self.output.is_dragging() {
            return false;
        }
        let Some((at, side)) = self.cell_at(position) else {
            return false;
        };
        self.output.drag(at, side, self.autoscroll(position), ctx)
    }

    /// Ends the gesture, reporting whether this pane had one.
    fn release(&self) -> bool {
        self.output.release()
    }

    /// The cell of the viewport a window position lands on, and which half of
    /// it the pointer is on.
    ///
    /// Clamped into the grid rather than refused outside it, because a drag
    /// that has left the pane is still selecting: past the right edge means the
    /// end of the row, and past the bottom means the last row — and the row it
    /// clamps to is the one [`Self::autoscroll`] is about to scroll under the
    /// pointer.
    ///
    /// The answer is a cell of the *screen*, not of the text. Turning one into
    /// the other needs the display offset, which only the emulator has and
    /// which moves whenever the shell prints while the viewport is scrolled
    /// back — so that conversion happens under the terminal's own lock at the
    /// moment the press lands, and a selection dragged out four screens into
    /// the scrollback cannot end up on the live output.
    fn cell_at(&self, position: Vector2F) -> Option<(ViewportPoint, CellSide)> {
        let bounds = self.bounds()?;
        let metrics = self.font.metrics();
        let (fitting_columns, fitting_rows) = metrics.grid_for(bounds.width(), bounds.height());
        let columns = usize::from(fitting_columns).min(self.snapshot.columns);
        let rows = usize::from(fitting_rows).min(self.snapshot.rows);
        if columns == 0 || rows == 0 {
            return None;
        }

        let local = position - bounds.origin();
        let (column, side) = column_at(local.x(), metrics.width, columns);
        let row = (local.y() / metrics.height)
            .floor()
            .clamp(0., (rows - 1) as f32) as usize;
        Some((ViewportPoint::new(row, column), side))
    }

    /// How far to scroll before a drag lands, when the pointer has left the top
    /// or the bottom of the grid.
    ///
    /// Proportional to how far past the edge it is, in rows, and positive is
    /// back into history — the sense the wheel and the emulator both use. That
    /// is what makes a selection able to run past the screen it started on:
    /// each move of the pointer beyond the edge takes the viewport with it.
    /// Bounded by a screenful per event so that flinging the pointer at the
    /// bottom of the window does not skip the output it was dragging over.
    fn autoscroll(&self, position: Vector2F) -> i32 {
        let Some(bounds) = self.bounds() else {
            return 0;
        };
        let height = self.font.metrics().height;
        let past = if position.y() < bounds.min_y() {
            bounds.min_y() - position.y()
        } else if position.y() > bounds.max_y() {
            bounds.max_y() - position.y()
        } else {
            return 0;
        };

        let rows = self.snapshot.rows.max(1) as f32;
        let lines = (past / height).clamp(-rows, rows);
        // Away from zero: a pointer one pixel past the edge is already asking
        // for the next line, and having to travel a whole row's height before
        // anything happens would make the edge feel dead.
        if lines > 0. {
            lines.ceil() as i32
        } else {
            lines.floor() as i32
        }
    }

    /// Moves the viewport through the scrollback, if the wheel turned over this
    /// pane.
    fn scroll(&self, event: &Event) -> bool {
        let Event::ScrollWheel {
            position, delta, ..
        } = event
        else {
            return false;
        };
        if !self
            .bounds()
            .is_some_and(|bounds| bounds.contains_point(*position))
        {
            return false;
        }

        let Some(handle) = self.output.handle() else {
            return false;
        };
        let height = self.font.metrics().height;
        let lines = (delta.to_pixels(height).y() / height).round() as i32;
        if lines == 0 {
            return false;
        }
        // Positive is up the screen and back into history, which is the sense
        // both the wheel and the emulator use.
        handle.scroll_lines(lines);
        true
    }
}

impl Element for TerminalElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        let size = vec2f(
            bounded(constraint.max.x(), constraint.min.x()),
            bounded(constraint.max.y(), constraint.min.y()),
        );

        // **The pty is not resized here.** It is resized from the pane's own
        // rectangle, by `body::PaneSizer`, because this element's box moves
        // whenever the composer appears or hides and a `SIGWINCH` per such
        // frame is a storm at programs that handle them badly.
        //
        // What is left is the consequence: the pane resized the terminal
        // *before* this was laid out, so the snapshot this frame was built
        // with may have been measured for the grid the last frame had.
        // Painting it into the new box would draw a frame of the wrong width.
        if let Some(handle) = self.output.handle() {
            let (columns, rows) = self.font.metrics().grid_for(size.x(), size.y());
            let stale = usize::from(columns) != self.snapshot.columns
                || usize::from(rows) != self.snapshot.rows;
            if stale {
                self.snapshot = handle.snapshot();
            }
        }

        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        let size = self
            .size
            .expect("a grid was painted before it was laid out");

        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        // The cursor is filled only when the grid is where typing would go. On
        // the normal screen that is the field below, so the shell's own cursor
        // is drawn as the outline an unfocused terminal gets: two filled block
        // cursors in one pane say nothing about which of them is listening.
        let owns_caret = self.output.keys() == Keys::All
            && pane_surface::of(&self.snapshot, std::time::Instant::now()).output_owns_caret();
        paint_grid(
            &self.snapshot,
            &self.font,
            origin,
            size,
            owns_caret,
            ctx.scene,
        );
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        _: &AppContext,
    ) -> bool {
        let Some(z_index) = self.z_index() else {
            return false;
        };

        // **The rest of a gesture this grid already owns is not hit tested.**
        // A press opened it here; where the pointer has got to since is the
        // gesture, not a new one. Dragging *out* of the pane is how a selection
        // is taken to the end of a line and past the end of the screen, and the
        // button can perfectly well come up over the tabs panel or a popup —
        // painted in a later layer, so `at_z_index` would drop the release and
        // leave the gesture open for ever, and every unrelated drag afterwards
        // would rewrite this pane's selection. What keeps a neighbour's drag
        // out is [`PaneSelection`]: only the pane the press landed on has one.
        match event.raw_event() {
            Event::MouseDragged {
                button: MouseButton::Left,
                position,
                ..
            } => return self.drag(*position, ctx),
            Event::MouseUp {
                button: MouseButton::Left,
                ..
            } => return self.release(),
            _ => {}
        }

        // Keystrokes are never filtered by what is painted over them — they are
        // not about a place on screen — but a press and the wheel are, so that
        // a menu open over a pane is neither clicked nor scrolled through.
        let Some(event) = event.at_z_index(z_index, ctx) else {
            return false;
        };

        if self.type_key(event, ctx) || self.scroll(event) {
            // Both move the viewport to somewhere the last frame did not draw:
            // typing returns to the live output, and the wheel leaves it.
            ctx.notify();
            return true;
        }

        match event {
            Event::MouseDown {
                button: MouseButton::Left,
                position,
                click_count,
                modifiers,
            } if self
                .bounds()
                .is_some_and(|bounds| bounds.contains_point(*position)) =>
            {
                self.press(*position, *click_count, *modifiers, ctx)
            }
            _ => false,
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// An extent to lay out at, given a maximum that may be unbounded.
///
/// A flex asks "how big would you like to be?" with an infinite maximum. A grid
/// has no answer to that — it fills what it is given — so it takes the minimum,
/// which is the only finite number in the question.
fn bounded(max: f32, min: f32) -> f32 {
    if max.is_finite() { max } else { min }
}

/// Paints one snapshot into `scene`, filling `size` from `origin`.
///
/// `owns_caret` is whether this grid is where typing would go. It is the
/// cursor's business and nothing else's: a grid that is not listening draws an
/// outline where a listening one draws the filled block the program asked for,
/// which is how every terminal with splits says which shell has the keyboard.
fn paint_grid(
    snapshot: &Snapshot,
    font: &CellFont,
    origin: Vector2F,
    size: Vector2F,
    owns_caret: bool,
    scene: &mut Scene,
) {
    let metrics = font.metrics();
    let (fitting_columns, fitting_rows) = metrics.grid_for(size.x(), size.y());
    let columns = usize::from(fitting_columns).min(snapshot.columns);
    let rows = usize::from(fitting_rows).min(snapshot.rows);

    // The ground the emulator resolved, painted once. Every cell that agrees
    // with it — which on an idle screen is all of them — then costs nothing.
    scene
        .draw_rect_without_hit_recording(RectF::new(origin, size))
        .with_background(color(snapshot.background));

    let shape = snapshot
        .cursor
        .as_ref()
        .map(|cursor| shape_of(cursor, owns_caret));
    // A block cursor is drawn as a filled cell, so the character inside it has
    // to be drawn in the ground colour or it disappears into the fill. An
    // outline leaves the character alone.
    let inverted = snapshot
        .cursor
        .as_ref()
        .filter(|_| shape == Some(CursorShape::Block))
        .map(|cursor| (cursor.row, cursor.column));

    for (row, cells) in snapshot.iter_rows().enumerate().take(rows) {
        let top = origin.y() + row as f32 * metrics.height;
        let cells = &cells[..columns];

        paint_backgrounds(cells, snapshot.background, origin.x(), top, metrics, scene);
        // Over the cells' own backgrounds and under everything else: the
        // highlight is translucent, so it takes the colour of whatever the
        // shell painted the cell and the character is still drawn on top of it
        // in its own ink. Selecting text changes its ground, never its colour.
        paint_selection(snapshot, row, columns, origin.x(), top, metrics, scene);
        paint_rules(cells, origin.x(), top, metrics, scene);
        paint_glyphs(
            cells,
            &RowMarks::Snapshot { snapshot, row },
            origin.x(),
            top,
            font,
            Ink {
                ground: snapshot.background,
                inverted: inverted.and_then(|(at, column)| (at == row).then_some(column)),
            },
            scene,
        );
    }

    if let (Some(cursor), Some(shape)) = (snapshot.cursor.as_ref(), shape)
        && cursor.row < rows
        && cursor.column < columns
    {
        paint_cursor(cursor, shape, origin, metrics, scene);
    }
}

/// Where a row's zero-width characters come from.
///
/// A combining accent belongs to the cell before it and has no column of its
/// own, so it is stored beside the row rather than in it — and the two stores
/// a row can come out of keep it differently. This is the one difference
/// between painting a live row and a harvested one, and it is why the rest of
/// the cell path is shared rather than written twice.
pub(super) enum RowMarks<'a> {
    /// A live row's, looked up per column in the snapshot it belongs to.
    Snapshot {
        /// The snapshot holding the row.
        snapshot: &'a Snapshot,
        /// Which of its rows this is.
        row: usize,
    },
    /// A harvested row's, which arrive already scoped to the row.
    Block(&'a [RowCombining]),
}

impl RowMarks<'_> {
    /// The characters stacked on one cell of the row.
    fn at(&self, column: usize) -> &[char] {
        match self {
            Self::Snapshot { snapshot, row } => snapshot.zerowidth(*row, column),
            Self::Block(marks) => marks
                .iter()
                .find(|entry| entry.column == column)
                .map_or(&[], |entry| &entry.characters),
        }
    }
}

/// What decides a glyph's colour beyond the cell's own.
#[derive(Copy, Clone)]
pub(super) struct Ink {
    /// The colour behind the row, which a character under a filled cursor is
    /// drawn in so that it does not disappear into the fill.
    pub(super) ground: Rgb,
    /// The column that cursor is on, when it is on this row.
    pub(super) inverted: Option<usize>,
}

/// Draws one row's characters, and whatever is stacked on them.
///
/// The monospace fast path, and the only one: a harvested block's row is
/// materialised into a scratch `Vec<SnapshotCell>` and handed straight to this,
/// so a finished command and a live one are drawn by the same code. Everything
/// that makes an idle screen cheap is here — a blank cell draws nothing, the
/// trailing half of a double-width character draws nothing, and a glyph is
/// placed by multiplying a column by a fixed advance rather than by shaping.
pub(super) fn paint_glyphs(
    cells: &[SnapshotCell],
    marks: &RowMarks<'_>,
    left: f32,
    top: f32,
    font: &CellFont,
    ink: Ink,
    scene: &mut Scene,
) {
    let metrics = font.metrics();
    for (column, cell) in cells.iter().enumerate() {
        // The trailing half of a double-width character: its background
        // belongs to the pair and its glyph was drawn a column ago.
        if cell.flags.contains(CellFlags::WIDE_SPACER) || is_blank(cell.c) {
            continue;
        }

        let pen = vec2f(left + column as f32 * metrics.width, top + metrics.baseline);
        let foreground = color(if ink.inverted == Some(column) {
            ink.ground
        } else {
            cell.foreground
        });
        let face = font.face(cell.flags);
        paint_cell(cell.c, pen, face, foreground, font, metrics, scene);

        // Combining marks are drawn from the same pen position the character
        // was: a mark glyph carries its own offset from the character it sits
        // on, and no column of its own to sit in.
        for mark in marks.at(column) {
            paint_cell(*mark, pen, face, foreground, font, metrics, scene);
        }
    }
}

/// Draws one character at a pen position, in whichever face can draw it.
///
/// The face that comes back is not always the one that went in: the monospace
/// family covers Latin and not a spinner's braille, a powerline separator or a
/// CJK filename, and [`CellFont::glyph`] finds a face that does. A character no
/// installed font can draw is the only one that leaves the cell empty.
fn paint_cell(
    character: char,
    pen: Vector2F,
    face: crookui_core::fonts::FontId,
    ink: Color,
    font: &CellFont,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    let Some((drawn_with, glyph)) = font.glyph(face, character) else {
        log::trace!("no installed font can draw {character:?}");
        return;
    };
    scene.draw_glyph(pen, glyph, drawn_with, metrics.font_size, ink);
}

/// What a cursor is drawn as, given whether the grid is where typing goes.
///
/// A grid that is not listening is always an outline, whatever shape the child
/// asked for: the shape says what the *program* wants, and this says whether
/// the shape means anything right now. It does not while an input field below
/// holds the keyboard, which on the normal screen is always.
fn shape_of(cursor: &Cursor, owns_caret: bool) -> CursorShape {
    if owns_caret {
        cursor.shape
    } else {
        CursorShape::HollowBlock
    }
}

/// Fills the cells of one row whose background is not the grid's own.
///
/// Adjacent cells sharing a colour become one rectangle. A line of `ls --color`
/// is a handful of runs; painting it per cell would be one instanced quad per
/// column, most of them invisible.
pub(super) fn paint_backgrounds(
    cells: &[SnapshotCell],
    ground: Rgb,
    left: f32,
    top: f32,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    let mut start = 0;
    while start < cells.len() {
        let background = cells[start].background;
        let end = cells[start..]
            .iter()
            .position(|cell| cell.background != background)
            .map_or(cells.len(), |offset| start + offset);

        if background != ground {
            let run = RectF::new(
                vec2f(left + start as f32 * metrics.width, top),
                vec2f((end - start) as f32 * metrics.width, metrics.height),
            );
            scene
                .draw_rect_without_hit_recording(run)
                .with_background(color(background));
        }
        start = end;
    }
}

/// Fills the cells of one row that are inside the selection.
///
/// Merged into runs the way the backgrounds are: a selected line is one
/// rectangle, not eighty, and a selection dragged over a screenful of `cat` is
/// a rectangle per row.
fn paint_selection(
    snapshot: &Snapshot,
    row: usize,
    columns: usize,
    left: f32,
    top: f32,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    if snapshot.selection.is_none() {
        return;
    }

    let mut start = 0;
    while start < columns {
        if !snapshot.is_selected(row, start) {
            start += 1;
            continue;
        }

        let end = (start..columns)
            .find(|column| !snapshot.is_selected(row, *column))
            .unwrap_or(columns);
        scene
            .draw_rect_without_hit_recording(RectF::new(
                vec2f(left + start as f32 * metrics.width, top),
                vec2f((end - start) as f32 * metrics.width, metrics.height),
            ))
            .with_background(theme().selection);
        start = end;
    }
}

/// Which column a horizontal offset lands on, and which half of it.
///
/// The half is what makes a selection end *between* two characters rather than
/// on one: dragging right from the left of a character takes it and dragging
/// right from its middle does not. Outside the row the answer is its first cell
/// on the left and its last on the right, which is what a drag that has left
/// the pane means.
pub(super) fn column_at(x: f32, width: f32, columns: usize) -> (usize, CellSide) {
    if x < 0. {
        return (0, CellSide::Left);
    }
    let cell = x / width;
    let column = cell.floor() as usize;
    if column >= columns {
        return (columns - 1, CellSide::Right);
    }
    let side = if cell.fract() < 0.5 {
        CellSide::Left
    } else {
        CellSide::Right
    };
    (column, side)
}

/// Draws the underlines and strikeouts one row asks for.
///
/// Merged into runs the way backgrounds are, and for the same reason: `man`
/// through `less` renders italics as underline, so a full-width line of it is
/// two hundred abutting rectangles of identical height and colour where one
/// would do — a megabyte of instance data per frame, per pane, for a page that
/// is not even moving.
pub(super) fn paint_rules(
    cells: &[SnapshotCell],
    left: f32,
    top: f32,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    let below = (metrics.height - metrics.baseline) * UNDERLINE_OFFSET_RATIO;
    rule_runs(
        cells,
        CellFlags::UNDERLINE,
        left,
        top + metrics.baseline + below,
        metrics,
        scene,
    );

    let above = metrics.font_size * STRIKEOUT_HEIGHT_RATIO;
    rule_runs(
        cells,
        CellFlags::STRIKEOUT,
        left,
        top + metrics.baseline - above,
        metrics,
        scene,
    );
}

/// One rule per run of adjacent cells carrying `flag` in the same colour.
fn rule_runs(
    cells: &[SnapshotCell],
    flag: CellFlags,
    left: f32,
    top: f32,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    let mut start = 0;
    while start < cells.len() {
        if !cells[start].flags.contains(flag) {
            start += 1;
            continue;
        }

        let ink = cells[start].foreground;
        let end = cells[start..]
            .iter()
            .position(|cell| !cell.flags.contains(flag) || cell.foreground != ink)
            .map_or(cells.len(), |offset| start + offset);

        let thickness = (metrics.font_size * RULE_THICKNESS_RATIO).max(1.);
        scene
            .draw_rect_without_hit_recording(RectF::new(
                vec2f(left + start as f32 * metrics.width, top),
                vec2f((end - start) as f32 * metrics.width, thickness),
            ))
            .with_background(color(ink));
        start = end;
    }
}

/// Draws the cursor in `shape`, at the cell the viewport puts it in.
fn paint_cursor(
    cursor: &Cursor,
    shape: CursorShape,
    origin: Vector2F,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    paint_cursor_in(
        cursor,
        shape,
        vec2f(
            origin.x() + cursor.column as f32 * metrics.width,
            origin.y() + cursor.row as f32 * metrics.height,
        ),
        metrics,
        scene,
    );
}

/// Draws the cursor at a cell somebody else placed.
///
/// What the block list needs: the open block's rows are drawn where the *list*
/// puts them, which is not where the viewport would, so the caller has already
/// done the arithmetic. `owns_caret` is whether typing would go here — a
/// surface that is not listening draws an outline where a listening one draws
/// the filled block the program asked for.
pub(super) fn paint_cursor_at(
    cursor: &Cursor,
    owns_caret: bool,
    at: Vector2F,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    paint_cursor_in(cursor, shape_of(cursor, owns_caret), at, metrics, scene);
}

/// Draws the cursor in `shape`, in the cell whose top-left corner is `at`.
fn paint_cursor_in(
    cursor: &Cursor,
    shape: CursorShape,
    at: Vector2F,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    let cell = RectF::new(at, vec2f(metrics.width, metrics.height));
    let ink = color(cursor.color);

    let bounds = match shape {
        CursorShape::Block => cell,
        CursorShape::HollowBlock => {
            // The one shape that is a stroke rather than a fill, which is what
            // an unfocused terminal conventionally draws.
            scene.draw_rect_without_hit_recording(cell).with_border(
                crookui_core::scene::Border::all(CURSOR_STROKE).with_border_color(ink),
            );
            return;
        }
        CursorShape::Underline => RectF::new(
            vec2f(cell.origin().x(), cell.max_y() - CURSOR_STROKE),
            vec2f(metrics.width, CURSOR_STROKE),
        ),
        CursorShape::Beam => RectF::new(cell.origin(), vec2f(CURSOR_STROKE, metrics.height)),
    };

    scene
        .draw_rect_without_hit_recording(bounds)
        .with_background(ink);
}

/// Whether a cell has nothing to draw.
///
/// A blank cell holds a space by contract, never a `\0`; the null is checked
/// anyway because drawing one would put a notdef box on every empty column of a
/// grid built by something other than this emulator.
fn is_blank(character: char) -> bool {
    character == ' ' || character == '\0'
}

/// A resolved terminal colour as the scene spells one. Every cell is opaque:
/// the emulator has already applied inverse, dim and hidden.
pub(super) fn color(rgb: Rgb) -> Color {
    Color::rgb(rgb.r, rgb.g, rgb.b)
}

#[cfg(test)]
#[path = "terminal_element_tests.rs"]
mod tests;
