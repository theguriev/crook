//! Who draws the window's frame, and what the application owes if it is Crook.
//!
//! A window is either the window manager's to decorate or the application's,
//! and [`WindowChrome`] is the whole of that choice. What is not obvious is
//! that "the application's" means something different on macOS than it does
//! anywhere else. AppKit will make the title bar transparent and let the
//! content view reach the top of the window while leaving the traffic lights
//! exactly where every other Mac application has them; turning decorations
//! *off* there would take the buttons away instead, and no Mac application
//! draws its own. So one value of this enum becomes a borderless window on
//! Windows and Linux and a still-decorated one on macOS.
//!
//! Everything else here follows from that split. A window that keeps its frame
//! is still moved, resized and closed by the window manager; a frameless one
//! has to do all three itself, which is what [`WindowControls`] and
//! [`ResizeEdge`] are for.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crookui_core::geometry::Vector2F;
use winit::window::{CursorIcon, ResizeDirection, Window, WindowAttributes};

/// Who draws the window's title bar and the controls in it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum WindowChrome {
    /// The window manager, in a bar of its own above the client area.
    ///
    /// The application's first row is a row like any other: nothing is over
    /// it, and nothing about the window is its business.
    #[default]
    Native,
    /// The application, in its own first row.
    ///
    /// The window's controls are drawn over that row — by the system on macOS,
    /// which keeps its traffic lights, and by the application everywhere else,
    /// where the window has no frame at all.
    Client,
}

impl WindowChrome {
    /// Whether the window has no frame of the window manager's: no controls,
    /// no resize border, and nothing to drag it by.
    ///
    /// Not simply "is it `Client`". On macOS a client-decorated window keeps
    /// its frame — a transparent title bar over a full-size content view, with
    /// the system's own buttons in it — so everything a frameless window has
    /// to do for itself is already being done.
    pub(super) const fn is_frameless(self) -> bool {
        matches!(self, Self::Client) && !cfg!(target_os = "macos")
    }
}

/// Adds `chrome` to the attributes a window is about to be created with.
#[cfg(target_os = "macos")]
pub(super) fn with_chrome(attributes: WindowAttributes, chrome: WindowChrome) -> WindowAttributes {
    use winit::platform::macos::WindowAttributesExtMacOS as _;

    match chrome {
        WindowChrome::Native => attributes.with_decorations(true),
        // Decorations stay on, and that is the entire trick: the buttons are
        // part of the decoration, and a window without them is not what any
        // Mac application looks like. What goes is the *bar* — transparent,
        // titleless, with the content view extended up underneath it — so the
        // application's own surface is what the traffic lights sit on.
        WindowChrome::Client => attributes
            .with_decorations(true)
            .with_titlebar_transparent(true)
            .with_title_hidden(true)
            .with_fullsize_content_view(true),
    }
}

/// Adds `chrome` to the attributes a window is about to be created with.
#[cfg(target_os = "windows")]
pub(super) fn with_chrome(attributes: WindowAttributes, chrome: WindowChrome) -> WindowAttributes {
    use winit::platform::windows::WindowAttributesExtWindows as _;

    // Windows has no equivalent of macOS's transparent title bar, so a
    // client-decorated window here is a borderless one and the application
    // draws the controls and finds the resize edges itself. What it has to ask
    // for back is the shadow: DWM hides it for an undecorated window, and
    // without it there is nothing whatsoever between Crook and what is behind
    // it — no frame, no border, and no edge at all against another dark
    // window. The one pixel of line it brings along at the top is the border
    // every Windows 11 window has.
    let decorated = matches!(chrome, WindowChrome::Native);
    attributes
        .with_decorations(decorated)
        .with_undecorated_shadow(!decorated)
}

/// What a Linux desktop calls this application.
///
/// Wayland's `app_id` and X11's `WM_CLASS`, which are the same fact under two
/// names: it is what a window rule matches, what a dock groups by, what an
/// alt-tab list labels, and what a `.desktop` file is tied to. A window
/// without one is a window a person cannot write a rule for — and this had
/// none, because winit only sets it when asked and nothing asked.
///
/// One name for both channels. The dev build says so in its *title*, which is
/// what a person reads; the id is what their configuration matches, and a rule
/// that stopped working because they ran a different build of the same
/// application would be a rule nobody could debug.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const APPLICATION: &str = "crook";

/// Adds `chrome` to the attributes a window is about to be created with.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) fn with_chrome(attributes: WindowAttributes, chrome: WindowChrome) -> WindowAttributes {
    // Wayland's extension, and it is not only Wayland's: `with_name` writes
    // one field, which the X11 backend reads as `WM_CLASS` and the Wayland one
    // as `app_id`. Which display server is in force is the session's business
    // rather than this line's.
    use winit::platform::wayland::WindowAttributesExtWayland;

    // No Linux compositor has macOS's arrangement either, so a
    // client-decorated window here is a borderless one and the application
    // draws the controls and finds the resize edges itself. The shadow around
    // it is the compositor's own business and there is nothing to ask for.
    let attributes = attributes.with_decorations(matches!(chrome, WindowChrome::Native));

    WindowAttributesExtWayland::with_name(attributes, APPLICATION, APPLICATION)
}

/// How far into a frameless window a press still counts as grabbing its edge,
/// in logical pixels.
///
/// Wider than the hairline it looks like, because the border is invisible and
/// what a person aims at is the window's outline, and narrow enough that what
/// it costs is a strip of somebody's padding rather than a control. Nothing an
/// application draws is allowed in it, which is why the header's own controls
/// are inset from the edges of the window.
pub(super) const RESIZE_GRAB: f32 = 5.;

/// Which edge or corner of a frameless window a drag is resizing.
///
/// Crook's own enum rather than winit's, because this crosses the seam: the
/// windowing layer is the only place a winit type may be named, and the eight
/// zones are geometry that is worth testing on a machine that has no window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResizeEdge {
    /// The top edge.
    North,
    /// The top-right corner.
    NorthEast,
    /// The right edge.
    East,
    /// The bottom-right corner.
    SouthEast,
    /// The bottom edge.
    South,
    /// The bottom-left corner.
    SouthWest,
    /// The left edge.
    West,
    /// The top-left corner.
    NorthWest,
}

impl ResizeEdge {
    /// The same edge, as winit says it.
    const fn direction(self) -> ResizeDirection {
        match self {
            Self::North => ResizeDirection::North,
            Self::NorthEast => ResizeDirection::NorthEast,
            Self::East => ResizeDirection::East,
            Self::SouthEast => ResizeDirection::SouthEast,
            Self::South => ResizeDirection::South,
            Self::SouthWest => ResizeDirection::SouthWest,
            Self::West => ResizeDirection::West,
            Self::NorthWest => ResizeDirection::NorthWest,
        }
    }

    /// The pointer that says which way this edge moves.
    const fn cursor(self) -> CursorIcon {
        match self {
            Self::North => CursorIcon::NResize,
            Self::NorthEast => CursorIcon::NeResize,
            Self::East => CursorIcon::EResize,
            Self::SouthEast => CursorIcon::SeResize,
            Self::South => CursorIcon::SResize,
            Self::SouthWest => CursorIcon::SwResize,
            Self::West => CursorIcon::WResize,
            Self::NorthWest => CursorIcon::NwResize,
        }
    }
}

/// Which resize zone `position` is in, for a window of `size`.
///
/// Corners win over edges, which is what makes a corner grabbable at all: the
/// two `grab`-wide strips overlap there, and a person aiming at the corner of
/// a window means the corner.
///
/// Every zone belongs to the window and nothing else: this runs before the
/// element tree sees a press, so an application that draws a control against
/// the edge of a frameless window loses its outermost pixels to a resize.
pub(super) fn edge_at(position: Vector2F, size: Vector2F, grab: f32) -> Option<ResizeEdge> {
    let inside =
        (0. ..=size.x()).contains(&position.x()) && (0. ..=size.y()).contains(&position.y());
    if !inside {
        return None;
    }

    let north = position.y() <= grab;
    let south = position.y() >= size.y() - grab;
    let west = position.x() <= grab;
    let east = position.x() >= size.x() - grab;

    Some(match (north, south, west, east) {
        (true, _, true, _) => ResizeEdge::NorthWest,
        (true, _, _, true) => ResizeEdge::NorthEast,
        (_, true, true, _) => ResizeEdge::SouthWest,
        (_, true, _, true) => ResizeEdge::SouthEast,
        (true, ..) => ResizeEdge::North,
        (_, true, ..) => ResizeEdge::South,
        (_, _, true, _) => ResizeEdge::West,
        (_, _, _, true) => ResizeEdge::East,
        _ => return None,
    })
}

/// The window itself, as much of it as the application is allowed to touch.
///
/// Cheap to clone, and every method is a no-op until a window is attached:
/// that is what lets the application be built before the window exists — which
/// it must be, since building it is what the event loop calls before opening
/// one — and what lets the headless paths hold a handle to nothing.
///
/// No winit type appears in any signature here. What crosses the seam is four
/// verbs and two questions.
#[derive(Clone, Default)]
pub struct WindowControls(Rc<RefCell<State>>);

#[derive(Default)]
struct State {
    window: Option<Arc<Window>>,
    /// Whether a gesture has just handed the pointer to the window manager.
    ///
    /// A move or a resize runs inside the system's own event loop until the
    /// button comes up, and that release is delivered to the *window manager*,
    /// not to us: winit reports the press and then nothing. Left alone, the
    /// input state goes on believing the button is held and turns every later
    /// move into a drag. See [`Self::take_gesture_started`].
    gesture_started: bool,
    /// The resize cursor currently set, so the pointer is only changed when it
    /// actually changes — a mouse move is a common event and setting a cursor
    /// is a call into the window system.
    cursor: Option<ResizeEdge>,
}

impl WindowControls {
    /// Starts moving the window with the pointer.
    pub fn start_drag(&self) {
        self.gesture(|window| window.drag_window(), "move");
    }

    /// Maximises the window, or restores it if it is already maximised.
    pub fn toggle_maximized(&self) {
        let state = self.0.borrow();
        if let Some(window) = state.window.as_ref() {
            window.set_maximized(!window.is_maximized());
        }
    }

    /// Sends the window to the dock, the taskbar or wherever this desktop
    /// keeps minimised windows.
    pub fn minimize(&self) {
        let state = self.0.borrow();
        if let Some(window) = state.window.as_ref() {
            window.set_minimized(true);
        }
    }

    /// Whether the window fills the work area.
    pub fn is_maximized(&self) -> bool {
        let state = self.0.borrow();
        state
            .window
            .as_ref()
            .is_some_and(|window| window.is_maximized())
    }

    /// Whether the window has taken over the whole screen.
    ///
    /// True however it got there, including the macOS green button, which the
    /// application never hears about otherwise — and has to, because macOS
    /// moves the traffic lights out of the window in fullscreen and anything
    /// reserving room for them has to give it back.
    pub fn is_fullscreen(&self) -> bool {
        let state = self.0.borrow();
        state
            .window
            .as_ref()
            .is_some_and(|window| window.fullscreen().is_some())
    }

    /// Hands over the window these controls act on.
    pub(super) fn attach(&self, window: Arc<Window>) {
        self.0.borrow_mut().window = Some(window);
    }

    /// Starts resizing the window from `edge`.
    pub(super) fn start_resize(&self, edge: ResizeEdge) {
        self.gesture(
            |window| window.drag_resize_window(edge.direction()),
            "resize",
        );
    }

    /// Shows the pointer for `edge`, or the ordinary one for `None`.
    pub(super) fn set_resize_cursor(&self, edge: Option<ResizeEdge>) {
        let mut state = self.0.borrow_mut();
        if state.cursor == edge {
            return;
        }
        state.cursor = edge;
        if let Some(window) = state.window.as_ref() {
            window.set_cursor(edge.map_or(CursorIcon::Default, ResizeEdge::cursor));
        }
    }

    /// Whether a gesture was started since this was last asked, which means
    /// the press that started it will never be released to us.
    pub(super) fn take_gesture_started(&self) -> bool {
        std::mem::take(&mut self.0.borrow_mut().gesture_started)
    }

    fn gesture(
        &self,
        start: impl FnOnce(&Window) -> Result<(), winit::error::ExternalError>,
        name: &str,
    ) {
        let mut state = self.0.borrow_mut();
        let Some(window) = state.window.as_ref() else {
            return;
        };

        match start(window) {
            Ok(()) => state.gesture_started = true,
            // Not fatal and not rare: a compositor may refuse, and macOS
            // refuses when there is no current event to drag with.
            Err(error) => log::warn!("the window manager would not {name} the window: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crookui_core::geometry::vec2f;

    use super::*;

    const SIZE: Vector2F = vec2f(800., 600.);
    const GRAB: f32 = 5.;

    #[test]
    fn each_of_the_eight_zones_answers_with_its_own_edge() {
        // Walked as a table so the four corners cannot quietly become four
        // edges: on the corner of a window, both strips are true, and a
        // `match` that tested the edges first would resize one axis of a
        // gesture that meant two.
        let cases = [
            (vec2f(400., 1.), ResizeEdge::North),
            (vec2f(799., 1.), ResizeEdge::NorthEast),
            (vec2f(799., 300.), ResizeEdge::East),
            (vec2f(799., 599.), ResizeEdge::SouthEast),
            (vec2f(400., 599.), ResizeEdge::South),
            (vec2f(1., 599.), ResizeEdge::SouthWest),
            (vec2f(1., 300.), ResizeEdge::West),
            (vec2f(1., 1.), ResizeEdge::NorthWest),
        ];

        for (position, expected) in cases {
            assert_eq!(
                edge_at(position, SIZE, GRAB),
                Some(expected),
                "{position:?} is not the {expected:?} zone"
            );
        }
    }

    #[test]
    fn the_middle_of_the_window_is_not_an_edge() {
        assert_eq!(edge_at(vec2f(400., 300.), SIZE, GRAB), None);
        // One pixel inside the grab strip on every side.
        assert_eq!(edge_at(vec2f(GRAB + 1., GRAB + 1.), SIZE, GRAB), None);
        assert_eq!(
            edge_at(
                vec2f(SIZE.x() - GRAB - 1., SIZE.y() - GRAB - 1.),
                SIZE,
                GRAB
            ),
            None
        );
    }

    #[test]
    fn a_pointer_outside_the_window_is_on_no_edge_at_all() {
        // A drag that left the window still reports positions, and they are
        // outside it. Answering `West` for x = -40 would start a resize the
        // moment the pointer came back over the title bar.
        for position in [
            vec2f(-40., 300.),
            vec2f(300., -40.),
            vec2f(SIZE.x() + 40., 300.),
            vec2f(300., SIZE.y() + 40.),
        ] {
            assert_eq!(edge_at(position, SIZE, GRAB), None, "{position:?}");
        }
    }

    #[test]
    fn the_top_right_corner_is_an_edge_like_any_other() {
        // It was not always: the application used to draw three caption
        // buttons there and the border was told to keep out of the box they
        // were in. Nothing is drawn in that corner now, so a window that gave
        // it up would be a window that cannot be resized from the corner every
        // other application resizes from.
        assert_eq!(
            edge_at(vec2f(SIZE.x() - 1., 1.), SIZE, GRAB),
            Some(ResizeEdge::NorthEast)
        );
        assert_eq!(
            edge_at(vec2f(SIZE.x() - 1., 30.), SIZE, GRAB),
            Some(ResizeEdge::East)
        );
    }

    #[test]
    fn a_window_with_a_native_frame_is_never_frameless_and_macos_never_is() {
        assert!(!WindowChrome::Native.is_frameless());
        assert_eq!(
            WindowChrome::Client.is_frameless(),
            !cfg!(target_os = "macos"),
            "macOS keeps its frame under client chrome; nothing else does"
        );
    }

    #[test]
    fn controls_with_no_window_do_nothing_rather_than_panicking() {
        // The state every headless path is in, and the state the real one is
        // in between building the application and opening the window.
        let controls = WindowControls::default();

        controls.start_drag();
        controls.toggle_maximized();
        controls.minimize();
        controls.start_resize(ResizeEdge::SouthEast);
        controls.set_resize_cursor(Some(ResizeEdge::North));

        assert!(!controls.is_maximized());
        assert!(!controls.is_fullscreen());
        assert!(
            !controls.take_gesture_started(),
            "a gesture that never reached a window must not eat the next release"
        );
    }
}
