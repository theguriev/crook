//! The pirate: the one mark here that is filled rather than stroked.
//!
//! Everything else in this module is Lucide — a 24-unit box, a 2-unit stroke,
//! no fill — and a chip that says how much of a session is spent is the one
//! place that rule does not reach. The mark it wants is a picture: a yellow
//! head with a bite out of it, an eyepatch, and a strap across the brow. Three
//! bites and two colours cannot be said in strokes of one width, so this is
//! the geometry that says it, in the same 24-unit grid so that nothing
//! downstream has to know which kind of mark it is holding.
//!
//! # Two layers, because a mask has one colour
//!
//! A rasterized mark is an [`A8`](crate::fonts::RasterFormat::A8) coverage
//! mask that the renderer tints, so a two-colour picture is two masks drawn
//! one over the other: [`Art::PirateFace`] in the pirate's yellow, and
//! [`Art::PirateInk`] over it in black. Neither is meaningful alone, and the
//! chip draws them as one [`Stack`](crate::elements::Stack).
//!
//! # Clipping, and why it is a layer rather than a rectangle
//!
//! The artwork is drawn well outside the head: the strap runs off both edges
//! of the box and the grin is one arc of a circle that is mostly off it. What
//! keeps them on the face is that every layer is multiplied by the coverage of
//! the layer it is drawn on — the ink by its own frame's face, and a bitten
//! face by the whole head — which is the same clip the source artwork applies
//! and needs no clip geometry of its own.
//!
//! The geometry is the artwork from Warp's `pirate-pacman-*.svg`, scaled from
//! its 330-unit box into this one. It is transcribed rather than parsed: an
//! SVG parser at runtime would spend work on every launch to produce a
//! constant, which is the same reason [`data`](super::data) is generated.

use super::Segment::{self, Close, Cubic, Line, Move};

/// How far into the bite the pirate is.
///
/// The chip cycles shut → open → wide → open while a person waits on a
/// reading, and rests on [`Shut`](Self::Shut) — the frame that is a whole
/// head, so the pirate never freezes mid-bite.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Chomp {
    /// Mouth closed, wearing a grin.
    Shut,
    /// Mouth open by a quarter turn.
    Open,
    /// Mouth open as wide as it goes.
    Wide,
}

/// One layer of a drawn mark.
///
/// Two per frame, because a coverage mask carries no colour of its own. Draw
/// them in this order — face, then ink — and never one without the other: the
/// ink alone is a strap and an eyepatch floating over the header.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Art {
    /// The head: a disc, with a bite out of it in every frame but
    /// [`Chomp::Shut`].
    PirateFace(Chomp),
    /// What is drawn on the head: the eyepatch, the strap across the brow,
    /// and — while the mouth is shut — the grin.
    PirateInk(Chomp),
}

impl Art {
    /// What this layer is made of.
    pub(super) fn painting(self) -> Painting {
        match self {
            Self::PirateFace(Chomp::Shut) => Painting {
                fills: &[HEAD],
                strokes: &[],
                // The whole head *is* the clip every other layer is cut to,
                // so this is the one layer with nothing above it.
                clip: None,
            },
            Self::PirateFace(Chomp::Open) => Painting {
                fills: &[FACE_OPEN],
                strokes: &[],
                clip: Some(Self::PirateFace(Chomp::Shut)),
            },
            Self::PirateFace(Chomp::Wide) => Painting {
                fills: &[FACE_WIDE],
                strokes: &[],
                clip: Some(Self::PirateFace(Chomp::Shut)),
            },
            Self::PirateInk(Chomp::Shut) => Painting {
                fills: &[PATCH_SHUT],
                strokes: &[(STRAP_SHUT, STRAP_STROKE), (GRIN_SHUT, GRIN_STROKE)],
                clip: Some(Self::PirateFace(Chomp::Shut)),
            },
            // One eyepatch serves both open frames: the head turns into the
            // bite, the face it is painted on does not.
            Self::PirateInk(chomp @ (Chomp::Open | Chomp::Wide)) => Painting {
                fills: &[PATCH_CHOMPING],
                strokes: &[(STRAP_CHOMPING, STRAP_STROKE)],
                clip: Some(Self::PirateFace(chomp)),
            },
        }
    }
}

/// What one layer is made of, in the 24-unit grid.
///
/// Fills first, then strokes, both unioned into one coverage mask — they are
/// one colour, so an overlap is not a question anyone has to answer — and the
/// result multiplied by `clip`.
pub(super) struct Painting {
    /// Subpaths to fill. Closed implicitly, so a missing [`Close`] is not a
    /// hole in the picture.
    pub fills: &'static [&'static [Segment]],
    /// Subpaths to stroke, each with its own width in grid units. Lucide's
    /// single stroke width is no use here: the strap and the grin are drawn at
    /// different weights in the artwork.
    pub strokes: &'static [(&'static [Segment], f32)],
    /// The layer whose coverage this one is multiplied by, which is what keeps
    /// the strap on the head.
    pub clip: Option<Art>,
}

/// The strap's width, which is the artwork's 15 units of 330 in this grid.
const STRAP_STROKE: f32 = 1.091;

/// The grin's, which is the artwork's 17.
const GRIN_STROKE: f32 = 1.236;

/// The whole head: a disc filling the box.
///
/// Four cubics rather than an arc, because nothing downstream draws arcs —
/// the generator resolves Lucide's into cubics for the same reason. The
/// control points are the usual 0.5523 of the radius.
const HEAD: &[Segment] = &[
    Move(24.000, 12.000),
    Cubic(24.000, 18.627, 18.627, 24.000, 12.000, 24.000),
    Cubic(5.373, 24.000, 0.000, 18.627, 0.000, 12.000),
    Cubic(0.000, 5.373, 5.373, 0.000, 12.000, 0.000),
    Cubic(18.627, 0.000, 24.000, 5.373, 24.000, 12.000),
    Close,
];

/// The head with a quarter-turn bite out of it, opening to the right.
const FACE_OPEN: &[Segment] = &[
    Move(23.048, 7.316),
    Cubic(21.914, 4.642, 19.849, 2.470, 17.236, 1.202),
    Cubic(14.622, -0.065, 11.638, -0.341, 8.836, 0.425),
    Cubic(6.034, 1.190, 3.605, 2.946, 2.000, 5.367),
    Cubic(0.394, 7.787, -0.279, 10.708, 0.105, 13.587),
    Cubic(0.489, 16.466, 1.905, 19.108, 4.089, 21.023),
    Cubic(6.272, 22.938, 9.077, 23.995, 11.981, 24.000),
    Cubic(14.886, 24.005, 17.693, 22.955, 19.883, 21.047),
    Cubic(22.073, 19.139, 23.496, 16.502, 23.890, 13.624),
    Line(12.000, 12.000),
    Line(23.048, 7.316),
    Close,
];

/// The same head, bitten as wide as the artwork goes.
const FACE_WIDE: &[Segment] = &[
    Move(23.048, 7.316),
    Cubic(22.034, 4.924, 20.271, 2.926, 18.024, 1.621),
    Cubic(15.776, 0.317, 13.167, -0.222, 10.587, 0.084),
    Cubic(8.007, 0.390, 5.596, 1.525, 3.716, 3.318),
    Cubic(1.836, 5.112, 0.589, 7.467, 0.163, 10.030),
    Cubic(-0.264, 12.593, 0.153, 15.225, 1.351, 17.531),
    Cubic(2.548, 19.837, 4.462, 21.691, 6.803, 22.816),
    Cubic(9.145, 23.942, 11.789, 24.276, 14.338, 23.770),
    Cubic(16.886, 23.264, 19.201, 21.945, 20.935, 20.010),
    Line(12.000, 12.000),
    Line(23.048, 7.316),
    Close,
];

/// The eyepatch on the shut face.
const PATCH_SHUT: &[Segment] = &[
    Move(11.627, 6.343),
    Cubic(12.645, 8.384, 13.840, 9.395, 15.825, 9.216),
    Cubic(18.176, 9.003, 18.932, 7.566, 19.323, 4.335),
    Close,
];

/// Its strap, which runs off both sides of the box and is cut to the head.
///
/// The last two subpaths are the patch's own edges, stroked over the fill
/// above so that the patch is drawn with the same rounded corners the strap
/// has rather than as a bare triangle.
const STRAP_SHUT: &[Segment] = &[
    Move(31.491, 1.164),
    Cubic(28.486, 1.948, 22.328, 3.551, 19.323, 4.335),
    Cubic(16.318, 5.119, 14.633, 5.559, 11.627, 6.343),
    Line(-5.891, 11.037),
    Move(11.627, 6.343),
    Cubic(12.645, 8.384, 13.840, 9.395, 15.825, 9.216),
    Cubic(18.176, 9.003, 18.932, 7.566, 19.323, 4.335),
    Move(11.627, 6.343),
    Line(19.323, 4.335),
];

/// The grin the shut mouth wears: one arc of a circle that is mostly off the
/// box, of which the head keeps the stroke across its lower right.
const GRIN_SHUT: &[Segment] = &[
    Move(27.848, 4.800),
    Cubic(28.529, 5.650, 29.036, 6.625, 29.340, 7.670),
    Cubic(29.644, 8.716, 29.739, 9.811, 29.619, 10.893),
    Cubic(29.500, 11.975, 29.169, 13.024, 28.645, 13.978),
    Cubic(28.120, 14.932, 27.413, 15.774, 26.564, 16.455),
    Cubic(25.714, 17.135, 24.739, 17.642, 23.693, 17.946),
    Cubic(22.648, 18.250, 21.553, 18.346, 20.470, 18.226),
    Cubic(19.388, 18.107, 18.340, 17.776, 17.386, 17.251),
    Cubic(16.432, 16.727, 15.590, 16.020, 14.909, 15.170),
];

/// The eyepatch while the mouth is open, which sits higher on the head than
/// the shut frame's does.
const PATCH_CHOMPING: &[Segment] = &[
    Move(8.320, 5.428),
    Cubic(9.811, 7.153, 11.220, 7.837, 13.098, 7.172),
    Cubic(15.323, 6.383, 15.700, 4.804, 15.279, 1.577),
    Close,
];

/// Its strap, with the patch's edges stroked over it as in [`STRAP_SHUT`].
const STRAP_CHOMPING: &[Segment] = &[
    Move(26.282, -4.509),
    Cubic(23.565, -3.005, 17.996, 0.073, 15.279, 1.577),
    Cubic(12.561, 3.081, 11.037, 3.924, 8.320, 5.428),
    Line(-7.490, 14.313),
    Move(8.320, 5.428),
    Cubic(9.811, 7.153, 11.220, 7.837, 13.098, 7.172),
    Cubic(15.323, 6.383, 15.700, 4.804, 15.279, 1.577),
    Move(8.320, 5.428),
    Line(15.279, 1.577),
];
