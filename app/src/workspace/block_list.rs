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
//! # Selecting across it
//!
//! A press anywhere in the list starts a selection and a drag takes it as far
//! as it goes — through the finished blocks, through the gaps between them and
//! into the open one, because all of them are items of one address space and
//! none of them is a special case. The anchors, the region and the copy are
//! [`crate::selection`]; what is here is the arithmetic between a pixel and a
//! cell, and the rectangles the highlight is drawn as.
//!
//! Two things the list does *not* draw a selection over. A pane showing one
//! grid rather than a list is [`TerminalElement`](super::TerminalElement)'s,
//! and a selection made here is let go of when the pane crosses to it, because
//! the two number their rows differently — see
//! [`Cells`](crate::selection::Cells). And a block harvested at a wider pane
//! than the one drawing it holds rows that cannot be drawn: the highlight
//! stops with the glyphs, while the copy still takes the whole row.
//!
//! Click-to-select a block, keyboard block navigation, the sticky header and
//! the jump-to-bottom button are all deliberately absent; see
//! `docs/blocks.md`.

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use crook_terminal::url;
use crook_terminal::{Block, BlockId, CellSide, Rows, SelectionKind, Snapshot, SnapshotCell};
use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::{DispatchedEvent, Event, Modifiers, MouseButton};
use crookui_core::geometry::{Point, RectF, Vector2F, vec2f};
use crookui_core::icons::{IconKey, Lucide};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};
use crookui_core::scene::{ClipBounds, CornerRadius, Radius, Scene};

use crate::browser;
use crate::clipboard::Clipboard;
use crate::pane_blocks::{Control, PaneBlocks, ScrollCause};
use crate::pane_link::{LinkRow, LinkSpan, PaneLink};
use crate::pane_selection::PaneSelection;
use crate::pane_surface;
use crate::selection::{Anchor, Blocks, Cells, Item, Region, Selection};
use crate::tab::PaneId;
use crate::terminal_font::{CellFont, CellMetrics};
use crate::terminal_model::{BlockHistory, TerminalHandle};
use crate::text_input::TextInput;
use crate::theme::theme;

use super::action::{BlockAction, WorkspaceAction};
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

/// The side of each square control.
pub(super) const CONTROL_SIZE: f32 = 26.;

/// How far the rightmost control's right edge sits in from the pane's, which
/// is what keeps it clear of the thumb.
pub(super) const CONTROL_INSET: f32 = 12.;

/// The space between the two controls.
///
/// Narrow enough that they read as one group about the block under them,
/// wide enough that the plate behind a hovered one is a plate rather than half
/// of a longer bar.
const CONTROL_GAP: f32 = 4.;

/// How far below a block's top edge the controls sit.
///
/// Warp's `overflow_offset`. Below the divider rather than on it, and above
/// the first row, so the control lands on the block's prompt line and never on
/// its output.
pub(super) const CONTROL_OFFSET: f32 = 12.;

/// How round the control's plate is.
const CONTROL_RADIUS: f32 = 5.;

/// The icon inside a plate, centred.
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
    /// The link under the pointer, which the workspace keeps per pane because
    /// the move that finds one and the frame that underlines it are different
    /// frames. `None` for a list nothing can be clicked in.
    links: Option<PaneLink>,
    /// The block whose menu is up, when one is up over this list.
    ///
    /// The workspace's answer rather than this element's: the menu is an
    /// element the workspace builds, and what the list does with the fact is
    /// keep the block's controls painted under it.
    menu: Option<BlockId>,
    /// Whether there is a menu to open at all, which is whether anything has
    /// contributed to the slot it is drawn from.
    ///
    /// A list built without one draws the copy square alone, in the place the
    /// square has when it is the only control — so switching
    /// [`crook/blocks`](crate::plugins::blocks) off leaves no gap where its
    /// button was.
    menu_available: bool,
}

/// Where one item's rows are painted, and which of them are on screen.
#[derive(Copy, Clone, Debug)]
struct RowBox {
    /// The x the first column starts at.
    left: f32,
    /// The y the item's first row starts at.
    rows_top: f32,
    /// How many columns the list is drawing.
    columns: usize,
    /// The first row of the item that is in view.
    first_visible: usize,
    /// One past the last row of it that is.
    last_visible: usize,
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
            links: None,
            menu: None,
            menu_available: false,
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
    pub fn with_input(mut self, input: TextInput) -> Self {
        self.output = self.output.with_input(input);
        self
    }

    /// Makes the list selectable and the copy control useful: `gesture` is
    /// this pane's selection, `clipboard` is where a copy goes, and `pane` is
    /// who the release is dispatched for.
    pub fn with_selection(
        mut self,
        pane: PaneId,
        gesture: PaneSelection,
        clipboard: Clipboard,
    ) -> Self {
        self.output = self
            .output
            .with_selection(pane, gesture, clipboard, Cells::List);
        self
    }

    /// The blocks a selection in this pane addresses: the finished commands,
    /// then the open one.
    fn addressed(&self) -> Blocks<'_> {
        Blocks::list(&self.blocks, &self.snapshot)
    }

    /// The rows of one item of the list, out of whichever store holds them.
    fn rows_of(&self, index: usize) -> Rows<'_> {
        match self.block(index) {
            Some(block) => Rows::Stored(&block.rows),
            None => match self.live_rows() {
                Some((top, bottom)) => Rows::Live {
                    snapshot: &self.snapshot,
                    top,
                    count: bottom - top + 1,
                },
                // The open block has printed nothing yet, so it holds no rows
                // rather than one blank one.
                None => Rows::Live {
                    snapshot: &self.snapshot,
                    top: 0,
                    count: 0,
                },
            },
        }
    }

    /// How many columns of a row this list actually draws.
    ///
    /// The hit test and the paint both go through it, because a column the
    /// list is too narrow to draw is a column a drag must not be able to
    /// reach.
    fn drawn_columns(&self) -> usize {
        let size = self.size.unwrap_or_default();
        usize::from(
            self.font
                .metrics()
                .grid_for(size.x() - GUTTER * 2., size.y())
                .0,
        )
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
        self.snapshot.live_rows()
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
        let Some((first, last)) = self.live_rows() else {
            return false;
        };
        self.anchor_at(position).is_some_and(|at| {
            at.block == self.snapshot.live_block.id && at.row == last - first && at.column >= start
        })
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

    /// The rectangle one of a block's controls is drawn in, given where the
    /// block starts.
    ///
    /// Measured from the *pane's* right edge, which is this element's — it is
    /// laid out full width and applies the gutter itself — so the controls
    /// clear the thumb rather than landing under it the first time a session
    /// grows long enough to scroll. The menu is the outermost of them, which
    /// is where every application that has both puts it: the row reads
    /// "these, and then everything else".
    fn control_at(&self, origin: Vector2F, item: Visible, control: Control) -> RectF {
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
        // Counted from the outermost control inwards, so that a list with no
        // menu puts its copy square where the menu's dots would have been
        // rather than leaving a hole out at the pane's edge.
        let from_end = self
            .controls()
            .iter()
            .rev()
            .position(|drawn| *drawn == control)
            .unwrap_or(0) as f32;
        let from_right = CONTROL_INSET + from_end * (CONTROL_SIZE + CONTROL_GAP);

        RectF::new(
            origin + vec2f(size.x() - from_right - CONTROL_SIZE, top),
            vec2f(CONTROL_SIZE, CONTROL_SIZE),
        )
    }

    /// Which item a window position is over, and which of that item's controls
    /// it is on, if it is on one at all.
    ///
    /// `None` for a position outside the list, and for the open block: there
    /// is nothing to copy out of a command that has not finished, and its rows
    /// are still in the grid where the pointer can select them.
    fn item_at(&self, position: Vector2F) -> Option<(Visible, Option<Control>)> {
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
        let control = self.controls().iter().copied().find(|control| {
            self.control_at(bounds.origin(), *item, *control)
                .contains_point(position)
        });
        Some((*item, control))
    }

    /// Which cell of the list a window position lands on, and which side of it
    /// the pointer is on.
    ///
    /// Every item answers, not only the open one: a finished block's rows are
    /// in a store rather than in the emulator, and the whole point of the
    /// address space is that this does not matter. Clamped into the list
    /// rather than refused outside it, because a drag that has left the pane is
    /// still selecting — past the bottom means the last row of the last item
    /// on screen, which is the one [`Self::autoscroll`] is about to bring more
    /// of into view.
    fn anchor_at(&self, position: Vector2F) -> Option<Anchor> {
        let bounds = self.bounds()?;
        let metrics = self.font.metrics();
        let local = position - bounds.origin();

        // The item the pointer is inside, or the nearest end of the window.
        let item = *self
            .window
            .iter()
            .find(|item| local.y() < item.top + item.height)
            .or_else(|| self.window.last())?;

        let rows = self.rows_of(item.index);
        let count = rows.count();
        if count == 0 {
            return None;
        }
        let columns = self.drawn_columns().min(rows.columns());
        if columns == 0 {
            return None;
        }

        let rows_top = item.top + self.padding_top(item.index) * metrics.height;
        let row = ((local.y() - rows_top) / metrics.height)
            .floor()
            .clamp(0., (count - 1) as f32) as usize;
        let (column, side) =
            terminal_element::column_at(local.x() - GUTTER, metrics.width, columns);
        Some(Anchor::new(self.id(item.index), row, column, side))
    }

    /// The space above an item's first row, in lines.
    ///
    /// One number, asked by the height index, the painter and the hit test, so
    /// that a click lands on the row it looks like it lands on.
    fn padding_top(&self, index: usize) -> f32 {
        padding_top(self.block(index).map(Arc::as_ref))
    }

    /// Whether the selection this gesture would make covers any cells.
    ///
    /// Only the list can answer it — the anchors name blocks, and the blocks
    /// are here — so it is answered here and handed to the state, which keeps
    /// a selection only once it is a real one.
    fn covers(&self, kind: SelectionKind, anchor: Anchor, head: Anchor) -> bool {
        Selection::new(kind, anchor, head)
            .region(&self.addressed())
            .is_some()
    }

    /// Makes the URLs in this pane's output clickable, through the link state
    /// the workspace keeps for it.
    pub fn with_links(mut self, links: PaneLink) -> Self {
        self.links = Some(links);
        self
    }

    /// Says whether a block can be given a menu at all, and which block's is
    /// up — so the block under an open menu keeps its controls drawn beneath
    /// it.
    pub fn with_menu(mut self, available: bool, open_on: Option<BlockId>) -> Self {
        self.menu_available = available;
        self.menu = open_on.filter(|_| available);
        self
    }

    /// The controls a hovered block carries, outermost last.
    fn controls(&self) -> &'static [Control] {
        if self.menu_available {
            &[Control::Copy, Control::Menu]
        } else {
            &[Control::Copy]
        }
    }

    /// The link under the pointer, or `None`.
    ///
    /// Works on every item of the list rather than only the open one, which is
    /// the difference between this and [`Self::cell_at`]: a selection has to
    /// live in the emulator and so can only cover the open block, but a link
    /// is read straight off the text and a URL printed by a command that
    /// finished an hour ago is still a URL.
    fn link_at(&self, position: Vector2F, modifiers: Modifiers) -> Option<LinkSpan> {
        if !terminal_element::opens_links(modifiers) {
            return None;
        }
        let bounds = self.bounds().filter(|b| b.contains_point(position))?;

        let metrics = self.font.metrics();
        let local = position - bounds.origin();
        let item = *self
            .window
            .iter()
            .find(|item| local.y() >= item.top && local.y() < item.top + item.height)?;

        let rows_top = item.top + PADDING_TOP * metrics.height;
        let row = ((local.y() - rows_top) / metrics.height).floor();
        if row < 0. {
            // The padding above a block's first row, which belongs to no row.
            return None;
        }
        let row = row as usize;

        let columns = usize::from(
            metrics
                .grid_for(bounds.width() - GUTTER * 2., bounds.height())
                .0,
        );
        if columns == 0 {
            return None;
        }
        let (column, _) = terminal_element::column_at(local.x() - GUTTER, metrics.width, columns);

        let (link_row, text) = match self.block(item.index) {
            Some(block) => {
                let text = block.rows.text(row).to_owned();
                (
                    LinkRow::Block {
                        index: item.index,
                        row,
                    },
                    text,
                )
            }
            None => {
                // The open block, whose rows are still the snapshot's.
                let (first, last) = self.live_rows()?;
                let source = first + row;
                if source > last || source >= self.snapshot.rows {
                    return None;
                }
                let text = self
                    .snapshot
                    .row(source)
                    .iter()
                    .map(|cell| cell.c)
                    .collect();
                (
                    LinkRow::Block {
                        index: item.index,
                        row,
                    },
                    text,
                )
            }
        };

        let url = url::at(&text, column)?;
        Some(LinkSpan {
            row: link_row,
            start: url.start,
            len: url.len,
            uri: url.uri,
        })
    }

    /// Finds the link under the pointer, reporting whether the frame changed.
    fn track_link(&self, position: Vector2F, modifiers: Modifiers) -> bool {
        let Some(links) = self.links.as_ref() else {
            return false;
        };
        links.set(self.link_at(position, modifiers))
    }

    /// Opens the link under the pointer, reporting whether there was one.
    fn open_link(&self, position: Vector2F, modifiers: Modifiers) -> bool {
        let Some(link) = self.link_at(position, modifiers) else {
            return false;
        };
        browser::open(&link.uri)
    }

    /// Underlines the link under the pointer, when it is on this row.
    fn paint_link(&self, row: LinkRow, left: f32, top: f32, ctx: &mut PaintContext) {
        let Some(link) = self.links.as_ref().and_then(|links| links.on(row)) else {
            return;
        };
        terminal_element::paint_link_rule(
            vec2f(left, top),
            link.start,
            link.len,
            self.font.metrics(),
            color(self.snapshot.foreground),
            ctx.scene,
        );
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

    /// Follows the pointer, so that the block under it shows its controls.
    fn hover(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
        self.view.point(Some(position));
        if self.rehover() {
            ctx.notify();
        }
        // Never handled: a hover is not a claim on the event, and the pane
        // under this one wants to know about the same move.
        false
    }

    /// Puts the controls on whichever block is under the pointer *now*,
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
        self.view.hover(block, over.and_then(|(_, on)| on))
    }

    /// Presses one of a block's controls, or a selection into the open block.
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

        if let Some((item, Some(control))) = self.item_at(*position) {
            self.view.press_control(self.id(item.index), control);
            ctx.notify();
            return true;
        }

        if !self
            .bounds()
            .is_some_and(|bounds| bounds.contains_point(*position))
        {
            return false;
        }

        // The row the composer's first line continues is half this list's and
        // half the composer's. See `press_is_the_composer_s`: the composer
        // clears the output's selection itself, as every press into it does,
        // so there is nothing to do here but stand aside.
        if self.press_is_the_composer_s(*position) {
            return false;
        }

        let Some(at) = self.anchor_at(*position) else {
            return false;
        };
        let kind = selection_kind(*click_count, modifiers.alt);
        // A double or triple click selects on its own; a single one selects
        // nothing until it is dragged, which is what makes a plain click on
        // the output let go of the last selection.
        let covers = self.covers(kind, at, at);
        self.output
            .press(kind, at, covers, self.snapshot.columns, ctx)
    }

    /// Runs the control that was pressed and released, or ends a selection
    /// gesture.
    fn release(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
        if let Some((pressed, control)) = self.view.release_control() {
            // Only where it went down, and only on the same control: a press
            // that slid from the copy square onto the dots is not a click on
            // either, which is what every other button in the application
            // does.
            let on_it = matches!(
                self.item_at(position),
                Some((item, Some(now))) if self.id(item.index) == pressed && now == control
            );
            if on_it {
                match control {
                    Control::Copy => {
                        if self.copy_block(pressed) {
                            ctx.notify();
                        }
                    }
                    // Dispatched rather than done here: what a menu is and
                    // where it hangs belongs to the workspace, and this
                    // element holds no `&mut` on it. The same arrangement the
                    // selection's release is made with.
                    Control::Menu => {
                        if let Some(pane) = self.output.pane() {
                            ctx.dispatch_typed_action(WorkspaceAction::Block(
                                BlockAction::OpenMenu {
                                    pane,
                                    block: pressed,
                                },
                            ));
                        }
                    }
                }
            }
            return true;
        }
        // The list never gives a press to a program — a block is Crook's own
        // surface, and the grid is where a program that reads the mouse draws
        // — so any gesture open here is a selection.
        self.output.release()
    }

    /// Puts one finished block's command and output on the clipboard.
    ///
    /// Exactly that block's text, with no neighbour's and no trailing blank
    /// rows: the rows were harvested when the command ended, so this is what
    /// was on screen and nothing else. This is the thing scrollback cannot do.
    ///
    /// **Through the same region a drag over the block makes**, rather than
    /// through a second walk of the store. Two implementations disagree, and
    /// these two disagreed about the one thing a terminal must not get wrong
    /// in a copy: a line too long for the pane, which the control used to hand
    /// over with the terminal's own fold turned into a newline. Pasting that
    /// runs a path or a URL as three commands.
    fn copy_block(&self, id: BlockId) -> bool {
        let Some(text) = self.whole_block(id) else {
            return false;
        };
        self.output.copy(text.trim_end())
    }

    /// One whole block as a copy of it would read.
    fn whole_block(&self, id: BlockId) -> Option<String> {
        block_text(&self.addressed(), id, 0)
    }

    /// Paints one item of the list.
    fn paint_item(
        &mut self,
        origin: Vector2F,
        item: Visible,
        region: Option<Region>,
        ctx: &mut PaintContext,
    ) {
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
        let rows_top = top + self.padding_top(item.index) * metrics.height;
        let columns = self.drawn_columns();
        let id = self.id(item.index);
        self.paint_gaps(origin, item, region, rows_top, ctx);

        // The rows of *this* item that are on screen. The same arithmetic the
        // list does over its items, applied inside one of them, which is what
        // keeps a fifty-thousand-row block costing a screenful.
        let first_visible = ((origin.y() - rows_top) / metrics.height).floor().max(0.) as usize;
        let last_visible = ((origin.y() + size.y() - rows_top) / metrics.height).ceil() as usize;

        // Taken before the shadow below: `item` becomes the selection's view
        // of this block, and a link is addressed by the block's position in
        // the list rather than by its identity.
        let listed = item.index;

        match self.block(item.index).cloned() {
            Some(block) => {
                let item = Item {
                    id,
                    rows: Rows::Stored(&block.rows),
                    first: 0,
                };
                let rows = block.rows.rows().min(last_visible);
                for row in first_visible..rows {
                    let selected = region
                        .and_then(|region| region.columns_on(&item, row))
                        .map(|selected| clamped(selected, columns));
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
                    if let Some(selected) = selected {
                        paint_selection(selected, left, top, metrics, ctx.scene);
                    }
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
                    self.paint_link(LinkRow::Block { index: listed, row }, left, top, ctx);
                }
            }
            None => self.paint_live(
                RowBox {
                    left,
                    rows_top,
                    columns,
                    first_visible,
                    last_visible,
                },
                region,
                ctx,
            ),
        }
    }

    /// Fills the padding above and below an item's rows when the selection
    /// runs through the gap into the block on the other side of it.
    ///
    /// See [`paint_gap`] for why it bridges at all.
    fn paint_gaps(
        &self,
        origin: Vector2F,
        item: Visible,
        region: Option<Region>,
        rows_top: f32,
        ctx: &mut PaintContext,
    ) {
        let Some(region) = region else {
            return;
        };
        let metrics = self.font.metrics();
        let block = Item {
            id: self.id(item.index),
            rows: self.rows_of(item.index),
            first: 0,
        };
        let count = block.rows.count();
        if count == 0 {
            return;
        }
        let columns = self.drawn_columns();
        let left = origin.x() + GUTTER;
        let top = origin.y() + item.top;

        // The band takes the columns of the row it continues, rather than the
        // whole width: a run of text leaving the last row of a block leaves it
        // wherever it started on that row, and an alt-drag crossing the gap is
        // a column rather than a bar across the padding.
        let band = |row| {
            region
                .columns_on(&block, row)
                .map(|selected| clamped(selected, columns))
                .filter(|selected| !selected.is_empty())
        };

        // Above the first row, when the selection came into this block from
        // the one before it.
        if let Some(columns) = band(0).filter(|_| region.start.block < block.id) {
            paint_gap(
                RectF::from_points(vec2f(left, top), vec2f(left, rows_top)),
                columns,
                left,
                metrics,
                ctx.scene,
            );
        }
        // And below the last row, when it goes on into the block after this
        // one.
        let bottom = rows_top + count as f32 * metrics.height;
        if let Some(columns) = band(count - 1).filter(|_| region.end.block > block.id) {
            paint_gap(
                RectF::from_points(vec2f(left, bottom), vec2f(left, top + item.height)),
                columns,
                left,
                metrics,
                ctx.scene,
            );
        }
    }

    /// Paints the open block, straight from the snapshot.
    ///
    /// Only the rows its anchor names: everything above them is a stale copy
    /// of blocks that have already been harvested into the store, and drawing
    /// those would show the same output twice.
    fn paint_live(&mut self, at: RowBox, region: Option<Region>, ctx: &mut PaintContext) {
        let RowBox {
            left,
            rows_top,
            columns,
            first_visible,
            last_visible,
        } = at;
        let metrics = self.font.metrics();
        let Some((first, last)) = self.live_rows() else {
            return;
        };
        let ground = self.snapshot.background;
        let item = Item {
            id: self.snapshot.live_block.id,
            rows: Rows::Live {
                snapshot: &self.snapshot,
                top: first,
                count: last - first + 1,
            },
            first: 0,
        };

        let count = last - first + 1;
        for row in first_visible..count.min(last_visible) {
            let source = first + row;
            let cells = &self.snapshot.row(source)[..columns.min(self.snapshot.columns)];
            let top = rows_top + row as f32 * metrics.height;

            terminal_element::paint_backgrounds(cells, ground, left, top, metrics, ctx.scene);
            if let Some(selected) = region
                .and_then(|region| region.columns_on(&item, row))
                .map(|selected| clamped(selected, columns))
            {
                paint_selection(selected, left, top, metrics, ctx.scene);
            }
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
            self.paint_link(
                LinkRow::Block {
                    index: self.live_index(),
                    row,
                },
                left,
                top,
                ctx,
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

    /// Paints the controls on the hovered block, or on the block whose menu is
    /// up.
    ///
    /// The menu keeps them on screen for as long as it is up, and that is not
    /// decoration. A modal menu takes the pointer away from the list
    /// underneath, so the hover it opened from is gone by the next frame — and
    /// the button a menu is hanging off must not be one that has just
    /// disappeared. It is also where the menu's own corner is measured from:
    /// see [`PaneBlocks::set_menu_at`].
    fn paint_control(&self, origin: Vector2F, ctx: &mut PaintContext) {
        let Some(shown) = self.menu.or_else(|| self.view.hovered()) else {
            self.view.set_menu_at(None);
            return;
        };
        let Some(item) = self
            .window
            .iter()
            .find(|item| self.id(item.index) == shown && item.index != self.live_index())
        else {
            self.view.set_menu_at(None);
            return;
        };

        for control in self.controls().iter().copied() {
            let bounds = self.control_at(origin, *item, control);
            // The dots stay lit while their menu is up, because they are what
            // it belongs to.
            let held = self.menu == Some(shown) && control == Control::Menu;
            let plate = if held || self.view.on_control() == Some(control) {
                theme().overlay_3
            } else {
                theme().overlay_1
            };
            // Hit-recorded, so that a press on it is a press on the control
            // rather than on the block's text underneath.
            ctx.scene
                .draw_rect_with_hit_recording(bounds)
                .with_background(plate)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CONTROL_RADIUS)));

            let inset = (CONTROL_SIZE - CONTROL_ICON_SIZE) / 2.;
            ctx.scene.draw_icon(
                IconKey::new(icon_of(control), CONTROL_ICON_SIZE),
                RectF::new(
                    bounds.origin() + Vector2F::splat(inset),
                    Vector2F::splat(CONTROL_ICON_SIZE),
                ),
                theme().text_muted,
            );

            if control == Control::Menu {
                self.view
                    .set_menu_at(Some(bounds.origin() + bounds.size() - origin));
            }
        }
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

/// One block's rows from `from` to its last, as a copy of them would read.
///
/// **Through the region a drag over the block makes**, rather than through a
/// walk of the store: the block's control, the block's menu and a selection
/// have to agree about where a folded line ends, and the way to make three
/// answers one is to have one implementation. `from` is what makes it serve
/// both of the menu's copies — zero is the whole block, and a block's
/// [`output_from`](crook_terminal::Block::output_from) is what it printed.
///
/// A row past the block's last is an empty string rather than nothing at all:
/// a command that printed nothing has an output, and it is empty.
pub(super) fn block_text(blocks: &Blocks<'_>, id: BlockId, from: usize) -> Option<String> {
    let item = blocks.item(blocks.index_of(id)?)?;
    let last = item.rows.count().checked_sub(1)?;
    if from > last {
        return Some(String::new());
    }
    Selection::new(
        SelectionKind::Simple,
        Anchor::new(id, from, 0, CellSide::Left),
        Anchor::new(
            id,
            last,
            item.rows.columns().saturating_sub(1),
            CellSide::Right,
        ),
    )
    .text(blocks)
}

/// The icon inside a control's plate.
///
/// Lucide's `copy` is two overlapping sheets and its `ellipsis-vertical` is
/// the three dots every application on every desktop opens a menu about the
/// thing beside them with. Neither is a picture of Crook's: an icon somebody
/// has to learn is an icon that says nothing on a surface they are meeting for
/// the first time.
fn icon_of(control: Control) -> Lucide {
    match control {
        Control::Copy => Lucide::Copy,
        Control::Menu => Lucide::EllipsisVertical,
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
        self.output.laid_out(self.snapshot.columns);

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

        // Resolved once for the frame rather than once per row: a double
        // click asks the text under it where the word ends, and asking that
        // eighty times a screen would be a walk of the block per row.
        let region = self
            .output
            .selection()
            .and_then(|selection| selection.region(&self.addressed()));
        for item in std::mem::take(&mut self.window) {
            self.paint_item(origin, item, region, ctx);
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
                if self.view.hover(None, None) {
                    ctx.notify();
                }
            }
            return false;
        };

        match self.output.type_key(event, ctx) {
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
            Event::MouseMoved {
                position,
                modifiers,
                ..
            } => {
                let mut changed = self.track_link(*position, *modifiers);
                if changed {
                    ctx.notify();
                }
                changed |= self.hover(*position, ctx);
                changed
            }
            // Letting go of the chord key puts the pointer back to selecting,
            // and the underline goes with it — under a pointer that never
            // moved, which is why the move above cannot be the only place this
            // is asked.
            Event::ModifiersChanged {
                position,
                modifiers,
            } => {
                let changed = self.track_link(*position, *modifiers);
                if changed {
                    ctx.notify();
                }
                changed
            }
            // Before the press below, and instead of it: a chord-click on a
            // link is not a selection gesture, and starting one would leave a
            // highlight behind the browser that just opened.
            Event::MouseDown {
                button: MouseButton::Left,
                position,
                modifiers,
                ..
            } if self.open_link(*position, *modifiers) => true,
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
    /// The *list* scrolls, and nothing else: the anchors name blocks and rows
    /// of blocks, so what the emulator's own viewport is doing underneath is
    /// none of a selection's business. That is the whole reason a selection
    /// made in a block that has since scrolled off the top still copies the
    /// text it was drawn around.
    fn drag(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
        let Some((kind, anchor)) = self.output.pressed() else {
            return false;
        };
        let lines = self.autoscroll(position);
        if lines != 0. && self.view.apply(ScrollCause::Wheel(lines)) {
            ctx.notify();
        }
        let Some(at) = self.anchor_at(position) else {
            return false;
        };
        let covers = self.covers(kind, anchor, at);
        self.output.drag(at, covers, ctx)
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

/// The space above one item's first row, in lines.
///
/// `None` is the open block, which always has it. A finished block with no
/// command has none, which is the other half of [`block_height`]'s rule — and
/// they have to be the same rule, or the rows are painted somewhere other than
/// where the height index says the item is and a press lands a row out.
fn padding_top(block: Option<&Block>) -> f32 {
    match block {
        Some(block) if block.command.is_none() => 0.,
        _ => PADDING_TOP,
    }
}

/// How tall the open block is, in lines.
///
/// Zero when it has printed nothing, so a prompt that has not arrived yet
/// leaves no gap above the composer.
fn live_height(snapshot: &Snapshot, composer: bool) -> f32 {
    let Some((first, last)) = snapshot.live_rows() else {
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
    let (_, last) = snapshot.live_rows()?;
    if prompt.row < 0 || prompt.row as usize != last || prompt.column >= snapshot.columns {
        return None;
    }
    Some(prompt.column)
}

/// An extent to lay out at, given a maximum that may be unbounded.
fn bounded(max: f32, min: f32) -> f32 {
    if max.is_finite() { max } else { min }
}

/// A selected run of columns, cut down to the ones this list is drawing.
///
/// A finished block keeps the width it was harvested at, so a pane narrowed
/// since holds rows wider than it can draw — and a highlight painted at the
/// stored width would run out past the last glyph, over the gutter and the
/// scrollbar. The copy still takes the whole row: those cells are the block's
/// text, and only the picture is short.
fn clamped(columns: Range<usize>, drawn: usize) -> Range<usize> {
    columns.start.min(drawn)..columns.end.min(drawn)
}

/// Fills the cells of one row that are inside the selection.
///
/// One rectangle, because the region gives one run of columns per row: a
/// selected line costs a quad, not eighty. Drawn over the cells' own
/// backgrounds and under everything else, so the highlight takes the colour
/// the shell painted the cell and the character is still drawn on top of it in
/// its own ink — selecting text changes its ground, never its colour.
fn paint_selection(
    columns: Range<usize>,
    left: f32,
    top: f32,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    if columns.is_empty() {
        return;
    }
    scene
        .draw_rect_without_hit_recording(RectF::new(
            vec2f(left + columns.start as f32 * metrics.width, top),
            vec2f(columns.len() as f32 * metrics.width, metrics.height),
        ))
        .with_background(theme().selection);
}

/// Fills the padding above or below an item's rows when the selection runs
/// through it.
///
/// **The highlight bridges the gap between two blocks, deliberately.** A
/// selection that runs out of one block and into the next is one continuous
/// run of text with a line break in it — the same thing a selection across two
/// paragraphs is, where the space between them is highlighted too — and a
/// highlight with a hole at every boundary would read as several selections
/// that happen to be touching. The band takes the columns of the row it
/// continues rather than the pane's width, so it lines up with the rows above
/// and below it instead of bleeding past them — and an alt-drag, which takes
/// the same few columns out of every row it crosses, bridges as the column it
/// is rather than as a bar across the padding.
///
/// Nothing in the gap is copied: the padding, the divider and the copy control
/// are chrome, and [`Region::text`] joins two blocks with the one newline
/// between the last row of one and the first row of the next.
fn paint_gap(
    band: RectF,
    columns: Range<usize>,
    left: f32,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    if band.height() <= 0. || columns.is_empty() {
        return;
    }
    scene
        .draw_rect_without_hit_recording(RectF::new(
            vec2f(left + columns.start as f32 * metrics.width, band.min_y()),
            vec2f(columns.len() as f32 * metrics.width, band.height()),
        ))
        .with_background(theme().selection);
}

#[cfg(test)]
#[path = "block_list_tests.rs"]
mod tests;
