//! The vertical tabs panel: a 248px column down the left of the window.
//!
//! Crook's default layout, and the one place its defaults deliberately differ
//! from Warp's — [`Layout`](crate::settings::Layout) says why. It is the other
//! half of a mutually exclusive pair with [`tab_bar`](super::tab_bar): when
//! this renders, the header carries no tab items at all, which is exactly what
//! Warp's early return at `view.rs:20916` does. There is no state in which
//! both a strip and a panel show tabs.
//!
//! Top to bottom: a control bar, then the list. Warp's panel also has a search
//! field in that bar and a scrollbar down the list; Crook has neither a text
//! input nor a scrollable element, so the search slot is an empty flexible gap
//! that keeps the gear and the `+` where they belong, and the list is
//! [`Clipped`] with a ceiling written down below.
//!
//! # Granularity is two changes, not one
//!
//! `View as` decides *how many rows a tab produces* and *what chrome wraps
//! them*, and the two are unrelated. Both halves are real in Crook now that a
//! tab holds a [`PaneGroup`](crate::tab::PaneGroup): a four-way split shows
//! four rows in `Panes` and one in `Tabs`.
//!
//! The chrome inverts between them, and getting only the row count right
//! leaves the panel looking wrong in both modes:
//!
//! | | `Panes` | `Tabs` |
//! |---|---|---|
//! | tab container | 8px inset, hairline separators, no gaps | no inset, no border, 4px gaps |
//! | group header | when the tab holds more than one pane | never |
//! | list column | no padding, no spacing | `uniform(8).with_top(0)`, spacing 4 |
//! | hover card | the row's own pane | every pane of the tab, one section each |
//!
//! That last row is the one that is easy to leave out. `Tabs` drops a tab's
//! other panes from the list with no count and no expander, so the card is the
//! only place they appear — which is why the target follows the granularity
//! rather than the row.
//!
//! # A list longer than the panel
//!
//! It scrolls. That is one line here and it was not free: until
//! `crookui_core` grew a [`Scrollable`], this list was a [`Clipped`] with a
//! hard ceiling — in the 1024x640 window Crook opens, nine tabs in the default
//! combination and seven at the other extreme, with every tab past that drawn,
//! clipped away, and unclickable.
//!
//! [`Scrollable`] keeps the half of [`Clipped`] that mattered — a row that
//! overflows is neither painted over the body nor hit-tested there — and adds
//! the wheel. What it does not add is auto-scroll: selecting a tab with
//! `cmd/ctrl-shift-left/right` moves the selection whether or not the row is
//! on screen, and does not bring it into view. Warp scrolls to its selected
//! tab, which needs a scrollable that can be told "make this child visible",
//! and that means an element that knows where its children ended up. It is a
//! real gap and it is written down here rather than faked with a guess at the
//! row's offset.

use crookui_core::elements::Padding;
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crookui_core::elements::MouseStateHandle;

use crate::settings::Granularity;
use crate::tab::{PaneId, Tab, TabAction, TabId};
use crate::theme::THEME;

use super::action::WorkspaceAction;
use super::controls;
use super::view::Workspace;

mod row;

/// Warp's `PANEL_WIDTH`.
///
/// Warp's panel is resizable between `MIN_PANEL_WIDTH = 200` and half the
/// window; Crook has no `Resizable`, so this is fixed. Nothing about a row's
/// layout depends on the width being adjustable, so the day a drag bar arrives
/// this becomes the initial value and nothing else moves.
pub(super) const PANEL_WIDTH: f32 = 248.;

/// Warp's `CONTROL_BAR_VERTICAL_PADDING`.
const CONTROL_BAR_VERTICAL_PADDING: f32 = 4.;

/// The control bar's left and right padding.
const CONTROL_BAR_HORIZONTAL_PADDING: f32 = 8.;

/// Warp's `CONTROL_BAR_SPACING`, between the controls in that bar.
const CONTROL_BAR_SPACING: f32 = 4.;

/// Warp's `GROUP_HORIZONTAL_PADDING`, inset either side of a `Panes` tab.
const GROUP_HORIZONTAL_PADDING: f32 = 8.;

/// Warp's `GROUP_BODY_BOTTOM_PADDING`.
const GROUP_BODY_BOTTOM_PADDING: f32 = 8.;

/// Warp's `GROUP_HEADER_VERTICAL_PADDING`.
const GROUP_HEADER_VERTICAL_PADDING: f32 = 4.;

/// Warp's `GROUP_ITEM_SPACING`, between the rows inside one `Panes` tab.
const GROUP_ITEM_SPACING: f32 = 4.;

/// Warp's `TABS_MODE_ITEM_SPACING`, between tabs in `Tabs` granularity.
const TABS_MODE_ITEM_SPACING: f32 = 4.;

/// The group header's text size. Ten, like the `Compact` subtitle and the
/// metadata line, and unlike everything else in a row.
const GROUP_HEADER_SIZE: f32 = 10.;

/// The padding around the empty state, and the size it is set in.
const EMPTY_STATE_PADDING: f32 = 12.;

/// The whole panel: the control bar, then the list.
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(control_bar(workspace))
                // The list takes whatever the control bar left, and scrolls
                // inside it.
                .with_child(
                    Expanded::new(
                        1.,
                        Scrollable::new(workspace.panel_scroll(), list(workspace, app))
                            .with_scrollbar(THEME.overlay_3)
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
        )
        .with_background_color(THEME.surface)
        .with_border(Border::right(1.).with_border_color(THEME.border))
        .finish(),
    )
    .with_width(PANEL_WIDTH)
    .finish()
}

/// The bar across the top of the panel: a search slot, the gear, the `+`.
///
/// The search slot is an [`Empty`] that takes the surplus width, not a
/// placeholder for a control that is coming: Crook has no text input, and a
/// search field that cannot be typed into would be worse than a gap. What the
/// slot does earn today is the layout — the gear and the `+` sit against the
/// panel's right edge, where they will still be when something fills it.
fn control_bar(workspace: &Workspace) -> Box<dyn Element> {
    // The one place in the panel that can be under the window's own controls:
    // on a client-decorated macOS window the traffic lights are in this
    // corner, which is a fact about the layout rather than about the platform.
    // With a horizontal strip this reservation belongs to the header instead,
    // and `layout_insets` is what makes that one decision.
    let inset = workspace.window_insets().panel_left;

    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(CONTROL_BAR_SPACING)
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
            .with_child(controls::gear_button(
                workspace,
                AnchorTo {
                    // Right edges aligned, unlike the strip's. The gear sits
                    // near the right edge of a 248px column and the menu is
                    // 200 wide: hung from its left edge it would open across
                    // the body, and `keep_on_screen` would not pull it back
                    // because the window has plenty of room to its right.
                    parent: Corner::BottomRight,
                    child: Corner::TopRight,
                    offset: vec2f(0., 4.),
                    keep_on_screen: true,
                    keep_clear_of_parent: false,
                },
            ))
            .with_child(controls::new_tab_button(workspace))
            .finish(),
    )
    .with_padding(Padding {
        top: CONTROL_BAR_VERTICAL_PADDING,
        left: CONTROL_BAR_HORIZONTAL_PADDING + inset,
        bottom: CONTROL_BAR_VERTICAL_PADDING,
        right: CONTROL_BAR_HORIZONTAL_PADDING,
    })
    .finish()
}

/// The tabs, wrapped in whichever chrome the granularity asks for.
fn list(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let granularity = workspace.options().granularity;
    let rows = rows_by_tab(workspace, granularity);

    if rows.is_empty() {
        // Unreachable: the strip refuses to empty itself and closes the window
        // instead. Warp's panel can genuinely be empty — its search filters
        // live — so the state is drawn rather than asserted away, and it costs
        // one `Text`.
        return empty_state(workspace.fonts().ui);
    }

    let last = rows.len() - 1;
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    match granularity {
        Granularity::Panes => {
            for (index, (tab, panes)) in rows.into_iter().enumerate() {
                column.add_child(panes_tab(workspace, tab, &panes, index == last, app));
            }
            // No spacing and no padding: `Panes` separates tabs with hairlines
            // rather than with gaps, and a gap here would show the panel's
            // ground through every separator.
            column.finish()
        }
        Granularity::Tabs => {
            for (tab, panes) in rows {
                column.add_child(tabs_tab(workspace, tab, &panes, app));
            }

            Container::new(column.with_spacing(TABS_MODE_ITEM_SPACING).finish())
                .with_padding(Padding {
                    top: 0.,
                    left: GROUP_HORIZONTAL_PADDING,
                    bottom: GROUP_HORIZONTAL_PADDING,
                    right: GROUP_HORIZONTAL_PADDING,
                })
                .finish()
        }
    }
}

/// Every row the panel draws, gathered under the tab that owns it.
///
/// Built from [`TabStrip::rows`](crate::tab::TabStrip::rows) rather than by
/// walking the tabs again, because that function *is* the granularity rule —
/// a second walk here would be a second copy of it, free to disagree.
fn rows_by_tab(workspace: &Workspace, granularity: Granularity) -> Vec<(TabId, Vec<PaneId>)> {
    let mut grouped: Vec<(TabId, Vec<PaneId>)> = Vec::new();

    for (tab, pane) in workspace.tabs().rows(granularity) {
        match grouped.last_mut() {
            // `rows` walks the tabs in order, so a tab's rows are always
            // contiguous and this never has to search backwards.
            Some((last, panes)) if *last == tab => panes.push(pane),
            _ => grouped.push((tab, vec![pane])),
        }
    }

    grouped
}

/// One tab in `Panes` granularity: a container around all of its rows.
///
/// Warp's `uses_outer_group_container == true` branch. The hairline borders
/// are what separate tabs — a top border on every one and a bottom border on
/// the last — so the list has no gaps in it at all and a tab reads as a block
/// rather than as a card.
fn panes_tab(
    workspace: &Workspace,
    tab: TabId,
    panes: &[PaneId],
    is_last: bool,
    app: &AppContext,
) -> Box<dyn Element> {
    let Some(chrome) = workspace.tab_chrome(tab) else {
        log::error!("tab {tab:?} has no interaction state and was skipped");
        return Empty::new().finish();
    };
    let Some(tab_data) = workspace.tabs().get(tab) else {
        log::error!("tab {tab:?} came from `rows` and is not in the strip");
        return Empty::new().finish();
    };

    let is_active = workspace.tabs().is_active(tab);
    let header = shows_group_header(tab_data).then(|| tab_data.name().to_owned());
    let ui = workspace.fonts().ui;

    let mut rows = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(GROUP_ITEM_SPACING);
    for pane in panes {
        rows.add_child(row::render(workspace, tab, *pane, app));
    }

    let body = Container::new(rows.finish())
        .with_padding(Padding {
            // Only when there is no header: the header's own bottom padding
            // has already separated it from the first row.
            top: if header.is_some() {
                0.
            } else {
                GROUP_HORIZONTAL_PADDING
            },
            left: GROUP_HORIZONTAL_PADDING,
            bottom: GROUP_BODY_BOTTOM_PADDING,
            right: GROUP_HORIZONTAL_PADDING,
        })
        .finish();

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    if let Some(name) = header {
        column.add_child(group_header(name, tab, chrome.header.clone(), ui));
    }
    column.add_child(body);
    let column = column.finish();

    Hoverable::new(chrome.container.clone(), move |mouse| {
        Container::new(column)
            .with_background_color(if is_active || mouse.is_hovered() {
                THEME.overlay_1
            } else {
                Color::TRANSPARENT
            })
            .with_border(
                Border::new(1.)
                    .with_sides(true, false, is_last, false)
                    .with_border_color(THEME.overlay_1),
            )
            .finish()
    })
    // No click handler on purpose, and Warp's container has none either — it
    // takes right-clicks, middle-clicks and drags. One here would fire in
    // *addition* to the row's, because a `Hoverable` hands an event to its
    // child and then still claims it. Selecting the tab from its chrome is
    // [`group_header`]'s job.
    .finish()
}

/// One tab in `Tabs` granularity: a bare container around its single row.
///
/// Warp's `uses_outer_group_container == false` branch. No header, no inset,
/// no borders — the tab's visual footprint is the row's footprint, and the
/// gaps between tabs come from the list column's spacing instead.
fn tabs_tab(
    workspace: &Workspace,
    tab: TabId,
    panes: &[PaneId],
    app: &AppContext,
) -> Box<dyn Element> {
    let Some(chrome) = workspace.tab_chrome(tab) else {
        log::error!("tab {tab:?} has no interaction state and was skipped");
        return Empty::new().finish();
    };

    let is_active = workspace.tabs().is_active(tab);
    let mut rows = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    // Always exactly one, because `rows` returns one pane per tab in this
    // granularity — the focused one, or the first visible one when the focused
    // pane has gone. The other panes are simply not listed: no count, no
    // expander, and the row silently re-targets as focus moves inside the tab.
    for pane in panes {
        rows.add_child(row::render(workspace, tab, *pane, app));
    }
    let rows = rows.finish();

    Hoverable::new(chrome.container.clone(), move |mouse| {
        Container::new(rows)
            .with_background_color(if is_active || mouse.is_hovered() {
                THEME.overlay_1
            } else {
                Color::TRANSPARENT
            })
            .finish()
    })
    .finish()
}

/// Whether a `Panes` tab names itself above its rows.
///
/// Warp's `should_show_tab_group_header(has_custom_title, is_being_renamed,
/// visible_pane_count) = has_custom_title || is_being_renamed ||
/// visible_pane_count > 1`. The first two clauses are permanently false in
/// Crook, which has no rename flow, so what is left is the third — the clause
/// Warp added to fix issue #9098, and the reason a single-pane tab shows no
/// redundant heading above its one row.
fn shows_group_header(tab: &Tab) -> bool {
    tab.panes().len() > 1
}

/// A `Panes` tab's name, above its rows.
///
/// Clicking it selects the tab, which is Warp's `render_group_header` →
/// `WorkspaceAction::ActivateTab`. It is the one part of a tab's chrome that
/// does something: the container around the rows takes right-clicks and
/// middle-clicks in Warp and no left-click at all, so the 8px inset and the
/// gaps between rows are inert there too, and a click handler on the container
/// here would fire *in addition to* the row's own — [`Hoverable`] hands an
/// event to its child and then still claims it — and re-target the tab on
/// every click meant for a row.
fn group_header(
    name: String,
    tab: TabId,
    state: MouseStateHandle,
    ui: FamilyId,
) -> Box<dyn Element> {
    Hoverable::new(state, move |mouse| {
        Container::new(
            Text::new(name, ui, GROUP_HEADER_SIZE)
                .with_color(if mouse.is_hovered() {
                    THEME.text_primary
                } else {
                    THEME.text_muted
                })
                .finish(),
        )
        .with_horizontal_padding(GROUP_HORIZONTAL_PADDING)
        .with_vertical_padding(GROUP_HEADER_VERTICAL_PADDING)
        .finish()
    })
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::Select(tab)));
    })
    .finish()
}

/// What the panel says when there is nothing in it.
fn empty_state(ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new("No tabs open", ui, 12.)
            .with_color(THEME.text_muted)
            .finish(),
    )
    .with_uniform_padding(EMPTY_STATE_PADDING)
    .finish()
}
