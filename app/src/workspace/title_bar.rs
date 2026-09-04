//! What makes the header a title bar rather than a row that happens to be at
//! the top: something to pick the window up by, and the controls the platform
//! does not draw for us.
//!
//! Crook opens a client-decorated window
//! ([`WINDOW_CHROME`](crate::WINDOW_CHROME)), which means the window's own
//! controls are over Crook's surface rather than in a bar above it. Two things
//! follow, and they are the two halves of this module.
//!
//! # Dragging
//!
//! A window with no title bar of its own has to be moved by the application.
//! [`draggable`] wraps a row and turns a press on its *empty space* into a
//! window drag, and a double click there into maximise — the gesture every
//! desktop gives a title bar.
//!
//! "Empty" is not a list of rectangles kept in step with the header's layout.
//! It is whatever the row's own children did not claim: the children see every
//! event first, and a press that a tab, the gear, the `+` or a caption button
//! handled never reaches the drag. Add a control to the header and it stops
//! being draggable there on the same frame, with nothing to remember.
//!
//! # The controls
//!
//! macOS keeps its traffic lights — they are the system's, painted over the
//! header by AppKit — so there is nothing to draw and this module draws
//! nothing. Windows and Linux have no equivalent: the window is borderless
//! there, so [`caption_buttons`] is the minimise, maximise and close controls
//! themselves, at the end of the header the platform puts them at.
//!
//! Their width is not a second opinion about the reservation in
//! [`platform_insets`](crate::platform_insets): [`CaptionMetrics::width`] is
//! what the cluster measures and the tests below assert it is exactly what the
//! header reserved. A cluster wider than its reservation would sit on the
//! usage chip; a narrower one would leave a hole.

use crookui_core::event::{DispatchedEvent, Event, MouseButton};
use crookui_core::geometry::{Point, ZIndex};
use crookui_core::prelude::*;
use crookui_core::presenter::{LayoutContext, PaintContext};

use crate::platform_insets::{ControlLayout, WindowChrome};
use crate::theme::theme;

use super::action::{WindowAction, WorkspaceAction};
use super::view::Workspace;

/// Picks the window up by `row`'s empty space.
///
/// `row` unchanged under native chrome: there the window manager draws a title
/// bar of its own and moves the window by it, and a header that also moved it
/// would be a second title bar with the tabs inside it.
pub(super) fn draggable(workspace: &Workspace, row: Box<dyn Element>) -> Box<dyn Element> {
    if matches!(workspace.window_chrome(), WindowChrome::Native) {
        return row;
    }

    DragToMove {
        child: row,
        origin: None,
        child_max_z_index: None,
    }
    .finish()
}

/// The window's own controls, where Crook is the one drawing them.
///
/// `None` on macOS, and under native chrome anywhere: in both of those the
/// window already has controls and a second set would be two close buttons on
/// one window.
pub(super) fn caption_buttons(workspace: &Workspace) -> Option<Box<dyn Element>> {
    let metrics = CaptionMetrics::of(workspace.control_layout(), workspace.window_chrome())?;
    let maximized = workspace.window_state().maximized;

    let row = Flex::row()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(metrics.spacing)
        .with_child(button(
            &metrics,
            Glyph::Minimize,
            workspace.caption().minimize.clone(),
            WindowAction::Minimize,
        ))
        .with_child(button(
            &metrics,
            // The control says what it would do, not what the window is: on a
            // maximised window it puts the window back.
            if maximized {
                Glyph::Restore
            } else {
                Glyph::Maximize
            },
            workspace.caption().maximize.clone(),
            WindowAction::ToggleMaximized,
        ))
        .with_child(button(
            &metrics,
            Glyph::Close,
            workspace.caption().close.clone(),
            WindowAction::Close,
        ));

    Some(
        // Fixed to the width the header reserved rather than left to the sum
        // of its parts: the reservation and the cluster are one number, and
        // this is where it is enforced rather than merely asserted.
        ConstrainedBox::new(
            Container::new(row.finish())
                .with_margin_left(metrics.lead)
                .with_margin_right(metrics.trail)
                .finish(),
        )
        .with_width(metrics.width())
        .finish(),
    )
}

/// What one platform's caption buttons measure.
///
/// Held together in one value because the sum of them is what the header
/// reserved: change any field and [`CaptionMetrics::width`] has to keep coming
/// out at [`ControlLayout::insets`]'s answer, which is what the tests check.
pub(super) struct CaptionMetrics {
    /// Room between the last header item and the first button.
    lead: f32,
    /// One button's box.
    size: Vector2F,
    /// Room between two buttons.
    spacing: f32,
    /// Room between the last button and the window's edge.
    trail: f32,
    /// The corner the button's fill is drawn with.
    radius: Radius,
}

impl CaptionMetrics {
    /// How this platform draws its caption buttons, or `None` where Crook does
    /// not draw them at all.
    pub(super) fn of(layout: ControlLayout, chrome: WindowChrome) -> Option<Self> {
        if matches!(chrome, WindowChrome::Native) {
            return None;
        }

        match layout {
            // AppKit's, over Crook's surface. Not ours to draw, and not ours
            // to move: `platform_insets` reserves the corner they land in.
            ControlLayout::MacOs => None,
            // Windows' caption buttons: wide, square, edge to edge, and hard
            // against the corner of the window.
            ControlLayout::Windows => Some(Self {
                lead: 1.,
                size: vec2f(45., BUTTON_HEIGHT),
                spacing: 0.,
                trail: 0.,
                radius: Radius::Pixels(0.),
            }),
            // GNOME's and KDE's: round, smaller, and inset from the corner.
            ControlLayout::Freedesktop => Some(Self {
                lead: 8.,
                size: vec2f(30., 30.),
                spacing: 5.,
                trail: 8.,
                radius: Radius::Percentage(50.),
            }),
        }
    }

    /// What the cluster costs the header, from the last header item to the
    /// window's edge.
    pub(super) fn width(&self) -> f32 {
        self.lead + self.size.x() * 3. + self.spacing * 2. + self.trail
    }

    /// How far down the window the cluster reaches.
    fn height(&self) -> f32 {
        self.size.y()
    }
}

/// The corner this build draws the window's own controls in, for the resize
/// border to keep out of.
///
/// `None` where Crook draws none — macOS, and a natively decorated window
/// anywhere. Measured from the same [`CaptionMetrics`] the cluster is drawn
/// from rather than written down again: the resize border runs *before* the
/// element tree sees a press, so a box that disagreed with the buttons by five
/// pixels would be five pixels of close button that resizes the window
/// instead — in the corner a person aims at without looking.
///
/// `layout` is a parameter rather than [`ControlLayout::host`] for the same
/// reason `--controls` exists: two thirds of this is a corner no one here can
/// press, and the only way to check that it covers the buttons is to lay the
/// other platforms out and measure them.
pub(crate) fn caption_area(layout: ControlLayout, chrome: WindowChrome) -> Option<Vector2F> {
    let metrics = CaptionMetrics::of(layout, chrome)?;
    Some(vec2f(metrics.width(), metrics.height()))
}

/// How tall a square caption button is.
///
/// The height of the tab beside it rather than the height of the header: the
/// header grows with its content and this sits on its bottom edge, so a button
/// that tried to fill it would need a number the layout only knows afterwards.
const BUTTON_HEIGHT: f32 = 30.;

/// The mark inside a caption button.
///
/// Two of them are rectangles rather than icons, because that is what they
/// are: every desktop draws minimise as a bar and maximise as a square, and
/// Lucide's versions of both are a stroked path around the same shape at a
/// different weight from the rest of this row.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Glyph {
    /// A bar along the bottom of the button.
    Minimize,
    /// An empty square.
    Maximize,
    /// Two of them, offset: what a maximised window's middle button does.
    Restore,
    /// A cross.
    Close,
}

/// The glyph's box, which every one of them is drawn inside.
const GLYPH_SIZE: f32 = 10.;

impl Glyph {
    /// The mark itself, in `color`.
    fn render(self, color: Color) -> Box<dyn Element> {
        match self {
            Self::Minimize => Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(GLYPH_SIZE)
                    .with_height(1.)
                    .finish(),
            )
            .with_background_color(color)
            .finish(),
            Self::Maximize => Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_width(GLYPH_SIZE)
                    .with_height(GLYPH_SIZE)
                    .finish(),
            )
            .with_border(Border::all(1.).with_border_color(color))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(1.)))
            .finish(),
            // Two overlapping rounded squares, which is what restore means
            // everywhere and what Lucide's `copy` already is.
            Self::Restore => Icon::new(Lucide::Copy, GLYPH_SIZE + 1.)
                .with_color(color)
                .finish(),
            Self::Close => Icon::new(Lucide::X, GLYPH_SIZE + 2.)
                .with_color(color)
                .finish(),
        }
    }
}

/// One caption button: its fill, its glyph, and what pressing it asks of the
/// window.
fn button(
    metrics: &CaptionMetrics,
    glyph: Glyph,
    state: MouseStateHandle,
    action: WindowAction,
) -> Box<dyn Element> {
    let (size, radius) = (metrics.size, metrics.radius);
    let idle_fill = if radius == Radius::Percentage(50.) {
        // A round button is a shape whether or not the pointer is on it, the
        // way GNOME draws them; a square one is a region of the header and
        // shows itself only when it is about to be used, the way Windows does.
        theme().overlay_1
    } else {
        Color::TRANSPARENT
    };

    Hoverable::new(state, move |mouse| {
        // Closing is the one irreversible thing in the row, and every desktop
        // says so in red. Not a red *plate* with a light mark on it: nothing
        // in a palette says what reads on top of a fill, and the tint is the
        // one form of it that is right in a light theme and a dark one.
        let danger = glyph == Glyph::Close;
        let (fill, mark) = match (mouse.is_clicked(), mouse.is_hovered(), danger) {
            (true, _, true) => (
                theme().usage_critical.with_alpha(90),
                theme().usage_critical,
            ),
            (true, _, false) => (theme().overlay_3, theme().text_primary),
            (_, true, true) => (
                theme().usage_critical.with_alpha(56),
                theme().usage_critical,
            ),
            (_, true, false) => (theme().overlay_2, theme().text_primary),
            _ => (idle_fill, theme().text_muted),
        };

        ConstrainedBox::new(
            Container::new(Align::new(glyph.render(mark)).finish())
                .with_background_color(fill)
                .with_corner_radius(CornerRadius::with_all(radius))
                .finish(),
        )
        .with_width(size.x())
        .with_height(size.y())
        .finish()
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(WorkspaceAction::Window(action)))
    .finish()
}

/// A row whose empty space moves the window.
///
/// Not a [`Hoverable`]: this needs the press itself, and it needs it only when
/// nothing else wanted it. `Hoverable` answers a *click*, which is a press and
/// a release in the same place — by which time a window drag has already had
/// to start — and it claims a press whether or not its child took it first.
struct DragToMove {
    child: Box<dyn Element>,
    origin: Option<Point>,
    /// The topmost layer the child painted into, so a press on something drawn
    /// *over* the header — the options menu, a tooltip — is not a press on the
    /// header.
    child_max_z_index: Option<ZIndex>,
}

impl Element for DragToMove {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.child.layout(constraint, ctx, app)
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
        self.child_max_z_index = Some(ctx.scene.max_active_z_index());
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        // The children first, and their answer is what "empty space" means.
        if self.child.dispatch_event(event, ctx, app) {
            return true;
        }

        let (Some(origin), Some(size), Some(z_index)) =
            (self.origin, self.child.size(), self.child_max_z_index)
        else {
            // Before the first paint there is nothing to pick up.
            return false;
        };

        let Some(Event::MouseDown {
            button: MouseButton::Left,
            position,
            click_count,
            ..
        }) = event.at_z_index(z_index, ctx)
        else {
            return false;
        };

        if !ctx
            .visible_rect(origin, size)
            .is_some_and(|visible| visible.contains_point(*position))
        {
            return false;
        }

        // The second press of a series and only the second. `click_count` goes
        // on counting for as long as the presses stay inside half a second and
        // four pixels of each other, so `>= 2` made every press after a double
        // click another maximise: reach straight for the title bar to move the
        // window you have just maximised and it would restore and stay put,
        // because no drag was ever started.
        ctx.dispatch_typed_action(WorkspaceAction::Window(if *click_count == 2 {
            WindowAction::ToggleMaximized
        } else {
            WindowAction::Drag
        }));
        true
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cluster_is_exactly_as_wide_as_the_header_reserved_for_it() {
        // The two halves of one number, written down in two modules because
        // one of them has to be decided before there is a header to measure.
        // Drift either way is visible on a platform nobody here can look at:
        // wider and the close button sits on the usage chip, narrower and the
        // header ends in a hole.
        for layout in [ControlLayout::Windows, ControlLayout::Freedesktop] {
            let metrics =
                CaptionMetrics::of(layout, WindowChrome::Client).expect("{layout:?} draws its own");
            assert_eq!(
                metrics.width(),
                layout.insets(WindowChrome::Client, false).right,
                "{layout:?} draws a cluster that is not the width it reserved"
            );
        }
    }

    #[test]
    fn a_platform_whose_controls_are_not_ours_draws_none() {
        // macOS's traffic lights are AppKit's, and a natively decorated window
        // has controls in a bar of its own on every platform. Drawing a second
        // set in either case is two close buttons on one window.
        assert!(CaptionMetrics::of(ControlLayout::MacOs, WindowChrome::Client).is_none());
        for layout in [
            ControlLayout::MacOs,
            ControlLayout::Windows,
            ControlLayout::Freedesktop,
        ] {
            assert!(CaptionMetrics::of(layout, WindowChrome::Native).is_none());
        }
    }

    #[test]
    fn the_corner_kept_from_the_resize_border_is_the_cluster_itself() {
        // Two numbers that have to be one: the border consumes a press before
        // the element tree sees it, so a corner smaller than the cluster is a
        // close button that resizes the window, and a larger one is a strip of
        // header the window cannot be resized from.
        for layout in [
            ControlLayout::MacOs,
            ControlLayout::Windows,
            ControlLayout::Freedesktop,
        ] {
            let area = caption_area(layout, WindowChrome::Client);
            match CaptionMetrics::of(layout, WindowChrome::Client) {
                Some(metrics) => {
                    assert_eq!(
                        area,
                        Some(vec2f(metrics.width(), metrics.height())),
                        "{layout:?}"
                    );
                }
                None => assert_eq!(
                    area, None,
                    "{layout:?} draws no controls and keeps no corner"
                ),
            }
            assert_eq!(
                caption_area(layout, WindowChrome::Native),
                None,
                "{layout:?} with a frame the system draws has no corner of Crook's to keep"
            );
        }
    }

    #[test]
    fn every_button_fits_inside_the_cluster_it_is_drawn_in() {
        // The buttons are laid out by a flex from these numbers, so the check
        // that matters is that the parts add up to the whole: three boxes, two
        // gaps and the two margins are the width, with nothing left over to
        // overflow the reservation.
        for layout in [ControlLayout::Windows, ControlLayout::Freedesktop] {
            let metrics = CaptionMetrics::of(layout, WindowChrome::Client).expect("draws its own");
            let buttons = metrics.size.x() * 3. + metrics.spacing * 2.;

            assert!(
                buttons <= metrics.width(),
                "{layout:?}'s buttons are {buttons} wide inside {} of room",
                metrics.width()
            );
            assert!(
                metrics.size.x() >= GLYPH_SIZE && metrics.size.y() >= GLYPH_SIZE,
                "{layout:?}'s button is smaller than the mark in it"
            );
        }
    }
}
