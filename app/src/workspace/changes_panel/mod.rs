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

#[cfg(test)]
mod tests;

pub(crate) use launch::Editor;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use crookui_core::elements::{MouseStateHandle, Padding};
use crookui_core::fonts::{FamilyId, Properties, Weight};
use crookui_core::prelude::*;

use crate::git::changes::{
    Against, Base, Error, FileChange, FileDiff, MAX_COMMITS, MAX_FILES, Overview, Status,
};
use crate::theme::theme;

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

    /// Whether a read is under way.
    pub(super) fn is_reading(&self) -> bool {
        self.in_flight
    }

    /// Puts the column up, looking at `target`, and answers the read to
    /// start: `None` when there is nowhere to read.
    pub(super) fn open(&mut self, target: Option<PathBuf>, editor: Option<Editor>) -> Option<u64> {
        self.open = true;
        self.editor = editor;
        self.retarget(target)
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
        self.forget_hover_state();
        self.relayout();
    }

    /// Starts looking somewhere else, and answers the read to start.
    pub(super) fn retarget(&mut self, target: Option<PathBuf>) -> Option<u64> {
        self.target = target;
        self.reading = Reading::Nothing;
        self.expanded.clear();
        self.hunks.clear();
        self.launch_failure = None;
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
    /// everything else.
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
                // A diff nobody is looking at is read again when somebody
                // does, rather than kept on the strength of a list that has
                // just been read again itself.
                let expanded = &self.expanded;
                self.hunks.retain(|path, _| expanded.contains(path));
                self.reading = Reading::Ready(overview);
            }
            Err(error) => {
                self.reading = Reading::Failed(error);
                self.expanded.clear();
                self.hunks.clear();
            }
        }

        let mut again: Vec<PathBuf> = self.expanded.iter().cloned().collect();
        again.sort();
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
        let Some(hunks) = self.hunks.get_mut(path) else {
            return false;
        };
        if hunks.ticket != Some(ticket) {
            return false;
        }
        hunks.ticket = None;
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
                    rows.extend((0..diff.lines.len()).map(|line| Row::Line(index, line)));
                    if diff.cut {
                        rows.push(Row::Note(
                            "Cut short here. Open the file to see the rest.".to_owned(),
                        ));
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
                Some(text) => diff_line(text, fonts.monospace),
                None => Empty::new().finish(),
            }
        }
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
