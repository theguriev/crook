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
//! turns notifications off rather than only hiding their settings. With it
//! off, or every switch on the page off, Crook.app does not ask macOS whether
//! it may post either, not even for the dock's badge (see [`are_wanted`]).

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::notify::{LONG_COMMAND, Occasion, Service};
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

/// Whether the person wants any notification from `workspace` at all: this
/// plugin is loaded and one of its page's switches is on.
///
/// What the window asks before it asks macOS, for the dock's badge, for the
/// leave Crook.app posts with. macOS words that prompt as Crook wanting to
/// send notifications, and a person who turned them off here should not be
/// asked that — nor, having refused, find the notifications they turn on
/// later refused with it.
pub fn are_wanted(workspace: &Workspace) -> bool {
    is_on(workspace.host()) && workspace.general().notifies_on_any()
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
/// Where this build posts nothing, the switches are drawn without a handler —
/// a control with nothing to do is inert, not live and ignored — and the note
/// says why, so that a person on Windows, or running a Mac binary outside
/// Crook.app, who is looking for why nothing arrived finds the answer where
/// they would look for the setting.
fn notifications(workspace: &Workspace, _: &AppContext) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();
    let general = workspace.general();
    let service = Service::here();
    let live = service.posts();

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

    vec![
        widgets::category("Notify me", vec![needs_input, failed, long_command]),
        widgets::category(
            "How it works",
            vec![widgets::note(&how_it_works(service), ui)],
        ),
    ]
}

/// The page's note: when a notification is posted and where it goes through
/// `service` — or, where nothing is posted, what points at the window instead.
fn how_it_works(service: Service) -> String {
    const WHEN: &str = "Only while the Crook window is behind another one: a pane in front of \
                        you already has your attention. At most one for each pane every half \
                        minute, however often its agent stops, unless you have looked at the \
                        pane since. The title names Crook and the tab, and the text is the \
                        agent's question when it asked one.";
    match service {
        Service::NotifySend => format!(
            "{WHEN} It goes to your desktop's notification service through notify-send, on \
             this machine and nowhere else; with no notify-send installed, nothing is shown."
        ),
        Service::NotificationCenter => format!(
            "{WHEN} It goes to Notification Center, on this Mac and nowhere else. macOS asks \
             whether Crook may post the first time a pane waits for you or Crook has \
             something to post, and System Settings → Notifications → Crook is where that \
             answer changes. The same answer, and its Badges switch, decide whether the dock \
             icon shows the count of waiting panes. With every switch here off the question \
             is never put to you, and the dock may show no count."
        ),
        Service::OutsideTheApp => "This copy of Crook is not running from Crook.app, and \
             macOS posts notifications only for an app, so it posts none. The window's title \
             counts the panes waiting for you, and the dock icon bounces when one more starts."
            .to_owned(),
        Service::Nowhere => "Crook posts desktop notifications on Linux and macOS only for \
             now. Here the window's title counts the panes waiting for you, and the taskbar \
             points at the window when one more starts."
            .to_owned(),
    }
}
