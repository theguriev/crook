//! The header row: whatever is pinned to the right of it — and, since the
//! window has no title bar of its own, the place to pick the window up by.
//!
//! What is pinned to the right is whatever a plugin put in
//! [`HEADER_RIGHT`](crate::plugins::header::HEADER_RIGHT), and this row does
//! not know what that is — nothing a release binary carries fills it, and a
//! plugin installed from a file does. This is the first surface in Crook that
//! draws something it was given rather than something it imports, which is why
//! an empty slot has to be as ordinary here as a full one.
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
//! # The reservation is a hole and nothing else
//!
//! Both ends of it are room for controls this row does not draw: Crook draws
//! none anywhere, so what is reserved is the corner macOS's traffic lights are
//! painted into and nothing more. On Windows and Linux the window has no
//! controls over this surface at all and the reservation is zero at both ends.

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
    // title bar and the traffic lights are painted over it on macOS. Which
    // end, and how much, is one answer, given once rather than per platform.
    let insets = workspace.window_insets();

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
        // Room for controls this row does not draw, on the platform where
        // something is painted over this end of it.
        right: PADDING.right + insets.header_right,
        ..PADDING
    })
    .finish();

    title_bar::draggable(
        workspace,
        Container::new(items)
            .with_background_color(theme().surface)
            // The seam between the header and the body, and the only line
            // across the top of the window: what used to be above this row was
            // the window manager's title bar, and there is no longer one to
            // divide anything from.
            .with_border(Border::bottom(1.).with_border_color(theme().border))
            .finish(),
    )
}
