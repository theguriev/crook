//! What a derived palette has to be true of, whatever colours it was given.

use super::*;

/// A theme derived from four colours, for asserting the rules rather than the
/// values.
fn derived(background: Color, foreground: Color) -> Theme {
    Theme::derived(
        Color::hex(0x8b5cf6),
        TerminalColors {
            background,
            foreground,
            ..builtin::DARK.terminal
        },
    )
}

/// How far apart two colours are, as the largest per-channel difference.
///
/// Crude next to a perceptual distance, and enough for what these tests ask:
/// "did this move at all" and "did it move the right way".
fn distance(left: Color, right: Color) -> i16 {
    let channel = |left: u8, right: u8| (i16::from(left) - i16::from(right)).abs();
    channel(left.r, right.r)
        .max(channel(left.g, right.g))
        .max(channel(left.b, right.b))
}

#[test]
fn a_derived_ladder_climbs_away_from_the_background_it_started_at() {
    // Warp's `neutral` scale, and the reason a theme file can be a handful of
    // colours long: every surface is the background moved a few percent
    // towards the text, so a palette nobody anticipated still has surfaces
    // that read.
    let theme = derived(Color::hex(0x14161a), Color::hex(0xe8ebf0));

    assert!(
        theme.ground.r < theme.surface.r,
        "the window behind the panes should be darker than they are"
    );
    assert!(
        theme.surface_raised.r > theme.surface.r,
        "a raised surface should be lighter than the surface it floats over"
    );
    assert!(
        theme.border.r > theme.surface_raised.r,
        "a hairline should be the lightest of the three, or it cannot be seen"
    );
}

#[test]
fn the_same_derivation_on_a_light_background_runs_the_other_way() {
    // The one thing a light theme cannot inherit from a dark one: its ladder
    // *descends*, because the foreground being mixed in is dark. Nothing here
    // special-cases that — it falls out of compositing the theme's own
    // foreground rather than a hardcoded white.
    let theme = derived(Color::hex(0xfbfbfd), Color::hex(0x1f232b));

    assert!(
        theme.border.r < theme.surface.r,
        "a hairline on a light theme should be darker than the surface it divides"
    );
    assert!(
        theme.surface_raised.r < theme.surface.r,
        "a raised surface on a light theme should be darker than what it floats over"
    );
    // The recess is the exception that does not flip: a well is a shadow on
    // both kinds of theme.
    assert!(
        theme.ground.r < theme.surface.r,
        "the window behind the panes should be a shade deeper than they are"
    );
}

#[test]
fn a_background_with_nowhere_to_recede_still_gets_a_ground_of_its_own() {
    // Pure black and pure white are palettes people actually write, and on
    // both of them the window behind the panes cannot step further away from
    // the text. It steps towards it instead: the field a command is typed into
    // is a well in that ground, and a well the colour of what surrounds it is
    // not a well.
    for (background, foreground) in [
        (Color::hex(0x000000), Color::hex(0xffffff)),
        (Color::hex(0xffffff), Color::hex(0x000000)),
    ] {
        let theme = derived(background, foreground);
        assert_ne!(
            theme.ground, theme.surface,
            "a {background:?} theme's ground is indistinguishable from its panes"
        );
    }
}

#[test]
fn muted_text_stays_between_the_ground_and_the_text_it_is_quieter_than() {
    let theme = derived(Color::hex(0x14161a), Color::hex(0xe8ebf0));

    assert!(
        distance(theme.text_muted, theme.ground) > 40,
        "muted text has to be legible against the ground it sits on"
    );
    assert!(
        distance(theme.text_muted, theme.text_primary) > 40,
        "muted text has to be visibly quieter than primary text"
    );
}

#[test]
fn every_built_in_theme_is_legible() {
    // The one assertion worth making about a palette somebody chose by eye:
    // that its text can be read on its surfaces. A theme that fails this is
    // not a matter of taste.
    for builtin in BUILTIN {
        let theme = builtin.theme;
        let name = builtin.name;

        assert!(
            distance(theme.text_primary, theme.ground) > 100,
            "{name}: primary text is too close to the ground"
        );
        assert!(
            distance(theme.text_primary, theme.surface) > 100,
            "{name}: primary text is too close to a surface"
        );
        assert!(
            distance(theme.text_muted, theme.surface) > 40,
            "{name}: muted text is too close to a surface"
        );
        assert!(
            distance(theme.terminal.foreground, theme.terminal.background) > 100,
            "{name}: the grid's text is too close to the grid"
        );
        assert_eq!(
            theme.terminal.background, theme.surface,
            "{name}: the grid and the pane around it have to be one surface"
        );
    }
}

#[test]
fn whether_a_theme_is_light_is_inferred_from_its_text() {
    // Warp's rule, and the reason no theme file declares a mode: a palette
    // whose text is dark is a palette meant for a light background, and
    // nobody can write that down wrongly because nobody writes it down.
    // Through the array rather than by naming each constant: a `const`'s
    // field is a constant, and an assertion about one is a tautology clippy is
    // right to point at. Read out of `BUILTIN` it is the same claim about a
    // value the compiler has not folded away.
    let light_ones: Vec<&str> = BUILTIN
        .iter()
        .filter(|builtin| builtin.theme.is_light)
        .map(|builtin| builtin.name)
        .collect();
    assert_eq!(light_ones, ["Crook Light"]);

    assert!(derived(Color::hex(0xffffff), Color::hex(0x111111)).is_light);
    assert!(!derived(Color::hex(0x111111), Color::hex(0xffffff)).is_light);
}

#[test]
fn the_theme_a_view_reads_is_the_one_that_was_set() {
    // The whole of "Crook has themes", asserted once: a view asks for a role
    // and gets whatever palette is in force.
    let _guard = ThemeGuard::new(builtin::LIGHT);

    assert!(theme().is_light);
    assert_eq!(theme().ground, builtin::LIGHT.ground);
}

#[test]
fn the_guard_puts_the_default_back() {
    {
        let _guard = ThemeGuard::new(builtin::MIDNIGHT);
        assert_eq!(theme().ground, builtin::MIDNIGHT.ground);
    }

    assert_eq!(
        theme(),
        DARK,
        "a test left its theme behind for the next one"
    );
}
