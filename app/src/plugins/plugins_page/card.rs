//! The card: everything worth knowing about one plugin.
//!
//! What a row cannot say. What it is, where it came from, whether it is
//! running, what it puts on screen and what it can be asked to do — and, when
//! it did not load, why.
//!
//! The frame is [`section::content`](crate::workspace::section::content)'s,
//! which is the same frame a settings page is drawn in, so what is here is
//! only the body: the plugin's *name* is the page title and is drawn by the
//! frame.
//!
//! # Three zones, and each is a shape the settings already have
//!
//! **Identity** is bare text: one line of facts with the plugin's state at the
//! front of it, and the sentence it describes itself with. **Decisions** are
//! boxes — [`widgets::asked`], the box the Store answers Install in — one per
//! thing a person decides here, holding what has to be read before pressing
//! anything in the same rectangle as the control that is pressed; and the
//! same box, captioned, around the one row on the card the host did not
//! write. **Reference** is categories, drawn by [`widgets::category_element`]
//! exactly as a category of settings is, so a heading here has the same rule
//! above it and the same gap under it as one over a group of settings.
//!
//! A box is where something is decided or where the plugin is drawing; the
//! bare page is where the host explains. That is the whole of the visual
//! grammar, and it is why the card no longer opens with a heading over a list
//! over a paragraph over a box: the eye lands on the state, then on the stack
//! of boxes, then past the first rule on the lists — which is the order the
//! questions are asked in. Is it on, what did I agree to, what does it do.
//!
//! # There is room here for a picture
//!
//! Deliberately: a plugin from a store will want one, and the shape of this
//! card is the one that has room for it at the top of the body, under the name
//! and above the facts, without anything else moving. Nothing carries an image
//! yet — a native plugin's manifest is a `&'static str` per field and a
//! sandboxed one's has no picture in it — so there is nothing to draw and this
//! says so rather than reserving a grey rectangle for a future release.

use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crook_plugin::{EntryId, Manifest, PluginId, SlotId, Slots};

use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{Command, Mark, Tone};
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{Workspace, WorkspaceAction};

use super::{
    HOLDS_THE_PAGE, PLUGIN_CARD, Stance, action, covered, elsewhere, only_by_name, stalled, stance,
    tier_words, wanted,
};

/// The gap under a box, before the next one.
///
/// Half the gap over the first box: the boxes are one stack, and members that
/// sat as far apart as the stack sits from the description would read as
/// three things rather than one.
const BOX_GAP: f32 = 8.;

/// The body of the card: everything under the plugin's name.
pub(super) fn render(
    workspace: &Workspace,
    app: &AppContext,
    manifest: &'static Manifest,
) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let on = workspace.host().is_loaded(&manifest.id);

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(facts(manifest, on, ui))
        .with_child(widgets::description(manifest.description, ui));

    // The stack of boxes: the switch, then what the plugin may do, then what
    // it says it is doing. In that order because it is the order somebody who
    // has just installed something needs them in — a fresh arrival should
    // meet the question before the controls that depend on the answer, and
    // the note under those controls says "the answer is above".
    let mut boxes = vec![Boxed::plain(switch(workspace, manifest, on, ui))];
    if let Some(block) = permissions(workspace, manifest, on, ui) {
        boxes.push(block);
    }

    // Kept, rather than only added, because whether the plugin drew its own
    // controls is also what decides how the list at the foot is drawn.
    let drawn = status(workspace, app, manifest, ui);
    let drew_its_own = drawn.is_some();
    if let Some(block) = drawn {
        boxes.push(Boxed::plain(block));
    }

    let last = boxes.len() - 1;
    for (index, boxed) in boxes.into_iter().enumerate() {
        // A box that ends in an explanation ends in the note's own gap
        // instead, which is what a note before a rule gets on the Appearance
        // page; the last bare box keeps the gap a row keeps before the rule.
        let gap = match (boxed.explained, index == last) {
            (true, _) => 0.,
            (false, true) => widgets::ROW_SPACING,
            (false, false) => BOX_GAP,
        };
        column.add_child(
            Container::new(boxed.element)
                .with_margin_bottom(gap)
                .finish(),
        );
    }

    // Before the list of slots rather than after everything, because every
    // complaint the audit can make is about a slot.
    let problems = problems(workspace, &manifest.id);
    if !problems.is_empty() {
        column.add_child(widgets::toned_category(
            "Problems",
            Tone::Warning,
            false,
            problems
                .into_iter()
                .map(|line| widgets::note(&line, ui).element)
                .collect(),
            ui,
        ));
    }

    let contributions = draws(workspace, &manifest.id);
    if !contributions.is_empty() {
        // One row per slot, with what the plugin put there on the line under
        // it. This used to be one line per entry, and a plugin with eight
        // entries in one menu said the menu's name eight times: what the
        // section is read for is *where*, and the entries are the detail.
        let rows: Vec<widgets::Entry> = contributions
            .into_iter()
            .map(|(slot, entries)| {
                let names: Vec<String> = entries.iter().map(ToString::to_string).collect();
                let keywords: Vec<&str> = names.iter().map(String::as_str).collect();
                widgets::row(
                    Words::new(slot.to_string())
                        .with_description(names.join(", "))
                        .with_keywords(&keywords),
                    true,
                    Empty::new().finish(),
                    ui,
                )
            })
            .collect();
        column.add_child(section("What it puts on screen", rows, ui));
    }

    let (commands, unoffered) = offers(workspace, &manifest.id);
    if !commands.is_empty() || unoffered > 0 {
        // A plugin that drew its own controls has already said what it can be
        // asked to do, in the shape it chose to be asked in — so a column of
        // Run buttons under it is the same commands a second time, in the
        // shape the card invented, and on the card of a plugin whose whole
        // surface is one row it is much the longest section on the page.
        //
        // Counted rather than hidden. A plugin that draws a chip has not
        // thereby promised that the chip reaches everything it can do, and a
        // card that answered "what can it be asked to do" with silence would
        // be worse than one that answers at length. What the count keeps is
        // the two facts a list of names is actually read for: how many there
        // are, and where to go for them.
        let rows: Vec<widgets::Entry> = if drew_its_own || commands.is_empty() {
            // Also the shape for a plugin that offers nothing and answers to
            // something: "and 7 more it does not offer" under a heading with
            // no list above it was a sentence starting with "and".
            vec![widgets::note(&elsewhere(commands.len(), unoffered), ui)]
        } else {
            // A row with a button rather than a line of text. What this
            // section used to be was nine action names a person could read and
            // not reach: the only way to run one was the command palette,
            // which the card never mentions. A command that is listed where it
            // cannot be run is a command most people will never find.
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
                // actions and not commands, and a page whose longest section
                // is somebody's internal wiring is a page nobody reads.
                rows.push(widgets::note(&only_by_name(unoffered), ui));
            }
            rows
        };
        column.add_child(section("What it can be asked to do", rows, ui));
    }

    column.finish()
}

/// One box of the stack, and whether it ends in a sentence of its own.
///
/// The gap under a box depends on what comes after it, which is known only
/// once the stack is built — and on whether the box brought its explanation
/// with it, which only the box knows.
struct Boxed {
    element: Box<dyn Element>,
    /// Whether an explanation with its own gap is already under the box.
    explained: bool,
}

impl Boxed {
    /// A box with nothing under it.
    fn plain(element: Box<dyn Element>) -> Self {
        Self {
            element,
            explained: false,
        }
    }
}

/// The line under the name: whether it is running, who owns it, which version,
/// where it came from.
fn facts(manifest: &Manifest, on: bool, ui: FamilyId) -> Box<dyn Element> {
    let (_, tier) = tier_words(manifest.tier);
    let state = if on { "running" } else { "switched off" };
    widgets::facts(
        Some((on, state)),
        &format!("{} \u{b7} {} \u{b7} {tier}", manifest.id, manifest.version),
        ui,
    )
}

/// The switch, with the word beside it rather than a bare toggle — and above
/// it, whatever a person has to know to read it.
///
/// A switch that cannot be thrown is told about *in the box it is in*, not in
/// the description and not in a section further down: the sentence that
/// explains a control belongs with the control. Three things can be that
/// sentence, and a warning is a heading in the one colour the Store already
/// speaks warnings in.
fn switch(workspace: &Workspace, manifest: &Manifest, on: bool, ui: FamilyId) -> Box<dyn Element> {
    let holds_the_page = HOLDS_THE_PAGE.contains(&manifest.id.as_str());
    // A version the registry withdrew is not a version to offer a switch for:
    // the plugin is off because somebody published a sentence about it, and a
    // switch that turned it back on would be a control that undoes a warning.
    // Updating or removing it is what the Store is for.
    let withdrawn = workspace.withdrawn(&manifest.id);
    let command = workspace
        .host()
        .action(&action("toggle", &manifest.id))
        .map(WorkspaceAction::Run)
        .filter(|_| !holds_the_page && withdrawn.is_none());
    let live = command.is_some();

    let mut question = None;
    let mut body = Vec::new();

    // Above everything else that could be wrong with a plugin, because this
    // is the one that is somebody else's news rather than this machine's
    // trouble.
    if let Some(why) = withdrawn {
        question = Some(("Withdrawn from the registry", Tone::Warning));
        body.push(widgets::note_text(why, ui));
        body.push(widgets::note_text(
            "This version is not being offered any more, so it is not running. The Store has \
             whatever replaced it, and Remove there takes this one off.",
            ui,
        ));
    }

    if let Some((_, problem)) = workspace
        .host()
        .refused()
        .iter()
        .find(|(id, _)| *id == manifest.id)
    {
        // A withdrawn version that also failed to load keeps the withdrawal
        // as its heading — that is the news — and the loader's sentence
        // follows as one more line.
        question = question.or(Some(("Did not load", Tone::Warning)));
        body.push(widgets::note_text(problem, ui));
    }

    if holds_the_page {
        body.push(widgets::note_text(
            "It is what draws the page you are on, so it cannot be switched off from here \
             \u{2014} the way back would be editing settings.json by hand.",
            ui,
        ));
    }

    let control = widgets::switch(
        on,
        command,
        workspace
            .settings_page()
            .control(named(&format!("plugins.switch.{}", manifest.id))),
    );
    widgets::asked(
        question,
        body,
        vec![widgets::answer_row("Enabled", live, control, ui)],
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
/// paragraph under the box says when the plugin acts on it.
///
/// The list and the button share a box, and the paragraph is under it rather
/// than between the two: the card exists so that a person reads the list
/// before answering, and what stood between the terms and the control was the
/// mechanism — reference, which goes where reference goes.
fn permissions(
    workspace: &Workspace,
    manifest: &Manifest,
    on: bool,
    ui: FamilyId,
) -> Option<Boxed> {
    if manifest.capabilities.is_empty() {
        return None;
    }

    let granted = workspace.settings().granted_to(manifest.id.as_str());
    let stance = stance(&wanted(manifest), granted);

    let items: Vec<Box<dyn Element>> = manifest
        .capabilities
        .iter()
        .map(|capability| {
            let sentence = capability.sentence();
            // Marked by the dot, and by the dot alone in every stance but
            // one. On the card where the list has grown, which line is new is
            // the whole message: the lines already in force go grey behind
            // filled dots, and the new ones are the only lit text in the list
            // — with the word kept, because the paragraph under the box says
            // "the lines marked new" and a shape is not a thing to quote.
            let (line, mark) = match stance {
                Stance::Unanswered => (sentence, Mark::Open),
                Stance::Allowed => (sentence, Mark::Settled),
                Stance::Escalated if covered(capability, granted) => (sentence, Mark::Faded),
                Stance::Escalated => (format!("{sentence} \u{2014} new"), Mark::Open),
            };
            widgets::item(&line, mark, ui)
        })
        .collect();

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

    let element = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(widgets::asked(
            Some((title, Tone::Plain)),
            items,
            vec![widgets::answer_row(state, live, control, ui)],
            ui,
        ))
        .with_child(widgets::footnote(explanation(stance, on), Tone::Plain, ui))
        .finish();

    Some(Boxed {
        element,
        explained: true,
    })
}

/// The paragraph under the box, where the mechanism is said out loud.
///
/// Three things the sentences themselves cannot say: that anything not allowed
/// is refused rather than quietly working, that allowing answers the whole list
/// rather than the line somebody was looking at, and that a plugin reads its
/// grant when it is *built* — so answering rebuilds it, there and then, and a
/// plugin that was refused a minute ago asks again a moment later. That last
/// one is worth saying on the card rather than only in a comment: a person who
/// allows something and is told nothing about what happens next has no way to
/// tell a control that worked from one that did not.
///
/// Unless the plugin is not running, in which case nothing is started and the
/// sentence must not say it is: the answer is written down, and the plugin
/// reads it when it is next switched on.
fn explanation(stance: Stance, on: bool) -> &'static str {
    match (stance, on) {
        (Stance::Unanswered, true) => {
            "None of this is allowed yet, and a plugin is refused everything it has not been \
             allowed. Allowing starts it again with the answer, which takes a moment."
        }
        (Stance::Unanswered, false) => {
            "None of this is allowed yet, and a plugin is refused everything it has not been \
             allowed. The answer takes effect when it is next switched on."
        }
        (Stance::Allowed, true) => {
            "Allowed, and nothing beyond it \u{2014} anything else this plugin asks for is \
             refused. Revoking starts it again with nothing allowed."
        }
        (Stance::Allowed, false) => {
            "Allowed, and nothing beyond it \u{2014} anything else this plugin asks for is \
             refused. Revoking takes effect when it is next switched on."
        }
        (Stance::Escalated, true) => {
            "This version asks for more than you allowed. What you allowed still holds and the \
             lines marked new are refused until you allow them; allowing answers the whole list \
             above and starts the plugin again with it."
        }
        (Stance::Escalated, false) => {
            "This version asks for more than you allowed. What you allowed still holds and the \
             lines marked new are refused until you allow them; allowing answers the whole list \
             above, and takes effect when it is next switched on."
        }
    }
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

/// Where this plugin has put something: each slot, and what is in it.
///
/// Asked of the host rather than of the plugin, which is the point: a plugin
/// cannot claim to draw something it did not contribute, and one that
/// contributed something it forgot to mention is listed anyway.
fn draws(workspace: &Workspace, plugin: &PluginId) -> Vec<(SlotId, Vec<EntryId>)> {
    let host = workspace.host();
    // Both registries, because a plugin's card should say what it draws and
    // not what kind of slot it drew it in: a mark on every tab row is the
    // loudest thing a plugin can put on screen, and listing only the slots
    // that are drawn once would leave it out.
    let mut slots = drawn_in(host.slots(), plugin);
    slots.extend(drawn_in(host.rows(), plugin));
    slots
}

/// The slots of one registry this plugin is in, each with its entries.
fn drawn_in<C: 'static>(slots: &Slots<C>, plugin: &PluginId) -> Vec<(SlotId, Vec<EntryId>)> {
    let mut filled = Vec::new();
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
        if !mine.is_empty() {
            filled.push((slot, mine));
        }
    }
    filled
}

/// What the plugin is doing, drawn from its own contribution to
/// [`PLUGIN_CARD`], or nothing when it contributed none.
///
/// Only its own: the slot is a list every plugin may put one entry on, and a
/// card that drew the whole list would put every plugin's state on every one
/// of their pages.
///
/// In a box with a heading, because it is the one thing on the card the host
/// did not write, and a box is how this card marks where the host stops. The
/// heading is in the same "What it…" family as the others so that the row
/// reads as an answer to a question the card asked, not as controls that
/// wandered in.
fn status(
    workspace: &Workspace,
    app: &AppContext,
    manifest: &Manifest,
    ui: FamilyId,
) -> Option<Box<dyn Element>> {
    let host = workspace.host();
    let slots = host.slots();
    let index = slots
        .contributors(PLUGIN_CARD)
        .into_iter()
        .position(|(owner, _)| owner == manifest.id)?;
    let drawn = slots.at(PLUGIN_CARD, index, |build| build(workspace, app))?;

    // Inside the box rather than under it, because what it is about is the
    // controls in the box. A line of prose floating between two blocks belongs
    // to whichever one the reader guesses.
    let mut body = vec![drawn];
    if let Some(why) = stalled(
        manifest,
        workspace.settings().granted_to(manifest.id.as_str()),
    ) {
        body.push(widgets::note_text(why, ui));
    }

    Some(widgets::asked(
        Some(("What it is doing", Tone::Plain)),
        body,
        Vec::new(),
        ui,
    ))
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
