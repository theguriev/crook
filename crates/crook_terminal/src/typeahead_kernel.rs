//! What the pacing in [`super`] asks the kernel, and how it waits on it:
//! `FIONREAD` and `tcgetattr` on the master, and a `kqueue` on its write
//! side.

use std::ffi::{c_int, c_long, c_ulong, c_void};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::ptr;
use std::time::Instant;

use super::{Modes, NCCS};

/// `struct termios`, as `<sys/termios.h>` lays it out.
#[repr(C)]
struct Termios {
    iflag: c_ulong,
    oflag: c_ulong,
    cflag: c_ulong,
    lflag: c_ulong,
    cc: [u8; NCCS],
    ispeed: c_ulong,
    ospeed: c_ulong,
}

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

/// The modes of the child's terminal behind `master`.
pub(crate) fn modes(master: &impl AsRawFd) -> io::Result<Modes> {
    let mut termios = Termios {
        iflag: 0,
        oflag: 0,
        cflag: 0,
        lflag: 0,
        cc: [0; NCCS],
        ispeed: 0,
        ospeed: 0,
    };
    // SAFETY: `tcgetattr` writes one `struct termios`, and `termios` is one.
    if unsafe { tcgetattr(master.as_raw_fd(), &raw mut termios) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Modes::new(termios.iflag, termios.lflag, termios.cc))
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
