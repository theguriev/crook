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

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::notify::Occasion;

pub mod watch;

/// The directory Crook keeps its per-user files in, inside the platform's
/// configuration directory.
const CONFIG_DIRECTORY: &str = "crook";

/// The one file in it.
const SETTINGS_FILE: &str = "settings.json";

/// The file every save of the settings locks while it reads and writes them.
/// See [`hold_the_lock`].
const LOCK_FILE: &str = "settings.lock";

/// How long a save waits for another window's to finish. See
/// [`hold_the_lock`].
const LOCK_PATIENCE: std::time::Duration = std::time::Duration::from_secs(2);

/// The directory beside the file that keeps copies of settings files a save
/// wrote over because they did not read. See [`back_up_unreadable`].
const BACKUP_DIRECTORY: &str = "settings-backups";

/// How many of those copies are kept.
const BACKUPS_KEPT: usize = 3;

/// The key the chosen theme's name is stored under.
const THEME_KEY: &str = "theme";

/// The key the terminal's monospace family is stored under.
const FONT_FAMILY_KEY: &str = "font_family";

/// The key the list of switched-off plugins is stored under.
const DISABLED_PLUGINS_KEY: &str = "disabled_plugins";

/// The key the things a person has allowed a plugin to do are stored under.
const PLUGIN_GRANTS_KEY: &str = "plugin_grants";

/// The key the lines a person gave their agents' resumes are stored under.
const RESUME_LINES_KEY: &str = "resume_lines";

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

/// What the mark at the head of a row says its status with — the page's
/// "Status marks".
///
/// Crook's own; Warp has a glyph in every row's mark and a status ring on its
/// corner, so it never had to choose. The disc is the default because a fresh
/// install should open looking like Warp's, and the glyphs are for the person
/// the disc says nothing to: one who cannot tell violet from grey, a
/// screenshot in greyscale, or a row washed amber where an amber dot
/// disappears. Either way the colour is the same, so a person who reads both
/// loses nothing by switching.
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusMarks {
    /// A filled disc in the status colour, and nothing else.
    #[default]
    Dots,
    /// A small glyph per state in the same colour: a shape that reads where a
    /// colour does not.
    Glyphs,
}

/// What Tab does in the command line while a suggestion from the history
/// stands after the caret.
///
/// Warp's "Tab key behavior", and the same two answers. Its dropdown rewrites
/// two keybindings; here it is one option, because what Tab means in the
/// field is decided by the field rather than by the keymap — see
/// `TextInput::apply`.
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TabKey {
    /// Tab asks the shell what the word could become, and steps through
    /// what it offered; Right takes the suggestion.
    #[default]
    Completions,
    /// Tab takes the suggestion, as Right does. With none standing it asks
    /// the shell, as it always did, and Ctrl-Space asks the shell whatever
    /// is standing.
    Suggestion,
}

impl TabKey {
    /// What the page calls this.
    pub fn label(self) -> &'static str {
        match self {
            Self::Completions => "Complete",
            Self::Suggestion => "Accept suggestion",
        }
    }
}

/// Which Option key a program reads as Meta on a Mac, and which one types
/// what the keyboard layout puts on it.
///
/// Every Mac terminal has this, as `macos_option_as_alt` in kitty,
/// `option_as_alt` in Alacritty, `macos-option-as-alt` in Ghostty and a menu
/// per Option key in iTerm2, with the same four answers. The fifth, the
/// default, is Crook's: a rule per key rather than per keyboard. See
/// [`crate::terminal_keys::Alt`] for what each one sends.
///
/// Linux and Windows never read it: their layouts type with AltGr, which is
/// not Alt, so Alt there is only ever Meta.
///
/// No value reaches an Option combination the layout makes a dead key: US
/// Option-E, -U, -I, -N and Option-backtick, and German Option-N, which is
/// how `~` is typed there. The Mac's input method takes those to start an accent before
/// Crook is sent a key press, so under every value they start one, as they
/// always have.
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionAsMeta {
    /// Option types the ASCII symbols a layout puts on it, `@ [ ] { } | \`
    /// on a German keyboard and `#` on a British one, and is Meta for every
    /// other key. A US keyboard's Option layer has no ASCII in it, so there
    /// Option is Meta throughout, as it always was in Crook, dead keys aside.
    #[default]
    Auto,
    /// The left Option is Meta and the right one types.
    Left,
    /// The right Option is Meta and the left one types.
    Right,
    /// Both are Meta, and Option types nothing in a program but the accents
    /// of the layout's dead keys.
    Both,
    /// Neither is Meta: Option types whatever the layout puts on it, `ƒ` and
    /// `å` included, as it does in Terminal.app out of the box.
    #[serde(rename = "none")]
    Neither,
}

impl OptionAsMeta {
    /// Every value, in the order the page offers them.
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::Left,
        Self::Right,
        Self::Both,
        Self::Neither,
    ];

    /// What the page calls this.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Both => "Both",
            Self::Neither => "None",
        }
    }
}

impl StatusMarks {
    /// What the page calls this.
    pub fn label(self) -> &'static str {
        match self {
            Self::Dots => "Dots",
            Self::Glyphs => "Glyphs",
        }
    }
}

/// Whether anything in the window moves on its own: a running agent's mark
/// breathing, and the caret blinking.
///
/// The two things in Crook that move with nobody touching anything, and one
/// setting for both, because the person who wants one of them still wants
/// the other still too: what it is for is a pulse in the corner of the eye
/// for hours, and a caret blinking beside it is the same pulse. Every
/// desktop has a switch for exactly this — macOS's Reduce motion, GNOME's
/// animations, Windows' animation effects — so the default is to do as that
/// switch says, and the other two answers are for a person who wants Crook
/// to differ from the rest of their desktop.
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    /// As the desktop says: still where it asks for less motion, and
    /// moving where it does not or cannot be asked. See [`crate::motion`].
    #[default]
    System,
    /// Running marks breathe and the caret blinks, whatever the desktop
    /// says.
    Full,
    /// Nothing moves: a running mark is drawn in its full colour and the
    /// caret is solid, and no timer runs for either.
    Reduced,
}

impl Motion {
    /// Every value, in the order the page offers them.
    pub const ALL: [Self; 3] = [Self::System, Self::Full, Self::Reduced];

    /// What the page calls this.
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Full => "Full",
            Self::Reduced => "Reduced",
        }
    }

    /// Whether nothing should move, given whether the desktop asks for less
    /// motion.
    pub fn is_reduced(self, desktop_reduces: bool) -> bool {
        match self {
            Self::System => desktop_reduces,
            Self::Full => false,
            Self::Reduced => true,
        }
    }
}

/// Everything the settings page writes that is not about the tab strip.
///
/// A type size and eight switches, and that is not an accident of
/// scheduling. Crook has a window and a tab strip; every option that could be
/// offered about the strip is already in [`TabOptions`], and what a plugin
/// wants asked about itself belongs to that plugin rather than here. Warp's
/// settings hold roughly eight hundred keys behind a schema system, a
/// migration path and a cloud-sync policy — `docs/architecture.md` is
/// explicit that a `serde` struct in a file is the right answer until there
/// are ten of them, and this is the second struct, not the beginning of a
/// schema.
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
    /// tabs, splits and the directories their shells were in, and the name of
    /// an agent a pane was running, whose resume line is offered unsent. No
    /// output is restored and no process is: see [`crate::session`].
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
    /// Whether the tabs panel is drawn down the left of the window.
    ///
    /// Off, the body takes the whole width — what a person reading wide
    /// output or working on a small screen wants, and what herdr's
    /// `prefix+b` does to its sidebar. The tabs are still there and every
    /// chord that reaches them still works; only the column is gone. It is
    /// remembered here rather than in the session because it is a preference
    /// about the window and not a fact about what was open in it: somebody
    /// who hid the panel wants it hidden tomorrow too, whatever tabs come
    /// back.
    ///
    /// The tabs section only. Every other section of the sidebar — the
    /// settings, the plugins, the store — *is* its list, so the column comes
    /// back while one of those is showing and goes again on the way back to
    /// the tabs.
    pub show_tabs_panel: bool,
    /// How wide the tabs panel is, in points, as its edge was last dragged.
    ///
    /// Here for the reason [`Self::show_tabs_panel`] is: how much of the
    /// window a person gives the tabs is a preference about the window. Read
    /// through the panel's limits rather than trusted, so a hand-edited
    /// width the panel cannot be drawn at is the nearest one it can.
    pub tabs_panel_width: f32,
    /// What Tab does while a suggestion stands after the caret. See
    /// [`TabKey`].
    pub tab_key: TabKey,
    /// What Option means to a program on a Mac: Meta, or the key a layout
    /// types its symbols with. See [`OptionAsMeta`].
    ///
    /// The composer is not a program and does not read it. There Option
    /// always types, as it does in every Mac text field, and Option-Left and
    /// Option-Right are already its word motions.
    pub option_as_meta: OptionAsMeta,
    /// Whether a close that would end something still working asks first.
    ///
    /// The window's close, a tab's, a group's and a pane's all end the
    /// processes in the panes they take — a pty that goes hangs its child up
    /// — and a pane whose agent is running or waiting, or whose shell is
    /// running a command, is somebody's unfinished work. On, because the
    /// question costs a press and an agent's context cannot be had back.
    /// Off, for a person who closes things on purpose and would rather not be
    /// asked; "End all agents and quit" is the same answer given once, with
    /// this left on. See `workspace::closing`.
    pub ask_before_ending_agents: bool,
    /// Whether the window takes the questions agents' permission hooks hand
    /// it — see [`crate::control::ask`] — or refuses each as `declined`, and
    /// the hook passes at once to the agent's own prompt.
    ///
    /// On, because a question taken changes nothing a person does not look
    /// for: the agent's prompt is still up, the window shows the command on
    /// the row, and the card that answers it opens only when a person opens
    /// it. Off — "Let agents ask in Crook", under Tabs on the Appearance
    /// page — is the switch for a person who wants Crook to have no part in
    /// what an agent is allowed to do, and it ends every question already
    /// held as a pass.
    pub answer_agents_from_crook: bool,
    /// Whether a pane whose row turns to needs-input while the window is
    /// behind another one posts a desktop notification.
    ///
    /// On, because it is the one notification a person away from the window
    /// is waiting for: an agent that stopped to ask does nothing until
    /// somebody answers. See [`crate::notify`] for when else one is posted
    /// and how often.
    pub notify_on_needs_input: bool,
    /// Whether an agent saying it failed posts one.
    ///
    /// Off, because a failure is the agent's to report and many agents say
    /// "failed" on the way to trying again; somebody who wants to hear it
    /// turns it on.
    pub notify_on_failed: bool,
    /// Whether a command that ran for at least
    /// [`LONG_COMMAND`](crate::notify::LONG_COMMAND) posts one when it ends.
    ///
    /// Off, for the reason every terminal that offers this has it off: a
    /// banner at the end of every build is a banner a person learns to
    /// ignore, and the one that mattered goes with the rest.
    pub notify_on_long_command: bool,
    /// Whether Crook is opened when the person logs in. See
    /// [`crate::login_item`], which is what puts the entry in place: this is
    /// what the person asked for, and the entry is what the system reads.
    pub open_at_login: bool,
    /// Whether a bell a program rings plays the system's alert sound.
    ///
    /// Off, as in Warp and every terminal that has learned what a shell
    /// ringing on each failed completion sounds like across an afternoon. A
    /// pane nobody is looking at is marked whichever way this is set; see
    /// [`crate::bell`] for the sound.
    pub audible_bell: bool,
    /// Whether text selected in Crook becomes PRIMARY, the selection a middle
    /// click pastes, and whether a middle click in a pane pastes it.
    ///
    /// On, because it is what every Linux terminal does and what a Linux hand
    /// does without looking. Read only where the desktop keeps PRIMARY at all,
    /// and offered only there: on macOS and Windows there is nothing for it to
    /// switch. See [`crate::clipboard`].
    pub middle_click_paste: bool,
    /// Whether running marks breathe and the caret blinks, or the window
    /// keeps still. See [`Motion`].
    pub motion: Motion,
}

/// How wide the tabs panel is until its edge is dragged: Warp's
/// `PANEL_WIDTH`.
pub const DEFAULT_TABS_PANEL_WIDTH: f32 = 248.;

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
            show_tabs_panel: true,
            tabs_panel_width: DEFAULT_TABS_PANEL_WIDTH,
            tab_key: TabKey::Completions,
            option_as_meta: OptionAsMeta::Auto,
            ask_before_ending_agents: true,
            answer_agents_from_crook: true,
            notify_on_needs_input: true,
            notify_on_failed: false,
            notify_on_long_command: false,
            open_at_login: false,
            audible_bell: false,
            middle_click_paste: true,
            motion: Motion::System,
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

    /// Whether a person asked to be notified of `occasion`.
    pub fn notifies_on(self, occasion: Occasion) -> bool {
        match occasion {
            Occasion::NeedsInput => self.notify_on_needs_input,
            Occasion::Failed => self.notify_on_failed,
            Occasion::LongCommand => self.notify_on_long_command,
        }
    }

    /// Whether a person asked to be notified of anything at all: one of the
    /// switches [`Self::notifies_on`] reads is on.
    pub fn notifies_on_any(self) -> bool {
        self.notify_on_needs_input || self.notify_on_failed || self.notify_on_long_command
    }

    /// The same options, with `occasion`'s switch turned the other way.
    pub fn toggled(mut self, occasion: Occasion) -> Self {
        let switch = match occasion {
            Occasion::NeedsInput => &mut self.notify_on_needs_input,
            Occasion::Failed => &mut self.notify_on_failed,
            Occasion::LongCommand => &mut self.notify_on_long_command,
        };
        *switch = !*switch;
        self
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
    /// "Show: Diff stats" — the chip counting added and removed lines, and a
    /// branch's commits since its base.
    pub show_diff_stats: bool,
    /// "Show details on hover" — whether hovering a row opens its detail card.
    /// One of the two options the menu shows in every state.
    pub show_details_on_hover: bool,
    /// "Show tab numbers" — whether a row leads with the tab's place in the
    /// strip, which is the number `cmd-1`…`cmd-8` (`alt-` off macOS) selects
    /// it by. Crook's own, with no Warp key behind it: once tabs are named the
    /// chord is blind without it.
    pub show_tab_numbers: bool,
    /// "Status marks" — whether the mark at the head of a row says its status
    /// as a disc or as a glyph. Crook's own, like the numbers: Warp's rows
    /// carry both a glyph and a ring, and this file has one mark to spend.
    pub status_marks: StatusMarks,
}

impl Default for TabOptions {
    /// Warp's defaults — `Panes`, `Compact`, `Command`, `Branch`, and every
    /// "Show" on. The tabs live in a panel rather than a strip, which is the
    /// one place Crook's defaults deliberately differ from Warp's, and that
    /// is the window's shape rather than an option here.
    ///
    /// The three "Show" booleans are why this is written out rather than
    /// derived — `bool`'s default is `false`, and Warp's is `true` for all
    /// three. The tab numbers are off and the status marks are dots: both
    /// are Crook's own, and a fresh install should open looking like Warp's.
    fn default() -> Self {
        Self {
            granularity: Granularity::default(),
            density: Density::default(),
            primary_info: PrimaryInfo::default(),
            subtitle: Subtitle::default(),
            show_pr_link: true,
            show_diff_stats: true,
            show_details_on_hover: true,
            show_tab_numbers: false,
            status_marks: StatusMarks::default(),
        }
    }
}

/// Crook's settings, and the file they came from.
///
/// Cheap to clone — a path and a handful of JSON keys. What a save takes off
/// the main thread is smaller still: [`Settings::take_changes`], what this
/// instance changed, for [`Settings::write_changes`] to lay over the file on
/// the background pool.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Where [`Settings::save_blocking`] writes. `None` only when the platform
    /// admits to no configuration directory at all, which is a machine with no
    /// home directory rather than a machine that has never run Crook.
    path: Option<PathBuf>,
    /// The file exactly as it was last read whole, including keys this build
    /// knows nothing about.
    ///
    /// Not what a save writes over — a save writes over the file as it is on
    /// disk at that moment, see [`Settings::write_changes`]. This is what it
    /// falls back on when the file on disk is one that does not read: a hand
    /// edit with a trailing comma must cost the person that edit, which is
    /// kept in a copy, and not every option they had.
    document: Map<String, Value>,
    /// What this instance last agreed with the file about: the keys this
    /// build owns, as [`Settings::owned`] renders them, at the load or the
    /// adoption that last brought the two together.
    ///
    /// **This is what keeps two windows from undoing each other.** Each
    /// window of Crook is a process of its own with a copy of the settings,
    /// and a save used to write the whole copy back — so a plugin revoked in
    /// one window was granted again by the next theme chosen in the other.
    /// What a save writes now is only what differs from this: the keys this
    /// window *changed*, laid over the file as it is. See
    /// [`Settings::changes`].
    base: Map<String, Value>,
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
    /// What each plugin has been allowed to do, by `owner/name`, as
    /// [`Capability::keys`](crook_plugin_api::Capability::keys) writes it
    /// down.
    ///
    /// Only what was *allowed*, and one entry per host and per path rather
    /// than one per capability. That is what makes an escalation visible: a
    /// plugin whose next version wants a second host asks for a key that is
    /// not in this list, so it is refused and the Plugins page can say which
    /// line is new — where a stored "yes" would have covered whatever the
    /// plugin asked for next.
    ///
    /// Keys for plugins this build has never heard of are kept and ignored,
    /// for the reason [`Settings::disabled_plugins`] keeps names it does not
    /// know: it is a plugin that has been uninstalled, and forgetting the
    /// grant would mean asking again for something already answered.
    plugin_grants: BTreeMap<String, Vec<String>>,
    /// The line each agent is resumed with in a restored pane, by the
    /// agent's program name, where a person has said something other than
    /// the built-in one.
    ///
    /// Only what a person wrote, never the built-in lines copied in: an
    /// agent's flags drift from release to release, and a file that held
    /// this build's spelling would go on offering it after the build that
    /// knew better had shipped. An empty line is an agent a person wants
    /// offered nothing for. Set in the file rather than on the page — see
    /// [`Settings::resume_line`].
    resume_lines: BTreeMap<String, String>,
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
            return Self::from_document(None, Map::new());
        };

        Self::load(path)
    }

    /// Settings that live only as long as the process.
    ///
    /// Nothing to read and nowhere to write, which is what a test and a
    /// headless snapshot both want: a run that neither depends on the options
    /// of whoever started it nor changes them.
    pub fn ephemeral() -> Self {
        Self::from_document(None, Map::new())
    }

    /// Reads the settings file at `path`, defaulting past anything unusable.
    ///
    /// Tests point this at a scratch directory; nothing else should need to.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let document = read_document(&path);
        Self::from_document(Some(path), document)
    }

    /// The settings a document says, defaulting past anything unusable in it,
    /// agreeing with it about every key: nothing has been changed yet.
    fn from_document(path: Option<PathBuf>, document: Map<String, Value>) -> Self {
        // What a warning about a key names, which is the file even for
        // settings that were never read from one.
        let named = path.clone().unwrap_or_else(|| PathBuf::from(SETTINGS_FILE));

        // Parsed from a copy, so that a value this build cannot make sense of
        // still reaches `document` and survives the next save.
        //
        // Two independent parses of the same object rather than one parse of a
        // struct holding both, because the two groups fail independently: a
        // hand-edited `display_granularity` that names nothing must not take
        // the type size down with it.
        let tab_options = parse_group(&document, &named, "tab options");
        let general = parse_group(&document, &named, "general options");
        // Not through `parse_group`: a theme name is one string rather than a
        // group of typed options, and a file naming a theme this machine does
        // not have is not a parse failure — the name is kept, and the theme
        // falls back to the default until whatever wrote it is put back.
        //
        // Through `named_theme`, the same as `light_theme` and `dark_theme`
        // below: a hand-edited name is trimmed of the space around it (which
        // an exact-match lookup would otherwise miss), and an empty one is the
        // default rather than a `no theme called ""` warning on every launch.
        let theme = named_theme(&document, THEME_KEY, crate::theme::DEFAULT_NAME);

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

        // Read the way the list above is: one unusable entry costs that
        // plugin's grant and not everybody's. But not trimmed, as a name is:
        // a key is the capability's own text, compared to what the plugin
        // asks for byte for byte, and `type:git commit -m {} ` trimmed was a
        // grant for a command nobody asks for — the dialog came back on
        // every launch for the one that was allowed.
        let plugin_grants = document
            .get(PLUGIN_GRANTS_KEY)
            .and_then(Value::as_object)
            .map(|plugins| {
                plugins
                    .iter()
                    .filter_map(|(plugin, keys)| {
                        let keys: Vec<String> = keys
                            .as_array()?
                            .iter()
                            .filter_map(Value::as_str)
                            .filter(|key| !key.is_empty())
                            .map(str::to_owned)
                            .collect();
                        (!keys.is_empty()).then(|| (plugin.clone(), keys))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Read the way the grants are: one unusable entry costs that agent's
        // line and not everybody's. Trimmed, because a line is typed into a
        // shell and the space around it means nothing there — and an empty
        // one is kept, because it is how a person says "offer nothing".
        let resume_lines = document
            .get(RESUME_LINES_KEY)
            .and_then(Value::as_object)
            .map(|agents| {
                agents
                    .iter()
                    .filter_map(|(program, line)| {
                        Some((program.trim().to_owned(), line.as_str()?.trim().to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let light_theme = named_theme(&document, LIGHT_THEME_KEY, crate::theme::DEFAULT_LIGHT_NAME);
        let dark_theme = named_theme(&document, DARK_THEME_KEY, crate::theme::DEFAULT_NAME);

        let mut settings = Self {
            path,
            document,
            base: Map::new(),
            tab_options,
            general,
            disabled_plugins,
            plugin_grants,
            resume_lines,
            theme,
            light_theme,
            dark_theme,
            font_family,
        };
        settings.base = settings.owned();
        settings
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

    /// The file as it was last read whole, which a save falls back on when
    /// the file on disk does not read. See [`Settings::write_changes`].
    pub fn document(&self) -> &Map<String, Value> {
        &self.document
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

    /// What a plugin has been allowed to do.
    ///
    /// Empty for a plugin nobody has answered for, which is the state every
    /// plugin installs in: asking is not being granted.
    pub fn granted_to(&self, plugin: &str) -> &[String] {
        self.plugin_grants
            .get(plugin)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Everything allowed, for handing to the plugins as they build.
    pub fn plugin_grants(&self) -> &BTreeMap<String, Vec<String>> {
        &self.plugin_grants
    }

    /// Records what a plugin may do, replacing whatever it could before.
    ///
    /// Replacing rather than adding, because this is the answer to a dialog
    /// that showed a whole list: allowing a plugin that now wants two hosts is
    /// allowing both, and revoking is the same call with nothing in it.
    /// Touches no file.
    pub fn set_granted(&mut self, plugin: &str, keys: Vec<String>) {
        if keys.is_empty() {
            self.plugin_grants.remove(plugin);
        } else {
            self.plugin_grants.insert(plugin.to_owned(), keys);
        }
    }

    /// The line a restored pane that was running `program` is offered, in
    /// the `resume` form, or `None` when there is nothing to offer.
    ///
    /// A person's own line for the agent, from `resume_lines` in the file,
    /// when there is one: it replaces the [`Resume::Last`] line, and an empty
    /// one turns both off. Otherwise, and for the picker whatever the file
    /// says, the agent's built-in line — see [`crate::agent::resume_line`].
    ///
    /// [`Resume::Last`]: crate::agent::Resume::Last
    pub fn resume_line(&self, program: &str, resume: crate::agent::Resume) -> Option<String> {
        match (self.resume_lines.get(program), resume) {
            (Some(line), _) if line.is_empty() => None,
            (Some(line), crate::agent::Resume::Last) => Some(line.clone()),
            _ => crate::agent::resume_line(program, resume).map(str::to_owned),
        }
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

    /// Writes what this instance changed to disk, atomically, over the file
    /// as it is now.
    ///
    /// Blocking, and more of it than it looks: this creates a directory,
    /// waits its turn at the lock beside the file, reads it, writes a
    /// temporary, waits for the filesystem to admit it has the bytes, and
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
    /// What is written is [`Settings::changes`] and nothing else: a key this
    /// instance did not touch is whatever the file says when the save lands,
    /// which is what another window, the command line or a person's editor
    /// may have put there since this was read. See
    /// [`Settings::write_changes`]. A window, which saves many times, uses
    /// [`Settings::take_changes`] instead, so that each save carries only what
    /// changed since the one before.
    ///
    /// A crash part-way through leaves the previous file intact: the bytes go
    /// to a temporary beside the target and only a rename — one operation the
    /// filesystem either does or does not do — puts them in place.
    pub fn save_blocking(&self) -> Result<()> {
        let path = self
            .path
            .as_ref()
            .context("there is no per-user configuration directory to save into")?;
        Self::write_changes(path, &self.changes(), &self.document)
    }

    /// What this instance says differently from the file it last agreed
    /// with: every key a person changed here since it was loaded or since the
    /// last [`Settings::take_changes`].
    ///
    /// By key, and finer than a key where a key holds one answer per plugin
    /// or per agent: a grant revoked here and a grant given in another window
    /// are two different plugins' entries under `plugin_grants`, and both
    /// have to survive.
    pub fn changes(&self) -> Changes {
        Changes::between(&self.base, &self.owned())
    }

    /// [`Settings::changes`], after which this instance agrees with the file
    /// again: the next call says only what changed after this one.
    ///
    /// For a window, which saves on every click. What this hands back is the
    /// window's to keep until a save has written it — a save that fails, or
    /// that a later one overtook, must not be what loses it.
    pub fn take_changes(&mut self) -> Changes {
        let owned = self.owned();
        let changes = Changes::between(&self.base, &owned);
        self.base = owned;
        changes
    }

    /// The settings with a newer file under them: what the file says now,
    /// with `pending` — changes made here and not yet written — and whatever
    /// this instance changed since it last agreed with the file, both laid
    /// over it.
    ///
    /// What a window does when the file changes under it: another window
    /// saved, the command line uninstalled a plugin, a person edited the file
    /// in an editor. Every key this window did not change takes the file's
    /// value — a grant revoked anywhere is revoked here — and the ones it did
    /// change keep this window's, because those are on their way to the file
    /// and will be what it says once they land.
    pub fn adopt(&mut self, document: Map<String, Value>, pending: &Changes) {
        let unsaved = self.changes();
        let mut agreed = document.clone();
        pending.apply_to(&mut agreed);
        let base = Self::from_document(self.path.clone(), agreed.clone()).base;
        let mut wanted = agreed;
        unsaved.apply_to(&mut wanted);

        let mut adopted = Self::from_document(self.path.clone(), wanted);
        adopted.document = document;
        adopted.base = base;
        *self = adopted;
    }

    /// Lays `changes` over the file at `path` as it is now, and writes the
    /// result back atomically.
    ///
    /// **One window at a time.** Two windows saving at once would each read
    /// the file, lay their own change over it, and the second rename would
    /// take the first one's change back out. So the read and the write are
    /// one step under an advisory lock on [`LOCK_FILE`] beside the file, which
    /// every Crook takes before it saves; a window that finds it held waits
    /// its turn. See [`hold_the_lock`] for what happens when there is no lock
    /// to be had.
    ///
    /// A file that is not there is an empty one: the changes alone are
    /// written. A file that does not read — a hand edit with a trailing comma,
    /// half a file from a tool that does not write atomically — is copied,
    /// byte for byte, into [`BACKUP_DIRECTORY`] beside it, and the changes are
    /// laid over `fallback` instead: the file as this window last read it
    /// whole. The person's own text is kept where they can find it, and the
    /// options they had are not traded for the one they just changed.
    ///
    /// Nothing changed is nothing written, so a save that has nothing to say
    /// leaves a person's file — its formatting, its key order — as it is.
    pub fn write_changes(
        path: &Path,
        changes: &Changes,
        fallback: &Map<String, Value>,
    ) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        if let Some(directory) = path.parent() {
            ensure_directory(directory)?;
        }

        let _held = hold_the_lock(path, LOCK_FILE);
        let target = &through_a_link(path);
        let mut document = match fs::read(target) {
            Ok(bytes) => match parse_document(&bytes) {
                Ok(document) => document,
                Err(why) => {
                    log::warn!(
                        "{} does not read ({why}); writing over the last copy that did",
                        path.display()
                    );
                    back_up_unreadable(&bytes, target);
                    fallback.clone()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
            // A file that is there and cannot be read is not one to write
            // over blind: whatever it holds is somebody's, and the changes
            // are kept for the next save to try again.
            Err(error) => {
                return Err(error).with_context(|| format!("could not read {}", path.display()));
            }
        };
        changes.apply_to(&mut document);

        let mut json = serde_json::to_string_pretty(&Value::Object(document))
            .context("could not serialize the settings")?;
        // A file a person may open in an editor ends in a newline.
        json.push('\n');
        write_then_rename(&temporary_path(target), target, json.as_bytes())
    }

    /// The keys this build owns, as a save writes them.
    ///
    /// Both groups are flat in the same object, which is what lets a person
    /// find `start_login_shell` beside `font_size` in a file they opened in an
    /// editor. It also means the two structs may not name the same key twice —
    /// the later `extend` would silently win — and that is a thing to check
    /// when a third group appears rather than a thing to defend against here.
    ///
    /// A key that is absent here is one a save takes out of the file, when
    /// this instance is what changed it: see [`Changes`].
    fn owned(&self) -> Map<String, Value> {
        let mut document = Map::new();
        for group in [
            owned_keys(self.tab_options, "tab options"),
            owned_keys(self.general, "general options"),
        ] {
            // Cannot fail for a struct of plain fields; if it ever did, the
            // group would be missing from both sides of a comparison alike
            // and so from no save.
            match group {
                Ok(keys) => document.extend(keys),
                Err(error) => log::warn!("{error:#}"),
            }
        }
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
        // Written back only when something has been allowed, for the reason
        // the list above is: a person who has installed no plugin should not
        // find an empty object in a file they opened to read.
        if self.plugin_grants.is_empty() {
            document.remove(PLUGIN_GRANTS_KEY);
        } else {
            document.insert(
                PLUGIN_GRANTS_KEY.to_owned(),
                Value::Object(
                    self.plugin_grants
                        .iter()
                        .map(|(plugin, keys)| {
                            (
                                plugin.clone(),
                                Value::Array(
                                    keys.iter().map(|key| Value::String(key.clone())).collect(),
                                ),
                            )
                        })
                        .collect(),
                ),
            );
        }
        // Written back only when a person has written one, for the reason
        // the grants are — and so that a build's own lines are never frozen
        // into the file: see `Settings::resume_lines`.
        if self.resume_lines.is_empty() {
            document.remove(RESUME_LINES_KEY);
        } else {
            document.insert(
                RESUME_LINES_KEY.to_owned(),
                Value::Object(
                    self.resume_lines
                        .iter()
                        .map(|(program, line)| (program.clone(), Value::String(line.clone())))
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
        document
    }
}

/// What one window changed in the settings, to be laid over the file as it
/// is when the save lands.
///
/// Three grains, because the keys this build owns hold three kinds of thing.
/// Most hold one answer, and a change to one is the key's new value or its
/// absence. Two hold an answer *per plugin* or *per agent* — the grants and
/// the resume lines — and a change there is one entry's: a window that
/// revoked one plugin has said nothing about another, and a save that wrote
/// the whole object back would take back a grant given elsewhere in the
/// meantime. The list of plugins switched off is a set, and a change to it is
/// one name going on or coming off.
///
/// A later change to the same key, entry or name replaces an earlier one:
/// see [`Changes::then`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    /// Each key's new value, or `None` for a key the file should not have.
    values: BTreeMap<String, Option<Value>>,
    /// Each entry's, by the key it is under and its own name.
    entries: BTreeMap<(String, String), Option<Value>>,
    /// Each plugin switched off (`true`) or back on (`false`).
    switched_off: BTreeMap<String, bool>,
}

/// The keys whose value is an object of one answer per name.
const ENTRY_KEYS: [&str; 2] = [PLUGIN_GRANTS_KEY, RESUME_LINES_KEY];

impl Changes {
    /// Whether there is nothing to write.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.entries.is_empty() && self.switched_off.is_empty()
    }

    /// What turns the keys in `before` into the keys in `after`, both as
    /// [`Settings::owned`] renders them.
    fn between(before: &Map<String, Value>, after: &Map<String, Value>) -> Self {
        let mut changes = Self::default();
        let keys: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
        for key in keys {
            let (was, is) = (before.get(key), after.get(key));
            if key == DISABLED_PLUGINS_KEY {
                let (was, is) = (names_in(was), names_in(is));
                for name in is.difference(&was) {
                    changes.switched_off.insert(name.clone(), true);
                }
                for name in was.difference(&is) {
                    changes.switched_off.insert(name.clone(), false);
                }
            } else if ENTRY_KEYS.contains(&key.as_str()) {
                let empty = Map::new();
                let was = was.and_then(Value::as_object).unwrap_or(&empty);
                let is = is.and_then(Value::as_object).unwrap_or(&empty);
                let names: std::collections::BTreeSet<&String> =
                    was.keys().chain(is.keys()).collect();
                for name in names {
                    if was.get(name) != is.get(name) {
                        changes
                            .entries
                            .insert((key.clone(), name.clone()), is.get(name).cloned());
                    }
                }
            } else if was != is {
                changes.values.insert(key.clone(), is.cloned());
            }
        }
        changes
    }

    /// Adds `later` on top of these: where both say something about the same
    /// key, entry or name, `later` is what stands.
    pub fn then(&mut self, later: Changes) {
        self.values.extend(later.values);
        self.entries.extend(later.entries);
        self.switched_off.extend(later.switched_off);
    }

    /// Puts back `earlier` — changes a save took and could not write — under
    /// these: where both say something, these are newer and stand.
    pub fn under(&mut self, earlier: Changes) {
        let mut restored = earlier;
        restored.then(std::mem::take(self));
        *self = restored;
    }

    /// Lays these over `document`.
    ///
    /// The object keys and the list are left out of the file once nothing
    /// is in them, which is the rule [`Settings::owned`] writes them by: a
    /// person who never allowed a plugin does not find an empty object in a
    /// file they opened to read.
    fn apply_to(&self, document: &mut Map<String, Value>) {
        for (key, value) in &self.values {
            match value {
                Some(value) => document.insert(key.clone(), value.clone()),
                None => document.remove(key),
            };
        }

        for key in ENTRY_KEYS {
            let mut changed = self
                .entries
                .iter()
                .filter(|((under, _), _)| under == key)
                .peekable();
            if changed.peek().is_none() {
                continue;
            }
            // What the file has under the key, if it is an object; one that is
            // not was unreadable on every load, and is replaced.
            let mut object = document
                .get(key)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for ((_, name), value) in changed {
                match value {
                    Some(value) => object.insert(name.clone(), value.clone()),
                    None => object.remove(name),
                };
            }
            match object.is_empty() {
                true => document.remove(key),
                false => document.insert(key.to_owned(), Value::Object(object)),
            };
        }

        if !self.switched_off.is_empty() {
            let mut names = names_in(document.get(DISABLED_PLUGINS_KEY));
            for (name, off) in &self.switched_off {
                match off {
                    true => names.insert(name.clone()),
                    false => names.remove(name),
                };
            }
            match names.is_empty() {
                true => document.remove(DISABLED_PLUGINS_KEY),
                false => document.insert(
                    DISABLED_PLUGINS_KEY.to_owned(),
                    Value::Array(names.into_iter().map(Value::String).collect()),
                ),
            };
        }
    }
}

/// The plugin names in a `disabled_plugins` value, read the way
/// [`Settings::load`] reads them: strings only, trimmed, none empty.
fn names_in(value: Option<&Value>) -> std::collections::BTreeSet<String> {
    value
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
        .unwrap_or_default()
}

/// One group of options out of a settings document, defaulting past anything
/// unusable.
///
/// `group` names the group in the warning, which is the only thing that tells
/// a person which half of their file the parser gave up on.
fn parse_group<T: Default + Serialize + for<'de> Deserialize<'de>>(
    document: &Map<String, Value>,
    path: &Path,
    group: &str,
) -> T {
    match serde_json::from_value(Value::Object(document.clone())) {
        Ok(parsed) => parsed,
        Err(_) => parse_group_key_by_key(document, path, group),
    }
}

/// The same group, one key at a time, once the whole did not read.
///
/// A file is hand-edited: `"login_shell": "yes"` where a `true` was wanted,
/// or a `view_mode` this build has never heard of. Serde refuses the whole
/// struct for one such field, and defaulting the group for it took the type
/// size down with a typo in the login switch — the very thing the two groups
/// being parsed separately was meant to prevent, one level up. So each key
/// the group owns is tried on its own against the defaults, and the one that
/// does not read is the one that costs its default, with a line in the log
/// that names it.
fn parse_group_key_by_key<T: Default + Serialize + for<'de> Deserialize<'de>>(
    document: &Map<String, Value>,
    path: &Path,
    group: &str,
) -> T {
    let Ok(mut kept) = owned_keys(T::default(), group) else {
        return T::default();
    };
    for key in kept.keys().cloned().collect::<Vec<_>>() {
        let Some(value) = document.get(&key) else {
            continue;
        };
        let mut candidate = kept.clone();
        candidate.insert(key.clone(), value.clone());
        match serde_json::from_value::<T>(Value::Object(candidate)) {
            Ok(_) => {
                kept.insert(key, value.clone());
            }
            Err(err) => log::warn!(
                "{} holds a `{key}` this build cannot read ({err}); using its default",
                path.display()
            ),
        }
    }
    serde_json::from_value(Value::Object(kept)).unwrap_or_default()
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

/// Makes `directory`, and whatever is missing above it, or says what stood
/// in the way.
///
/// `create_dir_all` refused by a *file* where the folder should be says
/// "File exists" — or "Not a directory", for a folder wanted under that
/// file — which reads as the opposite of the problem or as none. Every file
/// Crook writes under the configuration directory goes through here, and
/// each of their failures is a sentence on screen now (the settings page,
/// the tab list, the theme creator), so the sentence names the thing that
/// is wrong: the nearest path that exists, and that it is a file.
pub fn ensure_directory(directory: &Path) -> Result<()> {
    if let Some(blocking) = directory.ancestors().find(|ancestor| ancestor.exists())
        && !blocking.is_dir()
    {
        bail!("{} is a file, not a folder", blocking.display());
    }
    fs::create_dir_all(directory)
        .with_context(|| format!("could not create {}", directory.display()))
}

/// Writes `contents` to `path` without ever leaving a half-written file there.
///
/// The bytes go to a temporary beside the target and only a rename — one
/// operation the filesystem either does or does not do — puts them in place,
/// so a crash part-way through leaves whatever was there before intact. Shared
/// with [`crate::session`], which wants exactly the same promise about a file
/// that is written far more often than this one.
pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let path = &through_a_link(path);
    write_then_rename(&temporary_path(path), path, contents)
}

/// The file a write to `path` should land in: the one a symlink there points
/// at, or `path` itself.
///
/// A rename replaces what is at its destination, and what is at a symlink's
/// path is the link. Somebody who keeps `settings.json` or `keybindings.json`
/// in a dotfiles repository and links it here would otherwise find the link
/// swapped for a plain file by the first option they changed — their edits
/// no longer reaching the repository, and the repository's no longer reaching
/// Crook. So the temporary and the rename go beside the link's target.
///
/// Only a link is resolved, so every other path is written exactly where it
/// was named. A link to nothing is written over as before: there is no file
/// of theirs to keep.
fn through_a_link(path: &Path) -> PathBuf {
    let is_a_link = fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink());
    match is_a_link {
        true => fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()),
        false => path.to_owned(),
    }
}

/// Reads `path` as a JSON object, treating every way that can fail as an empty
/// one.
///
/// An empty document is every option at its default, which is the right
/// answer at startup for each of these cases. The next save replaces an
/// unusable file with one that parses, and keeps a copy of what was there
/// first — see [`Settings::write_changes`].
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

    match parse_document(contents.as_bytes()) {
        Ok(document) => document,
        Err(why) => {
            log::warn!("{} {why}; using defaults", path.display());
            Map::new()
        }
    }
}

/// A settings file's bytes as the object they hold, or why they do not hold
/// one — in words that follow the file's name in a sentence.
pub(crate) fn parse_document(bytes: &[u8]) -> std::result::Result<Map<String, Value>, String> {
    match serde_json::from_slice(bytes) {
        Ok(Value::Object(document)) => Ok(document),
        Ok(_) => Err(String::from("is JSON but not an object")),
        Err(error) => Err(format!("could not be parsed ({error})")),
    }
}

/// Holds the lock every save of a file under the configuration directory
/// takes — `name`, beside it — for as long as the returned file is kept.
///
/// An advisory lock on a file of its own — [`LOCK_FILE`] for the settings —
/// taken the way the session's is: through the standard library, `flock` on
/// macOS and Linux and `LockFileEx` on Windows. A file of its own rather than
/// the one being saved, because a save *replaces* that file: a lock on the
/// one that was there would be a lock on a file nobody opens again.
///
/// Waited for, but not forever. A save holds it for the milliseconds a small
/// file takes to read and write, and a shell being started can hold a copy
/// for as long as its fork takes to exec; [`LOCK_PATIENCE`] is far past both.
/// Past it — and on a file system that keeps no locks at all — the save goes
/// ahead without one, which is the way every save went before there was a
/// lock, rather than a click whose change is never written.
pub(crate) fn hold_the_lock(path: &Path, name: &str) -> Option<fs::File> {
    let lock = path.with_file_name(name);
    // Open for writing as well as reading, although nothing is written: NFS
    // on Linux refuses an exclusive lock on a file opened only for reading.
    let file = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock)
    {
        Ok(file) => file,
        Err(error) => {
            log::warn!(
                "could not open {} ({error}); saving without it",
                lock.display()
            );
            return None;
        }
    };
    let deadline = std::time::Instant::now() + LOCK_PATIENCE;
    loop {
        match file.try_lock() {
            Ok(()) => return Some(file),
            Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(fs::TryLockError::WouldBlock) => {
                log::warn!(
                    "{} was held for longer than {LOCK_PATIENCE:?}; saving without it",
                    lock.display()
                );
                return None;
            }
            Err(fs::TryLockError::Error(error)) => {
                log::warn!(
                    "could not lock {} ({error}); saving without it",
                    lock.display()
                );
                return None;
            }
        }
    }
}

/// Keeps a copy of a settings file that did not read, as it was.
///
/// A save writes over a file that does not read — see
/// [`Settings::write_changes`] — and without this a person who saved their
/// editor with a typo in it and then clicked an option in Crook would find
/// their edit gone. The bytes go untouched to
/// `settings-backups/settings-<timestamp>.json` beside the file, and the
/// newest [`BACKUPS_KEPT`] are kept. The same arrangement as the session
/// file's, in a directory of its own so that neither prunes the other's.
///
/// Nothing here can stop the save: a copy that cannot be made is a line in
/// the log.
fn back_up_unreadable(bytes: &[u8], path: &Path) {
    let Some(directory) = path.parent() else {
        return;
    };
    let directory = directory.join(BACKUP_DIRECTORY);
    if let Err(error) = ensure_directory(&directory) {
        log::warn!("could not keep a copy of {}: {error:#}", path.display());
        return;
    }
    // To the millisecond, because two saves over the same broken file are
    // one copy each, and a name that sorts as a date is what lets the pruning
    // below find the oldest by name alone.
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let backup = directory.join(format!("settings-{stamp}.json"));
    if let Err(error) = fs::write(&backup, bytes) {
        log::warn!(
            "could not keep a copy of {} at {}: {error}",
            path.display(),
            backup.display()
        );
        return;
    }
    log::warn!(
        "{} did not read; a copy of it is kept at {}",
        path.display(),
        backup.display()
    );

    let Ok(entries) = fs::read_dir(&directory) else {
        return;
    };
    let mut backups: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with("settings-") && name.ends_with(".json"))
        })
        .collect();
    backups.sort();
    for stale in backups.iter().rev().skip(BACKUPS_KEPT) {
        let _ = fs::remove_file(stale);
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
///
/// Whatever step fails, the temporary goes: nothing ever reads a leftover,
/// and one per failed save accumulates forever. A full disk is the usual
/// way a save fails, and it fails at the write, not at the rename — so the
/// write has to be covered as well.
fn write_then_rename(temporary: &Path, destination: &Path, contents: &[u8]) -> Result<()> {
    let mut file = fs::File::create(temporary)
        .with_context(|| format!("could not create {}", temporary.display()))?;

    // The temporary is a new file with the default mode, and the rename puts
    // it where the old one was — so a `settings.json` somebody had made
    // private with `chmod 600` came back readable by everybody after the
    // first option they changed. The old file's mode goes onto the new one
    // first. Unix only: Windows' one bit is read-only, and a read-only
    // temporary is one the rename and the cleanup below cannot touch.
    #[cfg(unix)]
    if let Ok(existing) = fs::metadata(destination) {
        let _ = file.set_permissions(existing.permissions());
    }

    let written = (|| {
        file.write_all(contents)
            .with_context(|| format!("could not write {}", temporary.display()))?;
        // The rename is atomic with respect to the directory, not to the
        // data: without this, a machine that loses power just after the
        // rename can come back to a settings file full of zeroes.
        file.sync_all()
            .with_context(|| format!("could not flush {}", temporary.display()))
    })();
    // Closed before the rename, or before the removal: Windows will do
    // neither to a file that is still open.
    drop(file);

    written
        .and_then(|()| rename_replacing(temporary, destination))
        .inspect_err(|_| {
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
            show_tab_numbers: true,
            status_marks: StatusMarks::Glyphs,
        }
    }

    #[test]
    fn test_a_file_where_the_folder_should_be_is_named_as_such() {
        // `create_dir_all` says "File exists" for the file itself and "Not a
        // directory" for a folder wanted under it — the first reads as the
        // opposite of the problem and the second names nothing. Both reach a
        // sentence on screen, so both say which path is the file.
        let scratch = ScratchDirectory::new("a-file-not-a-folder");
        let blocking = scratch.path.join("crook");
        fs::write(&blocking, "not a folder\n").expect("the scratch file should be writable");

        let itself = ensure_directory(&blocking).expect_err("a file is not a folder");
        assert_eq!(
            format!("{itself:#}"),
            format!("{} is a file, not a folder", blocking.display())
        );

        let under = ensure_directory(&blocking.join("themes")).expect_err("nor is under one");
        assert_eq!(
            format!("{under:#}"),
            format!("{} is a file, not a folder", blocking.display()),
            "the sentence names the file, not the folder wanted under it"
        );

        // And a folder that can be made is made, with what is missing above.
        let deep = scratch.path.join("a").join("b");
        ensure_directory(&deep).expect("two levels of folder should be creatable");
        assert!(deep.is_dir());
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
        // Crook's own, and off: a fresh install opens looking like Warp.
        assert!(!options.show_tab_numbers);
        assert_eq!(StatusMarks::Dots, options.status_marks);
    }

    #[test]
    fn test_a_desktop_notification_is_on_for_a_question_and_off_for_the_rest() {
        let general = GeneralOptions::default();

        assert!(general.notifies_on(Occasion::NeedsInput));
        assert!(!general.notifies_on(Occasion::Failed));
        assert!(!general.notifies_on(Occasion::LongCommand));
    }

    #[test]
    fn test_each_notification_switch_turns_only_itself() {
        let general = GeneralOptions::default();
        for occasion in [
            Occasion::NeedsInput,
            Occasion::Failed,
            Occasion::LongCommand,
        ] {
            let toggled = general.toggled(occasion);
            assert_ne!(
                toggled.notifies_on(occasion),
                general.notifies_on(occasion),
                "{occasion:?} did not turn"
            );
            assert_eq!(
                toggled.toggled(occasion),
                general,
                "{occasion:?} turned something else with it"
            );
        }
    }

    #[test]
    fn test_the_notification_switches_survive_a_save_and_a_load() {
        let scratch = ScratchDirectory::new("notifications");
        let mut settings = Settings::load(scratch.settings_file());
        settings.set_general(
            settings
                .general()
                .toggled(Occasion::NeedsInput)
                .toggled(Occasion::LongCommand),
        );
        settings.save_blocking().expect("the save should succeed");

        let general = Settings::load(scratch.settings_file()).general();
        assert!(!general.notifies_on(Occasion::NeedsInput));
        assert!(!general.notifies_on(Occasion::Failed));
        assert!(general.notifies_on(Occasion::LongCommand));
    }

    // `/dev/full` answers every write with "no space left on device", which
    // is the way a save really fails; reached through a link, so what the
    // failed save removes is the link and never the device.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_save_that_fails_at_the_write_leaves_no_temporary_behind() {
        let scratch = ScratchDirectory::new("full-disk");
        let temporary = temporary_path(&scratch.settings_file());
        std::os::unix::fs::symlink("/dev/full", &temporary).expect("a link can be made");

        let saved = write_then_rename(&temporary, &scratch.settings_file(), b"{}");

        assert!(saved.is_err(), "a write to a full disk succeeded");
        assert!(
            fs::symlink_metadata(&temporary).is_err(),
            "the temporary outlived the failed save"
        );
        assert!(!scratch.settings_file().exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_a_save_keeps_the_mode_the_file_had() {
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = ScratchDirectory::new("private");
        fs::write(scratch.settings_file(), "{}\n").expect("the file should be writable");
        fs::set_permissions(scratch.settings_file(), fs::Permissions::from_mode(0o600))
            .expect("the mode should be settable");

        let mut settings = Settings::load(scratch.settings_file());
        settings.set_tab_options(everything_flipped());
        settings.save_blocking().expect("the save should succeed");

        let mode = fs::metadata(scratch.settings_file())
            .expect("the file is there")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the save made a private file readable");
    }

    #[cfg(unix)]
    #[test]
    fn test_a_settings_file_that_is_a_link_stays_one_and_its_target_is_written() {
        // A dotfiles repository linked into place: the save lands in the file
        // the link points at, and the link is still a link afterwards.
        let scratch = ScratchDirectory::new("linked");
        let target = scratch.path.join("dotfiles-settings.json");
        fs::write(&target, "{}\n").expect("the target should be writable");
        std::os::unix::fs::symlink(&target, scratch.settings_file())
            .expect("the link should be creatable");

        let mut settings = Settings::load(scratch.settings_file());
        settings.set_tab_options(everything_flipped());
        settings.save_blocking().expect("the save should succeed");

        assert!(
            fs::symlink_metadata(scratch.settings_file())
                .expect("the link should still be there")
                .is_symlink(),
            "the save replaced the link with a file"
        );
        assert_eq!(
            everything_flipped(),
            Settings::load(&target).tab_options(),
            "the save did not reach the file the link points at"
        );
        // And no temporary is left beside either of them.
        assert_eq!(
            scratch.entries(),
            vec![
                "dotfiles-settings.json".to_owned(),
                SETTINGS_FILE.to_owned(),
                LOCK_FILE.to_owned(),
            ]
        );
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
        // would put a key in a file nobody asked for one in. A save that
        // never touched the key leaves it as the person wrote it.
        let empty = ScratchDirectory::new("font-family-empty");
        fs::write(empty.settings_file(), r#"{"font_family": "   "}"#)
            .expect("the file should be writable");
        let mut settings = Settings::load(empty.settings_file());
        assert_eq!(settings.font_family(), None);
        settings.set_theme("Midnight");
        settings.save_blocking().expect("the save should succeed");

        let written = fs::read_to_string(empty.settings_file()).expect("readable");
        assert!(!written.contains("null"), "{written}");
        assert_eq!(Settings::load(empty.settings_file()).font_family(), None);

        // And none at all is written for a file that never had one.
        let absent = ScratchDirectory::new("font-family-absent");
        let mut settings = Settings::load(absent.settings_file());
        settings.set_theme("Midnight");
        settings.save_blocking().expect("the save should succeed");
        let written = fs::read_to_string(absent.settings_file()).expect("readable");
        assert!(!written.contains("font_family"), "{written}");
    }

    #[test]
    fn test_a_theme_name_is_trimmed_and_an_empty_one_is_the_default() {
        // The `theme` key is read the same way `light_theme`, `dark_theme` and
        // `font_family` are: hand-edited, so the space around a name is trimmed
        // — an exact-match theme lookup would miss `" Midnight "` — and an empty
        // one is the default, not a warning on every launch.
        let scratch = ScratchDirectory::new("theme-name");
        fs::write(scratch.settings_file(), r#"{"theme": "  Midnight  "}"#)
            .expect("the file should be writable");
        assert_eq!(
            Settings::load(scratch.settings_file()).theme(),
            "Midnight",
            "the surrounding space is trimmed off a hand-edited name"
        );

        let empty = ScratchDirectory::new("theme-empty");
        fs::write(empty.settings_file(), r#"{"theme": "   "}"#)
            .expect("the file should be writable");
        assert_eq!(
            Settings::load(empty.settings_file()).theme(),
            crate::theme::DEFAULT_NAME,
            "an empty name falls back rather than naming a theme nothing has"
        );
    }

    #[test]
    fn test_option_as_meta_is_written_as_the_five_words_a_person_would_type() {
        // The file is hand-edited, so the spellings are the ones the other
        // terminals' documentation leads a person to: `none` rather than the
        // `neither` the code calls it, because `None` is Alacritty's word.
        for (option, word) in [
            (OptionAsMeta::Auto, "auto"),
            (OptionAsMeta::Left, "left"),
            (OptionAsMeta::Right, "right"),
            (OptionAsMeta::Both, "both"),
            (OptionAsMeta::Neither, "none"),
        ] {
            let scratch = ScratchDirectory::new("option-as-meta");
            // Another word than the one being written, so that writing it is
            // a change and a save has to spell it.
            let other = if word == "left" { "right" } else { "left" };
            fs::write(
                scratch.settings_file(),
                format!(r#"{{"option_as_meta": "{other}"}}"#),
            )
            .expect("the file should be writable");
            let mut settings = Settings::load(scratch.settings_file());
            settings.set_general(GeneralOptions {
                option_as_meta: option,
                ..settings.general()
            });
            settings.save_blocking().expect("the save should succeed");

            let written: Map<String, Value> = serde_json::from_str(
                &fs::read_to_string(scratch.settings_file()).expect("the file should be readable"),
            )
            .expect("the file should be a JSON object");
            assert_eq!(Some(&Value::from(word)), written.get("option_as_meta"));
            assert_eq!(
                Settings::load(scratch.settings_file())
                    .general()
                    .option_as_meta,
                option,
                "{word} did not come back"
            );
        }
    }

    #[test]
    fn test_motion_is_written_as_three_words_and_follows_the_desktop_by_default() {
        assert_eq!(GeneralOptions::default().motion, Motion::System);
        for (motion, word) in [
            (Motion::System, "system"),
            (Motion::Full, "full"),
            (Motion::Reduced, "reduced"),
        ] {
            let scratch = ScratchDirectory::new("motion");
            // Another word than the one being written, for the reason the
            // test above starts from one.
            let other = if word == "full" { "reduced" } else { "full" };
            fs::write(
                scratch.settings_file(),
                format!(r#"{{"motion": "{other}"}}"#),
            )
            .expect("the file should be writable");
            let mut settings = Settings::load(scratch.settings_file());
            settings.set_general(GeneralOptions {
                motion,
                ..settings.general()
            });
            settings.save_blocking().expect("the save should succeed");

            let written: Map<String, Value> = serde_json::from_str(
                &fs::read_to_string(scratch.settings_file()).expect("the file should be readable"),
            )
            .expect("the file should be a JSON object");
            assert_eq!(Some(&Value::from(word)), written.get("motion"));
            assert_eq!(
                Settings::load(scratch.settings_file()).general().motion,
                motion,
                "{word} did not come back"
            );
        }
    }

    #[test]
    fn test_only_the_system_answer_listens_to_the_desktop() {
        for desktop in [false, true] {
            assert_eq!(Motion::System.is_reduced(desktop), desktop);
            assert!(!Motion::Full.is_reduced(desktop));
            assert!(Motion::Reduced.is_reduced(desktop));
        }
    }

    #[test]
    fn test_an_option_as_meta_this_build_cannot_read_is_auto() {
        // `true` is what Ghostty takes, and a person moving a setting across
        // would try it. It costs that key and nothing beside it.
        let scratch = ScratchDirectory::new("option-as-meta-unknown");
        fs::write(
            scratch.settings_file(),
            r#"{"option_as_meta": true, "restore_session": false}"#,
        )
        .expect("the file should be writable");

        let general = Settings::load(scratch.settings_file()).general();
        assert_eq!(general.option_as_meta, OptionAsMeta::Auto);
        assert!(!general.restore_session);
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
        // What changed is what was written, spelled as below.
        assert_eq!(
            [
                "compact_subtitle",
                "display_granularity",
                "primary_info",
                "show_details_on_hover",
                "show_diff_stats",
                "show_pr_link",
                "show_tab_numbers",
                "status_marks",
                "view_mode",
            ],
            written.keys().collect::<Vec<_>>()[..]
        );

        // And every key a save can write, which is the whole vocabulary.
        let owned = settings.owned();
        assert_eq!(
            vec![
                // Crook's own: whether the window takes the questions agents'
                // permission hooks hand it.
                "answer_agents_from_crook",
                // Crook's own: whether a close that would end an agent still
                // working asks first.
                "ask_before_ending_agents",
                // Warp's "audible terminal bell", off as it is there.
                "audible_bell",
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
                // Crook's own, and Linux's: whether a selection is PRIMARY
                // and a middle click pastes it. Written everywhere, read only
                // where a desktop keeps one.
                "middle_click_paste",
                // Crook's own: whether running marks breathe and the caret
                // blinks, or the window keeps still as the desktop asks.
                "motion",
                // Crook's own: which of a pane's stops post a desktop
                // notification while the window is behind another.
                "notify_on_failed",
                "notify_on_long_command",
                "notify_on_needs_input",
                // Crook's own: an entry in the system's own list of programs
                // to open at login.
                "open_at_login",
                // Crook's own spelling of what kitty calls
                // `macos_option_as_alt`: which Option is Meta on a Mac.
                "option_as_meta",
                "primary_info",
                "restore_session",
                "show_details_on_hover",
                "show_diff_stats",
                "show_pr_link",
                // Crook's own: the number a row leads with, which Warp's rows
                // never carry.
                "show_tab_numbers",
                // Crook's own: whether the column of tabs is drawn at all.
                "show_tabs_panel",
                // Crook's own: what the mark at the head of a row is drawn
                // as, which in Warp is not a choice.
                "status_marks",
                // Warp's "Tab key behavior", as one option rather than two
                // rewritten keybindings.
                "tab_key",
                // Crook's own: how wide the column of tabs was dragged.
                "tabs_panel_width",
                // The chosen theme's name, which is a string rather than an
                // option with a type: see `Settings::theme`.
                "theme",
                "use_system_theme",
                "view_mode",
            ],
            owned.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            Some(&Value::from("tabs")),
            written.get("display_granularity")
        );
        assert_eq!(Some(&Value::from("expanded")), written.get("view_mode"));
        assert_eq!(Some(&Value::from("branch")), written.get("primary_info"));
        assert_eq!(Some(&Value::from("glyphs")), written.get("status_marks"));
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
        // is not a reason to throw away a key that a later Crook can. Nor is
        // the value it could not read, while nobody changes it here: it may
        // be one a later Crook can.
        let mut settings = settings;
        settings.set_theme("Midnight");
        settings.save_blocking().expect("the save should succeed");
        let written: Map<String, Value> = serde_json::from_str(
            &fs::read_to_string(scratch.settings_file()).expect("the file should be readable"),
        )
        .expect("the file should be a JSON object");
        assert_eq!(Some(&Value::from(7)), written.get("future_key"));
        assert_eq!(Some(&Value::from("cosy")), written.get("view_mode"));
    }

    #[test]
    fn test_a_key_this_build_cannot_read_costs_that_key_and_not_its_group() {
        // A typo in one key used to default the whole group: `"login_shell":
        // "yes"` took the font size with it, and a `view_mode` from another
        // build took every tab option.
        let scratch = ScratchDirectory::new("one-bad-key");
        fs::write(
            scratch.settings_file(),
            r#"{"font_size": 15.0, "login_shell": "yes", "restore_session": false,
                "view_mode": "cosy", "show_pr_link": false}"#,
        )
        .expect("the file should be writable");

        let settings = Settings::load(scratch.settings_file());
        let general = settings.general();
        assert_eq!(
            general.font_size(),
            15.0,
            "the font size was lost to a typo elsewhere"
        );
        assert!(!general.restore_session, "and so was the session switch");
        assert_eq!(
            general.login_shell,
            GeneralOptions::default().login_shell,
            "the key that could not be read takes its default"
        );

        let options = settings.tab_options();
        assert!(
            !options.show_pr_link,
            "the tab option beside the unknown one was lost"
        );
        assert_eq!(options.density, TabOptions::default().density);
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
        let mut settings = Settings::load(scratch.settings_file());
        settings.set_theme("Midnight");
        settings.save_blocking().expect("the save should succeed");
        settings
            .save_blocking()
            .expect("the second save should succeed");

        // The file, and the lock every save takes beside it.
        assert_eq!(
            vec![SETTINGS_FILE.to_owned(), LOCK_FILE.to_owned()],
            scratch.entries()
        );
    }

    #[test]
    fn test_a_save_replaces_the_file_rather_than_adding_to_it() {
        let scratch = ScratchDirectory::new("replace");
        // A long value this save changes to a short one: a save that only
        // overwrote the front of the file would leave a tail of the old one,
        // and the file would not parse.
        fs::write(
            scratch.settings_file(),
            format!(r#"{{"theme": "{}"}}"#, "x".repeat(8192)),
        )
        .expect("the file should be writable");
        let mut settings = Settings::load(scratch.settings_file());

        settings.set_theme("Midnight");
        settings.save_blocking().expect("the save should succeed");

        let contents =
            fs::read_to_string(scratch.settings_file()).expect("the file should be readable");
        let written: Map<String, Value> =
            serde_json::from_str(&contents).expect("the file should be a JSON object");
        assert_eq!(Some(&Value::from("Midnight")), written.get("theme"));
        assert!(!contents.contains("xxxx"));
    }

    #[test]
    fn test_a_save_writes_what_changed_and_leaves_the_rest_of_the_file_as_it_is_now() {
        // Loaded before the file holds anything, the way a window that opened
        // before another window saved holds a copy from before that save.
        // What this one writes is what it changed — the tab options — laid
        // over the file as it is when it writes, so the key written since
        // survives and none of this window's untouched defaults is written
        // over the other window's choices.
        let scratch = ScratchDirectory::new("changed-only");
        let mut settings = Settings::load(scratch.settings_file());
        fs::write(
            scratch.settings_file(),
            r#"{"padding": "kept", "theme": "Chosen elsewhere", "font_size": 20}"#,
        )
        .expect("the file should be writable");

        settings.set_tab_options(everything_flipped());
        settings.save_blocking().expect("the save should succeed");

        let reloaded = Settings::load(scratch.settings_file());
        assert_eq!(everything_flipped(), reloaded.tab_options());
        assert_eq!("Chosen elsewhere", reloaded.theme());
        assert_eq!(20., reloaded.general().font_size());
        let written = fs::read_to_string(scratch.settings_file()).expect("readable");
        assert!(written.contains(r#""padding": "kept""#), "{written}");
    }

    #[test]
    fn test_a_save_creates_the_directory_it_writes_into() {
        let scratch = ScratchDirectory::new("nested");
        let path = scratch.path.join("nested").join(SETTINGS_FILE);
        let mut settings = Settings::load(&path);
        settings.set_theme("Midnight");

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
    fn test_a_grant_is_read_back_exactly_as_it_was_written() {
        // The space at the end of a command template is part of the command:
        // the plugin asks for `type:git commit -m {} `, and a grant read back
        // as anything else is not a grant for that.
        let scratch = ScratchDirectory::new("grant-exactly");
        let mut settings = Settings::load(scratch.settings_file());
        let keys = vec![
            String::from("type:git commit -m {} "),
            String::from("file: leading space.txt"),
            String::from("net:api.github.com"),
        ];
        settings.set_granted("someone/git", keys.clone());
        settings.save_blocking().expect("the save should succeed");

        let loaded = Settings::load(scratch.settings_file());
        assert_eq!(loaded.granted_to("someone/git"), &keys[..]);
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
    fn test_an_agent_is_resumed_with_its_own_line_until_a_person_writes_another() {
        use crate::agent::Resume;

        let scratch = ScratchDirectory::new("resume-lines");
        let settings = Settings::load(scratch.settings_file());
        assert_eq!(
            settings.resume_line("claude", Resume::Last).as_deref(),
            Some("claude --continue")
        );

        fs::write(
            scratch.settings_file(),
            r#"{"resume_lines": {"claude": "  claude --continue --model opus ",
                                 "codex": "", "opencode": "opencode --continue",
                                 "gemini": 7}}"#,
        )
        .expect("the file should be writable");
        let settings = Settings::load(scratch.settings_file());

        assert_eq!(
            settings.resume_line("claude", Resume::Last).as_deref(),
            Some("claude --continue --model opus"),
            "the person's line replaces the built-in one"
        );
        assert_eq!(
            settings.resume_line("claude", Resume::Pick).as_deref(),
            Some("claude --resume"),
            "and the picker is still the agent's own"
        );
        assert_eq!(
            settings.resume_line("codex", Resume::Last),
            None,
            "an empty line is an agent offered nothing"
        );
        assert_eq!(settings.resume_line("codex", Resume::Pick), None);
        assert_eq!(
            settings.resume_line("opencode", Resume::Last).as_deref(),
            Some("opencode --continue"),
            "an agent with no line of its own can be given one"
        );
        assert_eq!(
            settings.resume_line("gemini", Resume::Last).as_deref(),
            Some("gemini --resume latest"),
            "a value that is not a line costs only itself"
        );
    }

    #[test]
    fn test_resume_lines_are_kept_through_a_save_and_never_written_unasked() {
        use crate::agent::Resume;

        let scratch = ScratchDirectory::new("resume-lines-save");
        let mut settings = Settings::load(scratch.settings_file());
        settings.set_theme("Midnight");
        settings.save_blocking().expect("the save should succeed");
        let text = fs::read_to_string(scratch.settings_file()).expect("readable");
        assert!(
            !text.contains(RESUME_LINES_KEY),
            "the built-in lines were copied into the file, where the next build's \
             corrections would never reach them: {text}"
        );

        fs::write(
            scratch.settings_file(),
            r#"{"resume_lines": {"claude": "claude --continue --model opus"}}"#,
        )
        .expect("the file should be writable");
        let mut settings = Settings::load(scratch.settings_file());
        settings.set_theme("Midnight");
        settings.save_blocking().expect("the save should succeed");
        assert_eq!(
            Settings::load(scratch.settings_file())
                .resume_line("claude", Resume::Last)
                .as_deref(),
            Some("claude --continue --model opus")
        );
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

    /// Two windows' settings, read from one file the way two processes read
    /// it at launch.
    fn two_windows(scratch: &ScratchDirectory) -> (Settings, Settings) {
        (
            Settings::load(scratch.settings_file()),
            Settings::load(scratch.settings_file()),
        )
    }

    #[test]
    fn test_a_revoke_in_one_window_survives_a_save_in_another() {
        // The bug: each window wrote its whole copy, so B's theme save put
        // back the grant A had just revoked, and the next launch ran the
        // plugin with it.
        let scratch = ScratchDirectory::new("revoke-two-windows");
        let mut first = Settings::load(scratch.settings_file());
        first.set_granted("eugen/probe", vec![String::from("clipboard")]);
        first.save_blocking().expect("the grant is saved");

        let (mut a, mut b) = two_windows(&scratch);
        a.set_granted("eugen/probe", Vec::new());
        a.save_blocking().expect("A's revoke is saved");
        b.set_theme("Midnight");
        b.save_blocking().expect("B's theme is saved");

        let after = Settings::load(scratch.settings_file());
        assert!(
            after.granted_to("eugen/probe").is_empty(),
            "B's save gave back the grant A revoked: {:?}",
            after.granted_to("eugen/probe")
        );
        assert_eq!(after.theme(), "Midnight", "A's save lost B's theme");
    }

    #[test]
    fn test_two_windows_changing_different_entries_of_one_key_keep_both() {
        // Finer than a key: a grant given in A and another plugin's in B are
        // both under `plugin_grants`, and a plugin switched off in each is
        // one list.
        let scratch = ScratchDirectory::new("entries-two-windows");
        let (mut a, mut b) = two_windows(&scratch);
        a.set_granted("eugen/one", vec![String::from("tabs.read")]);
        a.set_plugin_disabled("eugen/off-in-a", true);
        a.save_blocking().expect("A saves");
        b.set_granted("eugen/two", vec![String::from("clipboard")]);
        b.set_plugin_disabled("eugen/off-in-b", true);
        b.save_blocking().expect("B saves");

        let after = Settings::load(scratch.settings_file());
        assert_eq!(after.granted_to("eugen/one"), ["tabs.read"]);
        assert_eq!(after.granted_to("eugen/two"), ["clipboard"]);
        assert_eq!(
            after.disabled_plugins(),
            ["eugen/off-in-a", "eugen/off-in-b"]
        );

        // And switching one back on in a window that never saw the other
        // switched off leaves the other off.
        let mut c = Settings::load(scratch.settings_file());
        let mut d = Settings::load(scratch.settings_file());
        c.set_plugin_disabled("eugen/off-in-a", false);
        c.save_blocking().expect("C saves");
        d.set_granted("eugen/one", Vec::new());
        d.save_blocking().expect("D saves");
        let after = Settings::load(scratch.settings_file());
        assert_eq!(after.disabled_plugins(), ["eugen/off-in-b"]);
        assert!(after.granted_to("eugen/one").is_empty());
        assert_eq!(after.granted_to("eugen/two"), ["clipboard"]);
    }

    #[test]
    fn test_saves_from_many_windows_at_once_all_land() {
        // The read and the write are one step under the lock: without it,
        // two saves that read the file at the same moment each write their
        // own change over what the other did not see, and one is lost.
        let scratch = ScratchDirectory::new("concurrent");
        const WINDOWS: usize = 8;
        const ROUNDS: usize = 6;
        let start = std::sync::Arc::new(std::sync::Barrier::new(WINDOWS));
        let threads: Vec<_> = (0..WINDOWS)
            .map(|window| {
                let path = scratch.settings_file();
                let start = start.clone();
                std::thread::spawn(move || {
                    let mut settings = Settings::load(&path);
                    start.wait();
                    for round in 0..ROUNDS {
                        settings.set_granted(
                            &format!("eugen/window-{window}"),
                            vec![format!("round-{round}")],
                        );
                        let changes = settings.take_changes();
                        Settings::write_changes(&path, &changes, &Map::new())
                            .expect("the save lands");
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("no window panicked");
        }

        let after = Settings::load(scratch.settings_file());
        for window in 0..WINDOWS {
            assert_eq!(
                after.granted_to(&format!("eugen/window-{window}")),
                [format!("round-{}", ROUNDS - 1)],
                "window {window}'s last save was lost; the file holds {:?}",
                after.plugin_grants()
            );
        }
    }

    #[test]
    fn test_a_save_over_a_file_that_does_not_read_keeps_a_copy_and_every_option() {
        // A person saved their editor with a trailing comma in it. The next
        // option clicked in Crook writes a file that reads — with the
        // options the window last read, not only the one clicked — and the
        // person's text is kept where they can find it.
        let scratch = ScratchDirectory::new("unreadable-at-save");
        fs::write(
            scratch.settings_file(),
            r#"{"theme": "Midnight", "font_size": 20}"#,
        )
        .expect("the file should be writable");
        let mut settings = Settings::load(scratch.settings_file());
        let broken = r#"{"theme": "Midnight", "font_size": 21,}"#;
        fs::write(scratch.settings_file(), broken).expect("the file should be writable");

        settings.set_granted("eugen/probe", vec![String::from("tabs.read")]);
        settings.save_blocking().expect("the save should succeed");

        let after = Settings::load(scratch.settings_file());
        assert_eq!(after.theme(), "Midnight", "the theme was lost");
        assert_eq!(after.general().font_size(), 20., "the type size was lost");
        assert_eq!(after.granted_to("eugen/probe"), ["tabs.read"]);
        let copies: Vec<_> = fs::read_dir(scratch.path.join(BACKUP_DIRECTORY))
            .expect("a copy was kept")
            .map(|entry| entry.expect("readable").path())
            .collect();
        assert_eq!(copies.len(), 1, "{copies:?}");
        assert_eq!(
            fs::read_to_string(&copies[0]).expect("readable"),
            broken,
            "the copy is not the person's text"
        );
    }

    #[test]
    fn test_a_save_with_nothing_changed_leaves_the_file_as_it_is() {
        // Their formatting and their key order: a save that has nothing to
        // say does not rewrite a file a person keeps by hand.
        let scratch = ScratchDirectory::new("nothing-changed");
        let text = "{ \"theme\":   \"Midnight\" }";
        fs::write(scratch.settings_file(), text).expect("the file should be writable");
        let settings = Settings::load(scratch.settings_file());
        settings
            .save_blocking()
            .expect("nothing to save is no error");
        assert_eq!(
            fs::read_to_string(scratch.settings_file()).expect("readable"),
            text
        );
    }

    #[test]
    fn test_a_file_edited_by_hand_is_adopted_and_not_overwritten_by_the_next_save() {
        let scratch = ScratchDirectory::new("hand-edit");
        let mut window = Settings::load(scratch.settings_file());
        fs::write(
            scratch.settings_file(),
            r#"{"theme": "Edited by hand", "plugin_grants": {"eugen/probe": ["tabs.read"]}}"#,
        )
        .expect("the file should be writable");
        let document = read_document(&scratch.settings_file());

        window.adopt(document, &Changes::default());
        assert_eq!(window.theme(), "Edited by hand");
        assert_eq!(window.granted_to("eugen/probe"), ["tabs.read"]);
        assert!(window.changes().is_empty(), "adopting is not a change");

        // The next thing this window changes is all it writes.
        window.set_granted("eugen/probe", Vec::new());
        window.save_blocking().expect("the save should succeed");
        let after = Settings::load(scratch.settings_file());
        assert_eq!(after.theme(), "Edited by hand");
        assert!(after.granted_to("eugen/probe").is_empty());
    }

    #[test]
    fn test_adopting_a_newer_file_keeps_what_this_window_has_not_written_yet() {
        // A tabs panel being dragged is a width changed and not yet saved,
        // and a save on its way is a change the file does not say yet: a
        // look at the file in the middle of either must not take it back.
        let scratch = ScratchDirectory::new("adopt-unsaved");
        let mut window = Settings::load(scratch.settings_file());
        window.set_theme("Mine, on its way");
        let pending = window.take_changes();
        window.set_general(GeneralOptions {
            tabs_panel_width: 300.,
            ..window.general()
        });

        let mut elsewhere = Map::new();
        elsewhere.insert(THEME_KEY.to_owned(), Value::from("From another window"));
        elsewhere.insert("font_size".to_owned(), Value::from(20));
        window.adopt(elsewhere, &pending);

        assert_eq!(window.theme(), "Mine, on its way");
        assert_eq!(window.general().tabs_panel_width, 300.);
        assert_eq!(window.general().font_size(), 20.);
        // The width is still this window's to save; the theme is the
        // pending save's, and is not said twice.
        let changes = window.changes();
        assert!(
            changes.values.contains_key("tabs_panel_width"),
            "{changes:?}"
        );
        assert!(!changes.values.contains_key(THEME_KEY), "{changes:?}");
    }

    #[test]
    fn test_changes_put_back_under_newer_ones_lose_to_them() {
        let mut older = Changes::default();
        older
            .values
            .insert("theme".to_owned(), Some(Value::from("old")));
        older
            .values
            .insert("font_size".to_owned(), Some(Value::from(20)));
        let mut newer = Changes::default();
        newer
            .values
            .insert("theme".to_owned(), Some(Value::from("new")));

        newer.under(older.clone());
        assert_eq!(newer.values["theme"], Some(Value::from("new")));
        assert_eq!(newer.values["font_size"], Some(Value::from(20)));

        let mut later = older;
        later.then(newer.clone());
        assert_eq!(later, newer);
    }
}
