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
///
/// `login` is the setting: whether the shell is started the way `login(1)` and
/// every other terminal on the desktop start one. See
/// [`crook_terminal::login_arguments`].
pub fn plan(shell: Shell, program: &Path, scratch: &Path, login: bool, host: &HostEnv) -> Launch {
    let planned = match shell {
        Shell::Zsh => zsh(program, scratch, login, host),
        Shell::Bash => bash(program, scratch, login),
        Shell::Fish => fish(program, scratch, login, host),
        Shell::Other => None,
    };
    planned.unwrap_or_else(|| plain(program, login))
}

/// The launch with no integration in it: the shell, started the way the desktop
/// starts one, and nothing in the environment but the two variables that name
/// the terminal.
///
/// This is what an unrecognised shell gets — nushell, xonsh, dash, tcsh,
/// something nobody has thought of. Crook runs the shell the user chose and goes
/// without the marks; substituting a shell that does support them would be a
/// terminal silently disobeying the one thing `$SHELL` says. It is still a login
/// shell: [`Program::login_shell`] asks by argv\[0\] where it has no switch to
/// pass, which is the convention `login(1)` itself uses and the one every shell
/// with a login mode understands.
///
/// A *recognised* shell reaches here too — a zsh with no `HOME` to point its
/// stubs back at, or any of the three with the marks switched off. The marks and
/// the person's own startup files are two separate promises, and losing one must
/// not lose the other.
///
/// What this path does *not* have is `bash_rc`'s fallback to `~/.bashrc` for a
/// home with no profile file in it. That fallback exists because `--rcfile`
/// replaces the file bash would have read and Crook has to put it back; here
/// nothing was replaced, and a real `bash -l` against such a home reads nothing
/// — which is exactly what the same person gets from Terminal.app. Reproducing
/// the platform is what this function is for, generosity included only where
/// Crook took something away.
pub fn plain(program: &Path, login: bool) -> Launch {
    Launch {
        program: program_for(program, login),
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
/// The stubs already covered every file a login zsh reads, which is why `-l`
/// costs nothing here: `/etc/zprofile`, `$ZDOTDIR/.zprofile`, then the rc
/// files, then `/etc/zlogin` and `$ZDOTDIR/.zlogin`. Each stub sources the
/// user's own copy exactly once, and `.zlogin` is the one that could have been
/// sourced twice — the `.zshrc` stub's last act is to hand `ZDOTDIR` back, so
/// by the time zsh goes looking for `$ZDOTDIR/.zlogin` it is looking in the
/// user's own directory and reads their file directly. Crook's `.zlogin` stub
/// is left for the one shell that never read `.zshrc`: a login shell that is
/// not interactive.
///
/// # `ZDOTDIR` has to look untouched to the files that read it
///
/// The stubs run with `ZDOTDIR` pointing at Crook's directory, and the user's
/// files must never see that. `ZDOTDIR=${ZDOTDIR:-$HOME/.config/zsh}` is a
/// common spelling in a `.zshenv` — it takes the wrong branch against a
/// `ZDOTDIR` somebody else set, and the whole of that person's configuration
/// then lives in a directory nothing reads. So each stub puts `ZDOTDIR` back to
/// exactly the state it was in before Crook — *unset*, for almost everybody —
/// before sourcing, and `has_zdotdir` is what carries "was it set?" into the
/// shell. The same restoration is the `.zshrc` stub's last act, so a nested
/// `zsh` or an `exec zsh` in the pane starts from the state a first zsh does.
///
/// `None` when there is no `HOME` and no `ZDOTDIR` to point back at: a shell
/// started with `ZDOTDIR` set to a directory of stubs that source nothing is a
/// shell with none of the user's configuration, which is far worse than a shell
/// with no marks.
pub fn zsh(program: &Path, scratch: &Path, login: bool, host: &HostEnv) -> Option<Launch> {
    let user_zdotdir = host.user_zdotdir()?.to_str()?.to_owned();
    let scratch_text = scratch.to_str()?.to_owned();
    let has_zdotdir = host.zdotdir.is_some();

    let mut environment = identity();
    environment.push(("ZDOTDIR".to_owned(), scratch_text));
    environment.push(("USER_ZDOTDIR".to_owned(), user_zdotdir));

    Some(Launch {
        program: program_for(program, login),
        environment,
        files: vec![
            ScratchFile {
                path: scratch.join(".zshenv"),
                contents: zshenv_stub(has_zdotdir),
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

/// bash: `--rcfile`, pointed at a file that runs the user's own startup files
/// and then carries the integration inline.
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
///
/// # The one shell that will not take `-l`
///
/// bash is the exception to the whole scheme, and it is not a matter of
/// argument order. A login bash reads `/etc/profile` and then the first of
/// `~/.bash_profile`, `~/.bash_login`, `~/.profile` — and it reads *no* rc
/// file at all, `--rcfile`'s included. That is not a side effect of `~/.bashrc`
/// being skipped: `bash --rcfile <file> -l -i` silently never opens `<file>`,
/// which was checked against the 3.2.57 macOS ships and 5.3.9. (`bash -l
/// --rcfile <file>` does not even start: bash takes its long options before its
/// short ones and answers `--: invalid option`.)
///
/// So on bash, `-l` and the marks are mutually exclusive, and the marks are
/// what this module is for. The rc file runs bash's own login sequence itself
/// instead, in bash's own order — see `bash_rc` — and its logout sequence
/// too, since `~/.bash_logout` and the `logout` builtin are the half of a login
/// shell that only shows up on the way out. What it does not reproduce is
/// `shopt -q login_shell`, which stays off, and `$0`, which is the shell's path
/// rather than `-bash`; a script that asks either of those will disagree with
/// what the shell has actually read. What it does reproduce is every file the
/// person actually wrote, which is where their `PATH` and their prompt live. A
/// bash with the marks switched off goes through [`plain`] and is a real login
/// shell.
pub fn bash(program: &Path, scratch: &Path, login: bool) -> Option<Launch> {
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
        environment: bash_environment(),
        files: vec![ScratchFile {
            contents: bash_rc(super::snippet(Shell::Bash)?, login),
            path: rc,
        }],
    })
}

/// What a bash with the marks is started with: the identity, and Apple's
/// notice about zsh switched off.
///
/// The bash macOS ships prints "The default interactive shell is now zsh"
/// before its first prompt unless this variable is set, on every shell it
/// starts — and in Crook that is a block of Apple's prose at the head of
/// every new pane, before the person has typed anything, in a pane that keeps
/// blocks apart on purpose. The notice is about the account's login shell,
/// which nothing in this pane can change; the person who kept bash has read
/// it. Every other bash ignores the variable, so it is not gated on the
/// platform. A bash with the marks off goes through [`plain`] and keeps the
/// notice, the way it keeps everything else the platform does.
fn bash_environment() -> Vec<(String, String)> {
    let mut environment = identity();
    environment.push((
        "BASH_SILENCE_DEPRECATION_WARNING".to_owned(),
        "1".to_owned(),
    ));
    environment
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
///
/// `-l` and `vendor_conf.d` do not interact: fish reads its data directories
/// from the config it always reads, so the integration arrives either way. What
/// `-l` buys on fish is the same thing it buys everywhere — fish's own
/// `config.fish` runs `path_helper` only `if status is-login`, so without it a
/// fish on macOS has a `PATH` that nobody assembled.
pub fn fish(program: &Path, scratch: &Path, login: bool, host: &HostEnv) -> Option<Launch> {
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
        program: program_for(program, login),
        environment,
        files: vec![ScratchFile {
            path: scratch.join(FISH_VENDOR_CONF),
            contents: super::snippet(Shell::Fish)?.to_owned(),
        }],
    })
}

/// How a shell is started: as a login shell when that is wanted, and plainly
/// when it is not.
///
/// One place rather than three, because "is this a login shell?" has to have
/// the same answer for zsh, for fish and for a shell nobody recognised — the
/// setting is one switch, and a person who turns it off has to be able to trust
/// that it is off everywhere. bash is not routed through here and cannot be:
/// see [`bash`].
fn program_for(program: &Path, login: bool) -> Program {
    if login {
        Program::login_shell(program)
    } else {
        Program::command(program, Vec::<OsString>::new())
    }
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

/// The `--rcfile` bash is handed: the user's own startup files, then the
/// integration.
///
/// *Which* of their files depends on `login`, and that is bash's own rule
/// rather than a choice. A login bash reads `/etc/profile` and then the
/// **first** of `~/.bash_profile`, `~/.bash_login`, `~/.profile`, and never
/// reads `~/.bashrc` itself — the profile is what usually does, through the
/// `[ -f ~/.bashrc ] && . ~/.bashrc` line nearly every `~/.bash_profile`
/// carries. A non-login interactive bash reads `~/.bashrc` and none of the
/// profile files. Doing both halves would source `~/.bashrc` twice for almost
/// everybody, and a twice-sourced rc is a prompt framework installed twice.
///
/// The one thing added to bash's rule is the fallback. When *no* profile file
/// exists, bash's login path would have read nothing at all, so the file a
/// non-login bash would have read is used instead. That is the person on Linux
/// whose terminal has always started a non-login shell and who therefore has a
/// `~/.bashrc` and nothing else; without it, turning this setting on would
/// empty their shell.
fn bash_rc(integration: &str, login: bool) -> String {
    let user_files = if login {
        BASH_LOGIN_FILES
    } else {
        BASH_INTERACTIVE_FILES
    };
    format!(
        "# Crook hands this file to bash with --rcfile, which REPLACES the files
# bash would have read rather than adding to them. Running them here is not a
# nicety: without this the shell starts with none of their configuration.
{user_files}
# Second, deliberately. A prompt framework loaded by the files above that
# assigns PROMPT_COMMAND or installs a DEBUG trap has already done so by this
# point, and what follows chains onto what it finds instead of replacing it.

{integration}"
    )
}

/// What the rc file runs when this is not a login shell: exactly the one file
/// bash would have read on its own, which `--rcfile` is standing in front of.
const BASH_INTERACTIVE_FILES: &str = "\
if [ -f \"$HOME/.bashrc\" ]; then
\t. \"$HOME/.bashrc\"
fi
";

/// What it runs when this is a login shell.
///
/// bash will not be given a login shell *and* an rc file — see [`bash`] — so
/// this is bash's own login sequence, written out, in bash's own order. The
/// `break` is the whole point of the middle stanza: bash reads the first of the
/// three that exists and stops, and a launcher that sourced all three would run
/// a `~/.profile` that no login shell of theirs has ever run.
const BASH_LOGIN_FILES: &str = "\
if [ -f /etc/profile ]; then
\t. /etc/profile
fi

__crook_profile=
for __crook_candidate in \"$HOME/.bash_profile\" \"$HOME/.bash_login\" \"$HOME/.profile\"; do
\tif [ -f \"$__crook_candidate\" ]; then
\t\t. \"$__crook_candidate\"
\t\t__crook_profile=$__crook_candidate
\t\tbreak
\tfi
done

# A login bash never reads ~/.bashrc itself; the profile above is what normally
# does, and sourcing it here as well would run it twice. This is only for the
# person with no profile file at all, whose login shell would otherwise start
# with nothing.
if [ -z \"$__crook_profile\" ] && [ -f \"$HOME/.bashrc\" ]; then
\t. \"$HOME/.bashrc\"
fi
unset __crook_profile __crook_candidate

# The other end of a login shell, which bash will not give this one either: a
# login bash reads ~/.bash_logout on its way out and answers to `logout`. Both
# are visible — a .bash_logout that runs `ssh-agent -k` or `history -a` simply
# stops running, and `logout` answers \"not login shell\" in a pane where every
# other terminal on the machine closes.
__crook_prior_exit_trap=$(trap -p EXIT)
__crook_logout() {
\tif [ -n \"$__crook_prior_exit_trap\" ]; then
\t\t# `trap -p` prints the line that would reinstall the trap:
\t\t# trap -- '<shellcode>' EXIT. Read back as an array, element 2 is the
\t\t# shellcode, quoted exactly as it went in.
\t\t__crook_exit_parts=()
\t\teval \"__crook_exit_parts=( $__crook_prior_exit_trap )\"
\t\teval \"${__crook_exit_parts[2]}\"
\tfi
\tif [ -f \"$HOME/.bash_logout\" ]; then
\t\t. \"$HOME/.bash_logout\"
\tfi
}
trap __crook_logout EXIT

logout() {
\tbuiltin exit \"$@\"
}
";

/// `$ZDOTDIR/.zshenv`: the first file zsh reads, in every kind of shell.
///
/// `set` says whether the person already had a `ZDOTDIR` when Crook started.
/// That is the state their own files have to be shown — see [`zsh`] — and it is
/// not knowable from inside the shell, because by then Crook has overwritten
/// it.
fn zshenv_stub(set: bool) -> String {
    let had_one = if set { "1" } else { "" };
    format!(
        "\
# Crook points ZDOTDIR at a scratch directory of its own so it can install shell
# integration without writing to a file you own. Every stub in it sources your
# real file; USER_ZDOTDIR is where those live.
CROOK_ZDOTDIR=$ZDOTDIR
# Whether you had a ZDOTDIR of your own before Crook set one. Nothing inside the
# shell can tell, and your files must not be shown Crook's.
CROOK_USER_ZDOTDIR_SET={had_one}

# ZDOTDIR as your own files would have found it. `ZDOTDIR=${{ZDOTDIR:-...}}` is a
# common line in a .zshenv, and against a ZDOTDIR that somebody else set it
# takes the wrong branch and loses every file it was pointing at.
__crook_user_zdotdir() {{
\tif [[ -n $CROOK_USER_ZDOTDIR_SET ]]; then
\t\tZDOTDIR=$USER_ZDOTDIR
\telse
\t\tunset ZDOTDIR
\tfi
}}

# The second test is the one thing that can loop: a .zshenv reached through a
# USER_ZDOTDIR that is already Crook's own directory is this file.
if [[ -f $USER_ZDOTDIR/.zshenv && $USER_ZDOTDIR != \"$CROOK_ZDOTDIR\" ]]; then
\t__crook_user_zdotdir
\t. $USER_ZDOTDIR/.zshenv
\t# Wherever it decided your files live is where the rest of the stubs look,
\t# and zsh's own rule for that is ZDOTDIR when it is set and HOME when it is
\t# not.
\tif [[ -n ${{ZDOTDIR+set}} ]]; then
\t\tCROOK_USER_ZDOTDIR_SET=1
\t\tUSER_ZDOTDIR=$ZDOTDIR
\telse
\t\tCROOK_USER_ZDOTDIR_SET=
\t\tUSER_ZDOTDIR=$HOME
\tfi
\tZDOTDIR=$CROOK_ZDOTDIR
fi
"
    )
}

/// `$ZDOTDIR/.zprofile`: login shells only, read before `.zshrc`.
const ZPROFILE_STUB: &str = "\
if [[ -f $USER_ZDOTDIR/.zprofile ]]; then
\t__crook_user_zdotdir
\t. $USER_ZDOTDIR/.zprofile
\tZDOTDIR=$CROOK_ZDOTDIR
fi
";

/// `$ZDOTDIR/.zshrc`: interactive shells, and where the integration is
/// installed.
///
/// The `HISTFILE` stanza is not belt-and-braces. macOS ships an `/etc/zshrc`
/// that runs before this one and does `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history`
/// — with `ZDOTDIR` being Crook's scratch directory, which is deleted the moment
/// the pane closes. Testing only for an empty `HISTFILE`, which is what every
/// framework's own guard does, never fires there: the pane starts with an empty
/// history, and everything typed into it is thrown away on exit. So the test is
/// also "does it point inside Crook's directory", which is the one answer that
/// cannot have come from the user.
const ZSHRC_STUB: &str = "\
# zsh resolves HISTFILE against ZDOTDIR, and this shell's ZDOTDIR is a scratch
# directory that will not exist tomorrow. macOS's own /etc/zshrc has already
# resolved it against that directory by the time this runs, so an empty HISTFILE
# is not the only case: one pointing into the scratch is Crook's doing too, and
# leaving it there loses the history for good.
if [[ -z $HISTFILE || $HISTFILE == \"$CROOK_ZDOTDIR\"/* ]]; then
\tHISTFILE=$USER_ZDOTDIR/.zsh_history
fi

if [[ -f $USER_ZDOTDIR/.zshrc ]]; then
\t__crook_user_zdotdir
\t. $USER_ZDOTDIR/.zshrc
\tZDOTDIR=$CROOK_ZDOTDIR
fi

# The user's configuration first and the integration second, so a framework that
# assigns precmd_functions wholesale cannot wipe the hooks.
. $CROOK_ZDOTDIR/crook.zsh

# Nothing from here on reads ZDOTDIR expecting Crook's copy, and a great deal
# reads it expecting the user's — zsh itself included, for .zlogin. Left the way
# it was found rather than merely pointed at the user's directory: an exported
# ZDOTDIR where there was none is what a nested zsh, or `exec zsh`, would then
# take the wrong branch on.
__crook_user_zdotdir
";

/// `$ZDOTDIR/.zlogin`: login shells, read after `.zshrc`.
const ZLOGIN_STUB: &str = "\
# Only reached by a login shell that never read .zshrc, since .zshrc is what
# puts ZDOTDIR back and an interactive shell would have read the user's own
# .zlogin directly.
__crook_user_zdotdir
if [[ -f $USER_ZDOTDIR/.zlogin ]]; then
\t. $USER_ZDOTDIR/.zlogin
fi
";
