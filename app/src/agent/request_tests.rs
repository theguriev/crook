use serde_json::json;

use super::*;

/// What Claude Code 2.1 hands a `Bash` permission hook as `tool_input`, every
/// key it sends present.
fn whole() -> Value {
    json!({
        "command": "rm -rf build && cargo build",
        "description": "Rebuild from clean",
        "timeout": 120000,
        "run_in_background": false,
        "dangerouslyDisableSandbox": true,
    })
}

#[test]
fn every_key_claude_code_sends_is_read_into_the_request() {
    assert_eq!(
        from_claude_bash(&whole()),
        Some(AskRequest::Command {
            command: "rm -rf build && cargo build".to_owned(),
            note: Some("Rebuild from clean".to_owned()),
            timeout_ms: Some(120_000),
            background: false,
            outside_sandbox: true,
        })
    );
    // The command alone is a request, every flag off.
    assert_eq!(
        from_claude_bash(&json!({"command": "ls"})),
        Some(AskRequest::Command {
            command: "ls".to_owned(),
            note: None,
            timeout_ms: None,
            background: false,
            outside_sandbox: false,
        })
    );
}

#[test]
fn the_allowlist_refuses_an_unknown_key() {
    // A key Crook has never heard of may change what runs — where, as whom,
    // with what — and a card that showed the rest would be asking a person
    // to allow something it had not shown them.
    let mut input = whole();
    input["sandbox_profile"] = json!("none");
    assert_eq!(from_claude_bash(&input), None);

    // And a known key with a type it does not take, a null included.
    for (key, wrong) in [
        ("command", json!(["rm", "-rf"])),
        ("description", json!(null)),
        ("timeout", json!("120000")),
        ("timeout", json!(-1)),
        ("timeout", json!(1.5)),
        ("run_in_background", json!("false")),
        ("dangerouslyDisableSandbox", json!(0)),
    ] {
        let mut input = whole();
        input[key] = wrong.clone();
        assert_eq!(from_claude_bash(&input), None, "{key}: {wrong}");
    }

    // No command, or not an object at all.
    assert_eq!(from_claude_bash(&json!({"description": "x"})), None);
    assert_eq!(from_claude_bash(&json!("rm -rf build")), None);
}

#[test]
fn a_command_is_one_byte_to_forty_eight_kilobytes_with_no_nul() {
    let bash = |command: &str| from_claude_bash(&json!({ "command": command }));
    assert_eq!(bash(""), None, "nothing to run");
    assert!(bash("x").is_some());
    assert!(bash(&"x".repeat(MAX_COMMAND)).is_some());
    assert_eq!(bash(&"x".repeat(MAX_COMMAND + 1)), None);
    assert_eq!(bash("echo a\0b"), None, "a NUL no shell runs as written");

    let noted = |note: String| from_claude_bash(&json!({ "command": "ls", "description": note }));
    assert!(noted("n".repeat(MAX_NOTE)).is_some());
    assert_eq!(noted("n".repeat(MAX_NOTE + 1)), None);
}

/// In a build whose `serde_json` keeps its maps sorted, as this one's does,
/// the sort by hand changes nothing here; this is the test that fails with
/// `--features serde_json/preserve_order` the day it goes, which is the
/// build a dependency turning that feature on would make.
#[test]
fn canonical_json_does_not_depend_on_key_order() {
    let one: Value =
        serde_json::from_str(r#"{"b":1,"a":{"d":[{"y":2,"x":1}],"c":"é\n"}}"#).expect("JSON");
    let other: Value =
        serde_json::from_str(r#"{"a":{"c":"é\n","d":[{"x":1,"y":2}]},"b":1}"#).expect("JSON");
    assert_eq!(canonical(&one), canonical(&other));
    assert_eq!(
        canonical(&one),
        r#"{"a":{"c":"é\n","d":[{"x":1,"y":2}]},"b":1}"#,
        "sorted at every depth, with no white space"
    );
    // An array is a sequence, and its order is what it says.
    assert_ne!(canonical(&json!([1, 2])), canonical(&json!([2, 1])));
}

#[test]
fn the_digest_names_the_session_the_tool_and_the_request_and_nothing_else() {
    let request = from_claude_bash(&whole()).expect("a request");
    let digest = digest("session-a", "Bash", &request);
    assert_eq!(digest, super::digest("session-a", "Bash", &request.clone()));
    assert_ne!(digest, super::digest("session-b", "Bash", &request));
    assert_ne!(digest, super::digest("session-a", "Edit", &request));
    // The NUL between the parts is what keeps one split from reading as
    // another.
    assert_ne!(
        super::digest("ab", "c", &request),
        super::digest("a", "bc", &request)
    );
    let other =
        from_claude_bash(&json!({"command": "rm -rf build && cargo build"})).expect("a request");
    assert_ne!(digest, super::digest("session-a", "Bash", &other));

    let fingerprint = fingerprint(&digest);
    assert_eq!(fingerprint.len(), 32);
    assert!(
        fingerprint
            .chars()
            .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit))
    );
    assert_eq!(fingerprint, hex(&digest)[..32]);
}

#[test]
fn visible_writes_out_esc_cr_bidi_and_zero_width() {
    // A carriage return rewrites the line it ends on a terminal: what a
    // person would read is the second half, and what runs is both.
    assert_eq!(
        visible("echo safe\rrm -rf ~"),
        "echo safe␍rm -rf ~",
        "a carriage return"
    );
    assert_eq!(
        visible("printf '\u{1b}[2J'"),
        "printf '⟨U+001B⟩[2J'",
        "an escape"
    );
    assert_eq!(
        visible("a\u{7f}b\u{9b}c"),
        "a⟨U+007F⟩b⟨U+009B⟩c",
        "DEL and C1"
    );
    assert_eq!(
        visible("ls \u{202e}txt.exe"),
        "ls ⟨U+202E⟩txt.exe",
        "a right-to-left override"
    );
    for hidden in [
        '\u{061c}',
        '\u{200e}',
        '\u{200f}',
        '\u{202a}',
        '\u{202d}',
        '\u{2066}',
        '\u{2069}',
        '\u{200b}',
        '\u{200d}',
        '\u{2060}',
        '\u{2064}',
        '\u{feff}',
        '\u{fff9}',
        '\u{fffb}',
        '\u{e0001}',
        '\u{e007f}',
        '\u{00ad}',
        '\u{2028}',
    ] {
        let shown = visible(&format!("a{hidden}b"));
        assert_eq!(
            shown,
            format!("a⟨U+{:04X}⟩b", u32::from(hidden)),
            "{:04X}",
            u32::from(hidden)
        );
    }

    // Lines stay lines, a tab is the spaces to its stop, and text is text.
    assert_eq!(visible("a\tb\n\tc"), "a       b\n        c");
    assert_eq!(
        visible("cargo build —release 修正"),
        "cargo build —release 修正"
    );
}

#[test]
fn a_requests_debug_never_shows_the_command() {
    let request = from_claude_bash(&whole()).expect("a request");
    let printed = format!("{request:?}");
    assert!(!printed.contains("rm -rf"), "{printed}");
    assert!(!printed.contains("Rebuild"), "{printed}");
    assert!(printed.contains("outside_sandbox: true"), "{printed}");
}
