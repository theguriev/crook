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

/// How long the file has to stay gone, look after look, before a look says
/// so.
///
/// An editor that saves by moving the old file aside and writing a new one
/// leaves a moment with no file at all, and one on a slow disk or a network
/// home leaves a longer one. A look that fell in it and said "no file" would
/// put every option back to its default for a moment and then back again.
/// Even past this the window takes a missing file as "do not know" for the
/// plugins: see `Settings::adopt_missing`.
const MISSING_FOR: Duration = Duration::from_secs(3);

/// What a look found.
#[derive(Debug)]
pub enum OnDisk {
    /// No file, look after look for [`MISSING_FOR`]: somebody deleted it.
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
    /// What a Crook that could not write the file parked beside it. See
    /// [`super::Parked`].
    pub parked: super::Parked,
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

/// What one look read: the settings file's bytes and the parked file's,
/// each `None` for no file.
type Bytes = (Option<Vec<u8>>, Option<Vec<u8>>);

/// The thread: a look every `every` until the window goes.
fn look(
    path: &std::path::Path,
    every: Duration,
    stamp: &dyn Fn() -> Option<u64>,
    mailbox: &Mailbox,
) {
    // `None` until the first look, which is therefore always delivered.
    let mut last: Option<Bytes> = None;
    // Since when the looks have found no file.
    let mut gone_since: Option<std::time::Instant> = None;
    loop {
        std::thread::sleep(every);
        if mailbox.lock().closed {
            return;
        }

        let stamped = stamp();
        // The parked revokes beside the file are part of what it says: a
        // change to either is a change.
        let parked = read(&path.with_file_name(super::PARKED_FILE))
            .ok()
            .flatten();
        let bytes = match read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                // Not a change: a file that is there and cannot be read says
                // nothing about what it holds. Looked at again next tick.
                log::debug!("could not look at {}: {error}", path.display());
                continue;
            }
        };
        if bytes.is_none() {
            let since = *gone_since.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() < MISSING_FOR {
                continue;
            }
        } else {
            gone_since = None;
        }
        // Taken only by a look that would deliver, so that one waiting out a
        // missing file does not use it up.
        let again = std::mem::take(&mut mailbox.lock().again);
        let seen = (bytes, parked);
        if !again && last.as_ref() == Some(&seen) {
            continue;
        }
        last = Some(seen.clone());
        let (bytes, parked) = seen;
        let parked = parked
            .and_then(|bytes| super::parse_document(&bytes).ok())
            .map(|document| super::Parked::from_document(&document))
            .unwrap_or_default();

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
                parked,
            });
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The file's bytes, or `None` for no file.
fn read(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Runs `work` on a thread of its own and hands back a future of its answer,
/// for the window's chain to wait on.
///
/// For the settings' and the keybindings' writes, and for the pause before
/// one is tried again. Either can wait — for another Crook's lock, for the
/// pause itself — and a wait on the background pool holds a worker for all
/// of it: see `crate::PARKED_WORKERS`. A thread that cannot be started runs
/// `work` here and now, which is a frame late rather than a change lost.
pub fn on_a_thread<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = T> + 'static {
    let slot: Arc<Mutex<Answer<T>>> = Arc::new(Mutex::new(Answer {
        value: None,
        waker: None,
    }));
    // Shared with the thread, so that the work is still here to do if the
    // thread never starts.
    let job = Arc::new(Mutex::new(Some(work)));
    let (filling, taking) = (slot.clone(), job.clone());
    let started = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let work = taking.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(work) = work {
                fill(&filling, work());
            }
        });
    if let Err(error) = started {
        log::warn!("could not start {name}: {error}; doing it here instead");
        let work = job.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(work) = work {
            fill(&slot, work());
        }
    }
    Answered { slot }
}

/// A future that is ready once `after` has passed, waited out on a thread.
pub fn after(after: Duration) -> impl Future<Output = ()> + 'static {
    on_a_thread("crook-settings-retry", move || std::thread::sleep(after))
}

/// Where a thread's answer waits for the future that takes it.
struct Answer<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

/// Puts the answer where the future will find it, and wakes the future.
fn fill<T>(slot: &Mutex<Answer<T>>, value: T) {
    let waker = {
        let mut answer = slot.lock().unwrap_or_else(PoisonError::into_inner);
        answer.value = Some(value);
        answer.waker.take()
    };
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// The future [`on_a_thread`] hands back.
struct Answered<T> {
    slot: Arc<Mutex<Answer<T>>>,
}

impl<T> Future for Answered<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<T> {
        let mut answer = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        match answer.value.take() {
            Some(value) => Poll::Ready(value),
            None => {
                answer.waker = Some(ctx.waker().clone());
                Poll::Pending
            }
        }
    }
}
