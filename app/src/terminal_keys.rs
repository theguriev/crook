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
//!
//! # Option on a Mac
//!
//! The one entry that has to ask a question. Alt is Meta to a terminal
//! program, ESC in front of the key, and on a Mac Option is also the key a
//! layout types its third level with: on a German keyboard `@` is Option-L and
//! `{` is Option-8, and sent as Meta neither can be typed into vim or Claude
//! Code at all. Which of the two a press was is [`Alt`]'s to say, under the
//! person's [`OptionAsMeta`]. Off a Mac a layout types with AltGr, which never
//! arrives as Alt, and Alt is Meta and nothing else.

use crook_terminal::{Key, KeypadKey, Modifiers};
use crookui_core::event::{KeyOrigin, Keystroke};

use crate::input_keys::Platform;
use crate::settings::OptionAsMeta;

/// The highest function key [`Key::Function`] encodes.
const HIGHEST_FUNCTION_KEY: u8 = 20;

/// What Alt means to the program a key is sent to.
///
/// On a Mac a press of Option either typed the character the layout put on the
/// key or meant Meta, and which one is decided per press, by the rule each
/// [`OptionAsMeta`] names.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Alt {
    /// Meta and only Meta. Linux and Windows, where a layout's third level is
    /// AltGr's.
    Meta,
    /// Option on a Mac, read as this setting says.
    Option(OptionAsMeta),
}

impl Alt {
    /// What Alt is on `platform`, where only a Mac reads `setting`.
    pub fn on(platform: Platform, setting: OptionAsMeta) -> Self {
        match platform {
            Platform::Mac => Self::Option(setting),
            Platform::Other => Self::Meta,
        }
    }
}

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
///
/// Except where Option typed it, which only `origin` and `alt` can say. Then
/// the character is sent as typed and with no Alt at all: ESC `{` is Meta-{ to
/// every program, and `{` is what the person pressed Option-8 for.
pub fn key_for(
    keystroke: &Keystroke,
    chars: &str,
    origin: &KeyOrigin,
    alt: Alt,
) -> Option<(Key, Modifiers)> {
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

    if let Some(key) = named(&keystroke.key) {
        return Some((key, modifiers));
    }
    if let Some(character) = typed_with_option(keystroke, chars, origin, alt) {
        let typed = Modifiers {
            alt: false,
            ..modifiers
        };
        return Some((Key::Char(character), typed));
    }
    Some((typed(keystroke, chars, origin, alt)?, modifiers))
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

/// The character Option typed, when this press of it typed rather than meant
/// Meta.
///
/// Only Option without Control, which makes a chord of anything, and only a
/// key that typed one character a program can be handed as text. Never a
/// named key: Option-Space types a no-break space on every Mac layout, which
/// nobody has wanted in a command line, and the space bar is named.
fn typed_with_option(
    keystroke: &Keystroke,
    chars: &str,
    origin: &KeyOrigin,
    alt: Alt,
) -> Option<char> {
    let Alt::Option(setting) = alt else {
        return None;
    };
    let modifiers = keystroke.modifiers;
    if !modifiers.alt || modifiers.ctrl {
        return None;
    }
    let character = single(chars).filter(|character| !character.is_control())?;

    let meta = match setting {
        OptionAsMeta::Both => true,
        OptionAsMeta::Neither => false,
        // The named Option is Meta and the other one types. A press that did
        // not say which Option it was gets the rule a fresh install has
        // rather than a guess at a side.
        OptionAsMeta::Left if origin.left_alt || origin.right_alt => origin.left_alt,
        OptionAsMeta::Right if origin.left_alt || origin.right_alt => origin.right_alt,
        OptionAsMeta::Auto | OptionAsMeta::Left | OptionAsMeta::Right => {
            !puts_a_symbol_there(character, origin)
        }
    };
    (!meta).then_some(character)
}

/// Whether Option put an ASCII character on a key that types something else
/// without it, which is the whole of [`OptionAsMeta::Auto`].
///
/// That is `{` from German Option-8, `@` from Option-L and `#` from British
/// Option-3: the characters programs are written in, which those keyboards
/// type no other way. A US keyboard's Option layer has no ASCII in it, `ƒ`
/// and `∫` and `å`, so there Option stays Meta on every key, as it was before
/// this rule, and so it does on the letters of every Latin layout. An ASCII
/// character the key types anyway, in either case, is Option doing nothing
/// Shift does not, and Meta is what that leaves. So is a key with nothing
/// under it to compare with, which is what Option always was.
fn puts_a_symbol_there(character: char, origin: &KeyOrigin) -> bool {
    single(&origin.unmodified).is_some_and(|unmodified| {
        character.is_ascii_graphic() && !character.eq_ignore_ascii_case(&unmodified)
    })
}

/// The character a key press stands for.
fn typed(keystroke: &Keystroke, chars: &str, origin: &KeyOrigin, alt: Alt) -> Option<Key> {
    let modifiers = keystroke.modifiers;
    // With Control or Alt held it is the base key that matters, because that is
    // what folds to a control code or takes an ESC prefix.
    if !modifiers.ctrl
        && !modifiers.alt
        && let Some(character) = single(chars).filter(|character| !character.is_control())
    {
        return Some(Key::Char(character));
    }

    // Option as Meta is Meta on the key the layout prints, which the name is
    // not always. It is what Option made of the key when that was ASCII, `{`
    // for German Option-8, and otherwise the US key in the same place, which
    // puts Meta-Y where a German keyboard says Z. The layout's own character
    // only when it is ASCII, for the reason the name falls back at all: Meta on
    // a Cyrillic letter is no key any program binds, while Meta-T on the key
    // where a US keyboard has its T is one readline does.
    if matches!(alt, Alt::Option(_))
        && modifiers.alt
        && !modifiers.ctrl
        && let Some(character) = single(&origin.unmodified).filter(char::is_ascii_graphic)
    {
        return Some(Key::Char(character));
    }

    // A key with a multi-character name and no text: a media key, a modifier
    // on its own, an IME key. None of them sends anything.
    single(&keystroke.key).map(Key::Char)
}

/// The one character in `text`, when there is exactly one.
fn single(text: &str) -> Option<char> {
    let mut characters = text.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Some(character),
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

    fn option() -> UiModifiers {
        UiModifiers {
            alt: true,
            ..Default::default()
        }
    }

    fn option_shift() -> UiModifiers {
        UiModifiers {
            alt: true,
            shift: true,
            ..Default::default()
        }
    }

    /// A press of the key under `unmodified`, with the left Option or the
    /// right one, as a Mac reports it.
    fn under(unmodified: &str, left: bool) -> KeyOrigin {
        KeyOrigin {
            unmodified: unmodified.to_owned(),
            left_alt: left,
            right_alt: !left,
        }
    }

    /// Every way Alt is read, for the answers that must not depend on it.
    const EVERY_ALT: [Alt; 6] = [
        Alt::Meta,
        Alt::Option(OptionAsMeta::Auto),
        Alt::Option(OptionAsMeta::Left),
        Alt::Option(OptionAsMeta::Right),
        Alt::Option(OptionAsMeta::Both),
        Alt::Option(OptionAsMeta::Neither),
    ];

    /// What a keystroke is sent as on every platform under every setting,
    /// failing when any two of them disagree.
    fn key_for_everywhere(keystroke: &Keystroke, chars: &str) -> Option<(Key, Modifiers)> {
        let answers = EVERY_ALT.map(|alt| key_for(keystroke, chars, &KeyOrigin::default(), alt));
        for (alt, answer) in EVERY_ALT.iter().zip(&answers) {
            assert_eq!(
                answer, &answers[0],
                "{keystroke:?} with {chars:?} is sent differently under {alt:?}"
            );
        }
        answers[0]
    }

    /// The bytes a program is sent for a keystroke, outside every keyboard
    /// mode a program can turn on.
    fn bytes(keystroke: &Keystroke, chars: &str, origin: &KeyOrigin, alt: Alt) -> Option<Vec<u8>> {
        let (key, modifiers) = key_for(keystroke, chars, origin, alt)?;
        crook_terminal::input::encode(key, modifiers, Default::default())
    }

    #[test]
    fn alt_and_shift_on_a_letter_reach_the_shell_as_the_capital() {
        // What the window hands over for Alt+Shift+H: the key by its
        // lower-case name, and the capital as the text, which is set aside
        // once Alt is held.
        assert_eq!(
            bytes(
                &keystroke("h", option_shift()),
                "H",
                &KeyOrigin::default(),
                Alt::Meta
            ),
            Some(b"\x1bH".to_vec())
        );
        // And on a Mac, where the text is what Option made of the key and the
        // name is the letter under it.
        assert_eq!(
            bytes(
                &keystroke("h", option_shift()),
                "Ó",
                &under("h", true),
                Alt::Option(OptionAsMeta::Auto)
            ),
            Some(b"\x1bH".to_vec())
        );
    }

    #[test]
    fn the_command_modifier_never_reaches_the_shell() {
        // `cmd-t` is handled before this is asked; `cmd-k` is not bound to
        // anything, and it must still not type a `k` into somebody's shell.
        let command = UiModifiers {
            cmd: true,
            ..Default::default()
        };
        assert_eq!(key_for_everywhere(&keystroke("k", command), "k"), None);
        assert_eq!(key_for_everywhere(&keystroke("t", command), "t"), None);
    }

    #[test]
    fn control_reaches_the_shell_with_the_base_character() {
        // The whole of ctrl-c. The platform may report the text as "c", as the
        // control code itself, or as nothing; the base key is what encodes.
        let expected = Some((Key::Char('c'), Modifiers::CONTROL));
        assert_eq!(key_for_everywhere(&keystroke("c", ctrl()), "c"), expected);
        assert_eq!(
            key_for_everywhere(&keystroke("c", ctrl()), "\u{3}"),
            expected
        );
        assert_eq!(key_for_everywhere(&keystroke("c", ctrl()), ""), expected);
    }

    #[test]
    fn the_text_the_platform_produced_carries_the_layout() {
        let shift = UiModifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            key_for_everywhere(&keystroke("a", shift), "A"),
            Some((Key::Char('A'), Modifiers::SHIFT)),
            "the shifted character is what was typed, and the key name is not it"
        );
        assert_eq!(
            key_for_everywhere(&keystroke("e", UiModifiers::default()), "é"),
            Some((Key::Char('é'), Modifiers::NONE)),
            "a composed dead-key sequence arrives as its result"
        );
    }

    #[test]
    fn altgr_types_what_the_layout_puts_on_it_as_it_always_did() {
        // AltGr+Q on a German keyboard off a Mac: the window reports no Alt
        // at all, because AltGr is not Alt, and the `@` it typed as the text.
        // Nothing about Option may reach this.
        assert_eq!(
            key_for_everywhere(&keystroke("@", UiModifiers::default()), "@"),
            Some((Key::Char('@'), Modifiers::NONE))
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
                key_for_everywhere(&keystroke(name, UiModifiers::default()), ""),
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
                key_for_everywhere(&keystroke(name, UiModifiers::default()), ""),
                Some((Key::Keypad(key), Modifiers::NONE)),
                "{name} did not reach the shell as a keypad key"
            );
        }

        // And the number row is untouched by any of it.
        assert_eq!(
            key_for_everywhere(&keystroke("5", UiModifiers::default()), "5"),
            Some((Key::Char('5'), Modifiers::NONE))
        );
    }

    #[test]
    fn a_key_with_no_encoding_sends_nothing() {
        for name in ["f21", "f0", "audiovolumeup", "shift", "contextmenu"] {
            assert_eq!(
                key_for_everywhere(&keystroke(name, UiModifiers::default()), ""),
                None,
                "{name} sent bytes it has no encoding for"
            );
        }
    }

    #[test]
    fn option_on_a_german_mac_types_the_symbols_the_layout_puts_on_it() {
        // What the window hands over for each: the key named by what Option
        // made of it, that as the text, and the key's own character beside
        // them. Sent with ESC in front, every one of these was Meta-something
        // to Claude Code, and `@` could not be typed into it at all.
        let auto = Alt::Option(OptionAsMeta::Auto);
        for (name, modifiers, unmodified, typed) in [
            ("{", option(), "8", "{"),
            ("}", option(), "9", "}"),
            ("[", option(), "5", "["),
            ("]", option(), "6", "]"),
            ("|", option(), "7", "|"),
            ("@", option(), "l", "@"),
            // Shift is the layout's too: German Option-Shift-7 is the
            // backslash, and it is sent as typed.
            ("\\", option_shift(), "7", "\\"),
            // And a British keyboard's `#`, which is Option-3 there.
            ("#", option(), "3", "#"),
        ] {
            assert_eq!(
                bytes(
                    &keystroke(name, modifiers),
                    typed,
                    &under(unmodified, true),
                    auto
                ),
                Some(typed.as_bytes().to_vec()),
                "Option on the {unmodified} key did not type {typed}"
            );
        }
    }

    #[test]
    fn option_on_a_us_mac_is_still_meta() {
        // The US layout's Option layer is all outside ASCII, and the window
        // names those keys by the letter they sit under. Meta-F is readline's
        // word forward, and `ƒ` is a character nobody pressed Option-F for.
        let auto = Alt::Option(OptionAsMeta::Auto);
        for (name, modifiers, typed, sent) in [
            ("f", option(), "ƒ", &b"\x1bf"[..]),
            ("b", option(), "∫", b"\x1bb"),
            ("1", option(), "¡", b"\x1b1"),
            ("f", option_shift(), "Ï", b"\x1bF"),
        ] {
            assert_eq!(
                bytes(&keystroke(name, modifiers), typed, &under(name, true), auto),
                Some(sent.to_vec()),
                "Option-{name} typing {typed} was not Meta-{name}"
            );
        }
    }

    #[test]
    fn option_that_changed_nothing_about_a_key_is_meta() {
        // A layout whose Option leaves a key as it was, or only does what
        // Shift does: Option typed nothing of its own, so it is Meta.
        let auto = Alt::Option(OptionAsMeta::Auto);
        assert_eq!(
            bytes(&keystroke(";", option()), ";", &under(";", true), auto),
            Some(b"\x1b;".to_vec())
        );
        assert_eq!(
            bytes(
                &keystroke("a", option_shift()),
                "A",
                &under("a", true),
                auto
            ),
            Some(b"\x1bA".to_vec())
        );
    }

    #[test]
    fn option_as_meta_is_the_key_the_layout_prints_rather_than_what_option_made_of_it() {
        let both = Alt::Option(OptionAsMeta::Both);
        // German Option-8 as Meta is Meta-8: the key's own character, and not
        // the `{` the window names the press by.
        assert_eq!(
            bytes(&keystroke("{", option()), "{", &under("8", true), both),
            Some(b"\x1b8".to_vec())
        );
        // German Option-Z types `Ω`, outside ASCII, so the window names it by
        // the US key in the same place, which is Y. The key says Z on it.
        assert_eq!(
            bytes(&keystroke("y", option()), "Ω", &under("z", true), both),
            Some(b"\x1bz".to_vec())
        );
        // A Russian layout's own character is no Meta key any program binds,
        // so there it stays the US key in the same place.
        assert_eq!(
            bytes(&keystroke("t", option()), "†", &under("е", true), both),
            Some(b"\x1bt".to_vec())
        );
    }

    #[test]
    fn the_setting_decides_which_option_types_and_which_is_meta() {
        let brace = keystroke("{", option());
        let eff = keystroke("f", option());
        for (setting, left, brace_sent, eff_sent) in [
            (OptionAsMeta::Auto, true, "{", "\x1bf"),
            (OptionAsMeta::Auto, false, "{", "\x1bf"),
            (OptionAsMeta::Left, true, "\x1b8", "\x1bf"),
            (OptionAsMeta::Left, false, "{", "ƒ"),
            (OptionAsMeta::Right, true, "{", "ƒ"),
            (OptionAsMeta::Right, false, "\x1b8", "\x1bf"),
            (OptionAsMeta::Both, true, "\x1b8", "\x1bf"),
            (OptionAsMeta::Both, false, "\x1b8", "\x1bf"),
            (OptionAsMeta::Neither, true, "{", "ƒ"),
            (OptionAsMeta::Neither, false, "{", "ƒ"),
        ] {
            let side = if left { "left" } else { "right" };
            assert_eq!(
                bytes(&brace, "{", &under("8", left), Alt::Option(setting)),
                Some(brace_sent.as_bytes().to_vec()),
                "German Option-8 on the {side} under {setting:?}"
            );
            assert_eq!(
                bytes(&eff, "ƒ", &under("f", left), Alt::Option(setting)),
                Some(eff_sent.as_bytes().to_vec()),
                "US Option-F on the {side} under {setting:?}"
            );
        }
    }

    #[test]
    fn an_option_the_platform_did_not_name_is_read_by_the_default_rule() {
        // Left or Right with no word on which Option was held: neither side
        // can be told apart, so the press gets what a fresh install gives it.
        let nameless = |unmodified: &str| KeyOrigin {
            unmodified: unmodified.to_owned(),
            ..KeyOrigin::default()
        };
        for setting in [OptionAsMeta::Left, OptionAsMeta::Right] {
            assert_eq!(
                bytes(
                    &keystroke("{", option()),
                    "{",
                    &nameless("8"),
                    Alt::Option(setting)
                ),
                Some(b"{".to_vec()),
                "German Option-8 under {setting:?}"
            );
            assert_eq!(
                bytes(
                    &keystroke("f", option()),
                    "ƒ",
                    &nameless("f"),
                    Alt::Option(setting)
                ),
                Some(b"\x1bf".to_vec()),
                "US Option-F under {setting:?}"
            );
        }
    }

    #[test]
    fn option_with_control_or_on_a_named_key_never_types() {
        let neither = Alt::Option(OptionAsMeta::Neither);
        // Option-Space types a no-break space on every Mac layout, and a
        // terminal that sent it would put an invisible character into a
        // command line. The space bar is named, and stays Meta-Space.
        assert_eq!(
            bytes(
                &keystroke("space", option()),
                "\u{a0}",
                &under(" ", true),
                neither
            ),
            Some(b"\x1b ".to_vec())
        );
        // With Control as well it is a chord, whatever Option typed.
        let control_option = UiModifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        };
        assert_eq!(
            bytes(
                &keystroke("c", control_option),
                "ç",
                &under("c", true),
                neither
            ),
            Some(b"\x1b\x03".to_vec())
        );
    }

    #[test]
    fn off_a_mac_alt_is_meta_whatever_the_setting_says() {
        for setting in OptionAsMeta::ALL {
            assert_eq!(Alt::on(Platform::Other, setting), Alt::Meta);
            assert_eq!(Alt::on(Platform::Mac, setting), Alt::Option(setting));
        }
        // The same press a German Mac types `{` with is ESC `{` anywhere
        // else, as it was: an Alt there is never the layout's.
        assert_eq!(
            bytes(&keystroke("{", option()), "{", &under("8", true), Alt::Meta),
            Some(b"\x1b{".to_vec())
        );
    }
}
