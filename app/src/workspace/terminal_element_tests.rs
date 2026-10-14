//! What a grid actually puts in the scene.
//!
//! Painted against [`CellFont::headless`], where a character is its own glyph
//! id and a cell is half the font size — so an assertion here is about the grid
//! rather than about which fonts the machine running it happens to have.

use crook_terminal::{BlockId, CellCombining, Emulator, LiveBlock, Palette, TerminalSize};
use crookui_core::scene::{ClipBounds, Fill, Glyph, PointerShape, Rect};

use super::*;
use crate::selection::Place;
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
        cursor_cell: None,
        foreground: INK,
        background: GROUND,
        display_offset: 0,
        history_len: 0,
        alt_screen: false,
        live_block: LiveBlock::default(),
        title: None,
        mouse_reporting: false,
        line_editor: false,
    }
}

/// A region from one cell to another, in the numbering a grid selection uses:
/// rows counted from the oldest line the scrollback holds.
fn region(start: (usize, usize), end: (usize, usize)) -> (BlockId, Region) {
    let id = LiveBlock::default().id;
    (
        id,
        Region {
            start: Place::new(id, start.0, start.1),
            end: Place::new(id, end.0, end.1),
            block: false,
        },
    )
}

/// Paints a snapshot into a scene big enough for all of it.
fn painted(snapshot: &Snapshot) -> Scene {
    paint(snapshot, None, true)
}

/// The same, with a region highlighted.
fn painted_selected(snapshot: &Snapshot, selected: (BlockId, Region)) -> Scene {
    paint(snapshot, Some(selected), true)
}

/// Paints a snapshot at its natural size.
fn paint(snapshot: &Snapshot, selected: Option<(BlockId, Region)>, owns_caret: bool) -> Scene {
    let font = font();
    let metrics = font.metrics();
    let size = vec2f(
        snapshot.columns as f32 * metrics.width,
        snapshot.rows as f32 * metrics.height,
    );

    let mut scene = Scene::new(1.);
    scene.start_layer(ClipBounds::None);
    paint_grid(
        snapshot,
        &font,
        Vector2F::zero(),
        size,
        owns_caret,
        &Washes {
            selected,
            ..Washes::default()
        },
        &mut scene,
    );
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
        &Washes::default(),
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

/// The middle of one cell of a grid painted at the origin.
fn middle_of(row: usize, column: usize) -> Vector2F {
    let metrics = font().metrics();
    vec2f(
        (column as f32 + 0.5) * metrics.width,
        (row as f32 + 0.5) * metrics.height,
    )
}

#[test]
fn the_pointer_over_a_grid_is_the_i_beam_a_selection_is_made_with() {
    let scene = painted(&snapshot(&["ls", ""], 10));

    assert_eq!(scene.pointer_at(middle_of(0, 1)), PointerShape::Text);
    assert_eq!(
        scene.pointer_at(middle_of(1, 8)),
        PointerShape::Text,
        "a blank cell is selected from like any other"
    );
    assert_eq!(scene.pointer_at(vec2f(-1., -1.)), PointerShape::Arrow);
}

#[test]
fn the_pointer_over_a_program_that_has_the_mouse_is_the_arrow() {
    // A press here goes down the pty to `vim`, not into a selection, and the
    // I-beam would be promising the selection.
    let mut grid = snapshot(&["~", "~"], 10);
    grid.mouse_reporting = true;

    let scene = painted(&grid);

    assert_eq!(scene.pointer_at(middle_of(0, 0)), PointerShape::Arrow);
}

#[test]
fn a_link_s_rule_comes_with_the_hand_over_its_cells_and_only_its_cells() {
    // The rule is drawn only while the chord key is held and the pointer is
    // on the link, and so is the hand: both say a click follows it.
    let grid = snapshot(&["see https://a.test now"], 24);
    let font = font();
    let metrics = font.metrics();
    // Into the layer the grid painted, as the element paints it.
    let mut scene = Scene::new(1.);
    scene.start_layer(ClipBounds::None);
    paint_grid(
        &grid,
        &font,
        Vector2F::zero(),
        vec2f(24. * metrics.width, metrics.height),
        true,
        &Washes::default(),
        &mut scene,
    );
    paint_link_rule(Vector2F::zero(), 4, 14, metrics, Color::WHITE, &mut scene);
    scene.stop_layer();

    assert_eq!(scene.pointer_at(middle_of(0, 4)), PointerShape::Hand);
    assert_eq!(scene.pointer_at(middle_of(0, 17)), PointerShape::Hand);
    assert_eq!(scene.pointer_at(middle_of(0, 18)), PointerShape::Text);
    assert_eq!(scene.pointer_at(middle_of(0, 1)), PointerShape::Text);
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
    paint(snapshot, None, false)
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

/// The rectangles the highlight put in the scene, left to right and top to
/// bottom, as (origin, size) pairs.
fn selection_rects(scene: &Scene) -> Vec<(Vector2F, Vector2F)> {
    rects(scene)
        .into_iter()
        .filter(|rect| rect.background == Fill::Solid(crate::theme::theme().selection))
        .map(|rect| (rect.bounds.origin(), rect.bounds.size()))
        .collect()
}

#[test]
fn a_selected_run_is_one_rectangle_and_the_text_on_it_is_untouched() {
    // The two halves of "the text stays readable": the highlight is one quad
    // behind the run rather than eighty, and every glyph is still drawn in the
    // colour the shell asked for.
    let font = font();
    let metrics = font.metrics();
    let scene = painted_selected(&snapshot(&["hello world"], 11), region((0, 6), (0, 10)));

    let highlight = selection_rects(&scene);
    assert_eq!(highlight.len(), 1, "one rectangle for one run of cells");
    assert_eq!(highlight[0].0, vec2f(6. * metrics.width, 0.));
    assert_eq!(highlight[0].1, vec2f(5. * metrics.width, metrics.height));

    let painted_in: Vec<_> = glyphs(&scene).iter().map(|glyph| glyph.color).collect();
    assert_eq!(
        painted_in.len(),
        10,
        "the space between the words draws nothing"
    );
    assert!(
        painted_in.iter().all(|ink| *ink == color(INK)),
        "selecting text repainted the text"
    );
}

#[test]
fn a_selection_over_several_rows_fills_each_of_them_to_its_edge() {
    // What tells a run of text from a rectangle. The middle row is selected
    // edge to edge whatever columns the two ends are in, because that is what
    // the text between them is.
    let font = font();
    let metrics = font.metrics();
    let scene = painted_selected(
        &snapshot(&["abcd", "efgh", "ijkl"], 4),
        region((0, 2), (2, 1)),
    );

    let highlight = selection_rects(&scene);
    assert_eq!(highlight.len(), 3, "one run per row");
    assert_eq!(highlight[0].0, vec2f(2. * metrics.width, 0.));
    assert_eq!(highlight[0].1.x(), 2. * metrics.width, "from the press on");
    assert_eq!(highlight[1].0, vec2f(0., metrics.height));
    assert_eq!(highlight[1].1.x(), 4. * metrics.width, "the whole row");
    assert_eq!(highlight[2].0, vec2f(0., 2. * metrics.height));
    assert_eq!(highlight[2].1.x(), 2. * metrics.width, "up to the release");
}

#[test]
fn a_screen_with_nothing_selected_pays_nothing_for_the_highlight() {
    let scene = painted(&snapshot(&["hello world"], 11));
    assert!(selection_rects(&scene).is_empty());
}

#[test]
fn a_selection_made_in_the_scrollback_is_drawn_against_the_lines_it_holds() {
    // A grid selection is numbered from the oldest line of the scrollback, so
    // the same region is the top row of a viewport scrolled two lines back and
    // nowhere at all once it has scrolled to the live output. That numbering is
    // the whole of "a selection made four screens back must not highlight the
    // live output".
    let mut scrolled = snapshot(&["abcd", "efgh"], 4);
    scrolled.history_len = 2;
    let region = region((0, 0), (0, 3));

    scrolled.display_offset = 2;
    assert_eq!(
        selection_rects(&painted_selected(&scrolled, region)).len(),
        1,
        "the selected line is the top row of this viewport"
    );

    scrolled.display_offset = 0;
    assert!(
        selection_rects(&painted_selected(&scrolled, region)).is_empty(),
        "scrolling back to the live output left the highlight behind on screen"
    );
}

/// Paints a snapshot at its natural size with the find's matches washed over
/// it.
fn painted_found(
    snapshot: &Snapshot,
    finds: Vec<(BlockId, Region)>,
    current: Option<(BlockId, Region)>,
) -> Scene {
    let font = font();
    let metrics = font.metrics();
    let size = vec2f(
        snapshot.columns as f32 * metrics.width,
        snapshot.rows as f32 * metrics.height,
    );
    let mut scene = Scene::new(1.);
    scene.start_layer(ClipBounds::None);
    paint_grid(
        snapshot,
        &font,
        Vector2F::zero(),
        size,
        true,
        &Washes {
            finds,
            find_current: current,
            selected: None,
        },
        &mut scene,
    );
    scene.stop_layer();
    scene
}

/// The rectangles washed in `colour`, as (origin, size) pairs.
fn washes_in(scene: &Scene, colour: Color) -> Vec<(Vector2F, Vector2F)> {
    rects(scene)
        .into_iter()
        .filter(|rect| rect.background == Fill::Solid(colour))
        .map(|rect| (rect.bounds.origin(), rect.bounds.size()))
        .collect()
}

#[test]
fn a_find_on_the_grid_washes_its_matches_and_the_current_one_brighter() {
    // The list's two washes on the grid: every match in the amber, the one the
    // bar is standing on in the accent over it, both under the text — and a
    // match that ends on a double-width character takes both its columns.
    let font = font();
    let metrics = font.metrics();
    let mut grid = snapshot(&["ab \u{6f22} ab"], 7);
    grid.cells[3].flags = CellFlags::WIDE;
    grid.cells[4].c = ' ';
    grid.cells[4].flags = CellFlags::WIDE_SPACER;
    let first = region((0, 0), (0, 1));
    let wide = region((0, 1), (0, 3));
    let last = region((0, 5), (0, 6));

    let scene = painted_found(&grid, vec![first, wide, last], Some(last));

    let amber = washes_in(
        &scene,
        crate::theme::theme()
            .usage_high
            .with_alpha(super::FIND_MATCH_ALPHA),
    );
    assert_eq!(
        vec![
            (vec2f(0., 0.), vec2f(2. * metrics.width, metrics.height)),
            (
                vec2f(metrics.width, 0.),
                vec2f(4. * metrics.width, metrics.height)
            ),
            (
                vec2f(5. * metrics.width, 0.),
                vec2f(2. * metrics.width, metrics.height)
            ),
        ],
        amber,
        "one wash a match, the wide character's two columns both inside it"
    );
    let accent = washes_in(
        &scene,
        crate::theme::theme()
            .accent
            .with_alpha(super::FIND_CURRENT_ALPHA),
    );
    assert_eq!(
        vec![(
            vec2f(5. * metrics.width, 0.),
            vec2f(2. * metrics.width, metrics.height)
        )],
        accent,
        "the current match, over its own amber"
    );
    assert!(
        glyphs(&scene).iter().all(|glyph| glyph.color == color(INK)),
        "a highlight repainted the text"
    );
}

#[test]
fn a_find_match_in_the_scrollback_is_drawn_against_the_line_it_is_on() {
    // A grid's matches are numbered from the oldest line of the scrollback,
    // as its selection is: the same match is the top row of a viewport
    // scrolled two lines back and nowhere once it is back at the live output.
    let mut scrolled = snapshot(&["abcd", "efgh"], 4);
    scrolled.history_len = 2;
    let matched = region((0, 1), (0, 2));
    let amber = crate::theme::theme()
        .usage_high
        .with_alpha(super::FIND_MATCH_ALPHA);

    scrolled.display_offset = 2;
    assert_eq!(
        1,
        washes_in(&painted_found(&scrolled, vec![matched], None), amber).len()
    );
    scrolled.display_offset = 0;
    assert!(washes_in(&painted_found(&scrolled, vec![matched], None), amber).is_empty());
}

/// Paints `lines` and then `text` composed over them from `column` of the
/// first row, with the input method's caret where `cursor` says, the way a
/// grid with the keyboard does. Returns the scene and the cell the input
/// method's caret was answered at.
fn painted_composing(
    lines: &[&str],
    columns: usize,
    column: usize,
    composing: Option<(&str, Option<(usize, usize)>)>,
) -> (Scene, RectF) {
    let grid = snapshot(lines, columns);
    let mut scene = paint(&grid, None, true);
    let composition = PaneComposition::new();
    if let Some((text, cursor)) = composing {
        composition.set(text, cursor);
    }
    scene.start_layer(ClipBounds::None);
    let caret = paint_composition(
        composition.preedit().as_ref(),
        CursorRow {
            origin: Vector2F::zero(),
            columns,
            column,
        },
        (INK, GROUND),
        &font(),
        &mut scene,
    );
    scene.stop_layer();
    (scene, caret)
}

#[test]
fn a_composition_is_drawn_from_the_cursor_over_what_the_program_drew_and_underlined() {
    let metrics = font().metrics();
    let (scene, caret) = painted_composing(&["abcdef"], 8, 2, Some(("日x", Some((3, 3)))));

    // In a layer over the grid's, which is the only way a ground can cover the
    // program's glyphs: a layer draws all of its glyphs over all of its rects.
    let layer = scene
        .layers()
        .last()
        .expect("the composition has a layer of its own");
    assert_eq!(
        layer.clip_bounds,
        Some(RectF::new(
            Vector2F::zero(),
            vec2f(metrics.width * 8., metrics.height)
        )),
        "and it is clipped to the cursor's row"
    );
    let drawn: Vec<_> = layer
        .glyphs
        .iter()
        .map(|glyph| (glyph.glyph_key.glyph_id, glyph.position))
        .collect();
    assert_eq!(
        drawn,
        vec![
            (u32::from('日'), vec2f(metrics.width * 2., metrics.baseline)),
            // Two cells on, because the one before it is double width.
            (u32::from('x'), vec2f(metrics.width * 4., metrics.baseline)),
        ]
    );

    let ground = &layer.rects[0];
    assert_eq!(ground.background, Fill::Solid(color(GROUND)));
    assert_eq!(
        ground.bounds,
        RectF::new(
            vec2f(metrics.width * 2., 0.),
            vec2f(metrics.width * 3., metrics.height)
        ),
        "the ground covers the three cells the composition takes"
    );
    let underline = &layer.rects[1];
    assert_eq!(underline.background, Fill::Solid(color(INK)));
    assert!(
        underline.bounds.min_y() > metrics.baseline,
        "the rule is under the text"
    );
    assert_eq!(underline.bounds.min_x(), metrics.width * 2.);
    assert_eq!(underline.bounds.width(), metrics.width * 3.);
    let beam = &layer.rects[2];
    assert_eq!(
        beam.bounds,
        RectF::new(
            vec2f(metrics.width * 4., 0.),
            vec2f(CURSOR_STROKE, metrics.height)
        ),
        "the input method's caret stands in front of the `x`"
    );

    assert_eq!(
        caret,
        RectF::new(
            vec2f(metrics.width * 4., 0.),
            vec2f(metrics.width, metrics.height)
        ),
        "and that is the cell the input method is told about"
    );
}

#[test]
fn with_nothing_composed_the_cursor_cell_is_answered_and_nothing_is_drawn() {
    let metrics = font().metrics();
    let (scene, caret) = painted_composing(&["abcdef"], 8, 3, None);
    let (abandoned, _) = painted_composing(&["abcdef"], 8, 3, Some(("", None)));

    assert_eq!(
        scene.layer_count(),
        abandoned.layer_count(),
        "an empty composition is none"
    );
    assert_eq!(
        glyphs(&scene).len(),
        6,
        "the program's line, and nothing else"
    );
    assert_eq!(
        caret,
        RectF::new(
            vec2f(metrics.width * 3., 0.),
            vec2f(metrics.width, metrics.height)
        )
    );
}

#[test]
fn a_composition_at_the_end_of_a_row_is_drawn_back_from_it_rather_than_cut_off() {
    let metrics = font().metrics();
    let (scene, caret) = painted_composing(&["abcdef"], 6, 5, Some(("日本", None)));
    let layer = scene.layers().last().expect("a layer of its own");

    let columns: Vec<_> = layer
        .glyphs
        .iter()
        .map(|glyph| glyph.position.x() / metrics.width)
        .collect();
    assert_eq!(columns, vec![2., 4.], "the whole word, ending at the edge");
    assert_eq!(
        layer.rects.len(),
        2,
        "a ground and a rule, and no caret: the input method asked for none"
    );
    assert_eq!(
        caret.min_x(),
        metrics.width * 5.,
        "a caret past the edge is answered at the last cell"
    );
}

#[test]
fn a_column_is_split_down_its_middle_so_a_selection_ends_between_characters() {
    // Dragging right from the left half of a character takes it; from the
    // right half it does not. Nobody notices this until it is missing.
    assert_eq!(column_at(0., 10., 8), (0, CellSide::Left));
    assert_eq!(column_at(4.9, 10., 8), (0, CellSide::Left));
    assert_eq!(column_at(5., 10., 8), (0, CellSide::Right));
    assert_eq!(column_at(25., 10., 8), (2, CellSide::Right));

    // Outside the row is its first cell on the left and its last on the right,
    // which is what a drag that has left the pane means.
    assert_eq!(column_at(-40., 10., 8), (0, CellSide::Left));
    assert_eq!(column_at(800., 10., 8), (7, CellSide::Right));
}

#[test]
fn a_row_of_no_columns_answers_zero_rather_than_underflowing() {
    // No caller asks — the hit tests refuse an empty grid first — but the last
    // column is `columns - 1`, and on nothing that would wrap round. It
    // saturates to zero instead, whichever side of the (absent) row is asked.
    assert_eq!(column_at(0., 10., 0), (0, CellSide::Right));
    assert_eq!(column_at(800., 10., 0), (0, CellSide::Right));
    assert_eq!(column_at(-40., 10., 0), (0, CellSide::Left));
}

/// A URL the terminal folded onto the next row of the grid is one link from a
/// pointer on either row, and underlines both. The fold here is the flag set
/// by hand, because the scan reads the flag and nothing else; what the
/// emulator sets it for is pinned in `crook_terminal::rows`.
#[test]
fn a_folded_url_on_the_grid_is_one_link_from_either_row() {
    use crate::pane_link::{LinkCells, LinkRow, LinkTarget};
    use crookui_core::event::Modifiers;
    use crookui_core::geometry::{Point, ZIndex};

    let url = "https://example.com/abcdefghijklmnop";
    let mut grid = snapshot(&[&url[..20], &url[20..], "https://b.test"], 20);
    grid.cells[19].flags = CellFlags::WRAPLINE;

    let font = font();
    let metrics = font.metrics();
    let mut element = TerminalElement::new(Arc::new(grid), font);
    element.size = Some(vec2f(20. * metrics.width, 3. * metrics.height));
    element.origin = Some(Point::from_vec2f(Vector2F::zero(), ZIndex::Normal(0)));

    // The modifier that follows a link is the platform's — Command on macOS,
    // Control off it — the same split `opens_links` makes, and the test runs
    // on both.
    let follow = if cfg!(target_os = "macos") {
        Modifiers {
            cmd: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    };
    let at = |row: usize, column: usize| {
        vec2f(
            (column as f32 + 0.5) * metrics.width,
            (row as f32 + 0.5) * metrics.height,
        )
    };

    let whole = vec![
        LinkCells {
            row: LinkRow::Viewport(0),
            start: 0,
            len: 20,
        },
        LinkCells {
            row: LinkRow::Viewport(1),
            start: 0,
            len: 16,
        },
    ];
    for (row, column) in [(0, 5), (1, 3)] {
        let link = element
            .link_at(at(row, column), follow)
            .unwrap_or_else(|| panic!("row {row} has no link under the pointer"));
        assert_eq!(
            link.target,
            LinkTarget::Url(url.to_owned()),
            "row {row} found a cut-off URL"
        );
        assert_eq!(
            link.cells, whole,
            "row {row} did not map the link onto both rows"
        );
    }

    // The row below was printed with a newline before it: a link of its own.
    let below = element
        .link_at(at(2, 3), follow)
        .expect("the hard-wrapped row's link");
    assert_eq!(below.target, LinkTarget::Url("https://b.test".to_owned()));
    assert_eq!(
        below.cells,
        vec![LinkCells {
            row: LinkRow::Viewport(2),
            start: 0,
            len: 14,
        }]
    );

    assert!(
        element.link_at(at(0, 5), Modifiers::default()).is_none(),
        "without the chord key the pointer is selecting, not following"
    );
}

/// A path on the grid is read against where the pane's shell last said it
/// was, which is where the full-screen program or the command still running
/// on it was started — and is no link at all when the shell never said.
#[test]
fn a_path_on_the_grid_is_read_against_where_the_shell_said_it_was() {
    use crate::pane_link::{FileLink, LinkTarget, PaneLink};
    use crookui_core::event::Modifiers;
    use crookui_core::geometry::{Point, ZIndex};
    use std::path::{Path, PathBuf};

    let grid = Arc::new(snapshot(&["error: src/main.rs:3"], 20));
    let font = font();
    let metrics = font.metrics();
    let follow = if cfg!(target_os = "macos") {
        Modifiers {
            cmd: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    };
    let on_the_path = vec2f(10.5 * metrics.width, 0.5 * metrics.height);
    let element = |directory: Option<PathBuf>| {
        let mut element =
            TerminalElement::new(grid.clone(), font.clone()).with_links(PaneLink::new(), directory);
        element.size = Some(vec2f(20. * metrics.width, metrics.height));
        element.origin = Some(Point::from_vec2f(Vector2F::zero(), ZIndex::Normal(0)));
        element
    };

    let link = element(Some(PathBuf::from("/project")))
        .link_at(on_the_path, follow)
        .expect("the path is a link");
    assert_eq!(
        link.target,
        LinkTarget::File(FileLink {
            path: Path::new("/project").join("src/main.rs"),
            line: Some(3),
            column: None,
        })
    );
    assert_eq!(
        element(None).link_at(on_the_path, follow),
        None,
        "a relative path was read against nothing"
    );
}

/// A session the pane draws as one grid, with a picture in it: no shell
/// integration, as over `ssh`, and more than a screenful of output before
/// `imgcat` printed, so the one open block starts above the top of the screen.
///
/// Six rows of twenty columns, with cells of 10×20 pixels as far as the
/// emulator is told, so the picture's `width=4;height=2` is four columns and
/// two rows. `ABOVE` is the line printed just before it.
fn grid_with_a_picture(args: &str) -> Emulator {
    grid_with_a_picture_after(10, 100, args)
}

/// [`grid_with_a_picture`], after `lines` of output into a history that
/// keeps `scrollback` of them.
fn grid_with_a_picture_after(lines: usize, scrollback: usize, args: &str) -> Emulator {
    use base64::Engine as _;

    let mut emulator = Emulator::new(
        TerminalSize::new(20, 6).with_cell_size(10, 20),
        scrollback,
        Palette::default(),
    );
    for line in 0..lines {
        emulator.advance(format!("log {line}\r\n").as_bytes());
    }
    emulator.advance(b"ABOVE\r\n");
    let png = crate::picture::tests::png_of(8, 8, png::ColorType::Rgba, &[200, 30, 30, 255]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    emulator.advance(format!("\x1b]1337;File={args}inline=1:{encoded}\x07").as_bytes());
    emulator.advance(b"\r\nBELOW");
    emulator
}

/// The viewport row whose text starts with `prefix`.
fn row_saying(snapshot: &Snapshot, prefix: &str) -> usize {
    snapshot
        .text()
        .lines()
        .position(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no row says {prefix:?}:\n{}", snapshot.text()))
}

/// Every picture a frame draws, with the clip of the layer it is drawn in.
fn pictures(scene: &Scene) -> Vec<(RectF, Option<RectF>)> {
    scene
        .layers()
        .flat_map(|layer| {
            layer
                .images
                .iter()
                .map(|image| (image.bounds, layer.clip_bounds))
        })
        .collect()
}

#[test]
fn a_picture_printed_into_the_grid_is_drawn_over_the_rows_it_took() {
    // `imgcat loss.png` after a screenful of training logs: the emulator kept
    // the rows for it, and a grid that drew nothing over them left a blank
    // hole where the plot should be.
    let mut emulator = grid_with_a_picture("width=4;height=2;");
    let snapshot = emulator.snapshot();
    assert_eq!(
        pane_surface::of(&snapshot, std::time::Instant::now()).surface,
        pane_surface::Surface::Grid,
        "the open block has run past the top of the screen, so the pane is a grid"
    );

    let metrics = font().metrics();
    let row = row_saying(&snapshot, "ABOVE") + 1;
    let drawn = pictures(&painted(&snapshot));
    assert_eq!(drawn.len(), 1, "the picture is drawn, once: {drawn:?}");
    let (bounds, clip) = drawn[0];
    assert_eq!(
        bounds.origin(),
        vec2f(0., row as f32 * metrics.height),
        "on the row under the line printed before it, at its first column"
    );
    // A square, fitted inside four columns by two rows with its shape kept.
    let side = (4. * metrics.width).min(2. * metrics.height);
    assert_eq!(bounds.size(), vec2f(side, side));
    assert_eq!(
        clip,
        Some(RectF::new(
            Vector2F::zero(),
            vec2f(20. * metrics.width, 6. * metrics.height)
        )),
        "cut off at the grid's rows, so a picture half off the screen does not hang over the pane"
    );
}

#[test]
fn a_picture_asked_to_be_stretched_fills_its_cells_on_the_grid() {
    let mut emulator = grid_with_a_picture("width=4;height=2;preserveAspectRatio=0;");
    let snapshot = emulator.snapshot();

    let metrics = font().metrics();
    let drawn = pictures(&painted(&snapshot));
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    assert_eq!(
        drawn[0].0.size(),
        vec2f(4. * metrics.width, 2. * metrics.height)
    );
}

#[test]
fn a_picture_on_the_grid_moves_with_its_rows_when_the_scrollback_is_scrolled() {
    let mut emulator = grid_with_a_picture("width=4;height=2;");
    let metrics = font().metrics();
    let live = row_saying(&emulator.snapshot(), "ABOVE") + 1;

    // Two lines back into the history: everything moves down two rows, and
    // the picture's second row is now under the bottom of the screen.
    emulator.scroll_lines(2);
    let snapshot = emulator.snapshot();
    let row = row_saying(&snapshot, "ABOVE") + 1;
    assert_eq!(row, live + 2, "the viewport moved");
    let drawn = pictures(&painted(&snapshot));
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    assert_eq!(
        drawn[0].0.origin().y(),
        row as f32 * metrics.height,
        "the picture stayed under the line printed before it"
    );

    // Far enough back that it is below the screen altogether.
    emulator.scroll_lines(10);
    let snapshot = emulator.snapshot();
    assert!(!snapshot.text().contains("ABOVE"), "{}", snapshot.text());
    assert!(
        pictures(&painted(&snapshot)).is_empty(),
        "a picture scrolled out of the grid is not drawn"
    );
}

#[test]
fn a_full_screen_program_does_not_show_the_pictures_of_the_screen_under_it() {
    // The alternate screen is the program's, cell for cell, and the picture
    // printed on the primary screen before it started is not on it.
    let mut emulator = grid_with_a_picture("width=4;height=2;");
    emulator.advance(b"\x1b[?1049h");
    let snapshot = emulator.snapshot();
    assert!(snapshot.alt_screen);
    assert!(pictures(&painted(&snapshot)).is_empty());

    emulator.advance(b"\x1b[?1049l");
    assert_eq!(
        pictures(&painted(&emulator.snapshot())).len(),
        1,
        "and it is back when the program leaves"
    );
}

#[test]
fn a_picture_past_a_full_scrollback_is_not_drawn_over_the_rows_after_it() {
    // A session longer than its scrollback — ten thousand lines in a pane,
    // ten here — has a history that no longer grows, and nothing then says
    // which rows a picture printed into it is over: told where the cursor
    // was, the grid would draw it over `BELOW` and, at the bottom of the
    // screen, over the prompt and what is typed at it. It leaves the blank
    // rows it took instead.
    let mut emulator = grid_with_a_picture_after(30, 10, "width=4;height=2;");
    let snapshot = emulator.snapshot();
    assert_eq!(
        pane_surface::of(&snapshot, std::time::Instant::now()).surface,
        pane_surface::Surface::Grid
    );
    let below = row_saying(&snapshot, "BELOW");
    let drawn = pictures(&painted(&snapshot));
    let metrics = font().metrics();
    assert!(
        drawn
            .iter()
            .all(|(bounds, _)| bounds.max_y() <= below as f32 * metrics.height),
        "nothing over BELOW, on row {below}: {drawn:?}"
    );
    assert!(drawn.is_empty(), "{drawn:?}");
}
