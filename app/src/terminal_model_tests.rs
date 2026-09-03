//! The model's own tests.
//!
//! Everything except the last one runs with no process at all. The last one
//! opens a real shell, because "bytes a shell printed reach the main thread and
//! repaint a pane" is the entire point of this file and there is nothing left of
//! it once the shell is stubbed out.

use std::sync::Arc;

use crookui_core::App;
use crookui_core::executor::{Background, LocalQueue};

use super::*;

/// How long a test waits for a shell to say something before giving up.
///
/// Generous: it is competing with every other test in the binary for cores, and
/// the failure being looked for is silence, not slowness.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

/// An app whose local queue the test can drive by hand.
fn app() -> (Arc<LocalQueue>, App) {
    let queue = LocalQueue::new();
    let app = App::new(queue.foreground(), Arc::new(Background::new(1)));
    (queue, app)
}

fn pane() -> PaneId {
    PaneId::next()
}

#[test]
fn a_model_that_was_never_started_opens_nothing() {
    // A headless snapshot renders the real view tree; it must not leave four
    // shells running on a build machine to do it.
    let (_queue, mut app) = app();
    let model = app.update(|ctx| ctx.add_model(TerminalModel::new));
    let pane = pane();

    app.update(|ctx| {
        model.update(ctx, |model, ctx| model.sync(&[(pane, None)], ctx));
    });

    app.read(|ctx| {
        assert!(!model.as_ref(ctx).is_live());
        assert!(model.as_ref(ctx).handle(pane).is_none());
        assert!(model.as_ref(ctx).failure(pane).is_none());
    });
}

#[test]
fn a_grid_is_packed_so_either_dimension_moving_is_a_resize() {
    // `TerminalHandle::resize` compares these instead of the terminal's own
    // size, so a packing that collided would silently stop sending `SIGWINCH`.
    assert_eq!(packed(TerminalSize::new(80, 24)), packed(INITIAL_GRID));
    assert_ne!(
        packed(TerminalSize::new(80, 24)),
        packed(TerminalSize::new(24, 80))
    );
    assert_ne!(
        packed(TerminalSize::new(80, 24)),
        packed(TerminalSize::new(80, 25))
    );
    assert_eq!(
        packed(TerminalSize::new(80, 24)),
        packed(TerminalSize::new(80, 24).with_cell_size(7, 15)),
        "the cell size cannot change while the process runs, so it is not part \
         of the comparison"
    );
}

#[test]
fn the_grid_is_drawn_on_the_panel_it_sits_in() {
    // A default cell whose background matched nothing would make an empty
    // screen thousands of rectangles instead of none.
    let palette = crook_palette();
    assert_eq!(palette.background, rgb(THEME.surface));
    assert_eq!(palette.foreground, rgb(THEME.text_primary));
    assert_eq!(
        palette.ansi[1],
        Palette::default().ansi[1],
        "a program that asked for red must still get red"
    );
    // Dim text resolves through this slot, so a palette that left it at the
    // default would hold a grey Crook does not use back by two thirds.
    assert_eq!(palette.dim_foreground, rgb(THEME.text_muted));
}

#[test]
fn a_wait_is_resolved_from_the_reader_thread_and_stays_resolved_once_it_stops() {
    // The bridge between a reader thread and the main one, without a pty in the
    // way. A post that did not clear would spin the chain flat out; an end that
    // did clear would strand a closed pane's wait forever.
    let queue = LocalQueue::new();
    let wake = Arc::new(Wake::default());

    let posting = wake.clone();
    let reader = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        posting.raise();
    });
    queue.block_on(wake.woken());
    reader.join().expect("the posting thread panicked");

    assert!(
        !wake.lock().raised,
        "a post that is not consumed by the wait it woke would spin the chain"
    );
    assert!(!wake.is_finished());

    wake.finish();
    queue.block_on(wake.woken());
    queue.block_on(wake.woken());
    assert!(wake.is_finished(), "the end of a terminal is not consumed");
}

#[test]
fn a_shell_prints_what_it_was_asked_to_and_the_pane_learns_about_it() {
    // The whole file in one test: a shell on a pty, a reader thread parsing it,
    // a foreground chain waking on the result, and a snapshot the main thread
    // can read without a lock.
    let (queue, mut app) = app();
    let model = app.update(|ctx| ctx.add_model(TerminalModel::new));
    let pane = pane();

    app.update(|ctx| {
        model.update(ctx, |model, ctx| model.start(&[(pane, None)], ctx));
    });

    let failure = app.read(|ctx| model.as_ref(ctx).failure(pane).map(str::to_owned));
    if let Some(failure) = failure {
        eprintln!("skipped: no shell could be started here ({failure})");
        return;
    }

    app.read(|ctx| {
        model
            .as_ref(ctx)
            .type_into(pane, "printf 'crook-was-here\\n'\n");
    });

    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        queue.run_until_parked();
        let printed = app.read(|ctx| {
            model
                .as_ref(ctx)
                .snapshot(pane)
                .is_some_and(|snapshot| snapshot.text().contains("crook-was-here"))
        });
        if printed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the shell never printed what it was told to"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // And closing the pane ends it: the session goes, and with it the child and
    // the thread reading it.
    app.update(|ctx| {
        model.update(ctx, |model, ctx| model.sync(&[], ctx));
    });
    app.read(|ctx| {
        assert!(model.as_ref(ctx).handle(pane).is_none());
        assert!(model.as_ref(ctx).snapshot(pane).is_none());
    });
}

/// A session with a real pty behind it, or `None` where one cannot be opened.
///
/// The child is `cat`, which says nothing until it is spoken to; the bytes
/// these tests parse come from a stand-in reader rather than from it, so what
/// the pty is really for is to make the `Terminal` a real one.
fn session() -> Option<Arc<Shared>> {
    let mut terminal = Terminal::spawn(TerminalOptions {
        program: crook_terminal::Program::command("cat", Vec::<String>::new()),
        ..TerminalOptions::default()
    })
    .ok()?;
    let snapshot = terminal.snapshot();
    Some(Arc::new(Shared {
        terminal: Mutex::new(terminal),
        latest: Mutex::new(snapshot),
        events: Mutex::new(Vec::new()),
        wake: Arc::new(Wake::default()),
        grid: AtomicU32::new(packed(INITIAL_GRID)),
        resize_failing: AtomicBool::new(false),
        publish: Mutex::new(PublishState::new()),
    }))
}

/// A pty that hands out a burst in the chunks a real one does.
///
/// A pty master returns about a kilobyte per read however large a buffer it is
/// given, which is the fact the throttle used to be wrong about.
struct Burst {
    remaining: usize,
    chunk: usize,
}

impl io::Read for Burst {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let read = self.chunk.min(buffer.len()).min(self.remaining);
        buffer[..read].fill(b'x');
        self.remaining -= read;
        Ok(read)
    }
}

/// A pty nothing will ever write to again, and that nobody can close.
struct Silence;

impl io::Read for Silence {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        // Long enough to outlast the test, short enough that the thread is not
        // left parked for the life of the binary.
        thread::sleep(Duration::from_secs(5));
        Ok(0)
    }
}

#[test]
fn a_burst_is_read_at_full_speed_and_drawn_at_the_repaint_ceiling() {
    // A megabyte in the kilobyte chunks a pty actually hands out. Pausing for a
    // frame after each of those — which is what "throttle the reader" means —
    // is sixteen seconds for this, and the *program* on the far end runs at
    // that speed too, because it blocks on its own writes once the kernel's
    // buffer fills.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let flusher = Arc::new(Flusher::default());
    let reader = Burst {
        remaining: 1024 * 1024,
        chunk: 1024,
    };

    let started = Instant::now();
    read_loop(&Arc::downgrade(&shared), &flusher, reader);
    let took = started.elapsed();

    assert!(
        took < Duration::from_secs(4),
        "a megabyte took {took:?}; reading is being paced by the repaint interval"
    );
    assert!(shared.snapshot().text().contains("xxxxxxxx"));

    // And the ceiling still holds: at most one frame per interval, plus the one
    // the end of the pty always draws.
    let published = shared.publish_state().count;
    let ceiling = took.as_millis() / PAINT_INTERVAL.as_millis() + 2;
    assert!(
        u128::from(published) <= ceiling,
        "{published} frames for {took:?} is above the ceiling of {ceiling}"
    );
}

#[test]
fn the_last_batch_of_a_burst_is_drawn_even_though_nothing_follows_it() {
    // The batch that misses its window is *always* the last one, and the reader
    // is about to block on a pty that will never speak again. Without the
    // flusher the final screenful of a `cat` would sit invisible until the next
    // keystroke.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let flusher = Arc::new(Flusher::default());
    let running = flusher.clone();
    thread::spawn(move || flush_loop(&running));

    shared.feed(b"drawn straight away");
    assert_eq!(Publish::Done, shared.publish_if_due());

    shared.feed(b"\r\nand the tail of the burst");
    assert_eq!(
        Publish::Deferred,
        shared.publish_if_due(),
        "a batch inside the interval is parsed and left undrawn"
    );
    flusher.defer(Arc::downgrade(&shared));

    let deadline = Instant::now() + Duration::from_secs(5);
    while !shared.snapshot().text().contains("tail of the burst") {
        assert!(
            Instant::now() < deadline,
            "the deferred batch was never drawn"
        );
        thread::sleep(Duration::from_millis(2));
    }
    flusher.stop();
}

#[test]
fn a_closed_pane_frees_its_terminal_even_while_its_reader_is_still_blocked() {
    // A `sleep 60 &` keeps the pty open after the pane is closed, so the read
    // cannot return — and the thread owns the object whose drop would return
    // it. Holding the session weakly is what unties that: the emulator, its ten
    // thousand lines of scrollback and the pty go anyway.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let weak = Arc::downgrade(&shared);
    let reading = weak.clone();
    let flusher = Arc::new(Flusher::default());
    thread::spawn(move || read_loop(&reading, &flusher, Silence));
    thread::sleep(Duration::from_millis(50));

    drop(shared);
    assert!(
        weak.upgrade().is_none(),
        "the reader is still holding the terminal it can no longer reach"
    );
}

#[test]
fn a_resize_is_recorded_only_once_it_has_reached_the_pty() {
    // The size was recorded before the work was attempted, so one failed
    // `ioctl` — a flicker while a window edge is dragged — left the emulator at
    // the new geometry, the child at the old one, and every later frame
    // answering "unchanged" and never trying again.
    let grid = AtomicU32::new(packed(INITIAL_GRID));
    let wanted = TerminalSize::new(100, 30);

    assert!(needs_resize(&grid, wanted));
    // The resize failed, so nothing is recorded and the next frame tries again.
    assert!(needs_resize(&grid, wanted));

    record_resize(&grid, wanted);
    assert!(!needs_resize(&grid, wanted));
    assert!(needs_resize(&grid, TerminalSize::new(100, 31)));
}

#[test]
fn a_resize_that_reached_the_pty_is_not_sent_twice() {
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let handle = TerminalHandle(shared.clone());

    assert!(handle.resize(TerminalSize::new(100, 30)));
    assert!(!handle.resize(TerminalSize::new(100, 30)));
    assert_eq!(TerminalSize::new(100, 30), shared.lock().size());
    assert!(handle.resize(TerminalSize::new(60, 20)));
}
