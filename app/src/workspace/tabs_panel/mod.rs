//! The vertical tabs panel: a 248px column down the left of the window.
//!
//! Crook's default layout, and the one place its defaults deliberately differ
//! from Warp's — [`Layout`](crate::settings::Layout) says why. It is the other
//! half of a mutually exclusive pair with [`tab_bar`](super::tab_bar): when
//! this renders, the header carries no tab items at all, which is exactly what
//! Warp's early return at `view.rs:20916` does. There is no state in which
//! both a strip and a panel show tabs.
//!
//! Top to bottom: a control bar, the search box, the list, and the row of
//! buttons that says what the list is. Warp keeps its search field *in* that
//! bar; this one is a row of its own, which is Telegram's arrangement and is
//! what the bar being the window's title bar forces — see [`search`].
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
//! hard ceiling — in the 1024x640 window Crook opens, eight tabs in the
//! default combination and six at the other extreme, with every tab past that
//! drawn, clipped away, and unclickable. (Eight and six rather than the nine
//! and seven they were before the row of section buttons took a strip off the
//! bottom of the list.)
//!
//! [`Scrollable`] keeps the half of [`Clipped`] that mattered — a row that
//! overflows is neither painted over the body nor hit-tested there — and adds
//! the wheel.
//!
//! It also scrolls to the row a selection lands on, which is what Warp does
//! and what `cmd-alt-left/right` — `ctrl-pageup/pagedown` off macOS — needs to
//! be usable at all. A `Scrollable` can be told to scroll to an offset and
//! cannot be asked where a particular child is, and the rows are not a fixed
//! height anyway: the density changes them, the granularity changes how many
//! there are, and a group header sits above each tab's. So the rows write down
//! where they were painted, in [`geometry`], and the workspace reads the one
//! it needs. The offsets are the *last* frame's, which is right: rows do not
//! move when a selection does.

use crookui_core::elements::Padding;
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crookui_core::elements::MouseStateHandle;

use crate::settings::Granularity;
use crate::tab::{GroupId, PaneId, Tab, TabAction, TabId};
use crate::theme::theme;

use super::action::WorkspaceAction;
use super::controls;
use super::settings_page::search::Query;
use super::title_bar;
use super::view::Workspace;

pub(crate) mod drag;
pub(super) mod geometry;
mod row;
pub(crate) mod search;

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

/// Warp's `TAB_GROUP_MEMBER_INDENT`: how far a group's members sit in from its
/// heading.
///
/// The whole of what says they are its members. Warp also has a colour per
/// group and Crook does not, so this carries the meaning on its own — which is
/// why it is an indent a person can see rather than the four pixels that would
/// read as a rounding error.
const MEMBER_INDENT: f32 = 12.;

/// The radius a group's card takes in `Tabs` granularity, where a tab is a
/// card too. Warp's `ROW_CORNER_RADIUS`, which is the rows' own.
const GROUP_RADIUS: f32 = 4.;

/// The heading's own name line, which is a row's title size rather than the
/// ten of the line under it.
const HEADING_SIZE: f32 = 12.;

/// Warp's `TAB_GROUP_ICON_SIZE`: the chevron that says which way the group
/// folds.
const HEADING_ICON_SIZE: f32 = 16.;

/// The square it is centred in: a row's leading icon, so the heading's name
/// starts somewhere near the column its members' names are in. Warp's
/// `VERTICAL_TABS_ICON_SIZE`.
const HEADING_ICON_SLOT: f32 = 24.;

/// The gap between that slot and the name. Warp's `ICON_WITH_STATUS_GAP`,
/// which is the gap on a row.
const HEADING_ICON_GAP: f32 = 8.;

/// How thick the line that says where a drop will land is, and how much room
/// it takes between two rows.
///
/// Warp's `GROUP_INSERTION_INDICATOR_HEIGHT` and
/// `GROUP_INSERTION_TARGET_HEIGHT`. The slot is taller than the line so that
/// the rows part around it rather than the line being drawn over one of them:
/// a list that does not move tells you nothing about where the gap is.
const INSERTION_LINE_HEIGHT: f32 = 2.;
/// See [`INSERTION_LINE_HEIGHT`].
const INSERTION_SLOT_HEIGHT: f32 = 8.;

/// The padding around the empty state, and the size it is set in.
const EMPTY_STATE_PADDING: f32 = 12.;

/// What the sidebar's first button says and draws.
///
/// The window's own section, and the only one no plugin contributes: the tabs
/// are what Crook is, not a thing that was added to it.
const AGENTS_SECTION_TITLE: &str = "Agents";
/// See [`AGENTS_SECTION_TITLE`].
const AGENTS_ICON: Lucide = Lucide::LayoutGrid;

/// The inset around the row of section buttons.
const SECTION_BAR_PADDING: f32 = 8.;

/// The icon in one of them, and the name under it.
const SECTION_ICON_SIZE: f32 = 18.;
/// See [`SECTION_ICON_SIZE`].
const SECTION_LABEL_SIZE: f32 = 10.;

/// The whole panel: the control bar, whatever the chosen section puts in it,
/// and the row of buttons that chooses.
///
/// `body` is the section's own — the tab list when the tabs are showing, and
/// the section's sidebar otherwise. It is handed in rather than built here
/// because the window builds it and its other half together: see
/// [`SidebarSection`](crate::plugin::SidebarSection).
pub(super) fn render(workspace: &Workspace, body: Box<dyn Element>) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(control_bar(workspace));

    // Only over the tabs, because it filters the tabs. Every other section
    // brings its own search where it needs one — the settings rail has one at
    // the top of the very same column — and a second box above it, filtering a
    // list that is not on screen, would be two boxes and one meaning.
    if workspace.panel_search_is_showing() {
        column.add_child(search::render(workspace));
    }

    column.add_child(Expanded::new(1., body).finish());
    column.add_child(sections(workspace));

    ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme().surface)
            .with_border(Border::right(1.).with_border_color(theme().border))
            .finish(),
    )
    .with_width(PANEL_WIDTH)
    .finish()
}

/// The tab list, scrolling inside whatever the bars left it.
pub(super) fn tab_list(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    Scrollable::new(
        workspace.panel_scroll(),
        // Inside the scrollable and outside every row, which is what makes a
        // row's offset measurable from the content rather than from the
        // window.
        geometry::Content::new(
            workspace.panel_rows(),
            // Outside every row and inside the scrollable, for the reason the
            // content wrapper is: the rows record their boxes against it.
            drag::Frame::new(workspace.panel_drag(), list(workspace, app)).finish(),
        )
        .finish(),
    )
    .with_scrollbar(theme().overlay_3)
    .finish()
}

/// The row of buttons at the foot of the panel.
///
/// Telegram's shape, and the reason is Telegram's: one column, and what is in
/// it is chosen by a short row of buttons at the bottom rather than by
/// navigating away from it. They are not tabs and are not drawn as tabs —
/// there is no strip, no separators, no bar and no rule above them — because
/// what they switch is the whole window and a tab does not do that. The panel
/// already ends where the window does; a line saying so again only cuts the
/// column in two.
///
/// The first is the window's own and every other comes from a plugin, in the
/// order the sections were contributed. A build with the settings plugin
/// switched off has one button, which is correct: there is nowhere else to go.
fn sections(workspace: &Workspace) -> Box<dyn Element> {
    let showing = workspace.showing_section();
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_alignment(MainAxisAlignment::SpaceEvenly)
        .with_child(button(
            workspace,
            None,
            AGENTS_SECTION_TITLE,
            AGENTS_ICON,
            showing.is_none(),
        ));

    for (id, title, icon) in workspace.host().sidebar_sections() {
        row.add_child(button(
            workspace,
            Some(id),
            &title,
            icon,
            showing == Some(id),
        ));
    }

    Container::new(row.finish())
        .with_padding(Padding {
            top: SECTION_BAR_PADDING,
            bottom: SECTION_BAR_PADDING,
            left: SECTION_BAR_PADDING,
            right: SECTION_BAR_PADDING,
        })
        .finish()
}

/// One of those buttons: an icon with its name under it.
fn button(
    workspace: &Workspace,
    section: Option<crate::plugin::SectionId>,
    title: &str,
    icon: Lucide,
    chosen: bool,
) -> Box<dyn Element> {
    let key = match section {
        Some(id) => workspace
            .host()
            .sidebar_section_key(id)
            .unwrap_or_default()
            .to_owned(),
        None => "tabs".to_owned(),
    };
    let state = workspace.section_button(&key);
    let label = title.to_owned();

    Hoverable::new(state, move |mouse| {
        // Three states and only two colours: the chosen one is lit, and
        // hovering an unchosen one lifts it towards being lit. A background
        // as well would make this a row of tabs, which is the one thing it
        // must not read as.
        let color = if chosen {
            theme().accent
        } else if mouse.is_hovered() {
            theme().text_primary
        } else {
            theme().text_muted
        };

        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Icon::new(icon, SECTION_ICON_SIZE)
                        .with_color(color)
                        .finish(),
                )
                .with_child(
                    Container::new(
                        Text::new(label.clone(), workspace.fonts().ui, SECTION_LABEL_SIZE)
                            .with_color(color)
                            .finish(),
                    )
                    .with_margin_top(3.)
                    .finish(),
                )
                .finish(),
        )
        .with_horizontal_padding(10.)
        .with_vertical_padding(4.)
        .finish()
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::ShowSection(section)))
    .finish()
}

/// The bar across the top of the panel: the gear and the `+`, at its right
/// edge.
///
/// The flexible [`Empty`] in front of them is what puts them there, and it is
/// now the whole of the bar's left half: the search box that Warp keeps *in*
/// this bar is a row of its own underneath, because this one is also the
/// window's title bar and a field wide enough to type a path into would leave
/// nothing to pick the window up by. See [`search`].
///
/// In this layout the bar is also half of the window's title bar: it is the
/// top-left corner, so it is what the traffic lights sit on and what a person
/// picks that end of the window up by. Both follow from the corner rather than
/// from the panel, which is why the inset and the drag come from the same two
/// places the header's do.
fn control_bar(workspace: &Workspace) -> Box<dyn Element> {
    // The one place in the panel that can be under the window's own controls:
    // on a client-decorated macOS window the traffic lights are in this
    // corner, which is a fact about where the tabs are rather than about the
    // platform. `WindowControlInsets::split` is what makes that one decision.
    let inset = workspace.window_insets().panel_left;

    title_bar::draggable(
        workspace,
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(CONTROL_BAR_SPACING)
                .with_child(Expanded::new(1., Empty::new().finish()).finish())
                .with_child(controls::gear_button(workspace))
                .with_child(controls::new_tab_button(workspace))
                .finish(),
        )
        .with_padding(Padding {
            top: CONTROL_BAR_VERTICAL_PADDING,
            left: CONTROL_BAR_HORIZONTAL_PADDING + inset,
            bottom: CONTROL_BAR_VERTICAL_PADDING,
            right: CONTROL_BAR_HORIZONTAL_PADDING,
        })
        .finish(),
    )
}

/// The tabs the search left, gathered into blocks and wrapped in whichever
/// chrome the granularity asks for.
///
/// A block is a group with its members, or a tab that is in no group — see
/// [`TabStrip::blocks`](crate::tab::TabStrip::blocks). Walking those rather
/// than the tabs is what makes the heading and its members one element instead
/// of a run of rows that happen to be adjacent, and it is the only reason the
/// panel can draw a fold at all.
fn list(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let granularity = workspace.options().granularity;
    let query = workspace.panel_search().query();
    let rows = rows_by_tab(workspace, app, &query, granularity);

    if rows.is_empty() {
        // Reachable exactly one way, and it is the search: the strip refuses
        // to empty itself and closes the window instead, which is why the
        // empty state says what the filter did rather than what a panel with
        // no tabs would say. Warp's panel is empty in the same one case.
        return empty_state(workspace.fonts().ui, &query);
    }

    let blocks = blocks_of(workspace, rows);
    // Where a drop right now would land, which is the only thing a drag draws.
    let pending = workspace.panel_drag().pending();
    let last = blocks.len().saturating_sub(1);

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    for (index, block) in blocks.into_iter().enumerate() {
        let is_last = index == last;
        if let Some(first) = block.tabs.first().map(|(tab, _)| *tab)
            && drag::line_above_block(pending, first)
        {
            column.add_child(insertion_line(false));
        }

        match block.group {
            Some(group) => {
                column.add_child(group_block(
                    workspace,
                    group,
                    &block.tabs,
                    is_last,
                    pending,
                    app,
                ));
            }
            None => {
                for (tab, panes) in &block.tabs {
                    column.add_child(tab_block(workspace, *tab, panes, None, is_last, app));
                }
            }
        }
    }

    if drag::line_at_end(pending) {
        column.add_child(insertion_line(false));
    }

    match granularity {
        // No spacing and no padding: `Panes` separates tabs with hairlines
        // rather than with gaps, and a gap here would show the panel's ground
        // through every separator.
        Granularity::Panes => column.finish(),
        Granularity::Tabs => Container::new(column.with_spacing(TABS_MODE_ITEM_SPACING).finish())
            .with_padding(Padding {
                top: 0.,
                left: GROUP_HORIZONTAL_PADDING,
                bottom: GROUP_HORIZONTAL_PADDING,
                right: GROUP_HORIZONTAL_PADDING,
            })
            .finish(),
    }
}

/// One block of the panel: a group with the rows that survived the search, or
/// a single ungrouped tab.
struct PanelBlock {
    group: Option<GroupId>,
    tabs: Vec<(TabId, Vec<PaneId>)>,
}

/// The surviving rows, gathered into the blocks the strip says they form.
///
/// The strip is asked rather than the rows re-walked, so the panel's idea of
/// what is grouped is the strip's idea and there is no second rule here to
/// disagree with it. A group whose every member the search filtered away
/// produces no block at all: a heading over nothing is a claim that there is
/// something under it.
fn blocks_of(workspace: &Workspace, rows: Vec<(TabId, Vec<PaneId>)>) -> Vec<PanelBlock> {
    workspace
        .tabs()
        .blocks()
        .into_iter()
        .filter_map(|block| {
            let tabs: Vec<(TabId, Vec<PaneId>)> = block
                .tabs
                .iter()
                .filter_map(|tab| rows.iter().find(|(id, _)| id == tab).cloned())
                .collect();
            (!tabs.is_empty()).then_some(PanelBlock {
                group: block.group,
                tabs,
            })
        })
        .collect()
}

/// One tab, at whichever granularity, as a thing that can be picked up.
///
/// The two granularities draw a tab differently enough to be two functions —
/// see [`panes_tab`] and [`tabs_tab`] — and they are picked up identically,
/// which is why the grip goes on out here.
fn tab_block(
    workspace: &Workspace,
    tab: TabId,
    panes: &[PaneId],
    group: Option<GroupId>,
    is_last: bool,
    app: &AppContext,
) -> Box<dyn Element> {
    let Some(chrome) = workspace.tab_chrome(tab) else {
        log::error!("tab {tab:?} has no interaction state and was skipped");
        return Empty::new().finish();
    };

    let element = match workspace.options().granularity {
        Granularity::Panes => panes_tab(
            workspace,
            tab,
            panes,
            group.is_none() && is_last,
            group.is_some(),
            app,
        ),
        Granularity::Tabs => tabs_tab(workspace, tab, panes, app),
    };

    drag::Handle::new(
        drag::Grip::Tab { tab, group },
        workspace.panel_drag(),
        chrome.container.clone(),
        element,
    )
    .finish()
}

/// A group: its heading, and its members under it unless it is folded away.
///
/// The container is Warp's `render_grouped_tab_container`, with the members
/// indented past it so that "these belong together" is said by the indent
/// rather than by a colour. A member skips its own outer chrome — in `Panes`
/// the hairlines that separate tabs — because the group is already providing
/// it; that is Warp's `uses_outer_group_container = !in_tab_group && …`.
fn group_block(
    workspace: &Workspace,
    group: GroupId,
    members: &[(TabId, Vec<PaneId>)],
    is_last: bool,
    pending: Option<TabAction>,
    app: &AppContext,
) -> Box<dyn Element> {
    let Some(data) = workspace.tabs().group(group) else {
        log::error!("group {group:?} came from `blocks` and is not in the strip");
        return Empty::new().finish();
    };
    let Some(chrome) = workspace.group_chrome(group) else {
        log::error!("group {group:?} has no interaction state and was skipped");
        return Empty::new().finish();
    };
    let granularity = workspace.options().granularity;
    let collapsed = data.is_collapsed();
    let holds_the_active_tab = members
        .iter()
        .any(|(tab, _)| workspace.tabs().is_active(*tab));

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(heading(
            workspace,
            group,
            data.name(),
            members.len(),
            collapsed,
        ));

    if !collapsed {
        let mut rows = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(match granularity {
                Granularity::Panes => 0.,
                Granularity::Tabs => TABS_MODE_ITEM_SPACING,
            });

        for (tab, panes) in members {
            if drag::line_above_member(pending, group, *tab) {
                rows.add_child(insertion_line(true));
            }
            rows.add_child(tab_block(workspace, *tab, panes, Some(group), false, app));
        }
        if drag::line_ends_group(pending, group) {
            rows.add_child(insertion_line(true));
        }

        column.add_child(
            Container::new(rows.finish())
                .with_padding(Padding {
                    top: 0.,
                    left: MEMBER_INDENT,
                    bottom: GROUP_BODY_BOTTOM_PADDING,
                    right: 0.,
                })
                .finish(),
        );
    }

    let column = column.finish();
    let heading_state = chrome.container.clone();

    Hoverable::new(heading_state, move |mouse| {
        let lit = holds_the_active_tab || mouse.is_hovered();
        let container = Container::new(column).with_background_color(if lit {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        });

        match granularity {
            // The chrome a tab wears in this mode, one level up: hairlines
            // rather than a card, so the list still has no gaps in it.
            Granularity::Panes => container
                .with_border(
                    Border::new(1.)
                        .with_sides(true, false, is_last, false)
                        .with_border_color(theme().overlay_1),
                )
                .finish(),
            Granularity::Tabs => container
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(GROUP_RADIUS)))
                .finish(),
        }
    })
    .finish()
}

/// A group's heading: a chevron saying which way it folds, its name, and how
/// many tabs are under it.
///
/// Clicking it folds the group away and back — Warp's, and the gesture every
/// disclosure in every sidebar has. The close button beside it closes every
/// tab in the group, which is the only way to put a group down in one gesture
/// once the tabs inside it are the work rather than the panes.
fn heading(
    workspace: &Workspace,
    group: GroupId,
    name: &str,
    members: usize,
    collapsed: bool,
) -> Box<dyn Element> {
    let Some(chrome) = workspace.group_chrome(group) else {
        return Empty::new().finish();
    };
    let ui = workspace.fonts().ui;
    let name = name.to_owned();
    let count = if members == 1 {
        "1 tab".to_owned()
    } else {
        format!("{members} tabs")
    };
    let close = chrome.close.clone();
    let guard = chrome.close.clone();
    let state = chrome.heading.clone();

    let element = Hoverable::new(state.clone(), move |mouse| {
        let hovered = mouse.is_hovered();
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(HEADING_ICON_GAP)
                .with_child(
                    // Centred in a slot the size of a row's leading icon, which
                    // is Warp's line for Warp's reason: it is what brings the
                    // name on the heading into something like the column its
                    // members' names are in.
                    ConstrainedBox::new(
                        Align::new(
                            Icon::new(
                                if collapsed {
                                    Lucide::ChevronRight
                                } else {
                                    Lucide::ChevronDown
                                },
                                HEADING_ICON_SIZE,
                            )
                            .with_color(if hovered {
                                theme().text_primary
                            } else {
                                theme().text_muted
                            })
                            .finish(),
                        )
                        .finish(),
                    )
                    .with_width(HEADING_ICON_SLOT)
                    .with_height(HEADING_ICON_SLOT)
                    .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.,
                        Flex::column()
                            .with_main_axis_size(MainAxisSize::Min)
                            .with_cross_axis_alignment(CrossAxisAlignment::Start)
                            .with_child(
                                Text::new(name.clone(), ui, HEADING_SIZE)
                                    .with_color(theme().text_primary)
                                    .finish(),
                            )
                            .with_child(
                                Text::new(count.clone(), ui, GROUP_HEADER_SIZE)
                                    .with_color(theme().text_muted)
                                    .finish(),
                            )
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(row::close_slot(
                    TabAction::CloseGroup(group),
                    close.clone(),
                    hovered,
                ))
                .finish(),
        )
        .with_horizontal_padding(GROUP_HORIZONTAL_PADDING)
        .with_vertical_padding(GROUP_HEADER_VERTICAL_PADDING)
        .finish()
    })
    .on_click(move |_, ctx, _| {
        // The close button is a descendant, so a release over it hit-tests
        // true for both — the row's own guard, for the same reason.
        if guard.lock().is_hovered() {
            return;
        }
        ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::ToggleGroup(group)));
    })
    .finish();

    drag::Handle::new(
        drag::Grip::Heading { group, collapsed },
        workspace.panel_drag(),
        state,
        element,
    )
    .finish()
}

/// The line that says where a drop will land.
///
/// Two pixels of the accent colour in the gap between two rows, inset to the
/// indentation of whatever it is joining — so a drop *into* a group reads
/// differently from a drop between blocks, which is the one thing the line has
/// to be able to say. Warp draws exactly this, at exactly this inset.
fn insertion_line(in_group: bool) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(
            Container::new(Empty::new().finish())
                .with_background_color(theme().accent)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                    INSERTION_LINE_HEIGHT / 2.,
                )))
                .finish(),
        )
        .with_padding(Padding {
            top: (INSERTION_SLOT_HEIGHT - INSERTION_LINE_HEIGHT) / 2.,
            left: if in_group {
                0.
            } else {
                GROUP_HORIZONTAL_PADDING
            },
            bottom: (INSERTION_SLOT_HEIGHT - INSERTION_LINE_HEIGHT) / 2.,
            right: GROUP_HORIZONTAL_PADDING,
        })
        .finish(),
    )
    .with_height(INSERTION_SLOT_HEIGHT)
    .finish()
}

/// Every row the panel draws, gathered under the tab that owns it.
///
/// Built from [`TabStrip::rows`](crate::tab::TabStrip::rows) rather than by
/// walking the tabs again, because that function *is* the granularity rule —
/// a second walk here would be a second copy of it, free to disagree. The
/// filter is applied to what it yields, so a query narrows the rows the
/// granularity chose rather than choosing rows of its own.
fn rows_by_tab(
    workspace: &Workspace,
    app: &AppContext,
    query: &Query,
    granularity: Granularity,
) -> Vec<(TabId, Vec<PaneId>)> {
    let mut grouped: Vec<(TabId, Vec<PaneId>)> = Vec::new();

    for (tab, pane) in workspace.tabs().rows(granularity) {
        // The one thing the search does, and the only place it does it: a row
        // the query does not answer for is not built, so nothing downstream —
        // the chrome, the group header, the geometry a scroll reads — has to
        // know that a filter exists.
        if !search::keeps(workspace, app, query, granularity, tab, pane) {
            continue;
        }
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
    in_group: bool,
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
        rows.add_child(
            geometry::Tracked::new(
                *pane,
                workspace.panel_rows(),
                row::render(workspace, tab, *pane, app),
            )
            .finish(),
        );
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

    // A member of a group wears none of this: the group's own container is
    // already the box around it, and a second lift inside the first reads as
    // two nested cards. Warp's `uses_outer_group_container = !in_tab_group &&
    // …` is the same line.
    if in_group {
        return column;
    }

    Hoverable::new(chrome.container.clone(), move |mouse| {
        Container::new(column)
            .with_background_color(if is_active || mouse.is_hovered() {
                theme().overlay_1
            } else {
                Color::TRANSPARENT
            })
            .with_border(
                Border::new(1.)
                    .with_sides(true, false, is_last, false)
                    .with_border_color(theme().overlay_1),
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
        rows.add_child(
            geometry::Tracked::new(
                *pane,
                workspace.panel_rows(),
                row::render(workspace, tab, *pane, app),
            )
            .finish(),
        );
    }
    let rows = rows.finish();

    Hoverable::new(chrome.container.clone(), move |mouse| {
        Container::new(rows)
            .with_background_color(if is_active || mouse.is_hovered() {
                theme().overlay_1
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
                    theme().text_primary
                } else {
                    theme().text_muted
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
fn empty_state(ui: FamilyId, query: &Query) -> Box<dyn Element> {
    let line = if query.is_empty() {
        "No tabs open"
    } else {
        // The settings page's wording, for the box that behaves like the
        // settings page's box. A person who has emptied both lists in one
        // session should not have to read two sentences to learn the same
        // thing.
        "No tabs match your search."
    };

    Container::new(
        Text::new(line, ui, 12.)
            .with_color(theme().text_muted)
            .finish(),
    )
    .with_uniform_padding(EMPTY_STATE_PADDING)
    .finish()
}
