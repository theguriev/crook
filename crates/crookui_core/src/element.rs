//! The [`Element`] trait: Flutter's box protocol, reduced to three walks.
//!
//! Constraints go down, sizes come up, and the parent decides position. A
//! child is told how big it may be, answers how big it chose to be, and only
//! later — in a second walk — is told where it ended up. Keeping position out
//! of layout is what lets a flex measure every child before placing any of
//! them.
//!
//! An element is not a description of a frame. It is a mutable scratch buffer
//! for one: `layout` caches the size it returned, `paint` caches the origin it
//! was given, and `dispatch_event` reads both back to hit-test. The whole tree
//! is thrown away and rebuilt whenever its view re-renders, so **no
//! interaction state may live in an element** — hover, pressed and scroll
//! offsets belong to the view, handed down each time it renders.

use std::any::Any;

use crate::AppContext;
use crate::event::DispatchedEvent;
use crate::geometry::{Axis, Point, RectF, Vector2F, ZIndex};
use crate::presenter::{EventContext, LayoutContext, PaintContext};

/// One node in a view's render tree.
pub trait Element {
    /// Chooses a size within `constraint`.
    ///
    /// The returned size must satisfy the constraint, and implementors cache
    /// it for [`Self::paint`] to read back.
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F;

    /// Emits draw commands for this element at `origin`.
    ///
    /// `origin` is in absolute window coordinates and was chosen by the
    /// parent. Painting an element that was never laid out panics: its cached
    /// size is still `None`.
    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext);

    /// The size chosen by the last layout, or `None` before the first.
    fn size(&self) -> Option<Vector2F>;

    /// The position given by the last paint, or `None` before the first.
    fn origin(&self) -> Option<Point>;

    /// Handles an input event, returning whether to stop propagation.
    ///
    /// Parents call this on every child unconditionally; each child does its
    /// own hit testing. Returning `true` means "handled, do not tell my
    /// parent".
    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool;

    /// The scene layer this element painted into.
    fn z_index(&self) -> Option<ZIndex> {
        self.origin().map(|origin| origin.z_index())
    }

    /// Where this element ended up, once it has been laid out and painted.
    fn bounds(&self) -> Option<RectF> {
        Some(RectF::new(self.origin()?.xy(), self.size()?))
    }

    /// Configuration this element passes *up* to a particular parent type.
    ///
    /// The channel is deliberately untyped: [`Flex`](crate::elements::Flex)
    /// downcasts it to its own parent-data type and ignores anything else.
    /// That gives per-parent child configuration without a trait per
    /// container, at the cost of failing silently when a wrapper swallows it —
    /// which is why `Expanded` only works as a *direct* child of a flex.
    fn parent_data(&self) -> Option<&dyn Any> {
        None
    }

    /// Boxes this element, ending a builder chain.
    fn finish(self) -> Box<dyn Element>
    where
        Self: 'static + Sized,
    {
        Box::new(self)
    }
}

/// Adds children to any container that can be extended with them.
pub trait ParentElement: Extend<Box<dyn Element>> + Sized {
    /// Appends one child.
    fn add_child(&mut self, child: Box<dyn Element>) {
        self.extend(Some(child));
    }

    /// Appends several children.
    fn add_children(&mut self, children: impl IntoIterator<Item = Box<dyn Element>>) {
        self.extend(children);
    }

    /// Appends one child, for builder chains.
    fn with_child(self, child: Box<dyn Element>) -> Self {
        self.with_children(Some(child))
    }

    /// Appends several children, for builder chains.
    fn with_children(mut self, children: impl IntoIterator<Item = Box<dyn Element>>) -> Self {
        self.add_children(children);
        self
    }
}

impl<T> ParentElement for T where T: Extend<Box<dyn Element>> {}

/// The range of sizes an element may choose from.
///
/// `max` may be infinite along an axis: that is how a flex asks a child "how
/// big would you like to be?". See the module comment on
/// [`flex`](crate::elements::flex) for what that costs.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SizeConstraint {
    /// The smallest acceptable size.
    pub min: Vector2F,
    /// The largest acceptable size.
    pub max: Vector2F,
}

impl SizeConstraint {
    /// A constraint spanning `min` to `max`.
    pub fn new(min: Vector2F, max: Vector2F) -> Self {
        Self { min, max }
    }

    /// A constraint that admits exactly one size.
    pub fn strict(size: Vector2F) -> Self {
        Self {
            min: size,
            max: size,
        }
    }

    /// `size` clamped into this constraint.
    pub fn apply(&self, size: Vector2F) -> Vector2F {
        size.clamp(self.min, self.max)
    }

    /// The largest allowed extent along `axis`.
    pub fn max_along(&self, axis: Axis) -> f32 {
        match axis {
            Axis::Horizontal => self.max.x(),
            Axis::Vertical => self.max.y(),
        }
    }

    /// The smallest and largest allowed extents along `axis`.
    pub fn constraint_for_axis(&self, axis: Axis) -> (f32, f32) {
        match axis {
            Axis::Horizontal => (self.min.x(), self.max.x()),
            Axis::Vertical => (self.min.y(), self.max.y()),
        }
    }

    /// The constraint for a flex child that must fill the cross axis and is
    /// free along the main one.
    pub fn tight_on_cross_axis(main_axis: Axis, parent: SizeConstraint) -> Self {
        match main_axis {
            Axis::Horizontal => Self {
                min: crate::geometry::vec2f(0., parent.max.y()),
                max: crate::geometry::vec2f(f32::INFINITY, parent.max.y()),
            },
            Axis::Vertical => Self {
                min: crate::geometry::vec2f(parent.max.x(), 0.),
                max: crate::geometry::vec2f(parent.max.x(), f32::INFINITY),
            },
        }
    }

    /// The constraint for a flex child that may be smaller than the cross axis
    /// and is free along the main one.
    pub fn child_constraint_along_axis(main_axis: Axis, parent: SizeConstraint) -> Self {
        let (_, cross_max) = parent.constraint_for_axis(main_axis.invert());
        match main_axis {
            Axis::Horizontal => Self {
                min: Vector2F::zero(),
                max: crate::geometry::vec2f(f32::INFINITY, cross_max),
            },
            Axis::Vertical => Self {
                min: Vector2F::zero(),
                max: crate::geometry::vec2f(cross_max, f32::INFINITY),
            },
        }
    }
}
