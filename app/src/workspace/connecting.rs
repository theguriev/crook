//! What the window knows about connecting coding agents to it: which are
//! installed, where Crook's plugin stands in each, the tabs a Connect or an
//! Update is running in, and the one hint that offers a connection.
//!
//! # The tab
//!
//! A press on Connect or Update runs nothing itself. It opens a tab, names
//! it for what it is doing — "Connect Claude Code" — and types the agent's
//! own commands into it, sent at its first prompt the way `crook tab new`'s
//! line is: see [`crate::agent::connect::line()`]. The person sees every word
//! that runs and everything it prints, and the block's status is what the
//! Agents page reports. Only the line that tab was opened for counts: one
//! the person changed before it was sent is theirs, and the run is let go.
//!
//! # The hint
//!
//! A pane whose title has said for a while that an agent Crook has a plugin
//! for is at work — Claude Code's spinner, codex's — and whose hooks have
//! never reported, is a pane where the plugin is not installed: the plugin
//! reports a prompt as it is submitted. So the header offers it once, under
//! itself, the way it says a crash or a session another window keeps:
//! Connect, or Not now. Only for an agent this machine has, found where a
//! pane's shell finds it, and running in the pane — by its name or its
//! path, or through `npx` or `mise x`, which keep their plugins where the
//! local one does — rather than over `ssh`. Never while a question is
//! waiting, and never again for an agent somebody said Not now to; the
//! Agents page turns the hints off altogether. The hint takes no key and
//! moves no focus.
//!
//! What it offers is what the agent's own list says, asked once for that
//! agent when the hint is first due: Connect where the plugin is missing.
//! Where the list says it is there and the pane still reports nothing, the
//! plugin is not what is missing — a Claude Code session started before it
//! was installed loads it only on a restart, and Codex runs none of its
//! hooks until they are trusted — so the line says that instead, with no
//! Connect. For any other agent, or a list that could not be read, nothing.

use std::collections::{HashMap, HashSet};

use crookui_core::prelude::*;
use std::time::{Duration, Instant};

use crookui_core::elements::MouseStateHandle;

use crate::agent::connect::{Located, Standing, Step};
use crate::tab::{PaneId, TabId};

/// How long a pane's title says its agent is at work, with no hook having
/// reported, before the hint is offered: longer than the plugin's first
/// report of a prompt takes to arrive, which is well under a second.
pub(crate) const HINT_AFTER: Duration = Duration::from_secs(8);

/// How long an answer from the agents' own lists is taken as still true, so
/// going back and forth across the rail does not ask them each time.
const LOOK_AGAIN_AFTER: Duration = Duration::from_secs(60);

/// What an agent's list said, or that it is being asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Looked {
    /// Its list is being run.
    Asking,
    /// What the list said.
    Said(Standing),
    /// It could not be asked, and why.
    Unanswered(String),
}

/// A tab a press opened, while its line has not finished.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Run {
    agent: &'static str,
    step: Step,
    tab: TabId,
    /// Whether the line was sent, as typed. Until it is, a command that
    /// finishes in the pane is somebody else's.
    sent: bool,
}

/// How the last press for an agent went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Its line is running, or waiting to, in the tab of this name.
    Running { step: Step, title: String },
    /// Its line ended with status 0.
    Done(Step),
    /// Its line ended otherwise: the status, when the shell said one, and
    /// the pane it ran in, whose tab says why for as long as it is open.
    Failed {
        step: Step,
        title: String,
        exit: Option<i32>,
        pane: PaneId,
    },
}

/// What the hint says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HintKind {
    /// The agent's list says Crook's plugin is not installed: Connect, or
    /// Not now.
    Connect,
    /// The list says it is, and the pane reports nothing all the same: why
    /// that may be, and Dismiss.
    Why(&'static str),
}

/// What the hint says over a Claude Code that has the plugin and reports
/// nothing: hooks load as a session starts.
pub(crate) const CLAUDE_RESTART: &str =
    "Claude Code has Crook\u{2019}s plugin: restart it to load the plugin in this session";

/// What it says over such a Codex: it runs no hook it has not been told to
/// trust, a plugin's included.
pub(crate) const CODEX_TRUST: &str =
    "Codex holds Crook\u{2019}s plugin\u{2019}s hooks until you trust them, in its /hooks";

/// Asks each of `found` what it has, side by side, in `home`: each one's
/// own `plugin list --json`. On the pool.
pub(crate) fn ask(found: &[Located], home: &std::path::Path) -> Vec<(&'static str, Looked)> {
    use crate::agent::connect;
    std::thread::scope(|scope| {
        let asked: Vec<_> = found
            .iter()
            .map(|located| {
                let asking = scope.spawn(move || connect::look(&located.file, home, &[]));
                (located.agent, asking)
            })
            .collect();
        asked
            .into_iter()
            .map(|(agent, asking)| {
                let looked = match asking.join() {
                    Ok(Ok(listed)) => {
                        Looked::Said(connect::standing(&listed, env!("CARGO_PKG_VERSION")))
                    }
                    Ok(Err(why)) => Looked::Unanswered(why),
                    Err(_) => Looked::Unanswered(String::from("the look at it stopped")),
                };
                (agent, looked)
            })
            .collect()
    })
}

/// The hint, while it is up.
#[derive(Clone, Debug)]
pub(crate) struct Hint {
    /// The agent it offers to connect.
    pub(crate) agent: &'static str,
    /// The pane whose title it was offered over.
    pub(crate) pane: PaneId,
    /// What it says.
    pub(crate) kind: HintKind,
    pub(crate) connect: MouseStateHandle,
    pub(crate) not_now: MouseStateHandle,
}

/// Everything above, for one window.
#[derive(Default)]
pub(crate) struct Connections {
    /// The connectable agents installed here, once looked for.
    found: Option<Vec<Located>>,
    /// Whether that look is on the pool.
    finding: bool,
    /// What each agent's list said, by agent.
    looked: HashMap<&'static str, Looked>,
    /// When the lists were last asked, whatever they said.
    looked_at: Option<Instant>,
    /// Whether they are being asked now, on the pool.
    looking: bool,
    /// Whether Claude Code's files say it updates the plugin by itself.
    claude_updates: bool,
    /// The tabs a press opened, by the pane their line is in.
    runs: HashMap<PaneId, Run>,
    /// How the last press for each agent went, until the page is shown again
    /// after it ended.
    outcomes: HashMap<&'static str, Outcome>,
    /// Panes whose agent's hooks have reported: no hint is ever offered over
    /// one.
    heard: HashSet<PaneId>,
    /// Since when each pane's title has said its agent is at work, with no
    /// hook heard, and which agent.
    at_work: HashMap<PaneId, (&'static str, Instant)>,
    /// The hint, while it is up.
    hint: Option<Hint>,
    /// The agents a hint has been offered for in this window, or a press made
    /// for: the hint is offered once.
    hinted: HashSet<&'static str>,
    /// Whether this window asks nothing of the agents: a picture's, or one
    /// whose state a fixture wrote.
    held: bool,
    /// Where a test keeps Claude Code's files, in place of the person's.
    #[cfg(test)]
    claude_config: Option<std::path::PathBuf>,
    /// How long a title says an agent is at work before the hint is due:
    /// [`HINT_AFTER`], or a test's own.
    #[cfg(test)]
    hint_after: Option<Duration>,
}

impl Connections {
    /// The connectable agents installed here, once looked for.
    pub(crate) fn found(&self) -> Option<&[Located]> {
        self.found.as_deref()
    }

    /// The installed agent called `agent`, if it was found.
    pub(crate) fn located(&self, agent: &str) -> Option<&Located> {
        self.found
            .as_deref()?
            .iter()
            .find(|found| found.agent == agent)
    }

    /// Whether a look for the agents should start: none has, and none is on
    /// the pool. Marks it started.
    pub(crate) fn start_finding(&mut self) -> bool {
        if self.held || self.finding || self.found.is_some() {
            return false;
        }
        self.finding = true;
        true
    }

    /// What the look for the agents found.
    pub(crate) fn found_them(&mut self, found: Vec<Located>) {
        self.finding = false;
        self.found = Some(found);
    }

    /// Whether the agents' lists should be asked now: `force`d, or not asked
    /// in the last minute, and not being asked. Marks every found agent as
    /// being asked.
    pub(crate) fn start_looking(&mut self, force: bool, now: Instant) -> bool {
        if self.held {
            return false;
        }
        if self.looking {
            return false;
        }
        let fresh = self
            .looked_at
            .is_some_and(|at| now.duration_since(at) < LOOK_AGAIN_AFTER);
        if fresh && !force {
            return false;
        }
        self.looked_at = Some(now);
        self.looking = true;
        for found in self.found.iter().flatten() {
            self.looked.insert(found.agent, Looked::Asking);
        }
        true
    }

    /// What the agents' lists said, and whether Claude Code updates the
    /// plugin by itself.
    pub(crate) fn looked(
        &mut self,
        found: Vec<Located>,
        said: Vec<(&'static str, Looked)>,
        claude_updates: bool,
        unmarked: &[PaneId],
    ) {
        self.found = Some(found);
        self.finding = false;
        self.looking = false;
        self.looked.clear();
        self.looked.extend(said);
        self.claude_updates = claude_updates;
        // A press in a tab whose shell marks no commands is never heard to
        // end — cmd.exe, PowerShell, nushell, a startup file that execs tmux
        // — so the list stands in for it: the row says what the agent has.
        for pane in unmarked {
            if let Some(run) = self.runs.remove(pane) {
                self.outcomes.remove(run.agent);
            }
        }
        // A connection that ended is told by the list from here on, which
        // says what it came to. A failure stays, since the list cannot say
        // why — unless the list says the plugin is there now, which a press
        // since, or one in another window, or by hand, may have done.
        let looked = &self.looked;
        self.outcomes.retain(|agent, outcome| match outcome {
            Outcome::Done(_) => false,
            Outcome::Failed { .. } => !matches!(
                looked.get(agent),
                Some(Looked::Said(
                    Standing::Connected { .. } | Standing::Behind { .. }
                ))
            ),
            Outcome::Running { .. } => true,
        });
    }

    /// Marks `agent` as being asked, alone, for the hint: whether it should
    /// be, being neither asked already nor answered.
    pub(crate) fn start_looking_at(&mut self, agent: &'static str) -> bool {
        if self.held || self.looking || self.looked.contains_key(agent) {
            return false;
        }
        self.looked.insert(agent, Looked::Asking);
        true
    }

    /// What `agent`'s list said, asked alone.
    pub(crate) fn looked_one(&mut self, agent: &'static str, looked: Looked) {
        self.looked.insert(agent, looked);
    }

    /// The panes a press is running in.
    pub(crate) fn running_panes(&self) -> Vec<PaneId> {
        self.runs.keys().copied().collect()
    }

    /// Whether any press is running, or failed: what the page asks the lists
    /// about again whenever it is shown.
    pub(crate) fn has_unsettled_outcome(&self) -> bool {
        self.outcomes
            .values()
            .any(|outcome| matches!(outcome, Outcome::Running { .. } | Outcome::Failed { .. }))
    }

    /// What `agent`'s list said, if it has been asked.
    pub(crate) fn looked_at(&self, agent: &str) -> Option<&Looked> {
        self.looked.get(agent)
    }

    /// Whether Claude Code's files say it updates the plugin by itself.
    pub(crate) fn claude_updates(&self) -> bool {
        self.claude_updates
    }

    /// How the last press for `agent` went.
    pub(crate) fn outcome(&self, agent: &str) -> Option<&Outcome> {
        self.outcomes.get(agent)
    }

    /// Whether a press for `agent` is running in a tab now.
    pub(crate) fn is_running(&self, agent: &str) -> bool {
        self.runs.values().any(|run| run.agent == agent)
    }

    /// A press for `agent` opened `tab`, whose pane is `pane`.
    pub(crate) fn ran(
        &mut self,
        agent: &'static str,
        step: Step,
        tab: TabId,
        pane: PaneId,
        title: String,
    ) {
        self.runs.insert(
            pane,
            Run {
                agent,
                step,
                tab,
                sent: false,
            },
        );
        self.outcomes
            .insert(agent, Outcome::Running { step, title });
        self.hinted.insert(agent);
        if self.hint.as_ref().is_some_and(|hint| hint.agent == agent) {
            self.hint = None;
        }
    }

    /// The pane's first line was sent as it was typed.
    pub(crate) fn sent(&mut self, pane: PaneId) {
        if let Some(run) = self.runs.get_mut(&pane) {
            run.sent = true;
        }
    }

    /// A command ran to its end in `pane`, with `exit`: the agent whose press
    /// it was, when it was the line the press typed.
    pub(crate) fn finished(&mut self, pane: PaneId, exit: Option<i32>) -> Option<&'static str> {
        self.at_work.remove(&pane);
        let run = self.runs.remove(&pane)?;
        let title = match self.outcomes.get(run.agent) {
            Some(Outcome::Running { title, .. }) => title.clone(),
            _ => crate::agent::connect::tab_title(run.agent, run.step),
        };
        if !run.sent {
            // Something before the line — a startup file's command — and
            // the line still waits for its prompt.
            self.runs.insert(pane, run);
            return None;
        }
        let outcome = match exit {
            Some(0) => Outcome::Done(run.step),
            exit => Outcome::Failed {
                step: run.step,
                title,
                exit,
                pane,
            },
        };
        self.outcomes.insert(run.agent, outcome);
        Some(run.agent)
    }

    /// The pane's first line was changed before its prompt and never sent:
    /// what runs there now is the person's, and the press is let go.
    pub(crate) fn abandoned(&mut self, pane: PaneId) {
        if let Some(run) = self.runs.remove(&pane) {
            self.outcomes.remove(run.agent);
        }
    }

    /// The tab a press is running in for `agent`, if one is.
    pub(crate) fn tab_of(&self, agent: &str) -> Option<TabId> {
        self.runs
            .values()
            .find(|run| run.agent == agent)
            .map(|run| run.tab)
    }

    /// A hook reported from `pane`: its agent is connected, and no hint is
    /// offered over it. Whether that took a hint down.
    pub(crate) fn heard(&mut self, pane: PaneId) -> bool {
        self.heard.insert(pane);
        self.at_work.remove(&pane);
        let over = self.hint.as_ref().is_some_and(|hint| hint.pane == pane);
        if over {
            self.hint = None;
        }
        over
    }

    /// `pane`'s title says `agent` is at work, now: whether the hint is due
    /// over it, which [`Self::offer`] then puts up if nothing else stands in
    /// its way.
    pub(crate) fn at_work(&mut self, pane: PaneId, agent: &'static str, now: Instant) -> bool {
        if self.held
            || self.hint.is_some()
            || self.heard.contains(&pane)
            || self.hinted.contains(agent)
        {
            return false;
        }
        let (was, since) = *self.at_work.entry(pane).or_insert((agent, now));
        if was != agent {
            self.at_work.insert(pane, (agent, now));
            return false;
        }
        now.duration_since(since) >= self.hint_after()
    }

    /// [`HINT_AFTER`], or a test's own.
    fn hint_after(&self) -> Duration {
        #[cfg(test)]
        if let Some(after) = self.hint_after {
            return after;
        }
        HINT_AFTER
    }

    /// Makes the hint due after `after` in place of [`HINT_AFTER`]. A test's.
    #[cfg(test)]
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn set_hint_after(&mut self, after: Duration) {
        self.hint_after = Some(after);
    }

    /// Whether the hint may still go up for `agent` over `pane`, now that
    /// its list has answered: nothing has been offered for it, no hint is
    /// up, the pane's hooks have not spoken since, and no press is running.
    pub(crate) fn may_still_offer(&self, agent: &str, pane: PaneId) -> bool {
        !self.held
            && self.hint.is_none()
            && !self.hinted.contains(agent)
            && !self.heard.contains(&pane)
            && self.located(agent).is_some()
            && !self.is_running(agent)
    }

    /// What the hint says for `agent`, by what its list said: Connect where
    /// the plugin is missing; why the pane says nothing where it is there,
    /// for Claude Code and Codex; nothing otherwise, nor before the list has
    /// answered.
    pub(crate) fn hint_for(&self, agent: &str) -> Option<HintKind> {
        match self.looked.get(agent)? {
            Looked::Said(Standing::Missing) => Some(HintKind::Connect),
            Looked::Said(Standing::Connected { .. } | Standing::Behind { .. }) => match agent {
                "claude" => Some(HintKind::Why(CLAUDE_RESTART)),
                "codex" => Some(HintKind::Why(CODEX_TRUST)),
                _ => None,
            },
            _ => None,
        }
    }

    /// Puts the hint up for `agent`, over `pane`.
    pub(crate) fn offer(&mut self, agent: &'static str, pane: PaneId, kind: HintKind) {
        self.hinted.insert(agent);
        self.hint = Some(Hint {
            agent,
            pane,
            kind,
            connect: MouseStateHandle::default(),
            not_now: MouseStateHandle::default(),
        });
    }

    /// The hint, while it is up.
    pub(crate) fn hint(&self) -> Option<&Hint> {
        self.hint.as_ref()
    }

    /// Takes the hint down, if it is up for `agent`; whether it was.
    pub(crate) fn withdraw(&mut self, agent: &str) -> bool {
        let up = self.hint.as_ref().is_some_and(|hint| hint.agent == agent);
        if up {
            self.hint = None;
        }
        up
    }

    /// Forgets the panes that have closed.
    pub(crate) fn keep_only(&mut self, open: &[PaneId]) {
        self.runs.retain(|pane, _| open.contains(pane));
        self.heard.retain(|pane| open.contains(pane));
        self.at_work.retain(|pane, _| open.contains(pane));
        if self
            .hint
            .as_ref()
            .is_some_and(|hint| !open.contains(&hint.pane))
        {
            self.hint = None;
        }
        // A press whose tab closed before its line finished came to nothing
        // anybody saw.
        // And a failure whose tab closed has nothing left to say why.
        let running: HashSet<&str> = self.runs.values().map(|run| run.agent).collect();
        self.outcomes.retain(|agent, outcome| match outcome {
            Outcome::Running { .. } => running.contains(agent),
            Outcome::Failed { pane, .. } => open.contains(pane),
            Outcome::Done(_) => true,
        });
    }

    /// Claude Code's configuration directory, which is read for whether it
    /// updates the plugin by itself: the person's, or in a test only the
    /// test's own.
    pub(crate) fn claude_config(&self) -> Option<std::path::PathBuf> {
        #[cfg(test)]
        return self.claude_config.clone();
        #[cfg(not(test))]
        crate::agent::connect::claude_config()
    }

    /// See [`Self::claude_config`].
    #[cfg(test)]
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn set_claude_config(&mut self, config: Option<std::path::PathBuf>) {
        self.claude_config = config;
    }

    /// Asks nothing of the agents from here on.
    pub(crate) fn hold(&mut self) {
        self.held = true;
    }

    /// Writes the state a fixture says, and asks nothing from here on.
    pub(crate) fn fix(
        &mut self,
        found: Vec<Located>,
        said: Vec<(&'static str, Looked)>,
        claude_updates: bool,
        hint: Option<(&'static str, PaneId)>,
    ) {
        self.looked(found, said, claude_updates, &[]);
        if let Some((agent, pane)) = hint
            && let Some(kind) = self.hint_for(agent)
        {
            self.offer(agent, pane, kind);
        }
        self.held = true;
    }
}

/// What a picture says of the agents: which are installed, what each one's
/// list says, and whether the hint is up — read from a JSON file by
/// `--agents-fixture`, so a picture of the Agents page and of the hint is the
/// same on every machine and asks no agent anything.
///
/// ```json
/// { "agents": { "claude": "behind 6b2a55f742d2", "codex": "missing" },
///   "claude_updates": false, "hint": "claude" }
/// ```
///
/// Each agent says `missing`, `connected`, `connected <version>`,
/// `behind <version>`, `off`, `asking` or `unanswered <why>`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentsFixture {
    #[serde(default)]
    agents: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    claude_updates: bool,
    #[serde(default)]
    hint: Option<String>,
}

impl AgentsFixture {
    /// Reads the fixture at `path`, or says why it is not one.
    pub fn read(path: &std::path::Path) -> anyhow::Result<Self> {
        use anyhow::Context as _;
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let fixture: Self = serde_json::from_str(&text)
            .with_context(|| format!("{} is not a fixture of agents", path.display()))?;
        for (agent, said) in &fixture.agents {
            if !crate::agent::connect::is_connectable(agent) {
                anyhow::bail!("{}: Crook has no plugin for {agent:?}", path.display());
            }
            if Self::looked(said).is_none() {
                anyhow::bail!("{}: {said:?} is not what a list says", path.display());
            }
        }
        if let Some(agent) = &fixture.hint
            && !fixture.agents.contains_key(agent)
        {
            anyhow::bail!(
                "{}: the hint is for {agent:?}, which is not installed",
                path.display()
            );
        }
        Ok(fixture)
    }

    /// What one agent's word says.
    fn looked(said: &str) -> Option<Looked> {
        let (word, rest) = said.split_once(' ').unwrap_or((said, ""));
        let rest = (!rest.is_empty()).then(|| rest.to_owned());
        Some(match (word, rest) {
            ("missing", None) => Looked::Said(Standing::Missing),
            ("connected", version) => Looked::Said(Standing::Connected { version }),
            ("behind", Some(version)) => Looked::Said(Standing::Behind { version }),
            ("off", None) => Looked::Said(Standing::Off),
            ("asking", None) => Looked::Asking,
            ("unanswered", Some(why)) => Looked::Unanswered(why),
            _ => return None,
        })
    }

    /// Writes the fixture into `workspace`, the hint over its focused pane.
    pub fn seed(
        &self,
        workspace: &mut super::view::Workspace,
        ctx: &mut ViewContext<super::view::Workspace>,
    ) {
        let found: Vec<Located> = crate::agent::connect::connectable()
            .filter(|agent| self.agents.contains_key(*agent))
            .map(|agent| Located {
                agent,
                file: agent.into(),
                typed: agent.to_owned(),
            })
            .collect();
        let said = found
            .iter()
            .filter_map(|located| {
                let looked = Self::looked(self.agents.get(located.agent)?)?;
                Some((located.agent, looked))
            })
            .collect();
        let hint = self
            .hint
            .as_deref()
            .and_then(|hint| crate::agent::connect::connectable().find(|agent| *agent == hint));
        let updates = self.claude_updates;
        workspace.fix_connections(
            move |connections, pane| {
                connections.fix(found, said, updates, hint.zip(pane));
            },
            ctx,
        );
    }
}

/// The hint, under the header, while it may be drawn: what connecting the
/// agent gives, and Connect and Not now. See
/// [`Workspace::connect_hint`](super::view::Workspace::connect_hint).
pub(super) fn render(workspace: &super::view::Workspace) -> Option<Box<dyn Element>> {
    use super::settings_page::widgets;
    use crate::plugins::agents::action;

    let hint = workspace.connect_hint()?;
    let program = crate::agent::program(hint.agent)?;
    let ui = workspace.fonts().ui;
    Some(match hint.kind {
        HintKind::Connect => {
            let connect = workspace.run_about(&action("connect"), hint.agent)?;
            let not_now = workspace.run_about(&action("not-now"), hint.agent)?;
            super::crash_note::line(
                hint_text(hint.agent, program),
                [
                    widgets::text_button("Connect", Some(connect), hint.connect.clone(), ui),
                    widgets::text_button("Not now", Some(not_now), hint.not_now.clone(), ui),
                ],
                ui,
            )
        }
        HintKind::Why(said) => {
            let dismiss = workspace.run_about(&action("dismiss"), hint.agent)?;
            super::crash_note::line(
                said,
                [widgets::text_button(
                    "Dismiss",
                    Some(dismiss),
                    hint.not_now.clone(),
                    ui,
                )],
                ui,
            )
        }
    })
}

/// What the hint says for `agent`, which calls itself `program`.
pub(crate) fn hint_text(agent: &str, program: &str) -> String {
    format!(
        "Connect {program} to Crook \u{2014} {}",
        crate::agent::connect::gives(agent)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> Located {
        Located {
            agent: "claude",
            file: "/agents/claude".into(),
            typed: "claude".into(),
        }
    }

    #[test]
    fn the_hint_is_due_only_after_a_while_at_work_with_no_hook_heard() {
        let mut connections = Connections::default();
        let pane = PaneId::next();
        let start = Instant::now();
        assert!(!connections.at_work(pane, "claude", start));
        assert!(!connections.at_work(pane, "claude", start + HINT_AFTER / 2));
        assert!(connections.at_work(pane, "claude", start + HINT_AFTER));

        // A hook heard is the plugin, and the pane is never hinted over.
        connections.heard(pane);
        assert!(!connections.at_work(pane, "claude", start + HINT_AFTER * 3));
    }

    #[test]
    fn what_the_hint_offers_is_what_the_agents_list_said() {
        let mut connections = Connections::default();
        let pane = PaneId::next();
        connections.found_them(vec![claude()]);
        assert_eq!(
            connections.hint_for("claude"),
            None,
            "before the list answered"
        );
        assert!(connections.may_still_offer("claude", pane));
        assert!(
            !connections.may_still_offer("codex", pane),
            "not installed here"
        );

        connections.looked_one("claude", Looked::Said(Standing::Missing));
        assert_eq!(connections.hint_for("claude"), Some(HintKind::Connect));
        connections.looked_one(
            "claude",
            Looked::Said(Standing::Connected { version: None }),
        );
        assert_eq!(
            connections.hint_for("claude"),
            Some(HintKind::Why(CLAUDE_RESTART))
        );
        connections.looked_one("codex", Looked::Said(Standing::Connected { version: None }));
        assert_eq!(
            connections.hint_for("codex"),
            Some(HintKind::Why(CODEX_TRUST))
        );
        connections.looked_one(
            "copilot",
            Looked::Said(Standing::Connected { version: None }),
        );
        assert_eq!(connections.hint_for("copilot"), None);
        connections.looked_one("claude", Looked::Unanswered("no".into()));
        assert_eq!(connections.hint_for("claude"), None);

        connections.offer("claude", pane, HintKind::Connect);
        assert!(connections.withdraw("claude"));
        assert!(
            !connections.may_still_offer("claude", PaneId::next()),
            "offered again for the same agent"
        );
    }

    #[test]
    fn a_failure_clears_once_the_list_says_the_plugin_is_there_or_its_tab_closes() {
        let failed = |connections: &mut Connections, pane| {
            connections.ran(
                "claude",
                Step::Connect,
                TabId::next(),
                pane,
                "Connect".into(),
            );
            connections.sent(pane);
            connections.finished(pane, Some(1));
            assert!(matches!(
                connections.outcome("claude"),
                Some(Outcome::Failed { .. })
            ));
        };
        let mut connections = Connections::default();
        let pane = PaneId::next();
        failed(&mut connections, pane);
        // A list that says it is still missing leaves the failure standing.
        connections.looked(
            vec![claude()],
            vec![("claude", Looked::Said(Standing::Missing))],
            false,
            &[],
        );
        assert!(connections.outcome("claude").is_some());
        connections.looked(
            vec![claude()],
            vec![(
                "claude",
                Looked::Said(Standing::Connected { version: None }),
            )],
            false,
            &[],
        );
        assert_eq!(connections.outcome("claude"), None);

        let pane = PaneId::next();
        failed(&mut connections, pane);
        connections.keep_only(&[pane]);
        assert!(connections.outcome("claude").is_some());
        connections.keep_only(&[]);
        assert_eq!(connections.outcome("claude"), None);
    }

    #[test]
    fn a_run_in_a_shell_with_no_marks_gives_way_to_the_list() {
        let mut connections = Connections::default();
        let pane = PaneId::next();
        connections.ran(
            "claude",
            Step::Connect,
            TabId::next(),
            pane,
            "Connect".into(),
        );
        // A marked shell's run goes on waiting for its end.
        connections.looked(vec![claude()], Vec::new(), false, &[]);
        assert!(connections.is_running("claude"));
        connections.looked(vec![claude()], Vec::new(), false, &[pane]);
        assert!(!connections.is_running("claude"));
        assert_eq!(connections.outcome("claude"), None);
    }

    #[test]
    fn a_run_reports_its_line_only_once_it_was_sent_as_typed() {
        let mut connections = Connections::default();
        let pane = PaneId::next();
        let tab = TabId::next();
        connections.ran(
            "claude",
            Step::Connect,
            tab,
            pane,
            "Connect Claude Code".into(),
        );
        assert!(connections.is_running("claude"));
        // Something ran before the line was sent: the press still waits.
        assert_eq!(connections.finished(pane, Some(0)), None);
        assert!(connections.is_running("claude"));
        // The person changed the line, which was then never sent.
        connections.abandoned(pane);
        assert_eq!(connections.outcome("claude"), None);
        assert!(!connections.is_running("claude"));

        connections.ran(
            "claude",
            Step::Connect,
            tab,
            pane,
            "Connect Claude Code".into(),
        );
        connections.sent(pane);
        assert_eq!(connections.finished(pane, Some(1)), Some("claude"));
        assert_eq!(
            connections.outcome("claude"),
            Some(&Outcome::Failed {
                step: Step::Connect,
                title: "Connect Claude Code".into(),
                exit: Some(1),
                pane,
            })
        );
        assert!(!connections.is_running("claude"));
    }
}
