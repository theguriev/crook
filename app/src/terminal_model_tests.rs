//! The model's own tests.
//!
//! Everything except the last one runs with no process at all. The last one
//! opens a real shell, because "bytes a shell printed reach the main thread and
//! repaint a pane" is the entire point of this file and there is nothing left of
//! it once the shell is stubbed out.

use std::sync::Arc;
use std::sync::mpsc;

use crook_terminal::Program;
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
fn a_closed_pane_lets_go_of_the_grid_that_was_recorded_for_it() {
    // A grid is recorded per pane so the shell that opens in it starts at the
    // size the layout measured. Pane ids are minted once and never reused, so
    // a grid left behind by a closed pane is read by nothing — and a window
    // spends a long day being split and closed. `sync` drops it, from the one
    // list that knows which panes are still open.
    let (_queue, mut app) = app();
    let model = app.update(|ctx| ctx.add_model(TerminalModel::new));
    let (kept, gone) = (pane(), pane());
    let measured = model.read(&app, |model, _| model.measured());
    measured.record(kept, TerminalSize::new(80, 24));
    measured.record(gone, TerminalSize::new(80, 24));

    app.update(|ctx| {
        model.update(ctx, |model, ctx| model.sync(&[(kept, None)], ctx));
    });

    assert_eq!(
        measured.get(kept),
        Some(TerminalSize::new(80, 24)),
        "the open pane lost the grid it will open at"
    );
    assert_eq!(
        measured.get(gone),
        None,
        "the closed pane's grid was kept for a pane that will never read it"
    );
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
    assert_ne!(
        packed(TerminalSize::new(80, 24).with_cell_size(7, 15)),
        packed(TerminalSize::new(80, 24).with_cell_size(7, 17)),
        "a zoom changes the cell and not the grid, and the pty has to hear it"
    );
}

#[test]
fn a_zoom_that_keeps_the_grid_still_tells_the_pty_the_new_cell_size() {
    // A pane small enough that one step of zoom leaves it ten by three.
    let Some(shared) = session() else {
        return;
    };
    let handle = TerminalHandle(shared.clone());
    let before = TerminalSize::new(10, 3).with_cell_size(8, 15);
    let after = TerminalSize::new(10, 3).with_cell_size(8, 17);
    assert!(handle.resize(before));
    assert_eq!(before, shared.lock().size(), "the first size did not land");

    handle.resize(after);
    assert_eq!(
        after,
        shared.lock().size(),
        "the pty still says the cell is the size it was before the zoom"
    );
}

#[test]
fn the_grid_is_drawn_on_the_panel_it_sits_in() {
    // A default cell whose background matched nothing would make an empty
    // screen thousands of rectangles instead of none.
    let palette = crook_palette();
    assert_eq!(palette.background, rgb(theme().surface));
    assert_eq!(palette.foreground, rgb(theme().text_primary));
    // The theme's red rather than the emulator's: Crook Dark's is lighter than
    // xterm's, so that it reads as text on this panel.
    assert_eq!(
        palette.ansi[1],
        rgb(theme().terminal.normal[1]),
        "a program that asked for red must still get red"
    );
    // Dim text resolves through this slot, so a palette that left it at the
    // default would hold a grey Crook does not use back by two thirds.
    assert_eq!(palette.dim_foreground, rgb(theme().text_muted));
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
        model.update(ctx, |model, ctx| {
            // Without the marks, as the workspace's plain-grid tests run: with
            // them, a finished command's output is harvested into a block,
            // and the live grid this reads holds only the next prompt.
            model.set_shell_marks(false);
            model.start(&[(pane, None)], &[], ctx)
        });
    });

    let failure = app.read(|ctx| model.as_ref(ctx).failure(pane).map(str::to_owned));
    if let Some(failure) = failure {
        eprintln!("skipped: no shell could be started here ({failure})");
        return;
    }

    // Printed by pieces, so that the line it prints is nowhere in the line it
    // types: a pty echoes what is typed into it, and a marker the command
    // itself contained was on screen before the shell had read a byte — the
    // test passed whether or not anything ran. On Windows the shell is
    // `%ComSpec%`, cmd, which has no `printf` and never ran the old line
    // either; its caret escapes the next character, and is gone from what
    // `echo` prints.
    let line = match cfg!(windows) {
        true => "echo crook-^was-here\r",
        false => "printf 'crook-%s\\n' was-here\n",
    };
    app.read(|ctx| model.as_ref(ctx).type_into(pane, line));

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

#[test]
fn a_program_that_asks_which_terminal_it_is_in_is_told_crook_and_its_version() {
    // cmd has no `stty`, `dd` or `tr` to read the answer with.
    if cfg!(windows) {
        return;
    }
    let (queue, mut app) = app();
    let model = app.update(|ctx| ctx.add_model(TerminalModel::new));
    let pane = pane();
    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.set_shell_marks(false);
            model.start(&[(pane, None)], &[], ctx)
        });
    });
    let failure = app.read(|ctx| model.as_ref(ctx).failure(pane).map(str::to_owned));
    if let Some(failure) = failure {
        eprintln!("skipped: no shell could be started here ({failure})");
        return;
    }

    // The answer arrives as input, so the line stops the tty echoing it and
    // holding it for a newline, reads exactly its length, and prints it with
    // the two escapes made visible — which nothing in the typed line is.
    let told = format!(
        ".P>|{} {}.\\",
        shell_integration::TERM_PROGRAM,
        shell_integration::VERSION
    );
    let line = format!(
        "stty -echo -icanon; printf '\\033[>q'; \
         dd bs=1 count={} 2>/dev/null | tr -c '[:print:]' .\n",
        told.len()
    );
    app.read(|ctx| model.as_ref(ctx).type_into(pane, &line));

    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        queue.run_until_parked();
        let answered = app.read(|ctx| {
            model
                .as_ref(ctx)
                .snapshot(pane)
                .is_some_and(|snapshot| snapshot.text().contains(&told))
        });
        if answered {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the program was never told {told:?}, got: {:?}",
            app.read(|ctx| model.as_ref(ctx).snapshot(pane).map(|grid| grid.text()))
        );
        thread::sleep(Duration::from_millis(10));
    }

    app.update(|ctx| {
        model.update(ctx, |model, ctx| model.sync(&[], ctx));
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
        latest: Mutex::new(Latest::new(snapshot)),
        blocks: Mutex::new(Arc::default()),
        unpublished: Mutex::new(Vec::new()),
        events: Mutex::new(Vec::new()),
        wake: Arc::new(Wake::default()),
        grid: AtomicU64::new(packed(INITIAL_GRID)),
        resize_failing: AtomicBool::new(false),
        scroll_remainder: AtomicU32::new(0.0f32.to_bits()),
        publish: Mutex::new(PublishState::new()),
        drives: std::sync::atomic::AtomicUsize::new(0),
        on_screen: AtomicBool::new(true),
        built: AtomicU64::new(0),
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
fn a_burst_of_bells_is_handed_over_as_one() {
    // Each bell the pane hands over reaches every plugin watching it, and a
    // plugin that rings on a bell plays a sound for each; a program printing
    // BEL in a loop would queue them by the dozen.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let bells = |events: &[TerminalEvent]| {
        events
            .iter()
            .filter(|event| matches!(event, TerminalEvent::Bell))
            .count()
    };

    shared.feed(b"\x07\x07\x07");
    shared.feed(b"\x07");
    shared.publish();
    assert_eq!(1, bells(&shared.take_events()), "one read, and the next");

    // Published twice before the main thread looked: the bell handed over
    // the first time still stands for the one the second time brings.
    shared.feed(b"\x07");
    shared.publish();
    shared.feed(b"\x07");
    shared.publish();
    assert_eq!(1, bells(&shared.take_events()), "one frame, and the next");

    // Once taken, the next bell is news again.
    shared.feed(b"\x07");
    shared.publish();
    assert_eq!(1, bells(&shared.take_events()));
}

#[test]
fn a_burst_of_notifications_is_handed_over_as_the_last_one() {
    // The bell's rule, with one difference: a notification carries words,
    // and the ones worth showing are the last the program said.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let said = |events: Vec<TerminalEvent>| {
        events
            .into_iter()
            .filter_map(|event| match event {
                TerminalEvent::Notification(notification) => notification.body,
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    shared.feed(b"\x1b]9;one\x07");
    shared.feed(b"\x07");
    shared.feed(b"\x1b]9;two\x07");
    shared.publish();
    let events = shared.take_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Bell)),
        "the bell between them was dropped with the first notification"
    );
    assert_eq!(vec!["two".to_owned()], said(events));

    shared.feed(b"\x1b]9;three\x07");
    shared.publish();
    assert_eq!(vec!["three".to_owned()], said(shared.take_events()));
}

#[test]
fn a_finished_command_is_handed_over_only_once_its_block_can_be_read() {
    // `crook pane wait --until finished` answers on this event, and the
    // `crook pane blocks --last 1` an agent runs straight after it reads the
    // published block list. An event handed over with the bytes that carried
    // it, an interval ahead of the picture, answered the wait while the read
    // still found the command running and refused it.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let finished = |events: &[TerminalEvent]| {
        events
            .iter()
            .any(|event| matches!(event, TerminalEvent::CommandFinished { .. }))
    };

    shared.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07work\r\n\x1b]133;C\x07the answer\r\n");
    shared.publish();
    assert!(!finished(&shared.take_events()));
    assert_eq!(
        BlockState::Executing,
        shared.snapshot().live_block.state,
        "the command was drawn running"
    );

    // The end, parsed and not drawn: the reader is inside the interval, and
    // the flusher has not come back for it yet. Fed rather than read off a
    // pty, so nothing races to publish it.
    shared.feed(b"\x1b]133;D;4\x07\x1b]133;A\x07$ \x1b]133;B\x07");
    let early = shared.take_events();
    assert!(
        !finished(&early),
        "the command's end was handed over while {} blocks were published and the \
         drawn command was {:?}",
        shared.blocks().len(),
        shared.snapshot().live_block.state
    );

    // What the flusher does an interval later.
    shared.publish();
    let events = shared.take_events();
    assert!(finished(&events), "the end never arrived: {events:?}");
    let blocks = shared.blocks();
    let [block] = &blocks.iter().collect::<Vec<_>>()[..] else {
        panic!("one finished block, published with the end");
    };
    assert_eq!(Some(4), block.exit);
    assert_ne!(BlockState::Executing, shared.snapshot().live_block.state);
}

/// A pty that hands out `bytes` in the kilobyte reads a real one does, and
/// then closes.
struct Scripted {
    bytes: Vec<u8>,
    at: usize,
}

impl io::Read for Scripted {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let rest = &self.bytes[self.at..];
        let read = rest.len().min(1024).min(buffer.len());
        buffer[..read].copy_from_slice(&rest[..read]);
        self.at += read;
        Ok(read)
    }
}

#[test]
fn a_pane_nobody_draws_builds_no_snapshot_however_much_it_prints() {
    // A snapshot is every cell of the viewport resolved through the palette
    // and compared with the last one, and a pane behind the active tab built
    // one per publication, up to the repaint ceiling, that no frame drew.
    // What the main thread does need of it still goes over with every
    // publication: the block list, the open block, and the events.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    shared.on_screen.store(false, Ordering::Relaxed);
    let handle = TerminalHandle(shared.clone());
    let built = handle.snapshots_built();

    let mut bytes = b"\x1b]133;A\x07$ \x1b]133;B\x07make\r\n\x1b]133;C\x07".to_vec();
    let mut line = 0;
    while bytes.len() < 1024 * 1024 {
        bytes.extend_from_slice(format!("line {line} of a long build log\r\n").as_bytes());
        line += 1;
    }
    bytes.extend_from_slice(b"\x07\x1b]133;D;2\x07\x1b]133;A\x07$ \x1b]133;B\x07");
    let flusher = Arc::new(Flusher::default());
    read_loop(
        &Arc::downgrade(&shared),
        &flusher,
        Scripted { bytes, at: 0 },
    );

    assert!(
        shared.publish_state().count > 0,
        "nothing was published, so nothing was tested"
    );
    assert_eq!(
        handle.snapshots_built(),
        built,
        "snapshots were built of a grid nothing draws"
    );
    let events = shared.take_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, TerminalEvent::Bell)),
        "the bell was not handed over: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, TerminalEvent::CommandFinished { .. })),
        "the command's end was not handed over: {events:?}"
    );
    let blocks = shared.blocks();
    let [block] = &blocks.iter().collect::<Vec<_>>()[..] else {
        panic!("one finished block, published with the end");
    };
    assert_eq!(Some(2), block.exit);
    assert_eq!(
        BlockState::AtPrompt,
        shared.glance().0.state,
        "the open block was not published"
    );
}

#[test]
fn a_pane_nobody_draws_shows_what_it_printed_the_moment_it_is_asked() {
    // Find, copy, the control socket and the frame that brings the pane back
    // all ask the handle, and a publication off screen built nothing for
    // them: the answer is built now, from what the shell has printed.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    shared.on_screen.store(false, Ordering::Relaxed);
    let handle = TerminalHandle(shared.clone());
    shared.feed(b"printed where nobody could see it");
    shared.publish();
    let built = handle.snapshots_built();

    let snapshot = handle.snapshot();
    assert!(
        snapshot
            .text()
            .contains("printed where nobody could see it"),
        "the pane showed what it showed before: {:?}",
        snapshot.text()
    );
    assert_eq!(handle.snapshots_built(), built + 1);
    assert!(
        Arc::ptr_eq(&snapshot, &handle.snapshot()),
        "asked again with nothing printed, the same snapshot comes back"
    );
    assert_eq!(handle.snapshots_built(), built + 1, "and nothing is built");
}

#[test]
fn a_snapshot_built_when_asked_comes_with_the_blocks_it_shows() {
    // Built from what the emulator holds when it is asked, which can be
    // newer than the last publication: a command that ended since is drawn
    // with the shell back at its prompt, so its block has to be in the list
    // beside it, or the command is in neither and gone from the pane until
    // the shell next prints.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    shared.on_screen.store(false, Ordering::Relaxed);
    shared.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07work\r\n\x1b]133;C\x07the answer\r\n");
    shared.publish();
    shared.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07");

    let snapshot = shared.snapshot();
    assert_eq!(BlockState::AtPrompt, snapshot.live_block.state);
    assert_eq!(
        shared.blocks().len(),
        1,
        "the snapshot was drawn at the prompt beside a list without the command"
    );
}

#[test]
fn a_pane_nobody_draws_is_read_with_the_blocks_its_picture_shows() {
    // `crook pane blocks` says whether a command is running from the picture
    // and which have ended from the list, and a worker behind the active tab
    // has its picture built when it is asked for: newer than the list its
    // last publication left, by whatever ended since. A list read before the
    // picture would have the command in neither, and answer for a worker that
    // had just finished that it is at rest with nothing run.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    shared.on_screen.store(false, Ordering::Relaxed);
    let handle = TerminalHandle(shared.clone());
    shared.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07work\r\n\x1b]133;C\x07the answer\r\n");
    shared.publish();
    shared.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07");

    let (snapshot, blocks) = handle.snapshot_with_blocks();
    assert_eq!(BlockState::AtPrompt, snapshot.live_block.state);
    let [block] = &blocks.iter().collect::<Vec<_>>()[..] else {
        panic!("the command that ended is in neither the picture nor the list");
    };
    assert_eq!(Some(0), block.exit);
}

/// Every update a model handed on, in the order it handed them.
struct Heard(Vec<TerminalUpdate>);

impl Entity for Heard {
    type Event = ();
}

/// A model with one pane, whose bytes a test hands over as the reader thread
/// would, and what the workspace hears of them.
struct Batches {
    queue: Arc<LocalQueue>,
    app: App,
    /// Held so the model lives as long as the test: one dropped hands nothing
    /// on.
    _model: ModelHandle<TerminalModel>,
    heard: ModelHandle<Heard>,
    shared: Arc<Shared>,
    /// The pane, as what the workspace hears names it.
    pane: PaneId,
}

impl Batches {
    /// `marks` is whether the pane's shell reports command marks, which
    /// decides what its first prompt is. `None` where no pty can be opened.
    fn new(marks: bool) -> Option<Self> {
        let shared = session()?;
        let (live_block, moved) = shared.glance();
        let (queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(TerminalModel::new));
        let heard = app.update(|ctx| {
            ctx.add_model(|ctx| {
                ctx.subscribe_to_model(&model, |heard: &mut Heard, _, update, _| {
                    heard.0.push(update.clone());
                });
                Heard(Vec::new())
            })
        });
        let pane = pane();
        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                let integration = shell_integration::Session::open(
                    pane,
                    &shell_integration::Options {
                        enabled: false,
                        ..Default::default()
                    },
                );
                model.sessions.insert(
                    pane,
                    Session {
                        shared: shared.clone(),
                        _integration: integration,
                        long_running_timer: false,
                        live_block,
                        moved,
                        title: None,
                        directory: None,
                        running: None,
                        token: None,
                        marks,
                        prompted: false,
                        shell: model.shell(),
                    },
                );
                model.watch(pane, ctx);
            });
        });
        Some(Self {
            queue,
            app,
            _model: model,
            heard,
            shared,
            pane,
        })
    }

    /// Hands `reads` over as one batch — the reader parsed them all before
    /// the main thread looked — and answers what the workspace heard of the
    /// command and its title, in order.
    fn batch(&mut self, reads: &[&str]) -> Vec<&'static str> {
        for read in reads {
            self.shared.feed(read.as_bytes());
        }
        self.shared.publish();
        self.queue.run_until_parked();
        let heard = self.app.update(|ctx| {
            self.heard
                .update(ctx, |heard, _| std::mem::take(&mut heard.0))
        });
        heard
            .iter()
            .filter_map(|update| match update {
                TerminalUpdate::Running(_, Some(_)) => Some("start"),
                TerminalUpdate::Running(_, None) => Some("stop"),
                TerminalUpdate::Title(..) => Some("title"),
                TerminalUpdate::CommandFinished { .. } => Some("finished"),
                TerminalUpdate::Prompted(_) => Some("prompted"),
                _ => None,
            })
            .collect()
    }
}

const PROMPT: &str = "\x1b]133;A\x07$ ";
const STARTED: &str = "\x1b]133;B\x07claude\r\n\x1b]133;C\x07";

#[test]
fn a_command_stopping_is_handed_over_after_the_title_written_before_it() {
    // The workspace takes back a running a program's title said when the
    // pane's command stops, since a program killed mid-turn says nothing
    // more. Its last title and the shell's marks after it come in one batch
    // whenever the main thread did not look between them, and a stop handed
    // over ahead of the title was taken back by the title saying running
    // again, for good. A start still goes first: what the command wrote,
    // its first title included, came after it.
    let Some(mut batches) = Batches::new(true) else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    assert_eq!(batches.batch(&[PROMPT]), ["prompted"]);

    let started = format!("{STARTED}\x1b]0;\u{2733} Claude Code\x07");
    assert_eq!(batches.batch(&[&started]), ["start", "title"]);
    // An `A` with no `D`: a shell whose marks start prompts and do not end
    // commands. Two reads in the batch here, and one below.
    let title = "\x1b]0;\u{25d0} Fix the flaky test\x07";
    assert_eq!(batches.batch(&[title, PROMPT]), ["title", "stop"]);

    // A `D`: the stop goes out just ahead of the end, so the command is
    // still heard to stop before it is heard to finish, and a title the
    // prompt writes after the `A` is heard after both.
    assert_eq!(batches.batch(&[STARTED]), ["start"]);
    let ended = format!(
        "\x1b]0;\u{25d3} Fix the flaky test\x07\x1b]133;D;137\x07{PROMPT}\
         \x1b]0;eugen@devbox: ~/src\x07"
    );
    assert_eq!(
        batches.batch(&[&ended]),
        ["title", "stop", "finished", "title"]
    );
}

#[test]
fn a_first_prompt_with_marks_is_handed_over_after_the_title_written_before_it() {
    // An agent the shell's startup files ran: no command is marked for it, so
    // the first prompt is the word that it has gone, and it takes back a
    // running the agent's title said — after that title, as the stop above.
    let Some(mut batches) = Batches::new(true) else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let title = "\x1b]0;\u{25d0} Fix the flaky test\x07";
    assert_eq!(batches.batch(&[title, PROMPT]), ["title", "prompted"]);
}

#[test]
fn a_first_prompt_without_marks_is_handed_over_before_the_title_beside_it() {
    // Where nothing marks a prompt, the first prompt is the first text, and
    // that can be an agent's own first frame: handed over after the frame's
    // title, it would take back a running the agent had only just said.
    let Some(mut batches) = Batches::new(false) else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let frame = "\x1b]0;\u{25d0} Fix the flaky test\x07Claude Code\r\n";
    assert_eq!(batches.batch(&[frame]), ["prompted", "title"]);
}

impl Batches {
    /// Hands `reads` over as one batch, and answers everything the workspace
    /// heard of it.
    fn everything(&mut self, reads: &[&str]) -> Vec<TerminalUpdate> {
        for read in reads {
            self.shared.feed(read.as_bytes());
        }
        self.shared.publish();
        self.queue.run_until_parked();
        self.app.update(|ctx| {
            self.heard
                .update(ctx, |heard, _| std::mem::take(&mut heard.0))
        })
    }

    /// Takes the pane off screen, as a frame that does not draw it says.
    fn hide(&self) {
        self.shared.on_screen.store(false, Ordering::Relaxed);
    }

    /// How many snapshots have been built for the pane.
    fn built(&self) -> u64 {
        TerminalHandle(self.shared.clone()).snapshots_built()
    }
}

#[test]
fn a_pane_nobody_draws_is_still_heard_running_and_repainted_with_no_snapshot_built() {
    // The row of a tab behind the active one says what its pane is running,
    // and the frame that brings the pane back is asked for by a repaint, so
    // both still come of every publication — off the open block, which is
    // published without the snapshot nothing would draw.
    let Some(mut batches) = Batches::new(true) else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    batches.hide();
    let built = batches.built();
    let pane = batches.pane;

    let heard = batches.everything(&[PROMPT]);
    assert!(
        heard.contains(&TerminalUpdate::Prompted(pane)),
        "the first prompt was not heard: {heard:?}"
    );
    let heard = batches.everything(&[STARTED, "the first of its output\r\n"]);
    assert!(
        heard.contains(&TerminalUpdate::Running(pane, Some("claude".to_owned()))),
        "the command was not heard running: {heard:?}"
    );
    assert!(
        heard.contains(&TerminalUpdate::Repainted(pane)),
        "the frame that would bring the pane back was never asked for: {heard:?}"
    );
    assert_eq!(
        batches.built(),
        built,
        "a snapshot was built of a grid nothing draws"
    );
}

#[test]
fn a_shell_with_no_marks_is_heard_at_its_first_prompt_where_nobody_can_see_it() {
    // The one thing the model needs a grid's text for: a shell that marks
    // nothing has come to its prompt once it has printed anything at all. A
    // pane off screen is published with no snapshot, and the one that text
    // is read from is built for it.
    let Some(mut batches) = Batches::new(false) else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    batches.hide();
    let pane = batches.pane;

    let heard = batches.everything(&["   "]);
    assert!(
        !heard.contains(&TerminalUpdate::Prompted(pane)),
        "blanks were heard as a prompt: {heard:?}"
    );
    let heard = batches.everything(&["$ "]);
    assert!(
        heard.contains(&TerminalUpdate::Prompted(pane)),
        "the prompt of a shell nobody can see was never heard: {heard:?}"
    );
}

impl Batches {
    /// Hands `read` over and answers where the workspace heard the shell
    /// went: `here` and a path for a directory on this machine, and the host
    /// and a path for one on another.
    fn moves(&mut self, read: &str) -> Vec<(String, PathBuf)> {
        self.shared.feed(read.as_bytes());
        self.shared.publish();
        self.queue.run_until_parked();
        let heard = self.app.update(|ctx| {
            self.heard
                .update(ctx, |heard, _| std::mem::take(&mut heard.0))
        });
        heard
            .into_iter()
            .filter_map(|update| match update {
                TerminalUpdate::WorkingDirectory(_, path) => Some(("here".to_owned(), path)),
                TerminalUpdate::RemoteDirectory(_, remote) => Some((remote.host, remote.directory)),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn a_directory_on_another_machine_is_heard_as_that_machine_s_and_one_here_as_this_one_s() {
    // The far side of an ssh with Crook's snippet pasted in, in a directory
    // this machine has too, and back: the path is the same all three times,
    // and where it is is not. A report naming no host, `localhost` or this
    // machine's own name is this machine's, which is what every shell Crook
    // starts says — fish 4 by itself, and the snippets by the name the
    // system gave when the shell started.
    let Some(mut batches) = Batches::new(true) else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let here = gethostname::gethostname().to_string_lossy().into_owned();
    let path = PathBuf::from("/home/eugen/work/app");
    let osc = |host: &str| format!("\x1b]7;file://{host}/home/eugen/work/app\x07");

    assert_eq!(
        batches.moves(&osc(&here)),
        [("here".to_owned(), path.clone())]
    );
    assert_eq!(
        batches.moves(&osc("buildbox.internal")),
        [("buildbox.internal".to_owned(), path.clone())]
    );
    assert_eq!(batches.moves(&osc("")), [("here".to_owned(), path.clone())]);
    assert_eq!(
        batches.moves(&osc("buildbox.internal")),
        [("buildbox.internal".to_owned(), path.clone())]
    );
    assert_eq!(
        batches.moves(&osc("localhost")),
        [("here".to_owned(), path.clone())]
    );
    // A second report of the same place says nothing new.
    assert_eq!(batches.moves(&osc("localhost")), []);
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
    let grid = AtomicU64::new(packed(INITIAL_GRID));
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

#[test]
fn a_scroll_shorter_than_a_line_is_carried_to_the_next_one() {
    // A trackpad reports pixels, which reach this as a fraction of a line an
    // event. Rounded, a slow drag was dropped and never moved the grid;
    // carried, the fractions add up to a line.
    assert_eq!((0, 0.5), whole_lines(0.0, 0.5));
    assert_eq!((1, 0.0), whole_lines(0.5, 0.5));

    // A wheel notch is a whole line and leaves nothing behind.
    assert_eq!((1, 0.0), whole_lines(0.0, 1.0));

    // A flick crosses several lines at once, and the fraction still carries.
    assert_eq!((2, 0.5), whole_lines(0.0, 2.5));

    // The other way carries a negative remainder, and truncation never borrows
    // a line it has not been given.
    assert_eq!((0, -0.5), whole_lines(0.0, -0.5));
    assert_eq!((-1, 0.0), whole_lines(-0.5, -0.5));
}

#[test]
fn a_line_typed_with_a_space_in_front_names_the_tab_without_it() {
    // `crook -e htop` types ` exec htop`, the space keeping it out of the
    // shell's history. The block keeps the line as it was typed, which is
    // what Copy command and Run again hand back; the tab's name and the
    // status line are read from here, and a row that began with a space was
    // drawn a cell to the right of every other row.
    let typed = crate::exec_line(&["htop".to_owned()]);
    assert_eq!(" exec htop", typed);

    let mut emulator =
        crook_terminal::Emulator::new(TerminalSize::new(40, 5), 100, Palette::default());
    emulator.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");
    emulator.command_submitted(&typed);
    assert_eq!(
        None,
        running_command(&emulator.live_block()),
        "a line the shell has not started is not running"
    );

    emulator.advance(format!("{typed}\r\n\x1b]133;C\x07").as_bytes());
    let live = emulator.live_block();
    assert_eq!(Some(typed.as_str()), live.command.as_deref());
    assert_eq!(Some("exec htop"), running_command(&live).as_deref());

    emulator.advance(b"\x1b]133;D;0\x07\x1b]133;A\x07$ \x1b]133;B\x07");
    assert_eq!(None, running_command(&emulator.live_block()));
}

/// How long a test watches for the terminal to come free while a writer is
/// kept from installing the snapshot it built.
///
/// Not a wait for something to happen: on the right code the terminal never
/// comes free here and the test spends the whole of it. On the wrong code it
/// comes free the instant the writer has synced its blocks, so this only has to
/// outlast a scheduler that has parked the writer's thread in between — which
/// is also the one way the wrong code could slip through.
const GAP_WINDOW: Duration = Duration::from_millis(100);

/// Runs `write` on a thread of its own with the snapshot slot held, and asserts
/// that it does not let go of the terminal before its snapshot is in the slot.
///
/// The gap is the whole race. A writer that builds its snapshot and syncs the
/// blocks under the terminal's lock, then installs the snapshot after letting
/// go, leaves a moment in which the other writer can build, sync and install a
/// newer pair — and then have its snapshot overwritten by the older one, beside
/// its own newer block list. A command that finished during a drag was painted
/// twice, once as a block and once in the live viewport, with its tab still
/// labelled running.
///
/// Holding the slot here turns that moment into one that lasts, so the test
/// needs no second writer racing the first: a terminal that can be taken while
/// the slot is still waiting is the gap.
///
/// `write` must leave the drawn content as it was. That is what lets the count
/// on the installed snapshot say where the writer has got to.
fn assert_the_terminal_is_held_until_the_snapshot_is_published(
    shared: &Arc<Shared>,
    write: impl FnOnce() + Send + 'static,
) {
    let slot = shared.latest.lock().unwrap_or_else(PoisonError::into_inner);
    // The terminal is clean, so the snapshot a writer builds is this very `Arc`
    // again, and one more count on it is the writer holding a snapshot it has
    // built and not yet installed — under the terminal's lock either way.
    let installed = Arc::strong_count(&slot.snapshot);
    let writer = thread::spawn(write);

    let deadline = Instant::now() + REPLY_TIMEOUT;
    while Arc::strong_count(&slot.snapshot) == installed {
        assert!(
            Instant::now() < deadline,
            "the writer never built a snapshot"
        );
        thread::yield_now();
    }

    let watching = Instant::now();
    while watching.elapsed() < GAP_WINDOW {
        assert!(
            shared.terminal.try_lock().is_err(),
            "the terminal was let go before its snapshot was published, so the \
             other writer could publish a newer one in between and be overwritten"
        );
        thread::yield_now();
    }

    drop(slot);
    writer.join().expect("the writer panicked");
}

#[test]
fn a_key_press_publishes_what_it_drew_before_it_lets_go_of_the_terminal() {
    // The UI thread's writer: typing, a resize, the wheel, all through `drive`.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let handle = TerminalHandle(shared.clone());

    assert_the_terminal_is_held_until_the_snapshot_is_published(&shared, move || {
        handle.send_key(Key::Char('a'), Modifiers::default());
    });
}

#[test]
fn the_reader_publishes_what_it_drew_before_it_lets_go_of_the_terminal() {
    // The other writer: the reader thread and the flusher, through `publish`.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    let publishing = shared.clone();

    assert_the_terminal_is_held_until_the_snapshot_is_published(&shared, move || {
        publishing.publish();
    });
}

#[test]
fn the_mouse_modes_a_pointer_move_asks_for_wait_on_nothing_the_reader_holds() {
    // Every pointer move over a pane asks this, on the thread that draws, and
    // asking took the terminal's lock and rebuilt the snapshot under it: a
    // reader holding the lock through a flood held up every move the window
    // was given. The modes the frame showed are in the snapshot it was drawn
    // from.
    let Some(shared) = session() else {
        eprintln!("skipped: no pty could be opened here");
        return;
    };
    shared.feed(b"\x1b[?1003h\x1b[?1006h");
    shared.publish();
    let handle = TerminalHandle(shared.clone());

    let parsing = shared.lock();
    let asked = returns(move || handle.mouse_modes());
    drop(parsing);

    let modes = asked.expect("the mouse modes waited on the terminal's lock");
    assert!(
        modes.motion,
        "the modes were not the ones the program asked for"
    );
    assert!(modes.sgr);
}

/// How long something a person did is given to come back before the test
/// calls the window frozen.
///
/// Every call here returns in microseconds once nothing writes to the pty
/// under the terminal's lock. The bound is for the code that did, which never
/// returned at all, so that it fails rather than hanging the suite.
const RETURNS_WITHIN: Duration = Duration::from_secs(5);

/// Runs `work` on a thread of its own and hands back what it returned, or
/// `None` if it has not returned within [`RETURNS_WITHIN`] — which, on the UI
/// thread, would have been a window that stopped drawing. A thread that did
/// not return is left where it is stuck: joining it would hang the suite.
fn returns<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(work());
    });
    finished.recv_timeout(RETURNS_WITHIN).ok()
}

/// The thread that draws what a reader parsed too soon to draw, as a window
/// has one. Stopped when the test is done with it.
fn repainting() -> Arc<Flusher> {
    let flusher = Arc::new(Flusher::default());
    let running = flusher.clone();
    thread::spawn(move || flush_loop(&running));
    flusher
}

/// What every shell this file starts is given in its environment: `given`,
/// then an empty `HISTFILE` over it and over the one the shell inherits.
///
/// The shells inherit the home of whoever runs the suite. An interactive bash
/// — the one a test starts by name, and `/bin/sh` where that is bash — reads
/// its history file as it starts and cuts it to `HISTFILESIZE` lines, 500
/// where no rc file says otherwise, then saves its own lines into it as it
/// ends: every run replaced somebody's `~/.bash_history` with a test's filler.
/// Empty, because unset is `~/.bash_history` again; and from the start, since
/// a line typed into the shell comes after the cut. A hung-up bash with a
/// history file also kept writing it past its `SIGKILL` on a busy disk, for
/// longer than a closed pane's ending waits to reap it.
fn without_history(given: &[(&str, &str)]) -> Vec<(String, String)> {
    given
        .iter()
        .chain(&[("HISTFILE", "")])
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// A session started the way [`spawn_session`] starts one — its reader
/// and its writer each on a thread, its deferred batches drawn by `flusher` —
/// running `program`, or `None` where it cannot be started.
fn started(program: Program, flusher: &Arc<Flusher>) -> Option<Arc<Shared>> {
    started_with(program, &[], flusher)
}

/// [`started`], with `given` in the shell's environment. See
/// [`without_history`].
fn started_with(
    program: Program,
    given: &[(&str, &str)],
    flusher: &Arc<Flusher>,
) -> Option<Arc<Shared>> {
    let terminal = Terminal::spawn(TerminalOptions {
        program,
        size: TerminalSize::new(80, 24),
        environment: without_history(given),
        ..TerminalOptions::default()
    })
    .ok()?;
    start(terminal, PaneId::next(), flusher).ok()
}

/// Where bash is, or `None` where it is not installed — and on Windows, whose
/// shell is cmd.
fn bash() -> Option<&'static str> {
    if cfg!(windows) {
        return None;
    }
    ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
        .into_iter()
        .find(|bash| std::path::Path::new(bash).exists())
}

/// `script`, run by `sh`. Not on Windows, whose shell is cmd.
fn sh(script: &str, arguments: &[&str]) -> Option<Program> {
    if cfg!(windows) {
        return None;
    }
    let mut args = vec!["-c", script];
    args.extend_from_slice(arguments);
    Some(Program::command("/bin/sh", args))
}

/// Whether the pane's published grid comes to show `wanted`.
fn comes_to_show(shared: &Shared, wanted: &str) -> bool {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while Instant::now() < deadline {
        if shared.snapshot().text().contains(wanted) {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// A file that is removed when the test is done with it, passed or not.
struct Removed(PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn a_megabyte_pasted_into_cat_returns_at_once_and_arrives_whole() {
    // `cat` reads only as fast as its output is read, and its output is read
    // by the pane's reader — which needed the terminal's lock to parse it,
    // while the paste held that lock writing into a pty `cat` had stopped
    // reading. The window froze for good at about eighteen kilobytes.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let file = Removed(std::env::temp_dir().join(format!(
        "crook-pasted-into-cat-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    let path = file.0.to_string_lossy().into_owned();
    // `tee` is `cat` that also keeps what it read, which is how the test sees
    // all of it: the grid only ever holds a screenful.
    let Some(program) = sh("stty -echo; echo crook-ready; exec tee \"$0\"", &[&path]) else {
        return;
    };
    let flusher = repainting();
    let Some(shared) = started(program, &flusher) else {
        eprintln!("skipped: no shell could be started here");
        return;
    };
    assert!(
        comes_to_show(&shared, "crook-ready"),
        "the child never got ready"
    );

    // Lines, because a terminal in canonical mode holds at most four
    // kilobytes of one.
    let mut text = String::new();
    while text.len() < 1024 * 1024 {
        text.push_str(&format!("line {:07} {}\n", text.len(), "x".repeat(60)));
    }
    let handle = TerminalHandle(shared.clone());
    let pasting = text.clone();
    assert_eq!(
        Some(true),
        returns(move || handle.paste(&pasting)),
        "a megabyte pasted into cat did not come back: the window would be frozen"
    );

    // The paste turns each newline into the carriage return Enter sends, and
    // the line discipline turns it back.
    let deadline = Instant::now() + REPLY_TIMEOUT;
    let kept = loop {
        let kept = std::fs::read(&file.0).unwrap_or_default();
        if kept.len() >= text.len() || Instant::now() > deadline {
            break kept;
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(
        kept == text.as_bytes(),
        "{} of {} bytes arrived, or not in the order they were pasted",
        kept.len(),
        text.len()
    );
    flusher.stop();
}

#[test]
fn a_long_composed_line_submitted_to_an_interactive_shell_returns_at_once() {
    // The composer's Enter on a pasted block: a shell's line editor echoes
    // what it reads, so it stops reading once its echo has nowhere to go —
    // and the echo went nowhere while the submit held the lock the reader
    // needed. Thirty-two kilobytes froze bash and sixteen zsh and fish.
    let Some(bash) = bash() else {
        eprintln!("skipped: no bash here");
        return;
    };
    let flusher = repainting();
    let Some(shared) = started(
        Program::command(bash, ["--norc", "--noprofile", "-i"]),
        &flusher,
    ) else {
        eprintln!("skipped: no shell could be started here");
        return;
    };

    // Commands that do nothing, and one at the end whose output is nowhere in
    // what was typed: the shell has run every line once it is on the grid.
    let mut block = String::new();
    while block.len() < 64 * 1024 {
        block.push_str(&format!(": {:06} {}\n", block.len(), "y".repeat(80)));
    }
    block.push_str("echo crook-ran-$((6 * 7))");
    let handle = TerminalHandle(shared.clone());
    assert_eq!(
        Some(true),
        returns(move || handle.submit(&block)),
        "a 64 KiB submit into an interactive shell did not come back"
    );
    assert!(
        comes_to_show(&shared, "crook-ran-42"),
        "the shell did not run all of what was submitted"
    );
    flusher.stop();
}

/// A program that asks the terminal where its cursor is, and what it is,
/// thousands of times, and never reads an answer: raw mode, so the line
/// discipline drops nothing, and then a marker once the asking is done.
const FLOOD: &str = "stty raw -echo; i=0; \
    while [ $i -lt 6000 ]; do printf '\\033[6n\\033[c'; i=$((i + 1)); done; \
    printf 'crook-%s\\r\\n' asked; exec sleep 60";

#[test]
fn a_program_that_floods_queries_and_never_reads_holds_up_neither_its_reader_nor_a_key() {
    // The answers were written by the reader, under the terminal's lock, and
    // a program that does not read them fills the pty after twenty kilobytes.
    // The reader then waited in that write for good, holding the lock: the
    // rest of the program's output was never parsed, and a key press — the
    // Ctrl-C to stop it — or a pointer moving over the pane waited too.
    let Some(program) = sh(FLOOD, &[]) else {
        return;
    };
    let flusher = repainting();
    let Some(shared) = started(program, &flusher) else {
        eprintln!("skipped: no shell could be started here");
        return;
    };

    assert!(
        comes_to_show(&shared, "crook-asked"),
        "the reader stopped parsing a program that floods queries"
    );
    let handle = TerminalHandle(shared.clone());
    let control = Modifiers {
        control: true,
        ..Modifiers::default()
    };
    assert_eq!(
        Some(true),
        returns(move || handle.send_key(Key::Char('c'), control)),
        "a key press into a pane flooded with queries did not come back"
    );
    let handle = TerminalHandle(shared.clone());
    assert!(
        returns(move || handle.mouse_modes()).is_some(),
        "a pointer moving over a pane flooded with queries did not come back"
    );
    flusher.stop();
}

#[test]
fn a_program_that_floods_queries_and_never_reads_does_not_stop_another_pane_drawing() {
    // One thread draws every pane's late batches, and it took the flooded
    // pane's lock to draw it: once that pane's reader was stuck holding the
    // lock, the end of every other pane's output stayed undrawn.
    //
    // The flood here is output with a question in every line, as fast as
    // `yes` prints it, rather than questions alone: the reader then has a
    // batch parsed and waiting for that thread at the moment its answers
    // fill the pty, which is what sends the thread into the stuck pane.
    let (Some(flooding), Some(quiet)) = (
        sh(
            "stty raw -echo; exec yes \"$(printf 'crook%070d\\033[6n' 0)\"",
            &[],
        ),
        sh(
            "printf 'crook-%s\\n' first; sleep 0.004; printf 'crook-%s\\n' last; exec sleep 60",
            &[],
        ),
    ) else {
        return;
    };
    let flusher = repainting();
    let Some(flooded) = started(flooding, &flusher) else {
        eprintln!("skipped: no shell could be started here");
        return;
    };
    // Long enough for the flood to have filled the pty, and the repaint
    // thread to have come back for the flooded pane.
    thread::sleep(Duration::from_millis(500));

    // Printed twice within a frame, so the second lands inside the repaint
    // interval and only the shared thread draws it.
    let Some(other) = started(quiet, &flusher) else {
        eprintln!("skipped: no shell could be started here");
        return;
    };
    assert!(
        comes_to_show(&other, "crook-last"),
        "a pane beside one flooded with queries stopped drawing the end of its output"
    );
    drop(flooded);
    flusher.stop();
}

/// The longest closing a handful of panes may hold the thread that draws.
///
/// A fifth of what the four closes in the first test below took there when
/// each ended its shell in place, one after another: the first look at the
/// child comes before it can have gone, so a shell that goes at the hang-up
/// cost 50 ms and one that ignores it 200. Handed to threads of their own,
/// the four take well under a millisecond.
const CLOSE_BUDGET: Duration = Duration::from_millis(100);

/// A model with a `/bin/sh` running in each of its panes.
struct Shells {
    queue: Arc<LocalQueue>,
    app: App,
    model: ModelHandle<TerminalModel>,
    panes: Vec<PaneId>,
}

impl Shells {
    /// One shell per entry, ignoring the hang-up a close sends first where
    /// its entry says so, once every one has said it is ready — or `None`
    /// where none can be started, and on Windows, whose close is
    /// `TerminateProcess` and has no hang-up to ignore.
    fn start(ignoring_the_hang_up: &[bool]) -> Option<Self> {
        Self::start_with(ignoring_the_hang_up, &[])
    }

    /// [`Self::start`], with `given` in every shell's environment. See
    /// [`without_history`].
    fn start_with(ignoring_the_hang_up: &[bool], given: &[(&str, &str)]) -> Option<Self> {
        if cfg!(windows) {
            return None;
        }
        let (queue, mut app) = app();
        let model = app.update(|ctx| ctx.add_model(TerminalModel::new));
        let panes: Vec<_> = ignoring_the_hang_up.iter().map(|_| pane()).collect();
        let open: Vec<_> = panes.iter().map(|pane| (*pane, None)).collect();
        app.update(|ctx| {
            model.update(ctx, |model, ctx| {
                model.set_shell(Some(PathBuf::from("/bin/sh")));
                model.set_environment(without_history(given));
                model.set_shell_marks(false);
                model.set_shell_login(false);
                // The order a window takes: the shells are expected before
                // its first frame, and started after it.
                model.expect();
                model.start(&open, &[], ctx);
            });
        });
        let shells = Self {
            queue,
            app,
            model,
            panes,
        };

        let handles = shells.handles();
        if handles.len() < shells.panes.len() {
            eprintln!("skipped: no shell could be started here");
            return None;
        }
        for ((pane, handle), deaf) in shells.panes.iter().zip(&handles).zip(ignoring_the_hang_up) {
            // Printed by pieces, so the marker is nowhere in the line the pty
            // echoes: on screen, it is the shell having run the trap before it.
            let line = match deaf {
                true => "trap '' HUP; printf 'crook-%s\\n' ready\n",
                false => "printf 'crook-%s\\n' ready\n",
            };
            shells
                .app
                .read(|ctx| shells.model.as_ref(ctx).type_into(*pane, line));
            assert!(
                comes_to_show(&handle.0, "crook-ready"),
                "a shell never got ready"
            );
        }
        Some(shells)
    }

    /// The terminal of every pane that still has one.
    fn handles(&self) -> Vec<TerminalHandle> {
        self.app.read(|ctx| {
            self.panes
                .iter()
                .filter_map(|pane| self.model.as_ref(ctx).handle(*pane))
                .collect()
        })
    }
}

/// Whether the session's child has been ended and reaped, by the model's own
/// threads: [`Terminal::has_exited`] reaps nothing, so a test asking it cannot
/// be what collected the child.
fn comes_to_be_reaped(shared: &Shared) -> bool {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while Instant::now() < deadline {
        if shared.lock().has_exited() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn closing_panes_returns_at_once_and_their_shells_still_end_even_ignoring_the_hang_up() {
    // A close ended its shell on the thread that draws, through a kill that
    // sleeps: every tab closed was a three-frame hitch, and "Close other
    // tabs" on fifteen froze the window for most of a second.
    let Some(mut shells) = Shells::start(&[false, false, true, true]) else {
        return;
    };
    let handles = shells.handles();

    let closing = Instant::now();
    let model = &shells.model;
    shells.app.update(|ctx| {
        model.update(ctx, |model, ctx| model.sync(&[], ctx));
    });
    let took = closing.elapsed();
    assert!(
        took < CLOSE_BUDGET,
        "closing four panes held the thread that draws for {took:?}"
    );

    for handle in &handles {
        assert!(
            comes_to_be_reaped(&handle.0),
            "a closed pane's shell was never ended, or never reaped"
        );
        let exit = handle.0.lock().try_wait().ok().flatten();
        assert!(
            exit.as_ref().is_some_and(|exit| exit.signal.is_some()),
            "a closed pane's shell was not ended by a signal: {exit:?}"
        );
    }

    // And nothing is left holding a session: the threads that ended the shells
    // let go of theirs, the readers saw the ptys close, and the one chain that
    // asks after every shell came round once more and found none.
    let sessions: Vec<_> = handles
        .into_iter()
        .map(|handle| Arc::downgrade(&handle.0))
        .collect();
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while sessions.iter().any(|session| session.upgrade().is_some()) {
        assert!(
            Instant::now() < deadline,
            "a closed pane's session is still held by something"
        );
        shells.queue.run_until_parked();
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_window_that_goes_hears_its_shells_ended_before_its_wait_for_them_returns() {
    // The threads that end the shells die with the process, and a shell that
    // ignores the hang-up outlives Crook if its thread never had the 200 ms
    // to kill it — so a window that is going ends them all at once and waits.
    let Some(mut shells) = Shells::start(&[true, true, false]) else {
        return;
    };
    let handles = shells.handles();

    let closing = Instant::now();
    let model = &shells.model;
    let endings = shells
        .app
        .update(|ctx| model.update(ctx, |model, _| model.close_all()));
    let took = closing.elapsed();
    assert!(
        took < CLOSE_BUDGET,
        "ending a window's shells held the thread that draws for {took:?}"
    );

    assert!(
        endings.wait(REPLY_TIMEOUT),
        "a window that is going gave up waiting for its shells"
    );
    let waited = closing.elapsed();
    for handle in &handles {
        if handle.0.lock().has_exited() {
            continue;
        }
        // What the wait promises is the kill, and the reap only as a rule: a
        // shell the kernel is finishing a write for goes once the write is
        // done, which can be after its ending stopped waiting to reap it. The
        // wait must still not have come back before that ending did.
        assert!(
            waited >= REAP_TIMEOUT,
            "the wait came back after {waited:?}, before a shell it was waiting for had been ended"
        );
        assert!(
            comes_to_be_reaped(&handle.0),
            "a shell the wait said was ended never went"
        );
    }

    // And the panes the strip still holds, which the session file brings
    // back, are not given a shell again on the way out: not by the strip
    // moving, and not by a frame drawn after this one.
    let open: Vec<_> = shells.panes.iter().map(|pane| (*pane, None)).collect();
    shells.app.update(|ctx| {
        model.update(ctx, |model, ctx| model.sync(&open, ctx));
    });
    assert!(
        shells.handles().is_empty(),
        "a window that is going opened a shell again"
    );
    assert!(
        !shells.app.read(|ctx| model.as_ref(ctx).is_expected()),
        "a window that is going would start its shells again at its next frame"
    );
}

#[test]
fn a_shell_started_here_leaves_the_history_file_it_was_handed_alone() {
    // A shell started both of the ways this file starts one, each handed a
    // history longer than bash keeps by default: a bash that read it would
    // cut it as it started, and add to it as it ended. Handed here, rather
    // than found in the home of whoever runs the suite, so the test risks
    // nobody's history to see it.
    let Some(bash) = bash() else {
        eprintln!("skipped: no bash here");
        return;
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let history = Removed(std::env::temp_dir().join(format!(
        "crook-history-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    let kept: String = (0..600).map(|line| format!(": kept {line}\n")).collect();
    std::fs::write(&history.0, &kept).expect("the history is written");
    let modified = || std::fs::metadata(&history.0).and_then(|file| file.modified());
    let written = modified().expect("the history is there");
    let path = history.0.to_string_lossy().into_owned();
    let given = [("HISTFILE", path.as_str())];

    // As a pane opens one, and hung up as a pane is closed.
    if let Some(mut shells) = Shells::start_with(&[false], &given) {
        let handles = shells.handles();
        let model = &shells.model;
        shells.app.update(|ctx| {
            model.update(ctx, |model, ctx| model.sync(&[], ctx));
        });
        for handle in &handles {
            assert!(
                comes_to_be_reaped(&handle.0),
                "a closed pane's shell was never ended"
            );
        }
    }

    // As the tests that drive one shell start it, here ended by `exit`.
    let flusher = repainting();
    let Some(shared) = started_with(
        Program::command(bash, ["--norc", "--noprofile", "-i"]),
        &given,
        &flusher,
    ) else {
        eprintln!("skipped: no shell could be started here");
        return;
    };
    assert!(
        TerminalHandle(shared.clone()).write(": typed\nexit\n"),
        "the shell could not be typed into"
    );
    assert!(comes_to_be_reaped(&shared), "the shell never exited");
    flusher.stop();

    assert!(
        std::fs::read(&history.0).ok().as_deref() == Some(kept.as_bytes()),
        "a shell started here cut or added to the history file it was handed"
    );
    assert_eq!(
        modified().ok(),
        Some(written),
        "a shell started here rewrote the history file it was handed"
    );
}

/// What one spawn is made to cost in the tests of shells opened on the pool:
/// long enough that the test can act while one is being spawned.
const POOL_SPAWN: Duration = Duration::from_millis(300);

/// A model whose panes `behind` are queued for the pool, with every spawn
/// costing [`POOL_SPAWN`], and none of them opened yet: the task for the
/// first is on its way and nothing has been taken in. `None` where no shell
/// can be started, and on Windows, as for [`Shells`].
fn opening_behind(behind: &[PaneId]) -> Option<(Arc<LocalQueue>, App, ModelHandle<TerminalModel>)> {
    if cfg!(windows) {
        return None;
    }
    let (queue, mut app) = app();
    let model = app.update(|ctx| ctx.add_model(TerminalModel::new));
    let behind: Vec<_> = behind.iter().map(|pane| (*pane, None)).collect();
    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.set_shell(Some(PathBuf::from("/bin/sh")));
            model.set_environment(without_history(&[]));
            model.set_shell_marks(false);
            model.set_shell_login(false);
            model.set_spawn_cost(POOL_SPAWN);
            model.expect();
            model.start(&[], &behind, ctx);
        });
    });
    Some((queue, app, model))
}

/// Pumps the queue until `settled`, or panics with `what` past
/// [`REPLY_TIMEOUT`].
fn pump_until(
    queue: &LocalQueue,
    app: &App,
    what: &str,
    mut settled: impl FnMut(&AppContext) -> bool,
) {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        queue.run_until_parked();
        if app.read(|ctx| settled(ctx)) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn shells_behind_the_shown_ones_open_off_the_thread_that_draws_and_all_arrive() {
    // A restore's tabs behind the active one: the thread that draws waited on
    // every fork in turn, and the frame after the restore waited for all of
    // them.
    let behind: Vec<_> = (0..3).map(|_| pane()).collect();
    let started = Instant::now();
    let Some((queue, app, model)) = opening_behind(&behind) else {
        return;
    };
    let took = started.elapsed();
    assert!(
        took < POOL_SPAWN,
        "queueing three shells held the thread that draws for {took:?}"
    );
    app.read(|ctx| {
        let model = model.as_ref(ctx);
        for pane in &behind {
            assert!(model.is_opening(*pane), "{pane:?} is not on its way");
            assert!(model.handle(*pane).is_none() && model.failure(*pane).is_none());
        }
    });

    pump_until(
        &queue,
        &app,
        "a shell opened on the pool never arrived",
        |ctx| {
            behind
                .iter()
                .all(|pane| model.as_ref(ctx).handle(*pane).is_some())
        },
    );
    app.read(|ctx| {
        assert!(
            !behind
                .iter()
                .any(|pane| model.as_ref(ctx).is_opening(*pane)),
            "a shell that arrived is still on its way"
        );
    });
}

#[test]
fn a_shown_pane_waiting_its_turn_opens_in_place_and_the_one_on_the_pool_is_left_to_arrive() {
    // A tab switched to before a restore got to it: the first is on the
    // pool, the second and third wait their turn, and the first and third
    // are shown.
    let behind: Vec<_> = (0..3).map(|_| pane()).collect();
    let Some((queue, mut app, model)) = opening_behind(&behind) else {
        return;
    };
    app.update(|ctx| {
        model.update(ctx, |model, ctx| {
            model.open_shown(&[behind[0], behind[2]], ctx);
        });
    });
    app.read(|ctx| {
        let model = model.as_ref(ctx);
        assert!(
            model.handle(behind[2]).is_some(),
            "a shown pane is still waiting its turn on the pool"
        );
        assert!(
            !model.is_opening(behind[2]),
            "a shown pane opened in place is still queued for a second shell"
        );
        assert!(
            model.handle(behind[0]).is_none() && model.is_opening(behind[0]),
            "the shell the pool was spawning was started a second time in place"
        );
        assert!(
            model.handle(behind[1]).is_none() && model.is_opening(behind[1]),
            "a pane nobody shows did not wait its turn"
        );
    });

    pump_until(
        &queue,
        &app,
        "a shell left on the pool never arrived",
        |ctx| {
            behind
                .iter()
                .all(|pane| model.as_ref(ctx).handle(*pane).is_some())
        },
    );
}

#[test]
fn a_pane_closed_while_its_shell_is_on_the_way_never_gets_one() {
    // The first is on the pool and the second waits its turn when both
    // close; the third, still open, is the one that arrives.
    let behind: Vec<_> = (0..3).map(|_| pane()).collect();
    let Some((queue, mut app, model)) = opening_behind(&behind) else {
        return;
    };
    app.update(|ctx| {
        model.update(ctx, |model, ctx| model.sync(&[(behind[2], None)], ctx));
    });
    app.read(|ctx| {
        let model = model.as_ref(ctx);
        assert!(
            !model.is_opening(behind[1]),
            "a pane that closed before its turn is still waiting for it"
        );
        assert!(model.is_opening(behind[2]));
    });

    pump_until(
        &queue,
        &app,
        "the pane still open never got its shell",
        |ctx| model.as_ref(ctx).handle(behind[2]).is_some(),
    );
    app.read(|ctx| {
        let model = model.as_ref(ctx);
        for pane in &behind[..2] {
            assert!(
                model.handle(*pane).is_none(),
                "a pane closed while its shell was on the way got one"
            );
            assert!(model.failure(*pane).is_none());
            assert!(!model.is_opening(*pane));
        }
    });
}

#[test]
fn a_window_that_goes_while_a_shell_is_being_spawned_waits_for_that_one_too() {
    // The task that spawns it is the only thing that can end it, and the
    // process may be gone by the time it would have come home: what the
    // window waits for before the process ends has to count it.
    let pane = pane();
    let started = Instant::now();
    let Some((queue, mut app, model)) = opening_behind(&[pane]) else {
        return;
    };
    // Begun, rather than waiting for a worker: a task that has not begun
    // spawns nothing once its pane has gone, and there is nothing to wait for.
    let begun = |app: &App| {
        app.read(|ctx| {
            model
                .as_ref(ctx)
                .opening
                .as_ref()
                .is_some_and(|(_, opening)| !matches!(*opening.lock(), OpeningState::Queued))
        })
    };
    let deadline = Instant::now() + REPLY_TIMEOUT;
    while !begun(&app) {
        assert!(Instant::now() < deadline, "the pool never began the spawn");
        thread::sleep(Duration::from_millis(1));
    }

    let endings = app.update(|ctx| model.update(ctx, |model, _| model.close_all()));
    assert!(
        endings.wait(REPLY_TIMEOUT),
        "a window that is going gave up waiting for its shells"
    );
    // The spawn began after `started` and takes `POOL_SPAWN` at the least.
    let waited = started.elapsed();
    assert!(
        waited >= POOL_SPAWN,
        "the wait came back {waited:?} in, before the shell being spawned could have been"
    );

    queue.run_until_parked();
    app.read(|ctx| {
        assert!(
            model.as_ref(ctx).handle(pane).is_none(),
            "a window that is going took in a shell"
        );
    });
}

/// A shell that ignores the hang-up a close sends first, started the way a
/// pool task starts one and past its `trap` — or `None` where none can be
/// started. Ending it takes the `SIGKILL` the escalation sends, which only an
/// ending gets to.
fn deaf_shell(flusher: &Arc<Flusher>) -> Option<Opened> {
    let pane = pane();
    // `exec`, so the process the pty started is the one that ignores it: the
    // disposition outlives the exec.
    let script = "trap '' HUP; printf 'crook-%s\\n' ready; exec sleep 600";
    let shared = started(sh(script, &[])?, flusher)?;
    assert!(
        comes_to_show(&shared, "crook-ready"),
        "a shell never got ready"
    );
    Some(Opened {
        shared,
        integration: shell_integration::Session::open(
            pane,
            &shell_integration::Options {
                enabled: false,
                ..Default::default()
            },
        ),
        marks: false,
        token: None,
        shell: PathBuf::from("/bin/sh"),
        palette: Palette::default(),
    })
}

/// Whether `shared`'s child is ended, by a signal, and reaped.
fn ended_by_a_signal(shared: &Shared) -> bool {
    comes_to_be_reaped(shared)
        && shared
            .lock()
            .try_wait()
            .ok()
            .flatten()
            .is_some_and(|exit| exit.signal.is_some())
}

#[test]
fn a_shell_whose_pane_went_while_it_was_spawned_is_ended_by_the_task_that_spawned_it() {
    let flusher = repainting();
    let Some(opened) = deaf_shell(&flusher) else {
        eprintln!("skipped: no shell could be started here");
        flusher.stop();
        return;
    };
    let shared = opened.shared.clone();
    let (pane, endings, opening) = (pane(), Endings::default(), Opening::default());

    opening.fill(
        pane,
        || {
            opening.abandon(pane, &endings);
            Ok(opened)
        },
        &endings,
    );

    assert!(
        opening.take().is_none(),
        "a shell nobody wants was left for the model to take in"
    );
    assert!(endings.wait(REPLY_TIMEOUT), "the shell's ending never came");
    assert!(
        ended_by_a_signal(&shared),
        "a shell spawned for a pane that had gone was left running"
    );
    flusher.stop();
}

#[test]
fn a_shell_spawned_before_its_pane_went_is_ended_by_the_close() {
    let flusher = repainting();
    let Some(opened) = deaf_shell(&flusher) else {
        eprintln!("skipped: no shell could be started here");
        flusher.stop();
        return;
    };
    let shared = opened.shared.clone();
    let (pane, endings, opening) = (pane(), Endings::default(), Opening::default());

    opening.fill(pane, || Ok(opened), &endings);
    opening.abandon(pane, &endings);

    assert!(
        opening.take().is_none(),
        "a shell nobody wants was left for the model to take in"
    );
    assert!(endings.wait(REPLY_TIMEOUT), "the shell's ending never came");
    assert!(
        ended_by_a_signal(&shared),
        "a shell whose pane went before it was taken in was left running"
    );
    flusher.stop();
}

#[test]
fn a_shell_whose_pane_went_before_the_pool_got_to_it_is_never_spawned() {
    let (pane, endings, opening) = (pane(), Endings::default(), Opening::default());
    let mut spawned = false;

    opening.abandon(pane, &endings);
    opening.fill(
        pane,
        || {
            spawned = true;
            Err("spawned".to_owned())
        },
        &endings,
    );

    assert!(!spawned, "a shell was spawned for a pane that had gone");
    assert!(
        endings.wait(Duration::ZERO),
        "a window would wait for a shell that was never spawned"
    );
}
