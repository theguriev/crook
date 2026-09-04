//! The window system: a winit event loop, one window, and the seam above it.
//!
//! Everything winit is confined to this module and never appears in a signature
//! the application crate sees. What it sees instead is [`WindowDelegate`] —
//! build a scene, handle an event, note that a frame was drawn — plus
//! [`Platform`], which hands it the main-thread executor and a handle for
//! waking that thread from anywhere else.
//!
//! # Opening a window
//!
//! [`run`] takes the window's [`WindowOptions`], a boxed
//! [`FontDb`](crookui_core::platform::FontDb), and a closure that builds the
//! delegate. The closure runs after the event loop and the executor exist but
//! before the window does, which is why it receives a [`Platform`] rather than
//! constructing one: an application that spawns work during startup needs
//! somewhere to spawn it, and a window that fails to open should not have
//! required the application to be built first.
//!
//! [`run`] returns when the window closes.
//!
//! # A window the application decorates itself
//!
//! [`WindowChrome::Client`] moves the window's controls onto the application's
//! own first row, and [`chrome`] is where the consequences live: what to ask
//! the platform for, what a frameless window has to do about its own resize
//! edges, and [`WindowControls`], the four verbs and two questions the
//! application needs to move a window it is drawing the title bar of.

mod app;
mod chrome;
mod event;
mod window;

pub use app::{Platform, Proxy, WindowDelegate, WindowOptions, run};
pub use chrome::{ResizeEdge, WindowChrome, WindowControls};
