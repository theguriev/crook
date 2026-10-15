use std::time::Duration;

use base64::Engine as _;

use super::*;
use crate::blocks::{HISTORY_HEADROOM, MAX_IMAGE_BYTES, PICTURE_FLOOR};
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

/// Twenty columns by six rows, keeping `scrollback` lines of history, with
/// `lines` of output, then `ABOVE` and a picture two rows tall — and no shell
/// integration, so all of it is the one open block.
fn picture_printed_after(lines: usize, scrollback: usize) -> Emulator {
    let mut emulator = Emulator::new(
        TerminalSize::new(20, 6).with_cell_size(10, 20),
        scrollback,
        Palette::default(),
    );
    for line in 0..lines {
        emulator.advance(format!("log {line}\r\n").as_bytes());
    }
    emulator.advance(b"ABOVE\r\n");
    emulator.advance(&osc("width=4;height=2;", &png(8, 8)));
    emulator
}

/// [`picture_printed_after`], and then `BELOW`.
fn picture_after(lines: usize, scrollback: usize) -> Emulator {
    let mut emulator = picture_printed_after(lines, scrollback);
    emulator.advance(b"\r\nBELOW\r\n");
    emulator
}

/// The viewport row whose text starts with `prefix`.
fn row_saying(emulator: &mut Emulator, prefix: &str) -> i32 {
    let text = emulator.snapshot().text();
    let row = text.lines().position(|line| line.starts_with(prefix));
    row.unwrap_or_else(|| panic!("no row says {prefix:?}:\n{text}")) as i32
}

/// The viewport row of the first line the grid still holds, on the screen
/// or above it in the history, whose text starts with `prefix`: negative in
/// the history, the way an open block's picture rows are. `None` once that
/// line has left the history.
fn line_saying(emulator: &Emulator, prefix: &str) -> Option<i32> {
    let history = emulator.history_len();
    let last = history + usize::from(emulator.size().rows) - 1;
    let (rows, first) = emulator.harvest_rows(0, last);
    (0..rows.rows())
        .find(|row| rows.text(*row).starts_with(prefix))
        .map(|row| (first + row) as i32 - history as i32)
}

/// The rows the open block's pictures are at, oldest first.
fn picture_rows(emulator: &mut Emulator) -> Vec<i32> {
    let snapshot = emulator.snapshot();
    snapshot
        .live_block
        .images
        .iter()
        .map(|image| image.row)
        .collect()
}

#[test]
fn a_picture_follows_its_rows_while_the_history_has_room() {
    // What the history's own size could always count, so that the tests
    // after this one are seen to be about the history being full.
    let mut emulator = picture_after(10, 100);
    let row = row_saying(&mut emulator, "ABOVE") + 1;
    let images = &emulator.snapshot().live_block.images;
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].row, row);

    emulator.advance(b"more\r\n");
    let row = row_saying(&mut emulator, "ABOVE") + 1;
    assert_eq!(emulator.snapshot().live_block.images[0].row, row);
}

#[test]
fn a_picture_keeps_its_rows_after_the_history_fills() {
    // A full history does not grow: every line scrolled into it evicts one,
    // so its size stops saying how far the lines under a picture have moved.
    // Nine more lines fill this one, and the two after them go past it.
    let mut emulator = picture_after(2, 10);
    for line in 0..11 {
        emulator.advance(format!("more {line}\r\n").as_bytes());
        let above = line_saying(&emulator, "ABOVE").expect("ABOVE is in the history");
        assert_eq!(
            picture_rows(&mut emulator),
            [above + 1],
            "after line {line}"
        );
    }
    assert_eq!(emulator.history_len(), 10);
}

#[test]
fn a_picture_printed_into_a_full_history_follows_its_rows() {
    // Thirty lines fill the history before the picture is printed, on the
    // bottom row: an anchor that stayed on the screen row it was taken on
    // would put the picture over the prompt and whatever is typed at it.
    let mut emulator = picture_printed_after(30, 10);
    emulator.advance(b"\r\nBELOW\r\n");
    for line in 0..8 {
        let above = line_saying(&emulator, "ABOVE").expect("ABOVE is in the history");
        assert_eq!(
            picture_rows(&mut emulator),
            [above + 1],
            "after line {line}"
        );
        emulator.advance(format!("more {line}\r\n").as_bytes());
    }
}

#[test]
fn a_picture_whose_first_row_leaves_the_history_goes_with_it() {
    // Twelve lines take `ABOVE` out of the history and leave the picture's
    // first row the oldest line there is. One more takes that row too, and
    // an anchor on a line that is gone reads as the oldest line: the picture
    // would be drawn a row down, over its own second row, and then over
    // whatever followed it.
    let mut emulator = picture_after(2, 10);
    for line in 0..12 {
        emulator.advance(format!("more {line}\r\n").as_bytes());
    }
    assert_eq!(line_saying(&emulator, "ABOVE"), None);
    assert_eq!(picture_rows(&mut emulator), [-10]);

    emulator.advance(b"one more\r\n");
    assert_eq!(picture_rows(&mut emulator), [0; 0]);
}

#[test]
fn a_picture_that_scrolls_an_older_one_out_of_the_history_takes_it() {
    // The older picture's first row is the oldest line there is, and the
    // newer one, printed at the foot of the screen and two rows tall, takes
    // one more line into the history: nothing else in the read scrolls.
    let mut emulator = picture_after(2, 10);
    for line in 0..12 {
        emulator.advance(format!("more {line}\r\n").as_bytes());
    }
    assert_eq!(picture_rows(&mut emulator), [-10]);

    emulator.advance(&osc("width=4;height=2;", &png(8, 8)));
    assert_eq!(picture_rows(&mut emulator), [4]);
}

#[test]
fn a_shorter_pane_that_fills_the_history_keeps_its_pictures_on_their_rows() {
    // One line of history to start with, of the three it may hold: four rows
    // fewer push four lines into it, two past its length.
    let mut emulator = picture_after(2, 3);
    assert_eq!(emulator.history_len(), 1);
    assert_eq!(emulator.snapshot().live_block.images.len(), 1);

    emulator.resize(TerminalSize::new(20, 2).with_cell_size(10, 20));
    assert_eq!(emulator.history_len(), 3);
    let above = line_saying(&emulator, "ABOVE").expect("ABOVE is in the history");
    assert_eq!(picture_rows(&mut emulator), [above + 1]);
}

#[test]
fn a_read_of_more_lines_than_the_headroom_keeps_the_pictures_on_their_rows() {
    // One read, past a full history, of three times as many lines as
    // alacritty has room for past it: fed whole, the read would scroll the
    // history to the end of that room and the pictures would go with the
    // count. It is fed a slice at a time, cut back after each.
    let mut emulator = picture_printed_after(1100, 1000);
    emulator.advance(b"\r\nBELOW\r\n");
    let read: String = (0..HISTORY_HEADROOM * 3)
        .map(|line| format!("more {line}\r\n"))
        .collect();
    emulator.advance(read.as_bytes());

    assert_eq!(emulator.history_len(), 1000);
    let above = line_saying(&emulator, "ABOVE").expect("ABOVE is in the history");
    assert_eq!(picture_rows(&mut emulator), [above + 1]);
}

#[test]
fn more_lines_than_the_headroom_at_once_take_the_pictures() {
    // A synchronized update reaches the grid all at once, however it was
    // fed. Fifty lines more than the headroom, past a full history, are fifty
    // that alacritty evicts uncounted: the picture would be drawn fifty rows
    // under its own, so it goes instead.
    let mut emulator = picture_printed_after(1100, 1000);
    emulator.advance(b"\r\nBELOW\r\n");
    let mut update = b"\x1b[?2026h".to_vec();
    for line in 0..HISTORY_HEADROOM + 50 {
        update.extend_from_slice(format!("more {line}\r\n").as_bytes());
    }
    update.extend_from_slice(b"\x1b[?2026l");
    emulator.advance(&update);

    assert!(line_saying(&emulator, "ABOVE").is_some());
    assert_eq!(picture_rows(&mut emulator), [0; 0]);
}

#[test]
fn the_open_block_lets_its_oldest_pictures_go_once_they_weigh_too_much() {
    // A picture weighs its file, and never less than the floor. One file of
    // three floors, then headers at the floor each, a cell apiece, to one
    // floor past the budget: the heavy one, printed first, is what goes, and
    // every header stays. Two hundred columns hold them all on fewer rows
    // than the history keeps.
    let floors = MAX_IMAGE_BYTES / PICTURE_FLOOR;
    let mut heavy = png(30, 10);
    heavy.resize(3 * PICTURE_FLOOR, 0);
    let mut read = osc("", &heavy);
    let header = osc("", &png(10, 10));
    for _ in 0..floors - 2 {
        read.extend_from_slice(&header);
    }
    let mut emulator = Emulator::new(
        TerminalSize::new(200, 6).with_cell_size(10, 20),
        1000,
        Palette::default(),
    );
    emulator.advance(&read);

    let images = &emulator.snapshot().live_block.images;
    assert_eq!(images.len(), floors - 2);
    assert!(images.iter().all(|image| image.image.size() == (10, 10)));
}

#[test]
fn a_picture_redrawn_in_place_replaces_the_one_under_it() {
    // A plot redrawn as it changes: the cursor goes back to where the last
    // frame started and the next one is printed over it, a hundred times.
    // Kept, the frames would pile up on the same cells, every one of them
    // drawn under the last on every frame the pane draws.
    let mut emulator = training();
    for frame in 0..100 {
        emulator.advance(b"\x1b[3H");
        emulator.advance(&osc("width=4;height=2;", &png(8, 8 + frame % 2)));
    }
    let images = &emulator.snapshot().live_block.images;
    assert_eq!(images.len(), 1);
    assert_eq!((images[0].row, images[0].image.size()), (2, (8, 9)));

    emulator.advance(b"\x1b]133;D;0\x07\x1b]133;A\x07$ ");
    let rows: Vec<i32> = emulator.blocks()[0]
        .images
        .iter()
        .map(|image| image.row)
        .collect();
    assert_eq!(rows, [2]);
}

/// A picture stretched over `columns` by `rows` cells, so that it covers
/// exactly the box it asks for.
fn stretched(columns: usize, rows: usize) -> Vec<u8> {
    osc(
        &format!("width={columns};height={rows};preserveAspectRatio=0;"),
        &png(8, 8),
    )
}

/// The open block's pictures, oldest first, as their row, column, columns
/// and rows.
fn boxes(emulator: &mut Emulator) -> Vec<(i32, usize, usize, usize)> {
    let snapshot = emulator.snapshot();
    let images = &snapshot.live_block.images;
    images
        .iter()
        .map(|image| (image.row, image.column, image.columns, image.rows))
        .collect()
}

#[test]
fn a_picture_goes_only_once_a_later_one_covers_every_cell_of_it() {
    let mut emulator = emulator_with_cells();
    emulator.advance(&stretched(4, 2));
    // Over one corner of it, and over the opposite corner and past it:
    // neither covers all of it.
    emulator.advance(b"\x1b[H");
    emulator.advance(&stretched(2, 1));
    emulator.advance(b"\x1b[2;4H");
    emulator.advance(&stretched(2, 2));
    assert_eq!(
        boxes(&mut emulator),
        [(0, 0, 4, 2), (0, 0, 2, 1), (1, 3, 2, 2)]
    );

    // Over all of the corner and none of the rest of either.
    emulator.advance(b"\x1b[H");
    emulator.advance(&stretched(3, 2));
    assert_eq!(
        boxes(&mut emulator),
        [(0, 0, 4, 2), (1, 3, 2, 2), (0, 0, 3, 2)]
    );

    // Over all of the one before the last, which starts a row lower than
    // the last does.
    emulator.advance(b"\x1b[2;4H");
    emulator.advance(&stretched(2, 2));
    assert_eq!(
        boxes(&mut emulator),
        [(0, 0, 4, 2), (0, 0, 3, 2), (1, 3, 2, 2)]
    );
}

#[test]
fn a_picture_lets_go_of_every_one_it_covers_wherever_their_corners_are() {
    // Printed first, and below the one printed last: one on the last cell
    // it takes, one inside it, and one each just past its right edge and
    // just under its foot. It covers the first two, whose corners are on
    // cells of its own other than its corner, and neither of the others.
    let mut emulator = emulator_with_cells();
    for (at, (columns, rows)) in [
        ("4;5", (1, 1)),
        ("2;2", (2, 1)),
        ("1;6", (1, 1)),
        ("5;1", (1, 1)),
    ] {
        emulator.advance(format!("\x1b[{at}H").as_bytes());
        emulator.advance(&stretched(columns, rows));
    }
    emulator.advance(b"\x1b[H");
    emulator.advance(&stretched(5, 4));
    assert_eq!(
        boxes(&mut emulator),
        [(0, 5, 1, 1), (4, 0, 1, 1), (0, 0, 5, 4)]
    );
}

#[test]
fn a_full_screen_program_started_and_left_in_full_slices_keeps_the_pictures_on_their_rows() {
    // Two reads as long as a slice into a full history may be, and nearly
    // all newlines: the first scrolls the shell's screen and puts a program's
    // up, the second takes it down and scrolls the shell's screen again. The
    // shell's history has to be cut back between the newlines and the
    // switch, both ways, or the second read scrolls it past the end of the
    // headroom.
    let mut emulator = picture_printed_after(1100, 1000);
    emulator.advance(b"\r\nBELOW\r\n");
    let newlines = b"\n".repeat(HISTORY_HEADROOM - 1 - b"\x1b[?1049h".len());
    let mut up = newlines.clone();
    up.extend_from_slice(b"\x1b[?1049h");
    emulator.advance(&up);
    let mut down = b"\x1b[?1049l".to_vec();
    down.extend_from_slice(&newlines);
    emulator.advance(&down);

    let above = line_saying(&emulator, "ABOVE").expect("ABOVE is in the history");
    assert_eq!(picture_rows(&mut emulator), [above + 1]);
}

#[test]
fn a_taller_pane_under_a_program_put_up_after_newlines_keeps_the_pictures_on_their_rows() {
    // Lines scrolled into the shell's history on the way to a program's
    // screen, twice, and then a taller pane, which pulls lines back out of it
    // before the shell's screen is back.
    let mut emulator = picture_printed_after(1100, 1000);
    emulator.advance(b"\r\nBELOW\r\n");
    let mut up = b"\n".repeat(247);
    up.extend_from_slice(b"\x1b[?1049h");
    emulator.advance(&up);
    let mut again = b"\x1b[?1049l".to_vec();
    again.extend_from_slice(&b"\n".repeat(239));
    again.extend_from_slice(b"\x1b[?1049h");
    emulator.advance(&again);
    emulator.resize(TerminalSize::new(20, 26).with_cell_size(10, 20));
    emulator.advance(b"\x1b[?1049l");

    let above = line_saying(&emulator, "ABOVE").expect("ABOVE is in the history");
    assert_eq!(picture_rows(&mut emulator), [above + 1]);
}

#[test]
fn a_taller_pane_after_an_update_that_scrolled_out_of_sight_takes_the_pictures() {
    // An update reaches the grid all at once: fifty lines past the end of
    // the headroom on the shell's screen, and then a program's screen up,
    // under which nothing can read the shell's history to see that it went
    // past. A taller pane pulls sixty lines back out of it before the
    // shell's screen is back, and the history is then short of the end:
    // the fifty lines nothing counted would put the picture fifty rows under
    // its own. So the pictures go with the taller pane, under the program,
    // and not once the shell's history can be read again.
    let mut emulator = picture_printed_after(1100, 1000);
    emulator.advance(b"\r\nBELOW\r\n");
    let mut update = b"\x1b[?2026h".to_vec();
    for line in 0..HISTORY_HEADROOM + 50 {
        update.extend_from_slice(format!("more {line}\r\n").as_bytes());
    }
    update.extend_from_slice(b"\x1b[?1049h\x1b[?2026l");
    emulator.advance(&update);
    assert!(emulator.is_alt_screen());
    assert_eq!(emulator.open_pictures(), 1);
    emulator.resize(TerminalSize::new(20, 66).with_cell_size(10, 20));
    assert_eq!(emulator.open_pictures(), 0);
    emulator.advance(b"\x1b[?1049l");
    assert_eq!(picture_rows(&mut emulator), [0; 0]);

    // Once the shell's screen is back its history is read again, and a
    // picture printed after that keeps its rows through the next program.
    emulator.advance(b"ABOVE\r\n");
    emulator.advance(&osc("width=4;height=2;", &png(8, 8)));
    emulator.advance(b"\x1b[?1049h");
    emulator.resize(TerminalSize::new(20, 76).with_cell_size(10, 20));
    emulator.advance(b"\x1b[?1049l");
    let above = row_saying(&mut emulator, "ABOVE");
    assert_eq!(picture_rows(&mut emulator), [above + 1]);
}

/// The most bytes `vte` 0.15 holds in a synchronized update: a feed that
/// would take its buffer to `SYNC_BUFFER_SIZE - 1` lets go of the update
/// instead, and is parsed after what the update held.
const SYNC_HELD_AT_MOST: usize = 0x20_0000 - 2;

#[test]
fn an_update_let_go_of_by_its_own_switch_byte_takes_the_pictures_under_a_taller_pane() {
    // The update fills `vte`'s buffer to the last byte it holds with lines
    // and the start of a switch to a program's screen, so the switch's last
    // byte is the one that lets go of it: the lines scroll the shell's
    // history far past the end of the headroom, and the program's screen is
    // up before anything can read it.
    let mut emulator = picture_printed_after(1100, 1000);
    emulator.advance(b"\r\nBELOW\r\n");
    let switch = b"\x1b[?1049";
    let mut held = Vec::new();
    for line in 0.. {
        let next = format!("m {line:07}\r\n");
        if held.len() + next.len() + switch.len() > SYNC_HELD_AT_MOST {
            break;
        }
        held.extend_from_slice(next.as_bytes());
    }
    held.resize(SYNC_HELD_AT_MOST - switch.len(), b' ');
    held.extend_from_slice(switch);
    let mut update = b"\x1b[?2026h".to_vec();
    update.extend_from_slice(&held);
    update.push(b'h');
    emulator.advance(&update);
    // Let go of by that byte, and neither after it nor before: still held,
    // the update would not have put the program's screen up yet, and let go
    // of sooner, the trim before the switch would have found the history at
    // the end of the headroom and taken the picture then.
    assert!(emulator.is_alt_screen());
    assert_eq!(emulator.open_pictures(), 1);

    emulator.resize(TerminalSize::new(20, 66).with_cell_size(10, 20));
    emulator.advance(b"\x1b[?1049l");
    assert_eq!(picture_rows(&mut emulator), [0; 0]);
}

#[test]
fn a_taller_pane_under_a_program_drawing_in_updates_keeps_the_pictures() {
    // A program that draws every frame in an update on its own screen never
    // touches the shell's, which nothing then has to doubt. Each frame
    // arrives in two reads, so that the update is held between them.
    let mut emulator = picture_after(9, 10);
    emulator.advance(b"\x1b[?1049h");
    for frame in 0..3 {
        emulator.advance(format!("\x1b[?2026h\x1b[Hframe {frame}").as_bytes());
        emulator.advance(b"\x1b[?2026l");
    }
    emulator.resize(TerminalSize::new(20, 8).with_cell_size(10, 20));
    emulator.advance(b"\x1b[?1049l");

    let above = row_saying(&mut emulator, "ABOVE");
    assert_eq!(picture_rows(&mut emulator), [above + 1]);
}

#[test]
fn a_taller_pane_after_a_scroll_past_the_headroom_and_a_program_takes_the_pictures() {
    // Six bytes scroll a screen this tall by more lines than the headroom
    // holds, and a program's screen goes up right after them. The history
    // is read before the switch, while it still can be, and found at the end
    // of the headroom: read only once the shell's screen is back, after a
    // taller pane has pulled lines out of it, it would be short of the end,
    // and the lines nothing counted would put the picture under its own row.
    // So the pictures have gone by the time the program's screen is up.
    let size = |rows| TerminalSize::new(20, rows).with_cell_size(10, 20);
    let tall = (HISTORY_HEADROOM + 20) as u16;
    let mut emulator = Emulator::new(size(tall), 1000, Palette::default());
    let lines: String = (0..1100 + usize::from(tall))
        .map(|line| format!("log {line}\r\n"))
        .collect();
    emulator.advance(lines.as_bytes());
    emulator.advance(b"ABOVE\r\n");
    emulator.advance(&osc("width=4;height=2;", &png(8, 8)));
    emulator.advance(b"\r\nBELOW\r\n");
    let above = line_saying(&emulator, "ABOVE").expect("ABOVE is on the screen");
    assert_eq!(picture_rows(&mut emulator), [above + 1]);

    emulator.advance(format!("\x1b[{tall}S\x1b[?1049h").as_bytes());
    assert!(emulator.is_alt_screen());
    assert_eq!(emulator.open_pictures(), 0);
    emulator.resize(size(tall + 100));
    emulator.advance(b"\x1b[?1049l");
    assert_eq!(picture_rows(&mut emulator), [0; 0]);
}

#[test]
fn a_shorter_pane_under_a_full_screen_program_takes_its_pictures() {
    // `Term` resizes the shell's grid under vim too, where nothing here can
    // read its history or cut it back. A pane shorter by more rows than the
    // headroom pushes lines past its end, which alacritty evicts uncounted,
    // and a taller one pulls lines back out before the shell's screen is
    // back, so its history is short of the end again by then and nothing can
    // see that any went: the anchor would be off by every line that did. So
    // the pictures go with the shorter pane, under the program.
    let size = |rows| TerminalSize::new(20, rows).with_cell_size(10, 20);
    let tall = (HISTORY_HEADROOM + 20) as u16;
    let mut emulator = Emulator::new(size(tall), 10, Palette::default());
    for line in 0..tall {
        emulator.advance(format!("log {line}\r\n").as_bytes());
    }
    emulator.advance(b"ABOVE\r\n");
    emulator.advance(&osc("width=4;height=2;", &png(8, 8)));
    emulator.advance(b"\r\nBELOW\r\n");
    let above = line_saying(&emulator, "ABOVE").expect("ABOVE is on the screen");
    assert_eq!(picture_rows(&mut emulator), [above + 1]);

    emulator.advance(b"\x1b[?1049h");
    assert_eq!(emulator.open_pictures(), 1);
    emulator.resize(size(2));
    assert_eq!(emulator.open_pictures(), 0);
    emulator.resize(size(tall));
    emulator.advance(b"\x1b[?1049l");
    assert_eq!(picture_rows(&mut emulator), [0; 0]);
}

#[test]
fn a_taller_pane_under_a_full_screen_program_keeps_its_pictures() {
    // The lines it pulls back out of the history are counted, full or not.
    let mut emulator = picture_after(9, 10);
    emulator.advance(b"\x1b[?1049h");
    emulator.resize(TerminalSize::new(20, 8).with_cell_size(10, 20));
    emulator.advance(b"\x1b[?1049l");

    let row = row_saying(&mut emulator, "ABOVE") + 1;
    let images = &emulator.snapshot().live_block.images;
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].row, row);
}

/// What `reset` prints, from ncurses 6.6 with `TERM=xterm-256color`: a full
/// reset (`ESC c`) first, which empties the history along with the screen.
const RESET: &[u8] = b"\x1bc\x1b]104\x07\x1b[!p\x1b[?3;4l\x1b[4l\x1b>\x1b[?69l\r";

#[test]
fn a_reset_takes_the_pictures_with_the_history() {
    // The anchor is measured against a history the reset emptied, so it
    // names a row far below the screen, and output growing the history
    // again brings it back up, onto whatever is on that row by then.
    let mut emulator = picture_after(50, 10_000);
    for line in 0..50 {
        emulator.advance(format!("log {line}\r\n").as_bytes());
    }
    emulator.advance(RESET);
    emulator.advance(b"$ ");
    for line in 0..52 {
        emulator.advance(format!("new {line}\r\n").as_bytes());
    }
    emulator.advance(b"$ ");

    let images = &emulator.snapshot().live_block.images;
    assert!(images.is_empty(), "{images:?}");
}

#[test]
fn a_reset_read_with_a_full_screen_program_starting_takes_the_pictures() {
    // The shell's history is gone whichever screen the read ends on.
    let mut emulator = picture_after(50, 10_000);
    let mut read = RESET.to_vec();
    read.extend_from_slice(b"\x1b[?1049h");
    emulator.advance(&read);
    emulator.advance(b"\x1b[?1049l$ ");
    for line in 0..52 {
        emulator.advance(format!("new {line}\r\n").as_bytes());
    }

    let images = &emulator.snapshot().live_block.images;
    assert!(images.is_empty(), "{images:?}");
}

#[test]
fn a_picture_that_scrolls_out_in_an_expired_update_goes() {
    // The update's lines reach the grid when it expires, which is a snapshot
    // with no output after it: the history is cut back to its length there,
    // and the picture whose first row went with the lines cut goes too.
    let mut emulator = picture_after(2, 10);
    assert_eq!(emulator.snapshot().live_block.images.len(), 1);

    let mut update = b"\x1b[?2026h".to_vec();
    for line in 0..20 {
        update.extend_from_slice(format!("more {line}\r\n").as_bytes());
    }
    emulator.advance(&update);
    std::thread::sleep(Duration::from_millis(200));
    let images = &emulator.snapshot().live_block.images;
    assert!(images.is_empty(), "{images:?}");
    assert_eq!(emulator.history_len(), 10);
}

/// A grid of twenty columns by six rows keeping ten lines of history, at a
/// prompt where `train` has started: shell integration, so the history was
/// dropped when the block opened, and fills only under this one command
/// printing more than all of it.
fn training() -> Emulator {
    let mut emulator = Emulator::new(
        TerminalSize::new(20, 6).with_cell_size(10, 20),
        10,
        Palette::default(),
    );
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07train\r\n\x1b]133;C\x07");
    emulator
}

/// The row of `block` that reads `text`, whole.
fn block_row_saying(block: &crate::Block, text: &str) -> Option<i32> {
    (0..block.rows.rows())
        .position(|row| block.rows.text(row) == text)
        .map(|row| row as i32)
}

#[test]
fn a_block_longer_than_the_history_keeps_its_picture_on_its_row() {
    // The picture is placed while the history has room, and the lines that
    // fill it and go past it arrive in the one read with the `D` that
    // harvests the block.
    let mut emulator = training();
    for line in 0..20 {
        emulator.advance(format!("epoch {line}\r\n").as_bytes());
    }
    emulator.advance(b"BEFORE\r\n");
    emulator.advance(&osc("width=4;height=2;", &png(8, 8)));
    let mut read = b"\r\nAFTER\r\n".to_vec();
    for line in 0..8 {
        read.extend_from_slice(format!("tail {line}\r\n").as_bytes());
    }
    read.extend_from_slice(b"\x1b]133;D;0\x07\x1b]133;A\x07$ ");
    emulator.advance(&read);

    let block = &emulator.blocks()[0];
    let before = block_row_saying(block, "BEFORE").expect("BEFORE is in the block");
    let rows: Vec<i32> = block.images.iter().map(|image| image.row).collect();
    assert_eq!(rows, [before + 1]);
}

#[test]
fn a_picture_that_scrolled_out_of_a_long_block_is_not_harvested() {
    // Thirty lines after it take its rows out of a history of ten, in the one
    // read with the `D`. Read at a line that is gone, its anchor would name
    // the oldest line there is, which is the block's first row: the picture
    // would be filed there, over `log 15`. Only a look at the history before
    // the mark is applied stands between the two.
    let mut emulator = training();
    emulator.advance(b"log\r\n");
    emulator.advance(&osc("width=4;height=2;", &png(8, 8)));
    assert_eq!(emulator.snapshot().live_block.images.len(), 1);

    let mut read = b"\r\n".to_vec();
    for line in 0..30 {
        read.extend_from_slice(format!("log {line}\r\n").as_bytes());
    }
    read.extend_from_slice(b"\x1b]133;D;0\x07\x1b]133;A\x07$ ");
    emulator.advance(&read);

    let block = &emulator.blocks()[0];
    assert!(block.images.is_empty(), "{:?}", block.images);
}

#[test]
fn seventy_icons_are_all_kept_on_their_rows() {
    // What `imgcat *.png` prints over a folder of icons: each one's name,
    // and the icon on the row under it: seventy of them, two kilobytes of files.
    let mut emulator = Emulator::new(
        TerminalSize::new(40, 10).with_cell_size(10, 20),
        10_000,
        Palette::default(),
    );
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07imgcat *.png\r\n\x1b]133;C\x07");
    for index in 0..70 {
        emulator.advance(format!("icon{index}.png\r\n").as_bytes());
        emulator.advance(&osc("", &png(16, 16)));
        emulator.advance(b"\r\n");
    }
    emulator.advance(b"\x1b]133;D;0\x07\x1b]133;A\x07$ ");

    let block = &emulator.blocks()[0];
    let names: Vec<i32> = (0..70)
        .map(|index| block_row_saying(block, &format!("icon{index}.png")).expect("a name") + 1)
        .collect();
    let rows: Vec<i32> = block.images.iter().map(|image| image.row).collect();
    assert_eq!(rows, names);
}

fn emulator_with_cells() -> Emulator {
    Emulator::new(
        TerminalSize::new(20, 6).with_cell_size(10, 20),
        100,
        Palette::default(),
    )
}
