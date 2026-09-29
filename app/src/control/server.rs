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

use std::ffi::OsString;
use std::fs::{self, DirBuilder, Permissions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use super::protocol::{self, MAX_LINE, PaneEvent, Refusal, Reply, Request, Verb, Wait, code};
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
}

/// A connection's place in one of the [`Lanes`], given back when it is
/// dropped — however the connection's thread ends.
#[derive(Debug)]
pub struct Place {
    lanes: Arc<Lanes>,
    kept: bool,
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
            kept: false,
        })
    }

    /// Moves this place to the connections kept, giving the one it had back,
    /// or says there is no room there.
    fn keep(&mut self) -> bool {
        if self.kept {
            return true;
        }
        if self.lanes.kept.fetch_add(1, Ordering::AcqRel) >= MAX_KEPT {
            self.lanes.kept.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        self.lanes.answered.fetch_sub(1, Ordering::AcqRel);
        self.kept = true;
        true
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let lane = match self.kept {
            true => &self.lanes.kept,
            false => &self.lanes.answered,
        };
        lane.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Answers a connection's lines, one reply a line, until it closes, sends a
/// line too long to read, or runs out of time.
///
/// A blank line is skipped rather than refused, the way a shell skips one. A
/// last line with no newline before the end is still a request: the client
/// said all it had to say and closed its half.
pub fn converse(stream: &UnixStream, deadline: Instant, answer: &Ask, place: &mut Place) {
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
                keep(stream, id, request.clone(), deadline, answer, place);
                return;
            }
            // A verb that waits on git is given its own time, for this reply
            // only: the reading goes on against the connection's deadline, so
            // a line after a slow one finds that time has run out and the
            // connection closes, rather than one slow verb lending the rest
            // of the connection its two minutes.
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
    if !place.keep() {
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

/// Starts the thread that sees a kept connection's client hang up, and hangs
/// its feed up when it does.
///
/// A read of the connection, blocked for as long as the client says nothing —
/// which after its one request is for as long as it is there. Whatever it
/// sends is read and let go; its end of the connection closing, or the
/// connection being shut down when the verb is over, returns the read.
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
                    Ok(0) => break,
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
