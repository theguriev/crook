//! Crook's renderer-agnostic UI core.
//!
//! This crate is the whole application framework minus the pixels: entities and
//! handles, views and elements, layout, the scene, input. It depends on no GPU
//! API, no window system and no font library — the two traits in
//! [`platform`] are the entire seam — so it builds anywhere and its tests run
//! headless.
//!
//! # How a frame happens
//!
//! 1. Something mutates a model or a view and calls `ctx.notify()`.
//! 2. The effect queue drains, and the view's id lands in the window's
//!    [`WindowInvalidation`].
//! 3. The platform crate hands that invalidation to a [`Presenter`], which
//!    re-runs `View::render` for exactly those views.
//! 4. [`Presenter::build_scene`] lays the tree out against the window size and
//!    paints it into a [`Scene`] of rectangles and glyph references.
//! 5. The renderer walks the scene's layers.
//!
//! Nothing polls. A view that changes without notifying shows stale content
//! and reports no error — that is the one bug class this design trades for its
//! simplicity, and it is worth knowing about before writing a view.
//!
//! # Where things live
//!
//! * The crate root — [`App`], [`AppContext`], handles, contexts, actions,
//!   focus.
//! * [`element`] and [`elements`] — the render tree and its primitives.
//! * [`presenter`] — the layout, paint and event walks.
//! * [`scene`] — what a frame compiles down to.
//! * [`geometry`], [`fonts`], [`text_layout`], [`event`] — the vocabulary.
//! * [`platform`] — what a renderer must implement.
//! * [`executor`] — the two executors the app runs on.

mod core;

pub mod element;
pub mod elements;
pub mod event;
pub mod executor;
pub mod fonts;
pub mod geometry;
pub mod platform;
pub mod prelude;
pub mod presenter;
pub mod scene;
pub mod text_layout;

pub use element::{Element, ParentElement, SizeConstraint};
pub use event::Event;
pub use geometry::{Color, RectF, Vector2F, vec2f};
pub use presenter::{EventContext, LayoutContext, PaintContext, Presenter};
pub use scene::{ClipBounds, Scene};

pub use crate::core::*;
