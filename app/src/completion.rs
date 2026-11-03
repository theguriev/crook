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
//! # Descriptions
//!
//! A line of an answer is a candidate, and a tab in it starts what the shell
//! says about that candidate: fish describes nearly everything, bash what a
//! cobra-generated completion describes (gh, kubectl), zsh whatever its
//! completion system would list beside a match.
//!
//! # What each shell can actually answer
//!
//! Not the same thing, and the difference is the shells'.
//!
//! * **fish** answers with `complete -C`, which is the real question: every
//!   `complete` definition fish has, for every command.
//! * **bash** answers with the command's own completion when it has one —
//!   the `_git`, `_ssh` and `_systemctl` functions `bash-completion` installs
//!   — called the way readline would call it, and with `compgen` for commands,
//!   files and variables otherwise. See `__crook_programmable` in the snippet
//!   for how the line is split, which is the part that has to be readline's.
//! * **zsh** answers with its own completion system when `compinit` has
//!   loaded it. That system runs only inside a completion widget and hands
//!   what it finds to `compadd`; the snippet's handler is a widget already,
//!   so it runs a completion widget of its own over the line and stands in
//!   for `compadd` to write the candidates down — see `__crook_compsys` in
//!   the snippet. Without `compinit`, globs and zsh's own hashes.
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
    /// What the shell said about each candidate, at the same index, or an
    /// empty string where it said nothing.
    ///
    /// fish describes nearly everything; bash only what a cobra-generated
    /// completion describes (gh, kubectl, docker); zsh what it would list. Shorter
    /// than [`Self::candidates`], or empty, means the rest have none.
    pub descriptions: Vec<String>,
    /// Whether the shell offered more than [`MAX_CANDIDATES`] and the rest
    /// were dropped.
    pub truncated: bool,
    /// Whether the order the candidates came in is itself a ranking — the
    /// history list's, best fit for the pane first — that a scattered match
    /// should only coarsely outrank. See [`Self::ranked`].
    pub ordered: bool,
}

impl Completions {
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
        let mut seen = std::collections::HashSet::new();
        self.candidates
            .iter()
            .map(String::as_str)
            .filter(|&candidate| {
                starts_with_ignoring_case(candidate, word) && seen.insert(candidate)
            })
            .collect()
    }

    /// The candidates the menu shows for `word`: every one [`Self::matching`]
    /// returns, in the shell's order, and after them the ones that hold the
    /// letters of `word` in order but not at their start, best first.
    ///
    /// **The second half is what the list is for.** One candidate offered in
    /// the line must share its start with what was typed, or the field reads as
    /// having invented it; a list shows *which* letters matched, so `fcp` can
    /// find `fix/card-pip-fallback` among forty branches without a person
    /// having to type the start of it. Prefix matches still come first, in the
    /// order the shell put them in, because a word that is the start of a name
    /// is the strongest thing a person can have typed.
    ///
    /// Each match says which of its characters matched, by index, for the menu
    /// to draw them in the accent.
    pub fn ranked(&self, word: &str) -> Vec<Match<'_>> {
        // A name the shell gave more than once is listed once, where it first
        // came, with the first thing said about it in any of its places: zsh
        // offers `start` bare from one completer and described from another.
        let mut described = std::collections::HashMap::new();
        for (candidate, description) in self.candidates.iter().zip(&self.descriptions) {
            if !description.is_empty() {
                described
                    .entry(candidate.as_str())
                    .or_insert(description.as_str());
            }
        }
        let mut seen = std::collections::HashSet::new();
        let mut prefixed = Vec::new();
        let mut scattered = Vec::new();
        for candidate in &self.candidates {
            if !seen.insert(candidate.as_str()) {
                continue;
            }
            let description = described.get(candidate.as_str()).copied();
            if starts_with_ignoring_case(candidate, word) {
                prefixed.push(Match {
                    candidate,
                    description,
                    matched: (0..word.chars().count()).collect(),
                });
            } else if let Some((score, matched)) = scattered_match(candidate, word) {
                scattered.push((
                    score,
                    Match {
                        candidate,
                        description,
                        matched,
                    },
                ));
            }
        }
        // Stable, so equal scores keep the shell's order. An order that is a
        // ranking already is outranked only by a clearly better match — one
        // more of the typed letters starting a word — and not by the few
        // points two letters side by side earn: `gco` is `git checkout` run
        // ten minutes ago here at least as much as `git commit` run last week.
        if self.ordered {
            scattered.sort_by_key(|(score, _)| std::cmp::Reverse(coarse(*score)));
        } else {
            scattered.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        prefixed.extend(scattered.into_iter().map(|(_, found)| found));
        prefixed
    }

    /// What to insert in place of `word`, or `None` when there is nothing to
    /// add.
    ///
    /// The rule every shell's Tab follows: one candidate is inserted whole,
    /// several insert as much as they agree on, and an answer that agrees on
    /// no more than what is already typed inserts nothing — at which point the
    /// candidates themselves are what a person needs to see.
    ///
    /// In place of `word`, not after it. The match is case-insensitive — see
    /// [`Self::matching`] — so the letters already typed may be the wrong case
    /// for the name the shell returned, `car` for `Cargo.toml`. What comes
    /// back is the shared prefix in the shell's own case, for the caller to put
    /// where `word` was: that is what turns `car` into `Cargo` rather than
    /// leaving `cargo`, a name that is not there.
    pub fn insertion(&self, word: &str) -> Option<String> {
        let prefix = self.common_prefix();
        (prefix.chars().count() > word.chars().count() && starts_with_ignoring_case(&prefix, word))
            .then_some(prefix)
    }
}

/// Whether a space should follow `candidate` once it is the whole word: the
/// space every shell types after a completion that left nothing to choose.
///
/// Not after a name that asks for more to be written onto it, which is what
/// a shell's `nospace` marks: a directory (`src/`), an option that takes its
/// value in the same word (`--format=`), a user before a host (`git@`), a
/// host before a path (`server:`), a list going on (`a,`). The candidates a
/// shell hands back carry that last character with them, so it can be read
/// off the name rather than carried as a flag through three shells' bridges.
pub fn closes_word(candidate: &str) -> bool {
    !candidate.is_empty() && !candidate.ends_with(['/', '=', ':', '@', ','])
}

/// One candidate the menu shows, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match<'a> {
    /// The candidate, as it goes on the line.
    pub candidate: &'a str,
    /// What the shell said about it, if anything.
    pub description: Option<&'a str>,
    /// Which of its characters the typed word matched, by character index, in
    /// order.
    pub matched: Vec<usize>,
}

/// Whether `word`'s characters appear in `candidate` in order, ignoring case,
/// and how well: the score, and the characters matched.
///
/// The fzf family's idea without its machinery. Each character is looked for
/// from where the last one was found, and a match is worth more where it
/// starts a part of the name — after `/`, `-`, `_`, `.` or at the very start —
/// or carries straight on from the one before, which is what makes `fcp` rank
/// `fix/card-pip` above a name that merely contains an f, a c and a p. Greedy
/// rather than optimal, which can miss the best alignment of a word against a
/// long name; the names a shell offers are short, and the order of the list is
/// a hint, not a verdict.
fn scattered_match(candidate: &str, word: &str) -> Option<(i32, Vec<usize>)> {
    let characters: Vec<char> = candidate.chars().collect();
    let mut matched = Vec::new();
    let mut score = 0;
    let mut from = 0;
    for typed in word.chars() {
        let found =
            (from..characters.len()).find(|&at| same_ignoring_case(characters[at], typed))?;
        score += 1;
        if found == 0 || matches!(characters[found - 1], '/' | '-' | '_' | '.' | ' ') {
            score += 8;
        }
        if matched.last().is_some_and(|&last| last + 1 == found) {
            score += 4;
        }
        matched.push(found);
        from = found + 1;
    }
    // Shorter names first among equals: less of them is left unexplained.
    Some((score * 16 - characters.len().min(15) as i32, matched))
}

/// A [`scattered_match`] score with only its big steps kept: how many of the
/// typed letters start a word, near enough. The score is sixteen to a point
/// less up to fifteen for length, and a letter starting a word is worth nine
/// points where a letter beside the last is worth five.
fn coarse(score: i32) -> i32 {
    (score + 15) / 16 / 8
}

/// Whether two characters are the same but for case, in any script.
fn same_ignoring_case(one: char, other: char) -> bool {
    one == other || one.to_lowercase().eq(other.to_lowercase())
}

/// Whether `word` is the start of `candidate` but for case.
///
/// Any script's case, not only ASCII's: `до` is the start of `Документы` as
/// much as `car` is of `Cargo.toml`, and folding ASCII alone left a person
/// with Cyrillic or Greek names unable to narrow a list by typing lowercase.
/// Character by character, so a character whose lowercase is longer than one
/// (`İ`) still lines up with exactly one typed character.
fn starts_with_ignoring_case(candidate: &str, word: &str) -> bool {
    let mut left = candidate.chars();
    word.chars()
        .all(|typed| left.next().is_some_and(|c| same_ignoring_case(c, typed)))
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
///
/// **A tab ends the name**, and what comes after it is the candidate's
/// description. A name cannot hold a raw tab by the time it gets here: all
/// three snippets write a candidate the way it has to be typed, and a typed
/// tab is `$'\t'` in bash and zsh and `\t` in fish.
pub fn parse_answer(text: &str) -> Completions {
    let mut candidates = Vec::new();
    let mut descriptions = Vec::new();
    for line in text.lines() {
        let (name, description) = line.split_once('\t').unwrap_or((line, ""));
        let name = without_trailing_blanks(name);
        if name.is_empty() {
            continue;
        }
        if candidates.len() == MAX_CANDIDATES {
            return Completions {
                candidates,
                descriptions,
                truncated: true,
                ordered: false,
            };
        }
        candidates.push(name.to_owned());
        descriptions.push(description.trim().to_owned());
    }

    Completions {
        candidates,
        descriptions,
        truncated: false,
        ordered: false,
    }
}

/// A line of an answer with the blanks after it taken off — but not a blank
/// the candidate escaped.
///
/// A file whose name ends in a space is written `trail\ ` by all three
/// shells, and trimmed as if the space were padding it came back `trail\`:
/// a backslash left to escape whatever is typed next, and a different word
/// from the one completed. So a run of backslashes an odd number long keeps
/// the one blank it escapes.
pub fn without_trailing_blanks(line: &str) -> &str {
    let trimmed = line.trim_end_matches(char::is_whitespace);
    let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
    match line[trimmed.len()..].chars().next() {
        Some(escaped) if backslashes % 2 == 1 => &line[..trimmed.len() + escaped.len_utf8()],
        _ => trimmed,
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

/// The word a completion would replace: everything after the last space, tab
/// or newline of the line up to the caret.
///
/// The snippets break on a space alone (`${line##* }`); breaking on a tab or a
/// newline as well only ever starts the word *later*, so what this returns is a
/// suffix of what the shell split off — on an ordinary line, with no tab or
/// newline before the caret, the very same word. The caller splices the shell's
/// answer back where this word was, so the two agreeing on where it began is
/// what keeps the result a line both of them meant.
///
/// **A space with a backslash before it is part of the word**, the way every
/// shell reads it — and the way all three snippets write a name with a space
/// in it when they answer. Breaking there made the next Tab after such a name
/// ask about the fragment after the space and find nothing: `cd with\ space/`
/// could not be completed any further. A backslash that is itself escaped is
/// not one, so `a\\ b` still breaks. Quotes are not parsed — `"with sp`
/// completes as `sp` — which offers too *few* completions rather than the
/// wrong ones.
pub fn word_at_end(line_to_caret: &str) -> &str {
    let bytes = line_to_caret.as_bytes();
    let separator = (0..bytes.len()).rev().find(|&at| {
        matches!(bytes[at], b' ' | b'\t' | b'\n')
            && bytes[..at]
                .iter()
                .rev()
                .take_while(|&&byte| byte == b'\\')
                .count()
                % 2
                == 0
    });
    match separator {
        Some(at) => &line_to_caret[at + 1..],
        None => line_to_caret,
    }
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;
