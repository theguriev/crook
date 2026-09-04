//! What the vocabulary and the registries promise.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::*;

fn plugin(id: &str) -> PluginId {
    PluginId::parse(id).expect("a well-formed id")
}

fn action(name: &str) -> ActionName {
    ActionName::parse(name).expect("a well-formed action name")
}

const HEADER: SlotId = SlotId::new("header.right");
const CHIPS: SlotId = SlotId::new("tab.row.chips");

#[test]
fn a_plugin_id_is_an_owner_and_a_name() {
    let id = plugin("eugen/ci-status");

    assert_eq!(id.owner(), "eugen");
    assert_eq!(id.name(), "ci-status");
    assert_eq!(id.to_string(), "eugen/ci-status");
}

#[test]
fn a_name_with_the_wrong_number_of_parts_is_refused() {
    // The three shapes somebody will try on their first attempt.
    assert!(PluginId::parse("ci-status").is_err());
    assert!(PluginId::parse("eugen/ci/status").is_err());
    assert!(ActionName::parse("eugen/ci-status").is_err());
}

#[test]
fn an_uppercase_name_is_refused() {
    // Two directories that differ by case are one directory on macOS and
    // Windows, and two rows in an index everywhere. Better to have neither.
    assert!(PluginId::parse("Eugen/themes").is_err());
    assert!(PluginId::parse("eugen/Themes").is_err());
}

#[test]
fn a_name_that_could_escape_its_directory_is_refused() {
    // A plugin id is also a path under the store's checkout. `.` is left out
    // of the alphabet for exactly this.
    assert!(PluginId::parse("../etc/themes").is_err());
    assert!(PluginId::parse("eugen/..").is_err());
    assert!(PluginId::parse("eugen/").is_err());
    assert!(PluginId::parse("/themes").is_err());
}

#[test]
fn an_action_knows_which_plugin_owns_it() {
    assert_eq!(
        action("eugen/ci-status/refresh").plugin(),
        plugin("eugen/ci-status")
    );
}

#[test]
fn dropping_a_registration_takes_the_contribution_back_out() {
    let slots: Slots<&'static str> = Slots::new();
    let owner = plugin("crook/header");
    let _declared = slots.declare(&owner, HEADER, Cardinality::Single);

    let contribution = slots.contribute(
        &plugin("eugen/ci-status"),
        HEADER,
        EntryId::new("chip"),
        0,
        "ci",
    );
    assert_eq!(slots.map(HEADER, |entry| *entry), ["ci"]);

    drop(contribution);
    assert!(slots.is_empty(HEADER), "the entry outlived its guard");
}

#[test]
fn a_registration_that_is_kept_forever_is_never_taken_back() {
    let slots: Slots<&'static str> = Slots::new();
    let owner = plugin("crook/header");
    slots
        .declare(&owner, HEADER, Cardinality::Single)
        .keep_forever();
    slots
        .contribute(&owner, HEADER, EntryId::new("chip"), 0, "usage")
        .keep_forever();

    assert_eq!(slots.map(HEADER, |entry| *entry), ["usage"]);
}

#[test]
fn entries_are_drawn_in_order_and_then_by_when_they_arrived() {
    // Two entries with one `order` keep the order they were registered in,
    // which for a list of plugins is the order they loaded — deterministic,
    // and the one thing a contributor can reason about without knowing who
    // else is in the slot.
    let slots: Slots<&'static str> = Slots::new();
    let owner = plugin("crook/rows");
    slots
        .declare(&owner, CHIPS, Cardinality::List)
        .keep_forever();

    for (order, payload) in [(10, "diff"), (0, "branch"), (10, "pr")] {
        slots
            .contribute(&owner, CHIPS, EntryId::new(payload), order, payload)
            .keep_forever();
    }

    assert_eq!(slots.map(CHIPS, |entry| *entry), ["branch", "diff", "pr"]);
}

#[test]
fn a_single_slot_draws_the_lowest_order_and_says_the_rest_were_offered() {
    let slots: Slots<&'static str> = Slots::new();
    let owner = plugin("crook/header");
    slots
        .declare(&owner, HEADER, Cardinality::Single)
        .keep_forever();

    slots
        .contribute(&owner, HEADER, EntryId::new("usage"), 10, "usage")
        .keep_forever();
    slots
        .contribute(
            &plugin("eugen/ci-status"),
            HEADER,
            EntryId::new("ci"),
            0,
            "ci",
        )
        .keep_forever();

    assert_eq!(slots.one(HEADER, |entry| *entry), Some("ci"));
    assert!(matches!(
        slots.audit().as_slice(),
        [Complaint::SlotIsSingle { entries: 2, .. }]
    ));
}

#[test]
fn contributing_to_a_slot_nobody_declares_is_a_complaint_and_not_a_failure() {
    // A typo in a slot name, which is the likeliest mistake a plugin author
    // makes and must not be one that costs anybody their window.
    let slots: Slots<&'static str> = Slots::new();
    let by = plugin("eugen/ci-status");
    slots
        .contribute(
            &by,
            SlotId::new("header.rihgt"),
            EntryId::new("chip"),
            0,
            "ci",
        )
        .keep_forever();

    let complaints = slots.audit();
    assert!(matches!(
        complaints.as_slice(),
        [Complaint::SlotNotDeclared { .. }]
    ));
    assert!(
        complaints[0].to_string().contains("eugen/ci-status"),
        "a complaint has to name who to go and talk to: {}",
        complaints[0]
    );
}

#[test]
fn one_complaint_per_plugin_per_slot_however_many_entries_it_offered() {
    let slots: Slots<&'static str> = Slots::new();
    let by = plugin("eugen/ci-status");
    for entry in ["a", "b", "c"] {
        slots
            .contribute(&by, SlotId::new("nowhere"), EntryId::new(entry), 0, entry)
            .keep_forever();
    }

    assert_eq!(slots.audit().len(), 1);
}

#[test]
fn a_slot_has_one_owner_and_the_first_one_keeps_it() {
    let slots: Slots<&'static str> = Slots::new();
    let first = plugin("crook/header");
    let second = plugin("eugen/ci-status");
    slots
        .declare(&first, HEADER, Cardinality::Single)
        .keep_forever();
    let refused = slots.declare(&second, HEADER, Cardinality::List);

    assert!(matches!(
        slots.audit().as_slice(),
        [Complaint::SlotDeclaredTwice { .. }]
    ));
    // And the refusal still hands back a guard, so a caller collecting them
    // never has to ask whether one arrived.
    drop(refused);
}

#[test]
fn audit_is_a_question_and_gives_the_same_answer_twice() {
    // The plugins page asks this every time it draws, so an audit that emptied
    // itself on the first ask would show a problem once and then claim there
    // was none.
    let slots: Slots<&'static str> = Slots::new();
    let by = plugin("eugen/ci-status");
    let contribution = slots.contribute(&by, SlotId::new("nowhere"), EntryId::new("chip"), 0, "ci");

    assert_eq!(slots.audit().len(), 1);
    assert_eq!(slots.audit().len(), 1);

    // And it is a question about what is registered *now*: disabling the
    // plugin answers it.
    drop(contribution);
    assert!(slots.audit().is_empty());
}

#[test]
fn a_slot_can_say_who_put_each_thing_in_it() {
    // The plugins page has to be able to answer "what is this, and who is
    // responsible for it", and a host that catches a panic has to have
    // somebody to blame.
    let slots: Slots<&'static str> = Slots::new();
    let owner = plugin("crook/rows");
    slots
        .declare(&owner, CHIPS, Cardinality::List)
        .keep_forever();
    slots
        .contribute(&owner, CHIPS, EntryId::new("diff"), 10, "diff")
        .keep_forever();
    slots
        .contribute(
            &plugin("eugen/ci-status"),
            CHIPS,
            EntryId::new("ci"),
            0,
            "ci",
        )
        .keep_forever();

    assert_eq!(
        slots.contributors(CHIPS),
        [
            (plugin("eugen/ci-status"), EntryId::new("ci")),
            (plugin("crook/rows"), EntryId::new("diff")),
        ]
    );
}

#[test]
fn an_action_is_found_by_name_and_gone_when_its_guard_is() {
    let actions: Actions<fn() -> u32> = Actions::new();
    let owner = plugin("eugen/ci-status");
    let name = action("eugen/ci-status/refresh");

    let registration = actions.register(&owner, name.clone(), || 7);
    assert_eq!(actions.with(&name, |handler| handler()), Some(7));

    drop(registration);
    assert!(!actions.contains(&name));
    assert_eq!(actions.with(&name, |handler| handler()), None);
}

#[test]
fn the_first_registration_of_an_action_keeps_the_name() {
    let actions: Actions<fn() -> u32> = Actions::new();
    let name = action("eugen/ci-status/refresh");
    actions
        .register(&plugin("eugen/ci-status"), name.clone(), || 1)
        .keep_forever();
    actions
        .register(&plugin("masha/other"), name.clone(), || 2)
        .keep_forever();

    assert_eq!(actions.with(&name, |handler| handler()), Some(1));
    assert!(matches!(
        actions.audit().as_slice(),
        [Complaint::ActionTaken { .. }]
    ));
}

#[test]
fn every_action_can_be_listed_for_a_palette() {
    let actions: Actions<fn() -> u32> = Actions::new();
    let owner = plugin("crook/tabs");
    for name in ["crook/tabs/close", "crook/tabs/new"] {
        actions.register(&owner, action(name), || 0).keep_forever();
    }

    let listed: Vec<String> = actions
        .names()
        .into_iter()
        .map(|(name, _)| name.to_string())
        .collect();
    assert_eq!(listed, ["crook/tabs/close", "crook/tabs/new"]);
}

#[test]
fn a_registry_dropped_before_its_guards_does_not_panic() {
    // The order things are dropped in during shutdown is not something a
    // plugin author will get right, so it must not matter.
    let guard = {
        let slots: Slots<&'static str> = Slots::new();
        let owner = plugin("crook/header");
        slots
            .declare(&owner, HEADER, Cardinality::Single)
            .keep_forever();
        slots.contribute(&owner, HEADER, EntryId::new("chip"), 0, "usage")
    };

    drop(guard);
}

#[test]
fn an_actions_handler_can_reach_the_registry_that_is_running_it() {
    // The obvious thing to write, and a panic until `with` stopped holding the
    // borrow across the call: a "disable this plugin" button is an action, and
    // the first thing its handler does is drop the registrations of the plugin
    // it belongs to — this one included.
    let registry: Actions<Box<dyn Fn()>> = Actions::new();
    let owner = plugin("eugen/ci-status");
    let name = action("eugen/ci-status/refresh");

    let seen = Rc::new(RefCell::new(Vec::new()));
    let reentrant = {
        let registry = registry.clone();
        let name = name.clone();
        let seen = seen.clone();
        move || {
            seen.borrow_mut()
                .push(usize::from(registry.contains(&name)));
            seen.borrow_mut().push(registry.names().len());
            // And the registry may be changed from inside a handler.
            registry
                .register(
                    &plugin("eugen/other"),
                    action("eugen/other/thing"),
                    Box::new(|| {}),
                )
                .keep_forever();
        }
    };
    registry
        .register(&owner, name.clone(), Box::new(reentrant) as Box<dyn Fn()>)
        .keep_forever();

    registry.with(&name, |handler| handler());

    assert_eq!(
        seen.borrow().as_slice(),
        &[1, 1],
        "the action was invisible to its own handler"
    );
    assert!(registry.contains(&action("eugen/other/thing")));
    // And it is still there afterwards, run twice as happily as once.
    assert!(registry.contains(&name));
    registry.with(&name, |handler| handler());
}

#[test]
fn an_action_that_invokes_itself_does_it_once() {
    // The price of not holding the borrow, written down as a test rather than
    // left to be discovered: recursion through the registry stops at one hop.
    let registry: Actions<Box<dyn Fn()>> = Actions::new();
    let owner = plugin("eugen/loop");
    let name = action("eugen/loop/again");
    let runs = Rc::new(Cell::new(0));

    let handler = {
        let registry = registry.clone();
        let name = name.clone();
        let runs = runs.clone();
        move || {
            runs.set(runs.get() + 1);
            registry.with(&name, |handler| handler());
        }
    };
    registry
        .register(&owner, name.clone(), Box::new(handler) as Box<dyn Fn()>)
        .keep_forever();

    registry.with(&name, |handler| handler());

    assert_eq!(runs.get(), 1);
}

#[test]
fn a_handler_that_unregisters_itself_is_gone_when_it_returns() {
    let registry: Actions<Box<dyn Fn()>> = Actions::new();
    let owner = plugin("eugen/once");
    let name = action("eugen/once/run");
    let guard = Rc::new(RefCell::new(None::<Registration>));

    let handler = {
        let guard = guard.clone();
        move || {
            // What "disable the plugin that owns this action" does.
            guard.borrow_mut().take();
        }
    };
    *guard.borrow_mut() = Some(registry.register(&owner, name.clone(), Box::new(handler)));

    registry.with(&name, |handler| handler());

    assert!(
        !registry.contains(&name),
        "the handler put itself back after taking itself out"
    );
}

#[test]
fn one_entry_can_be_reached_by_its_place_in_the_order() {
    // What a rail of pages needs: show the third one without building the
    // other four.
    let slots: Slots<&'static str> = Slots::new();
    let owner = plugin("crook/settings");
    slots
        .declare(&owner, CHIPS, Cardinality::List)
        .keep_forever();
    for (order, payload) in [(10, "keys"), (0, "appearance"), (20, "about")] {
        slots
            .contribute(&owner, CHIPS, EntryId::new(payload), order, payload)
            .keep_forever();
    }

    assert_eq!(slots.at(CHIPS, 0, |entry| *entry), Some("appearance"));
    assert_eq!(slots.at(CHIPS, 2, |entry| *entry), Some("about"));
    assert_eq!(slots.at(CHIPS, 3, |entry| *entry), None);
    // And it is the same order the contributors are listed in, so an index
    // taken from one names the same entry in the other.
    assert_eq!(
        slots.contributors(CHIPS)[1].1,
        EntryId::new("keys"),
        "the two orders disagree"
    );
}

#[test]
fn a_manifest_says_what_the_plugin_wants_to_be_allowed_to_do() {
    // The rule this field exists for: what a plugin asks for has to survive
    // the trip into the manifest the interface reads, or the page that asks a
    // person to allow something has nothing to show them — and the only
    // honest answer to "what may this do?" becomes "install it and find out".
    let manifest = Manifest {
        schema: Manifest::SCHEMA,
        id: plugin("eugen/ci-status"),
        name: "CI status",
        description: "Whether the build is green.",
        version: "0.1.0",
        tier: Tier::Wasm,
        capabilities: Vec::leak(vec![crook_plugin_api::Capability::Network(vec![
            "api.github.com".to_owned(),
        ])]),
    };

    assert_eq!(
        manifest.capabilities[0].keys(),
        vec!["net:api.github.com".to_owned()]
    );
}

#[test]
fn two_manifests_that_ask_for_different_things_are_different_manifests() {
    // Equality has to notice a changed capability list, because that is the
    // one difference between two versions of a plugin that a person has to be
    // asked about again.
    let quiet = Manifest {
        schema: Manifest::SCHEMA,
        id: plugin("eugen/ci-status"),
        name: "CI status",
        description: "Whether the build is green.",
        version: "0.1.0",
        tier: Tier::Wasm,
        capabilities: &[],
    };
    let curious = Manifest {
        capabilities: Vec::leak(vec![crook_plugin_api::Capability::Clipboard]),
        ..quiet.clone()
    };

    assert_ne!(quiet, curious);
}
