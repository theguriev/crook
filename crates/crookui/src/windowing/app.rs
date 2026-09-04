//! The event loop: one window, demand-driven, and the bridge back to it.
//!
//! Nothing here polls. `ControlFlow::Wait` means the process sleeps until the
//! OS has something to say, and a frame happens only in response to
//! `RedrawRequested`, which only a `request_redraw` produces. The consequence
//! worth knowing is that an application that changes state without asking for a
//! redraw shows stale pixels and reports no error.
//!
//! The other half of the module is how work gets *back* to this thread. A
//! background task cannot touch an entity, so its result comes home as a
//! [`CrookEvent`] on winit's proxy — which is also what the
//! [`Foreground`](crookui_core::executor::Foreground) executor schedules onto.
//! There is no second scheduler: the event loop is the main-thread executor.
//!
//! Ported from Warp's `crates/warpui/src/windowing/winit/{app,event_loop}.rs`
//! (MIT), reduced from thirty `CustomEvent` variants to three and written
//! against `ApplicationHandler` rather than the deprecated closure API.

use std::fmt;
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crookui_core::event::Event;
use crookui_core::executor::{Foreground, Runnable};
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform::FontDb;
use crookui_core::scene::Scene;
use parking_lot::Mutex;
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};

use crate::rendering::init_wgpu_instance;

use super::event::InputState;
use super::window::Window;

/// How a window is opened.
pub struct WindowOptions {
    /// The title the window manager shows.
    pub title: String,
    /// The initial inner size, in logical pixels.
    pub size: Vector2F,
    /// The smallest inner size the user may drag it to, in logical pixels.
    pub min_size: Vector2F,
    /// Whether the window manager draws a title bar and frame.
    pub decorations: bool,
    /// Whether the window may be see-through where the scene is.
    pub transparent: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Crook".to_owned(),
            size: vec2f(1024., 640.),
            min_size: vec2f(480., 192.),
            decorations: true,
            transparent: true,
        }
    }
}

/// What the window asks of the application above it.
///
/// This is the whole seam between the platform layer and the application:
/// three methods, no winit types, no wgpu types. A headless test double
/// implements it in a dozen lines.
pub trait WindowDelegate: 'static {
    /// Lays out and paints the frame to draw.
    ///
    /// `size` is the window's inner size in logical pixels and `scale_factor`
    /// is its physical-to-logical ratio; the returned [`Scene`] must carry that
    /// same scale factor, because the renderer multiplies by it exactly once.
    fn build_scene(&mut self, size: Vector2F, scale_factor: f32) -> Rc<Scene>;

    /// Handles one input event.
    ///
    /// Return `true` only when something actually changed and the frame needs
    /// rebuilding. Returning `true` unconditionally turns a demand-driven loop
    /// into a busy one.
    fn handle_event(&mut self, event: Event) -> bool;

    /// Runs after each frame reaches the screen.
    fn frame_drawn(&mut self);
}

/// A `Send + Sync` handle for reaching the main thread from anywhere.
///
/// Cloning is cheap. Every method is a message on winit's proxy, so all of them
/// are safe to call from a background thread — and all of them silently do
/// nothing once the event loop has exited, which is the right behaviour for a
/// task that outlives the window it was going to update.
#[derive(Clone)]
pub struct Proxy(Arc<Mutex<EventLoopProxy<CrookEvent>>>);

impl Proxy {
    /// Asks the window to rebuild its scene and draw it.
    pub fn request_redraw(&self) {
        self.send(CrookEvent::RedrawRequested);
    }

    /// Asks the application to quit.
    pub fn exit(&self) {
        self.send(CrookEvent::Exit);
    }

    /// Says where the text being composed is, so the platform can put an input
    /// method's candidate list beside it rather than in a corner.
    ///
    /// In logical pixels, like everything else that crosses this seam. Sending
    /// the same rectangle twice is harmless and does nothing; a caller that
    /// sends one per frame is only paying for a message.
    pub fn set_ime_area(&self, origin: Vector2F, size: Vector2F) {
        self.send(CrookEvent::SetImeArea { origin, size });
    }

    /// An executor whose tasks run on the main thread, through this proxy.
    pub fn foreground(&self) -> Rc<Foreground> {
        let proxy = self.clone();
        Rc::new(Foreground::new(move |runnable| {
            // `Runnable` is `Send`, but the future underneath it is not: these
            // come from `spawn_local`. Wrapping it means that if the event loop
            // exits with tasks still queued, winit drops the envelope and the
            // future leaks rather than being dropped on the wrong thread.
            proxy.send(CrookEvent::RunTask(ManuallyDrop::new(runnable)));
        }))
    }

    fn send(&self, event: CrookEvent) {
        let _ = self.0.lock().send_event(event);
    }
}

impl fmt::Debug for Proxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Proxy")
    }
}

/// Everything the event loop hands the application before it starts running.
pub struct Platform {
    /// The main-thread executor. Tasks spawned on it may touch entities.
    pub foreground: Rc<Foreground>,
    /// A handle for waking the main thread from a background task.
    pub proxy: Proxy,
}

/// Everything that reaches the main thread from somewhere else.
///
/// Three variants rather than Warp's thirty, because Crook has one window and
/// no menu bar, no global hotkeys and no notifications. Adding a fourth is how
/// any future off-thread capability should arrive.
enum CrookEvent {
    /// Poll a foreground task.
    RunTask(ManuallyDrop<Runnable>),
    /// Drop the cached scene and draw a fresh one.
    RedrawRequested,
    /// Move the rectangle an input method puts its candidate list beside.
    SetImeArea {
        /// The top-left corner, in logical pixels.
        origin: Vector2F,
        /// How big the composed text's box is, in logical pixels.
        size: Vector2F,
    },
    /// Leave the event loop.
    Exit,
}

/// Opens a window and runs until it closes.
///
/// The order matters. The event loop exists first, because wgpu's instance has
/// to be built from its display handle before any surface is created; the proxy
/// exists next, because the application needs an executor before it can build
/// anything; and only then is `build_delegate` called.
pub fn run(
    options: WindowOptions,
    font_db: Box<dyn FontDb>,
    build_delegate: impl FnOnce(&Platform) -> Box<dyn WindowDelegate>,
) -> Result<()> {
    let event_loop = EventLoop::<CrookEvent>::with_user_event()
        .build()
        .context("failed to create the event loop")?;

    // Sleep between events rather than spinning: a terminal is idle almost all
    // of the time, and a frame nobody asked for is a frame nobody sees.
    event_loop.set_control_flow(ControlFlow::Wait);

    // wgpu rejects a surface whose display handle differs from its instance's,
    // so the instance is built here, before the first window can exist.
    init_wgpu_instance(Box::new(event_loop.owned_display_handle()));

    let proxy = Proxy(Arc::new(Mutex::new(event_loop.create_proxy())));
    let platform = Platform {
        foreground: proxy.foreground(),
        proxy: proxy.clone(),
    };
    let delegate = build_delegate(&platform);

    let mut app = App {
        options,
        font_db,
        delegate,
        window: None,
        input: InputState::default(),
        replay_requested_redraw: false,
        frame_retry: None,
    };

    event_loop
        .run_app(&mut app)
        .context("the event loop exited with an error")
}

struct App {
    options: WindowOptions,
    font_db: Box<dyn FontDb>,
    delegate: Box<dyn WindowDelegate>,
    window: Option<Window>,
    input: InputState,

    /// Whether the redraw now pending is the one the hover replay itself asked
    /// for. Without it, a delegate that repaints in response to the replay
    /// would turn the replay in [`App::redraw`] into a spin; with it, every
    /// relayout gets exactly one replay and the replay's own frame gets none.
    replay_requested_redraw: bool,

    /// How long to wait before trying the frame that just failed again, if one
    /// did. `None` once a frame reaches the screen.
    frame_retry: Option<Duration>,
}

/// How long to wait after a frame failed before trying again, and the longest
/// that wait is allowed to grow to.
///
/// A failure is usually momentary — a window that is occluded, a swapchain
/// texture that timed out — and with `ControlFlow::Wait` nothing else will ask
/// for another frame, so the retry has to come from here. Backing off means a
/// window left minimised for an hour costs one wakeup a second rather than
/// thousands.
const FIRST_FRAME_RETRY: Duration = Duration::from_millis(50);
const LONGEST_FRAME_RETRY: Duration = Duration::from_secs(1);

impl ApplicationHandler<CrookEvent> for App {
    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        // The only timer this application sets is the one a failed frame asks
        // for, so reaching it means it is time to try that frame again.
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && self.frame_retry.is_some()
            && let Some(window) = self.window.as_mut()
        {
            window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(match self.frame_retry {
            Some(delay) => ControlFlow::WaitUntil(Instant::now() + delay),
            None => ControlFlow::Wait,
        });
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Android and iOS destroy the render surface on suspend, so winit only
        // promises a usable one from here. On desktop this fires once, right
        // after startup.
        if self.window.is_some() {
            return;
        }

        match Window::new(event_loop, &self.options) {
            Ok(window) => self.window = Some(window),
            Err(error) => {
                log::error!("could not open the window: {error:#}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: CrookEvent) {
        match event {
            CrookEvent::RunTask(runnable) => {
                // Back on the thread that spawned it, which is the only thread
                // where unwrapping the envelope and polling the future is sound.
                ManuallyDrop::into_inner(runnable).run();
            }
            CrookEvent::RedrawRequested => {
                if let Some(window) = self.window.as_mut() {
                    window.request_redraw();
                }
            }
            CrookEvent::SetImeArea { origin, size } => {
                if let Some(window) = self.window.as_mut() {
                    window.set_ime_area(origin, size);
                }
            }
            CrookEvent::Exit => event_loop.exit(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().map(Window::id) != Some(window_id) {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }

            // Reconfiguring the swapchain here would cost one `configure` per
            // resize event; flagging it costs one per frame instead. A scale
            // change is a resize too, because the swapchain is sized in
            // physical pixels even when the logical size did not move.
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.with_window(Window::mark_resized);
                return;
            }

            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }

            _ => {}
        }

        let scale_factor = self.with_window(|window| window.scale_factor());
        let Some(event) = self.input.convert(&event, scale_factor) else {
            return;
        };

        if self.delegate.handle_event(event) {
            self.with_window(Window::request_redraw);
        }
    }
}

impl App {
    /// Runs `action` against the window, which the caller has already
    /// established exists.
    fn with_window<T>(&mut self, action: impl FnOnce(&mut Window) -> T) -> T {
        action(
            self.window
                .as_mut()
                .expect("the window was matched by id a moment ago"),
        )
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let App {
            window,
            delegate,
            font_db,
            input,
            replay_requested_redraw,
            frame_retry,
            ..
        } = self;

        // Taken before anything can set it again: this frame is the replay's
        // own only if the *previous* frame's replay asked for it.
        let after_replay = std::mem::take(replay_requested_redraw);
        let Some(window) = window.as_mut() else {
            return;
        };

        let mut rebuilt = false;
        let result = (|| {
            // Reconfigure before building, so the frame is laid out at the size
            // it is about to be drawn at rather than the size of the last one.
            window.update_size_if_needed()?;

            let new_scene = if window.has_scene() {
                None
            } else {
                rebuilt = true;
                Some(delegate.build_scene(window.logical_size(), window.scale_factor()))
            };

            window.render(new_scene, font_db.as_ref())
        })();

        let Err(error) = result else {
            *frame_retry = None;
            delegate.frame_drawn();

            // A frame that changed layout moved the world out from under a
            // pointer that never moved, so hover state is now answering the
            // wrong question — close a tab and its neighbour stays highlighted,
            // and a chip that grew when a reading landed slides the tab under
            // the cursor out from under it. Replaying the last position re-asks
            // it against the new geometry. Every rebuild gets one, whatever
            // caused it; only the replay's own frame is skipped, which is what
            // keeps this from looping.
            if rebuilt && !after_replay && delegate.handle_event(input.synthetic_mouse_move()) {
                *replay_requested_redraw = true;
                window.request_redraw();
            }
            return;
        };

        // Nothing else is going to ask for this frame again: the delegate was
        // never told it happened, and a window that is idle by design produces
        // no events of its own. Without the retry the last good frame stays on
        // screen forever and a run bounded by a frame budget never reaches it.
        let delay = next_frame_retry(*frame_retry);
        *frame_retry = Some(delay);

        log::warn!("failed to draw a frame: {error}; retrying in {delay:?}");
        if error.requires_renderer_recreation() {
            log::warn!("rebuilding the renderer to recover");
            window.drop_renderer(Box::new(event_loop.owned_display_handle()));
            window.recreate_renderer();
        }
    }
}

/// How long to wait before trying a failed frame again, given how long the
/// wait before it was.
fn next_frame_retry(previous: Option<Duration>) -> Duration {
    match previous {
        Some(previous) => previous.saturating_mul(2).min(LONGEST_FRAME_RETRY),
        None => FIRST_FRAME_RETRY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_frame_is_retried_sooner_at_first_and_then_less_and_less_often() {
        let first = next_frame_retry(None);
        assert_eq!(first, FIRST_FRAME_RETRY);
        assert_eq!(next_frame_retry(Some(first)), first * 2);

        // A window left minimised for an hour must cost one wakeup a second,
        // not twenty.
        let mut delay = first;
        for _ in 0..64 {
            delay = next_frame_retry(Some(delay));
        }
        assert_eq!(delay, LONGEST_FRAME_RETRY);
    }
}
