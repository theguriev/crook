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
//! | [`diff::since_base_blocking`] | one or two more, off the base | the same again |
//!
//! They also fail apart, which is the point of splitting them: a machine with
//! no `git` binary still shows every tab's branch, because that branch came
//! out of a file.
//!
//! [`gather`] is all of them together, for a caller that is already on a
//! background thread and wants everything a row can hold. The count since the
//! base needs a base, which is up to three subprocesses of its own and almost
//! never changes, so a gather is handed [`Bases`] — what the last gathers
//! found — rather than asking again every time it runs.

pub mod branch;
pub mod diff;
pub mod merged;
mod run;
pub mod worktree;

#[cfg(test)]
mod tests;

/// What the tests make their scratch directories from: a path spelled the
/// way git will print it back.
///
/// Three test files compare a path they made with a path git listed, and
/// the temporary directory they make it under is not spelled the way git
/// spells it on two of the three platforms: macOS reaches it through
/// `/var -> /private/var`, and a GitHub Windows runner names it by its 8.3
/// short form (`RUNNER~1`), while git prints the resolved, long-named form
/// of both. `canonicalize` resolves both — and on Windows answers in the
/// `\\?\` spelling, which git cannot be handed (`worktree add` refuses to
/// create leading directories under it), so that prefix comes off again.
#[cfg(test)]
pub(crate) fn as_git_prints_it(path: std::path::PathBuf) -> std::path::PathBuf {
    let Ok(canonical) = std::fs::canonicalize(&path) else {
        return path;
    };
    if cfg!(windows) {
        let text = canonical.to_string_lossy();
        if let Some(plain) = text.strip_prefix(r"\\?\")
            && !plain.starts_with("UNC")
        {
            return std::path::PathBuf::from(plain);
        }
    }
    canonical
}

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use branch::{Head, RepoLayout, branches as branches_in, discover, read_head};
pub use diff::{DiffStats, SinceBase, diff_stats_blocking, git_is_missing, since_base_blocking};

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
    /// What the branch has committed since it left its base, and every line
    /// since then.
    ///
    /// `None` wherever `diff` already says everything there is: on the base
    /// itself, on a detached `HEAD` — which is on no branch to have left
    /// anything — on a branch with nothing committed since the base, and in a
    /// repository with no base to count from or no commit yet. Also `None`
    /// wherever git could not answer, for the reasons `diff` can be.
    pub since_base: Option<SinceBase>,
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

/// The branch the work in a directory is on, and the repository it is a
/// branch of. See [`branch_at_work`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchAtWork {
    /// The repository's [`RepoLayout::common_dir`]: the one every linked
    /// worktree of it shares, and a submodule, which is a repository of its
    /// own, does not. A branch name alone is not a branch — `feat` is one in
    /// a great many repositories.
    pub repository: PathBuf,
    /// What `HEAD` says, with a rebase's detached one read as the branch
    /// being rebased.
    pub head: Head,
}

impl BranchAtWork {
    /// Whether `other` is in the same repository, however the path to it was
    /// spelled.
    ///
    /// The paths as found are the whole answer unless they differ. A pane's
    /// directory is the shell's `$PWD`, links and all, and [`discover`] keeps
    /// the spelling it reached a repository by — so a link to a checkout and
    /// the checkout itself find one repository under two names. Only then
    /// are both resolved — once, for a pane that really has moved, since its
    /// pull request goes.
    pub fn is_same_repository(&self, other: &Self) -> bool {
        self.repository == other.repository
            || matches!(
                (
                    std::fs::canonicalize(&self.repository),
                    std::fs::canonicalize(&other.repository),
                ),
                (Ok(mine), Ok(theirs)) if mine == theirs
            )
    }
}

/// The branch the work in `dir` is on: [`current_branch`], except that a
/// `HEAD` a rebase detached is the branch being rebased, as `git status`
/// says it is — see [`branch::rebasing`] — and with the repository it is in.
///
/// For what goes with a branch rather than with a commit — a pull request —
/// where a rebase to bring a branch up to date is not leaving it. A row's
/// own label keeps [`current_branch`]'s reading, which shows the commit the
/// rebase has reached.
pub fn branch_at_work(dir: &Path) -> Option<BranchAtWork> {
    let layout = discover(dir)?;
    let head = match read_head(&layout.git_dir)? {
        head @ Head::Detached { .. } => {
            branch::rebasing(&layout.git_dir).map_or(head, Head::Branch)
        }
        branch => branch,
    };
    Some(BranchAtWork {
        repository: layout.common_dir,
        head,
    })
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
        since_base: None,
        worktree: layout.is_linked_worktree(),
    }
}

/// Everything about the repository `dir` sits in.
///
/// **Blocking**: this runs `git` for the diff stats, and for the count since
/// the base on a branch that is not the base. Call it from the background
/// executor, the way [`crate::git_model`] runs its cycle — never from a view.
/// Use [`current_branch`] when only the cheap half is wanted.
///
/// A bare repository has no working tree to diff, so its `diff` is `None`,
/// and so is its `since_base`.
///
/// `bases` is asked for the repository's base only when there is a branch to
/// count and a working tree to count it in, which is also the only time the
/// answer is used. A branch whose name is the base's own — `main` against
/// `refs/remotes/origin/main` as much as against `refs/heads/main` — is the
/// base itself, and is left with the plain count alone.
pub fn gather(dir: &Path, bases: &mut Bases) -> GitFacts {
    let Some(layout) = discover(dir) else {
        return GitFacts::default();
    };

    let branch = read_head(&layout.git_dir);
    let work_tree = layout.work_tree.as_deref();
    let diff = work_tree.and_then(diff_stats_blocking);
    let since_base = match (&branch, work_tree) {
        (Some(Head::Branch(name)), Some(work_tree)) => bases
            .base(&layout.common_dir, work_tree, name)
            .filter(|base| diff::base_name(base) != name.as_str())
            .and_then(|base| since_base_blocking(work_tree, &base)),
        _ => None,
    };

    GitFacts {
        branch,
        diff,
        since_base,
        worktree: layout.is_linked_worktree(),
    }
}

/// Each repository's base, as the gathers that have run so far found it.
///
/// A base is up to three subprocesses to find — see
/// [`merged::base_of`] — and what it names almost never changes: a
/// repository's default branch is decided when it is made. Asked on every
/// gather it would more than double what a refresh costs, for an answer that
/// was the same last time. So it is looked up once per repository — by its
/// common git directory, which every worktree of it shares — and kept.
///
/// Two answers are not kept for ever:
///
/// - **No base** is kept only for the branches that asked. A repository made
///   a minute ago has no `main` until its first commit, and one whose only
///   answer was "none" would otherwise count nothing on any branch for as long
///   as it stayed on screen. A branch that has not asked before asks again, so
///   the next checkout of a new branch — which is when a base starts to
///   matter — finds the `main` that exists by then.
/// - **Every answer** is forgotten once a whole cycle goes by without a
///   gather asking for it — see [`Self::forget_unasked`] — which is what keeps
///   this from growing for the life of the process, and gives a repository that
///   comes back on screen a fresh look.
pub struct Bases {
    /// What each repository's lookup found, by common git directory.
    known: HashMap<PathBuf, Known>,
    /// The repositories asked about since the last [`Self::forget_unasked`].
    asked: HashSet<PathBuf>,
    /// How a base is found. [`merged::base_of`], except in a test that counts
    /// the lookups.
    look_up: fn(&Path) -> Option<String>,
}

/// What one repository's lookup found.
enum Known {
    /// A base, kept until the repository leaves the screen.
    Base(String),
    /// None, as found for these branches; any other branch asks again.
    Nothing(HashSet<String>),
}

impl Default for Bases {
    fn default() -> Self {
        Self::looking_up_with(merged::base_of)
    }
}

impl Bases {
    /// Bases found by `look_up` rather than by [`merged::base_of`].
    ///
    /// The seam a test counts lookups through; everything else takes the
    /// default.
    pub fn looking_up_with(look_up: fn(&Path) -> Option<String>) -> Self {
        Self {
            known: HashMap::new(),
            asked: HashSet::new(),
            look_up,
        }
    }

    /// The base of the repository whose common git directory is
    /// `repository`, asked for by `branch`, looked up in `work_tree` only
    /// when nothing is known that answers it.
    fn base(&mut self, repository: &Path, work_tree: &Path, branch: &str) -> Option<String> {
        self.asked.insert(repository.to_owned());
        match self.known.get(repository) {
            Some(Known::Base(base)) => return Some(base.clone()),
            Some(Known::Nothing(asked_for)) if asked_for.contains(branch) => return None,
            _ => {}
        }

        let found = (self.look_up)(work_tree);
        let known = self
            .known
            .entry(repository.to_owned())
            .or_insert_with(|| Known::Nothing(HashSet::new()));
        match (&found, known) {
            (Some(base), known) => *known = Known::Base(base.clone()),
            (None, Known::Nothing(asked_for)) => {
                asked_for.insert(branch.to_owned());
            }
            // Answered above, and never looked up again while it is kept.
            (None, Known::Base(_)) => {}
        }
        found
    }

    /// Forgets every repository no gather has asked about since the last call.
    ///
    /// For the end of a cycle that gathered with the counts on. A cycle that
    /// ran without them asked nothing, and calling this after one would throw
    /// away every base for the sake of a toggle.
    pub fn forget_unasked(&mut self) {
        let asked = std::mem::take(&mut self.asked);
        self.known
            .retain(|repository, _| asked.contains(repository));
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
