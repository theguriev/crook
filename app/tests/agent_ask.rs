//! `crook --agent ask claude`, run the way Claude Code runs a hook, against
//! the real binary and a window the test plays.
//!
//! The hook names its pane by the device number of the terminal its report
//! would go to, which under Claude Code is not its own: Claude Code starts
//! every hook in a session of its own, with no controlling terminal. A test
//! that ran it in the test's own session, or below a stand-in that never had
//! a terminal, would pass while the product named nothing. So these run the
//! real binary under `sh -c` in a new session, below a process that holds a
//! pty as its terminal the way `claude` holds the pane's, and read what the
//! pty's other end gets — which must be nothing of the hook's: it writes to
//! no terminal. The window is a socket in a directory only this user can
//! enter, as a window's is, answering the way the case says.

#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crook_terminal::{Program, Pty, PtyLink as _, TerminalSize};
use serde_json::{Value, json};

/// The binary under test, the one a release ships.
const CROOK: &str = env!("CARGO_BIN_EXE_crook");

/// What Claude Code prints for an allow, which the hook must print exactly.
const ALLOW: &str = "{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"allow\"}}}\n";

/// How long one case is given before the test calls it a failure rather than
/// hanging the suite.
const DEADLINE: Duration = Duration::from_secs(30);

/// What the process on the pty prints once the hook has finished.
const DONE: &str = "crook-hook-done";

/// What the pty's process runs: names its terminal into a file, then runs
/// the hook's command under `sh -c` in a session of its own — through
/// `perl`'s `setsid`, the way Claude Code's detached spawn does — with its
/// input, output and errors on files the way Claude Code's are on pipes.
/// With `HOOK_ATTACHED` set, the hook runs in this session instead, on the
/// pty, where a write to its terminal would be a write to the pane.
const SCRIPT: &str = r#"tty >"$HOOK_DIR/tty"
detach() { perl -MPOSIX -e 'POSIX::setsid() > 0 or die "setsid: $!\n"; exec @ARGV or die "exec: $!\n"' /bin/sh -c "$@"; }
if [ -n "$HOOK_ATTACHED" ]; then
  /bin/sh -c "$HOOK_COMMAND" <"$HOOK_DIR/input" >"$HOOK_DIR/stdout" 2>"$HOOK_DIR/stderr"
else
  detach "$HOOK_COMMAND" <"$HOOK_DIR/input" >"$HOOK_DIR/stdout" 2>"$HOOK_DIR/stderr"
fi
echo "$?" >"$HOOK_DIR/status"
echo crook-hook-done"#;

/// The hook's command: the binary, as Claude Code's hook entry would run
/// it, below a shell that writes its own pid down first and does not `exec`
/// the binary, so that the shell is the hook's parent.
const HOOK: &str = r#"echo $$ >"$HOOK_DIR/sh.pid"; "$CROOK_BIN" --agent ask claude; exit $?"#;

/// What Claude Code 2.1 hands a `PermissionRequest` hook for a `Bash` call.
fn input() -> String {
    json!({
        "session_id": "4f1c2b7e-0d3a-4e5f-9a8b-7c6d5e4f3a2b",
        "cwd": "/home/someone/work/crook",
        "permission_mode": "default",
        "hook_event_name": "PermissionRequest",
        "tool_name": "Bash",
        "tool_input": {"command": "rm -rf build", "description": "Remove the build directory"},
    })
    .to_string()
}

/// Whether `perl` is here: it is what puts a hook in a session of its own.
/// CI, where a skip would be a pass for the wrong reason, fails instead.
fn perl_is_here() -> bool {
    let found = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join("perl").is_file())
    });
    assert!(
        found || std::env::var_os("CI").is_none(),
        "no perl on PATH, which these tests need to put a hook in a session of its own"
    );
    if !found {
        eprintln!("skipped: no perl on PATH to put a hook in a session of its own");
    }
    found
}

/// A directory that removes itself: in `$XDG_RUNTIME_DIR` when there is one,
/// which is this user's alone and not `/tmp`'s quota, and short enough for a
/// socket's path on macOS.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|runtime| runtime.is_absolute() && runtime.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!(
            "crook-ask-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a temporary directory");
        Self(path)
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// How the window the test plays answers the one connection it takes.
enum Plays {
    /// Holds the question, then allows it.
    Allows,
    /// Is a Crook from before `agent.ask`, refusing the verb.
    IsOlder,
    /// Holds the question, says so on the channel, and never answers.
    Holds(mpsc::Sender<()>),
}

/// What the window saw of the hook.
#[derive(Debug, Default)]
struct Seen {
    /// The request line, read.
    request: Option<Value>,
    /// The line the hook sent after the window's last, when it sent one.
    after: Option<String>,
    /// When the window wrote its last line.
    wrote_at: Option<Instant>,
    /// When the hook's end closed.
    closed_at: Option<Instant>,
}

/// A window: a socket in a directory only this user can enter, `0600`, the
/// way a window's is.
struct Window {
    directory: Scratch,
    socket: PathBuf,
    listener: UnixListener,
}

impl Window {
    fn new() -> Self {
        let directory = Scratch::new();
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o700)).expect("closed");
        let socket = directory.0.join("w.sock");
        let listener = UnixListener::bind(&socket).expect("a socket to answer on");
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).expect("locked");
        listener.set_nonblocking(true).expect("non-blocking");
        Self {
            directory,
            socket,
            listener,
        }
    }

    /// Whether anything has connected, without waiting for it.
    fn was_asked(&self) -> bool {
        self.listener.accept().is_ok()
    }

    /// Answers the one connection the way `plays` says, on a thread of its
    /// own, and hands back what it saw.
    fn answer(&self, plays: Plays) -> thread::JoinHandle<Seen> {
        let listener = self.listener.try_clone().expect("a second handle");
        thread::spawn(move || {
            let deadline = Instant::now() + DEADLINE;
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                    Err(error) => panic!("the hook never asked: {error}"),
                }
            };
            stream.set_nonblocking(false).expect("blocking");
            stream.set_read_timeout(Some(DEADLINE)).expect("a timeout");
            let mut lines = BufReader::new(stream.try_clone().expect("a second handle"));
            let mut writer = stream;
            let mut seen = Seen::default();
            let mut line = String::new();
            lines.read_line(&mut line).expect("the request");
            seen.request = serde_json::from_str(&line).ok();

            let mut write = |text: &str, seen: &mut Seen| {
                writer.write_all(text.as_bytes()).expect("written");
                seen.wrote_at = Some(Instant::now());
            };
            let held = "{\"v\":1,\"ok\":true,\"result\":{\"asked\":1}}\n";
            match plays {
                Plays::Allows => {
                    write(held, &mut seen);
                    write("{\"answer\":{\"decision\":\"allow\"}}\n", &mut seen);
                }
                Plays::IsOlder => write(
                    "{\"v\":1,\"ok\":false,\"error\":{\"code\":\"unknown-verb\",\"message\":\"this Crook does not know the verb \\\"agent.ask\\\"\"}}\n",
                    &mut seen,
                ),
                Plays::Holds(registered) => {
                    write(held, &mut seen);
                    let _ = registered.send(());
                }
            }
            // What the hook says next, if anything, and then its leaving.
            let mut after = String::new();
            if lines.read_line(&mut after).is_ok_and(|read| read > 0) {
                seen.after = Some(after);
                let mut rest = Vec::new();
                let _ = lines.read_to_end(&mut rest);
            }
            seen.closed_at = Some(Instant::now());
            seen
        })
    }
}

/// What one hook did.
#[derive(Debug)]
struct Ran {
    /// Everything that arrived on the pty: the pane, as far as the hook
    /// knows.
    pane: String,
    /// The terminal the pty's process said it is on, and that terminal's
    /// device number, read while the pty was still open.
    tty: (String, Option<u32>),
    /// The pty's own device number.
    device: Option<u32>,
    /// Its exit status, as `sh` printed it.
    status: String,
    /// What it wrote to its standard output, which Claude Code reads.
    stdout: String,
    /// What it wrote to its standard error.
    stderr: String,
}

/// How a case runs the hook.
struct Case<'a> {
    /// The window's socket, as `CROOK_SOCKET`.
    socket: &'a Path,
    /// `CLAUDE_CODE_ENTRYPOINT`, or `None` to leave it unset.
    entrypoint: Option<&'a str>,
    /// Whether the hook runs on the pty itself rather than in a session of
    /// its own.
    attached: bool,
    /// Done once the hook has started, on the test's thread: given the
    /// scratch directory the hook writes its shell's pid into.
    meanwhile: Box<dyn FnOnce(&Path) + 'a>,
}

impl<'a> Case<'a> {
    fn new(socket: &'a Path) -> Self {
        Self {
            socket,
            entrypoint: Some("cli"),
            attached: false,
            meanwhile: Box::new(|_| {}),
        }
    }
}

/// Runs the hook as `case` says, below a process holding a pty.
fn run(case: Case<'_>) -> Ran {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("input"), input()).expect("the input should be writable");
    let command = match case.entrypoint {
        Some(_) => HOOK.to_owned(),
        None => format!("unset CLAUDE_CODE_ENTRYPOINT; {HOOK}"),
    };
    let environment: Vec<(String, String)> = [
        ("TERM_PROGRAM", "Crook"),
        ("CROOK_BIN", CROOK),
        ("CROOK_SOCKET", case.socket.to_str().expect("text")),
        ("CROOK_TOKEN", "c0ffee"),
        ("CLAUDE_CODE_ENTRYPOINT", case.entrypoint.unwrap_or("cli")),
        ("HOOK_COMMAND", &command),
        ("HOOK_ATTACHED", if case.attached { "1" } else { "" }),
        ("HOOK_DIR", scratch.0.to_str().expect("text")),
        // No multiplexer but the one a case names: a suite run from inside
        // a person's own tmux would otherwise hand that one to the hook.
        ("TMUX", ""),
        ("STY", ""),
        ("ZELLIJ", ""),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect();
    let mut pty = Pty::spawn(
        &Program::command("/bin/sh", ["-c", SCRIPT]),
        TerminalSize::new(200, 24),
        Some(scratch.0.as_path()),
        &environment,
    )
    .expect("a shell should start on a pty");
    let device = pty.tty_device();

    let (sender, arriving) = mpsc::channel();
    let mut reader = pty.take_reader().expect("a fresh pty has its reader");
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                return;
            }
        }
    });
    (case.meanwhile)(&scratch.0);
    let mut pane = String::new();
    let deadline = Instant::now() + DEADLINE;
    while !pane.contains(DONE) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(chunk) = arriving.recv_timeout(remaining) else {
            break;
        };
        pane.push_str(&String::from_utf8_lossy(&chunk));
    }
    // Before the pty goes, and its node with it.
    let tty = scratch.read("tty").trim().to_owned();
    let named = fs::metadata(&tty)
        .ok()
        .map(|metadata| metadata.rdev() as u32);
    let _ = pty.kill();
    assert!(
        pane.contains(DONE),
        "the hook did not finish in {DEADLINE:?}; the pane got {pane:?}"
    );
    Ran {
        pane,
        tty: (tty, named),
        device,
        status: scratch.read("status").trim().to_owned(),
        stdout: scratch.read("stdout"),
        stderr: scratch.read("stderr"),
    }
}

#[test]
fn prints_claudes_allow_exactly_when_the_window_allows() {
    if !perl_is_here() {
        return;
    }
    let window = Window::new();
    let answering = window.answer(Plays::Allows);
    let ran = run(Case::new(&window.socket));
    let seen = answering.join().expect("the window");

    assert_eq!(ran.stdout, ALLOW, "{ran:?}");
    assert_eq!(ran.status, "0", "{ran:?}");
    assert_eq!(ran.stderr, "", "{ran:?}");
    assert_eq!(
        seen.after.as_deref(),
        Some("{\"delivered\":true}\n"),
        "the hook said it printed it: {seen:?}"
    );
}

#[test]
fn sends_the_device_of_the_pty_it_runs_below() {
    if !perl_is_here() {
        return;
    }
    let window = Window::new();
    let answering = window.answer(Plays::Allows);
    let ran = run(Case::new(&window.socket));
    let seen = answering.join().expect("the window");

    let request = seen.request.expect("a request");
    assert_eq!(request["verb"], json!("agent.ask"));
    assert_eq!(request["token"], json!("c0ffee"));
    let args = &request["args"];
    // The terminal the hook's parent's parent is on, as that process named
    // it — not the hook's own, which it has none of in a session of its own.
    let named = ran
        .tty
        .1
        .unwrap_or_else(|| panic!("{:?} is not a terminal", ran.tty.0));
    assert_eq!(args["tty"], json!(named), "{ran:?}");
    assert_eq!(Some(named), ran.device, "the pty names the same device");
    assert_eq!(args["agent"], json!("claude"));
    assert_eq!(
        args["session"],
        json!("4f1c2b7e-0d3a-4e5f-9a8b-7c6d5e4f3a2b")
    );
    assert_eq!(args["cwd"], json!("/home/someone/work/crook"));
    assert_eq!(args["wait"], json!(300));
    assert_eq!(
        args["request"],
        json!({
            "kind": "command",
            "command": "rm -rf build",
            "note": "Remove the build directory",
            "background": false,
            "outside_sandbox": false,
        })
    );
}

#[test]
fn prints_nothing_and_exits_0_against_an_older_window() {
    if !perl_is_here() {
        return;
    }
    let window = Window::new();
    let answering = window.answer(Plays::IsOlder);
    let ran = run(Case::new(&window.socket));
    let seen = answering.join().expect("the window");

    assert_eq!(ran.stdout, "", "{ran:?}");
    assert_eq!(ran.status, "0", "{ran:?}");
    assert_eq!(seen.after, None, "{seen:?}");
    let lingered = seen
        .closed_at
        .zip(seen.wrote_at)
        .map(|(closed, wrote)| closed.saturating_duration_since(wrote))
        .expect("the hook left");
    assert!(
        lingered < Duration::from_millis(300),
        "the hook stayed {lingered:?} after it was refused"
    );
}

#[test]
fn headless_claude_never_connects() {
    if !perl_is_here() {
        return;
    }
    for entrypoint in [Some("sdk-cli"), Some(""), None] {
        let window = Window::new();
        let ran = run(Case {
            entrypoint,
            ..Case::new(&window.socket)
        });
        assert_eq!(ran.stdout, "", "{entrypoint:?}: {ran:?}");
        assert_eq!(ran.status, "0", "{entrypoint:?}: {ran:?}");
        assert!(!window.was_asked(), "{entrypoint:?}: the hook connected");
    }
}

#[test]
fn never_writes_to_the_terminal_it_runs_under() {
    if !perl_is_here() {
        return;
    }
    // On the pty itself, where a write to its terminal is a write to the
    // pane, and in a session of its own below it, where an ancestor's
    // terminal is the pane: either way the pane gets the script's own line
    // and nothing else, an allow being printed included.
    for attached in [true, false] {
        let window = Window::new();
        let answering = window.answer(Plays::Allows);
        let ran = run(Case {
            attached,
            ..Case::new(&window.socket)
        });
        answering.join().expect("the window");
        assert_eq!(ran.stdout, ALLOW, "attached {attached}: {ran:?}");
        assert_eq!(
            ran.pane.trim(),
            DONE,
            "attached {attached}: the hook wrote to the pane: {:?}",
            ran.pane
        );
    }
}

#[test]
fn refuses_a_socket_in_a_directory_others_can_enter() {
    if !perl_is_here() {
        return;
    }
    let window = Window::new();
    fs::set_permissions(&window.directory.0, fs::Permissions::from_mode(0o755)).expect("loosened");
    let ran = run(Case::new(&window.socket));
    assert_eq!(ran.stdout, "", "{ran:?}");
    assert_eq!(ran.status, "0", "{ran:?}");
    assert!(
        !window.was_asked(),
        "the hook asked a socket anyone could have left"
    );
}

#[test]
fn exits_within_a_second_when_its_parent_sh_is_killed() {
    if !perl_is_here() {
        return;
    }
    let window = Window::new();
    let (registering, registered) = mpsc::channel();
    let answering = window.answer(Plays::Holds(registering));
    let killed_at = std::cell::Cell::new(None);
    let ran = run(Case {
        meanwhile: Box::new(|scratch| {
            registered
                .recv_timeout(DEADLINE)
                .expect("the window held the question");
            let pid = fs::read_to_string(scratch.join("sh.pid")).expect("the shell's pid");
            let killed = crook::process::command("kill")
                .args(["-KILL", pid.trim()])
                .status()
                .expect("kill runs");
            // Once `kill` has sent it, which is what the hook can notice.
            killed_at.set(Some(Instant::now()));
            assert!(killed.success());
        }),
        ..Case::new(&window.socket)
    });
    let seen = answering.join().expect("the window");
    assert_eq!(ran.stdout, "", "{ran:?}");
    let took = seen
        .closed_at
        .zip(killed_at.get())
        .map(|(closed, killed)| closed.saturating_duration_since(killed))
        .expect("the hook left");
    assert!(
        took < Duration::from_secs(1),
        "the hook outlived its parent by {took:?}"
    );
}
