//! The channel a program reports its own state on.
//!
//! A shell says where a command starts and ends with OSC 133, and that is all
//! a shell can say: from where it stands an agent is one command that has not
//! finished yet. Whether that agent is working, waiting for an answer or has
//! given up is something only the agent knows, and this is the sequence it
//! says it with — `OSC 6340 ; <status> [; <title> [;; <message>]] BEL`,
//! written to its own terminal.
//!
//! The terminal rather than a socket, because the terminal is the one thing
//! the program already has. It needs no address, no file to find and no
//! protocol to speak; it crosses `ssh`, a container and a `docker exec`
//! unchanged, which a socket on this machine never would; and it lands on the
//! pane the program is in without anybody having to say which pane that was.
//! Every other terminal drops an OSC it does not know, so a program that
//! reports this way costs nothing anywhere else.
//!
//! The number is Crook's own, beside the completion channel's 6339 and for
//! the same reason: far from anything standardised, so a stream carrying one
//! was written by something that meant it for Crook.
//!
//! What the sequence carries is a word, optionally a name, and optionally
//! what the agent is waiting for. The word is one of four and the name is
//! what the agent calls the work it is doing. The message is the one thing
//! an agent that has stopped to ask can add that the word cannot say — *what*
//! it is asking, "run `rm -rf build`?" — and the tab keeps it only beside
//! `needs-input`, since it is the answer to "waiting for what?" and nothing
//! else is waiting. Nothing more than those: not a number, not a colour. A
//! plugin can put a picture on a status; the status itself is a fact about
//! the work.
//!
//! # How the fields are cut
//!
//! `vte` splits an OSC on every `;`, and the title has always been allowed
//! to hold one — the sequence has exactly two fields before it, and
//! everything after them is put back together. So the message cannot be a
//! third field: `fix a; then b` already is a third and a fourth. What ends
//! the title instead is an *empty* field — `;;` — and everything after that
//! is the message, put back together the same way. A title with a `;` in it
//! still travels whole, a sequence with no message reads exactly as it did,
//! and the writer squeezes a `;;` out of either text so that neither can
//! forge the cut. A Crook older than the message reads the whole tail as the
//! title, `port the tab bar;;run rm -rf build?` — wrong on the row, and
//! nothing worse than wrong, which is what makes the field safe to send to
//! a pane whose Crook is not known.

use std::str;

/// The OSC number the report arrives on.
pub const OSC: &str = "6340";

/// What a program said it was doing.
///
/// The four states a tab of agents wants at a glance, said in words so the
/// same sequence means the same thing in a theme written years from now.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AgentReport {
    /// Waiting for a prompt. What every pane is until something says
    /// otherwise, and what the terminal itself goes back to when the command
    /// the report came from ends.
    #[default]
    Idle,
    /// Working, with no attention needed.
    Running,
    /// Stopped, waiting on a person — an approval or an answer.
    NeedsInput,
    /// Stopped because something went wrong.
    Failed,
}

impl AgentReport {
    /// Every status, in the order the words are documented.
    pub const ALL: [Self; 4] = [Self::Idle, Self::Running, Self::NeedsInput, Self::Failed];

    /// The word for this status on the wire and on a command line.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::NeedsInput => "needs-input",
            Self::Failed => "failed",
        }
    }

    /// The status a word names, or `None` for a word that is not one.
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.word() == word)
    }
}

/// The bytes a program writes to its terminal to report `status`.
///
/// The title is dropped rather than escaped when it holds a byte that would
/// end the sequence early or be read as a parameter of its own: a program
/// with a control character in its title has no title worth keeping, and an
/// escaping scheme is a second parser on both ends of the wire. The message
/// is dropped by the same rule, and both lose a `;;`, because that is the
/// cut between them — see the module docs. A message with no title is the
/// cut straight after the status, `;;message`: the empty piece the reader
/// looks for is the first one, and there is no title in front of it.
pub fn report(status: AgentReport, title: Option<&str>, message: Option<&str>) -> String {
    let mut sequence = format!("\x1b]{OSC};{}", status.word());
    // And a title loses a `;` at either end, or is left off when that leaves
    // nothing: beside the `;` in front of it, or the `;;` behind it, an edge
    // semicolon is an empty piece of its own, and the reader cuts at the
    // first one — `running;;title` is a message with no title.
    let title = title
        .and_then(field)
        .map(|title| title.trim_matches(';').to_owned())
        .filter(|title| !title.is_empty());
    if let Some(title) = title {
        sequence.push(';');
        sequence.push_str(&title);
    }
    if let Some(message) = message.and_then(field) {
        sequence.push_str(";;");
        sequence.push_str(&message);
    }
    sequence.push('\x07');
    sequence
}

/// `text` as one field of the sequence, or `None` when it cannot be one.
fn field(text: &str) -> Option<String> {
    if text.chars().any(char::is_control) {
        return None;
    }
    let mut squeezed = text.to_owned();
    while squeezed.contains(";;") {
        squeezed = squeezed.replace(";;", ";");
    }
    Some(squeezed)
}

/// One report, as it arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reported {
    /// What the program said it was doing.
    pub status: AgentReport,
    /// What it called its work, when it said.
    pub title: Option<String>,
    /// What it is waiting for, when it said.
    pub message: Option<String>,
}

/// Reads a report out of the parameters of an OSC sequence, or returns `None`
/// when they are not one this crate reads.
///
/// `parameters` is what `vte` split on `;`, so a title with a semicolon in it
/// arrives in pieces and is put back together here: the sequence has exactly
/// two fields before the title, and everything after them is the title up to
/// the first empty piece, which is the `;;` that starts the message.
pub(crate) fn parse(parameters: &[&[u8]]) -> Option<Reported> {
    let [number, word, rest @ ..] = parameters else {
        return None;
    };
    if *number != OSC.as_bytes() {
        return None;
    }
    let status = AgentReport::parse(str::from_utf8(word).ok()?)?;
    let (title, message) = match rest.iter().position(|piece| piece.is_empty()) {
        Some(cut) => (&rest[..cut], &rest[cut + 1..]),
        None => (rest, &rest[rest.len()..]),
    };
    Some(Reported {
        status,
        title: joined(title),
        message: joined(message),
    })
}

/// `pieces` as the one text `vte` cut them out of, or `None` for no text.
fn joined(pieces: &[&[u8]]) -> Option<String> {
    let text = pieces
        .iter()
        .map(|piece| String::from_utf8_lossy(piece))
        .collect::<Vec<_>>()
        .join(";");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `vte` hands `osc_dispatch` for `sequence`, with the terminator
    /// taken off.
    fn split(sequence: &str) -> Vec<Vec<u8>> {
        let body = sequence
            .strip_prefix("\x1b]")
            .and_then(|rest| rest.strip_suffix('\x07'))
            .expect("a report is a BEL-terminated OSC");
        body.split(';')
            .map(|piece| piece.as_bytes().to_vec())
            .collect()
    }

    fn parsed(sequence: &str) -> Option<Reported> {
        let pieces = split(sequence);
        let parameters: Vec<&[u8]> = pieces.iter().map(Vec::as_slice).collect();
        parse(&parameters)
    }

    #[test]
    fn every_word_round_trips_through_the_wire() {
        for status in AgentReport::ALL {
            assert_eq!(Some(status), AgentReport::parse(status.word()));
            assert_eq!(
                Some(Reported {
                    status,
                    title: None,
                    message: None,
                }),
                parsed(&report(status, None, None))
            );
        }
    }

    #[test]
    fn a_title_travels_with_the_status_and_keeps_its_semicolons() {
        let sequence = report(AgentReport::Running, Some("fix a; then b"), None);
        assert_eq!(
            Some(Reported {
                status: AgentReport::Running,
                title: Some("fix a; then b".to_owned()),
                message: None,
            }),
            parsed(&sequence)
        );
    }

    #[test]
    fn a_message_travels_after_the_title_and_both_keep_their_semicolons() {
        let sequence = report(
            AgentReport::NeedsInput,
            Some("fix a; then b"),
            Some("run `rm -rf build`; then `make`?"),
        );
        assert_eq!(
            format!("\x1b]{OSC};needs-input;fix a; then b;;run `rm -rf build`; then `make`?\x07"),
            sequence
        );
        assert_eq!(
            Some(Reported {
                status: AgentReport::NeedsInput,
                title: Some("fix a; then b".to_owned()),
                message: Some("run `rm -rf build`; then `make`?".to_owned()),
            }),
            parsed(&sequence)
        );
    }

    #[test]
    fn a_message_with_no_title_is_the_cut_straight_after_the_status() {
        // The cut has to be there for the message to be read as one, and
        // nothing before it is what an absent title looks like on the wire.
        let sequence = report(AgentReport::NeedsInput, None, Some("approve?"));
        assert_eq!(format!("\x1b]{OSC};needs-input;;approve?\x07"), sequence);
        assert_eq!(
            Some(Reported {
                status: AgentReport::NeedsInput,
                title: None,
                message: Some("approve?".to_owned()),
            }),
            parsed(&sequence)
        );
    }

    #[test]
    fn a_double_semicolon_in_either_text_cannot_forge_the_cut() {
        // Squeezed to one on the way out, so `a;;b` in a title is `a;b` on
        // the row rather than a title `a` waiting for `b`.
        let sequence = report(AgentReport::Running, Some("a;;;b"), Some("c;;d"));
        assert_eq!(
            Some(Reported {
                status: AgentReport::Running,
                title: Some("a;b".to_owned()),
                message: Some("c;d".to_owned()),
            }),
            parsed(&sequence)
        );
    }

    #[test]
    fn a_semicolon_at_the_edge_of_a_title_cannot_forge_the_cut_either() {
        // Squeezing is not enough at the ends: `;x` beside the `;` in front
        // of it is `;;x`, which read as a message with no title.
        let read = |title: &str, message: Option<&str>| {
            parsed(&report(AgentReport::NeedsInput, Some(title), message))
                .map(|reported| (reported.title, reported.message))
        };
        let text = |text: &str| Some(text.to_owned());

        assert_eq!(read(";; comment", None), Some((text("comment"), None)));
        assert_eq!(read("; x", None), Some((text("x"), None)));
        assert_eq!(
            read("fix a;", Some("approve?")),
            Some((text("fix a"), text("approve?")))
        );
        // A title that is nothing but the edge is no title, and the message
        // after it arrives whole rather than with a `;` in front.
        assert_eq!(read(";", Some("approve?")), Some((None, text("approve?"))));
        assert_eq!(read("", Some("approve?")), Some((None, text("approve?"))));
    }

    #[test]
    fn a_title_that_would_break_the_sequence_is_left_off() {
        assert_eq!(
            format!("\x1b]{OSC};failed\x07"),
            report(AgentReport::Failed, Some("one\x07two"), None)
        );
        assert_eq!(
            format!("\x1b]{OSC};failed\x07"),
            report(AgentReport::Failed, None, Some("one\x07two"))
        );
        assert_eq!(
            Some(Reported {
                status: AgentReport::NeedsInput,
                title: None,
                message: None,
            }),
            parsed(&report(AgentReport::NeedsInput, Some("   "), Some(" ")))
        );
    }

    #[test]
    fn a_word_that_is_not_a_status_is_not_a_report() {
        assert_eq!(None, parsed("\x1b]6340;sleeping\x07"));
        assert_eq!(None, parsed("\x1b]6341;running\x07"));
        assert_eq!(None, parse(&[b"6340"]));
    }
}
