//! Where a rasterized glyph goes inside a texture.
//!
//! Two pieces, both deliberately dumb. [`Allocator`] packs items into one
//! square texture with shelf/next-fit and never frees; [`Manager`] mints a new
//! texture when one fills up. There is no eviction, no repacking and no
//! defragmentation, because a terminal's working set of glyphs is bounded — the
//! failure mode is a long session that cycles fonts and scale factors growing
//! by 4 MB a time, which is a trade worth knowing about and not worth
//! preventing yet.
//!
//! Ported from Warp's `crates/warpui/src/rendering/atlas/` (MIT), with
//! pathfinder's `RectI` replaced by [`PixelRect`] — the allocator only ever
//! produces non-negative coordinates, so unsigned is both honest and one fewer
//! cast at every use site.

mod allocator;
mod manager;

use std::error;
use std::fmt;

use crookui_core::geometry::RectF;

pub use allocator::Allocator;
pub use manager::{Manager, TextureId, TextureOffset};

/// A rectangle in whole texture pixels.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct PixelRect {
    /// Distance from the atlas's left edge.
    pub x: u32,
    /// Distance from the atlas's top edge.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// A region of an atlas that has been handed out.
///
/// Both descriptions of the same rectangle: the UV one is what the shader
/// samples with, the pixel one is what the upload writes to and what the glyph
/// quad is sized by.
#[derive(Copy, Clone, Debug)]
pub struct AllocatedRegion {
    /// The region in UV space, `0.0..=1.0` on both axes.
    pub uv_region: RectF,
    /// The region in whole pixels.
    pub pixel_region: PixelRect,
}

/// Why an insert did not fit.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AllocationError {
    /// This atlas has no room left. The caller should start another one.
    Full,
    /// The item is larger than a whole atlas, so no atlas will ever take it.
    ItemTooLarge,
}

impl fmt::Display for AllocationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => write!(f, "the atlas is full"),
            Self::ItemTooLarge => write!(f, "the item is too large to fit into an atlas"),
        }
    }
}

impl error::Error for AllocationError {}
