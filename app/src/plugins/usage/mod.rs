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
//! and gets the same model. What is still on the workspace is the poll the
//! setting starts, which is lifecycle rather than chip code.
//!
//! # The settings page is here too
//!
//! A person who disables this plugin should lose the page along with the chip,
//! and the only way that is true is if the page is this plugin's. It is added
//! through `settings.page`, which is a slot like any other.
//!
//! # A named action, and what that buys
//!
//! `crook/usage/refresh` is the first thing in Crook that can be asked for by
//! name. It does what clicking the pill does, and the difference is who can
//! ask: a line in `keymap.json` reading `"cmd-shift-u": "crook/usage/refresh"`
//! now works, and did not before, because there was no way to name it. Nothing
//! about the chip changed to make that true — the action is the seam.
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

use crate::plugin::{ActionName, BuildError, Host, Plugin};
use crate::usage_model::UsageModel;
use crate::workspace::SettingsAction;
use crate::workspace::Workspace;
use crate::workspace::settings_page::named;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{self, Category};

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

        // What clicking the pill does, reachable by a chord or by anything
        // else that can name an action.
        host.register_action(
            ActionName::parse("crook/usage/refresh").expect("a literal"),
            {
                let usage = usage.clone();
                move |_, ctx| {
                    usage.update(ctx, |model, ctx| model.refresh_from_user(ctx));
                }
            },
        );

        // The page that turns the chip off, which belongs here rather than in
        // the settings module for the same reason the chip does: it is this
        // feature's, and a person who disables this plugin should lose the
        // page along with the chip.
        host.add_settings_page("page", "Usage", 20, {
            let usage = usage.clone();
            move |workspace, app| page(workspace, &usage, app)
        });

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

/// The usage chip, and what turning it off actually stops.
///
/// The reading comes from the plugin's own handle rather than from
/// `Workspace::usage`, which is the difference between a page the settings
/// module happens to draw and a page this feature owns.
fn page(workspace: &Workspace, usage: &ModelHandle<UsageModel>, app: &AppContext) -> Vec<Category> {
    let ui = workspace.fonts().ui;
    let state = workspace.settings_page();
    let general = workspace.general();

    let chip = widgets::row(
        Words::new("Show the usage chip")
            .with_description(
                "The pill in the header, showing how much of the session budget is spent.",
            )
            .with_keywords(&[
                "usage", "token", "budget", "claude", "limit", "quota", "network", "poll", "pill",
            ]),
        true,
        widgets::switch(
            general.show_usage_chip,
            Some(SettingsAction::ToggleUsageChip.into()),
            state.control(named("show-usage-chip")),
        ),
        ui,
    );

    let reading = usage.as_ref(app);
    let current = match (reading.snapshot(), reading.problem()) {
        (Some(snapshot), None) => format!(
            "{}% of the session budget",
            snapshot.session_percent_rounded()
        ),
        (_, Some(problem)) => problem.chip_label().to_owned(),
        (None, None) if general.show_usage_chip => "not read yet".to_owned(),
        (None, None) => "not being read".to_owned(),
    };

    vec![
        widgets::category("Claude Code", vec![chip]),
        widgets::category(
            "Session",
            vec![
                widgets::note(
                    "Crook reads the session Claude Code already stores on this machine and asks \
                     Anthropic what it has spent. Turning the chip off stops both: a hidden chip \
                     does not poll.",
                    ui,
                ),
                widgets::fact(
                    Words::new("Last reading")
                        .with_keywords(&["usage", "percent", "spent", "session", "poll"]),
                    current,
                    false,
                    workspace.fonts(),
                ),
            ],
        ),
    ]
}
