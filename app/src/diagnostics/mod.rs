//! What a run leaves on disk for somebody to read after it went wrong: a log
//! file per launch, and a report of a panic.
//!
//! Crook has no telemetry, so the only way anyone learns that it failed is that
//! a person says so — and until this existed there was nothing to say it with.
//! The log went to a stderr nobody sees when Crook is opened from the Dock, a
//! `.desktop` entry or Explorer, and a panic took its message with it. Both
//! land in files now, in one folder a person can be pointed at: Settings →
//! About names it, and the README says what to attach to an issue.
//!
//! # Nothing here is sent anywhere
//!
//! Every byte stays in [`folder`], and nothing in Crook reads it back except
//! to tell the next window that a report is waiting — see
//! [`crash::unseen`]. A person reads a file, decides what of it to share,
//! and shares it themselves. That is what keeps the README's promise of no
//! telemetry and no network until asked.
//!
//! # Only a window run
//!
//! [`start`] is called for a window and for nothing else. `crook --agent`
//! runs from an agent's hooks several times a minute, and a log file for each
//! of those would push the window's own out of the five that are kept within
//! the hour.
//!
//! # What is not caught
//!
//! A panic is Rust's own failure, and the hook sees it before the process
//! goes. A *native* crash — a segfault in a GPU driver, an access violation,
//! an abort from inside a system library — never runs Rust code on the way
//! down, so it leaves no report; the log file is what is left of it, up to
//! the last line that was written.

pub mod crash;
pub mod log_file;

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use crate::Channel;

/// How many log files, and how many crash reports, are kept per channel.
///
/// Five, because the launch that went wrong is rarely the last one: a person
/// usually tries again once or twice before they go looking, and the files
/// those tries wrote must not have pushed out the one worth reading.
pub const KEPT: usize = 5;

/// The folder under [`folder`] the log files go in.
const LOGS: &str = "logs";

/// The folder under [`folder`] the crash reports go in.
const CRASHES: &str = "crashes";

/// What this run writes to, and what the last one left unread.
///
/// Handed to the window rather than found by it, for the reason the plugins
/// directory is: a window that looked in the real folder would be a window no
/// test could open without reading the crash reports of whoever runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostics {
    /// The folder the logs and the crash reports are both under, which is
    /// what the About page names and opens.
    pub folder: PathBuf,
    /// The newest crash report of this channel nobody has looked at yet, if
    /// there is one.
    pub crash: Option<PathBuf>,
}

/// Where this machine keeps Crook's logs and crash reports.
///
/// The platform's own place for them rather than the configuration folder,
/// which a person may keep in a dotfiles repository and would not want logs
/// committed into: the state directory on Linux (`~/.local/state/crook`),
/// with the data directory where there is none; `~/Library/Logs/Crook` on
/// macOS, which is where Console.app looks; the local rather than the roaming
/// application data on Windows, since a log is about this machine.
pub fn folder() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return dirs::home_dir().map(|home| home.join("Library").join("Logs").join("Crook"));
    }
    if cfg!(target_os = "windows") {
        return dirs::data_local_dir().map(|local| local.join("crook"));
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|state| state.join("crook"))
}

/// Starts this run's diagnostics: opens the log file, installs the panic
/// hook, and looks for a report an earlier run left.
///
/// `None` only on a machine with no home directory to put anything in, and
/// then nothing is written — stderr carries on as it always did. A log file
/// that cannot be opened is a line on stderr and not a reason to stop: the
/// hook is installed regardless, since a report is a file of its own.
pub fn start(channel: Channel) -> Option<Diagnostics> {
    let folder = folder()?;
    let started = stamp(chrono::Local::now());

    match log_file::open(&folder.join(LOGS), channel, &started) {
        Ok(path) => log::info!("this run's log is {}", path.display()),
        Err(error) => log::warn!("this run keeps no log file: {error:#}"),
    }

    let crashes = folder.join(CRASHES);
    // Looked for before the hook is in, so the report found can never be one
    // this run wrote.
    let crash = crash::unseen(&crashes, channel);
    crash::install(crashes, channel, started);
    Some(Diagnostics { folder, crash })
}

/// A moment as the files of one launch are named with it.
///
/// Second resolution in an order-preserving spelling, the way the session's
/// backups are named: a name that sorts as a date is what lets the pruning
/// tell the oldest file by name alone, and a name survives a copy of the
/// folder where a modification time does not.
pub fn stamp(at: chrono::DateTime<chrono::Local>) -> String {
    at.format("%Y%m%d-%H%M%S").to_string()
}

/// The start every file of `channel` is named with.
///
/// The channel is in the name so that a dev build and a shipped one can share
/// the folder without either counting, pruning or reporting the other's
/// files — the side-by-side install [`Channel`] exists for.
fn prefix(channel: Channel) -> String {
    format!("crook-{}-", channel.name())
}

/// Creates `<stem>.<extension>` in `directory`, or, when another launch in the
/// same second got there first, `<stem>-2.<extension>` and on.
///
/// Created rather than opened, so that two processes started together never
/// write one file between them.
fn fresh(directory: &Path, stem: &str, extension: &str) -> io::Result<(PathBuf, File)> {
    /// A bound, so a folder that answers every name with "exists" is an
    /// error rather than a loop.
    const ATTEMPTS: usize = 64;

    let mut taken = None;
    for attempt in 1..=ATTEMPTS {
        let name = match attempt {
            1 => format!("{stem}.{extension}"),
            _ => format!("{stem}-{attempt}.{extension}"),
        };
        let path = directory.join(name);
        match File::options().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => taken = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(taken.unwrap_or_else(|| io::Error::other("no name was free")))
}

/// Removes every file in `directory` named `<prefix>…<suffix>` but the newest
/// `kept`.
///
/// Newest by name, which is newest by start time: see [`stamp`]. A file that
/// will not go is left, quietly — the next launch tries again, and a few files
/// nobody can delete are not a problem this can solve.
fn prune(directory: &Path, prefix: &str, suffix: &str, kept: usize) {
    for stale in named(directory, prefix, suffix)
        .into_iter()
        .rev()
        .skip(kept)
    {
        let _ = fs::remove_file(stale);
    }
}

/// Every file in `directory` named `<prefix>…<suffix>`, oldest first.
fn named(directory: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
        })
        .collect();
    files.sort();
    files
}

/// A folder of a test's own under the system's temporary one, removed when
/// the test is done with it.
#[cfg(test)]
struct Scratch(PathBuf);

#[cfg(test)]
impl Scratch {
    fn new(name: &str) -> Self {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "crook-diagnostics-{name}-{}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
