use alacritty_terminal::vte::ansi::Rgb as VteRgb;

use super::*;

/// The colour overrides a fresh terminal has: none.
fn no_overrides() -> Colors {
    Colors::default()
}

#[test]
fn test_the_default_palette_covers_the_whole_indexed_space() {
    let palette = Palette::default();

    // The sixteen ANSI colours.
    assert_eq!(Rgb::hex(0x000000), palette.ansi[0]);
    assert_eq!(Rgb::hex(0xffffff), palette.ansi[15]);
    // The 6x6x6 cube: index 16 is black, 231 is white, and 196 is pure red.
    assert_eq!(Rgb::new(0, 0, 0), palette.ansi[16]);
    assert_eq!(Rgb::new(255, 255, 255), palette.ansi[231]);
    assert_eq!(Rgb::new(255, 0, 0), palette.ansi[196]);
    // The grey ramp runs 8 to 238 in steps of ten.
    assert_eq!(Rgb::new(8, 8, 8), palette.ansi[232]);
    assert_eq!(Rgb::new(238, 238, 238), palette.ansi[255]);
}

#[test]
fn test_named_colours_resolve_to_their_slots() {
    let palette = Palette::default();
    let overrides = no_overrides();

    assert_eq!(
        palette.foreground,
        palette.named(NamedColor::Foreground, &overrides)
    );
    assert_eq!(
        palette.background,
        palette.named(NamedColor::Background, &overrides)
    );
    assert_eq!(
        palette.cursor,
        palette.named(NamedColor::Cursor, &overrides)
    );
    assert_eq!(
        palette.ansi[3],
        palette.named(NamedColor::Yellow, &overrides)
    );
    assert_eq!(
        palette.ansi[11],
        palette.named(NamedColor::BrightYellow, &overrides)
    );
    assert_eq!(
        palette.ansi[3].scaled(DIM_FACTOR),
        palette.named(NamedColor::DimYellow, &overrides)
    );
}

#[test]
fn test_an_override_wins_over_the_palette() {
    let palette = Palette::default();
    let mut overrides = no_overrides();
    overrides[NamedColor::Red] = Some(VteRgb { r: 1, g: 2, b: 3 });

    assert_eq!(
        Rgb::new(1, 2, 3),
        palette.named(NamedColor::Red, &overrides)
    );
    assert_eq!(Rgb::new(1, 2, 3), palette.color_at(1, &overrides));
    assert_eq!(palette.ansi[2], palette.color_at(2, &overrides));
}

#[test]
fn test_colours_arrive_in_every_form_the_grid_uses() {
    let palette = Palette::default();
    let overrides = no_overrides();

    assert_eq!(
        Rgb::new(9, 9, 9),
        palette.resolve(Color::Spec(VteRgb { r: 9, g: 9, b: 9 }), &overrides)
    );
    assert_eq!(
        palette.ansi[42],
        palette.resolve(Color::Indexed(42), &overrides)
    );
    assert_eq!(
        palette.foreground,
        palette.resolve(Color::Named(NamedColor::Foreground), &overrides)
    );
}

#[test]
fn test_scaling_a_colour_holds_it_back() {
    assert_eq!(Rgb::new(0, 0, 0), Rgb::new(0, 0, 0).scaled(0.5));
    assert_eq!(Rgb::new(100, 50, 0), Rgb::new(200, 100, 0).scaled(0.5));
    // Nothing overflows on the way up.
    assert_eq!(Rgb::new(255, 255, 255), Rgb::new(200, 200, 200).scaled(4.));
}

#[test]
fn test_hex_literals_split_into_channels() {
    assert_eq!(Rgb::new(0x12, 0x34, 0x56), Rgb::hex(0x123456));
}

#[test]
fn test_cell_flags_combine_and_test() {
    let mut flags = CellFlags::NONE;
    assert!(flags.is_empty());

    flags |= CellFlags::BOLD;
    flags |= CellFlags::UNDERLINE;

    assert!(!flags.is_empty());
    assert!(flags.contains(CellFlags::BOLD));
    assert!(flags.contains(CellFlags::UNDERLINE));
    assert!(flags.contains(CellFlags::BOLD | CellFlags::UNDERLINE));
    assert!(!flags.contains(CellFlags::ITALIC));
    assert_ne!(0, flags.bits());
}

#[test]
fn test_a_grid_is_never_smaller_than_a_terminal_can_be() {
    // A window laid out before it knows its font size asks for nothing at all.
    let size = TerminalSize::new(0, 0);

    assert_eq!(2, size.columns());
    assert_eq!(1, size.screen_lines());
    assert_eq!(size.screen_lines(), size.total_lines());
}

#[test]
fn test_the_default_size_is_the_one_every_terminal_starts_at() {
    let size = TerminalSize::default();

    assert_eq!(80, size.columns);
    assert_eq!(24, size.rows);
    assert_eq!(
        0, size.cell_width,
        "the pixel size is unknown until a font is laid out"
    );
}
