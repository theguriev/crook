//! The two facts a draggable split divider needs, and neither of them is in
//! the model.
//!
//! The pane group holds *weights*: how large a share of the split each pane
//! takes. What those become in pixels is decided by a flex layout that has
//! finished running long before a pointer arrives, and a divider dragged
//! twenty pixels has to be turned back into a share before the group can act
//! on it. So the panes measure themselves and leave the number here, and the
//! divider reads its two neighbours' extents when a press lands.
//!
//! The drag itself lives here for the reason every other pointer gesture in
//! this crate does: the element tree is rebuilt on every frame, and a press is
//! only half a gesture. One [`DividerDrag`] is shared by every divider in the
//! window, which is also what makes "only one divider can be dragged at a
//! time" a fact rather than a rule somebody has to keep.

use std::cell::Cell;
use std::rc::Rc;

use crate::tab::PaneId;

/// How many pixels one pane occupies along the axis its split divides.
///
/// Written by the element that lays the pane out and read by the divider
/// beside it. Zero before the first frame, which is the one state a divider
/// treats as "there is nothing to drag yet".
///
/// Cheap to clone — it is an [`Rc`] — because the element that measures it is
/// thrown away every frame.
#[derive(Clone, Default)]
pub struct PaneExtent(Rc<Cell<f32>>);

impl PaneExtent {
    /// An extent nothing has measured yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many pixels the pane last measured.
    pub fn get(&self) -> f32 {
        self.0.get()
    }

    /// Records what the pane measured this frame.
    pub fn set(&self, extent: f32) {
        self.0.set(extent);
    }
}

/// A divider being dragged: which pair, from where, and how big they were.
///
/// The extents are captured at the press rather than read again on every move,
/// and that is what makes the drag stable. Reading them live would mean each
/// move resizing the panes and the next move measuring the result, so a drag
/// would chase its own output and the divider would drift away from the
/// pointer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Drag {
    /// The pane on the left, or above.
    pub before: PaneId,
    /// The pane on the right, or below.
    pub after: PaneId,
    /// Where along the axis the press landed, in window coordinates.
    pub anchor: f32,
    /// How many pixels `before` occupied at that moment.
    pub before_extent: f32,
    /// How many pixels `after` occupied at that moment.
    pub after_extent: f32,
}

/// The divider drag in progress anywhere in the window, if there is one.
///
/// Cheap to clone — it is an [`Rc`] — because every divider takes one every
/// frame, and they all have to be looking at the same gesture.
#[derive(Clone, Default)]
pub struct DividerDrag(Rc<Cell<Option<Drag>>>);

impl DividerDrag {
    /// A window with no divider being dragged.
    pub fn new() -> Self {
        Self::default()
    }

    /// The drag in progress, if there is one.
    pub fn get(&self) -> Option<Drag> {
        self.0.get()
    }

    /// Starts a drag, replacing any there was.
    pub fn begin(&self, drag: Drag) {
        self.0.set(Some(drag));
    }

    /// Ends the drag, reporting whether there was one to end.
    pub fn end(&self) -> bool {
        self.0.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(before: PaneId, after: PaneId) -> Drag {
        Drag {
            before,
            after,
            anchor: 100.,
            before_extent: 400.,
            after_extent: 200.,
        }
    }

    #[test]
    fn only_one_divider_can_be_dragged_at_a_time() {
        // Not a rule anybody has to keep: there is one cell, and every divider
        // in the window is holding it.
        let held = DividerDrag::new();
        let another_divider = held.clone();

        let (a, b) = (PaneId::next(), PaneId::next());
        held.begin(drag(a, b));

        assert_eq!(another_divider.get().map(|drag| drag.before), Some(a));
        assert!(another_divider.end());
        assert!(!held.end(), "the gesture was already over");
    }

    #[test]
    fn an_extent_survives_the_element_that_measured_it() {
        // The whole reason it is an `Rc`: the tree that measured the pane has
        // been thrown away by the time the divider beside it is pressed.
        let kept = PaneExtent::new();
        let handed_to_the_element = kept.clone();

        assert_eq!(kept.get(), 0., "nothing has been laid out yet");
        handed_to_the_element.set(512.);
        assert_eq!(kept.get(), 512.);
    }
}
