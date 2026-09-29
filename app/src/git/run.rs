//! Running git under a deadline, for every part of the git module that has
//! to wait on it.
//!
//! [`super::worktree`] grew this, because two of its commands write and a
//! write runs the repository's own hooks, which are arbitrary code somebody
//! else wrote. [`super::merged`] needs the same promise for another reason —
//! it reads a history, and a history's honest length has no bound the way a
//! listing's does — so the runner lives here and both spawn git through it.
//!
//! The deadline bounds the *call*, not only git. Killing a process does not
//! reach what it left behind: a hook that backgrounds a helper — `direnv
//! reload &`, a file watcher, a dev server — hands that helper the two pipes
//! git was writing down, and they stay open for as long as it lives, so
//! reading them to the end can outlive git by hours. The readers therefore run
//! under the same deadline as git itself and are abandoned at it; `collect`
//! says what abandoning one costs and why it is the cheaper of the two prices.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::process::command;

/// Why git could not be asked, as opposed to what it answered.
///
/// The four ways a call ends without an exit status to read. Each caller
/// turns them into its own error — [`super::worktree::Error`] has a variant
/// for every one — because what a UI can offer to do about a missing git or
/// a deadline depends on what it had asked for.
#[derive(Debug)]
pub(super) enum Failure {
    /// There is no `git` on `PATH`. Latched, so nothing spawns again this
    /// session.
    GitMissing,
    /// git could not be started, or could not be waited for.
    CouldNotRun(std::io::Error),
    /// git was still running at the deadline and was killed.
    TimedOut {
        /// The deadline it outlived.
        after: Duration,
    },
    /// The directory it was to run in is not there, which
    /// [`super::worktree`] reports as not being a repository.
    NoDirectory,
}

/// Set the first time git turns out not to be on `PATH`.
///
/// The same latch [`super::diff`] keeps, for the same reason: without it a
/// machine with no git spawns a doomed process on every attempt for the life of
/// the session. The two are read together and set separately — whichever half
/// discovers it first spares the other — and deliberately not shared, because
/// sharing would mean one module exporting a setter the other calls, and this
/// module is worth keeping self-contained.
static GIT_MISSING: AtomicBool = AtomicBool::new(false);

/// How long a read may take before git is killed.
///
/// A timeout is not a performance budget; it is the thing that stops a wedged
/// git from holding a pool worker until Crook exits. It has to be far longer
/// than any honest run — listing a hundred checkouts is milliseconds, and the
/// only reason a local read is slow is a cold page cache — and short enough
/// that a stall is something a person waits out rather than a hang they have to
/// restart the app to clear.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long [`super::worktree::add`] may take.
///
/// Twelve times the read budget, for one reason: `add` writes out a whole
/// working tree. On a large repository on a slow disk that is genuinely tens of
/// seconds, and then it runs the repository's `post-checkout` hook, which is
/// whatever the user wrote. Killing an honest checkout halfway leaves a
/// half-written directory *and* a registered worktree — strictly worse than
/// having waited.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

/// How long [`super::worktree::remove`] may take.
///
/// Longer again, because what a removal does is delete a directory tree and
/// the tree is as big as whatever was built in it — a `target/` of forty
/// gigabytes is an afternoon's work on this machine's own checkouts, and an
/// `rm` of that on a spinning disk is minutes, not the two the write budget
/// allows. Killing git halfway through it is the worst of every outcome: the
/// files are half gone, the worktree is still registered, and `git status`
/// on what is left reports every deleted file as a modification, so the sweep
/// that tried to tidy it away will from then on refuse to touch it. Fifteen
/// minutes is not a performance budget either; it is where a removal that has
/// not finished has stopped being one.
const DELETE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// How long the deadline is checked at [`EAGER_POLL`] before it drops to
/// [`LAZY_POLL`].
///
/// git answers a local question in 5-50ms, so the common case is over before
/// the interval widens and pays no measurable latency for the poll; a call that
/// is already past fifty milliseconds is not made faster by asking it a
/// thousand times a second.
const EAGER_POLLING: Duration = Duration::from_millis(50);
/// The poll interval while a call is still young.
const EAGER_POLL: Duration = Duration::from_millis(1);
/// The poll interval once it is not.
const LAZY_POLL: Duration = Duration::from_millis(20);

/// Whether git has already been found to be missing from this machine.
///
/// Reads [`super::diff`]'s latch too: a session that has already discovered
/// there is no git while drawing a tab row need not discover it again here.
pub(super) fn git_is_missing() -> bool {
    GIT_MISSING.load(Ordering::Relaxed) || super::diff::git_is_missing()
}

/// Whether an invocation only reads.
///
/// One enum for two decisions, because they are the same decision: how long a
/// call may take, and whether it is allowed to take git's locks.
#[derive(Copy, Clone)]
pub(super) enum Intent {
    /// `worktree list`, `status`: answers a question and changes nothing.
    Read,
    /// `worktree add`, `lock`, `unlock`: writes the repository.
    Write,
    /// `worktree remove`: writes the repository, and deletes a directory tree
    /// first, which is the one thing here whose honest duration has no bound
    /// a number can promise. See [`DELETE_TIMEOUT`].
    Delete,
}

impl Intent {
    /// The deadline this kind of call gets.
    fn timeout(self) -> Duration {
        match self {
            Self::Read => READ_TIMEOUT,
            Self::Write => write_timeout(),
            Self::Delete => DELETE_TIMEOUT,
        }
    }
}

/// [`WRITE_TIMEOUT`], which nothing outside a test build can change.
#[cfg(not(test))]
fn write_timeout() -> Duration {
    WRITE_TIMEOUT
}

/// The write deadline the calling thread is running under.
///
/// The one behaviour here that cannot be demonstrated any other way is [`run`]
/// returning *at* its deadline when something a hook started is holding the
/// pipes: proving it means outliving the deadline, and two minutes is not a
/// thing to spend in a suite that runs in seconds. So the tests can shorten it.
///
/// Thread-local rather than a global, because the suite runs its tests in
/// parallel threads and a global would shorten the deadline for every write
/// test running beside the one that asked. `run` waits on the thread that
/// called it, so a thread-local is exactly the scope of one test.
#[cfg(test)]
fn write_timeout() -> Duration {
    WRITE_DEADLINE.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// See [`write_timeout`].
    pub(super) static WRITE_DEADLINE: std::cell::Cell<Duration> =
        const { std::cell::Cell::new(WRITE_TIMEOUT) };
}

/// What one git invocation produced.
pub(super) struct Finished {
    /// Whether git exited zero.
    pub(super) success: bool,
    /// The code it exited with, for the commands whose non-zero exit is an
    /// answer — `check-ignore` exits 1 for "none of them". `None` when a
    /// signal ended it.
    pub(super) code: Option<i32>,
    /// stdout, as bytes, because paths come out of it.
    pub(super) stdout: Vec<u8>,
    /// stderr, as text, because messages come out of it.
    pub(super) stderr: String,
}

/// Runs `git <args>` in `directory` and waits for it under [`Intent`]'s
/// deadline.
///
/// The deadline is the whole call's, not git's. Spawning, waiting and reading
/// both pipes to the end all come out of the one budget, because the last of
/// those can outlive git by as long as whatever a hook backgrounded cares to
/// live — see `collect`.
pub(super) fn run(directory: &Path, args: &[&OsStr], intent: Intent) -> Result<Finished, Failure> {
    run_feeding(directory, args, intent, None)
}

/// [`run`], with `input` written to git's stdin when there is one — and stdin
/// closed at once when there is not, so nothing can wait on it.
pub(super) fn run_feeding(
    directory: &Path,
    args: &[&OsStr],
    intent: Intent,
    input: Option<Vec<u8>>,
) -> Result<Finished, Failure> {
    if git_is_missing() {
        return Err(Failure::GitMissing);
    }
    // A `current_dir` that does not exist makes `spawn` fail with `NotFound` —
    // the same error kind as a missing `git` binary. Checking first is what
    // keeps the latch in `start` honest: without it, one call against a
    // directory somebody had just deleted would switch git off for the rest
    // of the session.
    if !directory.is_dir() {
        return Err(Failure::NoDirectory);
    }

    let stdin = if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    let mut child = start(directory, args, intent, [stdin, Stdio::piped(), Stdio::piped()])?;

    // Both pipes are drained on their own threads. Polling `try_wait` with the
    // output left unread deadlocks the moment git writes more than a pipe
    // buffer — `status --ignored` in a repository with a fat `target/` does
    // exactly that — and the deadlock would then surface as a timeout, which is
    // a lie about what happened.
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);
    if let (Some(input), Some(stdin)) = (input, child.stdin.take()) {
        feed(stdin, input);
    }

    let timeout = intent.timeout();
    let deadline = Instant::now() + timeout;
    let waited = wait_for(&mut child, deadline, timeout);

    // Collected against a grace that starts *here*, and pointedly not joined.
    // A join has no deadline to give, and the thread on the other end of one
    // can be blocked for ever: killing git closes git's handles on the pipes
    // and nothing else's, so anything git left running still holds the write
    // end and `read_to_end` still has no end to read to.
    //
    // The grace is short and does not come out of the command's own budget,
    // because by this line git has been reaped. Everything it was ever going
    // to write is already in the pipe, at most a buffer of it, and a reader
    // that is going to finish finishes in microseconds. A reader still blocked
    // after a moment is blocked on somebody else's descriptor and will be
    // blocked on it just as much two minutes later — so spending the rest of a
    // 120-second write budget on it parks a pool worker for two minutes to
    // learn what a second already said.
    let drained_by = (Instant::now() + DRAIN_GRACE).min(deadline);
    let stdout = collect(stdout, drained_by);
    let stderr = collect(stderr, drained_by);

    let status = waited?;
    if !status.success() {
        log::debug!(
            "git {args:?} in {} exited {:?}",
            directory.display(),
            status.code()
        );
    }

    // Whether an abandoned reader has cost this call its answer depends on
    // which pipe it was and on what git did, and the rule is the narrow one:
    // never report a command as having failed when it did not, and never hand
    // back a fragment of an answer as though it were the answer.
    let lost = if status.success() {
        // Only a read is its output. A write is its *effect* — `worktree add`
        // that exited zero has made the checkout, and saying otherwise because
        // a daemon the hook started still holds a pipe would be telling a
        // person the directory they are looking at is not there. Its stdout is
        // never parsed, and its stderr only ever matters when it failed.
        matches!(intent, Intent::Read) && stdout.is_none()
    } else {
        // Every failure is classified out of stderr, and classifying a
        // fragment of one names the wrong error or, more often, none at all.
        stderr.is_none()
    };
    if lost {
        log::warn!(
            "git {args:?} in {} exited but left its output held open past {}s",
            directory.display(),
            timeout.as_secs()
        );
        return Err(Failure::TimedOut { after: timeout });
    }

    Ok(Finished {
        success: status.success(),
        code: status.code(),
        stdout: stdout.unwrap_or_default(),
        stderr: String::from_utf8_lossy(&stderr.unwrap_or_default()).into_owned(),
    })
}

/// Starts `git <args>` in `directory`, set up the way every call here is,
/// with `stdio` as its stdin, stdout and stderr.
///
/// Latches [`GIT_MISSING`] when there is no git to start.
fn start(
    directory: &Path,
    args: &[&OsStr],
    intent: Intent,
    stdio: [Stdio; 3],
) -> Result<Child, Failure> {
    let mut git = command("git");

    if matches!(intent, Intent::Read) {
        // `diff`'s treatment, for `diff`'s reason: a background read must not
        // take `.git/index.lock`, or it races the user's own commit, and must
        // not rewrite the index as a side effect of refreshing it. The
        // environment variable carries the same rule into anything git itself
        // spawns.
        //
        // A write gets neither, on purpose. `add` and `remove` *must* take that
        // lock: it is how git stops two writers interleaving, and a write that
        // skipped it would corrupt the thing the flag exists to protect.
        git.arg("--no-optional-locks")
            .args(["-c", "diff.autoRefreshIndex=false"])
            .env("GIT_OPTIONAL_LOCKS", "0");
    }

    git.args(args)
        .current_dir(directory)
        // No command here touches a remote — `add` is handed an explicit branch
        // and start point precisely so nothing can decide to go and fetch one —
        // so there is no credential to be asked for. These two make that a
        // guarantee rather than an argument: git may not prompt on a terminal,
        // and has no terminal on stdin to prompt on — a pipe, when there is
        // input, is no terminal either.
        .env("GIT_TERMINAL_PROMPT", "0");

    let [stdin, stdout, stderr] = stdio;
    git.stdin(stdin).stdout(stdout).stderr(stderr);
    match git.spawn() {
        Ok(child) => Ok(child),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !GIT_MISSING.swap(true, Ordering::Relaxed) {
                log::warn!("git is not on PATH; worktrees are off for this session");
            }
            Err(Failure::GitMissing)
        }
        Err(error) => Err(Failure::CouldNotRun(error)),
    }
}

/// Runs `git <upstream> | git <downstream>` in `directory`, as a read, and
/// waits for both under one deadline.
///
/// For a read that is a stream through a filter — `log -p` into `patch-id` —
/// where handing the second command the first one's output from here would
/// mean holding all of it, and a history's patches are as many megabytes as
/// the history is long. The two are joined by a pipe of their own, and
/// nothing passes through this process but what the second one prints.
///
/// A `timeout` rather than an [`Intent`], because a pipe here only ever
/// reads and it is the caller that knows how long its read deserves.
///
/// `success` is both commands', and `stderr` is the second one's. The first
/// one's is thrown away: the only thing a caller does with a failed pipe is
/// not believe it, and a reader thread for text nobody reads is a thread for
/// nothing.
pub(super) fn pipe(
    directory: &Path,
    upstream: &[&OsStr],
    downstream: &[&OsStr],
    timeout: Duration,
) -> Result<Finished, Failure> {
    if git_is_missing() {
        return Err(Failure::GitMissing);
    }
    // See `run`: a directory that is not there must not read as a missing git.
    if !directory.is_dir() {
        return Err(Failure::NoDirectory);
    }

    let mut first = start(
        directory,
        upstream,
        Intent::Read,
        [Stdio::null(), Stdio::piped(), Stdio::null()],
    )?;

    // Handed over rather than lent: the command that starts the second git
    // holds this process's copy of the pipe's read end until it is dropped,
    // which `start` does on its way out. While that copy is open the first
    // git never learns that the second has stopped reading — it blocks on a
    // full pipe until the deadline kills it, rather than ending at once on a
    // write nobody will take.
    let feed = first.stdout.take().map_or_else(Stdio::null, Stdio::from);
    let second = start(
        directory,
        downstream,
        Intent::Read,
        [feed, Stdio::piped(), Stdio::piped()],
    );
    let mut second = match second {
        Ok(child) => child,
        Err(failure) => {
            let _ = first.kill();
            let _ = first.wait();
            return Err(failure);
        }
    };

    let stdout = second.stdout.take().map(drain);
    let stderr = second.stderr.take().map(drain);

    let deadline = Instant::now() + timeout;
    let waited = wait_for(&mut first, deadline, timeout).and_then(|first| {
        wait_for(&mut second, deadline, timeout).map(|second| first.success() && second.success())
    });
    if waited.is_err() {
        // The first was killed at the deadline, or the second was and this is
        // a no-op on a process already reaped. Either way the second is not
        // left running on half an input, and not left a zombie.
        let _ = second.kill();
        let _ = second.wait();
    }

    // `run`'s grace, for `run`'s reason: both have been reaped by here.
    let drained_by = (Instant::now() + DRAIN_GRACE).min(deadline);
    let stdout = collect(stdout, drained_by);
    let stderr = collect(stderr, drained_by);

    let success = waited?;
    // A read is its output, and a fragment of one is not an answer.
    let Some(stdout) = stdout else {
        log::warn!(
            "git {upstream:?} | git {downstream:?} in {} left its output held open past {}s",
            directory.display(),
            timeout.as_secs()
        );
        return Err(Failure::TimedOut { after: timeout });
    };

    Ok(Finished {
        success,
        stdout,
        stderr: String::from_utf8_lossy(&stderr.unwrap_or_default()).into_owned(),
    })
}

/// Reads a pipe to the end on a thread of its own, answering through a channel
/// rather than through a join.
///
/// The channel is the difference between having a deadline and promising one.
/// `read_to_end` returns when the *last* handle on the pipe's write end closes,
/// and git's is not necessarily the last one: a `post-checkout` hook that ran
/// `direnv reload &` handed the same two descriptors to something that is still
/// running, and killing git does not touch it. A join would then wait on this
/// thread for as long as that process lives; a channel can be given up on.
fn drain<R: std::io::Read + Send + 'static>(mut pipe: R) -> Receiver<Vec<u8>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // A read that fails keeps whatever arrived before it. There is nothing
        // better to do with the error: the exit status is what decides whether
        // the output is worth trusting.
        let _ = pipe.read_to_end(&mut bytes);
        // Ignored because the only way this fails is `run` having given up and
        // dropped the receiver, which is not this thread's problem — and is
        // what lets it end rather than block on a send nobody will take.
        let _ = sender.send(bytes);
    });
    receiver
}

/// Writes `input` to git's stdin on a thread of its own, then closes it.
///
/// Not on the calling thread, for the deadline's sake: a write into a pipe
/// git is not reading blocks, and it would block before [`wait_for`] had
/// started counting. Not joined, for [`drain`]'s reason. Once git exits, or is
/// killed at its deadline, the pipe has no reader and the write fails rather
/// than blocks, so the thread ends with the command — nothing git runs for
/// the one command that is fed, `check-ignore`, inherits the pipe to keep it
/// open.
fn feed(mut stdin: std::process::ChildStdin, input: Vec<u8>) {
    std::thread::spawn(move || {
        use std::io::Write as _;
        // Ignored: a git that stopped reading has exited or failed, and says
        // so in its exit status, which is what the caller reads.
        let _ = stdin.write_all(&input);
    });
}

/// How long a reader is given once git itself has been reaped.
///
/// Not a share of the command's timeout: see the comment at the call. This is
/// the time a pipe's remaining buffer takes to reach a thread that is already
/// sitting in `read`, which is microseconds, with four orders of magnitude of
/// slack for a machine under load.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// The bytes one [`drain`] collected, or `None` if it was still reading at
/// `deadline`.
///
/// The argument is an `Option` because [`std::process::Child`]'s pipes are: a
/// handle somebody already took is no bytes and no waiting.
///
/// `None` back means the reader has been abandoned — left blocked in a read on
/// a pipe nothing is going to close, holding a thread, a stack and a buffer
/// until whatever inherited the write end exits, which for a daemon is never.
/// That is a leak, and it is the cheaper of the two prices. The other is
/// blocking *this* thread on the same pipe, and this thread is a worker of the
/// background pool: a leaked reader sleeps in a syscall and costs some pages,
/// where a lost pool worker costs every git read, every worktree command and
/// every other background task Crook meant to run for the rest of the session.
///
/// It is also usually temporary. The abandoned reader ends itself the instant
/// the pipe does close — its send finds the receiver gone and it returns — so a
/// hook that merely takes a while past the deadline cleans up after itself, and
/// only one that leaves something running for ever leaks for ever.
fn collect(reader: Option<Receiver<Vec<u8>>>, deadline: Instant) -> Option<Vec<u8>> {
    let Some(reader) = reader else {
        return Some(Vec::new());
    };

    let left = deadline.saturating_duration_since(Instant::now());
    match reader.recv_timeout(left) {
        Ok(bytes) => Some(bytes),
        // The thread ended without sending, which it only does by panicking.
        // Nothing arrived and nothing is coming — which is not the same as
        // still waiting, because there is no reader left to abandon.
        Err(RecvTimeoutError::Disconnected) => Some(Vec::new()),
        Err(RecvTimeoutError::Timeout) => None,
    }
}

/// Waits for `child`, killing it at `deadline`.
///
/// `timeout` is the span `deadline` was made from and is only what the error
/// and the log line report. The waiting itself is against the instant, so a
/// caller that shares one deadline between this and the reading of the pipes
/// hands out its budget once rather than twice.
fn wait_for(
    child: &mut std::process::Child,
    deadline: Instant,
    timeout: Duration,
) -> Result<std::process::ExitStatus, Failure> {
    let started = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => return Err(Failure::CouldNotRun(error)),
        }

        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            // Reaping after the kill matters as much as the kill: a process id
            // that is never waited for stays a zombie, and one that is reaped
            // by somebody else can be handed out again to a stranger.
            //
            // What killing does *not* do is close the pipes. It closes the
            // handles git itself held on them and nothing else's, and a hook
            // that backgrounded a helper gave that helper the same two
            // descriptors — so a kill can leave a reader with no end of file
            // coming and a process nothing here has a way to signal, `command`
            // putting git in no process group of its own. That is why the
            // readers are bounded by this same deadline instead; see `collect`.
            let _ = child.kill();
            let _ = child.wait();
            log::warn!(
                "git did not finish within {}s and was killed",
                timeout.as_secs()
            );
            return Err(Failure::TimedOut { after: timeout });
        }

        let interval = if started.elapsed() < EAGER_POLLING {
            EAGER_POLL
        } else {
            LAZY_POLL
        };
        std::thread::sleep(interval.min(left));
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
