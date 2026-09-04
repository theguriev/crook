use std::ffi::OsString;
use std::io::Read as _;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

/// How long a shell that should answer immediately is given before the test
/// calls it a failure rather than hanging the suite.
const DEADLINE: Duration = Duration::from_secs(30);

/// A shell that exists on this machine, or `None` when there is none — a build
/// container with nothing installed is not a failing test.
fn usable_shell() -> Option<OsString> {
    let shell = default_shell();
    if Path::new(&shell).exists() {
        return Some(shell);
    }
    let fallback = OsString::from("/bin/sh");
    Path::new(&fallback).exists().then_some(fallback)
}

/// A program that runs one line of shell and exits.
fn shell_command(line: &str) -> Option<Program> {
    if cfg!(windows) {
        return Some(Program::command(
            "cmd.exe",
            ["/C".to_owned(), line.to_owned()],
        ));
    }
    Some(Program::command(
        usable_shell()?,
        ["-c".to_owned(), line.to_owned()],
    ))
}

/// Moves the pty's reader onto a thread, since reading blocks until the child
/// speaks, and posts what it reads back over a channel.
fn read_on_a_thread(mut reader: PtyReader) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        // A closed pty is end-of-file on some platforms and an error on others;
        // both mean the child is gone and there is nothing left to read.
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                return;
            }
        }
    });
    receiver
}

/// Feeds the terminal whatever the child says until `wanted` shows up on the
/// grid. Returns whether it ever did.
fn feed_until(terminal: &mut Terminal, output: &Receiver<Vec<u8>>, wanted: &str) -> bool {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let Ok(chunk) = output.recv_timeout(remaining) else {
            return false;
        };
        terminal
            .feed(&chunk)
            .expect("feeding the emulator should not fail");
        if terminal.snapshot().text().contains(wanted) {
            return true;
        }
    }
}

/// Waits for the child to finish, rather than hanging if it never does.
fn wait_for_exit(terminal: &mut Terminal) -> ChildExit {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(exit) = terminal
            .try_wait()
            .expect("checking on the child should work")
        {
            return exit;
        }
        assert!(
            Instant::now() < deadline,
            "the child should have exited by now"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn test_a_real_shell_runs_echo_and_the_output_reaches_the_grid() {
    let Some(program) = shell_command("echo crook-terminal-works") else {
        return;
    };
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        size: TerminalSize::new(40, 6),
        ..TerminalOptions::default()
    })
    .expect("a shell should start on a pty");

    let reader = terminal
        .take_reader()
        .expect("a fresh terminal has its reader");
    assert!(
        terminal.take_reader().is_none(),
        "the reader is handed out exactly once"
    );
    let output = read_on_a_thread(reader);

    assert!(
        feed_until(&mut terminal, &output, "crook-terminal-works"),
        "the shell's output should reach the grid, got: {:?}",
        terminal.snapshot().text()
    );

    let exit = wait_for_exit(&mut terminal);
    assert!(exit.success(), "echo should succeed, got {exit}");
    assert!(terminal.has_exited());
    assert!(
        terminal
            .take_events()
            .iter()
            .any(|event| matches!(event, TerminalEvent::ChildExited(_))),
        "the application should be told the child finished"
    );
}

#[test]
fn test_the_child_is_started_at_the_size_it_was_asked_for() {
    // `stty` reads the size straight out of the kernel, so this proves the
    // grid's dimensions reached the pty and not just the emulator.
    if cfg!(windows) {
        return;
    }
    let Some(program) = shell_command("stty size") else {
        return;
    };
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        size: TerminalSize::new(100, 30),
        ..TerminalOptions::default()
    })
    .expect("a shell should start on a pty");
    let output = read_on_a_thread(terminal.take_reader().expect("a reader"));

    assert!(
        feed_until(&mut terminal, &output, "30 100"),
        "the child should see a 100x30 terminal, got: {:?}",
        terminal.snapshot().text()
    );
}

#[test]
fn test_typing_reaches_the_child() {
    // `cat` sends back what it is given, so seeing it on the grid proves the
    // writer reached the far end of the pty.
    let Some(program) = shell_command("cat") else {
        return;
    };
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        size: TerminalSize::new(40, 6),
        ..TerminalOptions::default()
    })
    .expect("a shell should start on a pty");
    let output = read_on_a_thread(terminal.take_reader().expect("a reader"));

    for character in "ping".chars() {
        assert!(
            terminal
                .send_key(Key::Char(character), Modifiers::NONE)
                .expect("a write")
        );
    }
    terminal
        .send_key(Key::Enter, Modifiers::NONE)
        .expect("a write");

    assert!(
        feed_until(&mut terminal, &output, "ping"),
        "what was typed should come back, got: {:?}",
        terminal.snapshot().text()
    );

    terminal.shutdown().expect("the child should be endable");
    assert!(terminal.has_exited());
}

#[test]
fn test_resizing_changes_the_grid_and_the_pty() {
    let Some(program) = shell_command("cat") else {
        return;
    };
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        size: TerminalSize::new(40, 6),
        ..TerminalOptions::default()
    })
    .expect("a shell should start on a pty");

    terminal
        .resize(TerminalSize::new(90, 20).with_cell_size(8, 17))
        .expect("a resize");

    assert_eq!(
        TerminalSize::new(90, 20).with_cell_size(8, 17),
        terminal.size()
    );
    let snapshot = terminal.snapshot();
    assert_eq!((90, 20), (snapshot.columns, snapshot.rows));

    terminal.shutdown().expect("the child should be endable");
}

#[test]
fn test_a_key_with_no_encoding_writes_nothing() {
    let Some(program) = shell_command("cat") else {
        return;
    };
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        ..TerminalOptions::default()
    })
    .expect("a shell should start on a pty");

    assert!(
        !terminal
            .send_key(Key::Function(99), Modifiers::NONE)
            .expect("a write")
    );

    terminal.shutdown().expect("the child should be endable");
}

#[test]
fn test_a_paste_cannot_end_its_own_bracket() {
    // Pasted text is untrusted: a snippet copied from a web page or a chat log
    // can contain the end marker, and a terminal that passed it through would
    // leave paste mode there and run whatever followed with nobody pressing
    // Enter.
    let Some(program) = shell_command("cat") else {
        return;
    };
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        ..TerminalOptions::default()
    })
    .expect("a shell should start on a pty");
    let output = read_on_a_thread(
        terminal
            .take_reader()
            .expect("a fresh terminal has its reader"),
    );

    terminal
        .feed(b"\x1b[?2004h")
        .expect("the mode should reach the emulator");
    terminal
        .paste("safe\x1b[201~; echo PWNED\n")
        .expect("the paste should reach the child");

    // `cat` echoes what it is given, so what comes back is what the shell on
    // the far end would have parsed.
    assert!(feed_until(&mut terminal, &output, "echo PWNED"));
    let echoed = terminal.snapshot().text();
    assert!(
        !echoed.contains('\u{1b}'),
        "the escape that would end the bracket was passed through: {echoed:?}"
    );

    terminal.shutdown().expect("the child should be endable");
}

#[test]
fn test_a_child_that_ignores_the_hangup_is_still_ended() {
    // `portable-pty`'s detachable killer sends `SIGHUP` and reports success.
    // A shell whose dotfiles trap it — or anything run under a wrapper that
    // does — survives that, and every caller that then waits for it waits for
    // ever, on whichever thread asked.
    if cfg!(windows) {
        return;
    }
    let Some(program) = shell_command("trap '' HUP; sleep 120") else {
        return;
    };
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        ..TerminalOptions::default()
    })
    .expect("a shell should start on a pty");

    // Long enough for the shell to have installed the trap.
    thread::sleep(Duration::from_millis(300));

    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let started = Instant::now();
        let exit = terminal.shutdown();
        let _ = done.send((started.elapsed(), exit.is_ok()));
    });

    let (took, ended) = finished
        .recv_timeout(DEADLINE)
        .expect("shutting a HUP-ignoring child down must return, not block for ever");
    assert!(ended, "the child should have been ended");
    assert!(
        took < DEADLINE,
        "shutting down took {took:?}, which is longer than the escalation should need"
    );
}

#[test]
fn test_a_real_child_is_told_which_terminal_it_is_on() {
    if cfg!(windows) {
        return;
    }
    let Some(program) = shell_command("printf '[%s][%s]\\n' \"$TERM\" \"$COLORTERM\"") else {
        return;
    };
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        size: TerminalSize::new(60, 6),
        ..TerminalOptions::default()
    })
    .expect("a shell should start on a pty");
    let output = read_on_a_thread(terminal.take_reader().expect("a reader"));

    assert!(
        feed_until(&mut terminal, &output, "[xterm-256color][truecolor]"),
        "the two variables that say what the grid can do have to reach the \
         child itself, not just the builder, got: {:?}",
        terminal.snapshot().text()
    );
}

#[test]
fn test_a_child_with_no_working_directory_starts_where_a_new_window_would() {
    if cfg!(windows) {
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let Some(program) = shell_command("pwd") else {
        return;
    };
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        size: TerminalSize::new(120, 6),
        ..TerminalOptions::default()
    })
    .expect("a shell should start on a pty");
    let output = read_on_a_thread(terminal.take_reader().expect("a reader"));

    // Not the directory Crook itself was started in, which is whatever the
    // Finder or a launcher happened to leave: a new window opens at home.
    assert!(
        feed_until(&mut terminal, &output, &home.to_string_lossy()),
        "a pane with no directory of its own should open at $HOME, got: {:?}",
        terminal.snapshot().text()
    );
}
