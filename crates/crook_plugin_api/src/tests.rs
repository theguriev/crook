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
