//! What the header's chip — "M to answer" and "N waiting" — remembers from
//! one frame to the next, which the `crook/tabs` plugin that draws it cannot
//! keep for itself: when its halves last moved, and whether a press may reach
//! them yet.
//!
//! # Why its presses wait
//!
//! The halves are counts, and each is there only while it has something to
//! count, so the chip changes shape under a still pointer whenever either
//! count does. "1 waiting" turning up — a bell in another tab, or a second
//! agent asking, which tends to come with a new question — is the change
//! that matters: a press aimed at what was there a moment before lands on
//! what is there now. "N waiting" focuses a pane whose agent's dialog may be
//! up with Yes as its default, and an Enter pressed next, expecting the
//! list, answers it.
//!
//! So "to answer" is drawn first, where a shift leaves it alone and a miss
//! lands on the list rather than on a pane; and for [`SETTLE`] after the
//! chip's counts change neither half has a handler. One one-shot wait, asked
//! for here and started by [`Workspace::frame_drawn`](super::Workspace::frame_drawn),
//! draws it live again — not a timer. A half that turns live is drawn with
//! mouse state of its own, so a press begun on it while it was inert cannot
//! be finished on it.

use std::cell::{Cell, RefCell};
use std::time::Instant;

use crookui_core::elements::MouseStateHandle;

use super::closing::SETTLE;

/// The header chip's state between frames. See the module.
#[derive(Default)]
pub(crate) struct HeaderChip {
    /// The counts it was last laid out with: questions to answer, then panes
    /// waiting. Both naught is no chip at all.
    counts: Cell<(usize, usize)>,
    /// When a press may first reach its halves: [`SETTLE`] after the counts
    /// last changed. None before they ever have.
    live_at: Cell<Option<Instant>>,
    /// Whether its halves were last drawn with their handlers.
    live: Cell<bool>,
    /// The moment the one wait that draws them live again is wanted for,
    /// until the frame after has started it.
    wake: Cell<Option<Instant>>,
    /// The halves' mouse state, "to answer" first.
    handles: RefCell<[MouseStateHandle; 2]>,
}

impl HeaderChip {
    /// Lays the chip out at `now` for `to_answer` questions and `waiting`
    /// panes, and says whether its halves may be pressed, with the mouse
    /// state to draw them with, "to answer" first.
    pub(crate) fn lay_out(
        &self,
        to_answer: usize,
        waiting: usize,
        now: Instant,
    ) -> (bool, [MouseStateHandle; 2]) {
        if self.counts.replace((to_answer, waiting)) != (to_answer, waiting) {
            let at = now + SETTLE;
            self.live_at.set(Some(at));
            self.wake.set(Some(at));
        }
        let live = self.live_at.get().is_none_or(|at| now >= at);
        if live && !self.live.get() {
            *self.handles.borrow_mut() = Default::default();
        }
        self.live.set(live);
        (live, self.handles.borrow().clone())
    }

    /// The moment a frame laid out since the last call asked to be drawn
    /// again at, once: what the one-shot wait is started for.
    pub(super) fn take_wake(&self) -> Option<Instant> {
        self.wake.take()
    }

    /// Lets a press reach the halves from the next frame on, as a second of
    /// the chip keeping still would — which a test says this way rather than
    /// by sleeping through it.
    #[cfg(test)]
    pub(super) fn settle(&self) {
        self.live_at.set(Some(Instant::now()));
    }
}
