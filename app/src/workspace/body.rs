//! The active tab's body: one pane per pane the tab holds, each running a
//! shell.
//!
//! A pane is its terminal and nothing else. Not a card: no corner radius, no
//! border, no margin, no heading. What the session is called and what its
//! shell is doing are already in the tab strip beside it, and everything a box
//! around the grid would add is chrome charged against the thing a person came
//! to look at. Warp draws its panes exactly this way — its pane view has no
//! radius anywhere, its optional accent border is off by default, and what
//! separates two panes is a one- or two-pixel divider and nothing else.
//!
//! A tab that has never been split therefore renders a single grid filling the
//! body. That is Warp's collapse rule seen from the front: a group that fell
//! back to one pane is indistinguishable from one that never split — no
//! divider, `in_split_pane == false`.
//!
//! # The output, and what draws it
//!
//! A pane's output is one of two elements, and [`pane_surface::of`] says
//! which. Ordinarily it is [`BlockList`]: the commands that have finished, as
//! a virtualising list, with the open one at the end. A full-screen program —
//! the alternate screen, or anything that has been running on the primary
//! screen for longer than a blink — gets [`TerminalElement`] and the whole
//! pane instead, because a pager with a text field under it is a text field
//! nobody can use. So does a shell with no command marks at all, whose one
//! open block has already grown past the top of the emulator's viewport:
//! there is no block structure to draw, and the honest rendering is the grid.
//!
//! The grid gets the *whole* pane, with no composer under it, and that is not
//! a preference: [`PaneSizer`] tells the pty how many rows the pane holds, and
//! a grid laid out above a composer has fewer rows than that to draw them in,
//! so the newest ones — the prompt, the line being echoed — end up under the
//! field and are never painted.
//!
//! # The composer
//!
//! Under the output, and it is **not a box**. No border, no radius, no fill of
//! its own and no margin: it sits on the pane's own ground, in the terminal's
//! font, sharing the list's left gutter, so the command inside a block and the
//! line being typed under it start at the same x. The only thing that ever
//! separates it from the output is a one-pixel rule in the same role and the
//! same full-bleed extent as the rule the list draws between two blocks — and
//! that rule is drawn only when there is output cut off underneath it, which
//! is why the seam is invisible until it means something.
//!
//! It grows downwards as the line wraps or gains lines and the output gives up
//! the space, which is why it is a plain child of the column and the output is
//! the flexible one.
//!
//! **And where the shell says where its prompt ends, the composer's first row
//! is that prompt's row.** [`block_list::inline_start`] answers whether it is
//! and at which column, from the `B` mark the emulator kept and from where the
//! list is scrolled to; the field then draws its first row above its own box
//! and measures a row shorter, so the caret sits immediately after the `❯`
//! rather than under it. The composer being on a row of its own is what a
//! composer *is* to a reader, however little chrome it carries, and no shell
//! puts its prompt on one line and your typing on the next.
//!
//! The two arrangements are the same three-fact decision seen from both sides:
//! the frame that gains the rule above the composer is the frame the composer
//! takes a row back, because both are "there is output cut off under this".
//!
//! # Where the pty learns its size
//!
//! [`PaneSizer`], from the *pane's* rectangle and never from the output's box.
//! The output's box moves whenever the composer appears or hides, and a
//! `SIGWINCH` on each of those frames is a storm at programs that handle them
//! badly. Reporting the pane also means a full-screen program that fills the
//! pane fits.
//!
//! # When there is no terminal
//!
//! Two cases, and they are told apart on purpose. A run that never started the
//! terminals — the headless snapshot, a test — draws a panel that says the pane
//! has no shell. A run whose shell *failed to start* draws the reason, because
//! "no shell" is a much worse answer than "the login shell is not executable".

use std::sync::Arc;
use std::time::Instant;

use crook_terminal::{Snapshot, TerminalSize};
use crookui_core::element::SizeConstraint;
use crookui_core::elements::Padding;
use crookui_core::event::{DispatchedEvent, Event, MouseButton};
use crookui_core::fonts::{Properties, Weight};
use crookui_core::geometry::{Point, Vector2F, vec2f};
use crookui_core::prelude::*;
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};

use crate::pane_blocks::PaneBlocks;
use crate::pane_split::{DividerDrag, Drag, PaneExtent};
use crate::pane_surface::{self, Surface};
use crate::tab::{Pane, PaneId, SplitAxis, TabAction};
use crate::terminal_font::CellFont;
use crate::terminal_model::TerminalHandle;
use crate::theme::theme;

use super::action::WorkspaceAction;
use super::block_list::{self, BlockList};
use super::input_element::{CommandInput, Ink};
use super::pane_output::Keys;
use super::terminal_element::{TerminalElement, color};
use super::view::Workspace;

/// The gap between a pane's edge and the text inside it, left and right.
///
/// Every logical pixel here is a column the shell does not get. It is paid
/// anyway, and it is paid *once*: the list insets its blocks by it, the grid
/// container insets the grid by it, the composer insets its own text by it,
/// and [`PaneSizer`] takes it off the pane before it works out how many
/// columns to tell the pty about. One number, four uses, which is the whole of
/// why a command echoed by the shell lands under the one that was typed.
pub(super) const GUTTER: f32 = block_list::GUTTER;

/// The gap between a pane's top and bottom edges and the grid inside it.
///
/// A tenth of a cell, which is Warp's vertical grid padding. Small, because
/// unlike the gutter this one is measured in rows.
pub(super) const GRID_VERTICAL_PADDING: f32 = 2.;

/// The line between two panes, and the whole of what separates them.
///
/// Warp's `get_divider_thickness`: one pixel under its minimalist UI flag and
/// two without it. One, and the drag is made easy to hit by [`DIVIDER_GRAB`]
/// rather than by a thicker line — Warp pads its own divider on each side for
/// exactly the same reason.
const DIVIDER_THICKNESS: f32 = 1.;

/// How wide a divider is to grab, as opposed to how wide it is drawn.
///
/// A one-pixel target is not a target. Every window manager and every editor
/// gives a split handle a band around it, and because this is only a hit area
/// the panes on each side are still drawn right up to the line.
const DIVIDER_GRAB: f32 = 9.;

/// The rule between two blocks, and above the composer.
///
/// One pixel, in [`Theme::overlay_2`](crate::theme::Theme::overlay_2) — the
/// foreground at ten per cent, translucent rather than flattened, so it
/// composites over whatever background the shell has actually made. The
/// composer's rule and the list's dividers are the same width, the same colour
/// and the same full-bleed extent, which is what makes the composer read as
/// one more block rather than as a footer.
pub(super) const RULE: f32 = 1.;

/// The space between the composer's rule and the line being composed, in
/// lines.
///
/// A block's own `padding_top`, so the composer's first row sits exactly one
/// block-padding below its rule, as a block's first row does below its
/// divider.
const COMPOSER_PADDING_TOP: f32 = 1.1;

/// The space under the composer's last row, in pixels.
///
/// Warp's `editor_bottom_padding`, and the pane's own bottom inset: nothing
/// else pads the bottom of a pane.
pub(super) const COMPOSER_PADDING_BOTTOM: f32 = 20.;

/// The gap between two chips in the row above the composer.
const CHIP_GAP: f32 = 6.;

/// The gap between the line being composed and the row of chips under it.
const CHIPS_PADDING_TOP: f32 = 8.;

/// How far the floating row is held off the corner it sits in.
///
/// The row is over a screen a program has taken rather than beside a prompt,
/// so it is inset from both edges: a chip flush against the corner reads as
/// part of whatever the program drew there.
const CHIPS_INSET: f32 = 12.;

pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let Some(tab) = workspace.tabs().active() else {
        return Empty::new().finish();
    };
    let panes = tab.panes();

    let mut layout = match panes.axis() {
        SplitAxis::Horizontal => Flex::row(),
        SplitAxis::Vertical => Flex::column(),
    }
    .with_main_axis_size(MainAxisSize::Max);

    let mut previous: Option<PaneId> = None;
    for pane in panes.iter() {
        if let Some(before) = previous {
            // The divider between this pane and the one before it, which is
            // the pair a drag on it moves the boundary of.
            layout.add_child(
                SplitDivider::new(
                    panes.axis(),
                    before,
                    pane.id(),
                    (
                        workspace.pane_extent(before),
                        workspace.pane_extent(pane.id()),
                    ),
                    workspace.divider_drag().clone(),
                )
                .finish(),
            );
        }
        // The pane's own weight, which is one until a divider is dragged
        // between it and a neighbour. Warp gives each node a `PaneFlex(1.0)`
        // and moves away from it on exactly the same gesture.
        let state = PaneState {
            is_focused: panes.is_focused(pane.id()),
        };
        // Measured on the way past, because the divider beside it has no other
        // way to learn how many pixels a weight became.
        let measured = Measured::new(
            panes.axis(),
            workspace.pane_extent(pane.id()),
            panel(workspace, pane, state, app),
        );
        layout.add_child(Expanded::new(pane.flex(), measured.finish()).finish());
        previous = Some(pane.id());
    }

    layout.finish()
}

/// What a pane is drawn as. Warp's `SplitPaneState`, which is likewise
/// resolved by the group rather than kept by the pane, so "two panes are
/// focused" is not representable.
///
/// One field, since the chrome went: whether a pane is in a split no longer
/// changes anything about how it is drawn, only the divider between panes
/// says there is more than one.
#[derive(Copy, Clone)]
struct PaneState {
    is_focused: bool,
}

/// One pane, and a click target that focuses it.
///
/// **No box.** A pane fills its share of the body: no corner radius, no
/// margin, no border, and nothing between it and the next pane but a
/// [`divider`]. That is Warp's arrangement, and it is Warp's for a reason
/// worth copying — a terminal is the thing you are looking at, and a rounded
/// card around it spends contrast, corners and twelve pixels of every edge on
/// chrome that says nothing a person needs to know.
///
/// **And no focus outline.** The card used to carry one: an accent border
/// around the focused pane of a split tab. Warp has the same border and turns
/// it off by default — `show_accent_border` is `false` and only the
/// shared-session path sets it — because the cursor already answers the
/// question. `terminal_element` draws a filled block in the pane that is
/// listening and a hollow one everywhere else, which is the signal at the one
/// place a person is already looking, rather than four hundred pixels of
/// outline at the edges of their vision.
///
/// Warp's other option here is dimming the inactive panes, and it is also off
/// by default (`appearance.panes.should_dim_inactive_panes`). Crook has no
/// setting to hang it on, so it is not implemented rather than implemented
/// with the default nobody chose.
fn panel(
    workspace: &Workspace,
    pane: &Pane,
    state: PaneState,
    app: &AppContext,
) -> Box<dyn Element> {
    let id = pane.id();
    let PaneState { is_focused } = state;

    let Some(interaction) = workspace.interaction(id) else {
        // Unreachable, for the same reason it is in the bar.
        log::error!("pane {id:?} has no interaction state and its panel was skipped");
        return Empty::new().finish();
    };

    // Fetched once and used twice: the grid draws it, and the pane is painted
    // in whatever background it resolved. See `pane_ground`.
    let terminal = workspace.terminal(id, app);
    let ground = pane_ground(terminal.as_ref());

    // **Where the keyboard line is drawn.** A pane takes typing only when it is
    // the focused one *and* nothing is floating over the window: the options
    // menu is modal, and a shell that swallowed the keys while it was up would
    // make the menu unusable. What the menu cannot take away is the three keys
    // that interrupt, end and suspend — see [`Keys::Signals`]. Everything
    // upstream of this — the bindings the help text lists — was already
    // consumed by `Workspace::action_for` in the window delegate, so nothing
    // here has to know which chords those are.
    //
    let keys = match (is_focused, workspace.a_popup_is_open()) {
        (false, _) => Keys::None,
        (true, true) => Keys::Signals,
        (true, false) => Keys::All,
    };
    let content = contents(workspace, pane, terminal, keys, is_focused, app);

    // The `Hoverable` is here for its click handler alone — nothing about a
    // pane changes under the pointer any more — and it is what records the hit
    // rect that makes the pane clickable at all.
    Hoverable::new(interaction.body.clone(), move |_| {
        Container::new(
            // The pane fills the share it was given, whatever its content
            // measured. A grid measures to whole cells and stops short of the
            // remainder, and a container that sized itself to that would leave
            // a strip down the right of every pane and along the bottom that
            // looked like part of the pane and did not focus it when clicked.
            // The card's margin used to hide the ragged edge; nothing hides it
            // now, so it is filled instead.
            Align::new(content).top_left().finish(),
        )
        .with_background_color(ground)
        // **No padding.** Every inset a pane has is applied by the surface
        // inside it, because the two lines that have to run edge to edge — the
        // rule between two blocks and the rule above the composer — would
        // otherwise be inset by it and would stop reading as the same line.
        .finish()
    })
    // Warp wraps every leaf of its tree in the same handler, dispatching
    // `Activate(pane_id, ActivationReason::Click)`. On an unsplit tab this
    // re-focuses the pane that is already focused, which is a no-op that costs
    // no frame.
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::FocusPane(id)));
    })
    .finish()
}

/// What is painted behind a pane, and therefore what its padding is made of.
///
/// The shell's own background, whatever the shell has made of it. That matters
/// more than it looks: `crook_palette` starts a grid on the pane's colour
/// precisely so an untouched screen and the pane around it are one surface,
/// and a pane painted in anything else turns [`GUTTER`] into a visible frame
/// around the grid — which is exactly what a rounded card was, minus the
/// rounding. A shell that sets its own background with OSC 11 moves the pane
/// with it, for the same reason, and it carries the composer too, because the
/// composer fills nothing of its own.
///
/// The fallback is the palette's default rather than a colour of this module's
/// choosing: a pane with no shell yet is about to have one, and it should not
/// change colour when it arrives.
fn pane_ground(terminal: Option<&(TerminalHandle, Arc<Snapshot>)>) -> Color {
    terminal.map_or(theme().surface, |(_, snapshot)| color(snapshot.background))
}

/// The pane's output and the composer under it, or an explanation of why it
/// has neither.
///
/// The terminal is passed in rather than looked up again: `panel` has already
/// asked for it, because the pane is painted in the background this same
/// snapshot resolved.
fn contents(
    workspace: &Workspace,
    pane: &Pane,
    terminal: Option<(TerminalHandle, Arc<Snapshot>)>,
    keys: Keys,
    is_focused: bool,
    app: &AppContext,
) -> Box<dyn Element> {
    let font = workspace.cell_font().clone();

    let Some((handle, snapshot)) = terminal else {
        let reason = workspace.terminal_failure(pane.id(), app).map_or_else(
            || "no shell is running in this pane".to_owned(),
            |failure| format!("the shell could not be started: {failure}"),
        );
        return notice(workspace, pane, reason);
    };

    let id = pane.id();
    let alt_screen = snapshot.alt_screen;
    // Read before the snapshot is handed to whichever surface draws it: the
    // composer is painted in the colours the *shell* resolved, not the
    // theme's, so that a shell which changed them takes the field with it.
    let ink = Ink::of(&snapshot);
    let surface = pane_surface::of(&snapshot, Instant::now());
    // Read before the snapshot goes to the surface, and from the same three
    // facts the list itself reads, so the two elements cannot disagree about
    // whether they share a row. See `block_list::inline_start`.
    let cut_off = is_cut_off(workspace, id);
    let inline = block_list::inline_start(&snapshot, surface, cut_off);
    let output = match surface.surface {
        Surface::Blocks => blocks(workspace, id, &handle, snapshot, font.clone(), keys, app),
        Surface::Grid => grid(workspace, id, &handle, snapshot, font.clone(), keys),
    };

    // What a plugin pinned to this pane, or nothing at all — which is what a
    // release binary carries. Built once and spent in exactly one of the two
    // places below: beside the prompt while there is one, over the corner
    // while a program has the screen.
    let chips = is_focused.then(|| chips(workspace, app)).flatten();

    // The pty is told the *pane's* size, so it is measured here — outside the
    // column, where the composer appearing and hiding cannot move the box the
    // measurement comes from.
    if !surface.composer {
        // The chips float rather than take a row. A program that has the
        // screen was given the whole pane and counts on having it: a row of
        // chrome above `top` would be a row `top` does not know it lost.
        let output = match chips {
            Some(chips) => over_the_corner(output, chips),
            None => output,
        };
        return PaneSizer::new(handle, font, output).finish();
    }

    // The output is the flexible one and the composer is not, so the composer
    // is measured first and the output divides what is left. That is the whole
    // of "the output gives up the space rather than the composer overflowing".
    let column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        // The panel *is* the room there is. A composer grown past it — a
        // six-line command in a pane on a laptop — would be drawn over the
        // pane below it with the output squeezed to nothing above, so it is
        // measured again against what is left instead. Its own ceiling is the
        // other half of this: see `input_element::row_budget`.
        .with_no_overflow()
        .with_child(Expanded::new(1., output).finish())
        .with_child(composer(
            workspace,
            id,
            handle.clone(),
            font.clone(),
            chips,
            ComposerState {
                focused: keys == Keys::All,
                alt_screen,
                cut_off,
                inline,
                ink,
            },
        ))
        .finish();
    PaneSizer::new(handle, font, column).finish()
}

/// Whether there is output cut off under the composer, which is the only thing
/// that draws the rule above it.
///
/// That is exactly "the list is scrolled off its own bottom": while it is
/// following the end there is nothing below the fold and no seam is wanted.
/// The answer comes from the last layout, so the rule appears on the frame
/// after the wheel that earned it — one frame of a hairline, against a second
/// layout pass on every scroll.
///
/// The list is the only surface this is ever asked about: the grid is drawn
/// without a composer, so there is no seam to rule. See [`pane_surface::of`].
fn is_cut_off(workspace: &Workspace, pane: PaneId) -> bool {
    workspace
        .pane_blocks(pane)
        .is_some_and(PaneBlocks::is_cut_off)
}

/// The pane's finished commands as a list, with the open one at the end.
///
/// Full width and full bleed: the list insets its own rows by [`GUTTER`] so
/// that its dividers, its washes and its copy control can reach the pane's
/// edges — which is what makes the composer's rule and a block divider the
/// same line rather than two lines of different lengths.
fn blocks(
    workspace: &Workspace,
    pane: PaneId,
    handle: &TerminalHandle,
    snapshot: Arc<Snapshot>,
    font: CellFont,
    keys: Keys,
    app: &AppContext,
) -> Box<dyn Element> {
    let history = workspace.terminal_blocks(pane, app).unwrap_or_default();
    let view = workspace.pane_blocks(pane).cloned().unwrap_or_default();
    let mut list =
        BlockList::new(history, snapshot, font, view).with_terminal(handle.clone(), keys);
    // The list reads the composer's line to tell an end of input from a
    // delete, so it is given it here for the same reason the grid is.
    if let Some(input) = workspace.input(pane) {
        list = list.with_input(input.clone());
    }
    // The gesture outlives this element by design: a press and the drag that
    // follows it are separated by however many frames the pointer takes to
    // move, and every one of them throws this tree away.
    if let Some(interaction) = workspace.interaction(pane) {
        list = list
            .with_selection(
                pane,
                interaction.selection.clone(),
                workspace.clipboard().clone(),
            )
            .with_links(interaction.links.clone());
    }
    list.finish()
}

/// The pane's output as one grid: a full-screen program, or a shell with no
/// command marks.
fn grid(
    workspace: &Workspace,
    pane: PaneId,
    handle: &TerminalHandle,
    snapshot: Arc<Snapshot>,
    font: CellFont,
    keys: Keys,
) -> Box<dyn Element> {
    let mut grid = TerminalElement::new(snapshot, font).with_terminal(handle.clone(), keys);
    if let Some(input) = workspace.input(pane) {
        grid = grid.with_input(input.clone());
    }
    if let Some(interaction) = workspace.interaction(pane) {
        grid = grid
            .with_selection(
                pane,
                interaction.selection.clone(),
                workspace.clipboard().clone(),
            )
            .with_links(interaction.links.clone());
    }
    // The grid's own gutter, rather than the pane's: the pane has none, so
    // that the composer's rule can run edge to edge.
    Container::new(grid.finish())
        .with_horizontal_padding(GUTTER)
        .with_vertical_padding(GRID_VERTICAL_PADDING)
        .finish()
}

/// What the composer is drawn as: whether it has the keys, which screen the
/// pane is on, and whether anything is cut off above it.
#[derive(Copy, Clone)]
struct ComposerState {
    focused: bool,
    alt_screen: bool,
    cut_off: bool,
    /// The column its first row starts at when it continues the shell's own
    /// prompt line rather than taking a row of its own.
    inline: Option<usize>,
    ink: Ink,
}

/// The pane's command input, on the pane's own ground and in the pane's own
/// font.
///
/// **No box.** No background, no corner radius, no side or bottom border, no
/// margin, and no focus ring — the caret is the whole of the focus
/// affordance, which is Warp's answer and the reason its composer does not
/// read as a widget bolted under the output. What is left is one conditional
/// hairline along the top, at the width and in the role the list draws between
/// two blocks, and the same gutter the output uses.
fn composer(
    workspace: &Workspace,
    pane: PaneId,
    handle: TerminalHandle,
    font: CellFont,
    chips: Option<Box<dyn Element>>,
    state: ComposerState,
) -> Box<dyn Element> {
    let Some(input) = workspace.input(pane) else {
        // Unreachable: an open pane always has one, for the same reason it
        // always has its mouse state.
        log::error!("pane {pane:?} has no input state and its composer was skipped");
        return Empty::new().finish();
    };

    let cell = font.metrics().height;
    let mut composing =
        CommandInput::new(input.clone(), font.clone(), workspace.clipboard().clone())
            .for_pane(pane)
            .with_terminal(handle, state.focused, state.alt_screen)
            .with_inline(state.inline)
            .with_ink(state.ink);
    // So that Enter brings the list back to the block the command is about to
    // make, however far up somebody had scrolled to read.
    if let Some(view) = workspace.pane_blocks(pane) {
        composing = composing.with_blocks(view.clone());
    }
    // The output's selection outranks this field's for the copy chord, and a
    // click in here lets go of it. Both need the same one the output has.
    if let Some(interaction) = workspace.interaction(pane) {
        composing = composing.with_selection(interaction.selection.clone());
    }

    // **Nothing is drawn under the field.** What the shell offered stands
    // after the caret, in the line itself — see
    // [`TextInput::suggestion`](crate::text_input::TextInput::suggestion) —
    // because a list under the composer is a surface that appears and
    // disappears under whatever a person is reading, and it took the output
    // with it every time.
    let mut composing: Box<dyn Element> = composing.finish();

    // **Under the line, not over it.** Warp puts its chips on the row above
    // the one being typed, and that is the one detail of this row that cannot
    // be copied: Warp's input is a box of its own, and Crook's *is the
    // terminal's next row* — drawn in the cell grid, at column zero, on the
    // prompt row the shell itself printed. A row of chips above it would push
    // the line off that prompt, which is the seam this whole arrangement
    // exists to remove. So they go under it, where they read as what they are:
    // facts about the pane, beneath the line they are facts about.
    if let Some(chips) = chips {
        composing = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(composing)
            .with_child(
                Container::new(chips)
                    .with_margin_top(CHIPS_PADDING_TOP)
                    .finish(),
            )
            .finish();
    }

    let rule = if state.cut_off { RULE } else { 0. };
    Container::new(composing)
        .with_border(Border::top(rule).with_border_color(theme().overlay_2))
        .with_padding(Padding {
            // Nothing above the line when there is no rule above it: the
            // composer is then simply the next row of the terminal, directly
            // under the prompt the shell drew, and a gap there would be the
            // seam this whole arrangement exists to remove. When there *is* a
            // rule, it gets a block's own padding_top so that the composer
            // sits under its line exactly as a block sits under its divider —
            // measured from the line, because an inset border has already
            // taken its pixel out of the box.
            top: if state.cut_off {
                (COMPOSER_PADDING_TOP * cell - rule).max(0.)
            } else {
                0.
            },
            left: GUTTER,
            bottom: COMPOSER_PADDING_BOTTOM,
            right: GUTTER,
        })
        .finish()
}

/// Whatever a plugin pinned to this pane, in a row, or `None` when nothing did.
///
/// The slot is a list and this row does not know what is in it — the same
/// bargain the header's right-hand side made, and the reason an empty one has
/// to be as ordinary here as a full one: nothing a release binary carries
/// fills it, and a plugin installed from a file does.
fn chips(workspace: &Workspace, app: &AppContext) -> Option<Box<dyn Element>> {
    let built = workspace
        .host()
        .slots()
        .map(crate::plugins::pane::PANE_CHIPS, |build| {
            build(workspace, app)
        });
    if built.is_empty() {
        return None;
    }

    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(CHIP_GAP);
    row.extend(built);
    Some(row.finish())
}

/// Floats `chips` over the bottom-right corner of `content`.
///
/// An overlay rather than a row, because the pane below it belongs to a
/// program that was told how many rows it has. Anchored corner-to-corner and
/// pulled back inside by [`CHIPS_INSET`], so the row sits *in* the corner
/// rather than hanging off it — and `keep_on_screen` is left on, which is what
/// keeps a wide chip inside a narrow pane.
fn over_the_corner(content: Box<dyn Element>, chips: Box<dyn Element>) -> Box<dyn Element> {
    let mut stack = Stack::new().with_child(content);
    stack.add_anchored_overlay_child(
        chips,
        AnchorTo {
            parent: Corner::BottomRight,
            child: Corner::BottomRight,
            offset: vec2f(-CHIPS_INSET, -CHIPS_INSET),
            keep_on_screen: true,
            keep_clear_of_parent: false,
        },
    );
    stack.finish()
}

/// Resizes a pane's pty from the pane's own rectangle, and draws its child
/// inside it.
///
/// **This is the only place a pty is resized**, and the rectangle it is
/// measured from is the pane's rather than the output's. The output's box
/// changes whenever the composer appears, hides or grows a line, and resizing
/// the child on each of those sends a storm of `SIGWINCH`s at programs that
/// handle them badly. Reporting the pane also means a full-screen program that
/// fills the pane fits it.
///
/// Everything but the resize is pass-through: the child is laid out at this
/// element's own box, painted at its origin, and offered every event first.
struct PaneSizer {
    handle: TerminalHandle,
    font: CellFont,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl PaneSizer {
    /// Wraps `child`, resizing `handle` to whatever box this is given.
    fn new(handle: TerminalHandle, font: CellFont, child: Box<dyn Element>) -> Self {
        Self {
            handle,
            font,
            child,
            size: None,
            origin: None,
        }
    }
}

impl Element for PaneSizer {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        // A parent that leaves an axis open has no pane rectangle to offer,
        // and the smallest acceptable extent is then the only number in the
        // question — the same answer the surfaces inside give.
        let size = vec2f(
            bounded(constraint.max.x(), constraint.min.x()),
            bounded(constraint.max.y(), constraint.min.y()),
        );
        let metrics = self.font.metrics();
        // The pane minus the insets every surface applies, so the count the
        // pty is told does not change when the surface does.
        let (columns, rows) = metrics.grid_for(
            size.x() - GUTTER * 2.,
            size.y() - GRID_VERTICAL_PADDING * 2.,
        );
        self.handle.resize(
            TerminalSize::new(columns, rows)
                .with_cell_size(metrics.width.round() as u16, metrics.height.round() as u16),
        );

        self.size = Some(size);
        self.child.layout(SizeConstraint::strict(size), ctx, app);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// What a panel with no grid says.
fn notice(workspace: &Workspace, pane: &Pane, reason: String) -> Box<dyn Element> {
    let fonts = workspace.fonts();

    Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(9.)
        .with_child(
            Text::new(pane.title().to_owned(), fonts.ui, 16.)
                .with_color(theme().text_primary)
                .with_style(Properties {
                    weight: Weight::Semibold,
                    ..Default::default()
                })
                .finish(),
        )
        .with_child(
            Text::new(reason, fonts.monospace, 12.5)
                .with_color(theme().text_muted)
                .finish(),
        )
        .finish()
}

/// An extent to lay out at, given a maximum that may be unbounded.
fn bounded(max: f32, min: f32) -> f32 {
    if max.is_finite() { max } else { min }
}

/// A hairline between two panes.
///
/// A line and nothing else: this one is not draggable, so the panes always
/// share the tab equally. When it is made draggable it has to move out of the
/// flex and into a [`Stack`] above the panes, which is where Warp keeps its
/// dividers and for the reason its comment gives
/// (`app/src/pane_group/tree.rs:1097`) — pane content painted after a divider
/// otherwise steals the hit box a person is aiming at.
fn divider(axis: SplitAxis) -> Box<dyn Element> {
    let line = Container::new(Empty::new().finish())
        .with_background_color(theme().border)
        .finish();

    // Edge to edge, with no margin along its length. It used to stop twenty
    // pixels short of both ends, which was right when each pane was a rounded
    // card with a gutter around it: a full-length line would have crossed the
    // gutter and pointed at nothing. Now the panes meet, and the line is where
    // they meet.
    let line = ConstrainedBox::new(line);
    match axis {
        SplitAxis::Horizontal => line.with_width(DIVIDER_THICKNESS).finish(),
        SplitAxis::Vertical => line.with_height(DIVIDER_THICKNESS).finish(),
    }
}

/// Records how many pixels its child was given along one axis.
///
/// The one thing a divider cannot work out for itself. The group holds
/// weights; a `Flex` turns them into pixels and then forgets; and a drag of
/// twenty pixels has to become a share before the group can act on it. So the
/// pane writes down what it measured and the divider beside it reads it.
///
/// It is not a `PaneSizer` because a pane whose shell has not started has no
/// pty to size and is
/// still a pane a divider can be dragged against.
struct Measured {
    axis: SplitAxis,
    extent: PaneExtent,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Measured {
    fn new(axis: SplitAxis, extent: PaneExtent, child: Box<dyn Element>) -> Self {
        Self {
            axis,
            extent,
            child,
            size: None,
            origin: None,
        }
    }
}

impl Element for Measured {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.extent.set(match self.axis {
            SplitAxis::Horizontal => size.x(),
            SplitAxis::Vertical => size.y(),
        });
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// A divider that can be dragged, moving the boundary between the two panes it
/// separates.
///
/// # Why it takes the pixels and reports a ratio
///
/// Nothing outside the layout knows how wide a pane is: the group holds
/// weights, and what those become in pixels is decided by a `Flex` that has
/// already finished by the time a pointer arrives. So the two panes measure
/// themselves — [`PaneSizer`] already computes exactly that in order to size
/// the pty — and leave the number where this can read it. A drag then has
/// everything it needs: the pair covers `before + after` pixels, moving the
/// divider `d` of them makes it `before + d` and `after - d`, and the *share*
/// that falls to the first is what the group can act on however many other
/// panes are beside them.
///
/// # Why it is wider than the line it draws
///
/// A one-pixel target is not a target. The hit area is
/// [`DIVIDER_GRAB`] across, centred on the line, which is what every window
/// manager and every editor does with a split handle — and because it is only
/// a hit area, the pane on each side is drawn right up to the line.
struct SplitDivider {
    axis: SplitAxis,
    before: PaneId,
    after: PaneId,
    /// The two panes' measured extents, which they write during layout.
    extents: (PaneExtent, PaneExtent),
    /// The drag in progress, shared with every other divider so that only one
    /// can be dragged at a time.
    drag: DividerDrag,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl SplitDivider {
    fn new(
        axis: SplitAxis,
        before: PaneId,
        after: PaneId,
        extents: (PaneExtent, PaneExtent),
        drag: DividerDrag,
    ) -> Self {
        Self {
            axis,
            before,
            after,
            extents,
            drag,
            child: divider(axis),
            size: None,
            origin: None,
        }
    }

    /// How far along the dragged axis a window position is.
    fn along(&self, position: Vector2F) -> f32 {
        match self.axis {
            SplitAxis::Horizontal => position.x(),
            SplitAxis::Vertical => position.y(),
        }
    }

    /// The rectangle a press has to land in, which is wider than the line.
    fn grab_area(&self) -> Option<RectF> {
        let bounds = self.bounds()?;
        let grow = (DIVIDER_GRAB - DIVIDER_THICKNESS).max(0.) / 2.;
        Some(match self.axis {
            SplitAxis::Horizontal => RectF::new(
                bounds.origin() - vec2f(grow, 0.),
                bounds.size() + vec2f(grow * 2., 0.),
            ),
            SplitAxis::Vertical => RectF::new(
                bounds.origin() - vec2f(0., grow),
                bounds.size() + vec2f(0., grow * 2.),
            ),
        })
    }

    /// Takes the press, if it landed on this divider.
    fn press(&self, position: Vector2F, click_count: u32, ctx: &mut EventContext) -> bool {
        if !self
            .grab_area()
            .is_some_and(|area| area.contains_point(position))
        {
            return false;
        }

        // A double click evens the split out again, which is the only way back
        // once a divider has been dragged and is what every editor does.
        if click_count >= 2 {
            ctx.dispatch_typed_action(TabAction::EvenPanes);
            return true;
        }

        let (before, after) = (self.extents.0.get(), self.extents.1.get());
        // A pane that has never been laid out has no extent to divide, which
        // is the frame before the first one. There is nothing to drag yet.
        if before + after <= 0. {
            return false;
        }
        self.drag.begin(Drag {
            before: self.before,
            after: self.after,
            anchor: self.along(position),
            before_extent: before,
            after_extent: after,
        });
        true
    }

    /// Moves the boundary to wherever the pointer has got to.
    ///
    /// Not hit-tested: a divider drag routinely leaves the few pixels it
    /// started in, and the pointer is free to travel the whole window. What
    /// keeps another divider out of it is that the drag names the pair it
    /// began between.
    fn drag_to(&self, position: Vector2F, ctx: &mut EventContext) -> bool {
        let Some(drag) = self.drag.get().filter(|drag| drag.before == self.before) else {
            return false;
        };

        let total = drag.before_extent + drag.after_extent;
        if total <= 0. {
            return false;
        }
        let moved = self.along(position) - drag.anchor;
        let leading = ((drag.before_extent + moved) / total).clamp(0., 1.);

        ctx.dispatch_typed_action(TabAction::ResizePanes {
            before: drag.before,
            after: drag.after,
            leading,
        });
        true
    }
}

impl Element for SplitDivider {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        // The rest of a drag this divider owns is not hit-tested, for the
        // reason a selection's is not: the pointer has left the line by the
        // first pixel of the gesture.
        match event.raw_event() {
            Event::MouseDragged {
                button: MouseButton::Left,
                position,
                ..
            } => return self.drag_to(*position, ctx),
            Event::MouseUp {
                button: MouseButton::Left,
                ..
            } => {
                return self
                    .drag
                    .get()
                    .is_some_and(|drag| drag.before == self.before)
                    && self.drag.end();
            }
            _ => {}
        }

        let Some(z_index) = self.z_index() else {
            return self.child.dispatch_event(event, ctx, app);
        };
        if let Some(Event::MouseDown {
            button: MouseButton::Left,
            position,
            click_count,
            ..
        }) = event.at_z_index(z_index, ctx)
            && self.press(*position, *click_count, ctx)
        {
            return true;
        }

        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
