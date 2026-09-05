//! One row of the panel, in either density.
//!
//! Warp switches densities with two lines at `vertical_tabs.rs:2304-2308`:
//! a `match` over the setting picking between `render_compact_pane_row` and
//! `render_pane_row`, two whole functions with the same signature that both
//! end in the same `render_pane_row_element`. That split is copied here rather
//! than collapsed into one renderer taking a flag, because the divergence is
//! real — a different number of lines, at different sizes, with different gaps,
//! and chips in one and not the other — and a single function threading a
//! `Density` through six decisions is how a row ends up looking like a taller
//! version of the other one.
//!
//! # What is the same in both, and is easy to get wrong
//!
//! The padding is `8` on all four sides in **both**. The leading icon is `24`
//! in **both**. `COMPACT_ICON_SIZE = 16.` exists in Warp and is a trap: it is
//! used only for the density segment's glyph inside the options popup
//! (`vertical_tabs.rs:6400`), never in a row. Corner radius, border, hover and
//! selection are identical too. What changes is the text column, and only the
//! text column.
//!
//! # Two places this row is not Warp's, on purpose
//!
//! **`Expanded`'s description line is always drawn**, with an empty string
//! when the session has nothing to put there. Warp's terminal rows always have
//! both a command and a working directory; a Crook session whose directory was
//! deleted out from under it does not, and letting the line disappear would
//! make that row 14px shorter than its neighbours.
//!
//! **The mark at the head of the row is not this file's any more.** It is two
//! slots — `tab.row.mark` and the badge on its corner — declared by
//! `crook/tabs`, which draws the status disc when nothing has taken them. See
//! [`crate::plugins::tabs`] for why the disc is what the host draws rather
//! than a contribution competing with a plugin's.
//!
//! **There is a close button in the trailing edge**, in a slot reserved on
//! every row in every state. Warp closes a tab from a floating action belt
//! that overhangs the tab's top-right corner — an overlay — and a row already
//! spends its one overlay on the hover detail card. A reserved slot costs 16px
//! of a 248px column and keeps the panel usable with a mouse; the alternative
//! was a layout in which the only way to close a tab is a keystroke.

use crookui_core::elements::MouseStateHandle;
use crookui_core::fonts::{FamilyId, Weight};
use crookui_core::prelude::*;

use crate::plugins::tabs::TabRow;
use crate::settings::{Density, Granularity, TabOptions};
use crate::tab::{PaneId, TabAction, TabId};
use crate::theme::theme;

use super::super::action::{WorkspaceAction, WorktreeAction};
use super::super::row_content::{
    Chips, DetailSection, PANEL_PATH_CHARS, RowFacts, detail_card, detail_panes, metadata_line,
};
use super::super::view::Workspace;
use super::super::{CLOSE_BUTTON_SIZE, CLOSE_ICON_SIZE};

/// Warp's `ICON_WITH_STATUS_GAP`, between the icon and the text column.
const ICON_GAP: f32 = 8.;

/// Warp's `ROW_CORNER_RADIUS`.
const ROW_RADIUS: f32 = 4.;

/// Warp's row padding: `Padding::uniform(8.)`, in both densities.
const ROW_PADDING: f32 = 8.;

/// The title line, in both densities.
const TITLE_SIZE: f32 = 12.;

/// A `Compact` row's second line.
const SUBTITLE_SIZE: f32 = 10.;

/// An `Expanded` row's second line.
///
/// Twelve, not ten. Warp's `render_text_line` hardcodes `12.` and only line
/// 3's left text, the `Compact` subtitle and the group header are 10pt.
/// Rendering this at 10 makes `Expanded` look like a taller `Compact` instead
/// of a different row.
const DESCRIPTION_SIZE: f32 = 12.;

/// The gap between a `Compact` row's two lines.
const COMPACT_LINE_GAP: f32 = 1.;

/// The gap above each line of an `Expanded` row after the first.
const EXPANDED_LINE_GAP: f32 = 2.;

/// The gap between a panel row and the detail card it opens.
///
/// Warp's `DETAIL_SIDECAR_HORIZONTAL_GAP`. The card opens on the side away
/// from the panel, which for a panel on the left is the right.
const CARD_GAP: f32 = 12.;

/// One row: the pane it stands for, drawn at the density the options ask for.
pub(super) fn render(
    workspace: &Workspace,
    tab: TabId,
    pane: PaneId,
    app: &AppContext,
) -> Box<dyn Element> {
    let strip = workspace.tabs();
    let options = workspace.options();
    let ui = workspace.fonts().ui;

    let (Some(tab_data), Some(interaction)) = (strip.get(tab), workspace.interaction(pane)) else {
        // Unreachable: the ids came from `rows`, and `Workspace::apply` is the
        // only thing that can open a pane and always adds its mouse state.
        log::error!("pane {pane:?} has no tab or no interaction state and was skipped");
        return Empty::new().finish();
    };
    let Some(pane_data) = tab_data.panes().get(pane) else {
        log::error!("pane {pane:?} is not in the tab `rows` paired it with");
        return Empty::new().finish();
    };

    let session = pane_data.session();
    let git = workspace.git_facts(session, app);
    let status = pane_data.status();
    let home = workspace.home();

    // What the row's own right press does, and whether the menu it opens is
    // up. About the tab rather than about the row that draws it.
    let is_the_tabs_row = tab_data.panes().focused_id() == pane;
    let menu_is_open = is_the_tabs_row && workspace.tab_menu().tab == Some(tab);
    let opens_menu = is_the_tabs_row && git.is_some_and(|facts| facts.branch.is_some());

    // A conjunction, and that is the whole of what `Panes` granularity is for:
    // the active tab's container is lifted while only its focused pane's row
    // is selected, so "which tab" and "which pane" stay two separate signals.
    // Painting every row of the active tab as selected loses the distinction.
    let is_selected = strip.is_active(tab) && tab_data.panes().is_focused(pane);

    // Under `Tabs` the row stands for its whole tab, so its close button
    // closes the tab. Under `Panes` it closes the pane it names, and the tab
    // only goes with it if that was the last one.
    let close_action = match options.granularity {
        Granularity::Panes => TabAction::ClosePane(pane),
        Granularity::Tabs => TabAction::Close(tab),
    };

    let facts = RowFacts::resolve(session, git, home, PANEL_PATH_CHARS);
    let chips = match options.density {
        // Warp's `render_compact_pane_row` never calls
        // `render_terminal_right_badges`, which is exactly why the menu hides
        // the two "Show" toggles in this density.
        Density::Expanded => Chips::resolve(session, git, options),
        Density::Compact => Chips::default(),
    };
    let body = match options.density {
        Density::Compact => compact_column(&facts, options, ui),
        Density::Expanded => expanded_column(&facts, &chips, options, ui),
    };

    // Made once and lent to whatever is drawing the row's mark, which is
    // this build's `crook/tabs` unless a plugin has taken the slot. Every
    // field of it is something this function already had.
    let row = TabRow {
        tab,
        pane,
        title: session.display_title(),
        active: strip.is_active(tab),
        status,
        directory: session.working_directory.as_deref(),
        git,
    };

    let close_state = interaction.close.clone();
    let guard = interaction.close.clone();

    let element = Hoverable::new(interaction.chip.clone(), move |state| {
        let hovered = state.is_hovered();
        let show_close = hovered || is_selected;
        if !show_close {
            // A row that stopped drawing its close button must also stop
            // believing the pointer is on it, or the guard in the click
            // handler below would swallow this row's next click forever.
            close_state.lock().reset_interaction_state();
        }

        row_shell(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_spacing(ICON_GAP)
                // Warp's rule: the icon reads as a heading beside a multi-line
                // column and as a bullet beside a single line.
                .with_cross_axis_alignment(if body.is_multiline {
                    CrossAxisAlignment::Start
                } else {
                    CrossAxisAlignment::Center
                })
                .with_child(crate::plugins::tabs::mark(workspace, &row, app))
                .with_child(Expanded::new(1., body.column).finish())
                .with_child(close_slot(close_action, close_state.clone(), show_close))
                .finish(),
            is_selected,
            hovered,
        )
    })
    .on_click(move |_, ctx, _| {
        // The close button is a descendant, so a release over it hit-tests
        // true for both. Focusing a pane that has just been closed is a no-op,
        // but activating a row as it disappears still flickers.
        if guard.lock().is_hovered() {
            return;
        }
        ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::FocusPane(pane)));
    })
    .on_middle_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Tab(close_action));
    })
    // The secondary button opens the menu. See the strip's own row, which is
    // the same rule.
    .on_right_click(move |_, ctx, _| {
        if !opens_menu {
            return;
        }
        ctx.dispatch_typed_action(WorkspaceAction::Worktree(WorktreeAction::OpenMenu(tab)));
    })
    .on_hover(move |entered, _, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::HoverRow { pane, entered });
    })
    .finish();

    if menu_is_open {
        // Below the row and inside the panel. Beside it — where the card goes
        // — would put a 260px menu over the body, which is the same argument
        // that right-aligns the list's own options menu in this column.
        let mut stack = Stack::new().with_child(element);
        stack.add_anchored_overlay_child(
            Dismiss::new(super::super::tab_menu::render(workspace))
                .modal()
                .on_dismiss(|ctx, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::Worktree(WorktreeAction::CloseMenu));
                })
                .finish(),
            AnchorTo::below(vec2f(0., 4.)),
        );
        return stack.finish();
    }

    if !workspace.shows_details_for(pane) {
        return element;
    }

    let sections: Vec<DetailSection<'_>> = detail_panes(tab_data, pane, options.granularity)
        .into_iter()
        .map(|pane| DetailSection {
            session: pane.session(),
            facts: workspace.git_facts(pane.session(), app),
            status: pane.status(),
        })
        .collect();

    let mut stack = Stack::new().with_child(element);
    // Beside the row rather than below it, and on the side away from the
    // panel: a card hung under a panel row would cover the rows the pointer is
    // about to travel to. Warp opens its sidecar on the far side for the same
    // reason. The only overlay on this stack — a row hit-tests against the
    // topmost overlay that existed when it painted, so a second would make the
    // row unclickable.
    stack.add_anchored_overlay_child(
        detail_card(&sections, home, ui),
        AnchorTo {
            parent: Corner::TopRight,
            child: Corner::TopLeft,
            offset: vec2f(CARD_GAP, 0.),
            keep_on_screen: true,
            // A 320px card beside a 248px panel needs a window 559 wide to
            // fit, and one can be dragged down to 480. Without this the
            // on-screen slide lands it on top of the rows it is anchored to:
            // the row under it stops hit-testing as hovered, so the card is
            // torn down and re-armed frame after frame, and the close button
            // beneath it cannot be reached at all. Warp narrows its sidecar to
            // 240 and then lets it clip off the window edge rather than move
            // back over the panel; Crook has no way to measure the room at
            // build time, so it keeps the width and takes the clip.
            keep_clear_of_parent: true,
        },
    );
    stack.finish()
}

/// A row's text column, and whether the icon beside it has more than one line
/// to align against.
struct RowBody {
    column: Box<dyn Element>,
    is_multiline: bool,
}

/// `Compact`: a title, and a subtitle when the chosen fact has a value.
///
/// Warp's `render_compact_pane_row`. The column's spacing is 1, and the row
/// really does lose 12px of height when the subtitle is absent — the icon is
/// the floor, so a subtitle-less row is exactly as tall as the icon plus the
/// padding.
fn compact_column(facts: &RowFacts, options: TabOptions, ui: FamilyId) -> RowBody {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(COMPACT_LINE_GAP)
        .with_child(facts.title(options.primary_info).render(
            TITLE_SIZE,
            theme().text_primary,
            Weight::Normal,
            ui,
        ));

    // Resolved here as well as in the menu: a menu that ticked the stored
    // value would put a check beside an option this line is overriding.
    let subtitle = facts.subtitle(options);
    let is_multiline = subtitle.is_some();
    if let Some(subtitle) = subtitle {
        column.add_child(subtitle.render(SUBTITLE_SIZE, theme().text_muted, Weight::Normal, ui));
    }

    RowBody {
        column: column.finish(),
        is_multiline,
    }
}

/// `Expanded`: a title, a description, and a height-locked metadata line.
///
/// Warp's `render_pane_row` through `render_terminal_row_content`. The column
/// has no spacing of its own; the second and third children each carry a 2px
/// top margin, which is the same arithmetic said in the place Warp says it.
fn expanded_column(facts: &RowFacts, chips: &Chips, options: TabOptions, ui: FamilyId) -> RowBody {
    let description = facts
        .description(options.primary_info)
        // An empty line rather than no line: see the module docs. The row's
        // height must not depend on whether a session has a working directory.
        .map_or_else(
            || {
                Text::new("", ui, DESCRIPTION_SIZE)
                    .with_color(theme().text_muted)
                    .finish()
            },
            |line| line.render(DESCRIPTION_SIZE, theme().text_muted, Weight::Normal, ui),
        );

    let column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        // Stretch, so the metadata line below gets the column's full width to
        // push its chips to the far end of.
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(facts.title(options.primary_info).render(
            TITLE_SIZE,
            theme().text_primary,
            Weight::Normal,
            ui,
        ))
        .with_child(
            Container::new(description)
                .with_margin_top(EXPANDED_LINE_GAP)
                .finish(),
        )
        .with_child(
            Container::new(metadata_line(
                facts.metadata(options.primary_info),
                chips,
                ui,
            ))
            .with_margin_top(EXPANDED_LINE_GAP)
            .finish(),
        )
        .finish();

    RowBody {
        column,
        is_multiline: true,
    }
}

/// The box every row is drawn in, at either density.
///
/// Warp's `render_pane_row_element(props, Padding::uniform(8.), true, ..)`,
/// which both of its density functions call with exactly those arguments.
///
/// The border is present in every state and merely changes colour, because
/// Crook's border participates in layout: drawing it only when selected would
/// make the selected row two pixels taller than its neighbours and move the
/// whole list every time the selection changed. That costs the row 2px against
/// Warp's arithmetic and costs it nothing against itself.
fn row_shell(content: Box<dyn Element>, is_selected: bool, is_hovered: bool) -> Box<dyn Element> {
    let background = if is_selected {
        theme().overlay_2
    } else if is_hovered {
        theme().overlay_1
    } else {
        Color::TRANSPARENT
    };

    Container::new(
        // A 63px row's chips can otherwise overhang into the next row's hit
        // area, where they would be drawn *and* clickable.
        Clipped::new(content).finish(),
    )
    .with_uniform_padding(ROW_PADDING)
    .with_background_color(background)
    .with_border(Border::all(1.).with_border_color(if is_selected {
        theme().overlay_3
    } else {
        Color::TRANSPARENT
    }))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(ROW_RADIUS)))
    .finish()
}

/// A fixed square, holding the close button or holding nothing.
///
/// A group's heading takes the same one: it is the same gesture on the same
/// kind of thing, and a second close button drawn a second way is how the two
/// end up different sizes.
pub(super) fn close_slot(
    action: TabAction,
    state: MouseStateHandle,
    visible: bool,
) -> Box<dyn Element> {
    let inner: Box<dyn Element> = if visible {
        Hoverable::new(state, move |state| {
            let hovered = state.is_hovered();
            Container::new(
                Align::new(
                    Icon::new(Lucide::X, CLOSE_ICON_SIZE)
                        .with_color(if hovered {
                            theme().text_primary
                        } else {
                            theme().text_muted
                        })
                        .finish(),
                )
                .finish(),
            )
            .with_background_color(if hovered {
                theme().overlay_3
            } else {
                Color::TRANSPARENT
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish()
        })
        .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Tab(action)))
        .finish()
    } else {
        Empty::new().finish()
    };

    ConstrainedBox::new(inner)
        .with_width(CLOSE_BUTTON_SIZE)
        .with_height(CLOSE_BUTTON_SIZE)
        .finish()
}
