//! The keybindings file as *text*, so that a binding changed from the
//! settings page does not rewrite the rest of it.
//!
//! # Why this is not `Vec<Value>`
//!
//! A person's keybindings file is a file a person wrote. It has their comments
//! in it — VSCode's own opens as four lines of them — their blank lines, their
//! indentation, and quite possibly a line this build does not understand
//! because the plugin it names is not installed. Reading the file into
//! [`Value`]s, changing one and writing them back would silently take every
//! one of those away, and it would take them away as the side effect of
//! clicking a button in a settings page: the worst moment to lose something is
//! the moment you were doing something else.
//!
//! So an edit is made to the **text**. The file is scanned once for the array
//! and for the span of each entry in it; adding a binding inserts a line after
//! the last entry, and taking one away cuts exactly that entry's span out.
//! Everything between the entries is never looked at and therefore never
//! touched.
//!
//! # What it does not do
//!
//! Reformat, sort, deduplicate, or repair. A file that is not a JSON array —
//! an object, half a line, a paste that lost its closing bracket — is a file
//! this refuses to edit rather than one it replaces: see [`Document::append`],
//! which is where every edit starts. The page reports that the file could not
//! be written; the file itself is left exactly as its owner left it.

use std::ops::Range;

use serde_json::Value;

use super::strip_comments;

/// The indentation an entry is written at when the file has none to copy.
///
/// VSCode's, in VSCode's file.
const INDENT: &str = "    ";

/// A keybindings file, as text, with the entries in it located.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    /// What the file says, and what will be written back.
    text: String,
}

/// One entry of the array: where it is, and what it says.
///
/// The value is `None` for a span that is not an entry at all — a comment
/// after the last one, a fragment that does not parse. Those are located so
/// that an edit does not cut through them, and are never removed: a line this
/// build cannot read is still a line somebody wrote.
type Entry = (Range<usize>, Option<Value>);

impl Document {
    /// The document a file holds, or an empty one for a file that is not there.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// What would be written to the file.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the file has nothing in it to write.
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Whether the file says nothing but a person's own prose.
    ///
    /// The state VSCode's file is created in — its own is four lines of
    /// comments and an empty array, and a file that lost the array is the same
    /// thing with less of it. An array may be *added* to one of these, which a
    /// file with a value in it may not be: there is nothing to disagree with.
    fn is_all_comments(&self) -> bool {
        super::strip_comments(&self.text).trim().is_empty()
    }

    /// Adds an entry to the end of the array, written as it stands.
    ///
    /// `entry` is a whole JSON object — `{ "key": "ctrl+t", "command": "x" }` —
    /// because what the entry should *look* like is the caller's business and
    /// serialising a [`Value`] would decide it here, in alphabetical order.
    ///
    /// The end rather than the beginning, and that is the whole of why this
    /// works: the last rule that matches a chord is the one in force, so an
    /// entry appended to the file beats every entry above it without anything
    /// having to be taken out first.
    ///
    /// `false` when the file is not a JSON array and is not empty, which is
    /// the one case where an edit would have to guess what a person meant.
    pub fn append(&mut self, entry: &str) -> bool {
        let Some(array) = scan(&self.text) else {
            // A file with no array is one an edit may write the array into,
            // but only when there is nothing there to disagree with: an empty
            // file, or one that is nothing but somebody's comments. Their
            // prose is kept and the array goes under it.
            if !self.is_all_comments() {
                return false;
            }
            let prose = match self.text.trim_end() {
                "" => String::new(),
                text => format!("{text}\n"),
            };
            self.text = format!("{prose}[\n{INDENT}{entry}\n]\n");
            return true;
        };

        match array.entries.last() {
            // After the last entry, at the indentation that entry is written
            // at: a file indented with two spaces, or with tabs, stays that
            // way.
            Some((span, _)) => {
                let indent = self.indentation(span.start);
                self.text
                    .insert_str(span.end, &format!(",\n{indent}{entry}"));
            }
            // An array with nothing in it may be written `[]`, and an entry
            // pushed straight into that would leave the file on one line.
            None => self
                .text
                .replace_range(array.body.clone(), &format!("\n{INDENT}{entry}\n")),
        }
        true
    }

    /// Cuts out every entry `unwanted` accepts, and says how many went.
    ///
    /// Backwards, so that each span is still the span of the entry it was
    /// measured on: cutting the first entry would move every entry after it.
    pub fn remove(&mut self, unwanted: impl Fn(&Value) -> bool) -> usize {
        let Some(array) = scan(&self.text) else {
            return 0;
        };

        let mut removed = 0;
        for (span, value) in array.entries.into_iter().rev() {
            if value.as_ref().is_some_and(&unwanted) {
                self.cut(span);
                removed += 1;
            }
        }
        removed
    }

    /// Every entry of the array, in the order they are written.
    pub fn entries(&self) -> Vec<Value> {
        scan(&self.text)
            .map(|array| array.entries.into_iter().filter_map(|(_, it)| it).collect())
            .unwrap_or_default()
    }

    /// The whitespace at the start of the line `at` is on.
    fn indentation(&self, at: usize) -> &str {
        let line = self.text[..at].rfind('\n').map(|end| end + 1).unwrap_or(0);
        let indentation = &self.text[line..at];
        match indentation.trim().is_empty() {
            true => indentation,
            // Two entries on one line: the second has no indentation of its
            // own to copy, and the file's shape is nobody's to guess at.
            false => INDENT,
        }
    }

    /// Takes one entry out, along with the comma and the line it leaves behind.
    ///
    /// An entry removed without its comma is a file that no longer parses, and
    /// an entry removed without its line is a hole in the middle of the file
    /// that grows by one blank line every time somebody changes a binding.
    fn cut(&mut self, span: Range<usize>) {
        let mut start = span.start;
        let mut end = span.end;

        let rest = &self.text[end..];
        let next = rest.find(|character: char| !character.is_whitespace());
        match next {
            // The comma after it, and nothing else on that line.
            Some(at) if rest[at..].starts_with(',') => end += at + 1,
            // Nothing follows: this is the last entry, so the comma that has
            // to go is the one *before* it, which is what stops the file
            // ending in `, ]`.
            _ => {
                let head = self.text[..start].trim_end();
                if head.ends_with(',') {
                    start = head.len() - 1;
                }
            }
        }

        // The line it was on, where it had one to itself.
        let line = self.text[..start]
            .rfind('\n')
            .map(|end| end + 1)
            .unwrap_or(0);
        if self.text[line..start].trim().is_empty() {
            start = line;
            for ending in ["\r\n", "\n"] {
                if self.text[end..].starts_with(ending) {
                    end += ending.len();
                    break;
                }
            }
        }

        self.text.replace_range(start..end, "");
    }
}

/// Where the array is, and where each of its entries is.
struct Array {
    /// Everything between the brackets.
    body: Range<usize>,
    /// One span per entry, trimmed of the whitespace around it, with what it
    /// parses as.
    entries: Vec<Entry>,
}

/// Finds the array and the entries in it, or `None` for a file that has none.
///
/// One pass, and it is the same walk [`strip_comments`](super::strip_comments)
/// makes: a `[` inside a string is not an array and a `,` inside a comment
/// does not end an entry, so both have to be tracked to find either.
///
/// A comment is nobody's: it is skipped before an entry can start at it and it
/// never moves the end of the entry above it, so a comment between two entries
/// stays between them whichever of them is taken away — and a comma inserted
/// after the last entry cannot land on the end of a `//` line, where it would
/// be a comma nothing outside the comment can see.
fn scan(text: &str) -> Option<Array> {
    let mut characters = text.char_indices().peekable();
    let mut open = None;
    let mut nesting = 0usize;
    let mut entries: Vec<Entry> = Vec::new();
    let mut entry: Option<usize> = None;
    let mut code_ends_at = 0;

    while let Some((at, character)) = characters.next() {
        if character == '/' && matches!(characters.peek(), Some((_, '/'))) {
            for (_, character) in characters.by_ref() {
                if character == '\n' {
                    break;
                }
            }
            continue;
        }
        if character == '/' && matches!(characters.peek(), Some((_, '*'))) {
            characters.next();
            let mut star = false;
            for (_, character) in characters.by_ref() {
                if star && character == '/' {
                    break;
                }
                star = character == '*';
            }
            continue;
        }

        // A string is skipped whole; an escape inside it cannot end it.
        let ends_at = match character {
            '"' => {
                let mut escaped = false;
                let mut ends_at = text.len();
                for (at, character) in characters.by_ref() {
                    match character {
                        _ if escaped => escaped = false,
                        '\\' => escaped = true,
                        '"' => {
                            ends_at = at + 1;
                            break;
                        }
                        _ => {}
                    }
                }
                ends_at
            }
            character => at + character.len_utf8(),
        };

        let Some(start) = open else {
            // Before the array: the first bracket opens it, and anything else
            // — an object, a number, half a line — means there is no array
            // here to edit.
            match character {
                '[' => open = Some(at),
                character if character.is_whitespace() => {}
                _ => return None,
            }
            continue;
        };

        // Inside the array: what is tracked is only whether the next comma or
        // bracket is the array's own or belongs to something nested in it.
        match character {
            '{' | '[' => nesting += 1,
            '}' | ']' if nesting > 0 => nesting -= 1,
            ',' | ']' if nesting == 0 => {
                if let Some(from) = entry.take() {
                    entries.push(measure(text, from..code_ends_at));
                }
                if character == ']' {
                    return Some(Array {
                        body: start + 1..at,
                        entries,
                    });
                }
                continue;
            }
            _ => {}
        }

        if !character.is_whitespace() {
            code_ends_at = ends_at;
            // The entry starts at the first thing in it that is not
            // whitespace and is not a comment.
            if entry.is_none() {
                entry = Some(at);
            }
        }
    }

    // A bracket that is never closed: the file is half written, and half a
    // file is not one an edit may guess the rest of.
    None
}

/// One entry's span, and what its text parses as.
fn measure(text: &str, span: Range<usize>) -> Entry {
    let value = serde_json::from_str(&strip_comments(&text[span.clone()])).ok();
    (span, value)
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
