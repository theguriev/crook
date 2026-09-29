//! The log, written to a file beside stderr, and its last lines kept for a
//! crash report.
//!
//! One logger, installed once at the top of [`crate::run`], in place of the
//! `env_logger` that used to be the whole of it. What reaches stderr is
//! untouched — the same filter, the same `RUST_LOG`, the same colours and
//! format — because [`Tee`] hands every record to that very logger first and
//! only then writes its own copy. So a plugin's `log` import, which calls
//! `log::info!` like any line of Crook's, lands in the file with nothing done
//! about it.
//!
//! # The file is opened late
//!
//! The logger exists from the first line of `run`, but a file is opened only
//! for a window — see [`super`] for why — and only once [`open`] is called.
//! Until then the lines go to stderr and to the recent lines, and the file is
//! started with those, so nothing a window run said before its file existed
//! is missing from it.
//!
//! # Nothing here can stop Crook
//!
//! A folder that cannot be made or a file that cannot be written is an error
//! returned to whoever opened it, and after that the logger behaves exactly as
//! it did with no file: a write that fails is dropped, and stderr never
//! notices. The file stops growing at eight megabytes, so a plugin that logs
//! in a loop fills a few megabytes rather than a disk; and the recent lines
//! keep the first few kilobytes of each, so a plugin that logs a megabyte at
//! a time costs neither the memory of two hundred of them nor a crash report
//! too large to attach to an issue.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use log::{Level, Log, Metadata, Record};

use crate::Channel;
use crate::settings::ensure_directory;

/// How many of the latest lines are kept for a crash report.
pub const RECENT: usize = 200;

/// How much of one line the recent lines keep.
///
/// A plugin's `log` import takes a string as long as anything a plugin may
/// hand the host, a megabyte, and a line of Crook's own is a few dozen bytes.
/// Enough for any message a person would read to the end, and small enough
/// that [`RECENT`] of them are under a megabyte.
pub const LINE_BYTES: usize = 4 * 1024;

/// How far one launch's log file may grow.
///
/// Far more than a day of Crook's own lines, which are a few a minute at the
/// default level, and small enough that five of them are nothing on any disk.
const MOST_BYTES: u64 = 8 * 1024 * 1024;

/// The last line a file that reached [`MOST_BYTES`] gets.
const FULL: &str = "[the log file reached its limit here; the rest went to stderr only]\n";

/// The logger this process logs through, once [`init`] has run.
static LOGGER: OnceLock<Tee> = OnceLock::new();

/// Installs the logger: stderr as before, and the recent lines beside it.
///
/// Called once, first thing. A second call changes nothing, which is what
/// `log` itself allows.
pub fn init() {
    let stderr =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    let most = stderr.filter();
    let logger = LOGGER.get_or_init(|| Tee::new(stderr));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(most);
    }
}

/// Starts writing the log into a new file in `directory`, and prunes the
/// channel's older ones to the newest [`KEPT`](super::KEPT). Returns the file.
pub fn open(directory: &Path, channel: Channel, started: &str) -> Result<PathBuf> {
    match LOGGER.get() {
        Some(logger) => logger.open(directory, channel, started),
        None => bail!("the logger was never started"),
    }
}

/// The latest lines logged, oldest first, each ending in a newline.
pub fn recent() -> Vec<String> {
    LOGGER.get().map(Tee::recent).unwrap_or_default()
}

/// Writes `message` into the file and the recent lines, as an error, without
/// printing it on stderr.
///
/// For the two things that are already on stderr in their own words — a panic,
/// which the default hook has printed, and the error a failed startup ends
/// the process with — and would otherwise never reach the file.
pub fn note(message: &str) {
    if let Some(logger) = LOGGER.get() {
        logger.note(message);
    }
}

/// A logger that is `env_logger` on stderr and, beside it, a file and the
/// recent lines.
pub struct Tee {
    /// The logger stderr has always had, and the filter every copy obeys.
    stderr: env_logger::Logger,
    /// The file, once one is open.
    file: Mutex<Option<Sink>>,
    /// The latest [`RECENT`] lines, whether or not a file is open.
    recent: Mutex<VecDeque<String>>,
    /// How far the file may grow: [`MOST_BYTES`], except in a test.
    most_bytes: u64,
}

impl Tee {
    /// A logger over `stderr`, with no file yet.
    pub fn new(stderr: env_logger::Logger) -> Self {
        Self {
            stderr,
            file: Mutex::new(None),
            recent: Mutex::new(VecDeque::with_capacity(RECENT)),
            most_bytes: MOST_BYTES,
        }
    }

    /// The same, with a file that stops at `most_bytes`, which is how a test
    /// reaches the limit without writing megabytes.
    #[cfg(test)]
    fn limited_to(stderr: env_logger::Logger, most_bytes: u64) -> Self {
        Self {
            most_bytes,
            ..Self::new(stderr)
        }
    }

    /// See [`open`].
    pub fn open(&self, directory: &Path, channel: Channel, started: &str) -> Result<PathBuf> {
        ensure_directory(directory)?;
        let stem = format!("{}{started}", super::prefix(channel));
        let (path, file) = super::fresh(directory, &stem, "log")
            .with_context(|| format!("could not create a log file in {}", directory.display()))?;

        let mut sink = Sink {
            file,
            written: 0,
            most: self.most_bytes,
        };
        // What was said before there was a file, so the file is the whole of
        // this run rather than the part of it after the window was asked for
        // (a line past `LINE_BYTES` as the recent lines kept it, clipped).
        // Under the file's lock, which every line takes before the recent
        // lines' — see `keep` — so a line logged meanwhile is written once,
        // after these, rather than twice or not at all.
        let mut open = held(&self.file);
        for line in held(&self.recent).iter() {
            sink.write(line);
        }
        *open = Some(sink);
        drop(open);

        super::prune(
            directory,
            &super::prefix(channel),
            ".log",
            super::KEPT,
            &path,
        );
        Ok(path)
    }

    /// See [`recent`].
    ///
    /// Patient for a moment and no longer: a crash report asks for these from
    /// a panic hook, and a hook must not wait forever on a lock the panicking
    /// thread may be holding.
    pub fn recent(&self) -> Vec<String> {
        briefly(&self.recent)
            .map(|recent| recent.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// See [`note`].
    pub fn note(&self, message: &str) {
        let line = line(Level::Error, "crook", format_args!("{message}"));
        let mut file = briefly(&self.file);
        if let Some(sink) = file.as_mut().and_then(|file| file.as_mut()) {
            sink.write(&line);
        }
        if let Some(mut recent) = briefly(&self.recent) {
            remember(&mut recent, line);
        }
    }

    /// Writes one line that already passed the filter to the file and the
    /// recent lines.
    ///
    /// The file's lock is held until the line is among the recent ones too,
    /// and it is always taken first — here, in `note` and in `open` — which
    /// is the order that keeps two locks from ever waiting on each other.
    fn keep(&self, line: String) {
        let mut file = held(&self.file);
        if let Some(sink) = file.as_mut() {
            sink.write(&line);
        }
        remember(&mut held(&self.recent), line);
    }
}

impl Log for Tee {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.stderr.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        // One filter for every copy, and it is the one stderr always had: a
        // line `RUST_LOG` keeps off the terminal is kept out of the file too.
        if !self.stderr.matches(record) {
            return;
        }
        self.stderr.log(record);
        // Formatted before either lock is taken, so a `Display` that panics
        // does it holding nothing a crash report needs.
        self.keep(line(record.level(), record.target(), *record.args()));
    }

    fn flush(&self) {
        self.stderr.flush();
    }
}

/// An open log file and how much has gone into it.
struct Sink {
    file: File,
    written: u64,
    most: u64,
}

impl Sink {
    /// Writes `line`, unless the file has reached its limit — and says so,
    /// once, when it does.
    ///
    /// Unbuffered: every line is on disk when the call returns, which is the
    /// only way a native crash, which runs no code on its way out, leaves its
    /// last lines behind.
    fn write(&mut self, line: &str) {
        if self.written >= self.most {
            return;
        }
        let length = line.len() as u64;
        if self.written + length > self.most {
            let _ = self.file.write_all(FULL.as_bytes());
            self.written = self.most;
            return;
        }
        if self.file.write_all(line.as_bytes()).is_ok() {
            self.written += length;
        }
    }
}

/// One record as a line of the file: when, how bad, from where, and what.
///
/// The shape stderr's lines have, so a line copied from either reads the same,
/// with the local time and its offset rather than UTC — a person matching a
/// line to "about ten past three" does it in their own time zone.
fn line(level: Level, target: &str, message: std::fmt::Arguments<'_>) -> String {
    let at = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z");
    format!("[{at} {level:<5} {target}] {message}\n")
}

/// Keeps `line` as the newest of the recent lines, dropping the oldest past
/// [`RECENT`], and the part of it past [`LINE_BYTES`].
fn remember(recent: &mut VecDeque<String>, line: String) {
    if recent.len() == RECENT {
        recent.pop_front();
    }
    recent.push_back(clipped(line));
}

/// `line` cut to its first [`LINE_BYTES`], saying how much more there was,
/// and still ending in a newline.
fn clipped(line: String) -> String {
    let said = line.strip_suffix('\n').unwrap_or(&line);
    if said.len() <= LINE_BYTES {
        return line;
    }
    let mut end = LINE_BYTES;
    while !said.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [… {} bytes more]\n", &said[..end], said.len() - end)
}

/// Takes `lock`, a poisoned one included: a thread that panicked while
/// logging leaves a queue of lines that is still a queue of lines.
fn held<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Takes `lock` if it comes free within a moment. See [`Tee::recent`].
fn briefly<T>(lock: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    /// A tenth of a second, in all: longer than any line takes to write, and
    /// short enough that a hook stuck behind a lock it will never get still
    /// writes its report without it.
    const TRIES: usize = 100;

    for _ in 0..TRIES {
        match lock.try_lock() {
            Ok(guard) => return Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    None
}

#[cfg(test)]
#[path = "log_file_tests.rs"]
mod tests;
