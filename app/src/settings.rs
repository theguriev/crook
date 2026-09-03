//! The tab strip's options, and the file that makes them outlive a launch.
//!
//! Six things the options menu writes — what a row stands for, how tall it is,
//! what its title says, what its second line says, which chips it carries, and
//! whether hovering it opens a detail card — plus their home on disk. The
//! defaults are Warp's, value for value, because the menu is Warp's: a Crook
//! that opened with different ones would be a different feature wearing the
//! same labels.
//!
//! # Why JSON, and why one file
//!
//! Warp stores these in the user's TOML under `appearance.vertical_tabs.*`,
//! reached through a settings-schema system that gives every key a type, a
//! default, a migration path and a cloud-sync policy. Crook has none of that,
//! and seven keys do not earn it. JSON is what `serde_json` — already in the
//! workspace for the usage client — reads and writes with no further
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

/// What one row of the tab strip stands for — the menu's "View as".
///
/// Warp's `VerticalTabsDisplayGranularity`, under
/// `appearance.vertical_tabs.display_granularity`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// Where the tabs live, and therefore how the whole window is arranged.
///
/// Warp spells this as the boolean `use_vertical_tabs`, gated behind a feature
/// flag, and **defaults it to false**. Crook defaults to [`Layout::Vertical`],
/// and that is the one value in this file that deliberately departs from
/// Warp's: the person Crook is being built for asked for the panel to be what
/// a fresh install opens in. It is written down here so the divergence reads
/// as a decision rather than as a defaults table somebody got wrong.
///
/// The key on disk is `layout` rather than Warp's `use_vertical_tabs`, because
/// a boolean named after one of its two states reads backwards the moment the
/// other state is the default — `"use_vertical_tabs": true` as the value you
/// get by *not* writing it is a sentence nobody can check.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// A panel down the left edge, the full height of the window, with the
    /// header and the body beside it.
    #[default]
    Vertical,
    /// A strip across the header, with the body under it.
    Horizontal,
}

impl Layout {
    /// The other one. What the keybinding and the toggle action produce.
    pub fn toggled(self) -> Self {
        match self {
            Self::Vertical => Self::Horizontal,
            Self::Horizontal => Self::Vertical,
        }
    }
}

/// Which fact a row leads with — the menu's "Pane title as".
///
/// Warp's `VerticalTabsPrimaryInfo`, under
/// `appearance.vertical_tabs.primary_info`. Whichever of the three this names
/// takes the title line, and the remaining two fill the lines below it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Where the tabs live. The one option whose default is not Warp's, and
    /// [`Layout`] says why.
    pub layout: Layout,
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
            layout: Layout::default(),
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
        let tab_options = match serde_json::from_value(Value::Object(document.clone())) {
            Ok(tab_options) => tab_options,
            Err(err) => {
                log::warn!(
                    "{} holds tab options this build cannot read ({err}); using defaults",
                    path.display()
                );
                TabOptions::default()
            }
        };

        Self {
            path: Some(path),
            document,
            tab_options,
        }
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
    fn merged_document(&self) -> Result<Map<String, Value>> {
        let owned = serde_json::to_value(self.tab_options)
            .context("could not serialize the tab options")?;
        let Value::Object(owned) = owned else {
            bail!("the tab options serialized to something other than a JSON object");
        };

        let mut document = self.document.clone();
        document.extend(owned);
        Ok(document)
    }
}

/// `<configuration directory>/crook/settings.json`, where there is one.
pub fn user_settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join(CONFIG_DIRECTORY).join(SETTINGS_FILE))
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
            layout: Layout::Horizontal,
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
    fn test_the_defaults_are_warps_defaults_except_for_the_layout() {
        let options = TabOptions::default();

        // The deliberate departure, asserted rather than described: Warp opens
        // horizontal, Crook opens with the panel because that is what was
        // asked for. A change here is a change of product, not of tidiness.
        assert_eq!(Layout::Vertical, options.layout);

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
                "display_granularity",
                "layout",
                "primary_info",
                "show_details_on_hover",
                "show_diff_stats",
                "show_pr_link",
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
        assert_eq!(Some(&Value::from("horizontal")), written.get("layout"));
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

        assert_eq!(8, written.len());
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
}
