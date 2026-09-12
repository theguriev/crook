//! The About page: what this build is, and where it keeps its files.
//!
//! Last in the rail, because that is where every settings window ever written
//! puts it.

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};
use crook_plugin_api::ABI_VERSION;

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category};

/// The plugin that owns the About page.
pub struct About;

impl Plugin for About {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn mark(&self) -> Option<Lucide> {
        Some(Lucide::Info)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.add_settings_page("page", "About", 40, |workspace, _| about(workspace));
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

/// What this build is, where it keeps its file, and who owns what in it.
fn about(workspace: &Workspace) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let fonts = workspace.fonts();

    let file = match workspace.settings().path() {
        Some(path) => path.display().to_string(),
        // Either a run with ephemeral settings — a snapshot, a test — or a
        // machine with no configuration directory at all. The options still
        // work in both cases; they just do not outlive the process, and that
        // is worth saying in the one place somebody would come looking.
        None => "nowhere — this run keeps its options in memory".to_owned(),
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
                             against. One built for a later number is refused when it is \
                             opened, and the sentence names both numbers."
                        ))
                        .with_keywords(&["abi", "plugin", "api", "wasm", "sdk", "crate"]),
                    format!("crook_plugin_api = \"0.{ABI_VERSION}\""),
                    true,
                    fonts,
                ),
                widgets::fact(
                    Words::new("Settings file").with_keywords(&[
                        "json",
                        "config",
                        "path",
                        "where",
                        "folder",
                        "directory",
                    ]),
                    file,
                    true,
                    fonts,
                ),
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
