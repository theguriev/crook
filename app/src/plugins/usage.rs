//! The usage chip: how much of the session's budget is spent, in the header.
//!
//! The first feature to be drawn from a slot rather than from a line in the
//! renderer that draws the row, and it was chosen because it is the smallest
//! thing that is genuinely a *feature*: a model that polls, a view that
//! observes it, a place in the chrome, and a setting that turns both off.
//!
//! # What has moved, and what has not
//!
//! What has moved is where it is drawn from. `header_toolbar` no longer names
//! the chip; it renders whatever is in [`HEADER_RIGHT`], and this is what is in
//! it. What has *not* moved is who owns the chip: the `UsageChip` view and the
//! `UsageModel` behind it are still built by `Workspace::new` and still live on
//! the workspace, so this plugin reads them rather than holding them. Moving
//! ownership needs a plugin to be able to make an entity while it builds, which
//! needs a context the workspace does not have until it exists — that is the
//! next thing to do here, not something to pretend has been done.
//!
//! # The setting is still the poll's switch
//!
//! Returning [`Empty`] rather than a chip drawn transparently keeps the rule
//! the old code stated: with the setting off there is no `ChildView`, so there
//! is no view being rendered, observed and laid out for a number nobody asked
//! for.

use crookui_core::elements::Margin;
use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId, Tier};

use crate::plugin::{BuildError, Host, Plugin};

use super::header::HEADER_RIGHT;

/// Space between the tab strip and the chip, so a wide title never runs into a
/// percentage.
const GUTTER: f32 = 12.;

/// Lifts the chip off the header's bottom edge, which the tabs sit flush
/// against.
const LIFT: f32 = 5.;

/// The plugin that puts the usage chip in the header.
pub struct Usage;

impl Plugin for Usage {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host) -> Result<(), BuildError> {
        host.contribute(HEADER_RIGHT, "chip", 0, |workspace, _| {
            if !workspace.general().show_usage_chip {
                return Empty::new().finish();
            }

            Container::new(ChildView::new(workspace.chip()).finish())
                .with_margin(Margin {
                    left: GUTTER,
                    bottom: LIFT,
                    ..Margin::default()
                })
                .finish()
        });
        Ok(())
    }
}

/// Built once and leaked; see `header::manifest`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/usage").expect("a literal that parses"),
        name: "Usage chip",
        description: "How much of the Claude Code session budget is spent, and when it resets.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
    })
}
