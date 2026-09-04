//! The Plugins page: what this build is made of, one plugin at a time.
//!
//! The page a person opens to find out what Crook is. Until it existed,
//! "everything is a plugin" was a claim about the source rather than something
//! anybody could see, and a plugin that failed to load left one line in a log
//! nobody reads.
//!
//! # A list beside a card, which is VS Code's shape
//!
//! A column of every plugin the binary carries with a field above it — in the
//! sidebar, where the tab list otherwise is — and beside it whatever the list
//! has selected: the name, what it is, where it came from, what it puts on
//! screen, and the switch. That is the shape every extension manager has
//! settled on, and the reason is the same everywhere: a row can say a plugin's
//! name and whether it is on, and nothing else worth reading fits on one line.
//!
//! It is a *section of the sidebar* and not a page of the settings. Plugins
//! are not a setting — they are what the application is made of, and a person
//! looking for them is not looking for a preference.
//!
//! # It is a plugin listing plugins, and that is not a joke
//!
//! It reads the same [`Host`] every other surface reads, so it cannot fall
//! behind: a plugin added to `defaults` is on this page without this file
//! being touched, one installed into the plugins directory is on it too, and
//! this page is on the rail because a plugin put it there. It lists itself.
//!
//! # Everything a click does is a named action
//!
//! Selecting a row and flipping a switch are both actions —
//! `crook/plugins/show-crook-usage` and `crook/plugins/toggle-crook-usage` —
//! registered one per plugin in [`Plugin::ready`], because a page that offers
//! something per plugin cannot know how many that is until they have all
//! arrived. The switches are *commands*, so they are in the palette; the
//! selections are not, because a list of things to do should not be a list of
//! rows to look at.
//!
//! Two switches are drawn inert, and [`HOLDS_THE_PAGE`] says which and why: a
//! switch that removes the switch is a one-way door whose way back is editing
//! a JSON file.

mod card;
mod list;
mod state;

use std::rc::Rc;

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::plugin::{ActionName, BuildError, Host, Plugin};
use crate::workspace::Workspace;
use crate::workspace::section;

use state::PluginsState;

/// Where the card has been scrolled to.
const CARD_SCROLL: &str = "plugins.card";

/// The two plugins whose switches are drawn inert.
///
/// `crook/settings` declares the slot this page is contributed to, and this
/// plugin is the page. Switching either off from here would take the switch
/// away with it, and the way back would be a JSON file — so the page says so
/// instead of offering a door that only opens one way.
pub(super) const HOLDS_THE_PAGE: [&str; 2] = ["crook/settings", "crook/plugins"];

/// The plugin that lists the plugins.
pub struct Plugins {
    /// What the list has selected, and the field above it.
    ///
    /// Shared with the closures that draw the page and with the handlers that
    /// change it: a contribution is an `Fn` and a handler is an `Fn`, so
    /// neither can hold a `&mut` to this.
    state: Rc<PluginsState>,
}

impl Plugins {
    /// One, not showing anything yet.
    pub fn new() -> Self {
        Self {
            state: Rc::new(PluginsState::new()),
        }
    }
}

impl Default for Plugins {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for Plugins {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // Just before About, which is where a "what is this build" page
        // belongs: after everything that configures the application and before
        // the one that describes it.
        let state = self.state.clone();
        host.add_sidebar_section(
            "section",
            "Plugins",
            Lucide::Blocks,
            10,
            move |workspace, _| page(workspace, &state),
        );
        Ok(())
    }

    fn ready(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // One pair of actions per plugin, and only now: `available` is not
        // filled in until every plugin has built.
        let carried: Vec<&'static Manifest> = host.available().to_vec();
        for manifest in carried {
            let plugin = manifest.id.clone();

            let state = self.state.clone();
            let chosen = plugin.clone();
            host.register_action(action("show", &plugin), move |_, ctx| {
                state.select(&chosen);
                ctx.notify();
            });

            if HOLDS_THE_PAGE.contains(&plugin.as_str()) {
                continue;
            }
            host.register_command(
                action("toggle", &plugin),
                format!("Turn the {} plugin on or off", manifest.name),
                move |workspace, ctx| workspace.toggle_plugin(&plugin, ctx),
            );
        }
        Ok(())
    }
}

/// The section: the list in the sidebar, and what it has selected beside it.
///
/// The list is worked out once and handed to both halves, because the card is
/// about *what the list is showing*: a query that filters the chosen plugin
/// out of the list leaves a card describing something nobody can see, so the
/// selection falls to the first row that survived.
///
/// Both halves are drawn in [`section`]'s frame, which is the frame the
/// settings are drawn in: the plugin's name is the page title, so it stays put
/// while the card scrolls, exactly as a settings page's name does.
fn page(workspace: &Workspace, state: &Rc<PluginsState>) -> (Box<dyn Element>, Box<dyn Element>) {
    let matching = list::matching(workspace);
    let selected = state.showing(&matching);
    let showing = selected.as_ref().and_then(|id| {
        workspace
            .host()
            .available()
            .iter()
            .find(|manifest| manifest.id == *id)
            .copied()
    });

    let list = list::render(workspace, &matching, selected.as_ref());
    // Unreachable while any plugin is loaded, and this page is one.
    let Some(manifest) = showing else {
        return (list, Empty::new().finish());
    };

    (
        list,
        section::content(
            manifest.name,
            card::render(workspace, manifest),
            workspace.settings_page().scroll_named(CARD_SCROLL),
            workspace.fonts().ui,
        ),
    )
}

/// What one of this page's per-plugin actions is called.
///
/// `owner/name` has a slash in it and an action name has exactly three parts,
/// so the plugin's own separator becomes a dash: `crook/usage` is toggled by
/// `crook/plugins/toggle-crook-usage` and shown by `…/show-crook-usage`.
pub(super) fn action(verb: &str, plugin: &PluginId) -> ActionName {
    ActionName::parse(&format!(
        "crook/plugins/{verb}-{}-{}",
        plugin.owner(),
        plugin.name()
    ))
    .expect("a name built from a verb and two names that already parsed")
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

/// The word a person would search for to find a tier, and what a card prints.
pub(super) fn tier_words(tier: Tier) -> (&'static str, &'static str) {
    match tier {
        Tier::Native => ("built-in", "Built in"),
        Tier::Wasm => ("sandboxed", "Installed, sandboxed"),
        Tier::Process => ("external", "Installed, runs as a program"),
    }
}
