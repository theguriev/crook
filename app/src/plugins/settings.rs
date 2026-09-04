//! The settings pane, and the slot every page of it comes from.
//!
//! The *pane* is the workspace's — it is a pane like any other, it opens in a
//! tab, it splits, it closes with the same chord — and so is the rail, the
//! search field and the scrolling column. What is *in* it is this slot.
//!
//! Nothing else is here. A plugin that owns a place in the interface is the
//! one that declares it, and the settings rail is a place with no page of its
//! own: every one of the five Crook ships belongs to the plugin whose feature
//! it configures, which is why the Usage page is `crook/usage`'s and not this
//! one's.

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::plugin::{BuildError, Host, Plugin};
use crate::workspace::Workspace;

/// The plugin that owns the settings rail.
pub struct Settings;

impl Plugin for Settings {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        host.declare_settings_slot();
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
