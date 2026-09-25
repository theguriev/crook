use std::fs;

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
fn test_a_multi_line_zsh_entry_is_one_command() {
    // zsh writes a newline inside a command as a backslash ending the line.
    // The loop is one command, read as its first line; its body and its
    // `done` are not commands anybody ran.
    let text = concat!(
        ": 1700000000:0;for f in a b; do\\\n",
        "echo $f\\\n",
        "done\n",
        ": 1700000001:0;git status\n",
        "printf 'bare\\n'\\\n",
        "  | wc -l\n",
    );

    assert_eq!(
        parse(Shell::Zsh, text),
        vec!["for f in a b; do", "git status", "printf 'bare\\n'"]
    );
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

/// `text` as zsh writes it: every byte from `0x83` to `0xa2`, and NUL, as
/// `0x83` and the byte XOR `0x20`.
fn metafied(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for byte in text.bytes() {
        if byte == 0 || (0x83..=0xa2).contains(&byte) {
            bytes.extend([0x83, byte ^ 0x20]);
        } else {
            bytes.push(byte);
        }
    }
    bytes
}

#[test]
fn test_a_zsh_history_is_read_back_the_way_zsh_wrote_it() {
    // A dash, an emoji and Cyrillic, all three of which have a byte zsh
    // metafies; read straight as UTF-8 each came back as something else.
    let commands = [
        "git commit -m 'a \u{2014} b'",
        "echo \u{1f680}",
        "cd \u{43f}\u{440}\u{43e}\u{435}\u{43a}\u{442}\u{44b}",
    ];
    let mut text = Vec::new();
    for (at, command) in commands.iter().enumerate() {
        text.extend(format!(": 170000000{at}:0;").bytes());
        text.extend(metafied(command));
        text.push(b'\n');
    }
    assert_ne!(
        String::from_utf8_lossy(&text),
        commands.map(|command| format!("{command}\n")).concat(),
        "nothing here is metafied, so this reads nothing"
    );

    let path = std::env::temp_dir().join(format!("crook-zsh-history-{}", std::process::id()));
    fs::write(&path, &text).expect("the history file is written");
    let read = tail(&path, Shell::Zsh).expect("the history file is read");
    let _ = fs::remove_file(&path);

    assert_eq!(parse(Shell::Zsh, &read), commands);
}

#[test]
fn test_only_zsh_is_unmetafied() {
    // bash writes what it was given, and a byte of 0x83 in its file is a
    // byte of somebody's command.
    let bytes = vec![b'a', 0x83, 0xb4, b'b'];
    assert_eq!(unmetafy(bytes.clone()), [b'a', 0x94, b'b']);

    let path = std::env::temp_dir().join(format!("crook-bash-history-{}", std::process::id()));
    fs::write(&path, &bytes).expect("the history file is written");
    let read = tail(&path, Shell::Bash).expect("the history file is read");
    let _ = fs::remove_file(&path);
    assert_eq!(read, String::from_utf8_lossy(&bytes));
}
