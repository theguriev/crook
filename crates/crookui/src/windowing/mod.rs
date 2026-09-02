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

mod app;
mod event;
mod window;

pub use app::{Platform, Proxy, WindowDelegate, WindowOptions, run};
