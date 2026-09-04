use super::*;

fn completions(candidates: &[&str]) -> Completions {
    Completions {
        candidates: candidates.iter().map(|c| (*c).to_owned()).collect(),
        truncated: false,
    }
}

#[test]
fn test_one_candidate_is_inserted_whole() {
    let answer = completions(&["Cargo.toml"]);

    assert_eq!(
        answer.insertion("Car").as_deref(),
        Some("Cargo.toml"),
        "an unambiguous answer is the whole word"
    );
}

#[test]
fn test_several_candidates_insert_as_much_as_they_agree_on() {
    // What every shell's Tab does: type as much as is certain and stop.
    let answer = completions(&["tests/", "terminal.rs", "temporary"]);
    assert_eq!(answer.common_prefix(), "te");
    assert_eq!(answer.insertion("t").as_deref(), Some("te"));

    // And one candidate that shares less pulls the whole answer back to it.
    let wider = completions(&["tests/", "terminal.rs", "target/"]);
    assert_eq!(wider.common_prefix(), "t");

    let closer = completions(&["Cargo.toml", "Cargo.lock"]);
    assert_eq!(closer.insertion("C").as_deref(), Some("Cargo."));
}

#[test]
fn test_an_answer_that_adds_nothing_inserts_nothing() {
    // The moment the list itself is the only useful thing left: pressing Tab
    // again must not retype what is already there.
    let answer = completions(&["src/", "settings.json"]);

    assert_eq!(answer.insertion("s"), None, "`s` is already the prefix");
    assert_eq!(completions(&[]).insertion("s"), None);
}

#[test]
fn test_a_candidate_that_does_not_extend_the_word_is_refused() {
    // A shell that answered with something else entirely — a bug in a
    // completion script, a `complete -C` that rewrote the word — must not have
    // its answer spliced in behind the caret.
    let answer = completions(&["something-else"]);

    assert_eq!(answer.insertion("src"), None);
}

#[test]
fn test_a_prefix_never_cuts_a_character_in_half() {
    // Measured in characters, not bytes. Two names sharing the first byte of a
    // multi-byte character share none of it.
    let answer = completions(&["日本.txt", "日記.txt"]);

    assert_eq!(answer.common_prefix(), "日");
}

#[test]
fn test_the_word_a_completion_replaces_is_the_one_after_the_last_space() {
    assert_eq!(word_at_end("cargo te"), "te");
    assert_eq!(word_at_end("cargo"), "cargo");
    assert_eq!(word_at_end("cargo "), "");
    assert_eq!(word_at_end("cat a\tb"), "b");
    assert_eq!(word_at_end(""), "");
}

#[test]
fn test_an_answer_is_one_candidate_per_line_with_the_blanks_dropped() {
    // A shell that found nothing prints one empty line — that is what
    // `printf '%s\n' ${empty[@]}` does — and an empty candidate would insert
    // nothing and list as a blank row.
    assert_eq!(parse_answer("").candidates, Vec::<String>::new());
    assert_eq!(parse_answer("\n").candidates, Vec::<String>::new());
    assert_eq!(
        parse_answer("src/\ntests/\n").candidates,
        vec!["src/".to_owned(), "tests/".to_owned()]
    );

    // A trailing `\r` from a shell on Windows, and a description fish would
    // have stripped but a hand-written answer might not.
    assert_eq!(parse_answer("src/\r\n").candidates, vec!["src/".to_owned()]);
}

#[test]
fn test_a_thousand_candidates_come_back_bounded_and_say_so() {
    // `compgen -c` on a full PATH is thousands of entries, and nobody reads a
    // thousand of anything. What the caller shows is that the list is too
    // long, which needs a bounded list rather than all of it.
    let text: String = (0..MAX_CANDIDATES * 4)
        .map(|index| format!("candidate-{index}\n"))
        .collect();
    let answer = parse_answer(&text);

    assert_eq!(answer.candidates.len(), MAX_CANDIDATES);
    assert!(answer.truncated);
    assert!(!parse_answer("one\ntwo\n").truncated);
}

#[test]
fn test_a_request_is_its_number_and_then_the_line() {
    // The line is last because it can hold newlines: a command being composed
    // in Crook's field really can span several, and there is nothing after it
    // that a reader would have to find again.
    assert_eq!(request_text(7, "cargo te"), "7\ncargo te");
    assert_eq!(request_text(1, "echo 'a\nb"), "1\necho 'a\nb");
}

#[test]
fn test_an_answer_that_is_not_there_is_not_an_error() {
    // The ordinary case for a shell with no integration: it never bound the
    // key, so nothing was ever written.
    let missing = std::env::temp_dir().join("crook-completion-that-does-not-exist");
    let _ = std::fs::remove_file(&missing);

    assert_eq!(read_answer(&missing), None);
}

#[test]
fn test_an_answer_on_disk_comes_back() {
    let directory = std::env::temp_dir().join(format!("crook-completion-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("writable");
    let path = directory.join("complete.out");
    std::fs::write(&path, "src/\ntests/\n").expect("writable");

    let answer = read_answer(&path).expect("the file is there");
    assert_eq!(answer.candidates.len(), 2);
    assert_eq!(answer.insertion("").as_deref(), None, "`s` is not shared");
}
