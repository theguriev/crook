//! Turning a PNG a plugin carries into pixels the scene can draw.
//!
//! The one place in the application that decodes an image. `crookui_core`
//! holds the [`Bitmap`] and deliberately knows no file format; a plugin's
//! icon and previews arrive as PNG bytes out of its module, and this is where
//! they become straight-alpha RGBA8 — the one shape the renderer takes.
//!
//! # The bytes came from a stranger
//!
//! A module is something somebody downloaded, so its pictures are decoded
//! under a ceiling rather than trusted: the decoder is handed a byte budget it
//! may not allocate past, the header is read *before* any pixel is and a
//! picture whose stated size is past the rule is refused there, and an APNG
//! is refused because a picture on a card holds still. What the registry's
//! reader already checked is checked again, because the registry is not the
//! only way a module reaches this machine.

use crookui_core::image::{Bitmap, resample};

/// What a picture may cost before it is decoded.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most either side may be, read out of the header before decoding.
    pub max_side: u32,
    /// The most the decoder may allocate.
    pub bytes: usize,
    /// Whether the picture has to be as wide as it is tall.
    pub square: bool,
}

impl Limits {
    /// An icon: square, at most 256 a side, and a few megabytes of decoder.
    ///
    /// The same numbers `crook_plugin_api::pictures` names for the module,
    /// which the registry refuses a build over; here they are the ceiling a
    /// module that arrived some other way is held to.
    pub const ICON: Self = Self {
        max_side: crook_plugin_api::pictures::MAX_ICON_EDGE,
        bytes: 4 << 20,
        square: true,
    };

    /// A preview: any shape, at most 2048 a side.
    ///
    /// Thirty-two megabytes is what a 2048-square RGBA picture needs twice
    /// over — the decoder's rows and the output — with room for the chunks
    /// around them.
    pub const PREVIEW: Self = Self {
        max_side: crook_plugin_api::pictures::MAX_PREVIEW_EDGE,
        bytes: 32 << 20,
        square: false,
    };
}

/// The long edge a preview is kept to once decoded.
///
/// A preview is captured at 2x and drawn at half its pixel count, and the
/// widest a card is drawn is 560 logical pixels — so 1024 device pixels is
/// already more than a 1x display will ever read of it and exactly what a 2x
/// one reads at the card's full measure. Past that the pixels are memory in
/// an atlas for nothing.
const PREVIEW_LONG_EDGE: u32 = 1024;

/// Decodes a PNG into a bitmap, or says why it will not.
///
/// Every colour type a PNG can hold — grey, grey with alpha, palette, RGB,
/// RGBA, at any depth — comes out as 8-bit RGBA, because that is the one
/// format the atlas takes and a picture is decoded once per session.
pub fn decode(png: &[u8], limits: Limits) -> Result<Bitmap, String> {
    let mut decoder = png::Decoder::new_with_limits(
        std::io::Cursor::new(png),
        png::Limits {
            bytes: limits.bytes,
        },
    );
    decoder.set_transformations(
        png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
    );

    // The header first, and the rule checked against it before a pixel is
    // decoded: a header claiming twenty thousand pixels a side is refused for
    // what it says rather than for what it would cost to find out.
    let header = decoder
        .read_header_info()
        .map_err(|why| format!("is not a PNG: {why}"))?;
    let (width, height) = (header.width, header.height);
    if width == 0 || height == 0 {
        return Err(format!("is {width}\u{d7}{height}, which has no pixels"));
    }
    if width > limits.max_side || height > limits.max_side {
        return Err(format!(
            "is {width}\u{d7}{height} and a side is at most {}",
            limits.max_side
        ));
    }
    if limits.square && width != height {
        return Err(format!("is {width}\u{d7}{height} and an icon is square"));
    }

    let mut reader = decoder
        .read_info()
        .map_err(|why| format!("is not a PNG: {why}"))?;
    if reader.info().animation_control().is_some() {
        return Err(String::from("is animated, and a picture holds still"));
    }

    let size = reader
        .output_buffer_size()
        .ok_or_else(|| String::from("is larger than a picture may be"))?;
    let mut pixels = vec![0; size];
    let frame = reader
        .next_frame(&mut pixels)
        .map_err(|why| format!("could not be decoded: {why}"))?;
    pixels.truncate(frame.buffer_size());

    let rgba = widened(pixels, reader.output_color_type())?;
    Bitmap::rgba8(width, height, rgba)
}

/// Decodes a preview and keeps it to [`PREVIEW_LONG_EDGE`].
///
/// Resampled here, on whichever thread decoded it, rather than left for the
/// renderer: the renderer resamples to the drawn size on every new size, and a
/// 2048-wide source would be a megabyte of work per frame the card first
/// appears at, where a 1024-wide one is a quarter of it.
pub fn decode_preview(png: &[u8]) -> Result<Bitmap, String> {
    let bitmap = decode(png, Limits::PREVIEW)?;
    let (width, height) = bitmap.size();
    let long = width.max(height);
    if long <= PREVIEW_LONG_EDGE {
        return Ok(bitmap);
    }

    // The product before the quotient, so a 2000×1000 picture comes out
    // exactly 1024×512 and not a pixel short of it.
    let fitted_width = (u64::from(width) * u64::from(PREVIEW_LONG_EDGE) / u64::from(long)) as u32;
    let fitted_height = (u64::from(height) * u64::from(PREVIEW_LONG_EDGE) / u64::from(long)) as u32;
    Ok(resample(&bitmap, fitted_width.max(1), fitted_height.max(1)))
}

/// `pixels` as RGBA, whatever the decoder produced.
///
/// The transformations asked for above leave four possible outputs, all
/// 8-bit; the three narrower ones are widened here. A depth other than eight
/// cannot arrive, and is refused rather than assumed away.
fn widened(pixels: Vec<u8>, color: (png::ColorType, png::BitDepth)) -> Result<Vec<u8>, String> {
    use png::{BitDepth, ColorType};

    let per_pixel = match color {
        (ColorType::Rgba, BitDepth::Eight) => return Ok(pixels),
        (ColorType::Rgb, BitDepth::Eight) => 3,
        (ColorType::GrayscaleAlpha, BitDepth::Eight) => 2,
        (ColorType::Grayscale, BitDepth::Eight) => 1,
        (color, depth) => {
            return Err(format!(
                "decoded as {color:?} at {depth:?} bits, which is not a picture this build draws"
            ));
        }
    };

    let mut rgba = Vec::with_capacity(pixels.len() / per_pixel * 4);
    for pixel in pixels.chunks_exact(per_pixel) {
        match per_pixel {
            3 => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
            2 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]),
            _ => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
        }
    }
    Ok(rgba)
}

#[cfg(test)]
#[path = "picture_tests.rs"]
pub(crate) mod tests;
