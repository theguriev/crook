//! What a report says, where it goes, and which one the next launch offers.

use std::fs;

use super::*;
use crate::diagnostics::Scratch;

/// A panic as the hook would describe one, with nothing in it that depends
/// on the machine.
fn panic() -> Panic {
    Panic {
        message: String::from("index out of bounds: the len is 3 but the index is 7"),
        location: Some(String::from("app/src/workspace/view.rs:1204:17")),
        thread: String::from("main"),
        at: String::from("2026-09-29 10:15:02.118 +02:00"),
        backtrace: String::from("   0: crook::workspace::view::Workspace::save_problem\n"),
    }
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

#[test]
fn a_report_names_the_build_the_message_where_it_happened_and_what_led_up_to_it() {
    let recent = [
        String::from("[2026-09-29T10:15:01.900+02:00 INFO  crook] rendering with Vulkan\n"),
        String::from("[2026-09-29T10:15:02.100+02:00 WARN  crook::git] git is slow\n"),
    ];
    let text = report(
        Channel::Stable,
        &panic(),
        Some("Vulkan DiscreteGpu (Radeon)"),
        &recent,
    );

    for wanted in [
        env!("CARGO_PKG_VERSION"),
        "Channel  stable",
        std::env::consts::OS,
        "Thread   main",
        "Where    app/src/workspace/view.rs:1204:17",
        "index out of bounds: the len is 3 but the index is 7",
        "GPU      Vulkan DiscreteGpu (Radeon)",
        "crook::workspace::view::Workspace::save_problem",
        "The last 2 lines of the log:",
        "WARN  crook::git] git is slow\n",
        "has not been sent anywhere",
    ] {
        assert!(
            text.contains(wanted),
            "the report lacks {wanted:?}:\n{text}"
        );
    }
}

#[test]
fn a_report_from_before_the_gpu_opened_says_so() {
    let text = report(Channel::Dev, &panic(), None, &[]);
    assert!(text.contains("GPU      none opened yet"), "{text}");
}

#[test]
fn a_written_report_is_the_one_the_next_launch_finds() {
    let scratch = Scratch::new("found");
    let written = write(
        scratch.path(),
        Channel::Stable,
        "20260929T081500Z",
        "report",
    )
    .expect("the report is written");

    assert_eq!(
        written.file_name().and_then(|name| name.to_str()),
        Some("crook-stable-20260929T081500Z.txt")
    );
    assert_eq!(unseen(scratch.path(), Channel::Stable), [written]);
}

#[test]
fn the_next_launch_is_told_of_every_report_nobody_has_seen_oldest_first() {
    let scratch = Scratch::new("every");
    let older =
        write(scratch.path(), Channel::Stable, "20260928T070000Z", "older").expect("written");
    let newer =
        write(scratch.path(), Channel::Stable, "20260929T081500Z", "newer").expect("written");
    let oldest = write(
        scratch.path(),
        Channel::Stable,
        "20260927T060000Z",
        "oldest",
    )
    .expect("written");

    assert_eq!(
        unseen(scratch.path(), Channel::Stable),
        [oldest, older, newer]
    );
}

#[test]
fn a_report_marked_seen_is_not_offered_again_and_is_still_there() {
    let scratch = Scratch::new("seen");
    let older =
        write(scratch.path(), Channel::Stable, "20260928T070000Z", "older").expect("written");
    let newer =
        write(scratch.path(), Channel::Stable, "20260929T081500Z", "newer").expect("written");

    let seen = mark_seen(&newer).expect("the report is renamed");

    assert_eq!(
        seen.file_name().and_then(|name| name.to_str()),
        Some("crook-stable-20260929T081500Z.seen.txt")
    );
    assert_eq!(
        fs::read_to_string(&seen).expect("the seen report reads"),
        "newer"
    );
    assert_eq!(
        unseen(scratch.path(), Channel::Stable),
        std::slice::from_ref(&older),
        "the seen report was offered, or the unseen one beside it was not"
    );

    mark_seen(&older).expect("the older report is renamed");
    assert!(unseen(scratch.path(), Channel::Stable).is_empty());
    assert_eq!(
        mark_seen(&seen).expect("marking it again changes nothing"),
        seen
    );
}

#[test]
fn another_channel_s_report_is_not_this_one_s_to_offer() {
    // A dev build that panicked is its developer's business, not a line in
    // the window of the Crook they use for work.
    let scratch = Scratch::new("channels");
    write(scratch.path(), Channel::Dev, "20260929T081500Z", "dev").expect("written");

    assert!(unseen(scratch.path(), Channel::Stable).is_empty());
    assert_eq!(unseen(scratch.path(), Channel::Dev).len(), 1);
}

#[test]
fn a_folder_with_no_reports_offers_nothing() {
    let scratch = Scratch::new("empty");
    assert!(unseen(scratch.path(), Channel::Stable).is_empty());
}

#[test]
fn only_the_newest_five_reports_of_a_channel_are_kept_seen_or_not() {
    let scratch = Scratch::new("rotation");
    write(scratch.path(), Channel::Dev, "20200101T000000Z", "dev").expect("written");
    for minute in 0..7 {
        let written = write(
            scratch.path(),
            Channel::Stable,
            &format!("20260929T08{minute:02}00Z"),
            "report",
        )
        .expect("written");
        // Every other one looked at, so the count is of both kinds.
        if minute % 2 == 0 {
            mark_seen(&written).expect("renamed");
        }
    }

    assert_eq!(
        names(scratch.path()),
        [
            "crook-dev-20200101T000000Z.txt",
            "crook-stable-20260929T080200Z.seen.txt",
            "crook-stable-20260929T080300Z.txt",
            "crook-stable-20260929T080400Z.seen.txt",
            "crook-stable-20260929T080500Z.txt",
            "crook-stable-20260929T080600Z.seen.txt",
        ]
    );
}

#[test]
fn a_report_is_never_pruned_by_its_own_writing() {
    // A clock set back names a panic older than five reports already there.
    // It is still the report of the latest panic, and the one the hook's
    // line in the log points at.
    let scratch = Scratch::new("set-back");
    for hour in 10..15 {
        write(
            scratch.path(),
            Channel::Stable,
            &format!("20260929T{hour}0000Z"),
            "later",
        )
        .expect("written");
    }

    let latest = write(
        scratch.path(),
        Channel::Stable,
        "20260929T090000Z",
        "latest",
    )
    .expect("written");

    assert!(latest.exists(), "the report was pruned as it was written");
    assert_eq!(
        fs::read_to_string(&latest).expect("reads"),
        "latest",
        "the report was not left as written"
    );
}

#[test]
fn two_panics_in_one_second_are_two_reports_in_the_order_they_came() {
    let scratch = Scratch::new("twice");
    let first = write(scratch.path(), Channel::Stable, "20260929T081500Z", "first").expect("one");
    let second = write(
        scratch.path(),
        Channel::Stable,
        "20260929T081500Z",
        "second",
    )
    .expect("two");

    assert_ne!(first, second);
    assert_eq!(fs::read_to_string(&first).expect("reads"), "first");
    assert_eq!(fs::read_to_string(&second).expect("reads"), "second");
    assert_eq!(
        unseen(scratch.path(), Channel::Stable),
        [first, second],
        "the second report of a second does not sort after the first"
    );
}

#[test]
fn a_second_that_wrote_six_reports_keeps_the_last_five_it_wrote() {
    // The same order as the pruning sees it: the one that goes is the one
    // written first, and not the second, which is what a name that sorted
    // `-2` before `.` did.
    let scratch = Scratch::new("six");
    for number in 1..=6 {
        write(
            scratch.path(),
            Channel::Stable,
            "20260929T081500Z",
            &format!("report {number}"),
        )
        .expect("written");
    }

    let kept: Vec<String> = unseen(scratch.path(), Channel::Stable)
        .iter()
        .map(|report| fs::read_to_string(report).expect("reads"))
        .collect();
    assert_eq!(
        kept,
        ["report 2", "report 3", "report 4", "report 5", "report 6"]
    );
}

#[test]
fn a_second_that_wrote_more_reports_than_are_kept_keeps_the_last_it_wrote() {
    // Seven or more panics in one second — the reader threads of every pane
    // tripping over the same bug. The sixth report's pruning frees the
    // second's first name, and a seventh that took it again sorted before
    // `_02`: offered as the oldest, and the next one pruned while earlier
    // reports stayed.
    let scratch = Scratch::new("eight");
    for number in 1..=8 {
        write(
            scratch.path(),
            Channel::Stable,
            "20260929T081500Z",
            &format!("report {number}"),
        )
        .expect("written");
    }

    let kept: Vec<String> = unseen(scratch.path(), Channel::Stable)
        .iter()
        .map(|report| fs::read_to_string(report).expect("reads"))
        .collect();
    assert_eq!(
        kept,
        ["report 4", "report 5", "report 6", "report 7", "report 8"]
    );
}

#[test]
fn a_report_of_the_same_second_as_a_seen_one_never_takes_its_name() {
    // A report marked seen still holds its second's first mark. One written
    // into that second afterwards — under a clock set back, say — that took
    // the name back would be renamed over the first when it was marked seen
    // in its turn, and the first report would be gone.
    let scratch = Scratch::new("seen-second");
    let first = write(scratch.path(), Channel::Stable, "20260929T081500Z", "first").expect("one");
    let first = mark_seen(&first).expect("the first report is renamed");
    let second = write(
        scratch.path(),
        Channel::Stable,
        "20260929T081500Z",
        "second",
    )
    .expect("two");
    let second = mark_seen(&second).expect("the second report is renamed");

    assert_ne!(first, second);
    assert_eq!(
        fs::read_to_string(&first).expect("the first report reads"),
        "first",
        "marking the second report seen wrote over the first"
    );
    assert_eq!(
        fs::read_to_string(&second).expect("the second report reads"),
        "second"
    );
}

/// Where [`panics_with_the_hook_installed`] writes, set by the test that
/// starts it.
const CHILD_FOLDER: &str = "CROOK_TEST_CRASH_FOLDER";

/// What the child panics with, so the report can be told from any other.
const CHILD_MESSAGE: &str = "the child panicked on purpose";

#[test]
fn a_real_panic_goes_through_the_installed_hook_into_a_report() {
    // The hook is process-wide, and a test that installed it here would be
    // writing a report for every other test's panic for as long as this
    // binary ran. So it is installed in a process of its own: this test
    // binary again, asked to run the one ignored test below, which installs
    // the hook and panics.
    let scratch = Scratch::new("child");
    let binary = std::env::current_exe().expect("the test binary knows where it is");
    let output = crate::process::command(binary.to_str().expect("the path is text"))
        .args([
            "--exact",
            "diagnostics::crash::tests::panics_with_the_hook_installed",
            "--ignored",
            "--test-threads=1",
        ])
        .env(CHILD_FOLDER, scratch.path())
        // Unset, as it is for anybody who opened Crook from the Dock, so the
        // backtrace in the report is there because the hook asked for one.
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .output()
        .expect("the test binary runs again");
    assert!(
        !output.status.success(),
        "the child did not panic: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let reports = unseen(scratch.path(), Channel::Stable);
    let [report] = reports.as_slice() else {
        panic!(
            "the hook wrote {} reports, not one; the child said {}{}",
            reports.len(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    };
    let text = fs::read_to_string(report).expect("the report reads");
    for wanted in [env!("CARGO_PKG_VERSION"), "Channel  stable", CHILD_MESSAGE] {
        assert!(
            text.contains(wanted),
            "the report lacks {wanted:?}:\n{text}"
        );
    }

    // Where the panic was, from the hook's own location and not from a frame
    // of the backtrace that happens to be in the same file.
    let place = text
        .lines()
        .find_map(|line| line.strip_prefix("Where    "))
        .unwrap_or_else(|| panic!("the report has no Where line:\n{text}"));
    assert!(
        place.contains("crash_tests.rs:"),
        "the report does not say where the panic was: {place:?}"
    );

    // A backtrace that names the function that panicked — which a backtrace
    // that was only asked for when `RUST_BACKTRACE` is set would not, since it
    // is not. With the crate's name, which the Thread line, the test's name
    // as libtest gives it, does not have.
    let backtrace = text
        .split_once(BACKTRACE)
        .and_then(|(_, rest)| rest.split_once("\nThe last "))
        .map(|(backtrace, _)| backtrace)
        .unwrap_or_else(|| panic!("the report has no backtrace section:\n{text}"));
    assert!(
        backtrace.contains("crook::diagnostics::crash::tests::panics_with_the_hook_installed"),
        "the backtrace does not name the function that panicked:\n{backtrace}"
    );
}

#[test]
#[ignore = "run by a_real_panic_goes_through_the_installed_hook_into_a_report, in a process of its own"]
fn panics_with_the_hook_installed() {
    // Run by hand, with nowhere to write, it does nothing: an ignored test
    // somebody runs with `--ignored` must not install a hook over their run.
    let Some(folder) = std::env::var_os(CHILD_FOLDER) else {
        return;
    };
    install(PathBuf::from(folder), Channel::Stable);
    panic!("{CHILD_MESSAGE}");
}
