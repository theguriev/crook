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

use std::collections::HashMap;

use crookui_core::prelude::*;

use super::super::action::{OptionsAction, SettingsAction, ThemeAction};
use super::super::theme_preview;
use super::super::view::Workspace;
use super::search::Words;
use super::widgets::{self, Segment};
use super::widgets::{Category, Entry};
use super::{Control, Section};
use crate::input_keys::Platform;
use crate::keymap::{self, Bound};
use crate::settings::{
    Density, FONT_SIZE_STEP, Granularity, Layout, PrimaryInfo, TabOptions, resolve_subtitle,
    subtitle_options_for,
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

/// One page, as the categories it is made of.
///
/// Built rather than drawn, because the search filters it and because the rail
/// counts it: what a page holds has to be a value the moment anything but the
/// page itself needs to ask a question about it. Every page is built on every
/// keystroke while a query is in force — four pages of about thirty rows, once
/// per frame, which is a rounding error beside the frame it is part of.
pub(super) fn of(workspace: &Workspace, section: Section, app: &AppContext) -> Vec<Category> {
    match section {
        Section::Appearance => appearance(workspace),
        Section::Shell => shell(workspace),
        Section::Usage => usage(workspace, app),
        Section::Keys => keys(workspace),
        Section::About => about(workspace),
    }
}

/// Where the tabs are and what a row of them looks like.
fn appearance(workspace: &Workspace) -> Vec<Category> {
    let options = workspace.options();
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();

    let placement = widgets::row(
        Words::new("Tab placement")
            .with_description("Where the list of what you are working on lives.")
            .with_keywords(&[
                "sidebar",
                "strip",
                "header",
                "vertical",
                "horizontal",
                "left",
            ]),
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
            state.control(Control::RestoreSession),
        ),
        ui,
    );

    vec![
        widgets::category("Theme", theme_category(workspace)),
        widgets::category("Text", text_category(workspace)),
        widgets::category("Tabs", vec![placement, granularity, density, restore]),
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
            state.control(Control::FontSmaller),
            step(FONT_SIZE_STEP),
            state.control(Control::FontBigger),
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
                state.control(Control::ThemeRow),
                fonts,
            ),
            workspace.theme_name().to_owned(),
            ThemeAction::OpenPanel.into(),
            state.control(Control::ThemeRowButton),
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
                state.control(Control::FollowSystemTheme),
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
                    state.control(Control::Subtitle(subtitle)),
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
            state.control(Control::ShowPrLink),
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
            state.control(Control::ShowDiffStats),
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
            state.control(Control::ShowDetailsOnHover),
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
            state.control(Control::ResetTabOptions),
            ui,
        ),
        ui,
    ));

    rows
}

/// How a pane's shell is started, which decides which of the person's own
/// files it reads.
///
/// One switch, and it is worth a page of its own rather than a row on another
/// because of what it changes: the answer to `which`, the version of every tool
/// on `PATH`, and whether the thing a person's `~/.zprofile` prints appears at
/// all. The note is not decoration — a switch called "login shell" means
/// nothing to most people, and the sentence under it is the setting.
///
/// Which way it starts is the desktop's answer rather than Crook's, so the note
/// names both directions rather than assuming the macOS one: see
/// `shell_integration::login_by_default`.
fn shell(workspace: &Workspace) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();

    let login = widgets::row(
        Words::new("Start a login shell")
            .with_description(
                "Read the files a login gives you — ~/.zprofile, ~/.bash_profile, /etc/profile — \
                 the way your desktop's own terminal does.",
            )
            .with_keywords(&[
                "login",
                "shell",
                "profile",
                "zprofile",
                "zlogin",
                "bash_profile",
                "path",
                "environment",
                "dotfiles",
                "startup",
                "zsh",
                "bash",
                "fish",
            ]),
        true,
        widgets::switch(
            workspace.general().login_shell,
            Some(SettingsAction::ToggleLoginShell.into()),
            state.control(Control::LoginShell),
        ),
        ui,
    );

    vec![
        widgets::category("Startup", vec![login]),
        widgets::category(
            "What it changes",
            vec![widgets::note(
                "Your PATH is assembled by those files, so a shell that skips them finds \
                 different tools than the terminal beside it. It starts on where every macOS \
                 terminal starts a login shell, and off on Linux where GNOME Terminal and Konsole \
                 do not and your PATH is more likely to be in ~/.bashrc. Turn it off if a profile \
                 of yours expects to run once when you log in rather than once per pane; turn it \
                 on if ~/.profile is where your PATH lives. Either way it applies to the next \
                 shell opened, not the ones already running.",
                ui,
            )],
        ),
    ]
}

/// The usage chip, and what turning it off actually stops.
fn usage(workspace: &Workspace, app: &AppContext) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();
    let general = workspace.general();

    let chip = widgets::row(
        Words::new("Show the usage chip")
            .with_description(
                "The pill in the header, showing how much of the session budget is spent.",
            )
            .with_keywords(&[
                "usage", "token", "budget", "claude", "limit", "quota", "network", "poll", "pill",
            ]),
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

    vec![
        widgets::category("Claude Code", vec![chip]),
        widgets::category(
            "Session",
            vec![
                widgets::note(
                    "Crook reads the session Claude Code already stores on this machine and asks \
                     Anthropic what it has spent. Turning the chip off stops both: a hidden chip \
                     does not poll.",
                    ui,
                ),
                widgets::fact(
                    Words::new("Last reading")
                        .with_keywords(&["usage", "percent", "spent", "session", "poll"]),
                    current,
                    false,
                    workspace.fonts(),
                ),
            ],
        ),
    ]
}

/// Everything reachable by name, and the chord that reaches it.
///
/// The one part of this page that is not written down in this file. A named
/// action is registered by a plugin, so what is listed here depends on which
/// plugins are loaded — which is the whole point of a name: the page can print
/// a row for something it has never heard of, and a person can bind a chord to
/// it without anybody adding a variant to an enum.
fn named_actions(workspace: &Workspace) -> Category {
    let fonts = workspace.fonts();
    let keymap = workspace.keymap();

    // Which chord, if any, reaches each name. A name may be bound more than
    // once; all of them are printed, because a person looking for "why does
    // this fire" needs to see the second one.
    let mut bound: HashMap<String, Vec<String>> = HashMap::new();
    for (keystroke, action) in keymap::bindings(keymap) {
        if let Some(Bound::Named(name)) = action {
            bound
                .entry(name.as_str().to_owned())
                .or_default()
                .push(keymap::format_chord(&keystroke));
        }
    }
    // A stable order, since the map's is not one.
    for chords in bound.values_mut() {
        chords.sort();
    }

    let mut entries: Vec<Entry> = workspace
        .host()
        .actions()
        .names()
        .into_iter()
        .map(|(action, owner)| {
            let chords = match bound.get(action.as_str()) {
                Some(chords) => chords.join(", "),
                None => "not bound".to_owned(),
            };
            widgets::fact(
                Words::new(action.to_string())
                    .with_description(format!("From {owner}."))
                    .with_keywords(&["plugin", "action", "bind", "keymap"]),
                chords,
                true,
                fonts,
            )
        })
        .collect();

    entries.push(widgets::note(
        "Bind one by putting its name in keymap.json beside a chord — \
         \"cmd-shift-u\": \"crook/usage/refresh\". A name no loaded plugin answers to is \
         a chord that does nothing, so a keymap written for a plugin you have not installed \
         costs you nothing but that one chord.",
        fonts.ui,
    ));

    widgets::category("Named actions", entries)
}

/// The bindings, which are fixed.
fn keys(workspace: &Workspace) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let fonts = workspace.fonts();

    // The chord is the value, which is what makes it searchable: somebody who
    // remembers the key and not the name of what it does types the key.
    let binding = |words: Words, chord: String| widgets::fact(words, chord, true, fonts);
    let key = Words::new;

    vec![
        widgets::category(
            "Tabs and panes",
            vec![
                binding(
                    key("New agent tab").with_keywords(&["open", "create", "another"]),
                    chord("cmd-t", "ctrl-shift-t"),
                ),
                binding(
                    key("Close the focused pane").with_keywords(&["quit", "exit", "kill"]),
                    chord("cmd-w", "ctrl-shift-w"),
                ),
                binding(
                    key("Split to the right").with_keywords(&["vertical", "side", "beside"]),
                    chord("cmd-d", "ctrl-shift-d"),
                ),
                binding(
                    key("Split downwards").with_keywords(&["horizontal", "below", "under"]),
                    chord("cmd-shift-d", "ctrl-shift-e"),
                ),
                binding(
                    key("Previous / next tab").with_keywords(&["switch", "cycle", "between"]),
                    chord("cmd-alt-left / right", "ctrl-pageup / pagedown"),
                ),
                binding(
                    key("Move the active tab").with_keywords(&["reorder", "position"]),
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
        ),
        widgets::category(
            "Command blocks",
            vec![
                binding(
                    key("Copy a whole command and its output").with_keywords(&[
                        "clipboard",
                        "block",
                        "yank",
                    ]),
                    "hover it, then click".to_owned(),
                ),
                binding(
                    key("Move through the commands").with_keywords(&["scroll", "block", "wheel"]),
                    "wheel".to_owned(),
                ),
                widgets::note(
                    "A pane's output is a list of commands, and that needs the shell to say \
                     where each one starts and ends. Crook installs the marks that do it into \
                     zsh, bash and fish by itself, without writing to any dotfile — there is \
                     nothing to set up here. It cannot reach a shell it did not start, so on \
                     the far side of an ssh, in a container, or under a shell it has no \
                     snippet for, a pane is a plain terminal instead: one continuous stream, \
                     no blocks, and no field — every key goes straight to the shell.",
                    ui,
                ),
                widgets::note(
                    "There is no keyboard binding here yet. Selecting a block, walking between \
                     them and jumping to the bottom are all still to come; what a block can be \
                     asked for today is its own text, exactly, with one click.",
                    ui,
                ),
            ],
        ),
        widgets::category(
            "Selecting the output",
            vec![
                binding(
                    key("Select a run of text").with_keywords(&["mouse", "drag", "highlight"]),
                    "drag".to_owned(),
                ),
                binding(
                    key("Select a word / a whole line")
                        .with_keywords(&["mouse", "double", "triple", "click"]),
                    "double / triple click".to_owned(),
                ),
                binding(
                    key("Select a column of it").with_keywords(&[
                        "block",
                        "rectangular",
                        "alt",
                        "option",
                    ]),
                    "alt-drag".to_owned(),
                ),
                binding(
                    key("Copy what is selected").with_keywords(&["clipboard", "yank"]),
                    chord("cmd-c", "ctrl-c or ctrl-shift-c"),
                ),
                widgets::note(
                    "A selection in the output owns the copy chord for as long as it exists, \
                     and copying lets go of it — which is the only sign a copy happened. That \
                     is what settles ctrl-c off macOS, where the same key is also the \
                     interrupt: with nothing selected it interrupts exactly as it always has, \
                     and because copying releases the selection, the very next press does too.",
                    ui,
                ),
                widgets::note(
                    "The copy leaves the command field alone. A selection is also let go of by \
                     typing, by clicking into the field, and by clicking elsewhere in the \
                     output — but never by the shell printing, which is exactly when somebody \
                     is reading what is already on screen.",
                    ui,
                ),
                widgets::note(
                    "A selection lives in the command that is still running, and nowhere else. \
                     A finished command's rows have been copied out of the terminal — which is \
                     what makes them survive a clear, a resize and the scrollback filling up — \
                     and a selection has to stay in the terminal to stay anchored to its own \
                     text while output arrives. A press on a finished command lets go of \
                     whatever was selected rather than starting a new selection; its copy \
                     control takes the whole of it.",
                    ui,
                ),
            ],
        ),
        widgets::category(
            "The command field",
            vec![
                binding(
                    key("Send the line to the shell")
                        .with_keywords(&["run", "execute", "submit", "return"]),
                    "enter".to_owned(),
                ),
                binding(
                    key("Lengthen it by a line").with_keywords(&[
                        "multiline",
                        "newline",
                        "continue",
                    ]),
                    "shift-enter".to_owned(),
                ),
                binding(
                    key("Walk this pane's history").with_keywords(&["previous", "recall", "arrow"]),
                    "up / down".to_owned(),
                ),
                widgets::note(
                    "Everything else in the field is the text editing this platform already \
                     does. ctrl-c interrupts the shell and throws the half-written line away \
                     with it, ctrl-z suspends, and ctrl-d ends the input when the field is empty \
                     and deletes a character when it is not.",
                    ui,
                ),
                widgets::note(
                    "The field goes away, and every key reaches the program instead, in three \
                     cases: a full-screen program — vim, `top` — is up, the shell has reported \
                     a command running for longer than a blink, or the output is being drawn as \
                     a plain terminal because the shell reports no command boundaries at all.",
                    ui,
                ),
            ],
        ),
        widgets::category(
            "Window",
            vec![
                binding(
                    key("Move the tabs panel").with_keywords(&[
                        "sidebar",
                        "strip",
                        "placement",
                        "layout",
                    ]),
                    chord("cmd-b", "ctrl-shift-b"),
                ),
                binding(
                    key("Open these settings").with_keywords(&["preferences", "options", "config"]),
                    chord("cmd-,", "ctrl-,"),
                ),
                binding(
                    key("Make the text bigger / smaller")
                        .with_keywords(&["zoom", "font", "size", "scale"]),
                    chord("cmd-+ / cmd--", "ctrl-+ / ctrl--"),
                ),
                binding(
                    key("Put the text back to its size").with_keywords(&["zoom", "reset", "font"]),
                    chord("cmd-0", "ctrl-0"),
                ),
                widgets::note(
                    "These settings are a pane, like a session is, so they close the way every \
                     pane does and have no key of their own for it. Pressing the binding again \
                     brings this tab forward rather than closing it.",
                    ui,
                ),
                widgets::fact(
                    Words::new("Keymap file").with_keywords(&[
                        "bindings",
                        "shortcuts",
                        "chords",
                        "rebind",
                        "keymap",
                    ]),
                    crate::keymap::user_keymap_path()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| {
                            "nowhere — this machine has no configuration directory".to_owned()
                        }),
                    true,
                    fonts,
                ),
                widgets::note(
                    "A chord in that file wins over the one above it, and \"none\" takes a chord \
                     away — which is how one is given back to a shell or an editor that wants \
                     it. It is read when Crook starts. What a pane does with a key is not in it: \
                     ctrl-c interrupts and ctrl-d ends an input, and a keymap that could take \
                     one of those away would be one that breaks a terminal.",
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
        ),
        named_actions(workspace),
    ]
}

/// What this build is, where it keeps its file, and who owns what in it.
fn about(workspace: &Workspace) -> Vec<Category> {
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

    vec![
        widgets::category(
            "Build",
            vec![
                widgets::fact(
                    Words::new("Version").with_keywords(&["build", "release", "crook"]),
                    env!("CARGO_PKG_VERSION").to_owned(),
                    false,
                    fonts,
                ),
                widgets::fact(
                    Words::new("Channel").with_keywords(&["dev", "stable", "build"]),
                    workspace.channel().to_owned(),
                    false,
                    fonts,
                ),
                widgets::fact(
                    Words::new("Settings file").with_keywords(&[
                        "json",
                        "config",
                        "path",
                        "where",
                        "folder",
                        "directory",
                    ]),
                    file,
                    true,
                    fonts,
                ),
            ],
        ),
        widgets::category(
            "Licence",
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
                widgets::note(
                    "The bundled palettes named after Catppuccin, Everforest, Gruvbox, \
                     Kanagawa, Nord, Rosé Pine and Tokyo Night belong to those projects, each \
                     under its own licence. Their values were read from the theme files \
                     Omarchy ships, which is MIT, from Basecamp; Matte Black and Osaka Jade \
                     are Omarchy's own.",
                    ui,
                ),
            ],
        ),
    ]
}
