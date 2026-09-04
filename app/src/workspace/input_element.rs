//! The command input: an [`Editor`] with a caret, a selection and a mouse.
//!
//! This is the composer under a pane's output, and it is drawn on the
//! output's terms.
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
//! # The first row continues the shell's prompt
//!
//! No shell puts its prompt on one line and your typing on the next, so
//! neither does this. When the shell says where its prompt ended — OSC 133
//! `B`, whose cell the emulator keeps — the field's first row is drawn on that
//! same row, starting at the cell after the prompt's last one, and the caret
//! sits immediately after the `❯` exactly as it would in any other terminal.
//! That row is *above* this element's own box, because the list above painted
//! the prompt there, and the field is therefore one row shorter than the
//! number of rows it draws.
//!
//! The offset belongs to the first row of the text and to no other. A line
//! that wraps carries on at the gutter on the next row, the way a shell's own
//! line editor wraps, and a field that has scrolled past its first row —
//! a command long enough to overflow its budget — has no row that continues
//! anything and goes back to being a block of rows at the gutter.
//!
//! Where the shell said nothing, the field keeps its own row. See
//! [`block_list::inline_start`](super::block_list::inline_start) for the
//! whole of that decision, and for why there is no third case.
//!
//! # Rows
//!
//! The editor's lines are logical: it stores newlines and knows nothing about
//! how wide anything is. Wrapping is this element's business, and it is where
//! the composer's height comes from — a line longer than the box becomes two
//! rows, the box grows by one, and the output above gives up the space. It
//! stops growing at [`MAX_ROWS`], and sooner than that in a pane too short to
//! spare them: see [`row_budget`]. Past its budget it scrolls, keeping the
//! caret in view, because a command that has run away with itself must not
//! push the output it was written against off the screen.
//!
//! Only the rows that are drawn are ever built. A pasted megabyte is one
//! command line as far as the editor is concerned, and materialising its
//! hundred thousand rows to draw eight of them would cost a quarter of a
//! second on every keystroke after it.
//!
//! # No box, and no prompt
//!
//! The chrome is in `body::composer`, and there is almost none of it: no
//! background, no radius, no side or bottom border, no margin, and no focus
//! ring. What this element draws is text, a selection and a caret — a three
//! pixel bar in the *terminal's* cursor colour, because an accent caret is a
//! form field's and would disagree with the output above it.
//!
//! There is no prompt glyph either. The shell's own prompt is the one the
//! field's first row continues, and a second invented one under it is two
//! prompts on screen, which is exactly what makes a composer read as a widget
//! bolted under the output. Column zero here is column zero there.
//!
//! # What it does not own
//!
//! Not the text: that is the [`TextInput`] the workspace keeps per pane, which
//! is what survives this element being thrown away and rebuilt on every frame.
//! Not the keymap either — see [`crate::input_keys`], which is also where the
//! decision to hand a keystroke to the shell instead is made.

use std::collections::VecDeque;
use std::ops::Range;

use crook_terminal::Snapshot;
use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::{DispatchedEvent, Event, Keystroke, MouseButton};
use crookui_core::geometry::{Color, Point, RectF, Vector2F, vec2f};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};
use crookui_core::scene::{ClipBounds, CornerRadius, Radius, Scene};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::clipboard::Clipboard;
use crate::input_keys::{self, Platform, Route};
use crate::pane_blocks::{PaneBlocks, ScrollCause};
use crate::pane_selection::PaneSelection;
use crate::selection::Cells;
use crate::terminal_font::{CellFont, CellMetrics};
use crate::terminal_model::TerminalHandle;
use crate::text_input::TextInput;
use crate::theme::theme;

use super::terminal_element::color;

/// The tallest the composer ever grows, in rows.
const MAX_ROWS: usize = 8;

/// How wide the caret is drawn.
///
/// A bar rather than a filled cell, in the same three pixels Warp uses. The
/// grid draws the *shell's* cursor as a block, and a second block in the
/// composer would be two cursors claiming the same keyboard.
const CARET_WIDTH: f32 = 3.;

/// How tall the caret is, as a fraction of the cell.
///
/// The glyph's own height rather than the line box's, so the bar stands beside
/// the text it is in rather than touching the row above and the row below.
const CARET_HEIGHT: f32 = 0.8;

/// What the composer draws its text and its caret in.
///
/// The *terminal's* colours rather than the theme's. A shell that changed its
/// foreground or its cursor colour at runtime — OSC 10, OSC 12, any of the
/// scripts that follow a light or dark system theme — moves the output and the
/// pane's ground with it, and a field left behind in the theme's own grey is
/// then the one thing on the pane that does not follow. The block above and
/// the line being typed at its prompt have to be the same ink.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ink {
    /// The characters.
    pub text: Color,
    /// The caret.
    pub caret: Color,
}

impl Ink {
    /// The colours a snapshot resolved: its default foreground, and the colour
    /// the grid paints the shell's own cursor in.
    ///
    /// The theme's cursor is the fallback for a shell that has hidden its
    /// cursor, which is the one case a snapshot has no colour to offer.
    pub fn of(snapshot: &Snapshot) -> Self {
        Self {
            text: color(snapshot.foreground),
            caret: snapshot
                .cursor
                .as_ref()
                .map_or_else(|| theme().terminal.cursor, |cursor| color(cursor.color)),
        }
    }
}

impl Default for Ink {
    /// What a composer with no shell behind it draws in.
    fn default() -> Self {
        Self {
            text: theme().terminal.foreground,
            caret: theme().terminal.cursor,
        }
    }
}

/// One pane's command input.
pub struct CommandInput {
    input: TextInput,
    font: CellFont,
    clipboard: Clipboard,
    ink: Ink,

    /// The shell a submitted line is sent to, when there is one.
    terminal: Option<TerminalHandle>,

    /// What the pane's output has selected, which this element does two things
    /// with: it never takes the copy chord away from it, and clicking in here
    /// lets go of it.
    selection: Option<PaneSelection>,

    /// The list above this composer, which a submitted line returns to its own
    /// end.
    blocks: Option<PaneBlocks>,

    /// Whether this pane's field was the one listening when the frame was
    /// built. The caret's business and the prompt's; a keystroke asks
    /// [`TextInput::has_keys`] instead, because by then this is a frame old.
    focused: bool,

    /// Whether the program on the far end owns the screen. Passed to
    /// [`input_keys::route`] rather than assumed, so the whole keyboard policy
    /// stays in the one function that states it.
    alt_screen: bool,

    /// The column the first row starts at when it continues the shell's own
    /// prompt line, or `None` when the field takes a row of its own.
    ///
    /// Handed in by the body rather than worked out here, because the answer
    /// is about the *list* above: which row it drew last, and whether it is
    /// scrolled to its end. See [`super::block_list::inline_start`].
    inline: Option<usize>,

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
    pub fn new(input: TextInput, font: CellFont, clipboard: Clipboard) -> Self {
        Self {
            input,
            font,
            clipboard,
            ink: Ink::default(),
            terminal: None,
            selection: None,
            blocks: None,
            focused: false,
            alt_screen: false,
            inline: None,
            size: None,
            origin: None,
            rows: None,
            budget: MAX_ROWS,
        }
    }

    /// Draws in the terminal's own colours rather than the theme's.
    pub fn with_ink(mut self, ink: Ink) -> Self {
        self.ink = ink;
        self
    }

    /// Continues the shell's prompt line: the first row is drawn on the row
    /// above this element's box, starting at `column`.
    pub fn with_inline(mut self, column: Option<usize>) -> Self {
        self.inline = column;
        self
    }

    /// Attaches the block list above this composer, so that a command run from
    /// here returns it to its own end.
    pub fn with_blocks(mut self, blocks: PaneBlocks) -> Self {
        self.blocks = Some(blocks);
        self
    }

    /// Attaches what the output above has selected, which owns the copy chord
    /// for as long as it exists.
    pub fn with_selection(mut self, selection: PaneSelection) -> Self {
        self.selection = Some(selection);
        self
    }

    /// Attaches the shell a submitted line goes to, and says whether this pane
    /// is the one whose composer has the keys.
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

    /// How many rows the pane holds, which is what the composer's ceiling is
    /// half of.
    ///
    /// Asked of the terminal during layout rather than baked in when the frame
    /// was built: the pane resizes the pty in this same pass, a step above
    /// this element, so a row count taken from the snapshot the tree was built
    /// with would be one frame stale — and one frame stale here is a composer
    /// that takes the whole of a pane that has just been made short.
    fn pane_rows(&self) -> usize {
        self.terminal
            .as_ref()
            .map_or(0, |handle| handle.snapshot().rows)
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
            // Asked even though the answer only ever takes a key *away* from
            // this element: routing the same keystroke against a different
            // pane would let `cmd-c` copy the output above and then overwrite
            // the clipboard with whatever this field happened to have
            // selected. One question, one answer, two elements.
            //
            // Which holds only because nothing changes the answer while the
            // keystroke is in flight. The grid above is handed the same event
            // first, and it *does* let go of the selection it copies — but
            // through an action, applied once every element has routed. See
            // `WorkspaceAction::ReleaseSelection`.
            // The list's space, because that is the only surface a composer
            // is ever under: the grid never has one. A selection left over
            // from the grid is one this pane is no longer drawing.
            grid_has_selection: self
                .selection
                .as_ref()
                .is_some_and(|selection| selection.has_selection_in(Cells::List)),
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
            // The grid beside this element owns both: the interrupt's key and
            // the selection the copy chord is asking for.
            Route::CopyOutput | Route::Raw | Route::Ignored => false,
        }
    }

    /// Runs a composed line.
    ///
    /// [`TerminalHandle::submit`] rather than a write with a newline stuck on
    /// the end: the emulator is told the exact command and the exact moment
    /// before a byte leaves, which is the block boundary that does not depend
    /// on the shell having any integration at all.
    fn send(&self, line: &str) {
        let Some(terminal) = self.terminal.as_ref() else {
            log::warn!("a line was composed in a pane with no shell to send it to");
            return;
        };
        if !terminal.submit(line) {
            return;
        }
        // Running a command means the person is done reading history, and the
        // block it makes is at the end of the list. Without this a list left
        // scrolled up stays there, and nothing on screen answers the Enter.
        if let Some(blocks) = self.blocks.as_ref() {
            blocks.apply(ScrollCause::Submit);
        }
    }

    /// Puts the caret where a press landed, selecting a word or a line when the
    /// press was part of a double or triple click.
    fn press(&self, position: Vector2F, click_count: u32, ctx: &mut EventContext) -> bool {
        let Some(bounds) = self.bounds() else {
            return false;
        };
        if !self.takes_press(position - bounds.origin(), bounds.width()) {
            return false;
        }

        // Clicking into the field lets go of whatever was selected in the
        // output above it. Two highlights in one pane, only one of which the
        // pointer is anywhere near, is a person's next `cmd-c` copying the
        // wrong one — and the caret they just placed is where they are now
        // looking.
        if let Some(selection) = self.selection.as_ref() {
            selection.clear();
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

    /// Whether a press at `local` — measured from this element's origin — is
    /// the field's to answer.
    ///
    /// The field's own box, plus the part of the row above it that the field
    /// draws into when it continues the shell's prompt. **That row belongs to
    /// two elements**, and the split is by column: the prompt on the left is
    /// output for the list above to select, and the first cell of the line
    /// being typed onwards is the field's. `BlockList::press` declines exactly
    /// the half claimed here, so a press on that row is answered once.
    fn takes_press(&self, local: Vector2F, width: f32) -> bool {
        if local.x() < 0. || local.x() > width {
            return false;
        }
        let height = self.size.map_or(0., Vector2F::y);
        if local.y() >= 0. {
            return local.y() <= height;
        }

        let metrics = self.font.metrics();
        let Some(rows) = self.rows.as_ref().filter(|rows| rows.shift() > 0) else {
            return false;
        };
        local.y() >= -metrics.height && local.x() >= rows.indent(0, metrics)
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
        let rows = Rows::of(
            editor.text(),
            editor.caret(),
            width,
            metrics,
            self.budget,
            self.inline,
        );
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
        let budget = row_budget(constraint.max.y(), metrics, self.pane_rows());
        let editor = self.input.editor();
        let rows = Rows::of(
            editor.text(),
            editor.caret(),
            width,
            metrics,
            budget,
            self.inline,
        );
        // Short by the row shared with the prompt, which the list above has
        // already been given the space for. A one-line field that continues a
        // prompt therefore measures zero and the whole column is a row
        // shorter, which is the point: the line being typed is *on* the
        // prompt's row rather than under it.
        let height = rows.height(metrics);
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

        // **A layer, and only when the first row continues the prompt.** That
        // row was painted by the list above, which paints into a layer of its
        // own — and layers composite in the order they were started, so text
        // drawn back into the enclosing one would end up *under* the ground
        // the list filled its box with. A layer started here is later than the
        // list's and therefore over it. It is not clipped to this element
        // either: the row it needs is outside the box, so it inherits the
        // pane's clip instead.
        let layered = rows.shift() > 0;
        if layered {
            ctx.scene.start_layer(ClipBounds::ActiveLayer);
        }
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        paint_input(
            &self.input,
            &self.font,
            origin,
            rows,
            self.focused,
            self.ink,
            ctx.scene,
        );
        if layered {
            ctx.scene.stop_layer();
        }
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
    /// The column the first drawn row starts at, when that row continues the
    /// shell's prompt, and `None` when every row starts at the gutter.
    ///
    /// Resolved here rather than taken on trust, because the answer depends on
    /// the wrapping this type just did: a field scrolled past its own first
    /// row has nothing on screen that continues the prompt.
    inline: Option<usize>,
}

impl Rows {
    /// The rows of `text` a field this wide draws, scrolled so that `caret` is
    /// among them.
    ///
    /// `inline` is the column the first row starts at when it continues the
    /// prompt above — see [`super::block_list::inline_start`] — which is a
    /// column that row does not have for text of its own.
    fn of(
        text: &str,
        caret: usize,
        width: f32,
        metrics: CellMetrics,
        budget: usize,
        inline: Option<usize>,
    ) -> Self {
        let budget = budget.max(1);
        let columns = columns_for(width, metrics);
        // Never zero: a field one cell wide still has to make progress, which
        // is the same floor `columns_for` applies.
        let first_columns = inline.map_or(columns, |start| columns.saturating_sub(start).max(1));
        let mut rows = VecDeque::with_capacity(budget);

        // The caret is kept on the last row of the window rather than the
        // first: a command is written left to right and downwards, so the rows
        // that matter are the ones behind the caret. Only when there are not
        // that many behind it does the window run on past it.
        for row in RowWalk::new(text, columns, first_columns) {
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

        let rows: Vec<_> = rows.into();
        // The offset is the *first* row of the text's, and the window has to
        // still hold it. Once a runaway command has scrolled that row away,
        // the row at the top of the window is a middle one, which continues
        // nothing — and every row it does hold was wrapped at the full width,
        // so drawing them all at the gutter is exactly right.
        let inline = inline.filter(|_| rows.first().is_some_and(|row| row.start == 0));
        Self { rows, inline }
    }

    /// How many rows are drawn.
    fn drawn(&self) -> usize {
        self.rows.len()
    }

    /// How many of the drawn rows are drawn above the field's own box: one
    /// when the first row continues the prompt, and none otherwise.
    ///
    /// The field's height is short by exactly this, which is how the row it
    /// shares with the prompt is paid for.
    fn shift(&self) -> usize {
        usize::from(self.inline.is_some())
    }

    /// How tall the field is, which is the rows it draws minus the one it
    /// shares.
    fn height(&self, metrics: CellMetrics) -> f32 {
        (self.drawn() - self.shift()) as f32 * metrics.height
    }

    /// How far into the row the text of row `index` starts, in pixels.
    ///
    /// Only ever the first row's, and only when it continues the prompt: a
    /// wrapped line carries on at the gutter, the way a shell's own line
    /// editor wraps it.
    fn indent(&self, index: usize, metrics: CellMetrics) -> f32 {
        if index == 0 {
            self.inline.unwrap_or(0) as f32 * metrics.width
        } else {
            0.
        }
    }

    /// Where row `index` is drawn, relative to the field's own origin.
    ///
    /// The y is negative for the row shared with the prompt, which is the
    /// whole of how that row is reached: it was painted by the list above and
    /// this element draws into what is left of it.
    fn offset(&self, index: usize, metrics: CellMetrics) -> Vector2F {
        vec2f(
            self.indent(index, metrics),
            (index as f32 - self.shift() as f32) * metrics.height,
        )
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
    ///
    /// `local` is measured from the field's own origin, so the row it shares
    /// with the prompt is at a negative y and its first cell is
    /// [`Self::indent`] to the right — which is the same arithmetic
    /// [`Self::offset`] paints with, inverted, so a click lands on the
    /// grapheme it was aimed at rather than one column out.
    fn at_point(&self, text: &str, local: Vector2F, metrics: CellMetrics) -> usize {
        let row = (local.y() / metrics.height + self.shift() as f32)
            .floor()
            .clamp(0., (self.drawn() - 1) as f32) as usize;
        let column = ((local.x() - self.indent(row, metrics)) / metrics.width)
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
    /// How many fit on the *first* row, which is shorter when that row
    /// continues the shell's prompt: the prompt has already used the left of
    /// it. The same as [`Self::columns`] otherwise.
    first_columns: usize,
    /// Whether the next row produced is the first one.
    first: bool,
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
    fn new(text: &'a str, columns: usize, first_columns: usize) -> Self {
        let line_end = line_end_from(text, 0);
        Self {
            text,
            columns,
            first_columns,
            first: true,
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

        // The first row is the short one when it continues the prompt; every
        // row after it has the whole width, because a wrapped line carries on
        // at the gutter.
        let columns = if self.first {
            self.first_columns
        } else {
            self.columns
        };
        self.first = false;

        let start = self.at;
        let (bytes, filled) = take_cells(
            &self.text[start..self.line_end],
            columns,
            self.line_is_ascii,
        );
        let end = start + bytes;

        if end < self.line_end {
            // More of this line to come.
            self.at = end;
        } else if self.line_end == self.text.len() {
            self.at = end;
            self.trailing = filled == columns;
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

/// How many rows the composer may draw.
///
/// Never more than [`MAX_ROWS`], and never more than half the pane: the
/// composer grows downwards *into the output*, and one that took the whole
/// panel would leave the shell it is composing for nothing to be seen in.
/// Never fewer than one, because a pane can be dragged shorter than a cell and
/// a composer with no rows would have no caret.
///
/// `available` is what the parent offered, and it is usually infinite —
/// measuring a non-flexible flex child *is* asking it how big it would like to
/// be. Half of infinity is not an answer, so the fallback is `pane_rows`: the
/// grid the pty was told about, which is the pane minus its own insets and
/// therefore exactly the number the rule is about.
fn row_budget(available: f32, metrics: CellMetrics, pane_rows: usize) -> usize {
    let fits = if available.is_finite() {
        (available / metrics.height).floor().max(0.) as usize
    } else {
        pane_rows
    };
    (fits / 2).clamp(1, MAX_ROWS)
}

/// How many cells of text a composer this wide holds. Never fewer than one: a
/// pane can be dragged narrower than a cell.
///
/// The same count the grid above gets from the same width, because the two
/// share a column zero and a right edge — which is what makes a line that
/// wraps in the composer wrap in the same place when the shell echoes it back.
fn columns_for(width: f32, metrics: CellMetrics) -> usize {
    ((width / metrics.width).floor().max(0.) as usize).max(1)
}

/// Paints one composer into `scene` from `origin`, in the rows a layout
/// wrapped.
///
/// `focused` is whether this pane's composer is the one receiving keys, and it
/// is the caret's business and nothing else's. **There is no other focus
/// affordance**: no ring, no border, no tint, no change of ground. That is
/// Warp's answer and it is the whole reason the composer reads as the next
/// line of the terminal rather than as a widget bolted under it — a frame that
/// lights up when you click it is a form field, whatever colour it is.
///
/// **And there is no prompt glyph.** The shell's own prompt is the row the
/// first line continues, and a second invented one under it is two prompts on
/// screen, which is what kills the illusion. Column zero here is column zero
/// there.
///
/// Every row is placed through [`Rows::offset`], which is the one place that
/// knows the first row may start part-way along the row above. The glyphs, the
/// selection and the caret all go through it, so none of the three can end up
/// a column or a row away from the other two.
fn paint_input(
    input: &TextInput,
    font: &CellFont,
    origin: Vector2F,
    rows: &Rows,
    focused: bool,
    ink: Ink,
    scene: &mut Scene,
) {
    let metrics = font.metrics();
    let editor = input.editor();
    let text = editor.text();

    let selection = editor.selection().range();
    let caret = (focused && input.caret_is_visible())
        .then(|| rows.place(text, editor.caret()))
        .flatten();

    for (row, range) in rows.iter().enumerate() {
        let at = origin + rows.offset(row, metrics);
        paint_selection(text, range, &selection, at, metrics, scene);

        let mut column = 0;
        for grapheme in text[range.clone()].graphemes(true) {
            let pen = vec2f(
                at.x() + column as f32 * metrics.width,
                at.y() + metrics.baseline,
            );
            // Every character of a cluster shares one pen: a combining mark
            // carries its own offset from the character it sits on, and has no
            // cell of its own to sit in.
            for character in grapheme.chars() {
                paint_character(character, pen, ink.text, font, scene);
            }
            column += cells(grapheme);
        }
    }

    // Last, so that it is drawn over the selection it may be sitting in. In
    // the *terminal's* cursor colour, which is the one the grid paints the
    // shell's own cursor in: a caret in the accent would be a form field's,
    // and would disagree with the block above it.
    if let Some((row, column)) = caret {
        let height = metrics.height * CARET_HEIGHT;
        let at = origin + rows.offset(row, metrics);
        scene
            .draw_rect_without_hit_recording(RectF::new(
                vec2f(
                    at.x() + column as f32 * metrics.width,
                    at.y() + (metrics.height - height) / 2.,
                ),
                vec2f(CARET_WIDTH, height),
            ))
            .with_background(ink.caret)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARET_WIDTH / 2.)));
    }
}

/// Fills the cells of one row that are inside the selection.
///
/// One rectangle per row rather than per cell, for the reason the grid merges
/// its background runs: a selected line is one quad, not eighty.
///
/// `at` is where the row's first cell is drawn, which on a row that continues
/// the prompt is already past it — so a selection on that row starts where its
/// text does rather than at the gutter.
fn paint_selection(
    text: &str,
    row: &Range<usize>,
    selection: &Range<usize>,
    at: Vector2F,
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
            vec2f(at.x() + from as f32 * metrics.width, at.y()),
            vec2f((to - from) as f32 * metrics.width, metrics.height),
        ))
        .with_background(theme().selection);
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
