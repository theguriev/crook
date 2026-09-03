//! What a pane draws its shell as, and whether it draws a composer under it.
//!
//! One function, [`of`], answers both questions from one [`Snapshot`], and
//! every element that needs either asks it rather than testing a flag of its
//! own. That is the whole point of the module: "is there a field?" is asked by
//! the body when it builds the column, by the grid when it decides whether to
//! fill its cursor, and by the workspace when it decides whether a caret is
//! worth blinking for — and three copies of the rule is how a pane ends up
//! with two cursors, or with a text field under `top`.
//!
//! # The two surfaces
//!
//! * **The block list.** The ordinary one: finished commands as a list, with
//!   the open one at the bottom.
//! * **The grid.** One [`Snapshot`] painted straight, scrolled through the
//!   emulator's own history. A pane falls back to it in two cases, and both
//!   are the same case seen from different sides: the alternate screen, and an
//!   open block that has grown past the top of the emulator's viewport.
//!
//!   The second is what a shell with *no* integration looks like from its
//!   first screenful onwards — one block holding everything, most of it in the
//!   emulator's history rather than in the snapshot — and it is also where a
//!   program that addresses the whole primary screen ends up. That is the
//!   right place for it: a `less` redrawing row four means row four of the
//!   *screen*, not of a block with paddings above it.
//!
//! # The composer, separately
//!
//! It goes away wherever the grid is drawn — on the alternate screen and on an
//! overflowing block alike, because the pty is sized from the pane and a grid
//! squeezed above a field would lose its last rows under it — and it goes away
//! once the open block has been running for longer than [`LONG_RUNNING`], at
//! which point the block takes the space it leaves and every key goes to the
//! program.
//!
//! "Running" means the shell said so. A line handed to a shell that reports no
//! marks is never answered, and a rule that counted it as running would take
//! the field away at the first Enter of such a session and never give it back.
//! See [`BlockState::is_running`](crook_terminal::BlockState::is_running).
//!
//! Without that second rule, `less` on the primary screen is a block that
//! grows for ever with a text field underneath it that the program cannot see
//! and the person cannot use. Warp classifies a command as long-running fifty
//! milliseconds after it starts and hides the composer for exactly this
//! reason; the number is small enough that no command a person waits for is
//! missed and large enough that `ls` never flickers the composer away and
//! back.
//!
//! What that rule does *not* do is throw the block list away. A `cargo build`
//! printing for a minute is still one command among the ones before it, and a
//! program that genuinely owns the screen reaches the grid by the overflow
//! rule above, which is the honest way to get there.
//!
//! # Where the composer's first row goes is a separate question
//!
//! This module answers *whether* there is one. Whether that one continues the
//! shell's own prompt line — and at which column — is `inline_start`, in
//! `workspace::block_list`, because the answer is about the list's geometry
//! rather than the snapshot's: which row it drew last, and whether it is
//! scrolled to its end. It takes the [`PaneSurface`] from here as one of its
//! three facts.

use std::time::{Duration, Instant};

use crook_terminal::Snapshot;

/// How long a command runs before the composer goes away and its block takes
/// the space.
///
/// Warp's fifty milliseconds. Anything faster than this has already finished
/// by the time a frame could show it, so the composer never blinks; anything
/// slower is something a person is watching rather than typing under.
pub const LONG_RUNNING: Duration = Duration::from_millis(50);

/// What a pane draws its output as.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Surface {
    /// The block list: finished commands, then the open one.
    Blocks,
    /// One grid, painted straight from the snapshot and scrolled through the
    /// emulator's own history.
    Grid,
}

/// What a pane draws: its output surface, and whether a composer goes under
/// it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PaneSurface {
    /// What the output area is.
    pub surface: Surface,
    /// Whether a command is being composed under it.
    pub composer: bool,
}

impl PaneSurface {
    /// Whether the output, rather than a field under it, is where typing would
    /// go.
    ///
    /// The cursor's business, and only the cursor's. A surface with a composer
    /// under it draws no cursor at all — the composer's caret is the one — and
    /// a surface without one draws the shell's, filled where the pane has the
    /// keyboard and hollow where it does not. That is how a pane never shows
    /// two cursors and never shows none.
    pub fn output_owns_caret(self) -> bool {
        !self.composer
    }
}

/// What `snapshot` should be drawn as, as of `now`.
///
/// `now` is a parameter rather than read here so that a test can put a command
/// past the long-running threshold without sleeping through it.
pub fn of(snapshot: &Snapshot, now: Instant) -> PaneSurface {
    // A full-screen program owns the grid, there is no line to compose for it,
    // and none of the block marks are believed while it is up.
    if snapshot.alt_screen {
        return PaneSurface {
            surface: Surface::Grid,
            composer: false,
        };
    }

    // The open block starts above the viewport, so the snapshot no longer
    // holds all of it. Painting it as a block would silently drop everything
    // that scrolled off; painting it as the grid gives the wheel back the
    // emulator's scrollback, which is where those rows actually are. That is
    // also where a program addressing the whole primary screen ends up, and it
    // is the right place for it: a `less` redrawing row four means row four of
    // the *screen*, not of a block with paddings above it.
    let overflowing = snapshot.live_block.top_row < 0;
    PaneSurface {
        surface: if overflowing {
            Surface::Grid
        } else {
            Surface::Blocks
        },
        // **The grid never has a composer under it.** The pty is sized from
        // the pane, so a grid drawn into the pane *minus a composer* has fewer
        // rows to draw than the child was told it had, and the newest ones —
        // the prompt, the line being echoed — are under the field and never
        // painted at all. It is not a rule worth softening either: this
        // surface is the one for "there is no block structure here", and the
        // honest input for that is the pty itself, which is what every key
        // goes to once there is no field.
        //
        // Otherwise the composer goes away while a command runs, and the block
        // takes the space it leaves. The list stays: a `cargo build` printing
        // for a minute is still one command among the ones before it, and
        // throwing the session's blocks away for the duration would be a worse
        // answer than the text field this rule exists to remove.
        composer: !overflowing && !is_long_running(snapshot, now),
    }
}

/// Whether the open block has been running long enough to take the pane.
///
/// A block that reports no start time is not running as far as this is
/// concerned, whatever its state says: the threshold is measured from the
/// start, and there is nothing to measure from.
pub fn is_long_running(snapshot: &Snapshot, now: Instant) -> bool {
    if !snapshot.live_block.state.is_running() {
        return false;
    }
    snapshot
        .live_block
        .started_at
        .is_some_and(|started| now.saturating_duration_since(started) >= LONG_RUNNING)
}

/// How long until the open block would become long-running, or `None` when it
/// is not running or already is.
///
/// What a repaint has to be scheduled after: a `sleep 5` submitted into a
/// quiet shell produces no further output, so nothing else would ever come
/// along to draw the frame in which the composer goes away.
pub fn until_long_running(snapshot: &Snapshot, now: Instant) -> Option<Duration> {
    if !snapshot.live_block.state.is_running() {
        return None;
    }
    let started = snapshot.live_block.started_at?;
    // Zero would say "redraw immediately" for a block that has *already*
    // crossed the threshold, which is a repaint loop rather than a schedule.
    LONG_RUNNING
        .checked_sub(now.saturating_duration_since(started))
        .filter(|left| !left.is_zero())
}

#[cfg(test)]
#[path = "pane_surface_tests.rs"]
mod tests;
