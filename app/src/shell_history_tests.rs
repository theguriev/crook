use super::*;

#[test]
fn test_a_zsh_history_is_read_with_and_without_its_stamps() {
    // Both shapes appear in one file: `EXTENDED_HISTORY` turned on part-way
    // through a life leaves everything before it bare.
    let text = "cargo build\n: 1699999999:0;git status\n: 1700000000:12;cargo test --locked\n";

    assert_eq!(
        parse(Shell::Zsh, text),
        vec!["cargo build", "git status", "cargo test --locked"]
    );
}

#[test]
fn test_a_zsh_line_that_only_looks_like_a_stamp_is_kept_whole() {
    // `: ` is a command, and a person who ran one must not have it eaten.
    let text = ": not a stamp; really\n";

    assert_eq!(parse(Shell::Zsh, text), vec![": not a stamp; really"]);
}

#[test]
fn test_a_bash_timestamp_is_not_a_command() {
    let text = "#1699999999\ngit push\n#1700000000\ncargo fmt\n";

    assert_eq!(parse(Shell::Bash, text), vec!["git push", "cargo fmt"]);
}

#[test]
fn test_a_fish_record_is_one_command() {
    // The `- cmd:` line is the command; everything else in a record is
    // indented under it. A backslash the command itself carried is written
    // doubled, and a newline in a command is written `\n`.
    let text = concat!(
        "- cmd: grep -r 'a\\\\b' .\n",
        "  when: 1700000000\n",
        "  paths:\n",
        "    - src\n",
        "- cmd: echo one\\ntwo\n",
        "  when: 1700000001\n",
    );

    assert_eq!(
        parse(Shell::Fish, text),
        vec!["grep -r 'a\\b' .", "echo one"],
        "the second command has a newline in it and is read as its first line"
    );
}

#[test]
fn test_a_command_run_twice_is_kept_where_it_was_run_last() {
    // What makes the Up key agree with a person's sense of recency.
    let text = "cargo test\ngit status\ncargo test\n";

    assert_eq!(parse(Shell::Bash, text), vec!["git status", "cargo test"]);
}

#[test]
fn test_blank_lines_are_not_commands() {
    assert!(parse(Shell::Bash, "\n\n   \n").is_empty());
    assert!(parse(Shell::Other, "cargo build\n").is_empty());
}

#[test]
fn test_no_more_than_the_limit_is_kept() {
    let text: String = (0..LIMIT + 50).map(|n| format!("command {n}\n")).collect();
    let history = parse(Shell::Bash, &text);

    assert_eq!(history.len(), LIMIT);
    assert_eq!(
        history.last().map(String::as_str),
        Some(format!("command {}", LIMIT + 49).as_str()),
        "the newest end is the end that is kept"
    );
}
