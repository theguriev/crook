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
        multiplexed: false,
        entrypoint: Some("cli".to_owned()),
    }
}

/// The pane's terminal: pts/3.
const PTY: u32 = 34819;

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
};

/// What the hook prints, given `input`, `environment` and `window`, its
/// parent never leaving; and whether it connected at all.
fn hook(input: &Value, environment: &Environment, window: &Window) -> (Option<String>, bool) {
    let connected = Cell::new(false);
    let decided = decide(
        input.to_string().as_bytes(),
        environment,
        || Some(PTY),
        |_| {
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
    let asked =
        question(input().to_string().as_bytes(), &environment(), || Some(PTY)).expect("a question");
    assert_eq!(
        asked.socket,
        PathBuf::from("/run/user/1000/crook-control/7.sock")
    );
    assert_eq!(asked.token.as_deref(), Some("c0ffee"));
    assert_eq!(asked.asked.agent, AskingAgent::Claude);
    assert_eq!(asked.asked.session, "4f1c2b7e-0d3a-4e5f-9a8b-7c6d5e4f3a2b");
    assert_eq!(asked.asked.cwd, "/home/someone/work/crook");
    assert_eq!(asked.asked.tty, PTY);
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
    with("in a multiplexer", &|_, environment| {
        environment.multiplexed = true;
    });
    with("another event", &|input, _| {
        input["hook_event_name"] = json!("PreToolUse");
    });
    with("another tool", &|input, _| {
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
            || Some(PTY),
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

    // A parent that has gone ends the wait, with the answer still to come.
    let window = Window::saying([held(), Says::Nothing]);
    let looks = Cell::new(0);
    let decided = decide(
        input().to_string().as_bytes(),
        &environment(),
        || Some(PTY),
        |_| Some(window.clone()),
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
