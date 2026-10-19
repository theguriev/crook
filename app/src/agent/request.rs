//! What an agent asks permission for, in the one shape Crook will show.
//!
//! An agent's permission hook is handed the tool it wants to run and that
//! tool's input, in the agent's own schema, which the agent is free to grow.
//! A question is asked of the window only when every key of that input is one
//! Crook knows how to show, with the type it expects: what a person is asked
//! to allow has to be the whole of what will run, and an input with a key
//! Crook has never heard of — a new flag that changes where or how a command
//! runs — is one it cannot promise that about. So [`from_claude`] reads
//! Claude Code's `Bash`, `Edit`, `MultiEdit` and `Write` inputs, each through
//! an allowlist of its own, into an [`AskRequest`] and answers `None` for
//! anything else, and the agent's own prompt is then the only one, as it
//! always was.
//!
//! A change to a file is shown as the change: [`AskRequest::change`] is the
//! lines a card draws, what an edit takes out and what it puts in, and a
//! change with more lines than the Changes column draws of a diff is no
//! request at all — a card that showed part of it would be asking a person
//! to allow what they had not read. Nothing reads the file: what is on disk
//! is not what the agent was handed, and a read on the window's thread is a
//! stall the window does not take for anyone.
//!
//! The same request is what both ends name an ask by. [`digest`] is SHA-256
//! over a domain tag, the session, the tool and the request as
//! [`canonical`] JSON — keys sorted by hand, so that no feature a dependency
//! turns on in `serde_json` can change the bytes — and [`fingerprint`] is its
//! first half in hex, which a later hook computes from the same tool input to
//! say the question has been answered elsewhere.
//!
//! [`visible`] is how the text is shown: every character that could make a
//! command read as something other than what runs — an escape, a carriage
//! return, a bidirectional override, a zero-width joiner — written out as a
//! mark a person can see.
//!
//! Nothing here touches a socket, a terminal or a file, and all of it builds
//! on every platform.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// The longest command a question carries, in bytes.
///
/// Forty-eight kilobytes: a script pasted into a command is a few, and this
/// keeps the request inside the socket's one line however its JSON escapes
/// it, with room for everything else the line carries.
pub const MAX_COMMAND: usize = 48 * 1024;

/// The longest note — the agent's own description of a command — in bytes.
pub const MAX_NOTE: usize = 1024;

/// The longest path a change to a file names, in characters as [`visible`]
/// writes it out.
///
/// Four lines or so of the card, which draws the path whole above the change
/// and does not scroll it: a path long enough to push the change and the
/// buttons out of a window is one the card could not show whole, and its
/// agent's own prompt asks about it instead. Real paths are a fraction of
/// this.
pub const MAX_PATH: usize = 256;

/// The most lines a change to a file is drawn as, [`AskRequest::change`]
/// counted: the most the Changes column draws of one file's diff.
pub const MAX_CHANGE_LINES: usize = crate::git::changes::MAX_DIFF_LINES;

/// The domain tag a [`digest`] starts with, so that no other hash over the
/// same bytes can be taken for one.
const DOMAIN: &str = "crook-ask-v1";

/// How many columns a tab stop is, for [`visible`].
const TAB_STOP: usize = 8;

/// One request an agent asks a person to allow, normalised.
///
/// Read off the wire with every unknown field refused, a `kind` included: a
/// window that showed less than the request says would be asking a person to
/// allow something it had not shown them.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AskRequest {
    /// A shell command.
    Command {
        /// The command line, exactly as the agent would run it.
        command: String,
        /// What the agent says the command does: its words, not Crook's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        /// How long the agent would let it run, in milliseconds, when it
        /// said.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        /// Whether it would run in the background, the agent going on
        /// without waiting for it.
        background: bool,
        /// Whether it would run outside the agent's sandbox.
        outside_sandbox: bool,
    },
    /// Edits to a file: Claude Code's `Edit`, which is one, or its
    /// `MultiEdit`, which is several made in order.
    Edit {
        /// The file, absolute, as the agent named it.
        path: String,
        /// What is replaced with what, in the order they are made.
        edits: Vec<FileEdit>,
    },
    /// A file written whole: created, or replaced if it is there.
    Write {
        /// The file, absolute, as the agent named it.
        path: String,
        /// Everything it will hold.
        content: String,
    },
}

/// One replacement in a file.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileEdit {
    /// The text taken out.
    pub old: String,
    /// The text put in its place.
    pub new: String,
    /// Whether every place `old` is found is replaced, not only the one.
    pub replace_all: bool,
}

/// What a line of a [`AskRequest::change`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Taken out of the file.
    Removed,
    /// Put into it.
    Added,
    /// Which of several edits the lines under it are, and how it is made.
    Heading,
}

/// One line of a change to a file, as a card draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    /// What the line is.
    pub mark: Mark,
    /// Its text, written out through [`visible`], with no newline.
    pub text: String,
}

impl AskRequest {
    /// The word on the wire for what kind of request this is.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Command { .. } => "command",
            Self::Edit { .. } => "edit",
            Self::Write { .. } => "write",
        }
    }

    /// The command, for a request that is one.
    pub fn command(&self) -> Option<&str> {
        match self {
            Self::Command { command, .. } => Some(command),
            Self::Edit { .. } | Self::Write { .. } => None,
        }
    }

    /// The file, for a request that changes one.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Command { .. } => None,
            Self::Edit { path, .. } | Self::Write { path, .. } => Some(path),
        }
    }

    /// The lines a change to a file is drawn as, or nothing for a command.
    ///
    /// Each edit's old text as removed lines, then its new text as added
    /// ones, under a heading when there are several or when it replaces
    /// every match; a write's content as added lines. Every line of the
    /// text, split at its newlines, an empty last one included — `"a\n"` is
    /// two lines, the second empty — so that two texts drawn alike are the
    /// same text, give or take what [`visible`] writes out. An empty text is
    /// no lines.
    pub fn change(&self) -> Vec<Changed> {
        let mut lines = Vec::new();
        let mut push = |mark: Mark, text: &str| {
            if text.is_empty() {
                return;
            }
            lines.extend(visible(text).split('\n').map(|line| Changed {
                mark,
                text: line.to_owned(),
            }));
        };
        match self {
            Self::Command { .. } => {}
            Self::Edit { edits, .. } => {
                let several = edits.len() > 1;
                for (index, edit) in edits.iter().enumerate() {
                    let heading = match (several, edit.replace_all) {
                        (true, false) => Some(format!("Edit {} of {}", index + 1, edits.len())),
                        (true, true) => Some(format!(
                            "Edit {} of {} · every match",
                            index + 1,
                            edits.len()
                        )),
                        (false, true) => Some("Every match".to_owned()),
                        (false, false) => None,
                    };
                    if let Some(heading) = heading {
                        push(Mark::Heading, &heading);
                    }
                    push(Mark::Removed, &edit.old);
                    push(Mark::Added, &edit.new);
                }
            }
            Self::Write { content, .. } => push(Mark::Added, content),
        }
        lines
    }

    /// How many lines [`Self::change`] would be, counted without building
    /// them.
    fn change_lines(&self) -> usize {
        let lines = |text: &str| match text {
            "" => 0,
            text => text.matches('\n').count() + 1,
        };
        match self {
            Self::Command { .. } => 0,
            Self::Edit { edits, .. } => {
                let several = edits.len() > 1;
                edits
                    .iter()
                    .map(|edit| {
                        usize::from(several || edit.replace_all)
                            + lines(&edit.old)
                            + lines(&edit.new)
                    })
                    .sum()
            }
            Self::Write { content, .. } => lines(content),
        }
    }

    /// Why this request cannot be shown whole, or `None` when it can.
    ///
    /// The rule both ends hold a request to: [`from_claude_bash`] reads
    /// nothing that breaks it, and a window refuses a request that does.
    pub fn fault(&self) -> Option<&'static str> {
        match self {
            Self::Command { command, note, .. } => {
                if command.is_empty() {
                    return Some("the command is empty");
                }
                if command.len() > MAX_COMMAND {
                    return Some("the command is longer than 48 KiB");
                }
                if command.contains('\0') {
                    return Some("the command holds a NUL, which no shell runs as written");
                }
                if note.as_ref().is_some_and(|note| note.len() > MAX_NOTE) {
                    return Some("the note is longer than 1 KiB");
                }
                None
            }
            Self::Edit { path, edits } => {
                if let Some(fault) = path_fault(path) {
                    return Some(fault);
                }
                if edits.is_empty() {
                    return Some("there are no edits");
                }
                if edits
                    .iter()
                    .any(|edit| edit.old.contains('\0') || edit.new.contains('\0'))
                {
                    return Some("an edit holds a NUL");
                }
                self.change_fault()
            }
            Self::Write { path, content } => {
                if let Some(fault) = path_fault(path) {
                    return Some(fault);
                }
                if content.contains('\0') {
                    return Some("the content holds a NUL");
                }
                self.change_fault()
            }
        }
    }

    /// Why a change to a file is too long to draw whole, or `None`.
    fn change_fault(&self) -> Option<&'static str> {
        (self.change_lines() > MAX_CHANGE_LINES)
            .then_some("the change is longer than the 3000 lines a diff is drawn to")
    }
}

/// Why `path` is not one a card can name a file by, or `None` when it is:
/// absolute, at most [`MAX_PATH`] characters written out, and with no
/// control character in it — a newline or a carriage return in a path is a
/// line a card would draw as two, or rewrite, and a file named so is rare
/// enough that its agent's own prompt can ask about it.
fn path_fault(path: &str) -> Option<&'static str> {
    if !path.starts_with('/') {
        return Some("the path is not absolute");
    }
    // Counted short of the whole when it is far too long, since every
    // character is at least one written out.
    if path.len() > MAX_PATH * 4 || visible(path).chars().count() > MAX_PATH {
        return Some("the path is longer than 256 characters");
    }
    if path.chars().any(char::is_control) {
        return Some("the path holds a control character");
    }
    None
}

/// Whether `path` is inside `directory`, by their text alone: both absolute,
/// and `path` below `directory` once `.` and empty components are dropped.
///
/// No file is looked at — this is asked on the window's thread, where a
/// stat of a path on a hung network mount would hang the window — so a
/// symbolic link inside the directory that leads out of it is not seen. A
/// `..` anywhere in `path` counts as outside: whether it climbs out depends
/// on what links lie under it, which only a look at the disk could say.
pub fn lexically_inside(path: &str, directory: &str) -> bool {
    let parts = |text: &str| -> Vec<String> {
        text.split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .map(str::to_owned)
            .collect()
    };
    if !path.starts_with('/') || !directory.starts_with('/') {
        return false;
    }
    let (path, directory) = (parts(path), parts(directory));
    if path.iter().chain(&directory).any(|part| part == "..") {
        return false;
    }
    path.len() > directory.len() && path.starts_with(&directory)
}

/// `path` relative to `directory`, when it is [`lexically_inside`] it.
pub fn relative_to(path: &str, directory: &str) -> Option<String> {
    if !lexically_inside(path, directory) {
        return None;
    }
    let skip = directory
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .count();
    Some(
        path.split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .skip(skip)
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Never the command: a request is printed in logs and panics, and what an
/// agent runs stays between the agent, its hook and the card. What kind it
/// is, how long, and its fingerprint are enough to tell two apart.
impl fmt::Debug for AskRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command {
                command,
                note,
                timeout_ms,
                background,
                outside_sandbox,
            } => formatter
                .debug_struct("Command")
                .field("bytes", &command.len())
                .field("noted", &note.is_some())
                .field("timeout_ms", timeout_ms)
                .field("background", background)
                .field("outside_sandbox", outside_sandbox)
                .finish(),
            // Never the path either: a file's name says what is being worked
            // on as plainly as its content does.
            Self::Edit { path, edits } => formatter
                .debug_struct("Edit")
                .field("path_bytes", &path.len())
                .field("edits", &edits.len())
                .field(
                    "bytes",
                    &edits
                        .iter()
                        .map(|edit| edit.old.len() + edit.new.len())
                        .sum::<usize>(),
                )
                .finish(),
            Self::Write { path, content } => formatter
                .debug_struct("Write")
                .field("path_bytes", &path.len())
                .field("bytes", &content.len())
                .finish(),
        }
    }
}

/// Claude Code's input for `tool` as a request, or `None` when the tool is
/// not one Crook asks about or its input holds anything Crook would not show.
///
/// `Bash`, `Edit`, `MultiEdit` and `Write`, each read by its own allowlist;
/// see [`from_claude_bash`] and the three beside it. `NotebookEdit`, web
/// fetches and MCP tools are not asked about at all.
pub fn from_claude(tool: &str, tool_input: &Value) -> Option<AskRequest> {
    match tool {
        "Bash" => from_claude_bash(tool_input),
        "Edit" => from_claude_edit(tool_input),
        "MultiEdit" => from_claude_multi_edit(tool_input),
        "Write" => from_claude_write(tool_input),
        _ => None,
    }
}

/// Claude Code's `Edit` input: `file_path`, `old_string` and `new_string`,
/// strings, and `replace_all`, a boolean, which Claude Code 2.1.280 sends and
/// is taken as off when it is left out — what Claude Code does without it.
fn from_claude_edit(tool_input: &Value) -> Option<AskRequest> {
    let input = tool_input.as_object()?;
    let mut path = None;
    let edit = one_edit(input, &mut path)?;
    checked(AskRequest::Edit {
        path: path?,
        edits: vec![edit],
    })
}

/// Claude Code's `MultiEdit` input: `file_path`, a string, and `edits`, a
/// list of objects each with `old_string` and `new_string`, strings, and
/// `replace_all`, a boolean, when they say. 2.1.280 does not offer the model
/// this tool; an older or newer one that does is read by the same rules.
fn from_claude_multi_edit(tool_input: &Value) -> Option<AskRequest> {
    let input = tool_input.as_object()?;
    let mut path = None;
    let mut edits = None;
    for (key, value) in input {
        match key.as_str() {
            "file_path" => path = Some(value.as_str()?.to_owned()),
            "edits" => {
                edits = Some(
                    value
                        .as_array()?
                        .iter()
                        .map(|edit| {
                            let mut none = None;
                            one_edit(edit.as_object()?, &mut none).filter(|_| none.is_none())
                        })
                        .collect::<Option<Vec<_>>>()?,
                )
            }
            _ => return None,
        }
    }
    checked(AskRequest::Edit {
        path: path?,
        edits: edits?,
    })
}

/// One edit's keys out of `input`, and `file_path` into `path` when it is
/// there: `None` for any other key or a type it does not take.
fn one_edit(input: &serde_json::Map<String, Value>, path: &mut Option<String>) -> Option<FileEdit> {
    let mut old = None;
    let mut new = None;
    let mut replace_all = false;
    for (key, value) in input {
        match key.as_str() {
            "file_path" => *path = Some(value.as_str()?.to_owned()),
            "old_string" => old = Some(value.as_str()?.to_owned()),
            "new_string" => new = Some(value.as_str()?.to_owned()),
            "replace_all" => replace_all = value.as_bool()?,
            _ => return None,
        }
    }
    Some(FileEdit {
        old: old?,
        new: new?,
        replace_all,
    })
}

/// Claude Code's `Write` input: `file_path` and `content`, strings, both
/// required.
fn from_claude_write(tool_input: &Value) -> Option<AskRequest> {
    let input = tool_input.as_object()?;
    let mut path = None;
    let mut content = None;
    for (key, value) in input {
        match key.as_str() {
            "file_path" => path = Some(value.as_str()?.to_owned()),
            "content" => content = Some(value.as_str()?.to_owned()),
            _ => return None,
        }
    }
    checked(AskRequest::Write {
        path: path?,
        content: content?,
    })
}

/// `request`, when [`AskRequest::fault`] finds nothing wrong with it.
fn checked(request: AskRequest) -> Option<AskRequest> {
    request.fault().is_none().then_some(request)
}

/// Claude Code's `Bash` tool input as a request, or `None` when it holds
/// anything Crook would not show.
///
/// The keys Claude Code 2.1 sends, each with its type: `command`, a string
/// and the only one required; `description`, a string; `timeout`, a whole
/// number of milliseconds; `run_in_background` and `dangerouslyDisableSandbox`,
/// booleans. Anything else — a key not in that list, a `null`, a number where
/// a string goes — is no request, and neither is one [`AskRequest::fault`]
/// refuses.
pub fn from_claude_bash(tool_input: &Value) -> Option<AskRequest> {
    let input = tool_input.as_object()?;
    let mut command = None;
    let mut note = None;
    let mut timeout_ms = None;
    let mut background = false;
    let mut outside_sandbox = false;
    for (key, value) in input {
        match key.as_str() {
            "command" => command = Some(value.as_str()?.to_owned()),
            "description" => note = Some(value.as_str()?.to_owned()),
            "timeout" => timeout_ms = Some(value.as_u64()?),
            "run_in_background" => background = value.as_bool()?,
            "dangerouslyDisableSandbox" => outside_sandbox = value.as_bool()?,
            _ => return None,
        }
    }
    let request = AskRequest::Command {
        command: command?,
        note,
        timeout_ms,
        background,
        outside_sandbox,
    };
    checked(request)
}

/// `value` as JSON with every object's keys in sorted order, all the way
/// down, and no white space.
///
/// Sorted here, through a `BTreeMap`, rather than left to `serde_json`'s
/// map: whether that keeps keys sorted or in the order they came depends on
/// its `preserve_order` feature, which any crate in the build can turn on
/// for everyone, and a fingerprint that changed with the dependency tree
/// would match nothing the other end computed.
pub fn canonical(value: &Value) -> String {
    let mut written = String::new();
    write_canonical(value, &mut written);
    written
}

fn write_canonical(value: &Value, into: &mut String) {
    match value {
        Value::Object(fields) => write_object(fields, into),
        Value::Array(items) => {
            into.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    into.push(',');
                }
                write_canonical(item, into);
            }
            into.push(']');
        }
        scalar => into.push_str(&scalar.to_string()),
    }
}

/// An object's `fields`, in whatever order they come, written with their
/// keys sorted.
///
/// Taking the fields rather than a map is what lets a test hand it fields
/// out of order, which no `serde_json` map built without `preserve_order`
/// can hold: the sort is the thing that has to work the day the map stops
/// doing it.
fn write_object<'a>(fields: impl IntoIterator<Item = (&'a String, &'a Value)>, into: &mut String) {
    let sorted: BTreeMap<&String, &Value> = fields.into_iter().collect();
    into.push('{');
    for (index, (key, field)) in sorted.into_iter().enumerate() {
        if index > 0 {
            into.push(',');
        }
        into.push_str(&Value::String(key.clone()).to_string());
        into.push(':');
        write_canonical(field, into);
    }
    into.push('}');
}

/// SHA-256 over [`DOMAIN`], `session`, `tool` and the request's
/// [`canonical`] JSON, each after a NUL but the first.
///
/// What a press on the card carries and the window checks before it answers,
/// so that an answer meant for one request can never reach another that took
/// its place.
pub fn digest(session: &str, tool: &str, request: &AskRequest) -> [u8; 32] {
    let encoded = serde_json::to_value(request)
        .expect("a request is strings, numbers, booleans and lists of them, which encode");
    let mut hash = Sha256::new();
    for (index, part) in [DOMAIN, session, tool, &canonical(&encoded)]
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            hash.update([0u8]);
        }
        hash.update(part.as_bytes());
    }
    hash.finalize().into()
}

/// The first half of a [`digest`], as 32 lowercase hex digits: what a hook
/// that saw the tool run says it saw, without saying what the tool was.
pub fn fingerprint(digest: &[u8; 32]) -> String {
    hex(&digest[..16])
}

/// `bytes` as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `text` as a person should see it before allowing it to run.
///
/// Newlines stay, and a tab becomes the spaces to the next stop, so a script
/// reads as the lines it is. Everything else that would not print as itself
/// is written out where it is: a carriage return — which on a terminal
/// rewrites the line it ends — as `␍`, and every other C0 control, DEL, C1
/// control, bidirectional control, zero-width, format or other
/// default-ignorable character, and every space but U+0020, as `⟨U+XXXX⟩`. A command that hid `rm -rf ~` behind a carriage return, or
/// reordered itself with a right-to-left override, shows the trick instead
/// of doing it.
pub fn visible(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut column = 0;
    for character in text.chars() {
        match character {
            '\n' => {
                shown.push('\n');
                column = 0;
            }
            '\t' => {
                let spaces = TAB_STOP - column % TAB_STOP;
                shown.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            }
            '\r' => {
                shown.push('␍');
                column += 1;
            }
            character if hidden(character) => {
                let mark = format!("⟨U+{:04X}⟩", u32::from(character));
                column += mark.chars().count();
                shown.push_str(&mark);
            }
            character => {
                shown.push(character);
                column += unicode_width::UnicodeWidthChar::width(character).unwrap_or(0);
            }
        }
    }
    shown
}

/// Whether `character` would not print as itself: a control character, a
/// bidirectional control, or a zero-width or format character — Unicode's
/// general category `Cf`, which every one of those is but the controls —
/// any other character Unicode says a renderer may draw as nothing, the two
/// separators a renderer may break a line at, and every space but the
/// plain one.
fn hidden(character: char) -> bool {
    character.is_control()
        || format(character)
        || ignorable(character)
        || spacing(character)
        || matches!(character, '\u{2028}' | '\u{2029}')
}

/// Whether `character` is `Default_Ignorable_Code_Point` and outside `Cf`,
/// as of Unicode 16: a combining grapheme joiner, the Hangul fillers, the
/// Khmer inherent vowels, the Mongolian and the other variation selectors,
/// and the reserved runs, all of which a font may draw as nothing at all.
/// `rm ./build` with a variation selector after it is another file than
/// the one it reads as.
fn ignorable(character: char) -> bool {
    matches!(
        u32::from(character),
        0x034F
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x180B..=0x180F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFFA0
            | 0xFFF0..=0xFFF8
            | 0xE0080..=0xE0FFF
    )
}

/// Whether `character` is a space that is not U+0020: a no-break space, the
/// sized spaces of U+2000 to U+200A, and the narrow, mathematical and
/// ideographic ones. Each looks like the space a shell splits words at, and
/// none of them is one, so `rm\u{a0}-rf` is one word that reads as two.
fn spacing(character: char) -> bool {
    matches!(
        u32::from(character),
        0x00A0 | 0x1680 | 0x2000..=0x200A | 0x202F | 0x205F | 0x3000
    )
}

/// Whether `character` is in Unicode's general category `Cf`, format
/// characters, as of Unicode 16, with the unassigned points inside its runs
/// counted in: a point assigned later inside a run of invisibles is another
/// invisible far more often than not, and a mark where none was needed costs
/// nothing.
fn format(character: char) -> bool {
    matches!(
        u32::from(character),
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE007F
    )
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
