//! Turning pointer gestures into the bytes a program that asked for the mouse
//! expects.
//!
//! A terminal does not report the mouse until a program asks it to, and the
//! asking is a set of private modes rather than one switch. [`MouseModes`] is
//! what the child has turned on; [`encode`] is the answer, and it returns
//! `None` for every gesture the modes in force do not cover — which is most of
//! them, most of the time.
//!
//! ## The three things a program can ask for
//!
//! * **`?1000h`, click reporting.** Presses and releases, nothing else. This is
//!   what a program that only wants to know where it was clicked turns on.
//! * **`?1002h`, drag reporting.** The above, plus motion *while a button is
//!   down*. A pane divider you can drag, a selection inside `tmux`.
//! * **`?1003h`, motion reporting.** Every pointer move, button or no button.
//!   Hover highlighting in a full-screen program.
//!
//! They are exclusive — alacritty's `Term` clears the other two when one is set
//! — but this module does not rely on that: a gesture is reported when *any*
//! mode in force covers it, which is the same answer and does not depend on
//! the emulator's bookkeeping.
//!
//! ## The two ways to write it down
//!
//! The original encoding — X10, still what a program gets when it asks for
//! nothing else — spends one byte per coordinate, biased by 32. That caps a
//! column at 223 and has no way at all to say which button was released, so
//! every release is the same byte.
//!
//! `?1006h` replaces it with the SGR form: decimal parameters, no cap, and a
//! final `M` or `m` distinguishing a press from a release. Everything modern
//! asks for it, and a program that does gets exact coordinates in a window
//! wider than 223 columns and a release that names its button.
//!
//! `?1005h`, the UTF-8 form, is deliberately not implemented: it raises the cap
//! to 2015 and is ambiguous about which bytes are a coordinate. Programs that
//! set it also set SGR, and one that sets only it gets X10 — clamped, which is
//! what X10 has always done past its own limit.
//!
//! ## What is not here
//!
//! Focus reporting (`?1004h`) and pixel-precision reporting (`?1016h`). Neither
//! has a caller in Crook yet, and both are one variant and one branch away.

use crate::input::Modifiers;

/// A pointer button, as a terminal names one.
///
/// The wheel is a button here because that is what the protocol makes it: a
/// scroll is a press of button 64 or 65, and there is no release.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// The primary button.
    Left,
    /// The wheel pressed as a button.
    Middle,
    /// The secondary button.
    Right,
    /// The wheel turned away from the user.
    WheelUp,
    /// The wheel turned towards the user.
    WheelDown,
}

impl MouseButton {
    /// The protocol's number for this button, before modifiers and motion are
    /// folded in.
    ///
    /// 0, 1 and 2 are the three buttons; 64 and 65 are the wheel, which is
    /// bit 6 set and then the same low bits. Buttons 4 to 11 exist in the
    /// protocol and no gesture Crook produces reaches them.
    const fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
        }
    }

    /// Whether this "button" is a wheel notch, which is reported as a press
    /// with no release ever following it.
    const fn is_wheel(self) -> bool {
        matches!(self, Self::WheelUp | Self::WheelDown)
    }
}

/// What the pointer just did.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MouseEventKind {
    /// A button went down, or the wheel turned one notch.
    Press,
    /// A button came up.
    Release,
    /// The pointer moved.
    Motion,
}

/// Which mouse reports the child has asked for.
///
/// All false is the state every terminal starts in and the state a shell sits
/// in: the pointer belongs to the person, and selecting text with it is the
/// terminal's own business.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MouseModes {
    /// `?1000`: presses and releases.
    pub click: bool,
    /// `?1002`: the above, plus motion while a button is held.
    pub drag: bool,
    /// `?1003`: every pointer move.
    pub motion: bool,
    /// `?1006`: the SGR encoding, which has no coordinate limit and names the
    /// button a release belongs to.
    pub sgr: bool,
    /// `?1007`: while the alternate screen is up, the wheel sends arrow keys.
    ///
    /// Not a report at all — it is what makes the wheel scroll a pager that
    /// never asked for the mouse — so it is read by the caller rather than by
    /// [`encode`]. See [`MouseModes::wants_alternate_scroll`].
    pub alternate_scroll: bool,
}

impl MouseModes {
    /// Nothing asked for: the pointer is the person's.
    pub const NONE: Self = Self {
        click: false,
        drag: false,
        motion: false,
        sgr: false,
        alternate_scroll: false,
    };

    /// Whether the child is reading the mouse at all.
    ///
    /// The one question a caller asks before deciding whether a press starts a
    /// selection or goes down the pty. `alternate_scroll` is deliberately not
    /// part of it: a pager with it set has *not* asked for the mouse, and a
    /// drag across its text must still select.
    pub const fn is_reporting(self) -> bool {
        self.click || self.drag || self.motion
    }

    /// Whether the wheel should be sent as arrow keys, given whether the
    /// alternate screen is up.
    ///
    /// This is what makes the wheel scroll `less`, `man` and `git log` — none
    /// of which reports the mouse, all of which read arrow keys. It applies
    /// only on the alternate screen: on the primary one the wheel belongs to
    /// the scrollback, which is real history the person can go back to, and
    /// turning it into arrow keys there would send `k` and `j` to whatever
    /// happened to be reading.
    pub const fn wants_alternate_scroll(self, alt_screen: bool) -> bool {
        self.alternate_scroll && alt_screen && !self.is_reporting()
    }

    /// Whether this gesture is one the modes in force ask to hear about.
    const fn covers(self, kind: MouseEventKind, button: Option<MouseButton>) -> bool {
        match kind {
            MouseEventKind::Press | MouseEventKind::Release => self.is_reporting(),
            // A drag is motion with a button down; `?1002` wants exactly that
            // and `?1003` wants motion either way.
            MouseEventKind::Motion => self.motion || (self.drag && button.is_some()),
        }
    }
}

/// The largest coordinate the X10 encoding can express.
///
/// One byte, biased by 32, and 255 is the last value a byte holds. A wider
/// window clamps here rather than wrapping into a coordinate that would land
/// the program's cursor somewhere it was never clicked.
const X10_MAX: usize = 223;

/// The bit that says "this report is a motion, not a press".
const MOTION_BIT: u8 = 32;

/// The X10 code for "a button came up", which cannot say which one.
const X10_RELEASE: u8 = 3;

/// The bytes a pointer gesture sends, or `None` when the child has not asked
/// to hear about it.
///
/// `row` and `column` are a cell of the viewport, zero-based; the protocol
/// counts from one, so the conversion happens here and exactly once.
///
/// Two plain numbers rather than a point type, and that is the honest shape:
/// this is the *screen* the program is drawing on, which is the one address
/// space a selection deliberately no longer uses — see
/// [`crate::selection`] — so there is no shared type left to name it with.
///
/// A wheel notch is only ever a [`MouseEventKind::Press`]: there is no release
/// of a wheel, and a caller that sent one would make every scroll two events to
/// a program expecting one. A release passed for a wheel button reports
/// nothing.
pub fn encode(
    kind: MouseEventKind,
    button: Option<MouseButton>,
    row: usize,
    column: usize,
    modifiers: Modifiers,
    modes: MouseModes,
) -> Option<Vec<u8>> {
    if !modes.covers(kind, button) {
        return None;
    }
    if button.is_some_and(MouseButton::is_wheel) && kind != MouseEventKind::Press {
        return None;
    }

    let code = button_code(kind, button, modifiers, modes.sgr);
    // The protocol is one-based, and a cell of the viewport is not.
    let column = column + 1;
    let row = row + 1;

    Some(if modes.sgr {
        // The final byte is what says press or release, which is the whole
        // reason this encoding can keep the button's own number in `code`.
        let final_byte = match kind {
            MouseEventKind::Release => 'm',
            _ => 'M',
        };
        format!("\x1b[<{code};{column};{row}{final_byte}").into_bytes()
    } else {
        // Biased by 32, one byte each, and clamped rather than wrapped: a
        // wrapped coordinate would land a program's cursor somewhere nobody
        // clicked, where a clamped one lands at the edge.
        let cell = |value: usize| 32 + value.min(X10_MAX) as u8;
        vec![0x1b, b'[', b'M', 32 + code, cell(column), cell(row)]
    })
}

/// The button field of a report: the button, plus the modifier bits, plus the
/// motion bit.
///
/// A release is where the two encodings genuinely differ rather than spelling
/// the same thing differently. X10 has one code for "something came up" and no
/// way to name the button, so a program reading it has to have tracked the
/// press itself. SGR keeps the button's own number and says "release" with its
/// final byte instead, which is why `sgr` is a parameter here.
fn button_code(
    kind: MouseEventKind,
    button: Option<MouseButton>,
    modifiers: Modifiers,
    sgr: bool,
) -> u8 {
    let mut code = match (kind, button) {
        (MouseEventKind::Release, Some(button)) if sgr => button.code(),
        (MouseEventKind::Release, _) => X10_RELEASE,
        (_, Some(button)) => button.code(),
        // Motion with nothing held. The protocol spells it as the release code
        // with the motion bit set, which is what `?1003` reports for a pointer
        // wandering across the screen.
        (_, None) => X10_RELEASE,
    };

    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }
    if kind == MouseEventKind::Motion {
        code += MOTION_BIT;
    }
    code
}

#[cfg(test)]
#[path = "mouse_tests.rs"]
mod tests;
