//! The git facts behind the tab strip, gathered off the UI thread.
//!
//! [`crate::git`] is blocking by construction: reading `HEAD` is a handful of
//! syscalls and counting changed lines is a `git` subprocess. Neither belongs
//! in `render`, which runs whenever anything on screen moves. Warp learned this
//! the expensive way — its vertical-tab rows derive per-row state inside the
//! renderer and pay for it at frame rate — so nothing here is reachable from a
//! view except [`GitModel::facts`], which is a map lookup.
//!
//! The shape is [`crate::usage_model`]'s, for the same reason: one cycle runs
//! on the background pool, delivers on the foreground one, and starts the next.
//!
//! # Why a second poll chain cannot start here
//!
//! The same ownership trick. There is exactly one [`Ticket`], it is *moved*
//! into the cycle that is running, and only a finishing cycle can hand it to
//! the next. Nothing else in this file can start a cycle, because nothing else
//! has a ticket to give it. A directory appearing does not start work: it is
//! written into the shared list and the sleeping cycle is poked, and the cycle
//! it wakes is the one that serves it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crookui_core::prelude::*;

use crate::git::{self, GitFacts};

/// How long a cycle sleeps before gathering again.
///
/// Warp watches the filesystem and throttles the result to five seconds. Crook
/// has no watcher and should not grow one for this, so it polls — and the
/// polling interval is what a branch switch costs to notice. Fifteen seconds is
/// slow enough that the expensive half runs rarely and quick enough that a
/// checkout shows up before it is forgotten about.
const REFRESH_INTERVAL: Duration = Duration::from_secs(15);

/// The right to run a gather cycle. There is exactly one.
///
/// See the module docs: this is what makes a second poll chain unrepresentable
/// rather than merely unlikely.
struct Ticket;

/// Everything the tab strip knows about the repositories its sessions sit in.
pub struct GitModel {
    /// What the last gather found, by directory. A directory absent from here
    /// has not been read yet; a directory present with an empty [`GitFacts`]
    /// has been read and is not in a repository.
    facts: HashMap<PathBuf, GitFacts>,

    /// The directories the strip is currently showing.
    ///
    /// Shared rather than passed, because the cycle that will gather them is
    /// usually already asleep when a session's directory changes — it has to
    /// read the list at the moment it wakes, on a thread that cannot touch
    /// `self`.
    tracked: Arc<Mutex<Vec<PathBuf>>>,

    /// Whether anything on screen is showing diff stats.
    ///
    /// The cheap half — walking up for `.git` and reading `HEAD` — always runs.
    /// The subprocess only runs when a chip would print its answer, which is
    /// Warp's `needs_git_status_for_chip_ui` rule: do not pay for git when
    /// nothing displays git.
    wants_diff: Arc<AtomicBool>,

    /// Cuts the sleeping cycle's wait short.
    ///
    /// Replaced by every [`Self::spawn_cycle`], which is why it cannot be the
    /// only record that somebody asked — see [`Self::poked`].
    wake: Option<Sender<()>>,

    /// Whether anybody has asked for a gather that has not happened yet.
    ///
    /// The channel alone loses the request that matters most. A wake sent
    /// while a cycle is *running* goes into a receiver that has already been
    /// read for the last time, and [`Self::finish`] then throws that channel
    /// away and schedules the next cycle a full [`REFRESH_INTERVAL`] out — so
    /// "show it now" silently becomes "show it in fifteen seconds", in exactly
    /// the window a person is most likely to be in: the cycle a new tab or a
    /// switched-on toggle just started. This flag outlives the channel, and
    /// `finish` reads it. [`crate::usage_model`] guards its click the same way
    /// and for the same reason.
    poked: Arc<AtomicBool>,

    /// The ticket, while no cycle owns it. `Some` only before [`Self::start`].
    idle_ticket: Option<Ticket>,
}

impl Entity for GitModel {
    type Event = ();
}

impl GitModel {
    /// A model that has read nothing and is not polling yet.
    ///
    /// Polling starts at [`Self::start`] rather than here, so a headless run
    /// and a test never spawn `git`.
    pub fn new(_: &mut ModelContext<Self>) -> Self {
        Self {
            facts: HashMap::new(),
            tracked: Arc::new(Mutex::new(Vec::new())),
            wants_diff: Arc::new(AtomicBool::new(false)),
            wake: None,
            poked: Arc::new(AtomicBool::new(false)),
            idle_ticket: Some(Ticket),
        }
    }

    /// What is known about the repository at `dir`, if it has been read.
    ///
    /// The one method a renderer may call. It is a map lookup and nothing else:
    /// no walk, no file read, no subprocess.
    pub fn facts(&self, dir: &Path) -> Option<&GitFacts> {
        self.facts.get(dir)
    }

    /// Starts the gather chain. Does nothing on a second call.
    pub fn start(&mut self, ctx: &mut ModelContext<Self>) {
        let Some(ticket) = self.idle_ticket.take() else {
            return;
        };
        self.spawn_cycle(ticket, Duration::ZERO, ctx);
    }

    /// Tells the model which directories the strip is showing.
    ///
    /// Cheap and idempotent, so the workspace can call it after every change to
    /// the strip. A directory nobody shows any more is forgotten, which is what
    /// keeps the map from growing for the life of the process; a new one wakes
    /// the sleeping cycle so its branch appears now rather than up to
    /// [`REFRESH_INTERVAL`] from now.
    pub fn track(&mut self, mut dirs: Vec<PathBuf>, ctx: &mut ModelContext<Self>) {
        dirs.sort();
        dirs.dedup();

        let mut tracked = self.tracked.lock().unwrap_or_else(PoisonError::into_inner);
        if *tracked == dirs {
            return;
        }
        let is_new = dirs.iter().any(|dir| !tracked.contains(dir));
        *tracked = dirs;
        drop(tracked);

        self.forget_untracked(ctx);
        if is_new {
            self.wake_cycle();
        }
    }

    /// Says whether the expensive half is worth running.
    ///
    /// Turning it on wakes the sleeping cycle, so switching "Diff stats" on
    /// fills the chips in without waiting out the timer.
    pub fn set_diff_stats_wanted(&mut self, wanted: bool) {
        if self.wants_diff.swap(wanted, Ordering::Relaxed) == wanted {
            return;
        }
        if wanted {
            self.wake_cycle();
        }
    }

    /// Records what is known about one directory.
    ///
    /// The delivery point for a finished gather, and the way a test puts known
    /// facts in front of the renderer without a repository on disk — which is
    /// the only way to assert what a row draws for a branch or a diff without
    /// making the assertion depend on the machine running it.
    pub fn record(&mut self, dir: PathBuf, facts: GitFacts, ctx: &mut ModelContext<Self>) {
        if self.facts.get(&dir) == Some(&facts) {
            return;
        }
        self.facts.insert(dir, facts);
        ctx.notify();
    }

    /// Runs one cycle: wait, gather, hand the ticket back on the main thread.
    fn spawn_cycle(&mut self, ticket: Ticket, delay: Duration, ctx: &mut ModelContext<Self>) {
        let (wake, wakeup) = mpsc::channel();
        self.wake = Some(wake);

        let tracked = self.tracked.clone();
        let wants_diff = self.wants_diff.clone();
        let poked = self.poked.clone();
        let background = ctx.background().clone();

        // The whole cycle — the wait and the blocking reads — is one background
        // task, so the pool holds one worker for one chain and there is nothing
        // to keep in step between a timer and a subprocess.
        ctx.spawn(
            async move {
                background
                    .spawn(async move {
                        if !delay.is_zero() {
                            // Returns early when `track` pokes the channel, and
                            // on the timeout otherwise.
                            let _ = wakeup.recv_timeout(delay);
                        }

                        // Cleared here rather than by the sender, so that a
                        // poke arriving from this line onwards is still
                        // outstanding when `finish` asks: the gather below has
                        // already read the list it is going to read.
                        poked.store(false, Ordering::Relaxed);

                        let dirs = tracked
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone();
                        let with_diff = wants_diff.load(Ordering::Relaxed);
                        let gathered: Vec<_> = dirs
                            .into_iter()
                            .map(|dir| {
                                let facts = gather(&dir, with_diff);
                                (dir, facts)
                            })
                            .collect();

                        (ticket, gathered)
                    })
                    .await
            },
            |model, (ticket, gathered), ctx| model.finish(ticket, gathered, ctx),
        )
        .detach();
    }

    /// Records a cycle's result and starts the next one.
    fn finish(
        &mut self,
        ticket: Ticket,
        gathered: Vec<(PathBuf, GitFacts)>,
        ctx: &mut ModelContext<Self>,
    ) {
        let mut changed = false;
        for (dir, facts) in gathered {
            // A directory that stopped being shown while the gather ran does
            // not come back into the map because a worker was mid-subprocess.
            if !self.is_tracked(&dir) {
                continue;
            }
            changed |= self.facts.get(&dir) != Some(&facts);
            self.facts.insert(dir, facts);
        }

        // A cycle that read the same branch and the same numbers as the last
        // one repaints nothing, which is what keeps a fifteen-second timer from
        // being a fifteen-second frame.
        if changed {
            ctx.notify();
        }

        self.spawn_cycle(ticket, self.next_delay(), ctx);
    }

    /// How long the next cycle should wait before gathering.
    ///
    /// Nothing, when somebody has asked and not been served. A poke that landed
    /// while a cycle was running could not be folded into it — the cycle read
    /// the list before the poke arrived — and the channel that carried it is
    /// about to be thrown away, so this flag is the only thing left that
    /// remembers the request.
    fn next_delay(&self) -> Duration {
        if self.poked.load(Ordering::Relaxed) {
            Duration::ZERO
        } else {
            REFRESH_INTERVAL
        }
    }

    fn is_tracked(&self, dir: &Path) -> bool {
        self.tracked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|tracked| tracked == dir)
    }

    fn forget_untracked(&mut self, ctx: &mut ModelContext<Self>) {
        let tracked = self
            .tracked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();

        let before = self.facts.len();
        self.facts.retain(|dir, _| tracked.contains(dir));
        if self.facts.len() != before {
            ctx.notify();
        }
    }

    /// Cuts the sleeping cycle's wait short. Poking a cycle that is already
    /// awake is harmless: it reads the shared list when it wakes either way.
    ///
    /// The flag is set before the send and never depends on it, because the
    /// send is the half that can be lost: a running cycle's receiver is never
    /// read again, and a finished cycle's is already dropped.
    fn wake_cycle(&self) {
        self.poked.store(true, Ordering::Relaxed);
        if let Some(wake) = self.wake.as_ref() {
            let _ = wake.send(());
        }
    }
}

/// Reads one directory, skipping the subprocess when nothing shows its answer.
fn gather(dir: &Path, with_diff: bool) -> GitFacts {
    if with_diff {
        git::gather(dir)
    } else {
        GitFacts {
            branch: git::current_branch(dir),
            diff: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crookui_core::App;
    use crookui_core::executor::{Background, LocalQueue};

    use super::*;
    use crate::git::{DiffStats, Head};

    /// How long a test waits for the background gather to come home before it
    /// gives up and fails.
    const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

    fn facts(branch: &str) -> GitFacts {
        GitFacts {
            branch: Some(Head::Branch(branch.to_owned())),
            diff: Some(DiffStats::default()),
        }
    }

    /// An app whose local queue the test can drive by hand.
    fn app() -> (Arc<LocalQueue>, App) {
        let queue = LocalQueue::new();
        let app = App::new(queue.foreground(), Arc::new(Background::new(1)));
        (queue, app)
    }

    #[test]
    fn a_directory_nobody_shows_any_more_is_forgotten() {
        // Otherwise the map grows for the life of the process, one entry per
        // directory any session has ever been opened in.
        let (_queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));
        let kept = PathBuf::from("/opt/kept");
        let dropped = PathBuf::from("/opt/dropped");

        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                model.track(vec![kept.clone(), dropped.clone()], ctx);
                model.record(kept.clone(), facts("main"), ctx);
                model.record(dropped.clone(), facts("side"), ctx);
            });
        });
        app.read(|ctx| {
            assert!(model.as_ref(ctx).facts(&kept).is_some());
            assert!(model.as_ref(ctx).facts(&dropped).is_some());
        });

        app.update(|ctx| {
            model.update(ctx, |model, ctx| model.track(vec![kept.clone()], ctx));
        });

        app.read(|ctx| {
            assert!(model.as_ref(ctx).facts(&kept).is_some());
            assert!(
                model.as_ref(ctx).facts(&dropped).is_none(),
                "a directory that left the strip kept its entry"
            );
        });
    }

    #[test]
    fn tracking_the_same_directories_again_is_a_no_op() {
        // `Workspace::sync_git` calls this after every change to the strip,
        // most of which do not move a session anywhere.
        let (_queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));
        let dir = PathBuf::from("/opt/kept");

        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                model.track(vec![dir.clone()], ctx);
                model.record(dir.clone(), facts("main"), ctx);
                // Same set, in a different order: the list is normalised
                // before it is compared, so this must not clear the facts.
                model.track(vec![dir.clone(), dir.clone()], ctx);
            });
        });

        app.read(|ctx| assert!(model.as_ref(ctx).facts(&dir).is_some()));
    }

    #[test]
    fn a_directory_that_appears_with_no_cycle_listening_still_gathers_at_once() {
        // The wake channel cannot carry this on its own. `spawn_cycle` moves
        // its receiver into the cycle that is running, which reads it once at
        // the top; a poke that lands after that goes into a channel nobody
        // will read again, and `finish` then replaces the channel and sleeps
        // out the whole interval. A new tab, or a session that sets a working
        // directory, lands in exactly that window — and a row with no branch
        // for fifteen seconds is what this file promises will not happen.
        let (_queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));

        app.read(|ctx| {
            assert_eq!(
                REFRESH_INTERVAL,
                model.as_ref(ctx).next_delay(),
                "a model nobody has asked anything of is in a hurry"
            );
        });

        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                model.track(vec![PathBuf::from("/opt/appeared")], ctx);
            });
        });

        app.read(|ctx| {
            assert_eq!(
                Duration::ZERO,
                model.as_ref(ctx).next_delay(),
                "the wake was dropped: a directory that just appeared waits                  out the timer before anything reads its branch"
            );
        });
    }

    #[test]
    fn switching_diff_stats_on_with_no_cycle_listening_still_gathers_at_once() {
        // `set_diff_stats_wanted` wakes the cycle precisely so the chips fill
        // in on the click rather than up to fifteen seconds later.
        let (_queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));

        app.update(|ctx| {
            model.update(ctx, |model, _| model.set_diff_stats_wanted(true));
        });
        app.read(|ctx| assert_eq!(Duration::ZERO, model.as_ref(ctx).next_delay()));
    }

    #[test]
    fn switching_diff_stats_off_asks_for_nothing() {
        // What is on screen after it goes off is a subset of what the last
        // cycle already gathered, so there is nothing to hurry.
        let (_queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));

        app.update(|ctx| {
            model.update(ctx, |model, _| model.set_diff_stats_wanted(false));
        });

        app.read(|ctx| assert_eq!(REFRESH_INTERVAL, model.as_ref(ctx).next_delay()));
    }

    #[test]
    fn a_cycle_that_served_the_poke_stops_asking_for_another() {
        // The other half of the flag: it has to be cleared by the cycle that
        // acts on it, or the model polls flat out for the life of the process.
        let Ok(here) = std::env::current_dir() else {
            eprintln!("skipped: the test process has no working directory");
            return;
        };

        let (queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));
        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                model.track(vec![here.clone()], ctx);
                model.start(ctx);
            });
        });

        let deadline = std::time::Instant::now() + DELIVERY_TIMEOUT;
        loop {
            queue.run_until_parked();
            if app.read(|ctx| model.as_ref(ctx).facts(&here).is_some()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the gather never came home"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        app.read(|ctx| {
            assert_eq!(
                REFRESH_INTERVAL,
                model.as_ref(ctx).next_delay(),
                "the poke latched, so the model gathers without pausing"
            );
        });
    }

    #[test]
    fn a_gather_runs_off_the_main_thread_and_comes_home_on_it() {
        // The whole point of the model. It is asserted against Crook's own
        // checkout, because the directory a test runs in is the one directory
        // it can be sure is a repository.
        let Ok(here) = std::env::current_dir() else {
            eprintln!("skipped: the test process has no working directory");
            return;
        };
        let Some(layout) = crate::git::discover(&here) else {
            eprintln!("skipped: {} is not in a repository", here.display());
            return;
        };
        let expected = crate::git::read_head(&layout.git_dir);

        let (queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));
        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                model.track(vec![here.clone()], ctx);
                model.start(ctx);
            });
        });

        // The gather is on a worker thread and its result is delivered by a
        // foreground task, so the test has to drive the queue until it lands.
        let deadline = std::time::Instant::now() + DELIVERY_TIMEOUT;
        loop {
            queue.run_until_parked();
            if app.read(|ctx| model.as_ref(ctx).facts(&here).is_some()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the gather never came home"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        app.read(|ctx| {
            let gathered = model
                .as_ref(ctx)
                .facts(&here)
                .expect("the loop above only exits once there are facts");
            assert_eq!(expected, gathered.branch);
        });
    }
}
