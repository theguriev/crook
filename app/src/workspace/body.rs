//! The active tab's body: one panel per pane it holds, each running a shell.
//!
//! A panel is the pane's terminal and nothing else. What the session is called
//! and what its agent is doing are already in the tab strip beside it, and a
//! heading repeated inside every panel would cost two rows of grid to say what
//! is on screen twice — Warp draws its panes the same way, for the same reason.
//!
//! A tab that has never been split renders exactly the panel it always did.
//! That is Warp's collapse rule seen from the front: a group that fell back to
//! one pane is indistinguishable from one that never split — no divider, no
//! accent border, `in_split_pane == false`.
//!
//! # When there is no terminal
//!
//! Two cases, and they are told apart on purpose. A run that never started the
//! terminals — the headless snapshot, a test — draws a panel that says the pane
//! has no shell. A run whose shell *failed to start* draws the reason, because
//! "no shell" is a much worse answer than "the login shell is not executable".

use crookui_core::fonts::{Properties, Weight};
use crookui_core::prelude::*;

use crate::tab::{Pane, SplitAxis, TabAction};
use crate::theme::THEME;

use super::action::WorkspaceAction;
use super::terminal_element::TerminalElement;
use super::view::Workspace;

/// The gap between a panel's edge and the grid inside it.
///
/// Smaller than the placeholder panel's padding was: every logical pixel here
/// is a column or a row the shell does not get.
const GRID_PADDING: f32 = 8.;

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
            is_split: panes.is_split(),
        };
        layout.add_child(Expanded::new(1., panel(workspace, pane, state, app)).finish());
    }

    layout.finish()
}

/// What a panel is drawn as. Warp's `SplitPaneState`, which is likewise
/// resolved by the group rather than kept by the pane, so "two panes are
/// focused" is not representable.
#[derive(Copy, Clone)]
struct PaneState {
    is_focused: bool,
    is_split: bool,
}

/// One pane's panel: the grid its shell is drawing, and a click target that
/// focuses it.
fn panel(
    workspace: &Workspace,
    pane: &Pane,
    state: PaneState,
    app: &AppContext,
) -> Box<dyn Element> {
    let id = pane.id();
    let PaneState {
        is_focused,
        is_split,
    } = state;

    let Some(interaction) = workspace.interaction(id) else {
        // Unreachable, for the same reason it is in the bar.
        log::error!("pane {id:?} has no interaction state and its panel was skipped");
        return Empty::new().finish();
    };

    // **Where the keyboard line is drawn.** A pane takes typing only when it is
    // the focused one *and* nothing is floating over the window: the options
    // menu is modal, and a shell that swallowed the keys while it was up would
    // make the menu unusable. Everything upstream of this — the bindings the
    // help text lists — was already consumed by `Workspace::action_for` in the
    // window delegate, so nothing here has to know which chords those are.
    let accepts_input = is_focused && !workspace.is_options_menu_open();
    let content = grid(workspace, pane, accepts_input, app);

    Hoverable::new(interaction.body.clone(), move |mouse| {
        // An unsplit tab has nothing to distinguish its one pane from, so it
        // keeps the plain panel it has always had. Warp says the same thing
        // with `SplitPaneState::NotInSplitPane`, which suppresses the
        // active-pane indicator outright.
        let border = if !is_split {
            THEME.border
        } else if is_focused {
            THEME.accent
        } else if mouse.is_hovered() {
            THEME.text_muted
        } else {
            THEME.border
        };

        Container::new(content)
            .with_background_color(THEME.surface)
            .with_border(Border::all(1.).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
            .with_uniform_margin(12.)
            .with_uniform_padding(GRID_PADDING)
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

/// The pane's grid, or an explanation of why it has none.
fn grid(
    workspace: &Workspace,
    pane: &Pane,
    accepts_input: bool,
    app: &AppContext,
) -> Box<dyn Element> {
    let font = workspace.cell_font().clone();

    if let Some((handle, snapshot)) = workspace.terminal(pane.id(), app) {
        return TerminalElement::new(snapshot, font)
            .with_terminal(handle, accepts_input)
            .finish();
    }

    let reason = workspace.terminal_failure(pane.id(), app).map_or_else(
        || "no shell is running in this pane".to_owned(),
        |failure| format!("the shell could not be started: {failure}"),
    );
    notice(workspace, pane, reason)
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

    match axis {
        SplitAxis::Horizontal => Container::new(ConstrainedBox::new(line).with_width(1.).finish())
            .with_vertical_margin(20.)
            .finish(),
        SplitAxis::Vertical => Container::new(ConstrainedBox::new(line).with_height(1.).finish())
            .with_horizontal_margin(20.)
            .finish(),
    }
}
