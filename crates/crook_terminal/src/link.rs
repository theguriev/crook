//! What a [`Terminal`](crate::Terminal) needs from the far end of its pty.
//!
//! A terminal is an emulator and a child it talks to, and the emulator does
//! not care where the child is. [`PtyLink`] is the whole of what a terminal
//! asks of the other side — bytes to read, somewhere to send keystrokes, a
//! size to pass on, and whether the child is still there — and it is exactly
//! what the terminal and the application driving it call today, no more.
//! [`crate::Pty`], a child on a pseudo-terminal this process opened, is one
//! implementation. A test that drives a whole session with no process in it
//! is another, and is what the seam is for until something else needs it.
//!
//! # Why a trait object
//!
//! Nothing a person does per keystroke goes through the link. A keystroke is
//! [`Terminal::write`](crate::Terminal::write), and that goes through the
//! writer the link hands out once, when the terminal is built; output goes
//! through the [`PtyReader`] the link hands out once, on a thread of its own.
//! Both of those were already boxed trait objects before the link existed —
//! they are what `portable-pty` returns — so neither path changed shape. What
//! is left on the link is a resize, the application's once-a-second question
//! about the child and a kill, where one indirect call is nothing beside the
//! syscall behind it.
//!
//! A type parameter would have cost more than it saved. Every session the
//! application keeps would carry it, and so would every function that touches
//! one, for a dispatch nobody could measure.

use std::fmt;
use std::io::Write;

use anyhow::Result;

use crate::pty::{ChildExit, PtyReader};
use crate::snapshot::TerminalSize;

/// The far end of a [`Terminal`](crate::Terminal): where its child's output
/// comes from, where its input goes, and what it can be asked about the
/// child.
///
/// Dropping a link is the link's own business. [`crate::Pty`] ends its child,
/// because a child left running on a pty nobody reads fills the buffer and
/// blocks for ever.
pub trait PtyLink: Send + fmt::Debug {
    /// The readable half, once.
    ///
    /// Reads block until the child says something, so the caller moves this
    /// to a thread of its own. Zero bytes, or an error, means the child is
    /// gone. Every later call returns `None`: the reads are one stream, and
    /// two owners would interleave escape sequences into nonsense.
    fn take_reader(&mut self) -> Option<PtyReader>;

    /// A writer for the child's input.
    ///
    /// [`Terminal`](crate::Terminal) asks for one when it is built and keeps
    /// it for its whole life, so this is not on any keystroke's path.
    fn writer(&self) -> Result<Box<dyn Write + Send>>;

    /// Tells the child its grid changed size.
    fn resize(&mut self, size: TerminalSize) -> Result<()>;

    /// The child's process id, where there is one to give.
    fn process_id(&self) -> Option<u32>;

    /// How the child finished, or `None` while it is still running.
    ///
    /// Must not block: the application asks under the same lock every other
    /// call on the terminal waits for.
    fn try_wait(&mut self) -> Result<Option<ChildExit>>;

    /// Blocks until the child finishes.
    fn wait(&mut self) -> Result<ChildExit>;

    /// Ends the child, without waiting to collect its exit status. Ending a
    /// child that has already finished is not an error.
    fn kill(&mut self) -> Result<()>;
}
