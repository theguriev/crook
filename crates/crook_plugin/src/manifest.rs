//! What a plugin says about itself before anything of it runs.
//!
//! A manifest is *data*: the store reads it to list a plugin, the settings page
//! reads it to describe one, and the install dialog reads it to say what a
//! plugin will be allowed to do — all without loading a byte of it. That is the
//! whole reason it is separate from the code. herdr's manifests are the model
//! and also the warning: theirs ignore unknown tables, so a plugin that
//! declares a keybinding section the host never learned to read gets silence
//! instead of an error, and its author finds out from a user.
//!
//! Capabilities are here for the reason the rest of it is data: a person
//! cannot refuse what they cannot read, and a manifest that dropped what a
//! plugin asked for would leave the only honest answer to "what may this do?"
//! as "install it and find out". They are the first of the bumps `schema`
//! exists to make possible; the API range, the licence and the rest of what
//! the store needs still arrive with the store.

use crook_plugin_api::Capability;

use crate::id::PluginId;

/// How a plugin runs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Compiled into the binary. This is what every one of Crook's own
    /// features is, and what the store never distributes: what is compiled in
    /// *is* Crook, so there is no such thing as a native plugin that ships
    /// disabled in everybody's binary.
    Native,
    /// A sandboxed module the store distributes and the host loads at runtime.
    /// It describes what it wants drawn; the host draws it.
    Wasm,
    /// A process the host talks to over a socket. Any language, no sandbox,
    /// crash-isolated — for an agent driving the terminal it runs inside.
    Process,
}

impl Tier {
    /// What it is called in a manifest and in the store.
    pub fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Wasm => "wasm",
            Self::Process => "process",
        }
    }
}

/// What a plugin says about itself.
///
/// `'static` throughout because a native plugin's manifest is a constant beside
/// its code. A manifest read from a `plugin.toml` at runtime will be a second
/// constructor returning the same shape with owned strings; the fields are what
/// matters and they are the same either way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The version of the manifest format itself. Two, today.
    pub schema: u32,
    /// Who wrote it and what it is called.
    pub id: PluginId,
    /// The name a person reads.
    pub name: &'static str,
    /// One sentence saying what it does. The store shows this and nothing else
    /// until somebody clicks.
    pub description: &'static str,
    /// The plugin's own version, as semver.
    pub version: &'static str,
    /// How it runs.
    pub tier: Tier,
    /// What it wants to be allowed to do, in the order it asked.
    ///
    /// The asking, and only the asking: what a person *allowed* is kept in
    /// the settings, one key per host and per path, and the two are compared
    /// rather than conflated. That is what makes a plugin whose next version
    /// wants one more host a plugin the Plugins page can say that about,
    /// instead of one that quietly inherits an old yes.
    ///
    /// Empty for every native plugin, and that is not an omission — what is
    /// compiled into Crook *is* Crook, and a capability it could ask itself
    /// for it already has.
    pub capabilities: &'static [Capability],
}

impl Manifest {
    /// The current manifest format.
    ///
    /// **Two**, because a manifest now says what a plugin wants to be allowed
    /// to do. One could name a plugin and place it in a tier, which is enough
    /// to list it and not enough to let anybody decide whether to install it.
    pub const SCHEMA: u32 = 2;
}
