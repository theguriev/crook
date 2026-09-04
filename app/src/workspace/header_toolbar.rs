//! The header row: whatever is pinned to the right of it — and, since the
//! window has no title bar of its own, the window's controls and the place to
//! pick it up by.
//!
//! What is pinned to the right is whatever a plugin put in
//! [`HEADER_RIGHT`](crate::plugins::header::HEADER_RIGHT), and this row does
//! not know what that is. It used to name the usage chip; that chip is now
//! contributed by the plugin that owns it, and this is the first surface in
//! Crook that draws something it was given rather than something it imports.
//!
//! One item, not a list, and that is a judgement about the surface: this row is
//! also the window's title bar, and a line of competing chips across it is how
//! a status bar becomes a place nobody reads.
//!
//! # It holds no tabs at all
//!
//! The tabs live in the panel down the left edge and nowhere else. Warp takes
//! the same early return at `view.rs:20916` when its vertical tabs are on, and
//! leaves a flexible slot where its title-bar search bar would go; Crook has no
//! search bar, so the slot is empty and whatever a plugin pinned to the right
//! is the whole of this row.
//!
//! The panel owns the window's top-left corner, so this row owes only the
//! right end of the window-control reservation. That is not decided here —
//! [`Workspace::window_insets`](super::view::Workspace) hands back the share
//! that belongs to this element, which is what keeps the two halves of one
//! answer from being written in two places.
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

use crate::theme::theme;

use super::title_bar;
use super::view::Workspace;

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
    // answer, given once rather than per platform.
    let insets = workspace.window_insets();
    let controls = title_bar::caption_buttons(workspace);

    let items = Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::End)
            // The panel draws the tabs; this row draws none. Warp takes the
            // same early return and leaves a flexible slot where its
            // title-bar search bar would go; Crook has no search bar, so the
            // slot is empty and whatever a plugin pinned is the whole of it.
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
            // Not a chip drawn transparently: the settings page's switch turns
            // the poll off as well as the pill, and a chip that was still in
            // the tree would still be a view being rendered, observed and laid
            // out for a number nobody asked for.
            .with_child(
                workspace
                    .host()
                    .slots()
                    .one(crate::plugins::header::HEADER_RIGHT, |build| {
                        build(workspace, app)
                    })
                    .unwrap_or_else(|| Empty::new().finish()),
            )
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
