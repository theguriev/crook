//! Comments on the lines of the Changes column, and the one message they
//! become.
//!
//! # Said to the agent, never done to the tree
//!
//! Reading a diff turns up things to change, and the column still writes
//! nothing: a "revert this" that Crook carried out would land in a tree an
//! agent is in the middle of writing, between its read of a file and its
//! write of it. So what a person finds becomes a comment on the line, the
//! comments become one message, and the message goes to the agent — which
//! makes the change itself, in the order it chooses, knowing it made it.
//!
//! # Held per tab, in memory
//!
//! A review belongs to a [`Key`]: the tab, the repository the column is
//! reading for it and the base it compares with. Two tabs on one repository
//! are two pieces of work and two reviews; one tab whose pane moves to
//! another repository and back finds its comments where it left them. They
//! live in memory and nowhere else — a restart, or closing the tab, loses
//! them — because a review is minutes of reading about a diff that is itself
//! moving, and a comment restored a day later would be about lines that have
//! long since changed under it.
//!
//! # A comment follows its line
//!
//! A comment remembers its line by what the line says, marker included, and
//! by where it was ([`Anchor`]). The agent goes on writing while a person
//! reads, and every refresh reads the diff again: [`reanchor`] moves the
//! comment to the line that says the same thing nearest to where it was, and
//! when no line says it any more — the agent already changed it, or undid the
//! whole file — the comment is dropped and the column says which. A comment
//! on a hunk's header is matched by the function name git prints after it
//! rather than by its numbers, which change whenever anything above the hunk
//! does.
//!
//! Not finding the line in a diff that was cut short is not the same as the
//! line being gone: the agent may only have written enough above it to push
//! it past the cut. Nor is it proof of the line being there — the agent may
//! as well have deleted it from the part that was read. Such a comment is
//! kept, marked [`Comment::past_cut`], and listed where the diff stops, under
//! the line it was last found on and with its ×, so the person can see it
//! will be sent and take it out; it is sent with that line and the number it
//! had then. The next read that finds the line puts it back under it; the
//! next one that is not cut and still does not find it drops it.
//!
//! # The message
//!
//! [`compose`]: a heading that says whose work since when, then each comment
//! as `path:line`, the line quoted with its `+` or `-`, and what was said
//! about it, a blank line between them. No newline at the end: the message
//! is pasted into the agent's prompt and left there for the person to read
//! and send, and a trailing newline is an Enter in every program that has
//! not asked for bracketed paste.

use std::path::PathBuf;

use crate::git::changes::{Against, Base};
use crate::tab::TabId;

/// Which review a comment belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    /// The tab the column was about when the comment was made.
    pub(crate) tab: TabId,
    /// The top of the repository it was reading.
    pub(crate) repository: PathBuf,
    /// What that repository's work was compared with, by name.
    pub(crate) base: String,
}

/// Where a line a comment can be left on is in its file.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// A hunk's header, at the line its new side begins on — or, for a hunk
    /// that takes everything away, where its old side did.
    Hunk(u32),
    /// A line that was added, numbered in the file as it is now.
    Added(u32),
    /// A line that was removed, numbered in the file as it was.
    Removed(u32),
}

impl Place {
    /// The number `path:line` is written with.
    fn number(self) -> u32 {
        match self {
            Self::Hunk(number) | Self::Added(number) | Self::Removed(number) => number,
        }
    }
}

/// The line a comment is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Anchor {
    /// The file, from the top of the repository.
    pub(crate) path: PathBuf,
    /// What the line said, marker and all, as the column drew it.
    pub(crate) line: String,
    /// Where it was in the file.
    pub(crate) place: Place,
    /// Which line of the file's diff it was, as last read: what the column
    /// draws the comment under.
    pub(crate) at: usize,
}

/// One comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Comment {
    /// Its name, which is what its × removes.
    pub(crate) id: u64,
    /// The line it is about.
    pub(crate) anchor: Anchor,
    /// What was said.
    pub(crate) text: String,
    /// Whether the last read of its file was cut short and did not find its
    /// line, so that it is kept and listed under the cut rather than drawn
    /// under a line. See the module's docs.
    pub(crate) past_cut: bool,
}

/// Where each line of a diff is, for the lines a comment can be left on:
/// `None` for a line of context and for git's `\ No newline` note, neither
/// of which is a change.
///
/// Counted from each hunk's header — `@@ -12,7 +14,8 @@` starts the old side
/// at line 12 and the new at 14 — through the lines under it.
pub(crate) fn places(lines: &[String]) -> Vec<Option<Place>> {
    let mut old = 0;
    let mut new = 0;
    lines
        .iter()
        .map(|line| {
            if line.starts_with("@@") {
                let (from, to) = starts(line).unwrap_or((0, 0));
                old = from;
                new = to;
                return Some(Place::Hunk(if to == 0 { from } else { to }));
            }
            match line.as_bytes().first() {
                Some(b'+') => {
                    new += 1;
                    Some(Place::Added(new - 1))
                }
                Some(b'-') => {
                    old += 1;
                    Some(Place::Removed(old - 1))
                }
                // An empty line is a blank context line whose space was
                // dropped, and counts as one.
                Some(b' ') | None => {
                    old += 1;
                    new += 1;
                    None
                }
                _ => None,
            }
        })
        .collect()
}

/// Where a hunk header says its old and new sides begin.
fn starts(header: &str) -> Option<(u32, u32)> {
    let mut parts = header.split_whitespace().skip(1);
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let first = |side: &str| side.split(',').next()?.parse().ok();
    Some((first(old)?, first(new)?))
}

/// Whether a line of a diff is one a comment can be left on: a line added or
/// removed, or a hunk's header, which stands for the hunk.
pub(crate) fn commentable(line: &str) -> bool {
    line.starts_with("@@") || matches!(line.as_bytes().first(), Some(b'+' | b'-'))
}

/// What a hunk header says after its numbers: the function or heading git
/// found above the hunk, which stays put when the numbers move.
fn section(header: &str) -> &str {
    header
        .strip_prefix("@@")
        .and_then(|rest| rest.split_once("@@"))
        .map_or("", |(_, section)| section.trim())
}

/// Whether `candidate` is the line `anchor` was on.
fn same_line(anchor: &str, candidate: &str) -> bool {
    if anchor.starts_with("@@") {
        return candidate.starts_with("@@") && section(anchor) == section(candidate);
    }
    anchor == candidate
}

/// Where the line `anchor` was on is in a diff read since: the line that
/// says the same thing nearest to where it was, or `None` when no line says
/// it any more.
///
/// Nearest by its number in the file, which moves with the line when the
/// agent writes above it, and by its place in the diff when two are as near
/// — two lines that say the same thing, a closing brace or a blank, where
/// either is a guess and the one that moved least is the better one.
pub(crate) fn reanchor(
    anchor: &Anchor,
    lines: &[String],
    places: &[Option<Place>],
) -> Option<(usize, Place)> {
    let was = i64::from(anchor.place.number());
    let at = anchor.at as i64;
    lines
        .iter()
        .zip(places)
        .enumerate()
        .filter(|(_, (line, _))| same_line(&anchor.line, line))
        .filter_map(|(index, (_, place))| Some((index, (*place)?)))
        .min_by_key(|(index, place)| {
            (
                (i64::from(place.number()) - was).abs(),
                (*index as i64 - at).abs(),
            )
        })
}

/// What the message opens with: whose work, since when.
///
/// `branch` is what the row calls the checkout's head — a branch's name, or
/// a short commit id for a head that is on none — and `None` where there is
/// no head to name.
pub(crate) fn heading(branch: Option<&str>, base: &Base) -> String {
    match (base.against, branch) {
        (Against::Branch, Some(branch)) => format!("Review of {branch} since {}:", base.name),
        (Against::Branch, None) => format!("Review of the work since {}:", base.name),
        (Against::Head, Some(branch)) => {
            format!("Review of the uncommitted changes on {branch}:")
        }
        (Against::Head, None) => "Review of the uncommitted changes:".to_owned(),
        (Against::Nothing, Some(branch)) => {
            format!("Review of {branch}, where nothing is committed yet:")
        }
        (Against::Nothing, None) => {
            "Review of the work, where nothing is committed yet:".to_owned()
        }
    }
}

/// `path:line`, the way a comment is introduced in the message.
///
/// A removed line has no number in the file as it is now, so it is given
/// the one it had, and says so: an agent that opened the file at that
/// number would otherwise find some other line there and take the comment
/// to be about it.
pub(crate) fn location(anchor: &Anchor) -> String {
    let path = anchor.path.display();
    match anchor.place {
        Place::Hunk(number) | Place::Added(number) => format!("{path}:{number}"),
        Place::Removed(number) => {
            format!("{path}:{number} (removed; numbered as before the change)")
        }
    }
}

/// The message: `heading`, then every comment in `comments`, in the order
/// given. No newline at the end — see the module's own docs.
pub(crate) fn compose(heading: &str, comments: &[&Comment]) -> String {
    let mut message = heading.to_owned();
    for comment in comments {
        message.push_str("\n\n");
        message.push_str(&location(&comment.anchor));
        message.push_str("\n> ");
        message.push_str(&comment.anchor.line);
        message.push('\n');
        message.push_str(comment.text.trim());
    }
    message
}

/// What the column says when a refresh has dropped comments whose lines are
/// gone, naming each by where it was.
pub(crate) fn dropped(locations: &[String]) -> Option<String> {
    match locations {
        [] => None,
        [one] => Some(format!(
            "Dropped the comment on {one}: that line is no longer in the diff."
        )),
        many => Some(format!(
            "Dropped {} comments whose lines are no longer in the diff: {}.",
            many.len(),
            many.join(", ")
        )),
    }
}
