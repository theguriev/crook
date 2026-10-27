//! What crosses the control socket: one JSON object a line, each way.
//!
//! Nothing here touches a socket. A line is read into a [`Request`] or a
//! [`Refusal`], and an answer is written out as a [`Reply`], so that
//! everything a client can get wrong — not JSON, no version, a verb this
//! window has never heard of, an argument a verb does not take — is decided in
//! one place that a test can reach without a connection, and the connection
//! only ever moves bytes.
//!
//! See "The window answers" in `docs/architecture.md` for the shape, and why
//! a verb, once shipped, is only ever added to.
//!
//! # A question a hook asks, and never an answer
//!
//! Nine verbs, and the sixth, `agent.ask`, goes the other way: not a script
//! asking the window something, but an agent's permission hook handing the
//! window the question its agent is about to ask a person — the whole
//! request, normalised — and waiting on the same connection for what the
//! person says. The answer comes back down that connection and no other, so
//! a process that makes a question up is answered about the question it made
//! up and about nothing else. No verb answers: an answer is a person's press
//! on a card in the window, and a verb that any process of this user could
//! send would be a way round the press. `agent.answer` is an unknown verb,
//! and a test holds it to that. See "Answering an agent" in
//! `docs/architecture.md`.
//!
//! The seventh, `pane.find`, is that hook's too, and carries nothing of the
//! question: only the terminals of the tmux clients the agent's tmux has, so
//! that a hook whose `CROOK_SOCKET` is stale can find the window holding one
//! before it tells any window what its agent asks. See [`Find`].
//!
//! The eighth and ninth, `pane.split` and `pane.close`, are `tab.new`'s
//! neighbours: a pane split off the caller's or off one it opened, and a pane
//! it opened closed once nothing in it is working. Neither types into a pane
//! and neither answers anything. See [`SplitPane`] and [`ClosePane`].

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agent::request::{self, AskRequest};

/// The version of the protocol this window speaks, carried on every reply.
pub const VERSION: u64 = 1;

/// The oldest request version this window still answers.
///
/// Equal to [`VERSION`] until a version changes what a field means; then it
/// is what says for how long the old meaning is kept.
pub const MIN_VERSION: u64 = 1;

/// The longest request line a window reads, in bytes, not counting its
/// newline.
///
/// A request is a verb and a handful of arguments. Sixty-four kilobytes is
/// several hundred times that, and it is what bounds the memory a client that
/// never sends a newline can make a connection hold.
pub const MAX_LINE: usize = 64 * 1024;

/// Why a request was not answered, as the `code` of a refusal.
///
/// Strings rather than an enum on the wire, because a client reads the codes
/// of windows newer than itself and must not fail to parse one it has not
/// heard of.
pub mod code {
    /// The line is not a JSON object carrying a version and a verb.
    pub const BAD_REQUEST: &str = "bad-request";
    /// The line is longer than [`MAX_LINE`](super::MAX_LINE).
    pub const TOO_LONG: &str = "too-long";
    /// The verb is not one this window knows.
    pub const UNKNOWN_VERB: &str = "unknown-verb";
    /// The request and the window cannot agree on a version.
    pub const VERSION: &str = "version";
    /// The window is answering as many connections as it takes at once.
    pub const BUSY: &str = "busy";
    /// The window did not answer in the time the connection had for it.
    ///
    /// That time ends a little short of the connection's deadline, not at it,
    /// so that the refusal can still be written: a window that answers in the
    /// last moments before the deadline is refused as this all the same.
    pub const TIMEOUT: &str = "timeout";
    /// The window closed before it answered.
    pub const GONE: &str = "gone";
    /// The request carries no token of a pane in this window, and the verb
    /// is one only a pane may ask.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// The caller already has as many tabs open on its behalf as it may.
    pub const BUDGET: &str = "budget";
    /// The caller has been refused so many times in a row that the window has
    /// stopped answering it, until its pane closes.
    pub const TOO_MANY_REFUSALS: &str = "too-many-refusals";
    /// The shell a new pane runs is one whose quoting Crook has not proven,
    /// so a command cannot be typed into it as the words it was given.
    pub const UNSUPPORTED_SHELL: &str = "unsupported-shell";
    /// The window tried and could not: git refused the worktree, the pane has
    /// no directory to find a repository in.
    pub const FAILED: &str = "failed";
    /// No pane with the number asked about is open in this window: it has
    /// closed, or it never was. A `pane.wait` on a closed tab the caller may
    /// watch is answered instead, with the pane closed.
    pub const NO_SUCH_PANE: &str = "no-such-pane";
    /// The pane asked about is neither the caller nor one it opened, and
    /// watching it needs a grant from the person who opened it — which this
    /// window cannot ask for yet.
    pub const NEEDS_GRANT: &str = "needs-grant";
    /// The pane has no finished command to read or wait for: the first one
    /// is still running, a full-screen program or an agent's TUI has the
    /// pane, or its shell reports no command marks and so never finishes one.
    pub const NO_BLOCKS: &str = "no-blocks";
    /// This window is set not to take agents' questions: the person turned
    /// answering them from Crook off, and the agent's own prompt is the one.
    pub const DECLINED: &str = "declined";
    /// The pane a `pane.close` names is the caller's own, which a request
    /// never closes: a pane that ends itself does so by exiting.
    pub const OWN_PANE: &str = "own-pane";
    /// The pane a `pane.close` names is neither one the caller opened nor
    /// one those opened: only a person closes those.
    pub const NOT_OPENED: &str = "not-opened";
    /// The pane a `pane.close` names is still working — its agent is running
    /// or waiting on a person, its hook is asking, or its shell has a command
    /// open — and closing it would end that. Crook asks a person before a
    /// close ends work, and a request is never that person.
    pub const WORKING: &str = "working";
    /// The pane a `pane.close` names is the one a person is working in: the
    /// active tab's focused pane.
    pub const FOCUSED: &str = "focused";
}

/// The longest a `pane.wait` may wait, in seconds, and how long one that
/// names no timeout does.
///
/// An hour. A wait holds a thread and a place among the connections kept open
/// — see `server::MAX_KEPT` — for as long as it lasts, so it has to end at a
/// time somebody chose rather than when the pane happens to get there; an hour
/// is past any turn an agent takes, and a caller that wants longer asks again,
/// which is also the moment it finds out the pane is still there.
pub const MAX_WAIT_SECS: u64 = 60 * 60;

/// The most finished blocks one `pane.blocks` answers with.
///
/// A hundred is far more than an agent reading what a worker did wants, and
/// it bounds the walk the window makes between two frames to build them.
pub const MAX_BLOCKS: usize = 100;

/// What a request can ask for.
///
/// Nine verbs: one that lists, one that opens a tab, three that watch a
/// pane the caller may watch — itself, and the tabs it opened — one an
/// agent's hook asks its own question with, one it finds the window to ask
/// with, one that splits a pane off the caller's or off one it opened, and
/// one that closes a pane it opened. Every one is a promise that is hard to
/// take back, which is why each is added on purpose and none by a pattern.
/// None of them types into a pane, and none answers an agent's question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verb {
    /// Every pane of the window, as a list of [`PaneEntry`].
    PaneList,
    /// A tab opened beside the caller's, running a command: see [`NewTab`].
    /// Answered with an [`Opened`].
    TabNew(NewTab),
    /// An answer when a pane gets somewhere, or when the time runs out: see
    /// [`Wait`]. Answered with a [`Waited`].
    PaneWait(Wait),
    /// A pane's finished commands, with what they printed: see
    /// [`ReadBlocks`]. Answered with a [`BlocksRead`].
    PaneBlocks(ReadBlocks),
    /// A line for everything that happens to the panes the caller may watch,
    /// for as long as the connection stays open: see [`Follow`]. Answered
    /// with a [`Following`], and then a [`PaneEvent`] a line.
    EventsFollow(Follow),
    /// An agent's permission hook handing the window the question its agent
    /// is asking, and waiting for what a person says: see [`AgentAsk`].
    /// Answered with a [`Registered`], then one [`Answered`] line.
    AgentAsk(AgentAsk),
    /// Whether one of this window's panes is on one of a few terminals: see
    /// [`Find`]. Answered with a [`Located`].
    PaneFind(Find),
    /// A pane split off the caller's own, or off one it opened, running a
    /// command: see [`SplitPane`]. Answered with an [`Opened`].
    PaneSplit(SplitPane),
    /// A pane the caller opened, closed: see [`ClosePane`]. Answered with a
    /// [`PaneClosed`].
    PaneClose(ClosePane),
}

impl Verb {
    /// Every verb's name, in the order a refusal names them.
    pub const NAMES: [&str; 9] = [
        "pane.list",
        "tab.new",
        "pane.wait",
        "pane.blocks",
        "events.follow",
        "agent.ask",
        "pane.find",
        "pane.split",
        "pane.close",
    ];

    /// Its name on the wire: a noun and a verb, joined by a dot.
    pub fn name(&self) -> &'static str {
        match self {
            Self::PaneList => Self::NAMES[0],
            Self::TabNew(_) => Self::NAMES[1],
            Self::PaneWait(_) => Self::NAMES[2],
            Self::PaneBlocks(_) => Self::NAMES[3],
            Self::EventsFollow(_) => Self::NAMES[4],
            Self::AgentAsk(_) => Self::NAMES[5],
            Self::PaneFind(_) => Self::NAMES[6],
            Self::PaneSplit(_) => Self::NAMES[7],
            Self::PaneClose(_) => Self::NAMES[8],
        }
    }

    /// Its arguments on the wire, or `None` for a verb that takes none.
    fn args(&self) -> Option<Value> {
        let encoded = match self {
            Self::PaneList => return None,
            Self::TabNew(asked) => serde_json::to_value(asked),
            Self::PaneWait(asked) => serde_json::to_value(asked),
            Self::PaneBlocks(asked) => serde_json::to_value(asked),
            Self::EventsFollow(asked) => serde_json::to_value(asked),
            Self::AgentAsk(asked) => serde_json::to_value(asked),
            Self::PaneFind(asked) => serde_json::to_value(asked),
            Self::PaneSplit(asked) => serde_json::to_value(asked),
            Self::PaneClose(asked) => serde_json::to_value(asked),
        };
        Some(encoded.expect("a verb's arguments are numbers, strings and booleans"))
    }

    /// Whether the connection is kept for this verb after it is asked: a
    /// wait with time to wait, a stream of events, and an agent's question.
    ///
    /// Such a connection holds a place of its own rather than one of the few
    /// a question is answered in — see `server::MAX_KEPT` and
    /// `server::MAX_ASKS` — and ends early when the client hangs up. A wait
    /// of no time at all is a question like any other.
    pub fn keeps_connection(&self) -> bool {
        match self {
            Self::PaneWait(asked) => !asked.timeout().is_zero(),
            Self::EventsFollow(_) | Self::AgentAsk(_) => true,
            Self::PaneList
            | Self::TabNew(_)
            | Self::PaneBlocks(_)
            | Self::PaneFind(_)
            | Self::PaneSplit(_)
            | Self::PaneClose(_) => false,
        }
    }

    /// How long the window may take over this one, when that is longer than a
    /// connection is otherwise given.
    ///
    /// `None` for everything answered between two frames. A tab in a new
    /// worktree waits for the checkout the worktree menu's creator would
    /// make: git checks a working tree out and runs the repository's own
    /// `post-checkout` hook, which the worktree module lets run for two
    /// minutes, then the checkout is locked and given what `.worktreeinclude`
    /// names. A connection that gave up before all that was done would print
    /// `timeout` about a tab that then opened. So that one is given every
    /// deadline the making runs under — the listing that names the
    /// repository, then `HeldLocks::MAKE_CHECKOUT_BUDGET` — and a margin over
    /// them.
    pub fn patience(&self) -> Option<Duration> {
        match self {
            Self::TabNew(NewTab {
                worktree: Some(_), ..
            }) => Some(
                crate::git::worktree::READ_TIMEOUT
                    + crate::workspace::HeldLocks::MAKE_CHECKOUT_BUDGET
                    + Duration::from_secs(10),
            ),
            // The wait, and time on either side of it: to be asked, and to
            // be asked once more what the pane is doing when the wait runs
            // out — see `server`'s kept connections. A wait of no time is a
            // question like any other.
            Self::PaneWait(asked) if self.keeps_connection() => {
                Some(asked.timeout() + Duration::from_secs(10))
            }
            _ => None,
        }
    }
}

/// What a pane is waited for: one of the four words `crook pane wait
/// --until` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Until {
    /// The agent in it said it is idle — or its command ended after it had
    /// said something else. The idle a pane starts in, before anything has
    /// reported, is not it: a worker whose agent has not started yet has not
    /// finished either.
    Idle,
    /// The agent in it has stopped for a person.
    NeedsInput,
    /// The pane has closed: its shell ended, or somebody closed it.
    Exited,
    /// Nothing is running in it, nothing is waiting to be sent to it, and a
    /// command has finished there — its shell said so with OSC 133 `D`. An
    /// empty line is not a command, though a shell ends one with a `D` too.
    Finished,
}

impl Until {
    /// Every word, in the order a refusal names them.
    pub const WORDS: [&str; 4] = ["idle", "needs-input", "exited", "finished"];

    /// Its word on the wire and on the command line.
    pub fn word(self) -> &'static str {
        match self {
            Self::Idle => Self::WORDS[0],
            Self::NeedsInput => Self::WORDS[1],
            Self::Exited => Self::WORDS[2],
            Self::Finished => Self::WORDS[3],
        }
    }

    /// The condition a word names, if it names one.
    pub fn from_word(word: &str) -> Option<Self> {
        [Self::Idle, Self::NeedsInput, Self::Exited, Self::Finished]
            .into_iter()
            .find(|until| until.word() == word)
    }
}

/// What `pane.wait` asks for: a pane, what to wait for, and for how long.
///
/// Unknown fields refused, as [`NewTab`]'s are, and for the same reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wait {
    /// The pane's number, as `pane.list` gives it.
    pub pane: u64,
    /// What it is waited for.
    pub until: Until,
    /// For how many seconds, at most [`MAX_WAIT_SECS`], which is also what
    /// none means. Zero answers at once with where the pane is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
}

impl Wait {
    /// How long the wait is.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout.unwrap_or(MAX_WAIT_SECS))
    }
}

/// What `pane.wait` answers: whether the pane got there, and where it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waited {
    /// The pane waited on.
    pub pane_id: u64,
    /// What it was waited for.
    pub until: Until,
    /// Whether it got there. `false` when the time ran out first, or when the
    /// pane closed first and was not waited on for that.
    pub reached: bool,
    /// What its agent last said, in `pane.list`'s words; `None` once it has
    /// closed.
    #[serde(default)]
    pub status: Option<String>,
    /// What the agent is waiting for, while it waits, or what went wrong,
    /// while it says it failed.
    #[serde(default)]
    pub message: Option<String>,
    /// For `finished`: the status the command exited with, when the shell
    /// reported one.
    #[serde(default)]
    pub exit: Option<i32>,
    /// Whether the pane has closed.
    pub closed: bool,
}

/// What `pane.blocks` asks for: a pane, and how many of its newest finished
/// commands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadBlocks {
    /// The pane's number, as `pane.list` gives it.
    pub pane: u64,
    /// How many, from 1 to [`MAX_BLOCKS`]; every one the pane holds, up to
    /// that, when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<usize>,
}

/// What `pane.blocks` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlocksRead {
    /// The pane read.
    pub pane_id: u64,
    /// Its newest finished commands, oldest first.
    pub blocks: Vec<BlockEntry>,
    /// Whether something is running in the pane now — a command, a
    /// full-screen program, an agent's TUI — whose output is in none of these
    /// blocks yet.
    pub running: bool,
}

/// One finished command, as `pane.blocks` answers for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEntry {
    /// The command line, as the shell echoed it. Display-only: it came off
    /// the screen, and anything that can print can put anything there.
    #[serde(default)]
    pub command: Option<String>,
    /// The status it exited with, when the shell reported one.
    #[serde(default)]
    pub exit: Option<i32>,
    /// Where the shell said it was when the command started.
    #[serde(default)]
    pub cwd: Option<String>,
    /// How long it ran, in milliseconds, when both ends are known.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// What it printed, as a copy of the block's output would read — its
    /// end, when there was more than an answer carries.
    pub output: String,
    /// Whether `output` is only the end of what it printed.
    pub truncated: bool,
}

/// What `events.follow` asks for: every pane the caller may watch, or one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Follow {
    /// Only this pane, by its number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<u64>,
}

/// The first line `events.follow` answers with, before the events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Following {
    /// The panes followed as of now, in the panel's order. A tab the caller
    /// opens later joins with an `opened` event.
    pub panes: Vec<u64>,
}

/// One line of an `events.follow` stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum PaneEvent {
    /// A pane the caller may watch has opened: a tab it asked for.
    Opened {
        /// The new pane.
        pane_id: u64,
    },
    /// What a pane's agent says has changed.
    Status {
        /// The pane.
        pane_id: u64,
        /// One of the four words `crook --agent` takes.
        status: String,
        /// What it is waiting for, while it waits, or what went wrong, while
        /// it says it failed.
        #[serde(default)]
        message: Option<String>,
    },
    /// A command was seen running in a pane.
    ///
    /// Seen, on a frame the pane's terminal published while it ran: one that
    /// starts and ends between two of them — `true`, `git status` — is never
    /// seen running and is told only by its [`Self::Finished`], which names
    /// it too.
    Started {
        /// The pane.
        pane_id: u64,
        /// The command line, display-only.
        command: String,
    },
    /// A command in a pane finished: its shell said so with OSC 133 `D`.
    Finished {
        /// The pane.
        pane_id: u64,
        /// The command line, display-only; `None` for an empty line or a
        /// cancelled one, which a shell ends with a `D` too.
        #[serde(default)]
        command: Option<String>,
        /// The status it exited with, when the shell reported one.
        #[serde(default)]
        exit: Option<i32>,
        /// How long it ran, in milliseconds, when that is known.
        #[serde(default)]
        duration_ms: Option<u64>,
    },
    /// The agent in a pane has asked a person through Crook: its permission
    /// hook handed the window a question it could answer, and waits.
    ///
    /// Which agent and which tool, and nothing of what it asks: never the
    /// command, the file, the input or the digest. A lead is told that its
    /// worker waits so that it can say so to its person; there is no verb
    /// that answers, and the question is only ever answered on the card.
    Asked {
        /// The pane.
        pane_id: u64,
        /// The agent, as `crook --agent ask` names it: `claude`.
        agent: String,
        /// The agent's own name for the tool it asks to use: `Bash`, `Edit`,
        /// `MultiEdit` or `Write`.
        tool: String,
    },
    /// A question a pane's agent asked through Crook is over: see
    /// [`AnsweredBy`] for how. Told for every question told as
    /// [`Self::Asked`], once, and never with what it asked.
    ///
    /// Never that the agent did what it was told: a hook's allow is one the
    /// agent may still drop, and only its next status says what it did.
    Answered {
        /// The pane.
        pane_id: u64,
        /// How it ended.
        by: AnsweredBy,
        /// What a person sent, when it was answered [`AnsweredBy::Here`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        decision: Option<Decision>,
    },
    /// A pane closed. When it is the pane the stream was asked about — the
    /// caller's own, or the one named — it is the last line.
    Closed {
        /// The pane.
        pane_id: u64,
    },
    /// The reader fell behind and this many events were dropped, the oldest
    /// first, rather than kept for it without bound.
    Lagged {
        /// How many.
        dropped: u64,
    },
}

/// How a question a lead was told of with [`PaneEvent::Asked`] ended, in
/// one of six words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnsweredBy {
    /// A person answered it on its card in Crook: the event's `decision`
    /// says which way.
    Here,
    /// A person answered it in the agent's own dialog: the tool it was about
    /// has run.
    InAgent,
    /// The window let it go to the agent's own dialog unanswered: answering
    /// from Crook was turned off.
    Released,
    /// Its time ran out, and the agent's own dialog is the one left.
    Expired,
    /// The agent stopped asking: its hook went, its turn or its command
    /// ended, or it asked a newer question in its place.
    Withdrawn,
    /// Its pane, or the window, closed.
    Closed,
}

impl PaneEvent {
    /// The event as it is written: one line of JSON and its newline.
    pub fn line(&self) -> String {
        let mut line =
            serde_json::to_string(self).expect("an event is numbers and strings, which encode");
        line.push('\n');
        line
    }
}

/// What `tab.new` asks for: a command, and where to run it.
///
/// Read with unknown fields refused rather than ignored. A newer client's
/// argument this window has never heard of could change what the tab is —
/// ignoring a `worktree` would open the agent in the caller's own checkout —
/// and a refusal naming it is the answer that cannot do harm.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewTab {
    /// The command, as the words it is made of. The window joins them for
    /// the new pane's shell, quoting each, so every word arrives as itself.
    pub command: Vec<String>,
    /// A branch to make a worktree on, from the caller's `HEAD`, for the tab
    /// to open in; the caller's own directory when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// Whether the tab joins the caller's group, making one of the two when
    /// the caller's tab is in none.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_my_group: bool,
    /// What to call the tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Where the tab's shell starts, in place of the caller's directory: an
    /// absolute path to a directory. Not beside a `worktree`, which says
    /// where the tab starts on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Which side of the pane it is split off a new pane goes: the two a
/// person's split keys open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SplitSide {
    /// To its right, in a row.
    #[default]
    Right,
    /// Under it, in a column.
    Down,
}

impl SplitSide {
    /// Every word, in the order a refusal names them.
    pub const WORDS: [&str; 2] = ["right", "down"];

    /// Its word on the wire and on the command line.
    pub fn word(self) -> &'static str {
        match self {
            Self::Right => Self::WORDS[0],
            Self::Down => Self::WORDS[1],
        }
    }

    /// The side a word names, if it names one.
    pub fn from_word(word: &str) -> Option<Self> {
        [Self::Right, Self::Down]
            .into_iter()
            .find(|side| side.word() == word)
    }
}

/// What `pane.split` asks for: a command, and the pane to split it off.
///
/// Unknown fields refused, as [`NewTab`]'s are, and for its reason.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitPane {
    /// The command, as the words it is made of, joined and quoted as
    /// [`NewTab::command`] is.
    pub command: Vec<String>,
    /// Which side of `beside` the new pane goes.
    #[serde(default)]
    pub direction: SplitSide,
    /// The pane to split it off: the caller's own when there is none, or a
    /// pane it opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beside: Option<u64>,
    /// What to call the pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Where its shell starts, in place of where `beside` is working: an
    /// absolute path to a directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// What `pane.close` asks for: a pane the caller opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosePane {
    /// The pane's number, as `pane.list` gives it.
    pub pane: u64,
}

/// What `pane.close` answers: the pane has closed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneClosed {
    /// The pane that closed.
    pub pane_id: u64,
    /// Whether its tab closed with it: it was the tab's last pane.
    pub tab_closed: bool,
}

/// What `tab.new` answers: the tab that opened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opened {
    /// The new pane's number: the `CROOK_PANE_ID` its shell has, and the
    /// `pane_id` `pane.list` lists it by.
    pub pane_id: u64,
    /// The number of the tab holding it.
    pub tab_id: u64,
    /// Where its shell starts: the worktree, when one was made.
    #[serde(default)]
    pub cwd: Option<String>,
    /// A sentence about the tab that the numbers cannot say, when there is
    /// one: what the main checkout's `.worktreeinclude` named that the new
    /// worktree was not given, and why.
    ///
    /// Left out when there is nothing to say, so an answer is the same three
    /// fields it always was unless something did not arrive; and read with
    /// a default, so a `crook` this new reads an older window's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The longest an agent's question may be held, in seconds.
///
/// Ten minutes: past the five a Claude Code permission hook is given, with
/// room for an agent that gives its hooks longer, and short enough that a
/// question nobody looks at gives its place back.
pub const MAX_ASK_WAIT_SECS: u64 = 600;

/// The longest working directory a question carries, in bytes: `PATH_MAX`
/// on Linux, past macOS's.
pub const MAX_CWD: usize = 4096;

/// The agents whose hooks may ask: one so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AskingAgent {
    /// Claude Code, through its `PermissionRequest` hook.
    Claude,
}

impl AskingAgent {
    /// The word on the wire and after `crook --agent ask`.
    pub fn word(self) -> &'static str {
        match self {
            Self::Claude => "claude",
        }
    }

    /// What a person calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
        }
    }

    /// The agent's own name for the tool a request is for, which is what a
    /// question's digest is taken over.
    ///
    /// Read off the request, so that the hook after the tool, which reads
    /// the same input into the same request, names it the same: a
    /// `MultiEdit` of one edit is the same change as an `Edit`, and is
    /// called one.
    pub fn tool(self, request: &AskRequest) -> &'static str {
        match (self, request) {
            (Self::Claude, AskRequest::Command { .. }) => "Bash",
            (Self::Claude, AskRequest::Edit { edits, .. }) if edits.len() == 1 => "Edit",
            (Self::Claude, AskRequest::Edit { .. }) => "MultiEdit",
            (Self::Claude, AskRequest::Write { .. }) => "Write",
        }
    }
}

/// What `agent.ask` carries: the question an agent's permission hook was
/// handed, normalised, and where it was asked from.
///
/// Unknown fields refused, as every verb's are, and here most of all: a
/// field this window does not know is one it would not show, and a person
/// must not be asked to allow less than will happen.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAsk {
    /// Which agent asks.
    pub agent: AskingAgent,
    /// The agent's session, as its hook was handed it: what a newer question
    /// from the same session replaces an older one by.
    pub session: String,
    /// The directory the agent works in, absolute.
    pub cwd: String,
    /// The device number of the terminal a status from the hook would be
    /// written to: the hook's own controlling terminal, or the agent's. What
    /// binds the question to the pane whose pty that is.
    ///
    /// Left out for a question from inside a multiplexer, which names the
    /// terminals of its clients instead: there the agent's terminal is one
    /// of the multiplexer's own, which no pane is on. A window from before
    /// [`Self::multiplexer`] refuses a question without it, and its hook
    /// passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<u32>,
    /// For a question from inside tmux, in place of [`Self::tty`]: the
    /// terminals of the clients attached to the agent's session, one of
    /// which a pane of the window is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplexer: Option<Multiplexer>,
    /// How long the hook waits for an answer, in seconds, from 1 to
    /// [`MAX_ASK_WAIT_SECS`].
    pub wait: u64,
    /// What the agent asks to do.
    pub request: AskRequest,
}

impl AgentAsk {
    /// How long the hook waits.
    pub fn wait(&self) -> Duration {
        Duration::from_secs(self.wait)
    }

    /// The agent's name for the tool the request is for.
    pub fn tool(&self) -> &'static str {
        self.agent.tool(&self.request)
    }

    /// The question's digest: see [`request::digest`].
    pub fn digest(&self) -> [u8; 32] {
        request::digest(&self.session, self.tool(), &self.request)
    }

    /// Why the window will not take this question, or `None` when it will.
    fn fault(&self) -> Option<String> {
        if crook_terminal::agent::session_id(&self.session).is_none() {
            return Some(format!(
                "`session` is one word of at most {} bytes",
                crook_terminal::agent::SESSION_BYTES
            ));
        }
        if !self.cwd.starts_with('/') || self.cwd.len() > MAX_CWD || self.cwd.contains('\0') {
            return Some(format!(
                "`cwd` is an absolute path of at most {MAX_CWD} bytes"
            ));
        }
        if !(1..=MAX_ASK_WAIT_SECS).contains(&self.wait) {
            return Some(format!("`wait` is from 1 to {MAX_ASK_WAIT_SECS} seconds"));
        }
        match (&self.tty, &self.multiplexer) {
            (Some(_), None) => {}
            (None, Some(multiplexer)) => {
                if let Some(fault) = multiplexer.fault() {
                    return Some(fault);
                }
            }
            _ => return Some("a question names exactly one of `tty` and `multiplexer`".to_owned()),
        }
        self.request
            .fault()
            .map(|fault| format!("`request` cannot be shown whole: {fault}"))
    }
}

/// The agent, the kind of request and the first twelve hex digits of its
/// digest — never the command, nor the directory: a request is printed in
/// logs and panics, and what an agent asks to run stays between the agent,
/// its hook and the person asked.
impl fmt::Debug for AgentAsk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentAsk")
            .field("agent", &self.agent)
            .field("kind", &self.request.kind())
            .field("sha256", &request::hex(&self.digest()[..6]))
            .finish_non_exhaustive()
    }
}

/// The most terminals a question from inside a multiplexer names, and a
/// `pane.find` asks about.
///
/// Eight: a person attaches to a session from a few terminals at most, and
/// this bounds the walk of the panes a window makes for one.
pub const MAX_CLIENTS: usize = 8;

/// The longest tmux pane id a question carries, in bytes: `%` and the
/// pane's number.
pub const MAX_PANE_ID: usize = 12;

/// The multiplexers a question can come from inside: one so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MultiplexerKind {
    /// tmux, whose clients' terminals it can be asked for.
    Tmux,
}

impl MultiplexerKind {
    /// Its word on the wire, and what a person calls it.
    pub fn word(self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
        }
    }
}

/// Where a question from inside a multiplexer was asked: the terminals of
/// the multiplexer's clients, which bind it, and the multiplexer's own name
/// for the agent's pane, which is only shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Multiplexer {
    /// Which multiplexer.
    pub kind: MultiplexerKind,
    /// The device numbers of its clients' terminals, from 1 to
    /// [`MAX_CLIENTS`] of them: the question goes to the first pane of the
    /// window, in the panel's order, that is on one.
    pub clients: Vec<u32>,
    /// The agent's pane, as tmux names it — `%` and a number. Shown, and
    /// never what binds anything.
    pub pane: String,
}

impl Multiplexer {
    /// Why the window will not take this, or `None` when it will.
    fn fault(&self) -> Option<String> {
        if !(1..=MAX_CLIENTS).contains(&self.clients.len()) {
            return Some(format!(
                "`multiplexer.clients` names from 1 to {MAX_CLIENTS} terminals"
            ));
        }
        if !is_pane_id(&self.pane) {
            return Some(format!(
                "`multiplexer.pane` is `%` and a number, at most {MAX_PANE_ID} bytes"
            ));
        }
        None
    }
}

/// Whether `pane` is a tmux pane id: `%` and at least one digit, and
/// nothing else, so that what the card shows of it is never more than that.
pub fn is_pane_id(pane: &str) -> bool {
    pane.len() <= MAX_PANE_ID
        && pane.strip_prefix('%').is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// What `pane.find` asks: whether a pane of this window is on one of these
/// terminals.
///
/// Nothing else, and no token: a hook asks it of windows that may not hold
/// its agent's pane, so that only the one that does is ever told what the
/// agent asks. Unknown fields refused, so that nothing more can ride on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Find {
    /// The device numbers of the terminals, from 1 to [`MAX_CLIENTS`].
    pub ttys: Vec<u32>,
}

/// What `pane.find` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Located {
    /// Whether a pane of this window is on one of the terminals, and the
    /// window would take a question about it.
    pub found: bool,
}

/// What `agent.ask` answers first: the question is held, under this number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    /// The question's number in this window, never given to another.
    pub asked: u64,
}

/// What a person said to an agent's question, or that nobody will.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    /// Run it, this once.
    Allow,
    /// Do not run it.
    Deny,
    /// Nothing: the agent's own prompt is the one that answers.
    Pass,
}

/// The decision and what goes with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verdict {
    /// What was decided.
    pub decision: Decision,
    /// What a deny tells the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Why a pass is one, in a word: `expired`, `asked-again`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// The one line `agent.ask` answers with after [`Registered`].
///
/// Read with unknown fields refused at every level, so that a hook this old
/// never acts on an answer a newer window meant more by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answered {
    /// What was said.
    pub answer: Verdict,
}

impl Answered {
    /// A pass, and why.
    pub fn pass(why: &str) -> Self {
        Self {
            answer: Verdict {
                decision: Decision::Pass,
                message: None,
                why: Some(why.to_owned()),
            },
        }
    }

    /// The answer as it is written: one line of JSON and its newline.
    pub fn line(&self) -> String {
        let mut line =
            serde_json::to_string(self).expect("an answer is words and strings, which encode");
        line.push('\n');
        line
    }
}

/// The one line a hook sends after its answer: it has printed it, for its
/// agent to read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivered {
    /// Always `true`.
    pub delivered: bool,
}

/// Who connected to ask, as the kernel says rather than as the request does.
///
/// Filled in by the connection, for a question only, and never read off the
/// wire: what binds a question to a pane is the terminal it names, and this
/// is what says whether the asker is a process of that pane's at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asker {
    /// The process at the other end of the connection, when the kernel
    /// names one.
    pub pid: Option<u32>,
    /// That process and its ancestors, nearest first.
    pub lineage: Vec<u32>,
    /// For a question from inside a multiplexer, its ancestors — not itself
    /// — as far as they were read: which program each runs and which
    /// terminals each holds open. What says whether the multiplexer whose
    /// client is on the pane is one the asker runs under. Empty for every
    /// other question.
    pub holders: Vec<Holder>,
}

/// A program file, as the kernel names it: the device and inode of the
/// executable a process runs, which a name or a path could only claim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Executable {
    /// The device the file is on.
    pub device: u64,
    /// The file's inode there.
    pub inode: u64,
}

/// One ancestor of a process that asked, as the kernel says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holder {
    /// Its pid.
    pub pid: u32,
    /// The program it runs, when that could be read.
    pub executable: Option<Executable>,
    /// The device numbers of the terminals it holds open, low 32 bits, as
    /// many as were read.
    pub terminals: Vec<u32>,
}

/// One request: what it asks, and who asks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// What it asks.
    pub verb: Verb,
    /// The asking pane's `CROOK_TOKEN`, when it sent one. Whose it is, if it
    /// is anybody's, is the window's to say: see `control::spawn`.
    pub token: Option<String>,
    /// Who connected, for an agent's question: see [`Asker`]. `None` off
    /// the wire, always.
    pub asker: Option<Asker>,
}

/// Why a request was not answered: a code a script can branch on, and a
/// sentence a person can read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// One of [`code`]'s, or one a newer window added.
    pub code: String,
    /// What went wrong, written for whoever typed the command.
    pub message: String,
}

impl Refusal {
    /// A refusal with this code and this sentence.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

/// One answer, as it goes back down the connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// The version the window speaks. On every reply, refusals included, so a
    /// client can tell which window it reached before it reads anything else.
    pub v: u64,
    /// The request's own `id`, echoed, when it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Whether this is an answer rather than a refusal.
    pub ok: bool,
    /// The answer, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The refusal, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Refusal>,
}

impl Reply {
    /// The answer to the request `id` named.
    pub fn answered(id: Option<Value>, result: Value) -> Self {
        Self {
            v: VERSION,
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// The refusal of the request `id` named.
    pub fn refused(id: Option<Value>, refusal: Refusal) -> Self {
        Self {
            v: VERSION,
            id,
            ok: false,
            result: None,
            error: Some(refusal),
        }
    }

    /// The reply as it is written: one line of JSON and its newline.
    pub fn line(&self) -> String {
        let mut line =
            serde_json::to_string(self).expect("a reply is JSON values and strings, which encode");
        line.push('\n');
        line
    }
}

/// One pane, as `pane.list` answers for it.
///
/// What the panel shows about the pane and nothing it does not: no output, no
/// input, nothing a person would have to be asked about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneEntry {
    /// The pane's number: the same one its shell has in `CROOK_PANE_ID`, so a
    /// program in a pane can find its own entry.
    pub pane_id: u64,
    /// The number of the tab holding it. Two entries with one are the two
    /// halves of a split.
    pub tab_id: u64,
    /// What the pane's row is called: what somebody named it, else what the
    /// program in it calls its work, else what it is running, else its
    /// directory — the rule the row itself follows.
    pub title: String,
    /// What the tab's row is called when a row stands for the whole tab: its
    /// focused pane's title.
    pub tab_title: String,
    /// The group the tab is in, by the name on its heading.
    #[serde(default)]
    pub group: Option<String>,
    /// Whether this is the pane a person is working in: the active tab's
    /// focused pane. At most one entry says so.
    pub focused: bool,
    /// What the agent in it last said: `idle`, `running`, `needs-input` or
    /// `failed` — the four words `crook --agent` takes.
    pub status: String,
    /// What the agent said it is waiting for, while it waits, or what it
    /// said went wrong, while it says it failed.
    #[serde(default)]
    pub message: Option<String>,
    /// Where the pane is working, as its shell last reported it — on this
    /// machine: `None` for a pane whose shell said it is on another one,
    /// since the path there names some other directory here, or none.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The branch there, or the short sha of a detached head, once git has
    /// been asked.
    #[serde(default)]
    pub branch: Option<String>,
    /// Whether the agent in it is asking a person through Crook right now:
    /// its permission hook is waiting on a question this window could
    /// answer. Only that: what it asks is the person's to read on the card.
    ///
    /// Left out while it is not, so an entry is what it always was until a
    /// question is held; and read with a default, so a `crook` this new
    /// reads an older window's list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asking: bool,
}

/// The line a client sends to ask `verb`, carrying `token` when it has one.
pub fn request_line(verb: &Verb, token: Option<&str>) -> String {
    let mut request = Map::new();
    request.insert("v".to_owned(), VERSION.into());
    request.insert("verb".to_owned(), verb.name().into());
    if let Some(args) = verb.args() {
        request.insert("args".to_owned(), args);
    }
    if let Some(token) = token {
        request.insert("token".to_owned(), token.into());
    }
    format!("{}\n", Value::Object(request))
}

/// Reads one request line into what it asks for.
///
/// Either way the request's `id` comes back with the answer, when the line was
/// an object that had one, so that a refusal can be matched to its request as
/// surely as an answer can.
pub fn read(line: &[u8]) -> (Option<Value>, Result<Request, Refusal>) {
    let Ok(Value::Object(request)) = serde_json::from_slice::<Value>(line) else {
        return (None, Err(malformed("the line is not a JSON object")));
    };
    let id = request.get("id").cloned();
    (id, request_of(&request))
}

/// What a request object asks for, once its versions agree with this
/// window's.
fn request_of(request: &Map<String, Value>) -> Result<Request, Refusal> {
    let Some(version) = request.get("v").and_then(Value::as_u64) else {
        return Err(malformed("a request carries its protocol version as `v`"));
    };
    if version < MIN_VERSION {
        return Err(Refusal::new(
            code::VERSION,
            format!(
                "this Crook answers protocol versions {MIN_VERSION} to {VERSION}, and the \
                 request was written for {version}"
            ),
        ));
    }
    // Newer than this window is fine on its own: versions add, so a request
    // written later still means what it says here, unless it says it needs
    // more than this window has.
    match request.get("min_version") {
        None => {}
        Some(needed) => match needed.as_u64() {
            Some(needed) if needed <= VERSION => {}
            Some(needed) => {
                return Err(Refusal::new(
                    code::VERSION,
                    format!(
                        "this Crook speaks protocol version {VERSION}, and the request needs \
                         {needed}; update Crook"
                    ),
                ));
            }
            None => return Err(malformed("`min_version` is a whole number")),
        },
    }

    let token = match request.get("token") {
        None => None,
        Some(Value::String(token)) => Some(token.clone()),
        Some(_) => return Err(malformed("`token` is a string")),
    };
    let Some(name) = request.get("verb").and_then(Value::as_str) else {
        return Err(malformed("a request names its `verb`"));
    };
    let args = request.get("args");
    let verb = match name {
        "pane.list" => match args {
            None => Verb::PaneList,
            Some(Value::Object(args)) if args.is_empty() => Verb::PaneList,
            Some(_) => return Err(malformed("`pane.list` takes no `args`")),
        },
        "tab.new" => {
            let Some(args) = args else {
                return Err(malformed("`tab.new` needs `args` naming its `command`"));
            };
            let asked = NewTab::deserialize(args)
                .map_err(|error| malformed(&format!("`tab.new`'s args: {error}")))?;
            Verb::TabNew(asked)
        }
        "pane.wait" => {
            let Some(args) = args else {
                return Err(malformed(
                    "`pane.wait` needs `args` naming its `pane` and what it waits `until`",
                ));
            };
            let asked = Wait::deserialize(args)
                .map_err(|error| malformed(&format!("`pane.wait`'s args: {error}")))?;
            if asked.timeout.is_some_and(|timeout| timeout > MAX_WAIT_SECS) {
                return Err(Refusal::new(
                    code::BAD_REQUEST,
                    format!("a wait is at most {MAX_WAIT_SECS} seconds; wait again after it"),
                ));
            }
            Verb::PaneWait(asked)
        }
        "pane.blocks" => {
            let Some(args) = args else {
                return Err(malformed("`pane.blocks` needs `args` naming its `pane`"));
            };
            let asked = ReadBlocks::deserialize(args)
                .map_err(|error| malformed(&format!("`pane.blocks`'s args: {error}")))?;
            if asked
                .last
                .is_some_and(|last| last == 0 || last > MAX_BLOCKS)
            {
                return Err(Refusal::new(
                    code::BAD_REQUEST,
                    format!("`last` is from 1 to {MAX_BLOCKS}"),
                ));
            }
            Verb::PaneBlocks(asked)
        }
        "events.follow" => match args {
            None => Verb::EventsFollow(Follow::default()),
            Some(args) => Verb::EventsFollow(
                Follow::deserialize(args)
                    .map_err(|error| malformed(&format!("`events.follow`'s args: {error}")))?,
            ),
        },
        "agent.ask" => {
            let Some(args) = args else {
                return Err(malformed(
                    "`agent.ask` needs `args` naming its `agent`, `session`, `cwd`, `tty` or \
                     `multiplexer`, `wait` and `request`",
                ));
            };
            let asked = AgentAsk::deserialize(args)
                .map_err(|error| malformed(&format!("`agent.ask`'s args: {error}")))?;
            if let Some(fault) = asked.fault() {
                return Err(Refusal::new(code::BAD_REQUEST, fault));
            }
            Verb::AgentAsk(asked)
        }
        "pane.find" => {
            let Some(args) = args else {
                return Err(malformed("`pane.find` needs `args` naming its `ttys`"));
            };
            let asked = Find::deserialize(args)
                .map_err(|error| malformed(&format!("`pane.find`'s args: {error}")))?;
            if !(1..=MAX_CLIENTS).contains(&asked.ttys.len()) {
                return Err(Refusal::new(
                    code::BAD_REQUEST,
                    format!("`ttys` names from 1 to {MAX_CLIENTS} terminals"),
                ));
            }
            Verb::PaneFind(asked)
        }
        "pane.split" => {
            let Some(args) = args else {
                return Err(malformed("`pane.split` needs `args` naming its `command`"));
            };
            let asked = SplitPane::deserialize(args)
                .map_err(|error| malformed(&format!("`pane.split`'s args: {error}")))?;
            Verb::PaneSplit(asked)
        }
        "pane.close" => {
            let Some(args) = args else {
                return Err(malformed("`pane.close` needs `args` naming its `pane`"));
            };
            let asked = ClosePane::deserialize(args)
                .map_err(|error| malformed(&format!("`pane.close`'s args: {error}")))?;
            Verb::PaneClose(asked)
        }
        name => {
            return Err(Refusal::new(
                code::UNKNOWN_VERB,
                format!(
                    "this Crook does not know the verb {name:?}; it knows {}",
                    Verb::NAMES.join(", ")
                ),
            ));
        }
    };
    Ok(Request {
        verb,
        token,
        asker: None,
    })
}

/// A `bad-request` refusal that says what a request looks like.
fn malformed(what: &str) -> Refusal {
    Refusal::new(
        code::BAD_REQUEST,
        format!(
            "{what}: a request is one JSON object a line, like {{\"v\":1,\"verb\":\"pane.list\"}}"
        ),
    )
}
