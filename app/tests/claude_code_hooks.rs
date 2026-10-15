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
//!
//! And inside tmux, which a person runs in a pane to keep agents alive across
//! a restart of Crook, which keeps no processes. tmux drops every OSC it does
//! not know, 6340 among them, so a report written to the agent's terminal —
//! one of tmux's panes — never reached Crook's. These start tmux on the pty,
//! with the agent in a pane of tmux's own, and read what reaches the pty, which
//! is the pane Crook draws. And once with tmux only named in the environment,
//! the way every pane of a Crook started from inside tmux names it, where the
//! report has to stay on the pane rather than go to that tmux's client.

#![cfg(unix)]

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
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

/// What the pty runs the agent in.
#[derive(Clone, Copy)]
enum Pane<'a> {
    /// Nothing: the pty is the pane, and the agent's terminal.
    Plain,
    /// tmux, started on the pty, with the agent in its one pane: the pty is
    /// tmux's client's terminal, and the agent's is tmux's pane.
    Tmux(&'a Tmux),
    /// Nothing, in an environment that names a pane of a tmux all the same —
    /// its `TMUX` and its `TMUX_PANE` — the way every pane of a Crook started
    /// from inside that tmux names it.
    NamingTmux {
        /// The `TMUX` the environment has.
        tmux: &'a str,
        /// The `TMUX_PANE` it has.
        pane: &'a str,
    },
}

/// The configuration every [`Tmux`] starts with. `/bin/sh` for the command a
/// session is started on, whatever shell the person running the tests uses;
/// and a pane that stays after its command ends, so what the command wrote
/// last is drawn on the client before the server is killed, rather than lost
/// with a window that closed first.
const TMUX_CONFIG: &str = "set -g default-shell /bin/sh\nset -g remain-on-exit on\n";

/// A tmux server of the test's own, killed when this is dropped: on a socket
/// of its own, so neither a person's tmux nor another test's is touched, and
/// with no configuration but [`TMUX_CONFIG`] and what a case adds.
struct Tmux {
    /// The socket's name, under tmux's own directory.
    socket: String,
    /// Where the configuration file is.
    config: Scratch,
}

impl Tmux {
    /// A server yet to start, with `config` after [`TMUX_CONFIG`]; `None`,
    /// saying so, where no tmux runs.
    ///
    /// Skipped rather than failed without one, on CI too: tmux is what a
    /// person chooses to run in a pane, not something Crook needs, and a
    /// runner image is not promised to carry it.
    fn new(config: &str) -> Option<Self> {
        let here = crook::process::command("tmux")
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !here {
            eprintln!("skipped: no tmux to run");
            return None;
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let socket = format!(
            "crook-hooks-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let directory = Scratch::new();
        fs::write(
            directory.0.join("tmux.conf"),
            format!("{TMUX_CONFIG}{config}"),
        )
        .expect("the configuration should be writable");
        Some(Self {
            socket,
            config: directory,
        })
    }

    /// Whether this tmux is at least `major.minor`, by what `tmux -V` says; a
    /// build that names no number — `tmux master` — is taken to be.
    fn at_least(&self, major: u32, minor: u32) -> bool {
        let said = crook::process::command("tmux")
            .arg("-V")
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default();
        // `tmux 3.7c`, or `tmux next-3.8` for a build between releases.
        let version = said.split_whitespace().nth(1).unwrap_or_default();
        let mut numbers = version.trim_start_matches("next-").split('.').map(|part| {
            part.trim_end_matches(|character: char| !character.is_ascii_digit())
                .parse::<u32>()
                .ok()
        });
        match (numbers.next().flatten(), numbers.next().flatten()) {
            (Some(its_major), Some(its_minor)) => (its_major, its_minor) >= (major, minor),
            _ => true,
        }
    }

    /// `rest` after the arguments that make a `tmux` this server's.
    fn arguments(&self, rest: &[&str]) -> Vec<String> {
        let config = self.config.0.join("tmux.conf");
        let config = config.to_str().expect("the scratch path is text");
        ["-L", &self.socket, "-f", config]
            .iter()
            .chain(rest)
            .map(|word| (*word).to_owned())
            .collect()
    }

    /// A client of this server on the pty, running the command `rest` says.
    fn client(&self, rest: &[&str]) -> Program {
        Program::command("tmux", self.arguments(rest))
    }

    /// What the tmux command `rest` prints, run beside the server and not on
    /// a terminal.
    fn ask(&self, rest: &[&str]) -> String {
        let output = crook::process::command("tmux")
            .args(self.arguments(rest))
            .env_remove("TMUX")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .expect("tmux runs");
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        // A server that is killed leaves its socket behind, one more file in
        // tmux's directory for every case of every run.
        let socket = self.ask(&["display-message", "-p", "#{socket_path}"]);
        let _ = crook::process::command("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !socket.is_empty() {
            let _ = fs::remove_file(socket);
        }
    }
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
    run_as_a_hook_in(command, agent, Pane::Plain)
}

/// [`run_as_a_hook`], with the agent where `place` says rather than always on
/// the pty itself.
fn run_as_a_hook_in(command: &str, agent: Agent, place: Pane) -> Ran {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("input"), INPUT).expect("the input should be writable");
    let (hook, under_the_agent) = match agent {
        Agent::OnThePane => (command, ""),
        Agent::Detached => (UNDER_A_DETACHED_AGENT, command),
    };
    let mut environment = vec![
        ("TERM_PROGRAM", "Crook"),
        ("CROOK_BIN", CROOK),
        ("HOOK_COMMAND", hook),
        ("AGENT_HOOK_COMMAND", under_the_agent),
        (
            "HOOK_DIR",
            scratch.0.to_str().expect("the scratch path is text"),
        ),
        // No tmux but the one a case names: a test run from inside a person's
        // own tmux would otherwise hand that one to every hook.
        ("TMUX", ""),
        // Nor the id of the pane the tests were run from, which a tmux
        // started here would hand on to its panes and the hooks' guard would
        // let them through on, as though the server had been started in one.
        ("CROOK_PANE_ID", ""),
    ];
    let script = Program::command("/bin/sh", ["-c", SCRIPT]);
    let program = match place {
        Pane::Plain => script,
        // With no pane id for tmux to hand on, the way a server started
        // outside Crook has none: tmux says `TERM_PROGRAM=tmux` to its own
        // panes, and the hooks' guard lets them through on its `TMUX`.
        Pane::Tmux(tmux) => tmux.client(&["new-session", SCRIPT]),
        Pane::NamingTmux { tmux, pane } => {
            environment.extend([("TMUX", tmux), ("TMUX_PANE", pane)]);
            script
        }
    };
    let environment: Vec<_> = environment
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    let mut pty = Pty::spawn(
        &program,
        TerminalSize::new(200, 24),
        Some(scratch.0.as_path()),
        &environment,
    )
    .expect("a shell should start on a pty");

    let pane = read_until(&reading(&mut pty), DONE);
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

/// What arrives on `pty`, chunk by chunk, read on a thread of its own.
fn reading(pty: &mut Pty) -> mpsc::Receiver<Vec<u8>> {
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
    receiver
}

/// What `arriving` brings until it has brought `marker`, or until
/// [`DEADLINE`], whichever is first.
fn read_until(arriving: &mpsc::Receiver<Vec<u8>>, marker: &str) -> String {
    let mut arrived = String::new();
    let deadline = Instant::now() + DEADLINE;
    while !arrived.contains(marker) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(chunk) = arriving.recv_timeout(remaining) else {
            break;
        };
        arrived.push_str(&String::from_utf8_lossy(&chunk));
    }
    arrived
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
    assert_reaches_the_pane_from(event, command, Pane::Plain);
}

/// [`assert_reaches_the_pane`], with the agent where `place` says.
fn assert_reaches_the_pane_from(event: &str, command: &str, place: Pane) {
    let ran = run_as_a_hook_in(command, Agent::OnThePane, place);
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
    assert_reaches_nothing_from_a_detached_agent_in(event, command, Pane::Plain);
}

/// [`assert_reaches_nothing_from_a_detached_agent`], with the agent's own
/// agent where `place` says.
fn assert_reaches_nothing_from_a_detached_agent_in(event: &str, command: &str, place: Pane) {
    let ran = run_as_a_hook_in(command, Agent::Detached, place);
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

/// The Claude Code plugin's hooks, each beside the event it is for.
fn plugin_hooks() -> Vec<(String, String)> {
    let shipped: Value = serde_json::from_str(PLUGIN_HOOKS).expect("hooks.json is JSON");
    let events = shipped["hooks"].as_object().expect("hooks.json has hooks");
    assert!(!events.is_empty());
    events
        .iter()
        .map(|(event, groups)| {
            let command = groups[0]["hooks"][0]["command"]
                .as_str()
                .expect("a hook has a command");
            (event.clone(), command.to_owned())
        })
        .collect()
}

/// The plugin's hook for `event`.
fn plugin_hook(event: &str) -> String {
    plugin_hooks()
        .into_iter()
        .find_map(|(name, command)| (name == event).then_some(command))
        .unwrap_or_else(|| panic!("the plugin has no {event} hook"))
}

#[test]
fn every_plugin_hook_reaches_the_pane_from_inside_tmux_in_it() {
    // tmux drops the report written to its own pane, so it goes to the
    // terminal of tmux's client instead, which is the pane Crook draws.
    if !perl_is_here() {
        return;
    }
    for (event, command) in plugin_hooks() {
        let Some(tmux) = Tmux::new("") else {
            return;
        };
        assert_reaches_the_pane_from(&event, &command, Pane::Tmux(&tmux));
    }
}

#[test]
fn a_hook_that_cannot_ask_tmux_reaches_the_pane_through_tmuxs_passthrough() {
    // With no `tmux` on its PATH to ask which terminal the client is on, the
    // report goes the one other way out of a tmux pane: wrapped for tmux to
    // pass on, which it does where `allow-passthrough` is on. `-q`, because a
    // tmux older than the option passes the wrapper on without being asked.
    if !perl_is_here() {
        return;
    }
    let Some(tmux) = Tmux::new("set -gq allow-passthrough on\n") else {
        return;
    };
    // Without an answer from tmux, what says the pane is tmux's is the
    // TERM_PROGRAM tmux gives its panes, which it has since 3.2.
    if !tmux.at_least(3, 2) {
        eprintln!("skipped: a tmux older than 3.2 says nothing in TERM_PROGRAM");
        return;
    }
    let command = format!("PATH=/nonexistent; {}", plugin_hook("Notification"));
    assert_reaches_the_pane_from("Notification", &command, Pane::Tmux(&tmux));
}

#[test]
fn no_plugin_hook_of_an_agent_with_no_terminal_inside_tmux_reports_on_the_pane() {
    // `claude -p` run by the Bash tool of a Claude Code in tmux: its hooks
    // know the tmux pane from the environment they inherit, and the pane is
    // still the outer agent's, not theirs.
    if !perl_is_here() {
        return;
    }
    for (event, command) in plugin_hooks() {
        let Some(tmux) = Tmux::new("") else {
            return;
        };
        assert_reaches_nothing_from_a_detached_agent_in(&event, &command, Pane::Tmux(&tmux));
    }
}

#[test]
fn a_pane_of_a_crook_started_inside_tmux_reports_on_itself_and_not_to_that_tmux() {
    // Crook started from a terminal running tmux hands that tmux's `TMUX`
    // and `TMUX_PANE` to every pane of its own, where nothing is inside tmux
    // at all: a report sent to that tmux's client would light the terminal
    // Crook was started from, or be wrapped for a tmux that never sees it.
    if !perl_is_here() {
        return;
    }
    let Some(tmux) = Tmux::new("") else {
        return;
    };
    let mut outer = Pty::spawn(
        &tmux.client(&["new-session", "sleep 600"]),
        TerminalSize::new(200, 24),
        None,
        &[("TMUX".to_owned(), String::new())],
    )
    .expect("tmux should start on a pty");
    let outer_arrived = reading(&mut outer);
    let deadline = Instant::now() + DEADLINE;
    while tmux
        .ask(&["list-clients", "-F", "#{client_tty}"])
        .is_empty()
    {
        assert!(
            Instant::now() < deadline,
            "tmux's client did not attach in {DEADLINE:?}"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let variable = tmux
        .ask(&[
            "display-message",
            "-p",
            "#{socket_path},#{pid},#{session_id}",
        ])
        .replace('$', "");
    let pane = tmux.ask(&["display-message", "-p", "#{pane_id}"]);

    let command = plugin_hook("Notification");
    let ran = run_as_a_hook_in(
        &command,
        Agent::OnThePane,
        Pane::NamingTmux {
            tmux: &variable,
            pane: &pane,
        },
    );
    let report = expected_report(&command);
    assert!(
        ran.pane.contains(&report) && !ran.pane.contains("\x1bPtmux;"),
        "the pane did not get {report:?} as it is; it got {:?}, and the hook said {:?}",
        ran.pane,
        ran.stderr
    );
    // Whatever the hook wrote to tmux's client is on that terminal before
    // what tmux draws there after the hook has finished.
    tmux.ask(&["display-message", "crook-after-the-hook"]);
    let outer_got = read_until(&outer_arrived, "crook-after-the-hook");
    let _ = outer.kill();
    assert!(
        outer_got.contains("crook-after-the-hook"),
        "tmux drew nothing on its client in {DEADLINE:?}: {outer_got:?}"
    );
    assert!(
        !outer_got.contains("6340"),
        "the report went to the terminal Crook was started from: {outer_got:?}"
    );
}

/// `O_NONBLOCK`, which the standard library has no name for: the same on
/// Linux's x86 and Arm, and on macOS and the BSDs.
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(not(target_os = "linux"))]
const O_NONBLOCK: i32 = 0x0004;

/// Fills `terminal` until it takes no more: a terminal nobody is reading,
/// the way one is behind an `ssh` whose connection has gone quiet.
///
/// In smaller and smaller pieces, down to one byte: a terminal that refuses a
/// large write can still have room for a short one, and a report is short.
fn fill(terminal: &Path) {
    use std::io::{ErrorKind, Write as _};
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut terminal = fs::OpenOptions::new()
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open(terminal)
        .expect("the client's terminal opens");
    let mut taken = 0;
    for size in [4096, 256, 16, 1] {
        let piece = vec![b'x'; size];
        // A refusal can be a moment's, while tmux writes to the same
        // terminal; five in a row, a few milliseconds apart, are a full one.
        let mut refused = 0;
        while refused < 5 {
            match terminal.write(&piece) {
                Ok(written) => {
                    taken += written;
                    refused = 0;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    refused += 1;
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("the client's terminal would not be written to: {error}"),
            }
            // Far more than any terminal holds, so that one that never fills
            // is a failure rather than a test that never ends.
            assert!(
                taken < 1 << 24,
                "the client's terminal took 16 MiB and still had room"
            );
        }
    }
}

#[test]
fn a_tmux_client_on_a_terminal_nobody_reads_holds_no_hook() {
    // A second attach to the agent's session, over an `ssh` whose connection
    // has gone quiet — a laptop shut, a network dropped — stops being read,
    // and its terminal fills. A report written there waits for it to be read
    // again, and the hook with it, and the agent behind every tool call,
    // until the connection dies; and the Crook pane's client, if it came
    // after that one, never got the report at all.
    if !perl_is_here() {
        return;
    }
    let Some(tmux) = Tmux::new("") else {
        return;
    };
    let gate = Scratch::new();
    let go = gate.0.join("go");
    // The hook waits for the second client to be attached and filled.
    let command = format!(
        "until [ -e '{}' ]; do sleep 0.05; done; {}",
        go.display(),
        plugin_hook("Notification")
    );
    let (finished, finishing) = mpsc::channel::<()>();
    let (tmux, go) = (&tmux, &go);
    thread::scope(|scope| {
        scope.spawn(move || {
            let deadline = Instant::now() + DEADLINE;
            let clients = || tmux.ask(&["list-clients", "-F", "#{client_tty}"]);
            let mut first = clients();
            while first.is_empty() {
                assert!(Instant::now() < deadline, "tmux's client never attached");
                thread::sleep(Duration::from_millis(50));
                first = clients();
            }
            // Never read: its reader is never taken.
            let mut stalled = Pty::spawn(
                &tmux.client(&["attach-session"]),
                TerminalSize::new(200, 24),
                None,
                &[("TMUX".to_owned(), String::new())],
            )
            .expect("a second tmux client should start on a pty");
            let second = loop {
                let attached = clients();
                let is_new = |client: &&str| first.lines().all(|known| known != *client);
                if let Some(second) = attached.lines().find(is_new) {
                    break second.to_owned();
                }
                assert!(
                    Instant::now() < deadline,
                    "the second client never attached"
                );
                thread::sleep(Duration::from_millis(50));
            };
            fill(Path::new(&second));
            fs::write(go, "").expect("the gate should be writable");
            // Attached until the hook is over: a client that has left is one
            // tmux no longer names.
            let _ = finishing.recv();
            let _ = stalled.kill();
        });
        let started = Instant::now();
        assert_reaches_the_pane_from("Notification", &command, Pane::Tmux(tmux));
        drop(finished);
        // Most of this is tmux starting and the hook's gate; the report
        // itself is given a fraction of a second.
        assert!(
            started.elapsed() < DEADLINE / 2,
            "the hook took {:?}",
            started.elapsed()
        );
    });
}
