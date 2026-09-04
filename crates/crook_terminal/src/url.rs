//! Finding the URL under a cell.
//!
//! A terminal has two ways to know that some text is a link. The first is
//! **OSC 8**, where the program says so — and that is not what this module is:
//! see the note at the end. The second, and the one people actually rely on,
//! is that a URL looks like a URL. A build tool prints `https://…`, a `git
//! push` prints the address of the branch it created, `cargo` prints a docs
//! link, and none of them marks any of it up.
//!
//! ## Why this is a scan and not a table
//!
//! Nothing here runs unless a pointer is over a cell. Walking the whole grid
//! every frame to build a list of links would be a table nobody reads: a
//! screen is a few hundred rows and the pointer is over exactly one of them.
//! So the shape is [`at`] — one row of text, one column, one answer — and the
//! cost is a scan of a single row on the frames a pointer moves.
//!
//! ## Columns, not bytes
//!
//! Every offset here is a **cell of the row**, because that is what the caller
//! has: a snapshot row and a harvested block row are both one `char` per cell.
//! A double-width character occupies its cell and a spacer, and the spacer is
//! a character of the row too, so counting characters and counting cells are
//! the same count — which is exactly why nothing here converts between them.
//!
//! ## What is left out, and why
//!
//! **OSC 8 hyperlinks.** `alacritty_terminal` tracks them per cell, and
//! carrying one to the renderer means putting it on [`SnapshotCell`] — which
//! is twelve bytes and [`Copy`] precisely so that a full screen is one flat
//! allocation the renderer walks in order — or threading a side table through
//! the harvest path as well, so that a link works the same on a finished block
//! as on the live grid. Doing it on the grid alone would be worse than not
//! doing it: a link that stops working the moment its command finishes is a
//! link nobody can trust. So OSC 8 is not read, and a program that emits one
//! gets the same treatment as one that does not — the visible text is scanned,
//! and a link whose text *is* its URL, which is most of them, works anyway.
//!
//! [`SnapshotCell`]: crate::SnapshotCell

/// A URL found in a row, and the cells it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    /// The URL itself, exactly as it appeared.
    pub uri: String,
    /// The first cell of the row it occupies.
    pub start: usize,
    /// How many cells it covers.
    pub len: usize,
}

impl Url {
    /// One past the last cell it occupies.
    pub fn end(&self) -> usize {
        self.start + self.len
    }

    /// Whether this URL covers `column`.
    pub fn contains(&self, column: usize) -> bool {
        (self.start..self.end()).contains(&column)
    }
}

/// The schemes a bare run of text is recognised as a link for.
///
/// Deliberately short. Every one of them is something a terminal prints and
/// somebody wants to click; nothing here guesses at `www.` or at a bare
/// hostname, because `example.com` in a sentence is a word and underlining it
/// would make ordinary output twitch under the pointer.
const SCHEMES: [&str; 8] = [
    "https://", "http://", "ftps://", "ftp://", "file://", "ssh://", "git://", "mailto:",
];

/// Characters that end a URL wherever they appear.
///
/// Whitespace and the quotes and brackets a URL is routinely printed inside.
/// A closing bracket is *not* here: it is legal inside a URL — Wikipedia's are
/// full of them — and is handled by the balancing rule in [`trim_trailing`]
/// instead.
fn terminates(character: char) -> bool {
    character.is_whitespace()
        || character.is_control()
        || matches!(character, '"' | '\'' | '<' | '>' | '`' | '|' | '\\' | '^')
}

/// The URL covering `column` of `text`, or `None` when that cell is not part
/// of one.
///
/// `text` is one row, one `char` per cell.
pub fn at(text: &str, column: usize) -> Option<Url> {
    let cells: Vec<char> = text.chars().collect();
    if cells.get(column).is_none_or(|cell| terminates(*cell)) {
        return None;
    }

    // The run of non-terminating cells the column is inside. Runs are maximal,
    // so there is exactly one, and every URL on the row is inside one of them.
    let mut run_start = column;
    while run_start > 0 && !terminates(cells[run_start - 1]) {
        run_start -= 1;
    }
    let mut run_end = column;
    while run_end < cells.len() && !terminates(cells[run_end]) {
        run_end += 1;
    }

    // The last scheme that begins at or before the column. A URL does not have
    // to start the run it is in: `(https://example.com)` and `see:https://x`
    // are both things programs print, and in both the link begins partway
    // along.
    let start = (run_start..=column)
        .rev()
        .find(|at| scheme_at(&cells, *at).is_some())?;
    let scheme = scheme_at(&cells, start)?;

    let end = trim_trailing(&cells, start, run_end);
    // The scheme alone is not a link: `https://` with nothing after it points
    // at nothing, and underlining it would be underlining punctuation.
    if end <= start + scheme || !(start..end).contains(&column) {
        return None;
    }

    Some(Url {
        uri: cells[start..end].iter().collect(),
        start,
        len: end - start,
    })
}

/// How long the scheme at `start` is, or `None` when there is not one.
fn scheme_at(cells: &[char], start: usize) -> Option<usize> {
    SCHEMES.iter().find_map(|scheme| {
        let length = scheme.chars().count();
        let matches = cells
            .get(start..start + length)?
            .iter()
            .zip(scheme.chars())
            .all(|(cell, expected)| cell.eq_ignore_ascii_case(&expected));
        matches.then_some(length)
    })
}

/// Pulls the end of a run back off the punctuation that ended the sentence
/// rather than the URL.
///
/// Two rules, and both are what every terminal does. A trailing `.`, `,`, `;`,
/// `:`, `!` or `?` is sentence punctuation — no URL a person wants to click
/// ends in one. And a trailing `)`, `]` or `}` belongs to the URL only if
/// something inside the run opened it: `(see https://example.com/a)` ends a
/// bracket that the URL never started, while `https://en.wikipedia.org/wiki/A_(b)`
/// ends one it did.
fn trim_trailing(cells: &[char], start: usize, mut end: usize) -> usize {
    while end > start {
        let last = cells[end - 1];
        let closing = match last {
            '.' | ',' | ';' | ':' | '!' | '?' => {
                end -= 1;
                continue;
            }
            ')' => '(',
            ']' => '[',
            '}' => '{',
            _ => break,
        };

        let opened = cells[start..end - 1]
            .iter()
            .filter(|c| **c == closing)
            .count();
        let closed = cells[start..end - 1].iter().filter(|c| **c == last).count();
        if opened > closed {
            // The run opened it, so it is part of the URL.
            break;
        }
        end -= 1;
    }
    end
}

#[cfg(test)]
#[path = "url_tests.rs"]
mod tests;
