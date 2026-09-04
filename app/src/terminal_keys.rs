//! What a keystroke the shell is getting encodes as.
//!
//! **The other half of [`crate::input_keys`], and only the other half.**
//! Whether the shell gets a keystroke at all is decided there, in one function,
//! against the pane it arrived at; by the time anything here runs, that has
//! been settled and the only question left is which bytes go down the pty.
//! Nothing in this module knows what is bound, which screen is up or what is in
//! the input field, and it must not learn: two modules answering the same
//! question is how they come to answer it differently.
//!
//! So this is a table. A named key becomes a [`Key`]; a typed character
//! becomes itself, carrying whatever the platform's layout made of it; the
//! modifiers come along so that the emulator can apply the terminal's own
//! encodings to them.

use crook_terminal::{Key, KeypadKey, Modifiers};
use crookui_core::event::Keystroke;

/// The highest function key [`Key::Function`] encodes.
const HIGHEST_FUNCTION_KEY: u8 = 20;

/// The key and modifiers to send to a shell for this keystroke, or `None` when
/// the keystroke is not the shell's to have.
///
/// `chars` is the text the platform says the key produces, which is what
/// carries the keyboard layout: the key named `"a"` with Shift held produces
/// `"A"`, and a composed dead-key sequence arrives already composed. It is
/// deliberately ignored once Control or Alt is held, because then the character
/// wanted is the *base* one — `ctrl-c` folds the letter `c` to `0x03`, and the
/// text a platform reports for it is anything from `"c"` to `"\u{3}"` to
/// nothing at all.
pub fn key_for(keystroke: &Keystroke, chars: &str) -> Option<(Key, Modifiers)> {
    // See the module docs: the platform command modifier never types.
    if keystroke.modifiers.cmd {
        return None;
    }

    let modifiers = Modifiers {
        shift: keystroke.modifiers.shift,
        control: keystroke.modifiers.ctrl,
        alt: keystroke.modifiers.alt,
        logo: false,
    };

    Some((
        named(&keystroke.key).or_else(|| typed(keystroke, chars))?,
        modifiers,
    ))
}

/// The keys that have a name rather than a character.
///
/// The names are the ones the windowing layer produces: winit's `NamedKey`
/// spelled in lowercase, with the arrows and the page keys shortened.
fn named(key: &str) -> Option<Key> {
    let named = match key {
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "escape" => Key::Escape,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        // A named key that nonetheless types: the platform reports the space
        // bar by name, and every shell expects a plain `0x20` from it.
        "space" => Key::Char(' '),
        _ => return keypad(key).or_else(|| function(key)),
    };
    Some(named)
}

/// A key of the numeric keypad, which the windowing layer names apart from the
/// number row precisely so this can tell them apart.
///
/// A press that is *navigating* — the keypad with NumLock off — never reaches
/// here: it arrives under the name of the key it is acting as, `end` or `up`,
/// and is matched above.
fn keypad(key: &str) -> Option<Key> {
    let rest = key.strip_prefix("numpad")?;
    let keypad = match rest {
        "decimal" => KeypadKey::Decimal,
        "add" => KeypadKey::Add,
        "subtract" => KeypadKey::Subtract,
        "multiply" => KeypadKey::Multiply,
        "divide" => KeypadKey::Divide,
        "equal" => KeypadKey::Equal,
        "enter" => KeypadKey::Enter,
        digit => KeypadKey::Digit(digit.parse::<u8>().ok().filter(|digit| *digit <= 9)?),
    };
    Some(Key::Keypad(keypad))
}

/// `f1` through `f20`. The letter `f` on its own falls through to [`typed`],
/// which is what makes it type an `f`.
fn function(key: &str) -> Option<Key> {
    let number = key.strip_prefix('f')?.parse::<u8>().ok()?;
    (1..=HIGHEST_FUNCTION_KEY)
        .contains(&number)
        .then_some(Key::Function(number))
}

/// The character a key press stands for.
fn typed(keystroke: &Keystroke, chars: &str) -> Option<Key> {
    // With Control or Alt held it is the base key that matters, because that is
    // what folds to a control code or takes an ESC prefix.
    if !keystroke.modifiers.ctrl && !keystroke.modifiers.alt {
        let mut typed = chars.chars();
        if let (Some(character), None) = (typed.next(), typed.next())
            && !character.is_control()
        {
            return Some(Key::Char(character));
        }
    }

    let mut name = keystroke.key.chars();
    match (name.next(), name.next()) {
        (Some(character), None) => Some(Key::Char(character)),
        // A key with a multi-character name and no text: a media key, a
        // modifier on its own, an IME key. None of them sends anything.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crookui_core::event::Modifiers as UiModifiers;

    use super::*;

    fn keystroke(key: &str, modifiers: UiModifiers) -> Keystroke {
        Keystroke::new(key, modifiers)
    }

    fn ctrl() -> UiModifiers {
        UiModifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    #[test]
    fn the_command_modifier_never_reaches_the_shell() {
        // `cmd-t` is handled before this is asked; `cmd-k` is not bound to
        // anything, and it must still not type a `k` into somebody's shell.
        let command = UiModifiers {
            cmd: true,
            ..Default::default()
        };
        assert_eq!(key_for(&keystroke("k", command), "k"), None);
        assert_eq!(key_for(&keystroke("t", command), "t"), None);
    }

    #[test]
    fn control_reaches_the_shell_with_the_base_character() {
        // The whole of ctrl-c. The platform may report the text as "c", as the
        // control code itself, or as nothing; the base key is what encodes.
        let expected = Some((Key::Char('c'), Modifiers::CONTROL));
        assert_eq!(key_for(&keystroke("c", ctrl()), "c"), expected);
        assert_eq!(key_for(&keystroke("c", ctrl()), "\u{3}"), expected);
        assert_eq!(key_for(&keystroke("c", ctrl()), ""), expected);
    }

    #[test]
    fn the_text_the_platform_produced_carries_the_layout() {
        let shift = UiModifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            key_for(&keystroke("a", shift), "A"),
            Some((Key::Char('A'), Modifiers::SHIFT)),
            "the shifted character is what was typed, and the key name is not it"
        );
        assert_eq!(
            key_for(&keystroke("e", UiModifiers::default()), "é"),
            Some((Key::Char('é'), Modifiers::NONE)),
            "a composed dead-key sequence arrives as its result"
        );
    }

    #[test]
    fn named_keys_become_the_keys_that_have_encodings() {
        for (name, key) in [
            ("enter", Key::Enter),
            ("tab", Key::Tab),
            ("backspace", Key::Backspace),
            ("escape", Key::Escape),
            ("up", Key::Up),
            ("pagedown", Key::PageDown),
            ("space", Key::Char(' ')),
            ("f1", Key::Function(1)),
            ("f20", Key::Function(20)),
        ] {
            assert_eq!(
                key_for(&keystroke(name, UiModifiers::default()), ""),
                Some((key, Modifiers::NONE)),
                "{name} did not reach the shell"
            );
        }
    }

    #[test]
    fn the_keypad_is_named_apart_from_the_number_row() {
        // The whole reason the windowing layer reads the physical key: `5` on
        // the keypad and `5` above the letters are different keys to a
        // terminal, and only one of them changes with `DECPAM`.
        for (name, key) in [
            ("numpad0", KeypadKey::Digit(0)),
            ("numpad9", KeypadKey::Digit(9)),
            ("numpaddecimal", KeypadKey::Decimal),
            ("numpadadd", KeypadKey::Add),
            ("numpadsubtract", KeypadKey::Subtract),
            ("numpadmultiply", KeypadKey::Multiply),
            ("numpaddivide", KeypadKey::Divide),
            ("numpadequal", KeypadKey::Equal),
            ("numpadenter", KeypadKey::Enter),
        ] {
            assert_eq!(
                key_for(&keystroke(name, UiModifiers::default()), ""),
                Some((Key::Keypad(key), Modifiers::NONE)),
                "{name} did not reach the shell as a keypad key"
            );
        }

        // And the number row is untouched by any of it.
        assert_eq!(
            key_for(&keystroke("5", UiModifiers::default()), "5"),
            Some((Key::Char('5'), Modifiers::NONE))
        );
    }

    #[test]
    fn a_key_with_no_encoding_sends_nothing() {
        for name in ["f21", "f0", "audiovolumeup", "shift", "contextmenu"] {
            assert_eq!(
                key_for(&keystroke(name, UiModifiers::default()), ""),
                None,
                "{name} sent bytes it has no encoding for"
            );
        }
    }
}
