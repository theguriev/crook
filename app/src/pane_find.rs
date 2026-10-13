//! One pane's find bar: whether it is open, what is being looked for, and
//! which match a person is standing on.
//!
//! Per pane, and kept on [`PaneInteraction`](crate::workspace) beside the
//! selection, for the reason the selection is kept there: the element tree
//! that draws the bar is rebuilt every frame, so anything a keystroke changes
//! has to outlive it. It holds the two things that are not derivable from the
//! output — whether the bar is open, and which of however many matches is the
//! current one — and what the query was found to match, kept rather than
//! derived again.
//!
//! # Over the grid
//!
//! A pane drawing one grid — an agent past its first screenful, an ssh
//! session, a shell without the marks, a full-screen program — is searched
//! too, and its answer is kept as well, on its own terms. Its text is not
//! in any snapshot: everything above the viewport is in the emulator's
//! scrollback, behind the lock the pty's reader parses under, ten thousand
//! lines of it by default. Reading all of that back on every frame would hold
//! the lock and the frame for tens of milliseconds, so it is read off the
//! thread that draws, a [`GRID_PIECE`] of rows per hold of the lock, and
//! searched there with the very walk the list is searched with —
//! [`Blocks::find_all_with`] over the grid as a list of one block — so the
//! case switch, the pattern switch and the folding of capitals are the list's
//! own. What comes back is kept until the grid or the question changes: the
//! matches, the question they answer ([`GridAsk`]), and the rows they were
//! found in, so a query typed a letter at a time searches the same rows again
//! rather than reading them out of the emulator once a letter.
//!
//! What counts as the grid changing is its text. A picture that differs from
//! the one the rows were read for only in what [`same_text`] leaves out — the
//! title a working agent animates, the cursor, a colour — is that picture
//! still, and the rows and their answer are carried over to it
//! ([`PaneFind::carry_over`]). A picture whose text did change is read again,
//! but not straight away while an answer for the same question is on screen:
//! a program printing or redrawing would otherwise have the whole scrollback
//! read back to back for as long as the bar is open, so that read waits
//! [`GRID_PAUSE`] first, and the answer on screen stands meanwhile. A question
//! that changed — a letter typed, a switch flipped — is not kept waiting for
//! that pause: it takes the place of a read still waiting one out and starts
//! at once ([`GridPause`]), so it waits at most for a read already under way.
//!
//! The matches are rows of the grid numbered from the oldest line of the
//! scrollback, the numbering a selection on the grid uses, so a viewport that
//! scrolls does not renumber them. Like that selection's, they slide once the
//! scrollback is full and every new line drops the oldest: until the search
//! that the new line's repaint starts has come back, [`GRID_PAUSE`] and a read
//! later, a highlight can sit a few rows from its text.
//!
//! It shares the shape the search box has — a [`TextInput`], a wish to have
//! the keyboard, a hover state — and differs in the one way that matters: the
//! search box has one instance because there is one tab list, and this has one
//! per pane because a person can be searching the output of one pane while
//! another prints. Which pane's bar has the keyboard is the focused pane's,
//! decided in `Workspace::sync_input_keys` exactly as the search box's wish is.
//!
//! # What is kept, and why it cannot go stale
//!
//! The matches are a search of the pane's output, and the output changes under
//! the bar as a command prints. But only one block of it does: a finished
//! block's rows were harvested once and never change, so what a query matched
//! in one is what it will always match there. So [`PaneFind::matches`] keeps
//! the finished blocks' matches from one frame to the next and searches a
//! block once, when it finishes; the open block, the only part a command can
//! still be printing into, is searched again when the snapshot it is drawn
//! from is a new one. A different query or a switch flipped is a different
//! search and starts again; a block dropped off the front of the history takes
//! its matches with it. Walking the whole history for every frame instead cost
//! a tenth of a second a frame on a long-lived tab, for an answer that had not
//! changed.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crook_terminal::{BlockId, BlockRows, Rows, Snapshot};
use crookui_core::elements::MouseStateHandle;

use crate::selection::{
    Blocks, FindOptions, Item, LineMatcher, Place, Region, Selection, grid_first_row, regions_on,
};
use crate::terminal_model::BlockHistory;
use crate::text_input::TextInput;

/// How many rows of a grid are read out of the emulator under one hold of its
/// lock.
///
/// The lock is the one the pty's reader parses under and a keystroke is sent
/// under. Reading a full scrollback of two hundred columns back out in one go
/// holds it for something like a hundred milliseconds; a quarter of a thousand
/// rows at a time holds it for two or three, which neither of them notices.
pub const GRID_PIECE: usize = 256;

/// How long a read of a grid whose text changed under an answer already on
/// screen waits before it starts. See the module docs, "Over the grid".
///
/// Long enough that a program printing or animating costs a read of the
/// scrollback a few times a second rather than one after another, and short
/// enough that the count has caught up by the time anybody looks for it.
pub const GRID_PAUSE: Duration = Duration::from_millis(250);

/// What a search of a pane's grid was asked.
///
/// The question, how it is matched, and which picture of the grid it was asked
/// about: the revision of the snapshot that was on screen, and how wide the
/// grid was. A later revision is a grid that may have printed since, and is
/// asked again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridAsk {
    /// What is being looked for.
    pub query: String,
    /// How it is matched.
    pub options: FindOptions,
    /// The snapshot revision it was asked of.
    pub revision: u64,
    /// The grid's width, in columns, when it was asked. A grid of another
    /// width has been folded again, and its rows renumbered under the answer.
    pub columns: usize,
}

/// What the searches of a pane's grid have left behind. See the module docs,
/// "Over the grid".
#[derive(Default)]
struct GridSearch {
    /// The answer that came back last, and what it was asked.
    found: Option<(GridAsk, Rc<Vec<Selection>>)>,
    /// What the search on its way was asked, while one is.
    asking: Option<GridAsk>,
    /// The pause that search waits out before it reads, while it does: the
    /// search a changed question may take the place of. See [`GridPause`].
    pausing: Option<Pausing>,
    /// The rows the last search read, and the picture of the grid it read
    /// them for.
    rows: Option<(Arc<Snapshot>, Arc<BlockRows>)>,
    /// Whether the current match is to be brought into view once an answer
    /// lands: the bar opened, or stepped, before there was one to go to.
    reveal: bool,
}

/// A read of a grid waiting out [`GRID_PAUSE`] before it starts: the half the
/// worker that is to read holds.
///
/// The pause is for the grid's sake — a program printing without stop — and
/// not the question's, so a question that changes while it runs does not
/// wait it out behind it: it takes the read's place and starts at once (see
/// [`PaneFind::ask_grid`]). The read is spoken for exactly once, by the worker
/// going ahead with it or by the question taking its place, and whichever is
/// first has it: a read already under way is never run beside a second one.
/// Taking its place, or the bar letting go of what its searches kept, also
/// wakes the worker, so a pause nobody wants does not hold one of the pool's
/// threads for the rest of it.
pub struct GridPause {
    /// Whether the read is spoken for.
    claimed: Arc<AtomicBool>,
    /// What ends the wait early: its sender being dropped.
    wakeup: Receiver<()>,
}

impl GridPause {
    /// Waits out `pause` on this thread, reporting whether the read is still
    /// to be made — `false` when a changed question took its place or the bar
    /// let go of it, either of which ends the wait at once.
    pub fn wait(self, pause: Duration) -> bool {
        match self.wakeup.recv_timeout(pause) {
            Err(RecvTimeoutError::Timeout) => !self.claimed.swap(true, Ordering::AcqRel),
            Ok(()) | Err(RecvTimeoutError::Disconnected) => false,
        }
    }
}

/// The bar's half of a [`GridPause`].
struct Pausing {
    /// Whether the read is spoken for, shared with the worker.
    claimed: Arc<AtomicBool>,
    /// Never sent on. Dropped, it wakes the worker.
    wake: Sender<()>,
}

impl Pausing {
    /// Takes the paused read's place, reporting whether it had not begun and
    /// so never will. Wakes the worker either way.
    fn supersede(self) -> bool {
        let taken = !self.claimed.swap(true, Ordering::AcqRel);
        drop(self.wake);
        taken
    }
}

/// One pane's find bar.
#[derive(Clone)]
pub struct PaneFind(Rc<RefCell<State>>);

/// What the bar keeps between frames.
struct State {
    /// Whether it is on screen at all.
    open: bool,
    /// What is being looked for, with its own caret, selection and undo.
    input: TextInput,
    /// What the pointer is doing to the field, so its focus press has
    /// somewhere to remember it was hovered.
    field: MouseStateHandle,
    /// The two buttons that step between matches: previous, then next.
    steps: [MouseStateHandle; 2],
    /// The button that closes the bar.
    close_button: MouseStateHandle,
    /// Which match is current, counted from zero.
    ///
    /// Kept even while the bar is closed and even when there are no matches:
    /// a person who typed a query, stepped to the third match and cleared the
    /// field is at nothing, and the number is meaningless until a match
    /// exists again — at which point [`PaneFind::clamped`] brings it back inside
    /// the count rather than starting over at the top.
    current: usize,
    /// Whether the search tells capitals apart, and whether it reads the
    /// query as a regular expression. Kept with the bar rather than in the
    /// settings: each is a fact about one search, set for it and left with
    /// it, the way an editor's find keeps its own.
    case_sensitive: bool,
    regex: bool,
    /// The switches' hover states: case, regex, and the filter's.
    switches: [MouseStateHandle; 3],
    /// The block whose output the bar is filtering rather than searching,
    /// when it was opened from that block's menu. See [`PaneFind::filter`].
    filter: Option<crook_terminal::BlockId>,
    /// Whether the bar was opened over the alternate screen. See
    /// [`PaneFind::over_alt_screen`].
    alt_screen: bool,
    /// The grid's searches. See the module docs, "Over the grid".
    grid: GridSearch,
    /// The query [`Self::current`] was last counted against. When the query
    /// in the field is no longer this one, the search is a different search
    /// and [`PaneFind::follow_query`] takes the cursor back to the first
    /// match — the way a browser's find does — rather than leaving it
    /// wherever the last query was stepped to.
    last_query: String,
    /// What the query has been found to match so far. See
    /// [`PaneFind::matches`].
    found: Found,
}

impl Default for PaneFind {
    fn default() -> Self {
        Self::new()
    }
}

impl PaneFind {
    /// A pane whose find bar is closed and empty.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(State {
            open: false,
            input: TextInput::new(),
            field: MouseStateHandle::default(),
            steps: Default::default(),
            close_button: MouseStateHandle::default(),
            current: 0,
            last_query: String::new(),
            found: Found::default(),
            case_sensitive: false,
            regex: false,
            switches: Default::default(),
            filter: None,
            alt_screen: false,
            grid: GridSearch::default(),
        })))
    }

    /// Opens the bar over the screen the pane is showing — the alternate one
    /// when `alt_screen` — reporting whether it was closed.
    ///
    /// Does not touch the query: opening the bar a second time on the same
    /// pane comes back to what was being looked for, which is what a person
    /// pressing the chord again to get back to their search means.
    pub fn open(&self, alt_screen: bool) -> bool {
        let mut state = self.0.borrow_mut();
        state.alt_screen = alt_screen;
        !std::mem::replace(&mut state.open, true)
    }

    /// Whether the bar was opened over a full-screen program's screen.
    ///
    /// A bar is about the screen it was opened over. The pane changing screens
    /// under it — a full-screen program starting, or ending — takes that
    /// screen away, and a bar left open would keep Enter and Escape from
    /// whatever has the pane now; the pane closes it instead.
    pub fn over_alt_screen(&self) -> bool {
        self.0.borrow().alt_screen
    }

    /// Closes the bar, reporting whether it was open. Leaves the query where
    /// it was, for the same reason [`Self::open`] does not clear it.
    pub fn close(&self) -> bool {
        let mut state = self.0.borrow_mut();
        // A filter is a way of looking at one block while the bar is up, and
        // it goes down with the bar: the block shows all of itself again.
        state.filter = None;
        // Nothing is drawn from what was found while the bar is down, and the
        // output goes on changing under it; opening it again searches once.
        state.found = Found::default();
        // And what the grid's searches kept, which is a copy of the whole
        // scrollback; an answer still on its way finds nobody asking for it.
        state.grid = GridSearch::default();
        std::mem::replace(&mut state.open, false)
    }

    /// The matches the grid's searches last found for `query`, matched the way
    /// the switches now say, in a grid `columns` wide — or `None` while the
    /// answer to that is still on its way.
    ///
    /// An answer for an older picture of the grid still counts: it is the grid
    /// as it was a moment ago, and the search its repaint started will replace
    /// it. An answer to another question does not, so a query typed a letter
    /// at a time never shows the count of the letters before — and nor does
    /// one found at another width, whose rows a resize has folded again and
    /// numbered differently, so it would wash text it never found.
    pub fn grid_matches(&self, query: &str, columns: usize) -> Option<Rc<Vec<Selection>>> {
        let state = self.0.borrow();
        let options = FindOptions {
            case_sensitive: state.case_sensitive,
            regex: state.regex,
        };
        state
            .grid
            .found
            .as_ref()
            .filter(|(asked, _)| {
                asked.query == query && asked.options == options && asked.columns == columns
            })
            .map(|(_, matches)| matches.clone())
    }

    /// Takes `snapshot` as the picture the kept rows and their answer are of,
    /// when its text is the text of the picture they were read for — see
    /// [`same_text`]. Then the search it would start has nothing to find that
    /// the last one did not, and asking for it finds it answered.
    ///
    /// What keeps a working agent's spinning title, or a cursor that moved,
    /// from reading the whole scrollback once a frame.
    pub fn carry_over(&self, snapshot: &Arc<Snapshot>) {
        let mut state = self.0.borrow_mut();
        let grid = &mut state.grid;
        let Some((read_for, _)) = grid.rows.as_mut() else {
            return;
        };
        if read_for.revision == snapshot.revision || !same_text(read_for, snapshot) {
            return;
        }
        let was = std::mem::replace(read_for, snapshot.clone()).revision;
        // The rows and the answer found in them land together, so the answer
        // is to that picture — unless the question has moved on since, and
        // then it is no answer to carry.
        if let Some((answered, _)) = grid.found.as_mut()
            && answered.revision == was
        {
            answered.revision = snapshot.revision;
        }
    }

    /// Records that the grid is to be searched as `asked` says, reporting
    /// whether a search should start.
    ///
    /// Not when the last answer was to this very question, and not while
    /// another search is on its way: one at a time, because each reads the
    /// whole scrollback, and the one on its way asks again when it lands.
    ///
    /// Except a search still waiting out its pause (see [`GridPause`]). That
    /// one was asked because the grid moved under the same question, and a
    /// question that changed since — another query, a switch, another width —
    /// takes its place and starts now, rather than leaving the count blank
    /// for the rest of a pause that was only ever for the grid's sake. The
    /// grid moving again is not a changed question, and waits for it.
    pub fn ask_grid(&self, asked: &GridAsk) -> bool {
        let mut state = self.0.borrow_mut();
        let grid = &mut state.grid;
        if grid
            .found
            .as_ref()
            .is_some_and(|(answered, _)| answered == asked)
        {
            return false;
        }
        if let Some(asking) = grid.asking.as_ref() {
            let changed = asking.query != asked.query
                || asking.options != asked.options
                || asking.columns != asked.columns;
            if !changed || !grid.pausing.take().is_some_and(Pausing::supersede) {
                return false;
            }
        }
        grid.asking = Some(asked.clone());
        true
    }

    /// Has the search just asked for — [`Self::ask_grid`] said to start it —
    /// wait out [`GRID_PAUSE`] before it reads, and hands back the half of the
    /// pause the worker that reads waits on.
    pub fn pause_grid(&self) -> GridPause {
        let claimed = Arc::new(AtomicBool::new(false));
        let (wake, wakeup) = mpsc::channel();
        self.0.borrow_mut().grid.pausing = Some(Pausing {
            claimed: claimed.clone(),
            wake,
        });
        GridPause { claimed, wakeup }
    }

    /// What the search on its way was asked, and whether it is still waiting
    /// out its pause: for a test to catch one in it.
    #[cfg(test)]
    pub fn grid_asked(&self) -> Option<(String, bool)> {
        let state = self.0.borrow();
        let waiting = state
            .grid
            .pausing
            .as_ref()
            .is_some_and(|pausing| !pausing.claimed.load(Ordering::Acquire));
        state
            .grid
            .asking
            .as_ref()
            .map(|asked| (asked.query.clone(), waiting))
    }

    /// The rows the last search read, when it read them for `revision` —
    /// so that a new question about the same picture searches them again
    /// rather than reading the scrollback out of the emulator once more.
    pub fn grid_rows(&self, revision: u64) -> Option<Arc<BlockRows>> {
        self.0
            .borrow()
            .grid
            .rows
            .as_ref()
            .filter(|(read_for, _)| read_for.revision == revision)
            .map(|(_, rows)| rows.clone())
    }

    /// Takes in what a search of the grid found in `rows`, read for the
    /// picture `read_for`, reporting whether it was the one being waited for.
    ///
    /// One that is not — the bar closed since it set out, or the pane went
    /// back to its list of commands — is dropped: nobody is asking any more.
    pub fn land_grid(
        &self,
        asked: GridAsk,
        read_for: Arc<Snapshot>,
        rows: Arc<BlockRows>,
        matches: Vec<Selection>,
    ) -> bool {
        let mut state = self.0.borrow_mut();
        let grid = &mut state.grid;
        if grid.asking.as_ref() != Some(&asked) {
            return false;
        }
        grid.asking = None;
        grid.pausing = None;
        grid.rows = Some((read_for, rows));
        grid.found = Some((asked, Rc::new(matches)));
        true
    }

    /// Asks for the current match to be brought into view when the grid's
    /// next answer lands.
    pub fn reveal_when_found(&self) {
        self.0.borrow_mut().grid.reveal = true;
    }

    /// Whether the current match was asked to be brought into view, which
    /// asking answers.
    pub fn take_reveal(&self) -> bool {
        std::mem::take(&mut self.0.borrow_mut().grid.reveal)
    }

    /// Lets go of everything the grid's searches kept.
    ///
    /// For a pane that has gone back to its list of commands, which has no use
    /// for a copy of the scrollback, and whose next grid is another picture.
    pub fn forget_grid(&self) {
        self.0.borrow_mut().grid = GridSearch::default();
    }

    /// The block this bar is filtering, when it is filtering one.
    ///
    /// Opened from a block's menu, the bar keeps that block's command line
    /// and only the lines of its output that hold the query, rather than
    /// highlighting matches across every block — Warp's block filter, with
    /// the find's own field and switches.
    pub fn filter(&self) -> Option<crook_terminal::BlockId> {
        self.0.borrow().filter
    }

    /// Starts filtering `block`, or — with `None` — goes back to finding.
    pub fn set_filter(&self, block: Option<crook_terminal::BlockId>) {
        let mut state = self.0.borrow_mut();
        state.filter = block;
        state.current = 0;
    }

    /// Whether the bar is on screen.
    pub fn is_open(&self) -> bool {
        self.0.borrow().open
    }

    /// The editor behind the field.
    pub fn input(&self) -> TextInput {
        self.0.borrow().input.clone()
    }

    /// The field's hover state.
    pub fn field(&self) -> MouseStateHandle {
        self.0.borrow().field.clone()
    }

    /// The hover state of one of the two step buttons: `0` is previous, `1`
    /// is next.
    pub fn step_state(&self, forward: bool) -> MouseStateHandle {
        self.0.borrow().steps[usize::from(forward)].clone()
    }

    /// The close button's hover state.
    pub fn close_state(&self) -> MouseStateHandle {
        self.0.borrow().close_button.clone()
    }

    /// How the query is matched.
    pub fn options(&self) -> FindOptions {
        let state = self.0.borrow();
        FindOptions {
            case_sensitive: state.case_sensitive,
            regex: state.regex,
        }
    }

    /// Flips one of the two switches — case when `regex` is false — and
    /// takes the cursor back to the first match, since a search matched
    /// another way is a new search.
    pub fn toggle(&self, regex: bool) {
        let mut state = self.0.borrow_mut();
        if regex {
            state.regex = !state.regex;
        } else {
            state.case_sensitive = !state.case_sensitive;
        }
        state.current = 0;
    }

    /// A switch's hover state: the case switch, or the regex one.
    pub fn switch_state(&self, regex: bool) -> MouseStateHandle {
        self.0.borrow().switches[usize::from(regex)].clone()
    }

    /// The filter switch's hover state.
    pub fn filter_state(&self) -> MouseStateHandle {
        self.0.borrow().switches[2].clone()
    }

    /// What is being looked for.
    pub fn query(&self) -> String {
        self.0.borrow().input.editor().text().to_owned()
    }

    /// Every place the query is in a pane's blocks — `history`'s finished
    /// ones, then the open one `snapshot` holds — in reading order.
    ///
    /// Kept rather than walked fresh: a frame searches only the blocks that
    /// have finished since the one before it, and the open block when the
    /// snapshot is a new one, so a frame over an unchanged output reads none
    /// of it. See the module docs. Nothing at all for an empty query, or for
    /// a pattern that does not compile.
    pub fn matches(&self, history: &BlockHistory, snapshot: &Arc<Snapshot>) -> Matches {
        let query = self.query();
        let options = self.options();
        self.0
            .borrow_mut()
            .found
            .refresh(&query, options, history, snapshot)
    }

    /// Whether the query is a regular expression that does not compile, which
    /// the bar says rather than counting no results.
    ///
    /// Answered by the matcher the search keeps, so a frame does not compile
    /// the pattern again to ask, and the bar cannot call a pattern fine that
    /// the search could not compile — as it did when it built its own without
    /// the search's case folding.
    pub fn pattern_is_bad(&self) -> bool {
        !self.query().is_empty() && self.matcher().is_none()
    }

    /// The query, compiled the way the search reads it, or `None` for a
    /// pattern that does not compile.
    ///
    /// The very one the search keeps, compiled when the query or a switch
    /// last changed: what the bar's filter matches a block's lines with too,
    /// so that a frame of the filter compiles nothing either — a pattern that
    /// does not compile included, which leaves the filter no answer of its own
    /// to remember.
    pub fn matcher(&self) -> Option<Rc<LineMatcher>> {
        let query = self.query();
        let options = self.options();
        let mut state = self.0.borrow_mut();
        state.found.start(&query, options);
        state.found.matcher.clone()
    }

    /// Takes the cursor back to the first match when `query` is not the one
    /// the cursor was last counted against.
    ///
    /// A new search starts at the top, the way Ctrl+F does everywhere: a
    /// person who searched one thing, stepped to its fifth match and then
    /// typed a different thing means to look at the new thing from the
    /// start, not at whatever match five modulo the new count lands on. The
    /// *same* query is left alone, so closing a search and reopening it
    /// resumes where it was, which is what the workspace's `Open` handler
    /// leans on.
    ///
    /// Called where the matches are counted rather than on the keystroke,
    /// because the query lives in the field's own editor and this is the one
    /// place that reads it back as the thing a search is actually for.
    pub fn follow_query(&self, query: &str) {
        let mut state = self.0.borrow_mut();
        if state.last_query != query {
            state.current = 0;
            state.last_query = query.to_owned();
        }
    }

    /// The current match, brought inside a count of `total` matches.
    ///
    /// `None` when there are none: the field is empty, or nothing in the
    /// output matches what is in it. Otherwise the stored index wrapped into
    /// range, so a match count that shrank under the cursor — a command that
    /// printed over the line a match was on — leaves the cursor on a match
    /// that still exists rather than off the end.
    pub fn clamped(&self, total: usize) -> Option<usize> {
        (total > 0).then(|| self.0.borrow().current % total)
    }

    /// Steps `delta` matches from the current one, wrapping at both ends, and
    /// returns where it landed — or `None` when there is nothing to step to.
    ///
    /// The step is taken from the *clamped* current, so stepping forward from
    /// a cursor left past the end by a shrinking match count goes to the
    /// second match rather than to wherever the raw index modulo the count
    /// happened to fall.
    pub fn step(&self, delta: isize, total: usize) -> Option<usize> {
        if total == 0 {
            return None;
        }
        let total_signed = total as isize;
        let here = (self.0.borrow().current % total) as isize;
        let next = (here + delta).rem_euclid(total_signed) as usize;
        self.0.borrow_mut().current = next;
        Some(next)
    }
}

/// What a search has found in a pane's blocks, kept between frames.
///
/// See the module docs for why none of it can be out of date.
#[derive(Default)]
struct Found {
    /// The query and the switches all of this was found with. Any other is a
    /// different search, which starts again from nothing.
    search: Option<(String, FindOptions)>,
    /// The query, compiled once for the search rather than once a frame, or
    /// `None` for a pattern that does not compile — which the bar says, and
    /// which matches nothing. Shared with the filter: see
    /// [`PaneFind::matcher`].
    matcher: Option<Rc<LineMatcher>>,
    /// The newest finished block searched. Every block the history holds up
    /// to it has been, since blocks finish in the order of their ids; every
    /// one after it has not.
    through: Option<BlockId>,
    /// Every match in the finished blocks, in reading order.
    finished: Rc<Vec<Region>>,
    /// The snapshot the open block was last searched in, and what was found.
    /// The emulator hands back the very same snapshot until something drawn
    /// in it changes, so the same one is the same open block.
    open: Option<(Arc<Snapshot>, Rc<Vec<Region>>)>,
}

impl Found {
    /// Makes this the search for `query`, matched as `options` says: kept as
    /// it is when it already is that search, and begun again from nothing,
    /// with the query compiled afresh, when it is any other.
    fn start(&mut self, query: &str, options: FindOptions) {
        if self
            .search
            .as_ref()
            .is_none_or(|(held, how)| held != query || *how != options)
        {
            *self = Self {
                search: Some((query.to_owned(), options)),
                matcher: LineMatcher::new(query, options).map(Rc::new),
                ..Self::default()
            };
        }
    }

    /// Brings what is kept up to date with `history` and `snapshot` for
    /// `query`, matched as `options` says, and hands back every match.
    fn refresh(
        &mut self,
        query: &str,
        options: FindOptions,
        history: &BlockHistory,
        snapshot: &Arc<Snapshot>,
    ) -> Matches {
        if query.is_empty() {
            *self = Self::default();
            return Matches::default();
        }
        self.start(query, options);
        let Some(matcher) = &self.matcher else {
            return Matches::default();
        };

        // The blocks dropped off the front of the history since the last
        // frame, and their matches with them. Ids are handed out in order, so
        // theirs are the matches before the first block still held.
        let oldest = history.get(0).map(|block| block.id);
        let gone = self
            .finished
            .partition_point(|found| oldest.is_none_or(|oldest| found.start.block < oldest));
        if gone > 0 {
            Rc::make_mut(&mut self.finished).drain(..gone);
        }

        // The blocks that have finished since: everything after the newest
        // one searched, or — once that one has been dropped too — everything
        // the history holds.
        let new = self.through.map_or(0, |through| {
            history.position(through).map_or(0, |index| index + 1)
        });
        if new < history.len() {
            let finished = Rc::make_mut(&mut self.finished);
            for block in history.iter().skip(new) {
                let item = Item {
                    id: block.id,
                    rows: Rows::Stored(&block.rows),
                    first: 0,
                };
                item.find_with(matcher, finished);
            }
            self.through = history
                .len()
                .checked_sub(1)
                .and_then(|last| history.get(last))
                .map(|block| block.id);
        }

        let open = match &self.open {
            Some((seen, found)) if Arc::ptr_eq(seen, snapshot) => found.clone(),
            _ => {
                let mut found = Vec::new();
                if let Blocks::List {
                    open: Some(item), ..
                } = Blocks::list(history, snapshot)
                {
                    item.find_with(matcher, &mut found);
                }
                let found = Rc::new(found);
                self.open = Some((snapshot.clone(), found.clone()));
                found
            }
        };
        Matches {
            finished: self.finished.clone(),
            open,
            grid: Rc::default(),
        }
    }
}

/// Every match a find bar has, in reading order, as a frame is handed them.
///
/// Two runs that read as one list — the finished blocks' matches, then the
/// open block's — because they are kept on two clocks: see
/// [`PaneFind::matches`]. Each match is the cells it covers, already resolved,
/// and handing the lot to the element that paints them is two pointers.
///
/// Or, over a pane drawing one grid, the grid's matches instead: selections in
/// the grid's own numbering, as its search last found them (see the module
/// docs, "Over the grid"), which the bar counts and steps through the same
/// way. A pane draws one or the other, so only one of the two is ever held.
#[derive(Clone, Debug, Default)]
pub struct Matches {
    /// The finished blocks', oldest first.
    finished: Rc<Vec<Region>>,
    /// The open block's.
    open: Rc<Vec<Region>>,
    /// A grid's, when the bar is over one rather than over the list.
    grid: Rc<Vec<Selection>>,
}

impl Matches {
    /// A grid's matches, as [`PaneFind::grid_matches`] hands them out.
    pub fn over_grid(grid: Rc<Vec<Selection>>) -> Self {
        Self {
            grid,
            ..Self::default()
        }
    }

    /// How many there are.
    pub fn len(&self) -> usize {
        self.finished.len() + self.open.len() + self.grid.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One of the list's matches by its place in reading order, counted from
    /// zero. A grid's are not resolved here: see [`Self::start`].
    pub fn get(&self, index: usize) -> Option<Region> {
        match index.checked_sub(self.finished.len()) {
            None => self.finished.get(index).copied(),
            Some(open) => self.open.get(open).copied(),
        }
    }

    /// The first cell of one match by its place in reading order, counted
    /// from zero — what bringing it into view needs, over the list or a grid.
    pub fn start(&self, index: usize) -> Option<Place> {
        if self.grid.is_empty() {
            return self.get(index).map(|found| found.start);
        }
        // A find's selection is made from its first cell to its last, so its
        // anchor is where it starts.
        let from = self.grid.get(index)?.anchor;
        Some(Place::new(from.block, from.row, from.column))
    }

    /// The matches that cover any of row `row` of block `block`, in reading
    /// order — what painting one row asks, whatever the history holds.
    pub fn on_row(&self, block: BlockId, row: usize) -> impl Iterator<Item = &Region> {
        regions_on(&self.finished, block, row)
            .iter()
            .chain(regions_on(&self.open, block, row))
    }
}

/// Every row a grid holds, scrollback and screen, oldest first — read `piece`
/// rows at a time through `harvest`, which is asked for rows numbered from the
/// oldest line the scrollback holds and stops at the newest.
///
/// In pieces so that the lock `harvest` takes is let go between them, and the
/// pty's reader and a keystroke wait for a piece rather than for the whole of
/// a full scrollback. The price is that the grid can move between two pieces:
/// a resize makes rows of another width, which end the read with what it has,
/// and a full scrollback still filling drops a line for every one printed, so
/// the rows read after a drop are numbered a line further on than the text
/// they hold. Either is a grid that was moving, and its repaint asks again.
pub fn harvest_grid(mut harvest: impl FnMut(usize, usize) -> BlockRows, piece: usize) -> BlockRows {
    let piece = piece.max(1);
    let mut rows = BlockRows::default();
    let mut first = 0;
    loop {
        let read = harvest(first, first + piece - 1);
        let count = read.rows();
        if !rows.append(read) || count < piece {
            return rows;
        }
        first += piece;
    }
}

/// Whether two pictures of a grid hold the same text in the same places, so
/// that what was found in one is what would be found in the other.
///
/// Every character and every flag on its cell — among them the ones that say
/// how it reads: wide, a spacer, a line the terminal folded — and the marks
/// stacked on them, at the same width and height, over the same scrollback on
/// the same screen. Not the title, the cursor or any colour, which a working
/// agent changes many times a second without printing a letter:
/// [`Snapshot::same_content`] counts those, and its revision moves for them.
///
/// The scrollback is compared by its length alone, since it is not in a
/// snapshot. That is enough while it is filling, when a line can only be added
/// to it; once it is full, a line printed and the oldest dropped leave the
/// length where it was, and only the screen, which moved with them, tells.
///
/// Only for pictures of the live screen. Scrolled back, a snapshot holds the
/// rows in view and not the bottom of the screen under them, which a program
/// can rewrite in place — a status line, a list of things to do — without
/// adding a line: two pictures alike in every cell a snapshot holds can
/// differ there, and are not taken for each other.
pub fn same_text(a: &Snapshot, b: &Snapshot) -> bool {
    a.revision == b.revision
        || (a.columns == b.columns
            && a.rows == b.rows
            && a.display_offset == 0
            && b.display_offset == 0
            && a.history_len == b.history_len
            && a.alt_screen == b.alt_screen
            && a.combining == b.combining
            && a.cells.len() == b.cells.len()
            && a.cells
                .iter()
                .zip(&b.cells)
                .all(|(a, b)| a.c == b.c && a.flags == b.flags))
}

/// Every place `query` occurs in a grid's `rows`, each as the selection that
/// covers it: under the name `id`, with rows numbered from the oldest line —
/// the address space a selection on the grid is made in.
///
/// The list's own walk, over the grid as a list of one block, so the grid and
/// the list find a query by the same rules: the same switches, the same fold
/// of capitals in every script, a folded line read as the one line it is.
/// Empty for a pattern that does not compile, which the bar says for itself.
pub fn find_in_grid(
    rows: &BlockRows,
    id: BlockId,
    query: &str,
    options: FindOptions,
) -> Vec<Selection> {
    Blocks::one(id, Rows::Stored(rows), 0)
        .find_all_with(query, options)
        .unwrap_or_default()
}

/// Which of a grid's matches, in reading order, cross the rows `snapshot` has
/// on screen.
///
/// Matches never overlap, so their ends are in order as well as their starts,
/// and two binary searches find the run of them that is on screen: a frame
/// paints a screenful of them however many thousands the scrollback holds.
pub fn on_screen(matches: &[Selection], snapshot: &Snapshot) -> Range<usize> {
    let top = grid_first_row(snapshot);
    let bottom = top + snapshot.rows;
    let from = matches.partition_point(|found| found.head.row < top);
    let to = matches.partition_point(|found| found.anchor.row < bottom);
    from..to.max(from)
}

/// How many lines to scroll a grid to bring `row` — numbered from the oldest
/// line of its scrollback — onto the screen, or `None` when it is on screen
/// already. Positive is back into the scrollback, which is the emulator's
/// sense and the wheel's.
///
/// A third of a screen is left above it, the way the list brings a match into
/// view, so it lands in the body of the pane rather than against its top edge;
/// and a row already on screen moves nothing, so stepping between two matches
/// a few lines apart does not lurch the screen.
pub fn scroll_to_show(snapshot: &Snapshot, row: usize) -> Option<i32> {
    let top = grid_first_row(snapshot);
    if (top..top + snapshot.rows).contains(&row) {
        return None;
    }
    let wanted = row.saturating_sub(snapshot.rows / 3);
    // The offset that puts `wanted` at the top. A row on the live screen has
    // none, and the screen stays where it always is.
    let offset = snapshot.history_len.saturating_sub(wanted);
    let lines = offset as i64 - snapshot.display_offset as i64;
    i32::try_from(lines).ok().filter(|lines| *lines != 0)
}

#[cfg(test)]
#[path = "pane_find_tests.rs"]
mod tests;
