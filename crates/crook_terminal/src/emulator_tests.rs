use std::time::Duration;

use super::*;
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
fn test_the_mouse_modes_are_the_ones_the_child_asked_for() {
    let mut emulator = emulator();
    assert!(
        !emulator.mouse_modes().is_reporting(),
        "a fresh terminal reports nothing, which is what leaves the pointer to \
         the person"
    );
    assert!(
        emulator.mouse_modes().alternate_scroll,
        "alternate scroll is on from the start, as it is in xterm — which is \
         what makes the wheel scroll a pager that was never configured"
    );

    // What every full-screen program written this century sends: click
    // reporting, drag reporting, and the SGR encoding.
    emulator.advance(b"\x1b[?1000h\x1b[?1002h\x1b[?1006h");
    let modes = emulator.mouse_modes();
    assert!(modes.drag);
    assert!(modes.sgr);
    assert!(modes.is_reporting());

    // And on the way out it puts every one of them back, which is what makes
    // a selection work again in the shell the program returns to.
    emulator.advance(b"\x1b[?1000l\x1b[?1002l\x1b[?1006l");
    assert!(!emulator.mouse_modes().is_reporting());
    assert!(!emulator.mouse_modes().sgr);
}

#[test]
fn test_alternate_scroll_is_reported_separately_from_the_mouse() {
    // A pager relies on this and never asks for the mouse. The two have to
    // stay apart: a drag across `less` must still select, and the wheel must
    // still reach it as arrow keys.
    let mut emulator = emulator();
    assert!(emulator.mouse_modes().wants_alternate_scroll(true));

    // A program that turns it off wants the wheel to do nothing at all, which
    // is what `?1007l` has always meant.
    emulator.advance(b"\x1b[?1007l");
    assert!(!emulator.mouse_modes().alternate_scroll);
    assert!(!emulator.mouse_modes().wants_alternate_scroll(true));
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

    // The exact bytes Crook's own shell integration writes, captured from a
    // real bash: an empty authority, and `%` in a directory name escaped so it
    // is not read as the start of an escape. Both halves of the round trip are
    // in this repository, and this is the seam between them.
    assert_eq!(
        Some(PathBuf::from("/tmp")),
        parse_working_directory(b"file:///tmp")
    );
    assert_eq!(
        Some(PathBuf::from("/Users/eugen/work/connectly-frontend")),
        parse_working_directory(b"file:///Users/eugen/work/connectly-frontend")
    );
    assert_eq!(
        Some(PathBuf::from("/tmp/weird%dir")),
        parse_working_directory(b"file:///tmp/weird%25dir")
    );
}

/// Every integration reports the directory, and reports it the same way.
///
/// The bug this pins: Crook parsed OSC 7 and no shell it set up ever sent one,
/// so a tab printed the directory it was made in for the rest of its life —
/// through every `cd`, and from `/` when macOS launched the app from the Dock.
#[test]
fn test_every_shell_integration_reports_its_working_directory() {
    for (shell, snippet) in [
        (
            "zsh",
            include_str!("../../../app/src/shell_integration/crook.zsh"),
        ),
        (
            "bash",
            include_str!("../../../app/src/shell_integration/crook.bash"),
        ),
        (
            "fish",
            include_str!("../../../app/src/shell_integration/crook.fish"),
        ),
    ] {
        assert!(
            snippet.contains("\\e]7;file://"),
            "the {shell} integration sends no OSC 7, so nothing will ever \
             correct a tab's directory after a cd"
        );
    }
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

#[test]
fn test_a_program_reports_what_it_is_doing() {
    let mut emulator = emulator();
    assert_eq!(AgentReport::Idle, emulator.agent());

    emulator.advance(b"\x1b]6340;needs-input;port the tab bar\x07");

    assert_eq!(AgentReport::NeedsInput, emulator.agent());
    assert_eq!(
        vec![TerminalEvent::Agent(Reported {
            status: AgentReport::NeedsInput,
            title: Some("port the tab bar".to_owned()),
        })],
        emulator.take_events()
    );
}

#[test]
fn test_the_same_status_twice_is_reported_once_unless_it_brings_a_title() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b]6340;running\x07");
    assert_eq!(1, emulator.take_events().len());

    emulator.advance(b"\x1b]6340;running\x07");
    assert!(emulator.take_events().is_empty());

    // A title is news even when the status is not: the agent renamed its work.
    emulator.advance(b"\x1b]6340;running;still at it\x07");
    assert_eq!(1, emulator.take_events().len());
}

#[test]
fn test_a_report_survives_a_split_between_two_reads() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b]6340;fai");
    emulator.advance(b"led\x07");

    assert_eq!(AgentReport::Failed, emulator.agent());
}

#[test]
fn test_the_command_ending_takes_a_running_status_with_it() {
    // An agent that was interrupted never says it stopped. The shell's `D`
    // says it instead.
    let mut emulator = emulator();
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07claude\r\n\x1b]133;C\x07");
    emulator.advance(b"\x1b]6340;running\x07");
    emulator.take_events();

    emulator.advance(b"\x1b]133;D;130\x07");

    assert_eq!(AgentReport::Idle, emulator.agent());
    assert!(
        emulator
            .take_events()
            .contains(&TerminalEvent::Agent(Reported {
                status: AgentReport::Idle,
                title: None,
            }))
    );
}

#[test]
fn test_a_failure_outlives_its_command_and_goes_with_the_next_one() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07claude\r\n\x1b]133;C\x07");
    emulator.advance(b"\x1b]6340;failed\x07\x1b]133;D;1\x07");
    assert_eq!(AgentReport::Failed, emulator.agent());
    emulator.take_events();

    // The prompt coming back changes nothing; a new command starting does.
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");
    assert_eq!(AgentReport::Failed, emulator.agent());
    assert!(emulator.take_events().is_empty());

    emulator.advance(b"ls\r\n\x1b]133;C\x07");
    assert_eq!(AgentReport::Idle, emulator.agent());
    assert_eq!(1, emulator.take_events().len());
}
