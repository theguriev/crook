//! One icon, from the set in [`crate::icons`].

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Color, Point, RectF, Vector2F, vec2f};
use crate::icons::{IconKey, Lucide};
use crate::presenter::{EventContext, LayoutContext, PaintContext};

/// Draws one icon in a square of its own.
///
/// The square is `size` on both edges, which is what an icon is: Lucide draws
/// everything in a 24 x 24 box, so a 16px icon is that box at two thirds. It
/// is a *leaf* — no padding, no background, no hit rect — because a button is
/// this inside a [`Container`](super::Container) or a
/// [`Hoverable`](super::Hoverable), and an icon that carried its own box would
/// be a second way to say what those already say.
///
/// Given a constraint too tight to fit the square, it takes the size it is
/// allowed and draws the icon centred in the largest square that fits, rather
/// than stretching it into a rectangle.
pub struct Icon {
    key: IconKey,
    color: Color,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Icon {
    /// `icon` in a `size` x `size` square, with Lucide's own stroke width.
    pub fn new(icon: Lucide, size: f32) -> Self {
        Self {
            key: IconKey::new(icon, size),
            color: Color::WHITE,
            size: None,
            origin: None,
        }
    }

    /// Sets the tint.
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Sets the stroke width, in Lucide's 24-unit grid.
    ///
    /// [`STROKE_WIDTH`](crate::icons::STROKE_WIDTH) is what every icon is drawn with by default and what
    /// they are designed at. Worth raising only where an icon is small enough
    /// that a scaled-down stroke stops reading — the 12px info dot — and never
    /// by enough to make one icon look heavier than its neighbours.
    pub fn with_stroke_width(mut self, stroke_width: f32) -> Self {
        self.key.stroke_width = stroke_width;
        self
    }
}

impl Element for Icon {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        let size = constraint.apply(Vector2F::splat(self.key.size));
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        let size = self
            .size
            .expect("an icon was painted before it was laid out");
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));

        let edge = self.key.size.min(size.x()).min(size.y()).max(0.);
        let inset = (size - Vector2F::splat(edge)) * 0.5;
        let square = RectF::new(origin + inset, vec2f(edge, edge));

        // A key that has been squeezed asks the rasterizer for the size it
        // will actually be drawn at, so the mask is never scaled by the
        // sampler — the whole point of rasterizing per size.
        let key = IconKey {
            size: edge,
            ..self.key
        };
        ctx.scene.draw_icon(key, square, self.color);
    }

    fn dispatch_event(
        &mut self,
        _: &DispatchedEvent,
        _: &mut EventContext,
        _: &AppContext,
    ) -> bool {
        false
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
