//! A subtree taller than the box it was given, and a wheel that moves it.
//!
//! The twelfth primitive, and the first one added after the tab strip shipped.
//! It exists because two lists in Crook are already longer than their window —
//! the vertical tabs panel, whose ceiling is written down in
//! `app/src/workspace/tabs_panel`, and the settings page, which is a column of
//! sections that no window height can be relied on to hold.
//!
//! # What it is, and what it deliberately is not
//!
//! One axis: vertical. Warp's `scrollable` carries an axis parameter, elastic
//! overscroll, a drag-to-scroll thumb, autoscroll-to-child and a shared
//! scrollbar that several lists report into. None of that is reachable from
//! anything Crook draws today, and each piece is a state machine that has to
//! be right in three places — layout, paint and event — before it is right
//! anywhere. What is here is the part a settings page needs: the wheel moves
//! the content, the content is clipped to its box, and a thumb says how much
//! of it you are looking at.
//!
//! The thumb is an *indicator*, not a handle. It paints without hit recording,
//! so a press goes through it to whatever is underneath, and dragging it does
//! nothing. Giving it a drag means tracking a grab offset across frames — a
//! second piece of interaction state, in a second handle — and the wheel is
//! what people reach for. It is written down here so the omission reads as a
//! decision rather than as a half-finished control.
//!
//! # Where the offset lives
//!
//! In the view, like every other piece of interaction state, for the reason
//! [`Element`] gives: the tree is thrown away on every re-render, so an offset
//! stored in it would snap back to zero the first time scrolling caused a
//! repaint — which is every time. The view keeps a [`ScrollStateHandle`] and
//! hands it back each render, exactly as it does for
//! [`MouseState`](super::MouseState).
//!
//! The handle is also where *layout* reports what it learned. How far the
//! content can move is `content - viewport`, and neither number exists until
//! the child has been measured against a viewport that has itself been
//! decided. So layout writes both into the state and clamps the offset there,
//! which is what makes a window that has just been made taller scroll back up
//! on its own rather than sit on an offset that no longer has content under
//! it.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::{DispatchedEvent, Event};
use crate::geometry::{Color, Point, RectF, Vector2F, ZIndex, vec2f};
use crate::presenter::{EventContext, LayoutContext, PaintContext};
use crate::scene::{ClipBounds, CornerRadius, Radius};

/// How far one wheel click moves the content, in logical pixels.
///
/// A wheel reports lines rather than pixels ([`ScrollDelta::Lines`]), and an
/// element library that knows nothing about fonts has no line height to
/// convert them with. Twenty is three rows of 12px interface text per click:
/// enough that a long page is crossed in a few flicks, little enough that the
/// text does not jump past the eye.
///
/// [`ScrollDelta::Lines`]: crate::event::ScrollDelta::Lines
const PIXELS_PER_WHEEL_LINE: f32 = 20.;

/// How wide the thumb is painted.
const SCROLLBAR_WIDTH: f32 = 4.;

/// How far the thumb sits in from the right edge, and from the top and bottom.
const SCROLLBAR_INSET: f32 = 2.;

/// The shortest the thumb is allowed to get.
///
/// Proportional length alone gives a thousand-pixel page a two-pixel thumb,
/// which reads as a speck of dust rather than as a position.
const SCROLLBAR_MIN_LENGTH: f32 = 24.;

/// How far a [`Scrollable`] has been scrolled, and how far it can be.
///
/// The last two fields are written by layout and read by everything else: a
/// view that wants to know whether its list overflows asks
/// [`Self::is_scrollable`] rather than measuring anything itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollState {
    /// How far down the content has moved, in logical pixels. Never negative,
    /// never past [`Self::max_offset`].
    offset: f32,
    /// The height the child asked for at the last layout.
    content_height: f32,
    /// The height the box could give it.
    viewport_height: f32,
}

impl ScrollState {
    /// How far down the content is, in logical pixels.
    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// The largest offset that still has content under it.
    ///
    /// Zero when everything fits, which is what makes every other method here
    /// a no-op on a list that does not overflow.
    pub fn max_offset(&self) -> f32 {
        (self.content_height - self.viewport_height).max(0.)
    }

    /// Whether there is anything to scroll.
    pub fn is_scrollable(&self) -> bool {
        self.max_offset() > 0.
    }

    /// Moves the content by `delta` pixels, positive being further down, and
    /// says whether it actually moved.
    ///
    /// The answer is what stops a wheel at the end of a list from repainting
    /// the window on every click — and, more importantly, what lets a nested
    /// scrollable that has run out of room decline the event so the one around
    /// it can take it.
    pub fn scroll_by(&mut self, delta: f32) -> bool {
        self.scroll_to(self.offset + delta)
    }

    /// Puts the content at `offset`, clamped, and says whether that moved it.
    pub fn scroll_to(&mut self, offset: f32) -> bool {
        let clamped = offset.clamp(0., self.max_offset());
        if clamped == self.offset {
            return false;
        }
        self.offset = clamped;
        true
    }

    /// Back to the top. What opening a page fresh should do to the page under
    /// it.
    pub fn scroll_to_top(&mut self) -> bool {
        self.scroll_to(0.)
    }

    /// Records what layout measured, and clamps the offset into it.
    ///
    /// Called only by [`Scrollable::layout`]. Returns whether the clamp moved
    /// the content, which is a frame that has already been laid out against
    /// the old offset — see the call site for why that is allowed to stand for
    /// one frame.
    fn measured(&mut self, content_height: f32, viewport_height: f32) -> bool {
        self.content_height = content_height;
        self.viewport_height = viewport_height;
        let clamped = self.offset.clamp(0., self.max_offset());
        let moved = clamped != self.offset;
        self.offset = clamped;
        moved
    }

    /// Where the thumb goes inside a track of `height` pixels, as a start and
    /// a length, or `None` when there is nothing to scroll.
    fn thumb(&self, height: f32) -> Option<(f32, f32)> {
        let max_offset = self.max_offset();
        if max_offset <= 0. || height <= 0. {
            return None;
        }

        // The fraction of the content on screen, floored at a thumb that can
        // still be seen. A `min` against the track keeps the floor from
        // producing a thumb longer than the bar it rides in, on a viewport
        // shorter than `SCROLLBAR_MIN_LENGTH`.
        let visible = (self.viewport_height / self.content_height).clamp(0., 1.);
        let length = (height * visible).max(SCROLLBAR_MIN_LENGTH).min(height);

        // Against the *travel* — the track minus the thumb — so that an offset
        // at its maximum puts the thumb's bottom edge exactly on the track's.
        let progress = self.offset / max_offset;
        Some(((height - length) * progress, length))
    }
}

/// Shared ownership of a [`ScrollState`], held by a view across renders.
pub type ScrollStateHandle = Arc<Mutex<ScrollState>>;

/// Scrolls its child vertically inside the box it was given.
///
/// Measures the child against an unbounded height, keeps the box it was
/// allocated, and paints the child shifted up by the offset — inside a clipped
/// layer, so that the part hanging out of the box is neither drawn nor
/// clickable. [`Clipped`](super::Clipped) has the long version of why those
/// two are one job.
pub struct Scrollable {
    child: Box<dyn Element>,
    state: ScrollStateHandle,
    size: Option<Vector2F>,
    origin: Option<Point>,
    child_max_z_index: Option<ZIndex>,
    thumb_color: Option<Color>,
}

impl Scrollable {
    /// Wraps `child`, scrolling it against `state`.
    pub fn new(state: ScrollStateHandle, child: Box<dyn Element>) -> Self {
        Self {
            child,
            state,
            size: None,
            origin: None,
            child_max_z_index: None,
            thumb_color: None,
        }
    }

    /// Paints a thumb down the right edge in `color`, when there is anything
    /// to scroll.
    ///
    /// A colour rather than a flag because [`Scrollable`] is in the element
    /// library, which owns no palette: every other element takes the colours
    /// it paints from the caller, and a scrollbar that hardcoded one would be
    /// the only thing in `crookui_core` that knows what Crook looks like.
    pub fn with_scrollbar(mut self, color: Color) -> Self {
        self.thumb_color = Some(color);
        self
    }

    /// Whether `position` is over this element and not covered by anything.
    ///
    /// The same test [`Hoverable`](super::Hoverable) does, and for the same
    /// reason: a wheel over a popup that happens to be drawn above this list
    /// must move the popup, not the list under it.
    fn is_mouse_over(&self, position: Vector2F, ctx: &EventContext) -> bool {
        let (Some(origin), Some(size), Some(z_index)) =
            (self.origin, self.size, self.child_max_z_index)
        else {
            return false;
        };

        ctx.visible_rect(origin, size)
            .is_some_and(|visible| visible.contains_point(position))
            && !ctx.is_covered(Point::from_vec2f(position, z_index))
    }
}

impl Element for Scrollable {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        // Unbounded downwards, so the child reports the height it actually
        // wants rather than the height it was allowed. That number *is* the
        // scrollable range; a child measured against the viewport would report
        // the viewport back and there would be nothing to scroll.
        //
        // The minimum height goes to zero for the same reason: a tight minimum
        // handed down from a stretching parent would make every child at least
        // a viewport tall, and a two-row list would scroll.
        let child_constraint = SizeConstraint {
            min: vec2f(constraint.min.x(), 0.),
            max: vec2f(constraint.max.x(), f32::INFINITY),
        };
        let content = self.child.layout(child_constraint, ctx, app);

        // `apply` is what keeps this element inside its allocation: the width
        // the child chose, and a height clamped to what the parent offered. In
        // a parent that offered infinity — a column sized to its content —
        // nothing is clamped and nothing scrolls, which is the correct answer
        // for a list with no viewport to be smaller than.
        let size = constraint.apply(content);
        self.size = Some(size);

        // Clamping here can move content that this frame has already been laid
        // out against — the frame paints one wheel-click stale and the next
        // one is correct. The alternative is a second layout pass on every
        // resize, which is a real cost paid for a frame nobody can see.
        self.state.lock().measured(content.y(), size.y());

        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let size = self
            .size
            .expect("a scrollable was painted before it was laid out");

        ctx.scene
            .start_layer(ClipBounds::BoundedByActiveLayerAnd(RectF::new(
                origin, size,
            )));

        // Recorded inside the clipped layer, like `Clipped` does: the content
        // lives there, so that is the layer its hit tests resolve against.
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));

        let state = *self.state.lock();
        self.child.paint(origin - vec2f(0., state.offset), ctx, app);

        if let Some(color) = self.thumb_color {
            let track = size.y() - SCROLLBAR_INSET * 2.;
            if let Some((start, length)) = state.thumb(track) {
                // Without hit recording: the thumb is an indicator, and a
                // press on it belongs to whatever row it is floating over.
                ctx.scene
                    .draw_rect_without_hit_recording(RectF::new(
                        origin
                            + vec2f(
                                size.x() - SCROLLBAR_WIDTH - SCROLLBAR_INSET,
                                SCROLLBAR_INSET + start,
                            ),
                        vec2f(SCROLLBAR_WIDTH, length),
                    ))
                    .with_background(color)
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(
                        SCROLLBAR_WIDTH / 2.,
                    )));
            }
        }

        ctx.scene.stop_layer();

        // The topmost layer the child reached, for the same reason
        // `Hoverable` records it: a child painted into a layer above still
        // counts as this element's content rather than as something covering
        // it.
        self.child_max_z_index = Some(ctx.scene.max_active_z_index());
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        // The child first, always. A nested scrollable that can still move
        // takes the wheel; one that has hit its end returns false and this one
        // takes it, which is the behaviour a list inside a scrolling page
        // needs and the only thing the return value of `scroll_by` is for.
        let handled = self.child.dispatch_event(event, ctx, app);
        if handled {
            return true;
        }

        let Event::ScrollWheel {
            position, delta, ..
        } = event.raw_event()
        else {
            return false;
        };

        if !self.is_mouse_over(*position, ctx) {
            return false;
        }

        // Negated: a wheel reports positive-up, and the offset counts
        // positive-down.
        let pixels = -delta.to_pixels(PIXELS_PER_WHEEL_LINE).y();
        if !self.state.lock().scroll_by(pixels) {
            // At an end, with nothing to move. Declining lets a scrollable
            // further out take the event, and stops a repaint that would draw
            // an identical frame.
            return false;
        }

        ctx.notify();
        true
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
