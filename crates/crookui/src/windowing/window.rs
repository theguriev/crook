//! One window: its winit handle, its GPU resources, and the frame it draws.
//!
//! The GPU half sits behind an `Option` on purpose. A lost device is a normal
//! event — a driver reset, a laptop switching GPUs, a Wayland compositor
//! restarting — and recovering from one means dropping every wgpu object and
//! building fresh ones against the same, still-live, winit window. Designing
//! that in costs an `Option` and two methods; retrofitting it into a renderer
//! that assumed an infallible device costs a rewrite.
//!
//! Ported from Warp's `crates/warpui/src/windowing/winit/window.rs` (MIT),
//! keeping `open_window`, `update_size_if_needed`, `render`, `drop_renderer`
//! and `recreate_renderer`, and dropping the ~1,500 lines of window management
//! around them.

use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result};
use crookui_core::event::SystemTheme;
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform::FontDb;
use crookui_core::scene::Scene;
use wgpu::wgt::WgpuHasDisplayHandle;
use winit::event_loop::ActiveEventLoop;

use crate::rendering::{Error, Renderer, WindowResources, reset_wgpu_instance};

use super::event::system_theme;

use super::WindowOptions;
use super::chrome::with_chrome;

/// The wgpu half of a window, which can be thrown away and rebuilt.
struct RenderingResources {
    resources: WindowResources,
    renderer: Renderer,
}

/// A single application window.
pub(super) struct Window {
    window: Arc<winit::window::Window>,
    rendering: Option<RenderingResources>,

    /// The frame drawn last, kept so a redraw the compositor asked for — a
    /// window uncovered, a workspace switched back to — reuses it instead of
    /// laying the whole tree out again. [`Self::request_redraw`] drops it,
    /// which is how the application says its state has actually changed.
    scene: Option<Rc<Scene>>,

    /// The size the swapchain is currently configured at, in physical pixels.
    surface_size: Vector2F,

    /// Set by a resize, cleared by the reconfigure it causes.
    surface_requires_reconfiguration: bool,
}

impl Window {
    /// Creates the winit window and everything needed to draw into it.
    pub(super) fn new(event_loop: &ActiveEventLoop, options: &WindowOptions) -> Result<Self> {
        let attributes = with_chrome(winit::window::Window::default_attributes(), options.chrome)
            .with_title(options.title.clone())
            .with_inner_size(winit::dpi::LogicalSize::new(
                options.size.x() as f64,
                options.size.y() as f64,
            ))
            .with_min_inner_size(winit::dpi::LogicalSize::new(
                options.min_size.x() as f64,
                options.min_size.y() as f64,
            ))
            .with_transparent(options.transparent);

        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("failed to create the window")?,
        );

        // Without this the platform never starts a composition, and the keys
        // that would have begun one arrive as themselves — which is a window
        // that cannot type Japanese, Chinese or Korean at all. It costs
        // nothing on a machine with no input method: no composition ever
        // begins, and no `Ime` event is ever sent.
        window.set_ime_allowed(true);

        // The candidate list starts under the top-left corner, which is where
        // a caret that has never been drawn is. The first painted frame moves
        // it.

        // A surface may never be zero-sized, and a window that has not been
        // mapped yet can report exactly that.
        let surface_size = physical_size(&window).max(Vector2F::splat(1.));
        let resources = WindowResources::new(window.clone(), surface_size)?;
        let renderer = Renderer::new(&resources.resources, resources.format());

        Ok(Self {
            window,
            rendering: Some(RenderingResources {
                resources,
                renderer,
            }),
            scene: None,
            surface_size,
            surface_requires_reconfiguration: false,
        })
    }

    /// The window itself, for the controls the application drives it with.
    pub(super) fn handle(&self) -> Arc<winit::window::Window> {
        self.window.clone()
    }

    /// The winit id this window's events arrive under.
    pub(super) fn id(&self) -> winit::window::WindowId {
        self.window.id()
    }

    /// The window's size in logical pixels — the space a scene is laid out in.
    pub(super) fn logical_size(&self) -> Vector2F {
        self.surface_size / self.scale_factor()
    }

    /// Physical pixels per logical pixel.
    pub(super) fn scale_factor(&self) -> f32 {
        self.window.scale_factor() as f32
    }

    /// Whether the desktop is set to light or to dark, as it stands.
    ///
    /// A desktop that will not answer is taken as dark: winit reports `None`
    /// for a platform with no such setting and for one it cannot read, and a
    /// terminal has always been dark.
    pub(super) fn system_theme(&self) -> SystemTheme {
        self.window.theme().map_or(SystemTheme::Dark, system_theme)
    }

    /// Puts the rectangle an input method draws its candidate list beside.
    ///
    /// The coordinates are logical, like everything else that crosses the
    /// window seam, and winit takes logical ones here — so nothing is scaled.
    pub(super) fn set_ime_area(&self, origin: Vector2F, size: Vector2F) {
        self.window.set_ime_cursor_area(
            winit::dpi::LogicalPosition::new(origin.x() as f64, origin.y() as f64),
            winit::dpi::LogicalSize::new(size.x() as f64, size.y() as f64),
        );
    }

    /// Whether a frame is already built and waiting to be drawn.
    pub(super) fn has_scene(&self) -> bool {
        self.scene.is_some()
    }

    /// Rebuilds the frame from the application's current state and draws it.
    pub(super) fn request_redraw(&mut self) {
        self.scene = None;
        self.window.request_redraw();
    }

    /// Records that the window changed size, so the next redraw reconfigures.
    ///
    /// Resize events arrive far faster than frames, so the reconfigure is
    /// deferred: this costs at most one `surface.configure` per frame instead
    /// of one per event.
    pub(super) fn mark_resized(&mut self) {
        self.surface_requires_reconfiguration = true;
        self.request_redraw();
    }

    /// Reconfigures the swapchain if the window has changed size.
    ///
    /// Runs *before* the scene is built, so the frame is laid out at the size
    /// it is about to be rendered at.
    pub(super) fn update_size_if_needed(&mut self) -> Result<(), Error> {
        let window_size = physical_size(&self.window);
        let Some(rendering) = self.rendering.as_ref() else {
            return Ok(());
        };

        if self.surface_requires_reconfiguration || self.surface_size != window_size {
            rendering.resources.update_size(window_size)?;
            self.surface_size = window_size;
            self.surface_requires_reconfiguration = false;
        }

        Ok(())
    }

    /// Draws a frame, using `new_scene` if none was already stored.
    pub(super) fn render(
        &mut self,
        new_scene: Option<Rc<Scene>>,
        font_db: &dyn FontDb,
    ) -> Result<(), Error> {
        if self.scene.is_none() {
            self.scene = new_scene;
        }

        let Some(scene) = self.scene.clone() else {
            log::warn!("a redraw was requested but no scene was available to draw");
            return Ok(());
        };

        let Some(rendering) = self.rendering.as_mut() else {
            // Between `drop_renderer` and `recreate_renderer`, or after
            // recreation failed. The next frame will try again.
            return Ok(());
        };

        let window = &self.window;
        rendering
            .renderer
            .render(scene.as_ref(), &rendering.resources, font_db, || {
                window.pre_present_notify()
            })
    }

    /// Throws away every wgpu object this window owns.
    ///
    /// The instance goes too: some NVIDIA drivers deadlock when resources are
    /// rebuilt on the instance that lost them, so recovery starts from a
    /// genuinely new one.
    pub(super) fn drop_renderer(&mut self, display: Box<dyn WgpuHasDisplayHandle>) {
        self.rendering = None;
        reset_wgpu_instance(display);
    }

    /// Builds a fresh device, swapchain and renderer against the same window.
    ///
    /// Failure is logged rather than propagated: the window stays open with
    /// nothing to draw with, and the next redraw tries again. A GPU that has
    /// just been reset often refuses one device and hands over the next.
    pub(super) fn recreate_renderer(&mut self) {
        let resources = match WindowResources::new(self.window.clone(), self.surface_size)
            .context("failed to recreate the window's GPU resources")
        {
            Ok(resources) => resources,
            Err(error) => {
                log::error!("{error:#}");
                return;
            }
        };

        let renderer = Renderer::new(&resources.resources, resources.format());
        self.rendering = Some(RenderingResources {
            resources,
            renderer,
        });
        self.window.request_redraw();
    }
}

fn physical_size(window: &winit::window::Window) -> Vector2F {
    let size = window.inner_size();
    vec2f(size.width as f32, size.height as f32)
}
