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
//! remembering `history_size()` at capture and subtracting the difference,
//! which is exact for as long as the history keeps growing — and stops being
//! exact the moment the history is full, because alacritty then evicts from
//! the top without counting.
//!
//! Harvesting is what keeps that from mattering. When a block closes its rows
//! are copied out, the grid's history is dropped and the screen above the new
//! block is erased, so the grid holds the open block and nothing else. The
//! history can therefore only fill up when the open block itself is longer
//! than the whole scrollback — at which point "this block starts above the
//! oldest line we have" is not an approximation, it is the truth — and a
//! column change, which reflows every stored line number into meaninglessness,
//! can be answered by looking for the first line with anything on it. See
//! [`BlockTracker::reflowed`].

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{ClearMode, Color, Handler};

use crate::harvest::{self, BlockRows};
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
    pub working_directory: Option<PathBuf>,
    /// When the command started running, which is the `C` mark or the submit.
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
/// it. So the boundary hands over the two facts that are about the *command*
/// rather than about the block: how it went, and how long it took.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    /// The status the shell reported, or `None` when it reported none.
    pub exit: Option<i32>,
    /// How long it ran, measured from the submit rather than from OSC 133 `C`,
    /// for the reason [`Blocks::apply`] times it that way: what a person waited
    /// for started when they pressed Enter.
    ///
    /// `None` when the block was already open when Crook started watching, so
    /// there is no beginning to measure from.
    pub took: Option<Duration>,
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
    use Transition::{Close, Enter, Ignore, Record, Terminate};

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
        //               prompt  right/  prompt  submit  output  command
        //               start   cont.   end             start   finished   exit
        /* AtPrompt   */ [REDRAW, OTHER, Record, SENT,   RUN,    DONE,      Terminate],
        /* Submitted  */ [REDRAW, OTHER, Record, TYPED,  RUN,    DONE,      Terminate],
        /* Executing  */ [NEW,    OTHER, BUSY,   TYPED,  AGAIN,  DONE,      Terminate],
        /* Done       */ [NEW,    OTHER, Record, SENT,   RUN,    DONE,      Terminate],
        /* Unknown    */ [NEW,    OTHER, Record, SENT,   RUN,    DONE,      Terminate],
        /* Terminated */ [GONE,   GONE,  GONE,   GONE,   GONE,   GONE,      GONE],
    ]
};

/// A cell of the grid that keeps naming the same cell as output scrolls under
/// it.
///
/// Alacritty numbers line 0 at the top of the live screen and counts history
/// negatively, so a stored line moves up by one for every line that scrolls
/// off — and it rotates selections for you but not anchors. The correction is
/// the change in `history_size()` since the anchor was read.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Anchor {
    /// A cell, and the history size when it was read.
    At {
        /// Grid line at capture.
        line: i32,
        /// Column at capture.
        column: usize,
        /// `history_size()` at capture.
        history: usize,
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
    fn line<T>(self, term: &Term<T>) -> i32 {
        let grid = term.grid();
        let oldest = -(grid.history_size() as i64);
        let line = match self {
            Self::At { line, history, .. } => {
                let drift = grid.history_size() as i64 - history as i64;
                i64::from(line) - drift
            }
            Self::Top => oldest,
        };
        line.max(oldest) as i32
    }

    /// The cell the anchor names, clamped to one the grid actually has so it
    /// can be handed to `Term`.
    fn point<T>(self, term: &Term<T>) -> Point {
        let grid = term.grid();
        let newest = grid.screen_lines() as i32 - 1;
        let column = match self {
            Self::At { column, .. } => column,
            Self::Top => 0,
        };
        Point::new(
            Line(self.line(term).min(newest)),
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
    fn at_input<T>(term: &Term<T>) -> Self {
        let cursor = &term.grid().cursor;
        let (line, column) = if cursor.input_needs_wrap {
            (cursor.point.line.0 + 1, 0)
        } else {
            (cursor.point.line.0, cursor.point.column.0)
        };
        Self::at(line, column, term)
    }

    /// The cell the anchor names, as a grid line and an *unclamped* column.
    ///
    /// [`Self::point`] clamps both, because what it produces is handed to
    /// `Term`, which indexes a row with it. This does not: the column here is
    /// reported rather than read, and clamping it would turn "the row is full,
    /// the next character goes below" into "the next character goes on top of
    /// the last one".
    fn cell<T>(self, term: &Term<T>) -> (i32, usize) {
        let column = match self {
            Self::At { column, .. } => column,
            Self::Top => 0,
        };
        (self.line(term), column)
    }

    /// An anchor on a named cell, read against the history the grid has now.
    fn at<T>(line: i32, column: usize, term: &Term<T>) -> Self {
        Self::At {
            line,
            column,
            history: term.grid().history_size(),
        }
    }
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
    started_at: Option<Instant>,
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
    last_ignored: Option<IgnoreReason>,
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
                started_at: None,
            },
            finished: Vec::new(),
            evicted: 0,
            last_ignored: None,
        }
    }

    /// The finished blocks, oldest first.
    pub(crate) fn finished(&self) -> &[Block] {
        &self.finished
    }

    /// How many blocks have been dropped off the front to stay under
    /// [`MAX_BLOCKS`].
    pub(crate) fn evicted(&self) -> usize {
        self.evicted
    }

    /// Why the last mark that changed nothing changed nothing.
    pub(crate) fn last_ignored(&self) -> Option<IgnoreReason> {
        self.last_ignored
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
                self.open.top.line(term) + display_offset,
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
                let (line, column) = anchor.cell(term);
                PromptEnd {
                    row: line + display_offset,
                    column,
                }
            });

        LiveBlock {
            id: self.open.id,
            state: self.open.state,
            command: self.open.command.clone(),
            started_at: self.open.started_at,
            top_row,
            bottom_row,
            prompt_end,
        }
    }

    /// Applies an OSC 133 mark that has just been parsed, with the grid
    /// already advanced past it so the cursor is where the mark landed.
    pub(crate) fn mark<T: EventListener>(
        &mut self,
        mark: ShellMark,
        term: &mut Term<T>,
        palette: &Palette,
        working_directory: Option<&Path>,
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
        self.apply(signal, exit, None, term, palette, working_directory)
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
        working_directory: Option<&Path>,
    ) {
        // A submit opens a block; only the shell reporting `D` closes one with
        // a status, so there is never anything here to announce.
        let _ = self.apply(
            Signal::Submit,
            None,
            Some(command),
            term,
            palette,
            working_directory,
        );
    }

    /// Records that the shell exited, closing whatever was open.
    pub(crate) fn exited<T: EventListener>(
        &mut self,
        term: &mut Term<T>,
        palette: &Palette,
        working_directory: Option<&Path>,
    ) {
        // A session ending closes the open block without the shell reporting
        // on it, which is not a command finishing.
        let _ = self.apply(Signal::Exit, None, None, term, palette, working_directory);
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
    pub(crate) fn reflowed<T>(&mut self, term: &Term<T>) {
        let grid = term.grid();
        let oldest = -(grid.history_size() as i32);
        let newest = grid.screen_lines() as i32 - 1;
        let line = (oldest..=newest)
            .find(|line| !is_blank_line(term, *line))
            .unwrap_or_else(|| last_content_line(term) + 1);

        self.open.top = Anchor::at(line, 0, term);
        self.open.command_start = None;
        // And where its output began, for the same reason: a reflow moved
        // every line the anchor was measured against, so the row it names is
        // no longer the row the command started printing on. The block keeps
        // its rows and loses only the boundary inside them, which is the
        // recoverable direction — an entry that cannot say where the output
        // starts offers nothing rather than the wrong half.
        self.open.output_start = None;
    }

    /// Looks the signal up in [`TABLE`] and does what the cell says.
    fn apply<T: EventListener>(
        &mut self,
        signal: Signal,
        exit: Option<i32>,
        command: Option<&str>,
        term: &mut Term<T>,
        palette: &Palette,
        working_directory: Option<&Path>,
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
        // `None`.
        let started_at = self.open.started_at;
        let mut finished = None;

        match TABLE[self.open.state.row()][signal.column()] {
            Transition::Ignore(reason) => {
                self.last_ignored = Some(reason);
                return None;
            }
            // The only signal the table records without moving is the prompt
            // end, and what it records is where the echoed command line
            // starts.
            Transition::Record => self.open.command_start = Some(Anchor::at_input(term)),
            Transition::Enter(state) => {
                // A bare Enter at a prompt submits an empty line, and an empty
                // line is not a command: recording one would give a block a
                // header it never had, and would stop the echoed line being
                // read when a real command follows.
                if let Some(command) = command.filter(|line| !line.trim().is_empty()) {
                    self.open.command = Some(command.to_owned());
                }
                if state == BlockState::Executing && self.open.command.is_none() {
                    self.open.command = self.scrape_command(term);
                }
                // Timed from the submit rather than from `C`, which is not
                // what [`BlockState::is_running`] is: what a person waits for
                // starts when they press Enter, and a shell that took a moment
                // to report the start was busy for that moment too.
                if matches!(state, BlockState::Submitted | BlockState::Executing)
                    && self.open.started_at.is_none()
                {
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
                {
                    self.open.working_directory = working_directory.map(Path::to_path_buf);
                }
                // `C` is the one moment anything knows where the echo of the
                // command ends and the command's own output begins: the shell
                // has finished echoing and has printed nothing yet, so the row
                // it would print on next is the first row of the output. The
                // first one wins — a second `C` for the same command is
                // ignored by the table anyway — and a state entered by any
                // other signal records nothing.
                if state == BlockState::Executing && self.open.output_start.is_none() {
                    self.open.output_start = Some(Anchor::at(cursor_line(term) + 1, 0, term));
                }
                self.open.state = state;
            }
            Transition::Close(state) => {
                self.close(BlockState::Done, exit, term, palette);
                self.reopen(state, term, working_directory);
                // Only the shell saying so. A block also closes when the next
                // prompt arrives or the session ends, and neither of those is
                // a command reporting how it went — announcing them as one
                // would ring a bell for a person opening a tab.
                if signal == Signal::CommandFinished {
                    finished = Some(Finished {
                        exit,
                        took: started_at.map(|at| at.elapsed()),
                    });
                }
            }
            Transition::Terminate => {
                self.close(BlockState::Terminated, exit, term, palette);
                self.reopen(BlockState::Terminated, term, working_directory);
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
        let top = self.open.top.line(term);
        // The alternate screen is not scrollback. A full-screen program redraws
        // its frames in place and nothing there ever scrolled, so a block that
        // is being closed while one is up — the shell was killed under `vim` —
        // takes no rows rather than a picture of somebody else's screen.
        let bottom = if term.mode().contains(TermMode::ALT_SCREEN) {
            top - 1
        } else {
            last_content_line(term)
        };
        let rows = harvest::harvest(term, palette, top, bottom);

        // A shell prints a bare newline between a command finishing and its
        // next prompt, and that leftover is not a block. Neither is the empty
        // one a session opens with. Keeping either would put a row of chrome
        // on screen around nothing.
        let worth_keeping = self.open.command.is_some() || !rows.is_blank();
        if worth_keeping {
            // Where the output starts, as a row of the block rather than a
            // line of the grid: the two differ by exactly the block's top, and
            // the block is the only address that survives the harvest. Beyond
            // the last row it is clamped away entirely rather than clamped to
            // the end — a command that printed nothing has no output to offer,
            // and an empty answer is a truer one than the last row.
            let output_from = self.open.output_start.and_then(|anchor| {
                let from = usize::try_from(anchor.line(term) - top).ok()?;
                (from <= rows.rows()).then_some(from)
            });
            self.finished.push(Block {
                id: self.open.id,
                state,
                command: self.open.command.take(),
                exit,
                working_directory: self.open.working_directory.take(),
                started_at: self.open.started_at,
                finished_at: Some(Instant::now()),
                rows,
                output_from,
            });
            if self.finished.len() > MAX_BLOCKS {
                // A batch at a time, because a `Vec` shifts everything left on
                // a removal from the front: dropping one block per push past
                // the cap turns a forged stream into a quadratic memmove. The
                // list stays contiguous, which is what lets `finished` be a
                // slice the renderer can index.
                let over =
                    (self.finished.len() - MAX_BLOCKS + MAX_BLOCKS / 16).min(self.finished.len());
                self.finished.drain(..over);
                self.evicted += over;
            }
        }
    }

    /// Opens the next block where the last one ended, and drops the history
    /// the harvest just took a copy of.
    ///
    /// Dropping it is what keeps [`Anchor`] exact: the history then holds only
    /// what the open block has pushed off the screen, so it can fill up — the
    /// one thing that makes the arithmetic lossy — only when that block is
    /// longer than the entire scrollback. It also returns the emulator's own
    /// viewport to the live output, which costs nothing once a block list owns
    /// the scrolling: the rows a reader scrolled back to are in the store.
    fn reopen<T: EventListener>(
        &mut self,
        state: BlockState,
        term: &mut Term<T>,
        working_directory: Option<&Path>,
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
            top: Anchor::at(top, 0, term),
            command_start: None,
            output_start: None,
            command: None,
            working_directory: working_directory.map(Path::to_path_buf),
            started_at: None,
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
        let start = self.open.command_start?.point(term);
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

#[cfg(test)]
#[path = "blocks_tests.rs"]
mod tests;
