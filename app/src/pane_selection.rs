//! The selection gesture in one pane's output, as the view holds it between
//! frames.
//!
//! The selection itself is not here. It lives in the emulator, in `Term`'s own
//! `selection` field, because that is the only place it can stay anchored to
//! the text while the shell scrolls output underneath it — see
//! [`crook_terminal::selection`]. What cannot live there is the *gesture*: the
//! emulator never hears about a button, and "is the button that went down on
//! this grid still down?" is the one fact that decides whether the next pointer
//! move is dragging a selection out of this pane or is a drag that began in the
//! field beside it, or in another pane entirely.
//!
//! It cannot live in the element either, for the reason nothing interactive
//! can: the element tree is thrown away and rebuilt on every render, and a
//! press is only half a gesture. So the workspace keeps one of these per pane
//! and hands the element a clone each frame, exactly as it does with
//! [`TextInput`](crate::text_input::TextInput) and every mouse state — and a
//! pane that closes takes its gesture with it, along with the selection in the
//! terminal it closed.

use std::cell::Cell;
use std::rc::Rc;

/// Whether a selection is being dragged out of one pane's output.
///
/// Cheap to clone — it is an [`Rc`] — because the element that draws the grid
/// takes one every frame.
#[derive(Clone, Default)]
pub struct PaneSelection(Rc<Cell<bool>>);

impl PaneSelection {
    /// A pane with no gesture in progress.
    pub fn new() -> Self {
        Self::default()
    }

    /// Says a press landed on this pane's output, so the moves that follow are
    /// this pane's to act on.
    pub fn begin(&self) {
        self.0.set(true);
    }

    /// Whether a press on this pane's output has not been released yet.
    pub fn is_dragging(&self) -> bool {
        self.0.get()
    }

    /// Ends the gesture, reporting whether there was one to end.
    ///
    /// The answer is what makes a release meaningful: only the pane that took
    /// the press has anything to do with the button coming up again.
    pub fn end(&self) -> bool {
        self.0.replace(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gesture_belongs_to_whichever_pane_the_press_landed_on() {
        let pressed = PaneSelection::new();
        let untouched = PaneSelection::new();

        assert!(!pressed.is_dragging(), "nothing is being dragged to start");
        pressed.begin();

        assert!(pressed.is_dragging());
        assert!(
            !untouched.is_dragging(),
            "a press on one pane started a drag in another"
        );

        assert!(
            pressed.end(),
            "the release belongs to the pane that was pressed"
        );
        assert!(!pressed.is_dragging());
        assert!(
            !untouched.end(),
            "a pane with no gesture has no release to claim"
        );
    }

    #[test]
    fn the_element_that_is_handed_a_clone_is_looking_at_the_same_gesture() {
        // The whole reason it is an `Rc`: the tree that saw the press has been
        // thrown away by the time the drag arrives.
        let kept = PaneSelection::new();
        let handed_to_the_element = kept.clone();

        handed_to_the_element.begin();
        assert!(kept.is_dragging());
    }
}
