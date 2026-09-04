//! What a `when` clause means, and what it refuses to mean.

use super::*;

/// The context the window would hand a clause on the settings page.
fn context() -> Context {
    Context::new()
        .with("settingsFocused", true)
        .with("searchFocused", false)
        .with("paneFocused", true)
        .with_word("platform", "mac")
}

fn holds(clause: &str) -> bool {
    When::parse(clause)
        .unwrap_or_else(|| panic!("{clause:?} did not parse"))
        .evaluate(&context())
}

#[test]
fn a_bare_key_is_its_own_value() {
    assert!(holds("settingsFocused"));
    assert!(!holds("searchFocused"));
}

#[test]
fn a_key_the_window_says_nothing_about_is_false() {
    // A clause written against a build that has a key this one does not. It
    // is false rather than an error, which is what makes a keybindings file
    // shared between two versions cost one binding instead of all of them.
    assert!(!holds("editorTextFocus"));
    assert!(holds("!editorTextFocus"));
}

#[test]
fn negation_conjunction_and_disjunction_bind_the_way_they_do_in_vscode() {
    assert!(holds("!searchFocused && paneFocused"));
    assert!(!holds("!searchFocused && searchFocused"));
    assert!(holds("searchFocused || paneFocused"));
    // `&&` binds tighter than `||`: this is `false || (true && true)`.
    assert!(holds("searchFocused || settingsFocused && paneFocused"));
    assert!(!holds(
        "(searchFocused || settingsFocused) && searchFocused"
    ));
}

#[test]
fn a_comparison_reads_a_word_key() {
    assert!(holds("platform == mac"));
    assert!(holds("platform == 'mac'"));
    assert!(holds("platform != other"));
    assert!(!holds("platform == other"));
}

#[test]
fn a_comparison_against_a_key_the_window_lacks_is_never_equal() {
    // VSCode's `undefined != 'x'`, which is what lets a clause guard against
    // a key that only newer builds set.
    assert!(holds("language != rust"));
    assert!(!holds("language == rust"));
}

#[test]
fn a_flag_compares_as_a_word() {
    assert!(holds("settingsFocused == true"));
    assert!(holds("searchFocused == false"));
}

#[test]
fn what_does_not_parse_is_refused_rather_than_half_read() {
    // Each of these would otherwise bind a chord under a condition nobody
    // wrote: the half of the clause that happened to parse.
    for clause in [
        "",
        "&&",
        "a &&",
        "a & b",
        "a = b",
        "a =~ /b/",
        "(a || b",
        "a || b)",
        "a b",
        "!",
        "a == ",
        "a ++ b",
        "'unterminated",
    ] {
        assert_eq!(When::parse(clause), None, "{clause:?} parsed");
    }
}

#[test]
fn a_clause_names_the_keys_it_reads() {
    // What the settings page prints beside a binding, so that a row explains
    // why the chord did nothing.
    let clause = When::parse("!searchFocused && (paneFocused || platform == mac)").expect("parses");
    assert_eq!(clause.names(), ["searchFocused", "paneFocused", "platform"]);
}

#[test]
fn the_context_lists_what_it_knows() {
    assert_eq!(
        context().keys(),
        [
            ("paneFocused", "true"),
            ("platform", "mac"),
            ("searchFocused", "false"),
            ("settingsFocused", "true"),
        ]
    );
}
