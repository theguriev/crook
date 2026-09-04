//! Every checkout a repository has, and the two commands that make and unmake
//! one.
//!
//! A tab in Crook is an agent working somewhere, and two agents working in the
//! same directory fight over the same files. A worktree is git's answer to
//! that: one repository, several checkouts, each on its own branch. Giving a
//! new session its own checkout is two commands; this module is those two, plus
//! the reading that makes them safe to offer.
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
//! The second is that two of them write. `diff` documents that it has no
//! timeout and that a hung git parks a pool worker for the life of the
//! process; a write can hang for reasons a read cannot, because [`add`]
//! materialises a working tree and then runs the repository's own
//! `post-checkout` hook, which is arbitrary code somebody else wrote. So
//! everything here runs under a deadline and is killed at it.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::process::command;

/// Set the first time git turns out not to be on `PATH`.
///
/// The same latch [`super::diff`] keeps, for the same reason: without it a
/// machine with no git spawns a doomed process on every attempt for the life of
/// the session. The two are read together and set separately — whichever half
/// discovers it first spares the other — and deliberately not shared, because
/// sharing would mean one module exporting a setter the other calls, and this
/// module is worth keeping self-contained.
static GIT_MISSING: AtomicBool = AtomicBool::new(false);

/// How long a read may take before git is killed.
///
/// A timeout is not a performance budget; it is the thing that stops a wedged
/// git from holding a pool worker until Crook exits. It has to be far longer
/// than any honest run — listing a hundred checkouts is milliseconds, and the
/// only reason a local read is slow is a cold page cache — and short enough
/// that a stall is something a person waits out rather than a hang they have to
/// restart the app to clear.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long [`add`] and [`remove`] may take.
///
/// Twelve times the read budget, for one reason: `add` writes out a whole
/// working tree. On a large repository on a slow disk that is genuinely tens of
/// seconds, and then it runs the repository's `post-checkout` hook, which is
/// whatever the user wrote. Killing an honest checkout halfway leaves a
/// half-written directory *and* a registered worktree — strictly worse than
/// having waited.
const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

/// How long the deadline is checked at [`EAGER_POLL`] before it drops to
/// [`LAZY_POLL`].
///
/// git answers a local question in 5-50ms, so the common case is over before
/// the interval widens and pays no measurable latency for the poll; a call that
/// is already past fifty milliseconds is not made faster by asking it a
/// thousand times a second.
const EAGER_POLLING: Duration = Duration::from_millis(50);
/// The poll interval while a call is still young.
const EAGER_POLL: Duration = Duration::from_millis(1);
/// The poll interval once it is not.
const LAZY_POLL: Duration = Duration::from_millis(20);

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
    /// Crook's own agent worktrees are locked with a reason naming the session
    /// that holds them, which is what stops one agent tidying away another's.
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
}

impl Local {
    /// Whether the worktree holds nothing loose at all — nothing to warn
    /// about, nothing to lose, nothing to say.
    pub fn is_empty(self) -> bool {
        self.modified == 0 && self.untracked == 0 && self.ignored == 0
    }

    /// Whether a plain [`remove`] would refuse.
    ///
    /// Ignored files are deliberately left out: git removes over them without
    /// complaining, and counting them here would have a UI offering to force
    /// past an objection git was never going to raise — which is exactly the
    /// crying wolf that makes people stop reading warnings.
    pub fn blocks_removal(self) -> bool {
        self.modified > 0 || self.untracked > 0
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
    /// `path` is locked. `reason` is git's own, empty when the lock has none.
    /// [`remove`] never overrides one; see its documentation for why.
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

// MARK: - Reading

/// Whether git has already been found to be missing from this machine.
///
/// Reads [`super::diff`]'s latch too: a session that has already discovered
/// there is no git while drawing a tab row need not discover it again here.
pub fn git_is_missing() -> bool {
    GIT_MISSING.load(Ordering::Relaxed) || super::diff::git_is_missing()
}

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
    Ok(Local::parse_status(&finished.stdout))
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
pub fn add(repository: &Path, path: &Path, branch: &str, base: Option<&str>) -> Result<(), Error> {
    let finished = run(
        repository,
        &[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("-b"),
            OsStr::new(branch),
            // So a path beginning with a dash is a path.
            OsStr::new("--"),
            path.as_os_str(),
            OsStr::new(base.unwrap_or("HEAD")),
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
/// [`Error::Locked`] whichever way this is called. Crook's own worktrees are
/// locked by the session that owns them, and "discard my uncommitted work" must
/// never quietly also mean "take the checkout another agent is working in".
pub fn remove(repository: &Path, path: &Path, force: bool) -> Result<(), Error> {
    let mut args = vec![OsStr::new("worktree"), OsStr::new("remove")];
    if force {
        args.push(OsStr::new("--force"));
    }
    args.push(OsStr::new("--"));
    args.push(path.as_os_str());

    let finished = run(repository, &args, Intent::Write)?;
    if finished.success {
        return Ok(());
    }
    Err(classify(&finished.stderr))
}

// MARK: - Naming the next one

/// A branch name no worktree in `existing` is using.
///
/// Shaped like herdr's: `worktree/<adjective>-<noun>-<four hex digits>`. Short
/// enough to fit a tab row, lowercase and hyphenated so it is a legal ref and a
/// legal path component, and prefixed so that every branch Crook invented is
/// one `git branch --list 'worktree/*'` away from being found again.
///
/// Deterministic, and not seeded by the clock: the same set of worktrees always
/// suggests the same name. That makes the function testable, makes a retry
/// after a failed `add` predictable, and costs nothing — the vocabulary is
/// walked in order and the first free name wins, so collisions are avoided
/// outright instead of being made unlikely.
pub fn suggested_branch(existing: &[Worktree]) -> String {
    let taken: HashSet<&str> = existing
        .iter()
        .filter_map(|worktree| worktree.branch.as_deref())
        .collect();

    (0u32..)
        .map(candidate_branch)
        .find(|name| !taken.contains(name.as_str()))
        .expect("four billion candidate names cannot all be taken")
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
    format!("worktree/{adjective}-{noun}-{tag:04x}")
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
/// else a single `-`, no leading or trailing dashes, never empty.
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
fn path_from(bytes: &[u8]) -> PathBuf {
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

// MARK: - Running git

/// Whether an invocation only reads.
///
/// One enum for two decisions, because they are the same decision: how long a
/// call may take, and whether it is allowed to take git's locks.
#[derive(Copy, Clone)]
enum Intent {
    /// `worktree list`, `status`: answers a question and changes nothing.
    Read,
    /// `worktree add`, `worktree remove`: writes the repository.
    Write,
}

impl Intent {
    /// The deadline this kind of call gets.
    fn timeout(self) -> Duration {
        match self {
            Self::Read => READ_TIMEOUT,
            Self::Write => WRITE_TIMEOUT,
        }
    }
}

/// What one git invocation produced.
struct Finished {
    /// Whether git exited zero.
    success: bool,
    /// stdout, as bytes, because paths come out of it.
    stdout: Vec<u8>,
    /// stderr, as text, because messages come out of it.
    stderr: String,
}

/// Runs `git <args>` in `directory` and waits for it under [`Intent`]'s
/// deadline.
fn run(directory: &Path, args: &[&OsStr], intent: Intent) -> Result<Finished, Error> {
    if git_is_missing() {
        return Err(Error::GitMissing);
    }
    // A `current_dir` that does not exist makes `spawn` fail with `NotFound` —
    // the same error kind as a missing `git` binary. Checking first is what
    // keeps the latch below honest: without it, one call against a directory
    // somebody had just deleted would switch git off for the rest of the
    // session.
    if !directory.is_dir() {
        return Err(Error::NotARepository);
    }

    let mut git = command("git");

    if matches!(intent, Intent::Read) {
        // `diff`'s treatment, for `diff`'s reason: a background read must not
        // take `.git/index.lock`, or it races the user's own commit, and must
        // not rewrite the index as a side effect of refreshing it. The
        // environment variable carries the same rule into anything git itself
        // spawns.
        //
        // A write gets neither, on purpose. `add` and `remove` *must* take that
        // lock: it is how git stops two writers interleaving, and a write that
        // skipped it would corrupt the thing the flag exists to protect.
        git.arg("--no-optional-locks")
            .args(["-c", "diff.autoRefreshIndex=false"])
            .env("GIT_OPTIONAL_LOCKS", "0");
    }

    git.args(args)
        .current_dir(directory)
        // No command here touches a remote — `add` is handed an explicit branch
        // and start point precisely so nothing can decide to go and fetch one —
        // so there is no credential to be asked for. These two make that a
        // guarantee rather than an argument: git may not prompt on a terminal,
        // and has no terminal on stdin to prompt on.
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match git.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !GIT_MISSING.swap(true, Ordering::Relaxed) {
                log::warn!("git is not on PATH; worktrees are off for this session");
            }
            return Err(Error::GitMissing);
        }
        Err(error) => return Err(Error::CouldNotRun(error)),
    };

    // Both pipes are drained on their own threads. Polling `try_wait` with the
    // output left unread deadlocks the moment git writes more than a pipe
    // buffer — `status --ignored` in a repository with a fat `target/` does
    // exactly that — and the deadlock would then surface as a timeout, which is
    // a lie about what happened.
    let stdout = child.stdout.take().map(drain);
    let stderr = child.stderr.take().map(drain);

    let waited = wait_for(&mut child, intent.timeout());

    // Joined either way: a kill closes the pipes, so a reader on a killed child
    // reaches end of file rather than blocking, and joining is what makes sure
    // no thread outlives the call that made it.
    let stdout = stdout.map(collect).unwrap_or_default();
    let stderr = stderr.map(collect).unwrap_or_default();

    let status = waited?;
    if !status.success() {
        log::debug!(
            "git {args:?} in {} exited {:?}",
            directory.display(),
            status.code()
        );
    }

    Ok(Finished {
        success: status.success(),
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Reads a pipe to the end on a thread of its own.
fn drain<R: std::io::Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // A read that fails keeps whatever arrived before it. There is nothing
        // better to do with the error: the exit status is what decides whether
        // the output is worth trusting.
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

/// The bytes a [`drain`] thread collected, or none if it panicked.
fn collect(handle: std::thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.join().unwrap_or_default()
}

/// Waits for `child`, killing it if it outlives `timeout`.
fn wait_for(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, Error> {
    let started = Instant::now();
    let deadline = started + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => return Err(Error::CouldNotRun(error)),
        }

        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            // Killing closes both pipes, which is what lets the draining
            // threads finish. Reaping afterwards matters as much: a process id
            // that is never waited for stays a zombie, and one that is reaped
            // by somebody else can be handed out again to a stranger.
            let _ = child.kill();
            let _ = child.wait();
            log::warn!(
                "git did not finish within {}s and was killed",
                timeout.as_secs()
            );
            return Err(Error::TimedOut { after: timeout });
        }

        let interval = if started.elapsed() < EAGER_POLLING {
            EAGER_POLL
        } else {
            LAZY_POLL
        };
        std::thread::sleep(interval.min(left));
    }
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
