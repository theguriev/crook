use super::*;

#[test]
fn test_the_default_shell_names_something() {
    let shell = default_shell();
    assert!(!shell.is_empty(), "there is always a shell to fall back to");
}

#[test]
fn test_a_command_keeps_its_arguments() {
    let program = Program::command("git", ["status", "--short"]);
    assert_eq!(
        Program::Command {
            program: OsString::from("git"),
            args: vec![OsString::from("status"), OsString::from("--short")],
        },
        program
    );
}

#[test]
fn test_the_pty_size_carries_the_pixel_dimensions() {
    let size = TerminalSize::new(80, 24).with_cell_size(9, 20);
    let pty = pty_size(size);

    assert_eq!(80, pty.cols);
    assert_eq!(24, pty.rows);
    assert_eq!(720, pty.pixel_width);
    assert_eq!(480, pty.pixel_height);
}

#[test]
fn test_a_zero_sized_grid_still_opens_a_pty() {
    // A window can be laid out before it has a font, and a pty of zero columns
    // is rejected by the kernel.
    let pty = pty_size(TerminalSize::new(0, 0));

    assert_eq!(1, pty.cols);
    assert_eq!(1, pty.rows);
}

#[test]
fn test_an_exit_status_reads_as_english() {
    assert!(ChildExit::from_code(0).success());
    assert!(!ChildExit::from_code(1).success());
    assert_eq!("exited with code 2", ChildExit::from_code(2).to_string());

    let killed = ChildExit {
        code: 1,
        signal: Some("Terminated".to_owned()),
    };
    assert!(!killed.success());
    assert_eq!("terminated by Terminated", killed.to_string());
}
