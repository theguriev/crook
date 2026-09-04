//! The Appearance page: where the tabs are, and what a row of them looks like.
//!
//! Warp's Appearance page carries eleven categories; the two of them that
//! describe a tab strip are the two here, plus the text size and the theme.
//!
//! # Every control writes through the action the gear menu dispatches
//!
//! So the popup and the page cannot disagree about what an option means or
//! about when it is saved. The two actions that are the page's own are the
//! ones the popup has no control for.

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::settings::{
    Density, FONT_SIZE_STEP, Granularity, PrimaryInfo, TabOptions, resolve_subtitle,
    subtitle_options_for,
};
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category, Entry, Segment};
use crate::workspace::settings_page::{keyed, named};
use crate::workspace::{OptionsAction, SettingsAction, ThemeAction, Workspace, theme_preview};

/// The plugin that owns the Appearance page.
pub struct Appearance;

impl Plugin for Appearance {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.add_settings_page("page", "Appearance", 0, |workspace, _| {
            appearance(workspace)
        });
        Ok(())
    }
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/appearance").expect("a literal that parses"),
        name: "Appearance settings",
        description: "Where the tabs live, what a row says, the text size and the theme.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
    })
}

/// Where the tabs are and what a row of them looks like.
fn appearance(workspace: &Workspace) -> Vec<Category> {
    let options = workspace.options();
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();

    let granularity = widgets::row(
        Words::new("View as")
            .with_description("Whether one row stands for a single pane or for a whole tab.")
            .with_keywords(&["granularity", "split", "group", "one"]),
        true,
        widgets::segmented(
            vec![
                Segment {
                    label: "Panes",
                    selected: options.granularity == Granularity::Panes,
                    command: Some(OptionsAction::SetGranularity(Granularity::Panes).into()),
                    state: state.control(keyed("granularity", Granularity::Panes)),
                },
                Segment {
                    label: "Tabs",
                    selected: options.granularity == Granularity::Tabs,
                    command: Some(OptionsAction::SetGranularity(Granularity::Tabs).into()),
                    state: state.control(keyed("granularity", Granularity::Tabs)),
                },
            ],
            ui,
        ),
        ui,
    );

    let density = widgets::row(
        Words::new("Density")
            .with_description("How much of a row is text. Only an expanded row carries chips.")
            .with_keywords(&["compact", "expanded", "spacing", "height", "size", "small"]),
        true,
        widgets::segmented(
            vec![
                Segment {
                    label: "Compact",
                    selected: options.density == Density::Compact,
                    command: Some(OptionsAction::SetDensity(Density::Compact).into()),
                    state: state.control(keyed("density", Density::Compact)),
                },
                Segment {
                    label: "Expanded",
                    selected: options.density == Density::Expanded,
                    command: Some(OptionsAction::SetDensity(Density::Expanded).into()),
                    state: state.control(keyed("density", Density::Expanded)),
                },
            ],
            ui,
        ),
        ui,
    );

    let restore = widgets::row(
        Words::new("Bring the tabs back")
            .with_description(
                "Open with the tabs and splits the last window had, in their own directories.",
            )
            .with_keywords(&["session", "restore", "reopen", "startup", "remember"]),
        true,
        widgets::switch(
            workspace.general().restore_session,
            Some(SettingsAction::ToggleRestoreSession.into()),
            state.control(named("restore-session")),
        ),
        ui,
    );

    vec![
        widgets::category("Theme", theme_category(workspace)),
        widgets::category("Text", text_category(workspace)),
        widgets::category("Tabs", vec![granularity, density, restore]),
        widgets::category("Rows", rows_category(workspace)),
    ]
}
/// The "Text" category: how big the terminal's own type is, and which family
/// it is set in.
///
/// The size is a control; the family is a fact. Changing a family means
/// re-selecting four faces, re-measuring the cell and resizing every pty in
/// the window against a list of what is installed — and there is no element to
/// choose from such a list with, because `crookui_core` has no text field and
/// no combo box. So the family is set in the settings file, under
/// `font_family`, and this row says which one answered.
fn text_category(workspace: &Workspace) -> Vec<Entry> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();
    let general = workspace.general();
    let size = general.font_size();

    // `None` at each end of the range, which draws the button de-emphasised
    // and inert — the same "there is nothing to do here" the reset button has.
    let step = |by: f32| {
        let next = general.zoomed(by);
        (next != size).then(|| SettingsAction::SetFontSize(next).into())
    };

    let size_row = widgets::row(
        Words::new("Text size")
            .with_description("How big the terminal's own text is. Every pane resizes with it.")
            .with_keywords(&["font", "zoom", "bigger", "smaller", "scale", "type"]),
        true,
        widgets::stepper(
            format!("{size}"),
            step(-FONT_SIZE_STEP),
            state.control(named("font-smaller")),
            step(FONT_SIZE_STEP),
            state.control(named("font-bigger")),
            ui,
        ),
        ui,
    );

    // What the settings file asked for, which is not quite the same as what
    // answered: a family is resolved once, at startup, and a name nothing on
    // this machine answers to falls back with a line in the log. The note
    // under the row says so rather than this row pretending to know.
    let chosen = workspace
        .settings()
        .font_family()
        .map_or_else(|| "System default".to_owned(), str::to_owned);

    vec![
        size_row,
        widgets::fact(
            Words::new("Font").with_keywords(&["family", "typeface", "monospace", "font_family"]),
            chosen,
            false,
            workspace.fonts(),
        ),
        widgets::note(
            "The font is whichever monospace family this machine calls its default, unless the \
             settings file names another under \"font_family\". It is read once, when Crook \
             starts, and a name nothing answers to is a line in the log and the default.",
            ui,
        ),
    ]
}
/// The "Theme" category: which theme is in force, and the way to another.
///
/// Warp's shape exactly. The settings page does not list themes — it shows
/// *the* theme, as a live preview beside its name, and clicking the row opens
/// the Themes panel. The reason is worth keeping: a list of themes on a
/// settings page is a list you look at instead of your work, and the panel
/// exists so that a theme is judged against a running shell rather than
/// against a card.
fn theme_category(workspace: &Workspace) -> Vec<Entry> {
    let ui = workspace.fonts().ui;
    let fonts = workspace.fonts();
    let state = workspace.settings_page();

    vec![
        widgets::current_theme_row(
            Words::new("Current theme")
                .with_description("Choose another, or make one")
                .with_keywords(&[
                    "colour",
                    "color",
                    "palette",
                    "dark",
                    "light",
                    "scheme",
                    "appearance",
                ]),
            theme_preview::card(
                crate::theme::theme(),
                theme_preview::ROW_CARD,
                false,
                None,
                state.control(named("theme-row")),
                fonts,
            ),
            workspace.theme_name().to_owned(),
            ThemeAction::OpenPanel.into(),
            state.control(named("theme-row-button")),
            ui,
        ),
        widgets::row(
            Words::new("Follow the desktop")
                .with_description(
                    "Use one theme while the desktop is light and another while it is dark.",
                )
                .with_keywords(&["system", "light", "dark", "auto", "appearance", "os"]),
            true,
            widgets::switch(
                workspace.general().use_system_theme,
                Some(SettingsAction::ToggleFollowSystemTheme.into()),
                state.control(named("follow-system-theme")),
            ),
            ui,
        ),
        widgets::fact(
            Words::new("Light / dark").with_keywords(&["pair", "system", "theme"]),
            format!(
                "{} / {}",
                workspace.settings().light_theme(),
                workspace.settings().dark_theme()
            ),
            false,
            fonts,
        ),
        widgets::note(
            "While the desktop is being followed, choosing a theme sets the half it is currently \
             in, so the other one is left as it was. The pair is remembered whether or not the \
             switch is on.",
            ui,
        ),
        widgets::note(
            "Themes are read from your themes folder in Warp's own file format, so a theme \
             written for Warp works here unchanged. Drop a .yaml in and open the panel again.",
            ui,
        ),
        widgets::fact(
            Words::new("Themes folder").with_keywords(&[
                "yaml",
                "warp",
                "import",
                "custom",
                "directory",
                "path",
                "file",
            ]),
            crate::theme::user_themes_directory()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| {
                    "nowhere — this machine has no configuration directory".to_owned()
                }),
            true,
            fonts,
        ),
    ]
}
/// The "Rows" category: which fact goes on which line, and which chips a row
/// carries.
fn rows_category(workspace: &Workspace) -> Vec<Entry> {
    let options = workspace.options();
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();

    // The two conditions the gear menu resolves by hiding controls. The page
    // greys them instead, and this is the whole of the difference: one bool
    // read twice, rather than two branches that build different pages.
    let expanded = options.density == Density::Expanded;

    let mut rows = vec![widgets::choice_group(
        Words::new("Pane title as")
            .with_description("Which fact a row leads with. The others fill the lines below it.")
            .with_keywords(&["title", "name", "command", "directory", "branch", "leading"]),
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
                state.control(keyed("primary-info", primary)),
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
        Words::new("Additional metadata")
            .with_description(
                "What a compact row's second line says. An expanded row chooses for itself.",
            )
            .with_keywords(&["subtitle", "second", "line", "under", "branch", "directory"]),
        !expanded,
        subtitle_options_for(options.primary_info)
            .into_iter()
            .map(|subtitle| {
                widgets::choice(
                    subtitle.label(),
                    chosen == subtitle,
                    (!expanded).then(|| OptionsAction::SetSubtitle(subtitle).into()),
                    state.control(keyed("subtitle", subtitle)),
                    ui,
                )
            })
            .collect(),
        ui,
    ));

    rows.push(widgets::row(
        Words::new("Show the PR link chip")
            .with_description(
                "Crook has no forge integration yet, so no session has a link to show.",
            )
            .with_keywords(&["pull", "request", "github", "forge", "link", "chip"]),
        expanded,
        widgets::switch(
            options.show_pr_link,
            expanded.then_some(OptionsAction::ToggleShowPrLink.into()),
            state.control(named("show-pr-link")),
        ),
        ui,
    ));

    rows.push(widgets::row(
        Words::new("Show the diff stats chip")
            .with_description(
                "Added and removed lines in the row's repository. Expanded rows only.",
            )
            .with_keywords(&["diff", "stats", "added", "removed", "lines", "git", "chip"]),
        expanded,
        widgets::switch(
            options.show_diff_stats,
            expanded.then_some(OptionsAction::ToggleShowDiffStats.into()),
            state.control(named("show-diff-stats")),
        ),
        ui,
    ));

    rows.push(widgets::row(
        Words::new("Show details on hover")
            .with_description("Opens a card beside a row with everything the row had no space for.")
            .with_keywords(&["hover", "card", "detail", "tooltip", "popup", "preview"]),
        true,
        widgets::switch(
            options.show_details_on_hover,
            Some(OptionsAction::ToggleShowDetailsOnHover.into()),
            state.control(named("show-details-on-hover")),
        ),
        ui,
    ));

    // Warp's reset button, and Warp's trick with it: it is drawn dead and does
    // nothing while there is nothing to reset, which makes it the page's only
    // indication that anything on it has been changed from the default.
    let changed = options != TabOptions::default();
    rows.push(widgets::row(
        Words::new("Tab options")
            .with_description("Every option on this page, back to what a fresh install opens with.")
            .with_keywords(&["reset", "default", "restore", "undo", "revert"]),
        changed,
        widgets::text_button(
            "Reset to defaults",
            changed.then_some(SettingsAction::ResetTabOptions.into()),
            state.control(named("reset-tab-options")),
            ui,
        ),
        ui,
    ));

    rows
}
