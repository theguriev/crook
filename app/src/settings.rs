//! Everything Crook remembers between launches, and the file it remembers it
//! in.
//!
//! Two groups. [`TabOptions`] is what the tab list's options menu writes — what
//! a row stands for, how tall it is, what its title says, what its second line
//! says, which chips it carries, and whether hovering it opens a detail card.
//! Its defaults are Warp's, value for value, because the menu is Warp's: a
//! Crook that opened with different ones would be a different feature wearing
//! the same labels. [`GeneralOptions`] is what is left over once the tab strip
//! has had its say, and today that is one switch.
//!
//! Both are written by the settings page, and [`TabOptions`] is also written
//! by that menu. Neither knows which of the two changed it: a settings
//! page that had its own copy of an option would be a second source of truth
//! for the same key, and the menu and the page would disagree about what the
//! file says the moment both were open.
//!
//! # Why JSON, and why one file
//!
//! Warp stores these in the user's TOML under `appearance.vertical_tabs.*`,
//! reached through a settings-schema system that gives every key a type, a
//! default, a migration path and a cloud-sync policy. Crook has none of that,
//! and seven keys do not earn it. JSON is what `serde_json` — already in the
//! workspace for a plugin's manifest — reads and writes with no further
//! dependency and no schema, and it is the format in which "keep the keys this
//! build did not recognise" is a [`Map`] rather than a parser. The leaf key
//! names are Warp's, so the two files say the same thing about the same
//! options.
//!
//! # Nothing here can cost a person their window
//!
//! Every way this can go wrong — no configuration directory, no file, a file
//! that cannot be read, JSON that is not an object, an option whose value this
//! build has never heard of — resolves to the default for whatever could not
//! be read, logs one line, and carries on. A truncated config file is a
//! nuisance, not a startup failure.

use std::ffi::OsStr;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The directory Crook keeps its per-user files in, inside the platform's
/// configuration directory.
const CONFIG_DIRECTORY: &str = "crook";

/// The one file in it.
const SETTINGS_FILE: &str = "settings.json";

/// The key the chosen theme's name is stored under.
const THEME_KEY: &str = "theme";

/// The key the terminal's monospace family is stored under.
const FONT_FAMILY_KEY: &str = "font_family";

/// The key the list of switched-off plugins is stored under.
const DISABLED_PLUGINS_KEY: &str = "disabled_plugins";

/// The keys the two halves of the desktop-following pair are stored under.
const LIGHT_THEME_KEY: &str = "light_theme";
/// See [`LIGHT_THEME_KEY`].
const DARK_THEME_KEY: &str = "dark_theme";

/// One theme name out of a settings document, falling back to `default`.
///
/// Not a parse: a name that answers to nothing on this machine is a theme file
/// somebody deleted, not a broken setting, so the name is kept and whoever
/// applies it decides. An empty string names nothing and is treated as absent.
fn named_theme(document: &Map<String, Value>, key: &str, default: &str) -> String {
    document
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(default)
        .to_owned()
}

/// What one row of the tab strip stands for — the menu's "View as".
///
/// Warp's `VerticalTabsDisplayGranularity`, under
/// `appearance.vertical_tabs.display_granularity`.
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    /// A row per pane: a tab split several ways contributes several rows, and
    /// the tab itself becomes the container around them.
    #[default]
    Panes,
    /// A row per tab, showing its focused pane. The other panes are not listed
    /// at all — no count, no expander — and the row silently re-targets as
    /// focus moves inside the tab.
    Tabs,
}

/// How much of a row is text — the menu's "Density".
///
/// Warp's `VerticalTabsViewMode`, under `appearance.vertical_tabs.view_mode`.
/// What it changes is the number of lines and whether the metadata chips exist
/// at all; the padding and the leading icon are the same size either way.
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// A title and an optional subtitle. No chips, which is why
    /// [`TabOptions::show_pr_link`] and [`TabOptions::show_diff_stats`] are
    /// inert here and the menu hides them.
    #[default]
    Compact,
    /// A title, a description line, and a fixed-height metadata line carrying
    /// the branch and the chips.
    Expanded,
}

/// Which fact a row leads with — the menu's "Pane title as".
///
/// Warp's `VerticalTabsPrimaryInfo`, under
/// `appearance.vertical_tabs.primary_info`. Whichever of the three this names
/// takes the title line, and the remaining two fill the lines below it.
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimaryInfo {
    /// What the session is doing: the agent's own title, or the last command.
    #[default]
    Command,
    /// The session's working directory.
    WorkingDirectory,
    /// The git branch it is on, falling back to the working directory when the
    /// directory is not a repository.
    Branch,
}

impl PrimaryInfo {
    /// What the menu calls this, verbatim from Warp.
    pub fn label(self) -> &'static str {
        match self {
            Self::Command => "Command / Conversation",
            Self::WorkingDirectory => "Working Directory",
            Self::Branch => "Branch",
        }
    }
}

/// What a `Compact` row's second line says — the menu's "Additional metadata".
///
/// Warp's `VerticalTabsCompactSubtitle`, under
/// `appearance.vertical_tabs.compact_subtitle`. It names the same three facts
/// [`PrimaryInfo`] does, because the second line is whichever of them the title
/// did not take — see [`resolve_subtitle`] for what happens when a stored value
/// collides with the title's.
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subtitle {
    /// The git branch, falling back to the working directory, falling back to
    /// nothing at all.
    #[default]
    Branch,
    /// The working directory, or nothing when there is none.
    WorkingDirectory,
    /// What the session is doing — the same text the title shows under
    /// [`PrimaryInfo::Command`].
    Command,
}

impl Subtitle {
    /// What the menu calls this, verbatim from Warp.
    pub fn label(self) -> &'static str {
        match self {
            Self::Branch => "Branch",
            Self::WorkingDirectory => "Working Directory",
            Self::Command => "Command / Conversation",
        }
    }
}

/// The subtitle a row actually shows, given what the title is showing.
///
/// Warp's `resolve_compact_subtitle`, and it is a *read-time* rule: a stored
/// subtitle that names the same fact as the title is replaced here and the
/// correction is never written back. So changing "Pane title as" can silently
/// change what the second line says, and the file on disk can legitimately
/// disagree with the screen. Normalising on write instead would lose the user's
/// choice the moment they moved the title through the colliding value and back.
pub fn resolve_subtitle(primary: PrimaryInfo, preference: Subtitle) -> Subtitle {
    let collides = matches!(
        (primary, preference),
        (PrimaryInfo::Command, Subtitle::Command)
            | (PrimaryInfo::WorkingDirectory, Subtitle::WorkingDirectory)
            | (PrimaryInfo::Branch, Subtitle::Branch)
    );

    if collides {
        default_subtitle(primary)
    } else {
        preference
    }
}

/// The subtitle to fall back to when the stored one collides with the title.
pub fn default_subtitle(primary: PrimaryInfo) -> Subtitle {
    match primary {
        PrimaryInfo::Command | PrimaryInfo::WorkingDirectory => Subtitle::Branch,
        PrimaryInfo::Branch => Subtitle::Command,
    }
}

/// The two subtitles the menu offers, in Warp's order.
///
/// Only ever the two facts the title is not already showing, which is why the
/// menu can never be made to print the same string twice in one row.
pub fn subtitle_options_for(primary: PrimaryInfo) -> [Subtitle; 2] {
    match primary {
        PrimaryInfo::Command => [Subtitle::Branch, Subtitle::WorkingDirectory],
        PrimaryInfo::WorkingDirectory => [Subtitle::Branch, Subtitle::Command],
        PrimaryInfo::Branch => [Subtitle::Command, Subtitle::WorkingDirectory],
    }
}

/// Everything the settings page writes that is not about the tab strip.
///
/// Four switches, and that is not an accident of scheduling. Crook has a
/// window and a tab strip; every option that could be offered about the strip
/// is already in [`TabOptions`], and what a plugin wants asked about itself
/// belongs to that plugin rather than here. Warp's settings hold roughly eight
/// hundred keys behind a schema system, a migration path and a cloud-sync
/// policy —
/// `docs/architecture.md` is explicit that a `serde` struct in a file is the
/// right answer until there are ten of them, and this is the second struct,
/// not the beginning of a schema.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralOptions {
    /// How big the terminal's own text is, in logical pixels.
    ///
    /// The grid, the composer and every measurement made of a cell come from
    /// this one number: a pane's columns and rows are the pane's box divided
    /// by a cell, so changing it resizes every pty in the window.
    ///
    /// Read through [`GeneralOptions::font_size`] rather than directly, which
    /// is what keeps a hand-edited `0` — or a `NaN`, which JSON cannot hold
    /// but a future writer could — out of a division.
    pub font_size: f32,
    /// Whether the theme follows the desktop's light or dark setting.
    ///
    /// Off by default, because a terminal with a chosen theme that changed
    /// colour at sunset without being asked would be a surprise. When it is
    /// on, [`Settings::light_theme`] and [`Settings::dark_theme`] name the two
    /// themes and the desktop chooses between them.
    ///
    /// The resolution is a pure function of these three facts, which is what
    /// makes it testable with no window and no desktop: see
    /// [`Settings::theme_for`].
    pub use_system_theme: bool,
    /// Whether a window comes back holding the tabs the last one had.
    ///
    /// On, because it is what makes a terminal a place rather than a fresh
    /// start every morning, and because what comes back is only the *shape* —
    /// tabs, splits and the directories their shells were in. No output is
    /// restored and no process is: see [`crate::session`].
    pub restore_session: bool,
    /// Whether a pane's shell is started as a *login* shell.
    ///
    /// A login shell reads `/etc/zprofile`, `~/.zprofile` and `~/.zlogin` on
    /// zsh and `/etc/profile` and `~/.bash_profile` on bash, and on macOS it is
    /// what runs `path_helper` — which is what builds `PATH` out of
    /// `/etc/paths` and `/etc/paths.d` at all. A terminal that gets this wrong
    /// shows a different `PATH`, and therefore different tools, than the
    /// terminal beside it on the same machine.
    ///
    /// Which way that points is the desktop's answer and not Crook's, so the
    /// default is per platform: see [`login_by_default`]. On macOS it is on,
    /// because every macOS terminal starts a login shell; on Linux it is off,
    /// because GNOME Terminal and Konsole do not and Linux configurations are
    /// written for that.
    ///
    /// Turn it on where the default is off if your `~/.zprofile` or
    /// `~/.bash_profile` is where your `PATH` lives. Turn it off where the
    /// default is on if a profile of yours prints, or measures, or takes two
    /// seconds, and was written on the understanding that it runs once when you
    /// log in rather than once per pane.
    ///
    /// Only shells opened after the change: a shell's startup files are read
    /// once, at startup, and cannot be read into one that is already running.
    ///
    /// [`login_by_default`]: crate::shell_integration::login_by_default
    pub login_shell: bool,
}

impl Default for GeneralOptions {
    /// The tabs coming back, because that is what makes a terminal a place,
    /// and the type size the body panel already printed its one monospace line
    /// at.
    fn default() -> Self {
        Self {
            font_size: DEFAULT_FONT_SIZE,
            use_system_theme: false,
            restore_session: true,
            login_shell: crate::shell_integration::login_by_default(),
        }
    }
}

/// The em size a terminal grid is set at by default, in logical pixels.
pub const DEFAULT_FONT_SIZE: f32 = 12.5;

/// The smallest and largest the terminal's text may be set to.
///
/// Below the floor a cell is smaller than the subpixel grid the renderer
/// positions glyphs on; above the ceiling a pane holds fewer columns than the
/// two the emulator will accept. Both ends are reachable by holding a zoom
/// chord down, so both have to be answers rather than accidents.
pub const MIN_FONT_SIZE: f32 = 6.;
/// See [`MIN_FONT_SIZE`].
pub const MAX_FONT_SIZE: f32 = 48.;

/// How much one press of the zoom chord changes the size, in logical pixels.
///
/// A whole pixel, because a cell's width is derived from it and a step that
/// did not change the cell width would be a keystroke that did nothing.
pub const FONT_SIZE_STEP: f32 = 1.;

impl GeneralOptions {
    /// The terminal's type size, as a number a cell can be divided by.
    ///
    /// Clamped rather than trusted: this file is meant to be hand-edited, and
    /// a `0` in it would be a division by zero in every grid measurement in
    /// the window. A value that is not a number at all falls back to the
    /// default, because there is nothing sensible to clamp it to.
    pub fn font_size(self) -> f32 {
        if !self.font_size.is_finite() {
            return DEFAULT_FONT_SIZE;
        }
        self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    }

    /// The same size, `step` logical pixels bigger — or smaller, for a
    /// negative step — and still within the bounds.
    pub fn zoomed(self, step: f32) -> f32 {
        (self.font_size() + step).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    }
}

/// Everything the tab strip's options menu writes.
///
/// [`Copy`], so a renderer reads a snapshot of the whole menu by value and no
/// row can be looking at a half-updated one. Serialised flat, under Warp's leaf
/// key names.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
// Every field independently falls back to `Default`, so a file that predates a
// key — or that a person hand-edited a key out of — loads the rest.
#[serde(default)]
pub struct TabOptions {
    /// "View as".
    #[serde(rename = "display_granularity")]
    pub granularity: Granularity,
    /// "Density".
    #[serde(rename = "view_mode")]
    pub density: Density,
    /// "Pane title as".
    pub primary_info: PrimaryInfo,
    /// "Additional metadata" — what a `Compact` row's second line says. Read
    /// through [`resolve_subtitle`], never directly, or a row will print the
    /// same fact twice.
    #[serde(rename = "compact_subtitle")]
    pub subtitle: Subtitle,
    /// "Show: PR link" — the chip linking to the pull request for the branch.
    pub show_pr_link: bool,
    /// "Show: Diff stats" — the chip counting added and removed lines.
    pub show_diff_stats: bool,
    /// "Show details on hover" — whether hovering a row opens its detail card.
    /// The only option the menu shows in every state.
    pub show_details_on_hover: bool,
}

impl Default for TabOptions {
    /// Warp's defaults — `Panes`, `Compact`, `Command`, `Branch`, and every
    /// "Show" on — plus the one that is not Warp's: [`Layout::Vertical`].
    ///
    /// The three booleans are why this is written out rather than derived —
    /// `bool`'s default is `false`, and Warp's is `true` for all three.
    fn default() -> Self {
        Self {
            granularity: Granularity::default(),
            density: Density::default(),
            primary_info: PrimaryInfo::default(),
            subtitle: Subtitle::default(),
            show_pr_link: true,
            show_diff_stats: true,
            show_details_on_hover: true,
        }
    }
}

/// Crook's settings, and the file they came from.
///
/// Cheap to clone — a path and a handful of JSON keys — because that is how a
/// save gets off the main thread: clone the settings, hand the clone to the
/// background pool, call [`Settings::save_blocking`] there.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Where [`Settings::save_blocking`] writes. `None` only when the platform
    /// admits to no configuration directory at all, which is a machine with no
    /// home directory rather than a machine that has never run Crook.
    path: Option<PathBuf>,
    /// The file exactly as it was read, including keys this build knows
    /// nothing about. A save writes this back with the keys this build owns
    /// overwritten, which is what carries a key that a newer Crook wrote
    /// through an older Crook's save instead of dropping it.
    document: Map<String, Value>,
    /// The options themselves, already resolved against the defaults.
    tab_options: TabOptions,
    /// The options that are not the tab strip's, resolved the same way.
    general: GeneralOptions,
    /// The theme to use while the desktop is light, when it is being
    /// followed.
    ///
    /// A name like [`Settings::theme`], and read the same way: one that
    /// answers to nothing on this machine is a warning and the default rather
    /// than a refusal to start.
    light_theme: String,
    /// The theme to use while the desktop is dark.
    dark_theme: String,
    /// Which plugins a person has switched off, by `owner/name`.
    ///
    /// Names rather than anything richer, and *only the ones that are off*: a
    /// build that gains a plugin has it on for everybody, which is what
    /// shipping a feature means, and a list of the ones that are on would go
    /// stale the moment it did. A name this build has never heard of is kept
    /// and ignored — it is a plugin that has been uninstalled or renamed, and
    /// dropping it would silently switch the feature back on for somebody who
    /// reinstalls it.
    disabled_plugins: Vec<String>,
    /// The name of the theme to open in.
    ///
    /// A name rather than the palette itself, and that is the whole design: a
    /// theme lives in the binary or in a file of its own, and the settings
    /// remember which one was chosen. A settings file that carried the colours
    /// would go stale the moment the theme file it was copied from changed,
    /// and a theme that had been deleted would go on being applied from a copy
    /// nobody could find.
    ///
    /// A [`String`], which is why it is here rather than in [`GeneralOptions`]:
    /// that one is `Copy`, and a renderer reads it dozens of times a frame.
    theme: String,
    /// The monospace family the terminal draws in, by name.
    ///
    /// `None` — and an absent key — means the platform's own default, which is
    /// what a machine with no preference should get and what every machine had
    /// before this key existed.
    ///
    /// Applied at startup and nowhere else. Changing a font family means
    /// re-selecting four faces, re-measuring the cell and resizing every pty
    /// in the window, and the family a name resolves to depends on what is
    /// installed — so a name that answers to nothing is a warning and the
    /// default rather than a window that fails to open.
    ///
    /// Here rather than in [`GeneralOptions`] for the same reason the theme
    /// is: that struct is `Copy`.
    font_family: Option<String>,
}

impl Settings {
    /// Reads the per-user settings file.
    ///
    /// Blocking, and deliberately so: it is one small file, read once during
    /// startup, before there is a window to stall.
    pub fn for_user() -> Self {
        let Some(path) = user_settings_path() else {
            log::warn!("no per-user configuration directory; tab options cannot be remembered");
            return Self {
                path: None,
                document: Map::new(),
                tab_options: TabOptions::default(),
                general: GeneralOptions::default(),
                disabled_plugins: Vec::new(),
                theme: crate::theme::DEFAULT_NAME.to_owned(),
                light_theme: crate::theme::DEFAULT_LIGHT_NAME.to_owned(),
                dark_theme: crate::theme::DEFAULT_NAME.to_owned(),
                font_family: None,
            };
        };

        Self::load(path)
    }

    /// Settings that live only as long as the process.
    ///
    /// Nothing to read and nowhere to write, which is what a test and a
    /// headless snapshot both want: a run that neither depends on the options
    /// of whoever started it nor changes them.
    pub fn ephemeral() -> Self {
        Self {
            path: None,
            document: Map::new(),
            tab_options: TabOptions::default(),
            general: GeneralOptions::default(),
            disabled_plugins: Vec::new(),
            theme: crate::theme::DEFAULT_NAME.to_owned(),
            light_theme: crate::theme::DEFAULT_LIGHT_NAME.to_owned(),
            dark_theme: crate::theme::DEFAULT_NAME.to_owned(),
            font_family: None,
        }
    }

    /// Reads the settings file at `path`, defaulting past anything unusable.
    ///
    /// Tests point this at a scratch directory; nothing else should need to.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let document = read_document(&path);

        // Parsed from a copy, so that a value this build cannot make sense of
        // still reaches `document` and survives the next save.
        //
        // Two independent parses of the same object rather than one parse of a
        // struct holding both, because the two groups fail independently: a
        // hand-edited `display_granularity` that names nothing must not take
        // the type size down with it.
        let tab_options = parse_group(&document, &path, "tab options");
        let general = parse_group(&document, &path, "general options");
        // Not through `parse_group`: a theme name is one string rather than a
        // group of typed options, and a file naming a theme this machine does
        // not have is not a parse failure — the name is kept, and the theme
        // falls back to the default until whatever wrote it is put back.
        let theme = document
            .get(THEME_KEY)
            .and_then(Value::as_str)
            .unwrap_or(crate::theme::DEFAULT_NAME)
            .to_owned();

        // Read the same way the theme is, and for the same reason: a family
        // name that answers to nothing on this machine is not a parse failure,
        // it is a font that has been uninstalled since the file was written.
        // An empty string is treated as absent — it names no family, and the
        // alternative is a warning on every launch for a key somebody cleared.
        let font_family = document
            .get(FONT_FAMILY_KEY)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);

        // Anything in the list that is not a string is dropped with the rest
        // of the line's meaning intact: one unusable entry costs that plugin's
        // switch and not the whole list.
        let disabled_plugins = document
            .get(DISABLED_PLUGINS_KEY)
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let light_theme = named_theme(&document, LIGHT_THEME_KEY, crate::theme::DEFAULT_LIGHT_NAME);
        let dark_theme = named_theme(&document, DARK_THEME_KEY, crate::theme::DEFAULT_NAME);

        Self {
            path: Some(path),
            document,
            tab_options,
            general,
            disabled_plugins,
            theme,
            light_theme,
            dark_theme,
            font_family,
        }
    }

    /// The theme to be in, given what the desktop is set to.
    ///
    /// **A pure function of three facts**, which is why it is here rather than
    /// in the workspace: the flag, the pair of names, and the desktop. It
    /// needs no per-theme metadata — nothing has to declare itself light or
    /// dark, and a theme can be either half of the pair — which is exactly the
    /// design Warp arrived at.
    pub fn theme_for(&self, dark: bool) -> &str {
        if !self.general.use_system_theme {
            return &self.theme;
        }
        if dark {
            &self.dark_theme
        } else {
            &self.light_theme
        }
    }

    /// The theme to follow the desktop into the light.
    pub fn light_theme(&self) -> &str {
        &self.light_theme
    }

    /// The theme to follow it into the dark.
    pub fn dark_theme(&self) -> &str {
        &self.dark_theme
    }

    /// Records which theme belongs to which half of the desktop's setting.
    ///
    /// Which of the two is written is decided by `dark` rather than by the
    /// caller, so that "the theme just chosen" and "the half of the pair the
    /// desktop is currently in" cannot come apart.
    pub fn set_system_theme(&mut self, dark: bool, name: impl Into<String>) {
        if dark {
            self.dark_theme = name.into();
        } else {
            self.light_theme = name.into();
        }
    }

    /// The monospace family the terminal draws in, or `None` for the
    /// platform's default.
    pub fn font_family(&self) -> Option<&str> {
        self.font_family.as_deref()
    }

    /// The file this instance reads and writes.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The options as they stand.
    pub fn tab_options(&self) -> TabOptions {
        self.tab_options
    }

    /// Replaces the options.
    ///
    /// Touches no file: schedule a [`Settings::save_blocking`] when the new
    /// value should outlive the process.
    pub fn set_tab_options(&mut self, tab_options: TabOptions) {
        self.tab_options = tab_options;
    }

    /// The name of the theme to open in.
    pub fn theme(&self) -> &str {
        &self.theme
    }

    /// Records which theme was chosen. Touches no file.
    pub fn set_theme(&mut self, name: impl Into<String>) {
        self.theme = name.into();
    }

    /// Which plugins are switched off, by `owner/name`.
    pub fn disabled_plugins(&self) -> &[String] {
        &self.disabled_plugins
    }

    /// Switches one on or off. Touches no file.
    pub fn set_plugin_disabled(&mut self, plugin: &str, disabled: bool) {
        self.disabled_plugins.retain(|name| name != plugin);
        if disabled {
            self.disabled_plugins.push(plugin.to_owned());
        }
        self.disabled_plugins.sort();
    }

    /// The options that are not the tab strip's.
    pub fn general(&self) -> GeneralOptions {
        self.general
    }

    /// Replaces them. Touches no file, exactly as
    /// [`Settings::set_tab_options`] does not.
    pub fn set_general(&mut self, general: GeneralOptions) {
        self.general = general;
    }

    /// Writes the settings to disk, atomically.
    ///
    /// Blocking, and more of it than it looks: this creates a directory,
    /// writes a file, waits for the filesystem to admit it has the bytes, and
    /// renames it — plus, on Windows, sleeps between rename attempts. That is
    /// milliseconds on a good day and a stalled frame on a bad one, so it
    /// belongs on the background pool:
    ///
    /// ```ignore
    /// let settings = self.settings.clone();
    /// ctx.background()
    ///     .spawn(async move {
    ///         if let Err(err) = settings.save_blocking() {
    ///             log::warn!("could not save the settings: {err:#}");
    ///         }
    ///     })
    ///     .detach();
    /// ```
    ///
    /// A crash part-way through leaves the previous file intact: the bytes go
    /// to a temporary beside the target and only a rename — one operation the
    /// filesystem either does or does not do — puts them in place.
    pub fn save_blocking(&self) -> Result<()> {
        let path = self
            .path
            .as_ref()
            .context("there is no per-user configuration directory to save into")?;

        let mut json = serde_json::to_string_pretty(&Value::Object(self.merged_document()?))
            .context("could not serialize the settings")?;
        // A file a person may open in an editor ends in a newline.
        json.push('\n');

        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)
                .with_context(|| format!("could not create {}", directory.display()))?;
        }

        write_then_rename(&temporary_path(path), path, json.as_bytes())
    }

    /// What to write: the file as it was read, with the keys this build owns
    /// overwritten by their current values.
    ///
    /// Both groups are flat in the same object, which is what lets a person
    /// find `start_login_shell` beside `font_size` in a file they opened in an
    /// editor. It also means the two structs may not name the same key twice —
    /// the later `extend` would silently win — and that is a thing to check
    /// when a third group appears rather than a thing to defend against here.
    fn merged_document(&self) -> Result<Map<String, Value>> {
        let mut document = self.document.clone();
        document.extend(owned_keys(self.tab_options, "tab options")?);
        document.extend(owned_keys(self.general, "general options")?);
        document.insert(THEME_KEY.to_owned(), Value::String(self.theme.clone()));
        // Written back only when something is off, so a person who has never
        // switched a plugin off does not find an empty array in a file they
        // opened to read.
        if self.disabled_plugins.is_empty() {
            document.remove(DISABLED_PLUGINS_KEY);
        } else {
            document.insert(
                DISABLED_PLUGINS_KEY.to_owned(),
                Value::Array(
                    self.disabled_plugins
                        .iter()
                        .map(|name| Value::String(name.clone()))
                        .collect(),
                ),
            );
        }
        document.insert(
            LIGHT_THEME_KEY.to_owned(),
            Value::String(self.light_theme.clone()),
        );
        document.insert(
            DARK_THEME_KEY.to_owned(),
            Value::String(self.dark_theme.clone()),
        );
        // Written back only when there is one, so a person who never chose a
        // font does not find a null in a file they opened to read.
        match self.font_family.as_ref() {
            Some(family) => {
                document.insert(FONT_FAMILY_KEY.to_owned(), Value::String(family.clone()));
            }
            None => {
                document.remove(FONT_FAMILY_KEY);
            }
        }
        Ok(document)
    }
}

/// One group of options out of a settings document, defaulting past anything
/// unusable.
///
/// `group` names the group in the warning, which is the only thing that tells
/// a person which half of their file the parser gave up on.
fn parse_group<T: Default + for<'de> Deserialize<'de>>(
    document: &Map<String, Value>,
    path: &Path,
    group: &str,
) -> T {
    match serde_json::from_value(Value::Object(document.clone())) {
        Ok(parsed) => parsed,
        Err(err) => {
            log::warn!(
                "{} holds {group} this build cannot read ({err}); using defaults",
                path.display()
            );
            T::default()
        }
    }
}

/// One group of options as the JSON keys it owns.
fn owned_keys(options: impl Serialize, group: &str) -> Result<Map<String, Value>> {
    let value = serde_json::to_value(options)
        .with_context(|| format!("could not serialize the {group}"))?;
    let Value::Object(owned) = value else {
        bail!("the {group} serialized to something other than a JSON object");
    };
    Ok(owned)
}

/// `<configuration directory>/crook/settings.json`, where there is one.
pub fn user_settings_path() -> Option<PathBuf> {
    config_directory().map(|directory| directory.join(SETTINGS_FILE))
}

/// The directory Crook keeps its per-user files in.
///
/// `None` on a machine with no configuration directory at all, which is a
/// machine with no home rather than one that has never run Crook.
pub fn config_directory() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join(CONFIG_DIRECTORY))
}

/// Writes `contents` to `path` without ever leaving a half-written file there.
///
/// The bytes go to a temporary beside the target and only a rename — one
/// operation the filesystem either does or does not do — puts them in place,
/// so a crash part-way through leaves whatever was there before intact. Shared
/// with [`crate::session`], which wants exactly the same promise about a file
/// that is written far more often than this one.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    write_then_rename(&temporary_path(path), path, contents)
}

/// Reads `path` as a JSON object, treating every way that can fail as an empty
/// one.
///
/// An empty document also means an empty set of keys to carry forward, which
/// is the right answer in each of these cases: keys that could not be read are
/// not keys worth preserving, and the next save replaces the unusable file
/// with one that parses.
fn read_document(path: &Path) -> Map<String, Value> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        // The ordinary state of a machine that has never changed an option.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            log::debug!("no settings file at {}; using defaults", path.display());
            return Map::new();
        }
        Err(err) => {
            log::warn!("could not read {} ({err}); using defaults", path.display());
            return Map::new();
        }
    };

    match serde_json::from_str(&contents) {
        Ok(Value::Object(document)) => document,
        Ok(_) => {
            log::warn!(
                "{} is JSON but not an object; using defaults",
                path.display()
            );
            Map::new()
        }
        Err(err) => {
            log::warn!("could not parse {} ({err}); using defaults", path.display());
            Map::new()
        }
    }
}

/// A name beside `path` that no other save can be using.
///
/// Two saves sharing one temporary would interleave their bytes and the slower
/// one would rename a half-written file into place — the exact failure the
/// temporary exists to prevent. The process id keeps two Crooks apart; the
/// serial keeps two background tasks in one Crook apart.
fn temporary_path(path: &Path) -> PathBuf {
    static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

    let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(OsStr::to_os_string)
        .unwrap_or_default();
    name.push(format!(".{}.{serial}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Writes `contents` to `temporary` and moves it onto `destination`.
fn write_then_rename(temporary: &Path, destination: &Path, contents: &[u8]) -> Result<()> {
    // Scoped so the handle is closed before the rename: Windows will not move
    // a file that is still open.
    {
        let mut file = fs::File::create(temporary)
            .with_context(|| format!("could not create {}", temporary.display()))?;
        file.write_all(contents)
            .with_context(|| format!("could not write {}", temporary.display()))?;
        // The rename is atomic with respect to the directory, not to the data:
        // without this, a machine that loses power just after the rename can
        // come back to a settings file full of zeroes.
        file.sync_all()
            .with_context(|| format!("could not flush {}", temporary.display()))?;
    }

    rename_replacing(temporary, destination).inspect_err(|_| {
        // Nothing ever reads a leftover temporary, and one per failed save
        // accumulates forever.
        let _ = fs::remove_file(temporary);
    })
}

/// Moves `from` onto `to`, replacing whatever is there.
#[cfg(not(windows))]
fn rename_replacing(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to)
        .with_context(|| format!("could not move {} onto {}", from.display(), to.display()))
}

/// Moves `from` onto `to`, replacing whatever is there, retrying while Windows
/// says someone else is looking at it.
///
/// `rename` on Unix replaces the destination no matter who holds it open. The
/// `MoveFileEx` behind it on Windows does not: a virus scanner or the search
/// indexer reading the file Crook just wrote fails the move outright with a
/// sharing violation. Those holders let go in milliseconds, so a handful of
/// retries turns the common case from a lost save into a save that lands
/// slightly late. What retrying cannot fix — a read-only destination, a
/// destination on another volume — comes back as an error for the caller to
/// log.
#[cfg(windows)]
fn rename_replacing(from: &Path, to: &Path) -> Result<()> {
    /// How many times to try before giving the error to the caller.
    const RENAME_ATTEMPTS: u32 = 5;
    /// How long to wait between tries.
    const RENAME_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(20);

    for attempt in 1..=RENAME_ATTEMPTS {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) if attempt < RENAME_ATTEMPTS => {
                log::debug!(
                    "could not move {} onto {} on attempt {attempt} ({err}); retrying",
                    from.display(),
                    to.display()
                );
                std::thread::sleep(RENAME_RETRY_DELAY);
            }
            Err(err) => {
                return Err(anyhow::Error::new(err).context(format!(
                    "could not move {} onto {} in {RENAME_ATTEMPTS} attempts",
                    from.display(),
                    to.display()
                )));
            }
        }
    }

    // The loop returns from every iteration; this is unreachable, and saying
    // so in code beats an `unreachable!` that could one day actually run.
    bail!("could not move {} onto {}", from.display(), to.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, removed when the test ends.
    ///
    /// Every test here writes real files, and pointing them at the real
    /// configuration directory would mean a test run changing the options of
    /// whoever ran it.
    struct ScratchDirectory {
        path: PathBuf,
    }

    impl ScratchDirectory {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("crook-settings-{}-{name}", std::process::id()));
            // A previous run that was killed before its `Drop` leaves this
            // behind, and a test that starts with someone else's file is not
            // testing what it says it is.
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("the scratch directory should be creatable");
            Self { path }
        }

        fn settings_file(&self) -> PathBuf {
            self.path.join(SETTINGS_FILE)
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.path)
                .expect("the scratch directory should be readable")
                .map(|entry| {
                    entry
                        .expect("the entry should be readable")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for ScratchDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Every field set to something other than its default, so a round trip
    /// that quietly loses one cannot pass.
    fn everything_flipped() -> TabOptions {
        TabOptions {
            granularity: Granularity::Tabs,
            density: Density::Expanded,
            primary_info: PrimaryInfo::Branch,
            subtitle: Subtitle::Command,
            show_pr_link: false,
            show_diff_stats: false,
            show_details_on_hover: false,
        }
    }

    #[test]
    fn test_the_defaults_are_warps_defaults() {
        let options = TabOptions::default();

        assert_eq!(Granularity::Panes, options.granularity);
        assert_eq!(Density::Compact, options.density);
        assert_eq!(PrimaryInfo::Command, options.primary_info);
        assert_eq!(Subtitle::Branch, options.subtitle);
        assert!(options.show_pr_link);
        assert!(options.show_diff_stats);
        assert!(options.show_details_on_hover);
    }

    #[test]
    fn test_every_field_survives_a_save_and_a_load() {
        let scratch = ScratchDirectory::new("round-trip");
        let mut settings = Settings::load(scratch.settings_file());
        settings.set_tab_options(everything_flipped());
        settings.save_blocking().expect("the save should succeed");

        assert_eq!(
            everything_flipped(),
            Settings::load(scratch.settings_file()).tab_options()
        );
    }

    #[test]
    fn test_the_theme_is_resolved_from_three_facts_and_nothing_else() {
        // A pure function of the flag, the pair of names and the desktop. It
        // needs no per-theme metadata: nothing declares itself light or dark,
        // and either theme can be either half of the pair.
        let scratch = ScratchDirectory::new("system-theme");
        let mut settings = Settings::load(scratch.settings_file());
        settings.set_theme("Midnight");
        settings.set_system_theme(false, "Crook Light");
        settings.set_system_theme(true, "Crook Dark");

        // Off, the desktop is not consulted at all.
        assert_eq!(settings.theme_for(true), "Midnight");
        assert_eq!(settings.theme_for(false), "Midnight");

        let mut general = settings.general();
        general.use_system_theme = true;
        settings.set_general(general);
        assert_eq!(settings.theme_for(true), "Crook Dark");
        assert_eq!(settings.theme_for(false), "Crook Light");
    }

    #[test]
    fn test_the_pair_survives_a_save() {
        let scratch = ScratchDirectory::new("system-theme-save");
        let mut settings = Settings::load(scratch.settings_file());
        settings.set_system_theme(false, "Gruvbox Light");
        settings.set_system_theme(true, "Kanagawa");
        settings.save_blocking().expect("the save should succeed");

        let reread = Settings::load(scratch.settings_file());
        assert_eq!(reread.light_theme(), "Gruvbox Light");
        assert_eq!(reread.dark_theme(), "Kanagawa");
    }

    #[test]
    fn test_a_missing_pair_falls_back_to_two_built_ins_that_match() {
        // A person who turns following on and never touches it again gets a
        // matched pair rather than two unrelated palettes.
        let scratch = ScratchDirectory::new("system-theme-default");
        let settings = Settings::load(scratch.settings_file());

        assert_eq!(settings.light_theme(), crate::theme::DEFAULT_LIGHT_NAME);
        assert_eq!(settings.dark_theme(), crate::theme::DEFAULT_NAME);
        assert!(crate::theme::named(settings.light_theme()).is_some());
        assert!(crate::theme::named(settings.dark_theme()).is_some());
        assert!(
            crate::theme::named(settings.light_theme())
                .expect("a built-in")
                .is_light,
            "the light half of the pair has to be light"
        );
    }

    #[test]
    fn test_a_hand_edited_font_size_cannot_divide_by_zero() {
        // Every grid measurement in the window is a box divided by a cell, and
        // a cell's width comes from this number. The file is meant to be
        // hand-edited, so the guard is here rather than at each division.
        for written in [0., -12., f32::INFINITY, f32::NAN] {
            let options = GeneralOptions {
                font_size: written,
                ..GeneralOptions::default()
            };
            let size = options.font_size();
            assert!(
                size.is_finite() && (MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&size),
                "a stored {written} resolved to {size}"
            );
        }

        assert_eq!(GeneralOptions::default().font_size(), DEFAULT_FONT_SIZE);
    }

    #[test]
    fn test_zooming_stops_at_both_ends() {
        // Both ends are reachable by holding the chord down, so both have to
        // be answers rather than accidents.
        let mut options = GeneralOptions::default();
        for _ in 0..200 {
            options.font_size = options.zoomed(FONT_SIZE_STEP);
        }
        assert_eq!(options.font_size(), MAX_FONT_SIZE);

        for _ in 0..200 {
            options.font_size = options.zoomed(-FONT_SIZE_STEP);
        }
        assert_eq!(options.font_size(), MIN_FONT_SIZE);
    }

    #[test]
    fn test_a_font_family_survives_a_save_and_an_absent_one_stays_absent() {
        let scratch = ScratchDirectory::new("font-family");
        fs::write(scratch.settings_file(), r#"{"font_family": "  Hack  "}"#)
            .expect("the file should be writable");

        let settings = Settings::load(scratch.settings_file());
        assert_eq!(
            settings.font_family(),
            Some("Hack"),
            "a name is trimmed, because a file is hand-edited"
        );
        settings.save_blocking().expect("the save should succeed");
        assert_eq!(
            Settings::load(scratch.settings_file()).font_family(),
            Some("Hack")
        );

        // An empty string names no family, and writing `null` back for it
        // would put a key in a file nobody asked for one in.
        let empty = ScratchDirectory::new("font-family-empty");
        fs::write(empty.settings_file(), r#"{"font_family": "   "}"#)
            .expect("the file should be writable");
        let settings = Settings::load(empty.settings_file());
        assert_eq!(settings.font_family(), None);
        settings.save_blocking().expect("the save should succeed");

        let written = fs::read_to_string(empty.settings_file()).expect("readable");
        assert!(!written.contains("font_family"), "{written}");
    }

    #[test]
    fn test_the_file_uses_warps_key_names_and_spellings() {
        let scratch = ScratchDirectory::new("key-names");
        let mut settings = Settings::load(scratch.settings_file());
        settings.set_tab_options(everything_flipped());
        settings.save_blocking().expect("the save should succeed");

        let written: Map<String, Value> = serde_json::from_str(
            &fs::read_to_string(scratch.settings_file()).expect("the file should be readable"),
        )
        .expect("the file should be a JSON object");

        assert_eq!(
            vec![
                "compact_subtitle",
                // The dark half of the pair the desktop chooses between. Warp
                // has this too, spelled the same way.
                "dark_theme",
                "display_granularity",
                // Crook's own: Warp keeps the terminal's type size in its
                // appearance settings, which this file is not a copy of.
                "font_size",
                "light_theme",
                // Crook's own, and the one key here that changes what the
                // shell itself is rather than what the window looks like.
                "login_shell",
                "primary_info",
                "restore_session",
                "show_details_on_hover",
                "show_diff_stats",
                "show_pr_link",
                // The chosen theme's name, which is a string rather than an
                // option with a type: see `Settings::theme`.
                "theme",
                "use_system_theme",
                "view_mode",
            ],
            written.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            Some(&Value::from("tabs")),
            written.get("display_granularity")
        );
        assert_eq!(Some(&Value::from("expanded")), written.get("view_mode"));
        assert_eq!(Some(&Value::from("branch")), written.get("primary_info"));
    }

    #[test]
    fn test_a_missing_file_loads_the_defaults() {
        let scratch = ScratchDirectory::new("missing");
        let settings = Settings::load(scratch.settings_file());

        assert_eq!(TabOptions::default(), settings.tab_options());
        assert!(!scratch.settings_file().exists());
    }

    #[test]
    fn test_malformed_json_loads_the_defaults() {
        let scratch = ScratchDirectory::new("malformed");
        fs::write(scratch.settings_file(), r#"{"view_mode": "expa"#)
            .expect("the file should be writable");

        assert_eq!(
            TabOptions::default(),
            Settings::load(scratch.settings_file()).tab_options()
        );
    }

    #[test]
    fn test_json_that_is_not_an_object_loads_the_defaults() {
        let scratch = ScratchDirectory::new("not-an-object");
        fs::write(scratch.settings_file(), "[1, 2, 3]").expect("the file should be writable");

        assert_eq!(
            TabOptions::default(),
            Settings::load(scratch.settings_file()).tab_options()
        );
    }

    #[test]
    fn test_a_value_this_build_does_not_know_defaults_only_the_options() {
        let scratch = ScratchDirectory::new("unknown-value");
        fs::write(
            scratch.settings_file(),
            r#"{"view_mode": "cosy", "future_key": 7}"#,
        )
        .expect("the file should be writable");

        let settings = Settings::load(scratch.settings_file());
        assert_eq!(TabOptions::default(), settings.tab_options());

        // The rest of the file is still carried: an option Crook cannot read
        // is not a reason to throw away a key that a later Crook can.
        settings.save_blocking().expect("the save should succeed");
        let written: Map<String, Value> = serde_json::from_str(
            &fs::read_to_string(scratch.settings_file()).expect("the file should be readable"),
        )
        .expect("the file should be a JSON object");
        assert_eq!(Some(&Value::from(7)), written.get("future_key"));
        assert_eq!(Some(&Value::from("compact")), written.get("view_mode"));
    }

    #[test]
    fn test_missing_keys_take_their_defaults_and_the_present_one_is_kept() {
        let scratch = ScratchDirectory::new("partial");
        fs::write(scratch.settings_file(), r#"{"show_pr_link": false}"#)
            .expect("the file should be writable");

        let options = Settings::load(scratch.settings_file()).tab_options();

        assert!(!options.show_pr_link);
        assert_eq!(
            TabOptions {
                show_pr_link: true,
                ..options
            },
            TabOptions::default()
        );
    }

    #[test]
    fn test_a_key_this_build_does_not_know_survives_a_load_and_a_save() {
        let scratch = ScratchDirectory::new("unknown-key");
        fs::write(
            scratch.settings_file(),
            r#"{"tomorrows_option": {"nested": ["value"]}, "show_diff_stats": false}"#,
        )
        .expect("the file should be writable");

        let mut settings = Settings::load(scratch.settings_file());
        settings.set_tab_options(everything_flipped());
        settings.save_blocking().expect("the save should succeed");

        let written: Map<String, Value> = serde_json::from_str(
            &fs::read_to_string(scratch.settings_file()).expect("the file should be readable"),
        )
        .expect("the file should be a JSON object");

        assert_eq!(
            Some(&serde_json::json!({"nested": ["value"]})),
            written.get("tomorrows_option")
        );
        assert_eq!(
            Some(&Value::from("tabs")),
            written.get("display_granularity")
        );
    }

    #[test]
    fn test_a_save_leaves_no_temporary_behind() {
        let scratch = ScratchDirectory::new("no-temporary");
        let settings = Settings::load(scratch.settings_file());
        settings.save_blocking().expect("the save should succeed");
        settings
            .save_blocking()
            .expect("the second save should succeed");

        assert_eq!(vec![SETTINGS_FILE.to_owned()], scratch.entries());
    }

    #[test]
    fn test_a_save_replaces_the_file_rather_than_adding_to_it() {
        let scratch = ScratchDirectory::new("replace");
        // Loaded before the file exists, so this instance carries no keys of
        // the long file below — what it writes has to be the whole file, and a
        // save that only overwrote the front of it would leave a tail.
        let mut settings = Settings::load(scratch.settings_file());
        fs::write(
            scratch.settings_file(),
            format!(r#"{{"padding": "{}"}}"#, "x".repeat(8192)),
        )
        .expect("the file should be writable");

        settings.set_tab_options(everything_flipped());
        settings.save_blocking().expect("the save should succeed");

        let contents =
            fs::read_to_string(scratch.settings_file()).expect("the file should be readable");
        let written: Map<String, Value> =
            serde_json::from_str(&contents).expect("the file should be a JSON object");

        // Seven tab options, four general ones and three theme names, and
        // nothing else: the 8KB key the file started with is gone. The font
        // family is not among them — an absent key is what "no preference"
        // is, so a save writes no `font_family` unless one was chosen.
        assert_eq!(14, written.len());
        assert!(!contents.contains("padding"));
        assert_eq!(
            everything_flipped(),
            Settings::load(scratch.settings_file()).tab_options()
        );
    }

    #[test]
    fn test_a_save_creates_the_directory_it_writes_into() {
        let scratch = ScratchDirectory::new("nested");
        let path = scratch.path.join("nested").join(SETTINGS_FILE);
        let settings = Settings::load(&path);

        settings.save_blocking().expect("the save should succeed");

        assert!(path.exists());
    }

    #[test]
    fn test_settings_with_nowhere_to_save_say_so_instead_of_panicking() {
        let settings = Settings::ephemeral();

        assert!(settings.path().is_none());
        assert_eq!(TabOptions::default(), settings.tab_options());
        assert!(settings.save_blocking().is_err());
    }

    #[test]
    fn test_a_subtitle_that_repeats_the_title_falls_back_instead() {
        // The whole point of the rule: no row ever prints the same fact twice.
        assert_eq!(
            Subtitle::Branch,
            resolve_subtitle(PrimaryInfo::Command, Subtitle::Command)
        );
        assert_eq!(
            Subtitle::Branch,
            resolve_subtitle(PrimaryInfo::WorkingDirectory, Subtitle::WorkingDirectory)
        );
        assert_eq!(
            Subtitle::Command,
            resolve_subtitle(PrimaryInfo::Branch, Subtitle::Branch)
        );
    }

    #[test]
    fn test_a_subtitle_that_does_not_repeat_the_title_is_left_alone() {
        for primary in [
            PrimaryInfo::Command,
            PrimaryInfo::WorkingDirectory,
            PrimaryInfo::Branch,
        ] {
            for subtitle in subtitle_options_for(primary) {
                assert_eq!(
                    subtitle,
                    resolve_subtitle(primary, subtitle),
                    "{primary:?} rewrote a subtitle it does not collide with"
                );
            }
        }
    }

    #[test]
    fn test_the_menu_only_ever_offers_the_two_facts_the_title_is_not_showing() {
        assert_eq!(
            [Subtitle::Branch, Subtitle::WorkingDirectory],
            subtitle_options_for(PrimaryInfo::Command)
        );
        assert_eq!(
            [Subtitle::Branch, Subtitle::Command],
            subtitle_options_for(PrimaryInfo::WorkingDirectory)
        );
        assert_eq!(
            [Subtitle::Command, Subtitle::WorkingDirectory],
            subtitle_options_for(PrimaryInfo::Branch)
        );
    }

    #[test]
    fn test_the_correction_is_never_written_back() {
        // Warp resolves at read time and stores the preference untouched, so
        // moving the title through a colliding value and back has to give the
        // subtitle the user actually chose.
        let mut options = TabOptions {
            primary_info: PrimaryInfo::WorkingDirectory,
            subtitle: Subtitle::WorkingDirectory,
            ..TabOptions::default()
        };
        assert_eq!(
            Subtitle::Branch,
            resolve_subtitle(options.primary_info, options.subtitle)
        );

        options.primary_info = PrimaryInfo::Command;
        assert_eq!(
            Subtitle::WorkingDirectory,
            resolve_subtitle(options.primary_info, options.subtitle)
        );
    }

    #[test]
    fn test_the_user_path_is_settings_json_in_a_crook_directory() {
        // `None` is legal — a machine with no home directory — and the point of
        // `for_user` handling it, so there is nothing to assert about it here.
        if let Some(path) = user_settings_path() {
            assert!(path.ends_with(Path::new(CONFIG_DIRECTORY).join(SETTINGS_FILE)));
        }
    }

    #[test]
    fn test_a_plugin_switched_off_is_remembered_and_one_switched_back_on_is_forgotten() {
        // The half of the switch that is not on screen. A list that only held
        // the plugins that are *on* would go stale the day the build gains
        // one, which is why the file records the exceptions.
        //
        // The name is one this build no longer carries, and deliberately so:
        // this module knows nothing about which plugins exist, and a fixture
        // that named a real one would let a lookup creep in without any test
        // noticing.
        let scratch = ScratchDirectory::new("disabled-plugins");
        let mut settings = Settings::load(scratch.settings_file());
        assert!(settings.disabled_plugins().is_empty());

        settings.set_plugin_disabled("crook/usage", true);
        settings.set_plugin_disabled("crook/usage", true);
        assert_eq!(settings.disabled_plugins(), ["crook/usage"]);
        settings
            .save_blocking()
            .expect("the file should be written");

        let read_back = Settings::load(scratch.settings_file());
        assert_eq!(read_back.disabled_plugins(), ["crook/usage"]);

        let mut read_back = read_back;
        read_back.set_plugin_disabled("crook/usage", false);
        read_back
            .save_blocking()
            .expect("the file should be written");
        assert!(
            Settings::load(scratch.settings_file())
                .disabled_plugins()
                .is_empty()
        );
        // And the key is gone rather than left as an empty array, because a
        // person who has never switched a plugin off should not find one in a
        // file they opened to read.
        let text = fs::read_to_string(scratch.settings_file()).expect("readable");
        assert!(!text.contains("disabled_plugins"), "{text}");
    }

    #[test]
    fn test_an_unusable_entry_in_the_disabled_list_costs_only_itself() {
        let scratch = ScratchDirectory::new("disabled-plugins-rubbish");
        fs::write(
            scratch.settings_file(),
            r#"{"disabled_plugins": ["crook/usage", 7, "", "  ", "eugen/themes"]}"#,
        )
        .expect("the file should be writable");

        let settings = Settings::load(scratch.settings_file());

        assert_eq!(settings.disabled_plugins(), ["crook/usage", "eugen/themes"]);
    }
}
