//! The Shell page: which shell a pane starts, and how.
//!
//! Two facts and one switch. The facts say which program a pane runs and
//! whether it gets the command marks — the two things a person comes here to
//! check when the blocks are not there. The switch decides which of the
//! person's own files the shell reads. `shell_integration::login_by_default`
//! is where that default is argued; this is where it is overridden.

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::shell_integration::{Marks, OPT_OUT_VARIABLE, Standing};
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

/// Which shell a pane starts, whether it gets the marks, and how it is
/// started — which decides which of the person's own files it reads.
///
/// The two facts are read the way a launch reads them (`Standing`), not
/// guessed beside it: the page that said "zsh" while the pane ran `/bin/sh`
/// would be worse than no page. They are here because a person whose output
/// is one long block has nowhere else to find out why — `$SHELL` naming a
/// shell Crook has no marks for, or the opt-out variable set in a profile
/// they forgot — and the description under the marks says where the marks
/// go on a machine Crook cannot reach.
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
    let fonts = workspace.fonts();
    let state = workspace.settings_page();
    let standing = Standing::current();

    let program = widgets::fact(
        Words::new("Shell")
            .with_description(if cfg!(windows) {
                "What %ComSpec% names, or PowerShell when it is unset."
            } else {
                "What $SHELL names, or the account's own shell when it is unset."
            })
            .with_keywords(&[
                "shell", "program", "path", "which", "zsh", "bash", "fish", "sh", "default",
            ]),
        standing.program.display().to_string(),
        true,
        fonts,
    );

    let marks = widgets::fact(
        Words::new("Command marks")
            .with_description(
                "What turns a pane's output into blocks, one per command. Crook installs them \
                 for zsh, bash and fish when it starts the shell. A shell it did not start — over \
                 ssh, in a container — needs them pasted into its own configuration, and \
                 crook --shell-integration <shell> prints them.",
            )
            .with_keywords(&[
                "marks",
                "blocks",
                "integration",
                "osc",
                "133",
                "prompt",
                "ssh",
                "container",
                "remote",
            ]),
        marks_line(&standing.marks),
        false,
        fonts,
    );

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
        widgets::category("What a pane runs", vec![program, marks]),
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

/// The marks fact's value: what a pane gets, and when it gets nothing, why.
///
/// The opt-out names its variable, because a variable set once in a profile
/// and forgotten is exactly the case the line is for; a shell with no
/// snippet names the shell, because "this shell" would send a person back to
/// the row above to find out which.
fn marks_line(marks: &Marks) -> String {
    match marks {
        Marks::Installed(shell) => {
            format!("Installed for {}", shell.name().unwrap_or("this shell"))
        }
        Marks::OptedOut => format!("Off — {OPT_OUT_VARIABLE} is set"),
        Marks::NoneFor(name) => format!("None — Crook has no marks for {name}"),
    }
}
