//! What the composer actually puts in the scene, and where a click in it
//! lands.
//!
//! Painted against [`CellFont::headless`], where a character is its own glyph
//! id and a cell is half the font size — so an assertion here is about the
//! composer rather than about which fonts the machine running it happens to
//! have.

use crook_terminal::{Emulator, Palette, TerminalSize};
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

/// How wide a composer has to be to hold `columns` cells of text.
///
/// Exactly the columns, because there is no prompt glyph taking any of them:
/// column zero here is column zero in the output above.
fn width_for(columns: usize) -> f32 {
    columns as f32 * font().metrics().width
}

/// The rows a field `columns` cells wide draws `text` in, on a row of its own.
fn rows_of(text: &str, caret: usize, columns: usize) -> Rows {
    rows_inline(text, caret, columns, None)
}

/// The same, with the first row continuing a prompt that ended at `inline`.
fn rows_inline(text: &str, caret: usize, columns: usize, inline: Option<usize>) -> Rows {
    Rows::of(
        text,
        caret,
        width_for(columns),
        font().metrics(),
        MAX_ROWS,
        inline,
    )
}

/// How many rows a composer offered `available` pixels in a pane of
/// `pane_rows` rows may draw.
fn budget(available: f32, pane_rows: usize) -> usize {
    row_budget(available, font().metrics(), pane_rows)
}

/// Every row of `text` in a field `columns` cells wide, drawn or not.
fn wrap(text: &str, columns: usize) -> Vec<Range<usize>> {
    RowWalk::new(text, columns, columns).collect()
}

/// Paints a field `columns` cells wide, tall enough for everything in it.
fn painted(input: &PaneInput, columns: usize, focused: bool) -> Scene {
    painted_in(input, columns, focused, Ink::default())
}

/// The same, with the first row continuing a prompt that ended at `inline`.
fn painted_inline(input: &PaneInput, columns: usize, inline: Option<usize>) -> Scene {
    painted_rows(
        input,
        &{
            let editor = input.editor();
            rows_inline(editor.text(), editor.caret(), columns, inline)
        },
        true,
        Ink::default(),
    )
}

/// The same, in colours a shell resolved rather than the theme's.
fn painted_in(input: &PaneInput, columns: usize, focused: bool, ink: Ink) -> Scene {
    let editor = input.editor();
    let rows = rows_of(editor.text(), editor.caret(), columns);
    drop(editor);
    painted_rows(input, &rows, focused, ink)
}

/// Paints a field into a scene from rows somebody else wrapped.
fn painted_rows(input: &PaneInput, rows: &Rows, focused: bool, ink: Ink) -> Scene {
    let mut scene = Scene::new(1.);
    scene.start_layer(ClipBounds::None);
    paint_input(
        input,
        &font(),
        Vector2F::zero(),
        rows,
        focused,
        ink,
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

/// Where a point inside a field `columns` wide lands in the text.
fn offset_at(input: &PaneInput, columns: usize, local: Vector2F) -> usize {
    let editor = input.editor();
    let rows = rows_of(editor.text(), editor.caret(), columns);
    rows.at_point(editor.text(), local, font().metrics())
}

#[test]
fn there_is_no_prompt_glyph_and_the_text_starts_at_column_zero() {
    // The shell's own prompt is in the open block above. A second invented one
    // under it is two prompts on screen, which is what makes the composer read
    // as a widget rather than as the next line of the terminal.
    let metrics = font().metrics();
    let painted = glyphs(&painted(&holding("hi"), 20, true));

    assert_eq!(painted.len(), 2, "two characters, and nothing invented");
    assert_eq!(painted[0].glyph_key.glyph_id, u32::from('h'));
    assert_eq!(
        painted[0].position,
        vec2f(0., metrics.baseline),
        "column zero, which is the grid's column zero"
    );
    assert_eq!(painted[0].color, theme().terminal.foreground);
    assert_eq!(painted[1].position, vec2f(metrics.width, metrics.baseline));
}

#[test]
fn the_caret_is_the_whole_of_the_focus_affordance() {
    // No ring, no border, no tint, no change of ground: an unfocused composer
    // and a focused one differ by exactly one rectangle.
    let focused = painted(&holding("hi"), 20, true);
    let unfocused = painted(&holding("hi"), 20, false);

    assert_eq!(glyphs(&focused), glyphs(&unfocused), "the text is the text");
    assert_eq!(rects(&focused).len(), 1, "the caret, and nothing else");
    assert!(
        rects(&unfocused).is_empty(),
        "an unfocused composer paints no caret and no box"
    );
}

#[test]
fn the_caret_is_a_bar_in_the_terminal_cursor_colour() {
    // The colour the grid paints the shell's own cursor in. An accent caret is
    // a form field's, and it would disagree with the block above it.
    let metrics = font().metrics();
    let input = holding("hi");
    let caret = rects(&painted(&input, 20, true))
        .pop()
        .expect("a focused composer draws a caret");

    assert_eq!(
        caret.bounds.origin().x(),
        2. * metrics.width,
        "one cell past the last character, where the next one goes"
    );
    assert_eq!(
        caret.bounds.width(),
        CARET_WIDTH,
        "a bar, not a filled cell"
    );
    assert!(
        caret.bounds.height() < metrics.height,
        "and shorter than the cell"
    );
    assert_eq!(caret.background, Fill::Solid(theme().terminal.cursor));
}

#[test]
fn the_character_under_the_caret_keeps_its_own_colour() {
    // A bar three pixels wide stands beside the character rather than over it,
    // so there is nothing to invert — which is the other half of not using the
    // grid's block cursor here.
    let input = holding("hi");
    input.edit(|editor| editor.set_caret(0));

    let painted = glyphs(&painted(&input, 20, true));
    assert!(
        painted
            .iter()
            .all(|glyph| glyph.color == theme().terminal.foreground),
        "every character in its own ink"
    );
}

#[test]
fn a_selection_is_one_rectangle_over_the_cells_it_covers() {
    let metrics = font().metrics();
    let input = holding("echo hello");
    input.edit(|editor| editor.set_selection(Selection::new(5, 10)));

    let painted = rects(&painted(&input, 20, true));
    let selection = &painted[0];
    assert_eq!(selection.bounds.origin(), vec2f(5. * metrics.width, 0.));
    assert_eq!(
        selection.bounds.size(),
        vec2f(5. * metrics.width, metrics.height),
        "five cells, one quad"
    );
    assert_eq!(selection.background, Fill::Solid(theme().selection));
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
    let left = 0.;

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
fn the_composer_takes_at_most_half_a_short_pane() {
    // The composer grows downwards *into* the output. In a pane four rows
    // tall, one that took its eight would leave the shell it is composing for
    // nothing to be seen in — and would paint the other four over the pane
    // below.
    let metrics = font().metrics();
    assert_eq!(budget(metrics.height * 40., 0), MAX_ROWS);
    assert_eq!(budget(metrics.height * 4., 0), 2);
    assert_eq!(budget(metrics.height * 0.5, 0), 1);

    // A flex measures a non-flexible child against an unbounded height, which
    // is the parent asking how big it would like to be. Half of infinity is
    // not an answer; half the pane is.
    assert_eq!(budget(f32::INFINITY, 4), 2);
    assert_eq!(budget(f32::INFINITY, 40), MAX_ROWS);
    assert_eq!(budget(f32::INFINITY, 0), 1, "a pane with no rows yet");

    let text = "a\n".repeat(20);
    let rows = Rows::of(&text, text.len(), width_for(20), metrics, 2, None);
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
    let left = 0.;

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
    assert_eq!(columns_for(metrics.width * 1., metrics), 1);
    assert_eq!(
        columns_for(metrics.width * 12., metrics),
        12,
        "every column of the box, because no prompt glyph takes any of them"
    );
}

#[test]
fn a_combining_mark_is_drawn_over_the_character_it_belongs_to() {
    // macOS hands out decomposed text, so a pasted `José` arrives as `Jose`
    // plus an accent that has no cell of its own.
    let painted = glyphs(&painted(&holding("e\u{301}!"), 20, true));

    assert_eq!(painted[0].glyph_key.glyph_id, u32::from('e'));
    assert_eq!(painted[1].glyph_key.glyph_id, u32::from('\u{301}'));
    assert_eq!(
        painted[0].position, painted[1].position,
        "a mark carries its own offset and shares the character's pen"
    );
    assert_eq!(
        painted[2].position.x() - painted[0].position.x(),
        font().metrics().width,
        "and the cluster took one cell, not two"
    );
}

#[test]
fn the_composer_is_drawn_in_the_terminal_s_own_colours() {
    // A shell that changed its foreground or its cursor colour at runtime —
    // OSC 10 and OSC 12, which is what every light-or-dark theme script sends
    // — takes the pane's ground and every block on it with it. A field left
    // behind in the theme's own grey is then the one thing on the pane that
    // did not follow, and it is the thing being typed into.
    let mut emulator = Emulator::new(TerminalSize::new(20, 3), 10, Palette::default());
    emulator.advance(b"\x1b]10;#102030\x07\x1b]12;#405060\x07x");
    let ink = Ink::of(&emulator.snapshot());

    assert_eq!(ink.text, Color::rgb(0x10, 0x20, 0x30));
    assert_eq!(ink.caret, Color::rgb(0x40, 0x50, 0x60));
    assert_ne!(ink.text, theme().terminal.foreground, "the shell moved it");

    let scene = painted_in(&holding("hi"), 20, true, ink);
    assert!(
        glyphs(&scene).iter().all(|glyph| glyph.color == ink.text),
        "the line being typed is not in the colour the output above it is in"
    );
    assert_eq!(
        rects(&scene).pop().map(|caret| caret.background),
        Some(Fill::Solid(ink.caret)),
        "and the caret is not the one the grid draws the shell's cursor in"
    );
}

#[test]
fn the_first_row_continues_the_prompt_and_a_wrapped_one_starts_at_the_gutter() {
    // What a shell's own line editor does, and the whole of the change: the
    // line being typed carries on from the prompt's last cell, and when it
    // runs out of row it comes back to column zero on the next one.
    let metrics = font().metrics();
    // Twelve columns with a prompt three cells wide leaves nine for the first
    // row; `abcdefghijkl` is twelve characters, so three of them wrap.
    let painted = glyphs(&painted_inline(&holding("abcdefghijkl"), 12, Some(3)));

    assert_eq!(painted.len(), 12);
    assert_eq!(
        painted[0].position,
        vec2f(3. * metrics.width, metrics.baseline - metrics.height),
        "the first character goes in the cell after the prompt, on the prompt's own row"
    );
    assert_eq!(
        painted[8].position,
        vec2f(11. * metrics.width, metrics.baseline - metrics.height),
        "and the row fills to its right edge"
    );
    assert_eq!(
        painted[9].position,
        vec2f(0., metrics.baseline),
        "what wraps starts at the gutter, not under the prompt"
    );
    assert_eq!(painted[10].position, vec2f(metrics.width, metrics.baseline));
}

#[test]
fn the_caret_sits_in_the_cell_after_the_prompt_when_the_field_is_empty() {
    // Nothing typed yet is the state a person looks at most, and it is the one
    // that used to read as a widget: a caret alone on a row of its own under
    // a prompt that ends in `❯`.
    let metrics = font().metrics();
    let caret = rects(&painted_inline(&PaneInput::new(), 20, Some(2)))
        .pop()
        .expect("a focused composer draws a caret");

    assert_eq!(
        caret.bounds.origin().x(),
        2. * metrics.width,
        "immediately after the prompt, with no empty cell between"
    );
    assert!(
        caret.bounds.origin().y() < 0.,
        "and on the prompt's row, which is above the field's own box"
    );
}

#[test]
fn a_field_that_continues_the_prompt_is_a_row_shorter_than_it_draws() {
    // The row it shares was already paid for by the list above. A field that
    // measured its full height would push the output up by a blank row and
    // leave the prompt with a gap under it.
    let metrics = font().metrics();

    let one = rows_inline("ls", 2, 20, Some(2));
    assert_eq!(one.drawn(), 1);
    assert_eq!(one.height(metrics), 0., "one row, and it is the prompt's");

    let two = rows_inline("abcdefghijkl", 12, 12, Some(3));
    assert_eq!(two.drawn(), 2);
    assert_eq!(
        two.height(metrics),
        metrics.height,
        "only the wrapped row is its own"
    );

    let alone = rows_of("ls", 2, 20);
    assert_eq!(
        alone.height(metrics),
        metrics.height,
        "and without a prompt to continue, its own row"
    );
}

#[test]
fn a_click_on_the_prompt_s_row_lands_on_the_character_it_was_aimed_at() {
    // The hit test has to undo exactly what the painter did — both the row the
    // field is drawn above its box and the columns the prompt already used —
    // or every click on the first row is out by the width of the prompt.
    let metrics = font().metrics();
    let input = holding("abcdefghijkl");
    let editor = input.editor();
    let rows = rows_inline(editor.text(), editor.caret(), 12, Some(3));
    drop(editor);
    let text = "abcdefghijkl";

    let at = |x: f32, y: f32| rows.at_point(text, vec2f(x, y), metrics);

    // The shared row is at a negative y, and its column zero is three cells in.
    let row = -metrics.height * 0.5;
    assert_eq!(at(3. * metrics.width, row), 0, "before the first character");
    assert_eq!(at(6. * metrics.width, row), 3, "the boundary before `d`");
    assert_eq!(
        at(6.6 * metrics.width, row),
        4,
        "the right half of a character puts the caret after it"
    );
    // Aiming at the prompt itself is the start of the line, never a negative
    // column: the list beside this answers for those cells.
    assert_eq!(at(0., row), 0);

    // And the row below it is an ordinary row at the gutter.
    assert_eq!(
        at(0., metrics.height * 0.5),
        9,
        "the first wrapped character"
    );
    assert_eq!(at(2. * metrics.width, metrics.height * 0.5), 11);
}

#[test]
fn a_field_scrolled_past_its_first_row_continues_nothing() {
    // A command long enough to overflow its budget scrolls, and the row at the
    // top of the window is then a middle one. Drawing that indented would put
    // the middle of a command line beside the prompt, and the caret a row out.
    let metrics = font().metrics();
    let text = "a\n".repeat(MAX_ROWS + 4);
    let rows = rows_inline(&text, text.len(), 20, Some(2));

    assert_eq!(rows.drawn(), MAX_ROWS, "the field stops growing");
    assert_eq!(
        rows.shift(),
        0,
        "there is no row here that continues a prompt"
    );
    assert_eq!(rows.height(metrics), MAX_ROWS as f32 * metrics.height);
    assert_eq!(
        rows.offset(0, metrics),
        vec2f(0., 0.),
        "every row at the gutter"
    );
}
