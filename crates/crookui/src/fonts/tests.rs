//! Font backend tests. None of these touch a GPU.
//!
//! They do touch the machine's installed fonts, because measuring a face is
//! most of what this module does and a synthetic face would only test the
//! plumbing. Every test that needs one goes through [`fixture`], which returns
//! `None` on a machine with no usable font — a `FROM scratch` container, a
//! stripped CI image — rather than failing there. What such a machine is
//! supposed to do is covered without a font, by
//! [`font_db_construction_reports_an_empty_database`].

use std::ops::Range;

use cosmic_text::{FontSystem, fontdb};
use crookui_core::fonts::{
    FamilyId, GlyphKey, LineStyle, Properties, RasterFormat, StyleAndFont, SubpixelAlignment,
    TextStyle,
};
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform::{FontDb as _, TextLayoutSystem as _};

use super::str_index_map::StrIndexMap;
use super::{CosmicFontDb, CosmicTextLayout};

/// A backend with the platform's default UI family already registered.
struct Fixture {
    font_db: CosmicFontDb,
    layout: CosmicTextLayout,
    family: FamilyId,
}

impl Fixture {
    fn style_runs(&self, text: &str) -> Vec<(Range<usize>, StyleAndFont)> {
        vec![(
            0..text.len(),
            StyleAndFont::new(self.family, Properties::default(), TextStyle::default()),
        )]
    }
}

fn fixture() -> Option<Fixture> {
    let font_db = CosmicFontDb::new().ok()?;
    let family = font_db.default_ui_family().ok()?;
    let layout = font_db.text_layout();
    Some(Fixture {
        font_db,
        layout,
        family,
    })
}

/// The one bound the trait imposes that a type can silently lose.
#[test]
fn the_shaper_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<CosmicTextLayout>();
}

#[test]
fn font_db_construction_reports_an_empty_database() {
    let empty = FontSystem::new_with_locale_and_db("en-US".to_owned(), fontdb::Database::new());
    let font_db = CosmicFontDb::from_font_system(empty);

    let error = font_db
        .default_ui_family()
        .expect_err("a database with no faces cannot yield a family");
    assert!(
        format!("{error:#}").contains("no installed font family"),
        "a font-less machine must say so, not panic: {error:#}"
    );
}

#[test]
fn a_system_face_has_sane_metrics() {
    let Some(fixture) = fixture() else { return };

    let font_id = fixture
        .font_db
        .select_font(fixture.family, Properties::default())
        .expect("the family was just registered");
    let metrics = fixture.font_db.font_metrics(font_id);

    assert!(metrics.units_per_em > 0);
    assert!(metrics.ascent > 0, "ascent rises above the baseline");
    assert!(
        metrics.descent <= 0,
        "descent uses the sTypoDescender sign convention, so it drops below zero"
    );
    assert!(
        metrics.ascent as i32 - metrics.descent as i32 <= 4 * metrics.units_per_em as i32,
        "a text box four times the em is not a real face: {metrics:?}"
    );
}

#[test]
fn ascii_glyphs_have_positive_advances() {
    let Some(fixture) = fixture() else { return };

    let font_id = fixture
        .font_db
        .select_font(fixture.family, Properties::default())
        .expect("the family was just registered");

    for character in ['m', 'i', 'W', '0', '.'] {
        let glyph_id = fixture
            .font_db
            .glyph_for_char(font_id, character)
            .unwrap_or_else(|| panic!("a UI face must cover {character:?}"));
        let advance = fixture
            .font_db
            .glyph_advance(font_id, glyph_id)
            .unwrap_or_else(|error| panic!("no advance for {character:?}: {error:#}"));

        assert!(
            advance.x() > 0.,
            "{character:?} advances by {} font units",
            advance.x()
        );
    }
}

#[test]
fn em_width_scales_with_font_size() {
    let Some(fixture) = fixture() else { return };

    let small = fixture.font_db.em_width(fixture.family, 12.).unwrap();
    let large = fixture.font_db.em_width(fixture.family, 24.).unwrap();

    assert!(small > 0.);
    assert!(
        (large - small * 2.).abs() < 0.01,
        "em width is linear in font size: {small} then {large}"
    );
}

#[test]
fn a_missing_glyph_is_an_error_rather_than_a_zero_advance() {
    let Some(fixture) = fixture() else { return };

    let font_id = fixture
        .font_db
        .select_font(fixture.family, Properties::default())
        .expect("the family was just registered");

    assert!(
        fixture.font_db.glyph_advance(font_id, u32::MAX).is_err(),
        "a glyph id past the end of the face has no advance to report"
    );
}

#[test]
fn a_laid_out_line_advances_left_to_right() {
    let Some(fixture) = fixture() else { return };

    let text = "Tab 1";
    let line = fixture.layout.layout_line(
        text,
        LineStyle::default(),
        &fixture.style_runs(text),
        f32::INFINITY,
    );

    let positions: Vec<_> = line
        .runs
        .iter()
        .flat_map(|run| run.glyphs.iter())
        .map(|glyph| glyph.position_along_baseline.x())
        .collect();

    assert_eq!(positions.len(), text.chars().count());
    assert!(
        positions.windows(2).all(|pair| pair[1] > pair[0]),
        "left-to-right text advances monotonically: {positions:?}"
    );
    assert!(line.width > 0.);
    assert!(line.ascent > 0. && line.descent >= 0.);
    assert!(
        line.width <= positions.last().copied().unwrap_or_default() + line.font_size * 2.,
        "the reported width must match where the glyphs actually ended"
    );
}

#[test]
fn an_empty_line_still_has_a_baseline() {
    let Some(fixture) = fixture() else { return };

    let line = fixture
        .layout
        .layout_line("", LineStyle::default(), &fixture.style_runs(""), 100.);

    assert!(line.runs.is_empty());
    assert_eq!(line.width, 0.);
    assert!(
        line.ascent > 0.,
        "an empty tab title still occupies a line box: {line:?}"
    );
    assert!(line.baseline_position() > 0.);
}

/// `cosmic_text::ShapeLine` is documented to panic on input whose paragraphs
/// disagree about direction, and a tab title built from agent output containing
/// a newline is exactly that input.
#[test]
fn multiple_paragraphs_lay_out_on_one_line() {
    let Some(fixture) = fixture() else { return };

    let text = "a\nb";
    let line = fixture.layout.layout_line(
        text,
        LineStyle::default(),
        &fixture.style_runs(text),
        f32::INFINITY,
    );

    let indices: Vec<_> = line
        .runs
        .iter()
        .flat_map(|run| run.glyphs.iter())
        .map(|glyph| glyph.index)
        .collect();

    assert!(!indices.is_empty(), "both paragraphs must still render");
    assert!(
        indices.iter().all(|index| *index < text.len()),
        "glyph indices address the caller's string, not the joined one: {indices:?}"
    );
    assert!(
        indices.contains(&2),
        "the glyph after the newline reports its offset in the source: {indices:?}"
    );
}

#[test]
fn multiple_paragraphs_survive_windows_line_endings_and_a_trailing_newline() {
    let Some(fixture) = fixture() else { return };

    for text in ["a\r\nb", "ab\n", "\na", "\n", "a\n\nb"] {
        let line = fixture.layout.layout_line(
            text,
            LineStyle::default(),
            &fixture.style_runs(text),
            f32::INFINITY,
        );

        for glyph in line.runs.iter().flat_map(|run| run.glyphs.iter()) {
            assert!(
                glyph.index < text.len(),
                "{text:?} produced an out-of-range index {}",
                glyph.index
            );
        }
    }
}

#[test]
fn style_runs_keep_their_colors_across_a_run_break() {
    let Some(fixture) = fixture() else { return };

    let text = "ab";
    let red = TextStyle {
        foreground_color: Some(crookui_core::Color::rgb(255, 0, 0)),
        ..Default::default()
    };
    let style_runs = vec![
        (
            0..1,
            StyleAndFont::new(fixture.family, Properties::default(), TextStyle::default()),
        ),
        (
            1..2,
            StyleAndFont::new(fixture.family, Properties::default(), red),
        ),
    ];

    let line = fixture
        .layout
        .layout_line(text, LineStyle::default(), &style_runs, f32::INFINITY);

    assert_eq!(line.runs.len(), 2, "a style change breaks the run");
    assert_eq!(line.runs[0].styles, TextStyle::default());
    assert_eq!(line.runs[1].styles, red);
}

#[test]
fn subpixel_alignment_quantizes_to_three_steps() {
    let offsets: Vec<_> = (0..300)
        .map(|step| {
            SubpixelAlignment::new(vec2f(step as f32 / 100., 0.))
                .to_offset()
                .x()
        })
        .collect();

    let mut distinct = offsets.clone();
    distinct.sort_by(f32::total_cmp);
    distinct.dedup();

    assert_eq!(
        distinct.len(),
        3,
        "three horizontal bins per pixel, no more: {distinct:?}"
    );
    assert!(distinct.iter().all(|offset| (0. ..1.).contains(offset)));
}

#[test]
fn a_rasterized_glyph_fills_its_reported_bounds() {
    let Some(fixture) = fixture() else { return };

    let font_id = fixture
        .font_db
        .select_font(fixture.family, Properties::default())
        .expect("the family was just registered");
    let glyph_key = GlyphKey {
        glyph_id: fixture.font_db.glyph_for_char(font_id, 'm').unwrap(),
        font_id,
        font_size: 14.,
    };
    let scale = Vector2F::splat(2.);

    let bounds = fixture
        .font_db
        .glyph_raster_bounds(glyph_key, scale)
        .unwrap();
    assert!(!bounds.is_empty(), "'m' draws something: {bounds:?}");

    let glyph = fixture
        .font_db
        .rasterize_glyph(
            glyph_key,
            scale,
            SubpixelAlignment::default(),
            RasterFormat::Rgba32,
        )
        .unwrap();

    assert!(!glyph.is_emoji);
    assert_eq!(glyph.canvas.size, (bounds.width, bounds.height));
    assert_eq!(glyph.canvas.format, RasterFormat::Rgba32);
    assert_eq!(glyph.canvas.row_stride, bounds.width as usize * 4);
    assert_eq!(
        glyph.canvas.pixels.len(),
        glyph.canvas.row_stride * bounds.height as usize,
        "row_stride must describe the buffer, not the format it came from"
    );
    assert!(
        glyph.canvas.pixels.iter().any(|byte| *byte > 0),
        "a rasterized 'm' is not blank"
    );
}

#[test]
fn a_coverage_mask_is_replicated_into_every_requested_channel() {
    let Some(fixture) = fixture() else { return };

    let font_id = fixture
        .font_db
        .select_font(fixture.family, Properties::default())
        .expect("the family was just registered");
    let glyph_key = GlyphKey {
        glyph_id: fixture.font_db.glyph_for_char(font_id, 'm').unwrap(),
        font_id,
        font_size: 14.,
    };

    let coverage = fixture
        .font_db
        .rasterize_glyph(
            glyph_key,
            Vector2F::splat(1.),
            SubpixelAlignment::default(),
            RasterFormat::A8,
        )
        .unwrap();
    let rgba = fixture
        .font_db
        .rasterize_glyph(
            glyph_key,
            Vector2F::splat(1.),
            SubpixelAlignment::default(),
            RasterFormat::Rgba32,
        )
        .unwrap();

    assert_eq!(coverage.canvas.format, RasterFormat::A8);
    assert_eq!(coverage.canvas.pixels.len() * 4, rgba.canvas.pixels.len());
    assert!(
        rgba.canvas
            .pixels
            .chunks_exact(4)
            .zip(&coverage.canvas.pixels)
            .all(|(pixel, byte)| pixel == [*byte; 4]),
        "the glyph shader reads coverage from the red channel of an RGBA atlas"
    );
}

#[test]
fn a_glyph_that_draws_nothing_reports_empty_bounds() {
    let Some(fixture) = fixture() else { return };

    let font_id = fixture
        .font_db
        .select_font(fixture.family, Properties::default())
        .expect("the family was just registered");
    let glyph_key = GlyphKey {
        glyph_id: fixture.font_db.glyph_for_char(font_id, ' ').unwrap(),
        font_id,
        font_size: 14.,
    };

    let bounds = fixture
        .font_db
        .glyph_raster_bounds(glyph_key, Vector2F::splat(1.))
        .unwrap();
    assert!(bounds.is_empty(), "a space has no pixels: {bounds:?}");

    let glyph = fixture
        .font_db
        .rasterize_glyph(
            glyph_key,
            Vector2F::splat(1.),
            SubpixelAlignment::default(),
            RasterFormat::Rgba32,
        )
        .expect("an empty glyph is an ordinary answer, not an error");
    assert!(glyph.canvas.pixels.is_empty());
}

#[test]
fn registering_a_family_is_idempotent_and_bytes_are_validated() {
    let Some(mut fixture) = fixture() else { return };

    assert_eq!(
        fixture.font_db.default_ui_family().unwrap(),
        fixture.family,
        "asking for the default family twice must not mint a second id"
    );

    let error = fixture
        .font_db
        .load_family_from_bytes("Nonsense", vec![vec![0u8; 64]])
        .expect_err("64 zero bytes are not a font");
    assert!(format!("{error:#}").contains("parsed as a font"));
}

#[test]
fn fallback_fonts_are_left_to_the_shaper() {
    let Some(fixture) = fixture() else { return };

    let font_id = fixture
        .font_db
        .select_font(fixture.family, Properties::default())
        .expect("the family was just registered");

    assert!(
        fixture.font_db.fallback_fonts('漢', font_id).is_empty(),
        "the measurement path has no fallback; shaping does its own"
    );
}

#[test]
fn a_single_paragraph_is_borrowed_and_maps_to_itself() {
    let map = StrIndexMap::new("Tab 1");

    assert_eq!(map.shaped_text(), "Tab 1");
    for index in 0..=5 {
        assert_eq!(map.source_index(index), index);
        assert_eq!(map.shaped_index(index), index);
    }
}

#[test]
fn paragraphs_are_joined_and_offsets_map_back() {
    let map = StrIndexMap::new("a\nbc");

    assert_eq!(map.shaped_text(), "a\u{200b}bc");
    assert_eq!(map.source_index(0), 0, "'a'");
    assert_eq!(map.source_index(4), 2, "'b', past a three-byte joiner");
    assert_eq!(map.source_index(5), 3, "'c'");

    assert_eq!(map.shaped_index(0), 0);
    assert_eq!(map.shaped_index(2), 4);
    assert_eq!(
        map.shaped_index(4),
        6,
        "the end of the string maps to the end"
    );
}

#[test]
fn an_offset_inside_a_paragraph_separator_clamps_to_the_paragraph_before_it() {
    let map = StrIndexMap::new("a\nb");

    assert_eq!(
        map.shaped_index(1),
        1,
        "the newline itself sits at the end of the first paragraph"
    );
    assert_eq!(map.source_index(1), 1);
}

#[test]
fn a_trailing_separator_is_dropped_without_losing_the_end_offset() {
    let map = StrIndexMap::new("ab\n");

    assert_eq!(map.shaped_text(), "ab");
    assert_eq!(map.shaped_index(3), 2);
    assert_eq!(map.source_index(2), 2);
}

/// The atlas allocates from [`FontDb::glyph_raster_bounds`] and uploads from
/// [`FontDb::rasterize_glyph`]. If those two ever disagree about a size, the
/// upload runs off the end of its reserved region or leaves part of it holding
/// another glyph's pixels — so they must agree for every subpixel bin, not just
/// the one the bounds were measured at.
#[test]
fn every_subpixel_bin_fills_the_reported_bounds() {
    let Some(fixture) = fixture() else { return };

    let font_id = fixture
        .font_db
        .select_font(fixture.family, Properties::default())
        .expect("the family was just registered");

    for size in [11., 13.5, 16.] {
        for scale in [1., 2.] {
            for character in "abdgijmoyABMW019.,|".chars() {
                let Some(glyph_id) = fixture.font_db.glyph_for_char(font_id, character) else {
                    continue;
                };
                let glyph_key = GlyphKey {
                    glyph_id,
                    font_id,
                    font_size: size,
                };
                let bounds = fixture
                    .font_db
                    .glyph_raster_bounds(glyph_key, Vector2F::splat(scale))
                    .unwrap();

                for step in 0..3 {
                    let alignment = SubpixelAlignment::new(vec2f(step as f32 / 3., 0.));
                    let glyph = fixture
                        .font_db
                        .rasterize_glyph(
                            glyph_key,
                            Vector2F::splat(scale),
                            alignment,
                            RasterFormat::Rgba32,
                        )
                        .unwrap();

                    assert_eq!(
                        glyph.canvas.size,
                        (bounds.width, bounds.height),
                        "{character:?} at {size}px x{scale}, bin {step}"
                    );
                    assert_eq!(
                        glyph.canvas.pixels.len(),
                        glyph.canvas.row_stride * bounds.height as usize
                    );
                    assert!(
                        glyph.canvas.pixels.iter().any(|byte| *byte > 0),
                        "{character:?} at {size}px x{scale}, bin {step} composited to nothing"
                    );
                }
            }
        }
    }
}
