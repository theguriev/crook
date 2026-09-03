//! The palette every view reads, and the one place it can be changed.
//!
//! Twenty-one named roles. A view asks for a role — `theme().surface`, never a
//! hex literal — which is what makes a second palette possible at all: the
//! whole of "Crook has themes" is that [`theme`] returns a different [`Theme`]
//! than it did before.
//!
//! # Why a global, and what the alternative was
//!
//! The theme is read on the render path by free functions that have no context
//! to look anything up in. `row_content`, `settings_page::widgets` and half of
//! `tabs_panel` are plain functions over borrowed data — that is deliberate,
//! and it is what keeps a row renderer testable — so threading a `&Theme`
//! through them means a parameter on sixty signatures that every call site
//! then has to pass along.
//!
//! Warp does not have that problem: its render functions all hold an
//! `AppContext`, so its appearance layer is a singleton entity read the same
//! way every other model is. Crook's are not, so the current theme is a
//! process-wide value behind a lock, read by copy.
//!
//! The value is **per-thread**, and that is not a compromise but the correct
//! scope. Everything that reads a palette reads it while building a frame or
//! while building the terminal palette a frame is drawn through, and both
//! happen on the thread that owns the window; nothing on the background pool
//! has ever asked for a colour. Meanwhile a test binary runs its tests on many
//! threads against one process, so a process-wide theme would mean a test that
//! chooses a light palette repainting the window of every test running beside
//! it — which is a suite that fails one run in two for reasons that have
//! nothing to do with the change being tested.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use crookui_core::geometry::Color;

mod builtin;
pub mod creator;
mod file;
mod omarchy;

pub use builtin::{BUILTIN, Builtin, DARK, builtin_named};
pub use file::{ThemeFile, load_themes_in, load_user_themes, user_themes_directory, write_theme};

/// A palette, in the roles the interface asks for.
///
/// [`Copy`] because that is how it is read: [`theme`] hands back the whole
/// palette by value rather than a borrow of a locked one, so a renderer can
/// hold it across a call without holding the lock across a frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Theme {
    /// The window's own background, behind everything.
    pub ground: Color,
    /// Raised surfaces: the header, the tabs panel, a pane's ground.
    pub surface: Color,
    /// The ground of something that floats over the whole window — the tab
    /// options menu, the hover detail card.
    ///
    /// Warp derives this (`neutral_1`: the background composited with the
    /// foreground at 5% and flattened) because its popup has to read over any
    /// terminal theme a person has loaded. Crook composites it the same way,
    /// in [`Theme::derived`], for the same reason.
    pub surface_raised: Color,
    /// The foreground at 5%. A hovered menu row.
    pub overlay_1: Color,
    /// The foreground at 10%. A segmented control's track, a menu divider.
    pub overlay_2: Color,
    /// The foreground at 15%. The selected pill inside a segmented control.
    pub overlay_3: Color,
    /// The selected tab's fill.
    pub tab_active: Color,
    /// An unselected tab's fill.
    pub tab_inactive: Color,
    /// Hairlines: panel edges, the header's underline, tab outlines.
    pub border: Color,
    /// Text that is being read.
    pub text_primary: Color,
    /// Text that is available to be read.
    pub text_muted: Color,
    /// The one saturated colour, for what the app is currently doing.
    pub accent: Color,
    /// Session usage below half.
    pub usage_normal: Color,
    /// Session usage between half and 80%.
    pub usage_elevated: Color,
    /// Session usage between 80% and 95%.
    pub usage_high: Color,
    /// Session usage above 95%.
    pub usage_critical: Color,
    /// Added lines, in a row's diff-stats chip.
    pub diff_added: Color,
    /// Removed lines, in a row's diff-stats chip.
    pub diff_removed: Color,
    /// The sixteen ANSI colours a program asks for by number, and what the
    /// grid is set in when it asks for nothing.
    pub terminal: TerminalColors,
    /// Whether this palette is a light one.
    ///
    /// *Inferred*, never declared. Warp's theme files carry no light/dark
    /// field at all — it asks whether the foreground reads better against
    /// white or against black — and that is the better design for the same
    /// reason it is the cheaper one: a theme somebody else wrote cannot get it
    /// wrong, because nobody wrote it down.
    ///
    /// Nothing in the derivation below consults this. It is here for what
    /// wants to *group* themes: the settings page lists the light ones
    /// together, and pairing a light theme with a dark one for an OS that
    /// switches between them needs to know which is which.
    pub is_light: bool,
}

/// What a terminal grid is painted in.
///
/// Separate from the interface roles above because it is a different
/// vocabulary with a different owner: a program picks these by number and by
/// escape sequence, and the interface never asks for "colour 9".
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TerminalColors {
    /// Text the program did not colour.
    pub foreground: Color,
    /// The grid behind it.
    ///
    /// Also the pane's own ground: `body::pane_ground` paints a pane in
    /// whatever its grid resolved, so these are one surface rather than two
    /// that nearly match.
    pub background: Color,
    /// The cursor block.
    pub cursor: Color,
    /// Colours 0-7: black, red, green, yellow, blue, magenta, cyan, white.
    pub normal: [Color; 8],
    /// Colours 8-15, the bright half of the same eight.
    pub bright: [Color; 8],
}

impl Theme {
    /// Fills in every role a theme file does not carry.
    ///
    /// Warp's theme files name a handful of colours — an accent, a background,
    /// a foreground and the sixteen terminal ones — and derive the rest by
    /// compositing the foreground over the background at fixed percentages.
    /// Doing the same here is what keeps a theme file twenty lines rather than
    /// sixty, and it is why a theme written for Warp can be read by Crook at
    /// all.
    ///
    /// **The background comes from `terminal`, and there is no second one.**
    /// A pane is painted in whatever background its grid resolved, so a theme
    /// with one background for its chrome and another for its grid would draw
    /// the second inside the first with eight pixels of the first showing
    /// around it — the frame this application spent two commits removing. One
    /// colour, used for both, makes that unrepresentable rather than merely
    /// avoided.
    pub const fn derived(accent: Color, terminal: TerminalColors) -> Self {
        let background = terminal.background;
        let foreground = terminal.foreground;

        Self {
            ground: receded(background),
            // The chrome and the grid are one surface. See above.
            surface: background,
            surface_raised: composite(background, foreground, 5),
            // A light theme's overlays darken and a dark theme's lighten,
            // because both are the *foreground* at a percentage — Warp
            // resolves the same ladder the same way, so this is that rule with
            // the foreground named rather than assumed.
            overlay_1: foreground.with_alpha(percent_of_255(5)),
            overlay_2: foreground.with_alpha(percent_of_255(10)),
            overlay_3: foreground.with_alpha(percent_of_255(15)),
            tab_active: composite(background, foreground, 8),
            tab_inactive: receded(background),
            border: composite(background, foreground, 12),
            text_primary: foreground,
            // Further towards the text on a light palette than on a dark one.
            // The eye is not symmetric about this: lightening dark text on a
            // white ground loses contrast much faster than darkening light
            // text on a black one, and 55% — which reads as "quieter" on a
            // dark theme — falls to about 2.5:1 on a light one, under every
            // threshold there is.
            text_muted: composite(background, foreground, muted_mix(foreground)),
            accent,
            // The usage bands and the diff chips are read as signals rather
            // than as part of the palette — "critical" has to be red on every
            // theme — so they come from the theme's own ANSI colours, which is
            // where a theme says what it thinks red is.
            usage_normal: composite(background, foreground, muted_mix(foreground)),
            usage_elevated: terminal.normal[3],
            usage_high: terminal.bright[3],
            usage_critical: terminal.normal[1],
            diff_added: terminal.normal[2],
            diff_removed: terminal.normal[1],
            terminal,
            // Warp's rule, in integer arithmetic: a theme whose text is dark
            // is a theme meant for a light background.
            is_light: luminance(foreground) < 128,
        }
    }
}

/// How far muted text is mixed towards the text it is quieter than.
const fn muted_mix(foreground: Color) -> u8 {
    if luminance(foreground) < 128 { 72 } else { 62 }
}

/// How bright a colour reads, 0 to 255.
///
/// The Rec. 709 luma weights, in integers so this can run in a `const fn`. It
/// is not WCAG relative luminance — that needs a gamma curve and floating
/// point — and it does not need to be: the one question asked of it is which
/// side of the middle a colour falls on.
const fn luminance(color: Color) -> u8 {
    ((2126 * color.r as u32 + 7152 * color.g as u32 + 722 * color.b as u32) / 10000) as u8
}

/// The window behind the panes, and an unselected tab: the background, a step
/// deeper.
///
/// *Darker* on a light theme as well as on a dark one, because what this
/// paints is the recess — the well a command is typed into, the gutter a short
/// tab strip leaves — and a recess reads as a shadow in both. That is also why
/// `is_light` is not consulted here: the flag says which way the *text* goes,
/// and this is not about text.
///
/// A pure black background has nowhere darker to go, and pure black is a
/// palette people actually write, so it steps the other way instead. Something
/// has to separate the window from the panes on it.
const fn receded(background: Color) -> Color {
    // A *percentage* of a dark background is nothing: four per cent of #121212
    // is one step in each channel, which is a well nobody can see. So the step
    // has a floor in absolute terms, and the percentage only takes over on the
    // light backgrounds where four per cent is more than the floor.
    Color::rgb(
        recede_channel(background.r),
        recede_channel(background.g),
        recede_channel(background.b),
    )
}

/// One channel of [`receded`].
///
/// A free function rather than a closure because this runs in a `const fn`,
/// and a closure cannot be called in one.
const fn recede_channel(channel: u8) -> u8 {
    let percentage = (channel as u16 * GROUND_PERCENT as u16) / 100;
    let by = if percentage < GROUND_FLOOR as u16 {
        GROUND_FLOOR as u16
    } else {
        percentage
    };

    if (channel as u16) < by {
        // Already at the bottom: step the other way instead. Something has to
        // separate the window from the panes on it, and on a pure black theme
        // the only available direction is lighter.
        (channel as u16 + by) as u8
    } else {
        (channel as u16 - by) as u8
    }
}

/// How far the window's ground sits from the panes on it, as a percentage of
/// the background.
const GROUND_PERCENT: u8 = 8;

/// And never less than this, in absolute steps.
///
/// Eight per cent of a very dark background rounds to one or two steps out of
/// 255 — the difference between a recess and a rendering artefact. Every
/// bundled dark palette lands in exactly that region, so the floor is what
/// makes the field a command is typed into visible at all on them.
const GROUND_FLOOR: u8 = 9;

/// `top` at `percent` over `bottom`, flattened.
///
/// Warp's `neutral` scale is this: its surfaces are the background with the
/// foreground mixed into it, not a second colour somebody picked. That is what
/// makes a surface stay legible on a palette nobody anticipated.
///
/// Integer arithmetic, and not because it is faster: a `const fn` is what lets
/// the built-in themes be derived at compile time and sit in a `static`, and
/// `f32::round` cannot be called in one.
const fn composite(bottom: Color, top: Color, percent: u8) -> Color {
    const fn mix(bottom: u8, top: u8, percent: u8) -> u8 {
        let bottom = bottom as u16 * (100 - percent as u16);
        let top = top as u16 * percent as u16;
        // Rounded rather than truncated: a ladder of twelve steps built out of
        // truncations drifts a whole level down by the top of it.
        ((bottom + top + 50) / 100) as u8
    }

    Color::rgb(
        mix(bottom.r, top.r, percent),
        mix(bottom.g, top.g, percent),
        mix(bottom.b, top.b, percent),
    )
}

/// A percentage as an alpha byte.
const fn percent_of_255(percent: u8) -> u8 {
    ((percent as u16 * 255) / 100) as u8
}

/// A theme somebody can choose, wherever it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Available {
    /// What the chooser calls it, and what the settings file stores.
    pub name: String,
    /// The palette itself.
    pub theme: Theme,
    /// The file it was read from, for a built-in `None`.
    ///
    /// Carried because a name is not an identity: two files can declare the
    /// same `name:`, and the only thing that tells them apart — in the chooser,
    /// and for anything that ever wants to open or remove one — is where they
    /// came from.
    pub path: Option<PathBuf>,
}

impl Available {
    /// Whether this came from a file rather than from the binary.
    pub fn from_file(&self) -> bool {
        self.path.is_some()
    }
}

/// Every theme that can be chosen: the built-in ones, then whatever is in the
/// user's themes directory.
///
/// Read fresh each time rather than cached. Warp watches its themes directory
/// and reloads on any change; this is the same effect at the moments it
/// matters — a surface that lists themes asks when it opens — without a
/// watcher, a channel or a second thing that can be stale. A theme dropped in
/// while Crook is running shows up the next time the chooser is opened.
pub fn available() -> Vec<Available> {
    match user_themes_directory() {
        Some(directory) => available_in(&directory),
        None => builtins(),
    }
}

/// The same, from a themes directory named outright.
///
/// The seam every test needs. Without it a test asserting "three themes" would
/// be asserting something about the machine it runs on, and a test that
/// *wrote* a theme would write into the themes folder of whoever ran it.
pub fn available_in(directory: &Path) -> Vec<Available> {
    let mut themes = builtins();

    for file in load_themes_in(directory) {
        let entry = Available {
            name: file.name,
            theme: file.theme,
            path: Some(file.path),
        };

        // A user theme replaces the built-in of the same name, in place: the
        // person who wrote the file has said what that name means on this
        // machine, and a list with two "Crook Dark" rows — one of them
        // unreachable, since the settings file stores a name — would be worse
        // than either.
        //
        // Two *files* with the same name are a different case and both are
        // kept: see `disambiguate`.
        match themes
            .iter_mut()
            .find(|theme| theme.name == entry.name && !theme.from_file())
        {
            Some(existing) => *existing = entry,
            None => themes.push(entry),
        }
    }

    disambiguate(&mut themes);
    themes
}

/// The themes that ship in the binary, as choosable entries.
fn builtins() -> Vec<Available> {
    BUILTIN
        .iter()
        .map(|builtin| Available {
            name: builtin.name.to_owned(),
            theme: builtin.theme,
            path: None,
        })
        .collect()
}

/// Makes every name in the list unique, by the file each collision came from.
///
/// Two theme files can perfectly well declare the same `name:`, or derive the
/// same name from two stems in two directories. Warp shows both rows with
/// identical labels and tells them apart by path; Crook stores a *name* in its
/// settings file, so two rows with one label would mean one of them could
/// never be chosen twice in a row. The second and later ones are suffixed with
/// the file's own stem, which is the thing a person can act on: it is what
/// they would rename.
fn disambiguate(themes: &mut [Available]) {
    let mut seen: Vec<String> = Vec::new();

    for entry in themes.iter_mut() {
        if !seen.contains(&entry.name) {
            seen.push(entry.name.clone());
            continue;
        }

        let stem = entry
            .path
            .as_deref()
            .and_then(Path::file_stem)
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("2")
            .to_owned();
        let mut candidate = format!("{} ({stem})", entry.name);
        let mut serial = 2;
        while seen.contains(&candidate) {
            serial += 1;
            candidate = format!("{} ({stem} {serial})", entry.name);
        }

        seen.push(candidate.clone());
        entry.name = candidate;
    }
}

/// The theme with this name, or `None` when nothing on this machine has it.
pub fn named(name: &str) -> Option<Theme> {
    available()
        .into_iter()
        .find(|theme| theme.name == name)
        .map(|theme| theme.theme)
}

/// The name of the theme a fresh install opens in.
pub const DEFAULT_NAME: &str = BUILTIN[0].name;

thread_local! {
    /// The palette this thread draws in.
    static CURRENT: Cell<Theme> = const { Cell::new(DARK) };
}

/// The palette in force, as a value.
///
/// The one way a view gets a colour, and about as cheap as reading a field:
/// a thread-local and a copy of a struct of colours.
pub fn theme() -> Theme {
    CURRENT.with(Cell::get)
}

/// Puts a palette in force. Everything drawn after this is drawn in it.
///
/// Repainting is the caller's job — `Workspace::set_theme` notifies, because
/// it is the view that knows a window is on screen to repaint.
pub fn set_theme(theme: Theme) {
    CURRENT.with(|current| current.set(theme));
}

/// Puts a theme in force for the length of a test, and the default back after.
///
/// No lock: the theme is per-thread and a test has its thread to itself, so
/// nothing here can reach another test. What this is for is the *end* of a
/// test — a thread is reused for the tests that follow, and one that left a
/// light palette behind would hand it to them.
#[cfg(test)]
pub struct ThemeGuard;

#[cfg(test)]
impl ThemeGuard {
    /// Puts `theme` in force on this thread.
    pub fn new(theme: Theme) -> Self {
        set_theme(theme);
        Self
    }
}

#[cfg(test)]
impl Drop for ThemeGuard {
    fn drop(&mut self) {
        set_theme(DARK);
    }
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
