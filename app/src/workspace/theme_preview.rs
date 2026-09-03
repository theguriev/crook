//! One theme, drawn as a miniature of what choosing it would do.
//!
//! Warp's preview card, and it is the cheapest good idea in that part of Warp:
//! rather than a screenshot to keep up to date or a row of colour swatches
//! that says nothing about how they combine, it draws a fake terminal in the
//! theme — a line of text, two ANSI colours, a divider and a cursor. Five
//! colours in a card, always correct because they come out of the palette
//! itself.
//!
//! Here rather than in either of the two places that draw one, because both
//! draw the *same* card at two sizes: the settings page's row, and the
//! chooser panel's list. Warp's is 190 x 100 in the panel and the same
//! renderer at a 0.6 scale factor in the settings row, which is the shape this
//! keeps.
//!
//! # The one element in Crook painted in a palette that is not in force
//!
//! Every colour inside the card comes from the `theme` parameter. The only
//! things read from [`theme()`](crate::theme::theme) are the outline and the
//! label beside it, because "which card is chosen" and "which card the pointer
//! is on" are the page's language rather than the previewed theme's.

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::prelude::*;

use crate::theme::{Theme, theme};

use super::action::WorkspaceAction;
use super::view::Fonts;

/// The card in the chooser panel, which is Warp's own 190 x 100.
pub(super) const PANEL_CARD: Vector2F = vec2f(190., 100.);

/// The card on the settings page, beside the name of the theme in force.
///
/// Warp draws its settings row at 0.6 of the panel's card; this is that,
/// rounded to whole pixels.
pub(super) const ROW_CARD: Vector2F = vec2f(114., 60.);

/// A miniature terminal in `card`, sized to `size`.
///
/// `command` is what clicking it does; `None` draws a card that is not a
/// control at all — which is what the settings page's row wants, since there
/// the whole row is the button.
pub(super) fn card(
    card: Theme,
    size: Vector2F,
    selected: bool,
    command: Option<WorkspaceAction>,
    state: MouseStateHandle,
    fonts: Fonts,
) -> Box<dyn Element> {
    // Two type sizes, so the small card is not a wall of glyphs: the ratio is
    // the same as between the two cards.
    let text_size = if size.x() > 150. { 9. } else { 7.5 };
    let clickable = command.is_some();

    let element = Hoverable::new(state, move |mouse| {
        let outline = if selected {
            card.accent
        } else if clickable && mouse.is_hovered() {
            theme().text_muted
        } else {
            theme().border
        };

        // A shell that has just run `ls`: the command in the theme's own
        // foreground, then a directory in its blue and an executable in its
        // red, which is what a person actually looks at when they judge a
        // terminal theme.
        let line = |text: &'static str, color: Color| {
            Text::new(text, fonts.monospace, text_size)
                .with_color(color)
                .finish()
        };

        let grid = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(3.)
            .with_child(line("$ ls", card.terminal.foreground))
            .with_child(
                Flex::row()
                    .with_spacing(6.)
                    .with_child(line("docs", card.terminal.normal[4]))
                    .with_child(line("crook", card.terminal.normal[1]))
                    .with_child(line("README", card.terminal.foreground))
                    .finish(),
            )
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
            // The divider and the cursor: the input field, in miniature.
            .with_child(
                Container::new(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            Container::new(
                                ConstrainedBox::new(Empty::new().finish())
                                    .with_width(2.)
                                    .with_height(text_size + 1.)
                                    .finish(),
                            )
                            .with_background_color(card.accent)
                            .finish(),
                        )
                        .finish(),
                )
                .with_border(Border::top(1.).with_border_color(card.border))
                .with_padding(Padding {
                    top: 5.,
                    bottom: 1.,
                    left: 0.,
                    right: 0.,
                })
                .finish(),
            )
            .finish();

        ConstrainedBox::new(
            Container::new(grid)
                .with_background_color(card.terminal.background)
                .with_border(Border::all(if selected { 2. } else { 1. }).with_border_color(outline))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
                .with_uniform_padding(if size.x() > 150. { 8. } else { 6. })
                .finish(),
        )
        .with_width(size.x())
        .with_height(size.y())
        .finish()
    });

    match command {
        Some(action) => element
            .on_click(move |_, ctx, _| ctx.dispatch_typed_action(action))
            .finish(),
        None => element.finish(),
    }
}
