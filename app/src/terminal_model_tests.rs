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
    assert_eq!(
        palette.ansi[1],
        Palette::default().ansi[1],
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
            model.start(&[(pane, None)], ctx)
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
        blocks: Mutex::new(Arc::default()),
        unpublished: Mutex::new(Vec::new()),
        events: Mutex::new(Vec::new()),
        wake: Arc::new(Wake::default()),
        grid: AtomicU64::new(packed(INITIAL_GRID)),
        resize_failing: AtomicBool::new(false),
        scroll_remainder: AtomicU32::new(0.0f32.to_bits()),
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
}

impl Batches {
    /// `marks` is whether the pane's shell reports command marks, which
    /// decides what its first prompt is. `None` where no pty can be opened.
    fn new(marks: bool) -> Option<Self> {
        let shared = session()?;
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
                        snapshot: shared.snapshot(),
                        title: None,
                        directory: None,
                        running: None,
                        token: None,
                        marks,
                        prompted: false,
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
    let installed = Arc::strong_count(&*slot);
    let writer = thread::spawn(write);

    let deadline = Instant::now() + REPLY_TIMEOUT;
    while Arc::strong_count(&*slot) == installed {
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

/// A session started the way [`TerminalModel::open`] starts one — its reader
/// and its writer each on a thread, its deferred batches drawn by `flusher` —
/// running `program`, or `None` where it cannot be started.
fn started(program: Program, flusher: &Arc<Flusher>) -> Option<Arc<Shared>> {
    let terminal = Terminal::spawn(TerminalOptions {
        program,
        size: TerminalSize::new(80, 24),
        ..TerminalOptions::default()
    })
    .ok()?;
    start(terminal, PaneId::next(), flusher).ok()
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
    if cfg!(windows) {
        return;
    }
    let Some(bash) = ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
        .into_iter()
        .find(|bash| std::path::Path::new(bash).exists())
    else {
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
