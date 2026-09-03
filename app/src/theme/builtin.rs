//! The themes that ship in the binary.
//!
//! Three, and they are here rather than in files for the reason the default
//! settings are a `Default` impl rather than a bundled JSON: a build that
//! could not find its own themes would have nothing to draw, and "the
//! application starts" must not depend on a file being where it was put.
//!
//! [`DARK`] is written out colour by colour rather than derived, and that is
//! deliberate. Every other theme goes through [`Theme::derived`], which mixes
//! a whole palette out of four colours; the dark one is the palette this
//! interface was drawn against, hand-tuned before there was a derivation, and
//! rederiving it would move every surface in the window by a shade or two to
//! prove a point about consistency. What the derivation is for is the themes
//! nobody has written yet.

use crookui_core::geometry::Color;

use super::omarchy::OMARCHY;
use super::{TerminalColors, Theme};

/// One theme, and what to call it.
///
/// A name is not a field of [`Theme`] because a theme's *identity* belongs to
/// wherever it came from: a built-in is named here, and a theme read from disk
/// is named by its file. Keeping the two apart is what stops a user theme from
/// claiming to be a built-in by writing the same name into itself.
#[derive(Copy, Clone, Debug)]
pub struct Builtin {
    /// What the settings page calls it, and what the settings file stores.
    pub name: &'static str,
    /// The palette itself.
    pub theme: Theme,
}

/// Every theme that ships, in the order the panel lists them.
///
/// Crook's three first, then the palettes in [`omarchy`](super::omarchy) —
/// which are not Crook's and say so, and which are here so that a terminal on
/// an Omarchy desktop can be the same colours as the desktop around it.
pub const BUILTIN: [Builtin; 3 + OMARCHY.len()] = {
    let mut themes = [Builtin {
        name: "Crook Dark",
        theme: DARK,
    }; 3 + OMARCHY.len()];

    themes[1] = Builtin {
        name: "Crook Light",
        theme: LIGHT,
    };
    themes[2] = Builtin {
        name: "Midnight",
        theme: MIDNIGHT,
    };

    // A `for` in a `const` block, which is what lets one list be written in
    // one place: `omarchy` names its own palettes, and this only says where
    // they go.
    let mut index = 0;
    while index < OMARCHY.len() {
        let (name, theme) = OMARCHY[index];
        themes[3 + index] = Builtin { name, theme };
        index += 1;
    }

    themes
};

/// The sixteen colours a program asks for by number.
///
/// The xterm values every terminal agrees on, which is also what
/// `crook_terminal`'s own default palette holds. A theme that wants its own
/// sixteen says so; a theme that does not gets the ones every program was
/// written against.
pub(super) const ANSI_NORMAL: [Color; 8] = [
    Color::hex(0x000000),
    Color::hex(0xcd0000),
    Color::hex(0x00cd00),
    Color::hex(0xcdcd00),
    Color::hex(0x0000ee),
    Color::hex(0xcd00cd),
    Color::hex(0x00cdcd),
    Color::hex(0xe5e5e5),
];

/// The bright half of the same eight.
pub(super) const ANSI_BRIGHT: [Color; 8] = [
    Color::hex(0x7f7f7f),
    Color::hex(0xff0000),
    Color::hex(0x00ff00),
    Color::hex(0xffff00),
    Color::hex(0x5c5cff),
    Color::hex(0xff00ff),
    Color::hex(0x00ffff),
    Color::hex(0xffffff),
];

/// What Crook has always looked like, and what it opens in.
///
/// The three overlay rungs are Warp's `fg_overlay` ladder — 5%, 10% and 15% of
/// the foreground — resolved against this palette's own text colour rather
/// than against whatever a loaded theme has.
pub const DARK: Theme = Theme {
    ground: Color::hex(0x14161a),
    surface: Color::hex(0x1a1d24),
    surface_raised: Color::hex(0x1c1e23),
    overlay_1: Color::hex(0xe8ebf0).with_alpha(12),
    overlay_2: Color::hex(0xe8ebf0).with_alpha(25),
    overlay_3: Color::hex(0xe8ebf0).with_alpha(38),
    tab_active: Color::hex(0x1f2430),
    tab_inactive: Color::hex(0x171a20),
    border: Color::hex(0x252a34),
    text_primary: Color::hex(0xe8ebf0),
    text_muted: Color::hex(0x8a93a3),
    accent: Color::hex(0x8b5cf6),
    // Normal deliberately repeats `text_muted`: usage under half is not news,
    // and a colour there would spend the reader's attention on nothing.
    usage_normal: Color::hex(0x8a93a3),
    usage_elevated: Color::hex(0xe0b341),
    usage_high: Color::hex(0xe58a2e),
    usage_critical: Color::hex(0xe5484b),
    diff_added: Color::hex(0x4cc38a),
    diff_removed: Color::hex(0xe56a6d),
    terminal: TerminalColors {
        // The grid's ground is the pane's own surface, which is what makes an
        // untouched screen cost no rectangles at all.
        foreground: Color::hex(0xe8ebf0),
        background: Color::hex(0x1a1d24),
        cursor: Color::hex(0x8b5cf6),
        normal: ANSI_NORMAL,
        bright: ANSI_BRIGHT,
    },
    is_light: false,
};

/// The same interface on paper.
///
/// Derived, so it is also the proof that the derivation produces something
/// usable: four colours in, a whole palette out. The ANSI sixteen are darkened
/// from the xterm set, because the standard bright colours are chosen to sit
/// on black and are unreadable on white — every light terminal theme makes the
/// same adjustment.
pub const LIGHT: Theme = Theme::derived(
    Color::hex(0x6d3ae8),
    TerminalColors {
        foreground: Color::hex(0x1f232b),
        background: Color::hex(0xfbfbfd),
        cursor: Color::hex(0x6d3ae8),
        normal: [
            Color::hex(0x2b2f37),
            Color::hex(0xc0312f),
            Color::hex(0x1c7d3f),
            Color::hex(0x8a6600),
            Color::hex(0x1f54c4),
            Color::hex(0x9b2d9b),
            Color::hex(0x0f6f78),
            Color::hex(0x5c636e),
        ],
        bright: [
            Color::hex(0x5c636e),
            Color::hex(0xd63a37),
            Color::hex(0x22924a),
            Color::hex(0xa17800),
            Color::hex(0x2a66e0),
            Color::hex(0xb438b4),
            Color::hex(0x14848e),
            Color::hex(0x1f232b),
        ],
    },
);

/// For a room with the lights off, and for a screen that saves power by
/// switching pixels off entirely.
pub const MIDNIGHT: Theme = Theme::derived(
    Color::hex(0x7c9cff),
    TerminalColors {
        foreground: Color::hex(0xd8dee9),
        background: Color::hex(0x000000),
        cursor: Color::hex(0x7c9cff),
        normal: ANSI_NORMAL,
        bright: ANSI_BRIGHT,
    },
);

/// The built-in with this name, if there is one.
pub fn builtin_named(name: &str) -> Option<Theme> {
    BUILTIN
        .iter()
        .find(|builtin| builtin.name == name)
        .map(|builtin| builtin.theme)
}
