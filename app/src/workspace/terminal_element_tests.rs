//! What a grid actually puts in the scene.
//!
//! Painted against [`CellFont::headless`], where a character is its own glyph
//! id and a cell is half the font size — so an assertion here is about the grid
//! rather than about which fonts the machine running it happens to have.

use crook_terminal::CellCombining;
use crookui_core::scene::{ClipBounds, Fill, Glyph, Rect};

use super::*;
use crate::terminal_font::CELL_FONT_SIZE;

/// The colours a hand-built snapshot uses, chosen so none of them collides.
const GROUND: Rgb = Rgb::new(10, 10, 10);
const INK: Rgb = Rgb::new(200, 200, 200);
const HIGHLIGHT: Rgb = Rgb::new(180, 30, 30);

fn font() -> CellFont {
    CellFont::headless(CELL_FONT_SIZE)
}

/// A snapshot of `rows` lines built from the characters in `text`, one line per
/// entry, padded with blanks on the grid's own background.
fn snapshot(lines: &[&str], columns: usize) -> Snapshot {
    let rows = lines.len();
    let mut cells = Vec::with_capacity(rows * columns);
    for line in lines {
        let mut characters = line.chars();
        for _ in 0..columns {
            cells.push(SnapshotCell {
                c: characters.next().unwrap_or(' '),
                foreground: INK,
                background: GROUND,
                flags: CellFlags::NONE,
            });
        }
    }

    Snapshot {
        revision: 1,
        columns,
        rows,
        cells,
        combining: Vec::new(),
        cursor: None,
        foreground: INK,
        background: GROUND,
        display_offset: 0,
        history_len: 0,
        alt_screen: false,
        title: None,
    }
}

/// Paints a snapshot into a scene big enough for all of it.
fn painted(snapshot: &Snapshot) -> Scene {
    let font = font();
    let metrics = font.metrics();
    let size = vec2f(
        snapshot.columns as f32 * metrics.width,
        snapshot.rows as f32 * metrics.height,
    );

    let mut scene = Scene::new(1.);
    scene.start_layer(ClipBounds::None);
    paint_grid(snapshot, &font, Vector2F::zero(), size, true, &mut scene);
    scene.stop_layer();
    scene
}

fn glyphs(scene: &Scene) -> Vec<Glyph> {
    scene
        .layers()
        .flat_map(|layer| layer.glyphs.iter())
        .cloned()
        .collect()
}

fn rects(scene: &Scene) -> Vec<Rect> {
    scene
        .layers()
        .flat_map(|layer| layer.rects.iter())
        .cloned()
        .collect()
}

#[test]
fn an_untouched_screen_costs_one_rectangle_and_no_glyphs() {
    // The whole reason a terminal can be repainted at all: eighty columns of
    // nothing must not be eighty rectangles and eighty notdef boxes.
    let scene = painted(&snapshot(&["", "", ""], 80));

    assert_eq!(rects(&scene).len(), 1, "one ground, and nothing else");
    assert!(glyphs(&scene).is_empty(), "a blank cell draws no glyph");
}

#[test]
fn every_cell_gets_its_own_glyph_at_its_own_column() {
    let font = font();
    let metrics = font.metrics();
    let scene = painted(&snapshot(&["ab", "c"], 4));
    let painted = glyphs(&scene);

    assert_eq!(painted.len(), 3);
    let ids: Vec<_> = painted
        .iter()
        .map(|glyph| glyph.glyph_key.glyph_id)
        .collect();
    assert_eq!(ids, vec![u32::from('a'), u32::from('b'), u32::from('c')]);

    // A fixed advance per column, and the baseline of the row it is on.
    assert_eq!(painted[0].position, vec2f(0., metrics.baseline));
    assert_eq!(painted[1].position, vec2f(metrics.width, metrics.baseline));
    assert_eq!(
        painted[2].position,
        vec2f(0., metrics.height + metrics.baseline)
    );
    assert_eq!(painted[0].glyph_key.font_size, metrics.font_size);
}

#[test]
fn a_cell_is_painted_in_the_colour_the_emulator_resolved() {
    // Nothing here consults a theme: the snapshot's colours are final, which is
    // what lets a per-cell foreground cost nothing to honour.
    let mut grid = snapshot(&["xy"], 2);
    grid.cells[1].foreground = HIGHLIGHT;

    let painted = glyphs(&painted(&grid));
    assert_eq!(painted[0].color, Color::rgb(INK.r, INK.g, INK.b));
    assert_eq!(
        painted[1].color,
        Color::rgb(HIGHLIGHT.r, HIGHLIGHT.g, HIGHLIGHT.b)
    );
}

#[test]
fn neighbouring_cells_sharing_a_background_become_one_rectangle() {
    // A coloured `ls` line is runs, not columns. Painting it per cell would put
    // one instanced quad per column into every frame it is on screen.
    let mut grid = snapshot(&["abcdef"], 6);
    for cell in &mut grid.cells[1..4] {
        cell.background = HIGHLIGHT;
    }

    let painted = rects(&painted(&grid));
    let metrics = font().metrics();
    assert_eq!(painted.len(), 2, "the ground, and one run over it");
    assert_eq!(painted[1].bounds.origin(), vec2f(metrics.width, 0.));
    assert_eq!(painted[1].bounds.width(), metrics.width * 3.);
}

#[test]
fn a_block_cursor_fills_its_cell_and_the_character_in_it_is_inverted() {
    let mut grid = snapshot(&["hi"], 2);
    grid.cursor = Some(Cursor {
        row: 0,
        column: 1,
        shape: CursorShape::Block,
        color: HIGHLIGHT,
    });

    let scene = painted(&grid);
    let metrics = font().metrics();
    let cursor = rects(&scene).pop().expect("the cursor is painted last");
    assert_eq!(cursor.bounds.origin(), vec2f(metrics.width, 0.));
    assert_eq!(cursor.bounds.size(), vec2f(metrics.width, metrics.height));

    let painted = glyphs(&scene);
    assert_eq!(
        painted[1].color,
        Color::rgb(GROUND.r, GROUND.g, GROUND.b),
        "a character drawn in its own colour on a filled cursor is invisible"
    );
    assert_eq!(
        painted[0].color,
        Color::rgb(INK.r, INK.g, INK.b),
        "and only the cell the cursor is on is inverted"
    );
}

#[test]
fn a_beam_cursor_is_a_rule_down_the_left_of_its_cell() {
    let mut grid = snapshot(&["  "], 2);
    grid.cursor = Some(Cursor {
        row: 0,
        column: 0,
        shape: CursorShape::Beam,
        color: HIGHLIGHT,
    });

    let cursor = rects(&painted(&grid))
        .pop()
        .expect("a cursor was asked for");
    assert_eq!(cursor.bounds.width(), CURSOR_STROKE);
    assert_eq!(cursor.bounds.height(), font().metrics().height);
}

#[test]
fn a_cursor_scrolled_out_of_the_visible_grid_is_not_drawn() {
    // The emulator reports the cursor in viewport coordinates and the pane may
    // be showing fewer rows than the snapshot carries for one frame after a
    // resize; a cursor drawn past the edge would land on the pane below.
    let mut grid = snapshot(&["a", "b", "c"], 1);
    grid.cursor = Some(Cursor {
        row: 2,
        column: 0,
        shape: CursorShape::Block,
        color: HIGHLIGHT,
    });

    let metrics = font().metrics();
    let mut scene = Scene::new(1.);
    paint_grid(
        &grid,
        &font(),
        Vector2F::zero(),
        vec2f(metrics.width, metrics.height * 2.),
        true,
        &mut scene,
    );

    assert_eq!(rects(&scene).len(), 1, "the ground alone");
    assert_eq!(glyphs(&scene).len(), 2, "and only the rows that fit");
}

#[test]
fn a_double_width_character_draws_once_and_keeps_both_backgrounds() {
    let mut grid = snapshot(&["漢 "], 2);
    grid.cells[0].flags = CellFlags::WIDE;
    grid.cells[1].c = ' ';
    grid.cells[1].flags = CellFlags::WIDE_SPACER;
    grid.cells[1].background = HIGHLIGHT;
    grid.cells[0].background = HIGHLIGHT;

    let scene = painted(&grid);
    assert_eq!(glyphs(&scene).len(), 1, "the pair carries one character");
    assert_eq!(
        rects(&scene)[1].bounds.width(),
        font().metrics().width * 2.,
        "and one background across both of its columns"
    );
}

#[test]
fn an_underline_is_drawn_under_a_cell_that_asked_for_one() {
    let mut grid = snapshot(&["a"], 1);
    grid.cells[0].flags = CellFlags::UNDERLINE;

    let scene = painted(&grid);
    let metrics = font().metrics();
    let rule = rects(&scene).pop().expect("a rule was asked for");
    assert!(
        rule.bounds.origin().y() > metrics.baseline,
        "an underline above the baseline is a strikeout"
    );
    assert_eq!(rule.bounds.width(), metrics.width);
}

#[test]
fn a_grid_fills_the_space_it_is_given_even_when_the_parent_is_unbounded() {
    // A flex asks "how big would you like to be?" with an infinite maximum, and
    // a grid painted at an infinite size is a panic in the scene's debug
    // assertions rather than a frame.
    assert_eq!(bounded(f32::INFINITY, 320.), 320.);
    assert_eq!(bounded(640., 0.), 640.);
}

/// Paints a snapshot at its natural size, as an unfocused pane would.
fn painted_unfocused(snapshot: &Snapshot) -> Scene {
    let font = font();
    let metrics = font.metrics();
    let size = vec2f(
        snapshot.columns as f32 * metrics.width,
        snapshot.rows as f32 * metrics.height,
    );

    let mut scene = Scene::new(1.);
    scene.start_layer(ClipBounds::None);
    paint_grid(snapshot, &font, Vector2F::zero(), size, false, &mut scene);
    scene.stop_layer();
    scene
}

#[test]
fn a_run_of_underlined_cells_is_one_rule_rather_than_one_per_column() {
    // `man` through `less` renders italics as underline; a full-width line of
    // it used to be one rectangle per column, most of them abutting duplicates.
    let mut grid = snapshot(&["underlined"], 80);
    for cell in &mut grid.cells {
        cell.flags = CellFlags::UNDERLINE;
    }

    let painted = rects(&painted(&grid));
    let metrics = font().metrics();
    assert_eq!(painted.len(), 2, "the ground, and one rule across the row");
    assert_eq!(painted[1].bounds.origin().x(), 0.);
    assert_eq!(painted[1].bounds.width(), metrics.width * 80.);
}

#[test]
fn a_rule_breaks_where_the_colour_does_and_where_the_flag_does() {
    let mut grid = snapshot(&["abcdef"], 6);
    for cell in &mut grid.cells[..5] {
        cell.flags = CellFlags::UNDERLINE;
    }
    grid.cells[3].foreground = HIGHLIGHT;
    grid.cells[4].foreground = HIGHLIGHT;

    let painted = rects(&painted(&grid));
    let metrics = font().metrics();
    assert_eq!(painted.len(), 3, "the ground and two runs");
    assert_eq!(painted[1].bounds.width(), metrics.width * 3.);
    assert_eq!(painted[2].bounds.origin().x(), metrics.width * 3.);
    assert_eq!(painted[2].bounds.width(), metrics.width * 2.);
}

#[test]
fn an_underline_and_a_strikeout_on_the_same_run_are_two_rules() {
    let mut grid = snapshot(&["ab"], 2);
    for cell in &mut grid.cells {
        cell.flags = CellFlags::UNDERLINE | CellFlags::STRIKEOUT;
    }

    let painted = rects(&painted(&grid));
    let metrics = font().metrics();
    assert_eq!(painted.len(), 3);
    assert!(
        painted[1].bounds.origin().y() > metrics.baseline,
        "the underline sits below the baseline"
    );
    assert!(
        painted[2].bounds.origin().y() < metrics.baseline,
        "and the strikeout above it"
    );
}

#[test]
fn an_unfocused_pane_outlines_its_cursor_instead_of_filling_it() {
    // Two panes both painting a filled block in the accent colour say nothing
    // about which of them is going to receive what is typed.
    let mut grid = snapshot(&["hi"], 2);
    grid.cursor = Some(Cursor {
        row: 0,
        column: 1,
        shape: CursorShape::Block,
        color: HIGHLIGHT,
    });

    let scene = painted_unfocused(&grid);
    let cursor = rects(&scene).pop().expect("the cursor is painted last");
    assert_eq!(cursor.background, Fill::None, "an outline fills nothing");
    assert_eq!(
        cursor.border.width, CURSOR_STROKE,
        "and is a stroke instead"
    );

    assert!(
        glyphs(&scene)
            .iter()
            .all(|glyph| glyph.color == Color::rgb(INK.r, INK.g, INK.b)),
        "nothing is inverted, because nothing was filled over"
    );
}

#[test]
fn a_beam_cursor_is_still_outlined_when_the_pane_is_not_listening() {
    // The shape says what the program wants; focus says whether that means
    // anything right now.
    let mut grid = snapshot(&["  "], 2);
    grid.cursor = Some(Cursor {
        row: 0,
        column: 0,
        shape: CursorShape::Beam,
        color: HIGHLIGHT,
    });

    let metrics = font().metrics();
    let cursor = rects(&painted_unfocused(&grid))
        .pop()
        .expect("a cursor was asked for");
    assert_eq!(cursor.bounds.size(), vec2f(metrics.width, metrics.height));
    assert_eq!(cursor.background, Fill::None);
    assert_eq!(cursor.border.width, CURSOR_STROKE);
}

#[test]
fn a_combining_mark_is_drawn_over_the_character_it_belongs_to() {
    // macOS hands out decomposed text, so `José` arrives as `Jose` plus an
    // accent that has no column of its own.
    let mut grid = snapshot(&["e!"], 2);
    grid.combining = vec![CellCombining {
        cell: 0,
        characters: Box::new(['\u{301}']),
    }];

    let painted = glyphs(&painted(&grid));
    assert_eq!(painted.len(), 3);
    assert_eq!(painted[0].glyph_key.glyph_id, u32::from('e'));
    assert_eq!(painted[1].glyph_key.glyph_id, u32::from('\u{301}'));
    assert_eq!(
        painted[0].position, painted[1].position,
        "a mark carries its own offset and shares the character's pen"
    );
    assert_eq!(painted[2].glyph_key.glyph_id, u32::from('!'));
}
