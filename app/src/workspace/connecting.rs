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
//! pane's shell finds it, and running in the pane by that name rather than
//! through `ssh` or `npx`. Never while a question is waiting, and never
//! again for an agent somebody said Not now to; the Agents page turns the
//! hints off altogether. The hint takes no key and moves no focus.

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
    /// Its line ended otherwise: the status, when the shell said one.
    Failed {
        step: Step,
        title: String,
        exit: Option<i32>,
    },
}

/// The hint, while it is up.
#[derive(Clone, Debug)]
pub(crate) struct Hint {
    /// The agent it offers to connect.
    pub(crate) agent: &'static str,
    /// The pane whose title it was offered over.
    pub(crate) pane: PaneId,
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
    ) {
        self.found = Some(found);
        self.finding = false;
        self.looking = false;
        self.looked.clear();
        self.looked.extend(said);
        self.claude_updates = claude_updates;
        // A connection that ended is told by the list from here on, which
        // says what it came to; a failure stays, since the list cannot say
        // why.
        self.outcomes
            .retain(|_, outcome| !matches!(outcome, Outcome::Done(_)));
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
    pub(crate) fn set_hint_after(&mut self, after: Duration) {
        self.hint_after = Some(after);
    }

    /// Whether the hint may be offered for `agent`, by what the window knows
    /// of it: installed here, and not known to be connected.
    pub(crate) fn may_offer(&self, agent: &str) -> bool {
        if self.located(agent).is_none() {
            return false;
        }
        !matches!(
            self.looked.get(agent),
            Some(Looked::Said(
                Standing::Connected { .. } | Standing::Behind { .. } | Standing::Off
            )) | Some(Looked::Asking)
        ) && !self.is_running(agent)
    }

    /// Puts the hint up for `agent`, over `pane`.
    pub(crate) fn offer(&mut self, agent: &'static str, pane: PaneId) {
        self.hinted.insert(agent);
        self.hint = Some(Hint {
            agent,
            pane,
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
        let running: HashSet<&str> = self.runs.values().map(|run| run.agent).collect();
        self.outcomes.retain(|agent, outcome| {
            !matches!(outcome, Outcome::Running { .. }) || running.contains(agent)
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
        self.looked(found, said, claude_updates);
        if let Some((agent, pane)) = hint {
            self.offer(agent, pane);
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
    let connect = workspace.run_about(&action("connect"), hint.agent)?;
    let not_now = workspace.run_about(&action("not-now"), hint.agent)?;
    let ui = workspace.fonts().ui;
    Some(super::crash_note::line(
        hint_text(hint.agent, program),
        [
            widgets::text_button("Connect", Some(connect), hint.connect.clone(), ui),
            widgets::text_button("Not now", Some(not_now), hint.not_now.clone(), ui),
        ],
        ui,
    ))
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
    fn the_hint_is_offered_once_an_agent_and_only_for_one_found_here() {
        let mut connections = Connections::default();
        assert!(!connections.may_offer("claude"), "nothing was found");
        connections.found_them(vec![claude()]);
        assert!(connections.may_offer("claude"));
        assert!(!connections.may_offer("codex"), "not installed here");

        let pane = PaneId::next();
        connections.offer("claude", pane);
        assert!(connections.withdraw("claude"));
        let other = PaneId::next();
        let start = Instant::now();
        connections.at_work(other, "claude", start);
        assert!(
            !connections.at_work(other, "claude", start + HINT_AFTER),
            "offered again for the same agent"
        );
    }

    #[test]
    fn an_agent_whose_list_says_it_has_the_plugin_is_not_hinted() {
        let mut connections = Connections::default();
        connections.looked(
            vec![claude()],
            vec![(
                "claude",
                Looked::Said(Standing::Connected { version: None }),
            )],
            false,
        );
        assert!(!connections.may_offer("claude"));
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
                exit: Some(1)
            })
        );
        assert!(!connections.is_running("claude"));
    }
}
