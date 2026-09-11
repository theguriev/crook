//! What a row says, and the small pieces it is drawn out of.
//!
//! One shape now — the panel's — and this file is what is left of the day
//! there were two: a row is described here and laid out there, which is why
//! the description survived the strip that used to share it.
//! They differ in how many lines they have room for and in what wraps them,
//! and they must not differ in *which fact goes on which line* — that is
//! Warp's table, it is what the "Pane title as" menu section writes, and two
//! copies of it would disagree the first time one was edited.
//!
//! So the table lives here once, as [`RowFacts`]: resolve a session and its
//! git facts into three strings, then ask for the title, the description, the
//! metadata or the subtitle. Warp's comment at `vertical_tabs.rs:4396`:
//!
//! ```text
//! | Pane title as    | line 1               | line 2 (description) | line 3 left       |
//! | Command          | command/conversation | working directory    | git branch        |
//! | WorkingDirectory | working directory    | command/conversation | git branch        |
//! | Branch           | git branch           | command/conversation | working directory |
//! ```
//!
//! The strip has room for two lines and takes line 1 and line 3's left text;
//! the panel has room for three in `Expanded` and takes all of them. Neither
//! decides what those lines *are*.

use std::path::Path;

use crookui_core::elements::Padding;
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::git::{self, DiffStats, GitFacts, Head};
use crate::settings::{Granularity, PrimaryInfo, Subtitle, TabOptions, resolve_subtitle};
use crate::tab::{AgentSession, Pane, PaneId, Tab};
use crate::theme::theme;

/// The height a metadata line is pinned to, whether or not it has chips in it.
///
/// Warp's `METADATA_ROW_HEIGHT = BADGE_ICON_SIZE + 2`. Fixed on purpose:
/// letting it size to content means every arriving diff stat, and every flick
/// of the "Diff stats" toggle, reflows the entire list.
pub(super) const METADATA_ROW_HEIGHT: f32 = 14.;

/// The same, for a panel row: 248px of column, minus the icon and the padding,
/// holds fewer characters than a strip row that may be 220 wide with no icon.
pub(super) const PANEL_PATH_CHARS: usize = 26;

/// The same, for the hover card, which is wider than either.
pub(super) const CARD_PATH_CHARS: usize = 46;

/// One line of a row, and whether it is a branch rather than a path or a title.
#[derive(Clone)]
pub(super) struct RowLine {
    text: String,
    is_branch: bool,
}

impl RowLine {
    pub(super) fn plain(text: String) -> Self {
        Self {
            text,
            is_branch: false,
        }
    }

    pub(super) fn branch(name: &str) -> Self {
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
    pub(super) fn render(
        self,
        size: f32,
        color: Color,
        weight: Weight,
        ui: FamilyId,
    ) -> Box<dyn Element> {
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

/// The three facts every row is written out of, resolved once.
///
/// Resolved rather than looked up per line, because a row asks for two or
/// three of them and each one costs a path abbreviation or an `Option` walk.
pub(super) struct RowFacts {
    /// What the session is doing: the agent's own name for its work.
    command: String,
    /// Where it is working, already abbreviated and cut to fit.
    directory: Option<String>,
    /// The branch it is on, if the directory is a repository.
    branch: Option<String>,
    /// Whether [`Self::command`] is the directory's own name rather than a
    /// name the session has.
    ///
    /// What stops a row printing the same fact twice: with no name of its own
    /// the title becomes `crook`, and the line under it must not then be
    /// `~/work/crook`.
    command_is_the_directory: bool,
}

impl RowFacts {
    /// Reads a session and what git says about where it sits.
    ///
    /// `path_chars` is the row's width budget for a path — a panel row is
    /// narrower than a strip row, and the card is wider than both.
    pub(super) fn resolve(
        session: &AgentSession,
        facts: Option<&GitFacts>,
        home: Option<&Path>,
        path_chars: usize,
    ) -> Self {
        // A name the session has, else the directory's own — `agent 3` is the
        // last resort it always was, and now reaches only a pane with no name,
        // no program and nowhere to be.
        let named = session.name().map(str::to_owned);
        let label = session
            .working_directory
            .as_deref()
            .and_then(|directory| git::directory_label(directory, home));
        let command_is_the_directory = named.is_none() && label.is_some();

        Self {
            command: named
                .or(label)
                .unwrap_or_else(|| session.display_title().to_owned()),
            command_is_the_directory,
            directory: session.working_directory.as_deref().map(|directory| {
                git::truncate_start(&git::user_friendly_path(directory, home), path_chars)
            }),
            branch: facts
                .and_then(|facts| facts.branch.as_ref())
                .map(Head::label)
                .map(str::to_owned),
        }
    }

    /// The title line, per "Pane title as".
    ///
    /// Every arm falls back to the command, because a row with no title line
    /// is not a row. Warp's `Branch` arm quietly prints the working directory
    /// with no branch mark outside a repository, which is its one surprise: a
    /// row never says "no branch".
    pub(super) fn title(&self, primary: PrimaryInfo) -> RowLine {
        match primary {
            PrimaryInfo::Command => RowLine::plain(self.command.clone()),
            PrimaryInfo::WorkingDirectory => self
                .directory
                .clone()
                .map_or_else(|| RowLine::plain(self.command.clone()), RowLine::plain),
            PrimaryInfo::Branch => {
                let fallback = self
                    .directory
                    .clone()
                    .unwrap_or_else(|| self.command.clone());
                let (text, is_branch) = git::branch_label(self.branch.as_deref(), &fallback);
                RowLine { text, is_branch }
            }
        }
    }

    /// The description line: whichever of the command and the working
    /// directory the title did not take.
    pub(super) fn description(&self, primary: PrimaryInfo) -> Option<RowLine> {
        // Nothing at all when the title and the command are the same fact
        // spelled two ways: `crook` over `~/work/crook` is one thing taking two
        // lines, whichever of them the setting put on top. The branch line
        // below is untouched, and is a second fact.
        if self.command_is_the_directory {
            return None;
        }
        match primary {
            PrimaryInfo::Command => self.directory.clone().map(RowLine::plain),
            PrimaryInfo::WorkingDirectory | PrimaryInfo::Branch => {
                Some(RowLine::plain(self.command.clone()))
            }
        }
    }

    /// The metadata line's left text: the branch, or the working directory
    /// when the branch is already the title.
    pub(super) fn metadata(&self, primary: PrimaryInfo) -> Option<RowLine> {
        match primary {
            PrimaryInfo::Command | PrimaryInfo::WorkingDirectory => {
                self.branch.as_deref().map(RowLine::branch)
            }
            PrimaryInfo::Branch => self.directory.clone().map(RowLine::plain),
        }
    }

    /// A `Compact` row's second line, per "Additional metadata".
    ///
    /// Read through [`resolve_subtitle`] here as well as in the menu. Warp
    /// calls its `resolve_compact_subtitle` at both sites for the same reason:
    /// a menu that checked the stored value would put a tick beside an option
    /// the row is silently overriding.
    pub(super) fn subtitle(&self, options: TabOptions) -> Option<RowLine> {
        match resolve_subtitle(options.primary_info, options.subtitle) {
            // Warp's `compact_branch_subtitle_display`: the branch, else the
            // directory, else no second line at all.
            Subtitle::Branch => self
                .branch
                .as_deref()
                .map(RowLine::branch)
                .or_else(|| self.directory.clone().map(RowLine::plain)),
            Subtitle::WorkingDirectory => self.directory.clone().map(RowLine::plain),
            // The same rule as the description line: a compact row asked for
            // the command, and a command that is only the directory restated
            // is a second line saying what the first one said.
            Subtitle::Command if self.command_is_the_directory => None,
            Subtitle::Command => Some(RowLine::plain(self.command.clone())),
        }
    }
}

/// The chips a row is allowed to draw, after the toggles have had their say.
///
/// Resolved before the row is built rather than inside it, because "is there
/// anything to show" decides whether the metadata line reserves a gap.
#[derive(Default)]
pub(super) struct Chips {
    diff: Option<DiffStats>,
    pull_request: Option<String>,
}

impl Chips {
    pub(super) fn resolve(
        session: &AgentSession,
        facts: Option<&GitFacts>,
        options: TabOptions,
    ) -> Self {
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

    /// Every chip the data supports, with no reference to the "Show" toggles.
    ///
    /// What the hover card draws. Warp's `render_terminal_detail_section`
    /// ignores `show_pr_link` and `show_diff_stats` outright: those two
    /// settings govern the row, and the card is what the row could not fit.
    pub(super) fn everything(session: &AgentSession, facts: Option<&GitFacts>) -> Self {
        Self {
            diff: facts
                .and_then(|facts| facts.diff)
                .filter(|diff| !diff.is_empty()),
            pull_request: session.pull_request_label(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.diff.is_none() && self.pull_request.is_none()
    }

    /// The chips, in Warp's order: diff stats first, then the pull request.
    pub(super) fn render(&self, ui: FamilyId) -> Option<Box<dyn Element>> {
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
                    .with_color(theme().text_muted)
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
        .with_background_color(theme().overlay_1)
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
            theme().diff_added
        } else if token.starts_with('-') {
            theme().diff_removed
        } else {
            theme().text_muted
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

/// How heavy the branch mark's stroke is, in Lucide's 24-unit grid.
///
/// Above Lucide's own 2, because this is the smallest icon in the interface —
/// eight pixels beside a ten-pixel subtitle — and at eight pixels a 0.67px
/// stroke leaves the two nodes as grey smudges rather than as circles.
const BRANCH_STROKE_WIDTH: f32 = 2.75;

/// The mark that says a line is a branch and not a path.
///
/// Warp draws `UiIcon::GitBranch` from an SVG at `font_size - 2`, and so does
/// this now: it is Lucide's `git-branch`, at the same size, where three
/// rectangles used to stand in for one because there was no way to draw a
/// path. The slot is the size it always was, which is what keeps the two
/// ports' rows the same width.
fn branch_mark(size: f32, color: Color) -> Box<dyn Element> {
    Icon::new(Lucide::GitBranch, size)
        .with_stroke_width(BRANCH_STROKE_WIDTH)
        .with_color(color)
        .finish()
}

/// How wide the hover card is. Warp's `DETAIL_SIDECAR_DEFAULT_WIDTH`.
pub(super) const CARD_WIDTH: f32 = 320.;

/// Warp's `DETAIL_SIDECAR_SECTION_PADDING`, inside each section rather than
/// around the card, because a card can hold more than one of them.
const CARD_SECTION_PADDING: f32 = 12.;

/// One pane, as the hover card describes it.
///
/// A card is a list of these. Which panes go in it is granularity's to say —
/// see [`detail_card`] — and neither this nor the card knows the difference.
pub(super) struct DetailSection<'a> {
    /// The pane's agent and what it is working on.
    pub(super) session: &'a AgentSession,
    /// What git says about where that session sits.
    pub(super) facts: Option<&'a GitFacts>,
    /// What the agent is doing.
    pub(super) status: crate::tab::AgentStatus,
}

/// What the row could not fit, on hover.
///
/// Warp's detail sidecar, reduced to the sections Crook has data for. Two of
/// its rules are worth keeping exactly: it opens with no delay at all, and it
/// **ignores the "Show" toggles** — turning the chips off on the row leaves
/// them on in the card, because those two settings govern the row and this is
/// not the row. Only "Show details on hover" gates the card itself.
///
/// The third rule is why this takes a list. Warp's sidecar target follows the
/// granularity: `Pane` in `Panes` view, where the row already stands for one
/// pane, and `Tab` in `Tabs` view, where the list shows one row per tab and
/// *drops* the rest of its panes — no count, no expander, nothing. The card is
/// the only place those panes surface, so in that view it carries a section
/// per visible pane, divided by hairlines.
///
/// One card for both layouts. What differs between them is where it is
/// anchored — below a strip row, beside a panel row — and that is the caller's
/// to decide, because only the caller knows which side the panel is on.
pub(super) fn detail_card(
    sections: &[DetailSection<'_>],
    home: Option<&Path>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    for (index, section) in sections.iter().enumerate() {
        if index > 0 {
            column.add_child(section_divider());
        }
        column.add_child(detail_section(section, home, ui));
    }

    ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish(),
    )
    .with_width(CARD_WIDTH)
    .finish()
}

/// Which of a tab's panes the card that opened over `pane`'s row describes.
///
/// Warp's `detail_target_for_hovered_row` followed by
/// `visible_pane_ids_for_detail_target`, and the whole of why granularity
/// reaches the card at all. In `Panes` a row is a pane and the card is that
/// pane. In `Tabs` a row is a tab whose other panes were dropped from the
/// list entirely — so the card is the tab, every visible pane of it, in the
/// order the tab holds them.
///
/// Warp's target additionally collapses to nothing when *any* pane in the tab
/// is of a kind with no sidecar. Every Crook pane is an agent session, so that
/// clause has nothing to exclude and is not written here.
pub(super) fn detail_panes(tab: &Tab, pane: PaneId, granularity: Granularity) -> Vec<&Pane> {
    // Never the settings pane: the card exists to show what a row had no room
    // for, and a settings row has nothing behind its one line. Filtered here
    let panes: Vec<&Pane> = match granularity {
        Granularity::Panes => tab.panes().get(pane).into_iter().collect(),
        Granularity::Tabs => tab.panes().iter().collect(),
    };
    panes
}

/// The hairline between two sections of a card.
fn section_divider() -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(theme().overlay_2)
            .finish(),
    )
    .with_height(1.)
    .finish()
}

/// One pane's paragraph of a card.
fn detail_section(
    section: &DetailSection<'_>,
    home: Option<&Path>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let DetailSection {
        session,
        facts,
        status,
    } = *section;

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_spacing(4.)
        .with_child(
            Text::new(session.display_title().to_owned(), ui, 12.)
                .with_color(theme().text_primary)
                .finish(),
        );

    if let Some(directory) = session.working_directory.as_deref() {
        let friendly = git::user_friendly_path(directory, home);
        column.add_child(
            Text::new(git::truncate_start(&friendly, CARD_PATH_CHARS), ui, 12.)
                .with_color(theme().text_muted)
                .finish(),
        );
    }

    if let Some(branch) = facts.and_then(|facts| facts.branch.as_ref()) {
        column.add_child(RowLine::branch(branch.label()).render(
            12.,
            theme().text_muted,
            Weight::Normal,
            ui,
        ));
    }

    let mut footer = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Text::new(format!("agent \u{b7} {}", status.label()), ui, 10.)
                .with_color(theme().text_muted)
                .finish(),
        );
    if let Some(chips) = Chips::everything(session, facts).render(ui) {
        footer.add_child(chips);
    }
    column.add_child(footer.finish());

    Container::new(column.finish())
        .with_uniform_padding(CARD_SECTION_PADDING)
        .finish()
}

/// The fixed-height metadata line: text on the left, chips pushed to the right.
///
/// Both layouts draw this, and both need it height-locked for the same reason:
/// a chip appearing, disappearing or being switched off must not resize the
/// row it is in.
pub(super) fn metadata_line(
    left: Option<RowLine>,
    chips: &Chips,
    ui: FamilyId,
) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
        .with_cross_axis_alignment(CrossAxisAlignment::Center);

    // Shrinkable, so the text clips before it ever squeezes a chip.
    row.add_child(match left {
        Some(left) => {
            Shrinkable::new(1., left.render(10., theme().text_muted, Weight::Normal, ui)).finish()
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

#[cfg(test)]
mod naming_tests {
    use super::*;
    use std::path::PathBuf;

    fn session(directory: &str) -> AgentSession {
        let mut session = AgentSession::new("agent 3");
        session.working_directory = Some(PathBuf::from(directory));
        session
    }

    fn facts(session: &AgentSession) -> RowFacts {
        RowFacts::resolve(session, None, Some(Path::new("/Users/eugen")), 40)
    }

    /// A tab at a prompt is about where it is, and says so once.
    ///
    /// The row used to read `agent 3` over `~/work/crook`: a placeholder that
    /// named nothing, above the only fact on the row. Now the fact is the
    /// title, and the line that repeated it is gone.
    #[test]
    fn a_tab_with_no_name_is_called_after_its_directory_once() {
        let facts = facts(&session("/Users/eugen/work/crook"));
        assert_eq!(facts.title(PrimaryInfo::Command).text, "crook");
        assert!(
            facts.description(PrimaryInfo::Command).is_none(),
            "`crook` over `~/work/crook` is one fact printed twice"
        );
    }

    /// And once it is running something, that is what it is about — with the
    /// directory back underneath, because now they are two different facts.
    #[test]
    fn a_running_command_names_the_tab_and_the_path_comes_back() {
        let mut session = session("/Users/eugen/work/crook");
        session.running_command = Some("cargo test".to_owned());
        let facts = facts(&session);
        assert_eq!(facts.title(PrimaryInfo::Command).text, "cargo test");
        assert_eq!(
            facts
                .description(PrimaryInfo::Command)
                .map(|line| line.text),
            Some("~/work/crook".to_owned())
        );
    }

    /// The setting that puts the directory on top must not then print its own
    /// short name underneath.
    #[test]
    fn the_directory_is_not_repeated_whichever_line_it_is_on() {
        let facts = facts(&session("/Users/eugen/work/crook"));
        assert_eq!(
            facts.title(PrimaryInfo::WorkingDirectory).text,
            "~/work/crook"
        );
        assert!(facts.description(PrimaryInfo::WorkingDirectory).is_none());
    }

    /// Nowhere to be and nothing running: the placeholder is still the honest
    /// answer, because a row with no title line is not a row.
    #[test]
    fn the_placeholder_survives_where_there_is_nothing_else() {
        let facts = facts(&session("/"));
        assert_eq!(facts.title(PrimaryInfo::Command).text, "agent 3");
    }
}
