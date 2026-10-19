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
//! by `super::ancestors`, and nothing else is looked at.
//!
//! # When it asks nothing
//!
//! Before it connects anywhere, unless all of these hold: `CROOK_SOCKET` names
//! a window; no `TMUX`, `STY` or `ZELLIJ` says a multiplexer stands between
//! the agent and the pane; the input is Claude Code's `PermissionRequest` for
//! `Bash`, `Edit`, `MultiEdit` or `Write`, from the main thread and not a
//! subagent, with a session and an absolute working directory;
//! `CLAUDE_CODE_ENTRYPOINT` is exactly `cli` —
//! under `claude -p` an empty answer is Claude Code's own immediate deny, and
//! holding it would turn a refusal into a wait; the tool's input is one the
//! allowlist in [`super::request`] reads whole; and a terminal is found.
//!
//! The socket is asked only once it has been checked, as every client's is
//! — see `control::cli` — and the connection is never half closed: on macOS
//! the window cannot tell a closed half from a hook that has gone.
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
use super::request::from_claude;
use crate::control::protocol::AskingAgent;
#[cfg(any(unix, test))]
use crate::control::protocol::{
    self, AgentAsk, Answered, Decision, MAX_CWD, Registered, Reply, Verb, Verdict,
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
    /// Whether `TMUX`, `STY` or `ZELLIJ` names a multiplexer.
    pub multiplexed: bool,
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
            multiplexed: ["TMUX", "STY", "ZELLIJ"].into_iter().any(named),
            entrypoint: std::env::var("CLAUDE_CODE_ENTRYPOINT").ok(),
        }
    }
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
/// asked last, and only when every other rule holds.
#[cfg(any(unix, test))]
pub fn question(
    input: &[u8],
    environment: &Environment,
    terminal: impl FnOnce() -> Option<u32>,
) -> Option<Question> {
    let socket = environment.socket.clone()?;
    if environment.multiplexed {
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
    let tty = terminal()?;
    Some(Question {
        socket,
        token: environment.token.clone(),
        asked: AgentAsk {
            agent: AskingAgent::Claude,
            session: session.to_owned(),
            cwd: cwd.to_owned(),
            tty,
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
}

#[cfg(any(unix, test))]
impl Patience {
    /// A Claude Code hook's.
    #[cfg(unix)]
    pub const CLAUDE: Self = Self {
        register: REGISTER_DEADLINE,
        answer: CLAUDE_AWAIT.saturating_add(PAST_THE_AWAIT),
        poll: POLL,
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

/// Everything the hook does before it prints: the question, the connection
/// `connect` makes to the socket it names, and the line to print with the
/// connection it came down — or `None`, having connected to nothing unless
/// every rule held.
#[cfg(any(unix, test))]
pub fn decide<C: Connection>(
    input: &[u8],
    environment: &Environment,
    terminal: impl FnOnce() -> Option<u32>,
    connect: impl FnOnce(&Path) -> Option<C>,
    patience: &Patience,
    parent_left: impl Fn() -> bool,
) -> Option<(String, C)> {
    let Question {
        socket,
        token,
        asked,
    } = question(input, environment, terminal)?;
    let line = protocol::request_line(&Verb::AgentAsk(asked), token.as_deref());
    // Not asked rather than cut: the window reads no more than this, and a
    // request it could not show whole is no question.
    if line.len() > protocol::MAX_LINE + 1 {
        return None;
    }
    let mut connection = connect(&socket)?;
    let verdict = converse(&mut connection, &line, patience, parent_left)?;
    Some((decision_line(&verdict)?, connection))
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
    use std::time::Duration;

    use super::{Connection, Environment, Line, MAX_INPUT, Patience, REGISTER_DEADLINE};
    use crate::control::cli::unix::connect_checked;

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
        let decided = super::decide(
            &input,
            &Environment::here(),
            device,
            |socket| connect_checked(socket).ok().map(Socketed::new),
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
