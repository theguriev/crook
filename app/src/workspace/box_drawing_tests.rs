//! What each character is drawn as, pixel by pixel.
//!
//! A character is drawn into a cell of a chosen size at a scale of one and
//! read back as a picture: `#` where a rectangle in the full ink covers a
//! pixel's centre, `+` where only a translucent one does, and `.` where none
//! does. The pictures are the specification — a corner whose arms miss each
//! other, or a stroke a pixel off the centre line, is visible in one at a
//! glance.

use crookui_core::geometry::Vector2F;
use crookui_core::scene::{Fill, Rect};

use super::*;

const INK: Color = Color::rgb(200, 200, 200);

/// The size the grid is set at in these tests, at which a light line is one
/// device pixel at a scale of one.
const FONT_SIZE: f32 = 12.5;

/// The rectangles `character` is drawn as in `cell`, at `scale`, or `None`
/// when it is left to the font.
fn drawn_at(character: char, cell: RectF, scale: f32) -> Option<Vec<Rect>> {
    let mut scene = Scene::new(scale);
    paint(character, cell, FONT_SIZE, INK, &mut scene).then(|| {
        scene
            .layers()
            .flat_map(|layer| layer.rects.iter().cloned())
            .collect()
    })
}

/// The same, in a `width` by `height` cell at the origin, at a scale of one.
fn drawn(character: char, width: i32, height: i32) -> Option<Vec<Rect>> {
    drawn_at(
        character,
        RectF::new(Vector2F::zero(), vec2f(width as f32, height as f32)),
        1.,
    )
}

/// `character` in a `width` by `height` cell, as rows of pixels.
fn picture(character: char, width: i32, height: i32) -> Vec<String> {
    let rects = drawn(character, width, height)
        .unwrap_or_else(|| panic!("{character} is left to the font"));
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| {
                    let centre = vec2f(x as f32 + 0.5, y as f32 + 0.5);
                    let alpha = rects
                        .iter()
                        .filter(|rect| rect.bounds.contains_point(centre))
                        .filter_map(|rect| match rect.background {
                            Fill::Solid(color) => Some(color.a),
                            _ => None,
                        })
                        .max();
                    match alpha {
                        None => '.',
                        Some(255) => '#',
                        Some(_) => '+',
                    }
                })
                .collect()
        })
        .collect()
}

/// Every character this module draws, the diagonals aside.
fn every_drawn_character() -> impl Iterator<Item = char> {
    (0x2500..=0x259F)
        .chain(0x2800..=0x28FF)
        .filter(|code| !(0x2571..=0x2573).contains(code))
        .map(|code| char::from_u32(code).expect("a scalar value"))
}

/// Every line character, which is to say every one drawn as arms reaching to
/// the cell's edges: not the dashes, which stop short of them on purpose, nor
/// the arcs, which are not filled rectangles.
fn every_line_character() -> impl Iterator<Item = char> {
    every_drawn_character()
        .filter(|&character| matches!(Shape::of(character), Some(Shape::Lines(_))))
}

#[test]
fn every_line_block_and_braille_character_is_drawn_here_but_the_diagonals() {
    for character in every_drawn_character() {
        assert!(
            drawn(character, 7, 9).is_some(),
            "{character} (U+{:04X}) is left to the font",
            u32::from(character)
        );
    }
    for character in ['╱', '╲', '╳'] {
        assert!(
            drawn(character, 7, 9).is_none(),
            "{character} is a slope, which is the font's to draw"
        );
    }
    // Either side of each range, and a powerline separator, which is a
    // triangle.
    for character in ['a', ' ', '\u{24FF}', '■', '\u{27FF}', '⤀', '\u{E0B0}'] {
        assert!(
            drawn(character, 7, 9).is_none(),
            "{character} is drawn here"
        );
    }
}

#[test]
fn nothing_is_drawn_outside_the_cell() {
    // A rectangle past the edge would be painted over the cell beside it,
    // under whatever character that one holds.
    for character in every_drawn_character() {
        for rect in drawn(character, 7, 9).expect("drawn here") {
            let bounds = rect.bounds;
            assert!(
                bounds.min_x() >= 0.
                    && bounds.min_y() >= 0.
                    && bounds.max_x() <= 7.
                    && bounds.max_y() <= 9.,
                "{character} draws {bounds:?} outside its cell"
            );
        }
    }
}

#[test]
fn a_line_reaches_exactly_the_edges_its_arms_point_at_in_the_stroke_its_neighbour_meets() {
    // What makes a box out of cells: the arm a `─` has on its right has to
    // land on the very pixels the arm of the `┐` beside it does on its left,
    // and an edge with no arm must be empty, or a stub pokes into the next
    // cell's line. In a cell seven wide and nine tall the light stroke runs
    // down column 3 and along row 4.
    let across_columns = |weight| match weight {
        Weight::None => "",
        Weight::Light => "3",
        Weight::Heavy => "23",
        Weight::Double => "24",
    };
    let across_rows = |weight| match weight {
        Weight::None => "",
        Weight::Light => "4",
        Weight::Heavy => "34",
        Weight::Double => "35",
    };
    for character in every_line_character() {
        let Some(Shape::Lines(arms)) = Shape::of(character) else {
            unreachable!("filtered to lines");
        };
        let picture = picture(character, 7, 9);
        let filled = |pixels: Vec<char>| -> String {
            pixels
                .into_iter()
                .enumerate()
                .filter(|(_, pixel)| *pixel == '#')
                .map(|(at, _)| char::from_digit(at as u32, 10).expect("a digit"))
                .collect()
        };
        let row = |y: usize| filled(picture[y].chars().collect());
        let column = |x: usize| {
            filled(
                picture
                    .iter()
                    .map(|line| line.chars().nth(x).expect("in the cell"))
                    .collect(),
            )
        };

        assert_eq!(row(0), across_columns(arms.up), "the top of {character}");
        assert_eq!(
            row(8),
            across_columns(arms.down),
            "the bottom of {character}"
        );
        assert_eq!(column(0), across_rows(arms.left), "the left of {character}");
        assert_eq!(
            column(6),
            across_rows(arms.right),
            "the right of {character}"
        );
    }
}

#[test]
fn a_cross_is_two_lines_through_the_middle() {
    assert_eq!(
        picture('┼', 7, 9),
        [
            "...#...", //
            "...#...", //
            "...#...", //
            "...#...", //
            "#######", //
            "...#...", //
            "...#...", //
            "...#...", //
            "...#...", //
        ]
    );
}

#[test]
fn a_heavy_corner_is_twice_as_thick_and_square_on_the_outside() {
    assert_eq!(
        picture('┏', 7, 9),
        [
            ".......", //
            ".......", //
            ".......", //
            "..#####", //
            "..#####", //
            "..##...", //
            "..##...", //
            "..##...", //
            "..##...", //
        ]
    );
}

#[test]
fn a_double_corner_is_two_nested_corners() {
    assert_eq!(
        picture('╔', 7, 9),
        [
            ".......", //
            ".......", //
            ".......", //
            "..#####", //
            "..#....", //
            "..#.###", //
            "..#.#..", //
            "..#.#..", //
            "..#.#..", //
        ]
    );
}

#[test]
fn a_double_cross_leaves_the_middle_open() {
    assert_eq!(
        picture('╬', 7, 9),
        [
            "..#.#..", //
            "..#.#..", //
            "..#.#..", //
            "###.###", //
            ".......", //
            "###.###", //
            "..#.#..", //
            "..#.#..", //
            "..#.#..", //
        ]
    );
}

#[test]
fn a_single_stem_on_a_double_line_touches_it_without_filling_its_gap() {
    assert_eq!(
        picture('╤', 7, 9),
        [
            ".......", //
            ".......", //
            ".......", //
            "#######", //
            ".......", //
            "#######", //
            "...#...", //
            "...#...", //
            "...#...", //
        ]
    );
}

#[test]
fn a_line_that_changes_weight_does_so_in_the_middle() {
    assert_eq!(
        picture('╽', 7, 9),
        [
            "...#...", //
            "...#...", //
            "...#...", //
            "...#...", //
            "..##...", //
            "..##...", //
            "..##...", //
            "..##...", //
            "..##...", //
        ]
    );
}

#[test]
fn a_dashed_line_splits_its_gaps_between_the_ends_so_a_column_of_them_is_even() {
    // The gaps at the top and the bottom are one pixel each, so where one
    // row's `┆` meets the next the gap is two — the same as inside a cell.
    assert_eq!(
        picture('┆', 7, 18),
        [
            ".......", //
            "...#...", //
            "...#...", //
            "...#...", //
            "...#...", //
            ".......", //
            ".......", //
            "...#...", //
            "...#...", //
            "...#...", //
            "...#...", //
            ".......", //
            ".......", //
            "...#...", //
            "...#...", //
            "...#...", //
            "...#...", //
            ".......", //
        ]
    );
}

#[test]
fn the_half_blocks_tile_the_cell_between_them() {
    let (upper, lower) = (picture('▀', 7, 9), picture('▄', 7, 9));
    for (y, (upper, lower)) in upper.iter().zip(&lower).enumerate() {
        assert!(
            (upper == "#######") != (lower == "#######"),
            "row {y} is in exactly one half: {upper} {lower}"
        );
    }
    assert_eq!(
        picture('▐', 8, 2),
        ["....####", "....####"],
        "and the right half is the right half"
    );
    assert_eq!(picture('█', 7, 9), vec!["#######"; 9]);
}

#[test]
fn the_eighths_grow_from_their_own_edge() {
    assert_eq!(
        picture('▁', 3, 8),
        ["...", "...", "...", "...", "...", "...", "...", "###"]
    );
    assert_eq!(
        picture('▇', 3, 8),
        ["...", "###", "###", "###", "###", "###", "###", "###"]
    );
    assert_eq!(picture('▔', 3, 8)[..2], ["###", "..."]);
    assert_eq!(picture('▏', 8, 1), ["#......."]);
    assert_eq!(picture('▊', 8, 1), ["######.."]);
    assert_eq!(picture('▕', 8, 1), [".......#"]);
}

#[test]
fn a_quadrant_character_fills_the_quarters_its_name_says() {
    assert_eq!(
        picture('▚', 8, 8),
        [
            "####....", //
            "####....", //
            "####....", //
            "####....", //
            "....####", //
            "....####", //
            "....####", //
            "....####", //
        ]
    );
    assert_eq!(
        picture('▟', 4, 4),
        ["..##", "..##", "####", "####"],
        "upper right, lower left and lower right"
    );
}

#[test]
fn the_shades_are_the_whole_cell_in_the_ink_at_a_quarter_a_half_and_three_quarters() {
    for (character, alpha) in [('░', 63), ('▒', 127), ('▓', 191)] {
        let rects = drawn(character, 7, 9).expect("drawn here");
        assert_eq!(rects.len(), 1, "{character}");
        assert_eq!(rects[0].bounds, RectF::new(Vector2F::zero(), vec2f(7., 9.)));
        assert_eq!(rects[0].background, Fill::Solid(INK.with_alpha(alpha)));
    }
}

#[test]
fn braille_dots_are_numbered_down_the_left_then_down_the_right_then_along_the_bottom() {
    // ⡇ is dots 1, 2, 3 and 7: the whole left column, its bottom dot being
    // the one Unicode numbered last.
    assert_eq!(
        picture('⡇', 8, 16),
        [
            "........", //
            ".##.....", //
            ".##.....", //
            "........", //
            "........", //
            ".##.....", //
            ".##.....", //
            "........", //
            "........", //
            ".##.....", //
            ".##.....", //
            "........", //
            "........", //
            ".##.....", //
            ".##.....", //
            "........", //
        ]
    );
    assert_eq!(
        picture('⠈', 8, 16)[..3],
        ["........", ".....##.", ".....##."],
        "dot 4 is the top right"
    );
    assert_eq!(
        picture('⢀', 8, 16)[13],
        ".....##.",
        "dot 8 is the bottom right"
    );
    assert!(
        picture('⠀', 8, 16).iter().all(|row| row == "........"),
        "the blank pattern is blank"
    );
}

#[test]
fn braille_dots_are_as_far_apart_across_a_row_boundary_as_within_a_cell() {
    // A graph drawn in braille is a column of cells, and an uneven gap at
    // every row would stripe it.
    let full = picture('⣿', 8, 16);
    let lit: Vec<usize> = full
        .iter()
        .enumerate()
        .filter(|(_, row)| row.contains('#'))
        .map(|(y, _)| y)
        .collect();
    assert_eq!(lit, [1, 2, 5, 6, 9, 10, 13, 14]);
    let (above, below) = (lit[0], full.len() - 1 - lit[lit.len() - 1]);
    assert_eq!(
        above + below,
        lit[2] - lit[1] - 1,
        "the gap where one cell meets the next is the gap between two dots"
    );
}

#[test]
fn a_rounded_corner_is_a_quarter_circle_from_the_middle_out_to_its_two_edges() {
    // The renderer's rounded rectangle is the one curve the scene has, and a
    // rectangle stroked on two sides and rounded between them is exactly a
    // corner whose straight ends lie on the cell's centre lines.
    let rects = drawn('╭', 7, 9).expect("drawn here");
    assert_eq!(rects.len(), 1);
    let corner = &rects[0];
    assert_eq!(
        corner.bounds,
        RectF::from_points(vec2f(3., 4.), vec2f(7., 9.)),
        "from the light stroke's lines out to the right and bottom edges"
    );
    assert_eq!(corner.background, Fill::None, "a stroke, not a fill");
    assert_eq!(
        corner.border,
        Border::new(1.)
            .with_sides(true, true, false, false)
            .with_border_color(INK),
        "along the top and down the left, a light line thick"
    );
    assert_eq!(corner.corner_radius.get_top_left(), Radius::Pixels(2.));
    assert_eq!(corner.corner_radius.get_bottom_right(), Radius::Pixels(0.));

    for (character, top, left) in [('╮', true, false), ('╯', false, false), ('╰', false, true)]
    {
        let rects = drawn(character, 7, 9).expect("drawn here");
        let border = rects[0].border;
        assert_eq!(
            (border.top, border.left, border.bottom, border.right),
            (top, left, !top, !left),
            "{character} is stroked on the sides it turns between"
        );
    }
}

#[test]
fn a_light_line_is_a_device_pixel_at_one_times_and_two_at_two_and_a_heavy_one_twice_that() {
    let cell = RectF::new(Vector2F::zero(), vec2f(7., 9.));
    let thickness = |character, scale: f32| {
        let rects = drawn_at(character, cell, scale).expect("drawn here");
        rects[0].bounds.width() * scale
    };
    assert_eq!(thickness('│', 1.), 1.);
    assert_eq!(thickness('│', 2.), 2.);
    assert_eq!(thickness('┃', 1.), 2.);
    assert_eq!(thickness('┃', 2.), 4.);
}

#[test]
fn at_a_fractional_scale_and_origin_a_column_of_lines_still_meets_from_row_to_row() {
    // At 150% a fifteen-pixel row is twenty-two and a half device pixels, so
    // the rows' edges fall between pixels at every other boundary, and a
    // stroke that ended between two would cover whichever pixel centres it
    // happened to. Each edge is snapped on its own, so a row's bottom is the
    // very pixel the next row's top is: no gap, and no stub poking into it.
    let scale = 1.5;
    let (left, top, width, height) = (10.3, 7.7, 6.25, 15.);
    let device = |logical: f32| logical * scale;
    let whole = |value: f32| (value - value.round()).abs() < 1e-3;
    let rows: Vec<(f32, f32)> = (0..6)
        .map(|row| {
            let cell = RectF::new(vec2f(left, top + row as f32 * height), vec2f(width, height));
            let strokes = drawn_at('│', cell, scale).expect("drawn here");
            let mut reached = device(strokes[0].bounds.min_y());
            for stroke in strokes.iter().map(|rect| rect.bounds) {
                assert!(
                    [
                        stroke.min_x(),
                        stroke.max_x(),
                        stroke.min_y(),
                        stroke.max_y()
                    ]
                    .into_iter()
                    .all(|edge| whole(device(edge))),
                    "{stroke:?} is not on the device pixel grid"
                );
                assert!(
                    device(stroke.min_y()) <= reached + 1e-3,
                    "a gap in row {row}"
                );
                reached = reached.max(device(stroke.max_y()));
                assert_eq!(
                    (stroke.min_x(), stroke.width()),
                    (strokes[0].bounds.min_x(), strokes[0].bounds.width()),
                    "in one place and at one weight"
                );
            }
            (device(strokes[0].bounds.min_y()), reached)
        })
        .collect();

    for pair in rows.windows(2) {
        assert!(
            (pair[0].1 - pair[1].0).abs() < 1e-3,
            "one row's line ends at {} and the next one's starts at {}",
            pair[0].1,
            pair[1].0
        );
    }
    let heights: Vec<f32> = rows
        .iter()
        .map(|(start, end)| (end - start).round())
        .collect();
    assert!(
        heights.contains(&22.) && heights.contains(&23.),
        "the rows really do fall between pixels: {heights:?}"
    );
}
