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
//! and that one it has never opened.
//!
//! `--skill` prints [`SKILL`], the file that teaches an agent the rest of
//! this: how it tells it is in a pane, what the four words do to the row,
//! and what else the binary will do for it. A hook makes Claude Code report
//! without knowing it is; the skill is for an agent a person has asked to
//! know.

use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::Path;

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

/// The message on standard input: a hook's `message`, else the input itself.
///
/// Claude Code's Notification hook is handed a JSON object whose `message`
/// is the notification's text — "Claude needs your permission to use Bash"
/// — which is exactly what the row wants to say. Input that is not that
/// shape is taken whole, so `echo "approve the deploy?" | crook --agent
/// needs-input --message -` works from a script with no JSON to hand;
/// JSON with no `message` in it is not a message, and the status goes
/// without one rather than with a line of braces.
fn message_from_hook(input: &str) -> Option<String> {
    match serde_json::from_str::<Value>(input) {
        Ok(parsed) => parsed
            .get("message")?
            .as_str()
            .and_then(presentable_message),
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

/// The hooks that make `agent` report itself, as a fragment of its settings.
///
/// One agent is known: `claude`, for Claude Code, whose hooks are a JSON
/// object under `hooks` in `~/.claude/settings.json`. The binary is named by
/// its full path, because a hook runs in whatever `PATH` Claude Code was
/// started with and a development build is on nobody's.
pub fn hooks_text(agent: &str, binary: &Path) -> Result<String> {
    if agent != "claude" {
        bail!("Crook has hooks for `claude` (Claude Code), not for {agent}");
    }
    let run = |arguments: &str| format!("{} --agent {arguments}", quoted(binary));
    let hook =
        |arguments: &str| json!([{ "hooks": [{ "type": "command", "command": run(arguments) }] }]);
    let fragment = json!({
        "hooks": {
            // The prompt is the one moment the work has a name.
            "UserPromptSubmit": hook("running --title -"),
            // Before a tool as well as after it: a permission prompt comes
            // between the two, and only the second says it was answered.
            "PreToolUse": hook("running"),
            "PostToolUse": hook("running"),
            // Every notification Claude Code sends is one that wants a
            // person: a permission to give, a question to answer, a long
            // idle at its prompt. Its text says which, and the row says it.
            "Notification": hook("needs-input --message -"),
            "Stop": hook("idle"),
            "SessionEnd": hook("idle"),
        }
    });
    serde_json::to_string_pretty(&fragment).context("could not write the hooks as JSON")
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
        // JSON that is not a hook's says nothing, rather than `{`.
        assert_eq!(None, message_from_hook(r#"{"tool_name": "Bash"}"#));
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
        let text = hooks_text("claude", Path::new("/Applications/Crook's.app/crook")).unwrap();
        let parsed: Value = serde_json::from_str(&text).expect("the fragment is JSON");
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
    }

    #[test]
    fn every_status_the_hooks_emit_is_one_the_wire_reads() {
        // The hooks are string literals -- `--agent running`, `--agent idle`,
        // `--agent needs-input`. Each word has to be one `report` accepts, and
        // so one `AgentReport` names, or the hook Claude Code runs writes an
        // error to its own stdout and the tab never moves. Renaming a status is
        // a two-place edit; the other test pins `needs-input` and `running` by
        // name, but `idle` on Stop and SessionEnd was checked by nothing, so a
        // rename could leave those hooks calling a word the wire rejects with
        // the gate still green. This ties every hook's word back to the enum.
        let text = hooks_text("claude", Path::new("/usr/bin/crook")).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let hooks = parsed["hooks"].as_object().unwrap();
        for (event, group) in hooks {
            let command = group[0]["hooks"][0]["command"].as_str().unwrap();
            let status = command
                .split("--agent ")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .unwrap_or_else(|| panic!("{event} runs no `--agent`: {command}"));
            assert!(
                AgentReport::parse(status).is_some(),
                "{event} reports {status:?}, which is not a status the wire reads",
            );
        }
        assert_eq!(6, hooks.len(), "a hook was added or dropped: {hooks:?}");
    }

    #[test]
    fn an_agent_nobody_wrote_hooks_for_is_refused_by_name() {
        let error = hooks_text("codex", Path::new("/usr/bin/crook")).unwrap_err();
        assert!(error.to_string().contains("codex"));
    }

    #[test]
    fn a_word_that_is_not_a_status_is_refused_with_the_words_that_are() {
        let error = report("sleeping", None, None).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("sleeping"));
        assert!(message.contains("needs-input"));
    }
}
