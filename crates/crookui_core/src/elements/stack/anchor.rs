//! Where a floating child sits relative to the box it hangs off.

use crate::geometry::{RectF, Vector2F, vec2f};

/// One corner of a box.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Corner {
    /// The upper-left corner.
    TopLeft,
    /// The upper-right corner.
    TopRight,
    /// The lower-left corner.
    BottomLeft,
    /// The lower-right corner.
    BottomRight,
}

impl Corner {
    /// The offset from a box's origin to this corner of it.
    fn offset_in(self, size: Vector2F) -> Vector2F {
        match self {
            Self::TopLeft => Vector2F::zero(),
            Self::TopRight => vec2f(size.x(), 0.),
            Self::BottomLeft => vec2f(0., size.y()),
            Self::BottomRight => size,
        }
    }
}

/// How to place a floating child against the box it hangs off.
///
/// Warp spells this `OffsetPositioning`: five bounding modes times nine anchors
/// times a rule per axis. A menu hanging off a button uses one combination of
/// all that, and this is it — pin a corner of the child to a corner of the
/// parent, nudge it, and keep it inside the window.
#[derive(Copy, Clone, Debug)]
pub struct AnchorTo {
    /// The corner of the parent box to hang off.
    pub parent: Corner,

    /// The corner of the child that lands on it.
    pub child: Corner,

    /// Added once the corners are matched up: the gap between a button and the
    /// menu below it.
    pub offset: Vector2F,

    /// Whether to slide the child back inside the window when the anchor put
    /// part of it outside — a menu hung off the last tab in the strip would
    /// otherwise open past the right edge.
    pub keep_on_screen: bool,
}

impl AnchorTo {
    /// A child hung directly below its parent, left edges aligned.
    pub fn below(offset: Vector2F) -> Self {
        Self {
            parent: Corner::BottomLeft,
            child: Corner::TopLeft,
            offset,
            keep_on_screen: true,
        }
    }

    /// Where a `child`-sized box goes, given the `parent` box it hangs off and
    /// the window it must stay inside.
    pub(super) fn place(self, child: Vector2F, parent: RectF, window: Vector2F) -> Vector2F {
        let origin = parent.origin() + self.parent.offset_in(parent.size())
            - self.child.offset_in(child)
            + self.offset;

        if self.keep_on_screen {
            // Slid, not flipped: a menu that overhangs the right edge moves
            // left until it fits, and one larger than the window starts at the
            // origin rather than at a negative coordinate.
            origin.min(window - child).max(Vector2F::zero())
        } else {
            origin
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUTTON: RectF = RectF::new(vec2f(40., 10.), vec2f(20., 28.));

    #[test]
    fn matching_corners_hangs_the_child_off_the_parent() {
        let anchor = AnchorTo {
            keep_on_screen: false,
            ..AnchorTo::below(vec2f(0., 4.))
        };

        assert_eq!(
            anchor.place(vec2f(200., 100.), BUTTON, vec2f(1000., 1000.)),
            vec2f(40., 42.),
            "the menu's top-left sits 4 below the button's bottom-left"
        );

        let right_aligned = AnchorTo {
            parent: Corner::BottomRight,
            child: Corner::TopRight,
            ..anchor
        };
        assert_eq!(
            right_aligned.place(vec2f(200., 100.), BUTTON, vec2f(1000., 1000.)),
            vec2f(-140., 42.),
            "aligning the right edges puts a wide menu left of the button"
        );
    }

    #[test]
    fn keeping_a_child_on_screen_slides_it_back_inside_the_window() {
        let anchor = AnchorTo::below(vec2f(0., 4.));

        assert_eq!(
            anchor.place(vec2f(200., 100.), BUTTON, vec2f(100., 200.)),
            vec2f(0., 42.),
            "a child wider than the window starts at its left edge"
        );
        assert_eq!(
            anchor.place(vec2f(80., 20.), BUTTON, vec2f(100., 200.)),
            vec2f(20., 42.),
            "an overhanging child slides left by exactly its overhang"
        );
    }
}
