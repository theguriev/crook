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
//! So a writer here never lets input reach past the end of a ring. A ring
//! starts again at its first byte whenever it empties, so the writer asks how
//! much is unread, and from the last time that was nothing it counts what it
//! writes — see [`Pacing`]. While the count stays within [`ROOM`] a write
//! goes whole, as every write did before: keys typed ahead of a busy program
//! go at once, and so does a short paste. A write that does not fit waits for
//! the child's reads until nothing is unread, and then goes a piece at a
//! time, each one written into an empty queue and ending inside the ring.
//! Asking feeds pending input through again too, which loses nothing once
//! nothing waiting wraps.
//!
//! A piece is at most [`ROOM`], and ends at the last line end in it when it
//! holds one, because the count of unread input has a blind spot. In
//! canonical mode it leaves out a line the child has not finished, and a
//! shell that turns raw mode back on and reads part of such a line before the
//! next piece lands has that piece written in behind it — past the end of the
//! ring. Pieces that ended wherever [`ROOM`] fell left a line unfinished at
//! nearly every turn, and the same 730 lines still ran a line or two garbled
//! in one run in twenty-five. Pieces that end at a line leave nothing
//! unfinished but a line longer than [`ROOM`], so in canonical mode the count
//! starts from the most of a line that can be unfinished — everything written
//! since the last line end — and once that could fill the queue, nothing more
//! goes until the child reads. A shell reads the line as soon as it is back
//! in raw mode, and anything written before then would be past the kernel's
//! line limit and thrown away. A program that stays in canonical mode cannot
//! take such a line at all; see [`LONG_LINE_PATIENCE`].
//!
//! In raw mode the count is everything waiting, apart from fewer bytes than a
//! program with no read timeout has asked to be woken for (`VMIN`), which a
//! writer then adds behind; no shell asks for more than one.
//!
//! None of this holds back the few bytes the line discipline acts on and
//! never queues — the interrupt, quit and suspend characters, the start and
//! stop characters, and the status character in canonical mode; see
//! [`Modes::acts_on`]. They cannot wrap a ring, and they are what a person
//! presses to stop a program that has stopped reading, which is exactly when
//! input is held. One goes at once and on its own, and a piece stops short of
//! it. One pressed behind input the writer is already holding waits for that
//! input, as it waited behind the kernel's own full queue before.
//!
//! The wait is a `kqueue`, not the `select` the rest of the writer waits in:
//! `select` calls a master writable whenever the queue has room for a byte,
//! which it has from the child's first read to its last, and the event wanted
//! here is each read. The kernel reports every read to a master's write
//! filter, and a filter that is cleared once reported reports the next one.
//!
//! The pacing is arithmetic over the modes and the count, built and tested on
//! every platform; asking the kernel, and waiting on it, is macOS's alone.

use std::ffi::c_ulong;
use std::time::Duration;

#[cfg(target_os = "macos")]
#[path = "typeahead_kernel.rs"]
mod kernel;

#[cfg(target_os = "macos")]
pub(crate) use kernel::{Reads, Woke, modes, unread};

/// The most a writer lets into the queues from the last time they were
/// empty: XNU's `TTYHOG - 2`, the unread input past which a write from the
/// master waits for the child to read. Written into an empty queue it is
/// taken whole and ends inside the ring.
pub(crate) const ROOM: usize = 1022;

/// How long a writer holds the rest of a line longer than the queue, unread
/// in canonical mode, for the child to read it.
///
/// The one wait here with an end. A shell running the line before it is in
/// canonical mode only until it is back in its line editor, which then reads
/// the line in raw mode, and that read ends the wait. A program that stays in
/// canonical mode never reads a line it cannot finish, and nothing tells a
/// writer that a program has changed modes, so the wait has to end by itself:
/// the rest then goes as every terminal sends it, for the kernel to cut. The
/// line is given up on from then until a line end is written. The kernel has
/// cut it already, and holding back what is typed behind it — the Ctrl-U that
/// clears it included — would only make every key a second late.
pub(crate) const LONG_LINE_PATIENCE: Duration = Duration::from_secs(1);

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

/// A newline, or the carriage return the Enter key sends, which the line
/// discipline turns into one in canonical mode.
fn ends_a_line(byte: u8) -> bool {
    byte == b'\n' || byte == b'\r'
}

/// What a writer does with the bytes it was given, once it knows how much the
/// child has not read: see [`Pacing::next`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Next<'a> {
    /// Write these now: all of what was given, or the part of it that fits.
    Write(&'a [u8]),
    /// Wait for the child to read, for as long as that takes, and ask again.
    Wait,
    /// Wait for the child to read, for [`LONG_LINE_PATIENCE`] at most, and
    /// ask again; once that has passed, write these whole and call
    /// [`Pacing::give_up`].
    WaitAtMost(&'a [u8]),
}

/// One writer's count of what it has put into its child's queues, which is
/// how it knows how much more they take without wrapping.
///
/// [`Self::next`] says what to write, and [`Self::wrote`] is told what was.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct Pacing {
    /// How far into a ring unread input may reach: what has been written
    /// since the count of unread input was last nothing, and in canonical
    /// mode the line that was unfinished then, which that count leaves out.
    filled: usize,
    /// The bytes written since the last line end, the most of a line the
    /// child may have left unfinished.
    unended: usize,
    /// Whether that line was given up on: see [`LONG_LINE_PATIENCE`].
    given_up: bool,
    /// Whether the last byte written was the literal-next character, which
    /// makes the line discipline queue the byte after it as it is.
    quoting: bool,
    /// The modes the last [`Self::next`] was asked under, which are the ones
    /// what it said to write is written under.
    modes: Modes,
}

impl Pacing {
    /// What to do with `bytes`, when the child has `unread` bytes it has not
    /// read and its terminal is in `modes`.
    pub(crate) fn next<'a>(&mut self, bytes: &'a [u8], unread: usize, modes: &Modes) -> Next<'a> {
        self.modes = *modes;
        // No further than a write can reach. The writer asks again after
        // every read, which a program in canonical mode makes once a line,
        // and looking through all of a megabyte paste each time made pasting
        // it take minutes.
        let reach = &bytes[..bytes.len().min(ROOM + 1)];
        let wanted = match modes.first_acted_on(reach, self.quoting) {
            Some(0) => return Next::Write(&bytes[..1]),
            Some(at) => &bytes[..at],
            None => bytes,
        };
        if unread == 0 {
            self.filled = if modes.canonical() && !self.given_up {
                self.unended
            } else {
                0
            };
        }
        let room = ROOM.saturating_sub(self.filled);
        if wanted.len() <= room {
            Next::Write(wanted)
        } else if unread > 0 {
            // Not a piece of it. Pieces are written into an empty queue, so
            // that a paste goes the way it was measured to come through
            // whole, rather than in bits cut wherever the room ran out.
            Next::Wait
        } else if room > 0 {
            Next::Write(piece(wanted, room))
        } else {
            Next::WaitAtMost(wanted)
        }
    }

    /// Counts `written`, which [`Self::next`] said to write and the kernel
    /// took.
    pub(crate) fn wrote(&mut self, written: &[u8]) {
        self.filled = self.filled.saturating_add(written.len());
        if let [byte] = written
            && self.modes.first_acted_on(written, self.quoting) == Some(0)
        {
            // Never queued. The ones that send a signal throw away what the
            // child had not read, the unfinished line with it, unless the
            // terminal says not to; the count of unread input says so too
            // the next time it is asked, which is what clears `filled`.
            if self.modes.flushes(*byte) {
                self.unended = 0;
                self.given_up = false;
            }
            return;
        }
        for byte in written {
            let quoted = std::mem::take(&mut self.quoting);
            if !quoted && self.modes.quotes(*byte) {
                self.quoting = true;
            }
            if !quoted && ends_a_line(*byte) {
                self.unended = 0;
                self.given_up = false;
            } else {
                self.unended = self.unended.saturating_add(1);
            }
        }
    }

    /// The line unfinished in canonical mode was not read within
    /// [`LONG_LINE_PATIENCE`], and is not held back for again.
    pub(crate) fn give_up(&mut self) {
        self.given_up = true;
    }
}

// `struct termios`'s flags and control character indices, as XNU's
// `<sys/termios.h>` has them.
const IXON: c_ulong = 0x0000_0200;
const ISIG: c_ulong = 0x0000_0080;
const ICANON: c_ulong = 0x0000_0100;
const IEXTEN: c_ulong = 0x0000_0400;
const EXTPROC: c_ulong = 0x0000_0800;
const NOFLSH: c_ulong = 0x8000_0000;
const VINTR: usize = 8;
const VQUIT: usize = 9;
const VSUSP: usize = 10;
const VSTART: usize = 12;
const VSTOP: usize = 13;
const VLNEXT: usize = 14;
const VSTATUS: usize = 18;
/// How many control characters a `struct termios` holds.
pub(crate) const NCCS: usize = 20;
/// `_POSIX_VDISABLE`: a control character set to this is turned off.
const DISABLED: u8 = 0xff;

/// A child's terminal modes, as far as they decide what the line discipline
/// does with a byte from the master.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Modes {
    input: c_ulong,
    local: c_ulong,
    characters: [u8; NCCS],
}

impl Default for Modes {
    /// Modes nobody could read: taken for canonical, which can only make a
    /// piece smaller, with no byte acted on, so that nothing goes unpaced on
    /// a guess.
    fn default() -> Self {
        Self {
            input: 0,
            local: ICANON,
            characters: [DISABLED; NCCS],
        }
    }
}

impl Modes {
    /// The modes a `struct termios` gives as its input flags, its local flags
    /// and its control characters.
    pub(crate) fn new(input: c_ulong, local: c_ulong, characters: [u8; NCCS]) -> Self {
        Self {
            input,
            local,
            characters,
        }
    }

    /// Whether lines are read whole, in which case the count of unread input
    /// leaves out a line not finished yet.
    fn canonical(&self) -> bool {
        self.local & ICANON != 0
    }

    /// Whether `byte` is the control character at `index`, and that one is
    /// on.
    fn is(&self, index: usize, byte: u8) -> bool {
        byte != DISABLED && self.characters[index] == byte
    }

    /// Whether `byte` is the literal-next character, which the line
    /// discipline drops after marking the byte that follows it to be queued
    /// as it is.
    fn quotes(&self, byte: u8) -> bool {
        self.local & (IEXTEN | EXTPROC) == IEXTEN && self.is(VLNEXT, byte)
    }

    /// Whether the line discipline acts on `byte`, unquoted, and queues
    /// nothing: the checks `ttyinput` makes before it looks at the room left,
    /// which end in a signal to the program or in its output stopping or
    /// starting.
    pub(crate) fn acts_on(&self, byte: u8) -> bool {
        if self.local & EXTPROC != 0 || self.quotes(byte) {
            return false;
        }
        self.signals(byte)
            || (self.input & IXON != 0 && (self.is(VSTART, byte) || self.is(VSTOP, byte)))
            || (self.local & (ICANON | IEXTEN) == ICANON | IEXTEN && self.is(VSTATUS, byte))
    }

    /// Whether `byte` sends the program SIGINT, SIGQUIT or SIGTSTP.
    fn signals(&self, byte: u8) -> bool {
        self.local & ISIG != 0
            && (self.is(VINTR, byte) || self.is(VQUIT, byte) || self.is(VSUSP, byte))
    }

    /// Whether `byte`, acted on, also throws away the input not read yet.
    fn flushes(&self, byte: u8) -> bool {
        self.local & NOFLSH == 0 && self.signals(byte)
    }

    /// Where in `bytes` the first byte the line discipline acts on is, with
    /// `quoting` saying whether the byte written before them quotes the
    /// first.
    fn first_acted_on(&self, bytes: &[u8], mut quoting: bool) -> Option<usize> {
        bytes.iter().position(|byte| {
            if std::mem::take(&mut quoting) {
                return false;
            }
            if self.quotes(*byte) {
                quoting = true;
                return false;
            }
            self.acts_on(*byte)
        })
    }
}

#[cfg(test)]
#[path = "typeahead_tests.rs"]
mod tests;
