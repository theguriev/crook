use super::*;

fn chord(text: &str) -> Keystroke {
    parse_chord(text).expect("this chord should parse")
}

fn scratch(name: &str) -> PathBuf {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "crook-keymap-{name}-{}-{serial}",
        std::process::id()
    ));
    let _ = fs::create_dir_all(&directory);
    directory
}

#[test]
fn test_a_chord_is_its_modifiers_and_then_its_key() {
    assert_eq!(
        chord("cmd-t"),
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
    assert_eq!(chord("cmd-ctrl-alt-shift-k"), Keystroke::new("k", all));

    // Order does not matter, and neither does case.
    assert_eq!(chord("SHIFT-Ctrl-PageUp"), chord("ctrl-shift-pageup"));
}

#[test]
fn test_the_key_can_be_a_dash() {
    // `ctrl--` is the zoom-out chord on every platform, and a parser that
    // split on every dash would read it as a chord with no key.
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    assert_eq!(chord("ctrl--"), Keystroke::new("-", ctrl));
    assert_eq!(
        chord("cmd-="),
        Keystroke::new(
            "=",
            Modifiers {
                cmd: true,
                ..Modifiers::default()
            }
        )
    );
}

#[test]
fn test_a_bare_key_is_a_chord_with_no_modifiers() {
    assert_eq!(chord("f5"), Keystroke::new("f5", Modifiers::default()));
    assert_eq!(
        chord("  enter  "),
        Keystroke::new("enter", Modifiers::default())
    );
}

#[test]
fn test_a_string_of_modifiers_names_no_key() {
    // There is nothing to press. A binding on it would never fire and would
    // silently look installed.
    assert_eq!(parse_chord("cmd-"), None);
    assert_eq!(parse_chord(""), None);
    assert_eq!(parse_chord("   "), None);
}

#[test]
fn test_the_modifier_a_platform_calls_something_else_is_the_same_modifier() {
    // `super` on Linux, `cmd` on macOS, `win` on Windows: one modifier with
    // three names, and a person writing a keymap should not have to know which
    // one this build spells it with.
    for name in ["cmd", "super", "win", "meta"] {
        assert!(chord(&format!("{name}-t")).modifiers.cmd, "{name}");
    }
    for name in ["alt", "opt", "option"] {
        assert!(chord(&format!("{name}-t")).modifiers.alt, "{name}");
    }
}

#[test]
fn test_every_action_the_page_lists_is_one_that_parses() {
    // The list and the parser are one decision. A name the settings page
    // offered and the parser refused would be a keymap somebody wrote from the
    // documentation and Crook then ignored.
    for name in ACTION_NAMES {
        assert!(parse_action(name).is_some(), "{name}");
    }
    assert_eq!(parse_action("fly_to_the_moon"), None);
}

#[test]
fn test_a_chord_in_the_file_wins_and_everything_else_is_unchanged() {
    let directory = scratch("wins");
    let path = directory.join("keymap.json");
    fs::write(&path, r#"{"cmd-k": "new_tab"}"#).expect("writable");

    let keymap = Keymap::load(&path);
    assert_eq!(
        keymap.binding(&chord("cmd-k")),
        Some(Some(Bound::Builtin(Binding::NewTab)))
    );
    assert_eq!(
        keymap.binding(&chord("cmd-t")),
        None,
        "a chord the file says nothing about keeps Crook's own binding"
    );
}

#[test]
fn test_a_chord_can_be_taken_away() {
    // The one thing omission cannot express: leaving a chord out keeps Crook's
    // binding, and this is how somebody gives it back to their shell.
    let directory = scratch("unbound");
    let path = directory.join("keymap.json");
    fs::write(&path, r#"{"ctrl-shift-t": "none"}"#).expect("writable");

    assert_eq!(
        Keymap::load(&path).binding(&chord("ctrl-shift-t")),
        Some(None)
    );
}

#[test]
fn test_a_line_nobody_can_read_costs_one_binding_and_nothing_else() {
    // A keymap that refused to load would be a keymap that stops Crook
    // opening.
    let directory = scratch("partial");
    let path = directory.join("keymap.json");
    fs::write(
        &path,
        r#"{
            "cmd-k": "new_tab",
            "cmd-": "close_pane",
            "cmd-j": "fly_to_the_moon",
            "cmd-l": 7,
            "cmd-m": "zoom_in"
        }"#,
    )
    .expect("writable");

    let keymap = Keymap::load(&path);
    assert_eq!(
        keymap.binding(&chord("cmd-k")),
        Some(Some(Bound::Builtin(Binding::NewTab)))
    );
    assert_eq!(
        keymap.binding(&chord("cmd-m")),
        Some(Some(Bound::Builtin(Binding::ZoomIn)))
    );
    assert_eq!(keymap.binding(&chord("cmd-j")), None);
    assert_eq!(keymap.binding(&chord("cmd-l")), None);
}

#[test]
fn test_a_file_that_is_not_a_keymap_at_all_is_an_empty_one() {
    let directory = scratch("broken");
    let path = directory.join("keymap.json");

    assert!(Keymap::load(directory.join("nothing.json")).is_empty());

    fs::write(&path, "{ not json").expect("writable");
    assert!(Keymap::load(&path).is_empty());

    fs::write(&path, "[1, 2, 3]").expect("writable");
    assert!(Keymap::load(&path).is_empty());
}

#[test]
fn a_chord_can_be_bound_to_a_plugins_action() {
    // The whole point of a name. Nothing in this build knows what
    // `crook/usage/refresh` is at the moment the file is read — the plugins
    // have not been built — and the binding is kept anyway, because whether
    // something answers to it is a question with a different answer at every
    // moment of the window's life.
    assert_eq!(
        parse_action("crook/usage/refresh"),
        Some(Some(Bound::Named(
            ActionName::parse("crook/usage/refresh").expect("a literal that parses")
        )))
    );
    assert_eq!(
        parse_action("eugen/ci-status/open"),
        Some(Some(Bound::Named(
            ActionName::parse("eugen/ci-status/open").expect("a literal that parses")
        )))
    );
}

#[test]
fn a_name_that_is_not_an_action_name_is_refused() {
    // A `/` says "this is a plugin's", so what is checked is the *shape*: two
    // parts is a plugin id and not an action, and an uppercase part is a name
    // no plugin can have. Each is a warned-about line and a chord that keeps
    // Crook's own meaning, rather than a binding that never fires.
    assert_eq!(parse_action("crook/usage"), None);
    assert_eq!(parse_action("crook/usage/refresh/now"), None);
    assert_eq!(parse_action("Crook/Usage/Refresh"), None);
    assert_eq!(parse_action("/usage/refresh"), None);
}

#[test]
fn a_plugins_action_survives_a_trip_through_the_file() {
    let directory = scratch("plugin-action");
    let path = directory.join("keymap.json");
    fs::write(&path, r#"{"cmd-shift-u": "crook/usage/refresh"}"#).expect("a scratch file");

    let keymap = Keymap::load(&path);

    assert_eq!(
        keymap.binding(&chord("cmd-shift-u")),
        Some(Some(Bound::Named(
            ActionName::parse("crook/usage/refresh").expect("a literal that parses")
        )))
    );
}

#[test]
fn a_chord_is_written_the_way_it_is_read() {
    // The settings page prints chords, and printing them in a notation the
    // parser does not accept would be the page teaching a syntax that does not
    // work.
    for text in [
        "cmd-shift-u",
        "ctrl-alt-delete",
        "cmd-ctrl-alt-shift-k",
        "f5",
        "ctrl--",
    ] {
        let keystroke = parse_chord(text).expect("a chord");
        let written = format_chord(&keystroke);

        assert_eq!(
            parse_chord(&written),
            Some(keystroke),
            "{text:?} was written as {written:?}, which reads as something else"
        );
    }
}

#[test]
fn a_chord_is_written_in_one_order_however_it_was_typed() {
    assert_eq!(
        format_chord(&parse_chord("shift-alt-ctrl-cmd-k").expect("a chord")),
        "cmd-ctrl-alt-shift-k"
    );
}
