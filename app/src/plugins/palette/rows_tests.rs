//! What the list holds, and where the keyboard may be in it.
//!
//! No window: every one of these is a question about an index space, a sort or
//! a set of keys, and none of them needs a frame to answer. The surface these
//! feed is tested through the real presenter next door, in `workspace/tests`.

use std::collections::BTreeMap;

use crookui_core::fonts::FamilyId;

use crate::keybindings::{DEFAULTS_MAC, DEFAULTS_OTHER, Source, When, rule_from};
use crate::workspace::Fonts;

use super::*;

fn name(text: &str) -> ActionName {
    ActionName::parse(text).expect("a literal that parses")
}

fn owner(text: &str) -> PluginId {
    PluginId::parse(text).expect("a literal that parses")
}

/// A host with nothing loaded into it.
///
/// [`Host::new`] wants no window — it is the registries and nothing else — so
/// a test can register an action, ask what answers to a name, and mint the
/// `ActionId`s a row carries without opening one. Nothing is loaded, so
/// `name_of` falls back to the plugin id, which is what a heading reads as
/// here.
fn host() -> Host {
    Host::new(
        Fonts {
            ui: FamilyId(0),
            monospace: FamilyId(0),
        },
        BTreeMap::new(),
    )
}

/// An id to hang a row on.
///
/// An `ActionId` is an index into what a host registered and cannot be written
/// down, and which id it is never matters to an index space that only carries
/// it from the list to the dispatch.
fn an_id() -> ActionId {
    host().register_action(name("crook/test/act"), |_, _| {})
}

/// The action name a row about `title` would have, so that `find` has
/// something to find.
fn slug(title: &str) -> String {
    title.to_lowercase().replace(' ', "-")
}

fn entry(title: &str) -> Entry {
    Entry {
        action: ActionName::parse(&format!("crook/test/{}", slug(title))).ok(),
        title: title.to_owned(),
        detail: None,
        chords: Vec::new(),
        note: None,
    }
}

/// The same, with a key on it, for the orderings that turn on whether one is
/// bound.
fn bound(title: &str, text: &str) -> Entry {
    let mut entry = entry(title);
    entry.chords.push(Chord {
        text: text.to_owned(),
        only_when: None,
    });
    entry
}

fn heading() -> Row {
    Row::Group("Window commands".to_owned())
}

fn command(title: &str) -> Row {
    Row::Command(entry(title), Target::plain(an_id()))
}

fn fact(title: &str) -> Row {
    Row::Fact(entry(title))
}

/// `[Group, Command, Command, Group, Command]` — the shape every index method
/// has to be right about, and the one every off-by-headings mistake shows up
/// in.
fn grouped_list() -> Rows {
    Rows::new(
        Mode::Keys,
        vec![
            heading(),
            command("Close the pane"),
            command("Find in output"),
            heading(),
            command("Split to the right"),
        ],
    )
}

fn headings(rows: &[Row]) -> Vec<&str> {
    rows.iter()
        .filter_map(|row| match row {
            Row::Group(name) => Some(name.as_str()),
            Row::Command(..) | Row::Fact(_) => None,
        })
        .collect()
}

fn titles(rows: &[Row]) -> Vec<&str> {
    rows.iter()
        .filter_map(|row| match row {
            Row::Command(entry, _) | Row::Fact(entry) => Some(entry.title.as_str()),
            Row::Group(_) => None,
        })
        .collect()
}

/// Every line in order, headings marked, so that an assertion about the list
/// reads the way the list is drawn.
fn lines(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            Row::Group(name) => format!("# {name}"),
            Row::Command(entry, _) => entry.title.clone(),
            Row::Fact(entry) => format!("{} (fact)", entry.title),
        })
        .collect()
}

fn rule(text: &str, command: &str) -> Rule {
    rule_from(text, name(command), Source::Default).expect("a chord that parses")
}

/// Every shipped rule for one command, out of one of the tables that ship.
fn rules_for(table: &[(&str, &str)], command: &str) -> Vec<Rule> {
    table
        .iter()
        .filter(|(_, named)| *named == command)
        .map(|(text, named)| rule(text, named))
        .collect()
}

fn collapsed(rules: &[Rule]) -> Vec<Chord> {
    let rules: Vec<&Rule> = rules.iter().collect();
    chord::collapse(&rules)
}

fn texts(rules: &[Rule]) -> Vec<String> {
    collapsed(rules).into_iter().map(|key| key.text).collect()
}

fn clauses(rules: &[Rule]) -> Vec<Option<String>> {
    collapsed(rules)
        .into_iter()
        .map(|key| key.only_when)
        .collect()
}

/// The rules a command answers to, filed under it, the way a list builds it.
fn by(rules: &[Rule]) -> HashMap<&ActionName, Vec<&Rule>> {
    let mut by_command: HashMap<&ActionName, Vec<&Rule>> = HashMap::new();
    for rule in rules {
        by_command.entry(&rule.command).or_default().push(rule);
    }
    by_command
}

fn pane(label: &str) -> &'static PaneKey {
    PANE_KEYS
        .iter()
        .find(|key| key.label == label)
        .expect("a label out of the table this test is about")
}

#[test]
fn a_sigil_at_the_front_asks_for_one_list() {
    assert_eq!(Mode::of("?split"), (Mode::Keys, "split"));
    assert_eq!(Mode::of("?"), (Mode::Keys, ""));
    assert_eq!(Mode::of(">split"), (Mode::Commands, "split"));
    assert_eq!(Mode::of("@crook"), (Mode::Tabs, "crook"));
    assert_eq!(Mode::of("#theme"), (Mode::Settings, "theme"));
    assert_eq!(Mode::of(""), (Mode::Everything, ""));

    // Only the first character, so that a query which happens to contain one
    // is still a query.
    assert_eq!(Mode::of("split ?"), (Mode::Everything, "split ?"));
    assert_eq!(Mode::of("a > b"), (Mode::Everything, "a > b"));
}

#[test]
fn tab_walks_the_lists_and_comes_back_to_the_first() {
    // Five presses, five lists, and the search in the box survives every one
    // of them: that is the whole argument for keeping the mode in the text.
    let mut mode = Mode::Everything;
    let mut seen = vec![mode.seeded("split")];
    for _ in 0..Mode::ORDER.len() {
        mode = mode.next();
        seen.push(mode.seeded("split"));
    }

    assert_eq!(
        seen,
        vec!["split", "?split", ">split", "@split", "#split", "split"]
    );
    assert_eq!(mode, Mode::Everything);
}

#[test]
fn the_arrows_step_over_a_heading() {
    let rows = grouped_list();

    assert_eq!(rows.stepped(2, 1), Some(4));
    // Down off the last row wraps past the heading the list opens with.
    assert_eq!(rows.stepped(4, 1), Some(1));
    assert_eq!(rows.stepped(1, -1), Some(4));

    assert_eq!(rows.first_command(), Some(1));
    assert_eq!(rows.find(&name("crook/test/find-in-output")), Some(2));
    assert_eq!(rows.mode(), Mode::Keys);
    assert_eq!(rows.rows().len(), 5);
    assert!(!rows.is_empty());
}

#[test]
fn a_list_with_nothing_to_land_on_comes_back() {
    // The spin-forever test. A query answered by nothing but pane keys makes a
    // list with no command in it every day — `?sigint` is one — and two bare
    // headings is the same shape stripped to its bones, which is why the bound
    // has to be here rather than in the caller.
    let all_headings = Rows::new(Mode::Keys, vec![heading(), heading()]);
    assert_eq!(all_headings.stepped(0, 1), None);
    assert_eq!(all_headings.stepped(0, -1), None);
    assert_eq!(all_headings.settled(0), None);
    assert_eq!(all_headings.first_command(), None);

    let nothing = Rows::new(Mode::Everything, Vec::new());
    assert_eq!(nothing.stepped(0, 1), None);
    assert_eq!(nothing.settled(0), None);
    assert!(nothing.is_empty());
}

#[test]
fn a_selection_left_on_a_heading_is_moved_off_it() {
    let rows = grouped_list();

    // Backwards first, so a list that shrank under the selection keeps a
    // person near where they were rather than jumping forward past a heading.
    assert_eq!(rows.settled(3), Some(2));
    // Nothing before it, so forward.
    assert_eq!(rows.settled(0), Some(1));
    // Out of range is clamped first, which is the case this exists for.
    assert_eq!(rows.settled(99), Some(4));
}

#[test]
fn the_top_of_a_row_is_the_sum_of_what_is_above_it() {
    let rows = grouped_list();

    // 28 (heading) + 34 + 34 + 28 (the heading over it).
    assert_eq!(rows.top(4), 124.);
    assert_eq!(rows.top(0), 0.);
    assert_eq!(rows.top(1), list::GROUP_HEIGHT);
    // An index past the end sums the whole list rather than panicking.
    assert_eq!(rows.top(99), rows.top(5));
}

#[test]
fn arrowing_into_a_group_shows_the_heading_that_names_it() {
    let rows = grouped_list();

    // Arriving at the top of a group and not being told which group it is is
    // arriving nowhere, so the row at the head of one is brought in with its
    // heading and with nothing else.
    assert_eq!(rows.reveal(4), list::GROUP_HEIGHT);
    assert_eq!(rows.reveal(2), 0.);
    assert_eq!(rows.reveal(0), 0.);
}

#[test]
fn a_flat_list_is_the_grouped_one_with_no_headings() {
    let window = owner("crook/window");
    let rows = Rows::new(
        Mode::Commands,
        flat(vec![
            (&window, entry("Split upwards"), Target::plain(an_id())),
            (&window, entry("Even out the split"), Target::plain(an_id())),
            (&window, entry("Split downwards"), Target::plain(an_id())),
            (&window, entry("Close the pane"), Target::plain(an_id())),
            (&window, entry("Find in output"), Target::plain(an_id())),
        ]),
    );

    assert!(headings(rows.rows()).is_empty());
    assert_eq!(
        titles(rows.rows()),
        [
            "Close the pane",
            "Even out the split",
            "Find in output",
            "Split downwards",
            "Split upwards",
        ]
    );

    // There is one list implementation and not two: with nothing to step over,
    // the offset is a multiple and the step is `rem_euclid`, which is what
    // shipped.
    for at in 0..rows.rows().len() {
        assert_eq!(rows.top(at), at as f32 * list::ROW_HEIGHT);
        assert_eq!(rows.reveal(at), 0.);
    }
    assert_eq!(rows.first_command(), Some(0));
    assert_eq!(rows.stepped(1, 1), Some(2));
    assert_eq!(rows.stepped(4, 1), Some(0));
    assert_eq!(rows.stepped(0, -1), Some(4));
}

#[test]
fn every_spelling_of_one_key_is_one_chord() {
    // Four table lines for one physical key on Linux, because a `Keystroke`'s
    // key is the character the platform reported with Shift already applied.
    let zoom = rules_for(DEFAULTS_OTHER, "crook/window/zoom-in");
    assert_eq!(zoom.len(), 4);
    assert_eq!(collapsed(&zoom).len(), 1);

    let focus = rules_for(DEFAULTS_OTHER, "crook/window/focus-next-pane");
    assert_eq!(focus.len(), 2);
    assert_eq!(collapsed(&focus).len(), 1);

    // Six on macOS, and two of them are genuinely different chords: the
    // Ctrl-Shift-Command pair is not a second spelling of Command-equals.
    let mac = rules_for(DEFAULTS_MAC, "crook/window/zoom-in");
    assert_eq!(mac.len(), 6);
    assert_eq!(texts(&mac), ["cmd+=", "ctrl+shift+cmd+="]);
}

#[test]
fn the_spelling_kept_is_the_one_the_keycap_is_printed_with() {
    // The shifted spelling first, and the survivor is still the bare one: the
    // cap prints what is on the key, not what the platform reported.
    let both = vec![
        rule("ctrl+shift+}", "crook/window/focus-next-pane"),
        rule("ctrl+shift+]", "crook/window/focus-next-pane"),
    ];
    assert_eq!(texts(&both), ["ctrl+shift+]"]);

    // Among the spellings that are on the keycap, the fewest modifiers wins.
    assert_eq!(
        texts(&rules_for(DEFAULTS_OTHER, "crook/window/zoom-in")),
        ["ctrl+="]
    );
    assert_eq!(
        texts(&rules_for(DEFAULTS_OTHER, "crook/window/zoom-out")),
        ["ctrl+-"]
    );
}

#[test]
fn a_shift_on_a_key_that_prints_one_character_is_a_chord_of_its_own() {
    // `pageup` has no second character, so that Shift is one a person holds
    // rather than one already spelled into what the platform reported — and
    // `shift+pageup` is not `pageup`.
    let rules = vec![
        rule("shift+pageup", "crook/window/page-up"),
        rule("pageup", "crook/window/page-up"),
    ];
    assert_eq!(texts(&rules), ["shift+pageup", "pageup"]);
}

#[test]
fn a_default_and_a_line_restating_it_are_one_key() {
    let mut theirs = rule("cmd+t", "crook/window/new-tab");
    theirs.source = Source::User;

    // Two rules, and which of them is in force is a question for the settings
    // page. This surface is asked which key to press, and there is one.
    let rules = vec![rule("cmd+t", "crook/window/new-tab"), theirs];
    assert_eq!(texts(&rules), ["cmd+t"]);
}

#[test]
fn a_key_that_any_rule_reaches_unconditionally_carries_no_clause() {
    let mut conditional = rule("ctrl+k", "crook/window/find");
    conditional.when = When::parse("panelOpen");
    assert_eq!(
        clauses(std::slice::from_ref(&conditional)),
        [Some("panelOpen".to_owned())]
    );

    // `resolve` keeps the last rule that *applies*, so the unconditional line
    // still fires while the clause does not hold, and a row saying "when
    // panelOpen" over a key that always works would be a lie.
    let rules = vec![conditional, rule("ctrl+k", "crook/window/find")];
    assert_eq!(clauses(&rules), [None::<String>]);
}

#[test]
fn a_group_of_one_or_two_commands_is_folded_into_the_last_one() {
    let host = host();
    let big = owner("eugen/big");
    let small = owner("eugen/small");
    let sizes: HashMap<&PluginId, usize> = HashMap::from([(&big, 3), (&small, 2)]);

    let rows = grouped(
        &host,
        &sizes,
        vec![
            (&big, entry("Alpha"), Target::plain(an_id())),
            (&small, entry("Beta"), Target::plain(an_id())),
            (&big, entry("Gamma"), Target::plain(an_id())),
            (&big, entry("Delta"), Target::plain(an_id())),
        ],
    );

    // Seven headings over seven rows is a fence, so the owner too small for
    // one of its own goes under the fold — which is last, and after every
    // group that did claim a heading.
    assert_eq!(
        lines(&rows),
        [
            "# eugen/big",
            "Alpha",
            "Delta",
            "Gamma",
            "# Elsewhere",
            "Beta"
        ]
    );
}

#[test]
fn a_group_puts_what_is_bound_first_and_then_the_alphabet() {
    let host = host();
    let window = owner("crook/window");
    let tabs = owner("crook/tabs");
    // `crook/tabs` has three commands although only one of them survived the
    // query: the fold is decided by the plugin's whole size, so a group does
    // not become `Elsewhere` as somebody types.
    let sizes: HashMap<&PluginId, usize> = HashMap::from([(&window, 4), (&tabs, 3)]);

    let rows = grouped(
        &host,
        &sizes,
        vec![
            (&window, entry("Zoom in"), Target::plain(an_id())),
            (&window, bound("apply", "ctrl+a"), Target::plain(an_id())),
            (&window, entry("apply twice"), Target::plain(an_id())),
            (&tabs, entry("Next tab"), Target::plain(an_id())),
        ],
    );

    // The rows that *are* the keyboard in one block at the top, and the
    // alphabet under them lowercased, so `Zoom` does not sort before `add`.
    assert_eq!(
        lines(&rows),
        [
            "# crook/window",
            "apply",
            "apply twice",
            "Zoom in",
            "# crook/tabs",
            "Next tab",
        ]
    );
}

#[test]
fn a_binding_on_something_that_is_not_a_command_is_still_in_the_list() {
    let mut host = host();
    host.register_command(name("crook/test/known"), "Known", |_, _| {});
    host.register_action(name("crook/test/plain"), |_, _| {});

    let rules = vec![
        rule("ctrl+1", "crook/test/known"),
        rule("ctrl+2", "crook/test/plain"),
    ];
    let rows = other_bindings(&host, &by(&rules), &[]);

    // The command already has a row under its own plugin's heading. The plain
    // action somebody bound has been on no surface at all until now.
    assert_eq!(lines(&rows), ["# Other bindings", "crook/test/plain"]);

    let Some(Row::Command(entry, _)) = rows.get(1) else {
        panic!("something answers to this name, so Enter has something to run");
    };
    assert_eq!(entry.note, None);
    assert_eq!(
        entry
            .chords
            .iter()
            .map(|key| key.text.as_str())
            .collect::<Vec<_>>(),
        ["ctrl+2"]
    );
}

#[test]
fn a_binding_naming_nothing_says_so_and_cannot_be_run() {
    let host = host();
    let rules = vec![rule("ctrl+3", "eugen/ghost/act")];
    let rows = Rows::new(Mode::Keys, other_bindings(&host, &by(&rules), &[]));

    let Some(Row::Fact(entry)) = rows.rows().get(1) else {
        panic!("nothing answers to this name, so there is nothing for Enter to mean");
    };
    // The one thing a keybindings file cannot tell the person who wrote it:
    // the line is fine and the plugin it names is not installed.
    assert_eq!(entry.title, "eugen/ghost/act");
    assert_eq!(entry.note, Some("nothing answers to this"));

    assert!(rows.command_at(1).is_none());
    assert_eq!(rows.first_command(), None);
}

#[test]
fn the_arrows_step_over_a_fact() {
    let rows = Rows::new(
        Mode::Keys,
        vec![command("Alpha"), fact("Beta"), command("Gamma")],
    );

    // A fact is navigationally a heading that happens to be 34px tall.
    assert_eq!(rows.stepped(0, 1), Some(2));
    assert_eq!(rows.stepped(2, 1), Some(0));
    assert_eq!(rows.settled(1), Some(0));
}

#[test]
fn the_keys_a_pane_eats_are_the_table_the_settings_page_prints() {
    assert_eq!(PANE_KEYS.len(), 9);

    let signals = parts(pane("Interrupt, suspend, end the input").keys());
    assert_eq!(signals.len(), 3);
    assert!(signals.iter().all(|part| !part.text.contains(' ')));

    // A part with a space in it is a phrase and not a key: a rounded outline
    // around `alt+right for one word` would claim it is something to press.
    let suggestion = parts(pane("Take the suggestion standing after the caret").keys());
    assert_eq!(suggestion.len(), 2);
    assert!(!suggestion[0].text.contains(' '));
    assert!(suggestion[1].text.contains(' '));

    // The whole table under one heading, and nothing in it runnable.
    let rows = in_a_pane(&[]);
    assert_eq!(headings(&rows), [IN_A_PANE]);
    assert_eq!(rows.len(), PANE_KEYS.len() + 1);
    assert!(rows[1..].iter().all(|row| matches!(row, Row::Fact(_))));

    // The keywords are the half of the haystack the row does not print.
    assert_eq!(
        titles(&in_a_pane(&["sigint"])),
        ["Interrupt, suspend, end the input"]
    );
}
