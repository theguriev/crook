//! The active tab's body: one pane per pane the tab holds, each running a
//! shell — or, for one of them, the settings page.
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
//! # The field
//!
//! Under the grid, and only on the normal screen, is the pane's command input:
//! a bordered box that reads as a text field, where the next command is
//! composed before any of it reaches the shell. It grows downwards as the line
//! wraps or gains lines and the grid gives up the space, which is why it is a
//! plain child of the column and the grid is the flexible one.
//!
//! Whether it is drawn at all is [`input_keys::shows_input`], the visible half
//! of the rule [`input_keys::route`] states: a program that has taken the alt
//! screen is not reading a line, so there is no line to compose and every key
//! goes to it instead. The settings page is the other pane without one: it has
//! no shell to send a line to, and every control on it is a click.
//!
//! # When there is no terminal
//!
//! Two cases, and they are told apart on purpose. A run that never started the
//! terminals — the headless snapshot, a test — draws a panel that says the pane
//! has no shell. A run whose shell *failed to start* draws the reason, because
//! "no shell" is a much worse answer than "the login shell is not executable".

use std::sync::Arc;

use crook_terminal::Snapshot;
use crookui_core::fonts::{Properties, Weight};
use crookui_core::prelude::*;

use crate::input_keys;
use crate::tab::{Pane, PaneId, SplitAxis, TabAction};
use crate::terminal_font::CellFont;
use crate::terminal_model::TerminalHandle;
use crate::theme::THEME;

use super::action::WorkspaceAction;
use super::input_element::CommandInput;
use super::settings_page;
use super::terminal_element::{Keys, TerminalElement, color};
use super::view::Workspace;

/// The gap between a pane's edge and the grid inside it.
///
/// Every logical pixel here is a column or a row the shell does not get, which
/// is why it is small.
const GRID_PADDING: f32 = 8.;

/// The line between two panes, and the whole of what separates them.
///
/// Warp's `get_divider_thickness`: one pixel under its minimalist UI flag and
/// two without it. One, because Crook has no drag to make a thicker line
/// easier to grab — Warp pads its divider by four on each side for exactly
/// that and only when the thin one is in use.
const DIVIDER_THICKNESS: f32 = 1.;

/// How round the input field's box is.
///
/// Tighter than the panel around it, which is what makes it read as something
/// inside the panel rather than as a second panel.
pub(super) const FIELD_RADIUS: f32 = 6.;

/// The gap between the field's border and the line being composed.
pub(super) const FIELD_PADDING: f32 = 6.;

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

    for (index, pane) in panes.iter().enumerate() {
        if index > 0 {
            layout.add_child(divider(panes.axis()));
        }
        // Equal flex, tight: every pane gets the same share of the tab. Warp
        // gives each node a `PaneFlex(1.0)` too and only moves away from it
        // when a divider is dragged, which is the one part of a split this
        // does not do.
        let state = PaneState {
            is_focused: panes.is_focused(pane.id()),
        };
        layout.add_child(Expanded::new(1., panel(workspace, pane, state, app)).finish());
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
    let is_settings = pane.is_settings();
    let PaneState { is_focused } = state;

    let Some(interaction) = workspace.interaction(id) else {
        // Unreachable, for the same reason it is in the bar.
        log::error!("pane {id:?} has no interaction state and its panel was skipped");
        return Empty::new().finish();
    };

    // Fetched once and used twice: the grid draws it, and the pane is painted
    // in whatever background it resolved. See `pane_ground`.
    let terminal = if is_settings {
        None
    } else {
        workspace.terminal(id, app)
    };
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
    // The settings page is not in that argument: it is a pane with no shell,
    // and every control on it is a click. There is nothing to give the
    // keyboard to, and nothing to compose a line for either, so it is the one
    // pane that is drawn without a grid and without a field.
    let content = if is_settings {
        settings_page::render(workspace, app)
    } else {
        let keys = match (is_focused, workspace.is_options_menu_open()) {
            (false, _) => Keys::None,
            (true, true) => Keys::Signals,
            (true, false) => Keys::All,
        };
        contents(workspace, pane, terminal, keys, app)
    };

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
        .with_uniform_padding(if is_settings { 0. } else { GRID_PADDING })
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
/// and a pane painted in anything else turns [`GRID_PADDING`] into a visible
/// frame around the grid — which is exactly what a rounded card was, minus the
/// rounding. A shell that sets its own background with OSC 11 moves the pane
/// with it, for the same reason.
///
/// The fallback is the palette's default rather than a colour of this module's
/// choosing: a pane with no shell yet is about to have one, and it should not
/// change colour when it arrives.
fn pane_ground(terminal: Option<&(TerminalHandle, Arc<Snapshot>)>) -> Color {
    terminal.map_or(THEME.surface, |(_, snapshot)| color(snapshot.background))
}

/// The pane's grid and the field under it, or an explanation of why it has
/// neither.
///
/// The terminal is passed in rather than looked up again: `panel` has already
/// asked for it, because the pane is painted in the background this same
/// snapshot resolved.
fn contents(
    workspace: &Workspace,
    pane: &Pane,
    terminal: Option<(TerminalHandle, Arc<Snapshot>)>,
    keys: Keys,
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

    let alt_screen = snapshot.alt_screen;
    let mut grid = TerminalElement::new(snapshot, font.clone()).with_terminal(handle.clone(), keys);
    // The grid reads the field's line to tell an end of input from a delete,
    // so it is given the field even on the screen that does not draw one.
    if let Some(input) = workspace.input(pane.id()) {
        grid = grid.with_input(input.clone());
    }
    let grid = grid.finish();
    if !input_keys::shows_input(alt_screen) {
        return grid;
    }

    // The grid is the flexible one and the field is not, so the field is
    // measured first and the grid divides what is left. That is the whole of
    // "the grid gives up the space rather than the field overflowing".
    Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        // The panel *is* the room there is. A field grown past it — a six-line
        // command in a pane on a laptop — would be drawn over the panel's own
        // border and over the pane below it, with the grid squeezed to nothing
        // above it, so the field is measured again against what is left
        // instead. Its own ceiling is the other half of this: see `row_budget`.
        .with_no_overflow()
        .with_child(Expanded::new(1., grid).finish())
        .with_child(field(
            workspace,
            pane.id(),
            handle,
            font,
            FieldState {
                focused: keys == Keys::All,
                alt_screen,
            },
        ))
        .finish()
}

/// What the field is drawn as: whether it has the keys, and which screen the
/// pane is on.
#[derive(Copy, Clone)]
struct FieldState {
    focused: bool,
    alt_screen: bool,
}

/// The pane's command input, in the box that makes it read as a text field.
fn field(
    workspace: &Workspace,
    pane: PaneId,
    handle: TerminalHandle,
    font: CellFont,
    state: FieldState,
) -> Box<dyn Element> {
    let Some(input) = workspace.input(pane) else {
        // Unreachable: an open pane always has one, for the same reason it
        // always has its mouse state.
        log::error!("pane {pane:?} has no input state and its field was skipped");
        return Empty::new().finish();
    };

    let composing = CommandInput::new(input.clone(), font, workspace.clipboard().clone())
        .with_terminal(handle, state.focused, state.alt_screen)
        .finish();

    // Darker than the panel it sits in rather than lighter, which is what a
    // text field looks like on a dark ground: a well to type into.
    let border = if state.focused {
        THEME.accent
    } else {
        THEME.border
    };
    Container::new(composing)
        .with_background_color(THEME.ground)
        .with_border(Border::all(1.).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(FIELD_RADIUS)))
        .with_margin_top(GRID_PADDING)
        .with_uniform_padding(FIELD_PADDING)
        .finish()
}

/// What a panel with no grid says.
fn notice(workspace: &Workspace, pane: &Pane, reason: String) -> Box<dyn Element> {
    let fonts = workspace.fonts();

    Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_spacing(9.)
        .with_child(
            Text::new(pane.title().to_owned(), fonts.ui, 16.)
                .with_color(THEME.text_primary)
                .with_style(Properties {
                    weight: Weight::Semibold,
                    ..Default::default()
                })
                .finish(),
        )
        .with_child(
            Text::new(reason, fonts.monospace, 12.5)
                .with_color(THEME.text_muted)
                .finish(),
        )
        .finish()
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
        .with_background_color(THEME.border)
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
