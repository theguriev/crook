//! Every checkout a repository has, the two commands that make and unmake
//! one, what a new one is given, and the lock that says one is in use.
//!
//! A tab in Crook is an agent working somewhere, and two agents working in the
//! same directory fight over the same files. A worktree is git's answer to
//! that: one repository, several checkouts, each on its own branch. Giving a
//! new session its own checkout is two commands; this module is those two, plus
//! the reading that makes them safe to offer — and [`copy_included`], which
//! brings a new checkout the ignored files git leaves behind and an agent's
//! first run needs, the `.env` and the local configuration, by the rules of
//! Claude Code's `.worktreeinclude` — and the lock that tells every other
//! tool an agent is working in one: see [`LOCK_PREFIX`] for how Crook tells
//! its own lock from anybody else's.
//!
//! Everything here is a subprocess, which puts it in [`super::diff`]'s bracket
//! rather than [`super::branch`]'s: blocking, background-only, and able to
//! fail. It differs from `diff` in two ways that shape the whole module.
//!
//! The first is that these are *actions*, not a badge. A failed read makes a
//! row show no number and nobody has to be told; a failed `add` has to tell a
//! person what to do instead — jump to the checkout that already has that
//! branch, pick another name, force the removal. So every failure here is a
//! variant of [`Error`] rather than a `None`, and the ones a UI can act on
//! carry the branch or the path git named.
//!
//! The second is that several of them write. `diff` documents that it has no
//! timeout and that a hung git parks a pool worker for the life of the
//! process; a write can hang for reasons a read cannot, because [`add`]
//! materialises a working tree and then runs the repository's own
//! `post-checkout` hook, which is arbitrary code somebody else wrote. So
//! everything here runs under a deadline and is killed at it — the runner in
//! [`super::run`], which says why the deadline bounds the whole call and not
//! only git.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use super::run::{Failure, Intent, run, run_fed};
pub(crate) use super::run::{READ_TIMEOUT, WRITE_TIMEOUT};

// MARK: - What a repository has

/// One checkout of a repository.
///
/// Every field is what `git worktree list --porcelain` said, and `path` in
/// particular is git's own absolute path, deliberately not canonicalised: `~`
/// is a symlink on plenty of machines, and a canonical path stops matching the
/// one the user sees in their tab and typed into their shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Where the files are. Absolute, always — git prints it that way.
    pub path: PathBuf,
    /// The short sha of the commit checked out, abbreviated to the seven
    /// characters [`super::Head::Detached`] shows. `None` only for a bare
    /// entry, which has no `HEAD` line at all.
    pub head: Option<String>,
    /// The branch, without its `refs/heads/` prefix. `None` when the checkout
    /// is detached, and `None` for a bare entry.
    pub branch: Option<String>,
    /// Whether this is the repository's main checkout — the one that cannot be
    /// removed, and the one every linked worktree points back at.
    ///
    /// Nothing in git's output marks it. It is the first record git prints, and
    /// that is the whole of the rule.
    pub is_main: bool,
    /// Whether the entry is a bare repository rather than a checkout.
    pub is_bare: bool,
    /// The lock's reason when the worktree is locked, and `Some("")` when it is
    /// locked without one. `None` is unlocked.
    ///
    /// A checkout Crook makes is locked with [`lock_reason`] — `crook: ` and
    /// the branch — from the moment it is made until no pane in the window
    /// that made it is working in it, or that window closes, which is what
    /// tells git and every other tool that an agent is in there.
    /// [`Self::is_locked_by_crook`] and [`Self::is_locked_by_another`] tell
    /// that lock from anybody else's.
    pub locked: Option<String>,
    /// git's reason for thinking the entry could be pruned — most often that
    /// its directory is gone — with the same `Some("")` convention as
    /// [`Self::locked`].
    pub prunable: Option<String>,
}

/// What is loose in a worktree that removing it would destroy.
///
/// Three buckets, because git treats them as two and a person cares about all
/// three. Measured on git 2.55.0: `git worktree remove` refuses over modified
/// and untracked files and says which worktree it is refusing over; it deletes
/// ignored ones without a word, and a checkout holding nothing but a `build/`
/// removes cleanly on the first try.
///
/// So the split is not decoration. Modified and untracked are the refusal, and
/// what `force` is for. Ignored is the half nobody is warned about: a worktree
/// that has been built once holds a `target/` worth gigabytes and it goes
/// whether or not anyone forced anything, so saying it out loud before the
/// button is pressed is the only chance a person gets.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Local {
    /// Tracked files that differ from the index or from `HEAD`.
    pub modified: usize,
    /// Files git does not track and is not ignoring.
    pub untracked: usize,
    /// Files and directories `.gitignore` covers.
    pub ignored: usize,
    /// Commits the checkout's `HEAD` reaches that no branch, tag or remote
    /// does — a detached checkout somebody committed in, or a rebase stopped
    /// half way. The worktree's own reflog is the only thing that remembers
    /// them, and a removal deletes it: git removes over them without a word,
    /// and the next `gc` deletes the commits.
    pub stranded: usize,
}

impl Local {
    /// Whether the worktree holds nothing loose at all — nothing to warn
    /// about, nothing to lose, nothing to say.
    pub fn is_empty(self) -> bool {
        self.modified == 0 && self.untracked == 0 && self.ignored == 0 && self.stranded == 0
    }

    /// Whether the checkout holds work that must not go without a person
    /// saying so: what a plain [`remove`] would refuse over, and the commits
    /// it would lose without refusing.
    ///
    /// Ignored files are deliberately left out: git removes over them without
    /// complaining, and counting them here would have a UI offering to force
    /// past an objection git was never going to raise — which is exactly the
    /// crying wolf that makes people stop reading warnings. Stranded commits
    /// are the opposite case: git raises no objection either, and they are
    /// somebody's work, so a sweep that tidies checkouts away must pass over
    /// them rather than trust git to stop it.
    pub fn blocks_removal(self) -> bool {
        self.modified > 0 || self.untracked > 0 || self.stranded > 0
    }

    /// Counts one `git status --porcelain -z --ignored`.
    ///
    /// Each entry is `XY <path>`, NUL-terminated. Two of the codes are the
    /// buckets outright — `??` untracked, `!!` ignored — and everything else is
    /// a tracked file that changed.
    ///
    /// **A directory counts as one thing.** Without `--untracked-files=all`,
    /// git collapses an untracked or ignored directory into a single entry
    /// ending in `/`, so a `target/` with forty thousand files in it counts
    /// once. That is the honest answer to the question actually being asked —
    /// "is there anything here?" — and the true file count would cost a walk of
    /// the very directory the collapsing exists to avoid walking.
    pub fn parse_status(stdout: &[u8]) -> Self {
        let mut local = Self::default();
        let mut entries = stdout.split(|byte| *byte == 0).filter(|f| !f.is_empty());

        while let Some(entry) = entries.next() {
            let Some(code) = entry.get(..2) else {
                continue;
            };
            match code {
                b"??" => local.untracked += 1,
                b"!!" => local.ignored += 1,
                _ => {
                    local.modified += 1;
                    // A rename or a copy prints where it came from as a second
                    // NUL-terminated field. Not consuming it here would count
                    // the origin path as another changed file, and the letter
                    // can sit in either column.
                    if code.contains(&b'R') || code.contains(&b'C') {
                        entries.next();
                    }
                }
            }
        }

        local
    }
}

// MARK: - Why a command did not do what was asked

/// Why a worktree command failed.
///
/// Every variant is something a person can be told and most are something they
/// can be offered a way out of: jump to the checkout that already holds the
/// branch, pick a different name, force the removal, unlock it first. That is
/// the entire reason this is an enum and not an `anyhow::Error` — a UI cannot
/// offer "jump there" to a string.
#[derive(Debug)]
pub enum Error {
    /// There is no `git` on `PATH`. Latched, so nothing spawns again this
    /// session; a UI should hide the feature rather than offer it.
    GitMissing,
    /// git could not be started, or could not be waited for.
    CouldNotRun(std::io::Error),
    /// git was still running at the deadline and was killed.
    TimedOut {
        /// The deadline it outlived.
        after: Duration,
    },
    /// The directory is not inside a repository — including because it is not
    /// there at all.
    NotARepository,
    /// `path` exists already and is not an empty directory. An empty one is
    /// fine, and git will check out into it.
    PathExists {
        /// The path, echoed as it was passed to git.
        path: PathBuf,
    },
    /// A branch git allows in only one place at a time is already in another.
    ///
    /// `at` may no longer exist on disk — git holds the registration whether
    /// the directory survives or not — which is the case where a UI should
    /// offer to prune rather than to jump.
    AlreadyCheckedOut {
        /// The branch that is spoken for.
        branch: String,
        /// The checkout that has it.
        at: PathBuf,
    },
    /// The branch exists and could not be created again. Distinct from
    /// [`Self::AlreadyCheckedOut`]: nobody has this one checked out, so it is
    /// free to be checked out — the name is simply taken.
    BranchExists {
        /// The name that is taken.
        branch: String,
    },
    /// The name a new branch was to have is not one a branch can have: it was
    /// empty or began with a dash, and was refused before git was asked. The
    /// way out is typing another.
    InvalidBranch {
        /// The name, as it was handed to [`add`].
        branch: String,
    },
    /// The place a new branch was to start from is not one: git found no ref
    /// or commit by that name, or it began with a dash and was refused before
    /// git was asked. The way out is picking another.
    InvalidBase {
        /// The base, as it was handed to [`add`].
        base: String,
    },
    /// `path` is registered as a worktree and its directory is gone. `prune`,
    /// or a forced add over the top, is what clears it.
    MissingButRegistered {
        /// The registered path.
        path: PathBuf,
    },
    /// `path` holds modified or untracked files, and git will not throw them
    /// away unasked. [`local_work`] says what they are — including the ignored
    /// files git did not mention and would have deleted anyway.
    HoldsLocalWork {
        /// The checkout that is not clean.
        path: PathBuf,
    },
    /// The worktree is locked. `reason` is git's own, empty when the lock has
    /// none. [`remove`] never overrides one, see its documentation for why,
    /// and [`lock`] never puts another over it.
    Locked {
        /// Why whoever locked it said they were locking it.
        reason: String,
    },
    /// `path` is the repository's main checkout, which git will not remove.
    MainWorktree {
        /// The path that was asked for.
        path: PathBuf,
    },
    /// There is no worktree at `path`.
    NotAWorktree {
        /// The path that was asked for.
        path: PathBuf,
    },
    /// Anything else, carrying what git said.
    Failed {
        /// git's own diagnostic, tidied into one line.
        message: String,
    },
}

// Hand-written rather than derived. `thiserror` is in the workspace, but it is
// not one of this crate's dependencies, and adding it would move `Cargo.toml`
// and `Cargo.lock` for an enum whose messages come to twenty lines. The derive
// would save those twenty lines and nothing else.
impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GitMissing => write!(formatter, "git is not installed"),
            Self::CouldNotRun(error) => write!(formatter, "Could not run git: {error}"),
            Self::TimedOut { after } => {
                write!(formatter, "git did not answer in {}s", after.as_secs())
            }
            Self::NotARepository => write!(formatter, "Not a git repository"),
            Self::PathExists { path } => write!(formatter, "{} already exists", path.display()),
            Self::AlreadyCheckedOut { branch, at } => {
                write!(
                    formatter,
                    "{branch} is already checked out at {}",
                    at.display()
                )
            }
            Self::BranchExists { branch } => {
                write!(formatter, "A branch named {branch} already exists")
            }
            Self::InvalidBranch { branch } if branch.is_empty() => {
                write!(formatter, "A branch needs a name")
            }
            Self::InvalidBranch { branch } => {
                write!(
                    formatter,
                    "{branch} cannot name a branch: it begins with a dash"
                )
            }
            Self::InvalidBase { base } => {
                write!(formatter, "{base} is not a branch to start from")
            }
            Self::MissingButRegistered { path } => write!(
                formatter,
                "{} is registered as a worktree but is not there any more",
                path.display()
            ),
            Self::HoldsLocalWork { path } => write!(
                formatter,
                "{} holds work that removing it would throw away",
                path.display()
            ),
            Self::Locked { reason } if reason.is_empty() => {
                write!(formatter, "That worktree is locked")
            }
            Self::Locked { reason } => write!(formatter, "That worktree is locked: {reason}"),
            Self::MainWorktree { path } => write!(
                formatter,
                "{} is the main checkout and cannot be removed",
                path.display()
            ),
            Self::NotAWorktree { path } => {
                write!(formatter, "There is no worktree at {}", path.display())
            }
            Self::Failed { message } => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CouldNotRun(error) => Some(error),
            _ => None,
        }
    }
}

impl From<Failure> for Error {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::GitMissing => Self::GitMissing,
            Failure::CouldNotRun(error) => Self::CouldNotRun(error),
            Failure::TimedOut { after } => Self::TimedOut { after },
            // A path that is not there is not a repository either, and that
            // is what every command here has always called it.
            Failure::NoDirectory => Self::NotARepository,
        }
    }
}

// MARK: - Reading

/// Every worktree of the repository containing `directory`, main one first.
///
/// **Blocking.** One subprocess, background executor only, like everything else
/// here.
///
/// The order is git's, and git prints the main worktree first — which is the
/// only thing that identifies it, and so is what [`Worktree::is_main`] records.
/// A bare repository lists as one entry with no `HEAD` and no branch.
pub fn list(directory: &Path) -> Result<Vec<Worktree>, Error> {
    // `-z` rather than the newline-separated form, and not for tidiness. On git
    // 2.55.0 `--porcelain` does *not* C-quote a path containing a newline: a
    // worktree at `.../new\nline` prints as two lines, and a line-based parser
    // silently reads a truncated path plus a stray record. `-z` terminates
    // every field with a NUL and every record with an empty field, so a path is
    // exactly the bytes between two NULs and no quoting rule has to be
    // reimplemented to find out. It costs one flag.
    let finished = run(
        directory,
        &[
            OsStr::new("worktree"),
            OsStr::new("list"),
            OsStr::new("--porcelain"),
            OsStr::new("-z"),
        ],
        Intent::Read,
    )?;

    if !finished.success {
        return Err(classify(&finished.stderr));
    }
    Ok(parse_list(&finished.stdout))
}

/// Every branch the repository containing `directory` has, checked out or not.
///
/// **Blocking.** One subprocess, background executor only, like everything
/// else here — and the cheapest of the three reads: it walks `refs/heads` and
/// touches no working tree.
///
/// The names come back short — `main`, `worktree/amber-anchor-0155` — which is
/// the form [`Worktree::branch`] carries and the form [`add`] is handed, so the
/// three can be compared without any of them being reshaped first.
///
/// This exists for [`suggested_branch`], and the reason is worth stating where
/// it can be read: [`remove`] deliberately never deletes a branch, so every
/// checkout Crook has made and unmade has left its branch behind with nothing
/// checked out on it. Those are exactly the names a listing of *worktrees*
/// cannot see and `add` still refuses. It is also the list a new worktree can
/// be started from, and what [`default_branch`] checks its fallbacks against.
pub fn branches(directory: &Path) -> Result<Vec<String>, Error> {
    let finished = run(
        directory,
        &[
            OsStr::new("for-each-ref"),
            // `lstrip=2` rather than the more obvious `%(refname:short)`, which
            // is short in a way that is not always this: it shortens only as
            // far as the name stays unambiguous, so a repository holding both a
            // tag and a branch called `release` prints the branch as
            // `heads/release` — a name nothing else in this module would match
            // and `add` would never be given. Stripping two components off
            // `refs/heads/release` is the branch name and nothing else.
            OsStr::new("--format=%(refname:lstrip=2)"),
            OsStr::new("refs/heads"),
        ],
        Intent::Read,
    )?;

    if !finished.success {
        return Err(classify(&finished.stderr));
    }

    // Split on newlines, and not for want of the care `list` takes: that one
    // needs `-z` because a *path* may contain a newline, and a ref may not —
    // git's own `check-ref-format` refuses every ASCII control character in a
    // ref name. So a line here is exactly one branch.
    Ok(String::from_utf8_lossy(&finished.stdout)
        .lines()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect())
}

/// The branch new work in the repository containing `directory` usually
/// starts from, as a full ref name — `refs/remotes/origin/main`,
/// `refs/heads/main` — or `None` when nothing names one.
///
/// **Blocking.** One subprocess, and a second only when the first has no
/// answer; background executor only, like everything else here.
///
/// A full ref rather than the short name, because it is what [`add`] is
/// handed and a short name is resolved against tags and local branches before
/// remote ones: `origin/main` is a legal name for a *local* branch too, and a
/// repository holding one would start the worktree somewhere nobody picked.
///
/// `branches` is [`branches`]'s answer, which every caller has just read on
/// the same worker; reading it again here would be a third subprocess for a
/// list already in hand. `choose_default`, below, is the order the answers
/// are tried in.
pub fn default_branch(directory: &Path, branches: &[String]) -> Option<String> {
    let read = |args: &[&OsStr]| {
        run(directory, args, Intent::Read)
            .ok()
            .filter(|finished| finished.success)
            .map(|finished| String::from_utf8_lossy(&finished.stdout).trim().to_owned())
            .filter(|answer| !answer.is_empty())
    };

    // `for-each-ref` rather than `symbolic-ref`, for what it leaves out: a
    // remote HEAD that names a branch a `fetch --prune` has since deleted is
    // skipped as broken, where `symbolic-ref` would read the dangling target
    // back and `add` would then refuse it as an invalid reference.
    let origin_head = read(&[
        OsStr::new("for-each-ref"),
        OsStr::new("--format=%(symref)"),
        OsStr::new("refs/remotes/origin/HEAD"),
    ]);
    if origin_head.is_some() {
        return choose_default(origin_head.as_deref(), None, branches);
    }

    let configured = read(&[
        OsStr::new("config"),
        OsStr::new("--get"),
        OsStr::new("init.defaultBranch"),
    ]);
    choose_default(None, configured.as_deref(), branches)
}

/// Which of the answers [`default_branch`] read is the default branch.
///
/// In the order a person would look. What the remote says its default is
/// comes first — `origin/HEAD`, which `clone` writes and
/// `git remote set-head` refreshes — because it is the one answer about *this*
/// repository rather than about the machine. Then `init.defaultBranch`, then
/// `main`, then `master`, each only if the repository has a branch by that
/// name: the configuration is the machine's, and says nothing about a
/// repository that was cloned before it was set or made somewhere else.
///
/// Nothing that looks like a default is `None`, not a guess. A creator
/// offering a branch that does not exist is offering a failure.
///
/// Pure, so the order can be tested without four repositories.
fn choose_default(
    origin_head: Option<&str>,
    configured: Option<&str>,
    branches: &[String],
) -> Option<String> {
    if let Some(head) = origin_head.filter(|head| !head.is_empty()) {
        return Some(head.to_owned());
    }
    configured
        .into_iter()
        .chain(["main", "master"])
        .filter(|name| !name.is_empty())
        .find(|name| branches.iter().any(|branch| branch == name))
        .map(|name| format!("refs/heads/{name}"))
}

/// What is loose in `worktree`: what a removal would refuse over, and what it
/// would delete without mentioning.
///
/// **Blocking**, one subprocess, and the more expensive of the two reads here:
/// it stats the working tree. Call it when a person is about to be asked
/// whether to remove a checkout, not to keep a row up to date.
pub fn local_work(worktree: &Path) -> Result<Local, Error> {
    // A path that is not there is not a worktree, and saying so is better than
    // the "not a repository" the generic guard in `run` would produce.
    if !worktree.is_dir() {
        return Err(Error::NotAWorktree {
            path: worktree.to_owned(),
        });
    }

    let finished = run(
        worktree,
        &[
            OsStr::new("status"),
            OsStr::new("--porcelain"),
            OsStr::new("-z"),
            // Asked for precisely because git will *not* refuse a removal over
            // them. A build directory is the largest thing in a worktree and
            // the only thing that disappears without git ever saying so, which
            // makes it the one a person most needs told.
            OsStr::new("--ignored"),
        ],
        Intent::Read,
    )?;

    if !finished.success {
        return Err(classify(&finished.stderr));
    }
    let mut local = Local::parse_status(&finished.stdout);
    local.stranded = stranded(worktree)?;
    Ok(local)
}

/// How many commits `HEAD` in `worktree` reaches that no branch, tag or
/// remote does.
///
/// Not `--all`, which counts every worktree's `HEAD` as a ref — this one's
/// included — and would find nothing stranded ever. A checkout on a branch
/// comes to zero here by construction; a repository with no commit yet has
/// no `HEAD` to count from, and nothing to lose.
fn stranded(worktree: &Path) -> Result<usize, Error> {
    let finished = run(
        worktree,
        &[
            OsStr::new("rev-list"),
            OsStr::new("--count"),
            OsStr::new("HEAD"),
            OsStr::new("--not"),
            OsStr::new("--branches"),
            OsStr::new("--tags"),
            OsStr::new("--remotes"),
        ],
        Intent::Read,
    )?;
    if !finished.success {
        return Ok(0);
    }
    Ok(String::from_utf8_lossy(&finished.stdout)
        .trim()
        .parse()
        .unwrap_or(0))
}

// MARK: - Writing

/// Creates a worktree at `path`, on a new branch `branch`, starting from
/// `base` — or from `HEAD` when there is no `base`.
///
/// **Blocking**, and the slowest thing in this module: it writes out a working
/// tree and then runs the repository's `post-checkout` hook.
///
/// `path` should be absolute; [`checkout_path`] produces one. A relative path
/// is resolved by git, which is running in `repository` — not in the process's
/// own working directory, which is almost certainly not what a caller meant.
///
/// Nothing here reaches the network. That is not luck: left to itself
/// `git worktree add` will guess, and with no `-b` it can decide the path's
/// basename names a *remote* branch and go and fetch it. An explicit `-b` and
/// an explicit start point leave it nothing to guess with.
///
/// `base` is whatever git resolves — a full ref such as
/// `refs/remotes/origin/main` is what [`default_branch`] hands out, and is the
/// form that cannot be mistaken for a tag. It is refused as
/// [`Error::InvalidBase`] without git being asked when it is empty or begins
/// with a dash: `--` already keeps it from being read as an option, but past
/// `--` git still reads a lone `-` as `@{-1}`, the branch checked out before
/// this one, and a worktree started there is one nobody picked. No object
/// name begins with a dash, `git branch` refuses to make a branch that does,
/// and every base the creator offers is a full ref besides, so nothing real
/// is turned away.
///
/// `branch` is refused the same way, as [`Error::InvalidBranch`], and it is
/// the one of the two that `--` cannot protect. `-b <branch>` is carried out
/// by a child `git branch <branch> <base> --no-track` with nothing between
/// `branch` and the options, so a name of `-m` is `git branch -m <base>`: the
/// branch this repository has out, renamed to the base — which, being a full
/// ref, is a name `git branch` takes — before the worktree step fails. With
/// `HEAD` as the only base, `git branch` refused `HEAD` as a name and the
/// harm stopped there; with a base somebody picked, it does not. A branch
/// name cannot begin with a dash anyway (`git check-ref-format --branch`
/// refuses one), so here too nothing real is turned away.
///
/// The new branch is made with no upstream, whatever it starts from and
/// whatever `branch.autoSetupMerge` says. Made from `origin/main` git would
/// otherwise set `origin/main` as its upstream, and a branch called
/// `worktree/amber-anchor-0155` whose upstream is `origin/main` is one a
/// plain `git push` refuses and a plain `git pull` merges `main` into. Made
/// from `HEAD` that changes nothing under git's own default, and drops the
/// upstream `always` (the branch `HEAD` is on) or `inherit` (that branch's
/// own upstream) used to give it.
pub fn add(repository: &Path, path: &Path, branch: &str, base: Option<&str>) -> Result<(), Error> {
    if branch.is_empty() || branch.starts_with('-') {
        return Err(Error::InvalidBranch {
            branch: branch.to_owned(),
        });
    }
    let base = base.unwrap_or("HEAD");
    if base.is_empty() || base.starts_with('-') {
        return Err(Error::InvalidBase {
            base: base.to_owned(),
        });
    }

    let finished = run(
        repository,
        &[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--no-track"),
            OsStr::new("-b"),
            OsStr::new(branch),
            // So a path beginning with a dash is a path.
            OsStr::new("--"),
            path.as_os_str(),
            OsStr::new(base),
        ],
        Intent::Write,
    )?;

    if finished.success {
        return Ok(());
    }

    Err(match classify(&finished.stderr) {
        // git 2.55.0 reports a name that is taken as exactly that and says
        // nothing about where it is checked out: the "is already used by
        // worktree at '<path>'" wording comes from the forms of `add` that
        // check an *existing* branch out, which this one deliberately is not.
        // The path is the half a UI needs — it offers to jump there — and it is
        // one listing away, so the listing happens here rather than in every
        // caller. Best effort: a listing that fails leaves the error as it was.
        Error::BranchExists { branch } => match checked_out_at(repository, &branch) {
            Some(at) => Error::AlreadyCheckedOut { branch, at },
            None => Error::BranchExists { branch },
        },
        other => other,
    })
}

/// Removes the checkout at `path`, leaving its branch alone.
///
/// **Blocking**: it deletes a directory tree.
///
/// The branch is never deleted — not as an option, not under `force`. A branch
/// is the work; a checkout is a directory the work happened in, and deleting
/// the first because somebody asked to tidy up the second is not a thing a
/// terminal gets to decide.
///
/// `force` means "throw away what is loose in the directory": git refuses to
/// remove a worktree holding modified or untracked files, and `--force` is how
/// that refusal is answered.
///
/// It does **not** refuse over ignored files, on git 2.55.0 or anywhere else
/// this was measured — a checkout holding nothing but a `target/` removes on
/// the first try and takes the build with it. That silence is the reason
/// [`local_work`] counts them separately, and the reason a UI should show what
/// it found before it asks rather than after git has decided.
///
/// `force` deliberately does **not** override a lock. git wants `-f -f` for
/// that and only ever gets one `-f` here, so a locked worktree comes back as
/// [`Error::Locked`] whichever way this is called. A lock says an agent is
/// working in the checkout, and "discard my uncommitted work" must never
/// quietly also mean "take the checkout another agent is working in". The one
/// lock Crook takes off before a removal is its own, through [`release`], and
/// only once nothing in the window is working there.
pub fn remove(repository: &Path, path: &Path, force: bool) -> Result<(), Error> {
    let mut args = vec![OsStr::new("worktree"), OsStr::new("remove")];
    if force {
        args.push(OsStr::new("--force"));
    }
    args.push(OsStr::new("--"));
    args.push(path.as_os_str());

    let finished = run(repository, &args, Intent::Delete)?;
    if finished.success {
        return Ok(());
    }
    Err(classify(&finished.stderr))
}

/// Deletes the directories `removed` sat in that are empty now, from the
/// nearest up, stopping at the first that is not and never reaching `store`.
/// Answers how many went.
///
/// `git worktree remove` deletes the checkout and nothing above it, so a
/// store laid out as `<repository>/<branch>` is left holding a directory per
/// repository that nothing is in once its last checkout goes — and a checkout
/// made by hand at `<repository>/feat/x` leaves two. Nothing ever cleans
/// them up, and a store that is mostly empty directories is one a person
/// cannot read at a glance.
///
/// Two limits, and both are the point. Only inside `store`: a directory
/// outside it belongs to whoever laid it out, however empty it is, and a path
/// that climbs out through `..` is treated as outside whatever it starts
/// with. And only empty: `remove_dir` refuses a directory with anything in
/// it, so "is it empty?" and "delete it" are one question the filesystem
/// answers at once, with no moment between them for a checkout to arrive.
///
/// Best effort. A directory that will not go — not empty, not permitted,
/// already gone — ends the walk, and the removal it followed has already
/// succeeded either way.
pub fn prune_empty_parents(store: &Path, removed: &Path) -> usize {
    let climbs = removed
        .components()
        .any(|part| matches!(part, Component::ParentDir | Component::CurDir));
    if climbs || !removed.starts_with(store) {
        return 0;
    }

    let mut pruned = 0;
    let mut directory = removed.parent();
    while let Some(parent) = directory {
        if parent == store || !parent.starts_with(store) || std::fs::remove_dir(parent).is_err() {
            break;
        }
        pruned += 1;
        directory = parent.parent();
    }
    pruned
}

// MARK: - What a new checkout is given

/// The file in a repository's main checkout that names the ignored files a new
/// worktree of it is given.
///
/// Claude Code's name, and Claude Code's rules — see [`copy_included`] — so
/// that one file in a repository serves the worktrees both tools make. A second
/// file with a Crook name would be a second list of the same `.env` for
/// somebody to keep in step with the first.
pub const INCLUDE_FILE: &str = ".worktreeinclude";

/// What [`copy_included`] did for one new checkout.
///
/// The default is what a repository with no [`INCLUDE_FILE`] gets: nothing
/// copied, nothing left out, nothing refused.
#[derive(Debug, Default)]
pub struct Included {
    /// How many files were copied.
    pub copied: usize,
    /// Files that matched and were not copied, each with the reason.
    pub skipped: Vec<(PathBuf, Skip)>,
    /// Why nothing at all was copied, when that is what happened.
    pub refused: Option<Refusal>,
}

/// Why one matching file was not copied.
#[derive(Debug)]
pub enum Skip {
    /// The new checkout already has something at that path — a file the
    /// branch tracks, or one a `post-checkout` hook made. Never overwritten.
    Exists,
    /// It is a symbolic link, or the way to it goes through one, in the main
    /// checkout or in the new one. Never followed.
    Link,
    /// It is not a regular file: a socket, a pipe, a device.
    NotAFile,
    /// The main checkout ignores it and the new one does not — its
    /// `.gitignore` is an older branch's, or the main checkout's has an edit
    /// nobody committed. A copy there would be an untracked file, and the next
    /// `git add -A` in it would commit whatever secret the file holds.
    NotIgnored,
    /// The way to it goes through a submodule of the new checkout, where the
    /// main checkout has a plain directory. A file there is the submodule's,
    /// not the repository's — and written into a submodule nobody has
    /// initialised yet, which is every submodule of a checkout `git worktree
    /// add` has just made, it leaves a directory `git submodule update --init`
    /// refuses to clone into.
    Submodule,
    /// It is bigger than one file may be.
    TooLarge {
        /// Its size.
        bytes: u64,
        /// The most one file may be.
        limit: u64,
    },
    /// Reading it or writing the copy failed.
    Failed(std::io::Error),
}

/// Why nothing was copied from [`INCLUDE_FILE`].
#[derive(Debug)]
pub enum Refusal {
    /// More files match than a new checkout is given.
    TooMany {
        /// How many match.
        files: usize,
        /// How many a new checkout is given.
        limit: usize,
    },
    /// The files that match come to more bytes than a new checkout is given.
    TooLarge {
        /// How many bytes they come to.
        bytes: u64,
        /// How many a new checkout is given.
        limit: u64,
    },
    /// The file itself is there and could not be read.
    Unreadable(std::io::Error),
    /// git could not say which files the main checkout ignores, or which of
    /// them match.
    CouldNotLook(Error),
}

impl Included {
    /// The sentence a person should be shown, when something they asked for
    /// did not arrive: the copy was refused, or a file was left out for a
    /// reason other than already being there.
    ///
    /// `None` when everything that matched is in the new checkout, whoever put
    /// it there. A file the branch already has is the rule working, not a
    /// failure — and a note after every checkout that merely confirmed the
    /// copy would be a note people learn to dismiss unread.
    pub fn problem(&self) -> Option<String> {
        if let Some(refusal) = &self.refused {
            return Some(format!(
                "Nothing was copied from {INCLUDE_FILE}: {}.",
                refusal.reason()
            ));
        }

        let left_out: Vec<&(PathBuf, Skip)> = self
            .skipped
            .iter()
            .filter(|(_, skip)| !matches!(skip, Skip::Exists))
            .collect();
        if left_out.is_empty() {
            return None;
        }

        let mut named = left_out
            .iter()
            .take(NAMED_IN_A_NOTE)
            .map(|(path, skip)| format!("{} ({})", path.display(), skip.reason()))
            .collect::<Vec<_>>()
            .join(", ");
        let unnamed = left_out.len().saturating_sub(NAMED_IN_A_NOTE);
        if unnamed > 0 {
            named.push_str(&format!(" and {unnamed} more"));
        }
        Some(format!(
            "Copied {} from {INCLUDE_FILE}, but not {named}.",
            files(self.copied)
        ))
    }

    /// Nothing copied, for `refusal`.
    fn refused(refusal: Refusal) -> Self {
        Self {
            refused: Some(refusal),
            ..Self::default()
        }
    }
}

/// How many of the files left out a note names before it counts the rest.
///
/// The note is a line or two in a popup a few hundred pixels wide, and three
/// names is enough to say which kind of thing went wrong; the log has every
/// one of them.
const NAMED_IN_A_NOTE: usize = 3;

impl Skip {
    /// Why, as the few words a note puts in brackets after the path.
    fn reason(&self) -> String {
        match self {
            Self::Exists => "already there".to_owned(),
            Self::Link => "a symbolic link".to_owned(),
            Self::NotAFile => "not a file".to_owned(),
            Self::NotIgnored => "not ignored in the new checkout".to_owned(),
            Self::Submodule => "in a submodule of the new checkout".to_owned(),
            Self::TooLarge { bytes, limit } => {
                format!(
                    "{}, over the {} one file may be",
                    size(*bytes),
                    size(*limit)
                )
            }
            Self::Failed(error) => error.to_string(),
        }
    }
}

impl Refusal {
    /// Why, as the end of a sentence that begins "Nothing was copied".
    fn reason(&self) -> String {
        match self {
            Self::TooMany {
                files: count,
                limit,
            } => format!(
                "{} match it, more than the {limit} a new worktree is given",
                files(*count)
            ),
            Self::TooLarge { bytes, limit } => format!(
                "what matches it comes to {}, more than the {} a new worktree is given",
                size(*bytes),
                size(*limit)
            ),
            Self::Unreadable(error) => format!("it could not be read: {error}"),
            Self::CouldNotLook(error) => error.to_string(),
        }
    }
}

/// `count` files, in words.
fn files(count: usize) -> String {
    if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    }
}

/// `bytes` as a person reads a size.
fn size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes < KB {
        format!("{bytes} bytes")
    } else if bytes < MB {
        format!("{} KB", bytes.div_ceil(KB))
    } else if bytes.is_multiple_of(MB) {
        format!("{} MB", bytes / MB)
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

/// How many files a new checkout is given from [`INCLUDE_FILE`] before the
/// copy is refused outright.
///
/// What the file is for is a handful — `.env`, a local settings file, a
/// directory of certificates — and a thousand is far past any of that. What
/// goes past it is a pattern that caught a dependency tree: `*`, or `**` next
/// to a `node_modules/`, is tens of thousands of files. Refused whole rather
/// than cut off at the thousandth, because half a `node_modules` is worse than
/// none — it looks installed and is not — and which half would be the order
/// git happened to list it in.
pub const MAX_INCLUDED_FILES: usize = 1_000;

/// The largest one file copied from [`INCLUDE_FILE`] may be.
///
/// Configuration is kilobytes. A file past this is a database, a dump or a
/// build artefact a broad pattern caught, and copying it would make creating
/// the worktree the slow part of starting an agent. Left out on its own — the
/// rest of what matched is still configuration, and still wanted — and named
/// in the note, so a file somebody really meant is one they know to bring.
pub const MAX_INCLUDED_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// The most every file copied from [`INCLUDE_FILE`] may come to together.
///
/// The count and the per-file size between them still allow a thousand files
/// at the per-file limit, which is sixty-four gigabytes. This is the bound on
/// the whole, refused whole for the count's reason.
pub const MAX_INCLUDED_BYTES: u64 = 256 * 1024 * 1024;

/// How much a new checkout is given before a file is left out or the copy is
/// refused.
///
/// A value rather than the three constants read where they are needed, so the
/// tests can go past each one with a few bytes instead of writing hundreds of
/// megabytes to find out.
#[derive(Copy, Clone, Debug)]
struct Limits {
    /// More matching files than this, and nothing is copied.
    files: usize,
    /// A file bigger than this is left out.
    file_bytes: u64,
    /// All of them together bigger than this, and nothing is copied.
    total_bytes: u64,
    /// How many bytes of `:(exclude)` pathspecs the walk may be handed. See
    /// [`PRUNE_BUDGET`].
    prune_bytes: usize,
}

/// The limits outside a test.
const LIMITS: Limits = Limits {
    files: MAX_INCLUDED_FILES,
    file_bytes: MAX_INCLUDED_FILE_BYTES,
    total_bytes: MAX_INCLUDED_BYTES,
    prune_bytes: PRUNE_BUDGET,
};

/// Copies the ignored files the main checkout's [`INCLUDE_FILE`] names into
/// `worktree`, which has just been made from the repository `repository` is in.
///
/// **Blocking**: up to five git subprocesses and the copying. Background executor
/// only, and before a shell is started in `worktree` — a shell whose rc reads
/// `.env`, through `direnv` or anything like it, reads it once, as it starts.
/// The subprocesses are reads, under the read deadline like every other here;
/// the copying is local files and has no deadline of its own, and what bounds
/// it instead is the limits below, checked before the first byte is written.
///
/// Claude Code's rules, because the point is one file serving both tools
/// (<https://code.claude.com/docs/en/worktrees>):
///
/// * The file is [`INCLUDE_FILE`] at the root of the repository's **main**
///   checkout, which is also where every file is copied from — whichever
///   checkout `repository` is. A linked worktree has no `.env` of its own
///   unless somebody put one there.
/// * It is `.gitignore` syntax.
/// * A file is copied only when a pattern matches it **and** git ignores it.
///   A tracked file is already in the new checkout, as the branch has it, and
///   copying the main checkout's edit of it over the top would hand one agent
///   another's uncommitted work; an untracked file nothing ignores is work in
///   progress, not configuration.
/// * It goes to the same relative path in the new checkout.
/// * A directory git ignores as a whole — `node_modules/`, `target/` — is
///   looked inside only when a pattern reaches it: when it, or a directory
///   above it, matches a pattern; when a pattern names a path through it
///   (`vendor/**/config.json`); or, for a pattern beginning `**/`, when the
///   name after the `**/` is one of its own names (`**/.claude/skills/*.md`
///   reaches `.claude/`). `**/config.json` does not reach `vendor/`, and
///   neither does a bare `config.json`, which means the same thing. That is
///   what Claude Code documents, and it is also what keeps a `.env` pattern
///   from walking a `target/` of half a million files to look for one.
///
/// And Crook's own, which the documentation says nothing about either way:
///
/// * A symbolic link is never followed — not one git listed, not one on the
///   way to a file in either checkout. A link in the main checkout can point
///   anywhere on the machine, and a link in the new one would have the copy
///   written wherever it points.
/// * A file already in the new checkout is never overwritten.
/// * A file the new checkout does not ignore is not copied, because there it
///   would be an untracked file for the next `git add -A` to commit. See
///   [`Skip::NotIgnored`].
/// * A file whose way goes through a submodule of the new checkout is not
///   copied: it would be the submodule's, and the submodule could not then be
///   cloned over it. See [`Skip::Submodule`].
/// * The executable bit comes with the file, and on Unix the copy has the
///   source's mode from the moment it exists.
/// * Past [`MAX_INCLUDED_FILES`] or [`MAX_INCLUDED_BYTES`] nothing is copied,
///   and a file past [`MAX_INCLUDED_FILE_BYTES`] is left out on its own.
///
/// Nothing here fails the checkout it is copying into: that exists already,
/// and every way this can go wrong is a line in the log — and, from the moment
/// there is a file to have asked for something, a reason in what comes back.
/// See [`Included::problem`].
pub fn copy_included(repository: &Path, worktree: &Path) -> Included {
    copy_included_within(repository, worktree, LIMITS)
}

/// [`copy_included`] under `limits`.
fn copy_included_within(repository: &Path, worktree: &Path, limits: Limits) -> Included {
    let included = copy_or_refuse(repository, worktree, limits);

    for (path, skip) in &included.skipped {
        match skip {
            Skip::Exists => log::debug!(
                "{} is already in {}; not copied from {INCLUDE_FILE}",
                path.display(),
                worktree.display()
            ),
            _ => log::warn!(
                "{} not copied from {INCLUDE_FILE}: {}",
                path.display(),
                skip.reason()
            ),
        }
    }
    match &included.refused {
        Some(refusal) => log::warn!(
            "nothing copied from {INCLUDE_FILE} into {}: {}",
            worktree.display(),
            refusal.reason()
        ),
        None if included.copied > 0 => log::info!(
            "copied {} from {INCLUDE_FILE} into {}",
            files(included.copied),
            worktree.display()
        ),
        None => {}
    }
    included
}

/// The copy itself, with the logging of what it did left to the caller.
fn copy_or_refuse(repository: &Path, worktree: &Path, limits: Limits) -> Included {
    let main = match main_checkout(repository) {
        Ok(Some(main)) => main,
        // A bare repository has no checkout to have a `.env` in.
        Ok(None) => return Included::default(),
        // Logged and not refused, unlike every failure after this one. The
        // file is found through the main checkout, so without it there is no
        // telling whether the repository has one — and a note about a
        // `.worktreeinclude` somebody never wrote would be a note about
        // nothing.
        Err(error) => {
            log::warn!(
                "could not find the main checkout of {} to copy {INCLUDE_FILE} from: {error}",
                repository.display()
            );
            return Included::default();
        }
    };

    let include = main.join(INCLUDE_FILE);
    let text = match std::fs::read(&include) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Included::default();
        }
        Err(error) => return Included::refused(Refusal::Unreadable(error)),
    };
    let patterns = reaching_patterns(&String::from_utf8_lossy(&text));
    // Nothing but comments and negations: nothing can match, and git need
    // not be asked to confirm it.
    if patterns.is_empty() {
        return Included::default();
    }

    let candidates = match candidates(&main, &include, &patterns, limits.prune_bytes) {
        Ok(candidates) => candidates,
        Err(error) => return Included::refused(Refusal::CouldNotLook(error)),
    };
    if candidates.len() > limits.files {
        return Included::refused(Refusal::TooMany {
            files: candidates.len(),
            limit: limits.files,
        });
    }

    let mut included = Included::default();
    let mut going = Vec::with_capacity(candidates.len());
    for relative in candidates {
        match source(&main, &relative, limits) {
            Ok(metadata) => going.push((relative, metadata)),
            Err(skip) => included.skipped.push((relative, skip)),
        }
    }
    let going = match ignored_in(worktree, going, &mut included.skipped) {
        Ok(going) => going,
        Err(error) => return Included::refused(Refusal::CouldNotLook(error)),
    };
    // Measured before anything is written, so a refusal leaves the new
    // checkout exactly as git made it rather than holding the first few
    // hundred megabytes of what was refused.
    let total = going.iter().fold(0_u64, |total, (_, metadata)| {
        total.saturating_add(metadata.len())
    });
    if total > limits.total_bytes {
        return Included::refused(Refusal::TooLarge {
            bytes: total,
            limit: limits.total_bytes,
        });
    }

    for (relative, metadata) in going {
        match copy_one(&main, worktree, &relative, metadata.permissions()) {
            Ok(()) => included.copied += 1,
            Err(skip) => included.skipped.push((relative, skip)),
        }
    }
    included
}

/// The main checkout of the repository `repository` is in, or `None` when the
/// repository is bare and has none.
///
/// Out of `worktree list`, whose first record is the main checkout and which
/// is the rule [`Worktree::is_main`] already follows — rather than out of the
/// common git directory's parent, which is the main checkout only when nobody
/// used `--separate-git-dir`.
fn main_checkout(repository: &Path) -> Result<Option<PathBuf>, Error> {
    Ok(list(repository)?
        .into_iter()
        .next()
        .filter(|main| !main.is_bare)
        .map(|main| main.path))
}

/// The byte budget for the directories [`candidates`] tells git to leave out
/// of its walk.
///
/// They are pathspecs on the command line, and a Windows command line is
/// 32,767 characters, all of it. A monorepo with a `node_modules/` and a
/// `dist/` in each of four hundred packages would pass that, so the budget
/// goes to the shallowest directories and the deepest, which do not fit, are
/// walked instead — [`prunes`] says why. A directory walked gives the same
/// answer as one left out, because what is found in it is cut from the
/// candidates afterwards; what it costs is time, and a walk that outlives the
/// read deadline copies nothing.
const PRUNE_BUDGET: usize = 16 * 1024;

/// Every file in `main` that git ignores and that `patterns` — read from
/// `include` — match, as paths relative to `main`.
///
/// Two walks, because gitignore's grammar has no way to say "ignored by one
/// set of patterns *and* matched by another": in `.gitignore`'s precedence the
/// first list that has an opinion decides, and a list cannot be intersected
/// with another by writing it into the same file.
///
/// The first is `git status --ignored=matching`, which lists every ignored
/// file on its own — except that a directory a pattern ignores as a whole is
/// listed once, as the directory, without git walking into it. That is the
/// shape Claude Code's rule for such a directory is written against, and the
/// cheap half: `target/` is one line.
///
/// The second is `git ls-files --others --ignored --exclude-from=<include>`:
/// every untracked file the include patterns match, in git's own reading of
/// gitignore syntax, negations and all. `--exclude-from` is the flag that
/// *adds* ignore patterns, and `--ignored` turns the listing round to show
/// what they match instead of what they leave, so together they are "list
/// what these patterns match". The ignored directories no pattern reaches are
/// left out of that walk as `:(exclude)` pathspecs, as many as `prune_bytes`
/// holds, which is what keeps a `.env` pattern from reading all of `target/`
/// looking for one.
///
/// What comes back is the second list cut down to what the first says is
/// ignored: a file it listed, or one inside a directory it listed that a
/// pattern reaches.
fn candidates(
    main: &Path,
    include: &Path,
    patterns: &[Reach],
    prune_bytes: usize,
) -> Result<Vec<PathBuf>, Error> {
    let status = run(
        main,
        &[
            OsStr::new("status"),
            OsStr::new("--porcelain"),
            OsStr::new("-z"),
            OsStr::new("--ignored=matching"),
            // Spelled out, so a repository that sets `status.showUntrackedFiles`
            // to `no` still lists what it ignores — git lists nothing ignored
            // where it lists nothing untracked.
            OsStr::new("--untracked-files=normal"),
            // A submodule's own changes say nothing about what the
            // superproject ignores, and asking costs a `status` in each one.
            OsStr::new("--ignore-submodules=all"),
        ],
        Intent::Read,
    )?;
    if !status.success {
        return Err(classify(&status.stderr));
    }
    let ignored = ignored_entries(&status.stdout);

    let (files, directories): (Vec<&[u8]>, Vec<&[u8]>) = ignored
        .into_iter()
        .partition(|entry| !entry.ends_with(b"/"));
    let (reached, passed): (Vec<&[u8]>, Vec<&[u8]>) =
        directories.into_iter().partition(|directory| {
            let text = String::from_utf8_lossy(directory);
            let names: Vec<&str> = text.trim_end_matches('/').split('/').collect();
            patterns.iter().any(|pattern| pattern.reaches(&names))
        });
    // Nothing ignored that a pattern could be matching: the second walk would
    // list only files the cut below throws away, so it is not taken.
    if files.is_empty() && reached.is_empty() {
        return Ok(Vec::new());
    }

    let mut args = vec![
        OsString::from("ls-files"),
        OsString::from("-z"),
        OsString::from("--others"),
        OsString::from("--ignored"),
        OsString::from("--exclude-from"),
        include.as_os_str().to_owned(),
        OsString::from("--"),
    ];
    args.extend(prunes(&passed, prune_bytes));
    let args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();

    let listed = run(main, &args, Intent::Read)?;
    if !listed.success {
        return Err(classify(&listed.stderr));
    }

    let files: HashSet<&[u8]> = files.into_iter().collect();
    Ok(listed
        .stdout
        .split(|byte| *byte == 0)
        // An entry ending in a slash is a repository nested inside this one,
        // which git lists as a directory and does not look into.
        .filter(|path| !path.is_empty() && !path.ends_with(b"/"))
        .filter(|path| {
            files.contains(path) || reached.iter().any(|directory| path.starts_with(directory))
        })
        .map(path_from)
        .collect())
}

/// The `:(exclude)` pathspecs that leave `directories` out of a walk, as many
/// of them as fit in `budget` bytes, the shallowest first.
///
/// The shallowest first because that is where the weight is: `node_modules/`
/// and `target/` at the root hold hundreds of thousands of files between them
/// and cost a pathspec each, where the long tail of a monorepo's
/// `packages/<name>/dist/` is many pathspecs over a few files apiece. Keeping
/// the root's and walking the tail costs milliseconds; dropping everything
/// once the tail no longer fitted walked the root's too, and a cold walk of a
/// `node_modules/` runs past the read deadline. Among directories equally
/// deep the shorter goes first, so the budget leaves out as many as it can.
fn prunes(directories: &[&[u8]], budget: usize) -> Vec<OsString> {
    let mut specs: Vec<(usize, OsString)> = directories
        .iter()
        .map(|directory| {
            let depth = directory.iter().filter(|byte| **byte == b'/').count();
            // Literal, so a directory called `[abc]` or `*` is that
            // directory and not a glob over its neighbours.
            let mut spec = OsString::from(":(exclude,literal)");
            spec.push(path_from(directory));
            (depth, spec)
        })
        .collect();
    specs.sort_by_key(|(depth, spec)| (*depth, spec.len()));

    let mut left = budget;
    specs
        .into_iter()
        .filter_map(|(_, spec)| {
            let fits = spec.len() <= left;
            fits.then(|| {
                left -= spec.len();
                spec
            })
        })
        .collect()
}

/// The paths `git status --porcelain -z --ignored` marks ignored, a directory
/// keeping the slash git ends it with.
///
/// Every entry is `XY <path>`; `!!` is ignored. A rename or a copy of a tracked
/// file carries where it came from as a second field, which is skipped for
/// the reason [`Local::parse_status`] skips it.
fn ignored_entries(stdout: &[u8]) -> Vec<&[u8]> {
    let mut ignored = Vec::new();
    let mut entries = stdout.split(|byte| *byte == 0).filter(|f| !f.is_empty());
    while let Some(entry) = entries.next() {
        let (Some(code), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        if code == b"!!" {
            ignored.push(path);
        } else if code.contains(&b'R') || code.contains(&b'C') {
            entries.next();
        }
    }
    ignored
}

/// The file at `relative` in `main`, if it is one to copy: its metadata, or
/// why it is not.
fn source(main: &Path, relative: &Path, limits: Limits) -> Result<std::fs::Metadata, Skip> {
    if through_a_link(main, relative) {
        return Err(Skip::Link);
    }
    // `symlink_metadata`, which describes the link rather than what it points
    // at. git lists a link as a path of its own — to a file or to a directory,
    // it does not look — so this is where one is caught.
    let metadata = std::fs::symlink_metadata(main.join(relative)).map_err(Skip::Failed)?;
    if metadata.file_type().is_symlink() {
        return Err(Skip::Link);
    }
    if !metadata.is_file() {
        return Err(Skip::NotAFile);
    }
    if metadata.len() > limits.file_bytes {
        return Err(Skip::TooLarge {
            bytes: metadata.len(),
            limit: limits.file_bytes,
        });
    }
    Ok(metadata)
}

/// Of `going`, the files `worktree` ignores as well and has nothing in the
/// way of; each of the rest goes to `skipped` with its reason.
///
/// The main checkout's ignore rules decided which files qualify, and the new
/// checkout's can differ: a branch picked as the base from before `.env` was
/// in `.gitignore`, or a `.gitignore` edit in the main checkout nobody has
/// committed. There a copy would be an untracked file like any other, and an
/// agent's `git add -A` would commit it. Claude Code's documentation says
/// nothing about the new checkout's rules; this one is Crook's, and it only
/// ever leaves out a file Claude Code's rules would have copied, never adds
/// one.
///
/// Each question is asked only of what the one before it left: a way through
/// a link is [`Skip::Link`]; something already at the path — a file the
/// branch tracks, or one a hook made — is [`Skip::Exists`], as the copy would
/// have found it; a way through a submodule is [`Skip::Submodule`]. What is
/// left goes to one `git check-ignore --stdin` in the new checkout, and a file
/// it does not call ignored is [`Skip::NotIgnored`].
///
/// check-ignore dies on the first path it cannot take, and every other file's
/// answer goes with it. So links are asked about before it is, since it
/// refuses a path "beyond a symbolic link"; it does not read the index, where
/// a path in a submodule is "in submodule"; and every name goes to it behind
/// `./`, since to it one that begins with a colon is pathspec magic.
fn ignored_in(
    worktree: &Path,
    going: Vec<(PathBuf, std::fs::Metadata)>,
    skipped: &mut Vec<(PathBuf, Skip)>,
) -> Result<Vec<(PathBuf, std::fs::Metadata)>, Error> {
    let mut asking = Vec::with_capacity(going.len());
    for (relative, metadata) in going {
        // `copy_one` asks about links too; asked here as well, for
        // check-ignore's sake.
        if through_a_link(worktree, &relative) {
            skipped.push((relative, Skip::Link));
        } else if std::fs::symlink_metadata(worktree.join(&relative)).is_ok() {
            skipped.push((relative, Skip::Exists));
        } else {
            asking.push((relative, metadata));
        }
    }

    let submodules = submodules_on_the_way(worktree, &asking)?;
    let (asking, inside): (Vec<_>, Vec<_>) = asking
        .into_iter()
        .partition(|(relative, _)| !relative.ancestors().any(|above| submodules.contains(above)));
    skipped.extend(
        inside
            .into_iter()
            .map(|(relative, _)| (relative, Skip::Submodule)),
    );
    if asking.is_empty() {
        return Ok(asking);
    }

    let mut input = Vec::new();
    for (relative, _) in &asking {
        // Behind `./`, so that a name beginning with a colon is a name. Bare,
        // git reads it as pathspec magic: `:!x` is an exclude, which
        // check-ignore refuses outright, and `:memory:` is the path `memory:`,
        // whose answer is some other file's.
        input.extend_from_slice(b"./");
        input.extend_from_slice(&bytes_of(relative));
        input.push(0);
    }
    let answered = run_fed(
        worktree,
        &[
            OsStr::new("check-ignore"),
            // The ignore rules alone, without the index. With it, a path in
            // a submodule is fatal, and a name with a glob character in it —
            // `[ab].env` — is called not ignored whenever the glob matches a
            // tracked file, `a.env`. All the index would add is "a tracked
            // file is not ignored", and every tracked file the checkout has
            // was `Exists` before this.
            OsStr::new("--no-index"),
            OsStr::new("-z"),
            OsStr::new("--stdin"),
        ],
        input,
    )?;
    // 1 is "none of them is ignored", which is an answer; 128 is a failure.
    if !answered.success && answered.code != Some(1) {
        return Err(classify(&answered.stderr));
    }
    // Each path comes back as it went in, `./` and all.
    let ignored: HashSet<&[u8]> = answered
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| path.strip_prefix(b"./").unwrap_or(path))
        .collect();

    let (kept, not_ignored): (Vec<_>, Vec<_>) = asking
        .into_iter()
        .partition(|(relative, _)| ignored.contains(bytes_of(relative).as_slice()));
    skipped.extend(
        not_ignored
            .into_iter()
            .map(|(relative, _)| (relative, Skip::NotIgnored)),
    );
    Ok(kept)
}

/// The directories on the way to `asking`'s files that are submodules of
/// `worktree` — gitlinks in its index — relative to it.
///
/// Only a directory that could be one is asked about, which in the common
/// case is none and costs no git at all: one that is empty, which is how
/// `git worktree add` leaves a submodule and no other directory of the
/// checkout it makes, since git tracks no empty directory; or one holding a
/// `.git`, a submodule a `post-checkout` hook went on to initialise. Which of
/// those really are is the index's to say, not the shape of a directory — a
/// hook can make an empty one — so one `git ls-files --stage` over them does.
fn submodules_on_the_way(
    worktree: &Path,
    asking: &[(PathBuf, std::fs::Metadata)],
) -> Result<HashSet<PathBuf>, Error> {
    let mut looked: HashSet<&Path> = HashSet::new();
    let mut maybe = Vec::new();
    for (relative, _) in asking {
        for above in relative.ancestors().skip(1) {
            // One looked at before had every directory above it looked at
            // with it.
            if above.as_os_str().is_empty() || !looked.insert(above) {
                break;
            }
            if could_be_a_submodule(&worktree.join(above)) {
                maybe.push(above);
            }
        }
    }
    if maybe.is_empty() {
        return Ok(HashSet::new());
    }

    let mut args = vec![
        OsString::from("ls-files"),
        OsString::from("-z"),
        OsString::from("--stage"),
        OsString::from("--"),
    ];
    args.extend(maybe.into_iter().map(|directory| {
        // Literal, so a directory called `[abc]` is that directory.
        let mut spec = OsString::from(":(literal)");
        spec.push(directory);
        spec
    }));
    let args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
    let listed = run(worktree, &args, Intent::Read)?;
    if !listed.success {
        return Err(classify(&listed.stderr));
    }

    // Every entry is `<mode> <object> <stage>\t<path>`; a gitlink's mode is
    // 160000.
    Ok(listed
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let tab = entry.iter().position(|byte| *byte == b'\t')?;
            let (head, path) = entry.split_at(tab);
            head.starts_with(b"160000 ").then(|| path_from(&path[1..]))
        })
        .collect())
}

/// Whether `directory`, in a checkout git has just made, has the shape of a
/// submodule: empty, or holding a `.git`.
fn could_be_a_submodule(directory: &Path) -> bool {
    if std::fs::symlink_metadata(directory.join(".git")).is_ok() {
        return true;
    }
    std::fs::read_dir(directory).is_ok_and(|mut entries| entries.next().is_none())
}

/// Copies `relative` from `main` to the same place in `worktree`, making the
/// directories it goes in and giving it `permissions`.
fn copy_one(
    main: &Path,
    worktree: &Path,
    relative: &Path,
    permissions: std::fs::Permissions,
) -> Result<(), Skip> {
    // The new checkout's own links, which are the branch's: one tracked where
    // the copy wants a directory would have it written wherever that points.
    if through_a_link(worktree, relative) {
        return Err(Skip::Link);
    }
    let target = worktree.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(Skip::Failed)?;
    }

    let mut source = std::fs::File::open(main.join(relative)).map_err(Skip::Failed)?;
    let mut copy = match create_copy(&target, &permissions) {
        Ok(copy) => copy,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Skip::Exists);
        }
        Err(error) => return Err(Skip::Failed(error)),
    };

    let written =
        std::io::copy(&mut source, &mut copy).and_then(|_| copy.set_permissions(permissions));
    if let Err(error) = written {
        // The file is this call's own — `create_new` made it — so a half
        // copy can go rather than sit there looking like the real one.
        drop(copy);
        let _ = std::fs::remove_file(&target);
        return Err(Skip::Failed(error));
    }
    Ok(())
}

/// Creates the file a copy is written into at `target`, which must not exist,
/// born with the source's `permissions` as far as the platform allows.
///
/// `create_new` is the whole of "never overwrite", and it is one system call
/// rather than a look and then a write: it refuses anything already at the
/// path, a link to nowhere included, so nothing between a check and the open
/// can put something there to be written through.
///
/// Born with them, on Unix, rather than given them once the bytes are in: the
/// default is `0666` less the umask, and a key that is `0600` in the main
/// checkout would be readable by anybody on the machine for as long as the
/// write took — and for good, if Crook died before the `chmod` after it. The
/// umask can only take bits away, so the `set_permissions` that follows the
/// write is still what makes the mode exactly the source's.
fn create_copy(
    target: &Path,
    permissions: &std::fs::Permissions,
) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        options.mode(permissions.mode() & 0o777);
    }
    #[cfg(not(unix))]
    let _ = permissions;
    options.open(target)
}

/// Whether the way from `root` to `relative` passes through a symbolic link,
/// or leaves `root` some other way.
///
/// The directories above the file, not the file itself, which the callers look
/// at on their own. One that is not there yet is not a link, and is made as a
/// directory by whoever needs it.
fn through_a_link(root: &Path, relative: &Path) -> bool {
    let mut at = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        // git lists paths inside the repository and nothing else, so a `..`
        // or a root here is not a path to copy whatever else it is.
        let std::path::Component::Normal(name) = component else {
            return true;
        };
        if components.peek().is_none() {
            return false;
        }
        at.push(name);
        match std::fs::symlink_metadata(&at) {
            Ok(metadata) if metadata.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    false
}

/// One pattern of an [`INCLUDE_FILE`], read only as far as deciding which
/// ignored directories it reaches into.
///
/// Which files it matches is git's to say, in [`candidates`]; this is the one
/// question git cannot answer, because it is Claude Code's rule rather than
/// gitignore's.
#[derive(Debug)]
struct Reach {
    /// The pattern's names, split at its slashes, with a leading and a
    /// trailing slash taken off.
    names: Vec<String>,
    /// Whether it had a slash anywhere but at its end. In gitignore's grammar
    /// that ties it to the directory the file is in; without one it matches a
    /// name at any depth.
    anchored: bool,
}

impl Reach {
    /// Whether this pattern reaches into an ignored directory whose path from
    /// the root is `names`.
    ///
    /// See [`copy_included`] for the rule. A pattern without a slash matches a
    /// name at any depth, which is `**/` in front of it, and so reaches a
    /// directory only through one of the directory's own names — which, for a
    /// one-name pattern, is the directory or a directory above it matching.
    fn reaches(&self, names: &[&str]) -> bool {
        let Some(first) = self.names.first() else {
            return false;
        };
        if !self.anchored {
            return names.iter().any(|name| glob(first, name));
        }
        if first == "**" {
            return match self.names.get(1) {
                Some(after) => names.iter().any(|name| glob(after, name)),
                None => true,
            };
        }

        for (index, name) in names.iter().enumerate() {
            match self.names.get(index) {
                // The pattern ran out above the directory: it named one of
                // the directories the directory is in, and gitignore matches
                // everything inside a directory it matches.
                None => return true,
                Some(part) if part == "**" => return true,
                Some(part) if glob(part, name) => {}
                Some(_) => return false,
            }
        }
        // The directory ran out first, or both did together: the pattern
        // names it, or goes on inside it.
        true
    }
}

/// The patterns in an [`INCLUDE_FILE`]'s `text` that can add a file.
///
/// gitignore's line grammar, as much of it as [`Reach`] needs: blank lines and
/// `#` comments are nothing, a `\` before a leading `#` or `!` makes it a
/// character, and a `!` negation is left out — it can only take away what
/// another pattern matched, which cannot make a directory worth looking in.
///
/// Read the way git reads the file, which matters because git does the
/// matching and this only decides where it may look: a UTF-8 byte order mark
/// at the start is not part of the first pattern, and neither is the carriage
/// return at the end of a Windows line. Left in, either one would be a
/// character the pattern had to match, and its directory would go unwalked
/// with nothing said.
fn reaching_patterns(text: &str) -> Vec<Reach> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines()
        .filter_map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let line = line.trim_end_matches(' ');
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                return None;
            }
            let line = line
                .strip_prefix('\\')
                .filter(|rest| rest.starts_with(['#', '!']))
                .unwrap_or(line);
            // A trailing slash says the pattern matches only a directory,
            // which every path this is asked about is.
            let line = line.trim_end_matches('/');
            if line.is_empty() {
                return None;
            }
            Some(Reach {
                anchored: line.contains('/'),
                names: line
                    .trim_start_matches('/')
                    .split('/')
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

/// Whether `name`, one component of a path, matches `pattern`, one component
/// of a gitignore pattern.
///
/// `*` is any run of characters, `?` one, `[…]` one out of a set — `!` or `^`
/// first turning it round, `a-z` a range — and `\` makes the next character
/// itself. Two stars inside a component are two stars, which gitignore reads
/// as one. POSIX classes such as `[:alpha:]` are not understood, and read as
/// the characters they are spelled with; a pattern that relies on one reaches
/// fewer directories than git would match.
///
/// One backtracking point, the last star, rather than a recursion per star:
/// every other token takes exactly one character, and with that shape the
/// last star is the only choice worth revisiting, so a pattern with ten stars
/// costs what one does.
fn glob(pattern: &str, name: &str) -> bool {
    let tokens = tokens(pattern);
    let name: Vec<char> = name.chars().collect();
    let (mut token, mut at) = (0, 0);
    let mut star: Option<(usize, usize)> = None;

    loop {
        match tokens.get(token) {
            Some(Token::Star) => {
                star = Some((token, at));
                token += 1;
                continue;
            }
            Some(one) if at < name.len() && one.matches(name[at]) => {
                token += 1;
                at += 1;
                continue;
            }
            None if at == name.len() => return true,
            _ => {}
        }
        match star {
            Some((after, from)) if from < name.len() => {
                star = Some((after, from + 1));
                token = after + 1;
                at = from + 1;
            }
            _ => return false,
        }
    }
}

/// One piece of a pattern component, for [`glob`].
#[derive(Debug)]
enum Token {
    /// `*`, however many of them in a row.
    Star,
    /// `?`.
    Any,
    /// A character that matches itself.
    Char(char),
    /// `[…]`: the ranges it holds, a lone character being a range of one.
    Class {
        /// Whether it began `!` or `^`, and matches what is *not* in it.
        negated: bool,
        /// Its ranges, both ends included.
        ranges: Vec<(char, char)>,
    },
}

impl Token {
    /// Whether this one-character token matches `character`.
    fn matches(&self, character: char) -> bool {
        match self {
            Self::Star | Self::Any => true,
            Self::Char(own) => *own == character,
            Self::Class { negated, ranges } => {
                ranges
                    .iter()
                    .any(|(low, high)| (*low..=*high).contains(&character))
                    != *negated
            }
        }
    }
}

/// `pattern` as [`glob`]'s tokens.
fn tokens(pattern: &str) -> Vec<Token> {
    let characters: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < characters.len() {
        match characters[at] {
            '*' => {
                if !matches!(tokens.last(), Some(Token::Star)) {
                    tokens.push(Token::Star);
                }
                at += 1;
            }
            '?' => {
                tokens.push(Token::Any);
                at += 1;
            }
            '\\' if at + 1 < characters.len() => {
                tokens.push(Token::Char(characters[at + 1]));
                at += 2;
            }
            '[' => match class(&characters[at + 1..]) {
                Some((class, used)) => {
                    tokens.push(class);
                    at += 1 + used;
                }
                // No closing bracket: the bracket is a character, which is
                // what git makes of one.
                None => {
                    tokens.push(Token::Char('['));
                    at += 1;
                }
            },
            character => {
                tokens.push(Token::Char(character));
                at += 1;
            }
        }
    }
    tokens
}

/// The class a `[` opens, read from what follows it, and how many characters
/// it took up to and including its `]` — or `None` when nothing closes it.
///
/// A `]` straight after the `[`, or after its `!`, is a member rather than the
/// end, as it is in every glob that has classes.
fn class(rest: &[char]) -> Option<(Token, usize)> {
    let negated = matches!(rest.first(), Some('!' | '^'));
    let mut at = usize::from(negated);
    let mut ranges = Vec::new();
    let mut first = true;
    while at < rest.len() {
        if rest[at] == ']' && !first {
            return Some((Token::Class { negated, ranges }, at + 1));
        }
        first = false;
        if rest[at] == '\\' && at + 1 < rest.len() {
            at += 1;
        }
        let low = rest[at];
        if at + 2 < rest.len() && rest[at + 1] == '-' && rest[at + 2] != ']' {
            ranges.push((low, rest[at + 2]));
            at += 3;
        } else {
            ranges.push((low, low));
            at += 1;
        }
    }
    None
}

// MARK: - Saying a checkout is in use

/// What every reason Crook locks a checkout with begins with.
///
/// git records a lock as a sentence in a file and nothing about who wrote it,
/// so the sentence has to say. This is how Crook tells its own lock from
/// anybody else's. Its own the worktree menu may take off before removing a
/// checkout nothing in the window is working in — one a Crook that crashed or
/// was killed left behind, which would otherwise hold the checkout against
/// Crook's own menu for ever. The prefix does not say *which* Crook, so a
/// window closing its panes takes off only the locks it remembers taking.
/// Any other reason is somebody else's — Claude Code's `claude session …`, a
/// person's own `git worktree lock` — and nothing here ever takes one of
/// those off.
pub const LOCK_PREFIX: &str = "crook: ";

/// The reason Crook locks the checkout it made for `branch` with.
///
/// The branch after the prefix, because the reason is what `git worktree
/// list` and a refused `git worktree remove` show a person, and
/// `crook: worktree/amber-anchor-0155` says who is holding what.
pub fn lock_reason(branch: &str) -> String {
    format!("{LOCK_PREFIX}{branch}")
}

impl Worktree {
    /// Whether the lock on this checkout is one Crook took. See
    /// [`LOCK_PREFIX`].
    pub fn is_locked_by_crook(&self) -> bool {
        self.locked
            .as_deref()
            .is_some_and(|reason| reason.starts_with(LOCK_PREFIX))
    }

    /// Whether somebody other than Crook has locked this checkout — with no
    /// reason at all too, which Crook never does.
    pub fn is_locked_by_another(&self) -> bool {
        self.locked.is_some() && !self.is_locked_by_crook()
    }
}

/// Locks the checkout at `path`, giving `reason` as why.
///
/// **Blocking**, one subprocess, and a quick one: git writes one small file
/// under the repository's `worktrees/<name>`.
///
/// A lock is how everything else that touches the repository is told that
/// somebody is working in the checkout: git refuses to `remove` or `prune` a
/// locked worktree until it is unlocked or told twice, and a tool that tidies
/// checkouts away is expected to pass over one.
///
/// A checkout somebody has locked already stays theirs and comes back as
/// [`Error::Locked`] with their reason. git will not put one lock over
/// another, and neither does this.
pub fn lock(repository: &Path, path: &Path, reason: &str) -> Result<(), Error> {
    let finished = run(
        repository,
        &[
            OsStr::new("worktree"),
            OsStr::new("lock"),
            OsStr::new("--reason"),
            OsStr::new(reason),
            OsStr::new("--"),
            path.as_os_str(),
        ],
        Intent::Write,
    )?;

    if finished.success {
        return Ok(());
    }
    Err(classify(&finished.stderr))
}

/// Unlocks the checkout at `path`, whoever locked it.
///
/// Private, because "whoever" is the whole danger: [`release`] is the way
/// in, and it reads whose the lock is first.
///
/// A checkout that is not locked is not an error. Two asks can overlap — the
/// last pane leaving a checkout as the window closes, or as the menu removes
/// it — and the second finding the work done is the state they were both
/// asking for.
fn unlock(repository: &Path, path: &Path) -> Result<(), Error> {
    let finished = run(
        repository,
        &[
            OsStr::new("worktree"),
            OsStr::new("unlock"),
            OsStr::new("--"),
            path.as_os_str(),
        ],
        Intent::Write,
    )?;

    // "'<path>' is not locked", on git 2.55.0.
    if finished.success || finished.stderr.contains("is not locked") {
        return Ok(());
    }
    Err(classify(&finished.stderr))
}

/// Takes Crook's own lock off the checkout at `path`, and leaves anybody
/// else's where it is. Says whether there was one to take off.
///
/// **Blocking**: one read to learn whose the lock is, and one write when it
/// is Crook's. The read is not optional. `git worktree unlock` removes
/// whatever lock is there, so the only thing standing between "Crook's tab
/// closed" and "Claude's session lost the lock it took" is looking first.
///
/// `path` is matched against git's own listing, so it should be a path git
/// printed. Whether nothing is working in the checkout any more is the
/// caller's question to answer before asking this: a lock is taken off
/// because the last pane left, and only the window knows where its panes are
/// and which locks it took.
pub fn release(repository: &Path, path: &Path) -> Result<bool, Error> {
    let ours = list(repository)?
        .iter()
        .any(|worktree| worktree.path == path && worktree.is_locked_by_crook());
    if !ours {
        return Ok(false);
    }
    unlock(repository, path)?;
    Ok(true)
}

// MARK: - Naming the next one

/// A branch name that is neither one of `branches` nor checked out in one of
/// `worktrees`.
///
/// Shaped like herdr's: `worktree/<adjective>-<noun>-<four hex digits>`. Short
/// enough to fit a tab row, lowercase and hyphenated so it is a legal ref and a
/// legal path component, and prefixed so that every branch Crook invented is
/// one `git branch --list 'worktree/*'` away from being found again.
///
/// Two lists, and the branches are the half that matters. [`add`] collides
/// with a *name*, not with a checkout — it refuses `worktree/amber-anchor-0155`
/// whether or not anybody has it out — and [`remove`] never deletes a branch,
/// so every checkout Crook has ever made and unmade is still a name that is
/// taken. Suggesting against the worktrees alone offers the first of those
/// names back the moment its checkout is gone, and goes on offering it, because
/// the walk below is deterministic and starts from the beginning every time.
///
/// The worktrees are not redundant for all that a healthy repository makes
/// them a subset of the branches. The two are separate reads and either can
/// fail, and a suggestion made from half an answer should still step over the
/// half it was given.
///
/// Pure, and deliberately so. The caller reads the repository when its menu
/// opens and calls this from the frame that draws the creator, which is no
/// place for a subprocess — and a function that took a `&Path` could not be
/// tested against a repository that does not exist.
///
/// Deterministic, and not seeded by the clock: the same repository always
/// suggests the same name. That makes the function testable, makes a retry
/// after a failed `add` predictable, and costs nothing — the vocabulary is
/// walked in order and the first free name wins, so collisions are avoided
/// outright instead of being made unlikely.
pub fn suggested_branch(worktrees: &[Worktree], branches: &[String]) -> String {
    let taken = taken(worktrees, branches);

    (0u32..)
        .map(candidate_branch)
        .find(|name| !taken.contains(name.as_str()))
        .expect("four billion candidate names cannot all be taken")
}

/// The branch a task started from `prompt` is made on: the prompt's words,
/// under the prefix [`suggested_branch`] gives its names, or that suggestion
/// itself when the prompt has no words to name a branch with.
///
/// `Fix the login bug!` is `worktree/fix-the-login-bug`. The words are ASCII
/// letters and digits, lowercased; a letter outside ASCII is dropped from its
/// word, and everything else — spaces, punctuation, a quote or a `$(` — is
/// one dash between two words. The same bluntness [`checkout_path`] applies to
/// a directory, and for its reason: a name every filesystem, shell and ref
/// reads the same, rather than a guess at what `é` should become in each.
///
/// Cut to whole words under forty characters, because a prompt is a sentence
/// and a tab row is not, and the first few words are the part that says which
/// task this is. A name that is taken gets `-2`, `-3`… on the end,
/// since [`add`] refuses a name that merely exists and the same prompt twice
/// is two tasks.
///
/// Pure, like the suggestion: the creator calls it on every keystroke of the
/// prompt, from the frame's own thread.
pub fn branch_for_prompt(prompt: &str, worktrees: &[Worktree], branches: &[String]) -> String {
    let words = prompt_words(prompt);
    if words.is_empty() {
        return suggested_branch(worktrees, branches);
    }

    let taken = taken(worktrees, branches);
    let first = format!("{BRANCH_PREFIX}{words}");
    std::iter::once(first.clone())
        .chain((2u32..).map(|count| format!("{first}-{count}")))
        .find(|name| !taken.contains(name.as_str()))
        .expect("four billion numbered names cannot all be taken")
}

/// The prefix every branch Crook names begins with.
const BRANCH_PREFIX: &str = "worktree/";

/// How many characters of a prompt's words a branch is named with, at most.
const PROMPT_WORDS: usize = 40;

/// A prompt's words for a branch name: see [`branch_for_prompt`]. Empty when
/// it has none.
fn prompt_words(prompt: &str) -> String {
    let mut words = String::new();
    let mut gap = false;
    for character in prompt.chars() {
        if character.is_ascii_alphanumeric() {
            if gap && !words.is_empty() {
                words.push('-');
            }
            gap = false;
            words.push(character.to_ascii_lowercase());
        } else if !character.is_alphanumeric() {
            gap = true;
        }
    }

    if words.len() <= PROMPT_WORDS {
        return words;
    }
    // At the last dash under the cap, so the name ends on a whole word; a
    // first word longer than the cap has no dash to stop at and is cut in it.
    let cut = &words[..PROMPT_WORDS];
    let whole = if words.as_bytes()[PROMPT_WORDS] == b'-' {
        cut
    } else {
        cut.rfind('-').map_or(cut, |dash| &cut[..dash])
    };
    whole.to_owned()
}

/// Every name that is spoken for: a branch, or one a checkout is on.
fn taken<'a>(worktrees: &'a [Worktree], branches: &'a [String]) -> HashSet<&'a str> {
    worktrees
        .iter()
        .filter_map(|worktree| worktree.branch.as_deref())
        .chain(branches.iter().map(String::as_str))
        .collect()
}

/// Where the checkout for `branch` of `repository_name` should live under
/// `store`.
///
/// Grouped by repository, because one store serves every repository a person
/// has open and `~/.crook/worktrees/main` would otherwise mean four different
/// things at once.
///
/// Both halves are slugged, so the result is a path every filesystem accepts:
/// a branch called `eugen/Fix Tab Bar` becomes `eugen-fix-tab-bar`, and no
/// directory is created for the `eugen` that was never a directory.
pub fn checkout_path(store: &Path, repository_name: &str, branch: &str) -> PathBuf {
    store.join(slug(repository_name)).join(slug(branch))
}

/// The adjectives half of the vocabulary, one per letter so the list is
/// obviously complete and obviously has no duplicates.
const ADJECTIVES: [&str; 16] = [
    "amber", "brisk", "calm", "dusky", "eager", "fleet", "glad", "hazy", "ivory", "jolly", "keen",
    "lively", "mellow", "nimble", "olive", "plucky",
];

/// The nouns half. 16 x 16 is 256 pairs before the vocabulary repeats, which is
/// more open checkouts than anyone has.
const NOUNS: [&str; 16] = [
    "anchor", "badger", "cedar", "delta", "ember", "falcon", "grove", "harbor", "inlet", "jetty",
    "kestrel", "lantern", "meadow", "north", "otter", "pebble",
];

/// The `attempt`th candidate name.
///
/// The pairs are walked adjective-first so consecutive attempts read as
/// different words rather than as the same word with a different number, and
/// the four hex digits are a digest of the pair rather than the attempt itself
/// — a name ending `-0000` reads like a counter and invites people to read an
/// order into it, and once the 256 pairs are exhausted the digest is what keeps
/// the names apart.
fn candidate_branch(attempt: u32) -> String {
    let index = attempt as usize;
    let adjective = ADJECTIVES[index % ADJECTIVES.len()];
    let noun = NOUNS[(index / ADJECTIVES.len()) % NOUNS.len()];
    let tag = tag(adjective, noun, attempt);
    format!("{BRANCH_PREFIX}{adjective}-{noun}-{tag:04x}")
}

/// Four hex digits that depend on the whole candidate.
///
/// FNV-1a, folded in half. Hand-rolled rather than `DefaultHasher` because that
/// one is explicitly allowed to change between releases of the standard
/// library, and a name that changed when the toolchain did would be a name
/// nobody could write a test against.
fn tag(adjective: &str, noun: &str, attempt: u32) -> u16 {
    let mut hash: u32 = 0x811c_9dc5;
    let bytes = adjective
        .bytes()
        .chain(std::iter::once(b'-'))
        .chain(noun.bytes())
        .chain(attempt.to_le_bytes());
    for byte in bytes {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (hash ^ (hash >> 16)) as u16
}

/// What a slug falls back to when nothing survives the rules.
const SLUG_FALLBACK: &str = "worktree";

/// The longest a slugged path component may be.
///
/// Comfortably inside the 255 bytes every filesystem in use allows, and short
/// enough that the resulting path is still readable in a picker. Two very long
/// branch names can slug to the same directory, which shows up as
/// [`Error::PathExists`] rather than as a silent overwrite, because
/// `git worktree add` refuses a non-empty directory.
const SLUG_LIMIT: usize = 64;

/// `text` as one path component: ASCII alphanumerics lowercased, everything
/// else a single `-`, no leading or trailing dashes, never empty, and never a
/// name Windows keeps for a device.
///
/// Deliberately blunt about non-ASCII — `café` slugs to `caf-` and then `caf` —
/// because the alternative is deciding what a filesystem, a shell and a person
/// reading a path will each make of an arbitrary Unicode scalar, and the answer
/// differs on all three platforms.
fn slug(text: &str) -> String {
    let mut slug = String::with_capacity(text.len().min(SLUG_LIMIT));
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= SLUG_LIMIT {
            break;
        }
    }

    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        SLUG_FALLBACK.to_owned()
    } else if crate::filename::windows_reserved(trimmed) {
        // `con`, `com1`… name devices rather than directories on Windows, so a
        // branch or repository called one gets a trailing dash to check out
        // under. See [`crate::filename::windows_reserved`].
        format!("{trimmed}-")
    } else {
        trimmed.to_owned()
    }
}

// MARK: - Parsing what git said

/// Splits `git worktree list --porcelain -z` into records.
///
/// Every attribute is a NUL-terminated field and an empty field ends a record:
///
/// ```text
/// worktree /home/eugen/Work/crook\0HEAD 439ec81…\0branch refs/heads/main\0\0
/// worktree /home/eugen/Work/crook/.claude/worktrees/x\0HEAD 439ec81…\0detached\0locked claude session x\0\0
/// ```
///
/// `detached` needs no handling: a checkout with no `branch` attribute is
/// detached, which is what `branch: None` already says. A bare entry has
/// neither `HEAD` nor `branch` and is the only one whose `head` is `None`.
///
/// Unknown attributes are ignored rather than refused. git has added to this
/// format before — `prunable` arrived in 2.30 — and a Crook that could not list
/// worktrees at all because a newer git mentioned something new would be far
/// worse than one that skipped a line it had no field for.
fn parse_list(stdout: &[u8]) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;

    for field in stdout.split(|byte| *byte == 0) {
        if field.is_empty() {
            worktrees.extend(current.take());
            continue;
        }

        if let Some(path) = attribute(field, "worktree") {
            // `replace` rather than an assignment: with `-z` every record is
            // terminated, so `current` is always `None` here — but output cut
            // short, by a kill or a truncated pipe, should still yield every
            // record that did arrive rather than dropping the one before the
            // cut.
            worktrees.extend(current.replace(Worktree {
                path: path_from(path),
                head: None,
                branch: None,
                is_main: false,
                is_bare: false,
                locked: None,
                prunable: None,
            }));
            continue;
        }

        let Some(worktree) = current.as_mut() else {
            // An attribute before any `worktree` line. git emits none; if a
            // future one does, skipping it is the only sane thing to do.
            continue;
        };

        if let Some(head) = attribute(field, "HEAD") {
            worktree.head = Some(short_sha(head));
        } else if let Some(reference) = attribute(field, "branch") {
            worktree.branch = Some(branch_name(reference));
        } else if attribute(field, "bare").is_some() {
            worktree.is_bare = true;
        } else if let Some(reason) = attribute(field, "locked") {
            worktree.locked = Some(text(reason));
        } else if let Some(reason) = attribute(field, "prunable") {
            worktree.prunable = Some(text(reason));
        }
    }

    worktrees.extend(current.take());

    // The main worktree is the one git printed first, and nothing else in the
    // output distinguishes it.
    if let Some(main) = worktrees.first_mut() {
        main.is_main = true;
    }

    worktrees
}

/// `field`'s value if it is the attribute called `name`.
///
/// The separator is checked here rather than being written into `name` because
/// half of these attributes have no value at all: `bare` and `locked` appear
/// bare as often as `locked <reason>` does. Checking it also keeps a
/// hypothetical `worktrees` attribute from matching `worktree`.
fn attribute<'a>(field: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let rest = field.strip_prefix(name.as_bytes())?;
    match rest.first() {
        None => Some(&[]),
        Some(b' ') => Some(&rest[1..]),
        Some(_) => None,
    }
}

/// A path exactly as git printed it.
///
/// On Unix a path is bytes, and a filename that is not UTF-8 is legal and does
/// happen; converting it lossily would produce a path that looks right in a
/// list and cannot be opened, removed or compared, which is the worst of both.
/// On Windows there is no such conversion to make — git writes UTF-8 and
/// `PathBuf` holds UTF-16 — so lossy is the only route, and is exact for every
/// path git can produce there.
pub(super) fn path_from(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        PathBuf::from(OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// `path` as the bytes git would print for it: [`path_from`] the other way
/// round, and exact for every path that came out of it.
fn bytes_of(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}

/// An object id abbreviated the way [`super::branch`] abbreviates one, so the
/// two halves of the git module print the same commit the same way.
fn short_sha(bytes: &[u8]) -> String {
    let full = text(bytes);
    full.get(..7).unwrap_or(&full).to_owned()
}

/// A ref without its `refs/heads/` prefix.
///
/// Anything else is kept whole: a worktree's `branch` attribute is always under
/// `refs/heads/`, and inventing a shortening for a form git does not emit would
/// be inventing a bug for later.
fn branch_name(bytes: &[u8]) -> String {
    let reference = text(bytes);
    reference
        .strip_prefix("refs/heads/")
        .map_or(reference.clone(), str::to_owned)
}

/// Bytes as text, lossily.
///
/// Sound for everything that is not a path: a branch name, a lock reason and an
/// object id are shown and compared, never opened, so a replacement character
/// in one is a cosmetic problem rather than a broken operation. Paths go
/// through [`path_from`] instead.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Turns git's diagnostics into something a UI can act on.
///
/// Line by line, first match wins, and that is not an optimisation: `add`
/// writes `Preparing worktree (new branch 'x')` to *stderr* even when it is
/// about to fail, and that line has a quoted name in it. Anything that searched
/// the whole of stderr for the first quoted string would confidently report the
/// branch it was asked to create as the branch that was already taken.
///
/// The exit code is deliberately never consulted. git uses 128 for most
/// `fatal:`s and 255 for others — `a branch named 'x' already exists` is 255 on
/// 2.55.0 — so it separates nothing that the message does not separate better.
///
/// Matching is always on the distinguishing fragment and never on a whole line,
/// because git rewords the ends of these between versions and even between
/// invocations of one version: "not a git repository" is followed by "(or any
/// of the parent directories): .git" from a subdirectory and by "(or any parent
/// up to mount point /)" from a directory on another filesystem.
fn classify(stderr: &str) -> Error {
    stderr
        .lines()
        .find_map(classify_line)
        .unwrap_or_else(|| Error::Failed {
            message: summarise(stderr),
        })
}

/// The one line that says what went wrong, if this is it.
fn classify_line(line: &str) -> Option<Error> {
    // git single-quotes every path and ref it names, and does it with a plain
    // `'` even when the thing quoted contains one — so these are the quoted
    // runs in order, and nothing here tries to be cleverer than the format is.
    let mut quoted = line.split('\'').skip(1).step_by(2);
    let first = quoted.next();
    let second = quoted.next();

    if line.contains("not a git repository") {
        return Some(Error::NotARepository);
    }
    if line.contains("is already used by worktree at")
        && let (Some(branch), Some(at)) = (first, second)
    {
        return Some(Error::AlreadyCheckedOut {
            branch: branch.to_owned(),
            at: PathBuf::from(at),
        });
    }
    // Before the plain "already exists" below, which this line also contains.
    if line.contains("already exists")
        && line.contains("a branch named")
        && let Some(branch) = first
    {
        return Some(Error::BranchExists {
            branch: branch.to_owned(),
        });
    }
    // Unquoted, unlike everything else here: the name runs to the end of the
    // line.
    if let Some((_, base)) = line.split_once("invalid reference:") {
        return Some(Error::InvalidBase {
            base: base.trim().to_owned(),
        });
    }
    if line.contains("is a missing but already registered worktree")
        && let Some(path) = first
    {
        return Some(Error::MissingButRegistered {
            path: PathBuf::from(path),
        });
    }
    if line.contains("already exists")
        && let Some(path) = first
    {
        return Some(Error::PathExists {
            path: PathBuf::from(path),
        });
    }
    if line.contains("contains modified or untracked files")
        && let Some(path) = first
    {
        return Some(Error::HoldsLocalWork {
            path: PathBuf::from(path),
        });
    }
    if line.contains("cannot remove a locked working tree") {
        // "…, lock reason: <reason>" when there is one, and "…;" when there is
        // not. The reason runs to the end of the line.
        let reason = line
            .split_once("lock reason:")
            .map_or("", |(_, reason)| reason.trim());
        return Some(Error::Locked {
            reason: reason.to_owned(),
        });
    }
    // `lock` over somebody else's lock: "'<path>' is already locked, reason:
    // <reason>", or the same without the reason when there is none.
    if line.contains("is already locked") {
        let reason = line
            .split_once("is already locked, reason:")
            .map_or("", |(_, reason)| reason.trim());
        return Some(Error::Locked {
            reason: reason.to_owned(),
        });
    }
    if line.contains("is a main working tree")
        && let Some(path) = first
    {
        return Some(Error::MainWorktree {
            path: PathBuf::from(path),
        });
    }
    if line.contains("is not a working tree")
        && let Some(path) = first
    {
        return Some(Error::NotAWorktree {
            path: PathBuf::from(path),
        });
    }

    None
}

/// git's diagnostics as one line somebody can be shown.
///
/// Everything before the first `fatal:` is progress chatter — `add` announces
/// what it is preparing on stderr whether or not it goes on to fail — and
/// `hint:` lines are advice about git configuration addressed to a person at a
/// command line, of whom there is none here. What is left is joined rather than
/// truncated, because git's fatals are sometimes two lines and the second one
/// is the half that says what to do: "…is a missing but already registered
/// worktree;" means nothing without "use 'add -f' to override, or 'prune' or
/// 'remove' to clear".
fn summarise(stderr: &str) -> String {
    let from_fatal = stderr.find("fatal:").map_or(stderr, |at| &stderr[at..]);

    let mut message = String::new();
    for line in from_fatal.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("hint:") {
            continue;
        }
        let line = line.strip_prefix("fatal:").unwrap_or(line).trim();
        if line.is_empty() {
            continue;
        }
        if !message.is_empty() {
            message.push(' ');
        }
        message.push_str(line);
    }

    if message.is_empty() {
        "git failed and gave no reason".to_owned()
    } else {
        message
    }
}

/// Where `branch` is checked out, if it is anywhere.
fn checked_out_at(repository: &Path, branch: &str) -> Option<PathBuf> {
    list(repository)
        .ok()?
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some(branch))
        .map(|worktree| worktree.path)
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
