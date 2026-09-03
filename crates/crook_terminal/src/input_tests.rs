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
        application_keypad: false,
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
