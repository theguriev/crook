//! Finding an emoji by what it is called.
//!
//! The table is [`emoji_data`](super::emoji_data), every emoji Unicode has
//! but the skin-tone repeats, with its English name and CLDR's keywords in
//! English and Russian. A search is a few words; an emoji matches when every
//! one of them starts some word it is known by, or failing that appears
//! inside one — so `fox` finds the fox before it finds `firefox`-ish strays,
//! and `лис` finds it too.

use std::collections::HashSet;
use std::sync::OnceLock;

use unicode_segmentation::UnicodeSegmentation;

use super::emoji_data::EMOJI;

/// Up to `limit` emoji for `query`, best first.
///
/// Best is, in order: the query *being* the emoji's name — `dragon` is the
/// dragon before it is the dragon's face — then the start of its name, then
/// every word of the query starting some word the emoji is known by, then
/// merely appearing inside one. Ties go by Unicode's order, which is
/// grouped, so a search that matches a dozen animals lists them as Unicode
/// does.
pub(crate) fn search(query: &str, limit: usize) -> Vec<&'static str> {
    let query = query.to_lowercase();
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let joined = terms.join(" ");

    let mut found: Vec<(u8, usize, &'static str)> = EMOJI
        .iter()
        .enumerate()
        .filter_map(|(index, (emoji, name, words))| {
            let mut inside = false;
            for term in &terms {
                if words.split(' ').any(|word| word.starts_with(term)) {
                    continue;
                }
                if words.contains(term) {
                    inside = true;
                    continue;
                }
                return None;
            }
            let name = name.to_lowercase();
            let score = if inside {
                3
            } else if name == joined {
                0
            } else if name.starts_with(&joined) {
                1
            } else {
                2
            };
            Some((score, index, *emoji))
        })
        .collect();
    found.sort_unstable_by_key(|(score, index, _)| (*score, *index));
    found
        .into_iter()
        .take(limit)
        .map(|(_, _, emoji)| emoji)
        .collect()
}

/// Whether the first grapheme of `text` is an emoji rather than a letter.
///
/// What tells a paste from a search, in a field that takes both. By the
/// table, with the variation selector and any skin tone taken off first:
/// a pasted emoji carries whichever of those its picker added, and the
/// table holds each picture once.
pub(crate) fn is_emoji(text: &str) -> bool {
    let Some(grapheme) = text.trim().graphemes(true).next() else {
        return false;
    };
    known().contains(&bare(grapheme))
}

/// `emoji` without U+FE0F and the five skin-tone modifiers.
fn bare(emoji: &str) -> String {
    emoji
        .chars()
        .filter(|character| !matches!(character, '\u{fe0f}' | '\u{1f3fb}'..='\u{1f3ff}'))
        .collect()
}

/// Every emoji in the table, bare.
fn known() -> &'static HashSet<String> {
    static KNOWN: OnceLock<HashSet<String>> = OnceLock::new();
    KNOWN.get_or_init(|| EMOJI.iter().map(|(emoji, _, _)| bare(emoji)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOX: &str = "\u{1f98a}";

    #[test]
    fn a_name_finds_its_emoji_first_in_either_language() {
        assert_eq!(search("fox", 5).first(), Some(&FOX));
        assert_eq!(search("Fox", 5).first(), Some(&FOX), "case mattered");
        assert!(search("лиса", 5).contains(&FOX), "the Russian word missed");
        assert!(
            // Every cat face says кот as well, and they come first in
            // Unicode's order; what matters is that the cat is in the grid.
            search("кот", crate::plugins::tabs::EMOJI_FOUND).contains(&"\u{1f408}"),
            "кот missed the cat"
        );
    }

    #[test]
    fn every_word_of_a_search_has_to_match() {
        let red_heart = search("red heart", 40);
        assert!(red_heart.contains(&"\u{2764}\u{fe0f}"), "{red_heart:?}");
        assert!(
            !red_heart.contains(&"\u{1f499}"),
            "the blue heart matched a search for a red one"
        );
        assert!(search("fox zzzz", 5).is_empty());
        assert!(search("   ", 5).is_empty());
    }

    #[test]
    fn a_search_reaches_past_the_picks_to_the_whole_table() {
        // Flags, joined sequences and keycaps are in the table as well as the
        // single code points, which is what "any emoji" has to mean.
        assert!(search("germany", 5).contains(&"\u{1f1e9}\u{1f1ea}"));
        assert!(search("pirate", 5).contains(&"\u{1f3f4}\u{200d}\u{2620}\u{fe0f}"));
        assert!(EMOJI.len() > 1800, "the table is {} long", EMOJI.len());
    }

    #[test]
    fn an_emoji_is_told_from_a_word_however_it_was_pasted() {
        assert!(is_emoji(FOX));
        assert!(is_emoji(" \u{1f98a} and more"));
        assert!(is_emoji("\u{2764}"), "a heart without its selector");
        assert!(is_emoji("\u{1f44d}\u{1f3fd}"), "a thumb with a skin tone");
        assert!(!is_emoji("fox"));
        assert!(!is_emoji("лиса"));
        assert!(!is_emoji(""));
    }
}
