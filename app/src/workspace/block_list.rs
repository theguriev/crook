//! The pane's output as a list of blocks: one element that owns its own scroll
//! offset and paints only what is in view.
//!
//! # Why this is not a `Scrollable`
//!
//! [`Scrollable`](crookui_core::elements::Scrollable) lays its child out with
//! an infinite height and paints all of it. It is a clipper, not a
//! virtualiser: handing it a session's worth of blocks means laying out and
//! painting every one of them, sixty times a second, to show twenty. So this
//! element owns the offset, consumes the wheel and paints its own thumb —
//! reusing that element's thumb geometry so the two scrollbars in the
//! application match.
//!
//! # What it costs
//!
//! A [`Heights`] prefix sum over the items, kept in the view between frames
//! and rebuilt only when a command finishes. Layout binary-searches it for the
//! first visible item and walks forward until it passes the bottom of the box,
//! recording a small vector of what it found; paint replays that vector. So a
//! frame is `O(log n)` in the number of blocks plus `O(k)` in the number of
//! *visible* ones — and a block taller than the pane costs only the rows on
//! screen, because the same arithmetic is applied a second time inside it.
//!
//! # What is drawn from where
//!
//! A finished block's rows were harvested out of the emulator when it closed
//! and live in a compact per-block store; they are materialised one row at a
//! time into a scratch `Vec<SnapshotCell>` that is reused down the whole list
//! and handed to the same monospace glyph path
//! [`TerminalElement`](super::TerminalElement) uses. The open block is painted
//! straight from the [`Snapshot`], from the rows its anchor names and no
//! others — the rows above those are stale copies of blocks already harvested.
//!
//! # What this does not do, and where the rest is
//!
//! **Selection is still the emulator's**, so it works inside the open block
//! and nowhere else: a finished block's cells are no longer in the emulator to
//! drag across. Copying a whole finished block needs no selection and is
//! exact — that is what the hover control does. Cross-block selection,
//! multi-block selection, keyboard block navigation, the sticky header and the
//! jump-to-bottom button are all deliberately absent; see `docs/blocks.md`.

use std::sync::Arc;
use std::time::Instant;

use crook_terminal::{Block, BlockId, CellSide, LiveBlock, Snapshot, SnapshotCell, ViewportPoint};
use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::{DispatchedEvent, Event, MouseButton};
use crookui_core::geometry::{Point, RectF, Vector2F, vec2f};
use crookui_core::icons::{IconKey, Lucide};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};
use crookui_core::scene::{ClipBounds, CornerRadius, Radius, Scene};

use crate::clipboard::Clipboard;
use crate::pane_blocks::{PaneBlocks, ScrollCause};
use crate::pane_input::PaneInput;
use crate::pane_selection::PaneSelection;
use crate::pane_surface;
use crate::tab::PaneId;
use crate::terminal_font::{CellFont, CellMetrics};
use crate::terminal_model::{BlockHistory, TerminalHandle};
use crate::theme::theme;

use super::pane_output::{Keys, Output, Typed, selection_kind};
use super::terminal_element::{self, Ink, RowMarks, color};

/// The gap between a pane's edge and the text inside it.
///
/// One number used three times: the left inset of every block's rows, the
/// right inset that bounds them, and the composer's own left padding. That is
/// what puts a command inside a block and the line being typed under it on the
/// same x — see `body::GUTTER`, which is this, and why raising it costs
/// columns.
pub(super) const GUTTER: f32 = 16.;

/// The space above a block's first row, in lines.
///
/// Warp's `padding_top` for its normal spacing. Together with
/// [`PADDING_BOTTOM`] it is the whole of what separates two commands: there is
/// no margin object and no inter-block gap.
const PADDING_TOP: f32 = 1.1;

/// The space below a block's last row, in lines.
const PADDING_BOTTOM: f32 = 1.;

/// The space below the open block's last row when there is no composer under
/// it, in lines.
///
/// Warp's `padding_bottom` for a long-running block: enough that the last row
/// is not touching the window's edge, little enough that a program filling the
/// pane fills it.
const RUNNING_PADDING_BOTTOM: f32 = 0.2;

/// The hairline drawn along the top edge of every block but the first.
const DIVIDER: f32 = 1.;

/// How wide the stripe down the left of a block that failed or is running.
const STRIPE: f32 = 5.;

/// How much of the theme's red a failed block is washed in.
///
/// Ten per cent, which is Warp's: enough to find the command that broke by
/// scanning rather than by reading, little enough that the output over it is
/// still output rather than red text.
const WASH_ALPHA: u8 = 26;

/// The side of the square copy control.
const CONTROL_SIZE: f32 = 26.;

/// How far the control's right edge sits in from the pane's, which is what
/// keeps it clear of the thumb.
const CONTROL_INSET: f32 = 12.;

/// How far below a block's top edge the control sits.
///
/// Warp's `overflow_offset`. Below the divider rather than on it, and above
/// the first row, so the control lands on the block's prompt line and never on
/// its output.
const CONTROL_OFFSET: f32 = 12.;

/// How round the control's plate is.
const CONTROL_RADIUS: f32 = 5.;

/// The icon inside that plate, centred.
///
/// Two overlapping sheets, which used to be two rectangles here because there
/// was no icon system to ask — see `crookui_core::icons`. Lucide's `copy` is
/// the same drawing, and it does not need the front sheet filled with the
/// plate's colour to read as two: its back sheet is only the L behind the
/// front one.
const CONTROL_ICON_SIZE: f32 = 15.;

/// The thumb, in the geometry [`Scrollable`](crookui_core::elements::Scrollable)
/// paints its own with, so the two scrollbars in the application match.
const THUMB_WIDTH: f32 = 4.;
/// How far the thumb sits in from the right edge, and from the top and bottom.
const THUMB_INSET: f32 = 2.;
/// The shortest the thumb is allowed to get.
const THUMB_MIN_LENGTH: f32 = 24.;

/// One pane's output, as a list of blocks.
pub struct BlockList {
    /// The commands that have finished, oldest first.
    blocks: Arc<BlockHistory>,
    /// The grid, which is where the open block's rows are.
    snapshot: Arc<Snapshot>,
    font: CellFont,

    /// Where the list is scrolled to and what the pointer is over, which
    /// outlive this element by every frame the pointer takes to move.
    view: PaneBlocks,

    /// The keyboard and the selection, shared with the grid element. See
    /// [`Output`].
    output: Output,

    size: Option<Vector2F>,
    origin: Option<Point>,

    /// What layout found in view, replayed by paint. Walked once per frame
    /// rather than once per pass.
    window: Vec<Visible>,
    /// One row's cells, reused down the whole list.
    scratch: Vec<SnapshotCell>,
}

/// One item of the list that layout found in view.
#[derive(Copy, Clone, Debug)]
struct Visible {
    /// Which item: an index into the finished blocks, or one past the last of
    /// them for the open block.
    index: usize,
    /// Where its top edge is, in pixels from the element's origin. Negative
    /// for the item the viewport starts inside.
    top: f32,
    /// How tall it is, in pixels.
    height: f32,
}

impl BlockList {
    /// A list of `blocks` with `snapshot`'s open block at the end of it,
    /// driven by nothing.
    pub fn new(
        blocks: Arc<BlockHistory>,
        snapshot: Arc<Snapshot>,
        font: CellFont,
        view: PaneBlocks,
    ) -> Self {
        Self {
            blocks,
            snapshot,
            font,
            view,
            output: Output::detached(),
            size: None,
            origin: None,
            window: Vec::new(),
            scratch: Vec::new(),
        }
    }

    /// Attaches the terminal the blocks came from, so the list can be typed
    /// into as far as `keys` allows.
    pub fn with_terminal(mut self, handle: TerminalHandle, keys: Keys) -> Self {
        self.output = self.output.with_terminal(handle, keys);
        self
    }

    /// Attaches the composer under this list, whose line decides what Ctrl-D
    /// means.
    pub fn with_input(mut self, input: PaneInput) -> Self {
        self.output = self.output.with_input(input);
        self
    }

    /// Makes the open block selectable and the copy control useful: `gesture`
    /// is the press this pane has open, `clipboard` is where a copy goes, and
    /// `pane` is who the release is dispatched for.
    pub fn with_selection(
        mut self,
        pane: PaneId,
        gesture: PaneSelection,
        clipboard: Clipboard,
    ) -> Self {
        self.output = self.output.with_selection(pane, gesture, clipboard);
        self
    }

    /// The item index of the open block.
    fn live_index(&self) -> usize {
        self.blocks.len()
    }

    /// The block at an item index, or `None` for the open one.
    fn block(&self, index: usize) -> Option<&Arc<Block>> {
        self.blocks.get(index)
    }

    /// The id of whatever is at an item index.
    fn id(&self, index: usize) -> BlockId {
        self.block(index)
            .map_or(self.snapshot.live_block.id, |block| block.id)
    }

    /// The rows of the snapshot the open block occupies, or `None` when it has
    /// printed nothing yet.
    fn live_rows(&self) -> Option<(usize, usize)> {
        live_rows(&self.snapshot)
    }

    /// What this pane is drawing, as of now.
    ///
    /// Asked again rather than passed in: layout and paint both need it, and a
    /// command crosses the long-running threshold between two frames rather
    /// than at one the body happened to build.
    fn surface(&self) -> pane_surface::PaneSurface {
        pane_surface::of(&self.snapshot, Instant::now())
    }

    /// The column the composer's first row starts at on this list's last row,
    /// when it continues the prompt drawn there.
    ///
    /// The same answer `body` gives the composer, from the same function and
    /// the same three facts, which is what stops the two elements disagreeing
    /// about who owns the row they share.
    fn inline_start(&self) -> Option<usize> {
        inline_start(&self.snapshot, self.surface(), self.view.is_cut_off())
    }

    /// Whether a press belongs to the composer below rather than to this list.
    ///
    /// **The prompt's row belongs to two elements at once**, and the split is
    /// by column: the prompt on the left is output, which a drag selects, and
    /// everything from the composer's first cell rightwards is the line being
    /// typed. Both elements are handed every press — a flex offers an event to
    /// all of its children — so exactly one of them has to decline, and it is
    /// this one, because the composer is the element that knows where its own
    /// caret would go.
    fn press_is_the_composer_s(&self, position: Vector2F) -> bool {
        let Some(start) = self.inline_start() else {
            return false;
        };
        let Some((_, last)) = self.live_rows() else {
            return false;
        };
        self.cell_at(position)
            .is_some_and(|(at, _)| at.row == last && at.column >= start)
    }

    /// Brings the height index up to date with this frame's blocks, and walks
    /// out the items that are in view.
    fn measure(&mut self, size: Vector2F) {
        let metrics = self.font.metrics();
        let live = live_height(&self.snapshot, self.surface().composer);
        // The pointer alone would be enough for as long as an `Arc` lives,
        // and the count and the eviction mark are what keep it honest across a
        // free: a new history can land on a freed one's address, and only the
        // eviction mark moves when the front of the list does rather than the
        // back.
        let identity = (
            Arc::as_ptr(&self.blocks) as usize,
            self.blocks.len(),
            self.blocks.evicted(),
        );

        let viewport = size.y() / metrics.height;
        let content = self.view.with_heights(|heights| {
            heights.sync(
                identity,
                self.blocks.iter().map(|block| block_height(block)),
                live,
            );
            heights.total()
        });

        // Measured before the window is walked, because the offset the walk
        // starts from is resolved against exactly these numbers.
        self.view.measured(content, viewport);
        let offset = self.view.offset();
        let bottom = offset + viewport;

        // **A short list sits on the bottom of its box, not the top.** The
        // composer is pinned under the output, and the whole of why it reads
        // as the next line of the terminal rather than as a widget is that the
        // open block's last row is immediately above it. Top-aligning a
        // two-block session would put the prompt at the top of the pane and
        // the caret four hundred pixels below it.
        let lead = (viewport - content).max(0.) * metrics.height;

        self.window.clear();
        self.view.with_heights(|heights| {
            let mut index = heights.seek(offset);
            while index < heights.len() && heights.start(index) < bottom {
                self.window.push(Visible {
                    index,
                    top: lead + (heights.start(index) - offset) * metrics.height,
                    height: heights.height(index) * metrics.height,
                });
                index += 1;
            }
        });
    }

    /// The rectangle a block's copy control is drawn in, given where the block
    /// starts.
    ///
    /// Measured from the *pane's* right edge, which is this element's — it is
    /// laid out full width and applies the gutter itself — so the control
    /// clears the thumb rather than landing under it the first time a session
    /// grows long enough to scroll.
    fn control_at(&self, origin: Vector2F, item: Visible) -> RectF {
        let size = self.size.unwrap_or_default();
        // Below the block's top edge, or below the top of the *visible* part
        // of it when that edge has scrolled out of the list — a block taller
        // than the pane is the one most worth copying, and a control anchored
        // to a top nobody can see is painted outside the clip and cannot be
        // clicked. It still never leaves the block: on a short one at the
        // bottom of the window it stays inside its own rows.
        let lowest = item.top + item.height - CONTROL_SIZE;
        let top = (item.top.max(0.) + CONTROL_OFFSET)
            .min(lowest)
            .max(item.top.max(0.));

        RectF::new(
            origin + vec2f(size.x() - CONTROL_INSET - CONTROL_SIZE, top),
            vec2f(CONTROL_SIZE, CONTROL_SIZE),
        )
    }

    /// Which item a window position is over, and whether it is over that
    /// item's copy control.
    ///
    /// `None` for a position outside the list, and for the open block: there
    /// is nothing to copy out of a command that has not finished, and its rows
    /// are still in the grid where the pointer can select them.
    fn item_at(&self, position: Vector2F) -> Option<(Visible, bool)> {
        let bounds = self.bounds()?;
        if !bounds.contains_point(position) {
            return None;
        }
        let local = position.y() - bounds.origin().y();
        let item = self
            .window
            .iter()
            .find(|item| local >= item.top && local < item.top + item.height)?;
        if item.index == self.live_index() {
            return None;
        }
        Some((
            *item,
            self.control_at(bounds.origin(), *item)
                .contains_point(position),
        ))
    }

    /// The cell of the *viewport* a window position lands on, and which half of
    /// it the pointer is on.
    ///
    /// Only the open block answers: it is the only item whose rows are still
    /// in the emulator, and the emulator is where a selection has to live to
    /// stay anchored to its text while the shell prints. A position above the
    /// open block clamps to its first row and one below it to its last, which
    /// is what a drag that has left the block means.
    fn cell_at(&self, position: Vector2F) -> Option<(ViewportPoint, CellSide)> {
        let bounds = self.bounds()?;
        let live = *self
            .window
            .iter()
            .find(|item| item.index == self.live_index())?;
        let (first, last) = self.live_rows()?;

        let metrics = self.font.metrics();
        let local = position - bounds.origin();
        let rows_top = live.top + PADDING_TOP * metrics.height;
        let row = ((local.y() - rows_top) / metrics.height)
            .floor()
            .clamp(0., (last - first) as f32) as usize;

        let columns = usize::from(
            metrics
                .grid_for(bounds.width() - GUTTER * 2., bounds.height())
                .0,
        )
        .min(self.snapshot.columns);
        if columns == 0 {
            return None;
        }
        let (column, side) =
            terminal_element::column_at(local.x() - GUTTER, metrics.width, columns);
        Some((ViewportPoint::new(first + row, column), side))
    }

    /// How far to scroll before a drag lands, when the pointer has left the
    /// top or the bottom of the list, in lines.
    fn autoscroll(&self, position: Vector2F) -> f32 {
        let Some(bounds) = self.bounds() else {
            return 0.;
        };
        let height = self.font.metrics().height;
        let past = if position.y() < bounds.min_y() {
            position.y() - bounds.min_y()
        } else if position.y() > bounds.max_y() {
            position.y() - bounds.max_y()
        } else {
            return 0.;
        };

        // Bounded by a screenful per event, so that flinging the pointer at the
        // bottom of the window does not skip the output it was dragging over.
        let rows = bounds.height() / height;
        (past / height).clamp(-rows, rows)
    }

    /// Moves the list, if the wheel turned over it.
    fn scroll(&self, event: &Event, ctx: &mut EventContext) -> bool {
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

        let height = self.font.metrics().height;
        // Negated: a wheel reports positive-up, and an offset counts
        // positive-down the list.
        let lines = -delta.to_pixels(height).y() / height;
        if !self.view.apply(ScrollCause::Wheel(lines)) {
            // At an end with nothing to move. Declining stops a repaint that
            // would draw an identical frame.
            return false;
        }
        ctx.notify();
        true
    }

    /// Follows the pointer, so that the block under it shows its copy control.
    fn hover(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
        self.view.point(Some(position));
        if self.rehover() {
            ctx.notify();
        }
        // Never handled: a hover is not a claim on the event, and the pane
        // under this one wants to know about the same move.
        false
    }

    /// Puts the copy control on whichever block is under the pointer *now*,
    /// reporting whether that changed.
    ///
    /// Asked on every pointer move and again on every paint, because the list
    /// moves under a pointer that does not: a wheel, a command finishing and a
    /// resize all put a different block under it with no mouse event to say
    /// so, and a control drawn on the block that *was* there is an affordance
    /// pointing at output a click would not copy.
    fn rehover(&self) -> bool {
        // Only where there is a pointer to follow. A hover armed by
        // `--hover-block` for an unattended picture has none, and answering
        // "the pointer is over nothing" would take it straight back off.
        let Some(at) = self.view.pointer() else {
            return false;
        };
        let over = self.item_at(at);
        let block = over.map(|(item, _)| self.id(item.index));
        self.view.hover(block, over.is_some_and(|(_, on)| on))
    }

    /// Presses either a block's copy control or a selection into the open
    /// block.
    fn press(&self, event: &Event, ctx: &mut EventContext) -> bool {
        let Event::MouseDown {
            button: MouseButton::Left,
            position,
            click_count,
            modifiers,
        } = event
        else {
            return false;
        };

        if let Some((item, true)) = self.item_at(*position) {
            self.view.press_control(self.id(item.index));
            ctx.notify();
            return true;
        }

        let Some(bounds) = self
            .bounds()
            .filter(|bounds| bounds.contains_point(*position))
        else {
            return false;
        };

        // **A press on a finished block does not start a selection.** Its
        // cells were harvested out of the emulator when the command ended, and
        // a selection has to live there to stay anchored to its text while the
        // shell prints — so there is nothing under the pointer to drag out.
        // Letting go of whatever *was* selected is still right, for the same
        // reason a plain click anywhere on the output is.
        let live_top = self
            .window
            .iter()
            .find(|item| item.index == self.live_index())
            .map_or(f32::INFINITY, |item| item.top);
        if position.y() - bounds.origin().y() < live_top {
            self.output.release_selection(ctx);
            return false;
        }

        // The row the composer's first line continues is half this list's and
        // half the composer's. See `press_is_the_composer_s`: the composer
        // clears the output's selection itself, as every press into it does,
        // so there is nothing to do here but stand aside.
        if self.press_is_the_composer_s(*position) {
            return false;
        }

        let Some((at, side)) = self.cell_at(*position) else {
            return false;
        };
        self.output
            .press(at, side, selection_kind(*click_count, modifiers.alt), ctx)
    }

    /// Copies a block whose control was pressed and released, or ends a
    /// selection gesture.
    fn release(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
        if let Some(pressed) = self.view.release_control() {
            // Only where it went down, which is what every other button in the
            // application does.
            let on_it = matches!(self.item_at(position), Some((item, true)) if self.id(item.index) == pressed);
            if on_it && self.copy_block(pressed) {
                ctx.notify();
            }
            return true;
        }
        self.output.release()
    }

    /// Puts one finished block's command and output on the clipboard.
    ///
    /// Exactly that block's text, with no neighbour's and no trailing blank
    /// rows: the rows were harvested when the command ended, so this is what
    /// was on screen and nothing else. This is the thing scrollback cannot do.
    fn copy_block(&self, id: BlockId) -> bool {
        let Some(block) = self.blocks.iter().find(|block| block.id == id) else {
            return false;
        };
        self.output.copy(block.rows.to_text().trim_end())
    }

    /// Paints one item of the list.
    fn paint_item(&mut self, origin: Vector2F, item: Visible, ctx: &mut PaintContext) {
        let metrics = self.font.metrics();
        let size = self.size.unwrap_or_default();
        let top = origin.y() + item.top;
        let ground = self.snapshot.background;

        match self.verdict(item.index) {
            Verdict::Failed => {
                let red = theme().terminal.normal[1];
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(origin.x(), top),
                        vec2f(size.x(), item.height),
                    ))
                    .with_background(red.with_alpha(WASH_ALPHA));
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(origin.x(), top),
                        vec2f(STRIPE, item.height),
                    ))
                    .with_background(red);
            }
            // A command in flight says so with the same stripe in the colour
            // the rest of the application uses for "this is happening now".
            Verdict::Running => {
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        vec2f(origin.x(), top),
                        vec2f(STRIPE, item.height),
                    ))
                    .with_background(theme().accent);
            }
            Verdict::Plain => {}
        }

        // Over the wash rather than under it, and only above a block that has
        // one before it: a rule along the very top of the pane separates the
        // pane from nothing.
        if item.index > 0 || self.blocks.evicted() > 0 {
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    vec2f(origin.x(), top),
                    vec2f(size.x(), DIVIDER),
                ))
                .with_background(theme().overlay_2);
        }

        let left = origin.x() + GUTTER;
        let rows_top = top + PADDING_TOP * metrics.height;
        let columns = usize::from(metrics.grid_for(size.x() - GUTTER * 2., size.y()).0);

        // The rows of *this* item that are on screen. The same arithmetic the
        // list does over its items, applied inside one of them, which is what
        // keeps a fifty-thousand-row block costing a screenful.
        let first_visible = ((origin.y() - rows_top) / metrics.height).floor().max(0.) as usize;
        let last_visible = ((origin.y() + size.y() - rows_top) / metrics.height).ceil() as usize;

        match self.block(item.index).cloned() {
            Some(block) => {
                let rows = block.rows.rows().min(last_visible);
                for row in first_visible..rows {
                    let marks = block.rows.materialise(row, &mut self.scratch);
                    let cells = &self.scratch[..self.scratch.len().min(columns)];
                    let top = rows_top + row as f32 * metrics.height;

                    // The same passes in the same order as a live row, because
                    // a command that has ended is not drawn differently from
                    // one still printing. The store keeps every cell's
                    // background and every underline — a `git diff` hunk, a
                    // `grep --color` match, `ls`'s directory colours, a
                    // coloured `\e[K` tail, and a powerlevel10k prompt, which
                    // is almost entirely background — and painting the glyphs
                    // alone loses all of it the instant `D` arrives. An
                    // inverted cell loses more than that: the emulator has
                    // already swapped the colours into it, so the character is
                    // drawn in the background colour and disappears.
                    terminal_element::paint_backgrounds(
                        cells, ground, left, top, metrics, ctx.scene,
                    );
                    terminal_element::paint_rules(cells, left, top, metrics, ctx.scene);
                    terminal_element::paint_glyphs(
                        cells,
                        &RowMarks::Block(marks),
                        left,
                        top,
                        &self.font,
                        Ink {
                            ground,
                            inverted: None,
                        },
                        ctx.scene,
                    );
                }
            }
            None => self.paint_live(left, rows_top, columns, first_visible, last_visible, ctx),
        }
    }

    /// Paints the open block, straight from the snapshot.
    ///
    /// Only the rows its anchor names: everything above them is a stale copy
    /// of blocks that have already been harvested into the store, and drawing
    /// those would show the same output twice.
    fn paint_live(
        &mut self,
        left: f32,
        rows_top: f32,
        columns: usize,
        first_visible: usize,
        last_visible: usize,
        ctx: &mut PaintContext,
    ) {
        let metrics = self.font.metrics();
        let Some((first, last)) = self.live_rows() else {
            return;
        };
        let ground = self.snapshot.background;

        let count = last - first + 1;
        for row in first_visible..count.min(last_visible) {
            let source = first + row;
            let cells = &self.snapshot.row(source)[..columns.min(self.snapshot.columns)];
            let top = rows_top + row as f32 * metrics.height;

            terminal_element::paint_backgrounds(cells, ground, left, top, metrics, ctx.scene);
            paint_selection(
                &self.snapshot,
                source,
                cells.len(),
                left,
                top,
                metrics,
                ctx.scene,
            );
            terminal_element::paint_rules(cells, left, top, metrics, ctx.scene);
            terminal_element::paint_glyphs(
                cells,
                &RowMarks::Snapshot {
                    snapshot: &self.snapshot,
                    row: source,
                },
                left,
                top,
                &self.font,
                Ink {
                    ground,
                    inverted: None,
                },
                ctx.scene,
            );
        }

        // **The cursor is drawn only when there is no composer.** While one is
        // up its caret is where typing goes, and the shell's own cursor
        // sitting at the end of a prompt above it would be a second caret
        // claiming the same keyboard. Once the composer has gone — a command
        // has been running long enough to take the space — the program's
        // cursor is the only one there is, and it is filled where this pane
        // has the keyboard and hollow where it does not.
        if self.surface().composer {
            return;
        }
        let Some(cursor) = self
            .snapshot
            .cursor
            .as_ref()
            .filter(|cursor| cursor.row >= first && cursor.row <= last)
        else {
            return;
        };
        let row = cursor.row - first;
        if row < first_visible || row >= last_visible || cursor.column >= columns {
            return;
        }
        terminal_element::paint_cursor_at(
            cursor,
            self.output.keys() == Keys::All,
            vec2f(
                left + cursor.column as f32 * metrics.width,
                rows_top + row as f32 * metrics.height,
            ),
            metrics,
            ctx.scene,
        );
    }

    /// What an item's chrome says about it.
    fn verdict(&self, index: usize) -> Verdict {
        match self.block(index) {
            Some(block) => verdict_of(block),
            None if self.snapshot.live_block.state.is_running() => Verdict::Running,
            None => Verdict::Plain,
        }
    }

    /// Paints the copy control on the hovered block.
    fn paint_control(&self, origin: Vector2F, ctx: &mut PaintContext) {
        let Some(hovered) = self.view.hovered() else {
            return;
        };
        let Some(item) = self
            .window
            .iter()
            .find(|item| self.id(item.index) == hovered && item.index != self.live_index())
        else {
            return;
        };

        let bounds = self.control_at(origin, *item);
        let plate = if self.view.is_on_control() {
            theme().overlay_3
        } else {
            theme().overlay_1
        };
        // Hit-recorded, so that a press on it is a press on the control rather
        // than on the block's text underneath.
        ctx.scene
            .draw_rect_with_hit_recording(bounds)
            .with_background(plate)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CONTROL_RADIUS)));

        let inset = (CONTROL_SIZE - CONTROL_ICON_SIZE) / 2.;
        ctx.scene.draw_icon(
            IconKey::new(Lucide::Copy, CONTROL_ICON_SIZE),
            RectF::new(
                bounds.origin() + Vector2F::splat(inset),
                Vector2F::splat(CONTROL_ICON_SIZE),
            ),
            theme().text_muted,
        );
    }

    /// Paints the thumb, in the geometry the general-purpose scrollable uses.
    fn paint_thumb(&self, origin: Vector2F, ctx: &mut PaintContext) {
        let size = self.size.unwrap_or_default();
        let track = size.y() - THUMB_INSET * 2.;
        let max = self.view.max_offset();
        if !self.view.is_scrollable() || track <= 0. {
            return;
        }

        let metrics = self.font.metrics();
        let viewport = size.y() / metrics.height;
        let content = max + viewport;
        let visible = (viewport / content).clamp(0., 1.);
        let length = (track * visible).max(THUMB_MIN_LENGTH).min(track);
        let progress = self.view.offset() / max;

        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(
                origin
                    + vec2f(
                        size.x() - THUMB_WIDTH - THUMB_INSET,
                        THUMB_INSET + (track - length) * progress,
                    ),
                vec2f(THUMB_WIDTH, length),
            ))
            .with_background(theme().overlay_3)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(THUMB_WIDTH / 2.)));
    }
}

/// What a finished block's chrome says about it.
///
/// Neither Ctrl-C's 130 nor SIGPIPE's 141 is a failure — they are how a
/// command *ends*, and a session full of red every time somebody interrupted
/// one would make the colour mean nothing — and a command that reported no
/// status at all has no verdict to give.
fn verdict_of(block: &Block) -> Verdict {
    match block.exit {
        None | Some(0 | 130 | 141) => Verdict::Plain,
        Some(_) => Verdict::Failed,
    }
}

/// What a block's chrome says about it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Verdict {
    /// Nothing: the ordinary block, which has no box, no border and no fill.
    Plain,
    /// The command reported a non-zero status.
    Failed,
    /// The command is in flight.
    Running,
}

impl Element for BlockList {
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
        self.size = Some(size);

        // The pane resized the pty from its own rectangle before this element
        // was laid out, so a snapshot measured for the grid the last frame had
        // is refreshed here rather than painted into the new box. See
        // `body::PaneSizer` for why the resize is not done here.
        if let Some(handle) = self.output.handle() {
            let (columns, _) = self
                .font
                .metrics()
                .grid_for(size.x() - GUTTER * 2., size.y());
            if usize::from(columns) != self.snapshot.columns {
                self.snapshot = handle.snapshot();
            }
        }

        self.measure(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        let size = self
            .size
            .expect("a block list was painted before it was laid out");

        // Clipped, because an item is painted whole and the ones at the edges
        // of the window hang out of the box at both ends.
        ctx.scene
            .start_layer(ClipBounds::BoundedByActiveLayerAnd(RectF::new(
                origin, size,
            )));
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        // Now that this frame's geometry is settled, and before anything is
        // drawn from it: whatever the list has done since the last pointer
        // move, the control is drawn on the block a click would hit.
        self.rehover();

        // The ground the emulator resolved, painted once. Every cell that
        // agrees with it — which on an idle screen is all of them — then costs
        // nothing.
        ctx.scene
            .draw_rect_without_hit_recording(RectF::new(origin, size))
            .with_background(color(self.snapshot.background));

        for item in std::mem::take(&mut self.window) {
            self.paint_item(origin, item, ctx);
            self.window.push(item);
        }
        self.paint_control(origin, ctx);
        self.paint_thumb(origin, ctx);

        ctx.scene.stop_layer();
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

        // **The rest of a gesture this list already owns is not hit tested.** A
        // press opened it here; where the pointer has got to since is the
        // gesture, not a new one. Dragging out of the pane is how a selection
        // is taken past the end of a line, and the button can come up over a
        // popup painted in a later layer — which `at_z_index` would drop,
        // leaving the gesture open for ever.
        match event.raw_event() {
            Event::MouseDragged {
                button: MouseButton::Left,
                position,
                ..
            } => return self.drag(*position, ctx),
            Event::MouseUp {
                button: MouseButton::Left,
                position,
                ..
            } => return self.release(*position, ctx),
            _ => {}
        }

        // Keystrokes are never filtered by what is painted over them — they
        // are not about a place on screen — but a press, a move and the wheel
        // are, so that a menu open over a pane is neither clicked nor scrolled
        // through.
        let Some(event) = event.at_z_index(z_index, ctx) else {
            // The pointer is over something else, so nothing here is hovered.
            if matches!(event.raw_event(), Event::MouseMoved { .. }) {
                self.view.point(None);
                if self.view.hover(None, false) {
                    ctx.notify();
                }
            }
            return false;
        };

        match self.output.type_key(event, self.snapshot.alt_screen, ctx) {
            // Typing returns to the live block, whatever it was reading.
            Typed::SentToPty => {
                self.view.apply(ScrollCause::KeyToPty);
                ctx.notify();
                return true;
            }
            Typed::Handled => {
                ctx.notify();
                return true;
            }
            Typed::Ignored => {}
        }

        if self.scroll(event, ctx) {
            return true;
        }
        match event {
            Event::MouseMoved { position, .. } => self.hover(*position, ctx),
            Event::MouseDown { .. } => self.press(event, ctx),
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

impl BlockList {
    /// Drags the open end of the selection to the pointer, scrolling the list
    /// when the pointer has left it.
    ///
    /// The *list* scrolls, not the emulator: the emulator's viewport is where
    /// the open block's rows are, and moving it under a selection anchored to
    /// them is the one thing that would make a drag select text nobody
    /// dragged over.
    fn drag(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
        if !self.output.is_dragging() {
            return false;
        }
        let lines = self.autoscroll(position);
        if lines != 0. {
            self.view.apply(ScrollCause::Wheel(lines));
        }
        let Some((at, side)) = self.cell_at(position) else {
            return false;
        };
        self.output.drag(at, side, 0, ctx)
    }
}

/// How tall one finished block is, in lines.
///
/// A block with no command collapses to its rows alone: the padding is what
/// separates one command from the next, and a block that is not a command
/// leaving a two-line hole is exactly the empty chrome blocks exist to avoid.
fn block_height(block: &Block) -> f32 {
    let rows = block.rows.rows() as f32;
    if block.command.is_none() {
        return rows;
    }
    PADDING_TOP + rows + PADDING_BOTTOM
}

/// How tall the open block is, in lines.
///
/// Zero when it has printed nothing, so a prompt that has not arrived yet
/// leaves no gap above the composer.
fn live_height(snapshot: &Snapshot, composer: bool) -> f32 {
    let Some((first, last)) = live_rows(snapshot) else {
        return 0.;
    };
    // With a composer under it, no bottom padding at all: what follows the
    // open block is then not another command but the line being composed at
    // its prompt, and a blank line between a prompt and what is being typed at
    // it is the seam this whole arrangement exists to remove. The gap that
    // separates two *commands* is still there, above every block that has one
    // before it.
    //
    // Without a composer the block runs to the bottom of the pane, and the
    // padding is only what keeps its last row off the window's edge.
    let bottom = if composer { 0. } else { RUNNING_PADDING_BOTTOM };
    PADDING_TOP + (last - first + 1) as f32 + bottom
}

/// The cell column the composer's first row starts at when it continues the
/// shell's own prompt line, or `None` when the composer takes a row of its
/// own.
///
/// **Two cases, and there is deliberately no third.** Either the shell said
/// where its prompt ended — OSC 133 `B`, whose cell the emulator kept as
/// [`LiveBlock::prompt_end`] — and the line being typed continues that row,
/// which is what every other terminal does and what stops a composer reading
/// as a widget however little chrome it has; or nothing said, and the composer
/// starts at the gutter on the row below, exactly where it has always been.
///
/// The missing third option is *guessing*: finding the end of a prompt by
/// pattern-matching what is on screen. There is no pattern. A prompt is
/// whatever `PS1` was set to — a `$`, a `❯`, two lines of segments, a bare
/// space, a right-aligned clock — and a rule that put the caret after the last
/// `>` on the row would land in the middle of somebody's `=>` on one machine
/// in ten. A caret honestly one row down on every machine is better than a
/// caret in the wrong place on some of them, so an unmarked shell keeps
/// today's composer and the fallback is a case rather than a bug.
///
/// Past the mark, three things have to hold, and none of them is about the
/// mark: they are all about the row the composer would be drawn on.
///
/// * The pane draws a block list with a composer under it. On the grid — the
///   alternate screen, an overflowing block — there is no composer at all, and
///   the prompt row is not this list's to draw.
/// * The list is scrolled to its own end, so its last row *is* the row
///   immediately above the composer. Scrolled up, the row above the composer
///   is whatever the wheel left there, and a line typed onto it would land on
///   somebody's output. That is the same condition the rule above the composer
///   is drawn on, which is why the two changes are one change: the frame that
///   gains the seam is the frame the composer takes a row of its own.
/// * The mark is on that last row. A prompt the shell has since scrolled away
///   from fails this, and so does a `B` printed by a file somebody `cat`ted.
///
/// And there has to be a cell left on the row. A prompt that filled its row
/// exactly reports the first column of the row *below* as its end, which is
/// not a row this block has yet — so the composer's own row is where the next
/// character goes anyway, and the fallback is already the right answer.
pub(super) fn inline_start(
    snapshot: &Snapshot,
    surface: pane_surface::PaneSurface,
    cut_off: bool,
) -> Option<usize> {
    if surface.surface != pane_surface::Surface::Blocks || !surface.composer || cut_off {
        return None;
    }
    let prompt = snapshot.live_block.prompt_end?;
    let (_, last) = live_rows(snapshot)?;
    if prompt.row < 0 || prompt.row as usize != last || prompt.column >= snapshot.columns {
        return None;
    }
    Some(prompt.column)
}

/// The first and last viewport rows the open block occupies, or `None` when it
/// has printed nothing yet.
///
/// The anchor is already resolved against the display offset, so this is
/// arithmetic on viewport rows. It is clamped into the grid because the anchor
/// may name a row above the viewport — at which point the pane is drawn as a
/// grid instead and this list is not on screen at all.
fn live_rows(snapshot: &Snapshot) -> Option<(usize, usize)> {
    let LiveBlock {
        top_row,
        bottom_row,
        ..
    } = snapshot.live_block;
    if snapshot.rows == 0 || bottom_row < top_row || bottom_row < 0 {
        return None;
    }
    let first = top_row.max(0) as usize;
    let last = (bottom_row as usize).min(snapshot.rows - 1);
    (first <= last).then_some((first, last))
}

/// An extent to lay out at, given a maximum that may be unbounded.
fn bounded(max: f32, min: f32) -> f32 {
    if max.is_finite() { max } else { min }
}

/// Fills the cells of one open-block row that are inside the selection.
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

#[cfg(test)]
#[path = "block_list_tests.rs"]
mod tests;
