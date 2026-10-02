//! The one question Crook asks a forge, through the person's own `gh`.
//!
//! A row knows a pull request's address because the agent that opened it
//! said so, over its own terminal — see [`crook_terminal::agent`]. What it
//! does not know is whether that pull request is still open and whether its
//! checks passed, and nothing on this machine does: the answer is on the
//! forge. So "Check pull request" on the row's menu asks, and this is the
//! asking — `gh pr view <url> --json state,isDraft,statusCheckRollup`, run on the
//! background pool under a deadline, once per press.
//!
//! # Why the person's `gh`, and why only on a press
//!
//! Crook holds no token and speaks to no API. `gh` is already signed in as
//! the person, already knows which host their Enterprise server is on, and
//! already has the error messages for every way that can go wrong; a second
//! client inside a terminal would be a second place a person's credentials
//! live. And nothing asks unless a person pressed: the README's rule is that
//! Crook asks the network for nothing until it is asked to, with no timer and
//! no background poll, and a pull request whose checks are still running is
//! exactly the thing a poll would be built for. The answer stays on the card
//! until the next press says something newer.
//!
//! # The deadline
//!
//! `git::worktree`'s shape, for its reason: a call on the pool that can hang
//! — a network that answers nothing, a proxy that holds the connection open —
//! would hold a pool worker for as long as it hangs. So `gh` is killed at
//! [`TIMEOUT`], its two pipes are read on threads of their own so a full one
//! cannot wedge it, and a reader still blocked a moment after `gh` is gone is
//! abandoned rather than joined. `gh` runs no hooks, so what could hold its
//! pipes past its own death is rarer here than it is for `git`; the grace
//! costs nothing when it is not needed.
//!
//! # Finding `gh`
//!
//! On the `PATH` Crook was started with, and then where `gh` is usually
//! installed — see [`usual_places`]. The second look is not a nicety: a
//! `Crook.app` opened from the Dock or Finder is started by launchd with
//! `/usr/bin:/bin:/usr/sbin:/sbin` and nothing else, and every way of
//! installing `gh` on a Mac — Homebrew, gh's own package, MacPorts — puts it
//! outside that. `git` never had the problem because macOS keeps one in
//! `/usr/bin`; `gh` is the first program Crook runs from its own process that
//! is not there. The pane's shells have the person's whole `PATH` because
//! they are login shells; this process is not one, and does not run one to
//! find out — a profile that prints, prompts or takes seconds is a profile a
//! press would be waiting on.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::process::command;

/// How long `gh` may take before it is killed.
///
/// A timeout is not a performance budget. One API call is a second or two on
/// an ordinary line; this is where "slow" has become "not coming", and short
/// enough that a person who pressed and is looking at "Checking…" is not left
/// looking at it.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// How often a running `gh` is asked whether it has finished.
const POLL: Duration = Duration::from_millis(20);

/// How long a pipe's reader is given once `gh` has been reaped. See
/// `git::worktree`'s `DRAIN_GRACE`, which this is for the same reason.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// The longest line of `gh`'s own a failure carries to the card, in
/// characters.
const SAID_CHARS: usize = 160;

/// What a pull request is: open, a draft, merged, or closed without being
/// merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Open, and ready for review.
    Open,
    /// Open, and marked as a draft: not asking for a review yet, and not
    /// mergeable until it is marked ready.
    Draft,
    /// Merged.
    Merged,
    /// Closed without being merged.
    Closed,
}

impl State {
    /// The word the card says it with.
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Draft => "Draft",
            Self::Merged => "Merged",
            Self::Closed => "Closed",
        }
    }
}

/// How a pull request's checks stand, counted.
///
/// Counts rather than one verdict, because "failing" is only worth reading
/// with how many: one flaky job of twenty is a different afternoon from all
/// of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Checks {
    /// Finished, and nothing that blocks a merge: success, neutral, skipped.
    pub passing: usize,
    /// Finished, and failed — timed out, cancelled and needing action
    /// included, since each of those stops a merge the same way.
    pub failing: usize,
    /// Queued or still running.
    pub pending: usize,
}

impl Checks {
    /// How many there are.
    pub fn total(self) -> usize {
        self.passing + self.failing + self.pending
    }

    /// The half of the card's line about the checks.
    ///
    /// The worst news first: a check that failed is the one thing on the
    /// list that will not change by waiting, so it is said even while others
    /// are still running.
    pub fn summary(self) -> String {
        let total = self.total();
        let noun = if total == 1 { "check" } else { "checks" };
        if total == 0 {
            "no checks".to_owned()
        } else if self.failing > 0 {
            format!("{} of {total} {noun} failing", self.failing)
        } else if self.pending > 0 {
            format!("{} of {total} {noun} pending", self.pending)
        } else {
            format!("{total} {noun} passing")
        }
    }
}

/// What the forge said about a pull request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    /// Open, a draft, merged or closed.
    pub state: State,
    /// Its checks, counted.
    pub checks: Checks,
}

impl Found {
    /// The line the card prints: `Open · 1 of 4 checks failing`.
    pub fn summary(self) -> String {
        format!("{} \u{b7} {}", self.state.label(), self.checks.summary())
    }
}

/// Why a check did not come back with an answer.
///
/// The three a person can do something about each have a variant of their
/// own, because each has a different thing to do; the rest is what `gh` said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckError {
    /// There is no `gh` to ask: none on the `PATH` Crook was started with,
    /// and none in [`usual_places`].
    Missing,
    /// `gh` is installed and not signed in, or its token went bad.
    SignedOut,
    /// `gh` could not reach the forge.
    Offline,
    /// `gh` did not finish in time and was killed.
    TimedOut(Duration),
    /// `gh` failed for another reason, and this is its first line about it.
    Refused(String),
    /// `gh` answered, with something that is not the shape it answers in.
    Unreadable,
    /// `gh` could not be started, for a reason other than not being there.
    CouldNotRun(String),
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Not "not installed": this process's `PATH` is not the person's,
            // and a `gh` somewhere unusual is installed all the same.
            Self::Missing => f.write_str("GitHub CLI (gh) was not found on Crook's PATH"),
            Self::SignedOut => f.write_str("gh is not signed in: run `gh auth login`"),
            Self::Offline => f.write_str("gh could not reach GitHub: is there a network?"),
            Self::TimedOut(after) => {
                write!(f, "gh did not answer within {} s", after.as_secs())
            }
            Self::Refused(said) => f.write_str(said),
            Self::Unreadable => f.write_str("gh answered with something this cannot read"),
            Self::CouldNotRun(why) => write!(f, "gh could not be started: {why}"),
        }
    }
}

/// Asks `gh` how the pull request at `url` stands, killing it at `timeout`.
///
/// `gh` is the program's name — looked for on `PATH`, and then in
/// [`usual_places`], so that it is the person's own however Crook was
/// started — or a path, which is how a test hands it a fake and which is
/// looked for nowhere else. Blocking, for up to `timeout` a run: call it on
/// the background pool.
pub fn check(gh: &str, url: &str, timeout: Duration) -> Result<Found, CheckError> {
    check_in(gh, &usual_places(), url, timeout)
}

/// [`check`], with the directories a bare name is looked for in after
/// `PATH` given rather than [`usual_places`].
fn check_in(
    gh: &str,
    places: &[PathBuf],
    url: &str,
    timeout: Duration,
) -> Result<Found, CheckError> {
    let arguments = [
        "pr",
        "view",
        url,
        "--json",
        "state,isDraft,statusCheckRollup",
    ];
    let finished = match run(gh, &arguments, timeout) {
        Err(CheckError::Missing) => {
            let found = elsewhere(gh, places).ok_or(CheckError::Missing)?;
            run(&found, &arguments, timeout)?
        }
        finished => finished?,
    };
    if !finished.success {
        return Err(refused(finished.code, &finished.stderr));
    }
    read(&finished.stdout)
}

/// Where `gh` is looked for once the `PATH` Crook was started with has none.
///
/// The directories the ways of installing it put it in, which a login shell
/// adds to `PATH` and launchd does not. On a Mac: Homebrew's two prefixes,
/// Apple silicon's and Intel's — the second is also where gh's own package
/// installs it — and MacPorts'. Elsewhere on Unix: `/usr/local/bin` and
/// Homebrew on Linux's. Both: `~/.local/bin`, where a binary put in by hand
/// goes, and a Nix profile's. Windows has none: gh's installer puts it on the
/// system `PATH`, which an application started from Explorer is handed.
pub fn usual_places() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        let system: &[&str] = if cfg!(target_os = "macos") {
            &["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"]
        } else {
            &["/usr/local/bin", "/home/linuxbrew/.linuxbrew/bin"]
        };
        let mut places: Vec<PathBuf> = system.iter().map(PathBuf::from).collect();
        if let Some(home) = std::env::home_dir() {
            places.push(home.join(".local").join("bin"));
            places.push(home.join(".nix-profile").join("bin"));
        }
        places
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

/// `program` in the first of `places` that holds one that can be run, when
/// `program` is a bare name. A path names one program, and is not looked for
/// anywhere else.
fn elsewhere(program: &str, places: &[PathBuf]) -> Option<String> {
    let mut parts = Path::new(program).components();
    let bare = matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(_)), None)
    );
    if !bare {
        return None;
    }
    places
        .iter()
        .map(|place| place.join(program))
        .find(|candidate| runnable(candidate))
        .and_then(|found| found.to_str().map(str::to_owned))
}

/// Whether `path` is a file with an execute bit for anybody.
#[cfg(unix)]
fn runnable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .is_ok_and(|data| data.is_file() && data.permissions().mode() & 0o111 != 0)
}

/// Everywhere else, being a file is being runnable.
#[cfg(not(unix))]
fn runnable(path: &Path) -> bool {
    path.is_file()
}

/// What one `gh` invocation produced.
struct Finished {
    /// Whether it exited zero.
    success: bool,
    /// Its exit code, when it had one — a kill by signal has none.
    code: Option<i32>,
    /// stdout, as bytes, because it is JSON.
    stdout: Vec<u8>,
    /// stderr, as text, because failures are classified out of it.
    stderr: String,
}

/// Runs `program` with `args` and waits for it, killing it at `timeout`.
///
/// The deadline is the whole call's: waiting and the readers after it come
/// out of one budget, but for the grace the readers get once `gh` is gone.
fn run(program: &str, args: &[&str], timeout: Duration) -> Result<Finished, CheckError> {
    let spawned = command(program)
        .args(args)
        // A question with nobody at the keyboard to answer anything: no
        // prompt, no spinner and no "a new release of gh is available",
        // which is a second request to the network nobody pressed for.
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_SPINNER_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CheckError::Missing);
        }
        Err(error) => return Err(CheckError::CouldNotRun(error.to_string())),
    };

    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => return Err(CheckError::CouldNotRun(error.to_string())),
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            // Reaped as well as killed: a process nobody waits for stays a
            // zombie for the life of the window.
            let _ = child.kill();
            let _ = child.wait();
            log::warn!(
                "gh did not answer within {}s and was killed",
                timeout.as_secs()
            );
            return Err(CheckError::TimedOut(timeout));
        }
        std::thread::sleep(POLL.min(left));
    };

    let drained_by = Instant::now() + DRAIN_GRACE;
    let (Some(stdout), Some(stderr)) = (collect(stdout, drained_by), collect(stderr, drained_by))
    else {
        // Something `gh` left behind holds a pipe open. What arrived is not
        // all of what was said, and a fragment of JSON is no answer.
        return Err(CheckError::TimedOut(timeout));
    };
    Ok(Finished {
        success: status.success(),
        code: status.code(),
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Reads a pipe to the end on a thread of its own, answering through a
/// channel, which can be given up on where a join cannot.
fn drain<R: Read + Send + 'static>(mut pipe: R) -> Receiver<Vec<u8>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // Whatever arrived before a failed read is kept: the exit status is
        // what decides whether the output is worth trusting.
        let _ = pipe.read_to_end(&mut bytes);
        // Fails only when `run` gave up and dropped the receiver, which is
        // what lets this thread end rather than block on a send.
        let _ = sender.send(bytes);
    });
    receiver
}

/// The bytes one [`drain`] collected, or `None` if it was still reading at
/// `deadline` and has been abandoned.
fn collect(reader: Option<Receiver<Vec<u8>>>, deadline: Instant) -> Option<Vec<u8>> {
    let Some(reader) = reader else {
        return Some(Vec::new());
    };
    match reader.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(bytes) => Some(bytes),
        Err(RecvTimeoutError::Disconnected) => Some(Vec::new()),
        Err(RecvTimeoutError::Timeout) => None,
    }
}

/// Which failure `gh` exiting with `code` and saying `stderr` was.
///
/// By what it says, because that is the one thing gh keeps stable across
/// versions and hosts: exit 4 is its own code for "authentication required",
/// `gh auth login` is in every sentence about a missing or bad token, and a
/// network that is not there is `error connecting to` in gh's words or a
/// resolver's or dialer's in Go's.
fn refused(code: Option<i32>, stderr: &str) -> CheckError {
    let lowered = stderr.to_ascii_lowercase();
    if code == Some(4) || lowered.contains("gh auth login") {
        return CheckError::SignedOut;
    }
    const OFFLINE: [&str; 8] = [
        "error connecting to",
        "check your internet connection",
        "dial tcp",
        "no such host",
        "network is unreachable",
        "connection refused",
        "i/o timeout",
        "tls handshake timeout",
    ];
    if OFFLINE.iter().any(|said| lowered.contains(said)) {
        return CheckError::Offline;
    }
    match stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
        Some(line) => {
            let mut said: String = line.chars().take(SAID_CHARS).collect();
            if line.chars().count() > SAID_CHARS {
                said.push('\u{2026}');
            }
            CheckError::Refused(said)
        }
        None => CheckError::Refused(match code {
            Some(code) => format!("gh exited with status {code} and said nothing"),
            None => "gh was stopped before it said anything".to_owned(),
        }),
    }
}

/// What `gh pr view --json state,isDraft,statusCheckRollup` printed, read.
///
/// A state this does not know is not guessed at: gh has said the three for
/// as long as it has had `--json`, and one it adds later is a line on the
/// card saying so rather than a wrong word.
///
/// A draft is not a fourth state to gh — it is `OPEN` with `isDraft` set — and
/// read as plain `Open` it told a person whose agent opened a draft that the
/// pull request was waiting on reviewers it had never asked. A missing
/// `isDraft` is not one.
fn read(stdout: &[u8]) -> Result<Found, CheckError> {
    let view: Value = serde_json::from_slice(stdout).map_err(|_| CheckError::Unreadable)?;
    let draft = view.get("isDraft").and_then(Value::as_bool) == Some(true);
    let state = match view.get("state").and_then(Value::as_str) {
        Some("OPEN") if draft => State::Draft,
        Some("OPEN") => State::Open,
        Some("MERGED") => State::Merged,
        Some("CLOSED") => State::Closed,
        _ => return Err(CheckError::Unreadable),
    };
    let mut checks = Checks::default();
    let listed = view.get("statusCheckRollup").and_then(Value::as_array);
    for check in listed.into_iter().flatten() {
        match verdict(check) {
            Verdict::Passing => checks.passing += 1,
            Verdict::Failing => checks.failing += 1,
            Verdict::Pending => checks.pending += 1,
        }
    }
    Ok(Found { state, checks })
}

/// Where one check stands.
enum Verdict {
    Passing,
    Failing,
    Pending,
}

/// One entry of `statusCheckRollup`, judged.
///
/// Two kinds share the list. A commit status — the older API, what an
/// outside CI posts — has one `state`. A check run has a `status` that is
/// `COMPLETED` once it has finished, and only then a `conclusion` worth
/// reading; GitHub's own rule is that neutral and skipped do not block a
/// merge, so they are passing here too.
fn verdict(check: &Value) -> Verdict {
    let field = |name| check.get(name).and_then(Value::as_str).unwrap_or_default();
    if field("__typename") == "StatusContext" || check.get("context").is_some() {
        return match field("state") {
            "SUCCESS" => Verdict::Passing,
            "FAILURE" | "ERROR" => Verdict::Failing,
            _ => Verdict::Pending,
        };
    }
    if field("status") != "COMPLETED" {
        return Verdict::Pending;
    }
    match field("conclusion") {
        "SUCCESS" | "NEUTRAL" | "SKIPPED" => Verdict::Passing,
        "" => Verdict::Pending,
        _ => Verdict::Failing,
    }
}

#[cfg(test)]
#[path = "forge_tests.rs"]
mod tests;
