use super::*;

/// The Tab key row's description, as the Shell page writes it.
const TAB_KEY: &str = "What Tab does while a command from your history stands after the caret: \
                       ask the shell to complete the word, or take the suggestion as Right \
                       does. Either way Right takes the suggestion and Ctrl-Space asks the shell.";

#[test]
fn prose_that_fits_in_two_lines_is_not_folded() {
    let short = "Whether one row stands for a single pane or for a whole tab.";
    assert_eq!(fold(short), (short, None));
}

#[test]
fn a_long_description_keeps_its_first_sentence_and_folds_the_rest() {
    let (shown, hidden) = fold(TAB_KEY);
    assert_eq!(
        shown,
        "What Tab does while a command from your history stands after the caret: ask the \
         shell to complete the word, or take the suggestion as Right does."
    );
    assert_eq!(
        hidden,
        Some("Either way Right takes the suggestion and Ctrl-Space asks the shell.")
    );
}

#[test]
fn as_many_whole_sentences_are_shown_as_fit() {
    // Two short sentences fit in the two lines and are both shown; the third
    // would not, and is what waits.
    let text = format!(
        "Connected. Sessions started since it was installed report to Crook. {}",
        "Codex does not say which version of the plugin it has, so Crook cannot tell \
         whether it is behind: Update brings it up to date either way."
    );
    let (shown, hidden) = fold(&text);
    assert_eq!(
        shown,
        "Connected. Sessions started since it was installed report to Crook."
    );
    assert!(hidden.is_some_and(|hidden| hidden.starts_with("Codex does not say")));
}

#[test]
fn a_first_sentence_longer_than_the_fold_is_shown_whole() {
    let long = "word ".repeat(50);
    let text = format!("{}. And another.", long.trim_end());
    let (shown, hidden) = fold(&text);
    assert_eq!(shown, format!("{}.", long.trim_end()));
    assert_eq!(hidden, Some("And another."));
}

#[test]
fn one_long_sentence_is_never_cut() {
    let text = "word ".repeat(60);
    assert_eq!(fold(&text), (text.as_str(), None));
}

#[test]
fn the_dots_in_a_version_or_a_path_do_not_end_a_sentence() {
    // `~/.zprofile`, `0.1.29` and `e.g.` all have a dot followed by
    // something that is not a space and a capital.
    let text = format!(
        "Needs the plugin from v0.1.29 on, and reads ~/.zprofile, ~/.bash_profile and \
         /etc/profile the way e.g. a login does {}. Then it stops.",
        "and so on ".repeat(8).trim_end()
    );
    let (shown, hidden) = fold(&text);
    assert!(shown.ends_with("and so on."), "{shown:?}");
    assert_eq!(hidden, Some("Then it stops."));
}

#[test]
fn a_fold_is_open_after_one_press_and_shut_after_two() {
    let prose = Prose::default();
    let id = id_of(TAB_KEY);
    prose.toggle(id);
    assert!(prose.open.borrow().contains(&id));
    prose.toggle(id);
    assert!(!prose.open.borrow().contains(&id));
}
