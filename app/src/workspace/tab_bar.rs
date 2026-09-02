//! The tab strip: one rounded rect per row the strip says to draw.
//!
//! A row is four things — a status dot, a clipped label, a close button and
//! the box around them — and three gestures: click to focus, middle-click to
//! close, and the close button. Every handler *emits* an action and none of
//! them touches the strip, so no id captured while rendering can be stale by
//! the time the frame is over.
//!
//! A row stands for a *pane*, in both granularities. Under `Panes` a tab
//! contributes one row per pane it holds; under `Tabs` it contributes one, for
//! its focused pane. That is Warp's rule, and it is why exactly one row in the
//! whole bar is selected however many are drawn: `is_active_tab && is_focused`
//! (`app/src/workspace/view/vertical_tabs.rs:412`). Tinting every row of the
//! active tab and marking the focused pane separately would give a bar where
//! three rows look chosen.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::settings::Granularity;
use crate::tab::{AgentStatus, PaneId, TabAction, TabId};
use crate::theme::THEME;

use super::view::Workspace;
use super::{CLOSE_BUTTON_SIZE, STATUS_DOT_SIZE, TAB_MAX_WIDTH};

/// The whole strip, plus the button that opens another tab.
pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_spacing(4.)
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::End);

    for (tab, pane) in workspace.tabs().rows(workspace.granularity()) {
        row.add_child(render_row(workspace, tab, pane));
    }
    row.add_child(new_tab_button(workspace));

    // Soaks up whatever the tabs did not need, so they stay packed against the
    // left edge instead of spreading themselves across the bar. A flex factor
    // below one deliberately: tabs get first call on the width and reach their
    // cap before the spacer takes anything.
    row.add_child(Expanded::new(0.5, Empty::new().finish()).finish());
    row.finish()
}

fn render_row(workspace: &Workspace, tab: TabId, pane: PaneId) -> Box<dyn Element> {
    let strip = workspace.tabs();
    let ui = workspace.fonts().ui;

    let (Some(tab_data), Some(interaction)) = (strip.get(tab), workspace.interaction(pane)) else {
        // Unreachable: the ids came from `rows`, and `Workspace::apply` is the
        // only thing that can open a pane and always adds its mouse state.
        // Drawing nothing beats panicking a frame over it.
        log::error!("pane {pane:?} has no tab or no interaction state and was skipped");
        return Empty::new().finish();
    };
    let Some(pane_data) = tab_data.panes().get(pane) else {
        log::error!("pane {pane:?} is not in the tab `rows` paired it with");
        return Empty::new().finish();
    };

    let title = pane_data.title().to_owned();
    let status = pane_data.status();
    // The one selected row in the whole bar.
    let is_selected = strip.is_active(tab) && tab_data.panes().is_focused(pane);

    // Under `Tabs` the row stands for its whole tab, so its close button
    // closes the tab — which is what Warp's tab-group header button does.
    // Under `Panes` it closes the pane it names, and the tab only goes if that
    // was the last one.
    let close_action = match workspace.granularity() {
        Granularity::Panes => TabAction::ClosePane(pane),
        Granularity::Tabs => TabAction::Close(tab),
    };

    let close_state = interaction.close.clone();
    let guard = interaction.close.clone();

    let element = Hoverable::new(interaction.chip.clone(), move |state| {
        let hovered = state.is_hovered();

        // The close button is only *drawn* on the row under the cursor and on
        // the selected one, but its slot is reserved on every row in every
        // state. Warp instead freezes every tab to a measured width for as
        // long as a close button is hovered; a permanently reserved slot is
        // the same fix with no state to keep and nothing to get out of step.
        let show_close = hovered || is_selected;
        if !show_close {
            // A row that stopped drawing its close button must also stop
            // believing the pointer is on it, or the guard in the click
            // handler below would swallow this row's next click forever.
            close_state.lock().reset_interaction_state();
        }

        let (fill, border) = if is_selected {
            (THEME.tab_active, THEME.border)
        } else if hovered {
            (THEME.surface, THEME.tab_inactive)
        } else {
            (THEME.tab_inactive, THEME.tab_inactive)
        };

        // A row is given `bar width / row count` and no minimum, so past a
        // dozen or so of them the dot, the label and the close slot need more
        // room than the row has. The overflow is not cosmetic: an unclipped
        // close button paints over the *next* row and hit-tests there too, so
        // aiming at a row to select it would close its neighbour's agent
        // session. Clipping to the row's own box means a row too narrow for
        // its close button simply stops showing one.
        Clipped::new(
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(status_dot(status))
                    .with_child(Expanded::new(1., label(title, ui, is_selected)).finish())
                    .with_child(close_slot(close_action, close_state, show_close, ui))
                    .finish(),
            )
            .with_background_color(fill)
            .with_border(Border::all(1.).with_border_color(border))
            .with_corner_radius(CornerRadius::with_top(Radius::Pixels(8.)))
            .with_padding(Padding {
                top: 5.,
                left: 9.,
                bottom: 6.,
                right: 6.,
            })
            .finish(),
        )
        .finish()
    })
    .on_click(move |_, ctx, _| {
        // The close button is a descendant, so a release over it hit-tests
        // true for both. Addressing panes by id already makes the double fire
        // harmless — the close lands first and focusing a closed id is a
        // no-op — but activating a row as it disappears still flickers.
        if guard.lock().is_hovered() {
            return;
        }
        ctx.dispatch_typed_action(TabAction::FocusPane(pane));
    })
    .on_middle_click(move |_, ctx, _| ctx.dispatch_typed_action(close_action))
    .finish();

    Expanded::new(
        1.,
        ConstrainedBox::new(element)
            .with_max_width(TAB_MAX_WIDTH)
            .finish(),
    )
    .finish()
}

/// The agent's status, as the one coloured thing on an unselected row.
fn status_dot(status: AgentStatus) -> Box<dyn Element> {
    let color = match status {
        AgentStatus::Idle => THEME.border,
        AgentStatus::Running => THEME.accent,
        AgentStatus::NeedsInput => THEME.usage_high,
        AgentStatus::Failed => THEME.usage_critical,
    };

    Container::new(
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(color)
                .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                .finish(),
        )
        .with_width(STATUS_DOT_SIZE)
        .with_height(STATUS_DOT_SIZE)
        .finish(),
    )
    .with_margin_right(7.)
    .finish()
}

/// The title. Clipped rather than wrapped or ellipsised: the shaper drops the
/// glyphs that do not fit the width it was given, which is the whole of it.
fn label(title: String, ui: FamilyId, is_selected: bool) -> Box<dyn Element> {
    let (color, weight) = if is_selected {
        (THEME.text_primary, Weight::Semibold)
    } else {
        (THEME.text_muted, Weight::Normal)
    };

    Text::new(title, ui, 12.5)
        .with_color(color)
        .with_style(Properties {
            weight,
            ..Default::default()
        })
        .finish()
}

/// A fixed square, holding the close button or holding nothing.
fn close_slot(
    action: TabAction,
    state: MouseStateHandle,
    visible: bool,
    ui: FamilyId,
) -> Box<dyn Element> {
    let inner: Box<dyn Element> = if visible {
        Hoverable::new(state, move |state| {
            let hovered = state.is_hovered();
            Container::new(
                Align::new(
                    Text::new("\u{00d7}", ui, 14.)
                        .with_color(if hovered {
                            THEME.text_primary
                        } else {
                            THEME.text_muted
                        })
                        .finish(),
                )
                .finish(),
            )
            .with_background_color(if hovered {
                THEME.border
            } else {
                Color::TRANSPARENT
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish()
        })
        .on_click(move |_, ctx, _| ctx.dispatch_typed_action(action))
        .finish()
    } else {
        Empty::new().finish()
    };

    ConstrainedBox::new(inner)
        .with_width(CLOSE_BUTTON_SIZE)
        .with_height(CLOSE_BUTTON_SIZE)
        .finish()
}

fn new_tab_button(workspace: &Workspace) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;

    ConstrainedBox::new(
        Hoverable::new(workspace.new_tab_state(), move |state| {
            let hovered = state.is_hovered();
            Container::new(
                Align::new(
                    Text::new("+", ui, 16.)
                        .with_color(if hovered {
                            THEME.text_primary
                        } else {
                            THEME.text_muted
                        })
                        .finish(),
                )
                .finish(),
            )
            .with_background_color(if hovered {
                THEME.tab_active
            } else {
                Color::TRANSPARENT
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.)))
            .finish()
        })
        .on_click(|_, ctx, _| ctx.dispatch_typed_action(TabAction::New))
        .finish(),
    )
    .with_width(24.)
    .with_height(24.)
    .finish()
}
