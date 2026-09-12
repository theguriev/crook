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
pub mod worktree;

#[cfg(test)]
mod tests;

use std::path::Path;

pub use branch::{Head, RepoLayout, branches as branches_in, discover, read_head};
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
    /// Whether this is a linked worktree rather than the checkout the
    /// repository was cloned into.
    ///
    /// Free, in the sense that matters: [`discover`] already knows both git
    /// directories by the time it answers, and telling them apart is a
    /// comparison rather than another walk. `false` outside a repository, for
    /// the same reason `branch` is `None` there.
    pub worktree: bool,
}

/// The branch `dir` is on, if it is in a repository.
///
/// Cheap enough to call on every directory change: it walks up for `.git` and
/// reads one small file, and spawns nothing.
pub fn current_branch(dir: &Path) -> Option<Head> {
    discover(dir).and_then(|layout| read_head(&layout.git_dir))
}

/// Every branch the repository `dir` sits in has, in name order.
///
/// Cheap in the same way [`current_branch`] is — it reads files and spawns
/// nothing — so it is safe wherever a directory is, though it reads a
/// directory tree rather than one file and belongs on the background pool when
/// the caller is already on one.
///
/// An empty list is "no branches to offer", which is what a directory outside
/// a repository and a repository with no refs both come back as. Nothing here
/// distinguishes them, because nothing that asks can do anything with the
/// difference.
pub fn branches(dir: &Path) -> Vec<String> {
    discover(dir).as_ref().map(branches_in).unwrap_or_default()
}

/// Everything about `dir` that can be had without running git.
///
/// The half of [`gather`] that costs a walk and one small file, for the times
/// a row is drawn before the diff has come home — and the only place that
/// answers whether a directory is a linked worktree, because that is a
/// property of the layout rather than of anything git has to be asked.
pub fn facts_without_diff(dir: &Path) -> GitFacts {
    let Some(layout) = discover(dir) else {
        return GitFacts::default();
    };

    GitFacts {
        branch: read_head(&layout.git_dir),
        diff: None,
        worktree: layout.is_linked_worktree(),
    }
}

/// Everything about the repository `dir` sits in.
///
/// **Blocking**: this runs `git` for the diff stats. Call it from the
/// background executor, the way [`crate::git_model`] runs its cycle — never
/// from a view. Use [`current_branch`] when only the cheap half is wanted.
///
/// A bare repository has no working tree to diff, so its `diff` is `None`.
pub fn gather(dir: &Path) -> GitFacts {
    let Some(layout) = discover(dir) else {
        return GitFacts::default();
    };

    GitFacts {
        branch: read_head(&layout.git_dir),
        diff: layout.work_tree.as_deref().and_then(diff_stats_blocking),
        worktree: layout.is_linked_worktree(),
    }
}

/// A working directory as a row prints it: `$HOME` replaced by `~`.
///
/// Warp's `warp_util::path::user_friendly_path`, and it does exactly one thing
/// — no shortening of middle components, no basename-only mode.
/// `~/work/crook/app/src` stays `~/work/crook/app/src`, and it is the row —
/// a `Text` cut from its start, by measure — that decides what survives a
/// narrow one: the tail, which is the part that says where you are.
///
/// The prefix only counts when the next character is a separator, so a sibling
/// directory named `/Users/euge` beside `/Users/eugen` is not silently
/// reprinted as `~n`.
pub fn user_friendly_path(path: &Path, home: Option<&Path>) -> String {
    let display = path.to_string_lossy().into_owned();
    let Some(home) = home.map(|home| home.to_string_lossy().into_owned()) else {
        return display;
    };
    if home.is_empty() {
        return display;
    }

    let Some(rest) = display.strip_prefix(&home) else {
        return display;
    };
    match rest.chars().next() {
        None => "~".to_owned(),
        Some(separator) if std::path::is_separator(separator) => format!("~{rest}"),
        Some(_) => display,
    }
}

/// The short name a directory can be called, for a row that has nothing better.
///
/// The last component, which is what a person calls a checkout: `crook`, not
/// `~/work/crook`. The home directory is `~` rather than the account name,
/// because "eugen" is not what anybody calls their home — and since a macOS app
/// opened from the Dock now starts there, that is the first thing a new tab
/// would otherwise be named.
///
/// `None` for a root directory, which has no last component and is not a place
/// anybody is working in anyway.
pub fn directory_label(path: &Path, home: Option<&Path>) -> Option<String> {
    if home.is_some_and(|home| home == path) {
        return Some("~".to_owned());
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}
