//! The list of themes: one preview card per theme, with its name under it.
//!
//! Warp's rows are a 190 x 100 thumbnail, the name below it, sixteen pixels of
//! padding above and below, and a raised background on the row that is
//! selected. That is this, at Crook's type sizes.
//!
//! The list scrolls, and the keyboard scrolls it: rows are a fixed height, so
//! "keep the selected row on screen" is arithmetic rather than a measurement
//! the element layer would have to hand back. Warp's `UniformList` virtualises
//! — only the visible rows are built — which matters at twenty-one themes plus
//! a folder and does not at three plus a folder; every row is built here, and
//! the day that stops being true this is where it changes.

use crookui_core::elements::{Padding, Paragraph};
use crookui_core::prelude::*;

use crate::theme::theme;

use super::super::action::{ThemeAction, WorkspaceAction};
use super::super::theme_preview::{self, PANEL_CARD};
use super::super::view::Workspace;
use super::{Control, content_width};

/// The space above and below one row's card. Warp's sixteen.
const ROW_PADDING: f32 = 12.;

/// The gap between a card and the name under it.
const NAME_GAP: f32 = 8.;

/// How tall one row is, which is what makes scrolling to a row arithmetic.
///
/// The card, the gap, two lines of text at their line heights, and the padding
/// on both sides. Asserted in the workspace tests against the rendered frame,
/// because a constant that drifts from what is drawn would scroll to the wrong
/// place rather than fail.
pub(in crate::workspace) const ROW_HEIGHT: f32 =
    ROW_PADDING * 2. + PANEL_CARD.y() + NAME_GAP + 12. * 1.2 + 10. * 1.2 + 3.;

/// The whole list.
pub(super) fn render(workspace: &Workspace, _: &AppContext) -> Box<dyn Element> {
    let state = workspace.theme_panel();
    let ui = workspace.fonts().ui;
    let current = workspace.theme_name();

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    if workspace.themes().is_empty() {
        // Unreachable — the built-ins are always there — and cheaper to write
        // than to prove unreachable from here.
        return Empty::new().finish();
    }

    for (index, available) in workspace.themes().iter().enumerate() {
        column.add_child(row(
            workspace,
            index,
            available,
            available.name == current,
            index == state.selected,
            ui,
        ));
    }

    // After the rows, so the arrows' arithmetic — row `n` is at `n` heights
    // — is untouched, and so that what could not be chosen comes after what
    // can. The person who just saved a file with a mistake in it is looking
    // here for it to appear, and a list that said nothing sent them to a log.
    for unreadable in workspace.unreadable_themes() {
        column.add_child(unreadable_row(unreadable, ui));
    }

    Scrollable::new(state.scroll.clone(), column.finish())
        .with_scrollbar(theme().overlay_3)
        .finish()
}

/// A file in the themes folder that is not a theme yet: its name, and what
/// is wrong with it, in the words the reader used.
fn unreadable_row(
    unreadable: &crate::theme::Unreadable,
    ui: crookui_core::fonts::FamilyId,
) -> Box<dyn Element> {
    let name = unreadable
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| unreadable.path.display().to_string());

    Container::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Text::new(name, ui, 12.)
                    .with_color(theme().text_muted)
                    .with_ellipsis(Cut::Start)
                    .finish(),
            )
            .with_child(
                Container::new(
                    Paragraph::new(format!("could not be read: {}", unreadable.why), ui, 10.)
                        .with_color(theme().text_muted)
                        .with_line_height_ratio(1.35)
                        .finish(),
                )
                .with_margin_top(3.)
                .finish(),
            )
            .finish(),
    )
    .with_padding(Padding {
        top: ROW_PADDING,
        bottom: ROW_PADDING,
        left: (content_width() - PANEL_CARD.x()) / 2.,
        right: (content_width() - PANEL_CARD.x()) / 2.,
    })
    .finish()
}

/// One theme's row.
fn row(
    workspace: &Workspace,
    index: usize,
    available: &crate::theme::Available,
    in_force: bool,
    selected: bool,
    ui: crookui_core::fonts::FamilyId,
) -> Box<dyn Element> {
    let state = workspace.theme_panel();
    let fonts = workspace.fonts();
    let name = available.name.clone();
    let origin = if available.from_file() {
        "from your themes folder"
    } else {
        "built in"
    };

    // The card is the click target and carries the "this is the one in force"
    // outline; the row carries the "this is where the keyboard is" background.
    // Two different questions, and while the panel is only being clicked they
    // have the same answer.
    let card = theme_preview::card(
        available.theme,
        PANEL_CARD,
        in_force,
        Some(WorkspaceAction::Theme(ThemeAction::Choose(index))),
        state.control(Control::Theme(index)),
        fonts,
    );

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(Align::new(card).finish())
        .with_child(
            Container::new(
                Text::new(name, ui, 12.)
                    .with_color(theme().text_primary)
                    .finish(),
            )
            .with_margin_top(NAME_GAP)
            .finish(),
        );
    column.add_child(
        Container::new(
            Text::new(origin, ui, 10.)
                .with_color(theme().text_muted)
                .finish(),
        )
        .with_margin_top(3.)
        .finish(),
    );

    ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(if selected {
                theme().overlay_1
            } else {
                Color::TRANSPARENT
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .with_padding(Padding {
                top: ROW_PADDING,
                bottom: ROW_PADDING,
                left: (content_width() - PANEL_CARD.x()) / 2.,
                right: (content_width() - PANEL_CARD.x()) / 2.,
            })
            .finish(),
    )
    .with_height(ROW_HEIGHT)
    .finish()
}
