//! The settings section, and the slot every page of it comes from.
//!
//! Settings are a *section of the sidebar*, not a tab: the button at the foot
//! of the panel puts the rail of pages where the tab list was and the page
//! itself where the panes were. They were a pane once — Warp's design, and a
//! good one for Warp — and what replaced it is Telegram's: one column, whose
//! contents the buttons at its foot switch.
//!
//! Nothing else is here but the *section*: the button at the foot of the
//! sidebar and what the window shows while it is chosen. A plugin that owns a
//! place in the interface is the one that declares it, and the settings rail
//! is a place with no page of its own — every one of the five Crook ships
//! belongs to the plugin whose feature it configures, which is why the Usage
//! page is `crook/usage`'s and not this one's.

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::{Workspace, settings_page};

/// The key the Settings section answers to.
///
/// Named here rather than resolved by title, because the window itself has to
/// be able to show it — `--settings` asks for it, and so does the options menu's
/// last entry.
pub const SETTINGS_SECTION: &str = "crook/settings/section";

/// The plugin that owns the settings rail.
pub struct Settings;

impl Plugin for Settings {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.declare_settings_slot();
        // The rail and the page beside it are one answer: the query in the
        // rail's box filters both, and which page is *shown* is worked out
        // from what survived. Two builders would work it out twice.
        host.add_sidebar_section(
            "section",
            "Settings",
            Lucide::Settings,
            0,
            settings_page::render,
        );
        Ok(())
    }
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/settings").expect("a literal that parses"),
        name: "Settings",
        description: "The settings rail, and the slot every page of it comes from.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
    })
}
