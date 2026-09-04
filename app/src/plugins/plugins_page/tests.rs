//! What the card promises about a grant.
//!
//! The arithmetic and not the drawing: which of the three things a person is
//! being told, and which keys "Allow" writes down. Those are the two places a
//! mistake would be silent — a card that says "Allowed" about a plugin asking
//! for one more host than was agreed to is a card that has hidden the only
//! thing worth showing.

use crook_plugin::{Manifest, PluginId, Tier};
use crook_plugin_api::Capability;

use super::*;

/// A manifest asking for `capabilities`, of the tier that can ask for
/// anything.
///
/// Leaked, which is what `wasm::open` does with a manifest read out of a
/// module: the field is `&'static` because a native plugin's manifest is a
/// constant, and a plugin loaded at runtime pays one leak, once.
fn asking(capabilities: Vec<Capability>) -> Manifest {
    Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("eugen/ci-status").expect("a literal that parses"),
        name: "CI status",
        description: "Whether the build is green.",
        version: "0.1.0",
        tier: Tier::Wasm,
        capabilities: Vec::leak(capabilities),
    }
}

/// The keys somebody allowed, spelled the way the settings keep them.
fn granted(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

#[test]
fn allowing_writes_one_key_per_host_and_per_path() {
    // The rule the whole mechanism rests on: a grant is a list of the things
    // granted, not a yes. Two hosts on one capability are two keys, so
    // allowing one of them later cannot be mistaken for allowing the other.
    let manifest = asking(vec![
        Capability::ReadTabs,
        Capability::Network(vec![
            "api.github.com".to_owned(),
            "api.anthropic.com".to_owned(),
        ]),
    ]);

    assert_eq!(
        wanted(&manifest),
        granted(&["tabs.read", "net:api.github.com", "net:api.anthropic.com"])
    );
}

#[test]
fn a_plugin_nobody_has_answered_for_is_asking_and_not_allowed() {
    // Asking is not being granted. Every plugin installs in this state, and a
    // page that drew it as allowed would be a page that granted by displaying.
    let manifest = asking(vec![Capability::ReadSettings]);

    assert_eq!(stance(&wanted(&manifest), &[]), Stance::Unanswered);
}

#[test]
fn a_plugin_whose_every_key_was_allowed_is_allowed() {
    let manifest = asking(vec![
        Capability::ReadTabs,
        Capability::ReadFiles(vec!["~/.claude/.credentials.json".to_owned()]),
    ]);

    assert_eq!(
        stance(
            &wanted(&manifest),
            &granted(&["tabs.read", "file:~/.claude/.credentials.json"])
        ),
        Stance::Allowed
    );
}

#[test]
fn one_more_host_than_was_allowed_is_an_escalation() {
    // The reason keys are stored rather than an answer: the plugin's next
    // version added a host to a capability that was already allowed, and the
    // page has to be able to say so instead of inheriting the old yes.
    let manifest = asking(vec![Capability::Network(vec![
        "api.github.com".to_owned(),
        "telemetry.example.com".to_owned(),
    ])]);

    assert_eq!(
        stance(&wanted(&manifest), &granted(&["net:api.github.com"])),
        Stance::Escalated
    );
}

#[test]
fn a_whole_capability_that_is_new_is_an_escalation_too() {
    let manifest = asking(vec![Capability::ReadTabs, Capability::Clipboard]);

    assert_eq!(
        stance(&wanted(&manifest), &granted(&["tabs.read"])),
        Stance::Escalated
    );
}

#[test]
fn a_grant_that_covers_more_than_is_asked_for_is_still_an_allowance() {
    // A plugin whose new version dropped a host has taken something back, and
    // interrupting somebody to re-allow a shorter list would teach them that
    // the question means nothing. The stale key is pruned by the next Allow.
    let manifest = asking(vec![Capability::ReadTabs]);

    assert_eq!(
        stance(
            &wanted(&manifest),
            &granted(&["tabs.read", "net:api.github.com"])
        ),
        Stance::Allowed
    );
}

#[test]
fn a_capability_is_covered_only_when_every_key_of_it_is() {
    // What decides whether a line on the card reads "allowed" or "new". One
    // sentence names both hosts, so one host missing makes the sentence
    // something that has not been allowed.
    let both = Capability::Network(vec![
        "api.github.com".to_owned(),
        "telemetry.example.com".to_owned(),
    ]);

    assert!(!covered(&both, &granted(&["net:api.github.com"])));
    assert!(covered(
        &both,
        &granted(&["net:api.github.com", "net:telemetry.example.com"])
    ));
}

#[test]
fn a_plugin_that_asks_for_nothing_has_nothing_to_allow() {
    // Every native plugin. `wanted` being empty is what keeps the block off
    // the card entirely, rather than drawing a heading with no lines under it.
    let native = asking(Vec::new());

    assert!(wanted(&native).is_empty());
}
