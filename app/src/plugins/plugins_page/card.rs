//! The card: everything worth knowing about one plugin.
//!
//! What a row cannot say. What it is, where it came from, whether it is
//! running, what it puts on screen and what it can be asked to do — and, when
//! it did not load, why.
//!
//! The frame is [`section::content`](crate::workspace::section::content)'s,
//! which is the same frame a settings page is drawn in, so what is here is
//! only the body: the plugin's *name* is the page title and is drawn by the
//! frame. What follows it is the facts, the description and the switch, which
//! belong to no heading — and then one [`widgets::category_element`] per
//! headed block, so a heading here has the same weight, the same rule above it
//! and the same gap under it as a category of settings.
//!
//! # There is room here for a picture
//!
//! Deliberately: a plugin from a store will want one, and the shape of this
//! card is the one that has room for it at the top of the body, under the name
//! and above the facts, without anything else moving. Nothing carries an image
//! yet — a native plugin's manifest is a `&'static str` per field and a
//! sandboxed one's has no picture in it — so there is nothing to draw and this
//! says so rather than reserving a grey rectangle for a future release.

use crookui_core::elements::{Padding, Paragraph};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crook_plugin::{EntryId, Manifest, PluginId};

use crate::theme::theme;
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{Workspace, WorkspaceAction};

use super::{HOLDS_THE_PAGE, action, tier_words};

/// The body of the card: everything under the plugin's name.
pub(super) fn render(workspace: &Workspace, manifest: &'static Manifest) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let host = workspace.host();

    let on = host.is_loaded(&manifest.id);
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(facts(manifest, on, ui))
        .with_child(description(manifest, ui))
        .with_child(switch(workspace, manifest, on, ui));

    if let Some(problem) = host
        .refused()
        .iter()
        .find(|(id, _)| *id == manifest.id)
        .map(|(_, problem)| problem.clone())
    {
        column.add_child(section(
            "Did not load",
            vec![widgets::note(&problem, ui)],
            ui,
        ));
    }

    let problems = problems(workspace, &manifest.id);
    if !problems.is_empty() {
        column.add_child(section(
            "Problems",
            problems
                .into_iter()
                .map(|line| widgets::note(&line, ui))
                .collect(),
            ui,
        ));
    }

    let contributions = draws(workspace, &manifest.id);
    if !contributions.is_empty() {
        column.add_child(section(
            "What it puts on screen",
            contributions
                .into_iter()
                .map(|line| widgets::note(&line, ui))
                .collect(),
            ui,
        ));
    }

    let (commands, unoffered) = offers(workspace, &manifest.id);
    if !commands.is_empty() || unoffered > 0 {
        let mut rows: Vec<widgets::Entry> = commands
            .into_iter()
            .map(|line| widgets::note(&line, ui))
            .collect();
        if unoffered > 0 {
            // Counted rather than listed. A plugin's own arrow keys are
            // actions and not commands, and a page whose longest section is
            // somebody's internal wiring is a page nobody reads. The Keyboard
            // Shortcuts page lists every one of them.
            rows.push(widgets::note(
                &format!(
                    "and {unoffered} more it does not offer, reachable by name \u{2014} the \
                     Keyboard Shortcuts page lists them."
                ),
                ui,
            ));
        }
        column.add_child(section("What it can be asked to do", rows, ui));
    }

    column.finish()
}

/// The line under it: who owns it, which version, where it came from.
fn facts(manifest: &Manifest, on: bool, ui: FamilyId) -> Box<dyn Element> {
    let (_, tier) = tier_words(manifest.tier);
    let running = if on { "running" } else { "switched off" };
    let line = format!(
        "{} \u{b7} {} \u{b7} {tier} \u{b7} {running}",
        manifest.id, manifest.version
    );

    Container::new(
        Text::new(line, ui, widgets::DESCRIPTION_SIZE)
            .with_color(theme().text_muted)
            .finish(),
    )
    .with_margin_bottom(14.)
    .finish()
}

/// What the plugin says it is for.
fn description(manifest: &Manifest, ui: FamilyId) -> Box<dyn Element> {
    let text = if HOLDS_THE_PAGE.contains(&manifest.id.as_str()) {
        format!(
            "{} It is what draws the page you are on, so it cannot be switched off from here \
             \u{2014} the way back would be editing settings.json by hand.",
            manifest.description
        )
    } else {
        manifest.description.to_owned()
    };

    Container::new(
        Paragraph::new(text, ui, widgets::LABEL_SIZE)
            .with_color(theme().text_primary)
            .with_line_height_ratio(1.5)
            .finish(),
    )
    .with_margin_bottom(18.)
    .finish()
}

/// The switch, with the word beside it rather than a bare toggle.
fn switch(workspace: &Workspace, manifest: &Manifest, on: bool, ui: FamilyId) -> Box<dyn Element> {
    let holds_the_page = HOLDS_THE_PAGE.contains(&manifest.id.as_str());
    let command = workspace
        .host()
        .action(&action("toggle", &manifest.id))
        .map(WorkspaceAction::Run)
        .filter(|_| !holds_the_page);

    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new("Enabled", ui, widgets::LABEL_SIZE)
                    .with_color(if holds_the_page {
                        theme().text_muted
                    } else {
                        theme().text_primary
                    })
                    .finish(),
            )
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
            .with_child(widgets::switch(
                on,
                command,
                workspace
                    .settings_page()
                    .control(named(&format!("plugins.switch.{}", manifest.id))),
            ))
            .finish(),
    )
    .with_background_color(theme().overlay_1)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
    .with_padding(Padding {
        top: 10.,
        bottom: 10.,
        left: 12.,
        right: 12.,
    })
    .finish()
}

/// A heading with lines under it.
///
/// A settings page's category, drawn by the same function, so that a heading
/// on this page and a heading on a settings page are the same heading with the
/// same rule above it. Never `first`: the facts, the description and the switch
/// are always above it, so there is always something for the rule to separate
/// this from.
fn section(title: &str, rows: Vec<widgets::Entry>, ui: FamilyId) -> Box<dyn Element> {
    widgets::category_element(
        title,
        false,
        rows.into_iter().map(|row| row.element).collect(),
        ui,
    )
}

/// Everything the audit has to say about this plugin.
///
/// A complaint that names it, and one about a slot it is in — a slot with more
/// in it than it can draw is a problem for every plugin that put something
/// there, and for nobody else.
fn problems(workspace: &Workspace, plugin: &PluginId) -> Vec<String> {
    let host = workspace.host();
    let slots = host.slots();
    host.audit()
        .into_iter()
        .filter(|complaint| {
            complaint.names(plugin)
                || complaint
                    .slot()
                    .is_some_and(|slot| slots.contributors(slot).iter().any(|(by, _)| by == plugin))
        })
        .map(|complaint| complaint.to_string())
        .collect()
}

/// Where this plugin has put something, in words.
///
/// Asked of the host rather than of the plugin, which is the point: a plugin
/// cannot claim to draw something it did not contribute, and one that
/// contributed something it forgot to mention is listed anyway.
fn draws(workspace: &Workspace, plugin: &PluginId) -> Vec<String> {
    let slots = workspace.host().slots();
    let mut lines: Vec<String> = Vec::new();
    for slot in slots.declared() {
        let mine: Vec<EntryId> = slots
            .contributors(slot)
            .into_iter()
            .filter(|(by, _)| by == plugin)
            .map(|(_, entry)| entry)
            .collect();
        for entry in mine {
            lines.push(format!("{slot} \u{2014} {entry}"));
        }
    }
    lines
}

/// What this plugin *offers*, and how many more it merely answers to.
///
/// The distinction `register_command` draws: an action is reachable by name,
/// a command is one a person should be able to find. The card lists the
/// commands and counts the rest — a plugin whose internal wiring is its
/// longest section is a card nobody reads.
fn offers(workspace: &Workspace, plugin: &PluginId) -> (Vec<String>, usize) {
    let host = workspace.host();
    let mine: Vec<crate::plugin::ActionName> = host
        .actions()
        .names()
        .into_iter()
        .filter(|(_, owner)| owner == plugin)
        .map(|(name, _)| name)
        .collect();

    let offered: Vec<String> = mine
        .iter()
        .filter_map(|name| {
            host.title_of(name)
                .map(|title| format!("{name} \u{2014} {title}"))
        })
        .collect();
    let rest = mine.len() - offered.len();
    (offered, rest)
}
