//! What an edit to a keybindings file does to the rest of the file.

use super::*;

/// The command of an entry, for a predicate that reads like the file does.
fn command(entry: &Value) -> &str {
    entry.get("command").and_then(Value::as_str).unwrap_or("")
}

#[test]
fn an_entry_added_to_an_empty_file_makes_the_array() {
    let mut document = Document::new("");
    assert!(document.append(r#"{ "key": "ctrl+t", "command": "a/b/c" }"#));

    assert_eq!(
        document.text(),
        "[\n    { \"key\": \"ctrl+t\", \"command\": \"a/b/c\" }\n]\n"
    );
    assert_eq!(document.entries().len(), 1);
}

#[test]
fn an_entry_is_added_after_the_last_one_at_its_indentation() {
    let mut document = Document::new("[\n  { \"command\": \"a/b/c\" }\n]\n");
    document.append(r#"{ "command": "a/b/d" }"#);

    assert_eq!(
        document.text(),
        "[\n  { \"command\": \"a/b/c\" },\n  { \"command\": \"a/b/d\" }\n]\n"
    );
}

#[test]
fn an_entry_added_to_an_empty_array_does_not_leave_the_file_on_one_line() {
    let mut document = Document::new("[]\n");
    document.append(r#"{ "command": "a/b/c" }"#);

    assert_eq!(document.text(), "[\n    { \"command\": \"a/b/c\" }\n]\n");
}

#[test]
fn everything_a_person_wrote_around_the_entries_survives_an_edit() {
    // The whole point of editing the text rather than a `Vec<Value>`: the
    // comments, the blank line and the ordering are all still there
    // afterwards, and so is the entry naming a plugin this build never heard
    // of.
    let file = "\
// My bindings.
[
    // The one I use most.
    { \"key\": \"ctrl+t\", \"command\": \"a/b/c\" },

    { \"key\": \"ctrl+q\", \"command\": \"nobody/at/all\" }
]
";
    let mut document = Document::new(file);
    document.append(r#"{ "key": "ctrl+j", "command": "a/b/d" }"#);

    assert_eq!(
        document.text(),
        "\
// My bindings.
[
    // The one I use most.
    { \"key\": \"ctrl+t\", \"command\": \"a/b/c\" },

    { \"key\": \"ctrl+q\", \"command\": \"nobody/at/all\" },
    { \"key\": \"ctrl+j\", \"command\": \"a/b/d\" }
]
"
    );
}

#[test]
fn an_entry_is_removed_with_its_comma_and_its_line() {
    let mut document =
        Document::new("[\n    { \"command\": \"a/b/c\" },\n    { \"command\": \"a/b/d\" }\n]\n");
    assert_eq!(document.remove(|entry| command(entry) == "a/b/c"), 1);

    assert_eq!(document.text(), "[\n    { \"command\": \"a/b/d\" }\n]\n");
    assert!(serde_json::from_str::<Value>(document.text()).is_ok());
}

#[test]
fn removing_the_last_entry_takes_the_comma_before_it() {
    let mut document =
        Document::new("[\n    { \"command\": \"a/b/c\" },\n    { \"command\": \"a/b/d\" }\n]\n");
    document.remove(|entry| command(entry) == "a/b/d");

    assert_eq!(document.text(), "[\n    { \"command\": \"a/b/c\" }\n]\n");
    assert!(serde_json::from_str::<Value>(document.text()).is_ok());
}

#[test]
fn removing_every_entry_leaves_a_file_that_still_parses() {
    let mut document =
        Document::new("[\n    { \"command\": \"a/b/c\" },\n    { \"command\": \"a/b/d\" }\n]\n");
    assert_eq!(document.remove(|_| true), 2);

    assert!(serde_json::from_str::<Value>(document.text()).is_ok());
    assert!(document.entries().is_empty());
}

#[test]
fn a_comma_or_a_bracket_inside_a_string_is_not_one() {
    // The entry's own text contains everything that ends an entry, and a
    // scanner that did not know it was in a string would cut it in half.
    let mut document =
        Document::new("[\n    { \"key\": \"ctrl+,\", \"command\": \"a/b/],[\" }\n]\n");

    assert_eq!(document.entries().len(), 1);
    assert_eq!(document.remove(|entry| command(entry) == "a/b/],["), 1);
    assert!(document.entries().is_empty());
}

#[test]
fn a_nested_object_does_not_end_an_entry() {
    let mut document =
        Document::new("[\n    { \"command\": \"a/b/c\", \"args\": { \"n\": [1, 2] } }\n]\n");

    assert_eq!(document.entries().len(), 1);
    document.append(r#"{ "command": "a/b/d" }"#);
    assert_eq!(document.entries().len(), 2);
}

#[test]
fn an_entry_that_does_not_parse_is_left_where_it_is() {
    // A file half-edited by hand. The line that cannot be read is not this
    // module's to throw away, and the entry that can be read is still found.
    let file = "[\n    { \"command\": },\n    { \"command\": \"a/b/c\" }\n]\n";
    let mut document = Document::new(file);

    assert_eq!(document.entries().len(), 1);
    assert_eq!(document.remove(|_| true), 1);
    assert_eq!(document.text(), "[\n    { \"command\": }\n]\n");
}

#[test]
fn an_array_is_written_under_a_file_that_is_nothing_but_comments() {
    // The state VSCode's own keybindings file is created in, and what is left
    // of any file whose array somebody deleted. The prose stays.
    let mut document = Document::new("// Put your keybindings in here.\n");
    assert!(document.append(r#"{ "command": "a/b/c" }"#));

    assert_eq!(
        document.text(),
        "// Put your keybindings in here.\n[\n    { \"command\": \"a/b/c\" }\n]\n"
    );
    assert_eq!(document.entries().len(), 1);
}

#[test]
fn a_file_that_is_not_a_list_is_refused_rather_than_replaced() {
    // Somebody's settings pasted into the wrong file, or a bracket lost. Both
    // are files this cannot edit, and neither is a file it may overwrite.
    for text in [
        "{ \"key\": \"ctrl+t\" }\n",
        "[\n    { \"command\": \"a/b/c\" }\n",
    ] {
        let mut document = Document::new(text);
        assert!(!document.append(r#"{ "command": "a/b/d" }"#));
        assert_eq!(document.text(), text);
    }
}

#[test]
fn a_comment_between_the_entries_is_not_taken_for_one() {
    let file = "[\n    { \"command\": \"a/b/c\" }\n    // Nothing below here yet.\n]\n";
    let mut document = Document::new(file);

    assert_eq!(document.entries().len(), 1);
    document.append(r#"{ "command": "a/b/d" }"#);
    assert!(serde_json::from_str::<Value>(&strip_comments(document.text())).is_ok());
    assert_eq!(document.entries().len(), 2);
}
