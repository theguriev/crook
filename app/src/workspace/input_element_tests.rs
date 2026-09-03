//! What a field actually puts in the scene, and where a click in it lands.
//!
//! Painted against [`CellFont::headless`], where a character is its own glyph
//! id and a cell is half the font size — so an assertion here is about the
//! field rather than about which fonts the machine running it happens to have.

use crookui_core::scene::{ClipBounds, Fill, Glyph, Rect};

use super::*;
use crate::editor::Selection;
use crate::terminal_font::CELL_FONT_SIZE;

fn font() -> CellFont {
    CellFont::headless(CELL_FONT_SIZE)
}

/// An input holding `text`, with the caret left at the end of it.
fn holding(text: &str) -> PaneInput {
    let input = PaneInput::new();
    input.edit(|editor| editor.set_text(text));
    input
}

/// How wide a field has to be to hold `columns` cells of text.
fn width_for(columns: usize) -> f32 {
    (columns + PROMPT_COLUMNS) as f32 * font().metrics().width
}

/// The rows a field `columns` cells wide draws `text` in.
fn rows_of(text: &str, caret: usize, columns: usize) -> Rows {
    Rows::of(text, caret, width_for(columns), font().metrics(), MAX_ROWS)
}

/// Every row of `text` in a field `columns` cells wide, drawn or not.
fn wrap(text: &str, columns: usize) -> Vec<Range<usize>> {
    RowWalk::new(text, columns).collect()
}

/// Paints a field `columns` cells wide, tall enough for everything in it.
fn painted(input: &PaneInput, columns: usize, focused: bool) -> Scene {
    let editor = input.editor();
    let rows = rows_of(editor.text(), editor.caret(), columns);
    drop(editor);

    let mut scene = Scene::new(1.);
    scene.start_layer(ClipBounds::None);
    paint_input(input, &font(), Vector2F::zero(), &rows, focused, &mut scene);
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

/// Where a point inside a field `columns` wide lands in the text.
fn offset_at(input: &PaneInput, columns: usize, local: Vector2F) -> usize {
    let editor = input.editor();
    let rows = rows_of(editor.text(), editor.caret(), columns);
    rows.at_point(editor.text(), local, font().metrics())
}

#[test]
fn the_prompt_is_drawn_first_and_the_text_starts_after_it() {
    // The affordance that makes the box read as a command line rather than as
    // somewhere to write a paragraph.
    let metrics = font().metrics();
    let painted = glyphs(&painted(&holding("hi"), 20, true));

    assert_eq!(painted[0].glyph_key.glyph_id, u32::from(PROMPT));
    assert_eq!(painted[0].position, vec2f(0., metrics.baseline));
    assert_eq!(painted[0].color, THEME.accent);

    assert_eq!(painted[1].glyph_key.glyph_id, u32::from('h'));
    assert_eq!(
        painted[1].position,
        vec2f(PROMPT_COLUMNS as f32 * metrics.width, metrics.baseline)
    );
    assert_eq!(painted[2].glyph_key.glyph_id, u32::from('i'));
    assert_eq!(
        painted[2].position,
        vec2f(
            (PROMPT_COLUMNS + 1) as f32 * metrics.width,
            metrics.baseline
        )
    );
}

#[test]
fn an_unfocused_field_dims_its_prompt_and_draws_no_caret() {
    // Two panes both drawing a caret would say nothing about which of them is
    // going to receive what is typed.
    let scene = painted(&holding("hi"), 20, false);

    assert_eq!(glyphs(&scene)[0].color, THEME.text_muted);
    assert!(
        rects(&scene).is_empty(),
        "an unfocused field paints no caret and no selection it is not making"
    );
}

#[test]
fn the_caret_is_one_cell_at_the_column_it_is_on() {
    let metrics = font().metrics();
    let input = holding("hi");
    let caret = rects(&painted(&input, 20, true))
        .pop()
        .expect("a focused field draws a caret");

    assert_eq!(
        caret.bounds.origin(),
        vec2f((PROMPT_COLUMNS + 2) as f32 * metrics.width, 0.),
        "the caret sits one cell past the last character, where the next one goes"
    );
    assert_eq!(caret.bounds.size(), vec2f(metrics.width, metrics.height));
    assert_eq!(caret.background, Fill::Solid(THEME.accent));
}

#[test]
fn a_character_under_the_caret_is_drawn_in_the_ground_it_sits_on() {
    // A filled caret is a rectangle, and rectangles paint under glyphs: a
    // character left in its own colour would vanish into the fill.
    let input = holding("hi");
    input.edit(|editor| editor.set_caret(0));

    let painted = glyphs(&painted(&input, 20, true));
    assert_eq!(painted[1].color, THEME.ground, "the character on the caret");
    assert_eq!(painted[2].color, THEME.text_primary, "and only that one");
}

#[test]
fn a_selection_is_one_rectangle_over_the_cells_it_covers() {
    let metrics = font().metrics();
    let input = holding("echo hello");
    input.edit(|editor| editor.set_selection(Selection::new(5, 10)));

    let painted = rects(&painted(&input, 20, true));
    let selection = &painted[0];
    assert_eq!(
        selection.bounds.origin(),
        vec2f((PROMPT_COLUMNS + 5) as f32 * metrics.width, 0.)
    );
    assert_eq!(
        selection.bounds.size(),
        vec2f(5. * metrics.width, metrics.height),
        "five cells, one quad"
    );
    assert_eq!(
        selection.background,
        Fill::Solid(THEME.accent.with_alpha(SELECTION_ALPHA))
    );
    assert!(
        painted.len() > 1,
        "the caret is still drawn, at the head of the selection"
    );
}

#[test]
fn a_selection_across_a_line_break_is_a_rectangle_on_each_line() {
    let metrics = font().metrics();
    let input = holding("one\ntwo");
    input.edit(|editor| editor.select_all());

    let painted = rects(&painted(&input, 20, true));
    assert_eq!(painted[0].bounds.origin().y(), 0.);
    assert_eq!(painted[0].bounds.width(), 3. * metrics.width);
    assert_eq!(painted[1].bounds.origin().y(), metrics.height);
    assert_eq!(painted[1].bounds.width(), 3. * metrics.width);
}

#[test]
fn a_line_longer_than_the_field_wraps_onto_the_next_row() {
    // Where the field's height comes from: the editor has one line and the
    // field draws two, so the box grows and the grid above gives up the space.
    assert_eq!(wrap("abcd", 2), vec![0..2, 2..4, 4..4]);
    assert_eq!(wrap("abcde", 2), vec![0..2, 2..4, 4..5]);
    assert_eq!(wrap("", 2), vec![0..0]);
    assert_eq!(
        wrap("a\nb", 4),
        vec![0..1, 2..3],
        "a newline is not drawn in a cell of its own"
    );
}

#[test]
fn a_row_that_is_exactly_full_is_followed_by_an_empty_one() {
    // Where the caret goes when a row has been filled. Without the empty row it
    // would be drawn one cell outside the field.
    let input = holding("ab");
    let rows = rows_of("ab", 2, 2);

    assert_eq!(rows.drawn(), 2);
    assert_eq!(rows.place("ab", 2), Some((1, 0)));
    assert_eq!(input.editor().caret(), 2);
}

#[test]
fn only_the_last_line_opens_an_empty_row_when_it_fills_one() {
    // A line that ends in a newline already has a row after it — the next line
    // — and an empty one between them would be a blank line nobody typed, in a
    // box a row taller than its contents.
    assert_eq!(wrap("abc\ndef", 3), vec![0..3, 4..7, 7..7]);
    assert_eq!(
        rows_of("abc\ndef", 3, 3).drawn(),
        3,
        "two lines and the caret's row after them"
    );
}

#[test]
fn a_double_width_character_takes_the_two_cells_the_grid_gives_it() {
    // The field and the grid above it lay the same characters out, so they
    // have to count them the same way: `cd 日本語` is nine columns in both, or
    // the echo lands three columns left of what was typed.
    assert_eq!(cells("ab"), 2);
    assert_eq!(
        cells("日"),
        2,
        "East Asian width, the way the emulator reads it"
    );
    assert_eq!(
        cells("e\u{301}"),
        1,
        "a combining mark has no cell of its own"
    );

    // Three ideographs are six cells, so a six-cell field holds them exactly
    // and a five-cell one breaks before the last rather than through it.
    assert_eq!(wrap("日本語", 6), vec![0..9, 9..9]);
    assert_eq!(wrap("日本語", 5), vec![0..6, 6..9]);

    let rows = rows_of("日本", 6, 20);
    assert_eq!(
        rows.place("日本", 6),
        Some((0, 4)),
        "the caret sits past four cells of text, not two"
    );
}

#[test]
fn a_click_past_a_wide_character_lands_after_it() {
    let metrics = font().metrics();
    let input = holding("日x");
    let left = PROMPT_COLUMNS as f32 * metrics.width;

    assert_eq!(
        offset_at(&input, 20, vec2f(left + metrics.width * 0.4, 1.)),
        0
    );
    assert_eq!(
        offset_at(&input, 20, vec2f(left + metrics.width * 1.6, 1.)),
        3,
        "the second cell of a wide character is still that character"
    );
    assert_eq!(
        offset_at(&input, 20, vec2f(left + metrics.width * 2.6, 1.)),
        4,
        "and the cell after the pair is the character after it"
    );
}

#[test]
fn only_the_rows_that_are_drawn_are_ever_built() {
    // What keeps a pasted megabyte from costing a walk of itself per frame:
    // the rows before the window are counted, not kept.
    let text = "x".repeat(100_000);
    let rows = rows_of(&text, text.len(), 20);

    assert_eq!(rows.drawn(), MAX_ROWS);
    assert_eq!(
        rows.place(&text, text.len()),
        Some((MAX_ROWS - 1, 0)),
        "and the caret is on the last of them"
    );
    assert_eq!(rows.place(&text, 0), None, "the first row is far above");
}

#[test]
fn the_field_takes_at_most_half_a_short_pane() {
    // The field grows downwards *into* the grid. In a pane four rows tall, a
    // field that took its eight would leave the shell it is composing for
    // nothing to be seen in — and, before the flex was taught to bound it,
    // would paint the other four over the panel's border and the pane below.
    let metrics = font().metrics();
    assert_eq!(row_budget(f32::INFINITY, metrics), MAX_ROWS);
    assert_eq!(row_budget(metrics.height * 40., metrics), MAX_ROWS);
    assert_eq!(row_budget(metrics.height * 4., metrics), 2);
    assert_eq!(row_budget(metrics.height * 0.5, metrics), 1);

    let text = "a\n".repeat(20);
    let rows = Rows::of(&text, text.len(), width_for(20), metrics, 2);
    assert_eq!(rows.drawn(), 2);
}

#[test]
fn a_field_taller_than_its_limit_scrolls_to_keep_the_caret_in_view() {
    let text = "a\n".repeat(MAX_ROWS + 4);
    let rows = rows_of(&text, text.len(), 20);

    assert_eq!(rows.drawn(), MAX_ROWS, "the field stops growing");
    assert_eq!(
        rows.place(&text, text.len()).map(|(row, _)| row),
        Some(MAX_ROWS - 1),
        "and the caret is on the last row of what is drawn"
    );

    // A caret at the top scrolls the other way, and the rows above it are the
    // ones that are not drawn.
    let rows = rows_of(&text, 0, 20);
    assert_eq!(rows.place(&text, 0), Some((0, 0)));
    assert_eq!(rows.place(&text, text.len()), None);
}

#[test]
fn a_click_lands_on_the_boundary_nearest_where_it_was_aimed() {
    // The whole of "the caret goes where a person aimed": the boundary is
    // halfway across a cell, not at its left edge.
    let metrics = font().metrics();
    let input = holding("abc");
    let left = PROMPT_COLUMNS as f32 * metrics.width;

    assert_eq!(
        offset_at(&input, 20, vec2f(left + metrics.width * 0.49, 1.)),
        0
    );
    assert_eq!(
        offset_at(&input, 20, vec2f(left + metrics.width * 0.51, 1.)),
        1
    );
    assert_eq!(
        offset_at(&input, 20, vec2f(left + metrics.width * 1.49, 1.)),
        1
    );
    assert_eq!(
        offset_at(&input, 20, vec2f(left + metrics.width * 1.51, 1.)),
        2
    );
}

#[test]
fn a_click_outside_the_text_lands_at_the_nearest_end_of_the_row_it_is_on() {
    let metrics = font().metrics();
    let input = holding("ab\ncdef");

    assert_eq!(
        offset_at(&input, 20, vec2f(0., 1.)),
        0,
        "left of the prompt"
    );
    assert_eq!(
        offset_at(&input, 20, vec2f(900., 1.)),
        2,
        "past the end of the first line, not into the second"
    );
    assert_eq!(
        offset_at(&input, 20, vec2f(900., metrics.height * 1.5)),
        7,
        "and past the end of the last one"
    );
    assert_eq!(
        offset_at(&input, 20, vec2f(0., metrics.height * 40.)),
        3,
        "below every row is the row at the bottom"
    );
}

#[test]
fn a_click_on_a_wrapped_row_lands_on_that_row_rather_than_on_the_line() {
    // The one case a logical-line hit test gets wrong: the second half of a
    // wrapped line is a row of its own, and a click past its end must not run
    // to the end of the whole line.
    let metrics = font().metrics();
    let input = holding("abcdef");

    assert_eq!(offset_at(&input, 3, vec2f(900., 1.)), 3);
    assert_eq!(offset_at(&input, 3, vec2f(900., metrics.height * 1.5)), 6);
}

#[test]
fn a_field_narrower_than_its_own_prompt_still_holds_a_cell() {
    // A pane can be dragged narrower than anything, and a field that reported
    // no columns at all would divide by zero on the next wrap.
    let metrics = font().metrics();
    assert_eq!(columns_for(0., metrics), 1);
    assert_eq!(columns_for(metrics.width * 2., metrics), 1);
    assert_eq!(columns_for(metrics.width * 12., metrics), 10);
}

#[test]
fn a_combining_mark_is_drawn_over_the_character_it_belongs_to() {
    // macOS hands out decomposed text, so a pasted `José` arrives as `Jose`
    // plus an accent that has no cell of its own.
    let painted = glyphs(&painted(&holding("e\u{301}!"), 20, true));

    assert_eq!(painted[1].glyph_key.glyph_id, u32::from('e'));
    assert_eq!(painted[2].glyph_key.glyph_id, u32::from('\u{301}'));
    assert_eq!(
        painted[1].position, painted[2].position,
        "a mark carries its own offset and shares the character's pen"
    );
    assert_eq!(
        painted[3].position.x() - painted[1].position.x(),
        font().metrics().width,
        "and the cluster took one cell, not two"
    );
}
