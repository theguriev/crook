//! The pirate, as the one piece of artwork the window draws on its own behalf.
//!
//! He started as the usage chip's mark and the chip moved out of the binary,
//! which left the artwork in [`crookui_core::icons`] with exactly one caller:
//! a plugin naming a frame of his bite. He has a second job now. A popup in
//! this window that is waiting on git — a listing being read, eight checkouts
//! being looked in, six of them being deleted — used to say so in a sentence
//! that did not move, and a sentence that does not move for two minutes is
//! what a hang looks like. So the wait is drawn as the pirate eating his way
//! along it, and this module is what both callers share: his two colours, the
//! order of his frames, and the two layers that make one of them.
//!
//! # Why a bite and not a spinner
//!
//! Every other tool has a spinner and it says one thing: "not yet". The bite
//! says two, because the pirate stands in a row of pellets that are the
//! checkouts themselves — one eaten for each one dealt with, the rest still
//! ahead of him — so the same picture says *how far* as well. A person with
//! eight checkouts going and a `target/` in each can see that it is the third
//! one taking its time, not the button that has stopped listening.
//!
//! # No timer of its own
//!
//! Nothing here ticks. The frame is whatever the caller says it is, and the
//! caller advances it from a chain of its own — the worktree menu's, which
//! runs only while that menu is waiting on something and parks a worker for
//! one frame at a time. The plugin renderer's rule is the same one: a plugin
//! that wants him chomping names a different frame on each render and asks to
//! be drawn again.

use std::time::Duration;

use crookui_core::geometry::Color;
use crookui_core::icons::{Art, Chomp};
use crookui_core::prelude::*;

use crate::theme::theme;

/// The pirate's yellow, and the black of the patch and the strap.
///
/// Constants rather than theme roles: this is a piece of artwork, like a logo,
/// and a pirate whose face took the palette's cast would stop being the mark
/// people recognise. The one thing a caller decides is what a *stale* pirate
/// looks like — see [`mark`].
pub const FACE: Color = Color::hex(0xf9d949);
/// See [`FACE`].
pub const INK: Color = Color::hex(0x151515);

/// The bite, ending where it starts, so that stopping on any frame boundary
/// stops on a whole face.
pub const CYCLE: [Chomp; 4] = [Chomp::Shut, Chomp::Open, Chomp::Wide, Chomp::Open];

/// How long one frame of the bite is held.
///
/// The plugin's number, so that the pirate in a popup and the pirate in the
/// header chew at the same speed.
pub const FRAME: Duration = Duration::from_millis(110);

/// Which frame of the bite is `frame` steps in.
///
/// Any count, because the caller keeps a counter rather than an index into
/// [`CYCLE`], and a counter that wrapped at four would be one more thing to
/// get wrong.
pub fn chomp_at(frame: usize) -> Chomp {
    CYCLE[frame % CYCLE.len()]
}

/// The pirate, at one frame of his bite, `size` across.
///
/// Two layers rather than one: a rasterized mark is a coverage mask and a mask
/// has one colour, so the yellow head and the black on it are drawn one over
/// the other. Neither is meaningful alone.
///
/// `muted` greys the **face** and leaves the ink alone; otherwise both keep
/// the colours they were drawn in. Greying the face is what says a reading is
/// stale — a picture that stayed bright beside a greyed-out number would be
/// the loudest thing in the row insisting it is current.
///
/// The ink stays dark, and the reason is what a two-layer mask is. The ink is
/// the eyepatch, the strap and the grin, drawn *on* the face; painting both in
/// one colour does not produce a grey pirate, it produces a plain disc with
/// nothing on it, because there is nothing left to tell the layers apart. That
/// shipped, and what it looked like on somebody's screen was a grey circle.
pub fn mark(chomp: Chomp, muted: bool, size: f32) -> Box<dyn Element> {
    let face = if muted { theme().text_muted } else { FACE };

    Stack::new()
        .with_child(
            Icon::new(Art::PirateFace(chomp), size)
                .with_color(face)
                .finish(),
        )
        .with_child(
            Icon::new(Art::PirateInk(chomp), size)
                .with_color(INK)
                .finish(),
        )
        .finish()
}
