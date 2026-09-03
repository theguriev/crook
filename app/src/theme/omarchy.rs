//! The themes Omarchy dresses a desktop in.
//!
//! Ten palettes, read out of the `colors.toml` files Omarchy ships, so that
//! a terminal on an Omarchy desktop is the same colours as the desktop around
//! it. Switching the desktop theme does not switch Crook's — there is no
//! mechanism for that and inventing one would mean watching a file nobody
//! promised to keep — but the names line up, and picking the matching one
//! takes two clicks.
//!
//! # Whose colours these are
//!
//! Not Crook's, and not Omarchy's either for most of them. Each palette
//! belongs to the project that designed it — Catppuccin, Everforest, Gruvbox,
//! Kanagawa, Nord, Rosé Pine, Tokyo Night — each published under its own
//! licence, and each already redistributed in exactly this form by every
//! terminal that ships themes: a list of hex values. Omarchy is where the
//! values here were read from, because it has already done the work of
//! choosing one coherent set of sixteen per palette, and Omarchy is MIT, from
//! Basecamp. Matte Black and Osaka Jade are Omarchy's own.
//!
//! One palette Omarchy ships is deliberately **not** here: Ristretto, whose
//! colours are Monokai Pro's — a paid product whose licence does not permit
//! this. Bundling it would have been a claim nobody checked, which is the
//! whole reason this paragraph names licences one at a time rather than
//! saying "all permissive" and moving on.
//!
//! # The mapping
//!
//! Omarchy's files name their colours by role rather than by ANSI slot, so the
//! sixteen are assembled here:
//!
//! | ANSI | Omarchy |
//! | --- | --- |
//! | black | `darker_background` |
//! | red, green, yellow, blue, magenta, cyan | the same names |
//! | white | `light_foreground` |
//! | bright black | `muted` |
//! | bright red … bright cyan | `bright_*` |
//! | bright white | `bright_foreground` |
//!
//! Everything else — surfaces, borders, overlays, the muted text — is derived
//! from the background, the foreground and the accent by
//! [`Theme::derived`](super::Theme::derived), which is what every theme read
//! from a file gets too. These are not special.

use crookui_core::geometry::Color;

use super::{TerminalColors, Theme};

/// Catppuccin, as Omarchy sets it.
pub const CATPPUCCIN: Theme = Theme::derived(
    Color::hex(0x89b4fa),
    TerminalColors {
        foreground: Color::hex(0xcdd6f4),
        background: Color::hex(0x1e1e2e),
        cursor: Color::hex(0x89b4fa),
        normal: [
            Color::hex(0x101019),
            Color::hex(0xf38ba8),
            Color::hex(0xa6e3a1),
            Color::hex(0xf9e2af),
            Color::hex(0x89b4fa),
            Color::hex(0xf5c2e7),
            Color::hex(0x94e2d5),
            Color::hex(0xbac2de),
        ],
        bright: [
            Color::hex(0x585b70),
            Color::hex(0xf38ba8),
            Color::hex(0xa6e3a1),
            Color::hex(0xf9e2af),
            Color::hex(0x89b4fa),
            Color::hex(0xf5c2e7),
            Color::hex(0x94e2d5),
            Color::hex(0xcdd6f4),
        ],
    },
);

/// Catppuccin Latte, as Omarchy sets it.
pub const CATPPUCCIN_LATTE: Theme = Theme::derived(
    Color::hex(0x1e66f5),
    TerminalColors {
        foreground: Color::hex(0x4c4f69),
        background: Color::hex(0xeff1f5),
        cursor: Color::hex(0x1e66f5),
        normal: [
            Color::hex(0xd7d8dc),
            Color::hex(0xd20f39),
            Color::hex(0x40a02b),
            Color::hex(0xdf8e1d),
            Color::hex(0x1e66f5),
            Color::hex(0xea76cb),
            Color::hex(0x179299),
            Color::hex(0x5c5f77),
        ],
        bright: [
            Color::hex(0xacb0be),
            Color::hex(0xd20f39),
            Color::hex(0x40a02b),
            Color::hex(0xdf8e1d),
            Color::hex(0x1e66f5),
            Color::hex(0xea76cb),
            Color::hex(0x179299),
            Color::hex(0x4c4f69),
        ],
    },
);

/// Everforest, as Omarchy sets it.
pub const EVERFOREST: Theme = Theme::derived(
    Color::hex(0x7fbbb3),
    TerminalColors {
        foreground: Color::hex(0xd3c6aa),
        background: Color::hex(0x2d353b),
        cursor: Color::hex(0x7fbbb3),
        normal: [
            Color::hex(0x181d20),
            Color::hex(0xe67e80),
            Color::hex(0xa7c080),
            Color::hex(0xdbbc7f),
            Color::hex(0x7fbbb3),
            Color::hex(0xd699b6),
            Color::hex(0x83c092),
            Color::hex(0x9da9a0),
        ],
        bright: [
            Color::hex(0x475258),
            Color::hex(0xe67e80),
            Color::hex(0xa7c080),
            Color::hex(0xdbbc7f),
            Color::hex(0x7fbbb3),
            Color::hex(0xd699b6),
            Color::hex(0x83c092),
            Color::hex(0xd3c6aa),
        ],
    },
);

/// Gruvbox, as Omarchy sets it.
pub const GRUVBOX: Theme = Theme::derived(
    Color::hex(0x7daea3),
    TerminalColors {
        foreground: Color::hex(0xd4be98),
        background: Color::hex(0x282828),
        cursor: Color::hex(0x7daea3),
        normal: [
            Color::hex(0x161616),
            Color::hex(0xea6962),
            Color::hex(0xa9b665),
            Color::hex(0xd8a657),
            Color::hex(0x7daea3),
            Color::hex(0xd3869b),
            Color::hex(0x89b482),
            Color::hex(0xbdae93),
        ],
        bright: [
            Color::hex(0x665c54),
            Color::hex(0xea6962),
            Color::hex(0xa9b665),
            Color::hex(0xd8a657),
            Color::hex(0x7daea3),
            Color::hex(0xd3869b),
            Color::hex(0x89b482),
            Color::hex(0xd4be98),
        ],
    },
);

/// Kanagawa, as Omarchy sets it.
pub const KANAGAWA: Theme = Theme::derived(
    Color::hex(0xdcd7ba),
    TerminalColors {
        foreground: Color::hex(0xdcd7ba),
        background: Color::hex(0x1f1f28),
        cursor: Color::hex(0xdcd7ba),
        normal: [
            Color::hex(0x111116),
            Color::hex(0xc34043),
            Color::hex(0x76946a),
            Color::hex(0xc0a36e),
            Color::hex(0x7e9cd8),
            Color::hex(0x957fb8),
            Color::hex(0x6a9589),
            Color::hex(0xc8c093),
        ],
        bright: [
            Color::hex(0x54546d),
            Color::hex(0xe82424),
            Color::hex(0x98bb6c),
            Color::hex(0xe6c384),
            Color::hex(0x7fb4ca),
            Color::hex(0x938aa9),
            Color::hex(0x7aa89f),
            Color::hex(0xdcd7ba),
        ],
    },
);

/// Matte Black, as Omarchy sets it.
pub const MATTE_BLACK: Theme = Theme::derived(
    Color::hex(0xe68e0d),
    TerminalColors {
        foreground: Color::hex(0xbebebe),
        background: Color::hex(0x121212),
        cursor: Color::hex(0xe68e0d),
        normal: [
            Color::hex(0x090909),
            Color::hex(0xd35f5f),
            Color::hex(0xffc107),
            Color::hex(0xb91c1c),
            Color::hex(0xe68e0d),
            Color::hex(0xd35f5f),
            Color::hex(0xbebebe),
            Color::hex(0x8a8a8d),
        ],
        bright: [
            Color::hex(0x333333),
            Color::hex(0xb91c1c),
            Color::hex(0xffc107),
            Color::hex(0xb90a0a),
            Color::hex(0xf59e0b),
            Color::hex(0xb91c1c),
            Color::hex(0xeaeaea),
            Color::hex(0xbebebe),
        ],
    },
);

/// Nord, as Omarchy sets it.
pub const NORD: Theme = Theme::derived(
    Color::hex(0x81a1c1),
    TerminalColors {
        foreground: Color::hex(0xd8dee9),
        background: Color::hex(0x2e3440),
        cursor: Color::hex(0x81a1c1),
        normal: [
            Color::hex(0x191c23),
            Color::hex(0xbf616a),
            Color::hex(0xa3be8c),
            Color::hex(0xebcb8b),
            Color::hex(0x81a1c1),
            Color::hex(0xb48ead),
            Color::hex(0x88c0d0),
            Color::hex(0xadb5c4),
        ],
        bright: [
            Color::hex(0x4c566a),
            Color::hex(0xbf616a),
            Color::hex(0xa3be8c),
            Color::hex(0xebcb8b),
            Color::hex(0x81a1c1),
            Color::hex(0xb48ead),
            Color::hex(0x8fbcbb),
            Color::hex(0xd8dee9),
        ],
    },
);

/// Osaka Jade, as Omarchy sets it.
pub const OSAKA_JADE: Theme = Theme::derived(
    Color::hex(0x509475),
    TerminalColors {
        foreground: Color::hex(0xc1c497),
        background: Color::hex(0x111c18),
        cursor: Color::hex(0x509475),
        normal: [
            Color::hex(0x090f0d),
            Color::hex(0xff5345),
            Color::hex(0x549e6a),
            Color::hex(0x459451),
            Color::hex(0x509475),
            Color::hex(0xd2689c),
            Color::hex(0x2dd5b7),
            Color::hex(0xd6d5bc),
        ],
        bright: [
            Color::hex(0x53685b),
            Color::hex(0xdb9f9c),
            Color::hex(0x63b07a),
            Color::hex(0xe5c736),
            Color::hex(0xacd4cf),
            Color::hex(0x75bbb3),
            Color::hex(0x8cd3cb),
            Color::hex(0xf7e8b2),
        ],
    },
);

/// Rosé Pine, as Omarchy sets it.
pub const ROSE_PINE: Theme = Theme::derived(
    Color::hex(0x56949f),
    TerminalColors {
        foreground: Color::hex(0x575279),
        background: Color::hex(0xfaf4ed),
        cursor: Color::hex(0x56949f),
        normal: [
            Color::hex(0xe1dbd5),
            Color::hex(0xb4637a),
            Color::hex(0x286983),
            Color::hex(0xea9d34),
            Color::hex(0x56949f),
            Color::hex(0x907aa9),
            Color::hex(0xd7827e),
            Color::hex(0x6e6a86),
        ],
        bright: [
            Color::hex(0xcecacd),
            Color::hex(0xb4637a),
            Color::hex(0x286983),
            Color::hex(0xea9d34),
            Color::hex(0x56949f),
            Color::hex(0x907aa9),
            Color::hex(0xd7827e),
            Color::hex(0x575279),
        ],
    },
);

/// Tokyo Night, as Omarchy sets it.
pub const TOKYO_NIGHT: Theme = Theme::derived(
    Color::hex(0x7aa2f7),
    TerminalColors {
        foreground: Color::hex(0xa9b1d6),
        background: Color::hex(0x1a1b26),
        cursor: Color::hex(0x7aa2f7),
        normal: [
            Color::hex(0x0e0e14),
            Color::hex(0xf7768e),
            Color::hex(0x9ece6a),
            Color::hex(0xe0af68),
            Color::hex(0x7aa2f7),
            Color::hex(0xad8ee6),
            Color::hex(0x449dab),
            Color::hex(0xb4bee6),
        ],
        bright: [
            Color::hex(0x414868),
            Color::hex(0xff7a93),
            Color::hex(0xb9f27c),
            Color::hex(0xff9e64),
            Color::hex(0x7da6ff),
            Color::hex(0xbb9af7),
            Color::hex(0x0db9d7),
            Color::hex(0xc0caf5),
        ],
    },
);

/// Every Omarchy palette, in the order the panel lists them.
pub const OMARCHY: [(&str, Theme); 10] = [
    ("Catppuccin", CATPPUCCIN),
    ("Catppuccin Latte", CATPPUCCIN_LATTE),
    ("Everforest", EVERFOREST),
    ("Gruvbox", GRUVBOX),
    ("Kanagawa", KANAGAWA),
    ("Matte Black", MATTE_BLACK),
    ("Nord", NORD),
    ("Osaka Jade", OSAKA_JADE),
    ("Rosé Pine", ROSE_PINE),
    ("Tokyo Night", TOKYO_NIGHT),
];
