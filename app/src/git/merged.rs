//! Which branches' work has already landed on the base branch, proved from
//! the repository on disk alone.
//!
//! [`super::worktree::remove`] never deletes a branch, and git cannot see a
//! squash merge, so every finished task leaves a checkout behind on a branch
//! `git branch --no-merged` calls unmerged for ever: this repository had
//! eighteen of them, and every one had landed. What the worktree menu needs is
//! the answer git will not give — *has this branch's work reached the base?* —
//! so it can say so on the row and offer to tidy the checkout away. Asking the
//! forge would answer it, and would mean a network call, a token and a CLI
//! Crook does not otherwise need. Two local proofs answer most of it:
//!
//! * **Ancestry.** The branch's tip is on the base: it was merged or
//!   fast-forwarded.
//! * **The patch.** The branch's whole change since it left the base — `git
//!   diff <merge-base> <branch>` — has the `patch-id` of one commit that
//!   reached the base after the branch left it. A squash merge is exactly
//!   that commit.
//!
//! Either proof is taken only for a branch somebody committed on. `git branch
//! --merged` also lists a branch whose tip is on the base because nothing was
//! ever made on it — a checkout an agent was handed a minute ago, or one it
//! only brought up to date — and a branch cut from the tip of a squashed one
//! carries that one's patch without a line of its own. "merged" on either row
//! is a sentence about work that does not exist, so a branch whose own reflog
//! records no commit made on it is proved by neither — unless git has pruned
//! that record, as it does a month after an amend or a rebase: see
//! [`worked_on`].
//!
//! A branch neither proves is *unproved*, never "not merged": a stack squashed
//! in pieces, a branch rebased or reworded during review, and a branch with a
//! commit added after its squash all land here, and a miss only means a
//! checkout is not offered for tidying. The one thing this module must never
//! do is prove a branch that did not land — the day branches are deleted on
//! the strength of it, a false proof is somebody's work gone — so every doubt,
//! failure and timeout resolves to unproved. That includes whitespace, which
//! `patch-id` ignores unless it is told not to: a branch whose last commit
//! re-indents a line of Python or YAML, or puts a tab back in a Makefile,
//! changes what the file means, and a squash of the commits before it is not
//! that branch's work. So both sides are hashed `--verbatim`, which a git
//! older than 2.39 does not have: there `patch-id` refuses the flag, and
//! nothing is proved by patch — only by ancestry.
//!
//! Everything is read-only and runs through [`super::run`]'s deadline. The
//! pass over the base's history is bounded twice, by [`PASS_COMMITS`] and
//! [`PASS_DEADLINE`], because it is the one read here whose length grows
//! with the repository rather than with the question.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use super::run::{Intent, READ_TIMEOUT, pipe, run};

/// How many of the base's commits the pass reads, newest first.
///
/// The pass is `git log -p`, whose output is every patch in the range, and
/// on this repository a thousand commits is about two months of work and two
/// seconds of reading. A squash older than that belongs to a checkout nobody
/// has looked at in months, which is the one a person does not need a proof to
/// recognise — and missing it costs a row with no badge, where reading every
/// commit a monorepo has ever had would cost a pool worker for minutes.
const PASS_COMMITS: usize = 1000;

/// How long the pass may take before git is killed and nothing is proved by
/// patch.
///
/// The read budget [`super::run`] gives any question, because this is one:
/// the cap keeps an honest pass to seconds, and a pass past ten is a
/// repository whose commits are enormous — vendored trees, generated files —
/// where the menu that asked will have been closed before the answer came.
const PASS_DEADLINE: Duration = READ_TIMEOUT;

/// How a branch was shown to have landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proof {
    /// Its tip is on the base, and somebody committed on it: merged or
    /// fast-forwarded.
    Ancestor,
    /// Its whole change since it left the base is the change this commit made,
    /// which reached the base after the branch left it — a squash merge, most
    /// often.
    Patch {
        /// The commit on the base, as a full object id.
        commit: String,
    },
}

/// The branch a repository's work lands on, as a full ref name.
///
/// **Blocking**: up to three subprocesses, background executor only.
///
/// In order, the first that exists:
///
/// 1. What `origin/HEAD` points at — a remote-tracking branch, deliberately,
///    because squashes land on the forge and a local `main` nobody has pulled
///    into has not seen them. An `origin/HEAD` whose target a prune deleted
///    is passed over rather than answered.
/// 2. `init.defaultBranch`, the branch git itself would have made.
/// 3. `main`, then `master`.
///
/// `None` is a repository with none of them, which has no base to prove
/// anything against.
///
/// Small on purpose, and separate from any default-branch helper the
/// creator's base picker may grow: the two answer the same question for
/// different callers and can be made one once both have landed.
pub fn base_of(repository: &Path) -> Option<String> {
    // `for-each-ref` rather than `symbolic-ref`, for what it leaves out: it
    // skips a symbolic ref whose target is gone, where `symbolic-ref` would
    // read the dangling name back as though it were a branch.
    if let Some(target) = answer(
        repository,
        &[
            "for-each-ref",
            "--format=%(symref)",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        return Some(target);
    }

    let configured = answer(repository, &["config", "--get", "init.defaultBranch"])
        .map(|name| format!("refs/heads/{name}"));
    let candidates: Vec<String> = configured
        .into_iter()
        .chain(["refs/heads/main".to_owned(), "refs/heads/master".to_owned()])
        .collect();

    // One listing for all of them rather than a question apiece. A pattern
    // matches a whole prefix, so `refs/heads/main` would also list a
    // `refs/heads/main/x`; comparing whole names below is what keeps that from
    // standing in for a `main` that is not there.
    let mut args = vec!["for-each-ref", "--format=%(refname)"];
    args.extend(candidates.iter().map(String::as_str));
    let listed = answer(repository, &args)?;
    let existing: HashSet<&str> = listed.lines().collect();
    candidates
        .into_iter()
        .find(|candidate| existing.contains(candidate.as_str()))
}

/// Which of `branches` have landed on `base`, and how each was proved.
///
/// **Blocking**, and the most expensive read in the git module: a handful of
/// subprocesses per branch and one pass over the base's recent history, which
/// is a second or two on a busy repository. Background executor only, and
/// only on a gesture — the menu opening — never on a timer.
///
/// `branches` are short names, the form [`super::worktree::Worktree::branch`]
/// carries. `base` is a ref, the form [`base_of`] answers in. A branch absent
/// from the answer is unproved, which includes one that does not exist, one
/// git could not be asked about, and every one of them when git is missing.
///
/// `repository` may be any directory of the repository: the diffs are taken
/// with `diff.relative` switched off, since a configured one would narrow both
/// sides of the comparison to the directory it ran in, and two diffs that
/// agree about a subdirectory prove nothing about the rest.
pub fn merged(repository: &Path, base: &str, branches: &[String]) -> HashMap<String, Proof> {
    merged_within(repository, base, branches, PASS_COMMITS, PASS_DEADLINE)
}

/// [`merged`], with the pass bounded by `commits` and `deadline` rather than
/// the constants — which is how the tests reach a bound without building a
/// history a thousand commits long.
fn merged_within(
    repository: &Path,
    base: &str,
    branches: &[String],
    commits: usize,
    deadline: Duration,
) -> HashMap<String, Proof> {
    let mut landed = HashMap::new();
    // The branches ancestry did not prove: each one's fork from the base and
    // the patch-id of everything it has done since.
    let mut pending = Vec::new();

    for branch in branches {
        // In full, so that a tag or a file named like the branch is not what
        // git goes and reads.
        let reference = format!("refs/heads/{branch}");
        if is_ancestor(repository, &reference, base) {
            // Its diff since the fork is empty either way, so a branch
            // ancestry does not prove has nothing for the patch to prove.
            if worked_on(repository, &reference) {
                landed.insert(branch.clone(), Proof::Ancestor);
            }
            continue;
        }
        let Some(fork) = merge_base(repository, base, &reference) else {
            continue;
        };
        if let Some(patch) = patch_since(repository, &fork, &reference) {
            pending.push((branch, reference, fork, patch));
        }
    }
    if pending.is_empty() {
        return landed;
    }

    let forks: Vec<&str> = pending
        .iter()
        .map(|(_, _, fork, _)| fork.as_str())
        .collect();
    let since = oldest(repository, &forks);
    let patches = patches_on(repository, base, since.as_deref(), commits, deadline);
    for (branch, reference, fork, patch) in pending {
        let Some(candidates) = patches.get(&patch) else {
            continue;
        };
        // The pass reads back to the oldest fork of all of them, so for a
        // branch that left the base later it also reads commits from before
        // that — and one of those can never be this branch's squash. A change
        // the base made, took back, and this branch then made again has the
        // patch-id of the first of those, while the base no longer has the
        // change at all. Newest first, so a real squash of the same change
        // after the fork is the one found.
        let Some(commit) = candidates
            .iter()
            .find(|commit| after_fork(repository, commit, &fork))
        else {
            continue;
        };
        if worked_on(repository, &reference) {
            landed.insert(
                branch.clone(),
                Proof::Patch {
                    commit: commit.clone(),
                },
            );
        }
    }
    landed
}

/// What git printed for `args`, trimmed, when it succeeded and printed
/// anything at all.
fn answer(repository: &Path, args: &[&str]) -> Option<String> {
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    run(repository, &args, Intent::Read)
        .ok()
        .filter(|finished| finished.success)
        .map(|finished| String::from_utf8_lossy(&finished.stdout).trim().to_owned())
        .filter(|answer| !answer.is_empty())
}

/// Whether `reference`'s tip is on `base`.
///
/// Every way this can fail — git missing, a branch that does not exist —
/// reads as `false`, which sends the branch on to [`merge_base`], which
/// fails the same way and drops it.
fn is_ancestor(repository: &Path, reference: &str, base: &str) -> bool {
    let args = ["merge-base", "--is-ancestor", reference, base].map(OsStr::new);
    run(repository, &args, Intent::Read).is_ok_and(|finished| finished.success)
}

/// Whether `reference`'s own reflog records a commit made on it.
///
/// What each proof is otherwise blind to is a branch that holds no work of its
/// own. One just made from the base is on the base; one made there and then
/// only brought up to date — `rebase`, `pull`, `merge --ff-only`, before its
/// first commit — is on the base too; and one cut from the tip of a branch the
/// forge squashed has that branch's whole diff. All three are what an agent is
/// handed and has not started on yet, and "merged" on any of them is untrue.
///
/// What they share is that nothing was ever committed on them. git's own
/// ways of writing a commit onto the branch that is checked out record
/// themselves in that branch's reflog under the names in [`WORK`], and
/// nothing that only moves a branch does: creating one writes `branch:
/// Created from …`, a fast-forward `merge …: Fast-forward` or `pull …:
/// Fast-forward`, a rebase `rebase (finish): …`, a reset `reset: moving to
/// …`. The names are git's own and are not translated.
///
/// Every doubt reads as "not worked on", which only ever takes a proof away:
/// a reflog git could not read, and a branch whose only commits were written
/// some other way — on a detached `HEAD`, mid-rebase or before the branch was
/// named, or by a tool that moves the branch with a message of its own.
///
/// The exception is a reflog that no longer says what happened, which is not
/// known to be empty-handed and is left to the proofs as git would leave it.
/// One is no reflog at all: made with `core.logAllRefUpdates` off, or every
/// entry expired. The other is a reflog git has pruned. `gc` expires an entry
/// after thirty days (`gc.reflogExpireUnreachable`) once either commit it
/// names is one the branch no longer holds, so an amend or a rebase takes the
/// entry of every commit it replaced with it, its own entry too, and leaves
/// the creation — which reads exactly like a branch nobody committed on. What
/// tells the two apart is the newest entry left: every move writes one that
/// ends at the new tip, so a reflog whose newest entry ends anywhere else has
/// lost the ones after it, or the branch was moved by something that wrote
/// none. The three branches above lose nothing to that prune, since every
/// entry of theirs names a commit they still hold, and stay unproved.
///
/// That still misses a rebase less than a month old of commits more than a
/// month old: once gc has pruned the commits' entries, the rebase's is the
/// newest, ends at the tip and is not [`WORK`], and the branch is unproved
/// until that entry expires too.
fn worked_on(repository: &Path, reference: &str) -> bool {
    let args = ["log", "-g", "--format=%H %gs", reference].map(OsStr::new);
    let Ok(finished) = run(repository, &args, Intent::Read) else {
        return false;
    };
    if !finished.success {
        return false;
    }
    let reflog = String::from_utf8_lossy(&finished.stdout);
    let entries: Vec<(&str, &str)> = reflog
        .lines()
        .filter_map(|entry| entry.split_once(' '))
        .collect();
    let Some((newest, _)) = entries.first() else {
        return true;
    };
    if entries
        .iter()
        .any(|(_, entry)| WORK.iter().any(|work| entry.starts_with(work)))
    {
        return true;
    }
    answer(repository, &["rev-parse", "--verify", "--quiet", reference])
        .is_some_and(|tip| tip != *newest)
}

/// How git's reflog begins the entry for each way of writing a commit onto the
/// branch that is checked out: `commit`, with its `(initial)`, `(amend)` and
/// `(merge)` forms; `cherry-pick`; `revert`; and `am`, which applies a mailed
/// patch.
///
/// A merge git finishes by itself — `merge …: Merge made by …` — is left out
/// on purpose: it records other work coming in, not work made here. One that
/// somebody had to finish by hand is `commit (merge)`, and counts, because
/// the resolution was.
const WORK: [&str; 4] = ["commit", "cherry-pick:", "revert:", "am:"];

/// Whether `commit` reached the base after `fork` did: it is neither `fork`
/// nor one of the commits `fork` descends from.
///
/// Asked as "is there anything `commit` reaches that `fork` does not", which
/// is `commit` itself exactly when the answer is yes — so that a git that
/// could not be asked answers no, and a patch-id match it could not place
/// proves nothing.
fn after_fork(repository: &Path, commit: &str, fork: &str) -> bool {
    let exclude = format!("^{fork}");
    answer(
        repository,
        &["rev-list", "--max-count=1", commit, &exclude, "--"],
    )
    .is_some()
}

/// Where `reference` left `base`.
fn merge_base(repository: &Path, base: &str, reference: &str) -> Option<String> {
    answer(repository, &["merge-base", base, reference])
}

/// The flags every diff here is taken with, so that the branch's side and
/// the base's side are the same kind of text.
///
/// Each one is there because a setting somebody may have made would otherwise
/// change what the diff says. An external diff driver or a text conversion
/// runs a program of the user's instead of git's diff; colour puts escapes in
/// the text `patch-id` hashes; abbreviated object ids are as long as the
/// repository needs at that moment, and a binary file's patch-id is made of
/// them; and a submodule setting that hid a submodule's move from both sides
/// would let two changes that differ only there prove each other.
const DIFF_FLAGS: [&str; 5] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--full-index",
    "--ignore-submodules=none",
];

/// `patch-id --verbatim`, which is the half of each pipe that does the
/// proving: it hashes a patch with its line numbers taken out and every
/// character of every line kept, and sums the files' hashes so that their
/// order does not matter — `--verbatim` implies `--stable`, and git refuses
/// the two together.
///
/// Not plain `--stable`, which also takes the whitespace out of each line
/// before hashing it: a branch whose change differs from a squash on the base
/// only in indentation would be proved by it — its checkout marked merged and
/// offered for tidying, and, once branches are deleted on a proof, the branch
/// deleted too — though in a Python file, a YAML file or a Makefile the
/// indentation is the change. A git that predates the flag (2.39) fails the
/// pipe, which proves nothing — the answer this module gives to every doubt.
const PATCH_ID: [&str; 2] = ["patch-id", "--verbatim"];

/// The patch-id of everything `reference` changed since `fork`, or `None`
/// when it changed nothing — a branch whose commits cancel out has no patch
/// to find, and proving it by the empty patch would prove it against every
/// empty commit on the base.
fn patch_since(repository: &Path, fork: &str, reference: &str) -> Option<String> {
    let mut diff = vec!["-c", "diff.relative=false", "diff"];
    diff.extend(DIFF_FLAGS);
    diff.extend([fork, reference, "--"]);
    let diff: Vec<&OsStr> = diff.into_iter().map(OsStr::new).collect();

    let finished = pipe(repository, &diff, &PATCH_ID.map(OsStr::new), READ_TIMEOUT).ok()?;
    if !finished.success {
        return None;
    }
    let text = String::from_utf8_lossy(&finished.stdout);
    text.split_whitespace().next().map(str::to_owned)
}

/// The fork the pass has to read back to: the oldest of `forks`.
///
/// Every fork is on the base, so the one the others all descend from is the
/// point before which no branch here can have landed. `--octopus` is that
/// commit, and on a base with merges in it — where the forks need not lie on
/// one line — it is a commit they all descend from, which reads at most a
/// little more than it has to. `None` is no bound at all, and the cap is then
/// the only thing that stops the pass.
fn oldest(repository: &Path, forks: &[&str]) -> Option<String> {
    let unique: HashSet<&str> = forks.iter().copied().collect();
    if unique.len() == 1 {
        return unique.into_iter().next().map(str::to_owned);
    }
    let mut args = vec!["merge-base", "--octopus"];
    args.extend(unique);
    answer(repository, &args)
}

/// The patch-id of each of the newest `commits` commits on `base` since
/// `since`, mapped to every commit that has it, newest first.
///
/// One `git log -p` piped into one `patch-id`, which is the whole cost of
/// asking about any number of branches. Merges are left out: a merge's own
/// diff is not a change anybody wrote, and a branch merged by one is an
/// ancestor and proved without this.
///
/// Empty when the pass did not finish — killed at `deadline`, or refused. What
/// a killed pass had printed is thrown away rather than used: every line of it
/// would be a true patch-id, but a pass that read half the range is not the
/// pass the caller asked for, and nothing here gets to prove anything with an
/// answer nobody can describe.
///
/// Every commit and not only one, because two commits on a base can share a
/// patch-id — a change made, taken back and made again — and which of them
/// can prove a branch depends on where that branch left the base.
fn patches_on(
    repository: &Path,
    base: &str,
    since: Option<&str>,
    commits: usize,
    deadline: Duration,
) -> HashMap<String, Vec<String>> {
    let cap = format!("--max-count={commits}");
    let exclude = since.map(|since| format!("^{since}"));

    let mut pass = vec!["-c", "diff.relative=false", "log", "-p", "--no-merges"];
    pass.extend(DIFF_FLAGS);
    // `commit <id>` is what `patch-id` reads as the start of the next commit
    // and prints back beside its patch-id; spelled out rather than left to the
    // default format, which a `format.pretty` setting can change into
    // something it would not recognise.
    pass.extend(["--format=commit %H", &cap, base]);
    pass.extend(exclude.as_deref());
    pass.push("--");
    let pass: Vec<&OsStr> = pass.into_iter().map(OsStr::new).collect();

    let finished = match pipe(repository, &pass, &PATCH_ID.map(OsStr::new), deadline) {
        Ok(finished) if finished.success => finished,
        Ok(finished) => {
            log::debug!("the pass over {base} failed: {}", finished.stderr.trim());
            return HashMap::new();
        }
        Err(failure) => {
            log::warn!("the pass over {base} did not finish: {failure:?}");
            return HashMap::new();
        }
    };

    let mut patches: HashMap<String, Vec<String>> = HashMap::new();
    for (patch, commit) in String::from_utf8_lossy(&finished.stdout)
        .lines()
        .filter_map(|line| line.split_once(' '))
    {
        patches
            .entry(patch.to_owned())
            .or_default()
            .push(commit.trim().to_owned());
    }
    patches
}

#[cfg(test)]
#[path = "merged_tests.rs"]
mod tests;
