//! Finishing a task, and discarding one: what the two entries on the menu of
//! a tab in a checkout Crook made do, and the question each asks first.
//!
//! # The rule this reverses
//!
//! `git::worktree` said from the day it was written that a branch is never
//! deleted — "not as an option, not under `force`" — because a branch is the
//! work, and deleting it was "not a thing a terminal gets to decide." That
//! held for as long as nothing could tell a finished branch from an
//! unfinished one. [`crate::git::merged`] now proves, from the repository
//! alone, that a branch's work is on the base, and a branch proved that way is
//! no longer where the work is: it is a name git will call unmerged for ever
//! over commits whose change lives on the base. So Finish deletes a branch —
//! that one, on that proof, made again the moment before it goes, and refused
//! if the branch has moved since. A branch the proof cannot reach is kept, and
//! the question says so before anything is pressed: "Keeps feat/x: not on
//! main yet."
//!
//! Discard is the one way past the proof, and it is two questions. The first
//! says what goes, as Finish's does. The second names every commit that only
//! that branch holds and every file in the checkout that would go with it,
//! under a button that says "for good" — and Enter answers the first and never
//! the second, the rule the worktree menu's "Remove anyway" keeps.
//!
//! # What finishing does
//!
//! One press, and then, in order:
//!
//! 1. Every pane working in the checkout closes, in whichever tabs they are.
//!    Not the tab's *group*: a worktree opens into the group of the tab it was
//!    made from, so a group is a repository — the main checkout and every task
//!    beside it — and closing it would end work that has nothing to do with
//!    this one. Refused, with a sentence naming the pane, while any of them is
//!    working: an agent running or waiting on an answer, or a command open in
//!    its shell. Closing a pane ends its process, and a question that only
//!    asked about the checkout would be ending work nobody was asked about.
//! 2. The checkout is removed, through the same `git worktree remove` the ×
//!    uses. Finish never forces, so a checkout holding modified or untracked
//!    files, or commits on no branch, is one Finish will not take and says
//!    so; Discard forces, and its second question has named what that loses.
//! 3. The branch goes if the proof holds — or, for Discard, whether or not it
//!    does, provided it has not moved since its commits were counted.
//!
//! The panes go first because a shell sitting in a directory is a process
//! holding it — on Windows, one whose directory cannot be deleted — and
//! because nothing should be writing into a checkout while it is deleted.
//! Which leaves the answer with nowhere to be said: the menu hung off a row
//! that has just closed. So the menu moves to a tab in the repository's main
//! checkout — one already there, or one opened there — and says what happened
//! on its worktree list, which is where the checkout was and where a person
//! looks to see that it has gone.

use std::path::{Path, PathBuf};

use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::git::merged::{Landed, short_name};
use crate::git::worktree::{Commit, Local, Worktree};
use crate::tab::{AgentSession, AgentStatus, PaneId};

use super::action::WorktreeAction;
use super::tab_menu::{
    NAMED, busy_line, buttons, divider, evidence, header, holding, local_summary, named, note,
    stranded_summary,
};
use super::view::Workspace;

/// What the question about finishing a task is showing.
#[derive(Clone, Debug, Default)]
pub(super) enum Finishing {
    /// The checkout is being looked at: what is loose in it, whether its
    /// branch has landed, and what only that branch holds.
    #[default]
    Looking,
    /// What the look found.
    Ready(Box<Plan>),
    /// It could not be looked at, or it is not a checkout Crook made; why.
    Failed(String),
    /// The button has been pressed, and this is what is being waited on.
    Working(String),
}

impl Finishing {
    /// Whether git is being waited on, which is when the pirate chews.
    pub(super) fn is_working(&self) -> bool {
        matches!(self, Self::Looking | Self::Working(_))
    }
}

/// Everything the question needs to say what will happen, read once when it
/// opens.
#[derive(Clone, Debug)]
pub(super) struct Plan {
    /// Every checkout the repository has, main one first — the one being
    /// finished among them, and the one the menu moves to afterwards.
    pub(super) worktrees: Vec<Worktree>,
    /// Which of them is being finished.
    pub(super) index: usize,
    /// What is loose in it.
    pub(super) local: Local,
    /// Its modified and untracked files, by path: what Discard throws away.
    pub(super) loose: Vec<String>,
    /// The ref work lands on, if the repository has one.
    pub(super) base: Option<String>,
    /// The branch's tip when it was looked at, if it is on a branch.
    pub(super) tip: Option<String>,
    /// The proof that the branch has landed, if it has.
    pub(super) landed: Option<Landed>,
    /// The commits only the branch holds, newest first, when it has not
    /// landed: what deleting it loses.
    pub(super) lost: Vec<Commit>,
}

impl Plan {
    /// The checkout being finished.
    pub(super) fn checkout(&self) -> &Worktree {
        &self.worktrees[self.index]
    }

    /// The repository's main checkout: where git is run once the one being
    /// finished has gone, and where the menu moves to.
    pub(super) fn main(&self) -> &Worktree {
        &self.worktrees[0]
    }

    /// What the list calls the checkout: its branch, or the commit it is on.
    fn label(&self) -> String {
        let checkout = self.checkout();
        match (&checkout.branch, &checkout.head) {
            (Some(branch), _) => branch.clone(),
            (None, Some(head)) => format!("detached at {head}"),
            (None, None) => "the checkout".to_owned(),
        }
    }

    /// The base as a person says it.
    fn base_name(&self) -> Option<&str> {
        self.base.as_deref().map(short_name)
    }
}

/// Looks at the checkout `directory` is in, for the question about finishing
/// it.
///
/// **Blocking**: a listing, two `git status`es, the proof — a pass over the
/// base's history — and the commits only the branch holds. Background pool
/// only.
///
/// Refused, with a sentence, for anything that is not a linked checkout
/// under `store`: the entries are offered only there, and a checkout somebody
/// made by hand elsewhere is theirs to finish, not Crook's.
pub(super) fn read_plan(directory: &Path, store: &Path) -> Result<Plan, String> {
    use crate::git::worktree;

    let worktrees = worktree::list(directory).map_err(|problem| problem.to_string())?;
    let index = holding(&worktrees, Some(directory))
        .ok_or_else(|| "This tab is in none of the repository's checkouts.".to_owned())?;
    let checkout = &worktrees[index];
    if checkout.is_main || !checkout.path.starts_with(store) {
        return Err("This tab is not in a checkout Crook made.".to_owned());
    }

    let local = worktree::local_work(&checkout.path).map_err(|problem| problem.to_string())?;
    let loose = worktree::loose_files(&checkout.path).map_err(|problem| problem.to_string())?;
    let base = crate::git::merged::base_of(directory);

    let (tip, landed, lost) = match &checkout.branch {
        None => (None, None, Vec::new()),
        Some(branch) => {
            let landed = base.as_deref().and_then(|base| {
                crate::git::merged::merged(directory, base, std::slice::from_ref(branch))
                    .remove(branch)
            });
            match landed {
                // Its commits are on no other ref once it goes, but their
                // change is on the base: nothing is lost that is not there.
                Some(landed) => (Some(landed.tip().to_owned()), Some(landed), Vec::new()),
                None => {
                    let tip = worktree::tip_of(directory, branch)
                        .map_err(|problem| problem.to_string())?;
                    // Counted from the tip just read, which is the tip
                    // Discard will refuse to delete past.
                    let lost = match &tip {
                        Some(tip) => worktree::held_only_by(directory, branch, tip)
                            .map_err(|problem| problem.to_string())?,
                        None => Vec::new(),
                    };
                    (tip, None, lost)
                }
            }
        }
    };

    Ok(Plan {
        worktrees,
        index,
        local,
        loose,
        base,
        tip,
        landed,
        lost,
    })
}

/// Every pane working in the checkout, with what its row calls it.
pub(super) fn panes_in(workspace: &Workspace, plan: &Plan) -> Vec<(PaneId, String)> {
    workspace
        .tabs()
        .panes()
        .filter(|(_, pane)| {
            let directory = pane.session().working_directory.as_deref();
            holding(&plan.worktrees, directory) == Some(plan.index)
        })
        .map(|(_, pane)| (pane.id(), pane.session().display_title().to_owned()))
        .collect()
}

/// What is still going on in a pane, as the end of a sentence, or `None`
/// when closing it ends nothing.
///
/// Both what the agent says and what the shell says, because each misses
/// what the other sees: an agent started without Crook's hooks reports
/// nothing and is still a command running, and one in a shell with no
/// command marks reports its status and opens no block. A failed agent has
/// stopped, and so has an idle one.
fn at_work(session: &AgentSession) -> Option<String> {
    if let Some(command) = &session.running_command {
        return Some(format!("{command} is still running in it"));
    }
    match session.status {
        AgentStatus::Running => Some("its agent is still working".to_owned()),
        AgentStatus::NeedsInput => Some("its agent is waiting for an answer".to_owned()),
        AgentStatus::Idle | AgentStatus::Failed => None,
    }
}

/// Why the button cannot be pressed right now, in a sentence, or `None` when
/// it can.
///
/// Asked on every frame, not once when the question opened, for the first
/// reason above all: an agent that finishes while a person reads the question
/// should light the button, and one that starts again should put it out. It
/// is only a walk over the panes.
pub(super) fn refusal(workspace: &Workspace, plan: &Plan, discard: bool) -> Option<String> {
    let working = workspace.tabs().panes().find_map(|(_, pane)| {
        let session = pane.session();
        let directory = session.working_directory.as_deref();
        if holding(&plan.worktrees, directory) != Some(plan.index) {
            return None;
        }
        let why = at_work(session)?;
        Some((session.display_title().to_owned(), why))
    });
    if let Some((title, why)) = working {
        return Some(format!("Not yet: {title} is busy — {why}."));
    }

    let checkout = plan.checkout();
    // Neither of the two ever overrides a lock, for the reason `remove`
    // gives: somebody else's session is holding the checkout.
    if let Some(reason) = &checkout.locked {
        return Some(if reason.is_empty() {
            "It is locked, and nothing here takes a lock.".to_owned()
        } else {
            format!("It is locked ({reason}), and nothing here takes a lock.")
        });
    }
    if !discard && plan.local.blocks_removal() {
        return Some(format!(
            "There is work in it — {}. Finish leaves a checkout like that where it is; Discard \
             throws the work away.",
            work_in(plan.local)
        ));
    }
    None
}

/// What in a checkout a plain removal would refuse over or lose, in a
/// phrase: `2 modified, 1 untracked`, and any commits on no branch.
fn work_in(local: Local) -> String {
    let mut parts = Vec::new();
    if local.modified > 0 {
        parts.push(format!("{} modified", local.modified));
    }
    if local.untracked > 0 {
        parts.push(format!("{} untracked", local.untracked));
    }
    match local.stranded {
        0 => {}
        1 => parts.push("1 commit on no branch".to_owned()),
        stranded => parts.push(format!("{stranded} commits on no branch")),
    }
    parts.join(", ")
}

/// The sentence about the branch on the first question.
fn branch_line(plan: &Plan, discard: bool) -> Option<String> {
    let branch = plan.checkout().branch.as_deref()?;
    Some(match (&plan.landed, plan.base_name(), discard) {
        (Some(landed), _, _) => format!("Deletes {branch}: {}.", evidence(landed)),
        (None, Some(base), false) => format!("Keeps {branch}: not on {base} yet."),
        (None, None, false) => format!("Keeps {branch}: there is no base to check it against."),
        (None, _, true) => match plan.lost.len() {
            0 => format!("Deletes {branch}. Every commit on it is on another branch too."),
            1 => format!("Deletes {branch}, and the 1 commit only it holds."),
            lost => format!("Deletes {branch}, and the {lost} commits only it holds."),
        },
    })
}

/// Finish's question, or either of Discard's.
pub(super) fn render(
    workspace: &Workspace,
    discard: bool,
    losing: bool,
    ui: FamilyId,
) -> Box<dyn Element> {
    let state = workspace.tab_menu();
    let title = match (discard, losing) {
        (false, _) => "Finish this task?",
        (true, false) => "Discard this task?",
        (true, true) => "Discard it for good?",
    };
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header(title, ui));
    let label = match (discard, losing) {
        (false, _) => "Finish",
        (true, false) => "Discard…",
        (true, true) => "Discard for good",
    };

    let plan = match &state.finishing {
        Finishing::Looking => {
            column.add_child(busy_line(state, "Looking at the checkout…", None, ui));
            column.add_child(buttons(workspace, label, None, ui));
            return column.finish();
        }
        Finishing::Working(sentence) => {
            column.add_child(busy_line(state, sentence, None, ui));
            column.add_child(buttons(workspace, label, None, ui));
            return column.finish();
        }
        Finishing::Failed(problem) => {
            column.add_child(note(problem, ui));
            column.add_child(buttons(workspace, label, None, ui));
            return column.finish();
        }
        Finishing::Ready(plan) => plan,
    };

    if losing {
        for line in losses(plan) {
            column.add_child(note(line, ui));
        }
    } else {
        let titles: Vec<String> = panes_in(workspace, plan)
            .into_iter()
            .map(|(_, title)| title)
            .collect();
        column.add_child(note(
            match titles.len() {
                0 => "No tab is working in it.".to_owned(),
                _ => format!("Closes {}.", named(&titles)),
            },
            ui,
        ));
        column.add_child(note(
            format!(
                "Removes {}.",
                crate::git::user_friendly_path(&plan.checkout().path, workspace.home())
            ),
            ui,
        ));
        if let Some(line) = branch_line(plan, discard) {
            column.add_child(note(line, ui));
        }
        let local = plan.local;
        if discard {
            if local.stranded > 0 {
                column.add_child(note(stranded_summary(local.stranded), ui));
            }
            if local.modified + local.untracked + local.ignored > 0 {
                column.add_child(note(local_summary(&local), ui));
            }
        } else if local.ignored > 0 && !local.blocks_removal() {
            // The one thing a removal git does not object to still deletes.
            column.add_child(note(
                format!("{} ignored in it will be deleted.", local.ignored),
                ui,
            ));
        }
    }

    let refused = refusal(workspace, plan, discard);
    if let Some(refused) = &refused {
        column.add_child(divider());
        column.add_child(note(refused, ui));
    }
    if let Some(problem) = &state.problem {
        column.add_child(note(problem.as_str(), ui));
    }

    let action = match (discard, losing) {
        (false, _) => WorktreeAction::Finish,
        (true, false) => WorktreeAction::ReviewDiscard,
        (true, true) => WorktreeAction::Discard,
    };
    column.add_child(buttons(
        workspace,
        label,
        refused.is_none().then_some(action),
        ui,
    ));
    column.finish()
}

/// Discard's second question, a line at a time: everything the press throws
/// away, by name — what the branch takes with it, then what the checkout
/// does.
fn losses(plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();

    let branch = plan.checkout().branch.clone();
    if let (Some(branch), None) = (&branch, &plan.landed)
        && !plan.lost.is_empty()
    {
        lines.push(format!("Only {branch} holds these, and they go with it:"));
        for commit in plan.lost.iter().take(NAMED) {
            lines.push(format!("{} {}", commit.id, commit.subject));
        }
        if plan.lost.len() > NAMED {
            lines.push(format!("and {} more.", plan.lost.len() - NAMED));
        }
    }
    if plan.local.stranded > 0 {
        lines.push(stranded_summary(plan.local.stranded));
    }
    if !plan.loose.is_empty() {
        lines.push("Not committed, and going with the checkout:".to_owned());
        for path in plan.loose.iter().take(NAMED) {
            lines.push(path.clone());
        }
        if plan.loose.len() > NAMED {
            lines.push(format!("and {} more.", plan.loose.len() - NAMED));
        }
    }
    if plan.local.ignored > 0 {
        lines.push(format!("{} ignored in it go too.", plan.local.ignored));
    }
    if lines.is_empty() {
        lines.push(match (&branch, &plan.landed) {
            (Some(branch), Some(landed)) => format!(
                "Nothing is lost that is not somewhere else: {branch} is on {}, and the checkout holds no work.",
                short_name(landed.base())
            ),
            _ => "Nothing is lost that is not somewhere else.".to_owned(),
        });
    }
    lines
}

/// Removes the checkout and deletes or keeps its branch, and answers what
/// happened in a sentence or two.
///
/// **Blocking**: the removal deletes a directory tree, and the proof is made
/// again before the branch goes. Background pool only. `repository` is the
/// main checkout, since the one being finished is about to be gone.
///
/// The branch is not touched unless the checkout went: a checkout git would
/// not remove still has it checked out, and the deletion would refuse it
/// anyway.
pub(super) fn carry_out(
    repository: &Path,
    plan: &Plan,
    discard: bool,
    store: Option<&Path>,
) -> String {
    use crate::git::worktree;

    let checkout = plan.checkout();
    if let Err(problem) = super::view::remove_checkout(repository, &checkout.path, discard, store) {
        return format!("The checkout of {} was kept: {problem}.", plan.label());
    }
    let removed = format!("Removed the checkout of {}.", plan.label());
    let Some(branch) = &checkout.branch else {
        return removed;
    };

    let branch_said = match (&plan.landed, discard) {
        (Some(landed), _) => match worktree::delete_branch(repository, landed) {
            Ok(()) => format!("Deleted {branch}: {}.", evidence(landed)),
            Err(problem) => kept(problem),
        },
        (None, false) => match plan.base_name() {
            Some(base) => format!("Kept {branch}: not on {base} yet."),
            None => format!("Kept {branch}: there is no base to check it against."),
        },
        (None, true) => {
            let Some(tip) = &plan.tip else {
                return removed;
            };
            match worktree::discard_branch(repository, branch, tip, plan.base.as_deref()) {
                Ok(()) => match plan.lost.len() {
                    0 => format!("Deleted {branch}."),
                    1 => format!("Deleted {branch} and the 1 commit only it held."),
                    lost => format!("Deleted {branch} and the {lost} commits only it held."),
                },
                Err(problem) => kept(problem),
            }
        }
    };
    format!("{removed} {branch_said}")
}

/// What the question says while it is being carried out.
pub(super) fn under_way(plan: &Plan, discard: bool) -> String {
    match discard {
        false => format!("Finishing {}…", plan.label()),
        true => format!("Discarding {}…", plan.label()),
    }
}

/// A deletion git or the checks refused, as the end of the report.
fn kept(problem: crate::git::worktree::Error) -> String {
    format!("{problem}, so it was kept.")
}

/// Where the menu goes once the checkout's panes have closed: a pane already
/// in the main checkout, if one is open and is not one of the ones closing.
pub(super) fn pane_in_main(
    workspace: &Workspace,
    plan: &Plan,
    closing: &[PaneId],
) -> Option<PaneId> {
    workspace
        .tabs()
        .panes()
        .find(|(_, pane)| {
            !closing.contains(&pane.id())
                && holding(&plan.worktrees, pane.session().working_directory.as_deref()) == Some(0)
        })
        .map(|(_, pane)| pane.id())
}

/// The main checkout's directory, when it is one a tab can be opened in — a
/// bare repository's is not.
pub(super) fn main_directory(plan: &Plan) -> Option<PathBuf> {
    let main = plan.main();
    (!main.is_bare && main.path.is_dir()).then(|| main.path.clone())
}
