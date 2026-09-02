//! Crook's platform layer: the one crate that knows about winit, wgpu and fonts.
//!
//! [`crookui_core`] is the whole application framework minus the pixels, and it
//! names its dependency on a platform in exactly two traits —
//! [`FontDb`](crookui_core::platform::FontDb) and
//! [`TextLayoutSystem`](crookui_core::platform::TextLayoutSystem). This crate
//! implements both, and supplies the two things a trait cannot express: a
//! window to draw into and a GPU to draw with.
//!
//! That containment is the point. `winit` and `wgpu` are named in this crate's
//! `Cargo.toml` and nowhere else in the workspace, and no type from either
//! appears in a signature the application crate calls. Replacing the renderer
//! means rewriting this crate, not auditing the app.
//!
//! # The three modules
//!
//! * [`fonts`] — [`CosmicFontDb`] and [`CosmicTextLayout`], over `cosmic-text`,
//!   `fontdb` and `swash`. Finds system fonts with no linked library, which is
//!   what keeps the workspace free of build scripts on all three platforms.
//! * [`rendering`] — a [`Scene`](crookui_core::scene::Scene) to pixels, as two
//!   instanced-quad pipelines over a single render pass. The same renderer
//!   draws to a texture instead of a window in
//!   [`rendering::offscreen`], which is how it is tested without a display.
//! * [`windowing`] — the winit event loop, and the [`WindowDelegate`] seam the
//!   application implements.
//!
//! # Opening a window
//!
//! ```no_run
//! use std::rc::Rc;
//!
//! use crookui::{CosmicFontDb, WindowDelegate, WindowOptions};
//! use crookui_core::event::Event;
//! use crookui_core::geometry::{Vector2F, vec2f};
//! use crookui_core::scene::Scene;
//!
//! struct Ui {
//!     pointer: Vector2F,
//! }
//!
//! impl WindowDelegate for Ui {
//!     fn build_scene(&mut self, size: Vector2F, scale_factor: f32) -> Rc<Scene> {
//!         let mut scene = Scene::new(scale_factor);
//!         scene.draw_rect_with_hit_recording(crookui_core::RectF::new(Vector2F::zero(), size));
//!         Rc::new(scene)
//!     }
//!
//!     fn handle_event(&mut self, event: Event) -> bool {
//!         let Event::MouseMoved { position, .. } = event else {
//!             return false;
//!         };
//!         let moved = position != self.pointer;
//!         self.pointer = position;
//!         moved
//!     }
//!
//!     fn frame_drawn(&mut self) {}
//! }
//!
//! # fn main() -> anyhow::Result<()> {
//! let font_db = CosmicFontDb::new()?;
//! let options = WindowOptions {
//!     title: "Crook".to_owned(),
//!     size: vec2f(960., 600.),
//!     ..Default::default()
//! };
//!
//! crookui::windowing::run(options, Box::new(font_db), |_| {
//!     Box::new(Ui {
//!         pointer: Vector2F::zero(),
//!     })
//! })
//! # }
//! ```
//!
//! # Rendering without a window
//!
//! ```no_run
//! use crookui::{CosmicFontDb, render_scene_to_rgba};
//! use crookui_core::geometry::vec2f;
//! use crookui_core::scene::Scene;
//!
//! # fn main() -> anyhow::Result<()> {
//! let font_db = CosmicFontDb::new()?;
//! let scene = Scene::new(2.);
//! let (rgba, width, height) = render_scene_to_rgba(&scene, vec2f(400., 300.), &font_db)?;
//! assert_eq!(rgba.len(), (width * height * 4) as usize);
//! # Ok(())
//! # }
//! ```
//!
//! `examples/scene_to_png.rs` is that call against a scene built by hand, and
//! `examples/window.rs` is the same scene in a real window.

#![deny(missing_docs)]

pub mod fonts;
pub mod rendering;
pub mod windowing;

pub use fonts::{CosmicFontDb, CosmicTextLayout};
pub use rendering::render_scene_to_rgba;
pub use windowing::{Platform, Proxy, WindowDelegate, WindowOptions, run};
