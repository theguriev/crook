//! What the Plugins page has selected, between one frame and the next.
//!
//! Held by the plugin rather than by the settings page, because it is this
//! page's and nobody else's: somebody who disables this plugin should lose the
//! selection along with the page. The field it types into is the settings
//! page's, though — the settings page has to be able to take the keyboard
//! away from it, and it can only reach what it holds.
//!
//! Beside the selection, the two things a card holds that outlive a frame:
//! what the last thing pressed on it came to, and the pictures somebody asked
//! to see — decoded on the pool and landed here, one plugin's set at a time,
//! because six screenshots of pixels are a thing to hold for one card and not
//! for every plugin in the list.

use std::cell::RefCell;

use crook_plugin::{Manifest, PluginId};

use crate::plugins::pictures::Decoded;

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
    /// What the last press that could go wrong came to: which plugin it was
    /// about, the sentence, and whether it went wrong.
    ///
    /// Drawn on that plugin's card and on no other, the way the Store keeps
    /// a sentence with the row it is about: a line drawn on whatever card
    /// happens to be showing is a line about the wrong thing.
    said: RefCell<Option<(String, String, bool)>>,
    /// Which plugin's previews are being decoded right now, if any.
    opening: RefCell<Option<String>>,
    /// The previews somebody asked to see, decoded, with their captions —
    /// one plugin's, and the plugin they belong to.
    pictures: RefCell<Option<(String, Decoded)>>,
}

impl PluginsState {
    /// A page with nothing chosen.
    pub(super) fn new() -> Self {
        Self {
            selected: RefCell::new(None),
            said: RefCell::new(None),
            opening: RefCell::new(None),
            pictures: RefCell::new(None),
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

    /// Writes down what a press came to, about `plugin`.
    pub(super) fn say(&self, plugin: &PluginId, sentence: String, wrong: bool) {
        *self.said.borrow_mut() = Some((plugin.to_string(), sentence, wrong));
    }

    /// The sentence to draw on `plugin`'s card, and whether it is a warning,
    /// if the last press was about it.
    pub(super) fn said_about(&self, plugin: &PluginId) -> Option<(String, bool)> {
        self.said
            .borrow()
            .as_ref()
            .filter(|(about, _, _)| about == plugin.as_str())
            .map(|(_, sentence, wrong)| (sentence.clone(), *wrong))
    }

    /// Marks `plugin`'s previews as being decoded.
    pub(super) fn opening(&self, plugin: &PluginId) {
        *self.opening.borrow_mut() = Some(plugin.to_string());
    }

    /// Whether `plugin`'s previews are being decoded right now.
    pub(super) fn is_opening(&self, plugin: &PluginId) -> bool {
        self.opening.borrow().as_deref() == Some(plugin.as_str())
    }

    /// Takes the decoded previews of `plugin`, replacing whichever plugin's
    /// were held before.
    ///
    /// One set at a time, whichever card they were for: a person who opened
    /// the pictures of three plugins in a row is looking at one card, and
    /// the other two sets would be pixels held for nobody.
    pub(super) fn landed(&self, plugin: &PluginId, pictures: Decoded) {
        // Only if this is still the decode being waited on: a second press
        // that started another plugin's decode is the one whose landing
        // ends the wait.
        if self.is_opening(plugin) {
            *self.opening.borrow_mut() = None;
        }
        *self.pictures.borrow_mut() = Some((plugin.to_string(), pictures));
    }

    /// The decoded previews of `plugin`, if they are the ones held.
    pub(super) fn pictures_of(&self, plugin: &PluginId) -> Option<Decoded> {
        self.pictures
            .borrow()
            .as_ref()
            .filter(|(about, _)| about == plugin.as_str())
            .map(|(_, pictures)| pictures.clone())
    }
}
