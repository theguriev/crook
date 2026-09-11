//! A plugin's surface, standing still for its picture.
//!
//! Most of the flags in `--help` exist so that a surface can be drawn into a
//! PNG deterministically, and there was one surface none of them could reach:
//! whatever a sandboxed plugin draws. `--usage <PERCENT>` used to put a known
//! reading in the header's chip; the chip left the binary, and what replaced
//! it is a plugin whose contribution is a network answer, a file on somebody's
//! machine and a clock. `docs/plugins.md` §7 lists that regression as decision
//! 5 and three ways out of it. This is the first: a flag that stands a
//! contribution in for a **fixed [`Node`] tree**.
//!
//! ```sh
//! crook --plugin-fixture script/fixtures/header.json --snapshot header.png
//! ```
//!
//! # It is the plugins' own vocabulary, in JSON
//!
//! The file is a map of slot name to [`Node`], and a `Node` is `serde`'s to
//! decode — the same shape a guest encodes with `postcard` on the other side
//! of the wire, in the one format a person can write by hand. So a fixture is
//! not a picture of what the host *would* draw: it goes through
//! [`render::element`], the same translator, at the same scale, with the same
//! chrome, and a change to what a badge looks like changes the fixture's
//! picture with everybody's.
//!
//! What it cannot stand in for is a plugin's *behaviour* — nothing here runs,
//! ticks or answers — and it is not meant to: a picture is a picture of a
//! frame.
//!
//! # And a face, for a picture of the Plugins page
//!
//! A top-level `"icon"` names a PNG beside the fixture file, and the fixture
//! then carries it the way a module carries its own — so a picture of a row
//! with an icon on it is the same on every machine too. It is the one key
//! that is not a slot name, which is why the file is read as a map first and
//! as slots second.

use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};
use crook_plugin_api::Node;

use crate::picture::{self, Limits};
use crate::plugin::{BuildError, Host, Plugin};
use crate::plugins::pictures::Pictures;
use crate::workspace::Workspace;

use super::picker::Held;
use super::render::{self, Placement};

/// The key that names the icon rather than a slot.
const ICON_KEY: &str = "icon";

/// The plugin a fixture file becomes.
pub struct Fixture {
    /// What to draw, in the slot to draw it in, in the file's own order.
    trees: Vec<(String, Node)>,
    /// Its face, when the file named one.
    pictures: Pictures,
}

impl Fixture {
    /// Reads a fixture file, or says why it is not one.
    pub fn read(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|why| format!("could not be read: {why}"))?;
        let mut keyed: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&bytes).map_err(|why| format!("is not a fixture: {why}"))?;

        // The icon is taken out before the slots are read, so the rest of
        // the map is nothing but slots and a slot is nothing but a `Node`.
        // A path relative to the fixture rather than to the working
        // directory, because a fixture is checked in beside its picture.
        let pictures = match keyed.remove(ICON_KEY) {
            None => Pictures::default(),
            Some(serde_json::Value::String(named)) => {
                let at = path
                    .parent()
                    .map(|beside| beside.join(&named))
                    .unwrap_or_else(|| named.clone().into());
                let png = std::fs::read(&at)
                    .map_err(|why| format!("names an icon at {} that {why}", at.display()))?;
                let icon = picture::decode(&png, Limits::ICON)
                    .map_err(|why| format!("names an icon at {} that {why}", at.display()))?;
                Pictures {
                    icon: Some(Arc::new(icon)),
                    previews: Vec::new(),
                }
            }
            Some(_) => return Err(String::from("names an icon that is not a path")),
        };

        // A map rather than a list, because a slot takes one contribution from
        // one plugin and a file naming the same slot twice is a mistake worth
        // refusing at the door — which `BTreeMap` does by keeping the last, so
        // the order below is the file's sorted rather than its written one.
        let trees: BTreeMap<String, Node> =
            serde_json::from_value(serde_json::Value::Object(keyed))
                .map_err(|why| format!("is not a fixture: {why}"))?;

        match trees.is_empty() {
            true => Err(String::from("names no slots, so there is nothing to draw")),
            false => Ok(Self {
                trees: trees.into_iter().collect(),
                pictures,
            }),
        }
    }
}

impl Plugin for Fixture {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn pictures(&self) -> Option<&Pictures> {
        Some(&self.pictures)
    }

    fn build(&mut self, host: &mut Host, _: &mut ViewContext<Workspace>) -> Result<(), BuildError> {
        // The host's own chrome for a plugin's surface, which a fixture has to
        // have because the vocabulary includes things that hold state — a
        // panel that is open, a row the keyboard is on. Nothing here ever
        // changes it; what it is for is that the tree is drawn by the same
        // code as a real plugin's rather than by a copy of it.
        let chrome = Rc::new(Held::new(host.voice()));

        for (name, node) in &self.trees {
            let Some(slot) = host.slot_named(name) else {
                // A row slot is asked once per row and its contribution is
                // handed the row; a fixture has one answer, which for a panel
                // of seven tabs would be the same mark on all of them — a
                // picture of something no plugin does. So it is refused by
                // name rather than drawn wrongly.
                if host.row_slot_named(name).is_some() {
                    return Err(format!(
                        "{name:?} is drawn once per row and a fixture has one answer"
                    ));
                }
                return Err(format!("{name:?} is not a slot this build has"));
            };

            let node = node.clone();
            let chrome = chrome.clone();
            let hovers = Rc::new(render::Hovers::default());
            let placement = match name.as_str() {
                name if name == crate::plugins::pane::PANE_CHIPS.as_str() => Placement::Above,
                _ => Placement::Below,
            };

            // At the front of whatever is already there, because a fixture is
            // taking a picture *of* the plugin tier: a slot that only holds
            // one thing should hold this one.
            host.contribute(slot, "fixture", -1000, move |workspace, _| {
                render::element(
                    &node,
                    render::Chrome::new(
                        workspace.fonts(),
                        render::Scale::ROW,
                        placement,
                        &chrome,
                        workspace.clipboard(),
                    ),
                    // A fixture's buttons are inert, and they draw as buttons
                    // anyway: a control that answers to nothing is drawn dead
                    // rather than left out, which is a rule this borrows from
                    // the tier it is standing in for.
                    &|_| None,
                    &hovers,
                )
            });
        }

        Ok(())
    }
}

/// What this plugin says it is.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/fixture").expect("a literal that parses"),
        name: "Fixture",
        description: "A fixed plugin surface, so a picture of one is the same on every machine.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
        capabilities: &[],
    })
}

#[cfg(test)]
#[path = "fixture_tests.rs"]
mod tests;
