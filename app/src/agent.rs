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
//! would be read as an answer and never reach the screen.
//!
//! `--agent-hooks claude` prints the hooks that make Claude Code say all of
//! this by itself: running when a prompt is sent and while tools run,
//! needing input when it stops to ask — with what it is asking, read out of
//! the notification's own text — idle when it is done. What it prints
//! is a fragment of Claude Code's own settings file, to be merged into it by
//! the person whose file it is — Crook does not write a file it does not own,
//! and that one it has never opened. `codex`, `gemini` and `copilot` print
//! the same object under each one's own event names, which is how those
//! three read their hooks too; `opencode` has no command hooks and gets the
//! plugin its plugin directory loads instead; `aider` has no hooks, and gets
//! the sentence that says so and what to do instead.
//!
//! `--skill` prints [`SKILL`], the file that teaches an agent the rest of
//! this: how it tells it is in a pane, what the four words do to the row,
//! and what else the binary will do for it. A hook makes Claude Code report
//! without knowing it is; the skill is for an agent a person has asked to
//! know.
//!
//! The same table is what the worktree creator offers to start in a new
//! checkout: which of these agents are installed — [`found_on`], a look in
//! each directory of `PATH` for the program, which runs nothing — and the
//! line that starts one on a prompt, [`launch_line`], with the prompt quoted
//! by the host rather than pasted in. That line is typed into the new pane's
//! composer and left there, unsent, unless the person pressed Start.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use crook_terminal::AgentReport;
use serde_json::{Value, json};

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
pub fn report(status: &str, title: Option<&str>, message: Option<&str>) -> Result<()> {
    let status = AgentReport::parse(status).with_context(|| {
        let words: Vec<_> = AgentReport::ALL.iter().map(|word| word.word()).collect();
        format!("`--agent` takes one of {}, not {status}", words.join(", "))
    })?;

    // Read once, whichever of the two asked for it: stdin has one reading in
    // it, and `--title - --message -` would otherwise hand the second an
    // empty string.
    let input = if title == Some("-") || message == Some("-") {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .context("`-` reads the hook's input from stdin, and it could not be read")?;
        Some(input)
    } else {
        None
    };
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

    let mut terminal = terminal().context(
        "`--agent` writes to the terminal this was run in, and there is none: run it from a pane, or from a hook of a program in one",
    )?;
    terminal
        .write_all(
            crook_terminal::agent::report(status, title.as_deref(), message.as_deref()).as_bytes(),
        )
        .and_then(|()| terminal.flush())
        .context("could not write to the terminal")
}

/// The terminal this process is attached to, opened for writing.
///
/// Not standard output, which a hook's parent has taken; the controlling
/// terminal, which is the pane. A process with none — `cron`, a CI runner, a
/// detached service — has nowhere to report to, and says so.
fn terminal() -> io::Result<std::fs::File> {
    #[cfg(unix)]
    let path = "/dev/tty";
    #[cfg(windows)]
    let path = "CONOUT$";
    OpenOptions::new().write(true).open(path)
}

/// The title a hook's input names: the first line of its `prompt`, cut to
/// a row's width.
///
/// Claude Code hands every hook a JSON object on standard input, and the one
/// for a prompt being sent carries the prompt under `prompt`. Anything else
/// — no JSON, no such field, an empty line — is no title, and the status
/// goes without one rather than with one that says `{`.
fn title_from_hook(input: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(input).ok()?;
    let prompt = parsed.get("prompt")?.as_str()?;
    prompt.lines().find_map(presentable)
}

/// The message on standard input: a hook's `message`, else the tool a
/// permission request names, else the input itself.
///
/// Claude Code's Notification hook is handed a JSON object whose `message`
/// is the notification's text — "Claude needs your permission to use Bash"
/// — which is exactly what the row wants to say. Codex's PermissionRequest
/// hook is handed the tool and its input and no text, so the row says
/// which tool. Input that is not JSON is taken whole, so `echo "approve the
/// deploy?" | crook --agent needs-input --message -` works from a script
/// with no JSON to hand; JSON with neither field is not a message, and the
/// status goes without one rather than with a line of braces.
fn message_from_hook(input: &str) -> Option<String> {
    match serde_json::from_str::<Value>(input) {
        Ok(parsed) => match parsed.get("message").and_then(Value::as_str) {
            Some(message) => presentable_message(message),
            None => parsed
                .get("tool_name")
                .and_then(Value::as_str)
                .and_then(|tool| presentable_message(&format!("permission to use {tool}"))),
        },
        Err(_) => presentable_message(input),
    }
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
fn one_line(text: &str, chars: usize) -> Option<String> {
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
    /// The word on the command line, which is also the program's own name on
    /// `PATH`.
    name: &'static str,
    /// What the program calls itself.
    program: &'static str,
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
/// needing input on every notification, idle on stop.
///
/// The prompt is the one moment the work has a name. Before a tool as well
/// as after it: a permission prompt comes between the two, and only the
/// second says it was answered. Every notification Claude Code sends is one
/// that wants a person — a permission to give, a question to answer, a long
/// idle at its prompt — and its text says which, so the row says it.
const CLAUDE_EVENTS: &[(&str, &str)] = &[
    ("UserPromptSubmit", "running --title -"),
    ("PreToolUse", "running"),
    ("PostToolUse", "running"),
    ("Notification", "needs-input --message -"),
    ("Stop", "idle"),
    ("SessionEnd", "idle"),
];

/// The Codex CLI hooks: the same object as Claude Code's, in
/// `~/.codex/hooks.json`, with the same event names but one.
///
/// Codex has no `Notification`; what it has is `PermissionRequest`, run
/// before an approval prompt, whose input names the tool and not a message
/// — [`message_from_hook`] says "permission to use" the tool then. A
/// question the model asks ends its turn, which is `Stop`. Its older
/// `notify` setting runs a program with a JSON argument when a turn ends and
/// nothing else, and is the legacy of these; the hooks say more.
const CODEX_EVENTS: &[(&str, &str)] = &[
    ("UserPromptSubmit", "running --title -"),
    ("PreToolUse", "running"),
    ("PostToolUse", "running"),
    ("PermissionRequest", "needs-input --message -"),
    ("Stop", "idle"),
    ("SessionEnd", "idle"),
];

/// The Gemini CLI hooks: the same object, in `~/.gemini/settings.json`,
/// under its own names — `BeforeAgent` for the prompt, `BeforeTool` and
/// `AfterTool` around a tool, `AfterAgent` for the end of a turn.
///
/// Its `Notification` fires for a tool permission and carries a `message`,
/// the way Claude Code's does; a question the model asks is the end of a
/// turn, which is `AfterAgent`.
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
const COPILOT_EVENTS: &[(&str, &str)] = &[
    ("userPromptSubmitted", "running --title -"),
    ("preToolUse", "running"),
    ("postToolUse", "running"),
    ("notification", "needs-input --message -"),
    ("agentStop", "idle"),
    ("sessionEnd", "idle"),
];

/// The OpenCode plugin: a TypeScript file its plugin directory loads at
/// startup, since OpenCode has no command hooks in its settings.
///
/// A plugin is a function handed the Bun shell and returning the hooks it
/// wants: `chat.message` for a prompt, with the message's own parts to
/// name the work from, and `event` for the bus — `session.status` says busy
/// or idle, `permission.asked` and `question.asked` are the two ways it
/// stops for a person, and their replies are when it goes on. The binary is
/// spliced in as a string literal, because a plugin runs in whatever `PATH`
/// OpenCode was started with and a development build is on nobody's.
/// `.quiet().nothrow()`: a report that could not be written — no terminal,
/// a pane that is not Crook's — is nothing a plugin should throw over.
const OPENCODE_PLUGIN: &str = r#"// Crook: tell the tab this session runs in what OpenCode is doing.
// Save as ~/.config/opencode/plugins/crook.ts, or as a project's
// .opencode/plugins/crook.ts. Reports go to the terminal OpenCode was
// started in, so this is for `opencode` run in a pane, not `opencode serve`.
const crook = BINARY

export const CrookPlugin = async ({ $ }) => {
  const report = (status, flag, text) =>
    (text === undefined
      ? $`${crook} --agent ${status}`
      : $`${crook} --agent ${status} ${flag} ${text}`
    ).quiet().nothrow()
  return {
    "chat.message": async (_input, output) => {
      const text = output.parts.find((part) => part.type === "text")?.text ?? ""
      const title = text.split("\n").find((line) => line.trim() !== "") ?? ""
      await report("running", "--title", title)
    },
    event: async ({ event }) => {
      switch (event.type) {
        case "session.status":
          if (event.properties.status.type === "busy") await report("running")
          break
        case "permission.asked":
          await report("needs-input", "--message", `${event.properties.permission}: ${event.properties.patterns.join(", ")}`)
          break
        case "question.asked":
          await report("needs-input", "--message", event.properties.questions[0]?.question ?? "")
          break
        case "permission.replied":
        case "question.replied":
          await report("running")
          break
        case "session.idle":
          await report("idle")
          break
      }
    },
  }
}
"#;

/// The agents, in the order the errors list them.
const AGENTS: &[Agent] = &[
    Agent {
        name: "claude",
        program: "Claude Code",
        fragment: Fragment::Hooks {
            file: "~/.claude/settings.json, or a project's .claude/settings.json",
            events: CLAUDE_EVENTS,
            missing: "",
        },
        prompt_flag: None,
    },
    Agent {
        name: "codex",
        program: "Codex CLI",
        fragment: Fragment::Hooks {
            file: "~/.codex/hooks.json, or a project's .codex/hooks.json",
            events: CODEX_EVENTS,
            missing: " Codex has no notification hook: the row says needs-input for a \
permission, and a question the model asks ends its turn as idle.",
        },
        prompt_flag: None,
    },
    Agent {
        name: "gemini",
        program: "Gemini CLI",
        fragment: Fragment::Hooks {
            file: "~/.gemini/settings.json, or a project's .gemini/settings.json",
            events: GEMINI_EVENTS,
            missing: " Gemini CLI notifies for a tool permission only: a question the \
model asks ends its turn as idle.",
        },
        prompt_flag: Some("-i"),
    },
    Agent {
        name: "copilot",
        program: "GitHub Copilot CLI",
        fragment: Fragment::Copilot {
            file: "~/.copilot/hooks/crook.json, or a project's .github/hooks/crook.json",
            events: COPILOT_EVENTS,
        },
        prompt_flag: Some("-i"),
    },
    Agent {
        name: "opencode",
        program: "OpenCode",
        fragment: Fragment::Plugin {
            file: "~/.config/opencode/plugins/crook.ts, or a project's .opencode/plugins/crook.ts",
            source: OPENCODE_PLUGIN,
        },
        prompt_flag: Some("--prompt"),
    },
    Agent {
        name: "aider",
        program: "aider",
        fragment: Fragment::None(
            "aider has no hooks. Its `--notifications-command` runs one command when a turn \
ends and it waits for you, so `aider --notifications --notifications-command \"BINARY \
--agent idle\"` reports idle there and nothing else; for running, start it from a \
wrapper script that runs `BINARY --agent running` first.",
        ),
        prompt_flag: Some("--message"),
    },
];

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
pub fn found_on(directories: &[PathBuf]) -> Vec<&'static str> {
    AGENTS
        .iter()
        .map(|agent| agent.name)
        .filter(|name| {
            directories
                .iter()
                .any(|directory| runnable(directory, name))
        })
        .collect()
}

/// Where a running window looks for the agents: `PATH`, then the
/// directories their installers put them in.
///
/// The second half is for macOS, where an application started from the
/// Dock or the Finder has launchd's `PATH` — `/usr/bin:/bin:/usr/sbin:/sbin`
/// — and every agent is somewhere else: Homebrew's prefix, `/usr/local/bin`,
/// or `~/.local/bin`, which is where Claude Code's own installer puts it. The
/// pane's login shell has those on its `PATH`, which is the one the line
/// runs under; the window's own does not. Elsewhere they are usually on
/// `PATH` already, and a directory that is not there is simply not looked in.
pub fn search_path() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if cfg!(unix) {
        if let Some(home) = dirs::home_dir() {
            directories.push(home.join(".local").join("bin"));
        }
        directories.push(PathBuf::from("/opt/homebrew/bin"));
        directories.push(PathBuf::from("/usr/local/bin"));
    }
    directories
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
/// The binary is named by its full path, because a hook runs in whatever
/// `PATH` the agent was started with and a development build is on nobody's.
/// The fragment is printed rather than installed: Crook does not write a file
/// it does not own, and it has never opened any of these.
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
    let run = |arguments: &str| format!("{} --agent {arguments}", quoted(binary));
    let (text, note) = match known.fragment {
        Fragment::Hooks {
            file,
            events,
            missing,
        } => {
            let hooks: serde_json::Map<String, Value> = events
                .iter()
                .map(|(event, arguments)| {
                    let hook =
                        json!([{ "hooks": [{ "type": "command", "command": run(arguments) }] }]);
                    ((*event).to_owned(), hook)
                })
                .collect();
            let text = serde_json::to_string_pretty(&json!({ "hooks": hooks }))
                .context("could not write the hooks as JSON")?;
            let note = format!(
                "# Merge the `hooks` above into {file}. {} then tells the tab it runs in what it \
is doing.{missing}",
                known.program
            );
            (text, Some(note))
        }
        Fragment::Copilot { file, events } => {
            let hooks: serde_json::Map<String, Value> = events
                .iter()
                .map(|(event, arguments)| {
                    let hook = json!([{ "type": "command", "bash": run(arguments) }]);
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
    Ok(Hooks { text, note })
}

/// `path`, quoted for the shell a hook runs in.
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
    fn a_hook_input_with_no_prompt_names_nothing() {
        assert_eq!(None, title_from_hook("not json"));
        assert_eq!(None, title_from_hook(r#"{"tool_name": "Bash"}"#));
        assert_eq!(None, title_from_hook(r#"{"prompt": "   \n  "}"#));
    }

    #[test]
    fn the_hooks_name_the_binary_by_path_and_cover_every_moment() {
        let printed = hooks_text("claude", Path::new("/Applications/Crook's.app/crook")).unwrap();
        let parsed: Value = serde_json::from_str(&printed.text).expect("the fragment is JSON");
        let hooks = parsed["hooks"].as_object().unwrap();
        for event in [
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Notification",
            "Stop",
            "SessionEnd",
        ] {
            let command = hooks[event][0]["hooks"][0]["command"].as_str().unwrap();
            assert!(
                command.starts_with("'/Applications/Crook'\\''s.app/crook' --agent "),
                "{event} runs {command}"
            );
        }
        // The notification's text is what the agent is waiting for, and it
        // comes in on stdin: the hook pipes it through rather than needing
        // a `jq` on the machine to pick it out.
        assert!(
            hooks["Notification"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("needs-input --message -")
        );
        assert!(
            hooks["UserPromptSubmit"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("running --title -")
        );
        assert!(
            printed.note.unwrap().contains("~/.claude/settings.json"),
            "a person merging the fragment has to be told where it goes"
        );
    }

    #[test]
    fn every_known_agent_prints_its_own_fragment_with_the_binary_in_it() {
        let binary = Path::new("/opt/crook/bin/crook");
        // Codex reads Claude Code's object with one event of its own.
        let codex = hooks_text("codex", binary).unwrap();
        let parsed: Value = serde_json::from_str(&codex.text).expect("the fragment is JSON");
        let command = parsed["hooks"]["PermissionRequest"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(
            "'/opt/crook/bin/crook' --agent needs-input --message -",
            command
        );
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
            "'/opt/crook/bin/crook' --agent idle",
            parsed["hooks"]["agentStop"][0]["bash"].as_str().unwrap()
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
        for event in [
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
        assert!(
            opencode
                .note
                .unwrap()
                .contains("~/.config/opencode/plugins/crook.ts")
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
                    // word is in the `report("...")` call that fills it.
                    .filter(|word| !word.is_empty())
                    .collect::<Vec<_>>()
            };
            let statuses = [word_after("--agent "), word_after("report(\"")].concat();
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
        assert_eq!(6, hooks.len(), "a hook was added or dropped: {hooks:?}");
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

    #[test]
    fn a_word_that_is_not_a_status_is_refused_with_the_words_that_are() {
        let error = report("sleeping", None, None).unwrap_err();
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
            std::fs::write(&path, "#!/bin/sh\nprintf '%s\\n' \"$#\" \"$1\"\n")
                .expect("the fake agent is written");
            let mode = if runnable { 0o755 } else { 0o644 };
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .expect("the fake agent's mode is set");
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
        assert_eq!(found_on(&directories), ["claude", "opencode"]);
        // In the table's order rather than the directories'.
        assert_eq!(
            found_on(&[second.clone(), first.clone()]),
            ["claude", "opencode"]
        );
        assert!(found_on(&[]).is_empty());
        assert!(found_on(&[scratch.0.join("not-there")]).is_empty());
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
