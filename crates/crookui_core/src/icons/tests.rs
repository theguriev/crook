//! What the generator and the rasterizer promise each other.

use super::*;
use crate::fonts::RasterFormat;

/// The ink in a mask, as a fraction of a fully covered pixel.
fn ink(canvas: &crate::fonts::Canvas) -> f32 {
    canvas.pixels.iter().map(|byte| *byte as f32 / 255.).sum()
}

/// One pixel's coverage.
fn at(canvas: &crate::fonts::Canvas, x: u32, y: u32) -> u8 {
    canvas.pixels[y as usize * canvas.row_stride + x as usize]
}

#[test]
fn every_icon_carries_a_name_and_an_outline() {
    let mut names = Vec::new();
    for icon in all() {
        assert!(
            !icon.outline().is_empty(),
            "{icon:?} generated no geometry, which means the SVG behind it was empty"
        );
        names.push(icon.name());
    }

    assert_eq!(names.len(), ICONS);
    names.sort_unstable();
    let unique = names.len();
    names.dedup();
    assert_eq!(names.len(), unique, "two variants share one Lucide icon");
}

#[test]
fn an_outline_starts_where_it_is_told_to() {
    // The one thing a generator gets wrong silently: a relative `m` taken as
    // relative to wherever the *previous* element of the SVG ended, rather
    // than to the origin. Lucide's `x` is two strokes of a cross, and if the
    // second one is placed against the end of the first it walks off the box —
    // which is what the range below would catch.
    for icon in all() {
        for segment in icon.outline() {
            for coordinate in coordinates(segment) {
                assert!(
                    (0.0..=GRID).contains(&coordinate),
                    "{}'s outline leaves the 24-unit box at {coordinate}",
                    icon.name(),
                );
            }
        }
    }
}

/// Every number in one segment.
fn coordinates(segment: &Segment) -> Vec<f32> {
    match *segment {
        Segment::Move(x, y) | Segment::Line(x, y) => vec![x, y],
        Segment::Cubic(x1, y1, x2, y2, x, y) => vec![x1, y1, x2, y2, x, y],
        Segment::Close => Vec::new(),
    }
}

#[test]
fn a_mask_is_the_square_it_was_asked_for() {
    let mask = rasterize(Lucide::Settings, 37, STROKE_WIDTH);

    assert_eq!(mask.size, (37, 37));
    assert_eq!(
        mask.row_stride, 37,
        "a mask is one byte per pixel, unpadded"
    );
    assert_eq!(mask.pixels.len(), 37 * 37);
    assert_eq!(mask.format, RasterFormat::A8);
}

#[test]
fn an_icon_stays_off_the_edge_of_its_own_mask() {
    // Lucide keeps two units of margin on every side, so a stroke that reached
    // the edge of the mask would mean the geometry, the scale or the stroke
    // width was wrong — and it would be *clipped*, which is the one error a
    // glance at the icon does not reveal.
    for icon in all() {
        let mask = rasterize(icon, 24, STROKE_WIDTH);
        for i in 0..24 {
            for pixel in [
                at(&mask, i, 0),
                at(&mask, i, 23),
                at(&mask, 0, i),
                at(&mask, 23, i),
            ] {
                assert_eq!(pixel, 0, "{} paints its own border", icon.name());
            }
        }
    }
}

#[test]
fn the_cross_is_ink_through_the_middle_and_nothing_in_the_corners() {
    let mask = rasterize(Lucide::X, 24, STROKE_WIDTH);

    assert_eq!(at(&mask, 12, 12), 255, "both strokes cross at the centre");
    for (x, y) in [(1, 1), (22, 1), (1, 22), (22, 22), (12, 2), (2, 12)] {
        assert_eq!(at(&mask, x, y), 0, "({x}, {y}) is not on either stroke");
    }
}

#[test]
fn the_play_triangle_points_right_and_is_hollow() {
    let mask = rasterize(Lucide::Play, 24, STROKE_WIDTH);

    // Stroked rather than filled, which is what every Lucide icon is: the SVG
    // is `fill="none"` and the rasterizer strokes it. Worth pinning because
    // this is the first closed curved outline in the set, and a filled
    // triangle would be the only solid mark in Crook's chrome.
    assert_eq!(
        at(&mask, 12, 12),
        0,
        "the middle of a stroked outline is empty"
    );

    // Which way it points — the one thing a mirrored path would get wrong
    // while still drawing a plausible triangle. The flat back is a tall
    // straight edge on the left; the tip is a point on the right.
    let back = (0..24).filter(|y| at(&mask, 5, *y) > 200).count();
    let tip = (0..24).filter(|y| at(&mask, 21, *y) > 200).count();

    assert!(back > 12, "the back edge covers only {back} rows");
    assert!(tip < 4, "the tip covers {tip} rows, and a tip is a point");
}

#[test]
fn a_symmetric_icon_rasterizes_symmetrically() {
    // A cross is symmetric about both axes, so the mask must be too. This is
    // the coverage rule's own test: an off-by-half-a-pixel in it would show up
    // as one arm heavier than the other rather than as anything visible.
    let mask = rasterize(Lucide::X, 32, STROKE_WIDTH);

    for y in 0..32 {
        for x in 0..32 {
            assert_eq!(
                at(&mask, x, y),
                at(&mask, 31 - x, y),
                "({x}, {y}) does not match its mirror across the vertical"
            );
            assert_eq!(
                at(&mask, x, y),
                at(&mask, x, 31 - y),
                "({x}, {y}) does not match its mirror across the horizontal"
            );
        }
    }
}

#[test]
fn a_subpath_that_draws_nothing_still_draws_its_dot() {
    // `info` puts the dot on its `i` with `M12 8 h.01` — a subpath a hundredth
    // of a unit long, which is a dot because the cap is round. A stroker that
    // discards zero-length subpaths loses it, and the icon becomes an `i`
    // with no dot at every size.
    let mask = rasterize(Lucide::Info, 48, STROKE_WIDTH);

    assert!(
        at(&mask, 24, 16) > 200,
        "the dot at (12, 8) in the grid is missing"
    );
}

#[test]
fn a_heavier_stroke_is_more_ink() {
    let normal = ink(&rasterize(Lucide::Plus, 32, STROKE_WIDTH));
    let heavy = ink(&rasterize(Lucide::Plus, 32, STROKE_WIDTH * 1.5));

    assert!(
        heavy > normal * 1.3,
        "a stroke half again as wide drew {heavy} against {normal}"
    );
}

#[test]
fn a_stroke_thinner_than_a_pixel_still_draws() {
    // At 12 pixels Lucide's own stroke is exactly one pixel, and at 8 it is
    // two thirds of one. The coverage rule is exact for a straight band at any
    // width, so the plus — four straight runs — should carry very nearly its
    // geometric ink rather than fading out or snapping to a whole pixel.
    let mask = rasterize(Lucide::Plus, 8, STROKE_WIDTH);
    let stroke = STROKE_WIDTH * 8. / GRID;
    let length = 14. * 8. / GRID;

    assert!(
        (ink(&mask) - 2. * stroke * length).abs() < 1.,
        "{} of ink against a geometric {}",
        ink(&mask),
        2. * stroke * length
    );
}

#[test]
fn a_mask_with_no_pixels_in_it_is_not_an_error() {
    let mask = rasterize(Lucide::Check, 0, STROKE_WIDTH);

    assert_eq!(mask.size, (0, 0));
    assert!(mask.pixels.is_empty());
}

#[test]
fn two_sizes_of_one_icon_are_two_keys() {
    use std::collections::HashSet;

    let keys: HashSet<IconKey> = [
        IconKey::new(Lucide::X, 14.),
        IconKey::new(Lucide::X, 16.),
        IconKey::new(Lucide::Plus, 14.),
        IconKey::new(Lucide::X, 14.),
        IconKey {
            stroke_width: 3.,
            ..IconKey::new(Lucide::X, 14.)
        },
    ]
    .into_iter()
    .collect();

    assert_eq!(
        keys.len(),
        4,
        "the repeat of the first key is the only pair"
    );
}

/// A face's coverage at the centre of the head, where a mark that was stroked
/// rather than filled would have nothing.
#[test]
fn a_drawing_is_filled_rather_than_hollow() {
    let face = rasterize(Art::PirateFace(Chomp::Shut), 24, STROKE_WIDTH);

    assert_eq!(
        at(&face, 12, 12),
        255,
        "the middle of the pirate's head came out empty, which is what a \
         stroked outline looks like"
    );
}

#[test]
fn the_bite_takes_ink_out_of_the_face() {
    let shut = ink(&rasterize(Art::PirateFace(Chomp::Shut), 48, STROKE_WIDTH));
    let open = ink(&rasterize(Art::PirateFace(Chomp::Open), 48, STROKE_WIDTH));
    let wide = ink(&rasterize(Art::PirateFace(Chomp::Wide), 48, STROKE_WIDTH));

    assert!(
        wide < open && open < shut,
        "the frames are meant to be a mouth opening: {shut} shut, {open} open, {wide} wide"
    );

    // A shut mouth is a whole disc, and a bitten one is that disc minus a
    // wedge — which is most of the way to pi over four either way.
    let disc = std::f32::consts::PI * 24. * 24.;
    assert!((shut - disc).abs() / disc < 0.01, "{shut} is not a disc");
}

#[test]
fn the_ink_never_leaves_the_head_it_is_drawn_on() {
    // The strap runs off both edges of the box in the artwork and the grin is
    // an arc of a circle that is mostly outside it; both are cut to the face
    // rather than to a rectangle, so a pixel the face does not cover cannot
    // hold ink however far the geometry reaches.
    for chomp in [Chomp::Shut, Chomp::Open, Chomp::Wide] {
        let face = rasterize(Art::PirateFace(chomp), 32, STROKE_WIDTH);
        let ink_mask = rasterize(Art::PirateInk(chomp), 32, STROKE_WIDTH);

        for (i, (ink, face)) in ink_mask.pixels.iter().zip(&face.pixels).enumerate() {
            assert!(
                ink <= face,
                "{chomp:?} draws ink at pixel {i} where the face has none"
            );
        }
        assert!(
            ink(&ink_mask) > 1.,
            "{chomp:?} lost its eyepatch and its strap altogether"
        );
    }
}

#[test]
fn a_shape_that_reaches_the_border_does_not_bleed_into_the_next_row() {
    // The head is a disc inscribed in the box, so it touches the right border
    // exactly. The area that balances that crossing has to land outside the
    // mask: a column further right, where nothing reads it. Landing at the
    // start of the row below instead would be carried across that whole row
    // by the fill's running sum, and the mask would come out with a bar down
    // one side of it.
    let head = rasterize(Art::PirateFace(Chomp::Shut), 32, STROKE_WIDTH);

    for y in [0, 1, 2, 29, 30, 31] {
        assert_eq!(
            at(&head, 0, y),
            0,
            "row {y} starts with ink no part of the disc is near"
        );
    }
}
