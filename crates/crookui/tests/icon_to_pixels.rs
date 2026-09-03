//! An icon, from a scene reference to lit pixels.
//!
//! The unit tests either side of this one stop short of the seam: the
//! rasterizer's tests read coverage bytes and never reach a GPU, and the
//! renderer's tests upload synthetic squares. This one puts a real icon in a
//! real scene and checks that it comes back as pixels of the right colour in
//! the right square — which is the whole of what the atlas, the instance
//! layout and the shader's mask branch have to get right between them.
//!
//! Everything here needs a GPU but no display, and skips rather than fails on
//! a machine without one.

use crookui::{CosmicFontDb, render_scene_to_rgba};
use crookui_core::geometry::{Color, RectF, Vector2F, vec2f};
use crookui_core::icons::{IconKey, Lucide};
use crookui_core::scene::Scene;

const SIZE: Vector2F = vec2f(40., 40.);
const ICON_ORIGIN: Vector2F = vec2f(8., 8.);
const ICON_SIZE: f32 = 24.;
const SCALE: f32 = 2.;
const GROUND: Color = Color::rgb(0, 0, 0);

/// One icon in `ink`, over an opaque black ground.
fn scene(icon: Lucide, ink: Color) -> Scene {
    let mut scene = Scene::new(SCALE);
    scene
        .draw_rect_without_hit_recording(RectF::new(Vector2F::zero(), SIZE))
        .with_background(GROUND);
    scene.draw_icon(
        IconKey::new(icon, ICON_SIZE),
        RectF::new(ICON_ORIGIN, Vector2F::splat(ICON_SIZE)),
        ink,
    );
    scene
}

fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * width + x) * 4) as usize;
    pixels[start..start + 4].try_into().expect("four channels")
}

#[test]
fn an_icon_becomes_lit_pixels_inside_the_square_it_was_given() {
    let Ok(font_db) = CosmicFontDb::new() else {
        eprintln!("skipping: no usable system fonts");
        return;
    };

    let scene = scene(Lucide::X, Color::rgb(255, 255, 255));
    let Ok((pixels, width, height)) = render_scene_to_rgba(&scene, SIZE, &font_db) else {
        eprintln!("skipping: no usable GPU adapter");
        return;
    };

    assert_eq!((width, height), (80, 80));

    let left = (ICON_ORIGIN.x() * SCALE) as u32;
    let top = (ICON_ORIGIN.y() * SCALE) as u32;
    let right = left + (ICON_SIZE * SCALE) as u32;
    let bottom = top + (ICON_SIZE * SCALE) as u32;

    let mut lit = 0;
    let mut outside = 0;
    for y in 0..height {
        for x in 0..width {
            let [r, g, b, a] = pixel(&pixels, width, x, y);
            assert_eq!(a, 255, "the opaque ground should leave no transparency");
            if (r, g, b) == (0, 0, 0) {
                continue;
            }

            lit += 1;
            if x < left || x >= right || y < top || y >= bottom {
                outside += 1;
            }
        }
    }

    assert!(
        lit > 100,
        "a 24px cross on a 2x scale should light hundreds of pixels, got {lit}"
    );
    assert_eq!(
        outside, 0,
        "{outside} lit pixels fell outside the square the icon was given"
    );

    // A cross is two strokes through the middle, so the centre of the square
    // is the one pixel that is certainly ink and the corners are certainly
    // not. Anything that placed the quad wrongly — a baseline offset applied
    // to an icon, a flipped V coordinate — fails here rather than merely
    // drawing fewer pixels.
    let [r, g, b, _] = pixel(&pixels, width, (left + right) / 2, (top + bottom) / 2);
    assert_eq!(
        (r, g, b),
        (255, 255, 255),
        "the strokes cross at the centre"
    );
    for (x, y) in [
        (left + 1, top + 1),
        (right - 2, top + 1),
        (left + 1, bottom - 2),
        (right - 2, bottom - 2),
    ] {
        let [r, g, b, _] = pixel(&pixels, width, x, y);
        assert_eq!((r, g, b), (0, 0, 0), "({x}, {y}) is a corner, not a stroke");
    }
}

#[test]
fn an_icon_is_tinted_by_the_colour_it_was_drawn_with() {
    // The mask carries coverage, not colour: an icon is a monochrome glyph as
    // far as the shader is concerned, and it takes its colour from the
    // instance. A mask uploaded into the wrong channels, or an icon that
    // reached the shader flagged as an emoji, comes back grey or invisible.
    let Ok(font_db) = CosmicFontDb::new() else {
        eprintln!("skipping: no usable system fonts");
        return;
    };

    let scene = scene(Lucide::Plus, Color::rgb(255, 0, 0));
    let Ok((pixels, width, height)) = render_scene_to_rgba(&scene, SIZE, &font_db) else {
        eprintln!("skipping: no usable GPU adapter");
        return;
    };

    let mut lit = 0;
    for y in 0..height {
        for x in 0..width {
            let [r, g, b, _] = pixel(&pixels, width, x, y);
            if (r, g, b) == (0, 0, 0) {
                continue;
            }

            lit += 1;
            assert_eq!(
                (g, b),
                (0, 0),
                "({x}, {y}) is ({r}, {g}, {b}), which is not red over black"
            );
        }
    }

    assert!(lit > 50, "a plus should light more than {lit} pixels");
}
