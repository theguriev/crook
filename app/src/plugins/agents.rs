//! The Agents page: which coding agents this machine has, whether Crook's
//! plugin is connected to each, and the press that connects it or brings it
//! up to date — and the answers the hint under the header is pressed with.
//!
//! The commands are the agent's own and run where the person sees them: see
//! [`crate::agent::connect`] for what runs and [`crate::workspace`]'s
//! `connecting` for the tab it runs in. This plugin registers the presses as
//! answers, so no plugin is handed one however it was granted, and the
//! control socket and `crook mcp` have no verb that reaches them: connecting
//! an agent is the person's call.

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};

use crate::agent::connect::{self, Standing, Step};
use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category, Entry};
use crate::workspace::settings_page::{keyed, named};
use crate::workspace::{SettingsAction, Workspace, WorkspaceAction};

/// The page's key, `owner/entry`, by which the window knows it is showing.
pub const PAGE_KEY: &str = "crook/agents/page";

/// The plugin that owns the Agents page.
pub struct Agents;

/// `crook/agents/<name>`.
pub fn action(name: &str) -> ActionName {
    ActionName::parse(&format!("crook/agents/{name}")).expect("a name built from a literal")
}

impl Plugin for Agents {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Command)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // Answers, every one: each runs an agent's commands, or answers the
        // window's own offer to, and a plugin granted `run:` for one could
        // otherwise install a plugin into the person's agent.
        host.register_answer(action("connect"), |workspace, ctx| {
            let agent = workspace.host().said();
            workspace.connect_agent(&agent, Step::Connect, ctx);
        });
        host.register_answer(action("update"), |workspace, ctx| {
            let agent = workspace.host().said();
            workspace.connect_agent(&agent, Step::Update, ctx);
        });
        host.register_answer(action("not-now"), |workspace, ctx| {
            let agent = workspace.host().said();
            workspace.not_now_connecting(&agent, ctx);
        });
        // The hint's own Dismiss, when it says why a connected agent reports
        // nothing: down for this window, and nothing kept.
        host.register_answer(action("dismiss"), |workspace, ctx| {
            let agent = workspace.host().said();
            workspace.dismiss_the_hint(&agent, ctx);
        });
        host.register_answer(action("check"), |workspace, ctx| {
            workspace.look_at_agents(true, ctx);
        });
        host.register_person_command(
            action("open"),
            "Connect coding agents to Crook",
            |workspace, ctx| {
                let page = workspace.host().settings_page_id(PAGE_KEY);
                workspace.open_settings_page(page, ctx);
            },
        );
        // After the Shell page and before Notifications: what runs in a pane,
        // then what a pane's agent tells it.
        host.add_settings_page("page", "Agents", 15, agents);
        Ok(())
    }
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/agents").expect("a literal that parses"),
        name: "Agents",
        description: "Connects Claude Code, Codex and Copilot CLI to Crook with their own \
                      commands, run in a tab you can see, and offers to once when one works \
                      with nothing connected.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

/// A press about `agent`, or `None` while the action is not registered.
fn about(workspace: &Workspace, name: &str, agent: &str) -> Option<WorkspaceAction> {
    workspace.run_about(&action(name), agent)
}

/// The page: a row for each agent found, the switch for the hint, and what
/// the rows run.
fn agents(workspace: &Workspace, app: &AppContext) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let connections = workspace.connections();

    let mut rows: Vec<Entry> = Vec::new();
    match connections.found() {
        None => rows.push(widgets::note(
            "Looking for the agents installed here\u{2026}",
            ui,
        )),
        Some([]) => rows.push(widgets::note(
            "None of the agents Crook has a plugin for \u{2014} Claude Code, Codex CLI and \
             GitHub Copilot CLI \u{2014} is installed where a pane\u{2019}s shell would find it.",
            ui,
        )),
        Some(found) => {
            for located in found {
                rows.push(agent_row(workspace, located, app));
                if located.agent == "claude" {
                    rows.extend(auto_update_row(workspace));
                }
            }
            rows.push(check_again_row(workspace));
        }
    }

    let general = workspace.general();
    let state = workspace.settings_page();
    let hints = widgets::row(
        Words::new("Offer to connect agents")
            .with_description(
                "When Claude Code, Codex or Copilot works in a pane and nothing it runs \
                 reports to Crook, a line under the header offers to connect it \u{2014} once, \
                 and never again after Not now.",
            )
            .with_keywords(&["hint", "nudge", "suggest", "offer", "plugin", "connect"]),
        true,
        widgets::switch(
            general.hint_to_connect_agents,
            Some(SettingsAction::ToggleConnectHints.into()),
            state.control(named("agents.hints")),
        ),
        ui,
    );

    vec![
        widgets::category("Connected agents", rows),
        widgets::category("Hints", vec![hints]),
        widgets::category(
            "How it works",
            vec![widgets::note(
                "Connect and Update open a tab and run the agent\u{2019}s own plugin commands \
                 in it, where you can read every word and what it printed; Crook edits none of \
                 the agent\u{2019}s files. Nothing is installed until you press one. This page \
                 asks each agent what it has with its own read-only `plugin list` when it is \
                 shown. To set the hooks up by hand instead, `crook --agent-hooks claude` \
                 \u{2014} or codex, or copilot \u{2014} prints them, and the same commands.",
                ui,
            )],
        ),
    ]
}

/// One agent's row: what its plugin is, and the press that changes that.
fn agent_row(workspace: &Workspace, located: &connect::Located, app: &AppContext) -> Entry {
    use crate::workspace::{Looked, Outcome};

    let ui = workspace.fonts().ui;
    let connections = workspace.connections();
    let agent = located.agent;
    let program = crate::agent::program(agent).unwrap_or(agent);
    let shell = workspace.shell_for_new_panes(app);
    let typed = connect::typed_as(located, shell.as_os_str());
    let line = |step| connect::line(agent, step, &typed, shell.as_os_str());

    let (said, press): (String, Option<(&str, &str)>) = match connections.outcome(agent) {
        // Its button shows the tab rather than running anything again: a
        // shell that marks no commands, cmd.exe or PowerShell, never says the
        // line ended, and the row would otherwise have nothing to press.
        Some(Outcome::Running { step, title }) => (
            format!(
                "Running in the \u{201c}{title}\u{201d} tab\u{2026} Open this page again \
                 once it is done to see what it came to."
            ),
            Some((
                "Show tab",
                match step {
                    Step::Connect => "connect",
                    Step::Update => "update",
                },
            )),
        ),
        Some(Outcome::Failed {
            step, title, exit, ..
        }) => {
            let ended = match exit {
                Some(code) => format!("ended with status {code}"),
                None => String::from("did not finish"),
            };
            let name = match step {
                Step::Connect => "connect",
                Step::Update => "update",
            };
            (
                format!("What ran in the \u{201c}{title}\u{201d} tab {ended}; the tab says why."),
                Some(("Try again", name)),
            )
        }
        _ => match connections.looked_at(agent) {
            None | Some(Looked::Asking) => (format!("Asking {program} what it has\u{2026}"), None),
            Some(Looked::Unanswered(why)) => (
                format!(
                    "Could not ask {program} what it has: {why}. Connect runs: {}",
                    line(Step::Connect).unwrap_or_default()
                ),
                Some(("Connect", "connect")),
            ),
            Some(Looked::Said(Standing::Missing)) => (
                format!(
                    "Not connected. Connect runs, in a tab of its own: {}",
                    line(Step::Connect).unwrap_or_default()
                ),
                Some(("Connect", "connect")),
            ),
            Some(Looked::Said(Standing::Connected { version })) => (
                match version
                    .as_deref()
                    .filter(|version| connect::is_a_version(version))
                {
                    Some(version) => format!(
                        "Connected: Crook\u{2019}s plugin {version}. Sessions started since it \
                         was installed report to Crook."
                    ),
                    // Codex names a copy from a checkout `local`, and
                    // Copilot names none: nothing to compare with this Crook.
                    None => format!(
                        "Connected. Sessions started since it was installed report to Crook. \
                         {program} does not say which version of the plugin it has, so Crook \
                         cannot tell whether it is behind: Update brings it up to date either \
                         way."
                    ),
                },
                Some(("Update", "update")),
            ),
            Some(Looked::Said(Standing::Behind { version })) => (
                format!(
                    "Update available: the plugin is {version}, and this is Crook {}. Update \
                     runs: {}",
                    env!("CARGO_PKG_VERSION"),
                    line(Step::Update).unwrap_or_default()
                ),
                Some(("Update", "update")),
            ),
            Some(Looked::Said(Standing::Off)) => (
                format!("Installed, and turned off in {program}."),
                Some(("Update", "update")),
            ),
        },
    };

    let command = press.and_then(|(_, name)| about(workspace, name, agent));
    let label = press.map_or("Connect", |(label, _)| label);
    widgets::row(
        Words::new(program)
            .with_description(said)
            .with_keywords(&["agent", "plugin", "connect", "install", "update", agent]),
        true,
        widgets::text_button(
            label,
            command,
            workspace
                .settings_page()
                .control(keyed("agents.press", agent)),
            ui,
        ),
        ui,
    )
}

/// Whether Claude Code keeps the plugin up to date by itself, and how to
/// have it do so: a row under Claude Code's, once its list says it has the
/// plugin.
///
/// Claude Code has no command that turns auto-update on for a marketplace,
/// and Crook does not write its settings, so this says how rather than
/// offering a button.
fn auto_update_row(workspace: &Workspace) -> Option<Entry> {
    use crate::workspace::Looked;

    let connections = workspace.connections();
    let installed = matches!(
        connections.looked_at("claude"),
        Some(Looked::Said(
            Standing::Connected { .. } | Standing::Behind { .. }
        ))
    );
    if !installed {
        return None;
    }
    let ui = workspace.fonts().ui;
    let said = match connections.claude_updates() {
        true => String::from(
            "Claude Code updates it by itself: auto-update is on for the crook marketplace.",
        ),
        false => format!(
            "Claude Code updates a marketplace like Crook\u{2019}s only once auto-update is \
             on for it, and has no command that turns it on. In Claude Code, /plugin \u{203a} \
             Marketplaces \u{203a} crook \u{203a} Enable auto-update does it; or add \
             \u{201c}autoUpdate\u{201d} to Crook\u{2019}s entry in Claude Code\u{2019}s \
             settings.json yourself, which Crook does not edit: {}",
            connect::CLAUDE_AUTO_UPDATE
        ),
    };
    Some(widgets::row(
        Words::new("Keep Claude Code\u{2019}s plugin up to date")
            .with_description(said)
            .with_keywords(&[
                "auto-update",
                "autoupdate",
                "update",
                "marketplace",
                "claude",
            ]),
        true,
        Empty::new().finish(),
        ui,
    ))
}

/// The press that asks every agent's list again, now: for a row whose tab
/// never said how it ended, an agent changed by hand since the page asked,
/// or an answer that did not come.
fn check_again_row(workspace: &Workspace) -> Entry {
    let ui = workspace.fonts().ui;
    widgets::row(
        Words::new("Ask the agents again")
            .with_description(
                "Runs each agent\u{2019}s own read-only plugin list once more. The page asks by \
                 itself when it is opened, and no more than once a minute.",
            )
            .with_keywords(&["check", "refresh", "again", "reload", "plugin", "list"]),
        true,
        widgets::text_button(
            "Check again",
            workspace.run_about(&action("check"), ""),
            workspace.settings_page().control(named("agents.check")),
            ui,
        ),
        ui,
    )
}
