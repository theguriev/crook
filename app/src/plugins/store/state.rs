//! What the Store section has selected, and where its answers come from.
//!
//! Two things the section's closures and its action handlers both need, and
//! neither can own: a contribution is an `Fn(&Workspace, &AppContext)` and a
//! handler is an `Fn(&mut Workspace, &mut ViewContext)`, so neither can hold a
//! `&mut` to the plugin. This is what they share instead.

use std::cell::RefCell;

use crookui_core::prelude::*;

use crook_plugin::PluginId;

use super::index::Offer;
use super::model::StoreModel;

/// The section's own state.
pub(super) struct StoreState {
    /// Which row the card is about, by `owner/name`.
    ///
    /// A key rather than an index, for the reason the Plugins page keeps one:
    /// the list is filtered on every keystroke, and an index would select
    /// whatever moved into that place.
    selected: RefCell<Option<String>>,
    /// The model, once the plugin has built one.
    ///
    /// An `Option` because the state is made before there is a context to make
    /// a model with, and `None` is never seen after `build` — the section is
    /// not on screen until the plugin that draws it has loaded.
    model: RefCell<Option<ModelHandle<StoreModel>>>,
}

impl StoreState {
    /// A section with nothing chosen and nothing to show yet.
    pub(super) fn new() -> Self {
        Self {
            selected: RefCell::new(None),
            model: RefCell::new(None),
        }
    }

    /// Hands it the model its plugin just made.
    pub(super) fn attach(&self, model: ModelHandle<StoreModel>) {
        *self.model.borrow_mut() = Some(model);
    }

    /// The model, for whoever has a context to read or update it with.
    pub(super) fn model(&self) -> Option<ModelHandle<StoreModel>> {
        self.model.borrow().clone()
    }

    /// Chooses a row.
    pub(super) fn select(&self, plugin: &str) {
        *self.selected.borrow_mut() = Some(plugin.to_owned());
    }

    /// Which plugin the card is about, resolved against what the list shows.
    ///
    /// Against the *filtered* list, so a card about a row nobody can see falls
    /// back to the first one there is — the same rule the Plugins page's list
    /// follows, and for the same reason.
    pub(super) fn showing(&self, showing: &[Offer]) -> Option<PluginId> {
        let chosen = self
            .selected
            .borrow()
            .as_deref()
            .and_then(|key| showing.iter().find(|offer| offer.id.as_str() == key))
            .map(|offer| offer.id.clone());

        chosen.or_else(|| showing.first().map(|offer| offer.id.clone()))
    }
}
