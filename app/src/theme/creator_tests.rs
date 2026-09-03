//! What a generated theme has to be true of, whatever it was generated from.

use super::super::builtin::{DARK, LIGHT, MIDNIGHT};
use super::*;

/// How far apart two colours are, as the largest per-channel difference.
fn apart(left: Color, right: Color) -> i16 {
    let channel = |left: u8, right: u8| (i16::from(left) - i16::from(right)).abs();
    channel(left.r, right.r)
        .max(channel(left.g, right.g))
        .max(channel(left.b, right.b))
}

#[test]
fn the_candidates_are_five_distinct_colours_darkest_first() {
    for source in [DARK, LIGHT, MIDNIGHT] {
        let candidates = candidates(&source);

        // By L*, the lightness the clustering works in — not by luma, which
        // orders two saturated colours differently and would be asserting
        // something the code never claimed.
        for pair in candidates.windows(2) {
            assert!(
                Lab::of(pair[0]).l <= Lab::of(pair[1]).l,
                "the swatches are not in order of lightness: {candidates:?}"
            );
        }

        // Five swatches with two the same is a row with a hole in it.
        for (index, left) in candidates.iter().enumerate() {
            for right in candidates.iter().skip(index + 1) {
                assert!(
                    apart(*left, *right) > 4,
                    "two candidates are the same colour: {candidates:?}"
                );
            }
        }
    }
}

#[test]
fn clustering_the_same_theme_twice_gives_the_same_five() {
    // Warp seeds its k-means with zero so a photograph always yields the same
    // theme. Farthest-point initialisation gets that for nothing — and if it
    // did not, a person would get a different palette every time they opened
    // the creator on the same theme.
    assert_eq!(candidates(&DARK), candidates(&DARK));
    assert_eq!(candidates(&LIGHT), candidates(&LIGHT));
}

#[test]
fn the_text_of_a_generated_theme_is_black_or_white_and_never_anything_else() {
    // Warp's rule, and the reason a generated theme is always legible: the
    // foreground is not a colour anybody picked, it is whichever of the two
    // extremes contrasts better.
    for background in [
        Color::hex(0x000000),
        Color::hex(0xffffff),
        Color::hex(0x002b36),
        Color::hex(0xfbfbfd),
        Color::hex(0x808080),
    ] {
        let foreground = foreground_for(background);
        assert!(
            foreground == Color::hex(0x000000) || foreground == Color::hex(0xffffff),
            "{background:?} produced {foreground:?}"
        );
        assert!(
            contrast(background, foreground) >= 3.,
            "{background:?} and its text do not contrast"
        );
    }

    assert_eq!(foreground_for(Color::hex(0x111111)), Color::hex(0xffffff));
    assert_eq!(foreground_for(Color::hex(0xeeeeee)), Color::hex(0x000000));
}

#[test]
fn the_accent_avoids_the_colours_that_hug_the_background_and_the_text() {
    // Warp's own test of the same rule: against a black background and white
    // text, of a bright red, a dark red and a nearly-black red, the middle one
    // wins — an accent is the colour furthest from *both* ends, not the
    // loudest one.
    let candidates = [
        Color::hex(0x000000),
        Color::hex(0x0a0000),
        Color::hex(0x640000),
        Color::hex(0xff0000),
        Color::hex(0xffffff),
    ];

    let accent = accent_for(Color::hex(0x000000), Color::hex(0xffffff), candidates, 0);
    assert_eq!(accent, Color::hex(0x640000));
}

#[test]
fn a_draft_is_legible_whichever_swatch_is_chosen() {
    // The whole point of deciding everything but the background: a person
    // cannot produce a palette they cannot read.
    for source in [DARK, LIGHT, MIDNIGHT] {
        let mut draft = Draft::new(&source);

        for index in 0..CANDIDATES {
            draft.choose(index);
            let theme = draft.theme;

            assert_eq!(
                theme.terminal.background, draft.candidates[index],
                "the chosen swatch is not the background"
            );
            assert!(
                contrast(theme.terminal.background, theme.terminal.foreground) >= 3.,
                "swatch {index} of {:?} produced unreadable text",
                source.terminal.background
            );
            assert_eq!(
                theme.surface, theme.terminal.background,
                "the chrome and the grid have to be one surface"
            );
            assert_ne!(
                theme.accent, theme.terminal.background,
                "the accent is invisible against the background"
            );
            assert!(
                apart(theme.text_muted, theme.surface) > 30,
                "muted text is too close to the surface it sits on"
            );
        }
    }
}

#[test]
fn a_dark_draft_and_a_light_draft_get_different_ansi_sets() {
    // Warp picks one of two fixed palettes by which foreground won, because
    // the standard bright colours are chosen to sit on black and are
    // unreadable on white.
    let (dark_normal, _) = ansi_for(Color::hex(0xffffff));
    let (light_normal, _) = ansi_for(Color::hex(0x000000));

    assert_eq!(dark_normal, ANSI_NORMAL);
    assert_eq!(light_normal, LIGHT.terminal.normal);
    assert_ne!(dark_normal, light_normal);
}

#[test]
fn choosing_the_swatch_already_chosen_changes_nothing() {
    let mut draft = Draft::new(&DARK);
    let before = draft.clone();
    draft.choose(draft.chosen);
    assert_eq!(draft, before);
}

#[test]
fn lab_survives_a_round_trip_through_a_colour() {
    // The clustering works in Lab and hands back colours; a conversion that
    // drifted would move every swatch away from anything in the source theme.
    for color in [
        Color::hex(0x000000),
        Color::hex(0xffffff),
        Color::hex(0x8b5cf6),
        Color::hex(0x002b36),
        Color::hex(0x4cc38a),
    ] {
        let round_tripped = Lab::of(color).to_color();
        assert!(
            apart(color, round_tripped) <= 1,
            "{color:?} came back as {round_tripped:?}"
        );
    }
}
