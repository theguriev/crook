//! The week behind the percentage: what was spent, on which model, where.
//!
//! [`fetch_usage`](crate::fetch_usage) answers one question — how much of the
//! limit is gone — because that is all Claude's endpoint knows. It reports no
//! breakdown by model, none by day and none by project, and it never will:
//! those are not facts about a limit. They are facts about what this machine
//! did, and this machine already wrote them down.
//!
//! Claude Code keeps a transcript per session under
//! `~/.claude/projects/<slug>/<session>.jsonl`, one JSON object per line, and
//! every assistant turn in it carries the model that answered and the tokens
//! it cost. Reading them is the only way to say "Opus, 6.2M out" — so that is
//! what this does, and it does it the way the rest of the crate works: a
//! blocking read with no thread and no timer of its own, handed back as one
//! value the caller can draw.
//!
//! # Three things the format makes you do
//!
//! **Deduplicate.** A resumed or forked session copies the turns it inherited
//! into its own file, so the same answer is on disk several times — about two
//! in five lines here. Summing the files would overstate a heavy week by that
//! much, so every turn is keyed by the message and request it came from and
//! counted once.
//!
//! **Skip most lines without parsing them.** A week of transcripts is a couple
//! of hundred megabytes, nearly all of it the text of the conversation. Only
//! an assistant turn carries a `usage` object, so a line without that substring
//! is not JSON worth parsing — which turns the scan from seconds into a
//! fraction of one.
//!
//! **Trust the file's own timestamps, not its mtime.** Modification time is
//! only used to skip a file that cannot contain a turn inside the window: a
//! long session's file is touched every turn, so its early turns are older
//! than it is, and the window is applied per turn.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;

/// Where Claude Code writes its transcripts, relative to the home directory.
const TRANSCRIPTS_DIR: &str = ".claude/projects";

/// How many days the history covers, which is the window the weekly limit is
/// measured over.
pub const HISTORY_DAYS: i64 = 7;

/// How many projects are worth naming. Past a handful the list stops being a
/// summary and starts being a directory listing.
const TOP_PROJECTS: usize = 4;

/// Claude Code's placeholder for a turn it produced without a model — a
/// cancellation notice, an injected reminder. It costs nothing and naming it
/// in a breakdown of models would be a bug with a straight face.
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// What one model was used for over the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelUsage {
    /// The model's own id, as the transcript recorded it.
    pub model: String,
    /// Prompt tokens that were neither cached nor written to cache.
    pub input: u64,
    /// Tokens the model produced.
    pub output: u64,
    /// Prompt tokens written into the cache.
    pub cache_write: u64,
    /// Prompt tokens served from it.
    pub cache_read: u64,
    /// Assistant turns answered.
    pub requests: u64,
}

impl ModelUsage {
    /// Every token this model moved, which is what its share of the week is
    /// measured in.
    ///
    /// Cache reads dominate the sum by an order of magnitude on any session
    /// long enough to matter — that is what a cache is for — so this is a
    /// measure of *traffic*, not of what a token cost. What the sum is made of
    /// is broken out beside it wherever it is drawn, so a reader is never left
    /// to guess which of the four this was.
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_read
    }

    /// The model's name as a person writes it: `claude-opus-5` is Opus 5.
    ///
    /// Derived rather than looked up in a table, because a table would be
    /// wrong the week a new model ships and this is only ever a label. A name
    /// that does not fit the pattern is returned as it was recorded, which is
    /// still the most useful thing that can be said about it.
    pub fn display_name(&self) -> String {
        let name = self.model.strip_prefix("claude-").unwrap_or(&self.model);

        // A trailing release date is noise in a chip-sized panel:
        // `haiku-4-5-20251001` is Haiku 4.5.
        let mut parts: Vec<&str> = name.split('-').collect();
        if parts
            .last()
            .is_some_and(|part| part.len() == 8 && is_digits(part))
        {
            parts.pop();
        }

        let Some((family, version)) = parts.split_first() else {
            return self.model.clone();
        };
        if family.is_empty() || !version.iter().all(|part| is_digits(part)) {
            return self.model.clone();
        }

        let family = capitalized(family);
        if version.is_empty() {
            family
        } else {
            format!("{family} {}", version.join("."))
        }
    }
}

/// One day of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DayUsage {
    /// The day, in the machine's own time zone — a bar chart of days is read
    /// against the days the person lived, not against UTC.
    pub date: NaiveDate,
    /// Every token moved that day.
    pub tokens: u64,
}

/// One working directory's share of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectUsage {
    /// The directory's last component, which is what people call it.
    pub name: String,
    /// The branch most of its turns were on, when they were on one.
    pub branch: Option<String>,
    /// Every token moved in it.
    pub tokens: u64,
}

/// What the transcripts say about the last [`HISTORY_DAYS`] days.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageHistory {
    /// The oldest turn this counts.
    pub since: DateTime<Utc>,
    /// When it was read, so a panel can say how old it is.
    pub read_at: DateTime<Utc>,
    /// Models, heaviest first.
    pub by_model: Vec<ModelUsage>,
    /// Every day in the window, oldest first, including the quiet ones — a
    /// gap in a bar chart has to be a bar of height zero rather than a missing
    /// column, or the chart says the week was shorter than it was.
    pub by_day: Vec<DayUsage>,
    /// The busiest projects, heaviest first, at most [`TOP_PROJECTS`].
    pub projects: Vec<ProjectUsage>,
    /// How many distinct sessions were open in the window.
    pub sessions: u64,
    /// How many assistant turns they took.
    pub requests: u64,
}

impl UsageHistory {
    /// Every token moved in the window.
    pub fn tokens(&self) -> u64 {
        self.by_model.iter().map(ModelUsage::tokens).sum()
    }

    /// Whether the window holds nothing worth drawing.
    ///
    /// A fresh machine, a week off, or a home directory with no transcripts in
    /// it — three states with one answer, which is that the panel shows what it
    /// does know (the limits) and says the rest is empty rather than drawing an
    /// axis with nothing on it.
    pub fn is_empty(&self) -> bool {
        self.requests == 0
    }
}

/// Reads the transcripts and sums the last [`HISTORY_DAYS`] days of them.
///
/// Blocking, and proportional to what was written this week rather than to
/// what was ever written: call it from a background thread. A home directory
/// with no transcripts is not an error — it is an empty week, which is what a
/// machine that has never run Claude Code has.
pub fn read_history(now: DateTime<Utc>) -> anyhow::Result<UsageHistory> {
    match transcripts_dir() {
        Some(root) => Ok(read_history_in(&root, now)),
        // No home directory is the same answer as no transcripts in it.
        None => Ok(Scan::new(now - Duration::days(HISTORY_DAYS))
            .finish(now - Duration::days(HISTORY_DAYS), now)),
    }
}

/// The same read, against a directory named rather than found.
///
/// What [`read_history`] is once the home directory is known, and what a test
/// can point at a handful of transcripts it wrote itself.
pub(crate) fn read_history_in(root: &Path, now: DateTime<Utc>) -> UsageHistory {
    let since = now - Duration::days(HISTORY_DAYS);
    let mut scan = Scan::new(since);

    for path in transcript_files(root) {
        // A file untouched since before the window cannot hold a turn inside
        // it. The converse is not true — a long session's file is touched by
        // its newest turn while holding turns from days ago — so this only
        // ever skips, and every turn is dated by the line it is on.
        if modified_before(&path, since) {
            continue;
        }
        if let Err(err) = scan.read_file(&path) {
            // One unreadable transcript is a worse week's numbers, not a
            // failure: the rest of the files still say something true.
            log::warn!(
                "Failed to read the transcript at {}: {err:#}",
                path.display()
            );
        }
    }

    scan.finish(since, now)
}

/// The directory Claude Code keeps transcripts in, if this machine has a home.
fn transcripts_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(TRANSCRIPTS_DIR))
}

/// Every transcript under `root`, one directory deep, unsorted.
fn transcript_files(root: &Path) -> Vec<PathBuf> {
    let Ok(projects) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut files = Vec::new();
    for project in projects.flatten() {
        let Ok(sessions) = std::fs::read_dir(project.path()) else {
            continue;
        };
        files.extend(
            sessions
                .flatten()
                .map(|session| session.path())
                .filter(|path| path.extension().is_some_and(|kind| kind == "jsonl")),
        );
    }
    files
}

/// Whether `path` was last written before `since`, so it cannot hold a turn in
/// the window. A file whose time cannot be read is read rather than skipped.
fn modified_before(path: &Path, since: DateTime<Utc>) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(|modified| DateTime::<Utc>::from(modified) < since)
        .unwrap_or(false)
}

/// The running totals, and what has already been counted.
struct Scan {
    since: DateTime<Utc>,
    models: HashMap<String, ModelUsage>,
    days: HashMap<NaiveDate, u64>,
    projects: HashMap<String, ProjectTotals>,
    sessions: HashSet<String>,
    /// The turns already counted, so a transcript that inherited them from the
    /// session it was forked from does not count them again.
    counted: HashSet<(String, String)>,
    requests: u64,
}

/// One project's running totals, including which branch it was mostly on.
#[derive(Default)]
struct ProjectTotals {
    tokens: u64,
    branches: HashMap<String, u64>,
}

impl Scan {
    fn new(since: DateTime<Utc>) -> Self {
        Self {
            since,
            models: HashMap::new(),
            days: HashMap::new(),
            projects: HashMap::new(),
            sessions: HashSet::new(),
            counted: HashSet::new(),
            requests: 0,
        }
    }

    /// Adds every countable turn in one transcript.
    fn read_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let file = BufReader::new(File::open(path)?);
        for line in file.lines() {
            // A line that cannot be read as text is a partially written turn
            // at the end of a live session, which the next read will get.
            let Ok(line) = line else { break };
            if !line.contains(USAGE_FIELD) {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<Entry>(&line) else {
                continue;
            };
            self.add(entry);
        }
        Ok(())
    }

    /// Counts one turn, if it is one and has not been counted already.
    fn add(&mut self, entry: Entry) {
        if entry.kind.as_deref() != Some("assistant") {
            return;
        }
        let (Some(timestamp), Some(message)) = (entry.timestamp, entry.message) else {
            return;
        };
        if timestamp < self.since {
            return;
        }
        let (Some(model), Some(usage)) = (message.model, message.usage) else {
            return;
        };
        if model == SYNTHETIC_MODEL {
            return;
        }

        // Two ids rather than one: a message id is unique to an answer and a
        // request id to the call that produced it, and a turn missing either
        // is counted rather than dropped — there are a handful of them in a
        // week, and dropping is the worse error of the two.
        if let (Some(message_id), Some(request_id)) = (message.id, entry.request_id)
            && !self.counted.insert((message_id, request_id))
        {
            return;
        }

        let totals = self.models.entry(model).or_default();
        totals.input += usage.input_tokens;
        totals.output += usage.output_tokens;
        totals.cache_write += usage.cache_creation_input_tokens;
        totals.cache_read += usage.cache_read_input_tokens;
        totals.requests += 1;

        let tokens = usage.input_tokens
            + usage.output_tokens
            + usage.cache_creation_input_tokens
            + usage.cache_read_input_tokens;

        *self
            .days
            .entry(timestamp.with_timezone(&chrono::Local).date_naive())
            .or_default() += tokens;

        if let Some(cwd) = entry.cwd.as_deref().and_then(project_name) {
            let project = self.projects.entry(cwd).or_default();
            project.tokens += tokens;
            // A detached head reports "HEAD", which names nothing a person
            // would recognise; the project's own name is the better answer.
            if let Some(branch) = entry
                .git_branch
                .filter(|branch| !branch.is_empty() && branch != "HEAD")
            {
                *project.branches.entry(branch).or_default() += tokens;
            }
        }

        if let Some(session) = entry.session_id {
            self.sessions.insert(session);
        }
        self.requests += 1;
    }

    /// Turns the running totals into the history a panel draws.
    fn finish(self, since: DateTime<Utc>, now: DateTime<Utc>) -> UsageHistory {
        let mut by_model: Vec<ModelUsage> = self
            .models
            .into_iter()
            .map(|(model, mut totals)| {
                totals.model = model;
                totals
            })
            .collect();
        by_model.sort_by(|a, b| {
            b.tokens()
                .cmp(&a.tokens())
                .then_with(|| a.model.cmp(&b.model))
        });

        let today = now.with_timezone(&chrono::Local).date_naive();
        let by_day = (0..HISTORY_DAYS)
            .rev()
            .filter_map(|back| today.checked_sub_days(chrono::Days::new(back as u64)))
            .map(|date| DayUsage {
                date,
                tokens: self.days.get(&date).copied().unwrap_or_default(),
            })
            .collect();

        let mut projects: Vec<ProjectUsage> = self
            .projects
            .into_iter()
            .map(|(name, totals)| ProjectUsage {
                name,
                branch: totals
                    .branches
                    .into_iter()
                    .max_by_key(|(branch, tokens)| (*tokens, branch.clone()))
                    .map(|(branch, _)| branch),
                tokens: totals.tokens,
            })
            .collect();
        projects.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
        projects.truncate(TOP_PROJECTS);

        UsageHistory {
            since,
            read_at: now,
            by_model,
            by_day,
            projects,
            sessions: self.sessions.len() as u64,
            requests: self.requests,
        }
    }
}

/// The substring every countable line has and almost every other line has not.
const USAGE_FIELD: &str = "\"usage\"";

/// What a project is called: the last component of the directory worked in.
fn project_name(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

/// Whether every byte is an ASCII digit, which is what a version part is.
fn is_digits(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
}

/// `opus` as Opus, without dragging in a Unicode case table for names that are
/// ASCII in every model Anthropic has shipped.
fn capitalized(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// One line of a transcript, as far as this cares.
///
/// Every field is optional and unknown fields are ignored: the format is
/// Claude Code's own and it changes without notice, and a line this cannot
/// read is one turn missing from a total rather than a failure to report one.
#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
