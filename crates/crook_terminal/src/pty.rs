//! A shell running on a pseudo-terminal.
//!
//! Every platform has its own name for this — `openpty` and a controlling tty
//! on Unix, ConPTY on Windows — and `portable-pty` already knows all three, so
//! there is not one `#[cfg]` here that spawns a process. What is left is the
//! part a terminal has to decide for itself: which shell to run, what
//! environment to run it in, and how to hand the two ends of the pty to two
//! different threads.
//!
//! The reader and the writer are separate on purpose. Reading from a pty blocks
//! until the child says something, which may be hours; [`Pty::take_reader`]
//! hands that half out so it can block on a thread of its own while the writer
//! stays with the UI and stays responsive.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context as _, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::snapshot::TerminalSize;

/// The terminal type the child is told it is running on.
///
/// `alacritty_terminal` implements the xterm sequences this names, and the
/// entry is in every terminfo database old enough to matter, so nothing has to
/// be installed for a program to drive the grid correctly.
const TERM: &str = "xterm-256color";

/// What to run inside the pty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Program {
    /// The user's shell, as chosen by [`default_shell`].
    #[default]
    Shell,
    /// A specific executable, run instead of a shell.
    Command {
        /// The executable, resolved through `PATH` if it is not a full path.
        program: OsString,
        /// Its arguments, not including the program itself.
        args: Vec<OsString>,
    },
}

impl Program {
    /// A specific executable and its arguments.
    pub fn command<S, I, A>(program: S, args: I) -> Self
    where
        S: Into<OsString>,
        I: IntoIterator<Item = A>,
        A: Into<OsString>,
    {
        Self::Command {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

/// The shell a terminal should open when the user has not asked for anything
/// else.
///
/// On Unix that is `$SHELL`, which is what the login process set from the
/// password database, falling back to `/bin/sh` — the one shell POSIX promises
/// exists. On Windows it is `%ComSpec%`, which is how a program is told which
/// command processor to use, falling back to PowerShell when it is unset.
pub fn default_shell() -> OsString {
    #[cfg(windows)]
    {
        std::env::var_os("ComSpec")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| OsString::from("powershell.exe"))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| OsString::from("/bin/sh"))
    }
}

/// How a child process finished.
///
/// A copy of `portable_pty::ExitStatus` that can be compared and stored, which
/// the original cannot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildExit {
    /// The exit code, or 1 when the process was signalled.
    pub code: u32,
    /// The signal that killed it, on the platforms that have signals.
    pub signal: Option<String>,
}

impl ChildExit {
    /// A plain exit with the given code.
    pub fn from_code(code: u32) -> Self {
        Self { code, signal: None }
    }

    /// Whether the child finished the way it meant to.
    pub fn success(&self) -> bool {
        self.signal.is_none() && self.code == 0
    }
}

impl From<portable_pty::ExitStatus> for ChildExit {
    fn from(status: portable_pty::ExitStatus) -> Self {
        Self {
            code: status.exit_code(),
            signal: status.signal().map(str::to_owned),
        }
    }
}

impl fmt::Display for ChildExit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.signal {
            Some(signal) => write!(formatter, "terminated by {signal}"),
            None => write!(formatter, "exited with code {}", self.code),
        }
    }
}

/// The readable half of a pty, meant to be moved to a thread of its own.
///
/// Reads block until the child writes. When the child exits, the read returns
/// zero bytes, or an error on the platforms that report a closed pty that way —
/// both mean the same thing, and both mean it is time to stop reading.
pub struct PtyReader(Box<dyn Read + Send>);

impl Read for PtyReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buffer)
    }
}

impl fmt::Debug for PtyReader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PtyReader")
    }
}

/// A pty with a child process running on the far end of it.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Option<PtyReader>,
    exit: Option<ChildExit>,
}

impl Pty {
    /// Opens a pty of the given size and starts `program` on it.
    ///
    /// The child inherits this process's environment, with `TERM` and
    /// `COLORTERM` set to describe the emulator and the stale `LINES` and
    /// `COLUMNS` of whatever started Crook removed — the pty's own size is the
    /// truth, and a child that believed those would lay itself out wrong.
    /// Anything in `environment` is applied last and wins.
    pub fn spawn(
        program: &Program,
        size: TerminalSize,
        working_directory: Option<&Path>,
        environment: &[(String, String)],
    ) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .context("Failed to open a pseudo-terminal")?;

        let mut command = match program {
            Program::Shell => CommandBuilder::new(default_shell()),
            Program::Command { program, args } => {
                let mut command = CommandBuilder::new(program);
                command.args(args);
                command
            }
        };

        command.env("TERM", TERM);
        command.env("COLORTERM", "truecolor");
        command.env_remove("LINES");
        command.env_remove("COLUMNS");
        for (key, value) in environment {
            command.env(key, value);
        }
        if let Some(directory) = working_directory {
            command.cwd(directory);
        }

        let child = pair.slave.spawn_command(command).with_context(|| {
            format!("Failed to start {} in a pseudo-terminal", describe(program))
        })?;
        // The slave is dropped here on purpose: while this process holds it
        // open, a read on the master never sees end-of-file, so the reader
        // thread would never learn that the child had gone.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("Failed to read from the pseudo-terminal")?;

        Ok(Self {
            master: pair.master,
            child,
            reader: Some(PtyReader(reader)),
            exit: None,
        })
    }

    /// A writer for the child's standard input.
    ///
    /// Every call opens a new handle; dropping the last one sends end-of-file
    /// to the child, so a caller that wants the child to keep running must hold
    /// on to it.
    pub fn writer(&self) -> Result<Box<dyn Write + Send>> {
        self.master
            .take_writer()
            .context("Failed to write to the pseudo-terminal")
    }

    /// The readable half, once. Subsequent calls return `None`, because the
    /// reads are a stream and splitting it across two owners would interleave
    /// escape sequences into nonsense.
    pub fn take_reader(&mut self) -> Option<PtyReader> {
        self.reader.take()
    }

    /// Tells the kernel the window changed size, which is what makes the child
    /// see `SIGWINCH` and re-lay-out.
    pub fn resize(&self, size: TerminalSize) -> Result<()> {
        self.master
            .resize(pty_size(size))
            .context("Failed to resize the pseudo-terminal")
    }

    /// The child's process id, on the platforms that have one.
    pub fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// How the child finished, or `None` while it is still running. Does not
    /// block; the answer is remembered once it arrives.
    pub fn try_wait(&mut self) -> Result<Option<ChildExit>> {
        if self.exit.is_some() {
            return Ok(self.exit.clone());
        }
        let status = self
            .child
            .try_wait()
            .context("Failed to check on the child process")?;
        self.exit = status.map(ChildExit::from);
        Ok(self.exit.clone())
    }

    /// Blocks until the child finishes.
    pub fn wait(&mut self) -> Result<ChildExit> {
        if let Some(exit) = &self.exit {
            return Ok(exit.clone());
        }
        let status = self
            .child
            .wait()
            .context("Failed to wait for the child process")?;
        let exit = ChildExit::from(status);
        self.exit = Some(exit.clone());
        Ok(exit)
    }

    /// Ends the child process. Killing one that has already finished is not an
    /// error.
    ///
    /// This goes through the child itself rather than through the detachable
    /// killer `portable-pty` also offers, and the difference is the whole
    /// behaviour: the detached killer sends `SIGHUP` once and reports success,
    /// while the child's own kill sends `SIGHUP`, gives it a quarter of a
    /// second to go, and then sends `SIGKILL`. A shell whose dotfiles trap
    /// `HUP` — or any child running under a wrapper that does — survives the
    /// first and not the second, and anything that waits for a survivor waits
    /// forever.
    pub fn kill(&mut self) -> Result<()> {
        if self.exit.is_some() {
            return Ok(());
        }
        let killed = self.child.kill().context("Failed to end the child process");
        // The escalation waits on the child to find out whether the first
        // signal worked, which reaps it — and a reaped process id can be handed
        // straight back out to something else. Recording the exit here is what
        // stops a later kill, `Drop`'s included, from signalling a stranger.
        let _ = self.try_wait();
        killed
    }
}

impl Drop for Pty {
    /// Dropping the terminal must not leave the shell behind: it would keep
    /// running, holding a pty nothing reads from, until it filled the buffer
    /// and blocked forever.
    fn drop(&mut self) {
        if self.exit.is_none()
            && let Err(error) = self.kill()
        {
            log::debug!("Could not end the child process while closing a terminal: {error:#}");
        }
    }
}

impl fmt::Debug for Pty {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pty")
            .field("process_id", &self.process_id())
            .field("exit", &self.exit)
            .finish()
    }
}

/// The pty's idea of a size, which is the grid plus the pixel dimensions some
/// full-screen programs ask the kernel for.
fn pty_size(size: TerminalSize) -> PtySize {
    PtySize {
        rows: size.rows.max(1),
        cols: size.columns.max(1),
        pixel_width: size.columns.saturating_mul(size.cell_width),
        pixel_height: size.rows.saturating_mul(size.cell_height),
    }
}

/// Names a program for an error message.
fn describe(program: &Program) -> String {
    let name = match program {
        Program::Shell => default_shell(),
        Program::Command { program, .. } => program.clone(),
    };
    OsStr::new(&name).to_string_lossy().into_owned()
}

#[cfg(test)]
#[path = "pty_tests.rs"]
mod tests;
