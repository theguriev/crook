//! The commands this person has already run, read out of the shell's own
//! history file.
//!
//! # Why a file and not the shell
//!
//! Everything else Crook asks a shell it asks *through* the shell — the marks
//! come out of the integration snippet and a completion is a question with an
//! answer. A history could have gone the same way, and deliberately does not:
//! the file is on disk before the pane exists, so a pane can open already
//! knowing what its history is, while a question needs a prompt to arrive
//! first. The whole value of a suggestion is that it is there for the *first*
//! command typed into a fresh pane, which is exactly the moment nothing has
//! been asked yet.
//!
//! It also costs the shell nothing and cannot disturb it. `fc -l` and
//! `history` are commands, and a terminal that quietly runs commands in
//! somebody's shell to populate its own UI is a terminal that shows up in
//! their history, their `preexec` hooks and their `$?`.
//!
//! # What is read
//!
//! The newest [`LIMIT`] lines of the file the shell writes, and only the tail
//! of it — a history file is append-only and a person with a five megabyte
//! `.zsh_history` should not pay for the other four and a half. Duplicates are
//! collapsed onto the newest, because a suggestion that offers the same
//! command five times is five identical rows of the Up key.
//!
//! Each shell writes its own format, and none of them is a list of lines:
//!
//! * **zsh** writes `: <started>:<elapsed>;<command>` when `EXTENDED_HISTORY`
//!   is on and a bare line when it is not, and both appear in one file when
//!   the option is turned on part-way through a life.
//! * **bash** writes bare lines, with a `#<timestamp>` line before each when
//!   `HISTTIMEFORMAT` is set.
//! * **fish** writes YAML-ish records whose command is one `- cmd:` line with
//!   `\\` and `\n` escaped.
//!
//! What none of them writes is a way to tell a multi-line command from two
//! commands, so a command with a newline in it is read as its first line. It
//! is offered as a suggestion that stops at the newline rather than not
//! offered at all, which is the same trade every shell's own history search
//! makes.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::shell_integration::Shell;

/// How many commands a pane starts with.
///
/// The same bound the in-session history has — see
/// [`crate::editor`](crate::editor) — because the two become one list the
/// moment a pane is used, and a bound that only one half respected would be no
/// bound at all.
pub const LIMIT: usize = 1000;

/// How much of the end of a history file is read.
///
/// A megabyte is some tens of thousands of commands, which is more than
/// [`LIMIT`] can hold whatever the average line length turns out to be. The
/// file is append-only, so the end of it is the newest part.
const TAIL: u64 = 1 << 20;

/// The commands the user's shell has run, oldest last used first — read once
/// per process.
///
/// Cached because every pane wants the same list and the answer cannot change
/// in a way anybody would see: a history file grows at the far end while Crook
/// is running, and re-reading it per pane would be one file read per `cmd-t`
/// for entries the pane will have collected for itself by the time they
/// matter.
pub fn user_history() -> &'static [String] {
    // A test's panes open with nothing behind them. What a field suggests
    // would otherwise be a fact about the machine the suite is running on, and
    // a suite that passes on one developer's history and fails on another's is
    // worse than no coverage at all. The parsing is tested against fixtures
    // below, which is where the behaviour actually lives.
    if cfg!(test) {
        return &[];
    }

    static HISTORY: OnceLock<Vec<String>> = OnceLock::new();
    HISTORY.get_or_init(|| {
        let shell = user_shell();
        let Some(path) = path(shell) else {
            log::debug!("no history file for {shell:?}");
            return Vec::new();
        };
        let Some(text) = tail(&path) else {
            return Vec::new();
        };
        let lines = parse(shell, &text);
        log::debug!("read {} commands from {}", lines.len(), path.display());
        lines
    })
}

/// Which shell this person uses, by `$SHELL`.
///
/// The same question [`crate::shell_integration`] asks and the same answer,
/// but asked of the environment rather than of a pane: the history is read
/// before any pane exists, and a person whose panes run something else has a
/// `$SHELL` that says so.
fn user_shell() -> Shell {
    std::env::var_os("SHELL").map_or(Shell::Other, |shell| Shell::of(Path::new(&shell)))
}

/// Where a shell keeps its history, honouring the variable that moves it.
///
/// `HISTFILE` is consulted for zsh and bash because a person who has moved
/// their history has moved it — and because Crook itself sets it, in the zsh
/// stanza that keeps a scratch `ZDOTDIR` from swallowing the history. See
/// [`crate::shell_integration::launch`](crate::shell_integration).
fn path(shell: Shell) -> Option<PathBuf> {
    let named = |variable: &str| std::env::var_os(variable).map(PathBuf::from);
    match shell {
        Shell::Zsh => {
            named("HISTFILE").or_else(|| Some(std::env::home_dir()?.join(".zsh_history")))
        }
        Shell::Bash => {
            named("HISTFILE").or_else(|| Some(std::env::home_dir()?.join(".bash_history")))
        }
        // fish keeps its own, under the data directory rather than the home,
        // and names it after the session — `fish_history` being the default
        // session's.
        Shell::Fish => Some(dirs::data_dir()?.join("fish").join("fish_history")),
        Shell::Other => None,
    }
}

/// The last [`TAIL`] bytes of a file, as text.
///
/// The first line of what is read is dropped when the file was long enough to
/// be cut, because a seek into the middle of a file lands in the middle of a
/// line — and half a command offered as a suggestion is worse than one command
/// fewer.
fn tail(path: &Path) -> Option<String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            log::debug!("could not read {}: {error}", path.display());
            return None;
        }
    };

    let length = file.metadata().ok()?.len();
    let cut = length > TAIL;
    if cut && file.seek(SeekFrom::End(-(TAIL as i64))).is_err() {
        return None;
    }

    let mut bytes = Vec::with_capacity(TAIL.min(length) as usize);
    if let Err(error) = file.read_to_end(&mut bytes) {
        log::debug!("could not read {}: {error}", path.display());
        return None;
    }

    // Lossy, because a history file is not necessarily UTF-8: zsh metafies
    // bytes it cannot encode, and a single command somebody typed in another
    // encoding must not throw away the file it is in.
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if cut {
        let start = text.find('\n').map_or(text.len(), |at| at + 1);
        text.drain(..start);
    }
    Some(text)
}

/// The commands in a history file, oldest first, deduplicated onto the newest
/// use of each.
pub fn parse(shell: Shell, text: &str) -> Vec<String> {
    let commands: Vec<String> = match shell {
        Shell::Zsh => text.lines().filter_map(zsh_entry).collect(),
        Shell::Bash => text
            .lines()
            // `#1699999999` is a `HISTTIMEFORMAT` stamp, not a command. A real
            // comment typed at a prompt is lost with it, which is a command
            // that does nothing being missing from a list of suggestions.
            .filter(|line| !line.starts_with('#'))
            .map(str::to_owned)
            .collect(),
        Shell::Fish => text.lines().filter_map(fish_entry).collect(),
        Shell::Other => Vec::new(),
    };

    newest(commands)
}

/// One zsh entry, with the extended-history stamp taken off the front.
///
/// `: 1699999999:0;git status`, where the number before the semicolon is how
/// long the command took. A line that starts with `: ` and has no semicolon is
/// somebody's `: ` command rather than a stamp, and is kept whole.
fn zsh_entry(line: &str) -> Option<String> {
    let Some(rest) = line.strip_prefix(": ") else {
        return Some(line.to_owned());
    };
    let (stamp, command) = rest.split_once(';')?;
    stamp
        .split_once(':')
        .filter(|(started, elapsed)| {
            started.chars().all(|c| c.is_ascii_digit())
                && elapsed.chars().all(|c| c.is_ascii_digit())
        })
        .map_or_else(|| Some(line.to_owned()), |_| Some(command.to_owned()))
}

/// One fish entry: the `- cmd:` line, unescaped.
///
/// fish writes YAML it does not read with a YAML parser, and neither does
/// this: everything else in a record — `when:`, the `paths:` list — is
/// indented, so a line beginning `- cmd: ` is the whole of what is being
/// looked for.
fn fish_entry(line: &str) -> Option<String> {
    let command = line.strip_prefix("- cmd: ")?;
    let mut unescaped = String::with_capacity(command.len());
    let mut characters = command.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            unescaped.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => unescaped.push('\n'),
            Some('\\') => unescaped.push('\\'),
            // Anything else fish did not escape: the backslash is the
            // command's own.
            Some(other) => {
                unescaped.push('\\');
                unescaped.push(other);
            }
            None => unescaped.push('\\'),
        }
    }
    Some(unescaped)
}

/// The last [`LIMIT`] commands, blanks dropped and each kept only where it was
/// last run.
///
/// Deduplicating onto the *newest* use rather than the oldest is what makes
/// the Up key and the suggestion agree with a person's sense of recency: a
/// command run this morning and again ten minutes ago is ten minutes old.
fn newest(commands: Vec<String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for command in commands.into_iter().rev() {
        // A command with a newline in it is read as its first line — see this
        // module's header — which is what a zsh entry's trailing `\` and a
        // fish entry's `\n` both leave behind.
        let command = command.split('\n').next().unwrap_or_default();
        let command = command.trim_end_matches(['\r', ' ', '\t']);
        let command = command.trim_end_matches('\\');
        if command.trim().is_empty() || kept.iter().any(|seen| seen == command) {
            continue;
        }
        kept.push(command.to_owned());
        if kept.len() == LIMIT {
            break;
        }
    }
    kept.reverse();
    kept
}

#[cfg(test)]
#[path = "shell_history_tests.rs"]
mod tests;
