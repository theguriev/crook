//! The locks a window took on the checkouts it made, until it takes them off.
//!
//! git records a lock as a sentence and nothing about who wrote it, and a
//! `crook: ` sentence says only that *a* Crook wrote it — this window, a
//! second window on the same repository, or one that crashed last week. So a
//! window remembers the locks it took itself, and those are the only ones it
//! takes off on its own: when no pane in the window is working in the checkout
//! any more, and, for whatever is still held when the window goes, on the way
//! out of the process. A checkout git is still making when the window goes
//! takes its own lock straight back off once it is made, since by then
//! nothing else is left to. A checkout another Crook window made keeps its
//! lock however many of this window's panes pass through it.
//!
//! The one other place a `crook: ` lock comes off is the worktree menu, which
//! reads one on a checkout nothing in the window is working in as left behind
//! by a Crook that crashed; see `tab_menu::removable`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::git::worktree;

/// Where a held lock is, between being taken and being taken off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Taken by a creation whose tab has not opened yet.
    ///
    /// No pane is in the checkout, and none is meant to be until the tab
    /// opens, so a lock at this stage is not one for
    /// [`HeldLocks::start_releasing`] to find vacated.
    Opening,
    /// Its tab has opened, and it stays on until no pane in the window is
    /// working in the checkout.
    Held,
    /// The unlock has been asked for and has not answered yet.
    ///
    /// Kept in the list until it answers, so that a window closing in the
    /// meantime takes it off too rather than leaving it half done.
    Releasing,
}

/// A lock this window took on a checkout it made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldLock {
    /// Where the unlock runs from: the repository's main checkout, which is
    /// never the one losing its lock and, a moment later, perhaps its
    /// directory.
    pub(crate) repository: PathBuf,
    /// The checkout as git lists it, which is the spelling
    /// [`worktree::release`] matches, and the one a shell that resolved its
    /// directory reports.
    pub(crate) checkout: PathBuf,
    /// The checkout as Crook spelled it when it made it, which is where its
    /// tab opened. The same as `checkout` unless the way to the store runs
    /// through a link, or through a short name on Windows.
    pub(crate) opened_at: PathBuf,
    stage: Stage,
}

impl HeldLock {
    /// Locks the checkout Crook has just made at `path` for `branch`, and
    /// notes where to find it again to take the lock off.
    ///
    /// **Blocking**: the lock, and one read of the listing for git's own
    /// spelling of the checkout and of the main one, which only the listing
    /// has.
    ///
    /// `None` when git would not lock it. The checkout is made and is what
    /// was asked for, so that costs the lock and not the checkout, and is
    /// logged rather than shown.
    pub(crate) fn take(repository: &Path, path: &Path, branch: &str) -> Option<Self> {
        let reason = worktree::lock_reason(branch);
        if let Err(problem) = worktree::lock(repository, path, &reason) {
            log::warn!("{} was made but not locked: {problem}", path.display());
            return None;
        }

        // A listing that fails leaves Crook's own spelling standing in for
        // both. The unlock then runs from where the menu was opened, and
        // matches the path git was given, which is right everywhere the store
        // is reached without a link.
        let listed = worktree::list(repository).unwrap_or_default();
        let main = listed
            .iter()
            .find(|checkout| checkout.is_main)
            .map(|checkout| checkout.path.clone());
        let checkout = listed
            .iter()
            .find(|checkout| !checkout.is_main && checkout.branch.as_deref() == Some(branch))
            .map(|checkout| checkout.path.clone());

        Some(Self {
            repository: main.unwrap_or_else(|| repository.to_owned()),
            checkout: checkout.unwrap_or_else(|| path.to_owned()),
            opened_at: path.to_owned(),
            stage: Stage::Opening,
        })
    }

    /// The two spellings of the checkout, for asking whether a pane is in
    /// it.
    pub(crate) fn spellings(&self) -> [&Path; 2] {
        [&self.checkout, &self.opened_at]
    }
}

/// Every lock a window holds.
///
/// Shared, and across threads, because the workspace is not the only thing
/// that needs it. The creation that takes a lock hands it in from the
/// background pool, in `admit`, and the locks still held when the window
/// closes are taken off as the event loop stops, by [`Self::release_all`],
/// from the delegate the platform tells. Every way of closing a window
/// reaches that delegate. Only some of them pass through the workspace's own
/// quit first — the last tab closing, the header's × — and the window
/// manager's close and macOS's Quit do not.
#[derive(Debug, Clone, Default)]
pub struct HeldLocks(Arc<Mutex<Held>>);

/// What [`HeldLocks`] guards.
#[derive(Debug, Default)]
struct Held {
    locks: Vec<HeldLock>,
    /// Whether [`HeldLocks::release_all`] has run. The window has gone then,
    /// and a lock handed in after it is one nothing would ever take off.
    closed: bool,
}

impl HeldLocks {
    /// The list, for one change or one question.
    fn held(&self) -> MutexGuard<'_, Held> {
        // Every change to the list is one push, one retain or one field set in
        // place, so a panic in the middle of one leaves a list that is still
        // true, and the locks in it still need taking off.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Remembers a lock a creation has just taken, until its tab opens — or,
    /// when the window closed while git was making the checkout, takes it
    /// straight back off.
    ///
    /// **Blocking** in the second case, for the unlock. Called on the
    /// background pool by the creation that took the lock, which the process
    /// waits for on its way out. Once the window has gone, that creation is
    /// the one thing still running, so it is the one thing that can take its
    /// lock off.
    ///
    /// Decided under the lock [`Self::release_all`] closes the list under, so
    /// a lock is either in the list when that runs or finds it closed.
    pub(crate) fn admit(&self, lock: HeldLock) {
        {
            let mut held = self.held();
            if !held.closed {
                held.locks.push(lock);
                return;
            }
        }
        if let Err(problem) = worktree::release(&lock.repository, &lock.checkout) {
            log::warn!("could not unlock {}: {problem}", lock.checkout.display());
        }
    }

    /// Holds the lock on the checkout whose tab has just opened at
    /// `opened_at`: from here on it comes off when no pane in the window is
    /// working in the checkout.
    ///
    /// Nothing when there is no such lock, because git would not lock the
    /// checkout.
    pub(crate) fn opened(&self, opened_at: &Path) {
        for lock in &mut self.held().locks {
            if lock.stage == Stage::Opening && lock.opened_at == opened_at {
                lock.stage = Stage::Held;
            }
        }
    }

    /// Marks every held lock `vacated` says nothing is working in any more as
    /// being taken off, and hands those back to be.
    ///
    /// A lock already on its way off is not handed back twice, and a lock
    /// whose tab has not opened yet is not handed back at all.
    pub(crate) fn start_releasing(&self, vacated: impl Fn(&HeldLock) -> bool) -> Vec<HeldLock> {
        let mut held = self.held();
        let mut going = Vec::new();
        for lock in &mut held.locks {
            if lock.stage == Stage::Held && vacated(lock) {
                lock.stage = Stage::Releasing;
                going.push(lock.clone());
            }
        }
        going
    }

    /// Forgets the lock on `checkout` once taking it off has answered,
    /// whichever way it answered.
    ///
    /// A lock git would not take off is not asked about again: the menu
    /// recognises it as Crook's own, and a retry at every keystroke would be
    /// a subprocess a keystroke.
    pub(crate) fn released(&self, checkout: &Path) {
        self.held()
            .locks
            .retain(|lock| !(lock.stage == Stage::Releasing && lock.checkout == checkout));
    }

    /// Whether the window holds no lock at all, which is almost always, and
    /// is what lets every check of them cost nothing then.
    pub(crate) fn is_empty(&self) -> bool {
        self.held().locks.is_empty()
    }

    /// Whether the window holds the lock on the checkout git lists at
    /// `checkout`.
    pub(crate) fn holds(&self, checkout: &Path) -> bool {
        self.held()
            .locks
            .iter()
            .any(|lock| lock.spellings().contains(&checkout))
    }

    /// The checkouts whose lock the window still holds, as git lists them.
    #[cfg(test)]
    pub(crate) fn checkouts(&self) -> Vec<PathBuf> {
        self.held()
            .locks
            .iter()
            .map(|lock| lock.checkout.clone())
            .collect()
    }

    /// Takes off every lock still held, for a window that is closing, and
    /// closes the list: a creation that takes its lock after this takes it
    /// straight back off, in `admit`.
    ///
    /// **Blocking**, for at most `patience`. Every pane has gone with the
    /// window, so nothing is working in any of these checkouts, and a lock
    /// left on one would hold it against `git worktree remove`, `prune` and
    /// every other tool's tidy-up for good: a session brought back next time
    /// does not lock again, and with "Restore session" off nothing comes back
    /// to it at all.
    ///
    /// The unlocks run on a thread of their own, so a git that hangs costs the
    /// wait and not the exit. A lock git has not taken off by then stays, and
    /// is what the menu reads as one a crashed Crook left behind.
    pub fn release_all(&self, patience: Duration) {
        let held = {
            let mut held = self.held();
            held.closed = true;
            std::mem::take(&mut held.locks)
        };
        if held.is_empty() {
            return;
        }

        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for lock in &held {
                if let Err(problem) = worktree::release(&lock.repository, &lock.checkout) {
                    log::warn!("could not unlock {}: {problem}", lock.checkout.display());
                }
            }
            // Ignored: the only way this fails is the wait below having given
            // up and dropped the receiver.
            let _ = done.send(());
        });

        if finished.recv_timeout(patience).is_err() {
            log::warn!(
                "left Crook's locks to git after waiting {}s for it to take them off",
                patience.as_secs()
            );
        }
    }
}
