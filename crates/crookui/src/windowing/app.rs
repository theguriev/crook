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
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crookui_core::event::{Event, MouseButton, SystemTheme};
use crookui_core::executor::{Foreground, Runnable};
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform::FontDb;
use crookui_core::scene::Scene;
use parking_lot::Mutex;
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};

use crate::rendering::init_wgpu_instance;

use super::chrome::{RESIZE_GRAB, ResizeEdge, WindowChrome, WindowControls, edge_at};
use super::dock;
use super::event::InputState;
use super::placement::WindowPlacement;
use super::window::Window;

/// How a window is opened.
pub struct WindowOptions {
    /// The title the window manager shows.
    pub title: String,
    /// The initial inner size, in logical pixels — under `maximized` and
    /// `fullscreen`, the size the window goes back to when it leaves them.
    pub size: Vector2F,
    /// Where the window's top-left corner goes, in the desktop's own
    /// coordinates — see [`WindowPlacement::position`] — or `None` for
    /// wherever the desktop puts it.
    ///
    /// A position on none of the monitors there are now is moved onto one,
    /// and Wayland, where no application chooses where its window goes,
    /// never reads it.
    pub position: Option<[i32; 2]>,
    /// Whether the window opens maximised.
    pub maximized: bool,
    /// Whether the window opens fullscreen, on the monitor `position` is on.
    pub fullscreen: bool,
    /// The smallest inner size the user may drag it to, in logical pixels.
    pub min_size: Vector2F,
    /// Who draws the title bar the window's controls sit in.
    pub chrome: WindowChrome,
    /// Whether the window may be see-through where the scene is.
    pub transparent: bool,
    /// What a Linux desktop calls the window: Wayland's `app_id` and X11's
    /// `WM_CLASS`, which are the same fact under two names.
    ///
    /// It is what a window rule matches, what a dock groups by, and what a
    /// `.desktop` file's `StartupWMClass` ties the window to, so the default
    /// is one name for every channel: the dev build says it is the dev build
    /// in its *title*, which is what a person reads, and a rule that stopped
    /// matching because they ran a different build of the same application
    /// would be a rule nobody could debug. macOS and Windows have no such
    /// field and never read it.
    pub app_id: String,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Crook".to_owned(),
            size: vec2f(1024., 640.),
            position: None,
            maximized: false,
            fullscreen: false,
            min_size: vec2f(480., 192.),
            chrome: WindowChrome::default(),
            transparent: true,
            app_id: "crook".to_owned(),
        }
    }
}

/// What the window asks of the application above it.
///
/// This is the whole seam between the platform layer and the application:
/// nine methods, six of them with a default, no winit types, no wgpu types. A
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
    /// shortcut for closing a window, a window manager closing it — or, on
    /// macOS, the application's Quit, when [`Self::asks_before_quitting`]
    /// said a quit would ask.
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

    /// Whether quitting now would stop to ask first, as
    /// [`Self::close_requested`] would by putting a question up.
    ///
    /// For macOS's Quit — the application menu's, `cmd-q`, the Dock's, a
    /// logout's — which AppKit sends the application rather than the window
    /// and wants answered before the call returns, where the application
    /// cannot be reached. So this is asked each time the event loop goes
    /// idle, and the answer written down: `false` lets the next Quit end the
    /// process at once, as it always has, and `true` refuses it and asks
    /// [`Self::close_requested`] instead. Nothing else asks it, and nothing
    /// asks it off macOS.
    ///
    /// The default never asks, which is right for a window with nothing in it
    /// to lose.
    fn asks_before_quitting(&self) -> bool {
        false
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

    /// The window is somewhere else, another size, or maximised, restored or
    /// made fullscreen: `placement` is where it is now, as a later launch
    /// would open it with [`WindowOptions`].
    ///
    /// Told once as the window opens, then after a batch of events that moved
    /// or resized it if that changed it, and once more as the event loop
    /// stops if it changed without either — a desktop that maximises a window
    /// may say so only with a resize. Never twice the same. The default keeps
    /// nothing, which is right for a window that opens wherever the desktop
    /// puts it.
    fn placed(&mut self, placement: WindowPlacement) {
        let _ = placement;
    }

    /// The desktop asked the application to open these files, in this order:
    /// on macOS, a file Finder hands it — double-clicked once the application
    /// is the one that opens its kind, opened with it, dropped on its Dock
    /// icon — or `open -a` names.
    ///
    /// Full paths, of files and folders alike; a URL of any other kind never
    /// reaches here. Return `true` only when something changed and the frame
    /// needs rebuilding, as [`Self::handle_event`] does.
    ///
    /// Before the window's first frame when the request is what launched the
    /// application, and never off macOS: no other desktop hands a running
    /// application files to open — a Linux launcher starts a new process with
    /// them on its command line. The default opens nothing.
    fn open_files(&mut self, files: Vec<PathBuf>) -> bool {
        let _ = files;
        false
    }

    /// Runs once, as the event loop stops, whatever stopped it.
    ///
    /// The one place an application hears that it is ending. Only some of
    /// the ways a window closes pass through the application first — macOS's
    /// Quit with nothing to ask about goes straight to the platform, and ends
    /// the process without [`run`] ever returning — and every one of them
    /// passes through here.
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
    /// macOS's Quit was refused because a quit would ask: ask, as the
    /// window's own close does. See [`quit`](super::quit).
    #[cfg(target_os = "macos")]
    QuitRequested,
    /// macOS asked the application to open these files. See
    /// [`open_documents`](super::open_documents).
    #[cfg(target_os = "macos")]
    OpenFiles(Vec<PathBuf>),
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

    // Now, because winit hands the application its delegate as the loop is
    // built, and before the loop runs, which is when a Quit can first come.
    #[cfg(target_os = "macos")]
    super::quit::install({
        let proxy = proxy.clone();
        move || proxy.send(CrookEvent::QuitRequested)
    });
    // Then too, and for a reason of its own: a request to open a file that
    // launched the application comes while the loop starts, before its first
    // turn, and the proxy holds it until the window is open.
    #[cfg(target_os = "macos")]
    super::open_documents::install({
        let proxy = proxy.clone();
        move |files| proxy.send(CrookEvent::OpenFiles(files))
    });

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
        minimised_asked: None,
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

    /// When the desktop was last asked whether the window is minimised, and
    /// when an event last asked it, for asking again [`MINIMISED_LOOK`]
    /// later while the answer is all that holds the window out of sight —
    /// for [`MINIMISED_LOOKS_FOR`] after that event and no longer.
    minimised_asked: Option<MinimisedAsked>,
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

/// How long an answer that the window is minimised stands before the desktop
/// is asked again, while nothing else holds the window out of sight
/// (`InputState::held_by_an_answer`).
///
/// Only X11 is asked, and its window manager may map the window it restores
/// before it takes `_NET_WM_STATE_HIDDEN` off. A window brought back without
/// the keyboard and asked by its first frame, too soon, would otherwise stay
/// out of sight until somebody typed or clicked in it: a window out of sight
/// asks for no frame of its own, its panes' output included, and a frame is
/// where the question is otherwise asked. A second is how long such a window
/// can look frozen.
const MINIMISED_LOOK: Duration = Duration::from_secs(1);

/// How long after an event asked whether the window is minimised it goes on
/// being asked again by itself, [`MINIMISED_LOOK`] apart.
///
/// The events that ask — the keyboard moving, a frame — are what a restore
/// comes with: X11 says the focus again as it maps a window and exposes it,
/// which asks for a frame. The lag being put right is between two things one
/// window manager does for one restore, so an answer still saying minimised
/// this long after the event that asked is a window that is minimised. It is
/// left alone from then on, because the window keeps no idle timer, and a
/// window left minimised is the one most at rest: a clock a second there is
/// 3600 wakes, and as many round trips to the server, for each hour it stays
/// down.
const MINIMISED_LOOKS_FOR: Duration = Duration::from_secs(4);

impl ApplicationHandler<CrookEvent> for App {
    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        // A timer is set for a failed frame, and for a refused fullscreen in
        // the window's first seconds and an answer that the window is
        // minimised in the seconds after an event asked, which
        // `about_to_wait` asks again for. Reaching it with a frame waiting
        // means trying that frame again.
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && self.frame_retry.is_some()
            && let Some(window) = self.window.as_mut()
        {
            window.request_redraw();
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // A Quit that comes after this has no loop left to ask in, and a
        // refused one would leave a process that will not quit.
        #[cfg(target_os = "macos")]
        super::quit::set_asks(false);
        // Before the delegate hears it is ending, which is its last chance to
        // keep anything: a maximise the desktop reported only as a resize,
        // with the state changing after it, is known only by looking again.
        self.follow_placement();
        self.delegate.exiting();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // A drop whose last file was the last event of the batch: nothing
        // else may come for minutes, and the drop is over now.
        self.deliver_drop(None);

        // Before the look below: a fullscreen given up on here changes what
        // that look finds.
        let now = Instant::now();
        let fullscreen_look = self
            .window
            .as_mut()
            .and_then(|window| window.keep_asking_for_fullscreen(now));

        // Once the batch is over and not at each event in it, whose order
        // can say a window is somewhere before it says why.
        if let Some(placement) = self.window.as_mut().and_then(Window::settle_placement) {
            self.delegate.placed(placement);
        }

        // An answer that the window is minimised, once it is old enough to
        // ask again and the event that asked is recent enough: see
        // `MINIMISED_LOOK`. Here rather than only when the timer set for it
        // is reached, since a batch of something else — a pane's output,
        // which a window out of sight draws no frame for — may come first
        // every time.
        if self.window.is_some()
            && self.minimised_look().is_some_and(|look| look <= now)
            && self.look_again_whether_minimised(now)
        {
            self.with_window(Window::request_redraw);
        }

        // Every batch of events ends here, and a Quit that comes before the
        // next one is answered with what is written down now.
        #[cfg(target_os = "macos")]
        super::quit::set_asks(self.delegate.asks_before_quitting());

        let frame_retry = self.frame_retry.map(|delay| now + delay);
        let wake = [frame_retry, fullscreen_look, self.minimised_look()]
            .into_iter()
            .flatten()
            .min();
        event_loop.set_control_flow(match wake {
            Some(wake) => ControlFlow::WaitUntil(wake),
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
                let focused = window.has_focus();

                // Before the delegate hears anything: the controls handle is
                // what a header's drag, zoom or close reaches the real window
                // through, and the first event can arrive as soon as the
                // delegate is called below.
                self.controls.attach(window.handle());
                self.window = Some(window);
                if tell_the_opening(self.delegate.as_mut(), theme, focused) {
                    self.with_window(Window::request_redraw);
                }

                // Where the desktop actually put it, which for a window the
                // application did not place is somewhere only the desktop
                // knew until now. Told whether or not the look changed it,
                // since the application has heard nothing of it yet.
                let placement = self.with_window(|window| {
                    window.follow_placement();
                    window.placement()
                });
                self.delegate.placed(placement);
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
            // The same answer as `CloseRequested`'s, for the same reason.
            #[cfg(target_os = "macos")]
            CrookEvent::QuitRequested => {
                if self.delegate.close_requested() {
                    event_loop.exit();
                }
            }
            // Whether or not the window is open yet: the application has had
            // its workspace since before the loop ran, and the window's first
            // frame draws whatever the files opened.
            #[cfg(target_os = "macos")]
            CrookEvent::OpenFiles(files) => {
                if self.delegate.open_files(files)
                    && let Some(window) = self.window.as_mut()
                {
                    window.request_redraw();
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

        // Before this event, whatever it is: a drop is told a file at a time,
        // every file of it before anything else, so the first event that is
        // not one of its files is the end of it — and the application hears
        // the drop before the thing that came after it.
        self.deliver_drop(Some(&event));

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

            // Reconfiguring the swapchain here would cost one `configure` per
            // resize event; flagging it costs one per frame instead.
            //
            // And the application hears of a size with no room in it, which
            // is how Windows minimises a window and the only way it says so.
            // See `InputState::resized`.
            WindowEvent::Resized(size) => {
                self.with_window(|window| {
                    window.mark_resized();
                    window.placement_stirred();
                });
                if let Some(occluded) = self.input.resized(size)
                    && self.delegate.handle_event(occluded)
                {
                    self.with_window(Window::request_redraw);
                }
                return;
            }
            // A scale change is a resize too, because the swapchain is sized in
            // physical pixels even when the logical size did not move.
            WindowEvent::ScaleFactorChanged { .. } => {
                self.with_window(|window| {
                    window.mark_resized();
                    window.placement_stirred();
                });
                return;
            }

            WindowEvent::Moved(_) => {
                self.with_window(Window::placement_stirred);
                return;
            }

            // A new frame may have moved the world out from under a pointer
            // that never moved — a menu opened under it, a link lit up when
            // the chord key went down — so the pointer is asked again.
            //
            // And before the frame, a window that may be minimised asks
            // whether it is, since X11 never says: see
            // `InputState::minimised`. A frame is the moment to ask because
            // one asked for is what a hidden window would waste, and one
            // drawn when it comes back — X11 exposes a window it maps again —
            // is the first sign of that.
            WindowEvent::RedrawRequested => {
                if self.input.may_be_minimised() && self.ask_whether_minimised() {
                    self.with_window(Window::request_redraw);
                }
                if self.redraw(event_loop) {
                    self.update_pointer();
                }
                return;
            }

            // Taking the keyboard is the look a request for attention asked
            // for, so the request is over — whichever asked, a close's
            // question or a pane waiting. Taken back here rather than left to
            // the application, because only X11 needs taking back — it keeps
            // its urgency hint until somebody removes it — and nothing above
            // this line should have to know which desktop it is on. It is also
            // the moment the application's next keys stop being certainly its
            // own. Then on to the delegate as an event, like losing the
            // keyboard, because the workspace keeps whether the window is in
            // front.
            //
            // Gained or lost, the keyboard moving is also when a window is
            // minimised or comes back, so it asks whether it is: losing it is
            // how a minimise starts, and X11 says the focus again when it
            // maps a window it had minimised. The keyboard is noted first,
            // since a window that has it is in sight whatever the answer
            // says: see `InputState::occlusion`.
            WindowEvent::Focused(focused) => {
                if focused {
                    self.controls.focused();
                    if std::mem::take(&mut self.attention_requested) {
                        self.with_window(|window| window.withdraw_attention_request());
                    }
                    self.delegate.focused();
                }
                self.input.focus_moved(focused);
                if self.ask_whether_minimised() {
                    self.with_window(Window::request_redraw);
                }
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
        // A release asks too, not only a move: the drag it ends kept the
        // pointer it started with, and may have carried it somewhere else.
        let pointer_moved = matches!(event, Event::MouseMoved { .. } | Event::MouseUp { .. });
        let redraw = !grabbed && self.delegate.handle_event(event);

        // After both of them, which is the whole point. The border starts a
        // gesture above, but a window *move* is started by the header, inside
        // the delegate's own dispatch: asking before that ran read the flag one
        // event late, and one event is long enough for the first pointer move
        // after the drag to be converted as a drag with no button behind it.
        if self.controls.take_gesture_started() {
            self.input.release_buttons();
        }

        // Before the redraw is asked for, which throws the frame on screen
        // away: it is the one this move was made over.
        if pointer_moved {
            self.update_pointer();
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

    /// Asks the desktop whether the window is minimised, for an event that
    /// may come with the window being minimised or brought back, and tells
    /// the application when the answer changed whether it can be seen.
    /// Returns whether the application wants a frame for it.
    ///
    /// Asked rather than heard, because X11 never says: see
    /// [`InputState::minimised`]. Elsewhere the answer is `None`, since the
    /// rest either say it by themselves or cannot: see
    /// [`Window::is_minimised`].
    fn ask_whether_minimised(&mut self) -> bool {
        self.minimised_asked = Some(MinimisedAsked::by_an_event(Instant::now()));
        self.hear_whether_minimised()
    }

    /// [`Self::ask_whether_minimised`] for the look at `now` rather than for
    /// an event. It is noted as a look, so the looking still stops
    /// [`MINIMISED_LOOKS_FOR`] after the event that started it.
    fn look_again_whether_minimised(&mut self, now: Instant) -> bool {
        self.minimised_asked = self.minimised_asked.map(|asked| asked.looked(now));
        self.hear_whether_minimised()
    }

    /// The question itself, for [`Self::ask_whether_minimised`] and
    /// [`Self::look_again_whether_minimised`].
    fn hear_whether_minimised(&mut self) -> bool {
        let minimised = self.with_window(|window| window.is_minimised());
        self.input
            .minimised(minimised)
            .is_some_and(|occluded| self.delegate.handle_event(occluded))
    }

    /// When to ask the desktop again whether the window is minimised, if the
    /// last answer is all that holds the window out of sight and an event
    /// asked recently enough. See [`MINIMISED_LOOK`].
    fn minimised_look(&self) -> Option<Instant> {
        self.minimised_asked?
            .next_look(self.input.held_by_an_answer())
    }

    /// Tells the application where the window is, if that changed since it
    /// was last told.
    fn follow_placement(&mut self) {
        if let Some(placement) = self.window.as_mut().and_then(Window::follow_placement) {
            self.delegate.placed(placement);
        }
    }

    /// Hands the application the drop that `next` ends, if one is arriving
    /// and `next` is not another of its files; `None` is the end of the
    /// batch of events, which ends any drop.
    ///
    /// Straight to the application, past the resize border: a drop is not a
    /// press, and carries no position for the border to claim it by.
    fn deliver_drop(&mut self, next: Option<&WindowEvent>) {
        let Some(dropped) = self.input.finish_drop(next) else {
            return;
        };
        if self.delegate.handle_event(dropped)
            && let Some(window) = self.window.as_mut()
        {
            window.request_redraw();
        }
    }

    /// Resizes the window from its own edges, on a window that has none of the
    /// window manager's.
    ///
    /// Returns whether the event was the border's rather than the
    /// application's. Nothing happens at all under native chrome, and nothing
    /// happens on macOS, where a client-decorated window still has a frame and
    /// the system is still resizing it.
    fn handle_resize_border(&mut self, event: &Event) -> bool {
        let Event::MouseDown {
            button: MouseButton::Left,
            position,
            ..
        } = event
        else {
            return false;
        };

        match self.resize_edge(*position) {
            Some(edge) => {
                self.controls.start_resize(edge);
                true
            }
            None => false,
        }
    }

    /// The edge of a frameless window `position` is on, if it is on one and
    /// the window has edges to drag.
    fn resize_edge(&mut self, position: Vector2F) -> Option<ResizeEdge> {
        // A maximised window has no outside to drag towards, and a fullscreen
        // one is not a window with edges at all.
        if !self.options.chrome.is_frameless()
            || self.controls.is_maximized()
            || self.controls.is_fullscreen()
        {
            return None;
        }
        let size = self.with_window(|window| window.logical_size());
        edge_at(position, size, RESIZE_GRAB)
    }

    /// Shows the pointer for wherever it is now: the border's arrows on a
    /// frameless window's edge, and what the frame on screen asks for
    /// everywhere else.
    ///
    /// Set every time, including when the pointer leaves the border: a pointer
    /// that kept the resize arrow over the middle of a terminal would be worse
    /// than one that never showed it at all.
    ///
    /// Except while a button is held. A drag keeps the pointer it started
    /// with — the resize arrows across a divider drag that the split follows a
    /// frame behind, the I-beam across a selection dragged over the tabs
    /// panel — which is what keeps it from flickering between shapes under a
    /// pointer moving faster than the layout, and what a person dragging a
    /// split or a selection in any other application sees.
    fn update_pointer(&mut self) {
        if self.input.is_dragging() {
            return;
        }
        let position = self.input.pointer();
        let edge = self.resize_edge(position);
        let asked = self.with_window(|window| window.pointer_at(position));
        // Between a change and its frame there is nothing to ask but the
        // border, and the frame asks again when it lands.
        if edge.is_none() && asked.is_none() {
            return;
        }
        self.controls.show_pointer(edge, asked.unwrap_or_default());
    }

    /// Draws a frame, reporting whether a new one was built and reached the
    /// screen.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> bool {
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
            return false;
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
            return rebuilt;
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
        false
    }
}

/// Tells `delegate` what the window it is about to draw in is like, before
/// any frame, and returns whether that needs a frame.
///
/// The desktop's theme, which winit only reports when it changes. And that
/// the window does not have the keyboard, when it opens without it: X11 and
/// macOS say so themselves as the window appears, but on Wayland the first
/// word of focus is the keyboard arriving, and a window the compositor opened
/// behind another — a script's `crook -e`, a rule that sends it to another
/// workspace — was taken as in front, where nothing in it asks for attention,
/// until somebody had been to it once. Nothing is said for a window that has
/// the keyboard, which is what an application takes a window to have until
/// told otherwise.
fn tell_the_opening(delegate: &mut dyn WindowDelegate, theme: SystemTheme, focused: bool) -> bool {
    let themed = delegate.handle_event(Event::SystemTheme(theme));
    let unfocused = !focused && delegate.handle_event(Event::WindowFocused(false));
    themed || unfocused
}

/// When the desktop was asked whether the window is minimised: last of all,
/// and last by an event rather than by the look that follows one.
#[derive(Clone, Copy, Debug)]
struct MinimisedAsked {
    /// The last time it was asked, by anything.
    last: Instant,
    /// The last time an event asked: the keyboard moving, or a frame.
    by_an_event: Instant,
}

impl MinimisedAsked {
    /// Asked at `now` by an event.
    fn by_an_event(now: Instant) -> Self {
        Self {
            last: now,
            by_an_event: now,
        }
    }

    /// Asked again at `now` by the look, which leaves the event that started
    /// the looking, and so when it stops, where they were.
    fn looked(self, now: Instant) -> Self {
        Self { last: now, ..self }
    }

    /// When to ask again, given whether the answer is all that holds the
    /// window out of sight: [`MINIMISED_LOOK`] after it was last asked, if
    /// that is no later than [`MINIMISED_LOOKS_FOR`] after an event last
    /// asked.
    fn next_look(self, held_by_an_answer: bool) -> Option<Instant> {
        let look = self.last + MINIMISED_LOOK;
        (held_by_an_answer && look <= self.by_an_event + MINIMISED_LOOKS_FOR).then_some(look)
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

    /// A delegate that only writes down what it was told.
    #[derive(Default)]
    struct Told(Vec<Event>);

    impl WindowDelegate for Told {
        fn build_scene(&mut self, _: Vector2F, scale_factor: f32) -> Rc<Scene> {
            Rc::new(Scene::new(scale_factor))
        }

        fn handle_event(&mut self, event: Event) -> bool {
            self.0.push(event);
            true
        }

        fn frame_drawn(&mut self) {}
    }

    /// Whether `told` holds a `WindowFocused` saying `focused`.
    fn says_focused(told: &Told, focused: bool) -> bool {
        told.0
            .iter()
            .any(|event| matches!(event, Event::WindowFocused(said) if *said == focused))
    }

    #[test]
    fn a_window_that_opens_without_the_keyboard_says_so_before_its_first_frame() {
        // Wayland's case: no word of focus comes until the keyboard does, so
        // unless this says it, a window opened behind another is taken as in
        // front and nothing in it asks for a look.
        let mut told = Told::default();
        assert!(tell_the_opening(&mut told, SystemTheme::Dark, false));
        assert!(
            says_focused(&told, false),
            "the window opened without the keyboard and the application was not told: {:?}",
            told.0
        );
        assert!(
            matches!(told.0.first(), Some(Event::SystemTheme(SystemTheme::Dark))),
            "the theme is still told first: {:?}",
            told.0
        );
    }

    #[test]
    fn a_window_that_opens_with_the_keyboard_says_nothing_about_focus() {
        // What an application takes a window to have until told otherwise,
        // so saying it would only be a second thing to get wrong.
        let mut told = Told::default();
        tell_the_opening(&mut told, SystemTheme::Light, true);
        assert!(!says_focused(&told, true) && !says_focused(&told, false));
        assert!(matches!(
            told.0.as_slice(),
            [Event::SystemTheme(SystemTheme::Light)]
        ));
    }

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

    #[test]
    fn an_answer_that_the_window_is_minimised_is_asked_again_only_while_it_is_all_that_hides_it() {
        let event = Instant::now();
        let asked = MinimisedAsked::by_an_event(event);
        assert_eq!(
            asked.next_look(true),
            Some(event + MINIMISED_LOOK),
            "a window out of sight on an answer alone was never asked again"
        );
        // Anything else holding it out of sight says when it is over, and a
        // window in sight is asked by its frames.
        assert_eq!(asked.next_look(false), None);
    }

    #[test]
    fn a_window_left_minimised_stops_being_asked_a_few_seconds_after_the_event_that_asked() {
        // A restore comes with the focus and a frame, which ask; nothing
        // comes for a window that stays minimised, which must then wait for
        // the next event rather than wake every second for as long as it
        // stays down.
        let event = Instant::now();
        let mut asked = MinimisedAsked::by_an_event(event);
        let mut looks = 0;
        while let Some(look) = asked.next_look(true) {
            assert!(
                look <= event + MINIMISED_LOOKS_FOR,
                "a window left minimised was still asked {:?} after the event that asked",
                look - event
            );
            looks += 1;
            asked = asked.looked(look);
        }
        assert_eq!(
            looks,
            MINIMISED_LOOKS_FOR.as_millis() / MINIMISED_LOOK.as_millis(),
            "the looking stopped before the seconds after the event were up"
        );

        // The next event that asks — a restore's, an hour later — looks
        // again.
        let restored = event + Duration::from_secs(3600);
        assert_eq!(
            MinimisedAsked::by_an_event(restored).next_look(true),
            Some(restored + MINIMISED_LOOK)
        );
    }
}
