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
//!
//! # Answering from outside
//!
//! `crook pane …`, `crook tab new` and `crook events --follow` are not flags
//! and open no window: they ask a window that is already open, over the socket
//! [`control`] listens on, and print the answer.

pub mod agent;
pub mod browser;
pub mod clipboard;
pub mod completion;
pub mod control;
pub mod diagnostics;
pub mod editor;
pub mod filename;
pub mod forge;
pub mod git;
pub mod git_model;
pub mod input_keys;
pub mod keybindings;
pub mod notify;
pub mod order;
pub mod pane_blocks;
pub mod pane_find;
pub mod pane_link;
pub mod pane_selection;
pub mod pane_split;
pub mod pane_surface;
pub mod picture;
pub mod pirate;
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
pub mod update;
pub mod window_controls;
pub mod workspace;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use crook_plugin::ActionName;
use crook_terminal::BlockId;
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
use crate::workspace::{Fonts, Opening, QuitRequest, WindowAction, Workspace, WorkspaceAction};

/// The window Crook opens, in logical pixels.
const WINDOW_SIZE: Vector2F = vec2f(1024., 640.);

/// Who draws Crook's window controls.
///
/// The window is opened with the application's own chrome, so the header *is*
/// the title bar — [`open_window`] moves the window by its empty space. It
/// draws no caption buttons of its own, though: macOS's traffic lights are
/// AppKit's, painted over the corner [`platform_insets`] reserves for them,
/// and on Windows and Linux the desktop closes, minimises and maximises the
/// window the way it does any other, its own gestures and shortcuts and the
/// commands the window plugin registers — `workspace::title_bar` says why.
///
/// [`WindowChrome`] carries which of those a build gets, so the room the
/// header reserves and the controls the window actually has cannot drift
/// apart. One constant because it is one decision.
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
    /// Run these named actions before the picture is taken, in order — or,
    /// in a window, once the shells are up and any `--run` has answered.
    ///
    /// A way to look at a frame, like `--menu` and `--themes`, and the only
    /// one that reaches a *plugin's* surface: what a plugin puts up is put up
    /// by one of its own actions, and a picture of a panel nobody can open
    /// from the command line is a picture nobody can take. Run last, after the
    /// shells have settled, because what a plugin draws usually depends on
    /// what the pane has told it. In a window it is also the only way to
    /// reach a state a snapshot cannot hold — one that writes a file, say —
    /// without a hand on the keyboard.
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
    /// [`plugins::wasm::fixture`].
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
    /// Start with the Changes column open on the directory the run was
    /// started in, read then, with the first file that has lines showing
    /// them.
    ///
    /// A way to look at a frame, like `--themes`: the column is a surface, and
    /// a picture of it for the docs is a picture of the real thing — which is
    /// why it reads the real repository rather than an invented one.
    changes: bool,
    /// And press Create on it, so that what the creator says afterwards is
    /// the frame — which, against a themes folder that cannot be written,
    /// is why it was not.
    created: bool,
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
    /// Start with that menu making a task: the creator with the first agent
    /// it found picked, which is what "New task…" in the palette opens.
    creating_task: bool,
    /// Start with that menu asking about removing every free checkout.
    ///
    /// Implies `worktrees`, and is the other half of `creating_worktree`: the
    /// menu has four faces and a picture of one of them needs a way in.
    tidying_worktrees: bool,
    /// Start with that menu part way through removing them, staged.
    ///
    /// Implies `worktrees`. The one face of the menu a real run cannot hold
    /// still for a picture — it is over as soon as git is — and the one with
    /// the pirate in it, so it is staged: nothing is deleted for a screenshot.
    sweeping_worktrees: bool,
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
    /// Open the window, or draw the picture, at this size in logical pixels.
    ///
    /// A picture is drawn at 1024 by 640 whatever the session file says, so
    /// that it is the same picture on every machine — and so every picture
    /// was of one width. What the settings page does at 640 wide, or the
    /// palette at 700, was a thing nobody could look at without a window to
    /// drag. Whole pixels, because a fraction of one is not a size anybody
    /// asks for and the type is compared for equality.
    size: Option<[u32; 2]>,
    /// Run this shell in every pane rather than the user's own.
    ///
    /// The one way to a pane whose shell could not be started: the user's
    /// shell is resolved the way every terminal resolves it, and a `SHELL`
    /// naming something unrunnable falls back to the password database
    /// rather than to a pane that will not open. A path that does not exist
    /// pictures that pane; a path that does runs another shell for a look.
    shell: Option<PathBuf>,
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
    /// Open the first pane's find bar and leave this in it, so a picture of a
    /// search over the output can be taken. Needs a `--run` before it to have
    /// output to search.
    find_output: Option<String>,
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
    /// The program `-e` asked the first pane to run, and its arguments.
    ///
    /// Words rather than a line, because that is what a launcher hands over:
    /// `xdg-terminal-exec` and a desktop entry's `Terminal=true` both pass the
    /// program and each argument separately, the way xterm has always taken
    /// them. They are typed into the pane's shell as one `exec` — see
    /// [`exec_line`] — so the program starts where a person's own command
    /// would, and the pane closes when it exits the way it does when a shell
    /// does. Empty for every other window.
    command: Vec<String>,
    /// Where the window's shells start, and every tab opened after them.
    ///
    /// As `cd DIR && crook` would have started them, but for the root of a
    /// disk: that is where a Dock launch starts, so a plain `crook` in `/`
    /// swaps it for the home directory, and a root this names is kept — see
    /// [`tab::start_in`].
    ///
    /// Absolute, and a directory that was there when the command line was
    /// read: a file manager's "Open terminal here" that named a folder since
    /// deleted is a line on stderr rather than a window in the wrong place.
    working_directory: Option<PathBuf>,
    /// What the window is called where it would say "Crook".
    window_title: Option<String>,
    /// What a Linux desktop calls the window — the Wayland `app_id` and the
    /// X11 `WM_CLASS` — when it is not `crook`.
    ///
    /// For a window rule or a dock that should tell one Crook window apart
    /// from the rest, which is what `--app-id` means in every terminal a
    /// launcher knows how to ask.
    app_id: Option<String>,
}

impl Overrides {
    /// Whether this run was asked to pick something up.
    fn carries(&self) -> bool {
        self.carry.is_some() || self.carry_group.is_some()
    }

    /// Whether this run needs shells opened for it.
    ///
    /// Neither the field nor the grid is worth a picture without one: a pane
    /// with no shell draws a notice instead of both. A named shell counts,
    /// since the only reason to name one is to see what it does.
    fn wants_shells(&self) -> bool {
        self.shell.is_some()
            || !self.run.is_empty()
            || self.type_text.is_some()
            || self.select_output.is_some()
            || self.find_output.is_some()
            || self.hover_block.is_some()
            || self.block_menu.is_some()
            || self.scroll_blocks.is_some()
    }

    /// Whether a launcher opened this window rather than a person opening
    /// Crook.
    ///
    /// A window started with a command or a directory is a quick terminal
    /// somebody asked for beside their work — a file manager's "Open terminal
    /// here", a desktop entry that runs `htop` — and not the work itself. So it
    /// opens fresh rather than as the last window, and it never writes the
    /// session file: closing it must not replace the agent tabs the next
    /// ordinary launch comes back to with one pane in `~/Downloads`.
    fn launched(&self) -> bool {
        !self.command.is_empty() || self.working_directory.is_some()
    }

    /// What the window is called where it would say "Crook": the title
    /// `--title` asked for, or the channel's own name.
    ///
    /// Only that part of it. The active tab's title and the count of panes
    /// waiting still go in front — see [`window_title`] — because they are
    /// what tells two windows apart in a switcher, and a title that hid them
    /// would be a window that could no longer say an agent is waiting in it.
    fn window_name(&self, channel: Channel) -> String {
        self.window_title
            .clone()
            .unwrap_or_else(|| channel.window_title())
    }

    /// What the first pane's shell is given to run, in order: every `--run`,
    /// then the command `-e` named.
    ///
    /// Last, because the command replaces the shell, and a `--run` typed after
    /// it would be typed into the program rather than the shell.
    fn typed(&self) -> Vec<String> {
        let mut typed = self.run.clone();
        if !self.command.is_empty() {
            typed.push(exec_line(&self.command));
        }
        typed
    }

    /// Whether this run types into the focused pane's field: `--run`, which
    /// sends what it types, and `--type`, which leaves it there.
    fn types_into_the_field(&self) -> bool {
        !self.run.is_empty() || self.type_text.is_some()
    }
}

/// The line that runs `command` in place of the pane's shell.
///
/// `exec`, so that when the program exits the shell is already gone and the
/// pane closes exactly as it does when a shell exits — which is what xterm's
/// `-e` has always meant. Through the shell rather than instead of it, so the
/// program is found on the `PATH` a person's own profile built and runs with
/// the environment their own commands get; and typed into the field the way
/// `--run` types, so it is a block like any other while it runs. Every word is
/// quoted by [`shell_word`], since a launcher's arguments are words and a file
/// called `my notes.txt` is one.
///
/// With a space in front, which fish, zsh's `HIST_IGNORE_SPACE` and bash's
/// `HISTCONTROL=ignorespace` all read as "keep this out of the history". The
/// line is Crook's rather than the person's, and the history file is where
/// the field's suggestions come from — see [`crate::shell_history`] — so a
/// launcher's `btop` would otherwise come back as an `exec btop` suggestion
/// that closes whichever pane accepts it.
fn exec_line(command: &[String]) -> String {
    std::iter::once(" exec".to_owned())
        .chain(command.iter().map(|word| shell_word(word)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One word, spelled so that sh, bash, zsh and fish all read it back as
/// exactly itself.
///
/// Single quotes, which all four take literally, with two exceptions that are
/// the whole difficulty: none of them can hold a `'`, and fish alone reads
/// `\\` and `\'` inside them as escapes. So both characters go *outside* the
/// quotes, backslashed, which each of the four reads the same way — `it's`
/// is `'it'\''s'` and `a\b` is `'a'\\'b'`. A word made only of characters no
/// shell treats specially is left bare, so the line in the pane reads
/// `exec htop` rather than `exec 'htop'`; a leading `=` is not one of them,
/// since zsh reads `=cat` as the path to `cat`.
fn shell_word(word: &str) -> String {
    let bare = !word.is_empty()
        && !word.starts_with('=')
        && word
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_./:,+@=".contains(character));
    if bare {
        return word.to_owned();
    }
    // Still a word, and the loop below would spell it as nothing at all.
    if word.is_empty() {
        return "''".to_owned();
    }

    let mut spelled = String::with_capacity(word.len() + 2);
    let mut quoted = false;
    for character in word.chars() {
        if matches!(character, '\'' | '\\') {
            if quoted {
                spelled.push('\'');
                quoted = false;
            }
            spelled.push('\\');
        } else if !quoted {
            spelled.push('\'');
            quoted = true;
        }
        spelled.push(character);
    }
    if quoted {
        spelled.push('\'');
    }
    spelled
}

/// What a window opens with: the tabs the last one had, or none.
///
/// None for a window a launcher opened — see [`Overrides::launched`] — and
/// none when the setting is off. Read from beside the settings file, which is
/// where the window writes it, so a window whose settings are somewhere else
/// reads and writes one file rather than reading one and writing another.
fn opening_session(settings: &Settings, overrides: &Overrides) -> crate::session::Session {
    if overrides.launched() || !settings.general().restore_session {
        return crate::session::Session::default();
    }
    settings
        .path()
        .map(|path| crate::session::Session::load(crate::session::session_path_beside(path)))
        .unwrap_or_default()
}

/// Runs Crook.
///
/// Returns when the window closes, or immediately for `--help`, `--version`
/// and `--snapshot`.
pub fn run(channel: Channel) -> Result<()> {
    attach_to_parent_console();
    diagnostics::log_file::init();

    match parse_args(
        channel,
        command_line(std::env::args_os().skip(1))?.into_iter(),
    )? {
        Startup::Answered => Ok(()),
        Startup::Snapshot { path, overrides } => write_snapshot(&path, overrides),
        Startup::Window { frames, overrides } => {
            // First, before anything that could fail or panic on the way to a
            // window: the fonts, the GPU. Those are the failures a launch from
            // the Dock or a `.desktop` entry has nowhere else to put.
            let diagnostics = diagnostics::start(channel);
            open_window(channel, frames, overrides, diagnostics).inspect_err(|error| {
                // The binary prints it on stderr as it ends the process; this is
                // the copy for a person who was never looking at a stderr.
                diagnostics::log_file::note(&format!("Crook could not start: {error:#}"));
            })
        }
    }
}

/// The command line as text, or the word in it that is not.
///
/// `std::env::args` panics on a word that is not UTF-8, and on Linux a path
/// is bytes rather than text: a file manager's "Open terminal here" on a
/// folder named in Latin-1 would be a crash and no window. Every word Crook
/// reads is text — `-e`'s are typed into a field, a key at a time — so such a
/// word is a line on stderr that names it, like any other argument Crook
/// cannot use.
fn command_line(words: impl Iterator<Item = std::ffi::OsString>) -> Result<Vec<String>> {
    words
        .map(|word| {
            word.into_string().map_err(|word| {
                anyhow!("{word:?} is not UTF-8, and Crook reads its arguments as text")
            })
        })
        .collect()
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

/// What follows `--agent`, as the command line spelled it.
#[derive(Debug, PartialEq, Eq)]
struct AgentArguments {
    /// The status word, checked by [`agent::report`] rather than here.
    status: String,
    /// `--title`, when given.
    title: Option<String>,
    /// `--message`, when given.
    message: Option<String>,
    /// `--pull-request`, when given.
    pull_request: Option<String>,
}

/// What follows `--agent`: the status, and a title, a message and a pull
/// request if given.
///
/// Apart from [`parse_args`] so that what it accepts can be tested without
/// sending a report to whatever terminal the test runs in.
fn agent_arguments(
    args: &mut std::iter::Peekable<impl Iterator<Item = String>>,
) -> Result<AgentArguments> {
    let status = args
        .next()
        .context("`--agent` needs a status: idle, running, needs-input or failed")?;
    let mut title = None;
    let mut message = None;
    let mut pull_request = None;
    // In any order, each at most once: a hook that names the
    // work and says what it is waiting for spells both.
    while let Some(flag) = args.peek().map(String::as_str) {
        let (field, needs) = match flag {
            "--title" => (&mut title, "`--title` needs the title"),
            "--message" => (
                &mut message,
                "`--message` needs the message, or `-` to read it from stdin",
            ),
            "--pull-request" => (
                &mut pull_request,
                "`--pull-request` needs the pull request's address, or `-` to read a hook's \
                 input from stdin",
            ),
            _ => break,
        };
        if field.is_some() {
            bail!("`{flag}` was given twice");
        }
        args.next();
        *field = Some(args.next().context(needs)?);
    }
    // A flag left over is one nothing here reads — most often a
    // `--mesage` a hook misspelled, which would otherwise report
    // needs-input with no question and exit as though it had.
    // Refused before anything reaches the terminal. A word that is
    // not a flag is let through as it always was: Codex's older
    // `notify` runs the program it names with a JSON payload
    // appended, and `--agent idle` there has to keep working.
    if let Some(extra) = args.peek().filter(|word| word.starts_with("--")) {
        bail!(
            "unrecognised argument {extra}; `--agent <status>` takes only \
             --title, --message and --pull-request"
        );
    }
    Ok(AgentArguments {
        status,
        title,
        message,
        pull_request,
    })
}

fn parse_args(channel: Channel, args: impl Iterator<Item = String>) -> Result<Startup> {
    let mut args = args.peekable();
    // A noun and a verb rather than a flag, and only as the first word: it is
    // a question for a window that is already open, and the rest of the line
    // is its own — `--json` after it means what `pane list` says it means,
    // and everything after `tab new`'s `--` is the command it runs.
    if args.next_if(|word| word == "pane").is_some() {
        let printed = control::cli::pane(args)?;
        println!("{}", printed.text);
        // Printed first: a wait that did not get there still says where the
        // pane is, and the failure is what a script's `&&` reads.
        if let Some(failure) = printed.failure {
            bail!(failure);
        }
        return Ok(Startup::Answered);
    }
    if args.next_if(|word| word == "events").is_some() {
        control::cli::events(args)?;
        return Ok(Startup::Answered);
    }
    if args.next_if(|word| word == "tab").is_some() {
        println!("{}", control::cli::tab(args)?);
        return Ok(Startup::Answered);
    }
    let mut frames = None;
    let mut snapshot = None;
    let mut plugins = false;
    let mut json = false;
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
                if let Some(extra) = args.next() {
                    bail!("unrecognised argument {extra}; `--shell-integration` takes one shell");
                }
                println!("{}", shell_integration_text(shell.as_deref())?);
                return Ok(Startup::Answered);
            }
            // Answered without a window, like `--version`: the window this is
            // about is already open, and this process is a program inside it
            // saying one thing to it.
            "--agent" => {
                // A `--title` in front of the status is the window's as far
                // as the loop could tell, and almost always the report's put
                // in the wrong place — which it was refused as before the
                // window had a title of its own to take.
                if overrides.window_title.is_some() {
                    bail!("`--title` goes after `--agent <status>`");
                }
                let arguments = agent_arguments(&mut args)?;
                agent::report(
                    &arguments.status,
                    arguments.title.as_deref(),
                    arguments.message.as_deref(),
                    arguments.pull_request.as_deref(),
                )?;
                return Ok(Startup::Answered);
            }
            "--title" => {
                let title = args.next().context("`--title` needs the window's title")?;
                overrides.window_title = Some(title);
            }
            "--message" => bail!("`--message` goes after `--agent <status>`"),
            // Everything after it is the command, the way xterm has always
            // read `-e`: a launcher hands over a program and its arguments,
            // and an argument spelled like one of Crook's flags is the
            // program's all the same.
            "-e" | "--" => overrides.command = launched_command(&argument, args.by_ref())?,
            "--working-directory" | "--cwd" => {
                let named = args
                    .next()
                    .with_context(|| format!("`{argument}` needs a directory"))?;
                overrides.working_directory = Some(working_directory(&named)?);
            }
            "--app-id" => {
                let id = args.next().context("`--app-id` needs an id")?;
                if id.trim().is_empty() {
                    bail!("`--app-id` needs an id, and an empty one is none");
                }
                overrides.app_id = Some(id);
            }
            "--pull-request" => bail!("`--pull-request` goes after `--agent <status>`"),
            // Stdout is the fragment and stderr the lead and the note, so
            // `> hooks.json` takes exactly the fragment; an agent with no
            // hooks gets a sentence on stdout and no note, since the sentence
            // is the whole answer. The lead goes out first, because the
            // plugin it names is the way that needs no merging.
            "--agent-hooks" => {
                let agent = args.next().with_context(|| {
                    format!(
                        "`--agent-hooks` needs the agent to write hooks for: {}",
                        agent::names_listed()
                    )
                })?;
                let binary =
                    std::env::current_exe().context("could not find this binary's own path")?;
                let hooks = agent::hooks_text(&agent, &binary)?;
                if let Some(lead) = hooks.lead {
                    eprintln!("{lead}");
                }
                println!("{}", hooks.text);
                if let Some(note) = hooks.note {
                    eprintln!("{note}");
                }
                return Ok(Startup::Answered);
            }
            // The other thing printed for a person to put somewhere: the
            // hooks make an agent report without knowing it is, and this
            // tells one that was asked to know. Stdout is the file and
            // stderr the note, so `> SKILL.md` takes exactly the file.
            "--skill" => {
                print!("{}", agent::SKILL);
                eprintln!(
                    "# Save the text above as ~/.claude/skills/crook/SKILL.md, or as a \
project's .claude/skills/crook/SKILL.md. Claude Code then knows what a pane can do."
                );
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
                // No context naming the plugin, unlike the path on
                // `--install-plugin`: both refusals already name it — `"x" is
                // not `owner/name``, `x is not installed` — and a line that
                // began `x: x is not installed` said it twice.
                let id = crook_plugin::PluginId::parse(&name).map_err(anyhow::Error::msg)?;
                let removed = crate::plugins::wasm::uninstall(&id).map_err(anyhow::Error::msg)?;
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
            // Every one of these reaches the network, and every one of them
            // is somebody asking it to: see `update`'s first paragraph and
            // `store::fetch`'s. None of them opens a window — updating and
            // then drawing would be a window whose plugins were replaced
            // under it while it built.
            "--update-plugin" => {
                let name = args
                    .next()
                    .context("`--update-plugin` needs a plugin's `owner/name`")?;
                let id = crook_plugin::PluginId::parse(&name).map_err(anyhow::Error::msg)?;
                print!("{}", plugins_updated(Some(&id))?);
                return Ok(Startup::Answered);
            }
            "--update-plugins" => {
                print!("{}", plugins_updated(None)?);
                return Ok(Startup::Answered);
            }
            "--check-update" => {
                println!("{}", update_checked(channel)?);
                return Ok(Startup::Answered);
            }
            "--update" => {
                print!("{}", updated(channel)?);
                return Ok(Startup::Answered);
            }
            // Answered after the loop rather than here, unlike `--version`:
            // `--json` may come before it or after it, and the answer is not
            // known until both have been read.
            "--plugins" => plugins = true,
            "--json" => json = true,
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
            "--changes" => overrides.changes = true,
            "--worktrees" => overrides.worktrees = true,
            "--new-worktree" => {
                overrides.worktrees = true;
                overrides.creating_worktree = true;
            }
            "--new-task" => {
                overrides.worktrees = true;
                overrides.creating_task = true;
            }
            "--tidy-worktrees" => {
                overrides.worktrees = true;
                overrides.tidying_worktrees = true;
            }
            "--sweep-worktrees" => {
                overrides.worktrees = true;
                overrides.sweeping_worktrees = true;
            }
            "--new-theme" => {
                overrides.themes = true;
                overrides.creating = true;
            }
            "--create-theme" => {
                overrides.themes = true;
                overrides.creating = true;
                overrides.created = true;
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
                // Any flag, short as well as long: `-e`, `-h` and `-V` are
                // flags of this command line, and `--settings -e htop` read
                // `-e` as a page and then refused `htop`. No page's title
                // starts with a dash.
                let page = match args.peek().map(String::as_str) {
                    Some(other) if !other.starts_with('-') => Some(other.to_owned()),
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
            "--find-output" => {
                overrides.find_output = Some(
                    args.next()
                        .context("`--find-output` needs the text to search for")?,
                );
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
            "--size" => {
                let size = args.next().context("`--size` needs a width and a height")?;
                overrides.size = Some(parse_size(&size)?);
            }
            "--shell" => {
                let shell = args.next().context("`--shell` needs a path")?;
                overrides.shell = Some(PathBuf::from(shell));
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

    // Refused rather than ignored: a script that wrote `--json` and got a
    // window, or columns, would parse what it was not asked for and never
    // learn why. It is an argument error, on the same exit status as an
    // argument nobody has heard of.
    if json && !plugins {
        bail!("`--json` goes with `--plugins`; try --help");
    }
    if plugins {
        match json {
            true => println!("{}", installed_plugins_json()),
            false => println!("{}", installed_plugins_text()),
        }
        return Ok(Startup::Answered);
    }

    // `--search` with no section named is the settings page's box, and it
    // opens the page: a query with no list to filter is nothing to look at.
    // With a section named it is that section's box — the Store's, the
    // Plugins page's — which is why the implication is decided here, once
    // both flags have been read, rather than in the arm for either.
    if overrides.search.is_some() && overrides.section.is_none() && overrides.settings.is_none() {
        overrides.settings = Some(None);
    }

    // A picture has no launcher. Both of these are about how a window starts
    // and what it leaves behind, and a snapshot that quietly drew its seeded
    // tabs instead would be one that did not do what it was asked.
    if snapshot.is_some() {
        if !overrides.command.is_empty() {
            bail!("`-e` opens a window, and `--snapshot` draws a picture instead; ask for one");
        }
        if overrides.working_directory.is_some() {
            bail!(
                "`--working-directory` opens a window, and `--snapshot` draws a picture \
                 instead; ask for one"
            );
        }
    }

    // The window moves the whole process into the directory — see
    // `open_window` — and a relative path typed beside the flag was typed
    // relative to where Crook was started, so it is made whole while that is
    // still where it is running.
    if overrides.working_directory.is_some() {
        for path in [overrides.dev_plugin.as_mut(), overrides.fixture.as_mut()]
            .into_iter()
            .flatten()
        {
            *path = std::path::absolute(&*path)
                .with_context(|| format!("{} cannot be made absolute", path.display()))?;
        }
    }
    // The shell always, and not only beside `--working-directory`: every pane
    // starts it from its *own* directory, so `./bin/zsh` named a different
    // file in a restored tab or one opened where another pane was working,
    // and none at all in most of them. A shell named with no directory in it
    // is a `PATH` lookup, and stays one.
    if let Some(shell) = overrides
        .shell
        .as_mut()
        .filter(|shell| shell.components().count() > 1)
    {
        *shell = std::path::absolute(&*shell)
            .with_context(|| format!("{} cannot be made absolute", shell.display()))?;
    }

    match snapshot {
        Some(path) => Ok(Startup::Snapshot { path, overrides }),
        None => Ok(Startup::Window { frames, overrides }),
    }
}

/// The words `-e` hands the first pane, or why they cannot be typed there.
///
/// Refused on Windows, whose shells have no `exec` to type — see
/// [`exec_line`] — and for a word with a control character in it: the command
/// is typed into the pane a key at a time, where a line break is Return and a
/// tab asks for a completion, so `printf 'a<newline>b'` would be sent as half
/// a quote with the shell left waiting for the rest. And for a line longer
/// than a terminal holds before its shell is reading: see
/// [`TYPED_LINE_LIMIT`].
fn launched_command(flag: &str, words: impl Iterator<Item = String>) -> Result<Vec<String>> {
    if cfg!(windows) {
        bail!(
            "`{flag}` types an `exec` into the pane's shell, and no Windows shell has one; \
             open Crook and run the command there"
        );
    }
    let command: Vec<String> = words.collect();
    if command.is_empty() {
        bail!("`{flag}` needs a program to run, with its arguments after it");
    }
    if let Some(word) = command
        .iter()
        .find(|word| word.chars().any(char::is_control))
    {
        bail!("`{flag}` cannot type {word:?} into a shell: it holds a control character");
    }
    let typed = exec_line(&command).len();
    if typed > TYPED_LINE_LIMIT {
        bail!(
            "`{flag}` would type a line of {typed} bytes, and a terminal keeps at most \
             {TYPED_LINE_LIMIT} bytes of a line typed before its shell is reading; name fewer \
             or shorter arguments"
        );
    }
    Ok(command)
}

/// The longest line `-e` can type, in bytes.
///
/// The line is typed on the frame after the shell starts, before the shell is
/// reading, so it waits in the pty's line discipline — which keeps at most
/// this much of one line, drops the rest and still takes the Return. What
/// runs is then a command whose last argument was cut short, or a quote the
/// shell waits to see closed. Linux keeps 4095 bytes, measured against a
/// shell that had not started reading; macOS's `MAX_CANON` is 1024, and a
/// round number under it leaves room for the few bytes its line discipline
/// holds back.
const TYPED_LINE_LIMIT: usize = if cfg!(target_os = "linux") {
    4095
} else {
    1000
};

/// The directory `--working-directory` named, made absolute, when it is one.
///
/// Refused here rather than when the window opens, so the reason is a line
/// where the flag was typed — and not a pane that quietly started in `$HOME`,
/// which is what the pty does with a directory that is not there.
fn working_directory(named: &str) -> Result<PathBuf> {
    let path = std::path::absolute(named)
        .with_context(|| format!("{named:?} is not a directory Crook can start in"))?;
    if !path.is_dir() {
        bail!(
            "{} is not a directory; `--working-directory` needs one that is there",
            path.display()
        );
    }
    Ok(path)
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
        "`--shell-integration` needs a shell: zsh, bash or fish. Paste what it prints at the \
         end of that shell's own configuration on a machine Crook cannot start the shell on \
         itself — over ssh, in a container.",
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

/// `WxH`, in whole logical pixels, each at least one.
///
/// One spelling, the one every image tool uses, rather than a comma or a
/// space as well: a flag that accepts three notations documents three.
fn parse_size(text: &str) -> Result<[u32; 2]> {
    let complaint = || format!("`--size` takes a width and a height like 640x480, not {text:?}");
    let (width, height) = text.split_once('x').with_context(complaint)?;
    let width: u32 = width.trim().parse().with_context(complaint)?;
    let height: u32 = height.trim().parse().with_context(complaint)?;
    if width == 0 || height == 0 {
        bail!("`--size` needs a width and a height above zero, not {text:?}");
    }
    Ok([width, height])
}

fn help_text() -> String {
    format!(
        "crook {version} \u{2014} a terminal whose unit of work is an agent

USAGE:
    crook [OPTIONS]
    crook [OPTIONS] -e <PROGRAM> [ARGS]...
    crook pane list [--json]
    crook pane wait <ID> --until <STATE> [--timeout <SECS>] [--json]
    crook pane blocks <ID> [--last <N>] [--json]
    crook tab new [--worktree <BRANCH>] [--in-my-group] [--title <TITLE>] [--json] -- <COMMAND>...
    crook events --follow [--pane <ID>]

COMMANDS:
    pane list [--json] Ask the window this is run in which panes it has, over
                       the local socket CROOK_SOCKET names, and print them: the
                       number each pane's shell has in CROOK_PANE_ID (the
                       focused one marked *), what its agent is doing, its
                       title, group, branch and directory. --json prints the
                       window's own JSON array instead. Outside a pane it asks
                       the one Crook running, and refuses to pick among
                       several. Not on Windows yet
    pane wait <ID> --until <STATE> [--timeout <SECS>]
                       Wait until pane ID's agent is idle or needs-input, its
                       command has finished, or it has exited, and print where
                       it got to; fail, having printed where it is, when the
                       timeout (at most and by default 3600 seconds) passes
                       first. `finished` is the last command closed by the
                       shell's own mark, with its exit status; a tab that has
                       already closed answers `exited`. --json prints the
                       window's answer. Only your own pane and the tabs it
                       opened with `tab new`; not on Windows yet
    pane blocks <ID> [--last <N>]
                       Print pane ID's newest finished commands: the command,
                       what it printed (the end of it, when it is long), its
                       exit status, how long it ran and where. A pane whose
                       first command is still running, or that is drawing a
                       full-screen program or an agent's TUI, says so rather
                       than printing nothing. --json prints the window's
                       answer. Your own pane and the tabs it opened; not on
                       Windows yet
    tab new [...] -- <COMMAND>...
                       From inside a pane, open a tab beside it in the same
                       window, without switching to it, and run the command
                       there at the new shell's first prompt, every word of it
                       as itself. --worktree makes a worktree on a new branch
                       from this pane's HEAD and opens the tab in it;
                       --in-my-group puts the tab in this tab's group; --title
                       names it. Prints the new pane's number, or with --json
                       the window's answer. The pane is known by the secret in
                       its CROOK_TOKEN, and eight tabs may be open on behalf of
                       one pane a person opened. sh, bash, zsh and fish only;
                       not on Windows yet
    events --follow [--pane <ID>]
                       Print a line of JSON for every change of status, every
                       command finished (with its command line), every command
                       seen running and every pane opened or closed, in your
                       own pane and the tabs it opened (or only pane ID), until
                       the window ends it. A reader that falls behind is sent
                       a `lagged` line counting what it missed. Not on Windows
                       yet

OPTIONS:
    -e <PROGRAM> [ARGS]...
                       Open a window whose one pane runs PROGRAM with these
                       arguments in place of your shell, and closes when it
                       exits. Everything after -e is the command, as in xterm;
                       `crook -- PROGRAM ARGS` says the same. Not on Windows
    --working-directory <DIR>
                       Open a window whose shells start in DIR, and so do the
                       tabs opened after them; `--cwd` is the same. DIR can be
                       `/`, which `cd / && crook` swaps for your home. A window
                       opened with this or with -e is a launcher's: it neither
                       comes back as the last window nor replaces it
    --title <TITLE>    Call the window TITLE where it would say Crook; the
                       active tab's name still goes in front of it
    --app-id <ID>      Open the window with this Wayland app_id and X11
                       WM_CLASS rather than `crook`, for a window rule or a
                       dock that should tell it apart. Linux only
    --install-plugin <PATH>
                       Copy a plugin's `.wasm` into the plugins directory and exit,
                       after checking it is one. What it may then do is nothing
                       until you allow it on the Plugins page
    --uninstall-plugin <ID>
                       Remove an installed plugin by `owner/name`, with whatever
                       it was allowed to do, and exit
    --plugins          List the plugins installed as files: name, version and
                       which file each is running from, and what the registry's
                       copy on this machine says about that version
    --json             With --plugins, print the list as a JSON array instead,
                       one object per plugin — `id`, `version`, `path`,
                       `enabled` and `allowed` — for a script or an agent
    --update-plugin <ID>
                       Fetch the registry's newest build of an installed plugin
                       and install it over the version that is there
    --update-plugins   The same for every installed plugin the registry is
                       ahead of, one at a time, and say what each one did
    --check-update     Ask the releases page whether a newer Crook is out and
                       say so. Nothing is asked of the network until you run
                       this, or --update, or open the store
    --update           The same, and then install that release over this
                       binary: the archive for this platform, checked against
                       the published SHA256SUMS, renamed into place. Restart
                       Crook to run it
    --dev-plugin <PATH>
                       Run the plugin you are writing, from wherever you built
                       it, and run it again every time you build it. Takes a
                       `.wasm` or the directory holding one. Nothing is
                       installed and nothing is left behind
    --plugin-fixture <PATH>
                       Draw a fixed plugin surface, read from a JSON file of
                       slot names to the shapes a plugin describes, so a
                       picture of what a plugin puts on screen is the same on
                       every machine. A top-level `icon` names a PNG beside
                       the file, for a picture of the Plugins page with a face
                       on a row
    --snapshot <PATH>  Render one frame of the real view tree to a PNG and exit
    --frames <N>       Draw N frames, then exit; for running unattended
    --run <COMMAND>    Type COMMAND into the first pane's input field at startup,
                       send it, and report what the shell printed. Repeatable:
                       one command is one block
    --find-output <TEXT>
                       Open the first pane's find bar over its output with TEXT
                       in it; needs a `--run` before it to have output to search
    --hover-block <N>  Hover finished block N — the first is 0 — so its
                       controls are drawn
    --block-menu <N>   Open the menu on finished block N, the first being 0
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
    --settings [PAGE]  Start with a settings tab open, on the page with this
                       title: `appearance`, `shell`, `keyboard shortcuts` (quoted)
                       or `about`, or a plugin\'s own
    --find <TEXT>      Type TEXT into the tabs panel\'s search box, filtering the list
    --search <TEXT>    Type TEXT into the settings page\'s search box, opening it;
                       with --section, into that section\'s box instead — the
                       Store\'s registry search, the Plugins page\'s
    --record <COMMAND> Start with the Keyboard Shortcuts page recording a chord for
                       COMMAND, which is an action name like `crook/window/new-tab`
    --theme <NAME>     Start in this theme rather than the saved one
    --worktrees        Start with the active tab's worktree menu open
    --new-worktree     Start with that menu making a worktree
    --new-task         Start with that menu making a task, the first agent it
                       found picked
    --tidy-worktrees   Start with that menu asking about removing every checkout
                       nothing is working in
    --sweep-worktrees  Start with that menu part way through removing them, staged:
                       the pirate is drawn and nothing is deleted
    --themes           Start with the Themes panel open
    --changes          Start with the Changes column open on the repository
                       Crook was started in, with the first file that has
                       lines to show showing them
    --new-theme        Start with the Themes panel making a theme
    --create-theme     Start with that theme's Create pressed: a folder that can
                       be written to closes the creator, one that cannot leaves
                       it up saying why
    --hover            Start with the first row's detail card up
    --action <name [argument]>
                       Run this named action before the picture is taken, so a
                       plugin's own panel can be looked at. Anything after the
                       name is what the action is told — what a picker's row or
                       a menu's entry would have said. Repeatable. In a window
                       it runs once the shells are up and any --run has answered
    --with-plugins     Load the plugins this machine has installed, so that a
                       snapshot shows what they draw. Off by default: a picture
                       of the application is the same everywhere and one of a
                       plugin is not
    --carry <N>        Pick up row N of the tabs panel — the first is 0 — and
                       hold it there, for a picture of a drag in flight
    --carry-group <N>  The same for group N's whole block, the first being 0,
                       by its heading
    --carry-by <PX>    How far down the column to carry it; negative carries up
    --section <NAME>   Start showing a sidebar section by the name on its button
    --granularity <M>  Start with rows standing for `panes` or `tabs` rather than as saved
    --density <MODE>   Start in `compact` or `expanded` density rather than the saved one
    --controls <OS>    Draw `macos`, `windows` or `linux` window controls in the
                       header rather than this platform's, for a picture of the
                       title bar the other two get
    --size <WxH>       Open the window, or draw the picture, this many logical
                       pixels wide and high — `640x480` — rather than 1024x640,
                       for a look at what the layout does in a small window
    --shell <PATH>     Run this shell in every pane rather than your own; a
                       path that does not exist is the one way to a picture of
                       a pane whose shell could not be started
    --shell-integration <SHELL>
                       Print the OSC 133 snippet for `zsh`, `bash` or `fish`,
                       to paste into that shell\'s own configuration on a machine
                       Crook cannot start the shell on — over ssh, in a container
    --agent <STATUS> [--title <TEXT>] [--message <TEXT>] [--pull-request <URL>]
                       Tell the pane this is run in what the program in it is
                       doing: `idle`, `running`, `needs-input` or `failed`,
                       what it calls its work, with `needs-input` what it is
                       waiting for, and the https address of the pull request
                       it opened, which the row links to. Written to the
                       terminal, so it works from a hook, over ssh and in a
                       container; `--title -` takes the prompt out of a Claude
                       Code hook\'s input on stdin, `--message -` the
                       notification\'s text, and `--pull-request -` the address
                       a `gh pr create` printed
    --agent-hooks <AGENT>
                       Print the hooks that make an agent say all of that by
                       itself, to merge into its settings: `claude` (Claude
                       Code), `codex`, `gemini`, `copilot` or `opencode`;
                       `aider` has none, and this says what to do instead.
                       `claude` leads with the two commands that install the
                       same hooks as Crook's Claude Code plugin
    --skill            Print the skill file that teaches a coding agent what it
                       can do from inside a pane, to save as SKILL.md where
                       the agent loads its skills
    -h, --help         Print this message
    -V, --version      Print the version and channel

EXIT STATUS:
    0 when what was asked for was done: a window that was closed, a snapshot
    that was written, a list that was printed, a plugin that was installed.
    1 for everything else — an argument the parser refuses, a plugin that
    could not be installed or removed, a window that could not be opened —
    with the reason on stderr, after `crook: `. There is no third code.

KEYS (macOS):
    cmd+t                      New agent tab
    shift+cmd+t                Reopen the tab closed last, where it was
    cmd+,                      Show the settings
    shift+cmd+p                Open the command palette
    cmd+d / shift+cmd+d        Split the focused pane to the right / downwards
    cmd+w                      Close the focused pane, and its tab with the last one
    ctrl+shift+arrows          Focus the pane in that direction
    cmd+] / cmd+[              Focus the next / previous pane
    shift+cmd+m                Zoom the focused pane: the whole tab, and the split back
    cmd+b                      Hide the tabs panel, and bring it back
    cmd+k                      Search the tabs
    cmd+f                      Find in the output
    cmd+1 … cmd+8, cmd+9       Select a tab by position, and the last one
    alt+cmd+left/right         Select the previous/next tab
    ctrl+tab / ctrl+shift+tab  The same step, on the chord browsers use
    ctrl+cmd+left/right        Move the active tab
    alt+cmd+up / alt+cmd+down  Select the block above / below, and copy it with cmd+c
    shift+pageup/pagedown      Page the output
    shift+cmd+pageup/pagedown  Scroll it to its top / bottom
    cmd+plus / cmd+minus       Make the terminal's text bigger / smaller
    cmd+0                      Put the text back to its default size

KEYS (Linux and Windows):
    ctrl+shift+t               New agent tab
    ctrl+shift+alt+t           Reopen the tab closed last, where it was
    ctrl+,                     Show the settings
    ctrl+shift+p               Open the command palette
    ctrl+shift+d / ctrl+shift+e  Split the focused pane to the right / downwards
    ctrl+shift+w               Close the focused pane, and its tab with the last one
    alt+arrows                 Focus the pane in that direction
    ctrl+shift+] / ctrl+shift+[  Focus the next / previous pane
    ctrl+shift+m               Zoom the focused pane: the whole tab, and the split back
    ctrl+shift+b               Hide the tabs panel, and bring it back
    ctrl+shift+k               Search the tabs
    ctrl+shift+f               Find in the output
    alt+1 … alt+8, alt+9       Select a tab by position, and the last one
    ctrl+pageup/pagedown       Select the previous/next tab
    ctrl+tab / ctrl+shift+tab  The same step, on the chord browsers use
    ctrl+shift+pageup/pagedown Move the active tab
    ctrl+alt+up / ctrl+alt+down  Select the block above / below, and copy it with ctrl+shift+c
    shift+pageup/pagedown      Page the output
    alt+home / alt+end         Scroll it to its top / bottom
    ctrl+plus / ctrl+minus     Make the terminal's text bigger / smaller
    ctrl+0                     Put the text back to its default size

    Control-Shift, because a bare ctrl-letter belongs to the program in the
    pane: ctrl-c interrupts it, ctrl-d ends its input and ctrl-w takes back a
    word. Neither the comma nor Tab is a letter the tty wants — Tab is already
    a control code, so ctrl-Tab has never had a spelling a terminal could send
    — which is why those two are the entries here that keep a bare Control.

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
/// [`plugins::wasm`] — so a person with three installed
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
/// for as long as a window is open. "Update all" does not change that: the
/// downloads it asks for are a queue the model drains one task at a time,
/// each completion starting the next, so a queued update is still one task
/// and never a worker per plugin.
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
/// pane count is not possible; keeping the wait short is. The worktree menu's
/// bite — `Workspace::keep_chomping`, which moves the pirate while that menu
/// waits on git — is the same shape and is left out for the same reason: one
/// chain at most, a frame of `pirate::FRAME` at a time, and only while a git
/// command is running for the menu.
///
/// The control socket parks nothing here either. Its listener blocks in
/// `accept` for as long as nobody connects, which is a thread's job and not a
/// worker's — the pty reader's argument — so it and each connection it
/// accepts are threads of their own, and the window's side of a question is a
/// foreground task woken by one of them. See [`control`].
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

/// Opens a window's strip on what the session file describes, and then on
/// what the command line asked for on top of it.
///
/// One function rather than three steps in [`Shell::new`] because the order
/// is the point, and a test can hold a function to it:
///
/// - The restore first. The overrides open a settings page in whatever strip
///   is there, and a session that describes nothing leaves the strip a fresh
///   window's, which is what every launch had before there was a file to
///   read.
/// - The overrides next. See [`apply_overrides`].
/// - Last, for `--run` and `--type`, the focused pane's resume line taken back
///   out of its field. Both type into that field as though it were empty, and
///   a restore may have left an agent's resume line there: typed after it,
///   `--run 'git status'` would send `claude --continuegit status`. After the
///   overrides, because the pane they type into is the one focused once those
///   have run; after the restore, because until then there is no line.
fn restore_and_override(
    workspace: &mut Workspace,
    session: &crate::session::Session,
    overrides: &Overrides,
    ctx: &mut ViewContext<Workspace>,
) {
    if let Some(strip) = session.restore() {
        workspace.restore(strip, ctx);
    }
    apply_overrides(workspace, overrides, ctx);
    if overrides.types_into_the_field()
        && let Some(pane) = workspace.tabs().focused_pane_id()
    {
        workspace.withdraw_resume_offer(pane, ctx);
    }
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
    if let Some(shell) = overrides.shell.clone() {
        workspace.set_shell(Some(shell), ctx);
    }
    // Before anything below can change the strip, since every change to it
    // is a save. See `Overrides::launched`.
    if overrides.launched() {
        workspace.stop_saving_the_session();
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
        match overrides.section.is_some() {
            true => workspace.type_into_section_search(query, ctx),
            false => workspace.type_into_settings_search(query, ctx),
        }
    }
    // After `--section`, so that a run asking for both ends where the box is:
    // typing into it shows the tabs, whichever section was named.
    if let Some(query) = &overrides.find {
        workspace.type_into_panel_search(query, ctx);
    }
    if overrides.themes {
        workspace.open_theme_panel(overrides.creating, ctx);
        if overrides.created {
            workspace.create_theme(ctx);
        }
    }
    if overrides.changes {
        workspace.open_changes_for_snapshot(ctx);
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
        if overrides.creating_task {
            workspace.start_new_task(ctx);
        }
        if overrides.tidying_worktrees {
            workspace.start_tidying_worktrees(ctx);
        }
        if overrides.sweeping_worktrees {
            workspace.stage_sweeping_worktrees(ctx);
        }
    }
}

fn open_window(
    channel: Channel,
    frames: Option<u32>,
    overrides: Overrides,
    diagnostics: Option<diagnostics::Diagnostics>,
) -> Result<()> {
    let launch = Launch {
        channel,
        frames,
        overrides,
        diagnostics,
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
    // The process rather than the first pane: every pane starts where Crook
    // is — see `tab::starting_directory` — so the tab a person opens next in
    // a window a file manager opened on a folder opens in that folder too.
    // The paths the other flags named were made absolute when they were
    // read. And named, so that a root directory is where the panes start too,
    // rather than the home directory a Dock-launched Crook — or a plain
    // `cd / && crook` — swaps it for.
    if let Some(directory) = &launch.overrides.working_directory {
        crate::tab::start_in(directory)
            .with_context(|| format!("could not start in {}", directory.display()))?;
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
    // size it names. An empty one — a first run, an unreadable file, the
    // setting turned off, or a window a launcher opened — is the default
    // window and one tab, which is exactly what every launch did before this
    // file existed.
    let session = opening_session(&settings, &launch.overrides);
    let options = window_options(channel, &launch.overrides, &session);

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

/// How the window is opened: its name, what a Linux desktop calls it, its
/// size and who draws its frame.
fn window_options(
    channel: Channel,
    overrides: &Overrides,
    session: &crate::session::Session,
) -> WindowOptions {
    let defaults = WindowOptions::default();
    // The flag first, the session's size second, the default last: a
    // person asking for a window of a size is asking to look at that size,
    // whatever the last window was.
    WindowOptions {
        title: overrides.window_name(channel),
        app_id: overrides.app_id.clone().unwrap_or(defaults.app_id),
        size: overrides
            .size
            .map(|[width, height]| vec2f(width as f32, height as f32))
            .or_else(|| {
                session
                    .window_size()
                    .map(|[width, height]| vec2f(width, height))
            })
            .unwrap_or(WINDOW_SIZE),
        chrome: WINDOW_CHROME,
        ..defaults
    }
}

/// How long quitting waits for git to take Crook's locks off the checkouts
/// the window made.
///
/// Each is a listing and an unlock, milliseconds on any disk. The bound is
/// for a git that hangs — a repository on a network mount that went away —
/// and it is short, because it is the time between a person closing the
/// window and the process being gone. A lock not taken off by then is the
/// one a crash leaves, which the worktree menu already knows what to do with.
const RELEASE_PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);

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
    // With the plugins and not otherwise, which is the same rule the plugins
    // themselves follow: a picture of the application is the same everywhere,
    // and one that includes this machine's plugins already includes whatever
    // this machine knows about them — a withdrawn one among them, what the
    // registry offers in place of what is installed, and the directory they
    // are installed in, so that a `--dev-plugin` alone is a window that
    // could not remove a real one by the same name.
    let (withdrawn, heard, plugins_directory) = match overrides.with_plugins {
        true => {
            let (withdrawn, heard) = registry_at_startup();
            (withdrawn, heard, crate::plugins::wasm::directory())
        }
        false => Default::default(),
    };
    let (window_id, workspace) = app.add_window(|ctx| {
        Workspace::new(
            fonts,
            cell_font,
            Opening {
                settings,
                channel: Channel::Dev,
                plugins,
                withdrawn,
                heard,
                plugins_directory,
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
    let size = overrides.size.map_or(WINDOW_SIZE, |[width, height]| {
        vec2f(width as f32, height as f32)
    });
    let mut presenter = Presenter::new(window_id, text_layout);
    let frame = |app: &mut App, presenter: &mut Presenter| {
        app.update(|ctx| {
            let invalidation = ctx.take_all_invalidations_for_window(window_id);
            presenter.invalidate(invalidation, ctx);
            presenter.build_scene(size, SNAPSHOT_SCALE_FACTOR, ctx)
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
        // A frame before the shells, because it is *layout* that measures the
        // panes, and a shell opens at the grid its pane measured: a banner
        // the shell prints at startup is sized for the pane rather than for
        // the eighty columns every terminal used to start at. And a frame
        // after them, because a key is dispatched into the element tree the
        // last frame built, and that tree has to hold the shell.
        app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| workspace.expect_terminals(ctx));
        });
        frame(&mut app, &mut presenter);
        let pane = start_shells(&mut app, &workspace)?;
        frame(&mut app, &mut presenter);

        if !overrides.run.is_empty() {
            // A shell that could not be started prints nothing, and a run
            // that waited ten seconds to type into it would report an empty
            // block as if the command had run. The reason it has no shell is
            // the report.
            let failure = workspace.read(&app, |workspace, app| {
                workspace.terminal_failure(pane, app).map(str::to_owned)
            });
            if let Some(failure) = failure {
                bail!("the shell could not be started: {failure}");
            }
            await_shell(&queue, &mut app, &workspace, pane);
            for (index, command) in overrides.run.iter().enumerate() {
                // A shell that has gone — an `exit` typed before this, a
                // crash — takes nothing more, and a command typed into it
                // would wait out the whole timeout for a prompt that is not
                // coming, once per command still on the list.
                let gone =
                    workspace.read(&app, |workspace, app| workspace.shell_is_gone(pane, app));
                if gone {
                    let left = overrides.run.len() - index;
                    log::warn!("the shell has exited; {left} of the commands were not run");
                    break;
                }
                let before =
                    workspace.read(&app, |workspace, app| workspace.newest_block(pane, app));
                type_run(&mut app, &mut presenter, window_id, command);
                let printed = settle_run(&queue, &mut app, &workspace, pane, before);
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
        open_find_output(&mut app, &workspace, pane, &overrides);

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

    let (pixels, width, height) =
        render_scene_to_rgba(&scene, size, &font_db).context("failed to render the frame")?;

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
/// Opens the first pane's find bar with a query in it, for `--find-output`.
///
/// After [`aim_at_blocks`], so there is output to search and a frame has
/// measured how far it can scroll — the bar scrolls the first match into view
/// the way a keypress would.
fn open_find_output(
    app: &mut App,
    workspace: &ViewHandle<Workspace>,
    pane: PaneId,
    overrides: &Overrides,
) {
    let Some(query) = overrides.find_output.clone() else {
        return;
    };
    app.update(|ctx| {
        workspace.update(ctx, |workspace, ctx| {
            workspace.open_find_with(pane, &query, ctx);
        });
    });
}

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

/// The window's name: how many panes are waiting for a person, when any
/// are, then the active tab's title, then the application's.
///
/// The way a browser names its window after the page and a mail client
/// counts the unread in front of it. The count first, in the words the
/// header's chip uses, because it is the one thing about a window that
/// matters while it is *behind* another — an agent that stopped for an
/// answer in a window nobody is looking at is exactly what a switcher and
/// a taskbar are for finding. The tab next, since it is the part that
/// differs between two windows; the application after the dash so that a
/// switcher lists them under one name; and the application's name alone
/// when there is no tab, or a tab with nothing to call itself.
fn window_title(active: Option<&str>, waiting: usize, base: &str) -> String {
    let mut title = String::new();
    if waiting > 0 {
        title.push_str(&format!("({waiting} waiting) "));
    }
    match active.map(str::trim) {
        Some(tab) if !tab.is_empty() => title.push_str(&format!("{tab} — {base}")),
        _ => title.push_str(base),
    }
    title
}

/// When a window behind something else should ask the desktop for a look.
///
/// When the count of waiting panes *rises* while the window is unfocused, and
/// not whenever it is above zero: the panes waiting as a person leaves are
/// panes they just saw, and the pane they leave with a question on it joins
/// the count at that moment without being news. So the first count seen
/// with the window behind something else is the mark the count has to rise
/// past — the moment of leaving, since the window's focus changing is itself
/// a change to the workspace — and every count after it moves the mark to
/// wherever the count is, which is what makes a pane that stops waiting and
/// starts again ask again, and a count that holds still ask once.
#[derive(Debug, Default)]
struct Urgency {
    /// The count as of the last look.
    waiting: usize,
    /// Whether the window was behind something else at the last look.
    away: bool,
}

impl Urgency {
    /// Whether `waiting` panes waiting, in a window that does or does not
    /// have the focus, is one more than the person saw as they left.
    fn asks(&mut self, waiting: usize, focused: bool) -> bool {
        let rose = self.away && !focused && waiting > self.waiting;
        self.waiting = waiting;
        self.away = !focused;
        rose
    }
}

/// The window's name for the workspace as it stands: [`window_title`] of its
/// active tab and of the panes waiting in it.
fn window_title_of(workspace: &Workspace, base: &str) -> String {
    window_title(
        workspace.tabs().active().map(|tab| tab.title()),
        crate::plugins::tabs::waiting_count(workspace),
        base,
    )
}

/// Where a [`Beacon`] says what it says: the window's event loop, or a
/// test's notebook.
trait Desktop {
    /// Names the window.
    fn set_title(&self, title: String);
    /// Asks the desktop to point at the window.
    fn request_attention(&self);
    /// Puts the count of waiting panes on the application's icon, or takes
    /// it off at zero.
    fn set_badge(&self, waiting: usize);
    /// Asks, where the icon's badge is drawn only with a person's leave, for
    /// that leave, and sets the badge again once it is given.
    fn ask_to_badge(&self);
}

impl Desktop for Proxy {
    fn set_title(&self, title: String) {
        Proxy::set_title(self, title);
    }

    fn request_attention(&self) {
        Proxy::request_attention(self);
    }

    fn set_badge(&self, waiting: usize) {
        Proxy::set_badge(self, waiting);
    }

    fn ask_to_badge(&self) {
        let proxy = self.clone();
        crate::notify::ask_to_badge(move || proxy.show_badge_again());
    }
}

/// What the window tells the desktop about the workspace: its name, with the
/// count of waiting panes in front, the same count on the application's icon,
/// and when to point at it.
///
/// The name is the active tab's, the way every terminal names its window
/// after the shell's title and Warp after the tab's, because it is what the
/// taskbar, the dock and the switcher show: with an agent per tab, "bisect
/// the flaky test — Crook" is the difference between finding the right Crook
/// and opening each in turn. The badge is that count where a minimised
/// window still shows it — the dock icon, on macOS — and it is the title's
/// count exactly, so the two never disagree about who is waiting. Crook.app's
/// is drawn only with the leave its notifications are posted with, which is
/// asked for the first time there is a count to show while notifications are
/// wanted — a pane can wait with the window in front, where nothing is posted
/// to ask with. Not while they are not: that leave is asked for in macOS's
/// words as leave to send notifications, and a person who turned them off
/// has said no to those already.
///
/// Followed on every change to the window's views — the same invalidation
/// that asks for a frame — rather than on the frame, because the window
/// these are for is the one that may get no frames at all. A Wayland
/// compositor sends no frame callback to a surface it is not showing, and
/// winit holds every redraw back until the callback comes, so a window on
/// another workspace builds nothing; macOS refuses to present to an
/// occluded window.
///
/// Nothing the window did before it was watched goes unseen: registering the
/// callback is an update like any other, and its flush runs the callback of
/// every window holding changes no frame has taken yet — for a window that
/// has drawn nothing, every view it opened with. So the window is named, and
/// [`Urgency`] has the count its first rise is measured from, the moment the
/// callback is registered. The frame does not follow the workspace as well:
/// the title and the count read what the strip and the header's chip draw, so
/// a change to them that invalidated no view would be missing from the
/// window's own frame too, and that is where it would want fixing.
struct Beacon<D> {
    /// The application's own name for the window — "Crook", or the channel's
    /// spelling of it — which the active tab's title goes in front of.
    base_title: String,
    /// The name the window was last given, so a change that did not move it
    /// sends nothing: every window system takes a title as a message.
    title: Option<String>,
    /// When to ask the desktop to point at the window.
    urgency: Urgency,
    /// The count the icon's badge was last given, for the same reason as
    /// `title`: the dock redraws the tile for every one. Zero until then,
    /// which is the badge an application opens with — none.
    badge: usize,
    /// Whether the desktop has been asked for leave to badge the icon: once,
    /// with the first count above zero that comes while notifications are
    /// wanted, since after the first time the answer is the person's setting,
    /// which the dock follows by itself.
    asked_to_badge: bool,
    desktop: D,
}

impl<D: Desktop> Beacon<D> {
    fn new(base_title: String, desktop: D) -> Self {
        Self {
            base_title,
            title: None,
            urgency: Urgency::default(),
            badge: 0,
            asked_to_badge: false,
            desktop,
        }
    }

    /// Says whatever the workspace as it now stands changes: a new name, a
    /// new count on the badge, and a request for a look when one more pane
    /// has started waiting while the window is behind something else.
    fn follow(&mut self, workspace: &Workspace) {
        let title = window_title_of(workspace, &self.base_title);
        if self.title.as_deref() != Some(title.as_str()) {
            self.desktop.set_title(title.clone());
            self.title = Some(title);
        }

        let waiting = crate::plugins::tabs::waiting_count(workspace);
        if self.badge != waiting {
            self.desktop.set_badge(waiting);
            self.badge = waiting;
        }
        // Outside the badge's own change, so that notifications switched on
        // with a pane already waiting ask then, and not with the next count.
        if !self.asked_to_badge
            && waiting > 0
            && crate::plugins::notifications::are_wanted(workspace)
        {
            self.desktop.ask_to_badge();
            self.asked_to_badge = true;
        }
        if self.urgency.asks(waiting, workspace.is_window_focused()) {
            self.desktop.request_attention();
        }
    }
}

/// What runs on every change to the window's views: a frame is asked for, and
/// the [`Beacon`] follows the workspace without waiting for that frame.
///
/// The callback owns the beacon outright: the frame does not follow the
/// workspace too (see [`Beacon`]), so nothing else has a use for it.
fn on_every_change<D: Desktop + 'static>(
    mut beacon: Beacon<D>,
    workspace: ViewHandle<Workspace>,
    redraw: impl Fn() + 'static,
) -> impl FnMut(WindowId, &mut AppContext) + 'static {
    move |_, ctx| {
        redraw();
        workspace.read(&*ctx, |workspace, _| beacon.follow(workspace));
    }
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

/// Pumps the queue until the pane stops changing, and returns what the
/// command printed.
///
/// "Stopped changing" is the only definition of finished available from outside
/// a pty: a shell runs the command, prints its prompt back, and goes quiet.
/// Bounded by [`RUN_TIMEOUT`], because a command that never goes quiet — `top`,
/// a `sleep` — still has to produce a picture.
///
/// A shell that exits — because the command was `exit`, or because it was
/// the last thing the shell could take — is finished too, whatever the grid
/// says: a grid emptied by the exit would otherwise never count as quiet,
/// and the wait would run to its timeout.
///
/// What is reported depends on what the shell did with the output. A shell
/// with marks closes a block behind the command and its rows leave the grid
/// for it, so the block is the report — `newest_before` is the block that was
/// newest when the command was typed, and a different one now is the
/// command's. A shell without marks leaves everything on the grid, and the
/// grid is the report, as it was for both before the blocks were asked.
fn settle_run(
    queue: &LocalQueue,
    app: &mut App,
    workspace: &ViewHandle<Workspace>,
    pane: PaneId,
    newest_before: Option<BlockId>,
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
            break;
        }

        let gone = workspace.read(&*app, |workspace, app| workspace.shell_is_gone(pane, app));
        if gone {
            log::info!("the shell exited while the command ran");
            break;
        }
        if Instant::now() >= deadline {
            log::warn!("the command was still printing after {RUN_TIMEOUT:?}");
            break;
        }
        std::thread::sleep(RUN_POLL);
    }

    workspace
        .read(&*app, |workspace, app| {
            if workspace.newest_block(pane, app) == newest_before {
                return None;
            }
            workspace.newest_block_output(pane, app)
        })
        .unwrap_or(showing)
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
        /// Commits ahead of `main`, then the files, added and removed lines
        /// since the branch left it.
        ahead: Option<(u32, u32, u32, u32)>,
    }

    let seeded = [
        Seeded {
            title: "port the tab bar",
            status: AgentStatus::Running,
            directory: "app/src/workspace",
            branch: "eugen/tab-options",
            diff: Some((6, 214, 37)),
            ahead: None,
        },
        Seeded {
            title: "sandbox the plugin host",
            status: AgentStatus::NeedsInput,
            directory: "crates/crook_wasm/src",
            branch: "main",
            diff: Some((1, 12, 0)),
            ahead: None,
        },
        Seeded {
            title: "bisect the flaky test",
            status: AgentStatus::Failed,
            directory: "crates/crookui/src/rendering",
            branch: "eugen/atlas-repro",
            diff: None,
            // Everything committed, which is the row that used to go blank.
            ahead: Some((3, 4, 57, 9)),
        },
        Seeded {
            title: "read the recon notes",
            status: AgentStatus::Idle,
            directory: "docs",
            branch: "main",
            diff: None,
            ahead: None,
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
        ahead: None,
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
        let since_base = seed
            .ahead
            .map(|(commits, files, added, removed)| git::SinceBase {
                base: "main".to_owned(),
                commits,
                diff: git::DiffStats {
                    files_changed: files,
                    lines_added: added,
                    lines_removed: removed,
                },
            });
        let facts = git::GitFacts {
            branch: Some(git::Head::Branch(seed.branch.to_owned())),
            diff: seed.diff.map(
                |(files_changed, lines_added, lines_removed)| git::DiffStats {
                    files_changed,
                    lines_added,
                    lines_removed,
                },
            ),
            since_base,
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
                since_base: None,
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
/// Four methods, and each one is a translation: a size into a scene, an OS
/// event into an action, a finished frame into either another one or an exit,
/// and the desktop's close into the same close the title bar asks for.
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
    /// The named actions `--action` asked for, until they have run. See
    /// [`Overrides::actions`] for when that is.
    actions: Vec<String>,
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
    /// The socket the window answers `crook pane list` on, for as long as the
    /// window is open; dropping it closes the socket and removes its file.
    /// `None` where there is none — see [`control::Control::open`].
    _control: Option<control::Control>,
}

/// The real window, behind the handle the workspace holds.
///
/// The whole of the seam: five verbs forwarded to the windowing layer, which
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

    fn bring_forward(&self) {
        self.0.bring_forward();
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

/// What the command line decided, and what the launch found before the
/// window, as one argument.
///
/// Values that arrive together, are read once each, and travel from [`run`] to
/// [`Shell::new`] without anything in between looking at them. Passing them
/// separately put `Shell::new` over clippy's limit, and grouping them is the
/// answer that says something true: they are the launch, not so many
/// unrelated parameters.
struct Launch {
    /// Which channel is running, for the settings page's About section.
    channel: Channel,
    /// Exit after this many frames, so the binary is runnable unattended.
    frames: Option<u32>,
    /// What the command line asked to start differently.
    overrides: Overrides,
    /// Where this run's log and crash reports go, and the reports earlier
    /// runs left unread, for the About page and the line under the header.
    diagnostics: Option<diagnostics::Diagnostics>,
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
/// plugin that came from outside. After the file, whatever the registry's copy
/// on this machine has to say about that version: that it is behind, or that
/// it was taken back.
fn installed_plugins_text() -> String {
    let Some(directory) = crate::plugins::wasm::directory() else {
        return String::from("this machine has no data directory to install plugins into");
    };

    installed_plugins_in(
        &directory,
        &Settings::for_user(),
        crate::plugins::store::cache::Cache::user(),
    )
}

/// What `--check-update` prints: whether a newer Crook has been released.
///
/// One request, and the line it produces says what to do about the answer —
/// `--update` when this copy can replace itself, and the reason it cannot when
/// it cannot, since "0.2.0 is out" is only useful next to the way to get it.
fn update_checked(channel: Channel) -> Result<String> {
    let agent = crate::plugins::store::fetch::agent();
    let published = crate::update::published(&agent).map_err(anyhow::Error::msg)?;
    let running = crate::update::running();

    if !published.is_newer() {
        return Ok(format!("crook {running} is the newest release"));
    }
    let how = match crate::update::replaceable(channel) {
        Ok(binary) => format!("`crook --update` installs it over {}", binary.display()),
        Err(refusal) => refusal.to_string(),
    };
    Ok(format!(
        "crook {} is out, and this is {running}\n{how}",
        published.version
    ))
}

/// What `--update` prints, having done it.
///
/// Refusing is an error rather than a line, because a script that asked for an
/// update and got a sentence about a bundle should stop rather than carry on
/// as though it had one.
fn updated(channel: Channel) -> Result<String> {
    let agent = crate::plugins::store::fetch::agent();
    let published = crate::update::published(&agent).map_err(anyhow::Error::msg)?;
    let running = crate::update::running();

    if !published.is_newer() {
        return Ok(format!("crook {running} is already the newest release\n"));
    }
    // Before the download rather than after it: forty megabytes fetched to be
    // told the bundle cannot be written is forty megabytes nobody asked for.
    let binary = crate::update::replaceable(channel).map_err(|refusal| anyhow!("{refusal}"))?;

    let mut said = format!(
        "==> {running} → {} ({})\n",
        published.version, published.tag
    );
    crate::update::install(&published.tag, &binary, &agent).map_err(anyhow::Error::msg)?;
    said.push_str(&format!("==> {}\n", binary.display()));
    said.push_str("==> Updated. A window that is open is still the old one: restart it.\n");
    Ok(said)
}

/// What `--update-plugin` and `--update-plugins` print.
///
/// The work is `store::updating`, which is the same fetch, the same hash check
/// and the same write the store's button does — this is the part that turns
/// its outcomes into lines.
fn plugins_updated(only: Option<&crook_plugin::PluginId>) -> Result<String> {
    let outcomes = crate::plugins::store::updating::update(only).map_err(anyhow::Error::msg)?;
    if outcomes.is_empty() {
        return Ok(match only {
            Some(id) => format!("{id} is the newest build the registry has\n"),
            None => String::from("every installed plugin is the newest build the registry has\n"),
        });
    }

    let mut said = String::new();
    let mut failed = 0;
    for outcome in &outcomes {
        match &outcome.result {
            Ok(()) => said.push_str(&format!(
                "updated {} {} → {}\n",
                outcome.id, outcome.from, outcome.to
            )),
            Err(why) => {
                failed += 1;
                said.push_str(&format!(
                    "{} {} → {} failed: {why}\n",
                    outcome.id, outcome.from, outcome.to
                ));
            }
        }
    }
    // The lines are the answer either way; the exit status is what a script
    // reads, and one plugin that did not install is a run that did not do what
    // it was asked.
    match failed {
        0 => Ok(said),
        _ => {
            print!("{said}");
            bail!("{failed} of {} could not be updated", outcomes.len())
        }
    }
}

/// What `--plugins --json` prints: the same plugins, for a script.
///
/// One object per plugin and nothing a person would have to parse out of a
/// column. The keys are the ones a script can rely on — `id`, `version`,
/// `path`, `enabled`, `allowed` — and `allowed` is the grant exactly as the
/// settings hold it, one key per host and per path, since that is the form
/// the Plugins page compares against what a plugin asks for. The module's
/// ABI is not among them: the host's manifest does not carry it, and the
/// only way to read it is to run the module's own export — at which point it
/// is the host's, or the module would not have opened.
///
/// `[]` with no data directory rather than the sentence `--plugins` prints:
/// a script reads stdout as JSON or not at all, and the sentence goes to
/// the log, which is stderr.
fn installed_plugins_json() -> String {
    let Some(directory) = crate::plugins::wasm::directory() else {
        log::warn!("this machine has no data directory to install plugins into");
        return String::from("[]");
    };
    installed_plugins_json_in(&directory, &Settings::for_user())
}

/// The same, out of a named plugins directory against a named copy of the
/// settings — which is what lets a test read it back.
fn installed_plugins_json_in(directory: &std::path::Path, settings: &Settings) -> String {
    /// One entry, with the keys in the order the help lists them.
    #[derive(serde::Serialize)]
    struct Entry<'a> {
        id: &'a str,
        version: &'a str,
        /// `null` for a directory a version could not be read out of, which
        /// `installed` does not list — kept an `Option` so the two walks
        /// disagreeing is a `null` and not a panic.
        path: Option<String>,
        enabled: bool,
        allowed: &'a [String],
    }

    let installed = crate::plugins::wasm::installed(directory);
    let entries: Vec<Entry> = installed
        .iter()
        .map(|plugin| {
            let manifest = crate::plugin::Plugin::manifest(plugin.as_ref());
            let id = manifest.id.as_str();
            Entry {
                id,
                version: manifest.version,
                path: crate::plugins::wasm::module_in(directory, &manifest.id)
                    .map(|path| path.display().to_string()),
                enabled: !settings.disabled_plugins().iter().any(|name| name == id),
                allowed: settings.granted_to(id),
            }
        })
        .collect();

    // Pretty, because a person reads the output of a command they typed
    // before a script does, and a parser reads either.
    serde_json::to_string_pretty(&entries)
        .expect("a list of strings, a bool and a list of strings serializes")
}

/// The same, out of a named plugins directory against a named copy of the
/// index — which is what lets a test say what the line says.
fn installed_plugins_in(
    directory: &std::path::Path,
    settings: &Settings,
    cache: Option<crate::plugins::store::cache::Cache>,
) -> String {
    let installed = crate::plugins::wasm::installed(directory);
    if installed.is_empty() {
        return format!("no plugins installed in {}", directory.display());
    }

    let index = cache
        .and_then(|cache| cache.read())
        .map(|cached| cached.index);
    let heard = crate::plugins::store::index::Heard::offered(index.as_ref());

    // Gathered before any line is formatted, so the columns can be given a
    // width: a ragged left edge on the version turns reading a short list
    // into scanning it, which is what `pip list` and every other "what is
    // installed" pads to avoid.
    struct Row {
        id: String,
        version: String,
        module: String,
        facts: String,
    }
    let mut rows = Vec::with_capacity(installed.len());
    for plugin in &installed {
        let manifest = crate::plugin::Plugin::manifest(plugin.as_ref());
        let id = manifest.id.as_str();
        let module = crate::plugins::wasm::module_in(directory, &manifest.id)
            .map(|path| path.display().to_string())
            .unwrap_or_default();

        // Each fact in its own parentheses, and each only when it is so: a
        // plugin can be switched off and behind at once, and a line that
        // said one of those in place of the other would be a line somebody
        // acted on wrongly.
        let mut said = Vec::new();
        if settings.disabled_plugins().iter().any(|name| name == id) {
            said.push(String::from("(switched off)"));
        }
        let withdrawn = index.as_ref().and_then(|index| {
            crate::plugins::store::index::withdrawn(index, &manifest.id, manifest.version)
        });
        if let Some(why) = &withdrawn {
            said.push(format!("(withdrawn: {why})"));
        }
        if let Some(offer) = heard.offer(&manifest.id) {
            use crate::plugins::store::index::change;
            if let Some(release) =
                change(offer, Some(manifest.version), withdrawn.is_some()).fetchable()
            {
                said.push(format!("({} in the registry)", release.version));
            }
        }

        rows.push(Row {
            id: id.to_owned(),
            version: manifest.version.to_owned(),
            module,
            // The facts, each behind its own two spaces, so the path column
            // ends where a fact begins and the annotations trail off the
            // aligned block rather than sitting in it.
            facts: said.iter().map(|fact| format!("  {fact}")).collect(),
        });
    }

    // By character rather than by byte, since a plugin id and a version are
    // ASCII but the padding is a count of columns and not of bytes.
    let id_width = rows
        .iter()
        .map(|row| row.id.chars().count())
        .max()
        .unwrap_or(0);
    let version_width = rows
        .iter()
        .map(|row| row.version.chars().count())
        .max()
        .unwrap_or(0);

    rows.iter()
        .map(|row| {
            format!(
                "{id:id_width$}  {version:version_width$}  {module}{facts}",
                id = row.id,
                version = row.version,
                module = row.module,
                facts = row.facts,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    let builtin = plugins.len();
    if let Some(directory) = crate::plugins::wasm::directory() {
        for module in crate::plugins::wasm::installed(&directory) {
            // Nothing that installs one lets it take a built-in's id, but a
            // file copied into the directory by hand passed through nothing.
            let id = &module.manifest().id;
            if plugins[..builtin]
                .iter()
                .any(|carried| carried.manifest().id == *id)
            {
                log::warn!("{id} is one of Crook's own; the module installed as it is not loaded");
                continue;
            }
            plugins.push(module);
        }
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
    let mut carried = None;
    match std::fs::read(&module)
        .map_err(|why| why.to_string())
        .and_then(|bytes| crate::plugins::wasm::opened(&bytes).map(|plugin| (plugin, bytes.len())))
    {
        Ok((plugin, bytes)) => {
            let named = crate::plugin::Plugin::manifest(&plugin).id.clone();
            log::info!("{named} from {} ({bytes} bytes)", module.display());
            // The copy already installed under the same name gives way, on
            // the first frame as it would on the first rebuild: `Host::carry`
            // replaces by id, and a window that opened with both would draw
            // the chip twice and list the plugin twice until the watcher
            // fired. What is installed is what the person is replacing.
            plugins.retain(|carried| crate::plugin::Plugin::manifest(carried.as_ref()).id != named);
            // Which plugin this turned out to be, so that a rebuild renaming
            // it can take the old one out rather than leaving it loaded under
            // a name its source no longer has.
            carried = Some(named);
            plugins.push(Box::new(plugin));
        }
        Err(why) => log::warn!("{}: {why}", module.display()),
    }

    plugins.push(Box::new(Dev::watching(module, seen, carried)));
    Ok(plugins)
}

/// What the registry's copy on this machine says: which installed plugins it
/// has withdrawn and why, and everything it offers.
///
/// Read off the store's copy of the index — the file the Store section keeps
/// beside the plugins — rather than over the network, because a yank has to be
/// honoured on a machine that is offline and on the launch after the registry
/// said so. Nothing here fetches anything, and an index that is missing or
/// unreadable withdraws nothing and offers nothing: a list that cannot be read
/// is never a reason to stop running something somebody installed.
///
/// A version is what is withdrawn rather than a plugin, so the answer is about
/// the version on this machine: somebody running the one before the bad one
/// keeps running it. The offers are what the Plugins page compares that
/// version against, so a card can say "a newer one is in the registry" before
/// anybody has opened the Store.
fn registry_at_startup() -> (
    std::collections::BTreeMap<String, String>,
    crate::plugins::store::index::Heard,
) {
    let index = crate::plugins::store::cache::Cache::user()
        .and_then(|cache| cache.read())
        .map(|cached| cached.index);
    let heard = crate::plugins::store::index::Heard::offered(index.as_ref());

    let (Some(index), Some(directory)) = (index, crate::plugins::wasm::directory()) else {
        return (std::collections::BTreeMap::new(), heard);
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
    (withdrawn, heard)
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
        let (withdrawn, heard) = registry_at_startup();
        let control = control::Control::open();
        let (window_id, workspace) = app.add_window(|ctx| {
            Workspace::new(
                fonts,
                cell_font,
                Opening {
                    settings,
                    channel: launch.channel,
                    plugins,
                    withdrawn,
                    heard,
                    plugins_directory: crate::plugins::wasm::directory(),
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
                // strip is there: the git poll reads its directories, and
                // `start_terminals` opens a shell in every pane it holds. The
                // overrides have to be in place by then as well:
                // `start_git_poll` decides whether to pay for `git diff` from
                // the density it finds, and a density the command line asked
                // for that arrived later would leave the first cycle
                // gathering the wrong half.
                restore_and_override(workspace, &session, &launch.overrides, ctx);
                // This window is on a desktop, which is the one place a
                // notification is for: a snapshot and a test keep the silent
                // one the workspace opens with.
                workspace.set_notifier(crate::notify::for_this_desktop());
                // Before the shells, which are told where it is.
                if let Some(control) = &control {
                    control.serve(ctx);
                    workspace.set_control_socket(control.path().map(Path::to_path_buf), ctx);
                }
                if let Some(diagnostics) = launch.diagnostics.clone() {
                    workspace.set_diagnostics(diagnostics, ctx);
                }
                workspace.start_git_poll(ctx);
                workspace.start_caret_blink(ctx);
                // Last, and not yet: the shells open after the first frame,
                // which is what measures the panes they open in — see
                // `frame_drawn`. Announced here so that the frame draws the
                // panes as empty rather than as having no shell.
                workspace.expect_terminals(ctx);
            });
        });

        // The only thing that makes a frame happen: a view said it changed.
        // And, beside it, what tells the desktop the window's name and when
        // to point at it, which cannot wait for a frame a hidden window may
        // never be given. No frame has taken anything the window opened with
        // or the update above changed, so this call already runs it once and
        // names the window.
        let beacon = Beacon::new(launch.overrides.window_name(launch.channel), proxy.clone());
        let redraw = proxy.clone();
        app.on_window_invalidated(
            window_id,
            on_every_change(beacon, workspace.clone(), move || {
                redraw.request_redraw();
            }),
        );

        // The windowed run types the commands one after another, so they are
        // queued rather than joined: two commands sent as one line would be
        // one block. Aimed at the focused pane now; its shell opens with the
        // rest after the first frame, and the first command is typed after
        // that. `-e`'s command is the last of them, and the same machinery.
        let typed = launch.overrides.typed();
        let run = (!typed.is_empty())
            .then(|| workspace.read(&app, |workspace, _| workspace.tabs().focused_pane_id()))
            .flatten()
            .map(|pane| Run {
                pane,
                pending: typed.into(),
                deadline: Instant::now() + RUN_TIMEOUT,
                printed: false,
            });

        Self {
            composed: Composed::from_overrides(&launch.overrides),
            actions: launch.overrides.actions.clone(),
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
            _control: control,
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

    /// Opens the shells, once there has been a frame to measure the panes they
    /// open in.
    ///
    /// The first frame, and only once: `expect_terminals` was said before it
    /// and `start_terminals` clears the expectation. A shell opened before
    /// the frame opened at eighty by twenty-four, and whatever its startup
    /// printed — a banner sized with `tput cols` — was printed for that.
    ///
    /// Reports whether it did, because the frame just drawn was built before
    /// the shells existed: its tree has no field in any pane, so a command
    /// typed into it now would be typed into nothing. The caller waits for
    /// the next frame.
    fn start_expected_terminals(&mut self) -> bool {
        let expected = self.workspace.read(&self.app, |workspace, app| {
            workspace.terminals_expected(app)
        });
        if !expected {
            return false;
        }
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| workspace.start_terminals(ctx));
        });
        true
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

    /// Runs what `--action` named, once, at the moment the snapshot runs it:
    /// after the shells are up and any `--run` has been answered, since what
    /// a plugin's action puts up is usually about what the pane just said.
    ///
    /// Nothing is waited for, unlike the snapshot: whatever the action asked
    /// of the pool comes back through the event loop, and the frame after
    /// that draws it. The redraw asked for here is for the action itself,
    /// which is not an event and so has nothing else asking.
    fn run_pending_actions(&mut self) {
        if self.actions.is_empty() || !self.budget_has_started() {
            return;
        }
        for name in std::mem::take(&mut self.actions) {
            run_named_action(&mut self.app, &self.workspace, &name);
        }
        self.proxy.request_redraw();
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

    /// Says what came of the commands `--run` typed, so an unattended run
    /// leaves evidence that the loop worked.
    ///
    /// The blocks, command by command, the way the snapshot's `--run` reports
    /// them: the grid is the wrong thing to print for a shell with marks,
    /// since a finished command's rows leave it for the block, and what is
    /// left is the prompt over the blank lines above it. The grid is the
    /// report only for a shell without marks, which closes no block.
    fn report_run(&self) {
        let Some(run) = self.run.as_ref() else {
            return;
        };
        let printed = self
            .workspace
            .read(&self.app, |workspace, app| {
                workspace
                    .blocks_report(run.pane, app)
                    .or_else(|| workspace.terminal_text(run.pane, app))
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
    /// Takes off the locks the window still holds on the checkouts it made.
    ///
    /// Closing the window closes no pane — the strip keeps its last tab and
    /// the window goes instead — so nothing working in a checkout Crook made
    /// ever left it, and every agent in one ends with the process. Here
    /// rather than wherever the window asks to quit, because the window
    /// manager's close and macOS's Quit never ask.
    fn exiting(&mut self) {
        let held = self
            .workspace
            .read(&self.app, |workspace, _| workspace.held_locks());
        held.release_all(RELEASE_PATIENCE);
    }

    fn build_scene(&mut self, size: Vector2F, scale_factor: f32) -> Rc<Scene> {
        // Written down rather than dispatched: it costs nothing, it invalidates
        // nothing, and it is the only place the window's size is known. A size
        // that changed is told to the workspace all the same, once, because
        // the session file carries it and a resize moves no tab. The first
        // frame counts: a tiling desktop opens the window at whatever size it
        // likes, and the file should say that size rather than the one asked
        // for.
        let resized = self.window_size.get() != size;
        self.window_size.set(size);
        if resized {
            let workspace = &self.workspace;
            self.app.update(|ctx| {
                workspace.update(ctx, |workspace, ctx| workspace.window_resized(ctx))
            });
        }
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

        // The same for the window's focus: it is about the window rather than
        // anything in it, and it is half of what looking at a pane means,
        // which is the workspace's to decide.
        if let Event::WindowFocused(focused) = event {
            let workspace = &self.workspace;
            self.app.update(|ctx| {
                workspace.update(ctx, |workspace, ctx| {
                    workspace.set_window_focused(focused, ctx);
                });
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
        if self.start_expected_terminals() {
            // The shells opened after a frame that had none: the next frame
            // is the first with a field to type into, and it is asked for
            // rather than waited for, since a shell that has nothing to draw
            // yet would not ask.
            self.proxy.request_redraw();
            return;
        }
        self.follow_caret_with_the_input_method();
        self.type_pending_run();
        self.compose_pending_pane();
        self.run_pending_actions();

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

    /// The desktop asked the window to close: its close button, `alt-f4`,
    /// a window manager's close.
    ///
    /// The title bar's own close, dispatched as if it had been pressed, so
    /// that there is one close and it asks on one set of terms — a window
    /// with an agent still working in it puts a question up and stays, and
    /// one without quits through the workspace's `quit`, which is an `Exit`
    /// on the proxy and the event loop's next turn.
    ///
    /// Except in a run with a frame budget, which nobody is sitting at: it
    /// ends by its budget and was never going to wait for an answer, so a
    /// close from outside ends it at once. See `workspace::closing`.
    fn close_requested(&mut self) -> bool {
        if self.frame_budget.is_some() {
            return true;
        }
        let chain = [self.workspace.id()];
        self.app.dispatch_typed_action(
            self.window_id,
            &chain,
            &WorkspaceAction::Window(WindowAction::Close),
        );
        false
    }

    /// The window took the keyboard, which the question a close asks has to
    /// hear: the keys that follow may have been typed at another
    /// application. See `workspace::closing`.
    fn focused(&mut self) {
        let workspace = &self.workspace;
        self.app
            .update(|ctx| workspace.update(ctx, |workspace, _| workspace.window_focused()));
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
        let missing = shell_integration_text(None).expect_err("the flag needs its argument");
        // One sentence after another, and not the indentation of the source
        // file between them: the literal used to carry ten spaces where its
        // lines were joined, which `crook:` printed as they were.
        assert!(
            !format!("{missing:#}").contains("  "),
            "the message carries a run of spaces: {missing:#}"
        );
    }

    #[test]
    fn the_hooks_flag_without_an_agent_lists_the_agents_it_knows() {
        // The complaint is the list: a person who typed the flag bare is
        // asking which names it takes, and the answer that names them all
        // is the one that needs no second try.
        let complaint = parse(&["--agent-hooks"])
            .expect_err("the flag needs its agent")
            .to_string();
        for name in ["claude", "codex", "gemini", "copilot", "opencode", "aider"] {
            assert!(complaint.contains(name), "{complaint}");
        }
    }

    #[test]
    fn the_skill_is_printed_and_answered_without_a_window() {
        // The file is the answer, the way the hooks are: a flag that opened a
        // window after printing it would put a window over the shell that
        // was redirecting it into a file.
        assert_eq!(parse(&["--skill"]).expect("valid"), Startup::Answered);
    }

    #[test]
    fn tab_is_a_command_only_as_the_first_word() {
        // First, it is `crook tab …`: the rest of the line reaches the
        // command's parser, which asks for its verb.
        let bare = parse(&["tab"]).expect_err("tab needs a verb").to_string();
        assert!(bare.contains("needs a verb"), "{bare}");
        let later = parse(&["--frames", "3", "tab", "new"])
            .expect_err("not a flag")
            .to_string();
        assert!(later.contains("unrecognised argument tab"), "{later}");
    }

    #[test]
    fn events_is_a_command_only_as_the_first_word() {
        let bare = parse(&["events"])
            .expect_err("events needs --follow")
            .to_string();
        assert!(bare.contains("needs --follow"), "{bare}");
        let later = parse(&["--frames", "3", "events", "--follow"])
            .expect_err("not a flag")
            .to_string();
        assert!(later.contains("unrecognised argument events"), "{later}");
    }

    #[test]
    fn pane_is_a_command_only_as_the_first_word() {
        // First, it is `crook pane …`, and the rest of the line is its own:
        // it reaches the command's parser, which asks for its verb.
        let bare = parse(&["pane"]).expect_err("pane needs a verb").to_string();
        assert!(bare.contains("needs a verb"), "{bare}");
        // Anywhere else it is a word no flag takes, which is what it was
        // before it meant anything.
        let later = parse(&["--frames", "3", "pane", "list"])
            .expect_err("not a flag")
            .to_string();
        assert!(later.contains("unrecognised argument pane"), "{later}");
    }

    #[test]
    fn the_skill_has_front_matter_and_names_only_flags_the_parser_knows() {
        // Agent Skills format: a `---` block carrying a name and a one-line
        // description, then the body. A loader that finds no front matter
        // finds no skill.
        let mut lines = agent::SKILL.lines();
        assert_eq!(lines.next(), Some("---"));
        let front: Vec<&str> = lines.by_ref().take_while(|line| *line != "---").collect();
        assert!(front.contains(&"name: crook"), "front matter: {front:?}");
        let described = front
            .iter()
            .filter(|line| line.starts_with("description: "))
            .count();
        assert_eq!(1, described, "one description line, not {described}");
        assert!(
            lines.any(|line| line.starts_with("# ")),
            "the body starts with a heading"
        );

        // Every `crook --flag` the text tells an agent to run is one in the
        // help's own table. A flag renamed there and not here would be a
        // skill teaching an agent to type something `crook` calls
        // unrecognised, and the agent would have no way to know.
        let help = help_text();
        // The short spellings come first on their lines — `-h, --help` —
        // which is why the entry is the first long word rather than the
        // first word.
        let table: Vec<&str> = help
            .lines()
            .filter_map(|line| line.strip_prefix("    "))
            .filter(|entry| entry.starts_with('-'))
            .filter_map(|entry| entry.split(' ').find_map(|word| word.strip_prefix("--")))
            .collect();
        let mut flags: Vec<&str> = agent::SKILL
            .match_indices("crook --")
            .map(|(at, _)| {
                let rest = &agent::SKILL[at + "crook --".len()..];
                let end = rest
                    .find(|character: char| !character.is_ascii_lowercase() && character != '-')
                    .unwrap_or(rest.len());
                &rest[..end]
            })
            .collect();
        flags.sort_unstable();
        flags.dedup();
        assert!(
            flags.contains(&"agent") && flags.contains(&"agent-hooks"),
            "the skill teaches the status report: {flags:?}"
        );
        // The other way round, for the status report's own options: the skill
        // says it is the whole of what a pane can do, and it went without
        // `--message` from the day that flag arrived, so an agent following
        // it never said what it was waiting for.
        let report = help
            .lines()
            .find_map(|line| line.trim_start().strip_prefix("--agent <STATUS>"))
            .expect("--help lists the status report");
        for option in report.split('[').filter_map(|part| part.split(' ').next()) {
            if !option.is_empty() {
                assert!(
                    agent::SKILL.contains(option),
                    "--help gives `--agent` {option}, which the skill never mentions"
                );
            }
        }
        // And the commands, which are words rather than flags: the skill
        // teaches asking the window, and every `crook pane …` it names is
        // one --help lists.
        assert!(
            agent::SKILL.contains("crook pane list"),
            "the skill teaches asking the window what is open"
        );
        assert!(
            agent::SKILL.contains("crook tab new"),
            "the skill teaches opening a worker's tab"
        );
        for noun in ["pane", "tab"] {
            let command = format!("crook {noun} ");
            for (at, _) in agent::SKILL.match_indices(&command) {
                let verb = agent::SKILL[at + command.len()..]
                    .split(|character: char| !character.is_ascii_lowercase())
                    .next()
                    .unwrap_or_default();
                assert!(
                    help.contains(&format!("crook {noun} {verb}")),
                    "the skill names `crook {noun} {verb}`, which --help does not list"
                );
            }
        }
        // The watching commands, each with every flag the skill gives it
        // on the line of --help's usage that spells it.
        for command in ["crook pane wait", "crook pane blocks", "crook events"] {
            assert!(
                agent::SKILL.contains(command),
                "the skill teaches `{command}`"
            );
            let usage = help
                .lines()
                .map(str::trim_start)
                .find(|line| line.starts_with(command))
                .unwrap_or_else(|| panic!("--help's usage lists `{command}`"));
            for (at, _) in agent::SKILL.match_indices(&format!("{command} ")) {
                // To the end of the command: a `&&` or a comment starts
                // another one, and a backtick ends one quoted in prose.
                let line = agent::SKILL[at..].lines().next().unwrap_or_default();
                let line = line.split(['&', '#', '`']).next().unwrap_or_default();
                for flag in line.split(' ').filter(|word| word.starts_with("--")) {
                    assert!(
                        usage.contains(flag),
                        "the skill gives `{command}` {flag}, which --help's `{usage}` does not"
                    );
                }
            }
        }
        // Every flag of `tab new` the skill uses is one the command takes.
        for (at, _) in agent::SKILL.match_indices("crook tab new ") {
            let line = agent::SKILL[at..].lines().next().unwrap_or_default();
            let flags = line
                .split(" -- ")
                .next()
                .unwrap_or_default()
                .split(' ')
                .filter(|word| word.starts_with("--"));
            for flag in flags {
                assert!(
                    help.contains(&format!("[{flag}")),
                    "the skill gives `crook tab new` {flag}, which --help does not list"
                );
            }
        }
        for flag in flags {
            assert!(
                table.contains(&flag),
                "the skill names --{flag}, which --help does not list"
            );
            // And the parser, for the flags that stop at a missing argument:
            // `--plugins` bare would list this machine's plugins, which is
            // nothing a test should read.
            let takes_argument = help.contains(&format!("--{flag} <"));
            if takes_argument {
                let complaint = parse(&[&format!("--{flag}")])
                    .err()
                    .map(|error| error.to_string());
                assert!(
                    !complaint.is_some_and(|complaint| complaint.contains("unrecognised")),
                    "the skill names --{flag}, which the parser has never heard of"
                );
            }
        }
    }

    #[test]
    fn a_plugin_that_cannot_be_uninstalled_is_named_once() {
        // The refusal names the plugin; a context that named it again read
        // `x: "x" is not `owner/name``.
        let error = parse(&["--uninstall-plugin", "not a name"]).expect_err("not a plugin id");
        let text = format!("{error:#}");
        assert_eq!(
            text.matches("not a name").count(),
            1,
            "the name is said twice: {text}"
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
    fn the_window_is_named_after_the_tab_it_shows_and_after_crook_when_there_is_none() {
        assert_eq!(
            window_title(Some("bisect the flaky test"), 0, "Crook"),
            "bisect the flaky test — Crook"
        );
        assert_eq!(
            window_title(Some("port the tab bar"), 0, "Crook (dev)"),
            "port the tab bar — Crook (dev)"
        );
        // Nothing to call itself, or nothing at all: the application's name
        // alone rather than a dash with nothing in front of it.
        assert_eq!(window_title(Some("   "), 0, "Crook"), "Crook");
        assert_eq!(window_title(None, 0, "Crook"), "Crook");
    }

    #[test]
    fn the_window_counts_the_panes_waiting_for_a_person_in_front_of_its_name() {
        // In the header chip's words, so the two say the same thing; and
        // nothing at all for none, since a count of zero is not information.
        assert_eq!(
            window_title(Some("bisect the flaky test"), 1, "Crook"),
            "(1 waiting) bisect the flaky test — Crook"
        );
        assert_eq!(window_title(None, 3, "Crook"), "(3 waiting) Crook");
        assert_eq!(window_title(Some("x"), 0, "Crook"), "x — Crook");
    }

    #[test]
    fn a_window_in_front_never_asks_the_desktop_for_a_look() {
        // The person is at this window: a pane starting to wait is on the
        // chip, and X11 would put its urgency hint on a focused window all
        // the same, so nothing is asked here to be ignored further down.
        let mut urgency = Urgency::default();
        assert!(!urgency.asks(1, true));
        assert!(!urgency.asks(3, true));
    }

    #[test]
    fn leaving_with_a_question_on_screen_is_not_asked_about_and_the_next_one_is() {
        // The person left with the focused pane waiting; the count that now
        // includes it is what they saw, and a bounce as they switched away
        // would be the desktop telling them what they were just looking at.
        let mut urgency = Urgency::default();
        assert!(!urgency.asks(0, true));
        assert!(!urgency.asks(1, false), "the question they left with");

        assert!(urgency.asks(2, false), "a second pane stopped to ask");
        assert!(!urgency.asks(2, false), "and it asks once, not every look");

        // A pane that went back to work and then stopped again is a new
        // question, and asks again.
        assert!(!urgency.asks(1, false));
        assert!(urgency.asks(2, false));

        // Coming back and leaving again is leaving with whatever is there.
        assert!(!urgency.asks(2, true));
        assert!(!urgency.asks(3, false), "the one they left with this time");
        assert!(urgency.asks(4, false));
    }

    #[test]
    fn shell_names_the_program_and_is_a_reason_to_open_one() {
        let Startup::Window { overrides, .. } = parse(&["--shell", "/opt/fish"]).expect("valid")
        else {
            panic!("a shell alone opens the window");
        };
        assert_eq!(overrides.shell, Some(PathBuf::from("/opt/fish")));
        // A snapshot with a shell named opens a pane for it: naming one is
        // the whole reason.
        assert!(overrides.wants_shells());
        assert!(!Overrides::default().wants_shells());

        let bare = parse(&["--shell"]).expect_err("a shell needs a path");
        assert!(format!("{bare:#}").contains("--shell"), "{bare:#}");
    }

    #[test]
    fn search_opens_the_settings_unless_a_section_was_named() {
        let alone = parse(&["--search", "font"]).expect("valid");
        assert!(matches!(
            alone,
            Startup::Window {
                overrides: Overrides {
                    settings: Some(None),
                    ..
                },
                ..
            }
        ));
        let store = parse(&["--section", "Store", "--search", "pir"]).expect("valid");
        assert!(matches!(
            store,
            Startup::Window {
                overrides: Overrides { settings: None, .. },
                ..
            }
        ));
    }

    #[test]
    fn size_takes_a_width_and_a_height_and_nothing_else() {
        assert_eq!(
            parse(&["--size", "640x480"]).expect("valid"),
            Startup::Window {
                frames: None,
                overrides: Overrides {
                    size: Some([640, 480]),
                    ..Overrides::default()
                }
            }
        );
        for bad in ["640", "640x", "x480", "640,480", "0x480", "640x-1", "wide"] {
            assert!(parse(&["--size", bad]).is_err(), "{bad:?} was accepted");
        }
        assert!(parse(&["--size"]).is_err(), "the flag needs its argument");
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
        // A short flag after it is a flag, not a page.
        assert_eq!(
            window_overrides(&["--settings", "-e", "htop"]).settings,
            Some(None)
        );
        assert_eq!(
            parse(&["--settings", "-V"]).expect("valid"),
            Startup::Answered
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
    fn the_four_update_flags_are_documented_and_known_to_the_parser() {
        // Not in the loop below, and that is the point: `--check-update`,
        // `--update` and `--update-plugins` reach the network the moment they
        // are parsed, so a test that fed them to the parser would be a test
        // that asked GitHub how the suite is going. What can be checked
        // without leaving the machine is that the help names them and that
        // the one taking an argument complains about the argument rather than
        // about itself.
        let help = help_text();
        for flag in [
            "--check-update",
            "--update",
            "--update-plugin <ID>",
            "--update-plugins",
        ] {
            assert!(help.contains(flag), "{flag} is not in --help");
        }

        let complaint = parse(&["--update-plugin"])
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(
            complaint.contains("owner/name"),
            "--update-plugin with no plugin said {complaint:?}"
        );
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
            "--find-output",
            "--granularity",
            "--density",
            "--size",
            "--agent",
            "--agent-hooks",
            "--skill",
            "--json",
            "-e",
            "--working-directory",
            "--cwd",
            "--title",
            "--app-id",
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

    #[test]
    fn a_flag_after_what_agent_reads_is_refused_rather_than_dropped() {
        // A misspelled flag used to be skipped: the report went out with no
        // message and the command exited 0, so nothing told the hook's author
        // why the row said nothing. Refused before the terminal is touched,
        // which is also why this test needs no terminal.
        for (args, extra) in [
            (
                &["--agent", "needs-input", "--mesage", "approve deploy?"][..],
                "--mesage",
            ),
            (
                &["--agent", "running", "--title", "port", "--bogus"][..],
                "--bogus",
            ),
            (&["--shell-integration", "zsh", "--bogus"][..], "--bogus"),
        ] {
            let complaint = format!(
                "{:#}",
                parse(args).expect_err("a flag nothing reads is an error")
            );
            assert!(
                complaint.starts_with(&format!("unrecognised argument {extra};")),
                "{args:?}: {complaint}"
            );
        }

        // What an agent appends is not a flag, and is left alone: Codex's
        // older `notify` hands the program a JSON payload after its own
        // arguments.
        let mut appended = ["idle", r#"{"type":"agent-turn-complete"}"#]
            .map(str::to_owned)
            .into_iter()
            .peekable();
        assert_eq!(
            agent_arguments(&mut appended).expect("a payload an agent appends is let through"),
            AgentArguments {
                status: "idle".to_owned(),
                title: None,
                message: None,
                pull_request: None,
            }
        );
    }

    #[test]
    fn a_pull_request_is_read_beside_any_status_and_nowhere_else() {
        // Beside the other two, in any order, once — the shape `--title` and
        // `--message` have, which is what a hook author already knows.
        let mut given = [
            "running",
            "--pull-request",
            "https://github.com/o/r/pull/7",
            "--title",
            "port the tab bar",
        ]
        .map(str::to_owned)
        .into_iter()
        .peekable();
        assert_eq!(
            agent_arguments(&mut given).expect("valid"),
            AgentArguments {
                status: "running".to_owned(),
                title: Some("port the tab bar".to_owned()),
                message: None,
                pull_request: Some("https://github.com/o/r/pull/7".to_owned()),
            }
        );

        for (args, complaint) in [
            (
                &["--agent", "running", "--pull-request"][..],
                "`--pull-request` needs",
            ),
            (
                &[
                    "--agent",
                    "running",
                    "--pull-request",
                    "-",
                    "--pull-request",
                    "-",
                ][..],
                "was given twice",
            ),
            (
                &["--pull-request", "https://github.com/o/r/pull/7"][..],
                "goes after `--agent <status>`",
            ),
            // Refused by the report before it opens a terminal, so this
            // needs none either.
            (
                &[
                    "--agent",
                    "running",
                    "--pull-request",
                    "http://github.com/o/r/pull/7",
                ][..],
                "https://",
            ),
        ] {
            let said = format!("{:#}", parse(args).expect_err("refused"));
            assert!(said.contains(complaint), "{args:?}: {said}");
        }
    }

    #[test]
    fn every_command_the_shipped_tables_bind_has_a_line_in_the_help() {
        // The other direction for the keys: the tables grew a find bar, the
        // numbered tabs, the pane focus chords and the paging chords, and the
        // lists in --help stayed at the ten they were written with — one of
        // them without the block chords the other had. Every command a table
        // binds has one of its chords in the help, spelled as the table
        // spells it, so a chord the table gains is a line the help owes.
        let help = help_text();
        for (platform, table) in [
            ("macOS", crate::keybindings::DEFAULTS_MAC),
            ("Linux", crate::keybindings::DEFAULTS_OTHER),
        ] {
            let mut commands: Vec<&str> = table.iter().map(|(_, command)| *command).collect();
            commands.sort_unstable();
            commands.dedup();
            for command in commands {
                // Through the parser and back: the help spells a chord the way
                // the page prints it, and the page prints `format_chord`.
                let mentioned = table
                    .iter()
                    .filter(|(_, bound)| *bound == command)
                    .map(|(chord, _)| {
                        crate::keybindings::format_chord(
                            &crate::keybindings::parse_chord(chord).expect("a shipped chord"),
                        )
                    })
                    .any(|chord| help.contains(&chord) || spelled_out(&chord, &help));
                assert!(
                    mentioned,
                    "{platform}: no chord of {command} is in --help's KEYS"
                );
            }
        }
    }

    /// Whether the help spells `chord` some way other than the table's: a
    /// pair of keys on one line (`left/right`, `pageup/pagedown`), the four
    /// arrows as a family, a run of digits as a range, `=` and `-` as the
    /// words on the keys.
    fn spelled_out(chord: &str, help: &str) -> bool {
        let (modifiers, key) = chord.rsplit_once('+').unwrap_or(("", chord));
        let spellings: &[String] = match key {
            "left" | "right" => &[
                format!("{modifiers}+left/right"),
                format!("{modifiers}+arrows"),
            ],
            "up" | "down" => &[
                format!("{modifiers}+up/down"),
                format!("{modifiers}+arrows"),
            ],
            "pageup" | "pagedown" => &[format!("{modifiers}+pageup/pagedown")],
            "2" | "3" | "4" | "5" | "6" | "7" | "8" => &[format!("{modifiers}+1 … {modifiers}+8")],
            "=" | "+" => &[format!("{modifiers}+plus")],
            "-" | "_" => &[format!("{modifiers}+minus")],
            _ => &[],
        };
        spellings.iter().any(|spelling| help.contains(spelling))
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
            "a pool the size of the poll chains had a worker to spare, so this test is no \
             longer describing the machinery it names"
        );
        assert!(
            a_worker_is_left_over(PARKED_WORKERS + 1, std::time::Duration::from_secs(30)),
            "one worker per parked chain plus one was not enough to run a settings save"
        );

        // And that the floor is actually applied, on the machines that need
        // it rather than on whichever one is running the suite. Everything
        // above is about a pool the test built for itself; this is the only
        // line about the pool the application builds.
        for cores in 1..=PARKED_WORKERS {
            assert!(
                pool_size(cores) > PARKED_WORKERS,
                "a {cores}-core machine would get a pool with nothing left to run a settings \
                 save on"
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
    fn a_run_that_types_into_the_field_is_told_apart_from_one_that_does_not() {
        // The two flags a restored resume line is taken out of the field
        // for, because both type after whatever the field already holds.
        assert!(!Overrides::default().types_into_the_field());
        assert!(
            Overrides {
                run: vec!["git status".to_owned()],
                ..Overrides::default()
            }
            .types_into_the_field()
        );
        assert!(
            Overrides {
                type_text: Some("x".to_owned()),
                ..Overrides::default()
            }
            .types_into_the_field()
        );
        // Selecting output types nothing, and leaves the field's line alone.
        assert!(
            !Overrides {
                select_output: Some("x".to_owned()),
                find_output: Some("x".to_owned()),
                ..Overrides::default()
            }
            .types_into_the_field()
        );
    }

    #[test]
    fn help_and_version_answer_before_anything_is_opened() {
        assert_eq!(parse(&["--help"]).expect("valid"), Startup::Answered);
        assert_eq!(parse(&["-V"]).expect("valid"), Startup::Answered);
    }

    #[test]
    fn plugins_says_what_the_registry_thinks_of_each_installed_version() {
        // The two questions a person has before reporting or updating
        // something: which version am I running, and is it the one the
        // registry has. From the copy on disk, so it is answered offline —
        // and from nothing when there is no copy.
        use crate::plugins::store::cache::Cache;
        use crate::plugins::wasm::tests::{Scratch, install, wasm};

        let plugins = Scratch::new("plugins-text");
        install(
            plugins.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let settings = Settings::ephemeral();

        let empty = Scratch::new("plugins-text-empty");
        let line = installed_plugins_in(plugins.path(), &settings, Some(Cache::at(empty.path())));
        assert!(line.starts_with("eugen/probe  0.1.0  "), "{line}");
        assert!(line.ends_with("plugin.wasm"), "{line}");

        let ahead = Scratch::new("plugins-text-ahead");
        Cache::at(ahead.path())
            .write(
                br#"{"schema": 1, "plugins": [
                     {"id": "eugen/probe", "name": "Probe", "description": "d",
                      "versions": [{"version": "0.4.0", "abi": 8, "url": "https://x.invalid/p.wasm",
                                    "sha256": "aa"}]}]}"#,
                None,
            )
            .expect("the scratch index writes");
        let line = installed_plugins_in(plugins.path(), &settings, Some(Cache::at(ahead.path())));
        assert!(
            line.ends_with("plugin.wasm  (0.4.0 in the registry)"),
            "{line}"
        );

        // And a withdrawn version says so as well as what replaces it, after
        // the switched-off word when both apply.
        let mut off = Settings::ephemeral();
        off.set_plugin_disabled("eugen/probe", true);
        Cache::at(ahead.path())
            .write(
                br#"{"schema": 1, "plugins": [
                     {"id": "eugen/probe", "name": "Probe", "description": "d",
                      "versions": [{"version": "0.1.0", "abi": 8, "url": "https://x.invalid/a.wasm",
                                    "sha256": "aa", "yanked": "it read the wrong file"},
                                   {"version": "0.0.9", "abi": 8, "url": "https://x.invalid/b.wasm",
                                    "sha256": "bb"}]}]}"#,
                None,
            )
            .expect("the scratch index writes");
        let line = installed_plugins_in(plugins.path(), &off, Some(Cache::at(ahead.path())));
        assert!(
            line.ends_with(
                "plugin.wasm  (switched off)  (withdrawn: it read the wrong file)  (0.0.9 in the \
                 registry)"
            ),
            "{line}"
        );
    }

    #[test]
    fn plugins_are_listed_in_columns_that_line_up() {
        // A short id and a long one, so the version — the field a person
        // scans for — starts at the same column on both lines rather than
        // wherever each id happened to end.
        use crate::plugins::wasm::tests::{Scratch, install, wasm};

        let plugins = Scratch::new("plugins-aligned");
        install(
            plugins.path(),
            "eugen.a",
            &wasm("eugen/a", "header.right", 10),
        );
        install(
            plugins.path(),
            "eugen.longer",
            &wasm("eugen/longer-name", "header.right", 11),
        );
        let text = installed_plugins_in(plugins.path(), &Settings::ephemeral(), None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "two plugins, two lines: {text}");

        let version_at = |line: &str| line.find("0.1.0").expect("every line carries the version");
        assert_eq!(
            version_at(lines[0]),
            version_at(lines[1]),
            "the version column does not line up:\n{text}"
        );

        let short = lines
            .iter()
            .find(|line| line.starts_with("eugen/a "))
            .expect("the short id is listed");
        assert!(
            short.starts_with("eugen/a           "),
            "the short id was not padded to the long one: {short:?}"
        );
    }

    #[test]
    fn plugins_as_json_lists_what_the_columns_list() {
        // The same plugins the columns show, as objects with the keys the
        // help promises — so a script reads `id` and `path` rather than
        // counting spaces, and reads the grant as the settings hold it.
        use crate::plugins::wasm::tests::{Scratch, install, wasm};

        let plugins = Scratch::new("plugins-json");
        install(
            plugins.path(),
            "eugen.a",
            &wasm("eugen/a", "header.right", 10),
        );
        // In the directory an install writes, `owner.name`: the path is
        // looked up by id, and a directory called anything else is a
        // plugin with no path to name.
        install(
            plugins.path(),
            "eugen.longer-name",
            &wasm("eugen/longer-name", "header.right", 11),
        );
        let mut settings = Settings::ephemeral();
        settings.set_plugin_disabled("eugen/a", true);
        settings.set_granted(
            "eugen/longer-name",
            vec![String::from("net:api.github.com")],
        );

        let text = installed_plugins_json_in(plugins.path(), &settings);
        let listed: Vec<serde_json::Value> =
            serde_json::from_str(&text).unwrap_or_else(|why| panic!("not JSON ({why}): {text}"));

        let columns = installed_plugins_in(plugins.path(), &settings, None);
        let in_columns: Vec<&str> = columns
            .lines()
            .filter_map(|line| line.split(' ').next())
            .collect();
        let in_json: Vec<&str> = listed
            .iter()
            .map(|entry| entry["id"].as_str().expect("`id` is a string"))
            .collect();
        assert_eq!(
            in_json, in_columns,
            "the two listings disagree:\n{columns}\n{text}"
        );

        for entry in &listed {
            // Sorted, since a parsed object keeps its keys in no order a
            // test should pin — a script reads them by name.
            let object = entry.as_object().expect("one object per plugin");
            let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                ["allowed", "enabled", "id", "path", "version"],
                "the keys a script relies on: {entry}"
            );
            assert_eq!(entry["version"], "0.1.0");
            assert!(
                entry["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("plugin.wasm")),
                "the path names the module: {entry}"
            );
        }
        assert_eq!(listed[0]["enabled"], false, "eugen/a is switched off");
        assert_eq!(listed[0]["allowed"], serde_json::json!([]));
        assert_eq!(listed[1]["enabled"], true);
        assert_eq!(
            listed[1]["allowed"],
            serde_json::json!(["net:api.github.com"]),
            "the grant as the settings hold it"
        );
    }

    #[test]
    fn plugins_as_json_is_an_empty_array_when_nothing_is_installed() {
        // `[]` and not a sentence: a script parses stdout as JSON or not at
        // all, and the sentence is for the columns.
        use crate::plugins::wasm::tests::Scratch;

        let plugins = Scratch::new("plugins-json-empty");
        let text = installed_plugins_json_in(plugins.path(), &Settings::ephemeral());
        let listed: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(listed, serde_json::json!([]), "{text}");
    }

    #[test]
    fn json_without_plugins_is_an_argument_error() {
        // A `--json` with nothing to print as JSON is refused in the parser's
        // own voice, and it names the flag it goes with — not "unrecognised",
        // which would say the flag does not exist.
        let error = parse(&["--json"]).expect_err("nothing to print as JSON");
        let text = error.to_string();
        assert!(text.contains("--plugins"), "{text}");
        assert!(!text.contains("unrecognised"), "{text}");
        assert!(parse(&["--json", "--frames", "1"]).is_err());
    }

    #[test]
    fn the_plugin_being_written_replaces_the_installed_copy_of_itself() {
        // `crook --dev-plugin .` on a plugin that is also installed: the
        // window used to open carrying both, drawing the chip twice and
        // listing the plugin twice until the watcher's first rebuild
        // replaced one by id. It opens with the one being written.
        use crate::plugins::wasm::tests::{Scratch, install, wasm, wasm_at};

        let installed = Scratch::new("dev-replaces");
        install(
            installed.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let building = Scratch::new("dev-replaces-build");
        let module = building.path().join("plugin.wasm");
        std::fs::write(&module, wasm_at("eugen/probe", "0.2.0")).expect("the module writes");

        let plugins = with_dev_plugin(
            crate::plugins::wasm::installed(installed.path()),
            Some(&module),
        )
        .expect("the module opens");

        let probes: Vec<&str> = plugins
            .iter()
            .map(|plugin| plugin.manifest())
            .filter(|manifest| manifest.id.as_str() == "eugen/probe")
            .map(|manifest| manifest.version)
            .collect();
        assert_eq!(
            probes,
            ["0.2.0"],
            "one probe, and it is the one being written"
        );
    }

    /// The overrides a command line opens its window with, or a panic naming
    /// what it opened instead.
    fn window_overrides(args: &[&str]) -> Overrides {
        match parse(args) {
            Ok(Startup::Window { overrides, .. }) => overrides,
            other => panic!("{args:?} did not open a window: {other:?}"),
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn everything_after_dash_e_is_the_command_however_its_words_are_spelled() {
        // xterm's rule, and alacritty's and foot's: a launcher hands over the
        // program and its arguments as separate words, and a word that looks
        // like one of Crook's own flags is the program's all the same. `--`
        // says the same thing for a launcher that spells it that way.
        for dash_e in ["-e", "--"] {
            let overrides = window_overrides(&[
                "--title",
                "notes",
                dash_e,
                "vim",
                "my notes.txt",
                "--help",
                "-R",
            ]);
            assert_eq!(
                overrides.command,
                ["vim", "my notes.txt", "--help", "-R"],
                "{dash_e}"
            );
            assert_eq!(
                overrides.window_title.as_deref(),
                Some("notes"),
                "the flags in front of {dash_e} are still Crook's"
            );
            assert!(overrides.launched(), "{dash_e} is a launcher's window");
        }

        for bare in ["-e", "--"] {
            let complaint = format!("{:#}", parse(&[bare]).expect_err("a command is needed"));
            assert!(complaint.contains("needs"), "{bare}: {complaint}");
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn a_word_with_a_line_break_in_it_is_refused_rather_than_typed_as_two_lines() {
        // The command is typed into the pane a key at a time, and a line break
        // is Return: `printf 'a` would be sent on its own and the shell left
        // waiting for the rest of a quote. A tab is a completion request.
        for word in ["two\nlines", "a\ttab"] {
            let complaint = format!(
                "{:#}",
                parse(&["-e", "printf", word]).expect_err("a control character")
            );
            assert!(
                complaint.contains("control character") && complaint.contains(&format!("{word:?}")),
                "{complaint}"
            );
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn a_command_longer_than_a_terminal_line_is_refused_rather_than_cut_short() {
        // Typed before the shell is reading, the line waits in the pty's line
        // discipline, which keeps its first `TYPED_LINE_LIMIT` bytes and
        // still takes the Return: `vim` would open a file whose name was cut
        // off. What fits is typed.
        let prefix = exec_line(&["vim".to_owned(), String::new()]).len() - "''".len();
        let fits = "a".repeat(TYPED_LINE_LIMIT - prefix);
        assert_eq!(
            exec_line(&["vim".to_owned(), fits.clone()]).len(),
            TYPED_LINE_LIMIT
        );
        assert_eq!(
            window_overrides(&["-e", "vim", &fits]).command,
            ["vim", fits.as_str()]
        );

        let over = format!("{fits}a");
        let complaint = format!(
            "{:#}",
            parse(&["-e", "vim", &over]).expect_err("a line the terminal would cut")
        );
        assert!(
            complaint.contains(&(TYPED_LINE_LIMIT + 1).to_string())
                && complaint.contains(&TYPED_LINE_LIMIT.to_string()),
            "{complaint}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_word_that_is_not_text_is_an_error_and_not_a_crash() {
        // A folder name on Linux is bytes, and a file manager hands it over
        // as it is: `std::env::args` panicked on this one.
        use std::os::unix::ffi::OsStringExt;

        let folder = std::ffi::OsString::from_vec(b"/tmp/caf\xe9".to_vec());
        let complaint = format!(
            "{:#}",
            command_line(["--working-directory".into(), folder].into_iter())
                .expect_err("a word that is not UTF-8")
        );
        assert!(
            complaint.contains(r"caf\xE9") && complaint.contains("UTF-8"),
            "{complaint}"
        );

        let words = ["--cwd", "/tmp/café", "-e", "htop"];
        assert_eq!(
            command_line(words.map(std::ffi::OsString::from).into_iter()).expect("all text"),
            words
        );
    }

    #[test]
    fn the_command_is_typed_as_an_exec_after_every_run() {
        // With a space in front, which keeps the line out of the history the
        // field's suggestions are read from.
        assert_eq!(exec_line(&["htop".to_owned()]), " exec htop");
        assert_eq!(
            exec_line(&["vim", "my notes.txt"].map(str::to_owned)),
            " exec vim 'my notes.txt'"
        );

        // Last, since what comes after it is typed into the program.
        let overrides = Overrides {
            run: vec!["git status".to_owned()],
            command: ["htop", "-d", "10"].map(str::to_owned).to_vec(),
            ..Overrides::default()
        };
        assert_eq!(overrides.typed(), ["git status", " exec htop -d 10"]);
        assert!(Overrides::default().typed().is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn every_shell_reads_a_typed_word_back_as_exactly_the_word_it_was() {
        // The words a launcher hands over, through each shell a pane can be:
        // POSIX sh (dash on Ubuntu), bash, zsh — whose `=word` is a path
        // lookup — and fish, whose single quotes take a backslash escape
        // where the others' do not. A shell this machine lacks is skipped;
        // `sh` never is.
        let words = [
            "plain",
            "two words",
            "",
            "it's",
            r"back\slash",
            r"\\",
            r"\'",
            "$HOME",
            "`id`",
            "$(id)",
            "*",
            "~",
            "=cat",
            "a'b\"c",
            "--flag=value",
            "semi;colon",
            "#hash",
            "{a,b}",
            "%self",
            "!!",
            "(paren)",
            "ünïcode",
        ];
        let line = words
            .iter()
            .map(|word| shell_word(word))
            .collect::<Vec<_>>()
            .join(" ");
        let mut heard = 0;
        for shell in ["sh", "bash", "zsh", "fish"] {
            let Ok(output) = crate::process::command(shell)
                .arg("-c")
                .arg(format!("printf '%s\\n' {line}"))
                .output()
            else {
                assert_ne!(shell, "sh", "a Unix machine with no sh");
                continue;
            };
            heard += 1;
            let printed = String::from_utf8_lossy(&output.stdout);
            let read_back: Vec<&str> = printed.lines().collect();
            assert_eq!(
                read_back,
                words,
                "{shell} read `{line}` back as something else (stderr: {})",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(heard > 0);
    }

    #[test]
    fn a_working_directory_that_is_not_there_is_an_error_and_not_a_window() {
        use crate::plugins::wasm::tests::Scratch;

        let scratch = Scratch::new("working-directory");
        let missing = scratch.path().join("deleted-since");
        let complaint = format!(
            "{:#}",
            parse(&["--working-directory", &missing.display().to_string()])
                .expect_err("a directory that is not there")
        );
        assert!(
            complaint.contains("deleted-since"),
            "the error does not name the directory: {complaint}"
        );

        let file = scratch.path().join("a-file");
        std::fs::write(&file, "").expect("writable scratch");
        assert!(
            parse(&["--cwd", &file.display().to_string()]).is_err(),
            "a file is not somewhere a shell can start"
        );
        assert!(parse(&["--working-directory"]).is_err());

        // Compared as `absolute` spells it, which on Windows is the system's
        // own spelling of a full path rather than whatever the scratch was
        // joined as.
        let spelled = std::path::absolute(scratch.path()).expect("a full path");
        for flag in ["--working-directory", "--cwd"] {
            let overrides = window_overrides(&[flag, &scratch.path().display().to_string()]);
            assert_eq!(overrides.working_directory, Some(spelled.clone()), "{flag}");
            assert!(overrides.launched(), "{flag} is a launcher's window");
        }
    }

    #[test]
    fn a_relative_path_beside_a_working_directory_is_read_from_where_crook_was_started() {
        // The window moves the whole process into the directory, so a path
        // typed relative to where Crook was started has to be made whole
        // first or it would be read against the wrong one.
        use crate::plugins::wasm::tests::Scratch;

        let scratch = Scratch::new("anchored");
        let overrides = window_overrides(&[
            "--dev-plugin",
            "build/probe.wasm",
            "--working-directory",
            &scratch.path().display().to_string(),
            "--shell",
            "./my-shell",
        ]);
        let here = |path| std::path::absolute(path).expect("a current directory");
        assert_eq!(overrides.dev_plugin, Some(here("build/probe.wasm")));
        assert_eq!(overrides.shell, Some(here("./my-shell")));

        // A bare name is a `PATH` lookup, and stays one; and without a
        // directory nothing moves, so nothing is rewritten.
        let named = window_overrides(&[
            "--cwd",
            &scratch.path().display().to_string(),
            "--shell",
            "fish",
        ]);
        assert_eq!(named.shell, Some(PathBuf::from("fish")));
        let unmoved = window_overrides(&["--dev-plugin", "build/probe.wasm"]);
        assert_eq!(unmoved.dev_plugin, Some(PathBuf::from("build/probe.wasm")));

        // Except the shell, which every pane starts from a directory of its
        // own: a relative one is made whole with or without a move.
        let shell = window_overrides(&["--shell", "bin/zsh"]);
        assert_eq!(shell.shell, Some(here("bin/zsh")));
    }

    #[test]
    fn title_names_the_window_where_it_would_say_crook() {
        let overrides = window_overrides(&["--title", "scratch pad"]);
        assert_eq!(overrides.window_name(Channel::Stable), "scratch pad");
        // The tab still goes in front of it, the way it goes in front of
        // "Crook": the title is the window's name, not a lid on it.
        assert_eq!(
            window_title(Some("vim"), 0, &overrides.window_name(Channel::Stable)),
            "vim — scratch pad"
        );
        assert_eq!(Overrides::default().window_name(Channel::Stable), "Crook");
        assert_eq!(
            Overrides::default().window_name(Channel::Dev),
            "Crook (dev)"
        );
        assert!(!overrides.launched(), "a title alone is an ordinary window");
        assert!(parse(&["--title"]).is_err());

        // In front of `--agent` it is still a status report's title put in
        // the wrong place, and still refused rather than dropped.
        let misplaced = parse(&["--title", "port", "--agent", "running"])
            .expect_err("a title before the status it belongs to");
        assert!(
            format!("{misplaced:#}").contains("goes after `--agent"),
            "{misplaced:#}"
        );
    }

    #[test]
    fn the_window_is_opened_with_the_title_and_the_app_id_the_command_line_named() {
        let overrides = window_overrides(&["--title", "notes", "--app-id", "crook-notes"]);
        let options = window_options(Channel::Stable, &overrides, &Default::default());
        assert_eq!(options.title, "notes");
        assert_eq!(options.app_id, "crook-notes");

        let plain = window_options(Channel::Stable, &Overrides::default(), &Default::default());
        assert_eq!(plain.title, "Crook");
        assert_eq!(plain.app_id, "crook", "the id every window rule matches");

        assert!(parse(&["--app-id", ""]).is_err(), "an empty id is no id");
        assert!(parse(&["--app-id"]).is_err());
    }

    #[test]
    fn a_window_a_launcher_opened_does_not_come_back_as_the_last_one() {
        use crate::plugins::wasm::tests::Scratch;

        let scratch = Scratch::new("launched-session");
        std::fs::write(
            scratch.path().join("session.json"),
            r#"{ "tabs": [ { "name": "the agents", "panes": [ { "title": "agent 1" } ] } ] }"#,
        )
        .expect("writable scratch");
        let settings = Settings::load(scratch.path().join("settings.json"));
        assert!(
            !opening_session(&settings, &Overrides::default()).is_empty(),
            "an ordinary launch comes back as the last window"
        );

        for launched in [
            Overrides {
                command: vec!["htop".to_owned()],
                ..Overrides::default()
            },
            Overrides {
                working_directory: Some(scratch.path().to_owned()),
                ..Overrides::default()
            },
        ] {
            assert!(
                opening_session(&settings, &launched).is_empty(),
                "{launched:?} opened as the last window"
            );
        }
    }

    #[test]
    fn a_launchers_flags_open_a_window_and_are_refused_beside_a_snapshot() {
        use crate::plugins::wasm::tests::Scratch;

        let scratch = Scratch::new("launched-snapshot");
        let directory = scratch.path().display().to_string();
        let mut asked = vec![["--working-directory", directory.as_str()]];
        if cfg!(not(windows)) {
            asked.push(["-e", "htop"]);
        }
        for [flag, value] in asked {
            let complaint = format!(
                "{:#}",
                parse(&["--snapshot", "frame.png", flag, value])
                    .expect_err("a picture has no launcher")
            );
            assert!(
                complaint.contains("--snapshot") && complaint.contains(flag),
                "{flag}: {complaint}"
            );
        }
    }

    #[test]
    fn the_desktop_entry_parses_and_hands_crook_only_flags_it_knows() {
        // A line-based read of the file the archive, the AUR package and
        // `script/install --desktop` all install: one group, `key=value`
        // lines, no key twice. `desktop-file-validate` checks more, and is
        // what was run by hand; this is what fails in `cargo test` when the
        // entry and the binary drift apart.
        const ENTRY: &str = include_str!("../../packaging/linux/crook.desktop");
        const METAINFO: &str = include_str!("../../packaging/linux/id.crook.Crook.metainfo.xml");

        let mut group = None;
        let mut keys = std::collections::BTreeMap::new();
        for (number, line) in ENTRY.lines().enumerate() {
            let number = number + 1;
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                assert!(group.is_none(), "line {number}: a second group, {name}");
                group = Some(name);
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .unwrap_or_else(|| panic!("line {number}: {line:?} is not key=value"));
            assert_eq!(
                group,
                Some("Desktop Entry"),
                "line {number}: {key} is outside the group"
            );
            assert!(
                !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "line {number}: {key:?} is not a key"
            );
            assert!(
                keys.insert(key, value).is_none(),
                "line {number}: {key} twice"
            );
        }

        assert_eq!(keys.get("Type"), Some(&"Application"));
        assert_eq!(keys.get("Name"), Some(&"Crook"));
        assert_eq!(keys.get("Exec"), Some(&"crook"));
        assert_eq!(keys.get("TryExec"), Some(&"crook"));
        assert_eq!(keys.get("Icon"), Some(&"crook"));
        assert_eq!(keys.get("Terminal"), Some(&"false"));
        let categories = keys
            .get("Categories")
            .expect("a launcher files it somewhere");
        assert!(
            categories.ends_with(';')
                && categories.split(';').any(|name| name == "TerminalEmulator"),
            "Categories={categories}"
        );
        assert_eq!(
            keys.get("StartupWMClass").copied(),
            Some(WindowOptions::default().app_id.as_str()),
            "a dock ties the window to this entry by the id the window is opened with"
        );

        // What `xdg-terminal-exec --app-id=a --title=t --dir=d cmd arg` runs,
        // built the way it builds it: each flag the entry names, then its
        // value, in front of the command.
        #[cfg(not(windows))]
        {
            use crate::plugins::wasm::tests::Scratch;

            let scratch = Scratch::new("desktop-entry");
            let directory = scratch.path().display().to_string();
            let flag = |key: &str| {
                *keys
                    .get(key)
                    .unwrap_or_else(|| panic!("the entry has no {key}"))
            };
            let overrides = window_overrides(&[
                flag("X-TerminalArgAppId"),
                "crook-scratch",
                flag("X-TerminalArgTitle"),
                "scratch",
                flag("X-TerminalArgDir"),
                &directory,
                flag("X-TerminalArgExec"),
                "htop",
                "-d",
                "10",
            ]);
            assert_eq!(overrides.app_id.as_deref(), Some("crook-scratch"));
            assert_eq!(overrides.window_title.as_deref(), Some("scratch"));
            assert_eq!(overrides.working_directory.as_deref(), Some(scratch.path()));
            assert_eq!(overrides.command, ["htop", "-d", "10"]);
        }

        // And the AppStream file is about this entry and this binary.
        assert!(
            METAINFO.contains(r#"<launchable type="desktop-id">crook.desktop</launchable>"#),
            "the metainfo does not name the desktop entry"
        );
        assert!(METAINFO.contains("<binary>crook</binary>"));
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
