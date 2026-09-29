//! What the agent in a tab changed, read from the repository it works in.
//!
//! The row's chip says `+214 −37` and nothing else, and it counts only what
//! is not committed yet — so the moment an agent commits, the one thing Crook
//! says about its work goes blank. Checking what an agent did against what it
//! says it did means leaving for another terminal and typing `git log` and
//! `git diff`. This is those questions asked on the person's behalf, for the
//! Changes column to show:
//!
//! * [`base`] — what the work is compared with: where the tab's branch left
//!   the repository's base branch ([`super::merged::base_of`]), the same
//!   merge-base a forge's pull request compares from;
//! * [`commits`] — what was committed since then, newest first;
//! * [`files`] — every file that differs from there, committed or not, and
//!   the files nobody has added yet, because an agent's work is in all three
//!   places at once;
//! * [`hunks`] — one file's diff, read when somebody asks to see that file
//!   and not before.
//!
//! # Read-only, and a repository's configuration runs nothing
//!
//! Everything here reads, through [`super::run`]'s deadline and its
//! lock-free flags, on the background pool. A diff is taken with
//! `--no-ext-diff` and `--no-textconv`: `diff.external` and a `textconv`
//! driver are programs named by configuration, and git runs them for every
//! file it diffs, so without the two flags a column that only looks would
//! start whatever a repository's `.git/config` named. `core.fsmonitor` is
//! switched off for the same reason — it is a program too. What is left is
//! git's own clean filter, which is how git reads a working tree at all (Git
//! LFS is one) and which the agent's own `git status` runs in the same
//! repository.
//!
//! The settings that change the *text* this reads are pinned too: colour,
//! `diff.relative`, the `a/` and `b/` a patch names its sides with, how a
//! blank context line is spelled, and a signature check on `log`. Each of
//! them either breaks what is parsed here or makes "Copy diff" hand on a
//! patch `git apply` refuses — `diff.noprefix` is the second. Settings that
//! change only which of several right answers git gives, `diff.algorithm`
//! say, are left as the person set them.
//!
//! # Bounded
//!
//! A file's diff is as long as the file, and a lockfile or a generated bundle
//! is megabytes of it: [`MAX_DIFF_BYTES`] and [`MAX_DIFF_LINES`] cut it, and
//! the cut says so. The lists are capped too — [`MAX_COMMITS`], [`MAX_FILES`]
//! — because a branch cut from the wrong place, or a build directory nobody
//! ignored, is thousands of entries a column has no use for.

use std::borrow::Cow;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::run::{Failure, Intent, run, run_capped, run_fed};
use super::worktree::path_from;

/// How many commits [`commits`] lists.
///
/// A task's branch is tens of commits. Hundreds is a branch that was cut from
/// somewhere other than its base — or a base that could not be found and a
/// history compared with nothing — and a column listing thousands of
/// somebody else's commits says less than one that stops and says there are
/// more.
pub const MAX_COMMITS: usize = 500;

/// How many files [`files`] lists.
///
/// Well past what a review is: the column is windowed, so five thousand rows
/// cost a screenful to draw, and the list is kept whole up to here. Past it
/// is a `node_modules` nobody ignored or a vendored tree, where the count is
/// the news and the names are not, and holding a quarter of a million paths
/// to re-read every fifteen seconds is not worth doing for it.
pub const MAX_FILES: usize = 20_000;

/// How many bytes of one file's diff [`hunks`] keeps.
///
/// Half a megabyte is thousands of lines of source, which is more than
/// anybody reads in a column beside a terminal. A diff longer than that is
/// generated — a lockfile, a bundle, a snapshot — and what a person needs to
/// know about it is that it changed and how to see it whole: the editor, or
/// `git diff`. Reading the rest only to drop it would hold a pool worker and
/// the memory for nothing, so git is stopped at the cap instead
/// ([`super::run::run_capped`]).
pub const MAX_DIFF_BYTES: usize = 512 * 1024;

/// How many lines of one file's diff [`hunks`] keeps.
///
/// The byte cap bounds memory; this bounds the column. Every line is a row a
/// person scrolls past, and three thousand of them is already a scrollbar
/// rather than a review. A file of short lines — numbers, a data table —
/// reaches this long before it reaches the byte cap.
pub const MAX_DIFF_LINES: usize = 3_000;

/// How many bytes of one line of a diff [`hunks`] keeps to draw.
///
/// The column is about fifty characters wide, and a line of text is shaped
/// whole before it is cut to fit — glyph by glyph, and again on every frame
/// the window draws while the line is on screen, which is every frame an
/// agent prints anything. One line of a minified bundle, of a notebook's
/// embedded picture or of a one-line JSON fixture is tens or hundreds of
/// kilobytes, and shaping that is tens of milliseconds a frame spent on a
/// column that shows its first fifty characters. Five times what fits still
/// ends in an ellipsis at the edge, and the whole line is still in
/// [`FileDiff::patch`], which is what "Copy diff" copies.
pub const MAX_LINE_BYTES: usize = 256;

/// The flags every diff here is taken with. See the module's own docs.
///
/// The prefixes are the ones `git apply` strips by default. Without them
/// `diff.noprefix` would print `--- src/x.rs`, which `apply` reads as a file
/// called `x.rs` in a directory called nothing.
const DIFF_FLAGS: [&str; 6] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "-M",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// What every call here puts before the command.
///
/// `--literal-pathspecs` because a path is handed to `diff` as a pathspec,
/// and a pathspec is a glob: a file called `page[1].html` would otherwise be
/// asked about as `page1.html`. `core.fsmonitor` names a program git runs to
/// ask what changed; off, git looks for itself. `diff.relative` would narrow
/// every diff to the directory it runs in, which is the top of the
/// repository here, but a setting is not a promise and the paths have to be
/// from the top. `diff.suppressBlankEmpty` prints a blank context line as
/// nothing at all rather than as one space, and a line that is neither added,
/// removed nor context is one the count to the first changed line skips.
const GLOBAL: [&str; 7] = [
    "--literal-pathspecs",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "diff.relative=false",
    "-c",
    "diff.suppressBlankEmpty=false",
];

/// Why a read could not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The directory is in no repository, or in one with no working tree.
    NotARepository,
    /// There is no `git` on `PATH`.
    GitMissing,
    /// git was still running at the deadline and was killed.
    TimedOut,
    /// Anything else, carrying what git said.
    Failed(String),
}

// Hand-written for the reason `worktree::Error`'s is.
impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotARepository => write!(formatter, "This tab is not in a git repository"),
            Self::GitMissing => write!(formatter, "git is not installed"),
            Self::TimedOut => write!(formatter, "git took too long to answer"),
            Self::Failed(message) => write!(formatter, "{message}"),
        }
    }
}

impl From<Failure> for Error {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::GitMissing => Self::GitMissing,
            Failure::CouldNotRun(error) => Self::Failed(format!("Could not run git: {error}")),
            Failure::TimedOut { .. } => Self::TimedOut,
            Failure::NoDirectory => Self::NotARepository,
        }
    }
}

/// What kind of thing the work is compared with.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Against {
    /// Where the branch left the repository's base branch.
    Branch,
    /// `HEAD` itself: the repository has no base branch to find, so what is
    /// shown is what is not committed yet.
    Head,
    /// Nothing: no commit has been made, and every file is new.
    Nothing,
}

/// What the work is compared with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Base {
    /// What to call it: `origin/main`, `main`, `HEAD`.
    pub name: String,
    /// The object the comparison starts from, in full: the merge-base, the
    /// commit `HEAD` is, or the empty tree.
    pub fork: String,
    /// Which of those it is.
    pub against: Against,
}

/// One commit on the task's branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    /// Its id, abbreviated the way git abbreviates it in this repository.
    pub sha: String,
    /// The first line of its message.
    pub subject: String,
    /// When it was made, the way `git log` says it: "2 hours ago".
    pub when: String,
}

/// The commits since the base.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commits {
    /// Newest first, at most [`MAX_COMMITS`].
    pub commits: Vec<Commit>,
    /// Whether there were more than that.
    pub more: bool,
}

/// How a file differs from the base.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// It is new.
    Added,
    /// Its contents changed.
    Modified,
    /// It is gone.
    Deleted,
    /// It moved, and [`FileChange::from`] says from where.
    Renamed,
    /// It was copied, and [`FileChange::from`] says from what.
    Copied,
    /// It became another kind of thing: a file became a link, say.
    TypeChanged,
    /// It is in the middle of a merge git could not finish.
    Unmerged,
    /// Nobody has added it yet.
    Untracked,
    /// A directory nobody has added that is a repository of its own — a
    /// clone, a worktree, a submodule never registered — which git lists
    /// whole and does not look inside, so there is no diff of it to show.
    Repository,
}

impl Status {
    /// The letter `git status` prints for it.
    pub fn letter(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::TypeChanged => 'T',
            Self::Unmerged => 'U',
            Self::Untracked | Self::Repository => '?',
        }
    }

    /// The status for the letter a `diff --raw` entry's header ends with.
    ///
    /// `X` and anything a later git invents read as modified, which is the
    /// one answer that is never wrong about there being a change.
    fn from_letter(letter: u8) -> Self {
        match letter {
            b'A' => Self::Added,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'T' => Self::TypeChanged,
            b'U' => Self::Unmerged,
            _ => Self::Modified,
        }
    }
}

/// One file that differs from the base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// How.
    pub status: Status,
    /// Where it is, from the top of the repository.
    pub path: PathBuf,
    /// Where it was, for a rename or a copy.
    pub from: Option<PathBuf>,
}

/// The files that differ from the base.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Files {
    /// In path order, at most [`MAX_FILES`].
    pub files: Vec<FileChange>,
    /// Whether there were more than that.
    pub more: bool,
}

/// One file's diff.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    /// The patch as git printed it, headers and all, up to the caps: what
    /// "Copy diff" copies, and what `git apply` would take when it is whole.
    /// Its lines are never shortened; [`Self::lines`]' are.
    pub patch: String,
    /// The lines of its hunks — `@@` headers, context, and the lines added
    /// and removed — which is what the column draws, each cut to
    /// [`MAX_LINE_BYTES`] and ended with an ellipsis where it was.
    pub lines: Vec<String>,
    /// Whether git called the file binary, in which case there are no lines.
    pub binary: bool,
    /// Whether the diff was longer than the caps and this is its beginning.
    pub cut: bool,
    /// The first changed line, numbered in the file as it is now: where an
    /// editor should open it. `None` when there is no such line — a binary
    /// file, a pure rename, a file emptied of everything.
    pub first_line: Option<u32>,
}

/// Everything the column shows about a repository, but the hunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overview {
    /// The top of the working tree the directory is in.
    pub repository: PathBuf,
    /// What the work is compared with.
    pub base: Base,
    /// What was committed since.
    pub commits: Commits,
    /// Every file that differs, [`Files::files`].
    pub files: Vec<FileChange>,
    /// Whether there were more files than [`MAX_FILES`].
    pub more_files: bool,
}

/// Everything about the repository `directory` is in, but the hunks.
///
/// **Blocking**: about eight subprocesses. Background executor only.
///
/// `directory` may be anywhere in the working tree; everything is asked from
/// its top, because `ls-files` answers relative to where it runs and a path
/// that meant something only from a pane's subdirectory would open nothing.
pub fn overview(directory: &Path) -> Result<Overview, Error> {
    let repository = super::discover(directory)
        .and_then(|layout| layout.work_tree)
        .ok_or(Error::NotARepository)?;
    let base = base(&repository)?;
    let commits = commits(&repository, &base)?;
    let files = files(&repository, &base)?;
    Ok(Overview {
        repository,
        base,
        commits,
        files: files.files,
        more_files: files.more,
    })
}

/// What `repository`'s work is compared with.
///
/// **Blocking**: up to six subprocesses. Background executor only.
///
/// The merge-base of `HEAD` and [`super::merged::base_of`]'s branch, which is
/// what a pull request compares from and what "what did this task do" means:
/// the base moving on since the branch left it is not the task's change, and
/// comparing with its tip would show that as undone. With no base branch to
/// find, or none that shares a history with `HEAD`, it is `HEAD` itself —
/// what is not committed yet — and before the first commit it is the empty
/// tree, against which every file is new.
pub fn base(repository: &Path) -> Result<Base, Error> {
    if let Some(reference) = super::merged::base_of(repository)
        && let Some(fork) = answer(repository, &["merge-base", &reference, "HEAD"])?
    {
        return Ok(Base {
            name: short_name(&reference).to_owned(),
            fork,
            against: Against::Branch,
        });
    }

    if let Some(head) = answer(repository, &["rev-parse", "--verify", "--quiet", "HEAD"])? {
        return Ok(Base {
            name: "HEAD".to_owned(),
            fork: head,
            against: Against::Head,
        });
    }

    // The empty tree's id depends on the repository's hash, so it is asked
    // for rather than spelled: hashing nothing, since `run` gives git no
    // stdin to read.
    let empty = answer(repository, &["hash-object", "-t", "tree", "--stdin"])?
        .ok_or_else(|| Error::Failed("git could not name the empty tree".to_owned()))?;
    Ok(Base {
        name: "nothing".to_owned(),
        fork: empty,
        against: Against::Nothing,
    })
}

/// A ref as a person says it: `origin/main` rather than
/// `refs/remotes/origin/main`.
fn short_name(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
}

/// The commits on `HEAD` since `base`, newest first.
///
/// **Blocking**: one subprocess. Background executor only.
pub fn commits(repository: &Path, base: &Base) -> Result<Commits, Error> {
    // Nothing to list against `HEAD` itself, and `HEAD` is nothing at all
    // before the first commit.
    if base.against != Against::Branch {
        return Ok(Commits::default());
    }

    let cap = format!("--max-count={}", MAX_COMMITS + 1);
    let range = format!("{}..HEAD", base.fork);
    // Fields apart by a unit separator and commits by a newline: a subject is
    // one line by definition, and the separator is the one byte nobody puts
    // in one on purpose. `--no-show-signature` because `log.showSignature`
    // runs gpg once per commit.
    let args = [
        "log",
        "--no-color",
        "--no-show-signature",
        "--format=%h%x1f%s%x1f%cr",
        &cap,
        &range,
        "--",
    ];
    let listed = read(repository, &args)?;

    let mut commits: Vec<Commit> = String::from_utf8_lossy(&listed)
        .lines()
        .filter_map(|line| {
            // The subject between the first separator and the last, so one
            // that holds a separator of its own is still the whole subject.
            let (sha, rest) = line.split_once('\u{1f}')?;
            let (subject, when) = rest.rsplit_once('\u{1f}')?;
            Some(Commit {
                sha: sha.to_owned(),
                subject: subject.to_owned(),
                when: when.to_owned(),
            })
        })
        .collect();
    let more = commits.len() > MAX_COMMITS;
    commits.truncate(MAX_COMMITS);
    Ok(Commits { commits, more })
}

/// Every file in `repository` that differs from `base`, and every file
/// nobody has added.
///
/// **Blocking**: two subprocesses, and a third when a file's stat data has
/// moved since the index last looked. Background executor only.
/// `repository` is the top of the working tree: see [`overview`].
///
/// The diff is `base` against the working tree, so what was committed, what
/// is staged and what is merely saved all appear, once each. Untracked files
/// are listed as git lists them for `status`: one per file, and none that an
/// ignore file covers — and a repository inside this one as itself, since
/// git does not look into one.
///
/// A file is listed when its contents or its mode differ, and not when only
/// its stat data does, text or binary. Reads here never refresh the index
/// (see [`super::run`]), so a file rewritten with the bytes it already held
/// — `touch`, a `sed -i` that matched nothing, a formatter or a generator
/// that changed nothing, `npm install` writing the same lockfile, a test run
/// writing the same snapshot picture — still has the old mtime in the index,
/// and `--raw` lists it as modified, with no id for what it holds now, until
/// something outside Crook refreshes the index. Such a file is hashed
/// (`unchanged`) and left out when it hashes to the id it had at the base,
/// which is what `git diff` itself shows once it has refreshed.
///
/// Nothing here diffs a file. A diff costs what pairing the file's lines
/// costs under whatever `diff.algorithm` somebody chose — twenty seconds for
/// one data file under `histogram`, measured, past the read deadline
/// — and it fails on a file it cannot read, so a list that waited on every
/// changed file's diff was a list that one file could take away whole.
/// Hashing reads each file once, and a file it cannot read stays listed.
pub fn files(repository: &Path, base: &Base) -> Result<Files, Error> {
    let mut args = vec!["diff", "--raw", "--no-abbrev", "-z"];
    args.extend(DIFF_FLAGS);
    args.extend([base.fork.as_str(), "--"]);
    let raw = read(repository, &args)?;
    let listed = raw_entries(&raw);
    let same = unchanged(repository, &listed);
    let mut files: Vec<FileChange> = listed
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !same.contains(index))
        .map(|(_, entry)| entry.change)
        .collect();

    let untracked = read(
        repository,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let tracked: HashSet<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
    files.extend(
        untracked
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| FileChange {
                // `ls-files` names a nested repository as a directory, with
                // the slash on the end, and never the files in it.
                status: if path.ends_with(b"/") {
                    Status::Repository
                } else {
                    Status::Untracked
                },
                path: path_from(path),
                from: None,
            })
            .filter(|file| !tracked.contains(&file.path)),
    );

    files.sort_by(|left, right| left.path.cmp(&right.path));
    let more = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    Ok(Files { files, more })
}

/// One entry of `diff --raw`.
struct Listed<'a> {
    /// The change it names.
    change: FileChange,
    /// For a file `--raw` calls modified without saying what it holds now:
    /// what [`unchanged`] hashes it against.
    unhashed: Option<Unhashed<'a>>,
}

/// A file whose stat data no longer matches the index, which git reports
/// as modified without having read it.
struct Unhashed<'a> {
    /// Its id at the base, in full.
    base_id: &'a [u8],
    /// Its path, as git printed it.
    path: &'a [u8],
}

/// `diff --raw --no-abbrev -z`, read.
///
/// One entry per file: a header — a colon, both modes, both ids, and the
/// status letter with a rename's or a copy's score after it — then the path,
/// or for a rename or a copy where it was and where it is, every one of them
/// ended by a NUL. A path is read by its place and never asked what it
/// begins with.
///
/// The working tree's id is all zeros when git has not read the file, which
/// against a commit is exactly when the index's stat data for it has gone
/// stale: a file whose stat data matches is one git takes the index's id
/// for. Only a modified regular file whose mode is what it was is left to be
/// hashed. A mode that changed is a change whatever the bytes are, every other
/// status says something happened, and `hash-object` reads what a link points
/// at rather than the link, and cannot read a submodule at all.
fn raw_entries(bytes: &[u8]) -> Vec<Listed<'_>> {
    let mut fields = bytes.split(|byte| *byte == 0);
    let mut listed = Vec::new();
    while let Some(header) = fields.next().and_then(|field| field.strip_prefix(b":")) {
        let parts: Vec<&[u8]> = header.split(|byte| *byte == b' ').collect();
        let &[old_mode, new_mode, base_id, now_id, letters] = parts.as_slice() else {
            break;
        };
        let letter = letters.first().copied().unwrap_or(b'M');
        let status = Status::from_letter(letter);
        let Some(first) = fields.next() else {
            break;
        };
        let (path, from) = match status {
            Status::Renamed | Status::Copied => {
                let Some(to) = fields.next() else {
                    break;
                };
                (to, Some(first))
            }
            _ => (first, None),
        };
        let unhashed = (letter == b'M'
            && old_mode == new_mode
            && matches!(old_mode, b"100644" | b"100755")
            && now_id.iter().all(|byte| *byte == b'0'))
        .then_some(Unhashed { base_id, path });
        listed.push(Listed {
            change: FileChange {
                status,
                path: path_from(path),
                from: from.map(path_from),
            },
            unhashed,
        });
    }
    listed
}

/// Which of `listed` hold in the working tree what they held at the base,
/// by their place in it.
///
/// **Blocking**: one subprocess, or none when nothing needs hashing.
///
/// `hash-object --stdin-paths` hashes each file the way `add` would, through
/// the repository's clean filter — so a checkout that converts line endings,
/// or keeps its pictures in LFS, hashes to what was committed — and writes
/// nothing. It answers in the order it was asked and stops at the first file
/// it cannot open, so the answers it gave are the first files', and the file
/// it stopped at and every one after are kept: a file nobody could read may
/// well have changed. A git that fails outright or runs out of time
/// confirms nothing, which lists every one of them, as `--raw` does.
fn unchanged(repository: &Path, listed: &[Listed<'_>]) -> HashSet<usize> {
    let asked: Vec<(usize, &Unhashed<'_>)> = listed
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.unhashed.as_ref().map(|file| (index, file)))
        .collect();
    if asked.is_empty() {
        return HashSet::new();
    }

    let mut input = Vec::new();
    for (_, file) in &asked {
        input.extend_from_slice(&quoted(file.path));
        input.push(b'\n');
    }
    let mut full: Vec<&OsStr> = GLOBAL.map(OsStr::new).to_vec();
    full.extend(["hash-object", "--stdin-paths"].map(OsStr::new));
    let hashed = match run_fed(repository, &full, input) {
        Ok(finished) => {
            if !finished.success {
                log::debug!(
                    "hash-object stopped in {}: {}",
                    repository.display(),
                    finished.stderr.trim()
                );
            }
            finished.stdout
        }
        Err(failure) => {
            log::debug!("hash-object in {}: {failure:?}", repository.display());
            return HashSet::new();
        }
    };

    hashed
        .split(|byte| *byte == b'\n')
        .zip(&asked)
        .filter(|(id, (_, file))| id.strip_suffix(b"\r").unwrap_or(id) == file.base_id)
        .map(|(_, (index, _))| *index)
        .collect()
}

/// `path` the way git prints one: in double quotes with C's escapes when it
/// holds a control character, a quote, a backslash or a byte past ASCII,
/// and as it is otherwise.
///
/// Which is also the one way `hash-object --stdin-paths` takes a name with
/// a newline in it, or one ending in a carriage return, as one line of its
/// input — and the way a patch's headers name such a file.
fn quoted(path: &[u8]) -> Cow<'_, [u8]> {
    let escaped = |byte: u8| !(0x20..0x7f).contains(&byte) || byte == b'"' || byte == b'\\';
    if !path.iter().any(|byte| escaped(*byte)) {
        return Cow::Borrowed(path);
    }
    let mut out = Vec::with_capacity(path.len() + 8);
    out.push(b'"');
    for &byte in path {
        match byte {
            b'"' | b'\\' => out.extend([b'\\', byte]),
            0x07 => out.extend(b"\\a"),
            0x08 => out.extend(b"\\b"),
            b'\t' => out.extend(b"\\t"),
            b'\n' => out.extend(b"\\n"),
            0x0b => out.extend(b"\\v"),
            0x0c => out.extend(b"\\f"),
            b'\r' => out.extend(b"\\r"),
            _ if escaped(byte) => out.extend(format!("\\{byte:03o}").bytes()),
            _ => out.push(byte),
        }
    }
    out.push(b'"');
    Cow::Owned(out)
}

/// `file`'s diff against `base`, up to [`MAX_DIFF_BYTES`] and
/// [`MAX_DIFF_LINES`].
///
/// **Blocking**: one subprocess. Background executor only. `repository` is
/// the top of the working tree.
///
/// A file nobody has added is diffed against nothing, which is every line of
/// it added; a rename is diffed with both its names, so git can pair them. A
/// nested repository has no diff here at all, and says so. A link nobody has
/// added that points at a directory is the one diff written here rather
/// than by git: see `new_link`.
pub fn hunks(repository: &Path, base: &Base, file: &FileChange) -> Result<FileDiff, Error> {
    let mut args: Vec<&OsStr> = ["diff", "-U3"].map(OsStr::new).to_vec();
    args.extend(DIFF_FLAGS.map(OsStr::new));
    match file.status {
        Status::Repository => {
            return Err(Error::Failed(
                "A repository of its own, which git does not look inside".to_owned(),
            ));
        }
        // `/dev/null` is git's own name for nothing in a `--no-index` diff,
        // on every platform — Windows's `nul` is accepted too, and this
        // spelling is the one that means the same thing everywhere.
        Status::Untracked => {
            let on_disk = repository.join(&file.path);
            let is_link = std::fs::symlink_metadata(&on_disk)
                .is_ok_and(|metadata| metadata.file_type().is_symlink());
            if is_link && on_disk.is_dir() {
                let target = std::fs::read_link(&on_disk)
                    .map_err(|error| Error::Failed(format!("Could not read the link: {error}")))?;
                let patch = new_link(&bytes_of(&file.path), &bytes_of(&target));
                return Ok(parse_diff(&patch, false));
            }
            args.extend(["--no-index", "--", "/dev/null"].map(OsStr::new));
            args.push(file.path.as_os_str());
        }
        _ => {
            args.extend([OsStr::new(&base.fork), OsStr::new("--")]);
            if let Some(from) = &file.from {
                args.push(from.as_os_str());
            }
            args.push(file.path.as_os_str());
        }
    }

    let mut full: Vec<&OsStr> = GLOBAL.map(OsStr::new).to_vec();
    full.extend(args);
    let read = run_capped(repository, &full, MAX_DIFF_BYTES)?;
    // A cut diff is one git was stopped from finishing on purpose.
    // `--no-index` exits 1 to say there were differences — and exits 1 as
    // well when it could not read the file at all, having printed nothing
    // but its complaint: an untracked file deleted since the list was read.
    // A new file always has a header to print, an empty one included, so an
    // exit of 1 with nothing on stdout is the failure.
    let answered = read.cut
        || match read.code {
            Some(0) => true,
            Some(1) => !read.stdout.is_empty(),
            _ => false,
        };
    if !answered {
        return Err(failed(&read.stderr));
    }

    Ok(parse_diff(&read.stdout, read.cut))
}

/// The diff of a new link at `path` to `target`, as git prints one.
///
/// `diff --no-index` cannot give it. It follows a link to a directory, takes
/// the link for that directory, and — asked to compare a directory with a
/// file — goes looking inside it for a file of the other side's name, which
/// against `/dev/null` is `link/null`, and fails on a path nobody made. A
/// link is a file whose one line is its target, which is how git diffs one
/// it can read, so this is that patch: the same headers, the target as the
/// lines added, and no `index` line, which `git apply` does not need and
/// which would take another git to hash.
fn new_link(path: &[u8], target: &[u8]) -> Vec<u8> {
    let old = quoted(&[b"a/", path].concat()).into_owned();
    let new = quoted(&[b"b/", path].concat()).into_owned();
    let mut patch = Vec::new();
    patch.extend(b"diff --git ");
    patch.extend(&old);
    patch.push(b' ');
    patch.extend(&new);
    patch.extend(b"\nnew file mode 120000\n--- /dev/null\n+++ ");
    patch.extend(&new);
    // git ends a name that holds a space with a tab on this line, so that a
    // `patch` reading to the end of the line knows where the name stops.
    if new.contains(&b' ') {
        patch.push(b'\t');
    }

    let ends_a_line = target.ends_with(b"\n");
    let lines: Vec<&[u8]> = target
        .strip_suffix(b"\n")
        .unwrap_or(target)
        .split(|byte| *byte == b'\n')
        .collect();
    match lines.len() {
        1 => patch.extend(b"\n@@ -0,0 +1 @@\n"),
        count => patch.extend(format!("\n@@ -0,0 +1,{count} @@\n").bytes()),
    }
    for line in lines {
        patch.push(b'+');
        patch.extend(line);
        patch.push(b'\n');
    }
    if !ends_a_line {
        patch.extend(b"\\ No newline at end of file\n");
    }
    patch
}

/// A path's bytes, as git would print them: the bytes themselves on Unix,
/// and UTF-8 elsewhere, which is what git for Windows speaks.
fn bytes_of(path: &Path) -> Cow<'_, [u8]> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        Cow::Borrowed(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        match path.to_string_lossy() {
            Cow::Borrowed(text) => Cow::Borrowed(text.as_bytes()),
            Cow::Owned(text) => Cow::Owned(text.into_bytes()),
        }
    }
}

/// A diff as git printed it, up to the caps, read into what the column draws.
fn parse_diff(bytes: &[u8], cut_by_bytes: bool) -> FileDiff {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let mut cut = cut_by_bytes;
    if cut {
        // The bytes stop wherever the cap fell, which is mid-line more often
        // than not, and half a line drawn as though it were the line is a
        // change nobody made.
        match text.rfind('\n') {
            Some(end) => text.truncate(end + 1),
            None => text.clear(),
        }
    }

    let mut lines = Vec::new();
    let mut binary = false;
    let mut in_hunks = false;
    let mut patch_end = text.len();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        // A file that became another kind of thing — a file replaced by a
        // link — is two patches in one diff, its deletion and then its
        // creation, and the second one's headers are no more changed lines
        // than the first one's. A line of a hunk starts with a space, a `+`,
        // a `-`, a `\` or `@@`, so this is only ever a header.
        if bare.starts_with("diff --git ") {
            in_hunks = false;
        }
        if !in_hunks {
            if bare.starts_with("Binary files ") && bare.ends_with(" differ") {
                binary = true;
            }
            in_hunks = bare.starts_with("@@");
        }
        if in_hunks {
            if lines.len() == MAX_DIFF_LINES {
                cut = true;
                patch_end = offset;
                break;
            }
            lines.push(drawn(bare));
        }
        offset += line.len();
    }
    text.truncate(patch_end);

    FileDiff {
        first_line: first_changed_line(&lines),
        patch: text,
        lines,
        binary,
        cut,
    }
}

/// One line of a hunk as the column keeps it: whole up to
/// [`MAX_LINE_BYTES`], and past that its beginning and an ellipsis, cut
/// where a character ends.
fn drawn(line: &str) -> String {
    if line.len() <= MAX_LINE_BYTES {
        return line.to_owned();
    }
    let mut end = MAX_LINE_BYTES;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\u{2026}", &line[..end])
}

/// The line an editor should open at: the first one added or removed,
/// numbered in the file as it is now.
///
/// Counted from the first hunk's header — `@@ -12,7 +14,8 @@` begins the new
/// file at line 14 — through its context lines. A removal has no line of its
/// own in the new file, so it is placed where the lines around it now meet.
fn first_changed_line(lines: &[String]) -> Option<u32> {
    let mut number: Option<u32> = None;
    for line in lines {
        if line.starts_with("@@") {
            number = new_start(line);
            continue;
        }
        let current = number?;
        match line.as_bytes().first() {
            Some(b'+' | b'-') => return Some(current.max(1)),
            // An empty line is a blank context line with its space dropped,
            // which `diff.suppressBlankEmpty` does and [`GLOBAL`] undoes —
            // counted either way, or the editor opens lines early.
            Some(b' ') | None => number = Some(current + 1),
            _ => {}
        }
    }
    None
}

/// Where a hunk header says the new file's side begins.
fn new_start(header: &str) -> Option<u32> {
    let plus = header
        .split_whitespace()
        .find(|part| part.starts_with('+'))?;
    let start = plus[1..].split(',').next()?;
    start.parse().ok()
}

/// What git printed for `args`, trimmed, when it succeeded and printed
/// anything at all — and `None` when it succeeded and printed nothing, or
/// failed: every question [`base`] asks has a "there is none" answer that git
/// gives by failing.
fn answer(repository: &Path, args: &[&str]) -> Result<Option<String>, Error> {
    let mut full: Vec<&OsStr> = GLOBAL.map(OsStr::new).to_vec();
    full.extend(args.iter().map(OsStr::new));
    let finished = run(repository, &full, Intent::Read)?;
    if !finished.success {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&finished.stdout).trim().to_owned();
    Ok((!text.is_empty()).then_some(text))
}

/// What git printed for `args`, or why it could not say.
fn read(repository: &Path, args: &[&str]) -> Result<Vec<u8>, Error> {
    let mut full: Vec<&OsStr> = GLOBAL.map(OsStr::new).to_vec();
    full.extend(args.iter().map(OsStr::new));
    let finished = run(repository, &full, Intent::Read)?;
    if !finished.success {
        return Err(failed(&finished.stderr));
    }
    Ok(finished.stdout)
}

/// git's complaint, as one line an error can carry.
fn failed(stderr: &str) -> Error {
    let first = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git could not answer");
    let message = first.strip_prefix("fatal: ").unwrap_or(first);
    Error::Failed(message.to_owned())
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
