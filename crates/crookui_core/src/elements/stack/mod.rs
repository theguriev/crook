//! Children drawn on top of one another, and the popups hung off them.
//!
//! A stack has two kinds of child. Ordinary ones are painted at the stack's
//! own origin, each in a layer of its own — starting a layer per child *is*
//! what stacks them in z — and each contributes to the stack's size, so a
//! stack is as big as its largest child.
//!
//! Anchored ones do neither. They are laid out against the window rather than
//! against the stack's constraint, painted at a corner of the stack's box
//! rather than at its origin, and lifted into an overlay layer, so they float
//! above the entire frame. That is the shape of every menu: a `Stack` around
//! the button that opens it, the menu itself added as an anchored overlay
//! child while it is open.
//!
//! ```no_run
//! # use crookui_core::prelude::*;
//! # use crookui_core::elements::{AnchorTo, Dismiss, Stack};
//! # #[derive(Debug)]
//! # struct CloseTheMenu;
//! # fn build(button: Box<dyn Element>, menu: Box<dyn Element>, is_open: bool) -> Box<dyn Element> {
//! let mut stack = Stack::new().with_child(button);
//! if is_open {
//!     stack.add_anchored_overlay_child(
//!         Dismiss::new(menu)
//!             .modal()
//!             .on_dismiss(|ctx, _| ctx.dispatch_typed_action(CloseTheMenu))
//!             .finish(),
//!         AnchorTo::below(vec2f(0., 4.)),
//!     );
//! }
//! stack.finish()
//! # }
//! ```
//!
//! The menu's own root must be a [`Container`](crate::elements::Container)
//! with a background. Containers are the only elements that record a hit rect,
//! and a popup that records none floats visually while every click passes
//! straight through it to the tabs underneath.

mod anchor;
mod overlay;
mod positioned;

pub use anchor::{AnchorTo, Corner};
use overlay::Overlay;
use positioned::Positioned;

use crate::AppContext;
use crate::element::{Element, SizeConstraint};
use crate::event::DispatchedEvent;
use crate::geometry::{Point, Vector2F};
use crate::presenter::{EventContext, LayoutContext, PaintContext};
use crate::scene::ClipBounds;

/// A child and whether the last paint reached it.
struct StackChild {
    element: Box<dyn Element>,
    painted: bool,
}

/// Draws its children on top of one another, and hangs floating ones off its
/// own box.
///
/// See the [module docs](self) for the difference between the two kinds of
/// child.
#[derive(Default)]
pub struct Stack {
    children: Vec<StackChild>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Stack {
    /// An empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a child that floats above the whole frame, anchored to this
    /// stack's box.
    ///
    /// Add it *last*: an element hit-tests against the topmost layer that
    /// existed when it was painted, so a second overlay painted after this one
    /// would leave everything inside it unclickable.
    pub fn add_anchored_overlay_child(&mut self, child: Box<dyn Element>, anchor: AnchorTo) {
        self.push_child(Positioned::new(Overlay::new(child).finish(), anchor).finish());
    }

    /// Adds an anchored overlay child, for builder chains.
    pub fn with_anchored_overlay_child(
        mut self,
        child: Box<dyn Element>,
        anchor: AnchorTo,
    ) -> Self {
        self.add_anchored_overlay_child(child, anchor);
        self
    }

    fn push_child(&mut self, child: Box<dyn Element>) {
        self.children.push(StackChild {
            element: child,
            painted: false,
        });
    }
}

impl Extend<Box<dyn Element>> for Stack {
    fn extend<T: IntoIterator<Item = Box<dyn Element>>>(&mut self, children: T) {
        for child in children {
            self.push_child(child);
        }
    }
}

/// The anchor a child carries, if it is anchored rather than stacked.
fn anchor_of(child: &StackChild) -> Option<AnchorTo> {
    child
        .element
        .parent_data()
        .and_then(|data| data.downcast_ref::<AnchorTo>())
        .copied()
}

impl Element for Stack {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let mut size = constraint.min;
        for child in &mut self.children {
            if anchor_of(child).is_none() {
                size = size.max(child.element.layout(constraint, ctx, app));
            }
        }

        for child in &mut self.children {
            if anchor_of(child).is_some() {
                // The window, not this stack's constraint. A stack wrapping a
                // 28-pixel toolbar button would otherwise offer a menu 28
                // pixels of height and get every row of it laid out into
                // nothing.
                child.element.layout(
                    SizeConstraint::new(Vector2F::zero(), ctx.window_size),
                    ctx,
                    app,
                );
            }
        }

        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let bounds = self
            .bounds()
            .expect("a stack was painted before it was laid out");

        for child in &mut self.children {
            // One layer per child, which is the whole of stacking: the next
            // child paints into a later layer, so it covers this one and takes
            // the clicks that land on both.
            ctx.scene.start_layer(ClipBounds::ActiveLayer);

            let child_origin = match anchor_of(child) {
                Some(anchor) => {
                    let size = child
                        .element
                        .size()
                        .expect("a stack child was painted before it was laid out");
                    anchor.place(size, bounds, ctx.window_size)
                }
                None => origin,
            };
            child.element.paint(child_origin, ctx, app);
            child.painted = true;

            ctx.scene.stop_layer();
        }
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        // Topmost first, and the first child to claim the event ends it.
        // Broadcasting instead would hand a click on a menu to the tab drawn
        // underneath it as well, and leave both to work out which of them the
        // user meant.
        self.children
            .iter_mut()
            .rev()
            // A child that was never painted has no geometry from this frame,
            // and would hit-test against wherever it sat in the last one.
            .filter(|child| child.painted)
            .any(|child| child.element.dispatch_event(event, ctx, app))
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}
