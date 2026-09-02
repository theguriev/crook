//! The flex layout: Flutter's algorithm, and the layout spine of every screen.
//!
//! Children come in two kinds. Non-flexible ones are laid out first, each free
//! to be any size along the main axis; whatever room is left is then divided
//! among the flexible ones ([`Expanded`], [`Shrinkable`]) in proportion to
//! their flex factor.
//!
//! # On infinity
//!
//! `SizeConstraint::max` is allowed to be infinite here, and this module is
//! where it comes from: measuring a non-flexible child means asking it how big
//! it would like to be, which is exactly an unbounded main axis. Warp keeps
//! that same rule and pays for it with a checked-in debugging guide, because
//! two situations then have no answer:
//!
//! * a flex with `MainAxisSize::Max` and an infinite main axis has no "max" to
//!   expand to, and
//! * a flexible child divides infinity by a flex factor.
//!
//! Both are caught by `debug_assert!` below and logged in release. Forbidding
//! infinity outright was the alternative, and it was rejected: it would mean
//! either a separate intrinsic-sizing pass (which is the thing Flutter's
//! protocol exists to avoid) or making every leaf element guess a finite
//! preferred size. The rule to design against is instead: the root constraint
//! is the window, so a main axis only becomes infinite inside a flex, and a
//! flex that needs a bounded one must be given it — by an ancestor, or by
//! wrapping it in a [`ConstrainedBox`](crate::elements::ConstrainedBox).

use std::any::Any;

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Axis, F32Ext, Point, Vector2F, Vector2FExt, vec2f};
use crate::presenter::{EventContext, LayoutContext, PaintContext};

/// Lays its children out in a row or a column.
pub struct Flex {
    axis: Axis,
    children: Vec<Box<dyn Element>>,
    spacing: f32,
    main_axis_size: MainAxisSize,
    main_axis_alignment: MainAxisAlignment,
    cross_axis_alignment: CrossAxisAlignment,
    size: Option<Vector2F>,
    origin: Option<Point>,
    layout_state: Option<LayoutState>,
}

/// How much room a flex takes along its main axis.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MainAxisSize {
    /// Only as much as its children need.
    Min,
    /// All the room the constraint allows, distributing the surplus according
    /// to the [`MainAxisAlignment`].
    Max,
}

/// Where children sit along the main axis when there is room to spare.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MainAxisAlignment {
    /// Packed at the start.
    Start,
    /// Packed at the end.
    End,
    /// Packed in the middle.
    Center,
    /// Spread out, with the gaps between children.
    SpaceBetween,
    /// Spread out, with equal gaps before, between and after.
    SpaceEvenly,
}

/// Where children sit across the main axis.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CrossAxisAlignment {
    /// Aligned to the leading edge.
    Start,
    /// Aligned to the trailing edge.
    End,
    /// Centered.
    Center,
    /// Stretched to fill the cross axis.
    Stretch,
}

/// Where the surplus main-axis space went, computed once during layout and
/// read back during paint.
#[derive(Debug)]
struct LayoutState {
    /// Added between each pair of children.
    between_space: Vector2F,
    /// Added before the first child.
    leading_space: Vector2F,
}

impl LayoutState {
    fn compute(
        children_count: usize,
        spacing: f32,
        remaining_space: f32,
        alignment: MainAxisAlignment,
        axis: Axis,
    ) -> Self {
        let (between_space, leading_space) = match alignment {
            MainAxisAlignment::Start => (0., 0.),
            MainAxisAlignment::End => (0., remaining_space),
            MainAxisAlignment::Center => (0., remaining_space / 2.),
            MainAxisAlignment::SpaceBetween => match children_count {
                0 | 1 => (0., 0.),
                count => (remaining_space / (count - 1) as f32, 0.),
            },
            MainAxisAlignment::SpaceEvenly => match children_count {
                0 => (0., 0.),
                // One more gap than there are children: before, between, after.
                count => {
                    let even_space = remaining_space / (count + 1) as f32;
                    (even_space, even_space)
                }
            },
        };

        Self {
            between_space: (spacing + between_space).along(axis),
            leading_space: leading_space.along(axis),
        }
    }
}

impl Flex {
    /// An empty flex along `axis`.
    pub fn new(axis: Axis) -> Self {
        Self {
            axis,
            children: Vec::new(),
            spacing: 0.,
            main_axis_size: MainAxisSize::Min,
            main_axis_alignment: MainAxisAlignment::Start,
            cross_axis_alignment: CrossAxisAlignment::Start,
            size: None,
            origin: None,
            layout_state: None,
        }
    }

    /// An empty horizontal flex.
    pub fn row() -> Self {
        Self::new(Axis::Horizontal)
    }

    /// An empty vertical flex.
    pub fn column() -> Self {
        Self::new(Axis::Vertical)
    }

    /// Sets the gap between children.
    pub fn with_spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Sets how much room the flex takes along its main axis.
    pub fn with_main_axis_size(mut self, main_axis_size: MainAxisSize) -> Self {
        self.main_axis_size = main_axis_size;
        self
    }

    /// Sets where children sit along the main axis.
    pub fn with_main_axis_alignment(mut self, alignment: MainAxisAlignment) -> Self {
        self.main_axis_alignment = alignment;
        self
    }

    /// Sets where children sit across the main axis.
    pub fn with_cross_axis_alignment(mut self, alignment: CrossAxisAlignment) -> Self {
        self.cross_axis_alignment = alignment;
        self
    }

    /// Whether the flex has no children.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    fn child_flex(child: &dyn Element) -> Option<FlexParentData> {
        child
            .parent_data()
            .and_then(|data| data.downcast_ref::<FlexParentData>())
            .copied()
    }
}

impl Extend<Box<dyn Element>> for Flex {
    fn extend<T: IntoIterator<Item = Box<dyn Element>>>(&mut self, children: T) {
        self.children.extend(children);
    }
}

impl Element for Flex {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let cross_axis = self.axis.invert();
        let mut cross_axis_max: f32 = 0.;
        let mut total_flex = 0.;
        let mut fixed_space = self.spacing * self.children.len().saturating_sub(1) as f32;

        if self.main_axis_size == MainAxisSize::Max {
            report_infinite_main_axis(
                constraint,
                self.axis,
                "a flex that expands to fill its parent cannot do so along an infinite axis",
            );
        }

        // Pass one: measure everything that is not flexible, each free along
        // the main axis and bounded across it.
        for child in &mut self.children {
            if let Some(parent_data) = Self::child_flex(child.as_ref()) {
                total_flex += parent_data.flex;
                continue;
            }

            let child_constraint = if self.cross_axis_alignment == CrossAxisAlignment::Stretch {
                SizeConstraint::tight_on_cross_axis(self.axis, constraint)
            } else {
                SizeConstraint::child_constraint_along_axis(self.axis, constraint)
            };

            let size = child.layout(child_constraint, ctx, app);
            fixed_space += size.along(self.axis);
            if size.along(cross_axis).is_finite() {
                cross_axis_max = cross_axis_max.max(size.along(cross_axis));
            }
        }

        let mut size = if total_flex > 0. {
            report_infinite_main_axis(
                constraint,
                self.axis,
                "a flex with flexible children cannot divide an infinite axis between them",
            );

            // Pass two: hand out what is left, in flex-factor proportion. The
            // divisor shrinks as children take their share, so a child that
            // uses less than its allowance leaves the surplus to the rest.
            let mut remaining_space = (constraint.max_along(self.axis) - fixed_space).max(0.);
            let mut remaining_flex = total_flex;

            for child in &mut self.children {
                let Some(parent_data) = Self::child_flex(child.as_ref()) else {
                    continue;
                };

                let child_max = remaining_space / remaining_flex * parent_data.flex;
                let child_min = match parent_data.fit {
                    FlexFit::Tight => child_max,
                    FlexFit::Loose => 0.,
                };

                let child_constraint = match self.axis {
                    Axis::Horizontal => SizeConstraint::new(
                        vec2f(child_min, constraint.min.y()),
                        vec2f(child_max, constraint.max.y()),
                    ),
                    Axis::Vertical => SizeConstraint::new(
                        vec2f(constraint.min.x(), child_min),
                        vec2f(constraint.max.x(), child_max),
                    ),
                };

                let child_size = child.layout(child_constraint, ctx, app);
                remaining_space -= child_size.along(self.axis);
                remaining_flex -= parent_data.flex;
                if child_size.along(cross_axis).is_finite() {
                    cross_axis_max = cross_axis_max.max(child_size.along(cross_axis));
                }
            }

            self.stretch_children(constraint, cross_axis_max, ctx, app);
            self.axis.to_point(
                constraint.max_along(self.axis) - remaining_space,
                cross_axis_max,
            )
        } else {
            self.stretch_children(constraint, cross_axis_max, ctx, app);
            self.axis.to_point(fixed_space, cross_axis_max)
        };

        let allocated = size.along(self.axis);
        let max_along_axis = constraint.max_along(self.axis);
        let expanded = self.main_axis_size == MainAxisSize::Max && max_along_axis.is_finite();
        if expanded {
            match self.axis {
                Axis::Horizontal => size.set_x(max_along_axis),
                Axis::Vertical => size.set_y(max_along_axis),
            }
        }

        if constraint.min.x().is_finite() {
            size.set_x(size.x().max(constraint.min.x()));
        }
        if constraint.min.y().is_finite() {
            size.set_y(size.y().max(constraint.min.y()));
        }

        self.layout_state = Some(LayoutState::compute(
            self.children.len(),
            self.spacing,
            (size.along(self.axis) - allocated).max(0.),
            self.main_axis_alignment,
            self.axis,
        ));
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let layout_state = self
            .layout_state
            .as_ref()
            .expect("a flex was painted before it was laid out");
        let size = self
            .size
            .expect("a flex was painted before it was laid out");
        let cross_axis = self.axis.invert();
        let parent_cross_size = size.along(cross_axis);

        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let mut child_origin = origin + layout_state.leading_space;

        for child in &mut self.children {
            let child_size = child
                .size()
                .expect("a flex child was painted before it was laid out");

            let child_cross_size = child_size.along(cross_axis);
            let cross_offset = match self.cross_axis_alignment {
                CrossAxisAlignment::Start | CrossAxisAlignment::Stretch => 0.,
                CrossAxisAlignment::Center => (parent_cross_size - child_cross_size) / 2.,
                CrossAxisAlignment::End => parent_cross_size - child_cross_size,
            };

            child.paint(child_origin + cross_offset.along(cross_axis), ctx, app);
            child_origin += self.axis.to_point(child_size.along(self.axis), 0.);
            child_origin += layout_state.between_space;
        }
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        // Every child sees the event, whether or not an earlier one handled
        // it: siblings do not occlude each other, and each does its own hit
        // testing anyway.
        let mut handled = false;
        for child in &mut self.children {
            handled |= child.dispatch_event(event, ctx, app);
        }
        handled
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

impl Flex {
    /// Re-lays-out children that came back infinite across the cross axis, now
    /// that the largest finite child is known.
    fn stretch_children(
        &mut self,
        constraint: SizeConstraint,
        cross_axis_max: f32,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) {
        if self.cross_axis_alignment != CrossAxisAlignment::Stretch {
            return;
        }

        let cross_axis = self.axis.invert();
        let mut constraint = constraint;
        match cross_axis {
            Axis::Horizontal => constraint.max.set_x(cross_axis_max),
            Axis::Vertical => constraint.max.set_y(cross_axis_max),
        }

        for child in &mut self.children {
            if child
                .size()
                .is_some_and(|size| size.along(cross_axis).is_infinite())
            {
                child.layout(
                    SizeConstraint::tight_on_cross_axis(self.axis, constraint),
                    ctx,
                    app,
                );
            }
        }
    }
}

fn report_infinite_main_axis(constraint: SizeConstraint, axis: Axis, message: &str) {
    debug_assert!(
        constraint.max_along(axis).is_finite(),
        "{message}; give it a bounded ancestor or wrap it in a ConstrainedBox"
    );
    if constraint.max_along(axis).is_infinite() {
        log::error!("{message}; give it a bounded ancestor or wrap it in a ConstrainedBox");
    }
}

/// What a flexible child tells its flex.
#[derive(Copy, Clone, Debug)]
pub struct FlexParentData {
    /// This child's share of the surplus, relative to its siblings' shares.
    pub flex: f32,
    /// Whether the child must take its whole share.
    pub fit: FlexFit,
}

/// Whether a flexible child is obliged to fill the room it was given.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FlexFit {
    /// It must: the minimum equals the maximum.
    Tight,
    /// It may: the minimum is zero.
    Loose,
}

macro_rules! flexible_child {
    ($name:ident, $fit:expr, $doc:literal) => {
        #[doc = $doc]
        ///
        /// Only works as a **direct** child of a [`Flex`]. Wrapping it in
        /// anything that does not forward `parent_data` — a container, a
        /// constrained box — silently turns it back into an ordinary child.
        pub struct $name {
            parent_data: FlexParentData,
            child: Box<dyn Element>,
        }

        impl $name {
            /// Wraps `child` with a flex factor.
            pub fn new(flex: f32, child: Box<dyn Element>) -> Self {
                Self {
                    parent_data: FlexParentData { flex, fit: $fit },
                    child,
                }
            }
        }

        impl Element for $name {
            fn layout(
                &mut self,
                constraint: SizeConstraint,
                ctx: &mut LayoutContext,
                app: &AppContext,
            ) -> Vector2F {
                self.child.layout(constraint, ctx, app)
            }

            fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
                self.child.paint(origin, ctx, app);
            }

            fn dispatch_event(
                &mut self,
                event: &DispatchedEvent,
                ctx: &mut EventContext,
                app: &AppContext,
            ) -> bool {
                self.child.dispatch_event(event, ctx, app)
            }

            fn size(&self) -> Option<Vector2F> {
                self.child.size()
            }

            fn origin(&self) -> Option<Point> {
                self.child.origin()
            }

            fn parent_data(&self) -> Option<&dyn Any> {
                Some(&self.parent_data)
            }
        }
    };
}

flexible_child!(
    Expanded,
    FlexFit::Tight,
    "A flex child that fills its share of the surplus, whether or not its own child wanted to grow.\n\nThe idiomatic spacer is `Expanded::new(1., Empty::new().finish())`, which pushes everything after it to the far edge."
);

flexible_child!(
    Shrinkable,
    FlexFit::Loose,
    "A flex child that may use up to its share of the surplus, but is free to be smaller."
);
