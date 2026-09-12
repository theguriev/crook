//! The Shell page: how a pane's shell is started.
//!
//! One switch, and it decides which of the person's own files the shell reads.
//! `shell_integration::login_by_default` is where the default is argued; this
//! is where it is overridden.

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::settings_page::named;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category};
use crate::workspace::{SettingsAction, Workspace};

/// The plugin that owns the Shell page.
pub struct Shell;

impl Plugin for Shell {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Terminal)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.add_settings_page("page", "Shell", 10, |workspace, _| shell(workspace));
        Ok(())
    }
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/shell").expect("a literal that parses"),
        name: "Shell settings",
        description: "How a pane's shell is started, and which of your files it reads.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

/// How a pane's shell is started, which decides which of the person's own
/// files it reads.
///
/// One switch, and it is worth a page of its own rather than a row on another
/// because of what it changes: the answer to `which`, the version of every tool
/// on `PATH`, and whether the thing a person's `~/.zprofile` prints appears at
/// all. The note is not decoration — a switch called "login shell" means
/// nothing to most people, and the sentence under it is the setting.
///
/// Which way it starts is the desktop's answer rather than Crook's, so the note
/// names both directions rather than assuming the macOS one: see
/// `shell_integration::login_by_default`.
fn shell(workspace: &Workspace) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();

    let login = widgets::row(
        Words::new("Start a login shell")
            .with_description(
                "Read the files a login gives you — ~/.zprofile, ~/.bash_profile, /etc/profile — \
                 the way your desktop's own terminal does.",
            )
            .with_keywords(&[
                "login",
                "shell",
                "profile",
                "zprofile",
                "zlogin",
                "bash_profile",
                "path",
                "environment",
                "dotfiles",
                "startup",
                "zsh",
                "bash",
                "fish",
            ]),
        true,
        widgets::switch(
            workspace.general().login_shell,
            Some(SettingsAction::ToggleLoginShell.into()),
            state.control(named("login-shell")),
        ),
        ui,
    );

    vec![
        widgets::category("Startup", vec![login]),
        widgets::category(
            "What it changes",
            vec![widgets::note(
                "Your PATH is assembled by those files, so a shell that skips them finds \
                 different tools than the terminal beside it. It starts on where every macOS \
                 terminal starts a login shell, and off on Linux where GNOME Terminal and Konsole \
                 do not and your PATH is more likely to be in ~/.bashrc. Turn it off if a profile \
                 of yours expects to run once when you log in rather than once per pane; turn it \
                 on if ~/.profile is where your PATH lives. Either way it applies to the next \
                 shell opened, not the ones already running.",
                ui,
            )],
        ),
    ]
}
