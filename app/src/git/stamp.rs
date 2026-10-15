//! Whether a working tree can have changed since a gather last read it, told
//! without asking git to read it.
//!
//! The row's two diffs read the working tree, and what makes that dear is not
//! the counting. It is git making sure of every tracked file first: a file
//! whose size and times match what the index recorded is taken as unchanged,
//! but one written in the same second the index was is *racily clean* — the
//! times cannot tell it from a file edited a moment later — so git reads it
//! whole and hashes it, every time, until something writes the index again.
//! Crook never does: its reads take no lock, so they refresh nothing. A
//! checkout `git worktree add` has just made is every file of it written in
//! the second its index was, and `diff --shortstat HEAD` there hashed the
//! whole tree on each cycle — a quarter of a second on Crook's own checkout of
//! 450 files on a busy machine — and the count since the base hashed it again.
//!
//! A [`Stamp`] asks the question the other way round. Not whether a file
//! differs from the index, but whether anything about it has changed since the
//! last stamp: its size and its modification time, and on Unix its inode and
//! change time, which nothing short of the kernel can set back — so a tool
//! that restores a modification time after writing still moves it. Two stamps
//! that agree mean nothing tracked and not the index was written between them,
//! and the diffs read after the first still stand. A stamp costs a list of
//! paths out of the index and an `lstat` apiece, and never a byte of any file.
//!
//! In a sparse checkout the paths are only the ones checked out. Its index
//! lists every file of the repository, and the diffs pass over the ones it
//! marks *skip-worktree* — the files left out — without looking at them; a
//! stamp that went through those would cost what the whole tree costs, a
//! monorepo's millions of paths printed and as many `lstat`s finding nothing,
//! every cycle, for a checkout of a few thousand files. So it passes over them
//! too. A file somebody writes where a left-out one would be is one git stops
//! skipping as soon as it reads the index, and the list then has it like any
//! other. A *sparse index*, which keeps a left-out directory as one entry, is
//! listed without being expanded to every file in it: git calls that a slow
//! operation, and the diffs do not do it either.
//!
//! Two things a stamp cannot vouch for, and it says so by not being made:
//!
//! - **A file written moments ago.** A file system keeps its times to some
//!   grain — a jiffy on Linux, a second on some, two on FAT — and a file
//!   written twice inside one grain, the same length both times, looks the
//!   same both times. So a stamp taken while anything it covers is younger
//!   than [`SETTLING`] is no stamp, and the next gather reads again.
//! - **A submodule.** What git counts for one is its own `HEAD` and whether
//!   anything inside it is dirty, and neither shows on the directory it is
//!   checked out in.
//!
//! What it does not see at all is configuration. A `core.autocrlf` or a clean
//! filter changed over an unchanged tree leaves the last count standing until
//! something in the tree is written.

use std::collections::hash_map::DefaultHasher;
use std::ffi::OsStr;
use std::fs::Metadata;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::run::{Intent, run};

/// How long ago everything a stamp covers must last have been written for the
/// stamp to stand: longer than the coarsest grain a file system in use keeps
/// its times to, which is FAT's two seconds.
const SETTLING: Duration = Duration::from_secs(3);

/// The index and every file it tracks, as they stood, folded into one number.
///
/// Only ever compared with another: two that are equal say that nothing they
/// cover was written in between. Equal by accident is a 64-bit hash
/// colliding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stamp(u64);

/// The stamp of `work_tree`, whose git directory is `git_dir`, or `None` when
/// one cannot vouch for it: something in it is younger than [`SETTLING`],
/// it has a submodule, or git could not list what it tracks.
///
/// **Blocking**: one `git ls-files`, and an `lstat` for every file it names as
/// checked out.
pub(super) fn stamp(work_tree: &Path, git_dir: &Path) -> Option<Stamp> {
    // Before anything is looked at: a file written after this has a time this
    // late or later, and is what the age below is measured against.
    let taken = SystemTime::now();
    let mut stamp = Stamping {
        hasher: DefaultHasher::new(),
        newest: SystemTime::UNIX_EPOCH,
    };

    // The index before the list read out of it, so that an index written in
    // between is one this stamp holds the older time of, and the next differs.
    let index = std::fs::symlink_metadata(git_dir.join("index")).ok();
    stamp.note(index.as_ref())?;

    // `-t` tags each path with what the index says of it, which is how the
    // files a sparse checkout leaves out are told from the rest.
    let mut args = vec![OsStr::new("ls-files"), OsStr::new("-z"), OsStr::new("-t")];
    // `--sparse` lists a sparse index's left-out directories as they are,
    // rather than expanding each into every file in it. Only where there has
    // been a sparse checkout, which the file of its patterns says: a git older
    // than 2.35 refuses the flag, and the stamp with it.
    if git_dir.join("info").join("sparse-checkout").is_file() {
        args.push(OsStr::new("--sparse"));
    }
    let listed = run(work_tree, &args, Intent::Poll)
        .ok()
        .filter(|finished| finished.success)?;
    for path in checked_out(&listed.stdout)? {
        let path = path_from(path)?;
        let metadata = std::fs::symlink_metadata(work_tree.join(&path)).ok();
        if metadata.as_ref().is_some_and(Metadata::is_dir) {
            return None;
        }
        path.hash(&mut stamp.hasher);
        stamp.note(metadata.as_ref())?;
    }

    let settled = taken
        .duration_since(stamp.newest)
        .is_ok_and(|age| age >= settling());
    settled.then(|| Stamp(stamp.hasher.finish()))
}

/// A stamp being made.
struct Stamping {
    hasher: DefaultHasher,
    /// The latest time anything noted so far was written.
    newest: SystemTime,
}

impl Stamping {
    /// Folds in one file's metadata, or its absence — a tracked file deleted
    /// is a change like any other. `None` when the platform keeps no
    /// modification time to fold in.
    fn note(&mut self, metadata: Option<&Metadata>) -> Option<()> {
        let Some(metadata) = metadata else {
            false.hash(&mut self.hasher);
            return Some(());
        };
        true.hash(&mut self.hasher);
        metadata.len().hash(&mut self.hasher);
        metadata.file_type().is_symlink().hash(&mut self.hasher);
        let modified = metadata.modified().ok()?;
        modified.hash(&mut self.hasher);
        self.newest = self.newest.max(modified);

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            (metadata.ino(), metadata.mode()).hash(&mut self.hasher);
            (metadata.ctime(), metadata.ctime_nsec()).hash(&mut self.hasher);
            if let (Ok(seconds), Ok(nanos)) = (
                u64::try_from(metadata.ctime()),
                u32::try_from(metadata.ctime_nsec()),
            ) {
                let changed = SystemTime::UNIX_EPOCH + Duration::new(seconds, nanos);
                self.newest = self.newest.max(changed);
            }
        }
        Some(())
    }
}

/// The paths `ls-files -z -t` listed as checked out: every one but those
/// tagged `S`, *skip-worktree*, which a sparse checkout leaves out and the
/// diffs never look at. That includes a sparse index's left-out directory,
/// which `--sparse` lists as one entry so tagged. `None` for a listing that
/// is not tag, space, path.
pub(super) fn checked_out(listed: &[u8]) -> Option<Vec<&[u8]>> {
    let mut paths = Vec::new();
    for entry in listed.split(|byte| *byte == 0) {
        match entry {
            [] | [b'S', b' ', ..] => {}
            [_, b' ', path @ ..] if !path.is_empty() => paths.push(path),
            _ => return None,
        }
    }
    Some(paths)
}

/// A path as `ls-files -z` printed it: any bytes on Unix, where a path is
/// bytes; UTF-8 elsewhere, which is what git prints there, and no stamp for a
/// path that is not.
#[cfg(unix)]
fn path_from(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from(bytes: &[u8]) -> Option<PathBuf> {
    std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

/// [`SETTLING`], which nothing outside a test build can change.
#[cfg(not(test))]
fn settling() -> Duration {
    SETTLING
}

/// The settling time the calling thread stamps with.
///
/// A test writes its files and gathers a moment later, which is exactly the
/// stamp this refuses; waiting [`SETTLING`] out in every one of them would be
/// seconds apiece for nothing. Thread-local for the reason
/// `run::WRITE_DEADLINE` is.
#[cfg(test)]
fn settling() -> Duration {
    SETTLING_FOR.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// See [`settling`].
    pub(super) static SETTLING_FOR: std::cell::Cell<Duration> =
        const { std::cell::Cell::new(SETTLING) };
}
