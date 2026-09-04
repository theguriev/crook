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
fn written(name: &str, text: &str) -> Keybindings {
    let path = scratch(name).join(KEYBINDINGS_FILE);
    fs::write(&path, text).expect("a scratch keybindings file");
    Keybindings::load(&path)
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

#[test]
fn every_command_of_the_window_is_bound_on_both_platforms() {
    // The other direction: a command with no chord is a feature reachable only
    // through the palette, and each of these is one somebody expects a key for.
    for platform in [Platform::Mac, Platform::Other] {
        let keybindings = Keybindings::for_platform(platform);
        for (name, _, _) in crate::plugins::window::COMMANDS {
            let action = command(&format!("crook/window/{name}"));
            assert!(
                !keybindings.chords_for(&action).is_empty(),
                "{action} has no chord on {platform:?}"
            );
        }
    }
}

#[test]
fn no_shipped_binding_takes_a_chord_the_input_field_needs() {
    // The failure this test exists to make impossible: a binding consumed in
    // the window delegate never reaches `route`, so a chord in both tables is
    // a chord the field can never have — silently.
    let composing = Pane {
        alt_screen: false,
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
        let keybindings = Keybindings::load(&path);
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
