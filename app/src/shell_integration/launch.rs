//! Deciding how to start a shell so that it emits OSC 133, without touching a
//! file the user owns.
//!
//! Everything here is a pure function: given the shell, a directory Crook may
//! write scratch files into, and the few environment variables the answer
//! depends on, it returns the program to run, the environment to run it in, and
//! the files that have to exist first. Nothing in this module opens, creates or
//! deletes anything — that is [`Session`]'s job, and keeping the two apart is
//! what makes "does bash source the user's rc before the integration?" a
//! question a test can ask without a filesystem.
//!
//! [`Session`]: super::Session

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crook_terminal::Program;

use super::{Shell, TERM_PROGRAM, VERSION};

/// The file name the zsh integration is written under, inside the scratch
/// directory that also holds the `ZDOTDIR` stubs.
const ZSH_INTEGRATION: &str = "crook.zsh";

/// The file bash is pointed at with `--rcfile`.
const BASH_RC: &str = "bashrc";

/// Where fish looks for vendor configuration inside an `XDG_DATA_DIRS` entry.
/// fish sources every `.fish` file it finds there, before `config.fish`, with
/// no argument or setting needed.
const FISH_VENDOR_CONF: &str = "fish/vendor_conf.d/crook.fish";

/// What `XDG_DATA_DIRS` means when it is unset, per the XDG base directory
/// specification. Prepending the scratch directory to an unset variable would
/// otherwise silently drop these.
const XDG_DATA_DIRS_DEFAULT: &str = "/usr/local/share:/usr/share";

/// The parts of the ambient environment a launch plan depends on.
///
/// Passed in rather than read, so the planners stay pure and a test can ask
/// what happens with no `HOME`, or with a `ZDOTDIR` the user set themselves,
/// without touching the process's own environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostEnv {
    /// `$HOME`.
    pub home: Option<PathBuf>,
    /// `$ZDOTDIR`, when the user set one. This is where their zsh files live
    /// when it is set, and `$HOME` when it is not.
    pub zdotdir: Option<PathBuf>,
    /// `$XDG_DATA_DIRS`, verbatim.
    pub xdg_data_dirs: Option<String>,
}

impl HostEnv {
    /// What this process was started with.
    pub fn current() -> Self {
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
            zdotdir: std::env::var_os("ZDOTDIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
            xdg_data_dirs: std::env::var("XDG_DATA_DIRS")
                .ok()
                .filter(|value| !value.is_empty()),
        }
    }

    /// The directory the user's own zsh startup files live in.
    fn user_zdotdir(&self) -> Option<&Path> {
        self.zdotdir.as_deref().or(self.home.as_deref())
    }
}

/// One file a launch needs to exist before the shell starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScratchFile {
    /// Where to write it. Always inside the scratch directory the plan was
    /// given, and possibly in a subdirectory of it that does not exist yet.
    pub path: PathBuf,
    /// What to write.
    pub contents: String,
}

/// Everything `Terminal::spawn` needs, and the files that have to exist first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The shell and the arguments it is started with.
    pub program: Program,
    /// Environment variables to apply on top of the inherited ones.
    pub environment: Vec<(String, String)>,
    /// Files to write before spawning. Empty when this shell gets no
    /// integration, which is also what says no scratch directory is needed.
    pub files: Vec<ScratchFile>,
}

impl Launch {
    /// Whether this launch installs the marks.
    pub fn marks(&self) -> bool {
        !self.files.is_empty()
    }
}

/// The launch for `shell`, or the plain one when this shell has no integration
/// or the environment cannot carry it.
///
/// `scratch` is a directory Crook may write into. It does not have to exist:
/// a plan whose `files` are empty is the signal that it never will.
pub fn plan(shell: Shell, program: &Path, scratch: &Path, host: &HostEnv) -> Launch {
    let planned = match shell {
        Shell::Zsh => zsh(program, scratch, host),
        Shell::Bash => bash(program, scratch),
        Shell::Fish => fish(program, scratch, host),
        Shell::Other => None,
    };
    planned.unwrap_or_else(|| plain(program))
}

/// The launch Crook would have used before any of this existed: the shell, no
/// arguments, and nothing in the environment but the two variables that name
/// the terminal.
///
/// This is what an unrecognised shell gets — nushell, xonsh, dash, something
/// nobody has thought of. Crook runs the shell the user chose and goes without
/// the marks; substituting a shell that does support them would be a terminal
/// silently disobeying the one thing `$SHELL` says.
pub fn plain(program: &Path) -> Launch {
    Launch {
        program: Program::command(program, Vec::<OsString>::new()),
        environment: identity(),
        files: Vec::new(),
    }
}

/// zsh: a scratch `ZDOTDIR` whose stubs source the user's real files first and
/// the integration second.
///
/// zsh has no `--rcfile`, and the only thing it will follow to a different
/// directory is `ZDOTDIR`. So Crook points it at a directory of its own holding
/// a stub per startup file, and passes the real one along in `USER_ZDOTDIR` for
/// the stubs to source. This is the arrangement VS Code uses, whose scripts are
/// MIT and were read as a reference.
///
/// `None` when there is no `HOME` and no `ZDOTDIR` to point back at: a shell
/// started with `ZDOTDIR` set to a directory of stubs that source nothing is a
/// shell with none of the user's configuration, which is far worse than a shell
/// with no marks.
pub fn zsh(program: &Path, scratch: &Path, host: &HostEnv) -> Option<Launch> {
    let user_zdotdir = host.user_zdotdir()?.to_str()?.to_owned();
    let scratch_text = scratch.to_str()?.to_owned();

    let mut environment = identity();
    environment.push(("ZDOTDIR".to_owned(), scratch_text));
    environment.push(("USER_ZDOTDIR".to_owned(), user_zdotdir));

    Some(Launch {
        program: Program::command(program, Vec::<OsString>::new()),
        environment,
        files: vec![
            ScratchFile {
                path: scratch.join(".zshenv"),
                contents: ZSHENV_STUB.to_owned(),
            },
            ScratchFile {
                path: scratch.join(".zprofile"),
                contents: ZPROFILE_STUB.to_owned(),
            },
            ScratchFile {
                path: scratch.join(".zshrc"),
                contents: ZSHRC_STUB.to_owned(),
            },
            ScratchFile {
                path: scratch.join(".zlogin"),
                contents: ZLOGIN_STUB.to_owned(),
            },
            ScratchFile {
                path: scratch.join(ZSH_INTEGRATION),
                contents: super::snippet(Shell::Zsh)?.to_owned(),
            },
        ],
    })
}

/// bash: `--rcfile`, pointed at a file that sources `~/.bashrc` itself and
/// then carries the integration inline.
///
/// `--rcfile` *replaces* `~/.bashrc`; it does not add to it. A launcher that
/// forgets to source it hands the user a shell with none of their aliases,
/// their prompt or their `PATH` additions, and nothing on screen says why.
///
/// One file, not two, and that is bash 3.2 — still the bash macOS ships. When a
/// DEBUG trap is already installed and a second one is set from inside a
/// nested `source`, bash 3.2 throws the second one away as that file returns.
/// Sourcing the integration from the rc file is exactly that shape, so anyone
/// whose `~/.bashrc` traps DEBUG — bash-preexec is not the only thing that
/// does — would get a shell with prompt marks and no command marks at all.
/// Inline, the trap is set by the rc file itself, which is a level bash 3.2
/// does not unwind.
pub fn bash(program: &Path, scratch: &Path) -> Option<Launch> {
    let rc = scratch.join(BASH_RC);

    Some(Launch {
        program: Program::command(
            program,
            vec![
                OsString::from("--rcfile"),
                rc.clone().into_os_string(),
                OsString::from("-i"),
            ],
        ),
        environment: identity(),
        files: vec![ScratchFile {
            contents: bash_rc(super::snippet(Shell::Bash)?),
            path: rc,
        }],
    })
}

/// fish: a scratch directory at the front of `XDG_DATA_DIRS`, holding a
/// `vendor_conf.d` file.
///
/// fish needs no argument for this. It sources every `.fish` file in
/// `<dir>/fish/vendor_conf.d` for each `<dir>` in `XDG_DATA_DIRS`, which means
/// the integration arrives without `--init-command` and without `--no-config`,
/// and the user's own configuration loads exactly as it always did.
///
/// The file is installed whatever version of fish this is, because the version
/// is not knowable from here without running the binary, and the snippet is
/// what decides: fish 4.0 emits all four marks from its own reader, and the
/// snippet stands down when it finds that feature on rather than giving every
/// block two of every mark.
pub fn fish(program: &Path, scratch: &Path, host: &HostEnv) -> Option<Launch> {
    let scratch_text = scratch.to_str()?;
    let existing = host
        .xdg_data_dirs
        .as_deref()
        .unwrap_or(XDG_DATA_DIRS_DEFAULT);

    let mut environment = identity();
    environment.push((
        "XDG_DATA_DIRS".to_owned(),
        format!("{scratch_text}:{existing}"),
    ));

    Some(Launch {
        program: Program::command(program, Vec::<OsString>::new()),
        environment,
        files: vec![ScratchFile {
            path: scratch.join(FISH_VENDOR_CONF),
            contents: super::snippet(Shell::Fish)?.to_owned(),
        }],
    })
}

/// The two variables every Crook shell gets, marks or no marks. They are how a
/// script, a prompt framework or a person at the prompt tells which terminal
/// they are in, and they are not part of the injection: an unrecognised shell
/// gets them too.
fn identity() -> Vec<(String, String)> {
    vec![
        ("TERM_PROGRAM".to_owned(), TERM_PROGRAM.to_owned()),
        ("TERM_PROGRAM_VERSION".to_owned(), VERSION.to_owned()),
    ]
}

/// The `--rcfile` bash is handed: the user's own rc, then the integration.
fn bash_rc(integration: &str) -> String {
    format!(
        "# Crook hands this file to bash with --rcfile, which REPLACES ~/.bashrc
# rather than adding to it. Sourcing the user's own file here is not a nicety:
# without this the shell starts with none of their configuration.
if [ -f \"$HOME/.bashrc\" ]; then
\t. \"$HOME/.bashrc\"
fi

# Second, deliberately. A prompt framework loaded by ~/.bashrc that assigns
# PROMPT_COMMAND or installs a DEBUG trap has already done so by this point, and
# what follows chains onto what it finds instead of replacing it.

{integration}"
    )
}

/// `$ZDOTDIR/.zshenv`: the first file zsh reads, in every kind of shell.
const ZSHENV_STUB: &str = "\
# Crook points ZDOTDIR at a scratch directory of its own so it can install shell
# integration without writing to a file you own. Every stub in it sources your
# real file; USER_ZDOTDIR is where those live.
CROOK_ZDOTDIR=$ZDOTDIR
if [[ -f $USER_ZDOTDIR/.zshenv ]]; then
\tZDOTDIR=$USER_ZDOTDIR
\t# A .zshenv that sets ZDOTDIR itself is the one thing that can point this
\t# back at the scratch directory and loop.
\tif [[ $USER_ZDOTDIR != $CROOK_ZDOTDIR ]]; then
\t\t. $USER_ZDOTDIR/.zshenv
\tfi
\tUSER_ZDOTDIR=$ZDOTDIR
\tZDOTDIR=$CROOK_ZDOTDIR
fi
";

/// `$ZDOTDIR/.zprofile`: login shells only, read before `.zshrc`.
const ZPROFILE_STUB: &str = "\
if [[ -f $USER_ZDOTDIR/.zprofile ]]; then
\tZDOTDIR=$USER_ZDOTDIR
\t. $USER_ZDOTDIR/.zprofile
\tZDOTDIR=$CROOK_ZDOTDIR
fi
";

/// `$ZDOTDIR/.zshrc`: interactive shells, and where the integration is
/// installed.
const ZSHRC_STUB: &str = "\
# zsh resolves HISTFILE against ZDOTDIR, and this shell's ZDOTDIR is a scratch
# directory that will not exist tomorrow. Point it at the real one before the
# user's rc runs, since that is where a custom HISTFILE would be set.
[[ -z $HISTFILE ]] && HISTFILE=$USER_ZDOTDIR/.zsh_history

if [[ -f $USER_ZDOTDIR/.zshrc ]]; then
\tZDOTDIR=$USER_ZDOTDIR
\t. $USER_ZDOTDIR/.zshrc
\tZDOTDIR=$CROOK_ZDOTDIR
fi

# The user's configuration first and the integration second, so a framework that
# assigns precmd_functions wholesale cannot wipe the hooks.
. $CROOK_ZDOTDIR/crook.zsh

# Nothing from here on reads ZDOTDIR expecting Crook's copy, and a great deal
# reads it expecting the user's — zsh itself included, for .zlogin.
ZDOTDIR=$USER_ZDOTDIR
";

/// `$ZDOTDIR/.zlogin`: login shells, read after `.zshrc`.
const ZLOGIN_STUB: &str = "\
# Only reached by a login shell that never read .zshrc, since .zshrc is what
# puts ZDOTDIR back and an interactive shell would have read the user's own
# .zlogin directly.
ZDOTDIR=$USER_ZDOTDIR
if [[ -f $USER_ZDOTDIR/.zlogin ]]; then
\t. $USER_ZDOTDIR/.zlogin
fi
";
