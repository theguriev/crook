//! The PowerBook Duo pirate: six frames of a grin opening into a mouthful
//! of teeth, photographed off a Duo 280c's screen.
//!
//! An experiment beside [`crate::pirate`], not a replacement for him: the
//! frames are pictures rather than marks, so they keep their own colours and
//! do not take the theme's, and they are drawn by [`Image`] at whatever size
//! the caller names. Each was cut to its circle and shrunk to 48 px, which is
//! a 16 px mark at 3x and nearly five times the 10 px a row's corner
//! draws him at.

use std::sync::{Arc, OnceLock};

use crookui_core::geometry::vec2f;
use crookui_core::image::Bitmap;
use crookui_core::prelude::*;

/// The frames, in the order the grin opens.
const FRAMES: [&[u8]; 6] = [
    include_bytes!("../assets/duo/1.png"),
    include_bytes!("../assets/duo/2.png"),
    include_bytes!("../assets/duo/3.png"),
    include_bytes!("../assets/duo/4.png"),
    include_bytes!("../assets/duo/5.png"),
    include_bytes!("../assets/duo/6.png"),
];

/// The grin opening and closing again, so the loop has no seam: shut to the
/// full mouthful of teeth and back, the shut frame once.
pub const CYCLE: [usize; 10] = [0, 1, 2, 3, 4, 5, 4, 3, 2, 1];

/// Every frame, decoded once for the life of the process.
///
/// Decoded rather than drawn as a mark because they are photographs, and
/// held as one `Arc` each so that every row and every frame names the same
/// bitmap and the renderer resamples it once per size, not once per draw.
pub fn frames() -> &'static [Arc<Bitmap>] {
    static FRAMES_DECODED: OnceLock<Vec<Arc<Bitmap>>> = OnceLock::new();
    FRAMES_DECODED.get_or_init(|| {
        FRAMES
            .iter()
            .map(|png| {
                Arc::new(
                    crate::picture::decode(png, crate::picture::Limits::ICON)
                        .expect("a frame built into the binary decodes"),
                )
            })
            .collect()
    })
}

/// The Duo pirate `step` steps into his grin, `size` across.
pub fn mark(step: usize, size: f32) -> Box<dyn Element> {
    let bitmap = frames()[CYCLE[step % CYCLE.len()]].clone();
    Image::new(bitmap, vec2f(size, size)).finish()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_frame_built_into_the_binary_decodes() {
        assert_eq!(super::frames().len(), 6);
        for frame in super::frames() {
            assert_eq!(frame.size(), (48, 48));
        }
    }
}
