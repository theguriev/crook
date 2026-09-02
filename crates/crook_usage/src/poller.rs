//! Drives [`fetch_usage`] on whatever schedule the caller likes.
//!
//! The poller owns the state an indicator needs — the last snapshot, the last
//! failure, and the session token cached between reads — but neither a thread
//! nor a timer. [`UsagePoller::refresh`] blocks for one cycle and returns how
//! long to wait before the next one; deciding what runs it is the app's job,
//! because only the app knows which thread is allowed to block.
//!
//! A driver is a loop:
//!
//! ```no_run
//! use std::sync::mpsc::{Receiver, RecvTimeoutError};
//!
//! use crook_usage::{RefreshTrigger, UsagePoller};
//!
//! /// Runs on a background thread; `clicks` carries user-initiated refreshes.
//! fn drive(clicks: Receiver<()>) {
//!     let mut poller = UsagePoller::new();
//!     let mut trigger = RefreshTrigger::Poll;
//!     loop {
//!         let outcome = poller.refresh(trigger);
//!         // Report `poller.snapshot()` / `poller.last_error()` to the UI here,
//!         // chomping when `outcome.user_initiated`.
//!         trigger = match clicks.recv_timeout(outcome.next_poll_delay) {
//!             Ok(()) => RefreshTrigger::User,
//!             Err(RecvTimeoutError::Timeout) => RefreshTrigger::Poll,
//!             Err(RecvTimeoutError::Disconnected) => return,
//!         };
//!     }
//! }
//! ```

use std::time::Duration;

use chrono::Utc;

use crate::credentials::{self, ClaudeAccessToken};
use crate::usage::{
    ClaudeUsageError, ClaudeUsageSnapshot, IDLE_POLL_INTERVAL, POLL_INTERVAL, fetch_usage,
};

/// Who asked for a refresh.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RefreshTrigger {
    /// A person clicked the indicator.
    User,
    /// The poll interval elapsed.
    Poll,
}

/// What one refresh cycle produced.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RefreshOutcome {
    /// Whether a person asked for this cycle, and so should get the click
    /// animation. A background poll must never animate: it would repaint the
    /// header on a timer for no one's benefit.
    pub user_initiated: bool,
    /// How long to wait before the next [`RefreshTrigger::Poll`] refresh.
    pub next_poll_delay: Duration,
}

/// Claude Code usage, refreshed on demand.
///
/// Deliberately not `Debug`: it holds a live access token.
#[derive(Default)]
pub struct UsagePoller {
    snapshot: Option<ClaudeUsageSnapshot>,
    /// The most recent failure, kept so a caller can explain an empty chip.
    last_error: Option<ClaudeUsageError>,
    /// The session token used by the last cycle, reused until it expires so a
    /// poll doesn't touch the Keychain (which can prompt) once a minute.
    cached_token: Option<ClaudeAccessToken>,
    next_poll_delay: Duration,
}

impl UsagePoller {
    /// A poller that has read nothing yet, and whose first refresh is due now.
    pub fn new() -> Self {
        Self::default()
    }

    /// The last successful reading, if there has been one.
    pub fn snapshot(&self) -> Option<&ClaudeUsageSnapshot> {
        self.snapshot.as_ref()
    }

    /// Why the last cycle failed, cleared by the next success.
    pub fn last_error(&self) -> Option<&ClaudeUsageError> {
        self.last_error.as_ref()
    }

    /// How long the driver should wait before the next poll. Zero before the
    /// first refresh, 60s while a session is readable, 10 minutes when there is
    /// no session to read — every other failure is transient and the user may
    /// be fixing it right now, so those keep the fast cadence.
    pub fn next_poll_delay(&self) -> Duration {
        self.next_poll_delay
    }

    /// Runs one blocking cycle: read the session, ask Claude, record the result.
    ///
    /// Warp's version of this owned its own schedule — each completed fetch
    /// spawned the next poll from its own callback — so a click that landed
    /// between polls forked a second self-rescheduling chain and both ran for
    /// the rest of the session. That cannot happen here: `refresh` schedules
    /// nothing, so the number of pending polls is however many the driver
    /// keeps, and `&mut self` makes two overlapping cycles impossible without
    /// an `is_refreshing` flag to get wrong.
    pub fn refresh(&mut self, trigger: RefreshTrigger) -> RefreshOutcome {
        let result = self.access_token().and_then(|token| {
            let snapshot = fetch_usage(&token.token)?;
            Ok((snapshot, token))
        });

        self.next_poll_delay = match result {
            Ok((snapshot, token)) => {
                self.snapshot = Some(snapshot);
                self.cached_token = Some(token);
                self.last_error = None;
                POLL_INTERVAL
            }
            Err(err) => {
                let idle = matches!(
                    err,
                    ClaudeUsageError::NoSession | ClaudeUsageError::SessionExpired
                );
                log::warn!("Failed to refresh Claude Code usage: {err:#}");
                self.last_error = Some(err);
                if idle {
                    IDLE_POLL_INTERVAL
                } else {
                    POLL_INTERVAL
                }
            }
        };

        RefreshOutcome {
            user_initiated: trigger == RefreshTrigger::User,
            next_poll_delay: self.next_poll_delay,
        }
    }

    /// The cached token while it is still good, otherwise a fresh read from
    /// storage. Taking it means a failed cycle leaves nothing cached, which is
    /// what we want: a token Claude rejected is exactly the case where the copy
    /// on disk has already moved on.
    fn access_token(&mut self) -> Result<ClaudeAccessToken, ClaudeUsageError> {
        match self.cached_token.take() {
            Some(token) if token.is_usable(Utc::now()) => Ok(token),
            _ => credentials::load_access_token(),
        }
    }
}
