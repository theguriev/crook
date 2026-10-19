use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use serde_json::json;

use super::*;
use crate::agent::request::AskRequest;

/// What Claude Code 2.1 hands its `PermissionRequest` hook for a `Bash`
/// call on the main thread, as `claude` in a pane runs it.
fn input() -> Value {
    json!({
        "session_id": "4f1c2b7e-0d3a-4e5f-9a8b-7c6d5e4f3a2b",
        "transcript_path": "/home/someone/.claude/projects/x/4f1c.jsonl",
        "cwd": "/home/someone/work/crook",
        "permission_mode": "default",
        "hook_event_name": "PermissionRequest",
        "tool_name": "Bash",
        "tool_input": {
            "command": "rm -rf build",
            "description": "Remove the build directory",
        },
        "permission_suggestions": [{"type": "addRules", "behavior": "allow"}],
    })
}

/// A pane's environment: the window's socket, the pane's token, and Claude
/// Code run in a terminal.
fn environment() -> Environment {
    Environment {
        socket: Some(PathBuf::from("/run/user/1000/crook-control/7.sock")),
        token: Some("c0ffee".to_owned()),
        multiplexer: None,
        entrypoint: Some("cli".to_owned()),
    }
}

/// The pane's terminal: pts/3.
const PTY: u32 = 34819;

/// The terminal found, on the pane and in no multiplexer.
fn on_the_pane() -> Option<Bound> {
    Some(Bound::Tty(PTY))
}

/// A look at the disk that finds the file inside the working directory and
/// nothing there to replace.
fn seen_nothing(_: &AskRequest, _: &str) -> Option<OnDisk> {
    Some(OnDisk {
        resolves_outside: false,
        replaces: None,
    })
}

/// One thing the window does, in order.
#[derive(Clone, Debug)]
enum Says {
    /// Sends a line.
    Line(String),
    /// Says nothing for as long as it is waited on.
    Nothing,
    /// Closes its end.
    Closes,
}

/// A window that says what it is scripted to, and remembers what it was
/// sent.
#[derive(Clone, Default)]
struct Window {
    says: Rc<RefCell<VecDeque<Says>>>,
    sent: Rc<RefCell<Vec<String>>>,
}

impl Window {
    fn saying(says: impl IntoIterator<Item = Says>) -> Self {
        Self {
            says: Rc::new(RefCell::new(says.into_iter().collect())),
            sent: Rc::default(),
        }
    }

    /// One that holds the question and then answers `answer`.
    fn answering(answer: Value) -> Self {
        Self::saying([held(), Says::Line(format!("{answer}\n"))])
    }

    fn sent(&self) -> Vec<String> {
        self.sent.borrow().clone()
    }
}

impl Connection for Window {
    fn send(&mut self, line: &[u8]) -> io::Result<()> {
        self.sent
            .borrow_mut()
            .push(String::from_utf8(line.to_vec()).expect("text"));
        Ok(())
    }

    fn line(&mut self, timeout: Duration) -> io::Result<Line> {
        let next = self.says.borrow_mut().pop_front();
        match next {
            Some(Says::Line(line)) => Ok(Line::Read(line.into_bytes())),
            Some(Says::Closes) => Ok(Line::Closed),
            // Nothing for the whole of the time it is given, and nothing
            // after, the way a window that never answers is.
            Some(Says::Nothing) | None => {
                std::thread::sleep(timeout);
                self.says.borrow_mut().push_front(Says::Nothing);
                Ok(Line::Waiting)
            }
        }
    }
}

/// The window's first answer: it holds the question as number 1.
fn held() -> Says {
    Says::Line("{\"v\":1,\"ok\":true,\"result\":{\"asked\":1}}\n".to_owned())
}

/// A hook that waits no longer than a test should.
const QUICK: Patience = Patience {
    register: Duration::from_millis(200),
    answer: Duration::from_millis(300),
    poll: Duration::from_millis(20),
    find: Duration::from_millis(400),
};

/// What the hook prints, given `input`, `environment` and `window`, its
/// parent never leaving; and whether it connected at all.
fn hook(input: &Value, environment: &Environment, window: &Window) -> (Option<String>, bool) {
    let connected = Cell::new(false);
    let decided = decide(
        input.to_string().as_bytes(),
        environment,
        on_the_pane,
        seen_nothing,
        |_: &Path| {
            connected.set(true);
            Some(window.clone())
        },
        &QUICK,
        || false,
    );
    (decided.map(|(line, _)| line), connected.get())
}

#[test]
fn the_question_is_the_whole_request_bound_by_the_terminal_and_not_the_token() {
    let asked = question(
        input().to_string().as_bytes(),
        &environment(),
        on_the_pane,
        seen_nothing,
    )
    .expect("a question");
    assert_eq!(
        asked.socket,
        PathBuf::from("/run/user/1000/crook-control/7.sock")
    );
    assert_eq!(asked.token.as_deref(), Some("c0ffee"));
    assert_eq!(asked.asked.agent, AskingAgent::Claude);
    assert_eq!(asked.asked.session, "4f1c2b7e-0d3a-4e5f-9a8b-7c6d5e4f3a2b");
    assert_eq!(asked.asked.cwd, "/home/someone/work/crook");
    assert_eq!(asked.asked.tty, Some(PTY));
    assert_eq!(asked.asked.multiplexer, None);
    assert_eq!(asked.asked.wait, 300);
    assert_eq!(
        asked.asked.request,
        AskRequest::Command {
            command: "rm -rf build".to_owned(),
            note: Some("Remove the build directory".to_owned()),
            timeout_ms: None,
            background: false,
            outside_sandbox: false,
        }
    );

    // And the line it sends is that question, which a window reads back
    // as it went.
    let window = Window::answering(json!({"answer": {"decision": "pass"}}));
    hook(&input(), &environment(), &window);
    let sent = window.sent();
    let (_, read) = protocol::read(sent[0].trim_end().as_bytes());
    let read = read.expect("a request the window takes");
    assert_eq!(read.verb, Verb::AgentAsk(asked.asked));
    assert_eq!(read.token.as_deref(), Some("c0ffee"));
}

#[test]
fn every_pass_rule_passes_without_connecting() {
    let mut rules: Vec<(&str, Value, Environment)> = Vec::new();
    let mut with = |rule, change: &dyn Fn(&mut Value, &mut Environment)| {
        let (mut input, mut environment) = (input(), environment());
        change(&mut input, &mut environment);
        rules.push((rule, input, environment));
    };
    with("not in Crook", &|_, environment| environment.socket = None);
    with("in screen or zellij", &|_, environment| {
        environment.multiplexer = Some(Multiplexed::Other);
    });
    with("another event", &|input, _| {
        input["hook_event_name"] = json!("PreToolUse");
    });
    with("another tool", &|input, _| {
        input["tool_name"] = json!("NotebookEdit")
    });
    with("another tool's name on this input", &|input, _| {
        input["tool_name"] = json!("Edit")
    });
    with("a subagent", &|input, _| {
        input["agent_id"] = json!("a1b2c3")
    });
    with("a subagent named oddly", &|input, _| {
        input["agent_id"] = json!(7)
    });
    with("no session", &|input, _| {
        input
            .as_object_mut()
            .expect("an object")
            .remove("session_id");
    });
    with("a session the wire refuses", &|input, _| {
        input["session_id"] = json!("two words");
    });
    with("a relative directory", &|input, _| {
        input["cwd"] = json!("work")
    });
    with("headless: claude -p", &|_, environment| {
        environment.entrypoint = Some("sdk-cli".to_owned());
    });
    with("no entrypoint said", &|_, environment| {
        environment.entrypoint = None
    });
    with("an empty entrypoint", &|_, environment| {
        environment.entrypoint = Some(String::new());
    });
    with("a key the allowlist does not know", &|input, _| {
        input["tool_input"]["sandbox"] = json!("none");
    });
    with("a command too long to show", &|input, _| {
        input["tool_input"]["command"] = json!("x".repeat(crate::agent::request::MAX_COMMAND + 1));
    });

    // The base case connects, which is what makes every row below mean
    // something.
    let window = Window::answering(json!({"answer": {"decision": "pass"}}));
    assert!(
        hook(&input(), &environment(), &window).1,
        "the base case asks"
    );

    for (rule, input, environment) in rules {
        let window = Window::default();
        let (printed, connected) = hook(&input, &environment, &window);
        assert_eq!(printed, None, "{rule}");
        assert!(!connected, "{rule}: connected to ask nothing");
    }

    // Input that is not JSON, and no terminal found.
    let connected = Cell::new(false);
    let connect = |_: &Path| {
        connected.set(true);
        Some(Window::default())
    };
    assert!(
        decide(
            b"not json",
            &environment(),
            on_the_pane,
            seen_nothing,
            connect,
            &QUICK,
            || false
        )
        .is_none()
    );
    assert!(
        decide(
            input().to_string().as_bytes(),
            &environment(),
            || None,
            seen_nothing,
            |_: &Path| {
                connected.set(true);
                Some(Window::default())
            },
            &QUICK,
            || false
        )
        .is_none()
    );
    assert!(!connected.get(), "connected to ask nothing");

    // A request whose line is longer than the window reads is not asked.
    let mut long = input();
    long["tool_input"]["command"] = json!("\u{1}".repeat(crate::agent::request::MAX_COMMAND));
    let window = Window::default();
    assert_eq!(hook(&long, &environment(), &window), (None, false));
}

#[test]
fn allow_and_deny_print_exactly_the_bytes_claude_code_reads() {
    let window = Window::answering(json!({"answer": {"decision": "allow"}}));
    let (printed, _) = hook(&input(), &environment(), &window);
    assert_eq!(
        printed.as_deref(),
        Some(
            "{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"allow\"}}}\n"
        )
    );

    let window = Window::answering(json!({"answer": {
        "decision": "deny",
        "message": "Denied in Crook by the person at the keyboard.",
    }}));
    let (printed, _) = hook(&input(), &environment(), &window);
    assert_eq!(
        printed.as_deref(),
        Some(
            "{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"decision\":{\"behavior\":\"deny\",\"message\":\"Denied in Crook by the person at the keyboard.\"}}}\n"
        )
    );
    // Each is one line of JSON Claude Code can read.
    let read: Value = serde_json::from_str(printed.as_deref().expect("printed")).expect("JSON");
    assert_eq!(
        read["hookSpecificOutput"]["decision"]["behavior"],
        json!("deny")
    );
}

#[test]
fn unknown_verb_eof_garbage_and_timeout_print_nothing() {
    let refusal = |code: &str| {
        Says::Line(format!(
            "{{\"v\":1,\"ok\":false,\"error\":{{\"code\":\"{code}\",\"message\":\"no\"}}}}\n"
        ))
    };
    let answer = |answer: Value| Says::Line(format!("{answer}\n"));
    for (case, says) in [
        ("an older window", vec![refusal("unknown-verb")]),
        (
            "a window that takes no questions",
            vec![refusal("declined")],
        ),
        ("no such pane", vec![refusal("no-such-pane")]),
        ("busy", vec![refusal("busy")]),
        ("closed before it held it", vec![Says::Closes]),
        ("closed after it held it", vec![held(), Says::Closes]),
        ("garbage first", vec![Says::Line("{\n".to_owned())]),
        (
            "garbage after",
            vec![held(), Says::Line("allow\n".to_owned())],
        ),
        ("never held", vec![Says::Nothing]),
        ("never answered", vec![held(), Says::Nothing]),
        ("gone", vec![held(), refusal("gone")]),
        (
            "a pass",
            vec![
                held(),
                answer(json!({"answer": {"decision": "pass", "why": "expired"}})),
            ],
        ),
        (
            "an answer with more in it than this hook knows",
            vec![
                held(),
                answer(json!({"answer": {"decision": "allow", "scope": "session"}})),
            ],
        ),
        (
            "a decision this hook does not know",
            vec![held(), answer(json!({"answer": {"decision": "always"}}))],
        ),
        (
            "an allow it was not asked for",
            vec![
                answer(json!({"answer": {"decision": "allow"}})),
                answer(json!({"answer": {"decision": "allow"}})),
            ],
        ),
        (
            "an allow after a refusal",
            vec![
                refusal("unknown-verb"),
                answer(json!({"answer": {"decision": "allow"}})),
            ],
        ),
    ] {
        let window = Window::saying(says);
        let started = Instant::now();
        let (printed, connected) = hook(&input(), &environment(), &window);
        assert!(connected, "{case}");
        assert_eq!(printed, None, "{case}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{case}: took {:?}",
            started.elapsed()
        );
    }

    // A parent that has gone ends the wait, with the answer still to come:
    // at the fourth look, a few polls in, and nowhere near the minute the
    // answer has, which is how long a hook that never looked would take.
    let window = Window::saying([held(), Says::Nothing]);
    let looks = Cell::new(0);
    let started = Instant::now();
    let decided = decide(
        input().to_string().as_bytes(),
        &environment(),
        on_the_pane,
        seen_nothing,
        |_: &Path| Some(window.clone()),
        &Patience {
            answer: Duration::from_secs(60),
            ..QUICK
        },
        || {
            looks.set(looks.get() + 1);
            looks.get() > 3
        },
    );
    assert!(decided.is_none());
    assert_eq!(
        looks.get(),
        4,
        "it stopped at the look that saw the parent gone"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the wait ran on past the parent: {:?}",
        started.elapsed()
    );
}

#[test]
fn no_path_prints_updated_input_updated_permissions_or_interrupt() {
    for decision in [Decision::Allow, Decision::Deny, Decision::Pass] {
        for message in [None, Some("Denied in Crook by the person at the keyboard.")] {
            for why in [None, Some("expired")] {
                let verdict = Verdict {
                    decision,
                    message: message.map(str::to_owned),
                    why: why.map(str::to_owned),
                };
                let Some(line) = decision_line(&verdict) else {
                    assert_eq!(decision, Decision::Pass);
                    continue;
                };
                for never in ["updatedInput", "updatedPermissions", "interrupt", "always"] {
                    assert!(!line.contains(never), "{line}");
                }
                assert_eq!(line.matches('\n').count(), 1, "one line: {line:?}");
                assert!(line.ends_with('\n'));
            }
        }
    }
    // And a window that sends them is no answer at all.
    for extra in [
        json!({"decision": "allow", "updatedInput": {"command": "rm -rf /"}}),
        json!({"decision": "allow", "updatedPermissions": [{"type": "addRules"}]}),
        json!({"decision": "deny", "interrupt": true}),
    ] {
        let window = Window::answering(json!({ "answer": extra }));
        assert_eq!(hook(&input(), &environment(), &window).0, None, "{extra}");
    }
}

/// Claude Code 2.1.280's `PermissionRequest` for an `Edit`, a `Write` and a
/// `NotebookEdit`, as the probe in `docs/answering.md` recorded them (C13).
const EDIT: &str = include_str!("../../tests/fixtures/asks/claude-2.1.280-edit.json");
const WRITE: &str = include_str!("../../tests/fixtures/asks/claude-2.1.280-write.json");
const NOTEBOOK_EDIT: &str =
    include_str!("../../tests/fixtures/asks/claude-2.1.280-notebookedit.json");

fn fixture(text: &str) -> Value {
    serde_json::from_str(text).expect("the fixture is JSON")
}

#[test]
fn an_edit_request_is_asked_with_its_change_and_its_path() {
    let asked =
        question(EDIT.as_bytes(), &environment(), on_the_pane, seen_nothing).expect("a question");
    assert_eq!(asked.asked.cwd, "/run/user/1000/crook-probe-claude");
    assert_eq!(
        asked.asked.request,
        AskRequest::Edit {
            path: "/run/user/1000/crook-probe-claude/notes.txt".to_owned(),
            edits: vec![crate::agent::request::FileEdit {
                old: "alpha".to_owned(),
                new: "beta".to_owned(),
                replace_all: false,
            }],
            resolves_outside: false,
        }
    );
    assert_eq!(asked.asked.tool(), "Edit");

    let asked =
        question(WRITE.as_bytes(), &environment(), on_the_pane, seen_nothing).expect("a question");
    assert_eq!(
        asked.asked.request,
        AskRequest::Write {
            path: "/run/user/1000/crook-probe-claude/notes.txt".to_owned(),
            content: "alpha".to_owned(),
            resolves_outside: false,
            replaces: None,
        }
    );
    assert_eq!(asked.asked.tool(), "Write");

    // A notebook's cell is not a change this card can draw.
    assert_eq!(
        question(
            NOTEBOOK_EDIT.as_bytes(),
            &environment(),
            on_the_pane,
            seen_nothing
        ),
        None
    );

    // And the line the hook sends is the window's to read back whole, the
    // allow it gets back printed as for a command.
    for (input, kind) in [(EDIT, "edit"), (WRITE, "write")] {
        let window = Window::answering(json!({"answer": {"decision": "allow"}}));
        let (printed, connected) = hook(&fixture(input), &environment(), &window);
        assert!(connected);
        assert_eq!(printed.as_deref(), Some(ALLOW));
        let (_, read) = protocol::read(window.sent()[0].trim_end().as_bytes());
        let Verb::AgentAsk(read) = read.expect("a request the window takes").verb else {
            panic!("not a question");
        };
        assert_eq!(read.request.kind(), kind);
    }
}

#[test]
fn a_write_over_the_cap_is_not_asked() {
    let write = |content: String| {
        let mut input = fixture(WRITE);
        input["tool_input"]["content"] = json!(content);
        input
    };
    let lines = |count: usize| vec!["x"; count].join("\n");
    let asks = |input: &Value| {
        let window = Window::answering(json!({"answer": {"decision": "pass"}}));
        hook(input, &environment(), &window).1
    };
    assert!(asks(&write(lines(crate::agent::request::MAX_CHANGE_LINES))));
    // A line more than the Changes column draws of a diff.
    assert!(!asks(&write(lines(
        crate::agent::request::MAX_CHANGE_LINES + 1
    ))));
    // One line, and more than the window reads in one: not cut, not asked.
    assert!(!asks(&write("x".repeat(protocol::MAX_LINE))));
    // Short of the line, after JSON's escapes have had their say.
    assert!(!asks(&write("\u{1}".repeat(protocol::MAX_LINE / 6))));
    assert!(asks(&write("x".repeat(protocol::MAX_LINE / 2))));
}

#[test]
fn an_older_window_refuses_the_edit_kind_and_the_hook_prints_nothing() {
    // A window from before edits reads `kind: edit` as a field it does not
    // know, and refuses the question as a bad request; so does one that
    // knows the kind and not a field in it. Either way the hook passes and
    // Claude Code's own dialog is the one that asks.
    for code in ["bad-request", "unknown-verb"] {
        let window = Window::saying([Says::Line(format!(
            "{{\"v\":1,\"ok\":false,\"error\":{{\"code\":\"{code}\",\"message\":\"no\"}}}}\n"
        ))]);
        let (printed, connected) = hook(&fixture(EDIT), &environment(), &window);
        assert!(connected, "{code}");
        assert_eq!(printed, None, "{code}");
        assert!(window.sent()[0].contains("\"kind\":\"edit\""), "{code}");
    }

    // What v0.1.29's window reads a request as, which knew commands alone:
    // the line this hook sends is one it refuses, rather than one it reads
    // as something else.
    #[derive(Debug, serde::Deserialize)]
    #[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
    enum Older {
        Command {
            command: String,
            #[serde(default)]
            note: Option<String>,
            #[serde(default)]
            timeout_ms: Option<u64>,
            background: bool,
            outside_sandbox: bool,
        },
    }
    let window = Window::answering(json!({"answer": {"decision": "pass"}}));
    hook(&fixture(EDIT), &environment(), &window);
    let sent: Value = serde_json::from_str(&window.sent()[0]).expect("a line of JSON");
    assert!(serde_json::from_value::<Older>(sent["args"]["request"].clone()).is_err());
    let window = Window::answering(json!({"answer": {"decision": "pass"}}));
    hook(&input(), &environment(), &window);
    let sent: Value = serde_json::from_str(&window.sent()[0]).expect("a line of JSON");
    let Older::Command {
        command,
        note,
        timeout_ms,
        background,
        outside_sandbox,
    } = serde_json::from_value::<Older>(sent["args"]["request"].clone())
        .expect("and a command still reads as it did");
    assert_eq!(
        (
            command.as_str(),
            note.as_deref(),
            timeout_ms,
            background,
            outside_sandbox
        ),
        (
            "rm -rf build",
            Some("Remove the build directory"),
            None,
            false,
            false
        )
    );
}

#[test]
fn a_change_carries_what_the_look_found_on_disk_and_a_write_it_could_not_see_is_not_asked() {
    let looked = RefCell::new(Vec::new());
    let found = OnDisk {
        resolves_outside: true,
        replaces: Some(crate::agent::request::Replaced {
            lines: 3000,
            bytes: 70_000,
            more: false,
        }),
    };
    let asked = question(
        WRITE.as_bytes(),
        &environment(),
        on_the_pane,
        |request, cwd| {
            looked.borrow_mut().push((request.kind(), cwd.to_owned()));
            Some(found)
        },
    )
    .expect("a question");
    assert_eq!(
        looked.take(),
        [("write", "/run/user/1000/crook-probe-claude".to_owned())]
    );
    let AskRequest::Write {
        resolves_outside,
        replaces,
        ..
    } = asked.asked.request
    else {
        panic!("a write");
    };
    assert_eq!((resolves_outside, replaces), (true, found.replaces));
    let asked = question(EDIT.as_bytes(), &environment(), on_the_pane, |_, _| {
        Some(found)
    })
    .expect("a question");
    assert!(matches!(
        asked.asked.request,
        AskRequest::Edit {
            resolves_outside: true,
            ..
        }
    ));

    // Nothing to say about the disk is no question, and the window is
    // never connected to.
    assert_eq!(
        question(WRITE.as_bytes(), &environment(), on_the_pane, |_, _| None),
        None
    );
    // A command lands nowhere, and the disk is not looked at for it; nor is
    // it for a hook that has already found it asks nothing.
    let never = |_: &AskRequest, _: &str| -> Option<OnDisk> { panic!("looked") };
    assert!(
        question(
            input().to_string().as_bytes(),
            &environment(),
            on_the_pane,
            never
        )
        .is_some()
    );
    assert_eq!(
        question(WRITE.as_bytes(), &environment(), || None, never),
        None
    );
}

#[test]
fn a_window_from_before_the_look_refuses_a_change_that_carries_it() {
    // A window that drew edits and writes from their text alone, without
    // `resolves_outside` or `replaces`: the line this hook sends is one it
    // refuses, so that its hook passes rather than its card leaving a link
    // or a file replaced unsaid.
    #[derive(Debug, serde::Deserialize)]
    #[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
    enum Before {
        Edit { path: String, edits: Vec<Value> },
        Write { path: String, content: String },
    }
    for input in [EDIT, WRITE] {
        let window = Window::answering(json!({"answer": {"decision": "pass"}}));
        hook(&fixture(input), &environment(), &window);
        let sent: Value = serde_json::from_str(&window.sent()[0]).expect("a line of JSON");
        let request = &sent["args"]["request"];
        assert!(
            serde_json::from_value::<Before>(request.clone()).is_err(),
            "{request}"
        );
        let mut without = request.clone();
        let fields = without.as_object_mut().unwrap();
        fields.remove("resolves_outside");
        fields.remove("replaces");
        match serde_json::from_value::<Before>(without).expect("the rest reads as it did") {
            Before::Edit { path, edits } => {
                assert!(path.ends_with("notes.txt") && edits.len() == 1)
            }
            Before::Write { path, content } => {
                assert!(path.ends_with("notes.txt") && content == "alpha")
            }
        }
    }
}

/// A pane's environment inside tmux: the socket and token the tmux server
/// kept from wherever it was started.
fn in_tmux() -> Environment {
    Environment {
        multiplexer: Some(Multiplexed::Tmux),
        ..environment()
    }
}

/// tmux's clients, as the hook found them: on pts/3 and on one more.
fn clients() -> Multiplexer {
    Multiplexer {
        kind: crate::control::protocol::MultiplexerKind::Tmux,
        clients: vec![PTY, 34820],
        pane: "%3".to_owned(),
    }
}

/// A window's answer to `pane.find`.
fn located(found: bool) -> Says {
    Says::Line(format!(
        "{{\"v\":1,\"ok\":true,\"result\":{{\"found\":{found}}}}}\n"
    ))
}

/// The machine a hook asks from: the windows listening, by socket, the
/// others the control directory lists, and every socket connected to.
#[derive(Default)]
struct Machine {
    windows: Vec<(PathBuf, Window)>,
    others: Vec<PathBuf>,
    listed: Cell<bool>,
    connected: RefCell<Vec<PathBuf>>,
}

impl Machine {
    /// With the window `CROOK_SOCKET` names saying `says`, and others, each
    /// listed in the control directory in this order, saying theirs — or,
    /// for `None`, listed and not listening.
    fn with(
        says: Window,
        others: impl IntoIterator<Item = (&'static str, Option<Window>)>,
    ) -> Self {
        let mut machine = Self {
            windows: vec![(PathBuf::from("/run/user/1000/crook-control/7.sock"), says)],
            ..Self::default()
        };
        for (name, window) in others {
            let socket = PathBuf::from(format!("/run/user/1000/crook-control/{name}.sock"));
            machine.others.push(socket.clone());
            if let Some(window) = window {
                machine.windows.push((socket, window));
            }
        }
        machine
    }
}

impl Windows for &Machine {
    type Connection = Window;

    fn connect(&mut self, socket: &Path) -> Option<Window> {
        self.connected.borrow_mut().push(socket.to_owned());
        self.windows
            .iter()
            .find(|(listening, _)| listening == socket)
            .map(|(_, window)| window.clone())
    }

    fn others(&mut self, _: &Path) -> Vec<PathBuf> {
        self.listed.set(true);
        self.others.clone()
    }
}

/// What the hook prints from inside tmux, asking the windows of `machine`.
fn hook_in_tmux(machine: &Machine) -> Option<String> {
    decide(
        input().to_string().as_bytes(),
        &in_tmux(),
        || Some(Bound::Multiplexer(clients())),
        seen_nothing,
        machine,
        &QUICK,
        || false,
    )
    .map(|(line, _)| line)
}

/// Whether `line` is a `pane.find` carrying exactly `ttys`, and nothing
/// else: no token, nothing of the question.
fn is_a_find_of(line: &str, ttys: &[u32]) -> bool {
    let sent: Value = serde_json::from_str(line).expect("a line of JSON");
    sent == json!({"v": 1, "verb": "pane.find", "args": {"ttys": ttys}})
}

#[test]
fn inside_tmux_the_question_names_the_clients_terminals_and_carries_no_token() {
    let asked = question(
        input().to_string().as_bytes(),
        &in_tmux(),
        || Some(Bound::Multiplexer(clients())),
        seen_nothing,
    )
    .expect("a question");
    assert_eq!(asked.asked.tty, None);
    assert_eq!(asked.asked.multiplexer, Some(clients()));
    assert_eq!(asked.token, None, "the token tmux kept is no hint");

    // What the environment says to look for is what has to be found: a
    // terminal of its own is no answer inside tmux, nor tmux's clients
    // outside it.
    for (environment, bound) in [
        (in_tmux(), Bound::Tty(PTY)),
        (environment(), Bound::Multiplexer(clients())),
    ] {
        assert_eq!(
            question(
                input().to_string().as_bytes(),
                &environment,
                || Some(bound.clone()),
                seen_nothing
            ),
            None
        );
    }
}

#[test]
fn inside_tmux_the_question_goes_to_crook_socket_once_it_finds_a_client_there() {
    let window = Window::saying([
        located(true),
        held(),
        Says::Line(format!("{}\n", json!({"answer": {"decision": "allow"}}))),
    ]);
    let machine = Machine::with(window.clone(), [("8", Some(Window::default()))]);
    assert_eq!(hook_in_tmux(&machine).as_deref(), Some(ALLOW));
    let sent = window.sent();
    assert!(is_a_find_of(&sent[0], &[PTY, 34820]), "{:?}", sent[0]);
    let (_, read) = protocol::read(sent[1].trim_end().as_bytes());
    let Verb::AgentAsk(asked) = read.expect("a request the window takes").verb else {
        panic!("not a question: {:?}", sent[1]);
    };
    assert_eq!(asked.multiplexer, Some(clients()));
    assert!(!machine.listed.get(), "no other window was looked for");
}

#[test]
fn a_stale_crook_socket_inside_tmux_finds_the_window_without_telling_the_others_anything() {
    // CROOK_SOCKET's window holds none of the clients; of the others, one is
    // not listening, one holds none either, and the third does.
    let stale = Window::saying([located(false)]);
    let none = Window::saying([located(false)]);
    let holder = Window::saying([
        located(true),
        held(),
        Says::Line(format!("{}\n", json!({"answer": {"decision": "allow"}}))),
    ]);
    let after = Window::default();
    let machine = Machine::with(
        stale.clone(),
        [
            ("1", None),
            ("2", Some(none.clone())),
            ("3", Some(holder.clone())),
            ("4", Some(after.clone())),
        ],
    );
    assert_eq!(hook_in_tmux(&machine).as_deref(), Some(ALLOW));
    for (name, window) in [("CROOK_SOCKET's", &stale), ("the other", &none)] {
        let sent = window.sent();
        assert_eq!(sent.len(), 1, "{name} window was sent more: {sent:?}");
        assert!(is_a_find_of(&sent[0], &[PTY, 34820]), "{sent:?}");
        assert!(!sent[0].contains("rm -rf"));
    }
    let sent = holder.sent();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[1].contains("rm -rf build"), "{sent:?}");
    assert!(
        after.sent().is_empty(),
        "a window after the one that found it"
    );
}

#[test]
fn inside_tmux_at_most_four_other_windows_are_asked_and_then_nothing() {
    let others: Vec<(&'static str, Option<Window>)> = ["1", "2", "3", "4", "5", "6"]
        .into_iter()
        .map(|name| (name, Some(Window::saying([located(false)]))))
        .collect();
    let machine = Machine::with(Window::saying([located(false)]), others);
    assert_eq!(hook_in_tmux(&machine), None);
    // CROOK_SOCKET's, then four of the others; a socket that is not
    // listening is no window, and does not count.
    assert_eq!(machine.connected.borrow().len(), 1 + MAX_OTHER_WINDOWS);

    let mut others: Vec<(&'static str, Option<Window>)> = vec![("0", None)];
    others.extend(
        ["1", "2", "3", "4", "5"]
            .into_iter()
            .map(|name| (name, Some(Window::saying([located(false)])))),
    );
    let machine = Machine::with(Window::saying([located(false)]), others);
    assert_eq!(hook_in_tmux(&machine), None);
    assert_eq!(machine.connected.borrow().len(), 2 + MAX_OTHER_WINDOWS);
}

#[test]
fn inside_tmux_a_window_that_refuses_or_says_nothing_is_passed_over_in_time() {
    // An older window, which knows no `pane.find`, and one that never
    // answers: neither is told anything more, and the search ends within
    // its time whatever the windows do.
    let older = Window::saying([Says::Line(
        "{\"v\":1,\"ok\":false,\"error\":{\"code\":\"unknown-verb\",\"message\":\"no\"}}\n"
            .to_owned(),
    )]);
    let silent = Window::saying([Says::Nothing]);
    let machine = Machine::with(
        older.clone(),
        [
            ("1", Some(silent.clone())),
            ("2", Some(Window::saying([Says::Nothing]))),
        ],
    );
    let started = Instant::now();
    assert_eq!(hook_in_tmux(&machine), None);
    assert!(
        started.elapsed() < QUICK.find + QUICK.register,
        "{:?}",
        started.elapsed()
    );
    assert_eq!(older.sent().len(), 1);
    assert_eq!(silent.sent().len(), 1);
}

#[test]
fn outside_a_multiplexer_only_crook_socket_is_asked() {
    // Refused as `no-such-pane`: the hook passes, and asks nobody else.
    let window = Window::saying([Says::Line(
        "{\"v\":1,\"ok\":false,\"error\":{\"code\":\"no-such-pane\",\"message\":\"no\"}}\n"
            .to_owned(),
    )]);
    let elsewhere = Window::saying([located(true), held()]);
    let machine = Machine::with(window.clone(), [("8", Some(elsewhere.clone()))]);
    let decided = decide(
        input().to_string().as_bytes(),
        &environment(),
        on_the_pane,
        seen_nothing,
        &machine,
        &QUICK,
        || false,
    );
    assert!(decided.is_none());
    assert!(!machine.listed.get(), "another window was looked for");
    assert_eq!(machine.connected.borrow().len(), 1);
    assert!(elsewhere.sent().is_empty());
    assert_eq!(window.sent().len(), 1);
}

#[test]
fn an_ahead_agent_or_screen_or_zellij_inside_a_multiplexer_connects_to_nothing() {
    // A subagent's question passes before anything is looked for, inside
    // tmux as everywhere; screen and zellij cannot be asked for their
    // clients at all.
    let mut subagent = input();
    subagent["agent_id"] = json!("a1b2c3");
    let other = Environment {
        multiplexer: Some(Multiplexed::Other),
        ..environment()
    };
    for (input, environment) in [(subagent, in_tmux()), (input(), other)] {
        let machine = Machine::with(Window::saying([located(true), held()]), []);
        let looked = Cell::new(false);
        let decided = decide(
            input.to_string().as_bytes(),
            &environment,
            || {
                looked.set(true);
                Some(Bound::Multiplexer(clients()))
            },
            seen_nothing,
            &machine,
            &QUICK,
            || false,
        );
        assert!(decided.is_none());
        assert!(!looked.get(), "tmux was asked where its clients are");
        assert!(machine.connected.borrow().is_empty(), "connected");
    }
}
