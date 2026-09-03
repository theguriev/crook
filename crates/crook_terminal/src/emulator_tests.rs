use std::time::Duration;

use super::*;
use crate::selection::{CellSide, GridPoint, SelectionKind, ViewportPoint};
use crate::snapshot::{CellFlags, CursorShape, Rgb};

/// A small grid, which keeps the assertions readable and the snapshots cheap.
fn emulator() -> Emulator {
    Emulator::new(TerminalSize::new(20, 5), 100, Palette::default())
}

#[test]
fn test_written_text_appears_in_the_snapshot() {
    let mut emulator = emulator();
    emulator.advance(b"hello");

    let snapshot = emulator.snapshot();
    assert_eq!("hello", snapshot.text().lines().next().unwrap());
    assert_eq!('h', snapshot.cell(0, 0).unwrap().c);
    assert_eq!('o', snapshot.cell(0, 4).unwrap().c);
    assert_eq!(' ', snapshot.cell(0, 5).unwrap().c);
}

#[test]
fn test_carriage_return_and_line_feed_move_the_cursor() {
    let mut emulator = emulator();
    emulator.advance(b"ab\r\ncd");

    let snapshot = emulator.snapshot();
    assert_eq!("ab\ncd", snapshot.text().trim_end());
    let cursor = snapshot.cursor.expect("the cursor is visible by default");
    assert_eq!(1, cursor.row);
    assert_eq!(2, cursor.column);
    assert_eq!(CursorShape::Block, cursor.shape);
}

#[test]
fn test_hiding_the_cursor_removes_it_from_the_snapshot() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b[?25l");
    assert_eq!(None, emulator.snapshot().cursor);

    emulator.advance(b"\x1b[?25h");
    assert!(emulator.snapshot().cursor.is_some());
}

#[test]
fn test_an_sgr_colour_arrives_resolved() {
    let palette = Palette::default();
    let mut emulator = emulator();
    // Named red, then a 256-colour index, then a direct RGB triple.
    emulator.advance(b"\x1b[31mn\x1b[38;5;196mi\x1b[38;2;10;20;30mr");

    let snapshot = emulator.snapshot();
    assert_eq!(palette.ansi[1], snapshot.cell(0, 0).unwrap().foreground);
    assert_eq!(palette.ansi[196], snapshot.cell(0, 1).unwrap().foreground);
    assert_eq!(
        Rgb::new(10, 20, 30),
        snapshot.cell(0, 2).unwrap().foreground
    );
    // Nothing set a background, so every cell has the palette's.
    assert_eq!(palette.background, snapshot.cell(0, 0).unwrap().background);
}

#[test]
fn test_inverse_swaps_the_resolved_colours() {
    let palette = Palette::default();
    let mut emulator = emulator();
    emulator.advance(b"\x1b[31;7mx");

    let cell = *emulator.snapshot().cell(0, 0).unwrap();
    assert_eq!(palette.background, cell.foreground);
    assert_eq!(palette.ansi[1], cell.background);
}

#[test]
fn test_attributes_reach_the_snapshot_flags() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b[1;3;4;9mx");

    let flags = emulator.snapshot().cell(0, 0).unwrap().flags;
    assert!(flags.contains(CellFlags::BOLD));
    assert!(flags.contains(CellFlags::ITALIC));
    assert!(flags.contains(CellFlags::UNDERLINE));
    assert!(flags.contains(CellFlags::STRIKEOUT));
    assert!(!flags.contains(CellFlags::WIDE));
}

#[test]
fn test_a_double_width_character_claims_two_columns() {
    let mut emulator = emulator();
    emulator.advance("\u{6f22}x".as_bytes());

    let snapshot = emulator.snapshot();
    let wide = snapshot.cell(0, 0).unwrap();
    assert_eq!('\u{6f22}', wide.c);
    assert!(wide.flags.contains(CellFlags::WIDE));
    assert!(
        snapshot
            .cell(0, 1)
            .unwrap()
            .flags
            .contains(CellFlags::WIDE_SPACER)
    );
    assert_eq!('x', snapshot.cell(0, 2).unwrap().c);
    // The spacer contributes no character of its own to the text.
    assert_eq!("\u{6f22}x", snapshot.text().lines().next().unwrap());
}

#[test]
fn test_resizing_changes_the_snapshot_dimensions() {
    let mut emulator = emulator();
    let before = emulator.snapshot();
    assert_eq!((20, 5), (before.columns, before.rows));

    emulator.resize(TerminalSize::new(40, 12));

    let after = emulator.snapshot();
    assert_eq!((40, 12), (after.columns, after.rows));
    assert_eq!(40 * 12, after.cells.len());
    assert!(after.revision > before.revision);
    assert_eq!(TerminalSize::new(40, 12), emulator.size());
}

#[test]
fn test_the_alternate_screen_switches_and_switches_back() {
    let mut emulator = emulator();
    emulator.advance(b"primary");
    assert!(!emulator.is_alt_screen());

    // Entering the alternate screen keeps the cursor where it was, so a
    // full-screen program homes it before drawing, and so does this.
    emulator.advance(b"\x1b[?1049h\x1b[H");
    emulator.advance(b"alternate");
    assert!(emulator.is_alt_screen());
    let alternate = emulator.snapshot();
    assert!(alternate.alt_screen);
    assert_eq!("alternate", alternate.text().lines().next().unwrap());

    emulator.advance(b"\x1b[?1049l");
    assert!(!emulator.is_alt_screen());
    let primary = emulator.snapshot();
    assert!(!primary.alt_screen);
    assert_eq!("primary", primary.text().lines().next().unwrap());
}

#[test]
fn test_osc_zero_sets_the_title() {
    let mut emulator = emulator();
    assert_eq!(None, emulator.title());

    emulator.advance(b"\x1b]0;building crook\x07");

    assert_eq!(Some("building crook"), emulator.title());
    assert_eq!(Some("building crook".to_owned()), emulator.snapshot().title);
    assert_eq!(
        vec![TerminalEvent::Title(Some("building crook".to_owned()))],
        emulator.take_events()
    );
    // Draining takes the events with it.
    assert!(emulator.take_events().is_empty());
}

#[test]
fn test_setting_the_same_title_twice_reports_it_once() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b]2;same\x07");
    assert_eq!(1, emulator.take_events().len());

    emulator.advance(b"\x1b]2;same\x07");
    assert!(emulator.take_events().is_empty());
}

#[test]
fn test_osc_seven_reports_the_working_directory() {
    let mut emulator = emulator();
    assert_eq!(None, emulator.working_directory());

    emulator.advance(b"\x1b]7;file://localhost/tmp/some%20dir\x1b\\");

    assert_eq!(
        Some(Path::new("/tmp/some dir")),
        emulator.working_directory()
    );
    assert_eq!(
        vec![TerminalEvent::WorkingDirectory(PathBuf::from(
            "/tmp/some dir"
        ))],
        emulator.take_events()
    );
}

#[test]
fn test_osc_seven_survives_a_split_between_two_reads() {
    let mut emulator = emulator();
    // The pty hands over whatever happens to have arrived, which routinely
    // cuts an escape sequence in half.
    emulator.advance(b"\x1b]7;file://host/var/l");
    emulator.advance(b"og\x07");

    assert_eq!(Some(Path::new("/var/log")), emulator.working_directory());
}

#[test]
fn test_the_bell_is_reported() {
    let mut emulator = emulator();
    emulator.advance(b"ding\x07");

    assert_eq!(vec![TerminalEvent::Bell], emulator.take_events());
}

#[test]
fn test_an_osc_52_write_is_reported_and_a_read_is_not_answered() {
    let mut emulator = emulator();
    // `c` is the clipboard selection; the payload is base64, which alacritty
    // decodes. "aGVsbG8=" is "hello".
    emulator.advance(b"\x1b]52;c;aGVsbG8=\x07");

    assert_eq!(
        vec![TerminalEvent::ClipboardStore("hello".to_owned())],
        emulator.take_events()
    );

    // A `?` payload asks the terminal to *send* the clipboard back. Answering
    // it would hand any program that can print to a pty the contents of the
    // clipboard, so it produces neither an event nor a reply.
    emulator.advance(b"\x1b]52;c;?\x07");
    assert!(emulator.take_events().is_empty());
    assert!(emulator.take_replies().is_empty());
}

#[test]
fn test_a_query_from_the_child_is_answered() {
    let mut emulator = emulator();
    // Device attributes: a program that gets no answer to this waits forever.
    emulator.advance(b"\x1b[c");

    let replies = emulator.take_replies();
    assert!(
        !replies.is_empty(),
        "the child's query should have been answered"
    );
    assert!(emulator.take_replies().is_empty());
}

#[test]
fn test_the_revision_moves_only_when_something_changed() {
    let mut emulator = emulator();
    let first = emulator.snapshot();
    assert_eq!(0, first.revision);

    // Nothing at all.
    emulator.advance(b"");
    assert!(Arc::ptr_eq(&first, &emulator.snapshot()));

    // Bytes that change nothing on screen: a private mode with no meaning here.
    emulator.advance(b"\x1b[?7727h");
    assert!(Arc::ptr_eq(&first, &emulator.snapshot()));

    // Taking a snapshot twice over is not a change either.
    assert!(Arc::ptr_eq(&emulator.snapshot(), &emulator.snapshot()));

    emulator.advance(b"x");
    let second = emulator.snapshot();
    assert_eq!(first.revision + 1, second.revision);
    assert!(Arc::ptr_eq(&second, &emulator.snapshot()));
}

#[test]
fn test_scrolling_back_moves_the_viewport_and_returns() {
    let mut emulator = emulator();
    for line in 0..10 {
        emulator.advance(format!("line{line}\r\n").as_bytes());
    }
    assert!(emulator.history_len() > 0);

    let live = emulator.snapshot();
    assert_eq!(0, live.display_offset);

    emulator.scroll_lines(3);
    let scrolled = emulator.snapshot();
    assert_eq!(3, scrolled.display_offset);
    assert!(scrolled.text().contains("line6"));

    emulator.scroll_to_bottom();
    assert_eq!(0, emulator.snapshot().display_offset);
}

#[test]
fn test_a_new_palette_repaints_the_grid() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b[31mx");
    let before = emulator.snapshot();

    let mut palette = Palette::default();
    palette.ansi[1] = Rgb::new(1, 2, 3);
    emulator.set_palette(palette);

    let after = emulator.snapshot();
    assert!(after.revision > before.revision);
    assert_eq!(Rgb::new(1, 2, 3), after.cell(0, 0).unwrap().foreground);
}

#[test]
fn test_an_osc_four_override_beats_the_palette() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b]4;1;rgb:00/ff/00\x07\x1b[31mx");

    assert_eq!(
        Rgb::new(0, 255, 0),
        emulator.snapshot().cell(0, 0).unwrap().foreground
    );
}

#[test]
fn test_bracketed_paste_and_cursor_modes_are_reported() {
    let mut emulator = emulator();
    assert!(!emulator.bracketed_paste());
    assert!(!emulator.input_modes().application_cursor);

    emulator.advance(b"\x1b[?2004h\x1b[?1h");

    assert!(emulator.bracketed_paste());
    assert!(emulator.input_modes().application_cursor);
}

#[test]
fn test_working_directory_urls_are_parsed() {
    assert_eq!(
        Some(PathBuf::from("/home/eugen/work")),
        parse_working_directory(b"file://laptop/home/eugen/work")
    );
    // Some shells report a bare path.
    assert_eq!(
        Some(PathBuf::from("/srv")),
        parse_working_directory(b"/srv")
    );
    // Percent escapes come back as themselves.
    assert_eq!(
        Some(PathBuf::from("/a b/c#d")),
        parse_working_directory(b"/a%20b/c%23d")
    );
    // A URL with no path at all says nothing useful.
    assert_eq!(None, parse_working_directory(b"file://host"));
    assert_eq!(None, parse_working_directory(b""));
}

#[test]
fn test_percent_decoding_rejects_truncated_escapes() {
    assert_eq!(Some("plain".to_owned()), percent_decode("plain"));
    assert_eq!(None, percent_decode("%zz"));
    // A trailing `%` is not an escape, so it survives as itself.
    assert_eq!(Some("100%".to_owned()), percent_decode("100%"));
}

#[test]
fn test_a_synchronized_update_nobody_ended_gives_up() {
    // A full-screen program that is killed between `BSU` and `ESU` leaves the
    // parser buffering. Without a deadline the pane keeps drawing the frame
    // from before the update for the rest of its life, and no later output can
    // ever appear.
    let mut emulator = emulator();
    emulator.advance(b"\x1b[?2026hHELLO");
    assert_eq!(
        Some(""),
        emulator.snapshot().text().lines().next(),
        "while the update is live the buffered bytes are correctly invisible"
    );

    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        Some("HELLO"),
        emulator.snapshot().text().lines().next(),
        "painting is the moment a dead program's update expires"
    );

    emulator.advance(b"MORE");
    assert_eq!(Some("HELLOMORE"), emulator.snapshot().text().lines().next());
}

#[test]
fn test_a_synchronized_update_that_ends_in_time_is_still_atomic() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b[?2026hhalf");
    assert_eq!(Some(""), emulator.snapshot().text().lines().next());
    emulator.advance(b" drawn\x1b[?2026l");
    assert_eq!(
        Some("half drawn"),
        emulator.snapshot().text().lines().next()
    );
}

#[test]
fn test_dim_text_resolves_through_the_palettes_own_dim_slots() {
    // The dim slots are what a theme sets to say "this is my dim grey"; a
    // renderer that scaled the resolved colour instead could never read them.
    let mut palette = Palette {
        foreground: Rgb::hex(0x123456),
        dim_foreground: Rgb::hex(0xabcdef),
        ..Palette::default()
    };
    palette.ansi[1] = Rgb::hex(0x800000);

    let mut emulator = Emulator::new(TerminalSize::new(20, 2), 10, palette.clone());
    emulator.advance(b"\x1b[2mD\x1b[31mR\x1b[38;2;200;100;50mS");

    let snapshot = emulator.snapshot();
    assert_eq!(
        palette.dim_foreground,
        snapshot.cell(0, 0).unwrap().foreground,
        "dim over the default foreground is the palette's dim foreground"
    );
    assert_eq!(
        palette.ansi[1].scaled(0.66),
        snapshot.cell(0, 1).unwrap().foreground,
        "no palette carries the eight dim ANSI colours, so they are held back"
    );
    assert_eq!(
        Rgb::new(200, 100, 50).scaled(0.66),
        snapshot.cell(0, 2).unwrap().foreground,
        "a 24-bit colour has no dim counterpart to look up"
    );
}

#[test]
fn test_combining_marks_stay_with_the_cell_they_belong_to() {
    // macOS hands out decomposed filenames and author names by default, so
    // dropping these turns `José` into `Jose` in a `git log` — and a virama
    // into a different word.
    let mut emulator = emulator();
    emulator.advance("e\u{301} \u{915}\u{94d}\u{937} x".as_bytes());

    let snapshot = emulator.snapshot();
    assert_eq!(
        Some("e\u{301} \u{915}\u{94d}\u{937} x"),
        snapshot.text().lines().next()
    );
    assert!(snapshot.has_combining());
    assert_eq!(&['\u{301}'], snapshot.zerowidth(0, 0));
    // The virama is zero-width and rides on the consonant before it; the
    // second consonant is a cell of its own.
    assert_eq!(&['\u{94d}'], snapshot.zerowidth(0, 2));
    assert_eq!('\u{937}', snapshot.cell(0, 3).unwrap().c);
    assert_eq!(&[] as &[char], snapshot.zerowidth(0, 1));
    // Out of the grid is empty rather than a panic, like `cell`.
    assert_eq!(&[] as &[char], snapshot.zerowidth(99, 99));

    // A cell keeping its character but losing its marks is a changed screen.
    let mut bare = Emulator::new(TerminalSize::new(20, 5), 100, Palette::default());
    bare.advance(b"e \xe0\xa4\x95 x");
    assert!(!snapshot.same_content(&bare.snapshot()));
}

#[test]
fn test_an_ordinary_screen_carries_no_combining_table_at_all() {
    let mut emulator = emulator();
    emulator.advance(b"plain ascii");
    let snapshot = emulator.snapshot();
    assert!(!snapshot.has_combining());
    assert!(snapshot.combining.is_empty());
}

/// Selects from one cell to another with a plain drag, the way a pointer does.
fn drag(emulator: &mut Emulator, from: (usize, usize), to: (usize, usize)) {
    emulator.start_selection(
        SelectionKind::Simple,
        ViewportPoint::new(from.0, from.1),
        CellSide::Left,
    );
    emulator.update_selection(ViewportPoint::new(to.0, to.1), CellSide::Right);
}

#[test]
fn test_a_drag_selects_the_cells_it_covers_and_hands_back_their_text() {
    let mut emulator = emulator();
    emulator.advance(b"hello world");
    drag(&mut emulator, (0, 6), (0, 10));

    assert_eq!(Some("world".to_owned()), emulator.selection_text());
    assert!(emulator.has_selection());

    let snapshot = emulator.snapshot();
    assert!(snapshot.is_selected(0, 6));
    assert!(snapshot.is_selected(0, 10));
    assert!(!snapshot.is_selected(0, 5), "the space before it");
    assert!(!snapshot.is_selected(0, 11), "past the last cell");
}

#[test]
fn test_which_half_of_a_cell_was_pressed_decides_whether_it_is_taken() {
    // The gap a selection ends in is between two cells, not on one. Dragging
    // right from the left of `w` takes it; starting from its right half does
    // not.
    let mut emulator = emulator();
    emulator.advance(b"hello world");

    emulator.start_selection(
        SelectionKind::Simple,
        ViewportPoint::new(0, 6),
        CellSide::Right,
    );
    emulator.update_selection(ViewportPoint::new(0, 10), CellSide::Right);
    assert_eq!(Some("orld".to_owned()), emulator.selection_text());

    emulator.start_selection(
        SelectionKind::Simple,
        ViewportPoint::new(0, 6),
        CellSide::Left,
    );
    emulator.update_selection(ViewportPoint::new(0, 10), CellSide::Left);
    assert_eq!(Some("worl".to_owned()), emulator.selection_text());
}

#[test]
fn test_a_press_with_no_drag_behind_it_selects_nothing_at_all() {
    // What makes a plain click clear the last selection rather than leave a
    // one-cell highlight where it landed.
    let mut emulator = emulator();
    emulator.advance(b"hello world");
    drag(&mut emulator, (0, 0), (0, 4));
    assert!(emulator.has_selection());

    emulator.start_selection(
        SelectionKind::Simple,
        ViewportPoint::new(0, 8),
        CellSide::Left,
    );
    assert!(!emulator.has_selection(), "a click selected a cell");
    assert_eq!(None, emulator.snapshot().selection);
}

#[test]
fn test_a_word_and_a_line_selection_use_the_emulators_own_boundaries() {
    // The double and triple click. The rules are the emulator's — its semantic
    // escape characters, its idea of where a wrapped line ends — which is the
    // reason for going through it rather than splitting the snapshot's text.
    let mut emulator = emulator();
    emulator.advance(b"one two three");

    emulator.start_selection(
        SelectionKind::Semantic,
        ViewportPoint::new(0, 5),
        CellSide::Left,
    );
    assert_eq!(Some("two".to_owned()), emulator.selection_text());

    emulator.start_selection(
        SelectionKind::Lines,
        ViewportPoint::new(0, 5),
        CellSide::Left,
    );
    assert_eq!(
        Some("one two three\n".to_owned()),
        emulator.selection_text()
    );
}

#[test]
fn test_a_selection_dragged_out_of_a_wrapped_line_comes_back_as_one_line() {
    // A line too long for the grid is one line of text on two rows, and the
    // fold is a place the terminal put the text rather than something the
    // shell printed. Copying it back with a newline in it would break the
    // command it came from.
    let mut emulator = emulator();
    emulator.advance(b"abcdefghijklmnopqrstuvwxyz");
    assert_eq!(
        "abcdefghijklmnopqrst",
        emulator
            .snapshot()
            .row(0)
            .iter()
            .map(|cell| cell.c)
            .collect::<String>()
    );

    drag(&mut emulator, (0, 0), (1, 5));
    assert_eq!(
        Some("abcdefghijklmnopqrstuvwxyz".to_owned()),
        emulator.selection_text()
    );
}

#[test]
fn test_a_selection_stays_on_its_text_while_output_scrolls_underneath_it() {
    // **The reason the selection lives in the emulator.** `Selection::rotate`
    // is what moves it with the text, and `Term` calls it on every path that
    // scrolls — so a selection made around a word is still around that word
    // after another screenful has printed below it, and the cells it covers
    // are somewhere else entirely.
    let mut emulator = emulator();
    emulator.advance(b"marker line\r\n");
    drag(&mut emulator, (0, 0), (0, 10));
    assert_eq!(Some("marker line".to_owned()), emulator.selection_text());
    assert_eq!(
        Some(GridPoint::new(0, 0)),
        emulator.selection().map(|span| span.start)
    );

    // Enough lines to push it off the screen entirely: the grid is five rows.
    for line in 0..12 {
        emulator.advance(format!("filler {line}\r\n").as_bytes());
    }

    assert_eq!(
        Some("marker line".to_owned()),
        emulator.selection_text(),
        "the selection came away from the text it was drawn around"
    );
    let span = emulator.selection().expect("it is still selected");
    assert_eq!(
        GridPoint::new(-9, 0),
        span.start,
        "and it has followed the text up into the history"
    );
    assert!(
        !emulator.snapshot().text().contains("marker"),
        "the marker is not even on screen any more"
    );
    assert_eq!(
        0,
        emulator.snapshot().display_offset,
        "the viewport did not move; the text did"
    );
}

#[test]
fn test_a_selection_made_in_the_scrollback_lands_on_the_lines_it_was_aimed_at() {
    // The display offset, which is the whole of "a selection made four screens
    // back must not highlight the live output".
    let mut emulator = emulator();
    for line in 0..20 {
        emulator.advance(format!("line {line:02}\r\n").as_bytes());
    }
    emulator.scroll_lines(10);
    let snapshot = emulator.snapshot();
    assert_eq!(10, snapshot.display_offset);
    assert_eq!("line 06", snapshot.text().lines().next().unwrap());

    drag(&mut emulator, (0, 0), (0, 6));
    assert_eq!(Some("line 06".to_owned()), emulator.selection_text());
    assert!(
        emulator.snapshot().is_selected(0, 0),
        "the top row of the scrolled viewport is the row that was pressed"
    );

    // And scrolling back to the live output leaves the highlight behind with
    // the text, rather than dragging it down the screen.
    emulator.scroll_to_bottom();
    let snapshot = emulator.snapshot();
    assert_eq!(Some("line 06".to_owned()), emulator.selection_text());
    assert!(!snapshot.is_selected(0, 0));
}

#[test]
fn test_moving_a_selection_never_rebuilds_the_grid() {
    // The reason `Snapshot::selection` is a span and not a flag on every cell.
    // A drag is a stream of pointer moves and none of them changes a character
    // the shell printed, so the cells of the last snapshot are the right cells
    // and are carried across untouched.
    let mut emulator = emulator();
    emulator.advance(b"hello world");
    let before = emulator.snapshot();

    drag(&mut emulator, (0, 0), (0, 4));
    let after = emulator.snapshot();

    assert_eq!(
        before.cells, after.cells,
        "a drag changed a cell the child printed"
    );
    assert_eq!(
        before.revision + 1,
        after.revision,
        "a highlight is drawn content, so the revision has to move with it"
    );
    assert!(!before.same_content(&after));

    // And a pointer move that lands on the same cells costs no revision at
    // all, so holding a button still is free.
    emulator.update_selection(ViewportPoint::new(0, 4), CellSide::Right);
    assert_eq!(after.revision, emulator.snapshot().revision);
}

#[test]
fn test_a_cleared_selection_leaves_the_grid_exactly_as_it_was() {
    let mut emulator = emulator();
    emulator.advance(b"hello world");
    let before = emulator.snapshot();

    drag(&mut emulator, (0, 0), (0, 4));
    emulator.snapshot();
    emulator.clear_selection();

    let after = emulator.snapshot();
    assert!(!emulator.has_selection());
    assert_eq!(None, emulator.selection_text());
    assert!(
        before.same_content(&after),
        "clearing a selection changed something other than the selection"
    );
}

#[test]
fn test_a_selection_can_be_found_by_the_text_it_covers() {
    // What `--select-output` and the tests select with, since neither has a
    // pointer to aim.
    let mut emulator = emulator();
    emulator.advance(b"alpha\r\nbeta\r\ngamma");

    let (start, end) = emulator
        .snapshot()
        .find("beta\ngam")
        .expect("the screen shows it");
    assert_eq!(ViewportPoint::new(1, 0), start);
    assert_eq!(ViewportPoint::new(2, 2), end);

    emulator.start_selection(SelectionKind::Simple, start, CellSide::Left);
    emulator.update_selection(end, CellSide::Right);
    assert_eq!(Some("beta\ngam".to_owned()), emulator.selection_text());

    assert_eq!(None, emulator.snapshot().find("nothing here"));
    assert_eq!(None, emulator.snapshot().find(""));
}

/// Every block gesture that can be aimed at a small grid, so the sweep below
/// cannot miss one for being unable to imagine it.
fn block_gestures(
    rows: usize,
    columns: usize,
) -> impl Iterator<Item = ((usize, usize, CellSide), (usize, usize, CellSide))> {
    let sides = [CellSide::Left, CellSide::Right];
    (0..rows).flat_map(move |from_row| {
        (0..columns).flat_map(move |from_column| {
            sides.into_iter().flat_map(move |from_side| {
                (0..rows).flat_map(move |to_row| {
                    (0..columns).flat_map(move |to_column| {
                        sides.into_iter().map(move |to_side| {
                            (
                                (from_row, from_column, from_side),
                                (to_row, to_column, to_side),
                            )
                        })
                    })
                })
            })
        })
    })
}

#[test]
fn test_a_block_that_ends_where_it_began_selects_nothing_rather_than_a_row_it_cannot_index() {
    // **A crash and a stolen interrupt, from the same arithmetic.**
    // `Selection::range_block` moves the start a column right when the drag
    // began on the right of a cell and the end a column left when it finished
    // on the left of one, and never checks the two did not cross. On the last
    // column the start lands one past the row, which `selection_to_string`
    // indexes it with; anywhere else the range comes back inverted, covering
    // no cell — so nothing is highlighted while `has_selection` still says
    // there is something to copy, and off macOS that spends the interrupt.
    let mut emulator = emulator();
    for row in 0..5 {
        emulator.advance(format!("row{row} abcdefghijklmn\r\n").as_bytes());
    }
    let columns = emulator.size().columns as usize;

    for column in [0, 4, columns - 1] {
        emulator.start_selection(
            SelectionKind::Block,
            ViewportPoint::new(0, column),
            CellSide::Right,
        );
        emulator.update_selection(ViewportPoint::new(2, column), CellSide::Left);

        assert_eq!(
            None,
            emulator.selection(),
            "a block with no width in it selected column {column}"
        );
        assert!(!emulator.has_selection());
        // The line that used to panic on the last column, and used to hand
        // back a clipboard full of newlines on every other one.
        assert_eq!(None, emulator.selection_text());
        assert_eq!(None, emulator.snapshot().selection);
    }
}

#[test]
fn test_no_block_gesture_at_all_leaves_a_span_the_grid_cannot_be_walked_with() {
    // The exhaustive form of the test above: every pair of cells on a small
    // grid, both sides of each, is either a span whose text can be taken or no
    // span at all. Nothing in between, because everything in between is a
    // panic or a highlight of nothing.
    let mut emulator = Emulator::new(TerminalSize::new(8, 4), 100, Palette::default());
    emulator.advance(b"ab cd ef\r\ngh ij kl\r\nmn op qr\r\nst uv wx");
    let (rows, columns) = (4, 8);

    for (from, to) in block_gestures(rows, columns) {
        emulator.start_selection(
            SelectionKind::Block,
            ViewportPoint::new(from.0, from.1),
            from.2,
        );
        emulator.update_selection(ViewportPoint::new(to.0, to.1), to.2);

        let Some(span) = emulator.selection() else {
            assert_eq!(None, emulator.selection_text(), "{from:?} -> {to:?}");
            continue;
        };
        assert!(
            span.start.line <= span.end.line && span.start.column <= span.end.column,
            "{from:?} -> {to:?} came back inverted: {span:?}"
        );
        assert!(
            span.end.column < columns,
            "{from:?} -> {to:?} names a column the grid does not have: {span:?}"
        );
        // Would panic rather than fail, which is the point of running it.
        assert!(emulator.selection_text().is_some(), "{from:?} -> {to:?}");
    }
}

#[test]
fn test_a_double_width_character_is_highlighted_across_both_of_its_columns() {
    // Its glyph is drawn once, from the first column, across the width of two.
    // A highlight that lit only the column the character is stored in would
    // cut the glyph down the middle — while the copy took the whole of it,
    // because `line_to_string` emits the character for a range touching
    // either half.
    let mut emulator = emulator();
    emulator.advance("a漢b".as_bytes());

    // Ending on the right of the character's own column, and on either side of
    // its trailing half: all three take the character, so all three light both
    // of the columns it is drawn across.
    for end in [
        (1, CellSide::Right),
        (2, CellSide::Left),
        (2, CellSide::Right),
    ] {
        emulator.start_selection(
            SelectionKind::Simple,
            ViewportPoint::new(0, 0),
            CellSide::Left,
        );
        emulator.update_selection(ViewportPoint::new(0, end.0), end.1);

        let snapshot = emulator.snapshot();
        let text = emulator.selection_text().expect("something is selected");
        assert!(text.contains('漢'), "{end:?} did not take the character");
        assert!(
            snapshot.is_selected(0, 1) && snapshot.is_selected(0, 2),
            "{end:?} highlighted half a glyph"
        );
    }

    // And the character is not dragged into a selection that stops before it.
    emulator.start_selection(
        SelectionKind::Simple,
        ViewportPoint::new(0, 0),
        CellSide::Left,
    );
    emulator.update_selection(ViewportPoint::new(0, 0), CellSide::Right);
    let snapshot = emulator.snapshot();
    assert_eq!(Some("a".to_owned()), emulator.selection_text());
    assert!(!snapshot.is_selected(0, 1) && !snapshot.is_selected(0, 2));
}
