//! The list of plugins, and the field that narrows it.
//!
//! The frame is [`section::sidebar`]'s, which is the same frame the settings
//! rail is drawn in; what is here is only what goes in it.
//!
//! # Its field is the workspace's
//!
//! A section may bring a field, but it does not *own* one: `sync_input_keys`
//! has to be able to take the keyboard away from every field there is, and it
//! can only reach what the workspace holds. So the input comes from
//! [`Workspace::field`] under a key of this page's own, and pressing it
//! dispatches the action that moves the keyboard to it.
//!
//! The rule is the one a person can predict with no focus ring to look at:
//! **the last field pressed is the one being typed into.**

use crookui_core::elements::Paragraph;
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId};

use crate::plugins::store;
use crate::plugins::store::index::change;
use crate::theme::theme;
use crate::workspace::section;
use crate::workspace::settings_page::focus::Place;
use crate::workspace::settings_page::search::{Query, Words};
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{SettingsAction, TextField, Workspace, WorkspaceAction};

use super::state::PluginsState;
use super::{about, tier_words};

/// What the field says while nothing has been typed.
const PLACEHOLDER: &str = "Search plugins";

/// Where the list has been scrolled to.
const LIST_SCROLL: &str = "plugins.list";

/// The section this list is the sidebar of, and the name its field is
/// registered under.
pub(super) const SECTION: &str = "crook/plugins/section";
/// See [`SECTION`].
const FIELD: &str = "search";

/// Every plugin the field has not filtered out, in the host's own order.
pub(super) fn matching(workspace: &Workspace) -> Vec<&'static Manifest> {
    let (_, input) = workspace.field(SECTION, FIELD);
    let query = Query::new(input.editor().text());
    workspace
        .host()
        .available()
        .iter()
        .filter(|manifest| matches(manifest, &query))
        .copied()
        .collect()
}

/// Whether anything has been typed into the field.
pub(super) fn is_searching(workspace: &Workspace) -> bool {
    let (_, input) = workspace.field(SECTION, FIELD);
    !Query::new(input.editor().text()).is_empty()
}

/// What the list holds, in the two groups it is drawn in.
///
/// Installed first, because that is what a person opens this page to see:
/// the plugins they chose, and whether each is running. The modules the
/// binary is built from are eighteen rows of the same answer — on, built in —
/// and listed first they pushed the one plugin somebody installed to the foot
/// of the list, under everything they did not choose.
pub(super) struct Groups<'a> {
    /// The plugins that are not part of the binary, in the host's order.
    pub(super) installed: &'a [&'static Manifest],
    /// The ones that are, in the host's order.
    pub(super) built_in: &'a [&'static Manifest],
    /// Whether the built-in ones are drawn: the fold is open, or a search
    /// is looking through them.
    pub(super) open: bool,
    /// Whether something has been typed into the field.
    pub(super) searching: bool,
}

/// The heading over the plugins somebody installed.
const INSTALLED: &str = "Installed";

/// What the fold over the built-in plugins is called.
const BUILT_IN: &str = "Built in";

/// What the list says under [`INSTALLED`] when nothing is.
const NOTHING_INSTALLED: &str = "Nothing installed yet. The Store lists plugins to add.";

/// The list, with its field above it.
pub(super) fn render(
    workspace: &Workspace,
    groups: Groups<'_>,
    showing: Option<&PluginId>,
    state: &PluginsState,
) -> Box<dyn Element> {
    let settings = workspace.settings_page();
    let ui = workspace.fonts().ui;
    let (index, input) = workspace.field(SECTION, FIELD);
    let host = workspace.host();
    let focus = settings.focus_of(SECTION);

    let field = TextField::new(
        input,
        workspace.clipboard().clone(),
        workspace.fonts(),
        settings.control(named("plugins.search")),
        PLACEHOLDER,
    )
    .with_icon(Lucide::Search)
    // Drawn as a box without the keys while the ring is on the list or the
    // card, though it keeps them for text, as the settings' box does: see
    // `focus::key_for`.
    .with_window_focused(workspace.is_window_focused() && focus.place() == Place::Search)
    .with_focus(WorkspaceAction::Settings(SettingsAction::FocusField(Some(
        index,
    ))))
    .with_edit(WorkspaceAction::Settings(SettingsAction::SearchEdited))
    .finish();

    let list = if groups.installed.is_empty() && groups.built_in.is_empty() {
        // Inset to the row's own padding rather than to the list's, so the
        // line starts where a row's label would have.
        Container::new(
            Text::new("No plugin matches that.", ui, widgets::LABEL_SIZE)
                .with_color(theme().text_muted)
                .finish(),
        )
        .with_uniform_padding(8.)
        .finish()
    } else {
        let mut rows = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        let plugin_row = |manifest: &&'static Manifest| {
            row(
                workspace,
                manifest,
                showing == Some(&manifest.id),
                host.is_loaded(&manifest.id),
                ui,
            )
        };

        // The heading, and the way to the Store when there is nothing under
        // it. Left out of a search that found only built-in plugins: an
        // "Installed" with nothing under it there would read as the search
        // saying nothing is installed.
        if !groups.installed.is_empty() || !groups.searching {
            rows.add_child(section::list_heading(INSTALLED, true, ui));
        }
        if groups.installed.is_empty() && !groups.searching {
            rows.add_child(section::list_note(NOTHING_INSTALLED, ui));
            rows.add_child(store_link(workspace, ui));
        }
        rows.add_children(groups.installed.iter().map(plugin_row));

        if !groups.built_in.is_empty() {
            let count = groups.built_in.len().to_string();
            // A search opens the group whatever the fold says, so the fold
            // is drawn as the heading it then is, with nothing to press: a
            // chevron that shut nothing while the field had text would be a
            // control that did not work.
            rows.add_child(match groups.searching {
                true => section::list_heading(&format!("{BUILT_IN} \u{b7} {count}"), false, ui),
                false => built_in_fold(workspace, groups.open, count, ui),
            });
            if groups.open {
                rows.add_children(groups.built_in.iter().map(plugin_row));
            }
        }
        rows.finish()
    };

    // A footer only when there is something for it to say. The build line
    // under the settings rail says which build this is, and a second copy of
    // it under a list of what the build is made of would be the same sentence
    // twice; what this list has to say at its foot is what became of the
    // row it just lost, and how many of its rows the registry is ahead of
    // — the latter only while the Store, which is what fetches, is there to
    // ask.
    let mut footer = Vec::new();
    if let Some(sentence) = state.said_of_nobody() {
        footer.push(
            Container::new(
                Paragraph::new(sentence, ui, widgets::DESCRIPTION_SIZE)
                    .with_color(theme().text_muted)
                    .with_line_height_ratio(1.4)
                    .finish(),
            )
            .with_uniform_padding(8.)
            .finish(),
        );
    }
    footer.extend(updates_footer(workspace, ui));
    let footer = match footer.is_empty() {
        true => None,
        false => Some(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_children(footer)
                .finish(),
        ),
    };
    section::sidebar(
        &focus,
        field,
        list,
        settings.scroll_named(LIST_SCROLL),
        footer,
    )
}

/// The fold over the built-in plugins: a chevron, the group's name, and how
/// many it holds.
///
/// Shut until somebody opens it, for as long as the window is open — see
/// `PluginsState::built_in_open`. The count is there so the fold says what
/// it holds without being opened: "18" is the answer to "is there anything
/// under this", and it is the whole of what a shut group has to say.
fn built_in_fold(
    workspace: &Workspace,
    open: bool,
    count: String,
    ui: FamilyId,
) -> Box<dyn Element> {
    let press = workspace
        .host()
        .action(&about("fold"))
        .map(WorkspaceAction::Run);
    // Above the fold, not inside it: a gap between the last installed row
    // and the fold says the two are different groups, which a chevron
    // pressed against the row above it does not.
    Container::new(section::press_row(
        section::PressRow {
            label: BUILT_IN.to_owned(),
            icon: if open {
                Lucide::ChevronDown
            } else {
                Lucide::ChevronRight
            },
            trailing: Some(count),
            heading: true,
            state: workspace.settings_page().control(named("plugins.built-in")),
            press,
        },
        ui,
    ))
    .with_margin_top(FOLD_GAP)
    .finish()
}

/// The air above the fold, which is what sets the built-in group apart from
/// the installed one above it.
const FOLD_GAP: f32 = 8.;

/// The row that takes a person with nothing installed to the Store.
///
/// The section switch the sidebar's own Store button sends, so arriving there
/// is arriving the way the button would. Nothing when the Store has been
/// switched off: a link to a section that is not there would be a row that
/// did nothing, and the sentence above it still says where plugins come from.
fn store_link(workspace: &Workspace, ui: FamilyId) -> Box<dyn Element> {
    let Some(store) = workspace.host().sidebar_section_id(store::SECTION) else {
        return Empty::new().finish();
    };
    section::press_row(
        section::PressRow {
            label: "Open the Store".to_owned(),
            icon: Lucide::Store,
            trailing: None,
            heading: false,
            state: workspace
                .settings_page()
                .control(named("plugins.open-store")),
            press: Some(WorkspaceAction::ShowSection(Some(store))),
        },
        ui,
    )
}

/// The count of updates and the button that takes them all, or nothing.
///
/// Nothing rather than a dead button when the Store is switched off: the
/// count is worth a line only beside a way to act on it, and the Store's own
/// action is what acts. A plugin's card still says its version is behind.
/// Dead while every one of them is already on its way, as the Store's own
/// is: the two buttons answer one state.
fn updates_footer(workspace: &Workspace, ui: FamilyId) -> Option<Box<dyn Element>> {
    let update_all = workspace.host().action(&store::action("update-all"))?;
    let updates = workspace.updates();
    if updates.is_empty() {
        return None;
    }
    let waiting = updates
        .iter()
        .all(|(plugin, _)| workspace.heard().busy(plugin).is_some());
    Some(widgets::update_all_footer(
        updates.len(),
        (!waiting).then_some(WorkspaceAction::Run(update_all)),
        workspace
            .settings_page()
            .control(named("plugins.update-all")),
        ui,
    ))
}

/// Whether a query is looking for this plugin.
///
/// Against the same [`Words`] the settings rows are matched with, so "what
/// counts as finding something" is one rule in one place: the name, the line
/// under it, and the words that are not written on the row — which here are
/// the id and the tier, because somebody looking for a sandboxed plugin types
/// "sandboxed" and somebody who read a keymap types `crook/usage`.
fn matches(manifest: &Manifest, query: &Query) -> bool {
    let (tier, _) = tier_words(manifest.tier);
    query.matches(
        &Words::new(manifest.name)
            .with_description(manifest.description)
            .with_keywords(&[manifest.id.as_str(), tier, "plugin", "extension"]),
        &[],
    )
}

/// The word the card would head its permissions with, while it is one that
/// says the plugin is not doing what it could: "not allowed" for a plugin
/// nobody has answered, "partly allowed" for one asking for more than it was
/// given. Nothing for a plugin that asked for nothing, or has all it asked.
fn unanswered_word(workspace: &Workspace, manifest: &Manifest) -> Option<&'static str> {
    if manifest.capabilities.is_empty() {
        return None;
    }
    let granted = workspace.settings().granted_to(manifest.id.as_str());
    match super::stance(&super::wanted(manifest), granted) {
        super::Stance::Unanswered => Some("not allowed"),
        super::Stance::Escalated => Some("partly allowed"),
        super::Stance::Allowed => None,
    }
}

/// One plugin's row.
fn row(
    workspace: &Workspace,
    manifest: &'static Manifest,
    selected: bool,
    on: bool,
    ui: FamilyId,
) -> Box<dyn Element> {
    let host = workspace.host();
    let state = workspace
        .settings_page()
        .control(named(&format!("plugins.row.{}", manifest.id)));
    // One action for every row, run about this one. `None` only while this
    // page is not loaded, which is a page nobody can be looking at.
    let command = workspace.run_about(&about("show"), manifest.id.as_str());

    // The dot, then the face in a box the same size on every row, so the
    // names line up whatever a row wears: a picture from a module, a mark
    // for a plugin from the box, nothing for a module built before pictures.
    let face = host.face_of(&manifest.id);
    let leading = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(widgets::state_dot(on))
                .with_margin_right(section::LEADING_GAP)
                .finish(),
        )
        .with_child(widgets::picture_box(face, widgets::ROW_ICON))
        .finish();

    // The version an update would bring, and only that: a row is scanned for
    // what is running and, now, for what is behind. "installed" would be
    // every row, and "current" is what the absence of a word says. Only for
    // a plugin that is a file, which is what the card and the count at the
    // foot offer an update for.
    let update = workspace
        .heard()
        .offer(&manifest.id)
        .filter(|_| workspace.is_installed(&manifest.id))
        .and_then(|offer| {
            change(
                offer,
                Some(manifest.version),
                workspace.withdrawn(&manifest.id).is_some(),
            )
            .fetchable()
            .map(|release| release.version.clone())
        });
    // And before it, the answer that is still owed: a plugin nobody has
    // answered is running and doing nothing — refused everything it asked
    // for, with the refusal a line in the log — and a row that showed a dot
    // for "running" and nothing else read as a plugin at work. The card's
    // own word, so the list and the card agree, with the version after it
    // when there is one: neither is the kind of thing to hide behind the
    // other.
    let trailing = match (unanswered_word(workspace, manifest).filter(|_| on), update) {
        (Some(word), Some(version)) => Some(format!("{word} \u{b7} {version}")),
        (Some(word), None) => Some(word.to_owned()),
        (None, update) => update,
    };

    section::row(
        section::Row {
            label: manifest.name.to_owned(),
            // A filled dot for a plugin that is running and a hollow one for a
            // plugin that is not, then its face. The card draws the same dot
            // before the same word, from the same function.
            leading: Some(leading),
            trailing,
            selected,
            emphasis: if on {
                section::Emphasis::Lit
            } else {
                section::Emphasis::Dim
            },
            state,
            command,
        },
        ui,
    )
}
