//! What the palette looks like.
//!
//! A card floated over the middle of the window: a field at the top, a list of
//! commands under it, and a line at the bottom saying what the keys do. The
//! shape is the one every palette has had since Sublime Text, and there is
//! nothing to argue with in it.
//!
//! # It is centred by the element tree, not by the anchor
//!
//! An anchor's offset is a constant and the window's width is not, so the
//! surface is anchored at the window's top-left corner and laid out against
//! the whole window — a row with a spacer on each side, which is what puts the
//! card in the middle at any width. See `WINDOW_OVERLAY`.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::plugin::ActionId;
use crate::theme::theme;
use crate::workspace::{TextField, WorkspaceAction};

use super::state::{Command, Control, Palette};

/// How wide the card is.
///
/// Wide enough for a title and an owner on one line without the two meeting,
/// and narrow enough that the eye does not have to travel from one to the
/// other. VS Code's is 600 at a 13px body; this is that at Crook's 12.
const CARD_WIDTH: f32 = 560.;

/// How far down the window the card starts. Not centred vertically: a palette
/// that grows downwards from a fixed point does not move under the pointer as
/// the list shortens, and every palette worth copying does this.
const TOP: f32 = 96.;

/// One row's height, which is what makes scrolling to a row arithmetic.
pub(super) const ROW_HEIGHT: f32 = 34.;

/// How many rows are shown before the list scrolls.
const VISIBLE_ROWS: f32 = 9.;

/// The inset inside the card.
const PADDING: f32 = 10.;

/// What the field says while nothing has been typed.
const PLACEHOLDER: &str = "Run a command";

/// The whole surface, or nothing at all while the palette is down.
pub(super) fn render(palette: &Palette, commands: &[(Command, ActionId)]) -> Box<dyn Element> {
    if !palette.is_open() {
        return Empty::new().finish();
    }

    let fonts = palette.fonts();
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Container::new(
                TextField::new(
                    palette.query().clone(),
                    palette.clipboard().clone(),
                    fonts,
                    palette.control(Control::Query),
                    PLACEHOLDER,
                )
                .with_icon(Lucide::Search)
                .finish(),
            )
            .with_margin_bottom(PADDING)
            .finish(),
        );

    if commands.is_empty() {
        column.add_child(
            Container::new(
                Text::new("No command matches that.", fonts.ui, 12.)
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
        for (index, (command, id)) in commands.iter().enumerate() {
            rows.add_child(row(
                command,
                *id,
                index == palette.selected(),
                palette.control(Control::Row(index)),
                fonts.ui,
            ));
        }

        column.add_child(
            ConstrainedBox::new(
                Scrollable::new(palette.scroll(), rows.finish())
                    .with_scrollbar(theme().overlay_3)
                    .finish(),
            )
            // The list is as tall as it needs to be up to nine rows, so a
            // palette with three commands in it is a small card rather than a
            // tall one with a hole in the bottom.
            .with_max_height(ROW_HEIGHT * VISIBLE_ROWS)
            .finish(),
        );
    }

    column.add_child(hint(fonts.ui));

    let card = ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme().surface)
            .with_border(Border::all(1.).with_border_color(theme().overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
            .with_uniform_padding(PADDING)
            .finish(),
    )
    .with_width(CARD_WIDTH)
    .finish();

    // A row that spans the window with the card between two spacers, which is
    // what centres it — see this module's own doc.
    let centred = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(card)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .finish();

    Container::new(centred).with_margin_top(TOP).finish()
}

/// One command.
fn row(
    command: &Command,
    id: ActionId,
    selected: bool,
    state: MouseStateHandle,
    ui: FamilyId,
) -> Box<dyn Element> {
    let line = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Text::new(command.title.clone(), ui, 12.)
                .with_color(theme().text_primary)
                .finish(),
        )
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        // The name, which is what a person copies into their keybindings.
        .with_child(
            Text::new(command.action.to_string(), ui, 10.5)
                .with_color(theme().text_muted)
                .finish(),
        )
        .finish();

    ConstrainedBox::new(
        Hoverable::new(state, move |mouse| {
            let background = match (selected, mouse.is_hovered()) {
                (true, _) => theme().overlay_2,
                (false, true) => theme().overlay_1,
                (false, false) => Color::TRANSPARENT,
            };

            Container::new(line)
                .with_background_color(background)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                .with_padding(Padding {
                    top: 7.,
                    bottom: 7.,
                    left: 8.,
                    right: 8.,
                })
                .finish()
        })
        .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Run(id)))
        .finish(),
    )
    .with_height(ROW_HEIGHT)
    .finish()
}

/// The line at the bottom that says what the keys do.
fn hint(ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(
            "\u{2191}\u{2193} to move \u{00b7} enter to run \u{00b7} esc to close",
            ui,
            10.5,
        )
        .with_color(theme().text_muted)
        .finish(),
    )
    .with_margin_top(PADDING)
    .finish()
}
