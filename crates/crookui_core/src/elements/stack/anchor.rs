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
    /// The corner on the same side, at the other end top to bottom.
    fn mirrored(self) -> Self {
        match self {
            Self::TopLeft => Self::BottomLeft,
            Self::TopRight => Self::BottomRight,
            Self::BottomLeft => Self::TopLeft,
            Self::BottomRight => Self::TopRight,
        }
    }

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

/// How close to the window's edge a slid child may come.
///
/// A menu that stops exactly on the edge reads as one that was cut off there
/// — the eye cannot tell a border on the last pixel from no border at all.
/// Eight pixels of ground between the two is what says the box ends where it
/// is drawn ending, and it is the inset the rest of the chrome keeps.
pub const WINDOW_INSET: f32 = 8.;

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

    /// Whether that slide may move the child back over the box it hangs off.
    ///
    /// A menu hung below a button may cover it harmlessly, so the default is
    /// that it may. A card that *describes* the thing it hangs off may not: on
    /// top of its own row it takes the clicks meant for the row, the pointer
    /// that opened it stops counting as being over the row, and the card is
    /// torn down and re-armed frame after frame. Warp's detail sidecar has the
    /// same rule — it narrows to a minimum and then clips off the window edge
    /// rather than moving back across the panel.
    ///
    /// Held per axis, and only on the axes the anchor already put the child
    /// clear of the parent on: a child that overlapped its parent to begin
    /// with is left where it is.
    pub keep_clear_of_parent: bool,

    /// Whether a child that does not fit on its side of the parent is hung
    /// off the other side instead, when it fits there.
    ///
    /// Up and down only, which is the axis a menu runs out of. A menu hung
    /// below a row near the foot of the window used to be slid up until it
    /// fitted — over the row it was opened on, and over the rows above that,
    /// so the thing a person had just pressed was gone under what it opened.
    /// Hung above the row instead it covers none of them, which is what every
    /// desktop's context menu does there.
    ///
    /// Only when the other side has room for all of it: where neither does,
    /// the slide is still the answer, since a menu cut short to stay off its
    /// row is a menu whose last entries are behind a scroll.
    pub flip_to_fit: bool,
}

impl AnchorTo {
    /// A child hung directly below its parent, left edges aligned.
    pub fn below(offset: Vector2F) -> Self {
        Self {
            parent: Corner::BottomLeft,
            child: Corner::TopLeft,
            offset,
            keep_on_screen: true,
            keep_clear_of_parent: false,
            flip_to_fit: false,
        }
    }

    /// Where a `child`-sized box goes, given the `parent` box it hangs off and
    /// the window it must stay inside.
    pub(super) fn place(self, child: Vector2F, parent: RectF, window: Vector2F) -> Vector2F {
        let anchored = self.anchored(child, parent);
        let anchored = if self.flip_to_fit {
            self.flipped_if_short(anchored, child, parent, window)
        } else {
            anchored
        };

        let origin = if self.keep_on_screen {
            // Slid, unless it was flipped above: a menu that overhangs the right edge moves
            // left until it fits, with [`WINDOW_INSET`] of air left beyond it,
            // and one larger than the window starts at the inset rather than
            // at a negative coordinate.
            let inset = vec2f(WINDOW_INSET, WINDOW_INSET);
            anchored.min(window - child - inset).max(inset)
        } else {
            anchored
        };

        if self.keep_clear_of_parent {
            self.push_clear(origin, anchored, child, parent)
        } else {
            origin
        }
    }

    /// Where the anchor puts a `child`-sized box against `parent`, before
    /// anything keeps it on screen.
    fn anchored(self, child: Vector2F, parent: RectF) -> Vector2F {
        parent.origin() + self.parent.offset_in(parent.size()) - self.child.offset_in(child)
            + self.offset
    }

    /// `anchored`, or the same anchor mirrored top for bottom when `anchored`
    /// runs past the window's lower or upper inset and the mirror does not.
    /// See [`Self::flip_to_fit`].
    ///
    /// The mirror swaps which edge of the parent and of the child meet and
    /// turns the vertical gap round, so a menu 4 below a row becomes a menu
    /// 4 above it, its bottom edge on the row's top.
    fn flipped_if_short(
        self,
        anchored: Vector2F,
        child: Vector2F,
        parent: RectF,
        window: Vector2F,
    ) -> Vector2F {
        let fits = |top: f32| top >= WINDOW_INSET && top + child.y() <= window.y() - WINDOW_INSET;
        if fits(anchored.y()) {
            return anchored;
        }
        let mirrored = Self {
            parent: self.parent.mirrored(),
            child: self.child.mirrored(),
            offset: vec2f(self.offset.x(), -self.offset.y()),
            ..self
        }
        .anchored(child, parent);
        if fits(mirrored.y()) {
            mirrored
        } else {
            anchored
        }
    }

    /// Undoes, per axis, as much of the on-screen slide as would have put the
    /// child back over `parent`.
    ///
    /// The unslid `anchored` position is what says which side of the parent
    /// this child was meant to be on, so an anchor that already overlapped its
    /// parent keeps overlapping it and only the ones that were clear are held.
    fn push_clear(
        self,
        origin: Vector2F,
        anchored: Vector2F,
        child: Vector2F,
        parent: RectF,
    ) -> Vector2F {
        let hold = |placed: f32, anchored: f32, extent: f32, min: f32, max: f32| {
            if anchored >= max {
                placed.max(max)
            } else if anchored + extent <= min {
                placed.min(min - extent)
            } else {
                placed
            }
        };

        vec2f(
            hold(
                origin.x(),
                anchored.x(),
                child.x(),
                parent.min_x(),
                parent.max_x(),
            ),
            hold(
                origin.y(),
                anchored.y(),
                child.y(),
                parent.min_y(),
                parent.max_y(),
            ),
        )
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
    fn a_child_kept_clear_of_its_parent_clips_rather_than_sliding_over_it() {
        // A detail card beside a 20-wide button in a 100-wide window: there is
        // no room to its right, and sliding it back would put it on top of the
        // thing it describes.
        let anchor = AnchorTo {
            parent: Corner::TopRight,
            child: Corner::TopLeft,
            offset: vec2f(12., 0.),
            keep_on_screen: true,
            keep_clear_of_parent: true,
            flip_to_fit: false,
        };

        assert_eq!(
            anchor.place(vec2f(80., 20.), BUTTON, vec2f(100., 200.)),
            vec2f(60., 10.),
            "the card was pulled left of the button's right edge"
        );
        assert_eq!(
            AnchorTo {
                keep_clear_of_parent: false,
                flip_to_fit: false,
                ..anchor
            }
            .place(vec2f(80., 20.), BUTTON, vec2f(100., 200.)),
            vec2f(20. - WINDOW_INSET, 10.),
            "without the rule it slides over the button, which is the default"
        );

        // The other axis is untouched: a card whose top is level with its row
        // was never clear of it vertically, so it still slides up to stay on
        // screen — to the inset, which is as far up as anything slides.
        assert_eq!(
            anchor.place(vec2f(80., 60.), BUTTON, vec2f(200., 60.)).y(),
            WINDOW_INSET,
            "the vertical slide was held by a rule about the horizontal one"
        );
    }

    #[test]
    fn keeping_a_child_on_screen_slides_it_back_inside_the_window() {
        let anchor = AnchorTo::below(vec2f(0., 4.));

        assert_eq!(
            anchor.place(vec2f(200., 100.), BUTTON, vec2f(100., 200.)),
            vec2f(WINDOW_INSET, 42.),
            "a child wider than the window starts at the inset from its left edge"
        );
        assert_eq!(
            anchor.place(vec2f(80., 20.), BUTTON, vec2f(100., 200.)),
            vec2f(20. - WINDOW_INSET, 42.),
            "an overhanging child slides left by its overhang and the inset"
        );
    }

    #[test]
    fn a_child_short_of_room_below_is_hung_above_when_it_fits_there() {
        // A menu opened on a row near the foot of the window was slid up
        // until it fitted, over the row it was opened on. Hung above the
        // row, it covers none of it.
        let anchor = AnchorTo {
            flip_to_fit: true,
            ..AnchorTo::below(vec2f(0., 4.))
        };
        let low = RectF::new(vec2f(40., 300.), vec2f(20., 28.));
        let window = vec2f(400., 400.);

        let placed = anchor.place(vec2f(80., 200.), low, window);
        assert_eq!(
            placed,
            vec2f(40., 300. - 4. - 200.),
            "the menu's bottom is not 4 above the row's top"
        );
        assert!(placed.y() + 200. <= low.min_y(), "it covers the row");

        // Without the rule it slides, over the row: the default.
        let slid = AnchorTo::below(vec2f(0., 4.)).place(vec2f(80., 200.), low, window);
        assert_eq!(slid.y() + 200., 400. - WINDOW_INSET);
        assert!(slid.y() < low.max_y(), "{slid:?}");
    }

    #[test]
    fn a_child_that_fits_below_stays_below_and_one_that_fits_nowhere_slides() {
        let anchor = AnchorTo {
            flip_to_fit: true,
            ..AnchorTo::below(vec2f(0., 4.))
        };
        let window = vec2f(400., 400.);

        // Room below: below, as it always was, even with more room above.
        let low = RectF::new(vec2f(40., 300.), vec2f(20., 28.));
        assert_eq!(anchor.place(vec2f(80., 40.), low, window), vec2f(40., 332.));

        // Too tall for either side of a row in the middle: slid to fit the
        // window, as without the rule, rather than hung off the far side and
        // cut by the window's edge.
        let middle = RectF::new(vec2f(40., 180.), vec2f(20., 28.));
        let placed = anchor.place(vec2f(80., 300.), middle, window);
        assert_eq!(placed.y() + 300., 400. - WINDOW_INSET, "{placed:?}");
        assert!(placed.y() >= WINDOW_INSET, "{placed:?}");
    }

    #[test]
    fn a_slid_child_stops_short_of_the_edge_by_the_inset() {
        // A menu hung off a control near the bottom of the window used to end
        // on the window's last pixel, where its border and the edge were one
        // line. It stops the inset short now, on every side it was slid to.
        let anchor = AnchorTo::below(vec2f(0., 4.));
        let low = RectF::new(vec2f(40., 180.), vec2f(20., 10.));

        let placed = anchor.place(vec2f(80., 60.), low, vec2f(200., 200.));
        assert_eq!(placed.y() + 60., 200. - WINDOW_INSET, "{placed:?}");
        assert_eq!(placed.x(), 40., "an axis with room was slid anyway");
    }
}
