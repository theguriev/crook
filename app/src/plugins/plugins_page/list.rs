//! The list of plugins, and the field that narrows it.
//!
//! # Its field is the settings page's
//!
//! A page may bring a field, but it does not *own* one: `sync_input_keys` has
//! to be able to take the keyboard away from every field on the pane, and it
//! can only reach what the settings page holds. So the input comes from
//! [`SettingsState::field`] under a key of this page's own, and pressing it
//! dispatches the action that moves the keyboard to it.
//!
//! Two fields on one surface is a thing the settings page did not have before,
//! and the rule is the one a person can predict with no focus ring to look at:
//! **the last one pressed is the one being typed into.**

use crookui_core::elements::Padding;
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crook_plugin::{Manifest, PluginId};

use crate::theme::theme;
use crate::workspace::settings_page::search::{Query, Words};
use crate::workspace::settings_page::{named, widgets};
use crate::workspace::{SettingsAction, TextField, Workspace, WorkspaceAction};

use super::{action, tier_words};

/// How wide the list is.
///
/// VS Code's extensions sidebar is 300 at a 13px body and holds a publisher, a
/// version, a rating and an install button. This holds a name and a dot, so it
/// is the width of the tabs panel Crook already has — two lists of about the
/// same weight should not be two different widths.
pub(super) const LIST_WIDTH: f32 = 248.;

/// The inset around it.
const PADDING: f32 = 12.;

/// One row's height, which is what makes scrolling to a row arithmetic.
const ROW_HEIGHT: f32 = 34.;

/// The dot that says whether a plugin is running.
const DOT: f32 = 6.;

/// What the field says while nothing has been typed.
const PLACEHOLDER: &str = "Search plugins";

/// The key this page's field is registered under.
const FIELD: &str = "crook/plugins:search";

/// Every plugin the field has not filtered out, in the host's own order.
pub(super) fn matching(workspace: &Workspace) -> Vec<&'static Manifest> {
    let (_, input) = workspace.settings_page().field(FIELD);
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
    let (index, input) = settings.field(FIELD);

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                TextField::new(
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
                .finish(),
            )
            .with_margin_bottom(10.)
            .finish(),
        );

    let host = workspace.host();

    if matching.is_empty() {
        column.add_child(
            Container::new(
                Text::new("No plugin matches that.", ui, widgets::LABEL_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            )
            .with_uniform_padding(8.)
            .finish(),
        );
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
        column.add_child(
            Expanded::new(
                1.,
                Scrollable::new(settings.scroll_named("plugins.list"), rows.finish())
                    .with_scrollbar(theme().overlay_3)
                    .finish(),
            )
            .finish(),
        );
    }

    ConstrainedBox::new(
        Container::new(column.finish())
            .with_border(Border::right(1.).with_border_color(theme().border))
            .with_padding(Padding {
                top: PADDING,
                bottom: PADDING,
                left: PADDING,
                right: PADDING,
            })
            .finish(),
    )
    .with_width(LIST_WIDTH)
    .finish()
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
    let command = workspace
        .host()
        .action(&action("show", &manifest.id))
        .map(WorkspaceAction::Run);

    let line = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        // A filled dot for a plugin that is running and a hollow one for a
        // plugin that is not, which is the whole of what a row has to say
        // about a plugin beyond its name.
        .with_child(dot(on))
        .with_child(
            Container::new(
                Text::new(manifest.name, ui, widgets::LABEL_SIZE)
                    .with_color(if on {
                        theme().text_primary
                    } else {
                        theme().text_muted
                    })
                    .finish(),
            )
            .with_margin_left(8.)
            .finish(),
        )
        .finish();

    let hoverable = Hoverable::new(state, move |mouse| {
        let background = match (selected, mouse.is_hovered()) {
            (true, _) => theme().overlay_3,
            (false, true) => theme().overlay_1,
            (false, false) => Color::TRANSPARENT,
        };
        Container::new(line)
            .with_background_color(background)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .with_padding(Padding {
                top: 6.,
                bottom: 6.,
                left: 8.,
                right: 8.,
            })
            .with_margin_bottom(2.)
            .finish()
    });

    let element = match command {
        Some(command) => hoverable
            .on_click(move |_, ctx, _| ctx.dispatch_typed_action(command))
            .finish(),
        // Unreachable: `ready` registers one per plugin. Cheaper to draw than
        // to prove unreachable from here.
        None => hoverable.finish(),
    };

    ConstrainedBox::new(element)
        .with_height(ROW_HEIGHT)
        .finish()
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
