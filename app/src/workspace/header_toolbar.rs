//! The header row: the tab strip when there is one, then whatever is pinned to
//! the right — and, since the window has no title bar of its own, the window's
//! controls and the place to pick it up by.
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
//!
//! # The reservation is spent once
//!
//! Both ends of it are room for the window's controls, but only one of them is
//! ever *empty*. On macOS the left end is a hole for the traffic lights AppKit
//! paints over this row; on Windows and Linux the right end is not a hole at
//! all — it is where [`title_bar::caption_buttons`] draws the controls Crook
//! owes the window. So the left end is padding and the right end is either
//! padding or the cluster itself, never both.

use crookui_core::elements::Padding;
use crookui_core::prelude::*;

use crate::settings::Layout;
use crate::theme::theme;

use super::tab_bar;
use super::title_bar;
use super::view::Workspace;

/// Space between the tab strip and the chip, so a wide title never runs into
/// a percentage.
const CHIP_GUTTER: f32 = 12.;

/// Lifts the chip off the header's bottom edge, which the tabs sit flush
/// against.
const CHIP_LIFT: f32 = 5.;

/// The header's own padding, before anything the window asked for.
const PADDING: Padding = Padding {
    top: 6.,
    left: 8.,
    bottom: 0.,
    right: 10.,
};

pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    // Crook's window is the application's to decorate, so this row is the
    // title bar and something is over it: the traffic lights on macOS, the
    // controls below on Windows and Linux. Which end, and how much, is one
    // answer given per layout rather than per platform.
    let insets = workspace.window_insets();
    let controls = title_bar::caption_buttons(workspace);

    let leading = match workspace.options().layout {
        Layout::Horizontal => tab_bar::render(workspace, app),
        // The panel is drawing the tabs. An `Empty` rather than a shortened
        // strip: the two are mutually exclusive, and a strip that rendered
        // "no rows" would still be listening for the clicks the panel is
        // handling.
        Layout::Vertical => Empty::new().finish(),
    };

    let items = Container::new(
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
    .with_padding(Padding {
        left: insets.header_left + PADDING.left,
        // Room for controls this row does not draw. Where it draws them, the
        // cluster beside this is the reservation and adding it here as well
        // would spend it twice.
        right: PADDING.right
            + if controls.is_some() {
                0.
            } else {
                insets.header_right
            },
        ..PADDING
    })
    .finish();

    // Aligned to the *top*, unlike the row inside it: the tabs hang from the
    // header's bottom edge, but the caption buttons belong to the window and
    // every desktop that draws them puts them hard against its top-right
    // corner. Bottom-aligning them left a strip of inert header above the
    // close button — exactly where a person throws the pointer to close a
    // maximised window without aiming.
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(Expanded::new(1., items).finish());
    // Outside the padding, so the close button reaches the corner of the
    // window the way a caption button is expected to.
    row.add_children(controls);

    title_bar::draggable(
        workspace,
        Container::new(row.finish())
            .with_background_color(theme().surface)
            // The seam between the header and the body, and the only line
            // across the top of the window: what used to be above this row was
            // the window manager's title bar, and there is no longer one to
            // divide anything from.
            .with_border(Border::bottom(1.).with_border_color(theme().border))
            .finish(),
    )
}
