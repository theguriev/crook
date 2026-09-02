//! Reads the OAuth session that Claude Code already stores locally.
//!
//! Claude Code keeps its credentials in two places: a JSON file at
//! `~/.claude/.credentials.json` (used on Linux and Windows, and as a fallback
//! on macOS) and, on macOS, a generic-password Keychain item under the
//! `Claude Code-credentials` service. The Keychain copy is the one Claude Code
//! refreshes, so the file can lag behind by days; we read both and keep
//! whichever token lives longest.
//!
//! Nothing here re-authenticates: if no usable session exists the caller shows a
//! hint to run Claude Code once, which refreshes the token in place.

use anyhow::Context as _;
use chrono::{DateTime, TimeZone as _, Utc};
use serde::Deserialize;

use crate::usage::ClaudeUsageError;

/// The Keychain service name Claude Code stores its credentials under.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// `security`'s exit status for "no such item", which is the ordinary state of
/// a machine that has never run Claude Code.
#[cfg(target_os = "macos")]
const KEYCHAIN_ITEM_NOT_FOUND: i32 = 44;

/// Path of the credentials file, relative to the home directory.
const CREDENTIALS_FILE: &str = ".claude/.credentials.json";

/// Refuse to reuse a token that is about to expire mid-request.
const EXPIRY_SKEW_SECONDS: i64 = 60;

/// An access token plus the moment it stops being usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaudeAccessToken {
    pub(crate) token: String,
    pub(crate) expires_at: Option<DateTime<Utc>>,
}

impl ClaudeAccessToken {
    fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    /// Whether this token can still be reused instead of re-read from storage.
    pub(crate) fn is_usable(&self, now: DateTime<Utc>) -> bool {
        !self.is_expired(now + chrono::TimeDelta::seconds(EXPIRY_SKEW_SECONDS))
    }
}

/// The subset of Claude Code's credential blob that we need.
#[derive(Debug, Deserialize)]
struct StoredCredentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: StoredOauth,
}

#[derive(Debug, Deserialize)]
struct StoredOauth {
    #[serde(rename = "accessToken")]
    access_token: String,
    /// Milliseconds since the Unix epoch.
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

/// Returns the longest-lived access token from any local Claude Code session.
///
/// Blocking file and Keychain reads: call this from a background thread.
pub(crate) fn load_access_token() -> Result<ClaudeAccessToken, ClaudeUsageError> {
    let now = Utc::now();
    let mut candidates = Vec::new();

    match read_credentials_file() {
        Ok(Some(token)) => candidates.push(token),
        Ok(None) => {}
        // A malformed or unreadable file is worth a local breadcrumb, but the
        // Keychain may still hold a usable session, so keep going.
        Err(err) => log::warn!("Failed to read Claude Code credentials file: {err:#}"),
    }

    // Only pay for the Keychain (which can prompt for access) when the file is
    // missing or has already expired.
    let file_token_usable = candidates.iter().any(|token| !token.is_expired(now));
    if !file_token_usable {
        match read_keychain() {
            Ok(Some(token)) => candidates.push(token),
            Ok(None) => {}
            Err(err) => log::warn!("Failed to read Claude Code credentials keychain item: {err:#}"),
        }
    }

    // Prefer the token that stays valid longest. `None` sorts before any date,
    // so a token that states an expiry wins over one that doesn't.
    candidates.sort_by_key(|token| token.expires_at);
    let token = candidates.pop().ok_or(ClaudeUsageError::NoSession)?;
    if token.is_expired(now) {
        return Err(ClaudeUsageError::SessionExpired);
    }
    Ok(token)
}

fn read_credentials_file() -> anyhow::Result<Option<ClaudeAccessToken>> {
    let Some(path) = dirs::home_dir().map(|home| home.join(CREDENTIALS_FILE)) else {
        return Ok(None);
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(anyhow::Error::new(err).context("Failed to read credentials file"));
        }
    };
    parse_credentials(&contents).map(Some)
}

/// Reads the Keychain item via the `security` CLI.
///
/// Using the CLI keeps this free of a platform-specific Keychain dependency, and
/// matches how the item is scoped: a generic password looked up by service name
/// alone, which the account-keyed APIs of the keyring crates cannot express.
#[cfg(target_os = "macos")]
// The workspace lint points at `crook::process::command`, which exists to set
// CREATE_NO_WINDOW on Windows. This crate cannot depend on the application
// crate, and this call is `cfg`-gated to macOS, where the flag has no meaning
// and there is no console window to hide.
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "macOS-only, so there is no console window to hide"
)]
fn read_keychain() -> anyhow::Result<Option<ClaudeAccessToken>> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .context("Failed to run the security CLI")?;

    if !output.status.success() {
        // Every failure reads as "no session" to the caller, so a denied access
        // dialog would otherwise be indistinguishable from never having run
        // Claude Code. Leave a breadcrumb for that case.
        if output.status.code() != Some(KEYCHAIN_ITEM_NOT_FOUND) {
            log::warn!(
                "The security CLI refused the Claude Code keychain item: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        return Ok(None);
    }

    let contents = String::from_utf8(output.stdout)
        .context("Keychain item is not valid UTF-8")?
        .trim()
        .to_string();
    if contents.is_empty() {
        return Ok(None);
    }
    parse_credentials(&contents).map(Some)
}

/// Claude Code writes the plaintext credentials file everywhere else, so there
/// is no second source to consult.
#[cfg(not(target_os = "macos"))]
fn read_keychain() -> anyhow::Result<Option<ClaudeAccessToken>> {
    Ok(None)
}

fn parse_credentials(contents: &str) -> anyhow::Result<ClaudeAccessToken> {
    let credentials: StoredCredentials =
        serde_json::from_str(contents).context("Failed to parse Claude Code credentials")?;
    let oauth = credentials.claude_ai_oauth;
    anyhow::ensure!(
        !oauth.access_token.is_empty(),
        "Claude Code credentials contain an empty access token"
    );
    Ok(ClaudeAccessToken {
        token: oauth.access_token,
        expires_at: oauth
            .expires_at
            .and_then(|millis| Utc.timestamp_millis_opt(millis).single()),
    })
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
