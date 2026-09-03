//! Making the user's shell say where one command ends and the next begins.
//!
//! A terminal cannot see command boundaries. Everything arriving from the pty
//! is one stream of bytes, and the prompt, the command the user typed and the
//! output it produced are indistinguishable inside it. The shell is the only
//! party that knows, so the shell has to say — and the way it says so is
//! [OSC 133], four escape sequences that every terminal doing this reads:
//!
//! | mark | means |
//! |---|---|
//! | `ESC ] 133 ; A ST` | a prompt is about to be drawn |
//! | `ESC ] 133 ; B ST` | the prompt is finished; the cursor is where the user types |
//! | `ESC ] 133 ; C ST` | the line was accepted and is about to run |
//! | `ESC ] 133 ; D ; <exit> ST` | it finished, with that status |
//!
//! This module is the emitting half only: it starts the user's shell in a way
//! that makes those four sequences appear in the stream. Reading them back out
//! of the stream, and turning them into blocks, happens in `crook_terminal`.
//!
//! # Without touching a file the user owns
//!
//! Nothing here writes to `~/.zshrc`, `~/.bashrc` or `config.fish`. Editing a
//! person's dotfiles to install a terminal feature is a change they did not
//! make, that outlives the terminal, and that they have to undo by hand. Each
//! shell instead gets a launch that reaches the same place through its own
//! documented startup path:
//!
//! * **zsh** — a per-session `ZDOTDIR` of stubs. Each stub sources the user's
//!   real file first and the integration second, with `USER_ZDOTDIR` naming
//!   where the real files live. This is VS Code's arrangement, whose scripts
//!   are MIT and were read as a reference.
//! * **bash** — `--rcfile <stub> -i`, where the stub sources `~/.bashrc`
//!   itself. `--rcfile` replaces the user's rc rather than adding to it, so
//!   that line is load-bearing: forget it and the shell starts with none of
//!   their configuration and nothing on screen explains why.
//! * **fish** — a scratch directory at the front of `XDG_DATA_DIRS` holding
//!   `fish/vendor_conf.d/crook.fish`. fish sources it on its own; no argument
//!   changes and the user's configuration loads exactly as it did. fish 4.0
//!   marks its own prompts, so the snippet installs nothing where it finds
//!   that already on: two parties marking the same block is worse than
//!   neither.
//! * **anything else** — nushell, xonsh, elvish, dash, `/bin/sh`, a shell
//!   nobody has anticipated: nothing is injected and the shell is started the
//!   way Crook always started it. Crook does not substitute a shell that
//!   supports marks for the one the user chose; a terminal that quietly runs a
//!   different shell than `$SHELL` names is a worse bug than a terminal
//!   without blocks.
//!
//! Every shell also gets `TERM_PROGRAM=Crook` and `TERM_PROGRAM_VERSION`,
//! including the unrecognised ones. Those are how a script or a prompt
//! framework tells which terminal it is in; they are not part of the
//! injection.
//!
//! # What it costs when it does not work
//!
//! One pane's blocks, and nothing else. [`Session::open`] cannot fail: an
//! unrecognised shell, an unwritable temporary directory, no `HOME` for zsh's
//! stubs to point back at — each of them returns a session whose
//! [`marks`](Session::marks) is false, which starts the user's shell exactly as
//! before. Degraded is a state this module reports, not an error it raises.
//!
//! # Getting out of it, and getting it somewhere else
//!
//! [`Options::enabled`] is the setting. [`OPT_OUT_VARIABLE`] in the
//! environment overrides it for the cases a setting cannot reach, and is
//! checked at the one place that injects so that no caller can forget it.
//!
//! Injection only reaches shells Crook starts. A shell on the far side of
//! `ssh`, inside a container, or in a `docker exec` is a shell Crook never
//! spawned and cannot reach into, and there is no honest way around that.
//! [`snippet`] returns the same text for a person to paste into their own
//! configuration on that machine, and [`manual_install_file`] names the file it
//! belongs at the end of. The snippets are written to be sourced that way:
//! interactive-only, guarded against being sourced twice, and chaining onto
//! whatever hooks they find rather than replacing them.
//!
//! # Layout
//!
//! * `launch` decides — pure functions, no filesystem, one per shell.
//! * `scratch` does — writes the files, hands over the launch, removes the
//!   directory when the session is dropped.
//! * `crook.zsh`, `crook.bash` and `crook.fish` are the snippets themselves,
//!   compiled in.
//!
//! [OSC 133]: https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/proposals/semantic-prompts.md

pub mod launch;
mod scratch;

#[cfg(test)]
mod tests;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub use crate::shell_integration::launch::{HostEnv, Launch, ScratchFile};
pub use crate::shell_integration::scratch::Session;

/// What `TERM_PROGRAM` is set to in every shell Crook starts.
pub const TERM_PROGRAM: &str = "Crook";

/// What `TERM_PROGRAM_VERSION` is set to.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Setting this in the environment to anything but `0` or the empty string
/// stops Crook injecting anything into any shell, whatever the setting says.
pub const OPT_OUT_VARIABLE: &str = "CROOK_NO_SHELL_INTEGRATION";

/// The zsh integration, for a person to paste into their own `~/.zshrc`.
const ZSH_SNIPPET: &str = include_str!("crook.zsh");

/// The bash integration.
const BASH_SNIPPET: &str = include_str!("crook.bash");

/// The fish integration.
const FISH_SNIPPET: &str = include_str!("crook.fish");

/// A shell Crook knows how to install command marks into, or one it does not.
#[derive(Copy, Clone, Debug, Default, Hash, PartialEq, Eq)]
pub enum Shell {
    /// zsh, reached through a scratch `ZDOTDIR`.
    Zsh,
    /// bash, reached through `--rcfile`.
    Bash,
    /// fish, reached through `XDG_DATA_DIRS` and `vendor_conf.d`.
    Fish,
    /// Everything else. Started as it always was, with no marks.
    #[default]
    Other,
}

impl Shell {
    /// Which shell an executable is, by its file name.
    ///
    /// `/bin/sh` is deliberately [`Shell::Other`] rather than bash. On one
    /// machine it is bash in POSIX mode, on the next it is dash, and the bash
    /// snippet in dash is a syntax error at the user's first prompt. A name
    /// that does not say which shell it is gets no injection.
    pub fn of(program: &Path) -> Self {
        let name = program
            .file_stem()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        match name.as_str() {
            "zsh" => Self::Zsh,
            "bash" => Self::Bash,
            "fish" => Self::Fish,
            _ => Self::Other,
        }
    }
}

/// What a pane wants from this module.
#[derive(Clone, Debug)]
pub struct Options {
    /// Whether to install the marks at all.
    ///
    /// The setting, and only the setting: [`OPT_OUT_VARIABLE`] in the
    /// environment turns injection off regardless of what this says.
    pub enabled: bool,
    /// The shell to run, or the user's own when unset.
    pub shell: Option<PathBuf>,
}

impl Default for Options {
    /// Marks on, the user's shell.
    fn default() -> Self {
        Self {
            enabled: true,
            shell: None,
        }
    }
}

/// The integration for a shell, as text.
///
/// This is what Crook writes into its scratch directory, and it is also exactly
/// what a person should paste at the end of their own configuration on a
/// machine Crook cannot reach — a remote host, a container. `None` for a shell
/// with no integration to offer.
pub fn snippet(shell: Shell) -> Option<&'static str> {
    match shell {
        Shell::Zsh => Some(ZSH_SNIPPET),
        Shell::Bash => Some(BASH_SNIPPET),
        Shell::Fish => Some(FISH_SNIPPET),
        Shell::Other => None,
    }
}

/// The file [`snippet`] belongs at the end of, when a person installs it by
/// hand.
pub fn manual_install_file(shell: Shell) -> Option<&'static str> {
    match shell {
        Shell::Zsh => Some("~/.zshrc"),
        Shell::Bash => Some("~/.bashrc"),
        Shell::Fish => Some("~/.config/fish/config.fish"),
        Shell::Other => None,
    }
}
