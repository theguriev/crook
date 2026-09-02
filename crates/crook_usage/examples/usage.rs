//! Prints one Claude Code usage reading and exits.
//!
//! The whole feature minus pixels: it reads this machine's Claude Code session
//! and hits the usage endpoint exactly the way the header chip will, so the
//! macOS, Linux and Windows paths can be checked before any UI exists.

use chrono::Utc;
use crook_usage::{RefreshTrigger, UsagePoller};

fn main() -> std::process::ExitCode {
    let mut poller = UsagePoller::new();
    poller.refresh(RefreshTrigger::User);

    let Some(snapshot) = poller.snapshot() else {
        // `last_error` is always set when there is no snapshot, but the fallback
        // keeps this a report rather than a panic.
        let message = poller
            .last_error()
            .map(|err| err.user_facing_message())
            .unwrap_or_else(|| "Claude reported no usage".to_string());
        eprintln!("{message}");
        return std::process::ExitCode::FAILURE;
    };

    let countdown = snapshot
        .time_until_session_reset(Utc::now())
        .unwrap_or_else(|| "unknown".to_string());
    println!(
        "{}% · resets in {countdown}",
        snapshot.session_percent_rounded()
    );
    std::process::ExitCode::SUCCESS
}
