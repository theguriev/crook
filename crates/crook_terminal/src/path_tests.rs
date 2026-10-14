use super::*;

/// The path under a column, with the line and column it names, as text.
fn found(text: &str, column: usize) -> Option<(String, Option<u32>, Option<u32>)> {
    at(text, column).map(|path| (path.path, path.line, path.column))
}

/// The cells a path covers, location and all.
fn span_at(text: &str, column: usize) -> Option<(usize, usize)> {
    at(text, column).map(|path| (path.start, path.len))
}

/// What [`found`] answers for a path printed as `printed`, at `line` and
/// `column`.
fn path(
    printed: &str,
    line: Option<u32>,
    column: Option<u32>,
) -> (String, Option<u32>, Option<u32>) {
    (printed.to_owned(), line, column)
}

#[test]
fn test_the_forms_a_compiler_prints_name_a_line_and_a_column() {
    for (row, expected) in [
        // rustc, gcc, clang, go, tsc --pretty: the colon form.
        (
            " --> src/main.rs:42:5",
            path("src/main.rs", Some(42), Some(5)),
        ),
        ("src/main.rs:42", path("src/main.rs", Some(42), None)),
        // A panic, and go, end the location with a colon of their own.
        (
            "panicked at src/lib.rs:7:13:",
            path("src/lib.rs", Some(7), Some(13)),
        ),
        (
            "./main.go:12:5: undefined: x",
            path("./main.go", Some(12), Some(5)),
        ),
        // grep -n: the line, then the text that is on it.
        ("src/a.rs:42:use std;", path("src/a.rs", Some(42), None)),
        // MSVC, tsc and dotnet: the bracket form.
        (
            "src/app.ts(42,5): error TS2304",
            path("src/app.ts", Some(42), Some(5)),
        ),
        ("src/app.ts(42): error", path("src/app.ts", Some(42), None)),
        // No location at all.
        ("./README.md", path("./README.md", None, None)),
        ("/etc/hosts", path("/etc/hosts", None, None)),
        ("~/notes.md", path("~/notes.md", None, None)),
        ("Cargo.toml", path("Cargo.toml", None, None)),
        ("../crook/README.md", path("../crook/README.md", None, None)),
    ] {
        // From a cell inside the path itself; every row here is ASCII, so a
        // byte offset is a cell.
        let column = row
            .find(expected.0.as_str())
            .expect("the path is in the row")
            + 1;
        assert_eq!(found(row, column), Some(expected.clone()), "{row:?}");
    }
}

#[test]
fn test_a_path_is_found_from_every_cell_of_it_and_its_location() {
    let row = "error at app/src/view.rs:3712:9 here";
    let start = 9;
    let printed = "app/src/view.rs:3712:9";
    for column in start..start + printed.len() {
        assert_eq!(
            span_at(row, column),
            Some((start, printed.len())),
            "column {column}"
        );
    }
    assert_eq!(span_at(row, start - 1), None, "the space before it");
    assert_eq!(
        span_at(row, start + printed.len()),
        None,
        "the space after it"
    );
    assert_eq!(span_at(row, 2), None, "the word before it");
}

#[test]
fn test_the_bracket_form_is_found_from_the_bracket_too() {
    // The bracket ends a path, so the location is a run of its own; a pointer
    // on it still means the path it is about.
    let row = "src/app.ts(42,5): error";
    let whole = Some((0, "src/app.ts(42,5)".len()));
    for column in 10..16 {
        assert_eq!(span_at(row, column), whole, "column {column}");
    }
    assert_eq!(span_at(row, 16), None, "the colon after it");
}

#[test]
fn test_the_text_after_a_grep_line_is_not_part_of_the_link() {
    let row = "src/a.rs:42:use std;";
    assert_eq!(span_at(row, 2), Some((0, "src/a.rs:42".len())));
    assert_eq!(span_at(row, 14), None, "the text on the line");
}

#[test]
fn test_an_agent_names_a_path_inside_brackets() {
    // Claude Code: `Update(app/src/workspace/view.rs)`. The bracket is the
    // tool call's, never the path's.
    let row = "⏺ Update(app/src/workspace/view.rs)";
    assert_eq!(
        found(row, 12),
        Some(path("app/src/workspace/view.rs", None, None))
    );
    assert_eq!(found(row, 4), None, "the tool's name is not a path");
    // Node's stack frames put theirs in round brackets as well.
    let row = "    at main (/home/me/app.js:12:5)";
    assert_eq!(
        found(row, 16),
        Some(path("/home/me/app.js", Some(12), Some(5)))
    );
}

#[test]
fn test_a_name_inside_an_agents_bracket_may_start_with_digits() {
    // The digits after the tool's bracket look like the start of a `(42,5)`
    // location until the name carries on past them. The pointer on them is on
    // the file, like the pointer on the rest of its name.
    for (row, name) in [
        ("Read(2026-10-14-notes.md)", "2026-10-14-notes.md"),
        (
            "⏺ Write(20240101_create_users.rb)",
            "20240101_create_users.rb",
        ),
        ("Update(01_intro.md)", "01_intro.md"),
    ] {
        let cells: Vec<char> = row.chars().collect();
        let start = cells
            .iter()
            .position(|cell| *cell == '(')
            .expect("a bracket")
            + 1;
        let len = name.chars().count();
        for column in start..start + len {
            assert_eq!(
                found(row, column),
                Some(path(name, None, None)),
                "{row:?} at {column}"
            );
            assert_eq!(span_at(row, column), Some((start, len)));
        }
        assert_eq!(found(row, start - 2), None, "{row:?}: the tool's name");
    }
    // A whole location after a bracket still belongs to the path before it.
    assert_eq!(
        found("src/app.ts(42,5)", 12),
        Some(path("src/app.ts", Some(42), Some(5)))
    );
}

#[test]
fn test_a_path_that_ends_a_sentence_does_not_take_the_full_stop() {
    for (row, expected) in [
        ("see src/main.rs.", path("src/main.rs", None, None)),
        ("see src/main.rs:12.", path("src/main.rs", Some(12), None)),
        ("in src/a.rs, src/b.rs", path("src/a.rs", None, None)),
        ("is it src/main.rs?", path("src/main.rs", None, None)),
    ] {
        let column = row.find("src").expect("a path") + 1;
        assert_eq!(found(row, column), Some(expected), "{row:?}");
    }
}

#[test]
fn test_quotes_end_a_path() {
    let row = r#"  File "/usr/lib/python3/x.py", line 12"#;
    assert_eq!(
        found(row, 12),
        Some(path("/usr/lib/python3/x.py", None, None))
    );
    assert_eq!(found("'src/a.rs'", 3), Some(path("src/a.rs", None, None)));
    assert_eq!(found("`src/a.rs`", 3), Some(path("src/a.rs", None, None)));
}

#[test]
fn test_a_colon_that_is_part_of_the_path_is_not_a_line() {
    // Only a number that ends the run, or that a colon follows, is a line.
    assert_eq!(
        found("/tmp/12:30/log.txt", 3),
        Some(path("/tmp/12:30/log.txt", None, None))
    );
}

#[test]
fn test_a_windows_path_keeps_its_drive_and_its_backslashes() {
    assert_eq!(
        found(r"--> src\main.rs:2:5", 6),
        Some(path(r"src\main.rs", Some(2), Some(5)))
    );
    assert_eq!(
        found(r"C:\Users\me\app.rs:7", 5),
        Some(path(r"C:\Users\me\app.rs", Some(7), None))
    );
}

#[test]
fn test_words_that_only_look_like_one_are_not_paths() {
    for (row, column) in [
        ("a plain word", 3),
        ("crook::process::command", 5),
        ("version v0.1.24", 10),
        ("at 12:30:15", 5),
        ("127.0.0.1:8080", 3),
        ("3 / 4", 2),
        ("~/", 0),
        ("...", 1),
        ("Vec<String>", 5),
    ] {
        assert_eq!(found(row, column), None, "{row:?} at {column}");
    }
}

#[test]
fn test_a_url_is_left_to_the_url_scan() {
    // Whatever has a scheme is a URL or nothing: `sftp://host/a.tar` is not a
    // file in the directory the command ran in.
    assert_eq!(found("https://example.com/a.rs", 12), None);
    assert_eq!(found("sftp://files.example.com/a.tar", 12), None);
}

#[test]
fn test_a_path_with_wide_characters_keeps_every_cell() {
    // The caller fills a wide character's second column with a stand-in that
    // is not a terminator, as it does for a URL; it is part of the path.
    let row = "docs/東\u{e000}京\u{e000}.md:3";
    let printed = at(row, 6).expect("the path is found");
    assert_eq!(printed.path, "docs/東\u{e000}京\u{e000}.md");
    assert_eq!(printed.line, Some(3));
    assert_eq!((printed.start, printed.len), (0, row.chars().count()));
}

#[test]
fn test_full_width_punctuation_ends_a_path() {
    let row = "见 src/main.rs。\u{e000}";
    assert_eq!(found(row, 4), Some(path("src/main.rs", None, None)));
}

#[test]
fn test_a_line_number_too_long_to_be_one_is_no_location() {
    assert_eq!(
        found("src/a.rs:99999999999999", 2),
        Some(path("src/a.rs:99999999999999", None, None))
    );
}
