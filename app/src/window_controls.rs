//! The window, as much of it as a view is allowed to touch.
//!
//! Crook's header *is* the window's title bar
//! ([`WINDOW_CHROME`](crate::WINDOW_CHROME)), which means the header has to do
//! the things a title bar does: move the window when it is dragged, maximise
//! it when it is double-clicked, and — on the platforms where a client-drawn
//! title bar means a window with no frame at all — minimise and restore it
//! from buttons Crook draws itself.
//!
//! None of that can be a call into the windowing layer from the view, for the
//! same reason [`QuitRequest`](crate::workspace::QuitRequest) is a callback:
//! nothing in [`workspace`](crate::workspace) names `crookui`, so the headless
//! snapshot and every test run the same code path with nothing behind it. This
//! is that seam, one trait wide.
//!
//! # Why the state is asked for rather than pushed
//!
//! [`WindowState`] is read on the render path, every frame, rather than
//! delivered as an event. A window's own state changes for reasons the
//! application never hears about — the macOS green button, a tiling
//! compositor, a keyboard shortcut belonging to the desktop — and an
//! application that cached what it last set would draw a maximise button on a
//! maximised window. What *is* pushed is the repaint: the shell compares this
//! between frames and notifies the workspace when it moved, which is what gets
//! the new answer onto the screen.

use std::rc::Rc;

/// What the window is doing that changes what the header must draw.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowState {
    /// Whether the window fills the work area, so the maximise control is
    /// really a restore control.
    pub maximized: bool,
    /// Whether the window has taken over the screen.
    ///
    /// The one that reaches [`platform_insets`](crate::platform_insets):
    /// macOS moves the traffic lights into the menu-bar overlay in fullscreen,
    /// and a header still holding room for them has a hole where they were.
    pub fullscreen: bool,
}

/// What the header asks of the window it is the title bar of.
pub trait WindowControls {
    /// What the window is doing right now.
    fn state(&self) -> WindowState;

    /// Starts moving the window with the pointer, until it is let go.
    fn start_drag(&self);

    /// Maximises the window, or restores it if it is already maximised.
    fn toggle_maximized(&self);

    /// Sends the window wherever this desktop keeps minimised windows.
    fn minimize(&self);
}

/// Shared ownership of one, held by the workspace across renders.
pub type WindowHandle = Rc<dyn WindowControls>;

/// A window that is not there.
///
/// What the headless paths hold: `--snapshot` renders the real view tree with
/// no window behind it, and a test window has no frame to move. Every verb is
/// a no-op and the state is the one a window that cannot be maximised is in,
/// so the tree that renders is the tree a fresh window renders.
pub struct Detached;

impl WindowControls for Detached {
    fn state(&self) -> WindowState {
        WindowState::default()
    }

    fn start_drag(&self) {}

    fn toggle_maximized(&self) {}

    fn minimize(&self) {}
}

/// A window that only remembers what it was asked.
///
/// A test's window: `start_drag` cannot be observed by looking at a frame, so
/// the only way to test that pressing the header's empty space drags the
/// window — and that pressing a *control* in it does not — is to ask the
/// window afterwards what it was told.
#[cfg(test)]
#[derive(Default)]
pub struct Recorder {
    /// What has been asked of it, in order.
    pub asked: std::cell::RefCell<Vec<Request>>,
    /// What it answers when asked what the window is doing.
    pub state: std::cell::Cell<WindowState>,
}

/// One thing a [`Recorder`] was asked to do.
#[cfg(test)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Move the window.
    Drag,
    /// Maximise it, or put it back.
    ToggleMaximized,
    /// Minimise it.
    Minimize,
}

#[cfg(test)]
impl Recorder {
    /// Everything it has been asked to do so far.
    pub fn requests(&self) -> Vec<Request> {
        self.asked.borrow().clone()
    }
}

#[cfg(test)]
impl WindowControls for Recorder {
    fn state(&self) -> WindowState {
        self.state.get()
    }

    fn start_drag(&self) {
        self.asked.borrow_mut().push(Request::Drag);
    }

    fn toggle_maximized(&self) {
        self.asked.borrow_mut().push(Request::ToggleMaximized);
    }

    fn minimize(&self) {
        self.asked.borrow_mut().push(Request::Minimize);
    }
}
