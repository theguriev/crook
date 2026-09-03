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
//! Layout is also where the pty learns how big it is. The element is told what
//! space it has, divides it by the cell, and resizes the terminal — but only
//! when the whole number of columns or rows actually moved, because a resize is
//! a syscall, a `SIGWINCH`, and a full-screen program redrawing itself.
//!
//! [`Line`]: crookui_core::text_layout::Line

use std::sync::Arc;

use crook_terminal::{CellFlags, Cursor, CursorShape, Rgb, Snapshot, SnapshotCell, TerminalSize};
use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::{DispatchedEvent, Event};
use crookui_core::geometry::{Color, Point, RectF, Vector2F, vec2f};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};
use crookui_core::scene::Scene;

use crate::input_keys::{self, Platform};
use crate::pane_input::PaneInput;
use crate::terminal_font::{CellFont, CellMetrics};
use crate::terminal_keys;
use crate::terminal_model::TerminalHandle;

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

/// What a pane's grid does with the keys that reach the window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Keys {
    /// Nothing: some other pane is the focused one.
    None,
    /// Only the keys that interrupt, end and suspend a command.
    ///
    /// What a focused pane still takes while a modal menu is open over the
    /// window. The menu freezes everything under it, and a `sleep 30` that
    /// could not be interrupted until somebody found the mouse would be the
    /// menu taking away the one key a terminal must never lose.
    Signals,
    /// Everything the routing rule gives the shell.
    All,
}

/// One pane's terminal grid.
pub struct TerminalElement {
    /// The grid to paint. Replaced during layout when a resize moved it, so a
    /// frame that changed the column count draws the new one rather than the
    /// old one stretched over it.
    snapshot: Arc<Snapshot>,
    font: CellFont,

    /// The terminal behind the grid, when there is a live one.
    ///
    /// `None` draws a snapshot and nothing else — no resize, no typing — which
    /// is what a test does, and what a pane whose shell has gone would do.
    handle: Option<TerminalHandle>,

    /// How much of the keyboard this pane's grid takes.
    ///
    /// Decided by the workspace rather than here, because it is the workspace
    /// that knows which pane is focused and whether a menu is up over it. See
    /// [`crate::terminal_keys`] for the other half of the same line.
    keys: Keys,

    /// The line being composed under this grid, when there is a field.
    ///
    /// Read at the moment a key arrives rather than baked in when the frame
    /// was built, because it decides what Ctrl-D means: an end of input on an
    /// empty line, and a delete over a written one.
    input: Option<PaneInput>,

    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl TerminalElement {
    /// A grid that paints `snapshot` and is driven by nothing.
    pub fn new(snapshot: Arc<Snapshot>, font: CellFont) -> Self {
        Self {
            snapshot,
            font,
            handle: None,
            keys: Keys::None,
            input: None,
            size: None,
            origin: None,
        }
    }

    /// Attaches the terminal the snapshot came from, so the grid can resize the
    /// pty it measures and type into it as far as `keys` allows.
    pub fn with_terminal(mut self, handle: TerminalHandle, keys: Keys) -> Self {
        self.handle = Some(handle);
        self.keys = keys;
        self
    }

    /// Attaches the field under this grid, whose line decides what Ctrl-D
    /// means.
    pub fn with_input(mut self, input: PaneInput) -> Self {
        self.input = Some(input);
        self
    }

    /// The typed keystroke, if this pane is the one that should have it and the
    /// shell is the half of the pane it belongs to.
    fn type_key(&self, event: &Event) -> bool {
        let Event::KeyDown { keystroke, chars } = event else {
            return false;
        };
        if self.keys == Keys::None {
            return false;
        }
        let Some(handle) = self.handle.as_ref() else {
            return false;
        };

        // The whole policy is [`input_keys::route`]: on the alt screen the
        // program has every key, on the normal screen the shell has only the
        // ones that interrupt, end and suspend, and the input field below has
        // the rest.
        let pane = input_keys::Pane {
            alt_screen: self.snapshot.alt_screen,
            line_is_empty: self
                .input
                .as_ref()
                .is_none_or(|input| input.editor().is_empty()),
        };
        if !input_keys::route(keystroke, chars, pane, Platform::current()).reaches_the_shell() {
            return false;
        }
        // A modal menu takes the rest away: everything but the three keys a
        // running command has to keep hearing.
        if self.keys == Keys::Signals && !input_keys::is_signal(keystroke) {
            return false;
        }

        let Some((key, modifiers)) = terminal_keys::key_for(keystroke, chars) else {
            return false;
        };
        handle.send_key(key, modifiers)
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

        let Some(handle) = self.handle.as_ref() else {
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

        let metrics = self.font.metrics();
        let (columns, rows) = metrics.grid_for(size.x(), size.y());
        if let Some(handle) = self.handle.as_ref() {
            let resized = handle.resize(
                TerminalSize::new(columns, rows)
                    .with_cell_size(metrics.width.round() as u16, metrics.height.round() as u16),
            );
            // Only when the grid actually moved. The snapshot this element was
            // built with was measured for the old one, and painting it into the
            // new box would show a frame of the wrong width.
            if resized {
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
        let owns_caret =
            self.keys == Keys::All && !input_keys::shows_input(self.snapshot.alt_screen);
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
        // Keystrokes are never filtered by what is painted over them — they are
        // not about a place on screen — but the wheel is, so that a menu open
        // over a pane scrolls the menu rather than the shell underneath it.
        let Some(event) = event.at_z_index(z_index, ctx) else {
            return false;
        };

        if self.type_key(event) || self.scroll(event) {
            // Both move the viewport to somewhere the last frame did not draw:
            // typing returns to the live output, and the wheel leaves it.
            ctx.notify();
            return true;
        }
        false
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
        paint_rules(cells, origin.x(), top, metrics, scene);

        for (column, cell) in cells.iter().enumerate() {
            // The trailing half of a double-width character: its background
            // belongs to the pair and its glyph was drawn a column ago.
            if cell.flags.contains(CellFlags::WIDE_SPACER) || is_blank(cell.c) {
                continue;
            }

            let pen = vec2f(
                origin.x() + column as f32 * metrics.width,
                top + metrics.baseline,
            );
            let foreground = color(if inverted == Some((row, column)) {
                snapshot.background
            } else {
                cell.foreground
            });
            let face = font.face(cell.flags);
            paint_cell(cell.c, pen, face, foreground, font, metrics, scene);

            // Combining marks are drawn from the same pen position the
            // character was: a mark glyph carries its own offset from the
            // character it sits on, and no column of its own to sit in.
            for mark in snapshot.zerowidth(row, column) {
                paint_cell(*mark, pen, face, foreground, font, metrics, scene);
            }
        }
    }

    if let (Some(cursor), Some(shape)) = (snapshot.cursor.as_ref(), shape)
        && cursor.row < rows
        && cursor.column < columns
    {
        paint_cursor(cursor, shape, origin, metrics, scene);
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
fn paint_backgrounds(
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

/// Draws the underlines and strikeouts one row asks for.
///
/// Merged into runs the way backgrounds are, and for the same reason: `man`
/// through `less` renders italics as underline, so a full-width line of it is
/// two hundred abutting rectangles of identical height and colour where one
/// would do — a megabyte of instance data per frame, per pane, for a page that
/// is not even moving.
fn paint_rules(
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

/// Draws the cursor in `shape`.
fn paint_cursor(
    cursor: &Cursor,
    shape: CursorShape,
    origin: Vector2F,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    let cell = RectF::new(
        vec2f(
            origin.x() + cursor.column as f32 * metrics.width,
            origin.y() + cursor.row as f32 * metrics.height,
        ),
        vec2f(metrics.width, metrics.height),
    );
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
fn color(rgb: Rgb) -> Color {
    Color::rgb(rgb.r, rgb.g, rgb.b)
}

#[cfg(test)]
#[path = "terminal_element_tests.rs"]
mod tests;
