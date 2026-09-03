//! The tab strip: one rounded rect per row the strip says to draw.
//!
//! A row is a status dot, a text column, a close button and the box around
//! them, and three gestures: click to focus, middle-click to close, and the
//! close button. Every handler *emits* an action and none of them touches the
//! strip, so no id captured while rendering can be stale by the time the frame
//! is over.
//!
//! A row stands for a *pane*, in both granularities. Under `Panes` a tab
//! contributes one row per pane it holds; under `Tabs` it contributes one, for
//! its focused pane. That is Warp's rule, and it is why exactly one row in the
//! whole bar is selected however many are drawn: `is_active_tab && is_focused`
//! (`app/src/workspace/view/vertical_tabs.rs:412`). Tinting every row of the
//! active tab and marking the focused pane separately would give a bar where
//! three rows look chosen.
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

use std::path::Path;

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::git::{self, DiffStats, GitFacts, Head};
use crate::settings::{Density, Granularity, PrimaryInfo, Subtitle, TabOptions, resolve_subtitle};
use crate::tab::{AgentSession, AgentStatus, PaneId, TabAction, TabId};
use crate::theme::THEME;

use super::action::{OptionsAction, WorkspaceAction};
use super::view::Workspace;
use super::{CLOSE_BUTTON_SIZE, STATUS_DOT_SIZE, TAB_MAX_WIDTH, tab_options_menu};

/// The height a metadata line is pinned to, whether or not it has chips in it.
///
/// Warp's `METADATA_ROW_HEIGHT = BADGE_ICON_SIZE + 2`. Fixed on purpose:
/// letting it size to content means every arriving diff stat, and every flick
/// of the "Diff stats" toggle, reflows the entire strip.
const METADATA_ROW_HEIGHT: f32 = 14.;

/// How much of a working directory a row has room for.
///
/// A stand-in for the `ClipConfig::start()` Warp gives every path — clipped
/// from the *front*, so the tail survives. Crook's `Text` clips from the back,
/// which for a path throws away the only part worth reading, so the string is
/// cut before it gets there. Delete this the day the renderer grows a clip
/// config.
const ROW_PATH_CHARS: usize = 30;

/// The same, for the hover card, which is wider than a row.
const CARD_PATH_CHARS: usize = 46;

/// The whole strip, the button that opens another tab, and the options menu.
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_spacing(4.)
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::End);

    for (tab, pane) in workspace.tabs().rows(workspace.options().granularity) {
        row.add_child(render_row(workspace, tab, pane, app));
    }
    row.add_child(new_tab_button(workspace));
    row.add_child(options_button(workspace));

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

    let session = pane_data.session();
    let facts = workspace.git_facts(session, app);
    let status = pane_data.status();
    // Read once per row rather than once per line: it is the same answer for
    // every string a row prints, and a row prints up to four of them.
    let home = std::env::home_dir();
    let home = home.as_deref();
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

    let text = RowText::resolve(session, facts, home, options);
    // Chips exist only in Expanded: Compact has no metadata line to hang one
    // on, which is exactly why the menu hides their toggles there.
    let chips = match options.density {
        Density::Compact => Chips::default(),
        Density::Expanded => Chips::resolve(session, facts, options),
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
                    // Centred in both densities. Warp top-aligns a multi-line
                    // row because its leading mark is a 24px icon that reads as
                    // a column heading; Crook's is a 7px dot, and pinning that
                    // to the top of a two-line row leaves it floating.
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(status_dot(status))
                    .with_child(Expanded::new(1., text.render(&chips, is_selected, ui)).finish())
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
        ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::FocusPane(pane)));
    })
    .on_middle_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Tab(close_action));
    })
    // Warp arms its detail sidecar on the very first hover event, with no
    // timer anywhere in the path, and a port that added the 300ms a tooltip
    // usually gets would feel materially different. There is none here either.
    .on_hover(move |entered, _, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::HoverRow { pane, entered });
    })
    .finish();

    let mut stack = Stack::new().with_child(element);
    if workspace.shows_details_for(pane) {
        // Added last, and it is the only overlay on this stack: a row
        // hit-tests against the topmost overlay that existed when it painted,
        // so a second one added after this would make the row unclickable.
        stack.add_anchored_overlay_child(
            detail_card(session, facts, status, home, ui),
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

// --- what a row says ---------------------------------------------------------

/// One line of a row, and whether it is a branch rather than a path or a title.
#[derive(Clone)]
struct RowLine {
    text: String,
    is_branch: bool,
}

impl RowLine {
    fn plain(text: String) -> Self {
        Self {
            text,
            is_branch: false,
        }
    }

    fn branch(name: &str) -> Self {
        Self {
            text: name.to_owned(),
            is_branch: true,
        }
    }

    /// The line as elements: the branch mark, then the text.
    ///
    /// The mark is `size - 2` and sits 2px from the text, which is Warp's
    /// `render_git_branch_text` — 10px beside a 12pt title, 8px beside a 10pt
    /// metadata line.
    fn render(self, size: f32, color: Color, weight: Weight, ui: FamilyId) -> Box<dyn Element> {
        let text = Text::new(self.text, ui, size)
            .with_color(color)
            .with_style(Properties {
                weight,
                ..Default::default()
            })
            .finish();
        if !self.is_branch {
            return text;
        }

        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(2.)
            .with_child(branch_mark(size - 2., color))
            // Yields its width to nothing else on the line, but clips rather
            // than pushing the mark off the row.
            .with_child(Shrinkable::new(1., text).finish())
            .finish()
    }
}

/// A row's text, resolved once from the options, the session and the git facts.
///
/// Warp's table, as a comment in `vertical_tabs.rs:4396`:
///
/// ```text
/// | Pane title as    | line 1               | line 2 (Expanded)|
/// | Command          | command/conversation | git branch       |
/// | WorkingDirectory | working directory    | git branch       |
/// | Branch           | git branch           | working directory|
/// ```
///
/// Warp's Expanded row has three lines because its panel is a 248px column;
/// Crook's strip is a header row, so line 2 and line 3 are the same line — the
/// fixed-height metadata line, carrying what Warp puts on its line 3 left. In
/// Compact there is no metadata line at all, and the optional second line is
/// chosen by "Additional metadata" instead.
struct RowText {
    title: RowLine,
    second: Option<RowLine>,
    density: Density,
}

impl RowText {
    fn resolve(
        session: &AgentSession,
        facts: Option<&GitFacts>,
        home: Option<&Path>,
        options: TabOptions,
    ) -> Self {
        let branch = facts
            .and_then(|facts| facts.branch.as_ref())
            .map(Head::label);
        let directory = session
            .working_directory
            .as_deref()
            .map(|dir| git::user_friendly_path(dir, home));
        let path_line = || {
            directory
                .as_deref()
                .map(|dir| RowLine::plain(git::truncate_start(dir, ROW_PATH_CHARS)))
        };

        let title = match options.primary_info {
            PrimaryInfo::Command => RowLine::plain(session.display_title().to_owned()),
            PrimaryInfo::WorkingDirectory => {
                path_line().unwrap_or_else(|| RowLine::plain(session.display_title().to_owned()))
            }
            PrimaryInfo::Branch => {
                // Outside a repository this quietly prints the working
                // directory with no branch mark, which is Warp's rule and its
                // one surprise: a row never says "no branch".
                let fallback = path_line()
                    .map(|line| line.text)
                    .unwrap_or_else(|| session.display_title().to_owned());
                let (text, is_branch) = git::branch_label(branch, &fallback);
                RowLine { text, is_branch }
            }
        };

        let second = match options.density {
            Density::Expanded => match options.primary_info {
                PrimaryInfo::Command | PrimaryInfo::WorkingDirectory => branch.map(RowLine::branch),
                PrimaryInfo::Branch => path_line(),
            },
            Density::Compact => {
                match resolve_subtitle(options.primary_info, options.subtitle) {
                    // Warp's `compact_branch_subtitle_display`: the branch,
                    // else the directory, else no second line at all.
                    Subtitle::Branch => branch.map(RowLine::branch).or_else(path_line),
                    Subtitle::WorkingDirectory => path_line(),
                    Subtitle::Command => Some(RowLine::plain(session.display_title().to_owned())),
                }
            }
        };

        Self {
            title,
            second,
            density: options.density,
        }
    }

    /// The text column, as the density wants it.
    fn render(self, chips: &Chips, is_selected: bool, ui: FamilyId) -> Box<dyn Element> {
        let (title_color, weight) = if is_selected {
            (THEME.text_primary, Weight::Semibold)
        } else {
            (THEME.text_muted, Weight::Normal)
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
                    column.add_child(second.render(10., THEME.text_muted, Weight::Normal, ui));
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

/// The fixed-height second line of an Expanded row: text on the left, chips
/// pushed to the right.
fn metadata_line(left: Option<RowLine>, chips: &Chips, ui: FamilyId) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center);

    // Shrinkable, so the text clips before it ever squeezes a chip.
    row.add_child(match left {
        Some(left) => {
            Shrinkable::new(1., left.render(10., THEME.text_muted, Weight::Normal, ui)).finish()
        }
        None => Empty::new().finish(),
    });

    if let Some(chips) = chips.render(ui) {
        // The padding is part of the right element's natural width, so
        // SpaceBetween keeps a 4px gap even when the left text has collapsed
        // to nothing.
        row.add_child(Container::new(chips).with_padding_left(4.).finish());
    }

    ConstrainedBox::new(row.finish())
        .with_height(METADATA_ROW_HEIGHT)
        .finish()
}

// --- chips -------------------------------------------------------------------

/// The chips a row is allowed to draw, after the toggles have had their say.
///
/// Resolved before the row is built rather than inside it, because "is there
/// anything to show" decides whether the metadata line reserves a gap.
#[derive(Default)]
struct Chips {
    diff: Option<DiffStats>,
    pull_request: Option<String>,
}

impl Chips {
    fn resolve(session: &AgentSession, facts: Option<&GitFacts>, options: TabOptions) -> Self {
        Self {
            // A clean tree draws no chip at all, not a `0` — Warp filters the
            // same way one layer up, which is why the "0" token its formatter
            // can produce is unreachable from a row.
            diff: options
                .show_diff_stats
                .then(|| facts.and_then(|facts| facts.diff))
                .flatten()
                .filter(|diff| !diff.is_empty()),
            // Always `None` today: nothing populates `pull_request`, because
            // Crook has no forge to ask. The toggle is still live — it governs
            // whether the slot appears when there *is* a link — and the menu
            // says so on screen rather than leaving a chip that can never
            // appear to be discovered.
            pull_request: options
                .show_pr_link
                .then(|| session.pull_request_label())
                .flatten(),
        }
    }

    fn is_empty(&self) -> bool {
        self.diff.is_none() && self.pull_request.is_none()
    }

    /// The chips, in Warp's order: diff stats first, then the pull request.
    fn render(&self, ui: FamilyId) -> Option<Box<dyn Element>> {
        if self.is_empty() {
            return None;
        }

        let mut row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(4.);
        if let Some(diff) = self.diff {
            row.add_child(diff_chip(diff, ui));
        }
        if let Some(label) = self.pull_request.clone() {
            row.add_child(pill(
                Text::new(label, ui, 10.)
                    .with_color(THEME.text_muted)
                    .finish(),
            ));
        }
        Some(row.finish())
    }
}

/// A chip's box: Warp's `render_badge_container`.
///
/// It has no hover state, unlike Warp's, because Crook's chips are not
/// clickable — there is no code-review panel to open and no browser call to
/// make — and a box that lights up under the pointer and then does nothing is a
/// worse lie than one that does not.
fn pill(content: Box<dyn Element>) -> Box<dyn Element> {
    Container::new(content)
        .with_padding(Padding {
            top: 1.,
            left: 4.,
            bottom: 1.,
            right: 4.,
        })
        .with_background_color(THEME.overlay_1)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
        .finish()
}

/// `+12 -3`, each token in its own colour.
fn diff_chip(diff: DiffStats, ui: FamilyId) -> Box<dyn Element> {
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);

    for (index, token) in diff.tokens().into_iter().enumerate() {
        if index > 0 {
            row.add_child(Text::new(" ", ui, 10.).finish());
        }

        let color = if token.starts_with('+') {
            THEME.diff_added
        } else if token.starts_with('-') {
            THEME.diff_removed
        } else {
            THEME.text_muted
        };
        row.add_child(
            Text::new(token, ui, 10.)
                .with_color(color)
                .with_style(Properties {
                    weight: Weight::Semibold,
                    ..Default::default()
                })
                .finish(),
        );
    }

    pill(row.finish())
}

// --- the hover card ----------------------------------------------------------

/// How wide the hover card is. Warp's `DETAIL_SIDECAR_DEFAULT_WIDTH`.
const CARD_WIDTH: f32 = 320.;

/// What the row could not fit, on hover.
///
/// Warp's detail sidecar, reduced to the sections Crook has data for. Two of
/// its rules are worth keeping exactly: it opens with no delay at all, and it
/// **ignores the "Show" toggles** — turning the chips off on the row leaves
/// them on in the card, because those two settings govern the row and this is
/// not the row. Only "Show details on hover" gates the card itself.
fn detail_card(
    session: &AgentSession,
    facts: Option<&GitFacts>,
    status: AgentStatus,
    home: Option<&Path>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.)
        .with_child(
            Text::new(session.display_title().to_owned(), ui, 12.)
                .with_color(THEME.text_primary)
                .finish(),
        );

    if let Some(directory) = session.working_directory.as_deref() {
        let friendly = git::user_friendly_path(directory, home);
        column.add_child(
            Text::new(git::truncate_start(&friendly, CARD_PATH_CHARS), ui, 12.)
                .with_color(THEME.text_muted)
                .finish(),
        );
    }

    if let Some(branch) = facts.and_then(|facts| facts.branch.as_ref()) {
        column.add_child(RowLine::branch(branch.label()).render(
            12.,
            THEME.text_muted,
            Weight::Normal,
            ui,
        ));
    }

    // Built from the facts directly, with no reference to the "Show" toggles:
    // those two settings govern the row, and this is what the row could not
    // fit.
    let chips = Chips {
        diff: facts
            .and_then(|facts| facts.diff)
            .filter(|diff| !diff.is_empty()),
        pull_request: session.pull_request_label(),
    };
    let mut footer = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Text::new(format!("agent \u{b7} {}", status.label()), ui, 10.)
                .with_color(THEME.text_muted)
                .finish(),
        );
    if let Some(chips) = chips.render(ui) {
        footer.add_child(chips);
    }
    column.add_child(footer.finish());

    ConstrainedBox::new(
        Container::new(column.finish())
            .with_uniform_padding(12.)
            .with_background_color(THEME.surface_raised)
            .with_border(Border::all(1.).with_border_color(THEME.overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish(),
    )
    .with_width(CARD_WIDTH)
    .finish()
}

// --- marks and buttons -------------------------------------------------------

/// The mark that says a line is a branch and not a path.
///
/// Warp draws `UiIcon::GitBranch` from an SVG at `font_size - 2`. Crook has no
/// icon system, so this is the same geometry out of three rectangles: a trunk,
/// an arm, and the node the arm leads to. The slot is the same size either way,
/// which is what keeps the two ports' rows the same width.
fn branch_mark(size: f32, color: Color) -> Box<dyn Element> {
    let rule = |width: f32, height: f32| {
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(color)
                .finish(),
        )
        .with_width(width)
        .with_height(height)
        .finish()
    };

    Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(rule(1.5, size))
        .with_child(rule(size * 0.4, 1.5))
        .with_child(
            ConstrainedBox::new(
                Container::new(Empty::new().finish())
                    .with_background_color(color)
                    .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                    .finish(),
            )
            .with_width(3.)
            .with_height(3.)
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
        .on_click(|_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::New)))
        .finish(),
    )
    .with_width(24.)
    .with_height(24.)
    .finish()
}

/// What the gear says it does, while the pointer is on it and the menu is not
/// up.
pub(super) const GEAR_TOOLTIP: &str = "View options";

/// The gear's hit box: a 16px glyph slot with 2px of padding all round.
const GEAR_BUTTON_SIZE: f32 = 20.;

/// The tooltip's width, fixed so the centring offset below can be a constant.
///
/// Wide enough for [`GEAR_TOOLTIP`] at 11px plus its 16px of padding, with
/// room for an interface font wider than the average; the label is centred
/// inside it, so slack shows as symmetric margin rather than as a gap on one
/// side.
const GEAR_TOOLTIP_WIDTH: f32 = 88.;

/// The gear, and the menu it opens.
///
/// The [`Stack`] goes here, around the button, rather than at the root: the
/// menu is anchored to the gear's painted box, and a stack wrapping the whole
/// window would have nothing to anchor to. The stack is painted before the menu
/// in the same frame, so the anchor is this frame's rect and the menu never
/// lags a frame behind the button.
fn options_button(workspace: &Workspace) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let is_open = workspace.is_options_menu_open();

    let gear_state = workspace.menu().gear.clone();
    if is_open {
        // The modal underlay covers the gear while the menu is up, so its
        // `Hoverable` never sees the pointer leave. Without this the frame
        // after the menu is dismissed draws a tooltip for a gear the pointer
        // walked away from several clicks ago.
        gear_state.lock().reset_interaction_state();
    }

    let gear = Hoverable::new(gear_state, move |state| {
        // Three states, and the open one is not reachable through this
        // handler: while the menu is up, the modal underlay covers the gear, so
        // its `Hoverable` never fires and a second click closes the menu
        // through the dismiss path instead of toggling it twice.
        let (glyph, background) = if is_open {
            (THEME.text_primary, THEME.overlay_3)
        } else if state.is_hovered() {
            (THEME.text_muted, THEME.overlay_2)
        } else {
            (THEME.text_muted, Color::TRANSPARENT)
        };

        let button = Container::new(
            ConstrainedBox::new(
                Align::new(Text::new("\u{2699}", ui, 14.).with_color(glyph).finish()).finish(),
            )
            .with_width(16.)
            .with_height(16.)
            .finish(),
        )
        .with_uniform_padding(2.)
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish();

        // The tooltip belongs to the closed-and-hovered state only. Warp
        // suppresses it with the same `is_hovered() && !is_popup_open`, and it
        // has to be suppressed rather than merely unreachable: the gear is the
        // one thing the popup hangs off, so a tooltip left up would sit
        // between the button and its own menu.
        if !state.is_hovered() || is_open {
            return button;
        }

        let mut stack = Stack::new().with_child(button);
        stack.add_anchored_overlay_child(
            gear_tooltip(ui),
            AnchorTo {
                // Warp's `ParentAnchor::BottomMiddle` →
                // `ChildAnchor::TopMiddle`, 4px below. [`Corner`] has only the
                // four corners, so the centring is the offset's job, which is
                // exact because both widths are constants.
                parent: Corner::BottomLeft,
                child: Corner::TopLeft,
                offset: vec2f(-(GEAR_TOOLTIP_WIDTH - GEAR_BUTTON_SIZE) / 2., 4.),
                keep_on_screen: true,
            },
        );
        stack.finish()
    })
    .on_click(|_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Options(OptionsAction::TogglePopup));
    })
    .finish();

    let mut stack = Stack::new().with_child(gear);
    if is_open {
        stack.add_anchored_overlay_child(
            Dismiss::new(tab_options_menu::render(workspace))
                // The rest of the window is inert while the menu is up, which
                // is what makes re-clicking the gear one toggle rather than
                // two, and what stops a tab under the menu from hovering.
                .modal()
                .on_dismiss(|ctx, _| {
                    // The element only reports; taking the menu down is this
                    // handler's job, because what closing means belongs to the
                    // view that opened it.
                    ctx.dispatch_typed_action(WorkspaceAction::Options(OptionsAction::TogglePopup));
                })
                .finish(),
            AnchorTo::below(vec2f(0., 4.)),
        );
    }

    ConstrainedBox::new(Align::new(stack.finish()).finish())
        .with_width(24.)
        .with_height(24.)
        .finish()
}

/// The little panel that names the gear.
fn gear_tooltip(ui: FamilyId) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(
            Align::new(
                Text::new(GEAR_TOOLTIP, ui, 11.)
                    .with_color(THEME.text_primary)
                    .finish(),
            )
            .finish(),
        )
        .with_background_color(THEME.surface_raised)
        .with_border(Border::all(1.).with_border_color(THEME.overlay_2))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_padding(Padding {
            top: 4.,
            left: 8.,
            bottom: 5.,
            right: 8.,
        })
        .finish(),
    )
    .with_width(GEAR_TOOLTIP_WIDTH)
    .finish()
}
