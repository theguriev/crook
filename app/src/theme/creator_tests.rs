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
fn the_ansi_set_is_the_one_that_reads_on_the_background_it_will_be_drawn_on() {
    // Warp picks by which foreground won, which is right at both ends of the
    // range and wrong in the middle. The two extremes still land where they
    // should:
    let (on_black, _) = ansi_for(Color::hex(0x000000));
    let (on_white, _) = ansi_for(Color::hex(0xffffff));
    assert_eq!(on_black, ANSI_NORMAL);
    assert_eq!(on_white, LIGHT.terminal.normal);

    // And every draft, from every bundled palette, has a set of colours that
    // reads on the grid *as a whole*. Not every colour individually: ANSI
    // black is the shadow slot, and the xterm blue is almost invisible on
    // black in every terminal ever written — both are properties of the
    // standard palette rather than of this choice. What the choice has to get
    // right is the average, and that is what the by-the-foreground rule got
    // wrong on a mid-tone background.
    let average = |colors: crate::theme::TerminalColors| {
        let sum: f32 = colors
            .normal
            .iter()
            .skip(1)
            .chain(colors.bright.iter().skip(1))
            .map(|color| contrast(colors.background, *color))
            .sum();
        sum / 14.
    };

    for builtin in crate::theme::BUILTIN {
        let mut draft = Draft::new(&builtin.theme);
        for index in 0..CANDIDATES {
            draft.choose(index);
            assert!(
                average(draft.theme.terminal) >= 2.0,
                "{}: swatch {index} leaves its terminal colours at {:.2}:1 on average",
                builtin.name,
                average(draft.theme.terminal)
            );
        }
    }

    // The case the old rule got wrong, named: a mid-tone background takes
    // black text, so picking by the foreground would hand it the palette meant
    // for paper.
    let mid = Color::hex(0x7a7394);
    assert_eq!(
        foreground_for(mid),
        Color::hex(0x000000),
        "the middle takes black text"
    );
    assert_eq!(
        ansi_for(mid).0,
        ANSI_NORMAL,
        "a mid-tone grid should take the colours that read on it, not the ones its text implies"
    );
}

#[test]
fn a_palette_of_one_colour_still_makes_a_theme_with_a_visible_cursor() {
    // A theme file of sixteen identical colours parses, and clustering five
    // candidates out of one colour gives five of it. What must not happen is
    // an accent — and so a cursor — the same colour as the grid.
    let flat = Color::hex(0x000000);
    let source = Theme::derived(
        flat,
        TerminalColors {
            foreground: flat,
            background: flat,
            cursor: flat,
            normal: [flat; 8],
            bright: [flat; 8],
        },
    );

    let draft = Draft::new(&source);
    assert_ne!(draft.theme.accent, draft.theme.terminal.background);
    assert_ne!(draft.theme.terminal.cursor, draft.theme.terminal.background);
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
