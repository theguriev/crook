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
pub mod keymap;
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
pub mod shell_integration;
pub mod tab;
pub mod terminal_font;
pub mod terminal_keys;
pub mod terminal_model;
pub mod text_input;
pub mod theme;
pub mod usage_model;
pub mod window_controls;
pub mod workspace;

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crookui::{
    CosmicFontDb, Platform, Proxy, WindowControls as PlatformWindow, WindowDelegate, WindowOptions,
    render_scene_to_rgba,
};
use crookui_core::event::{Event, Keystroke, Modifiers};
use crookui_core::executor::{Background, LocalQueue};
use crookui_core::geometry::{Vector2F, vec2f};
use crookui_core::platform::TextLayoutSystem;
use crookui_core::prelude::*;
use crookui_core::scene::Scene;
use crookui_core::{AddSingletonModel as _, App, Presenter, WindowId};

use crate::platform_insets::{ControlLayout, WindowChrome};
use crate::settings::{Density, Granularity, Layout, Settings};
use crate::tab::{AgentStatus, Direction, PaneId, Tab, TabAction};
use crate::terminal_font::{CELL_FONT_SIZE, CellFont};
use crate::usage_model::UsageModel;
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
    /// Start with the Themes panel open, and — with `creating` — on its
    /// creator.
    ///
    /// A way to look at a frame, like `--menu` and `--hover`: the panel is a
    /// surface, and a snapshot of it is a snapshot of the real thing.
    themes: bool,
    /// Start with the Themes panel making a theme.
    creating: bool,
    /// Start with the active tab's worktree menu open, and its creator with
    /// it when `creating_worktree`.
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
    settings: Option<Option<String>>,
    /// Type this into the settings page's search box at startup.
    ///
    /// Implies `--settings`: a query with no page to filter is nothing to
    /// look at. Like `--type` for a pane's field, it leaves the text in the
    /// box rather than doing anything with it, because leaving it there *is*
    /// what the box does — the page is filtered on every keystroke.
    search: Option<String>,
    /// Start in this layout rather than the saved one.
    layout: Option<Layout>,
    /// Draw another platform's window controls rather than this one's.
    ///
    /// The only override here that changes nothing a person can set. It exists
    /// because two thirds of the window's chrome is invisible on whichever
    /// machine Crook is being written on: the traffic lights are macOS's own,
    /// and the caption buttons Crook draws for Windows and Linux are drawn by
    /// this process, which means a picture of them needs no Windows and no
    /// Linux — only a way to ask for them.
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
    /// Whether this run needs shells opened for it.
    ///
    /// Neither the field nor the grid is worth a picture without one: a pane
    /// with no shell draws a notice instead of both.
    fn wants_shells(&self) -> bool {
        !self.run.is_empty()
            || self.type_text.is_some()
            || self.select_output.is_some()
            || self.hover_block.is_some()
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
            "--snapshot" => {
                let path = args.next().context("`--snapshot` needs a path")?;
                snapshot = Some(PathBuf::from(path));
            }
            "--frames" => {
                let count = args.next().context("`--frames` needs a count")?;
                frames = Some(count.parse().context("`--frames` takes a number")?);
            }
            "--menu" => overrides.menu = true,
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
            "--hover" => overrides.hover = true,
            "--run" => {
                let command = args.next().context("`--run` needs a command")?;
                overrides.run.push(command);
            }
            "--hover-block" => {
                let index = args.next().context("`--hover-block` needs an index")?;
                overrides.hover_block =
                    Some(index.parse().context("`--hover-block` takes a number")?);
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
    --snapshot <PATH>  Render one frame of the real view tree to a PNG and exit
    --frames <N>       Draw N frames, then exit; for running unattended
    --run <COMMAND>    Type COMMAND into the first pane's input field at startup,
                       send it, and report what the shell printed. Repeatable:
                       one command is one block
    --hover-block <N>  Hover the Nth finished block, so its copy control is drawn
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
    --settings [PAGE]  Start with a settings tab open, on `appearance`,
                       `shell`, `usage`, `keys` or `about`
    --search <TEXT>    Type TEXT into the settings page\'s search box, opening it
    --theme <NAME>     Start in this theme rather than the saved one
    --worktrees        Start with the active tab's worktree menu open
    --new-worktree     Start with that menu making a worktree
    --themes           Start with the Themes panel open
    --new-theme        Start with the Themes panel making a theme
    --hover            Start with the first row's detail card up
    --layout <MODE>    Start with the tabs `vertical` or `horizontal` rather than as saved
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
    cmd-t                      New agent tab
    cmd-b                      Move the tabs between the side panel and the header strip
    cmd-,                      Open the settings tab, or bring it forward
    cmd-d / cmd-shift-d        Split the focused pane to the right / downwards
    cmd-w                      Close the focused pane, and its tab with the last one
    cmd-alt-left/right         Select the previous/next tab
    cmd-ctrl-left/right        Move the active tab
    cmd-plus / cmd-minus       Make the terminal's text bigger / smaller
    cmd-0                      Put the text back to its default size

KEYS (Linux and Windows):
    ctrl-shift-t               New agent tab
    ctrl-shift-b               Move the tabs between the side panel and the header strip
    ctrl-,                     Open the settings tab, or bring it forward
    ctrl-shift-d / ctrl-shift-e  Split the focused pane to the right / downwards
    ctrl-shift-w               Close the focused pane, and its tab with the last one
    ctrl-pageup/pagedown       Select the previous/next tab
    ctrl-shift-pageup/pagedown Move the active tab
    ctrl-plus / ctrl-minus     Make the terminal's text bigger / smaller
    ctrl-0                     Put the text back to its default size

    Control-Shift, because a bare ctrl-letter belongs to the program in the
    pane: ctrl-c interrupts it, ctrl-d ends its input and ctrl-w takes back a
    word. The comma is not a letter the tty wants, which is why the settings
    chord is the one entry here that keeps a bare Control.

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
/// Five: the usage poll between readings, the git gather between cycles, the
/// caret blink between halves of its phase, the one that asks the shells
/// whether they are still alive, and the themes folder being re-read while the
/// Themes panel is open. Each is one background task for the whole cycle — the
/// wait *and* the work — so each holds its worker across the wait rather than
/// yielding it, and none is ever counted as idle. All five can be parked at
/// once: a window with shells in it, the usage chip on and the panel open is
/// an ordinary afternoon. Raise this when a sixth such chain appears.
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
/// second rather than the fifteen the usage poll could. Sizing the pool for a
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
    if let Some(query) = &overrides.search {
        workspace.type_into_settings_search(query, ctx);
    }
    if overrides.themes {
        workspace.open_theme_panel(overrides.creating, ctx);
    }
    if let Some(layout) = overrides.controls {
        workspace.override_control_layout(layout, ctx);
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
        // What the resize border has to keep out of. Answered by the module
        // that draws the buttons, so the corner the border avoids is the
        // cluster itself rather than a second opinion about where it is.
        caption_buttons: workspace::caption_area(ControlLayout::host(), WINDOW_CHROME),
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
    // unless `--run` asked for it: neither the usage poll nor the git gather is
    // started, so the frame is the same on a build machine with no Claude Code
    // session, no network and no repository — and it uses ephemeral settings,
    // so it is also the same whatever options the person running it happens to
    // have.
    let queue = LocalQueue::new();
    let mut app = App::new(queue.foreground(), background_pool());
    app.update(|ctx| ctx.add_singleton_model(UsageModel::new));

    let quit: QuitRequest = Rc::new(|| {});
    let settings = Settings::ephemeral();
    apply_startup_theme(&settings, &overrides);
    // A snapshot is always rendered as the dev channel: the only thing the
    // channel reaches is the About page's label, and a PNG that said "stable"
    // on a machine that built it from a working tree would be wrong in the one
    // way a snapshot exists to catch.
    let (window_id, workspace) = app.add_window(|ctx| {
        Workspace::new(
            fonts,
            cell_font,
            Opening {
                settings,
                channel: Channel::Dev,
                plugins: crate::plugins::defaults(),
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
    /// desktop — and two things Crook draws depend on it: the maximise control
    /// becomes a restore control, and macOS takes the traffic lights away in
    /// fullscreen, so the room reserved for them has to go too. Comparing it
    /// each frame is what turns a change nobody reported into a repaint.
    window_state: WindowState,
}

/// The real window, behind the handle the workspace holds.
///
/// The whole of the seam: four verbs forwarded to the windowing layer, which
/// is the only crate in the workspace that knows what a window is. Everything
/// above it — the header, the panel's control bar, the caption buttons — is
/// written against [`window_controls::WindowControls`] and runs unchanged with
/// nothing behind it.
struct RealWindow(PlatformWindow);

impl window_controls::WindowControls for RealWindow {
    fn state(&self) -> WindowState {
        WindowState {
            maximized: self.0.is_maximized(),
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
        app.update(|ctx| ctx.add_singleton_model(UsageModel::new));

        let proxy = platform.proxy.clone();
        let window: WindowHandle = Rc::new(RealWindow(platform.window.clone()));
        let quit: QuitRequest = {
            let proxy = proxy.clone();
            // Closing the last tab closes the window, which is what keeps the
            // strip from ever having to represent "no tabs".
            Rc::new(move || proxy.exit())
        };

        let (window_id, workspace) = app.add_window(|ctx| {
            Workspace::new(
                fonts,
                cell_font,
                Opening {
                    settings,
                    channel: launch.channel,
                    plugins: everything_installed(),
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
                workspace.start_usage_poll(ctx);
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
            "--run",
            "--type",
            "--select",
            "--select-output",
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
        // so it is the one that can quietly stop being dispatched — and since
        // the two platforms no longer share a chord, it is also the entry the
        // list can most easily go on naming after the keymap has moved.
        for (chord, key, platform) in [
            (
                "cmd-b",
                Modifiers {
                    cmd: true,
                    ..Modifiers::default()
                },
                crate::input_keys::Platform::Mac,
            ),
            (
                "ctrl-shift-b",
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Modifiers::default()
                },
                crate::input_keys::Platform::Other,
            ),
        ] {
            assert!(help.contains(chord), "{chord} is not in --help");
            assert_eq!(
                crate::input_keys::binding(&Keystroke::new("b", key), platform),
                Some(crate::input_keys::Binding::ToggleLayout),
                "--help names {chord} and nothing is bound to it"
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
