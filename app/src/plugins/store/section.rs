//! The Store: the list a plugin is chosen from, and the card it is installed
//! from.
//!
//! The same two halves every section of the sidebar has, in the same frames —
//! a field over a list, and a title over a card — because a plugin that has
//! not been installed yet is not a different kind of thing from one that has,
//! and a store that looked like a different application would be one.
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
//! "checked 6 days ago" under it rather than an empty page.

use std::cmp::Ordering;
use std::rc::Rc;
use std::time::SystemTime;

use crookui_core::elements::Paragraph;
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crook_plugin::PluginId;

use crate::theme::theme;
use crate::workspace::section;
use crate::workspace::settings_page::search::{Query, Words};
use crate::workspace::settings_page::widgets::{Mark, Tone};
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{SettingsAction, TextField, Workspace, WorkspaceAction};

use super::super::wasm::version;
use super::index::Offer;
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
    offers: Vec<Offer>,
    looking: bool,
    fetched: Option<SystemTime>,
    /// What went wrong, and which plugin it was about — `None` for the
    /// registry itself, which is nobody's row.
    problem: Option<(Option<PluginId>, String)>,
    /// The same for what went right.
    said: Option<(Option<PluginId>, String)>,
    downloading: Option<PluginId>,
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
        offers: model.offers(),
        looking: model.looking(),
        fetched: model.fetched(),
        problem: model
            .problem()
            .map(|(about, why)| (about.cloned(), why.to_owned())),
        said: model
            .said()
            .map(|(about, said)| (about.cloned(), said.to_owned())),
        downloading: model.downloading().cloned(),
    });

    let matching = matching(workspace, &known.offers);
    let selected = state.showing(&matching);
    let list = list(workspace, state, &known, &matching, selected.as_ref(), ui);

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

    (
        list,
        section::content(
            &showing.name,
            None,
            card(workspace, &known, showing, ui),
            workspace.settings_page().scroll_named(CARD_SCROLL),
            ui,
        ),
    )
}

/// The sidebar half.
fn list(
    workspace: &Workspace,
    state: &Rc<StoreState>,
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
                state,
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

/// One row: what it is called, and one word about where it stands.
///
/// Its own rather than [`section::row`], because a row here has a second thing
/// to say and because what a press does is *choose*, which is not a named
/// action: there is one action per plugin on the Plugins page because the set
/// is known when that page loads, and the set here arrives over a network. A
/// selection is not a command either way — a palette row that silently changed
/// what a card is about would be a palette row nobody could see the effect of.
fn row(
    workspace: &Workspace,
    state: &Rc<StoreState>,
    offer: &Offer,
    selected: bool,
    ui: FamilyId,
) -> Box<dyn Element> {
    let mouse = workspace
        .settings_page()
        .control(named(&format!("store.row.{}", offer.id)));
    let label = offer.name.clone();
    let standing = standing(workspace, offer);
    let installed = installed_version(workspace, &offer.id).is_some();

    let state = state.clone();
    let key = offer.id.to_string();
    Hoverable::new(mouse, move |mouse| {
        let background = if selected {
            theme().overlay_3
        } else if mouse.is_hovered() {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        };

        let line = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            // The name takes what is left rather than what it wants: a
            // registry is a list of names strangers chose, and one long enough
            // would otherwise push the word after it off the row and out of
            // the panel.
            .with_child(
                Expanded::new(
                    1.,
                    Text::new(label.clone(), ui, widgets::LABEL_SIZE)
                        .with_color(if installed {
                            theme().text_primary
                        } else {
                            theme().text_muted
                        })
                        .finish(),
                )
                .finish(),
            )
            .with_child(
                Text::new(standing.clone(), ui, widgets::DESCRIPTION_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            )
            .finish();

        Container::new(line)
            .with_padding(section::ROW_PADDING)
            .with_background_color(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(section::ROW_RADIUS)))
            .with_margin_bottom(section::ROW_GAP)
            .finish()
    })
    .on_click(move |_, ctx, _| {
        state.select(&key);
        ctx.notify();
    })
    .finish()
}

/// The card: everything worth reading before pressing Install.
fn card(workspace: &Workspace, known: &Known, offer: &Offer, ui: FamilyId) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(facts(workspace, offer, ui))
        .with_child(widgets::description(&offer.description, ui))
        .with_child(decision(workspace, known, offer, ui));

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
    let busy = known.downloading.as_ref() == Some(&offer.id);
    let installed = installed_version(workspace, &offer.id);
    let (label, live): (String, bool) = match (&offer.release, &installed) {
        _ if busy => (String::from("Getting it\u{2026}"), false),
        (None, _) => (String::from("Not for this build"), false),
        (Some(release), Some(installed)) if *installed == release.version => {
            (String::from("Installed"), false)
        }
        (Some(release), Some(_)) => (format!("Update to {}", release.version), true),
        (Some(_), None) => (String::from("Install"), true),
    };

    let command = match live {
        true => workspace
            .host()
            .action(&action("install"))
            .map(WorkspaceAction::Run),
        false => None,
    };

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
        standing_label(offer, installed.as_deref(), busy),
        live,
        widgets::text_button(
            label,
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
    // looking at this box.
    if installed.is_some() {
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

/// A heading with lines under it, in the settings' own shape.
fn headed(title: &str, rows: Vec<widgets::Entry>, ui: FamilyId) -> Box<dyn Element> {
    widgets::category_element(
        title,
        false,
        rows.into_iter().map(|row| row.element).collect(),
        ui,
    )
}

/// The foot of the list: the one request this application makes of its own.
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

    Flex::column()
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
        ))
        .finish()
}

/// The card drawn when the list has nothing in it.
///
/// Not the sentence the list is showing: two copies of one line, side by side,
/// read as a mistake. This one says what the *page* is for.
fn nothing_chosen(known: &Known, ui: FamilyId) -> Box<dyn Element> {
    let line = match (known.offers.is_empty(), known.looking) {
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
    match (known.offers.is_empty(), known.looking) {
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

/// The word a row ends in.
fn standing(workspace: &Workspace, offer: &Offer) -> String {
    let installed = installed_version(workspace, &offer.id);
    match (&offer.release, installed) {
        (None, _) => String::from("newer Crook"),
        (Some(release), Some(installed)) if installed == release.version => {
            String::from("installed")
        }
        (Some(release), Some(_)) => release.version.clone(),
        (Some(_), None) => String::new(),
    }
}

/// The label beside the button, which says what the button answers.
fn standing_label(offer: &Offer, installed: Option<&str>, busy: bool) -> String {
    if busy {
        return String::from("Downloading");
    }
    if let Some(why) = &offer.withdrawn {
        return format!("Taken back: {why}");
    }
    match (&offer.release, installed) {
        (None, _) => match &offer.newest_anywhere {
            Some(newest) => format!("Built for plugin API {}", newest.abi),
            None => String::from("Nothing built yet"),
        },
        (Some(release), Some(installed)) => match version::compare(&release.version, installed) {
            Ordering::Greater => format!("{installed} installed, {} out", release.version),
            _ => format!("Version {installed}"),
        },
        (Some(release), None) => format!("Version {}", release.version),
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
