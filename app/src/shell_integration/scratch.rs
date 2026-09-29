//! The scratch directory a shell launch needs, and how long it lives.
//!
//! A [`Session`] is the materialised form of a [`Launch`]: it writes the stub
//! files a shell has to read at startup, hands the caller the program and
//! environment to spawn with, and deletes the directory again when it is
//! dropped. One session per pane, and the pane holds it for exactly as long as
//! its shell runs.
//!
//! # One user's, and nobody else's
//!
//! A scratch holds more than startup stubs. `complete.in` is the command line
//! as far as the caret, rewritten every time Tab is pressed, and a command
//! line is where people type things they would not show anyone. So every
//! session's scratch sits in one root that belongs to this user and nobody
//! else:
//!
//! * `$XDG_RUNTIME_DIR/crook` when the runtime directory is set, absolute and
//!   really this user's — logind makes it 0700 for each login, on a tmpfs of
//!   its own, and removes it at logout;
//! * otherwise `crook-<uid>` in the temporary directory: usually `/tmp` on
//!   Linux, and the per-user `$TMPDIR` on macOS;
//! * on Windows, `crook-shell-integration` in the temporary directory, which
//!   is the user's own there already.
//!
//! The root is made 0700 in one step, never made and then narrowed. One that
//! is already there is looked at with `lstat` and used only when it is a
//! directory — not a link to one — that this user owns and nobody else can
//! enter, because `/tmp` is shared and a name in it is anyone's to take first.
//! A [`Root`] is a root that passed, and the per-pane directories and the
//! [`sweep`] act only on one. Inside it every directory is 0700 and every file
//! Crook writes is 0600, written under another name and renamed into place.
//!
//! Everything here is best-effort by design. A temporary directory that cannot
//! be created, a root that has to be refused, a file that cannot be written, a
//! removal that fails — each of them costs the marks for one pane and nothing
//! else. A person opening a terminal must never be shown an error because a
//! scratch directory was difficult.

#[cfg(unix)]
use std::ffi::OsStr;
use std::ffi::OsString;
#[cfg(unix)]
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crook_terminal::{Program, TerminalOptions, default_shell};

use super::launch::{HostEnv, Launch, ScratchFile, plain, plan};
use super::{Options, PANE_ID_VARIABLE, Shell, opted_out};
use crate::control::SOCKET_VARIABLE;
use crate::tab::PaneId;

/// The root's name inside `XDG_RUNTIME_DIR`, which is this user's alone
/// already, so the name needs nothing added to it.
#[cfg(unix)]
const RUNTIME_ROOT: &str = "crook";

/// What the root's name in the temporary directory starts with; the uid
/// follows. `/tmp` is one directory for every user on the machine, and one name
/// for all of them would belong to whichever of them took it first.
#[cfg(unix)]
const TEMP_ROOT_PREFIX: &str = "crook-";

/// The root's name in the temporary directory on Windows, where that directory
/// is the user's own `AppData\Local\Temp` and no name in it is shared.
#[cfg(not(unix))]
const TEMP_ROOT: &str = "crook-shell-integration";

/// The mode of every directory in a scratch, the root included: the owner may
/// do anything and nobody else anything at all.
#[cfg(unix)]
const PRIVATE_DIRECTORY: u32 = 0o700;

/// The mode of every file Crook writes into a scratch.
#[cfg(unix)]
const PRIVATE_FILE: u32 = 0o600;

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
    /// Prepares a shell launch for `pane`, installing the integration when
    /// this shell has one and the user has not opted out.
    ///
    /// Never fails. Every way this can go wrong — an unrecognised shell, a
    /// temporary directory that cannot be written, a root somebody else got
    /// to first, no `HOME` to point zsh's stubs back at — produces a session
    /// that starts the user's shell exactly as Crook did before any of this
    /// existed, with [`Self::marks`] false. The pane's number reaches the shell
    /// on every one of those paths too.
    pub fn open(pane: PaneId, options: &Options) -> Self {
        Self::with_host(pane, options, HostEnv::current())
    }

    /// [`Self::open`] against a stated environment rather than the process's
    /// own, so a test can spawn a real shell with a `HOME` it controls.
    pub(super) fn with_host(pane: PaneId, options: &Options, host: HostEnv) -> Self {
        Self::in_root(pane, options, host, &root())
    }

    /// [`Self::with_host`] with the scratch under `root` rather than under this
    /// user's own, so a test can hand it a root that has to be refused.
    pub(super) fn in_root(pane: PaneId, options: &Options, host: HostEnv, root: &Path) -> Self {
        let program = options
            .shell
            .clone()
            .unwrap_or_else(|| PathBuf::from(default_shell()));
        let shell = Shell::of(&program);

        if !options.enabled || opted_out() {
            return Self::unmarked(pane, shell, &program, options);
        }

        let scratch = scratch_directory(root);
        let launch = plan(shell, &program, &scratch, options.login, &host);
        if !launch.marks() {
            return Self::unmarked(pane, shell, &program, options);
        }

        // Nothing is made, written or removed inside the root before it has
        // passed, so a refusal leaves whatever is there exactly as it was.
        let root = match Root::open(root) {
            Ok(root) => root,
            Err(error) => {
                log::warn!(
                    "could not install shell integration in {}: {error:#}; \
                     the shell will run without command marks",
                    root.display()
                );
                return Self::unmarked(pane, shell, &program, options);
            }
        };
        SWEPT.call_once(|| sweep(&root, &mine(), STALE_AFTER));

        if let Err(error) = write(&launch.files) {
            log::warn!(
                "could not install shell integration in {}: {error:#}; \
                 the shell will run without command marks",
                scratch.display()
            );
            remove(&scratch);
            return Self::unmarked(pane, shell, &program, options);
        }

        Self::from_launch(pane, shell, launch, Some(scratch), options)
    }

    /// The shell this launch is for.
    pub fn shell(&self) -> Shell {
        self.shell
    }

    /// Whether the shell will emit OSC 133 marks.
    ///
    /// False is a supported state, not a failure: an unrecognised shell, an
    /// opt-out, an unwritable temporary directory and a refused root all land
    /// here, and all of them mean the pane is a plain terminal with no command
    /// boundaries.
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

    /// The launch with no integration in it. Still a login shell, when that is
    /// what the setting says and the shell has a switch for it.
    fn unmarked(pane: PaneId, shell: Shell, program: &Path, options: &Options) -> Self {
        Self::from_launch(pane, shell, plain(program, options.login), None, options)
    }

    /// Puts a completion request — [`crate::completion::request_text`] —
    /// where the shell reads it.
    ///
    /// Inside the session's own scratch, so it is removed with everything else
    /// when the pane closes, and it is per-pane: two shells asked at the same
    /// moment answer into two different files. Written the way everything in
    /// a scratch is, 0600 and renamed into place, and here the second half is
    /// load-bearing: the request is rewritten on every Tab, possibly while the
    /// shell is still reading the last one. Rewritten in place, it could be
    /// read as empty, or as one request's number above part of the next one's
    /// line, and the shell's answer to that would be taken for an answer to
    /// the line on screen.
    ///
    /// `None` for a session with no scratch, whose shell has nothing bound to
    /// the key and would never read one.
    pub fn write_completion_request(&self, request: &str) -> Option<io::Result<()>> {
        let scratch = self.scratch.as_ref()?;
        Some(write_private(
            &scratch.join(COMPLETION_REQUEST_FILE),
            request.as_bytes(),
        ))
    }

    /// Where the shell writes its answer to the request
    /// [`Self::write_completion_request`] put there.
    pub fn completion_answer(&self) -> Option<PathBuf> {
        Some(self.scratch.as_ref()?.join(COMPLETION_ANSWER_FILE))
    }

    fn from_launch(
        pane: PaneId,
        shell: Shell,
        launch: Launch,
        scratch: Option<PathBuf>,
        options: &Options,
    ) -> Self {
        let mut environment = launch.environment;
        // Here rather than in the launch, because the launch is what a shell
        // gets and this is what a pane gets: the planners know the shell and
        // not the pane, and every session — marked, unmarked, opted out —
        // passes through this one constructor, so no path can forget it.
        environment.push((PANE_ID_VARIABLE.to_owned(), pane.as_u64().to_string()));
        // Set when there is no socket too, and set empty: the variable is
        // otherwise inherited, and a Crook started in another Crook's pane
        // would hand its shells a window they are not in. A path that is not
        // UTF-8 cannot be put in the environment this crate builds, and is no
        // socket as far as the shell can tell.
        let socket = options
            .control_socket
            .as_deref()
            .and_then(Path::to_str)
            .unwrap_or_default();
        environment.push((SOCKET_VARIABLE.to_owned(), socket.to_owned()));
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

/// The directory every session's scratch directory sits in: this user's own.
/// See [`root_for`].
#[cfg(unix)]
fn root() -> PathBuf {
    root_for(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        &std::env::temp_dir(),
        uid(),
    )
}

/// The directory every session's scratch directory sits in: [`TEMP_ROOT`] in
/// the temporary directory, which on Windows is the user's own already.
#[cfg(not(unix))]
fn root() -> PathBuf {
    std::env::temp_dir().join(TEMP_ROOT)
}

/// Where the root is for a process running as `me`, given what
/// `XDG_RUNTIME_DIR` says and where the temporary directory is.
///
/// In the runtime directory when that is really this user's: set, absolute —
/// the base directory specification says a relative one is to be ignored —
/// and a directory, not a link to one, that `me` owns. It is the better place
/// on every count: logind makes it 0700 for each login and removes it at
/// logout, and it is a tmpfs of its own rather than a `/tmp` that a build can
/// fill. Otherwise `crook-<uid>` in the temporary directory, so that two users
/// on one machine are never handed the same name.
///
/// Where the root is decides nothing about whether it is used: [`Root::open`]
/// checks it either way.
#[cfg(unix)]
pub(super) fn root_for(runtime: Option<&OsStr>, temp: &Path, me: u32) -> PathBuf {
    use std::os::unix::fs::MetadataExt as _;

    let runtime = runtime.map(Path::new).filter(|runtime| {
        runtime.is_absolute()
            && fs::symlink_metadata(runtime).is_ok_and(|found| found.is_dir() && found.uid() == me)
    });
    match runtime {
        Some(runtime) => runtime.join(RUNTIME_ROOT),
        None => temp.join(format!("{TEMP_ROOT_PREFIX}{me}")),
    }
}

/// The uid this process runs as — the effective one, since that is who owns
/// whatever it creates.
#[cfg(unix)]
fn uid() -> u32 {
    // Declared rather than depended on, the way `attach_to_parent_console`
    // declares its one kernel32 call: the C library is linked into every Unix
    // target already. `geteuid` takes nothing, touches no memory and cannot
    // fail, which is what makes declaring it `safe` true, and `uid_t` is 32
    // bits on every Unix Crook builds for.
    unsafe extern "C" {
        safe fn geteuid() -> u32;
    }
    geteuid()
}

/// A root that has been checked: a directory, not a link to one, that this
/// user owns and nobody else can enter.
///
/// [`Root::open`] is the only way to have one, and the two things that act on
/// what is inside a root wait for it: a pane's directory is made only after
/// one has been opened, and [`sweep`] takes one rather than a path. The sweep
/// is where that matters most. It deletes, and pointed through a link another
/// user planted where the root should be, it would delete week-old
/// directories wherever the link led.
#[derive(Debug)]
pub(super) struct Root(PathBuf);

impl Root {
    /// The root at `path`: made, private from its first moment, if it is not
    /// there, and checked if it is.
    #[cfg(unix)]
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        Self::open_as(path, uid())
    }

    /// The root at `path`, made if it is not there.
    ///
    /// Nothing is checked on Windows: the temporary directory is per user
    /// there, so there is no shared name for anyone to take first, and a mode
    /// is not how Windows says who may read a file.
    #[cfg(not(unix))]
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        fs::create_dir_all(path)?;
        Ok(Self(path.to_path_buf()))
    }

    /// [`Self::open`] for a process running as `me`: how a test asks about a
    /// directory another user owns, which it has no way to make.
    #[cfg(unix)]
    pub(super) fn open_as(path: &Path, me: u32) -> io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt as _;

        // Made with its mode rather than made and then narrowed, so there is
        // no moment when it exists and is open. Not recursive: the parent is
        // the runtime or the temporary directory, which is there, and a
        // recursive builder would give the same mode to any parent it made.
        match fs::DirBuilder::new().mode(PRIVATE_DIRECTORY).create(path) {
            Ok(()) => {}
            // An earlier pane's or an earlier Crook's — or a name somebody
            // else took first, which is what the check below is for.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        // `lstat`, not `stat`: a link is refused for being a link, not judged
        // by the directory its maker pointed it at.
        match Refusal::of(&fs::symlink_metadata(path)?, me) {
            None => Ok(Self(path.to_path_buf())),
            Some(refusal) => Err(io::Error::new(io::ErrorKind::PermissionDenied, refusal)),
        }
    }

    /// Where the root is.
    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

/// Why a root that was already there is not one to keep a pane's files in.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// It is a symbolic link, and whoever made it chose where it leads.
    Link,
    /// It is a file, a socket, a pipe — anything but a directory.
    NotADirectory,
    /// Another user owns it, with this uid, and can read whatever is put in
    /// it and swap it for something else.
    Owner(u32),
    /// Other users can list or enter it; these are its permission bits.
    Open(u32),
}

#[cfg(unix)]
impl Refusal {
    /// Why what `lstat` said about a root makes it no root for a process
    /// running as `me`, or `None` when it is one.
    fn of(found: &fs::Metadata, me: u32) -> Option<Self> {
        use std::os::unix::fs::MetadataExt as _;

        let kind = found.file_type();
        let permissions = found.mode() & 0o777;
        if kind.is_symlink() {
            Some(Self::Link)
        } else if !kind.is_dir() {
            Some(Self::NotADirectory)
        } else if found.uid() != me {
            Some(Self::Owner(found.uid()))
        } else if permissions & !PRIVATE_DIRECTORY != 0 {
            Some(Self::Open(permissions))
        } else {
            None
        }
    }
}

#[cfg(unix)]
impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Link => formatter.write_str("it is a symbolic link, which could lead anywhere"),
            Self::NotADirectory => formatter.write_str("it is not a directory"),
            Self::Owner(uid) => write!(formatter, "it belongs to another user, uid {uid}"),
            Self::Open(mode) => write!(
                formatter,
                "other users can enter it: its mode is {mode:o}, and it has to be 700"
            ),
        }
    }
}

#[cfg(unix)]
impl std::error::Error for Refusal {}

/// The prefix this process's own scratch directories carry.
fn mine() -> String {
    format!("{}-", std::process::id())
}

/// A directory name in `root` no other session, in this process or another,
/// will pick.
fn scratch_directory(root: &Path) -> PathBuf {
    let session = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
    root.join(format!("{}{session}", mine()))
}

/// Writes a launch's files, making whatever directories they sit in.
fn write(files: &[ScratchFile]) -> io::Result<()> {
    for file in files {
        if let Some(parent) = file.path.parent() {
            private_directories(parent)?;
        }
        write_private(&file.path, file.contents.as_bytes())?;
    }
    Ok(())
}

/// Makes `path`, and whichever of its parents are missing, closed to everyone
/// but this user.
///
/// Recursive, because fish's file sits two directories below the session's,
/// and every directory the builder makes gets the mode. One that is there
/// already is left as it is: inside a root that has passed, only Crook can
/// have made it.
fn private_directories(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, PRIVATE_DIRECTORY);
    builder.create(path)
}

/// Writes `contents` to `path` the way every file Crook puts in a scratch is
/// written: readable by this user alone, and never found half written.
///
/// The bytes go to a file beside it first, created 0600, and that file is
/// renamed over `path`, so whoever opens `path` gets either the last whole file
/// or this one — and a reader that already had the last one open keeps reading
/// all of it.
fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    // A leading dot and a suffix, so the half-written file is never one a
    // shell picks up by its name: fish sources every `*.fish` in
    // `vendor_conf.d`, and zsh reads dotfiles out of its `ZDOTDIR` by name.
    let mut name = OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(".partial");
    let partial = path.with_file_name(name);

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, PRIVATE_FILE);

    let written = options
        .open(&partial)
        .and_then(|mut file| file.write_all(contents))
        .and_then(|()| fs::rename(&partial, path));
    if written.is_err() {
        let _ = fs::remove_file(&partial);
    }
    written
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
/// whatever their age. The root is a [`Root`] rather than a path because this
/// deletes, and a root that has not been checked could be a link to anywhere.
pub(super) fn sweep(root: &Root, mine: &str, stale_after: Duration) {
    let Ok(entries) = fs::read_dir(root.path()) else {
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
