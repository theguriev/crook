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
    let written = write(scratch.path(), Channel::Stable, "20260929-101500", "report")
        .expect("the report is written");

    assert_eq!(
        written.file_name().and_then(|name| name.to_str()),
        Some("crook-stable-20260929-101500.txt")
    );
    assert_eq!(unseen(scratch.path(), Channel::Stable), Some(written));
}

#[test]
fn the_next_launch_offers_the_newest_report_nobody_has_seen() {
    let scratch = Scratch::new("newest");
    write(scratch.path(), Channel::Stable, "20260928-090000", "older").expect("written");
    let newer =
        write(scratch.path(), Channel::Stable, "20260929-101500", "newer").expect("written");
    write(scratch.path(), Channel::Stable, "20260927-080000", "oldest").expect("written");

    assert_eq!(unseen(scratch.path(), Channel::Stable), Some(newer));
}

#[test]
fn a_report_marked_seen_is_not_offered_again_and_is_still_there() {
    let scratch = Scratch::new("seen");
    let older =
        write(scratch.path(), Channel::Stable, "20260928-090000", "older").expect("written");
    let newer =
        write(scratch.path(), Channel::Stable, "20260929-101500", "newer").expect("written");

    let seen = mark_seen(&newer).expect("the report is renamed");

    assert_eq!(
        seen.file_name().and_then(|name| name.to_str()),
        Some("crook-stable-20260929-101500.seen.txt")
    );
    assert_eq!(
        fs::read_to_string(&seen).expect("the seen report reads"),
        "newer"
    );
    assert_eq!(
        unseen(scratch.path(), Channel::Stable),
        Some(older.clone()),
        "the seen report was offered, or the unseen one behind it was not"
    );

    mark_seen(&older).expect("the older report is renamed");
    assert_eq!(unseen(scratch.path(), Channel::Stable), None);
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
    write(scratch.path(), Channel::Dev, "20260929-101500", "dev").expect("written");

    assert_eq!(unseen(scratch.path(), Channel::Stable), None);
    assert!(unseen(scratch.path(), Channel::Dev).is_some());
}

#[test]
fn a_folder_with_no_reports_offers_nothing() {
    let scratch = Scratch::new("empty");
    assert_eq!(unseen(scratch.path(), Channel::Stable), None);
}

#[test]
fn only_the_newest_five_reports_of_a_channel_are_kept_seen_or_not() {
    let scratch = Scratch::new("rotation");
    write(scratch.path(), Channel::Dev, "20200101-000000", "dev").expect("written");
    for minute in 0..7 {
        let written = write(
            scratch.path(),
            Channel::Stable,
            &format!("20260929-10{minute:02}00"),
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
            "crook-dev-20200101-000000.txt",
            "crook-stable-20260929-100200.seen.txt",
            "crook-stable-20260929-100300.txt",
            "crook-stable-20260929-100400.seen.txt",
            "crook-stable-20260929-100500.txt",
            "crook-stable-20260929-100600.seen.txt",
        ]
    );
}

#[test]
fn two_panics_in_one_run_are_two_reports() {
    let scratch = Scratch::new("twice");
    let first = write(scratch.path(), Channel::Stable, "20260929-101500", "first").expect("one");
    let second = write(scratch.path(), Channel::Stable, "20260929-101500", "second").expect("two");

    assert_ne!(first, second);
    assert_eq!(fs::read_to_string(first).expect("reads"), "first");
    assert_eq!(fs::read_to_string(second).expect("reads"), "second");
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
        .output()
        .expect("the test binary runs again");
    assert!(
        !output.status.success(),
        "the child did not panic: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let report = unseen(scratch.path(), Channel::Stable).unwrap_or_else(|| {
        panic!(
            "the hook wrote no report; the child said {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let text = fs::read_to_string(report).expect("the report reads");
    for wanted in [
        env!("CARGO_PKG_VERSION"),
        "Channel  stable",
        CHILD_MESSAGE,
        "crash_tests.rs",
        "Backtrace",
    ] {
        assert!(
            text.contains(wanted),
            "the report lacks {wanted:?}:\n{text}"
        );
    }
}

#[test]
#[ignore = "run by a_real_panic_goes_through_the_installed_hook_into_a_report, in a process of its own"]
fn panics_with_the_hook_installed() {
    // Run by hand, with nowhere to write, it does nothing: an ignored test
    // somebody runs with `--ignored` must not install a hook over their run.
    let Some(folder) = std::env::var_os(CHILD_FOLDER) else {
        return;
    };
    install(
        PathBuf::from(folder),
        Channel::Stable,
        String::from("20260929-101500"),
    );
    panic!("{CHILD_MESSAGE}");
}
