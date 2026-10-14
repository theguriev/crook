//! `tab.new`: a tab opened beside the pane that asked, running a command.
//!
//! What an agent in one pane uses to fan its work out into others it can be
//! seen doing: `crook tab new --worktree fix-x --in-my-group -- claude "…"`
//! opens a row with a dot, a branch and a checkout of its own, rather than a
//! sub-agent running invisibly inside the one pane.
//!
//! # Who is asking
//!
//! Only a pane may open a tab, and it says which pane it is with the token its
//! shell was handed in [`TOKEN_VARIABLE`](super::TOKEN_VARIABLE). A request
//! with none, or with one no open pane of this window holds, is refused as
//! `unauthorized` — that is every process this user runs outside a pane, a
//! script, a cron job, a tool in another terminal, which can all reach the
//! socket. The token is in the environment of everything the pane runs, so
//! anything the agent starts can open tabs as the agent; that is the boundary
//! meant, since a pane is one piece of work and what runs in it acts for it.
//!
//! Only into the window the token belongs to, since its socket is the one the
//! pane was told about and no token names a pane of any other; beside the
//! caller's tab, and in its group when asked. Never selected: the person may
//! be typing somewhere, and a tab that took the keyboard would take the rest
//! of their line with it.
//!
//! # The budget
//!
//! [`SPAWN_BUDGET`] tabs at once, counted against the pane a person opened
//! that the asking began at — the caller, or the root its own lineage names —
//! and counting the tabs whose worktree is still being checked out. A worker
//! that opens workers spends its root's budget rather than one of its own,
//! which is what keeps a lead from being eight tabs, each of them eight more.
//! A tab that closes gives its place back.
//!
//! Refusals are bounded the way a plugin's are: [`REFUSALS_ALLOWED`] in a row
//! and the window stops answering that pane's `tab.new` until it closes, with
//! one line in the log to say so. An agent that answers a refusal by asking
//! again is a loop, and each turn of it is a process, a connection and a line
//! in the log; sixteen is far more than a pane has reason to be refused, and
//! the count starts again at every tab that opens for it. Only at one that
//! opens: a worktree git will not make is refused after the request was
//! agreed to, and a loop asking for that branch is a loop like any other.
//! And never once the pane is stopped: a worktree agreed to before the stop
//! can finish after it, and the tab still opens, but the stop stands.
//!
//! # The worktree
//!
//! Made through the one function the worktree menu's creator makes its
//! checkouts with, `HeldLocks::make_checkout`, so a worker's checkout is the
//! one a person would have got from the menu: locked while the window works
//! in it, and given what the main checkout's `.worktreeinclude` names before
//! the tab's shell starts. Only its base differs — the caller's `HEAD`, where
//! the menu offers a choice. What the copy left out is said in the answer's
//! `note`, where the menu would reopen on the new tab to say it: a tab that
//! never takes the keyboard has no business opening a menu that does.
//!
//! # The command
//!
//! Words rather than a line, joined here with the quoting the plugin host
//! types an argument with — `quote` in `plugins::wasm` — so that every word
//! arrives in the new pane's shell as itself: a quote, a `$(…)` or a backtick
//! in it is a character, and an alias or a keyword is a word. That quoting is
//! proven in `sh`, `bash`, `zsh` and `fish`, and a pane that would run any
//! other shell is refused rather than handed a line it may read differently.
//! The line is typed into the new pane's field and sent at its first prompt:
//! see `Workspace::run_at_first_prompt`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crookui_core::prelude::*;

use super::Answering;
use super::protocol::{NewTab, Opened, Refusal, code};
use crate::git::worktree;
use crate::plugins::wasm::quote;
use crate::tab::{Lineage, PaneId, TabId};
use crate::workspace::Workspace;

/// How many tabs may be open on behalf of one pane a person opened.
///
/// Eight: a lead fanning a task out wants a handful of workers, and the
/// fan-out a person can still watch — a row, a dot and a branch each — is a
/// panel's worth, not a page's. A loop that opens tabs is stopped at a number
/// a person can close by hand. Counted as described in the module docs.
pub const SPAWN_BUDGET: usize = 8;

/// How many refusals in a row a pane gets before the window stops answering
/// its `tab.new` for the rest of the pane's life.
///
/// Sixteen, which is the plugin host's number for the same loop — see
/// `REFUSALS_ALLOWED` in the wasm runtime — and for its reason: far more than
/// a pane has cause to be refused, and few enough that a loop stops being
/// free.
pub const REFUSALS_ALLOWED: u32 = 16;

/// The shells whose quoting `quote` is proven in: the ones the plugin host's
/// test reads a quoted word back out of.
const QUOTED_SHELLS: [&str; 4] = ["sh", "bash", "zsh", "fish"];

/// What the window remembers between one `tab.new` and the next.
///
/// Owned by the chain that answers the socket and shared with the answers it
/// hands to the pool, so it lives exactly as long as the window answers.
#[derive(Debug, Default)]
pub struct Spawns {
    /// How many times in a row each pane has been refused.
    refusals: HashMap<PaneId, u32>,
    /// How many times in a row a request that named no pane has been refused.
    /// Only the log is bounded by it: there is nobody to stop answering.
    strangers: u32,
    /// How many tabs each root has accepted and not yet opened: the ones
    /// waiting on git for their worktree.
    opening: HashMap<PaneId, usize>,
}

impl Spawns {
    /// Whether the window has stopped answering this pane.
    pub fn stopped(&self, caller: PaneId) -> bool {
        self.refusals
            .get(&caller)
            .is_some_and(|count| *count >= REFUSALS_ALLOWED)
    }

    /// Counts a refusal of `caller`, and logs it while there is still reason
    /// to.
    pub fn refuse(&mut self, caller: PaneId, refusal: &Refusal) {
        let count = self.refusals.entry(caller).or_default();
        *count = count.saturating_add(1);
        if *count <= REFUSALS_ALLOWED {
            log::info!(
                "pane {} was refused a tab: {} ({})",
                caller.as_u64(),
                refusal.message,
                refusal.code
            );
        }
        if *count == REFUSALS_ALLOWED {
            log::warn!(
                "pane {} has been refused {REFUSALS_ALLOWED} tabs in a row and will not be \
                 answered again until it closes",
                caller.as_u64()
            );
        }
    }

    /// Counts a refusal of a request that carried no pane's token.
    pub fn refuse_stranger(&mut self) {
        self.strangers = self.strangers.saturating_add(1);
        if self.strangers <= REFUSALS_ALLOWED {
            log::info!("a request with no pane's token was refused a tab");
        }
        if self.strangers == REFUSALS_ALLOWED {
            log::warn!(
                "{REFUSALS_ALLOWED} requests with no pane's token have been refused a tab in a \
                 row; the next are refused without a line here"
            );
        }
    }

    /// Holds a place in `root`'s budget for a tab accepted and not yet open,
    /// until [`Self::settled`].
    ///
    /// Not an end to the caller's run of refusals: the tab may still not
    /// open — git may refuse the worktree — and a loop asking for a branch
    /// git will never make has to add up to [`REFUSALS_ALLOWED`] like any
    /// other. That is [`Self::opened`]'s.
    pub(super) fn accept(&mut self, root: PaneId) {
        *self.opening.entry(root).or_default() += 1;
    }

    /// Starts `caller`'s count of refusals again, and the strangers' with it:
    /// a tab opened for it, so it is a pane in the ordinary state again.
    ///
    /// Unless the window has stopped answering it. A worktree agreed to
    /// before the stop can open after it, while git ran, and a stop that
    /// such a tab lifted would be sixteen more turns of the loop for every
    /// slow checkout it had in flight, where the refusal promised none.
    pub(super) fn opened(&mut self, caller: PaneId) {
        if !self.stopped(caller) {
            self.refusals.remove(&caller);
        }
        self.strangers = 0;
    }

    /// Gives back the place [`Self::accept`] held for `root`: the tab is open
    /// now and counted as one, or it never will be.
    fn settled(&mut self, root: PaneId) {
        if let Some(count) = self.opening.get_mut(&root) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.opening.remove(&root);
            }
        }
    }

    /// How many tabs `root` has accepted and not yet opened.
    fn opening(&self, root: PaneId) -> usize {
        self.opening.get(&root).copied().unwrap_or(0)
    }

    /// Forgets the panes that have closed: a pane number is never handed out
    /// twice, so what was counted against one that is gone is only memory.
    fn forget_closed(&mut self, open: impl Fn(PaneId) -> bool) {
        self.refusals.retain(|pane, _| open(*pane));
    }
}

/// Answers one `tab.new`.
///
/// Everything that can be said at once is said at once. A tab in the
/// caller's own directory opens now; one in a new worktree waits for git on
/// the pool and answers when that comes home, with its place in the budget
/// held while it waits.
pub fn open(
    workspace: &mut Workspace,
    asked: NewTab,
    token: Option<&str>,
    spawns: &Rc<RefCell<Spawns>>,
    answer: Answering,
    ctx: &mut ViewContext<Workspace>,
) {
    spawns
        .borrow_mut()
        .forget_closed(|pane| workspace.tabs().pane(pane).is_some());

    let Some(caller) = token.and_then(|token| workspace.pane_with_token(token, ctx)) else {
        spawns.borrow_mut().refuse_stranger();
        answer.send(Err(Refusal::new(
            code::UNAUTHORIZED,
            "only a pane of this window can open a tab, and the request carries no pane's \
             CROOK_TOKEN; run `crook tab new` inside a Crook pane",
        )));
        return;
    };
    if spawns.borrow().stopped(caller) {
        answer.send(Err(Refusal::new(
            code::TOO_MANY_REFUSALS,
            format!(
                "this pane has been refused {REFUSALS_ALLOWED} tabs in a row, and this window \
                 opens no more for it until it closes"
            ),
        )));
        return;
    }

    let planned = plan(workspace, asked, caller, &spawns.borrow(), ctx);
    let plan = match planned {
        Ok(plan) => plan,
        Err(refusal) => {
            spawns.borrow_mut().refuse(caller, &refusal);
            answer.send(Err(refusal));
            return;
        }
    };
    let root = plan.lineage.root;
    spawns.borrow_mut().accept(root);

    let Some(branch) = plan.branch.clone() else {
        spawns.borrow_mut().settled(root);
        finish(workspace, plan, None, spawns, answer, ctx);
        return;
    };
    let (Some(repository), Some(store)) = (
        plan.directory.clone(),
        workspace.worktrees_directory().map(Path::to_path_buf),
    ) else {
        // Checked in `plan`, which refuses either missing; here only so the
        // types say so.
        spawns.borrow_mut().settled(root);
        answer.send(Err(Refusal::new(
            code::FAILED,
            "there is nowhere to make the worktree",
        )));
        return;
    };

    // The worktree menu's own steps, in its order: the listing names the
    // repository the checkout is filed under, and the checkout is made the
    // one way the menu's creator makes its own — locked, and given what
    // `.worktreeinclude` names — from the caller's `HEAD`.
    let held = workspace.held_locks();
    let made = ctx.background().spawn(async move {
        let listed = worktree::list(&repository)?;
        let name = listed
            .first()
            .and_then(|main| main.path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or(worktree::Error::NotARepository)?;
        let path = worktree::checkout_path(&store, &name, &branch);
        let included = held.make_checkout(&repository, &path, &branch, None)?;
        Ok::<_, worktree::Error>(Made {
            path,
            repository: name,
            lacks: included.problem(),
        })
    });
    let spawns = spawns.clone();
    ctx.spawn(made, move |workspace, made, ctx| {
        spawns.borrow_mut().settled(root);
        match made {
            Ok(made) => finish(workspace, plan, Some(made), &spawns, answer, ctx),
            Err(error) => {
                let refusal = Refusal::new(
                    code::FAILED,
                    format!("git could not make the worktree: {error}"),
                );
                spawns.borrow_mut().refuse(plan.lineage.caller, &refusal);
                answer.send(Err(refusal));
            }
        }
    })
    .detach();
}

/// A worktree git has made for a tab that is not open yet.
struct Made {
    /// Where it is, which is where the tab opens.
    path: PathBuf,
    /// The name of the repository it is a checkout of, which names the group
    /// the tab joins, as the worktree menu names the group it opens into.
    repository: String,
    /// What the main checkout's `.worktreeinclude` named that the worktree
    /// was not given, as a sentence: see `Included::problem`.
    lacks: Option<String>,
}

/// A `tab.new` the window has agreed to, before the tab is open.
struct Plan {
    /// The caller's tab, which the new one opens beside.
    beside: TabId,
    /// Whether it joins the caller's group.
    grouped: bool,
    /// The caller's directory: where the tab opens, or the repository its
    /// worktree is made from.
    directory: Option<PathBuf>,
    /// The branch to make a worktree on, when one was asked for.
    branch: Option<String>,
    /// What to call it.
    title: Option<String>,
    /// The command line, quoted for the new pane's shell.
    line: String,
    /// Where it came from.
    lineage: Lineage,
}

/// Decides whether `caller` may have the tab it asked for, and what it is.
fn plan(
    workspace: &Workspace,
    asked: NewTab,
    caller: PaneId,
    spawns: &Spawns,
    app: &AppContext,
) -> Result<Plan, Refusal> {
    let bad = |message: String| Refusal::new(code::BAD_REQUEST, message);
    let strip = workspace.tabs();
    let Some((beside, session)) = strip
        .panes()
        .find(|(_, pane)| pane.id() == caller)
        .map(|(tab, pane)| (tab, pane.session()))
    else {
        // A token is only ever found for an open pane; this is a pane that
        // closed between the two reads, which cannot happen on one thread.
        return Err(Refusal::new(code::GONE, "the pane that asked has closed"));
    };

    let program = workspace.shell_standing(app).0.program;
    let line = command_line(&asked.command, &program)?;

    let title = match asked.title.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(title) if title.chars().any(char::is_control) => {
            return Err(bad(
                "a title is one line of text, with no control character".to_owned(),
            ));
        }
        Some(title) => Some(title.to_owned()),
    };

    let directory = session.working_directory.clone();
    // Trimmed and required, as the worktree menu's creator takes it; the
    // name itself is git's to judge, and a name git refuses comes back as
    // what git said.
    let branch = match asked.worktree.as_deref().map(str::trim) {
        None => None,
        Some("") => return Err(bad("a worktree needs a branch name".to_owned())),
        Some(branch) => {
            if directory.is_none() {
                return Err(Refusal::new(
                    code::FAILED,
                    "this pane has no directory to find a repository in",
                ));
            }
            if workspace.worktrees_directory().is_none() {
                return Err(Refusal::new(
                    code::FAILED,
                    "this machine has no data directory for Crook to keep worktrees in",
                ));
            }
            Some(branch.to_owned())
        }
    };

    let root = session
        .spawned_by
        .as_ref()
        .map_or(caller, |lineage| lineage.root);
    let open = strip
        .panes()
        .filter(|(_, pane)| {
            pane.session()
                .spawned_by
                .as_ref()
                .is_some_and(|lineage| lineage.root == root)
        })
        .count();
    if open + spawns.opening(root) >= SPAWN_BUDGET {
        return Err(Refusal::new(
            code::BUDGET,
            format!(
                "{SPAWN_BUDGET} tabs are already open on behalf of the pane this work began in, \
                 which is as many as it may have; wait for one of them to close — `crook pane \
                 list` shows them — rather than asking again"
            ),
        ));
    }

    Ok(Plan {
        beside,
        grouped: asked.in_my_group,
        directory,
        branch,
        title,
        line,
        lineage: Lineage {
            caller,
            root,
            title: super::title(session, workspace.home()),
        },
    })
}

/// Opens the tab a [`Plan`] describes, in the worktree `made` for it or the
/// caller's own directory, and answers with it.
///
/// The one place a caller's run of refusals ends, since it is the one place a
/// tab is known to have opened — short of a stop, which [`Spawns::opened`]
/// leaves standing.
fn finish(
    workspace: &mut Workspace,
    plan: Plan,
    made: Option<Made>,
    spawns: &RefCell<Spawns>,
    answer: Answering,
    ctx: &mut ViewContext<Workspace>,
) {
    let Plan {
        beside,
        grouped,
        directory,
        title,
        line,
        lineage,
        ..
    } = plan;
    let caller = lineage.caller;
    let checkout = made.as_ref().map(|made| made.path.clone());
    let (directory, heading, lacks) = match made {
        Some(made) => (Some(made.path), Some(made.repository), made.lacks),
        None => (directory, None, None),
    };
    let cwd = directory
        .as_deref()
        .map(|directory| directory.to_string_lossy().into_owned());
    let opened = workspace.open_worker_tab(beside, grouped, directory, heading, ctx, |session| {
        session.spawned_by = Some(lineage);
        if title.is_some() {
            session.custom_title = title;
        }
    });
    // Held from here, as the menu's creator holds the checkouts it opens: the
    // lock comes off when no pane in the window is working in the checkout.
    // Whether or not the tab opened, since one that did not leaves nobody in
    // it, and a lock still waiting for its tab would stay on until the window
    // closed.
    if let Some(checkout) = &checkout {
        workspace.held_locks().opened(checkout);
    }
    let Some((tab, pane)) = opened else {
        let refusal = Refusal::new(code::FAILED, "the tab could not be opened");
        spawns.borrow_mut().refuse(caller, &refusal);
        answer.send(Err(refusal));
        return;
    };
    spawns.borrow_mut().opened(caller);
    workspace.run_at_first_prompt(pane, line, ctx);
    log::info!(
        "pane {} opened pane {} through the control socket",
        caller.as_u64(),
        pane.as_u64()
    );
    let opened = Opened {
        pane_id: pane.as_u64(),
        tab_id: tab.as_u64(),
        cwd,
        note: lacks,
    };
    answer.send(Ok(
        serde_json::to_value(opened).expect("an opened tab is numbers and text, which encode")
    ));
}

/// The command line `words` make, for a pane that runs `shell`.
///
/// Every word through the plugin host's `quote`, so each is one word to the
/// shell and nothing in it is expanded, substituted or globbed. Refused when
/// there is no word, when a word holds a control character — a newline is the
/// character that ends a command line, and would send the rest as a second
/// one — and when the shell is not one the quoting is proven in.
///
/// A control character is refused rather than written as an escape the shell
/// decodes — `$'…'` in bash and zsh, `\n` outside the quotes in fish — since
/// that is a second quoting with rules of its own in each shell, and `sh` has
/// none to offer; the one here is the quoting proven in all four (#342). A
/// prompt with lines goes in a file the command names, and the refusal says
/// so, since the brief a lead hands a worker is the prompt that has them. Its
/// example lets the worker read that file: a `--worktree` tab starts in
/// another directory, and Claude Code asks before it reads outside its own,
/// which a `claude -p` worker is refused and an interactive one stops at. The
/// rule names the brief and nothing else — the `/` before an absolute `$PWD`
/// makes the `//` that marks a rule's path as absolute — since
/// `--add-dir "$PWD"` would make the lead's checkout a directory the worker
/// may write in too, and under acceptEdits without asking, which is what
/// `--worktree` keeps it from.
pub fn command_line(words: &[String], shell: &Path) -> Result<String, Refusal> {
    if words.is_empty() {
        return Err(Refusal::new(
            code::BAD_REQUEST,
            "`tab.new` needs a command to run: `crook tab new -- <command>…`",
        ));
    }
    if words.iter().any(|word| word.chars().any(char::is_control)) {
        return Err(Refusal::new(
            code::BAD_REQUEST,
            "a word of the command holds a control character, and a newline in a command line \
             is a second command; keep the prompt on one line, or write it to a file and pass \
             one line that names the file by its full path, letting the worker read that file: \
             `claude --allowedTools \"Read(/$PWD/task.md)\" -- \"do the task in $PWD/task.md\"`",
        ));
    }
    if !quoting_is_proven(shell) {
        return Err(Refusal::new(
            code::UNSUPPORTED_SHELL,
            format!(
                "a new pane here runs {}, and Crook types a command only into {}, the shells \
                 its quoting is proven in",
                shell.display(),
                QUOTED_SHELLS.join(", ")
            ),
        ));
    }
    Ok(words
        .iter()
        .map(|word| quote(word))
        .collect::<Vec<_>>()
        .join(" "))
}

/// Whether `quote`'s words are read back as themselves by `shell`.
///
/// By the program's name, as the shell integration tells shells apart. Never
/// on Windows, where the quoting is PowerShell's and not the one proven for
/// these four.
fn quoting_is_proven(shell: &Path) -> bool {
    let name = shell
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    !cfg!(windows) && QUOTED_SHELLS.contains(&name.as_str())
}
