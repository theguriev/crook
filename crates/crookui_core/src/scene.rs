//! The retained draw list a frame is compiled into.
//!
//! Painting an element tree produces a `Scene`: a stack of [`Layer`]s, each
//! holding flat vectors of rectangles and glyph references. Nothing here knows
//! about a GPU — the renderer walks the layers in order, sets a scissor from
//! each layer's clip bounds, and issues one instanced draw per primitive kind.
//! That separation is what lets the element tree be tested without a device.
//!
//! There are two layer lists, not one. A layer started with
//! [`Scene::start_overlay_layer`] joins the second, and the frame is the first
//! list followed by the second — so a menu emitted halfway through the tree
//! still paints, and hit-tests, above everything painted after it. There is no
//! depth value and no sorting anywhere: paint order is z order is list order.
//!
//! Glyphs are stored as *references* ([`crate::fonts::GlyphKey`] plus a
//! position), never as pixels. The renderer resolves them against its atlas.

use crate::fonts::{FontId, GlyphId, GlyphKey};
use crate::geometry::{Color, Point, RectF, Vector2F, ZIndex};
use crate::icons::IconKey;

/// One frame's worth of draw commands.
#[derive(Clone)]
pub struct Scene {
    scale_factor: f32,
    active_layer_index_stack: Vec<ZIndex>,
    layers: Vec<Layer>,

    /// Painted, and hit-tested, after every layer in `layers`, whatever order
    /// the two lists were built in.
    overlay_layers: Vec<Layer>,
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

    /// Icons to draw, in paint order. Painted after this layer's glyphs, which
    /// matters only in the layer that draws one over the other and there is
    /// none: an icon sits beside a label, never on it.
    pub icons: Vec<Icon>,

    /// Whether this layer is invisible to hit testing.
    ///
    /// Set it on a layer that exists only to paint *over* something that must
    /// stay clickable — a hover tint drawn on top of a button, say. Without
    /// it the tint would cover the button and the button would decline every
    /// click that landed on it.
    pub click_through: bool,
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

/// One icon, filling a square.
///
/// The square is where it goes; the mask is rasterized to fit it and tinted
/// with `color`, exactly as a monochrome glyph is. An icon records no hit
/// rect for the same reason a glyph does not — wrap it to make it clickable.
#[derive(Clone, Debug, PartialEq)]
pub struct Icon {
    /// Which icon, at which size and stroke.
    pub icon_key: IconKey,
    /// Where it goes, in logical pixels. Square by construction.
    pub bounds: RectF,
    /// What to tint the mask with.
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
            active_layer_index_stack: vec![ZIndex::Normal(0)],
            layers: vec![Layer::default()],
            overlay_layers: Vec::new(),
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

    /// The topmost layer that exists so far, on the same list as the active
    /// one.
    ///
    /// An element that paints children into layers above its own hit-tests
    /// against this rather than [`Self::z_index`], so a click that landed on a
    /// child still reaches the parent that wrapped it.
    pub fn max_active_z_index(&self) -> ZIndex {
        match self.z_index() {
            ZIndex::Normal(_) => ZIndex::Normal(self.layers.len() - 1),
            ZIndex::Overlay(_) => ZIndex::Overlay(
                self.overlay_layers
                    .len()
                    .checked_sub(1)
                    // The active layer being an overlay means one was pushed,
                    // and layers are only ever deactivated, never removed.
                    .expect("an overlay layer is active, so the overlay list cannot be empty"),
            ),
        }
    }

    /// Pushes a new layer and makes it active.
    ///
    /// The new layer joins whichever list the active one belongs to. That is
    /// not an optimisation: it is what keeps a [`Clipped`] nested inside an
    /// overlay from dropping the rest of that overlay's subtree back down
    /// below every other overlay in the frame.
    ///
    /// [`Clipped`]: crate::elements::Clipped
    pub fn start_layer(&mut self, bounds: ClipBounds) {
        let layer = self.create_layer(bounds);
        match self.z_index() {
            ZIndex::Normal(_) => self.push_normal_layer(layer),
            ZIndex::Overlay(_) => self.push_overlay_layer(layer),
        }
    }

    /// Pushes a new layer that paints above every non-overlay layer in the
    /// frame, wherever in the tree it was started from, and makes it active.
    ///
    /// This is what a popup is: an overlay child of a
    /// [`Stack`](crate::elements::Stack) starts one unclipped, so a menu
    /// escapes both the z order and the scissor rect of the panel that spawned
    /// it.
    pub fn start_overlay_layer(&mut self, bounds: ClipBounds) {
        let layer = self.create_layer(bounds);
        self.push_overlay_layer(layer);
    }

    /// Makes the active layer invisible to hit testing.
    ///
    /// See [`Layer::click_through`] for when that is what you want.
    pub fn set_active_layer_click_through(&mut self) {
        self.active_layer().click_through = true;
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

    /// Adds an icon, filling the square `bounds`.
    ///
    /// The size the mask is rasterized at comes from the key rather than from
    /// `bounds`, so an icon squeezed by a tight constraint is drawn at the
    /// size it asked for and centred, rather than stretched.
    pub fn draw_icon(&mut self, icon_key: IconKey, bounds: RectF, color: Color) -> &mut Icon {
        debug_assert!(
            bounds.origin().is_finite() && bounds.size().is_finite(),
            "an icon reached the scene with a non-finite coordinate: {bounds:?}"
        );

        let layer = self.active_layer();
        layer.icons.push(Icon {
            icon_key,
            bounds,
            color,
        });
        layer.icons.last_mut().expect("just pushed")
    }

    /// Whether anything in a layer above `position`'s own covers it.
    ///
    /// This is the whole of hit testing: an element asks whether the point it
    /// was clicked at is still visible from where it painted. Nothing tells an
    /// element that a menu opened over it — it finds out by asking this.
    pub fn is_covered(&self, position: Point) -> bool {
        let covers = |layer: &Layer| !layer.click_through && layer.contains_point(position.xy());
        match position.z_index() {
            // Every overlay layer is above every normal one, including the
            // ones started before this point was painted.
            ZIndex::Normal(index) => self
                .layers
                .get((index + 1)..)
                .into_iter()
                .flatten()
                .chain(self.overlay_layers.iter())
                .any(covers),
            ZIndex::Overlay(index) => self
                .overlay_layers
                .get((index + 1)..)
                .into_iter()
                .flatten()
                .any(covers),
        }
    }

    /// The part of a rect that its own layer's clip leaves visible, or `None`
    /// when the clip hides it entirely.
    pub fn visible_rect(&self, origin: Point, size: Vector2F) -> Option<RectF> {
        let rect = RectF::new(origin.xy(), size);
        let layer = match origin.z_index() {
            ZIndex::Normal(index) => self.layers.get(index),
            ZIndex::Overlay(index) => self.overlay_layers.get(index),
        };
        match layer.and_then(|layer| layer.clip_bounds) {
            Some(clip) => clip.intersection(rect),
            None => Some(rect),
        }
    }

    /// Every layer, bottom to top: paint order and z order are the same thing.
    ///
    /// Normal layers first, then overlay ones. The renderer walks this twice —
    /// once to append instance data, once to draw index ranges into it — and
    /// the two walks agree only because they are the same iterator.
    pub fn layers(&self) -> impl Iterator<Item = &Layer> {
        self.layers.iter().chain(self.overlay_layers.iter())
    }

    /// How many layers this frame ended up with, overlays included.
    pub fn layer_count(&self) -> usize {
        self.layers.len() + self.overlay_layers.len()
    }

    fn create_layer(&mut self, bounds: ClipBounds) -> Layer {
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

        Layer {
            clip_bounds,
            ..Default::default()
        }
    }

    fn push_normal_layer(&mut self, layer: Layer) {
        self.active_layer_index_stack
            .push(ZIndex::Normal(self.layers.len()));
        self.layers.push(layer);
    }

    fn push_overlay_layer(&mut self, layer: Layer) {
        self.active_layer_index_stack
            .push(ZIndex::Overlay(self.overlay_layers.len()));
        self.overlay_layers.push(layer);
    }

    fn active_layer(&mut self) -> &mut Layer {
        match self.z_index() {
            ZIndex::Normal(index) => &mut self.layers[index],
            ZIndex::Overlay(index) => &mut self.overlay_layers[index],
        }
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
            !scene.is_covered(Point::from_vec2f(vec2f(50., 50.), ZIndex::Normal(0))),
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

        assert!(!scene.is_covered(Point::from_vec2f(vec2f(5., 5.), ZIndex::Normal(0))));
    }

    #[test]
    fn an_overlay_layer_covers_normal_layers_started_after_it() {
        let mut scene = Scene::new(1.);
        scene.start_overlay_layer(ClipBounds::None);
        scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(10., 10.)));
        scene.stop_layer();

        // Started later, and still underneath: this is the whole point of the
        // second list.
        scene.start_layer(ClipBounds::None);
        let later = Point::from_vec2f(vec2f(5., 5.), scene.z_index());
        scene.stop_layer();

        assert_eq!(scene.z_index(), ZIndex::Normal(0));
        assert!(scene.is_covered(later));
    }

    #[test]
    fn an_overlay_layer_is_covered_only_by_later_overlay_layers() {
        let mut scene = Scene::new(1.);
        scene.start_overlay_layer(ClipBounds::None);
        let below = Point::from_vec2f(vec2f(5., 5.), scene.z_index());
        scene.stop_layer();

        scene.start_overlay_layer(ClipBounds::None);
        let above = Point::from_vec2f(vec2f(5., 5.), scene.z_index());
        scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(10., 10.)));
        scene.stop_layer();

        assert!(scene.is_covered(below));
        assert!(!scene.is_covered(above), "a layer never covers itself");
    }

    #[test]
    fn a_layer_started_inside_an_overlay_stays_in_the_overlay_list() {
        let mut scene = Scene::new(1.);
        let normal = Point::from_vec2f(vec2f(5., 5.), scene.z_index());

        scene.start_overlay_layer(ClipBounds::None);
        // A clip, a hover tint, anything that starts a layer while a popup is
        // being painted: it must not fall back to normal depth, or the rest of
        // the popup's subtree would paint under the window it floats over.
        scene.start_layer(ClipBounds::ActiveLayer);
        assert_eq!(scene.z_index(), ZIndex::Overlay(1));
        scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(10., 10.)));
        scene.stop_layer();
        scene.stop_layer();

        assert!(scene.is_covered(normal));
    }

    #[test]
    fn a_click_through_layer_covers_nothing() {
        let mut scene = Scene::new(1.);
        let below = Point::from_vec2f(vec2f(5., 5.), scene.z_index());

        scene.start_overlay_layer(ClipBounds::None);
        scene.draw_rect_with_hit_recording(RectF::new(vec2f(0., 0.), vec2f(10., 10.)));
        scene.set_active_layer_click_through();
        scene.stop_layer();

        assert!(!scene.is_covered(below));
    }

    #[test]
    fn max_active_z_index_reports_the_top_of_the_active_list() {
        let mut scene = Scene::new(1.);
        scene.start_layer(ClipBounds::None);
        scene.stop_layer();
        assert_eq!(scene.max_active_z_index(), ZIndex::Normal(1));

        scene.start_overlay_layer(ClipBounds::None);
        scene.start_layer(ClipBounds::ActiveLayer);
        scene.stop_layer();
        assert_eq!(scene.max_active_z_index(), ZIndex::Overlay(1));
        scene.stop_layer();

        assert_eq!(
            scene.max_active_z_index(),
            ZIndex::Normal(1),
            "back in a normal layer, the overlays above are not the ceiling"
        );
    }

    #[test]
    fn layers_hands_the_renderer_normal_layers_first_and_overlays_last() {
        let mut scene = Scene::new(1.);
        scene.start_overlay_layer(ClipBounds::None);
        scene.draw_rect_without_hit_recording(RectF::new(vec2f(1., 0.), vec2f(1., 1.)));
        scene.stop_layer();

        // Started after the overlay, and drawn before it: the renderer appends
        // instance data in exactly this order and draws index ranges into it,
        // so this order *is* the frame.
        scene.start_layer(ClipBounds::None);
        scene.draw_rect_without_hit_recording(RectF::new(vec2f(0., 0.), vec2f(1., 1.)));
        scene.stop_layer();

        let origins: Vec<_> = scene
            .layers()
            .flat_map(|layer| layer.rects.iter())
            .map(|rect| rect.bounds.origin())
            .collect();

        assert_eq!(origins, [vec2f(0., 0.), vec2f(1., 0.)]);
        assert_eq!(scene.layer_count(), 3, "the root, one normal, one overlay");
    }
}
