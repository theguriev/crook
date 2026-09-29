//! Executable specification for the one question Crook asks a forge.
//!
//! The reading cases are pure and run anywhere: what `gh pr view --json`
//! prints is written out here as gh prints it. The rest run a fake `gh` — a
//! shell script that prints what the real one would and exits the way it
//! would — so what they prove is the whole path, from starting the program to
//! the sentence on the card, with no network and no account. The scripts are
//! `sh`, so those are Unix-only; a missing `gh` is a path that does not exist,
//! which every platform can be handed.

use std::time::Duration;

use super::*;

const URL: &str = "https://github.com/theguriev/crook/pull/398";

/// What `gh pr view --json state,statusCheckRollup` prints, for a state and
/// a list of checks already in gh's own shape.
fn printed(state: &str, checks: &str) -> String {
    format!(r#"{{"state":"{state}","statusCheckRollup":[{checks}]}}"#)
}

/// A check run as gh lists one: a status, and a conclusion once complete.
fn run(status: &str, conclusion: &str) -> String {
    format!(
        r#"{{"__typename":"CheckRun","name":"ci","workflowName":"Crook CI","status":"{status}","conclusion":"{conclusion}","startedAt":"2026-09-29T10:00:00Z","completedAt":"2026-09-29T10:05:00Z","detailsUrl":"https://github.com/theguriev/crook/actions/runs/1"}}"#
    )
}

/// A commit status as gh lists one: the older API, with a single state.
fn status(state: &str) -> String {
    format!(
        r#"{{"__typename":"StatusContext","context":"ci/external","state":"{state}","startedAt":"2026-09-29T10:00:00Z","targetUrl":"https://ci.example.com/1"}}"#
    )
}

#[test]
fn an_open_pull_request_with_passing_checks_reads_as_one() {
    let checks = [
        run("COMPLETED", "SUCCESS"),
        run("COMPLETED", "SKIPPED"),
        status("SUCCESS"),
    ]
    .join(",");
    let found = read(printed("OPEN", &checks).as_bytes()).expect("gh's own shape");
    assert_eq!(State::Open, found.state);
    assert_eq!(
        Checks {
            passing: 3,
            failing: 0,
            pending: 0
        },
        found.checks
    );
    assert_eq!("Open \u{b7} 3 checks passing", found.summary());
}

#[test]
fn one_failing_check_is_what_the_summary_says_first() {
    let checks = [
        run("COMPLETED", "SUCCESS"),
        run("COMPLETED", "FAILURE"),
        run("IN_PROGRESS", ""),
        status("ERROR"),
    ]
    .join(",");
    let found = read(printed("OPEN", &checks).as_bytes()).unwrap();
    assert_eq!(
        Checks {
            passing: 1,
            failing: 2,
            pending: 1
        },
        found.checks
    );
    // Failing before pending: a check still running changes nothing about
    // the one that already failed.
    assert_eq!("Open \u{b7} 2 of 4 checks failing", found.summary());

    let waiting = [
        run("QUEUED", ""),
        status("PENDING"),
        run("COMPLETED", "SUCCESS"),
    ]
    .join(",");
    assert_eq!(
        "Open \u{b7} 2 of 3 checks pending",
        read(printed("OPEN", &waiting).as_bytes())
            .unwrap()
            .summary()
    );
}

#[test]
fn a_merged_or_closed_pull_request_says_so() {
    let merged = read(printed("MERGED", &run("COMPLETED", "SUCCESS")).as_bytes()).unwrap();
    assert_eq!(State::Merged, merged.state);
    assert_eq!("Merged \u{b7} 1 check passing", merged.summary());

    let closed = read(printed("CLOSED", "").as_bytes()).unwrap();
    assert_eq!(State::Closed, closed.state);
    assert_eq!("Closed \u{b7} no checks", closed.summary());
}

#[test]
fn an_answer_that_is_not_gh_s_shape_is_unreadable_rather_than_guessed() {
    for answer in ["", "not json", r#"{"state":"DRAFT"}"#, r#"{"checks":[]}"#] {
        assert_eq!(
            Err(CheckError::Unreadable),
            read(answer.as_bytes()),
            "{answer:?}"
        );
    }
}

#[test]
fn gh_s_failures_are_told_apart_by_what_it_says() {
    // Exit 4 is gh's own code for "authentication required", and the
    // sentence it prints is the same when a token has gone bad.
    assert_eq!(
        CheckError::SignedOut,
        refused(
            Some(4),
            "To get started with GitHub CLI, please run:  gh auth login\n"
        )
    );
    assert_eq!(
        CheckError::SignedOut,
        refused(
            Some(1),
            "HTTP 401: Bad credentials (https://api.github.com/graphql)\nTry authenticating with:  gh auth login\n"
        )
    );
    // Exit 4 alone says it, whatever a later gh prints beside it.
    assert_eq!(
        CheckError::SignedOut,
        refused(Some(4), "authentication required\n")
    );
    assert_eq!(
        CheckError::Offline,
        refused(
            Some(1),
            "error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com\n"
        )
    );
    assert_eq!(
        CheckError::Offline,
        refused(
            Some(1),
            "Post \"https://api.github.com/graphql\": dial tcp: lookup api.github.com: no such host\n"
        )
    );
    // Anything else is gh's own first line, which is the most anybody here
    // knows about it.
    assert_eq!(
        CheckError::Refused(
            "GraphQL: Could not resolve to a PullRequest with the number of 99999. (repository.pullRequest)"
                .to_owned()
        ),
        refused(
            Some(1),
            "\nGraphQL: Could not resolve to a PullRequest with the number of 99999. (repository.pullRequest)\n"
        )
    );
    assert_eq!(
        CheckError::Refused("gh exited with status 1 and said nothing".to_owned()),
        refused(Some(1), "  \n")
    );
}

#[test]
fn every_failure_says_what_to_do_in_words() {
    // The card shows these as they are, so each has to be a sentence a
    // person can act on — and the three the brief names say the three
    // things they are.
    assert!(CheckError::Missing.to_string().contains("gh"));
    assert!(CheckError::Missing.to_string().contains("not installed"));
    assert!(CheckError::SignedOut.to_string().contains("gh auth login"));
    assert!(CheckError::Offline.to_string().contains("network"));
    assert!(
        CheckError::TimedOut(Duration::from_secs(20))
            .to_string()
            .contains("20 s")
    );
}

#[test]
fn a_gh_that_is_not_there_is_said_to_be_missing() {
    let nowhere = std::env::temp_dir()
        .join("crook-forge-no-such-directory")
        .join("gh");
    assert_eq!(
        Err(CheckError::Missing),
        check(&nowhere.to_string_lossy(), URL, TIMEOUT)
    );
}

// --- a fake gh ---------------------------------------------------------------

/// The cases that run a `gh` of their own: a shell script, so Unix only.
#[cfg(unix)]
mod with_a_fake_gh {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    /// A directory under the system temp directory, removed when it drops.
    struct ScratchDir {
        path: PathBuf,
    }

    impl ScratchDir {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "crook-forge-{label}-{}-{unique}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("the system temp directory is writable");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// A `gh` that records its arguments beside itself and then runs `body`.
    fn fake_gh(scratch: &ScratchDir, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt as _;

        let gh = scratch.path().join("gh");
        let asked = scratch.path().join("asked");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n{body}\n",
                asked.display()
            ),
        )
        .expect("the scratch directory is writable");
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))
            .expect("the script can be made executable");
        gh.to_string_lossy().into_owned()
    }

    #[test]
    fn a_fake_gh_that_says_merged_is_read_as_merged_and_was_asked_the_one_question() {
        let scratch = ScratchDir::new("merged");
        let gh = fake_gh(
            &scratch,
            &format!(
                "cat <<'EOF'\n{}\nEOF",
                printed("MERGED", &run("COMPLETED", "SUCCESS"))
            ),
        );

        let found = check(&gh, URL, TIMEOUT).expect("the fake answers");
        assert_eq!(State::Merged, found.state);
        assert_eq!("Merged \u{b7} 1 check passing", found.summary());

        // One question, about that address, for those two fields, and nothing
        // else — the press asks what it says it asks.
        let asked = std::fs::read_to_string(scratch.path().join("asked")).unwrap();
        assert_eq!(
            vec!["pr", "view", URL, "--json", "state,statusCheckRollup"],
            asked.lines().collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_fake_gh_with_an_open_pull_request_and_a_failing_check_says_both() {
        let scratch = ScratchDir::new("failing");
        let checks = [run("COMPLETED", "FAILURE"), run("COMPLETED", "SUCCESS")].join(",");
        let gh = fake_gh(
            &scratch,
            &format!("cat <<'EOF'\n{}\nEOF", printed("OPEN", &checks)),
        );

        let found = check(&gh, URL, TIMEOUT).expect("the fake answers");
        assert_eq!(State::Open, found.state);
        assert_eq!("Open \u{b7} 1 of 2 checks failing", found.summary());
    }

    #[test]
    fn a_fake_gh_that_is_signed_out_or_offline_is_told_apart() {
        let scratch = ScratchDir::new("signed-out");
        let gh = fake_gh(
            &scratch,
            "echo 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4",
        );
        assert_eq!(Err(CheckError::SignedOut), check(&gh, URL, TIMEOUT));

        let scratch = ScratchDir::new("offline");
        let gh = fake_gh(
            &scratch,
            "echo 'error connecting to api.github.com' >&2\nexit 1",
        );
        assert_eq!(Err(CheckError::Offline), check(&gh, URL, TIMEOUT));
    }

    #[test]
    fn a_gh_that_never_answers_is_given_up_on_at_the_deadline() {
        // `exec` so that the kill reaches the sleep itself: a shell that forked
        // it would leave a `sleep` holding the pipes, which is the case the
        // readers' own grace exists for and not the one this is about.
        let scratch = ScratchDir::new("hung");
        let gh = fake_gh(&scratch, "exec sleep 5");
        let deadline = Duration::from_millis(300);

        let started = Instant::now();
        let outcome = check(&gh, URL, deadline);
        let took = started.elapsed();

        assert_eq!(Err(CheckError::TimedOut(deadline)), outcome);
        assert!(
            took < Duration::from_secs(3),
            "gh was waited on for {took:?}, not killed at {deadline:?}"
        );
    }
}
