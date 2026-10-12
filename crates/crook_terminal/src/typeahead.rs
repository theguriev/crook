//! What a pty on macOS does to input a child has not read yet, and how a
//! writer keeps clear of it.
//!
//! XNU's line discipline holds a child's unread input in two rings of 1024
//! bytes, the raw queue and the canonical one. A program that turns canonical
//! mode on while input is still unread in the raw queue — a shell leaving its
//! line editor to run the line it was given, with the lines after it already
//! typed ahead — has that input marked pending, and the kernel feeds it
//! through the line discipline again the next time anything touches the
//! queues: more input arriving, the program reading, or anybody asking how
//! much there is (`ttypend`, in `bsd/kern/tty.c`). It feeds the raw queue back
//! *into itself*. The copy it reads from shares the ring it writes to, and the
//! writing starts again at the ring's first byte, so input that wrapped past
//! the ring's end is overwritten before it is read: the program gets the start
//! of one line over and over where the rest of its input was, `L0L0L0L0…`
//! where `L00147\nL00148\n…` was written. Input that does not wrap comes
//! through whole, because the reading stays ahead of the writing. Linux keeps
//! its queue another way and is not affected.
//!
//! A writer that adds input as fast as the queue takes it keeps the raw queue
//! full while a shell reads it a byte at a time, so what is waiting wraps
//! almost always — and a shell running a pasted block switches to canonical
//! mode and back between every two of its lines. Worse follows when the
//! garbled input is a line with no end: in canonical mode a write from the
//! master is never made to wait while no line is finished, and everything
//! past the kernel's 1024-byte line limit is thrown away, so one write of the
//! rest of a paste can vanish whole. A block of 730 lines submitted to the
//! `bash` macOS ships ran garbled every time in 55 runs: about eighty lines
//! mangled and two hundred and seventy never run, and in one run in three the
//! shell was left at a half line with the rest of the block gone.
//!
//! So a writer here never adds to input the child has not read. It asks how
//! much is unread, waits for the child's next read until nothing is, and then
//! writes a piece that cannot wrap: one that starts at the ring's first byte,
//! since the queue is empty, and ends inside it. Asking feeds pending input
//! through again too, which loses nothing once nothing waiting wraps.
//!
//! A piece is at most [`ROOM`], and ends at the last line end in it when it
//! holds one, because the count has a blind spot. In canonical mode it leaves
//! out a line the child has not finished, and a shell that turns raw mode back
//! on and reads part of such a line before the next piece lands has that
//! piece written in behind it — past the end of the ring. Pieces that ended
//! wherever [`ROOM`] fell left a line unfinished at nearly every turn, and
//! the same 730 lines still ran a line or two garbled in one run in
//! twenty-five. Pieces that end at a line leave nothing unfinished but a line
//! longer than [`ROOM`], so in canonical mode a piece is also kept to what fits
//! behind the most of a line that can be unfinished — everything written since
//! the last line end — and once that could fill the queue, nothing more goes
//! until the child reads. A shell reads the line as soon as it is back in raw
//! mode, and anything written before then would be past the kernel's line
//! limit and thrown away. A program that stays in canonical mode cannot take
//! such a line at all; see [`LONG_LINE_PATIENCE`].
//!
//! In raw mode the count is everything waiting, apart from fewer bytes than a
//! program with no read timeout has asked to be woken for (`VMIN`), which a
//! writer then adds behind; no shell asks for more than one.
//!
//! The wait is a `kqueue`, not the `select` the rest of the writer waits in:
//! `select` calls a master writable whenever the queue has room for a byte,
//! which it has from the child's first read to its last, and the event wanted
//! here is each read. The kernel reports every read to a master's write
//! filter, and a filter that is cleared once reported reports the next one.

use std::ffi::{c_int, c_long, c_ulong, c_void};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::ptr;
use std::time::{Duration, Instant};

/// The most a writer adds at once: XNU's `TTYHOG - 2`, the unread input past
/// which a write from the master waits for the child to read. Written into an
/// empty queue it is taken whole and ends inside the ring.
pub(crate) const ROOM: usize = 1022;

/// What of `bytes` to write into an empty queue: at most `room`, cut after the
/// last line end in it when it holds one and is not all of `bytes`.
pub(crate) fn piece(bytes: &[u8], room: usize) -> &[u8] {
    let window = &bytes[..bytes.len().min(room)];
    if window.len() < bytes.len()
        && let Some(end) = window.iter().rposition(|byte| ends_a_line(*byte))
    {
        return &window[..=end];
    }
    window
}

/// The most a piece may hold: [`ROOM`], less in canonical mode the most of a
/// line the child may not have finished — the `unended` bytes written since
/// the last line end, which the count of unread input leaves out.
///
/// Nothing once that line may fill the queue: anything more would be past the
/// kernel's line limit, and thrown away. See [`LONG_LINE_PATIENCE`].
pub(crate) fn room(canonical: bool, unended: usize) -> usize {
    if canonical {
        ROOM.saturating_sub(unended)
    } else {
        ROOM
    }
}

/// How long a writer holds the rest of a line longer than the queue, unread
/// in canonical mode, for the child to read it.
///
/// The one wait here with an end. A shell running the line before it is in
/// canonical mode only until it is back in its line editor, which then reads
/// the line in raw mode, and that read ends the wait. A program that stays in
/// canonical mode never reads a line it cannot finish, and nothing tells a
/// writer that a program has changed modes, so the wait has to end by itself:
/// the rest then goes as every terminal sends it, for the kernel to cut,
/// rather than holding back everything typed behind it — the Ctrl-C included.
pub(crate) const LONG_LINE_PATIENCE: Duration = Duration::from_secs(1);

/// The bytes written since the last line end, once `written` has followed
/// `unended` of them.
pub(crate) fn unended(unended: usize, written: &[u8]) -> usize {
    match written.iter().rposition(|byte| ends_a_line(*byte)) {
        Some(end) => written.len() - end - 1,
        None => unended.saturating_add(written.len()),
    }
}

/// A newline, or the carriage return the Enter key sends, which the line
/// discipline turns into one in canonical mode.
fn ends_a_line(byte: u8) -> bool {
    byte == b'\n' || byte == b'\r'
}

/// `struct termios`, as `<sys/termios.h>` lays it out.
#[repr(C)]
struct Termios {
    iflag: c_ulong,
    oflag: c_ulong,
    cflag: c_ulong,
    lflag: c_ulong,
    cc: [u8; 20],
    ispeed: c_ulong,
    ospeed: c_ulong,
}

const ICANON: c_ulong = 0x0000_0100;

/// `struct timespec`.
#[repr(C)]
struct Timespec {
    seconds: c_long,
    nanoseconds: c_long,
}

/// `struct kevent`, as `<sys/event.h>` lays it out.
#[repr(C)]
struct Event {
    ident: usize,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: isize,
    udata: *mut c_void,
}

impl Event {
    fn on(fd: RawFd, filter: i16, flags: u16) -> Self {
        Self {
            ident: ident(fd),
            filter,
            flags,
            fflags: 0,
            data: 0,
            udata: ptr::null_mut(),
        }
    }
}

/// How `kevent` names a descriptor. One that is negative is no descriptor,
/// and names nothing a descriptor could be.
fn ident(fd: RawFd) -> usize {
    usize::try_from(fd).unwrap_or(usize::MAX)
}

const EVFILT_READ: i16 = -1;
const EVFILT_WRITE: i16 = -2;
const EV_ADD: u16 = 0x0001;
const EV_CLEAR: u16 = 0x0020;
const EV_ERROR: u16 = 0x4000;
const EV_EOF: u16 = 0x8000;
/// `_IOR('f', 127, int)`.
const FIONREAD: c_ulong = 0x4004_667f;

// In `libSystem`, which every macOS binary links: four functions do not
// justify a crate with a build script.
unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn tcgetattr(fd: c_int, termios: *mut Termios) -> c_int;
    fn kqueue() -> c_int;
    fn kevent(
        queue: c_int,
        changes: *const Event,
        change_count: c_int,
        events: *mut Event,
        event_count: c_int,
        timeout: *const Timespec,
    ) -> c_int;
}

/// How much input the child behind `master` has not read.
///
/// On a master `FIONREAD` counts what the slave can read, not what the master
/// can. Asking feeds pending input through the line discipline, as anything
/// touching the queues does; see the module docs for why that is harmless to
/// input written the way this module writes it.
pub(crate) fn unread(master: &impl AsRawFd) -> io::Result<usize> {
    let mut count: c_int = 0;
    // SAFETY: `FIONREAD` writes one `int`, and `count` is one.
    if unsafe { ioctl(master.as_raw_fd(), FIONREAD, &raw mut count) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(usize::try_from(count).unwrap_or(0))
}

/// Whether the child behind `master` has its terminal in canonical mode, in
/// which the count of unread input leaves out a line not finished yet.
pub(crate) fn canonical(master: &impl AsRawFd) -> io::Result<bool> {
    let mut termios = Termios {
        iflag: 0,
        oflag: 0,
        cflag: 0,
        lflag: 0,
        cc: [0; 20],
        ispeed: 0,
        ospeed: 0,
    };
    // SAFETY: `tcgetattr` writes one `struct termios`, and `termios` is one.
    if unsafe { tcgetattr(master.as_raw_fd(), &raw mut termios) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(termios.lflag & ICANON != 0)
}

/// What a wait for the child to read came to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Woke {
    /// The child read, or may have: the count is to be asked again.
    Read,
    /// Nobody is on the other end of the master any more.
    HungUp,
    /// The pipe watched beside the master was closed: the pty closed on this
    /// side.
    Closed,
    /// The deadline passed first.
    Waited,
}

/// A wait for the child behind one master to read.
pub(crate) struct Reads {
    queue: OwnedFd,
    /// The pipe that says the pty closed on this side, once it is watched.
    closing: Option<RawFd>,
}

impl Reads {
    /// A wait on `master`, which this watches and does not own.
    pub(crate) fn new(master: RawFd) -> io::Result<Self> {
        // SAFETY: takes nothing, and returns a new descriptor or -1.
        let queue = unsafe { kqueue() };
        if queue < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `kqueue` just made it, and nothing else holds it.
        let queue = unsafe { OwnedFd::from_raw_fd(queue) };
        let reads = Self {
            queue,
            closing: None,
        };
        // Cleared once reported, so that a wait sleeps until the next read
        // rather than returning while the queue has room, which is always.
        reads.add(&Event::on(master, EVFILT_WRITE, EV_ADD | EV_CLEAR))?;
        Ok(reads)
    }

    /// Ends the wait, from now on, when `pipe` becomes readable: when its far
    /// end, which the pty holds, is closed.
    pub(crate) fn watch_closing(&mut self, pipe: RawFd) -> io::Result<()> {
        if self.closing == Some(pipe) {
            return Ok(());
        }
        self.add(&Event::on(pipe, EVFILT_READ, EV_ADD))?;
        self.closing = Some(pipe);
        Ok(())
    }

    fn add(&self, change: &Event) -> io::Result<()> {
        // SAFETY: one change, read from `change`, and no events to write.
        let added = unsafe {
            kevent(
                self.queue.as_raw_fd(),
                change,
                1,
                ptr::null_mut(),
                0,
                ptr::null(),
            )
        };
        if added < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Waits for the child's next read, or for the pty to go — or, given a
    /// deadline, for that to pass.
    ///
    /// Otherwise with no end, for the same reason the rest of the writer has
    /// none: a child that is not reading may not read for hours.
    pub(crate) fn wait(&self, deadline: Option<Instant>) -> io::Result<Woke> {
        let mut events = [Event::on(0, 0, 0), Event::on(0, 0, 0)];
        let count = loop {
            let timeout = deadline.map(|deadline| {
                let left = deadline.saturating_duration_since(Instant::now());
                Timespec {
                    seconds: c_long::try_from(left.as_secs()).unwrap_or(c_long::MAX),
                    nanoseconds: c_long::from(left.subsec_nanos()),
                }
            });
            // SAFETY: at most two events, written into `events`, which holds
            // two; no changes; the timeout, when there is one, read from
            // `timeout`, which outlives the call.
            let count = unsafe {
                kevent(
                    self.queue.as_raw_fd(),
                    ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    2,
                    timeout.as_ref().map_or(ptr::null(), ptr::from_ref),
                )
            };
            match usize::try_from(count) {
                Ok(0) => return Ok(Woke::Waited),
                Ok(count) => break count,
                Err(_) => {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
            }
        };

        let mut woke = Woke::Read;
        for event in events.iter().take(count) {
            if event.flags & EV_ERROR != 0 {
                let code = i32::try_from(event.data).unwrap_or(0);
                return Err(io::Error::from_raw_os_error(code));
            }
            // The pty closing wins over anything beside it, as it does in the
            // `poll` the rest of the writer waits in.
            if self
                .closing
                .is_some_and(|closing| event.ident == ident(closing))
            {
                return Ok(Woke::Closed);
            }
            if event.flags & EV_EOF != 0 {
                woke = Woke::HungUp;
            }
        }
        Ok(woke)
    }
}

#[cfg(test)]
#[path = "typeahead_tests.rs"]
mod tests;
