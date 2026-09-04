//! What a plugin is, in this application.
//!
//! [`crook_plugin`] holds the vocabulary — identities, manifests, the two
//! registries and the guard that takes a contribution back out — and knows
//! nothing about Crook. This is the other half: what a contribution *is* here
//! (a closure that builds an element from the workspace), what a handler is (a
//! closure that reaches the workspace), and the [`Host`] that hands both to a
//! plugin while it builds.
//!
//! # Everything Crook does is meant to arrive through here
//!
//! Not as a slogan. An API the application itself does not use is an API
//! nobody has built anything with, and the way to find out whether a slot is
//! the right shape is to move something real onto it and see what breaks. The
//! order features move is in `docs/plugins.md`; each one that lands is a
//! surface a stranger's plugin can reach for the same reason Crook's own can.
//!
//! # What a plugin may not do
//!
//! Panic during `build`, and nothing else is forbidden yet — a native plugin is
//! compiled into the binary and reviewed as the binary. The rule that governs
//! it is the one `settings.rs` states for its own file and that everything
//! since has kept: **nothing here may cost a person their window**. A plugin
//! that fails to build is skipped by name, with one line in the log, and the
//! window opens without whatever it was contributing.

use crookui_core::prelude::*;
use crookui_core::{AppContext, Element};

pub use crook_plugin::{
    ActionName, Actions, Cardinality, Complaint, EntryId, Manifest, PluginId, Registration, SlotId,
    Slots, Tier,
};

use crate::workspace::Workspace;

/// What a plugin contributes to a slot: something that can build an element
/// out of the workspace, every frame.
///
/// A closure taking `&Workspace` rather than a value captured when the plugin
/// built, because `View::render` is immutable and runs again for every frame:
/// a contribution that captured what it wanted to draw would be drawing the
/// state of the window at the moment the plugin loaded.
pub type UiContribution = Box<dyn Fn(&Workspace, &AppContext) -> Box<dyn Element>>;

/// What answers to an [`ActionName`].
///
/// `&mut Workspace` and its context, which is what every existing handler in
/// `Workspace::handle_action` already receives — a named action is the same
/// thing an enum variant was, addressable by people who cannot add a variant.
pub type ActionHandler = Box<dyn Fn(&mut Workspace, &mut ViewContext<Workspace>)>;

/// Everything a plugin may register, and the record of who registered what.
///
/// A plugin never keeps its own guards: [`Host`] holds them, filed under the
/// plugin that made them, so that "disable this plugin" is one `retain` and
/// every surface it touched goes back to what it was. That is the same
/// arrangement Cordis reaches for with a fibre state machine, and here it is
/// a vector of [`Registration`]s being dropped.
pub struct Host {
    slots: Slots<UiContribution>,
    actions: Actions<ActionHandler>,
    /// Whose registrations are being made right now. Set around each plugin's
    /// `build` so a plugin cannot register in another's name by accident.
    building: Option<PluginId>,
    kept: Vec<(PluginId, Registration)>,
    /// The plugins that built, in the order they did.
    loaded: Vec<&'static Manifest>,
    /// The ones that did not, and what went wrong.
    refused: Vec<(PluginId, String)>,
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    /// A host with nothing registered.
    pub fn new() -> Self {
        Self {
            slots: Slots::new(),
            actions: Actions::new(),
            building: None,
            kept: Vec::new(),
            loaded: Vec::new(),
            refused: Vec::new(),
        }
    }

    /// The plugin whose registrations are being made.
    ///
    /// Outside a `build` there is none, and registering then is a bug in the
    /// host rather than in a plugin — so it is named after the host.
    fn who(&self) -> PluginId {
        self.building
            .clone()
            .unwrap_or_else(|| PluginId::parse("crook/host").expect("a literal that parses"))
    }

    /// Declares a slot this plugin owns and will render.
    pub fn declare_slot(&mut self, slot: SlotId, cardinality: Cardinality) {
        let who = self.who();
        let registration = self.slots.declare(&who, slot, cardinality);
        self.kept.push((who, registration));
    }

    /// Contributes something to a slot somebody declares.
    ///
    /// `order` places it among the others: lower is earlier, and two entries
    /// with the same order keep the order their plugins loaded in.
    pub fn contribute(
        &mut self,
        slot: SlotId,
        entry: impl Into<String>,
        order: i32,
        build: impl Fn(&Workspace, &AppContext) -> Box<dyn Element> + 'static,
    ) {
        let who = self.who();
        let registration = self.slots.contribute(
            &who,
            slot,
            EntryId::new(entry),
            order,
            Box::new(build) as UiContribution,
        );
        self.kept.push((who, registration));
    }

    /// Registers something the application can be asked to do by name.
    pub fn register_action(
        &mut self,
        action: ActionName,
        handler: impl Fn(&mut Workspace, &mut ViewContext<Workspace>) + 'static,
    ) {
        let who = self.who();
        let registration = self
            .actions
            .register(&who, action, Box::new(handler) as ActionHandler);
        self.kept.push((who, registration));
    }

    /// The slots, for the renderers that draw them.
    pub fn slots(&self) -> &Slots<UiContribution> {
        &self.slots
    }

    /// The actions, for whatever dispatches one.
    pub fn actions(&self) -> &Actions<ActionHandler> {
        &self.actions
    }

    /// Every plugin that built, in the order it did.
    pub fn loaded(&self) -> &[&'static Manifest] {
        &self.loaded
    }

    /// Every plugin that did not, and why.
    pub fn refused(&self) -> &[(PluginId, String)] {
        &self.refused
    }

    /// Everything the registries have to complain about.
    pub fn audit(&self) -> Vec<Complaint> {
        let mut complaints = self.slots.audit();
        complaints.extend(self.actions.audit());
        complaints
    }

    /// Takes back everything one plugin registered.
    ///
    /// Which is the whole of what disabling a plugin does: the guards drop, the
    /// registries forget, and the next frame draws the window that was there
    /// before the plugin loaded.
    pub fn unload(&mut self, plugin: &PluginId) {
        self.kept.retain(|(by, _)| by != plugin);
        self.loaded.retain(|manifest| &manifest.id != plugin);
    }

    /// Builds one plugin, filing everything it registers under its own name.
    fn build_one(&mut self, plugin: &mut dyn Plugin) {
        let manifest = plugin.manifest();
        self.building = Some(manifest.id.clone());
        let outcome = plugin.build(self);
        self.building = None;

        match outcome {
            Ok(()) => self.loaded.push(manifest),
            Err(problem) => {
                // Everything it managed to register before it gave up goes
                // back out, so a half-built plugin never leaves half a
                // contribution on screen.
                self.unload(&manifest.id);
                log::warn!("the plugin {} did not load: {problem}", manifest.id);
                self.refused.push((manifest.id.clone(), problem));
            }
        }
    }
}

/// Why a plugin did not load.
pub type BuildError = String;

/// One plugin.
///
/// Deliberately two methods. The manifest is *data* — the store reads it to
/// list a plugin and the settings page reads it to describe one, neither of
/// which should have to run any of it — and `build` is everything else.
pub trait Plugin {
    /// What this plugin says about itself.
    fn manifest(&self) -> &'static Manifest;

    /// Registers everything it contributes.
    ///
    /// Called once, before the first frame. Returning an error is how a plugin
    /// declines to load — the host takes back whatever it registered first and
    /// says so by name; it is never how a plugin reports something a person
    /// should act on, which is what the log and the plugins page are for.
    fn build(&mut self, host: &mut Host) -> Result<(), BuildError>;
}

#[cfg(test)]
#[path = "plugin_tests.rs"]
mod tests;

/// Builds every plugin in `plugins`, in order, into a fresh host.
///
/// Order is load order, and load order is what settles two entries that asked
/// for the same place in a slot. It is a list in the source rather than
/// anything resolved at runtime: a dependency here is a `use`, and a plugin
/// that will not compile is a build failure with a name on it rather than a
/// window that comes up silently missing a feature.
pub fn load(plugins: Vec<Box<dyn Plugin>>) -> Host {
    let mut host = Host::new();
    let mut plugins = plugins;
    for plugin in &mut plugins {
        host.build_one(plugin.as_mut());
    }

    for complaint in host.audit() {
        log::warn!("{complaint}");
    }

    host
}
