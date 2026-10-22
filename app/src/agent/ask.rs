//! `crook --agent ask claude`: an agent's permission hook, asking the window.
//!
//! Claude Code runs its `PermissionRequest` hook as it opens a permission
//! dialog, hands it the tool and the tool's input on standard input, and
//! reads what the hook prints on standard output as the decision — or, when
//! it prints nothing, goes on with the dialog it already has open. This is
//! that hook: it hands the window the pane is in the whole request, waits on
//! the connection for what a person says there, and prints it. Claude Code
//! runs it as its `PermissionRequest` hook — see `CLAUDE_ASKS` in
//! [`crate::agent`] — and a person answers it by pressing Allow once or Deny
//! on the window's card; see [`crate::control::ask`].
//!
//! # What it promises
//!
//! Standard output carries one decision line or nothing. Nothing is printed
//! but an allow or a deny the window sent, exactly the bytes Claude Code
//! reads for one — never `updatedInput`, `updatedPermissions` or `interrupt`
//! — and every other way it can go prints nothing: not in Crook, an older
//! window, a refusal, a timeout, a parent gone, a reader gone. It exits 0 on
//! every path, since a hook that fails can fail its agent. It never opens or
//! writes a terminal: which pane it is in is the device number of the
//! terminal a status from it would be written to, read off the process table
//! by `super::ancestors`. For a change to a file it looks, for at most a
//! second, at where the change would land — through which links, and over
//! what file — and writes nothing: see `super::on_disk`.
//!
//! # When it asks nothing
//!
//! Before it connects anywhere, unless all of these hold: `CROOK_SOCKET` names
//! a window; no `STY` or `ZELLIJ` says screen or zellij stands between the
//! agent and the pane; the input is Claude Code's `PermissionRequest` for
//! `Bash`, `Edit`, `MultiEdit` or `Write`, from the main thread and not a
//! subagent, with a session and an absolute working directory;
//! `CLAUDE_CODE_ENTRYPOINT` is exactly `cli` —
//! under `claude -p` an empty answer is Claude Code's own immediate deny, and
//! holding it would turn a refusal into a wait; the tool's input is one the
//! allowlist in [`super::request`] reads whole; a terminal is found — inside
//! tmux, a client of tmux's — and, for a write, whether a file is there could
//! be said.
//!
//! The socket is asked only once it has been checked, as every client's is
//! — see `control::cli` — and the connection is never half closed: on macOS
//! the window cannot tell a closed half from a hook that has gone.
//!
//! # Inside tmux
//!
//! The terminal a status from here goes to inside tmux is not the agent's,
//! which is one of tmux's panes, but that of each client attached to the
//! agent's session — see `super::tmux` — and one of those is the Crook pane
//! a person sees tmux in. So the question names those terminals rather than
//! the agent's, and nothing it inherited: a tmux server keeps the
//! `CROOK_SOCKET` and `CROOK_TOKEN` of the pane it was started from, which
//! may be another window's, or one's since closed. No token is sent.
//!
//! And no window is told what the agent asks until it has said it holds one
//! of those terminals: `pane.find`, which carries the terminals and nothing
//! else, is asked first of `CROOK_SOCKET`'s window, and, when that one is
//! gone or holds none, of at most four others listening in this user's
//! checked control directory, all within two seconds — see `find`, which
//! only Unix has. The question goes to the first that says yes, on the
//! connection it said it on, and to no other. screen and zellij cannot be
//! asked where their clients are, and a hook inside one asks nothing.
//!
//! # Windows
//!
//! There is no socket there, and this prints nothing at once.

use std::io::{self, Write};
#[cfg(any(unix, test))]
use std::path::{Path, PathBuf};
#[cfg(any(unix, test))]
use std::time::{Duration, Instant};

#[cfg(any(unix, test))]
use serde_json::Value;

#[cfg(any(unix, test))]
use super::request::{AskRequest, OnDisk, from_claude};
use crate::control::protocol::AskingAgent;
#[cfg(any(unix, test))]
use crate::control::protocol::{
    self, AgentAsk, Answered, Decision, Find, Located, MAX_CWD, Multiplexer, Registered, Reply,
    Verb, Verdict,
};

/// How long a Claude Code hook waits for a person, in all.
///
/// Five minutes, which Claude Code's dialog waits beside it, and under the
/// timeout its hook entry will be given; the window holds the question this
/// long and a little more, and says when it is over.
#[cfg(any(unix, test))]
pub const CLAUDE_AWAIT: Duration = Duration::from_secs(300);

/// How long the window has to say it holds the question.
///
/// A second: the window answers between two frames, and a window that cannot
/// in a second is one whose agent's own prompt is the better place to answer.
#[cfg(unix)]
pub const REGISTER_DEADLINE: Duration = Duration::from_secs(1);

/// How long a hook waits for a window to take its connection.
///
/// A second, as for the window to say it holds the question: a window takes
/// a connection at once unless it has stopped, and one that has, with its
/// backlog full, is no window to ask. It keeps the hook inside the 330
/// seconds Claude Code gives it: before it reads for its answer, which it
/// does for [`CLAUDE_AWAIT`] and 15 seconds more, it has spent at most a
/// second asking tmux, a connect to `CROOK_SOCKET`'s window, those to the
/// others inside [`FIND_DEADLINE`] and one past it, and [`REGISTER_DEADLINE`]
/// for the window to say it holds the question — a few seconds in all.
#[cfg(unix)]
pub const CONNECT_DEADLINE: Duration = Duration::from_secs(1);

/// How long, in all, a hook inside tmux looks for the window that holds one
/// of its clients' terminals before it gives up and asks nothing.
#[cfg(unix)]
pub const FIND_DEADLINE: Duration = Duration::from_secs(2);

/// How many windows besides `CROOK_SOCKET`'s a hook inside tmux asks
/// whether they hold one of its clients' terminals.
#[cfg(any(unix, test))]
pub const MAX_OTHER_WINDOWS: usize = 4;

/// How much longer than [`CLAUDE_AWAIT`] the hook reads, so that the window,
/// which knows the question is over, is the side that says so.
#[cfg(unix)]
const PAST_THE_AWAIT: Duration = Duration::from_secs(15);

/// How often, while it waits, the hook looks at whether its parent is still
/// the one that started it.
#[cfg(unix)]
const POLL: Duration = Duration::from_millis(500);

/// The most of standard input read, in bytes. More is no question: a
/// request too long to show is not asked.
#[cfg(unix)]
pub const MAX_INPUT: usize = 1024 * 1024;

/// What Claude Code reads as "allow it, this once".
#[cfg(any(unix, test))]
pub const ALLOW: &str = "{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"allow\"}}}\n";

/// The parts of the environment the hook's rules read.
#[cfg(any(unix, test))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Environment {
    /// `CROOK_SOCKET`.
    pub socket: Option<PathBuf>,
    /// `CROOK_TOKEN`, when it is not empty.
    pub token: Option<String>,
    /// Which multiplexer, when `TMUX`, `STY` or `ZELLIJ` names one.
    pub multiplexer: Option<Multiplexed>,
    /// `CLAUDE_CODE_ENTRYPOINT`.
    pub entrypoint: Option<String>,
}

#[cfg(any(unix, test))]
impl Environment {
    /// This process's.
    #[cfg(unix)]
    pub fn here() -> Self {
        let named = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        Self {
            socket: std::env::var_os(crate::control::SOCKET_VARIABLE)
                .filter(|socket| !socket.is_empty())
                .map(PathBuf::from),
            token: std::env::var(crate::control::TOKEN_VARIABLE)
                .ok()
                .filter(|token| !token.is_empty()),
            multiplexer: if named("STY") || named("ZELLIJ") {
                Some(Multiplexed::Other)
            } else if named("TMUX") {
                Some(Multiplexed::Tmux)
            } else {
                None
            },
            entrypoint: std::env::var("CLAUDE_CODE_ENTRYPOINT").ok(),
        }
    }
}

/// A multiplexer between the agent and the pane.
#[cfg(any(unix, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Multiplexed {
    /// tmux, which can be asked where its clients are.
    Tmux,
    /// screen or zellij, which cannot — or tmux inside one of them.
    Other,
}

/// What names the pane a question goes to.
#[cfg(any(unix, test))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bound {
    /// The device number of the terminal a status from here would be
    /// written to.
    Tty(u32),
    /// Inside tmux: the terminals of its clients.
    Multiplexer(Multiplexer),
}

/// A question to ask, and where.
#[cfg(any(unix, test))]
#[derive(Debug, PartialEq, Eq)]
pub struct Question {
    /// The window's socket.
    pub socket: PathBuf,
    /// The pane's token, when the environment has one: a hint the window
    /// checks, never what binds the question.
    pub token: Option<String>,
    /// What is asked.
    pub asked: AgentAsk,
}

/// The question `input` asks, or `None` when the hook asks nothing: see the
/// module docs for the rules, in the order they are read. `terminal` is
/// asked once every other rule holds — for the terminal, or inside tmux for
/// its clients', whichever the environment says to look for — and then
/// `look`, for a change to a file, says what is on disk where it would land
/// under the working directory — or `None`, which is no question.
#[cfg(any(unix, test))]
pub fn question(
    input: &[u8],
    environment: &Environment,
    terminal: impl FnOnce() -> Option<Bound>,
    look: impl FnOnce(&AskRequest, &str) -> Option<OnDisk>,
) -> Option<Question> {
    let socket = environment.socket.clone()?;
    if environment.multiplexer == Some(Multiplexed::Other) {
        return None;
    }
    let hook: Value = serde_json::from_slice(input).ok()?;
    let said = |key: &str| hook.get(key).and_then(Value::as_str);
    if said("hook_event_name") != Some("PermissionRequest") {
        return None;
    }
    let tool = said("tool_name")?;
    // A subagent's: Claude Code puts its `agent_id` in every hook it runs
    // for one, and an id that is not a string is no promise it is not.
    match hook.get("agent_id") {
        None | Some(Value::Null) => {}
        Some(Value::String(id)) if id.trim().is_empty() => {}
        Some(_) => return None,
    }
    let session = said("session_id").and_then(crook_terminal::agent::session_id)?;
    let cwd = said("cwd").filter(|cwd| cwd.starts_with('/') && cwd.len() <= MAX_CWD)?;
    if environment.entrypoint.as_deref() != Some("cli") {
        return None;
    }
    let request = from_claude(tool, hook.get("tool_input")?)?;
    let (tty, multiplexer) = match (environment.multiplexer, terminal()?) {
        (None, Bound::Tty(tty)) => (Some(tty), None),
        (Some(Multiplexed::Tmux), Bound::Multiplexer(multiplexer)) => (None, Some(multiplexer)),
        _ => return None,
    };
    let request = match request.path() {
        Some(_) => {
            let on_disk = look(&request, cwd)?;
            request.seen(on_disk)
        }
        None => request,
    };
    Some(Question {
        socket,
        // The token tmux keeps is the pane's that started its server, which
        // says nothing about the pane the question is for.
        token: environment.token.clone().filter(|_| multiplexer.is_none()),
        asked: AgentAsk {
            agent: AskingAgent::Claude,
            session: session.to_owned(),
            cwd: cwd.to_owned(),
            tty,
            multiplexer,
            wait: CLAUDE_AWAIT.as_secs(),
            request,
        },
    })
}

/// How long the hook waits for each thing, and how often it looks up.
#[cfg(any(unix, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Patience {
    /// For the window to say it holds the question.
    pub register: Duration,
    /// For the answer, once it does.
    pub answer: Duration,
    /// Between two looks at whether its parent is still there.
    pub poll: Duration,
    /// Inside tmux, for a window to say it holds one of the clients'
    /// terminals, all of the windows asked together.
    pub find: Duration,
}

#[cfg(any(unix, test))]
impl Patience {
    /// A Claude Code hook's.
    #[cfg(unix)]
    pub const CLAUDE: Self = Self {
        register: REGISTER_DEADLINE,
        answer: CLAUDE_AWAIT.saturating_add(PAST_THE_AWAIT),
        poll: POLL,
        find: FIND_DEADLINE,
    };
}

/// What one read of the window's half found.
#[cfg(any(unix, test))]
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// A line, its newline included when it had one.
    Read(Vec<u8>),
    /// Nothing yet.
    Waiting,
    /// The window closed its end.
    Closed,
}

/// The hook's end of a connection to the window: what [`converse`] needs,
/// and what a test stands in for.
#[cfg(any(unix, test))]
pub trait Connection {
    /// Sends `line`, whole.
    fn send(&mut self, line: &[u8]) -> io::Result<()>;

    /// The next line, waiting for one at most `timeout`.
    fn line(&mut self, timeout: Duration) -> io::Result<Line>;
}

/// The windows a hook can ask: what [`decide`] connects through, and what a
/// test stands in for.
#[cfg(any(unix, test))]
pub trait Windows {
    /// A connection to one.
    type Connection: Connection;

    /// A connection to the window listening at `socket`, once the socket and
    /// the listener have passed every check; `None` otherwise.
    fn connect(&mut self, socket: &Path) -> Option<Self::Connection>;

    /// The other sockets in this user's checked control directory, besides
    /// `socket`, in name order: where a hook inside tmux looks when
    /// `socket`'s window holds none of its clients' terminals.
    fn others(&mut self, socket: &Path) -> Vec<PathBuf>;
}

/// Only the one socket: a connection made by the closure, and no others.
#[cfg(any(unix, test))]
impl<C: Connection, F: FnMut(&Path) -> Option<C>> Windows for F {
    type Connection = C;

    fn connect(&mut self, socket: &Path) -> Option<C> {
        self(socket)
    }

    fn others(&mut self, _: &Path) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// Asks the question in `request` over `connection`, and hands back what the
/// window said — or `None` for every way it can say nothing: no reply that
/// it holds the question within [`Patience::register`], a refusal, no
/// answer by [`Patience::answer`], the connection closing, a line that is
/// not an answer, or `parent_left` saying the hook's parent has gone, which
/// it is asked before every wait.
#[cfg(any(unix, test))]
pub fn converse(
    connection: &mut impl Connection,
    request: &str,
    patience: &Patience,
    parent_left: impl Fn() -> bool,
) -> Option<Verdict> {
    connection.send(request.as_bytes()).ok()?;
    let first = next_line(
        connection,
        Instant::now() + patience.register,
        patience,
        &parent_left,
    )?;
    let reply: Reply = serde_json::from_slice(&first).ok()?;
    match (reply.ok, reply.result) {
        (true, Some(result)) => serde_json::from_value::<Registered>(result).ok()?,
        _ => return None,
    };
    let answer = next_line(
        connection,
        Instant::now() + patience.answer,
        patience,
        &parent_left,
    )?;
    serde_json::from_slice::<Answered>(&answer)
        .ok()
        .map(|answered| answered.answer)
}

/// The next line by `deadline`, looking at `parent_left` at least every
/// [`Patience::poll`]; `None` when the deadline passes, the parent goes, or
/// the connection ends or fails first.
#[cfg(any(unix, test))]
fn next_line(
    connection: &mut impl Connection,
    deadline: Instant,
    patience: &Patience,
    parent_left: &impl Fn() -> bool,
) -> Option<Vec<u8>> {
    loop {
        if parent_left() {
            return None;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match connection.line(left.min(patience.poll)).ok()? {
            Line::Read(line) => return Some(line),
            Line::Waiting => {}
            Line::Closed => return None,
        }
    }
}

/// What the hook prints for `verdict`: one line Claude Code reads as the
/// decision, or `None` for a pass.
///
/// Only the behaviour, and for a deny the window's message: never anything
/// that would change the request, write a rule, or stop the agent's turn.
#[cfg(any(unix, test))]
pub fn decision_line(verdict: &Verdict) -> Option<String> {
    match verdict.decision {
        Decision::Allow => Some(ALLOW.to_owned()),
        Decision::Deny => {
            let message = verdict
                .message
                .as_ref()
                .map(|message| format!(",\"message\":{}", Value::String(message.clone())))
                .unwrap_or_default();
            Some(format!(
                "{{\"hookSpecificOutput\":{{\"hookEventName\":\"PermissionRequest\",\"decision\":{{\"behavior\":\"deny\"{message}}}}}}}\n"
            ))
        }
        Decision::Pass => None,
    }
}

/// Everything the hook does before it prints: the question, `terminal` and
/// `look` asked as [`question`] asks them, the connection `windows` makes to
/// the window it goes to, and the line to print with the connection it came
/// down — or `None`, having connected to nothing unless every rule held.
///
/// The window is the socket the question names; inside tmux, the first of
/// it and the others `windows` lists to say it holds one of the clients'
/// terminals — see [`find`] — and none when none does.
#[cfg(any(unix, test))]
pub fn decide<W: Windows>(
    input: &[u8],
    environment: &Environment,
    terminal: impl FnOnce() -> Option<Bound>,
    look: impl FnOnce(&AskRequest, &str) -> Option<OnDisk>,
    mut windows: W,
    patience: &Patience,
    parent_left: impl Fn() -> bool,
) -> Option<(String, W::Connection)> {
    let Question {
        socket,
        token,
        asked,
    } = question(input, environment, terminal, look)?;
    let clients = asked
        .multiplexer
        .as_ref()
        .map(|multiplexer| multiplexer.clients.clone());
    let line = protocol::request_line(&Verb::AgentAsk(asked), token.as_deref());
    // Not asked rather than cut: the window reads no more than this, and a
    // request it could not show whole is no question.
    if line.len() > protocol::MAX_LINE + 1 {
        return None;
    }
    let mut connection = match clients {
        None => windows.connect(&socket)?,
        Some(clients) => find(&mut windows, &socket, clients, patience, &parent_left)?,
    };
    let verdict = converse(&mut connection, &line, patience, parent_left)?;
    Some((decision_line(&verdict)?, connection))
}

/// The connection to the first window that says it holds one of `ttys`:
/// `socket`'s, else one of the first [`MAX_OTHER_WINDOWS`] others `windows`
/// lists that can be connected to — or `None`, when none says so within
/// [`Patience::find`].
///
/// What each is asked is `pane.find`, which carries the terminals and no
/// more: nothing of the question, and no token. A window that is gone,
/// refuses, says no or says nothing in time is passed over, and its
/// connection closed.
#[cfg(any(unix, test))]
pub fn find<W: Windows>(
    windows: &mut W,
    socket: &Path,
    ttys: Vec<u32>,
    patience: &Patience,
    parent_left: &impl Fn() -> bool,
) -> Option<W::Connection> {
    let line = protocol::request_line(&Verb::PaneFind(Find { ttys }), None);
    let deadline = Instant::now() + patience.find;
    let ask = |connection: &mut W::Connection| {
        connection.send(line.as_bytes()).ok()?;
        let by = deadline.min(Instant::now() + patience.register);
        let reply: Reply =
            serde_json::from_slice(&next_line(connection, by, patience, parent_left)?).ok()?;
        let located: Located = serde_json::from_value(reply.result.filter(|_| reply.ok)?).ok()?;
        located.found.then_some(())
    };
    if let Some(mut connection) = windows.connect(socket)
        && ask(&mut connection).is_some()
    {
        return Some(connection);
    }
    let mut asked = 0;
    for other in windows.others(socket) {
        if asked == MAX_OTHER_WINDOWS || Instant::now() >= deadline {
            break;
        }
        let Some(mut connection) = windows.connect(&other) else {
            continue;
        };
        asked += 1;
        if ask(&mut connection).is_some() {
            return Some(connection);
        }
    }
    None
}

/// Prints `line` in one write, and says whether it all went: a reader that
/// has gone — EPIPE — is no answer printed, and nothing to say.
#[cfg(unix)]
pub fn print(out: &mut impl Write, line: &str) -> bool {
    out.write_all(line.as_bytes())
        .and_then(|()| out.flush())
        .is_ok()
}

/// `crook --agent ask`, given what follows `ask`: asks the window, prints
/// what it says if anything, and returns — never with an error.
///
/// Only `claude` is an agent this asks for. Anything else, or anything after
/// it, is one line on standard error and nothing on standard output, so that
/// a hook a newer Crook wrote reaches an older one as a pass.
pub fn run(mut args: impl Iterator<Item = String>) {
    let agent = args.next();
    let extra = args.next();
    if agent.as_deref() != Some(AskingAgent::Claude.word()) || extra.is_some() {
        let said = match (agent, extra) {
            (Some(agent), None) => format!("not {agent}"),
            (None, _) => "nothing".to_owned(),
            (Some(_), Some(extra)) => format!("nothing after it, not {extra}"),
        };
        let _ = writeln!(
            io::stderr().lock(),
            "crook: `--agent ask` takes claude, {said}; this hook asks nothing"
        );
    } else {
        #[cfg(unix)]
        unix::run();
    }
}

/// The socket half, which only Unix has.
#[cfg(unix)]
mod unix {
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::{
        Bound, Connection, Environment, Line, MAX_INPUT, Multiplexed, Patience, REGISTER_DEADLINE,
        Windows,
    };
    use crate::control::cli::unix::{connect_checked_within, sockets_in};

    /// The longest line read from the window, in bytes.
    const MAX_REPLY: usize = 64 * 1024;

    /// What the hook tells the window once it has printed its answer.
    const DELIVERED: &[u8] = b"{\"delivered\":true}\n";

    /// The hook, here: this process's input, environment and terminal.
    pub fn run() {
        let parent = std::os::unix::process::parent_id();
        let Some(input) = input(io::stdin().lock()) else {
            return;
        };
        let environment = Environment::here();
        let decided = super::decide(
            &input,
            &environment,
            || match environment.multiplexer {
                None => device().map(Bound::Tty),
                Some(Multiplexed::Tmux) => inside_tmux().map(Bound::Multiplexer),
                Some(Multiplexed::Other) => None,
            },
            crate::agent::on_disk::look,
            Sockets,
            &Patience::CLAUDE,
            || std::os::unix::process::parent_id() != parent,
        );
        let Some((line, mut connection)) = decided else {
            return;
        };
        if super::print(&mut io::stdout().lock(), &line) {
            // Best effort: the answer is printed either way, and the window
            // only remembers whether it heard so.
            let _ = connection.send(DELIVERED);
        }
    }

    /// Standard input, up to [`MAX_INPUT`]; `None` past it, or when it cannot
    /// be read.
    fn input(from: impl Read) -> Option<Vec<u8>> {
        let mut input = Vec::new();
        from.take(MAX_INPUT as u64 + 1)
            .read_to_end(&mut input)
            .ok()?;
        (input.len() <= MAX_INPUT).then_some(input)
    }

    /// The device number of the terminal a status from here would go to.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn device() -> Option<u32> {
        crate::agent::ancestors::device()
    }

    /// Nowhere else is the process table read, so no pane can be named.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn device() -> Option<u32> {
        None
    }

    /// Inside tmux, the terminals of the clients attached to the agent's
    /// session that are this user's, at most
    /// [`MAX_CLIENTS`](crate::control::protocol::MAX_CLIENTS), and the
    /// agent's pane as tmux names it; `None` unless tmux says this process
    /// is on the pane `TMUX_PANE` names and has such a client.
    ///
    /// The terminal of another user's client — one attached to a server
    /// shared with `server-access` — is no pane of this user's windows, and
    /// is left out rather than named to them.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn inside_tmux() -> Option<crate::control::protocol::Multiplexer> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};

        use crate::control::protocol::{self, MultiplexerKind};

        let pane = std::env::var("TMUX_PANE")
            .ok()
            .filter(|pane| protocol::is_pane_id(pane))?;
        let user = crate::control::server::euid();
        let mut clients = Vec::new();
        for client in crate::agent::tmux::clients()? {
            let Ok(metadata) = std::fs::metadata(&client) else {
                continue;
            };
            // The low 32 bits, as the window reads its ptys'.
            let device = metadata.rdev() as u32;
            if metadata.file_type().is_char_device()
                && metadata.uid() == user
                && !clients.contains(&device)
            {
                clients.push(device);
            }
        }
        clients.truncate(protocol::MAX_CLIENTS);
        (!clients.is_empty()).then_some(protocol::Multiplexer {
            kind: MultiplexerKind::Tmux,
            clients,
            pane,
        })
    }

    /// Nowhere else is tmux asked where its clients are.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn inside_tmux() -> Option<crate::control::protocol::Multiplexer> {
        None
    }

    /// The windows of this user: a socket asked once it has passed the
    /// client's checks, and the others in the control directory.
    struct Sockets;

    impl Windows for Sockets {
        type Connection = Socketed;

        fn connect(&mut self, socket: &Path) -> Option<Socketed> {
            connect_checked_within(socket, super::CONNECT_DEADLINE)
                .ok()
                .map(Socketed::new)
        }

        fn others(&mut self, socket: &Path) -> Vec<PathBuf> {
            let mut others = sockets_in(&crate::control::server::directory());
            others.retain(|other| other != socket);
            others
        }
    }

    /// A connection to the window's socket.
    struct Socketed {
        lines: BufReader<UnixStream>,
        /// What has come of a line that has not ended yet.
        partial: Vec<u8>,
    }

    impl Socketed {
        fn new(stream: UnixStream) -> Self {
            Self {
                lines: BufReader::new(stream),
                partial: Vec::new(),
            }
        }
    }

    impl Connection for Socketed {
        fn send(&mut self, line: &[u8]) -> io::Result<()> {
            let mut stream = self.lines.get_ref();
            stream.set_write_timeout(Some(REGISTER_DEADLINE))?;
            stream.write_all(line)
        }

        fn line(&mut self, timeout: Duration) -> io::Result<Line> {
            self.lines
                .get_ref()
                .set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
            loop {
                let room = (MAX_REPLY + 1).saturating_sub(self.partial.len()) as u64;
                match (&mut self.lines)
                    .take(room)
                    .read_until(b'\n', &mut self.partial)
                {
                    Ok(0) if self.partial.is_empty() => return Ok(Line::Closed),
                    // A line that ended, one cut short by the end of the
                    // connection, or one longer than any answer — which
                    // reads as no answer.
                    Ok(_) => return Ok(Line::Read(std::mem::take(&mut self.partial))),
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        return Ok(Line::Waiting);
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "ask_tests.rs"]
mod tests;
