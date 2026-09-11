//! One picture, at a logical size of the caller's choosing.

use std::sync::Arc;

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Point, RectF, Vector2F, vec2f};
use crate::image::Bitmap;
use crate::presenter::{EventContext, LayoutContext, PaintContext};

/// Draws one bitmap, scaled to fit the room it is given.
///
/// A picture's pixels say nothing about how big it is on screen: a 128 px
/// icon is drawn twelve logical pixels wide in a list row and eighteen beside
/// a title, and a screenshot captured on a 2x display is half its pixel count
/// across. So the caller names the *logical* size, the renderer multiplies it
/// by the scale factor exactly once like every other primitive, and the
/// pixels are resampled to that device size on the CPU. Layout never sees the
/// scale factor and never sees the pixel count — the same tree lays out the
/// same way on every display.
///
/// Given less room than that, the picture shrinks to fit, keeping its aspect;
/// it never grows past its logical size, so a wide column does not blow a
/// small icon up. It is a *leaf* — no padding, no background, no hit rect —
/// wrapped in a [`Container`](super::Container) or a
/// [`Hoverable`](super::Hoverable) when it should be clicked, like an
/// [`Icon`](super::Icon).
pub struct Image {
    bitmap: Arc<Bitmap>,
    logical: Vector2F,
    fitted: Option<Vector2F>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Image {
    /// `bitmap` drawn `logical` pixels wide and tall, or as large as fits.
    pub fn new(bitmap: Arc<Bitmap>, logical: Vector2F) -> Self {
        Self {
            bitmap,
            logical,
            fitted: None,
            size: None,
            origin: None,
        }
    }
}

impl Element for Image {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        // An infinite axis is a flex asking how big the picture would like to
        // be, and the answer is its logical size — nothing here may divide by
        // infinity. A finite axis that is too small shrinks both axes by the
        // same factor, and a second axis still too small shrinks them again.
        // The tight axis takes the maximum exactly, and the other is a product
        // before a quotient, so 340 at three fifths is 204 and not a hair over.
        let mut fitted = self.logical;
        let (max_x, max_y) = (constraint.max.x(), constraint.max.y());
        if max_x.is_finite() && max_x < fitted.x() && fitted.x() > 0. {
            fitted = vec2f(max_x, fitted.y() * max_x / fitted.x());
        }
        if max_y.is_finite() && max_y < fitted.y() && fitted.y() > 0. {
            fitted = vec2f(fitted.x() * max_y / fitted.y(), max_y);
        }
        self.fitted = Some(fitted);

        // A stretched constraint hands back a box wider than the picture; the
        // picture keeps its fitted size inside it rather than stretching.
        let size = constraint.apply(fitted);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        let fitted = self
            .fitted
            .expect("an image was painted before it was laid out");
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));

        // Top-left of the box, not centred: a figure sits on the measure's
        // left edge like every line of text above and below it.
        ctx.scene
            .draw_image(self.bitmap.clone(), RectF::new(origin, fitted));
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
