//! Bindings a person wrote down, consulted before Crook's own.
//!
//! # What this is not
//!
//! Not a keymap *system*. Warp has editable bindings, fixed bindings, context
//! predicates and a resolution order between them; this is one file, one table,
//! and one rule: a chord in it wins over the built-in binding for the same
//! chord, and everything it does not mention is unchanged.
//!
//! That is the whole of what the seam was left open for. Keyboard and mouse
//! already produce the *same* action values — a click on the `+` and `cmd-t`
//! dispatch one `TabAction::New` — so a layer that turns a chord into an
//! action needs no second dispatch path and touches no handler. This is that
//! layer, and it is deliberately the smallest thing that is honestly one.
//!
//! # What can be bound
//!
//! Only the window's own actions: opening a tab, closing a pane, moving
//! between tabs, the zoom, the settings page. **Not what the pane does with a
//! key**, which is the shell's — `ctrl-c` interrupts, `ctrl-d` ends an input,
//! and a keymap that could take one of those away would be a keymap that can
//! break a terminal. [`crate::input_keys::route`] is where that line is drawn,
//! and nothing here reaches it.
//!
//! # Nothing here can cost a person their window
//!
//! The rule the rest of the configuration follows. A missing file, one that is
//! not JSON, a chord nobody can parse, an action this build has never heard
//! of: each is one line in the log and one binding that is not installed. A
//! keymap that refused to load would be a keymap that stops Crook opening.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crookui_core::event::{Keystroke, Modifiers};
use serde_json::{Map, Value};

use crate::input_keys::Binding;
use crate::settings::config_directory;

/// The file a person writes their bindings in.
const KEYMAP_FILE: &str = "keymap.json";

/// The value that means "this chord does nothing".
///
/// The one thing a table of overrides cannot express by omission: leaving a
/// chord out keeps Crook's own binding, and this is how it is taken away —
/// which is what somebody does when a chord of Crook's collides with one their
/// shell or their editor wants.
const UNBOUND: &str = "none";

/// The bindings a person wrote down.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keymap {
    /// What each chord was bound to, or `None` where it was unbound.
    bindings: HashMap<Keystroke, Option<Binding>>,
}

impl Keymap {
    /// An empty keymap: every chord means what Crook says it means.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether nothing was overridden.
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// What this chord means, or `None` to fall through to Crook's own table.
    ///
    /// The two `None`s are different and the nesting says which is which: the
    /// outer one is "this keymap says nothing about that chord", and the inner
    /// one is "this keymap says that chord does nothing".
    pub fn binding(&self, keystroke: &Keystroke) -> Option<Option<Binding>> {
        self.bindings.get(keystroke).copied()
    }

    /// Reads the per-user keymap, defaulting past anything unusable.
    pub fn for_user() -> Self {
        match user_keymap_path() {
            Some(path) => Self::load(path),
            None => Self::new(),
        }
    }

    /// Reads the keymap at `path`. Tests point this at a scratch directory.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let Ok(text) = fs::read_to_string(path) else {
            // No file is the ordinary state, and not worth a line.
            return Self::new();
        };

        let document: Map<String, Value> = match serde_json::from_str(&text) {
            Ok(Value::Object(document)) => document,
            Ok(_) => {
                log::warn!("{} is not a JSON object of chords", path.display());
                return Self::new();
            }
            Err(error) => {
                log::warn!("could not read {}: {error}", path.display());
                return Self::new();
            }
        };

        Self::from_document(&document, path)
    }

    /// Turns a parsed document into a table, dropping what cannot be read.
    fn from_document(document: &Map<String, Value>, path: &Path) -> Self {
        let mut bindings = HashMap::new();

        for (chord, action) in document {
            let Some(keystroke) = parse_chord(chord) else {
                log::warn!("{}: {chord:?} is not a chord", path.display());
                continue;
            };
            let Some(action) = action.as_str() else {
                log::warn!(
                    "{}: {chord:?} is bound to something that is not a name",
                    path.display()
                );
                continue;
            };
            let Some(binding) = parse_action(action) else {
                log::warn!(
                    "{}: {chord:?} is bound to {action:?}, which is not an action",
                    path.display()
                );
                continue;
            };
            bindings.insert(keystroke, binding);
        }

        Self { bindings }
    }
}

/// Where the per-user keymap lives.
pub fn user_keymap_path() -> Option<PathBuf> {
    Some(config_directory()?.join(KEYMAP_FILE))
}

/// A chord like `cmd-shift-d`, or `None` when it names no key.
///
/// The modifiers are taken off the front one at a time and whatever is left is
/// the key, which is what makes `ctrl--` and `cmd-+` parse: the key is
/// everything after the last modifier, however many dashes are in it.
pub fn parse_chord(chord: &str) -> Option<Keystroke> {
    let mut rest = chord.trim();
    let mut modifiers = Modifiers::default();

    loop {
        let Some((head, tail)) = rest.split_once('-') else {
            break;
        };
        match head.to_ascii_lowercase().as_str() {
            "cmd" | "super" | "win" | "meta" => modifiers.cmd = true,
            "ctrl" | "control" => modifiers.ctrl = true,
            "alt" | "opt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            // Not a modifier, so the rest of the string — dashes and all — is
            // the key. This is what makes `ctrl--` parse: the second dash
            // splits into an empty head, which is not a modifier, and what is
            // left is the `-` key.
            _ => break,
        }
        rest = tail;
    }

    let key = rest.to_ascii_lowercase();
    // A chord with no key is a string of modifiers, which names nothing that
    // can be pressed.
    (!key.is_empty()).then(|| Keystroke::new(key, modifiers))
}

/// The action a name stands for: `Some(None)` unbinds, `None` is a name this
/// build does not know.
pub fn parse_action(name: &str) -> Option<Option<Binding>> {
    let binding = match name.trim().to_ascii_lowercase().as_str() {
        UNBOUND => return Some(None),
        "new_tab" => Binding::NewTab,
        "close_pane" => Binding::ClosePane,
        "split_right" => Binding::SplitRight,
        "split_down" => Binding::SplitDown,
        "previous_tab" => Binding::PreviousTab,
        "next_tab" => Binding::NextTab,
        "move_tab_left" => Binding::MoveTabLeft,
        "move_tab_right" => Binding::MoveTabRight,
        "toggle_layout" => Binding::ToggleLayout,
        "open_settings" => Binding::OpenSettings,
        "zoom_in" => Binding::ZoomIn,
        "zoom_out" => Binding::ZoomOut,
        "zoom_reset" => Binding::ZoomReset,
        _ => return None,
    };
    Some(Some(binding))
}

/// Every action a keymap may name, for the settings page to list.
///
/// In the order the settings page's Keys section already lists the bindings,
/// so a person reading one and writing the other is reading the same order
/// twice.
pub const ACTION_NAMES: [&str; 14] = [
    "new_tab",
    "close_pane",
    "split_right",
    "split_down",
    "previous_tab",
    "next_tab",
    "move_tab_left",
    "move_tab_right",
    "toggle_layout",
    "open_settings",
    "zoom_in",
    "zoom_out",
    "zoom_reset",
    UNBOUND,
];

#[cfg(test)]
#[path = "keymap_tests.rs"]
mod tests;
