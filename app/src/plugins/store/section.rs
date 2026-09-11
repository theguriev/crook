//! The Store: the list a plugin is chosen from, and the card it is installed
//! from.
//!
//! The same two halves every section of the sidebar has, in the same frames —
//! a field over a list, and a title over a card — because a plugin that has
//! not been installed yet is not a different kind of thing from one that has,
//! and a store that looked like a different application would be one. The
//! rows are the Plugins list's rows, drawn by the same function: a face in a
//! box before the name, and one word after it.
//!
//! # What a row is allowed to say
//!
//! Its name, and one word about where it stands: installed, or the version an
//! update would bring, or that this build cannot run it. Everything else — the
//! description, the licence, where the source is, and above all *what it will
//! ask to be allowed to do* — is on the card, because those are the things
//! somebody should have read before pressing anything, and a list that tried
//! to say them says none of them.
//!
//! # Nothing is fetched by drawing this
//!
//! Opening the section reads the copy on disk and asks the network nothing.
//! The button at the foot is the request, and its line above says how old the
//! answer it is showing is — so an offline machine shows last week's list with
//! "checked 6 days ago" under it rather than an empty page. The one other
//! button here that fetches is "Fetch pictures", and it says beside itself
//! that it fetches the plugin: the pictures are inside the module, so seeing
//! them before installing is downloading the same file Install would.

use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use crookui_core::elements::Paragraph;
use crookui_core::fonts::FamilyId;
use crookui_core::image::Bitmap;
use crookui_core::prelude::*;

use crook_plugin::PluginId;

use crate::plugins::pictures::Decoded;
use crate::theme::theme;
use crate::workspace::section;
use crate::workspace::settings_page::search::{Query, Words};
use crate::workspace::settings_page::widgets::{Mark, Tone};
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{SettingsAction, TextField, Workspace, WorkspaceAction};

use super::index::{Busy, Change, Heard, Offer, change};
use super::model::Icons;
use super::state::StoreState;
use super::{FIELD, SECTION, action};

/// What the field says while nothing has been typed.
const PLACEHOLDER: &str = "Search the registry";

/// Where the list has been scrolled to.
const LIST_SCROLL: &str = "store.list";

/// Where the card has been scrolled to.
const CARD_SCROLL: &str = "store.card";

/// Everything the section draws from, read out of the model in one go.
///
/// One read rather than six, because a model is checked out of the app for the
/// duration of each one and the section wants a consistent answer: a list that
/// was fetched between two of them would be a card about a row the list no
/// longer has.
struct Known {
    /// The offers and what is being fetched — the same snapshot the rest of
    /// the window hears, read here straight off the model.
    heard: Heard,
    looking: bool,
    fetched: Option<SystemTime>,
    /// What went wrong, and which plugin it was about — `None` for the
    /// registry itself, which is nobody's row.
    problem: Option<(Option<PluginId>, String)>,
    /// The same for what went right.
    said: Option<(Option<PluginId>, String)>,
    /// The faces the list carries, as far as they have been decoded.
    icons: Icons,
    /// Which plugin's module is being looked inside, if any.
    looking_inside: Option<PluginId>,
    /// The last module somebody looked inside, and what was in it.
    looked_inside: Option<Looked>,
}

/// The pictures out of one module, as the card draws them.
struct Looked {
    /// Whose they are.
    plugin: PluginId,
    /// Whether the module was fetched to get them — in which case its bytes
    /// are held and Install needs no second download.
    fetched: bool,
    /// The pictures, each at the size it was captured at.
    pictures: Decoded,
}

impl Known {
    /// The line to draw on `plugin`'s card, if the last thing that happened
    /// was about it.
    fn about(&self, plugin: &PluginId) -> Option<(&str, bool)> {
        fn mine<'a>(
            line: &'a Option<(Option<PluginId>, String)>,
            plugin: &PluginId,
            wrong: bool,
        ) -> Option<(&'a str, bool)> {
            let (about, text) = line.as_ref()?;
            (about.as_ref() == Some(plugin)).then_some((text.as_str(), wrong))
        }

        mine(&self.problem, plugin, true).or_else(|| mine(&self.said, plugin, false))
    }

    /// The line to draw under the list, which is everything that was about the
    /// registry rather than about one plugin.
    fn loose(&self) -> Option<(&str, bool)> {
        match (&self.problem, &self.said) {
            (Some((None, problem)), _) => Some((problem.as_str(), true)),
            (None, Some((None, said))) => Some((said.as_str(), false)),
            _ => None,
        }
    }

    /// The pictures out of `plugin`'s module, if that is the module somebody
    /// last looked inside.
    fn looked(&self, plugin: &PluginId) -> Option<&Looked> {
        self.looked_inside
            .as_ref()
            .filter(|looked| looked.plugin == *plugin)
    }
}

/// Which plugin the card is about, worked out the way the section works it
/// out.
///
/// The one answer, in one place, because the section and the two buttons on it
/// have to agree: the list is filtered by the field above it, and a handler
/// that resolved the selection against everything the registry has would act
/// on whatever happens to be first in *that* list — a card about one plugin
/// and an Install that fetched another.
pub(super) fn chosen(
    workspace: &Workspace,
    state: &Rc<StoreState>,
    offers: &[Offer],
) -> Option<Offer> {
    let matching = matching(workspace, offers);
    let id = state.showing(&matching)?;
    matching.into_iter().find(|offer| offer.id == id)
}

/// Everything the field has not filtered out.
fn matching(workspace: &Workspace, offers: &[Offer]) -> Vec<Offer> {
    let (_, input) = workspace.field(SECTION, FIELD);
    let query = Query::new(input.editor().text());
    offers
        .iter()
        .filter(|offer| matches(offer, &query))
        .cloned()
        .collect()
}

/// The section: the list in the sidebar, and the card beside it.
pub(super) fn render(
    workspace: &Workspace,
    app: &AppContext,
    state: &Rc<StoreState>,
) -> (Box<dyn Element>, Box<dyn Element>) {
    let ui = workspace.fonts().ui;
    let Some(model) = state.model() else {
        return (Empty::new().finish(), Empty::new().finish());
    };

    let known = model.read(app, |model, _| Known {
        heard: model.heard(),
        looking: model.looking(),
        fetched: model.fetched(),
        problem: model
            .problem()
            .map(|(about, why)| (about.cloned(), why.to_owned())),
        said: model
            .said()
            .map(|(about, said)| (about.cloned(), said.to_owned())),
        icons: model.icons().clone(),
        looking_inside: model.looking_inside().cloned(),
        looked_inside: model.looked_inside().map(|looked| Looked {
            plugin: looked.plugin.clone(),
            fetched: !looked.bytes.is_empty(),
            pictures: looked.pictures.clone(),
        }),
    });

    let matching = matching(workspace, &known.heard.offers);
    let selected = state.showing(&matching);
    let list = list(workspace, &known, &matching, selected.as_ref(), ui);

    let Some(showing) = selected
        .as_ref()
        .and_then(|id| matching.iter().find(|offer| offer.id == *id))
    else {
        // In the frame every other card is in, so the half of the window that
        // has nothing to show is still a page with a title rather than a
        // paragraph floating in the middle of it.
        return (
            list,
            section::content(
                "Store",
                None,
                nothing_chosen(&known, ui),
                workspace.settings_page().scroll_named(CARD_SCROLL),
                ui,
            ),
        );
    };

    // The face beside the name, for a plugin that has one. Nothing for one
    // that does not, for the reason the Plugins page draws nothing: a title
    // indented past an empty box would be a title saying a picture was
    // missing.
    let mark = icon(workspace, &known, &showing.id)
        .map(|icon| widgets::picture_box(Some(icon), widgets::TITLE_MARK));

    (
        list,
        section::content(
            &showing.name,
            mark,
            card(workspace, &known, showing, ui),
            workspace.settings_page().scroll_named(CARD_SCROLL),
            ui,
        ),
    )
}

/// The face the list has for `plugin`, or the one inside the module already
/// on this machine.
///
/// The list's first: it is the face of the version the registry offers,
/// which is what a store is showing. The installed module's is the answer
/// for a plugin the registry lists with no face yet — a version published
/// before there were pictures — running from a build that has one.
fn icon<'a>(
    workspace: &'a Workspace,
    known: &'a Known,
    plugin: &PluginId,
) -> Option<&'a Arc<Bitmap>> {
    known.icons.get(plugin.as_str()).or_else(|| {
        workspace
            .host()
            .pictures_of(plugin)
            .and_then(|pictures| pictures.icon.as_ref())
    })
}

/// The sidebar half.
fn list(
    workspace: &Workspace,
    known: &Known,
    matching: &[Offer],
    selected: Option<&PluginId>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let settings = workspace.settings_page();
    let (index, input) = workspace.field(SECTION, FIELD);

    let field = TextField::new(
        input,
        workspace.clipboard().clone(),
        workspace.fonts(),
        settings.control(named("store.search")),
        PLACEHOLDER,
    )
    .with_icon(Lucide::Search)
    .with_focus(WorkspaceAction::Settings(SettingsAction::FocusField(Some(
        index,
    ))))
    .finish();

    let body = if matching.is_empty() {
        Container::new(
            Text::new(nothing_to_show(known), ui, widgets::LABEL_SIZE)
                .with_color(theme().text_muted)
                .finish(),
        )
        .with_uniform_padding(8.)
        .finish()
    } else {
        let mut rows = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        for offer in matching {
            rows.add_child(row(
                workspace,
                known,
                offer,
                selected == Some(&offer.id),
                ui,
            ));
        }
        rows.finish()
    };

    section::sidebar(
        field,
        body,
        settings.scroll_named(LIST_SCROLL),
        Some(footer(workspace, known, ui)),
    )
}

/// One row: its face, what it is called, and one word about where it stands.
///
/// [`section::row`]'s, which is the Plugins list's row, so the two lists are
/// one list to the eye. A press is the store's own `show` action run about
/// this plugin — the same gesture "Show in Store" on the plugin's card makes
/// — so choosing a row and being sent to it are one path.
fn row(
    workspace: &Workspace,
    known: &Known,
    offer: &Offer,
    selected: bool,
    ui: FamilyId,
) -> Box<dyn Element> {
    let state = workspace
        .settings_page()
        .control(named(&format!("store.row.{}", offer.id)));
    let installed = installed_version(workspace, &offer.id);
    let command = workspace.run_about(&action("show"), offer.id.as_str());

    section::row(
        section::Row {
            label: offer.name.clone(),
            leading: Some(widgets::picture_box(
                icon(workspace, known, &offer.id),
                widgets::ROW_ICON,
            )),
            trailing: standing(
                offer,
                installed.as_deref(),
                workspace.withdrawn(&offer.id).is_some(),
            ),
            selected,
            // Lit for what is on this machine, whatever the pointer is doing,
            // for the reason the Plugins list lights what is running: the
            // list is scanned for what one already has.
            emphasis: match installed.is_some() {
                true => section::Emphasis::Lit,
                false => section::Emphasis::Dim,
            },
            state,
            command,
        },
        ui,
    )
}

/// The card: everything worth reading before pressing Install.
fn card(workspace: &Workspace, known: &Known, offer: &Offer, ui: FamilyId) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(facts(workspace, offer, ui))
        .with_child(widgets::description(&offer.description, ui))
        .with_child(decision(workspace, known, offer, ui));

    // Between the decision and where it comes from: what it looks like is
    // the next thing a person deciding wants, and the one that was on the
    // plugin's own card a moment ago if they came from there.
    if let Some(category) = looks(workspace, known, offer, ui) {
        column.add_child(category);
    }

    if !offer.repository.is_empty() {
        column.add_child(headed(
            "Where it comes from",
            vec![
                widgets::note(&offer.repository, ui),
                widgets::note(
                    "The registry builds every artifact from that source, and the terminal \
                     checks what arrived against what the list promised.",
                    ui,
                ),
            ],
            ui,
        ));
    }

    column.finish()
}

/// The line under the name: who owns it, which version, and its licence.
fn facts(workspace: &Workspace, offer: &Offer, ui: FamilyId) -> Box<dyn Element> {
    let mut parts = vec![offer.id.to_string()];
    if let Some(installed) = installed_version(workspace, &offer.id) {
        parts.push(format!("{installed} installed"));
    }
    if let Some(release) = &offer.release {
        parts.push(format!("{} in the registry", release.version));
    }
    if !offer.license.is_empty() {
        parts.push(offer.license.clone());
    }

    // No state to lead with: a plugin in the registry is not running or
    // stopped, it is offered. The plugin's own card says the rest.
    widgets::facts(None, &parts.join(" \u{b7} "), ui)
}

/// The box the one decision on this card is made in: what the plugin will ask
/// to be allowed to do, and Install directly under it.
///
/// One box rather than a box and two categories, for the reason the plugin's
/// own card keeps its terms and its Allow in one rectangle: a person cannot
/// reach Install without their eye crossing the list, and the hollow dots are
/// the ones they will meet again on the plugin's card once it has arrived.
fn decision(workspace: &Workspace, known: &Known, offer: &Offer, ui: FamilyId) -> Box<dyn Element> {
    let installed = installed_version(workspace, &offer.id);
    let decided = decided(
        offer,
        installed.as_deref(),
        workspace.withdrawn(&offer.id),
        known.heard.busy(&offer.id),
    );

    // Install is about whatever the card is about, resolved the way the
    // card was; an update is about this plugin by name, which is the same
    // action the plugin's own card runs. Either way the press names what it
    // fetches.
    let command = match decided.press {
        Press::Nothing => None,
        Press::Install => workspace
            .host()
            .action(&action("install"))
            .map(WorkspaceAction::Run),
        Press::Update => workspace.run_about(&action("update"), offer.id.as_str()),
    };
    let live = command.is_some();

    // What the plugin will ask for, above the button that fetches it. Nothing
    // at all for a plugin this build cannot run: there is no release to read
    // the asks off, and a heading over nothing would be a question with no
    // terms.
    let question = offer
        .release
        .as_ref()
        .map(|_| ("What it will ask to be allowed to do", Tone::Plain));
    let body: Vec<Box<dyn Element>> = match &offer.release {
        // Said rather than left out: a plugin that asks for nothing is the
        // strongest thing this card can tell somebody, and an empty list says
        // it to nobody.
        Some(release) if release.asks.is_empty() => vec![widgets::note_text(
            "Nothing. It draws what it draws and reaches none of this machine.",
            ui,
        )],
        Some(release) => release
            .asks
            .iter()
            .map(|ask| widgets::item(ask, Mark::Open, ui))
            .collect(),
        None => Vec::new(),
    };

    let mut foot = vec![widgets::answer_row(
        decided.label,
        live,
        widgets::text_button(
            decided.button,
            command,
            workspace
                .settings_page()
                .control(named(&format!("store.install.{}", offer.id))),
            ui,
        ),
        ui,
    )];

    // Under the install rather than in a box of its own: it is the same
    // decision, answered the other way, and a person looking for it is
    // looking at this box. Asked of the directory rather than of the
    // version above, as the plugin's own card asks: a module running from
    // where it was built is not one a Remove could take off the machine.
    if workspace.is_installed(&offer.id) {
        foot.push(widgets::answer_row(
            "On this machine",
            true,
            widgets::text_button(
                "Remove",
                workspace
                    .host()
                    .action(&action("remove"))
                    .map(WorkspaceAction::Run),
                workspace
                    .settings_page()
                    .control(named(&format!("store.remove.{}", offer.id))),
                ui,
            ),
            ui,
        ));
    }

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(widgets::asked(question, body, foot, ui));

    // Under the box, as the mechanism is under the box on the plugin's own
    // card: it explains the decision and is not part of it. It used to be a
    // heading over one sentence after the terms — and the terms came after
    // the button, so Install sat above the list it is the answer to.
    let mut prose = false;
    if offer.release.is_some() {
        column.add_child(widgets::footnote(
            "Installing is not allowing: a plugin that has just arrived may do nothing at all. \
             What it asks for is answered on its own card in Plugins, after you have read it \
             there.",
            Tone::Plain,
            ui,
        ));
        prose = true;
    }

    // Only what was said about *this* plugin. A line drawn on whatever card
    // happens to be showing is a sentence about the wrong thing, which on a
    // page about installing is worse than no sentence at all.
    if let Some((line, wrong)) = known.about(&offer.id) {
        let tone = match wrong {
            true => Tone::Warning,
            false => Tone::Plain,
        };
        column.add_child(widgets::footnote(
            &format!("{} {line}", offer.name),
            tone,
            ui,
        ));
        prose = true;
    }

    // A box that ends in prose ends in the prose's own gap, which is what a
    // note before a rule gets on every settings page; a bare box keeps the gap
    // a row keeps before the rule.
    Container::new(column.finish())
        .with_margin_bottom(if prose { 0. } else { widgets::ROW_SPACING })
        .finish()
}

/// What pressing the card's one button does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Press {
    /// Nothing: the button is drawn dead, and its label says why.
    Nothing,
    /// Fetches whatever the card is about, for a plugin not on this machine.
    Install,
    /// Fetches the release offered in place of the version on this machine —
    /// newer, or the replacement for one taken back.
    Update,
}

/// The one button on the card, and the label beside it.
#[derive(Debug, PartialEq, Eq)]
struct Decided {
    /// What the row says, which is where the plugin stands.
    label: String,
    /// What the button says.
    button: String,
    /// What pressing it does.
    press: Press,
}

/// What the card's button and its label come to, for `offer` on a machine
/// holding `installed` — withdrawn for `withdrawn`, if it was — while the
/// store is `busy` about it.
///
/// Every answer goes through [`change`], which is what keeps the button and
/// the label from disagreeing: a person on a withdrawn 0.10.0 offered 0.9.0
/// is offered a replacement and not an update, and a registry that is
/// merely behind offers nothing.
fn decided(
    offer: &Offer,
    installed: Option<&str>,
    withdrawn: Option<&str>,
    busy: Option<Busy>,
) -> Decided {
    let (label, button, press) = match busy {
        Some(Busy::Downloading) => (
            String::from("Downloading"),
            String::from("Getting it\u{2026}"),
            Press::Nothing,
        ),
        Some(Busy::Waiting) => (
            String::from("Waiting"),
            String::from("Waiting\u{2026}"),
            Press::Nothing,
        ),
        None => match change(offer, installed, withdrawn.is_some()) {
            // Two reasons there is nothing, and the button says which: a
            // withdrawal is the registry's doing and a newer Crook would not
            // change it.
            Change::Nothing => match (&offer.withdrawn, &offer.newest_anywhere) {
                (Some(why), _) => (
                    format!("Taken back: {why}"),
                    String::from("Nothing offered"),
                    Press::Nothing,
                ),
                (None, Some(newest)) => (
                    format!("Built for plugin API {}", newest.abi),
                    String::from("Not for this build"),
                    Press::Nothing,
                ),
                (None, None) => (
                    String::from("Nothing built yet"),
                    String::from("Not for this build"),
                    Press::Nothing,
                ),
            },
            Change::Install(release) => (
                format!("Version {}", release.version),
                String::from("Install"),
                Press::Install,
            ),
            Change::Update(release) => (
                format!(
                    "{} installed, {} out",
                    installed.unwrap_or_default(),
                    release.version
                ),
                format!("Update to {}", release.version),
                Press::Update,
            ),
            Change::Replace(release) => (
                format!("Taken back: {}", withdrawn.unwrap_or_default()),
                format!("Install {}", release.version),
                Press::Update,
            ),
            Change::Current => (
                format!("Version {}", installed.unwrap_or_default()),
                String::from("Installed"),
                Press::Nothing,
            ),
        },
    };
    Decided {
        label,
        button,
        press,
    }
}

/// The word a row ends in, or nothing for a plugin that is simply there to
/// be installed.
fn standing(offer: &Offer, installed: Option<&str>, withdrawn: bool) -> Option<String> {
    match change(offer, installed, withdrawn) {
        // Taken back, and nothing in its place: what the row's card says,
        // and not "newer Crook", which would be a row saying this build is
        // the reason.
        Change::Nothing if offer.withdrawn.is_some() => Some(String::from("taken back")),
        Change::Nothing => Some(String::from("newer Crook")),
        Change::Current => Some(String::from("installed")),
        Change::Update(release) | Change::Replace(release) => Some(release.version),
        Change::Install(_) => None,
    }
}

/// What it looks like: the previews inside the module, opened by a press —
/// and fetched by one, for a plugin not on this machine.
///
/// Absent when neither the offered release nor the module already here
/// carries any. The row says how many there are; the button says "Show" when
/// the pictures are on this machine — in the installed module, or in the
/// bytes a previous look fetched — and "Fetch" when seeing them means
/// fetching the plugin, with the note under it saying exactly that, and how
/// much. While the pictures are on their way the button is dead and the room
/// each will take is drawn in the box fill, at the size the index or the
/// module said, so nothing under them moves when they land; once they have,
/// it is dead and says so.
///
/// The index's list of sizes is held to the rule the module's reader holds
/// a module to — at most [`MAX_PREVIEWS`] of them, each within
/// [`MAX_PREVIEW_EDGE`] — before a room is laid out for any of them. The
/// registry's own check refuses a row past it, and a list that arrived some
/// other way must not be a way of laying out three thousand rooms a frame.
///
/// [`MAX_PREVIEWS`]: crook_plugin_api::pictures::MAX_PREVIEWS
/// [`MAX_PREVIEW_EDGE`]: crook_plugin_api::pictures::MAX_PREVIEW_EDGE
fn looks(
    workspace: &Workspace,
    known: &Known,
    offer: &Offer,
    ui: FamilyId,
) -> Option<Box<dyn Element>> {
    use crook_plugin_api::pictures::{MAX_PREVIEW_EDGE, MAX_PREVIEWS};

    let carried = workspace
        .host()
        .pictures_of(&offer.id)
        .filter(|pictures| pictures.count() > 0)
        .map(|pictures| pictures.previews.as_slice());
    let sizes: Vec<(u32, u32, Option<&str>)> = match carried {
        Some(previews) => previews
            .iter()
            .map(|preview| (preview.width, preview.height, preview.caption.as_deref()))
            .collect(),
        None => offer
            .release
            .iter()
            .flat_map(|release| release.previews.iter())
            .filter(|size| {
                (1..=MAX_PREVIEW_EDGE).contains(&size.width)
                    && (1..=MAX_PREVIEW_EDGE).contains(&size.height)
            })
            .take(MAX_PREVIEWS)
            .map(|size| (size.width, size.height, None))
            .collect(),
    };
    if sizes.is_empty() {
        return None;
    }

    let looked = known.looked(&offer.id);
    let held = carried.is_some() || looked.is_some_and(|looked| looked.fetched);
    let opening = known.looking_inside.as_ref() == Some(&offer.id);
    let looking = match (opening, held, looked.is_some()) {
        (true, true, _) => widgets::Looking::Opening,
        (true, false, _) => widgets::Looking::Fetching,
        (false, _, true) => widgets::Looking::Shown,
        (false, true, false) => widgets::Looking::Show,
        (false, false, false) => widgets::Looking::Fetch,
    };

    // The verb the card's own button uses, so the note names a control that
    // is on the card: for a plugin already here the button says Update.
    let installed = installed_version(workspace, &offer.id);
    let fetching = change(
        offer,
        installed.as_deref(),
        workspace.withdrawn(&offer.id).is_some(),
    )
    .fetchable()
    .map(|release| release.version.clone());
    let mut note = None;
    if !held {
        // Said before the press and not after: this is the one button in
        // the store that downloads something without installing it, and
        // what it costs — the plugin itself — is the thing to know before
        // pressing.
        let size = offer
            .release
            .as_ref()
            .map(|release| release.bytes)
            .filter(|bytes| *bytes > 0)
            .map(|bytes| format!("{} KB, ", (bytes / 1000).max(1)))
            .unwrap_or_default();
        let (verb, doing) = match fetching {
            Some(_) => ("Update", "Updating"),
            None => ("Install", "Installing"),
        };
        note = Some(format!(
            "Seeing them is fetching the plugin itself \u{2014} {size}the same file {verb} \
             fetches \u{2014} and nothing about you goes with it. {doing} afterwards needs no \
             second download."
        ));
    } else if let (Some(_), Some(offered)) = (carried, fetching) {
        // The pictures are the installed version's and the card is offering
        // another: said, so that somebody deciding on the update knows
        // which version they are looking at.
        let installed = installed.unwrap_or_default();
        let listed = offer
            .release
            .as_ref()
            .map_or(0, |release| release.previews.len());
        note = Some(match listed {
            0 => format!("The pictures of {installed}, the version on this machine."),
            1 => format!(
                "The pictures of {installed}, the version on this machine; {offered} lists 1."
            ),
            listed => format!(
                "The pictures of {installed}, the version on this machine; {offered} lists \
                 {listed}."
            ),
        });
    }

    Some(widgets::pictures_category(
        widgets::Looks {
            count: sizes.len(),
            looking,
            command: workspace.run_about(&action("look-inside"), offer.id.as_str()),
            control: workspace
                .settings_page()
                .control(named(&format!("store.pictures.{}", offer.id))),
            shown: looked.map(|looked| looked.pictures.as_slice()),
            rooms: match opening {
                true => &sizes,
                false => &[],
            },
            note: note.as_deref(),
        },
        ui,
    ))
}

/// A heading with lines under it, in the settings' own shape.
fn headed(title: &str, rows: Vec<widgets::Entry>, ui: FamilyId) -> Box<dyn Element> {
    widgets::category_element(
        title,
        false,
        rows.into_iter().map(|row| row.element).collect(),
        ui,
    )
}

/// The foot of the list: the one request this application makes of its own —
/// and, when the registry is ahead of something installed, the one that
/// takes every update at once.
fn footer(workspace: &Workspace, known: &Known, ui: FamilyId) -> Box<dyn Element> {
    let label = match known.looking {
        true => "Looking\u{2026}",
        false => "Look for plugins",
    };
    let command = match known.looking {
        true => None,
        false => workspace
            .host()
            .action(&action("look"))
            .map(WorkspaceAction::Run),
    };

    // What went wrong with the *registry* belongs here rather than on a card:
    // it is not about any one plugin, and the button it is about is the one
    // under it.
    let (line, color) = match known.loose() {
        Some((line, true)) => (line.to_owned(), theme().usage_critical),
        Some((line, false)) => (line.to_owned(), theme().text_muted),
        None => (age(known), theme().text_muted),
    };

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                Paragraph::new(line, ui, widgets::DESCRIPTION_SIZE)
                    .with_color(color)
                    .with_line_height_ratio(1.4)
                    .finish(),
            )
            .with_uniform_padding(8.)
            .finish(),
        )
        .with_child(widgets::text_button(
            label,
            command,
            workspace.settings_page().control(named("store.look")),
            ui,
        ));

    // Under the look, because it is what a look is for: the count is of
    // installed plugins the list is ahead of, and the button takes them one
    // module at a time. Dead while every one of them is already on its way.
    let updates = workspace.updates();
    if !updates.is_empty() {
        let waiting = updates
            .iter()
            .all(|(plugin, _)| known.heard.busy(plugin).is_some());
        let command = match waiting {
            true => None,
            false => workspace
                .host()
                .action(&action("update-all"))
                .map(WorkspaceAction::Run),
        };
        column.add_child(widgets::update_all_footer(
            updates.len(),
            command,
            workspace.settings_page().control(named("store.update-all")),
            ui,
        ));
    }

    column.finish()
}

/// The card drawn when the list has nothing in it.
///
/// Not the sentence the list is showing: two copies of one line, side by side,
/// read as a mistake. This one says what the *page* is for.
fn nothing_chosen(known: &Known, ui: FamilyId) -> Box<dyn Element> {
    let line = match (known.heard.offers.is_empty(), known.looking) {
        (_, true) => "Reading the list the registry publishes.",
        (true, false) => {
            "Every plugin the registry has built, with what each one will ask to be allowed to \
             do. Nothing is fetched until you press the button under the list, and nothing about \
             you goes with the request."
        }
        (false, false) => "Nothing in the registry matches what is in the field.",
    };

    Container::new(
        Paragraph::new(line.to_owned(), ui, widgets::LABEL_SIZE)
            .with_color(theme().text_muted)
            .with_line_height_ratio(1.5)
            .finish(),
    )
    .with_uniform_padding(24.)
    .finish()
}

/// Why the list is empty, which is not always the same reason.
fn nothing_to_show(known: &Known) -> &'static str {
    match (known.heard.offers.is_empty(), known.looking) {
        (_, true) => "Looking\u{2026}",
        (true, false) => {
            "Nothing here yet. \"Look for plugins\" reads the registry's list \u{2014} one file, \
             and nothing about you goes with the request."
        }
        (false, false) => "No plugin in the registry matches that.",
    }
}

/// How old the answer on screen is.
fn age(known: &Known) -> String {
    if known.looking {
        return String::from("Reading the registry\u{2026}");
    }
    let Some(fetched) = known.fetched else {
        return String::from("Never looked");
    };

    match fetched.elapsed() {
        Ok(elapsed) => format!("Checked {}", ago(elapsed.as_secs())),
        // A clock that went backwards, which is a machine that changed time
        // zone or synchronised. Not worth a sentence about clocks.
        Err(_) => String::from("Checked recently"),
    }
}

/// "just now", "6 minutes ago", "3 days ago".
pub(super) fn ago(seconds: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    let (count, unit) = match seconds {
        0..MINUTE => return String::from("just now"),
        seconds if seconds < HOUR => (seconds / MINUTE, "minute"),
        seconds if seconds < DAY => (seconds / HOUR, "hour"),
        seconds => (seconds / DAY, "day"),
    };

    match count {
        1 => format!("1 {unit} ago"),
        count => format!("{count} {unit}s ago"),
    }
}

/// Which version of this plugin is on this machine, if any.
pub(super) fn installed_version(workspace: &Workspace, plugin: &PluginId) -> Option<String> {
    workspace
        .host()
        .available()
        .iter()
        .find(|manifest| manifest.id == *plugin)
        .map(|manifest| manifest.version.to_owned())
}

/// Whether a query is looking for this plugin.
fn matches(offer: &Offer, query: &Query) -> bool {
    query.matches(
        &Words::new(&offer.name)
            .with_description(&offer.description)
            .with_keywords(&[offer.id.as_str(), "plugin", "store", "registry"]),
        &[],
    )
}

#[cfg(test)]
#[path = "section_tests.rs"]
mod tests;
