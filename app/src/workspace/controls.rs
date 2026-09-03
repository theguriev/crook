//! The two buttons both layouts carry: the gear, and the `+`.
//!
//! Warp renders these twice, once in the header toolbar and once in the
//! vertical panel's control bar, and the two copies have drifted — the panel's
//! gear grew an entrypoint parameter the header's never got. Here there is one
//! of each, and the only thing a caller chooses is where the menu opens.
//!
//! That parameter is not decoration. In the strip the gear sits at the right
//! end of a full-width header and the menu hangs below it, left edges aligned.
//! In the panel the gear sits near the right edge of a 248px column, and a
//! left-aligned 200px menu would open across the body; aligning the two
//! *right* edges instead keeps it inside the panel it belongs to. Both are
//! [`AnchorTo`], so neither needs its own function.

use crookui_core::elements::Padding;
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::tab::TabAction;
use crate::theme::theme;

use super::GEAR_ICON;
use super::action::{OptionsAction, WorkspaceAction};
use super::tab_options_menu;
use super::view::Workspace;

/// What the gear says it does, while the pointer is on it and the menu is not
/// up.
pub(super) const GEAR_TOOLTIP: &str = "View options";

/// The gear itself, inside that slot. Lucide's own 24-unit box scaled to
/// fourteen, which is the size Warp draws its toolbar icons at.
const GEAR_ICON_SIZE: f32 = 14.;

/// The gear's hit box: a 16px icon slot with 2px of padding all round.
const GEAR_BUTTON_SIZE: f32 = 20.;

/// The tooltip's width, fixed so the centring offset below can be a constant.
///
/// Wide enough for [`GEAR_TOOLTIP`] at 11px plus its 16px of padding, with
/// room for an interface font wider than the average; the label is centred
/// inside it, so slack shows as symmetric margin rather than as a gap on one
/// side.
const GEAR_TOOLTIP_WIDTH: f32 = 88.;

/// The square both buttons occupy, so a control bar is the same height
/// whichever of them is in it.
pub(super) const BUTTON_SIZE: f32 = 24.;

/// The gear, and the menu it opens at `menu_anchor`.
///
/// The [`Stack`] goes here, around the button, rather than at the root: the
/// menu is anchored to the gear's painted box, and a stack wrapping the whole
/// window would have nothing to anchor to. The stack is painted before the menu
/// in the same frame, so the anchor is this frame's rect and the menu never
/// lags a frame behind the button.
pub(super) fn gear_button(workspace: &Workspace, menu_anchor: AnchorTo) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let is_open = workspace.is_options_menu_open();

    let gear_state = workspace.menu().gear.clone();
    if is_open {
        // The modal underlay covers the gear while the menu is up, so its
        // `Hoverable` never sees the pointer leave. Without this the frame
        // after the menu is dismissed draws a tooltip for a gear the pointer
        // walked away from several clicks ago.
        gear_state.lock().reset_interaction_state();
    }

    let gear = Hoverable::new(gear_state, move |state| {
        // Three states, and the open one is not reachable through this
        // handler: while the menu is up, the modal underlay covers the gear, so
        // its `Hoverable` never fires and a second click closes the menu
        // through the dismiss path instead of toggling it twice.
        let (glyph, background) = if is_open {
            (theme().text_primary, theme().overlay_3)
        } else if state.is_hovered() {
            (theme().text_muted, theme().overlay_2)
        } else {
            (theme().text_muted, Color::TRANSPARENT)
        };

        let button = Container::new(
            ConstrainedBox::new(
                Align::new(
                    Icon::new(GEAR_ICON, GEAR_ICON_SIZE)
                        .with_color(glyph)
                        .finish(),
                )
                .finish(),
            )
            .with_width(16.)
            .with_height(16.)
            .finish(),
        )
        .with_uniform_padding(2.)
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish();

        // The tooltip belongs to the closed-and-hovered state only. Warp
        // suppresses it with the same `is_hovered() && !is_popup_open`, and it
        // has to be suppressed rather than merely unreachable: the gear is the
        // one thing the popup hangs off, so a tooltip left up would sit
        // between the button and its own menu.
        if !state.is_hovered() || is_open {
            return button;
        }

        let mut stack = Stack::new().with_child(button);
        stack.add_anchored_overlay_child(
            gear_tooltip(ui),
            AnchorTo {
                // Warp's `ParentAnchor::BottomMiddle` →
                // `ChildAnchor::TopMiddle`, 4px below. [`Corner`] has only the
                // four corners, so the centring is the offset's job, which is
                // exact because both widths are constants.
                parent: Corner::BottomLeft,
                child: Corner::TopLeft,
                offset: vec2f(-(GEAR_TOOLTIP_WIDTH - GEAR_BUTTON_SIZE) / 2., 4.),
                keep_on_screen: true,
                keep_clear_of_parent: false,
            },
        );
        stack.finish()
    })
    .on_click(|_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Options(OptionsAction::TogglePopup));
    })
    .finish();

    let mut stack = Stack::new().with_child(gear);
    if is_open {
        stack.add_anchored_overlay_child(
            Dismiss::new(tab_options_menu::render(workspace))
                // The rest of the window is inert while the menu is up, which
                // is what makes re-clicking the gear one toggle rather than
                // two, and what stops a tab under the menu from hovering.
                .modal()
                .on_dismiss(|ctx, _| {
                    // The element only reports; taking the menu down is this
                    // handler's job, because what closing means belongs to the
                    // view that opened it.
                    ctx.dispatch_typed_action(WorkspaceAction::Options(OptionsAction::TogglePopup));
                })
                .finish(),
            menu_anchor,
        );
    }

    ConstrainedBox::new(Align::new(stack.finish()).finish())
        .with_width(BUTTON_SIZE)
        .with_height(BUTTON_SIZE)
        .finish()
}

/// The little panel that names the gear.
///
/// The label is centred with a [`Flex`] rather than with an [`Align`], and
/// that is not a style choice. An anchored overlay child is laid out against
/// the whole window, and `Align` returns `constraint.max` on every finite
/// axis — so an `Align` here measured 88 by the window's full height and
/// painted an 88px bar from the top of the window to the bottom, straight down
/// the tab list, with the label stranded in the middle of it. A row flex takes
/// the width it is given and hugs its child's height, which is the only half
/// of `Align` this wanted.
fn gear_tooltip(ui: FamilyId) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::Center)
                .with_child(
                    Text::new(GEAR_TOOLTIP, ui, 11.)
                        .with_color(theme().text_primary)
                        .finish(),
                )
                .finish(),
        )
        .with_background_color(theme().surface_raised)
        .with_border(Border::all(1.).with_border_color(theme().overlay_2))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_padding(Padding {
            top: 4.,
            left: 8.,
            bottom: 5.,
            right: 8.,
        })
        .finish(),
    )
    .with_width(GEAR_TOOLTIP_WIDTH)
    .finish()
}

/// The button that opens another tab.
pub(super) fn new_tab_button(workspace: &Workspace) -> Box<dyn Element> {
    ConstrainedBox::new(
        Hoverable::new(workspace.new_tab_state(), move |state| {
            let hovered = state.is_hovered();
            Container::new(
                Align::new(
                    Icon::new(Lucide::Plus, GEAR_ICON_SIZE)
                        .with_color(if hovered {
                            theme().text_primary
                        } else {
                            theme().text_muted
                        })
                        .finish(),
                )
                .finish(),
            )
            .with_background_color(if hovered {
                theme().tab_active
            } else {
                Color::TRANSPARENT
            })
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(5.)))
            .finish()
        })
        .on_click(|_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::New)))
        .finish(),
    )
    .with_width(BUTTON_SIZE)
    .with_height(BUTTON_SIZE)
    .finish()
}
