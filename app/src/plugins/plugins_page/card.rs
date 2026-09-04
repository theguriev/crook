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

use crook_plugin::{EntryId, Manifest, PluginId, SlotId, Slots};

use crate::theme::theme;
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::Command;
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{Workspace, WorkspaceAction};

use super::{
    HOLDS_THE_PAGE, PLUGIN_CARD, Stance, action, covered, stance, tier_words, wanted,
};

/// The corner of the box a control sits in.
///
/// The card's own rounding, so the box reads as part of the card rather than
/// as something dropped on top of it.
const CONTROL_RADIUS: f32 = 8.;

/// The inset inside that box.
///
/// Wider than it is tall, which is what the switch has carried since it was
/// the only control here: a label at one end and a control at the other need
/// more air along the row than across it, or both sit against a corner.
const CONTROL_PADDING: Padding = Padding {
    top: 10.,
    bottom: 10.,
    left: 12.,
    right: 12.,
};

/// The body of the card: everything under the plugin's name.
pub(super) fn render(
    workspace: &Workspace,
    app: &AppContext,
    manifest: &'static Manifest,
) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let host = workspace.host();

    let on = host.is_loaded(&manifest.id);
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(facts(manifest, on, ui))
        .with_child(description(manifest, ui))
        .with_child(switch(workspace, manifest, on, ui));

    // Directly under the switch, and above everything that merely describes
    // the plugin: what it may do is the other decision this card exists to
    // let somebody make, and an escalation is a thing to see without
    // scrolling past four sections of prose first.
    if let Some(block) = permissions(workspace, manifest, ui) {
        column.add_child(block);
    }

    // What the plugin says about itself right now, if it says anything. Above
    // the sections that merely describe it, because "Microwave, ringing" is
    // the line somebody opened this card to read and the rest is reference.
    if let Some(status) = status(workspace, app, &manifest.id) {
        column.add_child(status);
    }

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
        // A row with a button rather than a line of text. What this section
        // used to be was nine action names a person could read and not reach:
        // the only way to run one was the command palette, which the card
        // never mentions. A command that is listed where it cannot be run is a
        // command most people will never find.
        let mut rows: Vec<widgets::Entry> = commands
            .into_iter()
            .map(|offered| {
                let live = offered.command.is_some();
                let control = widgets::text_button(
                    "Run",
                    offered.command,
                    workspace
                        .settings_page()
                        .control(named(&format!("plugins.run.{}", offered.name))),
                    ui,
                );
                widgets::row(
                    Words::new(offered.title)
                        .with_description(&offered.name)
                        .with_keywords(&[&offered.name]),
                    live,
                    control,
                    ui,
                )
            })
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
    let live = command.is_some();

    answer(
        "Enabled",
        live,
        widgets::switch(
            on,
            command,
            workspace
                .settings_page()
                .control(named(&format!("plugins.switch.{}", manifest.id))),
        ),
        ui,
    )
}

/// What the plugin asked to be allowed to do, and the one control that answers.
///
/// Absent — rather than a heading with nothing under it — for a plugin that
/// asked for nothing, which is every native one: a section saying "nothing" on
/// nine cards out of ten is a section a person has learned to skip by the time
/// they reach the one where it matters.
///
/// Read from the settings rather than from the host, and the difference is the
/// point: the host holds what each plugin was *built* with, which is what a
/// request is answered against, while the settings hold the answer as it
/// stands. A card that showed the host's copy would go on saying "Not allowed"
/// after somebody pressed Allow. What it shows instead is the answer, and the
/// paragraph under the list says when the plugin acts on it.
fn permissions(
    workspace: &Workspace,
    manifest: &Manifest,
    ui: FamilyId,
) -> Option<Box<dyn Element>> {
    if manifest.capabilities.is_empty() {
        return None;
    }

    let granted = workspace.settings().granted_to(manifest.id.as_str());
    let stance = stance(&wanted(manifest), granted);

    let mut rows: Vec<widgets::Entry> = manifest
        .capabilities
        .iter()
        .map(|capability| {
            let sentence = capability.sentence();
            // Marked only in the state where a mark says something. A column
            // of "allowed" beside every line is a column nobody reads to the
            // bottom; on the card where the list has grown, which line is new
            // is the whole message.
            let line = match stance {
                Stance::Escalated if covered(capability, granted) => {
                    format!("{sentence} \u{2014} allowed")
                }
                Stance::Escalated => format!("{sentence} \u{2014} new"),
                _ => sentence,
            };
            widgets::note(&line, ui)
        })
        .collect();
    rows.push(widgets::note(explanation(stance), ui));

    let (title, state, label, verb) = match stance {
        Stance::Unanswered => (
            "What it wants to be allowed to do",
            "Not allowed",
            "Allow",
            "allow",
        ),
        Stance::Allowed => ("What it is allowed to do", "Allowed", "Revoke", "revoke"),
        Stance::Escalated => (
            "It is asking for more than you allowed",
            "Partly allowed",
            "Allow",
            "allow",
        ),
    };

    let command = workspace
        .host()
        .action(&action(verb, &manifest.id))
        .map(WorkspaceAction::Run);
    let live = command.is_some();
    let control = widgets::text_button(
        label,
        command,
        workspace
            .settings_page()
            .control(named(&format!("plugins.grant.{}", manifest.id))),
        ui,
    );

    Some(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(section(title, rows, ui))
            .with_child(answer(state, live, control, ui))
            .finish(),
    )
}

/// The paragraph under the list, where the mechanism is said out loud.
///
/// Three things the sentences themselves cannot say: that anything not allowed
/// is refused rather than quietly working, that allowing answers the whole list
/// rather than the line somebody was looking at, and that a plugin reads its
/// grant when it is *built* — so answering rebuilds it, there and then, and a
/// plugin that was refused a minute ago asks again a moment later. That last
/// one is worth saying on the card rather than only in a comment: a person who
/// allows something and is told nothing about what happens next has no way to
/// tell a control that worked from one that did not.
fn explanation(stance: Stance) -> &'static str {
    match stance {
        Stance::Unanswered => {
            "None of this is allowed yet, and a plugin is refused everything it has not been \
             allowed. Allowing starts it again with the answer, which takes a moment."
        }
        Stance::Allowed => {
            "Allowed, and nothing beyond it \u{2014} anything else this plugin asks for is \
             refused. Revoking starts it again with nothing allowed."
        }
        Stance::Escalated => {
            "This version asks for more than you allowed. What you allowed still holds and the \
             lines marked new are refused until you allow them; allowing answers the whole list \
             above and starts the plugin again with it."
        }
    }
}

/// The box a control that answers a question about this plugin sits in.
///
/// One shape for both of them. "Is it on?" and "may it do this?" are the same
/// kind of question — the two decisions this card exists to let somebody make —
/// and two boxes differing by a couple of pixels would read as two mechanisms.
///
/// `live` is whether the control does anything, and it is the label that says
/// so: a muted word beside a control that cannot be pressed is the only hint
/// there is, since a disabled control carries no handler at all.
fn answer(
    label: &'static str,
    live: bool,
    control: Box<dyn Element>,
    ui: FamilyId,
) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new(label, ui, widgets::LABEL_SIZE)
                    .with_color(if live {
                        theme().text_primary
                    } else {
                        theme().text_muted
                    })
                    .finish(),
            )
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
            .with_child(control)
            .finish(),
    )
    .with_background_color(theme().overlay_1)
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CONTROL_RADIUS)))
    .with_padding(CONTROL_PADDING)
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
    host.audit()
        .into_iter()
        .filter(|complaint| {
            complaint.names(plugin)
                || complaint.slot().is_some_and(|slot| {
                    contributes_to(host.slots(), slot, plugin)
                        || contributes_to(host.rows(), slot, plugin)
                })
        })
        .map(|complaint| complaint.to_string())
        .collect()
}

/// Whether this plugin put something in that slot, in whichever registry the
/// slot lives in.
fn contributes_to<C: 'static>(slots: &Slots<C>, slot: SlotId, plugin: &PluginId) -> bool {
    slots.contributors(slot).iter().any(|(by, _)| by == plugin)
}

/// Where this plugin has put something, in words.
///
/// Asked of the host rather than of the plugin, which is the point: a plugin
/// cannot claim to draw something it did not contribute, and one that
/// contributed something it forgot to mention is listed anyway.
fn draws(workspace: &Workspace, plugin: &PluginId) -> Vec<String> {
    let host = workspace.host();
    // Both registries, because a plugin's card should say what it draws and
    // not what kind of slot it drew it in: a mark on every tab row is the
    // loudest thing a plugin can put on screen, and listing only the slots
    // that are drawn once would leave it out.
    let mut lines = drawn_in(host.slots(), plugin);
    lines.extend(drawn_in(host.rows(), plugin));
    lines
}

/// The lines for one registry.
fn drawn_in<C: 'static>(slots: &Slots<C>, plugin: &PluginId) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for slot in slots.declared() {
        // Not the card's own status line. This section exists to say where on
        // screen a plugin's work shows up — which chip in the header is whose
        // — and naming a thing drawn six inches above, on this page, is
        // telling somebody about what they are looking at.
        if slot == PLUGIN_CARD {
            continue;
        }
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

/// What the plugin is doing, drawn from its own contribution to
/// [`PLUGIN_CARD`], or nothing when it contributed none.
///
/// Only its own: the slot is a list every plugin may put one entry on, and a
/// card that drew the whole list would put every plugin's state on every one
/// of their pages.
fn status(workspace: &Workspace, app: &AppContext, plugin: &PluginId) -> Option<Box<dyn Element>> {
    let host = workspace.host();
    let slots = host.slots();
    let index = slots
        .contributors(PLUGIN_CARD)
        .into_iter()
        .position(|(owner, _)| &owner == plugin)?;
    let drawn = slots.at(PLUGIN_CARD, index, |build| build(workspace, app))?;

    Some(
        Container::new(drawn)
            .with_padding(CONTROL_PADDING)
            .with_background_color(theme().overlay_1)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CONTROL_RADIUS)))
            .with_margin_bottom(18.)
            .finish(),
    )
}

/// What this plugin *offers*, and how many more it merely answers to.
///
/// The distinction `register_command` draws: an action is reachable by name,
/// a command is one a person should be able to find. The card lists the
/// commands and counts the rest — a plugin whose internal wiring is its
/// longest section is a card nobody reads.
fn offers(workspace: &Workspace, plugin: &PluginId) -> (Vec<Offered>, usize) {
    let host = workspace.host();
    let mine: Vec<crate::plugin::ActionName> = host
        .actions()
        .names()
        .into_iter()
        .filter(|(_, owner)| owner == plugin)
        .map(|(name, _)| name)
        .collect();

    let offered: Vec<Offered> = mine
        .iter()
        .filter_map(|name| {
            let title = host.title_of(name)?;
            Some(Offered {
                // The title is the label and the name goes under it. What a
                // person is looking for is what the command *does*; the name
                // is what they need only once they want to bind it, and
                // putting it first left every row starting with three words
                // of punctuation.
                title: title.to_owned(),
                name: name.to_string(),
                command: host.action(name).map(WorkspaceAction::Run),
            })
        })
        .collect();
    let rest = mine.len() - offered.len();
    (offered, rest)
}

/// One command a plugin offers, and how to run it.
struct Offered {
    /// What a palette would call it, which is the row's label.
    title: String,
    /// `owner/plugin/action`, under the label and in the row's keywords: it is
    /// what somebody binding a chord to this needs, and what somebody reading
    /// the card does not.
    name: String,
    /// What pressing it does, or `None` for a command that has gone — a plugin
    /// switched off between the list being read and the frame being drawn.
    command: Command,
}
