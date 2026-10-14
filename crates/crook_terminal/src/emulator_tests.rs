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
fn test_a_hidden_cursor_still_says_which_cell_it_is_in() {
    // What a program that draws its own caret does — Claude Code hides the
    // real cursor and parks it on the field it is reading — and the cell an
    // input method composes at. Moving it while hidden is a change to draw.
    let mut emulator = emulator();
    emulator.advance(b"\x1b[?25lab\r\ncd");
    let before = emulator.snapshot();
    assert_eq!(None, before.cursor, "nothing to draw");
    assert_eq!(Some((1, 2)), before.cursor_cell);

    emulator.advance(b"\x1b[1;2H");
    let after = emulator.snapshot();
    assert_eq!(Some((0, 1)), after.cursor_cell);
    assert!(
        after.revision > before.revision,
        "the move was not a change"
    );

    // Shown, the two agree.
    emulator.advance(b"\x1b[?25h");
    let shown = emulator.snapshot();
    let cursor = shown.cursor.expect("the cursor is shown again");
    assert_eq!(Some((cursor.row, cursor.column)), shown.cursor_cell);
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

/// Four megabytes of `x`, the length of title that froze the window.
fn huge() -> String {
    "x".repeat(4 << 20)
}

/// `text` cut at `bytes` the way a pane's string is, for a text of one-byte
/// characters.
fn cut_at(text: &str, bytes: usize) -> String {
    format!("{}\u{2026}", &text[..bytes])
}

#[test]
fn test_a_title_longer_than_a_row_is_kept_cut_to_its_cap() {
    // A title is whatever the program in the pane wrote, and `vte` keeps an
    // OSC however long it runs: `cat` a file holding `ESC ] 2 ;` and four
    // megabytes, and that was the tab's name — shaped whole by every frame
    // that drew its row, seconds each, until something set another.
    let mut emulator = emulator();
    let huge = huge();
    emulator.advance(format!("\x1b]2;{huge}\x07").as_bytes());

    let kept = cut_at(&huge, agent::TITLE_BYTES);
    let title = emulator.title().map(str::len);
    assert_eq!(Some(kept.len()), title, "the title kept {title:?} bytes");
    assert_eq!(Some(kept.as_str()), emulator.title());
    assert_eq!(Some(&kept), emulator.snapshot().title.as_ref());
    let events = emulator.take_events();
    assert!(
        matches!(&events[..], [TerminalEvent::Title(Some(title))] if *title == kept),
        "the event carried {:?} bytes",
        events.iter().map(|event| match event {
            TerminalEvent::Title(title) => title.as_ref().map(String::len),
            _ => None,
        })
    );

    // The same title again is the same name, cut the same way, and no news.
    emulator.advance(format!("\x1b]0;{huge}\x07").as_bytes());
    assert!(emulator.take_events().is_empty());
}

#[test]
fn test_a_title_is_kept_whole_up_to_its_cap_and_cut_where_a_character_ends() {
    let mut emulator = emulator();
    let whole = "x".repeat(agent::TITLE_BYTES);
    emulator.advance(format!("\x1b]2;{whole}\x07").as_bytes());
    assert_eq!(Some(whole.as_str()), emulator.title());

    // Three bytes each, against a cap that is no multiple of three, so the
    // cap falls inside a character: it goes whole, rather than leaving half
    // of itself before the mark.
    assert_ne!(0, agent::TITLE_BYTES % 3);
    let wide = "語".repeat(agent::TITLE_BYTES);
    emulator.advance(format!("\x1b]2;{wide}\x07").as_bytes());
    let kept = format!("{}\u{2026}", "語".repeat(agent::TITLE_BYTES / 3));
    assert_eq!(Some(kept.as_str()), emulator.title());
}

/// The title the grid itself pushed last with `CSI 22 t`, popped back off
/// its stack: what `Term` holds there, before this crate cuts anything.
fn pushed_title(emulator: &mut Emulator) -> Option<String> {
    emulator.proxy.take();
    Handler::pop_title(&mut emulator.term);
    emulator
        .proxy
        .take()
        .into_iter()
        .find_map(|event| match event {
            Event::Title(title) => Some(title),
            _ => None,
        })
}

#[test]
fn test_an_over_long_title_is_cut_in_the_copy_the_grid_pushes() {
    // `Term` keeps a title of its own, and `CSI 22 t` pushes a copy of it on
    // a stack 4096 deep: four megabytes of title and twenty kilobytes of
    // pushes after it, in the same file, are 4096 copies: sixteen gigabytes.
    let mut emulator = emulator();
    let huge = huge();
    emulator.advance(format!("\x1b]2;{huge}\x07\x1b[22t\x1b]2;short\x07").as_bytes());

    let kept = cut_at(&huge, agent::TITLE_BYTES);
    let pushed = pushed_title(&mut emulator);
    let length = pushed.as_ref().map(String::len);
    assert_eq!(Some(kept.len()), length, "the grid pushed {length:?} bytes");
    assert_eq!(Some(kept.clone()), pushed);

    // Telling the grid the cut title is no news of its own.
    let events = emulator.take_events();
    assert!(
        matches!(
            &events[..],
            [TerminalEvent::Title(Some(first)), TerminalEvent::Title(Some(second))]
                if *first == kept && second == "short"
        ),
        "{} events, not the cut title and then the short one",
        events.len()
    );
}

#[test]
fn test_an_over_long_title_in_a_synchronized_update_is_cut_before_what_follows_it() {
    // `vte` holds an update's bytes until it ends and then plays them to the
    // grid in one go, the title and the pushes after it together. So the
    // update ends at the title, the way one nobody ended does, and what is
    // left of it is drawn as it comes. One byte past what an update is left
    // to carry, and one megabyte, because past two `vte` gives up on the
    // update by itself.
    for bytes in [UPDATE_TITLE_BYTES + 1, 1 << 20] {
        let mut emulator = emulator();
        let long = "x".repeat(bytes);
        emulator.advance(
            format!("\x1b[?2026h\x1b]2;{long}\x07\x1b[22t\x1b]2;short\x07\x1b[?2026l").as_bytes(),
        );

        let kept = cut_at(&long, agent::TITLE_BYTES);
        let pushed = pushed_title(&mut emulator);
        let length = pushed.as_ref().map(String::len);
        assert_eq!(
            Some(kept.len()),
            length,
            "a {bytes}-byte title: the grid pushed {length:?} bytes"
        );
        assert_eq!(Some("short"), emulator.title());
    }
}

#[test]
fn test_a_title_a_program_could_write_leaves_its_synchronized_update_whole() {
    // Past the cap, but no more than an update is left to carry: ending the
    // update at the title drew half a frame each time a program that
    // brackets its frames set a title a few hundred bytes long, to save
    // copies that come to 16 MiB at most. The update is held until it ends,
    // and its title is cut when the grid hands it over.
    for bytes in [agent::TITLE_BYTES + 1, 300, UPDATE_TITLE_BYTES] {
        let mut emulator = emulator();
        let title = "t".repeat(bytes);
        emulator.advance(format!("\x1b[?2026h\x1b]2;{title}\x07\x1b[22tframe").as_bytes());
        assert_eq!(
            Some(""),
            emulator.snapshot().text().lines().next(),
            "a {bytes}-byte title drew the update before it ended"
        );

        emulator.advance(b"\x1b[?2026l");
        assert_eq!(Some("frame"), emulator.snapshot().text().lines().next());
        let kept = cut_at(&title, agent::TITLE_BYTES);
        assert_eq!(Some(kept.as_str()), emulator.title());
        // The cost of leaving the update alone: the copy pushed inside it is
        // whole, and no copy can be longer than this.
        let length = pushed_title(&mut emulator).as_ref().map(String::len);
        assert_eq!(Some(bytes), length, "a {bytes}-byte title");
    }
}

#[test]
fn test_a_report_longer_than_a_row_is_kept_cut_to_its_caps() {
    // Anything in the pane can write the sequence — a forged report is a
    // `printf` away — and its title and message reach the same row a
    // terminal title does, the message on a line of its own.
    let mut emulator = emulator();
    let huge = huge();
    emulator.advance(format!("\x1b]6340;needs-input;{huge};;{huge}\x07").as_bytes());

    let reported = match &emulator.take_events()[..] {
        [TerminalEvent::Agent(reported)] => reported.clone(),
        events => panic!("{} events, not one report", events.len()),
    };
    let length = |text: &Option<String>| text.as_ref().map(String::len);
    let (title, message) = (length(&reported.title), length(&reported.message));
    assert_eq!(
        (
            Some(cut_at(&huge, agent::TITLE_BYTES).len()),
            Some(cut_at(&huge, agent::MESSAGE_BYTES).len())
        ),
        (title, message),
        "the title kept {title:?} bytes and the message {message:?}"
    );
    assert_eq!(
        Reported {
            status: AgentReport::NeedsInput,
            title: Some(cut_at(&huge, agent::TITLE_BYTES)),
            message: Some(cut_at(&huge, agent::MESSAGE_BYTES)),
        },
        reported
    );
}

#[test]
fn test_a_working_directory_longer_than_any_path_is_refused_rather_than_cut() {
    // Cut, it would be another directory, which the row, git and the next
    // pane would all be told about; refused, the pane is where it last said.
    let mut emulator = emulator();
    emulator.advance(b"\x1b]7;file:///tmp\x07");
    emulator.take_events();

    emulator.advance(format!("\x1b]7;file://host/{}\x07", huge()).as_bytes());
    let directory = emulator
        .working_directory()
        .map(|path| path.as_os_str().len());
    assert_eq!(
        Some(4),
        directory,
        "the directory is {directory:?} bytes long"
    );
    assert_eq!(Some(Path::new("/tmp")), emulator.working_directory());
    assert!(emulator.take_events().is_empty());

    let longest = format!("/{}", "d".repeat(DIRECTORY_BYTES - 1));
    assert_eq!(
        Some((None, PathBuf::from(&longest))),
        parse_working_directory(longest.as_bytes())
    );
    let over = format!("{longest}d");
    assert_eq!(None, parse_working_directory(over.as_bytes()));
    // Measured as the path it decodes to, not the escapes it arrived in.
    let escaped = format!("/{}", "%64".repeat(DIRECTORY_BYTES - 1));
    assert_eq!(
        Some((None, PathBuf::from(&longest))),
        parse_working_directory(escaped.as_bytes())
    );
}

#[test]
fn test_what_the_grid_reports_before_a_mark_is_handed_over_before_it() {
    // A program's last title and the shell's `D` after it land in one read
    // whenever nothing reads the pty between them. The workspace takes back
    // a running the title said when the command ends, so a title handed over
    // after the end would say running again for a program that has gone —
    // and for good, since it says nothing more. One read hands them over in
    // the order they were written, as a read for each does; a bell keeps its
    // place the same way. A title written after the mark, as a shell's
    // prompt writes one, still comes after it.
    let bell_and_title = "\x07\x1b]0;\u{25d0} Fix the flaky test\x07";
    let end = "\x1b]133;D;137\x07";
    let prompt = "\x1b]0;eugen@devbox: ~/src\x07\x1b]133;A\x07$ ";
    let started = || {
        let mut emulator = emulator();
        emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07claude\r\n\x1b]133;C\x07");
        emulator.take_events();
        emulator
    };
    let order = |emulator: &mut Emulator| -> Vec<&'static str> {
        emulator
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                TerminalEvent::Bell => Some("bell"),
                TerminalEvent::Title(_) => Some("title"),
                TerminalEvent::CommandFinished { .. } => Some("finished"),
                _ => None,
            })
            .collect()
    };
    let written = ["bell", "title", "finished", "title"];

    let mut split = started();
    for read in [bell_and_title, end, prompt] {
        split.advance(read.as_bytes());
    }
    assert_eq!(order(&mut split), written, "a read for each");

    let mut whole = started();
    whole.advance(format!("{bell_and_title}{end}{prompt}").as_bytes());
    assert_eq!(order(&mut whole), written, "all three in one read");
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
        vec![TerminalEvent::WorkingDirectory {
            host: Some("localhost".to_owned()),
            path: PathBuf::from("/tmp/some dir"),
        }],
        emulator.take_events()
    );
}

#[test]
fn test_the_same_directory_on_another_machine_is_reported_again() {
    // `ssh` from ~/work/app into a server with the same layout, and back: the
    // path never changes, and the machine it is on does twice. A report that
    // only looked at the path would say nothing either time, and the pane
    // would go on reading this machine's repository for the server's shell —
    // or, once back, the server's for this one.
    let mut emulator = emulator();
    emulator.advance(b"\x1b]7;file://laptop/home/eugen/work/app\x07");
    emulator.take_events();

    emulator.advance(b"\x1b]7;file://buildbox/home/eugen/work/app\x07");
    assert_eq!(
        vec![TerminalEvent::WorkingDirectory {
            host: Some("buildbox".to_owned()),
            path: PathBuf::from("/home/eugen/work/app"),
        }],
        emulator.take_events()
    );

    emulator.advance(b"\x1b]7;file://laptop/home/eugen/work/app\x07");
    assert_eq!(
        vec![TerminalEvent::WorkingDirectory {
            host: Some("laptop".to_owned()),
            path: PathBuf::from("/home/eugen/work/app"),
        }],
        emulator.take_events()
    );

    // And the same place twice is still said once.
    emulator.advance(b"\x1b]7;file://laptop/home/eugen/work/app\x07");
    assert!(emulator.take_events().is_empty());
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
fn test_the_snapshot_says_whether_the_child_has_the_mouse() {
    // What the pointer over the grid is drawn from: the frame paints from the
    // snapshot, and asking the terminal instead would take the lock the
    // reader parses under on every frame.
    let mut emulator = emulator();
    assert!(!emulator.snapshot().mouse_reporting);

    emulator.advance(b"\x1b[?1000h");
    assert!(
        emulator.snapshot().mouse_reporting,
        "a mode the child asked for with nothing drawn is still a new snapshot"
    );

    // Any of the three reports is the mouse taken; alternate scroll is not.
    emulator.advance(b"\x1b[?1000l\x1b[?1003h");
    assert!(emulator.snapshot().mouse_reporting);
    emulator.advance(b"\x1b[?1003l");
    assert!(!emulator.snapshot().mouse_reporting);
    assert!(emulator.mouse_modes().alternate_scroll);
}

#[test]
fn test_focus_reporting_is_the_mode_the_child_asked_for() {
    let mut emulator = emulator();
    assert!(
        !emulator.focus_reporting(),
        "a fresh terminal reports no focus, which is what keeps `CSI I` off a \
         shell's command line"
    );

    emulator.advance(b"\x1b[?1004h");
    assert!(emulator.focus_reporting());

    emulator.advance(b"\x1b[?1004l");
    assert!(!emulator.focus_reporting());
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
fn test_a_mirror_owes_the_child_no_replies() {
    // What a mirror is for: the stream it follows is also being followed by
    // an emulator that answers, and a second answer would reach the child
    // as input it never asked for.
    let queries = b"\x1b[c\x1b[6n\x1b]11;?\x07\x1b[14t";

    let mut answering = emulator();
    answering.advance(queries);
    assert!(!answering.take_replies().is_empty());

    let mut mirror = emulator();
    mirror.advance_mirrored(Fed::Output(queries));
    assert_eq!(Vec::<u8>::new(), mirror.take_replies());
}

#[test]
fn test_a_mirror_owes_nothing_for_an_update_that_expires_while_it_paints() {
    // A synchronized update nobody ended is let go by painting as well as by
    // the next feed, and the queries buffered inside it are answered then —
    // after the mirrored call that fed them has returned.
    let update = b"\x1b[?2026h\x1b[c\x1b[6n";

    let mut answering = emulator();
    answering.advance(update);
    let mut mirror = emulator();
    mirror.advance_mirrored(Fed::Output(update));
    assert_eq!(Vec::<u8>::new(), answering.take_replies());
    assert_eq!(Vec::<u8>::new(), mirror.take_replies());

    std::thread::sleep(Duration::from_millis(200));
    answering.snapshot();
    mirror.snapshot();
    assert!(
        !answering.take_replies().is_empty(),
        "the expired update's queries are answered when it is painted"
    );
    assert_eq!(Vec::<u8>::new(), mirror.take_replies());

    // A mirror stays one, whichever way it is fed after.
    mirror.advance(b"\x1b[c");
    assert_eq!(Vec::<u8>::new(), mirror.take_replies());
}

/// What [`named`] answers XTVERSION with.
const VERSION_REPLY: &str = "\x1bP>|Crook 1.2.3\x1b\\";

/// An emulator with a name to answer XTVERSION with, as Crook's own have.
fn named() -> Emulator {
    let mut emulator = emulator();
    emulator.set_name_and_version(Some("Crook 1.2.3"));
    emulator
}

/// The replies owed so far, as text, so a failure prints escapes rather than
/// a list of numbers.
fn answered(emulator: &mut Emulator) -> String {
    String::from_utf8(emulator.take_replies()).expect("every reply here is text")
}

#[test]
fn test_a_version_query_is_answered_ahead_of_the_queries_after_it() {
    // Claude Code's startup probe, byte for byte: XTVERSION, the kitty
    // keyboard query and DA1, the sentinel every terminal answers. It reads
    // up to the sentinel's answer, and asks whether synchronized output is
    // supported only when the version came before it.
    let mut emulator = named();
    emulator.advance(b"\x1b[>0q\x1b[?u\x1b[c");
    assert_eq!(
        format!("{VERSION_REPLY}\x1b[?0u\x1b[?6c"),
        answered(&mut emulator)
    );
}

#[test]
fn test_a_version_query_is_answered_between_the_queries_either_side_of_it() {
    // `Term` queues its answers as it parses: a version answered once the
    // whole read was parsed went out last, and one answered before any of
    // it went out first.
    let mut emulator = named();
    emulator.advance(b"\x1b[?u\x1b[>q\x1b[6n\x1b[c");
    assert_eq!(
        format!("\x1b[?0u{VERSION_REPLY}\x1b[1;1R\x1b[?6c"),
        answered(&mut emulator)
    );
}

#[test]
fn test_a_version_query_split_across_reads_is_answered_once() {
    let probe = b"\x1b[>0q\x1b[c";
    for cut in 1..probe.len() {
        let mut emulator = named();
        emulator.advance(&probe[..cut]);
        let mut replies = answered(&mut emulator);
        emulator.advance(&probe[cut..]);
        replies.push_str(&answered(&mut emulator));
        assert_eq!(
            format!("{VERSION_REPLY}\x1b[?6c"),
            replies,
            "cut after {cut} bytes"
        );
    }
}

#[test]
fn test_only_xtversion_is_answered_with_the_version() {
    // The other `q`s: the cursor shape, DECLL's keyboard lights, and the
    // version query with a parameter xterm gives no meaning.
    let mut emulator = named();
    emulator.advance(b"\x1b[ q\x1b[1q\x1b[>1q\x1b[c");
    assert_eq!("\x1b[?6c", answered(&mut emulator));
}

#[test]
fn test_an_emulator_with_no_name_leaves_the_version_query_unanswered() {
    // What a program probing with a sentinel behind the query is built for:
    // the sentinel's answer says no other is coming.
    let mut emulator = emulator();
    emulator.advance(b"\x1b[>0q\x1b[c");
    assert_eq!("\x1b[?6c", answered(&mut emulator));
}

#[test]
fn test_a_mirror_does_not_answer_the_version_query() {
    let mut answering = named();
    answering.advance(b"\x1b[>0q");
    assert_eq!(VERSION_REPLY, answered(&mut answering));

    let mut mirror = named();
    mirror.advance_mirrored(Fed::Output(b"\x1b[>0q"));
    assert_eq!("", answered(&mut mirror));
}

#[test]
fn test_a_version_query_in_a_synchronized_update_is_answered_before_the_update_ends() {
    // `vte` holds an update's queries until it ends, and answers them then.
    // The version cannot wait with them in its place — the watcher sees it
    // as it arrives — so it goes at once: ahead of every query sent after
    // it, which is the order a program waiting on a sentinel needs.
    let mut emulator = named();
    emulator.advance(b"\x1b[?2026h\x1b[>0q\x1b[c");
    assert_eq!(VERSION_REPLY, answered(&mut emulator));

    emulator.advance(b"\x1b[?2026l");
    assert_eq!("\x1b[?6c", answered(&mut emulator));
}

#[test]
fn test_a_program_asking_about_synchronized_updates_is_told_they_are_supported() {
    // The question a version answer unlocks for Claude Code, which brackets
    // its frames when the mode is known, set (1) or reset (2). Each frame is
    // then held whole until it ends, rather than painted half done when a
    // read ends in the middle of it.
    let mut emulator = emulator();
    emulator.advance(b"\x1b[?2026$p");
    assert_eq!("\x1b[?2026;2$y", answered(&mut emulator));
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

/// The first `rows` rows of a snapshot, as text.
fn top_rows(snapshot: &Snapshot, rows: usize) -> Vec<String> {
    snapshot
        .text()
        .lines()
        .take(rows)
        .map(str::to_owned)
        .collect()
}

/// An emulator showing `line0` to `line9`, scrolled back three lines so that
/// its view runs from `line3` to `line7`.
fn scrolled_back() -> Emulator {
    let mut emulator = emulator();
    for line in 0..10 {
        emulator.advance(format!("line{line}\r\n").as_bytes());
    }
    emulator.scroll_lines(3);
    assert_eq!(
        vec!["line3", "line4", "line5", "line6", "line7"],
        top_rows(&emulator.snapshot(), 5)
    );
    emulator
}

#[test]
fn test_output_under_a_scrolled_back_view_leaves_it_on_the_lines_it_showed() {
    let mut emulator = scrolled_back();
    let before = emulator.snapshot();

    emulator.advance(b"more\r\nand more\r\n");

    let after = emulator.snapshot();
    assert_eq!(before.display_offset + 2, after.display_offset);
    assert_eq!(before.text(), after.text());
}

#[test]
fn test_a_scroll_region_below_the_top_row_leaves_a_scrolled_back_view_on_its_lines() {
    let mut emulator = scrolled_back();
    let before = emulator.snapshot();

    // Row 1 pinned and rows 2 to 5 scrolling. Nothing the region scrolls
    // reaches the history, so the three history rows in view and the pinned
    // row under them are exactly what was there.
    emulator.advance(b"\x1b[2;5r\x1b[5;1Hone\r\ntwo\r\n");

    let after = emulator.snapshot();
    assert_eq!(before.display_offset, after.display_offset);
    assert_eq!(before.history_len, after.history_len);
    assert_eq!(top_rows(&before, 4), top_rows(&after, 4));
}

#[test]
fn test_deleting_lines_below_the_top_row_leaves_a_scrolled_back_view_on_its_lines() {
    let mut emulator = scrolled_back();
    let before = emulator.snapshot();

    // No region at all: a delete at row 3 scrolls rows 3 to 5 up through the
    // same path in alacritty a region below the top row does, and the two
    // screen rows in view are both above it.
    emulator.advance(b"\x1b[3;1H\x1b[M");

    let after = emulator.snapshot();
    assert_eq!(before.display_offset, after.display_offset);
    assert_eq!(before.text(), after.text());
}

#[test]
fn test_a_scrolled_back_view_follows_its_lines_through_a_full_history() {
    // Four lines of history, full long before the tenth line. Every line
    // printed now evicts the oldest, which moves everything above the screen
    // up by one, and the view has to move with it to stay where it was.
    let mut emulator = Emulator::new(TerminalSize::new(20, 5), 4, Palette::default());
    for line in 0..10 {
        emulator.advance(format!("line{line}\r\n").as_bytes());
    }
    assert_eq!(4, emulator.history_len());
    emulator.scroll_lines(2);
    let before = emulator.snapshot();
    assert_eq!(
        vec!["line4", "line5", "line6", "line7", "line8"],
        top_rows(&before, 5)
    );

    emulator.advance(b"more\r\n");

    let after = emulator.snapshot();
    assert_eq!(before.display_offset + 1, after.display_offset);
    assert_eq!(before.text(), after.text());
}

#[test]
fn test_a_scroll_region_below_the_top_row_cannot_take_the_view_past_the_history() {
    // The offset alacritty raised for every line such a region scrolled was
    // capped by the scrollback setting, not by the history there is, so with
    // the default setting it ran thousands of lines past the oldest one and
    // the snapshot indexed rows the grid does not have. In a debug build any
    // cap trips alacritty's own assertion on that index. The real default is
    // what makes a release build panic too, out of bounds in the snapshot,
    // where a small cap only draws stale or blank rows. The screen only has
    // to be tall enough for the region below: on five rows `5;24` is a single
    // row, which alacritty clears rather than scrolls, and the offset never
    // moves.
    let mut emulator = Emulator::new(
        TerminalSize::new(80, 24),
        crate::DEFAULT_SCROLLBACK_LINES,
        Palette::default(),
    );
    for line in 0..30 {
        emulator.advance(format!("line{line}\r\n").as_bytes());
    }
    emulator.scroll_lines(5);
    let before = emulator.snapshot();

    // A four-row header pinned above a scrolling log, which is what
    // `tput csr` or a follower with a status bar sets up.
    emulator.advance(b"\x1b[5;24r\x1b[24;1H");
    let output: String = (0..5000).map(|line| format!("out{line}\r\n")).collect();
    emulator.advance(output.as_bytes());

    let after = emulator.snapshot();
    assert_eq!(5, after.display_offset);
    assert_eq!(top_rows(&before, 9), top_rows(&after, 9));
}

#[test]
fn test_a_screen_switch_behind_such_a_region_still_cannot_take_the_view_past_the_history() {
    // The read that scrolls the region also enters the alternate screen,
    // which parks the shell's grid, raised offset and all, where a view
    // measured on one screen cannot be compared with the other. It comes back
    // a read later, and has to come back inside its history.
    let mut emulator = Emulator::new(
        TerminalSize::new(80, 24),
        crate::DEFAULT_SCROLLBACK_LINES,
        Palette::default(),
    );
    for line in 0..30 {
        emulator.advance(format!("line{line}\r\n").as_bytes());
    }
    emulator.scroll_lines(5);

    let mut output = b"\x1b[5;24r\x1b[24;1H".to_vec();
    for line in 0..5000 {
        output.extend_from_slice(format!("out{line}\r\n").as_bytes());
    }
    output.extend_from_slice(b"\x1b[?1049h");
    emulator.advance(&output);
    assert!(emulator.snapshot().alt_screen);
    emulator.advance(b"\x1b[?1049l");

    let after = emulator.snapshot();
    assert!(!after.alt_screen);
    assert!(after.display_offset <= after.history_len);
}

#[test]
fn test_a_synchronized_update_let_go_by_a_paint_leaves_a_scrolled_back_view_on_its_lines() {
    // The update's bytes reach the grid only when the snapshot gives up on
    // it, so that is a feed of its own, and the region it scrolls is the
    // same trap there.
    let mut emulator = scrolled_back();
    let before = emulator.snapshot();

    emulator.advance(b"\x1b[?2026h\x1b[2;5r\x1b[5;1Hone\r\ntwo\r\n");
    std::thread::sleep(Duration::from_millis(200));

    let after = emulator.snapshot();
    assert_eq!(before.display_offset, after.display_offset);
    assert_eq!(top_rows(&before, 4), top_rows(&after, 4));
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
    // The host is the machine the path is on, and it comes back with it: a
    // shell on the far side of an ssh reports a directory there, and whether
    // that is this machine is the application's to say.
    assert_eq!(
        Some((Some("laptop".to_owned()), PathBuf::from("/home/eugen/work"))),
        parse_working_directory(b"file://laptop/home/eugen/work")
    );
    assert_eq!(
        Some((Some("localhost".to_owned()), PathBuf::from("/tmp"))),
        parse_working_directory(b"file://localhost/tmp")
    );
    // Some shells report a bare path, which names no host at all.
    assert_eq!(
        Some((None, PathBuf::from("/srv"))),
        parse_working_directory(b"/srv")
    );
    // Percent escapes come back as themselves.
    assert_eq!(
        Some((None, PathBuf::from("/a b/c#d"))),
        parse_working_directory(b"/a%20b/c%23d")
    );
    // A URL with no path at all says nothing useful.
    assert_eq!(None, parse_working_directory(b"file://host"));
    assert_eq!(None, parse_working_directory(b""));

    // The exact bytes Crook's own shell integration writes, captured from a
    // real bash: the host the shell is on, and `%` in a directory name
    // escaped so it is not read as the start of an escape. Both halves of the
    // round trip are in this repository, and this is the seam between them.
    assert_eq!(
        Some((Some("omarchy".to_owned()), PathBuf::from("/tmp"))),
        parse_working_directory(b"file://omarchy/tmp")
    );
    assert_eq!(
        Some((
            Some("Eugens-MacBook-Pro.local".to_owned()),
            PathBuf::from("/Users/eugen/work/connectly-frontend")
        )),
        parse_working_directory(
            b"file://Eugens-MacBook-Pro.local/Users/eugen/work/connectly-frontend"
        )
    );
    assert_eq!(
        Some((Some("omarchy".to_owned()), PathBuf::from("/tmp/weird%dir"))),
        parse_working_directory(b"file://omarchy/tmp/weird%25dir")
    );
    // `;` is escaped to `%3B` by the snippet, because a raw `;` is where the
    // terminal splits the OSC into fields — `params[1]` would have stopped at
    // it — and it decodes back here. See the semicolon snippet test below.
    assert_eq!(
        Some((Some("omarchy".to_owned()), PathBuf::from("/tmp/a;b"))),
        parse_working_directory(b"file://omarchy/tmp/a%3Bb")
    );
    // What a snippet older than the host wrote, and what a shell whose
    // hostname is empty still writes: no host, which is this machine.
    assert_eq!(
        Some((None, PathBuf::from("/tmp"))),
        parse_working_directory(b"file:///tmp")
    );
}

#[test]
fn a_hostile_working_directory_url_is_refused_rather_than_believed() {
    // OSC 7 is whatever the program in the pane chose to write, so the parser
    // is the seam between a stranger's bytes and a path Crook trusts.

    // A percent escape that is not two hex digits decodes to nothing, so the
    // whole update is dropped rather than a corrupt path believed.
    assert_eq!(None, parse_working_directory(b"/a%zzb"));
    assert_eq!(None, parse_working_directory(b"/a%2Gb"));
    // Nor is a signed number, which the integer parser underneath takes.
    assert_eq!(None, parse_working_directory(b"/a%+1b"));

    // What is not a path something could be in is not one: another scheme's
    // URL, or a word, used to come through as a relative path, resolved
    // against Crook's own directory by everything that read it.
    assert_eq!(None, parse_working_directory(b"kitty-shell-cwd://host/tmp"));
    assert_eq!(None, parse_working_directory(b"foo"));

    // The host is printed on the pane's row, so it is held to what a
    // machine's name can be: no control character — an ESC there would be
    // drawn as one — no space, and no longer than any name a resolver takes.
    assert_eq!(None, parse_working_directory(b"file://evil\x1b[2Jbox/tmp"));
    assert_eq!(None, parse_working_directory(b"file://two words/tmp"));
    let long = format!("file://{}/tmp", "h".repeat(HOST_BYTES + 1));
    assert_eq!(None, parse_working_directory(long.as_bytes()));
    let longest = format!("file://{}/tmp", "h".repeat(HOST_BYTES));
    assert_eq!(
        Some((Some("h".repeat(HOST_BYTES)), PathBuf::from("/tmp"))),
        parse_working_directory(longest.as_bytes())
    );

    // Escapes that decode to bytes that are not UTF-8 are refused the same way
    // — a lone continuation byte is no path.
    assert_eq!(None, parse_working_directory(b"/%ff"));
    assert_eq!(None, parse_working_directory(b"/%c3%28"));

    // An escape with too few digits to be one is left as itself rather than
    // eaten: a trailing `%` or `%2` is a literal, not the start of a byte.
    assert_eq!(
        Some((None, PathBuf::from("/a%"))),
        parse_working_directory(b"/a%")
    );
    assert_eq!(
        Some((None, PathBuf::from("/a%2"))),
        parse_working_directory(b"/a%2")
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

/// Every integration says which machine its directory is on, by the shell's
/// own copy of the machine's name.
///
/// The bug this pins: with the authority left empty, a shell on the far side
/// of an ssh — where the README says to paste these — reported a path that
/// Crook read as one on this machine, and the pane showed this machine's
/// checkout at that path. Each shell keeps the name in a variable it read
/// once at startup, so naming it costs a prompt nothing.
#[test]
fn test_every_shell_integration_names_the_machine_its_directory_is_on() {
    for (shell, snippet, host) in [
        (
            "zsh",
            include_str!("../../../app/src/shell_integration/crook.zsh"),
            "\"${HOST-}\"",
        ),
        (
            "bash",
            include_str!("../../../app/src/shell_integration/crook.bash"),
            "\"${HOSTNAME-}\"",
        ),
        (
            "fish",
            include_str!("../../../app/src/shell_integration/crook.fish"),
            "\"$hostname\"",
        ),
    ] {
        let line = snippet
            .lines()
            .find(|line| line.contains("\\e]7;file://"))
            .unwrap_or_else(|| panic!("the {shell} integration sends no OSC 7"));
        assert!(
            line.contains("\\e]7;file://%s%s") && line.contains(host),
            "the {shell} integration does not put {host} in its OSC 7's \
             authority, so a shell on another machine reads as one on this: {line}"
        );
    }
}

/// Every integration escapes a `;` in the path it reports.
///
/// A `;` is where the terminal splits an OSC string into fields, and the OSC 7
/// reader takes the second field only. A working directory holding one — a
/// legal byte on every Unix filesystem, `mkdir 'a;b'` — would be reported cut
/// off at it: the tab's directory, its git chips and any pane that inherits
/// the cwd would all follow the wrong path. So each snippet must send the `;`
/// as `%3B`, which the reader decodes back (see `test_working_directory_urls_are_parsed`).
#[test]
fn test_every_shell_integration_escapes_a_semicolon_in_the_cwd() {
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
            snippet.contains("%3B"),
            "the {shell} integration does not escape `;` in the working \
             directory, so a path holding one is reported truncated at it"
        );
    }
}

/// Every integration speaks the completion channel Crook parses, both ways.
///
/// Completion is two numbers in a Rust constant and a shell snippet that no
/// compiler reconciles: the key Crook sends to ask ([`crate::COMPLETION_REQUEST`],
/// which the snippet must bind) and the OSC the shell replies on
/// (`COMPLETIONS_OSC`, which the snippet must emit). A snippet that bound a
/// different key, or replied on a different number, would leave Tab doing
/// nothing with nothing to say why — the same silent break the OSC 7 test
/// above pins, for the channel most likely to drift.
#[test]
fn test_every_shell_integration_speaks_the_completion_channel() {
    let reply = std::str::from_utf8(COMPLETIONS_OSC).expect("the OSC number is ASCII");
    // `COMPLETION_REQUEST` is the ESC byte and then the bindable tail; the
    // snippets write the ESC as the two characters `\e`, so the tail is what
    // shows up in them verbatim.
    let request = std::str::from_utf8(&crate::COMPLETION_REQUEST[1..]).expect("the tail is ASCII");
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
            snippet.contains(&format!("]{reply};")),
            "the {shell} integration does not reply on the OSC Crook reads completions from"
        );
        assert!(
            snippet.contains(request),
            "the {shell} integration does not bind the key Crook sends to ask for a completion"
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
            message: None,
        })],
        emulator.take_events()
    );
}

#[test]
fn test_a_program_says_what_it_is_waiting_for() {
    // The message rides after the `;;` cut, and it is news even when the
    // status is not: the agent is asking a different question.
    let mut emulator = emulator();
    emulator.advance(b"\x1b]6340;needs-input;port the tab bar;;run rm -rf build?\x07");
    assert_eq!(
        vec![TerminalEvent::Agent(Reported {
            status: AgentReport::NeedsInput,
            title: Some("port the tab bar".to_owned()),
            message: Some("run rm -rf build?".to_owned()),
        })],
        emulator.take_events()
    );

    emulator.advance(b"\x1b]6340;needs-input;;overwrite main.rs?\x07");
    assert_eq!(
        vec![TerminalEvent::Agent(Reported {
            status: AgentReport::NeedsInput,
            title: None,
            message: Some("overwrite main.rs?".to_owned()),
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
fn test_a_program_says_which_pull_request_its_work_is() {
    // Beside a status, in the same read, and each reported on its own: the
    // pull request is not a status and changes none.
    let mut emulator = emulator();
    emulator.advance(
        b"\x1b]6340;running\x07\x1b]6342;pr;https://github.com/theguriev/crook/pull/398\x07",
    );

    assert_eq!(AgentReport::Running, emulator.agent());
    let events = emulator.take_events();
    assert!(
        events.contains(&TerminalEvent::PullRequest(
            "https://github.com/theguriev/crook/pull/398".to_owned()
        )),
        "{events:?}"
    );

    // Said again, it is said again: the row may have dropped it since — the
    // pane changed branch — and an agent repeating it is putting it back.
    emulator.advance(b"\x1b]6342;pr;https://github.com/theguriev/crook/pull/398\x07");
    assert_eq!(1, emulator.take_events().len());
}

#[test]
fn test_a_pull_request_survives_a_split_and_a_refused_one_says_nothing() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b]6342;pr;https://github.com/o/r/pu");
    emulator.advance(b"ll/7\x07");
    assert_eq!(
        vec![TerminalEvent::PullRequest(
            "https://github.com/o/r/pull/7".to_owned()
        )],
        emulator.take_events()
    );

    // Through the real parser, which drops what is past its sixteenth piece:
    // an address long enough in `;` to have been cut is not read as the part
    // of it that arrived.
    let cut = format!("\x1b]6342;pr;https://example.com/p{}\x07", ";x".repeat(20));
    emulator.advance(cut.as_bytes());
    emulator.advance(b"\x1b]6342;pr;http://github.com/o/r/pull/7\x07");
    assert!(emulator.take_events().is_empty());
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
            .contains(&TerminalEvent::AgentSettled)
    );
}

#[test]
fn test_a_running_status_settles_when_its_end_is_in_the_same_read() {
    // The command ending takes a running status with it whether the `running`
    // report and its terminating `D` land in one read or two: the emulator
    // settles on the bytes, not on where a pty happened to split them. The
    // two-read form is the test above; the parser only stops on a mark, so a
    // report in the same piece always precedes it.
    for report in [
        b"\x1b]6340;running\x07".as_slice(),
        b"\x1b]6340;needs-input\x07",
    ] {
        let mut emulator = emulator();
        emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07claude\r\n\x1b]133;C\x07");
        emulator.take_events();

        let mut one_read = report.to_vec();
        one_read.extend_from_slice(b"\x1b]133;D;130\x07");
        emulator.advance(&one_read);

        assert_eq!(
            AgentReport::Idle,
            emulator.agent(),
            "a {report:?} report and its `D` in one read left the status stuck"
        );
        assert!(
            emulator
                .take_events()
                .contains(&TerminalEvent::AgentSettled)
        );
    }
}

#[test]
fn test_a_finished_command_is_reported_with_its_own_command_line() {
    // What ended, not only how: a command that starts and ends between two
    // snapshots is never seen running, and this is the one place that still
    // names it.
    let mut emulator = emulator();
    let finished = |emulator: &mut Emulator, bytes: &[u8]| {
        emulator.advance(bytes);
        let events = emulator.take_events();
        match &events[..] {
            [TerminalEvent::CommandFinished { command, exit, .. }] => (command.clone(), *exit),
            other => panic!("one command finished: {other:?}"),
        }
    };
    let first = b"\x1b]133;A\x07$ \x1b]133;B\x07true\r\n\x1b]133;C\x07\x1b]133;D;0\x07";
    assert_eq!(
        (Some("true".to_owned()), Some(0)),
        finished(&mut emulator, first)
    );
    // The next names its own, not the one before it.
    let second = b"\x1b]133;A\x07$ \x1b]133;B\x07false\r\n\x1b]133;C\x07\x1b]133;D;1\x07";
    assert_eq!(
        (Some("false".to_owned()), Some(1)),
        finished(&mut emulator, second)
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

#[test]
fn test_a_report_too_long_for_the_parser_says_where_it_was_cut() {
    // `vte` keeps sixteen parameters of an OSC and drops the rest without a
    // word. A message that is a chain of commands waiting for approval ran
    // past that, and the row read as the whole question with its end gone.
    // Written through `report` and read back through the real parser.
    let arrived = |title: Option<&str>, message: Option<&str>| {
        let mut emulator = emulator();
        let sequence = crate::agent::report(AgentReport::NeedsInput, title, message);
        emulator.advance(sequence.as_bytes());
        match emulator.take_events().as_slice() {
            [TerminalEvent::Agent(reported)] => (reported.title.clone(), reported.message.clone()),
            other => panic!("one report should have arrived, got {other:?}"),
        }
    };
    let commands = |count: usize| {
        (1..=count)
            .map(|at| format!("step {at}"))
            .collect::<Vec<_>>()
            .join(";")
    };

    // Exactly as much as there is room for, with a title of one piece:
    // sixteen, less the number, the status, the title and the cut.
    let fits = commands(12);
    assert_eq!(
        (Some("port the tab bar".to_owned()), Some(fits.clone())),
        arrived(Some("port the tab bar"), Some(&fits)),
        "a message that fits was cut"
    );

    // One more than that, and more still: cut at the last `;` that fits, and
    // marked as cut rather than passed off as the whole of it.
    for count in [13, 15, 40] {
        assert_eq!(
            (
                Some("port the tab bar".to_owned()),
                Some(format!("{}\u{2026}", commands(12)))
            ),
            arrived(Some("port the tab bar"), Some(&commands(count))),
            "{count} pieces"
        );
    }

    // A title that would take the message's room leaves it some.
    let (title, message) = arrived(Some(&commands(30)), Some("approve?"));
    assert_eq!(title, Some(format!("{}\u{2026}", commands(12))));
    assert_eq!(message.as_deref(), Some("approve?"));

    // And a title with no message after it has the whole of the room.
    let (title, _) = arrived(Some(&commands(30)), None);
    assert_eq!(title, Some(format!("{}\u{2026}", commands(14))));
}

/// What `bytes` ask a fresh emulator for: the one notification they carry,
/// or `None` when they carry none — and nothing else either.
fn notified(bytes: &[u8]) -> Option<Notification> {
    let mut emulator = emulator();
    emulator.advance(bytes);
    match emulator.take_events().as_slice() {
        [] => None,
        [TerminalEvent::Notification(notification)] => Some(notification.clone()),
        other => panic!("one notification at most, and nothing else: {other:?}"),
    }
}

/// A notification with a body and no title, which is all OSC 9 can carry.
fn saying(body: &str) -> Option<Notification> {
    Some(Notification {
        title: None,
        body: Some(body.to_owned()),
    })
}

fn titled(title: &str, body: Option<&str>) -> Option<Notification> {
    Some(Notification {
        title: Some(title.to_owned()),
        body: body.map(str::to_owned),
    })
}

#[test]
fn test_osc_nine_asks_for_a_look_with_its_message() {
    // iTerm2's form, which Claude Code writes with its notification channel
    // set to `iterm2`. The BEL is the sequence's terminator, not a bell.
    assert_eq!(
        saying("Claude needs your permission to use Bash"),
        notified(b"\x1b]9;Claude needs your permission to use Bash\x07")
    );
    assert_eq!(saying("build done"), notified(b"\x1b]9;build done\x1b\\"));
    assert_eq!(
        None,
        notified(b"\x1b]9;\x07"),
        "an empty message says nothing"
    );
    assert_eq!(None, notified(b"\x1b]9;   \x07"));
}

#[test]
fn test_a_notification_leaves_the_status_alone() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b]6340;running;port the tab bar\x07");
    emulator.take_events();

    emulator.advance(b"\x1b]9;tests passed\x07");

    assert_eq!(AgentReport::Running, emulator.agent());
    assert_eq!(
        vec![TerminalEvent::Notification(
            saying("tests passed").expect("a notification")
        )],
        emulator.take_events()
    );
}

#[test]
fn test_a_notification_with_semicolons_arrives_whole() {
    // `vte` splits every OSC on `;`, so each of these arrives in pieces.
    assert_eq!(
        saying("cargo test; 3 failed; see the log"),
        notified(b"\x1b]9;cargo test; 3 failed; see the log\x07")
    );
    assert_eq!(
        titled("deploy", Some("staging; then prod?")),
        notified(b"\x1b]777;notify;deploy;staging; then prod?\x07")
    );
    assert_eq!(
        titled("fix a; then b", None),
        notified(b"\x1b]99;;fix a; then b\x1b\\")
    );
}

#[test]
fn test_a_notification_too_long_for_the_parser_says_where_it_was_cut() {
    // `vte` keeps sixteen pieces of an OSC and drops the rest without a
    // word: the number, and fifteen of the message.
    let steps = |count: usize| {
        (1..=count)
            .map(|at| format!("step {at}"))
            .collect::<Vec<_>>()
            .join(";")
    };
    let osc_nine = |message: &str| notified(format!("\x1b]9;{message}\x07").as_bytes());

    assert_eq!(saying(&steps(14)), osc_nine(&steps(14)));
    // Fifteen fill the parser exactly, and nothing says whether more were
    // dropped, so it is marked as possibly cut rather than passed off as
    // whole.
    for count in [15, 16, 40] {
        assert_eq!(
            saying(&format!("{}\u{2026}", steps(15))),
            osc_nine(&steps(count)),
            "{count} pieces"
        );
    }
    // The same for the other two, which spend pieces on fields of their own.
    assert_eq!(
        titled("deploy", Some(&format!("{}\u{2026}", steps(13)))),
        notified(format!("\x1b]777;notify;deploy;{}\x07", steps(30)).as_bytes())
    );
    assert_eq!(
        saying(&format!("{}\u{2026}", steps(14))),
        notified(format!("\x1b]99;p=body;{}\x1b\\", steps(30)).as_bytes())
    );
}

#[test]
fn test_conemus_commands_on_osc_nine_are_not_notifications() {
    for sequence in [
        &b"\x1b]9;4;1;50\x07"[..],
        b"\x1b]9;4;0\x07",
        b"\x1b]9;4;3\x1b\\",
        b"\x1b]9;9;/home/me\x07",
        b"\x1b]9;1;500\x07",
        b"\x1b]9;5\x07",
        b"\x1b]9;12\x07",
    ] {
        assert_eq!(
            None,
            notified(sequence),
            "{}",
            String::from_utf8_lossy(sequence).escape_debug()
        );
    }
    // A number ConEmu has no command for starts a message like any word.
    assert_eq!(saying("13;done"), notified(b"\x1b]9;13;done\x07"));
    assert_eq!(
        saying("4 tests failed"),
        notified(b"\x1b]9;4 tests failed\x07")
    );
}

#[test]
fn test_control_characters_are_taken_out_of_a_notification() {
    // `vte` drops C0 inside an OSC itself. DEL and the C1 controls, written
    // as UTF-8, reach the reader — and a C1 CSI is an escape sequence
    // waiting for something to print it.
    assert_eq!(
        saying("one 31m two three"),
        notified("\x1b]9;one\u{9b}31m\u{7f}two\u{85}three\x07".as_bytes())
    );
    assert_eq!(
        titled("a b", Some("c d")),
        notified("\x1b]777;notify;a\u{90}b;c\u{9c}d\x07".as_bytes())
    );
    assert_eq!(
        titled("e f", None),
        notified("\x1b]99;;e\u{7f}f\x1b\\".as_bytes())
    );
    // A text that is nothing but control characters is no text.
    assert_eq!(None, notified("\x1b]9;\u{9b}\u{7f}\x07".as_bytes()));
}

#[test]
fn test_an_over_long_notification_is_cut_to_one_line() {
    let long = "y".repeat(notify::BODY_CHARS + 300);
    let cut = format!("{}\u{2026}", "y".repeat(notify::BODY_CHARS));
    assert_eq!(
        saying(&cut),
        notified(format!("\x1b]9;{long}\x07").as_bytes())
    );

    let title = "t".repeat(notify::TITLE_CHARS + 40);
    assert_eq!(
        titled(
            &format!("{}\u{2026}", "t".repeat(notify::TITLE_CHARS)),
            Some(&cut)
        ),
        notified(format!("\x1b]777;notify;{title};{long}\x07").as_bytes())
    );
    assert_eq!(
        Some(Notification {
            title: None,
            body: Some(cut.clone()),
        }),
        notified(format!("\x1b]99;p=body;{long}\x1b\\").as_bytes())
    );
}

#[test]
fn test_osc_777_notify_carries_a_title_and_a_body() {
    // rxvt-unicode's, which Ghostty reads and Claude Code writes with its
    // channel set to `ghostty`.
    assert_eq!(
        titled(
            "Claude Code",
            Some("Claude needs your permission to use Bash")
        ),
        notified(b"\x1b]777;notify;Claude Code;Claude needs your permission to use Bash\x07")
    );
    assert_eq!(
        titled("build", None),
        notified(b"\x1b]777;notify;build\x07")
    );
    assert_eq!(
        saying("no title"),
        notified(b"\x1b]777;notify;;no title\x07")
    );
    assert_eq!(None, notified(b"\x1b]777;notify;;\x07"));
    assert_eq!(None, notified(b"\x1b]777;notify\x07"));
    // 777 is rxvt's number for all its extensions.
    assert_eq!(None, notified(b"\x1b]777;preexec;ls\x07"));
}

#[test]
fn test_kittys_osc_99_in_one_chunk() {
    // kitty's own first example: no metadata, and the payload is the title.
    assert_eq!(
        titled("Hello world", None),
        notified(b"\x1b]99;;Hello world\x1b\\")
    );
    assert_eq!(
        saying("just a body"),
        notified(b"\x1b]99;p=body;just a body\x1b\\")
    );
    // Keys that do not change the text are passed over, known or not.
    assert_eq!(
        titled("done", None),
        notified(b"\x1b]99;i=7:d=1:a=focus:u=2:zz=9;done\x1b\\")
    );
    assert_eq!(None, notified(b"\x1b]99;;\x1b\\"));
    assert_eq!(None, notified(b"\x1b]99\x1b\\"));
}

#[test]
fn test_kittys_chunks_are_put_back_together() {
    // Exactly what Claude Code writes with its channel set to `kitty`: the
    // title in an unfinished chunk, the body in the chunk that finishes it,
    // and a third, empty, that adds nothing.
    let mut emulator = emulator();
    emulator.advance(b"\x1b]99;i=42:d=0:p=title;Claude Code\x1b\\");
    assert!(
        emulator.take_events().is_empty(),
        "a notification was shown before its last chunk"
    );
    emulator.advance(b"\x1b]99;i=42:p=body;Claude needs your permission to use Bash\x1b\\");
    emulator.advance(b"\x1b]99;i=42:d=1:a=focus;\x1b\\");
    assert_eq!(
        vec![TerminalEvent::Notification(
            titled(
                "Claude Code",
                Some("Claude needs your permission to use Bash")
            )
            .expect("a notification")
        )],
        emulator.take_events()
    );

    // A body sent in two chunks is one body.
    assert_eq!(
        saying("first half, second half"),
        notified(b"\x1b]99;i=a:d=0:p=body;first half, \x1b\\\x1b]99;i=a:p=body;second half\x1b\\")
    );
    // A chunk of another notification starts afresh: the unfinished one is
    // dropped rather than mixed into it.
    assert_eq!(
        titled("second", None),
        notified(b"\x1b]99;i=1:d=0;first\x1b\\\x1b]99;i=2;second\x1b\\")
    );
}

#[test]
fn test_kittys_encoded_payloads_and_other_kinds_are_not_shown() {
    // Base64 is not decoded here, and shown as it arrived it is garbage.
    assert_eq!(None, notified(b"\x1b]99;e=1;SGVsbG8=\x1b\\"));
    assert_eq!(
        titled("build", None),
        notified(b"\x1b]99;i=2:d=0;build\x1b\\\x1b]99;i=2:p=body:e=1;ZG9uZQ==\x1b\\")
    );
    // Closing one, asking whether one is alive, asking what the terminal
    // supports: none of them is a notification, and none of them disturbs
    // one still arriving.
    assert_eq!(None, notified(b"\x1b]99;i=1:p=close;\x1b\\"));
    assert_eq!(None, notified(b"\x1b]99;i=1:p=?;\x1b\\"));
    assert_eq!(
        titled("build", Some("done")),
        notified(
            b"\x1b]99;i=3:d=0;build\x1b\\\x1b]99;i=9:p=alive;\x1b\\\x1b]99;i=3:p=body;done\x1b\\"
        )
    );
    // An icon is part of a notification, and adds no text to it.
    assert_eq!(
        titled("build", None),
        notified(b"\x1b]99;i=4:d=0;build\x1b\\\x1b]99;i=4:p=icon;AAAA\x1b\\")
    );
}

#[test]
fn test_a_notification_survives_a_split_between_two_reads() {
    let mut emulator = emulator();
    emulator.advance(b"\x1b]9;build ");
    emulator.advance(b"done\x07");

    assert_eq!(
        vec![TerminalEvent::Notification(
            saying("build done").expect("a notification")
        )],
        emulator.take_events()
    );
}

#[test]
fn test_the_last_notification_in_a_read_stands_for_the_others() {
    assert_eq!(
        saying("two"),
        notified(b"\x1b]9;one\x07\x1b]777;notify;;two\x07")
    );
}

#[test]
fn test_a_notification_and_a_report_keep_their_order_in_one_read() {
    // The workspace reads the two against each other: `running` takes away
    // the look a notification asked for, a notification after it asks again,
    // and a status change after one takes its place. So one read has to hand
    // them over in the order they were written, as one read per sequence
    // does, or the same bytes settle differently depending on where a pty
    // split them.
    //
    // Two reports with a notification between them are two reports, not the
    // last of them: kept in one slot, `needs-input` and `running` around a
    // notification went out as the `running` alone, which from `running` is
    // no change at all, and a `running` repeated after one went out behind
    // the notification it was written before.
    let running = b"\x1b]6340;running\x07".as_slice();
    let needs_input = b"\x1b]6340;needs-input\x07".as_slice();
    let idle = b"\x1b]6340;idle\x07".as_slice();
    let done = b"\x1b]9;done\x07".as_slice();
    // Ended by ST, the watcher stops on its ESC, and the `\` is the next
    // piece's first byte.
    let done_st = b"\x1b]9;done\x1b\\".as_slice();
    let end = b"\x1b]133;D;0\x07".as_slice();
    let started = |from: &[u8]| {
        let mut emulator = emulator();
        emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07claude\r\n\x1b]133;C\x07");
        emulator.advance(from);
        emulator.take_events();
        emulator
    };
    // How long the command took is timed, and is no part of the order.
    let events = |emulator: &mut Emulator| -> Vec<TerminalEvent> {
        emulator
            .take_events()
            .into_iter()
            .map(|event| match event {
                TerminalEvent::CommandFinished {
                    command, exit, ran, ..
                } => TerminalEvent::CommandFinished {
                    command,
                    exit,
                    took: None,
                    ran,
                },
                other => other,
            })
            .collect()
    };

    for (from, sequences) in [
        (idle, [done, running].as_slice()),
        (idle, &[running, done]),
        (idle, &[done, end]),
        (idle, &[running, done, end]),
        (idle, &[done, running, end]),
        (idle, &[running, end, done]),
        (idle, &[running, done, running]),
        (idle, &[running, done, idle]),
        (idle, &[running, done, running, end]),
        (running, &[idle, done, running]),
        (running, &[needs_input, done, running]),
        (running, &[needs_input, done, done, running]),
        (running, &[done, needs_input, done, running]),
        (running, &[needs_input, done_st, running]),
    ] {
        let mut split = started(from);
        let mut whole = started(from);
        for sequence in sequences {
            split.advance(sequence);
        }
        whole.advance(&sequences.concat());

        assert_eq!(
            events(&mut split),
            events(&mut whole),
            "{:?} in one read, from {:?}",
            sequences
                .iter()
                .map(|sequence| String::from_utf8_lossy(sequence))
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(from),
        );
    }
}
