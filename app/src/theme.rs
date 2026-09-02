//! The palette, as one constant.
//!
//! Twelve named colours and no settings system. Warp's appearance layer exists
//! to serve user themes, per-terminal ANSI palettes and cloud-synced
//! preferences; none of that is a v1 concern, and a `const` struct is
//! everything a v1 actually reads. When themes do arrive, this type is the
//! shape they load into, and every call site already goes through a name
//! rather than a hex literal.
//!
//! The values are the ones `crookui/examples/scene_to_png.rs` composes its
//! mock from, so the real view tree and that mock agree on what Crook looks
//! like.

use crookui_core::geometry::Color;

/// A dark palette, in the roles the UI asks for.
pub struct Theme {
    /// The window's own background, behind everything.
    pub ground: Color,
    /// Raised surfaces: the header, the content panel, a hovered control.
    pub surface: Color,
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
}

/// The palette every view reads.
pub const THEME: Theme = Theme {
    ground: Color::hex(0x14_16_1a),
    surface: Color::hex(0x1a_1d_24),
    tab_active: Color::hex(0x1f_24_30),
    tab_inactive: Color::hex(0x17_1a_20),
    border: Color::hex(0x25_2a_34),
    text_primary: Color::hex(0xe8_eb_f0),
    text_muted: Color::hex(0x8a_93_a3),
    accent: Color::hex(0x8b_5c_f6),
    // Normal deliberately repeats `text_muted`: usage under half is not news,
    // and a colour there would spend the reader's attention on nothing.
    usage_normal: Color::hex(0x8a_93_a3),
    usage_elevated: Color::hex(0xe0_b3_41),
    usage_high: Color::hex(0xe5_8a_2e),
    usage_critical: Color::hex(0xe5_48_4b),
};
