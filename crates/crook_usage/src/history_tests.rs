//! What a week of transcripts has to add up to.

use std::io::Write as _;

use chrono::TimeZone as _;

use super::*;

/// A fixed "now", so the window and the day buckets are the same on every run.
fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap()
}

/// One assistant turn, as Claude Code writes it. Only the fields the scan
/// reads are here; the real line carries the whole answer beside them.
fn turn(id: &str, request: &str, model: &str, at: &str, output: u64) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{at}","requestId":"{request}",
            "sessionId":"session-1","cwd":"/home/someone/work/crook","gitBranch":"pirate",
            "message":{{"id":"{id}","model":"{model}","role":"assistant",
            "usage":{{"input_tokens":2,"output_tokens":{output},
            "cache_creation_input_tokens":100,"cache_read_input_tokens":1000}}}}}}"#
    )
    .replace('\n', "")
}

/// Writes `lines` as one session's transcript under a project directory.
fn transcript(root: &Path, project: &str, session: &str, lines: &[String]) {
    let directory = root.join(project);
    std::fs::create_dir_all(&directory).expect("a temporary directory");
    let mut file = std::fs::File::create(directory.join(format!("{session}.jsonl")))
        .expect("a transcript to write");
    for line in lines {
        writeln!(file, "{line}").expect("a line to write");
    }
}

/// A directory of this test's own, removed when the test ends.
struct Transcripts(PathBuf);

impl Transcripts {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("crook-usage-history-{name}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a temporary directory");
        Self(path)
    }
}

impl Drop for Transcripts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_turn_counted_twice_on_disk_is_counted_once_here() {
    let root = Transcripts::new("duplicates");
    let first = turn(
        "msg-1",
        "req-1",
        "claude-opus-5",
        "2026-09-04T09:00:00Z",
        500,
    );
    let second = turn(
        "msg-2",
        "req-2",
        "claude-opus-5",
        "2026-09-04T10:00:00Z",
        300,
    );

    transcript(
        &root.0,
        "project",
        "session-a",
        &[first.clone(), second.clone()],
    );
    // What a resumed session looks like: the turns it inherited, written again
    // under a new session's name.
    transcript(&root.0, "project", "session-b", &[first, second]);

    let history = read_history_in(&root.0, now());

    assert_eq!(
        history.requests, 2,
        "the resumed session's copies were counted again"
    );
    assert_eq!(history.by_model.len(), 1);
    assert_eq!(history.by_model[0].output, 800);
    assert_eq!(history.by_model[0].requests, 2);
}

#[test]
fn only_the_last_week_is_in_the_window() {
    let root = Transcripts::new("window");
    transcript(
        &root.0,
        "project",
        "session",
        &[
            turn(
                "old",
                "req-old",
                "claude-opus-5",
                "2026-08-20T09:00:00Z",
                900,
            ),
            turn(
                "new",
                "req-new",
                "claude-opus-5",
                "2026-09-03T09:00:00Z",
                100,
            ),
        ],
    );

    let history = read_history_in(&root.0, now());

    assert_eq!(
        history.requests, 1,
        "a turn from a fortnight ago is not this week's"
    );
    assert_eq!(history.by_model[0].output, 100);
    assert_eq!(history.since, now() - Duration::days(HISTORY_DAYS));
}

#[test]
fn the_models_are_ordered_by_what_they_moved_and_the_days_have_no_gaps() {
    let root = Transcripts::new("breakdown");
    transcript(
        &root.0,
        "project",
        "session",
        &[
            turn("a", "req-a", "claude-fable-5-1", "2026-09-04T09:00:00Z", 10),
            turn("b", "req-b", "claude-opus-5", "2026-09-04T10:00:00Z", 5_000),
            // The turn Claude Code writes for something no model answered.
            turn("c", "req-c", "<synthetic>", "2026-09-04T11:00:00Z", 0),
        ],
    );

    let history = read_history_in(&root.0, now());

    let names: Vec<String> = history
        .by_model
        .iter()
        .map(ModelUsage::display_name)
        .collect();
    assert_eq!(
        names,
        vec!["Opus 5", "Fable 5.1"],
        "the heaviest model goes first, and the synthetic turn is not a model"
    );

    assert_eq!(
        history.by_day.len(),
        HISTORY_DAYS as usize,
        "a quiet day is a bar of height zero, not a missing column"
    );
    assert!(
        history
            .by_day
            .windows(2)
            .all(|pair| pair[0].date < pair[1].date),
        "the days run oldest first"
    );
    assert_eq!(
        history.by_day.last().expect("seven days").tokens,
        history.tokens(),
        "every turn in this fixture is from today"
    );
}

#[test]
fn a_project_is_named_by_its_directory_and_its_busiest_branch() {
    let root = Transcripts::new("projects");
    transcript(
        &root.0,
        "project",
        "session",
        &[turn(
            "a",
            "req-a",
            "claude-opus-5",
            "2026-09-04T09:00:00Z",
            10,
        )],
    );

    let history = read_history_in(&root.0, now());

    assert_eq!(history.projects.len(), 1);
    assert_eq!(history.projects[0].name, "crook");
    assert_eq!(history.projects[0].branch.as_deref(), Some("pirate"));
    assert_eq!(history.sessions, 1);
}

#[test]
fn a_home_with_no_transcripts_in_it_is_an_empty_week_rather_than_an_error() {
    let root = Transcripts::new("empty");

    let history = read_history_in(&root.0, now());

    assert!(history.is_empty());
    assert_eq!(history.tokens(), 0);
    assert_eq!(
        history.by_day.len(),
        HISTORY_DAYS as usize,
        "the week is still seven days long when nothing happened in it"
    );
}

#[test]
fn a_model_is_named_the_way_a_person_writes_it() {
    let named = |model: &str| {
        ModelUsage {
            model: model.to_owned(),
            ..ModelUsage::default()
        }
        .display_name()
    };

    assert_eq!(named("claude-opus-5"), "Opus 5");
    assert_eq!(named("claude-fable-5-1"), "Fable 5.1");
    assert_eq!(named("claude-haiku-4-5-20251001"), "Haiku 4.5");
    // Anything the pattern does not fit is reported as it was recorded, which
    // is more useful than a guess: a name nobody recognises is still the name
    // in the transcript.
    assert_eq!(named("some-local-thing"), "some-local-thing");
    assert_eq!(named("gpt-4o"), "gpt-4o");
}
