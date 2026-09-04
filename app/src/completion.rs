//! Asking the shell what a half-typed line could become, and doing something
//! with the answer.
//!
//! # Why the shell and not Crook
//!
//! Because the answer is the shell's. `PATH` hashing, `complete` definitions,
//! aliases, functions, the glob rules a word is expanded under: every one of
//! those is a fact about the shell in the pane, and a completer written here
//! would be a second, worse copy of all of them that disagreed with the shell
//! the moment somebody defined an alias.
//!
//! # The channel
//!
//! OSC 133 cannot carry this. It is an announcement — the shell says where a
//! prompt began and how a command ended — and completion is a *question*. So
//! there is a second channel, and it is two halves:
//!
//! * **The question is a file.** Crook writes the request into the session's
//!   own scratch directory and sends a key press the integration snippet has
//!   bound. A command line can hold a semicolon, a newline and bytes that are
//!   not UTF-8, and escaping every one of them past a shell and past an OSC
//!   parser is a protocol nobody should have to debug.
//! * **The answer is a file too**, and the escape sequence that says it is
//!   ready carries nothing but the request's number — `ESC ] 6339 ; n BEL`.
//!   The number is what makes a stale answer discardable: pressing Tab twice
//!   quickly leaves two requests outstanding, and only the second is about the
//!   line on screen.
//!
//! # What each shell can actually answer
//!
//! Not the same thing, and the difference is the shells'.
//!
//! * **fish** answers with `complete -C`, which is the real question: every
//!   `complete` definition fish has, for every command.
//! * **bash** answers with `compgen`, which is the shell's own command, file
//!   and variable completion. The `_git`, `_docker` and `_ssh` functions that
//!   `bash-completion` installs are *not* reached: driving one means setting
//!   `COMP_WORDS`, `COMP_CWORD`, `COMP_LINE` and `COMP_POINT` by hand and
//!   calling a function whose name has to be dug out of `complete -p`, and
//!   getting any of it wrong runs somebody's completion script against a line
//!   it was never given.
//! * **zsh** answers with globs and its own hashes, and is the weakest of the
//!   three. Its completion system runs inside a ZLE widget and reports through
//!   `compstate` rather than returning anything, so there is nothing to ask
//!   from outside one.
//!
//! All three are honest about being the shell's own answer for the common
//! cases — a command, a path, a variable — which is what Tab is for most of
//! the time.

use std::fs;
use std::path::Path;

/// The most candidates a request will carry back.
///
/// A `compgen -c` on a full `PATH` is thousands of entries, and nobody reads a
/// thousand of anything. What the caller does with a list this long is show
/// that it is too long, which needs a bounded list rather than all of it.
pub const MAX_CANDIDATES: usize = 256;

/// What the shell said a line could become.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completions {
    /// The candidates, in the order the shell gave them.
    pub candidates: Vec<String>,
    /// Whether the shell offered more than [`MAX_CANDIDATES`] and the rest
    /// were dropped.
    pub truncated: bool,
}

impl Completions {
    /// Whether the shell had nothing to offer.
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// The longest prefix every candidate shares.
    ///
    /// What Tab inserts when the answer is ambiguous, which is what every
    /// shell does: type as much as is certain and stop. Measured in
    /// characters rather than bytes, so a prefix never cuts one in half.
    pub fn common_prefix(&self) -> String {
        let mut candidates = self.candidates.iter();
        let Some(first) = candidates.next() else {
            return String::new();
        };

        let mut prefix: Vec<char> = first.chars().collect();
        for candidate in candidates {
            let shared = candidate
                .chars()
                .zip(prefix.iter())
                .take_while(|(one, other)| one == *other)
                .count();
            prefix.truncate(shared);
            if prefix.is_empty() {
                break;
            }
        }
        prefix.into_iter().collect()
    }

    /// The candidates that still answer `word`, in the order the shell gave
    /// them and without repeats.
    ///
    /// **Prefix, and nothing cleverer.** A fuzzy match is worth having where
    /// there is a list to show what it matched; offered one at a time in the
    /// line itself — see
    /// [`TextInput::suggestion`](crate::text_input::TextInput::suggestion) —
    /// a candidate that shares no start with what was typed reads as the field
    /// having invented something. Case is ignored, because a person typing
    /// `car` for `Cargo.toml` has typed the start of it.
    ///
    /// Repeats are dropped because `compgen -c` lists a command once per
    /// directory of `PATH` that holds it, and stepping through the same name
    /// four times is four presses that appear to do nothing.
    pub fn matching(&self, word: &str) -> Vec<&str> {
        let mut kept: Vec<&str> = Vec::new();
        for candidate in &self.candidates {
            if starts_with_ignoring_case(candidate, word) && !kept.contains(&candidate.as_str()) {
                kept.push(candidate);
            }
        }
        kept
    }

    /// What to insert in place of `word`, or `None` when there is nothing to
    /// add.
    ///
    /// The rule every shell's Tab follows: one candidate is inserted whole,
    /// several insert as much as they agree on, and an answer that agrees on
    /// no more than what is already typed inserts nothing — at which point the
    /// candidates themselves are what a person needs to see.
    pub fn insertion(&self, word: &str) -> Option<String> {
        let prefix = self.common_prefix();
        (prefix.len() > word.len() && prefix.starts_with(word)).then_some(prefix)
    }
}

/// Whether `word` is the start of `candidate` but for case.
fn starts_with_ignoring_case(candidate: &str, word: &str) -> bool {
    let mut left = candidate.chars();
    word.chars()
        .all(|typed| left.next().is_some_and(|c| c.eq_ignore_ascii_case(&typed)))
}

/// Reads an answer the shell has written, or `None` when there is nothing
/// usable there.
///
/// A missing file is the ordinary case for a shell with no integration: it
/// never bound the key, so nothing was written. Everything else — an
/// unreadable file, one holding bytes that are not UTF-8 — is one line in the
/// log and no completions, because a completion nobody can read is not one.
pub fn read_answer(path: &Path) -> Option<Completions> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::debug!("could not read {}: {error}", path.display());
            return None;
        }
    };

    Some(parse_answer(&text))
}

/// The candidates in an answer's text: one per line, blanks dropped.
///
/// Blanks are dropped rather than kept because a shell that found nothing
/// prints one empty line — `printf '%s\n' ${empty[@]}` in bash, and the same
/// shape in the other two — and an empty candidate would insert nothing and
/// list as a blank row.
pub fn parse_answer(text: &str) -> Completions {
    let mut candidates: Vec<String> = text
        .lines()
        .map(|line| line.trim_end_matches(char::is_whitespace))
        .filter(|line| !line.is_empty())
        .take(MAX_CANDIDATES + 1)
        .map(str::to_owned)
        .collect();

    let truncated = candidates.len() > MAX_CANDIDATES;
    candidates.truncate(MAX_CANDIDATES);
    Completions {
        candidates,
        truncated,
    }
}

/// The text of a request: its number, then the line up to the caret.
///
/// The *prefix* rather than the whole line and a cursor offset, because that
/// is the question every shell's completion actually answers — `complete -C`
/// in fish and `compgen` in bash both complete the end of what they are given.
pub fn request_text(serial: u64, line_to_caret: &str) -> String {
    format!("{serial}\n{line_to_caret}")
}

/// The word a completion would replace: everything after the last space.
///
/// The same approximation the snippets make, and it has to be: the caller
/// splices the answer back into the line, and if the two disagreed about where
/// the word began the result would be a line neither of them meant. Quoting is
/// not parsed — a path with a space in it completes as the fragment after the
/// space — which offers too *few* completions rather than the wrong ones.
pub fn word_at_end(line_to_caret: &str) -> &str {
    match line_to_caret.rfind([' ', '\t', '\n']) {
        Some(at) => &line_to_caret[at + 1..],
        None => line_to_caret,
    }
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;
