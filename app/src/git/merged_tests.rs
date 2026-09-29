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

use std::collections::HashMap;
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

/// [`super::merged`], answered as the proofs alone.
///
/// Named after it and standing in front of it on purpose: almost every case
/// here is about *whether* and *how* a branch was proved, and a [`Landed`]
/// carries more than that — the tip, the base, the evidence — which has its
/// own cases below and would be noise in every assertion above them.
fn merged(repository: &Path, base: &str, branches: &[String]) -> HashMap<String, Proof> {
    proofs(super::merged(repository, base, branches))
}

/// [`super::merged_within`], answered the same way.
fn merged_within(
    repository: &Path,
    base: &str,
    branches: &[String],
    commits: usize,
    deadline: Duration,
) -> HashMap<String, Proof> {
    proofs(super::merged_within(
        repository, base, branches, commits, deadline,
    ))
}

fn proofs(landed: HashMap<String, Landed>) -> HashMap<String, Proof> {
    landed
        .into_iter()
        .map(|(branch, landed)| (branch, landed.proof().clone()))
        .collect()
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

    let landed = merged(&repo, MAIN, &names(&["feature"]));

    assert_eq!(landed.get("feature"), Some(&Proof::Ancestor));
}

#[test]
fn a_branch_nobody_has_committed_on_is_not_called_merged() {
    // Its tip is on main, so git's own `branch --merged` lists it — and on the
    // row of a checkout an agent was handed a minute ago, "merged" is a
    // sentence about work that does not exist. Its own reflog records no
    // commit made on it, and that is what keeps it out.
    if without_git("a_branch_nobody_has_committed_on_is_not_called_merged") {
        return;
    }
    let scratch = ScratchDir::new("fresh");
    let repo = repository(&scratch, "repo");
    git(&repo, &["branch", "fresh", "main"]);
    git(&repo, &["switch", "-c", "switched"]);
    git(&repo, &["switch", "main"]);
    commit(&repo, "later.txt", "main moves on\n", "later");

    let landed = merged(&repo, MAIN, &names(&["fresh", "switched"]));

    assert_eq!(landed.get("fresh"), None);
    assert_eq!(landed.get("switched"), None);

    // Committed on and merged into main, it has work of its own, and its tip
    // being on main is that work having landed.
    git(&repo, &["switch", "fresh"]);
    commit(&repo, "fresh.txt", "work\n", "work");
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--no-edit", "fresh"]);
    assert_eq!(
        merged(&repo, MAIN, &names(&["fresh"])).get("fresh"),
        Some(&Proof::Ancestor)
    );
}

#[test]
fn a_branch_only_brought_up_to_date_is_not_called_merged() {
    // Handed to an agent whose first move was to catch up with main — a
    // rebase, a pull, a fast-forward — before its first commit. The branch
    // has moved since it was made, and its tip is on main, and it still holds
    // no work of its own.
    if without_git("a_branch_only_brought_up_to_date_is_not_called_merged") {
        return;
    }
    let scratch = ScratchDir::new("synced");
    let repo = repository(&scratch, "repo");
    let synced = ["rebased", "pulled", "forwarded"];
    for branch in synced {
        git(&repo, &["branch", branch, "main"]);
    }
    commit(&repo, "later.txt", "main moves on\n", "later");
    git(&repo, &["switch", "rebased"]);
    git(&repo, &["rebase", "main"]);
    git(&repo, &["switch", "pulled"]);
    git(&repo, &["pull", "--ff-only", ".", "main"]);
    git(&repo, &["switch", "forwarded"]);
    git(&repo, &["merge", "--ff-only", "main"]);
    git(&repo, &["switch", "main"]);
    for branch in synced {
        // Or this would be the case above over again.
        let newest = git(
            &repo,
            &[
                "log",
                "-g",
                "-1",
                "--format=%gs",
                &format!("refs/heads/{branch}"),
            ],
        );
        assert!(
            !newest.starts_with("branch: Created"),
            "{branch} did not move: {newest}"
        );
    }

    let landed = merged(&repo, MAIN, &names(&synced));

    assert!(landed.is_empty(), "proved {landed:?}");
}

#[test]
fn a_branch_cut_from_a_squashed_branch_is_not_called_merged() {
    // A new checkout made from a tab still sitting in a finished one starts
    // at that one's tip, so its diff since main is the finished branch's
    // whole diff — the squash's patch, though nothing was done on it.
    if without_git("a_branch_cut_from_a_squashed_branch_is_not_called_merged") {
        return;
    }
    let scratch = ScratchDir::new("cut-from-squashed");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "feature"]);
    commit(&repo, "feature.txt", "the work\n", "the work");
    let squash = squash_onto_main(&repo, "feature");
    git(&repo, &["branch", "fresh", "feature"]);

    let landed = merged(&repo, MAIN, &names(&["feature", "fresh"]));

    assert_eq!(
        landed.get("feature"),
        Some(&Proof::Patch { commit: squash })
    );
    assert_eq!(landed.get("fresh"), None);
}

/// What `git gc` does to every reflog once its entries are a month old:
/// `gc.reflogExpireUnreachable`'s thirty days, brought forward to now.
fn prune_reflogs(repo: &Path) {
    git(
        repo,
        &["reflog", "expire", "--expire-unreachable=now", "--all"],
    );
}

/// Every entry `branch`'s reflog still holds, newest first.
fn reflog(repo: &Path, branch: &str) -> Vec<String> {
    git(
        repo,
        &["log", "-g", "--format=%gs", &format!("refs/heads/{branch}")],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

#[test]
fn a_rewritten_branch_keeps_its_proof_once_git_prunes_its_reflog() {
    // An amend or a rebase replaces the commits the branch's `commit` entries
    // name, and a month later gc expires every entry that names one — all of
    // them but the creation, which is what a branch nobody committed on has.
    // These three were committed on, and landed, and still have after gc.
    if without_git("a_rewritten_branch_keeps_its_proof_once_git_prunes_its_reflog") {
        return;
    }
    let scratch = ScratchDir::new("pruned");
    let repo = repository(&scratch, "repo");
    let rewritten = ["amended", "rebased", "forwarded"];
    for branch in rewritten {
        git(&repo, &["branch", branch, "main"]);
    }
    git(&repo, &["switch", "amended"]);
    commit(&repo, "amended.txt", "a first draft\n", "the work");
    std::fs::write(repo.join("amended.txt"), "the final draft\n")
        .expect("the scratch directory is writable");
    git(
        &repo,
        &["commit", "--no-verify", "--amend", "--all", "--no-edit"],
    );
    for branch in ["rebased", "forwarded"] {
        git(&repo, &["switch", branch]);
        commit(&repo, &format!("{branch}.txt"), "the work\n", "the work");
    }
    git(&repo, &["switch", "main"]);
    commit(&repo, "later.txt", "main moves on\n", "later");
    for branch in ["rebased", "forwarded"] {
        git(&repo, &["switch", branch]);
        git(&repo, &["rebase", "main"]);
    }
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--ff-only", "forwarded"]);
    let amended = squash_onto_main(&repo, "amended");
    let rebased = squash_onto_main(&repo, "rebased");

    prune_reflogs(&repo);
    for branch in rewritten {
        // Or the pruning did not happen, and this is a test of nothing.
        let left = reflog(&repo, branch);
        assert!(
            left.len() == 1 && left[0].starts_with("branch: Created"),
            "{branch}'s reflog was not pruned to its creation: {left:?}"
        );
    }

    let landed = merged(&repo, MAIN, &names(&rewritten));

    assert_eq!(
        landed.get("amended"),
        Some(&Proof::Patch { commit: amended })
    );
    assert_eq!(
        landed.get("rebased"),
        Some(&Proof::Patch { commit: rebased })
    );
    assert_eq!(landed.get("forwarded"), Some(&Proof::Ancestor));
}

#[test]
fn a_pruned_reflog_does_not_make_a_branch_nobody_committed_on_merged() {
    // gc leaves these reflogs whole — every entry names a commit the branch
    // still holds — so the leniency a pruned reflog is given must not reach
    // them: fresh, only brought up to date, and cut from a squashed branch.
    if without_git("a_pruned_reflog_does_not_make_a_branch_nobody_committed_on_merged") {
        return;
    }
    let scratch = ScratchDir::new("pruned-empty-handed");
    let repo = repository(&scratch, "repo");
    git(&repo, &["branch", "fresh", "main"]);
    git(&repo, &["branch", "synced", "main"]);
    git(&repo, &["switch", "-c", "feature"]);
    commit(&repo, "feature.txt", "the work\n", "the work");
    squash_onto_main(&repo, "feature");
    git(&repo, &["branch", "cut", "feature"]);
    git(&repo, &["switch", "synced"]);
    git(&repo, &["rebase", "main"]);
    git(&repo, &["switch", "main"]);

    prune_reflogs(&repo);
    assert_eq!(reflog(&repo, "synced").len(), 2, "gc took a live entry");

    let landed = merged(&repo, MAIN, &names(&["fresh", "synced", "cut"]));

    assert!(landed.is_empty(), "proved {landed:?}");
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
fn a_proof_names_the_tip_it_was_made_against_and_the_squash_it_found() {
    // A proof is what a branch is deleted on the strength of, so it has to
    // say what it proved: the commit that was the tip — a deletion is refused
    // once the branch is anywhere else — and, for a squash, the commit on the
    // base a person reads before agreeing to it.
    if without_git("a_proof_names_the_tip_it_was_made_against_and_the_squash_it_found") {
        return;
    }
    let scratch = ScratchDir::new("evidence");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "done"]);
    let done_tip = commit(&repo, "done.txt", "finished\n", "done");
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--ff-only", "done"]);
    git(&repo, &["switch", "-c", "feature"]);
    let feature_tip = commit(&repo, "feature.txt", "the work\n", "the work");
    let squash = squash_onto_main(&repo, "feature");

    let landed = super::merged(&repo, MAIN, &names(&["feature", "done"]));

    let feature = &landed["feature"];
    assert_eq!(feature.branch(), "feature");
    assert_eq!(feature.base(), MAIN);
    assert_eq!(feature.tip(), feature_tip);
    assert_eq!(feature.proof(), &Proof::Patch { commit: squash });
    assert_eq!(feature.subject(), Some("feature, squashed"));

    let done = &landed["done"];
    assert_eq!(done.tip(), done_tip);
    assert_eq!(done.proof(), &Proof::Ancestor);
    assert_eq!(done.subject(), None, "a fast-forward has no squash to name");
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

#[test]
fn a_patch_the_base_had_before_the_branch_left_it_proves_nothing() {
    // Main adds a file and takes it back; a branch then adds it again. The
    // branch's diff has the patch-id of main's first commit — and that commit
    // is from before the branch left main, so it cannot be the branch's
    // squash, and main does not have the file. An older branch in the same
    // call is what sends the pass back far enough to meet it.
    if without_git("a_patch_the_base_had_before_the_branch_left_it_proves_nothing") {
        return;
    }
    let scratch = ScratchDir::new("reland");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "old"]);
    commit(&repo, "old.txt", "early\n", "early");
    git(&repo, &["switch", "main"]);
    let first = commit(&repo, "foo.txt", "foo\n", "foo");
    git(&repo, &["revert", "--no-edit", &first]);
    git(&repo, &["switch", "-c", "reland"]);
    commit(&repo, "foo.txt", "foo\n", "foo, again");
    let branches = names(&["old", "reland"]);

    let landed = merged(&repo, MAIN, &branches);

    assert!(landed.is_empty(), "proved {landed:?}");

    // Squashed after all, it is proved by that squash — though the commit
    // from before its fork has the same patch-id and sits further down.
    let squash = squash_onto_main(&repo, "reland");
    assert_eq!(
        merged(&repo, MAIN, &branches).get("reland"),
        Some(&Proof::Patch { commit: squash })
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
