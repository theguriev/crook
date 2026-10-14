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
//! on the pty's other end. And once below a program with no terminal of its
//! own, the way `claude -p` is when another agent's Bash tool runs it, where
//! nothing may arrive: that program is not on the pane, and its hooks'
//! reports are not the pane's agent's.
//!
//! The hooks `--agent-hooks` prints for Codex CLI, Gemini CLI and GitHub
//! Copilot CLI are run the same way, beside Claude Code's, and twice more
//! where no report can be made — below an agent with no terminal, and with
//! the binary gone — because there a hook that fails fails its agent: Copilot
//! CLI denies the tool call whose `preToolUse` hook errored, so a report that
//! could not be written stopped every tool of a `copilot -p` under CI, and of
//! every Copilot anywhere once the binary the hooks named had moved.

#![cfg(unix)]

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crook_terminal::agent::Session;
use crook_terminal::{AgentReport, Program, Pty, PtyLink as _, TerminalSize};
use serde_json::Value;

/// The binary under test, the one a release ships.
const CROOK: &str = env!("CARGO_BIN_EXE_crook");

/// The Claude Code plugin's hooks, as the repository ships them.
const PLUGIN_HOOKS: &str = include_str!("../../packaging/claude-code/hooks/hooks.json");

/// The agents whose hooks `--agent-hooks` prints as shell commands. OpenCode
/// gets a TypeScript plugin and aider a sentence, and neither is a command
/// an agent runs.
const AGENTS_WITH_HOOKS: [&str; 4] = ["claude", "codex", "gemini", "copilot"];

/// A binary that is not there: Crook uninstalled, Crook.app moved, or the
/// build the hooks were printed from deleted since.
const GONE: &str = "/nonexistent/crook";

/// What every hook is handed on stdin: a prompt to take a title from and a
/// notification's text to take a message from, so each event's report says
/// whatever its arguments ask it to.
const INPUT: &str = r#"{"session_id": "test", "hook_event_name": "Notification", "prompt": "port the tab bar\nand the tests", "message": "Claude needs your permission to use Bash"}"#;

/// The title `--title -` makes of [`INPUT`]: its prompt's first line.
const TITLE: &str = "port the tab bar";

/// The message `--message -` makes of [`INPUT`].
const MESSAGE: &str = "Claude needs your permission to use Bash";

/// The session `--session -` and `--session-start -` read out of [`INPUT`].
const SESSION: &str = "test";

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
const SCRIPT: &str = r#"detach() { perl -MPOSIX -e 'POSIX::setsid() > 0 or die "setsid: $!\n"; exec @ARGV or die "exec: $!\n"' /bin/sh -c "$@"; }
detach 'if (: >/dev/tty) 2>/dev/null; then echo attached; else echo detached; fi' >"$HOOK_DIR/probe" 2>&1
detach "$HOOK_COMMAND" <"$HOOK_DIR/input" >"$HOOK_DIR/stdout" 2>"$HOOK_DIR/stderr"
echo "$?" >"$HOOK_DIR/status"
echo crook-hook-done"#;

/// What [`Agent::Detached`] runs as the hook's command: the real command,
/// in a session further down, below a shell that stands in for an agent with
/// no terminal. The `exit` after it keeps that shell from `exec`ing `perl`,
/// which would leave no agent between and make `setsid` fail, the process
/// already leading a session.
const UNDER_A_DETACHED_AGENT: &str = r#"perl -MPOSIX -e 'POSIX::setsid() > 0 or die "setsid: $!\n"; exec @ARGV or die "exec: $!\n"' /bin/sh -c "$AGENT_HOOK_COMMAND"; exit $?"#;

/// Where the program that runs the hook is.
#[derive(Clone, Copy, Debug)]
enum Agent {
    /// On the pty, the way Claude Code is on the pane.
    OnThePane,
    /// In a session of its own below the pty, with no terminal, the way
    /// `claude -p` is when the pane's Claude Code runs it through its Bash
    /// tool, which starts every command that way.
    Detached,
}

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

/// Runs `command` the way Claude Code runs a hook in a Crook pane, with
/// Claude Code where `agent` says: `sh -c`, a session of its own, [`INPUT`] on
/// stdin, and the pane's `TERM_PROGRAM` and `CROOK_BIN` in its environment.
///
/// With [`Agent::Detached`] the probe is the question for the agent rather
/// than the hook: the agent's session, like the probe's, is started from the
/// pty's process.
fn run_as_a_hook(command: &str, agent: Agent) -> Ran {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("input"), INPUT).expect("the input should be writable");
    let (hook, under_the_agent) = match agent {
        Agent::OnThePane => (command, ""),
        Agent::Detached => (UNDER_A_DETACHED_AGENT, command),
    };
    let environment = [
        ("TERM_PROGRAM", "Crook"),
        ("CROOK_BIN", CROOK),
        ("HOOK_COMMAND", hook),
        ("AGENT_HOOK_COMMAND", under_the_agent),
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
/// after `--agent`, with the title, the message or the session `-` reads from
/// stdin, and no pull request, since [`INPUT`] names none.
fn expected_report(command: &str) -> String {
    let (_, arguments) = command
        .split_once("--agent ")
        .unwrap_or_else(|| panic!("{command} runs no `--agent`"));
    let mut words = arguments
        .split_whitespace()
        .take_while(|word| *word != "||");
    let status = words.next().expect("`--agent` is followed by a status");
    let status = AgentReport::parse(status).unwrap_or_else(|| panic!("{status} is no status"));
    let (mut title, mut message, mut session) = (None, None, None);
    while let Some(flag) = words.next() {
        assert_eq!(
            Some("-"),
            words.next(),
            "{command}: {flag} is not read from stdin"
        );
        match flag {
            "--title" => title = Some(TITLE),
            "--message" => message = Some(MESSAGE),
            // Read for the address a `gh pr create` printed, and INPUT is no
            // such call's: the report is the status alone, as it is for
            // every tool that opens no pull request.
            "--pull-request" => {}
            "--session" => session = Some(Session::Of(SESSION)),
            "--session-start" => session = Some(Session::Starts(SESSION)),
            other => panic!("{command}: {other} is not a flag `--agent` takes"),
        }
    }
    crook_terminal::agent::report(status, title, message, session)
}

/// Asserts that `command`, run as a hook, told the pane what its arguments
/// say, exited 0, and printed nothing Claude Code would read.
fn assert_reaches_the_pane(event: &str, command: &str) {
    let ran = run_as_a_hook(command, Agent::OnThePane);
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

/// Asserts that `command`, run as a hook of an agent with no terminal, told
/// no terminal anything, failed saying so, and still exited 0 and printed
/// nothing Claude Code would read.
fn assert_reaches_nothing_from_a_detached_agent(event: &str, command: &str) {
    let ran = run_as_a_hook(command, Agent::Detached);
    assert_eq!(
        "detached\n", ran.probe,
        "{event}: the agent here has to have no terminal of its own, as under a Bash tool"
    );
    assert!(
        !ran.pane.contains("\x1b]6340;"),
        "{event}: an agent that is not on the pane reported on it: {:?}",
        ran.pane
    );
    // Proof that `crook` ran and found nowhere to write, rather than that the
    // guard stopped it before it could.
    assert!(
        ran.stderr.contains("there is none"),
        "{event}: `crook --agent` did not fail for want of a terminal; it said {:?}",
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

/// Each command `--agent-hooks <agent>` prints, run from `binary`, beside
/// the agent and the event it is for: under `command` in a matcher group for
/// Claude Code, Codex and Gemini CLI, under `bash` for Copilot CLI.
fn printed_hooks(agent: &str, binary: &str) -> Vec<(String, String)> {
    let printed = crook::agent::hooks_text(agent, Path::new(binary))
        .unwrap_or_else(|error| panic!("{agent} has no hooks: {error}"));
    let fragment: Value = serde_json::from_str(&printed.text)
        .unwrap_or_else(|error| panic!("{agent}'s fragment is not JSON: {error}"));
    let events = fragment["hooks"]
        .as_object()
        .unwrap_or_else(|| panic!("{agent}'s fragment has no hooks"));
    assert!(!events.is_empty(), "{agent} prints no hooks");
    events
        .iter()
        .map(|(event, hooks)| {
            let command = hooks[0]["hooks"][0]["command"]
                .as_str()
                .or_else(|| hooks[0]["bash"].as_str())
                .unwrap_or_else(|| panic!("{agent}'s {event} hook runs no command"));
            (format!("{agent} {event}"), command.to_owned())
        })
        .collect()
}

/// `command` as a hook in a pane runs it once the binary is gone: the pane
/// still names the one it was started by, and there is no `crook` on `PATH`
/// — nor anything else, which the hooks have no need of.
fn with_crook_gone(command: &str) -> String {
    format!("CROOK_BIN={GONE}; PATH=/nonexistent; {command}")
}

#[test]
fn the_hooks_merged_by_hand_reach_the_pane_from_a_session_with_no_terminal() {
    if !perl_is_here() {
        return;
    }
    for agent in AGENTS_WITH_HOOKS {
        for (event, command) in printed_hooks(agent, CROOK) {
            assert_reaches_the_pane(&event, &command);
        }
    }
}

#[test]
fn no_hook_merged_by_hand_fails_its_agent_when_the_agent_has_no_terminal() {
    // `copilot -p` under CI, cron, a git hook or `ssh` without `-t`: the
    // report has nowhere to go, `crook --agent` says so and exits 1, and the
    // hook has to exit 0 all the same or Copilot CLI refuses the tool call.
    if !perl_is_here() {
        return;
    }
    for agent in AGENTS_WITH_HOOKS {
        for (event, command) in printed_hooks(agent, CROOK) {
            assert_reaches_nothing_from_a_detached_agent(&event, &command);
        }
    }
}

#[test]
fn no_hook_merged_by_hand_fails_its_agent_once_crook_is_gone() {
    // The hooks outlive the binary in the agent's own settings: once it has
    // gone, a hook that ran it would be "not found" before every tool call
    // of every session, in a message that never says Crook.
    if !perl_is_here() {
        return;
    }
    for agent in AGENTS_WITH_HOOKS {
        for (event, command) in printed_hooks(agent, GONE) {
            let ran = run_as_a_hook(&with_crook_gone(&command), Agent::OnThePane);
            assert_eq!(
                ("0\n", "", ""),
                (
                    ran.status.as_str(),
                    ran.stdout.as_str(),
                    ran.stderr.as_str()
                ),
                "{event} with the binary gone runs {command:?}"
            );
            assert!(
                !ran.pane.contains("\x1b]6340;"),
                "{event} reported with no binary to report through: {:?}",
                ran.pane
            );
        }
    }
}

#[test]
fn no_plugin_hook_of_an_agent_with_no_terminal_reports_on_the_pane_above_it() {
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
        assert_reaches_nothing_from_a_detached_agent(event, command);
    }
}
