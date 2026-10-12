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
//! Nor is either proof taken for a branch that moved off work of its own on
//! the way. One somebody committed on and then reset back onto the base — an
//! agent told to start over, `git reset --hard origin/main` — has its tip on
//! the base and a commit in its reflog, which is all the two proofs ask; but
//! that commit is on no branch, and the branch's reflog is the last thing
//! that remembers it, which deleting the branch deletes. So every commit a
//! branch's reflog records as made on it has to be somewhere still, or its
//! change has to be: see [`dropped`]. A rebase, an amend and a squash made by
//! hand move a branch off commits too, and each of those leaves the change
//! in what the branch went on to hold. The change is looked for as the lines
//! a commit added and removed, without the lines around them: a rebase onto
//! a base that edited a line near the branch's, or one whose conflict was
//! settled by keeping both sides, gives each copy new surroundings and the
//! same lines. Those lines are where the look starts and never where it
//! ends, because the same edit made to another, identical line of the same
//! file — a second `timeout=3`, a second `replicas: 1` — has them too, and
//! leaves the branch's own line as it was. A commit that has them accounts
//! for one the branch moved off only when it is that commit as git copies
//! one, by the same author at the same second, or has them among the same
//! lines around them as well. A commit undone with a reset and made again
//! differently leaves its lines in nothing, and so does one whose conflict
//! was settled by rewriting the branch's own lines; their branch is unproved
//! until `gc` expires the entry, a month on.
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
//! older than 2.39 does not have: there `patch-id` refuses the flag, nothing
//! is proved by patch — only by ancestry — and no change a branch moved off
//! can be looked for, so a branch that moved off commits is proved only when
//! an amend is what moved it.
//!
//! Everything is read-only and runs through [`super::run`]'s deadline. A pass
//! over history — the base's, and a branch's beside it when [`unaccounted`]
//! looks for a change — is bounded twice, by [`PASS_COMMITS`] and
//! [`PASS_DEADLINE`], because those are the reads here whose length grows
//! with the repository rather than with the question. The passes that look
//! for the change of what branches moved off share one deadline between
//! them, so that however many branches did, the look costs one more deadline
//! at most.
//!
//! # What a proof is good for
//!
//! That day has come: [`super::worktree::delete_branch`] deletes a branch on
//! the strength of one. So a proof is a value, [`Landed`], that only
//! [`merged`] can make — no code outside this module can write "I think it
//! landed" in a form the deletion will accept — and it says what it was a
//! proof *of*: the branch, the base, and the one commit that was the branch's
//! tip when it was proved. A branch that has moved since is a different
//! question, which the proof never answered.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, Instant};

use super::run::{Intent, READ_TIMEOUT, pipe, run, run_fed};

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

/// How many commits a branch was moved off [`unaccounted`] looks for the
/// change of before it takes every one of them as lost.
///
/// Each is named on a command line, and what a branch moves off in the month
/// before `gc` expires the entries is a rebase or two of its own commits and
/// an amend here and there: a dozen, most often. One past this many keeps no
/// proof until those entries expire, which costs a badge; reading the change
/// of hundreds of commits would cost a long wait on a menu.
const MOST_DROPPED: usize = 64;

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

/// A branch [`merged`] proved has landed, and exactly what it proved.
///
/// Every field is private and this module is the only place one is made, so
/// holding one means the proof was run: it is what
/// [`super::worktree::delete_branch`] takes instead of a name, and a caller
/// cannot hand it a branch nobody proved. Cloning one copies a proof that
/// was made; it does not make a new one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landed {
    /// The branch, as a short name.
    branch: String,
    /// The ref it was proved against, as [`base_of`] names one.
    base: String,
    /// The branch's tip when it was proved, as a full object id. Every
    /// question the proof asked was asked of this commit rather than of the
    /// name, which an agent can move while the questions are being asked.
    tip: String,
    /// How it was proved.
    proof: Proof,
    /// The subject line of the commit a [`Proof::Patch`] names, when git
    /// would say it: the evidence a person reads before a branch is deleted.
    subject: Option<String>,
}

impl Landed {
    /// A proof that has just gone through.
    ///
    /// Private, and that is the whole of what makes a `Landed` mean
    /// something: [`merged_within`] is the only caller.
    fn new(branch: &str, base: &str, tip: String, proof: Proof) -> Self {
        Self {
            branch: branch.to_owned(),
            base: base.to_owned(),
            tip,
            proof,
            subject: None,
        }
    }

    /// The branch, as a short name.
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// The ref the branch was proved to have landed on.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The commit the branch's tip was when it was proved, in full.
    pub fn tip(&self) -> &str {
        &self.tip
    }

    /// How it was proved.
    pub fn proof(&self) -> &Proof {
        &self.proof
    }

    /// The subject of the commit a patch proof found on the base, if the
    /// proof was by patch and git said what it was.
    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }
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

/// A ref as a person says it: `main` for `refs/heads/main`, `origin/main`
/// for `refs/remotes/origin/main`, and anything else whole.
///
/// For showing [`base_of`]'s answer, which is always one of the first two.
pub fn short_name(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
}

/// Whether `branch`, a short name, is `base` — or the local branch a
/// remote-tracking `base` shares its name with.
///
/// The second, because a base read off `origin/HEAD` is
/// `refs/remotes/origin/main`, and a local `main` behind it with a commit of
/// somebody's own on it is proved by ancestry like any other branch — and is
/// the one branch in the repository nobody finishing a task means.
pub fn is_base(branch: &str, base: &str) -> bool {
    let local = base.strip_prefix("refs/heads/");
    let remote = base
        .strip_prefix("refs/remotes/")
        .and_then(|rest| rest.split_once('/'))
        .map(|(_, name)| name);
    local == Some(branch) || remote == Some(branch)
}

/// Which of `branches` have landed on `base`, and how each was proved.
///
/// **Blocking**, and the most expensive read in the git module: a handful of
/// subprocesses per branch and one pass over the base's recent history, which
/// is a second or two on a busy repository. A branch a proof reached that
/// moved off commits of its own — a rebase or an amend in the month before
/// `gc` prunes its reflog — adds [`nothing_dropped`]'s look for their change:
/// a second pass over the base, shared by every such branch, and a pass over
/// the own commits of each one proved by patch, which all share one more
/// [`PASS_DEADLINE`]; and a few subprocesses for each of up to
/// [`MOST_DROPPED`] commits a branch moved off that no pass settles.
/// Background executor only, and only on a gesture — the menu opening —
/// never on a timer.
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
pub fn merged(repository: &Path, base: &str, branches: &[String]) -> HashMap<String, Landed> {
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
) -> HashMap<String, Landed> {
    // Every branch a proof has reached, with its reflog, which is asked last
    // whether the branch moved off work on the way that is now lost.
    let mut proved = Vec::new();
    // The branches ancestry did not prove: each one's tip, its fork from the
    // base, and the patch-id of everything it has done since.
    let mut pending = Vec::new();

    for branch in branches {
        // In full, so that a tag or a file named like the branch is not what
        // git goes and reads.
        let reference = format!("refs/heads/{branch}");
        // Read once, and every question after is asked of the commit rather
        // than of the name: a branch an agent commits on while this runs
        // would otherwise have its ancestry asked about one tip and its patch
        // about the next, and the proof would be about neither.
        let Some(tip) = answer(
            repository,
            &["rev-parse", "--verify", "--quiet", &reference],
        ) else {
            continue;
        };
        if is_ancestor(repository, &tip, base) {
            // Its diff since the fork is empty either way, so a branch
            // ancestry does not prove has nothing for the patch to prove.
            if let Some(reflog) =
                reflog_of(repository, &reference).filter(|reflog| worked_on(reflog, &tip))
            {
                proved.push((Landed::new(branch, base, tip, Proof::Ancestor), reflog));
            }
            continue;
        }
        let Some(fork) = merge_base(repository, base, &tip) else {
            continue;
        };
        if let Some(patch) = patch_since(repository, &fork, &tip, Hunks::InContext) {
            pending.push((branch, reference, tip, fork, patch));
        }
    }
    if pending.is_empty() {
        return nothing_dropped(repository, base, proved, commits, deadline);
    }

    let forks: Vec<&str> = pending
        .iter()
        .map(|(_, _, _, fork, _)| fork.as_str())
        .collect();
    let since = oldest(repository, &forks);
    let patches = patches_on(
        repository,
        &[base],
        since.as_deref(),
        commits,
        deadline,
        Hunks::InContext,
    );
    for (branch, reference, tip, fork, patch) in pending {
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
        if let Some(reflog) =
            reflog_of(repository, &reference).filter(|reflog| worked_on(reflog, &tip))
        {
            let proof = Proof::Patch {
                commit: commit.clone(),
            };
            proved.push((Landed::new(branch, base, tip, proof), reflog));
        }
    }

    let mut landed = nothing_dropped(repository, base, proved, commits, deadline);
    let subjects = {
        let commits: Vec<&str> = landed
            .values()
            .filter_map(|landed| match &landed.proof {
                Proof::Patch { commit } => Some(commit.as_str()),
                Proof::Ancestor => None,
            })
            .collect();
        subjects(repository, &commits)
    };
    for landed in landed.values_mut() {
        if let Proof::Patch { commit } = &landed.proof {
            landed.subject = subjects.get(commit).cloned();
        }
    }
    landed
}

/// The commits `branch` was moved off that nothing else holds and whose
/// change nothing else holds either — what its reflog is the last thing to
/// remember, and what deleting it at `tip` loses with that reflog — newest
/// first, as full object ids.
///
/// **Blocking**: the reflog, a `rev-list`, and, only for a branch that has
/// moved off commits of its own, two passes over history — the base's since
/// the oldest of them left it, and the branch's own commits — which share one
/// [`PASS_DEADLINE`]. Background executor only.
///
/// "Nothing else" is `tip`, `base` and every branch, tag and remote-tracking
/// branch, the refs [`super::worktree::held_only_by`] counts as somewhere a
/// commit still is once a branch has gone. "Its change" is as [`unaccounted`]
/// looks for it. `base` is `None` only in a repository [`base_of`] found none
/// in, where the change is looked for in what `tip` holds alone. `None` when
/// git could not say.
///
/// [`merged`] proves no branch this is not empty for. Discard, which deletes
/// a branch without a proof, names what it holds.
pub fn dropped(
    repository: &Path,
    base: Option<&str>,
    branch: &str,
    tip: &str,
) -> Option<Vec<String>> {
    let reflog = reflog_of(repository, &format!("refs/heads/{branch}"))?;
    let made = made_on(&reflog);
    let nowhere = unheld(repository, base, &made, &[tip])?;
    let moved_off: Vec<String> = made
        .into_iter()
        .filter(|commit| nowhere.contains(*commit))
        .map(str::to_owned)
        .collect();
    if moved_off.is_empty() || moved_off.len() > MOST_DROPPED {
        return Some(moved_off);
    }

    let until = Instant::now() + PASS_DEADLINE;
    let on_base = match base {
        Some(base) => {
            let since = moved_off
                .last()
                .and_then(|oldest| merge_base(repository, base, oldest));
            held_on(
                repository,
                &[base],
                since.as_deref(),
                PASS_COMMITS,
                left(until),
            )
        }
        None => HashMap::new(),
    };
    let held = Held {
        on_base: &on_base,
        on_tip: held_on(repository, &[tip], base, PASS_COMMITS, left(until)),
    };
    Some(unaccounted(
        repository, base, &reflog, &moved_off, &held, false,
    ))
}

/// The proofs in `proved` whose branch moved off no work that is lost — see
/// [`dropped`] — by branch. Each comes with its branch's reflog, already read
/// for [`worked_on`].
///
/// One `rev-list` for all of them, which is the whole of what this costs a
/// branch that never moved off a commit of its own: almost every one. The
/// rest share one pass over the base, back to where the oldest of their
/// commits left it, and a branch proved by patch reads its own commits
/// besides, which are on no base. Not the patch proof's pass, which hashes
/// each change with the lines around it: these hash the lines alone — see
/// [`Hunks::Bare`]. Each pass is capped at `commits`, and all of them
/// together have `deadline`, so that the look costs one more deadline at
/// most however many branches it is for. A git that could not be asked
/// takes every proof away.
fn nothing_dropped(
    repository: &Path,
    base: &str,
    proved: Vec<(Landed, Vec<Entry>)>,
    commits: usize,
    deadline: Duration,
) -> HashMap<String, Landed> {
    let made: Vec<&str> = proved
        .iter()
        .flat_map(|(_, reflog)| made_on(reflog))
        .collect();
    let tips: Vec<&str> = proved.iter().map(|(landed, _)| landed.tip()).collect();
    let Some(nowhere) = unheld(repository, Some(base), &made, &tips) else {
        return HashMap::new();
    };
    let moved_off: Vec<Vec<String>> = proved
        .iter()
        .map(|(_, reflog)| {
            made_on(reflog)
                .into_iter()
                .filter(|commit| nowhere.contains(*commit))
                .map(str::to_owned)
                .collect()
        })
        .collect();

    let until = Instant::now() + deadline;
    // Where the oldest commit each branch moved off left the base, so that
    // one pass back to the oldest of those serves every branch.
    let forks: Vec<String> = moved_off
        .iter()
        .filter(|moved_off| (1..=MOST_DROPPED).contains(&moved_off.len()))
        .filter_map(|moved_off| merge_base(repository, base, moved_off.last()?))
        .collect();
    let on_base = if forks.is_empty() {
        HashMap::new()
    } else {
        let forks: Vec<&str> = forks.iter().map(String::as_str).collect();
        let needed = oldest(repository, &forks);
        held_on(repository, &[base], needed.as_deref(), commits, left(until))
    };

    proved
        .into_iter()
        .zip(moved_off)
        .filter(|((landed, reflog), moved_off)| {
            if moved_off.is_empty() {
                return true;
            }
            if moved_off.len() > MOST_DROPPED {
                return false;
            }
            let on_tip = match landed.proof() {
                Proof::Ancestor => HashMap::new(),
                Proof::Patch { .. } => held_on(
                    repository,
                    &[landed.tip()],
                    Some(base),
                    commits,
                    left(until),
                ),
            };
            let held = Held {
                on_base: &on_base,
                on_tip,
            };
            unaccounted(repository, Some(base), reflog, moved_off, &held, true).is_empty()
        })
        .map(|((landed, _), _)| (landed.branch.clone(), landed))
        .collect()
}

/// What is left of a deadline that ends at `until`, for the next pass that
/// shares it: nothing, once it is over, which a pass answers with nothing.
fn left(until: Instant) -> Duration {
    until.saturating_duration_since(Instant::now())
}

/// The commits [`unaccounted`] looks for a change among, by the patch-id of
/// what each changed on its own, [`Hunks::Bare`].
struct Held<'a> {
    /// The base's commits, back to where the commits it asks about left it.
    on_base: &'a HashMap<String, Vec<String>>,
    /// The branch's own commits, which the base does not have. Empty for a
    /// branch whose tip is on the base: its commits are among the base's.
    on_tip: HashMap<String, Vec<String>>,
}

impl Held<'_> {
    /// The commits still somewhere whose own change has the patch-id
    /// `patch`: the branch's, then the base's, newest first, and at most
    /// [`MOST_DROPPED`] of them.
    ///
    /// The branch's first, because a copy of a commit a branch moved off is
    /// most often on the branch; and no more than that many, because each is
    /// then named on a command line, and a change a base makes over and over
    /// — a version bumped, a line added to a changelog — can have hundreds
    /// of commits to its name. One cut off accounts for nothing, which costs
    /// a proof and never makes one.
    fn with(&self, patch: &str) -> Vec<&str> {
        self.on_tip
            .get(patch)
            .into_iter()
            .chain(self.on_base.get(patch))
            .flatten()
            .map(String::as_str)
            .take(MOST_DROPPED)
            .collect()
    }
}

/// Which of `moved_off` — commits `reflog` records as made on a branch that
/// nothing holds any more, newest first — have a change nothing holds
/// either. With `first`, it stops at the first, which is all a proof needs to
/// know.
///
/// The change is looked for in `held`: the commits the base has, back to
/// where the oldest of `moved_off` left it, and those the branch's tip has
/// that the base does not. A commit is accounted for in one of three ways,
/// which between them are how git moves a branch off a commit without
/// throwing its work away:
///
/// * **A copy** of it is held. A rebase or a cherry-pick copies each commit,
///   and a fast-forward or a squash of one lands it. A copy can stand among
///   other lines than its original did: the base it went onto may have
///   edited one beside it, or touched the same spot, and the conflict been
///   settled by keeping both. So a copy is looked for by the lines its
///   change adds and removes alone, [`Hunks::Bare`] — and those are not
///   enough on their own, because the same edit made to another, identical
///   line of the same file has them too, and leaves the original's line as
///   it was. What tells a copy from that is what git carries over when it
///   makes one: the author, and the moment they wrote the commit, to the
///   second. A copy that lost them — a rebase told to reset the date — still
///   counts when its lines stand among the same lines around them as the
///   original's, [`Hunks::InContext`], which is what the patch proof asks of
///   a squash. A conflict settled by rewriting the branch's own lines leaves
///   no copy, and the commit unaccounted for.
/// * **An amend** replaced it — the reflog's next entry is `commit (amend)` —
///   with a commit that is held or accounted for. An amend rewrites a commit
///   in place, so its draft's change is in nothing by design, and the draft
///   goes the way of what replaced it.
/// * **Its whole change** — everything the branch had done when this was its
///   tip, `git diff <where it left the base> <commit>` — is the change one
///   held commit made, and then so is every commit it reached since. That is
///   a squash on the base of a branch since reset onto it, and a squash made
///   by hand with `reset --soft` and a `commit`. A squash is a new commit, by
///   whoever made it, when they made it, so only the lines can say; it is
///   found by its lines alone and kept only when the lines around them agree
///   too.
///
/// Anywhere in that history, and not only after where the commit left the
/// base, as the patch proof asks of a squash: the question here is not
/// whether a branch's work reached the base but whether a change it moved
/// off is still in something, and one the base had once is.
///
/// Every doubt leaves a commit unaccounted for: a read that failed, a commit
/// with no change at all, and every one of them past [`MOST_DROPPED`].
fn unaccounted(
    repository: &Path,
    base: Option<&str>,
    reflog: &[Entry],
    moved_off: &[String],
    held: &Held,
    first: bool,
) -> Vec<String> {
    if moved_off.is_empty() || moved_off.len() > MOST_DROPPED {
        return moved_off.to_vec();
    }
    let commits: Vec<&str> = moved_off.iter().map(String::as_str).collect();
    let own = own_patches(repository, &commits, Hunks::Bare);
    // The held commits that make each one's change, and who wrote every one
    // of them, in one read.
    let alike: HashMap<&str, Vec<&str>> = commits
        .iter()
        .filter_map(|commit| {
            let alike = held.with(own.get(*commit)?);
            (!alike.is_empty()).then_some((*commit, alike))
        })
        .collect();
    let asked: HashSet<&str> = alike
        .iter()
        .flat_map(|(commit, alike)| std::iter::once(*commit).chain(alike.iter().copied()))
        .collect();
    let authors = authors(repository, &asked.into_iter().collect::<Vec<_>>());
    let amended = amendments(reflog);
    let nowhere: HashSet<&str> = commits.iter().copied().collect();

    let mut accounted: HashSet<String> = HashSet::new();
    let mut lost = Vec::new();
    // Newest first, so that an amend's replacement and a whole change's tip
    // are each settled before the commits they account for.
    for commit in moved_off {
        if accounted.contains(commit) {
            continue;
        }
        let replaced = amended
            .get(commit.as_str())
            .is_some_and(|by| !nowhere.contains(by) || accounted.contains(*by));
        // A copy second, since telling one can take reads that an amend's
        // replacement never needs.
        if replaced
            || alike
                .get(commit.as_str())
                .is_some_and(|alike| copied(repository, commit, alike, &authors))
        {
            accounted.insert(commit.clone());
            continue;
        }
        if let Some(reached) = base.and_then(|base| whole_held(repository, base, commit, held)) {
            accounted.extend(reached);
            accounted.insert(commit.clone());
            continue;
        }
        lost.push(commit.clone());
        if first {
            break;
        }
    }
    lost
}

/// Whether one of `alike` — held commits whose own change has `commit`'s
/// lines, [`Hunks::Bare`] — is a copy of `commit`: written by the same author
/// at the same moment, as `authors` has them, or making the change among the
/// same lines around it, [`Hunks::InContext`]. See [`unaccounted`].
fn copied(
    repository: &Path,
    commit: &str,
    alike: &[&str],
    authors: &HashMap<String, String>,
) -> bool {
    if let Some(author) = authors.get(commit)
        && alike.iter().any(|held| authors.get(*held) == Some(author))
    {
        return true;
    }
    let mut asked = vec![commit];
    asked.extend(alike);
    let patches = own_patches(repository, &asked, Hunks::InContext);
    patches
        .get(commit)
        .is_some_and(|patch| alike.iter().any(|held| patches.get(*held) == Some(patch)))
}

/// The commits `commit` reaches since it left `base`, when everything they
/// changed is the change one commit `held` has made: `commit`'s whole change
/// as a branch, found in one commit somewhere else. Found by its lines,
/// [`Hunks::Bare`], and kept only when the lines around them agree as well,
/// [`Hunks::InContext`] — see [`unaccounted`].
fn whole_held(repository: &Path, base: &str, commit: &str, held: &Held) -> Option<Vec<String>> {
    let fork = merge_base(repository, base, commit)?;
    let lines = patch_since(repository, &fork, commit, Hunks::Bare)?;
    let alike = held.with(&lines);
    if alike.is_empty() {
        return None;
    }
    let whole = patch_since(repository, &fork, commit, Hunks::InContext)?;
    let theirs = own_patches(repository, &alike, Hunks::InContext);
    if !theirs.values().any(|patch| *patch == whole) {
        return None;
    }
    let exclude = format!("^{fork}");
    let reached = answer(repository, &["rev-list", commit, &exclude, "--"])?;
    Some(reached.lines().map(str::to_owned).collect())
}

/// The commits of [`patches_on`]'s pass, [`Hunks::Bare`], by patch-id: what
/// [`Held`] is made of.
fn held_on(
    repository: &Path,
    heads: &[&str],
    since: Option<&str>,
    commits: usize,
    deadline: Duration,
) -> HashMap<String, Vec<String>> {
    patches_on(repository, heads, since, commits, deadline, Hunks::Bare)
}

/// The patch-id of what each of `commits` changed on its own, against its
/// first parent, hashed as `hunks` says, by commit.
///
/// One `git log -p` piped into one `patch-id`, as the pass is. A merge has no
/// patch here, and nor has anything when git could not be asked.
fn own_patches(repository: &Path, commits: &[&str], hunks: Hunks) -> HashMap<String, String> {
    if commits.is_empty() {
        return HashMap::new();
    }
    let mut log = vec![
        "-c",
        "diff.relative=false",
        "log",
        "-p",
        "--no-walk=unsorted",
    ];
    log.extend(DIFF_FLAGS);
    log.extend(hunks.flags());
    log.push("--format=commit %H");
    log.extend(commits);
    log.push("--");
    let log: Vec<&OsStr> = log.into_iter().map(OsStr::new).collect();

    match pipe(repository, &log, &PATCH_ID.map(OsStr::new), READ_TIMEOUT) {
        Ok(finished) if finished.success => String::from_utf8_lossy(&finished.stdout)
            .lines()
            .filter_map(|line| line.split_once(' '))
            .map(|(patch, commit)| (commit.trim().to_owned(), patch.to_owned()))
            .collect(),
        _ => HashMap::new(),
    }
}

/// Who wrote each of `commits` and when — the name, the address, and the
/// moment to the second with its zone — as one string apiece, by commit.
///
/// What git carries over to the copy when a rebase, a cherry-pick or an
/// amend rewrites a commit, and what two commits written apart do not share:
/// see [`unaccounted`]. The commits go in on stdin, for [`unheld`]'s reason.
/// Empty when git could not be asked.
fn authors(repository: &Path, commits: &[&str]) -> HashMap<String, String> {
    if commits.is_empty() {
        return HashMap::new();
    }
    let input: String = commits.iter().map(|commit| format!("{commit}\n")).collect();
    // `raw` is seconds and a zone, which no `log.date` setting rounds; and
    // `log.showSignature` would put gpg's lines among the answers.
    let args = [
        "log",
        "--no-walk=unsorted",
        "--no-show-signature",
        "--stdin",
        "--date=raw",
        "--format=%H %ad %an <%ae>",
        "--",
    ]
    .map(OsStr::new);
    run_fed(repository, &args, input.into_bytes())
        .ok()
        .filter(|finished| finished.success)
        .map(|finished| {
            String::from_utf8_lossy(&finished.stdout)
                .lines()
                .filter_map(|line| line.split_once(' '))
                .map(|(commit, author)| (commit.to_owned(), author.to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Each commit an amend replaced, mapped to the commit that replaced it.
///
/// Read off the order of `reflog`, newest first: an entry that is an amend
/// moved the branch off the commit the entry after it — the one before it in
/// time — had left it at.
fn amendments(reflog: &[Entry]) -> HashMap<&str, &str> {
    reflog
        .windows(2)
        .filter(|pair| pair[0].message.starts_with(AMEND))
        .map(|pair| (pair[1].commit.as_str(), pair[0].commit.as_str()))
        .collect()
}

/// How git's reflog begins the entry for `commit --amend`, one of the
/// [`WORK`] entries.
const AMEND: &str = "commit (amend)";

/// Every commit `reflog` records as made on the branch — its [`WORK`]
/// entries — once each, newest first.
fn made_on(reflog: &[Entry]) -> Vec<&str> {
    let mut seen = HashSet::new();
    reflog
        .iter()
        .filter(|entry| entry.is_work())
        .map(|entry| entry.commit.as_str())
        // An object id and nothing else: anything that is not one names no
        // commit, and would have git refuse the whole question it was in.
        .filter(|commit| !commit.is_empty() && commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .filter(|commit| seen.insert(*commit))
        .collect()
}

/// Which of `commits`, and of the commits they reach, nothing holds: none of
/// `tips`, not `base`, and no branch, tag or remote-tracking branch. `None`
/// when git could not say.
///
/// The commits go in on stdin rather than on the command line: a branch
/// worked on for a year has a commit in its reflog for every one made on it,
/// and Windows gives a command line 32,767 characters.
fn unheld(
    repository: &Path,
    base: Option<&str>,
    commits: &[&str],
    tips: &[&str],
) -> Option<HashSet<String>> {
    if commits.is_empty() {
        return Some(HashSet::new());
    }
    let mut input = String::new();
    for commit in commits {
        input.push_str(commit);
        input.push('\n');
    }
    for tip in tips {
        input.push('^');
        input.push_str(tip);
        input.push('\n');
    }
    // `--ignore-missing`, for a commit git no longer has: nothing a deletion
    // could lose, and otherwise a refusal of the whole question.
    let mut args = vec!["rev-list", "--ignore-missing", "--stdin", "--not"];
    args.extend(base);
    args.extend(["--branches", "--tags", "--remotes", "--"]);
    let args: Vec<&OsStr> = args.into_iter().map(OsStr::new).collect();

    let finished = run_fed(repository, &args, input.into_bytes())
        .ok()
        .filter(|finished| finished.success)?;
    Some(
        String::from_utf8_lossy(&finished.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
    )
}

/// The subject line of each of `commits`, by full object id.
///
/// One `git log` for all of them rather than one apiece, because a sweep can
/// prove a dozen squashes at once and this is only what is shown beside them.
/// Empty when git could not be asked, which costs a line of evidence and
/// never a proof.
fn subjects(repository: &Path, commits: &[&str]) -> HashMap<String, String> {
    if commits.is_empty() {
        return HashMap::new();
    }
    let mut args = vec!["log", "--no-walk=unsorted", "--format=%H %s"];
    args.extend(commits);
    args.push("--");
    answer(repository, &args)
        .map(|listed| {
            listed
                .lines()
                .filter_map(|line| line.split_once(' '))
                .map(|(commit, subject)| (commit.to_owned(), subject.to_owned()))
                .collect()
        })
        .unwrap_or_default()
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

/// One entry of a branch's reflog.
struct Entry {
    /// The commit the move left the branch at, in full.
    commit: String,
    /// git's words for the move: `commit: <subject>`, `reset: moving to …`.
    message: String,
}

impl Entry {
    /// Whether the move wrote a commit onto the branch: see [`WORK`].
    fn is_work(&self) -> bool {
        WORK.iter().any(|work| self.message.starts_with(work))
    }
}

/// `reference`'s reflog, newest entry first, or `None` when git could not
/// read it.
fn reflog_of(repository: &Path, reference: &str) -> Option<Vec<Entry>> {
    // `log.showSignature` would put gpg's lines among the entries, each read
    // as one more move, to a commit nobody can name.
    let args = [
        "log",
        "-g",
        "--no-show-signature",
        "--format=%H %gs",
        reference,
    ]
    .map(OsStr::new);
    let finished = run(repository, &args, Intent::Read)
        .ok()
        .filter(|finished| finished.success)?;
    Some(
        String::from_utf8_lossy(&finished.stdout)
            .lines()
            .filter_map(|entry| entry.split_once(' '))
            .map(|(commit, message)| Entry {
                commit: commit.to_owned(),
                message: message.to_owned(),
            })
            .collect(),
    )
}

/// Whether `reflog`, a branch's own, records a commit made on it.
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
/// a reflog git could not read, which [`reflog_of`] answers with nothing to
/// ask this of, and a branch whose only commits were written some other way
/// — on a detached `HEAD`, mid-rebase or before the branch was named, or by a
/// tool that moves the branch with a message of its own.
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
///
/// `tip` is the commit the proof is about, read before any of this was asked.
fn worked_on(reflog: &[Entry], tip: &str) -> bool {
    let Some(newest) = reflog.first() else {
        return true;
    };
    if reflog.iter().any(Entry::is_work) {
        return true;
    }
    tip != newest.commit
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
/// only in indentation would be proved by it, and deleted on the strength of
/// it, though in a Python file, a YAML file or a Makefile the indentation is
/// the change. A git that predates the flag (2.39) fails the pipe, which
/// proves nothing — the answer this module gives to every doubt.
const PATCH_ID: [&str; 2] = ["patch-id", "--verbatim"];

/// How much of each change a diff handed to [`PATCH_ID`] carries: the proofs
/// and [`unaccounted`] ask different questions of the same commits.
#[derive(Clone, Copy)]
enum Hunks {
    /// Each change with the lines git shows around it. What the proofs hash:
    /// a branch has landed when the base has its change where the branch
    /// made it, and a commit adding the same line to another part of the
    /// file has made a different change. And what [`unaccounted`] asks of a
    /// match it found by [`Self::Bare`] and cannot otherwise tell from the
    /// same edit made elsewhere.
    InContext,
    /// The lines each change adds and removes and nothing else,
    /// `--unified=0`. What [`unaccounted`] looks for a change by: a rebase
    /// gives a copy new lines around its own whenever the base it went onto
    /// edited one near them — or touched the same spot, and the conflict was
    /// settled by keeping both. Never a match on its own, because the same
    /// edit to another, identical line of the file has the same lines.
    Bare,
}

impl Hunks {
    /// What asks a diff for this, beside [`DIFF_FLAGS`].
    fn flags(self) -> &'static [&'static str] {
        match self {
            Self::InContext => &[],
            Self::Bare => &["--unified=0"],
        }
    }
}

/// The patch-id of everything `reference` changed since `fork`, or `None`
/// when it changed nothing — a branch whose commits cancel out has no patch
/// to find, and proving it by the empty patch would prove it against every
/// empty commit on the base.
fn patch_since(repository: &Path, fork: &str, reference: &str, hunks: Hunks) -> Option<String> {
    let mut diff = vec!["-c", "diff.relative=false", "diff"];
    diff.extend(DIFF_FLAGS);
    diff.extend(hunks.flags());
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

/// The patch-id of each of the newest `commits` commits `heads` reach since
/// `since`, mapped to every commit that has it, newest first.
///
/// `heads` is the base alone for the proof by patch, which hashes
/// [`Hunks::InContext`], and the base or a branch's tip for [`unaccounted`],
/// which hashes [`Hunks::Bare`].
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
    heads: &[&str],
    since: Option<&str>,
    commits: usize,
    deadline: Duration,
    hunks: Hunks,
) -> HashMap<String, Vec<String>> {
    let cap = format!("--max-count={commits}");
    let exclude = since.map(|since| format!("^{since}"));

    let mut pass = vec!["-c", "diff.relative=false", "log", "-p", "--no-merges"];
    pass.extend(DIFF_FLAGS);
    pass.extend(hunks.flags());
    // `commit <id>` is what `patch-id` reads as the start of the next commit
    // and prints back beside its patch-id; spelled out rather than left to the
    // default format, which a `format.pretty` setting can change into
    // something it would not recognise.
    pass.extend(["--format=commit %H", &cap]);
    pass.extend(heads);
    pass.extend(exclude.as_deref());
    pass.push("--");
    let pass: Vec<&OsStr> = pass.into_iter().map(OsStr::new).collect();

    let finished = match pipe(repository, &pass, &PATCH_ID.map(OsStr::new), deadline) {
        Ok(finished) if finished.success => finished,
        Ok(finished) => {
            log::debug!("the pass over {heads:?} failed: {}", finished.stderr.trim());
            return HashMap::new();
        }
        Err(failure) => {
            log::warn!("the pass over {heads:?} did not finish: {failure:?}");
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
