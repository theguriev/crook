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
pub mod git_model;
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
use crate::settings::{Density, Granularity, Layout, Settings};
use crate::tab::{AgentStatus, Direction, PaneId, Tab, TabAction};
use crate::usage_model::UsageModel;
use crate::workspace::{Fonts, QuitRequest, Section, Workspace};

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
        /// What the command line asked to start differently.
        overrides: Overrides,
    },
    /// Render one frame to a PNG and exit.
    Snapshot {
        /// Where to write it.
        path: PathBuf,
        /// What the command line asked to start differently.
        overrides: Overrides,
    },
    /// The argument was answered on stdout; there is nothing left to do.
    Answered,
}

/// Startup state a run was asked for rather than loaded.
///
/// These exist so a snapshot can show a state a fresh install is not in — a
/// menu is not much of a rendering check while it is closed, and neither is a
/// hover card nobody is hovering. The three option overrides are handed to the
/// matching `Workspace::override_*` method, which puts a value in front of the
/// renderer without letting it into the settings the next menu click saves, so
/// asking for one does not change the options of whoever asked.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct Overrides {
    /// Start with the tab options menu open.
    menu: bool,
    /// Start with the first row's hover detail card up.
    hover: bool,
    /// Start with a settings tab open, on this page of it.
    ///
    /// Unlike the three option overrides below it, this one has nothing to
    /// keep out of the settings file: which page of the settings somebody is
    /// looking at is not an option and is never written down. It does put an
    /// extra tab in the strip, which is the point — a snapshot of the
    /// settings page is a snapshot of a window with the settings open in it.
    settings: Option<Section>,
    /// Start in this layout rather than the saved one.
    layout: Option<Layout>,
    /// Start with rows standing for this rather than for the saved one.
    granularity: Option<Granularity>,
    /// Start in this density rather than the saved one.
    density: Option<Density>,
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
        Startup::Snapshot { path, overrides } => write_snapshot(&path, overrides),
        Startup::Window { frames, overrides } => open_window(channel, frames, overrides),
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
    let mut overrides = Overrides::default();

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
            "--menu" => overrides.menu = true,
            "--settings" => {
                // The section is optional, and a bare `--settings` opens the
                // page where a click on the menu entry opens it. Peeking
                // rather than consuming is what lets `--settings --hover`
                // mean what it looks like it means.
                let section = match args.peek().map(String::as_str) {
                    Some("appearance") => Some(Section::Appearance),
                    Some("usage") => Some(Section::Usage),
                    Some("keys") => Some(Section::Keys),
                    Some("about") => Some(Section::About),
                    Some(other) if !other.starts_with("--") => {
                        bail!("`--settings` takes appearance, usage, keys or about, not {other}");
                    }
                    _ => None,
                };
                if section.is_some() {
                    args.next();
                }
                overrides.settings = Some(section.unwrap_or_default());
            }
            "--hover" => overrides.hover = true,
            "--density" => {
                let mode = args.next().context("`--density` needs a mode")?;
                overrides.density = Some(match mode.as_str() {
                    "compact" => Density::Compact,
                    "expanded" => Density::Expanded,
                    other => bail!("`--density` takes compact or expanded, not {other}"),
                });
            }
            "--granularity" => {
                let mode = args.next().context("`--granularity` needs a mode")?;
                overrides.granularity = Some(match mode.as_str() {
                    "panes" => Granularity::Panes,
                    "tabs" => Granularity::Tabs,
                    other => bail!("`--granularity` takes panes or tabs, not {other}"),
                });
            }
            "--layout" => {
                let mode = args.next().context("`--layout` needs a mode")?;
                overrides.layout = Some(match mode.as_str() {
                    "vertical" => Layout::Vertical,
                    "horizontal" => Layout::Horizontal,
                    other => bail!("`--layout` takes vertical or horizontal, not {other}"),
                });
            }
            other => bail!("unrecognised argument {other}; try --help"),
        }
    }

    match snapshot {
        Some(path) => Ok(Startup::Snapshot { path, overrides }),
        None => Ok(Startup::Window { frames, overrides }),
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
    --menu             Start with the tab options menu open
    --settings [PAGE]  Start with a settings tab open, on `appearance`,
                       `usage`, `keys` or `about`
    --hover            Start with the first row's detail card up
    --layout <MODE>    Start with the tabs `vertical` or `horizontal` rather than as saved
    --granularity <M>  Start with rows standing for `panes` or `tabs` rather than as saved
    --density <MODE>   Start in `compact` or `expanded` density rather than the saved one
    -h, --help         Print this message
    -V, --version      Print the version and channel

KEYS:
    cmd/ctrl-t                 New agent tab
    cmd/ctrl-b                 Move the tabs between the side panel and the header strip
    cmd/ctrl-,                 Open the settings tab, or bring it forward
    cmd/ctrl-d                 Split the focused pane to the right
    cmd/ctrl-shift-d           Split the focused pane downwards
    cmd/ctrl-w                 Close the focused pane, and its tab with the last one
    cmd/ctrl-shift-left/right  Select the previous/next tab
    cmd/ctrl-alt-left/right    Move the active tab",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// How many workers are parked on a timer at any moment.
///
/// Two: the usage poll between readings, and the git gather between cycles.
/// Both are one background task for the whole cycle — the wait *and* the work
/// — so each holds its worker across a `recv_timeout` rather than yielding it,
/// and neither is ever counted as idle. Raise this when a third such chain
/// appears, and see the test at the bottom of this file for what happens if it
/// is not raised.
const PARKED_WORKERS: usize = 2;

/// A pool with a worker left over once both poll chains are asleep.
///
/// The floor is not a round number, it is [`PARKED_WORKERS`] plus one. On a
/// two-core machine a pool the size of the chains has nothing left to run a
/// settings save on, and a save is the one background task a click is waiting
/// for: it would sit in the queue until one of the timers expired, up to
/// fifteen seconds, and be discarded outright if the window closed first.
fn background_pool() -> Arc<Background> {
    let cores = std::thread::available_parallelism().map_or(1, |count| count.get());
    Arc::new(Background::new(cores.max(PARKED_WORKERS + 1)))
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

/// Puts the workspace into the state the command line asked to start in.
///
/// The options first, because the last two are read against them: the hover
/// card is armed on the first row of a list whose rows the granularity decides
/// and whose heights the density decides.
///
/// Each option goes to its `Workspace::override_*` method rather than into the
/// [`Settings`] the workspace was built from. Folding them into the settings is
/// the obvious arrangement and it is wrong: a menu click saves the *whole*
/// options snapshot, so the first click of the run would write the override to
/// the file and `--density expanded` — a way to look at a frame — would become
/// what every later launch does.
fn apply_overrides(
    workspace: &mut Workspace,
    overrides: Overrides,
    ctx: &mut ViewContext<Workspace>,
) {
    if let Some(layout) = overrides.layout {
        workspace.override_layout(layout, ctx);
    }
    if let Some(granularity) = overrides.granularity {
        workspace.override_granularity(granularity, ctx);
    }
    if let Some(density) = overrides.density {
        workspace.override_density(density, ctx);
    }
    if overrides.menu {
        workspace.open_options_menu(ctx);
    }
    if overrides.hover {
        workspace.hover_first_row(ctx);
    }
    if let Some(section) = overrides.settings {
        workspace.open_settings_page(section, ctx);
    }
}

fn open_window(channel: Channel, frames: Option<u32>, overrides: Overrides) -> Result<()> {
    // Everything fallible happens before the event loop takes over, because
    // the delegate is built inside a closure that cannot report an error.
    let font_db = CosmicFontDb::new().context("no usable system fonts")?;
    let fonts = resolve_fonts(&font_db)?;
    let text_layout: Arc<dyn TextLayoutSystem> = Arc::new(font_db.text_layout());
    // Blocking, and deliberately: one small file, read once, before there is a
    // window to stall.
    let settings = Settings::for_user();

    let options = WindowOptions {
        title: channel.window_title(),
        size: WINDOW_SIZE,
        decorations: WINDOW_CHROME == WindowChrome::Native,
        ..Default::default()
    };

    crookui::run(options, Box::new(font_db), move |platform| {
        Box::new(Shell::new(
            platform,
            fonts,
            settings.clone(),
            channel,
            text_layout.clone(),
            frames,
            overrides,
        ))
    })
}

/// Renders one frame of the real view tree and writes it to `path`.
fn write_snapshot(path: &std::path::Path, overrides: Overrides) -> Result<()> {
    let font_db = CosmicFontDb::new().context("no usable system fonts")?;
    let fonts = resolve_fonts(&font_db)?;
    let text_layout: Arc<dyn TextLayoutSystem> = Arc::new(font_db.text_layout());

    // A local queue stands in for the event loop. Nothing is spawned onto it:
    // neither the usage poll nor the git gather is started, so the frame is the
    // same on a build machine with no Claude Code session, no network and no
    // repository — and it uses ephemeral settings, so it is also the same
    // whatever options the person running it happens to have.
    let mut app = App::new(LocalQueue::new().foreground(), background_pool());
    app.update(|ctx| ctx.add_singleton_model(UsageModel::new));

    let quit: QuitRequest = Rc::new(|| {});
    let settings = Settings::ephemeral();
    // A snapshot is always rendered as the dev channel: the only thing the
    // channel reaches is the About page's label, and a PNG that said "stable"
    // on a machine that built it from a working tree would be wrong in the one
    // way a snapshot exists to catch.
    let (window_id, workspace) =
        app.add_window(|ctx| Workspace::new(fonts, settings, Channel::Dev, quit, ctx));
    app.update(|ctx| {
        workspace.update(ctx, |workspace, ctx| {
            seed_snapshot_tabs(workspace, ctx);
            apply_overrides(workspace, overrides, ctx);
        });
    });

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
/// A window opens on one tab holding one pane in the directory Crook was
/// started from; a rendering check wants the states that cannot show — an
/// unselected row beside a selected one, each status the dot has a colour for,
/// a tab split into two panels, and four different repositories so that every
/// "Pane title as" mode has something distinct to print.
///
/// The git facts are recorded rather than read. The snapshot never starts the
/// gather chain — it must render the same frame on a machine with no `git` and
/// no checkout — and a frame with no branch on any row would not show what the
/// options actually do.
fn seed_snapshot_tabs(workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>) {
    /// One seeded session: its name, what its agent is doing, where it works,
    /// and what git says about there.
    struct Seeded {
        title: &'static str,
        status: AgentStatus,
        directory: &'static str,
        branch: &'static str,
        diff: Option<(u32, u32, u32)>,
    }

    let seeded = [
        Seeded {
            title: "port the tab bar",
            status: AgentStatus::Running,
            directory: "app/src/workspace",
            branch: "eugen/tab-options",
            diff: Some((6, 214, 37)),
        },
        Seeded {
            title: "write the usage chip",
            status: AgentStatus::NeedsInput,
            directory: "crates/crook_usage/src",
            branch: "main",
            diff: Some((1, 12, 0)),
        },
        Seeded {
            title: "bisect the flaky test",
            status: AgentStatus::Failed,
            directory: "crates/crookui/src/rendering",
            branch: "eugen/atlas-repro",
            diff: None,
        },
        Seeded {
            title: "read the recon notes",
            status: AgentStatus::Idle,
            directory: "docs",
            branch: "main",
            diff: None,
        },
    ];

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

    let root = std::env::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join("work")
        .join("crook");

    for (id, seed) in panes.iter().zip(seeded) {
        let directory = root.join(seed.directory);
        let facts = git::GitFacts {
            branch: Some(git::Head::Branch(seed.branch.to_owned())),
            diff: seed.diff.map(
                |(files_changed, lines_added, lines_removed)| git::DiffStats {
                    files_changed,
                    lines_added,
                    lines_removed,
                },
            ),
        };

        workspace.update_session(*id, ctx, |session| {
            session.derived_title = Some(seed.title.to_owned());
            session.status = seed.status;
            session.working_directory = Some(directory.clone());
        });
        workspace
            .git()
            .update(ctx, |model, ctx| model.record(directory, facts, ctx));
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
        settings: Settings,
        channel: Channel,
        text_layout: Arc<dyn TextLayoutSystem>,
        frame_budget: Option<u32>,
        overrides: Overrides,
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

        let (window_id, workspace) =
            app.add_window(|ctx| Workspace::new(fonts, settings, channel, quit, ctx));
        app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                // Before the polls, not after: `start_git_poll` decides
                // whether to pay for `git diff` from the density it finds, and
                // a density the command line asked for has to be in place by
                // then or the first cycle gathers the wrong half.
                apply_overrides(workspace, overrides, ctx);
                workspace.start_usage_poll(ctx);
                workspace.start_git_poll(ctx);
            });
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
            Startup::Window {
                frames: None,
                overrides: Overrides::default()
            }
        );
    }

    #[test]
    fn frames_bounds_the_run() {
        assert_eq!(
            parse(&["--frames", "3"]).expect("a count is valid"),
            Startup::Window {
                frames: Some(3),
                overrides: Overrides::default()
            }
        );
    }

    #[test]
    fn snapshot_takes_a_path_and_wins_over_a_frame_budget() {
        assert_eq!(
            parse(&["--frames", "3", "--snapshot", "/tmp/frame.png"])
                .expect("both are valid together"),
            Startup::Snapshot {
                path: PathBuf::from("/tmp/frame.png"),
                overrides: Overrides::default()
            }
        );
    }

    #[test]
    fn the_startup_overrides_reach_both_kinds_of_run() {
        assert_eq!(
            parse(&[
                "--menu",
                "--hover",
                "--layout",
                "horizontal",
                "--granularity",
                "tabs",
                "--density",
                "expanded"
            ])
            .expect("valid"),
            Startup::Window {
                frames: None,
                overrides: Overrides {
                    menu: true,
                    hover: true,
                    settings: None,
                    layout: Some(Layout::Horizontal),
                    granularity: Some(Granularity::Tabs),
                    density: Some(Density::Expanded)
                }
            }
        );
        assert_eq!(
            parse(&["--snapshot", "/tmp/frame.png", "--menu"]).expect("valid"),
            Startup::Snapshot {
                path: PathBuf::from("/tmp/frame.png"),
                overrides: Overrides {
                    menu: true,
                    ..Overrides::default()
                }
            }
        );
        // `--settings` takes an optional page, so it has to be right about
        // both halves: a page it recognises is consumed, and the next flag is
        // left for the loop rather than eaten as a page name.
        assert_eq!(
            parse(&["--settings", "about", "--hover"]).expect("valid"),
            Startup::Window {
                frames: None,
                overrides: Overrides {
                    hover: true,
                    settings: Some(Section::About),
                    ..Overrides::default()
                }
            }
        );
        assert_eq!(
            parse(&["--settings", "--menu"]).expect("valid"),
            Startup::Window {
                frames: None,
                overrides: Overrides {
                    menu: true,
                    settings: Some(Section::Appearance),
                    ..Overrides::default()
                }
            }
        );
        assert!(parse(&["--settings", "keybindings"]).is_err());

        assert!(parse(&["--density", "cosy"]).is_err());
        assert!(parse(&["--density"]).is_err());
        assert!(parse(&["--layout", "diagonal"]).is_err());
        assert!(parse(&["--layout"]).is_err());
        assert!(parse(&["--granularity", "sessions"]).is_err());
        assert!(parse(&["--granularity"]).is_err());
    }

    #[test]
    fn every_option_and_key_the_help_names_is_one_the_parser_or_a_binding_knows() {
        // A flag documented and never parsed, or parsed and never documented,
        // is the kind of drift nobody notices until somebody types it.
        let help = help_text();

        for flag in [
            "--snapshot",
            "--frames",
            "--menu",
            "--hover",
            "--layout",
            "--granularity",
            "--density",
        ] {
            assert!(help.contains(flag), "{flag} is not in --help");
            // Either it parses, or it complains about the value it is missing.
            // What it must never do is call itself unrecognised.
            let complaint = parse(&[flag]).err().map(|error| error.to_string());
            assert!(
                !complaint.is_some_and(|complaint| complaint.contains("unrecognised")),
                "--help documents {flag}, which the parser has never heard of"
            );
        }

        // The layout binding is the one KEYS entry that is not a tab action,
        // so it is the one that can quietly stop being dispatched.
        assert!(help.contains("cmd/ctrl-b"));
    }

    /// Whether a pool of `workers` can still run a task once
    /// [`PARKED_WORKERS`] of them are parked on a timer.
    ///
    /// The parked tasks report that they are *running* before they park, so
    /// the answer never depends on how quickly the pool picked them up.
    fn a_worker_is_left_over(workers: usize, patience: std::time::Duration) -> bool {
        use std::sync::mpsc;

        let pool = Background::new(workers);
        let mut releases = Vec::new();

        for _ in 0..PARKED_WORKERS {
            let (release, parked) = mpsc::channel::<()>();
            let (started, running) = mpsc::channel();
            releases.push(release);
            pool.spawn(async move {
                let _ = started.send(());
                // Stands in for a poll chain's `recv_timeout`: the worker is
                // held for the whole wait rather than handed back.
                let _ = parked.recv();
            })
            .detach();
            running
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("a parked task never reached a worker");
        }

        let (done, finished) = mpsc::channel();
        pool.spawn(async move {
            let _ = done.send(());
        })
        .detach();

        let ran = finished.recv_timeout(patience).is_ok();
        // Closing the channels lets the parked tasks return, so dropping the
        // pool joins its workers instead of hanging on them.
        drop(releases);
        ran
    }

    #[test]
    fn the_pool_keeps_a_worker_free_of_the_chains_that_park_on_timers() {
        // The whole of why `background_pool`'s floor is not the core count.
        // On a two-core machine the old `max(2)` gave the two poll chains the
        // entire pool, and the settings save a click had just asked for sat in
        // the queue behind a fifteen-second sleep.
        assert!(
            !a_worker_is_left_over(PARKED_WORKERS, std::time::Duration::from_millis(500)),
            "a pool the size of the poll chains had a worker to spare, so              this test is no longer describing the machinery it names"
        );
        assert!(
            a_worker_is_left_over(PARKED_WORKERS + 1, std::time::Duration::from_secs(30)),
            "one worker per parked chain plus one was not enough to run a              settings save"
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
