//! How many lines the working tree is away from `HEAD`, and how much a branch
//! has done since it left its base.
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
//!
//! The first count alone goes blank at exactly the wrong moment: `--shortstat
//! HEAD` compares the working tree with the last commit, so the instant an
//! agent commits its work the row has nothing left to say about it.
//! [`SinceBase`] is the count that survives a commit — how many commits the
//! branch has that its base does not, and every line since it left the base,
//! committed or not.

use std::ffi::OsStr;
use std::path::Path;

use super::run::{Intent, run};

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
/// not have to: every call here goes through `run.rs`, which checks the same
/// latch and returns immediately.
pub fn git_is_missing() -> bool {
    super::run::git_is_missing()
}

/// Counts the working tree's line changes against `HEAD`.
///
/// **Blocking, and expensive**: it spawns `git` and waits for it, 5-50ms in a
/// warm repository and worse in a cold one. Call it from the background
/// executor, never from a view or a foreground task. It deliberately does not
/// spawn its own thread — who runs it is the caller's decision, and Crook
/// already has a pool for exactly this.
///
/// Through the git module's shared runner, `run.rs`, and so under its read
/// deadline, with its locks rule and its no-prompt rule. This count used to
/// spawn git itself with no deadline at all, and it runs on the tab strip's
/// gather chain: a git that never came back — an `fsmonitor` hook that hung,
/// a network filesystem that stopped answering — held a pool worker for the
/// life of the window and stopped every row's numbers from updating.
///
/// `None` covers every way there can be no number, none of which is an error a
/// row should render:
///
/// - git is not installed — latched, so nothing spawns again this session;
/// - `work_tree` is not a repository;
/// - the repository has no commits yet, so `HEAD` resolves to nothing;
/// - git did not answer within the deadline, and was killed;
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
    let args = [
        OsStr::new("diff"),
        OsStr::new("--shortstat"),
        OsStr::new("HEAD"),
    ];
    let finished = run(work_tree, &args, Intent::Read).ok()?;

    // Never `from_utf8`: a path in a shortstat line can be any bytes the
    // filesystem accepted, and a rename clause prints one.
    let stdout = String::from_utf8_lossy(&finished.stdout);

    // git's diff family reports 1 for "there were differences" under some
    // flags, so an exit of 1 that produced output is a successful read.
    let read_succeeded =
        finished.success || (finished.code == Some(1) && !stdout.trim().is_empty());

    if !read_succeeded {
        log::debug!(
            "no diff stats for {}: {}",
            work_tree.display(),
            finished
                .stderr
                .lines()
                .next()
                .unwrap_or("git reported no reason")
        );
        return None;
    }

    Some(DiffStats::parse_shortstat(&stdout))
}

/// What a branch has done since it left the branch its work lands on.
///
/// Only ever made for a branch with commits of its own. Zero commits ahead
/// means `HEAD` is on the base, where the fork *is* `HEAD` and every line
/// since it is already what [`DiffStats`] counts — so a zero would be a second
/// copy of that number wearing a different name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SinceBase {
    /// The base, by the name a person calls it: `main`, whether the ref it
    /// was read from is `refs/heads/main` or `refs/remotes/origin/main`. See
    /// [`base_name`].
    pub base: String,
    /// Commits `HEAD` has that the base does not. Never zero.
    pub commits: u32,
    /// Lines changed between where the branch left the base and the working
    /// tree: the committed work and the uncommitted edits on top of it, as one
    /// number.
    pub diff: DiffStats,
}

/// The name a person calls a base by.
///
/// [`super::merged::base_of`] answers in full refs, and deliberately with the
/// remote-tracking one when there is one — `refs/remotes/origin/main`, because
/// that is where a squash lands. Nobody says "since origin/main" about their
/// branch, and a row has no room for it, so the remote goes with the prefix:
/// both spellings are `main`. Anything outside those two namespaces is shown
/// as it is.
pub fn base_name(base: &str) -> &str {
    if let Some(name) = base.strip_prefix("refs/heads/") {
        return name;
    }
    if let Some(remote_and_name) = base.strip_prefix("refs/remotes/") {
        return remote_and_name
            .split_once('/')
            .map_or(remote_and_name, |(_, name)| name);
    }
    base
}

/// Counts what `HEAD` has done since it left `base`.
///
/// **Blocking**: one subprocess, and a second when there are commits to
/// count lines for. Background executor only. Both go through the git
/// module's shared runner, `run.rs`, and its deadline, as
/// [`diff_stats_blocking`] does.
///
/// `base` is a full ref, the form [`super::merged::base_of`] answers in, so
/// that neither a tag nor a file with the same name can stand in for it.
///
/// The two questions are the two a person asks of an agent's branch:
///
/// - `rev-list --count <base>..HEAD` — commits on `HEAD` the base cannot
///   reach. The same set as counting from the merge-base, without asking git
///   where that is first — and a branch that has merged the base back in
///   since is not credited with the base's commits that came in with it.
/// - `diff --shortstat --merge-base <base>` — the working tree against where
///   `HEAD` left the base, which is everything the branch has done, committed
///   or not, and nothing the base has done since. `--merge-base` is what makes
///   that one subprocess rather than two.
///
/// `None` is "nothing to add to the plain count": no commits ahead, which is
/// the base or a branch that has not started yet; a `HEAD` with no commit
/// behind it; a base this repository has no history in common with, or more
/// than one merge-base with; and every failure git can have, none of which a
/// row should render.
pub fn since_base_blocking(work_tree: &Path, base: &str) -> Option<SinceBase> {
    let range = format!("{base}..HEAD");
    let commits: u32 = read(work_tree, &["rev-list", "--count", &range, "--"])?
        .trim()
        .parse()
        .ok()?;
    if commits == 0 {
        return None;
    }
    let stat = read(
        work_tree,
        &["diff", "--shortstat", "--merge-base", base, "--"],
    )?;
    Some(SinceBase {
        base: base_name(base).to_owned(),
        commits,
        diff: DiffStats::parse_shortstat(&stat),
    })
}

/// What git printed for `args`, when it succeeded.
fn read(work_tree: &Path, args: &[&str]) -> Option<String> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let finished = run(work_tree, &args, Intent::Read).ok()?;
    if !finished.success {
        log::debug!(
            "no count since the base for {}: {}",
            work_tree.display(),
            finished
                .stderr
                .lines()
                .next()
                .unwrap_or("git reported no reason")
        );
        return None;
    }
    Some(String::from_utf8_lossy(&finished.stdout).into_owned())
}
