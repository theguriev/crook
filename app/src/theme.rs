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
//! That is a trade with two things to be honest about. The lock is taken on
//! every colour lookup — a few hundred per frame, uncontended, which is
//! nanoseconds against a frame that shapes text and talks to a GPU. And a
//! global is shared by a test binary's threads, so a test that changes the
//! theme has to hold [`ThemeGuard`] rather than simply setting one.

use std::sync::RwLock;

use crookui_core::geometry::Color;

mod builtin;
mod file;

pub use builtin::{BUILTIN, Builtin, DARK, builtin_named};
pub use file::{ThemeFile, load_user_themes, user_themes_directory};

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
            text_muted: composite(background, foreground, 55),
            accent,
            // The usage bands and the diff chips are read as signals rather
            // than as part of the palette — "critical" has to be red on every
            // theme — so they come from the theme's own ANSI colours, which is
            // where a theme says what it thinks red is.
            usage_normal: composite(background, foreground, 55),
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
    let deeper = composite(background, Color::hex(0x000000), GROUND_PERCENT);
    if deeper.r == background.r && deeper.g == background.g && deeper.b == background.b {
        composite(background, Color::hex(0xffffff), GROUND_PERCENT)
    } else {
        deeper
    }
}

/// How far the window's ground sits from the panes on it.
const GROUND_PERCENT: u8 = 4;

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
#[derive(Clone, Debug)]
pub struct Available {
    /// What the settings page calls it, and what the settings file stores.
    pub name: String,
    /// The palette itself.
    pub theme: Theme,
    /// Whether it came from a file rather than from the binary.
    ///
    /// The settings page says so, because it is the difference between a theme
    /// that will still be there after a reinstall and one that will not.
    pub from_file: bool,
}

/// Every theme that can be chosen: the built-in ones, then whatever is in the
/// themes directory.
///
/// Read fresh each time rather than cached. Warp watches its themes directory
/// and reloads on any change; this is the same effect at the one moment it
/// matters — the settings page asks for the list when it draws — without a
/// watcher, a channel or a second thing that can be stale. A theme dropped in
/// while Crook is running shows up the next time the page is opened.
///
/// A user theme whose name collides with a built-in wins: whoever wrote the
/// file has said what they want that name to mean on this machine.
pub fn available() -> Vec<Available> {
    let mut themes: Vec<Available> = BUILTIN
        .iter()
        .map(|builtin| Available {
            name: builtin.name.to_owned(),
            theme: builtin.theme,
            from_file: false,
        })
        .collect();

    for file in load_user_themes() {
        let entry = Available {
            name: file.name,
            theme: file.theme,
            from_file: true,
        };
        match themes.iter_mut().find(|theme| theme.name == entry.name) {
            Some(existing) => *existing = entry,
            None => themes.push(entry),
        }
    }

    themes
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

/// The palette in force, as a value.
///
/// The one way a view gets a colour. Cheap: a read lock and a copy of a
/// struct of colours.
pub fn theme() -> Theme {
    *current().read().expect("the theme lock was poisoned")
}

/// Puts a palette in force. Everything drawn after this is drawn in it.
///
/// Repainting is the caller's job — `Workspace::set_theme` notifies, because
/// it is the view that knows a window is on screen to repaint.
pub fn set_theme(theme: Theme) {
    *current().write().expect("the theme lock was poisoned") = theme;
}

/// The lock behind [`theme`].
fn current() -> &'static RwLock<Theme> {
    static CURRENT: RwLock<Theme> = RwLock::new(DARK);
    &CURRENT
}

/// Holds the theme still for the length of a test.
///
/// A test binary runs its tests on many threads against this one global, so a
/// test that changes the theme would change it under every other test running
/// at that moment. Taking this makes those tests take turns, and puts the
/// default back when the last one is done.
///
/// Only tests need it, which is why it is `cfg(test)`: nothing in a running
/// Crook changes the theme from two places at once.
#[cfg(test)]
pub struct ThemeGuard {
    /// The turn itself. Held and never read — dropping it is the whole point,
    /// which is what the leading underscore says to the dead-code lint.
    _turn: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl ThemeGuard {
    /// Takes the theme for this test, and sets it to `theme`.
    pub fn new(theme: Theme) -> Self {
        static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

        // A poisoned lock means another test panicked while holding it; the
        // theme it left behind is about to be overwritten anyway.
        let turn = TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        set_theme(theme);
        Self { _turn: turn }
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
