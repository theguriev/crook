//! The header row: the tab strip when there is one, then whatever is pinned to
//! the right.
//!
//! There is one right-hand item and its slot is hard-coded. Warp's header
//! items are a persisted, user-reorderable, cloud-synced setting, which means
//! adding one is a settings-schema migration; at two features that is a cost
//! with nothing on the other side of it.
//!
//! # What the layout changes here
//!
//! With the tabs in a panel this row holds *no tab items at all* — not a
//! narrower strip, not a collapsed one. Warp takes the same early return at
//! `view.rs:20916` and leaves a flexible slot where its title-bar search bar
//! would go; Crook has no search bar, so the slot is empty and the usage chip
//! is the whole of the header.
//!
//! The window-control reservation moves with the tabs. With a strip, this row
//! spans the top edge and owes both ends of it; with a panel, the panel owns
//! the top-left corner and this row owes only the right. Neither of those is
//! decided here — [`Workspace::window_insets`](super::view::Workspace) hands
//! back the share that belongs to this element, which is what keeps the two
//! halves of one answer from being written in two places.

use crookui_core::elements::Padding;
use crookui_core::prelude::*;

use crate::settings::Layout;
use crate::theme::theme;

use super::tab_bar;
use super::view::Workspace;

/// Space between the tab strip and the chip, so a wide title never runs into
/// a percentage.
const CHIP_GUTTER: f32 = 12.;

/// Lifts the chip off the header's bottom edge, which the tabs sit flush
/// against.
const CHIP_LIFT: f32 = 5.;

pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    // Crook's window is decorated by the window manager, so the controls are
    // in a bar of their own above this row and nothing here has to move out of
    // their way — on every platform, in both layouts. The call is kept rather
    // than the answer hard-coded: the day the window goes borderless, this is
    // one of the two lines that already has the right answer.
    let insets = workspace.window_insets();

    let leading = match workspace.options().layout {
        Layout::Horizontal => tab_bar::render(workspace, app),
        // The panel is drawing the tabs. An `Empty` rather than a shortened
        // strip: the two are mutually exclusive, and a strip that rendered
        // "no rows" would still be listening for the clicks the panel is
        // handling.
        Layout::Vertical => Empty::new().finish(),
    };

    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::End)
            .with_child(Expanded::new(1., leading).finish())
            // Not a chip drawn transparently: the settings page's switch turns
            // the poll off as well as the pill, and a chip that was still in
            // the tree would still be a view being rendered, observed and laid
            // out for a number nobody asked for.
            .with_child(if workspace.general().show_usage_chip {
                Container::new(ChildView::new(workspace.chip()).finish())
                    .with_margin_left(CHIP_GUTTER)
                    .with_margin_bottom(CHIP_LIFT)
                    .finish()
            } else {
                Empty::new().finish()
            })
            .finish(),
    )
    .with_background_color(theme().surface)
    .with_border(Border::bottom(1.).with_border_color(theme().border))
    .with_padding(Padding {
        top: 6.,
        left: insets.header_left + 8.,
        bottom: 0.,
        right: insets.header_right + 10.,
    })
    .finish()
}
