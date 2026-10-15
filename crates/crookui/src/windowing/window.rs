//! One window: its winit handle, its GPU resources, the frame it draws, and
//! where it is on the desktop.
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
use std::time::Instant;

use anyhow::{Context, Result};
use crookui_core::event::SystemTheme;
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform::FontDb;
use crookui_core::scene::{PointerShape, Scene};
use wgpu::wgt::WgpuHasDisplayHandle;
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

use crate::rendering::{Error, Renderer, WindowResources, reset_wgpu_instance};

use super::event::system_theme;

use super::WindowOptions;
use super::chrome::with_chrome;
use super::fullscreen_entered::FullscreenEntered;
use super::placement::{
    Following, FullscreenAsked, FullscreenStep, Monitor, Sighting, WindowPlacement,
    fills_a_monitor, onto_a_monitor,
};

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

    /// Where the window is on the desktop and what it is doing, as of the
    /// last time it was looked at — see [`Self::settle_placement`].
    following: Following,

    /// Whether the desktop is X11, where winit cannot see a fullscreen it did
    /// not ask for — see [`placement`](super::placement).
    on_x11: bool,

    /// The fullscreen asked for as the window opened, while macOS may still
    /// refuse it — see [`FullscreenAsked`] — with AppKit's word on whether
    /// the window has entered it, where its `NSWindow` could be reached.
    fullscreen_asked: Option<(FullscreenAsked, Option<FullscreenEntered>)>,
}

impl Window {
    /// Creates the winit window and everything needed to draw into it.
    pub(super) fn new(event_loop: &ActiveEventLoop, options: &WindowOptions) -> Result<Self> {
        // Checked against the monitors there are now rather than trusted: the
        // one the window was on may have been unplugged since, and a window
        // opened where nothing is shown is one nobody can reach to move.
        let monitors: Vec<_> = event_loop.available_monitors().collect();
        let landing = options.position.and_then(|position| {
            let desktop: Vec<Monitor> = monitors.iter().map(desktop_monitor).collect();
            onto_a_monitor(position, options.size, &desktop)
        });

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
            .with_maximized(options.maximized)
            .with_transparent(options.transparent);
        let attributes = match landing {
            Some((position, _)) => attributes.with_position(winit_position(position)),
            None => attributes,
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let attributes = super::chrome::with_app_id(attributes, &options.app_id);

        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("failed to create the window")?,
        );

        let on_x11 = matches!(
            HasDisplayHandle::display_handle(event_loop).map(|display| display.as_raw()),
            Ok(RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_))
        );

        // Once the window exists rather than as one of its attributes, which
        // is what makes leaving fullscreen go back to the size and the place
        // asked for above: macOS opens a window asked to be fullscreen at the
        // size of the screen and restores it to that, and Windows takes the
        // keyboard from whatever has it. On the monitor the window was put on,
        // or the one the desktop put it on. Never on X11, where winit goes on
        // taking a window it made fullscreen as fullscreen after the window
        // manager has ended it, and where no window is kept as fullscreen —
        // see `placement` — so a fullscreen read there was written elsewhere.
        let fullscreen = options.fullscreen && !on_x11;
        // Only macOS refuses one this way; Windows enters it before
        // `set_fullscreen` returns, and a Wayland compositor that refuses
        // has said what it will do. Listened for before it is asked, so that
        // AppKit's answer cannot come first.
        let fullscreen_asked = (fullscreen && cfg!(target_os = "macos")).then(|| {
            (
                FullscreenAsked::new(Instant::now()),
                FullscreenEntered::watch(&window),
            )
        });
        if fullscreen {
            let monitor = landing.and_then(|(_, index)| monitors.get(index).cloned());
            window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(monitor)));
        }

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
            // What the window was asked to be, which is what it goes back to
            // if the first look finds it maximised already.
            following: Following::new(WindowPlacement {
                position: landing.map(|(position, _)| position),
                size: options.size,
                maximized: options.maximized,
                fullscreen,
            }),
            on_x11,
            fullscreen_asked,
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

    /// Whether the window has the desktop's keyboard focus, as the platform
    /// knows it now.
    pub(super) fn has_focus(&self) -> bool {
        self.window.has_focus()
    }

    /// Where the window is on the desktop and what it is doing, as of the
    /// last look.
    pub(super) fn placement(&self) -> WindowPlacement {
        self.following.placement()
    }

    /// Notes that the window said it moved, was resized or changed scale, for
    /// [`Self::settle_placement`] to look at once the batch of events is over.
    pub(super) fn placement_stirred(&mut self) {
        self.following.stirred();
    }

    /// Ends a batch of events: looks at where the window is if anything in it
    /// said it moved, and returns the placement to keep if that changed.
    ///
    /// Once a batch rather than on each event, both because an event can come
    /// before the state that explains it — see [`placement`](super::placement)
    /// — and because on X11 whether the window is maximised is a question to
    /// the server.
    pub(super) fn settle_placement(&mut self) -> Option<WindowPlacement> {
        let Self {
            window,
            following,
            on_x11,
            fullscreen_asked,
            ..
        } = self;
        following.settle(|| sighting(window, *on_x11, fullscreen_asked.is_some()))
    }

    /// Looks at where the window is whatever was said, and returns the
    /// placement to keep if it changed since the last look: as the window
    /// opens and as the event loop stops, with no batch to wait for.
    pub(super) fn follow_placement(&mut self) -> Option<WindowPlacement> {
        let seen = sighting(&self.window, self.on_x11, self.fullscreen_asked.is_some());
        self.following.see(seen)
    }

    /// Asks again for the fullscreen asked for as the window opened, if macOS
    /// refused it, and returns when to come back to it — `None` once there is
    /// nothing more to do.
    ///
    /// For a few seconds after the window opens and only then, and only on
    /// macOS: see [`FullscreenAsked`].
    pub(super) fn keep_asking_for_fullscreen(&mut self, now: Instant) -> Option<Instant> {
        let (asked, entered) = self.fullscreen_asked.as_mut()?;
        let fullscreen = self.window.fullscreen().is_some();
        let entered = entered.as_ref().is_some_and(FullscreenEntered::entered);
        match asked.step(now, fullscreen, entered) {
            FullscreenStep::Over => {
                // Looked at again: it was taken as fullscreen for being asked
                // to be, and from now on it is taken as it is.
                self.fullscreen_asked = None;
                self.following.stirred();
                None
            }
            FullscreenStep::AskAgain(then) => {
                self.window
                    .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                Some(then)
            }
            FullscreenStep::LookAt(then) => Some(then),
        }
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

    /// Names the window for the window manager.
    pub(super) fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }

    /// Asks the desktop to point at the window, the way it does for any
    /// application that wants a look: once, and without taking the keyboard.
    /// Returns whether it asked.
    ///
    /// Winit's informational kind, whose spelling is the platform's — the
    /// dock icon bounces once on macOS, the window manager's urgency hint is
    /// set on X11, an activation request goes to a Wayland compositor that
    /// takes them, and the taskbar button flashes on Windows. A window that
    /// already has the focus is not asked about at all, and that is checked
    /// here rather than trusted to the platform: X11 sets its hint on a
    /// focused window as readily as on any other, and the request can reach
    /// this after the focus did, since winit's X11 loop reads the server's
    /// events before the ones the application sent.
    pub(super) fn request_attention(&self) -> bool {
        if self.window.has_focus() {
            return false;
        }
        self.window
            .request_user_attention(Some(winit::window::UserAttentionType::Informational));
        true
    }

    /// Takes a request for attention back.
    ///
    /// For X11, whose urgency hint stays until somebody takes it down. The
    /// rest need nothing and winit makes this nothing there: Windows stops
    /// the flash itself when the window comes to the front, and ignores this
    /// for the active window anyway; macOS bounces once; and a Wayland
    /// compositor clears its own.
    pub(super) fn withdraw_attention_request(&self) {
        self.window.request_user_attention(None);
    }

    /// Whether the desktop says the window is minimised, or `None` where it
    /// cannot say, which on Wayland is always, or is not asked, which on
    /// macOS and Windows is always.
    ///
    /// Those two say it by themselves — macOS as an `Occluded`, Windows as a
    /// size with no room in it — so their answer adds nothing but the chance
    /// of being late. On macOS it can be: winit hands on AppKit's word that
    /// a window it restores is in view from inside the restore, before
    /// `isVisible` says yes, and `isMiniaturized` asked then, or at a focus
    /// that arrives before the restore is done, may still say yes and hold a
    /// window in plain sight out of it.
    ///
    /// On X11 this is a round trip to the server, for the window manager's
    /// `_NET_WM_STATE_HIDDEN`; see `InputState::may_be_minimised` for when
    /// it is asked.
    pub(super) fn is_minimised(&self) -> Option<bool> {
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            return None;
        }
        self.window.is_minimized()
    }

    /// Whether a frame is already built and waiting to be drawn.
    pub(super) fn has_scene(&self) -> bool {
        self.scene.is_some()
    }

    /// The pointer the frame on screen asks for at `position`, or `None`
    /// between a change and the frame that draws it.
    ///
    /// `None` rather than an answer from the frame that is going: the next
    /// one is about to be built, and the pointer is asked again then.
    pub(super) fn pointer_at(&self, position: Vector2F) -> Option<PointerShape> {
        self.scene.as_ref().map(|scene| scene.pointer_at(position))
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

/// `window` as the platform reports it right now, on X11 if `on_x11`, and
/// with a fullscreen that is still being asked for if `asking_for_fullscreen`.
fn sighting(window: &winit::window::Window, on_x11: bool, asking_for_fullscreen: bool) -> Sighting {
    let scale_factor = window.scale_factor();
    Sighting {
        position: window
            .outer_position()
            .ok()
            .map(|position| desktop_position(position, scale_factor)),
        size: physical_size(window) / scale_factor as f32,
        maximized: window.is_maximized(),
        // Not winit's word on X11, where it knows only the fullscreen it was
        // asked for, and it never is there; covering a monitor is the guess
        // that stands in for it. A refusal on its way to being asked again is
        // not the window leaving fullscreen, and must not cost the session
        // its fullscreen.
        fullscreen: (!on_x11 && window.fullscreen().is_some()) || asking_for_fullscreen,
        covers_a_monitor: on_x11 && fills_a_monitor_now(window),
        minimized: window.is_minimized() == Some(true),
    }
}

/// Whether `window`'s inside covers the whole of one monitor, which on X11 is
/// the only sign that it may be fullscreen: see [`placement`](super::placement).
fn fills_a_monitor_now(window: &winit::window::Window) -> bool {
    let Ok(position) = window.inner_position() else {
        return false;
    };
    let size = window.inner_size();
    let monitors: Vec<Monitor> = window
        .available_monitors()
        .map(|monitor| desktop_monitor(&monitor))
        .collect();
    fills_a_monitor(
        [position.x, position.y],
        [
            i32::try_from(size.width).unwrap_or(i32::MAX),
            i32::try_from(size.height).unwrap_or(i32::MAX),
        ],
        &monitors,
    )
}

/// A position winit reports, in the desktop's own coordinates: see
/// [`placement`](super::placement) for why macOS's are points.
fn desktop_position(position: winit::dpi::PhysicalPosition<i32>, scale_factor: f64) -> [i32; 2] {
    if cfg!(target_os = "macos") {
        let points = position.to_logical::<f64>(scale_factor);
        [points.x.round() as i32, points.y.round() as i32]
    } else {
        [position.x, position.y]
    }
}

/// A position in the desktop's own coordinates, as winit takes one.
fn winit_position([x, y]: [i32; 2]) -> winit::dpi::Position {
    if cfg!(target_os = "macos") {
        winit::dpi::LogicalPosition::new(f64::from(x), f64::from(y)).into()
    } else {
        winit::dpi::PhysicalPosition::new(x, y).into()
    }
}

/// A monitor, in the desktop's own coordinates.
fn desktop_monitor(monitor: &winit::monitor::MonitorHandle) -> Monitor {
    let scale = monitor.scale_factor();
    let (origin, size) = (monitor.position(), monitor.size());
    if cfg!(target_os = "macos") {
        let origin = origin.to_logical::<f64>(scale);
        let size = size.to_logical::<f64>(scale);
        Monitor {
            origin: [origin.x.round() as i32, origin.y.round() as i32],
            size: [size.width.round() as i32, size.height.round() as i32],
            scale: 1.,
        }
    } else {
        Monitor {
            origin: [origin.x, origin.y],
            size: [
                i32::try_from(size.width).unwrap_or(i32::MAX),
                i32::try_from(size.height).unwrap_or(i32::MAX),
            ],
            scale,
        }
    }
}
