//! The terminal emulator: bytes in, a drawable grid out.
//!
//! [`Emulator`] wraps `alacritty_terminal`'s `Term`, which owns the grid, the
//! scrollback, the cursor, the alternate screen and the mode flags. This module
//! supplies the three things `Term` does not: the event listener it reports
//! through, a reader for the OSC sequences `vte` throws away — OSC 7 for the
//! working directory, OSC 133 for command boundaries, the notifications other
//! terminals show — and the translation of all of it into a [`Snapshot`] and
//! a queue of [`TerminalEvent`]s.
//!
//! It owns no pty and no thread, so it is the whole emulator under test: feed
//! it bytes with [`Emulator::advance`] and read [`Emulator::snapshot`]. The pty
//! is bolted on by [`crate::Terminal`].
//!
//! Some of what the child sends is a question rather than output — "what colour
//! is index 4?", "how big is the text area?", a device attributes request. The
//! emulator answers those into [`Emulator::take_replies`], and it is the
//! caller's job to put those bytes back on the pty; a caller that drops them
//! will hang any program that waits for an answer. The one caller that must
//! not answer is a mirror — an emulator following a stream that another one
//! is already answering — and [`Emulator::advance_mirrored`] is its feed.
//!
//! **Nothing here knows what the pointer has selected.** A pane's output is a
//! list of blocks, and all but the last of them were harvested out of this
//! grid when their commands ended — so a selection anchored to a cell of the
//! grid could only ever cover the newest of them. It is anchored to a block
//! and a row of that block instead, above this crate entirely; what this
//! supplies is [`Emulator::harvest_rows`], so that a pane drawing one grid
//! reads its selection back out of exactly the same store a finished block's
//! rows live in.

use std::path::{Path, PathBuf};
use std::str;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Handler, Processor};
use alacritty_terminal::vte::{Params, Parser, Perform};
use parking_lot::Mutex;

use crate::agent::{self, AgentReport, Reported};
use crate::blocks::{
    Block, BlockId, BlockTracker, HISTORY_HEADROOM, IgnoreReason, LiveBlock, Whereabouts,
};
use crate::harvest::{self, BlockRows};
use crate::image::{self, ImagePlacement};
use crate::input::{InputModes, KeyboardModes};
use crate::marks::ShellMark;
use crate::mouse::MouseModes;
use crate::notify::{self, Notification};
use crate::pty::ChildExit;
use crate::snapshot::{self, Palette, Snapshot, TerminalSize};

/// One item of a stream fed to a mirror, in the order it happened.
///
/// Almost everything an emulator knows arrives as bytes from the child. One
/// thing does not: a submitted command line, which [`crate::Terminal::submit`]
/// hands the emulator directly, before anything the child printed after it is
/// fed, and which nothing in the child's output announces — the echo is what
/// the shell chose to print, not what was typed. A stream replayed into another
/// emulator has to carry it, and carries it as an item of its own rather than
/// as bytes spliced in among the child's. A read ends wherever it ends, inside
/// an escape sequence or a character as readily as anywhere else, and a submit
/// lands between two reads, so bytes spliced in there would reach the parser in
/// the middle of the child's and end whatever it was reading. As an item it
/// reaches no parser at all: [`Emulator::advance_mirrored`] makes the same
/// [`Emulator::command_submitted`] call a direct submit makes, at the same
/// point. Nothing a child prints can pass for one, either. Turning a stream of
/// these into bytes is the writer's job, and framing them is how it keeps a
/// submit apart from the output around it.
///
/// The items are not all a replay has to keep. An emulator of the same size
/// fed the same items in the same order makes the blocks the stream's own
/// emulator made only when each read is fed as the [`Fed::Output`] it was, and
/// at the pace the reads arrived. The pace, because the clock is part of the
/// stream: a synchronized update the child never ended is let go by time, at
/// the first feed or paint 150 ms after it opened, and a replay that runs
/// ahead of that places the marks after it against a screen that has not
/// drawn the update, and harvests their block without it. The reads, because
/// each one's arrival is a moment the emulator looks at that clock, and
/// because the end of a read is where the first row of a block is looked for
/// again after `clear` erased the history above it, so joining or cutting
/// reads can change whether that block keeps the blank rows it began with.
/// Those are the conditions known to matter, not a list proven complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fed<'a> {
    /// Output from the child: one read's worth, exactly as it was read.
    Output(&'a [u8]),
    /// A command line submitted at this point in the stream, as it was
    /// typed.
    Submitted(&'a str),
}

/// Something the child process asked the surrounding application to do.
///
/// These are the events a grid cannot express. They queue up in the emulator
/// and are drained with [`Emulator::take_events`]; an application that ignores
/// them still renders correctly, it just never rings, retitles or closes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TerminalEvent {
    /// The title changed — `None` when the child reset it to the default. The
    /// current value is also on every [`Snapshot`]. At most
    /// [`agent::TITLE_BYTES`] of what the child wrote.
    Title(Option<String>),
    /// The child reported its working directory with OSC 7.
    ///
    /// With the host the URL named, which is the machine the path is on:
    /// a shell on the far side of an `ssh` reports a directory there. `None`
    /// for a URL with an empty host — `file:///tmp` — and for a bare path.
    /// Which host is this machine is not this crate's to say: see the
    /// application's `tab::Remote`.
    WorkingDirectory {
        /// The host the URL named, as it named it.
        host: Option<String>,
        /// The directory, on that host.
        path: PathBuf,
    },
    /// The bell rang.
    Bell,
    /// A command finished, because the shell said so with OSC 133 `D`.
    ///
    /// Only that mark. A block also closes when the next prompt arrives or the
    /// session ends, and neither is a command reporting on itself — so this
    /// fires exactly as often as the shell integration is installed and no
    /// more, which is the honest answer for something a plugin will ring on.
    CommandFinished {
        /// The command line, when there was one: display-only, as
        /// [`crate::Block::command`] is.
        command: Option<String>,
        /// The status the shell reported, or `None` when it reported none.
        exit: Option<i32>,
        /// How long it ran, timed from the submit.
        took: Option<Duration>,
        /// Whether a command ran: the block had a command line, or the shell
        /// said one started. `false` for the bare `D` of a line that ran
        /// nothing — ctrl-c at the prompt, or an empty Enter — which is still
        /// a boundary, and still reported.
        ran: bool,
    },
    /// The child asked the terminal to close.
    Exit,
    /// The child process finished. Raised by [`crate::Terminal`], not by the
    /// escape sequence stream.
    ChildExited(ChildExit),
    /// The child asked for text to be put on the system clipboard with OSC 52.
    ClipboardStore(String),
    /// The shell has answered a completion request, and the answer is waiting
    /// where the asker put the question.
    ///
    /// The number is the request's, echoed back: a person who pressed Tab
    /// twice quickly has two requests outstanding, and only the answer to the
    /// second one is about the line they are looking at.
    ///
    /// Nothing here reads the answer. It is a file, in a directory this crate
    /// has never heard of — the scratch the shell integration owns — which is
    /// exactly why the payload is a serial rather than the candidates: an
    /// escape sequence carrying filenames would need an encoding, and a shell
    /// that has to base64 its own output needs a `base64` this machine may not
    /// have.
    Completions(u64),
    /// A program in the pane said what it is doing, on the channel
    /// [`crate::agent`] describes.
    ///
    /// Reported on change only, so a program that says `running` on every
    /// tool call costs one event when it starts and nothing after. A title
    /// or a message is a change on its own: the same `needs-input` twice
    /// with a different question is two questions. A subagent's report is
    /// weighed before it gets here, and arrives without its title or not at
    /// all — see "Whose report it is" in [`crate::agent`].
    Agent(Reported),
    /// The command a running or waiting agent was ended, and took its report
    /// with it: the status is [`AgentReport::Idle`] again, with no title and
    /// no message.
    ///
    /// Its own event rather than an [`Self::Agent`] the program never sent,
    /// because the difference is the one thing a person asking "why is this
    /// row idle" wants to know: the agent said so, or the shell's `D` said it
    /// for an agent that was interrupted and never got to. The rule for
    /// which marks end which statuses is `Emulator::settle_agent`'s.
    AgentSettled,
    /// A program in the pane said which pull request its work is, on the
    /// channel [`crate::agent`] describes beside the status's own.
    ///
    /// The address has passed [`crate::agent::pull_request_url`]. Reported
    /// every time it is said, not on change: whoever holds it may have let
    /// it go since — the pane moved to another branch — and an agent saying
    /// it again is putting it back.
    PullRequest(String),
    /// A program in the pane asked for a look, in one of the notification
    /// sequences other terminals read — OSC 9, 777 or 99; see
    /// [`crate::notify`].
    ///
    /// Not a status, and it changes none: an agent that is running and
    /// says "tests passed" is still running. Every one is news, so none is
    /// dropped as a repeat; the last of a burst in a read stands for the
    /// others, because it is the last thing the program said. It keeps its
    /// place among the [`Self::Agent`] and [`Self::AgentSettled`] around it,
    /// because a status after it answers it and one before it does not, so
    /// a report between two notifications parts them into two.
    Notification(Notification),
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

/// Watches the byte stream for the OSC sequences the emulator itself drops.
///
/// `vte` parses OSC 7 and OSC 133 into params and then discards both: OSC 7
/// because alacritty resolves the child's directory from its process id
/// instead — a route that needs per-platform process introspection this crate
/// deliberately does not have — and OSC 133 because `vte::ansi::Handler` has no
/// hook for it at all, so no amount of implementing that trait can see one.
/// The notifications other terminals read — OSC 9, 99 and 777 — reach no hook
/// either, and are logged as unhandled. So does one question: XTVERSION,
/// `CSI > q`, which asks for the terminal's name and version, and which `vte`
/// does not tell apart from the `q`s it has no use for.
///
/// So the bytes get a second pass through `vte`'s own parser with a `Perform`
/// that implements little but `osc_dispatch`. That costs one extra walk of the
/// stream, which is a table-driven byte loop that does nothing at all outside
/// an escape sequence, and it buys a correct OSC parser instead of a hand-
/// rolled scanner that has to get chunk boundaries right.
///
/// The two are not read the same way. A working directory does not care where
/// the cursor is, so it is simply collected, and applied before the next mark
/// the watcher stops on: the block that mark opens is filed under it. A
/// command boundary means *the cursor is here, now*, so [`Self::terminated`]
/// stops the watcher on one and [`Emulator::advance`] feeds the real parser
/// only up to that point before reading the cursor.
/// It also stops where an agent report and a notification meet, because each is
/// kept as the last of its kind and the two are read against each other: one
/// slot for the report cannot say which side of the notification it was on.
/// And on a switch of screens, which is a CSI rather than an OSC, because the
/// shell's history has to be cut back right before the shell's screen goes
/// and right after it is back: see [`Emulator::switch_screens`].
#[derive(Default)]
struct OscWatcher {
    /// The last OSC 7 in the chunk: the host it named and the directory.
    working_directory: Option<(Option<String>, PathBuf)>,
    mark: Option<ShellMark>,
    completions: Option<u64>,
    agent: Option<Reported>,
    /// Which session's titles and stops the pane takes, kept here rather
    /// than on the emulator because a report is weighed as it is read: one
    /// the pane does not take must not replace one earlier in the same read
    /// that it does.
    sessions: agent::Sessions,
    pull_request: Option<String>,
    /// Keeps a kitty notification that is arriving in chunks.
    notifications: notify::Reader,
    notification: Option<Notification>,
    /// Whether [`Self::notification`] was written after [`Self::agent`],
    /// which only means anything while both are waiting to be handed over —
    /// at the stop [`Self::terminated`] makes when the second of them
    /// arrives: [`Emulator::apply_reports`] hands them over in that order.
    notified_last: bool,
    /// Whether the chunk erased the scrollback — `CSI 3 J`, the third thing
    /// `clear` prints.
    history_cleared: bool,
    /// Whether the chunk reset the terminal — `ESC c`, the first thing
    /// `reset` prints — which empties the shell's history along with
    /// everything else, from either screen, and leaves the alternate one.
    reset: bool,
    /// Whether the program asked which terminal this is, with XTVERSION —
    /// `CSI > q`, which is where the watcher stops.
    version_asked: bool,
    /// Whether the program switched screens — `CSI ? 1049 h` or `l`, the one
    /// switch alacritty makes — which is where the watcher stops.
    screen_switched: bool,
    /// Keeps an iTerm2 picture that is arriving in pieces.
    images: image::Reader,
    /// A picture to place at the cursor, which is where the watcher stops.
    image: Option<image::Requested>,
    /// How long a title an OSC 0 or 2 set, when it was longer than
    /// [`agent::TITLE_BYTES`], which is where the watcher stops: see
    /// [`Emulator::cut_grid_title`].
    long_title: Option<usize>,
}

impl Perform for OscWatcher {
    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if intermediates.is_empty() && byte == b'c' {
            self.reset = true;
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let first = params.iter().next();
        match (action, intermediates) {
            // Only the plain form: `CSI ? 3 J` is the selective erase, which
            // leaves the history alone.
            ('J', []) if first.is_some_and(|param| param == [3]) => {
                self.history_cleared = true;
            }
            // XTVERSION takes no parameter but 0, which is also what an
            // empty one parses as.
            ('q', [b'>']) if first.is_none_or(|param| param == [0]) => {
                self.version_asked = true;
            }
            // Among other modes, maybe: `Term` sets each one in turn.
            ('h' | 'l', [b'?']) if params.iter().any(|param| param.first() == Some(&1049)) => {
                self.screen_switched = true;
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        match params.first() {
            Some(&b"7") if params.len() >= 2 => {
                if let Some(reported) = parse_working_directory(params[1]) {
                    self.working_directory = Some(reported);
                }
            }
            Some(&b"133") => self.mark = ShellMark::parse(params),
            Some(&b"1337") => self.image = self.images.read(params),
            // Crook's own, and the number is deliberately far from anything
            // standardised: nothing but a shell Crook itself set up emits it,
            // and a stream that happens to contain one costs a caller a look
            // at a file it wrote.
            Some(&COMPLETIONS_OSC) if params.len() >= 2 => {
                self.completions = str::from_utf8(params[1])
                    .ok()
                    .and_then(|serial| serial.parse().ok());
            }
            // The last one wins like a status below, until a status or a
            // mark stops the watcher and it is handed over. Every one is
            // read, even the ones that finish nothing: a kitty notification
            // sent in chunks is put back together across them.
            Some(&(b"9" | b"99" | b"777")) => {
                if let Some(notification) = self.notifications.read(params) {
                    self.notification = Some(notification);
                    self.notified_last = true;
                }
            }
            // `Term` reads the title itself. This only measures it, as the
            // text `vte` joins the pieces back into, or a little over: `vte`
            // also trims it and drops a piece that is not UTF-8, so a title
            // just past the cap can stop the watcher and be cut to itself.
            Some(&(b"0" | b"2")) if params.len() >= 2 => {
                let joined =
                    params[1..].iter().map(|piece| piece.len()).sum::<usize>() + (params.len() - 2);
                if joined > agent::TITLE_BYTES {
                    self.long_title = Some(joined);
                }
            }
            // A status means the same thing wherever in the chunk the program
            // wrote it, so the last one wins, which is the last thing the
            // program said — up to a mark or a notification, which it has to
            // be handed over ahead of or behind.
            _ => {
                if let Some((reported, session)) = agent::parse(params) {
                    if let Some(reported) = self.sessions.admit(reported, session) {
                        self.agent = Some(reported);
                        self.notified_last = false;
                    }
                } else if let Some(url) = agent::parse_pull_request(params) {
                    self.pull_request = Some(url);
                }
            }
        }
    }

    /// Stops the parser on a captured mark: an OSC 133 in a dialect this does
    /// not read leaves it running, so a stream full of them costs nothing.
    ///
    /// And on the second of an agent report and a notification, so that
    /// [`Emulator::advance`] hands both over before the rest of the read can
    /// write another report over the first. Kept together, `needs-input`,
    /// `9;done`, `running` went out as the notification and a `running` that
    /// changed nothing. A burst of either with none of the other between
    /// still collapses to its last one, and costs no stop.
    ///
    /// And on XTVERSION, whose answer [`Emulator::advance`] makes from here:
    /// `Term` queues its own answers as it parses, so the version has to be
    /// answered once everything before the query is parsed and nothing after
    /// it, or it goes out of order with the answers either side of it.
    ///
    /// And on a title longer than [`agent::TITLE_BYTES`], so that the grid's
    /// own copy of it can be cut before the rest of the read pushes it.
    ///
    /// And on a switch of screens, so that [`Emulator::switch_screens`] can
    /// feed it apart from the output before it.
    fn terminated(&self) -> bool {
        self.mark.is_some()
            || self.image.is_some()
            || (self.agent.is_some() && self.notification.is_some())
            || self.version_asked
            || self.long_title.is_some()
            || self.screen_switched
    }
}

/// The OSC number a shell answers a completion request on.
///
/// Crook's own. VS Code took 633 for the same job and its own protocol; this
/// is far enough away from every number anything standardised uses that a
/// stream carrying one came from a shell Crook set up.
const COMPLETIONS_OSC: &[u8] = b"6339";

/// The longest working directory OSC 7 is believed for, in bytes.
///
/// Linux's `PATH_MAX`: four times macOS's, and past it Linux opens no path
/// by name. Windows goes further only once long paths are switched on, and
/// a directory that deep is not one anybody works in. Bounded at all because
/// the row prints the directory and shapes it again on every frame it draws;
/// refused rather than cut, as [`agent::pull_request_url`] refuses an
/// address, because a cut path is another directory, and the row, git, the
/// next pane's start and the session file would all be told that one.
const DIRECTORY_BYTES: usize = 4096;

/// The longest host OSC 7 is believed for, in bytes.
///
/// 253 is the longest name DNS resolves, and a little over it leaves room for
/// the `user@` a URL may put in front. Bounded for the reason
/// [`DIRECTORY_BYTES`] is: the row prints it, and shapes it on every frame.
const HOST_BYTES: usize = 255;

/// The longest title, in bytes, that a synchronized update is left to carry
/// to the grid whole, rather than ended at it: see
/// [`Emulator::cut_grid_title`].
///
/// The same 4096 as [`DIRECTORY_BYTES`], because what makes a title long is
/// most often the path in it. 4096 copies of a title this long, a full
/// `CSI 22 t` stack, are 16 MiB, which is the most leaving an update alone
/// can cost; ending it costs a frame drawn half done.
const UPDATE_TITLE_BYTES: usize = 4096;

/// Reads the payload of OSC 7, which is a `file://` URL or a bare path, into
/// the host the URL named and the path.
///
/// The host comes back with the path because the path means a directory on
/// that host and nowhere else: a shell on the far end of an ssh session
/// reports one that, read on this machine, is somebody else's checkout or
/// nothing at all. An empty host — `file:///tmp` — and a bare path name no
/// host, and come back as `None`. Whether a named one is this machine is the
/// caller's to decide, which needs the machine's own name.
///
/// A path longer than [`DIRECTORY_BYTES`] once decoded is no answer at all,
/// and neither is a host longer than [`HOST_BYTES`] or one holding a control
/// character or a space, since no machine is called that and the row would
/// print it.
fn parse_working_directory(value: &[u8]) -> Option<(Option<String>, PathBuf)> {
    let value = str::from_utf8(value).ok()?;
    let (host, path) = match value.strip_prefix("file://") {
        Some(authority_and_path) => {
            let (host, path) = authority_and_path.split_at(authority_and_path.find('/')?);
            (Some(host).filter(|host| !host.is_empty()), path)
        }
        None => (None, value),
    };
    if let Some(host) = host
        && (host.len() > HOST_BYTES
            || host
                .chars()
                .any(|character| character.is_control() || character.is_whitespace()))
    {
        return None;
    }

    let decoded = percent_decode(path)?;
    // A Windows path arrives as `/C:/Users/...`; the leading slash is part of
    // the URL, not of the path.
    let decoded = if cfg!(windows) && is_drive_prefixed(&decoded) {
        decoded[1..].to_owned()
    } else {
        decoded
    };

    // Anything else is not a directory anybody could be in. A URL of another
    // scheme — `kitty-shell-cwd://host/tmp`, which kitty's and Ghostty's own
    // integrations write — or a stray word came through as a *relative* path,
    // and everything that uses the directory resolved it against Crook's own.
    let rooted = decoded.starts_with(['/', '\\']) || Path::new(&decoded).is_absolute();
    (rooted && decoded.len() <= DIRECTORY_BYTES)
        .then(|| (host.map(str::to_owned), PathBuf::from(decoded)))
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
                // Two hex digits and nothing else: `from_str_radix` also takes
                // a sign, and `%+1` decoded to a control byte in a path.
                let digits = &bytes[index + 1..index + 3];
                if !digits.iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                let digits = str::from_utf8(digits).ok()?;
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

/// Where the viewport stood before a feed, read against the history under it,
/// which is what [`Emulator::keep_view`] needs to put it back on its lines.
#[derive(Clone, Copy)]
struct View {
    alt_screen: bool,
    offset: usize,
    history: usize,
}

/// What was typed ahead of a shell away from its prompt, counted the way it
/// will read it back: one prompt for each line ended with an Enter, which it
/// runs, and one for the start of a line after them, which it keeps. See
/// [`Emulator::hand_keys_to_line_editor`].
#[derive(Clone, Copy, Debug, Default)]
struct TypedAhead {
    lines: u32,
    unfinished: bool,
}

impl TypedAhead {
    fn add(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match byte {
                // The interrupt, quit and suspend characters a pty starts out
                // with, `ctrl-c`, `ctrl-\` and `ctrl-z`. Its line discipline
                // acts on one instead of queueing it, and throws away what was
                // waiting to be read: the shell is given none of it.
                0x03 | 0x1c | 0x1a => *self = Self::default(),
                b'\r' | b'\n' => {
                    self.lines = self.lines.saturating_add(1);
                    self.unfinished = false;
                }
                _ => self.unfinished = true,
            }
        }
    }

    /// Takes what the prompt the shell has just drawn is for, answering
    /// whether there was anything.
    fn take_line(&mut self) -> bool {
        if self.lines > 0 {
            self.lines -= 1;
            return true;
        }
        std::mem::take(&mut self.unfinished)
    }
}

/// A terminal grid being driven by a stream of bytes.
pub struct Emulator {
    term: Term<EventProxy>,
    parser: Processor,
    osc_parser: Parser,
    osc_watcher: OscWatcher,
    blocks: BlockTracker,
    proxy: EventProxy,
    palette: Palette,
    size: TerminalSize,
    /// The most history the grid keeps between feeds. Alacritty is allowed
    /// [`HISTORY_HEADROOM`] lines more while one is under way, which the
    /// blocks cut back off after every slice: see [`Self::feed`]. A history
    /// this long no longer grows by the lines that scroll into it, which is
    /// where [`Self::keep_view`] stops being able to count them.
    scrollback_lines: usize,
    /// Whether the synchronized update `parser` is holding switches screens,
    /// which the watcher saw go by: see [`Self::switch_screens`].
    switch_held: bool,
    title: Option<String>,
    working_directory: Option<PathBuf>,
    /// The host the last OSC 7 named, beside [`Self::working_directory`]:
    /// the same path on another machine is another place.
    working_host: Option<String>,
    /// What the program in the pane last said it was doing.
    agent: AgentReport,
    events: Vec<TerminalEvent>,
    replies: Vec<u8>,
    /// The whole answer to XTVERSION, made once, when there is a name to
    /// answer with: see [`Self::set_name_and_version`].
    version_reply: Option<String>,
    /// Whether this has ever been fed through [`Self::advance_mirrored`],
    /// which makes it an emulator that owes the child nothing for good.
    mirrored: bool,
    /// Whether the shell's own line editor has the keyboard. See
    /// [`Self::hand_keys_to_line_editor`].
    line_editor: bool,
    /// Whether the shell has finished a line, with `D`, since the line editor
    /// was handed the keyboard, and the prompt after it has not been looked
    /// at yet: the one moment the keyboard can come back. See
    /// [`Self::hand_keys_to_line_editor`].
    line_finished: bool,
    /// What was typed while the line editor had the keyboard and the shell
    /// was away from its prompt. See [`Self::typed`].
    typed_ahead: TypedAhead,
    snapshot: Arc<Snapshot>,
    /// Whether something has happened since the cached snapshot was built.
    ///
    /// Alacritty's own damage tracking is not used for this: `Term::damage`
    /// unconditionally reports the cursor's cell as damaged, so it can never
    /// say "nothing changed". This flag says "worth looking", and
    /// [`Emulator::snapshot`] settles it by comparing what it builds against
    /// what it already had.
    dirty: bool,
}

impl Emulator {
    /// A terminal of the given size, drawing with `palette` and keeping
    /// `scrollback_lines` of history above the viewport.
    pub fn new(size: TerminalSize, scrollback_lines: usize, palette: Palette) -> Self {
        let proxy = EventProxy::default();
        let config = Config {
            // Room past the scrollback for a slice of output to scroll into,
            // which the blocks cut back off and count, so that nothing
            // alacritty evicts from a full history goes uncounted.
            scrolling_history: scrollback_lines.saturating_add(HISTORY_HEADROOM),
            // Off in alacritty's default config, which means `Term` would
            // refuse the mode-setting escapes and never answer the query a
            // program uses to find out whether the protocol is available. The
            // encoder in `crate::input` reads the flags this maintains.
            kitty_keyboard: true,
            ..Config::default()
        };
        let mut term = Term::new(config, &size, proxy.clone());
        // `Term` starts fully damaged so a fresh renderer paints everything.
        // This crate tracks that itself, and its first snapshot is revision 0.
        term.reset_damage();

        let blocks = BlockTracker::new();
        let live = blocks.live(&term);
        let snapshot = Arc::new(snapshot::build(&term, &palette, 0, None, live, false));
        Self {
            term,
            parser: Processor::new(),
            osc_parser: Parser::new(),
            osc_watcher: OscWatcher::default(),
            blocks,
            proxy,
            palette,
            size,
            scrollback_lines,
            switch_held: false,
            title: None,
            working_directory: None,
            working_host: None,
            agent: AgentReport::default(),
            events: Vec::new(),
            replies: Vec::new(),
            version_reply: None,
            mirrored: false,
            line_editor: false,
            line_finished: false,
            typed_ahead: TypedAhead::default(),
            snapshot,
            dirty: false,
        }
    }

    /// Feeds output from the child process into the grid.
    ///
    /// The stream is fed in pieces rather than in one call, and the pieces are
    /// cut by the OSC 133 marks in it: the watcher runs first and stops on a
    /// mark, the grid is advanced only as far as the watcher got, and the mark
    /// is then applied while the cursor still stands where the shell left it.
    /// Feeding the whole chunk first and looking for marks afterwards would put
    /// every boundary wherever the end of the chunk happened to land, and a
    /// mark split across two calls to this method would land nowhere at all.
    /// `vte` keeps the state that spans the split, so a mark arriving one byte
    /// at a time works exactly as one arriving whole. What the grid reported
    /// in a piece — a title, a bell — goes out before the mark that ends it,
    /// so a title or a bell and a mark written after it come out in the order
    /// they were written however the pty cut the stream. That is the whole of
    /// the promise: a title and an OSC 6340 report or a notification in one
    /// read can still come out in another order than two reads give them,
    /// since the watcher keeps no note of where in a piece each one sat.
    ///
    /// The one case this cannot place exactly is a mark inside a synchronized
    /// update: `vte` buffers those bytes and applies them when the update
    /// closes, so the cursor read here is the one from before it opened. A
    /// program that brackets its whole frame that way and emits OSC 133 inside
    /// it is anchoring against a screen that has not been drawn yet — and a
    /// program drawing frames is on the alternate screen, where marks are
    /// ignored outright.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.expire_sync();
        if bytes.is_empty() {
            return;
        }

        let view = self.view();
        let mut exited = None;
        // Whether the shell finished a line in this read, which is the only
        // thing that can give the keyboard back to a pane whose line editor
        // has it. See [`Self::hand_keys_to_line_editor`].
        let mut line_finished = false;
        let mut rest = bytes;
        while !rest.is_empty() {
            let consumed = self
                .osc_parser
                .advance_until_terminated(&mut self.osc_watcher, rest);
            let (piece, remaining) = rest.split_at(consumed);
            if std::mem::take(&mut self.osc_watcher.screen_switched) {
                self.switch_screens(piece);
            } else {
                self.feed(piece);
            }
            // Before anything else reads the anchor: a piece that took a
            // full-screen program down is the moment a reflow made under it
            // can be done on the screen it was for.
            self.blocks.settle_reflow(&self.term);
            // Before the mark, which the piece ends on and so comes after
            // anything else in it. On the alternate screen the erase is the
            // full-screen program's own and touches no history of the shell's.
            // A reset empties the shell's whichever screen the piece ends on:
            // under a program started after it, the block is found again once
            // the shell's screen is back, as it is after a reflow there.
            let erased = std::mem::take(&mut self.osc_watcher.history_cleared)
                && !self.term.mode().contains(TermMode::ALT_SCREEN);
            if std::mem::take(&mut self.osc_watcher.reset) || erased {
                self.blocks.history_cleared(&self.term);
            }
            // The piece ends on the query, and `Term` has queued the answers
            // to everything before it on the proxy, so this lands behind
            // them and ahead of the rest of the read's, in the one queue
            // `drain` empties. A query inside a synchronized update is the
            // exception: `vte` holds the update's queries and answers them
            // when it ends, and this goes at once — ahead of every query
            // sent after it all the same, which is the order a program
            // waiting on a sentinel query behind it needs.
            if std::mem::take(&mut self.osc_watcher.version_asked)
                && let Some(reply) = &self.version_reply
            {
                self.proxy.send_event(Event::PtyWrite(reply.clone()));
            }
            // Wherever the watcher stopped, what it holds was written before
            // the stop and goes out now. A report earlier in this same read
            // has to reach `self.agent` before a mark settles against it, or
            // the same bytes settle differently depending on where a pty split
            // them: `drain` alone applies the report after the loop, too late
            // for a `D` that ends the very command the report was about. A
            // notification was written before the mark too, and goes out ahead
            // of what the mark settles. And a stop on a report and a
            // notification together has to empty one of them, or the watcher
            // stops again on the next byte having read nothing.
            if self.osc_watcher.terminated() {
                self.apply_reports();
            }
            if let Some(bytes) = self.osc_watcher.long_title.take() {
                exited = self.cut_grid_title(bytes).or(exited);
            }
            // After everything in the piece that scrolled, the update a long
            // title ended included, and before the mark, which may harvest
            // the pictures at the rows their anchors name.
            self.settle_history();
            if let Some(mark) = self.osc_watcher.mark.take() {
                // A directory too, and for the same reason: bash and zsh print
                // `D`, the directory and the next `A` back to back, and the
                // block that `A` opens is filed under whatever directory it
                // sees. Applied after the loop, it would file `pwd` after a
                // `cd` under the directory the `cd` left.
                self.apply_working_directory();
                // And what the grid itself reported in the piece — a title
                // above all — for the same reason again: it was written before
                // the mark, and goes out ahead of what the mark settles. Left
                // to `drain`, a program's last title and the shell's `D` after
                // it came out the other way round whenever one read held both,
                // and the workspace, which takes back a running the title said
                // when the command ends, heard the title say running again for
                // a program that had gone and would say nothing more.
                exited = self.hand_over_term_events().or(exited);
                self.settle_agent(mark);
                let finished = self.blocks.mark(
                    mark,
                    &mut self.term,
                    &self.palette,
                    Whereabouts::of(
                        self.working_directory.as_deref(),
                        self.working_host.as_deref(),
                    ),
                );
                if let Some(finished) = finished {
                    // Only a `D` the block list took: one a full-screen
                    // program printed is its own, and finishes nothing.
                    line_finished = true;
                    self.events.push(TerminalEvent::CommandFinished {
                        command: finished.command,
                        exit: finished.exit,
                        took: finished.took,
                        ran: finished.ran,
                    });
                }
            }
            if let Some(requested) = self.osc_watcher.image.take() {
                self.place_image(requested);
            }
            rest = remaining;
        }
        self.give_keys_back(line_finished);
        self.keep_view(view);
        self.dirty = true;
        self.drain();
        if let Some(exit) = exited {
            self.child_exited(exit);
        }
    }

    /// Puts a picture at the cursor, in the open block, and moves the cursor
    /// past the rows it covers.
    ///
    /// The rows are made the way the program's own newlines would make them,
    /// by feeding the grid newlines: they scroll the screen when the picture
    /// is printed at its foot, and they are the open block's rows like any
    /// other, harvested with it when it finishes. A picture that would not
    /// fit in what is left of the row starts on the next one.
    fn place_image(&mut self, requested: image::Requested) {
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        let grid_columns = self.term.columns();
        let (columns, rows) = image::cells(
            &requested,
            (self.size.cell_width, self.size.cell_height),
            (grid_columns, self.term.screen_lines()),
        );
        let cursor = &self.term.grid().cursor;
        if cursor.input_needs_wrap
            || (cursor.point.column.0 > 0 && cursor.point.column.0 + columns > grid_columns)
        {
            self.feed(b"\r\n");
        }
        let at = self.term.grid().cursor.point;
        let (line, column) = (at.line.0, at.column.0);
        self.blocks.image(
            line,
            column,
            ImagePlacement {
                image: Arc::new(requested.image),
                row: 0,
                column,
                columns,
                rows,
                stretch: requested.stretch,
            },
            &self.term,
        );

        // Down to its last row, and then along to the column after it — or,
        // when it reaches the edge, to the start of the row below, which is
        // where a character after it would wrap to anyway.
        let mut moves = b"\n".repeat(rows - 1);
        if column + columns >= grid_columns {
            moves.extend_from_slice(b"\r\n");
        } else {
            moves.extend_from_slice(format!("\x1b[{}G", column + columns + 1).as_bytes());
        }
        self.feed(&moves);
        // The rows it took scrolled the grid, which can take a first row out
        // of a full history: another picture's, or one as tall as the screen
        // printed at its foot.
        self.settle_history();
    }

    /// Feeds `bytes` to the grid, in slices too short to scroll the history
    /// to the end of its headroom a line at a time, and holds the history at
    /// its length after each: see [`BlockTracker::trim_history`].
    ///
    /// A byte scrolls one line at most, unless it ends a sequence that
    /// scrolls several — a scroll by a count, an erase that pushes the screen
    /// up, a synchronized update let go of — so a slice a byte shorter than
    /// the room left cannot reach the end with newlines, however many it
    /// holds. The history is at its length after every slice, so the room is
    /// the whole headroom when it is full, and the whole feed when it is far
    /// from it.
    fn feed(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            let (slice, after) = rest.split_at(self.history_room().min(rest.len()));
            let held = self.updating();
            self.parser.advance(&mut self.term, slice);
            if held {
                self.update_let_go();
            }
            self.blocks
                .trim_history(&mut self.term, self.scrollback_lines);
            rest = after;
        }
    }

    /// Feeds a piece the watcher stopped on a switch of screens at, cutting
    /// the shell's history back before the switch's last byte goes in and
    /// after it has.
    ///
    /// The bytes before that one may scroll the shell's screen, and the trim
    /// after them is the last chance to count what they scrolled before the
    /// shell's history is out of reach. The trim after it is the first chance
    /// once the history is back in reach, before anything scrolls it. So no
    /// slice scrolls the shell's history while it cannot be read, and under a
    /// program it is no longer than [`Self::history_room`] takes it to be.
    ///
    /// Inside a synchronized update the switch is held with the rest of the
    /// update, and reaches the grid only when it is let go of: see
    /// [`Self::update_let_go`]. It is noted as held before its last byte goes
    /// in, because that byte can be what lets go of the update: one that
    /// would fill `vte`'s buffer to its limit ends the update instead, inside
    /// the very feed that carries it.
    fn switch_screens(&mut self, piece: &[u8]) {
        if let Some((last, before)) = piece.split_last() {
            self.feed(before);
            self.switch_held |= self.updating();
            self.feed(std::slice::from_ref(last));
        }
    }

    /// Whether a synchronized update is being held, to reach the grid when it
    /// is let go of.
    fn updating(&self) -> bool {
        self.parser.sync_timeout().sync_timeout().is_some()
    }

    /// Called after anything that may have let go of a synchronized update:
    /// one that switched screens may have scrolled the shell's history out of
    /// sight, where nothing could cut it back. See
    /// [`BlockTracker::update_switched_screens`].
    fn update_let_go(&mut self) {
        if self.switch_held {
            self.blocks.update_switched_screens(&self.term);
            self.switch_held = self.updating();
        }
    }

    /// How long the next slice [`Self::feed`] feeds may be: a byte shorter
    /// than the lines the history has room for before alacritty evicts on its
    /// own, and a byte at least.
    ///
    /// On the alternate screen the shell's history cannot be read. It was
    /// held at its length right before the shell's screen went, and the
    /// program's output does not scroll it, so the headroom is the room
    /// until a slice brings the shell's screen back — which that slice ends
    /// on: see [`Self::switch_screens`]. A full reset scrolls it from empty.
    /// What else can move it under a program — a resize, an update let go of
    /// that switched screens — is [`BlockTracker::rows_changed`]'s.
    fn history_room(&self) -> usize {
        let history = if self.term.mode().contains(TermMode::ALT_SCREEN) {
            self.scrollback_lines
        } else {
            self.term.grid().history_size()
        };
        let limit = self.scrollback_lines.saturating_add(HISTORY_HEADROOM);
        limit.saturating_sub(history).saturating_sub(1).max(1)
    }

    /// Holds the history at its length, and forgets the open block's pictures
    /// whose first rows have left it: see [`BlockTracker::trim_history`] and
    /// [`BlockTracker::settle_pictures`].
    fn settle_history(&mut self) {
        self.blocks
            .trim_history(&mut self.term, self.scrollback_lines);
        self.blocks.settle_pictures(&self.term);
    }

    /// Feeds one item of a stream that another emulator is also being fed,
    /// and answering.
    ///
    /// The grid, the blocks and the events come out exactly as the same calls
    /// to [`Self::advance`] and [`Self::command_submitted`], in the same
    /// order, would make them. What does not come out is a reply: the other
    /// emulator has already answered every query in the stream, and a second
    /// answer would reach the child as input it never asked for, typed into
    /// whatever it is running. Feeding an emulator this way once makes it a
    /// mirror for the rest of its life, so [`Self::take_replies`] is empty from
    /// then on however it is fed — including of the answers to queries a
    /// synchronized update was holding when a later [`Self::snapshot`] let it
    /// go.
    ///
    /// Nothing in Crook feeds an emulator this way yet.
    pub fn advance_mirrored(&mut self, fed: Fed<'_>) {
        self.mirrored = true;
        match fed {
            Fed::Output(bytes) => self.advance(bytes),
            Fed::Submitted(line) => self.command_submitted(line),
        }
    }

    /// Records that a command line has been handed to the shell, which is the
    /// one block boundary that needs no cooperation from it.
    ///
    /// The application knows what it wrote and when, so a block opened this way
    /// carries the command text exactly, where a shell-integrated one carries
    /// whatever was echoed on screen. Call it once the line has been written,
    /// and before anything the child printed after it is fed: then a line
    /// that never reached the shell opens no block, and the echo of one that
    /// did lands in the block it opened.
    ///
    /// A stream that has to carry this — one replayed into another emulator —
    /// carries it as a [`Fed::Submitted`] of its own, which
    /// [`Self::advance_mirrored`] turns back into a call to this at the same
    /// point in the stream.
    pub fn command_submitted(&mut self, command: &str) {
        self.blocks.submitted(
            command,
            &mut self.term,
            &self.palette,
            Whereabouts::of(
                self.working_directory.as_deref(),
                self.working_host.as_deref(),
            ),
        );
        self.dirty = true;
    }

    /// Records that a key went to the shell's own line editor at its
    /// prompt, which has the keyboard from now until it gives its line back,
    /// and answers whether that took: only at a prompt whose end the shell
    /// has marked.
    ///
    /// The key is one a pane's own command-line field had no use for —
    /// `ctrl-r`, fzf's `ctrl-t`, Escape — and what the line editor draws in
    /// answer is on the prompt's row and under it. [`Snapshot::line_editor`]
    /// says so for as long as it lasts, which is until the shell finishes a
    /// line, with `D`, and the prompt it draws next has nothing on it: the
    /// line ran, or an empty Enter or `ctrl-c` ended it, and the line editor
    /// starts the next one from nothing.
    ///
    /// Only `D`, because it is the one mark a shell prints only once its line
    /// editor is done with a line. Everything else can come while the line
    /// editor still holds something, and giving the keyboard back then puts
    /// the field back over it:
    ///
    /// * **A prompt drawn again.** bash draws its whole prompt again, marks
    ///   and all, whenever a search ends, with the match it kept on the line
    ///   and the cursor at the start of it when the match starts the line;
    ///   and whenever a `bind -x` widget returns, which is how fzf's `ctrl-t`
    ///   puts the path it picked there. The next Enter would send the field's
    ///   line and the kept one as one.
    /// * **A prompt drawn again over an empty line.** Escape in vi mode
    ///   leaves insert mode, and fish's mode prompt, a zsh theme's mode
    ///   indicator and bash's `show-mode-in-prompt` draw the prompt again
    ///   to say so; an Escape in emacs mode leaves a Meta prefix waiting for
    ///   its key, and a resize draws the prompt again over it. The line is
    ///   empty and the line editor is not: the field's next line would be
    ///   read as vi commands, or lose its first letter to the prefix.
    /// * **`C`.** bash runs its DEBUG trap for a `bind -x` widget too, and a
    ///   copy of Crook's bash script from before it told the two apart, or
    ///   another terminal's integration on a host Crook did not start the
    ///   shell on, reports the widget starting as a command.
    ///
    /// And what was typed while the command ran is the line editor's still.
    /// It went to the shell, which reads it once it is back at its prompt and
    /// draws it on the line there — after the prompt, in a write of its own,
    /// so the read the prompt arrives in has an empty line on it. The prompt's
    /// cells cannot say, and so [`Self::typed`] counts what is sent instead:
    /// each line ended with an Enter is one the shell will run and finish with
    /// a `D` of its own, and the start of one is a line it will keep. The
    /// prompt after each `D` takes one of them, and keeps the keyboard. A
    /// command that read what was typed itself — `read`, a password prompt —
    /// leaves one behind, and the keyboard with the shell a line longer than
    /// it needed: the side to be wrong on.
    ///
    /// So zsh's `ctrl-g` and fish's Escape, which give a search up over an
    /// empty line and draw no mark at all, and bash's `ctrl-g`, which draws
    /// the prompt again, leave the line editor the keyboard until the next
    /// Enter. fish ends an empty line with no `D` either, so there it is the
    /// next command that gives it back.
    ///
    /// A shell that marks no prompt is never handed it, because nothing would
    /// ever say it had given the keyboard back.
    pub fn hand_keys_to_line_editor(&mut self) -> bool {
        if !self.blocks.at_prompt() || self.is_alt_screen() {
            return false;
        }
        // A `D` from before this key finished nothing it was handed.
        self.line_finished = false;
        if !self.line_editor {
            self.line_editor = true;
            self.typed_ahead = TypedAhead::default();
            self.dirty = true;
        }
        true
    }

    /// Records what went to the child as though typed — a key, a paste, a
    /// line — for [`Self::hand_keys_to_line_editor`] to count: typed while the
    /// line editor has the keyboard and the shell is away from its prompt, it
    /// is the shell's to read when it is back. [`crate::Terminal::write`]
    /// calls this for everything it sends, and that includes the mouse's and
    /// the focus's reports: those go only to a program that asked for them,
    /// which is one whose keys are not counted.
    ///
    /// Not while the program in front says it reads the keyboard itself, by
    /// asking for the alternate screen, bracketed pastes, the mouse, focus
    /// reports or kitty's keyboard. A shell has none of them on while it runs
    /// a command, so a program that has one on asked for it, and what is
    /// typed into `vim`, a REPL or Claude Code is theirs: counted, it would
    /// keep the keyboard from the shell's empty prompt after every one of
    /// them run from a history search. A program that reads the keyboard and
    /// asks for none of them has what is typed counted, and costs the line
    /// that [`Self::hand_keys_to_line_editor`] says a `read` does.
    pub fn typed(&mut self, bytes: &[u8]) {
        if self.line_editor && !self.blocks.at_prompt() && !self.reads_its_own_keys() {
            self.typed_ahead.add(bytes);
        }
    }

    /// Whether the program in front has asked for something only a program
    /// reading the keyboard itself asks for. See [`Self::typed`].
    fn reads_its_own_keys(&self) -> bool {
        self.term.mode().intersects(
            TermMode::ALT_SCREEN
                | TermMode::BRACKETED_PASTE
                | TermMode::MOUSE_MODE
                | TermMode::FOCUS_IN_OUT
                | TermMode::KITTY_KEYBOARD_PROTOCOL,
        )
    }

    /// Gives the keyboard back from the line editor once the shell has
    /// finished a line and drawn the prompt after it with nothing on it, at
    /// the end of a read — unless something typed while the shell was away is
    /// what it will put on that line next. See
    /// [`Self::hand_keys_to_line_editor`].
    fn give_keys_back(&mut self, line_finished: bool) {
        if !self.line_editor {
            return;
        }
        self.line_finished |= line_finished;
        // Until the next prompt's `B` has arrived there is no line to look at.
        if !self.line_finished || !self.blocks.at_prompt() {
            return;
        }
        self.line_finished = false;
        if self.typed_ahead.take_line() {
            return;
        }
        // And the cells, for a line on the prompt that nothing counted — typed
        // in the moment between an Enter and the shell's `C`, which reads as
        // typing at the prompt — when it is drawn in the read the prompt is.
        if self.blocks.at_an_empty_prompt(&self.term) {
            self.line_editor = false;
        }
    }

    /// Whether the shell's own line editor has the keyboard. Also on every
    /// [`Snapshot`]. See [`Self::hand_keys_to_line_editor`].
    pub fn line_editor_has_keys(&self) -> bool {
        self.line_editor
    }

    /// The finished blocks, oldest first.
    pub fn blocks(&self) -> &[Block] {
        self.blocks.finished()
    }

    /// One finished block by id, or `None` once it has been evicted.
    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.blocks
            .finished()
            .binary_search_by_key(&id, |block| block.id)
            .ok()
            .map(|at| &self.blocks.finished()[at])
    }

    /// Forgets every finished block and the scrollback, and keeps the block
    /// that is open where it is: what clearing a pane does — Warp's "Clear
    /// blocks", and the `cmd-k` of the terminals before it.
    ///
    /// The scrollback goes with the blocks because it is the rest of what a
    /// pane has to scroll back to: what the open block has pushed off the
    /// screen, and for a shell that reports no boundaries, everything above
    /// the screen. The screen itself stays — the prompt, or whatever a
    /// running command is printing — and so does the block on it, down to
    /// where its prompt ends; `BlockTracker::clear` says how.
    ///
    /// Not part of any stream, so an emulator fed the same items as a mirror
    /// keeps everything this one forgot.
    ///
    /// Answers the finished blocks it let go of, oldest first, for a caller
    /// with something to keep of them: what the last command exited with
    /// still answers whether the pane's command has finished.
    pub fn clear_blocks(&mut self) -> Vec<Block> {
        let dropped = self.blocks.clear(&mut self.term);
        self.dirty = true;
        dropped
    }

    /// How many blocks have been dropped off the front of the list to keep a
    /// stream of forged marks from growing it without bound, or by
    /// [`Self::clear_blocks`].
    pub fn blocks_evicted(&self) -> usize {
        self.blocks.evicted()
    }

    /// What [`Self::blocks_evicted`] came to at the last
    /// [`Self::clear_blocks`], or 0 for a pane never cleared. Past it, the
    /// count is blocks the cap pushed off the front, which leave something
    /// before the first block still held; up to it, blocks a person cleared
    /// away, which leave nothing.
    pub fn blocks_cleared_at(&self) -> usize {
        self.blocks.cleared_at()
    }

    /// The block everything arriving now belongs to. Also on every
    /// [`Snapshot`].
    pub fn live_block(&self) -> LiveBlock {
        self.blocks.live(&self.term)
    }

    /// Why the last mark that changed nothing changed nothing, or `None` when
    /// the last one was acted on.
    ///
    /// Marks are ignored constantly and on purpose — a prompt framework redraws
    /// its prompt several times per keystroke — so this is how "why is there no
    /// block here" gets an answer instead of a shrug.
    pub fn last_ignored_mark(&self) -> Option<IgnoreReason> {
        self.blocks.last_ignored()
    }

    /// The visible grid.
    ///
    /// Returns the very same `Arc` — and so the same revision — until the drawn
    /// content actually differs, so a caller may compare revisions, or the
    /// pointers, to decide whether to repaint.
    ///
    /// **Dragging a selection out never reaches this.** A selection lives
    /// above the emulator, in the list of blocks it spans, so a pointer move
    /// repaints without the grid being walked or a snapshot being built at
    /// all.
    pub fn snapshot(&mut self) -> Arc<Snapshot> {
        // A program that emitted a synchronized update and then died never
        // sends the bytes that would end it, and no more output will arrive to
        // notice that. Painting is the other moment the question comes up.
        self.expire_sync();
        if self.dirty {
            self.dirty = false;

            let revision = self.snapshot.revision + 1;
            let live = self.blocks.live(&self.term);
            let built = snapshot::build(
                &self.term,
                &self.palette,
                revision,
                self.title.as_deref(),
                live,
                self.line_editor,
            );
            if !built.same_content(&self.snapshot) {
                self.snapshot = Arc::new(built);
            }
            return Arc::clone(&self.snapshot);
        }
        Arc::clone(&self.snapshot)
    }

    /// Whether anything has happened that might have changed the grid. A cheap
    /// "should I bother taking a snapshot?".
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Changes the size of the grid, reflowing the scrollback into it.
    ///
    /// This does not tell the child process anything; only the pty can do that.
    /// [`crate::Terminal::resize`] does both.
    pub fn resize(&mut self, size: TerminalSize) {
        let columns_changed = size.columns != self.size.columns;
        let rows_changed = size.rows != self.size.rows;
        let shortened = size.rows < self.size.rows;
        let geometry_changed = columns_changed || rows_changed;
        self.size = size;
        if !geometry_changed {
            return;
        }
        self.term.resize(size);
        // A column change reflows the scrollback, which moves every line the
        // open block is anchored against; a row change does not, and the
        // anchor's own arithmetic already covers the lines a taller screen
        // pulls back out of the history. Afterwards, because the open block's
        // first row is then re-found on the reflowed grid rather than guessed
        // at.
        if columns_changed {
            self.blocks.reflowed(&self.term);
        }
        // A shorter screen pushes its top rows into the history, and a
        // taller one pulls them back out: the shell's as well under a
        // full-screen program, where its history cannot be read.
        if rows_changed {
            self.blocks.rows_changed(&self.term, shortened);
        }
        self.settle_history();
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

    /// Tests: how many pictures the open block holds, under a full-screen
    /// program too, where a [`Snapshot`]'s live block shows none of them.
    #[cfg(test)]
    pub(crate) fn open_pictures(&self) -> usize {
        self.blocks.open_pictures()
    }

    /// Copies lines out of the grid, addressed from the oldest line of the
    /// scrollback rather than from the top of the screen.
    ///
    /// The same store a finished block's rows live in, filled from the grid
    /// the same way, which is what lets a pane that draws one grid rather than
    /// a list of blocks copy a selection through exactly one implementation.
    /// Rows are numbered from zero at the oldest line the history still holds,
    /// so they do not move under a viewport that scrolls; what does move them
    /// is the history filling up and evicting from the top, and the clamped
    /// first row is returned so a caller can see that it did.
    pub fn harvest_rows(&self, first: usize, last: usize) -> (BlockRows, usize) {
        let history = self.history_len() as i32;
        let top = first as i32 - history;
        let rows = harvest::harvest(&self.term, &self.palette, top, last as i32 - history);
        let oldest = top.max(-history);
        (rows, (oldest + history).max(0) as usize)
    }

    /// The title the child last asked for.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The working directory the child last reported with OSC 7. `None` until a
    /// shell reports one, which most do only once they are configured to.
    ///
    /// On whichever machine the report named, which may not be this one: see
    /// [`TerminalEvent::WorkingDirectory`].
    pub fn working_directory(&self) -> Option<&Path> {
        self.working_directory.as_deref()
    }

    /// What the program in the pane last said it was doing.
    pub fn agent(&self) -> AgentReport {
        self.agent
    }

    /// Everything the child has asked for since the last call.
    pub fn take_events(&mut self) -> Vec<TerminalEvent> {
        std::mem::take(&mut self.events)
    }

    /// The bytes owed back to the child, in answer to its queries. The caller
    /// must write these to the pty. Always empty on a mirror — see
    /// [`Self::advance_mirrored`].
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// Records that the child process has finished: queues the event, and
    /// closes the open block, which nothing in the escape stream will.
    ///
    /// Terminating is absorbing. A shell that has exited never comes back, and
    /// anything still printing on the pty afterwards is a child that outlived
    /// it, so no later mark reopens the session.
    pub(crate) fn child_exited(&mut self, exit: ChildExit) {
        self.events.push(TerminalEvent::ChildExited(exit));
        // Nothing is left to give it back, and a pane whose shell has gone
        // draws what it left the way it always has.
        self.line_editor = false;
        self.blocks.exited(
            &mut self.term,
            &self.palette,
            Whereabouts::of(
                self.working_directory.as_deref(),
                self.working_host.as_deref(),
            ),
        );
        self.dirty = true;
    }

    /// Which encoding the child currently expects for cursor and keypad keys.
    pub fn input_modes(&self) -> InputModes {
        let mode = self.term.mode();
        InputModes {
            application_cursor: mode.contains(TermMode::APP_CURSOR),
            application_keypad: mode.contains(TermMode::APP_KEYPAD),
            keyboard: KeyboardModes {
                disambiguate: mode.contains(TermMode::DISAMBIGUATE_ESC_CODES),
                report_events: mode.contains(TermMode::REPORT_EVENT_TYPES),
                report_alternates: mode.contains(TermMode::REPORT_ALTERNATE_KEYS),
                report_all: mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
                report_text: mode.contains(TermMode::REPORT_ASSOCIATED_TEXT),
            },
        }
    }

    /// Which mouse reports the child has asked for.
    ///
    /// All false — [`MouseModes::NONE`] — is the state a shell sits in, and is
    /// what tells a caller that the pointer belongs to the person rather than
    /// to the program.
    pub fn mouse_modes(&self) -> MouseModes {
        let mode = self.term.mode();
        MouseModes {
            click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
            drag: mode.contains(TermMode::MOUSE_DRAG),
            motion: mode.contains(TermMode::MOUSE_MOTION),
            sgr: mode.contains(TermMode::SGR_MOUSE),
            alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
        }
    }

    /// Whether the child asked to be told when the keyboard arrives and
    /// leaves — `?1004h`, xterm's focus reporting.
    pub fn focus_reporting(&self) -> bool {
        self.term.mode().contains(TermMode::FOCUS_IN_OUT)
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

    /// Sets what this answers a program that asks which terminal it is,
    /// with XTVERSION (`CSI > q`): a name and a version, such as
    /// `Crook 0.1.21`, which goes back as `DCS > | Crook 0.1.21 ST`.
    ///
    /// The embedding application's name rather than this crate's, which is
    /// why it is handed in: a program asking is asking which terminal it is
    /// in, and some decide what to use on the answer — Claude Code asks
    /// whether synchronized output is supported only once this question has
    /// been answered, and draws frames that can be painted half done when
    /// it has not. `None`, the default, leaves the question unanswered, as
    /// a terminal that does not know it would.
    pub fn set_name_and_version(&mut self, name_and_version: Option<&str>) {
        self.version_reply = name_and_version.map(|name| format!("\x1bP>|{name}\x1b\\"));
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

        // The update's bytes reach the grid only now, so this is a feed like
        // any other.
        let view = self.view();
        self.parser.stop_sync(&mut self.term);
        self.update_let_go();
        self.settle_history();
        self.keep_view(view);
        self.dirty = true;
        self.drain();
    }

    /// Where the viewport stands now, taken before a feed so that
    /// [`Self::keep_view`] can be handed it after.
    fn view(&self) -> View {
        let grid = self.term.grid();
        View {
            alt_screen: self.term.mode().contains(TermMode::ALT_SCREEN),
            offset: grid.display_offset(),
            history: grid.history_size(),
        }
    }

    /// Puts a scrolled-back viewport back on the lines it showed before a
    /// feed, and never lets it past the oldest line there is.
    ///
    /// `alacritty_terminal` 0.26 raises the offset of a scrolled-back view by
    /// every line it scrolls up, so that the view stays on its lines while they
    /// move up into the history. Only a scroll that starts at the screen's top
    /// row moves anything into the history, though: one inside a scroll region
    /// set below row 1 (`CSI 5;24 r`, `tput csr`, a log follower pinning a
    /// header), or a line deleted below row 1 (`CSI M`), shuffles rows within
    /// the screen and leaves the history as it was — and the offset is raised
    /// all the same, capped at the scrollback setting rather than at the
    /// history there is. The view crept up the history as such output
    /// arrived, and once it was past the oldest line the snapshot indexed rows
    /// the grid does not have and panicked, taking down whichever thread
    /// asked for the frame.
    ///
    /// The lines that really reached the history are the lines it grew by, so
    /// the offset may rise by exactly that many: the view stays on what the
    /// reader was looking at, which is what it does for ordinary output, rather
    /// than drifting up or jumping to the live output. The count is exact
    /// until the history is full, when every line scrolled in evicts one and
    /// the size stops moving; from there alacritty's own figure stands, which
    /// is right for all but the scrolls above and cannot pass a history as long
    /// as its cap. A feed that switched screens compares two different grids,
    /// and alacritty's figure stands there too. Past both, the offset is held
    /// to the history there is.
    fn keep_view(&mut self, before: View) {
        let now = self.view();
        let mut offset = now.offset;
        if now.alt_screen == before.alt_screen && now.history < self.scrollback_lines {
            let scrolled_in = now.history.saturating_sub(before.history);
            offset = offset.min(before.offset + scrolled_in);
        }
        offset = offset.min(now.history);
        if offset < now.offset {
            let down = i32::try_from(now.offset - offset).unwrap_or(i32::MAX);
            self.term.scroll_display(Scroll::Delta(-down));
        }
    }

    /// Translates everything `Term` reported during the last operation.
    fn drain(&mut self) {
        let exited = self.hand_over_term_events();

        if let Some(serial) = self.osc_watcher.completions.take() {
            self.events.push(TerminalEvent::Completions(serial));
        }

        // A mirror's stream is answered by the emulator it mirrors. Here and
        // not at the end of a mirrored feed, because this is also where a
        // synchronized update that a snapshot let go gets its answers.
        if self.mirrored {
            self.replies.clear();
        }

        self.apply_reports();
        if let Some(url) = self.osc_watcher.pull_request.take() {
            self.events.push(TerminalEvent::PullRequest(url));
        }
        self.apply_working_directory();

        // Last, because it closes the open block: a working directory reported
        // on the way out belongs to the block that is still open.
        if let Some(exit) = exited {
            self.child_exited(exit);
        }
    }

    /// Translates what `Term` has reported since this was last called, in
    /// the order it reported it: [`Self::drain`] after every operation, and
    /// [`Self::advance`] before each mark too, ahead of what the mark settles.
    ///
    /// A child's exit is returned rather than applied: it closes the open
    /// block, which is the last thing done with a feed, so the caller applies
    /// it once everything else is in. `Term` itself never reports one — the
    /// exit comes from [`crate::Terminal::try_wait`] — so this only keeps the
    /// arm where it always was.
    fn hand_over_term_events(&mut self) -> Option<ChildExit> {
        let mut exited = None;
        for event in self.proxy.take() {
            match event {
                Event::Title(title) => self.set_title(Some(title)),
                Event::ResetTitle => self.set_title(None),
                Event::Bell => self.events.push(TerminalEvent::Bell),
                Event::Exit => self.events.push(TerminalEvent::Exit),
                Event::ChildExit(status) => {
                    let exit = ChildExit::from_code(status.code().unwrap_or(1) as u32);
                    exited = Some(exit);
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
        exited
    }

    /// Hands over the pending agent report and notification, in the order
    /// the program wrote them.
    ///
    /// Each is the last of its kind since the watcher last stopped, and the
    /// two are read against each other: a `running` takes away the look a
    /// notification asked for, a notification after it asks again, and a
    /// status change after one takes its place. Handing them over in a fixed
    /// order would make the same bytes settle one way in one read and another
    /// way split across two. Idempotent, like [`Self::apply_agent_report`].
    fn apply_reports(&mut self) {
        let notified_last = std::mem::take(&mut self.osc_watcher.notified_last);
        if !notified_last {
            self.apply_notification();
        }
        self.apply_agent_report();
        if notified_last {
            self.apply_notification();
        }
    }

    /// Emits a pending notification. Idempotent: it takes it.
    fn apply_notification(&mut self) {
        if let Some(notification) = self.osc_watcher.notification.take() {
            self.events.push(TerminalEvent::Notification(notification));
        }
    }

    /// Moves a pending agent report onto `self.agent`, emitting the change.
    ///
    /// Kept out of the OSC parser so that [`Self::settle_agent`] can be made to
    /// run against an up-to-date status: a report and the mark that ends it can
    /// arrive in one read, and the mark has to see the report. Idempotent — it
    /// takes the pending report, so a second call in the same [`Self::advance`]
    /// (from [`Self::drain`]) finds nothing.
    fn apply_agent_report(&mut self) {
        if let Some(reported) = self.osc_watcher.agent.take()
            && (reported.status != self.agent
                || reported.title.is_some()
                || reported.message.is_some())
        {
            self.agent = reported.status;
            self.events.push(TerminalEvent::Agent(reported));
        }
    }

    /// Moves a pending working directory onto `self.working_directory`,
    /// emitting the change.
    ///
    /// Kept out of the OSC parser for the reason [`Self::apply_agent_report`]
    /// is: a directory and the prompt mark after it can arrive in one read,
    /// and the block the mark opens has to be filed under that directory.
    /// Idempotent the same way — it takes the pending directory, so the call
    /// from [`Self::drain`] after one from [`Self::advance`] finds nothing.
    ///
    /// A change of host is a change too, with the path the same: `ssh` into
    /// a machine laid out like this one, and back out, moves the shell twice
    /// and leaves the path where it was both times.
    fn apply_working_directory(&mut self) {
        if let Some((host, directory)) = self.osc_watcher.working_directory.take()
            && (self.working_directory.as_deref() != Some(directory.as_path())
                || self.working_host != host)
        {
            self.working_directory = Some(directory.clone());
            self.working_host = host.clone();
            self.events.push(TerminalEvent::WorkingDirectory {
                host,
                path: directory,
            });
        }
    }

    /// Takes a status back when the command that reported it is over.
    ///
    /// A program that was interrupted never says it stopped, so the shell's
    /// own marks say it: `D` ends the command a running or waiting agent was,
    /// and with it the claim that anything is running or waiting. A failure
    /// outlives its command on purpose — it is the one status worth seeing
    /// after the fact — and goes when the *next* command starts, because new
    /// work is the thing that answers it.
    ///
    /// `D` also lets go of the session the pane held, whatever the status,
    /// and forgets the sessions that said they start: an agent interrupted
    /// mid-turn never said it stopped, and the next one started here reports
    /// from sessions of its own.
    fn settle_agent(&mut self, mark: ShellMark) {
        if matches!(mark, ShellMark::CommandFinished(_)) {
            self.osc_watcher.sessions.release();
        }
        let over = matches!(
            (mark, self.agent),
            (
                ShellMark::CommandFinished(_),
                AgentReport::Running | AgentReport::NeedsInput
            ) | (ShellMark::OutputStart, AgentReport::Failed)
        );
        if over {
            self.agent = AgentReport::Idle;
            self.events.push(TerminalEvent::AgentSettled);
        }
    }

    /// Records a new title, which every future snapshot carries.
    ///
    /// Cut to [`agent::TITLE_BYTES`] first, and compared after, so the same
    /// over-long title twice is no news: it is whatever the program wrote,
    /// `vte` keeps an OSC however long it runs, and the row shapes the title
    /// again on every frame it draws.
    fn set_title(&mut self, title: Option<String>) {
        let title = title.map(|title| agent::capped(title, agent::TITLE_BYTES));
        if self.title == title {
            return;
        }
        self.title = title.clone();
        self.dirty = true;
        self.events.push(TerminalEvent::Title(title));
    }

    /// Hands `Term` the cut title in place of the over-long one it was just
    /// given, which the watcher stopped on.
    ///
    /// `Term` keeps a title of its own, whole, and `CSI 22 t` pushes a copy
    /// of it onto a stack 4096 deep: four megabytes of title with twenty
    /// kilobytes of pushes behind it are 4096 copies, sixteen gigabytes,
    /// however short [`Self::set_title`] made the one this crate keeps.
    /// Given the cut one before the rest of the read is parsed, the pushes
    /// copy that instead.
    ///
    /// Inside a synchronized update `vte` holds the bytes and plays them to
    /// `Term` when the update ends, the title and the pushes after it
    /// together, and `Term` has not seen the title yet. Cutting it there
    /// means ending the update, as one nobody ended is ended, and what is
    /// left of the frame is then drawn as it arrives. A title of `bytes` up
    /// to [`UPDATE_TITLE_BYTES`] is left to the update instead, `Term` keeps
    /// it whole, and its copies come to 16 MiB at most. A torn frame only
    /// pays for itself past that: a megabyte of title pushed 4096 times is
    /// four gigabytes. [`Self::set_title`] cuts the copy this crate keeps
    /// either way, once `Term` hands the title over.
    ///
    /// The cut title goes back to the proxy as `Term` sends it, and is no
    /// news when it is handed over: it is the title this already has.
    fn cut_grid_title(&mut self, bytes: usize) -> Option<ChildExit> {
        if self.parser.sync_timeout().sync_timeout().is_some() {
            // `self.title` is still the one from before the update, and
            // handing `Term` that would only be undone when the update ends.
            if bytes <= UPDATE_TITLE_BYTES {
                return None;
            }
            self.parser.stop_sync(&mut self.term);
            self.update_let_go();
        }
        let exited = self.hand_over_term_events();
        if let Some(title) = &self.title {
            Handler::set_title(&mut self.term, Some(title.clone()));
        }
        exited
    }
}

#[cfg(test)]
#[path = "emulator_tests.rs"]
mod tests;
