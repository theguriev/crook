//! Crook: a terminal whose unit of work is an agent.
//!
//! This is the application. Everything below it is general — [`crookui_core`]
//! is a UI framework, [`crookui`] is a renderer and a window, [`crook_usage`]
//! is a client for one endpoint — and everything here is Crook: a strip of
//! agent sessions, a header that shows how much Claude is left, and the wiring
//! that turns one into pixels and the other into a number.
//!
//! # What a run looks like
//!
//! 1. Resolve the fonts, because a family id is needed before any view exists.
//! 2. Open the event loop, which hands back the main-thread executor.
//! 3. Build the [`App`], register the [`UsageModel`] singleton, add the window
//!    with a [`Workspace`] root, and start the poll chain.
//! 4. Point the app's invalidation callback at the window's redraw request,
//!    which is the only thing that makes a frame happen.
//!
//! Nothing polls and nothing spins: the process sleeps until the OS or a
//! finished background task has something to say.
//!
//! # Running it without a window
//!
//! `--snapshot <path>` renders one frame of the real view tree through the
//! offscreen renderer and exits. It is not a hand-built scene: it goes through
//! `View::render`, layout and paint exactly as the window does, which is what
//! makes it worth having in CI on a machine with no display.

pub mod git;
pub mod platform_insets;
pub mod process;
pub mod settings;
pub mod tab;
pub mod theme;
pub mod usage_model;
pub mod workspace;

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use crookui::{CosmicFontDb, Platform, Proxy, WindowDelegate, WindowOptions, render_scene_to_rgba};
use crookui_core::event::Event;
use crookui_core::executor::{Background, LocalQueue};
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform::TextLayoutSystem;
use crookui_core::prelude::*;
use crookui_core::scene::Scene;
use crookui_core::{AddSingletonModel as _, App, Presenter, WindowId};

use crate::platform_insets::WindowChrome;
use crate::tab::{AgentStatus, Direction, PaneId, Tab, TabAction};
use crate::usage_model::UsageModel;
use crate::workspace::{Fonts, QuitRequest, Workspace};

/// The window Crook opens, in logical pixels.
const WINDOW_SIZE: Vector2F = vec2f(1024., 640.);

/// Who draws Crook's window controls.
///
/// The window manager does: Crook opens a decorated window, so the controls
/// live in a title bar above the client area and the header underneath owes
/// them no room. The header asks for its insets with this, so the two facts
/// cannot drift apart — [`open_window`] derives `decorations` from it.
pub const WINDOW_CHROME: WindowChrome = WindowChrome::Native;

/// The scale factor the headless snapshot renders at. Two, because that is
/// where subpixel glyph positioning and the atlas are actually exercised.
const SNAPSHOT_SCALE_FACTOR: f32 = 2.;

/// Which build of Crook this is.
///
/// Channels exist so a development build and a shipped one can be installed
/// side by side without fighting over each other's state. Today they differ
/// only in what the title bar says; the enum is here because retrofitting it
/// is painful and adopting it now is free.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Built from a working tree, run by whoever wrote it.
    Dev,
    /// Built for release.
    Stable,
}

impl Channel {
    /// The name this channel is known by, in logs and in `--version`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Stable => "stable",
        }
    }

    /// What the window manager shows in the title bar.
    pub fn window_title(self) -> String {
        match self {
            Self::Dev => "Crook (dev)".to_owned(),
            Self::Stable => "Crook".to_owned(),
        }
    }
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Startup {
    /// Open a window, optionally for a fixed number of frames.
    Window {
        /// Exit after this many frames, so the binary is runnable unattended.
        frames: Option<u32>,
    },
    /// Render one frame to a PNG and exit.
    Snapshot {
        /// Where to write it.
        path: PathBuf,
    },
    /// The argument was answered on stdout; there is nothing left to do.
    Answered,
}

/// Runs Crook.
///
/// Returns when the window closes, or immediately for `--help`, `--version`
/// and `--snapshot`.
pub fn run(channel: Channel) -> Result<()> {
    attach_to_parent_console();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    match parse_args(channel, std::env::args().skip(1))? {
        Startup::Answered => Ok(()),
        Startup::Snapshot { path } => write_snapshot(&path),
        Startup::Window { frames } => open_window(channel, frames),
    }
}

/// Reconnects this process's standard streams to the console it was started
/// from, on the one platform where they are not connected already.
///
/// A shipped Windows build is linked for the `windows` subsystem so that
/// double-clicking it does not flash a console window. The cost is that every
/// command-line path of the same binary writes to handles that do not exist:
/// `--help`, `--version`, the line `--snapshot` prints, the logger, and the
/// message a failed startup dies with would all vanish, exactly the way they
/// do not on macOS and Linux — which is why this cannot be noticed in
/// development.
fn attach_to_parent_console() {
    #[cfg(windows)]
    {
        /// `ATTACH_PARENT_PROCESS`: attach to whatever console the parent has.
        const PARENT_PROCESS: u32 = u32::MAX;

        // Declared rather than depended on. `kernel32` is already linked into
        // every Windows target, so one function does not justify a crate — and
        // this stays a workspace with no build scripts.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }

        // Fails when the parent has no console — launched from Explorer, or a
        // console subsystem build that already has one. Both are cases where
        // there is nothing to attach to and nothing to do about it.
        unsafe { AttachConsole(PARENT_PROCESS) };
    }
}

fn parse_args(channel: Channel, args: impl Iterator<Item = String>) -> Result<Startup> {
    let mut args = args.peekable();
    let mut frames = None;
    let mut snapshot = None;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                println!("{}", help_text());
                return Ok(Startup::Answered);
            }
            "-V" | "--version" => {
                println!("crook {} ({})", env!("CARGO_PKG_VERSION"), channel.name());
                return Ok(Startup::Answered);
            }
            "--snapshot" => {
                let path = args.next().context("`--snapshot` needs a path")?;
                snapshot = Some(PathBuf::from(path));
            }
            "--frames" => {
                let count = args.next().context("`--frames` needs a count")?;
                frames = Some(count.parse().context("`--frames` takes a number")?);
            }
            other => bail!("unrecognised argument {other}; try --help"),
        }
    }

    match snapshot {
        Some(path) => Ok(Startup::Snapshot { path }),
        None => Ok(Startup::Window { frames }),
    }
}

fn help_text() -> String {
    format!(
        "crook {version} \u{2014} a terminal whose unit of work is an agent

USAGE:
    crook [OPTIONS]

OPTIONS:
    --snapshot <PATH>  Render one frame of the real view tree to a PNG and exit
    --frames <N>       Draw N frames, then exit; for running unattended
    -h, --help         Print this message
    -V, --version      Print the version and channel

KEYS:
    cmd/ctrl-t                 New agent tab
    cmd/ctrl-d                 Split the focused pane to the right
    cmd/ctrl-shift-d           Split the focused pane downwards
    cmd/ctrl-w                 Close the focused pane, and its tab with the last one
    cmd/ctrl-shift-left/right  Select the previous/next tab
    cmd/ctrl-alt-left/right    Move the active tab",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// One worker is parked on the usage poll's timer between readings, so the
/// pool needs a second one to have anything left over for real work.
fn background_pool() -> Arc<Background> {
    let cores = std::thread::available_parallelism().map_or(1, |count| count.get());
    Arc::new(Background::new(cores.max(2)))
}

fn resolve_fonts(font_db: &CosmicFontDb) -> Result<Fonts> {
    Ok(Fonts {
        ui: font_db
            .default_ui_family()
            .context("no usable interface font")?,
        monospace: font_db
            .default_monospace_family()
            .context("no usable monospace font")?,
    })
}

fn open_window(channel: Channel, frames: Option<u32>) -> Result<()> {
    // Everything fallible happens before the event loop takes over, because
    // the delegate is built inside a closure that cannot report an error.
    let font_db = CosmicFontDb::new().context("no usable system fonts")?;
    let fonts = resolve_fonts(&font_db)?;
    let text_layout: Arc<dyn TextLayoutSystem> = Arc::new(font_db.text_layout());

    let options = WindowOptions {
        title: channel.window_title(),
        size: WINDOW_SIZE,
        decorations: WINDOW_CHROME == WindowChrome::Native,
        ..Default::default()
    };

    crookui::run(options, Box::new(font_db), move |platform| {
        Box::new(Shell::new(platform, fonts, text_layout, frames))
    })
}

/// Renders one frame of the real view tree and writes it to `path`.
fn write_snapshot(path: &std::path::Path) -> Result<()> {
    let font_db = CosmicFontDb::new().context("no usable system fonts")?;
    let fonts = resolve_fonts(&font_db)?;
    let text_layout: Arc<dyn TextLayoutSystem> = Arc::new(font_db.text_layout());

    // A local queue stands in for the event loop. Nothing is spawned onto it:
    // the usage poll is deliberately not started, so the frame is the same on
    // a build machine with no Claude Code session and no network.
    let mut app = App::new(LocalQueue::new().foreground(), background_pool());
    app.update(|ctx| ctx.add_singleton_model(UsageModel::new));

    let quit: QuitRequest = Rc::new(|| {});
    let (window_id, workspace) = app.add_window(|ctx| Workspace::new(fonts, quit, ctx));
    app.update(|ctx| workspace.update(ctx, seed_snapshot_tabs));

    let mut presenter = Presenter::new(window_id, text_layout);
    let scene = app.update(|ctx| {
        let invalidation = ctx.take_all_invalidations_for_window(window_id);
        presenter.invalidate(invalidation, ctx);
        presenter.build_scene(WINDOW_SIZE, SNAPSHOT_SCALE_FACTOR, ctx)
    });

    let (pixels, width, height) = render_scene_to_rgba(&scene, WINDOW_SIZE, &font_db)
        .context("failed to render the frame")?;

    let file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .context("failed to write the PNG header")?
        .write_image_data(&pixels)
        .context("failed to write the PNG body")?;

    println!("wrote {} ({width}x{height})", path.display());
    Ok(())
}

/// Fills the snapshot's strip with something worth looking at.
///
/// A window opens on one tab holding one pane; a rendering check wants the
/// states that cannot show it — an unselected row beside a selected one, each
/// status the dot has a colour for, and a tab split into two panels so the
/// body is not always a single box.
fn seed_snapshot_tabs(workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>) {
    workspace.apply(TabAction::New, ctx);
    workspace.apply(TabAction::New, ctx);

    let first_tab = workspace.tabs().iter().map(Tab::id).next();
    if let Some(first) = first_tab {
        workspace.apply(TabAction::Select(first), ctx);
    }
    workspace.apply(TabAction::Split(Direction::Right), ctx);

    let panes: Vec<PaneId> = workspace
        .tabs()
        .panes()
        .map(|(_, pane)| pane.id())
        .collect();
    let sessions = [
        ("port the tab bar", AgentStatus::Running),
        ("write the usage chip", AgentStatus::NeedsInput),
        ("bisect the flaky test", AgentStatus::Failed),
        ("read the recon notes", AgentStatus::Idle),
    ];

    for (id, (title, status)) in panes.iter().zip(sessions) {
        workspace.update_session(*id, ctx, |session| {
            session.derived_title = Some(title.to_owned());
            session.status = status;
        });
    }

    // The split tab's first pane: the body then shows two panels, one of them
    // carrying the focused pane's accent border.
    if let Some(first) = panes.first() {
        workspace.apply(TabAction::FocusPane(*first), ctx);
    }
}

/// The application as the window sees it.
///
/// Three methods, and each one is a translation: a size into a scene, an OS
/// event into an action, a finished frame into either another one or an exit.
struct Shell {
    app: App,
    presenter: Presenter,
    window_id: WindowId,
    workspace: ViewHandle<Workspace>,
    proxy: Proxy,
    frames_drawn: u32,
    frame_budget: Option<u32>,
}

impl Shell {
    fn new(
        platform: &Platform,
        fonts: Fonts,
        text_layout: Arc<dyn TextLayoutSystem>,
        frame_budget: Option<u32>,
    ) -> Self {
        let mut app = App::new(platform.foreground.clone(), background_pool());
        app.update(|ctx| ctx.add_singleton_model(UsageModel::new));

        let proxy = platform.proxy.clone();
        let quit: QuitRequest = {
            let proxy = proxy.clone();
            // Closing the last tab closes the window, which is what keeps the
            // strip from ever having to represent "no tabs".
            Rc::new(move || proxy.exit())
        };

        let (window_id, workspace) = app.add_window(|ctx| Workspace::new(fonts, quit, ctx));
        app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| workspace.start_usage_poll(ctx));
        });

        // The only thing that makes a frame happen: a view said it changed.
        let redraw = proxy.clone();
        app.on_window_invalidated(window_id, move |_, _| redraw.request_redraw());

        Self {
            app,
            presenter: Presenter::new(window_id, text_layout),
            window_id,
            workspace,
            proxy,
            frames_drawn: 0,
            frame_budget,
        }
    }

    /// Handles a keystroke, if it is bound to something.
    fn handle_keystroke(&mut self, event: &Event) -> bool {
        let Event::KeyDown { keystroke, .. } = event else {
            return false;
        };

        let action = self
            .workspace
            .read(&self.app, |workspace, _| workspace.action_for(keystroke));
        let Some(action) = action else {
            return false;
        };

        // Straight to the workspace: keys are not hit-tested, so there is no
        // element under the pointer to start the chain from.
        let chain = [self.workspace.id()];
        self.app
            .dispatch_typed_action(self.window_id, &chain, &action);
        true
    }
}

impl WindowDelegate for Shell {
    fn build_scene(&mut self, size: Vector2F, scale_factor: f32) -> Rc<Scene> {
        let window_id = self.window_id;
        let presenter = &mut self.presenter;

        self.app.update(|ctx| {
            let invalidation = ctx.take_all_invalidations_for_window(window_id);
            presenter.invalidate(invalidation, ctx);
            presenter.build_scene(size, scale_factor, ctx)
        })
    }

    fn handle_event(&mut self, event: Event) -> bool {
        if self.handle_keystroke(&event) {
            return true;
        }

        let window_id = self.window_id;
        let presenter = &mut self.presenter;
        self.app
            .update(|ctx| ctx.dispatch_window_event(window_id, event, presenter));

        // Asked after the update rather than inside it: an action handler's
        // `notify` is an effect, and effects only drain once the outermost
        // update has unwound.
        self.app.read(|ctx| ctx.has_window_invalidations(window_id))
    }

    fn frame_drawn(&mut self) {
        self.frames_drawn += 1;

        let Some(budget) = self.frame_budget else {
            return;
        };

        if self.frames_drawn >= budget {
            log::info!("drew {budget} frames; exiting");
            self.proxy.exit();
        } else {
            self.proxy.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Startup> {
        parse_args(
            Channel::Dev,
            args.iter().map(|argument| (*argument).to_owned()),
        )
    }

    #[test]
    fn no_arguments_opens_a_window_that_runs_until_it_is_closed() {
        assert_eq!(
            parse(&[]).expect("no arguments is valid"),
            Startup::Window { frames: None }
        );
    }

    #[test]
    fn frames_bounds_the_run() {
        assert_eq!(
            parse(&["--frames", "3"]).expect("a count is valid"),
            Startup::Window { frames: Some(3) }
        );
    }

    #[test]
    fn snapshot_takes_a_path_and_wins_over_a_frame_budget() {
        assert_eq!(
            parse(&["--frames", "3", "--snapshot", "/tmp/frame.png"])
                .expect("both are valid together"),
            Startup::Snapshot {
                path: PathBuf::from("/tmp/frame.png")
            }
        );
    }

    #[test]
    fn help_and_version_answer_before_anything_is_opened() {
        assert_eq!(parse(&["--help"]).expect("valid"), Startup::Answered);
        assert_eq!(parse(&["-V"]).expect("valid"), Startup::Answered);
    }

    #[test]
    fn an_argument_that_needs_a_value_and_has_none_is_an_error() {
        assert!(parse(&["--snapshot"]).is_err());
        assert!(parse(&["--frames"]).is_err());
        assert!(parse(&["--frames", "soon"]).is_err());
        assert!(parse(&["--tabs"]).is_err());
    }
}
