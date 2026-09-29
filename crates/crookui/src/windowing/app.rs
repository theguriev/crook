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
//! [`Foreground`] executor schedules onto.
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
use crookui_core::event::{Event, MouseButton};
use crookui_core::executor::{Foreground, Runnable};
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform::FontDb;
use crookui_core::scene::Scene;
use parking_lot::Mutex;
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};

use crate::rendering::init_wgpu_instance;

use super::chrome::{RESIZE_GRAB, WindowChrome, WindowControls, edge_at};
use super::dock;
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
    /// Who draws the title bar the window's controls sit in.
    pub chrome: WindowChrome,
    /// Whether the window may be see-through where the scene is.
    pub transparent: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Crook".to_owned(),
            size: vec2f(1024., 640.),
            min_size: vec2f(480., 192.),
            chrome: WindowChrome::default(),
            transparent: true,
        }
    }
}

/// What the window asks of the application above it.
///
/// This is the whole seam between the platform layer and the application:
/// six methods, three of them with a default, no winit types, no wgpu types. A
/// headless test double implements it in a dozen lines.
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

    /// The window's own close was asked for: its close button, the desktop's
    /// shortcut for closing a window, a window manager closing it.
    ///
    /// Return `true` to leave the event loop now. An application that has
    /// something to ask first returns `false`, puts its question on screen —
    /// which asks for its own frame, as any change does — and leaves later
    /// through [`Proxy::exit`], or not at all if the answer was no.
    ///
    /// The default leaves at once, which is right for a window with nothing
    /// in it to lose. It is not asked when the operating system ends the
    /// process itself: that does not come through the window.
    fn close_requested(&mut self) -> bool {
        true
    }

    /// The window has just taken the keyboard: a click on it, the desktop's
    /// switcher, or the desktop answering a request for attention by focusing
    /// it.
    ///
    /// Not an [`Event`], because nothing in the element tree is under it and
    /// nothing is drawn differently for it. What it is for is knowing that the
    /// next few keys may have been typed at whatever had the keyboard before.
    /// The default does nothing with that.
    fn focused(&mut self) {}

    /// Runs once, as the event loop stops, whatever stopped it.
    ///
    /// The one place an application hears that it is ending. Only some of
    /// the ways a window closes pass through the application first — the
    /// window manager's close and macOS's Quit go straight to the platform,
    /// and Quit ends the process without [`run`] ever returning — and every
    /// one of them passes through here.
    ///
    /// Nothing is drawn after it, so work done here is work the person
    /// waits for with the window up and unanswering: bound it.
    fn exiting(&mut self) {}
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

    /// Names the window, the way the window manager, the taskbar and the
    /// switcher show it.
    ///
    /// Sending the same name twice is harmless; a caller that sends one per
    /// frame is only paying for a message, as with the input method's area.
    pub fn set_title(&self, title: String) {
        self.send(CrookEvent::SetTitle(title));
    }

    /// Asks the desktop to point at the window: a bounce of the dock icon, an
    /// urgency hint, a flash of the taskbar button — whichever the platform
    /// has.
    ///
    /// For something in the window that wants a person who is somewhere
    /// else, so it does nothing while the window has the focus. It is not a
    /// notification: it says nothing but "this window", and the platform
    /// decides how. The request is over when the window next gains the focus,
    /// which is the look it asked for — taken back then on the desktop that
    /// needs it, X11 — so the caller has nothing to undo.
    pub fn request_attention(&self) {
        self.send(CrookEvent::RequestAttention);
    }

    /// Puts `waiting` on the application's dock icon as a badge, or takes
    /// the badge off at zero.
    ///
    /// The count of panes waiting for a person, the number the window's
    /// title starts with, somewhere it is seen with the window out of sight.
    /// It is set here and drawn by the dock, which for an application with a
    /// bundle identifier — Crook.app — draws it only after the application
    /// has asked Notification Center for leave to badge, and only while the
    /// person's Badges switch allows it. Asking is the caller's, with
    /// [`Self::show_badge_again`] once the answer is yes. For a binary
    /// started from a shell, which has no identifier to ask as, it is set all
    /// the same, and whether the dock draws it is not established.
    ///
    /// macOS only, and nothing is sent anywhere else: a Linux desktop has no
    /// badge its docks agree on, and a Windows taskbar's overlay icon is a
    /// picture rather than a number. Sending the same count twice is
    /// harmless; a caller that sends one only when it changes saves the
    /// dock a redraw.
    pub fn set_badge(&self, waiting: usize) {
        if cfg!(target_os = "macos") {
            self.send(CrookEvent::SetBadge(waiting));
        }
    }

    /// Sets the dock icon's badge again as it stands, for a dock that may
    /// have dropped it while the application had no leave to badge.
    ///
    /// For the moment that leave arrives, which is on a queue of the
    /// system's: whatever count [`Self::set_badge`] last sent is the one
    /// shown, since both go through the event loop in the order they were
    /// sent. Nothing when there is no badge, and nothing is sent off macOS.
    pub fn show_badge_again(&self) {
        if cfg!(target_os = "macos") {
            self.send(CrookEvent::ShowBadgeAgain);
        }
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
    /// The window, for an application that draws its own title bar and so has
    /// to move, maximise and minimise it itself.
    ///
    /// Handed over before the window exists, because the application is built
    /// before it: every method does nothing until [`run`] has opened one.
    pub window: WindowControls,
}

/// Everything that reaches the main thread from somewhere else.
///
/// A handful of variants rather than Warp's thirty, because Crook has one
/// window and no menu bar, no global hotkeys and no notifications — asking
/// for attention is not one: it names only the window, and the desktop says
/// it however it says it; nor is the dock's badge, a number on the icon. The
/// application posts its notifications itself. Adding one is how any future
/// off-thread capability should arrive.
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
    /// Name the window.
    SetTitle(String),
    /// Ask the desktop to point at the window.
    RequestAttention,
    /// Put this many waiting panes on the dock icon's badge.
    SetBadge(usize),
    /// Set the dock icon's badge again, as it stands.
    ShowBadgeAgain,
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
    let controls = WindowControls::default();
    let platform = Platform {
        foreground: proxy.foreground(),
        proxy: proxy.clone(),
        window: controls.clone(),
    };
    let delegate = build_delegate(&platform);

    let mut app = App {
        options,
        font_db,
        delegate,
        window: None,
        controls,
        input: InputState::default(),
        attention_requested: false,
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
    controls: WindowControls,
    input: InputState,

    /// Whether the desktop has been asked to point at the window since it
    /// last had the focus, so that gaining it can take the request back.
    attention_requested: bool,

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

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.delegate.exiting();
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
            Ok(window) => {
                // The desktop's setting as it stands, before any frame is
                // built. Winit only *reports* a change, so an application that
                // waited for `ThemeChanged` would open in the wrong one and
                // stay there until somebody toggled the system setting. A
                // desktop that will not say is taken as dark, which is what a
                // terminal has always been.
                let theme = window.system_theme();

                // Before the delegate hears anything: the controls handle is
                // what a header's drag, zoom or close reaches the real window
                // through, and the first event can arrive as soon as the
                // delegate is called below.
                self.controls.attach(window.handle());
                self.window = Some(window);
                if self.delegate.handle_event(Event::SystemTheme(theme)) {
                    self.with_window(Window::request_redraw);
                }
            }
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
            CrookEvent::SetTitle(title) => {
                if let Some(window) = self.window.as_mut() {
                    window.set_title(&title);
                }
            }
            CrookEvent::RequestAttention => {
                if let Some(window) = self.window.as_mut()
                    && window.request_attention()
                {
                    self.attention_requested = true;
                }
            }
            CrookEvent::SetBadge(waiting) => dock::set_badge(dock::label(waiting).as_deref()),
            CrookEvent::ShowBadgeAgain => dock::show_again(),
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
            // The application's to answer rather than this loop's: a window
            // with work in it asks before that work is ended, and the answer
            // comes back as an `Exit` on the proxy — or never, if it was no.
            WindowEvent::CloseRequested => {
                if self.delegate.close_requested() {
                    event_loop.exit();
                }
                return;
            }

            // Losing the keyboard is nothing to anyone here; taking it is the
            // end of any attention the window asked for, and the moment the
            // application's next keys stop being certainly its own.
            WindowEvent::Focused(focused) => {
                if focused {
                    self.controls.focused();
                    self.delegate.focused();
                }
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

            // The look a request for attention asked for, so the request is
            // over. Taken back here rather than left to the application,
            // because only X11 needs taking back — it keeps its urgency hint
            // until somebody removes it — and nothing above this line should
            // have to know which desktop it is on. Then on to the delegate
            // like any other focus change.
            WindowEvent::Focused(true) if self.attention_requested => {
                self.attention_requested = false;
                self.with_window(|window| window.withdraw_attention_request());
            }

            _ => {}
        }

        let scale_factor = self.with_window(|window| window.scale_factor());
        let Some(event) = self.input.convert(&event, scale_factor) else {
            return;
        };

        // Before the delegate, because the resize border is *outside* the
        // application: a press five pixels into a frameless window belongs to
        // the window manager however interesting the element under it is —
        // except in the corner the application draws the window's own controls
        // in, which `handle_resize_border` leaves alone.
        let grabbed = self.handle_resize_border(&event);
        let redraw = !grabbed && self.delegate.handle_event(event);

        // After both of them, which is the whole point. The border starts a
        // gesture above, but a window *move* is started by the header, inside
        // the delegate's own dispatch: asking before that ran read the flag one
        // event late, and one event is long enough for the first pointer move
        // after the drag to be converted as a drag with no button behind it.
        if self.controls.take_gesture_started() {
            self.input.release_buttons();
        }

        if redraw {
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

    /// Resizes the window from its own edges, on a window that has none of the
    /// window manager's.
    ///
    /// Returns whether the event was the border's rather than the
    /// application's. Nothing happens at all under native chrome, and nothing
    /// happens on macOS, where a client-decorated window still has a frame and
    /// the system is still resizing it.
    fn handle_resize_border(&mut self, event: &Event) -> bool {
        if !self.options.chrome.is_frameless() {
            return false;
        }

        // A maximised window has no outside to drag towards, and a fullscreen
        // one is not a window with edges at all.
        let resizable = !self.controls.is_maximized() && !self.controls.is_fullscreen();
        let size = self.with_window(|window| window.logical_size());
        let edge = |position| {
            resizable
                .then(|| edge_at(position, size, RESIZE_GRAB))
                .flatten()
        };

        match event {
            // Set on every move, including the one that leaves the border: a
            // pointer that kept the resize arrow over the middle of a terminal
            // would be worse than one that never showed it at all.
            Event::MouseMoved { position, .. } => {
                self.controls.set_resize_cursor(edge(*position));
                false
            }
            Event::MouseDown {
                button: MouseButton::Left,
                position,
                ..
            } => match edge(*position) {
                Some(edge) => {
                    self.controls.start_resize(edge);
                    true
                }
                None => false,
            },
            _ => false,
        }
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
