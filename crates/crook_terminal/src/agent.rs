//! The channel a program reports its own state on.
//!
//! A shell says where a command starts and ends with OSC 133, and that is all
//! a shell can say: from where it stands an agent is one command that has not
//! finished yet. Whether that agent is working, waiting for an answer or has
//! given up is something only the agent knows, and this is the sequence it
//! says it with —
//! `OSC 6340 ; <status> [; <title> [;; <message> [;; <session> [;; start]]]] BEL`,
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
//! A second empty field ends the message the same way, and what follows it
//! is the session: `running;port the tab bar;;;f2368c19` is a title, no
//! message and a session. So both texts lose a `;` at either end as well,
//! since one there beside a cut would be an empty field of its own. The
//! session is one word, refused rather than cut or squeezed when it is not
//! one — see [`session_id`] — because a different word is a different
//! session. A Crook older than the field reads it into the message, which
//! the row shows only beside `needs-input`: wrong on the row there, and
//! nothing worse, as the message once was.
//!
//! A third empty field and the word `start` after the session say that the
//! session starts with this report —
//! `running;port the tab bar;;;f2368c19;;start` — which is what hands the
//! pane to it; see below. Anything else after the session is not a start,
//! and the session is still read.
//!
//! On 6340 rather than on a number of its own, as the pull request is,
//! because it is a fact about the report rather than about the work: a
//! sequence of its own would have to be paired with the report after it,
//! and a report another hook wrote between the two would be taken as said
//! by that session.
//!
//! # Whose report it is
//!
//! An agent can hand part of its work to a subagent in a session of its
//! own, and the agent's hooks run for that session as they do for the one
//! the person is talking to. GitHub Copilot CLI's say a subagent's prompt as
//! a prompt and its end as an end, while the parent is still working: taken
//! as the pane's, the prompt the agent wrote to itself named the tab until
//! the next one, and the subagent's end was the agent's, so a lead's
//! `crook pane wait --until idle` woke partway through the turn.
//!
//! So a report can say which session it is from, and the pane takes a title
//! and a stop — idle or failed — only from the session it holds: the first
//! to say `running` while nobody held it, or the last to say it starts,
//! until that session stops, another starts or the command the agent was
//! ends. Another session's question is still taken,
//! without its title, since it waits on a person all the same; so is its
//! `running`, which is how it says the question was answered. A report that
//! names no session is taken whoever holds the pane, because nothing says it
//! is a subagent's. `Sessions` is the rule.
//!
//! Let go when the session stops rather than held for the life of the
//! agent, because one agent can go on in a new session — Copilot's `/clear`
//! does — and a pane that kept the first for good would take every turn
//! after it for a subagent's. And taken by `running` alone, the start of a
//! turn, because an agent at rest can still ask for a look — Copilot's
//! notifications come whether or not a turn is under way — and a question
//! that took the pane would hold it against the next session's turn.
//!
//! And handed over, whoever holds it, to a session that says it starts. An
//! agent can stop without saying so — Copilot says no `agentStop` for a turn
//! interrupted with Esc — and go on in a new session in the same process:
//! `/new` and `/resume` close nothing, so the old session's `sessionEnd`
//! waits for the process to end. With the interrupted session still held,
//! every turn of the new one was taken for a subagent's, its prompt naming
//! nothing and its end ending nothing until the agent exited. Copilot says
//! a session starts, with the prompt that started it, on the first prompt
//! of every session a person begins or resumes — a few milliseconds after
//! that prompt's own report — and never for a subagent's, so that report is
//! the one the pane goes over on, and it brings back the title the prompt's
//! own report lost.
//!
//! # How long the fields may be
//!
//! The reader keeps at most [`TITLE_BYTES`] of a title and [`MESSAGE_BYTES`]
//! of a message, cut where a character ends and marked `…`. Anything in the
//! pane can write the sequence — a forged report, or a corrupted file
//! somebody `cat`s — and `vte` keeps an OSC however long it runs, while a
//! row shapes its text again on every frame it draws: four megabytes of
//! title held the window for seconds a frame, until something said another.
//! Both caps are far above what `crook --agent` hands [`report`], so nothing
//! a hook says is cut on the way in.
//!
//! A session goes first when the writer counts what `vte` will keep — it
//! and the cut before it, its start and the cut before that, and the cut
//! before the message, which it needs even when there is none — so that it
//! is never what the parser drops: a session lost is a subagent's report
//! taken as the parent's, and a start lost is a pane kept for a session
//! that has gone.
//!
//! # The pull request, on a channel of its own
//!
//! An agent that has opened a pull request can say which, and the row links
//! to it: `OSC 6342 ; pr ; <url> BEL`. Crook asks no forge for this — the
//! agent knows the address because it just made it, and the terminal is how
//! it says so, for every reason above.
//!
//! A sibling number rather than a field on 6340, for three reasons. The tail
//! of 6340 is already cut, on the `;` `vte` splits at and on the `;;` before
//! a message and before a session, and an address would be one more cut a
//! writer had to get right. A Crook older than the field would read the
//! address into the title, where 6340's own tail has always been "wrong on
//! the row and nothing worse"; a number it does not know is a link that is
//! not there, which is better. And an address is not a status: it arrives
//! once, when the pull request is made, and a report on 6340 would have to
//! carry a word the agent did not mean to say again. `6341` is spoken for,
//! by the question an agent that stopped to ask is waiting on.
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

/// The bytes a program writes to its terminal to report `status`, said by
/// `session` when it names one.
///
/// The title is dropped rather than escaped when it holds a byte that would
/// end the sequence early or be read as a parameter of its own: a program
/// with a control character in its title has no title worth keeping, and an
/// escaping scheme is a second parser on both ends of the wire. The message
/// is dropped by the same rule, and both lose a `;;`, because that is the
/// cut between them — see the module docs. A message with no title is the
/// cut straight after the status, `;;message`: the empty piece the reader
/// looks for is the first one, and there is no title in front of it. A
/// session that is not a [`session_id`] is left off, start and all, and the
/// report goes without it as it would have gone before there were sessions.
pub fn report(
    status: AgentReport,
    title: Option<&str>,
    message: Option<&str>,
    session: Option<Session<&str>>,
) -> String {
    // And each text loses a `;` at either end, or is left off when that
    // leaves nothing: beside the cut in front of it or the one behind it, an
    // edge semicolon is an empty piece of its own, and the reader cuts at
    // the first one — `running;;title` is a message with no title.
    let text = |text: Option<&str>| {
        text.and_then(field)
            .map(|text| text.trim_matches(';').to_owned())
            .filter(|text| !text.is_empty())
    };
    let (title, message) = (text(title), text(message));
    let session = session.and_then(|said| session_id(said.id()).map(|id| said.map(|_| id)));

    // What `vte` will keep of it. The number and the status are two pieces.
    // A session is two more with the cut in front of it, and its start two
    // more again; it needs the cut before a message whether or not there is
    // one, which is one more, and a message needs at least one of its own.
    // Everything past the limit is dropped by the parser without a word, so
    // it is cut here instead, where the cut can say so — and never out of
    // the session, counted first.
    let cut = message.is_some() || session.is_some();
    let said = match session {
        None => 0,
        Some(Session::Of(_)) => 2,
        Some(Session::Starts(_)) => 4,
    };
    let mut budget = MAX_PIECES - 2 - said - usize::from(cut);
    let mut fields = vec![OSC.to_owned(), status.word().to_owned()];
    if let Some(title) = title {
        let title = within(&title, budget - usize::from(message.is_some()));
        budget -= pieces(&title);
        fields.push(title);
    }
    if cut {
        fields.push(String::new());
    }
    if let Some(message) = message {
        fields.push(within(&message, budget));
    }
    if let Some(session) = session {
        fields.push(String::new());
        fields.push((*session.id()).to_owned());
        if session.starts() {
            fields.push(String::new());
            fields.push(STARTS.to_owned());
        }
    }
    format!("\x1b]{}\x07", fields.join(";"))
}

/// Which of an agent's sessions a report is from, and whether that session
/// starts with it — see "Whose report it is" in the module docs.
///
/// Over the id, so that the writer can hand one it borrows and the reader
/// one it owns.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Session<Id> {
    /// Said by the session with this id, which may be a subagent's.
    Of(Id),
    /// Said by the session with this id as it starts: one a person began or
    /// resumed, never a subagent's, and the pane's from this report on,
    /// whichever session held it.
    Starts(Id),
}

impl<Id> Session<Id> {
    /// The session's id.
    pub fn id(&self) -> &Id {
        match self {
            Self::Of(id) | Self::Starts(id) => id,
        }
    }

    /// Whether the session starts with the report.
    pub fn starts(&self) -> bool {
        matches!(self, Self::Starts(_))
    }

    /// The same session, starting or not, with its id borrowed.
    pub fn as_ref(&self) -> Session<&Id> {
        match self {
            Self::Of(id) => Session::Of(id),
            Self::Starts(id) => Session::Starts(id),
        }
    }

    /// The same session, starting or not, with its id made into another.
    pub fn map<To>(self, id: impl FnOnce(Id) -> To) -> Session<To> {
        match self {
            Self::Of(of) => Session::Of(id(of)),
            Self::Starts(of) => Session::Starts(id(of)),
        }
    }
}

/// The word after a session that says it starts with the report.
const STARTS: &str = "start";

/// The longest session id either end takes, in bytes.
///
/// Every agent that names its sessions names them with a UUID or something
/// shorter — OpenCode's are `ses_` and two dozen characters — and this is
/// room for one that does not, while what a forged report can make a pane
/// hold on to stays a few hundred bytes.
pub const SESSION_BYTES: usize = 128;

/// `text` as the id of an agent's session, or `None` when it cannot be one.
///
/// One word, and the same rule at both ends: something, at most
/// [`SESSION_BYTES`] of it, with no `;`, which would split it on the wire,
/// and no white space or control character, which no agent puts in an id.
/// Refused rather than cleaned, as a pull request's address is, because an
/// id with anything taken out of it is another session's.
pub fn session_id(text: &str) -> Option<&str> {
    let one_word = !text.is_empty()
        && text.len() <= SESSION_BYTES
        && !text.chars().any(|character| {
            character == ';' || character.is_whitespace() || character.is_control()
        });
    one_word.then_some(text)
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

/// The longest title a pane's output can give a row, in bytes: a report's,
/// and the one OSC 0 and 2 set, which is the same name on the same row.
///
/// The row prints a title on one line a few dozen characters wide, and the
/// widest place one goes — the window's own title bar — shows a few hundred
/// at most. The 60 characters `crook --agent` cuts a title to fit with room
/// to spare, even at four bytes each, with the mark they may end in.
pub const TITLE_BYTES: usize = 256;

/// The longest message a report keeps, in bytes.
///
/// A question rather than a name, so longer than [`TITLE_BYTES`]: the row
/// gives it a line of its own, and what it asks has to survive being cut.
/// The 200 characters `crook --agent` cuts one to fit at four bytes each,
/// with room left for a writer that cut nothing.
pub const MESSAGE_BYTES: usize = 1024;

/// `text` whole up to `bytes`, and past that its beginning and an ellipsis,
/// cut where a character ends.
///
/// Where a character ends rather than where a grapheme does: the cut is far
/// past anything a row has room to print, and the row cuts again at its own
/// width, so what a split grapheme would spoil is never drawn there.
pub(crate) fn capped(mut text: String, bytes: usize) -> String {
    if text.len() <= bytes {
        return text;
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push('\u{2026}');
    text
}

/// One report, as it arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reported {
    /// What the program said it was doing.
    pub status: AgentReport,
    /// What it called its work, when it said: at most [`TITLE_BYTES`] of it.
    pub title: Option<String>,
    /// What it is waiting for, when it said: at most [`MESSAGE_BYTES`] of it.
    pub message: Option<String>,
}

/// Reads a report out of the parameters of an OSC sequence, and the session
/// it names, or returns `None` when they are not one this crate reads.
///
/// `parameters` is what `vte` split on `;`, so a title with a semicolon in it
/// arrives in pieces and is put back together here: the sequence has exactly
/// two fields before the title, and everything after them is the title up to
/// the first empty piece, which is the `;;` that starts the message, and the
/// message up to the second, after which is the session, and after a third
/// whether it starts.
pub(crate) fn parse(parameters: &[&[u8]]) -> Option<(Reported, Option<Session<String>>)> {
    let [number, word, rest @ ..] = parameters else {
        return None;
    };
    if *number != OSC.as_bytes() {
        return None;
    }
    let status = AgentReport::parse(str::from_utf8(word).ok()?)?;
    let (title, rest) = cut(rest);
    let (message, rest) = cut(rest);
    let (session, rest) = cut(rest);
    let starts = matches!(rest, [word] if *word == STARTS.as_bytes());
    let reported = Reported {
        status,
        title: joined(title, TITLE_BYTES),
        message: joined(message, MESSAGE_BYTES),
    };
    let session = session_of(session).map(|id| {
        if starts {
            Session::Starts(id)
        } else {
            Session::Of(id)
        }
    });
    Some((reported, session))
}

/// `pieces` up to the first empty one, and what comes after it.
fn cut<'a, 'b>(pieces: &'a [&'b [u8]]) -> (&'a [&'b [u8]], &'a [&'b [u8]]) {
    match pieces.iter().position(|piece| piece.is_empty()) {
        Some(at) => (&pieces[..at], &pieces[at + 1..]),
        None => (pieces, &[]),
    }
}

/// The session the pieces after the message name, when they are one piece
/// and a [`session_id`].
fn session_of(pieces: &[&[u8]]) -> Option<String> {
    let [piece] = pieces else {
        return None;
    };
    session_id(str::from_utf8(piece).ok()?).map(str::to_owned)
}

/// Which session a pane takes a title and a stop from — the rule the module
/// docs give under "Whose report it is".
#[derive(Debug, Default)]
pub(crate) struct Sessions {
    /// The session the pane holds: the first to say it is running while
    /// nobody held it, or the last to say it starts, until it stops, another
    /// starts or the command the agent was ends.
    held: Option<String>,
}

impl Sessions {
    /// `reported` as the pane takes it from `session`, or `None` when the
    /// pane takes none of it.
    ///
    /// Another session's title is the prompt an agent wrote to its
    /// subagent, and its stop is the subagent's, with the agent still at
    /// work; its question and its going on after one are the agent's own.
    /// A session that starts is no subagent's, and holds the pane from here.
    pub(crate) fn admit(
        &mut self,
        mut reported: Reported,
        session: Option<Session<String>>,
    ) -> Option<Reported> {
        let Some(said) = session else {
            return Some(reported);
        };
        let starts = said.starts();
        let (Session::Of(session) | Session::Starts(session)) = said;
        if starts {
            self.held = Some(session.clone());
        }
        let stopped = matches!(reported.status, AgentReport::Idle | AgentReport::Failed);
        match &self.held {
            Some(held) if *held != session => {
                if stopped {
                    return None;
                }
                reported.title = None;
            }
            Some(_) if stopped => self.held = None,
            Some(_) => {}
            None if reported.status == AgentReport::Running => self.held = Some(session),
            None => {}
        }
        Some(reported)
    }

    /// Lets go of the session held, for whichever reports next.
    pub(crate) fn release(&mut self) {
        self.held = None;
    }
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

/// `pieces` as the one text `vte` cut them out of, [`capped`] at `bytes`, or
/// `None` for no text.
fn joined(pieces: &[&[u8]], bytes: usize) -> Option<String> {
    let text = pieces
        .iter()
        .map(|piece| String::from_utf8_lossy(piece))
        .collect::<Vec<_>>()
        .join(";");
    let text = text.trim();
    (!text.is_empty()).then(|| capped(text.to_owned(), bytes))
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
        parsed_with_session(sequence).map(|(reported, _)| reported)
    }

    /// [`parsed`], with the session the report names.
    fn parsed_with_session(sequence: &str) -> Option<(Reported, Option<Session<String>>)> {
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
                parsed(&report(status, None, None, None))
            );
        }
    }

    #[test]
    fn a_title_travels_with_the_status_and_keeps_its_semicolons() {
        let sequence = report(AgentReport::Running, Some("fix a; then b"), None, None);
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
            None,
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
        let sequence = report(AgentReport::NeedsInput, None, Some("approve?"), None);
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
        let sequence = report(AgentReport::Running, Some("a;;;b"), Some("c;;d"), None);
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
            parsed(&report(AgentReport::NeedsInput, Some(title), message, None))
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
            report(AgentReport::Failed, Some("one\x07two"), None, None)
        );
        assert_eq!(
            format!("\x1b]{OSC};failed\x07"),
            report(AgentReport::Failed, None, Some("one\x07two"), None)
        );
        assert_eq!(
            Some(Reported {
                status: AgentReport::NeedsInput,
                title: None,
                message: None,
            }),
            parsed(&report(
                AgentReport::NeedsInput,
                Some("   "),
                Some(" "),
                None
            ))
        );
    }

    #[test]
    fn a_word_that_is_not_a_status_is_not_a_report() {
        assert_eq!(None, parsed("\x1b]6340;sleeping\x07"));
        assert_eq!(None, parsed("\x1b]6341;running\x07"));
        assert_eq!(None, parse(&[b"6340"]));
    }

    const SESSION: &str = "f2368c19-368c-461e-8976-2cfa5d93311c";

    #[test]
    fn a_session_travels_after_a_second_cut() {
        // After the message when there is one, and after the cut a message
        // would have been behind when there is none, so that it is never
        // read as the message.
        for (title, message, wire) in [
            (None, None, format!("idle;;;{SESSION}")),
            (
                Some("fix a; then b"),
                None,
                format!("running;fix a; then b;;;{SESSION}"),
            ),
            (
                None,
                Some("approve?"),
                format!("needs-input;;approve?;;{SESSION}"),
            ),
            (
                Some("fix a"),
                Some("run `a`; then `b`?"),
                format!("needs-input;fix a;;run `a`; then `b`?;;{SESSION}"),
            ),
        ] {
            let status = AgentReport::parse(wire.split(';').next().unwrap()).unwrap();
            let sequence = report(status, title, message, Some(Session::Of(SESSION)));
            assert_eq!(format!("\x1b]{OSC};{wire}\x07"), sequence);
            assert_eq!(
                Some((
                    Reported {
                        status,
                        title: title.map(str::to_owned),
                        message: message.map(str::to_owned),
                    },
                    Some(Session::Of(SESSION.to_owned()))
                )),
                parsed_with_session(&sequence),
                "{wire}"
            );
        }
        // And a report that names none reads as it always did.
        assert_eq!(
            Some(None),
            parsed_with_session(&report(AgentReport::Idle, None, None, None))
                .map(|(_, session)| session)
        );
    }

    #[test]
    fn a_semicolon_at_the_edge_of_a_message_cannot_forge_the_session_cut() {
        // `approve;` before the cut is `approve;;;`, which read as a message
        // `approve` and a session that was an empty piece and the id.
        for message in ["approve;", ";approve", ";approve;", ";;approve;;"] {
            assert_eq!(
                Some((
                    Reported {
                        status: AgentReport::NeedsInput,
                        title: None,
                        message: Some("approve".to_owned()),
                    },
                    Some(Session::Of(SESSION.to_owned()))
                )),
                parsed_with_session(&report(
                    AgentReport::NeedsInput,
                    None,
                    Some(message),
                    Some(Session::Of(SESSION))
                )),
                "{message:?}"
            );
        }
    }

    #[test]
    fn a_session_that_is_not_one_word_is_left_off_rather_than_cut() {
        // Cleaned or cut, it would be another session's id, and the pane
        // would take the parent's report for a subagent's.
        let longest = "s".repeat(SESSION_BYTES);
        assert_eq!(Some(longest.as_str()), session_id(&longest));
        for refused in [
            String::new(),
            "two words".to_owned(),
            "a;b".to_owned(),
            "tab\there".to_owned(),
            "bell\x07".to_owned(),
            format!("{longest}s"),
        ] {
            assert_eq!(None, session_id(&refused), "{refused:?} was taken");
            for said in [
                Session::Of(refused.as_str()),
                Session::Starts(refused.as_str()),
            ] {
                assert_eq!(
                    report(AgentReport::Running, Some("port"), None, None),
                    report(AgentReport::Running, Some("port"), None, Some(said)),
                    "{said:?} was written"
                );
            }
        }
        // Read the same way: more than one piece after the cut, or a piece
        // that is not one word, is no session, and the report is still read.
        for wire in [
            format!("\x1b]{OSC};idle;;;{SESSION};more\x07"),
            format!("\x1b]{OSC};idle;;;{}s\x07", longest),
            format!("\x1b]{OSC};idle;;;two words\x07"),
        ] {
            assert_eq!(
                Some((
                    Reported {
                        status: AgentReport::Idle,
                        title: None,
                        message: None,
                    },
                    None
                )),
                parsed_with_session(&wire),
                "{wire:?}"
            );
        }
    }

    /// A report as [`admitted`] is handed it: a status, a title and the
    /// session that said it.
    type Said<'a> = (AgentReport, Option<&'a str>, Option<Session<&'a str>>);

    /// The reports the pane takes of `said`.
    fn admitted(said: &[Said<'_>]) -> Vec<Reported> {
        let mut sessions = Sessions::default();
        said.iter()
            .filter_map(|(status, title, session)| {
                let reported = Reported {
                    status: *status,
                    title: title.map(str::to_owned),
                    message: None,
                };
                sessions.admit(reported, session.map(|said| said.map(str::to_owned)))
            })
            .collect()
    }

    /// Each report the pane takes, as its status and its title.
    fn heard(taken: &[Reported]) -> Vec<(AgentReport, Option<&str>)> {
        taken
            .iter()
            .map(|reported| (reported.status, reported.title.as_deref()))
            .collect()
    }

    #[test]
    fn another_sessions_failure_is_no_more_the_panes_than_its_idle() {
        // A subagent that failed is a tool call that failed: the agent above
        // it reads the failure and goes on.
        use AgentReport::{Failed, Running};
        use Session::Of;
        let taken = admitted(&[
            (Running, Some("port"), Some(Of("parent"))),
            (Failed, None, Some(Of("child"))),
            (Failed, None, Some(Of("parent"))),
            (Running, Some("next"), Some(Of("child"))),
        ]);
        // The parent's failure lets the pane go, as its idle would.
        assert_eq!(
            vec![
                (Running, Some("port")),
                (Failed, None),
                (Running, Some("next"))
            ],
            heard(&taken)
        );
    }

    #[test]
    fn a_start_travels_after_the_session_behind_a_third_cut() {
        // Behind one more cut after the session, as the session is behind
        // one after the message, so that every field before it is read as
        // it was.
        for (title, message, wire) in [
            (None, None, format!("running;;;{SESSION};;start")),
            (
                Some("port the tab bar"),
                None,
                format!("running;port the tab bar;;;{SESSION};;start"),
            ),
            (
                Some("fix a"),
                Some("approve?"),
                format!("needs-input;fix a;;approve?;;{SESSION};;start"),
            ),
        ] {
            let status = AgentReport::parse(wire.split(';').next().unwrap()).unwrap();
            let sequence = report(status, title, message, Some(Session::Starts(SESSION)));
            assert_eq!(format!("\x1b]{OSC};{wire}\x07"), sequence);
            assert_eq!(
                Some((
                    Reported {
                        status,
                        title: title.map(str::to_owned),
                        message: message.map(str::to_owned),
                    },
                    Some(Session::Starts(SESSION.to_owned()))
                )),
                parsed_with_session(&sequence),
                "{wire}"
            );
        }
        // Any other word there, or more than the one, is no start, and the
        // session is still the report's.
        for wire in [
            format!("\x1b]{OSC};idle;;;{SESSION};;started\x07"),
            format!("\x1b]{OSC};idle;;;{SESSION};;start;more\x07"),
            format!("\x1b]{OSC};idle;;;{SESSION};;\x07"),
        ] {
            assert_eq!(
                Some(Some(Session::Of(SESSION.to_owned()))),
                parsed_with_session(&wire).map(|(_, session)| session),
                "{wire:?}"
            );
        }
    }

    #[test]
    fn a_session_that_starts_takes_the_pane_from_one_that_never_stopped() {
        // Copilot CLI says no `agentStop` for a turn interrupted with Esc,
        // and `/new` goes on in a new session in the same process: the new
        // session's prompt comes first and loses its title to the held one,
        // and its start, a moment later, hands the pane over and names it.
        use AgentReport::{Idle, Running};
        use Session::{Of, Starts};
        let taken = admitted(&[
            (Running, Some("first"), Some(Of("interrupted"))),
            (Running, Some("second"), Some(Of("new"))),
            (Running, Some("second"), Some(Starts("new"))),
            (Running, Some("run the subtask"), Some(Of("subagent"))),
            (Idle, None, Some(Of("subagent"))),
            (Idle, None, Some(Of("interrupted"))),
            (Idle, None, Some(Of("new"))),
        ]);
        assert_eq!(
            vec![
                (Running, Some("first")),
                (Running, None),
                (Running, Some("second")),
                (Running, None),
                (Idle, None),
            ],
            heard(&taken)
        );
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
            pull_request_read(&report(AgentReport::Running, Some(PR), None, None))
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
