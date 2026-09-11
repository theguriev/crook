//! A picture the scene can draw: decoded pixels with an identity.
//!
//! Everything else the scene holds is a *reference* — a glyph key, an icon
//! key — that the renderer turns into pixels on its own. A picture cannot be:
//! nothing in the renderer knows what a plugin's icon looks like, so the
//! pixels have to travel with the scene. They travel behind an `Arc`, and the
//! [`ImageId`] is what lets the renderer tell "the same bitmap again" from "a
//! new bitmap that happens to be the same size" without comparing a single
//! byte.
//!
//! This module decodes nothing. Which file format a picture arrives in is the
//! application's business; the one contract here is straight-alpha RGBA8, the
//! bytes a PNG decoder hands out and the bytes the glyph shader emits
//! unchanged.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::fonts::{Canvas, RasterFormat};

/// The identity of one decoded picture, for the renderer's cache to key on.
///
/// Minted from a counter and never reused, so a cache entry for a bitmap that
/// has been dropped can never be answered for a bitmap that was decoded later.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct ImageId(u64);

impl ImageId {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// A straight-alpha RGBA8 picture with an identity.
///
/// The canvas inside is exactly what an atlas upload wants — `Rgba32`, rows
/// packed with no padding — and the constructors refuse anything else, so the
/// renderer never has to check. Straight alpha, not premultiplied: the glyph
/// pipeline blends with a straight-alpha state and passes a colour texel
/// through untouched, so a premultiplied picture would come out with dark
/// fringes on every soft edge.
#[derive(Clone)]
pub struct Bitmap {
    id: ImageId,
    canvas: Canvas,
}

impl Bitmap {
    /// Wraps a canvas, refusing one the atlas could not take as it is.
    ///
    /// A padded row stride is refused rather than repacked because nothing in
    /// Crook produces one for a picture: a decoder writes rows tightly, and a
    /// canvas that does not is a bug worth hearing about.
    pub fn new(canvas: Canvas) -> Result<Self, String> {
        let (width, height) = canvas.size;
        if canvas.format != RasterFormat::Rgba32 {
            return Err(format!(
                "a bitmap is RGBA, and this canvas is {:?}",
                canvas.format
            ));
        }
        if width == 0 || height == 0 {
            return Err(format!(
                "a bitmap needs at least one pixel, not {width}×{height}"
            ));
        }
        let expected_stride = width as usize * RasterFormat::Rgba32.bytes_per_pixel() as usize;
        if canvas.row_stride != expected_stride {
            return Err(format!(
                "a bitmap's rows are packed, so a {width}-wide one has a stride of \
                 {expected_stride}, not {}",
                canvas.row_stride
            ));
        }
        let expected_len = expected_stride * height as usize;
        if canvas.pixels.len() != expected_len {
            return Err(format!(
                "a {width}×{height} bitmap holds {expected_len} bytes, not {}",
                canvas.pixels.len()
            ));
        }

        Ok(Self {
            id: ImageId::new(),
            canvas,
        })
    }

    /// A bitmap from tightly packed straight-alpha RGBA8 rows, top row first.
    ///
    /// This is the constructor for a decoded file: `pixels` is what a decoder
    /// hands back, and it must hold exactly `width * height * 4` bytes.
    pub fn rgba8(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, String> {
        Self::new(Canvas {
            pixels,
            size: (width, height),
            row_stride: width as usize * RasterFormat::Rgba32.bytes_per_pixel() as usize,
            format: RasterFormat::Rgba32,
        })
    }

    /// The identity the renderer caches this bitmap under.
    pub fn id(&self) -> ImageId {
        self.id
    }

    /// Width and height in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.canvas.size
    }

    /// The pixels, in the shape an atlas upload takes.
    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }
}

impl fmt::Debug for Bitmap {
    // The pixels are not worth printing: a failed assertion about a picture
    // is about its size or its identity, and a megabyte of bytes hides both.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (width, height) = self.size();
        f.debug_struct("Bitmap")
            .field("id", &self.id)
            .field("size", &format_args!("{width}×{height}"))
            .finish()
    }
}

/// `bitmap` at another size: area-averaged when shrinking, bilinear when
/// growing, decided per axis.
///
/// Pure CPU, and deliberately so: the atlas sampler filters linearly with no
/// mipmaps, so a picture it shrank by more than half would alias, and the
/// renderer's rule that a quad is drawn at exactly its atlas region's size
/// depends on the region already being the drawn size. Resampling here is what
/// keeps that rule true for pictures.
///
/// The averaging happens in premultiplied space and is un-premultiplied on
/// the way out, so a transparent pixel — whatever colour a decoder left in its
/// RGB — bleeds nothing into its opaque neighbours. A side of zero is raised
/// to one rather than refused: the caller has already decided the picture is
/// drawn, and a bitmap has at least one pixel.
pub fn resample(bitmap: &Bitmap, width: u32, height: u32) -> Bitmap {
    let (source_width, source_height) = bitmap.size();
    let width = width.max(1);
    let height = height.max(1);

    let columns = weights(source_width, width);
    let rows = weights(source_height, height);

    // Premultiplied floats, one pass per axis: horizontal first, into a
    // buffer that is the target width and the source height.
    let source = bitmap.canvas();
    let mut across = vec![[0f32; 4]; width as usize * source_height as usize];
    for y in 0..source_height as usize {
        let row = &source.pixels[y * source.row_stride..][..source_width as usize * 4];
        let target = &mut across[y * width as usize..][..width as usize];
        for (x, taps) in columns.iter().enumerate() {
            let mut sum = [0f32; 4];
            for &(index, weight) in taps {
                let texel = premultiplied(&row[index * 4..][..4]);
                for (channel, value) in sum.iter_mut().zip(texel) {
                    *channel += value * weight;
                }
            }
            target[x] = sum;
        }
    }

    let stride = width as usize * 4;
    let mut pixels = vec![0u8; stride * height as usize];
    for (y, taps) in rows.iter().enumerate() {
        let target = &mut pixels[y * stride..][..stride];
        for x in 0..width as usize {
            let mut sum = [0f32; 4];
            for &(index, weight) in taps {
                let texel = across[index * width as usize + x];
                for (channel, value) in sum.iter_mut().zip(texel) {
                    *channel += value * weight;
                }
            }
            target[x * 4..][..4].copy_from_slice(&straight(sum));
        }
    }

    Bitmap::rgba8(width, height, pixels)
        .expect("a resample writes exactly the bytes its size calls for")
}

/// Which source indices feed each target index along one axis, and by how
/// much. Every target's weights sum to one.
fn weights(source: u32, target: u32) -> Vec<Vec<(usize, f32)>> {
    let last = (source - 1) as usize;
    (0..target)
        .map(|index| {
            if target == source {
                vec![(index as usize, 1.)]
            } else if target < source {
                // Shrinking: each target pixel covers a span of source pixels,
                // and every source pixel contributes the fraction of the span
                // it overlaps — a box filter, which is what "area average"
                // means once the span's edges fall inside pixels.
                let scale = source as f32 / target as f32;
                let start = index as f32 * scale;
                let end = start + scale;
                let first = start.floor() as usize;
                let past = (end.ceil() as usize).min(source as usize);
                (first..past)
                    .map(|pixel| {
                        let overlap =
                            (end.min(pixel as f32 + 1.) - start.max(pixel as f32)) / scale;
                        (pixel, overlap.max(0.))
                    })
                    .collect()
            } else {
                // Growing: sample between the two source pixel centres the
                // target centre falls between. Past the edges both taps land
                // on the edge pixel, which is what clamp-to-edge means.
                let centre = (index as f32 + 0.5) * source as f32 / target as f32 - 0.5;
                let left = centre.floor();
                let blend = centre - left;
                let tap = |pixel: f32| pixel.clamp(0., last as f32) as usize;
                vec![(tap(left), 1. - blend), (tap(left + 1.), blend)]
            }
        })
        .collect()
}

fn premultiplied(texel: &[u8]) -> [f32; 4] {
    let alpha = texel[3] as f32 / 255.;
    [
        texel[0] as f32 * alpha,
        texel[1] as f32 * alpha,
        texel[2] as f32 * alpha,
        texel[3] as f32,
    ]
}

fn straight(texel: [f32; 4]) -> [u8; 4] {
    let alpha = texel[3];
    let channel = |value: f32| {
        if alpha <= 0. {
            0
        } else {
            (value / alpha * 255.).round().clamp(0., 255.) as u8
        }
    };
    [
        channel(texel[0]),
        channel(texel[1]),
        channel(texel[2]),
        alpha.round().clamp(0., 255.) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(bitmap: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let canvas = bitmap.canvas();
        let start = y as usize * canvas.row_stride + x as usize * 4;
        canvas.pixels[start..start + 4]
            .try_into()
            .expect("four channels")
    }

    #[test]
    fn a_bitmap_is_rgba_with_packed_rows_or_it_is_refused() {
        assert!(Bitmap::rgba8(2, 1, vec![0; 8]).is_ok());
        assert!(
            Bitmap::rgba8(2, 1, vec![0; 7]).is_err(),
            "a byte short of two pixels"
        );
        assert!(Bitmap::rgba8(0, 1, Vec::new()).is_err(), "no pixels at all");

        let padded = Canvas {
            pixels: vec![0; 12],
            size: (2, 1),
            row_stride: 12,
            format: RasterFormat::Rgba32,
        };
        assert!(Bitmap::new(padded).is_err(), "a padded row");

        let mask = Canvas {
            pixels: vec![0; 2],
            size: (2, 1),
            row_stride: 2,
            format: RasterFormat::A8,
        };
        assert!(Bitmap::new(mask).is_err(), "a coverage mask");
    }

    #[test]
    fn every_bitmap_has_an_identity_of_its_own() {
        let first = Bitmap::rgba8(1, 1, vec![0; 4]).unwrap();
        let second = Bitmap::rgba8(1, 1, vec![0; 4]).unwrap();

        assert_ne!(
            first.id(),
            second.id(),
            "the same bytes twice are two pictures, not one"
        );
        assert_eq!(
            first.clone().id(),
            first.id(),
            "and a clone is the same picture"
        );
    }

    #[test]
    fn shrinking_a_checkerboard_averages_it_to_grey() {
        let mut pixels = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                let value = if (x + y) % 2 == 0 { 0 } else { 255 };
                pixels.extend_from_slice(&[value, value, value, 255]);
            }
        }
        let board = Bitmap::rgba8(4, 4, pixels).unwrap();

        let small = resample(&board, 2, 2);

        assert_eq!(small.size(), (2, 2));
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(
                    pixel(&small, x, y),
                    [128, 128, 128, 255],
                    "each 2×2 block is two black and two white pixels"
                );
            }
        }
        assert_ne!(small.id(), board.id(), "a resample is a new picture");
    }

    #[test]
    fn growing_a_single_pixel_gives_a_solid() {
        let dot = Bitmap::rgba8(1, 1, vec![10, 20, 30, 255]).unwrap();

        let big = resample(&dot, 3, 5);

        assert_eq!(big.size(), (3, 5));
        for y in 0..5 {
            for x in 0..3 {
                assert_eq!(pixel(&big, x, y), [10, 20, 30, 255]);
            }
        }
    }

    #[test]
    fn growing_clamps_to_the_edge_rather_than_blending_across_it() {
        // Red beside blue, grown wide: the first column's sample point falls
        // left of the first pixel's centre, and what lies past the edge is
        // the edge, not the neighbour on the other side.
        let pair = Bitmap::rgba8(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();

        let wide = resample(&pair, 100, 1);

        assert_eq!(pixel(&wide, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&wide, 99, 0), [0, 0, 255, 255]);
        assert_eq!(
            pixel(&wide, 50, 0)[0],
            pixel(&wide, 49, 0)[2],
            "and the middle blends the two symmetrically"
        );
    }

    #[test]
    fn a_transparent_pixel_bleeds_no_colour_into_its_neighbour() {
        // Transparent red beside opaque white. Averaged straight, the result
        // would be pink; averaged premultiplied, the red is weighted by an
        // alpha of zero and contributes nothing but its transparency.
        let pair = Bitmap::rgba8(2, 1, vec![255, 0, 0, 0, 255, 255, 255, 255]).unwrap();

        let one = resample(&pair, 1, 1);

        assert_eq!(pixel(&one, 0, 0), [255, 255, 255, 128]);
    }

    #[test]
    fn shrinking_one_axis_while_growing_the_other_is_decided_per_axis() {
        // Two columns, one row: the horizontal pass averages, the vertical
        // pass spreads the one row down.
        let pair = Bitmap::rgba8(2, 1, vec![0, 0, 0, 255, 200, 200, 200, 255]).unwrap();

        let tall = resample(&pair, 1, 3);

        for y in 0..3 {
            assert_eq!(pixel(&tall, 0, y), [100, 100, 100, 255]);
        }
    }

    #[test]
    fn a_zero_side_becomes_one_rather_than_nothing() {
        let dot = Bitmap::rgba8(1, 1, vec![1, 2, 3, 4]).unwrap();

        assert_eq!(resample(&dot, 0, 0).size(), (1, 1));
    }

    #[test]
    fn the_weights_along_an_axis_always_sum_to_one() {
        for (source, target) in [(4, 2), (2, 4), (7, 3), (3, 7), (5, 5), (1, 9), (1000, 1)] {
            for taps in weights(source, target) {
                let total: f32 = taps.iter().map(|(_, weight)| weight).sum();
                assert!(
                    (total - 1.).abs() < 1e-4,
                    "{source} -> {target}: weights {taps:?} sum to {total}"
                );
                assert!(
                    taps.iter().all(|(index, _)| *index < source as usize),
                    "{source} -> {target}: a tap reached past the source"
                );
            }
        }
    }
}
