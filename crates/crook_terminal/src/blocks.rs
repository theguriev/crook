//! Where one command's output ends and the next one's begins.
//!
//! A terminal that only has a grid can say what is on screen but not what any
//! of it *was*. This module gives the emulator the other half: a list of
//! blocks, each one a command, its output and how it ended, and one open block
//! that everything arriving now belongs to.
//!
//! # The rule that shapes everything here
//!
//! **Merging two blocks is recoverable; splitting one is not.** A block that
//! swallowed the tail of the command before it still shows the user every
//! byte, in order, and they can see what happened. A block cut in half loses
//! the connection between a command and its output, and no later mark can put
//! it back. So every doubtful case in the table below resolves to "leave the
//! open block alone", and a mark that would split on thin evidence is ignored
//! with a reason rather than obeyed.
//!
//! # The table is the model
//!
//! [`TABLE`] is the whole state machine: one row per state an open block can
//! be in, one column per thing it can be told, and every cell is either a
//! transition or a named reason for doing nothing. There is no if-chain
//! anywhere else deciding what a mark means. Prompt frameworks redraw the
//! prompt several times per keystroke, shells emit `A` without `B`, hostile
//! output emits all four in the wrong order, and the only way to keep that
//! from becoming a thicket of special cases is to make every combination a
//! cell somebody had to fill in.
//!
//! # Degrading honestly
//!
//! A shell with no integration — an un-warpified `ssh`, a container, a shell
//! Crook has never heard of — emits no marks at all. Then nothing in the table
//! ever fires, the session keeps exactly one open block in
//! [`BlockState::Unknown`], and that block holds everything from the first byte
//! to the last. That is the honest rendering: one continuous stream, no chrome
//! claiming boundaries nobody reported.
//!
//! # Anchors, and why the emulator's history is dropped at every harvest
//!
//! A mark knows the cell it landed on, and a cell drifts: every line that
//! scrolls off the screen moves it up one. [`Anchor`] corrects for that by
//! counting the lines that have scrolled into the history since it was
//! taken. Alacritty's `history_size()` counts them only until the history is
//! full: from then on every line scrolled in evicts one off the top and the
//! size stays put. So the history is never left to fill. Alacritty is given
//! [`HISTORY_HEADROOM`] lines more than the scrollback, and
//! [`BlockTracker::trim_history`] cuts the history back to the scrollback
//! after every slice of output and counts what it cut. The size and that
//! count together are what an anchor follows, and they are exact for as long
//! as no slice scrolls more lines than the headroom holds — which takes more
//! than one line a byte, since the emulator feeds no slice longer than the
//! room left. When one does, every anchor is off by the lines nothing counted,
//! and the open block's pictures go. Under a full-screen program the shell's
//! history can be neither read nor cut back, so the emulator feeds a switch
//! of screens apart from the output before it, and the history is cut back
//! on both sides of the switch. What can still move it out of sight is
//! [`BlockTracker::rows_changed`]'s.
//!
//! A line that has left the history is gone, and an anchor on it reads as the
//! oldest line there is. For a block's first row that is the truth — "this
//! block starts above the oldest line we have" — and for a picture it is a
//! lie, so a picture whose first row has gone goes too: see
//! [`BlockTracker::settle_pictures`].
//!
//! Harvesting keeps the rest of the arithmetic honest. When a block closes its
//! rows are copied out, the grid's history is dropped and the screen above
//! the new block is erased, so the grid holds the open block and nothing
//! else. The history can therefore only fill up when the open block itself
//! is longer than the whole scrollback, and a column change, which reflows
//! every stored line number into meaninglessness, can be answered by looking
//! for the first line with anything on it. See [`BlockTracker::reflowed`]. A
//! picture's anchor has no such answer, and a picture drawn over the wrong
//! rows covers text, so a reflow drops the open block's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{ClearMode, Color, Handler};

use crate::harvest::{self, BlockRows};
use crate::image::ImagePlacement;
use crate::marks::{PromptKind, ShellMark};
use crate::snapshot::Palette;

/// How many finished blocks a pane keeps before the oldest are dropped.
///
/// There is no cap in the standard and no cap in Warp, but OSC 133 is
/// untrusted input: a file full of `D` marks is a block each, and a terminal
/// that grows a `Vec` per line of a hostile `cat` has handed anything that can
/// print a way to exhaust memory. Ten thousand blocks is far past any real
/// session and cheap to hold, because a finished block is text and runs.
const MAX_BLOCKS: usize = 10_000;

/// The longest command line scraped off the screen between `B` and `C`.
///
/// The text is display-only and comes from the stream, so it is capped at
/// something no real command line reaches rather than at whatever the grid
/// happens to hold.
const MAX_SCRAPED_COMMAND: usize = 4096;

/// How many rows a scraped command line may span before it is not believed.
///
/// A pasted command wraps over several rows; nothing wraps over thirty-two.
/// Past that the prompt-end mark is stale rather than the command long.
const MAX_SCRAPED_LINES: i32 = 32;

/// The most pictures may weigh, as the files they arrived as and never less
/// than [`PICTURE_FLOOR`] apiece: the finished blocks' together, and the open
/// block's together. Past it the oldest go first. The finished blocks that
/// lose theirs keep their text, so that a session that `imgcat`s all
/// afternoon holds the pictures a person is still likely to scroll back to
/// and not every one since lunch; the open block loses the ones printed
/// first, which are the furthest up.
pub(crate) const MAX_IMAGE_BYTES: usize = 256 << 20;

/// The least a picture weighs against [`MAX_IMAGE_BYTES`], however small its
/// file.
///
/// What a picture costs beyond its file does not shrink with it: its
/// placement, its anchor, and the copy of both every snapshot of the open
/// block makes. A PNG's header is all this crate reads of one, and a stream
/// of 24-byte headers would otherwise fit eleven million pictures under the
/// budget, every one of them copied into every frame. At this weight it fits
/// sixteen thousand, more than a folder of icons holds, and a photo weighs
/// what its file does.
pub(crate) const PICTURE_FLOOR: usize = 16 << 10;

/// How many lines past the scrollback alacritty's history may grow before
/// alacritty starts evicting from it on its own, uncounted.
///
/// [`BlockTracker::trim_history`] cuts the history back to the scrollback
/// after every slice of output and counts what it cut, which is what keeps an
/// [`Anchor`] exact once the history is full; this is the room a slice
/// scrolls into before then. It costs this many rows of cells more than the
/// scrollback while the history is full, and is kept under the thousand spare
/// rows alacritty holds on to, so the rows a trim cuts are kept for the lines
/// after it rather than freed and allocated again.
pub(crate) const HISTORY_HEADROOM: usize = 256;

/// Identifies one block for as long as the session lives.
///
/// Handed out in order and never reused, so a renderer may key a layout cache
/// on it and a list may notice that the oldest blocks were evicted by watching
/// the first id move.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(u64);

impl BlockId {
    /// The raw number, for a cache key or a log line.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// What is happening in a block.
///
/// The first five are states an open block passes through; a block that has
/// been harvested is always [`Self::Done`] or [`Self::Terminated`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlockState {
    /// A prompt has started and the shell is waiting for a command line. The
    /// prompt itself is part of the block, so this covers both drawing it and
    /// sitting at it.
    AtPrompt,
    /// A command line has been handed to the shell, which has not yet said it
    /// is running. The window where a slow shell still echoes what was typed.
    Submitted,
    /// The command is running and the output belongs to this block.
    Executing,
    /// Nothing is running. Either the command reported completion, or the
    /// block was closed and this is the gap before the next prompt.
    Done,
    /// Nowhere in particular. Where a session starts, and where an
    /// un-integrated shell stays: one open block holding everything, which is
    /// the honest picture when nothing has reported a boundary.
    #[default]
    Unknown,
    /// The shell exited. Absorbing: no mark moves a session out of it, because
    /// anything still printing is a child that outlived its parent.
    Terminated,
}

impl BlockState {
    /// How many states there are, which is the height of [`TABLE`].
    const COUNT: usize = 6;

    /// This state's row in [`TABLE`].
    const fn row(self) -> usize {
        match self {
            Self::AtPrompt => 0,
            Self::Submitted => 1,
            Self::Executing => 2,
            Self::Done => 3,
            Self::Unknown => 4,
            Self::Terminated => 5,
        }
    }

    /// Whether the shell has said a command is running, which is what decides
    /// that the pane should give the block the whole surface and hide the
    /// composer.
    ///
    /// [`Self::Submitted`] is deliberately *not* this. A line has been sent
    /// and nothing has come back, and for a shell that reports no marks —
    /// `ssh` to a host with no integration, a container, a shell Crook has
    /// never heard of — nothing ever will: the block would be "running" for
    /// the rest of the session and would take the composer with it, leaving
    /// no way to type another command. So the running chrome means what it
    /// says: the shell reported `OSC 133;C`.
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Executing)
    }
}

/// One finished command: what was run, what it printed, and how it ended.
///
/// The rows are owned, so a block outlives the lines it came from — the
/// scrollback evicting them, a resize reflowing them and `clear` wiping them
/// all leave it untouched.
#[derive(Clone, Debug)]
pub struct Block {
    /// Its identity for the life of the session.
    pub id: BlockId,
    /// [`BlockState::Done`], or [`BlockState::Terminated`] for the block that
    /// was open when the shell exited.
    pub state: BlockState,
    /// The command line, as the application submitted it or as it was echoed
    /// between the `B` and `C` marks. `None` when neither happened, which is
    /// what an un-integrated shell and a background block both look like.
    ///
    /// Display-only: it comes off the screen, and anything that can print can
    /// put anything there.
    pub command: Option<String>,
    /// The exit status the shell reported, or `None` when it reported none —
    /// a bare `D`, an empty line, a block closed by the next prompt arriving.
    pub exit: Option<i32>,
    /// Where the shell said it was when the block opened, if it had said.
    ///
    /// On the machine [`Self::working_host`] names, which may not be this
    /// one.
    pub working_directory: Option<PathBuf>,
    /// The host the shell named with [`Self::working_directory`], as its
    /// OSC 7 named it: `None` when it named none.
    ///
    /// The directory is a path on that machine. A shell on the far side of an
    /// `ssh` reports one there, and a pane's blocks can come from both sides
    /// of one: the `ssh` itself ran here, and what was typed after it ran
    /// there. Which hosts are this machine is not this crate's to say — see
    /// the application's `tab::Remote`.
    pub working_host: Option<String>,
    /// When the command started running, which is the `C` mark or the submit.
    ///
    /// `None` when nothing ran: an empty line, typed or submitted, which a
    /// shell ends with a bare `D` and no `C`, and what a shell printed on its
    /// own before a prompt.
    pub started_at: Option<Instant>,
    /// When it finished.
    pub finished_at: Option<Instant>,
    /// Everything the block put on screen: its prompt, its command line and
    /// its output, as the grid held them.
    pub rows: BlockRows,
    /// Which row of [`Self::rows`] the command's own output starts on: the
    /// rows before it are the prompt and the line that was echoed at it.
    ///
    /// `None` when nothing said. Only the `C` mark can say — it is the moment
    /// the shell stops echoing and starts printing — so a block that never ran
    /// (`Submitted` for the rest of the session), a shell with no integration
    /// and everything harvested off the alternate screen have no answer here
    /// and are not given a guessed one. Half of a block is not the half a
    /// reader meant, and "the output is everything after the first row" is
    /// wrong for every multi-line prompt there is.
    pub output_from: Option<usize>,
    /// The pictures the block printed, each at a row of [`Self::rows`] —
    /// empty for nearly every block, and emptied for the oldest once the
    /// pictures of every block together are too many to keep. See
    /// [`crate::image`].
    pub images: Vec<ImagePlacement>,
}

impl Block {
    /// Whether the command succeeded, or `None` when no status was reported.
    pub fn succeeded(&self) -> Option<bool> {
        self.exit.map(|status| status == 0)
    }

    /// How long the command ran, or `None` when either end is unknown.
    pub fn duration(&self) -> Option<Duration> {
        let (started, finished) = (self.started_at?, self.finished_at?);
        finished.checked_duration_since(started)
    }
}

/// The cell the shell's prompt finished on, resolved against the viewport the
/// way [`LiveBlock`]'s rows are.
///
/// This is where OSC 133 `B` left the cursor: one column past the last cell
/// the prompt painted, which is the cell the shell would echo the first
/// character of a command line into. A renderer that draws the line being
/// composed *at* this cell puts it where the shell itself would put it.
///
/// `row` may fall outside the viewport for the same reasons
/// [`LiveBlock::top_row`] may, and `column` may be `columns` — one past the
/// last column of the row — when the prompt filled its row exactly, because
/// the cell that is next is then the first of the row below.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PromptEnd {
    /// Viewport row the prompt finished on.
    pub row: i32,
    /// The column the next character the shell prints would land in.
    pub column: usize,
}

/// The block that everything arriving now belongs to, resolved against the
/// viewport the way [`crate::Cursor`] is.
///
/// Its rows are still in the emulator, so it is painted from
/// [`crate::Snapshot`] rather than from a store: rows `top_row..=bottom_row`
/// of the viewport are its own. Both may fall outside the viewport — negative
/// when the block started above it, past the last row when the view is
/// scrolled back — and `bottom_row < top_row` means the block has printed
/// nothing yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveBlock {
    /// Its identity, which changes the moment a new block opens.
    pub id: BlockId,
    /// What is happening in it.
    pub state: BlockState,
    /// The command line, once one is known.
    pub command: Option<String>,
    /// When the command started running. What the "this is long-running, give
    /// it the whole pane" rule is timed from.
    pub started_at: Option<Instant>,
    /// Viewport row its first line is drawn on.
    pub top_row: i32,
    /// Viewport row its last line is drawn on.
    pub bottom_row: i32,
    /// Where this block's prompt ended, or `None` when nothing has said.
    ///
    /// `None` in exactly three cases, and there is no fourth: a prompt that
    /// has not finished — `A` arrived and `B` has not — a session whose shell
    /// reports no marks at all, and the alternate screen, where a full-screen
    /// program owns the grid and the cell a prompt underneath it ended on
    /// means nothing.
    ///
    /// **It is not re-checked against what is on screen now.** A `B` the shell
    /// printed and then scrolled a long way from still reports its cell, moved
    /// with the history the way every other anchor here is; so does one a file
    /// somebody `cat`ted printed. Anything drawing at it has to ask whether
    /// the row is still one this block is showing — see
    /// `block_list::inline_start`, which is the caller this exists for.
    pub prompt_end: Option<PromptEnd>,
    /// The pictures printed in it, at viewport rows the way
    /// [`Self::top_row`] is, oldest first. None on the alternate screen, and
    /// none whose first row has scrolled out of the history: see
    /// [`crate::image`].
    pub images: Vec<ImagePlacement>,
}

impl Default for LiveBlock {
    /// The block a terminal that has seen nothing at all has open: the first
    /// one, in [`BlockState::Unknown`], holding no rows yet.
    fn default() -> Self {
        Self {
            id: BlockId(0),
            state: BlockState::Unknown,
            command: None,
            started_at: None,
            top_row: 0,
            bottom_row: -1,
            prompt_end: None,
            images: Vec::new(),
        }
    }
}

/// Why a mark changed nothing.
///
/// Every ignored mark has one of these rather than being dropped silently,
/// because "my blocks are wrong" is otherwise unanswerable: the reason names
/// which cell of [`TABLE`] declined and therefore which rule to argue with.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum IgnoreReason {
    /// A prompt start arrived while already at a prompt. Prompt frameworks
    /// redraw on every keystroke; obeying each one would open an empty block
    /// per character typed.
    PromptRedraw,
    /// A right, continuation or secondary prompt. None of them is the top of a
    /// block: the first shares a row with the primary prompt and the other two
    /// appear in the middle of a command that is still being typed.
    SecondaryPrompt,
    /// A prompt end arrived while a command was in flight. A real shell cannot
    /// be at a prompt and running something at the same time.
    PromptDuringCommand,
    /// A command line was submitted while one was already in flight.
    SubmitDuringCommand,
    /// A second output start for the same command.
    RepeatedOutputStart,
    /// A mark arrived while the alternate screen was active. Block tracking is
    /// suspended there: a full-screen program owns the whole grid, and a log
    /// viewer showing a file that contains these bytes would otherwise shred
    /// the list.
    AltScreen,
    /// The shell has exited. Nothing reopens a session.
    Terminated,
}

/// A command that has just ended, for somebody outside the block model.
///
/// The block itself keeps all of this and more, but a listener that only wants
/// to know something ended should not have to go looking through the finished
/// list to find out whether the thing it was told about is the last entry in
/// it. So the boundary hands over the facts that are about the *command*
/// rather than about the block: what it was, whether there was one, how it
/// went, and how long it took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    /// The command line, as [`Block::command`] has it — display-only, for the
    /// same reason — or `None` when there was none: an empty line, a
    /// cancelled one, a shell that echoed nothing to read it from.
    pub command: Option<String>,
    /// The status the shell reported, or `None` when it reported none.
    pub exit: Option<i32>,
    /// How long it ran, measured from the submit rather than from OSC 133 `C`,
    /// for the reason [`BlockTracker::apply`] times it that way: what a person waited
    /// for started when they pressed Enter.
    ///
    /// `None` when the block was already open when Crook started watching, so
    /// there is no beginning to measure from, and when nothing ran in it: see
    /// [`Block::started_at`].
    pub took: Option<Duration>,
    /// Whether a command ran behind this boundary: the block has a command
    /// line, or the shell said one started with `C`.
    ///
    /// `false` for the bare `D` a shell sends for a line that ran nothing —
    /// ctrl-c at the prompt, Enter on an empty one. That is a boundary all the
    /// same, and a listener ringing on "the shell said something ended" still
    /// hears it, but one that acts on a command having *run* must not: nothing
    /// moved, nothing changed directory, nothing was started.
    ///
    /// Not the status. bash 3.2 reports a bare `D` for a `( … )` line, which
    /// did run, and its command line is what says so.
    pub ran: bool,
}

/// Where the shell last said it was, as a block is filed under it: the
/// directory, and the host its OSC 7 named with it.
#[derive(Copy, Clone, Debug)]
pub(crate) struct Whereabouts<'a> {
    directory: &'a Path,
    host: Option<&'a str>,
}

impl<'a> Whereabouts<'a> {
    /// The pair, or `None` when the shell has not said where it is: a host
    /// with no directory is nowhere a block can be filed under.
    pub(crate) fn of(directory: Option<&'a Path>, host: Option<&'a str>) -> Option<Self> {
        Some(Self {
            directory: directory?,
            host,
        })
    }
}

/// What the block model is being told. The columns of [`TABLE`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Signal {
    /// OSC 133 `A` for a primary prompt.
    PromptStart,
    /// OSC 133 `A` for a right, continuation or secondary prompt.
    SecondaryPrompt,
    /// OSC 133 `B`.
    PromptEnd,
    /// The application handed a command line to the shell.
    Submit,
    /// OSC 133 `C`.
    OutputStart,
    /// OSC 133 `D`.
    CommandFinished,
    /// The shell exited.
    Exit,
}

impl Signal {
    /// How many signals there are, which is the width of [`TABLE`].
    const COUNT: usize = 7;

    /// This signal's column in [`TABLE`].
    const fn column(self) -> usize {
        match self {
            Self::PromptStart => 0,
            Self::SecondaryPrompt => 1,
            Self::PromptEnd => 2,
            Self::Submit => 3,
            Self::OutputStart => 4,
            Self::CommandFinished => 5,
            Self::Exit => 6,
        }
    }
}

/// What one cell of [`TABLE`] does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Transition {
    /// Keep the state; the signal only records where it landed.
    Record,
    /// Move the open block to another state.
    Enter(BlockState),
    /// Harvest the open block as finished and open a fresh one in this state,
    /// anchored where the signal landed.
    Close(BlockState),
    /// Harvest the open block as [`BlockState::Terminated`] and stay there.
    Terminate,
    /// The shell is drawing the prompt it is sitting at again. Nothing, as
    /// [`IgnoreReason::PromptRedraw`] — unless it is the first prompt start or
    /// end since a column change, and the rows above it are only the wrapped
    /// head of the prompt drawn before the change, reading as that prompt
    /// did. Then the block starts where the prompt is drawn now. See
    /// [`BlockTracker::reflowed`].
    Redraw,
    /// Do nothing, for this reason.
    Ignore(IgnoreReason),
}

/// The state machine, in full.
///
/// Rows are the open block's state in the order of [`BlockState::row`];
/// columns are the signal in the order of [`Signal::column`].
#[rustfmt::skip]
const TABLE: [[Transition; Signal::COUNT]; BlockState::COUNT] = {
    use BlockState as S;
    use IgnoreReason as R;
    use Transition::{Close, Enter, Ignore, Record, Redraw, Terminate};

    // Each cell of the table is one of these. Naming them is what lets the
    // table be read across a row rather than down a column of expressions.
    // A prompt starts a new block; a completion closes one.
    const NEW: Transition = Close(S::AtPrompt);
    const DONE: Transition = Close(S::Done);
    const SENT: Transition = Enter(S::Submitted);
    const RUN: Transition = Enter(S::Executing);
    // A prompt framework redrawing, or a prompt that is not the primary one.
    // A prompt start arriving while a line is in flight is one of these too:
    // every framework with a transient prompt — powerlevel10k's
    // `transient_prompt`, starship's `Enable-Transience` — re-renders `PS1`
    // when the line is accepted and re-emits the `A` and `B` embedded in it,
    // between the submit and `preexec`'s `C`. Obeying that would open an
    // empty, statusless block above every single command. Nothing is lost by
    // declining: a prompt that really is the next one is followed by a `C`,
    // and `Executing`'s own `NEW` closes the block then.
    const REDRAW: Transition = Ignore(R::PromptRedraw);
    // The same at the prompt itself, with one difference: it is also what a
    // shell prints after a column change wrapped the prompt, and then it is
    // the only thing that says where the prompt is. The shell draws the
    // prompt again from the row its cursor was left on, and the half of the
    // old one the reflow pushed above that row is nobody's. Only that half:
    // anything else above the redraw is the block's, and the cell declines.
    const REDRAWN: Transition = Redraw;
    const OTHER: Transition = Ignore(R::SecondaryPrompt);
    // A shell cannot be at a prompt and running a command at the same time,
    // and a second command line cannot start while the first is in flight.
    // `Submitted` is not that: nothing has come back yet, so a prompt end
    // there is a redraw whose command line is worth recording — it is where
    // the echo of what was sent begins.
    const BUSY: Transition = Ignore(R::PromptDuringCommand);
    const TYPED: Transition = Ignore(R::SubmitDuringCommand);
    const AGAIN: Transition = Ignore(R::RepeatedOutputStart);
    const GONE: Transition = Ignore(R::Terminated);

    [
        //               prompt   right/  prompt  submit  output  command
        //               start    cont.   end             start   finished   exit
        /* AtPrompt   */ [REDRAWN, OTHER, Record, SENT,   RUN,    DONE,      Terminate],
        /* Submitted  */ [REDRAW,  OTHER, Record, TYPED,  RUN,    DONE,      Terminate],
        /* Executing  */ [NEW,     OTHER, BUSY,   TYPED,  AGAIN,  DONE,      Terminate],
        /* Done       */ [NEW,     OTHER, Record, SENT,   RUN,    DONE,      Terminate],
        /* Unknown    */ [NEW,     OTHER, Record, SENT,   RUN,    DONE,      Terminate],
        /* Terminated */ [GONE,    GONE,  GONE,   GONE,   GONE,   GONE,      GONE],
    ]
};

/// A cell of the grid that keeps naming the same cell as output scrolls under
/// it.
///
/// Alacritty numbers line 0 at the top of the live screen and counts history
/// negatively, so a stored line moves up by one for every line that scrolls
/// off — and it rotates selections for you but not anchors. The correction is
/// how many lines have scrolled into the history since the anchor was read:
/// see [`scrolled`].
///
/// Every reading takes the lines [`BlockTracker::trim_history`] has cut off
/// the history so far, which is the half of that count alacritty does not
/// keep.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Anchor {
    /// A cell, and how far the grid had scrolled when it was read.
    At {
        /// Grid line at capture.
        line: i32,
        /// Column at capture.
        column: usize,
        /// [`scrolled`] at capture.
        scrolled: u64,
    },
    /// The oldest line the grid still holds. What a block anchors to when it
    /// never saw a mark: the first block of a session, and the single block a
    /// shell reporting no boundaries at all keeps, both of which start at the
    /// first byte that ever arrived.
    Top,
}

impl Anchor {
    /// Which grid line the anchor names now.
    ///
    /// Clamped at the old end only: a line that has scrolled out of the
    /// history reads as the oldest line there is. It is deliberately *not*
    /// clamped at the new end, because a block opens one row past the last row
    /// of the one before it — a row the grid does not have until the shell
    /// prints the newline that scrolls it in. Reporting that row as the
    /// current bottom instead would make an empty block look like it already
    /// held the last row of its predecessor.
    fn line<T>(self, term: &Term<T>, trimmed: u64) -> i32 {
        let oldest = -(term.grid().history_size() as i64);
        let line = self.unclamped(term, trimmed).unwrap_or(oldest);
        line.max(oldest) as i32
    }

    /// The grid line the anchor names now, before [`Self::line`] clamps it to
    /// one the history still holds: above the oldest line once it has scrolled
    /// out. `None` for [`Self::Top`], which names whichever line is oldest.
    fn unclamped<T>(self, term: &Term<T>, trimmed: u64) -> Option<i64> {
        self.counted()
            .map(|line| line - scrolled(term, trimmed) as i64)
    }

    /// The line the anchor names, numbered so that it stays put as the grid
    /// scrolls: the line it names now, plus how far the grid has scrolled
    /// ([`scrolled`]). Two anchors' compare as the lines they name do, however
    /// far apart they were taken. `None` for [`Self::Top`].
    fn counted(self) -> Option<i64> {
        match self {
            Self::At { line, scrolled, .. } => Some(i64::from(line) + scrolled as i64),
            Self::Top => None,
        }
    }

    /// Whether the line the anchor names has scrolled out of the history,
    /// which [`Self::line`] would read as the oldest line there is.
    fn scrolled_out<T>(self, term: &Term<T>, trimmed: u64) -> bool {
        let oldest = -(term.grid().history_size() as i64);
        self.unclamped(term, trimmed)
            .is_some_and(|line| line < oldest)
    }

    /// The cell the anchor names, clamped to one the grid actually has so it
    /// can be handed to `Term`.
    fn point<T>(self, term: &Term<T>, trimmed: u64) -> Point {
        let grid = term.grid();
        let newest = grid.screen_lines() as i32 - 1;
        let column = match self {
            Self::At { column, .. } => column,
            Self::Top => 0,
        };
        Point::new(
            Line(self.line(term, trimmed).min(newest)),
            Column(column.min(grid.columns().saturating_sub(1))),
        )
    }

    /// An anchor on the cell the next character the shell prints will land in.
    ///
    /// The cursor's own cell, except when the cursor is sitting on the last
    /// column with a wrap pending — which is exactly where a prompt that
    /// filled its row leaves it. Alacritty holds the cursor on that column
    /// rather than storing one past the end of the row, so the cell that is
    /// really next is the first of the row below, and an anchor on the cursor
    /// would name the cell the prompt's own last character is in.
    fn at_input<T>(term: &Term<T>, trimmed: u64) -> Self {
        let cursor = &term.grid().cursor;
        let (line, column) = if cursor.input_needs_wrap {
            (cursor.point.line.0 + 1, 0)
        } else {
            (cursor.point.line.0, cursor.point.column.0)
        };
        Self::at(line, column, term, trimmed)
    }

    /// The cell the anchor names, as a grid line and an *unclamped* column.
    ///
    /// [`Self::point`] clamps both, because what it produces is handed to
    /// `Term`, which indexes a row with it. This does not: the column here is
    /// reported rather than read, and clamping it would turn "the row is full,
    /// the next character goes below" into "the next character goes on top of
    /// the last one".
    fn cell<T>(self, term: &Term<T>, trimmed: u64) -> (i32, usize) {
        let column = match self {
            Self::At { column, .. } => column,
            Self::Top => 0,
        };
        (self.line(term, trimmed), column)
    }

    /// An anchor on a named cell, read against how far the grid has scrolled
    /// now.
    fn at<T>(line: i32, column: usize, term: &Term<T>, trimmed: u64) -> Self {
        Self::At {
            line,
            column,
            scrolled: scrolled(term, trimmed),
        }
    }
}

/// How many lines have scrolled into the history, less any taken back out:
/// the ones it holds, which alacritty counts, and the ones `trimmed` off its
/// old end, which it does not.
///
/// An anchor's line moves up by one for every one this goes up, and down by
/// one for every one it goes down, which a taller screen does by pulling lines
/// back out of the history. Erasing the history takes it down by the whole
/// history at once, and the anchors measured across that are found again or
/// carried across rather than followed: see [`BlockTracker::history_cleared`]
/// and [`BlockTracker::clear`].
fn scrolled<T>(term: &Term<T>, trimmed: u64) -> u64 {
    term.grid().history_size() as u64 + trimmed
}

/// The open block's pictures, oldest first, and what they weigh together.
///
/// Each is anchored to the cell its top-left corner is on so it scrolls with
/// the rows it covers; the placement's own row means nothing until it is read
/// against the anchor.
#[derive(Debug, Default)]
struct Pictures {
    /// Each picture under the number it was added with, which only goes up:
    /// oldest first, and found again by its number however many have gone
    /// before or after it.
    placed: BTreeMap<u64, (Anchor, ImagePlacement)>,
    /// Every picture in [`Self::placed`] by the cell its top-left corner is
    /// on: see [`Self::start`].
    ///
    /// What [`Self::place`] finds the pictures a new one may cover in: see
    /// [`Self::cornered_in`]. A picture then costs a look at the few whose
    /// corners are on its own cells, rather than at all the open block
    /// holds, which a stream can make thousands, in whatever order the
    /// stream printed them.
    starts: BTreeSet<(i64, usize, u64)>,
    /// The number the next picture is added with.
    next: u64,
    /// What [`Self::placed`] weighs, kept rather than summed: every picture
    /// added is weighed against it, and a stream can add thousands.
    weight: usize,
}

impl Pictures {
    /// Adds a picture, and lets the oldest go while they weigh more than
    /// [`MAX_IMAGE_BYTES`] together. A picture's file is at most a sixteenth
    /// of that, so the one added stays.
    fn push(&mut self, anchor: Anchor, placement: ImagePlacement) {
        let number = self.next;
        self.next += 1;
        if let Some(start) = Self::start(number, anchor, &placement) {
            self.starts.insert(start);
        }
        self.weight += weight_of(&placement);
        self.placed.insert(number, (anchor, placement));
        while self.weight > MAX_IMAGE_BYTES {
            let Some(oldest) = self.placed.keys().next().copied() else {
                break;
            };
            self.remove(oldest);
        }
    }

    /// Adds a picture as [`Self::push`] does, and first lets go of the ones
    /// it covers every cell of.
    ///
    /// A picture printed over another replaces it, the way text printed over
    /// text does, and the way iTerm2 treats the cells a picture takes. Kept,
    /// the older one would be drawn under the newer, seen only through its
    /// transparent pixels — and a plot redrawn in place, frame over frame,
    /// would pile up a picture for every frame on the same cells, each of them
    /// drawn every time the pane is. One covered only in part stays, since the
    /// rest of it still shows.
    fn place(&mut self, anchor: Anchor, placement: ImagePlacement) {
        let covered: Vec<u64> = self
            .cornered_in(anchor, &placement)
            .filter(|number| {
                self.placed
                    .get(number)
                    .is_some_and(|(older, under)| covers((anchor, &placement), (*older, under)))
            })
            .collect();
        for number in covered {
            self.remove(number);
        }
        self.push(anchor, placement);
    }

    /// The numbers of the pictures whose top-left corner is on a cell that
    /// `placement`, printed at `anchor`, takes. Every picture takes a cell
    /// at least ([`crate::image::cells`]), so these are the only ones it can
    /// cover.
    fn cornered_in(&self, anchor: Anchor, placement: &ImagePlacement) -> impl Iterator<Item = u64> {
        let (left, right) = (placement.column, placement.column + placement.columns);
        let rows = placement.rows as i64;
        anchor.counted().into_iter().flat_map(move |top| {
            (top..top + rows).flat_map(move |line| {
                self.starts
                    .range((line, left, 0)..(line, right, 0))
                    .map(|&(_, _, number)| number)
            })
        })
    }

    /// Lets the picture added with `number` go.
    fn remove(&mut self, number: u64) {
        if let Some((anchor, placement)) = self.placed.remove(&number) {
            self.weight -= weight_of(&placement);
            if let Some(start) = Self::start(number, anchor, &placement) {
                self.starts.remove(&start);
            }
        }
    }

    /// Where [`Self::starts`] files a picture: by the line its first row is
    /// on ([`Anchor::counted`]), then the column its first column is on, and
    /// then its number. `None` for one anchored to [`Anchor::Top`], which
    /// [`covers`] nothing and is covered by nothing.
    fn start(number: u64, anchor: Anchor, placement: &ImagePlacement) -> Option<(i64, usize, u64)> {
        anchor
            .counted()
            .map(|line| (line, placement.column, number))
    }

    /// Keeps the pictures `keep` says to, and lets the rest go.
    fn retain(&mut self, mut keep: impl FnMut(Anchor, &ImagePlacement) -> bool) {
        let (starts, weight) = (&mut self.starts, &mut self.weight);
        self.placed.retain(|number, (anchor, placement)| {
            let kept = keep(*anchor, placement);
            if !kept {
                *weight -= weight_of(placement);
                if let Some(start) = Self::start(*number, *anchor, placement) {
                    starts.remove(&start);
                }
            }
            kept
        });
    }

    /// Lets every picture go.
    fn clear(&mut self) {
        self.placed.clear();
        self.starts.clear();
        self.weight = 0;
    }

    /// Hands every picture over, oldest first, and keeps none.
    fn take(&mut self) -> impl Iterator<Item = (Anchor, ImagePlacement)> + use<> {
        let placed = std::mem::take(&mut self.placed);
        self.clear();
        placed.into_values()
    }

    /// The pictures, oldest first.
    fn iter(&self) -> impl Iterator<Item = &(Anchor, ImagePlacement)> {
        self.placed.values()
    }

    /// Whether there are none.
    fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }
}

/// Whether the picture `over` covers every cell of the picture `under`, each
/// with the anchor it was printed at.
fn covers(over: (Anchor, &ImagePlacement), under: (Anchor, &ImagePlacement)) -> bool {
    let ((at, over), (under_at, under)) = (over, under);
    let (Some(top), Some(under_top)) = (at.counted(), under_at.counted()) else {
        return false;
    };
    under_top >= top
        && under_top + under.rows as i64 <= top + over.rows as i64
        && under.column >= over.column
        && under.column + under.columns <= over.column + over.columns
}

/// The open block: everything known about it before it is harvested.
#[derive(Debug)]
struct OpenBlock {
    id: BlockId,
    state: BlockState,
    /// Its first row.
    top: Anchor,
    /// Where the echoed command line starts, set by the `B` mark.
    command_start: Option<Anchor>,
    /// The first row the command itself printed on, set by the `C` mark.
    ///
    /// An anchor rather than a row number for the reason [`Self::top`] is one:
    /// output scrolling under it moves every grid line, and the row this names
    /// has to be the same row afterwards. It becomes [`Block::output_from`]
    /// when the block is harvested, measured from the block's own first row.
    output_start: Option<Anchor>,
    command: Option<String>,
    working_directory: Option<PathBuf>,
    /// The host the shell named with [`Self::working_directory`].
    working_host: Option<String>,
    started_at: Option<Instant>,
    /// The pictures printed in it.
    pictures: Pictures,
    /// Whether a column change has reflowed the grid since its prompt was
    /// drawn, so that the next prompt start may say where it begins. Read
    /// only at the prompt, by [`Transition::Redraw`], and cleared by the
    /// first prompt start or prompt end after the reflow: whichever arrives
    /// first is the shell's redraw, and anything later is not.
    reflowed: bool,
    /// What the prompt reads as, set by the `B` mark at the prompt: its
    /// first line, from the first row of the block with anything on it to
    /// the prompt end or to the line's own end, as one string however many
    /// rows it was wrapped over. A reflow moves the cells and not what they
    /// say, so this is what tells the head of the old prompt from anything
    /// else a reflow leaves above the shell's redraw. See
    /// [`BlockTracker::only_the_old_prompt_above`].
    prompt: Option<String>,
}

/// The blocks of one pane: the finished ones, and the one still open.
///
/// Lives inside [`crate::Emulator`], which feeds it marks as they are parsed
/// and reads [`Self::live`] when it builds a snapshot.
#[derive(Debug)]
pub(crate) struct BlockTracker {
    next_id: u64,
    open: OpenBlock,
    finished: Vec<Block>,
    evicted: usize,
    /// What [`Self::evicted`] came to at the last [`Self::clear`]. The blocks
    /// counted up to there went because somebody asked for an empty pane,
    /// and leave nothing before the first block held; only the ones counted
    /// past it were pushed off by the cap.
    cleared_at: usize,
    last_ignored: Option<IgnoreReason>,
    /// A reflow that happened under a full-screen program, owed to the
    /// shell's screen for when it comes back. See [`Self::reflowed`].
    reflow_owed: bool,
    /// What the finished blocks' pictures weigh. See [`MAX_IMAGE_BYTES`].
    image_bytes: usize,
    /// How many lines [`Self::trim_history`] has cut off the old end of the
    /// history, which every [`Anchor`] reading takes: alacritty counts the
    /// lines its history holds, and not the ones that left it.
    trimmed: u64,
    /// Whether [`Self::trim_history`] has cut lines off since
    /// [`Self::settle_pictures`] last looked, which is the one way a
    /// picture's first row can leave the history without the picture going
    /// with it.
    trimmed_since_settle: bool,
    /// Whether the shell's history, out of sight under a full-screen
    /// program, may have gone past the end of the headroom: see
    /// [`Self::update_switched_screens`].
    history_unsure: bool,
}

impl BlockTracker {
    /// A pane that has seen nothing yet: one open block, in
    /// [`BlockState::Unknown`], holding whatever arrives.
    pub(crate) fn new() -> Self {
        Self {
            next_id: 1,
            open: OpenBlock {
                id: BlockId(0),
                state: BlockState::Unknown,
                top: Anchor::Top,
                command_start: None,
                output_start: None,
                command: None,
                working_directory: None,
                working_host: None,
                started_at: None,
                pictures: Pictures::default(),
                reflowed: false,
                prompt: None,
            },
            finished: Vec::new(),
            evicted: 0,
            cleared_at: 0,
            reflow_owed: false,
            image_bytes: 0,
            trimmed: 0,
            trimmed_since_settle: false,
            history_unsure: false,
            last_ignored: None,
        }
    }

    /// The finished blocks, oldest first.
    pub(crate) fn finished(&self) -> &[Block] {
        &self.finished
    }

    /// How many blocks have been dropped off the front to stay under
    /// [`MAX_BLOCKS`], or by a [`Self::clear`].
    pub(crate) fn evicted(&self) -> usize {
        self.evicted
    }

    /// What [`Self::evicted`] came to at the last [`Self::clear`], or 0 for a
    /// pane never cleared.
    pub(crate) fn cleared_at(&self) -> usize {
        self.cleared_at
    }

    /// Why the last mark that changed nothing changed nothing.
    pub(crate) fn last_ignored(&self) -> Option<IgnoreReason> {
        self.last_ignored
    }

    /// Tests: how many pictures the open block holds, under a full-screen
    /// program too, where [`Self::live`] shows none of them.
    #[cfg(test)]
    pub(crate) fn open_pictures(&self) -> usize {
        self.open.pictures.iter().count()
    }

    /// The open block, resolved against the viewport.
    pub(crate) fn live<T>(&self, term: &Term<T>) -> LiveBlock {
        let grid = term.grid();
        let alt_screen = term.mode().contains(TermMode::ALT_SCREEN);
        let display_offset = grid.display_offset() as i32;
        let (top_row, bottom_row) = if alt_screen {
            // A full-screen program owns the whole grid and there is no
            // history under it, so the anchor's arithmetic means nothing here.
            (0, grid.screen_lines() as i32 - 1)
        } else {
            (
                self.open.top.line(term, self.trimmed) + display_offset,
                last_content_line(term) + display_offset,
            )
        };

        // The `B` mark's cell, resolved the same way and for the same reason:
        // a caller has viewport rows and the anchor has grid lines. Nothing on
        // the alternate screen, where marks are not believed at all and the
        // anchor left over from the prompt underneath names a cell a
        // full-screen program has since painted over.
        let prompt_end = self
            .open
            .command_start
            .filter(|_| !alt_screen)
            .map(|anchor| {
                let (line, column) = anchor.cell(term, self.trimmed);
                PromptEnd {
                    row: line + display_offset,
                    column,
                }
            });

        let images = if alt_screen {
            Vec::new()
        } else {
            self.open
                .pictures
                .iter()
                .map(|(anchor, placement)| ImagePlacement {
                    row: anchor.line(term, self.trimmed) + display_offset,
                    ..placement.clone()
                })
                .collect()
        };

        LiveBlock {
            id: self.open.id,
            state: self.open.state,
            command: self.open.command.clone(),
            started_at: self.open.started_at,
            top_row,
            bottom_row,
            prompt_end,
            images,
        }
    }

    /// Whether the shell is sitting at a prompt whose end it has marked:
    /// the open block is at a prompt and its `B` has arrived.
    ///
    /// The `B` is the half that matters. A prompt start with no end is a
    /// prompt still being drawn, or a shell that never says where its prompt
    /// ends, and either way [`Self::at_an_empty_prompt`] would have no cell
    /// to ask about.
    pub(crate) fn at_prompt(&self) -> bool {
        self.open.state == BlockState::AtPrompt && self.open.command_start.is_some()
    }

    /// Whether the shell is at its prompt with nothing typed after it: the
    /// next character it prints would land on the cell its `B` mark did, and
    /// that cell is blank.
    ///
    /// The cursor, because a line editor holding a line usually parks it at
    /// the end of the line. Not the cursor alone, because it can park it at
    /// the start instead — `ctrl-a` does — with the whole line after it. And
    /// not every cell after the prompt, because those are not the line's
    /// alone: zsh draws a right prompt on the same row after the `B`, past a
    /// gap, and moves back.
    pub(crate) fn at_an_empty_prompt<T>(&self, term: &Term<T>) -> bool {
        if !self.at_prompt() || term.mode().contains(TermMode::ALT_SCREEN) {
            return false;
        }
        let trimmed = self.trimmed;
        let input = Anchor::at_input(term, trimmed);
        self.open
            .command_start
            .is_some_and(|end| end.cell(term, trimmed) == input.cell(term, trimmed))
            && is_blank_cell(term, input.cell(term, trimmed))
    }

    /// Records a picture printed with its top-left corner on grid `line` at
    /// `column`, into the open block, which lets go of the pictures it covers
    /// every cell of, and of the oldest of the rest once they weigh more than
    /// [`MAX_IMAGE_BYTES`].
    pub(crate) fn image<T>(
        &mut self,
        line: i32,
        column: usize,
        placement: ImagePlacement,
        term: &Term<T>,
    ) {
        let anchor = Anchor::at(line, column, term, self.trimmed);
        self.open.pictures.place(anchor, placement);
    }

    /// Applies an OSC 133 mark that has just been parsed, with the grid
    /// already advanced past it so the cursor is where the mark landed.
    pub(crate) fn mark<T: EventListener>(
        &mut self,
        mark: ShellMark,
        term: &mut Term<T>,
        palette: &Palette,
        whereabouts: Option<Whereabouts<'_>>,
    ) -> Option<Finished> {
        let signal = match mark {
            ShellMark::PromptStart(PromptKind::Initial) => Signal::PromptStart,
            ShellMark::PromptStart(_) => Signal::SecondaryPrompt,
            ShellMark::PromptEnd => Signal::PromptEnd,
            ShellMark::OutputStart => Signal::OutputStart,
            ShellMark::CommandFinished(_) => Signal::CommandFinished,
        };
        let exit = match mark {
            ShellMark::CommandFinished(status) => status,
            _ => None,
        };
        self.apply(signal, exit, None, term, palette, whereabouts)
    }

    /// Records that a command line has been handed to the shell.
    ///
    /// The one boundary that needs no escape sequence and is more reliable
    /// than one: the application knows what it wrote and when.
    pub(crate) fn submitted<T: EventListener>(
        &mut self,
        command: &str,
        term: &mut Term<T>,
        palette: &Palette,
        whereabouts: Option<Whereabouts<'_>>,
    ) {
        // A submit opens a block; only the shell reporting `D` closes one with
        // a status, so there is never anything here to announce.
        let _ = self.apply(
            Signal::Submit,
            None,
            Some(command),
            term,
            palette,
            whereabouts,
        );
    }

    /// Records that the shell exited, closing whatever was open.
    pub(crate) fn exited<T: EventListener>(
        &mut self,
        term: &mut Term<T>,
        palette: &Palette,
        whereabouts: Option<Whereabouts<'_>>,
    ) {
        // A session ending closes the open block without the shell reporting
        // on it, which is not a command finishing.
        let _ = self.apply(Signal::Exit, None, None, term, palette, whereabouts);
    }

    /// Re-finds the open block's first row, because a column change has
    /// reflowed the grid and every line number stored against it.
    ///
    /// Called *after* the resize, because the answer is read off the reflowed
    /// grid: the rows above the open block were erased when the block before
    /// it was harvested, so the first line with anything on it is this
    /// block's own first row. A block that has printed nothing yet anchors
    /// where the next row will be printed instead.
    ///
    /// Anchoring at the oldest line — which is what this used to do — is what
    /// it looks like when that erase is missing: the grid still holds the last
    /// screenful of the block that was harvested out of it, and the open block
    /// claims all of it. The list then paints a screenful of already-stored
    /// output a second time, and, when the reflowed top lands above the
    /// viewport, replaces the whole block list with a plain grid.
    ///
    /// A block's own leading blank rows are lost here — one that began with
    /// two empty lines starts at its third — which is the recoverable
    /// direction: nothing is duplicated and nothing that was printed
    /// disappears.
    ///
    /// The one thing on the grid this cannot see is a prompt being drawn
    /// again. A shell redraws its prompt when the width changes, from the row
    /// its cursor was left on, and when the prompt wrapped, the reflow put the
    /// first half of it a row above that — into the history, for a prompt on
    /// the top row. That half is what this finds, and once the shell has
    /// drawn the whole prompt below it, it is a stale copy: the block starts
    /// above the viewport, which takes the composer away, or paints the half
    /// twice. So the block remembers the reflow, and the prompt start of the
    /// shell's redraw moves it to where the prompt is now — when the rows
    /// above that are the head of the one line the redraw is drawing over,
    /// read as the prompt did, and nothing else. See [`Transition::Redraw`].
    /// A shell that redraws only the last line of a prompt of several — bash
    /// does — prints the prompt end and no prompt start, and leaves the rows
    /// above it where the reflow put them; then what this found is still the
    /// prompt's own first row, and that prompt end is where the block stops
    /// waiting.
    ///
    /// Not while a full-screen program is up. The grid `Term` hands out then
    /// is the program's, so the search would find vim's first row and anchor
    /// the shell's block to it — blank rows kept above the prompt once vim
    /// exits, or a top above the viewport that turns the pane into a plain
    /// grid. The shell's screen is reflowed underneath all the same, and the
    /// search is owed to it: [`Self::settle_reflow`] runs it once that screen
    /// is the one on show again.
    pub(crate) fn reflowed<T>(&mut self, term: &Term<T>) {
        if term.mode().contains(TermMode::ALT_SCREEN) {
            self.reflow_owed = true;
            return;
        }
        self.reflow_owed = false;
        let grid = term.grid();
        let oldest = -(grid.history_size() as i32);
        let newest = grid.screen_lines() as i32 - 1;
        let line = (oldest..=newest)
            .find(|line| !is_blank_line(term, *line))
            .unwrap_or_else(|| last_content_line(term) + 1);

        self.open.top = Anchor::at(line, 0, term, self.trimmed);
        self.open.command_start = None;
        // And where its output began, for the same reason: a reflow moved
        // every line the anchor was measured against, so the row it names is
        // no longer the row the command started printing on. The block keeps
        // its rows and loses only the boundary inside them, which is the
        // recoverable direction — an entry that cannot say where the output
        // starts offers nothing rather than the wrong half.
        self.open.output_start = None;
        // And its pictures, whose rows moved with the rest and cannot be
        // found again: a picture drawn over the wrong text is worse than the
        // gap it leaves.
        self.open.pictures.clear();
        self.open.reflowed = true;
    }

    /// Starts the open block on the row a prompt start has just landed on,
    /// because the shell is drawing its prompt again after a column change.
    ///
    /// Everything above that row is erased and the history dropped, the way
    /// [`Self::reopen`] does it, but with nothing harvested first, because
    /// none of it is anybody's: [`Self::only_the_old_prompt_above`] has
    /// said it is the part of the old prompt the redraw did not paint over,
    /// above rows the reflow found blank. Left there, the next column change
    /// would find it as the block's first line, and a widening one would join
    /// it to the start of the new prompt.
    ///
    /// The prompt's end and any picture go with it, as they do in a reflow,
    /// and for a second reason: their anchors were measured against the
    /// history this drops. The redraw's own `B` records the end again, and
    /// what the prompt reads as with it, since a shell drawing its prompt
    /// again may draw a different one.
    fn redrawn<T: EventListener>(&mut self, term: &mut Term<T>) {
        let line = term.grid().cursor.point.line;
        term.grid_mut().reset_region::<Color, _>(..line);
        term.clear_screen(ClearMode::Saved);
        self.open.top = Anchor::at(line.0, 0, term, self.trimmed);
        self.open.command_start = None;
        self.open.prompt = None;
        self.open.pictures.clear();
        self.open.reflowed = false;
    }

    /// Whether the rows the open block holds above the cursor's are the start
    /// of the prompt it showed before the column change, and nothing else.
    ///
    /// That is what a prompt the reflow wrapped leaves when the shell draws
    /// it again: the shell went back to the start of the row its cursor was
    /// on and draws from there, and the rows above are the head of the line
    /// it is drawing over. They are one line the terminal wrapped into the
    /// cursor's row, and they read as the start of [`OpenBlock::prompt`].
    ///
    /// Anything else there is the block's, and erasing it would lose it:
    /// nothing has harvested it. A job started with `&` writes at the prompt
    /// without the shell knowing, and the shell draws its prompt again below
    /// what the job wrote. Output ending in a newline usually leaves a row
    /// the terminal did not wrap, which ends the line. Output that does not
    /// end in one — or that went over a row already wrapped, which a newline
    /// does not unwrap — is on the prompt's own line: after the prompt's end,
    /// or over its start with a `\r` as a progress meter writes. Then the
    /// rows read as something other than the prompt. So do rows that reach
    /// text typed at the shell's own line editor, and those are kept too: a
    /// stale copy of a line the shell draws again is a smaller fault than
    /// losing one nobody can. What goes is only what the shell's redraw
    /// erases or paints over itself, such as the row its `\r\e[K` clears.
    ///
    /// True when there are no such rows at all, as for a shell that moved up
    /// to its prompt's first row before drawing it again: the rows above the
    /// block are the ones the reflow found blank.
    fn only_the_old_prompt_above<T>(&self, term: &Term<T>) -> bool {
        let grid = term.grid();
        let last = Column(grid.columns().saturating_sub(1));
        let mut head = String::new();
        for line in self.open.top.line(term, self.trimmed)..grid.cursor.point.line.0 {
            let row = &grid[Line(line)];
            if !row[last].flags.contains(Flags::WRAPLINE) {
                return false;
            }
            read_cells(&row[..], &mut head);
        }
        self.open
            .prompt
            .as_deref()
            .unwrap_or_default()
            .starts_with(&head)
    }

    /// What the prompt ending at `end` reads as. See [`OpenBlock::prompt`].
    ///
    /// From the block's first row with anything on it, because that is where
    /// [`Self::reflowed`] finds the block again: the rows a prompt starts
    /// with a blank line on are blank in both.
    fn prompt_text<T>(&self, end: Anchor, term: &Term<T>) -> String {
        let grid = term.grid();
        let (end_line, end_column) = end.cell(term, self.trimmed);
        let first = (self.open.top.line(term, self.trimmed)..end_line)
            .find(|line| !is_blank_line(term, *line))
            .unwrap_or(end_line);
        let mut text = String::new();
        for line in first..=end_line {
            // The prompt end can name the row under a prompt that filled its
            // own, which the grid does not have yet; none of it is read.
            let columns = if line == end_line {
                end_column.min(grid.columns())
            } else {
                grid.columns()
            };
            if columns == 0 {
                break;
            }
            let row = &grid[Line(line)];
            read_cells(&row[..Column(columns)], &mut text);
            if line < end_line && !row[Column(columns - 1)].flags.contains(Flags::WRAPLINE) {
                break;
            }
        }
        text
    }

    /// The scrollback was erased, and the open block now starts at the top of
    /// what is left.
    ///
    /// An anchor corrects for history that grew or was trimmed, and for
    /// nothing else. `clear` erases the display — which moves every row on it
    /// into the history — and then erases the history, so its size ends where
    /// it began and the anchor goes on naming the row the block opened on.
    /// Whatever the command printed after that starts at the top of the
    /// screen, above that row, and `clear; make` was a block that showed none
    /// of `make` and kept none of it when it finished.
    ///
    /// Found again the way [`Self::reflowed`] finds it, as the first row the
    /// grid holds that is not blank: nothing left on it is anybody else's — a
    /// block opens on a grid its predecessor was harvested out of and erased
    /// from, and the history is gone — and the rows that erase left blank
    /// above a prompt are not this block's either. The boundaries inside it
    /// go the same way, since the command line they were measured from may
    /// have been erased with the rest.
    ///
    /// `reset` comes here too. Its `ESC c` empties the history and the
    /// screen, so the anchor names a row below the screen, which output
    /// growing the history again brings back up onto other text.
    pub(crate) fn history_cleared<T>(&mut self, term: &Term<T>) {
        self.reflowed(term);
    }

    /// Drops every finished block and the grid's history, and keeps the open
    /// block on the rows it is on: what clearing a pane by hand does.
    ///
    /// The finished blocks go the way an eviction takes them, off the front
    /// and counted in [`Self::evicted`], so that whatever keeps a copy of the
    /// list — cloned a block at a time, and told what went by that count —
    /// lets them go too. [`Self::cleared_at`] marks where that count stood,
    /// for whoever has to tell blocks a clear let go of from blocks the cap
    /// pushed off: only the second kind leaves something before the first
    /// block. And they are handed back, to a caller with something to keep
    /// of them.
    ///
    /// The history goes as `CSI 3 J` takes it, but the open block is carried
    /// across rather than found again the way [`Self::history_cleared`]
    /// finds it. That one reads a stream which may have erased the screen in
    /// the same breath — `clear` does both — so the rows its anchors named
    /// may be anywhere by then. Here nothing on the screen moves: the rows
    /// below the history keep their lines, and an anchor, which follows its
    /// row by how much the history changed, only has to be told that the
    /// history is empty now. So each one is read before the drop and written
    /// again after it, at the line it named. Found again instead, the block
    /// would forget where its prompt ends, and the composer that continues
    /// the prompt's row would drop to a row of its own until the next prompt.
    ///
    /// An anchor in the history names a row that is gone. The block's top and
    /// the start of its output come to the top of the screen, where what is
    /// left of the block now starts — all of it output, when the start of the
    /// output was up there too. A prompt's end and a picture up there go, as
    /// a reflow loses them.
    ///
    /// The history stays while a full-screen program is up. The one `Term`
    /// hands out then is the program's, and empty; the shell's is underneath,
    /// out of reach, with the anchors measured against it.
    pub(crate) fn clear<T: EventListener>(&mut self, term: &mut Term<T>) -> Vec<Block> {
        let dropped = std::mem::take(&mut self.finished);
        self.evicted += dropped.len();
        self.cleared_at = self.evicted;
        self.image_bytes = 0;

        if term.mode().contains(TermMode::ALT_SCREEN) {
            return dropped;
        }
        // `Top` is the oldest line whatever the history holds, so it needs no
        // carrying: it is the top of the screen once the history is gone.
        let trimmed = self.trimmed;
        let top =
            matches!(self.open.top, Anchor::At { .. }).then(|| self.open.top.line(term, trimmed));
        let command_start = self
            .open
            .command_start
            .map(|anchor| anchor.cell(term, trimmed));
        let output_start = self
            .open
            .output_start
            .map(|anchor| anchor.line(term, trimmed));
        let pictures: Vec<_> = self
            .open
            .pictures
            .take()
            .map(|(anchor, placement)| (anchor.cell(term, trimmed), placement))
            .collect();

        term.clear_screen(ClearMode::Saved);

        if let Some(line) = top {
            self.open.top = Anchor::at(line.max(0), 0, term, trimmed);
        }
        self.open.command_start = command_start
            .filter(|(line, _)| *line >= 0)
            .map(|(line, column)| Anchor::at(line, column, term, trimmed));
        self.open.output_start = output_start.map(|line| Anchor::at(line.max(0), 0, term, trimmed));
        for ((line, column), placement) in pictures {
            if line >= 0 {
                let anchor = Anchor::at(line, column, term, trimmed);
                self.open.pictures.push(anchor, placement);
            }
        }
        dropped
    }

    /// Runs the reflow a full-screen program was up for, if one is owed and
    /// the shell's screen is back.
    pub(crate) fn settle_reflow<T>(&mut self, term: &Term<T>) {
        if self.reflow_owed && !term.mode().contains(TermMode::ALT_SCREEN) {
            self.reflowed(term);
        }
    }

    /// Holds the history at `scrollback_lines`, cutting off its old end
    /// whatever was scrolled in past that and counting it, so that an
    /// [`Anchor`] follows its row past a full history.
    ///
    /// Left to itself, alacritty evicts from a full history without counting,
    /// and its size stops moving: an anchor then stays on the screen row it
    /// named while the text moves on, and a picture drawn there covers
    /// whatever scrolled under it — output prints a picture at the foot of
    /// the screen, so that is the row the prompt comes to, and what is typed
    /// at it. So alacritty is given [`HISTORY_HEADROOM`] lines more, and this
    /// takes them back after every slice of output, which leaves alacritty
    /// nothing to evict.
    ///
    /// Unless a slice scrolled the history to the end of the headroom. Then
    /// lines may have gone that nothing counted, every anchor would be off by
    /// as many, and the open block's pictures go, leaving the blank rows
    /// they were printed over, as a reflow does. A history that has only just
    /// reached the end is still exact, but nothing tells it from one that
    /// went past, so its pictures go as well. The block's own first row and
    /// the boundaries near it are off by as many, but nothing above a block's
    /// first row is kept but the blank rows of a screen it opened on, so on a
    /// screen shorter than the headroom that row has left the history by then
    /// and reads as the oldest line either way.
    ///
    /// Not on the alternate screen, where the grid `Term` hands out is the
    /// program's, which has no history. The shell's is out of reach there,
    /// so the emulator feeds a switch of screens apart from the output before
    /// it, and this cuts the shell's history back right before its screen
    /// goes and right after it is back. Nothing the program prints scrolls
    /// it in between, but for a resize and a synchronized update that
    /// switches screens itself: those are [`Self::rows_changed`]'s.
    ///
    /// Called after every slice of output fed, and after anything else that
    /// can scroll the grid: a resize, a synchronized update let go of.
    pub(crate) fn trim_history<T>(&mut self, term: &mut Term<T>, scrollback_lines: usize) {
        if term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        self.history_unsure = false;
        let history = term.grid().history_size();
        let limit = scrollback_lines.saturating_add(HISTORY_HEADROOM);
        if history >= limit {
            self.open.pictures.clear();
        }
        if history > scrollback_lines {
            let grid = term.grid_mut();
            grid.update_history(scrollback_lines);
            grid.update_history(limit);
            self.trimmed += (history - scrollback_lines) as u64;
            self.trimmed_since_settle = true;
        }
    }

    /// Forgets the open block's pictures whose first row has left the
    /// history.
    ///
    /// An anchor on a line that is gone reads as the oldest line there is,
    /// and a picture drawn there covers whatever is on that line now: its own
    /// lower rows, and then the text that followed it. Without the picture
    /// the block shows the rows it covered that are still there, blank, which
    /// is what a reflow leaves too.
    ///
    /// A line leaves the history only off its old end, which is
    /// [`Self::trim_history`] cutting it, so nothing is looked at until a
    /// trim has happened. Not on the alternate screen, where the grid `Term`
    /// hands out is not the one the anchors are measured against.
    ///
    /// Called after anything that can scroll the grid, before anything that
    /// reads the anchors: [`Self::live`] for a frame, and a mark, which may
    /// harvest the block with its pictures at the rows their anchors name.
    pub(crate) fn settle_pictures<T>(&mut self, term: &Term<T>) {
        if !self.trimmed_since_settle || term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        self.trimmed_since_settle = false;
        let trimmed = self.trimmed;
        self.open
            .pictures
            .retain(|anchor, _| !anchor.scrolled_out(term, trimmed));
    }

    /// Forgets the open block's pictures when the screen has just changed
    /// height under a full-screen program, and the shell's history may have
    /// gone past the end of the headroom there, uncounted.
    ///
    /// `Term` resizes the shell's grid under the program's as well. A
    /// shorter one pushes its top rows into its history, where nothing can
    /// trim them: the history `Term` hands out is the program's. A screen
    /// shorter by more rows than [`HISTORY_HEADROOM`] pushes them past its
    /// end, uncounted, the way [`Self::trim_history`] describes. A taller one
    /// pulls rows back out, which the anchors count, and which also hides a
    /// history that had gone past the end: it is short of the end again by
    /// the time the shell's screen is back, and [`Self::trim_history`] sees
    /// nothing wrong. So a shorter screen takes the pictures whatever it
    /// lost, as a column change does, and a taller one takes them when an
    /// update may have scrolled the shell's history out of sight: see
    /// [`Self::update_switched_screens`].
    pub(crate) fn rows_changed<T>(&mut self, term: &Term<T>, shorter: bool) {
        if term.mode().contains(TermMode::ALT_SCREEN) && (shorter || self.history_unsure) {
            self.open.pictures.clear();
        }
    }

    /// Notes a synchronized update let go of that switched screens.
    ///
    /// An update reaches the grid all at once, so it can scroll the shell's
    /// screen by any number of lines on its way to a program's, or put the
    /// shell's up, scroll it and take it down again, and nothing can cut the
    /// shell's history back in between. Past the end of the headroom, the
    /// lines alacritty evicts go uncounted. A history left there shows it
    /// once the shell's screen is back, unless a taller screen has pulled
    /// lines back out of it first: see [`Self::rows_changed`]. Until
    /// [`Self::trim_history`] next reads it, the shell's history is taken to
    /// be one that may have.
    pub(crate) fn update_switched_screens<T>(&mut self, term: &Term<T>) {
        if term.mode().contains(TermMode::ALT_SCREEN) {
            self.history_unsure = true;
        }
    }

    /// Looks the signal up in [`TABLE`] and does what the cell says.
    fn apply<T: EventListener>(
        &mut self,
        signal: Signal,
        exit: Option<i32>,
        command: Option<&str>,
        term: &mut Term<T>,
        palette: &Palette,
        whereabouts: Option<Whereabouts<'_>>,
    ) -> Option<Finished> {
        // Block tracking is suspended on the alternate screen outright: the
        // marks a TUI emits belong to whatever it is displaying, not to the
        // shell that launched it, and a log viewer showing a file that contains
        // these bytes would otherwise shred the list. The shell exiting is the
        // one signal that still gets through, because a session that has ended
        // has to close its block wherever it ended.
        if signal != Signal::Exit && term.mode().contains(TermMode::ALT_SCREEN) {
            self.last_ignored = Some(IgnoreReason::AltScreen);
            return None;
        }

        // Read before the close takes it: `close` moves `started_at` into the
        // block it files, so a duration read afterwards would always be
        // `None`, and it takes the command line with it.
        let started_at = self.open.started_at;
        let ran = self.open.command.is_some() || self.open.state == BlockState::Executing;
        let mut finished = None;

        match TABLE[self.open.state.row()][signal.column()] {
            Transition::Ignore(reason) => {
                self.last_ignored = Some(reason);
                return None;
            }
            Transition::Redraw if !(self.open.reflowed && self.only_the_old_prompt_above(term)) => {
                self.open.reflowed = false;
                self.last_ignored = Some(IgnoreReason::PromptRedraw);
                return None;
            }
            Transition::Redraw => self.redrawn(term),
            // The only signal the table records without moving is the prompt
            // end, and what it records is where the echoed command line
            // starts, and what the prompt before it says. It also ends the
            // wait for a redraw after a reflow: a shell drawing the whole
            // prompt again has printed its start before this, and one drawing
            // only the last line of it prints no start at all.
            Transition::Record => {
                let end = Anchor::at_input(term, self.trimmed);
                // Only where a redraw reads it. Elsewhere the block's top
                // can be the oldest line there is, and its first line a
                // screenful of output.
                if self.open.state == BlockState::AtPrompt {
                    self.open.prompt = Some(self.prompt_text(end, term));
                }
                self.open.command_start = Some(end);
                self.open.reflowed = false;
            }
            Transition::Enter(state) => {
                // A bare Enter at a prompt submits an empty line, and an empty
                // line is not a command: recording one would give a block a
                // header it never had, and would stop the echoed line being
                // read when a real command follows.
                let sent = command.filter(|line| !line.trim().is_empty());
                if let Some(command) = sent {
                    self.open.command = Some(command.to_owned());
                }
                if state == BlockState::Executing && self.open.command.is_none() {
                    self.open.command = self.scrape_command(term);
                }
                // Timed from the submit rather than from `C`, which is not
                // what [`BlockState::is_running`] is: what a person waits for
                // starts when they press Enter, and a shell that took a moment
                // to report the start was busy for that moment too.
                //
                // Not from an empty one, since nothing starts: the shell ends
                // an empty line with a bare `D` and runs nothing between, so
                // its block has no command and no status, and a start would be
                // the one thing saying a command ran there. Anything counting
                // finished commands reads it that way — a lead asking for a
                // worker's last one among them.
                let starts = match state {
                    BlockState::Submitted => sent.is_some(),
                    BlockState::Executing => true,
                    _ => false,
                };
                if starts && self.open.started_at.is_none() {
                    self.open.started_at = Some(Instant::now());
                }
                // And where it is running, for a block that opened before the
                // shell had said. Every block but the first learns its
                // directory when it opens — the shell reported one at the
                // prompt before it — and the first one of a session opens
                // before a byte has arrived, so it would otherwise be the one
                // block in the list that cannot say where its command ran.
                //
                // Where the command *starts* rather than where it ends: a
                // `cd` inside the command moves the shell, and the answer to
                // "where was this run" is the directory it was typed in.
                if matches!(state, BlockState::Submitted | BlockState::Executing)
                    && self.open.working_directory.is_none()
                    && let Some(whereabouts) = whereabouts
                {
                    self.open.working_directory = Some(whereabouts.directory.to_path_buf());
                    self.open.working_host = whereabouts.host.map(str::to_owned);
                }
                // `C` is the one moment anything knows where the echo of the
                // command ends and the command's own output begins: the shell
                // has finished echoing and has printed nothing yet, so the row
                // it would print on next is the first row of the output. The
                // first one wins — a second `C` for the same command is
                // ignored by the table anyway — and a state entered by any
                // other signal records nothing.
                if state == BlockState::Executing && self.open.output_start.is_none() {
                    let line = cursor_line(term) + 1;
                    self.open.output_start = Some(Anchor::at(line, 0, term, self.trimmed));
                }
                self.open.state = state;
            }
            Transition::Close(state) => {
                // Read before the close files it, as `started_at` is, and only
                // for the close that is told about.
                let command = match signal {
                    Signal::CommandFinished => self
                        .open
                        .command
                        .clone()
                        .filter(|line| !line.trim().is_empty()),
                    _ => None,
                };
                self.close(BlockState::Done, exit, term, palette);
                self.reopen(state, term, whereabouts);
                // Only the shell saying so. A block also closes when the next
                // prompt arrives or the session ends, and neither of those is
                // a command reporting how it went — announcing them as one
                // would ring a bell for a person opening a tab.
                if signal == Signal::CommandFinished {
                    finished = Some(Finished {
                        command,
                        exit,
                        took: started_at.map(|at| at.elapsed()),
                        ran,
                    });
                }
            }
            Transition::Terminate => {
                self.close(BlockState::Terminated, exit, term, palette);
                self.reopen(BlockState::Terminated, term, whereabouts);
            }
        }
        self.last_ignored = None;
        finished
    }

    /// Harvests the open block's rows and files it, unless there is nothing in
    /// it worth keeping.
    fn close<T>(
        &mut self,
        state: BlockState,
        exit: Option<i32>,
        term: &Term<T>,
        palette: &Palette,
    ) {
        let trimmed = self.trimmed;
        let top = self.open.top.line(term, trimmed);
        // The alternate screen is not scrollback. A full-screen program redraws
        // its frames in place and nothing there ever scrolled, so a block that
        // is being closed while one is up — the shell was killed under `vim` —
        // takes no rows rather than a picture of somebody else's screen.
        let bottom = if term.mode().contains(TermMode::ALT_SCREEN) {
            top - 1
        } else {
            last_content_line(term)
        };
        let mut rows = harvest::harvest(term, palette, top, bottom);
        // A block whose rows the grid no longer has — `clear` wiped them, or
        // a program erased the screen on its way out — keeps its command as
        // its one row rather than being a gap with chrome around it. See
        // [`BlockRows::of_line`].
        let mut output_from = None;
        if rows.is_blank()
            && self.open.pictures.is_empty()
            && let Some(command) = &self.open.command
        {
            rows = harvest::BlockRows::of_line(term.columns(), command, palette);
            output_from = Some(Some(rows.rows()));
        }

        // A shell prints a bare newline between a command finishing and its
        // next prompt, and that leftover is not a block. Neither is the empty
        // one a session opens with. Keeping either would put a row of chrome
        // on screen around nothing.
        // A picture on rows with no text is not blank.
        let worth_keeping =
            self.open.command.is_some() || !rows.is_blank() || !self.open.pictures.is_empty();
        if worth_keeping {
            // Where the output starts, as a row of the block rather than a
            // line of the grid: the two differ by exactly the block's top, and
            // the block is the only address that survives the harvest. Beyond
            // the last row it is clamped away entirely rather than clamped to
            // the end — a command that printed nothing has no output to offer,
            // and an empty answer is a truer one than the last row.
            let output_from = output_from.unwrap_or_else(|| {
                self.open.output_start.and_then(|anchor| {
                    let from = usize::try_from(anchor.line(term, trimmed) - top).ok()?;
                    (from <= rows.rows()).then_some(from)
                })
            });
            // As rows of the block, the way the output's start is, and only
            // the ones whose top is a row the block kept.
            let images: Vec<ImagePlacement> = self
                .open
                .pictures
                .take()
                .filter_map(|(anchor, placement)| {
                    let row = anchor.line(term, trimmed) - top;
                    (row >= 0 && (row as usize) < rows.rows())
                        .then_some(ImagePlacement { row, ..placement })
                })
                .collect();
            self.image_bytes += weight(&images);
            self.finished.push(Block {
                id: self.open.id,
                state,
                command: self.open.command.take(),
                exit,
                working_directory: self.open.working_directory.take(),
                working_host: self.open.working_host.take(),
                started_at: self.open.started_at,
                finished_at: Some(Instant::now()),
                rows,
                output_from,
                images,
            });
            if self.finished.len() > MAX_BLOCKS {
                // A batch at a time, because a `Vec` shifts everything left on
                // a removal from the front: dropping one block per push past
                // the cap turns a forged stream into a quadratic memmove. The
                // list stays contiguous, which is what lets `finished` be a
                // slice the renderer can index.
                let over =
                    (self.finished.len() - MAX_BLOCKS + MAX_BLOCKS / 16).min(self.finished.len());
                let evicted: usize = self
                    .finished
                    .drain(..over)
                    .map(|block| weight(&block.images))
                    .sum();
                self.image_bytes -= evicted;
                self.evicted += over;
            }
            // The oldest pictures go first, and the blocks they were in stay.
            let mut oldest = self.finished.iter_mut();
            while self.image_bytes > MAX_IMAGE_BYTES {
                let Some(block) = oldest.next() else { break };
                self.image_bytes -= weight(&block.images);
                block.images = Vec::new();
            }
        }
    }

    /// Opens the next block where the last one ended, and drops the history
    /// the harvest just took a copy of.
    ///
    /// Dropping it means the history then holds only what the open block has
    /// pushed off the screen, so it fills up — where [`Self::trim_history`]
    /// starts cutting lines off, and a block's first row starts to be one the
    /// history no longer has — only when that block is longer than the entire
    /// scrollback. It also returns the emulator's own viewport to the live
    /// output, which costs nothing once a block list owns the scrolling: the
    /// rows a reader scrolled back to are in the store.
    fn reopen<T: EventListener>(
        &mut self,
        state: BlockState,
        term: &mut Term<T>,
        whereabouts: Option<Whereabouts<'_>>,
    ) {
        // Where the shell will print next, which is the cursor's own row when
        // it is at column zero and the row after it otherwise. Not the row
        // after the *harvest*: a program that moved the cursor back up over
        // its own output leaves rows below it that the harvest takes but the
        // shell is about to print over, and anchoring under those would leave
        // the next prompt in a block that says it is empty.
        //
        // It is also a row the grid may not have yet, and `Anchor::line` is
        // careful not to clamp it into one it does.
        let top = cursor_line(term) + 1;

        // The screen the harvest just took a copy of is erased, and the
        // history under it dropped, so that the grid holds the open block and
        // nothing else. That is what keeps every later answer honest: rows
        // below the cursor can then only be the open block's own, and a column
        // change — which reflows every line number stored against the grid —
        // can be answered by looking for the first line with anything on it.
        // See `reflowed` and `last_content_line`.
        //
        // Nothing is lost. A finished block's rows are owned by the store from
        // here on and the list paints them from there; the grid's copy is
        // stale the moment the next command prints over half of it.
        //
        // Not on the alternate screen: a shell killed under `vim` closes its
        // block wherever it ended, and blanking the program's last frame on
        // the way out would be a picture of nothing.
        if !term.mode().contains(TermMode::ALT_SCREEN) {
            term.grid_mut().reset_region::<Color, _>(..);
        }
        term.clear_screen(ClearMode::Saved);
        self.open = OpenBlock {
            id: BlockId(self.next_id),
            state,
            top: Anchor::at(top, 0, term, self.trimmed),
            command_start: None,
            output_start: None,
            command: None,
            working_directory: whereabouts.map(|at| at.directory.to_path_buf()),
            working_host: whereabouts.and_then(|at| at.host).map(str::to_owned),
            started_at: None,
            pictures: Pictures::default(),
            reflowed: false,
            prompt: None,
        };
        self.next_id += 1;
    }

    /// Reads the command line off the screen: what the shell echoed between
    /// the `B` mark and the cursor, which is where `C` just landed.
    ///
    /// The standard has no "here is the command" report — VS Code's `OSC 633`
    /// is a private extension — so this is the only source when the
    /// application did not submit the line itself, as it cannot for anything
    /// typed inside `ssh`, a container or a full-screen editor's shell.
    fn scrape_command<T>(&self, term: &Term<T>) -> Option<String> {
        let start = self.open.command_start?.point(term, self.trimmed);
        let cursor = term.grid().cursor.point;

        // `C` lands after the shell echoed the newline, so the cell before the
        // cursor is the last cell of the command line.
        let end = if cursor.column.0 == 0 {
            Point::new(
                Line(cursor.line.0 - 1),
                Column(term.columns().saturating_sub(1)),
            )
        } else {
            Point::new(cursor.line, Column(cursor.column.0 - 1))
        };
        // A command line that appears to span more of the screen than any real
        // one does means the prompt end is stale — a mark the shell printed
        // and then scrolled far away from, or one a file printed. Reading it
        // would put a screenful of output in the command header, so the block
        // goes without a command instead.
        if end < start
            || end.line.0 < -(term.grid().history_size() as i32)
            || end.line.0 - start.line.0 >= MAX_SCRAPED_LINES
        {
            return None;
        }

        let command = term.bounds_to_string(start, end);
        let command = command.trim();
        let end = command
            .char_indices()
            .map(|(at, character)| at + character.len_utf8())
            .take_while(|at| *at <= MAX_SCRAPED_COMMAND)
            .last()
            .unwrap_or(0);
        (!command.is_empty()).then(|| command[..end].to_owned())
    }
}

/// What `images` weigh, as the files they arrived as: see [`weight_of`].
fn weight(images: &[ImagePlacement]) -> usize {
    images.iter().map(weight_of).sum()
}

/// What one picture weighs against [`MAX_IMAGE_BYTES`]: its file, and never
/// less than [`PICTURE_FLOOR`].
fn weight_of(placement: &ImagePlacement) -> usize {
    placement.image.png().len().max(PICTURE_FLOOR)
}

/// The last grid line with anything on it, given where the cursor is.
///
/// A command whose output ended in a newline leaves the cursor at column 0 of
/// the row *after* the last one it printed, and taking that row would give
/// every block a trailing blank. So the cursor's row counts only when the
/// cursor has moved into it. The answer is one less than the top of the block
/// when the block is empty, which is what makes an empty block detectable.
///
/// Rows *below* the cursor count as well, whenever anything is on them. A
/// progress display that redraws its last three lines with `\e[3A` — `docker
/// compose`, a package manager, an interrupted spinner — leaves the cursor
/// above its own last row, and a block that stopped at the cursor would drop
/// those rows out of the harvest and then hand them to the *next* block,
/// which prints its prompt on top of them. Nothing else can have put anything
/// there: [`BlockTracker::reopen`] erases the screen above an open block, so
/// whatever is below the cursor was printed by the block that is open now.
fn last_content_line<T>(term: &Term<T>) -> i32 {
    let grid = term.grid();
    let at_cursor = cursor_line(term);

    (grid.cursor.point.line.0..grid.screen_lines() as i32)
        .rev()
        .find(|line| !is_blank_line(term, *line))
        .map_or(at_cursor, |line| line.max(at_cursor))
}

/// The last line the cursor itself has put anything on.
///
/// Half of [`last_content_line`], and the whole of where a block opens: the
/// row after this one is where the shell prints next, whatever a program left
/// further down the screen.
fn cursor_line<T>(term: &Term<T>) -> i32 {
    let cursor = term.grid().cursor.point;
    if cursor.column.0 == 0 {
        cursor.line.0 - 1
    } else {
        cursor.line.0
    }
}

/// Whether a grid line has nothing printed on it.
///
/// The characters and nothing else, deliberately: alacritty's own `is_clear`
/// counts a row erased under a background colour as occupied, and the rows
/// above an open block are erased with whatever colour the pen happened to be
/// carrying when the block before them finished.
fn is_blank_line<T>(term: &Term<T>, line: i32) -> bool {
    term.grid()[Line(line)][..].iter().all(|cell| {
        (cell.c == ' ' || cell.c == '\0') && cell.zerowidth().is_none_or(<[char]>::is_empty)
    })
}

/// Whether the cell at grid `line` and `column` has nothing printed in it,
/// in the sense of [`is_blank_line`]. A cell past the last row or column the
/// grid has is one nothing has been printed in yet.
fn is_blank_cell<T>(term: &Term<T>, (line, column): (i32, usize)) -> bool {
    let grid = term.grid();
    if line >= grid.screen_lines() as i32 || column >= grid.columns() {
        return true;
    }
    let cell = &grid[Line(line)][Column(column)];
    (cell.c == ' ' || cell.c == '\0') && cell.zerowidth().is_none_or(<[char]>::is_empty)
}

/// Appends the characters in `cells` to `text`, as a selection of them would
/// copy: the cell a wide character pads its row with is not one.
fn read_cells(cells: &[Cell], text: &mut String) {
    for cell in cells {
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        text.push(cell.c);
        text.extend(cell.zerowidth().unwrap_or_default());
    }
}

#[cfg(test)]
#[path = "blocks_tests.rs"]
mod tests;
