//! 2D geometry and color: the vocabulary types every layer of the UI shares.
//!
//! Warp spells these `pathfinder_geometry::vector::Vector2F`,
//! `pathfinder_geometry::rect::RectF` and `pathfinder_color::ColorU`. Those
//! crates are tiny and dependency-free, but `pathfinder_geometry` pulls in
//! `pathfinder_simd`, which Warp has to redirect through a `[patch]` at a fork
//! to keep building on stable — a cross-repo dependency Crook is not willing to
//! take on for 200 lines of arithmetic. They are hand-rolled here instead, with
//! the same method names so ported code reads the same.
//!
//! One consequence worth knowing: pathfinder's SIMD vectors carry float error
//! larger than `f32::EPSILON`, so Warp compares sizes against its own epsilon.
//! These are plain scalars, so ordinary comparisons are exact.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// A 2D vector of `f32`, used for both points and sizes.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vector2F {
    x: f32,
    y: f32,
}

/// Shorthand for [`Vector2F::new`], matching Warp's spelling at call sites.
pub const fn vec2f(x: f32, y: f32) -> Vector2F {
    Vector2F::new(x, y)
}

impl Vector2F {
    /// Constructs a vector from its components.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// The zero vector, which doubles as the origin and as an empty size.
    pub const fn zero() -> Self {
        Self::new(0., 0.)
    }

    /// A vector with both components set to `value`.
    pub const fn splat(value: f32) -> Self {
        Self::new(value, value)
    }

    /// The horizontal component.
    pub const fn x(self) -> f32 {
        self.x
    }

    /// The vertical component.
    pub const fn y(self) -> f32 {
        self.y
    }

    /// Overwrites the horizontal component in place.
    pub const fn set_x(&mut self, x: f32) {
        self.x = x;
    }

    /// Overwrites the vertical component in place.
    pub const fn set_y(&mut self, y: f32) {
        self.y = y;
    }

    /// The component-wise minimum of two vectors.
    pub fn min(self, other: Self) -> Self {
        Self::new(self.x.min(other.x), self.y.min(other.y))
    }

    /// The component-wise maximum of two vectors.
    pub fn max(self, other: Self) -> Self {
        Self::new(self.x.max(other.x), self.y.max(other.y))
    }

    /// The component-wise clamp of this vector into `[min, max]`.
    pub fn clamp(self, min: Self, max: Self) -> Self {
        self.max(min).min(max)
    }

    /// Multiplies both components by `factor`.
    pub fn scale(self, factor: f32) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }

    /// Divides both components by `divisor`.
    pub fn scale_down(self, divisor: f32) -> Self {
        Self::new(self.x / divisor, self.y / divisor)
    }

    /// Whether both components are finite, i.e. neither infinite nor NaN.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl Add for Vector2F {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl AddAssign for Vector2F {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Vector2F {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

impl SubAssign for Vector2F {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

/// Component-wise multiplication, which [`crate::elements::Align`] uses to turn
/// an alignment in `[-1, 1]²` into an offset from a center point.
impl Mul for Vector2F {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        Self::new(self.x * other.x, self.y * other.y)
    }
}

impl Mul<f32> for Vector2F {
    type Output = Self;

    fn mul(self, factor: f32) -> Self {
        self.scale(factor)
    }
}

impl Div<f32> for Vector2F {
    type Output = Self;

    fn div(self, divisor: f32) -> Self {
        self.scale_down(divisor)
    }
}

impl Neg for Vector2F {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

/// An axis-aligned rectangle, stored as an origin plus a size.
///
/// The origin is the upper-left corner: y grows downward, as it does in every
/// windowing system Crook targets.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct RectF {
    origin: Vector2F,
    size: Vector2F,
}

impl RectF {
    /// Constructs a rectangle from its upper-left corner and its size.
    pub const fn new(origin: Vector2F, size: Vector2F) -> Self {
        Self { origin, size }
    }

    /// Constructs a rectangle spanning from `upper_left` to `lower_right`.
    pub fn from_points(upper_left: Vector2F, lower_right: Vector2F) -> Self {
        Self::new(upper_left, lower_right - upper_left)
    }

    /// The upper-left corner.
    pub const fn origin(self) -> Vector2F {
        self.origin
    }

    /// The width and height.
    pub const fn size(self) -> Vector2F {
        self.size
    }

    /// The width.
    pub const fn width(self) -> f32 {
        self.size.x()
    }

    /// The height.
    pub const fn height(self) -> f32 {
        self.size.y()
    }

    /// The left edge.
    pub const fn min_x(self) -> f32 {
        self.origin.x()
    }

    /// The top edge.
    pub const fn min_y(self) -> f32 {
        self.origin.y()
    }

    /// The right edge.
    pub fn max_x(self) -> f32 {
        self.origin.x() + self.size.x()
    }

    /// The bottom edge.
    pub fn max_y(self) -> f32 {
        self.origin.y() + self.size.y()
    }

    /// The lower-right corner.
    pub fn lower_right(self) -> Vector2F {
        self.origin + self.size
    }

    /// The upper-right corner.
    pub fn upper_right(self) -> Vector2F {
        vec2f(self.max_x(), self.min_y())
    }

    /// Whether the rectangle has zero (or negative) area.
    pub fn is_empty(self) -> bool {
        self.size.x() <= 0. || self.size.y() <= 0.
    }

    /// Whether `point` falls inside the rectangle, right and bottom exclusive.
    pub fn contains_point(self, point: Vector2F) -> bool {
        point.x() >= self.min_x()
            && point.x() < self.max_x()
            && point.y() >= self.min_y()
            && point.y() < self.max_y()
    }

    /// The overlap between two rectangles, or `None` when they are disjoint.
    pub fn intersection(self, other: Self) -> Option<Self> {
        let upper_left = self.origin.max(other.origin);
        let lower_right = self.lower_right().min(other.lower_right());
        if upper_left.x() >= lower_right.x() || upper_left.y() >= lower_right.y() {
            None
        } else {
            Some(Self::from_points(upper_left, lower_right))
        }
    }
}

/// The index of the [`crate::scene::Layer`] an element painted into.
///
/// Depth is not a number an element chooses; it is the order in which layers
/// were started, so hit testing can ask "is any *later* layer covering me?".
/// Warp additionally distinguishes overlay layers that sort after every normal
/// one; Crook has no overlay-producing element, so a single counter suffices.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZIndex(pub usize);

/// A painted position, tagged with the layer it landed in.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Point {
    xy: Vector2F,
    z_index: ZIndex,
}

impl Point {
    /// Constructs a point from a position and the layer it was painted into.
    pub const fn from_vec2f(xy: Vector2F, z_index: ZIndex) -> Self {
        Self { xy, z_index }
    }

    /// The horizontal position.
    pub const fn x(self) -> f32 {
        self.xy.x()
    }

    /// The vertical position.
    pub const fn y(self) -> f32 {
        self.xy.y()
    }

    /// The position.
    pub const fn xy(self) -> Vector2F {
        self.xy
    }

    /// The layer this point was painted into.
    pub const fn z_index(self) -> ZIndex {
        self.z_index
    }
}

/// The two directions layout can run in.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Axis {
    /// Left to right.
    Horizontal,
    /// Top to bottom.
    Vertical,
}

impl Axis {
    /// The other axis — a flex's cross axis is its main axis inverted.
    pub fn invert(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }

    /// Builds a vector from a coordinate along this axis and one across it.
    pub fn to_point(self, along: f32, across: f32) -> Vector2F {
        match self {
            Self::Horizontal => vec2f(along, across),
            Self::Vertical => vec2f(across, along),
        }
    }
}

/// Axis-relative accessors for [`Vector2F`].
pub trait Vector2FExt {
    /// The component along `axis`.
    fn along(self, axis: Axis) -> f32;

    /// This vector with the component across `axis` zeroed.
    fn project_onto(self, axis: Axis) -> Vector2F;
}

impl Vector2FExt for Vector2F {
    fn along(self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.x(),
            Axis::Vertical => self.y(),
        }
    }

    fn project_onto(self, axis: Axis) -> Vector2F {
        match axis {
            Axis::Horizontal => vec2f(self.x(), 0.),
            Axis::Vertical => vec2f(0., self.y()),
        }
    }
}

/// Axis-relative promotion of a scalar to a [`Vector2F`].
pub trait F32Ext {
    /// A vector with this value along `axis` and zero across it.
    fn along(self, axis: Axis) -> Vector2F;
}

impl F32Ext for f32 {
    fn along(self, axis: Axis) -> Vector2F {
        match axis {
            Axis::Horizontal => vec2f(self, 0.),
            Axis::Vertical => vec2f(0., self),
        }
    }
}

/// Axis-relative accessors for [`RectF`].
pub trait RectFExt {
    /// The near edge along `axis`.
    fn min_along(self, axis: Axis) -> f32;

    /// The far edge along `axis`.
    fn max_along(self, axis: Axis) -> f32;
}

impl RectFExt for RectF {
    fn min_along(self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.min_x(),
            Axis::Vertical => self.min_y(),
        }
    }

    fn max_along(self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.max_x(),
            Axis::Vertical => self.max_y(),
        }
    }
}

/// A non-premultiplied 8-bit-per-channel RGBA color.
///
/// This is Warp's `pathfinder_color::ColorU` under a name Crook owns.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red, 0-255.
    pub r: u8,
    /// Green, 0-255.
    pub g: u8,
    /// Blue, 0-255.
    pub b: u8,
    /// Alpha, 0 (fully transparent) to 255 (fully opaque).
    pub a: u8,
}

impl Color {
    /// Fully transparent black — the identity for "nothing painted here".
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);

    /// Opaque black.
    pub const BLACK: Self = Self::rgb(0, 0, 0);

    /// Opaque white.
    pub const WHITE: Self = Self::rgb(255, 255, 255);

    /// Constructs a color from all four channels.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Constructs an opaque color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }

    /// Constructs an opaque color from a packed `0xRRGGBB` literal, so theme
    /// tables can be written the way designers hand them over.
    pub const fn hex(hex: u32) -> Self {
        Self::rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
    }

    /// This color at a different opacity.
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// The four channels as `0.0..=1.0` floats, in the order a GPU wants them.
    pub fn to_f32_array(self) -> [f32; 4] {
        [
            self.r as f32 / 255.,
            self.g as f32 / 255.,
            self.b as f32 / 255.,
            self.a as f32 / 255.,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_of_disjoint_rects_is_none() {
        let left = RectF::new(vec2f(0., 0.), vec2f(10., 10.));
        let right = RectF::new(vec2f(20., 0.), vec2f(10., 10.));
        assert_eq!(left.intersection(right), None);
    }

    #[test]
    fn intersection_clips_to_the_overlap() {
        let outer = RectF::new(vec2f(0., 0.), vec2f(10., 10.));
        let inner = RectF::new(vec2f(5., 5.), vec2f(10., 10.));
        assert_eq!(
            outer.intersection(inner),
            Some(RectF::new(vec2f(5., 5.), vec2f(5., 5.)))
        );
    }

    #[test]
    fn hex_unpacks_channels_in_designer_order() {
        assert_eq!(Color::hex(0x11_22_33), Color::rgb(0x11, 0x22, 0x33));
    }
}
