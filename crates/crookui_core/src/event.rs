//! Input events, in renderer-agnostic form.
//!
//! Nothing here mentions winit, AppKit or Win32: the platform crate translates
//! whatever the OS hands it into these types, and everything above the platform
//! line — elements, views, tests — speaks only this vocabulary. A synthetic
//! event is therefore indistinguishable from a real one, which is what makes
//! the element tree testable without a window.

use std::path::PathBuf;

use crate::geometry::{Point, Vector2F, ZIndex};
use crate::presenter::EventContext;

/// The keyboard modifiers held when an event fired.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Modifiers {
    /// Alt, or Option on macOS.
    pub alt: bool,
    /// Command on macOS, Super/Windows elsewhere.
    pub cmd: bool,
    /// Shift.
    pub shift: bool,
    /// Control.
    pub ctrl: bool,
}

impl Modifiers {
    /// Whether no modifier at all is held.
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// A key press, reduced to a name plus the modifiers held with it.
///
/// `key` is the logical key the OS reported after keyboard layout is applied,
/// lowercased: `"a"`, `"enter"`, `"f5"`, `"["`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct Keystroke {
    /// The modifiers held down.
    pub modifiers: Modifiers,
    /// The key's name.
    pub key: String,
}

impl Keystroke {
    /// Builds a keystroke from a key name and the modifiers held with it.
    pub fn new(key: impl Into<String>, modifiers: Modifiers) -> Self {
        Self {
            modifiers,
            key: key.into(),
        }
    }

    /// Whether this is `key` pressed with no modifiers at all.
    pub fn is_bare(&self, key: &str) -> bool {
        self.modifiers.is_empty() && self.key == key
    }
}

/// What a key press was made of beyond the name a chord matches and the text
/// it typed: the character under the key, where the key is, and which Alt key
/// was down.
///
/// Only a Mac asks, and it asks two things. The first is whether Option
/// *typed* the character it produced or meant Meta. On a German layout
/// Option-8 is the only way to type `{`; on a US one Option-F is an `ƒ` nobody
/// wanted and readline's Meta-F somebody did. The name and the text cannot
/// tell those apart, because `{` is the name German Option-8 is known by. The
/// second, when it meant Meta, is which key it was Meta on, and there the name
/// is no help either: French Option on the `(` key is named `{` too, and the
/// key's place is what says it is the 5. Not part of [`Keystroke`], which is
/// what a chord is matched against and must not stop matching because of which
/// Option a person reached for.
///
/// A synthetic event carries the default, which knows none of it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyOrigin {
    /// What the key types with no modifier held at all, under the layout in
    /// force: `8` for the key German Option turns into `{`. Empty for a key
    /// that types nothing, and where the platform does not say.
    pub unmodified: String,
    /// Where the key is, named by what a US keyboard prints there whatever
    /// the layout in force: `5` for the key a French layout prints `(` on.
    /// `None` for a key outside the letters, the digits and the punctuation,
    /// and where the platform does not say.
    pub position: Option<&'static str>,
    /// Whether the left Alt, Option on a Mac, is among the keys held.
    pub left_alt: bool,
    /// Whether the right one is. Neither, with Alt held, is a platform that
    /// did not say which.
    pub right_alt: bool,
}

/// Which mouse button an event came from.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum MouseButton {
    /// The primary button.
    Left,
    /// The wheel button. Closes a tab, by convention.
    Middle,
    /// The secondary button, which opens context menus.
    Right,
    /// The side button that navigates back.
    Back,
    /// The side button that navigates forward.
    Forward,
}

/// How far a scroll event asks to move, in its own units.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ScrollDelta {
    /// Discrete wheel clicks, in lines.
    Lines(Vector2F),
    /// A continuous gesture, already in logical pixels.
    Pixels(Vector2F),
}

impl ScrollDelta {
    /// The delta in logical pixels, converting line units at `line_height`.
    pub fn to_pixels(self, line_height: f32) -> Vector2F {
        match self {
            Self::Lines(lines) => lines.scale(line_height),
            Self::Pixels(pixels) => pixels,
        }
    }
}

/// Something the user (or the app, synthetically) did.
#[derive(Clone, Debug)]
pub enum Event {
    /// A key went down.
    KeyDown {
        /// The key and its modifiers.
        keystroke: Keystroke,
        /// The text this key press produces, if any.
        chars: String,
        /// The key under the text, and the Alt it was pressed with.
        origin: KeyOrigin,
    },

    /// A key came up.
    KeyUp {
        /// The key and its modifiers.
        keystroke: Keystroke,
    },

    /// A mouse button went down.
    MouseDown {
        /// Which button.
        button: MouseButton,
        /// Where, in logical window coordinates.
        position: Vector2F,
        /// Modifiers held at press time.
        modifiers: Modifiers,
        /// 1 for a single click, 2 for a double click, and so on.
        click_count: u32,
    },

    /// A mouse button came up.
    MouseUp {
        /// Which button.
        button: MouseButton,
        /// Where, in logical window coordinates.
        position: Vector2F,
        /// Modifiers held at release time.
        modifiers: Modifiers,
    },

    /// The mouse moved with a button held.
    MouseDragged {
        /// Which button is held.
        button: MouseButton,
        /// Where the pointer is now.
        position: Vector2F,
        /// Modifiers currently held.
        modifiers: Modifiers,
    },

    /// The mouse moved with no button held.
    MouseMoved {
        /// Where the pointer is now.
        position: Vector2F,
        /// Modifiers currently held.
        modifiers: Modifiers,
        /// Whether the app generated this itself rather than the OS.
        ///
        /// After a frame that changed layout, the platform replays the last
        /// real move as a synthetic one so hover state re-evaluates against
        /// the new geometry — otherwise closing a tab leaves the wrong tab
        /// highlighted. Elements that must not react twice check this flag.
        is_synthetic: bool,
    },

    /// The wheel turned or a scroll gesture moved.
    ScrollWheel {
        /// Where the pointer was.
        position: Vector2F,
        /// How far to scroll.
        delta: ScrollDelta,
        /// Modifiers currently held.
        modifiers: Modifiers,
    },

    /// A modifier key went down or came up on its own.
    ModifiersChanged {
        /// Where the pointer is, since hover styling often depends on both.
        position: Vector2F,
        /// The new modifier state.
        modifiers: Modifiers,
    },

    /// The input method said something.
    Ime(Ime),

    /// The desktop is set to light or to dark, or has just changed.
    ///
    /// Sent once when the window opens and again whenever the setting moves,
    /// so an application that follows the system needs no query of its own —
    /// which is what keeps every platform's way of asking behind the window
    /// layer.
    SystemTheme(SystemTheme),

    /// The window gained the desktop's keyboard focus (`true`) or lost it to
    /// another window (`false`).
    ///
    /// About the window, not about anything in it: an element's own focus is
    /// the application's business and never arrives this way. Sent on every
    /// change, and as the window opens only when it opens without the
    /// keyboard — a window is focused until told otherwise, which is what
    /// every window was before this was sent, so a desktop that never says
    /// leaves an application exactly as it was. A window that opens behind
    /// another hears `false` before its first frame, and `true` when the
    /// keyboard first arrives.
    WindowFocused(bool),

    /// Nothing of the window can be seen (`true`) — it is minimised, or
    /// every pixel of it is under other windows — or some of it can again
    /// (`false`).
    ///
    /// Sent on every change and not when the window opens, for the reason
    /// [`Self::WindowFocused`] is: a window is in view until told otherwise,
    /// so a desktop that never says leaves an application drawing as it
    /// always did. Not every desktop says all of it:
    ///
    /// - macOS says both.
    /// - Windows says neither; a minimised window is the one shrunk to
    ///   nothing, and a covered one goes unremarked.
    /// - X11 says a window is covered only when no compositor is running,
    ///   since a compositor draws every window off screen and the server
    ///   calls each of them in view; and says nothing of a minimised one,
    ///   which is asked about while the window does not have the keyboard.
    /// - Wayland says neither, and holds a hidden window's frames back
    ///   instead, which is the same saving made by the compositor.
    WindowOccluded(bool),

    /// Files were dragged from somewhere else — a file manager, a
    /// screenshot's thumbnail, a browser's downloads — and let go over the
    /// window.
    ///
    /// One event for the whole drop, however many files it carried: a
    /// platform that reports them a file at a time has them gathered before
    /// they cross the line, so an application never sees half a drop.
    ///
    /// It says nothing about *where* in the window they were let go, so it is
    /// not hit-tested and goes wherever the application decides. That is not
    /// an omission here. The platform layer reads its drops through a
    /// windowing library that reports no position with one, and while an
    /// operating system's drag is in progress the window is sent no pointer
    /// moves either, so the last position it was told of is wherever the
    /// pointer left it before the drag began. Wayland reports no drop at all.
    FilesDropped {
        /// The files, in the order the desktop listed them.
        paths: Vec<PathBuf>,
    },
}

/// Whether the desktop is set to light or to dark.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum SystemTheme {
    /// Dark, or a desktop that says nothing and is assumed to be.
    #[default]
    Dark,
    /// Light.
    Light,
}

impl SystemTheme {
    /// Whether this is the dark one.
    pub fn is_dark(self) -> bool {
        self == Self::Dark
    }
}

/// What an input method is doing to the text being composed.
///
/// A dead key, a Pinyin syllable, a Japanese conversion: on every platform the
/// path is the same. The IME takes the key presses, shows a string that is not
/// yet text — the *preedit* — and eventually either commits a result or gives
/// up. Between [`Ime::Enabled`] and [`Ime::Disabled`], key presses that belong
/// to the composition do not arrive as
/// [`KeyDown`](Event::KeyDown) at all, which is exactly why a field that
/// ignores these events cannot type Japanese: the keys never reach it and the
/// commit never does either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ime {
    /// A composition has begun. Anything the field had in flight is stale.
    Enabled,
    /// The composition so far, and where the IME wants its caret inside it, as
    /// a byte range.
    ///
    /// This is *not* text yet. It is drawn where the caret is, underlined, and
    /// replaced wholesale by the next preedit or by the commit. An empty string
    /// means the composition was cleared without committing.
    Preedit {
        /// The string being composed.
        text: String,
        /// Where the IME's own caret or selection sits within `text`, when it
        /// says. Byte offsets, and always on character boundaries.
        cursor: Option<(usize, usize)>,
    },
    /// The composition finished and this is the text it produced. It is
    /// inserted exactly as if it had been typed.
    Commit(String),
    /// The composition ended without producing anything. Any preedit on screen
    /// belongs to nothing and must go.
    Disabled,
}

impl Event {
    /// Where the pointer was, for events that carry a position.
    pub fn position(&self) -> Option<Vector2F> {
        match self {
            Self::MouseDown { position, .. }
            | Self::MouseUp { position, .. }
            | Self::MouseDragged { position, .. }
            | Self::MouseMoved { position, .. }
            | Self::ScrollWheel { position, .. }
            | Self::ModifiersChanged { position, .. } => Some(*position),
            Self::KeyDown { .. }
            | Self::KeyUp { .. }
            | Self::Ime(_)
            | Self::SystemTheme(_)
            | Self::WindowFocused(_)
            | Self::WindowOccluded(_)
            | Self::FilesDropped { .. } => None,
        }
    }

    /// Where a press landed, for press events only.
    ///
    /// An element uses this to notice presses that landed *outside* it, which
    /// is when it must abandon a click it had started tracking.
    pub fn mouse_down_position(&self) -> Option<Vector2F> {
        match self {
            Self::MouseDown { position, .. } => Some(*position),
            _ => None,
        }
    }

    /// The modifiers held when this event fired.
    pub fn modifiers(&self) -> Modifiers {
        match self {
            Self::KeyDown { keystroke, .. } | Self::KeyUp { keystroke } => keystroke.modifiers,
            Self::MouseDown { modifiers, .. }
            | Self::MouseUp { modifiers, .. }
            | Self::MouseDragged { modifiers, .. }
            | Self::MouseMoved { modifiers, .. }
            | Self::ScrollWheel { modifiers, .. }
            | Self::ModifiersChanged { modifiers, .. } => *modifiers,
            // An input method reports what it composed, never what was held to
            // compose it: the modifiers went into the composition and came out
            // the other side as text.
            Self::Ime(_)
            | Self::SystemTheme(_)
            | Self::WindowFocused(_)
            | Self::WindowOccluded(_)
            | Self::FilesDropped { .. } => Modifiers::default(),
        }
    }

    /// A copy of this move event marked synthetic, or `None` for other events.
    pub fn to_synthetic_mouse_move(&self) -> Option<Self> {
        match self {
            Self::MouseMoved {
                position,
                modifiers,
                is_synthetic: _,
            } => Some(Self::MouseMoved {
                position: *position,
                modifiers: *modifiers,
                is_synthetic: true,
            }),
            _ => None,
        }
    }
}

/// An [`Event`] on its way down an element tree.
///
/// The wrapper exists so an element can ask [`Self::at_z_index`] whether the
/// event is still meant for it, instead of every element reimplementing the
/// "is something on top of me" test.
#[derive(Debug)]
pub struct DispatchedEvent {
    event: Event,
}

impl DispatchedEvent {
    /// The event, filtered to `None` when a positional event landed on
    /// something painted above `z_index`.
    ///
    /// Keyboard and pointer-tracking events are never filtered: they are not
    /// about a location on screen even when they carry one.
    pub fn at_z_index(&self, z_index: ZIndex, ctx: &EventContext) -> Option<&Event> {
        match self.event {
            Event::MouseDown { position, .. }
            | Event::MouseUp { position, .. }
            | Event::MouseDragged { position, .. }
            | Event::ScrollWheel { position, .. } => {
                if ctx.is_covered(Point::from_vec2f(position, z_index)) {
                    None
                } else {
                    Some(&self.event)
                }
            }
            Event::KeyDown { .. }
            | Event::KeyUp { .. }
            | Event::Ime(_)
            | Event::SystemTheme(_)
            | Event::WindowFocused(_)
            | Event::WindowOccluded(_)
            | Event::FilesDropped { .. }
            | Event::MouseMoved { .. }
            | Event::ModifiersChanged { .. } => Some(&self.event),
        }
    }

    /// The event as it was dispatched, with no occlusion test.
    ///
    /// An element that uses this is responsible for its own hit testing; an
    /// element painted underneath another will otherwise react to clicks that
    /// visibly landed on its neighbour.
    pub fn raw_event(&self) -> &Event {
        &self.event
    }
}

impl From<Event> for DispatchedEvent {
    fn from(event: Event) -> Self {
        Self { event }
    }
}
