//! Claude Code's hooks for Crook, run the way Claude Code runs them, against
//! the real binary.
//!
//! On macOS and Linux Claude Code starts every command hook in a session of
//! its own, with no controlling terminal, so a hook cannot open `/dev/tty`.
//! `crook --agent` writes its report to a terminal, and a test that runs the
//! hook in the test's own session, or hands it a stand-in binary that never
//! opens one, passes while every report the product makes goes nowhere —
//! which is how the plugin's first version was checked. These run each
//! command under `sh -c` in a new session, below a process that holds a pty
//! as its terminal the way `claude` holds the pane's, and read what arrives
//! on the pty's other end.

#![cfg(unix)]

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crook_terminal::{AgentReport, Program, Pty, TerminalSize};
use serde_json::Value;

/// The binary under test, the one a release ships.
const CROOK: &str = env!("CARGO_BIN_EXE_crook");

/// The Claude Code plugin's hooks, as the repository ships them.
const PLUGIN_HOOKS: &str = include_str!("../../packaging/claude-code/hooks/hooks.json");

/// What every hook is handed on stdin: a prompt to take a title from and a
/// notification's text to take a message from, so each event's report says
/// whatever its arguments ask it to.
const INPUT: &str = r#"{"session_id": "test", "hook_event_name": "Notification", "prompt": "port the tab bar\nand the tests", "message": "Claude needs your permission to use Bash"}"#;

/// The title `--title -` makes of [`INPUT`]: its prompt's first line.
const TITLE: &str = "port the tab bar";

/// The message `--message -` makes of [`INPUT`].
const MESSAGE: &str = "Claude needs your permission to use Bash";

/// How long one hook is given before the test calls it a failure rather than
/// hanging the suite.
const DEADLINE: Duration = Duration::from_secs(30);

/// What the process on the pty prints once the hook has finished, so the
/// reader knows everything the hook wrote is already in.
const DONE: &str = "crook-hook-done";

/// What the pty's process runs: the hook's command under `sh -c`, started
/// through `perl`'s `setsid` so it is in a session of its own the way Claude
/// Code's `detached` spawn puts it, with its input, output and errors on
/// files the way Claude Code's are on pipes. Beside it, under the same
/// `setsid`, the question the test would be pointless without: whether a hook
/// started this way has a terminal of its own.
const SCRIPT: &str = r#"detach() { perl -MPOSIX -e 'defined POSIX::setsid() or die "setsid: $!\n"; exec @ARGV or die "exec: $!\n"' /bin/sh -c "$@"; }
detach 'if (: >/dev/tty) 2>/dev/null; then echo attached; else echo detached; fi' >"$HOOK_DIR/probe" 2>&1
detach "$HOOK_COMMAND" <"$HOOK_DIR/input" >"$HOOK_DIR/stdout" 2>"$HOOK_DIR/stderr"
echo "$?" >"$HOOK_DIR/status"
echo crook-hook-done"#;

/// A directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "crook-claude-code-hooks-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("a temporary directory should be creatable");
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

/// What one hook did.
#[derive(Debug)]
struct Ran {
    /// Everything that arrived on the pty: the pane, as far as the hook knows.
    pane: String,
    /// `detached` when the hook had no terminal of its own, as under Claude
    /// Code.
    probe: String,
    /// Its exit status, as `sh` printed it.
    status: String,
    /// What it wrote to its standard output, which Claude Code reads.
    stdout: String,
    /// What it wrote to its standard error.
    stderr: String,
}

/// Whether `perl` is here: it is what puts a hook in a session of its own,
/// on macOS and Linux alike, with nothing to install.
///
/// Every CI runner and every macOS has one; a machine without it skips these
/// tests, and CI, where a skip would be a pass for the wrong reason, fails
/// them instead.
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

/// Runs `command` the way Claude Code runs a hook in a Crook pane: `sh -c`,
/// a session of its own, [`INPUT`] on stdin, and the pane's `TERM_PROGRAM`
/// and `CROOK_BIN` in its environment.
fn run_as_a_hook(command: &str) -> Ran {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("input"), INPUT).expect("the input should be writable");
    let environment = [
        ("TERM_PROGRAM", "Crook"),
        ("CROOK_BIN", CROOK),
        ("HOOK_COMMAND", command),
        (
            "HOOK_DIR",
            scratch.0.to_str().expect("the scratch path is text"),
        ),
    ]
    .map(|(key, value)| (key.to_owned(), value.to_owned()));
    let mut pty = Pty::spawn(
        &Program::command("/bin/sh", ["-c", SCRIPT]),
        TerminalSize::new(200, 24),
        Some(scratch.0.as_path()),
        &environment,
    )
    .expect("a shell should start on a pty");

    let (sender, receiver) = mpsc::channel();
    let mut reader = pty.take_reader().expect("a fresh pty has its reader");
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                return;
            }
        }
    });
    let mut pane = String::new();
    let deadline = Instant::now() + DEADLINE;
    while !pane.contains(DONE) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(chunk) = receiver.recv_timeout(remaining) else {
            break;
        };
        pane.push_str(&String::from_utf8_lossy(&chunk));
    }
    let _ = pty.kill();
    assert!(
        pane.contains(DONE),
        "the hook did not finish in {DEADLINE:?}; the pane got {pane:?}"
    );
    Ran {
        pane,
        probe: scratch.read("probe"),
        status: scratch.read("status"),
        stdout: scratch.read("stdout"),
        stderr: scratch.read("stderr"),
    }
}

/// The report `command`'s `--agent` arguments make of [`INPUT`]: the status
/// after `--agent`, with the title or the message `-` reads from stdin.
fn expected_report(command: &str) -> String {
    let (_, arguments) = command
        .split_once("--agent ")
        .unwrap_or_else(|| panic!("{command} runs no `--agent`"));
    let mut words = arguments
        .split_whitespace()
        .take_while(|word| *word != "||");
    let status = words.next().expect("`--agent` is followed by a status");
    let status = AgentReport::parse(status).unwrap_or_else(|| panic!("{status} is no status"));
    let (mut title, mut message) = (None, None);
    while let Some(flag) = words.next() {
        assert_eq!(
            Some("-"),
            words.next(),
            "{command}: {flag} is not read from stdin"
        );
        match flag {
            "--title" => title = Some(TITLE),
            "--message" => message = Some(MESSAGE),
            other => panic!("{command}: {other} is not a flag `--agent` takes"),
        }
    }
    crook_terminal::agent::report(status, title, message)
}

/// Asserts that `command`, run as a hook, told the pane what its arguments
/// say, exited 0, and printed nothing Claude Code would read.
fn assert_reaches_the_pane(event: &str, command: &str) {
    let ran = run_as_a_hook(command);
    assert_eq!(
        "detached\n", ran.probe,
        "{event}: a hook here has to be where Claude Code puts one, with no terminal of its own"
    );
    let report = expected_report(command);
    assert!(
        ran.pane.contains(&report),
        "{event}: the pane never got {report:?}; it got {:?}, and the hook said {:?}",
        ran.pane,
        ran.stderr
    );
    assert_eq!("0\n", ran.status, "{event} failed: {}", ran.stderr);
    assert_eq!(
        "", ran.stdout,
        "{event} printed what Claude Code would read"
    );
}

#[test]
fn every_plugin_hook_reaches_the_pane_from_a_session_with_no_terminal() {
    if !perl_is_here() {
        return;
    }
    let shipped: Value = serde_json::from_str(PLUGIN_HOOKS).expect("hooks.json is JSON");
    let events = shipped["hooks"].as_object().expect("hooks.json has hooks");
    assert!(!events.is_empty());
    for (event, groups) in events {
        let command = groups[0]["hooks"][0]["command"]
            .as_str()
            .expect("a hook has a command");
        assert_reaches_the_pane(event, command);
    }
}

#[test]
fn the_hooks_merged_by_hand_reach_the_pane_from_a_session_with_no_terminal() {
    if !perl_is_here() {
        return;
    }
    let printed = crook::agent::hooks_text("claude", Path::new(CROOK)).expect("claude has hooks");
    let fragment: Value = serde_json::from_str(&printed.text).expect("the fragment is JSON");
    let events = fragment["hooks"]
        .as_object()
        .expect("the fragment has hooks");
    assert!(!events.is_empty());
    for (event, groups) in events {
        let command = groups[0]["hooks"][0]["command"]
            .as_str()
            .expect("a hook has a command");
        assert_reaches_the_pane(event, command);
    }
}
