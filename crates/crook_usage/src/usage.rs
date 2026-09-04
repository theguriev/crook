//! The usage endpoint: its wire format, the snapshot it turns into, and the one
//! blocking GET that produces it.
//!
//! `GET /api/oauth/usage` is Claude Code's own undocumented endpoint, pinned to
//! a beta header. It can change shape without notice, which is why every field
//! below is optional and a response that reports nothing degrades to 0% instead
//! of failing.

use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use ureq::Agent;
use ureq::http::StatusCode;

/// Claude's internal usage endpoint, the same one the Claude Code CLI reads.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// Beta header required by the OAuth-authenticated endpoints.
const OAUTH_BETA_HEADER: &str = "oauth-2025-04-20";

/// How often usage is refreshed while a session is readable.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Back-off used when there is no Claude Code session to read; there is nothing
/// to poll until the user runs Claude Code, so check back rarely.
pub(crate) const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(10 * 60);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// How usage is doing against the session limit. Drives the color of the
/// percentage in the header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClaudeUsageLevel {
    /// Below 50%.
    Normal,
    /// 50–80%.
    Elevated,
    /// 80–95%.
    High,
    /// Above 95%.
    Critical,
}

impl ClaudeUsageLevel {
    /// Which band a percentage falls in.
    ///
    /// Public because the chip is not the only thing that colours by usage:
    /// the panel under it draws the weekly limit in the same four colours, and
    /// two thresholds tables that agreed only by accident would be worse than
    /// one.
    pub fn from_percent(percent: f32) -> Self {
        if percent < 50. {
            Self::Normal
        } else if percent < 80. {
            Self::Elevated
        } else if percent < 95. {
            Self::High
        } else {
            Self::Critical
        }
    }
}

/// Extra (over-limit) usage, only meaningful when the user has enabled it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClaudeExtraUsage {
    /// Credits spent, in minor currency units (i.e. cents).
    pub used_credits: f64,
    /// Monthly cap, in minor currency units.
    pub monthly_limit: f64,
}

/// A point-in-time view of Claude usage, as shown in the header.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeUsageSnapshot {
    /// Utilization of the rolling 5-hour session window, 0–100.
    pub session_percent: f32,
    /// When the rolling session window rolls over, when the plan reports one.
    pub session_resets_at: Option<DateTime<Utc>>,
    /// Utilization of the weekly limit, 0–100, when the plan has one.
    pub weekly_percent: Option<f32>,
    /// When the weekly window rolls over, when the plan reports one.
    pub weekly_resets_at: Option<DateTime<Utc>>,
    /// Pay-as-you-go usage past the plan limit, when the user has enabled it.
    pub extra_usage: Option<ClaudeExtraUsage>,
}

impl ClaudeUsageSnapshot {
    /// The session percentage, clamped and rounded for display.
    pub fn session_percent_rounded(&self) -> u32 {
        self.session_percent.clamp(0., 100.).round() as u32
    }

    /// Which color band the session percentage falls in.
    pub fn level(&self) -> ClaudeUsageLevel {
        ClaudeUsageLevel::from_percent(self.session_percent)
    }

    /// A short "2h 18m" style countdown to the session reset, if one is known.
    pub fn time_until_session_reset(&self, now: DateTime<Utc>) -> Option<String> {
        let resets_at = self.session_resets_at?;
        Some(format_countdown(resets_at.signed_duration_since(now)))
    }

    /// The same countdown for the weekly window.
    pub fn time_until_weekly_reset(&self, now: DateTime<Utc>) -> Option<String> {
        let resets_at = self.weekly_resets_at?;
        Some(format_countdown(resets_at.signed_duration_since(now)))
    }
}

/// Truncates rather than rounds: 119 seconds reads as "1m", and the day branch
/// drops minutes entirely.
fn format_countdown(remaining: chrono::TimeDelta) -> String {
    let seconds = remaining.num_seconds();
    if seconds <= 0 {
        return "now".to_string();
    }
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

/// Why usage could not be read. Every variant is caused by the local
/// environment or by Claude's server, so none of them are a bug in Crook.
#[derive(thiserror::Error, Debug)]
pub enum ClaudeUsageError {
    /// Nothing on this machine has a Claude Code session.
    #[error("No Claude Code session found")]
    NoSession,
    /// A session exists, but its token is past its expiry.
    #[error("Claude Code session expired")]
    SessionExpired,
    /// The token was sent and Claude turned it down.
    #[error("Claude rejected the Claude Code session")]
    Unauthorized,
    /// A non-success status that isn't about the credentials.
    #[error("Claude returned an unexpected response")]
    BadResponse(StatusCode),
    /// Transport, TLS, or a response body that no longer parses.
    #[error(transparent)]
    Unexpected(#[from] anyhow::Error),
}

impl ClaudeUsageError {
    /// A single line that can be shown in the indicator's tooltip.
    pub fn user_facing_message(&self) -> String {
        match self {
            Self::NoSession => "Run Claude Code once to show usage here".to_string(),
            Self::SessionExpired | Self::Unauthorized => {
                "Claude Code session expired — run Claude Code to refresh it".to_string()
            }
            Self::BadResponse(status) => format!("Claude returned {status}"),
            Self::Unexpected(_) => "Couldn't reach Claude".to_string(),
        }
    }
}

// MARK: - Usage endpoint

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    five_hour: Option<UsageBucket>,
    #[serde(default)]
    seven_day: Option<UsageBucket>,
    #[serde(default)]
    extra_usage: Option<ExtraUsageResponse>,
}

#[derive(Debug, Deserialize)]
struct UsageBucket {
    /// Already a percentage, 0–100 — not a ratio.
    #[serde(default)]
    utilization: f32,
    #[serde(default)]
    resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct ExtraUsageResponse {
    #[serde(default)]
    is_enabled: bool,
    #[serde(default)]
    monthly_limit: Option<f64>,
    #[serde(default)]
    used_credits: Option<f64>,
}

impl From<UsageResponse> for ClaudeUsageSnapshot {
    fn from(response: UsageResponse) -> Self {
        let extra_usage = response.extra_usage.and_then(|extra| {
            let (used_credits, monthly_limit) = (extra.used_credits?, extra.monthly_limit?);
            (extra.is_enabled && monthly_limit > 0.).then_some(ClaudeExtraUsage {
                used_credits,
                monthly_limit,
            })
        });
        Self {
            session_percent: response
                .five_hour
                .as_ref()
                .map(|bucket| bucket.utilization)
                .unwrap_or_default(),
            session_resets_at: response.five_hour.and_then(|bucket| bucket.resets_at),
            weekly_percent: response.seven_day.as_ref().map(|bucket| bucket.utilization),
            weekly_resets_at: response.seven_day.and_then(|bucket| bucket.resets_at),
            extra_usage,
        }
    }
}

/// Asks Claude for the current usage of the session `token` belongs to.
///
/// Blocking, and bounded only by the 15-second timeout: call it from a
/// background thread, never from the thread that draws.
pub fn fetch_usage(token: &str) -> Result<ClaudeUsageSnapshot, ClaudeUsageError> {
    let agent = Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(REQUEST_TIMEOUT))
        // A 401 is an answer about the session, not a transport failure, so the
        // status checks below have to see it rather than an opaque error.
        .http_status_as_error(false)
        .build()
        .new_agent();

    let mut response = agent
        .get(USAGE_URL)
        .header("authorization", format!("Bearer {token}"))
        .header("anthropic-beta", OAUTH_BETA_HEADER)
        .call()
        .context("Failed to request Claude usage")?;

    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ClaudeUsageError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ClaudeUsageError::BadResponse(status));
    }
    let usage: UsageResponse = response
        .body_mut()
        .read_json()
        .context("Failed to parse the Claude usage response")?;
    Ok(usage.into())
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
