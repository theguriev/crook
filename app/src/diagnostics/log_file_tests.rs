//! What reaches the file, what is kept for a report, and what is thrown away.
//!
//! Every case builds a [`Tee`] of its own rather than touching the one the
//! process logs through: `log` takes one logger per process, and a test that
//! installed it would be deciding for every other test what logging does.

use std::fs;

use log::{Level, LevelFilter, Log, Record};

use super::*;
use crate::diagnostics::Scratch;

/// A logger that lets through what `level` and more severe, printing its
/// stderr copy where the test harness captures it.
fn tee(level: LevelFilter) -> Tee {
    Tee::new(stderr(level))
}

/// The stderr half on its own, for [`Tee::limited_to`].
fn stderr(level: LevelFilter) -> env_logger::Logger {
    env_logger::Builder::new()
        .filter_level(level)
        .is_test(true)
        .build()
}

/// Logs `message` at `level` from `target`, the way `log::info!` would.
fn say(tee: &Tee, level: Level, target: &str, message: &str) {
    tee.log(
        &Record::builder()
            .level(level)
            .target(target)
            .args(format_args!("{message}"))
            .build(),
    );
}

/// The names in `directory`, sorted.
fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .expect("the folder reads")
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    names.sort();
    names
}

/// The one file in `directory`, read whole.
fn the_file(directory: &Path) -> String {
    let files: Vec<_> = fs::read_dir(directory)
        .expect("the log folder exists")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(files.len(), 1, "expected one log file: {files:?}");
    fs::read_to_string(&files[0]).expect("the log file reads")
}

#[test]
fn a_line_the_filter_lets_through_reaches_the_file_with_its_level_and_where_it_came_from() {
    let scratch = Scratch::new("reaches");
    let tee = tee(LevelFilter::Info);
    let path = tee
        .open(scratch.path(), Channel::Stable, "20260929T081500Z")
        .expect("the file opens");

    say(
        &tee,
        Level::Warn,
        "crook::settings",
        "the settings file is a folder",
    );

    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("crook-stable-20260929T081500Z.log")
    );
    let file = the_file(scratch.path());
    assert!(
        file.contains("WARN  crook::settings] the settings file is a folder\n"),
        "the line is not in the file as it was said: {file:?}"
    );
}

#[test]
fn a_line_the_filter_holds_back_reaches_neither_the_file_nor_a_report() {
    // One filter, and it is the one stderr has always had: `RUST_LOG=warn`
    // keeps a debug line out of the file exactly as it keeps it off the
    // terminal, and the default keeps the file to what the terminal shows.
    let scratch = Scratch::new("filtered");
    let tee = tee(LevelFilter::Info);
    tee.open(scratch.path(), Channel::Stable, "20260929T081500Z")
        .expect("the file opens");

    say(&tee, Level::Debug, "crook::git", "asked git for the branch");
    say(&tee, Level::Info, "crook::git", "the branch is main");

    let file = the_file(scratch.path());
    assert!(
        !file.contains("asked git"),
        "a filtered line was written: {file:?}"
    );
    assert!(file.contains("the branch is main"), "{file:?}");
    assert!(
        tee.recent().iter().all(|line| !line.contains("asked git")),
        "a filtered line was kept for a report"
    );
}

#[test]
fn what_was_said_before_the_file_opened_is_the_start_of_it() {
    // The logger is up from the first line of `run`, and the file only once a
    // window has been asked for. Whatever was said in between is still this
    // run's, and belongs at the top of its file.
    let scratch = Scratch::new("before");
    let tee = tee(LevelFilter::Info);
    say(&tee, Level::Info, "crook", "before the file");

    tee.open(scratch.path(), Channel::Stable, "20260929T081500Z")
        .expect("the file opens");
    say(&tee, Level::Info, "crook", "after the file");

    let file = the_file(scratch.path());
    let before = file
        .find("before the file")
        .expect("the early line is there");
    let after = file
        .find("after the file")
        .expect("the later line is there");
    assert!(before < after, "{file:?}");
    assert_eq!(
        file.matches("before the file").count(),
        1,
        "the early line was written twice: {file:?}"
    );
}

#[test]
fn a_report_carries_the_last_two_hundred_lines_and_no_more() {
    let tee = tee(LevelFilter::Info);
    for number in 0..RECENT + 50 {
        say(&tee, Level::Info, "crook", &format!("line {number}"));
    }

    let recent = tee.recent();
    assert_eq!(recent.len(), RECENT);
    assert!(
        recent[0].contains("line 50\n"),
        "the oldest line kept is {:?}",
        recent[0]
    );
    assert!(recent[RECENT - 1].contains(&format!("line {}\n", RECENT + 49)));
}

#[test]
fn a_note_reaches_the_file_and_a_report_but_not_stderr_s_filter() {
    // A panic's line and a failed startup's error are on stderr already, in
    // their own words; the note is how they reach the file as well.
    let scratch = Scratch::new("note");
    let tee = tee(LevelFilter::Info);
    tee.open(scratch.path(), Channel::Dev, "20260929T081500Z")
        .expect("the file opens");

    tee.note("no usable GPU adapter was found");

    let file = the_file(scratch.path());
    assert!(
        file.contains("ERROR crook] no usable GPU adapter was found\n"),
        "{file:?}"
    );
    assert!(
        tee.recent()
            .iter()
            .any(|line| line.contains("no usable GPU adapter")),
        "the note is not among the recent lines"
    );
}

#[test]
fn only_the_newest_five_logs_of_a_channel_are_kept() {
    let scratch = Scratch::new("rotation");
    let tee = tee(LevelFilter::Info);
    // Another channel's file, which this channel's pruning must leave alone:
    // a dev build and a shipped one share the folder.
    fs::create_dir_all(scratch.path()).expect("the folder is made");
    fs::write(
        scratch.path().join("crook-dev-20200101T000000Z.log"),
        "the dev build's",
    )
    .expect("the other channel's file is written");

    for minute in 0..7 {
        tee.open(
            scratch.path(),
            Channel::Stable,
            &format!("20260929T08{minute:02}00Z"),
        )
        .expect("the file opens");
    }

    assert_eq!(
        names(scratch.path()),
        [
            "crook-dev-20200101T000000Z.log",
            "crook-stable-20260929T080200Z.log",
            "crook-stable-20260929T080300Z.log",
            "crook-stable-20260929T080400Z.log",
            "crook-stable-20260929T080500Z.log",
            "crook-stable-20260929T080600Z.log",
        ]
    );
}

#[test]
fn a_log_another_window_is_still_writing_is_never_pruned() {
    // Crook is a process per window. The window opened first is the oldest
    // log by name, and five launches later it is still being written — and
    // it is the one a driver crash in that window leaves as all there is.
    let scratch = Scratch::new("live");
    let morning = tee(LevelFilter::Info);
    let live = morning
        .open(scratch.path(), Channel::Stable, "20260929T070000Z")
        .expect("the first window's file opens");

    // Each of these a launch that has since quit: the next open drops its
    // file, as a process that ended would.
    let launches = tee(LevelFilter::Info);
    for hour in 10..16 {
        launches
            .open(
                scratch.path(),
                Channel::Stable,
                &format!("20260929T{hour}0000Z"),
            )
            .expect("a later window's file opens");
    }

    assert!(live.exists(), "a live window's log was pruned");
    say(&morning, Level::Info, "crook", "still here in the evening");
    assert!(
        fs::read_to_string(&live)
            .expect("the live log reads")
            .contains("still here in the evening"),
        "the live window no longer writes the file it was given"
    );
    assert_eq!(
        names(scratch.path()).len(),
        crate::diagnostics::KEPT + 1,
        "the live log was spared and something else was not pruned"
    );

    // Once that window is gone, its log is one more old file.
    drop(morning);
    launches
        .open(scratch.path(), Channel::Stable, "20260929T160000Z")
        .expect("another window's file opens");
    assert!(!live.exists(), "a log nobody writes any more was kept");
}

#[test]
fn a_long_line_is_whole_in_the_file_and_the_start_of_it_in_a_report() {
    // A plugin's `log` import takes up to a megabyte. The file has a limit
    // of its own; the recent lines keep the start of each, so two hundred of
    // them are not two hundred megabytes in memory and in a report.
    let scratch = Scratch::new("long");
    let tee = tee(LevelFilter::Info);
    tee.open(scratch.path(), Channel::Stable, "20260929T081500Z")
        .expect("the file opens");
    // Multi-byte, so a cut at a byte count lands inside a character.
    let long = "é".repeat(LINE_BYTES);

    say(&tee, Level::Info, "crook::plugins", &long);

    assert!(
        the_file(scratch.path()).contains(&long),
        "the file does not have the whole line"
    );
    let recent = tee.recent();
    let kept = recent.last().expect("the line is among the recent ones");
    assert!(
        kept.len() < LINE_BYTES + 64,
        "a report would carry {} bytes of one line",
        kept.len()
    );
    assert!(
        kept.contains("crook::plugins] éé") && kept.ends_with(" bytes more]\n"),
        "the kept line does not start as the line did and say it was cut: {:?}",
        &kept[kept.len().saturating_sub(80)..]
    );
}

#[test]
fn two_launches_in_one_second_write_two_files() {
    let scratch = Scratch::new("same-second");
    let first = tee(LevelFilter::Info);
    let second = tee(LevelFilter::Info);
    let one = first
        .open(scratch.path(), Channel::Stable, "20260929T081500Z")
        .expect("the first file opens");
    let two = second
        .open(scratch.path(), Channel::Stable, "20260929T081500Z")
        .expect("the second file opens");

    say(&first, Level::Info, "crook", "the first launch");
    say(&second, Level::Info, "crook", "the second launch");

    assert_ne!(one, two);
    let one = fs::read_to_string(one).expect("the first file reads");
    assert!(one.contains("the first launch") && !one.contains("the second launch"));
}

#[test]
fn a_folder_that_cannot_be_made_is_an_error_and_logging_carries_on() {
    let scratch = Scratch::new("refused");
    fs::create_dir_all(scratch.path()).expect("the folder is made");
    // A file where the logs folder should be.
    let blocked = scratch.path().join("logs");
    fs::write(&blocked, "not a folder").expect("the blocker is written");
    let tee = tee(LevelFilter::Info);

    let refused = tee.open(&blocked, Channel::Stable, "20260929T081500Z");
    say(&tee, Level::Info, "crook", "still logging");

    assert!(refused.is_err(), "a file was opened under a file");
    assert!(
        tee.recent()
            .iter()
            .any(|line| line.contains("still logging")),
        "logging stopped with the file"
    );
}

#[test]
fn a_file_at_its_limit_stops_growing_and_says_why() {
    let scratch = Scratch::new("limit");
    let tee = Tee::limited_to(stderr(LevelFilter::Info), 400);
    tee.open(scratch.path(), Channel::Stable, "20260929T081500Z")
        .expect("the file opens");

    for number in 0..100 {
        say(&tee, Level::Info, "crook", &format!("line {number}"));
    }

    let file = the_file(scratch.path());
    assert!(
        file.len() <= 400 + FULL.len(),
        "the file grew past its limit: {} bytes",
        file.len()
    );
    assert!(
        file.ends_with(FULL),
        "the file does not say it stopped: {file:?}"
    );
    assert_eq!(file.matches(FULL).count(), 1, "{file:?}");
    assert!(
        tee.recent().iter().any(|line| line.contains("line 99")),
        "the recent lines stopped with the file"
    );
}
