//! The query, and what it matches.
//!
//! Everything here is a pure function of a string and a row's words, which is
//! the reason it is a module of its own: what "matches" means is the whole of
//! the feature's behaviour, and it is worth being able to read it in one place
//! and test it without a window.

/// What a row can be found by.
///
/// The label and the description are what the row already says. The keywords
/// are what it does not: the words somebody would type looking for it, which
/// are very often not the words the interface chose. "Tab placement" is what
/// the row is called; "sidebar" is what a person types.
/// Owned rather than `&'static str`, and that is not an accident of
/// convenience. A row's label used to be a literal in this crate because every
/// row was; a plugin's named action is a string that does not exist until the
/// plugin has built, and a page that could only describe rows written down at
/// compile time is a page no plugin can appear on.
#[derive(Clone, Debug)]
pub(super) struct Words {
    /// What the row is called.
    pub(super) label: String,
    /// The line under the label, where there is one.
    pub(super) description: Option<String>,
    /// Words that find the row but are not written on it.
    pub(super) keywords: Vec<String>,
}

impl Words {
    /// A row found only by what it says.
    pub(super) fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
            keywords: Vec::new(),
        }
    }

    /// The same, with the line under it.
    pub(super) fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The same, with words that are not written on the row.
    pub(super) fn with_keywords(mut self, keywords: &[&str]) -> Self {
        self.keywords = keywords.iter().map(|word| (*word).to_owned()).collect();
        self
    }
}

/// What has been typed into the rail's field, ready to match against.
///
/// Held as terms rather than as the raw string because every match asks the
/// same question of the same words, and splitting once per frame is cheaper
/// and — more to the point — is where the "every term has to match" rule is
/// written down.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Query {
    terms: Vec<String>,
}

impl Query {
    /// The query somebody typed.
    ///
    /// Lowercased, because nobody searching a settings page means their shift
    /// key, and split on whitespace so that "show chip" finds a row that says
    /// "Show the usage chip". Every term has to match *something* about a row,
    /// but they need not match the same thing or be in that order: this is a
    /// person narrowing a list, not writing a pattern.
    pub(super) fn new(text: &str) -> Self {
        Self {
            terms: text
                .split_whitespace()
                .map(|term| term.to_lowercase())
                .collect(),
        }
    }

    /// Whether nothing has been typed, in which case nothing is filtered.
    pub(super) fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether a row with these words is one of the answers.
    ///
    /// `context` is the page and the category the row is in, which count as
    /// words of its own: searching "keys" should find every binding, and
    /// searching "theme" should find the row inside the category called
    /// Theme even though the row itself is called something else.
    pub(super) fn matches(&self, words: &Words, context: &[&str]) -> bool {
        if self.is_empty() {
            return true;
        }

        // Substring rather than fuzzy. A fuzzy match over thirty rows finds
        // something for almost any typing, and a settings page that answers
        // every query with a row is worse than one that answers some with
        // none: the answer stops meaning anything.
        self.terms.iter().all(|term| {
            contains(&words.label, term)
                || words
                    .description
                    .as_deref()
                    .is_some_and(|line| contains(line, term))
                || words.keywords.iter().any(|word| contains(word, term))
                || context.iter().any(|word| contains(word, term))
        })
    }
}

/// Whether `haystack` contains `term`, which is already lowercase.
///
/// Allocating a lowercase copy per comparison rather than walking both in
/// step: this runs over thirty rows on a keystroke, and the version that does
/// not allocate is the version that gets Turkish dotless I wrong.
fn contains(haystack: &str, term: &str) -> bool {
    haystack.to_lowercase().contains(term)
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
