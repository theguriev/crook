//! What a derived palette has to be true of, whatever colours it was given.

use super::*;
use crate::theme::creator::contrast;

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
fn every_colour_a_program_prints_in_reads_on_crooks_own_themes() {
    // `ls` paints directories in colour 4 and `git diff` its removals in
    // colour 1, so these are text, and text is held to WCAG's 4.5:1. The
    // xterm set fails it on a dark grid — its blue was 1.79:1 on Crook Dark —
    // which is why Crook's own dark themes carry a set of their own.
    //
    // Colour 0 is left out: on a dark grid black is the slot a program fills
    // behind light text or paints dark ink on a coloured fill with, and a
    // black that read as text on the grid would do neither.
    //
    // The palettes in `omarchy` are their authors' and are not held to this.
    let crooks: Vec<Builtin> = BUILTIN
        .into_iter()
        .filter(|builtin| {
            !omarchy::OMARCHY
                .iter()
                .any(|(name, _)| *name == builtin.name)
        })
        .collect();
    assert_eq!(
        crooks
            .iter()
            .map(|builtin| builtin.name)
            .collect::<Vec<_>>(),
        ["Crook Dark", "Crook Light", "Midnight"],
        "the themes this holds to the bar are not the ones Crook ships"
    );

    // The bright half is text too — `\e[91m` is as common as `\e[31m` — and
    // colours 9 to 15 are held to the same bar. Bright black is left out with
    // colour 0: it is the grey a prompt dims a suggestion with, quieter on
    // purpose.
    for builtin in crooks {
        let terminal = builtin.theme.terminal;
        let normal = terminal.normal.iter().enumerate().skip(1);
        let bright = terminal
            .bright
            .iter()
            .enumerate()
            .skip(1)
            .map(|(index, color)| (index + 8, color));
        for (index, color) in normal.chain(bright) {
            let ratio = contrast(*color, terminal.background);
            assert!(
                ratio >= 4.5,
                "{}: colour {index}, {color:?}, is {ratio:.2}:1 on its grid",
                builtin.name
            );
        }
    }
}

#[test]
fn every_tab_colour_reads_as_a_mark_on_crooks_own_themes() {
    // A tab's colour is a stripe down its row, which sits on `surface`, and a
    // swatch in its menu, which sits on `surface_raised`. Neither is text, so
    // the bar is WCAG 1.4.11's 3:1 for a mark rather than 4.5:1 — but it is a
    // floor on both surfaces, in every theme Crook writes itself. The palettes
    // in `omarchy` are their authors' and are not held to it.
    let crooks = BUILTIN.into_iter().filter(|builtin| {
        !omarchy::OMARCHY
            .iter()
            .any(|(name, _)| *name == builtin.name)
    });
    for builtin in crooks {
        let theme = builtin.theme;
        for color in crate::tab::TabColor::ALL {
            let mark = theme.terminal.bright[color.index()];
            for (surface, ground) in [
                ("surface", theme.surface),
                ("surface_raised", theme.surface_raised),
            ] {
                let ratio = contrast(mark, ground);
                assert!(
                    ratio >= 3.,
                    "{}: a {} tab, {mark:?}, is {ratio:.2}:1 on {surface}",
                    builtin.name,
                    color.name()
                );
            }
        }
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

    // The three light palettes that ship, and — for the two that are somebody
    // else's — this is also the assertion that the inference agrees with what
    // their own authors declared: Omarchy's files carry a `mode`, Crook's type
    // does not, and a rule that read "dark" where a palette says "light" would
    // have the overlay ladder running the wrong way on somebody else's theme.
    assert_eq!(
        light_ones,
        ["Crook Light", "Catppuccin Latte", "Rosé Pine"],
        "the inferred light themes are not the ones their authors call light"
    );

    assert!(derived(Color::hex(0xffffff), Color::hex(0x111111)).is_light);
    assert!(!derived(Color::hex(0x111111), Color::hex(0xffffff)).is_light);
}

#[test]
fn muted_text_reads_on_every_bundled_palette_light_or_dark() {
    // The rung that fails quietly: 55% towards the text reads as "quieter" on
    // a dark palette and falls to about 2.5:1 on a light one, which is under
    // every threshold there is. Every subtitle, description and placeholder in
    // the interface is drawn in it.
    for builtin in BUILTIN {
        let theme = builtin.theme;
        let ratio = contrast(theme.text_muted, theme.surface);
        assert!(
            ratio >= 3.5,
            "{}: muted text sits at {ratio:.2}:1 on its own surface",
            builtin.name
        );
    }
}

#[test]
fn the_window_behind_the_panes_is_visibly_deeper_than_they_are() {
    // The field a command is typed into is a well in this colour. A percentage
    // of a very dark background is one step out of 255 — the difference
    // between a recess and a rendering artefact — and every bundled dark
    // palette lands in exactly that region.
    for builtin in BUILTIN {
        let theme = builtin.theme;
        assert!(
            distance(theme.ground, theme.surface) >= 6,
            "{}: the ground and the panes on it differ by {}",
            builtin.name,
            distance(theme.ground, theme.surface)
        );
    }
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

#[test]
fn a_status_discs_glyph_reads_on_its_fill_in_every_builtin_palette() {
    // The bell and the cross inside a tab's status disc, on the amber and the
    // red, and the play on the accent: each at 3:1 against its fill where
    // either candidate gets there, and otherwise the better of the two.
    for available in builtins() {
        let theme = available.theme;
        for fill in [theme.accent, theme.usage_high, theme.usage_critical] {
            let mark = theme.mark_on(fill);
            let best = creator::contrast(theme.text_primary, fill)
                .max(creator::contrast(theme.surface, fill));
            assert!(
                creator::contrast(mark, fill) >= 3. || creator::contrast(mark, fill) >= best,
                "{}: the glyph is {:.2}:1 on {fill:?}",
                available.name,
                creator::contrast(mark, fill)
            );
        }
        assert_eq!(theme.mark_on(theme.accent), theme.on_accent_mark());
    }
}

#[test]
fn a_mark_on_the_accent_reads_there_in_every_builtin_palette() {
    // The knob of a switch that is on. At least 3:1 against the accent — the
    // contrast WCAG asks of a control — in every palette that ships, and in a
    // palette whose own text already reads there, that text, unchanged.
    for available in builtins() {
        let theme = available.theme;
        let mark = theme.on_accent_mark();
        let best = creator::contrast(theme.text_primary, theme.accent)
            .max(creator::contrast(theme.surface, theme.accent));
        assert!(
            creator::contrast(mark, theme.accent) >= 3.
                || creator::contrast(mark, theme.accent) >= best,
            "{}: the mark is {:.2}:1 on its accent",
            available.name,
            creator::contrast(mark, theme.accent)
        );
        if creator::contrast(theme.text_primary, theme.accent) >= 3. {
            assert_eq!(mark, theme.text_primary, "{}", available.name);
        }
    }
    // The palette the change is for.
    let light = builtins()
        .into_iter()
        .find(|available| available.name == "Crook Light")
        .expect("Crook Light ships")
        .theme;
    assert_ne!(light.on_accent_mark(), light.text_primary);
}

#[test]
fn a_failed_block_says_how_it_ended_in_its_stripe_s_red_where_that_reads() {
    // The caption under a failed block is eleven pixels of text on the grid
    // washed a tenth red. Crook's own two palettes put it in the same red as
    // the stripe, at WCAG's 4.5:1 for text or better; the duration beside it
    // is in the muted ink, which has to read on the same ground.
    for name in ["Crook Dark", "Crook Light"] {
        let theme = builtins()
            .into_iter()
            .find(|available| available.name == name)
            .expect("the palette ships")
            .theme;
        let red = theme.terminal.normal[1];
        let behind = composite(theme.terminal.background, red, 10);
        let ink = theme.failure_ink(behind);
        assert_eq!(ink, red, "{name}: the failure is not in the stripe's red");
        assert!(
            contrast(ink, behind) >= 4.5,
            "{name}: the failure is {:.2}:1 on its block",
            contrast(ink, behind)
        );
        assert!(
            contrast(theme.text_muted, behind) >= 4.5,
            "{name}: the duration is {:.2}:1 on a failed block",
            contrast(theme.text_muted, behind)
        );
    }

    // A red that only works as a fill gives the words to the palette's text.
    let mut washed_out = DARK;
    washed_out.terminal.normal[1] = washed_out.terminal.background;
    assert_eq!(
        washed_out.failure_ink(washed_out.terminal.background),
        washed_out.text_primary
    );
}
