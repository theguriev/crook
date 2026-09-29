//! `crook pane …`, `crook tab new` and `crook events --follow`: the command
//! line's end of the socket.
//!
//! The command line is the SDK: what a script or an agent can ask a window is
//! what this prints, a table for a person and, with `--json`, the window's own
//! answer with every field it sent — read as JSON rather than into
//! [`PaneEntry`], so a field a newer window adds reaches a script through an
//! older `crook`. `crook events --follow` prints the window's events as they
//! come, one JSON object a line, since what reads a stream is a program.
//!
//! `crook pane wait` exits with a failure when the pane did not get where it
//! was waited for, having printed where it is, so that `crook pane wait 7
//! --until finished && …` reads the way it looks.
//!
//! # Which window
//!
//! The one in `CROOK_SOCKET`, which every pane's shell is given. An empty one
//! means that pane's Crook has no socket, and a `CROOK_PANE_ID` with no
//! `CROOK_SOCKET` beside it means a Crook from before there was one; both are
//! refused, since either way the window the pane is in cannot be asked, and
//! asking some other window instead would answer a question nobody asked.
//! Outside every pane there is nothing to say which window is meant, so the
//! one live socket in this user's directory is taken when there is exactly
//! one, and several are refused by name: every window is its own process, and
//! the newest is a guess that hands a script another window's panes.
//!
//! # Which pane
//!
//! Every request carries the pane's `CROOK_TOKEN` when the environment has
//! one, and it is the window that decides what it is worth: nothing for a
//! listing, and everything for `tab new`, which is refused to a request
//! without one. The command line does not refuse first, so that the one
//! refusal there is comes from the one place that knows.
//!
//! # Nothing it prints is a control character
//!
//! What a window says about a pane came, in part, from the pane: its
//! directory from the OSC 7 its shell printed, percent-decoded, so `%1b` in it
//! is an ESC; its title from what the program in it set. Anything that can
//! print into one pane could otherwise put an escape sequence in the listing,
//! and `crook pane list` would replay it into the terminal it runs in —
//! retitle that pane, rewrite the rows above, set the clipboard. So the table
//! writes every control character out as its escape, and the JSON escapes the
//! ones a JSON encoder leaves as they are.

use std::path::Path;

use anyhow::{Context, Result, bail};

use super::protocol::{BlocksRead, Follow, NewTab, PaneEntry, ReadBlocks, Until, Wait, Waited};

/// What `crook pane` takes after it, for the refusal that lists it.
const VERBS: &str = "list, wait or blocks";

/// How `crook pane wait` is spelled, for the refusals that show it.
const PANE_WAIT: &str =
    "crook pane wait <ID> --until idle|needs-input|exited|finished [--timeout <SECS>] [--json]";

/// How `crook pane blocks` is spelled, for the refusals that show it.
const PANE_BLOCKS: &str = "crook pane blocks <ID> [--last <N>] [--json]";

/// How `crook events` is spelled, for the refusals that show it.
const EVENTS: &str = "crook events --follow [--pane <ID>]";

/// How `crook tab new` is spelled, for the refusals that show it.
const TAB_NEW: &str =
    "crook tab new [--worktree <BRANCH>] [--in-my-group] [--title <TITLE>] [--json] -- <COMMAND>…";

/// What a command prints, and whether it then fails.
#[derive(Debug, PartialEq, Eq)]
pub struct Printed {
    /// What goes to standard output.
    pub text: String,
    /// Why the command fails after printing it, when it does: a wait that
    /// ran out, or a pane that closed first.
    pub failure: Option<String>,
}

impl From<String> for Printed {
    fn from(text: String) -> Self {
        Self {
            text,
            failure: None,
        }
    }
}

/// What `crook pane …` was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub enum PaneCommand {
    /// `list`, and whether as JSON.
    List {
        /// `--json`.
        json: bool,
    },
    /// `wait`: the wait, and whether to print the answer as JSON.
    Wait {
        /// What is waited for.
        asked: Wait,
        /// `--json`.
        json: bool,
    },
    /// `blocks`: which, and whether as JSON.
    Blocks {
        /// Which pane, and how many.
        asked: ReadBlocks,
        /// `--json`.
        json: bool,
    },
}

/// Answers `crook pane …`, given everything after `pane`, with the text to
/// print.
pub fn pane(args: impl Iterator<Item = String>) -> Result<Printed> {
    match pane_arguments(args)? {
        PaneCommand::List { json } => listed(json).map(Printed::from),
        PaneCommand::Wait { asked, json } => waited(asked, json),
        PaneCommand::Blocks { asked, json } => blocks(asked, json).map(Printed::from),
    }
}

/// Follows `crook events …`, given everything after `events`, writing each
/// event to standard output as it comes, until the window ends the stream.
pub fn events(args: impl Iterator<Item = String>) -> Result<()> {
    let asked = events_arguments(args)?;
    followed(asked)
}

/// Answers `crook tab …`, given everything after `tab`, with the text to
/// print: the new pane's number, or the window's answer as JSON.
pub fn tab(args: impl Iterator<Item = String>) -> Result<String> {
    let (asked, json) = tab_arguments(args)?;
    opened(asked, json)
}

/// What follows `tab`: the tab to open, and whether `--json` was asked for.
///
/// The command is everything after `--`, word for word, and the `--` is
/// required: a command is the part of the line most likely to hold a word
/// that starts with a dash, and without the separator `claude --resume`
/// would be read as a flag of this command's.
pub fn tab_arguments(mut args: impl Iterator<Item = String>) -> Result<(NewTab, bool)> {
    let verb = args
        .next()
        .with_context(|| format!("`crook tab` needs a verb: {TAB_NEW}"))?;
    if verb != "new" {
        bail!("`crook tab` takes new, not {verb}: {TAB_NEW}");
    }
    let mut asked = NewTab::default();
    let mut json = false;
    let mut command = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--" => {
                command = Some(args.by_ref().collect::<Vec<_>>());
                break;
            }
            "--worktree" if asked.worktree.is_none() => {
                asked.worktree = Some(value_of(&mut args, "--worktree")?);
            }
            "--title" if asked.title.is_none() => {
                asked.title = Some(value_of(&mut args, "--title")?);
            }
            "--in-my-group" if !asked.in_my_group => asked.in_my_group = true,
            "--json" if !json => json = true,
            flag @ ("--worktree" | "--title" | "--in-my-group" | "--json") => {
                bail!("`{flag}` was given twice")
            }
            other => bail!("unrecognised argument {other}; the command goes after `--`: {TAB_NEW}"),
        }
    }
    asked.command = command
        .filter(|command| !command.is_empty())
        .with_context(|| format!("`crook tab new` needs a command after `--`: {TAB_NEW}"))?;
    Ok((asked, json))
}

/// The word after a flag of `tab new`'s that takes one.
fn value_of(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    value_after(args, flag, TAB_NEW)
}

/// What follows `pane`: the verb, its pane, and its flags.
///
/// Parsed the same on every platform, so that a line one platform refuses is
/// refused on all of them for the same reason. The pane's number may go
/// before its flags or among them; it is the one word that is not a flag or
/// a flag's value.
pub fn pane_arguments(mut args: impl Iterator<Item = String>) -> Result<PaneCommand> {
    let verb = args
        .next()
        .with_context(|| format!("`crook pane` needs a verb: {VERBS}"))?;
    match verb.as_str() {
        "list" => {
            let mut json = false;
            for argument in args {
                match argument.as_str() {
                    "--json" if !json => json = true,
                    "--json" => bail!("`--json` was given twice"),
                    other => {
                        bail!("unrecognised argument {other}; `crook pane list` takes only --json")
                    }
                }
            }
            Ok(PaneCommand::List { json })
        }
        "wait" => {
            let (mut until, mut timeout, mut json, mut pane) = (None, None, false, None);
            while let Some(argument) = args.next() {
                match argument.as_str() {
                    "--until" if until.is_none() => {
                        let word = value_after(&mut args, "--until", PANE_WAIT)?;
                        until = Some(Until::from_word(&word).with_context(|| {
                            format!("`--until` takes {}, not {word}", Until::WORDS.join(", "))
                        })?);
                    }
                    "--timeout" if timeout.is_none() => {
                        let seconds = value_after(&mut args, "--timeout", PANE_WAIT)?;
                        timeout = Some(seconds.parse::<u64>().with_context(|| {
                            format!("`--timeout` is a whole number of seconds, not {seconds}")
                        })?);
                    }
                    "--json" if !json => json = true,
                    flag @ ("--until" | "--timeout" | "--json") => {
                        bail!("`{flag}` was given twice")
                    }
                    other => pane = Some(pane_number(other, pane, PANE_WAIT)?),
                }
            }
            let pane =
                pane.with_context(|| format!("`crook pane wait` needs a pane: {PANE_WAIT}"))?;
            let until = until.with_context(|| {
                format!("`crook pane wait` needs what to wait --until: {PANE_WAIT}")
            })?;
            Ok(PaneCommand::Wait {
                asked: Wait {
                    pane,
                    until,
                    timeout,
                },
                json,
            })
        }
        "blocks" => {
            let (mut last, mut json, mut pane) = (None, false, None);
            while let Some(argument) = args.next() {
                match argument.as_str() {
                    "--last" if last.is_none() => {
                        let count = value_after(&mut args, "--last", PANE_BLOCKS)?;
                        last = Some(count.parse::<usize>().with_context(|| {
                            format!("`--last` is a whole number of blocks, not {count}")
                        })?);
                    }
                    "--json" if !json => json = true,
                    flag @ ("--last" | "--json") => bail!("`{flag}` was given twice"),
                    other => pane = Some(pane_number(other, pane, PANE_BLOCKS)?),
                }
            }
            let pane =
                pane.with_context(|| format!("`crook pane blocks` needs a pane: {PANE_BLOCKS}"))?;
            Ok(PaneCommand::Blocks {
                asked: ReadBlocks { pane, last },
                json,
            })
        }
        verb => bail!("`crook pane` takes {VERBS}, not {verb}"),
    }
}

/// What follows `events`: `--follow`, which is required — a stream is the only
/// thing it prints — and the one pane, when one is named.
pub fn events_arguments(mut args: impl Iterator<Item = String>) -> Result<Follow> {
    let mut follow = false;
    let mut asked = Follow::default();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--follow" if !follow => follow = true,
            "--pane" if asked.pane.is_none() => {
                let pane = value_after(&mut args, "--pane", EVENTS)?;
                asked.pane = Some(pane_number(&pane, None, EVENTS)?);
            }
            flag @ ("--follow" | "--pane") => bail!("`{flag}` was given twice"),
            other => bail!("unrecognised argument {other}: {EVENTS}"),
        }
    }
    if !follow {
        bail!("`crook events` prints a stream, and needs --follow to say so: {EVENTS}");
    }
    Ok(asked)
}

/// A pane's number, from the word that should be one — refused when it is a
/// flag nobody takes, is not a number, or is a second pane.
fn pane_number(word: &str, already: Option<u64>, usage: &str) -> Result<u64> {
    if word.starts_with('-') {
        bail!("unrecognised argument {word}: {usage}");
    }
    let number = word.parse::<u64>().with_context(|| {
        format!("a pane is its number, as `crook pane list` and CROOK_PANE_ID give it, not {word}")
    })?;
    if already.is_some() {
        bail!("one pane at a time: {usage}");
    }
    Ok(number)
}

/// The word after a flag that takes one.
fn value_after(args: &mut impl Iterator<Item = String>, flag: &str, usage: &str) -> Result<String> {
    args.next()
        .with_context(|| format!("`{flag}` needs a value: {usage}"))
}

#[cfg(not(unix))]
fn listed(json: bool) -> Result<String> {
    let flag = if json { " --json" } else { "" };
    bail!(
        "`crook pane list{flag}` is not available on this platform yet: a window answers on a \
         Unix socket, and the Windows named pipe is still to come"
    )
}

#[cfg(not(unix))]
fn opened(asked: NewTab, json: bool) -> Result<String> {
    let flag = if json { " --json" } else { "" };
    bail!(
        "`crook tab new{flag} -- {}` is not available on this platform yet: a window answers on \
         a Unix socket, and the Windows named pipe is still to come",
        asked.command.join(" ")
    )
}

#[cfg(not(unix))]
fn waited(asked: Wait, json: bool) -> Result<Printed> {
    let flag = if json { " --json" } else { "" };
    bail!(
        "`crook pane wait {} --until {}{flag}` is not available on this platform yet: a window \
         answers on a Unix socket, and the Windows named pipe is still to come",
        asked.pane,
        asked.until.word()
    )
}

#[cfg(not(unix))]
fn blocks(asked: ReadBlocks, json: bool) -> Result<String> {
    let flag = if json { " --json" } else { "" };
    bail!(
        "`crook pane blocks {}{flag}` is not available on this platform yet: a window answers on \
         a Unix socket, and the Windows named pipe is still to come",
        asked.pane
    )
}

#[cfg(not(unix))]
fn followed(asked: Follow) -> Result<()> {
    let pane = asked
        .pane
        .map(|pane| format!(" --pane {pane}"))
        .unwrap_or_default();
    bail!(
        "`crook events --follow{pane}` is not available on this platform yet: a window answers \
         on a Unix socket, and the Windows named pipe is still to come"
    )
}

#[cfg(unix)]
fn listed(json: bool) -> Result<String> {
    let socket = unix::socket_here()?;
    unix::listing(
        &socket,
        unix::token_here().as_deref(),
        json,
        std::env::home_dir().as_deref(),
    )
}

#[cfg(unix)]
fn opened(asked: NewTab, json: bool) -> Result<String> {
    let socket = unix::socket_here()?;
    unix::open_tab(&socket, unix::token_here().as_deref(), asked, json)
}

#[cfg(unix)]
fn waited(asked: Wait, json: bool) -> Result<Printed> {
    let socket = unix::socket_here()?;
    unix::wait(&socket, unix::token_here().as_deref(), asked, json)
}

#[cfg(unix)]
fn blocks(asked: ReadBlocks, json: bool) -> Result<String> {
    let socket = unix::socket_here()?;
    unix::read_blocks(
        &socket,
        unix::token_here().as_deref(),
        asked,
        json,
        std::env::home_dir().as_deref(),
    )
}

#[cfg(unix)]
fn followed(asked: Follow) -> Result<()> {
    let socket = unix::socket_here()?;
    unix::follow(
        &socket,
        unix::token_here().as_deref(),
        asked,
        &mut std::io::stdout().lock(),
    )
}

/// The table `crook pane list` prints: one row a pane, the focused one's
/// number marked with `*`, and what an agent is waiting for on a line of its
/// own under the row, where a person reading down the titles finds it.
pub fn table(panes: &[PaneEntry], home: Option<&Path>) -> String {
    const HEADINGS: [&str; 6] = ["PANE", "STATUS", "TITLE", "GROUP", "BRANCH", "DIRECTORY"];
    let dash = || "-".to_owned();

    let rows: Vec<[String; 6]> = panes
        .iter()
        .map(|pane| {
            [
                match pane.focused {
                    true => format!("{}*", pane.pane_id),
                    false => pane.pane_id.to_string(),
                },
                printable(&pane.status),
                printable(&pane.title),
                pane.group.as_deref().map(printable).unwrap_or_else(dash),
                pane.branch.as_deref().map(printable).unwrap_or_else(dash),
                pane.cwd
                    .as_deref()
                    .map(|directory| {
                        printable(&crate::git::user_friendly_path(Path::new(directory), home))
                    })
                    .unwrap_or_else(dash),
            ]
        })
        .collect();

    // By character rather than by byte: a title is whatever a person or an
    // agent called the work, and the padding is a count of columns.
    let mut widths = HEADINGS.map(|heading| heading.chars().count());
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |cells: [&str; 6]| {
        let mut line = String::new();
        for (column, cell) in cells.iter().enumerate() {
            if column > 0 {
                line.push_str("  ");
            }
            line.push_str(cell);
            if column + 1 < cells.len() {
                let pad = widths[column].saturating_sub(cell.chars().count());
                line.extend(std::iter::repeat_n(' ', pad));
            }
        }
        line.trim_end().to_owned()
    };
    // Under the title, which is where the eye already is.
    let indent = " ".repeat(widths[0] + 2 + widths[1] + 2);

    let mut lines = vec![line(HEADINGS)];
    for (pane, row) in panes.iter().zip(&rows) {
        lines.push(line(row.each_ref().map(String::as_str)));
        if let Some(message) = &pane.message {
            lines.push(format!("{indent}{}", printable(message)));
        }
    }
    lines.join("\n")
}

/// `text` with every control character written out as its escape — `\u{1b}`
/// for an ESC, `\n` for a newline — so that it prints as what it is rather
/// than doing what it says, and a row stays one line.
///
/// Written out rather than dropped, so that a person reading the table sees
/// what a pane tried. See the module docs for why.
fn printable(text: &str) -> String {
    printable_keeping(text, None)
}

/// The same, keeping `keep` as itself: a newline, in the output of a block
/// printed as the lines it was.
fn printable_keeping(text: &str, keep: Option<char>) -> String {
    let mut printable = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() && Some(character) != keep {
            printable.extend(character.escape_default());
        } else {
            printable.push(character);
        }
    }
    printable
}

/// What `crook pane wait` prints for the window's answer: where the pane is,
/// in a word — with what its agent is waiting for, or what its command
/// exited with, after it — and, when it did not get where it was waited for,
/// why the command fails.
pub fn waited_text(waited: &Waited, waited_for: std::time::Duration) -> Printed {
    let state = match (waited.closed, waited.until) {
        (true, Until::Exited) => "exited".to_owned(),
        (true, _) => "closed".to_owned(),
        (false, Until::Finished) if waited.reached => match waited.exit {
            Some(exit) => format!("finished: exit {exit}"),
            None => "finished".to_owned(),
        },
        (false, _) => {
            let status = waited.status.as_deref().map(printable).unwrap_or_default();
            match &waited.message {
                Some(message) => format!("{status}: {}", printable(message)),
                None => status,
            }
        }
    };
    let failure = (!waited.reached).then(|| match waited.closed {
        true => format!(
            "pane {} closed before it was {}",
            waited.pane_id,
            waited.until.word()
        ),
        false => format!(
            "pane {} was not {} within {} seconds; it is {}",
            waited.pane_id,
            waited.until.word(),
            waited_for.as_secs(),
            waited.status.as_deref().map(printable).unwrap_or_default()
        ),
    });
    Printed {
        text: state,
        failure,
    }
}

/// What `crook pane blocks` prints: each block as the command line after a
/// `$`, what it printed, and a line of what is known about how it ended.
pub fn blocks_text(read: &BlocksRead, home: Option<&Path>) -> String {
    let mut lines = Vec::new();
    for block in &read.blocks {
        let command = block
            .command
            .as_deref()
            .map(printable)
            .unwrap_or_else(|| "(a command the shell did not echo)".to_owned());
        lines.push(format!("$ {command}"));
        if block.truncated {
            lines.push("[… the start is cut; this is the end of what it printed]".to_owned());
        }
        if !block.output.is_empty() {
            lines.push(printable_keeping(&block.output, Some('\n')));
        }
        let mut facts = Vec::new();
        if let Some(exit) = block.exit {
            facts.push(format!("exit {exit}"));
        }
        if let Some(duration) = block.duration_ms {
            facts.push(format!("{:.1}s", duration as f64 / 1000.0));
        }
        if let Some(directory) = &block.cwd {
            facts.push(printable(&crate::git::user_friendly_path(
                Path::new(directory),
                home,
            )));
        }
        if !facts.is_empty() {
            lines.push(format!("[{}]", facts.join(" · ")));
        }
    }
    if read.blocks.is_empty() {
        lines.push(format!(
            "pane {} has not finished a command yet",
            read.pane_id
        ));
    }
    if read.grid {
        lines.push(format!(
            "(pane {} is drawing a full-screen program or an agent's TUI now, and what is on \
             its screen is in none of these)",
            read.pane_id
        ));
    }
    lines.join("\n")
}

/// What `crook pane list --json` prints, from the window's `result`.
///
/// Pretty, as `--plugins --json` is: a person reads the output of a command
/// they typed before a script does, and a parser reads either. The encoder
/// escapes the C0 controls in a string and leaves DEL and C1 as they are, and
/// a terminal that reads C1 out of UTF-8 may act on a `U+009B` as a CSI; those
/// are escaped here, as `\u009b`, which any parser reads back as the same
/// character. Outside a string, pretty JSON holds no control character but its
/// newlines, so nothing else is touched.
pub fn json(result: &serde_json::Value) -> String {
    escaped(serde_json::to_string_pretty(result).expect("a value that was just parsed encodes"))
}

/// The same on one line, for a stream: `crook events --follow`'s lines.
pub fn json_line(event: &serde_json::Value) -> String {
    escaped(serde_json::to_string(event).expect("a value that was just parsed encodes"))
}

/// Encoded JSON with every control character but its newlines escaped. See
/// [`json`].
fn escaped(encoded: String) -> String {
    let mut json = String::with_capacity(encoded.len());
    for character in encoded.chars() {
        if character.is_control() && character != '\n' {
            json.push_str(&format!("\\u{:04x}", u32::from(character)));
        } else {
            json.push(character);
        }
    }
    json
}

/// The socket half, which only Unix has.
#[cfg(unix)]
pub mod unix {
    use std::ffi::OsString;
    use std::fs;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use anyhow::{Context, Result, anyhow, bail};
    use serde_json::Value;

    use super::super::protocol::{
        self, BlocksRead, Follow, NewTab, Opened, PaneEntry, ReadBlocks, Reply, Verb, Wait, Waited,
    };
    use super::super::{SOCKET_VARIABLE, TOKEN_VARIABLE, server};
    use super::{Printed, table};

    /// How much longer than the window the command line waits.
    ///
    /// A second past the window's own deadline, so that a window which ran out
    /// of time says so itself rather than being cut off first.
    const PAST_THE_WINDOW: Duration = Duration::from_secs(1);

    /// The longest reply read, in bytes.
    ///
    /// A pane is a few hundred bytes of JSON and a window holds dozens, so
    /// this is far past any real answer; it bounds what a socket that is not
    /// a window's can make the command line hold.
    const MAX_REPLY: u64 = 16 * 1024 * 1024;

    /// The socket to ask from here: this pane's, or the one window running.
    pub fn socket_here() -> Result<PathBuf> {
        use crate::shell_integration::PANE_ID_VARIABLE;

        socket_from(
            std::env::var_os(SOCKET_VARIABLE),
            std::env::var_os(PANE_ID_VARIABLE),
            &server::directory(),
        )
    }

    /// This pane's token, when this runs in a pane that was handed one.
    pub fn token_here() -> Option<String> {
        std::env::var(TOKEN_VARIABLE)
            .ok()
            .filter(|token| !token.is_empty())
    }

    /// The socket to ask, from the pane's environment or, outside any pane,
    /// from what is listening in `directory`. See the module docs for the
    /// rule.
    pub fn socket_from(
        socket: Option<OsString>,
        pane: Option<OsString>,
        directory: &Path,
    ) -> Result<PathBuf> {
        match socket {
            Some(socket) if !socket.is_empty() => return Ok(PathBuf::from(socket)),
            Some(_) => bail!(
                "the Crook this pane is in has no control socket, so it cannot be asked; its log \
                 says why"
            ),
            None if pane.is_some() => bail!(
                "the Crook this pane is in is older than `crook pane` and has no socket to ask; \
                 update it and open a new pane"
            ),
            None => {}
        }

        let live = live_sockets(directory)?;
        match live.as_slice() {
            [] => bail!(
                "no Crook is running for this user on this machine; run this inside a Crook pane"
            ),
            [only] => Ok(only.clone()),
            several => bail!(
                "{} Crook windows are running and nothing says which one is meant; run this \
                 inside a pane of the one you mean, or set {SOCKET_VARIABLE} to one of:\n{}",
                several.len(),
                several
                    .iter()
                    .map(|socket| format!("  {}", socket.display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        }
    }

    /// Every socket in `directory` something is listening on, in name order.
    ///
    /// Only from a directory that passes the window's own check: a socket in
    /// one somebody else could write to could be anybody's, answering
    /// anything.
    fn live_sockets(directory: &Path) -> Result<Vec<PathBuf>> {
        match server::check(directory, server::euid()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).context("not asking any window: the control directory");
            }
        }
        let mut live: Vec<PathBuf> = fs::read_dir(directory)
            .with_context(|| format!("could not read {}", directory.display()))?
            .flatten()
            .filter(|entry| {
                entry
                    .file_type()
                    .is_ok_and(|file_type| file_type.is_socket())
            })
            .map(|entry| entry.path())
            .filter(|path| UnixStream::connect(path).is_ok())
            .collect();
        live.sort();
        Ok(live)
    }

    /// What `crook pane list` prints, having asked the window at `socket`.
    pub fn listing(
        socket: &Path,
        token: Option<&str>,
        json: bool,
        home: Option<&Path>,
    ) -> Result<String> {
        let result = ask(socket, &Verb::PaneList, token)?;
        if json {
            return Ok(super::json(&result));
        }
        let panes: Vec<PaneEntry> =
            serde_json::from_value(result).context("the window's answer is not a list of panes")?;
        Ok(table(&panes, home))
    }

    /// What `crook tab new` prints, having asked the window at `socket` to
    /// open `asked`: the new pane's number, which is what a script goes on to
    /// find in `crook pane list`, or the whole answer with `--json`.
    pub fn open_tab(
        socket: &Path,
        token: Option<&str>,
        asked: NewTab,
        json: bool,
    ) -> Result<String> {
        let result = ask(socket, &Verb::TabNew(asked), token)?;
        if json {
            return Ok(super::json(&result));
        }
        let opened: Opened = serde_json::from_value(result)
            .context("the window's answer is not the tab it opened")?;
        Ok(opened.pane_id.to_string())
    }

    /// What `crook pane wait` prints, having asked the window at `socket` to
    /// wait: where the pane got to, and a failure when it is not where it was
    /// waited for.
    pub fn wait(socket: &Path, token: Option<&str>, asked: Wait, json: bool) -> Result<Printed> {
        let waited_for = asked.timeout();
        let result = ask(socket, &Verb::PaneWait(asked), token)?;
        let waited: Waited = serde_json::from_value(result.clone())
            .context("the window's answer is not a wait's")?;
        let printed = super::waited_text(&waited, waited_for);
        if json {
            return Ok(Printed {
                text: super::json(&result),
                ..printed
            });
        }
        Ok(printed)
    }

    /// What `crook pane blocks` prints, having asked the window at `socket`.
    pub fn read_blocks(
        socket: &Path,
        token: Option<&str>,
        asked: ReadBlocks,
        json: bool,
        home: Option<&Path>,
    ) -> Result<String> {
        let result = ask(socket, &Verb::PaneBlocks(asked), token)?;
        if json {
            return Ok(super::json(&result));
        }
        let read: BlocksRead =
            serde_json::from_value(result).context("the window's answer is not blocks")?;
        Ok(super::blocks_text(&read, home))
    }

    /// Follows the window at `socket`'s events, writing each to `out` as one
    /// line of JSON as it comes, until the window ends the stream or `out`
    /// stops taking lines.
    pub fn follow(
        socket: &Path,
        token: Option<&str>,
        asked: Follow,
        out: &mut impl Write,
    ) -> Result<()> {
        let stream = connect(socket)?;
        let mut replies = BufReader::new(&stream);
        let verb = Verb::EventsFollow(asked);
        request(&stream, &verb, token)?;
        answer(&mut replies)?;
        // The events come when they come: no timeout from here on, and the
        // window ends the stream when there is nothing more to say.
        stream.set_read_timeout(None)?;
        loop {
            let mut line = Vec::new();
            (&mut replies)
                .take(MAX_REPLY)
                .read_until(b'\n', &mut line)
                .context("the window's stream broke off")?;
            if line.is_empty() {
                return Ok(());
            }
            let event: Value = serde_json::from_slice(&line)
                .context("what came down the stream is not an event")?;
            // A reader of the output that has gone — `| head -1` — is the
            // end of following, not a failure.
            if writeln!(out, "{}", super::json_line(&event)).is_err() || out.flush().is_err() {
                return Ok(());
            }
        }
    }

    /// Connects to the window at `socket`.
    fn connect(socket: &Path) -> Result<UnixStream> {
        UnixStream::connect(socket).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => anyhow!(
                "nothing is answering on {}: the Crook that opened it has closed",
                socket.display()
            ),
            _ => anyhow!(error).context(format!("could not connect to {}", socket.display())),
        })
    }

    /// Sends one request, with time for the window to answer it.
    fn request(stream: &UnixStream, verb: &Verb, token: Option<&str>) -> Result<()> {
        let wait = server::DEADLINE.max(verb.patience().unwrap_or_default()) + PAST_THE_WINDOW;
        stream.set_read_timeout(Some(wait))?;
        stream.set_write_timeout(Some(wait))?;
        let mut stream = stream;
        stream
            .write_all(protocol::request_line(verb, token).as_bytes())
            .context("could not ask the window")
    }

    /// Reads one reply, and hands back its answer or says its refusal.
    fn answer(replies: &mut impl BufRead) -> Result<Value> {
        let mut line = Vec::new();
        replies
            .take(MAX_REPLY)
            .read_until(b'\n', &mut line)
            .context("the window did not answer")?;
        if line.is_empty() {
            bail!("the window closed the connection without answering");
        }
        let reply: Reply =
            serde_json::from_slice(&line).context("what came back is not a window's reply")?;
        match (reply.result, reply.error) {
            (Some(result), None) if reply.ok => Ok(result),
            (_, Some(refusal)) => bail!("{} ({})", refusal.message, refusal.code),
            _ => bail!("the window's reply carries neither an answer nor a refusal"),
        }
    }

    /// Asks the window at `socket` one verb, and hands back its answer.
    fn ask(socket: &Path, verb: &Verb, token: Option<&str>) -> Result<Value> {
        let stream = connect(socket)?;
        request(&stream, verb, token)?;
        answer(&mut BufReader::new(&stream))
    }
}
