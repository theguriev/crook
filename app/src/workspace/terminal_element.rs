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
//! shell printed. A character it lacks — a powerline separator, a CJK
//! filename, an emoji — is drawn from a face that has it, resolved once per
//! distinct character by [`CellFont::glyph`]. A blank column would be worse
//! than wrong: the row would be silently short, and copying the screen would
//! disagree with looking at it.
//!
//! Nor does it take lines and blocks from a font at all. `│`, `╭`, `█`, `▌`
//! and braille are rectangles drawn from the cell's own edges by
//! [`box_drawing`], because every box an agent draws its interface in is made
//! of them and a font's stop short of a row this tall: the box would be a
//! dashed outline with a gap at every row.
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
//! # Pictures
//!
//! A picture a program printed — iTerm2's inline images, see
//! [`crook_terminal::image`] — is drawn over the rows it took here exactly as
//! it is on the list, by the same [`paint_images`]. A good share of them land
//! here rather than there: an `ssh` session with no shell integration is one
//! open block from its first byte, and so is anything whose output has run past
//! a screenful, a REPL or a training run printing its plot. The open block's
//! pictures arrive at viewport rows, numbered the way the snapshot's rows are,
//! so a grid scrolled back into its history finds them on the rows their text
//! went to without any arithmetic of its own. Nor does it check them: once the
//! history is full the emulator can no longer say which rows a picture is
//! over, and hands over none rather than a row the text has moved on from —
//! the bottom one, often, where the prompt is. The alternate screen has none
//! to draw either: the emulator does not place a picture there.
//!
//! # What an input method is composing
//!
//! A program that has the keyboard has no composer under it, so a word being
//! composed in Japanese, Chinese or Korean — or a dead key's accent — is drawn
//! here instead, over the cells from the program's cursor, underlined, the way
//! every terminal draws one: see [`paint_composition`], which the list's open
//! block draws with too. It is drawn and never sent, and it is drawn where the
//! cursor *is* rather than where it is shown, because a program that draws its
//! own caret hides the real one on the field it is reading. Where the input
//! method's caret ends up is written back to the pane's
//! [`PaneComposition`] for the window, which puts the candidate list beside
//! it.
//!
//! # Selecting the output
//!
//! The grid is the other selectable surface in a pane, and it is selected with
//! exactly the same machinery as the list: **a grid is a list of one block.**
//! A press starts a selection at the cell under it — a character, a word on the
//! second click, a line on the third, a block with Alt — a drag takes it out,
//! and a drag that leaves the top or the bottom edge scrolls the viewport under
//! the pointer so that a selection can run past the screen it began on. The
//! anchors, the region and the copy are all [`crate::selection`]; what is here
//! is the arithmetic between a pixel and a cell.
//!
//! The one thing this surface has to do that the list does not is *number* its
//! rows. A block's rows are its own and never move; the grid's move under a
//! viewport that scrolls, so a row is numbered from the oldest line the
//! scrollback still holds — see [`crate::selection::grid_first_row`] — and
//! copying reads those rows back out of the emulator, into the very store a
//! finished block's rows live in. That is what lets a drag through the
//! scrollback copy text the snapshot never held.
//!
//! **None of the selection is kept here**, because a press and the drag that
//! answers it are separated by every frame the pointer takes to move and this
//! element is thrown away on each of them. It belongs to the workspace, in a
//! [`PaneSelection`]. The rules that let go of one are typing, clicking into
//! the field, clicking somewhere else in the output, and the pane closing. The
//! fifth thing that could and must not is the shell printing, which is why
//! nothing in the paint path touches it.
//!
//! [`Line`]: crookui_core::text_layout::Line

use std::ops::Range;
use std::sync::Arc;

use crook_terminal::{
    BlockId, CellFlags, CellSide, Cursor, CursorShape, ImagePlacement, MouseEventKind, Rgb,
    RowCombining, Rows, SelectionKind, Snapshot, SnapshotCell,
};
// Both crates have a `MouseButton` and they are different types: one is what a
// window reported, the other is what a terminal protocol names. Keeping the
// unqualified name for the window's own is what makes every event pattern in
// this file read the way it did before mouse reporting existed.
use crook_terminal::Modifiers as ReportedModifiers;
use crook_terminal::MouseButton as ReportedButton;
use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::{DispatchedEvent, Event, Modifiers, MouseButton};
use crookui_core::geometry::{Color, Point, RectF, Vector2F, vec2f};
use crookui_core::image::device_size;
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};
use crookui_core::scene::{Border, ClipBounds, PointerShape, Scene};

use crate::browser;
use crate::clipboard::Clipboard;
use crate::input_keys::Platform;
use crate::pane_composition::{PaneComposition, Preedit};
use crate::pane_link::{self, LinkRow, LinkSpan, LinkTarget, PaneLink, PathsOn};
use crate::pane_selection::PaneSelection;
use crate::pane_surface;
use crate::selection::{Anchor, Blocks, Cells, Item, Region, Selection, grid_first_row};
use crate::tab::PaneId;
use crate::terminal_font::{CellFont, CellMetrics};
use crate::terminal_model::TerminalHandle;
use crate::text_input::TextInput;
use crate::theme::theme;

use super::action::WorkspaceAction;
use super::block_list::{FIND_CURRENT_ALPHA, FIND_MATCH_ALPHA, paint_wash};
use super::box_drawing;
use super::pane_output::{Keys, Output, Typed, selection_kind};

/// How thick the rules a cell can carry are, as a fraction of the font size.
///
/// The same ratio a shaped line underlines itself with, so a link underlined by
/// a shell and a label underlined by the UI are the same weight — and a light
/// box-drawing line is drawn at it too, see [`box_drawing`].
pub(super) const RULE_THICKNESS_RATIO: f32 = 0.06;

/// How far below the baseline an underline sits, as a fraction of the space
/// between the baseline and the bottom of the cell.
const UNDERLINE_OFFSET_RATIO: f32 = 0.4;

/// Where a strikeout crosses the cell, measured up from the baseline as a
/// fraction of the font size.
const STRIKEOUT_HEIGHT_RATIO: f32 = 0.28;

/// How wide the beam cursor and the hollow block's outline are drawn.
const CURSOR_STROKE: f32 = 2.;

/// How much of the program's ink the frame around a picture still being
/// decoded is drawn in, out of 255.
///
/// Enough to say a picture is coming to somebody looking at the rows it took,
/// and little enough to pass for the blank rows they are for the moment it
/// lasts.
const WAITING_PICTURE_ALPHA: u8 = 40;

/// The most wheel notches one scroll event is reported as.
///
/// A trackpad fling arrives as a single event carrying a large pixel delta, and
/// the mouse protocol counts notches: without a bound, one flick of two fingers
/// would be hundreds of six-byte writes down a pty for a screen that can only
/// scroll as far as it has lines.
const MAX_WHEEL_NOTCHES: u32 = 16;

/// Whether these modifiers mean "the pointer is following links right now".
///
/// The platform's own chord key, which is the one every terminal uses for
/// this: Command on macOS, Control everywhere else. It has to be a modifier
/// rather than a plain click, because the pointer is already spoken for — a
/// terminal where clicking a URL opened a browser is a terminal you cannot
/// select a URL in.
pub(super) fn opens_links(modifiers: Modifiers) -> bool {
    match Platform::current() {
        Platform::Mac => modifiers.cmd && !modifiers.ctrl,
        // Not with Shift, which suspends mouse reporting and is how a drag is
        // taken out of a program that has the pointer. Two meanings for one
        // combination is one too many.
        Platform::Other => modifiers.ctrl && !modifiers.cmd && !modifiers.shift,
    }
}

/// Puts the link a pointer found under it while the chord key is held — and
/// lets go of what was found when it is not — and asks the workspace to look
/// for the file a path names when that has not been done.
///
/// Its own function because both surfaces follow links, and both have to ask
/// the same way: the element holds no executor, and the look is a `stat` that
/// must not run on the thread that draws. See [`crate::pane_link`]. Reports
/// whether the frame changed.
pub(super) fn follow_link(
    links: &PaneLink,
    modifiers: Modifiers,
    pane: Option<PaneId>,
    ctx: &mut EventContext,
    find: impl FnOnce() -> Option<LinkSpan>,
) -> bool {
    let changed = links.track(opens_links(modifiers), find);
    if let Some(pane) = pane
        && links.wants_a_look()
    {
        ctx.dispatch_typed_action(WorkspaceAction::LookForPath(pane));
    }
    changed
}

/// Opens the link a chord-click landed on, reporting whether there was one to
/// open.
///
/// A URL goes to the platform's handler from here, as it always has. A path
/// is opened by the workspace, which knows the editor — and only one whose
/// file has been found: what is not underlined does not open, and a press on
/// it selects.
pub(super) fn open_link(
    links: Option<&PaneLink>,
    link: LinkSpan,
    pane: Option<PaneId>,
    ctx: &mut EventContext,
) -> bool {
    match link.target {
        LinkTarget::Url(uri) => browser::open(&uri),
        LinkTarget::File(file) => {
            let (Some(links), Some(pane)) = (links, pane) else {
                return false;
            };
            if !links.press(file) {
                return false;
            }
            ctx.dispatch_typed_action(WorkspaceAction::OpenPath(pane));
            true
        }
    }
}

/// Underlines the cells a link occupies, and asks for the hand over them.
///
/// Its own function because both surfaces draw it: a URL printed by a running
/// command is on the grid, and the same URL a moment later is a row of a
/// finished block.
///
/// The hand comes with the rule because both mean the same thing — a click
/// here follows the link — and both are true only while the chord key is
/// held: a link is under the pointer only then, so without it there is no
/// rule and the I-beam the surface asked for is what shows.
pub(super) fn paint_link_rule(
    row_origin: Vector2F,
    start: usize,
    len: usize,
    metrics: CellMetrics,
    color: Color,
    scene: &mut Scene,
) {
    scene.request_pointer(
        RectF::new(
            row_origin + vec2f(start as f32 * metrics.width, 0.),
            vec2f(len as f32 * metrics.width, metrics.height),
        ),
        PointerShape::Hand,
    );

    let thickness = (metrics.height * RULE_THICKNESS_RATIO).max(1.);
    scene
        .draw_rect_without_hit_recording(RectF::new(
            vec2f(
                row_origin.x() + start as f32 * metrics.width,
                row_origin.y() + metrics.height - thickness,
            ),
            vec2f(len as f32 * metrics.width, thickness),
        ))
        .with_background(color);
}

/// The terminal's name for a button the window reported, or `None` for one the
/// mouse protocol has no number for.
///
/// The two side buttons of a five-button mouse are the `None` case. The
/// protocol does have codes for them, and nothing that reads the mouse in a
/// terminal has ever expected one, so sending them would be inventing traffic
/// rather than reporting a gesture.
fn reported(button: MouseButton) -> Option<ReportedButton> {
    match button {
        MouseButton::Left => Some(ReportedButton::Left),
        MouseButton::Middle => Some(ReportedButton::Middle),
        MouseButton::Right => Some(ReportedButton::Right),
        MouseButton::Back | MouseButton::Forward => None,
    }
}

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

    /// The link under the pointer, which the workspace keeps per pane because
    /// the move that finds one and the frame that underlines it are different
    /// frames. `None` for a grid nothing can be clicked in.
    links: Option<PaneLink>,
    /// Which machine the pane's shell last said it was on, and where on this
    /// one, which is what a path the grid shows is read against. See
    /// [`Self::with_links`].
    paths: PathsOn,

    /// The find matches on screen, and the current one wherever it is. See
    /// [`Self::with_find`].
    finds: Vec<Selection>,
    find_current: Option<Selection>,

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
            links: None,
            paths: PathsOn::ThisMachine(None),
            finds: Vec::new(),
            find_current: None,
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

    /// Says what Option means to the program on a Mac. See
    /// [`OptionAsMeta`](crate::settings::OptionAsMeta).
    pub fn with_option_as_meta(mut self, option_as_meta: crate::settings::OptionAsMeta) -> Self {
        self.output = self.output.with_option_as_meta(option_as_meta);
        self
    }

    /// Says whether the window has the desktop's keyboard, which the cursor is
    /// drawn filled only while it does. See [`Output::draws_focus`].
    pub fn with_window_focused(mut self, focused: bool) -> Self {
        self.output = self.output.with_window_focused(focused);
        self
    }

    /// Attaches the field under this grid, whose line decides what Ctrl-D
    /// means.
    pub fn with_input(mut self, input: TextInput) -> Self {
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
        self.output = self
            .output
            .with_selection(pane, gesture, clipboard, Cells::Grid);
        self
    }

    /// Makes the URLs and the paths the shell printed clickable, through the
    /// link state the workspace keeps for this pane.
    ///
    /// `paths` is where the pane's shell last said it was. The grid is a
    /// full-screen program or a command still running, both started there, so
    /// it is what a relative path on it is read against — and when that is
    /// another machine, the paths on it are that machine's, and none is a
    /// link.
    pub fn with_links(mut self, links: PaneLink, paths: PathsOn) -> Self {
        self.links = Some(links);
        self.paths = paths;
        self
    }

    /// Attaches the pane's input method composition, which is drawn at the
    /// program's cursor while the grid has the keyboard. See
    /// [`Output::composition`].
    pub fn with_composition(mut self, composition: PaneComposition) -> Self {
        self.output = self.output.with_composition(composition);
        self
    }

    /// The find matches to highlight, and the one the bar is standing on.
    ///
    /// The ones on screen rather than all of them: a grid's matches reach back
    /// through the whole scrollback, and a frame paints a screenful. Each is a
    /// selection in the grid's own numbering — rows from the oldest line of the
    /// scrollback — so it is drawn by the very arithmetic a drag's selection
    /// is, wide characters and folded lines included.
    pub fn with_find(mut self, finds: Vec<Selection>, current: Option<Selection>) -> Self {
        self.finds = finds;
        self.find_current = current;
        self
    }

    /// Finds the link under the pointer, or lets go of the one that was there.
    ///
    /// Only while the platform's own chord key is held: the pointer is already
    /// spoken for by the selection, and a click that opened a browser instead
    /// of placing a selection would be a terminal you cannot copy a URL out
    /// of. Reports whether the frame changed.
    fn track_link(&self, position: Vector2F, modifiers: Modifiers, ctx: &mut EventContext) -> bool {
        let Some(links) = self.links.as_ref() else {
            return false;
        };
        follow_link(links, modifiers, self.output.pane(), ctx, || {
            self.link_at(position, modifiers)
        })
    }

    /// The link under a window position, on the viewport rows it covers.
    ///
    /// `None` unless the link modifier is held, which is what keeps this scan
    /// off every ordinary pointer move.
    fn link_at(&self, position: Vector2F, modifiers: Modifiers) -> Option<LinkSpan> {
        if !opens_links(modifiers) {
            return None;
        }
        if !self
            .bounds()
            .is_some_and(|bounds| bounds.contains_point(position))
        {
            return None;
        }

        // The *viewport's* row, because that is what the snapshot is indexed
        // by. A selection's anchor names a row of the text instead, which is a
        // different number the moment anything has scrolled.
        let (row, column) = self.viewport_cell_at(position)?;
        let rows = Rows::Live {
            snapshot: &self.snapshot,
            top: 0,
            count: self.snapshot.rows,
        };
        pane_link::find(&rows, row, column, &self.paths, LinkRow::Viewport)
    }

    /// Opens the link under the pointer, reporting whether there was one.
    fn open_link(&self, position: Vector2F, modifiers: Modifiers, ctx: &mut EventContext) -> bool {
        let Some(link) = self.link_at(position, modifiers) else {
            return false;
        };
        open_link(self.links.as_ref(), link, self.output.pane(), ctx)
    }

    /// The grid as the one block a selection addresses.
    ///
    /// Under the name the anchors already carry rather than the one the open
    /// block has now: there is exactly one item, so its identity is whatever
    /// the selection called it, and a block closing under a grid that is still
    /// up does not throw the highlight away.
    fn addressed(&self) -> Blocks<'_> {
        let id = self
            .output
            .selection()
            .map_or(self.snapshot.live_block.id, |selection| {
                selection.anchor.block
            });
        Blocks::grid(&self.snapshot, id)
    }

    /// Whether the selection this gesture would make covers any cells.
    fn covers(&self, kind: SelectionKind, anchor: Anchor, head: Anchor) -> bool {
        Selection::new(kind, anchor, head)
            .region(&self.addressed())
            .is_some()
    }

    /// The typed keystroke, if this pane is the one that should have it and
    /// the shell is the half of the pane it belongs to.
    fn type_key(&self, event: &Event, ctx: &mut EventContext) -> bool {
        self.output.type_key(event, ctx) != Typed::Ignored
    }

    /// Starts a selection where a press landed, hands the press to a program
    /// that is reading the mouse, or pastes PRIMARY for a middle one that it
    /// is not.
    fn press(
        &self,
        button: MouseButton,
        position: Vector2F,
        click_count: u32,
        modifiers: Modifiers,
        ctx: &mut EventContext,
    ) -> bool {
        let Some(at) = self.anchor_at(position) else {
            return false;
        };

        if self.report(MouseEventKind::Press, reported(button), position, modifiers) {
            self.output.begin_reporting();
            return true;
        }

        // A middle press the program did not take pastes PRIMARY, which is
        // what one means in every terminal on X11 and Wayland. Shift reaches
        // here past a program reading the mouse, as it does for a drag.
        if button == MouseButton::Middle {
            return self.output.paste_primary(ctx);
        }

        // Only the primary button selects. A right click a program did not
        // want, or a middle one with PRIMARY switched off, has nothing to do
        // with a selection, and taking one would throw away the highlight
        // somebody was about to copy.
        if button != MouseButton::Left {
            return false;
        }
        let kind = selection_kind(click_count, modifiers.alt);
        let covers = self.covers(kind, at, at);
        self.output
            .press(kind, at, covers, self.snapshot.columns, ctx)
    }

    /// Hands a gesture to the program in this pane, if it asked for the mouse.
    ///
    /// **Shift is the way out.** Every terminal makes holding it suspend mouse
    /// reporting, because otherwise there is no way at all to select text out
    /// of a program that has taken the pointer — and copying what `htop` is
    /// showing is a thing people do constantly. Holding it therefore reports
    /// nothing, and the gesture falls through to the selection below.
    fn report(
        &self,
        kind: MouseEventKind,
        button: Option<ReportedButton>,
        position: Vector2F,
        modifiers: Modifiers,
    ) -> bool {
        if modifiers.shift {
            return false;
        }
        let Some((row, column)) = self.viewport_cell_at(position) else {
            return false;
        };
        self.output.report_mouse(
            kind,
            button,
            row,
            column,
            ReportedModifiers {
                shift: false,
                control: modifiers.ctrl,
                alt: modifiers.alt,
                logo: modifiers.cmd,
            },
        )
    }

    /// Drags the open end of the selection to the pointer, scrolling the
    /// viewport when the pointer has left the grid.
    fn drag(
        &self,
        button: MouseButton,
        position: Vector2F,
        modifiers: Modifiers,
        ctx: &mut EventContext,
    ) -> bool {
        // A drag a program took goes on being reported to it, wherever the
        // pointer has got to. It is clamped into the grid rather than dropped
        // outside it for the same reason a selection is: a drag that left the
        // window is still a drag, and `vim` resizing a split needs to hear
        // about the row the pointer is level with.
        if self.output.is_reporting() {
            return self.report(
                MouseEventKind::Motion,
                reported(button),
                position,
                modifiers,
            );
        }

        // Deliberately not hit-tested: dragging *past* the pane is how a
        // selection is taken to the end of a line, and how it is taken past
        // the end of the screen. What keeps this pane's grid out of a drag
        // that began in the field, or in the pane beside it, is that only the
        // pane the press landed on has a gesture open.
        let Some((kind, anchor)) = self.output.pressed() else {
            return false;
        };
        // The viewport moves and the selection does not go with it: a row is
        // numbered from the oldest line of the scrollback, so scrolling the
        // screen under a drag brings *more* rows within reach rather than
        // renaming the ones already taken.
        if let Some(handle) = self.output.handle() {
            handle.scroll_lines(self.autoscroll(position));
        }
        let Some(at) = self.anchor_at(position) else {
            return false;
        };
        let covers = self.covers(kind, anchor, at);
        self.output.drag(at, covers, ctx)
    }

    /// Reports a pointer move that no button is behind, for a program that
    /// asked for `?1003`.
    ///
    /// Hit-tested, unlike a drag: with no button down there is no gesture that
    /// began here, so a pointer crossing a neighbouring pane is not this one's
    /// to report.
    fn moved(&self, position: Vector2F, modifiers: Modifiers) -> bool {
        if self.output.is_dragging()
            || self.output.is_reporting()
            || !self.output.mouse_modes().motion
        {
            return false;
        }
        if !self
            .bounds()
            .is_some_and(|bounds| bounds.contains_point(position))
        {
            return false;
        }
        self.report(MouseEventKind::Motion, None, position, modifiers)
    }

    /// Ends the gesture, reporting whether this pane had one.
    ///
    /// A press a program took and a press that was dragging a selection are
    /// two different gestures with one button, and the release belongs to
    /// whichever of them was open.
    fn release(&self, button: MouseButton, position: Vector2F, modifiers: Modifiers) -> bool {
        // A side button coming up is nothing to the program: its press was
        // never reported. Letting it through ended the reporting a drag with
        // another button had started — a release sent for a button still
        // held, and the real one's release then never sent at all.
        let Some(reported) = reported(button) else {
            return self.output.is_reporting();
        };
        if self.output.end_reporting() {
            self.report(MouseEventKind::Release, Some(reported), position, modifiers);
            return true;
        }
        self.output.release()
    }

    /// Which cell of the grid a window position lands on, and which side of it
    /// the pointer is on.
    ///
    /// Clamped into the grid rather than refused outside it, because a drag
    /// that has left the pane is still selecting: past the right edge means the
    /// end of the row, and past the bottom means the last row — and the row it
    /// clamps to is the one [`Self::autoscroll`] is about to scroll under the
    /// pointer.
    ///
    /// The row it answers is not the row of the *screen*. It is numbered from
    /// the oldest line the scrollback holds, which is what keeps a selection on
    /// the text it was dragged across while the viewport moves over it — and
    /// it is the same numbering [`Self::addressed`] hands the region.
    /// The cell of the *viewport* a window position lands on.
    ///
    /// Not an [`Anchor`], and the difference is the whole reason both exist. An
    /// anchor names a row of the *text*, numbered so that it stays on its own
    /// characters while the viewport scrolls under it; the mouse protocol names
    /// a row of the *screen*, because that is what the program drawing on it is
    /// addressing. A drag through the scrollback needs the first; `htop` needs
    /// the second.
    ///
    /// Clamped into the grid rather than refused outside it, for the reason a
    /// drag is: a pointer that has left the pane is still pointing at the row
    /// it is level with.
    fn viewport_cell_at(&self, position: Vector2F) -> Option<(usize, usize)> {
        let bounds = self.bounds()?;
        let metrics = self.font.metrics();
        let (fitting_columns, fitting_rows) = metrics.grid_for(bounds.width(), bounds.height());
        let columns = usize::from(fitting_columns).min(self.snapshot.columns);
        let rows = usize::from(fitting_rows).min(self.snapshot.rows);
        if columns == 0 || rows == 0 {
            return None;
        }

        let local = position - bounds.origin();
        let (column, _) = column_at(local.x(), metrics.width, columns);
        let row = (local.y() / metrics.height)
            .floor()
            .clamp(0., (rows - 1) as f32) as usize;
        Some((row, column))
    }

    fn anchor_at(&self, position: Vector2F) -> Option<Anchor> {
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
        let id = self
            .output
            .selection()
            .map_or(self.snapshot.live_block.id, |selection| {
                selection.anchor.block
            });
        Some(Anchor::new(
            id,
            grid_first_row(&self.snapshot) + row,
            column,
            side,
        ))
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
    /// Moves the viewport through the scrollback, if the wheel turned over this
    /// pane — or gives the wheel to whatever is running instead.
    ///
    /// Three destinations, in the order every terminal tries them:
    ///
    /// 1. **A program reading the mouse** gets the notch as button 64 or 65,
    ///    which is how `tmux` scrolls its own pane and `vim` its own buffer.
    /// 2. **A full-screen program that asked for `?1007`** gets arrow keys.
    ///    That is what makes the wheel work in `less`, `man` and `git log`,
    ///    none of which reports the mouse.
    /// 3. **Everything else** scrolls the emulator's history, which is the
    ///    only one of the three that is Crook's own scrollback.
    fn scroll(&self, event: &Event) -> bool {
        let Event::ScrollWheel {
            position,
            delta,
            modifiers,
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
        // Positive is up the screen and back into history, which is the sense
        // both the wheel and the emulator use. The pixels a trackpad reports
        // are a fraction of a line an event, so the handle carries what does
        // not add up to a whole one yet rather than rounding it away.
        let lines = handle.take_scroll_lines(delta.to_pixels(height).y() / height);
        if lines == 0 {
            return false;
        }

        // Shift takes the wheel back from whatever is running, the same way it
        // takes a press back: it is the one gesture that always means Crook's
        // own scrollback. On the alternate screen there is no history to move
        // through, so it lands on nothing — which is the honest answer, and
        // better than a program scrolling when somebody asked it not to.
        if !modifiers.shift {
            if self.report_wheel(lines, *position, *modifiers) {
                return true;
            }
            if self.output.alternate_scroll(lines) {
                return true;
            }
        }

        handle.scroll_lines(lines);
        true
    }

    /// Sends `lines` notches of the wheel to a program reading the mouse.
    ///
    /// One report per line, because a notch is what the protocol counts and
    /// there is no way to say "three" in one. Bounded, so that a trackpad
    /// fling cannot turn into hundreds of writes down a pty.
    fn report_wheel(&self, lines: i32, position: Vector2F, modifiers: Modifiers) -> bool {
        let button = if lines > 0 {
            ReportedButton::WheelUp
        } else {
            ReportedButton::WheelDown
        };

        let notches = lines.unsigned_abs().min(MAX_WHEEL_NOTCHES);
        let mut sent = false;
        for _ in 0..notches {
            sent |= self.report(MouseEventKind::Press, Some(button), position, modifiers);
        }
        sent
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
        // A resize re-wraps every row under a selection and a fall back to
        // this surface renumbers them, so a selection that survives neither is
        // let go of here. See `Output::laid_out`.
        self.output.laid_out(self.snapshot.columns);

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
        // Nor is it filled in a window behind another, which is not where
        // typing goes either.
        let owns_caret = self.output.draws_focus()
            && pane_surface::of(&self.snapshot, std::time::Instant::now()).output_owns_caret();
        let selected = self.output.selection().and_then(|selection| {
            Some((selection.anchor.block, selection.region(&self.addressed())?))
        });
        // Each match against the grid under the name it was found by, which
        // is every match's: one grid is one item, whatever it is called.
        let found = |selection: &Selection| {
            let blocks = Blocks::grid(&self.snapshot, selection.anchor.block);
            Some((selection.anchor.block, selection.region(&blocks)?))
        };
        let washes = Washes {
            finds: self.finds.iter().filter_map(found).collect(),
            find_current: self.find_current.as_ref().and_then(found),
            selected,
        };
        paint_grid(
            &self.snapshot,
            &self.font,
            origin,
            size,
            owns_caret,
            &washes,
            ctx.scene,
        );

        // Over everything the program drew, in a layer of its own. Where the
        // input method's caret is recorded whether or not anything is being
        // composed: it is the program's cursor until something is, and that
        // is where a composition starts.
        if let Some(composition) = self.output.composition() {
            let metrics = self.font.metrics();
            let (columns, rows) = drawn_grid(&self.snapshot, metrics, size);
            let caret = self
                .snapshot
                .cursor_cell
                .filter(|&(row, column)| row < rows && column < columns)
                .map(|(row, column)| {
                    paint_composition(
                        composition.preedit().as_ref(),
                        CursorRow {
                            origin: origin + vec2f(0., row as f32 * metrics.height),
                            columns,
                            column,
                        },
                        (self.snapshot.foreground, self.snapshot.background),
                        &self.font,
                        ctx.scene,
                    )
                });
            composition.set_caret_rect(caret);
        }

        // Over the grid, because it is an affordance rather than something the
        // shell printed: it appears when the chord key goes down and goes away
        // when it comes up, and the cells under it are unchanged.
        let Some(links) = self.links.as_ref() else {
            return;
        };
        for (row, link) in links.on_viewport_rows(self.snapshot.rows) {
            paint_link_rule(
                origin + vec2f(0., row as f32 * self.font.metrics().height),
                link.start,
                link.len,
                self.font.metrics(),
                color(self.snapshot.foreground),
                ctx.scene,
            );
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
                button,
                position,
                modifiers,
            } => return self.drag(*button, *position, *modifiers, ctx),
            Event::MouseUp {
                button,
                position,
                modifiers,
            } => return self.release(*button, *position, *modifiers),
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
                modifiers,
                ..
            } if self.open_link(*position, *modifiers, ctx) => {
                // Before the press below, and instead of it. A chord-click on a
                // link is not a selection gesture, and starting one would leave
                // a highlight behind the browser that just opened.
                true
            }
            Event::MouseDown {
                button,
                position,
                click_count,
                modifiers,
            } if self
                .bounds()
                .is_some_and(|bounds| bounds.contains_point(*position)) =>
            {
                self.press(*button, *position, *click_count, *modifiers, ctx)
            }
            // A bare move: it lights up a link under the pointer, and it is
            // reported to a program that asked to hear about every one. Last,
            // because it is the only mouse event that is *usually* nothing to
            // do with this element, and both halves answer `false` cheaply.
            //
            // A synthetic move is included, and deliberately: it is replayed
            // after a frame that changed layout, and a link is exactly the
            // kind of thing whose position moved out from under a pointer that
            // did not.
            Event::MouseMoved {
                position,
                modifiers,
                is_synthetic,
            } => {
                let mut changed = self.track_link(*position, *modifiers, ctx);
                if changed {
                    ctx.notify();
                }
                if !is_synthetic {
                    changed |= self.moved(*position, *modifiers);
                }
                changed
            }
            // Letting go of the chord key puts the pointer back to selecting,
            // and the underline has to go with it — under a pointer that never
            // moved, which is why this is not handled by the move above.
            Event::ModifiersChanged {
                position,
                modifiers,
            } => {
                let changed = self.track_link(*position, *modifiers, ctx);
                if changed {
                    ctx.notify();
                }
                changed
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

/// What is washed over a grid's cells, under their glyphs: the find's matches
/// and the selection, each a region with the name the grid answered to when it
/// was made.
#[derive(Default)]
struct Washes {
    /// The find matches on screen, in the terminal's amber.
    finds: Vec<(BlockId, Region)>,
    /// The match the bar is standing on, brighter and in the accent over the
    /// rest, so which of a screenful of them the next step leaves is never in
    /// doubt.
    find_current: Option<(BlockId, Region)>,
    /// What the pointer selected, over both: a person can select text a find
    /// has highlighted, and the selection is the one that has to win.
    selected: Option<(BlockId, Region)>,
}

/// Paints one snapshot into `scene`, filling `size` from `origin`.
///
/// `owns_caret` is whether this grid is where typing would go. It is the
/// cursor's business and nothing else's: a grid that is not listening draws an
/// outline where a listening one draws the filled block the program asked for,
/// which is how every terminal with splits says which shell has the keyboard.
///
/// `washes` are the regions to highlight, resolved by the caller.
fn paint_grid(
    snapshot: &Snapshot,
    font: &CellFont,
    origin: Vector2F,
    size: Vector2F,
    owns_caret: bool,
    washes: &Washes,
    scene: &mut Scene,
) {
    let metrics = font.metrics();
    let (columns, rows) = drawn_grid(snapshot, metrics, size);

    // The ground the emulator resolved, painted once. Every cell that agrees
    // with it — which on an idle screen is all of them — then costs nothing.
    scene
        .draw_rect_without_hit_recording(RectF::new(origin, size))
        .with_background(color(snapshot.background));

    // The I-beam says a press here selects, and over a program that has asked
    // for the mouse it does not: the press is the program's. So a grid whose
    // program has the mouse gets the arrow, which is what kitty and Alacritty
    // show over `vim` with the mouse on. Alacritty brings the I-beam back
    // while Shift is held, since Shift selects through the program; this does
    // not, because nothing is drawn for Shift going down, and the arrow is
    // what is true of the press a person makes without it.
    scene.request_pointer(
        RectF::new(origin, size),
        if snapshot.mouse_reporting {
            PointerShape::Arrow
        } else {
            PointerShape::Text
        },
    );

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

    let first = grid_first_row(snapshot);
    // The grid as the one item a selection addresses, which is what carries
    // the numbering its rows are in: screen row zero is `first`, not zero.
    let item = |id| Item {
        id,
        rows: Rows::Live {
            snapshot,
            top: 0,
            count: rows,
        },
        first,
    };
    // The columns of one screen row a region covers, cut to what is drawn.
    let covered = |(id, region): &(BlockId, Region), row: usize| {
        region
            .columns_on(&item(*id), first + row)
            .filter(|covered| covered.start < columns)
            .map(|covered| covered.start..covered.end.min(columns))
    };
    for (row, cells) in snapshot.iter_rows().enumerate().take(rows) {
        let top = origin.y() + row as f32 * metrics.height;
        let cells = &cells[..columns];

        paint_backgrounds(cells, snapshot.background, origin.x(), top, metrics, scene);
        // Over the cells' own backgrounds and under everything else: a
        // highlight is translucent, so it takes the colour of whatever the
        // shell painted the cell and the character is still drawn on top of it
        // in its own ink. Highlighting text changes its ground, never its
        // colour. The find's matches go first, the current one over the rest,
        // and the selection over all of them — the order the list paints in.
        for found in &washes.finds {
            if let Some(matched) = covered(found, row) {
                let wash = theme().usage_high.with_alpha(FIND_MATCH_ALPHA);
                paint_wash(matched, origin.x(), top, metrics, scene, wash);
            }
        }
        if let Some(matched) = washes
            .find_current
            .as_ref()
            .and_then(|current| covered(current, row))
        {
            let wash = theme().accent.with_alpha(FIND_CURRENT_ALPHA);
            paint_wash(matched, origin.x(), top, metrics, scene, wash);
        }
        if let Some(selected) = washes
            .selected
            .as_ref()
            .and_then(|selected| covered(selected, row))
        {
            paint_selection(selected, origin.x(), top, metrics, scene);
        }
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

    // Over the rows they took, which the emulator left blank when it moved the
    // cursor past them. Cut off at the last row drawn rather than at the box,
    // whose bottom is a fraction of a row nothing is drawn in.
    paint_images(
        &snapshot.live_block.images,
        origin,
        RectF::new(
            origin,
            vec2f(columns as f32 * metrics.width, rows as f32 * metrics.height),
        ),
        metrics,
        snapshot.foreground,
        scene,
    );

    if let (Some(cursor), Some(shape)) = (snapshot.cursor.as_ref(), shape)
        && cursor.row < rows
        && cursor.column < columns
    {
        paint_cursor(cursor, shape, origin, metrics, scene);
        if let Some(cell) = snapshot.cell(cursor.row, cursor.column) {
            // In the ground colour, the one `paint_glyphs` inverted the cell
            // to under the only cursor this draws anything over.
            paint_over_cursor(
                cell,
                shape,
                origin
                    + vec2f(
                        cursor.column as f32 * metrics.width,
                        cursor.row as f32 * metrics.height,
                    ),
                snapshot.background,
                metrics,
                scene,
            );
        }
    }
}

/// How many columns and rows of `snapshot` a box of `size` draws: as many as
/// fit, and no more than the snapshot has.
fn drawn_grid(snapshot: &Snapshot, metrics: CellMetrics, size: Vector2F) -> (usize, usize) {
    let (fitting_columns, fitting_rows) = metrics.grid_for(size.x(), size.y());
    (
        usize::from(fitting_columns).min(snapshot.columns),
        usize::from(fitting_rows).min(snapshot.rows),
    )
}

/// The row a program's cursor is on, as a surface draws it.
#[derive(Copy, Clone, Debug)]
pub(super) struct CursorRow {
    /// Where the row's first cell is drawn.
    pub(super) origin: Vector2F,
    /// How many of its cells are drawn.
    pub(super) columns: usize,
    /// Which of them the cursor is in.
    pub(super) column: usize,
}

/// Draws what an input method is composing at the program's cursor, and
/// answers the cell the input method's caret is in — the cursor's own cell
/// while nothing is being composed.
///
/// Both surfaces draw it with this: the grid, and the list's open block, which
/// is where a program on the primary screen — Claude Code — is drawn once it
/// has taken the composer away. `ink` is the foreground and the ground the
/// composition is drawn in, which are the program's defaults: it is text the
/// person is writing, not text the program coloured.
///
/// The composition covers the cells it takes, with the ground under it, so
/// what the program drew there does not show through a word that is not
/// there yet; it is underlined, which is what says it is not text; and the
/// input method's caret is a beam where it asked for one. It stays on the
/// row — see [`Preedit::lay_out`] — and is clipped to it.
pub(super) fn paint_composition(
    preedit: Option<&Preedit>,
    at: CursorRow,
    ink: (Rgb, Rgb),
    font: &CellFont,
    scene: &mut Scene,
) -> RectF {
    let metrics = font.metrics();
    let cell = |column: usize| {
        let column = column.min(at.columns.saturating_sub(1));
        RectF::new(
            at.origin + vec2f(column as f32 * metrics.width, 0.),
            vec2f(metrics.width, metrics.height),
        )
    };
    let Some(preedit) = preedit else {
        return cell(at.column);
    };
    let laid = preedit.lay_out(at.column, at.columns);
    if laid.span.is_empty() {
        return cell(at.column);
    }

    let (foreground, ground) = (color(ink.0), color(ink.1));
    let row = RectF::new(
        at.origin,
        vec2f(at.columns as f32 * metrics.width, metrics.height),
    );
    let span = RectF::new(
        at.origin + vec2f(laid.span.start as f32 * metrics.width, 0.),
        vec2f(laid.span.len() as f32 * metrics.width, metrics.height),
    );
    // A layer of its own, because a layer draws every glyph over every rect:
    // in the grid's, the ground below would be painted under the program's
    // characters rather than over them.
    scene.start_layer(ClipBounds::BoundedByActiveLayerAnd(row));
    scene
        .draw_rect_without_hit_recording(span)
        .with_background(ground);
    let face = font.regular();
    for &(column, character) in &laid.glyphs {
        let corner = at.origin + vec2f(column as f32 * metrics.width, 0.);
        paint_cell(character, corner, face, foreground, font, scene);
    }
    let thickness = (metrics.font_size * RULE_THICKNESS_RATIO).max(1.);
    let below = (metrics.height - metrics.baseline) * UNDERLINE_OFFSET_RATIO;
    scene
        .draw_rect_without_hit_recording(RectF::new(
            vec2f(span.min_x(), at.origin.y() + metrics.baseline + below),
            vec2f(span.width(), thickness),
        ))
        .with_background(foreground);
    if preedit.cursor().is_some() {
        // Kept inside the row: a caret past the last character of a
        // composition that ends at the row's edge would be clipped away.
        let x =
            (at.origin.x() + laid.caret as f32 * metrics.width).min(row.max_x() - CURSOR_STROKE);
        scene
            .draw_rect_without_hit_recording(RectF::new(
                vec2f(x, at.origin.y()),
                vec2f(CURSOR_STROKE, metrics.height),
            ))
            .with_background(foreground);
    }
    scene.stop_layer();
    cell(laid.caret)
}

/// Draws the pictures a program printed over the cells each one covers —
/// fitted inside them with its shape kept, unless the program asked for it
/// stretched.
///
/// Both surfaces draw them with this: a picture is on the list while its
/// block is, and on the grid while the open block has run past the top of the
/// screen. `origin` is where the top-left cell of the placements' numbering
/// is drawn, and `within` is the rows they may cover: a picture is cut off at
/// its edge rather than hanging over whatever is drawn beyond it.
///
/// A picture with no part inside `within` is passed over before it is
/// decoded, which is what keeps the pictures of a long session from being
/// decoded to draw the few on screen. One that is in view and has not been
/// decoded yet is drawn as a faint frame in `ink` around the room it will
/// take, and the frame its decode asks for draws the picture there — see
/// [`crate::inline_image`]. Where it goes is known before any of that: the
/// header said how big it is.
pub(super) fn paint_images(
    images: &[ImagePlacement],
    origin: Vector2F,
    within: RectF,
    metrics: CellMetrics,
    ink: Rgb,
    scene: &mut Scene,
) {
    use crate::inline_image::{self, Shown};

    let mut started = false;
    for placement in images {
        let cells = RectF::new(
            vec2f(
                origin.x() + placement.column as f32 * metrics.width,
                origin.y() + placement.row as f32 * metrics.height,
            ),
            vec2f(
                placement.columns as f32 * metrics.width,
                placement.rows as f32 * metrics.height,
            ),
        );
        if cells.intersection(within).is_none() {
            continue;
        }
        let drawn = if placement.stretch {
            cells.size()
        } else {
            let (width, height) = placement.image.size();
            let scale = (cells.width() / width as f32).min(cells.height() / height as f32);
            vec2f(width as f32 * scale, height as f32 * scale)
        };
        let bounds = RectF::new(cells.origin(), drawn);
        let shown = inline_image::shown(
            &placement.image,
            device_size(drawn, scene.scale_factor()),
            scene.frame(),
        );
        if let Shown::Nothing = shown {
            continue;
        }
        // A layer of their own, because a layer draws its pictures under its
        // glyphs and these go over the rows' — and only once one is drawn, so
        // a screen with none in view costs no layer.
        if !started {
            scene.start_layer(ClipBounds::BoundedByActiveLayerAnd(within));
            started = true;
        }
        match shown {
            Shown::Picture(bitmap) => {
                scene.draw_image(bitmap, bounds);
            }
            Shown::Waiting => {
                scene.draw_rect_without_hit_recording(bounds).with_border(
                    Border::all(1.).with_border_color(color(ink).with_alpha(WAITING_PICTURE_ALPHA)),
                );
            }
            Shown::Nothing => {}
        }
    }
    if started {
        scene.stop_layer();
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

        let at = vec2f(left + column as f32 * metrics.width, top);
        let foreground = color(if ink.inverted == Some(column) {
            ink.ground
        } else {
            cell.foreground
        });
        let face = font.face(cell.flags);
        paint_cell(cell.c, at, face, foreground, font, scene);

        // Combining marks are drawn from the same pen position the character
        // was: a mark glyph carries its own offset from the character it sits
        // on, and no column of its own to sit in.
        for mark in marks.at(column) {
            paint_cell(*mark, at, face, foreground, font, scene);
        }
    }
}

/// Draws one character into the cell whose top-left corner is `at`, in
/// whichever face can draw it — or in none, for a line or a block.
///
/// A line or a block character is drawn by [`box_drawing`] from the cell's own
/// edges, because a font's stops short of them and a column of `│` or `█`
/// would have a gap at every row. Everything else is a glyph on the cell's
/// baseline, and the face it comes from is not always the one that went in:
/// the monospace family covers Latin and not a CJK filename or a powerline
/// separator, and [`CellFont::glyph`] finds a face that does. A character no
/// installed font can draw is the only one that leaves the cell empty.
fn paint_cell(
    character: char,
    at: Vector2F,
    face: crookui_core::fonts::FontId,
    ink: Color,
    font: &CellFont,
    scene: &mut Scene,
) {
    let metrics = font.metrics();
    let cell = RectF::new(at, vec2f(metrics.width, metrics.height));
    if box_drawing::paint(character, cell, metrics.font_size, ink, scene) {
        return;
    }
    let Some((drawn_with, glyph)) = font.glyph(face, character) else {
        log::trace!("no installed font can draw {character:?}");
        return;
    };
    let pen = at + vec2f(0., metrics.baseline);
    scene.draw_glyph(pen, glyph, drawn_with, metrics.font_size, ink);
}

/// Draws a line or block character again, over the filled cursor just drawn
/// on its cell, in `ink`: the colour [`paint_glyphs`] gave it, which a surface
/// that inverts the character under a filled cursor passes inverted.
///
/// A glyph is drawn after every rectangle in its layer, so whatever character
/// is under the cursor shows over it. A line or a block is rectangles, painted
/// with its row and so under a cursor painted after the rows, where it would
/// vanish into a filled one. Anything else under the cursor is a glyph
/// already, and nothing is drawn for it here.
///
/// Nor is anything drawn under a cursor of any other `shape`. An outline, a
/// beam or an underline hides next to nothing of the first drawing, and a
/// second one over it would blend a shade or a rounded corner's smoothed edge
/// in twice: `░` under an unfocused pane's cursor would come out nearly `▒`.
fn paint_over_cursor(
    cell: &SnapshotCell,
    shape: CursorShape,
    at: Vector2F,
    ink: Rgb,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    if shape != CursorShape::Block || cell.flags.contains(CellFlags::WIDE_SPACER) {
        return;
    }
    box_drawing::paint(
        cell.c,
        RectF::new(at, vec2f(metrics.width, metrics.height)),
        metrics.font_size,
        color(ink),
        scene,
    );
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
/// One rectangle: the region gives one run of columns per row, so a selected
/// line costs a quad rather than eighty.
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

/// Which column a horizontal offset lands on, and which half of it.
///
/// The half is what makes a selection end *between* two characters rather than
/// on one: dragging right from the left of a character takes it and dragging
/// right from its middle does not. Outside the row the answer is its first cell
/// on the left and its last on the right, which is what a drag that has left
/// the pane means. A row of no columns has no last cell, so it answers with
/// column zero rather than reaching below `columns` for one — a value no caller
/// reads, each having refused an empty grid before it asks, but a total answer
/// keeps the arithmetic off the one input that could underflow it.
pub(super) fn column_at(x: f32, width: f32, columns: usize) -> (usize, CellSide) {
    if x < 0. {
        return (0, CellSide::Left);
    }
    let cell = x / width;
    let column = cell.floor() as usize;
    if column >= columns {
        return (columns.saturating_sub(1), CellSide::Right);
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

/// Draws the cursor at a cell somebody else placed, and the line or block
/// character under it again where the cursor would hide it.
///
/// What the block list needs: the open block's rows are drawn where the *list*
/// puts them, which is not where the viewport would, so the caller has already
/// done the arithmetic. `owns_caret` is whether typing would go here — a
/// surface that is not listening draws an outline where a listening one draws
/// the filled block the program asked for. `cell` is what the cursor is on.
///
/// The shape is decided once, here, for both drawings, so the character is
/// drawn again under the very cursor that was drawn and no other: see
/// [`paint_over_cursor`]. It is drawn in the cell's own colour, because the
/// list's rows invert nothing under a cursor.
pub(super) fn paint_cursor_at(
    cursor: &Cursor,
    cell: Option<&SnapshotCell>,
    owns_caret: bool,
    at: Vector2F,
    metrics: CellMetrics,
    scene: &mut Scene,
) {
    let shape = shape_of(cursor, owns_caret);
    paint_cursor_in(cursor, shape, at, metrics, scene);
    if let Some(cell) = cell {
        paint_over_cursor(cell, shape, at, cell.foreground, metrics, scene);
    }
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
