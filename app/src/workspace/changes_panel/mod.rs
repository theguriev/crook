//! The Changes column: what the agent in the focused tab changed, without
//! leaving Crook and without Crook becoming an editor.
//!
//! # What it shows
//!
//! The focused pane's repository, read by [`crate::git::changes`]: what the
//! work is compared with — where the branch left the base, the merge-base a
//! pull request would compare from — then the commits since, newest first,
//! then every file that differs, committed or not, with the files nobody has
//! added among them. Pressing a file shows its hunks under it, read then and
//! not before, and a file's own row of actions: open it in `$VISUAL` or
//! `$EDITOR` — at the first line that changed, for an editor known to take a
//! line — copy its path, copy its diff.
//!
//! # What it does not do
//!
//! Stage, revert, edit. The tree it shows is one an agent is working in, and
//! a write from here races whatever the agent is in the middle of writing —
//! a revert that lands between the agent's read of a file and its write of
//! it is a revert the agent silently undoes, or an edit it makes against a
//! file that is no longer what it read. Changing the work is the agent's job
//! (tell it) or the editor's (open it there). Nor is there syntax
//! highlighting: added and removed lines are the theme's own `diff_added`
//! and `diff_removed`, the colours the row's `+12 −3` chip is already in.
//!
//! # Telling the agent
//!
//! Which it does do. A press on a changed line, or on a hunk's header, opens
//! a field under it; what is typed there is a comment, and the comments of a
//! tab are one review, sent to that tab's agent as one message pasted into
//! its prompt and left there unsent — or copied, for an agent somewhere
//! else. How a comment keeps to its line while the agent goes on writing,
//! and what the message says, is [`review`]; which pane it goes to, and why
//! it goes nowhere rather than to a shell's prompt, is
//! [`Workspace::review_pane`] and the send beside it.
//!
//! # Where it is
//!
//! A docked column beside the work, composed where the Themes panel is — in
//! [`Workspace::render`](super::view::Workspace), once, for every section —
//! and pushing the work aside rather than covering it, so the agent's output
//! and what it did are side by side. `docs/plugins.md` designs a
//! `window.column` slot for exactly this kind of surface; with the Themes
//! panel and this the only two, a slot for two built-ins would be an API with
//! no stranger to hold it to, and it waits for one.
//!
//! # A screenful, whatever the diff
//!
//! Every row — headings, commits, files, a file's hunk lines — is one entry
//! of one flat list, each a fixed height by kind, with the running sum of
//! those heights kept beside it ([`ChangesPanelState::relayout`]). A frame
//! finds the first row under the scroll offset by binary search and builds
//! rows until it passes the bottom of the box, with a spacer above and below
//! standing for everything else — the arithmetic
//! [`super::block_list`] draws a pane's output with. So a five-thousand-file
//! diff costs the rows on screen, not five thousand of them; the sums are
//! rebuilt when what is listed changes, never per frame.
//!
//! # When it reads
//!
//! When it opens, when the focused pane moves to another repository, on the
//! Refresh button, and at the end of every cycle of the git facts behind the
//! tab rows — the fifteen seconds the branch chip already runs on. No timer
//! of its own: the window keeps one poll chain, and this rides it. Everything
//! is read on the background pool and lands through `ctx.spawn`; nothing
//! git-shaped is on the render path.

mod launch;
mod review;

#[cfg(test)]
mod tests;

pub(crate) use launch::Editor;
pub(crate) use review::Comment;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use crookui_core::elements::{MouseStateHandle, Padding, Paragraph};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::git::changes::{
    Against, Base, Error, FileChange, FileDiff, MAX_COMMITS, MAX_FILES, Overview, Status,
};
use crate::tab::TabId;
use crate::text_input::TextInput;
use crate::theme::theme;

use review::{Anchor, Key};

use super::action::{ChangesAction, WorkspaceAction};
use super::view::Workspace;

/// The column's width.
///
/// Wider than the Themes panel's 248, because a diff line is code and code
/// is read across: at 11px monospace this is about fifty columns, which is
/// most of a line of most code, and in the 1024-wide window Crook opens it
/// still leaves the pane beside it four hundred pixels.
pub(super) const PANEL_WIDTH: f32 = 340.;

/// The strip at the top, which holds Refresh and the close button at its
/// right end and nothing at its left: the Themes panel's strip, and as tall.
///
/// The empty end is not waste. With the tabs hidden this is the window's
/// leftmost column, and on a client-decorated macOS window the traffic
/// lights are painted over its top-left corner — see
/// [`Workspace::window_insets`](super::view::Workspace) — which is why the
/// title is on a row of its own below the strip rather than in it.
const HEADER_HEIGHT: f32 = 32.;

/// The column's own inset.
const PANEL_PADDING: f32 = 12.;

/// The close icon, inside its 20px button.
const ICON_SIZE: f32 = 12.;

/// What the column is called.
const TITLE: &str = "Changes";

/// The box a file's chevron is drawn in.
const CHEVRON_WIDTH: f32 = 12.;

/// The box a file's status letter is drawn in, which is also the gap between
/// it and the path: a letter touching the path reads as part of the name.
const LETTER_WIDTH: f32 = 16.;

/// How far a file's actions and notes are indented under it: past the
/// chevron and the status letter, so they read as the file's.
const INDENT: f32 = CHEVRON_WIDTH + LETTER_WIDTH;

/// Room kept clear down the list's right edge for the scrollbar's thumb,
/// which is painted over whatever is there: a commit's date, most often.
const THUMB_ROOM: f32 = 8.;

/// How much of the list a frame builds when the box it will be laid out in
/// has not been measured yet: the first frame after opening, and every frame
/// of a headless one. Taller than any window, so nothing on screen is left
/// unbuilt; once a layout has measured the box, the box is what counts.
const UNMEASURED_EXTENT: f32 = 2160.;

/// Each kind of row's height. Fixed per kind, which is what makes the list
/// windowable: where a row starts is a sum, not a measurement.
mod height {
    /// "3 commits since main".
    pub(super) const HEADING: f32 = 30.;
    /// A sentence where a list would be.
    pub(super) const MESSAGE: f32 = 22.;
    /// One commit.
    pub(super) const COMMIT: f32 = 22.;
    /// One file.
    pub(super) const FILE: f32 = 24.;
    /// A file's buttons.
    pub(super) const ACTIONS: f32 = 30.;
    /// One line of a hunk.
    pub(super) const LINE: f32 = 16.;
    /// A sentence under a file: reading, binary, cut short.
    pub(super) const NOTE: f32 = 22.;
    /// A comment, under the line it is about.
    pub(super) const COMMENT: f32 = 24.;
    /// The field a comment is typed in, and a little air around it.
    pub(super) const DRAFT: f32 = super::super::text_field::HEIGHT + 8.;
}

/// What has been read about the repository.
#[derive(Default)]
pub(super) enum Reading {
    /// Nothing yet: the first read is under way.
    #[default]
    Nothing,
    /// The last read's answer.
    Ready(Overview),
    /// Why the last read had none.
    Failed(Error),
}

/// One file's diff, as far as it has got.
#[derive(Default)]
struct Hunks {
    /// The last answer, which stays on screen while the next one is read.
    diff: Option<Result<FileDiff, String>>,
    /// The read under way, if one is: only its answer is taken.
    ticket: Option<u64>,
}

/// A diff to read on the background pool, and how to recognise its answer.
pub(super) struct HunkRead {
    /// The top of the working tree.
    pub(super) repository: PathBuf,
    /// What it is compared with.
    pub(super) base: Base,
    /// The file.
    pub(super) file: FileChange,
    /// Which read this is. See [`ChangesPanelState::land_hunks`].
    pub(super) ticket: u64,
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Row {
    /// What the rows under it are.
    Heading(String),
    /// A sentence where a list would be, or after one.
    Message(String),
    /// A commit, by its place in the overview.
    Commit(usize),
    /// A file, by its place in the overview.
    File(usize),
    /// A shown file's buttons.
    Actions(usize),
    /// Line `.1` of file `.0`'s hunks.
    Line(usize, usize),
    /// A sentence about a shown file's hunks.
    Note(String),
    /// A comment on file `.0`, by its id.
    Comment(usize, u64),
    /// The field a comment is being typed in, under a line of file `.0`.
    Draft(usize),
}

impl Row {
    /// How tall it is drawn.
    fn height(&self) -> f32 {
        match self {
            Self::Heading(_) => height::HEADING,
            Self::Message(_) => height::MESSAGE,
            Self::Commit(_) => height::COMMIT,
            Self::File(_) => height::FILE,
            Self::Actions(_) => height::ACTIONS,
            Self::Line(..) => height::LINE,
            Self::Note(_) => height::NOTE,
            Self::Comment(..) => height::COMMENT,
            Self::Draft(_) => height::DRAFT,
        }
    }
}

/// One clickable thing in the column.
///
/// Keyed by identity for the reason the Themes panel's controls are: the rows
/// come and go with what is in the repository.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Control {
    /// The × in the corner.
    Close,
    /// "Refresh".
    Refresh,
    /// A file's row.
    File(usize),
    /// A file's "Open in …".
    Open(usize),
    /// A file's "Copy path".
    CopyPath(usize),
    /// A file's "Copy diff".
    CopyDiff(usize),
    /// Line `.1` of file `.0`'s hunks, which a press leaves a comment on.
    Line(usize, usize),
    /// A comment's ×, by the comment's id.
    RemoveComment(u64),
    /// The field a comment is typed in.
    Draft,
    /// "Send N comments to the agent".
    Send,
    /// "Copy review".
    CopyReview,
    /// The × on what the review last said.
    DismissNote,
}

/// The comment being typed: where it will go, and what has been typed.
struct Draft {
    /// The line it is about.
    anchor: Anchor,
    /// What has been typed, with its own caret and undo.
    input: TextInput,
    /// Whether the keyboard has been put in it, rather than left with the
    /// pane. A wish, the way the tab search box's is: whether it actually has
    /// the keyboard is [`Workspace::changes_takes_keys`], which also asks
    /// whether anything is covering it.
    focused: bool,
    /// Why a read has left it with no line to be drawn under, if one has.
    adrift: Option<Adrift>,
}

/// Why the comment being typed has lost its line to a read.
///
/// A read never takes a field away from under a person's typing. A refresh
/// that comes home while they type — and the agent the comment is about is
/// editing the very lines being commented on — keeps the field, the words
/// and the keyboard, and moves the field to the top of the column with this
/// said over it. Were the field dropped instead, the keyboard would go back
/// to the focused pane with nobody asking it to, and the rest of the comment
/// and its Enter would be typed into the agent.
///
/// None of these is final. The field keeps the line it was on, and every
/// refresh reads its file again, folded or not: a read that finds the line
/// puts the field back under it, words and keyboard, and shows the file's
/// lines for it to be among. Most of what sets a field adrift says nothing
/// about the line — one read that timed out, a line past where a long diff
/// is cut short — and an agent that rewrote a line can write it back.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Adrift {
    /// The diff, or the list of files, was read again without the line.
    Gone,
    /// The diff was read again, cut short before the line was found.
    PastCut,
    /// A read failed, and what the column was showing went with it.
    Unread,
}

impl Adrift {
    /// What is said over the field.
    fn caption(self) -> &'static str {
        match self {
            Self::Gone => {
                "The line this comment is on is no longer in the diff. It goes back under the \
                 line if a refresh finds it again; Enter or Escape closes it."
            }
            Self::PastCut => {
                "The line this comment is on is not in the part of the diff that was read, which \
                 is cut short. It goes back under the line when a refresh finds it; Enter or \
                 Escape closes it."
            }
            Self::Unread => {
                "The changes could not be read again, so this comment has no line to go on for \
                 now. It goes back under the line when a refresh finds it; Enter or Escape \
                 closes it."
            }
        }
    }
}

/// Whether the column is up, what it is showing, and what the mouse is doing
/// to it.
#[derive(Default)]
pub(super) struct ChangesPanelState {
    /// Whether the column is on screen.
    pub(super) open: bool,
    /// Where it is looking: the top of the repository the focused pane is
    /// in, or the pane's directory when that is in none. `None` for a pane
    /// that has not said where it is.
    pub(super) target: Option<PathBuf>,
    /// What was read there.
    reading: Reading,
    /// Whether a read of the overview is under way, so a refresh arriving
    /// meanwhile is not a second one.
    in_flight: bool,
    /// Which read of the overview is the current one. An answer carrying
    /// another is about a question nobody is asking any more.
    epoch: u64,
    /// The files whose hunks are showing, by path — a path rather than a
    /// place in the list, because a refresh can move a file's place.
    expanded: HashSet<PathBuf>,
    /// Each file's diff, once somebody asked for it.
    hunks: HashMap<PathBuf, Hunks>,
    /// How many diffs have been asked for, which is also where each read's
    /// ticket comes from.
    hunk_reads: u64,
    /// Every row, in order. Rebuilt when what is listed changes.
    rows: Vec<Row>,
    /// Where each row starts, and after the last, where the list ends.
    starts: Vec<f32>,
    /// How far the list has been scrolled.
    pub(super) scroll: ScrollStateHandle,
    /// The editor "Open in …" starts, read from the environment when the
    /// column opens; `None` hides the button.
    editor: Option<Editor>,
    /// The file "Open" last failed to start the editor on, and why — said
    /// under that file's buttons, because a press that did nothing on screen
    /// reads as a button that is broken rather than an editor that is.
    launch_failure: Option<(PathBuf, String)>,
    /// One mouse state per control, made on the control's first frame.
    controls: RefCell<HashMap<Control, MouseStateHandle>>,
    /// The tab the column is about: the one the focused pane is in.
    tab: Option<TabId>,
    /// Every review there is, by tab, repository and base — kept when the
    /// column closes and when it moves to another tab, and let go of when
    /// the tab closes. See [`review`].
    reviews: HashMap<Key, Vec<Comment>>,
    /// How many comments have been made, which is also where each one's id
    /// comes from.
    comments_made: u64,
    /// The comment being typed, if one is.
    draft: Option<Draft>,
    /// The last thing the review had to say: where a Send went, why it went
    /// nowhere, which comments a refresh dropped.
    note: Option<String>,
}

impl ChangesPanelState {
    /// The mouse state for one control.
    pub(super) fn control(&self, control: Control) -> MouseStateHandle {
        self.controls
            .borrow_mut()
            .entry(control)
            .or_default()
            .clone()
    }

    /// Forgets every hover and press, for the reason the Themes panel does:
    /// the rows under the pointer are about to be different rows.
    pub(super) fn forget_hover_state(&self) {
        for state in self.controls.borrow().values() {
            state.lock().reset_interaction_state();
        }
    }

    /// What the last read found, if it found anything.
    pub(super) fn overview(&self) -> Option<&Overview> {
        match &self.reading {
            Reading::Ready(overview) => Some(overview),
            _ => None,
        }
    }

    /// How many diffs have been asked for since the column was made: what a
    /// test of "read once" counts.
    #[cfg(test)]
    pub(super) fn hunk_reads(&self) -> u64 {
        self.hunk_reads
    }

    /// Whether `path`'s hunks are showing.
    pub(super) fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// The editor "Open in …" would start.
    pub(super) fn editor(&self) -> Option<&Editor> {
        self.editor.as_ref()
    }

    /// The tab the column is about.
    pub(super) fn tab(&self) -> Option<TabId> {
        self.tab
    }

    /// Whether a read is under way.
    pub(super) fn is_reading(&self) -> bool {
        self.in_flight
    }

    /// Puts the column up, looking at `target`, and answers the read to
    /// start: `None` when there is nowhere to read.
    pub(super) fn open(
        &mut self,
        tab: Option<TabId>,
        target: Option<PathBuf>,
        editor: Option<Editor>,
    ) -> Option<u64> {
        self.open = true;
        self.editor = editor;
        self.retarget(tab, target)
    }

    /// Takes the column down, and everything it read with it: a column
    /// opened again an hour later is about the repository as it is then.
    pub(super) fn close(&mut self) {
        self.open = false;
        self.target = None;
        self.reading = Reading::Nothing;
        self.in_flight = false;
        self.epoch = self.epoch.wrapping_add(1);
        self.expanded.clear();
        self.hunks.clear();
        self.launch_failure = None;
        self.draft = None;
        self.note = None;
        self.forget_hover_state();
        self.relayout();
    }

    /// Starts looking somewhere else, and answers the read to start.
    pub(super) fn retarget(&mut self, tab: Option<TabId>, target: Option<PathBuf>) -> Option<u64> {
        self.tab = tab;
        self.target = target;
        self.reading = Reading::Nothing;
        self.expanded.clear();
        self.hunks.clear();
        self.launch_failure = None;
        self.draft = None;
        self.note = None;
        self.scroll.lock().scroll_to_top();
        self.forget_hover_state();
        self.epoch = self.epoch.wrapping_add(1);
        self.in_flight = self.target.is_some();
        self.relayout();
        self.in_flight.then_some(self.epoch)
    }

    /// Reads the same place again, unless a read of it is already under way
    /// — which will answer the same question — or there is nowhere to read.
    pub(super) fn refresh(&mut self) -> Option<u64> {
        if !self.open || self.in_flight || self.target.is_none() {
            return None;
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.in_flight = true;
        Some(self.epoch)
    }

    /// An overview came home. Answers `None` when it is not the answer to
    /// the current read, and otherwise the diffs to read again: every file
    /// still showing its hunks, because what they said may have changed with
    /// everything else, and every file a comment or the comment field is on.
    pub(super) fn land(
        &mut self,
        epoch: u64,
        read: Result<Overview, Error>,
    ) -> Option<Vec<HunkRead>> {
        if epoch != self.epoch || !self.open {
            return None;
        }
        self.in_flight = false;

        match read {
            Ok(overview) => {
                let listed: HashSet<&PathBuf> =
                    overview.files.iter().map(|file| &file.path).collect();
                self.expanded.retain(|path| listed.contains(path));
                self.reading = Reading::Ready(overview);
                self.drop_unlisted();
                // A diff nobody is looking at is read again when somebody
                // does, rather than kept on the strength of a list that has
                // just been read again itself — unless a comment is on it:
                // what is sent about a line has to be about the line as it
                // is, folded or not.
                let commented = self.commented();
                let expanded = &self.expanded;
                self.hunks
                    .retain(|path, _| expanded.contains(path) || commented.contains(path));
            }
            Err(error) => {
                self.reading = Reading::Failed(error);
                self.expanded.clear();
                self.hunks.clear();
            }
        }
        // A field on a file this read took off the list, or on a list that
        // could not be read, has nothing to be drawn under — and is kept,
        // words and keyboard, see [`Adrift`]. One on a file still listed
        // waits for its diff, which says where its line is now.
        if let Some(draft) = self.draft.as_mut() {
            match &self.reading {
                Reading::Failed(_) => {
                    draft.adrift.get_or_insert(Adrift::Unread);
                }
                Reading::Ready(overview)
                    if !overview
                        .files
                        .iter()
                        .any(|file| file.path == draft.anchor.path) =>
                {
                    draft.adrift = Some(Adrift::Gone);
                }
                Reading::Ready(_) | Reading::Nothing => {}
            }
        }

        // The field's file too, folded or not: a field set adrift comes back
        // only through a read that finds its line, and a failed list read
        // has folded every file.
        let mut again: Vec<PathBuf> = self.expanded.iter().cloned().collect();
        again.extend(self.commented());
        again.extend(self.draft.as_ref().map(|draft| draft.anchor.path.clone()));
        again.sort();
        again.dedup();
        let reads = again.iter().filter_map(|path| self.request(path)).collect();
        self.relayout();
        Some(reads)
    }

    /// Shows the file at `index`'s hunks, or stops showing them. Answers the
    /// diff to read: the first time, after a read that failed, and after a
    /// refresh that came while the file was folded — which drops a folded
    /// file's diff, see [`Self::land`]. Between refreshes a file shown,
    /// hidden and shown again is read once. A nested repository is never
    /// read: there is no diff of one.
    pub(super) fn toggle(&mut self, index: usize) -> Option<HunkRead> {
        let file = self.overview()?.files.get(index)?;
        let path = file.path.clone();
        let nested = file.status == Status::Repository;
        if self.expanded.remove(&path) {
            // A field nobody can see must not keep the keyboard. One adrift
            // is drawn at the top rather than under the file, and stays —
            // and a read that finds its line shows the file again, to put
            // the field back under it.
            if self
                .draft
                .as_ref()
                .is_some_and(|draft| draft.anchor.path == path && draft.adrift.is_none())
            {
                self.draft = None;
            }
            self.relayout();
            return None;
        }
        self.expanded.insert(path.clone());

        let known = nested
            || self
                .hunks
                .get(&path)
                .is_some_and(|hunks| hunks.ticket.is_some() || matches!(hunks.diff, Some(Ok(_))));
        let read = if known { None } else { self.request(&path) };
        self.relayout();
        read
    }

    /// A diff came home. Taken only if it is the answer to the read that is
    /// still wanted for that file; says whether it was.
    pub(super) fn land_hunks(
        &mut self,
        path: &Path,
        ticket: u64,
        read: Result<FileDiff, String>,
    ) -> bool {
        let key = self.key();
        let Some(hunks) = self.hunks.get_mut(path) else {
            return false;
        };
        if hunks.ticket != Some(ticket) {
            return false;
        }
        hunks.ticket = None;
        // Every comment on the file, and the field if it is on it, moves to
        // where its line is in what was just read. A read that failed moves
        // no comment: the diff is the same diff until git says otherwise. It
        // does take away the lines the field was drawn among, though.
        match &read {
            Ok(diff) => {
                let comments = key.as_ref().and_then(|key| self.reviews.get_mut(key));
                let dropped = follow_lines(path, diff, comments, &mut self.draft);
                if let Some(note) = review::dropped(&dropped) {
                    self.note = Some(note);
                }
                // A field this read found the line of is under that line,
                // which has to be showing for the field to be seen: one that
                // was adrift may be on a file a failed read, or a person,
                // folded.
                if self
                    .draft
                    .as_ref()
                    .is_some_and(|draft| draft.anchor.path == path && draft.adrift.is_none())
                {
                    self.expanded.insert(path.to_owned());
                }
            }
            Err(_) => {
                if let Some(draft) = self
                    .draft
                    .as_mut()
                    .filter(|draft| draft.anchor.path == path && draft.adrift.is_none())
                {
                    draft.adrift = Some(Adrift::Unread);
                }
            }
        }
        hunks.diff = Some(read);
        self.relayout();
        true
    }

    /// The diff of the file at `path` as last read, when it was.
    pub(super) fn diff(&self, path: &Path) -> Option<&FileDiff> {
        self.hunks.get(path)?.diff.as_ref()?.as_ref().ok()
    }

    /// What pressing "Open" on the file at `path` came to: a failure is said
    /// under that file, and anything else takes the last one away.
    pub(super) fn opened(&mut self, path: &Path, result: Result<(), String>) {
        self.launch_failure = result.err().map(|problem| (path.to_owned(), problem));
        self.relayout();
    }

    /// A read of `path`'s diff, minted with a fresh ticket.
    fn request(&mut self, path: &Path) -> Option<HunkRead> {
        let overview = self.overview()?;
        let file = overview
            .files
            .iter()
            .find(|file| file.path == path)?
            .clone();
        let repository = overview.repository.clone();
        let base = overview.base.clone();

        self.hunk_reads += 1;
        let ticket = self.hunk_reads;
        self.hunks.entry(path.to_owned()).or_default().ticket = Some(ticket);
        Some(HunkRead {
            repository,
            base,
            file,
            ticket,
        })
    }

    /// Which review the column is showing: its tab's, about the repository
    /// and the base it read. `None` until a read has come home, and for a
    /// read that failed — there is nothing to leave a comment on then.
    fn key(&self) -> Option<Key> {
        let overview = self.overview()?;
        Some(Key {
            tab: self.tab?,
            repository: overview.repository.clone(),
            base: overview.base.name.clone(),
        })
    }

    /// The comments of the review the column is showing, in the order they
    /// were made.
    pub(super) fn comments(&self) -> &[Comment] {
        self.key()
            .and_then(|key| self.reviews.get(&key))
            .map_or(&[], Vec::as_slice)
    }

    /// The comments drawn on the file at `path`, top to bottom: all of them
    /// but those on lines past where its diff was cut short.
    fn comments_on(&self, path: &Path) -> Vec<&Comment> {
        let mut on: Vec<&Comment> = self
            .comments()
            .iter()
            .filter(|comment| comment.anchor.path == path && !comment.past_cut)
            .collect();
        on.sort_by_key(|comment| (comment.anchor.at, comment.id));
        on
    }

    /// How many comments on the file at `path` are on lines past where its
    /// diff was cut short, and so are kept without being drawn.
    fn past_cut_on(&self, path: &Path) -> usize {
        self.comments()
            .iter()
            .filter(|comment| comment.anchor.path == path && comment.past_cut)
            .count()
    }

    /// Opens the field a comment is typed in, under line `line` of the file
    /// at `file`, with the keyboard in it — in place of any other, and what
    /// was typed there with it. Says whether it did: only a line added or
    /// removed, or a hunk's header, takes a comment.
    pub(super) fn start_comment(&mut self, file: usize, line: usize) -> bool {
        let Some(path) = self
            .overview()
            .and_then(|overview| overview.files.get(file))
            .map(|file| file.path.clone())
        else {
            return false;
        };
        let Some(diff) = self.diff(&path) else {
            return false;
        };
        let Some(text) = diff
            .lines
            .get(line)
            .filter(|text| review::commentable(text))
        else {
            return false;
        };
        let Some(place) = review::places(&diff.lines).get(line).copied().flatten() else {
            return false;
        };
        let anchor = Anchor {
            path,
            line: text.clone(),
            place,
            at: line,
        };
        self.draft = Some(Draft {
            anchor,
            input: TextInput::new(),
            focused: true,
            adrift: None,
        });
        self.relayout();
        true
    }

    /// What is being typed into the comment field, while one is up.
    pub(super) fn draft_input(&self) -> Option<&TextInput> {
        self.draft.as_ref().map(|draft| &draft.input)
    }

    /// Why a read has left the comment field with no line, if one has: the
    /// field is then drawn at the top of the column rather than in the list.
    pub(super) fn adrift(&self) -> Option<Adrift> {
        self.draft.as_ref().and_then(|draft| draft.adrift)
    }

    /// Tells the comment field whether the keyboard is its. See
    /// [`Workspace::sync_input_keys`].
    pub(super) fn set_draft_keys(&self, has_keys: bool) {
        if let Some(draft) = &self.draft {
            draft.input.set_has_keys(has_keys);
        }
    }

    /// Whether the keyboard has been put in the comment field.
    pub(super) fn draft_is_focused(&self) -> bool {
        self.draft.as_ref().is_some_and(|draft| draft.focused)
    }

    /// Puts the keyboard in the comment field, or takes it out; the words
    /// stay either way. Says whether that changed anything.
    pub(super) fn focus_draft(&mut self, focused: bool) -> bool {
        match &mut self.draft {
            Some(draft) if draft.focused != focused => {
                draft.focused = focused;
                true
            }
            _ => false,
        }
    }

    /// Adds what was typed as a comment on the field's line, and takes the
    /// field down. Says whether a comment was added: Enter on an empty field
    /// is a field somebody changed their mind in, and it only goes away — and
    /// so does Enter on a field a read has left adrift with no line, which
    /// says so.
    pub(super) fn add_comment(&mut self) -> bool {
        let Some(draft) = self.draft.take() else {
            return false;
        };
        let text = draft.input.editor().text().trim().to_owned();
        if draft.adrift.is_some() {
            if !text.is_empty() {
                self.note = Some(
                    "The comment was not added: it had no line in the diff to go on.".to_owned(),
                );
            }
            self.relayout();
            return false;
        }
        let added = match self.key() {
            Some(key) if !text.is_empty() => {
                self.comments_made += 1;
                self.reviews.entry(key).or_default().push(Comment {
                    id: self.comments_made,
                    anchor: draft.anchor,
                    text,
                    past_cut: false,
                });
                self.note = None;
                true
            }
            _ => false,
        };
        self.relayout();
        added
    }

    /// Takes the field down, and what was typed in it with it.
    pub(super) fn cancel_comment(&mut self) -> bool {
        if self.draft.take().is_none() {
            return false;
        }
        self.relayout();
        true
    }

    /// Takes the comment `id` away. Says whether there was one.
    pub(super) fn remove_comment(&mut self, id: u64) -> bool {
        let Some(comments) = self.key().and_then(|key| self.reviews.get_mut(&key)) else {
            return false;
        };
        let before = comments.len();
        comments.retain(|comment| comment.id != id);
        if comments.len() == before {
            return false;
        }
        self.note = None;
        self.relayout();
        true
    }

    /// The message the review's comments make, or `None` without a comment.
    ///
    /// In the order the diff is read in — file by file as the column lists
    /// them, top to bottom within each — rather than the order they were
    /// made in, which is the order a person happened to scroll past them.
    pub(super) fn review(&self, branch: Option<&str>) -> Option<String> {
        let overview = self.overview()?;
        let comments = self.comments();
        if comments.is_empty() {
            return None;
        }
        let file_order = |path: &Path| {
            overview
                .files
                .iter()
                .position(|file| file.path == path)
                .unwrap_or(usize::MAX)
        };
        let mut ordered: Vec<&Comment> = comments.iter().collect();
        ordered.sort_by_key(|comment| {
            (
                file_order(&comment.anchor.path),
                comment.anchor.at,
                comment.id,
            )
        });
        Some(review::compose(
            &review::heading(branch, &overview.base),
            &ordered,
        ))
    }

    /// Says `note` where the review is, in place of whatever it said before.
    pub(super) fn say(&mut self, note: String) {
        self.note = Some(note);
    }

    /// Takes away what the review last said.
    pub(super) fn dismiss_note(&mut self) {
        self.note = None;
    }

    /// The review went to the agent: its comments are done with.
    pub(super) fn sent(&mut self) {
        if let Some(key) = self.key() {
            self.reviews.remove(&key);
        }
        self.relayout();
    }

    /// What the review last said.
    pub(super) fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// Lets go of every review but those of tabs `open` says are still open.
    pub(super) fn forget_tabs(&mut self, open: impl Fn(TabId) -> bool) {
        self.reviews.retain(|key, _| open(key.tab));
    }

    /// Drops the comments on files the list no longer has, and says which.
    fn drop_unlisted(&mut self) {
        let (Some(key), Reading::Ready(overview)) = (self.key(), &self.reading) else {
            return;
        };
        let listed: HashSet<&PathBuf> = overview.files.iter().map(|file| &file.path).collect();
        let mut dropped = Vec::new();
        if let Some(comments) = self.reviews.get_mut(&key) {
            comments.retain(|comment| {
                if listed.contains(&comment.anchor.path) {
                    return true;
                }
                dropped.push(review::location(&comment.anchor));
                false
            });
        }
        if let Some(note) = review::dropped(&dropped) {
            self.note = Some(note);
        }
    }

    /// The files with a comment on them, in the review being shown.
    fn commented(&self) -> HashSet<PathBuf> {
        self.comments()
            .iter()
            .map(|comment| comment.anchor.path.clone())
            .collect()
    }

    /// Rebuilds the rows and the running sum of their heights, from what is
    /// read and what is showing. Once per change, never per frame.
    pub(super) fn relayout(&mut self) {
        let rows = self.rows_now();
        let mut starts = Vec::with_capacity(rows.len() + 1);
        let mut total = 0.;
        starts.push(total);
        for row in &rows {
            total += row.height();
            starts.push(total);
        }
        self.rows = rows;
        self.starts = starts;
    }

    /// Every row the column would draw, top to bottom.
    fn rows_now(&self) -> Vec<Row> {
        if !self.open {
            return Vec::new();
        }
        if self.target.is_none() {
            return vec![Row::Message(
                "This tab has not said where it is working yet.".to_owned(),
            )];
        }
        let overview = match &self.reading {
            Reading::Nothing => return vec![Row::Message("Reading the repository…".to_owned())],
            Reading::Failed(error) => return vec![Row::Message(error.to_string())],
            Reading::Ready(overview) => overview,
        };

        let mut rows = Vec::new();
        let base = &overview.base;
        if base.against == Against::Branch {
            rows.push(Row::Heading(match overview.commits.commits.len() {
                0 => format!("No commits since {}", base.name),
                1 => format!("1 commit since {}", base.name),
                count => format!("{count} commits since {}", base.name),
            }));
            rows.extend((0..overview.commits.commits.len()).map(Row::Commit));
            if overview.commits.more {
                rows.push(Row::Message(format!(
                    "And more: the list stops at {MAX_COMMITS}."
                )));
            }
        }

        rows.push(Row::Heading(match overview.files.len() {
            0 => "No files changed".to_owned(),
            1 => "1 file changed".to_owned(),
            count => format!("{count} files changed"),
        }));
        for (index, file) in overview.files.iter().enumerate() {
            rows.push(Row::File(index));
            if !self.expanded.contains(&file.path) {
                continue;
            }
            rows.push(Row::Actions(index));
            if let Some((path, problem)) = &self.launch_failure
                && *path == file.path
            {
                rows.push(Row::Note(problem.clone()));
            }
            if file.status == Status::Repository {
                rows.push(Row::Note(
                    "A repository of its own: git does not look inside it.".to_owned(),
                ));
                continue;
            }
            let hunks = self.hunks.get(&file.path);
            match hunks.and_then(|hunks| hunks.diff.as_ref()) {
                None => rows.push(Row::Note("Reading…".to_owned())),
                Some(Err(problem)) => {
                    rows.push(Row::Note(format!("Could not read the diff: {problem}")));
                }
                Some(Ok(diff)) if diff.binary => {
                    rows.push(Row::Note("A binary file: no lines to show.".to_owned()));
                }
                Some(Ok(diff)) if diff.lines.is_empty() => {
                    rows.push(Row::Note("No lines changed.".to_owned()));
                }
                Some(Ok(diff)) => {
                    let mut comments = self.comments_on(&file.path).into_iter().peekable();
                    let draft = self
                        .draft
                        .as_ref()
                        .filter(|draft| draft.anchor.path == file.path && draft.adrift.is_none())
                        .map(|draft| draft.anchor.at);
                    for line in 0..diff.lines.len() {
                        rows.push(Row::Line(index, line));
                        while let Some(comment) =
                            comments.next_if(|comment| comment.anchor.at == line)
                        {
                            rows.push(Row::Comment(index, comment.id));
                        }
                        if draft == Some(line) {
                            rows.push(Row::Draft(index));
                        }
                    }
                    if diff.cut {
                        rows.push(Row::Note(match self.past_cut_on(&file.path) {
                            0 => "Cut short here. Open the file to see the rest.".to_owned(),
                            1 => "Cut short here, with 1 comment on a line past it. Open the \
                                  file to see the rest."
                                .to_owned(),
                            count => format!(
                                "Cut short here, with {count} comments on lines past it. Open \
                                 the file to see the rest."
                            ),
                        }));
                    }
                }
            }
        }
        if overview.more_files {
            rows.push(Row::Message(format!(
                "And more: the list stops at {MAX_FILES} files."
            )));
        }
        rows
    }

    /// Every row, in order.
    #[cfg(test)]
    pub(super) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Where row `index` starts; the list's end for one past the last.
    fn start(&self, index: usize) -> f32 {
        self.starts.get(index).copied().unwrap_or(0.)
    }

    /// How tall the whole list is.
    pub(super) fn total(&self) -> f32 {
        self.starts.last().copied().unwrap_or(0.)
    }

    /// The rows any part of which is in `[offset, offset + extent)`.
    ///
    /// The offset is clamped to the most the list can scroll in a box
    /// `measured` tall — what the scrollable's own layout is about to clamp
    /// it to — so that a list that just got shorter under an offset past its
    /// new end builds the rows that will be drawn rather than none. A binary
    /// search for the first, a count forward for the last: the cost is the
    /// rows in view, whatever the length of the list.
    pub(super) fn visible(&self, offset: f32, measured: f32, extent: f32) -> Range<usize> {
        let count = self.rows.len();
        if count == 0 {
            return 0..0;
        }
        let offset = offset.min((self.total() - measured).max(0.)).max(0.);
        let first = self
            .starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
            .min(count - 1);
        let bottom = offset + extent;
        let end = self
            .starts
            .partition_point(|start| *start < bottom)
            .min(count);
        first..end.max(first + 1)
    }
}

/// Moves every comment in `comments` on the file at `path` — and `draft`,
/// if it is on that file — to where its line is in `diff`, which has just
/// been read. Answers where each comment whose line is gone was, having
/// dropped it.
///
/// A diff cut short proves nothing about a line it stops before: a comment
/// not found in one is kept and marked past the cut, and a draft is set
/// adrift, as it is when its line is gone. A draft already adrift is looked
/// for all the same, and put back under its line when it is found: it kept
/// the line it was on, and what set it adrift — a read that failed, or was
/// cut, or an agent that has since written the line back — may be over.
///
/// Over the fields rather than the state, because the diff it reads is the
/// state's own and is borrowed while this runs.
fn follow_lines(
    path: &Path,
    diff: &FileDiff,
    comments: Option<&mut Vec<Comment>>,
    draft: &mut Option<Draft>,
) -> Vec<String> {
    let places = review::places(&diff.lines);
    let follow = |anchor: &mut Anchor| match review::reanchor(anchor, &diff.lines, &places) {
        Some((at, place)) => {
            anchor.at = at;
            anchor.place = place;
            anchor.line.clone_from(&diff.lines[at]);
            true
        }
        None => false,
    };

    let mut dropped = Vec::new();
    if let Some(comments) = comments {
        comments.retain_mut(|comment| {
            if comment.anchor.path != path {
                return true;
            }
            if follow(&mut comment.anchor) {
                comment.past_cut = false;
                return true;
            }
            if diff.cut {
                comment.past_cut = true;
                return true;
            }
            dropped.push(review::location(&comment.anchor));
            false
        });
    }
    if let Some(draft) = draft.as_mut().filter(|draft| draft.anchor.path == path) {
        draft.adrift = if follow(&mut draft.anchor) {
            None
        } else if diff.cut {
            Some(Adrift::PastCut)
        } else {
            Some(Adrift::Gone)
        };
    }
    dropped
}

/// The whole column.
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;

    // Its own width, ground and edge, because it is a docked column rather
    // than something inside one — the Themes panel's arrangement.
    ConstrainedBox::new(
        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(header(workspace, ui))
                .with_child(title_row(ui))
                .with_child(base_line(workspace, ui))
                .with_child(review_bar(workspace, ui))
                .with_child(Expanded::new(1., list(workspace, app)).finish())
                .finish(),
        )
        .with_background_color(theme().surface)
        .with_border(Border::right(1.).with_border_color(theme().border))
        .with_padding(Padding {
            top: 0.,
            bottom: PANEL_PADDING,
            left: PANEL_PADDING,
            right: PANEL_PADDING,
        })
        .finish(),
    )
    .with_width(PANEL_WIDTH)
    .finish()
}

/// Refresh and the ×, at the right end of the strip.
fn header(workspace: &Workspace, ui: FamilyId) -> Box<dyn Element> {
    let state = workspace.changes_panel();
    let refresh = text_button(
        state.control(Control::Refresh),
        "Refresh",
        // Inert while a read is under way: it would ask the question that is
        // already being answered. A dead button has no handler at all.
        (!state.is_reading()).then_some(ChangesAction::Refresh),
        ui,
    );

    ConstrainedBox::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
            .with_child(refresh)
            .with_child(
                Container::new(close_button(workspace))
                    .with_margin_left(6.)
                    .finish(),
            )
            .finish(),
    )
    .with_height(HEADER_HEIGHT)
    .finish()
}

/// What the column is called, under the strip — where the Themes panel puts
/// its own.
fn title_row(ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(TITLE, ui, 16.)
            .with_color(theme().text_primary)
            .with_style(Properties {
                weight: Weight::Semibold,
                ..Properties::default()
            })
            .finish(),
    )
    .with_margin_bottom(4.)
    .finish()
}

/// The one line under the title: which repository, and compared with what.
fn base_line(workspace: &Workspace, ui: FamilyId) -> Box<dyn Element> {
    let state = workspace.changes_panel();
    let text = match (state.overview(), state.target.as_deref()) {
        (Some(overview), _) => {
            let repository = name_of(&overview.repository);
            let base = &overview.base;
            match base.against {
                Against::Branch => {
                    let fork = base.fork.get(..7).unwrap_or(&base.fork);
                    format!("{repository}, since {} at {fork}", base.name)
                }
                Against::Head => {
                    format!("{repository}: no base branch, so what is not committed")
                }
                Against::Nothing => format!("{repository}: nothing is committed yet"),
            }
        }
        (None, Some(target)) => name_of(target),
        (None, None) => String::new(),
    };

    Container::new(
        Text::new(text, ui, 11.)
            .with_color(theme().text_muted)
            .with_ellipsis(Cut::End)
            .finish(),
    )
    .with_margin_bottom(4.)
    .finish()
}

/// The review's strip, at the top of the column where it is found without
/// scrolling: the two ways a review leaves the column, what the last of them
/// said, and a comment field a read left with no line to be under (see
/// [`Adrift`]). Nothing at all with none of those, which is how the column is
/// most of the time.
fn review_bar(workspace: &Workspace, ui: FamilyId) -> Box<dyn Element> {
    let state = workspace.changes_panel();
    let count = state.comments().len();
    let note = state.note();
    let adrift = state.adrift();
    if count == 0 && note.is_none() && adrift.is_none() {
        return Empty::new().finish();
    }

    let mut children: Vec<Box<dyn Element>> = Vec::new();
    if let Some(adrift) = adrift {
        children.push(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(
                    Paragraph::new(adrift.caption().to_owned(), ui, 11.)
                        .with_color(theme().text_muted)
                        .with_line_height_ratio(1.4)
                        .finish(),
                )
                .with_child(draft_row(workspace))
                .finish(),
        );
    }
    if count > 0 {
        let label = match count {
            1 => "Send 1 comment to the agent".to_owned(),
            count => format!("Send {count} comments to the agent"),
        };
        children.push(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Container::new(text_button(
                        state.control(Control::Send),
                        &label,
                        Some(ChangesAction::Send),
                        ui,
                    ))
                    .with_margin_right(6.)
                    .finish(),
                )
                .with_child(text_button(
                    state.control(Control::CopyReview),
                    "Copy review",
                    Some(ChangesAction::CopyReview),
                    ui,
                ))
                .finish(),
        );
    }
    if let Some(note) = note {
        // Wrapped rather than cut: a note is the answer to a press, and the
        // half of it an ellipsis would take is the half that says what to
        // do instead.
        children.push(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_child(
                    Expanded::new(
                        1.,
                        Paragraph::new(note.to_owned(), ui, 11.)
                            .with_color(theme().text_muted)
                            .with_line_height_ratio(1.4)
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(small_cross(
                    state.control(Control::DismissNote),
                    ChangesAction::DismissNote,
                ))
                .finish(),
        );
    }

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for (index, child) in children.into_iter().enumerate() {
        column.add_child(
            Container::new(child)
                .with_margin_top(if index == 0 { 0. } else { 6. })
                .finish(),
        );
    }
    Container::new(column.finish())
        .with_margin_bottom(8.)
        .finish()
}

/// A directory's last component, which is what a person calls a checkout.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The list, windowed: the rows in view between two spacers that stand for
/// the rest. See the module's own docs.
fn list(workspace: &Workspace, _: &AppContext) -> Box<dyn Element> {
    let state = workspace.changes_panel();
    let (offset, measured) = {
        let scroll = state.scroll.lock();
        (scroll.offset(), scroll.viewport())
    };
    // The box can never be taller than the window, and the window's height
    // is known before the box has been laid out — which a resize and the
    // frame that opened the column both come before.
    let extent = match measured.max(workspace.window_height()) {
        known if known > 0. => known,
        _ => UNMEASURED_EXTENT,
    };
    let range = state.visible(offset, measured, extent);

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(spacer(state.start(range.start)));
    for index in range.clone() {
        column.add_child(row(workspace, &state.rows[index]));
    }
    column.add_child(spacer(state.total() - state.start(range.end)));
    let column = Container::new(column.finish())
        .with_padding_right(THUMB_ROOM)
        .finish();

    Scrollable::new(state.scroll.clone(), column)
        .with_scrollbar(theme().overlay_3)
        .finish()
}

/// Nothing, `height` tall: the rows a frame did not build.
fn spacer(height: f32) -> Box<dyn Element> {
    ConstrainedBox::new(Empty::new().finish())
        .with_height(height.max(0.))
        .finish()
}

/// One row, at its kind's height.
fn row(workspace: &Workspace, row: &Row) -> Box<dyn Element> {
    let fonts = workspace.fonts();
    let state = workspace.changes_panel();
    let overview = state.overview();

    let content = match row {
        Row::Heading(text) => Align::new(
            Text::new(text.clone(), fonts.ui, 11.)
                .with_color(theme().text_muted)
                .with_style(Properties {
                    weight: Weight::Semibold,
                    ..Properties::default()
                })
                .with_ellipsis(Cut::End)
                .finish(),
        )
        .bottom_left()
        .finish(),
        Row::Message(text) => sentence(text, 0., fonts.ui),
        Row::Note(text) => sentence(text, INDENT, fonts.ui),
        Row::Commit(index) => {
            match overview.and_then(|overview| overview.commits.commits.get(*index)) {
                Some(commit) => commit_row(commit, fonts),
                None => Empty::new().finish(),
            }
        }
        Row::File(index) => match overview.and_then(|overview| overview.files.get(*index)) {
            Some(file) => file_row(workspace, *index, file, fonts),
            None => Empty::new().finish(),
        },
        Row::Actions(index) => match overview.and_then(|overview| overview.files.get(*index)) {
            Some(file) => actions_row(workspace, *index, file, fonts.ui),
            None => Empty::new().finish(),
        },
        Row::Line(file, line) => {
            let text = overview
                .and_then(|overview| overview.files.get(*file))
                .and_then(|file| state.diff(&file.path))
                .and_then(|diff| diff.lines.get(*line));
            match text {
                Some(text) if review::commentable(text) => {
                    commentable_line(workspace, *file, *line, text, fonts.monospace)
                }
                Some(text) => diff_line(text, fonts.monospace),
                None => Empty::new().finish(),
            }
        }
        Row::Comment(_, id) => match state.comments().iter().find(|comment| comment.id == *id) {
            Some(comment) => comment_row(workspace, comment, fonts.ui),
            None => Empty::new().finish(),
        },
        Row::Draft(_) => draft_row(workspace),
    };

    ConstrainedBox::new(content)
        .with_height(row.height())
        .finish()
}

/// A line of text in the muted ink, left-aligned in its row.
fn sentence(text: &str, indent: f32, ui: FamilyId) -> Box<dyn Element> {
    Align::new(
        Container::new(
            Text::new(text.to_owned(), ui, 11.)
                .with_color(theme().text_muted)
                .with_ellipsis(Cut::End)
                .finish(),
        )
        .with_padding_left(indent)
        .finish(),
    )
    .left()
    .finish()
}

/// A commit: its short id, its subject, and when.
fn commit_row(commit: &crate::git::changes::Commit, fonts: super::Fonts) -> Box<dyn Element> {
    Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(
                Text::new(commit.sha.clone(), fonts.monospace, 11.)
                    .with_color(theme().text_muted)
                    .finish(),
            )
            .with_margin_right(8.)
            .finish(),
        )
        .with_child(
            Expanded::new(
                1.,
                Align::new(
                    Text::new(commit.subject.clone(), fonts.ui, 12.)
                        .with_color(theme().text_primary)
                        .with_ellipsis(Cut::End)
                        .finish(),
                )
                .left()
                .finish(),
            )
            .finish(),
        )
        .with_child(
            Container::new(
                Text::new(commit.when.clone(), fonts.ui, 10.)
                    .with_color(theme().text_muted)
                    .finish(),
            )
            .with_margin_left(8.)
            .finish(),
        )
        .finish()
}

/// The colour a status letter is drawn in: the chip's two for what came and
/// went, and the muted ink for everything that only changed.
fn status_color(status: Status) -> Color {
    match status {
        Status::Added | Status::Untracked | Status::Copied => theme().diff_added,
        Status::Deleted => theme().diff_removed,
        Status::Modified
        | Status::Renamed
        | Status::TypeChanged
        | Status::Unmerged
        | Status::Repository => theme().text_muted,
    }
}

/// A file: a chevron that says whether its hunks are showing, its status
/// letter, and its path — cut from the front, because the file's own name is
/// the end of it.
fn file_row(
    workspace: &Workspace,
    index: usize,
    file: &FileChange,
    fonts: super::Fonts,
) -> Box<dyn Element> {
    let state = workspace.changes_panel();
    let expanded = state.is_expanded(&file.path);
    let path = match &file.from {
        Some(from) => format!("{} → {}", from.display(), file.path.display()),
        None => file.path.display().to_string(),
    };
    let letter = file.status.letter().to_string();
    let color = status_color(file.status);

    Hoverable::new(state.control(Control::File(index)), move |mouse| {
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    ConstrainedBox::new(
                        Align::new(
                            Icon::new(
                                if expanded {
                                    Lucide::ChevronDown
                                } else {
                                    Lucide::ChevronRight
                                },
                                10.,
                            )
                            .with_color(theme().text_muted)
                            .finish(),
                        )
                        .finish(),
                    )
                    .with_width(CHEVRON_WIDTH)
                    .finish(),
                )
                .with_child(
                    ConstrainedBox::new(
                        Align::new(
                            Text::new(letter.clone(), fonts.monospace, 11.)
                                .with_color(color)
                                .finish(),
                        )
                        .left()
                        .finish(),
                    )
                    .with_width(LETTER_WIDTH)
                    .finish(),
                )
                .with_child(
                    Expanded::new(
                        1.,
                        Align::new(
                            Text::new(path.clone(), fonts.ui, 12.)
                                .with_color(theme().text_primary)
                                .with_ellipsis(Cut::Start)
                                .finish(),
                        )
                        .left()
                        .finish(),
                    )
                    .finish(),
                )
                .finish(),
        )
        .with_background_color(if mouse.is_hovered() || expanded {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish()
    })
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Changes(ChangesAction::ToggleFile(index)));
    })
    .finish()
}

/// A shown file's buttons. Each is there only when it can do something: no
/// "Open" for a file that is gone, for a nested repository, which is a
/// directory, or with no editor named; no "Copy diff" until there is a whole
/// one to copy.
///
/// "Open" and not "Open in code": an editor's name is whatever `$VISUAL`
/// says, a launcher script's included, and a name long enough pushed the
/// last button off the column's edge.
fn actions_row(
    workspace: &Workspace,
    index: usize,
    file: &FileChange,
    ui: FamilyId,
) -> Box<dyn Element> {
    let state = workspace.changes_panel();
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center);

    if !matches!(file.status, Status::Deleted | Status::Repository) && state.editor().is_some() {
        row.add_child(
            Container::new(text_button(
                state.control(Control::Open(index)),
                "Open",
                Some(ChangesAction::OpenFile(index)),
                ui,
            ))
            .with_margin_right(6.)
            .finish(),
        );
    }
    row.add_child(
        Container::new(text_button(
            state.control(Control::CopyPath(index)),
            "Copy path",
            Some(ChangesAction::CopyPath(index)),
            ui,
        ))
        .with_margin_right(6.)
        .finish(),
    );
    // A cut diff is a patch with its end missing, and pasting it anywhere —
    // to an agent, into `git apply` — would say something false about the
    // change. The binary sentence is not a patch either.
    if state
        .diff(&file.path)
        .is_some_and(|diff| !diff.binary && !diff.cut && !diff.patch.is_empty())
    {
        row.add_child(text_button(
            state.control(Control::CopyDiff(index)),
            "Copy diff",
            Some(ChangesAction::CopyDiff(index)),
            ui,
        ));
    }

    Container::new(row.finish())
        .with_padding_left(INDENT)
        .finish()
}

/// One line of a hunk, in the colour of what happened to it.
fn diff_line(text: &str, monospace: FamilyId) -> Box<dyn Element> {
    let color = match text.as_bytes().first() {
        Some(b'+') => theme().diff_added,
        Some(b'-') => theme().diff_removed,
        Some(b'@') => theme().accent,
        _ => theme().text_muted,
    };
    // A tab is drawn as whatever the font has for it, which is a box or
    // nothing; four spaces is what a diff viewer shows.
    let shown = text.replace('\t', "    ");

    Align::new(
        Text::new(shown, monospace, 11.)
            .with_color(color)
            .with_ellipsis(Cut::End)
            .finish(),
    )
    .left()
    .finish()
}

/// A changed line, or a hunk's header, which a press leaves a comment on.
///
/// The `+` at its end appears under the pointer and says so; the whole row
/// takes the press, because a sixteen-pixel line is target enough and a
/// twelve-pixel mark at the far end of it is not.
fn commentable_line(
    workspace: &Workspace,
    file: usize,
    line: usize,
    text: &str,
    monospace: FamilyId,
) -> Box<dyn Element> {
    let state = workspace.changes_panel();
    let text = text.to_owned();
    Hoverable::new(state.control(Control::Line(file, line)), move |mouse| {
        let hovered = mouse.is_hovered();
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1., diff_line(&text, monospace)).finish());
        if hovered {
            row.add_child(
                Icon::new(Lucide::Plus, 10.)
                    .with_color(theme().accent)
                    .finish(),
            );
        }
        Container::new(row.finish())
            .with_background_color(if hovered {
                theme().overlay_1
            } else {
                Color::TRANSPARENT
            })
            .finish()
    })
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Changes(ChangesAction::Comment {
            file,
            line,
        }));
    })
    .finish()
}

/// A comment, under its line: a bar in the accent down its left edge, so it
/// reads as a note on the code rather than as more of it, what was said, and
/// a × that takes it away.
fn comment_row(workspace: &Workspace, comment: &Comment, ui: FamilyId) -> Box<dyn Element> {
    let state = workspace.changes_panel();
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Expanded::new(
                    1.,
                    Align::new(
                        Text::new(comment.text.clone(), ui, 12.)
                            .with_color(theme().text_primary)
                            .with_ellipsis(Cut::End)
                            .finish(),
                    )
                    .left()
                    .finish(),
                )
                .finish(),
            )
            .with_child(small_cross(
                state.control(Control::RemoveComment(comment.id)),
                ChangesAction::RemoveComment(comment.id),
            ))
            .finish(),
    )
    .with_padding_left(8.)
    .with_margin_top(2.)
    .with_margin_bottom(2.)
    .with_background_color(theme().overlay_1)
    .with_border(Border::left(2.).with_border_color(theme().accent))
    .finish()
}

/// The field a comment is typed in, under the line it is about — or in the
/// review's strip, under what it says, when a read has set it adrift.
///
/// One line: [`TextField`](super::text_field::TextField) has no second, so
/// Shift-Enter does nothing here. Enter keeps the comment and Escape drops
/// it — both taken by the workspace before the field sees them, see
/// [`Workspace::changes_takes_keys`].
fn draft_row(workspace: &Workspace) -> Box<dyn Element> {
    let state = workspace.changes_panel();
    let Some(input) = state.draft_input() else {
        return Empty::new().finish();
    };
    Container::new(
        super::text_field::TextField::new(
            input.clone(),
            workspace.clipboard().clone(),
            workspace.fonts(),
            state.control(Control::Draft),
            "Comment for the agent · Enter adds it",
        )
        .with_focus(WorkspaceAction::Changes(ChangesAction::FocusComment))
        .finish(),
    )
    .with_vertical_padding(4.)
    .finish()
}

/// A 16px × that sends `action`: a comment's, and the note's.
fn small_cross(state: MouseStateHandle, action: ChangesAction) -> Box<dyn Element> {
    Hoverable::new(state, move |mouse| {
        let color = if mouse.is_hovered() {
            theme().text_primary
        } else {
            theme().text_muted
        };
        ConstrainedBox::new(
            Align::new(Icon::new(Lucide::X, 10.).with_color(color).finish()).finish(),
        )
        .with_width(16.)
        .with_height(16.)
        .finish()
    })
    .on_click(move |_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Changes(action));
    })
    .finish()
}

/// A small button with a word on it. `None` draws it muted and leaves it
/// without a handler — CONTRIBUTING's rule for a control that can do
/// nothing right now.
fn text_button(
    state: MouseStateHandle,
    label: &str,
    action: Option<ChangesAction>,
    ui: FamilyId,
) -> Box<dyn Element> {
    let label = label.to_owned();
    let live = action.is_some();
    let button = Hoverable::new(state, move |mouse| {
        Container::new(
            Text::new(label.clone(), ui, 11.)
                .with_color(if live {
                    theme().text_primary
                } else {
                    theme().text_muted
                })
                .finish(),
        )
        .with_padding(Padding {
            top: 3.,
            bottom: 3.,
            left: 7.,
            right: 7.,
        })
        .with_background_color(if live && mouse.is_hovered() {
            theme().overlay_2
        } else {
            Color::TRANSPARENT
        })
        .with_border(Border::all(1.).with_border_color(theme().border))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish()
    });
    match action {
        Some(action) => button
            .on_click(move |_, ctx, _| {
                ctx.dispatch_typed_action(WorkspaceAction::Changes(action));
            })
            .finish(),
        None => button.finish(),
    }
}

/// The × in the corner.
fn close_button(workspace: &Workspace) -> Box<dyn Element> {
    let state = workspace.changes_panel().control(Control::Close);

    Hoverable::new(state, move |mouse| {
        let (background, color) = if mouse.is_hovered() {
            (theme().overlay_2, theme().text_primary)
        } else {
            (Color::TRANSPARENT, theme().text_muted)
        };

        Container::new(
            ConstrainedBox::new(
                Align::new(Icon::new(Lucide::X, ICON_SIZE).with_color(color).finish()).finish(),
            )
            .with_width(20.)
            .with_height(20.)
            .finish(),
        )
        .with_background_color(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .finish()
    })
    .on_click(|_, ctx, _| {
        ctx.dispatch_typed_action(WorkspaceAction::Changes(ChangesAction::Close));
    })
    .finish()
}
