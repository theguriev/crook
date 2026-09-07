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

use std::rc::Rc;
use std::time::SystemTime;

use crookui_core::elements::{Padding, Paragraph};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crook_plugin::PluginId;

use crate::theme::theme;
use crate::workspace::section;
use crate::workspace::settings_page::search::{Query, Words};
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{SettingsAction, TextField, Workspace, WorkspaceAction};

use super::index::Offer;
use super::state::StoreState;
use super::{FIELD, SECTION, action};

/// What the field says while nothing has been typed.
const PLACEHOLDER: &str = "Search the registry";

/// Where the list has been scrolled to.
const LIST_SCROLL: &str = "store.list";

/// Where the card has been scrolled to.
const CARD_SCROLL: &str = "store.card";

/// The corner and the inset of the box a control sits in.
///
/// The Plugins page's, because the two cards are read one after the other and
/// a control that changed shape between them would read as a different kind of
/// control.
const CONTROL_RADIUS: f32 = 8.;
/// See [`CONTROL_RADIUS`].
const CONTROL_PADDING: Padding = Padding {
    top: 10.,
    bottom: 10.,
    left: 12.,
    right: 12.,
};

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
    problem: Option<String>,
    said: Option<String>,
    downloading: Option<PluginId>,
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
        problem: model.problem().map(str::to_owned),
        said: model.said().map(str::to_owned),
        downloading: model.downloading().cloned(),
    });

    let (_, input) = workspace.field(SECTION, FIELD);
    let query = Query::new(input.editor().text());
    let matching: Vec<Offer> = known
        .offers
        .iter()
        .filter(|offer| matches(offer, &query))
        .cloned()
        .collect();

    let selected = state.showing(&matching);
    let list = list(workspace, state, &known, &matching, selected.as_ref(), ui);

    let Some(showing) = selected
        .as_ref()
        .and_then(|id| matching.iter().find(|offer| offer.id == *id))
    else {
        return (list, nothing_chosen(&known, ui));
    };

    (
        list,
        section::content(
            &showing.name,
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
            .with_child(
                Text::new(label.clone(), ui, widgets::LABEL_SIZE)
                    .with_color(if installed {
                        theme().text_primary
                    } else {
                        theme().text_muted
                    })
                    .finish(),
            )
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
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
        .with_child(description(&offer.description, ui))
        .with_child(button(workspace, known, offer, ui));

    if let Some(release) = &offer.release {
        let asks: Vec<widgets::Entry> = match release.asks.is_empty() {
            // Said rather than left out: a plugin that asks for nothing is the
            // strongest thing this card can tell somebody, and an absent
            // heading says it to nobody.
            true => vec![widgets::note(
                "Nothing. It draws what it draws and reaches none of this machine.",
                ui,
            )],
            false => release
                .asks
                .iter()
                .map(|ask| widgets::note(ask, ui))
                .collect(),
        };
        column.add_child(headed("What it will ask to be allowed to do", asks, ui));
        column.add_child(headed(
            "Installing is not allowing",
            vec![widgets::note(
                "A plugin that has just arrived may do nothing at all. What it asks for is \
                 answered on its own card in Plugins, after you have read it there.",
                ui,
            )],
            ui,
        ));
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

    Container::new(
        Text::new(parts.join(" \u{b7} "), ui, widgets::DESCRIPTION_SIZE)
            .with_color(theme().text_muted)
            .finish(),
    )
    .with_margin_bottom(14.)
    .finish()
}

/// What the plugin says it is for.
fn description(text: &str, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(text.to_owned(), ui, widgets::LABEL_SIZE)
            .with_color(theme().text_primary)
            .with_line_height_ratio(1.5)
            .finish(),
    )
    .with_margin_bottom(18.)
    .finish()
}

/// The box the one decision on this card sits in.
fn button(workspace: &Workspace, known: &Known, offer: &Offer, ui: FamilyId) -> Box<dyn Element> {
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

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(answer(
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
        ));

    // Beside the install rather than under a heading of its own: it is the
    // same decision, answered the other way, and a person looking for it is
    // looking at this box.
    if installed.is_some() {
        column.add_child(
            Container::new(answer(
                String::from("On this machine"),
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
            ))
            .with_margin_top(8.)
            .finish(),
        );
    }

    if let Some(said) = &known.said {
        column.add_child(said_line(said, theme().text_muted, ui));
    }
    if let Some(problem) = &known.problem {
        column.add_child(said_line(problem, theme().usage_critical, ui));
    }

    Container::new(column.finish())
        .with_margin_bottom(18.)
        .finish()
}

/// A line under the controls, in whatever colour it deserves.
fn said_line(text: &str, color: Color, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(text.to_owned(), ui, widgets::DESCRIPTION_SIZE)
            .with_color(color)
            .with_line_height_ratio(1.4)
            .finish(),
    )
    .with_margin_top(10.)
    .finish()
}

/// The box a control sits in, with the label that says what it answers.
fn answer(label: String, live: bool, control: Box<dyn Element>, ui: FamilyId) -> Box<dyn Element> {
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

    Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                Text::new(age(known), ui, widgets::DESCRIPTION_SIZE)
                    .with_color(theme().text_muted)
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
fn nothing_chosen(known: &Known, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Paragraph::new(nothing_to_show(known).to_owned(), ui, widgets::LABEL_SIZE)
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
    match (&offer.release, installed) {
        (None, _) => match &offer.newest_anywhere {
            Some(newest) => format!("Built for plugin API {}", newest.abi),
            None => String::from("Nothing built yet"),
        },
        (Some(release), Some(installed)) if installed == release.version => {
            format!("Version {installed}")
        }
        (Some(release), Some(installed)) => {
            format!("{installed} installed, {} out", release.version)
        }
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
