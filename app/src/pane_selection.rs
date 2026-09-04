//! The pointer gesture in one pane's output, as the view holds it between
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
//! [`PaneInput`](crate::pane_input::PaneInput) and every mouse state — and a
//! pane that closes takes its gesture with it, along with the selection in the
//! terminal it closed.
//!
//! # Why the gesture has a kind
//!
//! A press on a pane means one of two entirely different things depending on
//! what is running in it. With no program reading the mouse it starts a
//! selection; with `vim`, `htop` or `tmux` in the pane it is a click that
//! belongs to the program, and the moves that follow are reports rather than a
//! selection being dragged out.
//!
//! Which of the two it was is decided **once, when the button goes down**, and
//! remembered here. Asking the terminal again on every move would be asking a
//! question whose answer can change mid-drag: a program that turns mouse
//! reporting off while a button is held would leave the release unreported and
//! a selection half-dragged out of a screen nobody selected in.

use std::cell::Cell;
use std::rc::Rc;

/// What the button that went down on a pane's output is doing.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Gesture {
    /// Nothing is held.
    #[default]
    None,
    /// A selection is being dragged out of the output.
    Selecting,
    /// The gesture belongs to a program that asked for the mouse. Every move
    /// and the release are reported to it, and nothing is selected.
    Reporting,
}

/// The pointer gesture one pane's output has open.
///
/// Cheap to clone — it is an [`Rc`] — because the element that draws the grid
/// takes one every frame.
#[derive(Clone, Default)]
pub struct PaneSelection(Rc<Cell<Gesture>>);

impl PaneSelection {
    /// A pane with no gesture in progress.
    pub fn new() -> Self {
        Self::default()
    }

    /// Says a press landed on this pane's output and started a selection, so
    /// the moves that follow are this pane's to act on.
    pub fn begin(&self) {
        self.0.set(Gesture::Selecting);
    }

    /// Says a press landed and was given to a program that reads the mouse.
    pub fn begin_reporting(&self) {
        self.0.set(Gesture::Reporting);
    }

    /// What the open gesture is, if there is one.
    pub fn gesture(&self) -> Gesture {
        self.0.get()
    }

    /// Whether a selection is being dragged out of this pane.
    pub fn is_dragging(&self) -> bool {
        self.0.get() == Gesture::Selecting
    }

    /// Whether the open gesture belongs to a program reading the mouse.
    pub fn is_reporting(&self) -> bool {
        self.0.get() == Gesture::Reporting
    }

    /// Whether any button that went down here is still down.
    pub fn is_open(&self) -> bool {
        self.0.get() != Gesture::None
    }

    /// Ends the gesture, reporting what it was.
    ///
    /// The answer is what makes a release meaningful: only the pane that took
    /// the press has anything to do with the button coming up again, and only
    /// it knows whether that release is a program's or a selection's.
    pub fn end(&self) -> Gesture {
        self.0.replace(Gesture::None)
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

        assert_eq!(
            pressed.end(),
            Gesture::Selecting,
            "the release belongs to the pane that was pressed"
        );
        assert!(!pressed.is_dragging());
        assert_eq!(
            untouched.end(),
            Gesture::None,
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

    #[test]
    fn a_press_a_program_took_is_never_mistaken_for_a_selection() {
        // The distinction the whole type exists to keep: a drag inside `vim`
        // must not leave a highlight behind it, and the release must reach the
        // program rather than ending a selection nobody made.
        let gesture = PaneSelection::new();
        gesture.begin_reporting();

        assert!(gesture.is_reporting());
        assert!(gesture.is_open());
        assert!(
            !gesture.is_dragging(),
            "a click a program took is not a selection being dragged"
        );
        assert_eq!(gesture.end(), Gesture::Reporting);
    }
}
