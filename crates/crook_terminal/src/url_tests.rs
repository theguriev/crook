use super::*;

/// The URL under a column, as its text.
fn uri_at(text: &str, column: usize) -> Option<String> {
    at(text, column).map(|url| url.uri)
}

/// The cells a URL covers, for checking that a highlight lands on the link and
/// not on the punctuation beside it.
fn span_at(text: &str, column: usize) -> Option<(usize, usize)> {
    at(text, column).map(|url| (url.start, url.len))
}

#[test]
fn test_a_bare_url_is_found_from_any_cell_of_it() {
    let row = "see https://example.com/a for more";
    let url = "https://example.com/a";

    // Every cell of the link answers with the whole link, which is what makes
    // a click land wherever the pointer happens to be inside it.
    for column in 4..4 + url.len() {
        assert_eq!(
            uri_at(row, column).as_deref(),
            Some(url),
            "column {column} did not find the link"
        );
    }

    assert_eq!(span_at(row, 10), Some((4, url.len())));
}

#[test]
fn test_the_text_around_a_url_is_not_part_of_it() {
    let row = "see https://example.com/a for more";

    assert_eq!(uri_at(row, 0), None, "the word before it");
    assert_eq!(uri_at(row, 3), None, "the space before it");
    assert_eq!(uri_at(row, 25), None, "the space after it");
    assert_eq!(uri_at(row, 30), None, "the word after it");
    assert_eq!(uri_at(row, 999), None, "past the end of the row");
}

#[test]
fn test_a_url_that_ends_a_sentence_does_not_take_the_full_stop() {
    // No URL anybody wants to click ends in sentence punctuation.
    for (row, expected) in [
        ("go to https://example.com.", "https://example.com"),
        ("go to https://example.com,", "https://example.com"),
        ("go to https://example.com!?", "https://example.com"),
        ("go to https://example.com/a;", "https://example.com/a"),
    ] {
        assert_eq!(uri_at(row, 10).as_deref(), Some(expected), "in {row:?}");
    }
}

#[test]
fn test_a_bracket_belongs_to_the_url_only_if_the_url_opened_it() {
    // The rule that separates the two cases people actually hit.
    assert_eq!(
        uri_at("(see https://example.com/a)", 10).as_deref(),
        Some("https://example.com/a"),
        "the bracket was the sentence's"
    );
    assert_eq!(
        uri_at("https://en.wikipedia.org/wiki/A_(b)", 10).as_deref(),
        Some("https://en.wikipedia.org/wiki/A_(b)"),
        "the bracket was the URL's"
    );

    // And a URL that starts inside a bracket is still found: the link does not
    // have to begin the run it is in.
    assert_eq!(
        span_at("(https://example.com)", 5),
        Some((1, "https://example.com".len()))
    );
}

#[test]
fn test_only_the_schemes_a_terminal_prints_are_recognised() {
    for row in [
        "http://example.com",
        "HTTPS://EXAMPLE.COM",
        "ftp://example.com/f",
        "file:///var/log/x",
        "ssh://host/path",
        "git://host/repo",
        "mailto:someone@example.com",
    ] {
        assert!(uri_at(row, 2).is_some(), "{row} was not recognised");
    }

    // A bare hostname is a word. Underlining it would make ordinary prose
    // twitch under the pointer, and clicking it would guess at a scheme.
    assert_eq!(uri_at("visit example.com today", 8), None);
    assert_eq!(uri_at("visit www.example.com today", 8), None);
}

#[test]
fn test_a_scheme_with_nothing_after_it_is_not_a_link() {
    assert_eq!(uri_at("https://", 2), None);
    assert_eq!(uri_at("see https:// and stop", 6), None);
}

#[test]
fn test_a_url_in_quotes_or_angle_brackets_stops_at_them() {
    // Both are how a URL is routinely printed, and neither character is ever
    // part of one.
    assert_eq!(
        uri_at("\"https://example.com/a\"", 5).as_deref(),
        Some("https://example.com/a")
    );
    assert_eq!(
        uri_at("<https://example.com/a>", 5).as_deref(),
        Some("https://example.com/a")
    );
}

#[test]
fn test_a_row_with_two_links_answers_with_the_one_under_the_pointer() {
    let row = "https://one.example https://two.example";

    assert_eq!(uri_at(row, 3).as_deref(), Some("https://one.example"));
    assert_eq!(uri_at(row, 25).as_deref(), Some("https://two.example"));
    assert_eq!(uri_at(row, 19), None, "the space between them");
}

#[test]
fn test_the_span_is_in_cells_so_a_highlight_lands_on_the_link() {
    // Every offset here is a cell of the row, which is what a caller has: a
    // snapshot row and a harvested block row are both one char per cell.
    let row = "  https://example.com";
    let url = at(row, 5).expect("the link is found");

    assert_eq!(url.start, 2);
    assert_eq!(url.end(), row.chars().count());
    assert!(url.contains(2));
    assert!(!url.contains(1));
    assert!(!url.contains(url.end()));
}
