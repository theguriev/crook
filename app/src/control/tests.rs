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
        cli::list_arguments(words.iter().map(|word| (*word).to_owned()))
            .map_err(|error| error.to_string())
    };
    assert_eq!(arguments(&["list"]), Ok(false));
    assert_eq!(arguments(&["list", "--json"]), Ok(true));
    for (words, said) in [
        (&[][..], "needs a verb"),
        (&["lsit"][..], "takes list, not lsit"),
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
    // An allowed tab starts the count again: a pane nobody refused since is
    // in the ordinary state, not a bad one.
    spawns.accept(pane, pane);
    for _ in 1..spawn::REFUSALS_ALLOWED {
        spawns.refuse(pane, &refusal);
    }
    assert!(
        !spawns.stopped(pane),
        "the count began again at the tab it was allowed"
    );

    spawns.refuse(pane, &refusal);
    assert!(spawns.stopped(pane), "sixteen in a row");
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
    let asked =
        std::thread::spawn(move || asking.ask(listing(), Instant::now() + Duration::from_secs(5)));
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
        .ask(listing(), Instant::now() + Duration::from_secs(5))
        .expect_err("closed");
    assert_eq!(refusal.code, code::GONE);
}

#[test]
fn a_question_nobody_answers_is_refused_as_a_timeout() {
    let inbox = Inbox::default();
    let refusal = inbox
        .ask(listing(), Instant::now() + Duration::from_millis(50))
        .expect_err("no window is serving this inbox");
    assert_eq!(refusal.code, code::TIMEOUT);
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
    use super::super::server::{self, Answer, Socket, converse};
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
        Arc::new(move |request: Request, _| match request.verb {
            Verb::PaneList => Ok(serde_json::to_value(&panes).expect("encodes")),
            Verb::TabNew(_) => Err(Refusal::new(code::UNAUTHORIZED, "not in this test")),
        })
    }

    /// The name this process asks for first.
    fn first_name(directory: &Path) -> PathBuf {
        directory.join(format!("{}.sock", std::process::id()))
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
        let (client, window) = UnixStream::pair().expect("a pair");
        let answering =
            thread::spawn(move || converse(&window, Instant::now() + deadline, &*answer));
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
            Arc::new(move |request, deadline| asking.ask(request, deadline)),
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
        drop(UnixListener::bind(first_name(&directory)).expect("bound"));
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
        drop(UnixListener::bind(&dead).expect("bound"));
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
            Arc::new(|_, _| {
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
        drop(UnixListener::bind(directory.join("1.sock")).expect("a dead one beside it"));
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
    }

    use crate::tab::{Lineage, PaneId};

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
}
