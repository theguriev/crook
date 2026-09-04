//! The header row, and the one place anything may be pinned to it.
//!
//! The row itself is still drawn by `workspace::header_toolbar`; what this
//! plugin owns is the *slot* — and owning it is what makes the header's
//! right-hand side something other things can reach. Its own doc used to say
//! the quiet part: "There is one right-hand item and its slot is hard-coded."
//! It is not any more.
//!
//! [`HEADER_RIGHT`] is [`Cardinality::Single`] rather than a list, and that is a
//! judgement about the surface rather than a limitation of the mechanism: the
//! header is the one row that is always on screen, it is also the window's
//! title bar, and a row of competing chips across it is how a status bar
//! becomes a place nobody reads. When two plugins want it, the lowest `order`
//! is drawn and the audit says so by name.

use crook_plugin::{Cardinality, Manifest, PluginId, SlotId, Tier};

use crate::plugin::{BuildError, Host, Plugin};

/// The one item pinned to the right of the header.
pub const HEADER_RIGHT: SlotId = SlotId::new("header.right");

/// The plugin that owns the header's slot.
pub struct Header;

impl Plugin for Header {
    fn manifest(&self) -> &'static Manifest {
        manifest()
    }

    fn build(&mut self, host: &mut Host) -> Result<(), BuildError> {
        host.declare_slot(HEADER_RIGHT, Cardinality::Single);
        Ok(())
    }
}

/// Built once and leaked, because a manifest outlives everything that reads it
/// and `PluginId` cannot be constructed in a `const`.
fn manifest() -> &'static Manifest {
    static MANIFEST: std::sync::OnceLock<Manifest> = std::sync::OnceLock::new();
    MANIFEST.get_or_init(|| Manifest {
        schema: Manifest::SCHEMA,
        id: PluginId::parse("crook/header").expect("a literal that parses"),
        name: "Header",
        description: "The row across the top of the window, and what may be pinned to it.",
        version: env!("CARGO_PKG_VERSION"),
        tier: Tier::Native,
    })
}
