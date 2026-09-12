//! What a keybindings file means, and what the shipped table binds.

use std::path::PathBuf;

use crate::input_keys::{Pane, Route, route};

use super::*;

fn chord(text: &str) -> Keystroke {
    parse_chord(text).expect("this chord should parse")
}

fn keys(text: &str) -> Vec<Keystroke> {
    parse_keys(text).expect("these chords should parse")
}

fn command(name: &str) -> ActionName {
    ActionName::parse(name).expect("a literal that parses")
}

fn scratch(name: &str) -> PathBuf {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "crook-keybindings-{name}-{}-{serial}",
        std::process::id()
    ));
    let _ = fs::create_dir_all(&directory);
    directory
}

/// Writes `text` as a keybindings file and reads it back over the defaults.
///
/// Over the Control-Shift table by name: these tests are about what a file
/// does to the table under it, and they spell that table's chords — which a
/// Mac, left to choose its own, would not have shipped.
fn written(name: &str, text: &str) -> Keybindings {
    let path = scratch(name).join(KEYBINDINGS_FILE);
    fs::write(&path, text).expect("a scratch keybindings file");
    Keybindings::load_over(Platform::Other, &path)
}

/// What one chord means, with nothing else held down and no conditions set.
fn meaning(keybindings: &Keybindings, text: &str) -> Resolution {
    keybindings.resolve(&keys(text), &Context::new())
}

#[test]
fn a_chord_is_its_modifiers_and_then_its_key() {
    assert_eq!(
        chord("cmd+t"),
        Keystroke::new(
            "t",
            Modifiers {
                cmd: true,
                ..Modifiers::default()
            }
        )
    );

    let all = Modifiers {
        cmd: true,
        ctrl: true,
        alt: true,
        shift: true,
    };
    assert_eq!(chord("ctrl+shift+alt+cmd+k"), Keystroke::new("k", all));

    // Order does not matter, and neither does case.
    assert_eq!(chord("SHIFT+Ctrl+PageUp"), chord("ctrl+shift+pageup"));
}

#[test]
fn the_key_can_be_a_plus() {
    // `ctrl++` is a zoom chord, and a parser that split on every plus would
    // read it as a chord with no key.
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    assert_eq!(chord("ctrl++"), Keystroke::new("+", ctrl));
    assert_eq!(chord("ctrl+-"), Keystroke::new("-", ctrl));
}

#[test]
fn a_bare_key_is_a_chord_with_no_modifiers() {
    assert_eq!(chord("f5"), Keystroke::new("f5", Modifiers::default()));
    assert_eq!(
        chord("  enter  "),
        Keystroke::new("enter", Modifiers::default())
    );
}

#[test]
fn a_string_of_modifiers_names_no_key() {
    // There is nothing to press. A binding on it would never fire and would
    // silently look installed.
    assert_eq!(parse_chord("cmd+"), None);
    assert_eq!(parse_chord(""), None);
    assert_eq!(parse_keys("   "), None);
}

#[test]
fn the_modifier_a_platform_calls_something_else_is_the_same_modifier() {
    // `super` on Linux, `cmd` on macOS, `win` on Windows: one modifier with
    // four names, and a person moving a file across should not have to know
    // which one this build spells it with.
    for name in ["cmd", "super", "win", "meta"] {
        assert!(chord(&format!("{name}+t")).modifiers.cmd, "{name}");
    }
    for name in ["alt", "opt", "option"] {
        assert!(chord(&format!("{name}+t")).modifiers.alt, "{name}");
    }
}

#[test]
fn a_key_is_known_by_the_name_the_window_reports_and_by_the_one_vscode_uses() {
    for (written, reported) in [
        ("esc", "escape"),
        ("escape", "escape"),
        ("return", "enter"),
        ("pgdn", "pagedown"),
        ("arrowleft", "left"),
        ("left", "left"),
    ] {
        assert_eq!(chord(written).key, reported, "{written}");
    }
}

#[test]
fn a_sequence_is_the_chords_it_is_written_from() {
    assert_eq!(
        keys("ctrl+k ctrl+s"),
        vec![chord("ctrl+k"), chord("ctrl+s")]
    );
    assert_eq!(
        parse_keys("ctrl+k ctrl+"),
        None,
        "half of it is not a chord"
    );
}

#[test]
fn a_chord_is_written_the_way_it_is_read() {
    // The settings page prints chords, and printing them in a notation the
    // parser does not accept would be the page teaching a syntax that does not
    // work.
    for text in [
        "shift+cmd+u",
        "ctrl+alt+delete",
        "ctrl+shift+alt+cmd+k",
        "f5",
        "ctrl+-",
        "ctrl++",
    ] {
        let keystroke = chord(text);
        let written = format_chord(&keystroke);

        assert_eq!(
            parse_chord(&written),
            Some(keystroke),
            "{text:?} was written as {written:?}, which reads as something else"
        );
    }
    assert_eq!(format_keys(&keys("ctrl+k ctrl+s")), "ctrl+k ctrl+s");
}

#[test]
fn a_chord_is_written_in_one_order_however_it_was_typed() {
    // VSCode's order, since it is VSCode's notation.
    assert_eq!(
        format_chord(&chord("shift+alt+ctrl+cmd+k")),
        "ctrl+shift+alt+cmd+k"
    );
}

#[test]
fn the_shipped_table_binds_the_chords_each_platform_is_used_to() {
    let mac = Keybindings::for_platform(Platform::Mac);
    let other = Keybindings::for_platform(Platform::Other);

    for (keybindings, text, expected) in [
        (&mac, "cmd+t", "crook/window/new-tab"),
        (&mac, "cmd+alt+left", "crook/window/previous-tab"),
        (&mac, "cmd+,", "crook/window/open-settings"),
        (&other, "ctrl+shift+t", "crook/window/new-tab"),
        (&other, "ctrl+pagedown", "crook/window/next-tab"),
        (&other, "ctrl+,", "crook/window/open-settings"),
        // The one chord the two tables spell the same way, which is the whole
        // point of it: somebody who moves between the two machines presses it
        // without thinking about which one they are on.
        (&mac, "ctrl+tab", "crook/window/next-tab"),
        (&mac, "ctrl+shift+tab", "crook/window/previous-tab"),
        (&other, "ctrl+tab", "crook/window/next-tab"),
        (&other, "ctrl+shift+tab", "crook/window/previous-tab"),
    ] {
        assert_eq!(
            meaning(keybindings, text),
            Resolution::Command(command(expected)),
            "{text}"
        );
    }
}

#[test]
fn every_shipped_binding_names_a_command_of_the_window() {
    // The two tables and the `crook/window` command list are one decision. A
    // default naming a command nothing registers would be a chord that looks
    // bound on the settings page and does nothing when pressed.
    for platform in [Platform::Mac, Platform::Other] {
        for rule in Keybindings::for_platform(platform).effective() {
            assert!(
                crate::plugins::window::binding_for(&rule.command).is_some(),
                "{} is bound on {platform:?} and is not one of the window's commands",
                rule.command
            );
        }
    }
}

/// Every command a person would report as broken if pressing its key did
/// nothing.
///
/// This list used to be [`COMMANDS`](crate::plugins::window::COMMANDS) itself,
/// and it stopped being it the day the window grew commands nobody expects a
/// shipped chord for — colouring a tab, copying a block's branch, splitting
/// leftwards. A shipped chord is a key taken away from the shell in every
/// pane, forever, and most of these are better reached by name: the palette
/// runs one, and the Keyboard Shortcuts page binds it to whatever a person
/// likes. What has to hold is that the chords somebody arrives *expecting*
/// are there, which is what this names.
const MUST_HAVE_A_CHORD: &[&str] = &[
    "new-tab",
    "close-pane",
    "split-right",
    "split-down",
    "previous-tab",
    "next-tab",
    "move-tab-left",
    "move-tab-right",
    "select-tab-1",
    "select-last-tab",
    "focus-pane-left",
    "focus-pane-right",
    "focus-pane-up",
    "focus-pane-down",
    "focus-next-pane",
    "focus-previous-pane",
    "search-tabs",
    "find",
    "select-block-up",
    "select-block-down",
    "page-up",
    "page-down",
    "scroll-to-top",
    "scroll-to-bottom",
    "open-settings",
    "zoom-in",
    "zoom-out",
    "zoom-reset",
];

#[test]
fn the_commands_a_person_arrives_expecting_are_bound_on_both_platforms() {
    for platform in [Platform::Mac, Platform::Other] {
        let keybindings = Keybindings::for_platform(platform);
        for name in MUST_HAVE_A_CHORD {
            let action = command(&format!("crook/window/{name}"));
            assert!(
                !keybindings.chords_for(&action).is_empty(),
                "{action} has no chord on {platform:?}"
            );
        }
    }
}

#[test]
fn nothing_in_that_list_has_gone_away() {
    // The list above names commands by string, so a command that is renamed or
    // dropped would quietly stop being checked rather than failing.
    for name in MUST_HAVE_A_CHORD {
        assert!(
            crate::plugins::window::COMMANDS
                .iter()
                .any(|(command, _, _)| command == name),
            "crook/window/{name} is in MUST_HAVE_A_CHORD and is not a command"
        );
    }
}

#[test]
fn no_command_of_the_window_is_in_the_table_twice() {
    // A duplicate name is two rows on the Keyboard Shortcuts page for one
    // command, and a `binding_for` that answers with whichever came first —
    // so the second row's chord would run the first row's handler.
    //
    // That the table's names are *registered* is not asked here, because it
    // cannot be: `binding_for` is a search of this same table and would say
    // yes to anything in it. The registry is asked where there is one, in
    // `workspace::tests::from_the_keyboard::every_command_in_the_table_is_registered`.
    let names: Vec<String> = crate::plugins::window::COMMANDS
        .iter()
        .map(|(name, _, _)| format!("crook/window/{name}"))
        .collect();
    for (at, name) in names.iter().enumerate() {
        assert!(
            !names[..at].contains(name),
            "{name} is in the command table twice"
        );
    }
}

/// What Shift turns a punctuation key into on a US layout.
///
/// Only the keys a chord table would reasonably reach for; a layout that
/// disagrees is exactly why both spellings are bound rather than one.
const SHIFTED: &[(&str, &str)] = &[
    ("`", "~"),
    ("-", "_"),
    ("=", "+"),
    ("[", "{"),
    ("]", "}"),
    ("\\", "|"),
    (";", ":"),
    ("'", "\""),
    (",", "<"),
    (".", ">"),
    ("/", "?"),
];

#[test]
fn the_table_of_shifted_keys_is_the_one_the_test_checks_against() {
    // `SHIFTED` is written out above rather than read from the production
    // table, because an oracle that reads the thing it is checking is not one:
    // a pair deleted from `SHIFTED_TWINS` would silently stop
    // `a_shift_chord_on_a_punctuation_key_is_bound_under_both_spellings`
    // demanding both spellings of it, and the chord it stopped demanding is
    // the one that then ships dead.
    //
    // So the two are kept apart and made to agree here, in the one place a
    // pair added to either has to be added to the other.
    assert_eq!(SHIFTED, super::SHIFTED_TWINS);
}

#[test]
fn a_shift_chord_on_a_punctuation_key_is_bound_under_both_spellings() {
    // **The bug this exists to make impossible.** A `Keystroke`'s key is the
    // character the platform *reports*, and Shift has already been applied to
    // it: the window builds it from winit's `logical_key`, which is
    // shift-applied for everything but Ctrl. So a rule written `ctrl+shift+]`
    // is a rule that never fires on a layout where that key prints `}` — and
    // `resolve` compares the key exactly, so nothing anywhere would say so.
    // It looks bound on the Keyboard Shortcuts page and does nothing.
    //
    // The zoom family has always shipped `=` and `+` and both of `-` and `_`
    // for this reason, and its comment says so. This is that rule, applied to
    // every chord rather than remembered by whoever writes the next one.
    let twins: Vec<(&str, &str)> = SHIFTED
        .iter()
        .flat_map(|(bare, shifted)| [(*bare, *shifted), (*shifted, *bare)])
        .collect();

    for platform in [Platform::Mac, Platform::Other] {
        let keybindings = Keybindings::for_platform(platform);
        let rules: Vec<(String, String)> = keybindings
            .effective()
            .into_iter()
            .map(|rule| (rule.chord(), rule.command.to_string()))
            .collect();

        for (chord, command) in &rules {
            let Some((modifiers, key)) = chord.rsplit_once('+') else {
                continue;
            };
            if !modifiers.contains("shift") {
                continue;
            }
            let Some((_, twin)) = twins.iter().find(|(from, _)| from == &key) else {
                continue;
            };
            let wanted = format!("{modifiers}+{twin}");
            assert!(
                rules
                    .iter()
                    .any(|(chord, bound)| chord == &wanted && bound == command),
                "{platform:?} binds {chord} to {command} and not {wanted}, \
                 so the chord is dead on any layout that reports {twin:?}"
            );
        }
    }
}

#[test]
fn the_two_platforms_bind_the_same_commands() {
    // What `every_command_of_the_window_is_bound_on_both_platforms` was
    // really holding before it became `MUST_HAVE_A_CHORD`: not that every
    // command has a chord — most now deliberately do not — but that the two
    // tables agree about *which* ones do. A command bound on one platform and
    // not the other is a feature half the people who install this have.
    let bound = |platform| -> Vec<String> {
        let mut commands: Vec<String> = Keybindings::for_platform(platform)
            .effective()
            .into_iter()
            .filter(|rule| rule.source == Source::Default)
            .map(|rule| rule.command.to_string())
            .collect();
        commands.sort();
        commands.dedup();
        commands
    };

    assert_eq!(bound(Platform::Mac), bound(Platform::Other));
}

#[test]
fn no_shipped_binding_takes_a_chord_the_input_field_needs() {
    // The failure this test exists to make impossible: a binding consumed in
    // the window delegate never reaches `route`, so a chord in both tables is
    // a chord the field can never have — silently.
    let composing = Pane {
        composer: true,
        line_is_empty: false,
        grid_has_selection: false,
    };

    for platform in [Platform::Mac, Platform::Other] {
        for rule in Keybindings::for_platform(platform).effective() {
            let keystroke = rule.keys.first().expect("a rule has a chord");
            assert_eq!(
                route(keystroke, "", composing, platform),
                Route::Ignored,
                "{} is both a binding and the field's on {platform:?}",
                rule.chord()
            );
        }
    }
}

#[test]
fn the_tty_keeps_the_plain_control_chords_off_macos() {
    // Ctrl-D has to be able to end an input and Ctrl-C to interrupt, so off
    // macOS nothing of Crook's own can live on a bare ctrl-letter.
    let keybindings = Keybindings::for_platform(Platform::Other);
    for key in ["c", "d", "z", "w", "t", "b", "k"] {
        assert_eq!(
            meaning(&keybindings, &format!("ctrl+{key}")),
            Resolution::Nothing,
            "ctrl+{key} is Crook's, and the tty cannot have it"
        );
    }
}

#[test]
fn the_zoom_chords_are_bound_on_both_platforms() {
    // Both keys of each pair, with and without the Shift that reaches the one
    // printed on the same key: which one the platform reports depends on the
    // layout and on whether Shift was held.
    for (key, expected) in [
        ("=", "crook/window/zoom-in"),
        ("+", "crook/window/zoom-in"),
        ("-", "crook/window/zoom-out"),
        ("_", "crook/window/zoom-out"),
    ] {
        for shift in ["", "shift+"] {
            assert_eq!(
                meaning(
                    &Keybindings::for_platform(Platform::Mac),
                    &format!("{shift}cmd+{key}")
                ),
                Resolution::Command(command(expected)),
                "{shift}cmd+{key}"
            );
            assert_eq!(
                meaning(
                    &Keybindings::for_platform(Platform::Other),
                    &format!("ctrl+{shift}{key}")
                ),
                Resolution::Command(command(expected)),
                "ctrl+{shift}{key}"
            );
        }
    }
}

#[test]
fn a_bare_minus_is_not_a_zoom() {
    // The chord is the modifier. Typing a `-` into a command line must not
    // resize every pane in the window.
    for platform in [Platform::Mac, Platform::Other] {
        let keybindings = Keybindings::for_platform(platform);
        for key in ["-", "=", "0", "+"] {
            assert_eq!(meaning(&keybindings, key), Resolution::Nothing);
            assert_eq!(
                meaning(&keybindings, &format!("shift+{key}")),
                Resolution::Nothing,
                "shift+{key} types a character"
            );
        }
    }
}

#[test]
fn a_rule_in_the_file_wins_and_everything_else_is_unchanged() {
    let keybindings = written(
        "wins",
        r#"[{ "key": "ctrl+shift+t", "command": "crook/window/split-right" }]"#,
    );

    assert_eq!(
        keybindings.resolve(&keys("ctrl+shift+t"), &Context::new()),
        Resolution::Command(command("crook/window/split-right"))
    );
    assert_eq!(
        keybindings.resolve(&keys("ctrl+shift+e"), &Context::new()),
        Resolution::Command(command("crook/window/split-down")),
        "a chord the file says nothing about keeps Crook's own binding"
    );
}

#[test]
fn the_last_rule_that_matches_is_the_one_that_wins() {
    // VSCode's whole resolution rule, and the reason the layers are a list
    // rather than a map: two lines about one chord are not a conflict, they
    // are an override.
    let keybindings = written(
        "last",
        r#"[
            { "key": "ctrl+shift+j", "command": "crook/window/new-tab" },
            { "key": "ctrl+shift+j", "command": "crook/window/close-pane" }
        ]"#,
    );

    assert_eq!(
        keybindings.resolve(&keys("ctrl+shift+j"), &Context::new()),
        Resolution::Command(command("crook/window/close-pane"))
    );
}

#[test]
fn a_command_is_taken_off_a_chord_by_name() {
    // The only way to unbind, exactly as it is in VSCode: the chord goes back
    // to whatever would have had it — in a pane, the shell.
    let keybindings = written(
        "removal",
        r#"[{ "key": "ctrl+shift+t", "command": "-crook/window/new-tab" }]"#,
    );

    assert_eq!(
        keybindings.resolve(&keys("ctrl+shift+t"), &Context::new()),
        Resolution::Nothing
    );
    assert_eq!(
        keybindings.resolve(&keys("ctrl+shift+w"), &Context::new()),
        Resolution::Command(command("crook/window/close-pane")),
        "the chords around it are untouched"
    );
}

#[test]
fn a_removal_with_no_chord_takes_every_one_of_a_commands() {
    // The one thing VSCode has no spelling for. Moving a command off the
    // keyboard entirely otherwise means writing out a line per chord, and the
    // zoom alone ships with eight of them.
    let keybindings = written("removal-all", r#"[{ "command": "-crook/window/zoom-in" }]"#);

    assert!(
        keybindings
            .chords_for(&command("crook/window/zoom-in"))
            .is_empty()
    );
    assert!(
        !keybindings
            .chords_for(&command("crook/window/zoom-out"))
            .is_empty()
    );
}

#[test]
fn a_removal_reaches_backwards_only() {
    // Which is what lets one file take a default off a chord and then put
    // something else on it, in the order a person reads.
    let keybindings = written(
        "removal-order",
        r#"[
            { "key": "ctrl+shift+t", "command": "-crook/window/new-tab" },
            { "key": "ctrl+shift+t", "command": "crook/window/new-tab", "when": "settingsFocused" }
        ]"#,
    );

    assert_eq!(
        keybindings.resolve(&keys("ctrl+shift+t"), &Context::new()),
        Resolution::Nothing
    );
    assert_eq!(
        keybindings.resolve(
            &keys("ctrl+shift+t"),
            &Context::new().with("settingsFocused", true)
        ),
        Resolution::Command(command("crook/window/new-tab"))
    );
}

#[test]
fn a_condition_decides_whether_a_rule_is_in_force() {
    let keybindings = written(
        "when",
        r#"[{
            "key": "ctrl+shift+j",
            "command": "crook/window/new-tab",
            "when": "!searchFocused && paneFocused"
        }]"#,
    );

    let pressed = |context| keybindings.resolve(&keys("ctrl+shift+j"), &context);

    assert_eq!(
        pressed(Context::new().with("paneFocused", true)),
        Resolution::Command(command("crook/window/new-tab"))
    );
    assert_eq!(
        pressed(
            Context::new()
                .with("paneFocused", true)
                .with("searchFocused", true)
        ),
        Resolution::Nothing
    );
    assert_eq!(pressed(Context::new()), Resolution::Nothing);
}

#[test]
fn a_rule_that_does_not_apply_lets_the_one_under_it_through() {
    // The reason a condition is on the rule rather than on the chord: a
    // person's conditional line does not take the default away for the rest of
    // the time.
    let keybindings = written(
        "fallthrough",
        r#"[{
            "key": "ctrl+shift+t",
            "command": "crook/window/split-down",
            "when": "settingsFocused"
        }]"#,
    );

    assert_eq!(
        keybindings.resolve(
            &keys("ctrl+shift+t"),
            &Context::new().with("settingsFocused", true)
        ),
        Resolution::Command(command("crook/window/split-down"))
    );
    assert_eq!(
        keybindings.resolve(&keys("ctrl+shift+t"), &Context::new()),
        Resolution::Command(command("crook/window/new-tab"))
    );
}

#[test]
fn half_a_sequence_puts_the_window_in_chord_mode() {
    let keybindings = written(
        "chord",
        r#"[{ "key": "ctrl+k ctrl+s", "command": "crook/window/open-settings" }]"#,
    );

    assert_eq!(
        keybindings.resolve(&keys("ctrl+k"), &Context::new()),
        Resolution::Chord
    );
    assert_eq!(
        keybindings.resolve(&keys("ctrl+k ctrl+s"), &Context::new()),
        Resolution::Command(command("crook/window/open-settings"))
    );
    assert_eq!(
        keybindings.resolve(&keys("ctrl+k ctrl+j"), &Context::new()),
        Resolution::Nothing,
        "a sequence nothing completes ends the chord rather than doing something else"
    );
}

#[test]
fn a_rule_written_after_a_sequence_takes_its_first_chord_back() {
    // The same "last one wins" rule, applied to the case where one binding is
    // the beginning of another. VSCode settles it the same way.
    let keybindings = written(
        "chord-taken",
        r#"[
            { "key": "ctrl+k ctrl+s", "command": "crook/window/open-settings" },
            { "key": "ctrl+k", "command": "crook/window/new-tab" }
        ]"#,
    );

    assert_eq!(
        keybindings.resolve(&keys("ctrl+k"), &Context::new()),
        Resolution::Command(command("crook/window/new-tab"))
    );
}

#[test]
fn a_plugins_chord_is_underneath_everything_else() {
    // A plugin cannot take a chord from the window or from the person, and it
    // does not have to know what either of them has bound to find that out.
    let mut keybindings = written(
        "plugin",
        r#"[{ "key": "ctrl+shift+p", "command": "crook/window/new-tab" }]"#,
    );
    keybindings.set_plugin_rules(vec![
        rule_from(
            "ctrl+shift+p",
            command("crook/palette/open"),
            Source::Plugin,
        )
        .expect("a chord"),
        rule_from(
            "ctrl+shift+o",
            command("crook/palette/open"),
            Source::Plugin,
        )
        .expect("a chord"),
    ]);

    assert_eq!(
        keybindings.resolve(&keys("ctrl+shift+p"), &Context::new()),
        Resolution::Command(command("crook/window/new-tab"))
    );
    assert_eq!(
        keybindings.resolve(&keys("ctrl+shift+o"), &Context::new()),
        Resolution::Command(command("crook/palette/open")),
        "a chord nothing else wants is the plugin's"
    );
}

#[test]
fn a_chord_can_be_bound_to_a_plugins_command() {
    // The whole point of a name. Nothing in this build knows what
    // `crook/palette/open` is at the moment the file is read — the plugins
    // have not been built — and the rule is kept anyway.
    let keybindings = written(
        "plugin-command",
        r#"[{ "key": "shift+cmd+u", "command": "crook/palette/open" }]"#,
    );

    assert_eq!(
        keybindings.resolve(&keys("shift+cmd+u"), &Context::new()),
        Resolution::Command(command("crook/palette/open"))
    );
}

#[test]
fn a_line_nobody_can_read_costs_one_binding_and_nothing_else() {
    // A file that refused to load would be a file that stops Crook opening.
    let keybindings = written(
        "partial",
        r#"[
            { "key": "shift+cmd+j", "command": "crook/window/new-tab" },
            { "key": "cmd+", "command": "crook/window/close-pane" },
            { "key": "shift+cmd+k", "command": "fly_to_the_moon" },
            { "key": "shift+cmd+l", "command": 7 },
            { "key": "shift+cmd+m", "command": "crook/window/new-tab", "when": "a &&" },
            "not a binding at all",
            { "key": "shift+cmd+n", "command": "crook/window/zoom-in" }
        ]"#,
    );

    assert_eq!(
        keybindings.resolve(&keys("shift+cmd+j"), &Context::new()),
        Resolution::Command(command("crook/window/new-tab"))
    );
    assert_eq!(
        keybindings.resolve(&keys("shift+cmd+n"), &Context::new()),
        Resolution::Command(command("crook/window/zoom-in"))
    );
    for lost in ["shift+cmd+k", "shift+cmd+l", "shift+cmd+m"] {
        assert_eq!(
            keybindings.resolve(&keys(lost), &Context::new()),
            Resolution::Nothing,
            "{lost}"
        );
    }
}

#[test]
fn a_file_that_is_not_a_list_of_bindings_leaves_the_defaults_alone() {
    let directory = scratch("broken");
    let path = directory.join(KEYBINDINGS_FILE);

    for text in ["{ not json", r#"{"ctrl+shift+t": "new_tab"}"#, "[1, 2, 3]"] {
        fs::write(&path, text).expect("writable");
        let keybindings = Keybindings::load_over(Platform::Other, &path);
        assert!(keybindings.is_empty(), "{text:?}");
        assert_ne!(
            keybindings.resolve(&keys("ctrl+shift+t"), &Context::new()),
            Resolution::Nothing,
            "{text:?} cost the shipped bindings"
        );
    }

    assert!(Keybindings::load(directory.join("nothing.json")).is_empty());
}

#[test]
fn the_comments_vscode_writes_are_not_a_parse_error() {
    // The file VSCode creates for a person is entirely comments, and losing
    // every binding in a file because of the line above them would be the
    // format refusing what it taught.
    let keybindings = written(
        "comments",
        r#"
        // Place your key bindings in this file to override the defaults
        [
            /* the tab chord, moved */
            { "key": "shift+cmd+j", "command": "crook/window/new-tab" }, // like this
            { "key": "shift+cmd+k", "command": "crook/window/close-pane" }
        ]
        "#,
    );

    assert_eq!(
        keybindings.resolve(&keys("shift+cmd+j"), &Context::new()),
        Resolution::Command(command("crook/window/new-tab"))
    );
    assert_eq!(
        keybindings.resolve(&keys("shift+cmd+k"), &Context::new()),
        Resolution::Command(command("crook/window/close-pane"))
    );
}

#[test]
fn a_slash_inside_a_string_is_not_a_comment() {
    // Every command name has two of them.
    let keybindings = written(
        "slashes",
        r#"[{ "key": "shift+cmd+j", "command": "crook/window/new-tab" }]"#,
    );

    assert_eq!(
        keybindings.resolve(&keys("shift+cmd+j"), &Context::new()),
        Resolution::Command(command("crook/window/new-tab"))
    );
}

#[test]
fn the_page_is_told_which_chords_reach_a_command_and_where_they_came_from() {
    let keybindings = written(
        "sources",
        r#"[{ "key": "shift+cmd+j", "command": "crook/window/new-tab" }]"#,
    );
    let new_tab = command("crook/window/new-tab");

    assert!(
        keybindings
            .chords_for(&new_tab)
            .contains(&"shift+cmd+j".to_owned())
    );
    assert_eq!(keybindings.source_of(&new_tab), Some(Source::User));
    assert_eq!(
        keybindings.source_of(&command("crook/window/zoom-reset")),
        Some(Source::Default)
    );
    assert_eq!(keybindings.source_of(&command("crook/nothing/here")), None);
}

/// A keybindings file at `path`, loaded, with `text` already in it.
///
/// The shape every edit test needs: an edit is made against a file that is
/// there, and what it did is then read off the file rather than off the object
/// that made it.
fn editable(name: &str, text: &str) -> (PathBuf, Keybindings) {
    let path = scratch(name).join(KEYBINDINGS_FILE);
    if text.is_empty() {
        let _ = fs::remove_file(&path);
    } else {
        fs::write(&path, text).expect("a scratch keybindings file");
    }
    let keybindings = Keybindings::load_over(Platform::Other, &path);
    (path, keybindings)
}

/// Writes the edit, as the workspace's background task would, and reads the
/// file back.
fn saved(save: Option<PendingSave>) -> String {
    let save = save.expect("an edit that has a file to write");
    save.write_blocking().expect("the file should be writable");
    fs::read_to_string(save.path()).expect("the file should be readable")
}

#[test]
fn a_binding_made_on_the_page_beats_the_shipped_one_and_replaces_it() {
    // VSCode's own edit: the command is taken off every chord it had, and then
    // given the one that was recorded. Without the first half the window would
    // answer to both.
    let (path, mut keybindings) = editable("bind", "");
    let new_tab = command("crook/window/new-tab");
    let shipped = keybindings.chords_for(&new_tab);

    saved(keybindings.bind(&new_tab, &keys("ctrl+alt+n")));

    assert_eq!(
        meaning(&keybindings, "ctrl+alt+n"),
        Resolution::Command(new_tab.clone())
    );
    for chord in shipped {
        assert_eq!(meaning(&keybindings, &chord), Resolution::Nothing);
    }
    assert_eq!(keybindings.chords_for(&new_tab), vec!["ctrl+alt+n"]);
    assert_eq!(keybindings.source_of(&new_tab), Some(Source::User));
    assert!(keybindings.is_yours(&new_tab));
    // And the same file read again means the same thing, which is the only
    // sense in which an edit was made at all.
    assert_eq!(Keybindings::load_over(Platform::Other, &path), keybindings);
}

#[test]
fn changing_one_binding_four_times_leaves_two_lines_in_the_file() {
    // The person's earlier lines about the command go before the new pair is
    // written, or a chord somebody keeps fiddling with grows the file forever.
    let (_, mut keybindings) = editable("rebind", "");
    let new_tab = command("crook/window/new-tab");

    let mut text = String::new();
    for chord in ["ctrl+alt+n", "ctrl+alt+m", "ctrl+alt+o", "ctrl+alt+p"] {
        text = saved(keybindings.bind(&new_tab, &keys(chord)));
    }

    assert_eq!(text.matches("crook/window/new-tab").count(), 2);
    assert_eq!(keybindings.chords_for(&new_tab), vec!["ctrl+alt+p"]);
}

#[test]
fn a_command_unbound_on_the_page_gives_its_chord_back() {
    let (_, mut keybindings) = editable("unbind", "");
    let close = command("crook/window/close-pane");
    let shipped = keybindings.chords_for(&close);

    saved(keybindings.unbind(&close));

    assert!(keybindings.chords_for(&close).is_empty());
    for chord in shipped {
        assert_eq!(meaning(&keybindings, &chord), Resolution::Nothing);
    }
}

#[test]
fn resetting_puts_the_shipped_chords_back() {
    let (_, mut keybindings) = editable("reset", "");
    let new_tab = command("crook/window/new-tab");
    let shipped = keybindings.chords_for(&new_tab);

    keybindings.bind(&new_tab, &keys("ctrl+alt+n"));
    let text = saved(keybindings.reset(&new_tab));

    assert_eq!(keybindings.chords_for(&new_tab), shipped);
    assert!(!keybindings.is_yours(&new_tab));
    assert!(!text.contains("crook/window/new-tab"));
}

#[test]
fn an_edit_leaves_the_rest_of_the_file_where_it_was() {
    // A person's file is a file a person wrote: their comment, their line
    // about another command, and their line about a plugin this build has
    // never heard of are all still there afterwards.
    let file = "\
// Mine.
[
    { \"key\": \"ctrl+alt+j\", \"command\": \"crook/window/next-tab\" },
    { \"key\": \"ctrl+alt+q\", \"command\": \"nobody/at/all\" }
]
";
    let (_, mut keybindings) = editable("untouched", file);

    let text = saved(keybindings.bind(&command("crook/window/new-tab"), &keys("ctrl+alt+n")));

    assert!(text.starts_with("// Mine.\n"));
    assert!(text.contains(r#"{ "key": "ctrl+alt+j", "command": "crook/window/next-tab" }"#));
    assert!(text.contains(r#"{ "key": "ctrl+alt+q", "command": "nobody/at/all" }"#));
    assert_eq!(
        meaning(&keybindings, "ctrl+alt+j"),
        Resolution::Command(command("crook/window/next-tab"))
    );
}

#[test]
fn a_binding_can_be_a_sequence_and_a_key_the_page_has_no_default_for() {
    // "Any key on any action", which is the point: a chord nothing ships with,
    // spelled over two presses, against a command that had another chord.
    let (_, mut keybindings) = editable("sequence", "");
    let settings = command("crook/window/open-settings");

    saved(keybindings.bind(&settings, &keys("ctrl+k ctrl+s")));

    assert_eq!(meaning(&keybindings, "ctrl+k"), Resolution::Chord);
    assert_eq!(
        meaning(&keybindings, "ctrl+k ctrl+s"),
        Resolution::Command(settings)
    );
}

#[test]
fn nothing_is_written_where_there_is_nowhere_to_write() {
    // A run with no configuration directory — a test, the headless snapshot —
    // reads no file and writes none, and the page draws its controls dead
    // rather than pretending an edit was kept.
    let mut keybindings = Keybindings::new();

    assert!(!keybindings.is_editable());
    assert_eq!(
        keybindings.bind(&command("crook/window/new-tab"), &keys("ctrl+alt+n")),
        None
    );
}

#[test]
fn a_file_that_is_not_a_list_is_left_alone_rather_than_replaced() {
    // Somebody's settings pasted into the wrong file. Refusing the edit costs
    // one binding; writing over it costs the file.
    let file = "{ \"key\": \"ctrl+t\" }\n";
    let (path, mut keybindings) = editable("not-a-list", file);

    assert_eq!(
        keybindings.bind(&command("crook/window/new-tab"), &keys("ctrl+alt+n")),
        None
    );
    assert_eq!(
        fs::read_to_string(&path).expect("the file should be readable"),
        file
    );
}

#[test]
fn a_chord_another_command_has_taken_is_not_printed_on_the_row_that_lost_it() {
    // The page's own honesty. Binding a chord somebody else already had is
    // allowed — the last rule wins, which is VSCode's whole model — and the
    // row that lost it must stop claiming it, or the page says two commands
    // answer to one chord.
    let keybindings = written(
        "shadowed",
        r#"[{ "key": "ctrl+shift+d", "command": "crook/window/new-tab" }]"#,
    );

    assert!(
        keybindings
            .chords_for(&command("crook/window/split-right"))
            .is_empty()
    );
    // The line only *adds* a chord, so the command still has the one it
    // shipped with beside the one it took.
    assert_eq!(
        keybindings.chords_for(&command("crook/window/new-tab")),
        vec!["ctrl+shift+t", "ctrl+shift+d"]
    );
    assert_eq!(
        meaning(&keybindings, "ctrl+shift+d"),
        Resolution::Command(command("crook/window/new-tab"))
    );
}

#[test]
fn a_conditional_rule_does_not_take_a_chord_off_the_row_that_owns_it() {
    // The other half: a rule that only holds sometimes takes the chord only
    // sometimes, and a row that went blank over a clause which does not hold
    // while somebody is reading the settings page would be the same lie the
    // other way round.
    let keybindings = written(
        "conditional",
        r#"[{ "key": "ctrl+shift+d", "command": "crook/window/new-tab", "when": "searchFocused" }]"#,
    );

    assert_eq!(
        keybindings.chords_for(&command("crook/window/split-right")),
        vec!["ctrl+shift+d"]
    );
}

#[test]
fn the_chords_a_command_answers_to_are_the_rules_that_still_reach_it() {
    // The pin on the refactor that lifted the shadow filter out of
    // `chords_for` and into `standing`: the filter is about the rule and not
    // about who is asking, so applying it once for every rule has to leave
    // every row of the settings page spelled exactly as it was.
    //
    // The oracle is the body `chords_for` used to have, written out, because
    // an oracle derived from `standing` would only say the two agree with each
    // other. Both are checked against it, so neither can drift.
    let was = |keybindings: &Keybindings, command: &ActionName| -> Vec<String> {
        let effective = keybindings.effective();
        let mut chords: Vec<String> = Vec::new();
        for (at, rule) in effective.iter().enumerate() {
            if &rule.command != command {
                continue;
            }
            let taken = effective[at + 1..].iter().any(|later| {
                later.keys == rule.keys && later.when.is_none() && later.command != rule.command
            });
            let chord = rule.chord();
            if !taken && !chords.contains(&chord) {
                chords.push(chord);
            }
        }
        chords
    };

    // Both shipped tables, and a file that takes a chord off one command, adds
    // a second chord to another and restates a third under a clause — because
    // a table with no user layer shadows nothing, and the shadow is the whole
    // of what moved.
    let yours = written(
        "standing",
        r#"[
            { "key": "ctrl+shift+d", "command": "crook/window/new-tab" },
            { "key": "ctrl+shift+t", "command": "crook/window/new-tab" },
            { "key": "ctrl+shift+f", "command": "crook/window/find", "when": "paneFocused" }
        ]"#,
    );

    for keybindings in [
        Keybindings::for_platform(Platform::Mac),
        Keybindings::for_platform(Platform::Other),
        yours,
    ] {
        let mut commands: Vec<&ActionName> = keybindings
            .effective()
            .into_iter()
            .map(|rule| &rule.command)
            .collect();
        commands.sort();
        commands.dedup();
        assert!(!commands.is_empty(), "a table with no rules proves nothing");

        for command in commands {
            let wanted = was(&keybindings, command);
            assert_eq!(keybindings.chords_for(command), wanted, "{command}");

            let mut over_standing: Vec<String> = Vec::new();
            for rule in keybindings.standing() {
                let chord = rule.chord();
                if &rule.command == command && !over_standing.contains(&chord) {
                    over_standing.push(chord);
                }
            }
            assert_eq!(over_standing, wanted, "{command}");
        }
    }
}

#[test]
fn a_clause_names_each_of_its_keys_once() {
    // `Vec::dedup` collapses neighbours and nothing else, so the row that has
    // no room for the clause itself cannot be built out of it: `a && b && a`
    // names three, and "it depends on a, b, a" describes a clause nobody
    // wrote.
    let clause = When::parse("a && b && a").expect("a clause that parses");

    assert_eq!(clause.names(), vec!["a", "b", "a"]);
    assert_eq!(clause.names_once(), vec!["a", "b"]);
}
