//! `agent.ask`: the questions agents' permission hooks are waiting on.
//!
//! When Claude Code is about to ask a person whether it may run a command, it
//! runs its `PermissionRequest` hook first, and the hook — `crook --agent ask
//! claude`, see [`crate::agent::ask`] — hands the window the whole request
//! and waits on its connection for an answer. This is the window's side: the
//! questions held, which pane each belongs to, and what happened to each.
//! Nothing here answers one. An answer is a person's press on a card the
//! window draws, which is the next piece of work; until then every question
//! ends with a pass, and the agent's own prompt — open beside the question
//! the whole time — is the one a person answers.
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
//! # How a question ends
//!
//! With the hook going ([`Cause::HookGone`], which the connection tells the
//! window through a [`Notice`] the moment it sees it), its time running out,
//! its pane closing, the agent's command ending, the agent saying its turn is
//! over or failed, the same session asking again, or the window closing.
//! Each but the last pushes a pass into the question's feed, which the
//! connection writes and the hook prints nothing for; a window that closes
//! says `gone` instead. Each is a line in the log — the agent, the tool, how
//! it ended and the first twelve hex digits of its digest, never the command
//! — and an entry in [`Asks::history`], in memory only.
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

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crookui_core::prelude::*;
use serde_json::Value;

use super::protocol::{AgentAsk, Answered, Asker, AskingAgent, Refusal, Registered, code};
#[cfg(test)]
use super::protocol::{Decision, Verdict};
use super::watch::Feed;
use super::{Inbox, Notice};
use crate::agent::request::{self, AskRequest};
use crate::tab::{PaneId, TabStrip};
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
#[cfg(test)]
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
    /// The window closed.
    WindowClosed,
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
            Self::WindowClosed => "window-closed",
        }
    }
}

/// What a person can answer a question with.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Run it, this once.
    Allow,
    /// Do not run it.
    Deny,
}

/// What became of an answer handed to [`Asks::deliver`].
#[cfg(test)]
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
    #[cfg(test)]
    Answered(Choice),
    /// It ended without an answer.
    Ended(Cause),
}

impl How {
    /// The word the log says.
    pub fn word(self) -> &'static str {
        match self {
            #[cfg(test)]
            Self::Answered(Choice::Allow) => "allowed",
            #[cfg(test)]
            Self::Answered(Choice::Deny) => "denied",
            Self::Ended(cause) => cause.word(),
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
    /// What it asks to do, whole.
    pub request: AskRequest,
    /// What it asks in one line: see [`summary`].
    pub summary: String,
    /// The digest of the session, the tool and the request: what a press
    /// carries, so an answer can only reach the question it was given about.
    pub digest: [u8; 32],
    /// The digest's first half in hex: see [`request::fingerprint`].
    #[cfg(test)]
    pub fingerprint: String,
    /// The pane whose terminal it named.
    pub pane: PaneId,
    /// Why it can be shown and not answered, when it cannot.
    pub cannot_answer: Option<CannotAnswer>,
    /// When the window took it.
    pub asked_at: Instant,
    /// What the pane's row was called when it asked.
    tab_title: String,
    /// What the hook's connection waits on.
    feed: Arc<Feed>,
}

impl Ask {
    /// The agent's name for the tool it asks to use.
    pub fn tool(&self) -> &'static str {
        self.agent.tool(&self.request)
    }
}

/// One question that has ended, as the window remembers it: what it was
/// about in a line, and how it went. Never the request itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Its number.
    pub id: AskId,
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
    pub fn answerable_in(&self, pane: PaneId) -> Option<&Ask> {
        self.live()
            .filter(|ask| ask.pane == pane && ask.cannot_answer.is_none())
            .max_by_key(|ask| ask.id)
    }

    /// How the questions that have ended went, oldest first.
    #[cfg(test)]
    pub fn history(&self) -> impl Iterator<Item = &Outcome> {
        self.history.iter()
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
        let alike = |ask: &Ask| ask.pane == found.pane && ask.cannot_answer.is_none() == answerable;
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
        // well as its own; a read-only one replaces only read-only ones.
        let replaced: Vec<AskId> = self
            .live
            .iter()
            .filter(|ask| ask.pane == found.pane && ask.session == asked.session)
            .filter(|ask| answerable || ask.cannot_answer.is_some())
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
            request,
            ..
        } = asked;
        self.live.push(Ask {
            id,
            agent,
            summary: summary(agent.tool(&request), &request),
            #[cfg(test)]
            fingerprint: request::fingerprint(&digest),
            digest,
            session,
            request,
            pane: found.pane,
            cannot_answer: found.cannot_answer,
            asked_at: now,
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
    #[cfg(test)]
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
        }
    }

    /// Takes the question at `index` out of those held, says how it ended in
    /// the log, and remembers it.
    fn finish(&mut self, index: usize, how: How, now: Instant) {
        let ask = self.live.remove(index);
        log::info!("{}", log_line(&ask, how, now));
        if self.history.len() >= HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(Outcome {
            id: ask.id,
            asked_at: ask.asked_at,
            ended_at: now,
            tab_title: ask.tab_title,
            agent: ask.agent,
            tool: ask.agent.tool(&ask.request),
            summary: ask.summary,
            how,
            delivered: false,
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
/// and the first line of the command with something on it, written out the
/// way [`request::visible`] writes it and cut the way a row's message is,
/// with a mark at the end when lines follow it.
pub fn summary(tool: &str, request: &AskRequest) -> String {
    let mut lines = request
        .command()
        .split('\n')
        .filter(|line| !line.trim().is_empty());
    let first = lines.next().unwrap_or_default();
    let more = if lines.next().is_some() { " …" } else { "" };
    let line = format!("{tool}: {}{more}", request::visible(first));
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
    let Some(pane) = workspace.pane_with_tty(asked.tty, ctx) else {
        return Err(Refusal::new(
            code::NO_SUCH_PANE,
            "no pane of this window is on the terminal the question names",
        ));
    };
    let caller = token.and_then(|token| workspace.pane_with_token(token, ctx));
    let shell = workspace.shell_pid(pane, ctx);
    let session = workspace.tabs().pane(pane).map(|open| open.session());
    let found = Found {
        pane,
        cannot_answer: cannot_answer(
            caller,
            pane,
            asker,
            shell,
            session.is_some_and(|session| session.remote.is_some()),
        ),
        tab_title: session
            .map(|session| super::title(session, workspace.home()))
            .unwrap_or_default(),
    };
    // Before the question is held, so before the connection hears it is and
    // can see anything happen to it. A window slower than the connection's
    // patience is handed a question its connection has given up on, which
    // `add` refuses: see its docs.
    feed.tell(Arc::downgrade(inbox));
    let id = workspace
        .asks_mut()
        .add(asked, found, feed, Instant::now())?;
    ctx.notify();
    Ok(encode(&Registered { asked: id.0 }))
}

/// Takes in what the questions' connections said happened to them, and
/// redraws what showed them.
pub fn heard(workspace: &mut Workspace, notices: &[Notice], ctx: &mut ViewContext<Workspace>) {
    for notice in notices {
        workspace.asks_mut().heard(*notice);
    }
    ctx.notify();
}

/// Encodes something the wire carries.
fn encode(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("what the wire carries is numbers and strings")
}
