//! Byte-sequence repros from the adversarial review of the emulator and the
//! input encoder.
//!
//! Everything here drives the crate through its public API only, and none of
//! it starts a process: what a child would be sent is asked for as bytes. Five
//! of them record behaviour the review checked and found correct, so a later
//! change cannot quietly break it; the other four were the defects it found,
//! and they now pass because those are fixed.

use std::thread;
use std::time::Duration;

use crook_terminal::{Emulator, InputModes, Key, Modifiers, Palette, TerminalSize, input};

/// A small grid, which keeps the assertions readable.
fn emulator(columns: u16, rows: u16) -> Emulator {
    Emulator::new(TerminalSize::new(columns, rows), 100, Palette::default())
}

#[test]
fn test_pasted_text_cannot_end_its_own_bracketed_paste() {
    // The bytes themselves, which is what the child on the far end of the pty
    // parses. This used to spawn `cat` and read its echo back, which is not
    // the same bytes on a Windows console — ConPTY re-renders what a child
    // writes, and an end marker the child echoed never came back as one.
    let bytes = input::paste("safe\x1b[201~; echo PWNED\n", true);
    let text = String::from_utf8(bytes).expect("a paste of text is text");

    let body = text
        .strip_prefix("\x1b[200~")
        .expect("the paste starts with the start marker");
    let inside = body
        .strip_suffix("\x1b[201~")
        .expect("the paste ends with the end marker");
    assert!(
        !inside.contains('\x1b'),
        "the escape that would end the bracket was passed through: {inside:?}"
    );
    assert!(
        inside.contains("echo PWNED"),
        "everything pasted must stay inside the brackets, but the child would see \
         {inside:?} and read the rest as if it had been typed"
    );

    // And with no bracketing asked for, a newline is the Enter key.
    assert_eq!(input::paste("one\ntwo\r\n", false), b"one\rtwo\r");
}

#[test]
fn test_an_unterminated_synchronized_update_gives_up() {
    let mut terminal = emulator(20, 5);
    // A program that dies between BSU and ESU — killed mid-frame, or crashed.
    terminal.advance(b"\x1b[?2026hHELLO");
    // vte's own limit is 150ms, and it is the caller that has to enforce it.
    thread::sleep(Duration::from_millis(300));
    terminal.advance(b"MORE");

    let snapshot = terminal.snapshot();
    assert_eq!(
        Some("HELLOMORE"),
        snapshot.text().lines().next(),
        "the grid is still empty: every byte since the BSU is sitting in vte's buffer"
    );
}

#[test]
fn test_control_with_no_c0_code_still_types_the_character() {
    let modes = InputModes::default();
    for character in ['1', '9', '0', '-', '=', ';', '.', ','] {
        let mut expected = [0; 4];
        assert_eq!(
            Some(character.encode_utf8(&mut expected).as_bytes().to_vec()),
            input::encode(Key::Char(character), Modifiers::CONTROL, modes),
            "ctrl+{character} should send the character, as xterm and every terminal after it do"
        );
    }
}

#[test]
fn test_combining_marks_survive_into_the_snapshot() {
    let mut terminal = emulator(20, 2);
    // A decomposed accent, and a Devanagari conjunct held together by a virama.
    terminal.advance("e\u{301} \u{915}\u{94d}\u{937}".as_bytes());

    let text = terminal.snapshot().text();
    let line = text.lines().next().expect("the grid has a first row");
    assert_eq!(
        "e\u{301} \u{915}\u{94d}\u{937}", line,
        "alacritty keeps these in Cell::zerowidth; the snapshot has nowhere to put them"
    );
}

#[test]
fn test_the_cursor_stays_inside_the_grid_through_every_resize() {
    let mut terminal = emulator(80, 24);
    for line in 0..300 {
        let color = 30 + line % 8;
        terminal.advance(
            format!("\x1b[{color}mrow {line} with enough content to wrap at a narrow width\r\n")
                .as_bytes(),
        );
    }

    let widths = [80, 3, 200, 2, 41, 7, 160, 5, 24, 1, 0, 97];
    let heights = [24, 1, 60, 2, 0, 9, 40, 3];
    for (wide, columns) in widths.into_iter().enumerate() {
        for (tall, rows) in heights.into_iter().enumerate() {
            terminal.resize(TerminalSize::new(columns, rows));
            if (wide + tall) % 3 == 0 {
                terminal.scroll_lines((wide * 7 + tall) as i32 - 10);
            }

            let snapshot = terminal.snapshot();
            assert_eq!(snapshot.rows * snapshot.columns, snapshot.cells.len());
            assert_eq!(snapshot.rows, snapshot.iter_rows().count());
            assert!(snapshot.display_offset <= snapshot.history_len);
            if let Some(cursor) = snapshot.cursor {
                assert!(cursor.row < snapshot.rows);
                assert!(cursor.column < snapshot.columns);
            }
        }
    }
}

#[test]
fn test_the_cube_and_the_grey_ramp_are_the_xterm_ones() {
    let palette = Palette::default();
    let mut terminal = emulator(40, 2);
    terminal.advance(
        b"\x1b[38;5;16mA\x1b[38;5;52mB\x1b[38;5;24mC\x1b[38;5;231mD\x1b[38;5;232mE\x1b[39mF",
    );

    let snapshot = terminal.snapshot();
    let expected = [
        crook_terminal::Rgb::new(0, 0, 0),
        crook_terminal::Rgb::new(95, 0, 0),
        crook_terminal::Rgb::new(0, 95, 135),
        crook_terminal::Rgb::new(255, 255, 255),
        crook_terminal::Rgb::new(8, 8, 8),
        palette.foreground,
    ];
    for (column, want) in expected.into_iter().enumerate() {
        assert_eq!(want, snapshot.cell(0, column).unwrap().foreground);
    }
}

#[test]
fn test_leaving_the_alternate_screen_restores_the_scrollback_under_it() {
    let mut terminal = emulator(20, 5);
    for line in 0..10 {
        terminal.advance(format!("p{line}\r\n").as_bytes());
    }
    let history = terminal.history_len();

    terminal.advance(b"\x1b[?1049h\x1b[Halternate");
    let alternate = terminal.snapshot();
    assert!(alternate.alt_screen);
    assert_eq!(
        0, alternate.history_len,
        "the alternate screen has no history"
    );
    // Scrolling has nowhere to go while a full-screen program is up.
    terminal.scroll_lines(5);
    assert_eq!(0, terminal.snapshot().display_offset);

    terminal.advance(b"\x1b[?1049l");
    let primary = terminal.snapshot();
    assert!(!primary.alt_screen);
    assert_eq!(history, primary.history_len);
    assert!(primary.text().contains("p9"));
}

#[test]
fn test_the_scrollback_limit_slides_rather_than_growing() {
    let mut terminal = Emulator::new(TerminalSize::new(20, 5), 3, Palette::default());
    for line in 0..20 {
        terminal.advance(format!("L{line}\r\n").as_bytes());
    }
    assert_eq!(3, terminal.history_len());

    terminal.scroll_lines(1000);
    let scrolled = terminal.snapshot();
    assert_eq!(
        3, scrolled.display_offset,
        "it stops at the oldest line it kept"
    );

    // Output arriving while the viewport is held back keeps it held back.
    terminal.advance(b"NEW\r\n");
    let after = terminal.snapshot();
    assert_eq!(3, after.display_offset);
    assert_eq!(3, after.history_len);
}

#[test]
fn test_the_revision_holds_still_for_changes_that_draw_the_same_thing() {
    let mut terminal = emulator(20, 5);
    let first = terminal.snapshot();

    // A cursor round trip, an SGR with nothing printed after it, and a
    // character written over an identical one.
    terminal.advance(b"\x1b[5C\x1b[5D\x1b[31m");
    assert_eq!(first.revision, terminal.snapshot().revision);

    terminal.advance(b"x");
    let printed = terminal.snapshot();
    assert_eq!(first.revision + 1, printed.revision);

    terminal.advance(b"\x08x");
    assert_eq!(printed.revision, terminal.snapshot().revision);
}
