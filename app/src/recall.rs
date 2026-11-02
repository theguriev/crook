//! What Crook remembers about each command it saw run: where it ran, how it
//! ended, and what ran before it — so the suggestion after the caret can be
//! the command that belongs *here*, not merely the newest one that starts the
//! same way.
//!
//! # Why a file of Crook's own
//!
//! The shell's history file is a list of lines. bash writes no directory and
//! no status; zsh writes a start time at most; fish writes the paths a command
//! *mentioned*, which is not where it ran. Crook sees all three things for
//! every command that ends — the directory from OSC 7 when it started, the
//! status from OSC 133 `D`, the command before it in the same pane — and
//! keeps them in `history.jsonl` under the data directory, one JSON object a
//! line, appended as each command ends.
//!
//! The shell's own file still seeds the field — see
//! [`crate::shell_history`] — and what it holds is offered when nothing here
//! answers better: it is the record of everything run before Crook was.
//!
//! # What is not kept
//!
//! A command that starts with a blank, the way `HISTCONTROL=ignorespace` and
//! zsh's `HIST_IGNORE_SPACE` mean it: a person who typed one asked for it not
//! to be remembered, and a second history that remembered it anyway would be
//! the worse of the two.
//!
//! # How a suggestion is chosen
//!
//! Among the commands that start with what is typed, by a small score and
//! then by recency — see [`score`]. Ran in this directory counts most; ran
//! right after the command this pane just ran counts next; ended in failure
//! counts against. Equal scores go to the newest, which keeps the rule a
//! person can predict: the command they ran last, here, after this.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How many records are read back, newest kept.
const LIMIT: usize = 5000;

/// How much of the end of the file is read, at most.
const TAIL: u64 = 2 * 1024 * 1024;

/// Past this size the file is rewritten holding only its newest [`LIMIT`]
/// records, so that it never grows for ever.
const TRIM_AT: u64 = 4 * 1024 * 1024;

/// One command that ran and ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The command line, as it was run.
    pub command: String,
    /// Where it started, when the shell said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<PathBuf>,
    /// How it ended, when the shell said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// When it ended, in seconds since the epoch.
    #[serde(default)]
    pub at: u64,
    /// The command that ended before it in the same pane, if there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// What a pane's field knows about where it is, which is what a suggestion is
/// ranked against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// The pane's working directory.
    pub directory: Option<PathBuf>,
    /// The command this pane ran last.
    pub previous: Option<String>,
}

/// The records, oldest first.
#[derive(Debug, Default)]
pub struct Recall {
    records: Vec<Record>,
    /// How many of [`Self::records`] hold each command: what says a plain
    /// line is one a record knows more about.
    known: HashMap<String, usize>,
    /// Where new records are appended; `None` keeps them in memory only.
    file: Option<PathBuf>,
}

impl Recall {
    /// A recall over `records` that writes nowhere.
    #[cfg(test)]
    pub fn in_memory(records: Vec<Record>) -> Self {
        Self::over(records, None)
    }

    fn over(records: Vec<Record>, file: Option<PathBuf>) -> Self {
        let mut known = HashMap::new();
        for record in &records {
            *known.entry(record.command.clone()).or_insert(0) += 1;
        }
        Self {
            records,
            known,
            file,
        }
    }

    /// The records in the file at `path`, appended to from now on.
    ///
    /// A file that is not there is an empty history, and a line that does not
    /// read is skipped: one bad line costs one command, not the file.
    pub fn open(path: PathBuf) -> Self {
        trim(&path);
        let records = read_tail(&path).map_or_else(Vec::new, |text| parse(&text));
        Self::over(records, Some(path))
    }

    /// The records, oldest first.
    #[cfg(test)]
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Remembers a command that ended, unless it is one not to remember.
    pub fn remember(&mut self, record: Record) {
        if record.command.trim().is_empty() || record.command.starts_with(char::is_whitespace) {
            return;
        }
        if let Some(path) = &self.file {
            append(path, &record);
        }
        *self.known.entry(record.command.clone()).or_insert(0) += 1;
        self.records.push(record);
        let over = self.records.len().saturating_sub(LIMIT);
        for gone in self.records.drain(..over) {
            if let Some(count) = self.known.get_mut(&gone.command) {
                *count -= 1;
                if *count == 0 {
                    self.known.remove(&gone.command);
                }
            }
        }
    }

    /// The command to offer for `line`: the best-scored among the records and
    /// `plain` — lines with nothing known about them, oldest first, the way
    /// the field's own history holds them — that start with `line` and add
    /// something to it.
    pub fn best<'a>(
        &'a self,
        line: &str,
        plain: &'a [String],
        context: &Context,
    ) -> Option<&'a str> {
        if line.is_empty() {
            return None;
        }
        let extends = |command: &str| command.len() > line.len() && command.starts_with(line);

        // One timeline: the plain lines first, since what Crook recorded
        // happened since the shell's file was read; then the records. A plain
        // line a record knows about is left to the record, which knows how
        // it ended: the field's own history holds every line sent, and a
        // command that failed would otherwise come back through it unscored.
        let plain = plain
            .iter()
            .enumerate()
            .filter(|(_, command)| extends(command) && !self.known.contains_key(command.as_str()))
            .map(|(at, command)| ((0, at), command.as_str()));
        let recorded = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| extends(&record.command))
            .map(|(at, record)| {
                (
                    (score(record, context), plain_len_offset(at)),
                    record.command.as_str(),
                )
            });
        plain
            .chain(recorded)
            .max_by_key(|(rank, _)| *rank)
            .map(|(_, command)| command)
    }
}

/// A record's place on the timeline after every plain line. Plain lines are
/// at most the field's history, far fewer than this.
fn plain_len_offset(at: usize) -> usize {
    usize::MAX / 2 + at
}

/// How well a record fits where the field is.
///
/// Small numbers on purpose, so the order is the one a person can say out
/// loud: ran in this directory, then ran after the command just run, and a
/// failure costs more than either earns alone — a corrected command run
/// once here beats the typo run three times before it.
pub fn score(record: &Record, context: &Context) -> i32 {
    let mut score = 0;
    if record.directory.is_some() && record.directory == context.directory {
        score += 4;
    }
    if record.after.is_some() && record.after == context.previous {
        score += 2;
    }
    if record.exit.is_some_and(|exit| exit != 0) {
        score -= 5;
    }
    score
}

/// The process's recall, read from the data directory the first time it is
/// asked for.
#[cfg(not(test))]
fn user() -> &'static Mutex<Recall> {
    static RECALL: OnceLock<Mutex<Recall>> = OnceLock::new();
    RECALL.get_or_init(|| {
        let recall = dirs::data_dir()
            .map(|directory| directory.join("crook").join("history.jsonl"))
            .map_or_else(Recall::default, Recall::open);
        Mutex::new(recall)
    })
}

/// Runs `look` over the process's recall.
#[cfg(not(test))]
pub fn with_user<T>(look: impl FnOnce(&Recall) -> T) -> T {
    look(&user().lock().unwrap_or_else(PoisonError::into_inner))
}

/// Remembers `record` in the process's recall.
#[cfg(not(test))]
pub fn remember(record: Record) {
    user()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remember(record);
}

// Under test, one recall per thread, in memory: every test runs on a thread of
// its own, and the commands one test's shells run must not become another
// test's suggestions.
#[cfg(test)]
thread_local! {
    static RECALL: std::cell::RefCell<Recall> = std::cell::RefCell::new(Recall::default());
}

/// Runs `look` over this test thread's recall.
#[cfg(test)]
pub fn with_user<T>(look: impl FnOnce(&Recall) -> T) -> T {
    RECALL.with(|recall| look(&recall.borrow()))
}

/// Remembers `record` in this test thread's recall.
#[cfg(test)]
pub fn remember(record: Record) {
    RECALL.with(|recall| recall.borrow_mut().remember(record));
}

/// Seconds since the epoch, now.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The records in `text`, one JSON object a line, the newest [`LIMIT`].
pub fn parse(text: &str) -> Vec<Record> {
    let mut records: Vec<Record> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let over = records.len().saturating_sub(LIMIT);
    records.drain(..over);
    records
}

/// Adds one record to the end of the file, creating it and its directory.
///
/// One `write` of one line in append mode, so that two Crooks ending a
/// command at the same moment put down two whole lines rather than one
/// line interleaved into the other. A record that cannot be written is a
/// line in the log: the suggestion this costs is not worth an error.
fn append(path: &Path, record: &Record) {
    let Ok(mut line) = serde_json::to_string(record) else {
        return;
    };
    line.push('\n');
    if let Some(directory) = path.parent() {
        let _ = fs::create_dir_all(directory);
    }
    let written = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
    if let Err(error) = written {
        log::debug!(
            "could not remember a command in {}: {error}",
            path.display()
        );
    }
}

/// The last [`TAIL`] bytes of the file, from the first whole line in them.
fn read_tail(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let cut = length > TAIL;
    if cut {
        file.seek(SeekFrom::End(-(TAIL as i64))).ok()?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(match (cut, text.find('\n')) {
        (true, Some(at)) => text[at + 1..].to_owned(),
        _ => text,
    })
}

/// Rewrites a file grown past [`TRIM_AT`] to hold only its newest records.
///
/// Through a temporary renamed into place, so that a Crook reading it meets
/// the old file or the new one and never half of either. A record another
/// Crook appends between the read and the rename is lost, which costs one
/// suggestion once in a few thousand commands.
fn trim(path: &Path) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.len() <= TRIM_AT {
        return;
    }
    let Some(text) = read_tail(path) else {
        return;
    };
    let mut kept = String::new();
    for record in parse(&text) {
        if let Ok(line) = serde_json::to_string(&record) {
            kept.push_str(&line);
            kept.push('\n');
        }
    }
    let temporary = path.with_extension(format!("jsonl.{}", std::process::id()));
    if fs::write(&temporary, kept)
        .and_then(|()| fs::rename(&temporary, path))
        .is_err()
    {
        let _ = fs::remove_file(&temporary);
    }
}

#[cfg(test)]
#[path = "recall_tests.rs"]
mod tests;
