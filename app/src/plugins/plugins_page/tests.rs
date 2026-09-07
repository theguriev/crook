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

#[test]
fn a_plugin_nobody_answered_for_says_so_where_its_own_controls_are() {
    // The dead Play button, as the card now tells it. A sandboxed plugin that
    // draws a control on its own card is drawing something the host will
    // refuse every time, and the press before this line existed produced
    // nothing at all — no sound, no message, no change to the button.
    let manifest = asking(vec![Capability::PlaySound]);

    assert_eq!(
        stalled(&manifest, &[]),
        Some(
            "Nothing this plugin asks for has been allowed yet, so a control here that needs it \
             is refused rather than broken. The answer is above."
        )
    );
}

#[test]
fn a_plugin_that_was_allowed_is_left_to_speak_for_itself() {
    // The line is about a refusal that is going to happen. A plugin whose
    // grant covers what it asks for has nothing refused, and a warning drawn
    // over working controls is a warning nobody believes the next time.
    let manifest = asking(vec![Capability::PlaySound, Capability::WatchCommands]);

    assert_eq!(
        stalled(&manifest, &granted(&["sound.play", "commands.watch"])),
        None
    );
}

#[test]
fn a_plugin_asking_for_more_than_was_allowed_says_which_way_it_is_stalled() {
    // Half a grant is the case the sentence has to get right: what was
    // allowed still works, so "nothing is allowed" would be a lie about the
    // controls that do work.
    let manifest = asking(vec![Capability::PlaySound, Capability::WatchCommands]);

    assert_eq!(
        stalled(&manifest, &granted(&["commands.watch"])),
        Some(
            "This plugin asks for more than you allowed, so a control here that needs the rest \
             is refused rather than broken. The answer is above."
        )
    );
}

#[test]
fn a_plugin_that_asks_for_nothing_cannot_be_stalled_by_a_grant() {
    // Every native plugin, and the reason the check is on the capabilities
    // rather than on the grant: a plugin that wants nothing is granted
    // nothing, and reading that as "not allowed yet" would put the sentence
    // under every built-in control on the page.
    let native = asking(Vec::new());

    assert_eq!(stalled(&native, &[]), None);
}

#[test]
fn a_plugin_that_drew_its_own_controls_has_its_list_counted_instead() {
    // The card's longest section, on the card of a plugin whose whole surface
    // is one row. Both halves say where to go, because a count that only says
    // a thing exists is worse than the list it replaced.
    assert_eq!(
        elsewhere(8, 2),
        "8 commands, which the command palette lists, and 2 more reachable by name from your \
         keybindings file."
    );

    // Nothing held back: no second clause about actions that are not there.
    assert_eq!(
        elsewhere(6, 0),
        "6 commands, which the command palette lists."
    );

    // And nothing offered: the sentence has to be able to start on its own
    // rather than opening with "and".
    assert_eq!(
        elsewhere(0, 3),
        "3 actions, reachable by name from your keybindings file."
    );
}

#[test]
fn one_command_is_not_one_commands() {
    assert_eq!(
        elsewhere(1, 0),
        "1 command, which the command palette lists."
    );
}

#[test]
fn an_action_that_is_not_offered_is_reachable_where_it_is_actually_reachable() {
    // This used to send people to the Keyboard Shortcuts page, which is built
    // from the *titled* commands and so lists none of these. The one place an
    // untitled action can be reached is a rule naming it in the keybindings
    // file, so that is what the card says.
    assert_eq!(
        only_by_name(4),
        "and 4 more it does not offer, reachable by name from your keybindings file."
    );
}
