//! The Notifications page: which of a pane's stops reach the desktop while the
//! window is behind another one.
//!
//! Three switches and a note. The switches are the occasions `crate::notify`
//! knows — a pane that needs you, on out of the box; an agent that failed and
//! a long command that ended, off — and the note is what a person needs to
//! trust them: when a notification is posted, how often, what it says, and
//! that it never leaves the machine.
//!
//! The feature is this plugin's as well as the page: the workspace posts only
//! while it is loaded (see [`is_on`]), so the Plugins page's switch for it
//! turns notifications off rather than only hiding their settings.

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::notify::{LONG_COMMAND, Occasion, posts_here};
use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::settings_page::keyed;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category};
use crate::workspace::{SettingsAction, Workspace};

/// The plugin that owns the Notifications page.
pub struct Notifications;

impl Plugin for Notifications {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Bell)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // Between the Shell page and the Keyboard Shortcuts: after the pages
        // about what the window looks like and what it runs, before the ones
        // about driving it.
        host.add_settings_page("page", "Notifications", 20, notifications);
        Ok(())
    }
}

/// Whether desktop notifications are on at all: this plugin is loaded.
///
/// What the workspace asks before it posts anything, whatever the switches
/// say — a person who turned the plugin off on the Plugins page meant the
/// feature, not the page it is configured on.
pub fn is_on(host: &Host) -> bool {
    host.is_loaded(&manifest().id)
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/notifications").expect("a literal that parses"),
        name: "Notifications",
        description: "Tells the desktop when a pane needs you while Crook is behind another window.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

/// The page: a switch per occasion, and what they do.
///
/// On a platform this build does not post on, the switches are drawn without
/// a handler — a control with nothing to do is inert, not live and ignored —
/// and the note says why, so that a person on macOS looking for why nothing
/// arrived finds the answer where they would look for the setting.
fn notifications(workspace: &Workspace, _: &AppContext) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();
    let general = workspace.general();
    let live = posts_here();

    let switch = |occasion: Occasion, words: Words| {
        widgets::row(
            words,
            live,
            widgets::switch(
                general.notifies_on(occasion),
                live.then(|| SettingsAction::ToggleNotification(occasion).into()),
                state.control(keyed("notify", occasion)),
            ),
            ui,
        )
    };

    let needs_input = switch(
        Occasion::NeedsInput,
        Words::new("When a pane needs you")
            .with_description(
                "An agent stops, to ask or because it is done, or a program rings the bell, \
                 while Crook is behind another window.",
            )
            .with_keywords(&[
                "notification",
                "notify",
                "desktop",
                "banner",
                "toast",
                "alert",
                "attention",
                "waiting",
                "needs input",
                "question",
                "approval",
                "done",
                "finished",
                "agent",
                "bell",
                "away",
            ]),
    );
    let failed = switch(
        Occasion::Failed,
        Words::new("When an agent fails")
            .with_description("An agent says it stopped because something went wrong.")
            .with_keywords(&[
                "notification",
                "notify",
                "desktop",
                "failed",
                "failure",
                "error",
                "agent",
            ]),
    );
    let long_command = switch(
        Occasion::LongCommand,
        Words::new("When a long command finishes")
            .with_description(format!(
                "A command that ran for {} seconds or more comes to an end.",
                LONG_COMMAND.as_secs()
            ))
            .with_keywords(&[
                "notification",
                "notify",
                "desktop",
                "command",
                "finished",
                "done",
                "build",
                "long",
                "slow",
            ]),
    );

    let note = if live {
        widgets::note(
            "Only while the Crook window is behind another one: a pane in front of you \
             already has your attention. At most one for each pane every half minute, \
             however often its agent stops. The title names Crook and the tab, and the text \
             is the agent's question when it asked one. It goes to your desktop's notification \
             service through notify-send, on this machine and nowhere else; with no \
             notify-send installed, nothing is shown.",
            ui,
        )
    } else {
        widgets::note(
            "Crook posts desktop notifications on Linux only for now. Here the window's \
             title counts the panes waiting for you, and the dock or the taskbar points at \
             the window when one more starts.",
            ui,
        )
    };

    vec![
        widgets::category("Notify me", vec![needs_input, failed, long_command]),
        widgets::category("How it works", vec![note]),
    ]
}
