//! The socket: where it lives, who can reach it, and the threads behind it.
//!
//! # Where, and who
//!
//! One socket per process, `<pid>.sock`, in a directory only this user can
//! enter: `$XDG_RUNTIME_DIR/crook-control` when that variable names an
//! absolute, real directory this user owns, and `crook-control-<uid>` in the
//! temporary directory otherwise — the per-user `$TMPDIR` on macOS. The
//! directory is `0700` and the socket `0600`. A directory that is already
//! there is used only when it is a real directory, this user's, and closed to
//! everyone else: whoever made it first could otherwise leave a socket in it
//! that answers `crook pane list` with whatever it likes. It is a directory of
//! its own rather than a corner of the shell integration's scratch, because
//! that one is swept of entries a week old and a window can be open longer.
//!
//! # A socket left behind
//!
//! A Crook that dies without running its drops leaves its socket file. At
//! startup the name this process wants is probed: a socket there that refuses
//! a connection is one of those, and is removed and taken; one that answers
//! belongs to somebody alive — another PID namespace sharing the directory —
//! and the next name is tried instead. Every other refusing socket in the
//! directory is swept once it is [`STALE_AFTER`] old, which is what keeps a
//! sweep off a socket another Crook has bound this instant and not yet started
//! listening on.
//!
//! # The threads
//!
//! One OS thread accepts. It blocks for as long as nobody connects, which is
//! the reason a pty reader has a thread of its own rather than a pool worker —
//! see `PARKED_WORKERS` in `crate` — and each connection gets a short-lived
//! thread, [`MAX_CONNECTIONS`] at once, which reads its lines against one
//! [`DEADLINE`] for the whole connection and asks the window through
//! [`Answer`]. Nothing here runs on the window's thread.
//!
//! # Connections that are kept
//!
//! A `pane.wait` with time to wait and an `events.follow` keep their
//! connection for as long as the wait or the stream lasts — see
//! [`protocol::Verb::keeps_connection`]. Such a connection gives its place
//! among the [`MAX_CONNECTIONS`] back and takes one of [`MAX_KEPT`] instead:
//! a lead waiting on each of its eight workers must not leave its own
//! `crook pane list` turned away as `busy`. Its thread blocks on a
//! [`Feed`] the window pushes into, and a second thread blocks on a read of
//! the connection, whose only use is to see the client hang up — a script
//! killed at its own timeout, an agent's tool call cut short — and end the
//! wait then rather than when its time is up. The connection answers nothing
//! else, and closes when the wait or the stream is over.
//!
//! A client that closes its half after its request — what `nc -N` does, and
//! what [`converse`] takes as the end of a request — has not hung up: it is
//! still reading. On Linux the end of what it says is told apart from its
//! leaving by `poll`, and its wait is answered; elsewhere nothing that blocks
//! tells the two apart — macOS's `poll` says `POLLHUP` for both — so there
//! the end of what the client says is taken for its leaving, and a client of
//! a kept verb keeps its half open until the reply comes. See
//! [`until_closed`].
//!
//! # An agent's question
//!
//! `agent.ask` keeps its connection too, in a lane of its own — at most
//! [`MAX_ASKS`], counted apart from the waits and streams — and only for a
//! process the kernel says is this user's. Its connection is the question's
//! whole life: the window holds it and says so, a reader on the hook's half
//! tells the window the moment the hook goes, and the one answer written is
//! a person's, or a pass when the time runs out, or `gone`. See [`ask_on`].

use std::ffi::OsString;
use std::fs::{self, DirBuilder, Permissions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use super::Notice;
use super::ask::{AskId, Cause};
use super::protocol::{
    self, Answered, Asker, Delivered, MAX_LINE, PaneEvent, Refusal, Reply, Request, Verb, Wait,
    code,
};
use super::watch::{Feed, Next};

/// The name of the directory the sockets live in, under the runtime or the
/// temporary directory.
const DIRECTORY: &str = "crook-control";

/// How long one connection has, from the moment it is accepted, to send its
/// lines and read its answers.
///
/// Five seconds, for everything but a verb that says it needs longer — see
/// [`protocol::Verb::patience`]: a `pane.list` is answered between two
/// frames, and a client that has not finished in five seconds is one that is
/// not going to. It is also the bound on how long a connection's
/// thread can be held by a client that connects and says nothing.
pub const DEADLINE: Duration = Duration::from_secs(5);

/// How much of a connection's time is kept back for its reply.
///
/// The window is asked to answer this long before the deadline rather than at
/// it. Waited on until the deadline itself, a window that did not answer
/// would be refused as `timeout` when the connection had no time left to
/// write the refusal in, and the line would close on the client with nothing
/// on it — a hang-up where the protocol promises a code.
const REPLY_MARGIN: Duration = Duration::from_millis(250);

/// How many connections are answered at once. The next is refused as `busy`.
///
/// A thread each, and only this user can connect, so the cap is not a defence
/// against anybody but a script of one's own gone into a loop — which should
/// cost it a refusal rather than the machine a thousand threads.
pub const MAX_CONNECTIONS: usize = 8;

/// How many connections are kept open at once for a wait or a stream of
/// events, beside the [`MAX_CONNECTIONS`] answered. The next is refused as
/// `busy`.
///
/// Thirty-two: a lead waiting on every tab its budget allows, and following
/// them, is nine, and a window with a few leads is still well inside it. Each
/// is two threads — one waiting on the window, one on the client — so this is
/// what bounds them.
pub const MAX_KEPT: usize = 32;

/// How many agents' questions are held at once, apart from the
/// [`MAX_KEPT`] waits and streams. The next is refused as `busy`, and its
/// hook passes to the agent's own prompt.
///
/// Sixteen: a pane holds four — see `ask::MAX_PER_PANE` — and a window with
/// a few agents each asking is well inside it. Counted apart from the waits,
/// so that a lead waiting on every worker it may open cannot leave a
/// worker's question with nowhere to wait, nor a burst of questions a lead's
/// `crook pane wait` turned away. Each is two threads, like a wait.
pub const MAX_ASKS: usize = 16;

/// How long after a question's own wait the connection gives it before
/// telling the hook it has expired.
///
/// The hook stops reading a little after its own wait — see
/// `agent::ask::CLAUDE_AWAIT` — so this is what makes the window, which
/// knows, the side that says the question is over.
pub const ASK_MARGIN: Duration = Duration::from_secs(5);

/// The longest line a hook may send after its question, in bytes.
///
/// It sends one, `{"delivered":true}`; four kilobytes is far past it, and a
/// bound on what a client sending something else can make a thread hold.
pub const MAX_ASKER_LINE: usize = 4 * 1024;

/// How old a refusing socket must be before a starting Crook sweeps it.
///
/// A socket is refused between being bound and being listened on, which is
/// microseconds; a minute is far past that and far short of mattering.
pub const STALE_AFTER: Duration = Duration::from_secs(60);

/// How many names a process tries before it gives up: its own pid, then the
/// pid with a count after it.
const NAMES: usize = 16;

/// How long the listener rests after an accept that failed.
///
/// The one pause in this file, and only after an error: a process out of
/// descriptors fails every accept at once, and a loop that retried
/// immediately would spin a core until one was freed.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// How the window is asked: a request, the feed a kept connection waits on —
/// see [`protocol::Verb::keeps_connection`] — and the deadline the answer has
/// to arrive by. Run on a connection's thread.
pub type Answer = Arc<Ask>;

/// The question [`Answer`] asks.
pub type Ask = dyn Fn(Request, Option<Arc<Feed>>, Instant) -> Result<Value, Refusal> + Send + Sync;

/// The user this process runs as.
pub fn euid() -> u32 {
    // Declared rather than depended on, the way `attach_to_parent_console`
    // declares its kernel32 call: libc is linked into every Unix target, and
    // `uid_t` is 32 bits on Linux, macOS and the BSDs.
    unsafe extern "C" {
        safe fn geteuid() -> u32;
    }
    geteuid()
}

/// Who is at the other end of a connection, as the kernel recorded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    /// The peer's effective user.
    pub uid: u32,
    /// The peer's process, when the kernel names it: `None` for one in a pid
    /// namespace this process cannot see into, which Linux reports as 0, and
    /// for one macOS will not name.
    ///
    /// Not the same process on both systems. On Linux it is the one that
    /// made the peer's end, fixed like the uid. On macOS it is the last
    /// process to have connected, sent, received or polled on that end, so a
    /// descriptor passed to another process names that one once it uses it:
    /// there it is a hint at lineage, worth what it says when read just after
    /// the peer has spoken, and never a proof of who connected.
    pub pid: Option<u32>,
}

/// Who is at the other end of `stream`.
///
/// What the kernel wrote down when that end was made — by `listen`,
/// `connect` or `socketpair` — and not who holds it now, so it cannot be
/// claimed by anything said over the connection, and a socket somebody swaps
/// in at a name after it was looked at still names whoever bound it. That
/// holds for the uid and the pid alike here; on macOS only the uid is fixed.
#[cfg(target_os = "linux")]
pub fn peer_credentials(stream: &UnixStream) -> io::Result<Peer> {
    use std::ffi::c_void;
    use std::os::fd::AsRawFd;

    /// `SOL_SOCKET` and `SO_PEERCRED` in `<asm-generic/socket.h>`, which
    /// x86_64 and aarch64 both take theirs from. Mips, PowerPC and SPARC
    /// number them otherwise; there the call fails, or answers an `int` that
    /// the length check below refuses.
    const SOL_SOCKET: i32 = 1;
    const SO_PEERCRED: i32 = 17;

    /// `struct ucred`: a `pid_t`, a `uid_t` and a `gid_t`, 32 bits each, and
    /// laid out alike on x86_64 and aarch64.
    #[repr(C)]
    struct Ucred {
        pid: i32,
        uid: u32,
        gid: u32,
    }

    // Declared rather than depended on, as `euid` declares `geteuid`.
    // `socklen_t` is 32 bits in glibc and musl alike.
    unsafe extern "C" {
        fn getsockopt(
            socket: i32,
            level: i32,
            name: i32,
            value: *mut c_void,
            length: *mut u32,
        ) -> i32;
    }

    // No uid anyone has, so that a call which wrote nothing names nobody
    // rather than root.
    let mut credentials = Ucred {
        pid: 0,
        uid: u32::MAX,
        gid: u32::MAX,
    };
    let mut length = size_of::<Ucred>() as u32;
    // SAFETY: the value points at a `Ucred` that lives across the call and
    // the length says how big it is, which is as much as the kernel writes.
    // The descriptor is `stream`'s, open for as long as it is borrowed.
    let read = unsafe {
        getsockopt(
            stream.as_raw_fd(),
            SOL_SOCKET,
            SO_PEERCRED,
            (&raw mut credentials).cast(),
            &mut length,
        )
    };
    if read != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != size_of::<Ucred>() {
        return Err(io::Error::other(format!(
            "SO_PEERCRED answered {length} bytes, not a struct ucred's {}",
            size_of::<Ucred>()
        )));
    }
    Ok(Peer {
        uid: credentials.uid,
        pid: u32::try_from(credentials.pid).ok().filter(|pid| *pid != 0),
    })
}

/// Who is at the other end of `stream`.
///
/// Two questions on macOS, which has no `SO_PEERCRED`, and they are not
/// alike. `getpeereid` answers with the credentials the kernel wrote down
/// when that end was made — by `listen`, `connect` or `socketpair` — and not
/// who holds it now, so the uid cannot be claimed by anything said over the
/// connection, and a socket somebody swaps in at a name after it was looked
/// at still names whoever bound it. `LOCAL_PEERPID` answers with the peer
/// socket's last owner, which XNU rewrites whenever a process connects,
/// sends, receives or polls on it, so the pid is whoever used that end last;
/// [`Peer::pid`] says what that is good for. macOS has nothing better:
/// `LOCAL_PEERCRED`'s `xucred` carries no pid, and `LOCAL_PEERTOKEN` reads
/// the same last owner.
#[cfg(target_os = "macos")]
pub fn peer_credentials(stream: &UnixStream) -> io::Result<Peer> {
    use std::ffi::c_void;
    use std::os::fd::AsRawFd;

    /// `SOL_LOCAL` and `LOCAL_PEERPID` in `<sys/un.h>`.
    const SOL_LOCAL: i32 = 0;
    const LOCAL_PEERPID: i32 = 0x002;

    // Declared rather than depended on, as `euid` declares `geteuid`: both
    // are in libSystem. `uid_t`, `gid_t`, `pid_t` and `socklen_t` are 32
    // bits there.
    unsafe extern "C" {
        fn getpeereid(socket: i32, uid: *mut u32, gid: *mut u32) -> i32;
        fn getsockopt(
            socket: i32,
            level: i32,
            name: i32,
            value: *mut c_void,
            length: *mut u32,
        ) -> i32;
    }

    let socket = stream.as_raw_fd();
    // No uid anyone has, as on Linux.
    let (mut uid, mut gid) = (u32::MAX, u32::MAX);
    // SAFETY: two `u32`s that live across the call, which writes one
    // `uid_t` and one `gid_t` through them, and `stream`'s descriptor.
    if unsafe { getpeereid(socket, &mut uid, &mut gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pid: i32 = 0;
    let mut length = size_of::<i32>() as u32;
    // SAFETY: the value points at a `pid_t` that lives across the call and
    // the length says how big it is, which is as much as the kernel writes.
    let read = unsafe {
        getsockopt(
            socket,
            SOL_LOCAL,
            LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &mut length,
        )
    };
    // The user is what decides whether a socket is trusted; a process the
    // kernel would not name is one nobody can check the lineage of, which is
    // what `None` says.
    let named = read == 0 && length as usize == size_of::<i32>();
    Ok(Peer {
        uid,
        pid: u32::try_from(pid).ok().filter(|pid| named && *pid != 0),
    })
}

/// The directory this user's control sockets live in.
pub fn directory() -> PathBuf {
    directory_for(
        std::env::var_os("XDG_RUNTIME_DIR"),
        &std::env::temp_dir(),
        euid(),
    )
}

/// The same, from stated inputs, so a test can say what the environment is.
///
/// The runtime directory is taken only when it is absolute and a real
/// directory `uid` owns: the variable is inherited, and a relative one, or one
/// naming somebody else's directory, is not the per-user tmpfs the
/// specification promises.
pub fn directory_for(runtime: Option<OsString>, temp: &Path, uid: u32) -> PathBuf {
    let runtime = runtime.map(PathBuf::from).filter(|runtime| {
        runtime.is_absolute()
            && fs::symlink_metadata(runtime)
                .is_ok_and(|metadata| metadata.is_dir() && metadata.uid() == uid)
    });
    match runtime {
        Some(runtime) => runtime.join(DIRECTORY),
        None => temp.join(format!("{DIRECTORY}-{uid}")),
    }
}

/// Refuses a directory that anyone but `uid` could have put a socket in.
///
/// Read with `lstat`, so a link to a directory that would pass is refused as
/// the link it is. The CLI reads the directory through the same check before
/// it trusts a socket in it.
pub fn check(directory: &Path, uid: u32) -> io::Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    let refuse = |why: String| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} {why}", directory.display()),
        ))
    };
    if metadata.file_type().is_symlink() {
        return refuse("is a link".to_owned());
    }
    if !metadata.is_dir() {
        return refuse("is not a directory".to_owned());
    }
    if metadata.uid() != uid {
        return refuse(format!("belongs to uid {}, not {uid}", metadata.uid()));
    }
    let mode = metadata.mode() & 0o777;
    if mode & 0o077 != 0 {
        return refuse(format!("is open to other users (mode {mode:o})"));
    }
    Ok(())
}

/// Makes the directory private, or checks that the one already there is.
fn prepare(directory: &Path) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(directory) {
        // Set again after making it, because the mode asked for is masked by
        // the umask — which can only take bits away, and one that took the
        // owner's would leave a directory nobody can use.
        Ok(()) => fs::set_permissions(directory, Permissions::from_mode(0o700))?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    check(directory, euid())
}

/// A socket this process is listening on.
///
/// Dropping it stops the listener and removes the file, when the file is
/// still the one this process bound.
pub struct Socket {
    path: PathBuf,
    /// The file's identity when it was bound, so that a drop never removes a
    /// socket somebody else has since put at the same name.
    device: u64,
    inode: u64,
    closing: Arc<AtomicBool>,
}

impl Socket {
    /// Binds a socket in `directory`, making the directory if it is not there,
    /// and starts the thread that accepts on it.
    pub fn open_in(directory: &Path, answer: Answer) -> io::Result<Self> {
        prepare(directory)?;
        sweep(directory, STALE_AFTER);
        let (listener, path) = claim(directory)?;
        Self::listening(listener, path.clone(), answer).inspect_err(|_| {
            let _ = fs::remove_file(&path);
        })
    }

    /// Locks the bound socket down and starts accepting on it.
    fn listening(listener: UnixListener, path: PathBuf, answer: Answer) -> io::Result<Self> {
        // The directory is what keeps other users out; this is the second
        // lock on the same door, for a directory somebody loosens later.
        fs::set_permissions(&path, Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&path)?;
        let closing = Arc::new(AtomicBool::new(false));
        let listening = closing.clone();
        thread::Builder::new()
            .name("crook-control".to_owned())
            .spawn(move || listen(&listener, &listening, &answer))?;
        Ok(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
            closing,
        })
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Release);
        let ours = fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.dev() == self.device && metadata.ino() == self.inode);
        // A file somebody else has put at the name since is theirs: not
        // removed, and not knocked on either. The listener then stays parked
        // until the process ends, which is the moment this runs anyway.
        if !ours {
            return;
        }
        // The listener is parked in `accept`, and nothing but a connection
        // returns it from there; it reads the flag before it does anything
        // with what it accepted.
        let _ = UnixStream::connect(&self.path);
        let _ = fs::remove_file(&self.path);
    }
}

/// Binds the first name in `directory` that is free or left behind.
fn claim(directory: &Path) -> io::Result<(UnixListener, PathBuf)> {
    let pid = std::process::id();
    let mut refused = None;
    for count in 0..NAMES {
        let path = match count {
            0 => directory.join(format!("{pid}.sock")),
            count => directory.join(format!("{pid}-{count}.sock")),
        };
        match UnixListener::bind(&path) {
            Ok(listener) => return Ok((listener, path)),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                // Taken only when nobody answers there: a live socket is
                // somebody's, however much its name looks like this one's.
                if refuses(&path) && fs::remove_file(&path).is_ok() {
                    match UnixListener::bind(&path) {
                        Ok(listener) => return Ok((listener, path)),
                        Err(error) => refused = Some(error),
                    }
                } else {
                    refused = Some(error);
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(refused.unwrap_or_else(|| io::Error::other("no name to bind")))
}

/// Whether `path` is a socket nobody is listening on.
fn refuses(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
        && UnixStream::connect(path)
            .is_err_and(|error| error.kind() == io::ErrorKind::ConnectionRefused)
}

/// Removes the sockets in `directory` nobody is listening on, once they are
/// `stale_after` old.
///
/// Best-effort, like every sweep of a scratch: a socket that cannot be removed
/// is one a later Crook will try again.
pub fn sweep(directory: &Path, stale_after: Duration) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "sock") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= stale_after);
        if old && refuses(&path) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Accepts until the socket is dropped.
fn listen(listener: &UnixListener, closing: &AtomicBool, answer: &Answer) {
    let lanes = Arc::new(Lanes::default());
    for accepted in listener.incoming() {
        if closing.load(Ordering::Acquire) {
            return;
        }
        match accepted {
            Ok(stream) => admit(stream, &lanes, answer),
            Err(error) => {
                log::debug!("the control socket could not accept a connection: {error}");
                thread::sleep(ACCEPT_BACKOFF);
            }
        }
    }
}

/// Gives a connection a thread of its own, or refuses it as `busy`.
fn admit(stream: UnixStream, lanes: &Arc<Lanes>, answer: &Answer) {
    let Some(mut place) = Place::take(lanes) else {
        // Written here, on the listener: a connection just accepted has an
        // empty buffer, so a line this short cannot block.
        let _ = send(
            &stream,
            Instant::now() + DEADLINE,
            &Reply::refused(
                None,
                Refusal::new(
                    code::BUSY,
                    format!(
                        "this Crook is answering {MAX_CONNECTIONS} connections already; try again"
                    ),
                ),
            ),
        );
        return;
    };

    let deadline = Instant::now() + DEADLINE;
    let answer = answer.clone();
    let started = thread::Builder::new()
        .name("crook-control-connection".to_owned())
        .spawn(move || converse(&stream, deadline, &*answer, &mut place));
    // A thread the OS would not start took the closure with it, and the place
    // inside it has already been given back.
    if let Err(error) = started {
        log::warn!("the control socket could not answer a connection: {error}");
    }
}

/// How many connections are open, by what they are open for.
#[derive(Debug, Default)]
pub struct Lanes {
    /// Connections being answered: at most [`MAX_CONNECTIONS`].
    answered: AtomicUsize,
    /// Connections kept for a wait or a stream: at most [`MAX_KEPT`].
    kept: AtomicUsize,
    /// Connections holding an agent's question: at most [`MAX_ASKS`].
    asking: AtomicUsize,
}

/// A connection's place in one of the [`Lanes`], given back when it is
/// dropped — however the connection's thread ends.
#[derive(Debug)]
pub struct Place {
    lanes: Arc<Lanes>,
    lane: Lane,
}

/// Which of the [`Lanes`] a [`Place`] is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    /// Among the connections answered.
    Answered,
    /// Among the connections kept for a wait or a stream.
    Kept,
    /// Among the connections holding an agent's question.
    Asking,
}

impl Place {
    /// A place among the connections answered, or `None` when they are full.
    pub fn take(lanes: &Arc<Lanes>) -> Option<Self> {
        if lanes.answered.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
            lanes.answered.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Self {
            lanes: lanes.clone(),
            lane: Lane::Answered,
        })
    }

    /// Moves this place to the lane `verb` keeps its connection in, giving
    /// the one it had back, or says there is no room there: an agent's
    /// question to the questions held, and a wait or a stream to the
    /// connections kept.
    fn keep_for(&mut self, verb: &Verb) -> bool {
        let lane = match verb {
            Verb::AgentAsk(_) => Lane::Asking,
            _ => Lane::Kept,
        };
        if self.lane == lane {
            return true;
        }
        let (count, most) = self.lanes.lane(lane);
        if count.fetch_add(1, Ordering::AcqRel) >= most {
            count.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        self.lanes.lane(self.lane).0.fetch_sub(1, Ordering::AcqRel);
        self.lane = lane;
        true
    }
}

impl Lanes {
    /// The count of a lane, and how many it holds at most.
    fn lane(&self, lane: Lane) -> (&AtomicUsize, usize) {
        match lane {
            Lane::Answered => (&self.answered, MAX_CONNECTIONS),
            Lane::Kept => (&self.kept, MAX_KEPT),
            Lane::Asking => (&self.asking, MAX_ASKS),
        }
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        self.lanes.lane(self.lane).0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Answers a connection's lines, one reply a line, until it closes, sends a
/// line too long to read, or runs out of time.
///
/// A blank line is skipped rather than refused, the way a shell skips one. A
/// last line with no newline before the end is still a request: the client
/// said all it had to say and closed its half. Closing it is no hang-up, on
/// Linux, for a verb that keeps the connection either — see the module docs.
pub fn converse(stream: &UnixStream, deadline: Instant, answer: &Ask, place: &mut Place) {
    converse_as(stream, deadline, answer, place, euid());
}

/// [`converse`], with the uid an agent's question must come from given
/// rather than taken from [`euid`], so that a test can be the stranger no
/// test can connect as: the way [`check`] and the client's checks take
/// theirs.
pub(crate) fn converse_as(
    stream: &UnixStream,
    deadline: Instant,
    answer: &Ask,
    place: &mut Place,
    uid: u32,
) {
    let mut reader = BufReader::new(Timed { stream, deadline });
    loop {
        let mut line = Vec::new();
        // One byte past the cap, so that a line of exactly the cap and its
        // newline is read whole and one byte more is known to be too long.
        match (&mut reader)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let ended = line.last() == Some(&b'\n');
        if ended {
            line.pop();
        } else if line.len() > MAX_LINE {
            let refusal = Refusal::new(
                code::TOO_LONG,
                format!("a request is one line of at most {MAX_LINE} bytes"),
            );
            // And closed: the rest of the line is still coming, and there is
            // no telling where the next one would start.
            let _ = send(stream, deadline, &Reply::refused(None, refusal));
            return;
        }

        if !line.iter().all(u8::is_ascii_whitespace) {
            let (id, request) = protocol::read(&line);
            if let Ok(request) = &request
                && request.verb.keeps_connection()
            {
                match request.verb {
                    Verb::AgentAsk(_) => {
                        ask_on(stream, id, request.clone(), deadline, answer, place, uid);
                    }
                    _ => keep(stream, id, request.clone(), deadline, answer, place),
                }
                return;
            }
            // A verb that waits on git is given its own time, for this reply
            // only: the reading goes on against the connection's deadline, so
            // a line after a slow one finds that time has run out and the
            // connection closes, rather than one slow verb lending the rest
            // of the connection its own patience.
            let replying_by = match request
                .as_ref()
                .ok()
                .and_then(|asked| asked.verb.patience())
            {
                Some(patience) => deadline.max(Instant::now() + patience),
                None => deadline,
            };
            // Short of the deadline, so that a window which runs out of time
            // still has some left to say so in: see `REPLY_MARGIN`.
            let answer_by = replying_by.checked_sub(REPLY_MARGIN).unwrap_or(replying_by);
            let reply = match request {
                Ok(request) => match answer(request, None, answer_by) {
                    Ok(result) => Reply::answered(id, result),
                    Err(refusal) => Reply::refused(id, refusal),
                },
                Err(refusal) => Reply::refused(id, refusal),
            };
            if send(stream, replying_by, &reply).is_err() {
                return;
            }
        }
        if !ended {
            return;
        }
    }
}

/// Answers a verb that keeps its connection — a wait, or a stream of
/// events — and closes the connection when it is over.
///
/// The window is asked by the connection's deadline, as for any question, and
/// handed a [`Feed`] to answer into from then on. A wait's answer is the one
/// item the window pushes, or, when its time runs out first, what the window
/// says the pane is doing when asked again with no time to wait; a stream's is
/// the window's first answer and then every item as it comes.
fn keep(
    stream: &UnixStream,
    id: Option<Value>,
    request: Request,
    deadline: Instant,
    answer: &Ask,
    place: &mut Place,
) {
    let asked_at = Instant::now();
    if !place.keep_for(&request.verb) {
        let refusal = Refusal::new(
            code::BUSY,
            format!(
                "this Crook is keeping {MAX_KEPT} waits and event streams open already; try \
                 again when one has ended"
            ),
        );
        let _ = send(stream, deadline, &Reply::refused(id, refusal));
        return;
    }
    let feed = Arc::new(Feed::for_verb(&request.verb));
    watch_for_hangup(stream, &feed);

    let answer_by = deadline.checked_sub(REPLY_MARGIN).unwrap_or(deadline);
    match answer(request.clone(), Some(feed.clone()), answer_by) {
        Err(refusal) => {
            let _ = send(stream, deadline, &Reply::refused(id, refusal));
        }
        Ok(first) => match &request.verb {
            Verb::PaneWait(asked) => {
                let until = asked_at + asked.timeout();
                wait_on(stream, id, &request, asked, until, &feed, answer);
            }
            _ => stream_to(stream, id, first, deadline, &feed),
        },
    }
    feed.hang_up();
    // Ends the connection for the client, and returns the read the hang-up
    // watch is parked in.
    let _ = stream.shutdown(Shutdown::Both);
}

/// Waits for a wait's answer until `until`, and writes it.
fn wait_on(
    stream: &UnixStream,
    id: Option<Value>,
    request: &Request,
    asked: &Wait,
    until: Instant,
    feed: &Feed,
    answer: &Ask,
) {
    let result = match feed.next(Some(until)) {
        Next::Item(result) => Ok(result),
        // Where the pane is now, asked as a wait of no time: the answer a
        // script reads is then the state it was left in, not a bare
        // "timeout" it has to ask about again.
        Next::TimedOut => {
            let now = Request {
                verb: Verb::PaneWait(Wait {
                    timeout: Some(0),
                    ..asked.clone()
                }),
                token: request.token.clone(),
                asker: None,
            };
            let deadline = Instant::now() + DEADLINE;
            let answer_by = deadline.checked_sub(REPLY_MARGIN).unwrap_or(deadline);
            let result = answer(now, None, answer_by);
            let reply = match result {
                Ok(result) => Reply::answered(id, result),
                Err(refusal) => Reply::refused(id, refusal),
            };
            let _ = send(stream, deadline, &reply);
            return;
        }
        Next::Ended => Err(Refusal::new(
            code::GONE,
            "the window closed before the pane got there",
        )),
        // Nobody left to tell.
        Next::HungUp | Next::Lagged(_) => return,
    };
    let reply = match result {
        Ok(result) => Reply::answered(id, result),
        Err(refusal) => Reply::refused(id, refusal),
    };
    let _ = send(stream, Instant::now() + DEADLINE, &reply);
}

/// Writes the window's first answer, then every event the feed is given,
/// until the window ends it or the client hangs up.
///
/// A write blocks while the client is not reading, for as long as it is not:
/// that is what the feed's bound is for, and a client that hangs up fails the
/// write it is blocked in.
fn stream_to(stream: &UnixStream, id: Option<Value>, first: Value, deadline: Instant, feed: &Feed) {
    if send(stream, deadline, &Reply::answered(id, first)).is_err() {
        return;
    }
    if stream.set_write_timeout(None).is_err() {
        return;
    }
    let mut stream = stream;
    loop {
        let line = match feed.next(None) {
            Next::Item(event) => {
                let mut line = event.to_string();
                line.push('\n');
                line
            }
            Next::Lagged(dropped) => PaneEvent::Lagged { dropped }.line(),
            Next::Ended | Next::HungUp | Next::TimedOut => return,
        };
        if stream.write_all(line.as_bytes()).is_err() {
            return;
        }
    }
}

/// Holds an agent's question in the window for as long as its hook waits,
/// and writes the hook the one answer it gets — or why it gets none.
///
/// The question takes a place among the [`MAX_ASKS`] held, and is asked of
/// the window only when the kernel says the process at the other end is this
/// user's: a question is the one request whose answer could ever carry an
/// allow, and a socket somebody else reached by a loosened directory gets
/// none. Whose process it is, and whose that process's ancestors are, go to
/// the window with it, read here on the connection's thread.
///
/// Once the window holds it, `{"asked": id}` is written, and two things wait:
/// this thread on the feed, until the question's wait and [`ASK_MARGIN`] are
/// up, and a reader on the client's half — see [`hear_the_hook`] — which is
/// how the window hears the hook went the moment it did. An answer pushed in
/// time is written, and the reader is given a moment to hear the hook say it
/// printed it; a wait that runs out is a pass, `expired`, which the window is
/// told of; a window that closes is `gone`.
fn ask_on(
    stream: &UnixStream,
    id: Option<Value>,
    request: Request,
    deadline: Instant,
    answer: &Ask,
    place: &mut Place,
    uid: u32,
) {
    let asked_at = Instant::now();
    let refuse = |refusal: Refusal| {
        let _ = send(stream, deadline, &Reply::refused(id.clone(), refusal));
    };
    if !place.keep_for(&request.verb) {
        refuse(Refusal::new(
            code::BUSY,
            format!(
                "this Crook is holding {MAX_ASKS} agents' questions already; answer in the \
                 agent's own prompt"
            ),
        ));
        return;
    }
    let asker = match asker(stream, uid) {
        Ok(asker) => asker,
        Err(refusal) => {
            refuse(refusal);
            return;
        }
    };
    let Verb::AgentAsk(asked) = &request.verb else {
        return;
    };
    let until = asked_at + asked.wait() + ASK_MARGIN;
    let request = Request {
        asker: Some(asker),
        ..request
    };
    let feed = Arc::new(Feed::for_verb(&request.verb));

    let answer_by = deadline.checked_sub(REPLY_MARGIN).unwrap_or(deadline);
    let registered = match answer(request, Some(feed.clone()), answer_by) {
        Ok(registered) => registered,
        Err(refusal) => {
            feed.hang_up();
            refuse(refusal);
            return;
        }
    };
    let held = registered
        .get("asked")
        .and_then(Value::as_u64)
        .map(AskId::from_wire);
    let Some(held) = held else {
        feed.hang_up();
        return;
    };
    if send(stream, deadline, &Reply::answered(id.clone(), registered)).is_err() {
        if feed.hang_up() {
            feed.notice(Notice::Withdrawn(held));
        }
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }

    let heard = hear_the_hook(stream, &feed, held);
    match feed.next_or_close(until) {
        Next::Item(item) => {
            let mut line = item.to_string();
            line.push('\n');
            if write_line(stream, &line).is_ok() {
                // The hook's `delivered`, or its leaving, whichever is first:
                // either is the end of what this connection is for.
                let _ = heard.recv_timeout(DEADLINE);
            }
        }
        Next::TimedOut => {
            let _ = write_line(stream, &Answered::pass(Cause::Expired.word()).line());
            feed.notice(Notice::Expired(held));
        }
        // By a deadline of its own: the connection's ran out long ago, as
        // a question is held for minutes.
        Next::Ended => {
            let gone = Refusal::new(
                code::GONE,
                "the window closed before the question was answered",
            );
            let _ = send(stream, Instant::now() + DEADLINE, &Reply::refused(id, gone));
        }
        Next::HungUp | Next::Lagged(_) => {}
    }
    feed.hang_up();
    // Ends the connection for the hook, and returns the read the reader is
    // parked in.
    let _ = stream.shutdown(Shutdown::Both);
}

/// Who the process at the other end of `stream` is, when the kernel says it
/// runs as `uid`; refused as `unauthorized` otherwise.
///
/// Its lineage is read here, on the connection's thread, and only for a
/// question: a walk of up to [`MOST_ASKERS`] processes is no work for a
/// frame to do. Read now, just after the request arrived, because on macOS
/// the pid the kernel names is whoever used the connection last.
pub fn asker(stream: &UnixStream, uid: u32) -> Result<Asker, Refusal> {
    let unauthorized = |why: String| {
        Refusal::new(
            code::UNAUTHORIZED,
            format!("only a process of this window's user may ask it a question: {why}"),
        )
    };
    let peer = peer_credentials(stream)
        .map_err(|error| unauthorized(format!("the kernel would not say who asked: {error}")))?;
    if peer.uid != uid {
        return Err(unauthorized(format!(
            "the process that asked is uid {}, not {uid}",
            peer.uid
        )));
    }
    Ok(Asker {
        pid: peer.pid,
        lineage: peer.pid.map(lineage).unwrap_or_default(),
    })
}

/// How many processes up from the asker its lineage is read.
pub const MOST_ASKERS: usize = 64;

/// `pid` and its ancestors, nearest first, as far as they can be read.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn lineage(pid: u32) -> Vec<u32> {
    crate::agent::ancestors::lineage(pid, MOST_ASKERS)
}

/// Nowhere else reads a process's parent yet, so a question asked there is
/// from a process nobody can place, and is shown and never answered.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn lineage(_: u32) -> Vec<u32> {
    Vec::new()
}

/// Starts the thread that reads what a hook says after its question, and
/// tells the window: [`Notice::Delivered`] for its `{"delivered":true}`, and
/// [`Notice::Withdrawn`] — once, and only while the question was still open
/// — when it hangs up.
///
/// One line at most, of at most [`MAX_ASKER_LINE`]: after it, everything the
/// hook sends is read and let go until it leaves, so a client that sends line
/// after line wakes the window once at most rather than once a line. A line
/// too long, or one cut off by the end, is taken for the hook leaving.
///
/// Not [`watch_for_hangup`]: that one reads and lets go, and would race this
/// for the line. Whatever this hears — a delivery or the end — is sent on
/// the channel it hands back, which the connection's thread waits on after
/// writing the answer. A hook never closes only its half, so the end of what
/// it says is its leaving, on every platform.
fn hear_the_hook(stream: &UnixStream, feed: &Arc<Feed>, held: AskId) -> mpsc::Receiver<()> {
    let (heard, hearing) = mpsc::channel();
    let Ok(reading) = stream.try_clone() else {
        return hearing;
    };
    // The connection's deadline set a timeout on reads, and a read that timed
    // out would be taken for the hook leaving.
    if reading.set_read_timeout(None).is_err() {
        return hearing;
    }
    let feed = feed.clone();
    let started = thread::Builder::new()
        .name("crook-control-asker".to_owned())
        .spawn(move || {
            let mut lines = BufReader::new(&reading);
            let mut line = Vec::new();
            let read = (&mut lines)
                .take(MAX_ASKER_LINE as u64 + 1)
                .read_until(b'\n', &mut line);
            // One line, and only a whole one: a hook says it printed its
            // answer once, and whatever else it sends is let go unheard.
            if read.is_ok_and(|read| read > 0 && read <= MAX_ASKER_LINE && line.ends_with(b"\n")) {
                let said = serde_json::from_slice::<Delivered>(&line);
                if said.is_ok_and(|said| said.delivered) {
                    feed.notice(Notice::Delivered(held));
                    let _ = heard.send(());
                }
                // Then only its leaving, read for and let go in a buffer of
                // its own, so that nothing a hook sends after its one line
                // reaches the window or costs it memory.
                let mut rest = [0; 512];
                while matches!(lines.read(&mut rest), Ok(read) if read > 0) {}
            }
            if feed.hang_up() {
                feed.notice(Notice::Withdrawn(held));
            }
            let _ = heard.send(());
        });
    if let Err(error) = started {
        log::debug!("no reader for an agent's question: {error}");
    }
    hearing
}

/// Writes one line, by [`DEADLINE`] from now.
fn write_line(stream: &UnixStream, line: &str) -> io::Result<()> {
    stream.set_write_timeout(Some(DEADLINE))?;
    let mut stream = stream;
    stream.write_all(line.as_bytes())
}

/// Starts the thread that sees a kept connection's client hang up, and hangs
/// its feed up when it does.
///
/// A read of the connection, blocked for as long as the client says nothing —
/// which after its one request is for as long as it is there. Whatever it
/// sends is read and let go; its end of the connection closing, or the
/// connection being shut down when the verb is over, returns the read. A
/// read that returns the end of what the client says is not yet the client
/// gone — it may only have closed its half — and [`until_closed`] waits for
/// the rest.
fn watch_for_hangup(stream: &UnixStream, feed: &Arc<Feed>) {
    let Ok(watched) = stream.try_clone() else {
        return;
    };
    // The connection's deadline set a timeout on reads, and a read that timed
    // out would be taken for the client leaving.
    if watched.set_read_timeout(None).is_err() {
        return;
    }
    let feed = feed.clone();
    let started = thread::Builder::new()
        .name("crook-control-hangup".to_owned())
        .spawn(move || {
            let mut ignored = [0u8; 256];
            let mut watched = &watched;
            loop {
                match watched.read(&mut ignored) {
                    Ok(0) => {
                        until_closed(watched);
                        break;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            feed.hang_up();
        });
    if let Err(error) = started {
        log::debug!("no watch for a kept connection's client hanging up: {error}");
    }
}

/// Returns once the connection is closed both ways — the client gone, or the
/// connection shut down here when its verb is over — having been called when
/// the client has said all it will say.
///
/// A client that closes only its half after its request is still reading,
/// and its wait is still to be answered. `poll` tells the two apart on Linux:
/// asked for nothing, it reports only `POLLHUP`, which a Unix socket raises
/// once neither way is open, and an error; the peer's half-close is
/// `POLLRDHUP` and `POLLIN`, which are not asked for and wake nothing.
#[cfg(target_os = "linux")]
fn until_closed(stream: &UnixStream) {
    use std::ffi::c_ulong;
    use std::os::fd::AsRawFd;

    /// `struct pollfd`.
    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    // Declared rather than depended on, as `euid` declares `geteuid`:
    // `nfds_t` is an `unsigned long` in glibc and musl alike.
    unsafe extern "C" {
        fn poll(fds: *mut PollFd, count: c_ulong, timeout: i32) -> i32;
    }

    let mut watched = PollFd {
        fd: stream.as_raw_fd(),
        events: 0,
        revents: 0,
    };
    loop {
        // SAFETY: one `pollfd`, which lives across the call, and a count of
        // one. The descriptor is `stream`'s, open for as long as it is
        // borrowed.
        let polled = unsafe { poll(&mut watched, 1, -1) };
        if polled > 0 {
            return;
        }
        // No timeout was given, so nothing but a signal returns it empty.
        if polled < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return;
        }
    }
}

/// Everywhere but Linux, at once: the end of what the client says is taken
/// for the client leaving.
///
/// Nothing that blocks tells a half-close from a close there — macOS's `poll`
/// raises `POLLHUP` for either, from the same end-of-file its `kqueue` sees —
/// and a watch that never took the client for gone would hold a killed
/// client's wait for its whole hour. So a client of a kept verb keeps its
/// half open until the reply comes; see the module docs.
#[cfg(not(target_os = "linux"))]
fn until_closed(_: &UnixStream) {}

/// Writes one reply, by the connection's deadline.
fn send(stream: &UnixStream, deadline: Instant, reply: &Reply) -> io::Result<()> {
    stream.set_write_timeout(Some(left(deadline)?))?;
    let mut stream = stream;
    stream.write_all(reply.line().as_bytes())
}

/// What is left before `deadline`, or a timeout when nothing is.
fn left(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(left)
}

/// A connection read against one deadline for the whole of it.
///
/// A read timeout alone is per read, and a client that sent a byte just
/// before each one ran out would hold the connection for as long as it liked;
/// setting it to what is *left* before every read is what makes five seconds
/// five seconds.
struct Timed<'a> {
    stream: &'a UnixStream,
    deadline: Instant,
}

impl Read for Timed<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(left(self.deadline)?))?;
        let mut stream = self.stream;
        stream.read(buffer)
    }
}
