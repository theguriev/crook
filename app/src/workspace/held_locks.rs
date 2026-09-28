//! The locks a window took on the checkouts it made, until it takes them off.
//!
//! git records a lock as a sentence and nothing about who wrote it, and a
//! `crook: ` sentence says only that *a* Crook wrote it — this window, a
//! second window on the same repository, or one that crashed last week. So a
//! window remembers the locks it took itself, and those are the only ones it
//! takes off on its own: when no pane in the window is working in the checkout
//! any more, and, for whatever is still held when the window goes, on the way
//! out of the process. A checkout another Crook window made keeps its lock
//! however many of this window's panes pass through it.
//!
//! The one other place a `crook: ` lock comes off is the worktree menu, which
//! reads one on a checkout nothing in the window is working in as left behind
//! by a Crook that crashed; see `tab_menu::removable`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use crate::git::worktree;

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
    /// Whether the unlock has been asked for and has not answered yet.
    ///
    /// Kept in the list until it answers, so that a window closing in the
    /// meantime takes it off too rather than leaving it half done.
    releasing: bool,
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
            releasing: false,
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
/// Shared, because the workspace is not the last thing to need it: the locks
/// still held when the window closes are taken off as the event loop stops,
/// by [`Self::release_all`], from the delegate the platform tells — which
/// every way of closing a window reaches, and the workspace's own quit does
/// not.
#[derive(Debug, Clone, Default)]
pub struct HeldLocks(Rc<RefCell<Vec<HeldLock>>>);

impl HeldLocks {
    /// Remembers a lock the window has just taken.
    pub(crate) fn hold(&self, lock: HeldLock) {
        self.0.borrow_mut().push(lock);
    }

    /// Marks every held lock `vacated` says nothing is working in any more as
    /// being taken off, and hands those back to be.
    ///
    /// A lock already on its way off is not handed back twice.
    pub(crate) fn start_releasing(&self, vacated: impl Fn(&HeldLock) -> bool) -> Vec<HeldLock> {
        let mut held = self.0.borrow_mut();
        let mut going = Vec::new();
        for lock in held.iter_mut() {
            if !lock.releasing && vacated(lock) {
                lock.releasing = true;
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
        self.0
            .borrow_mut()
            .retain(|lock| !(lock.releasing && lock.checkout == checkout));
    }

    /// Whether the window holds no lock at all, which is almost always, and
    /// is what lets every check of them cost nothing then.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }

    /// The checkouts whose lock the window still holds, as git lists them.
    #[cfg(test)]
    pub(crate) fn checkouts(&self) -> Vec<PathBuf> {
        self.0
            .borrow()
            .iter()
            .map(|lock| lock.checkout.clone())
            .collect()
    }

    /// Takes off every lock still held, for a window that is closing.
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
        let held = std::mem::take(&mut *self.0.borrow_mut());
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
