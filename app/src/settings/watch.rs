//! Looking at `settings.json` for changes this window did not make.
//!
//! Every window of Crook is a process of its own with its own copy of the
//! settings, so a change made in one — a plugin revoked, a theme chosen — is a
//! change the others hear about only by reading the file again. Until they
//! did, a plugin revoked in one window went on running with the grant in every
//! other one until it was restarted. The command line's `--uninstall-plugin`
//! and a person editing the file in an editor are the same case.
//!
//! # A look every [`Watch::start`]'s interval, on a thread of its own
//!
//! Not a file-system notification. There is no watcher in the dependency
//! tree, and the three platforms' are three different contracts — inotify
//! watches an inode, which an editor that saves by renaming a new file into
//! place replaces; FSEvents reports a directory late and coalesced; a symlink
//! into a dotfiles repository is a fourth case for each. Reading one small
//! file and comparing its bytes with the last read is the same on all of
//! them, catches every way of replacing it, and does not depend on a clock:
//! two edits inside one tick of a file system's coarse mtime are still two
//! different contents.
//!
//! On a thread rather than the background pool, because a chain on the pool
//! parks a worker for the whole of its wait — see `crate::PARKED_WORKERS` —
//! and this one would be parked for the life of the window. The thread sleeps
//! between looks and wakes the window only when the file says something new,
//! which is the arrangement the control socket's threads use: a mailbox and a
//! waker, and a future the window's chain waits on.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use serde_json::{Map, Value};

/// How long a look waits for a file that was not there to come back.
///
/// An editor that saves by moving the old file aside and writing a new one
/// leaves a moment with no file at all. A look that fell in it would read
/// every setting as gone — every grant revoked, the theme back to the
/// default — and the next look would put them all back. A file still missing
/// this much later is one somebody deleted.
const MISSING_CONFIRM: Duration = Duration::from_millis(150);

/// What a look found.
#[derive(Debug)]
pub enum OnDisk {
    /// No file: somebody deleted it, which is every setting at its default.
    Missing,
    /// A file that does not read, and why. Not adopted: what the window has
    /// is the last thing the file said that did read, and a half-saved edit
    /// must not cost a person every option they had.
    Unreadable(String),
    /// The file, read whole.
    Document(Map<String, Value>),
}

/// One look, and what the saves were doing when it was taken.
#[derive(Debug)]
pub struct Seen {
    /// What `stamp` said just before the file was read. See
    /// [`Watch::start`].
    pub stamp: Option<u64>,
    /// What the file held.
    pub file: OnDisk,
}

/// The look at one file, for as long as this is kept.
///
/// Dropping it ends the looking: the future the window waits on resolves to
/// `None`, and the thread returns at its next tick.
pub struct Watch {
    mailbox: Arc<Mailbox>,
}

/// What the thread hands the window.
#[derive(Default)]
struct Mailbox {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The newest look nobody has taken. An older one is replaced, not
    /// queued: only what the file says now matters.
    seen: Option<Seen>,
    /// The window's chain, parked until there is a look to take.
    waker: Option<Waker>,
    /// The window has gone.
    closed: bool,
    /// The window wants the next look whether or not the file changed: it
    /// set the last one aside. See [`Watch::again`].
    again: bool,
}

impl Mailbox {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Watch {
    /// Looks at `path` every `every`, and says so whenever what it holds is
    /// not what it held at the look before.
    ///
    /// `stamp` is asked just before each read, and its answer rides with
    /// what was read. It is how a window tells a look that may have raced
    /// one of its own saves from one that cannot have: the window's answer
    /// is the number of saves it has asked for when none is still on its way
    /// to disk, and `None` while one is. A look whose stamp is not the
    /// window's count as it takes it was read across a save, and is set
    /// aside.
    ///
    /// The first look is always delivered, whatever it found: the window
    /// read the file before this started, and anything that changed in
    /// between would otherwise go unheard until the next change.
    ///
    /// `None` when no thread could be started, which leaves a window that
    /// hears nothing from other windows — the way every window was before
    /// this existed — rather than one that does not open.
    pub fn start(
        path: PathBuf,
        every: Duration,
        stamp: impl Fn() -> Option<u64> + Send + 'static,
    ) -> Option<Self> {
        let mailbox = Arc::new(Mailbox::default());
        let looking = mailbox.clone();
        let started = std::thread::Builder::new()
            .name(String::from("crook-settings-watch"))
            .spawn(move || look(&path, every, &stamp, &looking));
        match started {
            Ok(_) => Some(Self { mailbox }),
            Err(error) => {
                log::warn!("could not start watching the settings file: {error}");
                None
            }
        }
    }

    /// The next look, or `None` once this has been dropped.
    pub fn next(&self) -> impl Future<Output = Option<Seen>> + 'static {
        Next {
            mailbox: self.mailbox.clone(),
        }
    }

    /// Asks for the next look to be delivered even if the file has not
    /// changed since this one.
    ///
    /// For a look the window could not take — it was read across one of the
    /// window's own saves. That save changes the file, so its own look
    /// usually follows anyway; a save that failed does not, and without this
    /// a change made elsewhere in the meantime would wait for the next one.
    pub fn again(&self) {
        self.mailbox.lock().again = true;
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let waker = {
            let mut state = self.mailbox.lock();
            state.closed = true;
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The future a window's chain waits on.
struct Next {
    mailbox: Arc<Mailbox>,
}

impl Future for Next {
    type Output = Option<Seen>;

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.mailbox.lock();
        if state.closed {
            return Poll::Ready(None);
        }
        match state.seen.take() {
            Some(seen) => Poll::Ready(Some(seen)),
            None => {
                state.waker = Some(ctx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// The thread: a look every `every` until the window goes.
fn look(
    path: &std::path::Path,
    every: Duration,
    stamp: &dyn Fn() -> Option<u64>,
    mailbox: &Mailbox,
) {
    // `None` until the first look, which is therefore always delivered.
    let mut last: Option<Option<Vec<u8>>> = None;
    loop {
        std::thread::sleep(every);
        let again = {
            let mut state = mailbox.lock();
            if state.closed {
                return;
            }
            std::mem::take(&mut state.again)
        };

        let stamped = stamp();
        let bytes = match read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                // Not a change: a file that is there and cannot be read says
                // nothing about what it holds. Looked at again next tick.
                log::debug!("could not look at {}: {error}", path.display());
                continue;
            }
        };
        if !again && last.as_ref() == Some(&bytes) {
            continue;
        }
        last = Some(bytes.clone());

        let file = match bytes {
            None => OnDisk::Missing,
            Some(bytes) => match super::parse_document(&bytes) {
                Ok(document) => OnDisk::Document(document),
                Err(why) => OnDisk::Unreadable(why),
            },
        };
        let waker = {
            let mut state = mailbox.lock();
            state.seen = Some(Seen {
                stamp: stamped,
                file,
            });
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The file's bytes, or `None` for no file — once it has stayed gone for
/// [`MISSING_CONFIRM`].
fn read(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    let once = |path: &std::path::Path| match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    };
    match once(path)? {
        Some(bytes) => Ok(Some(bytes)),
        None => {
            std::thread::sleep(MISSING_CONFIRM);
            once(path)
        }
    }
}
