use super::*;

#[test]
fn test_the_default_shell_names_something_that_can_actually_be_run() {
    let shell = default_shell();
    assert!(!shell.is_empty(), "there is always a shell to fall back to");
    // On Unix this is the whole point of the resolution: $SHELL is absent from
    // anything a desktop launches rather than a shell, and it outlives the
    // shell it names when a package is removed. Either way the answer has to be
    // a file that exists, because the alternative is a pane that will not open.
    if !cfg!(windows) {
        assert!(
            Path::new(&shell).is_file(),
            "{shell:?} is what a pane would try to spawn"
        );
    }
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

#[test]
fn test_the_shells_crook_knows_how_to_ask_for_a_login_shell() {
    for shell in ["/bin/zsh", "/bin/bash", "/opt/homebrew/bin/fish"] {
        assert_eq!(
            login_arguments(Path::new(shell)),
            vec![OsString::from("-l")],
            "{shell} reads the files that hold a person's PATH only as a login shell"
        );
    }
    // `file_stem` drops the extension, so a Windows build of one of them is
    // still one of them.
    assert_eq!(
        login_arguments(Path::new("bash.exe")),
        vec![OsString::from("-l")]
    );
}

#[test]
fn test_a_shell_crook_does_not_know_is_not_handed_an_option_it_may_not_have() {
    for shell in [
        "/bin/sh",
        "/bin/dash",
        "/usr/bin/nu",
        "/bin/tcsh",
        "/bin/csh",
    ] {
        assert!(
            login_arguments(Path::new(shell)).is_empty(),
            "{shell} would have to be started to find out whether it takes -l, and \
             the cost of guessing wrong is a pane with no shell in it — tcsh answers \
             `Unknown option: -l' unless -l is the only argument on the line"
        );
    }
    // Windows has no login shell at all: PATH is in the registry and $PROFILE
    // is read by every interactive PowerShell.
    for shell in ["powershell.exe", "cmd.exe", "pwsh.exe"] {
        assert!(login_arguments(Path::new(shell)).is_empty());
    }
}

#[test]
fn test_the_default_program_is_the_users_shell_started_the_way_a_terminal_starts_one() {
    let shell = default_shell();
    let command = command_for(&Program::Shell, &[]);
    let argv = command.get_argv();
    let arguments = login_arguments(Path::new(&shell));

    if arguments.is_empty() {
        // The argv[0] convention, which `portable-pty` offers only through a
        // builder that takes no arguments and names the shell itself, out of
        // the SHELL it is going to hand the child.
        assert!(argv.is_empty(), "a builder with an argv is not that one");
        assert_eq!(
            command.get_env("SHELL"),
            Some(OsStr::new(&shell)),
            "which is how it is told which shell to run"
        );
        return;
    }

    assert_eq!(argv[0], shell, "the shell $SHELL names, and no substitute");
    assert_eq!(
        argv[1..],
        arguments[..],
        "and started the way login(1) would have started it"
    );
}

#[test]
fn test_a_shell_with_no_login_switch_is_started_by_the_convention_login_itself_uses() {
    // tcsh answers ``Unknown option: `-l'``, so a terminal that guessed at a
    // switch would hand this person a pane with nothing in it. argv[0] with a
    // leading hyphen is what `login(1)` does and needs no option parsing at
    // all, which is why it is the one that generalises.
    let command = command_for(&Program::login_shell("/bin/tcsh"), &[]);

    if cfg!(unix) {
        assert!(
            command.get_argv().is_empty(),
            "no arguments, or it is not the argv[0] builder"
        );
        assert_eq!(command.get_env("SHELL"), Some(OsStr::new("/bin/tcsh")));
    }
}

#[test]
fn test_a_shell_crook_knows_a_switch_for_is_named_rather_than_resolved() {
    // The named three are spawned by name because the app has by then written
    // startup files shaped for that shell. A builder that resolved $SHELL for
    // itself could hand a fish the stubs zsh was going to read.
    let command = command_for(&Program::login_shell("/opt/homebrew/bin/fish"), &[]);

    assert_eq!(
        command.get_argv(),
        &[
            OsString::from("/opt/homebrew/bin/fish"),
            OsString::from("-l")
        ]
    );
}

#[test]
fn test_the_child_is_told_which_terminal_it_is_on() {
    let command = command_for(&Program::command("true", Vec::<String>::new()), &[]);

    assert_eq!(
        command.get_env("TERM"),
        Some(OsStr::new(TERM)),
        "a child with no TERM drives the grid with the wrong sequences, or none"
    );
    assert_eq!(
        command.get_env("COLORTERM"),
        Some(OsStr::new("truecolor")),
        "and this is what a program asks before it uses 24-bit colour"
    );
}

#[test]
fn test_the_size_of_whatever_started_crook_does_not_reach_the_child() {
    // Set in the environment the builder is seeded from, which is what a Crook
    // launched from another terminal inherits.
    let command = command_for(
        &Program::command("true", Vec::<String>::new()),
        &[
            ("LINES".to_owned(), "24".to_owned()),
            ("COLUMNS".to_owned(), "80".to_owned()),
        ],
    );

    // They are only here because this test put them there: what is being
    // checked below is that a plain spawn does not carry them.
    assert_eq!(command.get_env("LINES"), Some(OsStr::new("24")));

    let plain = command_for(&Program::command("true", Vec::<String>::new()), &[]);
    assert_eq!(
        plain.get_env("LINES"),
        None,
        "a stale LINES describes the window Crook was started from, and a child \
         that believed it would lay itself out to a size it is not on"
    );
    assert_eq!(plain.get_env("COLUMNS"), None);
}

#[test]
fn test_the_caller_gets_the_last_word_on_the_environment() {
    let command = command_for(
        &Program::command("true", Vec::<String>::new()),
        &[("TERM".to_owned(), "dumb".to_owned())],
    );

    assert_eq!(
        command.get_env("TERM"),
        Some(OsStr::new("dumb")),
        "the shell integration adds its own variables after these, and a caller \
         that cannot override one cannot fix a machine where it is wrong"
    );
}
