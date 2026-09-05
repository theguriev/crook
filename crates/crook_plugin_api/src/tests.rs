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
