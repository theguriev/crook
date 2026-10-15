//! The pictures programs print, decoded off the thread that draws and kept
//! at the size they are drawn.
//!
//! The emulator keeps a picture as the PNG it arrived as (see
//! [`crook_terminal::image`]), because a file is a fraction of its pixels and
//! a block may be scrolled to once a day. The pixels are made here, the first
//! time a frame draws the picture — but not by that frame. Decoding a
//! 4096-pixel PNG takes tens of milliseconds, and a screen of them decoded
//! where the frame is drawn was a window that stopped answering. So the frame
//! hands the file to a pool of its own and draws the room the picture will
//! take (see [`Shown::Waiting`]); the decode, once it lands, asks for the
//! frame that draws it, through [`wake_with`] — the way a redraw is asked for
//! from a thread that is not the window's.
//!
//! # At the size it is drawn
//!
//! A picture is resampled on the pool, straight after it is decoded, to the
//! device pixels it covers: a 4096-pixel square in two cells weighs a few
//! kilobytes here rather than sixty-four megabytes, and the renderer, which
//! would otherwise resample it to that size on the thread that draws, finds
//! it done. A picture drawn at a new size — the font grew, the window moved
//! to another display — is decoded again at it, and drawn from the old pixels
//! until then.
//!
//! Short of pixels that would be made up, though: past an atlas row, a side
//! drawn longer than its file has pixels along it is decoded at the pixels
//! it has — or, for a picture smaller than an atlas row, as many as it has
//! grown to one — and stretched the rest of the way by the renderer, which
//! does that on the GPU as it draws (see [`decoded_size`]). Side by side, so
//! that a picture stretched out of its shape keeps every pixel of the side
//! it has them for. So no picture is decoded longer on a side than the
//! longer of its file's side and an atlas row, whatever font it is zoomed
//! to — a 4096-pixel square at most, a quarter of the budget, which
//! therefore always has room for it.
//!
//! # Within a budget, and never at the frame's expense
//!
//! What the pictures weigh together is held to [`BUDGET`]. Room is made by
//! letting go of the picture drawn longest ago, but never of one this frame
//! has drawn: every entry is stamped with the last [`FrameId`] that asked for
//! it. When the frame's own pictures fill the budget, the next one waits —
//! drawn as its room — rather than evict one drawn a moment before, which, in
//! a frame that draws them in the same order every time, was every picture
//! decoded again every frame. A picture whose terminal has let go of it — its
//! block evicted from the history, its pane closed — is let go of here too,
//! once the frame after it is drawn: see [`forget_the_gone`].
//!
//! On the UI thread alone, which is the only one that paints, so the cache is
//! the thread's own and nothing is locked to read it; what crosses to the pool
//! and back is one slot per decode.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use crook_terminal::InlineImage;
use crookui_core::executor::Background;
use crookui_core::image::Bitmap;
use crookui_core::scene::FrameId;

use crate::picture::{self, Limits};

/// What the decoded pictures may weigh together, in pixels' bytes.
///
/// Pixels at the size they are drawn, so the pictures on one screen weigh
/// what the screen does — a 5K display's worth is sixty megabytes — and this
/// is room for a few screens of them to be scrolled back to without a decode.
const BUDGET: usize = 256 << 20;

/// The longest side a picture is decoded at exactly as it is drawn, however
/// few pixels its file has: an image atlas row, give or take the two pixels
/// crookui's renderer keeps clear of the atlas's edge.
///
/// Within an atlas row the renderer resamples whatever it is handed to the
/// drawn size, on the thread that draws, so a picture drawn there is grown
/// here, on the pool, to spare it that. Past an atlas row it stretches what
/// it is handed on the GPU instead, and a picture grown further than its own
/// pixels here would be megabytes that say nothing the stretch does not.
const EXACT_SIDE: u32 = 1024;

// The largest picture there is, decoded at its largest, fits the budget
// twice over: once as the pixels a frame draws, and once more as the decode
// at a new size that replaces them. So one picture is never left waiting for
// room the budget cannot make.
const _: () = assert!(
    EXACT_SIDE <= crook_terminal::image::MAX_SIDE
        && 2 * bytes((
            crook_terminal::image::MAX_SIDE,
            crook_terminal::image::MAX_SIDE
        )) <= BUDGET
);

/// What decoding one picture may allocate: a 4096-pixel square in RGBA,
/// twice over — the decoder's rows and the output — and the chunks around
/// them.
const LIMITS: Limits = Limits {
    max_side: crook_terminal::image::MAX_SIDE,
    bytes: 160 << 20,
    square: false,
};

/// How many pictures are decoded at once.
///
/// A pool of their own rather than the application's, which also runs the
/// settings save a click waits for: a screen of thirty pictures is thirty
/// decodes queued, and they wait behind each other here rather than in front
/// of that. Two, because each holds a picture's full-size pixels while it
/// works — up to the [`LIMITS`] — and two keep a screen of them coming
/// without holding a screen of them at full size at once.
const DECODERS: usize = 2;

/// What a frame draws where a picture goes.
#[derive(Clone, Debug)]
pub enum Shown {
    /// The pixels: at the size the frame asked for, short of
    /// [`decoded_size`]'s cap; or — between a change of that size and the
    /// decode at the new one — at the size they last were. Either way the
    /// caller draws them at the size it asked for. The renderer stretches or
    /// shrinks them to it on the GPU, unless that size fits an atlas row and
    /// they fit an atlas, where it resamples them on the thread that draws: a
    /// few megabytes made out of a few, once for that size.
    Picture(Arc<Bitmap>),
    /// Nothing to draw yet: it is being decoded, or waits for room in the
    /// budget. The caller draws the room it will take.
    Waiting,
    /// Nothing, ever: its file does not decode, or it is drawn at no size.
    Nothing,
}

/// What `frame` draws for `image`, drawn `drawn` device pixels big — see
/// [`crookui_core::image::device_size`].
///
/// Never decodes: a picture with no pixels yet is handed to the pool, once,
/// and is [`Shown::Waiting`] until the first frame after its decode lands.
pub fn shown(image: &Arc<InlineImage>, drawn: (u32, u32), frame: FrameId) -> Shown {
    CACHE.with_borrow_mut(|cache| cache.shown(image, drawn, frame))
}

/// The pixels a picture `own` pixels big is decoded at, drawn `drawn` big.
///
/// `drawn` itself while neither side is longer than [`EXACT_SIDE`]. Past
/// that, each side is `drawn`'s held to [`most_pixels`] along it, and the
/// renderer stretches the rest. Side by side rather than with `drawn`'s shape
/// kept, because the two shapes need not be one: a picture stretched into
/// its cells is drawn in theirs, and a photo stretched wide and short, capped
/// as a whole by its width, was decoded at a fraction of the rows its cells
/// cover and blown up soft to fill them. And a picture fitted a pixel past
/// its own size is decoded at its own size, rather than at one a rounding
/// short of it on the other side. A side never shrinks to nothing.
fn decoded_size(drawn: (u32, u32), own: (u32, u32)) -> (u32, u32) {
    if drawn.0.max(drawn.1) <= EXACT_SIDE {
        return drawn;
    }
    let most = most_pixels(own);
    (drawn.0.min(most.0).max(1), drawn.1.min(most.1).max(1))
}

/// The most pixels a picture `own` pixels big is decoded at along each side,
/// once it is drawn past an atlas row.
///
/// Its own, or — for a picture shorter than [`EXACT_SIDE`] — its own grown,
/// shape kept, until the longest is that: as far as the pool grows it within
/// an atlas row, and so no further past one.
fn most_pixels(own: (u32, u32)) -> (u32, u32) {
    let longest = own.0.max(own.1).max(1);
    if longest >= EXACT_SIDE {
        return own;
    }
    // The product before the quotient, rounded, so the longest side comes
    // out the row exactly and the other the nearest pixel to its share.
    let side = |pixels: u32| {
        let scaled = (u64::from(pixels) * u64::from(EXACT_SIDE) + u64::from(longest) / 2)
            / u64::from(longest);
        (scaled as u32).max(1)
    };
    (side(own.0), side(own.1))
}

/// Says how to ask for a frame once a decode has landed.
///
/// Called on the pool's thread, so it is a message to the event loop and
/// nothing more — a window's redraw, asked for through its proxy. Until it is
/// set a decode lands silently and is drawn by whatever frame comes next.
pub fn wake_with(wake: impl Fn() + Send + Sync + 'static) {
    CACHE.with_borrow_mut(|cache| cache.wake_with(wake));
}

/// Lets go of the pixels of every picture its terminal has let go of.
///
/// The first frame that draws a picture does this anyway; this is for the
/// frames that draw none, after the pane they were printed in closed.
pub fn forget_the_gone() {
    CACHE.with_borrow_mut(Cache::forget_the_gone);
}

/// Whether a decode this thread asked for has not landed yet.
pub fn decoding() -> bool {
    CACHE.with_borrow(Cache::decoding)
}

/// Waits up to `timeout` for every decode this thread asked for to land, and
/// says whether they all did.
///
/// For what draws a frame and has no event loop to be woken by: a snapshot,
/// and a test. A window waits for nothing; it is woken.
pub fn settle(timeout: Duration) -> bool {
    CACHE.with_borrow(|cache| cache.settle(timeout))
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::new(BUDGET));
}

/// The pool the pictures are decoded on, started the first time one is.
///
/// The process's, and never dropped, rather than the cache's: dropping a
/// [`Background`] joins its workers, and the cache is a thread-local, dropped
/// as its thread exits. On Windows a thread-local's destructor runs under the
/// loader lock, which a worker has to take to exit, so that join waited for
/// good — and held up every thread starting or exiting in the process behind
/// it, which stopped the whole test binary at the first test thread to draw
/// a picture.
static POOL: OnceLock<Background> = OnceLock::new();

/// The decoded pictures, and what they weigh.
struct Cache {
    entries: HashMap<u64, Entry>,
    /// What the entries weigh together: the pixels drawn, and those being
    /// decoded, counted from the moment they are asked for so that a screen
    /// of decodes cannot be started past the budget.
    weight: usize,
    budget: usize,
    /// The last frame that asked for anything.
    frame: Option<FrameId>,
    wake: Arc<dyn Fn() + Send + Sync>,
    flight: Arc<Flight>,
}

/// One picture: what is drawn for it, what is being decoded for it, and the
/// last frame that asked.
struct Entry {
    /// The picture, held weakly: the terminal's blocks own it, and the entry
    /// goes when they let go of it.
    image: Weak<InlineImage>,
    pixels: Option<Arc<Bitmap>>,
    decoding: Option<Decoding>,
    /// Its file did not decode, which is remembered so that a broken picture
    /// costs one attempt and not one a frame.
    broken: bool,
    frame: FrameId,
}

/// A decode on the pool: the size it makes, and where it lands.
struct Decoding {
    size: (u32, u32),
    slot: Arc<Slot>,
}

/// Where a decode lands: the pixels, or `None` for a file that would not
/// decode.
///
/// The entry holds it, and the job only weakly: an entry let go of before
/// its job ran — a picture scrolled away and evicted, a pane closed — leaves
/// a job that finds nobody waiting and decodes nothing.
type Slot = OnceLock<Option<Arc<Bitmap>>>;

/// How many decodes are out, for [`settle`] to wait on.
#[derive(Default)]
struct Flight {
    out: Mutex<usize>,
    landed: Condvar,
}

/// One decode, as the pool runs it.
struct Job {
    image: Weak<InlineImage>,
    size: (u32, u32),
    slot: Weak<Slot>,
    wake: Arc<dyn Fn() + Send + Sync>,
    flight: Arc<Flight>,
}

impl Cache {
    fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            weight: 0,
            budget,
            frame: None,
            wake: Arc::new(|| {}),
            flight: Arc::default(),
        }
    }

    fn wake_with(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Arc::new(wake);
    }

    fn decoding(&self) -> bool {
        *self.flight.out() > 0
    }

    fn settle(&self, timeout: Duration) -> bool {
        self.flight.settle(timeout)
    }

    fn shown(&mut self, image: &Arc<InlineImage>, drawn: (u32, u32), frame: FrameId) -> Shown {
        if self.frame != Some(frame) {
            self.frame = Some(frame);
            self.forget_the_gone();
        }
        if drawn.0 == 0 || drawn.1 == 0 {
            return Shown::Nothing;
        }
        // Everything below is at the size it is decoded at, not drawn at: a
        // picture zoomed past its cap is the same pixels at every step, and
        // compared with the size drawn it would be decoded again every frame.
        let size = decoded_size(drawn, image.size());

        let id = image.id();
        let Some(mut entry) = self.take(id) else {
            if !self.make_room(bytes(size), frame) {
                return Shown::Waiting;
            }
            let decoding = self.decode(image, size);
            self.put(
                id,
                Entry {
                    image: Arc::downgrade(image),
                    pixels: None,
                    decoding: Some(decoding),
                    broken: false,
                    frame,
                },
            );
            return Shown::Waiting;
        };

        let asked_before = entry.frame != frame;
        entry.frame = frame;
        entry.absorb();
        if entry.broken {
            self.put(id, entry);
            return Shown::Nothing;
        }

        // Once a frame at most, so that a picture somehow drawn at two sizes
        // in one frame is not decoded at each in turn, frame after frame.
        if asked_before && entry.making() != Some(size) {
            let held = entry.pixels.as_deref().map_or(0, weight);
            if self.make_room(held + bytes(size), frame) {
                entry.decoding = Some(self.decode(image, size));
            }
        }

        let shown = match &entry.pixels {
            Some(pixels) => Shown::Picture(pixels.clone()),
            None => Shown::Waiting,
        };
        self.put(id, entry);
        shown
    }

    /// Takes an entry out, and its weight with it.
    ///
    /// Out of the map while it is worked on, which is also what keeps
    /// [`Self::make_room`] from evicting the picture it is making room for.
    fn take(&mut self, id: u64) -> Option<Entry> {
        let entry = self.entries.remove(&id)?;
        self.weight -= entry.weight();
        Some(entry)
    }

    fn put(&mut self, id: u64, entry: Entry) {
        self.weight += entry.weight();
        self.entries.insert(id, entry);
    }

    /// Lets go of pictures last drawn before `frame`, longest ago first,
    /// until `needed` more fits the budget; or says it cannot, because what
    /// is left is this frame's own.
    fn make_room(&mut self, needed: usize, frame: FrameId) -> bool {
        while self.weight.saturating_add(needed) > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.frame < frame && entry.weight() > 0)
                .min_by_key(|(_, entry)| entry.frame)
                .map(|(id, _)| *id)
            else {
                return false;
            };
            self.take(oldest);
        }
        true
    }

    /// Drops the entries of pictures nobody holds any more.
    fn forget_the_gone(&mut self) {
        let mut freed = 0;
        self.entries.retain(|_, entry| {
            let held = entry.image.strong_count() > 0;
            if !held {
                freed += entry.weight();
            }
            held
        });
        self.weight -= freed;
    }

    /// Hands `image` to the pool to be decoded at `size`.
    fn decode(&mut self, image: &Arc<InlineImage>, size: (u32, u32)) -> Decoding {
        let slot = Arc::new(Slot::new());
        let job = Job {
            image: Arc::downgrade(image),
            size,
            slot: Arc::downgrade(&slot),
            wake: self.wake.clone(),
            flight: self.flight.clone(),
        };
        *self.flight.out() += 1;
        POOL.get_or_init(|| Background::new(DECODERS))
            .spawn(async move { job.run() })
            .detach();
        Decoding { size, slot }
    }
}

impl Entry {
    /// What the entry weighs: the pixels drawn, and the ones on their way.
    fn weight(&self) -> usize {
        self.pixels.as_deref().map_or(0, weight)
            + self
                .decoding
                .as_ref()
                .map_or(0, |decoding| bytes(decoding.size))
    }

    /// The size the picture is being decoded at, or else the size it is
    /// drawn from.
    fn making(&self) -> Option<(u32, u32)> {
        match &self.decoding {
            Some(decoding) => Some(decoding.size),
            None => self.pixels.as_deref().map(Bitmap::size),
        }
    }

    /// Takes in a decode that has landed, in place of the pixels it was
    /// decoded to replace.
    fn absorb(&mut self) {
        let Some(landed) = self
            .decoding
            .as_ref()
            .and_then(|decoding| decoding.slot.get())
            .cloned()
        else {
            return;
        };
        self.decoding = None;
        self.broken = landed.is_none();
        self.pixels = landed;
    }
}

impl Flight {
    /// The count, whatever a panicking holder left it at: it is a number,
    /// and one a job had not finished changing is still the number of jobs.
    fn out(&self) -> MutexGuard<'_, usize> {
        self.out
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn land(&self) {
        *self.out() -= 1;
        self.landed.notify_all();
    }

    fn settle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut out = self.out();
        while *out > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            out = self
                .landed
                .wait_timeout(out, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        true
    }
}

impl Job {
    fn run(self) {
        // The slot is filled before the window is woken, so the frame this
        // asks for finds the pixels there; and the flight lands last, so that
        // a `settle` it ends has seen everything the job did.
        if self.decode() {
            (self.wake)();
        }
        self.flight.land();
    }

    /// Decodes into the slot, unless the picture or the entry waiting for it
    /// went while the job waited for a worker. Says whether it did.
    fn decode(&self) -> bool {
        let Some(image) = self.image.upgrade() else {
            return false;
        };
        if self.slot.strong_count() == 0 {
            return false;
        }
        // A decoder that panics on a crafted file costs that picture and not
        // a worker: the job would never land, and the picture would wait for
        // it, framed, for the rest of the session.
        let (png, size) = (image.png(), self.size);
        let decoded = match panic::catch_unwind(|| picture::decode_at(png, LIMITS, size)) {
            Ok(Ok(bitmap)) => Some(Arc::new(bitmap)),
            Ok(Err(why)) => {
                log::debug!("a picture a program printed {why}");
                None
            }
            Err(_) => {
                log::warn!("a picture a program printed panicked its decoder");
                None
            }
        };
        let Some(slot) = self.slot.upgrade() else {
            return false;
        };
        // Each decode has a slot of its own, so it is filled once.
        let _ = slot.set(decoded);
        true
    }
}

/// What a picture `size` pixels big weighs, in RGBA.
const fn bytes(size: (u32, u32)) -> usize {
    (size.0 as usize)
        .saturating_mul(size.1 as usize)
        .saturating_mul(4)
}

/// What decoded pixels weigh.
fn weight(bitmap: &Bitmap) -> usize {
    bitmap.canvas().pixels.len()
}

#[cfg(test)]
#[path = "inline_image_tests.rs"]
mod tests;
