//! The retained draw list a frame is compiled into.
//!
//! Painting an element tree produces a `Scene`: a stack of [`Layer`]s, each
//! holding flat vectors of rectangles and glyph references. Nothing here knows
//! about a GPU — the renderer walks the layers in order, sets a scissor from
//! each layer's clip bounds, and issues one instanced draw per primitive kind.
//! That separation is what lets the element tree be tested without a device.
//!
//! Glyphs are stored as *references* ([`crate::fonts::GlyphKey`] plus a
//! position), never as pixels. The renderer resolves them against its atlas.

use crate::fonts::{FontId, GlyphId, GlyphKey};
use crate::geometry::{Color, Point, RectF, Vector2F, ZIndex};

/// One frame's worth of draw commands.
#[derive(Clone)]
pub struct Scene {
    scale_factor: f32,
    active_layer_index_stack: Vec<ZIndex>,
    layers: Vec<Layer>,
}

/// A clipping region, painted in one scissor rect.
#[derive(Clone, Default)]
pub struct Layer {
    /// Rectangles registered for hit testing, in paint order.
    ///
    /// Warp indexes these in an R-tree; a linear scan is faster below a few
    /// hundred rects and costs no dependency. A tab bar and a chip are two
    /// dozen. Revisit when a terminal grid starts registering per-cell rects.
    hit_map: Vec<RectF>,

    /// The scissor rect for this layer, or `None` for "do not clip".
    pub clip_bounds: Option<RectF>,

    /// Rectangles to fill, in paint order.
    pub rects: Vec<Rect>,

    /// Glyphs to draw, in paint order. Always painted after this layer's rects.
    pub glyphs: Vec<Glyph>,
}

impl Layer {
    fn record_hit_rect(&mut self, rect: RectF) {
        if let Some(visible) = self
            .clip_bounds
            .map_or(Some(rect), |c| rect.intersection(c))
        {
            self.hit_map.push(visible);
        }
    }

    fn contains_point(&self, point: Vector2F) -> bool {
        self.hit_map.iter().any(|rect| rect.contains_point(point))
    }
}

/// How to derive a new layer's clip bounds when starting it.
pub enum ClipBounds {
    /// Inherit the clip of the layer being painted into.
    ActiveLayer,
    /// Clip to exactly these bounds, ignoring the active layer's clip.
    BoundedBy(RectF),
    /// Clip to the intersection of the active layer's clip and these bounds.
    BoundedByActiveLayerAnd(RectF),
    /// Do not clip.
    None,
}

/// A filled, optionally bordered and rounded rectangle.
///
/// Rounded corners, per-side borders and gradients are all evaluated in the
/// renderer's fragment shader, so this stays one instanced quad.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rect {
    /// Where it goes, in logical pixels.
    pub bounds: RectF,
    /// Which corners are rounded, and by how much.
    pub corner_radius: CornerRadius,
    /// The fill inside the border.
    pub background: Fill,
    /// The stroke, inset into `bounds`.
    pub border: Border,
}

impl Rect {
    /// Sets the fill.
    pub fn with_background(&mut self, background: impl Into<Fill>) -> &mut Self {
        self.background = background.into();
        self
    }

    /// Sets the border.
    pub fn with_border(&mut self, border: Border) -> &mut Self {
        self.border = border;
        self
    }

    /// Merges corner radii into this rect's, with `radius` winning per corner.
    pub fn with_corner_radius(&mut self, radius: CornerRadius) -> &mut Self {
        self.corner_radius.merge(radius);
        self
    }
}

/// One glyph, positioned where its left edge meets the baseline.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    /// Which glyph, from which face, at which size.
    pub glyph_key: GlyphKey,
    /// Left edge on the baseline, in logical pixels.
    pub position: Vector2F,
    /// Tint for monochrome glyphs; ignored for color ones.
    pub color: Color,
}

/// How to fill a region.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum Fill {
    /// Paint nothing.
    #[default]
    None,
    /// One flat color.
    Solid(Color),
    /// A linear gradient between two points given in unit rect coordinates.
    Gradient {
        /// Gradient start, `(0,0)`-`(1,1)` relative to the rect.
        start: Vector2F,
        /// Gradient end, `(0,0)`-`(1,1)` relative to the rect.
        end: Vector2F,
        /// Color at `start`.
        start_color: Color,
        /// Color at `end`.
        end_color: Color,
    },
}

impl From<Color> for Fill {
    fn from(color: Color) -> Self {
        Fill::Solid(color)
    }
}

impl Fill {
    /// The gradient start, or the rect's upper-left for non-gradients.
    pub fn start(self) -> Vector2F {
        match self {
            Self::Gradient { start, .. } => start,
            _ => Vector2F::zero(),
        }
    }

    /// The gradient end, or the rect's upper-right for non-gradients.
    pub fn end(self) -> Vector2F {
        match self {
            Self::Gradient { end, .. } => end,
            _ => crate::geometry::vec2f(1., 0.),
        }
    }

    /// The color at the start of the fill.
    pub fn start_color(self) -> Color {
        match self {
            Self::Gradient { start_color, .. } => start_color,
            Self::Solid(color) => color,
            Self::None => Color::TRANSPARENT,
        }
    }

    /// The color at the end of the fill.
    pub fn end_color(self) -> Color {
        match self {
            Self::Gradient { end_color, .. } => end_color,
            Self::Solid(color) => color,
            Self::None => Color::TRANSPARENT,
        }
    }
}

/// A stroke around a [`Rect`], drawn inside its bounds and only on the enabled
/// sides.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Border {
    /// Stroke width in logical pixels.
    pub width: f32,
    /// Stroke fill.
    pub color: Fill,
    /// Whether to stroke the top edge.
    pub top: bool,
    /// Whether to stroke the left edge.
    pub left: bool,
    /// Whether to stroke the bottom edge.
    pub bottom: bool,
    /// Whether to stroke the right edge.
    pub right: bool,
}

impl Border {
    /// A border of the given width with no sides enabled yet.
    pub const fn new(width: f32) -> Self {
        Self {
            width,
            color: Fill::None,
            top: false,
            left: false,
            bottom: false,
            right: false,
        }
    }

    /// A border on all four sides.
    pub const fn all(width: f32) -> Self {
        Self {
            top: true,
            left: true,
            bottom: true,
            right: true,
            ..Self::new(width)
        }
    }

    /// A border on the top edge only.
    pub const fn top(width: f32) -> Self {
        Self {
            top: true,
            ..Self::new(width)
        }
    }

    /// A border on the left edge only.
    pub const fn left(width: f32) -> Self {
        Self {
            left: true,
            ..Self::new(width)
        }
    }

    /// A border on the bottom edge only.
    pub const fn bottom(width: f32) -> Self {
        Self {
            bottom: true,
            ..Self::new(width)
        }
    }

    /// A border on the right edge only.
    pub const fn right(width: f32) -> Self {
        Self {
            right: true,
            ..Self::new(width)
        }
    }

    /// Enables an explicit set of sides.
    pub const fn with_sides(mut self, top: bool, left: bool, bottom: bool, right: bool) -> Self {
        self.top = top;
        self.left = left;
        self.bottom = bottom;
        self.right = right;
        self
    }

    /// Strokes in a flat color.
    pub const fn with_border_color(mut self, color: Color) -> Self {
        self.color = Fill::Solid(color);
        self
    }

    /// Strokes with an arbitrary fill.
    pub fn with_border_fill(mut self, fill: impl Into<Fill>) -> Self {
        self.color = fill.into();
        self
    }

    /// The width of the top stroke, zero when that side is disabled.
    pub fn top_width(self) -> f32 {
        if self.top { self.width } else { 0. }
    }

    /// The width of the left stroke, zero when that side is disabled.
    pub fn left_width(self) -> f32 {
        if self.left { self.width } else { 0. }
    }

    /// The width of the bottom stroke, zero when that side is disabled.
    pub fn bottom_width(self) -> f32 {
        if self.bottom { self.width } else { 0. }
    }

    /// The width of the right stroke, zero when that side is disabled.
    pub fn right_width(self) -> f32 {
        if self.right { self.width } else { 0. }
    }
}

impl From<Color> for Border {
    fn from(color: Color) -> Self {
        Border::all(1.).with_border_color(color)
    }
}

/// How much to round one corner.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Radius {
    /// An absolute radius in logical pixels.
    Pixels(f32),
    /// A percentage of the rectangle's shorter side. `Percentage(50.)` is a
    /// pill, which is what makes the usage chip a chip.
    Percentage(f32),
}

impl Default for Radius {
    fn default() -> Self {
        Self::Pixels(0.)
    }
}

/// The four corner radii of a [`Rect`].
///
/// A corner left as `None` is square, and stays square through a [`merge`]
/// with a value that sets it — which is what lets a caller say "round the top,
/// and separately round the bottom right".
///
/// [`merge`]: CornerRadius::merge
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct CornerRadius {
    top_left: Option<Radius>,
    top_right: Option<Radius>,
    bottom_left: Option<Radius>,
    bottom_right: Option<Radius>,
}

impl CornerRadius {
    /// Rounds all four corners.
    pub const fn with_all(radius: Radius) -> Self {
        Self {
            top_left: Some(radius),
            top_right: Some(radius),
            bottom_left: Some(radius),
            bottom_right: Some(radius),
        }
    }

    /// Rounds the two top corners.
    pub const fn with_top(radius: Radius) -> Self {
        Self {
            top_left: Some(radius),
            top_right: Some(radius),
            bottom_left: None,
            bottom_right: None,
        }
    }

    /// Rounds the two bottom corners.
    pub const fn with_bottom(radius: Radius) -> Self {
        Self {
            top_left: None,
            top_right: None,
            bottom_left: Some(radius),
            bottom_right: Some(radius),
        }
    }

    /// Rounds the two left corners.
    pub const fn with_left(radius: Radius) -> Self {
        Self {
            top_left: Some(radius),
            top_right: None,
            bottom_left: Some(radius),
            bottom_right: None,
        }
    }

    /// Rounds the two right corners.
    pub const fn with_right(radius: Radius) -> Self {
        Self {
            top_left: None,
            top_right: Some(radius),
            bottom_left: None,
            bottom_right: Some(radius),
        }
    }

    /// Rounds the top-left corner.
    pub const fn with_top_left(radius: Radius) -> Self {
        Self {
            top_left: Some(radius),
            top_right: None,
            bottom_left: None,
            bottom_right: None,
        }
    }

    /// Rounds the top-right corner.
    pub const fn with_top_right(radius: Radius) -> Self {
        Self {
            top_left: None,
            top_right: Some(radius),
            bottom_left: None,
            bottom_right: None,
        }
    }

    /// Rounds the bottom-left corner.
    pub const fn with_bottom_left(radius: Radius) -> Self {
        Self {
            top_left: None,
            top_right: None,
            bottom_left: Some(radius),
            bottom_right: None,
        }
    }

    /// Rounds the bottom-right corner.
    pub const fn with_bottom_right(radius: Radius) -> Self {
        Self {
            top_left: None,
            top_right: None,
            bottom_left: None,
            bottom_right: Some(radius),
        }
    }

    /// Overlays `other` onto this, with `other`'s set corners winning.
    pub fn merge(&mut self, other: Self) {
        self.top_left = other.top_left.or(self.top_left);
        self.top_right = other.top_right.or(self.top_right);
        self.bottom_left = other.bottom_left.or(self.bottom_left);
        self.bottom_right = other.bottom_right.or(self.bottom_right);
    }

    /// The top-left radius, square if unset.
    pub fn get_top_left(self) -> Radius {
        self.top_left.unwrap_or_default()
    }

    /// The top-right radius, square if unset.
    pub fn get_top_right(self) -> Radius {
        self.top_right.unwrap_or_default()
    }

    /// The bottom-left radius, square if unset.
    pub fn get_bottom_left(self) -> Radius {
        self.bottom_left.unwrap_or_default()
    }

    /// The bottom-right radius, square if unset.
    pub fn get_bottom_right(self) -> Radius {
        self.bottom_right.unwrap_or_default()
    }
}

impl Scene {
    /// An empty scene with one unclipped root layer.
    pub fn new(scale_factor: f32) -> Self {
        Self {
            scale_factor,
            active_layer_index_stack: vec![ZIndex(0)],
            layers: vec![Layer::default()],
        }
    }

    /// Device pixels per logical pixel. Layout never sees this; the renderer
    /// multiplies every coordinate by it.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// The layer currently being painted into.
    pub fn z_index(&self) -> ZIndex {
        *self
            .active_layer_index_stack
            .last()
            .expect("the root layer is never popped")
    }

    /// The topmost layer that exists so far.
    ///
    /// An element that paints children into layers above its own hit-tests
    /// against this rather than [`Self::z_index`], so a click that landed on a
    /// child still reaches the parent that wrapped it.
    pub fn max_active_z_index(&self) -> ZIndex {
        ZIndex(self.layers.len() - 1)
    }

    /// Pushes a new layer and makes it active.
    pub fn start_layer(&mut self, bounds: ClipBounds) {
        let clip_bounds = match bounds {
            ClipBounds::ActiveLayer => self.active_layer().clip_bounds,
            ClipBounds::BoundedBy(bounds) => Some(bounds),
            ClipBounds::BoundedByActiveLayerAnd(bounds) => match self.active_layer().clip_bounds {
                // Non-overlapping clips must produce an empty rect rather than
                // no clip at all, or the layer would paint over everything.
                Some(active) => active.intersection(bounds).or(Some(RectF::default())),
                None => Some(bounds),
            },
            ClipBounds::None => None,
        };

        self.active_layer_index_stack
            .push(ZIndex(self.layers.len()));
        self.layers.push(Layer {
            clip_bounds,
            ..Default::default()
        });
    }

    /// Pops back to the enclosing layer.
    ///
    /// Layers are never removed from the scene, only deactivated: their index
    /// is their z order, so reusing an index would reorder the frame.
    pub fn stop_layer(&mut self) {
        self.active_layer_index_stack.pop();
        assert!(
            !self.active_layer_index_stack.is_empty(),
            "popped the root layer; every start_layer needs exactly one stop_layer"
        );
    }

    /// Adds a rectangle and registers it for hit testing.
    pub fn draw_rect_with_hit_recording(&mut self, rect: RectF) -> &mut Rect {
        self.active_layer().record_hit_rect(rect);
        self.draw_rect_without_hit_recording(rect)
    }

    /// Adds a rectangle that will never be hit-tested.
    ///
    /// Use this only for decoration drawn inside an area some other rect has
    /// already registered; a region that is only ever drawn this way is
    /// invisible to [`Self::is_covered`] and therefore transparent to clicks.
    pub fn draw_rect_without_hit_recording(&mut self, rect: RectF) -> &mut Rect {
        debug_assert!(
            rect.origin().is_finite() && rect.size().is_finite(),
            "a rect reached the scene with a non-finite coordinate: {rect:?}"
        );

        let layer = self.active_layer();
        layer.rects.push(Rect {
            bounds: rect,
            ..Default::default()
        });
        layer.rects.last_mut().expect("just pushed")
    }

    /// Adds a glyph, positioned where its left edge meets the baseline.
    ///
    /// Glyphs record no hit rect, so text alone is never clickable: wrap a
    /// label in a [`crate::elements::Container`] or a
    /// [`crate::elements::Hoverable`] to give it a hit area.
    pub fn draw_glyph(
        &mut self,
        position: Vector2F,
        glyph_id: GlyphId,
        font_id: FontId,
        font_size: f32,
        color: Color,
    ) -> &mut Glyph {
        let layer = self.active_layer();
        layer.glyphs.push(Glyph {
            glyph_key: GlyphKey {
                glyph_id,
                font_id,
                font_size,
            },
            position,
            color,
        });
        layer.glyphs.last_mut().expect("just pushed")
    }

    /// Whether anything in a layer above `position`'s own covers it.
    ///
    /// This is the whole of hit testing: an element asks whether the point it
    /// was clicked at is still visible from where it painted.
    pub fn is_covered(&self, position: Point) -> bool {
        self.layers
            .get((position.z_index().0 + 1)..)
            .into_iter()
            .flatten()
            .any(|layer| layer.contains_point(position.xy()))
    }

    /// The part of a rect that its own layer's clip leaves visible, or `None`
    /// when the clip hides it entirely.
    pub fn visible_rect(&self, origin: Point, size: Vector2F) -> Option<RectF> {
        let rect = RectF::new(origin.xy(), size);
        match self
            .layers
            .get(origin.z_index().0)
            .and_then(|l| l.clip_bounds)
        {
            Some(clip) => clip.intersection(rect),
            None => Some(rect),
        }
    }

    /// Every layer, bottom to top: paint order and z order are the same thing.
    pub fn layers(&self) -> impl Iterator<Item = &Layer> {
        self.layers.iter()
    }

    /// How many layers this frame ended up with.
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    fn active_layer(&mut self) -> &mut Layer {
        let ZIndex(index) = self.z_index();
        &mut self.layers[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::vec2f;

    #[test]
    fn a_rect_in_a_later_layer_covers_an_earlier_one() {
        let mut scene = Scene::new(1.);
        let below = Point::from_vec2f(vec2f(5., 5.), scene.z_index());
        scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(10., 10.)));

        assert!(!scene.is_covered(below));

        scene.start_layer(ClipBounds::None);
        scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(10., 10.)));
        scene.stop_layer();

        assert!(scene.is_covered(below));
        assert!(
            !scene.is_covered(Point::from_vec2f(vec2f(50., 50.), ZIndex(0))),
            "a point outside every rect is not covered"
        );
    }

    #[test]
    fn a_layer_clip_bounds_the_visible_rect() {
        let mut scene = Scene::new(1.);
        scene.start_layer(ClipBounds::BoundedBy(RectF::new(
            vec2f(0., 0.),
            vec2f(10., 10.),
        )));
        let origin = Point::from_vec2f(vec2f(5., 5.), scene.z_index());
        scene.stop_layer();

        assert_eq!(
            scene.visible_rect(origin, vec2f(100., 100.)),
            Some(RectF::new(vec2f(5., 5.), vec2f(5., 5.)))
        );
    }

    #[test]
    fn rects_drawn_without_hit_recording_are_click_through() {
        let mut scene = Scene::new(1.);
        scene.draw_rect_without_hit_recording(RectF::new(vec2f(0., 0.), vec2f(10., 10.)));
        scene.start_layer(ClipBounds::None);
        scene.stop_layer();

        assert!(!scene.is_covered(Point::from_vec2f(vec2f(5., 5.), ZIndex(0))));
    }
}
