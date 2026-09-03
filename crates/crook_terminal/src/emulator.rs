//! The terminal emulator: bytes in, a drawable grid out.
//!
//! [`Emulator`] wraps `alacritty_terminal`'s `Term`, which owns the grid, the
//! scrollback, the cursor, the alternate screen and the mode flags. This module
//! supplies the three things `Term` does not: the event listener it reports
//! through, an OSC 7 reader for the working directory (which no layer below
//! this one interprets), and the translation of both into a [`Snapshot`] and a
//! queue of [`TerminalEvent`]s.
//!
//! It owns no pty and no thread, so it is the whole emulator under test: feed
//! it bytes with [`Emulator::advance`] and read [`Emulator::snapshot`]. The pty
//! is bolted on by [`crate::Terminal`].
//!
//! Some of what the child sends is a question rather than output — "what colour
//! is index 4?", "how big is the text area?", a device attributes request. The
//! emulator answers those into [`Emulator::take_replies`], and it is the
//! caller's job to put those bytes back on the pty; a caller that drops them
//! will hang any program that waits for an answer.
//!
//! The pointer's selection is kept here too, in `Term`'s own `selection` field
//! rather than beside it, and that is not an implementation detail: every path
//! in `Term` that scrolls the grid rotates the selection with the text as it
//! goes, so a selection made around a word stays around that word while a
//! hundred more lines print underneath it. A copy kept anywhere else would have
//! to reproduce that, and would be wrong the first time a `\n` arrived. See
//! [`crate::selection`].

use std::path::{Path, PathBuf};
use std::str;
use std::sync::Arc;
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::selection::Selection;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::vte::{Parser, Perform};
use parking_lot::Mutex;

use crate::input::InputModes;
use crate::pty::ChildExit;
use crate::selection::{CellSide, GridPoint, SelectionKind, SelectionSpan, ViewportPoint};
use crate::snapshot::{self, Palette, Snapshot, TerminalSize};

/// Something the child process asked the surrounding application to do.
///
/// These are the events a grid cannot express. They queue up in the emulator
/// and are drained with [`Emulator::take_events`]; an application that ignores
/// them still renders correctly, it just never rings, retitles or closes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TerminalEvent {
    /// The title changed — `None` when the child reset it to the default. The
    /// current value is also on every [`Snapshot`].
    Title(Option<String>),
    /// The child reported its working directory with OSC 7.
    WorkingDirectory(PathBuf),
    /// The bell rang.
    Bell,
    /// The child asked the terminal to close.
    Exit,
    /// The child process finished. Raised by [`crate::Terminal`], not by the
    /// escape sequence stream.
    ChildExited(ChildExit),
    /// The child asked for text to be put on the system clipboard with OSC 52.
    ClipboardStore(String),
}

/// Collects `Term`'s events so they can be handled after parsing, rather than
/// re-entrantly in the middle of it.
///
/// `Term` calls its listener while it holds itself mutably, so the listener
/// cannot look at the terminal — it can only record. Translation happens in
/// [`Emulator::drain`], which does have the terminal and the palette.
#[derive(Clone, Default)]
struct EventProxy(Arc<Mutex<Vec<Event>>>);

impl EventProxy {
    fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.0.lock())
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        self.0.lock().push(event);
    }
}

/// Watches the byte stream for OSC 7, the working-directory report.
///
/// `vte` parses OSC 7 into params and then drops it, because alacritty resolves
/// the child's directory from its process id instead — a route that needs
/// per-platform process introspection this crate deliberately does not have. So
/// the bytes get a second pass through `vte`'s own parser with a `Perform` that
/// implements nothing but `osc_dispatch`. That costs one extra walk of the
/// stream, which is a table-driven byte loop that does nothing at all outside
/// an escape sequence, and it buys a correct OSC parser instead of a hand-
/// rolled scanner that has to get chunk boundaries right.
#[derive(Default)]
struct WorkingDirectoryWatcher {
    reported: Option<PathBuf>,
}

impl Perform for WorkingDirectoryWatcher {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.len() < 2 || params[0] != b"7" {
            return;
        }
        if let Some(path) = parse_working_directory(params[1]) {
            self.reported = Some(path);
        }
    }
}

/// Reads the payload of OSC 7, which is a `file://` URL or a bare path.
///
/// The host in the URL is ignored: a shell on the far end of an ssh session
/// reports a path that means nothing locally, but showing it still beats
/// showing nothing, and the caller can decide.
fn parse_working_directory(value: &[u8]) -> Option<PathBuf> {
    let value = str::from_utf8(value).ok()?;
    let path = match value.strip_prefix("file://") {
        Some(authority_and_path) => authority_and_path
            .find('/')
            .map(|at| &authority_and_path[at..])?,
        None => value,
    };

    let decoded = percent_decode(path)?;
    // A Windows path arrives as `/C:/Users/...`; the leading slash is part of
    // the URL, not of the path.
    let decoded = if cfg!(windows) && is_drive_prefixed(&decoded) {
        decoded[1..].to_owned()
    } else {
        decoded
    };

    (!decoded.is_empty()).then(|| PathBuf::from(decoded))
}

/// Whether a URL path is a Windows path with its leading slash still attached.
fn is_drive_prefixed(path: &str) -> bool {
    let mut chars = path.chars();
    chars.next() == Some('/')
        && chars
            .next()
            .is_some_and(|drive| drive.is_ascii_alphabetic())
        && chars.next() == Some(':')
}

/// Undoes the percent-encoding an OSC 7 URL uses for spaces and non-ASCII.
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let digits = str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
                decoded.push(u8::from_str_radix(digits, 16).ok()?);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// A terminal grid being driven by a stream of bytes.
pub struct Emulator {
    term: Term<EventProxy>,
    parser: Processor,
    working_directory_parser: Parser,
    working_directory_watcher: WorkingDirectoryWatcher,
    proxy: EventProxy,
    palette: Palette,
    size: TerminalSize,
    title: Option<String>,
    working_directory: Option<PathBuf>,
    events: Vec<TerminalEvent>,
    replies: Vec<u8>,
    snapshot: Arc<Snapshot>,
    /// Whether something has happened since the cached snapshot was built.
    ///
    /// Alacritty's own damage tracking is not used for this: `Term::damage`
    /// unconditionally reports the cursor's cell as damaged, so it can never
    /// say "nothing changed". This flag says "worth looking", and
    /// [`Emulator::snapshot`] settles it by comparing what it builds against
    /// what it already had.
    dirty: bool,
    /// Whether the pointer has moved the selection since the cached snapshot.
    ///
    /// Kept apart from [`Self::dirty`] because it is the cheap half: nothing
    /// the child printed has changed, so the cells of the cached snapshot are
    /// still the right cells and only the span beside them has to be replaced.
    /// See [`Emulator::snapshot`].
    selection_dirty: bool,
}

impl Emulator {
    /// A terminal of the given size, drawing with `palette` and keeping
    /// `scrollback_lines` of history above the viewport.
    pub fn new(size: TerminalSize, scrollback_lines: usize, palette: Palette) -> Self {
        let proxy = EventProxy::default();
        let config = Config {
            scrolling_history: scrollback_lines,
            ..Config::default()
        };
        let mut term = Term::new(config, &size, proxy.clone());
        // `Term` starts fully damaged so a fresh renderer paints everything.
        // This crate tracks that itself, and its first snapshot is revision 0.
        term.reset_damage();

        let snapshot = Arc::new(snapshot::build(&term, &palette, 0, None));
        Self {
            term,
            parser: Processor::new(),
            working_directory_parser: Parser::new(),
            working_directory_watcher: WorkingDirectoryWatcher::default(),
            proxy,
            palette,
            size,
            title: None,
            working_directory: None,
            events: Vec::new(),
            replies: Vec::new(),
            snapshot,
            dirty: false,
            selection_dirty: false,
        }
    }

    /// Feeds output from the child process into the grid.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.expire_sync();
        if bytes.is_empty() {
            return;
        }
        self.parser.advance(&mut self.term, bytes);
        self.working_directory_parser
            .advance(&mut self.working_directory_watcher, bytes);
        self.dirty = true;
        self.drain();
    }

    /// The visible grid.
    ///
    /// Returns the very same `Arc` — and so the same revision — until the drawn
    /// content actually differs, so a caller may compare revisions, or the
    /// pointers, to decide whether to repaint.
    ///
    /// **A selection that moved does not rebuild the grid.** Dragging one out
    /// is a stream of pointer moves, none of which changes a cell the child
    /// printed, so the cells of the last snapshot are still exactly the right
    /// cells: they are copied across and only the span beside them is replaced.
    /// That is a memcpy of an already-built `Vec`, where rebuilding would be a
    /// walk of the emulator's grid with a palette resolution per cell — the
    /// whole reason [`Snapshot::selection`] is a span and not a flag on every
    /// [`SnapshotCell`](crate::SnapshotCell).
    pub fn snapshot(&mut self) -> Arc<Snapshot> {
        // A program that emitted a synchronized update and then died never
        // sends the bytes that would end it, and no more output will arrive to
        // notice that. Painting is the other moment the question comes up.
        self.expire_sync();
        if self.dirty {
            self.dirty = false;
            self.selection_dirty = false;

            let revision = self.snapshot.revision + 1;
            let built = snapshot::build(&self.term, &self.palette, revision, self.title.as_deref());
            if !built.same_content(&self.snapshot) {
                self.snapshot = Arc::new(built);
            }
            return Arc::clone(&self.snapshot);
        }

        if self.selection_dirty {
            self.selection_dirty = false;
            let selection = self.selection();
            if selection != self.snapshot.selection {
                self.snapshot = Arc::new(Snapshot {
                    revision: self.snapshot.revision + 1,
                    selection,
                    ..(*self.snapshot).clone()
                });
            }
        }
        Arc::clone(&self.snapshot)
    }

    /// Whether anything has happened that might have changed the grid. A cheap
    /// "should I bother taking a snapshot?".
    pub fn is_dirty(&self) -> bool {
        self.dirty || self.selection_dirty
    }

    /// Changes the size of the grid, reflowing the scrollback into it.
    ///
    /// This does not tell the child process anything; only the pty can do that.
    /// [`crate::Terminal::resize`] does both.
    pub fn resize(&mut self, size: TerminalSize) {
        let geometry_changed = size.columns != self.size.columns || size.rows != self.size.rows;
        self.size = size;
        if !geometry_changed {
            return;
        }
        self.term.resize(size);
        self.dirty = true;
        self.drain();
    }

    /// The size the grid was last set to.
    pub fn size(&self) -> TerminalSize {
        self.size
    }

    /// Moves the viewport through the scrollback: a positive `delta` moves it
    /// back into history, a negative one towards the live output. It stops at
    /// the oldest line and at the bottom rather than wrapping.
    pub fn scroll_lines(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
        self.dirty = true;
        self.drain();
    }

    /// Returns the viewport to the live output.
    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
        self.dirty = true;
        self.drain();
    }

    /// How many lines of scrollback sit above the viewport.
    pub fn history_len(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Which cell of the text a cell of the viewport is showing.
    ///
    /// The one conversion that needs the emulator, and the reason
    /// [`ViewportPoint`] and [`GridPoint`] are separate types: a row of the
    /// screen is a different line of the text after every scroll, and a
    /// selection is kept in lines so that it stays on the words it was drawn
    /// around.
    pub fn grid_point(&self, at: ViewportPoint) -> GridPoint {
        let display_offset = self.term.grid().display_offset();
        GridPoint::new(at.row as i32 - display_offset as i32, at.column)
    }

    /// Starts a selection at a cell of the viewport, replacing any there was.
    ///
    /// A [`SelectionKind::Simple`] selection that is never dragged anywhere is
    /// empty, and an empty selection is no selection: that is what makes a
    /// plain click clear the last one rather than leave a one-cell highlight
    /// behind.
    pub fn start_selection(&mut self, kind: SelectionKind, at: ViewportPoint, side: CellSide) {
        let point = self.grid_point(at);
        self.term.selection = Some(Selection::new(kind.into(), point.into(), side.into()));
        self.selection_dirty = true;
    }

    /// Drags the open end of the selection to a cell of the viewport.
    ///
    /// Does nothing when no selection has been started, so a drag that began
    /// somewhere else cannot pull one out of this grid.
    pub fn update_selection(&mut self, at: ViewportPoint, side: CellSide) {
        let point = self.grid_point(at);
        let Some(selection) = self.term.selection.as_mut() else {
            return;
        };
        selection.update(point.into(), side.into());
        self.selection_dirty = true;
    }

    /// Drops the selection.
    pub fn clear_selection(&mut self) {
        if self.term.selection.take().is_some() {
            self.selection_dirty = true;
        }
    }

    /// What is selected, as cells of the grid, or `None` when nothing is.
    pub fn selection(&self) -> Option<SelectionSpan> {
        snapshot::selection_of(&self.term)
    }

    /// Whether there is anything selected to copy.
    ///
    /// Not `self.term.selection.is_some()`: an empty selection — a press with
    /// no drag behind it, or one whose text has scrolled out of the history —
    /// is a selection object with no cells in it.
    pub fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    /// The selected text, laid out the way the screen shows it: one line per
    /// row, and a wrapped line joined back into one.
    ///
    /// Asked of [`Self::selection`] first, and not for tidiness: that is the
    /// one place that decides whether the range alacritty built is a range the
    /// grid can be walked with, and `Term::selection_to_string` walks it with
    /// no such check — a block range it built inverted would index a row one
    /// column past its end. The text and the highlight come back from the same
    /// decision, so they cannot disagree about what is selected.
    pub fn selection_text(&self) -> Option<String> {
        self.selection()?;
        self.term.selection_to_string()
    }

    /// The title the child last asked for.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The working directory the child last reported with OSC 7. `None` until a
    /// shell reports one, which most do only once they are configured to.
    pub fn working_directory(&self) -> Option<&Path> {
        self.working_directory.as_deref()
    }

    /// Everything the child has asked for since the last call.
    pub fn take_events(&mut self) -> Vec<TerminalEvent> {
        std::mem::take(&mut self.events)
    }

    /// The bytes owed back to the child, in answer to its queries. The caller
    /// must write these to the pty.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// Queues an event for the application, used by [`crate::Terminal`] to
    /// report things the escape stream cannot, such as the child exiting.
    pub(crate) fn push_event(&mut self, event: TerminalEvent) {
        self.events.push(event);
    }

    /// Which encoding the child currently expects for cursor and keypad keys.
    pub fn input_modes(&self) -> InputModes {
        InputModes {
            application_cursor: self.term.mode().contains(TermMode::APP_CURSOR),
            application_keypad: self.term.mode().contains(TermMode::APP_KEYPAD),
        }
    }

    /// Whether the child asked for pasted text to be bracketed, so it can tell
    /// a paste from typing.
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Whether the alternate screen is active, which is how a full-screen
    /// program announces itself.
    pub fn is_alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// The colours the grid is resolved through.
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// Redraws the grid in a different set of colours, which is how a theme
    /// change reaches an already-running session.
    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
        self.dirty = true;
    }

    /// Ends a synchronized update that has outstayed its welcome.
    ///
    /// `\x1b[?2026h` tells the parser to buffer everything until `\x1b[?2026l`
    /// arrives, so a full-screen program can redraw without being seen
    /// half-drawn. The escape carries no promise that the second half will ever
    /// come: a program that is killed, crashes or hangs between the two leaves
    /// the parser buffering forever, and the pane would keep drawing the frame
    /// from before the update for the rest of its life.
    ///
    /// `vte` has the 150 ms deadline the specification asks for but does not
    /// enforce it — `pending_timeout` only answers "is one running", never "has
    /// it expired" — so enforcing it is the driver's job, exactly as it is in
    /// alacritty's own event loop.
    fn expire_sync(&mut self) {
        let expired = self
            .parser
            .sync_timeout()
            .sync_timeout()
            .is_some_and(|deadline| Instant::now() >= deadline);
        if !expired {
            return;
        }

        self.parser.stop_sync(&mut self.term);
        self.dirty = true;
        self.drain();
    }

    /// Translates everything `Term` reported during the last operation.
    fn drain(&mut self) {
        for event in self.proxy.take() {
            match event {
                Event::Title(title) => self.set_title(Some(title)),
                Event::ResetTitle => self.set_title(None),
                Event::Bell => self.events.push(TerminalEvent::Bell),
                Event::Exit => self.events.push(TerminalEvent::Exit),
                Event::ChildExit(status) => {
                    let exit = ChildExit::from_code(status.code().unwrap_or(1) as u32);
                    self.events.push(TerminalEvent::ChildExited(exit));
                }
                Event::ClipboardStore(_, text) => {
                    self.events.push(TerminalEvent::ClipboardStore(text));
                }
                Event::PtyWrite(text) => self.replies.extend_from_slice(text.as_bytes()),
                Event::ColorRequest(index, format) => {
                    let color = self.palette.color_at(index, self.term.colors());
                    self.replies
                        .extend_from_slice(format(color.into()).as_bytes());
                }
                Event::TextAreaSizeRequest(format) => {
                    let size = WindowSize {
                        num_lines: self.size.rows,
                        num_cols: self.size.columns,
                        cell_width: self.size.cell_width,
                        cell_height: self.size.cell_height,
                    };
                    self.replies.extend_from_slice(format(size).as_bytes());
                }
                // Reading the clipboard is refused rather than answered: OSC 52
                // paste hands any program that can print to the terminal the
                // contents of the user's clipboard.
                Event::ClipboardLoad(..)
                | Event::CursorBlinkingChange
                | Event::MouseCursorDirty
                | Event::Wakeup => {}
            }
        }

        if let Some(directory) = self.working_directory_watcher.reported.take()
            && self.working_directory.as_deref() != Some(directory.as_path())
        {
            self.working_directory = Some(directory.clone());
            self.events.push(TerminalEvent::WorkingDirectory(directory));
        }
    }

    /// Records a new title, which every future snapshot carries.
    fn set_title(&mut self, title: Option<String>) {
        if self.title == title {
            return;
        }
        self.title = title.clone();
        self.dirty = true;
        self.events.push(TerminalEvent::Title(title));
    }
}

#[cfg(test)]
#[path = "emulator_tests.rs"]
mod tests;
