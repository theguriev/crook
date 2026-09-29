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
//! to tell the next window that reports are waiting — see
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
//! the last line that was written — which is why no launch prunes a log
//! another window still has open.

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
    /// The crash reports of this channel nobody has looked at yet, oldest
    /// first: what the line under the header is about, and what answering it
    /// marks seen. Empty when there are none.
    pub crashes: Vec<PathBuf>,
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
/// hook, and looks for the reports earlier runs left.
///
/// `None` only on a machine with no home directory to put anything in, and
/// then nothing is written — stderr carries on as it always did. A log file
/// that cannot be opened is a line on stderr and not a reason to stop: the
/// hook is installed regardless, since a report is a file of its own.
pub fn start(channel: Channel) -> Option<Diagnostics> {
    let folder = folder()?;

    match log_file::open(&folder.join(LOGS), channel, &stamp(chrono::Utc::now())) {
        Ok(path) => log::info!("this run's log is {}", path.display()),
        Err(error) => log::warn!("this run keeps no log file: {error:#}"),
    }

    let directory = folder.join(CRASHES);
    // Looked for before the hook is in, so no report found can be one this
    // run wrote.
    let crashes = crash::unseen(&directory, channel);
    crash::install(directory, channel);
    Some(Diagnostics { folder, crashes })
}

/// A moment as a file is named with it: a log file with the moment its launch
/// began, a crash report with the moment of the panic.
///
/// Second resolution in an order-preserving spelling, the way the session's
/// backups are named: a name that sorts as a date is what lets the pruning
/// tell the oldest file by name alone, and a name survives a copy of the
/// folder where a modification time does not. In UTC, whatever zone `at` is
/// in, and marked so: a laptop that flew west, or a clock that went back an
/// hour for the winter, would otherwise name a later launch as an earlier one,
/// and the pruning would take it for the oldest.
pub fn stamp<Zone: chrono::TimeZone>(at: chrono::DateTime<Zone>) -> String {
    at.with_timezone(&chrono::Utc)
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

/// The start every file of `channel` is named with.
///
/// The channel is in the name so that a dev build and a shipped one can share
/// the folder without either counting, pruning or reporting the other's
/// files — the side-by-side install [`Channel`] exists for.
fn prefix(channel: Channel) -> String {
    format!("crook-{}-", channel.name())
}

/// Creates `<stem>.<extension>` in `directory`, or, when a file of the same
/// second is already there, `<stem>_02.<extension>` and on.
///
/// Created rather than opened, so that two processes started together never
/// write one file between them. The second file's mark is an underscore and
/// two digits because of how names sort: `_` comes after the `.` of the first
/// name and `02` before `10`, so a later file of the same second is a newer
/// one to [`prune`] and [`crash::unseen`] as well.
///
/// That holds only if a later file never takes a lower mark than one that was
/// there before it, so the marks start after the highest one in the folder
/// ([`next_mark`]) rather than at the first free name. The first name is free
/// again once the sixth file of its second has had it pruned, or once its
/// report is marked seen; a file that took it back would sort as the oldest
/// of its second and be pruned before the files it came after — or, as a
/// report, be renamed over the seen one when it was marked in its turn.
///
/// The file is held for as long as it is open, so that no other launch's
/// [`prune`] removes a log a window is still writing: on Windows by not
/// sharing the right to delete it, and elsewhere by an advisory lock, which a
/// person reading the file with anything else does not notice.
fn fresh(directory: &Path, stem: &str, extension: &str) -> io::Result<(PathBuf, File)> {
    /// The highest mark, so every mark is two digits — and a bound, so a
    /// folder that answers every name with "exists" is an error rather than a
    /// loop.
    const MOST_MARK: usize = 99;

    let mut options = File::options();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        /// `FILE_SHARE_READ | FILE_SHARE_WRITE`: everything std shares but
        /// `FILE_SHARE_DELETE`.
        const SHARED: u32 = 0x1 | 0x2;
        options.share_mode(SHARED);
    }

    // Still tried in turn from there, and each one created rather than
    // assumed free: another launch can take a mark between the look and the
    // creation.
    let mut taken = None;
    for mark in next_mark(directory, stem)..=MOST_MARK {
        let name = match mark {
            1 => format!("{stem}.{extension}"),
            _ => format!("{stem}_{mark:02}.{extension}"),
        };
        let path = directory.join(name);
        match options.open(&path) {
            Ok(file) => {
                // A file system with no locks is one where a live log can be
                // pruned, as before; it is not a reason to have no log.
                #[cfg(unix)]
                let _ = file.try_lock();
                return Ok((path, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => taken = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(taken.unwrap_or_else(|| io::Error::other("no name was free")))
}

/// The mark after the highest one any file of `stem` in `directory` has —
/// `1`, the bare `<stem>.<extension>`, when there is none.
///
/// Of any extension, so a report marked seen, `<stem>.seen.txt`, still holds
/// the mark it was written with.
fn next_mark(directory: &Path, stem: &str) -> usize {
    named(directory, stem, "")
        .iter()
        .filter_map(|path| mark(path.file_name()?.to_str()?.strip_prefix(stem)?))
        .max()
        .map_or(1, |highest| highest + 1)
}

/// The mark of a file of a stem, from what follows the stem in its name: `1`
/// for `.…`, and `NN` for `_NN.…`. `None` for a name that only starts the
/// same way.
fn mark(rest: &str) -> Option<usize> {
    if rest.starts_with('.') {
        return Some(1);
    }
    let (digits, _) = rest.strip_prefix('_')?.split_once('.')?;
    if digits.len() != 2 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Removes every file in `directory` named `<prefix>…<suffix>` but the newest
/// `kept`, and never `made`, the file the caller has just created, or one
/// another process is still writing.
///
/// Newest by name: see [`stamp`]. `made` is spared by name because a clock
/// set back can still stamp it older than the rest, and a file is not pruned
/// by its own creation; one in use is spared because Crook is a process per
/// window, and the log of a window opened this morning is the one worth
/// having if a driver takes that window down tonight. A file that will not go
/// is left, quietly — the next launch tries again, and a few files nobody can
/// delete are not a problem this can solve.
fn prune(directory: &Path, prefix: &str, suffix: &str, kept: usize, made: &Path) {
    for stale in named(directory, prefix, suffix)
        .into_iter()
        .rev()
        .skip(kept)
    {
        if stale != made && !in_use(&stale) {
            let _ = fs::remove_file(stale);
        }
    }
}

/// Whether another open file holds `path` the way [`fresh`] does.
///
/// Asked with a shared lock, which the writer's exclusive one refuses all the
/// same, because the file is opened here for reading only. An exclusive lock
/// can need a file open for writing: NFS on Linux carries these locks to the
/// server as byte-range ones and refuses an exclusive one on a file open only
/// for reading with `EBADF`, which is an error rather than "held" — so a home
/// folder on NFS, where the writer's own lock was taken, had the log of a
/// window still running pruned. A file system with no locks at all answers
/// with an error as well, and there a live log can be pruned, as [`fresh`]
/// says.
///
/// Never asked on Windows. The hold there is the missing right to delete, and
/// the removal that asks for it is refused by itself; and a lock there is
/// mandatory, so even a moment's lock taken to ask would refuse the other
/// window's next line.
fn in_use(path: &Path) -> bool {
    if cfg!(windows) {
        return false;
    }
    File::open(path)
        .is_ok_and(|file| matches!(file.try_lock_shared(), Err(fs::TryLockError::WouldBlock)))
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

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone as _};

    use super::{mark, stamp};

    #[test]
    fn a_mark_is_read_only_from_a_name_fresh_could_have_made() {
        assert_eq!(mark(".log"), Some(1));
        assert_eq!(mark(".seen.txt"), Some(1));
        assert_eq!(mark("_07.txt"), Some(7));
        assert_eq!(mark("_07.seen.txt"), Some(7));
        // Copies a file manager made beside a report, which say nothing about
        // how many files their second wrote.
        assert_eq!(mark(" copy.txt"), None);
        assert_eq!(mark("_07 (1).txt"), None);
        assert_eq!(mark("_7.txt"), None);
    }

    #[test]
    fn a_later_moment_is_a_later_name_whatever_zone_the_clock_was_in() {
        // Crook opened in Berlin at six in the evening, and again after a
        // flight, in New York at one in the afternoon — an hour later. Named
        // in local time, the second launch sorted before the first, and with
        // five files after it the pruning took it for the oldest as it was
        // made.
        let berlin = FixedOffset::east_opt(2 * 3600)
            .expect("an offset")
            .with_ymd_and_hms(2026, 9, 29, 18, 0, 0)
            .single()
            .expect("a moment");
        let new_york = FixedOffset::west_opt(4 * 3600)
            .expect("an offset")
            .with_ymd_and_hms(2026, 9, 29, 13, 0, 0)
            .single()
            .expect("a moment");

        assert_eq!(stamp(berlin), "20260929T160000Z");
        assert_eq!(stamp(new_york), "20260929T170000Z");
        assert!(stamp(berlin) < stamp(new_york));
    }
}
