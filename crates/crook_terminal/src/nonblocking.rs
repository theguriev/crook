//! The two halves of a Unix pty master, made never to leave a thread asleep
//! in a call the kernel will not wake.
//!
//! A blocking write to a master waits for the child to read, and on Linux it
//! can wait past the child: a write that was asleep when the child was killed
//! and reaped is still asleep twenty seconds later, because the wake-up the
//! slave's close sends can come before the count the write checks has
//! dropped. Nothing a terminal does can reach a thread in that state — closing
//! its own descriptors does not, since the call holds the file it is in. So
//! neither half ever makes that call. The master is non-blocking, and a half
//! that would block waits in `poll` instead, where the closing of the pty is
//! something it can see: `POLLHUP` once the child and everything it started
//! have let go of the slave, and, for the writer, a pipe of its own whose far
//! end the [`Pty`](crate::Pty) holds and drops — which covers a slave kept
//! open by a `sleep 60 &` that will read nothing either. The pipe is made the
//! first time a writer has to wait at all, which most never do, so a pane
//! holds the three descriptors it always held until its child stops reading.
//! A process out of descriptors cannot make it; the writer then waits on the
//! master alone, which a hang-up still ends, and tries again the next time it
//! has to wait — one wait a closed pane may not release, rather than a pane
//! whose input has stopped for good.
//!
//! Non-blocking is a property of the open file, which every descriptor
//! duplicated from the master shares, so the reader cannot stay blocking
//! while the writer is not. It waits in `poll` the same way, and to whoever
//! holds a [`PtyReader`](crate::PtyReader) a read still blocks until the child
//! says something.
//!
//! `poll` is `filedescriptor`'s, which `portable-pty` is built on: macOS's own
//! does not support devices, a pty included, and that crate's is `select`
//! there. `select` cannot take a descriptor past `FD_SETSIZE`, so a wait that
//! fails puts the master back to blocking and carries on — a write that waits
//! on a closed pane, as every write did before, rather than a pane that
//! cannot be read at all.
//!
//! On macOS a write waits for one thing more before it starts: for the child
//! to have read everything written before it, and it then adds no more than
//! the kernel's input queue holds. That kernel garbles input a child has not
//! read when the child turns canonical mode on, once there is enough of it to
//! wrap around the queue — which a shell running a pasted block line by line
//! does between every two lines. See the `typeahead` module, and its wait,
//! which is a `kqueue` with the pipe in it beside the master.

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::Instant;

use filedescriptor::{FileDescriptor, POLLIN, POLLOUT, Pipe, pollfd};
use parking_lot::Mutex;

#[cfg(target_os = "macos")]
use crate::typeahead::{self, Reads, Woke};

/// What a wait for the master came to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Waited {
    /// `poll` returned, with these events on the master — the readiness
    /// asked for, or `POLLHUP`, `POLLERR` and `POLLNVAL`, which it reports
    /// unasked and which mean nobody is on the other end any more.
    Polled(i16),
    /// The pty was closed on this side, which only the writer listens for.
    Closed,
    /// The wait itself failed, and the master is blocking again.
    Blocking,
}

/// The readable half: reads block, by waiting rather than by the call.
pub(crate) struct Readable(FileDescriptor);

impl Readable {
    /// The reading half of `master`, which has been made non-blocking.
    pub(crate) fn new(master: FileDescriptor) -> Self {
        Self(master)
    }
}

impl Read for Readable {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.0.read(buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    match wait(&mut self.0, POLLIN, None)? {
                        // Readable wins over a hang-up beside it: what the
                        // child printed before it went is still to be read,
                        // and the read after it reports the end.
                        Waited::Polled(events) if events & POLLIN != 0 => {}
                        Waited::Blocking => {}
                        // Hung up with nothing left to read, which is the end
                        // of the stream a blocking read would have reported.
                        Waited::Polled(_) | Waited::Closed => return Ok(0),
                    }
                }
                done => return done,
            }
        }
    }
}

/// What tells a writer waiting on its child that the pty has closed on this
/// side.
///
/// Held by the [`Pty`](crate::Pty), which calls [`Self::close`] when it is
/// dropped, and by every writer it handed out. A writer that has to wait makes
/// a pipe, keeps the reading end to wait on beside the master and leaves the
/// writing end here; closing drops the writing ends, and the reading ends read
/// the end of the stream, which `poll` reports.
#[derive(Default)]
pub(crate) struct Closing(Mutex<Ends>);

#[derive(Default)]
struct Ends {
    closed: bool,
    /// The writing end of each pipe a writer is waiting on, or once waited on.
    writing: Vec<FileDescriptor>,
    /// Tests: refuse to make a pipe, as a process out of descriptors does,
    /// and count the refusals.
    #[cfg(test)]
    refusing: Option<usize>,
}

impl Closing {
    /// A new pipe's reading end for a writer to wait on, or `None` once the
    /// pty has closed — a writer that only now has to wait must not start.
    ///
    /// Under the lock [`Self::close`] takes, so a pipe made as the pty closes
    /// is either refused or closed with the rest: a writer cannot slip into a
    /// wait that nothing will end.
    fn watch(&self) -> io::Result<Option<FileDescriptor>> {
        let mut ends = self.0.lock();
        if ends.closed {
            return Ok(None);
        }
        #[cfg(test)]
        if let Some(refused) = &mut ends.refusing {
            *refused += 1;
            return Err(io::Error::other("Too many open files"));
        }
        let Pipe { read, write } = Pipe::new().map_err(io::Error::other)?;
        ends.writing.push(write);
        Ok(Some(read))
    }

    /// Releases every writer waiting on the pty, and refuses any that would.
    pub(crate) fn close(&self) {
        let mut ends = self.0.lock();
        ends.closed = true;
        ends.writing.clear();
    }

    /// Tests: from now on [`Self::watch`] fails the way it does in a process
    /// with no descriptors left.
    #[cfg(test)]
    pub(crate) fn refuse_pipes(&self) {
        self.0.lock().refusing = Some(0);
    }

    /// Tests: how many times [`Self::watch`] has failed since
    /// [`Self::refuse_pipes`].
    #[cfg(test)]
    pub(crate) fn pipes_refused(&self) -> usize {
        self.0.lock().refusing.unwrap_or(0)
    }
}

/// The writable half: a write waits for the child to read, and gives up when
/// the pty closes.
///
/// On macOS a write also waits until the child has read everything written
/// before it, and adds at most a ring's worth: the kernel garbles input a
/// child has not read once there is more of it than that. See the `typeahead`
/// module.
pub(crate) struct Writable {
    master: FileDescriptor,
    closing: Arc<Closing>,
    /// The pipe this waits on beside the master, once it has had to wait.
    watching: Option<FileDescriptor>,
    /// Whether a failure to make that pipe has been logged already, so a
    /// process out of descriptors logs it once per writer, not once per wait.
    unwatched: bool,
    /// The wait for the child to read what it was given, once there has been
    /// something to wait for.
    #[cfg(target_os = "macos")]
    reads: Option<Reads>,
    /// Whether a write that could not be paced has been logged already.
    #[cfg(target_os = "macos")]
    unpaced: bool,
    /// The bytes written since the last line end, the most of a line the
    /// child may have left unfinished.
    #[cfg(target_os = "macos")]
    unended: usize,
}

impl Writable {
    /// The writing half of `master`, which has been made non-blocking, given
    /// up on once `closing` closes.
    pub(crate) fn new(master: FileDescriptor, closing: Arc<Closing>) -> Self {
        Self {
            master,
            closing,
            watching: None,
            unwatched: false,
            #[cfg(target_os = "macos")]
            reads: None,
            #[cfg(target_os = "macos")]
            unpaced: false,
            #[cfg(target_os = "macos")]
            unended: 0,
        }
    }

    /// Makes the pipe a wait watches beside the master, the first time there
    /// is a wait. An error is the pty having closed on this side already.
    fn watch_closing(&mut self) -> io::Result<()> {
        if self.watching.is_some() {
            return Ok(());
        }
        match self.closing.watch() {
            Ok(Some(watching)) => self.watching = Some(watching),
            Ok(None) => return Err(closed()),
            // Out of descriptors, most likely, which may not last: waited out
            // on the master alone, where a hang-up still ends it, and tried
            // again at the next wait. Ending the write here would end the
            // writer, and the pane would take no key again.
            Err(error) => {
                if !std::mem::replace(&mut self.unwatched, true) {
                    log::warn!(
                        "could not watch for a pty closing ({error}); \
                         a write waiting on it ends only when the program reads \
                         or hangs up"
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl Writable {
    /// The piece of `bytes` the child's input queue takes without wrapping,
    /// once the child has read everything written before them.
    ///
    /// All of them, as every write was before, when the kernel will not say
    /// how much is unread or cannot be waited on for a read: input that may
    /// arrive garbled is better than a pane whose input has stopped.
    fn paced<'a>(&mut self, bytes: &'a [u8]) -> io::Result<&'a [u8]> {
        // Set the first time a line longer than the queue holds the rest back,
        // and cleared whenever the child is found reading, which a program
        // stuck on such a line is not.
        let mut deadline = None;
        loop {
            let unread = match typeahead::unread(&self.master) {
                Ok(unread) => unread,
                Err(error) => return Ok(self.unpaced(bytes, &error)),
            };
            let patience = if unread == 0 {
                // Not knowing is taken for canonical, which can only make the
                // piece smaller.
                let canonical = typeahead::canonical(&self.master).unwrap_or(true);
                let room = typeahead::room(canonical, self.unended);
                if room > 0 {
                    return Ok(typeahead::piece(bytes, room));
                }
                Some(
                    *deadline.get_or_insert_with(|| Instant::now() + typeahead::LONG_LINE_PATIENCE),
                )
            } else {
                deadline = None;
                None
            };
            self.watch_closing()?;
            match self.next_read(patience) {
                Ok(Woke::Read) => {}
                Ok(Woke::HungUp) => return Err(hung_up()),
                Ok(Woke::Closed) => return Err(closed()),
                // A program that has stayed in canonical mode with a line it
                // cannot finish: see `LONG_LINE_PATIENCE`.
                Ok(Woke::Waited) => return Ok(bytes),
                Err(error) => return Ok(self.unpaced(bytes, &error)),
            }
        }
    }

    /// Waits for the child's next read, in a wait made the first time one is
    /// needed and watching the pipe from the first time there is one.
    fn next_read(&mut self, deadline: Option<Instant>) -> io::Result<Woke> {
        let reads = match self.reads.take() {
            Some(reads) => reads,
            None => Reads::new(self.master.as_raw_fd())?,
        };
        let reads = self.reads.insert(reads);
        if let Some(watching) = &self.watching {
            reads.watch_closing(watching.as_raw_fd())?;
        }
        reads.wait(deadline)
    }

    /// `bytes` whole, written as every write was before pacing, with the
    /// reason logged the first time.
    fn unpaced<'a>(&mut self, bytes: &'a [u8], error: &io::Error) -> &'a [u8] {
        if !std::mem::replace(&mut self.unpaced, true) {
            log::warn!(
                "could not wait for a program to read its input ({error}); \
                 a long paste may reach it garbled"
            );
        }
        bytes
    }
}

impl Write for Writable {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        #[cfg(target_os = "macos")]
        let bytes = self.paced(bytes)?;
        loop {
            match self.master.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.watch_closing()?;
                    match wait(&mut self.master, POLLOUT, self.watching.as_ref())? {
                        // A hang-up wins over writable beside it: Linux can
                        // report both and then refuse the write, and a loop
                        // that believed the first half would spin.
                        Waited::Polled(events) if events & !POLLOUT != 0 => {
                            return Err(hung_up());
                        }
                        Waited::Polled(_) | Waited::Blocking => {}
                        Waited::Closed => return Err(closed()),
                    }
                }
                #[cfg(target_os = "macos")]
                Ok(written) => {
                    let written = bytes.get(..written).unwrap_or(bytes);
                    self.unended = typeahead::unended(self.unended, written);
                    return Ok(written.len());
                }
                done => return done,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// What a write reports once the pty has closed on this side.
fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "the terminal was closed")
}

/// What a write reports once nobody is on the other end of the master.
fn hung_up() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "the program has closed its terminal",
    )
}

/// Waits until `master` is ready for `events` or hangs up, or until `closed`
/// — when there is one — becomes readable.
///
/// With no timeout: this is a thread waiting on a child, which may be hours,
/// and not a poll that ticks. An error is a master that can neither be
/// waited on nor made to block, which leaves nothing to do but stop.
fn wait(
    master: &mut FileDescriptor,
    events: i16,
    closed: Option<&FileDescriptor>,
) -> io::Result<Waited> {
    let watch = |fd: i32, events: i16| pollfd {
        fd,
        events,
        revents: 0,
    };
    let mut watched = [
        watch(master.as_raw_fd(), events),
        watch(closed.map_or(-1, AsRawFd::as_raw_fd), POLLIN),
    ];
    let count = if closed.is_some() { 2 } else { 1 };

    loop {
        match filedescriptor::poll(&mut watched[..count], None) {
            Ok(_) => break,
            Err(filedescriptor::Error::Poll(error) | filedescriptor::Error::Io(error))
                if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                log::warn!(
                    "could not wait on a pty ({error}); it will block until the program reads"
                );
                return match master.set_non_blocking(false) {
                    Ok(()) => Ok(Waited::Blocking),
                    Err(error) => Err(io::Error::other(error)),
                };
            }
        }
    }

    if count == 2 && watched[1].revents != 0 {
        return Ok(Waited::Closed);
    }
    Ok(Waited::Polled(watched[0].revents))
}

#[cfg(test)]
#[path = "nonblocking_tests.rs"]
mod tests;
