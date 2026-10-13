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
use std::rc::Rc;
use std::sync::Arc;

use crook_terminal::{BlockId, Rows, Snapshot};
use crookui_core::elements::MouseStateHandle;

use crate::selection::{Blocks, FindOptions, Item, LineMatcher, Region, regions_on};
use crate::terminal_model::BlockHistory;
use crate::text_input::TextInput;

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
        })))
    }

    /// Opens the bar, reporting whether it was closed.
    ///
    /// Does not touch the query: opening the bar a second time on the same
    /// pane comes back to what was being looked for, which is what a person
    /// pressing the chord again to get back to their search means.
    pub fn open(&self) -> bool {
        let mut state = self.0.borrow_mut();
        !std::mem::replace(&mut state.open, true)
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
        std::mem::replace(&mut state.open, false)
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
    /// which matches nothing.
    matcher: Option<LineMatcher>,
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
        if self
            .search
            .as_ref()
            .is_none_or(|(held, how)| held != query || *how != options)
        {
            *self = Self {
                search: Some((query.to_owned(), options)),
                matcher: LineMatcher::new(query, options),
                ..Self::default()
            };
        }
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
        }
    }
}

/// Every match a find bar has, in reading order, as a frame is handed them.
///
/// Two runs that read as one list — the finished blocks' matches, then the
/// open block's — because they are kept on two clocks: see
/// [`PaneFind::matches`]. Each match is the cells it covers, already resolved,
/// and handing the lot to the element that paints them is two pointers.
#[derive(Clone, Debug, Default)]
pub struct Matches {
    /// The finished blocks', oldest first.
    finished: Rc<Vec<Region>>,
    /// The open block's.
    open: Rc<Vec<Region>>,
}

impl Matches {
    /// How many there are.
    pub fn len(&self) -> usize {
        self.finished.len() + self.open.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One match by its place in reading order, counted from zero.
    pub fn get(&self, index: usize) -> Option<Region> {
        match index.checked_sub(self.finished.len()) {
            None => self.finished.get(index).copied(),
            Some(open) => self.open.get(open).copied(),
        }
    }

    /// The matches that cover any of row `row` of block `block`, in reading
    /// order — what painting one row asks, whatever the history holds.
    pub fn on_row(&self, block: BlockId, row: usize) -> impl Iterator<Item = &Region> {
        regions_on(&self.finished, block, row)
            .iter()
            .chain(regions_on(&self.open, block, row))
    }
}

#[cfg(test)]
#[path = "pane_find_tests.rs"]
mod tests;
