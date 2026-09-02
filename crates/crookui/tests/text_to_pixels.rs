//! The seam between the two halves of this crate, end to end.
//!
//! The unit tests either side of it use doubles: the renderer's tests
//! rasterize synthetic squares, and the font backend's tests never reach a GPU.
//! This one wires the real [`CosmicFontDb`] to the real renderer and checks
//! that a string of text becomes lit pixels in the right place — the one
//! failure mode neither half can see on its own.
//!
//! Everything here needs a GPU but no display. On a machine with neither an
//! adapter nor a system font, each test logs and returns rather than failing:
//! the alternative is a suite that cannot run in a container.

use crookui::{CosmicFontDb, render_scene_to_rgba};
use crookui_core::fonts::{LineStyle, Properties, StyleAndFont, TextStyle};
use crookui_core::geometry::{Color, RectF, Vector2F, vec2f};
use crookui_core::platform::TextLayoutSystem as _;
use crookui_core::scene::Scene;
use crookui_core::text_layout::Line;

const SIZE: Vector2F = vec2f(160., 40.);
const FONT_SIZE: f32 = 20.;
const TEXT_ORIGIN: Vector2F = vec2f(8., 8.);
const INK: Color = Color::rgb(255, 255, 255);
const GROUND: Color = Color::rgb(0, 0, 0);

/// Lays out `text` and paints it over an opaque black ground.
fn scene(font_db: &CosmicFontDb, text: &str, scale_factor: f32) -> Option<(Scene, Line)> {
    let family = font_db.default_ui_family().ok()?;
    let style = StyleAndFont::new(family, Properties::default(), TextStyle::default());
    let line = font_db.text_layout().layout_line(
        text,
        LineStyle {
            font_size: FONT_SIZE,
            ..Default::default()
        },
        &[(0..text.len(), style)],
        f32::INFINITY,
    );

    let mut scene = Scene::new(scale_factor);
    scene
        .draw_rect_without_hit_recording(RectF::new(Vector2F::zero(), SIZE))
        .with_background(GROUND);
    line.paint(
        RectF::new(TEXT_ORIGIN, vec2f(line.width, line.height())),
        INK,
        &mut scene,
    );

    Some((scene, line))
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * width + x) * 4) as usize;
    pixels[start..start + 4].try_into().expect("four channels")
}

#[test]
fn shaped_text_becomes_lit_pixels_inside_the_box_it_measured() {
    let Ok(font_db) = CosmicFontDb::new() else {
        eprintln!("skipping: no usable system fonts");
        return;
    };
    let Some((scene, line)) = scene(&font_db, "Crook", 2.) else {
        eprintln!("skipping: no default UI family");
        return;
    };

    assert!(
        line.width > 0.,
        "a five-letter word must measure wider than nothing"
    );
    assert!(
        !line.runs.is_empty(),
        "a five-letter word must shape into at least one run"
    );

    let Ok((pixels, width, height)) = render_scene_to_rgba(&scene, SIZE, &font_db) else {
        eprintln!("skipping: no usable GPU adapter");
        return;
    };

    assert_eq!((width, height), (320, 80));

    // The ground is opaque black and the text is white, so any pixel that is
    // not black is ink. Counting them separates "the glyphs drew" from "the
    // glyphs drew somewhere sensible".
    let mut lit = 0;
    let mut outside = 0;
    let box_left = (TEXT_ORIGIN.x() * 2.) as u32;
    let box_right = ((TEXT_ORIGIN.x() + line.width) * 2.).ceil() as u32;
    let box_top = (TEXT_ORIGIN.y() * 2.) as u32;
    let box_bottom = ((TEXT_ORIGIN.y() + line.height()) * 2.).ceil() as u32;

    for y in 0..height {
        for x in 0..width {
            let [r, g, b, a] = pixel(&pixels, width, x, y);
            assert_eq!(a, 255, "the opaque ground should leave no transparency");
            if (r, g, b) == (0, 0, 0) {
                continue;
            }

            lit += 1;
            if x < box_left || x >= box_right || y < box_top || y >= box_bottom {
                outside += 1;
            }
        }
    }

    assert!(
        lit > 200,
        "five glyphs at 20px on a 2x scale should light hundreds of pixels, got {lit}"
    );
    assert_eq!(
        outside, 0,
        "{outside} lit pixels fell outside the line box the shaper measured"
    );
}

#[test]
fn the_same_scene_renders_to_the_same_bytes_twice() {
    let Ok(font_db) = CosmicFontDb::new() else {
        eprintln!("skipping: no usable system fonts");
        return;
    };
    let Some((scene, _)) = scene(&font_db, "Crook", 1.) else {
        eprintln!("skipping: no default UI family");
        return;
    };

    let Ok((first, ..)) = render_scene_to_rgba(&scene, SIZE, &font_db) else {
        eprintln!("skipping: no usable GPU adapter");
        return;
    };
    let (second, ..) = render_scene_to_rgba(&scene, SIZE, &font_db).expect("the second render");

    // Glyphs are batched per layer in first-seen order rather than through a
    // hash map, so two renders of one scene emit the same command stream. That
    // is what makes a golden-image test of this renderer worth writing.
    assert_eq!(first, second);
}
