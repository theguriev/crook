//! The shells behind the panes, read off the UI thread.
//!
//! [`crook_terminal`] owns no thread and no timer on purpose: `Terminal` is a
//! value with methods, and someone else has to decide which thread is allowed to
//! block on a pty that may say nothing for hours. This is that someone, in the
//! shape [`crate::usage_model`] and [`crate::git_model`] established — work off
//! the UI thread, delivered on it, `ctx.notify` when something a viewer could
//! see actually changed.
//!
//! # Why a thread and not the background pool
//!
//! The chains that wait on a timer each hold one pool worker while they do, and
//! [`crate::PARKED_WORKERS`] is sized for exactly those. A pty read blocks for
//! as long as the shell is quiet, so a pane on the pool would park a worker for
//! the life of the session: four panes would take the whole pool and the
//! settings save a click was waiting on would never run. Each terminal gets an
//! OS thread of its own instead, and reaches the main thread through [`Wake`] —
//! a future the reader wakes, which is the same mechanism a completed pool task
//! already uses to come home.
//!
//! # The throttle
//!
//! **The ceiling is one repaint per pane per [`PAINT_INTERVAL`]**, so about 62 a
//! second — and it is a ceiling on *drawing*, never on reading. Those are two
//! different things and conflating them costs three orders of magnitude: a pty
//! master hands out at most about a kilobyte per `read` whatever buffer it is
//! given, so a reader that paused for a frame after every read would move 64 KB
//! a second and the *program* on the far end would run at that speed, blocked on
//! its own writes.
//!
//! So the reader never pauses. It reads and parses as fast as the child can
//! produce, and only *publishing* — rebuilding the snapshot and waking the main
//! thread — is rate-limited: a batch parsed less than [`PAINT_INTERVAL`] after
//! the last published one is left parsed and undrawn, and [`Flusher`] draws it
//! when the interval is up. That thread exists for exactly one case, and it is
//! not a nicety: the last batch of a burst is always the one that misses the
//! window, and without someone to come back for it the final screenful of a
//! `cat` would sit invisible until the shell next said something.
//!
//! There are two more filters behind that one. The reader publishes a snapshot
//! the emulator only rebuilds when the drawn content differs, so an escape
//! sequence that changed nothing visible hands back the same `Arc`; and
//! [`TerminalModel::absorb`] compares that `Arc` by pointer before it notifies.
//! A frame is only ever spent on a grid that actually changed.
//!
//! # The lock
//!
//! One mutex per terminal, and **it is never held across a frame**. The reader
//! takes it to feed bytes, and again to build a snapshot, and publishes the
//! `Arc` into a slot of its own; painting clones that `Arc` and walks owned
//! data. Layout takes the lock only when the computed grid actually changed,
//! which it establishes first with an atomic — so a window being dragged does
//! not contend with a shell that is printing.
//!
//! # What a closed pane costs
//!
//! Nothing, in every case a pane can be closed in — with one residue. Closing
//! kills the child, which closes the pty, which returns the reader's blocking
//! read; and the reader holds only a [`Weak`] on the session, so the terminal,
//! its ten thousand lines of scrollback and the pty go the moment the model
//! drops its own reference, whether or not the reader has noticed yet.
//!
//! The residue is a pane closed while something *other than the child* is
//! holding the pty open — a `sleep 60 &`, a server that outlived the shell that
//! started it. Nothing can make that read return: the slave is open in another
//! process and the reader's own descriptor is a duplicate of the master, so the
//! thread parks until the orphan exits, holding a descriptor and nothing else.
//! For the same reason a shell that exits *behind* such a process leaves its
//! pane open, because end-of-file on the master is the only signal this design
//! has that a session is over.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context as TaskContext, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use crook_terminal::{
    Key, Modifiers, Palette, Rgb, Snapshot, Terminal, TerminalEvent, TerminalOptions, TerminalSize,
};
use crookui_core::geometry::Color;
use crookui_core::prelude::*;

use crate::tab::PaneId;
use crate::theme::THEME;

/// The longest a pane goes between repaints while its shell is talking.
///
/// Sixteen milliseconds: one display refresh, which is the fastest a repaint
/// can be worth anything. See the module docs for how it is enforced.
const PAINT_INTERVAL: Duration = Duration::from_millis(16);

/// How much of a pty is taken in one read.
///
/// An upper bound, not a target: a pty master returns about a kilobyte per read
/// on macOS however large the buffer is. What this size buys is that the
/// platforms with no such limit take a burst in fewer syscalls, and it is small
/// enough that parsing one chunk holds the emulator's lock for well under a
/// frame.
const READ_CHUNK: usize = 16 * 1024;

/// How long the reader waits for a child to be reapable after the pty closed.
///
/// End-of-file on the master means every slave descriptor is shut, which in
/// practice means the child has exited — but "has exited" and "has been reaped"
/// are a scheduling decision apart, and the exit status is worth a short wait.
const REAP_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the reader sleeps between attempts to reap.
const REAP_INTERVAL: Duration = Duration::from_millis(5);

/// The grid a terminal starts at, before any pane has been laid out.
///
/// The size every terminal has ever defaulted to. The first layout replaces it
/// with what the pane actually measured, usually before the shell has printed
/// its prompt.
const INITIAL_GRID: TerminalSize = TerminalSize::new(80, 24);

/// Something one pane's shell did that the rest of the application cares about.
///
/// Everything else a terminal reports — a bell, a clipboard write, a repaint —
/// is either handled here or is not the workspace's business. These three are:
/// two of them rename or relocate a session, and the third closes a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalUpdate {
    /// The shell set a window title, or reset it. This is what makes a tab of
    /// shells rename itself with no rename plumbing at all.
    Title(PaneId, Option<String>),
    /// The shell reported where it is, with OSC 7. Every git fact a row shows
    /// is looked up by this, so it is also what makes the branch chip follow a
    /// `cd`.
    WorkingDirectory(PaneId, PathBuf),
    /// The shell is gone. The pane should close, and with it its tab and the
    /// window if they were the last ones.
    Closed(PaneId),
}

/// The terminals behind the open panes.
pub struct TerminalModel {
    sessions: HashMap<PaneId, Session>,

    /// Why a pane has no terminal, when the reason was worth telling somebody.
    ///
    /// A pane whose shell could not be started still has to draw something, and
    /// "no shell" is a much worse answer than the reason it failed.
    failures: HashMap<PaneId, String>,

    /// Whether panes may open terminals at all.
    ///
    /// Off until [`Self::start`], for the reason the poll chains are: a headless
    /// snapshot and a test render the real view tree without spawning a process.
    live: bool,

    /// The colours every terminal resolves its cells against.
    palette: Palette,

    /// The one thread that comes back for batches parsed too soon to draw.
    ///
    /// Shared by every pane and started with the first of them, so a model that
    /// never opens a terminal — the headless snapshot, a test — never starts a
    /// thread either.
    flusher: Arc<Flusher>,
    flushing: bool,
}

/// One running shell, as the model holds it.
struct Session {
    shared: Arc<Shared>,
    /// The last snapshot delivered to the main thread. Compared by pointer to
    /// decide whether a frame is worth spending.
    snapshot: Arc<Snapshot>,
    /// What the shell last called itself, so a title set twice notifies once.
    title: Option<String>,
    /// Where the shell last said it was, for the same reason.
    directory: Option<PathBuf>,
}

impl Entity for TerminalModel {
    type Event = TerminalUpdate;
}

impl TerminalModel {
    /// A model with no terminals, which is not allowed to open any yet.
    pub fn new(_: &mut ModelContext<Self>) -> Self {
        Self {
            sessions: HashMap::new(),
            failures: HashMap::new(),
            live: false,
            palette: crook_palette(),
            flusher: Arc::new(Flusher::default()),
            flushing: false,
        }
    }

    /// Lets panes open shells, and opens one for every pane already there.
    ///
    /// Call once, after the window exists. Separate from [`Self::new`] for the
    /// reason the poll chains' `start` is: nothing renders a frame in CI with
    /// four shells running underneath it.
    pub fn start(&mut self, panes: &[(PaneId, Option<PathBuf>)], ctx: &mut ModelContext<Self>) {
        if self.live {
            return;
        }
        self.live = true;
        self.sync(panes, ctx);
    }

    /// Whether shells are being opened at all.
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// Opens a shell for every pane that has none, and closes the ones whose
    /// panes have gone.
    ///
    /// Cheap and idempotent, so the workspace can call it after every change to
    /// the strip — which is what makes "a pane exists" and "a shell is running
    /// in it" one statement rather than two that can disagree.
    pub fn sync(&mut self, panes: &[(PaneId, Option<PathBuf>)], ctx: &mut ModelContext<Self>) {
        let closed: Vec<PaneId> = self
            .sessions
            .keys()
            .copied()
            .filter(|pane| !panes.iter().any(|(open, _)| open == pane))
            .collect();
        for pane in closed {
            self.close(pane);
        }
        self.failures
            .retain(|pane, _| panes.iter().any(|(open, _)| open == pane));

        if !self.live {
            return;
        }
        for (pane, directory) in panes {
            if !self.sessions.contains_key(pane) {
                self.open(*pane, directory.clone(), ctx);
            }
        }
    }

    /// The running terminal in a pane, for the element that draws it.
    pub fn handle(&self, pane: PaneId) -> Option<TerminalHandle> {
        self.sessions
            .get(&pane)
            .map(|session| TerminalHandle(session.shared.clone()))
    }

    /// Why this pane has no terminal, if the attempt failed rather than never
    /// having been made.
    pub fn failure(&self, pane: PaneId) -> Option<&str> {
        self.failures.get(&pane).map(String::as_str)
    }

    /// What the pane is showing right now.
    ///
    /// For a caller that wants the text rather than the pixels — the
    /// command-line runs that prove a shell printed what it was asked to.
    pub fn snapshot(&self, pane: PaneId) -> Option<Arc<Snapshot>> {
        self.sessions
            .get(&pane)
            .map(|session| session.shared.snapshot())
    }

    /// Types `text` into a pane's shell, as if a person had.
    ///
    /// The pty buffers it, so this works before the shell has finished starting:
    /// what is written now is read when it gets there.
    pub fn type_into(&self, pane: PaneId, text: &str) {
        let Some(session) = self.sessions.get(&pane) else {
            log::warn!("nothing to type into: pane {pane:?} has no terminal");
            return;
        };
        if let Err(error) = session.shared.lock().write(text.as_bytes()) {
            log::warn!("could not write to the shell in pane {pane:?}: {error}");
        }
    }

    /// Starts a shell for one pane and the thread that reads it.
    fn open(&mut self, pane: PaneId, directory: Option<PathBuf>, ctx: &mut ModelContext<Self>) {
        let options = TerminalOptions {
            size: INITIAL_GRID,
            working_directory: directory,
            palette: self.palette.clone(),
            ..Default::default()
        };

        let mut terminal = match Terminal::spawn(options) {
            Ok(terminal) => terminal,
            Err(error) => {
                let reason = format!("{error:#}");
                log::error!("could not open a shell for pane {pane:?}: {reason}");
                self.failures.insert(pane, reason);
                ctx.notify();
                return;
            }
        };

        // Taken before the terminal is shared, because it can only be taken
        // once and the thread below is the one owner that may block on it.
        let Some(reader) = terminal.take_reader() else {
            log::error!("the shell for pane {pane:?} has no readable end");
            self.failures
                .insert(pane, "the pseudo-terminal has no readable end".to_owned());
            ctx.notify();
            return;
        };

        let snapshot = terminal.snapshot();
        let shared = Arc::new(Shared {
            terminal: Mutex::new(terminal),
            latest: Mutex::new(snapshot.clone()),
            events: Mutex::new(Vec::new()),
            wake: Arc::new(Wake::default()),
            grid: AtomicU32::new(packed(INITIAL_GRID)),
            resize_failing: AtomicBool::new(false),
            publish: Mutex::new(PublishState::new()),
        });

        if let Err(error) = self.start_flushing() {
            log::error!("could not start the repaint thread for pane {pane:?}: {error}");
            self.failures.insert(pane, format!("{error}"));
            ctx.notify();
            return;
        }

        // The reader holds a `Weak`, not an `Arc`: a pane whose pty stays open
        // after it is closed — something other than the child is holding it —
        // must not keep the terminal and its scrollback alive behind it. The
        // thread owns the object whose drop would unblock it, and that is a
        // knot that only a weak reference unties.
        let reading = Arc::downgrade(&shared);
        let flusher = self.flusher.clone();
        let started = thread::Builder::new()
            .name(format!("crook-pty-{pane:?}"))
            .spawn(move || read_loop(&reading, &flusher, reader));
        if let Err(error) = started {
            log::error!("could not start a reader thread for pane {pane:?}: {error}");
            self.failures.insert(pane, format!("{error}"));
            ctx.notify();
            return;
        }

        self.failures.remove(&pane);
        self.sessions.insert(
            pane,
            Session {
                shared,
                snapshot,
                title: None,
                directory: None,
            },
        );
        self.watch(pane, ctx);
        ctx.notify();
    }

    /// Starts the shared repaint thread, once.
    fn start_flushing(&mut self) -> io::Result<()> {
        if self.flushing {
            return Ok(());
        }
        let flusher = self.flusher.clone();
        thread::Builder::new()
            .name("crook-pty-repaint".to_owned())
            .spawn(move || flush_loop(&flusher))?;
        self.flushing = true;
        Ok(())
    }

    /// Ends a pane's shell and lets its reader thread finish.
    ///
    /// Killing the child is what closes the pty, which is what returns the
    /// reader's blocking read. This runs on the thread that draws, so it kills
    /// and does not wait: the exit status is a line in a log, and a child that
    /// takes its time dying would otherwise hold the window. Nothing is leaked
    /// by not waiting — the kill escalates to a signal nothing survives, and
    /// the reader reports the status if it is still there to see it. The one
    /// cost is the escalation's own quarter-second grace period, and only for a
    /// child that ignores the first signal.
    fn close(&mut self, pane: PaneId) {
        let Some(session) = self.sessions.remove(&pane) else {
            return;
        };
        if let Err(error) = session.shared.lock().kill() {
            log::debug!("the shell in pane {pane:?} could not be ended: {error:#}");
        }
        session.shared.wake.finish();
    }

    /// Waits for one pane's reader to post, then absorbs what it posted.
    ///
    /// One outstanding wait per pane, and the only thing that starts the next
    /// one is the previous one finishing — the ownership trick the poll chains
    /// use, applied per session. A pane that has been closed is not re-watched,
    /// which is what ends the chain.
    fn watch(&mut self, pane: PaneId, ctx: &mut ModelContext<Self>) {
        let Some(session) = self.sessions.get(&pane) else {
            return;
        };
        let shared = session.shared.clone();
        ctx.spawn(
            async move { shared.wake.woken().await },
            move |model, (), ctx| model.absorb(pane, ctx),
        )
        .detach();
    }

    /// Takes what the reader posted, repaints if it changed anything, and waits
    /// again.
    fn absorb(&mut self, pane: PaneId, ctx: &mut ModelContext<Self>) {
        let Some(session) = self.sessions.get_mut(&pane) else {
            // The pane closed while the wait was outstanding. The chain ends
            // here, which is the whole of how a closed pane stops costing
            // anything.
            return;
        };

        let snapshot = session.shared.snapshot();
        let events = session.shared.take_events();
        let finished = session.shared.is_finished();

        let changed = !Arc::ptr_eq(&session.snapshot, &snapshot);
        session.snapshot = snapshot;

        // Collected before anything is emitted: emitting borrows the context,
        // and the session borrow has to be over by then.
        let mut updates = Vec::new();
        for event in events {
            match event {
                TerminalEvent::Title(title) => {
                    let title = title.filter(|title| !title.trim().is_empty());
                    if session.title != title {
                        session.title = title.clone();
                        updates.push(TerminalUpdate::Title(pane, title));
                    }
                }
                TerminalEvent::WorkingDirectory(directory) => {
                    if session.directory.as_ref() != Some(&directory) {
                        session.directory = Some(directory.clone());
                        updates.push(TerminalUpdate::WorkingDirectory(pane, directory));
                    }
                }
                TerminalEvent::ChildExited(exit) => {
                    log::debug!("the shell in pane {pane:?} {exit}");
                }
                // The child asked the terminal to close. It is about to stop
                // being readable anyway, so this is only ever early notice.
                TerminalEvent::Exit => log::debug!("the shell in pane {pane:?} asked to close"),
                TerminalEvent::Bell => log::trace!("bell in pane {pane:?}"),
                // Nothing in Crook can reach a system clipboard yet, and
                // silently dropping an OSC 52 is better than pretending.
                TerminalEvent::ClipboardStore(_) => {
                    log::debug!("pane {pane:?} asked to write the clipboard, which Crook cannot");
                }
                // The enum is `#[non_exhaustive]`. A shell asking for something
                // a later version of the emulator learned to report is not an
                // error here; it is a line in the log and a feature to add.
                other => log::debug!("pane {pane:?} reported {other:?}, which nothing acts on"),
            }
        }

        if changed {
            ctx.notify();
        }
        for update in updates {
            ctx.emit(update);
        }

        if finished {
            // The pane goes through the workspace, which is the one path that
            // knows a tab's last pane takes the tab, and the last tab the
            // window. Nothing here removes the session: the close that follows
            // comes back round through `sync`.
            ctx.emit(TerminalUpdate::Closed(pane));
            return;
        }
        self.watch(pane, ctx);
    }
}

impl Drop for TerminalModel {
    /// A window that closes takes its shells with it.
    ///
    /// Dropping the sessions alone would not: the reader threads hold the other
    /// half of every terminal, so nothing would be killed until the process
    /// itself went away — which is usually the next instant, and is not the same
    /// promise.
    fn drop(&mut self) {
        for pane in self.sessions.keys().copied().collect::<Vec<_>>() {
            self.close(pane);
        }
        self.flusher.stop();
    }
}

/// A running terminal, as the element that draws it holds it.
///
/// Cheap to clone. Every method takes the terminal's lock for as long as it
/// takes to do one thing and no longer; nothing here is allowed to hold it
/// across a frame.
#[derive(Clone)]
pub struct TerminalHandle(Arc<Shared>);

impl TerminalHandle {
    /// What the pane should draw. Owned data: no lock is held while it is
    /// painted, and the same `Arc` comes back until the content changes.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.0.snapshot()
    }

    /// Sets the grid and tells the child about it, reporting whether anything
    /// moved.
    ///
    /// Does nothing when the size has not changed, and establishes that without
    /// taking the lock: layout asks on every single frame, and a resize is a
    /// syscall, a `SIGWINCH`, and a full-screen program redrawing itself.
    ///
    /// The new size is recorded only once it has actually reached the pty. A
    /// resize that fails leaves the emulator at the new geometry and the child
    /// at the old one, and recording it anyway would make every later frame
    /// answer "unchanged" and never try again — a single failed `ioctl` while a
    /// window edge is being dragged would leave the shell's `$COLUMNS` wrong for
    /// the life of the pane.
    pub fn resize(&self, size: TerminalSize) -> bool {
        let shared = &self.0;
        if !needs_resize(&shared.grid, size) {
            return false;
        }

        let applied = self.drive(|terminal| terminal.resize(size));
        match applied {
            Ok(()) => {
                record_resize(&shared.grid, size);
                shared.resize_failing.store(false, Ordering::Relaxed);
            }
            // Complained about once: the retry is every frame, and so would the
            // log line be.
            Err(error) if !shared.resize_failing.swap(true, Ordering::Relaxed) => {
                log::warn!("could not resize a terminal to {size:?}: {error:#}");
            }
            Err(_) => {}
        }

        // The emulator moved either way — `Terminal::resize` reflows the grid
        // before it touches the pty — so the caller's snapshot is stale
        // whatever the pty made of it.
        true
    }

    /// Sends a key press to the shell, returning whether it produced any bytes.
    ///
    /// Typing also returns the viewport to the live output, which is what every
    /// terminal does and what stops a keystroke from disappearing above the
    /// scrollback somebody was reading.
    pub fn send_key(&self, key: Key, modifiers: Modifiers) -> bool {
        self.drive(|terminal| {
            terminal.scroll_to_bottom();
            match terminal.send_key(key, modifiers) {
                Ok(sent) => sent,
                Err(error) => {
                    log::debug!("could not send a key to a shell: {error}");
                    false
                }
            }
        })
    }

    /// Writes text to the shell as if it had been typed, returning whether it
    /// reached the pty.
    ///
    /// What the input field sends a finished command line with. The pty buffers
    /// it, so a line composed before the shell has finished starting is read
    /// when the shell gets there.
    pub fn write(&self, text: &str) -> bool {
        self.drive(|terminal| {
            terminal.scroll_to_bottom();
            match terminal.write(text.as_bytes()) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("could not send a line to a shell: {error}");
                    false
                }
            }
        })
    }

    /// Moves the viewport through the scrollback: positive is back into
    /// history.
    pub fn scroll_lines(&self, delta: i32) {
        if delta == 0 {
            return;
        }
        self.drive(|terminal| terminal.scroll_lines(delta));
    }

    /// Runs `work` against the terminal and republishes what it draws.
    ///
    /// Everything on this side of the handle moves the viewport — a resize
    /// reflows it, typing returns it to the bottom, the wheel leaves it — and
    /// the reader is what usually republishes. It may be hours away, so the
    /// caller does it instead and the frame it is about to paint is right.
    ///
    /// Deliberately outside the repaint ceiling: what reaches here is a person
    /// typing or dragging, which arrives at the rate a person produces it, and
    /// holding a keystroke back for a frame is the one delay a terminal must
    /// never have.
    fn drive<T>(&self, work: impl FnOnce(&mut Terminal) -> T) -> T {
        let mut terminal = self.0.lock();
        let outcome = work(&mut terminal);
        let snapshot = terminal.snapshot();
        drop(terminal);

        *self.0.latest.lock().unwrap_or_else(PoisonError::into_inner) = snapshot;
        outcome
    }
}

/// Everything a reader thread and the main thread share for one terminal.
struct Shared {
    terminal: Mutex<Terminal>,

    /// The most recent snapshot the reader built, so painting never waits on
    /// parsing.
    latest: Mutex<Arc<Snapshot>>,

    /// What the child has asked for and nobody has looked at yet.
    events: Mutex<Vec<TerminalEvent>>,

    /// The main thread's end of the wake.
    wake: Arc<Wake>,

    /// The grid the pty was last set to, packed as columns then rows.
    ///
    /// Here rather than read back from the terminal so that layout — which asks
    /// on every single frame — can answer "unchanged" without queueing behind a
    /// shell that is mid-burst. Only a resize that actually reached the pty is
    /// recorded, so one that failed is tried again on the next frame.
    grid: AtomicU32,

    /// Whether the last resize failed, so a pty that refuses every one of them
    /// is complained about once rather than sixty times a second.
    resize_failing: AtomicBool,

    /// The throttle: what has been parsed, and when it was last drawn.
    publish: Mutex<PublishState>,
}

/// What the reader has parsed and what the main thread has been shown.
struct PublishState {
    /// Bytes have been parsed that no published snapshot reflects.
    pending: bool,
    /// [`Flusher`] is holding a reference to this session and will come back
    /// for it, so nobody needs to hand it a second one.
    deferred: bool,
    /// When the grid was last published. The whole of the repaint ceiling.
    last: Instant,
    /// How many times the grid has been published, which is the number a test
    /// has to look at to see the ceiling holding.
    count: u64,
}

impl PublishState {
    fn new() -> Self {
        Self {
            pending: false,
            deferred: false,
            // An interval ago, so the first batch a shell prints is drawn the
            // moment it arrives rather than a frame later.
            last: Instant::now() - PAINT_INTERVAL,
            count: 0,
        }
    }
}

/// What came of asking a session to draw what it has parsed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Publish {
    /// Nothing has been parsed since the last time it was drawn.
    Nothing,
    /// Drawn, and the main thread woken.
    Done,
    /// Parsed, but too soon after the last frame to spend another one. Whoever
    /// got this answer has to make sure somebody comes back.
    Deferred,
    /// The same, except that somebody already is.
    AlreadyDeferred,
}

/// The one thread that draws what a reader parsed too soon to draw itself.
///
/// A reader that has just published cannot publish again for an interval, and
/// the batch it parses next may well be the last one the child ever sends. So
/// it hands the session here, and this comes back for it an interval later.
/// One thread for every pane, and it parks on a condition variable whenever
/// nothing is waiting, so an idle window does not tick.
#[derive(Default)]
struct Flusher {
    state: Mutex<FlusherState>,
    signal: Condvar,
}

#[derive(Default)]
struct FlusherState {
    waiting: Vec<Weak<Shared>>,
    stopped: bool,
}

impl Flusher {
    fn lock(&self) -> MutexGuard<'_, FlusherState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks for a session to be drawn once the interval is up.
    fn defer(&self, session: Weak<Shared>) {
        let mut state = self.lock();
        if state.stopped {
            return;
        }
        state.waiting.push(session);
        drop(state);
        self.signal.notify_one();
    }

    /// Stops the thread, which a window that is closing does.
    fn stop(&self) {
        self.lock().stopped = true;
        self.signal.notify_all();
    }
}

/// A reader thread's end of the main thread's attention.
///
/// One bit that a batch is waiting, one that the reader has stopped for good,
/// and the waker of whoever is parked on either. Owned separately from
/// [`Shared`] so it can be exercised without a pty.
#[derive(Default)]
struct Wake {
    state: Mutex<WakeState>,
}

#[derive(Default)]
struct WakeState {
    /// Something has been posted and not yet taken.
    raised: bool,
    /// The reader has stopped for good.
    finished: bool,
    /// Whoever is waiting, if anyone is.
    waker: Option<Waker>,
}

impl Wake {
    fn lock(&self) -> MutexGuard<'_, WakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Posts that the reader has parsed something, and wakes the main thread.
    fn raise(&self) {
        let mut state = self.lock();
        state.raised = true;
        let waker = state.waker.take();
        drop(state);
        // Woken from the reader thread; the executor's scheduler is what
        // carries the task back to the main one.
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Says the reader has stopped, and wakes whoever is waiting so they find
    /// out.
    fn finish(&self) {
        let mut state = self.lock();
        state.finished = true;
        let waker = state.waker.take();
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn is_finished(&self) -> bool {
        self.lock().finished
    }

    /// Resolves once the reader has posted something, or has stopped.
    fn woken(self: &Arc<Self>) -> Woken {
        Woken { wake: self.clone() }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Terminal> {
        self.terminal.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn publish_state(&self) -> MutexGuard<'_, PublishState> {
        self.publish.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn snapshot(&self) -> Arc<Snapshot> {
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn take_events(&self) -> Vec<TerminalEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn is_finished(&self) -> bool {
        self.wake.is_finished()
    }

    /// Parses output into the grid, and does not draw it.
    ///
    /// The lock covers the parse and nothing else; the snapshot it will
    /// eventually be drawn from is built under a second, later acquisition, so
    /// a burst never holds the terminal for longer than one chunk takes to
    /// parse.
    fn feed(&self, bytes: &[u8]) {
        let mut terminal = self.lock();
        if let Err(error) = terminal.feed(bytes) {
            log::debug!("could not answer a shell's query: {error}");
        }
        let events = terminal.take_events();
        drop(terminal);

        self.collect(events);
        self.publish_state().pending = true;
    }

    /// Draws what has been parsed, if the last frame is far enough behind.
    fn publish_if_due(&self) -> Publish {
        let mut publish = self.publish_state();
        if !publish.pending {
            return Publish::Nothing;
        }
        if publish.last.elapsed() < PAINT_INTERVAL {
            return if std::mem::replace(&mut publish.deferred, true) {
                Publish::AlreadyDeferred
            } else {
                Publish::Deferred
            };
        }

        publish.pending = false;
        publish.deferred = false;
        publish.last = Instant::now();
        publish.count += 1;
        drop(publish);

        self.publish();
        Publish::Done
    }

    /// Says this session is no longer waiting on [`Flusher`], so the next
    /// batch that misses its window asks for it again.
    fn undefer(&self) {
        self.publish_state().deferred = false;
    }

    /// Rebuilds the grid, hands it to the main thread, and wakes it.
    ///
    /// The `Arc` goes into a slot of its own so that painting never waits on
    /// parsing; the emulator returns the very same one when the drawn content
    /// did not change, which is what makes the wake-up free to ignore.
    fn publish(&self) {
        let mut terminal = self.lock();
        let snapshot = terminal.snapshot();
        let events = terminal.take_events();
        drop(terminal);

        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = snapshot;
        self.collect(events);
        self.wake.raise();
    }

    fn collect(&self, events: Vec<TerminalEvent>) {
        if events.is_empty() {
            return;
        }
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(events);
    }
}

/// The wait a pane's foreground chain is parked on between batches.
struct Woken {
    wake: Arc<Wake>,
}

impl Future for Woken {
    type Output = ();

    fn poll(self: Pin<&mut Self>, ctx: &mut TaskContext<'_>) -> Poll<()> {
        let mut state = self.wake.lock();

        // Finished stays ready for good: a closed pane's chain has to be able
        // to see it once more even after the last batch was taken.
        if state.finished {
            return Poll::Ready(());
        }
        if state.raised {
            state.raised = false;
            return Poll::Ready(());
        }

        state.waker = Some(ctx.waker().clone());
        Poll::Pending
    }
}

/// Reads one pty until it closes, parsing everything it says and drawing what
/// the interval allows.
///
/// **There is no sleep here, and that is the point.** A pty hands out about a
/// kilobyte per read, so pausing for a frame after each one would cap the child
/// at 64 KB a second — not the repaint, the *program*, which blocks on its own
/// writes once the kernel's buffer fills. Reading is unthrottled and drawing is
/// not; see the module docs.
///
/// The session is held weakly, so a closed pane's terminal is freed even while
/// this thread is still parked in a read that will never return.
fn read_loop(shared: &Weak<Shared>, flusher: &Arc<Flusher>, mut reader: impl io::Read) {
    let mut buffer = vec![0; READ_CHUNK];

    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            // A pty whose child has gone reports end-of-file on some platforms
            // and an error on others. Both mean there is nothing left to read.
            Err(error) => {
                log::debug!("a pty stopped being readable: {error}");
                break;
            }
        };

        // The pane closed while this was blocked. Nothing is left to feed.
        let Some(session) = shared.upgrade() else {
            return;
        };
        session.feed(&buffer[..read]);
        if session.publish_if_due() == Publish::Deferred {
            flusher.defer(shared.clone());
        }
    }

    let Some(session) = shared.upgrade() else {
        return;
    };
    reap(&session);
    session.wake.finish();
}

/// Draws sessions whose readers parsed something too soon to draw it.
///
/// One round per batch of deferrals: wait for one, sleep the interval out, then
/// publish everything that has come due. A session whose reader published in
/// the meantime and then parsed more is handed back for the next round rather
/// than drawn early, so the ceiling holds however many panes are talking.
fn flush_loop(flusher: &Arc<Flusher>) {
    loop {
        let due = {
            let mut state = flusher.lock();
            while state.waiting.is_empty() && !state.stopped {
                state = flusher
                    .signal
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if state.stopped {
                return;
            }
            std::mem::take(&mut state.waiting)
        };

        thread::sleep(PAINT_INTERVAL);
        for session in due {
            let Some(live) = session.upgrade() else {
                continue;
            };
            live.undefer();
            if live.publish_if_due() == Publish::Deferred {
                flusher.defer(session);
            }
        }
    }
}

/// Collects the child's exit status once its pty has closed.
///
/// Bounded, because a child that has closed its pty and then refuses to be
/// reaped must not hold a thread — and the status is a nicety, while the pane
/// closing is not.
fn reap(shared: &Shared) {
    let deadline = Instant::now() + REAP_TIMEOUT;
    loop {
        let reaped = match shared.lock().try_wait() {
            Ok(exit) => exit.is_some(),
            Err(error) => {
                log::debug!("could not collect a shell's exit status: {error:#}");
                true
            }
        };

        if reaped || Instant::now() >= deadline {
            break;
        }
        thread::sleep(REAP_INTERVAL);
    }

    // One publish rather than one per attempt: the grid did not move while the
    // child was being waited for, and a wake-up per five milliseconds would be
    // a hundred frames nobody asked for. This one ignores the interval, because
    // there is no later batch to fold it into.
    shared.publish_state().pending = false;
    shared.publish();
}

/// Whether the pty still has to be told about this size.
///
/// A read rather than a swap, because the size is only recorded once it has
/// actually been sent — see [`TerminalHandle::resize`].
fn needs_resize(grid: &AtomicU32, size: TerminalSize) -> bool {
    grid.load(Ordering::Relaxed) != packed(size)
}

/// Records a size the pty has been set to.
fn record_resize(grid: &AtomicU32, size: TerminalSize) {
    grid.store(packed(size), Ordering::Relaxed);
}

/// A grid as one comparable integer, for the atomic layout asks on every frame.
///
/// The cell size is deliberately not in it: it changes only when the font does,
/// which cannot happen while the process runs.
fn packed(size: TerminalSize) -> u32 {
    u32::from(size.columns) << 16 | u32::from(size.rows)
}

/// The terminal palette, in Crook's colours.
///
/// The two that matter are the defaults: a grid whose background is the panel's
/// own means an untouched screen costs no rectangles at all, and text that
/// matches the rest of the window means a shell does not look pasted into it.
/// Everything a program actually asks for — the sixteen ANSI colours, the cube,
/// a `Color::Spec` — keeps the values every terminal agrees on.
fn crook_palette() -> Palette {
    let foreground = rgb(THEME.text_primary);
    Palette {
        foreground,
        background: rgb(THEME.surface),
        cursor: rgb(THEME.accent),
        bright_foreground: foreground,
        // What dim text is drawn in. Derived from this palette's own
        // foreground: left at the default it would be held back from a grey
        // Crook does not use.
        dim_foreground: rgb(THEME.text_muted),
        ..Palette::default()
    }
}

/// A theme colour as the emulator spells one. Alpha is dropped: a terminal cell
/// is opaque.
fn rgb(color: Color) -> Rgb {
    Rgb::new(color.r, color.g, color.b)
}

#[cfg(test)]
#[path = "terminal_model_tests.rs"]
mod tests;
