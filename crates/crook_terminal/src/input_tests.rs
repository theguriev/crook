use super::*;

/// The encoding of a key with nothing held and no mode set.
fn plain(key: Key) -> Vec<u8> {
    encode(key, Modifiers::NONE, InputModes::default()).expect("this key should encode")
}

/// The encoding of a key with modifiers held and no mode set.
fn held(key: Key, modifiers: Modifiers) -> Vec<u8> {
    encode(key, modifiers, InputModes::default()).expect("this key should encode")
}

#[test]
fn test_printable_characters_are_sent_as_utf8() {
    assert_eq!(b"a".to_vec(), plain(Key::Char('a')));
    assert_eq!(b"Z".to_vec(), plain(Key::Char('Z')));
    assert_eq!("é".as_bytes().to_vec(), plain(Key::Char('é')));
    assert_eq!("🦀".as_bytes().to_vec(), plain(Key::Char('🦀')));
}

#[test]
fn test_the_editing_keys_send_their_control_codes() {
    assert_eq!(b"\r".to_vec(), plain(Key::Enter));
    assert_eq!(b"\t".to_vec(), plain(Key::Tab));
    assert_eq!(b"\x7f".to_vec(), plain(Key::Backspace));
    assert_eq!(b"\x1b".to_vec(), plain(Key::Escape));
}

#[test]
fn test_shift_tab_is_back_tab() {
    assert_eq!(b"\x1b[Z".to_vec(), held(Key::Tab, Modifiers::SHIFT));
}

#[test]
fn test_control_backspace_is_the_real_backspace_code() {
    assert_eq!(b"\x08".to_vec(), held(Key::Backspace, Modifiers::CONTROL));
    assert_eq!(b"\x1b\x7f".to_vec(), held(Key::Backspace, Modifiers::ALT));
}

#[test]
fn test_control_folds_characters_to_control_codes() {
    assert_eq!(b"\x03".to_vec(), held(Key::Char('c'), Modifiers::CONTROL));
    assert_eq!(b"\x03".to_vec(), held(Key::Char('C'), Modifiers::CONTROL));
    assert_eq!(b"\x01".to_vec(), held(Key::Char('a'), Modifiers::CONTROL));
    assert_eq!(b"\x1a".to_vec(), held(Key::Char('z'), Modifiers::CONTROL));
    assert_eq!(b"\0".to_vec(), held(Key::Char(' '), Modifiers::CONTROL));
    assert_eq!(b"\x1b".to_vec(), held(Key::Char('['), Modifiers::CONTROL));
    assert_eq!(b"\x1f".to_vec(), held(Key::Char('/'), Modifiers::CONTROL));
    assert_eq!(b"\x1e".to_vec(), held(Key::Char('6'), Modifiers::CONTROL));
}

#[test]
fn test_a_key_that_encodes_to_nothing_is_reported_as_such() {
    // Nothing a printable character can be combined with sends nothing — see
    // `test_control_over_a_key_with_no_c0_code_sends_the_character`. The keys
    // that do send nothing are the ones outside the encoded set entirely.
    assert_eq!(
        None,
        encode(Key::Function(0), Modifiers::NONE, InputModes::default())
    );
    assert_eq!(
        None,
        encode(Key::Function(21), Modifiers::CONTROL, InputModes::default())
    );
}

#[test]
fn test_alt_prefixes_with_escape() {
    assert_eq!(b"\x1bb".to_vec(), held(Key::Char('b'), Modifiers::ALT));
    assert_eq!(b"\x1b\r".to_vec(), held(Key::Enter, Modifiers::ALT));
    // Both at once: escape, then the control code.
    let both = Modifiers {
        alt: true,
        control: true,
        ..Modifiers::NONE
    };
    assert_eq!(b"\x1b\x02".to_vec(), held(Key::Char('b'), both));
}

#[test]
fn test_the_arrows_follow_the_cursor_mode() {
    let normal = InputModes::default();
    let application = InputModes {
        application_cursor: true,
        ..normal
    };

    assert_eq!(
        b"\x1b[A".to_vec(),
        encode(Key::Up, Modifiers::NONE, normal).unwrap()
    );
    assert_eq!(
        b"\x1b[B".to_vec(),
        encode(Key::Down, Modifiers::NONE, normal).unwrap()
    );
    assert_eq!(
        b"\x1b[C".to_vec(),
        encode(Key::Right, Modifiers::NONE, normal).unwrap()
    );
    assert_eq!(
        b"\x1b[D".to_vec(),
        encode(Key::Left, Modifiers::NONE, normal).unwrap()
    );

    assert_eq!(
        b"\x1bOA".to_vec(),
        encode(Key::Up, Modifiers::NONE, application).unwrap()
    );
    assert_eq!(
        b"\x1bOD".to_vec(),
        encode(Key::Left, Modifiers::NONE, application).unwrap()
    );
}

#[test]
fn test_home_and_end_follow_the_cursor_mode_too() {
    let normal = InputModes::default();
    let application = InputModes {
        application_cursor: true,
        ..normal
    };

    assert_eq!(
        b"\x1b[H".to_vec(),
        encode(Key::Home, Modifiers::NONE, normal).unwrap()
    );
    assert_eq!(
        b"\x1b[F".to_vec(),
        encode(Key::End, Modifiers::NONE, normal).unwrap()
    );
    assert_eq!(
        b"\x1bOH".to_vec(),
        encode(Key::Home, Modifiers::NONE, application).unwrap()
    );
    assert_eq!(
        b"\x1bOF".to_vec(),
        encode(Key::End, Modifiers::NONE, application).unwrap()
    );
}

#[test]
fn test_a_modified_arrow_drops_the_application_form() {
    let application = InputModes {
        application_cursor: true,
        ..InputModes::default()
    };

    // Control is modifier 5; the sequence has to become the CSI form to carry
    // it, whatever mode the child asked for.
    assert_eq!(
        b"\x1b[1;5C".to_vec(),
        encode(Key::Right, Modifiers::CONTROL, application).unwrap()
    );
    assert_eq!(b"\x1b[1;2A".to_vec(), held(Key::Up, Modifiers::SHIFT));
    assert_eq!(b"\x1b[1;3A".to_vec(), held(Key::Up, Modifiers::ALT));
    let all = Modifiers {
        shift: true,
        control: true,
        alt: true,
        logo: true,
    };
    assert_eq!(b"\x1b[1;16A".to_vec(), held(Key::Up, all));
}

#[test]
fn test_the_navigation_keys_use_the_tilde_form() {
    assert_eq!(b"\x1b[2~".to_vec(), plain(Key::Insert));
    assert_eq!(b"\x1b[3~".to_vec(), plain(Key::Delete));
    assert_eq!(b"\x1b[5~".to_vec(), plain(Key::PageUp));
    assert_eq!(b"\x1b[6~".to_vec(), plain(Key::PageDown));
    assert_eq!(b"\x1b[3;5~".to_vec(), held(Key::Delete, Modifiers::CONTROL));
}

#[test]
fn test_function_keys() {
    assert_eq!(b"\x1bOP".to_vec(), plain(Key::Function(1)));
    assert_eq!(b"\x1bOS".to_vec(), plain(Key::Function(4)));
    assert_eq!(b"\x1b[15~".to_vec(), plain(Key::Function(5)));
    assert_eq!(b"\x1b[24~".to_vec(), plain(Key::Function(12)));
    assert_eq!(b"\x1b[34~".to_vec(), plain(Key::Function(20)));
    // Modified, F1 to F4 join the CSI family.
    assert_eq!(
        b"\x1b[1;5P".to_vec(),
        held(Key::Function(1), Modifiers::CONTROL)
    );
    assert_eq!(
        b"\x1b[15;2~".to_vec(),
        held(Key::Function(5), Modifiers::SHIFT)
    );
}

#[test]
fn test_a_function_key_that_does_not_exist_sends_nothing() {
    assert_eq!(
        None,
        encode(Key::Function(0), Modifiers::NONE, InputModes::default())
    );
    assert_eq!(
        None,
        encode(Key::Function(21), Modifiers::NONE, InputModes::default())
    );
}

#[test]
fn test_the_modifier_parameter_matches_xterm() {
    assert_eq!(1, Modifiers::NONE.parameter());
    assert_eq!(2, Modifiers::SHIFT.parameter());
    assert_eq!(3, Modifiers::ALT.parameter());
    assert_eq!(5, Modifiers::CONTROL.parameter());
    assert_eq!(
        9,
        Modifiers {
            logo: true,
            ..Modifiers::NONE
        }
        .parameter()
    );
    assert_eq!(
        6,
        Modifiers {
            shift: true,
            control: true,
            ..Modifiers::NONE
        }
        .parameter()
    );
}

#[test]
fn test_control_over_a_key_with_no_c0_code_sends_the_character() {
    // xterm, kitty, gnome-terminal and foot all deliver the bare character
    // here: the legacy encoding has no way to say "Ctrl" about a key outside
    // the thirty-odd the C0 range covers. Sending nothing at all — which is
    // what dropping the key would do — is the one answer no terminal gives.
    for character in ['1', '9', '0', '-', '=', ';', '\'', ',', '.', '`', 'é'] {
        let mut expected = [0; 4];
        assert_eq!(
            character.encode_utf8(&mut expected).as_bytes().to_vec(),
            held(Key::Char(character), Modifiers::CONTROL),
            "ctrl+{character} should type {character}"
        );
    }

    // Alt still prefixes the escape, and the characters that do have a code
    // still fold to it.
    assert_eq!(
        b"\x1b1".to_vec(),
        held(
            Key::Char('1'),
            Modifiers {
                control: true,
                alt: true,
                ..Modifiers::NONE
            }
        )
    );
    assert_eq!(vec![0x01], held(Key::Char('a'), Modifiers::CONTROL));
    assert_eq!(vec![0x1e], held(Key::Char('6'), Modifiers::CONTROL));
}

/// The kitty keyboard protocol: the keys whose legacy bytes collide, told
/// apart.
mod kitty_keyboard {
    use super::*;

    /// What a program asks for with `CSI > 1 u`, which is the flag that
    /// carries the protocol and the one every program that uses it sets.
    fn disambiguating() -> InputModes {
        InputModes {
            keyboard: KeyboardModes {
                disambiguate: true,
                ..KeyboardModes::NONE
            },
            ..InputModes::default()
        }
    }

    fn sent(key: Key, modifiers: Modifiers, modes: InputModes) -> String {
        let bytes = encode(key, modifiers, modes).expect("this key should encode");
        String::from_utf8(bytes).expect("every encoding here is ASCII")
    }

    #[test]
    fn test_the_collisions_the_protocol_exists_for_are_told_apart() {
        let modes = disambiguating();

        // Plain, these four keep the numbers of the control codes they used to
        // send, which is what makes them recognisable to a program that has
        // only just turned the protocol on.
        assert_eq!(sent(Key::Escape, Modifiers::NONE, modes), "\x1b[27u");
        assert_eq!(sent(Key::Enter, Modifiers::NONE, modes), "\x1b[13u");
        assert_eq!(sent(Key::Tab, Modifiers::NONE, modes), "\x1b[9u");
        assert_eq!(sent(Key::Backspace, Modifiers::NONE, modes), "\x1b[127u");

        // And the whole point: these four were indistinguishable from the
        // unmodified key in every legacy encoding. Control is modifier 5.
        assert_eq!(sent(Key::Enter, Modifiers::CONTROL, modes), "\x1b[13;5u");
        assert_eq!(sent(Key::Tab, Modifiers::CONTROL, modes), "\x1b[9;5u");
        assert_eq!(sent(Key::Enter, Modifiers::SHIFT, modes), "\x1b[13;2u");
        assert_eq!(
            sent(Key::Char('i'), Modifiers::CONTROL, modes),
            "\x1b[105;5u",
            "ctrl-i and Tab both fold to 0x09 in the legacy encoding"
        );
    }

    #[test]
    fn test_a_control_combination_reports_the_unshifted_key() {
        // `ctrl-A` and `ctrl-a` are one key with a modifier, not two keys. 97
        // is a lowercase `a`, and the shift is in the parameter.
        let modes = disambiguating();
        let shifted = Modifiers {
            shift: true,
            control: true,
            ..Modifiers::NONE
        };

        assert_eq!(sent(Key::Char('A'), shifted, modes), "\x1b[97;6u");
        assert_eq!(
            sent(Key::Char('a'), Modifiers::CONTROL, modes),
            "\x1b[97;5u"
        );
    }

    #[test]
    fn test_the_keys_with_unambiguous_legacy_forms_are_left_alone() {
        // Arrows, function keys and the `CSI n ~` family already carry a
        // modifier parameter, so the protocol keeps them exactly as they were.
        // Replacing them would break every program that reads terminfo.
        let modes = disambiguating();

        assert_eq!(sent(Key::Up, Modifiers::NONE, modes), "\x1b[A");
        assert_eq!(sent(Key::Up, Modifiers::CONTROL, modes), "\x1b[1;5A");
        assert_eq!(sent(Key::Function(5), Modifiers::NONE, modes), "\x1b[15~");
        assert_eq!(sent(Key::PageUp, Modifiers::NONE, modes), "\x1b[5~");

        // A plain character still types, which is what keeps a shell usable in
        // a program that turned the protocol on around it.
        assert_eq!(sent(Key::Char('a'), Modifiers::NONE, modes), "a");
    }

    #[test]
    fn test_report_all_escapes_even_the_keys_that_would_have_typed() {
        let modes = InputModes {
            keyboard: KeyboardModes {
                disambiguate: true,
                report_all: true,
                ..KeyboardModes::NONE
            },
            ..InputModes::default()
        };

        // Bare, because nothing was held: `CSI 97u` rather than `CSI 97;1u`,
        // which is what kitty itself sends and what its own parser expects.
        assert_eq!(sent(Key::Char('a'), Modifiers::NONE, modes), "\x1b[97u");
        // The key is the unshifted one and the shift is the parameter, so a
        // capital `Z` is key 122 with modifier 2.
        assert_eq!(sent(Key::Char('Z'), Modifiers::SHIFT, modes), "\x1b[122;2u");
    }

    #[test]
    fn test_report_text_appends_what_the_key_produced() {
        // A program that asked for every key still has to know what was typed,
        // and this is how it finds out without a layout table of its own.
        let modes = InputModes {
            keyboard: KeyboardModes {
                disambiguate: true,
                report_all: true,
                report_text: true,
                ..KeyboardModes::NONE
            },
            ..InputModes::default()
        };

        // 97 is the key, 1 is "nothing held", and the second 97 is the `a`
        // that was typed.
        assert_eq!(
            sent(Key::Char('a'), Modifiers::NONE, modes),
            "\x1b[97;1;97u"
        );
        // The shifted character is what was produced, and the unshifted one is
        // still the key.
        assert_eq!(
            sent(Key::Char('A'), Modifiers::SHIFT, modes),
            "\x1b[97;2;65u"
        );
        // Escape produces no text, so nothing is appended for it.
        assert_eq!(sent(Key::Escape, Modifiers::NONE, modes), "\x1b[27u");
    }

    #[test]
    fn test_nothing_changes_until_a_program_asks() {
        // The legacy encodings, unchanged, which is what a shell sits in.
        let modes = InputModes::default();

        assert_eq!(sent(Key::Escape, Modifiers::NONE, modes), "\x1b");
        assert_eq!(sent(Key::Enter, Modifiers::CONTROL, modes), "\r");
        assert_eq!(sent(Key::Char('c'), Modifiers::CONTROL, modes), "\u{3}");
    }
}

/// The keypad, whose whole point is that it sends something different from the
/// number row once a program has asked it to.
mod keypad {
    use super::*;

    /// The mode `DECPAM` puts the keypad in, which every full-screen editor
    /// sets on the way in and clears on the way out.
    fn application() -> InputModes {
        InputModes {
            application_keypad: true,
            ..InputModes::default()
        }
    }

    fn in_application(key: KeypadKey) -> Vec<u8> {
        encode(Key::Keypad(key), Modifiers::NONE, application()).expect("a keypad key encodes")
    }

    #[test]
    fn test_the_keypad_types_its_own_characters_in_the_mode_a_shell_is_in() {
        assert_eq!(b"5".to_vec(), plain(Key::Keypad(KeypadKey::Digit(5))));
        assert_eq!(b"0".to_vec(), plain(Key::Keypad(KeypadKey::Digit(0))));
        assert_eq!(b".".to_vec(), plain(Key::Keypad(KeypadKey::Decimal)));
        assert_eq!(b"+".to_vec(), plain(Key::Keypad(KeypadKey::Add)));
        assert_eq!(b"/".to_vec(), plain(Key::Keypad(KeypadKey::Divide)));

        // Enter is Enter. A shell that got anything else from it would not run
        // the line.
        assert_eq!(b"\r".to_vec(), plain(Key::Keypad(KeypadKey::Enter)));
    }

    #[test]
    fn test_application_mode_sends_the_ss3_forms_the_vt100_gave_them() {
        // The digits run `p` to `y` in order, which is what terminfo describes
        // and what every curses program parses.
        assert_eq!(b"\x1bOp".to_vec(), in_application(KeypadKey::Digit(0)));
        assert_eq!(b"\x1bOu".to_vec(), in_application(KeypadKey::Digit(5)));
        assert_eq!(b"\x1bOy".to_vec(), in_application(KeypadKey::Digit(9)));

        assert_eq!(b"\x1bOn".to_vec(), in_application(KeypadKey::Decimal));
        assert_eq!(b"\x1bOk".to_vec(), in_application(KeypadKey::Add));
        assert_eq!(b"\x1bOm".to_vec(), in_application(KeypadKey::Subtract));
        assert_eq!(b"\x1bOj".to_vec(), in_application(KeypadKey::Multiply));
        assert_eq!(b"\x1bOo".to_vec(), in_application(KeypadKey::Divide));
        assert_eq!(b"\x1bOX".to_vec(), in_application(KeypadKey::Equal));
        assert_eq!(b"\x1bOM".to_vec(), in_application(KeypadKey::Enter));
    }

    #[test]
    fn test_a_modifier_drops_the_application_form() {
        // `SS3` has nowhere to put a parameter, and unlike the arrows there is
        // no agreed `CSI 1 ; m` spelling for a keypad key. What is left is the
        // character with the modifier applied, which is what was meant.
        assert_eq!(
            vec![0x1b, b'5'],
            encode(
                Key::Keypad(KeypadKey::Digit(5)),
                Modifiers::ALT,
                application()
            )
            .expect("a modified keypad key still encodes")
        );
        assert_eq!(
            b"\r".to_vec(),
            encode(
                Key::Keypad(KeypadKey::Enter),
                Modifiers::SHIFT,
                application()
            )
            .expect("shift-enter on the keypad is still enter")
        );
    }

    #[test]
    fn test_a_digit_outside_the_keypad_sends_nothing() {
        // Unreachable from the windowing layer, which only ever names `0` to
        // `9`. Silence beats a byte nobody can predict.
        assert_eq!(
            encode(
                Key::Keypad(KeypadKey::Digit(10)),
                Modifiers::NONE,
                application()
            ),
            None
        );
    }
}
