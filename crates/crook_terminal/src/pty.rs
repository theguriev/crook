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
    /// The user's shell, as chosen by [`default_shell`], started as a login
    /// shell. The same thing as [`Self::LoginShell`] with that shell named.
    #[default]
    Shell,
    /// A named shell, started the way `login(1)` starts one — with `-l` where
    /// [`login_arguments`] has one, and otherwise with the argv\[0\]
    /// convention `login(1)` itself uses.
    LoginShell {
        /// The shell. A full path, since this is the shell a person chose
        /// rather than a name to look up.
        program: OsString,
    },
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

    /// This shell, started as a login shell.
    pub fn login_shell<S: Into<OsString>>(program: S) -> Self {
        Self::LoginShell {
            program: program.into(),
        }
    }
}

/// The shell a terminal should open when the user has not asked for anything
/// else.
///
/// On Unix that is `$SHELL` when it names something this user may execute, and
/// otherwise the shell in the password database — `pw_shell`, which is what
/// `chsh` writes and what `login(1)` reads — falling back to `/bin/sh`, the one
/// shell POSIX promises exists. Both halves of that are load-bearing and
/// neither is hypothetical:
///
/// * `$SHELL` is absent from the environment of anything a desktop launches
///   rather than a shell — an application bundle opened from the Dock, a
///   `.desktop` entry, a container. Falling straight to `/bin/sh` there would
///   hand the user a POSIX shell with none of their configuration, no marks and
///   a name Crook cannot recognise, on the launch path most people use.
/// * `$SHELL` outliving the shell it names — a Homebrew or Nix package removed,
///   a `chsh` to a path that moved — is a pane that cannot open at all, since a
///   program that is not there cannot be spawned. Every other terminal reads
///   `pw_shell` and keeps working.
///
/// The answer comes from `portable-pty`'s own resolution, deliberately: this is
/// the name [`Program::Shell`] spawns *and* the name the app reads to decide
/// which shell integration to install, and a Crook that installed zsh's stubs
/// around a `/bin/sh` would be worse than one that installed none.
///
/// On Windows it is `%ComSpec%`, which is how a program is told which command
/// processor to use, falling back to PowerShell when it is unset.
pub fn default_shell() -> OsString {
    #[cfg(windows)]
    {
        std::env::var_os("ComSpec")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| OsString::from("powershell.exe"))
    }
    #[cfg(not(windows))]
    {
        OsString::from(CommandBuilder::new_default_prog().get_shell())
    }
}

/// The arguments that make `shell` a *login* shell, or none for a shell whose
/// switch Crook has not checked.
///
/// A login shell is what reads `/etc/zprofile`, `$ZDOTDIR/.zprofile` and
/// `$ZDOTDIR/.zlogin` on zsh, `/etc/profile` and `~/.bash_profile` on bash, and
/// what makes fish's own `config.fish` run `path_helper`. Those are the files a
/// person puts the facts about their whole session in — `PATH` before anything
/// else — and on macOS `/etc/zprofile` is where `path_helper` builds `PATH` out
/// of `/etc/paths` and `/etc/paths.d` at all. A terminal that starts a non-login
/// shell therefore shows a machine with different tools on it, in a different
/// order, than every other terminal on the same desktop does.
///
/// Empty is not "no login shell": it is "not by an argument". A shell with no
/// switch here is started as a login shell the way `login(1)` does it, by
/// argv\[0\] — see [`Program::LoginShell`].
///
/// # Windows
///
/// Nothing is returned there and nothing should be. PowerShell and cmd have no
/// login shell: there is no per-machine profile a session inherits its `PATH`
/// from, because `PATH` is in the registry and every process already has the
/// whole of it. `$PROFILE` is read by every interactive PowerShell, login or
/// not. Inventing an equivalent — running `profile.ps1` by hand, say — would
/// be Crook making up a startup convention that the platform does not have and
/// that no other terminal implements.
///
/// # Not the same table as the integration's
///
/// The app's `shell_integration::Shell` names the shells Crook has an OSC 133
/// snippet for; this names the shells whose login switch Crook has checked.
/// They hold the same three names today and they answer different questions —
/// a shell can take `-l` without Crook having anything to inject into it.
pub fn login_arguments(shell: &Path) -> Vec<OsString> {
    let name = shell
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match name.as_str() {
        "zsh" | "bash" | "fish" => vec![OsString::from("-l")],
        _ => Vec::new(),
    }
}

/// A spawn of `shell` as a login shell, by whichever of the two conventions
/// this shell can be asked in.
///
/// There are two, and they are different mechanisms rather than two spellings
/// of one:
///
/// * `-l`, which zsh, bash and fish all take, and which says exactly which
///   binary is being run.
/// * argv\[0\] set to the shell's file name with a leading hyphen, which is
///   what `login(1)` itself does and what every shell that has a login mode at
///   all understands — including the ones that answer ``Unknown option: `-l'``,
///   which tcsh does unless `-l` is the only argument on the line.
///
/// Crook uses the switch where it knows one and argv\[0\] where it does not,
/// and the split is not arbitrary. `portable-pty` offers the argv\[0\]
/// convention only through `new_default_prog`, a builder that takes no
/// arguments at all and resolves the shell itself, from `SHELL` in the
/// environment the child will get. That resolution is a feature for a shell
/// nobody recognised — a `SHELL` naming something unrunnable falls back to the
/// password database rather than to a pane that will not open — and a hazard
/// for the three that are recognised, because the app has by then written
/// startup stubs shaped for *that* shell, and a silent substitution would hand
/// a fish the stubs zsh was going to read. So the named three are spawned by
/// name, with `-l`; everything else is handed to `login(1)`'s own convention
/// with `SHELL` pinned to the shell that was asked for.
///
/// On Windows neither convention exists — see [`login_arguments`] — so the
/// shell is started exactly as it would have been.
fn login_command(shell: &OsStr) -> CommandBuilder {
    let arguments = login_arguments(Path::new(shell));
    if !arguments.is_empty() {
        let mut command = CommandBuilder::new(shell);
        command.args(arguments);
        return command;
    }

    #[cfg(unix)]
    {
        let mut command = CommandBuilder::new_default_prog();
        // How `new_default_prog` is told which shell to run: it reads `SHELL`
        // out of the environment it is going to hand the child. Setting it is
        // also correct on its own terms — a shell's `$SHELL` should name the
        // shell that is running.
        command.env("SHELL", shell);
        command
    }
    #[cfg(not(unix))]
    {
        CommandBuilder::new(shell)
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
    /// The size is set on the pty *before* the child exists, so the very first
    /// thing it can ask the kernel is already the right answer. A prompt that
    /// measures its terminal — a right-aligned segment, a rule across the width
    /// — is drawn correctly the first time rather than reflowed after a
    /// `SIGWINCH` it might not handle.
    ///
    /// The child inherits this process's environment, with `TERM` and
    /// `COLORTERM` set to describe the emulator and the stale `LINES` and
    /// `COLUMNS` of whatever started Crook removed — the pty's own size is the
    /// truth, and a child that believed those would lay itself out wrong.
    ///
    /// `working_directory` is where it starts, and when it is `None` — or names
    /// a directory that no longer exists, which is what a restored session can
    /// hand over — the child starts in `$HOME`, which is where a terminal opens
    /// a new window.
    ///
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

        let mut command = command_for(program, environment);
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

/// What a spawn will run, and the environment it will run it in.
///
/// Separated from [`Pty::spawn`] so that the answers to "what is the child
/// told about its terminal?" can be asked without a pty and without a process:
/// every one of them is a property of this value.
///
/// `LINES` and `COLUMNS` are removed rather than set. Whatever started Crook
/// may have had them, they described *its* window, and a child that believed
/// them would lay itself out to a size that is not the one it is on — the
/// kernel's `winsize`, which the pty was opened at before the child existed, is
/// the truth and is what `ioctl` and `$COLUMNS` in an interactive shell both
/// come back to.
fn command_for(program: &Program, environment: &[(String, String)]) -> CommandBuilder {
    let mut command = match program {
        Program::Shell => login_command(&default_shell()),
        Program::LoginShell { program } => login_command(program),
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
    command
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
        Program::LoginShell { program } | Program::Command { program, .. } => program.clone(),
    };
    OsStr::new(&name).to_string_lossy().into_owned()
}

#[cfg(test)]
#[path = "pty_tests.rs"]
mod tests;
