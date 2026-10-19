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

    // Fields handed over out of order, as a map that keeps the order keys
    // came in would hand them: the order this build's maps never have, so
    // the one way to see the sort do its work here. A `canonical` that left
    // the sort to `Value::to_string` would leave `write_object` with no
    // caller outside this test, which the gate's dead-code lint refuses.
    let (a, b, z) = ("a".to_owned(), "b".to_owned(), "z".to_owned());
    let inner = json!({"y": 2, "x": 1});
    let (one, two) = (json!(1), json!("é\n"));
    let mut written = String::new();
    write_object([(&z, &inner), (&b, &one), (&a, &two)], &mut written);
    assert_eq!(written, r#"{"a":"é\n","b":1,"z":{"x":1,"y":2}}"#);
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

    // What a font may draw as nothing though it is not `Cf`: a variation
    // selector makes `./build` another file than the one it reads as.
    assert_eq!(visible("rm ./build\u{FE0F}"), "rm ./build⟨U+FE0F⟩");
    for ignorable in [
        '\u{034f}',
        '\u{115f}',
        '\u{1160}',
        '\u{17b4}',
        '\u{17b5}',
        '\u{180b}',
        '\u{180d}',
        '\u{180f}',
        '\u{3164}',
        '\u{fe00}',
        '\u{ffa0}',
        '\u{e0100}',
        '\u{e01ef}',
    ] {
        assert_eq!(
            visible(&format!("a{ignorable}b")),
            format!("a⟨U+{:04X}⟩b", u32::from(ignorable)),
            "{:04X}",
            u32::from(ignorable)
        );
    }
    // A space that is not the one a shell splits at reads as one all the
    // same.
    assert_eq!(visible("rm\u{a0}-rf"), "rm⟨U+00A0⟩-rf");
    for space in ['\u{2000}', '\u{200a}', '\u{202f}', '\u{3000}'] {
        assert_eq!(
            visible(&format!("a{space}b")),
            format!("a⟨U+{:04X}⟩b", u32::from(space))
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

/// What Claude Code 2.1.280 hands an `Edit` permission hook as `tool_input`,
/// as the probe recorded it (`claude-2.1.280-edit.json`).
fn edit_input() -> Value {
    json!({
        "file_path": "/work/crook/notes.txt",
        "old_string": "alpha\nbeta",
        "new_string": "gamma",
        "replace_all": false,
    })
}

fn edit(old: &str, new: &str, replace_all: bool) -> FileEdit {
    FileEdit {
        old: old.to_owned(),
        new: new.to_owned(),
        replace_all,
    }
}

#[test]
fn an_edit_and_a_write_are_read_whole_and_drawn_as_their_change() {
    let request = from_claude("Edit", &edit_input()).expect("an edit");
    assert_eq!(
        request,
        AskRequest::Edit {
            path: "/work/crook/notes.txt".to_owned(),
            edits: vec![edit("alpha\nbeta", "gamma", false)],
            resolves_outside: true,
        }
    );
    assert_eq!(request.kind(), "edit");
    assert_eq!(request.path(), Some("/work/crook/notes.txt"));
    assert_eq!(request.command(), None);
    let change = |request: &AskRequest| -> Vec<(Mark, String)> {
        request
            .change()
            .into_iter()
            .map(|line| (line.mark, line.text))
            .collect()
    };
    assert_eq!(
        change(&request),
        [
            (Mark::Removed, "alpha".to_owned()),
            (Mark::Removed, "beta".to_owned()),
            (Mark::Added, "gamma".to_owned()),
        ]
    );
    // `replace_all` left out is Claude Code's own default, off.
    let mut input = edit_input();
    input
        .as_object_mut()
        .expect("an object")
        .remove("replace_all");
    assert_eq!(from_claude("Edit", &input), Some(request));

    // Every line, a last empty one included, and what would not print as
    // itself written out: a trailing newline is part of what is replaced.
    let write = from_claude(
        "Write",
        &json!({"file_path": "/work/crook/a.sh", "content": "echo \u{202e}hi\r\n"}),
    )
    .expect("a write");
    assert_eq!(write.kind(), "write");
    assert_eq!(
        change(&write),
        [
            (Mark::Added, "echo ⟨U+202E⟩hi␍".to_owned()),
            (Mark::Added, String::new()),
        ]
    );
    // An empty text is no lines; replacing every match says so.
    let creating = AskRequest::Edit {
        path: "/a".to_owned(),
        edits: vec![edit("", "new", true)],
        resolves_outside: true,
    };
    assert_eq!(
        change(&creating),
        [
            (Mark::Heading, "Every match".to_owned()),
            (Mark::Added, "new".to_owned()),
        ]
    );
}

#[test]
fn a_multi_edit_shows_every_edit() {
    let request = from_claude(
        "MultiEdit",
        &json!({
            "file_path": "/work/crook/src/lib.rs",
            "edits": [
                {"old_string": "one", "new_string": "uno"},
                {"old_string": "two", "new_string": "dos", "replace_all": true},
                {"old_string": "three\nfour", "new_string": ""},
            ],
        }),
    )
    .expect("a multi-edit");
    let AskRequest::Edit { edits, .. } = &request else {
        panic!("not an edit: {request:?}");
    };
    assert_eq!(edits.len(), 3);
    let drawn: Vec<(Mark, String)> = request
        .change()
        .into_iter()
        .map(|line| (line.mark, line.text))
        .collect();
    assert_eq!(
        drawn,
        [
            (Mark::Heading, "Edit 1 of 3".to_owned()),
            (Mark::Removed, "one".to_owned()),
            (Mark::Added, "uno".to_owned()),
            (Mark::Heading, "Edit 2 of 3 · every match".to_owned()),
            (Mark::Removed, "two".to_owned()),
            (Mark::Added, "dos".to_owned()),
            (Mark::Heading, "Edit 3 of 3".to_owned()),
            (Mark::Removed, "three".to_owned()),
            (Mark::Removed, "four".to_owned()),
        ]
    );
    assert_eq!(request.change_lines(), drawn.len());
    // None of the edits is lost to a digest that named only one.
    let fewer = AskRequest::Edit {
        path: "/work/crook/src/lib.rs".to_owned(),
        edits: edits[..2].to_vec(),
        resolves_outside: true,
    };
    assert_ne!(
        digest("s", "MultiEdit", &request),
        digest("s", "MultiEdit", &fewer)
    );
}

#[test]
fn an_unknown_edit_field_is_not_asked() {
    // Any key Crook does not know, in the input or in one of its edits.
    let mut input = edit_input();
    input["encoding"] = json!("latin1");
    assert_eq!(from_claude("Edit", &input), None);
    for wrong in [
        json!({"file_path": "/a", "content": "x", "mode": "0755"}),
        json!({"file_path": "/a", "content": "x", "old_string": "y"}),
    ] {
        assert_eq!(from_claude("Write", &wrong), None, "{wrong}");
    }
    for wrong in [
        json!({"file_path": "/a", "edits": [{"old_string": "a", "new_string": "b", "line": 3}]}),
        json!({"file_path": "/a", "edits": [{"old_string": "a", "new_string": "b", "file_path": "/b"}]}),
        json!({"file_path": "/a", "edits": [], "dry_run": true}),
        json!({"file_path": "/a", "edits": []}),
        json!({"file_path": "/a", "edits": [{"old_string": "a"}]}),
        json!({"file_path": "/a", "edits": {"old_string": "a", "new_string": "b"}}),
        json!({"file_path": "/a", "edits": ["a"]}),
    ] {
        assert_eq!(from_claude("MultiEdit", &wrong), None, "{wrong}");
    }
    // A known key of a type it does not take, a null included, or left out.
    for (key, wrong) in [
        ("file_path", json!(null)),
        ("file_path", json!(["/a"])),
        ("old_string", json!(1)),
        ("new_string", json!(null)),
        ("replace_all", json!("true")),
    ] {
        let mut input = edit_input();
        input[key] = wrong.clone();
        assert_eq!(from_claude("Edit", &input), None, "{key}: {wrong}");
    }
    for key in ["file_path", "old_string", "new_string"] {
        let mut input = edit_input();
        input.as_object_mut().expect("an object").remove(key);
        assert_eq!(from_claude("Edit", &input), None, "{key} left out");
    }
    assert_eq!(from_claude("Write", &json!({"file_path": "/a"})), None);

    // A path that is not absolute, or that a card would draw as other lines;
    // a NUL in what is written.
    for path in [
        "notes.txt",
        "./notes.txt",
        "~/notes.txt",
        "/a\nb",
        "/a\rb",
        "/a\u{1b}b",
        "/a\u{85}b",
    ] {
        let mut input = edit_input();
        input["file_path"] = json!(path);
        assert_eq!(from_claude("Edit", &input), None, "{path:?}");
    }
    let path = |path: String| {
        let mut input = edit_input();
        input["file_path"] = json!(path);
        from_claude("Edit", &input)
    };
    assert!(path(format!("/{}", "d".repeat(MAX_PATH - 1))).is_some());
    assert_eq!(
        path(format!("/{}", "d".repeat(MAX_PATH))),
        None,
        "a path too long"
    );
    // Counted as it is written out: thirty-two overrides are 256 characters.
    assert_eq!(path(format!("/{}", "\u{202e}".repeat(32))), None);
    assert!(path(format!("/{}", "\u{202e}".repeat(31))).is_some());
    let mut input = edit_input();
    input["new_string"] = json!("a\0b");
    assert_eq!(from_claude("Edit", &input), None, "a NUL");
    assert_eq!(
        from_claude("Write", &json!({"file_path": "/a", "content": "a\0b"})),
        None
    );
    // A path with a bidirectional override is shown, written out, not
    // refused.
    let mut input = edit_input();
    input["file_path"] = json!("/work/\u{202e}txt.rs");
    assert!(from_claude("Edit", &input).is_some());

    // And no tool Crook does not ask about, whatever its input.
    for tool in [
        "NotebookEdit",
        "WebFetch",
        "Read",
        "mcp__x__y",
        "edit",
        "bash",
    ] {
        assert_eq!(from_claude(tool, &edit_input()), None, "{tool}");
    }
}

#[test]
fn a_change_longer_than_a_diff_is_drawn_to_is_no_request() {
    let lines = |count: usize| vec!["x"; count].join("\n");
    let write =
        |content: String| from_claude("Write", &json!({"file_path": "/a", "content": content}));
    assert!(write(lines(MAX_CHANGE_LINES)).is_some());
    assert_eq!(write(lines(MAX_CHANGE_LINES + 1)), None);
    // An edit's two sides and its heading are counted together.
    let half = MAX_CHANGE_LINES / 2;
    let edit = |old: usize, new: usize, replace_all: bool| {
        from_claude(
            "Edit",
            &json!({"file_path": "/a", "old_string": lines(old), "new_string": lines(new), "replace_all": replace_all}),
        )
    };
    assert!(edit(half, half, false).is_some());
    assert_eq!(edit(half, half + 1, false), None);
    assert_eq!(edit(half, half, true), None, "the heading is a line too");
}

#[test]
fn a_path_is_inside_its_working_directory_by_its_text_alone() {
    for (path, directory) in [
        ("/work/crook/a.rs", "/work/crook"),
        ("/work/crook/src/a.rs", "/work/crook/"),
        ("/work//crook/./src/a.rs", "/work/crook"),
    ] {
        assert!(lexically_inside(path, directory), "{path} in {directory}");
    }
    for (path, directory) in [
        ("/work/crook-old/a.rs", "/work/crook"),
        ("/work/crook", "/work/crook"),
        ("/etc/passwd", "/work/crook"),
        // Where `..` leads depends on the links under it, which nothing here
        // looks at: outside, whatever its text seems to say.
        ("/work/crook/../crook/a.rs", "/work/crook"),
        ("/work/crook/src/../../../etc/passwd", "/work/crook"),
        ("relative/a.rs", "/work/crook"),
    ] {
        assert!(!lexically_inside(path, directory), "{path} in {directory}");
    }
    assert_eq!(
        relative_to("/work/crook/./src/a.rs", "/work/crook"),
        Some("src/a.rs".to_owned())
    );
    assert_eq!(relative_to("/etc/passwd", "/work/crook"), None);
}

#[test]
fn a_changes_debug_never_shows_its_path_or_its_text() {
    let edit = from_claude("Edit", &edit_input()).expect("an edit");
    let write = from_claude(
        "Write",
        &json!({"file_path": "/work/secret/plan.md", "content": "launch codes"}),
    )
    .expect("a write");
    for printed in [format!("{edit:?}"), format!("{write:?}")] {
        for hidden in ["notes", "alpha", "gamma", "secret", "launch"] {
            assert!(!printed.contains(hidden), "{printed}");
        }
    }
}

#[test]
fn a_tab_and_the_spaces_a_line_ends_in_are_drawn_as_muted_marks_in_a_change() {
    let drawn = |content: &str| -> Vec<(String, Vec<String>)> {
        from_claude("Write", &json!({"file_path": "/a", "content": content}))
            .expect("a write")
            .change()
            .into_iter()
            .map(|line| {
                let marks = line
                    .muted
                    .iter()
                    .map(|mark| line.text[mark.clone()].to_owned())
                    .collect();
                (line.text, marks)
            })
            .collect()
    };
    // A heredoc's `<<-` strips the tab and not the spaces: the two are not
    // the same file, and are not drawn as one.
    assert_eq!(
        drawn("\tEOF"),
        [("→       EOF".to_owned(), vec!["→".to_owned()])]
    );
    assert_eq!(drawn("        EOF"), [("        EOF".to_owned(), vec![])]);
    assert_ne!(drawn("\tEOF"), drawn("        EOF"));
    // A tab goes to the next stop from where it is.
    assert_eq!(drawn("ab\tc")[0].0, "ab→     c");
    // A space after a `\` ends the line it seemed to continue.
    assert_eq!(
        drawn("foo \\ "),
        [("foo \\·".to_owned(), vec!["·".to_owned()])]
    );
    assert_eq!(drawn("foo \\"), [("foo \\".to_owned(), vec![])]);
    assert_ne!(drawn("foo\\ "), drawn("foo\\"));
    // Every space and tab a line ends in, and only those; on every line.
    assert_eq!(
        drawn("a  \t\nb c \n  "),
        [
            (
                "a··→    ".to_owned(),
                vec!["·".to_owned(), "·".to_owned(), "→".to_owned()]
            ),
            ("b c·".to_owned(), vec!["·".to_owned()]),
            ("··".to_owned(), vec!["·".to_owned(), "·".to_owned()]),
        ]
    );
    // Everything else is written out as a command's is.
    assert_eq!(drawn("x\r\u{202e} ")[0].0, "x␍⟨U+202E⟩·");
    // A command's tab is still the spaces it reads as.
    assert_eq!(visible("a\tb"), "a       b");
}

#[test]
fn a_path_with_a_part_that_starts_or_ends_with_a_space_is_no_request() {
    let path = |path: &str| {
        let mut input = edit_input();
        input["file_path"] = json!(path);
        from_claude("Edit", &input)
    };
    for spaced in [
        "/work/ src/main.rs",
        "/work/src /main.rs",
        "/work/src/main.rs ",
        "/ work/main.rs",
        "/work/ /main.rs",
    ] {
        assert_eq!(path(spaced), None, "{spaced:?}");
    }
    // A space inside a part is drawn as itself, and is no trick.
    assert!(path("/work/my notes/a b.txt").is_some());
}

#[test]
fn what_the_hook_found_on_disk_is_carried_and_required_but_not_digested() {
    let edit = from_claude("Edit", &edit_input()).expect("an edit");
    // Nothing looked at is the worst a card can say.
    assert!(edit.outside("/work/crook"));
    let inside = edit.clone().seen(OnDisk {
        resolves_outside: false,
        replaces: None,
    });
    assert!(!inside.outside("/work/crook"));
    // Outside by its text whatever the hook found.
    assert!(inside.outside("/work/other"));
    assert_eq!(
        digest("s", "Edit", &edit),
        digest("s", "Edit", &inside),
        "the disk is not the request"
    );

    let write = from_claude(
        "Write",
        &json!({"file_path": "/work/crook/a", "content": "x"}),
    )
    .expect("a write");
    let replacing = write.clone().seen(OnDisk {
        resolves_outside: false,
        replaces: Some(Replaced {
            lines: 3,
            bytes: 12,
            more: false,
        }),
    });
    assert_eq!(
        digest("s", "Write", &write),
        digest("s", "Write", &replacing)
    );
    assert_ne!(
        digest("s", "Write", &write),
        digest(
            "s",
            "Write",
            &from_claude(
                "Write",
                &json!({"file_path": "/work/crook/a", "content": "y"})
            )
            .unwrap()
        )
    );

    // On the wire both are required: a hook that did not look is refused
    // rather than read as having found the file inside and nothing there.
    let encoded = serde_json::to_value(&replacing).expect("encodes");
    assert_eq!(
        encoded["replaces"],
        json!({"lines": 3, "bytes": 12, "more": false})
    );
    assert_eq!(
        serde_json::from_value::<AskRequest>(encoded.clone()).ok(),
        Some(replacing)
    );
    let encoded_none = serde_json::to_value(&write).expect("encodes");
    assert_eq!(encoded_none["replaces"], Value::Null);
    assert_eq!(
        serde_json::from_value::<AskRequest>(encoded_none).ok(),
        Some(write)
    );
    for field in ["replaces", "resolves_outside"] {
        let mut without = encoded.clone();
        without.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<AskRequest>(without).is_err(),
            "{field}"
        );
    }
    let mut without = serde_json::to_value(&inside).expect("encodes");
    without.as_object_mut().unwrap().remove("resolves_outside");
    assert!(serde_json::from_value::<AskRequest>(without).is_err());
    let mut wrong = encoded.clone();
    wrong["replaces"]["mode"] = json!("0644");
    assert!(serde_json::from_value::<AskRequest>(wrong).is_err());
}
