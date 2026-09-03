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
//! * Shift, Alt, Ctrl and Super on all of the above, as the xterm modifier
//!   parameter — `1 + shift + 2*alt + 4*ctrl + 8*super` — which switches the
//!   arrows and function keys to their `CSI 1 ; m A` form.
//!
//! ## What is not covered
//!
//! * **The kitty keyboard protocol.** Alacritty's `Term` will happily set the
//!   modes for it; nothing here reads them, so a program that asks for it gets
//!   legacy encodings, which every one of them still understands.
//! * **The numeric keypad.** [`InputModes::application_keypad`] is reported so
//!   a caller can see the mode, but no keypad key is modelled, so `DECPAM` has
//!   no effect on what is sent.
//! * **Key release and repeat events.** Only presses produce bytes.
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
    /// `DECPAM`: the numeric keypad sends its application forms. Reported for
    /// completeness; no keypad key is modelled here.
    pub application_keypad: bool,
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
