//! The git facts a tab row can show, and how they read.
//!
//! A tab in Crook is an agent working somewhere, and where it is working is
//! usually a repository — so "which branch" and "how much has changed" are the
//! two things a row can say that a directory path cannot. Warp reads both
//! through a shell chip pipeline backed by a filesystem-watcher model, and
//! Crook has no shell to run a chip in; it reads git directly, which is also
//! what Warp's own code has a `TODO` asking for.
//!
//! The two halves cost wildly different amounts and are kept apart for that
//! reason alone:
//!
//! | half | how | cost |
//! |---|---|---|
//! | [`current_branch`] | walk up for `.git`, read `HEAD` | a handful of syscalls |
//! | [`diff::diff_stats_blocking`] | one `git` subprocess | 5-50ms, background only |
//!
//! They also fail apart, which is the point of splitting them: a machine with
//! no `git` binary still shows every tab's branch, because that branch came
//! out of a file.
//!
//! [`gather`] is both together, for a caller that is already on a background
//! thread and wants everything a row can hold.

pub mod branch;
pub mod diff;

#[cfg(test)]
mod tests;

use std::path::Path;

pub use branch::{Head, RepoLayout, discover, read_head};
pub use diff::{DiffStats, diff_stats_blocking, git_is_missing};

/// Everything a tab row knows about the repository its session sits in.
///
/// Both fields are `None` outside a repository, and `diff` is additionally
/// `None` whenever git could not answer — see [`diff_stats_blocking`] for
/// the four ways that happens. `None` therefore means "no number to show",
/// never "zero changes"; zero changes is `Some` of an
/// [empty][DiffStats::is_empty] `DiffStats`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitFacts {
    /// The branch, or the short sha of a detached head.
    pub branch: Option<Head>,
    /// Working-tree line changes against `HEAD`.
    pub diff: Option<DiffStats>,
}

/// The branch `dir` is on, if it is in a repository.
///
/// Cheap enough to call on every directory change: it walks up for `.git` and
/// reads one small file, and spawns nothing.
pub fn current_branch(dir: &Path) -> Option<Head> {
    discover(dir).and_then(|layout| read_head(&layout.git_dir))
}

/// Everything about the repository `dir` sits in.
///
/// **Blocking**: this runs `git` for the diff stats. Call it from the
/// background executor, the way `usage_model` runs its poll — never from a
/// view. Use [`current_branch`] when only the cheap half is wanted.
///
/// A bare repository has no working tree to diff, so its `diff` is `None`.
pub fn gather(dir: &Path) -> GitFacts {
    let Some(layout) = discover(dir) else {
        return GitFacts::default();
    };

    GitFacts {
        branch: read_head(&layout.git_dir),
        diff: layout.work_tree.as_deref().and_then(diff_stats_blocking),
    }
}

/// The text that goes where a row shows the branch, and whether to draw the
/// branch icon beside it.
///
/// Warp's `branch_label_display`, and its one surprise is worth keeping: a
/// directory outside a repository is *not* rendered as "no branch". The row
/// falls back to `fallback` — the working directory — with the icon
/// suppressed, so a session outside a repository quietly shows a path. That
/// also means the same string can appear twice in one row when the row's other
/// line is the working directory too, which is what Warp does.
pub fn branch_label(branch: Option<&str>, fallback: &str) -> (String, bool) {
    match branch {
        Some(branch) if !branch.trim().is_empty() => (branch.to_owned(), true),
        _ => (fallback.to_owned(), false),
    }
}

/// Shortens `text` to `max_chars`, marking the cut with a trailing ellipsis.
///
/// Warp does not do this. Its branch label is `Shrinkable` plus
/// `ClipConfig::ellipsis()`, so it truncates to the pixels actually left over
/// after the badges have taken theirs, and there is no character limit
/// anywhere. Crook's `Text` cannot clip yet, and a ninety-character branch name
/// would push the rest of the strip off the window, so this is the stand-in —
/// and it should be deleted the day the renderer grows a clip config.
///
/// The ellipsis is one character and counts against the budget, so the result
/// is never longer than `max_chars`.
pub fn truncate_end(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }

    let mut truncated: String = text.chars().take(max_chars - 1).collect();
    truncated.push('\u{2026}');
    truncated
}
