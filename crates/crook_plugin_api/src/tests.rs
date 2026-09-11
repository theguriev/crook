//! What the wire promises.

use super::*;
use alloc::vec;

#[test]
fn a_node_survives_the_wire() {
    let tree = Node::Row(vec![
        Node::Icon {
            name: "git-branch".into(),
            tone: Tone::Muted,
        },
        Node::Gap(Gap::Small),
        Node::Text {
            text: "main".into(),
            size: Size::Small,
            tone: Tone::Primary,
        },
        Node::Badge {
            text: "3".into(),
            tone: Tone::Warning,
        },
    ]);

    let bytes = to_bytes(&tree).expect("a tree should encode");
    let read_back: Node = from_bytes(&bytes).expect("a tree should decode");

    assert_eq!(tree, read_back);
}

#[test]
fn a_manifest_survives_the_wire() {
    let manifest = Manifest {
        abi: ABI_VERSION,
        id: "eugen/ci-status".into(),
        name: "CI status".into(),
        description: "Whether the branch in the active pane is green.".into(),
        version: "0.2.0".into(),
        capabilities: vec![
            Capability::ReadWorkingDirectory,
            Capability::Network(vec!["api.github.com".into()]),
        ],
    };

    let bytes = to_bytes(&manifest).expect("a manifest should encode");
    assert_eq!(
        from_bytes::<Manifest>(&bytes).expect("a manifest should decode"),
        manifest
    );
}

#[test]
fn every_capability_says_what_it_is_in_a_sentence() {
    // The permission dialog prints these and nothing else, so a capability
    // whose sentence is empty is a capability nobody can refuse on purpose.
    for capability in [
        Capability::ReadSettings,
        Capability::ReadTabs,
        Capability::ReadWorkingDirectory,
        Capability::Network(vec!["api.github.com".into(), "example.invalid".into()]),
        Capability::Clipboard,
        Capability::Storage,
        Capability::ReadFiles(vec!["~/.claude/.credentials.json".into()]),
        Capability::WatchCommands,
        Capability::WatchBells,
    ] {
        let sentence = capability.sentence();
        assert!(!sentence.is_empty(), "{capability:?} says nothing");
        // And it is a phrase rather than a name: the dialog reads "This
        // plugin wants to: <sentence>".
        assert!(
            sentence.chars().next().is_some_and(char::is_uppercase),
            "{sentence:?} does not read as a sentence"
        );
    }

    assert_eq!(
        Capability::Network(vec!["api.github.com".into(), "example.invalid".into()]).sentence(),
        "Reach api.github.com, example.invalid"
    );
}

#[test]
fn what_a_plugin_registered_survives_the_wire() {
    let registered = Registered {
        contributions: vec![Contribution {
            slot: "header.right".into(),
            entry: "chip".into(),
            order: 0,
        }],
        actions: vec![
            Action {
                name: "refresh".into(),
                title: Some("Refresh CI status".into()),
            },
            Action {
                name: "internal".into(),
                title: None,
            },
        ],
    };

    let bytes = to_bytes(&registered).expect("it should encode");
    assert_eq!(
        from_bytes::<Registered>(&bytes).expect("it should decode"),
        registered
    );
}

#[test]
fn the_wire_is_compact_enough_to_run_every_frame() {
    // A contribution is decoded once per frame per slot, so the size of a
    // small tree is a thing worth knowing rather than assuming. This is the
    // usage chip's shape.
    let chip = Node::Badge {
        text: "42%".into(),
        tone: Tone::Primary,
    };

    let bytes = to_bytes(&chip).expect("it should encode");

    assert!(bytes.len() < 16, "{} bytes for a chip", bytes.len());
}

#[test]
fn a_grant_is_written_down_one_thing_at_a_time() {
    // The point of a key per host rather than per capability: allowing one
    // host today is not allowing whatever the next version puts beside it.
    assert_eq!(
        Capability::Network(vec!["api.anthropic.com".into(), "example.invalid".into()]).keys(),
        vec![
            String::from("net:api.anthropic.com"),
            String::from("net:example.invalid"),
        ]
    );
    assert_eq!(
        Capability::ReadFiles(vec!["~/.claude/.credentials.json".into()]).keys(),
        vec![String::from("file:~/.claude/.credentials.json")]
    );

    // And every capability answers with something, or a plugin asking for it
    // could never be granted.
    for capability in [
        Capability::ReadSettings,
        Capability::ReadTabs,
        Capability::ReadWorkingDirectory,
        Capability::Clipboard,
        Capability::Storage,
    ] {
        assert_eq!(capability.keys().len(), 1, "{capability:?}");
    }
}

#[test]
fn asking_and_being_answered_survive_the_wire() {
    let asking = Request::Fetch {
        method: Method::Post,
        url: "https://api.anthropic.com/api/oauth/usage".into(),
        headers: vec![("authorization".into(), "Bearer …".into())],
        body: Some(vec![b'{', b'}']),
    };
    let bytes = to_bytes(&asking).expect("a request should encode");
    assert_eq!(
        from_bytes::<Request>(&bytes).expect("a request should decode"),
        asking
    );

    for answer in [
        Answer::Fetched {
            status: 200,
            body: vec![1, 2, 3],
        },
        Answer::Read { bytes: vec![4, 5] },
        Answer::Refused("Reach api.anthropic.com".into()),
        Answer::Failed("timed out".into()),
    ] {
        let bytes = to_bytes(&answer).expect("an answer should encode");
        assert_eq!(
            from_bytes::<Answer>(&bytes).expect("an answer should decode"),
            answer
        );
    }
}

#[test]
fn a_panel_survives_the_wire() {
    // The whole of what version 2 added to the vocabulary, in one tree: the
    // shape a chip with a panel under it actually has.
    let tree = Node::Anchored {
        content: Box::new(Node::Pressable {
            content: Box::new(Node::Row(vec![
                Node::Icon {
                    name: "pirate".into(),
                    tone: Tone::Primary,
                },
                Node::Gap(Gap::Small),
                Node::Text {
                    text: "47%".into(),
                    size: Size::Small,
                    tone: Tone::Primary,
                },
            ])),
            action: "panel".into(),
        }),
        panel: Some(Box::new(Node::Column(vec![
            Node::Row(vec![
                Node::Text {
                    text: "Session".into(),
                    size: Size::Small,
                    tone: Tone::Muted,
                },
                Node::Fill,
                Node::Text {
                    text: "3h 12m left".into(),
                    size: Size::Small,
                    tone: Tone::Primary,
                },
            ]),
            Node::Meter {
                fraction: 0.47,
                tone: Tone::Success,
            },
            Node::Rule,
            Node::Note {
                text: "Read from the session Claude Code stores on this machine.".into(),
                tone: Tone::Muted,
            },
        ]))),
        dismiss: "dismiss".into(),
    };

    let bytes = to_bytes(&tree).expect("a tree should encode");
    assert_eq!(
        from_bytes::<Node>(&bytes).expect("a tree should decode"),
        tree
    );
}

#[test]
fn a_shut_panel_costs_almost_nothing() {
    // A chip is asked for its tree every frame and is shut on nearly all of
    // them, so the shut shape is the one whose size matters.
    let shut = Node::Anchored {
        content: Box::new(Node::Badge {
            text: "47%".into(),
            tone: Tone::Primary,
        }),
        panel: None,
        dismiss: "dismiss".into(),
    };

    let bytes = to_bytes(&shut).expect("it should encode");

    assert!(bytes.len() < 32, "{} bytes for a shut chip", bytes.len());
}

#[test]
fn what_a_render_is_about_survives_the_wire() {
    let render = Render {
        slot: "tab.row.mark".into(),
        entry: "mark".into(),
        subject: Some(Subject::Tab(TabFacts {
            key: 0x9e37_79b9_7f4a_7c15,
            tab: Some(TabInfo {
                title: "crook".into(),
                active: true,
                status: Status::Running,
            }),
            place: Some(Place {
                directory: "/home/somebody/work/crook".into(),
                branch: Some("main".into()),
                worktree: false,
            }),
        })),
    };

    let bytes = to_bytes(&render).expect("it should encode");

    assert_eq!(
        from_bytes::<Render>(&bytes).expect("and decode"),
        render,
        "a render does not survive its own wire"
    );
}

#[test]
fn an_explained_note_survives_the_wire() {
    // The shape a control with its own explanation has: the thing in the slot,
    // and beside it the whole of what a card would otherwise have to say
    // permanently.
    let tree = Node::Explained {
        content: Box::new(Node::Pressable {
            content: Box::new(Node::Badge {
                text: "Microwave".into(),
                tone: Tone::Accent,
            }),
            action: "open".into(),
        }),
        explanation: Box::new(Node::Column(vec![
            Node::Row(vec![
                Node::Text {
                    text: "Microwave".into(),
                    size: Size::Body,
                    tone: Tone::Primary,
                },
                Node::Fill,
                Node::Badge {
                    text: "ringing".into(),
                    tone: Tone::Accent,
                },
            ]),
            Node::Rule,
            Node::Note {
                text: "Plays when a command that ran for two seconds or more finishes.".into(),
                tone: Tone::Muted,
            },
        ])),
    };

    let bytes = to_bytes(&tree).expect("a tree should encode");
    assert_eq!(
        from_bytes::<Node>(&bytes).expect("a tree should decode"),
        tree
    );
}

#[test]
fn a_plugin_granted_nothing_is_still_told_which_row_it_is_drawing() {
    // The redaction, which is what makes a mark per tab something a plugin
    // allowed to know nothing can draw: no title, no directory, and still two
    // rows a plugin can tell apart and keep telling apart.
    let first = TabFacts {
        key: 1,
        tab: None,
        place: None,
    };
    let second = TabFacts {
        key: 2,
        ..first.clone()
    };

    let bytes = to_bytes(&Subject::Tab(first.clone())).expect("it should encode");

    assert_ne!(first.key, second.key);
    assert!(
        bytes.len() < 8,
        "{} bytes for a row a plugin may know nothing about",
        bytes.len()
    );
}

#[test]
fn a_note_is_described_on_every_frame_and_that_is_what_it_costs() {
    // The one price of the host showing a note itself rather than asking for
    // it when the pointer arrives: the words are on the wire whether or not
    // anybody is looking. The alternative is a call into the guest per pointer
    // transition, on the thread that draws — so this is the trade, and the
    // number is here so that a change to it is a change somebody notices.
    let sheet = Node::Note {
        text: "Plays when a command that ran for two seconds or more finishes.".into(),
        tone: Tone::Muted,
    };
    let bare = Node::Badge {
        text: "Microwave".into(),
        tone: Tone::Accent,
    };
    let explained = Node::Explained {
        content: Box::new(bare.clone()),
        explanation: Box::new(sheet),
    };

    let cost = to_bytes(&explained).expect("it should encode").len()
        - to_bytes(&bare).expect("it should encode").len();

    assert!(
        cost < 128,
        "{cost} bytes a frame for one sentence of explanation"
    );
}

#[test]
fn an_event_survives_the_wire() {
    // The only two things the host ever says to a plugin unprompted. A bell
    // is the smaller of them and the one with a flag in it, and a flag that
    // came back wrong would be a plugin ringing at every ambiguous Tab
    // completion somebody's shell answers.
    for event in [
        Event::CommandFinished {
            pane: 7,
            exit: Some(1),
            took_millis: Some(4_200),
        },
        Event::Bell {
            pane: 7,
            while_running: true,
        },
        Event::Bell {
            pane: 7,
            while_running: false,
        },
    ] {
        let bytes = to_bytes(&event).expect("an event encodes");
        assert_eq!(
            from_bytes::<Event>(&bytes).expect("and decodes"),
            event,
            "{event:?} did not survive the wire"
        );
    }
}

#[test]
fn the_crate_version_names_the_abi() {
    // A plugin writes `crook_plugin_api = "0.8"` and Cargo resolves that to
    // every `0.8.x`, which is exactly the set of releases that speak ABI 8.
    // That equivalence is the whole of a plugin author's compatibility story,
    // and it holds only while these two numbers are bumped together — so the
    // one that is easy to forget is asserted here rather than remembered.
    // The patch is free: 0.8.1 added the picture macros and moved nothing on
    // the wire, which is the kind of release that may happen again.
    let minor = env!("CARGO_PKG_VERSION")
        .split('.')
        .nth(1)
        .and_then(|minor| minor.parse::<u32>().ok())
        .expect("the crate version is major.minor.patch");

    assert_eq!(
        env!("CARGO_PKG_VERSION_MAJOR"),
        "0",
        "at 1.0 the ABI is the major version and this test is the wrong one"
    );
    assert_eq!(
        minor, ABI_VERSION,
        "the crate is at 0.{minor} and the ABI is {ABI_VERSION}: bump the version in \
         crates/crook_plugin_api/Cargo.toml with the constant"
    );
}

// The macros, expanded on the host. The README stands in for a PNG: what is
// being asserted is that a path resolves against the file the macro is
// *written in* and that the bytes come through — the rules about what the
// bytes have to be are the reader's, in `crook_wasm`.
crate::icon!("../README.md");
crate::preview!(3, "../README.md", "abc");
crate::preview!(6, "../README.md");

#[test]
fn a_picture_named_from_the_calling_file_is_in_the_static() {
    // `include_bytes!` inside an exported macro resolves against the file
    // that invoked the macro, not against this crate's own `lib.rs` — which
    // is what lets a plugin write `../../../assets/icon.png` and reach its
    // repository root. Off wasm the static is in no section at all, so this
    // is also the proof that a plugin's `cargo test` on a laptop still links.
    assert!(CROOK_ICON.starts_with(b"# crook_plugin_api"));
    assert_eq!(CROOK_PREVIEW_3, CROOK_ICON);
    assert_eq!(CROOK_PREVIEW_6, CROOK_ICON);
}

#[test]
fn a_caption_is_its_own_bytes_exactly() {
    // A string literal is a reference and a `#[link_section]` static has to
    // be an array, so the caption is copied into one by length at compile
    // time. Exactly its bytes: no terminator, no length prefix, nothing the
    // reader would have to know to strip.
    assert_eq!(CROOK_CAPTION_3, *b"abc");
    assert_eq!(pictures::caption_bytes::<0>(""), [0u8; 0]);
    assert_eq!(pictures::caption_bytes::<7>("héllo!"), *"héllo!".as_bytes());
}

#[test]
fn the_section_names_a_reader_looks_for_are_the_ones_the_macros_write() {
    // The macros spell every section out as a literal, so that what lands in
    // a `#[link_section]` is readable here rather than pasted together; the
    // reader composes the same names from these constants. The two cannot be
    // compared through the macro on a host build — the section is gated on
    // wasm — so the source is read instead: each arm's literal is looked
    // for in the text of the macro, which is what catches `crook.preview.4`
    // under the arm for 5.
    let source = include_str!("lib.rs");
    let icon = source
        .split("macro_rules! icon")
        .nth(1)
        .expect("the icon macro is in the crate root");
    assert!(
        icon.contains(&alloc::format!(
            "link_section = \"{}\"",
            pictures::ICON_SECTION
        )),
        "the icon macro does not write {}",
        pictures::ICON_SECTION
    );

    let preview = source
        .split("macro_rules! preview")
        .nth(1)
        .expect("the preview macro is in the crate root");
    for n in 1..=pictures::MAX_PREVIEWS {
        let arm = preview
            .split(&alloc::format!("({n}, $path:literal)"))
            .nth(1)
            .unwrap_or_else(|| panic!("no arm for preview {n}"));
        // The arm's own body, up to the next arm: the section it names has
        // to be this number's, and the caption arm's the same.
        let body: alloc::string::String = arm.chars().take(200).collect();
        assert!(
            body.contains(&alloc::format!(
                "link_section = \"{}{n}\"",
                pictures::PREVIEW_SECTION_PREFIX
            )),
            "the arm for preview {n} writes another section: {body}"
        );
        let captioned = preview
            .split(&alloc::format!("({n}, $path:literal, $caption:literal)"))
            .nth(1)
            .unwrap_or_else(|| panic!("no captioned arm for preview {n}"));
        let body: alloc::string::String = captioned.chars().take(250).collect();
        assert!(
            body.contains(&alloc::format!(
                "link_section = \"{}{n}\"",
                pictures::CAPTION_SECTION_PREFIX
            )),
            "the captioned arm for preview {n} writes another section: {body}"
        );
    }
    assert!(
        !preview.contains(&alloc::format!(
            "({}, $path:literal)",
            pictures::MAX_PREVIEWS + 1
        )),
        "the macro has an arm past MAX_PREVIEWS"
    );
}
