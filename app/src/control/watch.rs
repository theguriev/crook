//! `pane.wait` and `events.follow`: watching a pane from outside.
//!
//! What a lead agent that fanned its work out into tabs does next: waits for
//! a worker to stop, and reads what it did. `crook pane wait 7 --until
//! needs-input --timeout 600` answers when pane 7's agent has stopped for a
//! person, or when the ten minutes are up, and says which; `crook events
//! --follow` is a line for every status, every command finished — named, as
//! one too quick to be seen running is told only there — and every pane
//! closed, for as long as it is left running.
//!
//! # Who may watch what
//!
//! A caller — the pane whose `CROOK_TOKEN` the request carries — may watch
//! itself and the tabs it opened, and theirs in turn: what [`observes`] says.
//! Watching a pane a person opened, or one another pane's agent did, is
//! refused as `needs-grant` with a sentence saying so. The grant is the
//! person's to give on a card, and this window cannot ask for one yet; until
//! it can, the answer is no rather than yes, since a pane's output is what an
//! agent reading it would take instructions from.
//!
//! A tab it may watch that has closed — one it opened, or one of theirs — is
//! still its own to wait on, for as long as the window remembers who opened
//! it — see [`REMEMBERED_CLOSED`] — and the answer is that it has closed.
//!
//! # Where the waiting happens
//!
//! Never on the window's thread. The connection's own thread blocks, on a
//! [`Feed`] it made before it asked: the window registers the feed here,
//! beside what the request asked for, and pushes into it from where it
//! already applies what a pane's shell did — the agent's report, a command
//! starting, a command finishing, the pane closing — see
//! `Workspace::apply_terminal_update`. Pushing is a lock and a notify; no
//! poll, no timer, and nothing done at all while nobody is watching, but for
//! noting which of the tabs `tab.new` opened have closed when the strip
//! moves.
//!
//! A feed is bounded. A wait's holds its one answer; a stream's holds
//! [`FOLLOW_BUFFER`] events, and a reader that falls further behind than that
//! loses the oldest and is told how many with a `lagged` line, rather than
//! the window keeping everything it said for a client that is not reading.
//! A client that hangs up closes its feed, and a closed feed is dropped here
//! the next time anything is pushed.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crook_terminal::BlockState;
use crookui_core::prelude::*;
use serde_json::Value;

use super::protocol::{Follow, Following, PaneEvent, Refusal, Until, Verb, Wait, Waited, code};
use super::{blocks, word};
use crate::tab::{AgentSession, AgentStatus, PaneId, StatusSource, TabStrip};
use crate::workspace::Workspace;

/// How many events a stream holds for a reader that has not taken them yet.
///
/// A few hundred: far more than a burst of agents changing status puts out
/// between two reads by a reader that is keeping up, and a bound on what one
/// that is not can make the window hold for it.
pub const FOLLOW_BUFFER: usize = 256;

/// How many closed tabs `tab.new` opened the window remembers who opened, so
/// that a wait asked after its pane has closed is answered with that rather
/// than refused.
///
/// A few hundred: every worker a lead is still asking about, many times over,
/// and three numbers each, so a window open for a week of them holds a few
/// kilobytes. A wait on a pane older than that is refused as `no-such-pane`.
pub const REMEMBERED_CLOSED: usize = 256;

/// Where the window leaves what a kept connection is waiting for, and where
/// that connection's thread waits for it.
///
/// Shared by exactly two threads: the window's, which pushes, and the
/// connection's, which takes. Bounded at `capacity`: a push past it drops the
/// oldest item and counts it, and the next take says how many went.
pub struct Feed {
    state: Mutex<Fed>,
    changed: Condvar,
    capacity: usize,
}

/// What a [`Feed`] holds.
#[derive(Default)]
struct Fed {
    items: VecDeque<Value>,
    /// How many items were dropped, oldest first, since the last take said so.
    dropped: u64,
    /// The window will push nothing more.
    ended: bool,
    /// The connection is done with it: its client hung up, or it answered.
    hung_up: bool,
}

/// What a take from a [`Feed`] found.
#[derive(Debug, PartialEq)]
pub enum Next {
    /// The next item.
    Item(Value),
    /// This many items were dropped, oldest first, before the ones still
    /// held.
    Lagged(u64),
    /// The window has ended the feed and everything it pushed has been taken.
    Ended,
    /// The connection has hung up the feed.
    HungUp,
    /// Nothing came before the time given.
    TimedOut,
}

impl Feed {
    /// An empty feed holding at most `capacity` items.
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::default(),
            changed: Condvar::new(),
            capacity: capacity.max(1),
        }
    }

    /// The feed a kept connection asking `verb` needs: room for one answer
    /// for a wait, and [`FOLLOW_BUFFER`] events for a stream.
    pub fn for_verb(verb: &Verb) -> Self {
        match verb {
            Verb::PaneWait(_) => Self::new(1),
            _ => Self::new(FOLLOW_BUFFER),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Fed> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds an item, dropping the oldest when the feed is full. Nothing, once
    /// the feed is ended or hung up.
    pub fn push(&self, item: Value) {
        let mut fed = self.lock();
        if fed.ended || fed.hung_up {
            return;
        }
        if fed.items.len() >= self.capacity {
            fed.items.pop_front();
            fed.dropped += 1;
        }
        fed.items.push_back(item);
        self.changed.notify_all();
    }

    /// Says nothing more will be pushed. What is held can still be taken.
    pub fn end(&self) {
        self.lock().ended = true;
        self.changed.notify_all();
    }

    /// Says the connection is done with it, and lets go of what it holds.
    pub fn hang_up(&self) {
        let mut fed = self.lock();
        fed.hung_up = true;
        fed.items.clear();
        self.changed.notify_all();
    }

    /// Whether anything pushed now could still be taken.
    pub fn is_open(&self) -> bool {
        let fed = self.lock();
        !fed.ended && !fed.hung_up
    }

    /// How many items it holds, which is never more than its capacity.
    pub fn held(&self) -> usize {
        self.lock().items.len()
    }

    /// Takes the next item, waiting for one until `until`, or for as long as
    /// it takes when there is no `until`.
    ///
    /// Called on the connection's thread, never on the window's: this
    /// blocks, and the window is what it waits for.
    pub fn next(&self, until: Option<Instant>) -> Next {
        let mut fed = self.lock();
        loop {
            if fed.hung_up {
                return Next::HungUp;
            }
            if fed.dropped > 0 {
                return Next::Lagged(std::mem::take(&mut fed.dropped));
            }
            if let Some(item) = fed.items.pop_front() {
                return Next::Item(item);
            }
            if fed.ended {
                return Next::Ended;
            }
            fed = match until {
                None => self
                    .changed
                    .wait(fed)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(until) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Next::TimedOut;
                    }
                    self.changed
                        .wait_timeout(fed, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }
}

/// Everything the window has been asked to watch, and the feeds it answers
/// into.
///
/// Held by the workspace, which is where what happens to a pane is applied:
/// see [`Self::heard`] and [`Self::settled`]. Empty — and costing a length
/// check per update — while nobody watches.
///
/// It also remembers who opened the tabs `tab.new` opened, after they close:
/// see [`REMEMBERED_CLOSED`]. That is kept whether anybody watches or not,
/// since the wait it answers is the one asked after the pane has gone.
#[derive(Default)]
pub struct Watches {
    waiters: Vec<Waiter>,
    followers: Vec<Follower>,
    /// The panes `tab.new` opened that were open when the strip last
    /// settled.
    spawned: Vec<Spawned>,
    /// Those that have closed since, oldest first, at most
    /// [`REMEMBERED_CLOSED`].
    closed: VecDeque<Spawned>,
}

/// A pane `tab.new` opened, and who opened it: its
/// [`Lineage`](crate::tab::Lineage) without the title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Spawned {
    pane: PaneId,
    /// The pane that asked for it.
    caller: PaneId,
    /// The pane a person opened that the chain of asking began at.
    root: PaneId,
}

/// One `pane.wait` that has not got there yet.
struct Waiter {
    pane: PaneId,
    until: Until,
    feed: Arc<Feed>,
}

/// One `events.follow`.
struct Follower {
    feed: Arc<Feed>,
    /// The pane that asked.
    caller: PaneId,
    /// The one pane it asked about, when it named one.
    only: Option<PaneId>,
    /// The panes it is told about, as of the last time the strip moved.
    following: Vec<PaneId>,
}

impl Follower {
    /// The pane whose closing ends the stream: the one it asked about, or
    /// the caller's own.
    fn anchor(&self) -> PaneId {
        self.only.unwrap_or(self.caller)
    }
}

/// Something that happened in a pane, as [`Watches::heard`] is told it.
#[derive(Clone, Copy, Debug)]
pub enum Heard<'a> {
    /// What its agent says changed: the status or the message, which the
    /// session now holds.
    Status,
    /// A command started running.
    Started(&'a str),
    /// A command finished.
    Finished {
        /// Its command line, when there was one.
        command: Option<&'a str>,
        /// The status it exited with, when the shell reported one.
        exit: Option<i32>,
        /// How long it ran.
        took: Option<Duration>,
    },
}

impl Watches {
    /// Whether nothing is being watched.
    pub fn is_empty(&self) -> bool {
        self.waiters.is_empty() && self.followers.is_empty()
    }

    /// How many waits have not got there yet.
    pub fn waiting(&self) -> usize {
        self.waiters.len()
    }

    /// How many streams are open.
    pub fn following(&self) -> usize {
        self.followers.len()
    }

    /// Drops what a connection has let go of.
    fn prune(&mut self) {
        self.waiters.retain(|waiter| waiter.feed.is_open());
        self.followers.retain(|follower| follower.feed.is_open());
    }

    /// Answers into `feed` when `pane` gets to `until`.
    fn wait(&mut self, pane: PaneId, until: Until, feed: Arc<Feed>) {
        self.prune();
        self.waiters.push(Waiter { pane, until, feed });
    }

    /// Tells `feed` about everything that happens to `following` from now
    /// on, and to the tabs `caller` opens later when it named no pane.
    fn follow(
        &mut self,
        caller: PaneId,
        only: Option<PaneId>,
        following: Vec<PaneId>,
        feed: Arc<Feed>,
    ) {
        self.prune();
        self.followers.push(Follower {
            feed,
            caller,
            only,
            following,
        });
    }

    /// Tells everybody watching `pane` what happened in it. `session` is the
    /// pane's, as it is now that it has happened.
    pub fn heard(&mut self, pane: PaneId, session: &AgentSession, heard: Heard<'_>) {
        self.prune();
        let number = pane.as_u64();

        self.waiters.retain(|waiter| {
            if waiter.pane != pane {
                return true;
            }
            let (reached, exit) = match (waiter.until, heard) {
                (Until::Idle, Heard::Status) => (session.status == AgentStatus::Idle, None),
                (Until::NeedsInput, Heard::Status) => {
                    (session.status == AgentStatus::NeedsInput, None)
                }
                (Until::Finished, Heard::Finished { exit, .. }) => (true, exit),
                _ => (false, None),
            };
            if reached {
                waiter.feed.push(encode(&open_state(
                    number,
                    waiter.until,
                    true,
                    session,
                    exit,
                )));
            }
            !reached
        });

        if self.followers.is_empty() {
            return;
        }
        let event = match heard {
            Heard::Status => PaneEvent::Status {
                pane_id: number,
                status: word(session.status).to_owned(),
                message: session.message.clone(),
            },
            Heard::Started(command) => PaneEvent::Started {
                pane_id: number,
                command: command.to_owned(),
            },
            Heard::Finished {
                command,
                exit,
                took,
            } => PaneEvent::Finished {
                pane_id: number,
                command: command.map(str::to_owned),
                exit,
                duration_ms: took.map(millis),
            },
        };
        let event = encode(&event);
        for follower in &self.followers {
            if follower.following.contains(&pane) {
                follower.feed.push(event.clone());
            }
        }
    }

    /// Brings every watch up to date with the strip after it moved: a pane
    /// that closed answers the waits on it and is said to have closed, and a
    /// tab a follower's caller opened joins its stream. A tab `tab.new`
    /// opened that has closed is remembered, watched or not.
    pub fn settled(&mut self, strip: &TabStrip) {
        self.remember_closed(strip);
        if self.is_empty() {
            return;
        }
        self.prune();
        let open = |pane: PaneId| strip.pane(pane).is_some();

        self.waiters.retain(|waiter| {
            if open(waiter.pane) {
                return true;
            }
            waiter
                .feed
                .push(encode(&closed_state(waiter.pane, waiter.until)));
            false
        });

        let closed = &self.closed;
        self.followers.retain_mut(|follower| {
            let now = match follower.only {
                Some(only) => [only].into_iter().filter(|pane| open(*pane)).collect(),
                None => watchable(strip, closed, follower.caller),
            };
            for gone in follower.following.iter().filter(|pane| !now.contains(pane)) {
                follower.feed.push(encode(&PaneEvent::Closed {
                    pane_id: gone.as_u64(),
                }));
            }
            for new in now.iter().filter(|pane| !follower.following.contains(pane)) {
                follower.feed.push(encode(&PaneEvent::Opened {
                    pane_id: new.as_u64(),
                }));
            }
            follower.following = now;
            if open(follower.anchor()) {
                return true;
            }
            follower.feed.end();
            false
        });
    }

    /// Moves the tabs `tab.new` opened that are no longer in `strip` to the
    /// ones remembered as closed, forgetting the oldest past
    /// [`REMEMBERED_CLOSED`].
    ///
    /// Every road a pane opens or closes by settles the strip, and a tab is
    /// given its lineage before the settle that opens it, so each is seen
    /// open here before it is seen gone.
    fn remember_closed(&mut self, strip: &TabStrip) {
        let now: Vec<Spawned> = strip
            .panes()
            .filter_map(|(_, pane)| {
                let lineage = pane.session().spawned_by.as_ref()?;
                Some(Spawned {
                    pane: pane.id(),
                    caller: lineage.caller,
                    root: lineage.root,
                })
            })
            .collect();
        for gone in &self.spawned {
            if now.iter().any(|open| open.pane == gone.pane) {
                continue;
            }
            if self.closed.len() >= REMEMBERED_CLOSED {
                self.closed.pop_front();
            }
            self.closed.push_back(*gone);
        }
        self.spawned = now;
    }

    /// The closed pane numbered `number`, if it is one `tab.new` opened that
    /// is still remembered.
    fn remembered(&self, number: u64) -> Option<PaneId> {
        self.closed
            .iter()
            .rev()
            .map(|spawned| spawned.pane)
            .find(|pane| pane.as_u64() == number)
    }

    /// Whether `caller` may watch `pane` without anybody's grant: see
    /// [`observes`], which this is with the closed tabs remembered here.
    pub fn observes(&self, strip: &TabStrip, caller: PaneId, pane: PaneId) -> bool {
        observes(strip, &self.closed, caller, pane)
    }
}

impl Drop for Watches {
    /// A window that closes tells every kept connection so at once, rather
    /// than leaving a wait to run out its time for a window that is gone.
    fn drop(&mut self) {
        for waiter in &self.waiters {
            waiter.feed.end();
        }
        for follower in &self.followers {
            follower.feed.end();
        }
    }
}

/// Whether `caller` may watch `pane` without anybody's grant: it is the
/// caller, or a tab the caller opened, or one opened by one of those.
///
/// Up the chain of who opened what, through the panes still open and the
/// closed tabs still remembered in `closed`; and a pane whose lineage names
/// the caller as the root the chain began at is the caller's whichever pane
/// in between has gone.
fn observes(strip: &TabStrip, closed: &VecDeque<Spawned>, caller: PaneId, pane: PaneId) -> bool {
    let opener = |pane: PaneId| match strip.pane(pane) {
        Some(open) => open
            .session()
            .spawned_by
            .as_ref()
            .map(|lineage| (lineage.caller, lineage.root)),
        None => closed
            .iter()
            .find(|spawned| spawned.pane == pane)
            .map(|spawned| (spawned.caller, spawned.root)),
    };
    let mut at = pane;
    // Bounded by the panes there are and were, since a chain of who opened
    // what can be no longer; a lineage cannot loop, and this is what makes
    // that a fact about the loop rather than about every future change to it.
    for _ in 0..=strip.panes().count() + closed.len() {
        if at == caller {
            return true;
        }
        let Some((opened_by, root)) = opener(at) else {
            return false;
        };
        if root == caller {
            return true;
        }
        at = opened_by;
    }
    false
}

/// Every open pane `caller` may watch, in the panel's order.
fn watchable(strip: &TabStrip, closed: &VecDeque<Spawned>, caller: PaneId) -> Vec<PaneId> {
    strip
        .panes()
        .map(|(_, pane)| pane.id())
        .filter(|pane| observes(strip, closed, caller, *pane))
        .collect()
}

/// The pane a request comes from, by the token it carries — or the refusal
/// of a request that carries no pane's.
fn caller(workspace: &Workspace, token: Option<&str>, app: &AppContext) -> Result<PaneId, Refusal> {
    token
        .and_then(|token| workspace.pane_with_token(token, app))
        .ok_or_else(|| {
            Refusal::new(
                code::UNAUTHORIZED,
                "only a pane of this window can watch a pane, and the request carries no pane's \
                 CROOK_TOKEN; run this inside a Crook pane",
            )
        })
}

/// The open pane a request may watch, from its token and the number it asked
/// about — or why it may not — and the caller it comes from.
///
/// In this order, so that a request with no pane's token learns nothing about
/// which panes are open, and one with a token learns only what `pane.list`
/// would already tell it.
pub fn observed(
    workspace: &Workspace,
    token: Option<&str>,
    number: u64,
    app: &AppContext,
) -> Result<(PaneId, PaneId), Refusal> {
    match found(workspace, token, number, app)? {
        (caller, Found::Open(pane)) => Ok((caller, pane)),
        (_, Found::Closed(_)) => Err(no_such_pane(number)),
    }
}

/// A pane a request may watch.
enum Found {
    /// One that is open.
    Open(PaneId),
    /// A tab the caller opened, or one of its tabs did, that has closed and
    /// is still remembered: see [`REMEMBERED_CLOSED`].
    Closed(PaneId),
}

/// The pane a request may watch, open or closed, and the caller — or why it
/// may not, in [`observed`]'s order.
///
/// A closed pane the caller may not watch is `no-such-pane`, as it would be
/// were it not remembered: what a stranger learns of a pane that has gone is
/// what `pane.list` says of it, which is nothing.
fn found(
    workspace: &Workspace,
    token: Option<&str>,
    number: u64,
    app: &AppContext,
) -> Result<(PaneId, Found), Refusal> {
    let caller = caller(workspace, token, app)?;
    let strip = workspace.tabs();
    let watches = workspace.watches();
    let Some(pane) = strip
        .panes()
        .map(|(_, pane)| pane.id())
        .find(|pane| pane.as_u64() == number)
    else {
        return match watches.remembered(number) {
            Some(pane) if watches.observes(strip, caller, pane) => {
                Ok((caller, Found::Closed(pane)))
            }
            _ => Err(no_such_pane(number)),
        };
    };
    if !watches.observes(strip, caller, pane) {
        return Err(Refusal::new(
            code::NEEDS_GRANT,
            format!(
                "pane {number} is neither this pane nor a tab it opened, and watching it needs \
                 a grant from the person who opened it, which this version of Crook cannot ask \
                 for yet; watch your own pane and the tabs you open with `crook tab new`"
            ),
        ));
    }
    Ok((caller, Found::Open(pane)))
}

/// The refusal a number no open pane has gets.
pub fn no_such_pane(number: u64) -> Refusal {
    Refusal::new(
        code::NO_SUCH_PANE,
        format!(
            "no pane {number} is open in this window: it has closed, or it never was; \
             `crook pane list` shows the open ones"
        ),
    )
}

/// Answers one `pane.wait`.
///
/// With no feed — a wait of no time, or the question a kept connection asks
/// when its time has run out — the answer is where the pane is now. With
/// one, the answer goes into the feed: at once when the pane is already
/// there, or has closed, and otherwise when it gets there, with `null` said
/// now to say the wait is registered.
///
/// A pane that closed before the wait was asked is answered as one that
/// closed during it would be — `exited` reached, anything else not — for as
/// long as it is remembered: `crook pane wait 7 --until exited && …` must
/// not depend on whether the worker ended before the lead got round to
/// asking.
pub fn wait(
    workspace: &mut Workspace,
    asked: &Wait,
    token: Option<&str>,
    feed: Option<Arc<Feed>>,
    app: &AppContext,
) -> Result<Value, Refusal> {
    let (_, found) = found(workspace, token, asked.pane, app)?;
    let (pane, now) = match found {
        Found::Open(pane) => (pane, where_it_is(workspace, pane, asked.until, app)?),
        Found::Closed(pane) => (pane, closed_state(pane, asked.until)),
    };
    let Some(feed) = feed else {
        return Ok(encode(&now));
    };
    if now.reached || now.closed {
        feed.push(encode(&now));
    } else {
        workspace.watches_mut().wait(pane, asked.until, feed);
    }
    Ok(Value::Null)
}

/// Where a pane is, measured against `until`.
///
/// `finished` is refused for a pane whose shell reports no command marks:
/// nothing there ever says a command finished, and a wait for it would only
/// ever run out.
fn where_it_is(
    workspace: &Workspace,
    pane: PaneId,
    until: Until,
    app: &AppContext,
) -> Result<Waited, Refusal> {
    let number = pane.as_u64();
    let session = workspace
        .tabs()
        .pane(pane)
        .map(|open| open.session())
        .ok_or_else(|| no_such_pane(number))?;
    let (reached, exit) = match until {
        Until::Idle => (
            session.status == AgentStatus::Idle && session.source != StatusSource::NoReport,
            None,
        ),
        Until::NeedsInput => (session.status == AgentStatus::NeedsInput, None),
        Until::Exited => (false, None),
        Until::Finished => {
            match workspace.shell_marks(pane, app) {
                Some(true) => {}
                Some(false) => {
                    return Err(Refusal::new(
                        code::NO_BLOCKS,
                        format!(
                            "pane {number}'s shell reports no command marks, so nothing there \
                             ever says a command finished; wait for its agent's status instead"
                        ),
                    ));
                }
                None => {
                    return Err(Refusal::new(
                        code::NO_BLOCKS,
                        format!("pane {number} has no shell running to finish a command"),
                    ));
                }
            }
            finished(workspace, pane, app)
        }
    };
    Ok(open_state(number, until, reached, session, exit))
}

/// Whether a pane's last command has finished and nothing has taken its
/// place, and the status it exited with.
///
/// At rest — at a prompt, or between a command's end and the next prompt —
/// with no line waiting to be sent at its first prompt, and a command
/// finished there: a command's block, not what the shell printed on its own
/// (see [`blocks::is_command`]). The line waiting is what keeps a wait asked
/// the moment a tab opened from being answered by the tab's empty shell,
/// before the command it was opened for has started.
fn finished(workspace: &Workspace, pane: PaneId, app: &AppContext) -> (bool, Option<i32>) {
    if workspace.waits_for_first_prompt(pane) {
        return (false, None);
    }
    let resting = workspace.terminal(pane, app).is_some_and(|(_, snapshot)| {
        matches!(
            snapshot.live_block.state,
            BlockState::AtPrompt | BlockState::Done
        )
    });
    // The newest command's, past whatever the shell printed on its own: a
    // tab's startup lines before its first prompt are not a command that
    // finished.
    let last = workspace.terminal_blocks(pane, app).and_then(|history| {
        (0..history.len())
            .rev()
            .filter_map(|index| history.get(index))
            .find(|block| blocks::is_command(block))
            .map(|block| block.exit)
    });
    match (resting, last) {
        (true, Some(exit)) => (true, exit),
        _ => (false, None),
    }
}

/// A wait's answer for a pane that has closed: there is nothing left to say
/// of it but that, which is what `exited` waits for.
fn closed_state(pane: PaneId, until: Until) -> Waited {
    Waited {
        pane_id: pane.as_u64(),
        until,
        reached: until == Until::Exited,
        status: None,
        message: None,
        exit: None,
        closed: true,
    }
}

/// A wait's answer for a pane that is still open.
fn open_state(
    number: u64,
    until: Until,
    reached: bool,
    session: &AgentSession,
    exit: Option<i32>,
) -> Waited {
    Waited {
        pane_id: number,
        until,
        reached,
        status: Some(word(session.status).to_owned()),
        message: session.message.clone(),
        exit,
        closed: false,
    }
}

/// Answers one `events.follow`: the panes followed as of now, with every
/// event about them going into `feed` from here on.
pub fn follow(
    workspace: &mut Workspace,
    asked: &Follow,
    token: Option<&str>,
    feed: Option<Arc<Feed>>,
    app: &AppContext,
) -> Result<Value, Refusal> {
    let (caller, only) = match asked.pane {
        Some(number) => {
            let (caller, pane) = observed(workspace, token, number, app)?;
            (caller, Some(pane))
        }
        None => (caller(workspace, token, app)?, None),
    };
    let Some(feed) = feed else {
        // Only a kept connection has somewhere to put the events.
        return Err(Refusal::new(
            code::BAD_REQUEST,
            "`events.follow` streams over the connection it is asked on",
        ));
    };
    let following = match only {
        Some(pane) => vec![pane],
        None => watchable(workspace.tabs(), &workspace.watches().closed, caller),
    };
    let answer = Following {
        panes: following.iter().map(|pane| pane.as_u64()).collect(),
    };
    workspace
        .watches_mut()
        .follow(caller, only, following, feed);
    Ok(encode(&answer))
}

/// A duration in whole milliseconds, for the wire.
fn millis(took: Duration) -> u64 {
    u64::try_from(took.as_millis()).unwrap_or(u64::MAX)
}

/// Encodes something the wire carries.
fn encode(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("what the wire carries is numbers and strings")
}
