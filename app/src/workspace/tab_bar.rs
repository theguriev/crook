//! The tab strip: one rounded rect per agent session.
//!
//! A tab is four things — a status dot, a clipped label, a close button and
//! the box around them — and three gestures: click to select, middle-click to
//! close, and the close button. Every handler *emits* an action and none of
//! them touches the strip, so no id captured while rendering can be stale by
//! the time the frame is over.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::tab::{AgentStatus, Tab, TabAction, TabId};
use crate::theme::THEME;

use super::view::Workspace;
use super::{CLOSE_BUTTON_SIZE, STATUS_DOT_SIZE, TAB_MAX_WIDTH};

/// The whole strip, plus the button that opens another tab.
pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_spacing(4.)
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::End);

    for tab in workspace.tabs().iter() {
        row.add_child(render_tab(workspace, tab));
    }
    row.add_child(new_tab_button(workspace));

    // Soaks up whatever the tabs did not need, so they stay packed against the
    // left edge instead of spreading themselves across the bar. A flex factor
    // below one deliberately: tabs get first call on the width and reach their
    // cap before the spacer takes anything.
    row.add_child(Expanded::new(0.5, Empty::new().finish()).finish());
    row.finish()
}

fn render_tab(workspace: &Workspace, tab: &Tab) -> Box<dyn Element> {
    let id = tab.id();
    let is_active = workspace.tabs().is_active(id);
    let ui = workspace.fonts().ui;
    let title = tab.title().to_owned();
    let status = tab.status();

    let Some(interaction) = workspace.interaction(id) else {
        // Unreachable: `Workspace::apply` is the only thing that can add a tab
        // and it always adds the matching state. Drawing nothing beats
        // panicking a frame over it.
        log::error!("tab {id:?} has no interaction state and was skipped");
        return Empty::new().finish();
    };

    let close_state = interaction.close.clone();
    let guard = interaction.close.clone();

    let element = Hoverable::new(interaction.tab.clone(), move |state| {
        let hovered = state.is_hovered();

        // The close button is only *drawn* on the tab under the cursor and on
        // the active one, but its slot is reserved on every tab in every
        // state. Warp instead freezes every tab to a measured width for as
        // long as a close button is hovered; a permanently reserved slot is
        // the same fix with no state to keep and nothing to get out of step.
        let show_close = hovered || is_active;
        if !show_close {
            // A tab that stopped drawing its close button must also stop
            // believing the pointer is on it, or the guard in the click
            // handler below would swallow this tab's next click forever.
            close_state.lock().reset_interaction_state();
        }

        let (fill, border) = if is_active {
            (THEME.tab_active, THEME.border)
        } else if hovered {
            (THEME.surface, THEME.tab_inactive)
        } else {
            (THEME.tab_inactive, THEME.tab_inactive)
        };

        // A tab is given `bar width / tab count` and no minimum, so past a
        // dozen or so tabs the dot, the label and the close slot need more
        // room than the tab has. The overflow is not cosmetic: an unclipped
        // close button paints over the *next* tab and hit-tests there too, so
        // aiming at a tab to select it would close its neighbour's agent
        // session. Clipping to the tab's own box means a tab too narrow for
        // its close button simply stops showing one.
        Clipped::new(
            Container::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(status_dot(status))
                    .with_child(Expanded::new(1., label(title, ui, is_active)).finish())
                    .with_child(close_slot(id, close_state, show_close, ui))
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
        // true for both. Addressing tabs by id already makes the double fire
        // harmless — the close lands first and selecting a closed id is a
        // no-op — but activating a tab as it disappears still flickers.
        if guard.lock().is_hovered() {
            return;
        }
        ctx.dispatch_typed_action(TabAction::Select(id));
    })
    .on_middle_click(move |_, ctx, _| ctx.dispatch_typed_action(TabAction::Close(id)))
    .finish();

    Expanded::new(
        1.,
        ConstrainedBox::new(element)
            .with_max_width(TAB_MAX_WIDTH)
            .finish(),
    )
    .finish()
}

/// The agent's status, as the one coloured thing on an inactive tab.
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
fn label(title: String, ui: FamilyId, is_active: bool) -> Box<dyn Element> {
    let (color, weight) = if is_active {
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
fn close_slot(id: TabId, state: MouseStateHandle, visible: bool, ui: FamilyId) -> Box<dyn Element> {
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
        .on_click(move |_, ctx, _| ctx.dispatch_typed_action(TabAction::Close(id)))
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
