//! The pictures programs print, decoded once and kept while they are drawn.
//!
//! The emulator keeps a picture as the PNG it arrived as (see
//! [`crook_terminal::image`]), because a file is a fraction of its pixels and
//! a block may be scrolled to once a day. The pixels are made here, the first
//! time a frame draws the picture, and kept for the frames after it — up to a
//! budget, past which the picture drawn longest ago is let go and decoded
//! again if it is scrolled back to.
//!
//! On the UI thread alone, which is the only one that paints, so the cache is
//! the thread's own and nothing is locked to read it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crook_terminal::InlineImage;
use crookui_core::image::Bitmap;

use crate::picture::{self, Limits};

/// What the decoded pictures may weigh together, in pixels' bytes.
const BUDGET: usize = 256 << 20;

/// What decoding one picture may allocate: a 4096-pixel square in RGBA,
/// twice over — the decoder's rows and the output — and the chunks around
/// them.
const LIMITS: Limits = Limits {
    max_side: crook_terminal::image::MAX_SIDE,
    bytes: 160 << 20,
    square: false,
};

/// One decoded picture, or the reason it could not be, and when it was last
/// asked for.
struct Entry {
    bitmap: Option<Arc<Bitmap>>,
    used: u64,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<u64, Entry>,
    weight: usize,
    clock: u64,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
}

/// `image`'s pixels, decoded now if they have not been, or `None` when its
/// file does not decode — which is remembered, so a broken picture costs one
/// attempt and not one a frame.
pub fn bitmap(image: &InlineImage) -> Option<Arc<Bitmap>> {
    CACHE.with_borrow_mut(|cache| {
        cache.clock += 1;
        let clock = cache.clock;
        if let Some(entry) = cache.entries.get_mut(&image.id()) {
            entry.used = clock;
            return entry.bitmap.clone();
        }

        let bitmap = match picture::decode(image.png(), LIMITS) {
            Ok(bitmap) => Some(Arc::new(bitmap)),
            Err(why) => {
                log::debug!("a picture a program printed {why}");
                None
            }
        };
        cache.weight += weight(bitmap.as_deref());
        cache.entries.insert(
            image.id(),
            Entry {
                bitmap: bitmap.clone(),
                used: clock,
            },
        );
        // The one just decoded stays, however big: it is on screen.
        while cache.weight > BUDGET {
            let Some(oldest) = cache
                .entries
                .iter()
                .filter(|(id, _)| **id != image.id())
                .min_by_key(|(_, entry)| entry.used)
                .map(|(id, _)| *id)
            else {
                break;
            };
            if let Some(entry) = cache.entries.remove(&oldest) {
                cache.weight -= weight(entry.bitmap.as_deref());
            }
        }
        bitmap
    })
}

fn weight(bitmap: Option<&Bitmap>) -> usize {
    bitmap.map_or(0, |bitmap| bitmap.canvas().pixels.len())
}
