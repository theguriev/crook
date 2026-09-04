//! What the vocabulary and the registries promise.

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
