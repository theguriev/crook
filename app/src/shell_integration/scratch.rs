//! The scratch directory a shell launch needs, and how long it lives.
//!
//! A [`Session`] is the materialised form of a [`Launch`]: it writes the stub
//! files a shell has to read at startup, hands the caller the program and
//! environment to spawn with, and deletes the directory again when it is
//! dropped. One session per pane, and the pane holds it for exactly as long as
//! its shell runs.
//!
//! Everything here is best-effort by design. A temporary directory that cannot
//! be created, a file that cannot be written, a removal that fails — each of
//! them costs the marks for one pane and nothing else. A person opening a
//! terminal must never be shown an error because a scratch directory was
//! difficult.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crook_terminal::{Program, TerminalOptions, default_shell};

use super::launch::{HostEnv, Launch, ScratchFile, plain, plan};
use super::{OPT_OUT_VARIABLE, Options, Shell};

/// The one directory under the platform's temporary directory that every
/// session's scratch directory lives in, so a sweep has one place to look.
const SCRATCH_ROOT: &str = "crook-shell-integration";

/// How old a scratch directory belonging to no live session may get before a
/// later Crook removes it.
///
/// A week, and not an hour, because a shell can outlive its startup by a long
/// way and still read its startup files again: `exec zsh` re-reads `$ZDOTDIR`,
/// and a zsh whose `ZDOTDIR` had been swept out from under it would start with
/// none of the user's configuration rather than merely none of the marks. A
/// week of crashed sessions is a few kilobytes.
const STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Distinguishes the scratch directories of panes opened by the same process.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);

/// The sweep runs once per process, not once per pane.
static SWEPT: Once = Once::new();

/// The variable that tells the snippet where its scratch directory is.
///
/// The only channel that reaches it: the directory's name is minted per
/// session, so nothing in the snippet can know it without being told.
const COMPLETION_DIRECTORY_VAR: &str = "CROOK_SCRATCH";

/// The file a completion request is written to, inside that directory.
///
/// Two lines: the cursor's byte offset into the line, then the line itself.
/// A file rather than an escape sequence because a command line can hold a
/// semicolon, a newline and bytes that are not UTF-8, and every one of them
/// would have to be escaped past a shell *and* past an OSC parser.
const COMPLETION_REQUEST_FILE: &str = "complete.in";

/// The file the shell writes its answer to: one candidate per line.
const COMPLETION_ANSWER_FILE: &str = "complete.out";

/// A shell launch with its scratch files on disk.
///
/// Holding one is what keeps the directory alive: dropping it removes the
/// directory, so the value belongs to whatever owns the pane's shell and must
/// not be dropped while that shell is still starting.
#[derive(Debug)]
pub struct Session {
    shell: Shell,
    program: Program,
    environment: Vec<(String, String)>,
    scratch: Option<PathBuf>,
}

impl Session {
    /// Prepares a shell launch, installing the integration when this shell has
    /// one and the user has not opted out.
    ///
    /// Never fails. Every way this can go wrong — an unrecognised shell, a
    /// temporary directory that cannot be written, no `HOME` to point zsh's
    /// stubs back at — produces a session that starts the user's shell exactly
    /// as Crook did before any of this existed, with [`Self::marks`] false.
    pub fn open(options: &Options) -> Self {
        Self::with_host(options, HostEnv::current())
    }

    /// [`Self::open`] against a stated environment rather than the process's
    /// own, so a test can spawn a real shell with a `HOME` it controls.
    pub(super) fn with_host(options: &Options, host: HostEnv) -> Self {
        let program = options
            .shell
            .clone()
            .unwrap_or_else(|| PathBuf::from(default_shell()));
        let shell = Shell::of(&program);

        if !options.enabled || opted_out() {
            return Self::unmarked(shell, &program);
        }

        SWEPT.call_once(|| sweep(&root(), &mine(), STALE_AFTER));

        let scratch = scratch_directory();
        let launch = plan(shell, &program, &scratch, &host);
        if !launch.marks() {
            return Self::unmarked(shell, &program);
        }

        if let Err(error) = write(&launch.files) {
            log::warn!(
                "could not install shell integration in {}: {error:#}; \
                 the shell will run without command marks",
                scratch.display()
            );
            remove(&scratch);
            return Self::unmarked(shell, &program);
        }

        Self::from_launch(shell, launch, Some(scratch))
    }

    /// The shell this launch is for.
    pub fn shell(&self) -> Shell {
        self.shell
    }

    /// Whether the shell will emit OSC 133 marks.
    ///
    /// False is a supported state, not a failure: an unrecognised shell, an
    /// opt-out and an unwritable temporary directory all land here, and all
    /// three mean the pane is a plain terminal with no command boundaries.
    pub fn marks(&self) -> bool {
        self.scratch.is_some()
    }

    /// What to run, and with which arguments.
    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Environment variables to apply on top of the inherited ones.
    pub fn environment(&self) -> &[(String, String)] {
        &self.environment
    }

    /// The directory holding this session's stub files, while there is one.
    pub fn scratch(&self) -> Option<&Path> {
        self.scratch.as_deref()
    }

    /// Fills in the parts of a terminal's options this decided, leaving the
    /// size, the working directory and the palette to the caller.
    pub fn apply(&self, options: &mut TerminalOptions) {
        options.program = self.program.clone();
        options.environment.extend(self.environment.iter().cloned());
    }

    /// The launch with no integration in it.
    fn unmarked(shell: Shell, program: &Path) -> Self {
        Self::from_launch(shell, plain(program), None)
    }

    /// Where the shell reads a completion request from and writes its answer.
    ///
    /// Inside the session's own scratch, so it is removed with everything else
    /// when the pane closes, and it is per-pane: two shells asked at the same
    /// moment answer into two different files.
    pub fn completion_request(&self) -> Option<PathBuf> {
        Some(self.scratch.as_ref()?.join(COMPLETION_REQUEST_FILE))
    }

    /// Where the answer lands. See [`Self::completion_request`].
    pub fn completion_answer(&self) -> Option<PathBuf> {
        Some(self.scratch.as_ref()?.join(COMPLETION_ANSWER_FILE))
    }

    fn from_launch(shell: Shell, launch: Launch, scratch: Option<PathBuf>) -> Self {
        let mut environment = launch.environment;
        // The snippet has to be told where to look, and an environment
        // variable is the only channel that reaches it: the file it reads is
        // in a directory whose name is minted per session.
        if let Some(scratch) = scratch.as_ref().and_then(|path| path.to_str()) {
            environment.push((COMPLETION_DIRECTORY_VAR.to_owned(), scratch.to_owned()));
        }

        Self {
            shell,
            program: launch.program,
            environment,
            scratch,
        }
    }
}

impl Drop for Session {
    /// Removes the scratch directory. This is the path that covers a pane being
    /// closed, a window being closed and Crook quitting — everything short of
    /// the process dying without running any more code, which the sweep in this
    /// module covers instead.
    fn drop(&mut self) {
        if let Some(scratch) = &self.scratch {
            remove(scratch);
        }
    }
}

/// Whether the environment says to leave the user's shell alone.
///
/// The escape hatch for the case where the setting cannot be reached: a shell
/// that will not start under injection, a machine where the temporary directory
/// is on a filesystem mounted `noexec`, a person who simply does not want it.
/// Checked here rather than folded into [`Options`] so that no caller can
/// forget it.
fn opted_out() -> bool {
    std::env::var_os(OPT_OUT_VARIABLE).is_some_and(|value| !value.is_empty() && value != "0")
}

/// The directory every session's scratch directory sits in.
fn root() -> PathBuf {
    std::env::temp_dir().join(SCRATCH_ROOT)
}

/// The prefix this process's own scratch directories carry.
fn mine() -> String {
    format!("{}-", std::process::id())
}

/// A directory name no other session, in this process or another, will pick.
fn scratch_directory() -> PathBuf {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    root().join(format!("{}{session}", mine()))
}

/// Writes a launch's files, creating whatever directories they sit in.
fn write(files: &[ScratchFile]) -> io::Result<()> {
    for file in files {
        if let Some(parent) = file.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&file.path, &file.contents)?;
    }
    Ok(())
}

/// Removes a scratch directory, saying so only when it failed for a reason
/// other than the directory already being gone.
fn remove(scratch: &Path) {
    match fs::remove_dir_all(scratch) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => log::warn!(
            "could not remove the shell integration directory {}: {error}",
            scratch.display()
        ),
    }
}

/// Removes scratch directories left behind by sessions that are gone.
///
/// [`Session::drop`] is what normally removes a directory, and it does not run
/// when the process is killed outright — `SIGKILL`, a panic that aborts, the
/// machine losing power. What is left then is a directory of four small text
/// files that nothing will ever read again. This is what makes that harmless
/// rather than an accumulation: the next Crook to start deletes every scratch
/// directory that is not its own and has not been touched for `stale_after`.
///
/// `mine` is the prefix of this process's own directories, which are skipped
/// whatever their age.
pub(super) fn sweep(root: &Path, mine: &str, stale_after: Duration) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(mine) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= stale_after);
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}
