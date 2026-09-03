//! Claude Code usage as an application model.
//!
//! [`crook_usage`] deliberately owns no thread and no timer: [`UsagePoller`]
//! is a blocking state machine and someone else has to decide what runs it.
//! This is that someone. It runs each cycle on the background executor,
//! delivers the result on the foreground one, and calls `notify` so the header
//! repaints — which is the whole of the model half of the feature.
//!
//! # Why a second poll chain cannot start here
//!
//! Warp's version of this owns its own schedule: every completed fetch spawns
//! the next poll from its own completion callback, and `refresh()` early
//! returns on an `is_refreshing` flag without cancelling the timer that is
//! already outstanding. A click that lands between polls therefore forks a
//! second self-rescheduling chain, and both run for the rest of the session —
//! N clicks leave N+1 chains and N+1 requests a minute.
//!
//! The fix here is ownership, not a flag. There is exactly one
//! [`UsagePoller`], it is *moved* into the cycle that is running, and a cycle
//! is the only thing that can start the next one. Nothing else in this file
//! can spawn a cycle, because nothing else has a poller to give it. A click
//! does not start work at all: it sets a shared flag and pokes the sleeping
//! cycle awake, and the cycle it wakes is the one that serves it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use crook_usage::{
    ClaudeUsageError, ClaudeUsageSnapshot, RefreshOutcome, RefreshTrigger, UsagePoller,
};
use crookui_core::prelude::*;

/// Why the chip has no percentage to show.
///
/// A summary of [`ClaudeUsageError`] short enough to fit in a pill. The full
/// sentence goes to the log, where there is room for it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UsageProblem {
    /// Nothing on this machine has ever run Claude Code.
    NoSession,
    /// A session exists, but Claude will not accept it any more.
    SessionExpired,
    /// Claude could not be reached, or answered with something unusable.
    Unreachable,
}

impl UsageProblem {
    /// Whether this makes the last reading meaningless rather than merely old.
    ///
    /// A refusal or a network blip leaves a percentage that was true a minute
    /// ago and that the next cycle will refresh a minute from now, so showing
    /// it — visibly stale — beats replacing it with an apology. A session that
    /// is missing or rejected is different in kind: nothing will refresh that
    /// number until a person logs in again, so it can only get more wrong, and
    /// the chip has to say what happened instead.
    pub fn invalidates_the_reading(self) -> bool {
        matches!(self, Self::NoSession | Self::SessionExpired)
    }

    /// What the chip says in place of a percentage.
    pub fn chip_label(self) -> &'static str {
        match self {
            Self::NoSession => "no session",
            Self::SessionExpired => "session expired",
            Self::Unreachable => "unavailable",
        }
    }
}

impl From<&ClaudeUsageError> for UsageProblem {
    fn from(error: &ClaudeUsageError) -> Self {
        match error {
            ClaudeUsageError::NoSession => Self::NoSession,
            ClaudeUsageError::SessionExpired | ClaudeUsageError::Unauthorized => {
                Self::SessionExpired
            }
            ClaudeUsageError::BadResponse(_) | ClaudeUsageError::Unexpected(_) => Self::Unreachable,
        }
    }
}

/// The application's single view of Claude Code usage.
pub struct UsageModel {
    snapshot: Option<ClaudeUsageSnapshot>,
    problem: Option<UsageProblem>,

    /// The poller, while no cycle owns it. `Some` before the first
    /// [`Self::set_wanted`] and again once a cycle has parked it; while a
    /// chain is running it lives inside that cycle, which is what makes a
    /// second chain unrepresentable.
    idle_poller: Option<UsagePoller>,

    /// Whether anything on screen is showing the reading.
    ///
    /// The chip is the only thing that does, so this is the settings page's
    /// "Show the usage chip" switch, one indirection away. It gates the chain
    /// rather than merely the pixels: a hidden chip that went on polling would
    /// keep talking to Anthropic's servers every minute on behalf of a person
    /// who has just said they do not want to see the number.
    wanted: bool,

    /// Cuts the sleeping cycle's wait short. Sending to a cycle that is
    /// already awake is harmless: `user_request` is what actually carries the
    /// request, and this only saves it from waiting for the timer.
    wake: Option<Sender<()>>,

    /// Set by a click and consumed by whichever cycle picks it up. Shared
    /// rather than passed, because the cycle that will serve a click is
    /// usually already asleep when the click happens.
    user_request: Arc<AtomicBool>,

    /// Whether a person is waiting on a reading right now.
    busy_for_user: bool,
}

impl Entity for UsageModel {
    type Event = ();
}

impl SingletonEntity for UsageModel {}

impl UsageModel {
    /// A model that has read nothing and is not polling yet.
    ///
    /// Polling starts at [`Self::start`] rather than here, so a headless run
    /// that only wants to render a frame never touches the network.
    pub fn new(_: &mut ModelContext<Self>) -> Self {
        Self {
            snapshot: None,
            problem: None,
            idle_poller: Some(UsagePoller::new()),
            wanted: false,
            wake: None,
            user_request: Arc::new(AtomicBool::new(false)),
            busy_for_user: false,
        }
    }

    /// The last successful reading, if there has been one.
    pub fn snapshot(&self) -> Option<&ClaudeUsageSnapshot> {
        self.snapshot.as_ref()
    }

    /// Why there is no reading, if there is not one.
    pub fn problem(&self) -> Option<UsageProblem> {
        self.problem
    }

    /// Whether a person clicked and is still waiting.
    ///
    /// The chip shows this; a background poll deliberately does not set it. A
    /// poll that repainted the header would redraw the whole tab bar on a
    /// timer for nobody's benefit.
    pub fn is_busy_for_user(&self) -> bool {
        self.busy_for_user
    }

    /// Says whether the reading is being shown, starting and stopping the
    /// poll chain to match.
    ///
    /// Turning it on reads immediately rather than after a wait, so switching
    /// the chip on fills it in on the click. Turning it off does not cancel
    /// the request already in flight — there is no way to, and abandoning its
    /// answer would only mean fetching it again — so the chain stops one cycle
    /// later, in [`Self::finish`], which is where the poller comes back.
    ///
    /// Idempotent in both directions: calling it with the value it already has
    /// cannot start a second chain, and neither can calling it twice while a
    /// cycle owns the poller.
    pub fn set_wanted(&mut self, wanted: bool, ctx: &mut ModelContext<Self>) {
        if self.wanted == wanted {
            return;
        }
        self.wanted = wanted;

        if wanted && let Some(poller) = self.idle_poller.take() {
            self.spawn_cycle(poller, Duration::ZERO, ctx);
        }
    }

    /// Whether the poll chain is meant to be running.
    pub fn is_wanted(&self) -> bool {
        self.wanted
    }

    /// Asks for a reading now, on a person's behalf.
    pub fn refresh_from_user(&mut self, ctx: &mut ModelContext<Self>) {
        self.user_request.store(true, Ordering::Relaxed);
        if let Some(wake) = self.wake.as_ref() {
            let _ = wake.send(());
        }

        if !self.busy_for_user {
            self.busy_for_user = true;
            ctx.notify();
        }
    }

    /// Runs one cycle: wait, read, hand the poller back on the main thread.
    fn spawn_cycle(
        &mut self,
        mut poller: UsagePoller,
        delay: Duration,
        ctx: &mut ModelContext<Self>,
    ) {
        let (wake, wakeup) = mpsc::channel();
        self.wake = Some(wake);

        let user_request = self.user_request.clone();
        let background = ctx.background().clone();

        // The whole cycle — the wait and the blocking HTTP call — is one
        // background task, so the pool holds one worker for one chain and
        // there is nothing to keep in sync between a timer and a request.
        ctx.spawn(
            async move {
                background
                    .spawn(async move {
                        if !delay.is_zero() {
                            // Returns early when a click pokes the channel,
                            // and on the timeout otherwise. Either way the
                            // flag below decides what kind of cycle this is.
                            let _ = wakeup.recv_timeout(delay);
                        }

                        let trigger = if user_request.swap(false, Ordering::Relaxed) {
                            RefreshTrigger::User
                        } else {
                            RefreshTrigger::Poll
                        };

                        let outcome = poller.refresh(trigger);
                        (poller, outcome)
                    })
                    .await
            },
            |model, (poller, outcome), ctx| model.finish(poller, outcome, ctx),
        )
        .detach();
    }

    /// Records a cycle's result and starts the next one.
    fn finish(
        &mut self,
        poller: UsagePoller,
        outcome: RefreshOutcome,
        ctx: &mut ModelContext<Self>,
    ) {
        let changed = self.absorb(&poller);
        if outcome.user_initiated {
            self.busy_for_user = false;
        }

        // A poll that read the same numbers as the last one repaints nothing.
        // A cycle a person asked for always repaints, because at the very
        // least the chip has to stop saying it is working.
        if changed || outcome.user_initiated {
            ctx.notify();
        }

        // Nobody is looking at the number any more. The poller parks here
        // rather than at the moment the switch was thrown, because that moment
        // is in the middle of a blocking HTTP call this has no handle on.
        if !self.wanted {
            self.wake = None;
            self.idle_poller = Some(poller);
            return;
        }

        // A click that arrived while this cycle was already fetching could not
        // be folded into it, so the next one runs immediately instead.
        let delay = if self.user_request.load(Ordering::Relaxed) {
            Duration::ZERO
        } else {
            outcome.next_poll_delay
        };

        self.spawn_cycle(poller, delay, ctx);
    }

    /// Copies what the poller read into the model, reporting whether anything
    /// a viewer could see actually changed.
    fn absorb(&mut self, poller: &UsagePoller) -> bool {
        let snapshot = poller.snapshot().cloned();
        let problem = poller.last_error().map(UsageProblem::from);

        if let Some(error) = poller.last_error() {
            log::warn!("Claude usage unavailable: {}", error.user_facing_message());
        }

        let changed = snapshot != self.snapshot || problem != self.problem;
        self.snapshot = snapshot;
        self.problem = problem;

        if changed && let Some(snapshot) = self.snapshot.as_ref() {
            log::debug!(
                "Claude session usage {}%",
                snapshot.session_percent_rounded()
            );
        }

        changed
    }
}
