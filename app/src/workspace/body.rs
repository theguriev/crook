//! The active tab's body.
//!
//! There is no pseudo-terminal in v1 and no terminal emulation, so this panel
//! says so rather than pretending. What it does prove is the part that
//! matters at this stage: only the active tab's content is in the element
//! tree, and it is rebuilt from the session the strip says is active.

use crookui_core::fonts::{Properties, Weight};
use crookui_core::prelude::*;

use crate::theme::THEME;

use super::view::Workspace;

pub(super) fn render(workspace: &Workspace) -> Box<dyn Element> {
    let fonts = workspace.fonts();
    let Some(tab) = workspace.tabs().active() else {
        return Empty::new().finish();
    };

    let panel = Container::new(
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(9.)
            .with_child(
                Text::new(tab.title().to_owned(), fonts.ui, 16.)
                    .with_color(THEME.text_primary)
                    .with_style(Properties {
                        weight: Weight::Semibold,
                        ..Default::default()
                    })
                    .finish(),
            )
            .with_child(
                Text::new(
                    format!("agent session \u{b7} {}", tab.status().label()),
                    fonts.ui,
                    12.,
                )
                .with_color(THEME.text_muted)
                .finish(),
            )
            .with_child(
                Text::new(
                    format!("$ crook run --agent {}", tab.title().to_owned()),
                    fonts.monospace,
                    12.5,
                )
                .with_color(THEME.text_muted)
                .finish(),
            )
            .with_child(
                Text::new(
                    "# placeholder: v1 renders a session, it does not run one",
                    fonts.monospace,
                    12.5,
                )
                .with_color(THEME.text_muted.with_alpha(120))
                .finish(),
            )
            .finish(),
    )
    .with_background_color(THEME.surface)
    .with_border(Border::all(1.).with_border_color(THEME.border))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
    .with_uniform_margin(12.)
    .with_uniform_padding(16.)
    .finish();

    // A row that fills the width, so the panel inside it is handed a tight
    // constraint and stretches instead of shrinking to its longest line.
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., panel).finish())
        .finish()
}
