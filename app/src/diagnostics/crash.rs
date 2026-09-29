//! A panic, written down where the next launch can find it.
//!
//! [`install`] chains a hook in front of the one Rust already has, so stderr
//! still gets the panic exactly as it always did, and then writes a report:
//! which build, on what, where it panicked and with what message, the
//! backtrace, the GPU it was drawing with, and the last lines of the log. The
//! report goes into a file of its own under
//! [`folder`](super::folder)`/crashes`, and the newest five per channel are
//! kept.
//!
//! # The next launch, and "seen"
//!
//! A report nobody has looked at is one the next window mentions, once, with
//! a line under the header — see `workspace::crash_note`. Looking at it or
//! dismissing it renames the file to `….seen.txt` ([`mark_seen`]), and a seen
//! report is never offered again. The name rather than a settings key, so
//! the answer lives beside the thing it is about: a report deleted by hand
//! takes its "seen" with it, and a settings file copied to a new machine does
//! not carry the old one's.
//!
//! Every panic writes one, on whatever thread. One off the main thread need
//! not end the process — a pane's reader can die and leave the window up — but
//! it leaves a window that has lost a part of itself, which is a bug worth the
//! same report; the report names the thread.
//!
//! # Function names only
//!
//! A shipped build is stripped of its debug information (`strip =
//! "debuginfo"` in the workspace manifest), so its backtrace names functions
//! and not the files and lines they are in. It is still the part of a report
//! that says where to look, and a development build's has the lines as well.

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

/// Installs the hook that writes a report of every panic into `directory`,
/// named after `started`, the moment this launch began.
///
/// Chained to the hook that was there, which runs first: the message on
/// stderr is the one a person at a terminal has always had, and nothing here
/// can delay it or take it away.
pub fn install(directory: PathBuf, channel: Channel, started: String) {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        before(info);

        let panic = Panic::of(info);
        let adapter = crookui::rendering::adapter_in_use();
        let text = report(channel, &panic, adapter.as_deref(), &log_file::recent());
        match write(&directory, channel, &started, &text) {
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
    text.push_str("\nBacktrace (a shipped build carries function names, not files and lines):\n");
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

/// Writes `text` as a new report of `channel` in `directory`, and prunes the
/// channel's reports to the newest [`KEPT`](super::KEPT). Returns the file.
pub fn write(
    directory: &Path,
    channel: Channel,
    started: &str,
    text: &str,
) -> anyhow::Result<PathBuf> {
    ensure_directory(directory)?;
    let stem = format!("{}{started}", super::prefix(channel));
    let (path, mut file) = super::fresh(directory, &stem, "txt")?;
    file.write_all(text.as_bytes())?;
    super::prune(directory, &super::prefix(channel), REPORT, super::KEPT);
    Ok(path)
}

/// The newest report of `channel` in `directory` that nobody has looked at,
/// if there is one.
///
/// Newest by name, which is newest by the launch that wrote it; see
/// [`super::stamp`]. The older unseen ones are not lost — they are in the
/// folder a Show opens, beside it.
pub fn unseen(directory: &Path, channel: Channel) -> Option<PathBuf> {
    super::named(directory, &super::prefix(channel), REPORT)
        .into_iter()
        .rev()
        .find(|path| !is_seen(path))
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
