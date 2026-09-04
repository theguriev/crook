use super::*;

/// The row the whole feature exists for: called one thing, looked for under
/// another.
///
/// A function rather than a `const`, because a row's words are owned now — see
/// [`Words`] — so that a plugin's action can be a row.
fn placement() -> Words {
    Words::new("Tab placement")
        .with_description("Where the list of what you are working on lives.")
        .with_keywords(&["sidebar", "strip", "vertical", "horizontal"])
}

const CONTEXT: [&str; 2] = ["Appearance", "Tabs"];

#[test]
fn an_empty_query_matches_everything() {
    let query = Query::new("");

    assert!(query.is_empty());
    assert!(query.matches(&placement(), &CONTEXT));
}

#[test]
fn whitespace_alone_is_an_empty_query() {
    // What is left after somebody types a word and deletes it, or begins with
    // a space. A query of one empty term matches every row by `contains`, so
    // the page would be "filtered" to all of itself with the empty state's
    // rules in force.
    assert!(Query::new("   ").is_empty());
}

#[test]
fn a_row_is_found_by_its_label_whatever_the_case() {
    assert!(Query::new("placement").matches(&placement(), &CONTEXT));
    assert!(Query::new("PLACEMENT").matches(&placement(), &CONTEXT));
    assert!(Query::new("Tab Pla").matches(&placement(), &CONTEXT));
}

#[test]
fn a_row_is_found_by_the_line_under_it() {
    assert!(Query::new("working on lives").matches(&placement(), &CONTEXT));
}

#[test]
fn a_row_is_found_by_a_word_that_is_not_written_on_it() {
    // The point of the keywords: nothing on this row says "sidebar", and
    // "sidebar" is what somebody looking for it types.
    assert!(Query::new("sidebar").matches(&placement(), &CONTEXT));
    assert!(Query::new("vertical").matches(&placement(), &CONTEXT));
}

#[test]
fn a_row_is_found_by_the_page_and_the_category_it_is_in() {
    assert!(Query::new("appearance").matches(&placement(), &CONTEXT));
    assert!(Query::new("tabs").matches(&placement(), &CONTEXT));
}

#[test]
fn every_term_has_to_match_but_not_the_same_thing() {
    // "sidebar" is a keyword and "appearance" is the page: neither is in the
    // other's field, and together they still name this row.
    assert!(Query::new("sidebar appearance").matches(&placement(), &CONTEXT));
    assert!(
        !Query::new("sidebar usage").matches(&placement(), &CONTEXT),
        "a term that matches nothing should rule the row out"
    );
}

#[test]
fn a_term_that_is_in_nothing_matches_nothing() {
    assert!(!Query::new("keyboard").matches(&placement(), &CONTEXT));
}

#[test]
fn a_query_of_two_of_the_same_term_is_the_same_query() {
    // Not deduplicated, and it must not matter: `all` over a repeated term is
    // the term twice, which is the term.
    assert!(Query::new("tab tab").matches(&placement(), &CONTEXT));
}
