//! Making a theme: five swatches, a preview, and two buttons.
//!
//! Warp's creator is a 600 x 300 modal with a file picker at the top, a name
//! field, a row of five background swatches, and Cancel / Create theme. Take
//! away the picker — there is no image decoder and no way to draw an image —
//! and take away the name field — there is no text input outside a terminal
//! pane — and what is left is the part that decides the palette: **the five
//! swatches**. So that is what this is.
//!
//! It sits in the panel rather than in a modal of its own. Warp needs a modal
//! because its creator carries a file picker and a text field; a five-swatch
//! choice with a preview under it fits in 248 pixels, and a floating surface
//! over a docked one is a layer nobody asked for.
//!
//! # Live, like Warp's
//!
//! Every swatch click applies the draft to the whole window immediately —
//! chrome, grids and all — because that is the only way to judge a palette.
//! Nothing is written until "Create theme", and "Cancel" puts back the theme
//! that was in force when the creator opened.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::prelude::*;

use crate::theme::creator::CANDIDATES;
use crate::theme::theme;

use super::super::action::{ThemeAction, WorkspaceAction};
use super::super::theme_preview::{self, PANEL_CARD};
use super::super::view::Workspace;
use super::{Control, content_width};

/// One swatch's height. Warp's are 110 x 40 in a 600px modal; these are the
/// same height in a 248px panel, which is what makes five of them fit.
const SWATCH_HEIGHT: f32 = 40.;

/// The whole creator.
pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let state = workspace.theme_panel();
    let ui = workspace.fonts().ui;
    let Some(draft) = state.draft.as_ref() else {
        // Unreachable: the mode and the draft are set together.
        return Empty::new().finish();
    };

    // The buttons are pinned and everything above them scrolls. A column that
    // simply grew put Cancel and Create past the bottom edge of a short window
    // — drawn, unreachable, and with no hint that they were there.
    let scrolling = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(swatches(workspace, draft))
        .with_child(
            Container::new(Align::new(preview(workspace, draft)).finish())
                .with_margin_top(16.)
                .finish(),
        )
        .with_child(note(
            format!("Text and colours follow. {}", contrast_note(draft)),
            ui,
        ))
        .finish();

    Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Expanded::new(
                1.,
                Scrollable::new(state.scroll.clone(), scrolling)
                    .with_scrollbar(theme().overlay_3)
                    .finish(),
            )
            .finish(),
        )
        .with_child(
            Container::new(buttons(workspace, ui))
                .with_margin_top(10.)
                .finish(),
        )
        .finish()
}

/// The five colours a background is chosen from.
///
/// Warp rounds the first swatch's left corners and the last one's right
/// corners so the row reads as one control, and marks the chosen one with a
/// three-pixel border. Both are kept.
fn swatches(workspace: &Workspace, draft: &crate::theme::creator::Draft) -> Box<dyn Element> {
    let state = workspace.theme_panel();
    let width = content_width() / CANDIDATES as f32;

    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    for (index, color) in draft.candidates.iter().enumerate() {
        let color = *color;
        let chosen = index == draft.chosen;
        let radius = match index {
            0 => CornerRadius::with_left(Radius::Pixels(6.)),
            index if index == CANDIDATES - 1 => CornerRadius::with_right(Radius::Pixels(6.)),
            _ => CornerRadius::default(),
        };

        row.add_child(swatch(
            color,
            chosen,
            radius,
            width,
            state.control(Control::Swatch(index)),
            index,
        ));
    }

    row.finish()
}

/// One swatch.
fn swatch(
    color: Color,
    chosen: bool,
    radius: CornerRadius,
    width: f32,
    state: MouseStateHandle,
    index: usize,
) -> Box<dyn Element> {
    Hoverable::new(state, move |mouse| {
        // The mark goes *inside* the swatch, in whichever of black and white
        // reads on it — an outline in the interface's own colours would be
        // invisible on the swatch that happens to match it.
        let mark = if crate::theme::creator::foreground_for(color) == Color::hex(0xffffff) {
            Color::hex(0xffffff)
        } else {
            Color::hex(0x000000)
        };
        let border = if chosen {
            Border::all(3.).with_border_color(mark)
        } else if mouse.is_hovered() {
            Border::all(1.).with_border_color(mark)
        } else {
            Border::default()
        };

        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(color)
                .with_border(border)
                .with_corner_radius(radius)
                .finish(),
        )
        .with_width(width)
        .with_height(SWATCH_HEIGHT)
        .finish()
    })
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Theme(ThemeAction::PickBackground(index)));
    })
    .finish()
}

/// The draft, as the card the chooser would list it with.
fn preview(workspace: &Workspace, draft: &crate::theme::creator::Draft) -> Box<dyn Element> {
    theme_preview::card(
        draft.theme,
        PANEL_CARD,
        true,
        None,
        workspace.theme_panel().control(Control::Preview),
        workspace.fonts(),
    )
}

/// A line of explanation, broken to the panel's width.
///
/// `Text` never wraps — it is built for a tab title — so a paragraph is a
/// column of lines, and the budget is counted in characters because measuring
/// needs the shaper and this runs while the tree is being built.
fn note(text: String, ui: crookui_core::fonts::FamilyId) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Start);
    for line in super::super::wrap(&text, 34) {
        column.add_child(
            Text::new(line, ui, 10.)
                .with_color(theme().text_muted)
                .with_line_height_ratio(1.4)
                .finish(),
        );
    }

    Container::new(column.finish())
        .with_margin_top(10.)
        .finish()
}

/// What the draft decided for itself, in one clause.
fn contrast_note(draft: &crate::theme::creator::Draft) -> &'static str {
    if draft.theme.is_light {
        "This one is a light theme."
    } else {
        "This one is a dark theme."
    }
}

/// Cancel and Create theme.
fn buttons(workspace: &Workspace, ui: crookui_core::fonts::FamilyId) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.)
        .with_child(
            Expanded::new(
                1.,
                button(
                    workspace,
                    "Cancel",
                    false,
                    Control::Cancel,
                    ThemeAction::CancelCreating,
                    ui,
                ),
            )
            .finish(),
        )
        .with_child(
            Expanded::new(
                1.,
                button(
                    workspace,
                    "Create",
                    true,
                    Control::Save,
                    ThemeAction::Create,
                    ui,
                ),
            )
            .finish(),
        )
        .finish()
}

/// One of the two buttons.
fn button(
    workspace: &Workspace,
    label: &'static str,
    primary: bool,
    control: Control,
    action: ThemeAction,
    ui: crookui_core::fonts::FamilyId,
) -> Box<dyn Element> {
    let state = workspace.theme_panel().control(control);

    Hoverable::new(state, move |mouse| {
        let (background, border) = match (primary, mouse.is_hovered()) {
            (true, false) => (theme().accent, theme().accent),
            (true, true) => (theme().accent, theme().text_primary),
            (false, false) => (Color::TRANSPARENT, theme().border),
            (false, true) => (theme().overlay_2, theme().border),
        };

        Container::new(
            Align::new(
                Text::new(label, ui, 11.)
                    .with_color(theme().text_primary)
                    .finish(),
            )
            .finish(),
        )
        .with_padding(Padding {
            top: 6.,
            bottom: 6.,
            left: 8.,
            right: 8.,
        })
        .with_background_color(background)
        .with_border(Border::all(1.).with_border_color(border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .finish()
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Theme(action)))
    .finish()
}
