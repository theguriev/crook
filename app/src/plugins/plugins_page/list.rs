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
const SECTION: &str = "crook/plugins/section";
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

/// The list, with its field above it.
pub(super) fn render(
    workspace: &Workspace,
    matching: &[&'static Manifest],
    showing: Option<&PluginId>,
    state: &PluginsState,
) -> Box<dyn Element> {
    let settings = workspace.settings_page();
    let ui = workspace.fonts().ui;
    let (index, input) = workspace.field(SECTION, FIELD);
    let host = workspace.host();

    let field = TextField::new(
        input,
        workspace.clipboard().clone(),
        workspace.fonts(),
        settings.control(named("plugins.search")),
        PLACEHOLDER,
    )
    .with_icon(Lucide::Search)
    .with_focus(WorkspaceAction::Settings(SettingsAction::FocusField(Some(
        index,
    ))))
    .finish();

    let list = if matching.is_empty() {
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
        for manifest in matching {
            rows.add_child(row(
                workspace,
                manifest,
                showing == Some(&manifest.id),
                host.is_loaded(&manifest.id),
                ui,
            ));
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
    section::sidebar(field, list, settings.scroll_named(LIST_SCROLL), footer)
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

    // The dot, then the icon in a box the same size on every row, so the
    // names line up whether or not a plugin brought a face. A native plugin
    // has none and its box is empty: "no picture" is the ordinary state of
    // most of this list, and not a thing to draw a symbol for.
    let icon = host
        .pictures_of(&manifest.id)
        .and_then(|pictures| pictures.icon.as_ref());
    let leading = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(widgets::state_dot(on))
                .with_margin_right(section::LEADING_GAP)
                .finish(),
        )
        .with_child(widgets::picture_box(icon, widgets::ROW_ICON))
        .finish();

    // The version an update would bring, and only that: a row is scanned for
    // what is running and, now, for what is behind. "installed" would be
    // every row, and "current" is what the absence of a word says. Only for
    // a plugin that is a file, which is what the card and the count at the
    // foot offer an update for.
    let trailing = workspace
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
