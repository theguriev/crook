//! What the space under the tab list carries: the `+`, and the ground its
//! options menu opens from.
//!
//! Warp's browser ends a list of tabs with the affordance that lengthens it,
//! and Chrome puts the same button immediately after its last tab. So does
//! this: a band directly under the last row, the width of a row and lit like
//! one, with the `+` centred in it. The mark is not the button — a 28px square
//! is a smaller target than the one in the bar this replaced, and it reads as
//! a label rather than as something to press — but the band is a band and not
//! the whole column. The room below it is room, and a button that filled it
//! would be a three-hundred-pixel target for a thing the list does once.
//!
//! The gear that used to sit beside the `+` is gone entirely. "View options"
//! is a menu about the list, so what opens it is a secondary press on this
//! space — the gesture every desktop already spends on a menu about the thing
//! under the pointer, and the one this panel was already using for a row's own
//! menu one layer down. The band answers it too, so the gesture does not have
//! a hole in the middle of the space it belongs to.
//!
//! The menu is 200 wide in a 248px column, so it hangs from the list's *right*
//! edge: aligned left it would open across the body, and `keep_on_screen`
//! would not pull it back because the window has plenty of room to its right.

use crookui_core::prelude::*;

use crate::tab::TabAction;
use crate::theme::theme;

use super::action::{OptionsAction, WorkspaceAction};
use super::tab_options_menu;
use super::view::Workspace;

/// The `+` itself. Lucide's own 24-unit box scaled to sixteen, which is the
/// size the panel draws a group's chevron at.
///
/// Fourteen while it was a toolbar icon beside a gear; it is the only mark in
/// the space under the list now, and a toolbar's size in the middle of an
/// empty band reads as something left behind rather than as something to
/// press.
const PLUS_ICON_SIZE: f32 = 16.;

/// How far the lit band is held off the edges it would otherwise run into:
/// the last row above it and the panel's own sides.
///
/// [`GROUP_HORIZONTAL_PADDING`](super::tabs_panel), which is the inset a row
/// takes, so the two line up down both edges — the band under the list is
/// still part of the list.
const PLUS_INSET: f32 = 8.;

/// How deep that band is.
///
/// Deep enough to read as a place to press rather than as a line under the
/// last tab, and no deeper: it is taken off the scroll viewport, so every
/// pixel here is a row a person with a full list has to scroll for. Forty is
/// what leaves the eight rows the panel held when a control bar sat over it
/// instead — see [`tab_list`](super::tabs_panel::tab_list).
pub(super) const NEW_TAB_HEIGHT: f32 = 40.;

/// The corner radius of that ground.
///
/// Five rather than the rows' four, which is the radius the `+` has had since
/// it was in the control bar. It is also what tells the two apart in the
/// scene: every other rounded box in this panel is a row, a card or a close
/// button at four.
const PLUS_RADIUS: f32 = 5.;

/// The band under the last row, and the `+` centred in it.
///
/// The whole band lights up and the whole band answers the press. It is as
/// wide as a row because it is the end of the list, and the [`Align`] inside
/// it is what makes that true of the lit ground rather than only of the hit
/// box: `Align` returns `constraint.max` on a finite axis, so the container
/// around it measures the band rather than the glyph.
pub(super) fn new_tab_area(workspace: &Workspace) -> Box<dyn Element> {
    let band = Hoverable::new(workspace.new_tab_state(), move |state| {
        let hovered = state.is_hovered();
        Container::new(
            Align::new(
                Icon::new(Lucide::Plus, PLUS_ICON_SIZE)
                    .with_color(if hovered {
                        theme().text_primary
                    } else {
                        theme().text_muted
                    })
                    .finish(),
            )
            .finish(),
        )
        // The row's own hover colour. The `+` used to light up in
        // `tab_active`, which nothing else in this panel uses and which said
        // "this is the tab you are in" about a control that is not a tab.
        .with_background_color(if hovered {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(PLUS_RADIUS)))
        // A margin rather than the parent's padding: the inset is where the
        // band stops being *painted*, not where it stops answering. The press
        // still lands out to the panel's own edge.
        .with_uniform_margin(PLUS_INSET)
        .finish()
    })
    .on_click(|_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Tab(TabAction::New)))
    // The band paints over the ground, so without this the one strip of the
    // space under the list that opens no menu would be the strip in the middle
    // of it. Same action, because it is the same gesture on the same space.
    .on_right_click(|_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Options(OptionsAction::TogglePopup));
    })
    .finish();

    ConstrainedBox::new(band)
        .with_height(NEW_TAB_HEIGHT)
        .finish()
}

/// `list` over the ground its secondary press opens the options menu from.
///
/// The ground is a sibling *under* the list, not a wrapper around it, and that
/// is the whole of why this works. A [`Hoverable`] hands an event to its child
/// and then claims it anyway — the panel says so twice already, where a
/// container around a tab's rows deliberately carries no click handler — so
/// one wrapping the list would fire in addition to a row's own press and open
/// two menus at once. A sibling underneath is asked only for what the list did
/// not want: [`Stack`] walks its children topmost first and stops at the one
/// that claims the event, and the ground is covered by every rect a row
/// painted, so it never answers for a pixel a row is on.
///
/// The menu hangs from this box's top-right corner rather than from the `+`,
/// and the corner is the reason: the `+` moves down the column as tabs are
/// opened, so a menu hung from it would open somewhere different every time.
/// This box is the list area, which does not move — and its right edge is
/// where a 200px menu has to start if it is to stay inside a 248px column, per
/// this module's own doc. [`AnchorTo`] cannot follow a pointer, so a corner
/// that holds still is the nearest thing to a menu appearing where it was
/// asked for.
pub(super) fn options_ground(workspace: &Workspace, list: Box<dyn Element>) -> Box<dyn Element> {
    let mut stack = Stack::new()
        .with_child(
            Hoverable::new(workspace.panel_ground_state(), |_| Empty::new().finish())
                .on_right_click(|_, ctx, _| {
                    ctx.dispatch_typed_action(WorkspaceAction::Options(OptionsAction::TogglePopup));
                })
                .finish(),
        )
        .with_child(list);

    if workspace.is_options_menu_open() {
        stack.add_anchored_overlay_child(
            Dismiss::new(tab_options_menu::render(workspace))
                // The rest of the window is inert while the menu is up, which
                // is what makes a second press on the space under the list one
                // toggle rather than two — `Dismiss` treats the secondary
                // button as a dismissal for exactly this case — and what stops
                // a tab under the menu from hovering.
                .modal()
                .on_dismiss(|ctx, _| {
                    // The element only reports; taking the menu down is this
                    // handler's job, because what closing means belongs to the
                    // view that opened it.
                    ctx.dispatch_typed_action(WorkspaceAction::Options(OptionsAction::TogglePopup));
                })
                .finish(),
            AnchorTo {
                parent: Corner::TopRight,
                child: Corner::TopRight,
                offset: Vector2F::zero(),
                keep_on_screen: true,
                keep_clear_of_parent: false,
            },
        );
    }

    stack.finish()
}
