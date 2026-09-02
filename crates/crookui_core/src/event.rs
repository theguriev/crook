//! Input events, in renderer-agnostic form.
//!
//! Nothing here mentions winit, AppKit or Win32: the platform crate translates
//! whatever the OS hands it into these types, and everything above the platform
//! line — elements, views, tests — speaks only this vocabulary. A synthetic
//! event is therefore indistinguishable from a real one, which is what makes
//! the element tree testable without a window.

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
            Self::KeyDown { .. } | Self::KeyUp { .. } => None,
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
