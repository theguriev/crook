//! `agent.ask`: the questions agents' permission hooks are waiting on.
//!
//! When Claude Code is about to ask a person whether it may run a command, it
//! runs its `PermissionRequest` hook first, and the hook — `crook --agent ask
//! claude`, see [`crate::agent::ask`] — hands the window the whole request
//! and waits on its connection for an answer. This is the window's side: the
//! questions held, which pane each belongs to, and what happened to each.
//! Nothing here answers one by itself. An answer is a person's press on the
//! card the window draws — see `workspace::answering` — handed to
//! [`Asks::deliver`]; every other way a question ends is a pass, and the
//! agent's own prompt, open beside the question the whole time, is still
//! the one a person can answer instead.
//!
//! # Which pane
//!
//! The terminal the question names: the device number of the terminal a
//! status from the same hook would be written to, matched against each
//! pane's pty — see `Workspace::pane_with_tty`. Not the hook's `CROOK_TOKEN`,
//! which a nested agent and a tmux server inherit from whichever pane they
//! were started in; a question naming no pane of this window is refused as
//! `no-such-pane`, and the hook passes.
//!
//! Then three things that can make a question one this window will show but
//! never answer — see [`CannotAnswer`]: a token naming another pane, an asker
//! the kernel says is not a process of the pane's shell, and a pane working
//! on another machine.
//!
//! # From inside tmux
//!
//! There the agent's terminal is one of tmux's panes, which no pane of a
//! window is on, and the hook names the terminals of the tmux clients
//! attached to the agent's session instead — the terminals a status from it
//! goes to as well. The question goes to the first pane in the panel's order
//! that is on one of them: the pane a person sees tmux in. Of the three
//! checks, only the pane working on another machine is made. The tmux
//! server is not below that pane's shell — it was started wherever tmux was
//! first run, and lives on after — so the asker never is, and the token it
//! has is whichever pane started the server, which is no hint at all. What
//! is checked instead is the kernel's account of the asker's ancestors: one
//! of them has to hold the pane's terminal open and run the very program in
//! the pane's foreground, as tmux's server does for every process in its
//! panes — see [`from_its_tmux`]. Any process can name a terminal; only one
//! under that tmux has such an ancestor, and any other is shown read-only.
//! The card says the question is from inside tmux, and in how many other
//! tabs tmux is attached too.
//!
//! Such questions are counted apart from the pane's own, and never replace
//! one, nor stand in front of one: whatever comes in through tmux cannot
//! take the room or the place of a question the pane's agent asked
//! directly.
//!
//! A hook whose `CROOK_SOCKET` names a window that holds none of those
//! terminals — tmux was started from a window since closed — asks
//! `pane.find`, which carries the terminals and nothing else, of the
//! window's and a few others' sockets, and sends the question only to the
//! first that finds one: see [`find`].
//!
//! # How a question ends
//!
//! With the hook going ([`Cause::HookGone`], which the connection tells the
//! window through a [`Notice`] the moment it sees it), its time running out,
//! its pane closing, the agent's command ending, the agent saying its turn is
//! over or failed, the same session asking again, the tool it was about
//! having run ([`Cause::AnsweredInAgent`]: a person said yes in the agent's
//! own dialog, which tells the hook nothing), the agent's title saying it is
//! at work again after its dialog opened ([`Cause::AtWorkAgain`], the same
//! answer seen sooner: see [`Asks::titled`]), or the window closing.
//! Each but the last pushes a pass into the question's feed, which the
//! connection writes and the hook prints nothing for; a window that closes
//! says `gone` instead. Each is a line in the log — the agent, the tool, how
//! it ended and the first twelve hex digits of its digest, never the command
//! — and an entry in [`Asks::history`], in memory only: the last
//! [`HISTORY`], a line of each and never the request, which "Why this
//! status" and the inbox both read and say in one wording, [`How::said`].
//!
//! # What a lead is told
//!
//! That its worker asks, and how that ended — `asked` and `answered` on
//! the streams following the pane, by the lineage a status is told by: see
//! [`Asks::take_told`]. The agent, the tool and how it ended, and never the
//! command, the file, the directory, the digest or the fingerprint. A lead
//! can see that a worker waits and ask its person; nothing it can send
//! answers.
//!
//! A connection that gave up on the window before the window said it held
//! the question — a frame that stalled past the connection's patience — has
//! closed the question's feed and its hook has passed. Such a question is
//! refused if it has not been held yet, is never shown if it has, and is
//! ended as [`Cause::HookGone`] the next time a question is held or a pane
//! closes.
//!
//! Then which of them a pane shows: a question this window could answer
//! comes before one it cannot, a read-only question never takes an
//! answerable one's place or its room, and only an answerable one is ever
//! the row's second line.
//!
//! # What it costs
//!
//! Nothing while nobody asks. A question is a struct and a feed, at most
//! [`MAX_PER_PANE`] in a pane and as many in the window as the socket's lane
//! for them holds; nothing ticks for one — its time running out is the
//! connection's thread's to notice — and nothing about it is written to disk.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use crook_terminal::agent::Fingerprint;
use crookui_core::prelude::*;
use serde_json::Value;

use super::protocol::{
    AgentAsk, Answered, AnsweredBy, Asker, AskingAgent, Decision, Executable, Find, Located,
    MultiplexerKind, PaneEvent, Refusal, Registered, Verdict, code,
};
use super::watch::Feed;
use super::{Inbox, Notice};
use crate::agent::request::{self, AskRequest};
use crate::tab::{AgentStatus, PaneId, TabStrip};
use crate::workspace::Workspace;

/// How many questions one pane may hold at once. The next is refused as
/// `busy`, and its hook passes.
///
/// Claude Code shows one permission dialog at a time, and a newer question
/// from the same session replaces the one before; four is room for a few
/// agents sharing a pane — a lead and the shells it runs — and a bound on
/// what one gone wrong can make the window hold.
pub const MAX_PER_PANE: usize = 4;

/// How many ended questions the window remembers, newest last.
pub const HISTORY: usize = 64;

/// The longest a question's one-line summary is, in characters, its mark
/// where it was cut included.
pub const SUMMARY_CHARS: usize = 120;

/// What a deny tells the agent: who said no, and where.
pub const DENIED: &str = "Denied in Crook by the person at the keyboard.";

/// A question's number in this window: counted up from one, never handed to
/// another question, so that a press on a card can name the one it was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AskId(u64);

impl AskId {
    /// The number, as the wire carries it.
    #[cfg(test)]
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// The question the window numbered `number` when it said it held it:
    /// what a connection names its question by in a [`Notice`].
    pub(crate) fn from_wire(number: u64) -> Self {
        Self(number)
    }
}

/// Why a question is shown and can never be answered from this window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CannotAnswer {
    /// The hook carries the token of another pane of this window than the
    /// one whose terminal it named.
    OtherTab,
    /// The process that asked is not one the pane's shell started, as far
    /// as the kernel says.
    NotFromThisTab,
    /// The pane is working on another machine, where its agent would be, so
    /// whatever asked on this one is not that agent.
    Remote,
    /// From inside a multiplexer, and no ancestor of the process that asked
    /// both holds the pane's terminal open and runs the program in the
    /// pane's foreground: whatever asked is not under the tmux whose client
    /// is in the pane. See [`from_its_tmux`].
    NotFromItsTmux,
}

impl CannotAnswer {
    /// The sentence a person reads beside the question.
    pub fn sentence(self) -> &'static str {
        match self {
            Self::OtherTab => {
                "It cannot be answered here: the hook carries the token of another pane of \
                 this window."
            }
            Self::NotFromThisTab => {
                "It cannot be answered here: the process that asked was not started from this \
                 pane's shell."
            }
            Self::Remote => {
                "It cannot be answered here: this pane is working on another machine, and what \
                 asked is on this one."
            }
            Self::NotFromItsTmux => {
                "It cannot be answered here: it is not from the tmux whose client is in this tab."
            }
        }
    }
}

/// Why a question ended without an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The hook hung up: its agent killed it, or it saw its parent go.
    HookGone,
    /// Its time ran out.
    Expired,
    /// Its pane closed.
    PaneClosed,
    /// The command its agent ran in the pane ended.
    CommandEnded,
    /// The agent said its turn was over, or that it failed.
    TurnEnded,
    /// The same session in the same pane asked a newer question.
    AskedAgain,
    /// The tool it was about has run, so a person answered it in the agent's
    /// own dialog: the hook that runs after the tool said so, on the pane —
    /// see [`Asks::answered_in_agent`].
    AnsweredInAgent,
    /// The agent's title said it was at work again while its dialog was up,
    /// so a person answered it there: Yes, or "No, and tell Claude what to do
    /// differently", which go on with the turn and tell the hook nothing.
    /// Which of them, and whether the tool has run, the title cannot say.
    /// See [`Asks::titled`].
    AtWorkAgain,
    /// The window closed.
    WindowClosed,
    /// Answering agents from Crook was turned off while it waited.
    AnsweringOff,
}

impl Cause {
    /// The word a pass carries as its `why`, and the log says.
    pub fn word(self) -> &'static str {
        match self {
            Self::HookGone => "hook-gone",
            Self::Expired => "expired",
            Self::PaneClosed => "pane-closed",
            Self::CommandEnded => "command-ended",
            Self::TurnEnded => "turn-ended",
            Self::AskedAgain => "asked-again",
            Self::AnsweredInAgent => "answered-in-agent",
            Self::AtWorkAgain => "at-work-again",
            Self::WindowClosed => "window-closed",
            Self::AnsweringOff => "answering-off",
        }
    }

    /// How a person is told it ended, in a few words: what the inbox's
    /// "Earlier" and "Why this status" both say, so the two never tell one
    /// ending two ways. See [`How::said`].
    pub fn said(self) -> &'static str {
        match self {
            Self::HookGone => "Its hook stopped waiting",
            Self::Expired => "Timed out",
            Self::PaneClosed => "Its pane closed",
            Self::CommandEnded => "The agent exited",
            Self::TurnEnded => "The agent's turn ended",
            Self::AskedAgain => "A newer request replaced it",
            Self::AnsweredInAgent | Self::AtWorkAgain => "Answered in the agent's own dialog",
            Self::WindowClosed => "The window closed",
            Self::AnsweringOff => "Answering in Crook was turned off",
        }
    }

    /// How a lead following the pane is told it ended.
    pub fn by(self) -> AnsweredBy {
        match self {
            Self::AnsweredInAgent | Self::AtWorkAgain => AnsweredBy::InAgent,
            Self::Expired => AnsweredBy::Expired,
            Self::AnsweringOff => AnsweredBy::Released,
            Self::PaneClosed | Self::WindowClosed => AnsweredBy::Closed,
            Self::HookGone | Self::CommandEnded | Self::TurnEnded | Self::AskedAgain => {
                AnsweredBy::Withdrawn
            }
        }
    }
}

/// What a person can answer a question with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Run it, this once.
    Allow,
    /// Do not run it.
    Deny,
}

/// What became of an answer handed to [`Asks::deliver`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// The hook's connection took it.
    Delivered,
    /// The question had ended — its time ran out, its hook went — before the
    /// answer reached it, and nothing was sent.
    TooLate,
    /// The question is not the one the answer was given about: its digest
    /// differs, so the card showed something else.
    Stale,
    /// The question is one this window cannot answer: see [`CannotAnswer`].
    ReadOnly,
}

/// How a question ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// A person answered it.
    Answered(Choice),
    /// It ended without an answer.
    Ended(Cause),
}

impl How {
    /// The word the log says.
    pub fn word(self) -> &'static str {
        match self {
            Self::Answered(Choice::Allow) => "allowed",
            Self::Answered(Choice::Deny) => "denied",
            Self::Ended(cause) => cause.word(),
        }
    }

    /// How a person is told it ended, in a few words, the same wherever it
    /// is said: what was sent for an answer — never that the agent did it —
    /// and [`Cause::said`] for the rest.
    pub fn said(self) -> &'static str {
        match self {
            Self::Answered(Choice::Allow) => "Allow sent",
            Self::Answered(Choice::Deny) => "Deny sent",
            Self::Ended(cause) => cause.said(),
        }
    }

    /// What a lead following the pane is told: how it ended, and what was
    /// sent when a person answered it here.
    fn told(self) -> (AnsweredBy, Option<Decision>) {
        match self {
            Self::Answered(Choice::Allow) => (AnsweredBy::Here, Some(Decision::Allow)),
            Self::Answered(Choice::Deny) => (AnsweredBy::Here, Some(Decision::Deny)),
            Self::Ended(cause) => (cause.by(), None),
        }
    }
}

/// One question a hook is waiting on.
pub struct Ask {
    /// Its number.
    pub id: AskId,
    /// Which agent asks.
    pub agent: AskingAgent,
    /// The agent's session.
    pub session: String,
    /// Where the agent works.
    pub cwd: String,
    /// What it asks to do, whole.
    pub request: AskRequest,
    /// What it asks in one line: see [`summary`].
    pub summary: String,
    /// The digest of the session, the tool and the request: what a press
    /// carries, so an answer can only reach the question it was given about.
    pub digest: [u8; 32],
    /// The digest's first half in hex: see [`request::fingerprint`].
    pub fingerprint: String,
    /// The pane whose terminal it named.
    pub pane: PaneId,
    /// Why it can be shown and not answered, when it cannot.
    pub cannot_answer: Option<CannotAnswer>,
    /// When the window took it.
    pub asked_at: Instant,
    /// Where in a multiplexer it was asked, for a question from inside one.
    pub inside: Option<Inside>,
    /// What the pane's row was called when it asked.
    tab_title: String,
    /// What the hook's connection waits on.
    feed: Arc<Feed>,
    /// Whether the last title the pane has had since the window took it,
    /// or the one it had then, was Claude Code's `✳`: see [`Asks::titled`].
    seen_waiting: bool,
}

impl Ask {
    /// The agent's name for the tool it asks to use.
    pub fn tool(&self) -> &'static str {
        self.agent.tool(&self.request)
    }

    /// Whether the pane's row says needs input for this question from the
    /// moment it is held, rather than once the agent says so: one this
    /// window could answer, which the pane's own agent asked directly.
    ///
    /// Not one from inside a multiplexer. What ends the row's question when
    /// a person answers in the agent's dialog is the agent's title, and
    /// inside tmux the pane's title is tmux's — or, with its `set-titles`,
    /// whichever of its panes is current, which need not be the one asking.
    /// Nothing would take the amber back until the tool had run. Nor one
    /// this window cannot answer: that is some process that named the
    /// pane's terminal, and it does not get to make a row amber.
    ///
    /// And only Claude Code's: the title that ends it is Claude Code's
    /// glyph, measured on Claude Code. Another agent's question waits for
    /// its own measured cue before its row turns at once.
    pub fn shows_at_once(&self) -> bool {
        self.agent == AskingAgent::Claude && self.cannot_answer.is_none() && self.inside.is_none()
    }
}

/// One question that has ended, as the window remembers it: what it was
/// about in a line, and how it went. Never the request itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Its number.
    pub id: AskId,
    /// The pane it was asked in.
    pub pane: PaneId,
    /// When the window took it.
    pub asked_at: Instant,
    /// When it ended.
    pub ended_at: Instant,
    /// What its pane's row was called when it asked.
    pub tab_title: String,
    /// Which agent asked.
    pub agent: AskingAgent,
    /// The tool it asked to use.
    pub tool: &'static str,
    /// What it asked, in a line.
    pub summary: String,
    /// How it ended.
    pub how: How,
    /// Whether the hook said it printed the answer it was given.
    pub delivered: bool,
    /// Whether the hook left without saying so: gone, or going, before it
    /// could pass the answer on.
    pub lost: bool,
}

impl Outcome {
    /// How it ended, as a person is told it wherever that is said: see
    /// [`How::said`], and, for an answer, what the hook said became of it.
    pub fn said(&self) -> String {
        let how = self.how.said();
        if !matches!(self.how, How::Answered(_)) {
            return how.to_owned();
        }
        if self.delivered {
            format!("{how}, and its hook took it")
        } else if self.lost {
            format!("{how}, but its hook was gone")
        } else {
            how.to_owned()
        }
    }
}

/// What the window found out about a question before holding it, for
/// [`Asks::add`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The pane whose terminal it named.
    pub pane: PaneId,
    /// Why it cannot be answered here, when it cannot.
    pub cannot_answer: Option<CannotAnswer>,
    /// What the pane's row is called.
    pub tab_title: String,
    /// Where in a multiplexer it was asked, for a question from inside one.
    pub inside: Option<Inside>,
}

/// Where in a multiplexer a question was asked: which one, its name for the
/// agent's pane, and how many other tabs of the window its clients are in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inside {
    /// Which multiplexer.
    pub kind: MultiplexerKind,
    /// Its name for the agent's pane, as the hook said it: `%` and a number.
    pub pane: String,
    /// How many tabs of the window, besides the one the question went to,
    /// have a pane holding a client attached to the agent's session.
    pub elsewhere: usize,
}

impl Inside {
    /// Where, in a few words: `inside tmux`, and the other tabs when there
    /// are any.
    pub fn said(&self) -> String {
        let kind = self.kind.word();
        match self.elsewhere {
            0 => format!("inside {kind}"),
            1 => format!("inside {kind} (also attached in 1 other tab)"),
            more => format!("inside {kind} (also attached in {more} other tabs)"),
        }
    }
}

/// Every question the window is holding, and how the last ones ended.
///
/// Held by the workspace beside what the socket is watching. Empty, and
/// costing a length check when a pane's agent reports, while nobody asks.
#[derive(Default)]
pub struct Asks {
    /// The number the next question gets, less one.
    last: u64,
    /// The questions held, oldest first.
    live: Vec<Ask>,
    /// The questions ended, oldest first, at most [`HISTORY`].
    history: VecDeque<Outcome>,
    /// What the leads following the panes have yet to be told, oldest first:
    /// see [`Self::take_told`].
    told: VecDeque<(PaneId, PaneEvent)>,
    /// The panes whose last title was Claude Code's `✳`: see
    /// [`Self::titled`].
    waiting_titles: HashSet<PaneId>,
}

impl Asks {
    /// Whether no question is held.
    pub fn is_empty(&self) -> bool {
        self.live().next().is_none()
    }

    /// How many questions are held.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.live().count()
    }

    /// Every question held, oldest first.
    ///
    /// Only those whose hook is still waiting: a connection that gave up on
    /// the window — its hook gone, or the window too slow to say it held the
    /// question — has closed the feed, and a question nobody waits on is not
    /// one to show. It is dropped the next time a question is held or a pane
    /// closes; see [`Self::drop_abandoned`].
    pub fn live(&self) -> impl Iterator<Item = &Ask> {
        self.live.iter().filter(|ask| ask.feed.is_open())
    }

    /// The question held in `pane` that a person looking at it most wants to
    /// see: the newest this window could answer, else the newest it cannot.
    ///
    /// Answerable first, so that a process elsewhere naming the pane's
    /// terminal — which is all a read-only question is known to be — can
    /// never stand in front of the agent's own.
    pub fn in_pane(&self, pane: PaneId) -> Option<&Ask> {
        self.answerable_in(pane).or_else(|| {
            self.live()
                .filter(|ask| ask.pane == pane)
                .max_by_key(|ask| ask.id)
        })
    }

    /// The newest question held in `pane` that this window could answer:
    /// the one its agent's dialog is showing, as far as the window can tell.
    ///
    /// One the pane's agent asked directly comes before one from inside a
    /// multiplexer, whichever is newer.
    pub fn answerable_in(&self, pane: PaneId) -> Option<&Ask> {
        self.live()
            .filter(|ask| ask.pane == pane && ask.cannot_answer.is_none())
            .max_by_key(|ask| (ask.inside.is_none(), ask.id))
    }

    /// The newest question held in `pane` whose row says needs input from
    /// the moment it is held: see [`Ask::shows_at_once`]. What the pane's
    /// session mirrors as `asking`.
    pub fn shown_at_once_in(&self, pane: PaneId) -> Option<&Ask> {
        self.live()
            .filter(|ask| ask.pane == pane && ask.shows_at_once())
            .max_by_key(|ask| ask.id)
    }

    /// Whether the agent in `pane` is asking through Crook: a question this
    /// window could answer is held there. What `pane.list` says as
    /// `asking`, and nothing more.
    pub fn asking(&self, pane: PaneId) -> bool {
        self.answerable_in(pane).is_some()
    }

    /// Takes what the leads following the panes have yet to be told: an
    /// `asked` for every question held that this window could answer, and an
    /// `answered` when it ends — the agent, the tool and how, never what it
    /// asked. Who is told is the watches' to say, by the lineage they tell a
    /// status by: see `Watches::told`.
    ///
    /// Only the answerable ones: a read-only question is one the window
    /// cannot place, from some process that named the pane's terminal, and
    /// a lead told its worker asks on the strength of one would be told
    /// something nobody knows.
    pub fn take_told(&mut self) -> Vec<(PaneId, PaneEvent)> {
        self.told.drain(..).collect()
    }

    /// How many events [`Self::take_told`] has yet to take.
    #[cfg(test)]
    pub fn untold(&self) -> usize {
        self.told.len()
    }

    /// Keeps `event` about `pane` for [`Self::take_told`].
    ///
    /// Bounded at [`HISTORY`], the oldest going first, for a window whose
    /// workspace never asks — every road that changes a question takes them
    /// within the same turn, so it never bites.
    fn tell(&mut self, pane: PaneId, event: PaneEvent) {
        if self.told.len() >= HISTORY {
            self.told.pop_front();
        }
        self.told.push_back((pane, event));
    }

    /// How the questions that have ended went, oldest first.
    pub fn history(&self) -> impl Iterator<Item = &Outcome> {
        self.history.iter()
    }

    /// The question held as `id`, while its hook is still waiting on it.
    pub fn get(&self, id: AskId) -> Option<&Ask> {
        self.live().find(|ask| ask.id == id)
    }

    /// How the question held as `id` ended, once it has and while the window
    /// remembers it.
    pub fn outcome(&self, id: AskId) -> Option<&Outcome> {
        self.history.iter().rev().find(|ended| ended.id == id)
    }

    /// Holds `asked`, in the pane `found` names, answering into `feed`.
    ///
    /// Refused as `gone` when the feed is already closed: the connection
    /// stopped waiting for the window to say it held the question, and its
    /// hook has passed. Refused as `busy` when the pane already holds
    /// [`MAX_PER_PANE`] that it would keep. A question held from the same
    /// session in the same pane is one its agent is no longer asking —
    /// Claude Code shows one dialog at a time — and ends as
    /// [`Cause::AskedAgain`], and is not counted.
    ///
    /// Answerable and read-only questions are counted apart, and a read-only
    /// one ends only another read-only one: a question this window cannot
    /// answer is one it cannot place, from some process that named the
    /// pane's terminal, and it must neither take the agent's own question
    /// away nor fill the room the agent's next one needs.
    pub fn add(
        &mut self,
        asked: AgentAsk,
        found: Found,
        feed: Arc<Feed>,
        now: Instant,
    ) -> Result<AskId, Refusal> {
        if !feed.is_open() {
            return Err(Refusal::new(
                code::GONE,
                "the question's connection stopped waiting before the window could hold it",
            ));
        }
        self.drop_abandoned();
        let answerable = found.cannot_answer.is_none();
        let inside = found.inside.is_some();
        let alike = |ask: &Ask| {
            ask.pane == found.pane
                && ask.cannot_answer.is_none() == answerable
                && ask.inside.is_some() == inside
        };
        let held = self
            .live
            .iter()
            .filter(|ask| alike(ask) && ask.session != asked.session)
            .count();
        if held >= MAX_PER_PANE {
            return Err(Refusal::new(
                code::BUSY,
                format!(
                    "the pane holds {MAX_PER_PANE} agents' questions already; ask in the agent's \
                     own prompt"
                ),
            ));
        }
        // An answerable question replaces the session's read-only ones as
        // well as its own; a read-only one replaces only read-only ones; and
        // one from inside a multiplexer replaces only others from inside one.
        let replaced: Vec<AskId> = self
            .live
            .iter()
            .filter(|ask| ask.pane == found.pane && ask.session == asked.session)
            .filter(|ask| answerable || ask.cannot_answer.is_some())
            .filter(|ask| !inside || ask.inside.is_some())
            .map(|ask| ask.id)
            .collect();
        for id in replaced {
            self.end(id, Cause::AskedAgain);
        }

        self.last += 1;
        let id = AskId(self.last);
        let digest = asked.digest();
        let AgentAsk {
            agent,
            session,
            cwd,
            request,
            ..
        } = asked;
        // The row and the list say it is from inside tmux, ahead of what it
        // asks, since a person reads them for which agent wants what.
        let tool = match &found.inside {
            Some(inside) => format!("{} inside {}", agent.tool(&request), inside.kind.word()),
            None => agent.tool(&request).to_owned(),
        };
        if answerable {
            self.tell(
                found.pane,
                PaneEvent::Asked {
                    pane_id: found.pane.as_u64(),
                    agent: agent.word().to_owned(),
                    tool: agent.tool(&request).to_owned(),
                },
            );
        }
        self.live.push(Ask {
            id,
            agent,
            summary: summary(&tool, &request, &cwd),
            fingerprint: request::fingerprint(&digest),
            digest,
            session,
            cwd,
            request,
            pane: found.pane,
            cannot_answer: found.cannot_answer,
            asked_at: now,
            seen_waiting: self.waiting_titles.contains(&found.pane),
            inside: found.inside,
            tab_title: found.tab_title,
            feed,
        });
        Ok(id)
    }

    /// Ends question `id` without an answer: its hook is told to pass, with
    /// `cause` as why — or, for a window closing, that the window has gone.
    /// Nothing for a question already ended.
    pub fn end(&mut self, id: AskId, cause: Cause) {
        let Some(index) = self.live.iter().position(|ask| ask.id == id) else {
            return;
        };
        let ask = &self.live[index];
        if cause != Cause::WindowClosed {
            // Refused when the hook has already gone, or the time ran out
            // first; there is nobody left to tell either way.
            ask.feed.push(encode(&Answered::pass(cause.word())));
        }
        ask.feed.end();
        self.finish(index, How::Ended(cause), Instant::now());
    }

    /// Ends every question held in `pane`, for `cause`.
    pub fn end_in(&mut self, pane: PaneId, cause: Cause) {
        let ids: Vec<AskId> = self
            .live
            .iter()
            .filter(|ask| ask.pane == pane)
            .map(|ask| ask.id)
            .collect();
        for id in ids {
            self.end(id, cause);
        }
    }

    /// Ends the questions in `pane` whose turn a stop its agent reported
    /// ended, as [`Cause::TurnEnded`]: those of `session`, the session the
    /// stop said it was from, wherever in the pane they were asked; or, for
    /// a stop that names none, those asked in the pane's own terminal and
    /// none from inside a multiplexer, whose clients hear every one of its
    /// panes' reports — see `Workspace::end_asks_a_report_ends`.
    pub fn end_turn_in(&mut self, pane: PaneId, session: Option<&str>) {
        let ids: Vec<AskId> = self
            .live
            .iter()
            .filter(|ask| ask.pane == pane)
            .filter(|ask| match session {
                Some(session) => ask.session == session,
                None => ask.inside.is_none(),
            })
            .map(|ask| ask.id)
            .collect();
        for id in ids {
            self.end(id, Cause::TurnEnded);
        }
    }

    /// Ends every question held, for `cause`.
    pub fn end_all(&mut self, cause: Cause) {
        let ids: Vec<AskId> = self.live.iter().map(|ask| ask.id).collect();
        for id in ids {
            self.end(id, cause);
        }
    }

    /// Ends every question whose connection has stopped waiting on it
    /// without the window hearing so — one whose connection gave up on the
    /// window before it said it held the question — as [`Cause::HookGone`].
    ///
    /// A hook that hangs up after that is heard through its [`Notice`], and
    /// is ended then; this is for the one road no notice comes down.
    pub fn drop_abandoned(&mut self) {
        let abandoned: Vec<AskId> = self
            .live
            .iter()
            .filter(|ask| !ask.feed.is_open())
            .map(|ask| ask.id)
            .collect();
        for id in abandoned {
            self.end(id, Cause::HookGone);
        }
    }

    /// Ends every question whose pane `strip` no longer has, and every one
    /// nobody waits on any more: see [`Self::drop_abandoned`].
    pub fn forget_closed(&mut self, strip: &TabStrip) {
        self.waiting_titles
            .retain(|pane| strip.pane(*pane).is_some());
        if self.live.is_empty() {
            return;
        }
        self.drop_abandoned();
        let closed: Vec<AskId> = self
            .live
            .iter()
            .filter(|ask| strip.pane(ask.pane).is_none())
            .map(|ask| ask.id)
            .collect();
        for id in closed {
            self.end(id, Cause::PaneClosed);
        }
    }

    /// Answers question `id` with `choice`, when it is still held, is the
    /// question `digest` names, and is one this window may answer.
    ///
    /// The answer is pushed into the question's feed under the lock its
    /// expiry is decided under, so it either reaches the hook's connection or
    /// is [`Delivery::TooLate`]: never taken and lost.
    pub fn deliver(&mut self, id: AskId, digest: &[u8; 32], choice: Choice) -> Delivery {
        let Some(index) = self.live.iter().position(|ask| ask.id == id) else {
            return Delivery::TooLate;
        };
        let ask = &self.live[index];
        if &ask.digest != digest {
            return Delivery::Stale;
        }
        if ask.cannot_answer.is_some() {
            return Delivery::ReadOnly;
        }
        let verdict = match choice {
            Choice::Allow => Verdict {
                decision: Decision::Allow,
                message: None,
                why: None,
            },
            Choice::Deny => Verdict {
                decision: Decision::Deny,
                message: Some(DENIED.to_owned()),
                why: None,
            },
        };
        // A question whose time ran out, or whose hook went, is ended when
        // the connection's notice of it arrives, which is on its way.
        if !ask.feed.push(encode(&Answered { answer: verdict })) {
            return Delivery::TooLate;
        }
        ask.feed.end();
        self.finish(index, How::Answered(choice), Instant::now());
        Delivery::Delivered
    }

    /// Ends every question held in `pane` whose fingerprint is one of
    /// `fingerprints`, as [`Cause::AnsweredInAgent`], and says whether any
    /// was.
    ///
    /// The pane's own output says this — the hook after a tool writes it
    /// where it writes its status — so anything in the pane can say it of
    /// any question there whose fingerprint it knows. All it can do is
    /// withdraw: the hook is told to pass, which leaves the agent's own
    /// dialog the place to answer, as it is for a question the window never
    /// held. A question in another pane is never touched, whatever the
    /// fingerprint, and nothing is answered.
    pub fn answered_in_agent(&mut self, pane: PaneId, fingerprints: &[Fingerprint]) -> bool {
        let answered: Vec<AskId> = self
            .live
            .iter()
            .filter(|ask| {
                ask.pane == pane
                    && fingerprints
                        .iter()
                        .any(|done| done.as_str() == ask.fingerprint)
            })
            .map(|ask| ask.id)
            .collect();
        for &id in &answered {
            self.end(id, Cause::AnsweredInAgent);
        }
        !answered.is_empty()
    }

    /// Takes in a title the pane `pane` was given — `claude`, what Claude
    /// Code's glyph at its front says, see
    /// [`AgentStatus::from_claude_title`] — and ends, as
    /// [`Cause::AtWorkAgain`], the questions there that the title says a
    /// person has answered in the agent's own dialog. Says whether it ended
    /// any.
    ///
    /// Measured on Claude Code 2.1.280 (`docs/answering.md`, C12): its title
    /// turns to `✳` as a permission dialog opens, a few milliseconds before
    /// the hook starts, and stays so for as long as a dialog is up, a
    /// background subagent at work included; its spinner is back within a
    /// few dozen milliseconds of a Yes. So the spinner after the `✳` is the
    /// dialog closing, where the done of [`Self::answered_in_agent`] comes
    /// only once the tool has run, however long it takes.
    ///
    /// After the `✳`, and never on its own. Titles reach the window in the
    /// order Claude Code wrote them, and its spinner turns once a second
    /// while it works; one drawn before the dialog opened, read only once the
    /// hook's question has arrived over the socket, is the agent at work
    /// before it asked and says nothing of an answer. So a question ends only
    /// once the pane's title has been `✳` at or after the moment the window
    /// took it. A spinner from anything else in the pane can do no more than
    /// a done can: the hook passes, and the dialog stays up for the person to
    /// answer.
    ///
    /// The `✳` never ends one: it is the dialog opening, and Esc's turn
    /// ending, which kills the hook and ends its question that way. Only
    /// questions whose row said needs input at once ([`Ask::shows_at_once`]):
    /// inside a multiplexer the pane's title is not the agent's.
    pub fn titled(&mut self, pane: PaneId, claude: Option<AgentStatus>) -> bool {
        if claude == Some(AgentStatus::Idle) {
            self.waiting_titles.insert(pane);
            for ask in self.live.iter_mut().filter(|ask| ask.pane == pane) {
                ask.seen_waiting = true;
            }
            return false;
        }
        // Whatever else the title says, `✳` is no longer the last of them.
        self.waiting_titles.remove(&pane);
        if claude != Some(AgentStatus::Running) {
            return false;
        }
        let answered: Vec<AskId> = self
            .live()
            .filter(|ask| ask.pane == pane && ask.shows_at_once() && ask.seen_waiting)
            .map(|ask| ask.id)
            .collect();
        for &id in &answered {
            self.end(id, Cause::AtWorkAgain);
        }
        !answered.is_empty()
    }

    /// Takes in what a question's connection said happened to it.
    pub fn heard(&mut self, notice: Notice) {
        match notice {
            Notice::Delivered(id) => {
                if let Some(outcome) = self.history.iter_mut().rev().find(|ended| ended.id == id) {
                    outcome.delivered = true;
                }
            }
            Notice::Withdrawn(id) => self.end(id, Cause::HookGone),
            Notice::Expired(id) => self.end(id, Cause::Expired),
            Notice::Undelivered(id) => {
                if let Some(outcome) = self.history.iter_mut().rev().find(|ended| ended.id == id)
                    && matches!(outcome.how, How::Answered(_))
                {
                    outcome.lost = true;
                }
            }
        }
    }

    /// Takes the question at `index` out of those held, says how it ended in
    /// the log, and remembers it.
    fn finish(&mut self, index: usize, how: How, now: Instant) {
        let ask = self.live.remove(index);
        log::info!("{}", log_line(&ask, how, now));
        if ask.cannot_answer.is_none() {
            let (by, decision) = how.told();
            self.tell(
                ask.pane,
                PaneEvent::Answered {
                    pane_id: ask.pane.as_u64(),
                    by,
                    decision,
                },
            );
        }
        if self.history.len() >= HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(Outcome {
            id: ask.id,
            pane: ask.pane,
            asked_at: ask.asked_at,
            ended_at: now,
            tab_title: ask.tab_title,
            agent: ask.agent,
            tool: ask.agent.tool(&ask.request),
            summary: ask.summary,
            how,
            delivered: false,
            lost: false,
        });
    }
}

impl Drop for Asks {
    /// A window that closes tells every hook still waiting that it has gone,
    /// rather than leaving each to hold its agent for the rest of its wait.
    fn drop(&mut self) {
        let now = Instant::now();
        for ask in &self.live {
            ask.feed.end();
            log::info!("{}", log_line(ask, How::Ended(Cause::WindowClosed), now));
        }
    }
}

/// The log's line for a question that ended: its number, the agent, the
/// tool, how it ended, how long it was held and the first twelve hex digits
/// of its digest. Never the command, nor the directory.
pub fn log_line(ask: &Ask, how: How, now: Instant) -> String {
    format!(
        "ask {} {} {}: {} after {} ms (sha256 {})",
        ask.id.0,
        ask.agent.word(),
        ask.tool(),
        how.word(),
        now.saturating_duration_since(ask.asked_at).as_millis(),
        request::hex(&ask.digest[..6]),
    )
}

/// What a question asks, in one line of at most [`SUMMARY_CHARS`]: the tool,
/// and the first line of the command with something on it — or, for a
/// change to a file, the file, relative to `cwd` when it is inside it —
/// written out the way [`request::visible`] writes it and cut the way a
/// row's message is, with a mark at the end when lines follow it.
pub fn summary(tool: &str, request: &AskRequest, cwd: &str) -> String {
    let line = match (request.command(), request.path()) {
        (Some(command), _) => {
            let mut lines = command.split('\n').filter(|line| !line.trim().is_empty());
            let first = lines.next().unwrap_or_default();
            let more = if lines.next().is_some() { " …" } else { "" };
            format!("{tool}: {}{more}", request::visible(first))
        }
        (None, Some(path)) => {
            let shown = request::relative_to(path, cwd).unwrap_or_else(|| path.to_owned());
            format!("{tool}: {}", request::visible(&shown))
        }
        (None, None) => format!("{tool}:"),
    };
    crate::agent::one_line(&line, SUMMARY_CHARS - 1).unwrap_or_else(|| format!("{tool}:"))
}

/// Why a question from `asker` that named `pane` cannot be answered here, or
/// `None` when it can.
///
/// `caller` is the pane the hook's token names, if any; `shell` the pid of
/// the pane's shell; `remote` whether the pane works on another machine.
pub fn cannot_answer(
    caller: Option<PaneId>,
    pane: PaneId,
    asker: &Asker,
    shell: Option<u32>,
    remote: bool,
) -> Option<CannotAnswer> {
    if caller.is_some_and(|caller| caller != pane) {
        return Some(CannotAnswer::OtherTab);
    }
    if !shell.is_some_and(|shell| asker.lineage.contains(&shell)) {
        return Some(CannotAnswer::NotFromThisTab);
    }
    if remote {
        return Some(CannotAnswer::Remote);
    }
    None
}

/// Whether a question from inside a multiplexer from `asker`, bound to a pane
/// on the terminal `pane_tty` whose foreground runs `foreground`, comes from
/// under the tmux whose client that is: some ancestor of the asker — never
/// the asker itself, which is any process that opened the terminal — holds
/// the terminal open and runs that same program.
///
/// tmux's server is that ancestor for every process in its panes. A process
/// anywhere else that names the pane's terminal has none: a program it ran
/// to open the terminal is not the pane's foreground's, and a pane not
/// running tmux has no tmux to hold its terminal.
pub fn from_its_tmux(asker: &Asker, pane_tty: Option<u32>, foreground: Option<Executable>) -> bool {
    let (Some(pane_tty), Some(foreground)) = (pane_tty, foreground) else {
        return false;
    };
    asker
        .holders
        .iter()
        .any(|holder| holder.executable == Some(foreground) && holder.terminals.contains(&pane_tty))
}

/// The program in the foreground of the terminal the shell `shell` is on.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn foreground(shell: u32) -> Option<Executable> {
    crate::agent::holders::foreground(shell)
}

/// Nowhere else is it read, so a question from inside a multiplexer is shown
/// and never answered there.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn foreground(_: u32) -> Option<Executable> {
    None
}

/// Holds an agent's question in the window, or refuses it, and answers with
/// its number.
///
/// In this order, so that a window set not to take questions says nothing
/// about its panes: `declined`, then `no-such-pane` for a terminal no open
/// pane is on, then `busy` for a pane holding as many as it may.
pub fn register(
    workspace: &mut Workspace,
    asked: AgentAsk,
    token: Option<&str>,
    asker: &Asker,
    feed: Option<Arc<Feed>>,
    inbox: &Arc<Inbox>,
    ctx: &mut ViewContext<Workspace>,
) -> Result<Value, Refusal> {
    if !workspace.general().answer_agents_from_crook {
        return Err(Refusal::new(
            code::DECLINED,
            "this window is set not to take agents' questions; answer in the agent's own prompt",
        ));
    }
    let Some(feed) = feed else {
        // Only a kept connection has somewhere to wait for the answer.
        return Err(Refusal::new(
            code::BAD_REQUEST,
            "`agent.ask` waits for its answer over the connection it is asked on",
        ));
    };
    let Some((pane, inside)) = bind(workspace, &asked, ctx) else {
        return Err(Refusal::new(
            code::NO_SUCH_PANE,
            "no pane of this window is on the terminal the question names",
        ));
    };
    let session = workspace.tabs().pane(pane).map(|open| open.session());
    let remote = session.is_some_and(|session| session.remote.is_some());
    let cannot_answer = match inside {
        // Neither the token nor the asker's lineage says anything inside a
        // multiplexer; its ancestors' terminals and programs do. See the
        // module docs.
        Some(_) => {
            let foreground = workspace.shell_pid(pane, ctx).and_then(foreground);
            if remote {
                Some(CannotAnswer::Remote)
            } else if !from_its_tmux(asker, workspace.tty_device_of(pane, ctx), foreground) {
                Some(CannotAnswer::NotFromItsTmux)
            } else {
                None
            }
        }
        None => {
            let caller = token.and_then(|token| workspace.pane_with_token(token, ctx));
            let shell = workspace.shell_pid(pane, ctx);
            cannot_answer(caller, pane, asker, shell, remote)
        }
    };
    let found = Found {
        pane,
        cannot_answer,
        tab_title: session
            .map(|session| super::title(session, workspace.home()))
            .unwrap_or_default(),
        inside,
    };
    // Before the question is held, so before the connection hears it is and
    // can see anything happen to it. A window slower than the connection's
    // patience is handed a question its connection has given up on, which
    // `add` refuses: see its docs.
    feed.tell(Arc::downgrade(inbox));
    let added = workspace.asks_mut().add(asked, found, feed, Instant::now());
    // Refused or not: `add` may have ended questions on its way — one their
    // hook gave up on — and the leads following their pane hear of it now.
    workspace.asks_changed(ctx);
    Ok(encode(&Registered { asked: added?.0 }))
}

/// The pane a question goes to, and where in a multiplexer it was asked
/// when it was: the pane on the terminal it names, or, from inside a
/// multiplexer, the first pane in the panel's order on one of its clients'.
fn bind(
    workspace: &Workspace,
    asked: &AgentAsk,
    app: &AppContext,
) -> Option<(PaneId, Option<Inside>)> {
    match (asked.tty, &asked.multiplexer) {
        (Some(tty), _) => workspace.pane_with_tty(tty, app).map(|pane| (pane, None)),
        (None, Some(multiplexer)) => {
            let holding = workspace.panes_on(&multiplexer.clients, app);
            let &(tab, pane) = holding.first()?;
            let mut others: Vec<_> = holding
                .iter()
                .map(|(other, _)| *other)
                .filter(|other| *other != tab)
                .collect();
            others.dedup();
            Some((
                pane,
                Some(Inside {
                    kind: multiplexer.kind,
                    pane: multiplexer.pane.clone(),
                    elsewhere: others.len(),
                }),
            ))
        }
        (None, None) => None,
    }
}

/// Whether a pane of this window is on one of the terminals `asked` names,
/// for a hook finding the window to ask: `pane.find`'s answer.
///
/// Nothing is held and nothing is said about the pane. Not found while the
/// window is set not to take agents' questions, so that a hook goes on to
/// look for one that would.
pub fn find(workspace: &Workspace, asked: &Find, app: &AppContext) -> Value {
    let found = workspace.general().answer_agents_from_crook
        && !workspace.panes_on(&asked.ttys, app).is_empty();
    encode(&Located { found })
}

/// Takes in what the questions' connections said happened to them, and
/// redraws what showed them.
pub fn heard(workspace: &mut Workspace, notices: &[Notice], ctx: &mut ViewContext<Workspace>) {
    for notice in notices {
        workspace.asks_mut().heard(*notice);
    }
    workspace.asks_changed(ctx);
}

/// Encodes something the wire carries.
fn encode(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("what the wire carries is numbers and strings")
}
