//! The icon set: Lucide's geometry, and the mask one rasterizes into.
//!
//! # Why there is an icon system now
//!
//! Every mark in Crook's chrome used to be one of two things, and both were
//! wrong. A gear was `⚙` and a close button `×` — *codepoints*, drawn out of
//! whatever font the machine happened to have, which is why a gear is flat on
//! one machine and a colour emoji on the next, and why a check mark is a
//! different weight from the label beside it. Everything a font would not
//! draw was built out of `Container`s instead: the two density marks in the
//! gear menu were seven rectangles, and the git branch beside a tab title was
//! three, which is as close to a branch as three rectangles get.
//!
//! So: one set, with its own geometry, drawn by this crate. [Lucide] is that
//! set — ISC-licensed, and every icon in it is a 24 x 24 box with a 2-unit
//! stroke, round caps and round joins and no fill, which is the whole reason
//! it fits in 200 lines of rasterizer.
//!
//! [Lucide]: https://lucide.dev
//!
//! # How an icon gets here
//!
//! `script/icons` downloads a pinned `lucide-static` and writes [`data`]:
//! arcs, shorthands and the `circle`, `rect` and `polyline` elements are all
//! resolved at generation time, so what reaches the runtime is moves, lines
//! and cubics. Nothing parses SVG at runtime, because the icons cannot change
//! at runtime.
//!
//! # What it cannot draw
//!
//! Fills. Every Lucide icon is `fill="none"` and the rasterizer strokes what
//! it is given, so an icon with a filled region would come out as its own
//! outline. The generator refuses such an icon rather than letting it render
//! wrongly.

mod data;
mod raster;

#[cfg(test)]
mod tests;

pub use data::{ICONS, Lucide};
pub use raster::rasterize;

use std::hash::{Hash, Hasher};

/// The box every icon is drawn in, in Lucide's own units.
pub const GRID: f32 = 24.;

/// Lucide's stroke width in that box.
///
/// Scaled with the icon, which is what `lucide-react` does by default: a 16px
/// icon is drawn with a 1.33px stroke, not a 2px one.
pub const STROKE_WIDTH: f32 = 2.;

/// One step of an icon's outline, in the 24-unit grid.
///
/// Absolute, and only four kinds: everything else SVG can say was resolved
/// into these by the generator. There is no `Fill` because there are no fills.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Segment {
    /// Start a new subpath here.
    Move(f32, f32),
    /// A straight line to here.
    Line(f32, f32),
    /// A cubic Bézier: two control points, then the end.
    Cubic(f32, f32, f32, f32, f32, f32),
    /// Close the subpath back to where it started.
    Close,
}

/// Every icon there is, in the order the generator wrote them.
pub fn all() -> impl Iterator<Item = Lucide> {
    data::TABLE.iter().map(|(icon, _, _)| *icon)
}

impl Lucide {
    /// This icon's outline.
    pub fn outline(self) -> &'static [Segment] {
        self.row().2
    }

    /// Its name in Lucide, which is also the name of its file there.
    pub fn name(self) -> &'static str {
        self.row().1
    }

    /// Its row of the generated table.
    ///
    /// By search rather than by index, so the table's order is free to change
    /// and adding an icon out of alphabetical order cannot silently pair a
    /// variant with someone else's geometry.
    fn row(self) -> &'static (Lucide, &'static str, &'static [Segment]) {
        data::TABLE
            .iter()
            .find(|(icon, _, _)| *icon == self)
            .expect("every variant is generated from the same list as the table")
    }
}

/// Everything needed to rasterize one icon, and the key it is cached under.
///
/// The shape [`GlyphKey`](crate::fonts::GlyphKey) has, and for the same
/// reason: floats participate in equality by their bit pattern, because two
/// sizes that are not bitwise identical rasterize differently anyway.
#[derive(Copy, Clone, Debug)]
pub struct IconKey {
    /// Which icon.
    pub icon: Lucide,
    /// The edge of the square it is drawn in, in logical pixels.
    pub size: f32,
    /// The stroke width, in the 24-unit grid rather than in pixels — so an
    /// icon drawn smaller keeps Lucide's proportions.
    pub stroke_width: f32,
}

impl IconKey {
    /// An icon at `size`, with Lucide's own stroke.
    pub fn new(icon: Lucide, size: f32) -> Self {
        Self {
            icon,
            size,
            stroke_width: STROKE_WIDTH,
        }
    }
}

impl PartialEq for IconKey {
    fn eq(&self, other: &Self) -> bool {
        self.icon == other.icon
            && self.size.to_bits() == other.size.to_bits()
            && self.stroke_width.to_bits() == other.stroke_width.to_bits()
    }
}

impl Eq for IconKey {}

impl Hash for IconKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.icon.hash(state);
        self.size.to_bits().hash(state);
        self.stroke_width.to_bits().hash(state);
    }
}
