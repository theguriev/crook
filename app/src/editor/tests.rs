//! Executable specification for the input's behaviour.
//!
//! Every case here is one an ordinary GUI text field gets right and a raw
//! terminal cannot: a caret that lands between characters rather than inside
//! one, a selection that shrinks when shift-extended back, undo that peels off
//! a word instead of a keystroke, and a history walk that gives back the line
//! it interrupted.

use super::*;

/// An editor holding `text`, caret at `caret`, with an empty undo stack — the
/// state a test wants to start from rather than the state loading the text
/// left behind.
fn at(text: &str, caret: usize) -> Editor {
    let mut editor = Editor::new();
    editor.set_text(text);
    editor.set_caret(caret);
    editor.revisions.clear();
    editor
}

/// Types `text` one grapheme at a time, the way a keyboard delivers it.
fn type_out(editor: &mut Editor, text: &str) {
    for grapheme in text.graphemes(true) {
        editor.insert(grapheme);
    }
}

// --- Caret movement ----------------------------------------------------------

#[test]
fn left_and_right_step_one_grapheme_and_stop_at_the_ends() {
    let mut editor = at("abc", 0);

    editor.move_caret(Motion::Left);
    assert_eq!(editor.caret(), 0);

    editor.move_caret(Motion::Right);
    editor.move_caret(Motion::Right);
    editor.move_caret(Motion::Right);
    editor.move_caret(Motion::Right);
    assert_eq!(editor.caret(), 3);
}

#[test]
fn left_and_right_step_over_a_whole_grapheme_cluster() {
    // "e" plus a combining acute: two chars, three bytes, one character as far
    // as anyone typing is concerned.
    let mut editor = at("ae\u{301}b", 0);

    editor.move_caret(Motion::Right);
    assert_eq!(editor.caret(), 1);
    editor.move_caret(Motion::Right);
    assert_eq!(editor.caret(), 4);
    editor.move_caret(Motion::Left);
    assert_eq!(editor.caret(), 1);
}

#[test]
fn word_movement_skips_the_separators_between_words() {
    let mut editor = at("git commit -m", 0);

    editor.move_caret(Motion::WordRight);
    assert_eq!(editor.caret(), 3);
    editor.move_caret(Motion::WordRight);
    assert_eq!(editor.caret(), 10);
    editor.move_caret(Motion::WordRight);
    assert_eq!(editor.caret(), 13);
    editor.move_caret(Motion::WordRight);
    assert_eq!(editor.caret(), 13);
}

#[test]
fn word_left_from_inside_a_word_lands_on_that_words_start() {
    let mut editor = at("git commit -m", 7);

    editor.move_caret(Motion::WordLeft);
    assert_eq!(editor.caret(), 4);
    editor.move_caret(Motion::WordLeft);
    assert_eq!(editor.caret(), 0);
    editor.move_caret(Motion::WordLeft);
    assert_eq!(editor.caret(), 0);
}

#[test]
fn line_start_and_end_stay_inside_their_own_line() {
    let mut editor = at("one\ntwo\nthree", 5);

    editor.move_caret(Motion::LineStart);
    assert_eq!(editor.caret(), 4);
    editor.move_caret(Motion::LineEnd);
    assert_eq!(editor.caret(), 7);
}

#[test]
fn buffer_start_and_end_cross_every_line() {
    let mut editor = at("one\ntwo\nthree", 5);

    editor.move_caret(Motion::BufferStart);
    assert_eq!(editor.caret(), 0);
    editor.move_caret(Motion::BufferEnd);
    assert_eq!(editor.caret(), 13);
}

#[test]
fn up_and_down_keep_the_goal_column_across_a_short_line() {
    // Column 5 of the first line, over a two-column line, onto a long one.
    let mut editor = at("abcdef\nxy\nabcdef", 5);

    editor.move_caret(Motion::Down);
    assert_eq!(editor.caret(), 9);
    editor.move_caret(Motion::Down);
    assert_eq!(editor.caret(), 15);
    editor.move_caret(Motion::Up);
    assert_eq!(editor.caret(), 9);
    editor.move_caret(Motion::Up);
    assert_eq!(editor.caret(), 5);
}

#[test]
fn any_other_movement_forgets_the_goal_column() {
    let mut editor = at("abcdef\nxy\nabcdef", 5);

    editor.move_caret(Motion::Down);
    editor.move_caret(Motion::Left);
    editor.move_caret(Motion::Down);

    // Column 1 of the short line, not the forgotten 5.
    assert_eq!(editor.caret(), 11);
}

#[test]
fn up_from_the_top_and_down_from_the_bottom_run_to_the_ends() {
    let mut editor = at("one\ntwo", 5);

    editor.move_caret(Motion::Down);
    assert_eq!(editor.caret(), 7);
    editor.move_caret(Motion::BufferStart);
    editor.move_caret(Motion::Right);
    editor.move_caret(Motion::Up);
    assert_eq!(editor.caret(), 0);
}

#[test]
fn a_column_is_counted_in_graphemes_not_bytes() {
    // Two clusters on the first line, five bytes.
    let mut editor = at("e\u{301}x\nabcd", 4);

    editor.move_caret(Motion::Down);
    assert_eq!(editor.caret(), 7);
}

// --- Selection ---------------------------------------------------------------

#[test]
fn shift_extending_moves_the_head_and_leaves_the_anchor() {
    let mut editor = at("hello world", 6);

    editor.extend_selection(Motion::WordRight);
    assert_eq!(editor.selection(), Selection::new(6, 11));
    assert_eq!(editor.selected_text(), "world");
}

#[test]
fn shift_extending_back_the_other_way_shrinks_the_same_selection() {
    let mut editor = at("hello world", 6);

    editor.extend_selection(Motion::Right);
    editor.extend_selection(Motion::Right);
    editor.extend_selection(Motion::Left);
    assert_eq!(editor.selection(), Selection::new(6, 7));

    editor.extend_selection(Motion::Left);
    editor.extend_selection(Motion::Left);
    // Past the anchor, and now growing leftwards.
    assert_eq!(editor.selection(), Selection::new(6, 5));
    assert_eq!(editor.selected_text(), " ");
}

#[test]
fn left_and_right_only_collapse_a_selection() {
    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(2, 5));

    editor.move_caret(Motion::Right);
    assert_eq!(editor.caret(), 5);
    assert!(editor.selection().is_empty());

    editor.set_selection(Selection::new(5, 2));
    editor.move_caret(Motion::Left);
    assert_eq!(editor.caret(), 2);
}

#[test]
fn other_motions_run_from_the_edge_they_collapse_to() {
    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(2, 5));

    editor.move_caret(Motion::LineStart);
    assert_eq!(editor.caret(), 0);

    editor.set_selection(Selection::new(2, 5));
    editor.move_caret(Motion::WordRight);
    assert_eq!(editor.caret(), 11);
}

#[test]
fn select_all_takes_everything_with_the_caret_at_the_end() {
    let mut editor = at("one\ntwo", 0);

    editor.select_all();
    assert_eq!(editor.selection(), Selection::new(0, 7));
    assert_eq!(editor.selected_text(), "one\ntwo");
}

#[test]
fn selecting_the_word_at_a_position_takes_the_whole_word() {
    let mut editor = at("git commit -m", 6);

    editor.select_word_at(6);
    assert_eq!(editor.selected_text(), "commit");
}

#[test]
fn selecting_at_a_separator_takes_the_separator() {
    let mut editor = at("git commit", 0);

    editor.select_word_at(3);
    assert_eq!(editor.selected_text(), " ");
}

#[test]
fn selecting_past_the_last_character_takes_the_last_word() {
    let mut editor = at("git commit", 0);

    editor.select_word_at(10);
    assert_eq!(editor.selected_text(), "commit");
}

#[test]
fn selecting_the_line_at_a_position_leaves_the_newline_out() {
    let mut editor = at("one\ntwo\nthree", 5);

    editor.select_line_at(5);
    assert_eq!(editor.selected_text(), "two");
}

#[test]
fn a_caret_set_inside_a_character_snaps_back_off_it() {
    let mut editor = at("a\u{1F1FA}\u{1F1F8}b", 0);

    // Byte 3 is inside the first regional indicator.
    editor.set_caret(3);
    assert_eq!(editor.caret(), 1);

    editor.set_caret(usize::MAX);
    assert_eq!(editor.caret(), editor.text().len());
}

// --- Editing -----------------------------------------------------------------

#[test]
fn typing_inserts_at_the_caret() {
    let mut editor = at("ac", 1);

    editor.insert("b");
    assert_eq!(editor.text(), "abc");
    assert_eq!(editor.caret(), 2);
}

#[test]
fn typing_over_a_selection_replaces_it() {
    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(6, 11));

    editor.insert("there");
    assert_eq!(editor.text(), "hello there");
    assert_eq!(editor.caret(), 11);
    assert!(editor.selection().is_empty());
}

#[test]
fn a_newline_opens_a_second_line() {
    let mut editor = at("ab", 1);

    editor.insert_newline();
    assert_eq!(editor.text(), "a\nb");
    assert_eq!(editor.line_count(), 2);
    assert_eq!(editor.caret_line_column(), (1, 0));
}

#[test]
fn backspace_removes_one_grapheme_cluster_not_one_char() {
    let mut editor = at("cafe\u{301}", 6);

    editor.backspace();
    assert_eq!(editor.text(), "caf");
}

#[test]
fn backspace_removes_a_whole_flag_emoji() {
    let mut editor = at("a\u{1F1FA}\u{1F1F8}", 9);

    editor.backspace();
    assert_eq!(editor.text(), "a");
}

#[test]
fn backspace_removes_a_whole_zero_width_joiner_sequence() {
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let mut editor = at(family, family.len());

    editor.backspace();
    assert_eq!(editor.text(), "");
}

#[test]
fn backspace_at_the_start_does_nothing() {
    let mut editor = at("abc", 0);

    editor.backspace();
    assert_eq!(editor.text(), "abc");
    assert!(!editor.undo());
}

#[test]
fn backspace_with_a_selection_removes_the_selection() {
    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(5, 11));

    editor.backspace();
    assert_eq!(editor.text(), "hello");
    assert_eq!(editor.caret(), 5);
}

#[test]
fn delete_forward_removes_the_cluster_after_the_caret() {
    let mut editor = at("e\u{301}x", 0);

    editor.delete_forward();
    assert_eq!(editor.text(), "x");

    editor.delete_forward();
    assert_eq!(editor.text(), "");
    editor.delete_forward();
    assert_eq!(editor.text(), "");
}

#[test]
fn deleting_a_word_left_and_right_takes_the_separators_with_it() {
    let mut editor = at("git commit -m", 13);

    editor.delete_word_left();
    assert_eq!(editor.text(), "git commit -");

    let mut editor = at("git commit -m", 3);
    editor.delete_word_right();
    assert_eq!(editor.text(), "git -m");
}

#[test]
fn deleting_to_the_line_ends_stops_at_the_newlines() {
    let mut editor = at("one\ntwo\nthree", 5);

    editor.delete_to_line_start();
    assert_eq!(editor.text(), "one\nwo\nthree");

    let mut editor = at("one\ntwo\nthree", 5);
    editor.delete_to_line_end();
    assert_eq!(editor.text(), "one\nt\nthree");
}

#[test]
fn clearing_empties_the_line() {
    let mut editor = at("half a command", 4);

    editor.clear();
    assert!(editor.is_empty());
    assert_eq!(editor.caret(), 0);
}

// --- Clipboard ---------------------------------------------------------------

#[test]
fn copy_hands_back_the_selection_and_changes_nothing() {
    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(0, 5));

    assert_eq!(editor.copy().as_deref(), Some("hello"));
    assert_eq!(editor.text(), "hello world");
}

#[test]
fn copy_with_no_selection_hands_back_nothing() {
    let editor = at("hello", 2);

    assert_eq!(editor.copy(), None);
}

#[test]
fn cut_hands_back_the_selection_and_removes_it() {
    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(5, 11));

    assert_eq!(editor.cut().as_deref(), Some(" world"));
    assert_eq!(editor.text(), "hello");

    // Nothing selected now, so nothing to cut and nothing removed.
    assert_eq!(editor.cut(), None);
    assert_eq!(editor.text(), "hello");
}

#[test]
fn pasting_text_with_newlines_keeps_them() {
    let mut editor = at("ab", 1);

    editor.paste("one\ntwo");
    assert_eq!(editor.text(), "aone\ntwob");
    assert_eq!(editor.line_count(), 2);
    assert_eq!(editor.caret(), 8);
}

#[test]
fn pasting_normalises_carriage_returns() {
    let mut editor = at("", 0);

    editor.paste("one\r\ntwo\rthree");
    assert_eq!(editor.text(), "one\ntwo\nthree");
    assert_eq!(editor.line_count(), 3);
}

#[test]
fn pasting_over_a_selection_replaces_it_in_one_step() {
    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(0, 5));

    editor.paste("goodbye");
    assert_eq!(editor.text(), "goodbye world");

    editor.undo();
    assert_eq!(editor.text(), "hello world");
}

// --- Undo --------------------------------------------------------------------

#[test]
fn a_run_of_typed_characters_is_one_undo_step() {
    let mut editor = Editor::new();

    type_out(&mut editor, "hello");

    assert!(editor.undo());
    assert_eq!(editor.text(), "");
    assert!(!editor.undo());
}

#[test]
fn a_run_of_typing_breaks_at_a_word_boundary() {
    let mut editor = Editor::new();

    type_out(&mut editor, "git commit");

    assert!(editor.undo());
    assert_eq!(editor.text(), "git");
    assert!(editor.undo());
    assert_eq!(editor.text(), "");
}

#[test]
fn a_run_of_typing_breaks_when_the_caret_jumps() {
    let mut editor = Editor::new();

    type_out(&mut editor, "abc");
    editor.move_caret(Motion::Left);
    type_out(&mut editor, "x");

    assert_eq!(editor.text(), "abxc");
    editor.undo();
    assert_eq!(editor.text(), "abc");
    assert_eq!(editor.caret(), 2);
    editor.undo();
    assert_eq!(editor.text(), "");
}

#[test]
fn a_run_of_typing_breaks_at_a_delete() {
    let mut editor = Editor::new();

    type_out(&mut editor, "abc");
    editor.backspace();
    type_out(&mut editor, "d");

    assert_eq!(editor.text(), "abd");
    editor.undo();
    assert_eq!(editor.text(), "ab");
    editor.undo();
    assert_eq!(editor.text(), "abc");
    editor.undo();
    assert_eq!(editor.text(), "");
}

#[test]
fn a_run_of_backspaces_is_one_undo_step() {
    let mut editor = at("hello", 5);

    editor.backspace();
    editor.backspace();
    editor.backspace();

    assert_eq!(editor.text(), "he");
    assert!(editor.undo());
    assert_eq!(editor.text(), "hello");
    assert!(!editor.undo());
}

#[test]
fn typing_over_a_selection_is_a_step_of_its_own() {
    let mut editor = Editor::new();

    type_out(&mut editor, "hello");
    editor.select_all();
    type_out(&mut editor, "bye");

    editor.undo();
    assert_eq!(editor.text(), "hello");
    assert_eq!(editor.selection(), Selection::new(0, 5));
}

#[test]
fn undo_puts_the_selection_back_as_well_as_the_text() {
    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(6, 11));

    editor.backspace();
    assert_eq!(editor.text(), "hello ");

    editor.undo();
    assert_eq!(editor.selection(), Selection::new(6, 11));
}

#[test]
fn redo_replays_what_undo_took_back() {
    let mut editor = Editor::new();

    type_out(&mut editor, "hello");
    editor.undo();
    assert_eq!(editor.text(), "");

    assert!(editor.redo());
    assert_eq!(editor.text(), "hello");
    assert!(!editor.redo());
}

#[test]
fn editing_after_an_undo_abandons_the_redo() {
    let mut editor = Editor::new();

    type_out(&mut editor, "hello");
    editor.undo();
    type_out(&mut editor, "bye");

    assert!(!editor.redo());
    assert_eq!(editor.text(), "bye");
}

// --- History -----------------------------------------------------------------

#[test]
fn submitting_hands_back_the_line_empties_the_editor_and_records_it() {
    let mut editor = Editor::new();
    type_out(&mut editor, "ls -la");

    assert_eq!(editor.submit(), "ls -la");
    assert!(editor.is_empty());
    assert_eq!(editor.caret(), 0);
    assert_eq!(editor.history(), ["ls -la"]);
    assert!(!editor.undo());
}

#[test]
fn a_blank_line_is_not_recorded() {
    let mut editor = Editor::new();

    assert_eq!(editor.submit(), "");
    type_out(&mut editor, "   ");
    assert_eq!(editor.submit(), "   ");

    assert!(editor.history().is_empty());
}

#[test]
fn walking_up_the_history_reaches_the_oldest_entry_and_stops() {
    let mut editor = Editor::new();
    for line in ["one", "two", "three"] {
        type_out(&mut editor, line);
        editor.submit();
    }

    assert!(editor.history_previous());
    assert_eq!(editor.text(), "three");
    assert_eq!(editor.caret(), 5);
    assert!(editor.history_previous());
    assert_eq!(editor.text(), "two");
    assert!(editor.history_previous());
    assert_eq!(editor.text(), "one");
    assert!(!editor.history_previous());
    assert_eq!(editor.text(), "one");
}

#[test]
fn walking_back_down_restores_the_line_that_was_in_progress() {
    let mut editor = Editor::new();
    type_out(&mut editor, "ls");
    editor.submit();
    type_out(&mut editor, "half typed");

    editor.history_previous();
    assert_eq!(editor.text(), "ls");

    assert!(editor.history_next());
    assert_eq!(editor.text(), "half typed");
    assert_eq!(editor.caret(), 10);
    assert!(!editor.history_next());
}

#[test]
fn editing_a_recalled_entry_leaves_the_draft_alone() {
    let mut editor = Editor::new();
    type_out(&mut editor, "ls");
    editor.submit();
    type_out(&mut editor, "draft");

    editor.history_previous();
    type_out(&mut editor, " -la");
    assert_eq!(editor.text(), "ls -la");

    editor.history_next();
    assert_eq!(editor.text(), "draft");
}

#[test]
fn recalling_an_entry_starts_a_new_undo_session() {
    let mut editor = Editor::new();
    type_out(&mut editor, "ls");
    editor.submit();
    type_out(&mut editor, "draft");

    editor.history_previous();
    assert!(!editor.undo());
    assert_eq!(editor.text(), "ls");
}

#[test]
fn history_walking_does_nothing_when_there_is_nothing_to_walk() {
    let mut editor = at("typing", 6);

    assert!(!editor.history_previous());
    assert!(!editor.history_next());
    assert_eq!(editor.text(), "typing");
}

#[test]
fn up_reaches_for_history_only_from_the_first_line() {
    let mut editor = Editor::new();
    type_out(&mut editor, "old");
    editor.submit();
    type_out(&mut editor, "one");
    editor.insert_newline();
    type_out(&mut editor, "two");

    // On the second line: Up is a movement.
    editor.up();
    assert_eq!(editor.text(), "one\ntwo");
    assert_eq!(editor.caret(), 3);

    // On the first line: Up is the history.
    editor.up();
    assert_eq!(editor.text(), "old");
}

#[test]
fn down_reaches_for_history_only_from_the_last_line() {
    let mut editor = Editor::new();
    type_out(&mut editor, "old");
    editor.submit();
    type_out(&mut editor, "one");
    editor.insert_newline();
    type_out(&mut editor, "two");
    let draft = editor.text().to_string();

    editor.history_previous();
    assert_eq!(editor.text(), "old");

    editor.down();
    assert_eq!(editor.text(), draft);

    // Back on the multi-line draft, at the end: Down has nowhere to walk.
    editor.move_caret(Motion::BufferStart);
    editor.down();
    assert_eq!(editor.caret(), 4);
    assert_eq!(editor.text(), draft);
}

#[test]
fn up_with_no_history_still_moves_to_the_start_of_the_line() {
    let mut editor = at("one line", 4);

    editor.up();
    assert_eq!(editor.caret(), 0);
    assert_eq!(editor.text(), "one line");
}

#[test]
fn clearing_abandons_a_history_walk() {
    let mut editor = Editor::new();
    type_out(&mut editor, "ls");
    editor.submit();
    type_out(&mut editor, "draft");

    editor.history_previous();
    editor.clear();

    assert!(!editor.history_next());
    assert!(editor.is_empty());
}

// --- Positions for the renderer ---------------------------------------------

#[test]
fn a_line_and_column_round_trips_through_an_offset() {
    let editor = at("one\ntwo\nthree", 0);

    assert_eq!(editor.offset_at(0, 0), 0);
    assert_eq!(editor.offset_at(1, 2), 6);
    assert_eq!(editor.offset_at(2, 5), 13);
    // Past the end of a line, and past the end of the text.
    assert_eq!(editor.offset_at(1, 99), 7);
    assert_eq!(editor.offset_at(99, 0), 13);
}

#[test]
fn the_caret_reports_the_line_and_column_it_is_on() {
    let mut editor = at("one\ntwo", 0);
    assert_eq!(editor.caret_line_column(), (0, 0));

    editor.set_caret(6);
    assert_eq!(editor.caret_line_column(), (1, 2));
}

#[test]
fn a_trailing_newline_opens_a_line_the_caret_can_sit_on() {
    let editor = at("one\n", 4);

    assert_eq!(editor.line_count(), 2);
    assert_eq!(editor.caret_line_column(), (1, 0));
}

// --- What a selection changes ------------------------------------------------

#[test]
fn the_arrows_move_a_selected_caret_by_a_line_rather_than_reaching_for_history() {
    // The caret is what is on the first line or the last, not the edges of
    // what is selected. Selecting a whole two-line command and pressing Up
    // used to replace it with the previous history entry.
    let mut editor = at("one\ntwo", 0);
    editor.submit();
    editor.set_text("first\nsecond");
    editor.select_all();

    editor.up();
    assert_eq!(editor.text(), "first\nsecond", "the history took the line");
    assert_eq!(
        editor.caret_line_column(),
        (0, 0),
        "it collapsed the selection and moved, which is what Up does"
    );

    // And the mirror: a head dragged up to the first line, with the anchor
    // still below it, is not on the last line either.
    editor.set_selection(Selection::new(12, 2));
    editor.down();
    assert_eq!(editor.text(), "first\nsecond", "the history took the line");
    assert_eq!(editor.caret_line_column(), (1, 6));
}

#[test]
fn deleting_to_the_end_of_a_line_deletes_the_selection_when_there_is_one() {
    // Every other delete replaces the selection, and so does Cocoa's
    // `deleteToBeginningOfLine:`. These two used to delete *around* it: `hello
    // world` with `world` selected came back as `world`.
    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(6, 11));
    editor.delete_to_line_start();
    assert_eq!(editor.text(), "hello ");

    let mut editor = at("hello world", 0);
    editor.set_selection(Selection::new(0, 5));
    editor.delete_to_line_end();
    assert_eq!(editor.text(), " world");

    // With nothing selected they are the line kills they always were.
    let mut editor = at("hello world", 6);
    editor.delete_to_line_start();
    assert_eq!(editor.text(), "world");
}

// --- What can get into the buffer --------------------------------------------

#[test]
fn a_carriage_return_never_reaches_the_buffer_whichever_way_text_arrives() {
    // A `\r` is a character UAX #29 binds to the `\n` after it while every
    // line query here splits on the `\n` byte, so one left in the buffer puts
    // a caret between two halves of a grapheme cluster.
    for text in ["a\r\nb", "a\rb"] {
        let mut editor = Editor::new();
        editor.set_text(text);
        assert_eq!(editor.text(), "a\nb");

        let mut editor = Editor::new();
        editor.paste(text);
        assert_eq!(editor.text(), "a\nb");

        let mut editor = Editor::new();
        editor.insert(text);
        assert_eq!(editor.text(), "a\nb");
    }
}

#[test]
fn a_pasted_escape_sequence_is_not_carried_to_the_shell() {
    // A command copied off a web page can hold an ESC, a BEL or a tab. The
    // keyboard path already refuses every one of them; the clipboard used to
    // insert them verbatim, drawing nothing and then writing the escape to the
    // pty on Enter.
    let mut editor = Editor::new();
    editor.paste("ls\u{1b}[31m\u{7}\tx");
    assert_eq!(
        editor.text(),
        "ls[31m x",
        "the escape and the bell are gone; what is left is visible, which is \
         the whole point — the field shows exactly what Enter will send"
    );
    assert_eq!(editor.submit(), "ls[31m x");
}

#[test]
fn text_left_with_nothing_printable_in_it_inserts_nothing() {
    let mut editor = at("abc", 3);
    editor.paste("\u{1b}\u{7}");
    assert_eq!(editor.text(), "abc");
    assert!(!editor.undo(), "and it is not an undo step either");
}
