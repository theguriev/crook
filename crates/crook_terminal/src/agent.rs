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
//!
//! # The pull request, on a channel of its own
//!
//! An agent that has opened a pull request can say which, and the row links
//! to it: `OSC 6342 ; pr ; <url> BEL`. Crook asks no forge for this — the
//! agent knows the address because it just made it, and the terminal is how
//! it says so, for every reason above.
//!
//! A sibling number rather than a third field on 6340, for three reasons.
//! The tail of 6340 is already cut twice, on the `;` `vte` splits at and on
//! the `;;` before a message, and a third cut would be a third rule a writer
//! had to get right. A Crook older than the field would read the address
//! into the title, where 6340's own tail has always been "wrong on the row
//! and nothing worse"; a number it does not know is a link that is not
//! there, which is better. And an address is not a status: it arrives once,
//! when the pull request is made, and a report on 6340 would have to carry a
//! word the agent did not mean to say again. `6341` is spoken for, by the
//! question an agent that stopped to ask is waiting on.
//!
//! An address holds `:` and `/` always and `;` almost never, and `vte`
//! splits on nothing but `;` — so the pieces after the word are joined back
//! the way a title's are. What cannot be undone is the parser's limit: it
//! keeps sixteen pieces and drops the rest without a word, and a cut address
//! is a link to somewhere else. So the writer refuses an address that would
//! fill the list, and the reader takes a list that is full as one that was
//! cut. [`pull_request_url`] is the whole of what either end accepts.

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
    let message = message.and_then(field);

    // What `vte` will keep of it. The number and the status are two pieces,
    // the cut before a message is one more, and a message needs at least one
    // of its own; everything past the limit is dropped by the parser without
    // a word, so it is cut here instead, where the cut can say so.
    let mut budget = MAX_PIECES - 2;
    if let Some(title) = title {
        let room = budget - if message.is_some() { 2 } else { 0 };
        let title = within(&title, room);
        budget -= pieces(&title);
        sequence.push(';');
        sequence.push_str(&title);
    }
    if let Some(message) = message {
        sequence.push_str(";;");
        sequence.push_str(&within(&message, budget - 1));
    }
    sequence.push('\x07');
    sequence
}

/// How many parameters of an OSC sequence `vte` keeps. Its own
/// `MAX_OSC_PARAMS`, which it does not export: every `;` past the fifteenth
/// starts a piece that is dropped on the floor.
pub(crate) const MAX_PIECES: usize = 16;

/// How many pieces `text` is once `vte` has split it on `;`.
fn pieces(text: &str) -> usize {
    text.matches(';').count() + 1
}

/// `text` cut to `room` pieces, and marked where it was cut.
///
/// A message that held more `;` than the sequence has room for — a chain of
/// shell commands waiting for approval is the ordinary one — lost everything
/// past the limit without a mark, and read as complete: the row asking for a
/// person showed a question that was not the one being asked, and the part
/// that went was the end of it. Cut at the last `;` that fits and ended with
/// `…`, it is visibly not all of it. Written here rather than read around on
/// the other side because the parser gives nothing back of what it dropped.
fn within(text: &str, room: usize) -> String {
    if pieces(text) <= room {
        return text.to_owned();
    }
    let kept: Vec<&str> = text.splitn(room + 1, ';').take(room).collect();
    format!("{}\u{2026}", kept.join(";"))
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

/// The OSC number a pull request arrives on — see the module docs for why
/// it is not [`OSC`].
pub const PULL_REQUEST_OSC: &str = "6342";

/// The word a pull request's sequence carries after the number.
///
/// The one fact the channel knows today, named rather than implied, so that a
/// second fact about the work can take the same number with a word of its own
/// and a Crook that does not know that word ignores it.
const PULL_REQUEST_WORD: &str = "pr";

/// The longest pull request address either end accepts, in bytes.
///
/// Ten times the longest address a forge hands out for one — a GitHub
/// Enterprise host, an owner, a repository and a number are well under a
/// hundred — and half of the thousand-odd bytes a `vte` built without its
/// standard library keeps of a whole OSC. Longer is not an address anybody
/// made for a pull request, and it is refused rather than cut: a cut address
/// is a link to somewhere else.
pub const PULL_REQUEST_BYTES: usize = 512;

/// `text` as a pull request's address, or `None` when it cannot be one.
///
/// The one rule both ends apply, so that nothing the writer sends is refused
/// by the reader and nothing the reader keeps could not have been sent:
///
/// - `https://` and a host after it, the scheme case-blind the way
///   `browser`'s allow-list reads one. The row opens this address, and a
///   program's output is not trustworthy — a `curl` of somebody else's server
///   can print an escape sequence as easily as a line — so what it can ask to
///   have opened is one scheme, the one every forge serves pull requests on.
/// - At most [`PULL_REQUEST_BYTES`].
/// - No control character and no white space: an address has neither, and a
///   text with either is not one.
/// - Few enough `;` that the sequence never fills `vte`'s parameter list,
///   which is how the reader can tell a whole address from one `vte` cut.
pub fn pull_request_url(text: &str) -> Option<&str> {
    const SCHEME: &str = "https://";
    let scheme = text.get(..SCHEME.len())?;
    let host = &text[SCHEME.len()..];
    let fits = text.len() <= PULL_REQUEST_BYTES && pieces(text) <= MAX_PIECES - 3;
    let clean = !text
        .chars()
        .any(|character| character.is_control() || character.is_whitespace());
    (scheme.eq_ignore_ascii_case(SCHEME)
        && !host.is_empty()
        && !host.starts_with('/')
        && fits
        && clean)
        .then_some(text)
}

/// The bytes a program writes to its terminal to say which pull request its
/// work is, or `None` for an address [`pull_request_url`] refuses.
///
/// Refused rather than cleaned, unlike a title: a title with a tab in it is
/// still a name once the tab is a space, and an address with anything taken
/// out of it is a different address.
pub fn pull_request(url: &str) -> Option<String> {
    let url = pull_request_url(url)?;
    Some(format!(
        "\x1b]{PULL_REQUEST_OSC};{PULL_REQUEST_WORD};{url}\x07"
    ))
}

/// Reads a pull request out of the parameters of an OSC sequence, or returns
/// `None` when they are not one, or are one that fails [`pull_request_url`].
///
/// Everything after the word is the address, put back together on `;` the
/// way a title is. A list as long as `vte` keeps is one it may have cut, and
/// needs no check of its own here: the address it joins into has more pieces
/// than [`pull_request_url`] lets a writer send, so it is refused as one.
pub(crate) fn parse_pull_request(parameters: &[&[u8]]) -> Option<String> {
    let [number, word, rest @ ..] = parameters else {
        return None;
    };
    if *number != PULL_REQUEST_OSC.as_bytes() || *word != PULL_REQUEST_WORD.as_bytes() {
        return None;
    }
    // Strictly: bytes that are not UTF-8 are not an address, and a lossy
    // reading would hand the row one with a replacement character in it.
    let pieces = rest
        .iter()
        .map(|piece| str::from_utf8(piece).ok())
        .collect::<Option<Vec<_>>>()?;
    let url = pieces.join(";");
    pull_request_url(&url).map(str::to_owned)
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
    /// What a pull request sequence reads as, through the same split `vte`
    /// makes.
    fn pull_request_read(sequence: &str) -> Option<String> {
        let pieces = split(sequence);
        let parameters: Vec<&[u8]> = pieces.iter().map(Vec::as_slice).collect();
        parse_pull_request(&parameters)
    }

    const PR: &str = "https://github.com/theguriev/crook/pull/398";

    #[test]
    fn a_pull_request_travels_on_a_channel_of_its_own() {
        let sequence = pull_request(PR).expect("an https address is one");
        assert_eq!(format!("\x1b]6342;pr;{PR}\x07"), sequence);
        assert_eq!(Some(PR.to_owned()), pull_request_read(&sequence));
        // Neither channel reads the other's sequence: a status is not a pull
        // request, and a pull request is not a status an older Crook could
        // mistake for one.
        assert_eq!(None, parsed(&sequence));
        assert_eq!(
            None,
            pull_request_read(&report(AgentReport::Running, Some(PR), None))
        );
    }

    #[test]
    fn only_an_https_address_is_a_pull_request() {
        // The row opens it, so the scheme is the first thing checked: a
        // program's output is not trustworthy, and a link that reached the
        // platform's opener as `file:` or an application's own scheme is the
        // thing `browser`'s allow-list exists to stop.
        for refused in [
            "http://github.com/o/r/pull/1",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ssh://github.com/o/r",
            "github.com/o/r/pull/1",
            "https://",
            "https:///o/r/pull/1",
            "",
        ] {
            assert_eq!(None, pull_request_url(refused), "{refused:?} was taken");
            assert_eq!(None, pull_request(refused), "{refused:?} was written");
            assert_eq!(
                None,
                pull_request_read(&format!("\x1b]6342;pr;{refused}\x07")),
                "{refused:?} was read"
            );
        }
        // A scheme is case-blind, as `browser` treats it.
        assert_eq!(
            Some("HTTPS://github.com/o/r/pull/1"),
            pull_request_url("HTTPS://github.com/o/r/pull/1")
        );
    }

    #[test]
    fn a_control_character_or_a_space_is_no_part_of_an_address() {
        // `vte` drops the C0 controls inside an OSC before this sees them, so
        // the reader's check is for what it lets through — DEL, and the C1
        // range in UTF-8 — and the writer's is for all of them.
        for refused in [
            "https://github.com/o/r/pull/1\x07",
            "https://github.com/o/r/pull/1\x1b]0;x",
            "https://github.com/o/r/pull/1\x7f",
            "https://github.com/o/r/pull/1\u{9b}",
            "https://github.com/o/r/pull/1 and more",
            "https://github.com/o/r\n/pull/1",
        ] {
            assert_eq!(None, pull_request_url(refused), "{refused:?} was taken");
            assert_eq!(None, pull_request(refused), "{refused:?} was written");
        }
        assert_eq!(
            None,
            parse_pull_request(&[
                b"6342",
                b"pr",
                "https://github.com/o/r/pull/1\u{7f}".as_bytes()
            ])
        );
        assert_eq!(
            None,
            parse_pull_request(&[
                b"6342",
                b"pr",
                "https://github.com/o/r/\u{85}pull/1".as_bytes()
            ])
        );
        // Bytes that are not UTF-8 are not an address, rather than one with a
        // replacement character in the middle of it.
        assert_eq!(
            None,
            parse_pull_request(&[b"6342", b"pr", b"https://github.com/o/r/pull/\xff1"])
        );
    }

    #[test]
    fn an_address_longer_than_the_cap_is_refused_not_cut() {
        // Cut, it would be a different address — and a link to somewhere else
        // is worse than none.
        let base = "https://github.com/o/r/pull/1?";
        let longest = format!("{base}{}", "a".repeat(PULL_REQUEST_BYTES - base.len()));
        assert_eq!(PULL_REQUEST_BYTES, longest.len());
        assert_eq!(Some(longest.as_str()), pull_request_url(&longest));
        assert_eq!(
            Some(longest.clone()),
            pull_request_read(&pull_request(&longest).unwrap())
        );

        let over = format!("{longest}a");
        assert_eq!(None, pull_request_url(&over));
        assert_eq!(None, pull_request(&over));
        assert_eq!(None, pull_request_read(&format!("\x1b]6342;pr;{over}\x07")));
    }

    #[test]
    fn an_address_in_pieces_is_put_back_together_unless_vte_cut_it() {
        // A `;` in an address is rare and legal — a matrix parameter — and
        // `vte` splits on it like any other, so the pieces after the word are
        // joined again the way a title's are.
        let semicolons = "https://example.com/o/r/pull/1;a=1;b=2";
        assert_eq!(
            Some(semicolons.to_owned()),
            pull_request_read(&pull_request(semicolons).unwrap())
        );

        // `vte` keeps sixteen parameters and drops the rest without a word, so
        // a sequence that filled all sixteen may have lost its end. The
        // writer never fills them, and the reader takes a full list as cut.
        let most = format!("https://example.com/p{}", ";x".repeat(MAX_PIECES - 4));
        assert_eq!(MAX_PIECES - 1, pieces(&format!("6342;pr;{most}")));
        assert_eq!(
            Some(most.clone()),
            pull_request_read(&pull_request(&most).unwrap())
        );
        let too_many = format!("{most};x");
        assert_eq!(None, pull_request_url(&too_many));
        assert_eq!(None, pull_request(&too_many));
        let mut cut: Vec<&[u8]> = vec![b"6342", b"pr", b"https://example.com/p"];
        cut.extend(std::iter::repeat_n(b"x".as_slice(), MAX_PIECES - 3));
        assert_eq!(MAX_PIECES, cut.len());
        assert_eq!(None, parse_pull_request(&cut));
    }

    #[test]
    fn a_pull_request_sequence_with_another_word_or_none_is_not_one() {
        assert_eq!(
            None,
            pull_request_read(&format!("\x1b]6342;issue;{PR}\x07"))
        );
        assert_eq!(None, pull_request_read("\x1b]6342;pr\x07"));
        assert_eq!(None, pull_request_read(&format!("\x1b]6343;pr;{PR}\x07")));
        assert_eq!(None, parse_pull_request(&[b"6342"]));
    }
}
