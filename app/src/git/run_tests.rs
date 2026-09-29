//! Executable specification for the deadline runner.
//!
//! What can be shown without git: that a process outliving its deadline is
//! killed and reaped, and that one finishing inside it is left alone. What a
//! call does when a hook git ran leaves the pipes held open is shown where the
//! hook runs, in `worktree_tests.rs`, because only a real `worktree add` runs
//! one.

use std::process::Stdio;

use super::*;

#[cfg(unix)]
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

#[cfg(unix)]
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
