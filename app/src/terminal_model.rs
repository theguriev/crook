//! The shells behind the panes, read off the UI thread.
//!
//! [`crook_terminal`] owns no thread and no timer on purpose: `Terminal` is a
//! value with methods, and someone else has to decide which thread is allowed to
//! block on a pty that may say nothing for hours. This is that someone, in the
//! shape [`crate::git_model`] established — work off the UI thread, delivered
//! on it, and a word to the window only when something it draws could have
//! changed.
//!
//! # Why a thread and not the background pool
//!
//! The chains that wait on a timer each hold one pool worker while they do, and
//! [`crate::PARKED_WORKERS`] is sized for exactly those. A pty read blocks for
//! as long as the shell is quiet, so a pane on the pool would park a worker for
//! the life of the session: four panes would take the whole pool and the
//! settings save a click was waiting on would never run. Each terminal gets an
//! OS thread of its own instead — two, with the writer below, which blocks on
//! a child that is not reading just as long — and reaches the main thread
//! through [`Wake`]: a future the reader wakes, which is the same mechanism a
//! completed pool task already uses to come home.
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
//! produce, and only *publishing* — rebuilding the snapshot, handing over what
//! the child asked for in it, and waking the main thread — is rate-limited: a
//! batch parsed less than [`PAINT_INTERVAL`] after the last published one is
//! left parsed and undrawn, and [`Flusher`] draws it when the interval is up.
//! That thread exists for exactly one case, and it is not a nicety: the last
//! batch of a burst is always the one that misses the window, and without
//! someone to come back for it the final screenful of a `cat` would sit
//! invisible until the shell next said something.
//!
//! There are three more filters behind that one. The reader publishes a
//! snapshot the emulator only rebuilds when the drawn content differs, so an
//! escape sequence that changed nothing visible hands back the same `Arc`; and
//! [`TerminalModel::absorb`] compares that `Arc` by pointer before it reports
//! a [`TerminalUpdate::Repainted`]. A frame is only ever spent on a grid that
//! actually changed — and only on one somebody can see, which is the third.
//!
//! # Only what is on screen
//!
//! A changed grid is reported rather than notified, and the workspace decides
//! whether it is worth a frame. The model cannot: which panes a frame draws is
//! the active tab's, less the one a zoom hides, less all of them while a
//! section of the sidebar has the body — the workspace's state, which it
//! reads at the moment the repaint arrives rather than having it pushed here
//! from every place that state can change. A pane that is not on screen goes
//! on being read, parsed and published exactly as before, and everything its
//! *row* shows still arrives as it happens — a title, a bell, an agent's
//! status, a command finishing — because those are the tab's facts and the
//! tab is on screen. Only its grid stops costing frames, and the frame that
//! puts it back on screen reads the latest snapshot, not the last one drawn.
//!
//! # The writer
//!
//! **Nothing writes to a pty on the UI thread, or under the terminal's lock.**
//! A write to a pty waits for the child to read, and a child that is printing
//! reads nothing until its output has been read — by the reader, which needs
//! the lock to parse it. A paste that held the lock while it wrote into a
//! `cat` that had stopped reading therefore waited on the reader, which waited
//! on the paste, and the window froze for good at about eighteen kilobytes:
//! zsh and fish at sixteen on a composer's Enter, and any program that asked
//! the terminal questions faster than it read the answers, the reader writing
//! those under the lock itself.
//!
//! So each session has a third thread, beside the reader and the shared
//! flusher, that owns the pty's writer — see [`crook_terminal::PtyWriter`].
//! Keys, pastes, submitted lines and the emulator's answers are all copied
//! onto its one queue under the lock, in the order the lock was taken, which
//! is the order they were written in when each was written on the spot; the
//! thread writes them out after, holding nothing of the session's but that
//! queue. A keystroke costs a copy and a wake. A submit records its block
//! boundary under the same acquisition as the line it queues, once the queue
//! has taken the line: one refused behind the backlog below opens no block,
//! and the reader, which needs the lock to parse, finds the boundary in place
//! before it can parse anything the shell said in answer.
//!
//! What a child that never reads costs is bounded: a person's input is kept
//! whole, behind a backlog past which more is refused, and the emulator's
//! answers are dropped oldest first past a backlog of their own, since a
//! program that is not reading can use none of them.
//!
//! The lock order is the terminal, then the queue, and nothing takes them the
//! other way round: the writer takes the queue alone, and never holds it
//! across the write, so a push waits for at most a pop.
//!
//! # The lock
//!
//! One mutex per terminal, and **it is never held across a frame**, nor
//! across a write to the pty — see "The writer". The reader
//! takes it to feed bytes, and again to build a snapshot, and publishes the
//! `Arc` into a slot of its own before letting go; painting clones that `Arc`
//! and walks owned data. The UI thread publishes too, after a keystroke or a
//! resize, and doing it under the same lock is what stops either writer from
//! putting an older snapshot back over the other's newer one — see
//! `Shared::latest`. Layout takes the lock only when the computed grid
//! actually changed, which it establishes first with an atomic — so a window
//! being dragged does not contend with a shell that is printing.
//!
//! # What a closed pane costs
//!
//! Nothing, in every case a pane can be closed in — with one residue. Closing
//! kills the child, on a thread of its own rather than the one that draws (see
//! [`Endings`]), which closes the pty, which returns the reader's blocking
//! read; and the reader holds only a [`Weak`] on the session, so the terminal,
//! its ten thousand lines of scrollback and the pty go once the child has been
//! ended, whether or not the reader has noticed yet.
//!
//! The residue is a pane closed while something *other than the child* is
//! holding the pty open — a `sleep 60 &`, a server that outlived the shell that
//! started it. Nothing can make that read return: the slave is open in another
//! process and the reader's own descriptor is a duplicate of the master, so the
//! thread parks until the orphan exits, holding a descriptor and nothing else.
//!
//! What that used to cost as well was the *pane*: end-of-file on the master was
//! the only signal this design had that a session was over, so a shell that
//! exited behind such a process left its pane sitting there showing a dead
//! prompt. [`TerminalModel::watch_children`] asks the children directly instead,
//! once a second and in one task for the whole model, so the pane closes when
//! the shell does whatever the pty is doing.
//!
//! The writer is not part of the residue, save in a process short of
//! descriptors. It holds no reference to the session at all, only its end of
//! the queue, and the terminal being dropped closes the queue and the pty
//! both: a write that was waiting on a child that will never read — even one
//! whose pty an orphan is holding open — is released by that, and the thread
//! ends. What lets the closing reach it on Unix is a pipe beside the master,
//! which a write makes when it first has to wait. A process out of
//! descriptors cannot make one, and the write waits on the master alone; on
//! macOS, one with more open than `select` can watch cannot wait at all, and
//! the write blocks on the master. Either way an orphan holding the pty open
//! then holds the writer too — the thread and its copy of the master — until
//! the orphan exits. See `crook_terminal`'s `nonblocking` module for both.
//!
//! # What a restore costs
//!
//! **The thread that draws starts the shells of the panes it is about to
//! draw, and no others.** Starting one is a fork the parent waits on until
//! the child has exec'd — `portable-pty` sets the child up in a `pre_exec`,
//! which rules out `posix_spawn` — and with a restore's earlier shells
//! starting up beside it, that wait grew from a millisecond to a quarter of a
//! second. Sixty restored tabs, every shell started in place, held the window
//! for most of a second after its first frame.
//!
//! So [`TerminalModel::start`] starts the active tab's shells where it is
//! called, for the frame after it, and queues the rest for the background
//! pool, one at a time in the strip's order, each arrival sending the next:
//! a restore holds one worker however many tabs it brings back, and the wait
//! on each fork is that worker's rather than the frame's. The integration's
//! stub files, the spawn and the reader and writer threads are all made on
//! the worker, and what comes home through `ctx.spawn` is a session ready to
//! be listened to. Until then the pane's shell is on its way
//! ([`TerminalModel::is_opening`]) — drawn as its ground, as every pane is in
//! the frame before `start` — and its arrival is a
//! [`TerminalUpdate::Repainted`], a frame only if somebody is looking.
//!
//! A tab switched to before its turn has come is about to be drawn, so its
//! shells leave the queue and start in place
//! ([`TerminalModel::open_shown`]): a pane with no shell draws no composer
//! and no grid, nothing a key typed into it could reach, and waiting out the
//! queue would lose what was typed meanwhile. Only the one shell already on
//! the pool is left to arrive, a spawn away.
//!
//! A pane that closes while its shell is on the pool is ended by whichever of
//! the two sides gets there second, and a window that goes counts it among
//! the shells it waits for: see [`Opening`]. Everything else starts in place,
//! as it always has — a new tab, a split, a tab brought back — since each is
//! one shell, a few milliseconds, with a frame waiting to draw it.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context as TaskContext, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use crook_terminal::{
    Block, BlockId, BlockRows, BlockState, Key, LiveBlock, Modifiers, MouseButton, MouseEventKind,
    MouseModes, Palette, Rgb, Snapshot, Terminal, TerminalEvent, TerminalOptions, TerminalSize,
};
use crookui_core::geometry::{Color, Vector2F};
use crookui_core::prelude::*;

use crate::completion::{self, Completions};
use crate::pane_surface;
use crate::shell_integration;
use crate::tab::{AgentStatus, PaneId, Remote};
use crate::theme::theme;

/// What [`TerminalHandle::paste_bracketed`] came to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BracketedPaste {
    /// Written, between the markers.
    Sent,
    /// Not written: the program has not asked for bracketed paste.
    NotAsked,
    /// The write to the pty failed.
    Failed,
}

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

/// How long a child that has gone is waited for to be reapable: by the reader
/// once the pty has closed, and by a closed pane's ending once the kill is sent.
///
/// End-of-file on the master means every slave descriptor is shut, which in
/// practice means the child has exited — but "has exited" and "has been reaped"
/// are a scheduling decision apart, and the exit status is worth a short wait.
const REAP_TIMEOUT: Duration = Duration::from_millis(500);

/// How often the shells are asked whether they are still alive.
///
/// A second: fast enough that a pane whose pty is held open by a background
/// process closes while somebody is still looking at it, and one `waitpid` per
/// pane per second is nothing. See [`TerminalModel::watch_children`].
const CHILD_POLL: Duration = Duration::from_secs(1);

/// How long the reader sleeps between attempts to reap.
const REAP_INTERVAL: Duration = Duration::from_millis(5);

/// The grid a terminal starts at when nothing has measured its pane.
///
/// The size every terminal has ever defaulted to, and a fallback for a shell
/// started before any frame at all. Every other pane has a grid in
/// [`Measured`] by the time its shell opens: the panes of the active tab
/// record theirs in the frame the shells wait for, and the rest — the tabs
/// behind it, the pane a split just made — are worked out from the box that
/// frame measured and the tab's own weights. The first layout replaces
/// whichever it was.
///
/// The gap this would leave is real and not only a formality. A shell can
/// print its whole startup — a `~/.zprofile` banner, a greeting sized with
/// `tput cols` — inside its first grid, and what it printed is copied out of
/// the grid into a block the moment the first prompt mark arrives, so a
/// later resize cannot reflow it.
const INITIAL_GRID: TerminalSize = TerminalSize::new(80, 24);

/// The grids the panes measured before they had a terminal to give them to,
/// and the box the active tab's panes were laid out in.
///
/// Layout runs on a shared reference, so the numbers have to travel through
/// something layout can write — the arrangement [`TerminalHandle::resize`]
/// has in its atomics, for a pane that has no [`Shared`] yet. The grids are
/// written by the element that stands in a pane with no terminal; the body
/// by the element the tab's panes are laid out in, so that a pane no layout
/// has seen — one in a tab behind the active one, one just split off — can
/// be given the grid the layout *would* give it, from the box and the tab's
/// own weights. Read by [`TerminalModel::launch`].
#[derive(Clone, Default)]
pub struct Measured(Arc<Mutex<MeasuredState>>);

#[derive(Default)]
struct MeasuredState {
    panes: HashMap<PaneId, TerminalSize>,
    body: Option<Vector2F>,
}

impl Measured {
    /// Records the grid `pane` would hold, as its last layout measured it —
    /// or as the workspace worked it out for a pane no layout has measured.
    pub fn record(&self, pane: PaneId, size: TerminalSize) {
        self.lock().panes.insert(pane, size);
    }

    /// The grid `pane` last measured, if a layout has measured it.
    pub fn get(&self, pane: PaneId) -> Option<TerminalSize> {
        self.lock().panes.get(&pane).copied()
    }

    /// Records the box the active tab's panes share.
    pub fn record_body(&self, size: Vector2F) {
        self.lock().body = Some(size);
    }

    /// The box the active tab's panes share, once a layout has measured it.
    pub fn body(&self) -> Option<Vector2F> {
        self.lock().body
    }

    /// Drops the grid of every pane `keep` does not name.
    ///
    /// A pane id is minted once and never reused, so a grid recorded for one
    /// that has closed is read by nothing — but nothing dropped it either,
    /// and a window that spends a day being split and closed would carry a
    /// grid for every pane it ever held. Reaped alongside the failures, from
    /// the one list that knows which panes are still open. The body is not
    /// per-pane and stays.
    pub fn retain(&self, mut keep: impl FnMut(PaneId) -> bool) {
        self.lock().panes.retain(|pane, _| keep(*pane));
    }

    fn lock(&self) -> MutexGuard<'_, MeasuredState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Something one pane's shell did that the rest of the application cares about.
///
/// Everything else a terminal reports — a query already answered, an exit
/// that is about to show up as end-of-file — is either handled here or is not
/// the workspace's business. What is here renames, relocates or closes a
/// session, asks for something only the window can give, says what an agent
/// is doing — or, most often by far, says a grid changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalUpdate {
    /// What the pane draws has changed: its grid did, or its command has run
    /// long enough to take the composer's place.
    ///
    /// The one update that is about pixels rather than about the session,
    /// and the most frequent — up to once per pane per paint interval while
    /// a shell prints. Reported rather than answered with a notify because
    /// whether it is worth a frame is not this pane's to know: see the
    /// module docs, "Only what is on screen".
    Repainted(PaneId),
    /// The shell set a window title, or reset it. This is what makes a tab of
    /// shells rename itself with no rename plumbing at all.
    Title(PaneId, Option<String>),
    /// The shell reported where it is, with OSC 7, on this machine. Every git
    /// fact a row shows is looked up by this, so it is also what makes the
    /// branch chip follow a `cd`.
    WorkingDirectory(PaneId, PathBuf),
    /// The shell reported where it is on another machine: OSC 7 named a host
    /// that is not this one, which is what a shell on the far side of an
    /// `ssh` does. Nothing on this machine is read by it — see [`Remote`].
    RemoteDirectory(PaneId, Remote),
    /// What the pane is running, or `None` once it went back to a prompt.
    ///
    /// The command line off the open block, which the shell integration's
    /// OSC 133 marks already report — so this costs nothing to know and is the
    /// one fact about a session the strip never showed. It is what a tab with
    /// no name of its own calls itself: `cargo test` while the tests run, and
    /// the directory again when they stop.
    Running(PaneId, Option<String>),
    /// The shell is gone. The pane should close, and with it its tab and the
    /// window if they were the last ones.
    Closed(PaneId),
    /// The child asked for text to be put on the system clipboard with OSC 52.
    ///
    /// Only the window has a clipboard, so the terminal cannot answer this
    /// itself. The reverse direction — a child *reading* the clipboard — is
    /// refused in the emulator and never reaches here: answering it would hand
    /// any program that can print to a pty the contents of the clipboard.
    ClipboardStore(PaneId, String),
    /// The bell rang.
    ///
    /// What to do with it is the workspace's to decide, because the answer is
    /// about the tab strip rather than about the grid: a bell in a pane nobody
    /// is looking at is the only interesting kind of bell to *the strip*, and
    /// plugins watching for one have their own opinion about the rest.
    Bell {
        /// Which pane rang.
        pane: PaneId,
        /// Whether the shell had said a command was running when it did.
        ///
        /// Read here rather than worked out later because this is where the
        /// snapshot is: by the time the workspace hears about the bell the
        /// command may have ended, and a bell is told apart from a shell's own
        /// prompt-time ringing by what was running at the moment it rang.
        while_running: bool,
    },
    /// A command in a pane finished, because the shell said so with OSC 133
    /// `D`.
    ///
    /// Carried up rather than answered here for the same reason the bell is:
    /// what to do about a command ending is a question about the whole window
    /// — which plugins are watching, and whether this pane is the one being
    /// looked at — and the model of one pane cannot see any of that.
    CommandFinished {
        /// Which pane it ran in.
        pane: PaneId,
        /// The command line, when there was one. Display-only: it came off
        /// the screen, as [`crook_terminal::Block::command`] did.
        command: Option<String>,
        /// The status the shell reported, or `None` when it reported none.
        exit: Option<i32>,
        /// How long it ran, timed from the submit.
        took: Option<Duration>,
        /// Whether anything ran: `false` for the bare `D` of a line that ran
        /// nothing, ctrl-c at the prompt or an empty Enter.
        ran: bool,
    },
    /// The shell answered a completion request, and this is what it said.
    ///
    /// The serial is the request's, echoed back through the shell: pressing
    /// Tab twice quickly leaves two outstanding, and only the answer to the
    /// second is about the line on screen.
    Completions(PaneId, u64, Completions),
    /// A program in the pane said what it is doing.
    ///
    /// The status is the app's own from here on: the wire's word is the
    /// emulator's business, and what a tab does with it is the strip's.
    Agent {
        /// Which pane it is in.
        pane: PaneId,
        /// What it said.
        status: AgentStatus,
        /// What it called its work, when it said.
        title: Option<String>,
        /// What it is waiting for, when it said.
        message: Option<String>,
    },
    /// The command a running or waiting agent was ended, and the emulator
    /// took the status back to idle for it.
    ///
    /// Told apart from an [`Self::Agent`] saying idle so that a row's menu can
    /// say which of the two happened: the agent finished, or something ended
    /// it before it could say so.
    AgentSettled(PaneId),
    /// The shell has reached its first prompt, and a line sent now is a
    /// command of its own. Once per shell.
    ///
    /// For a shell that reports marks that is the first `A`: a line submitted
    /// into the block still open from before it is one the shell never
    /// reports a boundary for. For one that reports none it is the first
    /// thing it prints, which is the most a shell with no marks ever says
    /// about being ready — the wait `--run` makes before it types.
    Prompted(PaneId),
    /// A program in the pane asked for a look, in a notification sequence
    /// another terminal would have shown — OSC 9, 777 or 99.
    ///
    /// Carried up with its two texts apart rather than joined here, because
    /// the row has one line for them and a desktop notification has a line
    /// for each: how they are put together is the reader's to decide.
    Notification {
        /// Which pane it came from.
        pane: PaneId,
        /// What is asking, when the sequence named it.
        title: Option<String>,
        /// What it said.
        body: Option<String>,
    },
    /// A program in the pane said which pull request its work is.
    ///
    /// The address has been checked on the wire — https, capped, nothing a
    /// link opener could be tricked by — and is carried up rather than kept
    /// here because what it is recorded against is the pane's branch, which
    /// is the workspace's to know.
    PullRequest {
        /// Which pane it is in.
        pane: PaneId,
        /// The pull request's address.
        url: String,
    },
}

/// The finished blocks of one pane, as the surface holds them.
///
/// One [`Arc`] per block rather than one per list, because the list is
/// append-only: a batch that closes one command clones a pointer per block
/// that was already there and copies the rows of exactly the new one. The
/// whole structure is behind another `Arc`, so a frame takes the history for
/// the cost of a pointer.
///
/// Kept here rather than read off the emulator every frame for the reason the
/// snapshot is: the terminal's lock belongs to whichever thread is parsing,
/// and painting must never queue behind a shell mid-burst.
#[derive(Debug, Default)]
pub struct BlockHistory {
    /// Oldest first.
    blocks: Vec<Arc<Block>>,
    /// How many blocks have been dropped off the front since the session
    /// started. A list caching geometry per index compares this against what
    /// it saw last to learn that the front moved rather than the back grew.
    evicted: usize,
    /// What `evicted` came to when the pane was last cleared. See
    /// [`Self::evicted_since_clear`].
    cleared_at: usize,
}

impl BlockHistory {
    /// A history holding `blocks`, oldest first, `evicted` of which have
    /// already been dropped off the front.
    pub fn new(blocks: Vec<Arc<Block>>, evicted: usize) -> Self {
        Self {
            blocks,
            evicted,
            cleared_at: 0,
        }
    }

    /// The same history, of a pane last cleared when `evicted` came to
    /// `cleared_at`.
    #[must_use]
    pub fn cleared_at(self, cleared_at: usize) -> Self {
        Self { cleared_at, ..self }
    }

    /// The blocks, oldest first.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Arc<Block>> {
        self.blocks.iter()
    }

    /// One block by its position in the list, oldest first.
    pub fn get(&self, index: usize) -> Option<&Arc<Block>> {
        self.blocks.get(index)
    }

    /// How many blocks are held.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether no command has finished yet.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Where a block is in the list, or `None` once it has been evicted.
    ///
    /// A binary search, because ids are handed out in order and never reused:
    /// a selection resolved against a session with ten thousand blocks behind
    /// it costs a handful of comparisons rather than a walk.
    pub fn position(&self, id: BlockId) -> Option<usize> {
        self.blocks.binary_search_by_key(&id, |block| block.id).ok()
    }

    /// How many blocks have been dropped off the front.
    pub fn evicted(&self) -> usize {
        self.evicted
    }

    /// How many of [`Self::evicted`] went since the pane was last cleared:
    /// pushed off the front by the cap, so that the first block held still
    /// had blocks before it. The ones a clear let go of are not counted, a
    /// pane somebody cleared having nothing above what is left in it.
    pub fn evicted_since_clear(&self) -> usize {
        self.evicted.saturating_sub(self.cleared_at)
    }
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

    /// The shells still to be opened on the background pool, next first: the
    /// tabs behind the active one, when the window started. See "What a
    /// restore costs" in the module docs.
    queued: VecDeque<(PaneId, Option<PathBuf>)>,

    /// The one shell being opened on the pool, while it is.
    opening: Option<(PaneId, Opening)>,

    /// Whether [`Self::start`] is coming, once a frame has measured the panes.
    ///
    /// The one frame between the two is drawn with no shell in any pane, and
    /// this is what tells the pane to draw its ground and nothing else for
    /// it, rather than the notice a pane that will never have a shell draws.
    expected: bool,

    /// The grids the panes measured before their shells opened.
    measured: Measured,

    /// The colours every terminal resolves its cells against.
    palette: Palette,

    /// Whether the chain that asks the shells whether they are still alive is
    /// already running.
    ///
    /// One for the whole model rather than one per pane, and this is the flag
    /// that keeps it that way: every session that opens asks for the chain, and
    /// only the first one starts it. See [`Self::watch_children`].
    watching_children: bool,

    /// Whether a pane installs command marks into the shell it opens.
    ///
    /// On, and the seam a setting hangs on when there is one. Off is not a
    /// broken state: it is exactly what a person gets on a shell Crook has no
    /// integration for, on a machine whose temporary directory cannot be
    /// written, or with [`shell_integration::OPT_OUT_VARIABLE`] set — one open
    /// block holding the whole session, which the surface draws as one
    /// continuous stream.
    shell_marks: bool,

    /// Whether a pane's shell is started as a login shell.
    ///
    /// On, and set from
    /// [`GeneralOptions::login_shell`](crate::settings::GeneralOptions::login_shell).
    /// It is what makes the shell read `~/.zprofile`, `~/.bash_profile` and the
    /// rest — the files a person's `PATH` is built in.
    shell_login: bool,

    /// The shell a pane runs, when it is not the user's own.
    ///
    /// `None` is the user's shell, resolved the way every terminal resolves
    /// it. `Some` is what `--shell` asked for — another shell to try, or, for
    /// a picture and a test, one that does not exist, since a pane whose shell
    /// could not be started is a state nothing else can reach on purpose.
    shell: Option<PathBuf>,

    /// Where the window answers questions, for the shells to be told.
    ///
    /// `None` until the window has opened its socket, and for good on a
    /// platform with none: every shell is then told `CROOK_SOCKET` is empty.
    /// See [`crate::control`].
    control_socket: Option<PathBuf>,

    /// The one thread that comes back for batches parsed too soon to draw.
    ///
    /// Shared by every pane and started with the first of them, so a model that
    /// never opens a terminal — the headless snapshot, a test — never starts a
    /// thread either.
    flusher: Arc<Flusher>,
    flushing: bool,

    /// The shells of closed panes that are still being ended.
    endings: Endings,

    /// Variables every shell opened from now on is given last, over its own.
    /// See [`Self::set_environment`].
    #[cfg(test)]
    environment: Vec<(String, String)>,

    /// How long every spawn is made to take on top of its own cost. See
    /// [`Self::set_spawn_cost`].
    #[cfg(test)]
    spawn_cost: Duration,
}

/// One running shell, as the model holds it.
struct Session {
    shared: Arc<Shared>,
    /// The scratch files the shell was started against, held for as long as
    /// the shell runs. Dropping it removes the directory the shell's rc stubs
    /// live in, so it must outlive the process reading them — see
    /// [`shell_integration::Session`].
    _integration: shell_integration::Session,
    /// Whether a repaint is already scheduled for the moment the open block
    /// becomes long-running.
    ///
    /// One at a time: a command that prints while it runs would otherwise
    /// queue a timer per batch, all of them landing on the same frame.
    long_running_timer: bool,
    /// The last snapshot delivered to the main thread. Compared by pointer to
    /// decide whether a frame is worth spending.
    snapshot: Arc<Snapshot>,
    /// What the shell last called itself, so a title set twice notifies once.
    title: Option<String>,
    /// Where the shell last said it was, and on which host, for the same
    /// reason.
    directory: Option<(Option<String>, PathBuf)>,
    /// What it was last running, for the same reason again.
    running: Option<String>,
    /// The secret this pane's shell was handed for the control socket, and
    /// the whole of how the window tells which pane a request comes from.
    /// Goes with the session, so a token outlives its pane by nothing.
    token: Option<String>,
    /// Whether the shell reports command marks, which decides what its first
    /// prompt looks like from here: see [`TerminalUpdate::Prompted`].
    marks: bool,
    /// Whether [`TerminalUpdate::Prompted`] has been sent for this shell.
    prompted: bool,
    /// The shell this pane was started with, which the setting may have
    /// moved on from since: see [`TerminalModel::shell_of`].
    shell: PathBuf,
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
            queued: VecDeque::new(),
            opening: None,
            expected: false,
            measured: Measured::default(),
            palette: crook_palette(),
            watching_children: false,
            shell_marks: true,
            shell_login: shell_integration::login_by_default(),
            shell: None,
            control_socket: None,
            flusher: Arc::new(Flusher::default()),
            flushing: false,
            endings: Endings::default(),
            #[cfg(test)]
            environment: Vec::new(),
            #[cfg(test)]
            spawn_cost: Duration::ZERO,
        }
    }

    /// Lets panes open shells, and opens one for every pane already there:
    /// `shown`'s here, before this returns, for the next frame to draw, and
    /// `behind`'s on the background pool, one after another in the order
    /// given. See "What a restore costs" in the module docs.
    ///
    /// Call once, after the window exists. Separate from [`Self::new`] for the
    /// reason the poll chains' `start` is: nothing renders a frame in CI with
    /// four shells running underneath it.
    pub fn start(
        &mut self,
        shown: &[(PaneId, Option<PathBuf>)],
        behind: &[(PaneId, Option<PathBuf>)],
        ctx: &mut ModelContext<Self>,
    ) {
        if self.live {
            return;
        }
        self.live = true;
        // Queued before the sync, which opens in place every pane that has
        // no shell and is not waiting for one.
        self.queued = behind.iter().cloned().collect();
        let panes: Vec<_> = shown.iter().chain(behind).cloned().collect();
        self.sync(&panes, ctx);
        self.open_next(ctx);
    }

    /// Whether shells are being opened at all.
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// Says that [`Self::start`] is coming after the next frame.
    ///
    /// The frame is what measures the panes, and a shell opened before it
    /// opens at [`INITIAL_GRID`] — which is where a startup banner sized with
    /// `tput cols` gets its wrong answer. Until `start`, a pane draws its
    /// ground and nothing else, which is what it draws for the first frame
    /// after a shell opens too.
    pub fn expect(&mut self) {
        self.expected = true;
    }

    /// Whether shells are coming and have not been started yet.
    pub fn is_expected(&self) -> bool {
        self.expected && !self.live
    }

    /// Whether `pane`'s shell is on its way: waiting its turn, or being
    /// opened on the pool.
    ///
    /// Such a pane has no terminal yet and has not failed to get one. It is
    /// drawn as every pane is in the frame before [`Self::start`], its ground
    /// and nothing else, and nothing should take it for a pane whose shell
    /// has gone.
    pub fn is_opening(&self, pane: PaneId) -> bool {
        self.queued.iter().any(|(queued, _)| *queued == pane)
            || self
                .opening
                .as_ref()
                .is_some_and(|(opening, _)| *opening == pane)
    }

    /// Where a pane with no terminal writes the grid it measured.
    pub fn measured(&self) -> Measured {
        self.measured.clone()
    }

    /// Says whether shells opened from now on install command marks.
    ///
    /// Only shells opened *after* this: the integration is installed when a
    /// shell starts and cannot be taken out of one that is already running.
    pub fn set_shell_marks(&mut self, enabled: bool) {
        self.shell_marks = enabled;
    }

    /// Says whether shells opened from now on are login shells.
    ///
    /// Only shells opened *after* this, and for the same reason: a shell reads
    /// its startup files once, before it draws its first prompt, and no message
    /// sent afterwards can make it read them again.
    pub fn set_shell_login(&mut self, login: bool) {
        self.shell_login = login;
    }

    /// Says where the window's control socket is, for the shells opened from
    /// now on.
    ///
    /// Only those, for the reason every setting here is: a shell's environment
    /// is fixed when it starts. The window sets it before its first shell
    /// opens, so no pane of a window with a socket goes without it.
    pub fn set_control_socket(&mut self, socket: Option<PathBuf>) {
        self.control_socket = socket;
    }

    /// Whether the next shell opened will be a login shell.
    pub fn shell_login(&self) -> bool {
        self.shell_login
    }

    /// Says which shell panes opened from now on run, or `None` for the
    /// user's own. Only shells opened after this, like the two above.
    pub fn set_shell(&mut self, shell: Option<PathBuf>) {
        self.shell = shell;
    }

    /// The shell the next pane runs: the one named, or the user's own
    /// resolved the way a launch resolves it.
    pub fn shell(&self) -> PathBuf {
        self.shell
            .clone()
            .unwrap_or_else(|| PathBuf::from(crook_terminal::default_shell()))
    }

    /// Says what shells opened from now on are given in their environment,
    /// over what they inherit and what the integration sets. For tests.
    ///
    /// A shell a test starts inherits the home of whoever runs the suite, and
    /// an interactive bash reads its history file there as it starts, cutting
    /// it to `HISTFILESIZE` lines — 500 where no rc file says otherwise — and
    /// saves into it as it ends. Only the environment it starts with keeps it
    /// out: a line typed into it afterwards comes after the cut.
    #[cfg(test)]
    pub fn set_environment(&mut self, environment: Vec<(String, String)>) {
        self.environment = environment;
    }

    /// Makes every shell opened from now on take `cost` longer to spawn, on
    /// whichever thread spawns it. For tests.
    ///
    /// What a loaded machine charges: the parent waits for each child to
    /// exec, and with a restore's earlier shells starting up beside it that
    /// wait reached a quarter of a second. A test that measures what the
    /// thread that draws spends on a restore states the cost instead of
    /// depending on how fast the machine running it forks.
    #[cfg(test)]
    pub fn set_spawn_cost(&mut self, cost: Duration) {
        self.spawn_cost = cost;
    }

    /// Whether a shell was named, rather than the user's own resolved.
    pub fn shell_is_named(&self) -> bool {
        self.shell.is_some()
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
        // A pane that closed before its turn came is never opened, and one
        // whose shell is being opened is ended by whichever side holds it.
        self.queued
            .retain(|(pane, _)| panes.iter().any(|(open, _)| open == pane));
        if let Some((pane, opening)) = &self.opening
            && !panes.iter().any(|(open, _)| open == pane)
        {
            opening.abandon(*pane, &self.endings);
        }
        self.failures
            .retain(|pane, _| panes.iter().any(|(open, _)| open == pane));
        self.measured
            .retain(|pane| panes.iter().any(|(open, _)| *open == pane));

        if !self.live {
            return;
        }
        for (pane, directory) in panes {
            if !self.sessions.contains_key(pane) && !self.is_opening(*pane) {
                self.open(*pane, directory.clone(), ctx);
            }
        }
    }

    /// Starts here and now the shells of `shown` that are still waiting
    /// their turn on the pool: a tab behind the active one, switched to
    /// before a restore got to it.
    ///
    /// Its panes are about to be drawn, and a pane with no shell draws no
    /// composer and no grid, so whatever was typed into it until its turn
    /// came would reach nothing. One tab's shells cost what a new tab's do.
    /// The one shell already being spawned on the pool is left to arrive. See
    /// "What a restore costs" in the module docs.
    pub fn open_shown(&mut self, shown: &[PaneId], ctx: &mut ModelContext<Self>) {
        if !self.live {
            return;
        }
        let mut brought = Vec::new();
        self.queued.retain(|(pane, directory)| {
            let waits = !shown.contains(pane);
            if !waits {
                brought.push((*pane, directory.clone()));
            }
            waits
        });
        for (pane, directory) in brought {
            self.open(pane, directory, ctx);
        }
    }

    /// The pane whose shell was handed `token`, while that shell runs.
    ///
    /// A walk of the open sessions, which are a few dozen at most, asked once
    /// per request that carries a token. An empty token is nobody's: it is
    /// what a pane with none is told, and it must not match one either.
    pub fn pane_with_token(&self, token: &str) -> Option<PaneId> {
        if token.is_empty() {
            return None;
        }
        self.sessions
            .iter()
            .find(|(_, session)| session.token.as_deref() == Some(token))
            .map(|(pane, _)| *pane)
    }

    /// The token a pane's shell was handed, for a test that has to ask as it.
    ///
    /// Unix only, as the socket tests that ask are: a Windows test build
    /// would find it unused.
    #[cfg(all(test, unix))]
    pub fn token(&self, pane: PaneId) -> Option<&str> {
        self.sessions.get(&pane)?.token.as_deref()
    }

    /// Whether a pane's shell reports command marks, or `None` when the pane
    /// has no shell running. A shell without them is one open block for the
    /// whole session: nothing in it ever finishes.
    pub fn marks(&self, pane: PaneId) -> Option<bool> {
        self.sessions.get(&pane).map(|session| session.marks)
    }

    /// The shell a pane is running, or `None` when it has none.
    ///
    /// The one it was started with, not [`Self::shell`]: that is the next
    /// pane's, and changes with the setting while this one goes on running
    /// what it was given.
    pub fn shell_of(&self, pane: PaneId) -> Option<&Path> {
        self.sessions
            .get(&pane)
            .map(|session| session.shell.as_path())
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

    /// The commands that have finished in a pane, oldest first.
    pub fn blocks(&self, pane: PaneId) -> Option<Arc<BlockHistory>> {
        self.sessions
            .get(&pane)
            .map(|session| session.shared.blocks())
    }

    /// Asks a pane's shell what a half-typed line could become.
    ///
    /// Writes the question into the session's own scratch and sends the key
    /// press the integration snippet bound to it — see [`crate::completion`]
    /// for why the line travels in a file rather than in the escape sequence.
    ///
    /// Reports whether the question was asked at all. `false` is a pane with
    /// no shell, or one running a shell Crook could not install its
    /// integration into: there is nothing bound to the key and nothing would
    /// ever answer, so the caller must not sit waiting for one.
    pub fn request_completions(&self, pane: PaneId, serial: u64, line_to_caret: &str) -> bool {
        let Some(session) = self.sessions.get(&pane) else {
            return false;
        };
        // Written before the key is sent, and that ordering is the whole of the
        // handshake: the snippet reads the file the moment the key arrives.
        match session
            ._integration
            .write_completion_request(&completion::request_text(serial, line_to_caret))
        {
            Some(Ok(())) => {}
            None => return false,
            Some(Err(error)) => {
                log::debug!("could not write a completion request: {error}");
                return false;
            }
        }

        session.shared.request_completions()
    }

    /// Types `text` into a pane's shell, as if a person had.
    ///
    /// Queued for the session's writer, so this works before the shell has
    /// finished starting: what is sent now is read when it gets there.
    pub fn type_into(&self, pane: PaneId, text: &str) {
        let Some(session) = self.sessions.get(&pane) else {
            log::warn!("nothing to type into: pane {pane:?} has no terminal");
            return;
        };
        if let Err(error) = session.shared.lock().write(text.as_bytes()) {
            log::warn!("could not write to the shell in pane {pane:?}: {error}");
        }
    }

    /// Repaints every shell in the palette a new theme resolved.
    ///
    /// The one thing applying a theme has to do that repainting the chrome
    /// does not: a grid resolves its colours through a palette that was handed
    /// to it when its shell started, so a theme change has to reach every
    /// terminal that is already running. Each one is given the new palette and
    /// asked to publish a fresh snapshot — the emulator has not changed, only
    /// what its cells resolve to, so nothing is reparsed and the shell is not
    /// told anything.
    pub fn set_palette(&mut self, palette: Palette, ctx: &mut ModelContext<Self>) {
        if self.palette == palette {
            return;
        }
        self.palette = palette;

        for session in self.sessions.values() {
            session.shared.repaint_in(self.palette.clone());
        }
        ctx.notify();
    }

    /// Starts a shell for one pane, and the threads that read and write it,
    /// here and now.
    fn open(&mut self, pane: PaneId, directory: Option<PathBuf>, ctx: &mut ModelContext<Self>) {
        let opened = self
            .launch(pane, directory)
            .and_then(|launch| spawn_session(launch, &self.flusher));
        match opened {
            Ok(opened) => self.register(pane, opened, ctx),
            Err(reason) => {
                self.failures.insert(pane, reason);
            }
        }
        ctx.notify();
    }

    /// Works out everything a pane's shell is started with, from what the
    /// model says now, so that the spawn itself can happen on any thread.
    ///
    /// Refuses a shell that cannot be started, with the reason the pane
    /// shows.
    fn launch(&mut self, pane: PaneId, directory: Option<PathBuf>) -> Result<Launch, String> {
        // A shell named by a path that is not there is refused here, before
        // anything is spawned, because below here the platforms disagree
        // about it: Linux reports the missing program from the spawn, macOS
        // forks and the child dies on the exec — and where the name reaches
        // `portable-pty`'s own resolution, a login shell it has no switch
        // for, both fall back to the password database's shell. That
        // fallback is right for `$SHELL` outliving the shell it names, and
        // wrong for a shell a person asked for by name: they asked for that
        // one, and the pane owes them the reason it did not open.
        if let Some(shell) = &self.shell
            && shell.components().count() > 1
            && !shell.exists()
        {
            let reason = format!("{} does not exist", shell.display());
            log::error!("could not open a shell for pane {pane:?}: {reason}");
            return Err(reason);
        }

        // Before the spawn, wherever that happens: the reader hands this
        // thread the batches it parsed too soon to draw from its first read.
        if let Err(error) = self.start_flushing() {
            log::error!("could not start the repaint thread for pane {pane:?}: {error}");
            return Err(format!("{error}"));
        }

        // Only for a window that has a socket to send it to: a token that
        // could reach nothing would be a secret handed out for nothing.
        let token = self
            .control_socket
            .is_some()
            .then(crate::control::mint_token)
            .flatten();
        Ok(Launch {
            pane,
            integration: shell_integration::Options {
                enabled: self.shell_marks,
                login: self.shell_login,
                shell: self.shell.clone(),
                control_socket: self.control_socket.clone(),
                control_token: token.clone(),
            },
            options: TerminalOptions {
                size: self.measured.get(pane).unwrap_or(INITIAL_GRID),
                working_directory: directory,
                palette: self.palette.clone(),
                // What `TERM_PROGRAM` and `TERM_PROGRAM_VERSION` say, for a
                // program that asks the terminal instead of its environment.
                // The answer reaches it through ssh and sudo, where the
                // variables do not, and Claude Code waits on it before it will
                // draw its frames whole.
                name_and_version: Some(format!(
                    "{} {}",
                    shell_integration::TERM_PROGRAM,
                    shell_integration::VERSION
                )),
                ..Default::default()
            },
            token,
            shell: self.shell(),
            #[cfg(test)]
            environment: self.environment.clone(),
            #[cfg(test)]
            cost: self.spawn_cost,
        })
    }

    /// Takes a shell that has started into the model, and starts listening
    /// to it.
    fn register(&mut self, pane: PaneId, opened: Opened, ctx: &mut ModelContext<Self>) {
        let Opened {
            shared,
            integration,
            marks,
            token,
            shell,
            palette,
        } = opened;
        // A theme applied while the shell was being opened on the pool
        // reached every session but this one.
        if palette != self.palette {
            shared.repaint_in(self.palette.clone());
        }
        let snapshot = shared.snapshot();

        self.failures.remove(&pane);
        self.sessions.insert(
            pane,
            Session {
                shared,
                _integration: integration,
                long_running_timer: false,
                snapshot,
                title: None,
                directory: None,
                running: None,
                token,
                marks,
                prompted: false,
                shell,
            },
        );
        self.watch(pane, ctx);
        self.watch_children(ctx);
    }

    /// Opens the next queued shell on the background pool, unless one is
    /// being opened there already or the model has stopped opening any.
    ///
    /// One at a time, each arrival starting the next — the shape "Update
    /// all" drains its downloads in — so a restore of sixty tabs holds one
    /// worker rather than every worker the pool has, and the shells start in
    /// the order the strip lists them.
    fn open_next(&mut self, ctx: &mut ModelContext<Self>) {
        while self.live && self.opening.is_none() {
            let Some((pane, directory)) = self.queued.pop_front() else {
                return;
            };
            let launch = match self.launch(pane, directory) {
                Ok(launch) => launch,
                Err(reason) => {
                    self.failures.insert(pane, reason);
                    ctx.notify();
                    continue;
                }
            };

            let opening = Opening::default();
            let filling = opening.clone();
            let flusher = self.flusher.clone();
            let endings = self.endings.clone();
            let opened = ctx.background().spawn(async move {
                filling.fill(pane, || spawn_session(launch, &flusher), &endings);
            });
            ctx.spawn(opened, move |model, (), ctx| model.arrive(pane, ctx))
                .detach();
            self.opening = Some((pane, opening));
        }
    }

    /// Takes in the shell the pool opened for `pane`, and opens the next.
    fn arrive(&mut self, pane: PaneId, ctx: &mut ModelContext<Self>) {
        let Some((_, opening)) = self.opening.take_if(|(opening, _)| *opening == pane) else {
            return;
        };
        match opening.take() {
            Some(Ok(opened)) => {
                self.register(pane, opened, ctx);
                // Reported rather than notified, as a changed grid is: a pane
                // that has just got its shell is worth a frame only if it is
                // on screen, which is the workspace's to know — and a pane a
                // restore opens here almost never is.
                ctx.emit(TerminalUpdate::Repainted(pane));
            }
            Some(Err(reason)) => {
                self.failures.insert(pane, reason);
                ctx.notify();
            }
            // Its pane closed, or its window went, while it was on the way,
            // and whichever got there second has ended it.
            None => {}
        }
        self.open_next(ctx);
    }

    /// Notices that a shell has exited even though its pty has not.
    ///
    /// **The one thing end-of-file cannot tell us.** The reader thread learns a
    /// session is over by the pty master going quiet, and something other than
    /// the shell can hold the far end open: a `sleep 60 &`, a dev server
    /// started with an `&`, an `ssh -f`. The shell exits, the descriptor stays
    /// open, and the reader parks on it for as long as that process lives —
    /// leaving a pane sitting there showing a dead shell.
    ///
    /// So the child is asked directly. It is a poll, at a second, and that is
    /// a deliberate trade: the alternative is moving the child onto a thread of
    /// its own to block in `wait`, and the moment the child is reaped anywhere
    /// but under this lock, a `kill` racing it can signal whatever process id
    /// the system has since handed out. A `waitpid` with `WNOHANG` once a
    /// second costs nothing and cannot do that.
    ///
    /// **One chain for the whole model**, not one per pane, and that is not a
    /// tidiness point. A task that sleeps holds its worker for the whole cycle
    /// — see `PARKED_WORKERS` in `crate` — so a chain per pane would park a
    /// worker per pane, and a window with more panes than the machine has
    /// cores would have no worker left to run anything else on.
    ///
    /// The chain ends when the last session does, which is what stops a closed
    /// window from going on asking about shells that are gone.
    fn watch_children(&mut self, ctx: &mut ModelContext<Self>) {
        if self.sessions.is_empty() || self.watching_children {
            return;
        }
        self.watching_children = true;

        let shared: Vec<_> = self
            .sessions
            .iter()
            .map(|(pane, session)| (*pane, session.shared.clone()))
            .collect();

        let asking = ctx.background().spawn(async move {
            thread::sleep(CHILD_POLL);
            shared
                .into_iter()
                .filter(|(_, session)| {
                    // Under the terminal's own lock, like every other
                    // `try_wait` — the exit is cached there, and that cache is
                    // what stops a later kill from signalling a stranger.
                    match session.lock().try_wait() {
                        Ok(exit) => exit.is_some(),
                        Err(error) => {
                            log::debug!("could not check on a shell: {error:#}");
                            false
                        }
                    }
                })
                .map(|(pane, _)| pane)
                .collect::<Vec<_>>()
        });

        ctx.spawn(asking, move |model, exited, ctx| {
            model.watching_children = false;
            for pane in exited {
                if model.sessions.contains_key(&pane) {
                    // The same close every other path takes: the workspace is
                    // what knows a tab's last pane takes the tab, and the last
                    // tab the window. Nothing here removes the session — the
                    // close comes back round through `sync`.
                    ctx.emit(TerminalUpdate::Closed(pane));
                }
            }
            model.watch_children(ctx);
        })
        .detach();
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

    /// Ends a pane's shell and lets its reader and writer threads finish.
    ///
    /// Killing the child is what closes the pty, which is what returns the
    /// reader's blocking read; the terminal being dropped once nothing holds it
    /// any more is what ends the writer, whatever it was waiting on — but for a
    /// write that began to wait in a process short of descriptors, which an
    /// orphan holding the pty open keeps waiting, as the module docs say under
    /// "What a closed pane costs".
    ///
    /// **The kill is not done here.** This runs on the thread that draws, and
    /// a kill waits: it hangs the child up and looks at once, before any child
    /// can have gone, so it costs a 50 ms sleep for every shell and 200 ms for
    /// one that ignores the hang-up. Done here, every pane closed was a
    /// three-frame hitch, and "Close other tabs" on fifteen froze the window
    /// for most of a second. The session leaves the model now, so the pane is
    /// gone from the window at once, and [`Endings`] kills and reaps its child
    /// on a thread of its own.
    fn close(&mut self, pane: PaneId) {
        let Some(session) = self.sessions.remove(&pane) else {
            return;
        };
        session.shared.wake.finish();
        self.endings.end(pane, session.shared, session._integration);
    }

    /// Ends the shell in every pane, for a window that is going, and opens
    /// none from now on — not even for a pane the strip still holds.
    ///
    /// Hands back what to wait on before the process ends: the threads ending
    /// the shells die with it, and a shell that ignores the hang-up would
    /// outlive Crook if the thread that was to kill it never had the time.
    /// Called by the window's delegate as the event loop stops rather than
    /// left to [`Drop`], because macOS's Quit ends the process there and drops
    /// nothing.
    pub fn close_all(&mut self) -> Endings {
        // Both, since a model that is expecting shells and has none live is
        // one whose next frame starts them.
        self.live = false;
        self.expected = false;
        // The shells still waiting their turn never get one. The one on its
        // way is ended by whichever side holds it, and counted in what this
        // hands back either way.
        self.queued.clear();
        if let Some((pane, opening)) = &self.opening {
            opening.abandon(*pane, &self.endings);
        }
        for pane in self.sessions.keys().copied().collect::<Vec<_>>() {
            self.close(pane);
        }
        self.endings.clone()
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

    /// Takes what the reader posted, reports a repaint if it changed the grid,
    /// and waits again.
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

        // Read off the snapshot rather than from an event, because there is no
        // event for it: a block opens and closes as the marks arrive, and what
        // matters here is the resting state after they have. `None` the moment
        // the block stops running, so a tab goes back to naming itself after
        // its directory rather than after the last thing it happened to run.
        //
        // A start goes out ahead of the batch's events, since what the command
        // wrote came after it: its first title is the command's. A stop waits
        // for the events written before it. A program's last title comes
        // before the mark that ended it, and the workspace, which takes back a
        // running the title said when the pane goes back to a prompt, would
        // otherwise hear the title say running again for a program that had
        // gone, with nothing left to take it back. The stop goes out just
        // before the batch's first end of a command, where a `D` stopped the
        // block, so a command is still heard to stop before it is heard to
        // finish; or after every event, when an `A` with no `D` stopped it.
        let running = running_command(&session.snapshot.live_block);
        let mut stopped = None;
        if session.running != running {
            session.running = running.clone();
            match running {
                Some(_) => updates.push(TerminalUpdate::Running(pane, running)),
                None => stopped = Some(TerminalUpdate::Running(pane, None)),
            }
        }
        // Read off the snapshot for the reason `running` is: what matters is
        // where the shell has come to rest, not which marks it took to get
        // there. Only until it has been said once, so the text of a grid is
        // built for a shell with no marks until it prints and never after.
        //
        // In a shell with marks the first prompt is an `A`, written after
        // anything the startup files ran, so it waits for the events as a stop
        // does: it takes back a running title too. In a shell with none it is
        // the first text printed, which may be a program's own first frame,
        // and it goes out first, so it cannot take back a running that frame's
        // title has only just said.
        let mut prompted_after_the_events = None;
        if !session.prompted {
            let prompted = match session.marks {
                true => session.snapshot.live_block.state == BlockState::AtPrompt,
                false => !session.snapshot.text().trim().is_empty(),
            };
            if prompted {
                session.prompted = true;
                match session.marks {
                    true => prompted_after_the_events = Some(TerminalUpdate::Prompted(pane)),
                    false => updates.push(TerminalUpdate::Prompted(pane)),
                }
            }
        }
        for event in events {
            match event {
                TerminalEvent::Title(title) => {
                    let title = title.filter(|title| !title.trim().is_empty());
                    if session.title != title {
                        session.title = title.clone();
                        updates.push(TerminalUpdate::Title(pane, title));
                    }
                }
                TerminalEvent::WorkingDirectory { host, path } => {
                    let place = (host, path);
                    if session.directory.as_ref() != Some(&place) {
                        let (host, path) = place.clone();
                        session.directory = Some(place);
                        updates.push(match Remote::reported(host.as_deref(), path.clone()) {
                            Some(remote) => TerminalUpdate::RemoteDirectory(pane, remote),
                            None => TerminalUpdate::WorkingDirectory(pane, path),
                        });
                    }
                }
                TerminalEvent::ChildExited(exit) => {
                    log::debug!("the shell in pane {pane:?} {exit}");
                }
                // The child asked the terminal to close. It is about to stop
                // being readable anyway, so this is only ever early notice.
                TerminalEvent::Exit => log::debug!("the shell in pane {pane:?} asked to close"),
                TerminalEvent::Bell => updates.push(TerminalUpdate::Bell {
                    pane,
                    while_running: session.snapshot.live_block.state.is_running(),
                }),
                TerminalEvent::CommandFinished {
                    command,
                    exit,
                    took,
                    ran,
                } => {
                    updates.extend(stopped.take());
                    updates.push(TerminalUpdate::CommandFinished {
                        pane,
                        command,
                        exit,
                        took,
                        ran,
                    });
                }
                // The escape sequence says only that an answer is ready; the
                // answer itself is a file, in a directory this session owns.
                TerminalEvent::Completions(serial) => {
                    if let Some(answer) = session
                        ._integration
                        .completion_answer()
                        .as_deref()
                        .and_then(completion::read_answer)
                    {
                        updates.push(TerminalUpdate::Completions(pane, serial, answer));
                    }
                }
                // The clipboard belongs to the window, so this is carried up
                // rather than answered here. An empty write is dropped: it is
                // what a program clearing its own selection sends, and putting
                // an empty string on the clipboard would silently destroy
                // whatever the person had copied.
                TerminalEvent::ClipboardStore(text) => {
                    if !text.is_empty() {
                        updates.push(TerminalUpdate::ClipboardStore(pane, text));
                    }
                }
                TerminalEvent::Agent(reported) => updates.push(TerminalUpdate::Agent {
                    pane,
                    status: reported.status.into(),
                    title: reported.title,
                    message: reported.message,
                }),
                TerminalEvent::AgentSettled => updates.push(TerminalUpdate::AgentSettled(pane)),
                TerminalEvent::Notification(notification) => {
                    updates.push(TerminalUpdate::Notification {
                        pane,
                        title: notification.title,
                        body: notification.body,
                    });
                }
                TerminalEvent::PullRequest(url) => {
                    updates.push(TerminalUpdate::PullRequest { pane, url });
                }
                // The enum is `#[non_exhaustive]`. A shell asking for something
                // a later version of the emulator learned to report is not an
                // error here; it is a line in the log and a feature to add.
                other => log::debug!("pane {pane:?} reported {other:?}, which nothing acts on"),
            }
        }
        updates.extend(stopped);
        updates.extend(prompted_after_the_events);

        if changed {
            ctx.emit(TerminalUpdate::Repainted(pane));
        }
        self.schedule_long_running(pane, ctx);
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

    /// Draws the frame in which a running command takes the whole pane.
    ///
    /// Nothing else would. A `sleep 5` echoes its own line and then says
    /// nothing for five seconds, so the last frame before the shell goes quiet
    /// is one where the command has been running for a millisecond and the
    /// composer is still up — and it would stay up until the command ended.
    /// One timer per command, armed by the first batch that sees it running.
    fn schedule_long_running(&mut self, pane: PaneId, ctx: &mut ModelContext<Self>) {
        let Some(session) = self.sessions.get_mut(&pane) else {
            return;
        };
        let Some(waiting) = pane_surface::until_long_running(&session.snapshot, Instant::now())
        else {
            // Either nothing is running or the frame that hid the composer has
            // already been drawn. Either way the next command arms a fresh
            // timer.
            session.long_running_timer = false;
            return;
        };
        if session.long_running_timer {
            return;
        }
        session.long_running_timer = true;

        let sleeping = ctx.background().spawn(async move {
            std::thread::sleep(waiting);
        });
        ctx.spawn(sleeping, move |model, (), ctx| {
            if let Some(session) = model.sessions.get_mut(&pane) {
                session.long_running_timer = false;
            }
            // Unconditionally: the block may have finished while this waited,
            // in which case the frame this draws is the one with the finished
            // block in the list. A repaint like any other, so a pane nobody can
            // see is not drawn for this either.
            ctx.emit(TerminalUpdate::Repainted(pane));
        })
        .detach();
    }
}

impl Drop for TerminalModel {
    /// A window that closes takes its shells with it.
    ///
    /// Dropping the sessions alone would not: the reader threads hold the other
    /// half of every terminal, so nothing would be killed until the process
    /// itself went away — which is usually the next instant, and is not the same
    /// promise. Nor would closing them and returning, since the next instant
    /// takes the threads ending them too, so this waits for those as well. The
    /// window's delegate has usually ended every shell and waited for it by
    /// now, as the event loop stopped, and then there is nothing left here to
    /// wait for.
    fn drop(&mut self) {
        let endings = self.close_all();
        self.flusher.stop();
        endings.wait(ENDING_PATIENCE);
    }
}

/// The longest a window that is going waits for its shells to be ended.
///
/// They are ended all at once, so the wait is the slowest one's rather than
/// the sum: 50 ms for a window of shells that go when told, 200 if one ignores
/// the hang-up. A second leaves room for a machine under load and is still
/// short to sit through with the window up and no longer answering. Past it
/// the process ends anyway, and its ptys closing with it hang up whatever is
/// left once more.
pub const ENDING_PATIENCE: Duration = Duration::from_secs(1);

/// The shells of closed panes that are still being ended, each on a thread of
/// its own.
///
/// Ending a shell is a wait. `portable-pty` hangs the child up and then looks
/// at it every 50 ms for 200, sending `SIGKILL` after that, and its first look
/// comes straight after the hang-up, before any child can have acted on it:
/// a shell that goes when told costs 50 ms, and one that ignores it 200. So a
/// closed pane goes from the window at once and its shell is ended here, with
/// the kill and the reap under the terminal's lock as they always were — which
/// is what stops a kill from signalling a process id the system has since
/// handed to somebody else.
///
/// A thread rather than the background pool, for the reason the reader is one:
/// the kill sleeps, and fifteen closes would hold the pool's workers for most
/// of a second, with the chains it is sized for queued behind them. Nothing
/// ticks: a close starts the thread, and the thread ends with its shell.
///
/// What is left to wait for is the process ending. The threads die with it,
/// and a shell that ignores the hang-up would outlive Crook if the thread that
/// was to kill it never had its 200 ms — so a window that is going waits here
/// for all of them at once. See [`TerminalModel::close_all`].
#[derive(Clone, Default)]
pub struct Endings(Arc<EndingState>);

#[derive(Default)]
struct EndingState {
    /// How many shells are being ended.
    running: Mutex<usize>,
    /// Signalled as each one has been.
    ended: Condvar,
}

impl EndingState {
    fn running(&self) -> MutexGuard<'_, usize> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Endings {
    /// Ends `pane`'s shell on a thread of its own, then lets go of its session
    /// and of the scratch its shell was started against.
    fn end(&self, pane: PaneId, shared: Arc<Shared>, integration: shell_integration::Session) {
        let ending = Ending::start(self);
        // Kept back so that a process with no thread left to give still ends
        // the shell, here, the way every close used to.
        let in_place = shared.clone();
        let started = thread::Builder::new()
            .name(format!("crook-pty-end-{pane:?}"))
            .spawn(move || {
                end(pane, &shared);
                // The scratch after the shell, which may have been reading it
                // still; the count last, so that a wait that has returned
                // finds both gone.
                drop(shared);
                drop(integration);
                drop(ending);
            });
        if let Err(error) = started {
            log::error!("could not start a thread to end the shell in pane {pane:?}: {error}");
            end(pane, &in_place);
        }
    }

    /// Waits until every shell being ended has been killed and its reap tried,
    /// for at most `patience`, and says whether they all were.
    ///
    /// Killed is the promise, and all a process on its way out needs: the
    /// escalation has sent its `SIGKILL` by then, which nothing survives.
    /// Reaped is usual and not promised. A child the kernel is finishing a
    /// write for — a shell saving its history to a busy disk — goes once the
    /// write is done, which can be after its ending stopped waiting
    /// `REAP_TIMEOUT` to reap it, and the ending says so in the log.
    pub fn wait(&self, patience: Duration) -> bool {
        let deadline = Instant::now() + patience;
        let mut running = self.0.running();
        while *running > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                log::warn!(
                    "left {} shells to end with the process after waiting {patience:?} for them",
                    *running
                );
                return false;
            }
            running = self
                .0
                .ended
                .wait_timeout(running, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// One shell being ended, counted for [`Endings::wait`] until this is dropped:
/// at the end of its thread, or as the thread unwinds.
struct Ending(Endings);

impl Ending {
    fn start(endings: &Endings) -> Self {
        *endings.0.running() += 1;
        Self(endings.clone())
    }
}

impl Drop for Ending {
    fn drop(&mut self) {
        *self.0.0.running() -= 1;
        self.0.0.ended.notify_all();
    }
}

/// Ends a closed pane's shell and collects its exit status. See [`Endings`].
fn end(pane: PaneId, shared: &Shared) {
    if let Err(error) = shared.lock().kill() {
        log::debug!("the shell in pane {pane:?} could not be ended: {error:#}");
    }
    // A child the escalation had to `SIGKILL` has not gone the moment the
    // signal is sent, and the kill's own last look is usually too soon to reap
    // it. Reaped here rather than left to whatever drops the terminal last,
    // which would kill it again and wait again — on the thread that draws, as
    // often as not.
    if !collect_exit(shared) {
        log::debug!(
            "the shell in pane {pane:?} was killed and still had not gone {REAP_TIMEOUT:?} later"
        );
    }
}

/// A running terminal, as the element that draws it holds it.
///
/// Cheap to clone. Every method takes the terminal's lock for as long as it
/// takes to do one thing and no longer; nothing here is allowed to hold it
/// across a frame, or across a write to the pty.
#[derive(Clone)]
pub struct TerminalHandle(Arc<Shared>);

impl TerminalHandle {
    /// What the pane should draw. Owned data: no lock is held while it is
    /// painted, and the same `Arc` comes back until the content changes.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.0.snapshot()
    }

    /// The commands that have finished in this pane, oldest first.
    ///
    /// Owned for the same reason the snapshot is, and the same `Arc` comes
    /// back until a command ends — so a list that keys its geometry on this
    /// pointer rebuilds nothing on a frame that only moved the live block.
    pub fn blocks(&self) -> Arc<BlockHistory> {
        self.0.blocks()
    }

    /// Runs a composed command line, reporting whether it was queued for the
    /// pty — the line and its Enter together, or neither.
    ///
    /// Not [`Self::write`] with a newline stuck on the end: this is the block
    /// boundary that does not depend on the shell cooperating. The emulator is
    /// told the exact text and the exact moment, under the lock the line is
    /// queued under and so before the reader can parse what the shell echoes,
    /// which is what a shell with no OSC 133 integration can never report and
    /// what a shell that has one can only report as whatever it echoed. A line
    /// that was not queued is not recorded either, so `false` means nothing
    /// happened and the line may be tried again.
    pub fn submit(&self, line: &str) -> bool {
        self.drive(|terminal| {
            terminal.scroll_to_bottom();
            match terminal.submit(line) {
                Ok(()) => true,
                Err(error) => {
                    log::warn!("could not send a line to a shell: {error}");
                    false
                }
            }
        })
    }

    /// Whether the shell has exited, asked of the child now rather than of
    /// the last poll.
    ///
    /// For the command line's `--run`, which types into this shell and waits
    /// for it to go quiet: a shell that has gone is quiet for ever, and the
    /// wait would otherwise run to its timeout for every command still to
    /// be typed. Under the terminal's own lock, like every `try_wait`, so
    /// the exit is cached where the poll that closes the pane reads it.
    pub fn has_exited(&self) -> bool {
        self.drive(|terminal| terminal.try_wait().ok().flatten().is_some())
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
    ///
    /// One that reached the pty is a repaint, said the way the reader says
    /// one: the frame that resized was drawn from the picture before, and a
    /// child that prints nothing in answer — a `sleep`, a quiet `tail -f` —
    /// would leave that picture up, and a find bar over a grid the resize
    /// folded again answering the old one. Not one that failed: that is
    /// tried again on every frame, and a repaint for each try would be a
    /// frame for each.
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
                shared.wake.raise();
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

    /// Sends a key the composer had no use for to the shell's own line
    /// editor, and the keyboard with it, returning whether it produced any
    /// bytes. See [`Terminal::send_key_to_line_editor`].
    ///
    /// Under the one lock, so the snapshot this publishes already says the
    /// line editor has the keyboard: the next key, typed before any frame has
    /// been drawn, is routed against it and goes to the line editor too.
    pub fn send_key_to_line_editor(&self, key: Key, modifiers: Modifiers) -> bool {
        self.drive(|terminal| {
            terminal.scroll_to_bottom();
            match terminal.send_key_to_line_editor(key, modifiers) {
                Ok(sent) => sent,
                Err(error) => {
                    log::debug!("could not send a key to a shell's line editor: {error}");
                    false
                }
            }
        })
    }

    /// Puts pasted text into the shell, returning whether it was queued for
    /// the pty.
    ///
    /// Not `send_key` with the text as keystrokes: [`Terminal::paste`] wraps
    /// it in the bracketed-paste markers when the program has asked for them,
    /// which is how an editor tells a paste from typing and stops
    /// auto-indenting it — and strips the two bytes that could end the bracket
    /// early and run the rest.
    ///
    /// Scrolled to the bottom first, for the same reason typing is: text put
    /// into a program somebody has scrolled away from would land out of sight.
    pub fn paste(&self, text: &str) -> bool {
        self.drive(|terminal| {
            terminal.scroll_to_bottom();
            match terminal.paste(text) {
                Ok(()) => true,
                Err(error) => {
                    log::debug!("could not paste into a shell: {error}");
                    false
                }
            }
        })
    }

    /// Puts pasted text into the program only if it has asked for bracketed
    /// paste, and says which of those happened.
    ///
    /// [`Self::paste`] when it has, from the same path: markers around the
    /// text, and the bytes that could end the bracket taken out. When it has
    /// not, nothing is written — because without the markers every newline
    /// goes as the Enter key, and a message of several lines would arrive as
    /// that many submissions, the first of them half a thought. The caller
    /// has something better to do with the text than that.
    ///
    /// One lock for the question and the write, so a program that turns the
    /// mode off between them cannot be written to as though it had not.
    pub fn paste_bracketed(&self, text: &str) -> BracketedPaste {
        self.drive(|terminal| {
            if !terminal.emulator().bracketed_paste() {
                return BracketedPaste::NotAsked;
            }
            terminal.scroll_to_bottom();
            match terminal.paste(text) {
                Ok(()) => BracketedPaste::Sent,
                Err(error) => {
                    log::debug!("could not paste into a shell: {error}");
                    BracketedPaste::Failed
                }
            }
        })
    }

    /// Tells the program in this pane that the keyboard arrived or left, if it
    /// asked to be told, returning whether anything was sent.
    ///
    /// Not scrolled to the bottom, unlike a key: nobody typed anything, and a
    /// person who scrolled back to read and then switched windows should come
    /// back to what they were reading.
    pub fn send_focus(&self, focused: bool) -> bool {
        self.drive(|terminal| match terminal.send_focus(focused) {
            Ok(sent) => sent,
            Err(error) => {
                log::debug!("could not tell a shell about the keyboard: {error}");
                false
            }
        })
    }

    /// Which mouse reports the program in this pane has asked for.
    ///
    /// The one question a pointer gesture asks before it does anything: with
    /// nothing asked for it is a selection, and with something asked for it
    /// belongs to the program.
    ///
    /// Read off the published snapshot, never through `drive`. Every pointer
    /// move over a pane asks it, and the drive was the terminal's lock and a
    /// snapshot rebuilt on the thread that draws, per move — behind a reader
    /// parsing a flood, up to twelve milliseconds a move. What the published
    /// snapshot cannot have yet is a mode in output parsed since, which a frame
    /// has not shown either: the program asked for the mouse in bytes it has
    /// not been seen to print, and hears the pointer from the frame that draws
    /// them.
    pub fn mouse_modes(&self) -> MouseModes {
        self.0.snapshot().mouse
    }

    /// Sends a pointer gesture, returning whether the program took it.
    ///
    /// Deliberately **not** scrolled to the bottom first, unlike a key press.
    /// A program reading the mouse owns the screen, so there is no scrollback
    /// above it to be pulled away from — and a caller only ever reaches this
    /// with a cell of the viewport it just hit-tested, which a scroll would
    /// invalidate between the two.
    pub fn send_mouse(
        &self,
        kind: MouseEventKind,
        button: Option<MouseButton>,
        row: usize,
        column: usize,
        modifiers: Modifiers,
    ) -> bool {
        self.drive(
            |terminal| match terminal.send_mouse(kind, button, row, column, modifiers) {
                Ok(sent) => sent,
                Err(error) => {
                    log::debug!("could not send a pointer gesture to a shell: {error}");
                    false
                }
            },
        )
    }

    /// Sends the wheel as arrow keys to a full-screen program that asked for
    /// `?1007`, returning whether anything was sent.
    ///
    /// What makes the wheel scroll `less` and `man`, neither of which reports
    /// the mouse. Nothing happens on the primary screen, where the wheel
    /// belongs to the scrollback.
    pub fn send_alternate_scroll(&self, lines: i32) -> bool {
        self.drive(|terminal| match terminal.send_alternate_scroll(lines) {
            Ok(sent) => sent,
            Err(error) => {
                log::debug!("could not send the wheel to a shell: {error}");
                false
            }
        })
    }

    /// Writes text to the shell as if it had been typed, returning whether it
    /// was queued for the pty.
    ///
    /// What the input field sends a finished command line with. The session's
    /// writer holds it until the child reads it, so a line composed before the
    /// shell has finished starting is read when the shell gets there.
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

    /// The whole lines a scroll of `delta` lines should move now, carrying the
    /// fraction left over to the next call.
    ///
    /// A wheel turns in whole lines and this hands them straight back. A
    /// trackpad turns in pixels, which reach the caller as a fraction of a
    /// line, and a fraction rounded to an integer every event is either dropped
    /// (a slow drag, nothing moves) or doubled (`round(0.5)` is a whole line).
    /// Accumulated instead, a drag adds up and a high-resolution wheel keeps
    /// its pace. The remainder lives on the pane, so two panes do not share
    /// one and switching between them does not lurch.
    pub fn take_scroll_lines(&self, delta: f32) -> i32 {
        let carried = f32::from_bits(self.0.scroll_remainder.load(Ordering::Relaxed));
        let (whole, remainder) = whole_lines(carried, delta);
        self.0
            .scroll_remainder
            .store(remainder.to_bits(), Ordering::Relaxed);
        whole
    }

    /// Puts the viewport back on the live output.
    pub fn scroll_to_bottom(&self) {
        self.drive(Terminal::scroll_to_bottom);
    }

    /// Takes it to the oldest line the scrollback still holds.
    ///
    /// Asked of the emulator rather than computed from a delta, because how
    /// far back "the top" is is exactly what the emulator knows and a caller
    /// does not.
    pub fn scroll_to_top(&self) {
        self.drive(|terminal| {
            let history = terminal.history_len();
            terminal.scroll_lines(history as i32);
        });
    }

    /// Forgets the pane's finished commands and its scrollback, keeping the
    /// block that is open, and answers the blocks it let go of.
    ///
    /// Through `drive`, which publishes the cleared list and the
    /// picture of the open block together, and now: the reader that usually
    /// publishes may not run again for hours, and until something did, the
    /// pane would go on drawing every command the clear let go of. The blocks
    /// come back from under the same lock, so a command that finishes while
    /// the clear waits for it is among them rather than lost between a look
    /// at the history and the clear.
    pub fn clear_blocks(&self) -> Vec<Block> {
        self.drive(Terminal::clear_blocks)
    }

    /// Copies rows out of the grid into the store a finished block's rows live
    /// in, numbered from the oldest line of the scrollback.
    ///
    /// What a pane drawing one grid copies a selection out of. The rows a drag
    /// covered may have scrolled out of the viewport since — a snapshot only
    /// ever holds the viewport — so they come back out of the emulator, into
    /// exactly the store the block list already knows how to read. Also
    /// reports the row the answer starts at, which is later than `first` when
    /// the history has already dropped the line it named.
    ///
    /// And what the find bar over such a pane reads the whole scrollback out
    /// of, a few hundred rows a call and off the thread that draws — see
    /// [`crate::pane_find::harvest_grid`] — since the lock this takes is the
    /// one the pty's reader parses under.
    pub fn harvest_rows(&self, first: usize, last: usize) -> (BlockRows, usize) {
        self.0.lock().harvest_rows(first, last)
    }

    /// How many times [`Self::drive`] has run against this session, for a
    /// test that holds a gesture to leaving the lock alone.
    #[cfg(test)]
    pub(crate) fn drives(&self) -> usize {
        self.0.drives.load(Ordering::Relaxed)
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
    ///
    /// Nothing `work` sends the child is written here: it is queued, and the
    /// session's writer thread is woken for it, so a keystroke costs a copy and
    /// a wake and a paste the child is slow to read holds nothing — not this
    /// thread, and not the lock the reader needs. See the module docs, "The
    /// writer".
    fn drive<T>(&self, work: impl FnOnce(&mut Terminal) -> T) -> T {
        #[cfg(test)]
        self.0.drives.fetch_add(1, Ordering::Relaxed);
        let mut terminal = self.0.lock();
        let outcome = work(&mut terminal);
        let snapshot = terminal.snapshot();
        self.0.sync_blocks(&terminal);
        // Installed before the terminal is let go, never after: the reader
        // could otherwise publish a newer snapshot in between and have this
        // older one put back over it. See `Shared::latest` for the lock order.
        *self.0.latest.lock().unwrap_or_else(PoisonError::into_inner) = snapshot;
        drop(terminal);
        outcome
    }
}

/// Everything a pane's shell is started with, worked out on the UI thread
/// from what the model said when the shell was asked for. See
/// [`TerminalModel::launch`].
struct Launch {
    pane: PaneId,
    integration: shell_integration::Options,
    options: TerminalOptions,
    token: Option<String>,
    shell: PathBuf,
    /// See [`TerminalModel::set_environment`].
    #[cfg(test)]
    environment: Vec<(String, String)>,
    /// See [`TerminalModel::set_spawn_cost`].
    #[cfg(test)]
    cost: Duration,
}

/// A shell that has started, with what its session is made of.
struct Opened {
    shared: Arc<Shared>,
    /// The scratch the shell was started against. See [`Session`].
    integration: shell_integration::Session,
    marks: bool,
    token: Option<String>,
    shell: PathBuf,
    /// What its cells were resolved against when it started, which a theme
    /// applied since may have moved on from.
    palette: Palette,
}

/// Starts the shell a [`Launch`] describes and the threads that read and
/// write it, on whichever thread calls this: the UI thread for a pane about
/// to be drawn, and the background pool for the panes behind it. See "What a
/// restore costs" in the module docs.
fn spawn_session(launch: Launch, flusher: &Arc<Flusher>) -> Result<Opened, String> {
    let Launch {
        pane,
        integration,
        mut options,
        token,
        shell,
        ..
    } = launch;
    // The integration writes its stub files before the shell is started and
    // removes them when this value is dropped, so it is moved into the
    // session rather than left to fall out of scope here.
    let integration = shell_integration::Session::open(pane, &integration);
    let marks = integration.marks();
    integration.apply(&mut options);
    #[cfg(test)]
    options.environment.extend(launch.environment);
    if !marks {
        log::debug!(
            "pane {pane:?} is running {:?}, which Crook has no command marks for; \
             its output will be one continuous block",
            integration.shell()
        );
    }
    let palette = options.palette.clone();

    #[cfg(test)]
    thread::sleep(launch.cost);
    let terminal = Terminal::spawn(options).map_err(|error| {
        let reason = format!("{error:#}");
        log::error!("could not open a shell for pane {pane:?}: {reason}");
        reason
    })?;
    let shared = start(terminal, pane, flusher)?;
    Ok(Opened {
        shared,
        integration,
        marks,
        token,
        shell,
        palette,
    })
}

/// A shell being opened on the background pool, as the model and the task
/// opening it both hold it.
///
/// What it settles is who ends a shell whose pane has gone by the time it has
/// started — closed, or its window gone, while the task was waiting on the
/// fork. Either side can get there first: the task with what it opened, the
/// model with word that nobody wants it. Whichever arrives second ends it,
/// through [`Endings`] and under this one lock, so a shell is never left
/// running with nothing reading it and nobody to end it — and a window that
/// is going while one is being spawned counts it among the shells it waits
/// for, so that one which ignores the hang-up does not outlive Crook.
///
/// A task that has not begun when its pane goes spawns nothing, and nothing
/// waits for it: a pool that is shutting down drops the tasks it never ran,
/// and a wait on one of those would last the window's whole patience.
#[derive(Clone, Default)]
struct Opening(Arc<Mutex<OpeningState>>);

#[derive(Default)]
enum OpeningState {
    /// The task is waiting for a worker.
    #[default]
    Queued,
    /// The task is spawning the shell.
    Spawning,
    /// The task has finished, and this is what it came to, for the model to
    /// take in. Boxed, being a terminal's worth of state in an enum whose
    /// other states are a word.
    Opened(Box<Result<Opened, String>>),
    /// Nobody wants it any more.
    ///
    /// Holds a count in the window's [`Endings`] while the task is still
    /// spawning, so that a wait for the window's shells to end waits for this
    /// one too; the task lets go of it once the shell it opened has an ending
    /// counted of its own.
    Abandoned(Option<Ending>),
}

impl Opening {
    fn lock(&self) -> MutexGuard<'_, OpeningState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens the shell with `open`, on the pool, and leaves it for the model
    /// — or ends it, if its pane went while it was being opened.
    fn fill(&self, pane: PaneId, open: impl FnOnce() -> Result<Opened, String>, endings: &Endings) {
        {
            let mut state = self.lock();
            if matches!(*state, OpeningState::Abandoned(_)) {
                return;
            }
            *state = OpeningState::Spawning;
        }
        // Not under the lock, which a close on the thread that draws takes:
        // the spawn can take a quarter of a second.
        let opened = open();
        let mut state = self.lock();
        match &mut *state {
            OpeningState::Abandoned(ending) => {
                if let Ok(opened) = opened {
                    endings.end(pane, opened.shared, opened.integration);
                }
                // After the ending above has counted itself, so that a wait
                // never finds nothing to wait for in between.
                ending.take();
            }
            _ => *state = OpeningState::Opened(Box::new(opened)),
        }
    }

    /// What the task opened, for the model to take in once the task is done
    /// — or `None` when the shell was abandoned, and whoever got there second
    /// has ended it.
    fn take(&self) -> Option<Result<Opened, String>> {
        let mut state = self.lock();
        match std::mem::replace(&mut *state, OpeningState::Abandoned(None)) {
            OpeningState::Opened(opened) => Some(*opened),
            other => {
                *state = other;
                None
            }
        }
    }

    /// Says nobody wants the shell any more, and ends it if it has already
    /// started.
    fn abandon(&self, pane: PaneId, endings: &Endings) {
        let mut state = self.lock();
        match std::mem::replace(&mut *state, OpeningState::Abandoned(None)) {
            // The task finds this before it spawns anything.
            OpeningState::Queued => {}
            OpeningState::Spawning => {
                *state = OpeningState::Abandoned(Some(Ending::start(endings)));
            }
            OpeningState::Opened(opened) => {
                // A shell that could not be started has nothing to end.
                if let Ok(opened) = *opened {
                    endings.end(pane, opened.shared, opened.integration);
                }
            }
            abandoned @ OpeningState::Abandoned(_) => *state = abandoned,
        }
    }
}

/// Shares a terminal between the main thread and the two threads a session
/// runs on: one that reads its pty, and one that writes it.
///
/// Each of those blocks for as long as the child likes — the reader until the
/// child says something, the writer until it reads — so neither may happen on
/// the thread that draws, or under the terminal's lock, which the reader needs
/// to parse what a child waiting on its own output has printed. See the
/// module docs, "The writer".
///
/// The error is the reason the session could not start, for the pane to say.
fn start(
    mut terminal: Terminal,
    pane: PaneId,
    flusher: &Arc<Flusher>,
) -> Result<Arc<Shared>, String> {
    // Taken before the terminal is shared, because each can only be taken
    // once and the threads below are the one owners that may block on them.
    let Some(reader) = terminal.take_reader() else {
        log::error!("the shell for pane {pane:?} has no readable end");
        return Err("the pseudo-terminal has no readable end".to_owned());
    };
    let Some(writer) = terminal.take_writer() else {
        log::error!("the shell for pane {pane:?} has no writable end");
        return Err("the pseudo-terminal has no writable end".to_owned());
    };

    // Detached, and holding nothing of the session's but the queue: it ends
    // when the terminal is dropped, which closes the queue and — on the way —
    // the pty, which is what releases a write the child was never going to
    // read. A child that closes the pty first ends it sooner.
    let started = thread::Builder::new()
        .name(format!("crook-pty-writer-{pane:?}"))
        .spawn(move || {
            if let Err(error) = writer.run() {
                log::debug!("the pty of pane {pane:?} stopped taking input: {error}");
            }
        });
    if let Err(error) = started {
        log::error!("could not start a writer thread for pane {pane:?}: {error}");
        return Err(format!("{error}"));
    }

    let snapshot = terminal.snapshot();
    let shared = Arc::new(Shared {
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
        #[cfg(test)]
        drives: std::sync::atomic::AtomicUsize::new(0),
    });

    // The reader holds a `Weak`, not an `Arc`: a pane whose pty stays open
    // after it is closed — something other than the child is holding it —
    // must not keep the terminal and its scrollback alive behind it. The
    // thread owns the object whose drop would unblock it, and that is a
    // knot that only a weak reference unties.
    let reading = Arc::downgrade(&shared);
    let flusher = flusher.clone();
    let started = thread::Builder::new()
        .name(format!("crook-pty-{pane:?}"))
        .spawn(move || read_loop(&reading, &flusher, reader));
    if let Err(error) = started {
        log::error!("could not start a reader thread for pane {pane:?}: {error}");
        return Err(format!("{error}"));
    }
    Ok(shared)
}

/// The whole lines to move now and the fraction to carry, given the fraction
/// carried from before and a new `delta` of lines.
///
/// Truncates towards zero rather than rounding, so the carry is always what is
/// left of the line and never borrows against the next one.
fn whole_lines(carried: f32, delta: f32) -> (i32, f32) {
    let total = carried + delta;
    let whole = total.trunc();
    (whole as i32, total - whole)
}

/// Everything a reader thread and the main thread share for one terminal.
struct Shared {
    terminal: Mutex<Terminal>,

    /// The most recent snapshot the reader built, so painting never waits on
    /// parsing.
    ///
    /// **Written only while `terminal` is locked**, by the same acquisition
    /// that built the snapshot and synced the block list beside it. There are
    /// two writers, the reader's `publish` and the UI thread's
    /// [`TerminalHandle::drive`], and a writer that installed its snapshot
    /// after letting go of the terminal could be overtaken in that gap: the
    /// other one published a newer snapshot and block list, and the first then
    /// put its older snapshot back beside the newer list. A command that
    /// finished during a resize was painted twice, as its block and again in
    /// the live viewport, and its tab stayed labelled running until the next
    /// key press.
    ///
    /// So the lock order is `terminal`, then this, and it cannot deadlock
    /// because nothing takes the two the other way round. [`Self::snapshot`] is
    /// the one reader, and it holds this slot for a clone and takes nothing
    /// else. Nothing is locked while it is held apart from `terminal`, either:
    /// [`Self::sync_blocks`] has let go of the block list before this is taken,
    /// and the snapshot it replaces is plain data whose drop takes no lock.
    latest: Mutex<Arc<Snapshot>>,

    /// The finished blocks, rebuilt beside every snapshot and for the same
    /// reason. See [`BlockHistory`].
    blocks: Mutex<Arc<BlockHistory>>,

    /// What the child has asked for in output that has been parsed and not yet
    /// published, held back until the picture of that output is.
    ///
    /// An event reaches the main thread in the same publication as the
    /// snapshot and block list of the bytes that carried it, never ahead of
    /// them. `crook pane wait --until finished` is answered on
    /// [`TerminalEvent::CommandFinished`], and the `crook pane blocks` an agent
    /// runs straight after it reads [`Self::blocks`] and [`Self::snapshot`]. An
    /// end handed over as it was parsed, up to a [`PAINT_INTERVAL`] before the
    /// flusher published it, answered the wait while the read still found the
    /// command running and refused it.
    ///
    /// Filled only by the acquisition of `terminal` that parsed the bytes, and
    /// emptied only by the one that publishes them, so what `publish` finds
    /// here is exactly what its snapshot shows. Kept here rather than left in
    /// the emulator because bells and notifications are folded as they arrive
    /// (see [`Self::collect`]), and a program ringing in a loop would otherwise
    /// queue a frame's worth of them.
    unpublished: Mutex<Vec<TerminalEvent>>,

    /// What the child has asked for in output that has been published, and
    /// nobody has looked at yet.
    ///
    /// Filled while `terminal` is held, so two publishers hand their events
    /// over in the order they took their snapshots. The main thread takes it
    /// alone, so the order is `terminal`, then this, as it is for `latest`.
    events: Mutex<Vec<TerminalEvent>>,

    /// The main thread's end of the wake.
    wake: Arc<Wake>,

    /// The grid the pty was last set to, packed as columns then rows.
    ///
    /// Here rather than read back from the terminal so that layout — which asks
    /// on every single frame — can answer "unchanged" without queueing behind a
    /// shell that is mid-burst. Only a resize that actually reached the pty is
    /// recorded, so one that failed is tried again on the next frame.
    grid: AtomicU64,

    /// Whether the last resize failed, so a pty that refuses every one of them
    /// is complained about once rather than sixty times a second.
    resize_failing: AtomicBool,

    /// The fraction of a line a scroll gesture has asked for and not yet been
    /// given, as `f32` bits. A trackpad reports a scroll in pixels and a slow
    /// drag is a few of them an event — less than a line, which rounded to
    /// nothing and was dropped, so the grid did not move until a flick crossed
    /// a whole line at once. Carrying the remainder here makes a slow drag add
    /// up. Touched only by the main thread, from [`TerminalHandle::take_scroll_lines`].
    scroll_remainder: AtomicU32,

    /// The throttle: what has been parsed, and when it was last drawn.
    publish: Mutex<PublishState>,

    /// How many times [`TerminalHandle::drive`] has taken the lock from this
    /// side. See [`TerminalHandle::drives`].
    #[cfg(test)]
    drives: std::sync::atomic::AtomicUsize,
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

    /// Sends the key press that asks an integrated shell for completions.
    fn request_completions(&self) -> bool {
        match self.lock().request_completions() {
            Ok(()) => true,
            Err(error) => {
                log::debug!("could not ask a shell for completions: {error}");
                false
            }
        }
    }

    fn snapshot(&self) -> Arc<Snapshot> {
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn blocks(&self) -> Arc<BlockHistory> {
        self.blocks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Brings the held block list up to date with the emulator's, cloning the
    /// rows of the blocks that are new and the pointers of the ones that are
    /// not.
    ///
    /// Called under the same lock acquisition that builds a snapshot, because
    /// the two are one picture: a frame that had the new block but the old
    /// live-block anchor would paint the same rows twice.
    fn sync_blocks(&self, terminal: &Terminal) {
        let mut held = self.blocks.lock().unwrap_or_else(PoisonError::into_inner);
        let evicted = terminal.blocks_evicted();
        let cleared_at = terminal.blocks_cleared_at();
        let finished = terminal.blocks();
        if held.evicted == evicted && held.cleared_at == cleared_at && held.len() == finished.len()
        {
            return;
        }

        // What the emulator dropped off the front, dropped here too. The rest
        // is still the same list with more on the end, because ids are handed
        // out in order and never reused.
        let dropped = evicted.saturating_sub(held.evicted);
        let mut blocks: Vec<Arc<Block>> = held.iter().skip(dropped).cloned().collect();
        blocks.truncate(finished.len());
        blocks.extend(
            finished
                .iter()
                .skip(blocks.len())
                .map(|block| Arc::new(block.clone())),
        );

        *held = Arc::new(BlockHistory::new(blocks, evicted).cleared_at(cleared_at));
    }

    fn take_events(&self) -> Vec<TerminalEvent> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn is_finished(&self) -> bool {
        self.wake.is_finished()
    }

    /// Parses output into the grid, and does not draw it.
    ///
    /// The lock covers the parse and setting aside what it asked for, and
    /// nothing else; the snapshot it will eventually be drawn from is built
    /// under a second, later acquisition, so a burst never holds the terminal
    /// for longer than one chunk takes to parse.
    fn feed(&self, bytes: &[u8]) {
        let mut terminal = self.lock();
        if let Err(error) = terminal.feed(bytes) {
            log::debug!("could not answer a shell's query: {error}");
        }
        // Set aside, not handed over: the picture of these bytes is not
        // published yet. Under the parse's own acquisition, so a publish that
        // finds them here took its snapshot after they were parsed.
        Self::collect(&self.unpublished, terminal.take_events());
        drop(terminal);

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

    /// Resolves this session's cells through a new palette, and publishes what
    /// that changed.
    ///
    /// Publishing directly rather than marking the session pending: an idle
    /// shell writes nothing, so nothing would ever come along to carry the new
    /// colours to the screen, and a theme that only applied to panes with a
    /// command running would be a theme nobody could see themselves choose.
    fn repaint_in(&self, palette: Palette) {
        self.lock().set_palette(palette);
        self.publish();
    }

    /// Rebuilds the grid, hands it to the main thread with what the child
    /// asked for in it, and wakes it.
    ///
    /// The `Arc` goes into a slot of its own so that painting never waits on
    /// parsing; the emulator returns the very same one when the drawn content
    /// did not change, which is what makes the wake-up free to ignore.
    fn publish(&self) {
        let mut terminal = self.lock();
        let snapshot = terminal.snapshot();
        let events = terminal.take_events();
        self.sync_blocks(&terminal);
        // Under the terminal's lock for the same reason `TerminalHandle::drive`
        // is: a keystroke landing in the gap would otherwise be overwritten by
        // this older snapshot. See `latest` for the lock order.
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = snapshot;
        // Handed over only once the picture they belong to is in place, and
        // before the terminal is let go, so that nothing parsed after this
        // snapshot goes with them. The emulator's own leftovers — a child's
        // exit, found since the last parse — come after what was set aside.
        // See `unpublished`.
        let mut published = std::mem::take(
            &mut *self
                .unpublished
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        published.extend(events);
        Self::collect(&self.events, published);
        drop(terminal);

        self.wake.raise();
    }

    /// Adds `events` to `pending`, folding the ones that repeat.
    ///
    /// Used for both stages, `unpublished` and `events`, so a bell set aside
    /// and one handed over that the main thread has not taken yet are still
    /// one bell.
    fn collect(pending: &Mutex<Vec<TerminalEvent>>, events: Vec<TerminalEvent>) {
        if events.is_empty() {
            return;
        }
        let mut pending = pending.lock().unwrap_or_else(PoisonError::into_inner);
        for event in events {
            // A bell that has not been handed over yet stands for every bell
            // after it: `yes $'\a'` rings thousands of times a second, and each
            // one would reach every plugin watching the pane as a sound of its
            // own, each holding a worker on the shared pool while it plays.
            if matches!(event, TerminalEvent::Bell)
                && pending
                    .iter()
                    .any(|pending| matches!(pending, TerminalEvent::Bell))
            {
                continue;
            }
            // The same for a notification, except that the newer one takes
            // the older one's place: each carries words, and the ones worth
            // a row are the last the program said.
            if matches!(event, TerminalEvent::Notification(_)) {
                pending.retain(|pending| !matches!(pending, TerminalEvent::Notification(_)));
            }
            pending.push(event);
        }
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

/// Collects the child's exit status once its pty has closed, and draws the
/// grid the reader leaves behind.
fn reap(shared: &Shared) {
    collect_exit(shared);

    // One publish rather than one per attempt: the grid did not move while the
    // child was being waited for, and a wake-up per five milliseconds would be
    // a hundred frames nobody asked for. This one ignores the interval, because
    // there is no later batch to fold it into.
    shared.publish_state().pending = false;
    shared.publish();
}

/// Waits for a child that has gone, or been told to, to be reapable, and
/// reaps it. Says whether it stopped for any reason but the time.
///
/// Bounded, because a child that has closed its pty and then refuses to be
/// reaped must not hold a thread — and the status is a nicety, while the pane
/// closing is not.
fn collect_exit(shared: &Shared) -> bool {
    let deadline = Instant::now() + REAP_TIMEOUT;
    loop {
        let reaped = match shared.lock().try_wait() {
            Ok(exit) => exit.is_some(),
            Err(error) => {
                log::debug!("could not collect a shell's exit status: {error:#}");
                true
            }
        };

        if reaped {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(REAP_INTERVAL);
    }
}

/// Whether the pty still has to be told about this size.
///
/// A read rather than a swap, because the size is only recorded once it has
/// actually been sent — see [`TerminalHandle::resize`].
fn needs_resize(grid: &AtomicU64, size: TerminalSize) -> bool {
    grid.load(Ordering::Relaxed) != packed(size)
}

/// Records a size the pty has been set to.
fn record_resize(grid: &AtomicU64, size: TerminalSize) {
    grid.store(packed(size), Ordering::Relaxed);
}

/// A grid as one comparable integer, for the atomic layout asks on every frame.
///
/// The cell size is in it. It was left out once, when the font could not
/// change while the process ran; zooming changes it, and a zoom that happens
/// to leave the column and row counts where they were — a small pane, or one
/// at the floor — then never told the pty: `TIOCGWINSZ`'s pixel size and the
/// answer to `CSI 14 t` kept the cell of the size before, for any program that
/// sizes what it draws by pixels.
fn packed(size: TerminalSize) -> u64 {
    u64::from(size.columns) << 48
        | u64::from(size.rows) << 32
        | u64::from(size.cell_width) << 16
        | u64::from(size.cell_height)
}

/// The terminal palette, in the colours of the theme in force.
///
/// The two defaults matter most: a grid whose background is the pane's own
/// means an untouched screen costs no rectangles at all, and text that matches
/// the rest of the window means a shell does not look pasted into it. The
/// agreement runs the other way too — `body::pane_ground` paints the pane in
/// whatever background the grid resolved — so a shell that sets its own with
/// OSC 11 takes the padding around it along.
///
/// The sixteen ANSI colours come from the theme as well, which is the half of
/// a theme a terminal actually shows: a palette that themed the chrome and
/// left `ls` in xterm red would be a half-applied theme. The cube and the grey
/// ramp above them stay the values every terminal agrees on, because a program
/// asking for colour 137 is asking for a specific colour and not for an
/// opinion.
pub fn crook_palette() -> Palette {
    let theme = theme();
    let colours = theme.terminal;

    let mut ansi = Palette::default().ansi;
    for (entry, colour) in ansi.iter_mut().zip(
        colours
            .normal
            .iter()
            .chain(colours.bright.iter())
            .map(|colour| rgb(*colour)),
    ) {
        *entry = colour;
    }

    Palette {
        ansi,
        foreground: rgb(colours.foreground),
        background: rgb(colours.background),
        cursor: rgb(colours.cursor),
        bright_foreground: rgb(colours.bright[7]),
        // What dim text is drawn in. Derived from the interface's muted text
        // rather than left at the emulator's default, which is a grey Crook
        // does not use.
        dim_foreground: rgb(theme.text_muted),
    }
}

/// What a pane says it is running, for the tab's name and the status line:
/// the open block's command while it runs, and `None` otherwise.
///
/// Trimmed here rather than where the block records it. The block keeps the
/// line exactly as it was typed, so that Copy command and Run again hand
/// back a leading space that keeps a line out of the shell's history — the
/// `-e` line has one, and so does a line typed by somebody who set
/// `HIST_IGNORE_SPACE` for that reason. A name is a different thing: a row
/// titled ` exec htop` is drawn a cell to the right of every other row.
fn running_command(block: &LiveBlock) -> Option<String> {
    let command = block
        .command
        .as_deref()
        .filter(|_| block.state.is_running())?
        .trim();
    (!command.is_empty()).then(|| command.to_owned())
}

/// A theme colour as the emulator spells one. Alpha is dropped: a terminal cell
/// is opaque.
fn rgb(color: Color) -> Rgb {
    Rgb::new(color.r, color.g, color.b)
}

#[cfg(test)]
#[path = "terminal_model_tests.rs"]
mod tests;
