//! What the Plugins page has selected, between one frame and the next.
//!
//! Held by the plugin rather than by the settings page, because it is this
//! page's and nobody else's: somebody who disables this plugin should lose the
//! selection along with the page. The field it types into is the settings
//! page's, though — the settings page has to be able to take the keyboard
//! away from it, and it can only reach what it holds.

use std::cell::RefCell;

use crook_plugin::{Manifest, PluginId};

/// The page's own state.
pub(super) struct PluginsState {
    /// Which plugin the list has selected, by `owner/name`.
    ///
    /// A key rather than an index, because the list is filtered on every
    /// keystroke and an index would select whatever happened to move into that
    /// place. `None` before anything has been clicked, which the page resolves
    /// to the first row rather than to an empty card — a card that said
    /// "choose something" would be a card explaining an interface instead of
    /// being one.
    selected: RefCell<Option<String>>,
}

impl PluginsState {
    /// A page with nothing chosen.
    pub(super) fn new() -> Self {
        Self {
            selected: RefCell::new(None),
        }
    }

    /// Chooses one.
    pub(super) fn select(&self, plugin: &PluginId) {
        *self.selected.borrow_mut() = Some(plugin.to_string());
    }

    /// Which plugin the card is about, resolved against what the list shows.
    ///
    /// Against the *filtered* list rather than everything loaded, because a
    /// card about a plugin the list has filtered away is a card about
    /// something nobody can see. A choice that is not in it — filtered out,
    /// or uninstalled while the page was open — falls back to the first row,
    /// the same way an unchosen one does.
    pub(super) fn showing(&self, showing: &[&'static Manifest]) -> Option<PluginId> {
        let chosen = self
            .selected
            .borrow()
            .as_deref()
            .and_then(|key| showing.iter().find(|manifest| manifest.id.as_str() == key))
            .map(|manifest| manifest.id.clone());

        chosen.or_else(|| showing.first().map(|manifest| manifest.id.clone()))
    }
}
