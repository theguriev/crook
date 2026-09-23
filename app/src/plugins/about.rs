//! The About page: what this build is, whether there is a newer one, and
//! where it keeps its files.
//!
//! Last in the rail, because that is where every settings window ever written
//! puts it — and the page that says which version this is, which is the one
//! place a person looks when they wonder whether it is the current one.
//!
//! # The update it offers
//!
//! **Check for updates** is a press and nothing else asks: no timer, no check
//! at launch, no poll, which is the rule `store::fetch` states and the reason
//! the README can still say there is no telemetry. What the press starts is
//! [`UpdateModel::check`] on the pool; what it finds is drawn on the row it
//! was pressed from, and an **Update** button appears beside it when this copy
//! of Crook is one it may replace. Where it may not — a notarized bundle, a
//! package manager's binary, a build, a dev build — the row says which instead
//! of offering a button that would refuse.
//!
//! The `--check-update` and `--update` flags are the same two calls without a
//! window; see [`crate::update`].

use std::cell::RefCell;
use std::rc::Rc;

use crookui_core::prelude::*;

use crook_plugin::{ActionName, Manifest, PluginId, Tier};
use crook_plugin_api::ABI_VERSION;

use crate::plugin::{BuildError, Host, Plugin};
use crate::update::UpdateModel;
use crate::workspace::settings_page::named;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category, Entry};
use crate::workspace::{Workspace, WorkspaceAction};

/// The plugin that owns the About page.
#[derive(Default)]
pub struct About {
    /// The model the page reads and the two buttons drive.
    ///
    /// Shared the way the Store section shares its own: a settings page is an
    /// `Fn(&Workspace, &AppContext)` and an action is an
    /// `Fn(&mut Workspace, &mut ViewContext)`, so neither can hold the plugin
    /// and both need what it made.
    model: Rc<RefCell<Option<ModelHandle<UpdateModel>>>>,
}

/// `crook/about/<name>`.
fn action(name: &str) -> ActionName {
    ActionName::parse(&format!("crook/about/{name}")).expect("a name built from a literal")
}

impl Plugin for About {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Info)
    }

    fn build(
        &mut self,
        host: &mut Host,
        ctx: &mut ViewContext<Workspace>,
    ) -> Result<(), BuildError> {
        let model = ctx.add_model(|_| UpdateModel::new());
        *self.model.borrow_mut() = Some(model.clone());
        // Nothing is asked here. The model is made empty, and the page it is
        // drawn on says so until somebody presses the button.

        // A repaint when the answer lands, and the one fact the window keeps
        // outside this page: the sidebar marks the button that leads here when
        // a check has found a release this build is behind. Copied rather than
        // reached for, because the sidebar draws every frame and this changes
        // twice a month.
        ctx.observe(&model, |workspace, model, ctx| {
            let found = model.as_ref(ctx).newer().map(|found| found.version.clone());
            workspace.note_newer_release(found, ctx);
        });

        // Answers rather than actions: a sandboxed plugin granted
        // `run:crook/about/update` could otherwise replace the binary the
        // person is running. See `Host::register_answer`.
        let checking = self.model.clone();
        host.register_answer(action("check-updates"), move |_, ctx| {
            if let Some(model) = checking.borrow().clone() {
                model.update(ctx, |model, ctx| model.check(ctx));
            }
        });
        let installing = self.model.clone();
        host.register_answer(action("update"), move |workspace, ctx| {
            let channel = workspace.release_channel();
            if let Some(model) = installing.borrow().clone() {
                model.update(ctx, |model, ctx| model.install(channel, ctx));
            }
        });

        let drawn = self.model.clone();
        host.add_settings_page("page", "About", 40, move |workspace, app| {
            about(workspace, drawn.borrow().clone(), app)
        });
        Ok(())
    }
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/about").expect("a literal that parses"),
        name: "About",
        description: "The version, the channel, where the settings live and the licence.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

/// A refusal as a sentence that can follow another one.
///
/// The refusals are written to follow `crook: ` on a command line, where they
/// begin in lower case. Here they follow a full stop.
fn told(refusal: &crate::update::Refusal) -> String {
    let said = refusal.to_string();
    let mut characters = said.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => said,
    }
}

/// One of this page's own actions as something a button can dispatch, or
/// `None` when the action is not registered — which is what a control drawn
/// dead reads off.
fn run(workspace: &Workspace, name: &str) -> Option<WorkspaceAction> {
    workspace
        .host()
        .action(&action(name))
        .map(WorkspaceAction::Run)
}

/// The row that asks whether there is a newer Crook, and installs it.
///
/// One row rather than a category of its own: it is a fact about this build,
/// and it belongs next to the version it is about. What it says goes through
/// four states — never asked, asking, an answer, and what an install came to —
/// and the control beside it is whichever of the two presses the state allows,
/// or nothing at all while one is in flight.
fn update_row(
    workspace: &Workspace,
    model: Option<ModelHandle<UpdateModel>>,
    app: &AppContext,
) -> Entry {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();
    let model = model.map(|model| model.as_ref(app));

    let words = Words::new("Updates").with_keywords(&[
        "update", "upgrade", "newer", "release", "version", "download",
    ]);

    let Some(model) = model else {
        // Before the plugin has built its model, which is a frame nobody sees.
        return widgets::row(words, false, Empty::new().finish(), ui);
    };

    let running = crate::update::running();

    let (said, control) = if model.checking() {
        (String::from("Asking the releases page\u{2026}"), None)
    } else if model.installing() {
        (
            format!(
                "Installing {}\u{2026}",
                model.newer().map_or("", |found| found.version.as_str())
            ),
            None,
        )
    } else if let Some(version) = model.installed() {
        (
            format!("Updated to {version}. Restart Crook to run it."),
            None,
        )
    } else if let Some(why) = model.problem() {
        (
            why.to_owned(),
            run(workspace, "check-updates").map(|command| ("Try again", command)),
        )
    } else {
        match (model.newer(), model.found().is_some()) {
            // A newer release, and either the button that installs it or the
            // reason there is no button.
            (Some(found), _) => {
                // Whether this copy is one an update may be written over,
                // asked of the path rather than of the press: the sentence
                // goes under the label, where a person reads it before
                // pressing anything. Asked here and nowhere above, because the
                // asking writes a file beside the binary, and this page is
                // built on every keystroke of a settings search and of the
                // palette — not only while it is on screen.
                let refusal = crate::update::replaceable(workspace.release_channel()).err();
                (
                    match &refusal {
                        Some(refusal) => {
                            format!("crook {} is out. {}", found.version, told(refusal))
                        }
                        None => format!("crook {} is out, and this is {running}", found.version),
                    },
                    // The button is Update where this copy can take one, and
                    // the check again everywhere else: a row whose only
                    // control is dead is a row that looks broken rather than
                    // one that is telling you where your Crook came from.
                    match refusal.is_none() {
                        true => run(workspace, "update").map(|command| ("Update", command)),
                        false => {
                            run(workspace, "check-updates").map(|command| ("Check again", command))
                        }
                    },
                )
            }
            (None, true) => (
                format!("crook {running} is the newest release"),
                run(workspace, "check-updates").map(|command| ("Check again", command)),
            ),
            // Never asked, which is every window until somebody presses it.
            (None, false) => (
                String::from("Nothing is asked of the network until you press this."),
                run(workspace, "check-updates").map(|command| ("Check for updates", command)),
            ),
        }
    };

    let (label, command) = match control {
        Some((label, run)) => (label, Some(run)),
        None => ("Check for updates", None),
    };
    widgets::row(
        words.with_description(said),
        command.is_some(),
        widgets::text_button(label, command, state.control(named("check-updates")), ui),
        ui,
    )
}

/// What this build is, where it keeps its file, and who owns what in it.
fn about(
    workspace: &Workspace,
    model: Option<ModelHandle<UpdateModel>>,
    app: &AppContext,
) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let fonts = workspace.fonts();

    // Either a path, or the one word "nowhere" with the sentence under the
    // label rather than beside it: a value is cut from its start like a path
    // when it is drawn as one, and a sentence cut that way lost its first
    // word — the answer — in a window the width of a laptop's.
    let mut settings_file = Words::new("Settings file").with_keywords(&[
        "json",
        "config",
        "path",
        "where",
        "folder",
        "directory",
    ]);
    let (file, is_a_path) = match workspace.settings().path() {
        Some(path) => (path.display().to_string(), true),
        // Either a run with ephemeral settings — a snapshot, a test — or a
        // machine with no configuration directory at all. The options still
        // work in both cases; they just do not outlive the process, and that
        // is worth saying in the one place somebody would come looking.
        None => {
            settings_file = settings_file
                .with_description("This run keeps its options in memory; they do not outlive it.");
            ("nowhere".to_owned(), false)
        }
    };

    vec![
        widgets::category(
            "Build",
            vec![
                widgets::fact(
                    Words::new("Version").with_keywords(&["build", "release", "crook"]),
                    env!("CARGO_PKG_VERSION").to_owned(),
                    false,
                    fonts,
                ),
                widgets::fact(
                    Words::new("Channel").with_keywords(&["dev", "stable", "build"]),
                    workspace.channel().to_owned(),
                    false,
                    fonts,
                ),
                // The number a plugin author needs and the sentence they are
                // shown when they got it wrong — "built for plugin API 9, and
                // this is Crook 8" — had nowhere in the window that said what
                // this build speaks. The value is the line their Cargo.toml
                // wants, because the crate is versioned `0.<ABI>.<patch>` for
                // exactly that reason: see the README's "For plugin authors".
                widgets::fact(
                    Words::new("Plugin API")
                        .with_description(format!(
                            "ABI {ABI_VERSION}: the vocabulary a plugin from a file is built \
                             against. One built for any other number, later or earlier, is \
                             refused when it is opened, and the sentence names both \
                             numbers."
                        ))
                        .with_keywords(&["abi", "plugin", "api", "wasm", "sdk", "crate"]),
                    format!("crook_plugin_api = \"0.{ABI_VERSION}\""),
                    true,
                    fonts,
                ),
                widgets::fact(settings_file, file, is_a_path, fonts),
                update_row(workspace, model, app),
            ],
        ),
        widgets::category(
            "Licence",
            vec![
                widgets::note(
                    "Crook is open source under the MIT licence, in full and with no exceptions.",
                    ui,
                ),
                widgets::note(
                    "Its UI framework — the crookui and crookui_core crates — is derived from the \
                     warpui and warpui_core crates of Warp, which Denver Technologies publishes \
                     under the MIT licence. The rest of Warp is AGPL and none of it is here: what \
                     Crook took from those parts is architecture, read and rewritten, which is \
                     why this page can say MIT and mean it.",
                    ui,
                ),
                widgets::note(
                    "The bundled palettes named after Catppuccin, Everforest, Gruvbox, \
                     Kanagawa, Nord, Rosé Pine and Tokyo Night belong to those projects, each \
                     under its own licence. Their values were read from the theme files \
                     Omarchy ships, which is MIT, from Basecamp; Matte Black and Osaka Jade \
                     are Omarchy's own.",
                    ui,
                ),
            ],
        ),
    ]
}
