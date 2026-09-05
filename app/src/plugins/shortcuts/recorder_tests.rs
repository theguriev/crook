//! What gets written into somebody's keybindings file.
//!
//! The file belongs to the person, not to this control: it may hold comments,
//! their own formatting and rules this build has never heard of, and all of
//! that has to survive a chord being changed. So these are tests about what is
//! *added* rather than about what the file comes out as.

use super::*;

use crate::keybindings::{Keybindings, Source};

fn command() -> ActionName {
    ActionName::parse("crook/window/new-tab").expect("a literal that parses")
}

#[test]
fn a_file_that_does_not_exist_yet_becomes_a_list_with_one_binding_in_it() {
    let written = append("", &command(), "ctrl+alt+t").expect("an empty file is a fresh list");

    assert_eq!(
        written,
        "[\n  { \"command\": \"-crook/window/new-tab\" },\n  \
         { \"key\": \"ctrl+alt+t\", \"command\": \"crook/window/new-tab\" }\n]\n"
    );
}

#[test]
fn everything_already_in_the_file_is_left_exactly_as_it_was() {
    // Comments included, which is the whole reason this appends rather than
    // re-serialising: a tool that "fixed" somebody's file by rewriting it is a
    // tool nobody leaves a comment in.
    let existing =
        "// mine\n[\n  { \"key\": \"ctrl+j\", \"command\": \"crook/palette/open\" }\n]\n";

    let written = append(existing, &command(), "ctrl+alt+t").expect("a list to append to");

    assert!(written.starts_with("// mine\n["));
    assert!(written.contains("\"command\": \"crook/palette/open\""));
    assert!(written.contains("\"key\": \"ctrl+alt+t\""));
}

#[test]
fn what_is_added_is_a_removal_and_then_a_binding() {
    // "Change" rather than "add", in the file's own vocabulary: a `-command`
    // rule takes every chord that command had, and the line after it gives it
    // the one that was just pressed. The last matching rule wins, so what a
    // person reads at the bottom of their file is what their keyboard does.
    let existing = "[\n  { \"key\": \"ctrl+t\", \"command\": \"crook/window/new-tab\" }\n]\n";

    let written = append(existing, &command(), "ctrl+alt+t").expect("a list to append to");
    let bindings = read_written(&written);

    let chords: Vec<String> = bindings
        .effective()
        .iter()
        .filter(|rule| rule.command == command())
        .map(|rule| rule.chord())
        .collect();
    assert_eq!(chords, ["ctrl+alt+t"], "the old chord is gone with the new");
}

#[test]
fn a_file_that_is_not_a_list_is_left_alone_and_reported() {
    // Somebody's file, and somebody's mistake. Overwriting it would be this
    // control deciding it knows better than the person who wrote it.
    let problem =
        append("{ \"key\": \"ctrl+t\" }", &command(), "ctrl+alt+t").expect_err("not a list");

    assert_eq!(problem, "the keybindings file is not a list");
}

/// Reads what was written back through the real loader.
fn read_written(text: &str) -> Keybindings {
    let scratch = crate::plugins::wasm::tests::Scratch::new("keybindings");
    let path = scratch.path().join("keybindings.json");
    std::fs::write(&path, text).expect("the scratch file should be writable");
    let keybindings = Keybindings::load(&path);
    assert!(
        !keybindings.is_empty(),
        "what was written should be readable: {text}"
    );
    assert!(
        keybindings
            .effective()
            .iter()
            .any(|rule| rule.source == Source::User),
        "and should arrive as the person's own"
    );
    keybindings
}
