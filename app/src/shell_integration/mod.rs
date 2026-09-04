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
//! * **bash** — `--rcfile <stub> -i`, where the stub sources the user's own
//!   startup files itself. `--rcfile` replaces the user's rc rather than adding
//!   to it, so those lines are load-bearing: forget them and the shell starts
//!   with none of their configuration and nothing on screen explains why.
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
//! # Started the way a terminal starts a shell
//!
//! All of that is about *marks*. The other half of "this is my terminal" is
//! that the shell reads the files the person actually configured, and that is
//! a login shell — `/etc/zprofile` and `~/.zprofile` on zsh, `/etc/profile` and
//! `~/.bash_profile` on bash, `path_helper` on macOS in both. A non-login shell
//! skips every one of them, which on a Mac means a `PATH` that nobody
//! assembled: different tools, different versions, in a different order than
//! the same person sees in Terminal.app. [`Options::login`] is that switch and
//! [`login_by_default`] is where its default is argued out per platform, since
//! macOS terminals start login shells and Linux ones do not. The *how* is in
//! [`crook_terminal::login_arguments`] and [`crook_terminal::Program`]: `-l`
//! for the shells whose switch Crook has checked, and `login(1)`'s own argv\[0\]
//! convention for the ones it has not, so tcsh and ksh are login shells too.
//! bash is the exception and [`launch::bash`] is where it is explained: bash
//! will not accept a login shell and an rc file at the same time, so its rc
//! file runs bash's own login sequence — and its logout sequence — instead.
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
//! `crook --shell-integration <shell>` prints the same text for a person to
//! paste into their own configuration on that machine, with the file
//! [`manual_install_file`] names at the top of it. The snippets are written to
//! be sourced that way: interactive-only, guarded against being sourced twice,
//! and chaining onto whatever hooks they find rather than replacing them.
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

/// Whether a pane's shell is a login shell when nobody has said otherwise.
///
/// The question this answers is not "which is better" but "what does the
/// terminal beside it on this desktop do", and the two mainstream desktops
/// answer it differently for reasons that are true of each of them:
///
/// * **macOS: yes.** Terminal.app runs `login(1)`, iTerm2 and WezTerm start
///   login shells, and `/etc/zprofile` is where `path_helper` assembles `PATH`
///   out of `/etc/paths` and `/etc/paths.d` — nothing else runs it. A non-login
///   shell on a Mac has a `PATH` nobody assembled, so it finds different tools,
///   and different versions of them, than every other terminal on the machine.
/// * **Linux: no.** GNOME Terminal, Konsole and xfce4-terminal all start a
///   non-login shell, and Linux configurations are written to match: `PATH` and
///   the prompt go in `~/.bashrc` or `~/.zshrc`, and a `~/.bash_profile` that
///   does not source `~/.bashrc` is an ordinary thing to have. Defaulting to a
///   login shell there would read `/etc/profile` and the profile, skip
///   `~/.bashrc` — that is bash's rule, not Crook's — and hand the person a
///   shell with no aliases and no prompt, unlike the terminal next to it.
///
/// Windows keeps the macOS answer, and it costs nothing: PowerShell and cmd
/// have no login mode at all, and a bash there is Git Bash, which mintty starts
/// with `--login`.
///
/// It is a default and not a rule. `GeneralOptions::login_shell` is the switch,
/// and someone who keeps the same dotfiles on both platforms can have the same
/// shell on both by setting it.
///
/// [`GeneralOptions::login_shell`]: crate::settings::GeneralOptions::login_shell
pub const fn login_by_default() -> bool {
    !cfg!(target_os = "linux")
}

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
    /// Whether the shell is started as a *login* shell — the way `login(1)`,
    /// Terminal.app, iTerm2 and WezTerm all start one.
    ///
    /// A person's `PATH` is assembled by the files only a login shell reads, so
    /// a terminal that disagrees with the one beside it about this is showing a
    /// different machine. Which way that points is the desktop's answer rather
    /// than Crook's: see [`login_by_default`].
    ///
    /// Independent of [`Self::enabled`]: the marks and the startup files are
    /// two different promises and a person may want either without the other.
    pub login: bool,
    /// The shell to run, or the user's own when unset.
    pub shell: Option<PathBuf>,
}

impl Default for Options {
    /// Marks on, the user's shell, and a login shell where that is what the
    /// desktop's own terminal does — see [`login_by_default`].
    fn default() -> Self {
        Self {
            enabled: true,
            login: login_by_default(),
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
