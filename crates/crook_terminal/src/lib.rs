//! A terminal: a shell on a pty, an emulator consuming its output, and a
//! snapshot the renderer can draw.
//!
//! ```no_run
//! use std::io::Read as _;
//! use std::sync::mpsc;
//! use std::thread;
//!
//! use crook_terminal::{Terminal, TerminalOptions};
//!
//! # fn main() -> anyhow::Result<()> {
//! let mut terminal = Terminal::spawn(TerminalOptions::default())?;
//!
//! // The reader blocks, so it lives on a thread of its own and posts what it
//! // reads back to whichever thread owns the terminal.
//! let (output, from_shell) = mpsc::channel();
//! let mut reader = terminal.take_reader().expect("a fresh terminal has its reader");
//! thread::spawn(move || {
//!     let mut buffer = [0; 4096];
//!     while let Ok(read) = reader.read(&mut buffer) {
//!         if read == 0 || output.send(buffer[..read].to_vec()).is_err() {
//!             return;
//!         }
//!     }
//! });
//!
//! for chunk in from_shell {
//!     terminal.feed(&chunk)?;
//!     let screen = terminal.snapshot();
//!     // Draw `screen`, which is owned and needs no lock, then handle
//!     // `terminal.take_events()` — a bell, a new title, a finished child.
//!     let _ = screen;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## What this crate does not do
//!
//! It owns no thread, no executor and no timer. `Terminal` is a value with
//! methods; every one of them runs on the thread that called it and returns.
//! Deciding which thread reads the pty, which writes it, how often the grid is
//! repainted and when the child is reaped is the application's job, in the
//! same way `crook_wasm` records what a plugin asked for and leaves the doing
//! of it to whoever drives the sandbox — only the application knows which of
//! its threads is allowed to block.
//!
//! It also draws nothing and knows nothing about fonts, windows or the GPU. The
//! contract with the renderer is [`Snapshot`], which is plain owned data with
//! every colour already resolved to RGB.
//!
//! ## The pieces
//!
//! * [`Pty`] spawns the shell and owns both ends of the pseudo-terminal.
//! * [`PtyReader`] and [`PtyWriter`] are its two halves as a thread of the
//!   application's holds them: the reader blocks until the child speaks, the
//!   writer until it listens.
//! * [`PtyLink`] is what a [`Terminal`] asks of the far end — [`Pty`] is the
//!   local one — which is what lets a whole session be driven over something
//!   that is not a process at all.
//! * [`Emulator`] turns bytes into a grid. It works with no pty at all, which
//!   is what makes this crate testable without a process.
//! * [`Snapshot`] is what the renderer draws.
//! * [`Block`] is one command, its output and how it ended, with its rows owned
//!   rather than borrowed from the grid. [`Terminal::blocks`] is the history;
//!   [`Snapshot::live_block`] is the one still open.
//! * [`Rows`] is one block's rows, out of either store, which is how anything
//!   that reads a block's text avoids being written twice.
//! * [`input`] encodes key presses into the bytes a shell expects.
//! * [`selection`] is the vocabulary a pointer selects text with.
//!
//! ## Selection, and why none of it is here
//!
//! A pane's output is a list of blocks, and all but the last of them were
//! harvested out of the grid when their commands ended — so a selection
//! anchored to a cell of the grid could only ever cover the newest of them.
//! It is anchored to a block and a row of that block instead, in the list that
//! draws them, one level above this crate. What this supplies is the two words
//! both sides use — [`SelectionKind`] and [`CellSide`] — and the one way in to
//! a block's text whichever store it is in, [`Rows`], which is also what
//! [`Terminal::harvest_rows`] hands back for a pane drawing one grid.

pub mod agent;
mod blocks;
mod emulator;
mod harvest;
pub mod image;
pub mod input;
mod link;
mod marks;
pub mod mouse;
#[cfg(unix)]
mod nonblocking;
pub mod notify;
pub mod path;
mod pty;
mod rows;
pub mod selection;
mod snapshot;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod typeahead;
pub mod url;
mod writer;

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

pub use crate::agent::{AgentReport, Reported};
pub use crate::blocks::{Block, BlockId, BlockState, IgnoreReason, LiveBlock, PromptEnd};
pub use crate::emulator::{Emulator, Fed, TerminalEvent};
pub use crate::harvest::{BlockRows, RowCombining, StyleRun};
pub use crate::image::{ImagePlacement, InlineImage};
pub use crate::input::{InputModes, Key, KeyboardModes, KeypadKey, Modifiers};
pub use crate::link::PtyLink;
pub use crate::marks::{PromptKind, ShellMark};
pub use crate::mouse::{MouseButton, MouseEventKind, MouseModes};
pub use crate::notify::Notification;
pub use crate::pty::{ChildExit, Program, Pty, PtyReader, default_shell, login_arguments};
pub use crate::rows::Rows;
pub use crate::selection::{CellSide, SelectionKind};
pub use crate::snapshot::{
    CellCombining, CellFlags, Cursor, CursorShape, Palette, Rgb, Snapshot, SnapshotCell,
    TerminalSize,
};
use crate::writer::Outbox;
pub use crate::writer::PtyWriter;

/// How much output a terminal remembers above the viewport.
const DEFAULT_SCROLLBACK_LINES: usize = 10_000;

/// The key press that asks an integrated shell for completions.
///
/// A `CSI n ~` with a number nothing has ever assigned, which is what makes it
/// bindable: every shell's line editor can be told what `\e[6339~` means, and
/// none of them already thinks it means something. See
/// [`Terminal::request_completions`].
pub const COMPLETION_REQUEST: &[u8] = b"\x1b[6339~";

/// Everything a new terminal needs to know.
#[derive(Clone, Debug, Default)]
pub struct TerminalOptions {
    /// The grid to start at. The child is told this size before it starts, so
    /// its first prompt is already laid out correctly.
    pub size: TerminalSize,
    /// What to run. Defaults to the user's shell.
    pub program: Program,
    /// Where to run it. When unset — or when it names a directory that has
    /// since been removed, which is what a restored session can hand over — the
    /// child starts in `$HOME`, the way a terminal opens a new window.
    pub working_directory: Option<PathBuf>,
    /// Environment variables applied on top of the inherited ones.
    pub environment: Vec<(String, String)>,
    /// Lines of scrollback to keep. Defaults to [`DEFAULT_SCROLLBACK_LINES`].
    pub scrollback_lines: Option<usize>,
    /// The colours the grid is drawn in.
    pub palette: Palette,
    /// What the terminal tells a program that asks which terminal it is:
    /// see [`Emulator::set_name_and_version`]. `None` leaves the question
    /// unanswered.
    pub name_and_version: Option<String>,
}

/// A running terminal session.
///
/// Owns the emulator and the link to the child — a [`Pty`] of its own, for
/// every terminal [`Self::spawn`] opens. Everything about how it is driven —
/// the thread that reads, the frame that paints, the moment the child is
/// reaped — belongs to the caller.
pub struct Terminal {
    emulator: Emulator,
    link: Box<dyn PtyLink>,
    input: Input,
    exit: Option<ChildExit>,
}

/// Where a terminal's writes go.
enum Input {
    /// Straight into the link's writer, on the calling thread, which waits for
    /// as long as the child is not reading. What a terminal starts with.
    Direct(Box<dyn Write + Send>),
    /// Into the queue a [`PtyWriter`] writes from on a thread of its own, once
    /// [`Terminal::take_writer`] has handed it out. Nothing here waits.
    Queued(Outbox),
}

impl fmt::Debug for Terminal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Terminal")
            .field("link", &self.link)
            .field("size", &self.size())
            .field("title", &self.title())
            .finish_non_exhaustive()
    }
}

impl Terminal {
    /// Opens a pty, starts the program on it, and returns the terminal that
    /// drives them.
    pub fn spawn(options: TerminalOptions) -> Result<Self> {
        let TerminalOptions {
            size,
            program,
            working_directory,
            environment,
            scrollback_lines,
            palette,
            name_and_version,
        } = options;

        let pty = Pty::spawn(&program, size, working_directory.as_deref(), &environment)?;
        let mut emulator = Emulator::new(
            size,
            scrollback_lines.unwrap_or(DEFAULT_SCROLLBACK_LINES),
            palette,
        );
        emulator.set_name_and_version(name_and_version.as_deref());
        Self::with_link(Box::new(pty), emulator)
    }

    /// A terminal whose child is on the far end of `link`, drawn by
    /// `emulator`.
    ///
    /// The two are expected to agree on the size: the link's child was
    /// started at one, and the emulator's grid is the one the terminal reports
    /// and resizes from.
    pub fn with_link(link: Box<dyn PtyLink>, emulator: Emulator) -> Result<Self> {
        // Taken once and kept, which is what keeps every keystroke off the
        // link: a write goes straight to this.
        let writer = link.writer()?;
        Ok(Self {
            emulator,
            link,
            input: Input::Direct(writer),
            exit: None,
        })
    }

    /// The readable half of the link, once. Move it to a thread and read
    /// until it returns zero bytes or errors, feeding what it reads to
    /// [`Self::feed`] on the thread that owns this terminal.
    pub fn take_reader(&mut self) -> Option<PtyReader> {
        self.link.take_reader()
    }

    /// The writable half of the link, once, with a queue in front of it.
    ///
    /// Move it to a thread and call [`PtyWriter::run`] there. From then on no
    /// method here writes: [`Self::write`], every key, paste and line that
    /// goes through it, and the answers [`Self::feed`] owes the child are all
    /// copied onto the queue, in the order they were made, and the call
    /// returns. Which is the point — a write waits for the child to read, a
    /// child that is printing reads nothing until its output has been read,
    /// and the thread that reads it needs whatever lock the caller holds this
    /// terminal under.
    ///
    /// Dropping the terminal ends [`PtyWriter::run`], with anything still
    /// queued dropped, since the child it was for is ended with it. Every
    /// later call returns `None`.
    pub fn take_writer(&mut self) -> Option<PtyWriter> {
        let (outbox, inbox) = writer::queue();
        match std::mem::replace(&mut self.input, Input::Queued(outbox)) {
            Input::Direct(writer) => Some(inbox.into_writer(writer)),
            // Taken already, and the queue its writer drains stays this one.
            queued => {
                self.input = queued;
                None
            }
        }
    }

    /// Consumes output from the child.
    ///
    /// Some of that output is a question, and this answers it: replies the
    /// emulator owes the child are written back to the pty, because a program
    /// that asked for the cursor position or the palette will wait forever
    /// otherwise. Once the writer has been taken they are queued instead, and
    /// a program that asks faster than it reads has the oldest of them dropped
    /// rather than kept without end — see [`PtyWriter`].
    pub fn feed(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.emulator.advance(bytes);
        let replies = self.emulator.take_replies();
        if replies.is_empty() {
            return Ok(());
        }
        match &mut self.input {
            Input::Direct(writer) => {
                writer.write_all(&replies)?;
                writer.flush()
            }
            Input::Queued(outbox) => outbox.reply(replies),
        }
    }

    /// Consumes one item of a stream that some other emulator is also
    /// consuming and answering: draws it, and writes nothing back.
    ///
    /// [`Self::feed`] owes the child an answer to every question in its
    /// output. A terminal following a stream whose own emulator already
    /// answered does not: a second answer would reach the child as input it
    /// never asked for, typed into whatever it is running. The stream carries
    /// the lines submitted into it as items of their own — see [`Fed`] — and
    /// a terminal fed this way once is a mirror for good, as
    /// [`Emulator::advance_mirrored`] says. Nothing in Crook feeds a terminal
    /// this way yet.
    pub fn feed_mirrored(&mut self, fed: Fed<'_>) {
        self.emulator.advance_mirrored(fed);
    }

    /// The visible grid. Cheap to call every frame: the same `Arc`, carrying
    /// the same [`Snapshot::revision`], comes back until the content changes.
    pub fn snapshot(&mut self) -> Arc<Snapshot> {
        self.emulator.snapshot()
    }

    /// Sends bytes to the child as if they had been typed.
    ///
    /// Once the writer has been taken — see [`Self::take_writer`] — they are
    /// queued for it and this returns at once. `Ok` then means queued, not
    /// read: bytes queued for a child that has just gone, before the writer
    /// has found out, are dropped when it does. An error of kind
    /// [`BrokenPipe`](io::ErrorKind::BrokenPipe) means nothing more will reach
    /// the child; one of kind [`WouldBlock`](io::ErrorKind::WouldBlock) means
    /// these bytes were refused, whole, behind a backlog the child has not
    /// read, and what was queued before them is still on its way.
    ///
    /// The emulator is told what was sent, which it needs to know while the
    /// shell's line editor has the keyboard: see [`Emulator::typed`].
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        match &mut self.input {
            Input::Direct(writer) => {
                writer.write_all(bytes)?;
                writer.flush()?;
            }
            Input::Queued(outbox) => outbox.input(bytes)?,
        }
        self.emulator.typed(bytes);
        Ok(())
    }

    /// Runs a command line: writes the line followed by the carriage return
    /// the Enter key sends, then records the block boundary.
    ///
    /// This is the boundary that does not depend on the shell cooperating. A
    /// shell with OSC 133 integration reports where its prompt ended and when
    /// the command started; a shell without it reports nothing, and this is
    /// then the only thing that knows a command was run at all — and it knows
    /// the exact text, where a mark-driven block only has what was echoed.
    ///
    /// The line and its Enter are one write, so a queue that refuses more
    /// input — see [`PtyWriter`] — refuses them together: a line taken
    /// without its Enter would sit typed at the prompt, and a caller that
    /// tried again would type it a second time.
    ///
    /// The boundary comes after the write so that a line that was refused, or
    /// had no child left to go to, opens no block. A block opened for it would
    /// keep its text and its start time, and the next command the shell ran
    /// would be filed under a line it never got — and a line refused behind
    /// the queue's backlog is one the caller is meant to try again. After is
    /// still in time, since what the shell prints in answer reaches the
    /// emulator through [`Self::feed`], which cannot run until this has
    /// returned.
    pub fn submit(&mut self, line: &str) -> io::Result<()> {
        let mut entered = Vec::with_capacity(line.len() + 1);
        entered.extend_from_slice(line.as_bytes());
        entered.push(b'\r');
        self.write(&entered)?;
        self.emulator.command_submitted(line);
        Ok(())
    }

    /// The finished blocks, oldest first. Empty until the first command ends.
    pub fn blocks(&self) -> &[Block] {
        self.emulator.blocks()
    }

    /// One finished block by id, or `None` once it has been evicted.
    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.emulator.block(id)
    }

    /// How many blocks have been dropped off the front of the list. A list
    /// that caches per-block geometry can compare this against what it saw
    /// last to learn that the front moved.
    pub fn blocks_evicted(&self) -> usize {
        self.emulator.blocks_evicted()
    }

    /// What [`Self::blocks_evicted`] came to at the last
    /// [`Self::clear_blocks`]. See [`Emulator::blocks_cleared_at`].
    pub fn blocks_cleared_at(&self) -> usize {
        self.emulator.blocks_cleared_at()
    }

    /// Forgets the finished blocks and the scrollback, keeps the open block,
    /// and answers the blocks it let go of. The child is not told: it is the
    /// pane being cleared, not the program's screen. See
    /// [`Emulator::clear_blocks`].
    pub fn clear_blocks(&mut self) -> Vec<Block> {
        self.emulator.clear_blocks()
    }

    /// The block everything arriving now belongs to. Also on every
    /// [`Snapshot`], which is where a renderer should read it.
    pub fn live_block(&self) -> LiveBlock {
        self.emulator.live_block()
    }

    /// Sends a key press, encoded for whichever modes the child has asked for.
    ///
    /// Returns whether the key produced any bytes; keys with no encoding — a
    /// bare modifier, a media key, an unmapped Ctrl combination — produce none.
    pub fn send_key(&mut self, key: Key, modifiers: Modifiers) -> io::Result<bool> {
        let Some(bytes) = input::encode(key, modifiers, self.emulator.input_modes()) else {
            return Ok(false);
        };
        self.write(&bytes)?;
        Ok(true)
    }

    /// Sends a key press to the shell's own line editor, and hands it the
    /// keyboard with it: [`Snapshot::line_editor`] is true from here until the
    /// line editor gives its line back. See
    /// [`Emulator::hand_keys_to_line_editor`] for when that is, and for why a
    /// shell that has not marked the end of its prompt is sent the key and
    /// handed nothing.
    ///
    /// Returns what [`Self::send_key`] does. A key with no encoding hands
    /// nothing over either: the line editor never heard it.
    pub fn send_key_to_line_editor(&mut self, key: Key, modifiers: Modifiers) -> io::Result<bool> {
        let sent = self.send_key(key, modifiers)?;
        if sent {
            self.emulator.hand_keys_to_line_editor();
        }
        Ok(sent)
    }

    /// Sends a pointer gesture, if the child has asked to hear about it.
    ///
    /// Returns whether any bytes were sent, which is also the answer to "did
    /// the program take this gesture?" — a caller uses it to decide whether the
    /// same press should instead start a selection. Nothing is sent, and
    /// `false` comes back, whenever no mouse mode is in force.
    ///
    /// `at` is a cell of the viewport. Converting a pixel to one is the
    /// renderer's job: only it knows the cell size and where the grid was
    /// drawn.
    pub fn send_mouse(
        &mut self,
        kind: MouseEventKind,
        button: Option<MouseButton>,
        row: usize,
        column: usize,
        modifiers: Modifiers,
    ) -> io::Result<bool> {
        let modes = self.emulator.mouse_modes();
        let Some(bytes) = mouse::encode(kind, button, row, column, modifiers, modes) else {
            return Ok(false);
        };
        self.write(&bytes)?;
        Ok(true)
    }

    /// Sends the wheel as arrow keys, for a full-screen program that never
    /// asked for the mouse.
    ///
    /// This is what makes the wheel scroll `less`, `man` and `git log`. None of
    /// them reports the mouse; all of them read arrow keys, and `?1007` is how
    /// they say so. `lines` is positive for a turn away from the user, matching
    /// [`Self::scroll_lines`].
    ///
    /// Returns whether anything was sent. Nothing is on the primary screen: the
    /// wheel belongs to the scrollback there, which is real history a person
    /// can go back to.
    pub fn send_alternate_scroll(&mut self, lines: i32) -> io::Result<bool> {
        let modes = self.emulator.mouse_modes();
        if !modes.wants_alternate_scroll(self.emulator.is_alt_screen()) || lines == 0 {
            return Ok(false);
        }

        let key = if lines > 0 { Key::Up } else { Key::Down };
        let count = lines.unsigned_abs();
        // One key press per line, because that is what a wheel notch is to a
        // program reading arrow keys, and a pager has no way to be told "three
        // lines" in one.
        for _ in 0..count {
            self.send_key(key, Modifiers::NONE)?;
        }
        Ok(true)
    }

    /// Asks the shell for completions of a line it cannot see.
    ///
    /// **The one thing OSC 133 is not.** Command marks are an announcement:
    /// the shell says where a prompt began and how a command ended, and never
    /// answers a question. Completion is a question — "what could this line
    /// become?" — and the answer belongs to the shell, which owns the
    /// `PATH` hashing, the `complete` definitions and the glob rules that make
    /// one right.
    ///
    /// The line does not travel in this sequence, and that is deliberate. What
    /// travels is a *key*, one the integration snippet has bound to a function
    /// of its own; the line itself is a file the caller wrote and the snippet
    /// reads. An escape sequence carrying a command line would have to escape
    /// every character a shell can hold — a semicolon, a quote, a newline, a
    /// byte that is not UTF-8 at all — and the answer coming back would have
    /// the same problem twice over, in a shell that may have no `base64` to
    /// solve it with.
    ///
    /// A shell with no integration binds nothing, so this types an unknown
    /// escape sequence at a prompt: the line editor discards it and beeps at
    /// worst. Nothing is echoed and no line is submitted.
    pub fn request_completions(&mut self) -> io::Result<()> {
        self.write(COMPLETION_REQUEST)
    }

    /// Tells the child its terminal gained or lost the keyboard, if it has
    /// asked to be told.
    ///
    /// Returns whether anything was sent. Nothing is until the child has set
    /// `?1004`, which is what keeps `CSI I` off a shell's command line; see
    /// [`input::focus`]. Which terminal *has* the keyboard — the window's, and
    /// within it one pane's — is the caller's to know and to say.
    pub fn send_focus(&mut self, focused: bool) -> io::Result<bool> {
        let Some(bytes) = input::focus(focused, self.emulator.focus_reporting()) else {
            return Ok(false);
        };
        self.write(bytes)?;
        Ok(true)
    }

    /// Sends pasted text, the way [`input::paste`] spells it: bracketed when
    /// the child asked for that, with the bytes that could end the bracket
    /// taken out, and with newlines as the Enter key sends them otherwise.
    pub fn paste(&mut self, text: &str) -> io::Result<()> {
        self.write(&input::paste(text, self.emulator.bracketed_paste()))
    }

    /// Changes the size of the grid and tells the child about it.
    ///
    /// The pty is resized too, which is what raises `SIGWINCH` in the child; a
    /// full-screen program that never gets it keeps drawing at the old size.
    pub fn resize(&mut self, size: TerminalSize) -> Result<()> {
        self.emulator.resize(size);
        self.link.resize(size)
    }

    /// The size the grid was last set to.
    pub fn size(&self) -> TerminalSize {
        self.emulator.size()
    }

    /// Repaints the grid in a new palette.
    ///
    /// What applying a theme to a shell that is already running comes down to.
    /// The child is not told and does not need to be: a palette is how the
    /// *renderer* resolves the colours a program asked for by name or by
    /// number, so every cell already on screen is re-resolved and everything
    /// the program writes next lands in the new colours too.
    ///
    /// The overrides a program set for itself with OSC 4, 10, 11 and 12 are
    /// not touched, and that is the contract those escapes carry: a program
    /// that asked for a specific background keeps it until it resets it,
    /// whatever the theme underneath has become.
    pub fn set_palette(&mut self, palette: Palette) {
        self.emulator.set_palette(palette);
    }

    /// Moves the viewport through the scrollback: a positive `delta` moves it
    /// back into history, a negative one towards the live output.
    pub fn scroll_lines(&mut self, delta: i32) {
        self.emulator.scroll_lines(delta);
    }

    /// Returns the viewport to the live output.
    pub fn scroll_to_bottom(&mut self) {
        self.emulator.scroll_to_bottom();
    }

    /// How many lines of scrollback sit above the viewport.
    ///
    /// What "scroll to the top" needs and the only thing that knows it: a
    /// caller that guessed a large enough delta would be a caller that has to
    /// be changed the day the scrollback limit is.
    pub fn history_len(&self) -> usize {
        self.emulator.history_len()
    }

    /// Copies lines out of the grid into the same store a finished block's
    /// rows live in, numbered from the oldest line of the scrollback.
    ///
    /// What a pane drawing one grid rather than a list of blocks copies a
    /// selection out of: the rows a drag covered may have scrolled out of the
    /// viewport since, and the snapshot only ever holds the viewport. Also
    /// returns the row the answer actually starts at, which is later than
    /// `first` when the history has already dropped the line it named.
    pub fn harvest_rows(&self, first: usize, last: usize) -> (BlockRows, usize) {
        self.emulator.harvest_rows(first, last)
    }

    /// The title the child last asked for, which is what a tab should call
    /// itself.
    pub fn title(&self) -> Option<&str> {
        self.emulator.title()
    }

    /// The working directory the child last reported with OSC 7.
    pub fn working_directory(&self) -> Option<&Path> {
        self.emulator.working_directory()
    }

    /// Everything the child has asked for since the last call: bells, titles,
    /// working directories, a clipboard write, a notification, the child
    /// finishing.
    pub fn take_events(&mut self) -> Vec<TerminalEvent> {
        self.emulator.take_events()
    }

    /// The emulator, for the state that has no shortcut here — the alternate
    /// screen flag, the palette, the keyboard modes.
    pub fn emulator(&self) -> &Emulator {
        &self.emulator
    }

    /// The child's process id, on the platforms that have one.
    pub fn process_id(&self) -> Option<u32> {
        self.link.process_id()
    }

    /// How the child finished, or `None` while it is still running.
    ///
    /// Does not block. The first call that finds the child gone also queues a
    /// [`TerminalEvent::ChildExited`], so an application that only watches
    /// events still learns about it.
    pub fn try_wait(&mut self) -> Result<Option<ChildExit>> {
        if self.exit.is_some() {
            return Ok(self.exit.clone());
        }
        let Some(exit) = self.link.try_wait()? else {
            return Ok(None);
        };
        self.exit = Some(exit.clone());
        self.emulator.child_exited(exit.clone());
        Ok(Some(exit))
    }

    /// Whether the child has finished, as of the last [`Self::try_wait`].
    pub fn has_exited(&self) -> bool {
        self.exit.is_some()
    }

    /// Ends the child without waiting to collect its exit status.
    ///
    /// For a caller that wants the pty closed and does not need the status
    /// that [`Self::shutdown`] waits for. It is not for the thread that draws
    /// all the same, because on Unix the kill itself waits: 50 ms for a child
    /// that goes when it is hung up, and 200 for one that ignores it — see
    /// [`Pty`]'s `kill`. The kill escalates, so "without waiting for it" is not
    /// "without dying".
    pub fn kill(&mut self) -> Result<()> {
        self.link.kill()
    }

    /// Ends the session: kills the child if it is still running and waits for
    /// it, so nothing is left holding a pty nobody reads.
    ///
    /// Dropping a terminal does the same thing without the waiting, so this is
    /// only needed when the caller wants the exit status.
    pub fn shutdown(&mut self) -> Result<ChildExit> {
        self.link.kill()?;
        let exit = self.link.wait()?;
        if self.exit.is_none() {
            self.exit = Some(exit.clone());
            self.emulator.child_exited(exit.clone());
        }
        Ok(exit)
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
