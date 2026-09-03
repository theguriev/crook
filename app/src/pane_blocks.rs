//! Where a pane's block list is scrolled to, and which block the pointer is
//! over, kept between the frames that draw it.
//!
//! The same reason [`PaneInput`](crate::pane_input::PaneInput) and
//! [`PaneSelection`](crate::pane_selection::PaneSelection) exist: the element
//! tree is thrown away and rebuilt on every render, so an offset stored in it
//! would snap back to the top the first moment scrolling caused a repaint —
//! which is every moment. The workspace keeps one of these per pane and hands
//! the list a clone each frame.
//!
//! # Scroll position is a mode, not a number
//!
//! [`ScrollPosition::FollowBottom`] re-resolves to the current maximum every
//! frame. Storing "the bottom" as the float it happened to be would leave a
//! person one line short the moment the next line arrived, and stickiness
//! would die silently. Everything that moves the position goes through
//! [`PaneBlocks::apply`] with a named [`ScrollCause`], so "does typing snap me
//! back to the bottom?" has one readable answer rather than five scattered
//! assignments.
//!
//! # Heights are fractional lines, and are compared with a tolerance
//!
//! A block's height is its rows plus paddings measured in fractions of a cell,
//! summed through a prefix array. Exact float comparison over such a sum
//! produces spurious scrollbars, off-by-one visible items and seeks that fail
//! at the very ends, so every comparison here goes through
//! [`HEIGHT_TOLERANCE`] — including the "is there anything to scroll at all"
//! test.

use std::cell::RefCell;
use std::rc::Rc;

use crook_terminal::BlockId;
use crookui_core::geometry::Vector2F;

/// How close two heights have to be to count as equal, in lines.
///
/// Warp's number. It is one constant rather than one per comparison so that
/// the scrollbar, the seek and the follow-bottom test cannot disagree about
/// what "at the end" means.
pub const HEIGHT_TOLERANCE: f32 = 0.01;

/// Where a list is scrolled to.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum ScrollPosition {
    /// Pinned to the end of the list, wherever that turns out to be this
    /// frame. A block growing under it is followed with no auto-scroll step at
    /// all.
    #[default]
    FollowBottom,
    /// A fixed distance from the top of the list, in lines.
    ///
    /// From the *top* rather than from the bottom, so that a command printing
    /// below does not drag the rows somebody is reading up the screen.
    Fixed(f32),
}

/// Why a scroll position is being changed.
///
/// Every mutation names one. That is the whole point: the rules are a match
/// arm each in [`PaneBlocks::apply`] rather than scattered over an element.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ScrollCause {
    /// The wheel turned over the list, by this many lines. Positive is
    /// further down the list.
    Wheel(f32),
    /// The list was measured against a box or a content height it had not
    /// seen.
    Resize,
    /// A command line was handed to the shell.
    Submit,
    /// A keystroke reached the pty.
    KeyToPty,
}

/// What says a block list is the same one it was last frame: the pointer its
/// history arrived behind, how many blocks were in it, and how many had been
/// dropped off the front.
///
/// All three, because none of them is enough alone — a freed history's address
/// can be reused, and a list can lose a block off the front and gain one on the
/// end in the same batch.
pub type Identity = (usize, usize, usize);

/// A prefix sum of item heights, in lines.
///
/// `start(i)` is where item `i` begins and [`Self::total`] is the sum, so
/// [`Self::seek`] answers "which item is at line *y*" with a binary search and
/// the list never measures an item it is not about to draw.
///
/// Kept across frames and rebuilt only when the list of finished blocks
/// actually changed. The live block is the last item and is rewritten every
/// frame, which is one addition, because it is the one item whose height moves
/// while nothing else does.
#[derive(Debug, Default)]
pub struct Heights {
    /// One entry per item plus a final total. Empty when there are no items.
    starts: Vec<f32>,
    /// What the finished part of `starts` was built from. See
    /// [`Self::sync`].
    built_from: Option<Identity>,
}

impl Heights {
    /// How many items are indexed.
    pub fn len(&self) -> usize {
        self.starts.len().saturating_sub(1)
    }

    /// Whether nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The whole list's height, in lines.
    pub fn total(&self) -> f32 {
        self.starts.last().copied().unwrap_or(0.)
    }

    /// Where item `index` begins, in lines from the top of the list.
    pub fn start(&self, index: usize) -> f32 {
        self.starts.get(index).copied().unwrap_or(0.)
    }

    /// How tall item `index` is, in lines.
    pub fn height(&self, index: usize) -> f32 {
        self.start(index + 1) - self.start(index)
    }

    /// The item that `line` falls inside, clamped into the list at both ends.
    ///
    /// Clamped rather than refused: a sum of a thousand fractional heights can
    /// exceed what the caller believes the total is by a rounding error, and a
    /// seek that failed at the very bottom would blank the list on the frame
    /// somebody scrolled to the end of it.
    pub fn seek(&self, line: f32) -> usize {
        if self.is_empty() {
            return 0;
        }
        // The last item whose start is at or before `line`. `partition_point`
        // counts the entries strictly before it, which is that index plus one.
        let after = self.starts.partition_point(|start| *start <= line);
        after.saturating_sub(1).min(self.len() - 1)
    }

    /// Rebuilds the sums when the finished blocks changed, and rewrites the
    /// live block's height either way.
    ///
    /// `identity` is what says the finished part is the same list it was last
    /// frame.
    pub fn sync(&mut self, identity: Identity, finished: impl Iterator<Item = f32>, live: f32) {
        if self.built_from != Some(identity) {
            self.starts.clear();
            self.starts.push(0.);
            let mut total = 0.;
            for height in finished {
                total += height;
                self.starts.push(total);
            }
            // A placeholder for the live block, overwritten immediately below.
            self.starts.push(total);
            self.built_from = Some(identity);
        }

        // The live block is always last, so its height is one addition rather
        // than a walk — which is what makes a frame of a printing command cost
        // nothing in a session with ten thousand blocks behind it.
        let items = self.starts.len();
        self.starts[items - 1] = self.starts[items - 2] + live;
    }
}

/// What one pane's block list keeps between frames.
#[derive(Debug, Default)]
struct State {
    position: ScrollPosition,
    /// The whole list's height in lines, from the last layout.
    content: f32,
    /// The box the list was given, in lines.
    viewport: f32,
    /// Where the pointer was last seen, in window coordinates, or `None` when
    /// it is somewhere else entirely.
    ///
    /// Kept because the list moves under a pointer that does not: the wheel, a
    /// command finishing and a resize all change what is under it with no
    /// mouse event to say so, and the block that shows a copy control has to
    /// be the block a click would land on.
    pointer: Option<Vector2F>,
    /// The block the pointer is over, whose copy control is therefore drawn.
    hovered: Option<BlockId>,
    /// Whether the pointer is on that control rather than merely on its block.
    on_control: bool,
    /// The block whose control a press landed on, so that a release somewhere
    /// else is not a click.
    pressed: Option<BlockId>,
    heights: Heights,
}

/// Shared ownership of one pane's block-list state.
///
/// Cheap to clone — it is an [`Rc`] — because the element that draws the list
/// takes one every frame.
#[derive(Clone, Default)]
pub struct PaneBlocks(Rc<RefCell<State>>);

impl PaneBlocks {
    /// A list at the bottom, with nothing hovered.
    pub fn new() -> Self {
        Self::default()
    }

    /// Where the list is scrolled to, as a mode.
    pub fn position(&self) -> ScrollPosition {
        self.0.borrow().position
    }

    /// How far down the list is scrolled, in lines, resolved against the last
    /// measurement.
    pub fn offset(&self) -> f32 {
        let state = self.0.borrow();
        match state.position {
            ScrollPosition::FollowBottom => max_offset(&state),
            ScrollPosition::Fixed(lines) => lines.clamp(0., max_offset(&state)),
        }
    }

    /// The largest offset that still has content under it, in lines.
    pub fn max_offset(&self) -> f32 {
        max_offset(&self.0.borrow())
    }

    /// Whether there is anything to scroll.
    pub fn is_scrollable(&self) -> bool {
        self.max_offset() > HEIGHT_TOLERANCE
    }

    /// Whether the list has content below its own fold: it is scrolled off its
    /// bottom rather than following it.
    ///
    /// Two things ask, and they have to agree or the pane contradicts itself:
    /// the rule above the composer is drawn exactly when there is a seam to
    /// mark, and the composer only continues the shell's prompt line while the
    /// row above it really is the open block's last one. Both read the *last*
    /// layout's measurement, which is the only order that is not circular —
    /// the composer is measured before the list it would be told about.
    pub fn is_cut_off(&self) -> bool {
        self.max_offset() - self.offset() > HEIGHT_TOLERANCE
    }

    /// Records what layout measured and reconciles the position with it,
    /// reporting whether that moved the content.
    ///
    /// A pane made shorter, a block finishing, `clear`: all of them change
    /// what "the bottom" means, and all of them arrive here. A fixed offset
    /// that no longer has content under it falls back to following the bottom,
    /// which is [`ScrollCause::Resize`]'s whole rule.
    pub fn measured(&self, content: f32, viewport: f32) -> bool {
        {
            let mut state = self.0.borrow_mut();
            if (state.content - content).abs() < HEIGHT_TOLERANCE
                && (state.viewport - viewport).abs() < HEIGHT_TOLERANCE
            {
                return false;
            }
            state.content = content;
            state.viewport = viewport;
        }
        self.apply(ScrollCause::Resize)
    }

    /// Moves the list, reporting whether anything actually changed.
    ///
    /// The one mutator. Every rule about what sticks and what lets go is a
    /// match arm here.
    pub fn apply(&self, cause: ScrollCause) -> bool {
        let mut state = self.0.borrow_mut();
        let limit = max_offset(&state);
        let was = state.position;

        state.position = match cause {
            // Landing at the end returns to *following* it rather than pinning
            // the number that was the end a moment ago. Without this, somebody
            // who scrolls up to read and then scrolls back down finds the view
            // frozen while the command goes on printing.
            ScrollCause::Wheel(lines) => {
                let offset = match was {
                    ScrollPosition::FollowBottom => limit,
                    ScrollPosition::Fixed(at) => at,
                };
                let moved = (offset + lines).clamp(0., limit);
                if moved >= limit - HEIGHT_TOLERANCE {
                    ScrollPosition::FollowBottom
                } else {
                    ScrollPosition::Fixed(moved)
                }
            }
            // Typing and running a command both mean the person is done
            // reading history, and both must land on the live block.
            ScrollCause::Submit | ScrollCause::KeyToPty => ScrollPosition::FollowBottom,
            // The mode is kept. Only an offset that no longer fits gives way,
            // because a list that shrank under a fixed position has nothing at
            // that position any more.
            ScrollCause::Resize => match was {
                ScrollPosition::FollowBottom => ScrollPosition::FollowBottom,
                ScrollPosition::Fixed(at) if at <= limit + HEIGHT_TOLERANCE => {
                    ScrollPosition::Fixed(at.min(limit))
                }
                ScrollPosition::Fixed(_) => ScrollPosition::FollowBottom,
            },
        };

        moved(was, state.position, limit)
    }

    /// The block the pointer is over, if any.
    pub fn hovered(&self) -> Option<BlockId> {
        self.0.borrow().hovered
    }

    /// Whether the pointer is on the hovered block's copy control.
    pub fn is_on_control(&self) -> bool {
        self.0.borrow().on_control
    }

    /// Where the pointer was last seen over this list.
    pub fn pointer(&self) -> Option<Vector2F> {
        self.0.borrow().pointer
    }

    /// Records where the pointer is, or that it has left the list.
    pub fn point(&self, at: Option<Vector2F>) {
        self.0.borrow_mut().pointer = at;
    }

    /// Records what the pointer is over, reporting whether it moved.
    pub fn hover(&self, block: Option<BlockId>, on_control: bool) -> bool {
        let mut state = self.0.borrow_mut();
        let on_control = on_control && block.is_some();
        if state.hovered == block && state.on_control == on_control {
            return false;
        }
        state.hovered = block;
        state.on_control = on_control;
        true
    }

    /// Says a press landed on one block's copy control.
    pub fn press_control(&self, block: BlockId) {
        self.0.borrow_mut().pressed = Some(block);
    }

    /// Takes the press back, reporting the block it landed on.
    ///
    /// A release is only a click when it comes up on the control it went down
    /// on, which is what every other button in the application does.
    pub fn release_control(&self) -> Option<BlockId> {
        self.0.borrow_mut().pressed.take()
    }

    /// Runs `use_heights` against this list's prefix sums.
    ///
    /// Handed out rather than copied because they are rebuilt during layout
    /// and read during the same layout: the walk that finds the visible items
    /// happens inside this borrow, and what comes out of it is the small
    /// vector the element paints from.
    pub fn with_heights<T>(&self, use_heights: impl FnOnce(&mut Heights) -> T) -> T {
        use_heights(&mut self.0.borrow_mut().heights)
    }
}

/// The largest offset that still has content under it.
fn max_offset(state: &State) -> f32 {
    (state.content - state.viewport).max(0.)
}

/// Whether two positions resolve to different places.
fn moved(before: ScrollPosition, after: ScrollPosition, limit: f32) -> bool {
    let resolve = |position| match position {
        ScrollPosition::FollowBottom => limit,
        ScrollPosition::Fixed(at) => f32::clamp(at, 0., limit),
    };
    before != after && (resolve(before) - resolve(after)).abs() >= HEIGHT_TOLERANCE
}

#[cfg(test)]
#[path = "pane_blocks_tests.rs"]
mod tests;
