//! The child's input, queued for a thread of its own to write.
//!
//! A write to a pty blocks for as long as the child is not reading, and a
//! child that is busy printing is not reading: it is blocked on its own
//! output, which the terminal's reader drains only as fast as it can parse.
//! A terminal that wrote on the thread that also holds its emulator's lock —
//! the UI thread with a paste, the reader with an answer to a query — could
//! therefore wait on a child that was waiting on it, and a paste of a few
//! kilobytes into `cat` froze the window for good.
//!
//! So a terminal whose writer has been taken — see
//! [`Terminal::take_writer`](crate::Terminal::take_writer) — writes nothing
//! itself. Every write is an [`Outbox`] push, which copies the bytes and
//! returns, and the [`PtyWriter`] on the application's thread writes them to
//! the link in the order they were pushed. Keys, pastes, submitted lines and
//! the emulator's answers all go into the one queue, so the child sees them
//! in exactly the order the terminal produced them, as it did when each was
//! written on the spot.
//!
//! # What is kept for a child that never reads
//!
//! Everything a person sent, and a bounded tail of everything the emulator
//! answered — they are bounded by different things, and so they are kept
//! differently.
//!
//! **Input is never dropped.** A key, a paste or a line is what somebody
//! typed, and leaving out any part of it would type something different from
//! what they did: half a paste runs half a script. What a person can send is
//! bounded by what they hold — a paste is already whole on the clipboard and
//! on the UI thread — so the one bound is a backlog past which a new write is
//! refused rather than queued: [`INPUT_BACKLOG`].
//!
//! **Answers are dropped oldest first** once more than [`REPLY_BACKLOG`] of
//! them is waiting. They are produced by the child's own output, so a program
//! that floods queries and never reads its input — a renderer still asking
//! while its input loop is stuck — would otherwise grow the queue without
//! end. The oldest go because a program that stopped reading has no use for
//! answers to what it asked before it stopped: when it reads again, what it
//! needs are answers about the terminal as it is then. A batch is dropped
//! whole, one read's answers at a time, so a program never reads half a
//! sequence.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Write};
use std::sync::Arc;

use parking_lot::{Condvar, Mutex};

/// How much of the emulator's answers is kept for a child that is not reading
/// them, before the oldest are dropped.
///
/// A program that reads asks a handful of questions at a time and has the
/// answers in its input within a millisecond: a palette probe of all 256
/// colours is about eight kilobytes of them. Sixty-four is room for that many
/// times over, and nothing a program could use.
pub(crate) const REPLY_BACKLOG: usize = 64 * 1024;

/// How much of what a person sent may be waiting on a child before more is
/// refused.
///
/// Far beyond anything typed, and beyond any paste that is not a file's worth:
/// a backlog this long is a program that stopped reading a long time ago, and
/// a key pressed behind it would not reach it anyway. A write is refused only
/// once the backlog is already this long, never for its own size, so a single
/// paste of any size goes through whole.
pub(crate) const INPUT_BACKLOG: usize = 64 * 1024 * 1024;

/// What a queued run of bytes is, which decides whether it may be dropped.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Kind {
    /// What a person or the application sent: a key, a paste, a line.
    Input,
    /// What the emulator owed the child: one read's answers to its queries.
    Reply,
}

/// One run of bytes, in the order it was queued.
struct Chunk {
    kind: Kind,
    bytes: Vec<u8>,
}

/// The queue both halves share.
#[derive(Default)]
struct Queue {
    chunks: VecDeque<Chunk>,
    /// Bytes of input waiting, for [`INPUT_BACKLOG`].
    input: usize,
    /// Bytes of answers waiting, for [`REPLY_BACKLOG`].
    replies: usize,
    /// Whether answers have been dropped since the queue was last empty, so a
    /// flood is reported once rather than once per batch.
    dropping: bool,
    /// The terminal is gone, and nothing more will be queued.
    closed: bool,
    /// The writer is gone: the link stopped taking input, or the writer was
    /// dropped without ever running. Nothing queued would be written.
    gone: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    /// Signalled when the queue goes from empty to not, and when the terminal
    /// closes — the two things the writer waits for.
    ready: Condvar,
}

/// A new queue: the terminal's end, and the end a [`PtyWriter`] will be
/// made of once there is a writer to give it.
pub(crate) fn queue() -> (Outbox, Inbox) {
    let shared = Arc::new(Shared::default());
    (Outbox(shared.clone()), Inbox(shared))
}

/// The writer's end of a queue that has no writer yet.
pub(crate) struct Inbox(Arc<Shared>);

impl Inbox {
    /// The thread's half: this queue, written into `writer`.
    pub(crate) fn into_writer(self, writer: Box<dyn Write + Send>) -> PtyWriter {
        PtyWriter {
            shared: self.0,
            writer,
        }
    }
}

/// The terminal's end of the queue: every method copies and returns.
pub(crate) struct Outbox(Arc<Shared>);

impl Outbox {
    /// Queues what a person or the application sent.
    ///
    /// Joined onto the input already waiting behind it, when the last thing
    /// queued is input too: a child that is not reading would otherwise cost
    /// a write per key once it does.
    pub(crate) fn input(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut queue = self.0.queue.lock();
        if queue.gone {
            return Err(gone());
        }
        if queue.input >= INPUT_BACKLOG {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "the program has not read what it was already sent",
            ));
        }
        let woke = queue.chunks.is_empty();
        queue.input += bytes.len();
        match queue.chunks.back_mut() {
            Some(Chunk {
                kind: Kind::Input,
                bytes: waiting,
            }) => waiting.extend_from_slice(bytes),
            _ => queue.chunks.push_back(Chunk {
                kind: Kind::Input,
                bytes: bytes.to_vec(),
            }),
        }
        drop(queue);
        self.wake(woke);
        Ok(())
    }

    /// Queues one read's answers, dropping the oldest answers still waiting
    /// once there are more than [`REPLY_BACKLOG`] of them.
    ///
    /// Never these, however many there are: the bound is on what a child that
    /// is not reading leaves behind, and one batch is what a child that *is*
    /// reading is about to read.
    pub(crate) fn reply(&self, bytes: Vec<u8>) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut queue = self.0.queue.lock();
        if queue.gone {
            return Err(gone());
        }
        let woke = queue.chunks.is_empty();
        queue.replies += bytes.len();
        queue.chunks.push_back(Chunk {
            kind: Kind::Reply,
            bytes,
        });

        // The batch just queued is the last chunk, and stays the last as the
        // ones in front of it go.
        while queue.replies > REPLY_BACKLOG {
            let newest = queue.chunks.len() - 1;
            let Some(oldest) = queue
                .chunks
                .iter()
                .position(|chunk| chunk.kind == Kind::Reply)
                .filter(|oldest| *oldest != newest)
            else {
                break;
            };
            let Some(dropped) = queue.chunks.remove(oldest) else {
                break;
            };
            queue.replies -= dropped.bytes.len();
            if !std::mem::replace(&mut queue.dropping, true) {
                log::debug!(
                    "a program is not reading the answers to its queries; \
                     the oldest are being dropped"
                );
            }
        }
        drop(queue);
        self.wake(woke);
        Ok(())
    }

    /// Wakes the writer, if it may be waiting: it only waits on an empty
    /// queue, so a push onto one that was not empty needs nobody woken.
    fn wake(&self, woke: bool) {
        if woke {
            self.0.ready.notify_one();
        }
    }
}

impl Drop for Outbox {
    /// The terminal is gone, so its writer stops — with what it had not
    /// written yet dropped, since the child it was for is being ended too.
    fn drop(&mut self) {
        self.0.queue.lock().closed = true;
        self.0.ready.notify_all();
    }
}

/// The writable half of a terminal's link, with everything the terminal has
/// queued for it, meant to be moved to a thread of its own.
///
/// [`Self::run`] blocks for as long as the terminal lives: on the queue while
/// there is nothing to write, and in the link's writer while the child is not
/// reading what there is.
pub struct PtyWriter {
    shared: Arc<Shared>,
    writer: Box<dyn Write + Send>,
}

impl PtyWriter {
    /// Writes what the terminal queues, in the order it was queued, until the
    /// terminal is dropped.
    ///
    /// Returns `Ok` once the terminal has gone, and the link's error when the
    /// link stops taking input — the child closed the pty, or the link was
    /// closed while a write waited on it. Either way the terminal's later
    /// writes report that nothing will reach the child.
    pub fn run(mut self) -> io::Result<()> {
        while let Some(chunk) = self.next() {
            self.writer.write_all(&chunk.bytes)?;
            self.writer.flush()?;
        }
        Ok(())
    }

    /// The next run of bytes to write, or `None` once the terminal is gone.
    ///
    /// Taken off the queue before it is written, so the lock is never held
    /// across a write: the terminal side pushes while this waits on the
    /// child, which is the whole point.
    fn next(&self) -> Option<Chunk> {
        let mut queue = self.shared.queue.lock();
        loop {
            if queue.closed {
                return None;
            }
            if let Some(chunk) = queue.chunks.pop_front() {
                match chunk.kind {
                    Kind::Input => queue.input -= chunk.bytes.len(),
                    Kind::Reply => queue.replies -= chunk.bytes.len(),
                }
                if queue.chunks.is_empty() {
                    queue.dropping = false;
                }
                return Some(chunk);
            }
            self.shared.ready.wait(&mut queue);
        }
    }
}

impl Drop for PtyWriter {
    /// Nothing will be written from here on, whether [`Self::run`] ended or
    /// never began: the terminal is told, so it stops queueing bytes that
    /// would only be held.
    fn drop(&mut self) {
        let mut queue = self.shared.queue.lock();
        queue.gone = true;
        queue.chunks.clear();
        queue.input = 0;
        queue.replies = 0;
    }
}

impl fmt::Debug for PtyWriter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PtyWriter")
    }
}

/// What a write reports once nothing will reach the child.
fn gone() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "the pty is no longer taking input",
    )
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
