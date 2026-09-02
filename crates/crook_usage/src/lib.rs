//! Claude Code usage, read from the session Claude Code already keeps on disk.
//!
//! Claude's OAuth usage endpoint reports how much of the rolling 5-hour session
//! window (and of the weekly limits) has been burned through. This crate reads
//! the token Claude Code stores locally — it never signs in, and it stores no
//! credential of its own — asks the endpoint, and hands back a snapshot the
//! header can draw.
//!
//! It knows nothing about drawing, and nothing about Crook's executor: the
//! network call blocks, and [`UsagePoller`] is a plain state machine the app
//! drives from whichever thread it likes. That is the whole reason this is a
//! separate crate.

mod credentials;
mod poller;
mod usage;

pub use poller::{RefreshOutcome, RefreshTrigger, UsagePoller};
pub use usage::{
    ClaudeExtraUsage, ClaudeUsageError, ClaudeUsageLevel, ClaudeUsageSnapshot, fetch_usage,
};

/// Re-exported so matching on [`ClaudeUsageError::BadResponse`] doesn't drag
/// this crate's HTTP client into the caller.
pub use ureq::http::StatusCode;
