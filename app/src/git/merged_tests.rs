//! Executable specification for proving a branch landed.
//!
//! Every case builds a real repository and lands a branch on it the way a
//! forge does — a fast-forward, a squash, a stack squashed once or twice —
//! because the whole question is what git's own `patch-id` makes of what git's
//! own `merge --squash` wrote, and a fixture written by hand would answer it
//! for a git that does not exist. A machine with no git skips them with a
//! message rather than failing.
//!
//! The scratch harness is a copy of `worktree_tests.rs`'s, for the reason that
//! file gives: thirty lines duplicated beat a helper that has to be understood
//! from another module.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::process::command;

// --- scratch directories -----------------------------------------------------

/// A directory under the system temp directory, removed when it drops.
struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);

        let path = std::env::temp_dir().join(format!(
            "crook-merged-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("the system temp directory is writable");
        Self {
            path: crate::git::as_git_prints_it(path),
        }
    }

    /// An empty directory inside this one.
    fn dir(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        std::fs::create_dir_all(&path).expect("the scratch directory is writable");
        path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// --- running git ---------------------------------------------------------------

/// Whether to skip a test that needs git, saying so.
fn without_git(test: &str) -> bool {
    let installed = command("git")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok();
    if !installed {
        eprintln!("skipping {test}: git is not installed on this machine");
    }
    !installed
}

/// Runs git in `dir`, isolated from the machine's own configuration, and
/// returns its trimmed stdout. See `worktree_tests.rs` for why the isolation.
fn git(dir: &Path, args: &[&str]) -> String {
    let nowhere = dir.join("no-such-gitconfig");
    let output = command("git")
        .args(["-c", "init.defaultBranch=main"])
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", &nowhere)
        .env("GIT_CONFIG_SYSTEM", &nowhere)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Crook Tests")
        .env("GIT_AUTHOR_EMAIL", "tests@crook.invalid")
        .env("GIT_COMMITTER_NAME", "Crook Tests")
        .env("GIT_COMMITTER_EMAIL", "tests@crook.invalid")
        .stdin(Stdio::null())
        .output()
        .expect("git is installed; the caller checked");

    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository at `<scratch>/<name>` on `main`, with one commit.
fn repository(scratch: &ScratchDir, name: &str) -> PathBuf {
    let repo = scratch.dir(name);
    git(&repo, &["init"]);
    commit(&repo, "tracked.txt", "one\ntwo\nthree\n", "initial");
    repo
}

/// Writes `file` in `repo`, commits it on whatever is checked out, and returns
/// the new commit.
fn commit(repo: &Path, file: &str, contents: &str, message: &str) -> String {
    std::fs::write(repo.join(file), contents).expect("the scratch directory is writable");
    git(repo, &["add", file]);
    git(repo, &["commit", "--no-verify", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

/// Squashes `branch` onto `main` as one commit, the way a forge's "Squash and
/// merge" does, and returns that commit.
fn squash_onto_main(repo: &Path, branch: &str) -> String {
    git(repo, &["switch", "main"]);
    git(repo, &["merge", "--squash", branch]);
    git(
        repo,
        &[
            "commit",
            "--no-verify",
            "-m",
            &format!("{branch}, squashed"),
        ],
    );
    git(repo, &["rev-parse", "HEAD"])
}

const MAIN: &str = "refs/heads/main";

fn names(branches: &[&str]) -> Vec<String> {
    branches.iter().map(|name| (*name).to_owned()).collect()
}

// --- proving it ----------------------------------------------------------------

#[test]
fn a_fast_forward_merged_branch_is_proved_by_ancestry() {
    if without_git("a_fast_forward_merged_branch_is_proved_by_ancestry") {
        return;
    }
    let scratch = ScratchDir::new("ancestry");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "feature"]);
    commit(&repo, "feature.txt", "the work\n", "the work");
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--ff-only", "feature"]);
    // A branch that never left main has nothing on it main lacks, which is
    // what git's own `branch --merged` means by merged — and removing its
    // checkout loses nothing, because the branch stays.
    git(&repo, &["branch", "fresh", "main"]);

    let landed = merged(&repo, MAIN, &names(&["feature", "fresh"]));

    assert_eq!(landed.get("feature"), Some(&Proof::Ancestor));
    assert_eq!(landed.get("fresh"), Some(&Proof::Ancestor));
}

#[test]
fn a_squash_merged_branch_is_proved_by_its_patch() {
    if without_git("a_squash_merged_branch_is_proved_by_its_patch") {
        return;
    }
    let scratch = ScratchDir::new("squash");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "feature"]);
    commit(
        &repo,
        "tracked.txt",
        "one\ntwo\nthree\nfour\n",
        "first half",
    );
    commit(&repo, "feature.txt", "the work\n", "second half");
    // Main moves on beside it, so the squash is a merge and not a replay of
    // the branch's own commits, and after it, so the squash is not the tip.
    git(&repo, &["switch", "main"]);
    commit(&repo, "elsewhere.txt", "somebody else\n", "meanwhile");
    let squash = squash_onto_main(&repo, "feature");
    commit(&repo, "later.txt", "and then\n", "afterwards");
    // A branch nobody merged, which the same pass must not prove.
    git(&repo, &["switch", "-c", "unmerged", "main"]);
    commit(&repo, "unmerged.txt", "not yet\n", "not yet");

    let landed = merged(&repo, MAIN, &names(&["feature", "unmerged", "no-such"]));

    // Two commits on the branch and one on main, which ancestry can never
    // see: the branch's tip is not on main at all.
    assert_eq!(
        landed.get("feature"),
        Some(&Proof::Patch { commit: squash })
    );
    assert_eq!(landed.get("unmerged"), None);
    assert_eq!(landed.get("no-such"), None);
    assert_eq!(landed.len(), 1);
}

#[test]
fn the_lower_branch_of_a_stack_squashed_as_one_is_not_proved() {
    // Two stacked branches and one squash of the upper — the combined diff —
    // onto main. The upper's whole change is that commit's; the lower's is
    // half of it, which is no commit's, so it stays unproved even though its
    // work did land. A miss costs a checkout that is not offered for tidying;
    // a false "merged" would cost somebody's branch once deleting one is on
    // the table.
    if without_git("the_lower_branch_of_a_stack_squashed_as_one_is_not_proved") {
        return;
    }
    let scratch = ScratchDir::new("stack-once");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "lower"]);
    commit(&repo, "lower.txt", "the floor\n", "lower");
    git(&repo, &["switch", "-c", "upper"]);
    commit(&repo, "upper.txt", "the roof\n", "upper");
    let squash = squash_onto_main(&repo, "upper");

    let landed = merged(&repo, MAIN, &names(&["lower", "upper"]));

    assert_eq!(landed.get("lower"), None);
    assert_eq!(landed.get("upper"), Some(&Proof::Patch { commit: squash }));
}

#[test]
fn the_upper_branch_of_a_stack_squashed_in_two_is_not_proved() {
    // The forge's usual order for a stack: the lower lands, the upper is
    // retargeted and lands after it as only its own half. The upper's diff
    // since it left main is both halves, and main has them as two commits,
    // never one.
    if without_git("the_upper_branch_of_a_stack_squashed_in_two_is_not_proved") {
        return;
    }
    let scratch = ScratchDir::new("stack-twice");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "lower"]);
    commit(&repo, "lower.txt", "the floor\n", "lower");
    git(&repo, &["switch", "-c", "upper"]);
    commit(&repo, "upper.txt", "the roof\n", "upper");
    let first = squash_onto_main(&repo, "lower");
    squash_onto_main(&repo, "upper");

    let landed = merged(&repo, MAIN, &names(&["lower", "upper"]));

    assert_eq!(landed.get("lower"), Some(&Proof::Patch { commit: first }));
    assert_eq!(landed.get("upper"), None);
}

#[test]
fn a_branch_with_a_commit_after_its_squash_is_not_proved() {
    // Squashed, and then somebody went on working on it. The new commit is on
    // no base at all, so the branch has work that has not landed — and its
    // whole diff is the squash plus that, which matches nothing.
    if without_git("a_branch_with_a_commit_after_its_squash_is_not_proved") {
        return;
    }
    let scratch = ScratchDir::new("after-squash");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "feature"]);
    commit(&repo, "feature.txt", "the work\n", "the work");
    squash_onto_main(&repo, "feature");
    assert_eq!(
        merged(&repo, MAIN, &names(&["feature"])).len(),
        1,
        "the squash itself was not proved, so the rest proves nothing"
    );

    git(&repo, &["switch", "feature"]);
    commit(&repo, "feature.txt", "the work\nand more of it\n", "more");

    assert_eq!(
        merged(&repo, MAIN, &names(&["feature"])).get("feature"),
        None
    );
}

// --- what bounds the pass ------------------------------------------------------

/// A repository whose `feature` was squashed onto main, with `after` more
/// commits on main since, and a `done` branch main fast-forwarded over.
fn squashed_then_buried(scratch: &ScratchDir, after: usize) -> (PathBuf, String) {
    let repo = repository(scratch, "repo");
    git(&repo, &["switch", "-c", "done"]);
    commit(&repo, "done.txt", "finished\n", "done");
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--ff-only", "done"]);
    git(&repo, &["switch", "-c", "feature"]);
    commit(&repo, "feature.txt", "the work\n", "the work");
    let squash = squash_onto_main(&repo, "feature");
    for index in 0..after {
        commit(
            &repo,
            "history.txt",
            &format!("line {index}\n"),
            &format!("history {index}"),
        );
    }
    (repo, squash)
}

#[test]
fn a_squash_further_back_than_the_commit_cap_is_not_proved() {
    if without_git("a_squash_further_back_than_the_commit_cap_is_not_proved") {
        return;
    }
    let scratch = ScratchDir::new("cap");
    let (repo, squash) = squashed_then_buried(&scratch, 5);
    let branches = names(&["feature", "done"]);

    // Five commits on top of the squash, and a pass allowed to read three:
    // the squash is never seen, and not seeing it is "not proved" — never a
    // guess.
    let capped = merged_within(&repo, MAIN, &branches, 3, PASS_DEADLINE);
    assert_eq!(capped.get("feature"), None);
    // Ancestry costs no pass through history, so the cap does not touch it.
    assert_eq!(capped.get("done"), Some(&Proof::Ancestor));

    // And the same repository with room for it proves it, so what stopped
    // the first was the cap and not the case.
    let roomy = merged_within(&repo, MAIN, &branches, 6, PASS_DEADLINE);
    assert_eq!(roomy.get("feature"), Some(&Proof::Patch { commit: squash }));
}

#[test]
fn a_pass_that_outlives_its_deadline_proves_nothing_by_patch() {
    if without_git("a_pass_that_outlives_its_deadline_proves_nothing_by_patch") {
        return;
    }
    let scratch = ScratchDir::new("deadline");
    let (repo, _) = squashed_then_buried(&scratch, 2);
    let branches = names(&["feature", "done"]);
    assert!(
        merged(&repo, MAIN, &branches).contains_key("feature"),
        "the squash is not provable at all, so the deadline proves nothing"
    );

    // A deadline already over when the pass starts: git is killed before it
    // has read a commit, and what a killed pass printed is not an answer —
    // half a history read is not the history.
    let started = Instant::now();
    let rushed = merged_within(&repo, MAIN, &branches, PASS_COMMITS, Duration::ZERO);

    assert_eq!(rushed.get("feature"), None);
    assert_eq!(rushed.get("done"), Some(&Proof::Ancestor));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the pass took {:?} against no time at all",
        started.elapsed()
    );
}

// --- which branch is the base ---------------------------------------------------

#[test]
fn the_base_is_origin_head_then_the_default_branch_then_main_then_master() {
    if without_git("the_base_is_origin_head_then_the_default_branch_then_main_then_master") {
        return;
    }
    let scratch = ScratchDir::new("base");
    let repo = repository(&scratch, "repo");
    git(&repo, &["branch", "master"]);
    git(&repo, &["branch", "trunk"]);
    // Set in the repository, because the code under test reads whatever the
    // machine's own git configuration says and this one names a branch no
    // repository here has.
    git(&repo, &["config", "init.defaultBranch", "nowhere"]);

    assert_eq!(base_of(&repo).as_deref(), Some("refs/heads/main"));

    git(&repo, &["switch", "trunk"]);
    git(&repo, &["branch", "-D", "main"]);
    assert_eq!(base_of(&repo).as_deref(), Some("refs/heads/master"));

    // The configured default comes before the two conventional names — but
    // only a default that exists: `nowhere` above was passed over.
    git(&repo, &["config", "init.defaultBranch", "trunk"]);
    assert_eq!(base_of(&repo).as_deref(), Some("refs/heads/trunk"));

    // What the remote says its default is comes before all of them, as the
    // remote-tracking branch: squashes land on the forge, and a local branch
    // nobody has pulled into has not seen them.
    git(
        &repo,
        &["update-ref", "refs/remotes/origin/develop", "HEAD"],
    );
    git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/develop",
        ],
    );
    assert_eq!(
        base_of(&repo).as_deref(),
        Some("refs/remotes/origin/develop")
    );

    // An `origin/HEAD` left pointing at a branch the remote has since deleted
    // is no base at all, and the next rule answers.
    git(&repo, &["update-ref", "-d", "refs/remotes/origin/develop"]);
    assert_eq!(base_of(&repo).as_deref(), Some("refs/heads/trunk"));

    // And a repository with none of them has no base to prove anything
    // against.
    git(&repo, &["config", "init.defaultBranch", "nowhere"]);
    git(&repo, &["branch", "-D", "master"]);
    assert_eq!(base_of(&repo), None);
}
