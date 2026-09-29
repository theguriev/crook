use std::ffi::OsString;
use std::io::Read as _;
use std::sync::Mutex;
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
    //
    // Not on Windows, whose console re-renders what the child writes rather
    // than echoing it: the bytes themselves are checked for every platform
    // in `tests/adversarial_review.rs`, and this is the whole path — the mode
    // set, `Terminal::paste`, the pty — where a line discipline echoes.
    if cfg!(windows) {
        return;
    }
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

    // The pty echoes what the child was sent, with an escape spelled `^[` —
    // which is how it can be seen at all: the grid never holds an ESC, since
    // the parser takes every one, so asking the grid for one could not fail.
    // What was sent ends its bracket once, after everything pasted; an end
    // marker that got through would be a second one, before `echo PWNED`.
    assert!(feed_until(&mut terminal, &output, "^[[201~"));
    let echoed = terminal.snapshot().text();
    assert_eq!(
        echoed.matches("^[[201~").count(),
        1,
        "the escape that would end the bracket was passed through: {echoed:?}"
    );
    assert!(
        echoed.find("echo PWNED") < echoed.find("^[[201~"),
        "what was pasted after the marker fell outside the bracket: {echoed:?}"
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

/// A line of shell that prints the directory it started in, bracketed.
///
/// Bracketed because the check is otherwise a substring, and the directory
/// these tests run from — the crate, under a checkout under `$HOME` on every
/// machine that runs them — *contains* `$HOME`: a bare `pwd` of the wrong
/// directory passed for the right one.
const WHERE_IT_STARTED: &str = "printf '[%s]\\n' \"$(pwd)\"";

/// What [`WHERE_IT_STARTED`] prints for a child that started in `dir`.
///
/// And a test that is itself running in `dir` cannot tell the child's
/// directory from its own, so it says so rather than passing.
fn started_at(dir: &Path) -> String {
    assert_ne!(
        std::env::current_dir().ok().as_deref(),
        Some(dir),
        "the test runs in the directory it expects the child in, so it proves nothing"
    );
    format!("[{}]", dir.to_string_lossy())
}

#[test]
fn test_a_working_directory_that_is_gone_starts_the_child_at_home() {
    // What a restored session hands over after the worktree it named was
    // removed: a path that no longer exists. Passed to the child's `cwd` it
    // fails the spawn and the pane cannot open; dropped, the child lands at
    // $HOME the way one with no directory does.
    if cfg!(windows) {
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let Some(program) = shell_command(WHERE_IT_STARTED) else {
        return;
    };
    let gone = home.join("a-worktree-crook-never-made-and-that-is-not-there");
    assert!(!gone.exists(), "the test's supposedly-absent path exists");
    let mut terminal = Terminal::spawn(TerminalOptions {
        program,
        size: TerminalSize::new(120, 6),
        working_directory: Some(gone),
        ..TerminalOptions::default()
    })
    .expect("a shell should start even when the saved directory is gone");
    let output = read_on_a_thread(terminal.take_reader().expect("a reader"));

    assert!(
        feed_until(&mut terminal, &output, &started_at(&home)),
        "a pane whose saved directory is gone should open at $HOME, got: {:?}",
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
    let Some(program) = shell_command(WHERE_IT_STARTED) else {
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
        feed_until(&mut terminal, &output, &started_at(&home)),
        "a pane with no directory of its own should open at $HOME, got: {:?}",
        terminal.snapshot().text()
    );
}

/// Everything the far end of a [`FakeLink`] has been told, and how its child
/// is doing — shared with the test, since the link itself is moved into the
/// terminal.
#[derive(Debug, Default)]
struct FarEnd {
    /// Every byte the terminal wrote, in order.
    received: Vec<u8>,
    /// Every size the terminal passed on.
    sizes: Vec<TerminalSize>,
    exit: Option<ChildExit>,
    killed: bool,
}

/// A link with no process behind it: the child's output is whatever the test
/// says through [`FakeChild`], and everything sent to the child is kept.
///
/// The point of [`PtyLink`] — a whole session, driven through the same
/// methods the application drives a real one through, with nothing spawned.
#[derive(Debug)]
struct FakeLink {
    reader: Option<PtyReader>,
    far_end: Arc<Mutex<FarEnd>>,
}

/// The test's side of a [`FakeLink`]: the child's voice, and its ears.
struct FakeChild {
    output: Option<mpsc::Sender<Vec<u8>>>,
    far_end: Arc<Mutex<FarEnd>>,
}

/// The fake's process id, which is what proves [`Terminal::process_id`] asked
/// the link rather than making one up.
const FAKE_PID: u32 = 4242;

impl FakeLink {
    fn new() -> (Self, FakeChild) {
        let (output, chunks) = mpsc::channel();
        let far_end = Arc::new(Mutex::new(FarEnd::default()));
        let link = Self {
            reader: Some(PtyReader::new(Chunks {
                chunks,
                pending: Vec::new(),
            })),
            far_end: far_end.clone(),
        };
        let child = FakeChild {
            output: Some(output),
            far_end,
        };
        (link, child)
    }
}

impl PtyLink for FakeLink {
    fn take_reader(&mut self) -> Option<PtyReader> {
        self.reader.take()
    }

    fn writer(&self) -> Result<Box<dyn Write + Send>> {
        Ok(Box::new(Ears(self.far_end.clone())))
    }

    fn resize(&mut self, size: TerminalSize) -> Result<()> {
        self.far_end.lock().unwrap().sizes.push(size);
        Ok(())
    }

    fn process_id(&self) -> Option<u32> {
        Some(FAKE_PID)
    }

    fn try_wait(&mut self) -> Result<Option<ChildExit>> {
        Ok(self.far_end.lock().unwrap().exit.clone())
    }

    fn wait(&mut self) -> Result<ChildExit> {
        self.far_end
            .lock()
            .unwrap()
            .exit
            .clone()
            .ok_or_else(|| anyhow::anyhow!("the fake child cannot be waited for while it runs"))
    }

    fn kill(&mut self) -> Result<()> {
        let mut far_end = self.far_end.lock().unwrap();
        far_end.killed = true;
        far_end.exit.get_or_insert(ChildExit {
            code: 1,
            signal: Some("Hangup".to_owned()),
        });
        Ok(())
    }
}

impl FakeChild {
    /// Prints `bytes`, as one read's worth.
    fn say(&self, bytes: impl AsRef<[u8]>) {
        self.output
            .as_ref()
            .expect("an exited child says nothing")
            .send(bytes.as_ref().to_vec())
            .expect("the reader is still there");
    }

    /// Exits with `code`, which also closes its end of the stream.
    fn exit(&mut self, code: u32) {
        self.far_end.lock().unwrap().exit = Some(ChildExit::from_code(code));
        self.output = None;
    }

    fn received(&self) -> Vec<u8> {
        self.far_end.lock().unwrap().received.clone()
    }

    fn sizes(&self) -> Vec<TerminalSize> {
        self.far_end.lock().unwrap().sizes.clone()
    }

    fn killed(&self) -> bool {
        self.far_end.lock().unwrap().killed
    }
}

/// The fake's output, one [`FakeChild::say`] per read — which is what makes a
/// scripted session deterministic: a read returns exactly what was said.
struct Chunks {
    chunks: Receiver<Vec<u8>>,
    pending: Vec<u8>,
}

impl std::io::Read for Chunks {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            // The child exiting drops the sender, which is end-of-file.
            let Ok(chunk) = self.chunks.recv() else {
                return Ok(0);
            };
            self.pending = chunk;
        }
        let taken = self.pending.len().min(buffer.len());
        buffer[..taken].copy_from_slice(&self.pending[..taken]);
        self.pending.drain(..taken);
        Ok(taken)
    }
}

/// The fake's input: everything written lands in [`FarEnd::received`].
struct Ears(Arc<Mutex<FarEnd>>);

impl Write for Ears {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().received.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A terminal over a fresh fake link, at a small grid.
fn over_a_fake_link() -> (Terminal, FakeChild) {
    let (link, child) = FakeLink::new();
    let emulator = Emulator::new(TerminalSize::new(30, 6), 100, Palette::default());
    let terminal =
        Terminal::with_link(Box::new(link), emulator).expect("a fake link hands out a writer");
    (terminal, child)
}

/// Reads what the child said last and feeds it, the way the application's
/// reader thread does with every read.
fn hear(terminal: &mut Terminal, reader: &mut PtyReader) {
    let mut buffer = [0; 4096];
    let read = reader.read(&mut buffer).expect("the fake reads");
    terminal
        .feed(&buffer[..read])
        .expect("the fake takes every reply");
}

#[test]
fn test_a_terminal_over_an_in_memory_link_runs_a_scripted_session() {
    let (mut terminal, mut child) = over_a_fake_link();
    let mut reader = terminal
        .take_reader()
        .expect("the link hands its reader to the terminal's caller");
    assert!(
        terminal.take_reader().is_none(),
        "the reader is handed out exactly once"
    );
    assert_eq!(Some(FAKE_PID), terminal.process_id());

    // An integrated shell's first prompt: where it is, then the prompt
    // between its two marks.
    child.say("\x1b]7;file:///srv/app\x07\x1b]133;A\x07$ \x1b]133;B\x07");
    hear(&mut terminal, &mut reader);
    assert_eq!(BlockState::AtPrompt, terminal.live_block().state);

    terminal
        .submit("echo hello")
        .expect("the link takes a line");
    assert_eq!(b"echo hello\r".to_vec(), child.received());
    assert_eq!(BlockState::Submitted, terminal.live_block().state);

    child.say(
        "echo hello\r\n\x1b]133;C\x07hello\r\n\x1b]133;D;0\x07\
         \x1b]133;A\x07$ \x1b]133;B\x07",
    );
    hear(&mut terminal, &mut reader);

    let [block] = terminal.blocks() else {
        panic!("one finished block, got {}", terminal.blocks().len());
    };
    assert_eq!(BlockState::Done, block.state);
    assert_eq!(Some("echo hello".to_owned()), block.command);
    assert_eq!(Some(0), block.exit);
    assert_eq!("$ echo hello\nhello", block.rows.to_text());
    assert_eq!(
        Some(Path::new("/srv/app")),
        block.working_directory.as_deref()
    );
    assert_eq!(Some(1), block.output_from);
    assert_eq!("$", terminal.snapshot().text().trim());
    assert!(
        terminal
            .take_events()
            .iter()
            .any(|event| matches!(event, TerminalEvent::CommandFinished { exit: Some(0), .. })),
        "the shell's D reached the application as an event"
    );

    // A question goes back down the link: the cursor position report.
    child.say("\x1b[6n");
    hear(&mut terminal, &mut reader);
    let received = child.received();
    let reply = &received[b"echo hello\r".len()..];
    assert!(
        reply.starts_with(b"\x1b[") && reply.ends_with(b"R"),
        "the cursor report did not reach the child: {reply:?}"
    );

    let size = TerminalSize::new(40, 8).with_cell_size(8, 16);
    terminal.resize(size).expect("the link takes a size");
    assert_eq!(vec![size], child.sizes());
    assert_eq!(size, terminal.size());

    assert_eq!(None, terminal.try_wait().expect("the fake answers"));
    child.exit(0);
    assert_eq!(
        Some(ChildExit::from_code(0)),
        terminal.try_wait().expect("the fake answers")
    );
    assert!(terminal.has_exited());
    assert!(
        terminal
            .take_events()
            .iter()
            .any(|event| matches!(event, TerminalEvent::ChildExited(exit) if exit.success())),
        "the exit the link reported reached the application"
    );
    assert_eq!(BlockState::Terminated, terminal.live_block().state);
    assert_eq!(
        0,
        reader.read(&mut [0; 16]).expect("the fake reads"),
        "a child that exited closes its end of the stream"
    );
}

#[test]
fn test_ending_a_terminal_ends_the_child_through_its_link() {
    let (mut terminal, child) = over_a_fake_link();
    assert!(!child.killed());

    terminal.kill().expect("the link ends its child");
    assert!(child.killed());
    let exit = terminal.shutdown().expect("the link says how it ended");
    assert_eq!(Some("Hangup"), exit.signal.as_deref());
}

#[test]
fn test_a_mirrored_feed_answers_no_query_and_draws_the_same_grid() {
    // A device attributes request, a cursor position report and the OSC 11
    // background query: the three a program most often waits on.
    const QUERIES: &[u8] = b"hi\x1b[c\x1b[6n\x1b]11;?\x07";

    let (mut answering, answered) = over_a_fake_link();
    answering.feed(QUERIES).expect("the fake takes every reply");
    let replies = answered.received();
    assert!(
        replies.windows(3).any(|at| at == b"\x1b[?")
            && replies.windows(8).any(|at| at == b"]11;rgb:"),
        "a plain feed answers every query, got {replies:?}"
    );

    let (mut mirror, mirrored) = over_a_fake_link();
    mirror.feed_mirrored(Fed::Output(QUERIES));
    assert_eq!(
        Vec::<u8>::new(),
        mirrored.received(),
        "a mirror answered a query the stream's own terminal already had"
    );
    assert_eq!(
        answering.snapshot().text(),
        mirror.snapshot().text(),
        "a mirror draws what it is fed"
    );
}
