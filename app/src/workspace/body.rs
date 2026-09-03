//! The active tab's body: one panel per pane it holds.
//!
//! There is no pseudo-terminal in v1 and no terminal emulation, so a panel
//! says so rather than pretending. What it does prove is the part that matters
//! at this stage: only the active tab's content is in the element tree, it is
//! rebuilt from the panes the strip says are there, and a split is something
//! you can see rather than a number in a model.
//!
//! A tab that has never been split renders exactly the panel it always did.
//! That is Warp's collapse rule seen from the front: a group that fell back to
//! one pane is indistinguishable from one that never split — no divider, no
//! accent border, `in_split_pane == false`.

use crookui_core::fonts::{Properties, Weight};
use crookui_core::prelude::*;

use crate::tab::{Pane, SplitAxis, TabAction};
use crate::theme::THEME;

use super::action::WorkspaceAction;
use super::view::Workspace;

pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
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
        layout.add_child(Expanded::new(1., panel(workspace, pane, state)).finish());
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

/// One pane's panel: what it is called, what its agent is doing, and a click
/// target that focuses it.
fn panel(workspace: &Workspace, pane: &Pane, state: PaneState) -> Box<dyn Element> {
    let fonts = workspace.fonts();
    let id = pane.id();
    let title = pane.title().to_owned();
    let command = format!("$ crook run --agent {title}");
    let status = pane.status();
    let PaneState {
        is_focused,
        is_split,
    } = state;

    let Some(interaction) = workspace.interaction(id) else {
        // Unreachable, for the same reason it is in the bar.
        log::error!("pane {id:?} has no interaction state and its panel was skipped");
        return Empty::new().finish();
    };

    Hoverable::new(interaction.body.clone(), move |mouse| {
        // An unsplit tab has nothing to distinguish its one pane from, so it
        // keeps the plain panel it has always had. Warp says the same thing
        // with `SplitPaneState::NotInSplitPane`, which suppresses the
        // active-pane indicator outright.
        let (fill, border) = if !is_split {
            (THEME.surface, THEME.border)
        } else if is_focused {
            (THEME.surface, THEME.accent)
        } else if mouse.is_hovered() {
            (THEME.surface, THEME.border)
        } else {
            (THEME.ground, THEME.border)
        };

        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Max)
                .with_spacing(9.)
                .with_child(
                    Text::new(title, fonts.ui, 16.)
                        .with_color(THEME.text_primary)
                        .with_style(Properties {
                            weight: Weight::Semibold,
                            ..Default::default()
                        })
                        .finish(),
                )
                .with_child(
                    Text::new(
                        format!("agent session \u{b7} {}", status.label()),
                        fonts.ui,
                        12.,
                    )
                    .with_color(THEME.text_muted)
                    .finish(),
                )
                .with_child(
                    Text::new(command, fonts.monospace, 12.5)
                        .with_color(THEME.text_muted)
                        .finish(),
                )
                .with_child(
                    Text::new(
                        "# placeholder: v1 renders a session, it does not run one",
                        fonts.monospace,
                        12.5,
                    )
                    .with_color(THEME.text_muted.with_alpha(120))
                    .finish(),
                )
                .finish(),
        )
        .with_background_color(fill)
        .with_border(Border::all(1.).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
        .with_uniform_margin(12.)
        .with_uniform_padding(16.)
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
