//! The horizontal tab strip: one rounded rect per row the strip says to draw.
//!
//! A row is a status dot, a text column, a close button and the box around
//! them, and four gestures: click to focus, right-click to open the tab's
//! worktree menu, middle-click to close, and the close button. Every handler
//! *emits* an action and none of them touches the strip, so no id captured
//! while rendering can be stale by the time the frame is over.
//!
//! A row stands for a *pane*, in both granularities. Under `Panes` a tab
//! contributes one row per pane it holds; under `Tabs` it contributes one, for
//! its focused pane. That is Warp's rule, and it is why exactly one row in the
//! whole bar is selected however many are drawn: `is_active_tab && is_focused`
//! (`app/src/workspace/view/vertical_tabs.rs:412`). Tinting every row of the
//! active tab and marking the focused pane separately would give a bar where
//! three rows look chosen.
//!
//! # This is the layout Crook does *not* open in
//!
//! [`Layout::Horizontal`](crate::settings::Layout::Horizontal) is the other
//! half of a mutually exclusive pair: when the panel is up this module renders
//! nothing at all, and when the strip is up
//! [`tabs_panel`](super::tabs_panel) does. Warp draws the same line the same
//! way — with vertical tabs on, `render_tab_bar` takes an early return that
//! emits no tab items whatsoever — and it matters because both halves read the
//! same options and would otherwise both be listening for the same clicks.
//!
//! # What the options menu changes here
//!
//! Everything below the box. `granularity` decides how many rows a tab
//! produces; `density` decides how many lines a row has; `primary_info` decides
//! what the first line says and, with it, what the second one is left to say;
//! `show_pr_link` and `show_diff_stats` decide which chips the metadata line
//! carries; `show_details_on_hover` decides whether hovering opens the card
//! that shows what the row could not fit.
//!
//! Nothing is cached. Every row re-reads the options and the git facts as it is
//! built, which is Warp's arrangement and is why a click on the menu is visible
//! in the very next frame with no invalidation graph in between. What is *not*
//! read here is git itself: [`GitModel`](crate::git_model::GitModel) gathers on
//! a background thread and a row does a map lookup, because Warp's habit of
//! deriving per-row state inside the renderer is a cost paid at frame rate.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::{FamilyId, Weight};
use crookui_core::prelude::*;

use crate::settings::{Density, Granularity, TabOptions};
use crate::tab::{AgentStatus, PaneId, TabAction, TabId};
use crate::theme::theme;

use super::action::{WorkspaceAction, WorktreeAction};
use super::row_content::{
    Chips, DetailSection, ROW_PATH_CHARS, RowFacts, RowLine, detail_card, detail_panes,
    metadata_line,
};
use super::view::Workspace;
use super::{
    CLOSE_BUTTON_SIZE, CLOSE_ICON_SIZE, GEAR_ICON, STATUS_DOT_SIZE, TAB_MAX_WIDTH, controls,
};

/// The whole strip, the button that opens another tab, and the options menu.
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_spacing(4.)
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::End);

    for (tab, pane) in workspace.tabs().rows(workspace.options().granularity) {
        row.add_child(render_row(workspace, tab, pane, app));
    }
    row.add_child(controls::new_tab_button(workspace));
    // Left edges aligned, straight down: the strip's gear is at the right end
    // of a full-width header, so `keep_on_screen` is what stops the menu from
    // running off the window and nothing else has to be said.
    row.add_child(controls::gear_button(
        workspace,
        AnchorTo::below(vec2f(0., 4.)),
    ));

    // Soaks up whatever the tabs did not need, so they stay packed against the
    // left edge instead of spreading themselves across the bar. A flex factor
    // below one deliberately: tabs get first call on the width and reach their
    // cap before the spacer takes anything.
    row.add_child(Expanded::new(0.5, Empty::new().finish()).finish());
    row.finish()
}

fn render_row(
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
        // Drawing nothing beats panicking a frame over it.
        log::error!("pane {pane:?} has no tab or no interaction state and was skipped");
        return Empty::new().finish();
    };
    let Some(pane_data) = tab_data.panes().get(pane) else {
        log::error!("pane {pane:?} is not in the tab `rows` paired it with");
        return Empty::new().finish();
    };

    // `None` for the settings pane, which is the one row in the strip that
    // stands for something other than an agent: no session to read, no
    // repository to look up, and a gear where the status dot goes.
    let session = pane_data.session();
    let git = session.and_then(|session| workspace.git_facts(session, app));
    let status = pane_data.status();
    // Resolved once on the workspace rather than here: it is the same answer
    // for every string every row prints, and asking for it is a syscall.
    let home = workspace.home();
    // The two facts the row's own right press needs. A menu opens on the row
    // it landed on, and only where there is a repository to have worktrees of
    // — the same promise the branch chip makes, which appears exactly when
    // there is a branch to name.
    // The tab's *focused* pane's row, in both conditions. In `Panes`
    // granularity one tab draws a row per pane, and a menu that answered to
    // "is this my tab" would be drawn once per row — two popups over each
    // other, each with its own modal underlay.
    let is_the_tabs_row = tab_data.panes().focused_id() == pane;
    let menu_is_open = is_the_tabs_row && workspace.tab_menu().tab == Some(tab);
    let opens_menu = is_the_tabs_row && git.is_some_and(|facts| facts.branch.is_some());

    // The one selected row in the whole bar.
    let is_selected = strip.is_active(tab) && tab_data.panes().is_focused(pane);

    // Under `Tabs` the row stands for its whole tab, so its close button
    // closes the tab — which is what Warp's tab-group header button does.
    // Under `Panes` it closes the pane it names, and the tab only goes if that
    // was the last one.
    let close_action = match options.granularity {
        Granularity::Panes => TabAction::ClosePane(pane),
        Granularity::Tabs => TabAction::Close(tab),
    };

    let facts = match session {
        Some(session) => RowFacts::resolve(session, git, home, ROW_PATH_CHARS),
        None => RowFacts::settings(),
    };
    let text = RowText::resolve(&facts, options);
    // Chips exist only in Expanded: Compact has no metadata line to hang one
    // on, which is exactly why the menu hides their toggles there.
    let chips = match (session, options.density) {
        (Some(session), Density::Expanded) => Chips::resolve(session, git, options),
        _ => Chips::default(),
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
            (theme().tab_active, theme().border)
        } else if hovered {
            (theme().surface, theme().tab_inactive)
        } else {
            (theme().tab_inactive, theme().tab_inactive)
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
                    // Centred in both densities. Warp top-aligns a multi-line
                    // row because its leading mark is a 24px icon that reads as
                    // a column heading; the strip's is a 7px dot, and pinning
                    // that to the top of a two-line row leaves it floating.
                    // The panel, whose leading mark *is* the 24px disc, aligns
                    // the way Warp does.
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(status_dot(status))
                    .with_child(Expanded::new(1., text.render(&chips, is_selected, ui)).finish())
                    .with_child(close_slot(close_action, close_state, show_close))
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
        ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::FocusPane(pane)));
    })
    .on_middle_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Tab(close_action));
    })
    // The secondary button, because this is a context menu and that is the
    // button a context menu opens on everywhere else. It was on the left one
    // — on the row you were already in, where the click was otherwise free —
    // and that made the menu something you could open by accident while
    // reaching for the tab you are already in, and something you could not
    // open at all on any other tab.
    .on_right_click(move |_, ctx, _| {
        if !opens_menu {
            return;
        }
        ctx.dispatch_typed_action(WorkspaceAction::Worktree(WorktreeAction::OpenMenu(tab)));
    })
    // Warp arms its detail sidecar on the very first hover event, with no
    // timer anywhere in the path, and a port that added the 300ms a tooltip
    // usually gets would feel materially different. There is none here either.
    .on_hover(move |entered, _, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::HoverRow { pane, entered });
    })
    .finish();

    let mut stack = Stack::new().with_child(element);
    if menu_is_open {
        // In the card's place rather than beside it. A stack anchors one
        // overlay: a second added after this one would make the row itself
        // unclickable, because a row hit-tests against the topmost overlay
        // that existed when it painted.
        stack.add_anchored_overlay_child(
            Dismiss::new(super::tab_menu::render(workspace))
                .modal()
                .on_dismiss(|ctx, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::Worktree(WorktreeAction::CloseMenu));
                })
                .finish(),
            AnchorTo::below(vec2f(0., 4.)),
        );
    } else if workspace.shows_details_for(pane) {
        // Added last, and it is the only overlay on this stack: a row
        // hit-tests against the topmost overlay that existed when it painted,
        // so a second one added after this would make the row unclickable.
        //
        // Below the row, because the strip runs along the top of the window
        // and there is nowhere else for a card to go. The panel hangs the same
        // card beside its rows instead.
        let sections: Vec<DetailSection<'_>> = detail_panes(tab_data, pane, options.granularity)
            .into_iter()
            .map(|pane| DetailSection {
                // `detail_panes` has already dropped the settings pane, so
                // every pane here has a session and a status.
                session: pane.session().expect("a card section without a session"),
                facts: pane
                    .session()
                    .and_then(|session| workspace.git_facts(session, app)),
                status: pane.status().unwrap_or_default(),
            })
            .collect();
        stack.add_anchored_overlay_child(
            detail_card(&sections, home, ui),
            AnchorTo::below(vec2f(0., 4.)),
        );
    }

    Expanded::new(
        1.,
        ConstrainedBox::new(stack.finish())
            .with_max_width(TAB_MAX_WIDTH)
            .finish(),
    )
    .finish()
}

/// A strip row's text: a title, and at most one line under it.
///
/// Warp's panel has room for three lines in `Expanded` and takes all of them.
/// A header row does not, so line 2 and line 3 collapse into one — the
/// fixed-height metadata line, carrying what Warp puts on its line 3 left. In
/// `Compact` there is no metadata line at all, and the optional second line is
/// chosen by "Additional metadata" instead.
///
/// Which fact lands on which line is [`RowFacts`]'s to say, not this type's:
/// the panel asks the same questions of the same table and gets consistent
/// answers because there is only one table.
struct RowText {
    title: RowLine,
    second: Option<RowLine>,
    density: Density,
}

impl RowText {
    fn resolve(facts: &RowFacts, options: TabOptions) -> Self {
        Self {
            title: facts.title(options.primary_info),
            second: match options.density {
                Density::Expanded => facts.metadata(options.primary_info),
                Density::Compact => facts.subtitle(options),
            },
            density: options.density,
        }
    }

    /// The text column, as the density wants it.
    fn render(self, chips: &Chips, is_selected: bool, ui: FamilyId) -> Box<dyn Element> {
        let (title_color, weight) = if is_selected {
            (theme().text_primary, Weight::Semibold)
        } else {
            (theme().text_muted, Weight::Normal)
        };
        let title = self.title.render(12.5, title_color, weight, ui);

        match self.density {
            // One line, and nothing that can make the row taller. Warp's
            // Compact adds a 10pt subtitle when the chosen metadata has a value
            // and omits the line entirely when it does not.
            Density::Compact => {
                let mut column = Flex::column()
                    .with_main_axis_size(MainAxisSize::Min)
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_spacing(1.)
                    .with_child(title);
                if let Some(second) = self.second {
                    column.add_child(second.render(10., theme().text_muted, Weight::Normal, ui));
                }
                column.finish()
            }

            // Two lines, the second of them height-locked so a chip appearing,
            // disappearing or being switched off never resizes the row.
            Density::Expanded => Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(title)
                .with_child(
                    Container::new(metadata_line(self.second, chips, ui))
                        .with_margin_top(2.)
                        .finish(),
                )
                .finish(),
        }
    }
}

/// The agent's status, as the one coloured thing on an unselected row — or, on
/// the settings row, a gear in the same slot.
///
/// The slot is the same width either way, so a settings row's text starts
/// where every other row's does. What is in it is a mark of a different *kind*
/// rather than a fifth colour: the settings pane is not an agent in a fifth
/// state, and a grey dot beside it would say it was idle.
fn status_dot(status: Option<AgentStatus>) -> Box<dyn Element> {
    /// The gap between the dot and the title.
    const DOT_GAP: f32 = 7.;
    /// The gear's size. Bigger than the dot it stands in for, because a 7px
    /// gear is a smudge.
    const GEAR_SIZE: f32 = 11.;

    let (mark, gap): (Box<dyn Element>, f32) = match status {
        Some(status) => (
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(super::status_color(status))
                    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                    .finish(),
            )
            .with_width(STATUS_DOT_SIZE)
            .with_height(STATUS_DOT_SIZE)
            .finish(),
            DOT_GAP,
        ),
        // The gear gets its own width and a narrower gap, so that the slot and
        // the gap still add up to the dot's — every row's title starts at the
        // same x whichever mark is in front of it. It is not squeezed into the
        // dot's 7px slot instead because a gear at seven pixels is a smudge:
        // Lucide's gear has a hole in the middle of it.
        None => (
            Icon::new(GEAR_ICON, GEAR_SIZE)
                .with_color(theme().text_muted)
                .finish(),
            STATUS_DOT_SIZE + DOT_GAP - GEAR_SIZE,
        ),
    };

    Container::new(mark).with_margin_right(gap).finish()
}

/// A fixed square, holding the close button or holding nothing.
fn close_slot(action: TabAction, state: MouseStateHandle, visible: bool) -> Box<dyn Element> {
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
                theme().border
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
