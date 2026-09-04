//! The usage chip: how much of the session's budget is spent, in the header.
//!
//! The first feature to be drawn from a slot rather than from a line in the
//! renderer that draws the row, and it was chosen because it is the smallest
//! thing that is genuinely a *feature*: a model that polls, a view that
//! observes it, a place in the chrome, and a setting that turns both off.
//!
//! # The plugin owns the view
//!
//! `chip.rs` is here rather than under `workspace/`, and that is the point of
//! a plugin directory: what a feature is made of sits together, and what is
//! left in `workspace/` is chrome.
//!
//! [`UsageChip`] is made here, while the plugin builds, and the handle is
//! captured by the closure that draws it — so the workspace neither holds it
//! nor knows it exists. That is the whole point of handing [`Plugin::build`] a
//! context: a plugin that can only contribute closures can decorate the
//! chrome, and a plugin that can make an entity is a feature.
//!
//! The *model* is not owned by anybody, here or before: [`UsageModel`] is a
//! singleton on the app, so this asks for the same handle the workspace would
//! and gets the same model. What is still on the workspace is the settings row
//! that turns the chip off and the poll it starts, which are not chip code and
//! move when there is a `settings.section` slot to move them into.
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
use crate::usage_model::UsageModel;
use crate::workspace::Workspace;

mod chip;

use chip::UsageChip;

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

    fn build(
        &mut self,
        host: &mut Host,
        ctx: &mut ViewContext<Workspace>,
    ) -> Result<(), BuildError> {
        let fonts = host.fonts();
        let chip = ctx.add_view(|ctx| UsageChip::new(fonts, ctx));

        // The canonical model-changed-so-repaint bridge. The chip observes the
        // same model for itself; this is what keeps the header honest when the
        // reading changes the chip's width and the row around it has to be
        // laid out again.
        let usage = UsageModel::handle(ctx);
        ctx.observe(&usage, |_, _, ctx| ctx.notify());

        host.contribute(HEADER_RIGHT, "chip", 0, move |workspace, _| {
            if !workspace.general().show_usage_chip {
                return Empty::new().finish();
            }

            Container::new(ChildView::new(&chip).finish())
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
