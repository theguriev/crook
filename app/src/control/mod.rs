//! The window answers: a local socket a script or an agent asks a running
//! Crook over.
//!
//! A status travels one way, from the program in a pane to its row, and it
//! goes over the pane's own terminal — see [`crate::agent`]. A question has to
//! come back, and it cannot go the same way: anything that prints can write
//! into a pane, so a request read off the pane's output could be forged by a
//! `cat` of a file. So the window listens on a Unix socket only this user can
//! reach, tells every pane where it is in [`SOCKET_VARIABLE`], and answers two
//! verbs: `pane.list`, which only reads, and `tab.new`, which opens a tab
//! beside the pane that asked and runs a command in it. "The window answers"
//! in `docs/architecture.md` is the whole argument: the threat model, the
//! protocol, and what the verbs after these will need.
//!
//! # Who may open a tab
//!
//! Anything this user runs can connect, so connecting proves nothing about
//! which pane is asking — and a tab opened for nobody is a tab with no budget
//! and no lineage to show. Every pane's shell is handed a secret of its own in
//! [`TOKEN_VARIABLE`], minted from the operating system's random source by
//! [`mint_token`] when the shell opens and forgotten when it ends; a request
//! carries it, and the window reads the pane back out of it. A request with no
//! token, or one no open pane holds, may still list the panes and may open
//! nothing. See [`spawn`] for the rest of what `tab.new` checks.
//!
//! # Layout
//!
//! * [`protocol`] is the wire: a line into a [`Request`] or a refusal, an
//!   answer into a line. Nothing in it touches a socket.
//! * `server` is the socket, on Unix: the private directory, the stale-socket
//!   probe, the listener thread and one short-lived thread per connection.
//! * [`cli`] is `crook pane list` and `crook tab new`, the other end.
//! * [`spawn`] is the window's side of `tab.new`: the token, the budget, the
//!   worktree, and the command typed at the new pane's first prompt.
//! * This file is the window's side of the rest: the [`Inbox`] a connection
//!   leaves its question in, the chain that answers it on the main thread,
//!   and [`panes`], which is what `pane.list` says.
//!
//! # Never on the UI thread
//!
//! A connection's thread reads the line, and waits — until a little short of
//! its deadline, so that a `timeout` can still be written — for the window's
//! answer. The window hears about the question the way it hears about a pty's
//! output: a future the connection's thread wakes, awaited on the foreground
//! through `ctx.spawn`, so answering is a read of the tab strip done between
//! two frames and nothing on the main thread ever blocks on a socket. A window
//! that has closed says so rather than leaving the question to time out: see
//! [`Inbox::close`]. The one answer that takes longer, a tab in a new
//! worktree, does its git on the pool and answers when that comes home.
//!
//! # Windows
//!
//! Nothing listens there yet: [`Control::open`] is `None`, every pane is told
//! so with an empty [`SOCKET_VARIABLE`] and an empty [`TOKEN_VARIABLE`], and
//! `crook pane list` and `crook tab new` say the command is not available on
//! this platform. An owner-only named pipe is the route when it comes.

pub mod cli;
pub mod protocol;
#[cfg(unix)]
pub mod server;
pub mod spawn;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Instant;

use crook_terminal::AgentReport;
use crookui_core::prelude::*;
use serde_json::Value;

use crate::git;
use crate::tab::{AgentSession, AgentStatus};
use crate::workspace::Workspace;

use self::protocol::{PaneEntry, Refusal, Request, Verb, code};
use self::spawn::Spawns;

/// The variable every pane's shell finds the window's socket in.
///
/// Set beside `TERM_PROGRAM` and `CROOK_PANE_ID`, in every shell Crook starts,
/// and set *empty* when this window has no socket: a Crook started inside
/// another Crook's pane would otherwise hand its own shells the outer one's,
/// and `crook pane list` would answer about a window the person is not in.
pub const SOCKET_VARIABLE: &str = "CROOK_SOCKET";

/// The variable every pane's shell finds its own secret in: what a request
/// carries to say which pane it comes from.
///
/// Beside [`SOCKET_VARIABLE`], and set *empty* for the same reason: a Crook
/// started inside another Crook's pane would otherwise hand its shells the
/// outer pane's, and anything run in them could open tabs in the outer window
/// as that pane.
pub const TOKEN_VARIABLE: &str = "CROOK_TOKEN";

/// How many random bytes a token is made of.
///
/// Thirty-two, which is what a session key is: a token is compared with the
/// others by the window and nothing else, so its only job is to be impossible
/// to guess, and at 256 bits nobody is guessing.
const TOKEN_BYTES: usize = 32;

/// A new pane's secret, from the operating system's random source, as hex.
///
/// `None` when that source cannot be read, which the log says: the pane is
/// then handed no token and can open no tab, which is the safe way for a
/// secret to fail.
pub fn mint_token() -> Option<String> {
    let mut bytes = [0u8; TOKEN_BYTES];
    if let Err(error) = getrandom::getrandom(&mut bytes) {
        log::warn!("no control token for a pane: the random source failed: {error}");
        return None;
    }
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The window's end of the socket, for as long as the window holds it.
///
/// Dropping it closes the socket, removes its file, and refuses any question
/// still waiting as `gone`.
pub struct Control {
    #[cfg(unix)]
    socket: server::Socket,
    inbox: Arc<Inbox>,
}

impl Control {
    /// Opens this process's socket in the user's control directory.
    ///
    /// `None` on a platform with no socket yet, and when this one could not be
    /// opened — a directory somebody else could enter, a path too long for a
    /// socket — which the log says. A window without a socket is a window
    /// nothing outside can ask, and nothing else about it changes.
    pub fn open() -> Option<Self> {
        #[cfg(unix)]
        {
            let directory = server::directory();
            match Self::open_in(&directory) {
                Ok(control) => Some(control),
                Err(error) => {
                    log::warn!(
                        "no control socket in {}: {error}; `crook pane list` cannot ask this \
                         window",
                        directory.display()
                    );
                    None
                }
            }
        }
        #[cfg(not(unix))]
        {
            log::debug!("no control socket: this platform has none yet");
            None
        }
    }

    /// The same, in a named directory, which is how a test opens one of its
    /// own rather than one in the directory of whoever is running it.
    #[cfg(unix)]
    pub fn open_in(directory: &Path) -> std::io::Result<Self> {
        let inbox = Arc::new(Inbox::default());
        let asking = inbox.clone();
        let socket = server::Socket::open_in(
            directory,
            Arc::new(move |request, deadline| asking.ask(request, deadline)),
        )?;
        Ok(Self { socket, inbox })
    }

    /// Starts answering what the socket's connections ask, on the window's
    /// own thread, for as long as the window is open.
    pub fn serve(&self, ctx: &mut ViewContext<Workspace>) {
        serve(self.inbox.clone(), Rc::default(), ctx);
    }

    /// Where the socket is, for the panes' environment.
    pub fn path(&self) -> Option<&Path> {
        #[cfg(unix)]
        {
            Some(self.socket.path())
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.inbox.close();
    }
}

/// Where a connection leaves its question for the window, and where the
/// window picks it up.
///
/// A queue behind a lock and one waker, which is the whole of the mechanism a
/// pty's reader thread uses to reach the main thread — see
/// `terminal_model::Wake` — with an answer channel riding on each question.
#[derive(Default)]
pub struct Inbox {
    state: Mutex<Asked>,
}

/// What is waiting in an [`Inbox`].
#[derive(Default)]
struct Asked {
    questions: Vec<Question>,
    /// The window's chain, parked until there is something to answer.
    waker: Option<Waker>,
    /// The window has closed, and nothing will answer again.
    closed: bool,
}

/// One question, and where its answer goes.
struct Question {
    request: Request,
    answer: Answering,
}

/// Where one question's answer goes: the connection's thread, waiting on the
/// other end.
///
/// Carried rather than answered at once by a verb that has work to do off the
/// main thread first — a worktree to check out — and answered when that comes
/// home. A connection that has given up waiting has dropped its end, and an
/// answer sent to it goes nowhere, which is all giving up has to mean.
pub struct Answering(mpsc::Sender<Result<Value, Refusal>>);

impl Answering {
    /// Hands the connection its answer.
    pub fn send(self, answer: Result<Value, Refusal>) {
        // A closed receiver is the whole of how a connection that gave up
        // says so, and there is nobody left to tell.
        let _ = self.0.send(answer);
    }
}

impl Inbox {
    fn lock(&self) -> MutexGuard<'_, Asked> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks the window, and waits for its answer until `deadline`.
    ///
    /// Called on a connection's own thread, never on the window's: this
    /// blocks, and the window is what it is waiting for.
    pub fn ask(&self, request: Request, deadline: Instant) -> Result<Value, Refusal> {
        let (answer, answered) = mpsc::channel();
        let waker = {
            let mut asked = self.lock();
            if asked.closed {
                return Err(gone());
            }
            asked.questions.push(Question {
                request,
                answer: Answering(answer),
            });
            asked.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }

        match answered.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(answer) => answer,
            Err(RecvTimeoutError::Timeout) => Err(Refusal::new(
                code::TIMEOUT,
                "the window did not answer in time",
            )),
            // The window dropped the question without answering it: it closed
            // between the question being asked and being read.
            Err(RecvTimeoutError::Disconnected) => Err(gone()),
        }
    }

    /// Refuses every question waiting, and every one asked from now on.
    ///
    /// Without it a question asked of a window that has closed would sit in a
    /// queue nothing reads until its connection gave up on it, and a script
    /// would be told "timeout" about a window that is not there.
    pub fn close(&self) {
        let mut asked = self.lock();
        asked.closed = true;
        // Dropping each question drops its sender, which is what tells the
        // thread waiting on it.
        asked.questions.clear();
    }

    /// Resolves with every question asked since the last time it resolved.
    fn asked(self: &Arc<Self>) -> Questions {
        Questions {
            inbox: self.clone(),
        }
    }
}

/// The future the window's chain parks on until a question arrives.
struct Questions {
    inbox: Arc<Inbox>,
}

impl Future for Questions {
    type Output = Vec<Question>;

    fn poll(self: Pin<&mut Self>, ctx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        let mut asked = self.inbox.lock();
        if asked.questions.is_empty() {
            asked.waker = Some(ctx.waker().clone());
            return Poll::Pending;
        }
        Poll::Ready(std::mem::take(&mut asked.questions))
    }
}

/// The refusal a question to a window that has closed gets.
fn gone() -> Refusal {
    Refusal::new(code::GONE, "the window closed before it answered")
}

/// Answers every question the inbox holds, then waits for the next.
///
/// One outstanding wait, and the only thing that starts the next is the last
/// one finishing — the ownership trick every chain in the window uses. It
/// parks on a waker rather than a timer, so a window nobody asks anything
/// does no work at all. `spawns` rides along it: what the window remembers
/// about who has been refused and what is still being opened, which only
/// this chain and the answers it hands to the pool ever touch.
fn serve(inbox: Arc<Inbox>, spawns: Rc<RefCell<Spawns>>, ctx: &mut ViewContext<Workspace>) {
    let questions = inbox.asked();
    ctx.spawn(questions, move |workspace, questions, ctx| {
        for question in questions {
            answer(workspace, question, &spawns, ctx);
        }
        serve(inbox, spawns, ctx);
    })
    .detach();
}

/// What the window says to one question.
fn answer(
    workspace: &mut Workspace,
    question: Question,
    spawns: &Rc<RefCell<Spawns>>,
    ctx: &mut ViewContext<Workspace>,
) {
    let Question { request, answer } = question;
    match request.verb {
        Verb::PaneList => answer.send(Ok(serde_json::to_value(panes(workspace, ctx))
            .expect("a pane entry is numbers, strings and booleans, which encode"))),
        Verb::TabNew(asked) => {
            spawn::open(
                workspace,
                asked,
                request.token.as_deref(),
                spawns,
                answer,
                ctx,
            );
        }
    }
}

/// Every pane of the window, in the panel's order, as `pane.list` answers.
///
/// Read off the tab strip and the git model's map, both already in memory: an
/// answer costs a walk of a few dozen panes and no directory, no process and
/// no lock.
pub fn panes(workspace: &Workspace, app: &AppContext) -> Vec<PaneEntry> {
    let strip = workspace.tabs();
    let focused = strip.focused_pane_id();
    let home = workspace.home();
    let facts = workspace.git().as_ref(app);

    let mut entries = Vec::new();
    for tab in strip.iter() {
        let group = tab
            .group()
            .and_then(|group| strip.group(group))
            .map(|group| group.name().to_owned());
        let tab_title = tab
            .panes()
            .focused()
            .map(|pane| title(pane.session(), home))
            .unwrap_or_default();
        for pane in tab.panes().iter() {
            let session = pane.session();
            let directory = session.working_directory.as_deref();
            entries.push(PaneEntry {
                pane_id: pane.id().as_u64(),
                tab_id: tab.id().as_u64(),
                title: title(session, home),
                tab_title: tab_title.clone(),
                group: group.clone(),
                focused: focused == Some(pane.id()),
                status: word(session.status).to_owned(),
                message: session.message.clone(),
                cwd: directory.map(|directory| directory.to_string_lossy().into_owned()),
                branch: directory
                    .and_then(|directory| facts.facts(directory))
                    .and_then(|facts| facts.branch.as_ref())
                    .map(|head| head.label().to_owned()),
            });
        }
    }
    entries
}

/// What a pane's row is called, by the row's own rule — `RowFacts::resolve`
/// in the workspace — so that what a script reads is what a person sees: a
/// name the session has, else its directory's, else the name it was opened
/// with.
pub fn title(session: &AgentSession, home: Option<&Path>) -> String {
    session
        .name()
        .map(str::to_owned)
        .or_else(|| {
            session
                .working_directory
                .as_deref()
                .and_then(|directory| git::directory_label(directory, home))
        })
        .unwrap_or_else(|| session.display_title().to_owned())
}

/// The word for a status: the one `crook --agent` takes for it, from the one
/// place those words are spelled.
fn word(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => AgentReport::Idle,
        AgentStatus::Running => AgentReport::Running,
        AgentStatus::NeedsInput => AgentReport::NeedsInput,
        AgentStatus::Failed => AgentReport::Failed,
    }
    .word()
}
