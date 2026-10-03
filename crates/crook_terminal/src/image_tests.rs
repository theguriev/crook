use base64::Engine as _;

use super::*;
use crate::{Emulator, Palette, TerminalSize};

/// A PNG's signature and header, and nothing after: all this crate reads.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
    bytes
}

fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The sequence `imgcat` prints, with `args` before `inline=1`.
fn osc(args: &str, file: &[u8]) -> Vec<u8> {
    format!("\x1b]1337;File={args}inline=1:{}\x07", base64(file)).into_bytes()
}

fn requested(width: Dimension, height: Dimension, size: (u32, u32)) -> Requested {
    Requested {
        image: InlineImage::new(png(size.0, size.1)).expect("a PNG header"),
        width,
        height,
        stretch: false,
    }
}

#[test]
fn only_a_png_within_the_limits_is_a_picture() {
    assert_eq!(
        InlineImage::new(png(30, 20)).map(|image| image.size()),
        Some((30, 20))
    );
    assert!(InlineImage::new(b"GIF89a....".to_vec()).is_none());
    assert!(InlineImage::new(png(0, 20)).is_none());
    assert!(InlineImage::new(png(MAX_SIDE + 1, 20)).is_none());
}

#[test]
fn a_picture_takes_the_cells_its_pixels_need_and_no_more_than_the_pane() {
    // 10×20 cells: 35×50 pixels is four columns and three rows.
    let cell = (10, 20);
    let pane = (80, 24);
    let auto = requested(Dimension::Auto, Dimension::Auto, (35, 50));
    assert_eq!(cells(&auto, cell, pane), (4, 3));

    // Ten columns wide, and as tall as its shape makes that.
    let wide = requested(Dimension::Cells(10), Dimension::Auto, (200, 100));
    assert_eq!(cells(&wide, cell, pane), (10, 3));

    // Half the pane, by pixels and by share.
    let half = requested(Dimension::Percent(50), Dimension::Auto, (100, 100));
    assert_eq!(cells(&half, cell, pane).0, 40);
    let pixels = requested(Dimension::Pixels(100), Dimension::Auto, (100, 100));
    assert_eq!(cells(&pixels, cell, pane), (10, 5));

    // Fitted inside a box rather than stretched across it, unless asked.
    let mut boxed = requested(Dimension::Cells(20), Dimension::Cells(2), (100, 100));
    assert_eq!(cells(&boxed, cell, pane), (4, 2));
    boxed.stretch = true;
    assert_eq!(cells(&boxed, cell, pane), (20, 2));

    // A picture bigger than the pane is shrunk into it, keeping its shape.
    let huge = requested(Dimension::Auto, Dimension::Auto, (4000, 1000));
    assert_eq!(cells(&huge, cell, pane), (80, 10));

    // A pane that has not said how big its cells are is given some.
    assert_eq!(cells(&auto, (0, 0), pane), (5, 4));
}

#[test]
fn the_arguments_are_read_and_a_download_is_not_a_picture() {
    let mut reader = Reader::default();
    let read = |reader: &mut Reader, bytes: Vec<u8>| {
        // What `vte` hands over: the payload cut at every `;`.
        let body = &bytes[2..bytes.len() - 1];
        let params: Vec<&[u8]> = body.split(|byte| *byte == b';').collect();
        reader.read(&params)
    };

    let picture = read(
        &mut reader,
        osc("width=10;preserveAspectRatio=0;", &png(4, 4)),
    )
    .expect("an inline PNG");
    assert_eq!(picture.width, Dimension::Cells(10));
    assert_eq!(picture.height, Dimension::Auto);
    assert!(picture.stretch);

    let download = format!("\x1b]1337;File=name=eA==:{}\x07", base64(&png(4, 4)));
    assert!(read(&mut reader, download.into_bytes()).is_none());

    // In pieces, as `imgcat` sends it inside tmux; the base64 split anywhere.
    let encoded = base64(&png(6, 3));
    let (first, second) = encoded.split_at(9);
    assert!(
        read(
            &mut reader,
            b"\x1b]1337;MultipartFile=inline=1;height=2\x07".to_vec()
        )
        .is_none()
    );
    assert!(
        read(
            &mut reader,
            format!("\x1b]1337;FilePart={first}\x07").into_bytes()
        )
        .is_none()
    );
    assert!(
        read(
            &mut reader,
            format!("\x1b]1337;FilePart={second}\x07").into_bytes()
        )
        .is_none()
    );
    let joined = read(&mut reader, b"\x1b]1337;FileEnd\x07".to_vec()).expect("the parts joined");
    assert_eq!(joined.image.size(), (6, 3));
    assert_eq!(joined.height, Dimension::Cells(2));
}

#[test]
fn a_picture_is_placed_at_the_cursor_and_the_cursor_moves_past_it() {
    let mut emulator = Emulator::new(
        TerminalSize::new(20, 10).with_cell_size(10, 20),
        100,
        Palette::default(),
    );
    // 30×40 pixels: three columns, two rows.
    emulator.advance(b"before\r\n");
    emulator.advance(&osc("", &png(30, 40)));
    emulator.advance(b"after");

    let snapshot = emulator.snapshot();
    let images = &snapshot.live_block.images;
    assert_eq!(images.len(), 1);
    let placed = &images[0];
    assert_eq!((placed.row, placed.column), (1, 0));
    assert_eq!((placed.columns, placed.rows), (3, 2));
    // On the picture's last row, in the column after it.
    let text = snapshot.text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[2].trim_end(), "   after");
}

#[test]
fn a_finished_block_keeps_its_picture_at_its_own_row() {
    let mut emulator = Emulator::new(
        TerminalSize::new(20, 6).with_cell_size(10, 20),
        100,
        Palette::default(),
    );
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07imgcat x\r\n\x1b]133;C\x07");
    emulator.advance(b"one\r\ntwo\r\n");
    emulator.advance(&osc("", &png(20, 60)));
    emulator.advance(b"\r\n");
    // Enough output after it to scroll it off the screen: the anchor follows.
    emulator.advance(b"a\r\nb\r\nc\r\nd\r\ne\r\n");
    emulator.advance(b"\x1b]133;D;0\x07\x1b]133;A\x07$ ");

    let block = &emulator.blocks()[0];
    assert_eq!(block.images.len(), 1);
    let placed = &block.images[0];
    // The prompt's row, then `one` and `two`, then the picture.
    assert_eq!(placed.row, 3);
    assert_eq!((placed.columns, placed.rows), (2, 3));
    assert!(emulator.snapshot().live_block.images.is_empty());
}

#[test]
fn clear_and_the_alternate_screen_take_no_pictures() {
    let mut emulator = emulator_with_cells();
    emulator.advance(&osc("", &png(10, 10)));
    assert_eq!(emulator.snapshot().live_block.images.len(), 1);
    emulator.advance(b"\x1b[H\x1b[2J\x1b[3J");
    assert!(emulator.snapshot().live_block.images.is_empty());

    emulator.advance(b"\x1b[?1049h");
    emulator.advance(&osc("", &png(10, 10)));
    emulator.advance(b"\x1b[?1049l");
    assert!(emulator.snapshot().live_block.images.is_empty());
}

fn emulator_with_cells() -> Emulator {
    Emulator::new(
        TerminalSize::new(20, 6).with_cell_size(10, 20),
        100,
        Palette::default(),
    )
}
