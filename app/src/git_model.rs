//! The git facts behind the tab strip, gathered off the UI thread.
//!
//! [`crate::git`] is blocking by construction: reading `HEAD` is a handful of
//! syscalls and counting changed lines is a `git` subprocess. Neither belongs
//! in `render`, which runs whenever anything on screen moves. Warp learned this
//! the expensive way — its vertical-tab rows derive per-row state inside the
//! renderer and pay for it at frame rate — so nothing here is reachable from a
//! view except [`GitModel::facts`], which is a map lookup.
//!
//! The shape is the one every polling model in Crook has, and this is where it
//! is written down: one cycle runs on the background pool, delivers on the
//! foreground one, and starts the next.
//!
//! # Why a second poll chain cannot start here
//!
//! The same ownership trick. There is exactly one [`Ticket`], it is *moved*
//! into the cycle that is running, and only a finishing cycle can hand it to
//! the next. Nothing else in this file can start a cycle, because nothing else
//! has a ticket to give it. A directory appearing does not start work: it is
//! written into the shared list and the sleeping cycle is poked, and the cycle
//! it wakes is the one that serves it.
//!
//! # What a cycle costs
//!
//! A cycle reads every directory a pane is in, so its cost grows with the
//! panes, and it runs whether or not anybody is looking. Three things keep
//! that in proportion. Each directory reads only what can have changed since
//! the last cycle — see [`git::gather_remembering`] — so a checkout an agent
//! finished with costs a list and an `lstat` per file rather than three walks
//! of its tree. The wait after a cycle grows with what the cycle took, so that
//! however many checkouts there are, git holds a quarter of a core at most;
//! and it is four times as long while the window is behind another, which is
//! when nothing it gathers is being looked at. And the directories whose rows
//! are on screen are gathered and shown first, so a cycle that does have a
//! lot to read keeps the rows somebody can see waiting least.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

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

/// How many times as long as the last cycle took the next one waits, when
/// that is longer than [`REFRESH_INTERVAL`].
///
/// A fixed interval is a share of a core that grows with the checkouts: sixty
/// of them, each a second to read, would keep git busy for most of every
/// minute. Waiting three times what the last cycle took holds git to a
/// quarter of one core however many there are, and the cost is only that a
/// window with that much to read refreshes less often.
const PATIENCE: u32 = 3;

/// How many times as long a cycle waits while the window is behind another.
///
/// Nobody is reading the rows then, and a window left open behind an editor
/// all day is the common case rather than the odd one. Coming back to it cuts
/// the wait back down — see [`GitModel::set_window_focused`].
const AWAY: u32 = 4;

/// The right to run a gather cycle. There is exactly one.
///
/// See the module docs: this is what makes a second poll chain unrepresentable
/// rather than merely unlikely.
///
/// It also carries what the chain remembers from one cycle to the next: each
/// repository's base, which is up to three subprocesses to find and almost
/// never changes, and what each working tree read last, and from what. On the
/// ticket rather than behind a lock on the model, because only the cycle
/// holding the ticket ever reads or writes them, and there is only ever one.
struct Ticket {
    bases: git::Bases,
    readings: git::Readings,
}

/// One part of a cycle, come home: what it gathered, and what is still to
/// gather.
struct Part {
    ticket: Ticket,
    gathered: Vec<(PathBuf, GitFacts)>,
    /// The directories the cycle has yet to gather. Empty for its last part.
    rest: Vec<PathBuf>,
    /// Whether the cycle runs the subprocesses, as it was when it started.
    with_diff: bool,
    /// How long the cycle has spent gathering so far.
    took: Duration,
}

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
    /// The subprocesses — the diff, and the count since the base on a branch —
    /// only run when a chip would print their answer, which is Warp's
    /// `needs_git_status_for_chip_ui` rule: do not pay for git when nothing
    /// displays git.
    wants_diff: Arc<AtomicBool>,

    /// The directories whose rows are on screen, which a cycle gathers and
    /// delivers before the rest.
    ///
    /// Shared for the reason [`Self::tracked`] is. A hint and nothing more: a
    /// directory here that is not tracked is never gathered for being here.
    in_sight: Arc<Mutex<Vec<PathBuf>>>,

    /// Cuts the sleeping cycle's wait short. Each message is the latest
    /// moment the model wants the next gather by; the cycle wakes at the
    /// soonest of those and its own deadline.
    ///
    /// Replaced by every [`Self::spawn_cycle`], which is why it cannot be the
    /// only record that somebody asked — see [`Self::poked`].
    wake: Option<Sender<Instant>>,

    /// Whether anybody has asked for a gather that has not happened yet.
    ///
    /// The channel alone loses the request that matters most. A wake sent
    /// while a cycle is *running* goes into a receiver that has already been
    /// read for the last time, and [`Self::finish`] then throws that channel
    /// away and schedules the next cycle a full [`REFRESH_INTERVAL`] out — so
    /// "show it now" silently becomes "show it in fifteen seconds", in exactly
    /// the window a person is most likely to be in: the cycle a new tab or a
    /// switched-on toggle just started. This flag outlives the channel, and
    /// `finish` reads it — which is the whole reason a poke is a flag rather
    /// than a message on the channel the cycle owns.
    poked: Arc<AtomicBool>,

    /// The ticket, while no cycle owns it. `Some` only before [`Self::start`].
    idle_ticket: Option<Ticket>,

    /// Whether the window has the desktop's keyboard. See [`AWAY`].
    focused: bool,

    /// How long the last cycle spent gathering. See [`PATIENCE`].
    took: Duration,

    /// When the last cycle came home, which is what the wait before the next
    /// is counted from.
    finished: Option<Instant>,
}

// The event is the end of a cycle, emitted whether it found anything new or
// not: the Changes column re-reads on it. See `finish` for why.
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
            in_sight: Arc::new(Mutex::new(Vec::new())),
            wake: None,
            poked: Arc::new(AtomicBool::new(false)),
            idle_ticket: Some(Ticket {
                bases: git::Bases::default(),
                readings: git::Readings::default(),
            }),
            focused: true,
            took: Duration::ZERO,
            finished: None,
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

    /// Tells the model which of the tracked directories have a row on
    /// screen, for the next cycle to gather first.
    ///
    /// Wakes nothing: what is on screen changing changes which rows wait
    /// least, not when anything is read.
    pub fn set_in_sight(&mut self, mut dirs: Vec<PathBuf>) {
        dirs.sort();
        dirs.dedup();
        *self.in_sight.lock().unwrap_or_else(PoisonError::into_inner) = dirs;
    }

    /// Tells the model whether the window has the desktop's keyboard.
    ///
    /// Losing it lengthens the wait after the next cycle — see [`AWAY`].
    /// Getting it back shortens the wait the sleeping cycle is already in to
    /// the one a focused window would have had, counted from when the last
    /// cycle came home: a window that was away for longer than that gathers
    /// at once, and one that was away for a moment waits out the rest.
    pub fn set_window_focused(&mut self, focused: bool) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        if focused
            && let Some(finished) = self.finished
            && let Some(wake) = self.wake.as_ref()
        {
            let _ = wake.send(finished + self.interval());
        }
    }

    /// The directories the next gather reads, which is every one a pane
    /// is working in and nothing else.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> Vec<PathBuf> {
        self.tracked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
        let in_sight = self.in_sight.clone();
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
                        // Returns early when the model asks for a gather sooner
                        // — a poke, or the window coming back — and at `delay`
                        // otherwise.
                        wait(&wakeup, Instant::now() + delay);

                        // Cleared here rather than by the sender, so that a
                        // poke arriving from this line onwards is still
                        // outstanding when `finish` asks: the gather below has
                        // already read the list it is going to read.
                        poked.store(false, Ordering::Relaxed);

                        let dirs = tracked
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone();
                        let in_sight = in_sight
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .clone();
                        let (first, rest) = in_sight_first(dirs, &in_sight);
                        let with_diff = wants_diff.load(Ordering::Relaxed);
                        let mut ticket = ticket;
                        let started = Instant::now();
                        let gathered = gather_all(first, with_diff, &mut ticket);

                        Part {
                            ticket,
                            gathered,
                            rest,
                            with_diff,
                            took: started.elapsed(),
                        }
                    })
                    .await
            },
            |model, part, ctx| model.deliver(part, ctx),
        )
        .detach();
    }

    /// Records one part of a cycle, then gathers the rest of it or, when
    /// there is none, finishes it.
    ///
    /// The rows on screen are the first part, and they are shown before the
    /// rest is read: the hop back to this thread between the two is what lets
    /// a cycle with a lot to read show what somebody is looking at first.
    fn deliver(&mut self, part: Part, ctx: &mut ModelContext<Self>) {
        let Part {
            ticket,
            gathered,
            rest,
            with_diff,
            took,
        } = part;
        self.record_gathered(gathered, ctx);
        if rest.is_empty() {
            self.finish(ticket, with_diff, took, ctx);
            return;
        }

        let background = ctx.background().clone();
        ctx.spawn(
            async move {
                background
                    .spawn(async move {
                        let mut ticket = ticket;
                        let started = Instant::now();
                        let gathered = gather_all(rest, with_diff, &mut ticket);
                        Part {
                            ticket,
                            gathered,
                            rest: Vec::new(),
                            with_diff,
                            took: took + started.elapsed(),
                        }
                    })
                    .await
            },
            |model, part, ctx| model.deliver(part, ctx),
        )
        .detach();
    }

    /// Puts what a part of a cycle gathered in front of the renderer.
    fn record_gathered(
        &mut self,
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
    }

    /// Ends a cycle and starts the next one.
    ///
    /// A cycle that ran the subprocesses forgets the repositories and working
    /// trees none of its directories asked about; one that did not has asked
    /// nothing, and forgetting after it would throw everything away for the
    /// sake of a toggle.
    fn finish(
        &mut self,
        mut ticket: Ticket,
        with_diff: bool,
        took: Duration,
        ctx: &mut ModelContext<Self>,
    ) {
        if with_diff {
            ticket.bases.forget_unasked();
            ticket.readings.forget_unvisited();
        }
        self.took = took;
        self.finished = Some(Instant::now());

        // Every cycle says it ran, changed or not, which is the beat the
        // Changes column re-reads on: what it shows — commits, files — is
        // nothing gathered here, so "the facts changed" is no sign of it
        // having changed, and a column with a timer of its own would be a
        // second poll chain on a window that promises one.
        ctx.emit(());

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
            self.interval()
        }
    }

    /// The wait between cycles nobody has asked to hurry: [`REFRESH_INTERVAL`],
    /// or [`PATIENCE`] times the last cycle when that is longer, and [`AWAY`]
    /// times that while the window is behind another.
    fn interval(&self) -> Duration {
        let interval = REFRESH_INTERVAL.max(self.took * PATIENCE);
        if self.focused {
            interval
        } else {
            interval * AWAY
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
            let _ = wake.send(Instant::now());
        }
    }
}

/// Sleeps until `by`, or until the model asks for a gather sooner.
///
/// Every message is a moment the model wants the gather by, and the wait goes
/// on to the soonest of them — which is how coming back to the window cuts a
/// wait short without cutting it to nothing. A model that has gone, and taken
/// the sender with it, ends the wait.
fn wait(wakeup: &Receiver<Instant>, mut by: Instant) {
    loop {
        let left = by.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        match wakeup.recv_timeout(left) {
            Ok(sooner) => by = by.min(sooner),
            Err(_) => return,
        }
    }
}

/// `dirs`, as the part of a cycle to gather first and the part to gather
/// after it: the directories `in_sight` names, then the rest.
///
/// One part, with nothing after it, when either would be empty: a second hop
/// to the main thread buys nothing when nothing is on screen, or when
/// everything is.
fn in_sight_first(dirs: Vec<PathBuf>, in_sight: &[PathBuf]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (first, rest): (Vec<_>, Vec<_>) = dirs.into_iter().partition(|dir| in_sight.contains(dir));
    if first.is_empty() {
        (rest, Vec::new())
    } else {
        (first, rest)
    }
}

/// Reads the directories of one part of a cycle, skipping the subprocesses
/// when nothing shows their answer.
///
/// What the ticket carries is what makes the reads cheap: a repository's base
/// is looked up by the first cycle that needs it and read back by every one
/// after, and a working tree is read again only for what can have changed
/// since the last cycle read it. See [`GitModel::finish`] for when either is
/// forgotten.
fn gather_all(
    dirs: Vec<PathBuf>,
    with_diff: bool,
    ticket: &mut Ticket,
) -> Vec<(PathBuf, GitFacts)> {
    dirs.into_iter()
        .map(|dir| {
            let facts = if with_diff {
                git::gather_remembering(&dir, &mut ticket.bases, &mut ticket.readings)
            } else {
                git::facts_without_diff(&dir)
            };
            (dir, facts)
        })
        .collect()
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
            since_base: None,
            worktree: false,
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
    fn a_cycle_that_finds_nothing_new_still_says_it_finished() {
        // The Changes column re-reads on the cycle rather than on a timer of
        // its own, and what it re-reads — commits, files — is nothing this
        // model gathers. A cycle whose facts matched the last one notifies
        // nobody, so the event is the only thing that says it ran.
        struct Heard(usize);
        impl Entity for Heard {
            type Event = ();
        }

        let outside =
            std::env::temp_dir().join(format!("crook-git-model-cycle-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&outside);

        let (queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));
        let heard = app.update(|ctx| {
            ctx.add_model(|ctx| {
                ctx.subscribe_to_model(&model, |heard: &mut Heard, _, _, _| heard.0 += 1);
                Heard(0)
            })
        });
        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                model.track(vec![outside.clone()], ctx);
                // What the gather is about to find, so it finds nothing new.
                model.record(outside.clone(), GitFacts::default(), ctx);
                model.start(ctx);
            });
        });

        let deadline = std::time::Instant::now() + DELIVERY_TIMEOUT;
        loop {
            queue.run_until_parked();
            if app.read(|ctx| heard.as_ref(ctx).0) > 0 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "a cycle that changed nothing said nothing"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_long_cycle_waits_three_times_as_long_before_the_next() {
        // Sixty checkouts at a second apiece, fifteen seconds apart, would be
        // git running for four minutes in every five.
        let (_queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));

        app.update(|ctx| model.update(ctx, |model, _| model.took = Duration::from_secs(2)));
        app.read(|ctx| assert_eq!(REFRESH_INTERVAL, model.as_ref(ctx).next_delay()));

        app.update(|ctx| model.update(ctx, |model, _| model.took = Duration::from_secs(10)));
        app.read(|ctx| assert_eq!(Duration::from_secs(30), model.as_ref(ctx).next_delay()));
    }

    #[test]
    fn a_window_behind_another_waits_four_times_as_long() {
        let (_queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));

        app.update(|ctx| model.update(ctx, |model, _| model.set_window_focused(false)));
        app.read(|ctx| assert_eq!(REFRESH_INTERVAL * 4, model.as_ref(ctx).next_delay()));

        // Somebody asking still means now, focused or not.
        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                model.track(vec![PathBuf::from("/opt/appeared")], ctx);
            });
        });
        app.read(|ctx| assert_eq!(Duration::ZERO, model.as_ref(ctx).next_delay()));
    }

    #[test]
    fn coming_back_to_the_window_cuts_the_wait_it_is_in_short() {
        // The cycle asleep was put to sleep for the minute an unfocused window
        // waits. Coming back, the rows should be no older than a focused
        // window's would be: counted from when the last cycle came home.
        let (_queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));
        let (wake, wakeup) = mpsc::channel();
        let came_home = Instant::now() - Duration::from_secs(20);
        app.update(|ctx| {
            model.update(ctx, |model, _| {
                model.wake = Some(wake);
                model.finished = Some(came_home);
                model.set_window_focused(false);
            });
        });
        assert!(wakeup.try_recv().is_err(), "going away hurried the cycle");

        app.update(|ctx| model.update(ctx, |model, _| model.set_window_focused(true)));

        assert_eq!(wakeup.try_recv().ok(), Some(came_home + REFRESH_INTERVAL));
    }

    #[test]
    fn a_sooner_moment_on_the_channel_ends_the_wait_then() {
        let (wake, wakeup) = mpsc::channel();
        let started = Instant::now();
        wake.send(started).expect("the receiver is right here");

        wait(&wakeup, started + Duration::from_secs(60));

        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn the_rows_on_screen_come_home_before_the_rest_is_read() {
        // A cycle with twenty checkouts to read is seconds long, and a row
        // somebody is looking at should not wait behind nineteen nobody is.
        struct Heard(Vec<(bool, bool)>);
        impl Entity for Heard {
            type Event = ();
        }

        let scratch =
            std::env::temp_dir().join(format!("crook-git-model-in-sight-{}", std::process::id()));
        let (seen, unseen) = (scratch.join("seen"), scratch.join("unseen"));
        for dir in [&seen, &unseen] {
            let _ = std::fs::create_dir_all(dir);
        }

        let (queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(GitModel::new));
        let heard = app.update(|ctx| {
            let (seen, unseen) = (seen.clone(), unseen.clone());
            ctx.add_model(|ctx| {
                ctx.observe(&model, move |heard: &mut Heard, model, ctx| {
                    let model = model.as_ref(ctx);
                    heard
                        .0
                        .push((model.facts(&seen).is_some(), model.facts(&unseen).is_some()));
                });
                Heard(Vec::new())
            })
        });
        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                model.track(vec![seen.clone(), unseen.clone()], ctx);
                model.set_in_sight(vec![seen.clone()]);
                model.start(ctx);
            });
        });

        let deadline = Instant::now() + DELIVERY_TIMEOUT;
        loop {
            queue.run_until_parked();
            if app.read(|ctx| heard.as_ref(ctx).0.contains(&(true, true))) {
                break;
            }
            assert!(Instant::now() < deadline, "the gather never came home");
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&scratch);

        app.read(|ctx| {
            assert_eq!(
                heard.as_ref(ctx).0.first(),
                Some(&(true, false)),
                "the row on screen came home with the rest, not before it"
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
