//! What each page of the settings holds.
//!
//! Every control here writes through the action the gear menu already
//! dispatches, so the popup and the page cannot disagree about what an option
//! means or about when it is saved. The only two actions that are the page's
//! own are the ones the popup has no control for: the usage chip, and the
//! reset.
//!
//! The pages are Warp's, minus everything Crook does not have. Warp's
//! Appearance page carries eleven categories; the two of them that describe a
//! tab strip are the two here. Its Features page carries eight; not one of
//! them names something Crook can do. What is left is a rail of four, and a
//! settings page that offered more than the application has would be a
//! catalogue of things that do not work.

use crookui_core::prelude::*;

use super::super::action::{OptionsAction, SettingsAction};
use super::super::view::Workspace;
use super::widgets::{self, Segment};
use super::{Control, Section};
use crate::input_keys::Platform;
use crate::settings::{
    Density, Granularity, Layout, PrimaryInfo, TabOptions, resolve_subtitle, subtitle_options_for,
};

/// A binding as this platform spells it.
///
/// The two keymaps are not one chord with the modifier swapped. Off macOS a
/// bare ctrl-letter belongs to the program in the pane — ctrl-c interrupts it,
/// ctrl-d ends its input — so Crook's own chords take a Shift there and the
/// tabs move on Page Up and Page Down rather than on the arrows the field
/// selects with. A page that printed `cmd` or `ctrl` in front of one spelling
/// would name chords the window does not answer to, so each row spells both
/// out and this picks between them with the same [`Platform::current`] the
/// window delegate resolves a keystroke with.
fn chord(mac: &str, other: &str) -> String {
    match Platform::current() {
        Platform::Mac => mac,
        Platform::Other => other,
    }
    .to_owned()
}

/// One page.
pub(super) fn render(
    workspace: &Workspace,
    section: Section,
    app: &AppContext,
) -> Box<dyn Element> {
    match section {
        Section::Appearance => appearance(workspace),
        Section::Usage => usage(workspace, app),
        Section::Keys => keys(workspace),
        Section::About => about(workspace),
    }
}

/// A column of categories, which is what every page is.
fn page(children: Vec<Box<dyn Element>>) -> Box<dyn Element> {
    Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_children(children)
        .finish()
}

/// Where the tabs are and what a row of them looks like.
fn appearance(workspace: &Workspace) -> Box<dyn Element> {
    let options = workspace.options();
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();

    let placement = widgets::row(
        "Tab placement",
        Some("Where the list of what you are working on lives."),
        true,
        widgets::segmented(
            vec![
                Segment {
                    label: "Side panel",
                    selected: options.layout == Layout::Vertical,
                    command: Some(OptionsAction::SetLayout(Layout::Vertical).into()),
                    state: state.control(Control::Layout(Layout::Vertical)),
                },
                Segment {
                    label: "Header strip",
                    selected: options.layout == Layout::Horizontal,
                    command: Some(OptionsAction::SetLayout(Layout::Horizontal).into()),
                    state: state.control(Control::Layout(Layout::Horizontal)),
                },
            ],
            ui,
        ),
        ui,
    );

    let granularity = widgets::row(
        "View as",
        Some("Whether one row stands for a single pane or for a whole tab."),
        true,
        widgets::segmented(
            vec![
                Segment {
                    label: "Panes",
                    selected: options.granularity == Granularity::Panes,
                    command: Some(OptionsAction::SetGranularity(Granularity::Panes).into()),
                    state: state.control(Control::Granularity(Granularity::Panes)),
                },
                Segment {
                    label: "Tabs",
                    selected: options.granularity == Granularity::Tabs,
                    command: Some(OptionsAction::SetGranularity(Granularity::Tabs).into()),
                    state: state.control(Control::Granularity(Granularity::Tabs)),
                },
            ],
            ui,
        ),
        ui,
    );

    let density = widgets::row(
        "Density",
        Some("How much of a row is text. Only an expanded row carries chips."),
        true,
        widgets::segmented(
            vec![
                Segment {
                    label: "Compact",
                    selected: options.density == Density::Compact,
                    command: Some(OptionsAction::SetDensity(Density::Compact).into()),
                    state: state.control(Control::Density(Density::Compact)),
                },
                Segment {
                    label: "Expanded",
                    selected: options.density == Density::Expanded,
                    command: Some(OptionsAction::SetDensity(Density::Expanded).into()),
                    state: state.control(Control::Density(Density::Expanded)),
                },
            ],
            ui,
        ),
        ui,
    );

    page(vec![
        widgets::category("Tabs", true, vec![placement, granularity, density], ui),
        widgets::category("Rows", false, rows_category(workspace), ui),
    ])
}

/// The "Rows" category: which fact goes on which line, and which chips a row
/// carries.
fn rows_category(workspace: &Workspace) -> Vec<Box<dyn Element>> {
    let options = workspace.options();
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();

    // The two conditions the gear menu resolves by hiding controls. The page
    // greys them instead, and this is the whole of the difference: one bool
    // read twice, rather than two branches that build different pages.
    let expanded = options.density == Density::Expanded;

    let mut rows = vec![widgets::choice_group(
        "Pane title as",
        Some("Which fact a row leads with. The others fill the lines below it."),
        true,
        [
            PrimaryInfo::Command,
            PrimaryInfo::WorkingDirectory,
            PrimaryInfo::Branch,
        ]
        .into_iter()
        .map(|primary| {
            widgets::choice(
                primary.label(),
                options.primary_info == primary,
                Some(OptionsAction::SetPrimaryInfo(primary).into()),
                state.control(Control::PrimaryInfo(primary)),
                ui,
            )
        })
        .collect(),
        ui,
    )];

    // Against the *resolved* subtitle, so the check is never beside an option
    // the strip is silently overriding — the same read the gear menu does, and
    // for the same reason.
    let chosen = resolve_subtitle(options.primary_info, options.subtitle);
    rows.push(widgets::choice_group(
        "Additional metadata",
        Some("What a compact row's second line says. An expanded row chooses for itself."),
        !expanded,
        subtitle_options_for(options.primary_info)
            .into_iter()
            .map(|subtitle| {
                widgets::choice(
                    subtitle.label(),
                    chosen == subtitle,
                    (!expanded).then(|| OptionsAction::SetSubtitle(subtitle).into()),
                    state.control(Control::Subtitle(subtitle)),
                    ui,
                )
            })
            .collect(),
        ui,
    ));

    rows.push(widgets::row(
        "Show the PR link chip",
        Some("Crook has no forge integration yet, so no session has a link to show."),
        expanded,
        widgets::switch(
            options.show_pr_link,
            expanded.then_some(OptionsAction::ToggleShowPrLink.into()),
            state.control(Control::ShowPrLink),
        ),
        ui,
    ));

    rows.push(widgets::row(
        "Show the diff stats chip",
        Some("Added and removed lines in the row's repository. Expanded rows only."),
        expanded,
        widgets::switch(
            options.show_diff_stats,
            expanded.then_some(OptionsAction::ToggleShowDiffStats.into()),
            state.control(Control::ShowDiffStats),
        ),
        ui,
    ));

    rows.push(widgets::row(
        "Show details on hover",
        Some("Opens a card beside a row with everything the row had no space for."),
        true,
        widgets::switch(
            options.show_details_on_hover,
            Some(OptionsAction::ToggleShowDetailsOnHover.into()),
            state.control(Control::ShowDetailsOnHover),
        ),
        ui,
    ));

    // Warp's reset button, and Warp's trick with it: it is drawn dead and does
    // nothing while there is nothing to reset, which makes it the page's only
    // indication that anything on it has been changed from the default.
    let changed = options != TabOptions::default();
    rows.push(widgets::row(
        "Tab options",
        Some("Every option on this page, back to what a fresh install opens with."),
        changed,
        widgets::text_button(
            "Reset to defaults",
            changed.then_some(SettingsAction::ResetTabOptions.into()),
            state.control(Control::ResetTabOptions),
            ui,
        ),
        ui,
    ));

    rows
}

/// The usage chip, and what turning it off actually stops.
fn usage(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();
    let general = workspace.general();

    let chip = widgets::row(
        "Show the usage chip",
        Some("The pill in the header, showing how much of the session budget is spent."),
        true,
        widgets::switch(
            general.show_usage_chip,
            Some(SettingsAction::ToggleUsageChip.into()),
            state.control(Control::ShowUsageChip),
        ),
        ui,
    );

    let reading = workspace.usage().as_ref(app);
    let current = match (reading.snapshot(), reading.problem()) {
        (Some(snapshot), None) => format!(
            "{}% of the session budget",
            snapshot.session_percent_rounded()
        ),
        (_, Some(problem)) => problem.chip_label().to_owned(),
        (None, None) if general.show_usage_chip => "not read yet".to_owned(),
        (None, None) => "not being read".to_owned(),
    };

    page(vec![
        widgets::category("Claude Code", true, vec![chip], ui),
        widgets::category(
            "Session",
            false,
            vec![
                widgets::note(
                    "Crook reads the session Claude Code already stores on this machine and asks \
                     Anthropic what it has spent. Turning the chip off stops both: a hidden chip \
                     does not poll.",
                    ui,
                ),
                widgets::fact("Last reading", current, false, workspace.fonts()),
            ],
            ui,
        ),
    ])
}

/// The bindings, which are fixed.
fn keys(workspace: &Workspace) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let fonts = workspace.fonts();

    let binding = |label: &'static str, chord: String| widgets::fact(label, chord, true, fonts);

    page(vec![
        widgets::category(
            "Tabs and panes",
            true,
            vec![
                binding("New agent tab", chord("cmd-t", "ctrl-shift-t")),
                binding("Close the focused pane", chord("cmd-w", "ctrl-shift-w")),
                binding("Split to the right", chord("cmd-d", "ctrl-shift-d")),
                binding("Split downwards", chord("cmd-shift-d", "ctrl-shift-e")),
                binding(
                    "Previous / next tab",
                    chord("cmd-alt-left / right", "ctrl-pageup / pagedown"),
                ),
                binding(
                    "Move the active tab",
                    chord("cmd-ctrl-left / right", "ctrl-shift-pageup / pagedown"),
                ),
                widgets::note(
                    "Every chord here stays off the ones the field needs. On macOS that is why \
                     the tabs are on cmd-alt-arrow rather than cmd-shift-arrow, which selects to \
                     the end of a line; everywhere else it is why Crook's own chords carry a \
                     Shift, since a bare ctrl-letter belongs to the program in the pane.",
                    ui,
                ),
            ],
            ui,
        ),
        widgets::category(
            "The command field",
            false,
            vec![
                binding("Send the line to the shell", "enter".to_owned()),
                binding("Lengthen it by a line", "shift-enter".to_owned()),
                binding("Walk this pane's history", "up / down".to_owned()),
                widgets::note(
                    "Everything else in the field is the text editing this platform already \
                     does. ctrl-c interrupts the shell and throws the half-written line away \
                     with it, ctrl-z suspends, and ctrl-d ends the input when the field is empty \
                     and deletes a character when it is not. A full-screen program — vim, `top` \
                     — takes every key back and the field goes away while it runs.",
                    ui,
                ),
            ],
            ui,
        ),
        widgets::category(
            "Window",
            false,
            vec![
                binding("Move the tabs panel", chord("cmd-b", "ctrl-shift-b")),
                binding("Open these settings", chord("cmd-,", "ctrl-,")),
                widgets::note(
                    "These settings are a pane, like a session is, so they close the way every \
                     pane does and have no key of their own for it. Pressing the binding again \
                     brings this tab forward rather than closing it.",
                    ui,
                ),
                widgets::note(
                    "The bindings are fixed. Warp has editable keymaps with context predicates; \
                     Crook reads input directly, and the keyboard produces exactly the values the \
                     mouse produces — which is what lets a keymap be added later without touching \
                     a single handler.",
                    ui,
                ),
            ],
            ui,
        ),
    ])
}

/// What this build is, where it keeps its file, and who owns what in it.
fn about(workspace: &Workspace) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let fonts = workspace.fonts();

    let file = match workspace.settings().path() {
        Some(path) => path.display().to_string(),
        // Either a run with ephemeral settings — a snapshot, a test — or a
        // machine with no configuration directory at all. The options still
        // work in both cases; they just do not outlive the process, and that
        // is worth saying in the one place somebody would come looking.
        None => "nowhere — this run keeps its options in memory".to_owned(),
    };

    page(vec![
        widgets::category(
            "Build",
            true,
            vec![
                widgets::fact(
                    "Version",
                    env!("CARGO_PKG_VERSION").to_owned(),
                    false,
                    fonts,
                ),
                widgets::fact("Channel", workspace.channel().to_owned(), false, fonts),
                widgets::fact("Settings file", file, true, fonts),
            ],
            ui,
        ),
        widgets::category(
            "Licence",
            false,
            vec![
                widgets::note(
                    "Crook is open source under the MIT licence, in full and with no exceptions.",
                    ui,
                ),
                widgets::note(
                    "Its UI framework — the crookui and crookui_core crates — is derived from the \
                     warpui and warpui_core crates of Warp, which Denver Technologies publishes \
                     under the MIT licence. The rest of Warp is AGPL and none of it is here: what \
                     Crook took from those parts is architecture, read and rewritten, which is \
                     why this page can say MIT and mean it.",
                    ui,
                ),
            ],
            ui,
        ),
    ])
}
