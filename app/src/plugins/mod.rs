//! The plugins Crook is made of.
//!
//! Every feature in this application is meant to end up here, and the list in
//! [`defaults`] is what a release binary carries. That is the whole of what
//! "everything is a plugin" can mean in a language with no runtime loading —
//! it is Bevy's arrangement, not Cordis's: the set is a list in the source, a
//! dependency between two plugins is a `use`, and a plugin that will not
//! compile is a build failure with a name on it rather than a window that comes
//! up silently missing something.
//!
//! **Nothing here ships disabled.** What is compiled in is Crook; a plugin the
//! binary carried but did not run would be bytes in everybody's download for a
//! feature nobody asked for. The store's plugins are a different tier and live
//! outside the binary entirely — see `docs/plugins.md`.
//!
//! # The order of this list is load order
//!
//! And load order is what settles two contributions that asked for the same
//! place in a slot. Declarations come first by convention, so that a
//! contribution to a slot is normally made after the slot exists and the
//! startup audit stays quiet — though it need not, and a contribution that
//! arrives early is kept rather than refused.

mod about;
mod appearance;
pub mod header;
mod keys;
mod palette;
mod plugins_page;
mod settings;
mod shell;
mod usage;
pub mod wasm;
pub mod window;

use crate::plugin::Plugin;

/// Every plugin a release binary carries, in load order.
pub fn defaults() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(window::Window),
        Box::new(header::Header),
        Box::new(settings::Settings),
        // The rail's order is these five, and it is the `order` each of them
        // asks for rather than this list — a plugin that adds a page cannot
        // be made to load in the right place in somebody else's list.
        Box::new(appearance::Appearance),
        Box::new(shell::Shell),
        Box::new(usage::Usage),
        Box::new(keys::Keys),
        Box::new(plugins_page::Plugins),
        Box::new(about::About),
        Box::new(palette::CommandPalette),
    ]
}
