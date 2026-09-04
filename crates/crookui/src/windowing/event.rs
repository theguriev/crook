//! Translating winit's events into Crook's.
//!
//! This module is the only place in the crate that reads a `winit` event type,
//! and nothing it exports appears in a signature the app crate can see. That is
//! the whole point: above this line an event carries no evidence of which
//! window system produced it, so a synthetic event is indistinguishable from a
//! real one and the element tree is testable without a display.
//!
//! Winit reports positions in physical pixels; every [`Event`] leaving here is
//! in logical ones, because that is the space a [`Scene`](crookui_core::Scene)
//! is laid out in.

use std::time::{Duration, Instant};

use crookui_core::event::{
    Event, Ime, Keystroke, Modifiers, MouseButton, ScrollDelta, SystemTheme,
};
use crookui_core::geometry::{Vector2F, vec2f};
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// How long after a press a second one still counts as a double click.
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// How far the pointer may move between the clicks of a double click, in
/// logical pixels.
const MULTI_CLICK_SLOP: f32 = 4.;

/// The input state winit does not carry in its events.
///
/// Winit reports modifiers, cursor position and button transitions as separate
/// events, but a Crook [`Event`] carries all three at once — an element asking
/// "was cmd held when this was clicked?" should not have to remember. Click
/// counting lives here for the same reason: winit reports presses, not clicks.
#[derive(Debug, Default)]
pub(super) struct InputState {
    modifiers: Modifiers,
    cursor: Vector2F,
    buttons_down: Vec<MouseButton>,
    last_click: Option<Click>,
}

#[derive(Debug)]
struct Click {
    button: MouseButton,
    at: Instant,
    position: Vector2F,
    count: u32,
}

impl InputState {
    /// Converts one winit window event, or `None` when it is not input.
    ///
    /// `scale_factor` converts winit's physical coordinates to the logical ones
    /// the scene was laid out in, so it must be the window's *current* scale
    /// factor, read fresh each event.
    pub(super) fn convert(&mut self, event: &WindowEvent, scale_factor: f32) -> Option<Event> {
        match event {
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = to_modifiers(modifiers.state());
                Some(Event::ModifiersChanged {
                    position: self.cursor,
                    modifiers: self.modifiers,
                })
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = vec2f(position.x as f32, position.y as f32) / scale_factor;
                match self.buttons_down.last() {
                    Some(button) => Some(Event::MouseDragged {
                        button: *button,
                        position: self.cursor,
                        modifiers: self.modifiers,
                    }),
                    None => Some(Event::MouseMoved {
                        position: self.cursor,
                        modifiers: self.modifiers,
                        is_synthetic: false,
                    }),
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let button = to_mouse_button(*button)?;
                match state {
                    ElementState::Pressed => {
                        if !self.buttons_down.contains(&button) {
                            self.buttons_down.push(button);
                        }
                        Some(Event::MouseDown {
                            button,
                            position: self.cursor,
                            modifiers: self.modifiers,
                            click_count: self.count_click(button),
                        })
                    }
                    ElementState::Released => {
                        self.buttons_down.retain(|down| *down != button);
                        Some(Event::MouseUp {
                            button,
                            position: self.cursor,
                            modifiers: self.modifiers,
                        })
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => Some(Event::ScrollWheel {
                position: self.cursor,
                delta: match delta {
                    MouseScrollDelta::LineDelta(x, y) => ScrollDelta::Lines(vec2f(*x, *y)),
                    // A trackpad's pixel delta is physical, like every other
                    // winit coordinate.
                    MouseScrollDelta::PixelDelta(delta) => {
                        ScrollDelta::Pixels(vec2f(delta.x as f32, delta.y as f32) / scale_factor)
                    }
                },
                modifiers: self.modifiers,
            }),

            // The desktop moved between light and dark. Every platform has its
            // own way of being asked; winit is the one thing that knows all of
            // them, so the answer crosses this line as an event like any
            // other.
            WindowEvent::ThemeChanged(theme) => Some(Event::SystemTheme(system_theme(*theme))),

            // An input method is composing. These arrive *instead of* the key
            // presses that belong to the composition, which is why a field
            // that ignores them cannot type Japanese, Chinese or Korean at all.
            WindowEvent::Ime(ime) => Some(Event::Ime(match ime {
                winit::event::Ime::Enabled => Ime::Enabled,
                winit::event::Ime::Preedit(text, cursor) => Ime::Preedit {
                    text: text.clone(),
                    cursor: *cursor,
                },
                winit::event::Ime::Commit(text) => Ime::Commit(text.clone()),
                winit::event::Ime::Disabled => Ime::Disabled,
            })),

            WindowEvent::KeyboardInput {
                event,
                // Winit fabricates presses for every key already held when a
                // window gains focus. Delivering those would type into a window
                // the moment it is clicked on.
                is_synthetic: false,
                ..
            } => {
                let keystroke = Keystroke::new(event_key_name(event)?, self.modifiers);
                match event.state {
                    ElementState::Pressed => Some(Event::KeyDown {
                        keystroke,
                        chars: event.text.as_deref().unwrap_or_default().to_owned(),
                    }),
                    ElementState::Released => Some(Event::KeyUp { keystroke }),
                }
            }

            _ => None,
        }
    }

    /// Forgets every button it believes is held.
    ///
    /// For the one case where a release genuinely never arrives: a window move
    /// or resize runs inside the window manager's own event loop, which
    /// swallows the release that ends it. Without this the next pointer move
    /// is reported as a drag, and goes on being one until the next click.
    pub(super) fn release_buttons(&mut self) {
        self.buttons_down.clear();
    }

    /// Replays the last pointer position as a synthetic move.
    ///
    /// A frame that changed layout leaves hover state stale — close a tab and
    /// the wrong one stays highlighted — because the pointer did not move, the
    /// world did. Feeding this back in re-evaluates hover against the new
    /// geometry; elements that must not react twice check `is_synthetic`.
    pub(super) fn synthetic_mouse_move(&self) -> Event {
        Event::MouseMoved {
            position: self.cursor,
            modifiers: self.modifiers,
            is_synthetic: true,
        }
    }

    fn count_click(&mut self, button: MouseButton) -> u32 {
        let now = Instant::now();
        let continues = self.last_click.as_ref().is_some_and(|last| {
            last.button == button
                && now.duration_since(last.at) <= MULTI_CLICK_INTERVAL
                && (last.position - self.cursor).x().abs() <= MULTI_CLICK_SLOP
                && (last.position - self.cursor).y().abs() <= MULTI_CLICK_SLOP
        });

        let count = match (&self.last_click, continues) {
            (Some(last), true) => last.count + 1,
            _ => 1,
        };
        self.last_click = Some(Click {
            button,
            at: now,
            position: self.cursor,
            count,
        });

        count
    }
}

/// Winit's word for the desktop's setting, in Crook's vocabulary.
///
/// A desktop that reports nothing at all never reaches here — winit sends no
/// event for one — and the initial read in [`super::app`] treats it as dark,
/// which is what a terminal has always been.
pub(super) fn system_theme(theme: winit::window::Theme) -> SystemTheme {
    match theme {
        winit::window::Theme::Light => SystemTheme::Light,
        winit::window::Theme::Dark => SystemTheme::Dark,
    }
}

fn to_modifiers(state: ModifiersState) -> Modifiers {
    Modifiers {
        alt: state.alt_key(),
        cmd: state.super_key(),
        shift: state.shift_key(),
        ctrl: state.control_key(),
    }
}

fn to_mouse_button(button: winit::event::MouseButton) -> Option<MouseButton> {
    match button {
        winit::event::MouseButton::Left => Some(MouseButton::Left),
        winit::event::MouseButton::Right => Some(MouseButton::Right),
        winit::event::MouseButton::Middle => Some(MouseButton::Middle),
        winit::event::MouseButton::Back => Some(MouseButton::Back),
        winit::event::MouseButton::Forward => Some(MouseButton::Forward),
        // A twelve-button gaming mouse has nothing to say to a terminal.
        winit::event::MouseButton::Other(_) => None,
    }
}

/// The name a key press is known by, keypad included.
///
/// The keypad is the one place the *physical* key matters. Its digits report
/// the same logical key as the ones above the letters, and a terminal has to
/// tell them apart: in application keypad mode — which every full-screen editor
/// turns on — `5` on the keypad sends `SS3 u` and `5` on the number row sends a
/// `5`.
///
/// It is read only when the logical key agrees that a digit was typed. With
/// NumLock off, the keypad's `1` reports `End`, and that is what it means: the
/// key is navigating, not typing, and naming it `numpad1` would send a digit
/// where every terminal sends a cursor movement.
fn event_key_name(event: &winit::event::KeyEvent) -> Option<String> {
    if let Some(name) = keypad_name(event) {
        return Some(name.to_owned());
    }
    key_name(&event.logical_key)
}

/// The `numpad*` name for a keypad key that is typing rather than navigating,
/// or `None` for every other key.
fn keypad_name(event: &winit::event::KeyEvent) -> Option<&'static str> {
    use winit::keyboard::{KeyCode, PhysicalKey};

    let PhysicalKey::Code(code) = event.physical_key else {
        return None;
    };

    // The operators and Enter carry no NumLock question: they type the same
    // character whichever way it is set.
    let unconditional = match code {
        KeyCode::NumpadAdd => "numpadadd",
        KeyCode::NumpadSubtract => "numpadsubtract",
        KeyCode::NumpadMultiply => "numpadmultiply",
        KeyCode::NumpadDivide => "numpaddivide",
        KeyCode::NumpadEqual => "numpadequal",
        KeyCode::NumpadEnter => "numpadenter",
        _ => "",
    };
    if !unconditional.is_empty() {
        return Some(unconditional);
    }

    // The digits and the separator only count as keypad keys while NumLock has
    // them typing. Otherwise the logical key is Home, End, an arrow or Delete,
    // and that is the key that was pressed.
    let typed = match &event.logical_key {
        Key::Character(text) => text.as_str(),
        _ => return None,
    };
    let name = match (code, typed) {
        (KeyCode::Numpad0, "0") => "numpad0",
        (KeyCode::Numpad1, "1") => "numpad1",
        (KeyCode::Numpad2, "2") => "numpad2",
        (KeyCode::Numpad3, "3") => "numpad3",
        (KeyCode::Numpad4, "4") => "numpad4",
        (KeyCode::Numpad5, "5") => "numpad5",
        (KeyCode::Numpad6, "6") => "numpad6",
        (KeyCode::Numpad7, "7") => "numpad7",
        (KeyCode::Numpad8, "8") => "numpad8",
        (KeyCode::Numpad9, "9") => "numpad9",
        (KeyCode::NumpadDecimal, "." | ",") => "numpaddecimal",
        _ => return None,
    };
    Some(name)
}

/// The lowercase name a keymap would spell this key with.
///
/// `None` for a dead key or one the OS could not identify: neither can be bound
/// to, and a dead key's meaning only arrives with the next keystroke.
fn key_name(key: &Key) -> Option<String> {
    match key {
        Key::Named(named) => Some(named_key_name(*named)),
        Key::Character(text) => Some(text.to_lowercase()),
        Key::Dead(_) | Key::Unidentified(_) => None,
    }
}

fn named_key_name(key: NamedKey) -> String {
    // The keys a binding is actually written against get short names; the
    // hundred others — function keys, media keys, IME keys — already have a
    // Debug spelling that lowercases into something usable.
    match key {
        NamedKey::ArrowUp => "up",
        NamedKey::ArrowDown => "down",
        NamedKey::ArrowLeft => "left",
        NamedKey::ArrowRight => "right",
        NamedKey::PageUp => "pageup",
        NamedKey::PageDown => "pagedown",
        other => return format!("{other:?}").to_lowercase(),
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;

    use super::*;

    fn cursor_moved(x: f64, y: f64) -> WindowEvent {
        WindowEvent::CursorMoved {
            device_id: winit::event::DeviceId::dummy(),
            position: PhysicalPosition::new(x, y),
        }
    }

    fn mouse_input(state: ElementState) -> WindowEvent {
        WindowEvent::MouseInput {
            device_id: winit::event::DeviceId::dummy(),
            state,
            button: winit::event::MouseButton::Left,
        }
    }

    #[test]
    fn positions_arrive_in_logical_pixels() {
        let mut input = InputState::default();

        let event = input.convert(&cursor_moved(64., 32.), 2.).unwrap();

        assert_eq!(event.position(), Some(vec2f(32., 16.)));
    }

    #[test]
    fn a_move_with_a_button_held_is_a_drag() {
        let mut input = InputState::default();

        input.convert(&mouse_input(ElementState::Pressed), 1.);
        let dragged = input.convert(&cursor_moved(10., 10.), 1.).unwrap();
        assert!(matches!(dragged, Event::MouseDragged { .. }));

        input.convert(&mouse_input(ElementState::Released), 1.);
        let moved = input.convert(&cursor_moved(20., 20.), 1.).unwrap();
        assert!(matches!(moved, Event::MouseMoved { .. }));
    }

    #[test]
    fn two_presses_in_the_same_place_are_a_double_click() {
        let mut input = InputState::default();

        let counts: Vec<_> = (0..3)
            .map(|_| {
                let down = input.convert(&mouse_input(ElementState::Pressed), 1.);
                input.convert(&mouse_input(ElementState::Released), 1.);
                match down {
                    Some(Event::MouseDown { click_count, .. }) => click_count,
                    other => panic!("expected a press, got {other:?}"),
                }
            })
            .collect();

        assert_eq!(counts, vec![1, 2, 3]);
    }

    #[test]
    fn a_press_far_from_the_last_one_starts_a_new_click() {
        let mut input = InputState::default();

        input.convert(&mouse_input(ElementState::Pressed), 1.);
        input.convert(&mouse_input(ElementState::Released), 1.);
        input.convert(&cursor_moved(100., 100.), 1.);

        let down = input.convert(&mouse_input(ElementState::Pressed), 1.);
        assert!(matches!(
            down,
            Some(Event::MouseDown { click_count: 1, .. })
        ));
    }

    #[test]
    fn named_keys_get_short_lowercase_names() {
        assert_eq!(named_key_name(NamedKey::ArrowUp), "up");
        assert_eq!(named_key_name(NamedKey::Enter), "enter");
        assert_eq!(named_key_name(NamedKey::F5), "f5");
        assert_eq!(key_name(&Key::Character("A".into())).unwrap(), "a");
        assert_eq!(key_name(&Key::Dead(None)), None);
    }
}
