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
//! # The pictures are the plugin's own
//!
//! Two of them. The icon is beside the name in the title — the frame draws
//! it, from the same box a row of the list draws it in, so the face beside a
//! name is one face — and the previews are a category of their own, "What it
//! looks like", opened by a press rather than decoded for every card somebody
//! scrolls past. Both come out of the module itself, read beside the manifest
//! when it opened: nothing is fetched to draw them, and a plugin that carries
//! none has no title mark and no category rather than a grey rectangle where
//! a picture would go.
//!
//! # What the registry says, on the card that is about the plugin
//!
//! A box after the switch, for a plugin that is a file on this machine: the
//! version the registry has and the one that is running, Update where the
//! registry is ahead, Show in Store where there is a row to show, and Remove.
//! Updating is the Store's to do — the row runs the Store's own action about
//! this plugin, and is dead while the Store is off — but it is asked for
//! here, because "is there a newer one" is a question about the plugin, and
//! the plugin's card is where somebody looks for the answer.

use std::rc::Rc;

use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crook_plugin::{EntryId, Manifest, PluginId, SlotId, Slots, Tier};

use crate::plugin::ActionName;
use crate::plugins::store;
use crate::plugins::store::index::{Busy, Change, Release, change};
use crate::workspace::settings_page::search::Words;
use crate::workspace::settings_page::widgets::{Command, Mark, Tone};
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{Workspace, WorkspaceAction};

use super::state::PluginsState;
use super::{
    HOLDS_THE_PAGE, PLUGIN_CARD, Stance, about, action, covered, elsewhere, only_by_name, stalled,
    stance, tier_words, wanted,
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
    state: &Rc<PluginsState>,
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
    // Then what the registry says and what this machine holds, before the
    // grant: a version that is behind is news, and the box says whether
    // taking the update would ask for more than the box under it allows.
    if let Some(block) = machine(workspace, manifest, ui) {
        boxes.push(block);
    }
    // What the last press about *this* plugin came to, under the box it
    // was pressed in when there is one and under the switch when there is
    // not — a Remove refused for a plugin whose card never offered it was
    // run from the command line naming the plugin by hand, and the refusal
    // has to land on a card that has no machine box. Only this plugin's,
    // for the reason the Store keeps a sentence with its row: a removal
    // that failed is news on the card of the plugin that is still there.
    if let Some((sentence, wrong)) = state.said_about(&manifest.id) {
        let tone = match wrong {
            true => Tone::Warning,
            false => Tone::Plain,
        };
        let said = widgets::footnote(&format!("{} {sentence}", manifest.name), tone, ui);
        let under = boxes.pop().expect("the switch's box is always there");
        boxes.push(under.with_footnote(said));
    }
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

    // What it looks like comes first among the categories: it is the one a
    // person who has just installed something scrolls for, and the one that
    // was on the Store card a moment ago.
    if let Some(category) = pictures(workspace, manifest, state, ui) {
        column.add_child(category);
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
        column.add_child(listed("What it puts on screen", rows, ui));
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
        column.add_child(listed("What it can be asked to do", rows, ui));
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

    /// The same box with `footnote` under it — after whatever explanation it
    /// already ends in.
    fn with_footnote(self, footnote: Box<dyn Element>) -> Self {
        Self {
            element: Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(self.element)
                .with_child(footnote)
                .finish(),
            explained: true,
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
            "This version is not being offered any more, so it is not running. The box under \
             this one has what the registry offers instead, and Remove.",
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

    let command = run_about(workspace, about(verb), &manifest.id);
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
        Boxed::plain(widgets::asked(
            Some((title, Tone::Plain)),
            items,
            vec![widgets::answer_row(state, live, control, ui)],
            ui,
        ))
        .with_footnote(widgets::footnote(explanation(stance, on), Tone::Plain, ui)),
    )
}

/// `name`, run about this plugin — or `None`, drawn dead, while nothing
/// answers to the name.
fn run_about(workspace: &Workspace, name: ActionName, plugin: &PluginId) -> Command {
    workspace
        .host()
        .action(&name)
        .map(|id| WorkspaceAction::RunAbout(id, workspace.subject(plugin.as_str())))
}

/// The box about this machine and the registry: the version here, the
/// version there, and what can be done about either.
///
/// Only for a plugin that is a file — a native plugin is the binary, and the
/// binary is updated by the About page's own sentence — and only when at
/// least one row applies: a module being run from where it was built, with
/// no registry row and nothing installed, has nothing for the box to say.
///
/// Update goes first and Remove last, in the order of how much each undoes.
/// The heading and the body are there only when there is news: a newer
/// version, or a replacement for one taken back. A box that opened with
/// "Nothing newer" on every card would be a heading nobody reads by the
/// third card.
fn machine(workspace: &Workspace, manifest: &Manifest, ui: FamilyId) -> Option<Boxed> {
    if manifest.tier != Tier::Wasm {
        return None;
    }

    let heard = workspace.heard();
    let offer = heard.offer(&manifest.id);
    let withdrawn = workspace.withdrawn(&manifest.id);
    let installed = workspace.is_installed(&manifest.id);
    let standing = offer.map(|offer| change(offer, Some(manifest.version), withdrawn.is_some()));

    let mut question = None;
    let mut body: Vec<Box<dyn Element>> = Vec::new();
    let mut foot: Vec<Box<dyn Element>> = Vec::new();

    if let Some(Change::Update(release) | Change::Replace(release)) = &standing {
        let replacing = matches!(standing, Some(Change::Replace(_)));
        question = Some((
            match replacing {
                true => "The registry has a replacement",
                false => "A newer version is in the registry",
            },
            Tone::Plain,
        ));
        body.extend(asks_beyond(workspace, manifest, release, ui));

        // What the button says depends on what the Store is doing about it
        // and on whether the Store is there at all: a fetch in flight is a
        // dead button saying so, and a Store switched off is a dead button
        // with the reason under the row, since the card cannot fetch on its
        // own.
        let fetching = run_about(workspace, store::action("update"), &manifest.id);
        let (label, control): (String, Command) = match (heard.busy(&manifest.id), fetching) {
            (Some(Busy::Downloading), _) => (String::from("Getting it\u{2026}"), None),
            (Some(Busy::Waiting), _) => (String::from("Waiting\u{2026}"), None),
            (None, command) => (verb_for(replacing, release), command),
        };
        if heard.busy(&manifest.id).is_none() && control.is_none() {
            body.push(widgets::note_text(
                "The Store is switched off, and it is what fetches.",
                ui,
            ));
        }
        let live = control.is_some();
        let said = match withdrawn {
            Some(why) => format!("Taken back: {why}"),
            None => format!("Version {}", manifest.version),
        };
        foot.push(widgets::answer_row(
            said,
            live,
            widgets::text_button(
                label,
                control,
                workspace
                    .settings_page()
                    .control(named(&format!("plugins.update.{}", manifest.id))),
                ui,
            ),
            ui,
        ));
    }

    if offer.is_some() {
        let command = run_about(workspace, store::action("show"), &manifest.id);
        let live = command.is_some();
        foot.push(widgets::answer_row(
            "In the registry",
            live,
            widgets::text_button(
                "Show in Store",
                command,
                workspace
                    .settings_page()
                    .control(named(&format!("plugins.store.{}", manifest.id))),
                ui,
            ),
            ui,
        ));
    }

    if installed {
        let command = run_about(workspace, about("remove"), &manifest.id);
        let live = command.is_some();
        foot.push(widgets::answer_row(
            "On this machine",
            live,
            widgets::text_button(
                "Remove",
                command,
                workspace
                    .settings_page()
                    .control(named(&format!("plugins.remove.{}", manifest.id))),
                ui,
            ),
            ui,
        ));
    }

    if foot.is_empty() {
        return None;
    }

    Some(
        Boxed::plain(widgets::asked(question, body, foot, ui)).with_footnote(widgets::footnote(
            "Updating keeps what you allowed, and a version that asks for more says so above. \
             Removing takes it off this machine along with the answer.",
            Tone::Plain,
            ui,
        )),
    )
}

/// What the update button says.
fn verb_for(replacing: bool, release: &Release) -> String {
    match replacing {
        true => format!("Install {}", release.version),
        false => format!("Update to {}", release.version),
    }
}

/// What the offered release asks for beyond what is allowed, as the body of
/// the update box.
///
/// One sentence when its keys are within the grant; otherwise the sentence
/// that says so and the list it will ask about, marked open. The whole list
/// rather than the new lines only, because the index carries the sentences
/// and the keys as two lists that do not pair up line for line — which
/// lines are new is worked out from the module itself once it has landed,
/// and the card says so.
fn asks_beyond(
    workspace: &Workspace,
    manifest: &Manifest,
    release: &Release,
    ui: FamilyId,
) -> Vec<Box<dyn Element>> {
    let granted = workspace.settings().granted_to(manifest.id.as_str());
    let within = release
        .capabilities
        .iter()
        .all(|key| granted.iter().any(|had| had == key));

    if within {
        return vec![widgets::note_text(
            &format!("{} asks for nothing you have not allowed.", release.version),
            ui,
        )];
    }

    let mut body = vec![widgets::note_text(
        "It asks for more than you have allowed; after the update its card says what, marked \
         new, and refuses it until you allow it.",
        ui,
    )];
    body.extend(
        release
            .asks
            .iter()
            .map(|ask| widgets::item(ask, Mark::Open, ui)),
    );
    body
}

/// What it looks like: the previews the module carries, opened by a press.
///
/// Absent for a plugin that carries none, which is most of them. The row
/// says how many there are and offers to show them; while they are being
/// decoded it is dead and says so, and the room each will take is drawn in
/// the box fill so nothing under them moves when they land. Decoded on a
/// press rather than with the card, because a card is drawn for every
/// plugin somebody scrolls past and a preview is up to four million pixels.
fn pictures(
    workspace: &Workspace,
    manifest: &Manifest,
    state: &Rc<PluginsState>,
    ui: FamilyId,
) -> Option<Box<dyn Element>> {
    let carried = workspace.host().pictures_of(&manifest.id)?;
    let count = carried.count();
    if count == 0 {
        return None;
    }

    let opening = state.is_opening(&manifest.id);
    let shown = state.pictures_of(&manifest.id);
    let label = match count {
        1 => String::from("1 picture inside"),
        count => format!("{count} pictures inside"),
    };
    let (button, command) = match opening {
        true => ("Opening\u{2026}", None),
        false => (
            "Show pictures",
            run_about(workspace, about("pictures"), &manifest.id),
        ),
    };
    let live = command.is_some();
    let control = widgets::text_button(
        button,
        command,
        workspace
            .settings_page()
            .control(named(&format!("plugins.pictures.{}", manifest.id))),
        ui,
    );

    let mut rows = vec![
        widgets::row(
            Words::new(label).with_keywords(&["picture", "preview", "screenshot"]),
            live,
            control,
            ui,
        )
        .element,
    ];

    // The logical size comes from the header the module carried, not from
    // the pixels held now: `decode_preview` keeps a picture to a thousand
    // pixels, and one it shrank must still be drawn at the size it was
    // captured to be drawn at.
    match shown {
        Some(pictures) => rows.extend(pictures.into_iter().map(|picture| {
            widgets::preview(
                Some(&picture.bitmap),
                widgets::preview_size(picture.width, picture.height),
                picture.caption.as_deref(),
                ui,
            )
        })),
        None if opening => rows.extend(carried.previews.iter().map(|preview| {
            widgets::preview(
                None,
                widgets::preview_size(preview.width, preview.height),
                preview.caption.as_deref(),
                ui,
            )
        })),
        None => {}
    }

    Some(section("What it looks like", rows, ui))
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
fn section(title: &str, rows: Vec<Box<dyn Element>>, ui: FamilyId) -> Box<dyn Element> {
    widgets::category_element(title, false, rows, ui)
}

/// The same, over a category's entries.
fn listed(title: &str, rows: Vec<widgets::Entry>, ui: FamilyId) -> Box<dyn Element> {
    section(title, rows.into_iter().map(|row| row.element).collect(), ui)
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
