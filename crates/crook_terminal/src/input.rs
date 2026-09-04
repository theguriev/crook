//! Turning key presses into the bytes a shell expects.
//!
//! There is no single specification for this; what programs actually parse is
//! the DEC and xterm lineage that terminfo describes, so that is what this
//! module emits:
//!
//! * printable characters as UTF-8, with Alt sending ESC first and Ctrl folding
//!   the character to its C0 control code where one exists — and sending the
//!   character unchanged where none does, which is all a legacy encoding can
//!   say about `Ctrl+1`;
//! * Enter, Tab, Backspace and Escape, including Shift+Tab as `CSI Z` and
//!   Ctrl+Backspace as `BS` rather than `DEL`;
//! * the arrows and Home/End in both the normal (`CSI A`) and application
//!   (`SS3 A`) cursor encodings, and Insert, Delete, PageUp and PageDown as the
//!   `CSI n ~` family;
//! * F1 to F20, the first four as `SS3 P`..`SS3 S` and the rest as `CSI n ~`;
//! * the numeric keypad, as its character in the mode a shell sits in and as
//!   the `SS3` form `DECPAM` asks for once a full-screen program has set it;
//! * Shift, Alt, Ctrl and Super on all of the above, as the xterm modifier
//!   parameter — `1 + shift + 2*alt + 4*ctrl + 8*super` — which switches the
//!   arrows and function keys to their `CSI 1 ; m A` form.
//!
//! ## The kitty keyboard protocol
//!
//! A program that sends `CSI > flags u` is asking for keys to be reported
//! unambiguously, and [`KeyboardModes`] is what it asked for. The one that
//! matters is `disambiguate`: with it set, the keys whose legacy encoding
//! collides with something else get the `CSI number ; modifiers u` form
//! instead, so `ctrl-enter`, `ctrl-tab`, `shift-enter` and `ctrl-i` stop being
//! indistinguishable from `enter`, `tab`, `enter` and `tab`. That collision is
//! the entire reason the protocol exists.
//!
//! `report_all` (`CSI > 8 u`) extends it to every key, so a plain `a` is
//! reported as `CSI 97u` rather than typed, and `report_text` (`16`) appends
//! the codepoints the key produced, so a program can still know what was typed
//! without a layout table of its own.
//!
//! ## What is not covered
//!
//! * **Key release and repeat events**, and so the `report_events` flag: only
//!   presses produce bytes, because a release never reaches this crate. The
//!   flag is carried in [`KeyboardModes`] so a caller can see what was asked
//!   for, and honouring it means feeding releases in from the window.
//! * **Alternate keys** (`CSI > 4 u`): reporting the shifted and base-layout
//!   forms of a key needs the layout, which is the windowing layer's and does
//!   not reach here.
//! * **Mouse reporting**, which is not a keyboard concern and needs the modes
//!   and pixel geometry the renderer owns.
//! * **Dead keys and IME composition.** A composed character should arrive here
//!   as [`Key::Char`] once the platform has finished composing it.
//! * **Ctrl+Enter, Ctrl+Tab and the other combinations legacy encodings cannot
//!   express.** They are indistinguishable from the unmodified key, and that is
//!   what gets sent.

/// The modifier keys held down with a key press.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// Shift.
    pub shift: bool,
    /// Control.
    pub control: bool,
    /// Alt, or Option on macOS.
    pub alt: bool,
    /// Super: Command, Windows, or the Meta key.
    pub logo: bool,
}

impl Modifiers {
    /// Nothing held.
    pub const NONE: Self = Self {
        shift: false,
        control: false,
        alt: false,
        logo: false,
    };
    /// Shift alone.
    pub const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };
    /// Control alone.
    pub const CONTROL: Self = Self {
        control: true,
        ..Self::NONE
    };
    /// Alt alone.
    pub const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };

    /// The xterm modifier parameter, which is 1 when nothing is held and grows
    /// by one bit per modifier.
    fn parameter(self) -> u8 {
        1 + u8::from(self.shift)
            + 2 * u8::from(self.alt)
            + 4 * u8::from(self.control)
            + 8 * u8::from(self.logo)
    }
}

/// A key that produces bytes.
///
/// Deliberately not a full keyboard: it is the set of keys whose encoding is
/// agreed on across terminals. Anything a platform reports that is not here —
/// media keys, modifiers on their own, the keypad — sends nothing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A character the platform has already composed and case-folded.
    Char(char),
    /// Return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Insert.
    Insert,
    /// Delete, the forward one.
    Delete,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// A function key, `F1` through `F20`. Anything outside that sends nothing.
    Function(u8),
    /// A key of the numeric keypad, which sends different bytes depending on
    /// [`InputModes::application_keypad`].
    Keypad(KeypadKey),
}

/// A key of the numeric keypad.
///
/// Separate from [`Key::Char`] because the keypad's `5` and the number row's
/// `5` are different keys to a terminal: in application keypad mode — `DECPAM`,
/// which every full-screen editor sets — the keypad sends `SS3` sequences and
/// the number row goes on sending digits. A caller that cannot tell the two
/// apart should send [`Key::Char`] and will be right in the mode a shell is in.
///
/// Only the keys that have an `SS3` form are here. The keypad's navigation
/// behaviour with NumLock off is not the keypad at all: those presses *are*
/// Home, End and the arrows, and belong to those variants.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum KeypadKey {
    /// `0` through `9`. Anything outside that range sends nothing.
    Digit(u8),
    /// The decimal separator, whichever character the layout prints on it.
    Decimal,
    /// `+`.
    Add,
    /// `-`.
    Subtract,
    /// `*`.
    Multiply,
    /// `/`.
    Divide,
    /// `=`, which not every keypad has.
    Equal,
    /// The keypad's own Enter.
    Enter,
}

impl KeypadKey {
    /// The character this key types when the keypad is *not* in application
    /// mode, which is the mode a shell sits in.
    fn typed(self) -> Option<char> {
        let character = match self {
            Self::Digit(digit @ 0..=9) => char::from(b'0' + digit),
            Self::Digit(_) => return None,
            Self::Decimal => '.',
            Self::Add => '+',
            Self::Subtract => '-',
            Self::Multiply => '*',
            Self::Divide => '/',
            Self::Equal => '=',
            // Not a character: Enter is Enter, and sends the carriage return
            // every shell reads as "run this".
            Self::Enter => return None,
        };
        Some(character)
    }

    /// The final byte of this key's `SS3` form, the one `DECPAM` asks for.
    ///
    /// The letters are the VT100's, and terminfo still describes them: the
    /// digits run `p` to `y` in order, and the operators sit around them.
    fn application_byte(self) -> Option<u8> {
        let byte = match self {
            Self::Digit(digit @ 0..=9) => b'p' + digit,
            Self::Digit(_) => return None,
            Self::Decimal => b'n',
            Self::Add => b'k',
            Self::Subtract => b'm',
            Self::Multiply => b'j',
            Self::Divide => b'o',
            Self::Equal => b'X',
            Self::Enter => b'M',
        };
        Some(byte)
    }
}

/// The keyboard modes the child process has asked for.
///
/// A program that has entered application cursor mode — every full-screen
/// editor does — expects `SS3 A` from the up arrow instead of `CSI A`, and will
/// misread the wrong one as a literal `[`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct InputModes {
    /// `DECCKM`: the arrows and Home/End use the `SS3` form.
    pub application_cursor: bool,
    /// `DECPAM`: the numeric keypad sends its `SS3` application forms rather
    /// than the characters printed on it. See [`KeypadKey`].
    pub application_keypad: bool,
    /// What the kitty keyboard protocol has been asked for, if anything.
    pub keyboard: KeyboardModes,
}

/// The kitty keyboard protocol flags a program has turned on.
///
/// All false is the legacy encoding, which is what every terminal did before
/// this protocol and what a shell sits in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct KeyboardModes {
    /// `CSI > 1 u`: report the keys whose legacy encoding is ambiguous in the
    /// `CSI number ; modifiers u` form.
    ///
    /// The flag that carries the protocol. Everything else is an addition to
    /// it, and a program that sets any flag at all sets this one.
    pub disambiguate: bool,
    /// `CSI > 2 u`: report releases and repeats as well as presses.
    ///
    /// Carried so a caller can see it was asked for. Nothing acts on it: a key
    /// release never reaches this crate.
    pub report_events: bool,
    /// `CSI > 4 u`: report the shifted and base-layout forms of each key.
    ///
    /// Carried, not acted on: it needs the keyboard layout, which belongs to
    /// the windowing layer.
    pub report_alternates: bool,
    /// `CSI > 8 u`: report *every* key in the escape form, including the ones
    /// that would simply have typed a character.
    pub report_all: bool,
    /// `CSI > 16 u`: append the text the key produced, as codepoints.
    pub report_text: bool,
}

impl KeyboardModes {
    /// Nothing asked for: the legacy encodings, which every program
    /// understands.
    pub const NONE: Self = Self {
        disambiguate: false,
        report_events: false,
        report_alternates: false,
        report_all: false,
        report_text: false,
    };

    /// Whether the protocol is in force at all.
    pub const fn is_enabled(self) -> bool {
        self.disambiguate || self.report_all
    }
}

/// The final bytes of the `CSI n ~` keys.
const INSERT: u8 = 2;
const DELETE: u8 = 3;
const PAGE_UP: u8 = 5;
const PAGE_DOWN: u8 = 6;

/// The `CSI n ~` numbers of F5 through F20. F1 to F4 have their own form, and
/// the gaps — 16, 22, 27, 30, 35 — are unassigned in the xterm sequence.
const FUNCTION_NUMBERS: [u8; 16] = [
    15, 17, 18, 19, 20, 21, 23, 24, 25, 26, 28, 29, 31, 32, 33, 34,
];

/// The bytes a key press sends, or `None` when it sends nothing.
///
/// `modes` comes from the emulator; [`crate::Terminal::send_key`] reads it for
/// the caller, and this function is the way to encode a key without one.
pub fn encode(key: Key, modifiers: Modifiers, modes: InputModes) -> Option<Vec<u8>> {
    if modes.keyboard.is_enabled()
        && let Some(encoded) = kitty(key, modifiers, modes.keyboard)
    {
        return Some(encoded);
    }
    legacy(key, modifiers, modes)
}

/// The kitty form of a key, or `None` for one the protocol leaves to the
/// legacy encoding below.
///
/// The rule is narrower than it looks. Arrows, function keys and the `CSI n ~`
/// family already have unambiguous legacy encodings that carry a modifier
/// parameter, so the protocol keeps them exactly as they are; what it replaces
/// is the handful of keys whose legacy bytes collide — Escape, Enter, Tab and
/// Backspace, and every Ctrl combination that folds to a C0 code. With
/// `report_all` it replaces plain characters too.
fn kitty(key: Key, modifiers: Modifiers, keyboard: KeyboardModes) -> Option<Vec<u8>> {
    let (number, text) = match key {
        // The four keys the protocol exists for. Their numbers are the
        // codepoints of the control characters they used to send, which is how
        // a program that knows neither still recognises them.
        Key::Escape => (27, None),
        Key::Enter => (13, Some('\r')),
        Key::Tab => (9, Some('\t')),
        Key::Backspace => (127, None),

        // A character is only ever escaped when something makes it ambiguous:
        // Ctrl, which would otherwise fold it to a C0 code and lose which key
        // it was, or `report_all`, where the program asked for every key.
        Key::Char(character) if modifiers.control || keyboard.report_all => {
            // The protocol reports the *unshifted* key, so that `ctrl-A` and
            // `ctrl-a` are one key with a modifier rather than two keys.
            let lowered = character.to_lowercase().next().unwrap_or(character);
            (u32::from(lowered), Some(character))
        }

        // Everything else keeps its legacy encoding, which the protocol says
        // is already unambiguous.
        _ => return None,
    };

    let parameter = modifiers.parameter();
    let mut encoded = format!("\x1b[{number}");
    // A key with no modifier and no text to report is written as bare as it
    // can be: `CSI 27u` rather than `CSI 27;1u`. Both are legal and the short
    // form is what kitty itself sends.
    let wants_text = keyboard.report_text && text.is_some();
    if parameter > 1 || wants_text {
        encoded.push_str(&format!(";{parameter}"));
    }
    if let Some(text) = text.filter(|_| wants_text) {
        encoded.push_str(&format!(";{}", u32::from(text)));
    }
    encoded.push('u');
    Some(encoded.into_bytes())
}

/// The encoding every terminal used before the kitty protocol, and the one a
/// shell still sits in.
fn legacy(key: Key, modifiers: Modifiers, modes: InputModes) -> Option<Vec<u8>> {
    match key {
        Key::Char(character) => encode_char(character, modifiers),
        Key::Enter => Some(with_alt(b"\r", modifiers)),
        // `CSI Z` is back-tab, and it carries the shift itself.
        Key::Tab if modifiers.shift => Some(b"\x1b[Z".to_vec()),
        Key::Tab => Some(with_alt(b"\t", modifiers)),
        // The two are swapped by convention: Backspace sends DEL, and Ctrl
        // makes it the actual BS control code.
        Key::Backspace if modifiers.control => Some(with_alt(b"\x08", modifiers)),
        Key::Backspace => Some(with_alt(b"\x7f", modifiers)),
        Key::Escape => Some(with_alt(b"\x1b", modifiers)),
        Key::Up => Some(cursor_key(b'A', modifiers, modes)),
        Key::Down => Some(cursor_key(b'B', modifiers, modes)),
        Key::Right => Some(cursor_key(b'C', modifiers, modes)),
        Key::Left => Some(cursor_key(b'D', modifiers, modes)),
        Key::Home => Some(cursor_key(b'H', modifiers, modes)),
        Key::End => Some(cursor_key(b'F', modifiers, modes)),
        Key::Insert => Some(tilde_key(INSERT, modifiers)),
        Key::Delete => Some(tilde_key(DELETE, modifiers)),
        Key::PageUp => Some(tilde_key(PAGE_UP, modifiers)),
        Key::PageDown => Some(tilde_key(PAGE_DOWN, modifiers)),
        Key::Function(number) => function_key(number, modifiers),
        Key::Keypad(key) => keypad_key(key, modifiers, modes),
    }
}

/// A key of the numeric keypad.
///
/// In application keypad mode it sends its `SS3` form, which is what makes the
/// keypad usable as a set of distinct keys rather than a second number row.
/// Otherwise it types its character, exactly as the number row does, and its
/// Enter sends the same carriage return the main one does.
///
/// A modifier drops the application form. `SS3` has nowhere to put a parameter,
/// which is the same reason the arrows leave it — and unlike the arrows there
/// is no agreed `CSI 1 ; m` spelling to fall back to, so what a modified keypad
/// key sends is the character with the modifier applied to it, which is what
/// the person typing it meant.
fn keypad_key(key: KeypadKey, modifiers: Modifiers, modes: InputModes) -> Option<Vec<u8>> {
    let plain = !modifiers.shift && !modifiers.control && !modifiers.alt && !modifiers.logo;
    if modes.application_keypad && plain {
        return key.application_byte().map(|byte| vec![0x1b, b'O', byte]);
    }
    match key.typed() {
        Some(character) => encode_char(character, modifiers),
        None => Some(with_alt(b"\r", modifiers)),
    }
}

/// Prefixes with ESC when Alt is held, which is how every terminal has sent
/// Meta since it stopped setting the high bit.
fn with_alt(bytes: &[u8], modifiers: Modifiers) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(bytes.len() + 1);
    if modifiers.alt {
        encoded.push(0x1b);
    }
    encoded.extend_from_slice(bytes);
    encoded
}

/// A printable character, folded to a control code when Ctrl is held.
///
/// A Ctrl combination with no control code of its own sends the plain
/// character, which is what xterm and everything descended from it does: the
/// legacy encoding has no way to say "Ctrl" about a key that is not one of the
/// thirty-odd with a C0 code, so `Ctrl+1` and `1` are the same bytes. Dropping
/// the key instead would make it do nothing at all, which is the one behaviour
/// no terminal has.
fn encode_char(character: char, modifiers: Modifiers) -> Option<Vec<u8>> {
    let mut encoded = Vec::with_capacity(5);
    if modifiers.alt {
        encoded.push(0x1b);
    }
    match modifiers.control.then(|| control_code(character)) {
        Some(Some(code)) => encoded.push(code),
        _ => {
            let mut buffer = [0; 4];
            encoded.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        }
    }
    Some(encoded)
}

/// The C0 code a Ctrl combination produces, or `None` for a combination that
/// has none — Ctrl+1, Ctrl+. and every other key the C0 range never covered.
///
/// The digits are here because the punctuation they share a key with is
/// unreachable on many layouts: Ctrl+6 has always been the way to type RS.
fn control_code(character: char) -> Option<u8> {
    let code = match character.to_ascii_lowercase() {
        ' ' | '@' | '2' => 0,
        letter @ 'a'..='z' => letter as u8 - b'a' + 1,
        '[' | '3' => 27,
        '\\' | '4' => 28,
        ']' | '5' => 29,
        '^' | '6' => 30,
        '_' | '7' | '/' => 31,
        '?' | '8' => 127,
        _ => return None,
    };
    Some(code)
}

/// An arrow or Home/End key.
///
/// Once a modifier is involved xterm drops the application form and uses
/// `CSI 1 ; m X`, because `SS3` has nowhere to put a parameter.
fn cursor_key(final_byte: u8, modifiers: Modifiers, modes: InputModes) -> Vec<u8> {
    let parameter = modifiers.parameter();
    if parameter > 1 {
        format!("\x1b[1;{parameter}{}", final_byte as char).into_bytes()
    } else if modes.application_cursor {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

/// One of the `CSI n ~` keys.
fn tilde_key(number: u8, modifiers: Modifiers) -> Vec<u8> {
    let parameter = modifiers.parameter();
    if parameter > 1 {
        format!("\x1b[{number};{parameter}~").into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

/// A function key. F1 to F4 keep the `SS3` form they inherited from the VT100
/// keypad; everything above them is `CSI n ~`.
fn function_key(number: u8, modifiers: Modifiers) -> Option<Vec<u8>> {
    match number {
        1..=4 => {
            let final_byte = b'P' + (number - 1);
            let parameter = modifiers.parameter();
            let encoded = if parameter > 1 {
                format!("\x1b[1;{parameter}{}", final_byte as char)
            } else {
                format!("\x1bO{}", final_byte as char)
            };
            Some(encoded.into_bytes())
        }
        5..=20 => Some(tilde_key(
            FUNCTION_NUMBERS[usize::from(number) - 5],
            modifiers,
        )),
        _ => None,
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
