//! The notifications other terminals already read.
//!
//! Crook's own channel is OSC 6340 — see [`crate::agent`] — and only a
//! program that has been told about Crook speaks it. Three older sequences
//! say "look here, and this is why" to other terminals, and the programs a
//! person runs in a pane speak them already: Claude Code writes one when it
//! stops to ask, in whichever form its notification channel names; Codex
//! does when it is set to notify; a script ends a long build with
//! `printf '\e]9;done\a'`. They travel wherever the pane's bytes do — back
//! out of `ssh`, out of a container — so reading them is what lets an agent
//! on a machine with no Crook binary on it ask for a look.
//!
//! * **OSC 9**, iTerm2's: `9 ; <message>`. ConEmu took the same number for a
//!   family of commands, `9 ; <n> ; …` with `n` from 1 to 12, and its
//!   progress report, `9 ; 4`, is one other programs write too — winget
//!   among them. None of those is a notification, so a message whose first
//!   field is one of those numbers is read as ConEmu's and dropped. A
//!   notification that says only "4" is the price.
//! * **OSC 777**, rxvt-unicode's: `777 ; notify ; <title> ; <body>`, which
//!   Ghostty reads. `777` is rxvt's number for all its extensions, and
//!   `notify` is the only one read here.
//! * **OSC 99**, kitty's: `99 ; <metadata> ; <payload>`, the metadata a
//!   `:`-separated list of `key=value`. The payload is the title unless
//!   `p=body` says otherwise, and `d=0` says more is coming under the same
//!   `i=` id — Claude Code sends its title that way and its body in the
//!   chunk that finishes it — so an unfinished notification is kept until
//!   its last chunk. A payload in base64, `e=1`, is skipped rather than
//!   decoded or shown as it arrived; the kinds that carry no text — an
//!   icon, buttons — are part of a notification and add nothing to it; and
//!   the kinds that are not part of one at all — closing one, asking
//!   whether one is alive, asking what the terminal supports — are ignored
//!   and leave an unfinished one alone.
//!
//! What comes out is a [`Notification`]: a title, a body, or both. It is
//! not a status. A notification says nothing about whether the program is
//! working or waiting, and what it asks for is a look.
//!
//! # What reaches the row
//!
//! `vte` splits an OSC on every `;`, so a message holding one arrives in
//! pieces and is put back together here — everything after the fields
//! before it is the message. It also keeps only sixteen of those pieces and
//! drops the rest without a word, so a sequence that arrives with all
//! sixteen may have lost its end; it is marked `…` rather than passed off as
//! whole, which is the same answer `agent::report` gives from the writing
//! side. A control character becomes a space, since a C1 `CSI` in a message
//! is an escape sequence waiting for something to print it, and the text is
//! cut to one line of a row's length.

use std::str;

use crate::agent::MAX_PIECES;

/// The longest title a notification keeps, in characters.
///
/// A title names what is asking — "Claude Code", "build" — and this is the
/// length the application cuts an agent's own name for its work to.
pub(crate) const TITLE_CHARS: usize = 60;

/// The longest body a notification keeps, in characters.
///
/// The length the application cuts an agent's `needs-input` message to,
/// since this lands on the same line of the same row: one line, long enough
/// that a permission prompt's text fits with room to spare.
pub(crate) const BODY_CHARS: usize = 200;

/// How much of each text an unfinished kitty notification keeps while the
/// rest of it arrives, in bytes.
///
/// kitty's own limit on one chunk's payload, so a chunk always fits; a
/// program that sends unfinished chunks forever holds this much and no more.
const DRAFT_BYTES: usize = 2048;

/// ConEmu's commands on OSC 9, by number.
const CONEMU: std::ops::RangeInclusive<u8> = 1..=12;

/// A program asking for a look, with what it said.
///
/// At least one of the two is there: a notification with neither says
/// nothing, and is not reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// What is asking, when the sequence has a title of its own.
    pub title: Option<String>,
    /// What it said. OSC 9 has only this.
    pub body: Option<String>,
}

impl Notification {
    /// A notification out of what was read, or `None` when nothing was.
    fn new(title: Option<String>, body: Option<String>) -> Option<Self> {
        (title.is_some() || body.is_some()).then_some(Self { title, body })
    }
}

/// Reads notifications out of OSC sequences.
///
/// A value rather than a function because of kitty's chunks: a notification
/// sent in several sequences is one notification, and what came before its
/// last chunk has to be kept somewhere until it arrives.
#[derive(Debug, Default)]
pub(crate) struct Reader {
    draft: Option<Draft>,
}

/// A kitty notification whose last chunk has not arrived.
#[derive(Debug, Default)]
struct Draft {
    id: Option<Vec<u8>>,
    title: String,
    body: String,
}

impl Reader {
    /// Reads a notification out of the parameters of an OSC sequence, or
    /// returns `None` when they are not one — or not the last chunk of one.
    pub(crate) fn read(&mut self, parameters: &[&[u8]]) -> Option<Notification> {
        let cut = parameters.len() >= MAX_PIECES;
        match parameters {
            [b"9", message @ ..] => iterm(message, cut),
            [b"777", b"notify", title, body @ ..] => Notification::new(
                presentable(&String::from_utf8_lossy(title), TITLE_CHARS),
                presentable(&joined(body, cut), BODY_CHARS),
            ),
            [b"99", metadata, payload @ ..] => self.kitty(metadata, payload, cut),
            _ => None,
        }
    }

    /// One chunk of a kitty notification: kept when more is coming, and the
    /// whole notification when it is the last.
    fn kitty(&mut self, metadata: &[u8], payload: &[&[u8]], cut: bool) -> Option<Notification> {
        let chunk = Chunk::parse(metadata)?;
        // One notification is put together at a time. A chunk of another one
        // drops what was unfinished rather than mixing the two.
        let mut draft = match self.draft.take() {
            Some(draft) if draft.id.as_deref() == chunk.id => draft,
            _ => Draft {
                id: chunk.id.map(<[u8]>::to_vec),
                ..Draft::default()
            },
        };
        if !chunk.encoded {
            let text = joined(payload, cut);
            match chunk.part {
                Part::Title => push_within(&mut draft.title, &text),
                Part::Body => push_within(&mut draft.body, &text),
                Part::Textless => {}
            }
        }
        if !chunk.done {
            self.draft = Some(draft);
            return None;
        }
        Notification::new(
            presentable(&draft.title, TITLE_CHARS),
            presentable(&draft.body, BODY_CHARS),
        )
    }
}

/// OSC 9's message, unless it is one of ConEmu's commands.
fn iterm(message: &[&[u8]], cut: bool) -> Option<Notification> {
    if message.first().is_some_and(|first| is_conemu(first)) {
        return None;
    }
    Notification::new(None, presentable(&joined(message, cut), BODY_CHARS))
}

/// Whether the first field of an OSC 9 is the number of one of ConEmu's
/// commands rather than the start of a message.
fn is_conemu(field: &[u8]) -> bool {
    str::from_utf8(field)
        .ok()
        .filter(|field| !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|field| field.parse::<u8>().ok())
        .is_some_and(|command| CONEMU.contains(&command))
}

/// What one kitty chunk's metadata says about it.
struct Chunk<'a> {
    /// The `i=` naming the notification, which is what ties chunks together.
    id: Option<&'a [u8]>,
    /// Whether this is the last chunk — `d`, which is `1` unless it is `0`.
    done: bool,
    part: Part,
    /// Whether the payload is base64, `e=1`.
    encoded: bool,
}

/// Which part of a notification a chunk carries.
enum Part {
    Title,
    Body,
    /// Part of the notification, with no text for a row: an icon, buttons.
    Textless,
}

impl<'a> Chunk<'a> {
    /// Reads the metadata, or returns `None` for a chunk that is not part
    /// of a notification's content at all.
    ///
    /// Unknown keys are skipped, as kitty's spec asks, and so is every key
    /// that does not change what the text is — urgency, the click action,
    /// when to show it.
    fn parse(metadata: &'a [u8]) -> Option<Self> {
        let mut chunk = Self {
            id: None,
            done: true,
            part: Part::Title,
            encoded: false,
        };
        for pair in metadata.split(|byte| *byte == b':') {
            let Some(at) = pair.iter().position(|byte| *byte == b'=') else {
                continue;
            };
            let (key, value) = (&pair[..at], &pair[at + 1..]);
            match key {
                b"i" => chunk.id = Some(value),
                b"d" => chunk.done = value != b"0",
                b"e" => chunk.encoded = value == b"1",
                b"p" => {
                    chunk.part = match value {
                        b"title" => Part::Title,
                        b"body" => Part::Body,
                        b"icon" | b"buttons" => Part::Textless,
                        // `close`, `alive`, `?`, and anything newer: not
                        // text a notification shows.
                        _ => return None,
                    }
                }
                _ => {}
            }
        }
        Some(chunk)
    }
}

/// `text` on the end of `field`, as far as [`DRAFT_BYTES`] allows.
fn push_within(field: &mut String, text: &str) {
    for character in text.chars() {
        if field.len() + character.len_utf8() > DRAFT_BYTES {
            return;
        }
        field.push(character);
    }
}

/// `pieces` as the one text `vte` cut them out of, marked where the parser
/// may have cut it short.
fn joined(pieces: &[&[u8]], cut: bool) -> String {
    let mut text = pieces
        .iter()
        .map(|piece| String::from_utf8_lossy(piece))
        .collect::<Vec<_>>()
        .join(";");
    if cut {
        text.push('…');
    }
    text
}

/// `text` as one line of at most `chars` characters, marked where it was
/// cut, or `None` when nothing of it is left.
///
/// A control character or a run of whitespace becomes one space — the rule
/// the application's own title and message go through — and the ends are
/// trimmed.
fn presentable(text: &str, chars: usize) -> Option<String> {
    let mut line = String::with_capacity(text.len());
    let mut space = true;
    for character in text.chars() {
        if character.is_control() || character.is_whitespace() {
            if !space {
                line.push(' ');
                space = true;
            }
        } else {
            line.push(character);
            space = false;
        }
    }
    let line = line.trim_end();
    if line.is_empty() {
        return None;
    }
    let mut cut: String = line.chars().take(chars).collect();
    if line.chars().count() > chars {
        cut.push('…');
    }
    Some(cut)
}
