//! Executable specification for the deadline runner.
//!
//! What can be shown without git: that a process outliving its deadline is
//! killed and reaped, and that one finishing inside it is left alone. What a
//! call does when a hook git ran leaves the pipes held open is shown where the
//! hook runs, in `worktree_tests.rs`, because only a real `worktree add` runs
//! one.
//!
//! Unix only, because both cases spawn a Unix utility; on the other
//! platforms the module is empty rather than a pair of imports nothing uses.
#![cfg(unix)]

use std::process::Stdio;

use super::*;

#[test]
fn a_process_that_outlives_its_deadline_is_killed_and_reaped() {
    // `sleep` stands in for the git this exists to survive: one that has taken
    // a lock nobody will release, or is running a `post-checkout` hook waiting
    // on something that will never arrive. Fifty milliseconds against thirty
    // seconds leaves no doubt about which of the two ended the call.
    let mut child = command("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("sleep is on PATH");

    let started = std::time::Instant::now();
    let timeout = Duration::from_millis(50);
    let outcome = wait_for(&mut child, started + timeout, timeout);

    match outcome {
        Err(Failure::TimedOut { after }) => assert_eq!(after, timeout),
        other => panic!("expected TimedOut, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    // Killed *and* waited for. A process that is never reaped stays a zombie,
    // and a process id somebody else reaps can be handed out again to a
    // stranger — which is what makes a later kill dangerous rather than
    // useless.
    let reaped = child
        .try_wait()
        .expect("the child was waited for inside wait_for");
    assert!(matches!(reaped, Some(status) if !status.success()));
}

#[test]
fn a_process_that_finishes_in_time_is_not_killed() {
    let mut child = command("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("true is on PATH");

    let timeout = Duration::from_secs(30);
    let status =
        wait_for(&mut child, Instant::now() + timeout, timeout).expect("it exits immediately");

    assert!(status.success());
}

#[test]
fn a_directory_deleted_before_git_starts_is_not_taken_for_a_missing_git() {
    // The race Finish and the sweep run every time: a background read checks
    // that the checkout is there, the removal deletes it, and the spawn in it
    // fails with the `NotFound` a missing git binary gives too. `start` is
    // called past the callers' own check here, which is that race with the
    // gap held open. Latched, it said "git is not installed" until a restart.
    let gone = std::env::temp_dir().join(format!(
        "crook-run-tests-gone-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    assert!(
        !gone.exists(),
        "{} is not supposed to exist",
        gone.display()
    );

    let started = start(
        &gone,
        &[OsStr::new("--version")],
        Intent::Read,
        [Stdio::null(), Stdio::null(), Stdio::null()],
    );

    assert!(
        matches!(started, Err(Failure::NoDirectory)),
        "expected NoDirectory, got {:?}",
        started.map(|_| ())
    );
    assert!(
        !git_is_missing(),
        "a deleted directory switched git off for the session"
    );
}

#[test]
fn a_spawn_that_finds_nothing_in_a_directory_that_is_there_means_no_git() {
    // The other half of the rule, and why it still latches: the directory is
    // there after the failure, so it was the program that was not found.
    // Asked of the classifier rather than of a spawn, because a real one would
    // latch the switch for every test in this process.
    let here = std::env::temp_dir();
    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
    assert!(matches!(
        why_it_did_not_start(missing, &here),
        Failure::GitMissing
    ));

    let refused = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    assert!(matches!(
        why_it_did_not_start(refused, &here),
        Failure::CouldNotRun(_)
    ));
}
