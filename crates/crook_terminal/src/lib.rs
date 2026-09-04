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
//! Deciding which thread reads the pty, how often the grid is repainted and
//! when the child is reaped is the application's job, in the same way
//! `crook_usage`'s poller leaves its schedule to whoever drives it — only the
//! application knows which of its threads is allowed to block.
//!
//! It also draws nothing and knows nothing about fonts, windows or the GPU. The
//! contract with the renderer is [`Snapshot`], which is plain owned data with
//! every colour already resolved to RGB.
//!
//! ## The pieces
//!
//! * [`Pty`] spawns the shell and owns both ends of the pseudo-terminal.
//! * [`Emulator`] turns bytes into a grid. It works with no pty at all, which
//!   is what makes this crate testable without a process.
//! * [`Snapshot`] is what the renderer draws.
//! * [`Block`] is one command, its output and how it ended, with its rows owned
//!   rather than borrowed from the grid. [`Terminal::blocks`] is the history;
//!   [`Snapshot::live_block`] is the one still open.
//! * [`input`] encodes key presses into the bytes a shell expects.
//! * [`selection`] is the vocabulary a pointer selects text with.
//!
//! ## Selection, and what it does not cover yet
//!
//! Selecting text still belongs to the emulator, which means it works within
//! the grid — the live block and whatever of the previous one has not scrolled
//! away. A finished block's rows are no longer in the grid, so a drag cannot
//! reach across two of them. Copying a whole finished block is exact and needs
//! no selection at all: [`BlockRows::to_text`].

mod blocks;
mod emulator;
mod harvest;
pub mod input;
mod marks;
pub mod mouse;
mod pty;
pub mod selection;
mod snapshot;

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

pub use crate::blocks::{Block, BlockId, BlockState, IgnoreReason, LiveBlock, PromptEnd};
pub use crate::emulator::{Emulator, TerminalEvent};
pub use crate::harvest::{BlockRows, RowCombining, StyleRun};
pub use crate::input::{InputModes, Key, KeyboardModes, KeypadKey, Modifiers};
pub use crate::marks::{PromptKind, ShellMark};
pub use crate::mouse::{MouseButton, MouseEventKind, MouseModes};
pub use crate::pty::{ChildExit, Program, Pty, PtyReader, default_shell};
pub use crate::selection::{CellSide, GridPoint, SelectionKind, SelectionSpan, ViewportPoint};
pub use crate::snapshot::{
    CellCombining, CellFlags, Cursor, CursorShape, Palette, Rgb, Snapshot, SnapshotCell,
    TerminalSize,
};

/// How much output a terminal remembers above the viewport.
const DEFAULT_SCROLLBACK_LINES: usize = 10_000;

/// Everything a new terminal needs to know.
#[derive(Clone, Debug, Default)]
pub struct TerminalOptions {
    /// The grid to start at. The child is told this size before it starts, so
    /// its first prompt is already laid out correctly.
    pub size: TerminalSize,
    /// What to run. Defaults to the user's shell.
    pub program: Program,
    /// Where to run it, or the current process's directory when unset.
    pub working_directory: Option<PathBuf>,
    /// Environment variables applied on top of the inherited ones.
    pub environment: Vec<(String, String)>,
    /// Lines of scrollback to keep. Defaults to [`DEFAULT_SCROLLBACK_LINES`].
    pub scrollback_lines: Option<usize>,
    /// The colours the grid is drawn in.
    pub palette: Palette,
}

/// A running terminal session.
///
/// Owns the pty, the child process and the emulator. Everything about how it is
/// driven — the thread that reads, the frame that paints, the moment the child
/// is reaped — belongs to the caller.
pub struct Terminal {
    emulator: Emulator,
    pty: Pty,
    writer: Box<dyn Write + Send>,
    exit: Option<ChildExit>,
}

impl fmt::Debug for Terminal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Terminal")
            .field("pty", &self.pty)
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
        } = options;

        let pty = Pty::spawn(&program, size, working_directory.as_deref(), &environment)?;
        let writer = pty.writer()?;
        let emulator = Emulator::new(
            size,
            scrollback_lines.unwrap_or(DEFAULT_SCROLLBACK_LINES),
            palette,
        );

        Ok(Self {
            emulator,
            pty,
            writer,
            exit: None,
        })
    }

    /// The readable half of the pty, once. Move it to a thread and read until
    /// it returns zero bytes or errors, feeding what it reads to [`Self::feed`]
    /// on the thread that owns this terminal.
    pub fn take_reader(&mut self) -> Option<PtyReader> {
        self.pty.take_reader()
    }

    /// Consumes output from the child.
    ///
    /// Some of that output is a question, and this answers it: replies the
    /// emulator owes the child are written straight back to the pty, because a
    /// program that asked for the cursor position or the palette will wait
    /// forever otherwise.
    pub fn feed(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.emulator.advance(bytes);
        let replies = self.emulator.take_replies();
        if replies.is_empty() {
            return Ok(());
        }
        self.write(&replies)
    }

    /// The visible grid. Cheap to call every frame: the same `Arc`, carrying
    /// the same [`Snapshot::revision`], comes back until the content changes.
    pub fn snapshot(&mut self) -> Arc<Snapshot> {
        self.emulator.snapshot()
    }

    /// Sends bytes to the child as if they had been typed.
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Runs a command line: records the block boundary, then writes the line
    /// followed by the carriage return the Enter key sends.
    ///
    /// This is the boundary that does not depend on the shell cooperating. A
    /// shell with OSC 133 integration reports where its prompt ended and when
    /// the command started; a shell without it reports nothing, and this is
    /// then the only thing that knows a command was run at all — and it knows
    /// the exact text, where a mark-driven block only has what was echoed.
    pub fn submit(&mut self, line: &str) -> io::Result<()> {
        self.emulator.command_submitted(line);
        self.write(line.as_bytes())?;
        self.write(b"\r")
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
        at: ViewportPoint,
        modifiers: Modifiers,
    ) -> io::Result<bool> {
        let modes = self.emulator.mouse_modes();
        let Some(bytes) = mouse::encode(kind, button, at, modifiers, modes) else {
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

    /// Sends pasted text.
    ///
    /// When the child has asked for bracketed paste it is wrapped in the
    /// markers that let the child tell a paste from typing, which is how an
    /// editor avoids auto-indenting what you pasted. When it has not, newlines
    /// are normalised to carriage returns, because that is what the Enter key
    /// sends and a bare newline would not run the line.
    ///
    /// Inside the brackets, `ESC` and `ETX` are dropped. A paste is untrusted
    /// text — copied from a web page, a chat log, a file someone else wrote —
    /// and text that still contains `ESC` can write the end marker itself, at
    /// which point the shell leaves paste mode and reads the rest as if it had
    /// been typed. Everything after a smuggled `\x1b[201~` would then run
    /// without anyone pressing Enter. Filtering the two bytes that can do it is
    /// what every terminal with bracketed paste does.
    pub fn paste(&mut self, text: &str) -> io::Result<()> {
        if self.emulator.bracketed_paste() {
            self.write(b"\x1b[200~")?;
            self.write(text.replace(['\x1b', '\x03'], "").as_bytes())?;
            return self.write(b"\x1b[201~");
        }
        let typed = text.replace("\r\n", "\r").replace('\n', "\r");
        self.write(typed.as_bytes())
    }

    /// Changes the size of the grid and tells the child about it.
    ///
    /// The pty is resized too, which is what raises `SIGWINCH` in the child; a
    /// full-screen program that never gets it keeps drawing at the old size.
    pub fn resize(&mut self, size: TerminalSize) -> Result<()> {
        self.emulator.resize(size);
        self.pty.resize(size)
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

    /// Starts a selection at a cell of the viewport, replacing any there was.
    ///
    /// The kind is what the gesture meant: [`SelectionKind::Simple`] for a
    /// drag, [`SelectionKind::Semantic`] for a double click, and
    /// [`SelectionKind::Lines`] for a triple click.
    pub fn start_selection(&mut self, kind: SelectionKind, at: ViewportPoint, side: CellSide) {
        self.emulator.start_selection(kind, at, side);
    }

    /// Drags the open end of the selection to a cell of the viewport.
    pub fn update_selection(&mut self, at: ViewportPoint, side: CellSide) {
        self.emulator.update_selection(at, side);
    }

    /// Drops the selection.
    pub fn clear_selection(&mut self) {
        self.emulator.clear_selection();
    }

    /// Whether there is anything selected to copy.
    pub fn has_selection(&self) -> bool {
        self.emulator.has_selection()
    }

    /// The selected text, or `None` when nothing is selected.
    pub fn selection_text(&self) -> Option<String> {
        self.emulator.selection_text()
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
    /// working directories, a clipboard write, the child finishing.
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
        self.pty.process_id()
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
        let Some(exit) = self.pty.try_wait()? else {
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

    /// Ends the child without waiting for it.
    ///
    /// For a caller that wants the pty closed now and does not need the exit
    /// status — closing a pane on the thread that draws, where the wait
    /// [`Self::shutdown`] does would be a stall a person can see. The kill
    /// escalates, so "without waiting" is not "without dying".
    pub fn kill(&mut self) -> Result<()> {
        self.pty.kill()
    }

    /// Ends the session: kills the child if it is still running and waits for
    /// it, so nothing is left holding a pty nobody reads.
    ///
    /// Dropping a terminal does the same thing without the waiting, so this is
    /// only needed when the caller wants the exit status.
    pub fn shutdown(&mut self) -> Result<ChildExit> {
        self.pty.kill()?;
        let exit = self.pty.wait()?;
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
