//! Finding the path under a cell: `src/main.rs:42:5`, `./README.md`,
//! `/etc/hosts`, `~/notes.md`.
//!
//! The most common thing an agent or a compiler prints that somebody wants to
//! open is not a URL. It is a file in the project — `--> src/main.rs:42:5`,
//! `Update(app/src/view.rs)`, `tests.rs:31176` — and the line in it. This is
//! the scan that recognises one, beside [`crate::url`] and shaped exactly like
//! it: one row of text, one column, one answer, and nothing run unless a
//! pointer is over a cell.
//!
//! ## What this does not decide
//!
//! Whether the file is there. A word that looks like a path is only a guess
//! until somebody has looked, and looking is a `stat` — which on a network
//! mount whose server has gone blocks for as long as the mount does. That is
//! not something a scan run on a pointer move may do, so this answers with the
//! text it found and its place in the row, and the caller resolves it against
//! the directory the command ran in and looks for it somewhere a blocked
//! `stat` costs nothing. The bar for being *found* here is low on purpose:
//! whatever this lets through that is not a file is turned away by that look,
//! and costs one `stat` off the thread that draws.
//!
//! ## The forms
//!
//! A path is a run of the row with a separator in it, or whose last part has
//! an extension: `src/main.rs`, `./x`, `/etc/hosts`, `~/notes.md`,
//! `Cargo.toml`. A bare word — `Makefile`, `src` — is not one, because every
//! word on a screen would then be a `stat`, and `make` is a word long before
//! it is a file. After it may come the location the way the programs that
//! print one spell it:
//!
//! * `path:line` and `path:line:column` — rustc, gcc, clang, go, grep -n,
//!   pytest, Node's stack frames;
//! * `path(line)` and `path(line,column)` — MSVC, tsc and dotnet.
//!
//! Columns are cells here, exactly as in [`crate::url`], and for the same
//! reason: the caller has a row that is one `char` per cell.

/// A path found in a row, the location printed after it, and the cells both
/// cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintedPath {
    /// The path as it appeared, without the location after it.
    pub path: String,
    /// The line it names, if it names one.
    pub line: Option<u32>,
    /// The column on that line, if it names one.
    pub column: Option<u32>,
    /// The first cell of the row it occupies.
    pub start: usize,
    /// How many cells it covers, location included — what is underlined.
    pub len: usize,
}

impl PrintedPath {
    /// One past the last cell it occupies.
    pub fn end(&self) -> usize {
        self.start + self.len
    }
}

/// Characters that end a path wherever they appear.
///
/// Everything that ends a URL but the backslash, which is how Windows
/// separates one — cargo there prints `--> src\main.rs:2:5`. And the
/// brackets, which a URL keeps and balances but a path is printed *inside*:
/// `Update(app/src/view.rs)`, `at main (/home/me/app.js:12:5)`. A file named
/// with a bracket in it is rarer than either by far, and usually has a space
/// in it as well. The comma, semicolon and equals sign too, which separate a
/// list of paths and a `key=path` far more often than they name a file.
fn terminates(character: char) -> bool {
    (character != '\\' && crate::url::terminates(character))
        || matches!(
            character,
            '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '='
        )
}

/// The path covering `column` of `text`, or `None` when that cell is not part
/// of one.
///
/// `text` is one row, one `char` per cell.
pub fn at(text: &str, column: usize) -> Option<PrintedPath> {
    let cells: Vec<char> = text.chars().collect();
    // A pointer on the `(42,5)` after a path is on that path: the bracket
    // ends the run, so the scan starts from the cell before it. Failing that,
    // from the pointer's own cell — the digits after a bracket are as often
    // the start of a name inside it: `Write(2024-notes.md)`.
    before_bracketed_location(&cells, column)
        .and_then(|from| scan(&cells, from, column))
        .or_else(|| scan(&cells, column, column))
}

/// The path in the run of `cells` around `from`, if it covers `column`.
fn scan(cells: &[char], from: usize, column: usize) -> Option<PrintedPath> {
    if cells.get(from).is_none_or(|cell| terminates(*cell)) {
        return None;
    }

    let mut start = from;
    while start > 0 && !terminates(cells[start - 1]) {
        start -= 1;
    }
    let mut run_end = from;
    while run_end < cells.len() && !terminates(cells[run_end]) {
        run_end += 1;
    }

    // Whatever has a scheme is a URL or nothing, and the URL scan has had it
    // first. `sftp://host/a.tar` is not a file in the directory the command
    // ran in.
    if cells[start..run_end]
        .windows(3)
        .any(|three| three == [':', '/', '/'])
    {
        return None;
    }

    let end = trim_trailing(cells, start, run_end);
    let (path_end, location, end) = location(cells, start, end, run_end);
    let path: String = cells[start..path_end].iter().collect();
    if !looks_like_a_path(&path) || !(start..end).contains(&column) {
        return None;
    }
    Some(PrintedPath {
        path,
        line: location.map(|(line, _)| line),
        column: location.and_then(|(_, column)| column),
        start,
        len: end - start,
    })
}

/// The last cell of the path a bracketed location belongs to, when `column`
/// is inside one: `(`, digits, a comma, `)`.
///
/// Only a whole one, closed where its digits end. `Read(2026-10-14-notes.md)`
/// opens a bracket and starts with digits too, and a pointer on them is on
/// the name they begin, not on a location of `Read`'s.
fn before_bracketed_location(cells: &[char], column: usize) -> Option<usize> {
    let mut at = column;
    if *cells.get(at)? == ')' {
        at = at.checked_sub(1)?;
    }
    while at > 0 && (cells[at].is_ascii_digit() || cells[at] == ',') {
        at -= 1;
    }
    if cells[at] != '(' || at == 0 || terminates(cells[at - 1]) {
        return None;
    }
    let (_, after) = bracketed(cells, at + 1)?;
    (column < after).then(|| at - 1)
}

/// Pulls the end of a run back off the punctuation that ended the sentence
/// rather than the path — `see src/main.rs.`, `is it src/main.rs?` — and off
/// the colon a panic and go print after a location.
fn trim_trailing(cells: &[char], start: usize, mut end: usize) -> usize {
    while end > start && matches!(cells[end - 1], '.' | ':' | '!' | '?') {
        end -= 1;
    }
    end
}

/// A line, and the column on it if one was printed.
type Location = (u32, Option<u32>);

/// Splits the location off a path: where the path ends, the location, and
/// where the whole link ends.
///
/// The first `:line` that ends the run, or that another colon follows, is the
/// location: `src/a.rs:42:5`, and grep's `src/a.rs:42:use std;`, whose text
/// after the line is not part of the link. A colon and digits that carry on
/// as a path — `/tmp/12:30/log.txt` — are part of the path, and a Windows
/// drive's colon is passed over. Failing those, a bracketed location straight
/// after the run: `src/app.ts(42,5)`.
fn location(
    cells: &[char],
    start: usize,
    end: usize,
    run_end: usize,
) -> (usize, Option<Location>, usize) {
    let drive = cells.len() > start + 2
        && cells[start].is_ascii_alphabetic()
        && cells[start + 1] == ':'
        && matches!(cells[start + 2], '\\' | '/');
    let from = if drive { start + 2 } else { start };

    for colon in from..end {
        if cells[colon] != ':' {
            continue;
        }
        let Some((line, after_line)) = number(cells, colon + 1, end) else {
            continue;
        };
        if after_line < end && cells[after_line] != ':' {
            continue;
        }
        let column = number(cells, after_line + 1, end)
            .filter(|(_, after)| *after == end || cells[*after] == ':');
        return match column {
            Some((column, after_column)) => (colon, Some((line, Some(column))), after_column),
            None => (colon, Some((line, None)), after_line),
        };
    }

    // Only straight after the run: a path whose sentence punctuation was
    // trimmed off is not followed by a location of its own.
    if end == run_end
        && cells.get(run_end) == Some(&'(')
        && let Some((location, after)) = bracketed(cells, run_end + 1)
    {
        return (end, Some(location), after);
    }
    (end, None, end)
}

/// The `line)` or `line,column)` after an opening bracket at `from - 1`, and
/// the cell after the closing one.
fn bracketed(cells: &[char], from: usize) -> Option<(Location, usize)> {
    let (line, after_line) = number(cells, from, cells.len())?;
    match cells.get(after_line)? {
        ')' => Some(((line, None), after_line + 1)),
        ',' => {
            let (column, after_column) = number(cells, after_line + 1, cells.len())?;
            (cells.get(after_column) == Some(&')'))
                .then_some(((line, Some(column)), after_column + 1))
        }
        _ => None,
    }
}

/// The number written at `from`, and the cell after it: digits before `end`,
/// and no more of them than fit a line number.
fn number(cells: &[char], from: usize, end: usize) -> Option<(u32, usize)> {
    let mut after = from;
    while after < end && cells[after].is_ascii_digit() {
        after += 1;
    }
    if after == from {
        return None;
    }
    let digits: String = cells[from..after].iter().collect();
    Some((digits.parse().ok()?, after))
}

/// Whether a run is a path rather than a word: it has a separator in it, or
/// its last part has an extension — and it has a letter or a digit in it
/// somewhere, so that a `/` or a `~/` printed as punctuation is not a link to
/// the root of the disk.
fn looks_like_a_path(path: &str) -> bool {
    if !path.chars().any(char::is_alphanumeric) {
        return false;
    }
    if path.contains(['/', '\\']) {
        return true;
    }
    // An extension is letters and digits, at least one of them a letter, after
    // a name: `README.md` and `tests.rs`, and not `v0.1.24`, `127.0.0.1` or
    // `crook::process::command`.
    path.rsplit_once('.').is_some_and(|(name, extension)| {
        !name.is_empty()
            && !extension.is_empty()
            && extension.chars().all(char::is_alphanumeric)
            && extension
                .chars()
                .any(|character| character.is_ascii_alphabetic())
    })
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
