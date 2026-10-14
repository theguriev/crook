//! The link under the pointer in one pane's output.
//!
//! Kept by the workspace, per pane, for exactly the reason
//! [`PaneSelection`](crate::pane_selection::PaneSelection) is: the element tree
//! is thrown away and rebuilt on every frame, and this is a fact about where
//! the pointer is that has to survive between two of them. The move that finds
//! a link and the frame that underlines it are different frames.
//!
//! # Why a link is only live while a modifier is held
//!
//! Because the pointer is already spoken for. Dragging across the output
//! selects it, and a terminal where a click on a URL opened a browser instead
//! of placing a selection would be a terminal you cannot copy a URL out of.
//! Every terminal resolves this the same way: hold the platform's own chord
//! key and links light up; let go and the pointer goes back to selecting. So
//! this is set only while that key is down, and cleared the moment it comes up
//! — which is what makes the underline appear and disappear under a pointer
//! that never moved.
//!
//! # A folded line is one link
//!
//! [`find`] is the scan itself, and both surfaces go through it. The URL a
//! build tool prints is routinely longer than the pane is wide, and the
//! terminal folds it onto the next row without the program ever printing a
//! newline — the emulator marks the fold with [`CellFlags::WRAPLINE`], and a
//! copy already reads that flag to hand back the one line the shell printed.
//! A scan of a single row would find the link only as far as the fold, and
//! open a URL cut off at the pane's edge. So the row under the pointer is
//! joined with the rows the fold connects it to, above and below, and the span
//! that comes back is mapped onto every row it crosses. A hard line break —
//! two URLs printed on adjacent rows — carries no flag and is not joined.
//!
//! # A path is a link once its file has been found
//!
//! What an agent or a compiler prints most is not a URL but a file —
//! `--> src/main.rs:42:5`, `Update(app/src/view.rs)` — and [`find`] answers
//! with one of those too, read against the directory the command ran in. A
//! word that looks like a path is only a guess, though, until somebody has
//! looked, and underlining `and/or` under a pointer would be a link that opens
//! nothing. So a path is not shown as a link when it is found: [`PaneLink`]
//! keeps it waiting, the workspace looks for the file on the background pool,
//! and the underline appears when the answer says there is something there to
//! open — a frame or so after the chord key went down, on a disk that answers.
//!
//! The look is never on the thread that draws, because it is a `stat`, and on
//! a network mount whose server has gone a `stat` blocks for as long as the
//! mount does — for good, on a hard NFS mount. The same reason keeps it to one
//! look per pane at a time: each look into such a mount is a worker of the
//! pool that does not come back, and a pointer run along a line of paths into
//! it would otherwise take every one of them. What was found is kept while the
//! chord key is held and forgotten when it comes up, so a file a command has
//! since written, or removed, is looked for again the next time.
//!
//! [`CellFlags::WRAPLINE`]: crook_terminal::CellFlags::WRAPLINE

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crook_terminal::Rows;
use crook_terminal::url;
use unicode_width::UnicodeWidthChar;

/// A link under the pointer: what it opens, and the cells it covers.
///
/// One range of cells per row, top row first, because a line the terminal
/// folded puts one link on two rows — and the underline and the click have to
/// agree that both of them are it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    /// The cells it covers, one range per row it crosses, in row order. Never
    /// empty.
    pub cells: Vec<LinkCells>,
    /// What a click opens.
    pub target: LinkTarget,
}

/// What a link opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// An address with a scheme `crook_terminal::url` recognises, handed to
    /// the platform's handler as it was printed.
    Url(String),
    /// A file a program printed the path of.
    File(FileLink),
}

/// A printed path, read against the directory of the command that printed
/// it, and the place in the file it named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLink {
    /// Where the file is: absolute, or as absolute as the directory it was
    /// read against. Joined and never resolved further — canonicalizing is a
    /// walk of the disk, and this is found on a pointer move.
    pub path: PathBuf,
    /// The line printed after it, if one was.
    pub line: Option<u32>,
    /// The column printed after the line, if one was.
    pub column: Option<u32>,
}

/// The cells of one row a link occupies, in the coordinates of the surface
/// that found it.
///
/// The two surfaces number their rows differently — a grid row is a row of the
/// viewport, a block row is a row of one finished command — so the row is
/// stored beside a tag saying which it is. Without it, scrolling the list would
/// leave an underline on whichever row happened to inherit the number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkCells {
    /// Which surface, and which of its rows.
    pub row: LinkRow,
    /// The first cell of the row the link occupies.
    pub start: usize,
    /// How many cells it covers.
    pub len: usize,
}

/// Which row of which surface a link was found on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinkRow {
    /// A row of the live viewport, as the grid draws it.
    Viewport(usize),
    /// A row of one finished block: its index in the list, and the row within
    /// it.
    Block {
        /// Which block, by its position in the pane's history.
        index: usize,
        /// Which of that block's rows.
        row: usize,
    },
}

/// The link under the pointer in one pane, if there is one, and what is known
/// about the files the paths under it name.
///
/// Cheap to clone — it is an [`Rc`] — because the elements that draw a pane's
/// output take one every frame.
#[derive(Clone, Default)]
pub struct PaneLink(Rc<RefCell<Following>>);

/// What [`PaneLink`] holds.
#[derive(Default)]
struct Following {
    /// The link under the pointer, underlined.
    shown: Option<LinkSpan>,
    /// A path under the pointer whose file has not been found yet. It is
    /// [`Self::shown`] the moment the look says there is one.
    waiting: Option<LinkSpan>,
    /// What looking for files has found while the chord key has been held,
    /// by the path looked for — and the looks still out, which outlive the
    /// chord.
    looked: HashMap<PathBuf, Look>,
    /// The file a chord-click landed on, for the workspace to open.
    pressed: Option<(FileLink, Opens)>,
}

/// Where looking for one path's file has got to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Look {
    /// Asked, and not answered yet: on a mount whose server has gone, never.
    Out,
    /// Answered: how it opens, or `None` for nothing there to open.
    Found(Option<Opens>),
}

impl PaneLink {
    /// A pane with no link under the pointer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The link under the pointer, if there is one.
    pub fn get(&self) -> Option<LinkSpan> {
        self.0.borrow().shown.clone()
    }

    /// The cells of the link under the pointer that lie on a given row, if
    /// any do.
    ///
    /// What the paint path asks: it is drawing one row and wants to know
    /// whether any of its cells are underlined. A link on a folded line
    /// answers for each row it crosses.
    pub fn on(&self, row: LinkRow) -> Option<LinkCells> {
        self.0
            .borrow()
            .shown
            .as_ref()
            .and_then(|link| link.cells.iter().find(|cells| cells.row == row))
            .cloned()
    }

    /// What a click would open.
    pub fn target(&self) -> Option<LinkTarget> {
        self.0
            .borrow()
            .shown
            .as_ref()
            .map(|link| link.target.clone())
    }

    /// Puts a link under the pointer, reporting whether the frame changed.
    ///
    /// `None` clears it. The answer is what decides whether a pointer move is
    /// worth a repaint — a pointer travelling along one link produces a move
    /// per pixel and exactly one frame.
    ///
    /// A path is shown only once its file has been found. Until then it is
    /// kept waiting, and [`Self::wants_a_look`] says so; one whose look found
    /// nothing is no link at all.
    pub fn set(&self, link: Option<LinkSpan>) -> bool {
        let mut following = self.0.borrow_mut();
        let look = match link.as_ref().map(|link| &link.target) {
            Some(LinkTarget::File(file)) => Some(following.looked.get(&file.path).copied()),
            _ => None,
        };
        let (shown, waiting) = match look {
            // A URL, or nothing: no file to look for.
            None | Some(Some(Look::Found(Some(_)))) => (link, None),
            Some(Some(Look::Found(None))) => (None, None),
            Some(Some(Look::Out) | None) => (None, link),
        };
        following.waiting = waiting;
        if following.shown == shown {
            return false;
        }
        following.shown = shown;
        true
    }

    /// Follows the pointer, reporting whether the frame changed: with the
    /// chord key held, puts what `find` finds under it; without it, lets go
    /// of everything, the way [`Self::let_go`] does.
    ///
    /// What both surfaces do on a move and on a modifier changing, in one
    /// place, because the second half is not only the underline going: it is
    /// what was found being forgotten, so that a file a command has written
    /// or removed since is looked for again. `find` is not asked without the
    /// chord.
    pub fn track(&self, chord: bool, find: impl FnOnce() -> Option<LinkSpan>) -> bool {
        if !chord {
            return self.let_go();
        }
        self.set(find())
    }

    /// Whether there is a link under the pointer at all.
    pub fn is_some(&self) -> bool {
        self.0.borrow().shown.is_some()
    }

    /// The cells of the link that are on viewport rows this grid still draws,
    /// with the row each range is on.
    ///
    /// The bound matters: a grid that shrank between the move that found the
    /// link and the frame that draws it would otherwise underline a row
    /// outside its own box.
    pub fn on_viewport_rows(&self, rows: usize) -> Vec<(usize, LinkCells)> {
        self.0
            .borrow()
            .shown
            .as_ref()
            .map(|link| {
                link.cells
                    .iter()
                    .filter_map(|cells| match cells.row {
                        LinkRow::Viewport(row) if row < rows => Some((row, cells.clone())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether a path under the pointer is waiting for a look that has not
    /// been started — which the element that found it asks the workspace for.
    pub fn wants_a_look(&self) -> bool {
        self.0.borrow().next_look().is_some()
    }

    /// The path to look for next, written down as being looked for.
    ///
    /// `None` while another look of this pane's is still out: see the module
    /// documentation for why there is only ever one.
    pub fn take_look(&self) -> Option<PathBuf> {
        let mut following = self.0.borrow_mut();
        let path = following.next_look()?.clone();
        following.looked.insert(path.clone(), Look::Out);
        Some(path)
    }

    /// Writes down what looking for `path` found, reporting whether the frame
    /// changed: a path still under the pointer underlines the moment its file
    /// turns out to be there.
    pub fn found(&self, path: &Path, opens: Option<Opens>) -> bool {
        let mut following = self.0.borrow_mut();
        following
            .looked
            .insert(path.to_path_buf(), Look::Found(opens));
        let waiting_on_it = matches!(
            &following.waiting,
            Some(LinkSpan { target: LinkTarget::File(file), .. }) if file.path == path
        );
        if !waiting_on_it {
            return false;
        }
        let waiting = following.waiting.take();
        if opens.is_none() {
            return false;
        }
        following.shown = waiting;
        true
    }

    /// The chord key came up: the pointer is selecting again, so nothing is
    /// underlined, and what was found is forgotten so that the next time the
    /// key goes down the disk is asked again. The looks still out are kept,
    /// so that a path on a dead mount is not looked into a second time.
    /// Reports whether the frame changed.
    pub fn let_go(&self) -> bool {
        let mut following = self.0.borrow_mut();
        following.waiting = None;
        if !following.looked.is_empty() {
            following.looked.retain(|_, look| *look == Look::Out);
        }
        following.shown.take().is_some()
    }

    /// A chord-click on a path: written down for the workspace to open when
    /// its file has been found and opens, and refused — so the press selects,
    /// as it would anywhere that is not underlined — when it has not.
    pub fn press(&self, file: FileLink) -> bool {
        let mut following = self.0.borrow_mut();
        let Some(Look::Found(Some(opens))) = following.looked.get(&file.path).copied() else {
            return false;
        };
        following.pressed = Some((file, opens));
        true
    }

    /// The file a chord-click landed on, and how it opens, once.
    pub fn take_pressed(&self) -> Option<(FileLink, Opens)> {
        self.0.borrow_mut().pressed.take()
    }
}

impl Following {
    /// The path under the pointer that has not been looked for, unless a look
    /// is already out.
    fn next_look(&self) -> Option<&PathBuf> {
        if self.looked.values().any(|look| *look == Look::Out) {
            return None;
        }
        match &self.waiting.as_ref()?.target {
            LinkTarget::File(file) if !self.looked.contains_key(&file.path) => Some(&file.path),
            _ => None,
        }
    }
}

/// What looking for a printed path found on disk.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// A file somebody reads.
    File,
    /// A folder.
    Folder,
    /// Something the platform's handler would run rather than show: a file
    /// marked executable, one the platform runs by its extension, or an
    /// application bundle on macOS — by the name it was printed with or by
    /// what that resolves to.
    Program,
    /// Nothing, or nothing a click should open: a socket, a pipe, a device —
    /// an editor handed a pipe waits on it for ever.
    Nothing,
}

impl Found {
    /// Looks for `path` on disk.
    ///
    /// A `stat`, which blocks for as long as the mount it is on does: never
    /// on the thread that draws.
    ///
    /// The platform's handler opens what a path comes to, not what it was
    /// printed as — a link named `docs` to `Tool.app`, `X.app/Contents/..` —
    /// so what it resolves to is asked about as well as the name it was
    /// printed with. Resolving it is a walk of the disk, which is one more
    /// reason this is done here and not when the path is found.
    pub fn at(path: &Path) -> Self {
        let Ok(metadata) = std::fs::metadata(path) else {
            return Self::Nothing;
        };
        let resolved = std::fs::canonicalize(path).ok();
        let named = |kinds: &[&str]| {
            std::iter::once(path)
                .chain(resolved.as_deref())
                .any(|path| has_extension(path, kinds))
        };
        if metadata.is_dir() {
            return if named(BUNDLES) {
                Self::Program
            } else {
                Self::Folder
            };
        }
        if !metadata.is_file() {
            return Self::Nothing;
        }
        #[cfg(unix)]
        let marked = {
            use std::os::unix::fs::PermissionsExt as _;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let marked = false;
        if marked || named(WINDOWS_PROGRAMS) || named(RUNS) {
            Self::Program
        } else {
            Self::File
        }
    }
}

/// Whether `path`'s extension is one of `kinds`, in any case.
fn has_extension(path: &Path, kinds: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            kinds
                .iter()
                .any(|kind| kind.eq_ignore_ascii_case(extension))
        })
}

/// Windows' programs, installers, shortcuts and batch files, which are
/// programs on every platform rather than on Windows alone.
///
/// Elsewhere Wine runs them. Its desktop entry, `wine start /unix %f`, claims
/// `application/x-ms-dos-executable`, `x-msi`, `x-ms-shortcut`, `x-bat` and
/// `x-mswinurl`, which shared-mime-info gives to `.exe`, `.scr`, `.cpl`,
/// `.msi`, `.lnk`, `.bat`, `.cmd` and `.url` — so with Wine installed,
/// `xdg-open` on a `setup.exe` that `unzip` left without its exec bit starts
/// the installer. On macOS an application that claims one is usually a port
/// of the same Wine. `.com`, `.pif` and `.msp` are here for what Windows
/// itself does with them.
const WINDOWS_PROGRAMS: &[&str] = &[
    "exe", "com", "scr", "pif", "cpl", "msi", "msp", "lnk", "url", "bat", "cmd",
];

/// The other extensions of a file that the platform's handler runs, or hands
/// to something that runs what is in it, when the file is opened the way a
/// double-click opens it — which is what `explorer` given a `file://` URL
/// does.
///
/// Windows has nothing but the name to tell a program by: beside
/// [`WINDOWS_PROGRAMS`], its packages, its other shortcuts and its script
/// hosts, and the scripts the usual developer's installs take over — Git for
/// Windows runs `.sh` in Git Bash, Python's launcher runs `.py`, Ruby's and
/// Perl's installers theirs.
#[cfg(not(unix))]
const RUNS: &[&str] = &[
    // Programs, installers and packages.
    "msc",
    "msu",
    "appx",
    "appxbundle",
    "msix",
    "msixbundle",
    "appinstaller",
    "application",
    "appref-ms",
    "xbap",
    "gadget",
    "diagcab",
    // Shortcuts, which open whatever they point at.
    "scf",
    "settingcontent-ms",
    "library-ms",
    "searchconnector-ms",
    // What Windows runs itself: the script host, PowerShell, HTML
    // applications, compiled help, the registry.
    "vb",
    "vbs",
    "vbe",
    "js",
    "jse",
    "ws",
    "wsf",
    "wsh",
    "wsc",
    "sct",
    "hta",
    "chm",
    "ps1",
    "psm1",
    "reg",
    // What the usual installs register a runner for.
    "sh",
    "bash",
    "py",
    "pyw",
    "pyz",
    "pyzw",
    "pyc",
    "pyo",
    "rb",
    "rbw",
    "pl",
    "jar",
    "jnlp",
];

/// The same on macOS, for a file that is not marked executable: what
/// Terminal runs (`.command`, `.tool`, and a `.terminal` that names a
/// command), what Jar Launcher runs, what Python's own launcher runs — its
/// scripts and its bytecode, which it claims as well and runs the moment it
/// is opened — the installer, and the location files that open whatever they
/// point at.
#[cfg(target_os = "macos")]
const RUNS: &[&str] = &[
    "command", "tool", "terminal", "jar", "py", "pyw", "pyc", "pyo", "pkg", "mpkg", "fileloc",
    "inetloc", "webloc",
];

/// The same elsewhere: a `.jar`, which Java's desktop entry runs with
/// `java -jar`, and a `.desktop` launcher.
#[cfg(all(unix, not(target_os = "macos")))]
const RUNS: &[&str] = &["jar", "desktop"];

/// The folders `open` hands to Launch Services to start, or to install, rather
/// than showing them in the Finder: an application, an installer package in
/// its folder form — which a `.mpkg` almost always is — and the bundles that
/// install into the system.
#[cfg(target_os = "macos")]
const BUNDLES: &[&str] = &[
    "app", "pkg", "mpkg", "prefpane", "saver", "workflow", "action",
];

/// Elsewhere a folder is a folder.
#[cfg(not(target_os = "macos"))]
const BUNDLES: &[&str] = &[];

/// How a file link opens, once its file has been found.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Opens {
    /// In the editor `$VISUAL` or `$EDITOR` names, at the line.
    InEditor,
    /// With the platform's own handler, as a `file://` URL: a folder in the
    /// file manager, or a file when no editor that opens a window is named.
    ByPlatform,
}

impl Opens {
    /// How what was found opens, given whether an editor with a window of its
    /// own is named — or `None`, when it is not something to open.
    ///
    /// A program goes to the editor or nowhere. The platform's handler runs
    /// one, and a chord-click on a path a program printed must not be a way
    /// to start whatever is at it — the rule `browser::open_folder` keeps for
    /// the same handler. A folder goes to the file manager whatever editor is
    /// named, since not every editor opens one.
    pub fn of(found: Found, editor: bool) -> Option<Self> {
        match found {
            Found::File if editor => Some(Self::InEditor),
            Found::File | Found::Folder => Some(Self::ByPlatform),
            Found::Program => editor.then_some(Self::InEditor),
            Found::Nothing => None,
        }
    }
}

/// `path` as a `file://` URL, which is how the platform's handler is given a
/// file: through `browser::open` and its list of schemes, like any address a
/// program printed.
///
/// Every byte but the ones a URL path may carry as they are is escaped, so a
/// space or a `%` in a directory name means the same file on the far side.
pub fn file_url(path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt as _;
        path.as_os_str().as_bytes().to_vec()
    };
    // `C:\x` is `file:///C:/x`: the drive is the first segment of a path
    // that starts with a slash of its own.
    #[cfg(not(unix))]
    let bytes = {
        let forward = path.to_string_lossy().replace('\\', "/");
        if forward.starts_with('/') {
            forward.into_bytes()
        } else {
            format!("/{forward}").into_bytes()
        }
    };

    let mut url = String::from("file://");
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
    url
}

/// The link under a cell of a block's rows, or `None` when the cell is not
/// part of one.
///
/// `row` and `column` are the cell under the pointer, in the block's own
/// numbering; `place` names each row of the block on the surface the caller
/// draws, which is what the cells that come back are tagged with.
///
/// The rows the terminal folded around `row` are scanned as the one line they
/// are: every row contributes exactly its width, one `char` per cell, so a
/// cell of the joined text is a row and a column by arithmetic alone. Nothing
/// is trimmed on the way in — a blank cell ends a URL exactly as it does on
/// one row, and a line that folded right after a space stays two words rather
/// than one. The price is a double-width character that would not fit at the
/// margin: the blank it leaves there ends the link, so a URL with such a
/// character exactly at the fold is found only up to it.
///
/// A URL is looked for first, and a path where there is none — read against
/// `directory`, the one the command that printed it ran in. A relative path
/// with no directory to read it against is not a link: a shell that never
/// said where it is cannot say where its output's paths are either.
pub fn find(
    rows: &Rows<'_>,
    row: usize,
    column: usize,
    directory: Option<&Path>,
    place: impl Fn(usize) -> LinkRow,
) -> Option<LinkSpan> {
    let columns = rows.columns();
    if row >= rows.count() || column >= columns {
        return None;
    }
    let mut first = row;
    while first > 0 && rows.wraps(first - 1) {
        first -= 1;
    }
    // Bounded by the block: `wraps` is never true of its last row.
    let mut last = row;
    while rows.wraps(last) {
        last += 1;
    }

    let mut text = String::with_capacity((last - first + 1) * columns);
    for folded in first..=last {
        let mut cells = vec![' '; columns];
        // The marks stacked on a cell arrive after it, at its column, and the
        // scan counts cells: the cell's own character is the one that stands
        // for it. A wide character's spacer column is visited by nothing, and
        // is given a stand-in that continues a URL — left blank, as it is on
        // the grid, it ended one, and `https://ja.wikipedia.org/wiki/東京`
        // was a link to `…/wiki/東`.
        let mut filled = None;
        rows.visit_line(folded, columns, |character, at| {
            if filled != Some(at) {
                cells[at] = character;
                filled = Some(at);
                if character.width() == Some(2)
                    && let Some(spacer) = cells.get_mut(at + 1)
                {
                    *spacer = SPACER;
                }
            }
        });
        text.extend(cells);
    }

    let at = (row - first) * columns + column;
    let (start, end, target) = match url::at(&text, at) {
        Some(url) => (
            url.start,
            url.end(),
            LinkTarget::Url(url.uri.replace(SPACER, "")),
        ),
        None => {
            let printed = crook_terminal::path::at(&text, at)?;
            let path = resolve(
                &printed.path.replace(SPACER, ""),
                directory,
                std::env::home_dir,
            )?;
            (
                printed.start,
                printed.end(),
                LinkTarget::File(FileLink {
                    path,
                    line: printed.line,
                    column: printed.column,
                }),
            )
        }
    };
    let cells = (first..=last)
        .filter_map(|folded| {
            let row_start = (folded - first) * columns;
            let from = start.max(row_start);
            let to = end.min(row_start + columns);
            (from < to).then(|| LinkCells {
                row: place(folded),
                start: from - row_start,
                len: to - from,
            })
        })
        .collect();
    Some(LinkSpan { cells, target })
}

/// Where a printed path is: `~/` read against the home directory, an
/// absolute path as it is, and anything else against `directory`.
///
/// Joined, not looked up — nothing here touches the disk. `home` is asked
/// only for a path that starts with `~/`, because outside a `HOME` it is a
/// lookup in the user database, which is a file too, or a network service.
fn resolve(
    printed: &str,
    directory: Option<&Path>,
    home: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(rest) = printed
        .strip_prefix("~/")
        .or_else(|| printed.strip_prefix("~\\"))
    {
        return Some(home()?.join(rest));
    }
    let path = Path::new(printed);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    Some(directory?.join(path))
}

/// What a wide character's spacer column reads as while a line is scanned
/// for a link: a character from the private use area, which nothing prints
/// and nothing ends a URL or a path on. Taken out of the address or the path
/// before it is handed back; the cells it stood for stay, so the underline
/// covers the whole character.
const SPACER: char = '\u{e000}';

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crook_terminal::{Emulator, Palette, Snapshot, TerminalSize};

    use super::*;

    fn span(row: LinkRow) -> LinkSpan {
        LinkSpan {
            cells: vec![LinkCells {
                row,
                start: 4,
                len: 19,
            }],
            target: url("https://example.com"),
        }
    }

    /// A grid twenty columns wide holding whatever `bytes` drew — narrow
    /// enough that one URL folds, so the fold is the emulator's own rather
    /// than a flag a test set by hand.
    fn grid(bytes: &str) -> Arc<Snapshot> {
        let mut emulator = Emulator::new(TerminalSize::new(20, 6), 100, Palette::default());
        emulator.advance(bytes.as_bytes());
        emulator.snapshot()
    }

    /// The whole grid as the rows of one block.
    fn rows(snapshot: &Snapshot) -> Rows<'_> {
        Rows::Live {
            snapshot,
            top: 0,
            count: snapshot.rows,
        }
    }

    /// The link under a cell of the grid, tagged with viewport rows.
    fn link_at(snapshot: &Snapshot, row: usize, column: usize) -> Option<LinkSpan> {
        find(&rows(snapshot), row, column, None, LinkRow::Viewport)
    }

    /// A URL as a link's target.
    fn url(uri: &str) -> LinkTarget {
        LinkTarget::Url(uri.to_owned())
    }

    fn cells(row: usize, start: usize, len: usize) -> LinkCells {
        LinkCells {
            row: LinkRow::Viewport(row),
            start,
            len,
        }
    }

    #[test]
    fn a_url_with_wide_characters_in_it_is_one_link() {
        // Each of these is two columns, and the column after it is a spacer
        // the scan used to read as a blank — the end of the link, one
        // character into the path.
        let snapshot = grid("https://x.jp/\u{6771}\u{4eac}");
        let link = link_at(&snapshot, 0, 2).expect("the row is a link");
        assert_eq!(link.target, url("https://x.jp/\u{6771}\u{4eac}"));
        assert_eq!(link.cells, vec![cells(0, 0, 17)]);

        // From the second half of a wide character as well as the first.
        let link = link_at(&snapshot, 0, 16).expect("the spacer is part of it");
        assert_eq!(link.target, url("https://x.jp/\u{6771}\u{4eac}"));
    }

    #[test]
    fn a_url_the_terminal_folded_is_one_link_from_either_row() {
        // Thirty-six characters into twenty columns: the emulator folds it
        // after the slash, and nothing was printed at the fold.
        let snapshot = grid("https://example.com/abcdefghijklmnop");
        assert!(rows(&snapshot).wraps(0), "the emulator folded it");
        let whole = LinkSpan {
            cells: vec![cells(0, 0, 20), cells(1, 0, 16)],
            target: url("https://example.com/abcdefghijklmnop"),
        };

        assert_eq!(
            link_at(&snapshot, 0, 5),
            Some(whole.clone()),
            "from the top row"
        );
        assert_eq!(
            link_at(&snapshot, 1, 3),
            Some(whole),
            "from the row it folded onto"
        );
        assert_eq!(
            link_at(&snapshot, 1, 16),
            None,
            "the blank after it is not part of it"
        );
    }

    #[test]
    fn two_urls_on_adjacent_rows_stay_two_links() {
        // The first fills its row exactly, which is the case that looks most
        // like a fold — but the newline was printed, so there is no flag and
        // no join.
        let snapshot = grid("https://a.test/aaaaa\r\nhttps://b.test");
        assert!(!rows(&snapshot).wraps(0), "a printed newline is not a fold");

        let top = link_at(&snapshot, 0, 2).expect("the top row's link");
        assert_eq!(top.target, url("https://a.test/aaaaa"));
        assert_eq!(top.cells, vec![cells(0, 0, 20)]);

        let below = link_at(&snapshot, 1, 2).expect("the second row's link");
        assert_eq!(below.target, url("https://b.test"));
        assert_eq!(below.cells, vec![cells(1, 0, 14)]);
    }

    #[test]
    fn a_line_folded_right_after_a_space_does_not_run_the_link_into_the_next_row() {
        // The space is the twentieth cell, and the word after it folds. A copy
        // trims that space away and runs the two together; a link must not,
        // because the URL ended where the space was.
        let snapshot = grid("https://example.com next");
        assert!(rows(&snapshot).wraps(0), "the emulator folded it");

        let link = link_at(&snapshot, 0, 4).expect("the link on the top row");
        assert_eq!(link.target, url("https://example.com"));
        assert_eq!(link.cells, vec![cells(0, 0, 19)]);
        assert_eq!(link_at(&snapshot, 1, 1), None, "the word after the fold");
    }

    #[test]
    fn a_folded_link_after_wide_characters_lands_on_the_cells_it_was_drawn_in() {
        // Two double-width characters take four columns, so the link starts
        // at the sixth cell and folds fifteen characters in. Counting
        // characters instead of cells would put the underline two cells
        // left of the text on the top row and split the fold in the wrong
        // place.
        let snapshot = grid("漢字 https://example.com/abcdefg");
        assert!(rows(&snapshot).wraps(0), "the emulator folded it");
        let whole = LinkSpan {
            cells: vec![cells(0, 5, 15), cells(1, 0, 12)],
            target: url("https://example.com/abcdefg"),
        };

        assert_eq!(link_at(&snapshot, 0, 7), Some(whole.clone()));
        assert_eq!(link_at(&snapshot, 1, 2), Some(whole));
        assert_eq!(
            link_at(&snapshot, 0, 1),
            None,
            "the wide character before it"
        );
    }

    #[test]
    fn a_link_on_a_folded_line_is_offered_to_every_row_it_crosses() {
        let link = PaneLink::new();
        link.set(Some(LinkSpan {
            cells: vec![cells(2, 3, 17), cells(3, 0, 8)],
            target: url("https://example.com/abcde"),
        }));

        assert_eq!(link.on(LinkRow::Viewport(2)), Some(cells(2, 3, 17)));
        assert_eq!(link.on(LinkRow::Viewport(3)), Some(cells(3, 0, 8)));
        assert_eq!(link.on(LinkRow::Viewport(4)), None);
        // A grid that shrank to three rows between the move and the frame
        // draws the top half and not the row it no longer has.
        assert_eq!(link.on_viewport_rows(3), vec![(2, cells(2, 3, 17))]);
    }

    #[test]
    fn a_link_is_only_offered_to_the_row_it_was_found_on() {
        // The two surfaces number their rows differently. Without the tag, a
        // list scrolled by one row would underline whichever row inherited the
        // number.
        let link = PaneLink::new();
        link.set(Some(span(LinkRow::Viewport(3))));

        assert!(link.on(LinkRow::Viewport(3)).is_some());
        assert!(link.on(LinkRow::Viewport(4)).is_none());
        assert!(
            link.on(LinkRow::Block { index: 0, row: 3 }).is_none(),
            "row 3 of a block is not row 3 of the viewport"
        );
    }

    #[test]
    fn setting_the_same_link_again_is_not_a_repaint() {
        // A pointer travelling along one link produces a move per pixel. Every
        // one of them finds the same link, and exactly one of them is worth a
        // frame.
        let link = PaneLink::new();

        assert!(link.set(Some(span(LinkRow::Viewport(1)))));
        assert!(!link.set(Some(span(LinkRow::Viewport(1)))));
        assert!(link.set(Some(span(LinkRow::Viewport(2)))));
        assert!(link.set(None));
        assert!(!link.set(None));
    }

    #[test]
    fn the_element_that_is_handed_a_clone_is_looking_at_the_same_link() {
        let kept = PaneLink::new();
        let handed_to_the_element = kept.clone();

        handed_to_the_element.set(Some(span(LinkRow::Viewport(0))));

        assert!(kept.is_some());
        assert_eq!(kept.target(), Some(url("https://example.com")));
    }

    /// Where the commands in these tests ran.
    fn project() -> PathBuf {
        std::env::temp_dir().join("crook-pane-link-project")
    }

    /// A file link to `printed`, read against [`project`].
    fn file(printed: &str, line: Option<u32>, column: Option<u32>) -> FileLink {
        FileLink {
            path: project().join(printed),
            line,
            column,
        }
    }

    /// A one-cell link to a file, as the element hands [`PaneLink::set`] one.
    fn on(file: FileLink) -> LinkSpan {
        LinkSpan {
            cells: vec![cells(0, 0, 1)],
            target: LinkTarget::File(file),
        }
    }

    #[test]
    fn a_path_a_compiler_printed_is_read_against_the_directory_its_command_ran_in() {
        let snapshot = grid("--> src/main.rs:42:5");
        let project = project();
        let link = find(&rows(&snapshot), 0, 8, Some(&project), LinkRow::Viewport)
            .expect("the path is a link");

        assert_eq!(
            link.target,
            LinkTarget::File(file("src/main.rs", Some(42), Some(5)))
        );
        // The location is underlined with the path: it is what the click
        // opens at.
        assert_eq!(link.cells, vec![cells(0, 4, 16)]);
        assert_eq!(
            find(&rows(&snapshot), 0, 1, Some(&project), LinkRow::Viewport),
            None,
            "the arrow before it"
        );
    }

    #[test]
    fn a_relative_path_with_no_directory_to_read_it_against_is_not_a_link() {
        // A shell that never said where it is: the path could be anywhere.
        let snapshot = grid("src/main.rs:42");
        assert_eq!(link_at(&snapshot, 0, 3), None);
    }

    #[test]
    fn a_path_is_read_from_home_from_where_it_is_or_from_the_directory() {
        let home = || Some(PathBuf::from("home-of-the-test"));
        let project = project();
        assert_eq!(
            resolve("~/notes.md", Some(&project), home),
            Some(PathBuf::from("home-of-the-test").join("notes.md"))
        );
        assert_eq!(resolve("~/notes.md", Some(&project), || None), None);

        let absolute = std::env::temp_dir().join("elsewhere.rs");
        let printed = absolute.to_str().expect("the temporary directory is UTF-8");
        assert_eq!(resolve(printed, None, home), Some(absolute.clone()));
        assert_eq!(resolve(printed, Some(&project), home), Some(absolute));

        assert_eq!(
            resolve("./README.md", Some(&project), home),
            Some(project.join("./README.md"))
        );
        assert_eq!(resolve("README.md", None, home), None);
    }

    #[test]
    fn a_url_is_still_a_url_however_much_it_looks_like_a_path() {
        let snapshot = grid("https://x.test/a.rs");
        let project = project();
        let link = find(&rows(&snapshot), 0, 15, Some(&project), LinkRow::Viewport)
            .expect("the URL is a link");
        assert_eq!(link.target, url("https://x.test/a.rs"));
    }

    #[test]
    fn a_path_the_terminal_folded_is_one_link_from_either_row() {
        // Twenty-six cells into twenty columns: the emulator folds it.
        let snapshot = grid("app/src/workspace/view.rs:3");
        assert!(rows(&snapshot).wraps(0), "the emulator folded it");
        let project = project();
        let whole = LinkSpan {
            cells: vec![cells(0, 0, 20), cells(1, 0, 7)],
            target: LinkTarget::File(file("app/src/workspace/view.rs", Some(3), None)),
        };
        for (row, column) in [(0, 4), (1, 2)] {
            assert_eq!(
                find(
                    &rows(&snapshot),
                    row,
                    column,
                    Some(&project),
                    LinkRow::Viewport
                ),
                Some(whole.clone()),
                "from row {row}"
            );
        }
    }

    #[test]
    fn a_path_with_wide_characters_in_it_names_the_file_without_the_spacers() {
        let snapshot = grid("docs/\u{6771}\u{4eac}.md");
        let project = project();
        let link = find(&rows(&snapshot), 0, 6, Some(&project), LinkRow::Viewport)
            .expect("the path is a link");
        assert_eq!(
            link.target,
            LinkTarget::File(file("docs/\u{6771}\u{4eac}.md", None, None))
        );
        assert_eq!(link.cells, vec![cells(0, 0, 12)]);
    }

    #[test]
    fn a_path_is_not_underlined_until_its_file_is_found() {
        let link = PaneLink::new();
        let main = file("src/main.rs", Some(42), None);

        assert!(!link.set(Some(on(main.clone()))), "nothing to draw yet");
        assert_eq!(link.get(), None);
        assert!(link.wants_a_look());
        assert_eq!(link.take_look(), Some(main.path.clone()));
        assert!(!link.wants_a_look(), "the look is out");

        assert!(
            link.found(&main.path, Some(Opens::InEditor)),
            "the path under the pointer underlines when its file is found"
        );
        assert_eq!(link.get(), Some(on(main.clone())));

        // And from then on at once, without another look.
        assert!(link.set(None));
        assert!(link.set(Some(on(main))));
        assert!(!link.wants_a_look());
    }

    #[test]
    fn a_path_whose_file_is_not_there_is_never_a_link() {
        let link = PaneLink::new();
        let gone = file("and/or", None, None);

        link.set(Some(on(gone.clone())));
        let path = link.take_look().expect("a look is wanted");
        assert!(!link.found(&path, None), "nothing to draw");
        assert_eq!(link.get(), None);

        // Under the pointer again: not shown, and not looked for again while
        // the chord key is held.
        assert!(!link.set(Some(on(gone.clone()))));
        assert_eq!(link.get(), None);
        assert!(!link.wants_a_look());
        assert!(!link.press(gone), "a press on it is the selection's");
    }

    #[test]
    fn a_pane_has_one_look_out_at_a_time() {
        // A look into a mount whose server has gone takes a worker of the pool
        // that does not come back; a pointer run along a line of paths into
        // it must not take one each.
        let link = PaneLink::new();
        let first = file("dead/a.rs", None, None);
        let second = file("dead/b.rs", None, None);

        link.set(Some(on(first.clone())));
        assert_eq!(link.take_look(), Some(first.path.clone()));
        link.set(Some(on(second.clone())));
        assert!(!link.wants_a_look(), "another look while one is out");
        assert_eq!(link.take_look(), None);

        // The first comes home: the second is next.
        assert!(!link.found(&first.path, Some(Opens::InEditor)));
        assert_eq!(link.take_look(), Some(second.path));
    }

    #[test]
    fn letting_go_of_the_chord_forgets_what_was_found_but_not_what_is_out() {
        let link = PaneLink::new();
        let there = file("src/main.rs", None, None);
        let stuck = file("dead/a.rs", None, None);

        link.set(Some(on(there.clone())));
        let path = link.take_look().expect("a look is wanted");
        link.found(&path, Some(Opens::ByPlatform));
        // The pointer crosses a path on a mount whose server has gone, and
        // comes back.
        link.set(Some(on(stuck.clone())));
        assert_eq!(link.take_look(), Some(stuck.path.clone()));
        link.set(Some(on(there.clone())));
        assert!(link.is_some(), "found, and the chord is still held");

        assert!(link.let_go(), "the underline goes with the chord");
        assert_eq!(link.get(), None);

        // The chord goes down again. What was found is forgotten — the file
        // may have gone since — and the dead mount is not looked into again.
        link.set(Some(on(there.clone())));
        assert_eq!(link.get(), None, "shown from before the chord came up");
        link.set(Some(on(stuck.clone())));
        assert!(!link.wants_a_look(), "the dead mount was looked into again");

        // When it answers at last, the file is asked about again.
        link.found(&stuck.path, None);
        link.set(Some(on(there.clone())));
        assert_eq!(link.take_look(), Some(there.path));
    }

    #[test]
    fn a_press_takes_a_file_that_was_found_and_how_it_opens() {
        let link = PaneLink::new();
        let main = file("src/main.rs", Some(7), Some(2));

        link.set(Some(on(main.clone())));
        assert!(!link.press(main.clone()), "pressed before it was found");
        let path = link.take_look().expect("a look is wanted");
        link.found(&path, Some(Opens::InEditor));

        assert!(link.press(main.clone()));
        assert_eq!(link.take_pressed(), Some((main, Opens::InEditor)));
        assert_eq!(link.take_pressed(), None, "a press opens once");
    }

    #[test]
    fn a_program_opens_in_the_editor_or_not_at_all() {
        // The platform's handler runs a program, and a chord-click on a path
        // a program printed must not be a way to start what is at it.
        assert_eq!(Opens::of(Found::File, true), Some(Opens::InEditor));
        assert_eq!(Opens::of(Found::File, false), Some(Opens::ByPlatform));
        assert_eq!(Opens::of(Found::Folder, true), Some(Opens::ByPlatform));
        assert_eq!(Opens::of(Found::Folder, false), Some(Opens::ByPlatform));
        assert_eq!(Opens::of(Found::Program, true), Some(Opens::InEditor));
        assert_eq!(Opens::of(Found::Program, false), None);
        assert_eq!(Opens::of(Found::Nothing, true), None);
        assert_eq!(Opens::of(Found::Nothing, false), None);
    }

    /// A directory of the test's own, gone when it is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            static SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "crook-pane-link-{label}-{}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("a scratch directory");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn looking_for_a_path_tells_a_file_from_a_folder_from_nothing() {
        let scratch = Scratch::new("found");
        let notes = scratch.0.join("notes.md");
        std::fs::write(&notes, "# notes\n").expect("writable");

        assert_eq!(Found::at(&notes), Found::File);
        assert_eq!(Found::at(&scratch.0), Found::Folder);
        assert_eq!(Found::at(&scratch.0.join("missing.md")), Found::Nothing);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_marked_executable_is_a_program() {
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = Scratch::new("program");
        let script = scratch.0.join("release");
        std::fs::write(&script, "#!/bin/sh\n").expect("writable");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("the mode can be set");
        assert_eq!(Found::at(&script), Found::Program);

        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644))
            .expect("the mode can be set");
        assert_eq!(Found::at(&script), Found::File);
    }

    #[test]
    fn a_file_the_platform_runs_by_its_name_is_a_program() {
        // A `.jar` is run by `java -jar` from a double-click everywhere Java
        // is installed, marked executable or not.
        let scratch = Scratch::new("by-name");
        let tool = scratch.0.join("tool.jar");
        std::fs::write(&tool, "PK").expect("writable");
        assert_eq!(Found::at(&tool), Found::Program);
        let notes = scratch.0.join("notes.md");
        std::fs::write(&notes, "# notes\n").expect("writable");
        assert_eq!(Found::at(&notes), Found::File);
    }

    #[test]
    fn a_windows_program_is_a_program_on_every_platform() {
        // Wine's desktop entry runs these when `xdg-open` is handed one, and
        // `unzip` leaves a `setup.exe` without its exec bit.
        let scratch = Scratch::new("windows-program");
        let unmarked = |name: &str| {
            let path = scratch.0.join(name);
            std::fs::write(&path, "MZ").expect("writable");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                    .expect("the mode can be set");
            }
            path
        };
        for name in ["setup.exe", "INSTALL.MSI", "run.bat", "go.lnk", "site.url"] {
            assert_eq!(Found::at(&unmarked(name)), Found::Program, "{name}");
        }
        assert_eq!(Found::at(&unmarked("notes.txt")), Found::File);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_what_it_points_at() {
        // The handler opens what the link resolves to, so a harmless name on
        // a link to a program is a program.
        let scratch = Scratch::new("link");
        let tool = scratch.0.join("tool.jar");
        std::fs::write(&tool, "PK").expect("writable");
        let disguised = scratch.0.join("notes.md");
        std::os::unix::fs::symlink(&tool, &disguised).expect("a symlink");
        assert_eq!(Found::at(&disguised), Found::Program);

        let real = scratch.0.join("real.md");
        std::fs::write(&real, "# real\n").expect("writable");
        let plain = scratch.0.join("plain.md");
        std::os::unix::fs::symlink(&real, &plain).expect("a symlink");
        assert_eq!(Found::at(&plain), Found::File);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_application_is_a_program_however_it_is_reached() {
        // `open` hands an `.app` to Launch Services, which starts it — named
        // as it is, through a link with another name, or from inside itself.
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = Scratch::new("bundle");
        let bundle = scratch.0.join("Tool.app");
        std::fs::create_dir_all(bundle.join("Contents")).expect("a bundle");
        assert_eq!(Found::at(&bundle), Found::Program);
        assert_eq!(
            Found::at(&bundle.join("Contents").join("..")),
            Found::Program
        );
        let docs = scratch.0.join("docs");
        std::os::unix::fs::symlink(&bundle, &docs).expect("a symlink");
        assert_eq!(Found::at(&docs), Found::Program);
        assert_eq!(Found::at(&bundle.join("Contents")), Found::Folder);

        // Terminal runs a `.command` it is handed.
        let command = scratch.0.join("build.command");
        std::fs::write(&command, "make\n").expect("writable");
        std::fs::set_permissions(&command, std::fs::Permissions::from_mode(0o644))
            .expect("the mode can be set");
        assert_eq!(Found::at(&command), Found::Program);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn python_bytecode_and_a_folder_installer_are_programs_on_macos() {
        // Python Launcher runs a `.pyc` it is opened with, and `open` starts
        // Installer on a `.pkg` or `.mpkg` that is a folder.
        use std::os::unix::fs::PermissionsExt as _;

        let scratch = Scratch::new("macos-runs");
        for name in ["__main__.pyc", "legacy.pyo"] {
            let path = scratch.0.join(name);
            std::fs::write(&path, "").expect("writable");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                .expect("the mode can be set");
            assert_eq!(Found::at(&path), Found::Program, "{name}");
        }
        for name in ["Foo.mpkg", "Bar.pkg"] {
            let path = scratch.0.join(name);
            std::fs::create_dir_all(path.join("Contents")).expect("a bundle");
            assert_eq!(Found::at(&path), Found::Program, "{name}");
        }
        assert_eq!(Found::at(&scratch.0), Found::Folder);
    }

    #[cfg(windows)]
    #[test]
    fn what_windows_runs_by_its_name_is_a_program() {
        // Windows has no executable bit: `explorer` runs whatever the name
        // says is a program, a script Git for Windows or Python registered
        // included, in any case.
        let scratch = Scratch::new("windows");
        for name in ["tool.exe", "build.BAT", "release.sh", "test_x.py", "go.lnk"] {
            let path = scratch.0.join(name);
            std::fs::write(&path, "").expect("writable");
            assert_eq!(Found::at(&path), Found::Program, "{name}");
        }
        let notes = scratch.0.join("notes.md");
        std::fs::write(&notes, "# notes\n").expect("writable");
        assert_eq!(Found::at(&notes), Found::File);
    }

    #[test]
    fn letting_go_of_the_chord_key_forgets_what_was_found() {
        // What both surfaces do when a modifier comes up or the pointer moves
        // without the chord: the underline goes, and so does what was found,
        // so the file is asked about again the next time the key goes down.
        let link = PaneLink::new();
        let main = file("src/main.rs", None, None);

        assert!(!link.track(true, || Some(on(main.clone()))));
        let path = link.take_look().expect("a look is wanted");
        assert!(link.found(&path, Some(Opens::InEditor)));
        assert!(!link.track(true, || Some(on(main.clone()))));
        assert!(link.is_some());

        assert!(
            link.track(false, || panic!("looked for a link without the chord")),
            "the underline goes with the chord"
        );
        assert_eq!(link.get(), None);

        assert!(!link.track(true, || Some(on(main.clone()))));
        assert_eq!(link.get(), None, "shown from before the chord came up");
        assert_eq!(link.take_look(), Some(main.path), "not asked about again");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_url_escapes_what_a_url_cannot_carry() {
        assert_eq!(
            file_url(Path::new("/home/me/my project/100%.md")),
            "file:///home/me/my%20project/100%25.md"
        );
        assert_eq!(
            file_url(Path::new("/home/me/\u{6771}.md")),
            "file:///home/me/%E6%9D%B1.md"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_file_url_puts_the_drive_after_a_slash_of_its_own() {
        assert_eq!(
            file_url(Path::new(r"C:\Users\me\my project\a.rs")),
            "file:///C:/Users/me/my%20project/a.rs"
        );
    }
}
