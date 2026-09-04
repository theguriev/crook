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

use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId};

use crate::theme::theme;
use crate::workspace::section;
use crate::workspace::settings_page::search::{Query, Words};
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{SettingsAction, TextField, Workspace, WorkspaceAction};

use super::{action, tier_words};

/// The dot that says whether a plugin is running.
const DOT: f32 = 6.;

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

    // No footer: the build line under the settings rail says which build this
    // is, and a second copy of it under a list of what the build is made of
    // would be the same sentence twice.
    section::sidebar(field, list, settings.scroll_named(LIST_SCROLL), None)
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
    let state = workspace
        .settings_page()
        .control(named(&format!("plugins.row.{}", manifest.id)));
    // `None` is unreachable: `ready` registers one per plugin. Cheaper to draw
    // an unclickable row than to prove unreachable from here.
    let command = workspace
        .host()
        .action(&action("show", &manifest.id))
        .map(WorkspaceAction::Run);

    section::row(
        section::Row {
            label: manifest.name.to_owned(),
            // A filled dot for a plugin that is running and a hollow one for a
            // plugin that is not, which is the whole of what a row has to say
            // about a plugin beyond its name.
            leading: Some(dot(on)),
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

/// Filled for a plugin that is running, hollow for one that is not.
fn dot(on: bool) -> Box<dyn Element> {
    let (background, border) = if on {
        (theme().usage_normal, theme().usage_normal)
    } else {
        (Color::TRANSPARENT, theme().text_muted)
    };

    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(background)
            .with_border(Border::all(1.).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
            .finish(),
    )
    .with_width(DOT)
    .with_height(DOT)
    .finish()
}
