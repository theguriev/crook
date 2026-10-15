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
use std::sync::Mutex;
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
    proofs(
        super::merged_within(repository, base, branches, commits, deadline, &|| true)
            .expect("nothing stops a proof that is always going"),
    )
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
fn a_branch_whose_commit_after_its_squash_only_changes_whitespace_is_not_proved() {
    // The case plain `patch-id --stable` gets wrong: it hashes each line with
    // its whitespace taken out, so a follow-up that only re-indents a line
    // hashes the same as the squash before it. In YAML, Python or a Makefile
    // the indentation is the change — here `b` moves under `a` — and a proof
    // is what a branch is deleted on, so the follow-up has to count as work.
    if without_git("a_branch_whose_commit_after_its_squash_only_changes_whitespace_is_not_proved") {
        return;
    }
    let scratch = ScratchDir::new("whitespace");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "feature"]);
    commit(&repo, "config.yml", "a:\nb: 1\n", "the config");
    let squash = squash_onto_main(&repo, "feature");
    assert_eq!(
        merged(&repo, MAIN, &names(&["feature"])).get("feature"),
        Some(&Proof::Patch { commit: squash }),
        "the squash itself was not proved, so the rest proves nothing"
    );

    git(&repo, &["switch", "feature"]);
    commit(&repo, "config.yml", "a:\n  b: 1\n", "b belongs under a");
    assert_ne!(
        git(&repo, &["diff", "main", "feature"]),
        "",
        "the follow-up changed nothing, and this is a test of nothing"
    );

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

// --- work a branch was moved off ----------------------------------------------

/// Every commit `branch`'s reflog names that no branch holds any more.
fn moved_off(repo: &Path, branch: &str) -> Vec<String> {
    git(
        repo,
        &["log", "-g", "--format=%H", &format!("refs/heads/{branch}")],
    )
    .lines()
    .filter(|commit| git(repo, &["branch", "--contains", commit]).is_empty())
    .map(str::to_owned)
    .collect()
}

#[test]
fn a_branch_reset_back_onto_the_base_is_not_called_merged() {
    // An agent commits, goes off the rails and is told to start over: `git
    // reset --hard origin/main`. The tip is on main and the reflog records a
    // commit, which is all either proof asks — and that commit is on no
    // branch at all. The branch's reflog is the last thing that remembers it,
    // and a branch deleted on a proof takes its reflog with it.
    if without_git("a_branch_reset_back_onto_the_base_is_not_called_merged") {
        return;
    }
    let scratch = ScratchDir::new("reset-onto-base");
    let repo = repository(&scratch, "repo");
    let reset = ["started-over", "caught-up", "rebased-away"];
    let mut abandoned = HashMap::new();
    for branch in reset {
        git(&repo, &["switch", "-c", branch, "main"]);
        let work = commit(
            &repo,
            &format!("{branch}.txt"),
            "the abandoned work\n",
            "the abandoned work",
        );
        abandoned.insert(branch, work);
    }
    git(&repo, &["switch", "main"]);
    commit(&repo, "later.txt", "main moves on\n", "later");
    // One back where it started, one onto where main is now, and one onto
    // main by a rebase that kept none of its commits — whose reflog entry is
    // the one a rebase that kept them all would write.
    git(&repo, &["switch", "started-over"]);
    git(&repo, &["reset", "--hard", "HEAD~1"]);
    git(&repo, &["switch", "caught-up"]);
    git(&repo, &["reset", "--hard", "main"]);
    git(&repo, &["switch", "rebased-away"]);
    git(&repo, &["rebase", "--onto", "main", "HEAD"]);
    git(&repo, &["switch", "main"]);

    let landed = merged(&repo, MAIN, &names(&reset));

    assert!(landed.is_empty(), "proved {landed:?}");
    let tip = git(&repo, &["rev-parse", "started-over"]);
    assert_eq!(
        dropped(&repo, Some(MAIN), "started-over", &tip),
        Some(vec![abandoned["started-over"].clone()]),
        "the commit only the branch's history holds is not the one it was moved off"
    );

    // Put on a branch of its own before the reset, the work is somewhere a
    // deletion of this one does not reach, and the proof is back.
    git(&repo, &["branch", "kept", &abandoned["started-over"]]);
    assert_eq!(
        merged(&repo, MAIN, &names(&["started-over"])).get("started-over"),
        Some(&Proof::Ancestor)
    );
    assert_eq!(
        dropped(&repo, Some(MAIN), "started-over", &tip),
        Some(Vec::new())
    );
}

#[test]
fn a_squashed_branch_reset_away_from_work_after_its_squash_is_not_proved() {
    // Squashed, worked on again, and that work thrown off: the tip is the one
    // the squash proves, and the commit after it is in nothing but the
    // branch's reflog.
    if without_git("a_squashed_branch_reset_away_from_work_after_its_squash_is_not_proved") {
        return;
    }
    let scratch = ScratchDir::new("reset-after-squash");
    let repo = repository(&scratch, "repo");
    git(&repo, &["switch", "-c", "feature"]);
    commit(&repo, "feature.txt", "the work\n", "the work");
    let squash = squash_onto_main(&repo, "feature");
    assert_eq!(
        merged(&repo, MAIN, &names(&["feature"])).get("feature"),
        Some(&Proof::Patch { commit: squash }),
        "the squash itself was not proved, so the rest proves nothing"
    );

    git(&repo, &["switch", "feature"]);
    commit(&repo, "feature.txt", "the work\nand more of it\n", "more");
    git(&repo, &["reset", "--hard", "HEAD~1"]);
    git(&repo, &["switch", "main"]);

    assert_eq!(
        merged(&repo, MAIN, &names(&["feature"])).get("feature"),
        None
    );
}

#[test]
fn a_branch_rewritten_on_its_way_to_the_base_keeps_its_proof() {
    // Every one of these moved off commits of its own before it landed, and
    // git has pruned none of their reflogs — but each of those commits has
    // its change in what the branch holds or the base has: an amend, a
    // rebase before a squash or a fast-forward, a squash made by hand with
    // `reset --soft`, and a branch squashed on main and then reset onto it to
    // start the next thing. A proof that asked only whether the commits
    // themselves were somewhere would take all five away.
    //
    // The last four each make a new file and then edit one main has, in that
    // order. The edit's change has lines around it, where a new file's has
    // none, so it is the one that shows how each side of a comparison was
    // hashed; and it is not its branch's first commit, so its whole change
    // since the fork is more than its own, and a copy of it and a squash of
    // it are two different finds that cannot stand in for each other.
    if without_git("a_branch_rewritten_on_its_way_to_the_base_keeps_its_proof") {
        return;
    }
    let scratch = ScratchDir::new("rewritten");
    let repo = repository(&scratch, "repo");
    let rewritten = ["amended", "rebased", "forwarded", "by-hand", "synced"];
    let halved = ["rebased", "forwarded", "by-hand", "synced"];
    for branch in halved {
        commit(
            &repo,
            &format!("{branch}.txt"),
            "a\nb\nc\nd\ne\n",
            &format!("{branch}'s file"),
        );
    }
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
    for branch in halved {
        git(&repo, &["switch", branch]);
        commit(&repo, &format!("{branch}-1.txt"), "one half\n", "one half");
        commit(
            &repo,
            &format!("{branch}.txt"),
            "a\nb\nC\nd\ne\n",
            "the other",
        );
    }
    git(&repo, &["switch", "by-hand"]);
    git(&repo, &["reset", "--soft", "main"]);
    git(&repo, &["commit", "--no-verify", "-m", "both halves"]);

    git(&repo, &["switch", "main"]);
    commit(&repo, "later.txt", "main moves on\n", "later");
    for branch in ["rebased", "forwarded"] {
        git(&repo, &["switch", branch]);
        git(&repo, &["rebase", "main"]);
    }
    // And one more after the rebase, so that what was squashed is more than
    // the commits it moved off, and only their copies on the branch hold
    // their change.
    git(&repo, &["switch", "rebased"]);
    commit(&repo, "rebased-3.txt", "and more\n", "and more");
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--ff-only", "forwarded"]);
    let amended = squash_onto_main(&repo, "amended");
    let rebased = squash_onto_main(&repo, "rebased");
    let by_hand = squash_onto_main(&repo, "by-hand");
    squash_onto_main(&repo, "synced");
    git(&repo, &["switch", "synced"]);
    git(&repo, &["reset", "--hard", "main"]);
    git(&repo, &["switch", "main"]);
    for branch in rewritten {
        // Or nothing was moved off, and this is the case above over again.
        assert!(
            !moved_off(&repo, branch).is_empty(),
            "{branch} holds every commit its reflog names"
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
    assert_eq!(
        landed.get("by-hand"),
        Some(&Proof::Patch { commit: by_hand })
    );
    assert_eq!(landed.get("synced"), Some(&Proof::Ancestor));
}

/// What `commit` changed as `patch-id` reads it, with `context` lines around
/// each change: without the `index` lines, which name blobs, or the hunk
/// headers, whose line numbers it takes out.
fn change_of(repo: &Path, commit: &str, context: usize) -> Vec<String> {
    let context = format!("--unified={context}");
    git(repo, &["show", "--format=", "--no-color", &context, commit])
        .lines()
        .filter(|line| !line.starts_with("index ") && !line.starts_with("@@"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_branch_rebased_over_a_change_beside_its_own_keeps_its_proof() {
    // A rebase copies each commit onto the new base, and the copy's diff
    // carries the lines around its change as that base has them. When the
    // base edited a line within a diff's three of the branch's, or touched
    // the same spot and the conflict was settled by keeping both sides —
    // which the `union` driver does here, as a person would by hand — the
    // copy hashes differently from its original although its lines are the
    // same. The originals stay in the reflog, on no branch, and their lines
    // are in what landed: a fast-forward, and a squash of each of the others.
    // Where a branch has two commits the edit is the second, so that its
    // whole change since the fork is more than its own, and only its copy can
    // account for it.
    if without_git("a_branch_rebased_over_a_change_beside_its_own_keeps_its_proof") {
        return;
    }
    let scratch = ScratchDir::new("rebased-beside");
    let repo = repository(&scratch, "repo");
    let rebased = ["forwarded", "squashed", "conflicted"];
    for branch in rebased {
        commit(
            &repo,
            &format!("{branch}.txt"),
            "a\nb\nc\nd\ne\nf\ng\nh\n",
            &format!("{branch}'s file"),
        );
    }
    let info = repo.join(".git").join("info");
    std::fs::create_dir_all(&info).expect("the scratch directory is writable");
    std::fs::write(info.join("attributes"), "conflicted.txt merge=union\n")
        .expect("the scratch directory is writable");

    let mut originals = HashMap::new();
    for branch in ["forwarded", "squashed"] {
        git(&repo, &["switch", "-c", branch, "main"]);
        commit(&repo, &format!("{branch}-1.txt"), "one\n", "one");
        let edit = commit(
            &repo,
            &format!("{branch}.txt"),
            "a\nb\nc\nd\nE\nf\ng\nh\n",
            "two",
        );
        originals.insert(branch, edit);
    }
    git(&repo, &["switch", "-c", "conflicted", "main"]);
    let appended = commit(
        &repo,
        "conflicted.txt",
        "a\nb\nc\nd\ne\nf\ng\nh\nthe branch's line\n",
        "one",
    );
    originals.insert("conflicted", appended);

    git(&repo, &["switch", "main"]);
    for branch in ["forwarded", "squashed"] {
        std::fs::write(
            repo.join(format!("{branch}.txt")),
            "a\nb\nC\nd\ne\nf\ng\nh\n",
        )
        .expect("the scratch directory is writable");
    }
    std::fs::write(
        repo.join("conflicted.txt"),
        "a\nb\nc\nd\ne\nf\ng\nh\nmain's line\n",
    )
    .expect("the scratch directory is writable");
    git(&repo, &["commit", "--no-verify", "--all", "-m", "beside"]);

    for branch in rebased {
        git(&repo, &["switch", branch]);
        git(&repo, &["rebase", "main"]);
        let copy = git(&repo, &["rev-parse", "HEAD"]);
        let original = &originals[branch];
        // Or the copy hashes as its original does, and this is the case
        // above over again.
        assert_ne!(
            change_of(&repo, original, 3),
            change_of(&repo, &copy, 3),
            "{branch}'s copy stands among the lines its original did"
        );
        // Or the copy is a different change, which nothing should take for
        // the original.
        assert_eq!(
            change_of(&repo, original, 0),
            change_of(&repo, &copy, 0),
            "{branch}'s copy does not make its original's change"
        );
    }
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--ff-only", "forwarded"]);
    let squashed = squash_onto_main(&repo, "squashed");
    let conflicted = squash_onto_main(&repo, "conflicted");
    for branch in rebased {
        assert!(
            moved_off(&repo, branch).contains(&originals[branch]),
            "{branch} still holds the commit it was rebased off"
        );
    }

    let landed = merged(&repo, MAIN, &names(&rebased));

    assert_eq!(landed.get("forwarded"), Some(&Proof::Ancestor));
    assert_eq!(
        landed.get("squashed"),
        Some(&Proof::Patch { commit: squashed })
    );
    assert_eq!(
        landed.get("conflicted"),
        Some(&Proof::Patch { commit: conflicted })
    );
}

/// When `commit`'s author wrote it, as git keeps it: seconds and a zone.
fn written(repo: &Path, commit: &str) -> String {
    git(
        repo,
        &["log", "-1", "--date=raw", "--format=%ad", commit, "--"],
    )
}

#[test]
fn a_branch_rebased_with_its_dates_reset_keeps_its_proof() {
    // `rebase --reset-author-date` gives each copy the moment it was copied
    // rather than the one its original was written in, so the two no longer
    // share what tells a copy from the same edit made elsewhere. A copy whose
    // lines stand among the same lines around them as its original's is
    // still that commit's copy. The edit is the branch's second commit, so
    // that its whole change since the fork is more than its own.
    if without_git("a_branch_rebased_with_its_dates_reset_keeps_its_proof") {
        return;
    }
    let scratch = ScratchDir::new("dates-reset");
    let repo = repository(&scratch, "repo");
    commit(&repo, "feature.txt", "a\nb\nc\nd\ne\n", "feature's file");
    git(&repo, &["switch", "-c", "feature"]);
    // Long before the rebase, so that the moment it resets them to is
    // another one.
    let long_ago = "--date=2001-02-03T04:05:06+0000";
    std::fs::write(repo.join("feature-1.txt"), "one\n").expect("the scratch directory is writable");
    git(&repo, &["add", "feature-1.txt"]);
    git(&repo, &["commit", "--no-verify", long_ago, "-m", "one"]);
    std::fs::write(repo.join("feature.txt"), "a\nb\nC\nd\ne\n")
        .expect("the scratch directory is writable");
    git(
        &repo,
        &["commit", "--no-verify", "--all", long_ago, "-m", "two"],
    );
    let edit = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["switch", "main"]);
    commit(&repo, "later.txt", "main moves on\n", "later");
    git(&repo, &["switch", "feature"]);
    git(&repo, &["rebase", "--reset-author-date", "main"]);
    let copy = git(&repo, &["rev-parse", "HEAD"]);
    // Or the copy kept its original's moment, and this is a rebase like the
    // ones above.
    assert_ne!(
        written(&repo, &edit),
        written(&repo, &copy),
        "the rebase kept the date it was told to reset"
    );
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--ff-only", "feature"]);
    assert!(
        moved_off(&repo, "feature").contains(&edit),
        "feature still holds the commit it was rebased off"
    );

    assert_eq!(
        merged(&repo, MAIN, &names(&["feature"])).get("feature"),
        Some(&Proof::Ancestor)
    );
}

/// A client whose two functions each read with a timeout, the same line in
/// both: `fetch`'s on line 3, `upload`'s on line 10.
fn client(fetch: u32, upload: u32) -> String {
    format!(
        "def fetch(conn):\n    log(\"fetch\")\n    data = conn.read(timeout={fetch})\n    \
         return data\n\n\ndef upload(conn, blob):\n    log(\"upload\")\n    conn.write(blob)\n    \
         data = conn.read(timeout={upload})\n    return data\n"
    )
}

#[test]
fn a_branch_reset_off_an_edit_the_base_made_to_a_twin_line_is_not_proved() {
    // The lines a change adds and removes, hashed alone, are what lets a
    // rebased copy among new neighbours match its original — and they are
    // also what the same edit to another, identical line of the same file
    // has. Here the branch raised `upload`'s timeout, main later raised
    // `fetch`'s, and the branch was reset onto main to start over: `upload`
    // still reads with the old one everywhere, and the branch's reflog is the
    // last thing that remembers the change. Main's commit was written apart
    // from the branch's, at another moment, as one nobody copied is.
    if without_git("a_branch_reset_off_an_edit_the_base_made_to_a_twin_line_is_not_proved") {
        return;
    }
    let scratch = ScratchDir::new("twin-line");
    let repo = repository(&scratch, "repo");
    commit(&repo, "client.py", &client(3, 3), "the client");
    git(&repo, &["switch", "-c", "feature"]);
    let raised = commit(&repo, "client.py", &client(3, 30), "raise upload's timeout");
    git(&repo, &["switch", "main"]);
    std::fs::write(repo.join("client.py"), client(30, 3))
        .expect("the scratch directory is writable");
    git(
        &repo,
        &[
            "commit",
            "--no-verify",
            "--all",
            "--date=2001-02-03T04:05:06+0000",
            "-m",
            "raise fetch's timeout",
        ],
    );
    let twin = git(&repo, &["rev-parse", "HEAD"]);
    // Or the two are different changes even as lines alone, or the same
    // change in the same place, and this is not the case it says it is.
    assert_eq!(
        change_of(&repo, &raised, 0),
        change_of(&repo, &twin, 0),
        "the two edits are not the same lines"
    );
    assert_ne!(
        change_of(&repo, &raised, 3),
        change_of(&repo, &twin, 3),
        "the two edits stand among the same lines"
    );
    git(&repo, &["switch", "feature"]);
    git(&repo, &["reset", "--hard", "main"]);
    git(&repo, &["switch", "main"]);

    assert_eq!(
        merged(&repo, MAIN, &names(&["feature"])).get("feature"),
        None
    );
    let tip = git(&repo, &["rev-parse", "feature"]);
    assert_eq!(
        dropped(&repo, Some(MAIN), "feature", &tip),
        Some(vec![raised])
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

#[test]
fn a_look_for_moved_off_work_whose_deadline_is_spent_accounts_for_nothing() {
    // The passes over history are only the start of the look for what a
    // branch moved off: after them come reads of the commits themselves, of
    // who wrote them, and of what a branch had changed since its fork, for
    // each commit of each branch. Every one is handed what is left of the
    // deadline the passes had, so that the look costs that one deadline
    // however many reads it makes — and once it is spent, the rest account
    // for nothing, which costs a proof and never makes one.
    if without_git("a_look_for_moved_off_work_whose_deadline_is_spent_accounts_for_nothing") {
        return;
    }
    let scratch = ScratchDir::new("look-deadline");
    let repo = repository(&scratch, "repo");
    // One whose commits only their copies account for, rebased and
    // fast-forwarded, and one only its squash does, reset onto it.
    let moved = ["forwarded", "synced"];
    for branch in moved {
        git(&repo, &["switch", "-c", branch, "main"]);
        commit(&repo, &format!("{branch}-1.txt"), "one half\n", "one half");
        commit(
            &repo,
            &format!("{branch}-2.txt"),
            "the other\n",
            "the other",
        );
    }
    git(&repo, &["switch", "main"]);
    commit(&repo, "later.txt", "main moves on\n", "later");
    git(&repo, &["switch", "forwarded"]);
    git(&repo, &["rebase", "main"]);
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--ff-only", "forwarded"]);
    squash_onto_main(&repo, "synced");
    git(&repo, &["switch", "synced"]);
    git(&repo, &["reset", "--hard", "main"]);
    git(&repo, &["switch", "main"]);
    let on_base = held_on(&repo, &[MAIN], None, PASS_COMMITS, PASS_DEADLINE);
    let held = Held {
        on_base: &on_base,
        on_tip: HashMap::new(),
    };

    for branch in moved {
        let reflog =
            reflog_of(&repo, &format!("refs/heads/{branch}")).expect("git reads the reflog");
        let originals = moved_off(&repo, branch);
        assert_eq!(originals.len(), 2, "{branch} was not moved off its commits");
        let look = |until| unaccounted(&repo, Some(MAIN), &reflog, &originals, &held, false, until);

        assert_eq!(
            look(Instant::now() + PASS_DEADLINE),
            Vec::<String>::new(),
            "{branch}'s commits are not accounted for even in time"
        );
        assert_eq!(look(Instant::now()), originals);
    }
}

// --- proving only what moved ----------------------------------------------------
//
// The worktree menu asks about every branch each time it opens, and opens again
// after every removal. These count the gits a second look starts, because an
// answer kept and an answer proved again are the same answer.

/// Every git this thread has started since it last asked, each as its
/// arguments.
fn reads_since() -> Vec<String> {
    super::super::run::started_here()
}

/// A repository with a branch squashed onto `main`, one fast-forwarded into
/// it, and one with work `main` does not have.
fn three_branches(scratch: &ScratchDir) -> PathBuf {
    let repo = repository(scratch, "repo");
    git(&repo, &["switch", "-c", "squashed"]);
    commit(
        &repo,
        "squashed.txt",
        "the squashed work\n",
        "squashed work",
    );
    squash_onto_main(&repo, "squashed");
    git(&repo, &["switch", "-c", "merged"]);
    commit(&repo, "merged.txt", "the merged work\n", "merged work");
    git(&repo, &["switch", "main"]);
    git(&repo, &["merge", "--ff-only", "merged"]);
    git(&repo, &["switch", "-c", "open"]);
    commit(&repo, "open.txt", "the open work\n", "open work");
    git(&repo, &["switch", "main"]);
    repo
}

const THREE: [&str; 3] = ["merged", "open", "squashed"];

/// [`merged_remembering`] of [`THREE`], going all the way.
fn remembered(repo: &Path, proofs: &Mutex<Proofs>) -> HashMap<String, Landed> {
    merged_remembering(repo, MAIN, &names(&THREE), proofs, &|| true)
        .expect("nothing stops a proof that is always going")
}

/// Which of [`THREE`] the reads since the last ask named.
fn asked_about(reads: &[String]) -> Vec<&'static str> {
    THREE
        .into_iter()
        .filter(|branch| {
            let reference = format!("refs/heads/{branch}");
            reads
                .iter()
                .any(|read| read.split(' ').any(|word| word == reference))
        })
        .collect()
}

#[test]
fn a_second_look_at_branches_that_have_not_moved_proves_nothing() {
    if without_git("a_second_look_at_branches_that_have_not_moved_proves_nothing") {
        return;
    }
    let scratch = ScratchDir::new("kept");
    let repo = three_branches(&scratch);
    let kept = Mutex::new(Proofs::default());

    let first = remembered(&repo, &kept);
    assert_eq!(
        proofs(first.clone()),
        HashMap::from([
            ("merged".to_owned(), Proof::Ancestor),
            (
                "squashed".to_owned(),
                Proof::Patch {
                    commit: git(&repo, &["rev-parse", "main~1"])
                }
            ),
        ])
    );
    assert_eq!(asked_about(&reads_since()), THREE);

    let second = remembered(&repo, &kept);

    assert_eq!(second, first);
    let reads = reads_since();
    assert_eq!(reads.len(), 1, "{reads:?}");
    assert!(reads[0].starts_with("for-each-ref"), "{reads:?}");
}

#[test]
fn a_branch_that_moved_is_the_only_one_proved_again() {
    if without_git("a_branch_that_moved_is_the_only_one_proved_again") {
        return;
    }
    let scratch = ScratchDir::new("one-moved");
    let repo = three_branches(&scratch);
    let kept = Mutex::new(Proofs::default());
    let first = remembered(&repo, &kept);
    reads_since();

    git(&repo, &["switch", "open"]);
    commit(
        &repo,
        "open.txt",
        "the open work, and more\n",
        "more open work",
    );
    git(&repo, &["switch", "main"]);
    let second = remembered(&repo, &kept);

    assert_eq!(asked_about(&reads_since()), ["open"]);
    assert_eq!(second, first);
    assert_eq!(second, super::merged(&repo, MAIN, &names(&THREE)));
}

#[test]
fn a_base_that_moved_proves_every_branch_again() {
    // A squash that lands is the base moving, and it is the moment the
    // branches that were not proved before can be.
    if without_git("a_base_that_moved_proves_every_branch_again") {
        return;
    }
    let scratch = ScratchDir::new("base-moved-again");
    let repo = three_branches(&scratch);
    let kept = Mutex::new(Proofs::default());
    remembered(&repo, &kept);
    reads_since();

    let open = squash_onto_main(&repo, "open");
    let second = remembered(&repo, &kept);

    assert_eq!(asked_about(&reads_since()), THREE);
    assert_eq!(
        proofs(second).get("open"),
        Some(&Proof::Patch { commit: open })
    );
}

#[test]
fn a_branch_reset_to_the_very_commit_it_was_proved_at_is_proved_again() {
    // Its tip is where it was, so a proof kept by tip alone would still say
    // merged — about a branch whose reflog is now the last thing holding a
    // commit somebody made on it and reset away. That is the branch a proof
    // refuses, and so is the badge.
    if without_git("a_branch_reset_to_the_very_commit_it_was_proved_at_is_proved_again") {
        return;
    }
    let scratch = ScratchDir::new("reset-in-place");
    let repo = three_branches(&scratch);
    let kept = Mutex::new(Proofs::default());
    assert!(remembered(&repo, &kept).contains_key("merged"));

    git(&repo, &["switch", "merged"]);
    commit(&repo, "abandoned.txt", "the abandoned work\n", "abandoned");
    git(&repo, &["reset", "--hard", "HEAD~1"]);
    git(&repo, &["switch", "main"]);
    let second = remembered(&repo, &kept);

    assert!(!second.contains_key("merged"), "proved {second:?}");
}

#[test]
fn a_pass_nobody_wants_any_more_stops_between_branches_and_keeps_nothing() {
    // Opening the menu again while a pass is proving sixty branches for the
    // last opening would otherwise run that pass on, for seconds, for an
    // answer the menu drops on arrival.
    if without_git("a_pass_nobody_wants_any_more_stops_between_branches_and_keeps_nothing") {
        return;
    }
    let scratch = ScratchDir::new("superseded");
    let repo = three_branches(&scratch);
    let kept = Mutex::new(Proofs::default());
    let asked = std::cell::Cell::new(0);
    let wanted_once = || {
        asked.set(asked.get() + 1);
        asked.get() == 1
    };

    let stopped = merged_remembering(&repo, MAIN, &names(&THREE), &kept, &wanted_once);

    assert_eq!(stopped, None);
    assert_eq!(asked_about(&reads_since()), ["merged"]);
    remembered(&repo, &kept);
    assert_eq!(
        asked_about(&reads_since()),
        THREE,
        "a stopped pass kept an answer it never finished"
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
