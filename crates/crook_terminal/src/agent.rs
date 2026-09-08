//! The channel a program reports its own state on.
//!
//! A shell says where a command starts and ends with OSC 133, and that is all
//! a shell can say: from where it stands an agent is one command that has not
//! finished yet. Whether that agent is working, waiting for an answer or has
//! given up is something only the agent knows, and this is the sequence it
//! says it with — `OSC 6340 ; <status> [; <title>] BEL`, written to its own
//! terminal.
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
//! What the sequence carries is a word and, optionally, a name. The word is
//! one of four and the name is what the agent calls the work it is doing.
//! Nothing else: not a message, not a number, not a colour. A plugin can put
//! a picture on a status; the status itself is a fact about the work.

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
/// escaping scheme is a second parser on both ends of the wire.
pub fn report(status: AgentReport, title: Option<&str>) -> String {
    let mut sequence = format!("\x1b]{OSC};{}", status.word());
    if let Some(title) = title.filter(|title| !title.chars().any(char::is_control)) {
        sequence.push(';');
        sequence.push_str(title);
    }
    sequence.push('\x07');
    sequence
}

/// One report, as it arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reported {
    /// What the program said it was doing.
    pub status: AgentReport,
    /// What it called its work, when it said.
    pub title: Option<String>,
}

/// Reads a report out of the parameters of an OSC sequence, or returns `None`
/// when they are not one this crate reads.
///
/// `parameters` is what `vte` split on `;`, so a title with a semicolon in it
/// arrives in pieces and is put back together here: the sequence has exactly
/// two fields before the title, and everything after them is the title.
pub(crate) fn parse(parameters: &[&[u8]]) -> Option<Reported> {
    let [number, word, rest @ ..] = parameters else {
        return None;
    };
    if *number != OSC.as_bytes() {
        return None;
    }
    let status = AgentReport::parse(str::from_utf8(word).ok()?)?;
    let title = rest
        .iter()
        .map(|piece| String::from_utf8_lossy(piece))
        .collect::<Vec<_>>()
        .join(";");
    let title = title.trim();
    Some(Reported {
        status,
        title: (!title.is_empty()).then(|| title.to_owned()),
    })
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
                    title: None
                }),
                parsed(&report(status, None))
            );
        }
    }

    #[test]
    fn a_title_travels_with_the_status_and_keeps_its_semicolons() {
        let sequence = report(AgentReport::Running, Some("fix a; then b"));
        assert_eq!(
            Some(Reported {
                status: AgentReport::Running,
                title: Some("fix a; then b".to_owned()),
            }),
            parsed(&sequence)
        );
    }

    #[test]
    fn a_title_that_would_break_the_sequence_is_left_off() {
        assert_eq!(
            format!("\x1b]{OSC};failed\x07"),
            report(AgentReport::Failed, Some("one\x07two"))
        );
        assert_eq!(
            Some(Reported {
                status: AgentReport::NeedsInput,
                title: None,
            }),
            parsed(&report(AgentReport::NeedsInput, Some("   ")))
        );
    }

    #[test]
    fn a_word_that_is_not_a_status_is_not_a_report() {
        assert_eq!(None, parsed("\x1b]6340;sleeping\x07"));
        assert_eq!(None, parsed("\x1b]6341;running\x07"));
        assert_eq!(None, parse(&[b"6340"]));
    }
}
