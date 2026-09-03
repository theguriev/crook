//! The header row: the tab strip, then whatever is pinned to the right.
//!
//! There is one right-hand item and its slot is hard-coded. Warp's header
//! items are a persisted, user-reorderable, cloud-synced setting, which means
//! adding one is a settings-schema migration; at two features that is a cost
//! with nothing on the other side of it.

use crookui_core::elements::Padding;
use crookui_core::prelude::*;

use crate::WINDOW_CHROME;
use crate::platform_insets::window_control_insets;
use crate::theme::THEME;

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
    // their way. The call is kept rather than the answer hard-coded: the day
    // the window goes borderless, this is the one line that has to change, and
    // it changes by passing a different chrome. Fullscreen is `false` because
    // the windowing layer exposes no way to enter it and no way to ask — and
    // under native chrome the answer is the same either way.
    let insets = window_control_insets(WINDOW_CHROME, false);

    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::End)
            .with_child(Expanded::new(1., tab_bar::render(workspace, app)).finish())
            .with_child(
                Container::new(ChildView::new(workspace.chip()).finish())
                    .with_margin_left(CHIP_GUTTER)
                    .with_margin_bottom(CHIP_LIFT)
                    .finish(),
            )
            .finish(),
    )
    .with_background_color(THEME.surface)
    .with_border(Border::bottom(1.).with_border_color(THEME.border))
    .with_padding(Padding {
        top: 6.,
        left: insets.left + 8.,
        bottom: 0.,
        right: insets.right + 10.,
    })
    .finish()
}
