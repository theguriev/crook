//! Where a printed picture is decoded, at what size, and for how long it is
//! kept.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

use crookui_core::scene::Scene;

use super::*;
use crate::picture::tests::{header_only, preview_png};

/// How long a decode is given to land: long, because the machine running
/// the tests may be building ten other things, and what is being looked for
/// is a decode that never lands rather than a slow one.
const LANDING: Duration = Duration::from_secs(60);

/// A frame that has not been drawn before.
fn frame() -> FrameId {
    Scene::new(1.).frame()
}

/// A `width`×`height` picture, as a program printed it.
fn printed(width: u32, height: u32) -> Arc<InlineImage> {
    Arc::new(InlineImage::new(preview_png(width, height)).expect("a PNG within the limits"))
}

/// A cache of `budget` bytes, and how many decodes have landed in it — each
/// of which asks for a frame, and is counted when it does.
fn counting(budget: usize) -> (Cache, Arc<AtomicUsize>) {
    let mut cache = Cache::new(budget);
    let landed = Arc::new(AtomicUsize::new(0));
    let counter = landed.clone();
    cache.wake_with(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    (cache, landed)
}

/// Draws `screen` in one frame, top to bottom, each picture `size` pixels
/// big — and then waits for every decode the frame started, the way a window
/// waits for the frame a landing asks for.
fn draw(cache: &mut Cache, screen: &[Arc<InlineImage>], size: (u32, u32)) -> Vec<Shown> {
    let frame = frame();
    let shown = screen
        .iter()
        .map(|picture| cache.shown(picture, size, frame))
        .collect();
    assert!(cache.settle(LANDING), "a decode never landed");
    shown
}

/// What was drawn, in a word.
fn kind(shown: &Shown) -> &'static str {
    match shown {
        Shown::Picture(_) => "picture",
        Shown::Waiting => "waiting",
        Shown::Nothing => "nothing",
    }
}

/// The size of the pixels drawn, when there were any.
fn pixels_of(shown: &Shown) -> Option<(u32, u32)> {
    match shown {
        Shown::Picture(pixels) => Some(pixels.size()),
        _ => None,
    }
}

#[test]
fn a_picture_is_decoded_on_the_pool_and_drawn_by_the_frame_its_landing_asks_for() {
    // A screen of 4096-pixel pictures decoded where the frame is drawn was a
    // window that answered nothing for a tenth of a second at a time. The
    // frame that first meets one draws its room instead, and the decode asks
    // for the next frame, from the thread it ran on, once it lands.
    let (woken, wakes) = mpsc::channel();
    wake_with(move || {
        let _ = woken.send(thread::current().id());
    });
    let picture = printed(64, 32);

    let first = shown(&picture, (64, 32), frame());
    assert_eq!(kind(&first), "waiting", "the frame decoded it");

    let woke_from = wakes
        .recv_timeout(LANDING)
        .expect("the decode never asked for a frame");
    assert_ne!(
        woke_from,
        thread::current().id(),
        "decoded on the thread that draws"
    );
    assert_eq!(
        pixels_of(&shown(&picture, (64, 32), frame())),
        Some((64, 32))
    );
}

#[test]
fn six_large_pictures_on_screen_are_decoded_once_each_and_not_once_a_frame() {
    // A budget of four megabytes standing in for 256, and six pictures of a
    // megabyte each for six 4096-pixel squares, whose full-size pixels took
    // a hundred milliseconds a frame to decode. Kept at that size and let go
    // of least recently drawn first, each was evicted just before the frame
    // came round to it again — every one, every frame.
    let (mut cache, landed) = counting(4 << 20);
    let screen: Vec<_> = (0..6).map(|_| printed(512, 512)).collect();

    for _ in 0..4 {
        draw(&mut cache, &screen, (64, 64));
    }
    let last = draw(&mut cache, &screen, (64, 64));

    assert_eq!(
        landed.load(Ordering::SeqCst),
        6,
        "decoded more than once each"
    );
    assert!(
        last.iter().all(|shown| pixels_of(shown) == Some((64, 64))),
        "{last:?}"
    );
}

#[test]
fn pictures_that_fill_the_budget_in_one_frame_keep_it_and_the_rest_wait_as_their_room() {
    // Six pictures drawn a megabyte each, under three and a half: three fit.
    // The other three are drawn as the room they will take for as long as
    // the first three are on screen, rather than each evicting one the frame
    // has just drawn, to be evicted in turn.
    let (mut cache, landed) = counting(7 << 19);
    let screen: Vec<_> = (0..6).map(|_| printed(512, 512)).collect();

    let mut last = Vec::new();
    for _ in 0..4 {
        last = draw(&mut cache, &screen, (512, 512));
    }

    assert_eq!(
        last.iter().map(kind).collect::<Vec<_>>(),
        [
            "picture", "picture", "picture", "waiting", "waiting", "waiting"
        ]
    );
    assert_eq!(
        landed.load(Ordering::SeqCst),
        3,
        "nothing was decoded twice"
    );
}

#[test]
fn pictures_scrolled_out_of_view_make_room_for_the_ones_scrolled_into_it() {
    let (mut cache, landed) = counting(7 << 19);
    let above: Vec<_> = (0..3).map(|_| printed(512, 512)).collect();
    let below: Vec<_> = (0..3).map(|_| printed(512, 512)).collect();

    draw(&mut cache, &above, (512, 512));
    draw(&mut cache, &below, (512, 512));
    let scrolled = draw(&mut cache, &below, (512, 512));

    assert!(
        scrolled.iter().all(|shown| kind(shown) == "picture"),
        "the pictures of a frame before kept the budget from this one: {scrolled:?}"
    );
    assert_eq!(landed.load(Ordering::SeqCst), 6);
}

#[test]
fn a_picture_is_decoded_at_the_size_it_is_drawn_and_again_when_that_changes() {
    // A 2000-pixel screenshot in a few cells is kept at what the cells cover.
    // A font grown under it has it decoded again at the new size, and drawn
    // from the old pixels until then rather than blinking out.
    let (mut cache, landed) = counting(BUDGET);
    let screenshot = [printed(2000, 1000)];

    draw(&mut cache, &screenshot, (200, 100));
    let small = draw(&mut cache, &screenshot, (200, 100));
    assert_eq!(pixels_of(&small[0]), Some((200, 100)));

    let growing = draw(&mut cache, &screenshot, (400, 200));
    assert_eq!(
        pixels_of(&growing[0]),
        Some((200, 100)),
        "the old pixels, while the new ones are decoded"
    );
    let grown = draw(&mut cache, &screenshot, (400, 200));
    assert_eq!(pixels_of(&grown[0]), Some((400, 200)));
    assert_eq!(landed.load(Ordering::SeqCst), 2);
}

#[test]
fn a_picture_zoomed_past_the_whole_budget_is_drawn_from_its_own_pixels() {
    // A screenshot fitted to a pane 1400 logical pixels wide on a 2x display,
    // and the font taken from 12.5 to 48 under it: its cells now cover 10752
    // device pixels by 6720, 289 megabytes at that size, past the budget on
    // its own. Decoded at the size it was drawn, it waited as its room for
    // good. Its file has 2800 pixels by 1750, and those are what it is
    // decoded at; the renderer stretches them the rest of the way.
    let (mut cache, landed) = counting(BUDGET);
    let screenshot = [printed(2800, 1750)];
    let drawn = (10752, 6720);
    assert!(bytes(drawn) > BUDGET, "drawn within the budget");

    draw(&mut cache, &screenshot, drawn);
    let zoomed = draw(&mut cache, &screenshot, drawn);
    assert_eq!(pixels_of(&zoomed[0]), Some((2800, 1750)), "{zoomed:?}");

    // A font step further on, they are still the pixels it has: the same
    // ones, and not a decode at every step, or at every frame.
    let further = draw(&mut cache, &screenshot, (11520, 7200));
    let Shown::Picture(further) = &further[0] else {
        panic!("not drawn a step further on: {further:?}");
    };
    let Shown::Picture(zoomed) = &zoomed[0] else {
        unreachable!("checked above");
    };
    assert!(
        Arc::ptr_eq(further, zoomed),
        "decoded again a step further on"
    );
    draw(&mut cache, &screenshot, (11520, 7200));
    assert_eq!(landed.load(Ordering::SeqCst), 1, "decoded more than once");
}

#[test]
fn a_small_picture_is_grown_on_the_pool_as_far_as_an_atlas_row_and_no_further() {
    // Within an atlas row the renderer resamples what it is handed to the
    // size it is drawn, on the thread that draws, so the pool hands it that
    // size. Past one it stretches what it is handed on the GPU, and the pool
    // stops at the row: an icon zoomed across a pane is four megabytes, not
    // two hundred and eighty-nine.
    let (mut cache, _) = counting(BUDGET);
    let icon = [printed(64, 40)];

    draw(&mut cache, &icon, (640, 400));
    let grown = draw(&mut cache, &icon, (640, 400));
    assert_eq!(pixels_of(&grown[0]), Some((640, 400)));

    draw(&mut cache, &icon, (10752, 6720));
    let zoomed = draw(&mut cache, &icon, (10752, 6720));
    assert_eq!(pixels_of(&zoomed[0]), Some((EXACT_SIDE, 640)));
}

#[test]
fn a_side_past_the_cap_is_held_to_the_pixels_its_file_has_along_it() {
    let cap = |drawn| decoded_size(drawn, (2800, 1750));
    assert_eq!(cap((2800, 1750)), (2800, 1750), "at its own pixels");
    assert_eq!(cap((1400, 875)), (1400, 875), "within them");
    assert_eq!(cap((5600, 3500)), (2800, 1750));
    assert_eq!(
        cap((2801, 1750)),
        (2800, 1750),
        "a pixel past its own size is its own size, not a rounding short of it"
    );
    assert_eq!(
        decoded_size((3500, 5600), (1750, 2800)),
        (1750, 2800),
        "the long side is capped whichever it is"
    );

    // Stretched into its cells, out of its file's shape: a 1600x1200 photo
    // printed across a pane 1400 logical pixels wide and ten 16-pixel rows
    // tall, on a 2x display. Capped as a whole by its width, it was decoded
    // 183 rows tall and blown up soft to the 320 its cells cover, from a
    // file of 1200.
    assert_eq!(decoded_size((2800, 320), (1600, 1200)), (1600, 320));
    assert_eq!(
        decoded_size((3000, 300), (100, 1000)),
        (102, 300),
        "a picture shorter than a row is grown to one, its shape kept, and no further"
    );

    // Within an atlas row it is the drawn size, whatever its file's shape:
    // the renderer would resample anything else on the thread that draws.
    assert_eq!(decoded_size((1000, 300), (1000, 1)), (1000, 300));
    assert_eq!(decoded_size((1000, 300), (4000, 10)), (1000, 300));

    assert_eq!(
        decoded_size((40000, 3), (1000, 1)),
        (EXACT_SIDE, 1),
        "a side is never shrunk to nothing"
    );
}

#[test]
fn a_picture_its_terminal_let_go_of_is_let_go_of_by_the_next_frame() {
    // Its block scrolled out of the history, or its pane closed: nothing will
    // draw it again, and its pixels were kept until the budget ran out.
    let (mut cache, _) = counting(BUDGET);
    let scrolled_out = printed(64, 64);
    let staying = [printed(64, 64)];

    draw(&mut cache, std::slice::from_ref(&scrolled_out), (64, 64));
    let drawn = draw(&mut cache, std::slice::from_ref(&scrolled_out), (64, 64));
    let Shown::Picture(pixels) = &drawn[0] else {
        panic!("never drawn: {drawn:?}");
    };
    let pixels = Arc::downgrade(pixels);
    drop(drawn);
    drop(scrolled_out);

    draw(&mut cache, &staying, (64, 64));
    assert!(
        pixels.upgrade().is_none(),
        "the pixels of a picture nobody holds are still held"
    );
}

#[test]
fn the_pictures_of_a_closed_pane_are_let_go_of_after_a_frame_that_draws_none() {
    // The last pane with pictures in it closed, and no frame draws a picture
    // again to notice: the window lets go of them after every frame it draws.
    let picture = printed(64, 64);
    shown(&picture, (64, 64), frame());
    assert!(settle(LANDING), "the decode never landed");
    let pixels = match shown(&picture, (64, 64), frame()) {
        Shown::Picture(pixels) => Arc::downgrade(&pixels),
        other => panic!("never drawn: {other:?}"),
    };

    drop(picture);
    forget_the_gone();

    assert!(
        pixels.upgrade().is_none(),
        "the pixels of a closed pane's picture are still held"
    );
}

#[test]
fn a_picture_whose_file_does_not_decode_is_tried_once_and_then_draws_nothing() {
    let (mut cache, landed) = counting(BUDGET);
    let broken = [Arc::new(
        InlineImage::new(header_only(64, 64)).expect("its header is a PNG's"),
    )];

    let first = draw(&mut cache, &broken, (64, 64));
    assert_eq!(kind(&first[0]), "waiting");
    for _ in 0..3 {
        let shown = draw(&mut cache, &broken, (64, 64));
        assert_eq!(kind(&shown[0]), "nothing");
    }
    assert_eq!(landed.load(Ordering::SeqCst), 1, "tried again");
}
