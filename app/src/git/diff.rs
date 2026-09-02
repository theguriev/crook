//! How many lines the working tree is away from `HEAD`.
//!
//! This is the half of the tab row's git data that no file holds. The index is
//! a binary format whose contents only mean something next to the object
//! store, so counting changed lines means asking git, which means a
//! subprocess — the one thing [`super::branch`] avoids entirely.
//!
//! Two consequences shape everything here. A subprocess is 5-50ms, so
//! [`diff_stats_blocking`] is blocking by name and belongs on the background
//! executor; running it from a view stalls the frame. And a subprocess can
//! fail in ways a tab row has no business rendering — git not installed, a
//! directory that is not a repository, a repository with no commits yet, a
//! repository mid-rebase — so every one of them is a `None` here, and the
//! caller keeps whatever number it had.

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::process::command;

/// Set the first time git turns out not to be on `PATH`.
///
/// Without it a machine with no git spawns a doomed process on every refresh
/// tick for the life of the session. The answer cannot change while the
/// process runs in any way worth chasing, so it is latched rather than
/// retried.
static GIT_MISSING: AtomicBool = AtomicBool::new(false);

/// Lines added and removed in the working tree, against `HEAD`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffStats {
    /// Files with at least one changed line.
    pub files_changed: u32,
    /// Lines added.
    pub lines_added: u32,
    /// Lines removed.
    pub lines_removed: u32,
}

impl DiffStats {
    /// Parses one line of `git diff --shortstat` output.
    ///
    /// ```text
    ///  1 file changed, 2 insertions(+), 17 deletions(-)
    ///  3 files changed, 45 insertions(+)
    /// ```
    ///
    /// Empty input is a clean tree, not a failed read — git prints nothing at
    /// all when there is nothing to report — so this is total: the zero value
    /// *is* the answer. "Never read one" is carried by the `Option` around
    /// these stats, not by a second empty-ish value inside them.
    ///
    /// Each clause is found by the word after its number, matched on the
    /// prefix so that git's singular and plural need no special case, and any
    /// clause may be absent.
    pub fn parse_shortstat(raw: &str) -> Self {
        let words: Vec<_> = raw.split_whitespace().collect();
        let mut stats = Self::default();

        for (index, word) in words.iter().enumerate() {
            let Ok(count) = word.parse::<u32>() else {
                continue;
            };
            match words.get(index + 1) {
                Some(next) if next.starts_with("file") => stats.files_changed = count,
                Some(next) if next.starts_with("insertion") => stats.lines_added = count,
                Some(next) if next.starts_with("deletion") => stats.lines_removed = count,
                _ => {}
            }
        }

        stats
    }

    /// Whether the working tree matches `HEAD`.
    ///
    /// A row draws no badge at all in that case — not a `0`. Warp filters the
    /// same way, one layer up.
    pub fn is_empty(self) -> bool {
        self.files_changed == 0 && self.lines_added == 0 && self.lines_removed == 0
    }

    /// The badge's text, as separately coloured tokens: `["+12", "-3"]`, with
    /// either side omitted when it is zero.
    ///
    /// Yields `["0"]` for a clean tree, which a tab row never asks for because
    /// it checks [`Self::is_empty`] first and draws nothing.
    pub fn tokens(self) -> Vec<String> {
        let mut tokens = Vec::new();
        if self.lines_added > 0 {
            tokens.push(format!("+{}", self.lines_added));
        }
        if self.lines_removed > 0 {
            tokens.push(format!("-{}", self.lines_removed));
        }
        if tokens.is_empty() {
            tokens.push("0".to_owned());
        }
        tokens
    }
}

/// Whether git has already been found to be missing from this machine.
///
/// A scheduler can stop asking for stats entirely once this is true. It does
/// not have to: [`diff_stats_blocking`] checks the same latch and returns
/// immediately.
pub fn git_is_missing() -> bool {
    GIT_MISSING.load(Ordering::Relaxed)
}

/// Counts the working tree's line changes against `HEAD`.
///
/// **Blocking, and expensive**: it spawns `git` and waits for it, 5-50ms in a
/// warm repository and worse in a cold one. Call it from the background
/// executor, never from a view or a foreground task. It deliberately does not
/// spawn its own thread — who runs it is the caller's decision, and Crook
/// already has a pool for exactly this.
///
/// `None` covers every way there can be no number, none of which is an error a
/// row should render:
///
/// - git is not installed — latched, so nothing spawns again this session;
/// - `work_tree` is not a repository;
/// - the repository has no commits yet, so `HEAD` resolves to nothing;
/// - git failed for any other reason, which is usually transient — a rebase in
///   progress, an index being rewritten — and the caller should keep the last
///   number it had and try again on its normal cadence.
///
/// **Untracked files are not counted.** `--shortstat HEAD` compares tracked
/// content only, so a directory of brand-new files reads as clean. Warp's
/// watcher-backed path counts them (it runs `status --untracked-files=all` and
/// line-counts each new file); Warp's own cheap shell path, which this
/// matches, does not. The two therefore legitimately disagree for the same
/// repository.
pub fn diff_stats_blocking(work_tree: &Path) -> Option<DiffStats> {
    if git_is_missing() {
        return None;
    }

    let mut git = command("git");
    git
        // A background read must not take `.git/index.lock`, or it races the
        // user's own commit, and must not rewrite the index as a side effect
        // of refreshing it. Both flags are load-bearing; the environment
        // variable carries the first one into anything git itself spawns.
        .arg("--no-optional-locks")
        .args(["-c", "diff.autoRefreshIndex=false"])
        .args(["diff", "--shortstat", "HEAD"])
        .current_dir(work_tree)
        .env("GIT_OPTIONAL_LOCKS", "0")
        // A git that decides to prompt — for a credential, through a pager —
        // must fail rather than park a worker thread until the process exits.
        // Dropping the task that awaits this cannot cancel a blocked syscall,
        // so there is no rescue if it does hang.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = match git.output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !GIT_MISSING.swap(true, Ordering::Relaxed) {
                log::warn!("git is not on PATH; diff stats are off for this session");
            }
            return None;
        }
        Err(error) => {
            log::debug!("could not run git in {}: {error}", work_tree.display());
            return None;
        }
    };

    // Never `from_utf8`: a path in a shortstat line can be any bytes the
    // filesystem accepted, and a rename clause prints one.
    let stdout = String::from_utf8_lossy(&output.stdout);

    // git's diff family reports 1 for "there were differences" under some
    // flags, so an exit of 1 that produced output is a successful read.
    let read_succeeded =
        output.status.success() || (output.status.code() == Some(1) && !stdout.trim().is_empty());

    if !read_succeeded {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::debug!(
            "no diff stats for {}: {}",
            work_tree.display(),
            stderr.lines().next().unwrap_or("git reported no reason")
        );
        return None;
    }

    Some(DiffStats::parse_shortstat(&stdout))
}
