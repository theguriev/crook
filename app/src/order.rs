//! How a list of names is ordered: by the name, not by the case of its
//! first letter.
//!
//! `str::cmp` puts every capital before every lowercase — `Worktree` before
//! `dziling` — because that is where the bytes fall. A list a person reads
//! down is alphabetical the way a dictionary is, and a plugin whose author
//! spelled it in lowercase belongs between `Chips` and `Emoji`, not after
//! everything.

use std::cmp::Ordering;

/// `left` against `right`, letter by letter with the case folded away, and
/// by the bytes only where the folded names are the same — so two names that
/// differ only in case still have one order between two fetches.
///
/// No allocation: the fold is an iterator over each character's lowercase,
/// which is what lets the palette's launcher use it on every keystroke.
pub fn by_name(left: &str, right: &str) -> Ordering {
    let mut lefts = left.chars().flat_map(char::to_lowercase);
    let mut rights = right.chars().flat_map(char::to_lowercase);
    loop {
        match (lefts.next(), rights.next()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) => match l.cmp(&r) {
                Ordering::Equal => continue,
                other => return other,
            },
        }
    }
    // The folded names agree; the spelling is the tie-break, so that the
    // order is total and the same every time.
    left.cmp(right)
}

#[cfg(test)]
mod tests {
    use super::by_name;
    use std::cmp::Ordering;

    #[test]
    fn a_lowercase_name_sorts_among_the_capitals_rather_than_after_them() {
        let mut names = vec!["Worktree", "dziling", "Chips", "Emoji"];
        names.sort_by(|left, right| by_name(left, right));
        assert_eq!(names, ["Chips", "dziling", "Emoji", "Worktree"]);
    }

    #[test]
    fn names_that_differ_only_in_case_still_have_one_order() {
        assert_eq!(by_name("pirate", "Pirate"), Ordering::Greater);
        assert_eq!(by_name("Pirate", "pirate"), Ordering::Less);
        assert_eq!(by_name("Pirate", "Pirate"), Ordering::Equal);
    }

    #[test]
    fn a_prefix_comes_first() {
        assert_eq!(by_name("Tab", "Tabs"), Ordering::Less);
        assert_eq!(by_name("tabs", "Tab"), Ordering::Greater);
    }
}
