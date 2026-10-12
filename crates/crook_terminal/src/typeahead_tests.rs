use super::*;

/// The control characters macOS gives a new terminal, as far as the pacing
/// looks at them.
fn characters() -> [u8; NCCS] {
    let mut characters = [DISABLED; NCCS];
    characters[VINTR] = 0x03;
    characters[VQUIT] = 0x1c;
    characters[VSUSP] = 0x1a;
    characters[VSTART] = 0x11;
    characters[VSTOP] = 0x13;
    characters[VLNEXT] = 0x16;
    characters[VSTATUS] = 0x14;
    characters
}

/// How a shell leaves the terminal for the command it runs.
fn cooked() -> Modes {
    Modes::new(IXON, ISIG | ICANON | IEXTEN, characters())
}

/// How a line editor reads a key at a time, still stopped by Ctrl-C.
fn raw() -> Modes {
    Modes::new(IXON, ISIG | IEXTEN, characters())
}

/// How an editor that takes every key as a key reads: no signals, no flow
/// control, nothing quoted.
fn verbatim() -> Modes {
    Modes::new(0, 0, characters())
}

/// `count` lines of `width` bytes each, the line end included.
fn lines(count: usize, width: usize) -> Vec<u8> {
    let mut lines = Vec::new();
    for _ in 0..count {
        lines.extend(std::iter::repeat_n(b'y', width - 1));
        lines.push(b'\n');
    }
    lines
}

/// Asks `pacing` about `bytes`, and when it says to write some, says they
/// were written.
fn send<'a>(pacing: &mut Pacing, bytes: &'a [u8], unread: usize, modes: &Modes) -> Next<'a> {
    let next = pacing.next(bytes, unread, modes);
    if let Next::Write(written) = next {
        pacing.wrote(written);
    }
    next
}

#[test]
fn test_a_piece_ends_at_the_last_line_end_that_fits() {
    // Two lines and the start of a third fit: the third is left for the next
    // piece, so that no line is left half written in the queue.
    let bytes = b"one\ntwo\r\nthree four five";
    assert_eq!(b"one\ntwo\r\n", piece(bytes, 12));
}

#[test]
fn test_a_piece_of_a_line_longer_than_the_room_is_cut_at_the_room() {
    assert_eq!(b"abcd", piece(b"abcdefgh\n", 4));
}

#[test]
fn test_a_piece_that_is_everything_left_is_written_whole() {
    // Nothing follows it to keep apart from it, so a line it leaves unfinished
    // is the one the writer was given.
    assert_eq!(b"one\ntwo", piece(b"one\ntwo", ROOM));
}

#[test]
fn test_keys_typed_behind_a_line_the_program_has_not_read_go_at_once() {
    // A command is running and reads nothing; the person types the next one
    // and presses Enter twice. Holding the second Enter until the command
    // reads would hold every key behind it too, the Ctrl-C that would stop
    // it included.
    let mut pacing = Pacing::default();
    assert_eq!(
        Next::Write(b"ls\r"),
        send(&mut pacing, b"ls\r", 0, &cooked())
    );
    assert_eq!(Next::Write(b"\r"), send(&mut pacing, b"\r", 3, &cooked()));
    assert_eq!(Next::Write(b"pwd"), send(&mut pacing, b"pwd", 4, &cooked()));
}

#[test]
fn test_nothing_written_behind_unread_input_reaches_past_the_room() {
    // One key at a time into a program that reads none of them: from the
    // last time nothing was unread, a queue's worth and no more.
    let mut pacing = Pacing::default();
    assert_eq!(Next::Write(b"x"), send(&mut pacing, b"x", 0, &raw()));
    let mut written = 1;
    while send(&mut pacing, b"x", written, &raw()) == Next::Write(b"x") {
        written += 1;
    }
    assert_eq!(ROOM, written);
    assert_eq!(Next::Wait, send(&mut pacing, b"x", written, &raw()));

    // Once it has read it all, the count starts again.
    assert_eq!(Next::Write(b"x"), send(&mut pacing, b"x", 0, &raw()));
}

#[test]
fn test_a_paste_that_does_not_fit_behind_unread_input_waits_for_the_queue_to_empty() {
    // Not a piece cut to fit behind it: pieces go into an empty queue.
    let mut pacing = Pacing::default();
    let first = lines(10, 90);
    assert_eq!(
        Next::Write(&first[..]),
        send(&mut pacing, &first, 0, &raw())
    );
    let second = lines(2, 90);
    assert_eq!(Next::Wait, send(&mut pacing, &second, 600, &raw()));
    assert_eq!(Next::Wait, send(&mut pacing, &second, 1, &raw()));
    assert_eq!(
        Next::Write(&second[..]),
        send(&mut pacing, &second, 0, &raw())
    );
}

#[test]
fn test_a_long_paste_goes_a_queue_at_a_time_cut_at_line_ends() {
    let mut pacing = Pacing::default();
    let paste = lines(100, 90);
    let Next::Write(first) = send(&mut pacing, &paste, 0, &raw()) else {
        panic!("a paste into an empty queue waited");
    };
    assert_eq!(11 * 90, first.len(), "not the most whole lines that fit");
    let rest = &paste[first.len()..];
    assert_eq!(Next::Wait, send(&mut pacing, rest, 300, &raw()));
    let Next::Write(second) = send(&mut pacing, rest, 0, &raw()) else {
        panic!("the rest of a paste waited on an empty queue");
    };
    assert_eq!(11 * 90, second.len());
}

#[test]
fn test_in_canonical_mode_the_count_starts_from_the_line_left_unfinished() {
    // The count of unread input leaves that line out, and a shell back in raw
    // mode can read part of it before the next piece lands behind it.
    let paste = lines(20, 100);
    for (modes, room) in [(cooked(), ROOM - 300), (raw(), ROOM)] {
        let mut pacing = Pacing::default();
        let unfinished = [b'z'; 300];
        assert_eq!(
            Next::Write(&unfinished[..]),
            send(&mut pacing, &unfinished, 0, &modes)
        );
        let Next::Write(piece) = send(&mut pacing, &paste, 0, &modes) else {
            panic!("a paste into an empty queue waited");
        };
        assert_eq!(room / 100 * 100, piece.len(), "{modes:?}");
    }
}

#[test]
fn test_a_line_longer_than_the_queue_is_held_a_while_in_canonical_mode_and_then_given_up() {
    let mut pacing = Pacing::default();
    let line = [b'z'; 1500];
    assert_eq!(
        Next::Write(&line[..ROOM]),
        send(&mut pacing, &line, 0, &cooked())
    );
    // A shell back in raw mode would read it; anything written before then
    // would be thrown away.
    let rest = &line[ROOM..];
    assert_eq!(
        Next::WaitAtMost(rest),
        send(&mut pacing, rest, 0, &cooked())
    );

    // Not read: the kernel cuts it, and what is typed after it goes at once —
    // the Ctrl-U that clears it, and the line after that.
    pacing.give_up();
    pacing.wrote(rest);
    assert_eq!(
        Next::Write(b"\x15"),
        send(&mut pacing, b"\x15", 0, &cooked())
    );
    assert_eq!(
        Next::Write(b"abc\r"),
        send(&mut pacing, b"abc\r", 0, &cooked())
    );

    // The line end ended the giving up: the next long line is held again.
    assert_eq!(
        Next::Write(&line[..ROOM]),
        send(&mut pacing, &line, 0, &cooked())
    );
    assert_eq!(
        Next::WaitAtMost(rest),
        send(&mut pacing, rest, 0, &cooked())
    );
}

#[test]
fn test_in_raw_mode_a_line_longer_than_the_queue_is_not_held() {
    // Raw mode has no line limit, and the count leaves nothing out.
    let mut pacing = Pacing::default();
    let line = [b'z'; 1500];
    assert_eq!(
        Next::Write(&line[..ROOM]),
        send(&mut pacing, &line, 0, &raw())
    );
    let rest = &line[ROOM..];
    assert_eq!(Next::Write(rest), send(&mut pacing, rest, 0, &raw()));
}

#[test]
fn test_an_interrupt_goes_at_once_and_alone_whatever_is_unread() {
    // A program that has stopped reading is when input is held, and Ctrl-C,
    // Ctrl-\ and Ctrl-Z are how a person stops it. The kernel acts on them
    // and queues nothing, so they cannot wrap a ring either.
    for modes in [cooked(), raw()] {
        let mut pacing = Pacing::default();
        let filler = [b'x'; ROOM];
        send(&mut pacing, &filler, 0, &modes);
        assert_eq!(Next::Wait, send(&mut pacing, b"x", ROOM, &modes));
        for key in [b"\x03", b"\x1c", b"\x1a"] {
            assert_eq!(
                Next::Write(key),
                pacing.next(key, ROOM, &modes),
                "{key:?} in {modes:?}"
            );
        }
        assert_eq!(Next::Write(b"\x03"), pacing.next(b"\x03and", ROOM, &modes));
    }
}

#[test]
fn test_a_write_stops_short_of_a_byte_the_line_discipline_acts_on() {
    // So that it leads a write of its own, and goes at once.
    let mut pacing = Pacing::default();
    assert_eq!(
        Next::Write(b"abc"),
        send(&mut pacing, b"abc\x03def", 0, &cooked())
    );
    assert_eq!(
        Next::Write(b"\x03"),
        send(&mut pacing, b"\x03def", 0, &cooked())
    );
    assert_eq!(Next::Write(b"def"), send(&mut pacing, b"def", 0, &cooked()));

    let paste = lines(30, 100);
    let mut with_stop = paste.clone();
    with_stop.insert(150, 0x13);
    let Next::Write(first) = send(&mut pacing, &with_stop, 0, &cooked()) else {
        panic!("a paste into an empty queue waited");
    };
    assert_eq!(&paste[..150], first);
}

#[test]
fn test_start_stop_and_status_are_acted_on_only_in_the_modes_that_act_on_them() {
    assert!(cooked().acts_on(0x11) && cooked().acts_on(0x13));
    assert!(!Modes::new(0, ISIG | ICANON | IEXTEN, characters()).acts_on(0x13));
    assert!(cooked().acts_on(0x14));
    assert!(
        !raw().acts_on(0x14),
        "the status key is only canonical mode's"
    );
    assert!(!Modes::new(IXON, ICANON, characters()).acts_on(0x14));
}

#[test]
fn test_with_signals_off_an_interrupt_is_an_ordinary_key() {
    // An editor that reads Ctrl-C as a key gets it in its place, behind what
    // it has not read.
    let mut pacing = Pacing::default();
    let filler = [b'x'; ROOM];
    send(&mut pacing, &filler, 0, &verbatim());
    for key in [b"\x03", b"\x11", b"\x13", b"\x16"] {
        assert_eq!(Next::Wait, pacing.next(key, ROOM, &verbatim()), "{key:?}");
    }
    assert_eq!(None, verbatim().first_acted_on(b"a\x03\x1a\x1c", false));
}

#[test]
fn test_a_quoted_interrupt_is_input_like_any_other() {
    // Ctrl-V Ctrl-C puts a ^C into the line rather than interrupting.
    assert_eq!(None, cooked().first_acted_on(b"\x16\x03", false));
    assert_eq!(Some(2), cooked().first_acted_on(b"\x16\x16\x03", false));
    assert_eq!(None, cooked().first_acted_on(b"\x03", true));

    // Quoted across two writes, and nothing thrown away: the line it is in
    // still counts in full.
    let mut pacing = Pacing::default();
    let unfinished = [b'z'; 500];
    send(&mut pacing, &unfinished, 0, &cooked());
    assert_eq!(
        Next::Write(b"\x16"),
        send(&mut pacing, b"\x16", 0, &cooked())
    );
    assert_eq!(
        Next::Write(b"\x03"),
        send(&mut pacing, b"\x03", 0, &cooked())
    );
    let paste = lines(20, 100);
    let Next::Write(piece) = send(&mut pacing, &paste, 0, &cooked()) else {
        panic!("a paste into an empty queue waited");
    };
    assert_eq!(500, piece.len());
}

#[test]
fn test_an_interrupt_throws_the_unfinished_line_away_with_its_count() {
    // The kernel flushes what the program has not read, so what follows has
    // the whole queue again — unless the terminal says not to flush.
    let paste = lines(20, 100);
    for (local, room) in [(0, ROOM), (NOFLSH, ROOM - 800)] {
        let modes = Modes::new(IXON, ISIG | ICANON | IEXTEN | local, characters());
        let mut pacing = Pacing::default();
        let unfinished = [b'z'; 800];
        send(&mut pacing, &unfinished, 0, &modes);
        assert_eq!(Next::Write(b"\x03"), send(&mut pacing, b"\x03", 0, &modes));
        let Next::Write(piece) = send(&mut pacing, &paste, 0, &modes) else {
            panic!("a paste into an empty queue waited");
        };
        assert_eq!(room / 100 * 100, piece.len(), "flags {local:#x}");
    }
}

#[test]
fn test_modes_nobody_could_read_act_on_nothing_and_count_as_canonical() {
    let unknown = Modes::default();
    assert!(unknown.canonical());
    assert_eq!(
        None,
        unknown.first_acted_on(b"\x03\x1a\x1c\x11\x13\x14\xff", false)
    );
}

#[test]
fn test_a_long_paste_read_a_line_at_a_time_goes_through_whole_and_quickly() {
    // `cat` in canonical mode reads a line at a time, and the writer asks
    // again after every read: fourteen thousand times for a megabyte. Looking
    // through all that was left of the paste each time made that quadratic,
    // and a megabyte pasted into `cat` on macOS took minutes. A third of one
    // here: a fraction of a second paced, and over twenty seconds looked
    // through whole.
    const WIDTH: usize = 75;
    let paste = lines(4_000, WIDTH);
    let mut pacing = Pacing::default();
    let mut sent = 0;
    let mut unread = 0;
    let started = std::time::Instant::now();
    while sent < paste.len() {
        match send(&mut pacing, &paste[sent..], unread, &cooked()) {
            Next::Write(written) => {
                assert_eq!(0, written.len() % WIDTH, "a piece ended inside a line");
                sent += written.len();
                unread += written.len();
            }
            Next::Wait => {
                assert!(unread > 0, "waiting on a program with nothing to read");
                unread -= WIDTH;
            }
            Next::WaitAtMost(_) => panic!("no line here is longer than the queue"),
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "pacing {} bytes took {:?}",
        paste.len(),
        started.elapsed()
    );
}
