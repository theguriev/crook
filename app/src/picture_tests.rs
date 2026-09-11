//! What a PNG decodes to, and which ones are refused before they are decoded.

use super::*;

/// A PNG of `width`×`height` in `color`, every pixel `pixel`.
///
/// Encoded by the same crate that decodes it, which is what makes the tests
/// about the *colour types* rather than about a hand-built file: the encoder
/// writes a palette or a grey channel the way any tool would.
pub(crate) fn png_of(width: u32, height: u32, color: png::ColorType, pixel: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        if color == png::ColorType::Indexed {
            // Two entries, and the pixel is an index into them.
            encoder.set_palette(vec![0, 0, 0, 200, 100, 50]);
            encoder.set_trns(vec![255, 128]);
        }
        let mut writer = encoder.write_header().expect("the header writes");
        let data: Vec<u8> = pixel
            .iter()
            .copied()
            .cycle()
            .take(pixel.len() * (width * height) as usize)
            .collect();
        writer.write_image_data(&data).expect("the pixels write");
    }
    bytes
}

/// A square RGBA PNG, the shape an icon is.
pub(crate) fn icon_png(edge: u32) -> Vec<u8> {
    png_of(edge, edge, png::ColorType::Rgba, &[200, 100, 50, 255])
}

/// An RGBA PNG of any shape, the shape a preview is.
pub(crate) fn preview_png(width: u32, height: u32) -> Vec<u8> {
    png_of(width, height, png::ColorType::Rgba, &[30, 60, 90, 255])
}

/// The signature and an IHDR claiming `width`×`height`, and nothing after.
///
/// Enough for the header to be read and not enough to decode, which is the
/// point: a refusal that came from decoding would be a refusal that had paid
/// for the decode.
pub(crate) fn header_only(width: u32, height: u32) -> Vec<u8> {
    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&ihdr);
    png.extend_from_slice(&crc(b"IHDR", &ihdr).to_be_bytes());
    png
}

/// The CRC a chunk carries, which the decoder checks on the header.
fn crc(name: &[u8], data: &[u8]) -> u32 {
    let mut register = u32::MAX;
    for byte in name.iter().chain(data) {
        register ^= u32::from(*byte);
        for _ in 0..8 {
            register = match register & 1 {
                1 => 0xedb8_8320 ^ (register >> 1),
                _ => register >> 1,
            };
        }
    }
    !register
}

#[test]
fn every_colour_type_comes_out_as_rgba8() {
    // The atlas takes one format, and a plugin author saves whatever their
    // tool saved: a grey mark, a palette from an optimiser, RGB with no
    // alpha. Each is the same picture once decoded.
    let cases: [(png::ColorType, &[u8], [u8; 4]); 5] = [
        (png::ColorType::Grayscale, &[77], [77, 77, 77, 255]),
        (
            png::ColorType::GrayscaleAlpha,
            &[77, 128],
            [77, 77, 77, 128],
        ),
        (png::ColorType::Rgb, &[200, 100, 50], [200, 100, 50, 255]),
        (
            png::ColorType::Rgba,
            &[200, 100, 50, 128],
            [200, 100, 50, 128],
        ),
        (png::ColorType::Indexed, &[1], [200, 100, 50, 128]),
    ];
    for (color, pixel, expected) in cases {
        let bitmap = decode(&png_of(4, 4, color, pixel), Limits::PREVIEW)
            .unwrap_or_else(|why| panic!("{color:?} {why}"));

        assert_eq!(bitmap.size(), (4, 4));
        assert_eq!(
            &bitmap.canvas().pixels[..4],
            &expected,
            "{color:?} decoded to the wrong pixel"
        );
        assert_eq!(bitmap.canvas().pixels.len(), 4 * 4 * 4);
    }
}

#[test]
fn an_animated_picture_is_refused() {
    // A card holds still. The refusal is by the `acTL` chunk, which comes
    // before the pixels, so it costs no decode.
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 4, 4);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(2, 0).expect("two frames");
        let mut writer = encoder.write_header().expect("the header writes");
        writer.write_image_data(&[0; 64]).expect("frame one");
        writer.write_image_data(&[0; 64]).expect("frame two");
    }

    let refusal = decode(&bytes, Limits::PREVIEW).expect_err("an APNG");

    assert!(refusal.contains("animated"), "{refusal}");
}

#[test]
fn a_header_past_the_rule_is_refused_before_anything_is_decoded() {
    // Twenty thousand a side is 1.6 GB of pixels, and the header says so
    // before a single one is read. The bytes here hold no pixels at all, so
    // a decoder that got as far as decoding would fail differently.
    let refusal = decode(&header_only(20_000, 20_000), Limits::PREVIEW).expect_err("too big");
    assert!(refusal.contains("20000\u{d7}20000"), "{refusal}");
    assert!(refusal.contains("at most 2048"), "{refusal}");

    // And an icon that is not square, by the same header.
    let refusal = decode(&header_only(64, 32), Limits::ICON).expect_err("not square");
    assert!(refusal.contains("64\u{d7}32"), "{refusal}");
    assert!(refusal.contains("square"), "{refusal}");

    // Whereas a preview may be any shape.
    let bitmap = decode(&preview_png(64, 32), Limits::PREVIEW).expect("any shape");
    assert_eq!(bitmap.size(), (64, 32));
}

#[test]
fn something_that_is_not_a_png_is_refused_with_a_line() {
    let refusal = decode(b"GIF89a....", Limits::ICON).expect_err("not a PNG");
    assert!(refusal.contains("is not a PNG"), "{refusal}");
}

#[test]
fn a_preview_is_kept_to_a_thousand_pixels_on_its_long_edge() {
    // What the card can draw of it at 2x, and not a pixel more in the atlas.
    let shrunk = decode_preview(&preview_png(2000, 1000)).expect("it decodes");
    assert_eq!(shrunk.size(), (1024, 512));

    // A tall one by the same rule.
    let shrunk = decode_preview(&preview_png(500, 2000)).expect("it decodes");
    assert_eq!(shrunk.size(), (256, 1024));

    // And one already under it is left as it is.
    let kept = decode_preview(&preview_png(800, 600)).expect("it decodes");
    assert_eq!(kept.size(), (800, 600));
}
