//! What is on disk where a change to a file would land, as the permission
//! hook finds it before it asks.
//!
//! The window never looks: a stat on its thread of a path on a hung network
//! mount would hang every tab with it. The hook is a process of its own,
//! started for the one question, with Claude Code's five minutes and more to
//! spend, so it is the one that looks, and the request carries what it found
//! — see [`OnDisk`]. Two things, each a question a card cannot answer from
//! the path's text:
//!
//! - whether the file, followed through every symbolic link on the way to
//!   it, is still under the working directory: `./vendor/notes.txt` reads as
//!   inside, and is `/etc/notes.txt` when `vendor` is a link to `/etc`;
//! - for a write, whether a file is there, and how many lines and bytes it
//!   holds: "write a file" reads as making one, and replacing three
//!   thousand lines with five is another thing to allow.
//!
//! The look is bounded three ways: [`LOOK_DEADLINE`] in all, on a thread of
//! its own, so a hung mount is a look given up rather than a hook that hangs;
//! at most [`MAX_PARTS`] parts of the path followed; and at most
//! [`MAX_COUNTED`] bytes of a file read to count its lines. What it cannot
//! say it says the worst of: a link it could not follow is outside, and a
//! write whose file it could not look at is no question at all, its agent's
//! own prompt asking instead — a card that said "creates this file" over a
//! file it never saw would be telling a person something nobody knows.
//!
//! Unix only, as the hook that asks is.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use super::request::{AskRequest, OnDisk, Replaced, lexically_inside};

/// How long the hook looks before it gives up on the disk.
///
/// A second: a stat on a local disk takes microseconds, and one that takes
/// longer than this is a mount that may take forever, inside the
/// three hundred and thirty seconds Claude Code gives the hook.
pub const LOOK_DEADLINE: Duration = Duration::from_secs(1);

/// The most parts of a path below the working directory followed through
/// links. A path is at most [`super::request::MAX_PATH`] characters, so this
/// is reached only by a path of single letters; one past it counts as
/// outside.
pub const MAX_PARTS: usize = 64;

/// The most of a file a write would replace read to count its lines: four
/// MiB, a few milliseconds from a local disk. A file longer than this is
/// said to have at least as many lines as were counted.
pub const MAX_COUNTED: u64 = 4 * 1024 * 1024;

/// What `request` would land on under `cwd`, looked at for at most
/// [`LOOK_DEADLINE`]; `None` when it is a write and whether a file is there
/// could not be said in time, or at all.
///
/// An edit whose look runs out is outside: the file is there — an edit
/// changes what is — and only where it leads is unknown.
pub fn look(request: &AskRequest, cwd: &str) -> Option<OnDisk> {
    let Some(path) = request.path() else {
        return Some(OnDisk {
            resolves_outside: false,
            replaces: None,
        });
    };
    let writes = matches!(request, AskRequest::Write { .. });
    let (path, cwd) = (path.to_owned(), cwd.to_owned());
    let found = within(LOOK_DEADLINE, move || look_now(&path, &cwd, writes));
    match found {
        Some(found) => found,
        None => (!writes).then_some(OnDisk {
            resolves_outside: true,
            replaces: None,
        }),
    }
}

/// What `look` finds, looked at on this thread and for as long as it takes.
fn look_now(path: &str, cwd: &str, writes: bool) -> Option<OnDisk> {
    let replaces = if writes {
        replaced(Path::new(path)).ok()?
    } else {
        None
    };
    Some(OnDisk {
        resolves_outside: !resolves_inside(path, cwd),
        replaces,
    })
}

/// `work` done on a thread of its own, or `None` when it has not finished by
/// `deadline` — the thread left to finish or not, since a stat blocked in
/// the kernel cannot be called off, and the hook exits soon after anyway.
fn within<T: Send + 'static>(
    deadline: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("crook-ask-look".to_owned())
        .spawn(move || {
            // The receiver gone is a look given up on; nothing to tell.
            let _ = sender.send(work());
        })
        .ok()?;
    receiver.recv_timeout(deadline).ok()
}

/// Whether `path`, followed through every link on the way to it, is under
/// `cwd`, followed the same way.
///
/// The deepest part of `path` that is there — the file itself, or the
/// directory it would be made in — is resolved whole, and what is below it
/// is not there to be a link. A path [`lexically_inside`] does not take, a
/// link that leads nowhere, a part that cannot be looked at, or more than
/// [`MAX_PARTS`] of them, is not under it: each is a path a person should
/// be warned about rather than one they should be told is fine.
fn resolves_inside(path: &str, cwd: &str) -> bool {
    if !lexically_inside(path, cwd) {
        return false;
    }
    let Ok(home) = std::fs::canonicalize(cwd) else {
        return false;
    };
    let parts = |text: &str| -> Vec<String> {
        text.split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .map(str::to_owned)
            .collect()
    };
    let below: Vec<String> = parts(path).split_off(parts(cwd).len());
    if below.len() > MAX_PARTS {
        return false;
    }
    for depth in (0..=below.len()).rev() {
        let candidate: PathBuf = below[..depth]
            .iter()
            .fold(PathBuf::from(cwd), |path, part| path.join(part));
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                return std::fs::canonicalize(&candidate).is_ok_and(|real| real.starts_with(&home));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    false
}

/// The file at `path` a write would replace, followed through a link as the
/// write would be, or `None` when nothing is there; an error when something
/// is that is not a file, or that cannot be read.
///
/// A link that leads nowhere is nothing there: a write through it makes the
/// file it names, and [`resolves_inside`] has already said where that is.
fn replaced(path: &Path) -> io::Result<Option<Replaced>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // Never opened unless it is a file: opening a FIFO waits for a writer.
    if !metadata.is_file() {
        return Err(io::Error::other("not a file"));
    }
    let mut file = File::open(path)?.take(MAX_COUNTED);
    let mut buffer = vec![0; 64 * 1024];
    let (mut read, mut newlines) = (0u64, 0u64);
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                read += count as u64;
                newlines += buffer[..count]
                    .iter()
                    .filter(|&&byte| byte == b'\n')
                    .count() as u64;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let bytes = metadata.len().max(read);
    Ok(Some(Replaced {
        lines: if read == 0 { 0 } else { newlines + 1 },
        bytes,
        more: read < bytes,
    }))
}

#[cfg(test)]
#[path = "on_disk_tests.rs"]
mod tests;
