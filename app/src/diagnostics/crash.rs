//! A panic, written down where the next launch can find it.
//!
//! [`install`] chains a hook in front of the one Rust already has, so stderr
//! still gets the panic exactly as it always did, and then writes a report:
//! which build, on what, where it panicked and with what message, the
//! backtrace, the GPU it was drawing with, and the last lines of the log. The
//! report goes into a file of its own under
//! [`folder`](super::folder)`/crashes`, named with the moment of the panic
//! rather than the launch's start, so a window left open for days that
//! panics tonight writes the newest report and not one the pruning takes for
//! the oldest. The newest five per channel are kept.
//!
//! # The next launch, and "seen"
//!
//! The reports nobody has looked at are what the next window mentions, once,
//! with a line under the header — see `workspace::crash_note`. Looking at
//! them or dismissing the line renames every report it was about to
//! `….seen.txt` ([`mark_seen`]), and a seen report is never offered again.
//! Every one, because a launch that panicked twice, or two launches that
//! both did, are one line: answering it must not leave the other report to
//! say "Crook stopped unexpectedly last time" after a run that ended well.
//! And only those, because a report another window writes while the line is
//! up is one nobody has been told of yet.
//!
//! The name rather than a settings key, so the answer lives beside the thing
//! it is about: a report deleted by hand takes its "seen" with it, and a
//! settings file copied to a new machine does not carry the old one's.
//!
//! Every panic writes one, on whatever thread. One off the main thread need
//! not end the process — a pane's reader can die and leave the window up — but
//! it leaves a window that has lost a part of itself, which is a bug worth the
//! same report; the report names the thread.
//!
//! # Function names, where there are any
//!
//! A shipped build is stripped of its debug information (`strip =
//! "debuginfo"` in the workspace manifest), so its backtrace names functions
//! and not the files and lines they are in — on Linux and macOS, where the
//! names stay in the binary. A Windows build keeps them in a `crook.pdb`
//! beside the `.exe` it was linked into, and the download carries the `.exe`
//! alone, so a shipped Windows build's backtrace has no names at all; its
//! report's `Where` line is the one that says where to look, and the heading
//! over the backtrace says so. A development build's has the lines as well,
//! on every platform.

use std::io::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

use crate::Channel;
use crate::settings::ensure_directory;

use super::log_file;

/// What every report's name ends in, seen or not: a seen one keeps the
/// extension, so it still opens in whatever opens text.
const REPORT: &str = ".txt";

/// What a report is renamed to end in once somebody has looked at it.
const SEEN: &str = ".seen.txt";

/// The heading over the backtrace, saying what a shipped build's can name.
/// See the module's documentation.
const BACKTRACE: &str = if cfg!(windows) {
    "Backtrace (a shipped Windows build names nothing here, since the names are in a\n\
     crook.pdb the download does not carry; the Where line above is the place to start):\n"
} else {
    "Backtrace (a shipped build carries function names, not files and lines):\n"
};

/// One panic, in the terms a report writes it down in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panic {
    /// What it said.
    pub message: String,
    /// Where: `file:line:column`, when Rust knew.
    pub location: Option<String>,
    /// The thread it happened on, by name.
    pub thread: String,
    /// When, in local time with its offset.
    pub at: String,
    /// The backtrace, as `std` prints one.
    pub backtrace: String,
}

impl Panic {
    /// The panic a hook was handed, and the backtrace from where it is now.
    ///
    /// Captured whatever `RUST_BACKTRACE` says: the variable is for the
    /// terminal, and nobody launching Crook from the Dock has set it.
    fn of(info: &PanicHookInfo<'_>) -> Self {
        Self {
            message: info
                .payload_as_str()
                .unwrap_or("a panic with no message")
                .to_owned(),
            location: info.location().map(ToString::to_string),
            thread: std::thread::current()
                .name()
                .unwrap_or("an unnamed thread")
                .to_owned(),
            at: chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f %:z")
                .to_string(),
            backtrace: std::backtrace::Backtrace::force_capture().to_string(),
        }
    }
}

/// Installs the hook that writes a report of every panic into `directory`.
///
/// Chained to the hook that was there, which runs first: the message on
/// stderr is the one a person at a terminal has always had, and nothing here
/// can delay it or take it away.
pub fn install(directory: PathBuf, channel: Channel) {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        before(info);

        let at = super::stamp(chrono::Utc::now());
        let panic = Panic::of(info);
        let adapter = crookui::rendering::adapter_in_use();
        let text = report(channel, &panic, adapter.as_deref(), &log_file::recent());
        match write(&directory, channel, &at, &text) {
            Ok(path) => log_file::note(&format!(
                "panicked at {}: {}; the report is {}",
                panic.location.as_deref().unwrap_or("an unknown place"),
                panic.message,
                path.display()
            )),
            Err(error) => {
                eprintln!(
                    "crook: could not write a crash report into {}: {error:#}",
                    directory.display()
                );
            }
        }
    }));
}

/// The report of one panic, as the file says it.
///
/// Plain text, headed by a paragraph saying what the file is and that it has
/// gone nowhere, because it is read by a person first — the one deciding
/// what to paste into an issue — and by the owner second.
pub fn report(channel: Channel, panic: &Panic, adapter: Option<&str>, recent: &[String]) -> String {
    let mut text = String::from(
        "Crook stopped unexpectedly.\n\
         \n\
         This report was written on this machine and has not been sent anywhere. Read it\n\
         before you attach it to an issue: the log lines at the end can name folders and\n\
         files of yours.\n\
         \n",
    );

    let facts = [
        ("Version", env!("CARGO_PKG_VERSION").to_owned()),
        ("Channel", channel.name().to_owned()),
        ("Target", target()),
        ("When", panic.at.clone()),
        ("Thread", panic.thread.clone()),
        (
            "Where",
            panic
                .location
                .clone()
                .unwrap_or_else(|| String::from("not known")),
        ),
        (
            "GPU",
            adapter.map_or_else(|| String::from("none opened yet"), str::to_owned),
        ),
    ];
    for (name, value) in facts {
        text.push_str(&format!("{name:<9}{value}\n"));
    }

    text.push_str(&format!("\nMessage:\n{}\n", panic.message));
    text.push('\n');
    text.push_str(BACKTRACE);
    text.push_str(&panic.backtrace);
    if !panic.backtrace.ends_with('\n') {
        text.push('\n');
    }

    text.push_str(&format!("\nThe last {} lines of the log:\n", recent.len()));
    for line in recent {
        text.push_str(line);
    }
    text
}

/// What this build was compiled for, as `<arch>-<os>`.
///
/// Not the full triple, which only a build script is told; this is the part
/// of it that tells a report from one platform from a report from another.
fn target() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// Writes `text` as a new report of `channel` in `directory`, named with
/// `at`, the moment of the panic as [`super::stamp`] spells it, and prunes the
/// channel's reports to the newest [`KEPT`](super::KEPT) — never the one just
/// written. Returns the file.
pub fn write(directory: &Path, channel: Channel, at: &str, text: &str) -> anyhow::Result<PathBuf> {
    ensure_directory(directory)?;
    let stem = format!("{}{at}", super::prefix(channel));
    let (path, mut file) = super::fresh(directory, &stem, "txt")?;
    file.write_all(text.as_bytes())?;
    // Closed now: a report is written whole and once, so it needs no hold
    // against other launches' pruning, and its name is what spares it from
    // this one's, on a file system with locks or without.
    drop(file);
    super::prune(
        directory,
        &super::prefix(channel),
        REPORT,
        super::KEPT,
        &path,
    );
    Ok(path)
}

/// Every report of `channel` in `directory` that nobody has looked at,
/// oldest first; see [`super::stamp`] for why a name is its age.
pub fn unseen(directory: &Path, channel: Channel) -> Vec<PathBuf> {
    super::named(directory, &super::prefix(channel), REPORT)
        .into_iter()
        .filter(|path| !is_seen(path))
        .collect()
}

/// Marks a report as looked at, by its name, and returns where it is now.
///
/// A report already marked is where it was.
pub fn mark_seen(report: &Path) -> std::io::Result<PathBuf> {
    if is_seen(report) {
        return Ok(report.to_owned());
    }
    let name = report
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(REPORT))
        .ok_or_else(|| std::io::Error::other(format!("{} is not a report", report.display())))?;
    let seen = report.with_file_name(format!("{name}{SEEN}"));
    std::fs::rename(report, &seen)?;
    Ok(seen)
}

/// Whether a report's name says somebody has looked at it.
fn is_seen(report: &Path) -> bool {
    report
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(SEEN))
}

#[cfg(test)]
#[path = "crash_tests.rs"]
mod tests;
