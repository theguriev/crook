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
        refusal.message.contains("pane.frobnicate") && refusal.message.contains("pane.list"),
        "the refusal names what was asked and what there is: {}",
        refusal.message
    );
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
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
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

        let missing = root.0.join("nobody.sock");
        let refusal = cli::unix::listing(&missing, None, true, None).expect_err("nothing there");
        assert!(
            refusal.to_string().contains("nothing is answering"),
            "{refusal}"
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

        // Inside a pane, the pane's own window, whatever else is running.
        assert_eq!(
            socket_from(Some("/run/user/1000/crook-control/42.sock"), Some("3")),
            Ok(PathBuf::from("/run/user/1000/crook-control/42.sock"))
        );
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

        drop((first, second));
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("loosened");
        let _stranger = UnixListener::bind(directory.join("9.sock")).expect("bound");
        assert!(
            socket_from(None, None).is_err(),
            "a socket in a directory anyone could write to is nobody's to trust"
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
            fs::write(&program, script).expect("the shell is written");
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
                .expect("the shell is runnable");
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
            fs::write(&program, script).expect("the shell is written");
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
                .expect("the shell is runnable");
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
            Self {
                window,
                control,
                shell,
                _root: root,
            }
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
                    .map(|json| serde_json::from_str(&json).expect("the answer is JSON"))
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
    use crate::tab::{Lineage, PaneId};
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

        let worker = served.pane(opened.pane_id);
        assert_eq!(
            served.session(worker, |session| session.working_directory.clone()),
            Some(checkout),
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
            fs::write(&hook, script).expect("the hook is written");
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))
                .expect("the hook is runnable");
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
}
