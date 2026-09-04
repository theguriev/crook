//! The Plugins page: what is loaded, what is not, and what went wrong.
//!
//! The page a person opens to find out what this build is made of. Until it
//! existed, "everything is a plugin" was a claim about the source rather than
//! something anybody could see, and a plugin that failed to load left one line
//! in a log nobody reads.
//!
//! # It is a plugin listing plugins, and that is not a joke
//!
//! It reads the same [`Host`] every other surface reads, so it cannot fall
//! behind: a plugin added to `defaults` is on this page without this file being
//! touched, and this page is on the rail because a plugin put it there. It
//! lists itself, which is the honest thing for it to do.
//!
//! # The switches
//!
//! One per plugin the binary carries, registered in [`Plugin::ready`] rather
//! than in `build`: a page that offers a switch per plugin cannot know how
//! many that is until they have all arrived, which is the whole reason there
//! is a second pass.
//!
//! Each is a named action, so a switch and a palette entry and a chord in
//! somebody's keymap are the same thing — `crook/plugins/toggle-crook-usage`
//! is bindable like anything else.
//!
//! Two of them are drawn inert, and [`HOLDS_THE_PAGE`] says which and why: a
//! switch that removes the switch is a one-way door, and the way back is
//! editing a JSON file. Nothing a stranger writes can be in that list, which
//! is correct — nothing else is load-bearing for this page.

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::plugin::{ActionName, BuildError, Host, Plugin};
use crate::workspace::settings_page::named;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category, Entry};
use crate::workspace::{Workspace, WorkspaceAction};

/// The two plugins whose switches are drawn inert.
///
/// `crook/settings` declares the slot this page is contributed to, and this
/// plugin is the page. Switching either off from here would take the switch
/// away with it, and the way back would be a JSON file — so the page says so
/// instead of offering a door that only opens one way.
const HOLDS_THE_PAGE: [&str; 2] = ["crook/settings", "crook/plugins"];

/// The plugin that lists the plugins.
pub struct Plugins;

impl Plugin for Plugins {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // Just before About, which is where a "what is this build" page
        // belongs: after everything that configures the application and before
        // the one that describes it.
        host.add_settings_page("page", "Plugins", 35, |workspace, _| page(workspace));
        Ok(())
    }

    fn ready(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // One action per plugin, and only now: `available` is not filled in
        // until every plugin has built.
        let carried: Vec<&'static Manifest> = host.available().to_vec();
        for manifest in carried {
            let plugin = manifest.id.clone();
            if HOLDS_THE_PAGE.contains(&plugin.as_str()) {
                continue;
            }
            host.register_command(
                toggle_action(&plugin),
                format!("Turn the {} plugin on or off", manifest.name),
                move |workspace, ctx| workspace.toggle_plugin(&plugin, ctx),
            );
        }
        Ok(())
    }
}

/// What switching one plugin on or off is called.
///
/// `owner/name` has a slash in it and an action name has exactly three parts,
/// so the plugin's own separator becomes a dash: `crook/usage` is reached by
/// `crook/plugins/toggle-crook-usage`.
fn toggle_action(plugin: &crate::plugin::PluginId) -> ActionName {
    ActionName::parse(&format!(
        "crook/plugins/toggle-{}-{}",
        plugin.owner(),
        plugin.name()
    ))
    .expect("a name built from two names that already parsed")
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/plugins").expect("a literal that parses"),
        name: "Plugins",
        description: "What this build is made of, and what did not load.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
    })
}

/// The page.
fn page(workspace: &Workspace) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let fonts = workspace.fonts();
    let host = workspace.host();
    let state = workspace.settings_page();

    let mut categories = vec![widgets::category(
        "Loaded",
        std::iter::once(widgets::note(
            "Everything Crook does arrives through one of these. What is compiled in is \
             Crook itself — nothing here is carried and left switched off — so this list is \
             the whole of what this build can do.",
            ui,
        ))
        .chain(host.available().iter().map(|manifest| {
            let on = host.is_loaded(&manifest.id);
            let holds_the_page = HOLDS_THE_PAGE.contains(&manifest.id.as_str());
            // A row's description is one line and is clipped at the box, so
            // the two that cannot be switched off say *that* instead of what
            // they do — which is the thing a person needs to read here.
            let description = if holds_the_page {
                "This is what draws the page you are on."
            } else {
                manifest.description
            };

            widgets::row(
                Words::new(manifest.name)
                    .with_description(description)
                    .with_keywords(&[
                        "plugin",
                        "extension",
                        "enable",
                        "disable",
                        manifest.id.as_str(),
                        tier_word(manifest.tier),
                    ]),
                !holds_the_page,
                widgets::switch(
                    on,
                    command(host, &manifest.id).filter(|_| !holds_the_page),
                    state.control(named(&format!("plugin.{}", manifest.id))),
                ),
                ui,
            )
        }))
        .collect::<Vec<Entry>>(),
    )];

    // Both of the following are ordinarily empty, and a category that is
    // ordinarily empty must not be drawn empty: a heading with nothing under
    // it reads as something missing rather than as nothing wrong.
    let refused: Vec<Entry> = host
        .refused()
        .iter()
        .map(|(id, problem)| {
            widgets::fact(
                Words::new(id.to_string())
                    .with_description("This plugin declined to load, so its feature is missing.")
                    .with_keywords(&["plugin", "failed", "error", "broken", "missing"]),
                problem.clone(),
                false,
                fonts,
            )
        })
        .collect();
    if !refused.is_empty() {
        categories.push(widgets::category("Did not load", refused));
    }

    let complaints: Vec<Entry> = host
        .audit()
        .iter()
        .map(|complaint| {
            widgets::found_by(
                Words::new("A slot has more in it than it can draw")
                    .with_keywords(&["plugin", "problem", "slot", "conflict", "audit"]),
                &complaint.to_string(),
                ui,
            )
        })
        .collect();
    if !complaints.is_empty() {
        categories.push(widgets::category("Problems", complaints));
    }

    categories
}

/// What one plugin's switch dispatches, or `None` if nothing answers.
fn command(host: &Host, plugin: &crate::plugin::PluginId) -> Option<WorkspaceAction> {
    host.action(&toggle_action(plugin))
        .map(WorkspaceAction::Run)
}

/// The word a person would search for to find a tier.
fn tier_word(tier: Tier) -> &'static str {
    match tier {
        Tier::Native => "built-in",
        Tier::Wasm => "sandboxed",
        Tier::Process => "external",
    }
}
