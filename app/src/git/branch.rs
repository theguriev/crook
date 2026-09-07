//! Which branch a directory is on, read out of `.git` rather than out of git.
//!
//! A tab row wants a branch name on every cwd change, and a subprocess per row
//! per change is not a thing a tab bar can afford. It does not need one: the
//! name is a string in a file, and finding the file is a walk up the directory
//! tree. So this module spawns nothing, depends on nothing beyond `std`, and
//! costs a handful of syscalls — which is what makes it safe to call whenever
//! the directory moves, and what lets a branch keep showing on a machine that
//! has no `git` binary at all.
//!
//! The four layouts below are not exotic. A linked worktree and a submodule
//! both replace `.git` with a *file* pointing elsewhere, and a submodule's
//! pointer is relative to the file that holds it — get that join wrong and
//! every submodule in the workspace silently loses its branch.

use std::path::{Component, Path, PathBuf};

/// Where a repository's parts live, once one has been found.
///
/// The three paths are distinct in a linked worktree and a submodule, and only
/// there: in an ordinary checkout all three collapse onto `<root>` and
/// `<root>/.git`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoLayout {
    /// The directory the files are checked out into. `None` for a bare
    /// repository, which is also why diff stats are skipped for one — there is
    /// no working tree to diff.
    pub work_tree: Option<PathBuf>,
    /// Where `HEAD` lives. For a linked worktree this is
    /// `<main>/.git/worktrees/<name>`; for a submodule,
    /// `<super>/.git/modules/<name>`. Never assume `work_tree/.git`.
    pub git_dir: PathBuf,
    /// The `git_dir` shared by every worktree of the repository — where
    /// `packed-refs` and the object store live. Equal to `git_dir` except in a
    /// linked worktree.
    pub common_dir: PathBuf,
}

impl RepoLayout {
    /// Whether this is a linked worktree — one `git worktree add` made —
    /// rather than the checkout the repository was cloned into.
    ///
    /// The two git directories are the whole of the answer: a linked worktree
    /// keeps its own `HEAD` under `<main>/.git/worktrees/<name>` and shares
    /// everything else, so its `git_dir` is not its `common_dir`. A submodule
    /// has a `git_dir` somewhere unexpected too, and is *not* a worktree: its
    /// `common_dir` is that same directory, because nothing is shared with a
    /// checkout elsewhere. See [`common_dir_of`], which is where both of those
    /// answers come from.
    pub fn is_linked_worktree(&self) -> bool {
        self.git_dir != self.common_dir
    }
}

/// What `HEAD` points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// A branch. The name is everything after `refs/heads/`, slashes and all —
    /// `eugen/claude-code-usage-indicator` is one branch, not two.
    Branch(String),
    /// A commit, checked out directly.
    Detached {
        /// The first seven characters, which is what a row shows.
        short: String,
        /// The whole object id, for anything that later wants to resolve it.
        full: String,
    },
}

impl Head {
    /// The text a row shows for this head.
    ///
    /// A detached head reads as a bare short sha, exactly as git itself
    /// abbreviates one — and so is indistinguishable in the row from a branch
    /// whose name happens to be seven hex characters. Warp has the same
    /// ambiguity and it has never mattered.
    pub fn label(&self) -> &str {
        match self {
            Self::Branch(name) => name,
            Self::Detached { short, .. } => short,
        }
    }

    /// Whether `HEAD` is a commit rather than a branch.
    ///
    /// Worth asking before anything that is meaningless without a branch to
    /// name — a pull-request lookup, an upstream comparison.
    pub fn is_detached(&self) -> bool {
        matches!(self, Self::Detached { .. })
    }
}

/// Finds the repository `start` sits in, by walking up from it.
///
/// Returns `None` when there is no repository above `start`, which is an
/// ordinary answer and not a failure: most directories are not in one.
///
/// The walk stops *before* the home directory, so a dotfiles repository
/// checked out at `~` does not claim every directory beneath it. A repository
/// anywhere under `~` is found normally.
pub fn discover(start: &Path) -> Option<RepoLayout> {
    // Relative paths would make the parent walk terminate on the empty path
    // after one step, finding at most the process's own cwd.
    let start = std::path::absolute(start).ok()?;
    let home = std::env::home_dir();

    let mut current = start.as_path();
    loop {
        if home.as_deref() == Some(current) {
            return None;
        }
        if let Some(layout) = layout_at(current) {
            return Some(layout);
        }
        current = current.parent()?;
    }
}

/// Reads `<git_dir>/HEAD`.
///
/// `None` when the file is missing, unreadable, or holds something that is
/// neither of the two legal forms — a row then shows no branch, rather than a
/// branch literally named `ref: refs/heads/main`.
///
/// `packed-refs` is deliberately not consulted. It maps refs to object ids;
/// `HEAD` itself is never packed, so a freshly cloned or `git gc`-ed
/// repository reads through this function identically to any other.
pub fn read_head(git_dir: &Path) -> Option<Head> {
    let contents = match std::fs::read_to_string(git_dir.join("HEAD")) {
        Ok(contents) => contents,
        Err(error) => {
            log::debug!("cannot read {}/HEAD: {error}", git_dir.display());
            return None;
        }
    };
    parse_head(&contents)
}

/// Parses the one line `HEAD` holds.
fn parse_head(contents: &str) -> Option<Head> {
    let head = contents.trim();

    if let Some(reference) = head.strip_prefix("ref:") {
        let reference = reference.trim();
        // A symbolic HEAD can legally point outside refs/heads — at a remote
        // ref, or at a tag. Rare, but showing the last segment beats showing
        // nothing, and neither case is worth a branch in the type.
        let name = reference
            .strip_prefix("refs/heads/")
            .unwrap_or_else(|| reference.rsplit('/').next().unwrap_or(reference));
        return (!name.is_empty()).then(|| Head::Branch(name.to_owned()));
    }

    // A detached head is a bare object id: 40 hex characters under SHA-1, 64
    // under SHA-256.
    let is_object_id =
        matches!(head.len(), 40 | 64) && head.bytes().all(|byte| byte.is_ascii_hexdigit());
    is_object_id.then(|| Head::Detached {
        short: head[..7].to_owned(),
        full: head.to_owned(),
    })
}

/// Recognises a repository sitting exactly at `current`, in the three forms it
/// can take. `None` means "keep walking", including when `.git` is there but
/// points at something that is not a git directory.
fn layout_at(current: &Path) -> Option<RepoLayout> {
    // A bare repository is the directory itself, conventionally named `<x>.git`.
    // The convention is all there is to go on, so it is what is checked.
    let named_like_a_git_dir = current
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".git"));
    if named_like_a_git_dir && is_git_dir(current) {
        return Some(layout(None, current.to_path_buf()));
    }

    let dot_git = current.join(".git");
    // `metadata` rather than `symlink_metadata`: a `.git` symlinked to a
    // directory elsewhere is a real setup, and following the link reads it
    // correctly while refusing to follow it would treat the link as a pointer
    // file and fail to parse it.
    let metadata = std::fs::metadata(&dot_git).ok()?;

    if metadata.is_dir() {
        return is_git_dir(&dot_git).then(|| layout(Some(current.to_path_buf()), dot_git));
    }

    if metadata.is_file() {
        let git_dir = read_git_dir_pointer(&dot_git, current)?;
        return Some(layout(Some(current.to_path_buf()), git_dir));
    }

    None
}

/// Builds the layout, filling in the shared directory.
fn layout(work_tree: Option<PathBuf>, git_dir: PathBuf) -> RepoLayout {
    RepoLayout {
        common_dir: common_dir_of(&git_dir),
        work_tree,
        git_dir,
    }
}

/// Whether `dir` is a git directory, by the same test git uses on the cheap
/// path: it holds a `HEAD`.
fn is_git_dir(dir: &Path) -> bool {
    dir.join("HEAD").is_file()
}

/// Reads the `gitdir: <path>` line a linked worktree and a submodule put in
/// place of a `.git` directory.
///
/// `base` is the directory holding the file, and a relative pointer resolves
/// against *it* — not against the process's cwd. Submodules almost always
/// write a relative pointer, so resolving against the cwd works from the
/// repository root and breaks from everywhere else.
fn read_git_dir_pointer(file: &Path, base: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(file).ok()?;
    let pointer = contents.trim().strip_prefix("gitdir:")?.trim();
    if pointer.is_empty() {
        return None;
    }

    let git_dir = resolve_against(base, Path::new(pointer));
    is_git_dir(&git_dir).then_some(git_dir)
}

/// The `git_dir` every worktree of this repository shares.
///
/// A linked worktree records it explicitly in a `commondir` file; failing
/// that, a `git_dir` of the form `<main>/.git/worktrees/<name>` gives it away.
/// Everything else — an ordinary checkout, a bare repository, a submodule,
/// which owns its refs — is its own common directory.
fn common_dir_of(git_dir: &Path) -> PathBuf {
    if let Ok(contents) = std::fs::read_to_string(git_dir.join("commondir")) {
        let pointer = contents.trim();
        if !pointer.is_empty() {
            return resolve_against(git_dir, Path::new(pointer));
        }
    }

    let parent = git_dir.parent();
    if parent
        .and_then(Path::file_name)
        .is_some_and(|name| name == "worktrees")
        && let Some(main) = parent.and_then(Path::parent)
    {
        return main.to_path_buf();
    }

    git_dir.to_path_buf()
}

/// Resolves `path` against `base` when it is relative, and normalises the
/// result so a stored `../..` does not survive into a path the UI or a test
/// compares.
fn resolve_against(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    normalize(&base.join(path))
}

/// Removes `.` and `..` components lexically.
///
/// Sound here and only here: every path this is handed is an existing
/// directory joined with a pointer git wrote, so no `..` is being asked to
/// cross a symlink that has not already been resolved.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                // `/..` is `/`; a leading `..` in a relative path has nothing
                // to cancel and has to stay.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => normalized.push(component),
            },
            other => normalized.push(other),
        }
    }
    normalized
}

/// How deep the walk under `refs/heads` goes.
///
/// A branch name may hold slashes — `eugen/claude-code-usage-indicator` is one
/// name — and each of them is a directory on disk. Eight is far past any
/// naming scheme anybody uses and shallow enough that a `refs` directory
/// somebody has done something strange to cannot be a walk with no end.
const REF_DEPTH: usize = 8;

/// How many branches are read before the walk stops.
///
/// A bound rather than a judgement about repositories: this list is handed to
/// a picker and, for a sandboxed plugin, copied into its memory. A repository
/// with forty thousand refs in it should cost a long list rather than a
/// megabyte in somebody else's linear memory.
const REF_LIMIT: usize = 4096;

/// Every branch the repository has, in name order.
///
/// Read out of the files, like [`read_head`], and for the same reason: this is
/// asked for whenever a menu opens, and `git branch --list` is a subprocess.
/// Both places git keeps a ref are read, because a repository that has been
/// packed keeps most of its branches in one file and a freshly made one keeps
/// them all as loose files — a reader that knew about only one of the two
/// would work perfectly until the day `git gc` ran.
pub fn branches(layout: &RepoLayout) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    loose_branches(
        &layout.common_dir.join("refs").join("heads"),
        &mut String::new(),
        0,
        &mut found,
    );
    packed_branches(&layout.common_dir.join("packed-refs"), &mut found);

    found.sort();
    found.dedup();
    found
}

/// Walks `refs/heads`, appending every name under it.
///
/// `prefix` is what has been walked past, which is what makes a branch in a
/// directory come back as `eugen/thing` rather than as `thing`.
fn loose_branches(directory: &Path, prefix: &mut String, depth: usize, found: &mut Vec<String>) {
    if depth > REF_DEPTH || found.len() >= REF_LIMIT {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        // No loose refs is an ordinary state — a packed repository has none —
        // and so is no repository at all here.
        return;
    };

    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            // A ref whose name is not UTF-8 is not a ref git made.
            continue;
        };
        let length = prefix.len();
        prefix.push_str(&name);
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                prefix.push('/');
                loose_branches(&entry.path(), prefix, depth + 1, found);
            }
            Ok(_) => found.push(prefix.clone()),
            Err(_) => {}
        }
        prefix.truncate(length);
    }
}

/// Reads the branches out of `packed-refs`.
///
/// One line per ref, `<object id> <ref>`, with `#` for the header and `^` for
/// the commit a tag points at — neither of which is a branch.
fn packed_branches(file: &Path, found: &mut Vec<String>) {
    let Ok(contents) = std::fs::read_to_string(file) else {
        return;
    };

    for line in contents.lines() {
        if found.len() >= REF_LIMIT {
            return;
        }
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let Some((_, reference)) = line.split_once(' ') else {
            continue;
        };
        if let Some(name) = reference.trim().strip_prefix("refs/heads/")
            && !name.is_empty()
        {
            found.push(name.to_owned());
        }
    }
}
