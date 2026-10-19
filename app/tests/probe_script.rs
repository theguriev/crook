//! `script/probe-permission-hooks`, which a person runs against a live agent,
//! so no test runs it end to end. What it does without an agent is tested
//! here: the variant it leaves for the next hook, what it registers for
//! Codex and the line it prints to start Codex, and what `--clean` leaves
//! behind. That is nothing that a later agent would run, and nothing of the
//! probe's in what Claude Code keeps for the project: its directories under
//! `~/.claude` (`projects`, `session-env`, `file-history`), and under
//! `claude-<uid>` in the temp directories `CLAUDE_CODE_TMPDIR`, `TMPDIR` and
//! a session's `scratchpad_dir` name. Claude Code's last resort, `/tmp`,
//! which the script tries as well, is not checked: only a real
//! `/tmp/claude-<uid>` would show it.
//!
//! Each case runs it with `XDG_RUNTIME_DIR`, `HOME`, `TMPDIR` and
//! `CLAUDE_CODE_TMPDIR` in a scratch directory of its own under Cargo's
//! target directory, so it never finds a real probe project or a real
//! `~/.claude`, and looks in the real `/tmp/claude-<uid>` only for names no
//! project but the scratch one has. Not under the system temp directory: the
//! script refuses `/tmp`, whose quota cargo builds fill on the machine it was
//! written on.
//!
//! Unix only: the script is `sh`, and so is the recorder it installs.

#![cfg(unix)]

use std::ffi::OsString;
use std::io::Write as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use crook::process::command;
use serde_json::Value;

/// How long a case waits for something the script started or ended, on a
/// machine that may be building five branches at once.
const PATIENCE: Duration = Duration::from_secs(10);

/// One session id, in the shape Claude Code's are.
const SESSION: &str = "1df93b55-4180-45bd-8056-b10e1afdded3";

/// What one run of the script did.
struct Verdict {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// A scratch directory holding the runtime directory the probe projects go
/// in and the home directory the script sees. Removed when it drops, after
/// any process still running from it is killed.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    /// A scratch directory whose path holds `label`.
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let scratch = Self { root };
        for dir in [
            scratch.runtime(),
            scratch.home(),
            scratch.tmpdir(),
            scratch.claude_code_tmpdir(),
        ] {
            std::fs::create_dir_all(dir).expect("the target directory is writable");
        }
        scratch
    }

    fn runtime(&self) -> PathBuf {
        self.root.join("run")
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// The `TMPDIR` the script sees, as macOS sets one for every user.
    fn tmpdir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// The `CLAUDE_CODE_TMPDIR` the script sees, which Claude Code puts its
    /// temp directory in ahead of `TMPDIR`.
    fn claude_code_tmpdir(&self) -> PathBuf {
        self.root.join("claude-code-tmp")
    }

    /// The `claude-<uid>` directory Claude Code keeps its sessions' files in
    /// within the temp directory `temp`, for the uid this test runs as: the
    /// owner of the scratch directory it made.
    fn claude_code_temp(&self, temp: &Path) -> PathBuf {
        let uid = std::fs::metadata(&self.root)
            .expect("the scratch directory is there")
            .uid();
        temp.join(format!("claude-{uid}"))
    }

    /// The throwaway project the script makes for `agent`.
    fn project(&self, agent: &str) -> PathBuf {
        self.runtime().join(format!("crook-probe-{agent}"))
    }

    /// Runs `script/probe-permission-hooks <args>`.
    fn probe(&self, args: &[&str]) -> Verdict {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/probe-permission-hooks");
        let output = command("/bin/sh")
            .envs(self.environment())
            .arg(script)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("sh runs the script");
        Verdict {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Runs the script and asserts that it succeeded.
    fn probe_ok(&self, args: &[&str]) -> Verdict {
        let verdict = self.probe(args);
        assert_eq!(
            verdict.code,
            Some(0),
            "probe-permission-hooks {args:?}:\n{}{}",
            verdict.stdout,
            verdict.stderr
        );
        verdict
    }

    /// Starts the Claude Code project's recorder the way Claude Code runs a
    /// PermissionRequest hook, handed an empty payload. It then does what
    /// the variant says.
    fn permission_hook(&self) -> Child {
        let record = self.project("claude").join(".probe/record");
        let mut hook = command("/bin/sh")
            .envs(self.environment())
            .arg(record)
            .args(["claude", "PermissionRequest"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sh runs the recorder");
        hook.stdin
            .take()
            .expect("the hook's stdin is piped")
            .write_all(b"{}\n")
            .expect("the recorder reads its payload");
        hook
    }

    /// The scratch directories in place of the real ones, and each of the
    /// probe's own settings and the other temp directories empty, which it
    /// reads as unset, whatever the environment of whoever runs the tests
    /// sets them to.
    fn environment(&self) -> Vec<(&'static str, OsString)> {
        let mut environment = vec![
            ("XDG_RUNTIME_DIR", self.runtime().into_os_string()),
            ("HOME", self.home().into_os_string()),
            ("TMPDIR", self.tmpdir().into_os_string()),
            (
                "CLAUDE_CODE_TMPDIR",
                self.claude_code_tmpdir().into_os_string(),
            ),
        ];
        for name in [
            "TMP",
            "TEMP",
            "PROBE_DIR",
            "PROBE_TIMEOUT",
            "PROBE_SECOND",
            "PROBE_HOLD",
            "PROBE_AFTER",
            "PROBE_DECISION",
            "PROBE_TRAP",
            "PROBE_MESSAGE",
        ] {
            environment.push((name, OsString::new()));
        }
        environment
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Whatever a failed case left running from here: a recorder and its
        // watcher, which would otherwise hold for minutes.
        let _ = command("pkill")
            .args(["-KILL", "-f"])
            .arg(&self.root)
            .status();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Whether a process whose command line holds `pattern` is running, as
/// `pgrep -f` matches it.
fn running(pattern: &Path) -> bool {
    command("pgrep")
        .arg("-f")
        .arg(pattern)
        .stdout(Stdio::null())
        .status()
        .expect("pgrep runs")
        .success()
}

/// Waits up to [`PATIENCE`] for `done`, and says whether it came.
fn within_patience(mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < PATIENCE {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    done()
}

/// The name Claude Code 2.1.280 gives a project's directory under
/// `~/.claude/projects` and under `claude-<uid>` in its temp directory: its
/// path with every character but an ASCII letter or digit made a `-`, by
/// `replace(/[^a-zA-Z0-9]/g,"-")`.
fn claude_code_name(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A variant the script refuses part of the way through is no variant: the
/// next hook does what the last one said, not the pairs before the bad one
/// with the environment's defaults after them.
#[test]
fn a_refused_variant_leaves_the_last_one_as_it_was() {
    let scratch = Scratch::new("probe-variant");
    scratch.probe_ok(&["claude"]);
    let variant = scratch.project("claude").join(".probe/variant");
    scratch.probe_ok(&["claude", "variant", "HOLD=10", "DECISION=allow"]);
    let last = std::fs::read_to_string(&variant).expect("the variant is written");
    assert_eq!("PROBE_HOLD=10\nPROBE_DECISION=allow\n", last);

    let refused = scratch.probe(&["claude", "variant", "HOLD=60", "DECISION=alow"]);
    assert_eq!(
        Some(1),
        refused.code,
        "{}{}",
        refused.stdout,
        refused.stderr
    );
    assert!(
        refused.stderr.contains("not alow"),
        "the refusal says why: {}",
        refused.stderr
    );
    assert_eq!(
        last,
        std::fs::read_to_string(&variant).expect("the variant is still there"),
        "a refused variant changed the last one"
    );
    let left: Vec<_> = std::fs::read_dir(scratch.project("claude").join(".probe"))
        .expect("the probe's directory lists")
        .map(|entry| entry.expect("an entry").file_name())
        .filter(|name| name.to_string_lossy().starts_with("variant") && name != "variant")
        .collect();
    assert!(left.is_empty(), "a refused variant left {left:?} behind");
}

/// `--clean` promises that no recorder is left for an agent to run, and a
/// recorder held with `TRAP=stay` logs a TERM and holds on, as its watcher
/// does. Both have to be gone once `--clean` has said it removed the project.
#[test]
fn clean_ends_a_held_recorder_that_stays_on_a_term_and_its_watcher() {
    let scratch = Scratch::new("probe-clean-processes");
    scratch.probe_ok(&["claude"]);
    scratch.probe_ok(&["claude", "variant", "HOLD=60", "TRAP=stay"]);
    let record = scratch.project("claude").join(".probe/record");
    let mut watch = record.as_os_str().to_owned();
    watch.push(" --watch");
    let watch = PathBuf::from(watch);

    let mut hook = scratch.permission_hook();
    assert!(
        within_patience(|| running(&watch)),
        "the recorder never started its watcher"
    );
    scratch.probe_ok(&["claude", "--clean"]);

    let mut ended = false;
    let gone = within_patience(|| {
        ended = ended
            || hook
                .try_wait()
                .expect("the hook can be waited for")
                .is_some();
        ended && !running(&record)
    });
    assert!(
        ended,
        "the held recorder outlived --clean, which said it removed the project"
    );
    assert!(gone, "a watcher or child of the recorder outlived --clean");
}

/// Claude Code keeps the probe's conversations and session state under a
/// name of the project's path in which an `_` is a `-` as well as a `/` or a
/// `.`, and on macOS under the real path its sessions report, beneath
/// `/private`. It keeps their scratchpads and task output under that name in
/// its temp directory, which is `CLAUDE_CODE_TMPDIR` or else `TMPDIR`, a
/// directory under `/var/folders` on macOS, and which the session's
/// `scratchpad_dir` names when the session ran with another. `--clean` has to
/// find them by those names in each of those places, and leave the projects
/// that are not the probe's.
#[test]
fn clean_removes_what_claude_code_keeps_for_the_project_by_the_names_it_gives_them() {
    let scratch = Scratch::new("probe_clean_state");
    scratch.probe_ok(&["claude"]);
    let project = scratch.project("claude");
    let project = project.to_str().expect("the scratch path is text");
    assert!(project.contains('_'), "{project} has no _ to be renamed");

    // Two sessions, as SessionStart hands them to the recorder: one in the
    // project as the script named it, with no scratchpad, as under `-p`; and
    // one as macOS's getcwd names a directory under $TMPDIR, whose
    // scratchpad is in a temp directory the script's environment does not
    // name.
    let real = format!("/private{project}");
    let session_temp = scratch.claude_code_temp(&scratch.root.join("session-tmp"));
    let scratchpad = session_temp
        .join(claude_code_name(&real))
        .join(SESSION)
        .join("scratchpad");
    let evidence = Path::new(project).join("evidence");
    for (at, cwd, pad) in [(1, project, None), (2, real.as_str(), Some(&scratchpad))] {
        let transcript = scratch
            .home()
            .join(".claude/projects")
            .join(claude_code_name(cwd))
            .join(format!("{SESSION}.jsonl"));
        let mut payload = serde_json::json!({
            "session_id": SESSION,
            "transcript_path": transcript,
            "cwd": cwd,
            "hook_event_name": "SessionStart",
            "source": "startup",
        });
        if let Some(pad) = pad {
            payload["scratchpad_dir"] = serde_json::json!(pad);
        }
        std::fs::write(
            evidence.join(format!(
                "179215262{at}.405199817-411178{at}-SessionStart.json"
            )),
            payload.to_string(),
        )
        .expect("the evidence directory is writable");
    }

    let claude = scratch.home().join(".claude");
    let tmpdir = scratch.claude_code_temp(&scratch.tmpdir());
    let probes = [
        claude.join("projects").join(claude_code_name(project)),
        claude.join("projects").join(claude_code_name(&real)),
        claude.join("session-env").join(SESSION),
        claude.join("file-history").join(SESSION),
        scratch
            .claude_code_temp(&scratch.claude_code_tmpdir())
            .join(claude_code_name(project)),
        tmpdir.join(claude_code_name(&real)),
        scratchpad.clone(),
    ];
    let others = [
        claude.join("projects/-home-someone-work-crook"),
        tmpdir.join("-home-someone-work-crook"),
        session_temp.join("-home-someone-work-crook"),
    ];
    for dir in probes.iter().chain(&others) {
        std::fs::create_dir_all(dir).expect("the scratch directory is writable");
        std::fs::write(dir.join("kept.jsonl"), "{}\n").expect("the scratch directory is writable");
    }

    scratch.probe_ok(&["claude", "--clean"]);
    for dir in &probes {
        assert!(!dir.exists(), "--clean left {}", dir.display());
    }
    assert!(
        !session_temp.join(claude_code_name(&real)).exists(),
        "--clean left the session's project directory around its scratchpad"
    );
    assert!(!Path::new(project).exists(), "--clean left the project");
    for other in &others {
        assert!(
            other.join("kept.jsonl").exists(),
            "--clean removed {}, which is not the probe's",
            other.display()
        );
    }
}

/// Codex keeps the trust it is given in its `CODEX_HOME`, so the probe's
/// hooks go in a `CODEX_HOME` of the project's own, and the line that
/// starts Codex says so. None go in the project's `.codex`, which only a
/// trust kept in `~/.codex` would load.
#[test]
fn codex_gets_a_codex_home_of_the_projects_own() {
    let scratch = Scratch::new("probe-codex-home");
    let setup = scratch.probe_ok(&["codex"]);
    let project = scratch.project("codex");
    let home = project.join(".probe/codex-home");
    // The whole line, since the login line below it holds `CODEX_HOME=<home>
    // codex` too.
    let start = format!(
        "  cd {} && CODEX_HOME={} codex",
        project.display(),
        home.display()
    );
    assert!(
        setup.stdout.lines().any(|line| line == start),
        "no line starts Codex with its CODEX_HOME set, as {start:?} does: {}",
        setup.stdout
    );

    let hooks: Value = serde_json::from_str(
        &std::fs::read_to_string(home.join("hooks.json")).expect("the hooks are in CODEX_HOME"),
    )
    .expect("hooks.json is JSON");
    let permission = &hooks["hooks"]["PermissionRequest"][0]["hooks"][0]["command"];
    assert!(
        permission
            .as_str()
            .is_some_and(|command| command.contains(".probe/record")),
        "the PermissionRequest hook is not the recorder: {hooks}"
    );
    assert!(
        !project.join(".codex").exists(),
        "the project has a .codex of its own"
    );

    scratch.probe_ok(&["codex", "--clean"]);
    assert!(
        !project.exists(),
        "--clean left the project and its CODEX_HOME"
    );
}
