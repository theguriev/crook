//! What an agent asks permission for, in the one shape Crook will show.
//!
//! An agent's permission hook is handed the tool it wants to run and that
//! tool's input, in the agent's own schema, which the agent is free to grow.
//! A question is asked of the window only when every key of that input is one
//! Crook knows how to show, with the type it expects: what a person is asked
//! to allow has to be the whole of what will run, and an input with a key
//! Crook has never heard of — a new flag that changes where or how a command
//! runs — is one it cannot promise that about. So [`from_claude_bash`] reads
//! Claude Code's `Bash` input through an allowlist into an [`AskRequest`] and
//! answers `None` for anything else, and the agent's own prompt is then the
//! only one, as it always was.
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
}

impl AskRequest {
    /// The word on the wire for what kind of request this is.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Command { .. } => "command",
        }
    }

    /// The command, for a request that is one.
    pub fn command(&self) -> &str {
        match self {
            Self::Command { command, .. } => command,
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
        }
    }
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
        }
    }
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
#[cfg(any(unix, test, doc))]
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
    request.fault().is_none().then_some(request)
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
        .expect("a request is strings, numbers and booleans, which encode");
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
#[cfg(any(test, doc))]
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
