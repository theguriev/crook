//! The command input: an [`Editor`] with a caret, a selection and a mouse.
//!
//! This is the field under a pane's grid, and it is drawn on the grid's terms.
//! A terminal cell is the unit of everything here — the caret is one cell wide,
//! a selection is a run of cells, and a glyph is placed by multiplying a column
//! by a fixed advance — so the line being composed sits in the same type, on
//! the same rhythm, as the output above it. Nothing is shaped: like
//! [`TerminalElement`](super::TerminalElement), the field asks
//! [`CellFont::glyph`] for a glyph per character and puts the record straight
//! into the scene.
//!
//! # Cells, not characters
//!
//! A column is a cell and a cell is not a character: `日` is two columns wide
//! in a terminal and so is an emoji, which is why every measurement here goes
//! through [`cells`] rather than counting grapheme clusters. The grid above
//! gives the same characters the same two columns — the emulator flags the
//! second one as a spacer — so a command echoed by the shell lands under the
//! one the field drew it as, character for character.
//!
//! # Rows
//!
//! The editor's lines are logical: it stores newlines and knows nothing about
//! how wide anything is. Wrapping is this element's business, and it is where
//! the field's height comes from — a line longer than the field becomes two
//! rows, the box grows by one, and the grid above gives up the space. The box
//! stops growing at [`MAX_ROWS`], and sooner than that in a pane too short to
//! spare them: see [`row_budget`]. Past its budget the field scrolls, keeping
//! the caret in view, because a command that has run away with itself must not
//! push the output it was written against off the screen.
//!
//! Only the rows that are drawn are ever built. A pasted megabyte is one
//! command line as far as the editor is concerned, and materialising its
//! hundred thousand rows to draw eight of them would cost a quarter of a
//! second on every keystroke after it.
//!
//! # What it does not own
//!
//! Not the text: that is the [`PaneInput`] the workspace keeps per pane, which
//! is what survives this element being thrown away and rebuilt on every frame.
//! Not the keymap either — see [`crate::input_keys`], which is also where the
//! decision to hand a keystroke to the shell instead is made.

use std::collections::VecDeque;
use std::ops::Range;

use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::{DispatchedEvent, Event, Keystroke, MouseButton};
use crookui_core::geometry::{Color, Point, RectF, Vector2F, vec2f};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};
use crookui_core::scene::Scene;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::clipboard::Clipboard;
use crate::input_keys::{self, Platform, Route};
use crate::pane_input::PaneInput;
use crate::terminal_font::{CellFont, CellMetrics};
use crate::terminal_model::TerminalHandle;
use crate::theme::theme;

/// The mark at the left of the field.
///
/// A prompt rather than a label: it is what makes the box read as a command
/// line rather than as somewhere to write a paragraph. A machine with no font
/// that can draw it draws nothing, and the field is still a field.
const PROMPT: char = '\u{276f}';

/// How many cells the prompt and the gap after it take.
const PROMPT_COLUMNS: usize = 2;

/// The tallest the field ever grows, in rows.
const MAX_ROWS: usize = 8;

/// How much of the accent colour a selection is filled with.
///
/// Low enough to read text through, high enough to see at a glance which cells
/// are in it.
const SELECTION_ALPHA: u8 = 76;

/// One pane's command input.
pub struct CommandInput {
    input: PaneInput,
    font: CellFont,
    clipboard: Clipboard,

    /// The shell a submitted line is sent to, when there is one.
    terminal: Option<TerminalHandle>,

    /// Whether this pane's field was the one listening when the frame was
    /// built. The caret's business and the prompt's; a keystroke asks
    /// [`PaneInput::has_keys`] instead, because by then this is a frame old.
    focused: bool,

    /// Whether the program on the far end owns the screen. Passed to
    /// [`input_keys::route`] rather than assumed, so the whole keyboard policy
    /// stays in the one function that states it.
    alt_screen: bool,

    size: Option<Vector2F>,
    origin: Option<Point>,
    /// The rows the last layout wrapped, so that painting the frame it
    /// measured does not wrap it again.
    rows: Option<Rows>,
    /// How many rows the last layout was allowed to draw. See [`row_budget`].
    budget: usize,
}

impl CommandInput {
    /// A field over `input`, attached to no shell and listening to nothing.
    pub fn new(input: PaneInput, font: CellFont, clipboard: Clipboard) -> Self {
        Self {
            input,
            font,
            clipboard,
            terminal: None,
            focused: false,
            alt_screen: false,
            size: None,
            origin: None,
            rows: None,
            budget: MAX_ROWS,
        }
    }

    /// Attaches the shell a submitted line goes to, and says whether this pane
    /// is the one whose field has the keys.
    pub fn with_terminal(
        mut self,
        handle: TerminalHandle,
        focused: bool,
        alt_screen: bool,
    ) -> Self {
        self.terminal = Some(handle);
        self.focused = focused;
        self.alt_screen = alt_screen;
        self
    }

    /// Handles a keystroke, if this pane's field is the one listening and the
    /// keystroke is the field's to have.
    fn type_key(&self, keystroke: &Keystroke, chars: &str, ctx: &mut EventContext) -> bool {
        if !self.input.has_keys() {
            return false;
        }

        let pane = input_keys::Pane {
            alt_screen: self.alt_screen,
            line_is_empty: self.input.editor().is_empty(),
        };
        match input_keys::route(keystroke, chars, pane, Platform::current()) {
            Route::Edit(intent) => {
                if let Some(line) = self.input.apply(intent, &self.clipboard) {
                    self.send(&line);
                }
                ctx.notify();
                true
            }
            // The shell is being interrupted by the grid beside this element,
            // and the line that was being written for it goes too. Reported as
            // unhandled so that the grid, which asks the same question and
            // acts on the other half of the answer, still sends the key.
            Route::Interrupt => {
                self.input.abandon();
                ctx.notify();
                false
            }
            Route::Raw | Route::Ignored => false,
        }
    }

    /// Sends a composed line to the shell, with the newline that runs it.
    fn send(&self, line: &str) {
        let Some(terminal) = self.terminal.as_ref() else {
            log::warn!("a line was composed in a pane with no shell to send it to");
            return;
        };
        // A newline rather than a carriage return: the pty's line discipline
        // turns one into the other, and this is what the shell is waiting for.
        terminal.write(&format!("{line}\n"));
    }

    /// Puts the caret where a press landed, selecting a word or a line when the
    /// press was part of a double or triple click.
    fn press(&self, position: Vector2F, click_count: u32, ctx: &mut EventContext) -> bool {
        let Some(bounds) = self.bounds() else {
            return false;
        };
        if !bounds.contains_point(position) {
            return false;
        }

        self.input
            .press(self.offset_at(position - bounds.origin()), click_count);
        ctx.notify();
        true
    }

    /// Drags the selection out to the pointer.
    fn drag(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
        let Some(bounds) = self.bounds() else {
            return false;
        };

        // Deliberately not bounded by the field: dragging past its edge is how
        // a selection is taken to the end of a line, and the hit test clamps.
        if !self
            .input
            .drag_to(self.offset_at(position - bounds.origin()))
        {
            return false;
        }
        ctx.notify();
        true
    }

    /// The offset a point inside the field lands on, snapped to the nearest
    /// grapheme boundary.
    ///
    /// Wrapped again rather than read off the last layout: a keystroke and a
    /// click can arrive between two frames, and rows measured against text
    /// that has since changed would point into the middle of it.
    fn offset_at(&self, local: Vector2F) -> usize {
        let metrics = self.font.metrics();
        let width = self.size.map_or(0., Vector2F::x);
        let editor = self.input.editor();
        let rows = Rows::of(editor.text(), editor.caret(), width, metrics, self.budget);
        rows.at_point(editor.text(), local, metrics)
    }
}

impl Element for CommandInput {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        // A column with a stretched cross axis settles the width before it asks
        // for a height; the fallback is for a parent that leaves it open, where
        // the smallest acceptable width is the only number in the question.
        let width = if constraint.max.x().is_finite() {
            constraint.max.x()
        } else {
            constraint.min.x()
        };

        let metrics = self.font.metrics();
        let budget = row_budget(constraint.max.y(), metrics);
        let editor = self.input.editor();
        let rows = Rows::of(editor.text(), editor.caret(), width, metrics, budget);
        let height = rows.drawn() as f32 * metrics.height;
        drop(editor);

        let size = vec2f(
            width,
            height.max(constraint.min.y()).min(constraint.max.y()),
        );
        self.size = Some(size);
        self.budget = budget;
        self.rows = Some(rows);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        let rows = self
            .rows
            .as_ref()
            .expect("an input was painted before it was laid out");

        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        paint_input(
            &self.input,
            &self.font,
            origin,
            rows,
            self.focused,
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
        // Keystrokes are never filtered by what is painted over them; a press
        // is, so that a menu open over the pane is not typed through.
        let Some(event) = event.at_z_index(z_index, ctx) else {
            return false;
        };

        match event {
            Event::KeyDown { keystroke, chars } => self.type_key(keystroke, chars, ctx),
            Event::MouseDown {
                button: MouseButton::Left,
                position,
                click_count,
                ..
            } => self.press(*position, *click_count, ctx),
            Event::MouseDragged {
                button: MouseButton::Left,
                position,
                ..
            } => self.drag(*position, ctx),
            Event::MouseUp {
                button: MouseButton::Left,
                ..
            } => self.input.release(),
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

/// The rows of one field that are actually drawn.
///
/// Built for a frame and thrown away, and never longer than the budget: the
/// walk that finds them keeps a window of the last few rows as it goes and
/// stops one screen past the caret, so a field holding a pasted megabyte
/// allocates the same eight ranges a field holding `ls` does.
struct Rows {
    /// The byte range of the text each drawn row shows, top to bottom.
    rows: Vec<Range<usize>>,
}

impl Rows {
    /// The rows of `text` a field this wide draws, scrolled so that `caret` is
    /// among them.
    fn of(text: &str, caret: usize, width: f32, metrics: CellMetrics, budget: usize) -> Self {
        let budget = budget.max(1);
        let mut rows = VecDeque::with_capacity(budget);

        // The caret is kept on the last row of the window rather than the
        // first: a command is written left to right and downwards, so the rows
        // that matter are the ones behind the caret. Only when there are not
        // that many behind it does the window run on past it.
        for row in RowWalk::new(text, columns_for(width, metrics)) {
            let past_caret = row.start > caret;
            if past_caret && rows.len() >= budget {
                break;
            }
            rows.push_back(row);
            if !past_caret {
                while rows.len() > budget {
                    rows.pop_front();
                }
            }
        }

        Self { rows: rows.into() }
    }

    /// How many rows are drawn.
    fn drawn(&self) -> usize {
        self.rows.len()
    }

    /// The rows that are drawn, top to bottom.
    fn iter(&self) -> impl Iterator<Item = &Range<usize>> {
        self.rows.iter()
    }

    /// Where an offset is drawn: the row on screen and the cell column in it,
    /// or `None` when it has been scrolled out of the field.
    fn place(&self, text: &str, offset: usize) -> Option<(usize, usize)> {
        let row = self.rows.iter().rposition(|row| row.start <= offset)?;
        let range = &self.rows[row];
        if offset > range.end {
            return None;
        }
        Some((row, cells(&text[range.start..offset])))
    }

    /// The offset a point in the field lands on.
    ///
    /// The column rounds rather than truncating, so the boundary is halfway
    /// across a cell: a click on the left half of a character puts the caret
    /// before it and one on the right half puts it after.
    fn at_point(&self, text: &str, local: Vector2F, metrics: CellMetrics) -> usize {
        let row = (local.y() / metrics.height)
            .floor()
            .clamp(0., (self.drawn() - 1) as f32) as usize;
        let column = ((local.x() - PROMPT_COLUMNS as f32 * metrics.width) / metrics.width)
            .round()
            .max(0.) as usize;

        let Some(range) = self.rows.get(row) else {
            return text.len();
        };
        // Past the end of the row is the end of the row, never the start of the
        // next one: a click in the empty space to the right of a wrapped line
        // means the end of what is on that line.
        let mut at = 0;
        for (offset, grapheme) in text[range.clone()].grapheme_indices(true) {
            if at >= column {
                return range.start + offset;
            }
            at += cells(grapheme);
        }
        range.end
    }
}

/// The visual rows of a text, in order, one at a time.
///
/// An iterator rather than a `Vec` because of what it costs not to be one: the
/// caller draws at most a handful of rows and the text can be enormous, and
/// this way the rows before the window are counted rather than kept.
struct RowWalk<'a> {
    text: &'a str,
    /// How many cells of text fit on a row.
    columns: usize,
    /// Where the next row starts.
    at: usize,
    /// The end of the logical line the next row starts in, carried rather than
    /// searched for again: a line wrapped into ten thousand rows would
    /// otherwise be scanned for its newline ten thousand times.
    line_end: usize,
    /// Whether that line is all ASCII, decided once for the line for the same
    /// reason — a pasted megabyte is one line, and asking per row is asking
    /// ten thousand times.
    line_is_ascii: bool,
    /// Whether anything is left to produce.
    more: bool,
    /// Whether the next row is the empty one that a full final row opens. See
    /// the note on [`RowWalk::next`].
    trailing: bool,
}

impl<'a> RowWalk<'a> {
    fn new(text: &'a str, columns: usize) -> Self {
        let line_end = line_end_from(text, 0);
        Self {
            text,
            columns,
            at: 0,
            line_end,
            line_is_ascii: text[..line_end].is_ascii(),
            more: true,
            trailing: false,
        }
    }
}

impl Iterator for RowWalk<'_> {
    type Item = Range<usize>;

    /// The next row.
    ///
    /// The one row here that holds no text is the empty one after a *last*
    /// line that exactly filled its row: that is where the caret goes once a
    /// row is full, and where the next character will be drawn. A line that
    /// ends in a newline gets no such row — the next line is already there,
    /// and an empty row between the two would be a blank line nobody typed.
    fn next(&mut self) -> Option<Range<usize>> {
        if !self.more {
            return None;
        }
        if self.trailing {
            self.more = false;
            return Some(self.at..self.at);
        }

        let start = self.at;
        let (bytes, filled) = take_cells(
            &self.text[start..self.line_end],
            self.columns,
            self.line_is_ascii,
        );
        let end = start + bytes;

        if end < self.line_end {
            // More of this line to come.
            self.at = end;
        } else if self.line_end == self.text.len() {
            self.at = end;
            self.trailing = filled == self.columns;
            self.more = self.trailing;
        } else {
            // Past the newline that ended this line.
            self.at = self.line_end + 1;
            self.line_end = line_end_from(self.text, self.at);
            self.line_is_ascii = self.text[self.at..self.line_end].is_ascii();
        }
        Some(start..end)
    }
}

/// The end of the logical line `at` starts, exclusive of its newline.
fn line_end_from(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |index| at + index)
}

/// How many bytes of `line` fit in `columns` cells, and how many cells they
/// took, where `ascii` says whether the line holding it is all ASCII.
///
/// A cluster is never split across two rows: one that will not fit starts the
/// next row instead, and one wider than the whole row is taken anyway, because
/// a field one cell wide still has to make progress.
fn take_cells(line: &str, columns: usize, ascii: bool) -> (usize, usize) {
    // Every ASCII character the editor will hold is one cell — control
    // characters never reach the buffer, see `editor::text::printable` — so
    // the common line skips grapheme segmentation altogether. This is what
    // keeps a pasted base64 blob from costing a walk of itself per frame.
    if ascii {
        let taken = line.len().min(columns);
        return (taken, taken);
    }

    let mut bytes = 0;
    let mut filled = 0;
    for (offset, grapheme) in line.grapheme_indices(true) {
        let width = cells(grapheme);
        if filled + width > columns && filled > 0 {
            break;
        }
        filled += width;
        bytes = offset + grapheme.len();
        if filled >= columns {
            break;
        }
    }
    (bytes, filled)
}

/// How many cells a string takes on a terminal grid.
///
/// The same rule the emulator applies to what the shell prints: a character's
/// East Asian width, and zero for a combining mark, which is what gives `日`
/// two columns and `e` plus an acute one. Never zero for a whole cluster,
/// because the caret has to have somewhere to stand.
fn cells(text: &str) -> usize {
    if text.is_ascii() {
        return text.len();
    }
    text.graphemes(true)
        .map(|grapheme| UnicodeWidthStr::width(grapheme).max(1))
        .sum()
}

/// How many rows the field may draw in the space it has been offered.
///
/// Never more than [`MAX_ROWS`], and never more than half of what it was
/// offered: the field grows downwards *into the grid*, and one that took the
/// whole panel would leave the shell it is composing for nothing to be seen
/// in. Never fewer than one, because a pane can be dragged shorter than a cell
/// and a field with no rows would have no caret.
fn row_budget(available: f32, metrics: CellMetrics) -> usize {
    if !available.is_finite() {
        return MAX_ROWS;
    }
    let fits = (available / metrics.height).floor().max(0.) as usize;
    (fits / 2).clamp(1, MAX_ROWS)
}

/// How many cells of text a field this wide holds. Never fewer than one: a pane
/// can be dragged narrower than its own prompt.
fn columns_for(width: f32, metrics: CellMetrics) -> usize {
    let cells = (width / metrics.width).floor().max(0.) as usize;
    cells.saturating_sub(PROMPT_COLUMNS).max(1)
}

/// Paints one field into `scene` from `origin`, in the rows a layout wrapped.
///
/// `focused` is whether this pane's field is the one receiving keys, and it is
/// the caret's business and nothing else's: an unfocused field shows its text
/// and its selection and no caret at all, because a caret in a field that is
/// not listening is a lie about where typing would go.
fn paint_input(
    input: &PaneInput,
    font: &CellFont,
    origin: Vector2F,
    rows: &Rows,
    focused: bool,
    scene: &mut Scene,
) {
    let metrics = font.metrics();
    let editor = input.editor();
    let text = editor.text();
    let left = origin.x() + PROMPT_COLUMNS as f32 * metrics.width;

    let prompt = if focused {
        theme().accent
    } else {
        theme().text_muted
    };
    paint_character(
        PROMPT,
        vec2f(origin.x(), origin.y() + metrics.baseline),
        prompt,
        font,
        scene,
    );

    let selection = editor.selection().range();
    let caret = (focused && input.caret_is_visible())
        .then(|| rows.place(text, editor.caret()))
        .flatten();

    for (row, range) in rows.iter().enumerate() {
        let top = origin.y() + row as f32 * metrics.height;
        paint_selection(text, range, &selection, left, top, metrics, scene);

        let mut column = 0;
        for grapheme in text[range.clone()].graphemes(true) {
            let pen = vec2f(left + column as f32 * metrics.width, top + metrics.baseline);
            // A character under a filled caret has to be drawn in the ground it
            // sits on, or it disappears into the fill — the same trick the grid
            // plays with a block cursor.
            let ink = if caret == Some((row, column)) {
                theme().ground
            } else {
                theme().text_primary
            };

            // Every character of a cluster shares one pen: a combining mark
            // carries its own offset from the character it sits on, and has no
            // cell of its own to sit in.
            for character in grapheme.chars() {
                paint_character(character, pen, ink, font, scene);
            }
            column += cells(grapheme);
        }
    }

    // Last, so that it is drawn over the selection it may be sitting in.
    if let Some((row, column)) = caret {
        scene
            .draw_rect_without_hit_recording(RectF::new(
                vec2f(
                    left + column as f32 * metrics.width,
                    origin.y() + row as f32 * metrics.height,
                ),
                vec2f(metrics.width, metrics.height),
            ))
            .with_background(theme().accent);
    }
}

/// Fills the cells of one row that are inside the selection.
///
/// One rectangle per row rather than per cell, for the reason the grid merges
/// its background runs: a selected line is one quad, not eighty.
fn paint_selection(
    text: &str,
    row: &Range<usize>,
    selection: &Range<usize>,
    left: f32,
    top: f32,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    let start = selection.start.max(row.start);
    let end = selection.end.min(row.end);
    if start >= end {
        return;
    }

    let from = cells(&text[row.start..start]);
    let to = cells(&text[row.start..end]);
    scene
        .draw_rect_without_hit_recording(RectF::new(
            vec2f(left + from as f32 * metrics.width, top),
            vec2f((to - from) as f32 * metrics.width, metrics.height),
        ))
        .with_background(theme().accent.with_alpha(SELECTION_ALPHA));
}

/// Draws one character at a pen position, in whichever face can draw it.
///
/// The same fallback the grid takes, and for the same reason: a command line
/// holds filenames, and a filename is not Latin.
fn paint_character(character: char, pen: Vector2F, ink: Color, font: &CellFont, scene: &mut Scene) {
    let Some((face, glyph)) = font.glyph(font.regular(), character) else {
        log::trace!("no installed font can draw {character:?}");
        return;
    };
    scene.draw_glyph(pen, glyph, face, font.metrics().font_size, ink);
}

#[cfg(test)]
#[path = "input_element_tests.rs"]
mod tests;
