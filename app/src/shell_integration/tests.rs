use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crook_terminal::{Program, PtyReader, Terminal, TerminalOptions, TerminalSize};

use super::launch::{HostEnv, Launch, plain, plan};
use super::scratch::sweep;
use super::*;

/// How long a shell that should reach its first prompt immediately is given
/// before the test calls it a failure rather than hanging the suite.
const DEADLINE: Duration = Duration::from_secs(30);

/// A directory that removes itself. The workspace has no `tempfile`, and one
/// struct is cheaper than the dependency.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir()
            .join("crook-shell-integration-tests")
            .join(format!(
                "{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path).expect("a temporary directory should be creatable");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A host with a home directory and nothing else set, which is what most
/// machines look like.
fn host(home: &str) -> HostEnv {
    HostEnv {
        home: Some(PathBuf::from(home)),
        zdotdir: None,
        xdg_data_dirs: None,
    }
}

/// The arguments a launch would start its shell with.
fn arguments(launch: &Launch) -> Vec<String> {
    match &launch.program {
        Program::Command { args, .. } => args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
        Program::Shell => Vec::new(),
    }
}

/// One environment variable a launch adds.
fn variable<'a>(launch: &'a Launch, key: &str) -> Option<&'a str> {
    launch
        .environment
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// The contents of the one scratch file whose path ends with `name`.
fn file<'a>(launch: &'a Launch, name: &str) -> &'a str {
    launch
        .files
        .iter()
        .find(|file| file.path.ends_with(name))
        .map(|file| file.contents.as_str())
        .unwrap_or_else(|| panic!("the launch should write {name}, got {:?}", paths(launch)))
}

/// Every file a launch writes, as strings, for a failure message.
fn paths(launch: &Launch) -> Vec<String> {
    launch
        .files
        .iter()
        .map(|file| file.path.display().to_string())
        .collect()
}

#[test]
fn test_shell_detection_reads_the_programs_name() {
    assert_eq!(Shell::of(Path::new("/bin/zsh")), Shell::Zsh);
    assert_eq!(Shell::of(Path::new("/usr/local/bin/bash")), Shell::Bash);
    assert_eq!(Shell::of(Path::new("/opt/homebrew/bin/fish")), Shell::Fish);
    // `file_stem` drops the extension, so a Windows bash is still bash.
    assert_eq!(Shell::of(Path::new("bash.exe")), Shell::Bash);
    assert_eq!(
        Shell::of(Path::new("/bin/sh")),
        Shell::Other,
        "a name that does not say which shell it is gets no injection"
    );
    assert_eq!(Shell::of(Path::new("/usr/bin/nu")), Shell::Other);
    assert_eq!(Shell::of(Path::new("/usr/bin/xonsh")), Shell::Other);
}

#[test]
fn test_zsh_is_launched_with_a_scratch_zdotdir_and_no_arguments() {
    let launch = plan(
        Shell::Zsh,
        Path::new("/bin/zsh"),
        Path::new("/scratch"),
        &host("/home/person"),
    );

    assert!(launch.marks());
    assert_eq!(
        launch.program,
        Program::command("/bin/zsh", Vec::<String>::new()),
        "zsh has no --rcfile, so the whole strategy rides on ZDOTDIR"
    );
    assert_eq!(variable(&launch, "ZDOTDIR"), Some("/scratch"));
    assert_eq!(variable(&launch, "USER_ZDOTDIR"), Some("/home/person"));
    assert_eq!(variable(&launch, "TERM_PROGRAM"), Some(TERM_PROGRAM));
    assert_eq!(variable(&launch, "TERM_PROGRAM_VERSION"), Some(VERSION));

    for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin", "crook.zsh"] {
        assert!(
            !file(&launch, name).is_empty(),
            "{name} should have been written"
        );
    }
}

#[test]
fn test_zsh_points_user_zdotdir_at_a_zdotdir_the_user_already_had() {
    let launch = plan(
        Shell::Zsh,
        Path::new("/bin/zsh"),
        Path::new("/scratch"),
        &HostEnv {
            home: Some(PathBuf::from("/home/person")),
            zdotdir: Some(PathBuf::from("/home/person/.config/zsh")),
            xdg_data_dirs: None,
        },
    );

    assert_eq!(
        variable(&launch, "USER_ZDOTDIR"),
        Some("/home/person/.config/zsh"),
        "the stubs have to source the files the user actually uses"
    );
}

#[test]
fn test_zsh_with_nowhere_to_point_back_at_gets_no_injection() {
    let launch = plan(
        Shell::Zsh,
        Path::new("/bin/zsh"),
        Path::new("/scratch"),
        &HostEnv::default(),
    );

    assert!(
        !launch.marks(),
        "a ZDOTDIR of stubs that source nothing is a shell with none of the \
         user's configuration, which is worse than a shell with no marks"
    );
    assert_eq!(launch, plain(Path::new("/bin/zsh")));
}

#[test]
fn test_the_zsh_stubs_source_the_users_real_files() {
    let launch = plan(
        Shell::Zsh,
        Path::new("/bin/zsh"),
        Path::new("/scratch"),
        &host("/home/person"),
    );

    for (stub, real) in [
        (".zshenv", "$USER_ZDOTDIR/.zshenv"),
        (".zprofile", "$USER_ZDOTDIR/.zprofile"),
        (".zshrc", "$USER_ZDOTDIR/.zshrc"),
        (".zlogin", "$USER_ZDOTDIR/.zlogin"),
    ] {
        assert!(
            file(&launch, stub).contains(&format!(". {real}")),
            "the {stub} stub should source {real}"
        );
    }

    let zshrc = file(&launch, ".zshrc");
    let user = zshrc
        .find("$USER_ZDOTDIR/.zshrc")
        .expect("the stub sources the user's rc");
    let integration = zshrc
        .find("crook.zsh")
        .expect("the stub sources the integration");
    assert!(
        user < integration,
        "the user's configuration has to load first, or a framework that \
         assigns precmd_functions wholesale wipes the hooks"
    );
    assert!(
        zshrc.trim_end().ends_with("ZDOTDIR=$USER_ZDOTDIR"),
        "ZDOTDIR must be handed back, since everything after startup reads it \
         expecting the user's directory"
    );
}

#[test]
fn test_bash_is_given_an_rcfile_that_sources_the_users_bashrc_first() {
    let launch = plan(
        Shell::Bash,
        Path::new("/bin/bash"),
        Path::new("/scratch"),
        &host("/home/person"),
    );

    assert!(launch.marks());
    assert_eq!(arguments(&launch), ["--rcfile", "/scratch/bashrc", "-i"]);
    assert_eq!(
        variable(&launch, "ZDOTDIR"),
        None,
        "bash reaches its rc file by argument, not by environment"
    );

    let rc = file(&launch, "bashrc");
    let user = rc
        .find("$HOME/.bashrc")
        .expect("--rcfile suppresses the user's rc, so the stub must source it");
    let integration = rc
        .find("CROOK_SHELL_INTEGRATION")
        .expect("the rc file carries the integration");
    assert!(
        user < integration,
        "putting the integration first would let a prompt framework loaded by \
         ~/.bashrc replace PROMPT_COMMAND out from under it"
    );
    assert_eq!(
        launch.files.len(),
        1,
        "bash 3.2 discards a DEBUG trap installed from a nested source when one \
         was already set, so the integration is inline rather than sourced"
    );
}

#[test]
fn test_fish_prepends_its_directory_to_xdg_data_dirs() {
    let launch = plan(
        Shell::Fish,
        Path::new("/usr/bin/fish"),
        Path::new("/scratch"),
        &host("/home/person"),
    );

    assert!(launch.marks());
    assert!(
        arguments(&launch).is_empty(),
        "fish reads vendor_conf.d on its own; no argument changes are needed"
    );
    assert_eq!(
        variable(&launch, "XDG_DATA_DIRS"),
        Some("/scratch:/usr/local/share:/usr/share"),
        "an unset XDG_DATA_DIRS still means the two default directories, and \
         dropping them would cost the user every other vendor file on the box"
    );
    assert!(
        !file(&launch, "fish/vendor_conf.d/crook.fish").is_empty(),
        "fish only reads this one directory inside a data dir"
    );
}

#[test]
fn test_fish_keeps_the_data_dirs_the_user_already_had() {
    let launch = plan(
        Shell::Fish,
        Path::new("/usr/bin/fish"),
        Path::new("/scratch"),
        &HostEnv {
            home: Some(PathBuf::from("/home/person")),
            zdotdir: None,
            xdg_data_dirs: Some("/opt/share:/usr/share".to_owned()),
        },
    );

    assert_eq!(
        variable(&launch, "XDG_DATA_DIRS"),
        Some("/scratch:/opt/share:/usr/share")
    );
}

#[test]
fn test_an_unrecognised_shell_gets_nothing_added() {
    let launch = plan(
        Shell::Other,
        Path::new("/usr/bin/nu"),
        Path::new("/scratch"),
        &host("/home/person"),
    );

    assert!(!launch.marks());
    assert!(launch.files.is_empty(), "nothing is written for it");
    assert!(arguments(&launch).is_empty());
    assert_eq!(
        launch.program,
        Program::command("/usr/bin/nu", Vec::<String>::new()),
        "the shell the user chose is the shell that runs"
    );
    assert_eq!(
        launch
            .environment
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        ["TERM_PROGRAM", "TERM_PROGRAM_VERSION"],
        "naming the terminal is not injection, and every shell gets it"
    );
}

#[test]
fn test_every_snippet_is_safe_to_source_twice_and_survives_a_re_exec() {
    for shell in [Shell::Zsh, Shell::Bash, Shell::Fish] {
        let snippet = snippet(shell).expect("this shell has an integration");
        assert!(
            snippet.contains("CROOK_SHELL_INTEGRATION"),
            "{shell:?} needs a guard, or sourcing it twice doubles the hooks"
        );
        for exported in [
            "export CROOK_SHELL_INTEGRATION",
            "set -gx CROOK_SHELL_INTEGRATION",
            "set --export CROOK_SHELL_INTEGRATION",
        ] {
            assert!(
                !snippet.contains(exported),
                "{shell:?} must not export its guard: `exec {shell:?}` starts a \
                 shell with no hooks, and an inherited guard would tell it \
                 they were already installed"
            );
        }
    }
}

#[test]
fn test_every_snippet_emits_all_four_marks() {
    // How each shell spells each mark. zsh and bash put A and B in the prompt
    // string, where a mark has to be quoted as zero-width; fish builds its
    // prompt from a function, so it calls the emitter for all four.
    for (shell, marks) in [
        (
            Shell::Zsh,
            [
                "%{\\e]133;A\\a%}",
                "%{\\e]133;B\\a%}",
                "__crook_mark C",
                "__crook_mark \"D;",
            ],
        ),
        (
            Shell::Bash,
            [
                "\\[\\e]133;A\\a\\]",
                "\\[\\e]133;B\\a\\]",
                "__crook_mark C",
                "__crook_mark \"D;",
            ],
        ),
        (
            Shell::Fish,
            [
                "__crook_mark A",
                "__crook_mark B",
                "__crook_mark C",
                "__crook_mark \"D;",
            ],
        ),
    ] {
        let snippet = snippet(shell).expect("this shell has an integration");
        for (mark, edge) in marks.iter().zip([
            "the top of a block",
            "the end of the prompt",
            "the start of the output",
            "the end of the block",
        ]) {
            assert!(
                snippet.contains(mark),
                "{shell:?} should mark {edge}, or a block has no edge there"
            );
        }
        assert!(
            manual_install_file(shell).is_some(),
            "a person pasting {shell:?}'s snippet has to be told where it goes"
        );
    }
    assert_eq!(snippet(Shell::Other), None);
    assert_eq!(manual_install_file(Shell::Other), None);
}

#[test]
fn test_the_prompt_marks_are_wrapped_so_the_shell_does_not_miscount_them() {
    let zsh = snippet(Shell::Zsh).expect("zsh has an integration");
    assert!(
        zsh.contains("%{\\e]133;A\\a%}") && zsh.contains("%{\\e]133;B\\a%}"),
        "zsh needs %{{...%}} around a zero-width stretch of prompt, or every \
         long line the user types wraps in the wrong place"
    );

    let bash = snippet(Shell::Bash).expect("bash has an integration");
    assert!(
        bash.contains("\\[\\e]133;A\\a\\]") && bash.contains("\\[\\e]133;B\\a\\]"),
        "bash needs \\[...\\] around it, for the same reason"
    );
}

#[test]
fn test_the_bash_snippet_asks_which_bash_it_is_before_using_an_array() {
    let bash = snippet(Shell::Bash).expect("bash has an integration");
    assert!(
        bash.contains("BASH_VERSINFO"),
        "a bash older than 5.1 runs only element 0 of an array PROMPT_COMMAND, \
         so installing a chain as an array there leaves the shell with no marks \
         at all"
    );
}

#[test]
fn test_the_fish_snippet_stands_down_where_fish_already_marks() {
    let fish = snippet(Shell::Fish).expect("fish has an integration");
    assert!(
        fish.contains("mark-prompt"),
        "fish 4.0 marks prompts itself, behind that feature flag, and hooks \
         installed on top of it give every block two of every mark"
    );
}

#[test]
fn test_a_session_writes_its_scratch_files_and_takes_them_with_it() {
    let home = TempDir::new("home");
    let options = Options {
        enabled: true,
        // The launch is decided from the name, not from what is on disk, so a
        // shell that is not installed still exercises the whole arrangement.
        shell: Some(PathBuf::from("/nowhere/zsh")),
    };

    let scratch = {
        let session = Session::with_host(&options, host_at(&home));
        assert!(session.marks());
        assert_eq!(session.shell(), Shell::Zsh);

        let scratch = session
            .scratch()
            .expect("a marked session has a scratch directory")
            .to_path_buf();
        assert!(scratch.join(".zshrc").is_file());
        assert!(scratch.join("crook.zsh").is_file());
        scratch
    };

    assert!(
        !scratch.exists(),
        "dropping the session is what removes the directory, and that is the \
         path a closed pane and a quitting app both take"
    );
}

#[test]
fn test_a_session_fills_in_the_terminals_options() {
    let home = TempDir::new("home");
    let session = Session::with_host(
        &Options {
            enabled: true,
            shell: Some(PathBuf::from("/nowhere/bash")),
        },
        host_at(&home),
    );

    let mut options = TerminalOptions {
        environment: vec![("EXISTING".to_owned(), "kept".to_owned())],
        ..TerminalOptions::default()
    };
    session.apply(&mut options);

    assert_eq!(options.program, *session.program());
    assert!(
        options
            .environment
            .iter()
            .any(|(key, value)| key == "EXISTING" && value == "kept"),
        "what the caller already put in the environment stays there"
    );
    assert!(
        options
            .environment
            .iter()
            .any(|(key, _)| key == "TERM_PROGRAM")
    );
}

#[test]
fn test_the_opt_out_leaves_the_shell_exactly_as_it_was() {
    let home = TempDir::new("home");
    let session = Session::with_host(
        &Options {
            enabled: false,
            shell: Some(PathBuf::from("/nowhere/zsh")),
        },
        host_at(&home),
    );

    assert!(!session.marks());
    assert!(session.scratch().is_none(), "and nothing was written");
    assert!(
        session
            .environment()
            .iter()
            .all(|(key, _)| key != "ZDOTDIR")
    );
    assert_eq!(
        *session.program(),
        Program::command("/nowhere/zsh", Vec::<String>::new())
    );
}

#[test]
fn test_the_sweep_removes_directories_no_session_owns_and_leaves_ours() {
    let root = TempDir::new("sweep");
    let ours = root.path().join("9999-0");
    let abandoned = root.path().join("1-0");
    fs::create_dir_all(&ours).expect("a directory should be creatable");
    fs::create_dir_all(&abandoned).expect("a directory should be creatable");

    // Zero, because a test cannot wait a week and cannot backdate a directory
    // without a dependency. What is being checked is which directories the
    // sweep is willing to touch, not the calendar.
    sweep(root.path(), "9999-", Duration::ZERO);

    assert!(ours.exists(), "this process's own directories are in use");
    assert!(
        !abandoned.exists(),
        "a directory left behind by a killed Crook has to be collectable, or \
         the temporary directory grows for ever"
    );
}

/// A host whose zsh files live in a directory the test owns.
fn host_at(home: &TempDir) -> HostEnv {
    HostEnv {
        home: Some(home.path().to_path_buf()),
        zdotdir: None,
        xdg_data_dirs: std::env::var("XDG_DATA_DIRS").ok(),
    }
}

/// Where a shell is on this machine, or `None` when it is not installed — a
/// build container with only `sh` in it is not a failing test.
fn installed(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
}

/// Moves the pty's reader onto a thread, since reading blocks until the child
/// speaks, and posts what it reads back over a channel.
fn read_on_a_thread(mut reader: PtyReader) -> Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                return;
            }
        }
    });
    receiver
}

/// Starts a real shell through this module's launcher, runs one command in it,
/// and returns everything the shell wrote.
///
/// The only test that proves the feature: it asserts on the bytes on the wire,
/// with the user's own shell binary, its own startup files and its own hooks.
/// `None` when the shell is not installed here.
fn run_a_real_shell(name: &str, extra: &[(&str, &str)]) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let program = installed(name)?;
    let home = TempDir::new(name);

    let session = Session::with_host(
        &Options {
            enabled: true,
            shell: Some(program),
        },
        host_at(&home),
    );
    assert!(session.marks(), "{name} should get the integration");

    let mut options = TerminalOptions {
        size: TerminalSize::new(80, 24),
        ..TerminalOptions::default()
    };
    session.apply(&mut options);
    // A home of its own, so the test neither reads the developer's dotfiles
    // nor writes history into them. The shell still starts exactly the way the
    // launcher says it does.
    let home_text = home.path().display().to_string();
    options
        .environment
        .push(("HOME".to_owned(), home_text.clone()));
    options
        .environment
        .push(("XDG_CONFIG_HOME".to_owned(), format!("{home_text}/config")));
    options
        .environment
        .push(("XDG_STATE_HOME".to_owned(), format!("{home_text}/state")));
    for (key, value) in extra {
        options
            .environment
            .push(((*key).to_owned(), (*value).to_owned()));
    }

    let mut terminal = Terminal::spawn(options).expect("a shell should start on a pty");
    let output = read_on_a_thread(
        terminal
            .take_reader()
            .expect("a fresh terminal has its reader"),
    );

    let mut seen = String::new();
    let mut submitted = false;
    let deadline = Instant::now() + DEADLINE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Ok(chunk) = output.recv_timeout(remaining) else {
            break;
        };
        // Fed to the emulator as well as inspected, because the emulator is
        // what answers the questions a shell asks on startup, and one that
        // waits for an answer that never comes never reaches its prompt.
        terminal.feed(&chunk).expect("feeding should not fail");
        seen.push_str(&String::from_utf8_lossy(&chunk));

        if !submitted && seen.contains("\x1b]133;B") {
            submitted = true;
            terminal
                .write(b"printf 'crook-ok\\n'\r")
                .expect("writing to the shell should not fail");
        }
        if seen.contains("\x1b]133;D;0") {
            break;
        }
    }

    let _ = terminal.write(b"exit\r");
    Some(seen)
}

/// Asserts the four marks are all in what a shell wrote.
fn assert_four_marks(name: &str, seen: &str) {
    for (mark, means) in [
        ("\x1b]133;A", "a prompt is about to be drawn"),
        (
            "\x1b]133;B",
            "the prompt ended and the cursor is where the user types",
        ),
        ("\x1b]133;C", "the command was accepted and is about to run"),
        ("\x1b]133;D;0", "it finished, successfully"),
    ] {
        assert!(
            seen.contains(mark),
            "{name} should have said {means}; what it wrote was {seen:?}"
        );
    }
}

#[test]
fn test_a_real_zsh_emits_the_four_marks() {
    let Some(seen) = run_a_real_shell("zsh", &[]) else {
        return;
    };
    assert_four_marks("zsh", &seen);
}

#[test]
fn test_a_real_bash_emits_the_four_marks() {
    let Some(seen) = run_a_real_shell("bash", &[]) else {
        return;
    };
    assert_four_marks("bash", &seen);
}

#[test]
fn test_a_real_fish_emits_the_four_marks() {
    // fish 4.0 marks prompts itself, and the snippet stands down when it does,
    // so the only way to see Crook's own hooks work is to turn fish's off.
    let Some(seen) = run_a_real_shell("fish", &[("fish_features", "no-mark-prompt")]) else {
        return;
    };
    assert_four_marks("fish", &seen);
}

#[test]
fn test_a_real_fish_that_marks_its_own_prompt_is_left_alone() {
    let Some(seen) = run_a_real_shell("fish", &[]) else {
        return;
    };
    if !seen.contains("\x1b]133;") {
        // A fish old enough to have no marks of its own. The test above covers
        // that case; this one has nothing to say about it.
        return;
    }
    assert!(
        !seen.contains("\x1b]133;A\x07"),
        "the snippet terminates its marks with BEL and fish does not, so a BEL \
         mark here means both are marking and every block has two of each; \
         what fish wrote was {seen:?}"
    );
}
