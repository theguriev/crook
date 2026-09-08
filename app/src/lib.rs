//! Crook: a terminal whose unit of work is an agent.
//!
//! This is the application. Everything below it is general — [`crookui_core`]
//! is a UI framework, [`crookui`] is a renderer and a window, [`crook_wasm`]
//! is a sandbox for one guest — and everything here is Crook: a strip of
//! agent sessions, a header a plugin pins things to, and the wiring that turns
//! one into pixels and the other into a place to put them.
//!
//! # What a run looks like
//!
//! 1. Resolve the fonts, because a family id is needed before any view exists.
//! 2. Open the event loop, which hands back the main-thread executor.
//! 3. Build the [`App`], add the window with a [`Workspace`] root, and start
//!    the poll chains.
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
//!
//! `--run <command>` is the other half of running unattended, and it exists
//! because a frame budget alone cannot show a terminal working: `--frames 3`
//! draws three frames in the time it takes a shell to open a pty, so all three
//! are empty. With `--run`, the command is typed into the first pane's input
//! field a keystroke at a time and sent with Return, and the run waits for it to
//! print before it counts a frame or takes a picture — and says on stdout, or in
//! the log, what the shell actually wrote. That is the whole path a person's
//! command takes, from a key press to a line of output, in a run nobody is
//! sitting in front of.
//!
//! `--type <text>` is its quieter companion: it leaves text in the field
//! *unsent*, which together with `--select <text>` is how a picture can show a
//! line in the middle of being written. `--select-output <text>` is the same
//! trick above the field: it drags a highlight across what the shell printed,
//! which is the state a person is in the instant before they press copy and one
//! nobody can hold a button down for in a headless run.

pub mod browser;
pub mod clipboard;
pub mod completion;
pub mod editor;
pub mod git;
pub mod git_model;
pub mod input_keys;
pub mod keybindings;
pub mod pane_blocks;
pub mod pane_link;
pub mod pane_selection;
pub mod pane_split;
pub mod pane_surface;
pub mod platform_insets;
pub mod plugin;
pub mod plugins;
pub mod process;
pub mod selection;
pub mod session;
pub mod settings;
pub mod shell_history;
pub mod shell_integration;
pub mod tab;
pub mod terminal_font;
pub mod terminal_keys;
pub mod terminal_model;
pub mod text_input;
pub mod theme;
pub mod window_controls;
pub mod workspace;

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crook_plugin::ActionName;
use crookui::{
    CosmicFontDb, Platform, Proxy, WindowControls as PlatformWindow, WindowDelegate, WindowOptions,
    render_scene_to_rgba,
};
use crookui_core::event::{Event, Keystroke, Modifiers, MouseButton};
use crookui_core::executor::{Background, LocalQueue};
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform::TextLayoutSystem;
use crookui_core::prelude::*;
use crookui_core::scene::Scene;
use crookui_core::{App, Presenter, WindowId};

use crate::platform_insets::{ControlLayout, WindowChrome};
use crate::settings::{Density, Granularity, Settings};
use crate::tab::{AgentStatus, Direction, PaneId, Tab, TabAction};
use crate::terminal_font::{CELL_FONT_SIZE, CellFont};
use crate::window_controls::{Detached, WindowHandle, WindowState};
use crate::workspace::{Fonts, Opening, QuitRequest, Workspace};

/// The window Crook opens, in logical pixels.
const WINDOW_SIZE: Vector2F = vec2f(1024., 640.);

/// Who draws Crook's window controls.
///
/// Crook does. The window is opened with the application's own chrome, so the
/// header *is* the title bar and the window's controls are painted over it:
/// AppKit's traffic lights on macOS, Crook's own three buttons everywhere
/// else. [`WindowChrome`](crookui::WindowChrome) is where the difference
/// between those two lives.
///
/// One constant because it is one decision. [`open_window`] opens the window
/// with it and the header reserves room by it, so what the header leaves free
/// and what the window actually draws cannot drift apart.
pub const WINDOW_CHROME: WindowChrome = WindowChrome::Client;

/// The scale factor the headless snapshot renders at. Two, because that is
/// where subpixel glyph positioning and the atlas are actually exercised.
const SNAPSHOT_SCALE_FACTOR: f32 = 2.;

/// The longest a `--run` waits for its command to finish printing.
///
/// A wall clock rather than a signal, because there is no such thing as "the
/// command has finished" from outside the pty: a shell prints its prompt back
/// and goes quiet, and quiet is the only evidence there is.
const RUN_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the grid has to stop changing before a `--run` calls it finished.
const RUN_QUIET: Duration = Duration::from_millis(400);

/// How far `--carry` carries a row when the command line did not say.
///
/// Three ordinary rows in the default density, which is far enough to be
/// unmistakably a drag and short enough to leave the row it came out of on
/// screen beside it.
const CARRY_DISTANCE: f32 = 120.;

/// In how many moves, because the list moves between them.
const CARRY_STEPS: u32 = 12;

/// How long the headless `--run` sleeps between pumps while it waits.
const RUN_POLL: Duration = Duration::from_millis(10);

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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Overrides {
    /// Start with the tab options menu open.
    menu: bool,
    /// Start with the first row's hover detail card up.
    hover: bool,
    /// Run these named actions before the picture is taken, in order.
    ///
    /// A way to look at a frame, like `--menu` and `--themes`, and the only
    /// one that reaches a *plugin's* surface: what a plugin puts up is put up
    /// by one of its own actions, and a picture of a panel nobody can open
    /// from the command line is a picture nobody can take. Run last, after the
    /// shells have settled, because what a plugin draws usually depends on
    /// what the pane has told it.
    actions: Vec<String>,
    /// Load the plugins this machine has installed, as a real run does.
    ///
    /// Off by default and opt-in for one reason: a snapshot is a picture of
    /// *the application*, and one that quietly included whatever a person had
    /// installed would be a different picture on every machine and in CI. It
    /// is on the list all the same, because the only way to look at what a
    /// plugin draws is to draw it — and a tier whose one worked example can
    /// only be seen by launching a window is a tier nobody can screenshot.
    with_plugins: bool,
    /// A file of fixed plugin surfaces to draw, for a picture of the plugin
    /// tier that is the same on every machine.
    ///
    /// The other half of the answer `with_plugins` is: that one draws whatever
    /// somebody happens to have installed, which is what makes it useless in
    /// CI, and this one draws a tree written down in the repository. See
    /// [`plugins::wasm::fixture`](crate::plugins::wasm::fixture).
    fixture: Option<PathBuf>,
    /// A module to run without installing it, and to run again every time it
    /// is built.
    ///
    /// The plugin somebody is *writing*, which is a different thing from one
    /// they chose to have: nothing is copied anywhere, and when the window
    /// closes the machine is as it was.
    dev_plugin: Option<PathBuf>,
    /// Start with the Themes panel open, and — with `creating` — on its
    /// creator.
    ///
    /// A way to look at a frame, like `--menu` and `--hover`: the panel is a
    /// surface, and a snapshot of it is a snapshot of the real thing.
    themes: bool,
    /// Start with the Themes panel making a theme.
    creating: bool,
    /// Start with the active tab's own context menu open.
    ///
    /// A way to look at a frame, like `--menu`: the surface a secondary press
    /// on a row opens, with whatever its plugins put in it.
    tab_menu: bool,
    /// Start with the active tab's worktree menu open, and its creator with
    /// it when `creating_worktree`.
    ///
    /// Implies `tab_menu`, because the worktree list is drawn inside the menu
    /// it is an entry of — asking for the submenu and not the menu is not a
    /// frame this window can draw.
    worktrees: bool,
    /// Start with that menu making a worktree.
    creating_worktree: bool,
    /// Start in this theme rather than the saved one.
    ///
    /// Applied straight to the palette rather than through
    /// `Workspace::set_theme`, which would save it: `--theme` is a way to look
    /// at a frame — a screenshot of the light theme, a check that a file
    /// somebody wrote loads — and it must not become what the next launch
    /// opens in. The settings page goes on showing the *saved* theme as the
    /// chosen one, which is the same thing `--density` does and for the same
    /// reason.
    theme: Option<String>,
    /// Start with a settings tab open, on this page of it.
    ///
    /// Unlike the three option overrides below it, this one has nothing to
    /// keep out of the settings file: which page of the settings somebody is
    /// looking at is not an option and is never written down. It does put an
    /// extra tab in the strip, which is the point — a snapshot of the
    /// settings page is a snapshot of a window with the settings open in it.
    /// Start with this section of the sidebar showing, by the name on its
    /// button.
    section: Option<String>,
    settings: Option<Option<String>>,
    /// Start with the Keyboard Shortcuts page recording a chord for this
    /// command, by its action name.
    ///
    /// A way to look at a frame, like `--menu` and `--themes`: the recorder is
    /// a state of a row that a person is in for two seconds, and a snapshot of
    /// it is a snapshot of the real thing. It records nothing on its own — the
    /// keys still have to be pressed — so a run that takes a picture and exits
    /// writes no keybindings file.
    record: Option<String>,
    /// Type this into the settings page's search box at startup.
    ///
    /// Implies `--settings`: a query with no page to filter is nothing to
    /// look at. Like `--type` for a pane's field, it leaves the text in the
    /// box rather than doing anything with it, because leaving it there *is*
    /// what the box does — the page is filtered on every keystroke.
    search: Option<String>,
    /// Type this into the tabs panel's search box at startup.
    ///
    /// The panel's box rather than the settings page's, which is why it is not
    /// the same flag: they filter two different lists and are on screen at two
    /// different times. Like `--search` it leaves the text in the box, because
    /// leaving it there *is* what the box does — the list is filtered on every
    /// keystroke — and it takes the keyboard, so the picture is of a box being
    /// typed into rather than of one that happens to have words in it.
    find: Option<String>,
    /// Draw another platform's window controls rather than this one's.
    ///
    /// The only override here that changes nothing a person can set. It exists
    /// because the window's chrome is invisible on whichever machine Crook is
    /// being written on: macOS reserves the corner its traffic lights are
    /// painted into and the other two platforms reserve nothing, and laying
    /// the header out either way needs no second machine — only a way to ask
    /// for it.
    controls: Option<ControlLayout>,
    /// Start with rows standing for this rather than for the saved one.
    granularity: Option<Granularity>,
    /// Start in this density rather than the saved one.
    density: Option<Density>,
    /// Type these into the first pane's shell at startup, in order, waiting
    /// for what each one prints. See the module docs for why a frame budget
    /// alone is not enough.
    ///
    /// Repeatable, because one command is one block and a picture of a *list*
    /// of blocks needs several.
    run: Vec<String>,
    /// Hover the finished block at this index, so its copy control is drawn.
    ///
    /// A hover is a state that only exists while a pointer is over something,
    /// which is the other thing no unattended run can hold still.
    hover_block: Option<usize>,
    /// Open the menu on the finished block at this index.
    ///
    /// A menu is the fourth of these states, and it needs one thing the other
    /// three do not: a frame of its own before it opens. The dots the menu
    /// hangs off are painted by the list rather than built as an element, so
    /// where they are is something only a paint pass knows — see
    /// [`block_menu::anchor`](crate::workspace::block_menu). The block is
    /// hovered, a frame is drawn, and the menu opens onto the corner that
    /// frame recorded.
    block_menu: Option<usize>,
    /// Pick this row of the tabs panel up and carry it, without letting go.
    ///
    /// The third state no unattended run can hold still, after a hovered row
    /// and a selection dragged through a shell's output: a drag lasts exactly
    /// as long as a button is held down. The frame is drawn mid-gesture — the
    /// row in the air under the pointer, the hole it came out of, and the list
    /// already in the order it is going to be in, because the panel reorders
    /// itself while the hand is still moving rather than when it lets go.
    carry: Option<usize>,
    /// The same for a whole group's block, which is gripped by its heading.
    carry_group: Option<usize>,
    /// How far down the column to carry it, in whole pixels. Negative carries
    /// it up.
    carry_by: Option<i32>,
    /// Scroll the first pane's block list up by this many lines.
    ///
    /// What puts output under the composer, which is the only thing that draws
    /// the rule above it.
    scroll_blocks: Option<i32>,
    /// Put this in the first pane's input field, and leave it there unsent.
    ///
    /// `--run`'s companion: that one shows what a shell printed, and this one
    /// shows the line somebody is in the middle of composing. Together they are
    /// the only way a picture can hold both halves of a pane at once.
    type_text: Option<String>,
    /// Select the first occurrence of this in the input field.
    ///
    /// A selection is a state that only exists while a button is held, so it is
    /// the one thing about the field that no unattended run could otherwise
    /// show.
    select: Option<String>,
    /// Select the first occurrence of this in the first pane's *output*.
    ///
    /// `--select`'s companion, above the field rather than in it: what a person
    /// drags a pointer across the shell's output to take, which is the other
    /// state no unattended run can hold a button down for.
    select_output: Option<String>,
    /// Carry that selection on to the first occurrence of this.
    ///
    /// Two markers rather than one string, because the region worth a picture
    /// is the one that crosses a block boundary — and naming that in one
    /// string would mean spelling out the prompt between them, which is
    /// whatever `PS1` was on the machine taking the picture.
    select_through: Option<String>,
}

impl Overrides {
    /// Whether this run was asked to pick something up.
    fn carries(&self) -> bool {
        self.carry.is_some() || self.carry_group.is_some()
    }

    /// Whether this run needs shells opened for it.
    ///
    /// Neither the field nor the grid is worth a picture without one: a pane
    /// with no shell draws a notice instead of both.
    fn wants_shells(&self) -> bool {
        !self.run.is_empty()
            || self.type_text.is_some()
            || self.select_output.is_some()
            || self.hover_block.is_some()
            || self.block_menu.is_some()
            || self.scroll_blocks.is_some()
    }
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
            "--shell-integration" => {
                let shell = args.next();
                println!("{}", shell_integration_text(shell.as_deref())?);
                return Ok(Startup::Answered);
            }
            // Answered like `--version` rather than started like `--theme`: a
            // person installing a plugin is not opening a window, and doing
            // both would be a window that opened before the plugin it was
            // asked to carry was in place.
            "--install-plugin" => {
                let path = args
                    .next()
                    .context("`--install-plugin` needs a path to a plugin.wasm")?;
                // The path and then the reason, with nothing between them: the
                // reason already says what is wrong with it, and a line that
                // said "not a plugin this build can install: not a plugin this
                // build can run" would be saying it twice.
                let installed = crate::plugins::wasm::install(std::path::Path::new(&path))
                    .map_err(anyhow::Error::msg)
                    .with_context(|| path.clone())?;
                println!("installed {}", installed.display());
                return Ok(Startup::Answered);
            }
            // The other half of `--install-plugin`, and answered the same way.
            // What it takes is the plugin's name rather than a path, because
            // by the time somebody wants a plugin gone the file they installed
            // it from is a download they deleted a month ago.
            "--uninstall-plugin" => {
                let name = args
                    .next()
                    .context("`--uninstall-plugin` needs a plugin's `owner/name`")?;
                let id = crook_plugin::PluginId::parse(&name)
                    .map_err(anyhow::Error::msg)
                    .with_context(|| name.clone())?;
                let removed = crate::plugins::wasm::uninstall(&id)
                    .map_err(anyhow::Error::msg)
                    .with_context(|| name.clone())?;
                forget_plugin(&id)?;
                println!("removed {}", removed.display());
                return Ok(Startup::Answered);
            }
            // Not a plugin and not a setting: a fixture is a *picture*, and
            // it lives for the run that takes one. See
            // `plugins::wasm::fixture`.
            // Not installed and not remembered: the module runs from wherever
            // it was built, for as long as this window is open. See
            // `plugins::wasm::dev`.
            "--dev-plugin" => {
                let path = args
                    .next()
                    .context("`--dev-plugin` needs a module, or the directory it is built in")?;
                overrides.dev_plugin = Some(PathBuf::from(path));
            }
            "--plugin-fixture" => {
                let path = args
                    .next()
                    .context("`--plugin-fixture` needs a path to a fixture")?;
                overrides.fixture = Some(PathBuf::from(path));
            }
            "--plugins" => {
                println!("{}", installed_plugins_text());
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
            "--tab-menu" => overrides.tab_menu = true,
            "--themes" => overrides.themes = true,
            "--worktrees" => overrides.worktrees = true,
            "--new-worktree" => {
                overrides.worktrees = true;
                overrides.creating_worktree = true;
            }
            "--new-theme" => {
                overrides.themes = true;
                overrides.creating = true;
            }
            "--theme" => {
                let name = args.next().context("`--theme` needs a name")?;
                overrides.theme = Some(name);
            }
            "--search" => {
                let query = args
                    .next()
                    .context("`--search` needs something to search for")?;
                overrides.search = Some(query);
                if overrides.settings.is_none() {
                    overrides.settings = Some(None);
                }
            }
            "--find" => {
                let query = args
                    .next()
                    .context("`--find` needs something to search for")?;
                overrides.find = Some(query);
            }
            "--settings" => {
                // The section is optional, and a bare `--settings` opens the
                // page where a click on the menu entry opens it. Peeking
                // rather than consuming is what lets `--settings --hover`
                // mean what it looks like it means.
                //
                // The name is a page's *title*, matched when the window opens
                // rather than here: the pages come from plugins, so which of
                // them exist is not known until they have built, and a build
                // with an extra plugin should accept that plugin's page here
                // without this list growing a line.
                let page = match args.peek().map(String::as_str) {
                    Some(other) if !other.starts_with("--") => Some(other.to_owned()),
                    _ => None,
                };
                if page.is_some() {
                    args.next();
                }
                overrides.settings = Some(page);
            }
            "--record" => {
                let command = args
                    .next()
                    .context("`--record` needs the name of a command")?;
                overrides.record = Some(command);
                // The recorder is a row on that page, so the page has to be
                // showing for there to be a row: `--record` implies
                // `--settings "Keyboard Shortcuts"` rather than making a
                // person write both.
                overrides
                    .settings
                    .get_or_insert(Some(crate::plugins::shortcuts::PAGE_TITLE.to_owned()));
            }
            "--section" => {
                let name = args
                    .next()
                    .context("`--section` needs the name on a section's button")?;
                overrides.section = Some(name);
            }
            "--hover" => overrides.hover = true,
            "--with-plugins" => overrides.with_plugins = true,
            "--action" => {
                let name = args.next().context("`--action` needs a command name")?;
                overrides.actions.push(name);
            }
            "--run" => {
                let command = args.next().context("`--run` needs a command")?;
                overrides.run.push(command);
            }
            "--carry" => {
                let index = args.next().context("`--carry` needs a row")?;
                overrides.carry = Some(index.parse().context("`--carry` takes a number")?);
            }
            "--carry-group" => {
                let index = args.next().context("`--carry-group` needs a group")?;
                overrides.carry_group =
                    Some(index.parse().context("`--carry-group` takes a number")?);
            }
            "--carry-by" => {
                let pixels = args.next().context("`--carry-by` needs a distance")?;
                overrides.carry_by = Some(pixels.parse().context("`--carry-by` takes a number")?);
            }
            "--hover-block" => {
                let index = args.next().context("`--hover-block` needs an index")?;
                overrides.hover_block =
                    Some(index.parse().context("`--hover-block` takes a number")?);
            }
            "--block-menu" => {
                let index = args.next().context("`--block-menu` needs an index")?;
                overrides.block_menu =
                    Some(index.parse().context("`--block-menu` takes a number")?);
            }
            "--scroll-blocks" => {
                let lines = args.next().context("`--scroll-blocks` needs a count")?;
                overrides.scroll_blocks =
                    Some(lines.parse().context("`--scroll-blocks` takes a number")?);
            }
            "--type" => {
                let text = args.next().context("`--type` needs some text")?;
                overrides.type_text = Some(text);
            }
            "--select" => {
                let text = args.next().context("`--select` needs some text")?;
                overrides.select = Some(text);
            }
            "--select-output" => {
                let text = args.next().context("`--select-output` needs some text")?;
                overrides.select_output = Some(text);
            }
            "--select-through" => {
                let text = args.next().context("`--select-through` needs some text")?;
                overrides.select_through = Some(text);
            }
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
            "--controls" => {
                let platform = args.next().context("`--controls` needs a platform")?;
                overrides.controls = Some(match platform.as_str() {
                    "macos" => ControlLayout::MacOs,
                    "windows" => ControlLayout::Windows,
                    "linux" => ControlLayout::Freedesktop,
                    other => bail!("`--controls` takes macos, windows or linux, not {other}"),
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

/// The integration snippet for a shell, with the line that says where to put
/// it, for a machine Crook cannot start the shell on itself.
///
/// The whole scheme in `shell_integration` only reaches shells Crook spawns. A
/// shell on the far side of `ssh`, inside a container or in a `docker exec` was
/// never spawned by Crook and cannot be reached into, and this is the honest
/// answer to that: the same text Crook would have installed, for a person to
/// paste into their own configuration on that machine. Without it the snippets
/// exist only inside the binary, and the module's own documented answer to
/// "what about ssh?" is one nobody can act on.
fn shell_integration_text(shell: Option<&str>) -> Result<String> {
    let named = shell.context(
        "`--shell-integration` needs a shell: zsh, bash or fish.          Paste what it prints at the end of that shell's own configuration on a          machine Crook cannot start the shell on itself — over ssh, in a container.",
    )?;
    let shell = shell_integration::Shell::of(std::path::Path::new(named));
    let (snippet, file) = shell_integration::snippet(shell)
        .zip(shell_integration::manual_install_file(shell))
        .with_context(|| {
            format!("Crook has no shell integration for {named}; it has one for zsh, bash and fish")
        })?;
    Ok(format!(
        "# Crook shell integration for {named}. Append this to {file} on the \
machine you want blocks on.\n\n{snippet}"
    ))
}

fn help_text() -> String {
    format!(
        "crook {version} \u{2014} a terminal whose unit of work is an agent

USAGE:
    crook [OPTIONS]

OPTIONS:
    --install-plugin <PATH>
                       Copy a plugin's `.wasm` into the plugins directory and exit,
                       after checking it is one. What it may then do is nothing
                       until it is allowed it on the Plugins page
    --uninstall-plugin <ID>
                       Remove an installed plugin by `owner/name`, with whatever
                       it was allowed to do, and exit
    --plugins          List the plugins installed as files: name, version and
                       which file each is running from
    --dev-plugin <PATH>
                       Run the plugin you are writing, from wherever you built
                       it, and run it again every time you build it. Takes a
                       `.wasm` or the directory holding one. Nothing is
                       installed and nothing is left behind
    --plugin-fixture <PATH>
                       Draw a fixed plugin surface, read from a JSON file of
                       slot names to the shapes a plugin describes, so a
                       picture of what a plugin puts on screen is the same on
                       every machine
    --snapshot <PATH>  Render one frame of the real view tree to a PNG and exit
    --frames <N>       Draw N frames, then exit; for running unattended
    --run <COMMAND>    Type COMMAND into the first pane's input field at startup,
                       send it, and report what the shell printed. Repeatable:
                       one command is one block
    --hover-block <N>  Hover the Nth finished block, so its controls are drawn
    --block-menu <N>   Open the menu on the Nth finished block
    --scroll-blocks <N>
                       Scroll the first pane's block list up by N lines
    --type <TEXT>      Leave TEXT in the first pane's input field, unsent
    --select <TEXT>    Select the first occurrence of TEXT in that field
    --select-output <TEXT>
                       Select the first occurrence of TEXT in that pane's output
    --select-through <TEXT>
                       Carry that selection on to TEXT, which may be in a later
                       block: what a drag across several commands takes
    --menu             Start with the tab options menu open
    --tab-menu         Start with the active tab's own context menu open
    --settings [PAGE]  Start with a settings tab open, on `appearance`,
                       `shell`, `keys` or `about`
    --find <TEXT>      Type TEXT into the tabs panel\'s search box, filtering the list
    --search <TEXT>    Type TEXT into the settings page\'s search box, opening it
    --record <COMMAND> Start with the Keyboard Shortcuts page recording a chord for
                       COMMAND, which is an action name like `crook/window/new-tab`
    --theme <NAME>     Start in this theme rather than the saved one
    --worktrees        Start with the active tab's worktree menu open
    --new-worktree     Start with that menu making a worktree
    --themes           Start with the Themes panel open
    --new-theme        Start with the Themes panel making a theme
    --hover            Start with the first row's detail card up
    --action <name [argument]>
                       Run this named action before the picture is taken, so a
                       plugin's own panel can be looked at. Anything after the
                       name is what the action is told — what a picker's row or
                       a menu's entry would have said. Repeatable
    --with-plugins     Load the plugins this machine has installed, so that a
                       snapshot shows what they draw. Off by default: a picture
                       of the application is the same everywhere and one of a
                       plugin is not
    --carry <N>        Pick the Nth row of the tabs panel up and hold it there,
                       for a picture of a drag in flight
    --carry-group <N>  The same for the Nth group's whole block, by its heading
    --carry-by <PX>    How far down the column to carry it; negative carries up
    --section <NAME>   Start showing a sidebar section by the name on its button
    --granularity <M>  Start with rows standing for `panes` or `tabs` rather than as saved
    --density <MODE>   Start in `compact` or `expanded` density rather than the saved one
    --controls <OS>    Draw `macos`, `windows` or `linux` window controls in the
                       header rather than this platform's, for a picture of the
                       title bar the other two get
    --shell-integration <SHELL>
                       Print the OSC 133 snippet for `zsh`, `bash` or `fish`,
                       to paste into that shell\'s own configuration on a machine
                       Crook cannot start the shell on — over ssh, in a container
    -h, --help         Print this message
    -V, --version      Print the version and channel

KEYS (macOS):
    cmd+t                      New agent tab
    cmd+,                      Show the settings
    cmd+d / shift+cmd+d        Split the focused pane to the right / downwards
    cmd+w                      Close the focused pane, and its tab with the last one
    cmd+k                      Search the tabs
    alt+cmd+left/right         Select the previous/next tab
    ctrl+cmd+left/right        Move the active tab
    cmd+plus / cmd+minus       Make the terminal's text bigger / smaller
    cmd+0                      Put the text back to its default size

KEYS (Linux and Windows):
    ctrl+shift+t               New agent tab
    ctrl+,                     Show the settings
    ctrl+shift+d / ctrl+shift+e  Split the focused pane to the right / downwards
    ctrl+shift+w               Close the focused pane, and its tab with the last one
    ctrl+shift+k               Search the tabs
    ctrl+pageup/pagedown       Select the previous/next tab
    ctrl+shift+pageup/pagedown Move the active tab
    ctrl+plus / ctrl+minus     Make the terminal's text bigger / smaller
    ctrl+0                     Put the text back to its default size

    Control-Shift, because a bare ctrl-letter belongs to the program in the
    pane: ctrl-c interrupts it, ctrl-d ends its input and ctrl-w takes back a
    word. The comma is not a letter the tty wants, which is why the settings
    chord is the one entry here that keeps a bare Control.

    Every one of these is a *default*. They are keybindings in VSCode's format
    and with VSCode's rules, so <config>/crook/keybindings.json overrides any
    of them:

        [{{ \"key\": \"ctrl+shift+j\", \"command\": \"crook/window/new-tab\" }}]

    The last rule that matches a chord wins, a `-` in front of a command takes
    it off that chord, a \"when\" clause limits a rule to a condition, and a
    key may be a sequence like \"ctrl+k ctrl+s\". The Keyboard Shortcuts page
    in the settings lists every command by name, with the chord that reaches
    it.

THE OUTPUT:
    A pane's output is a list of commands. Each block holds its prompt, the
    line that was run and everything it printed; a hairline separates one from
    the next, a red wash marks one that failed, an accent stripe one that is
    still running, and hovering any of them reveals a control that copies
    exactly that command and its output — no neighbour's text and no trailing
    blank rows.

    Drag across the output to select it: a double click takes a word, a triple
    click a line, and alt-drag a column. The drag keeps going when the pointer
    leaves the pane, a drag past the top or bottom edge scrolls the screen
    under the pointer, and the selection stays on its own text while the shell
    prints more underneath. cmd-c — ctrl-c or ctrl-shift-c off macOS — copies
    it and lets it go, which is what keeps ctrl-c the interrupt it has always
    been the moment there is nothing selected. The half-written command line in
    the field below is left alone: a copy is not an interrupt. Typing, or
    clicking anywhere else in the pane, lets the selection go too.

    A selection lives in the block that is still running, and nowhere else: a
    finished command's rows have been copied out of the emulator, and a
    selection has to live there to stay anchored to its text while output
    arrives. Copying a finished block needs no selection — that is what its
    hover control is for.

SHELL INTEGRATION:
    Blocks need the shell to say where a command starts and ends, and Crook
    installs the four OSC 133 marks into zsh, bash and fish by itself. There is
    nothing to install and nothing to configure: a scratch rc stub is written
    before the shell starts, chains onto whatever hooks are already there, and
    is removed when the pane closes; no dotfile is ever written to. Set
    CROOK_NO_SHELL_INTEGRATION to anything but 0 to turn it off.

    It reaches only shells Crook starts — not the far side of an ssh, not a
    container, not pwsh, nu, ksh or tcsh. Without it a pane is a plain
    terminal: one continuous stream drawn as a grid, scrolled through the
    emulator's own scrollback, with every key going straight to the shell and
    no field under it. Everything else, selection and copying included, is
    unchanged.

THE INPUT FIELD:
    Each pane composes its next command in the field under its output, at the
    same column zero, in the terminal's own font and colours: no box, no
    border, no focus ring. Enter sends the line, shift-enter lengthens it, and
    the up and down arrows walk that pane's history. Everything else is the
    text editing this platform already does. ctrl-c interrupts the shell and
    throws the half-written line away with it, ctrl-z suspends, ctrl-d ends the
    input when the field is empty and deletes a character when it is not.

    The field goes away, and every key reaches the program instead, while a
    full-screen program is running, once the shell reports a command that has
    been running for longer than a blink, and whenever the output is drawn as a
    plain grid. The settings tab is the one pane that never has one: it has no
    shell, and every control on it is a click.",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// How many workers are parked on a timer at any moment.
///
/// Five: a sandboxed plugin's timer between ticks, the git gather between
/// cycles, the caret blink between halves of its phase, the one that asks the
/// shells whether they are still alive, and the themes folder being re-read
/// while the Themes panel is open. Each is one background task for the whole
/// cycle — the wait *and* the work — so each holds its worker across the wait
/// rather than yielding it, and none is ever counted as idle. All five can be
/// parked at once: a window with shells in it, a chip polling in the header
/// and the Themes panel open is an ordinary afternoon. Raise this when a sixth
/// such chain appears.
///
/// The plugin chain is counted once and not per plugin, which is the one
/// approximation here. Each sandboxed plugin gets a runtime of its own and
/// each runtime parks its own worker between ticks — see
/// [`plugins::wasm`](crate::plugins::wasm) — so a person with three installed
/// has three of these chains rather than one. One is what a machine with the
/// chip installed actually has, and the spare below is what keeps the second
/// from costing anybody a save; a fleet of polling plugins would want this
/// number raised, and there is nothing here that can count them at startup
/// because a plugin is installed by dropping a file in a directory.
///
/// One more chain exists and is deliberately not counted: the watch behind
/// `--dev-plugin`, which parks a worker between looks exactly as the others
/// do. It is not counted because it is not there — the flag has to be typed,
/// and a person typing it is a person building a plugin on a machine they are
/// paying attention to. If it were ever anything but a development flag, this
/// number would be six.
///
/// The store is deliberately not on that list, and it is worth saying why
/// since it is the newest thing that reaches the network. It parks nothing: a
/// look at the registry and a download are one task each, started by somebody
/// pressing something and ended by an answer, and both are bounded by the
/// twenty-second timeout the request carries. What that costs a save queued
/// behind it is that timeout at the very worst, which is the same argument the
/// long-running timer below makes with a smaller number — not a worker held
/// for as long as a window is open.
///
/// The number is a count of *chains*, never of panes. That is why the child
/// check is one task for the whole terminal model rather than one per session:
/// a chain per pane would park a worker per pane, and a window with more panes
/// than the machine has cores would have nothing left to run a save on.
///
/// One thing does park per pane and is deliberately not counted here:
/// `TerminalModel::schedule_long_running` arms a timer per running command, to
/// draw the frame in which that command takes the pane. It is bounded by
/// `pane_surface::LONG_RUNNING` — fifty milliseconds — rather than by a poll
/// interval, so what it can cost a save queued behind it is a fiftieth of a
/// second rather than the fifteen a plugin's poll could. Sizing the pool for a
/// pane count is not possible; keeping the wait short is.
///
/// **The test at the bottom of this file cannot check this number.** It builds
/// its scenario out of the constant itself, so it proves what
/// [`background_pool`] does with whatever the number says and nothing at all
/// about how many chains there really are. Counting them is what this comment
/// is for, and the themes poll is here because it went uncounted for as long
/// as the comment was the only thing that could have counted it.
const PARKED_WORKERS: usize = 5;

/// A pool with a worker left over once every chain that parks is asleep.
///
/// The floor is not a round number, it is [`PARKED_WORKERS`] plus one. On a
/// two-core machine a pool the size of the chains has nothing left to run a
/// settings save on, and a save is the one background task a click is waiting
/// for: it would sit in the queue until one of the timers expired, up to
/// fifteen seconds, and be discarded outright if the window closed first.
fn background_pool() -> Arc<Background> {
    let cores = std::thread::available_parallelism().map_or(1, |count| count.get());
    Arc::new(Background::new(pool_size(cores)))
}

/// How many workers a machine with `cores` of them gets.
///
/// Split out from [`background_pool`] so the floor can be checked on a machine
/// that does not have it: on anything with more cores than
/// [`PARKED_WORKERS`] the `max` never fires, and a test that called
/// `background_pool` would pass on a developer's laptop with the floor deleted.
fn pool_size(cores: usize) -> usize {
    cores.max(PARKED_WORKERS + 1)
}

/// The two families the window draws in.
///
/// `monospace` is the settings file's if it names one this machine answers to,
/// and the platform's default otherwise. A name that resolves to nothing is a
/// warning and the default rather than a window that fails to open: a font can
/// be uninstalled between two launches, and that is not a reason to refuse to
/// start — it is exactly the same rule the theme name is read under.
fn resolve_fonts(font_db: &CosmicFontDb, monospace: Option<&str>) -> Result<Fonts> {
    let chosen = monospace.and_then(|name| match font_db.load_family_from_system(name) {
        Ok(family) => Some(family),
        Err(error) => {
            log::warn!("no font family called {name:?} on this machine: {error:#}");
            None
        }
    });

    Ok(Fonts {
        ui: font_db
            .default_ui_family()
            .context("no usable interface font")?,
        monospace: match chosen {
            Some(family) => family,
            None => font_db
                .default_monospace_family()
                .context("no usable monospace font")?,
        },
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
    overrides: &Overrides,
    ctx: &mut ViewContext<Workspace>,
) {
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
    if let Some(name) = overrides.section.clone() {
        // By the name on the button, because that is the only name a person
        // ever sees. One nothing answers to is a line in the log and the tabs,
        // which is the rule every other unreadable input follows.
        let section = workspace.section_named(&name);
        if section.is_none() {
            log::warn!("no sidebar section is called {name:?}");
        }
        workspace.show_section(section, ctx);
    }
    if let Some(page) = overrides.settings.clone() {
        // A name nothing answers to opens the page the menu entry opens, with
        // a line in the log — the same rule every other unreadable input
        // follows, and better than refusing to start.
        let chosen = page.and_then(|name| {
            let found = workspace.settings_page_named(&name);
            if found.is_none() {
                log::warn!("no settings page is called {name:?}");
            }
            found
        });
        workspace.open_settings_page(chosen, ctx);
    }
    // After `--settings`, which is the page the row being recorded for is on.
    if let Some(command) = &overrides.record {
        match ActionName::parse(command) {
            Ok(name) if workspace.host().action(&name).is_some() => {
                workspace.start_recording(name, ctx)
            }
            // A name nothing answers to has no row to record on, which is a
            // line in the log and a window that opens anyway — the rule every
            // other unreadable input follows.
            _ => log::warn!("no command is called {command:?}"),
        }
    }
    if let Some(query) = &overrides.search {
        workspace.type_into_settings_search(query, ctx);
    }
    // After `--section`, so that a run asking for both ends where the box is:
    // typing into it shows the tabs, whichever section was named.
    if let Some(query) = &overrides.find {
        workspace.type_into_panel_search(query, ctx);
    }
    if overrides.themes {
        workspace.open_theme_panel(overrides.creating, ctx);
    }
    if let Some(layout) = overrides.controls {
        workspace.override_control_layout(layout, ctx);
    }
    // Before `--worktrees`, which opens this menu itself and then opens the
    // list inside it: asking for both must not toggle the menu shut again.
    if overrides.tab_menu && !overrides.worktrees {
        workspace.open_tab_context_menu_for_snapshot(ctx);
    }
    if overrides.worktrees {
        workspace.open_tab_menu_for_snapshot(ctx);
        if overrides.creating_worktree {
            workspace.start_creating_worktree(ctx);
        }
    }
}

fn open_window(channel: Channel, frames: Option<u32>, overrides: Overrides) -> Result<()> {
    let launch = Launch {
        channel,
        frames,
        overrides,
    };

    // Everything fallible happens before the event loop takes over, because
    // the delegate is built inside a closure that cannot report an error.
    //
    // The dev plugin's path is resolved here for that reason and one more: a
    // `--dev-plugin` naming nothing should be a line where the flag was typed,
    // not a window that opens missing the one thing it was started for.
    if let Some(path) = launch.overrides.dev_plugin.as_deref() {
        crate::plugins::wasm::dev::Dev::module(path)
            .map_err(anyhow::Error::msg)
            .with_context(|| path.display().to_string())?;
    }
    let font_db = CosmicFontDb::new().context("no usable system fonts")?;
    // Blocking, and deliberately: one small file, read once, before there is a
    // window to stall. Before the fonts, because it names one of them.
    let settings = Settings::for_user();
    let fonts = resolve_fonts(&font_db, settings.font_family())?;
    // Resolved here, from the database, because this is the last moment
    // anything can hold it: it is moved into the event loop on the next line
    // but one, and a grid needs to ask it for a glyph on every frame after
    // that. See `CosmicGlyphs`.
    let cell_font = CellFont::new(
        font_db.glyphs(),
        fonts.monospace,
        settings.general().font_size(),
    )?;
    let text_layout: Arc<dyn TextLayoutSystem> = Arc::new(font_db.text_layout());
    apply_startup_theme(&settings, &launch.overrides);

    // Read here, before the window exists, because the window opens at the
    // size it names. An empty one — a first run, an unreadable file, or the
    // setting turned off — is the default window and one tab, which is exactly
    // what every launch did before this file existed.
    let session = if settings.general().restore_session {
        crate::session::Session::for_user()
    } else {
        crate::session::Session::default()
    };

    let options = WindowOptions {
        title: channel.window_title(),
        size: session
            .window_size()
            .map_or(WINDOW_SIZE, |[width, height]| vec2f(width, height)),
        chrome: WINDOW_CHROME,
        ..Default::default()
    };

    crookui::run(options, Box::new(font_db), move |platform| {
        Box::new(Shell::new(
            platform,
            fonts,
            cell_font.clone(),
            settings.clone(),
            text_layout.clone(),
            &launch,
            session.clone(),
        ))
    })
}

/// Puts a theme in force before there is a window to repaint.
///
/// The command line's if it named one, and the saved one otherwise. A name
/// nothing on this machine answers to is a warning and the default — a theme
/// file can be deleted between two launches, and that is not a reason to
/// refuse to start.
fn apply_startup_theme(settings: &Settings, overrides: &Overrides) {
    let name = overrides
        .theme
        .as_deref()
        .unwrap_or_else(|| settings.theme());

    match theme::named(name) {
        Some(palette) => theme::set_theme(palette),
        None => log::warn!("no theme called {name:?} on this machine; opening in the default"),
    }
}

/// Renders one frame of the real view tree and writes it to `path`.
///
/// With `--run`, this is also the only way to see a shell in a picture: the
/// command is typed into the one pane, the queue is pumped until the grid stops
/// changing, and the frame that is written is the one with the output in it.
fn write_snapshot(path: &std::path::Path, overrides: Overrides) -> Result<()> {
    let font_db = CosmicFontDb::new().context("no usable system fonts")?;
    // The platform's default family at the default size, and never the
    // settings file's: a snapshot is a picture of the *application*, and one
    // that came out in whatever font and size the person running it happens to
    // have chosen would be a different picture on every machine.
    let fonts = resolve_fonts(&font_db, None)?;
    let cell_font = CellFont::new(font_db.glyphs(), fonts.monospace, CELL_FONT_SIZE)?;
    let text_layout: Arc<dyn TextLayoutSystem> = Arc::new(font_db.text_layout());

    // A local queue stands in for the event loop. Nothing is spawned onto it
    // unless `--run` asked for it: neither the git gather nor a sandboxed
    // plugin's poll is started, so the frame is the same on a build machine
    // with no network and no repository — and it uses ephemeral settings, so
    // it is also the same whatever options the person running it happens to
    // have.
    let queue = LocalQueue::new();
    let mut app = App::new(queue.foreground(), background_pool());

    let quit: QuitRequest = Rc::new(|| {});
    let mut settings = Settings::ephemeral();
    // The one thing a snapshot takes from the machine it runs on, and only
    // when it was asked to load that machine's plugins: what a person has
    // allowed each of them. A plugin drawn without its grants is a plugin
    // drawing the refusal rather than the thing, which is a picture of the
    // permission dialog and not of the plugin.
    if overrides.with_plugins {
        for (plugin, keys) in Settings::for_user().plugin_grants() {
            settings.set_granted(plugin, keys.clone());
        }
    }
    apply_startup_theme(&settings, &overrides);
    // A snapshot is always rendered as the dev channel: the only thing the
    // channel reaches is the About page's label, and a PNG that said "stable"
    // on a machine that built it from a working tree would be wrong in the one
    // way a snapshot exists to catch.
    // Before the window, because a fixture that is not one is a line to print
    // rather than a window to open — and a closure that builds a window has
    // nowhere to return an error to.
    let plugins = with_dev_plugin(
        with_fixture(
            match overrides.with_plugins {
                true => everything_installed(),
                false => crate::plugins::defaults(),
            },
            overrides.fixture.as_deref(),
        )?,
        overrides.dev_plugin.as_deref(),
    )?;
    let (window_id, workspace) = app.add_window(|ctx| {
        Workspace::new(
            fonts,
            cell_font,
            Opening {
                settings,
                channel: Channel::Dev,
                plugins,
                // With the plugins and not otherwise, which is the same rule
                // the plugins themselves follow: a picture of the application
                // is the same everywhere, and one that includes this machine's
                // plugins already includes whatever this machine knows about
                // them — a withdrawn one among them.
                withdrawn: match overrides.with_plugins {
                    true => withdrawn_plugins(),
                    false => std::collections::BTreeMap::new(),
                },
            },
            quit,
            Rc::new(Detached),
            ctx,
        )
    });
    app.update(|ctx| {
        workspace.update(ctx, |workspace, ctx| {
            // A run with a shell to show wants one pane filling the body, not
            // four seeded ones sharing it — and four shells opened to draw a
            // picture of one.
            if !overrides.wants_shells() {
                seed_snapshot_tabs(workspace, ctx);
            }
            apply_overrides(workspace, &overrides, ctx);
        });
    });

    // The presenter is a parameter rather than something the frame closure
    // holds, because typing needs it too: a keystroke is dispatched through the
    // element tree the last frame built.
    let mut presenter = Presenter::new(window_id, text_layout);
    let frame = |app: &mut App, presenter: &mut Presenter| {
        app.update(|ctx| {
            let invalidation = ctx.take_all_invalidations_for_window(window_id);
            presenter.invalidate(invalidation, ctx);
            presenter.build_scene(WINDOW_SIZE, SNAPSHOT_SCALE_FACTOR, ctx)
        })
    };

    // The gather a real run starts and a snapshot does not, for the same
    // reason the plugins are not loaded: it walks a directory tree and spawns
    // `git`, and a picture that did either would be a different picture in
    // every checkout. Started when the plugins are, because half of what they
    // draw is what it finds — a chip that says which branch you are on has
    // nothing to say until it has run.
    if overrides.with_plugins {
        app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| workspace.start_git_poll(ctx));
        });
    }

    if overrides.wants_shells() {
        let pane = start_shells(&mut app, &workspace)?;
        // A frame before a keystroke, because it is *layout* that measures the
        // pane and resizes the pty — and because a key is dispatched into the
        // element tree that frame builds. A command typed into the grid every
        // terminal starts at stays wrapped at eighty columns however wide the
        // window it is finally drawn in.
        frame(&mut app, &mut presenter);

        if !overrides.run.is_empty() {
            await_shell(&queue, &mut app, &workspace, pane);
            for command in &overrides.run {
                type_run(&mut app, &mut presenter, window_id, command);
                let printed = settle_run(&queue, &mut app, &workspace, pane);
                println!("the shell printed:\n{}", printed.trim_end());
            }
        }

        // Last, so that the line in the field is the one the picture was asked
        // for rather than whatever the shell has been doing since, and so that
        // there is output on screen for `--select-output` to find.
        compose_pane(
            &mut app,
            &workspace,
            pane,
            &Composed::from_overrides(&overrides),
        );

        // A frame first, because both of these are answered against what the
        // last layout measured: how far the list can scroll, and which blocks
        // are on screen to be hovered.
        frame(&mut app, &mut presenter);
        aim_at_blocks(&mut app, &workspace, pane, &overrides);

        // And then, for a menu, a second frame between the hover and the
        // opening: the corner a menu hangs from is where the dots were last
        // *painted*, and nothing has painted them until the block under them
        // is hovered. See `Overrides::block_menu`.
        if let Some(index) = overrides.block_menu {
            app.update(|ctx| {
                workspace.update(ctx, |workspace, ctx| {
                    workspace.hover_block(pane, index, ctx);
                });
            });
            frame(&mut app, &mut presenter);
            app.update(|ctx| {
                workspace.update(ctx, |workspace, ctx| {
                    if !workspace.open_block_menu_at(pane, index, ctx) {
                        log::warn!("`--block-menu` found no block {index} to open a menu on");
                    }
                });
            });
        }
    }

    // A frame first, and then the gesture: a press has to land on a row, and
    // where the rows are is a thing only a paint pass knows.
    if overrides.carries() {
        frame(&mut app, &mut presenter);
        carry_panel_row(
            &mut app,
            &mut presenter,
            window_id,
            &workspace,
            &overrides,
            |app, presenter| {
                frame(app, presenter);
            },
        );
    }

    // Last of everything, and after a frame: a plugin's action is usually
    // about what the pane has just told it, and what it puts up is drawn on
    // the frame after the one that ran it.
    if !overrides.actions.is_empty() {
        frame(&mut app, &mut presenter);
        for name in &overrides.actions {
            run_named_action(&mut app, &workspace, name);
            // Whatever it asked the host for — a directory listed, a
            // repository read — is done on the pool, so the foreground has
            // nothing to run until it comes back. Waited for the way
            // `await_shell` waits: a poll and a deadline, because there is no
            // event loop here to be woken by.
            let deadline = Instant::now() + ACTION_TIMEOUT;
            while Instant::now() < deadline {
                queue.run_until_parked();
                std::thread::sleep(RUN_POLL);
            }
        }
        frame(&mut app, &mut presenter);
    }

    let scene = frame(&mut app, &mut presenter);

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

/// How long a `--action` is given for whatever it asked the host for.
///
/// A snapshot is a still picture and this is the whole of the waiting in it.
/// Long enough for two things rather than one: the request the action raised,
/// which is answered off the pool in microseconds, and the *next turn of the
/// plugin's own poll* — because what a chip draws is usually what it last
/// asked for, and a plugin that refreshes every couple of seconds has nothing
/// new to say for a couple of seconds. A plugin that asked for something over
/// the network is still drawn mid-request, which is a picture of a plugin
/// waiting and a perfectly good thing to look at.
const ACTION_TIMEOUT: Duration = Duration::from_millis(2_500);

/// Runs one action by name, or says why it could not.
///
/// By name because that is what an action *is*: the command line has no idea
/// which plugins are installed, and a plugin's own name for its own action is
/// the whole of the addressing this tier has.
fn run_named_action(app: &mut App, workspace: &ViewHandle<Workspace>, given: &str) {
    // A name, and then whatever the caller wants the action to be told —
    // separated by a space, which no action name may hold. That second half is
    // what a picker's row or a menu's entry says when a person presses it, and
    // an action that takes one cannot be looked at without it.
    let (name, argument) = given.split_once(char::is_whitespace).unwrap_or((given, ""));
    let Ok(action) = crook_plugin::ActionName::parse(name.trim()) else {
        log::warn!("{name:?} is not the name of an action");
        return;
    };
    let argument = argument.trim().to_owned();
    app.update(|ctx| {
        workspace.update(ctx, |workspace, ctx| {
            workspace.host().say(argument.clone());
            match workspace.host().action(&action) {
                Some(id) => workspace.run_action(id, ctx),
                None => log::warn!("nothing here answers to {action}"),
            }
            // Whatever it did, the window has to be drawn again to show it. An
            // action run from a chord arrives with a keystroke and a frame
            // behind it; this one arrives from the command line and has
            // neither.
            ctx.notify();
        });
    });
}

/// Opens the shells and reports the pane the command line is aimed at.
fn start_shells(app: &mut App, workspace: &ViewHandle<Workspace>) -> Result<PaneId> {
    let pane = workspace
        .read(&*app, |workspace, _| workspace.tabs().focused_pane_id())
        .context("the strip opened with no pane to run a command in")?;

    app.update(|ctx| {
        workspace.update(ctx, |workspace, ctx| workspace.start_terminals(ctx));
    });
    Ok(pane)
}

/// Waits for the shell to say something of its own before anything is typed.
///
/// A pty echoes what is typed into it whether or not a child has read it yet,
/// so typing immediately produces text that looks like progress while the shell
/// is still starting. [`settle_run`] would then time its quiet window from that
/// echo and call a shell that has not run anything finished — which it did, on
/// a prompt that takes longer than [`RUN_QUIET`] to draw itself.
///
/// Returns false when nothing was heard, and the caller types anyway: a shell
/// with no prompt at all is a strange shell, not a reason to run nothing.
fn await_shell(
    queue: &LocalQueue,
    app: &mut App,
    workspace: &ViewHandle<Workspace>,
    pane: PaneId,
) -> bool {
    let deadline = Instant::now() + RUN_TIMEOUT;
    while Instant::now() < deadline {
        queue.run_until_parked();
        let spoken = workspace
            .read(&*app, |workspace, app| workspace.terminal_text(pane, app))
            .unwrap_or_default();
        if !spoken.trim().is_empty() {
            return true;
        }
        std::thread::sleep(RUN_POLL);
    }
    log::warn!("the shell printed nothing within {RUN_TIMEOUT:?}; typing anyway");
    false
}

/// What `--type`, `--select` and `--select-output` asked to leave on screen.
///
/// Applied *after* a `--run` command has been typed and sent, in both kinds of
/// run: the two share one field, and a run that typed its command over this
/// would send the two of them as one line — and there is nothing in the output
/// to select until the command that printed it has finished.
#[derive(Clone, Debug, Default)]
struct Composed {
    /// The line to leave in the field, unsent.
    text: Option<String>,
    /// The text to select within it.
    selected: Option<String>,
    /// The text to select in the pane's output, above the field.
    selected_output: Option<String>,
    /// How far to carry that selection, when it runs past its own marker.
    selected_through: Option<String>,
}

impl Composed {
    /// What the command line asked the pane to hold.
    fn from_overrides(overrides: &Overrides) -> Self {
        Self {
            text: overrides.type_text.clone(),
            selected: overrides.select.clone(),
            selected_output: overrides.select_output.clone(),
            selected_through: overrides.select_through.clone(),
        }
    }

    /// Whether nothing was asked for, which is every ordinary run.
    fn is_empty(&self) -> bool {
        self.text.is_none() && self.selected.is_none() && self.selected_output.is_none()
    }
}

/// Puts what the command line asked for into a pane: a line in its field, a
/// selection in that line, and a selection in the output above it.
fn compose_pane(app: &mut App, workspace: &ViewHandle<Workspace>, pane: PaneId, asked: &Composed) {
    if asked.is_empty() {
        return;
    }

    app.update(|ctx| {
        workspace.update(ctx, |workspace, ctx| {
            if let Some(text) = asked.text.as_deref() {
                workspace.type_into_input(pane, text, ctx);
            }
            if let Some(selected) = asked.selected.as_deref()
                && !workspace.select_in_input(pane, selected, ctx)
            {
                log::warn!("`--select` found no {selected:?} in the field to select");
            }
            if let Some(selected) = asked.selected_output.as_deref() {
                let through = asked.selected_through.as_deref().unwrap_or(selected);
                if !workspace.select_in_output_through(pane, selected, through, ctx) {
                    log::warn!(
                        "`--select-output` found no {selected:?}..{through:?} in the output"
                    );
                }
            }
        });
    });
}

/// Puts the pointer and the scroll position where `--hover-block` and
/// `--scroll-blocks` asked for them.
fn aim_at_blocks(
    app: &mut App,
    workspace: &ViewHandle<Workspace>,
    pane: PaneId,
    overrides: &Overrides,
) {
    app.update(|ctx| {
        workspace.update(ctx, |workspace, ctx| {
            if let Some(lines) = overrides.scroll_blocks
                && !workspace.scroll_blocks(pane, -lines as f32, ctx)
            {
                log::warn!("`--scroll-blocks` found nothing to scroll");
            }
            if let Some(index) = overrides.hover_block
                && !workspace.hover_block(pane, index, ctx)
            {
                log::warn!("`--hover-block` found no block {index} to hover");
            }
        });
    });
}

/// Presses on a row of the panel and carries it, and never lets go.
///
/// Through the window's own event dispatch, for the reason `--run` types its
/// command as keystrokes rather than writing to the pty: a drag is a press,
/// some travel and no release, and a state poked into the workspace instead
/// would make the picture a picture of a second code path. Everything a real
/// gesture goes through — the press's hit test, the threshold, the band, the
/// strip — is on this path too.
///
/// In steps rather than in one leap, because the list reorders itself under
/// the hand and every position is answered against the frame the last one
/// produced. One leap is not a thing a hand does.
fn carry_panel_row(
    app: &mut App,
    presenter: &mut Presenter,
    window_id: WindowId,
    workspace: &ViewHandle<Workspace>,
    overrides: &Overrides,
    mut frame: impl FnMut(&mut App, &mut Presenter),
) {
    let grip = workspace.read(&*app, |workspace, _| match overrides.carry {
        Some(index) => workspace.panel_row_grip(index),
        None => overrides
            .carry_group
            .and_then(|index| workspace.panel_block_grip(index)),
    });
    let Some(from) = grip else {
        log::warn!("`--carry` found nothing at that index to pick up");
        return;
    };
    let by = overrides
        .carry_by
        .map_or(CARRY_DISTANCE, |pixels| pixels as f32);

    // The pointer arrives before it presses, which is not decoration: a row
    // the pointer has reached is a row with its detail card up, and a press is
    // hit-tested against the layer that card put the row into.
    mouse(
        app,
        presenter,
        window_id,
        |position| Event::MouseMoved {
            position,
            modifiers: Modifiers::default(),
            is_synthetic: false,
        },
        from,
    );
    frame(app, presenter);
    mouse(
        app,
        presenter,
        window_id,
        |position| Event::MouseDown {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        },
        from,
    );
    frame(app, presenter);

    for step in 1..=CARRY_STEPS {
        let at = from + vec2f(0., by * step as f32 / CARRY_STEPS as f32);
        mouse(
            app,
            presenter,
            window_id,
            |position| Event::MouseDragged {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
            },
            at,
        );
        frame(app, presenter);
    }
}

/// Dispatches one mouse event at `position` through the window.
fn mouse(
    app: &mut App,
    presenter: &mut Presenter,
    window_id: WindowId,
    event: impl FnOnce(Vector2F) -> Event,
    position: Vector2F,
) {
    app.update(|ctx| ctx.dispatch_window_event(window_id, event(position), presenter));
}

/// Types a `--run` command into the focused pane's field and sends it.
///
/// As keystrokes, through the window's own event dispatch, because that is the
/// path a command actually takes now: the field composes the line and Enter is
/// what hands it to the pty. Writing to the pty directly would prove the pty
/// works and nothing at all about the terminal in front of it.
fn type_run(app: &mut App, presenter: &mut Presenter, window_id: WindowId, command: &str) {
    for character in command.chars() {
        // A command with a line in it is two lines sent, which is what pressing
        // Return twice would do.
        if character == '\n' {
            press(app, presenter, window_id, "enter", "\r");
            continue;
        }
        press(
            app,
            presenter,
            window_id,
            &character.to_lowercase().to_string(),
            &character.to_string(),
        );
    }
    press(app, presenter, window_id, "enter", "\r");
}

/// Presses one key on the window, exactly as the platform would.
///
/// Crook's own bindings would be consumed before this in the delegate; none of
/// them is a bare key, so nothing typed here is ever swallowed on the way.
fn press(app: &mut App, presenter: &mut Presenter, window_id: WindowId, key: &str, chars: &str) {
    let event = Event::KeyDown {
        keystroke: Keystroke::new(key, Modifiers::default()),
        chars: chars.to_owned(),
    };
    app.update(|ctx| ctx.dispatch_window_event(window_id, event, presenter));
}

/// Pumps the queue until the pane stops changing, and returns what it says.
///
/// "Stopped changing" is the only definition of finished available from outside
/// a pty: a shell runs the command, prints its prompt back, and goes quiet.
/// Bounded by [`RUN_TIMEOUT`], because a command that never goes quiet — `top`,
/// a `sleep` — still has to produce a picture.
fn settle_run(
    queue: &LocalQueue,
    app: &mut App,
    workspace: &ViewHandle<Workspace>,
    pane: PaneId,
) -> String {
    let deadline = Instant::now() + RUN_TIMEOUT;
    let mut showing = String::new();
    let mut unchanged_since = Instant::now();

    loop {
        queue.run_until_parked();
        let now = workspace
            .read(&*app, |workspace, app| workspace.terminal_text(pane, app))
            .unwrap_or_default();

        if now != showing {
            showing = now;
            unchanged_since = Instant::now();
        } else if !showing.trim().is_empty() && unchanged_since.elapsed() >= RUN_QUIET {
            return showing;
        }

        if Instant::now() >= deadline {
            log::warn!("the command was still printing after {RUN_TIMEOUT:?}");
            return showing;
        }
        std::thread::sleep(RUN_POLL);
    }
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
            title: "sandbox the plugin host",
            status: AgentStatus::NeedsInput,
            directory: "crates/crook_wasm/src",
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

    /// The checkout that joins the second tab's group: what a worktree opened
    /// from a tab looks like once it is one.
    const WORKTREE: Seeded = Seeded {
        title: "try the atlas rewrite",
        status: AgentStatus::Running,
        directory: "docs",
        branch: "eugen/atlas-rewrite",
        diff: Some((3, 88, 12)),
    };

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
            worktree: false,
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

    // And a worktree opened from the last tab, which is the gesture groups
    // exist for: a second checkout of the same work, folded under one heading
    // beside the tab it came from. After the sessions above, so the group
    // takes its heading from what that tab's agent has called its work rather
    // than from the name nobody chose.
    if let Some(last) = workspace.tabs().iter().map(Tab::id).last() {
        workspace.apply(TabAction::NewInGroupOf(last), ctx);
        if let Some(pane) = workspace.tabs().focused_pane_id() {
            // A checkout of its own, beside the one it was cut from, because
            // that is what a worktree *is*: git facts are recorded per
            // directory, so two rows sharing a path would show one row's
            // branch on both — and, now that a row's mark can be a plugin's,
            // one row's mark on both.
            let directory = root.with_file_name("crook-atlas").join(WORKTREE.directory);
            workspace.update_session(pane, ctx, |session| {
                session.derived_title = Some(WORKTREE.title.to_owned());
                session.status = WORKTREE.status;
                session.working_directory = Some(directory.clone());
            });
            let facts = git::GitFacts {
                branch: Some(git::Head::Branch(WORKTREE.branch.to_owned())),
                diff: WORKTREE
                    .diff
                    .map(
                        |(files_changed, lines_added, lines_removed)| git::DiffStats {
                            files_changed,
                            lines_added,
                            lines_removed,
                        },
                    ),
                // The one seeded row that is one, which is what makes a
                // plugin that marks worktrees visible in a demo window.
                worktree: true,
            };
            workspace
                .git()
                .update(ctx, |model, ctx| model.record(directory, facts, ctx));
        }
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
    /// The pane `--run` typed into, and how long it may take to answer.
    ///
    /// `None` for every ordinary run. When it is set, the frame budget does not
    /// start counting until this pane has printed something: `--frames 3` would
    /// otherwise draw three empty grids in the time it takes a shell to open a
    /// pty, and the run it was meant to prove would prove nothing.
    run: Option<Run>,
    /// What the command line asked to leave on screen, until the first frame
    /// has been drawn. See [`Composed`].
    composed: Composed,
    /// The rectangle the input method was last told the caret occupies, so a
    /// frame that did not move it sends no message.
    ime_area: Option<crookui_core::geometry::RectF>,
    /// Where the workspace reads the window's size from.
    ///
    /// The size is an argument to `build_scene` and reaches nothing in the
    /// view tree, so the delegate writes it down for the one thing that wants
    /// it: the session file, which is what makes the next window open the size
    /// this one was.
    window_size: Rc<std::cell::Cell<Vector2F>>,
    /// The window, for the header that is its title bar.
    window: WindowHandle,
    /// What the window was doing when the last frame was built.
    ///
    /// The window's state changes for reasons no application hears about — the
    /// macOS green button, a tiling compositor, a shortcut belonging to the
    /// desktop — and one thing Crook lays out depends on it: macOS takes the
    /// traffic lights away in fullscreen, so the room reserved for them has to
    /// go too. Comparing it each frame is what turns a change nobody reported
    /// into a repaint.
    window_state: WindowState,
}

/// The real window, behind the handle the workspace holds.
///
/// The whole of the seam: four verbs forwarded to the windowing layer, which
/// is the only crate in the workspace that knows what a window is. Everything
/// above it — the header, the panel's title strip, the window plugin's
/// commands — is written against [`window_controls::WindowControls`] and runs
/// unchanged with nothing behind it.
struct RealWindow(PlatformWindow);

impl window_controls::WindowControls for RealWindow {
    fn state(&self) -> WindowState {
        WindowState {
            fullscreen: self.0.is_fullscreen(),
        }
    }

    fn start_drag(&self) {
        self.0.start_drag();
    }

    fn toggle_maximized(&self) {
        self.0.toggle_maximized();
    }

    fn minimize(&self) {
        self.0.minimize();
    }
}

/// The state of a `--run` in a windowed session.
struct Run {
    pane: PaneId,
    /// The commands still to be typed, in order. The first waits for the first
    /// frame, because layout is what measures the pane and resizes the pty,
    /// and a command typed before it would be wrapped at the eighty columns
    /// every terminal starts at.
    pending: std::collections::VecDeque<String>,
    /// When to give up waiting for output and start counting frames anyway, so
    /// a command that prints nothing still ends the run.
    deadline: Instant,
    printed: bool,
}

/// What the command line decided, as one argument.
///
/// Three values that arrive together, are read once each, and travel from
/// [`parse_args`] to [`Shell::new`] without anything in between looking at
/// them. Passing them separately put `Shell::new` one argument over clippy's
/// limit, and grouping them is the answer that says something true: they are
/// the launch, not three unrelated parameters.
struct Launch {
    /// Which channel is running, for the settings page's About section.
    channel: Channel,
    /// Exit after this many frames, so the binary is runnable unattended.
    frames: Option<u32>,
    /// What the command line asked to start differently.
    overrides: Overrides,
}

/// Forgets what a plugin was allowed to do, and that it was switched off.
///
/// Uninstalling is the one moment either of those is thrown away. `settings`
/// deliberately keeps both for a plugin it does not recognise — a plugin can
/// be missing because it failed to load this morning, and a person who
/// switched one off does not want that answer forgotten when it comes back.
/// Being *removed* is different: it is a person saying they are done with it,
/// and a reinstall a year later must ask again rather than quietly running
/// under a grant nobody remembers giving.
///
/// This is the command line's half, and it is a *file* edit: a Crook that is
/// open at the time holds its own copy of the settings and will write the
/// grant back the next time anything saves them. That is the ordinary hazard
/// of editing a file an application has open, it is why `settings.json` says
/// what it says about hand-editing, and it is not worth a lock — the answer is
/// the Plugins page, which removes a plugin from inside the window that would
/// otherwise overwrite this.
fn forget_plugin(id: &crook_plugin::PluginId) -> Result<()> {
    let mut settings = Settings::for_user();
    settings.set_granted(id.as_str(), Vec::new());
    settings.set_plugin_disabled(id.as_str(), false);
    settings.save_blocking()
}

/// What `--plugins` prints: every plugin installed as a file, and where.
///
/// The ones in the binary are not on this list. It answers the two questions a
/// person has before they uninstall or report something — which version am I
/// running, and which file is it — and both of those are only questions for a
/// plugin that came from outside.
fn installed_plugins_text() -> String {
    let Some(directory) = crate::plugins::wasm::directory() else {
        return String::from("this machine has no data directory to install plugins into");
    };

    let settings = Settings::for_user();
    let installed = crate::plugins::wasm::installed(&directory);
    if installed.is_empty() {
        return format!("no plugins installed in {}", directory.display());
    }

    let mut lines = Vec::with_capacity(installed.len());
    for plugin in &installed {
        let manifest = crate::plugin::Plugin::manifest(plugin.as_ref());
        let id = manifest.id.as_str();
        let off = match settings.disabled_plugins().iter().any(|name| name == id) {
            true => "  (switched off)",
            false => "",
        };
        let module = crate::plugins::wasm::module(&manifest.id)
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        lines.push(format!("{id}  {}  {module}{off}", manifest.version));
    }
    lines.join("\n")
}

/// Every plugin this window carries: the ones in the box, then the ones a
/// person installed.
///
/// In that order, and it is load order: a slot's entries are settled by
/// `order` first and by load order second, so an installed plugin that asks
/// for the same place as one of Crook's own is drawn after it rather than
/// instead of it. A store plugin that wanted to come first has to say so.
fn everything_installed() -> Vec<Box<dyn crate::plugin::Plugin>> {
    let mut plugins = crate::plugins::defaults();
    if let Some(directory) = crate::plugins::wasm::directory() {
        plugins.extend(crate::plugins::wasm::installed(&directory));
    }
    plugins
}

/// The plugins a window carries, with a fixture among them if one was asked
/// for.
///
/// At the end of the list, which is the front of a slot: a fixture asks for a
/// low order and load order settles the rest, so what a picture is being taken
/// *of* is what fills a slot there is one of.
fn with_fixture(
    mut plugins: Vec<Box<dyn crate::plugin::Plugin>>,
    fixture: Option<&std::path::Path>,
) -> Result<Vec<Box<dyn crate::plugin::Plugin>>> {
    let Some(path) = fixture else {
        return Ok(plugins);
    };

    let fixture = crate::plugins::wasm::fixture::Fixture::read(path)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?;
    plugins.push(Box::new(fixture));
    Ok(plugins)
}

/// The same, for the plugin somebody is writing.
///
/// The path is resolved here so that a `--dev-plugin` naming nothing is a line
/// on the command line rather than a window that opens missing the one thing
/// it was started for. Whether the module *loads* is a different question and
/// deliberately not this one: a build that is half written is the ordinary
/// state of a plugin being worked on, and the watcher takes the next one.
fn with_dev_plugin(
    mut plugins: Vec<Box<dyn crate::plugin::Plugin>>,
    dev: Option<&std::path::Path>,
) -> Result<Vec<Box<dyn crate::plugin::Plugin>>> {
    let Some(path) = dev else {
        return Ok(plugins);
    };

    use crate::plugins::wasm::dev::Dev;

    let module = Dev::module(path)
        .map_err(anyhow::Error::msg)
        .with_context(|| path.display().to_string())?;

    // Loaded here rather than by the watcher, so that the plugin somebody is
    // writing is on the *first* frame: a window that came up without it and
    // grew it a second later would be a second of every launch, and a
    // snapshot with nothing in it.
    //
    // A module that does not open is a line and a window that still opens,
    // because half a build is the ordinary state of a plugin being written and
    // the next one is a second away.
    let seen = Dev::about(&module);
    match std::fs::read(&module)
        .map_err(|why| why.to_string())
        .and_then(|bytes| crate::plugins::wasm::opened(&bytes).map(|plugin| (plugin, bytes.len())))
    {
        Ok((plugin, bytes)) => {
            let named = crate::plugin::Plugin::manifest(&plugin).id.clone();
            log::info!("{named} from {} ({bytes} bytes)", module.display());
            plugins.push(Box::new(plugin));
        }
        Err(why) => log::warn!("{}: {why}", module.display()),
    }

    plugins.push(Box::new(Dev::watching(module, seen)));
    Ok(plugins)
}

/// Which installed plugins the registry has withdrawn, and why.
///
/// Read off the store's copy of the index — the file the Store section keeps
/// beside the plugins — rather than over the network, because a yank has to be
/// honoured on a machine that is offline and on the launch after the registry
/// said so. Nothing here fetches anything, and an index that is missing or
/// unreadable withdraws nothing: a list that cannot be read is never a reason
/// to stop running something somebody installed.
///
/// A version is what is withdrawn rather than a plugin, so the answer is about
/// the version on this machine: somebody running the one before the bad one
/// keeps running it.
fn withdrawn_plugins() -> std::collections::BTreeMap<String, String> {
    let Some(index) = crate::plugins::store::cache::Cache::user()
        .and_then(|cache| cache.read())
        .map(|cached| cached.index)
    else {
        return std::collections::BTreeMap::new();
    };

    let Some(directory) = crate::plugins::wasm::directory() else {
        return std::collections::BTreeMap::new();
    };

    let mut withdrawn = std::collections::BTreeMap::new();
    for plugin in crate::plugins::wasm::installed(&directory) {
        let manifest = crate::plugin::Plugin::manifest(plugin.as_ref());
        if let Some(why) =
            crate::plugins::store::index::withdrawn(&index, &manifest.id, manifest.version)
        {
            withdrawn.insert(manifest.id.to_string(), why);
        }
    }
    withdrawn
}

impl Shell {
    fn new(
        platform: &Platform,
        fonts: Fonts,
        cell_font: CellFont,
        settings: Settings,
        text_layout: Arc<dyn TextLayoutSystem>,
        launch: &Launch,
        session: crate::session::Session,
    ) -> Self {
        let mut app = App::new(platform.foreground.clone(), background_pool());

        let proxy = platform.proxy.clone();
        let window: WindowHandle = Rc::new(RealWindow(platform.window.clone()));
        let quit: QuitRequest = {
            let proxy = proxy.clone();
            // Closing the last tab closes the window, which is what keeps the
            // strip from ever having to represent "no tabs".
            Rc::new(move || proxy.exit())
        };

        // A path that names nothing was already refused before the window was
        // asked for — see `open_window` — so anything wrong here is a plugin
        // that will not load, which the watcher says and the window survives.
        let plugins = with_dev_plugin(
            everything_installed(),
            launch.overrides.dev_plugin.as_deref(),
        )
        .unwrap_or_else(|why| {
            log::warn!("{why:#}");
            everything_installed()
        });
        let (window_id, workspace) = app.add_window(|ctx| {
            Workspace::new(
                fonts,
                cell_font,
                Opening {
                    settings,
                    channel: launch.channel,
                    plugins,
                    withdrawn: withdrawn_plugins(),
                },
                quit,
                window.clone(),
                ctx,
            )
        });
        let window_size = workspace.read(&app, |workspace, _| workspace.window_size_cell());
        app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                // First of all, because everything below it works on whatever
                // strip is there: the overrides open a settings page in it,
                // the git poll reads its directories, and `start_terminals`
                // opens a shell in every pane it holds.
                //
                // A session that describes nothing leaves the strip a fresh
                // window's, which is what every launch had before there was a
                // file to read.
                if let Some(strip) = session.restore() {
                    workspace.restore(strip, ctx);
                }
                // Before the polls, not after: `start_git_poll` decides
                // whether to pay for `git diff` from the density it finds, and
                // a density the command line asked for has to be in place by
                // then or the first cycle gathers the wrong half.
                apply_overrides(workspace, &launch.overrides, ctx);
                workspace.start_git_poll(ctx);
                workspace.start_caret_blink(ctx);
                // Last, because it opens a shell in every pane there is and the
                // overrides above can still change how many that is.
                workspace.start_terminals(ctx);
            });
        });

        // The only thing that makes a frame happen: a view said it changed.
        let redraw = proxy.clone();
        app.on_window_invalidated(window_id, move |_, _| redraw.request_redraw());

        // The windowed run types the commands one after another, so they are
        // queued rather than joined: two commands sent as one line would be
        // one block.
        let run = (!launch.overrides.run.is_empty())
            .then(|| start_shells(&mut app, &workspace).ok())
            .flatten()
            .map(|pane| Run {
                pane,
                pending: launch.overrides.run.clone().into(),
                deadline: Instant::now() + RUN_TIMEOUT,
                printed: false,
            });

        Self {
            composed: Composed::from_overrides(&launch.overrides),
            app,
            presenter: Presenter::new(window_id, text_layout),
            window_id,
            workspace,
            proxy,
            frames_drawn: 0,
            frame_budget: launch.frames,
            run,
            ime_area: None,
            window_size,
            window,
            window_state: WindowState::default(),
        }
    }

    /// Repaints when the window's own state has changed under the frame.
    ///
    /// Asked rather than listened for, because there is nothing to listen to:
    /// entering fullscreen arrives as a resize like any other, and being
    /// maximised by the desktop arrives as nothing at all. Cheap enough to ask
    /// once a frame — two calls into the window system — and a frame is
    /// exactly when the answer is needed.
    fn sync_window_state(&mut self) {
        let state = self.window.state();
        if state == self.window_state {
            return;
        }

        self.window_state = state;
        // Its own update, before the one that builds the frame: an effect
        // drains when the outermost update unwinds, so a notify raised inside
        // the build would be taken by the frame after this one.
        let workspace = &self.workspace;
        self.app
            .update(|ctx| workspace.update(ctx, |_, ctx| ctx.notify()));
    }

    /// Types the `--run` command, once there has been a frame to size the pane
    /// it goes into and to build the tree the keystrokes are dispatched into.
    fn type_pending_run(&mut self) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        let Some(command) = run.pending.pop_front() else {
            return;
        };

        run.deadline = Instant::now() + RUN_TIMEOUT;
        type_run(&mut self.app, &mut self.presenter, self.window_id, &command);
    }

    /// Leaves what the command line asked for on screen, once there is a pane
    /// and any `--run` command has both been sent and answered.
    ///
    /// The wait for the answer is `--select-output`'s: there is nothing in the
    /// output to select until the shell has printed it, and a run with no
    /// command to wait for has already answered.
    fn compose_pending_pane(&mut self) {
        if self.composed.is_empty() || !self.budget_has_started() {
            return;
        }
        let asked = std::mem::take(&mut self.composed);
        if let Ok(pane) = start_shells(&mut self.app, &self.workspace) {
            compose_pane(&mut self.app, &self.workspace, pane, &asked);
        }
    }

    /// Whether a frame counts towards the budget yet.
    ///
    /// Always, unless `--run` is still waiting for its command to say
    /// something. See [`Shell::run`].
    fn budget_has_started(&mut self) -> bool {
        let Some(run) = self.run.as_ref() else {
            return true;
        };
        if run.printed {
            return true;
        }

        let pane = run.pane;
        let printed = self
            .workspace
            .read(&self.app, |workspace, app| {
                workspace.terminal_text(pane, app)
            })
            .is_some_and(|text| !text.trim().is_empty());

        let run = self.run.as_mut().expect("checked a moment ago");
        run.printed = printed || Instant::now() >= run.deadline;
        run.printed
    }

    /// Says what the pane `--run` typed into is showing, so an unattended run
    /// leaves evidence that the loop worked.
    fn report_run(&self) {
        let Some(run) = self.run.as_ref() else {
            return;
        };
        let printed = self
            .workspace
            .read(&self.app, |workspace, app| {
                workspace.terminal_text(run.pane, app)
            })
            .unwrap_or_default();
        log::info!("the shell printed:\n{}", printed.trim_end());
    }

    /// Moves the rectangle an input method puts its candidate list beside, so
    /// that a half-composed word and the list of things it could become are in
    /// the same place on screen.
    ///
    /// After the frame rather than during it: the caret's position is a result
    /// of laying the line out at the width the field was given, so it is not
    /// known until the field has been painted. Sent only when it moved,
    /// because every window system takes this as a message.
    fn follow_caret_with_the_input_method(&mut self) {
        let caret = self
            .workspace
            .read(&self.app, |workspace, _| workspace.caret_rect());
        if caret == self.ime_area {
            return;
        }
        self.ime_area = caret;

        // A field that has no caret keeps the last rectangle rather than
        // being given a meaningless one: there is no composition to place, and
        // moving the box to the origin would drag a candidate list somebody is
        // looking at into the corner.
        if let Some(caret) = caret {
            self.proxy.set_ime_area(caret.origin(), caret.size());
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
        // Written down rather than dispatched: it costs nothing, it invalidates
        // nothing, and it is the only place the window's size is known.
        self.window_size.set(size);
        self.sync_window_state();

        let window_id = self.window_id;
        let presenter = &mut self.presenter;

        self.app.update(|ctx| {
            let invalidation = ctx.take_all_invalidations_for_window(window_id);
            presenter.invalidate(invalidation, ctx);
            presenter.build_scene(size, scale_factor, ctx)
        })
    }

    fn handle_event(&mut self, event: Event) -> bool {
        // Not hit-tested and not dispatched into the tree: the desktop's
        // setting is about the window rather than about anything in it, and
        // the workspace is the one thing that knows whether it is being
        // followed.
        if let Event::SystemTheme(theme) = event {
            let workspace = &self.workspace;
            let dark = theme.is_dark();
            self.app.update(|ctx| {
                workspace.update(ctx, |workspace, ctx| workspace.set_system_dark(dark, ctx));
            });
            return self
                .app
                .read(|ctx| ctx.has_window_invalidations(self.window_id));
        }

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
        self.follow_caret_with_the_input_method();
        self.type_pending_run();
        self.compose_pending_pane();

        let Some(budget) = self.frame_budget else {
            return;
        };

        // A frame drawn before the command answered is not one of the N that
        // were asked for; asking for another is what keeps the loop turning
        // until the shell has something to draw.
        if !self.budget_has_started() {
            self.proxy.request_redraw();
            return;
        }

        self.frames_drawn += 1;
        if self.frames_drawn >= budget {
            log::info!("drew {budget} frames; exiting");
            self.report_run();
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
    fn the_shell_integration_snippet_can_be_asked_for_by_name() {
        // The module's own answer to "what about ssh, what about containers?"
        // is this text, and until it can be printed the answer is one nobody
        // can act on.
        for (name, file, guard) in [
            ("zsh", "~/.zshrc", "CROOK_SHELL_INTEGRATION"),
            ("bash", "~/.bashrc", "CROOK_SHELL_INTEGRATION"),
            (
                "fish",
                "~/.config/fish/config.fish",
                "CROOK_SHELL_INTEGRATION",
            ),
        ] {
            let text = shell_integration_text(Some(name))
                .unwrap_or_else(|error| panic!("{name} has an integration: {error:#}"));
            assert!(
                text.contains(file),
                "a person pasting {name}'s snippet has to be told where it goes"
            );
            assert!(text.contains(guard), "and it has to be the snippet itself");
        }

        assert!(
            shell_integration_text(Some("/usr/bin/nu")).is_err(),
            "a shell with no snippet has to say so rather than print nothing"
        );
        assert!(
            shell_integration_text(None).is_err(),
            "and the flag needs its argument"
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
                    granularity: Some(Granularity::Tabs),
                    density: Some(Density::Expanded),
                    ..Overrides::default()
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
                    settings: Some(Some("about".to_owned())),
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
                    settings: Some(None),
                    ..Overrides::default()
                }
            }
        );
        // A page name is not checked here: the pages come from plugins, so
        // which of them exist is not known until they have built. An unknown
        // one is a line in the log and the page the menu entry opens.
        assert_eq!(
            parse(&["--settings", "keybindings"]).expect("valid"),
            Startup::Window {
                frames: None,
                overrides: Overrides {
                    settings: Some(Some("keybindings".to_owned())),
                    ..Overrides::default()
                }
            }
        );

        // `--record` is a state of a row on one page, so it opens that page
        // rather than making somebody name it twice.
        assert_eq!(
            parse(&["--record", "crook/window/new-tab"]).expect("valid"),
            Startup::Window {
                frames: None,
                overrides: Overrides {
                    record: Some("crook/window/new-tab".to_owned()),
                    settings: Some(Some("Keyboard Shortcuts".to_owned())),
                    ..Overrides::default()
                }
            }
        );
        // Unless a page was already asked for, which is the person being more
        // specific rather than less.
        assert_eq!(
            parse(&["--settings", "about", "--record", "crook/window/new-tab"]).expect("valid"),
            Startup::Window {
                frames: None,
                overrides: Overrides {
                    record: Some("crook/window/new-tab".to_owned()),
                    settings: Some(Some("about".to_owned())),
                    ..Overrides::default()
                }
            }
        );
        assert!(parse(&["--record"]).is_err());

        // The two search boxes are two flags, because they are two lists
        // filtered at two different times. `--find` opens nothing: the panel
        // is on screen in every window there is.
        assert_eq!(
            parse(&["--find", "kettle"]).expect("valid"),
            Startup::Window {
                frames: None,
                overrides: Overrides {
                    find: Some("kettle".to_owned()),
                    ..Overrides::default()
                }
            }
        );
        assert!(parse(&["--find"]).is_err());

        assert!(parse(&["--density", "cosy"]).is_err());
        assert!(parse(&["--density"]).is_err());
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
            "--run",
            "--type",
            "--select",
            "--select-output",
            "--menu",
            "--hover",
            "--section",
            "--find",
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
    }

    /// Whether a pool of `workers` can still run a task once
    /// [`PARKED_WORKERS`] of them are parked on a timer.
    ///
    /// The parked tasks report that they are *running* before they park, so
    /// the answer never depends on how quickly the pool picked them up.
    ///
    /// Note what this does *not* establish: the count of parked tasks comes
    /// from [`PARKED_WORKERS`] itself, so a chain added to the application
    /// without raising that constant changes this scenario in step and goes on
    /// passing. Only the comment on the constant counts the chains.
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

        // And that the floor is actually applied, on the machines that need
        // it rather than on whichever one is running the suite. Everything
        // above is about a pool the test built for itself; this is the only
        // line about the pool the application builds.
        for cores in 1..=PARKED_WORKERS {
            assert!(
                pool_size(cores) > PARKED_WORKERS,
                "a {cores}-core machine would get a pool with nothing left to                  run a settings save on"
            );
        }
    }

    #[test]
    fn the_pane_overrides_are_carried_through_to_the_run() {
        assert_eq!(
            parse(&[
                "--type",
                "echo hi",
                "--select",
                "hi",
                "--select-output",
                "printed",
                "--select-through",
                "later"
            ])
            .expect("valid"),
            Startup::Window {
                frames: None,
                overrides: Overrides {
                    type_text: Some("echo hi".to_owned()),
                    select: Some("hi".to_owned()),
                    select_output: Some("printed".to_owned()),
                    select_through: Some("later".to_owned()),
                    ..Overrides::default()
                }
            }
        );
        // Any of them means a pane needs a shell, because a pane without one
        // draws a notice rather than a composer and has no output to select
        // in.
        assert!(Overrides::default().run.is_empty());
        assert!(!Overrides::default().wants_shells());
        assert!(
            Overrides {
                type_text: Some("x".to_owned()),
                ..Overrides::default()
            }
            .wants_shells()
        );
        assert!(
            Overrides {
                select_output: Some("x".to_owned()),
                ..Overrides::default()
            }
            .wants_shells()
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
        assert!(parse(&["--run"]).is_err());
        assert!(parse(&["--type"]).is_err());
        assert!(parse(&["--select"]).is_err());
        assert!(parse(&["--select-output"]).is_err());
        assert!(parse(&["--select-through"]).is_err());
        assert!(parse(&["--frames", "soon"]).is_err());
        assert!(parse(&["--tabs"]).is_err());
    }
}
