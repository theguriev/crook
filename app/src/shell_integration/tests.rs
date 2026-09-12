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
        Program::Shell | Program::LoginShell { .. } => Vec::new(),
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
fn test_zsh_is_launched_with_a_scratch_zdotdir_and_a_login_switch() {
    let launch = plan(
        Shell::Zsh,
        Path::new("/bin/zsh"),
        Path::new("/scratch"),
        true,
        &host("/home/person"),
    );

    assert!(launch.marks());
    assert_eq!(
        launch.program,
        Program::login_shell("/bin/zsh"),
        "zsh has no --rcfile, so the whole strategy rides on ZDOTDIR — and a \
         login shell is what makes it read /etc/zprofile, which is where \
         path_helper builds PATH"
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
        true,
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
        true,
        &HostEnv::default(),
    );

    assert!(
        !launch.marks(),
        "a ZDOTDIR of stubs that source nothing is a shell with none of the \
         user's configuration, which is worse than a shell with no marks"
    );
    assert_eq!(launch, plain(Path::new("/bin/zsh"), true));
}

#[test]
fn test_the_zsh_stubs_source_the_users_real_files() {
    let launch = plan(
        Shell::Zsh,
        Path::new("/bin/zsh"),
        Path::new("/scratch"),
        true,
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
        zshrc.trim_end().ends_with("__crook_user_zdotdir"),
        "ZDOTDIR must be put back the way it was found, since everything after \
         startup reads it expecting the user's state — zsh itself for .zlogin, \
         and any nested zsh for the `${{ZDOTDIR:-...}}` idiom"
    );
}

#[test]
fn test_bash_is_given_an_rcfile_that_runs_the_users_own_files_first() {
    let launch = plan(
        Shell::Bash,
        Path::new("/bin/bash"),
        Path::new("/scratch"),
        true,
        &host("/home/person"),
    );

    assert!(launch.marks());
    assert_eq!(
        arguments(&launch),
        ["--rcfile", "/scratch/bashrc", "-i"],
        "and no -l: a login bash reads no rc file at all, so asking for both \
         would silently throw the integration away"
    );
    assert_eq!(
        variable(&launch, "ZDOTDIR"),
        None,
        "bash reaches its rc file by argument, not by environment"
    );

    let rc = file(&launch, "bashrc");
    let user = rc
        .find("$HOME/.bash_profile")
        .expect("--rcfile suppresses every file bash would have read, so the stub must run them");
    let integration = rc
        .find("CROOK_SHELL_INTEGRATION")
        .expect("the rc file carries the integration");
    assert!(
        user < integration,
        "putting the integration first would let a prompt framework loaded by \
         the user's own files replace PROMPT_COMMAND out from under it"
    );
    assert_eq!(
        launch.files.len(),
        1,
        "bash 3.2 discards a DEBUG trap installed from a nested source when one \
         was already set, so the integration is inline rather than sourced"
    );
}

#[test]
fn test_the_bash_rc_runs_bashs_own_login_sequence_in_bashs_own_order() {
    let launch = plan(
        Shell::Bash,
        Path::new("/bin/bash"),
        Path::new("/scratch"),
        true,
        &host("/home/person"),
    );
    let rc = file(&launch, "bashrc");

    let at = |needle: &str| {
        rc.find(needle)
            .unwrap_or_else(|| panic!("a login bash reads {needle}; the rc file should too"))
    };
    assert!(
        at("/etc/profile") < at("$HOME/.bash_profile"),
        "bash reads the machine's profile before the person's, and on macOS \
         /etc/profile is where path_helper runs"
    );
    assert!(
        at("$HOME/.bash_profile") < at("$HOME/.bash_login"),
        "and the three personal ones in that order"
    );
    assert!(at("$HOME/.bash_login") < at("$HOME/.profile"));
    assert!(
        rc.contains("break"),
        "bash reads the FIRST of the three and stops; sourcing all three would \
         run a ~/.profile that no login shell of theirs has ever run"
    );

    let fallback = rc
        .rfind("$HOME/.bashrc")
        .expect("the rc file has a fallback for a home with no profile in it");
    assert!(
        fallback > at("$HOME/.profile"),
        "~/.bashrc is the fallback and only the fallback: a login bash does not \
         read it, the profile above almost always does, and sourcing it here as \
         well would install a prompt framework twice"
    );
    assert!(
        rc[..fallback].contains("if [ -z \"$__crook_profile\" ]"),
        "guarded on no profile having been found, or it is that double source"
    );
}

#[test]
fn test_without_the_login_setting_every_shell_starts_the_way_it_used_to() {
    for (shell, program) in [
        (Shell::Zsh, "/bin/zsh"),
        (Shell::Fish, "/usr/bin/fish"),
        (Shell::Other, "/usr/bin/nu"),
    ] {
        let launch = plan(
            shell,
            Path::new(program),
            Path::new("/scratch"),
            false,
            &host("/home/person"),
        );
        assert_eq!(
            launch.program,
            Program::command(program, Vec::<String>::new()),
            "{shell:?} should be started plainly when the setting is off — no \
             switch and no login argv[0]; a person who turned it off has a \
             profile that is not expecting to run"
        );
    }

    let bash = plan(
        Shell::Bash,
        Path::new("/bin/bash"),
        Path::new("/scratch"),
        false,
        &host("/home/person"),
    );
    let rc = file(&bash, "bashrc");
    assert!(
        rc.contains("$HOME/.bashrc"),
        "bash's rc file goes back to the one file a non-login bash reads"
    );
    assert!(
        !rc.contains("/etc/profile"),
        "and stops running the login sequence, which is the whole of what the \
         setting turns off on bash"
    );
}

#[test]
fn test_an_unrecognised_shell_is_a_login_shell_without_being_handed_a_switch() {
    for program in ["/usr/bin/nu", "/bin/tcsh", "/usr/bin/xonsh", "/bin/sh"] {
        let launch = plan(
            Shell::Other,
            Path::new(program),
            Path::new("/scratch"),
            true,
            &host("/home/person"),
        );
        assert_eq!(
            launch.program,
            Program::login_shell(program),
            "{program} would have to be run to find out whether it takes -l — \
             tcsh answers `Unknown option: -l', and a shell that refuses its \
             arguments is a pane with nothing in it. login(1)'s own argv[0] \
             convention needs no option parsing, so these get that instead of \
             going without their profile"
        );
        assert!(
            arguments(&launch).is_empty(),
            "and it is not done by passing an argument"
        );
    }
}

#[test]
fn test_fish_prepends_its_directory_to_xdg_data_dirs() {
    let launch = plan(
        Shell::Fish,
        Path::new("/usr/bin/fish"),
        Path::new("/scratch"),
        true,
        &host("/home/person"),
    );

    assert!(launch.marks());
    assert_eq!(
        launch.program,
        Program::login_shell("/usr/bin/fish"),
        "fish reads vendor_conf.d on its own, so the login shell is here for the \
         other reason: fish's own config.fish runs path_helper only for a login \
         shell"
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
        true,
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
        true,
        &host("/home/person"),
    );

    assert!(!launch.marks());
    assert!(launch.files.is_empty(), "nothing is written for it");
    assert_eq!(
        launch.program,
        Program::login_shell("/usr/bin/nu"),
        "the shell the user chose is the shell that runs, and it is still \
         started the way the desktop starts one"
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
        login: true,
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
            login: true,
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
            login: true,
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
        Program::login_shell("/nowhere/zsh"),
        "the marks are one switch and the startup files are another: turning \
         the integration off does not stop the shell being the user's"
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

/// A real shell, started through this module's launcher, against a home
/// directory the test owns.
///
/// The only kind of test that proves any of this: the user's own shell binary,
/// its own startup rules, its own hooks, and the bytes that actually came back
/// over the pty. Everything configurable about the run is a field, because the
/// questions worth asking differ — one needs `fish_features` set, one needs a
/// `~/.bash_profile` on disk, one needs to know what `$PATH` came out as.
struct RealShell<'a> {
    /// The shell's program name, looked up on `PATH`.
    name: &'a str,
    /// Whether the launcher is asked for a login shell.
    login: bool,
    /// Files to write into the temporary home first, by name.
    home_files: &'a [(&'a str, &'a str)],
    /// Environment variables on top of the launcher's.
    extra: &'a [(&'a str, &'a str)],
    /// The line typed at the first prompt.
    command: &'a str,
}

impl Default for RealShell<'_> {
    /// Every caller names its own shell; the name here is only so that the
    /// other four fields can be defaulted past.
    fn default() -> Self {
        Self {
            name: "zsh",
            login: true,
            home_files: &[],
            extra: &[],
            command: "printf 'crook-ok\\n'",
        }
    }
}

/// What one run came back with.
struct Ran {
    /// Everything the shell wrote, as text.
    seen: String,
    /// The home directory it ran against, kept alive: dropping it removes the
    /// startup files the shell was measured against, and a test that wants to
    /// run the same shell again by hand needs them still there.
    home: TempDir,
}

impl RealShell<'_> {
    /// Runs it, or `None` when this shell is not installed here — a build
    /// container with only `sh` in it is not a failing test.
    fn run(&self) -> Option<Ran> {
        if cfg!(windows) {
            return None;
        }
        let program = installed(self.name)?;
        let home = TempDir::new(self.name);
        for (name, contents) in self.home_files {
            // A name may carry a directory — a ZDOTDIR of the person's own is
            // one of the arrangements worth testing.
            let path = home.path().join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .expect("a directory in a temporary home should be creatable");
            }
            fs::write(path, contents).expect("a file in a temporary home should be writable");
        }

        let session = Session::with_host(
            &Options {
                enabled: true,
                login: self.login,
                shell: Some(program),
            },
            host_at(&home),
        );
        assert!(session.marks(), "{} should get the integration", self.name);

        let mut options = TerminalOptions {
            size: TerminalSize::new(80, 24),
            ..TerminalOptions::default()
        };
        session.apply(&mut options);
        options.environment.extend(home_environment(&home));
        options.environment.extend(
            self.extra
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );

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
                    .write(format!("{}\r", self.command).as_bytes())
                    .expect("writing to the shell should not fail");
            }
            if seen.contains("\x1b]133;D;0") {
                break;
            }
        }

        let _ = terminal.write(b"exit\r");
        Some(Ran { seen, home })
    }
}

/// A home of the test's own, so a run neither reads the developer's dotfiles
/// nor writes history into them. The shell still starts exactly the way the
/// launcher says it does.
fn home_environment(home: &TempDir) -> Vec<(String, String)> {
    let home_text = home.path().display().to_string();
    vec![
        ("HOME".to_owned(), home_text.clone()),
        ("XDG_CONFIG_HOME".to_owned(), format!("{home_text}/config")),
        ("XDG_STATE_HOME".to_owned(), format!("{home_text}/state")),
    ]
}

/// [`RealShell`] with only its shell named, which is what the mark tests want.
fn run_a_real_shell(name: &str, extra: &[(&str, &str)]) -> Option<String> {
    RealShell {
        name,
        extra,
        ..RealShell::default()
    }
    .run()
    .map(|ran| ran.seen)
}

/// What one line the shell printed said, out of everything it wrote.
///
/// The line is wrapped in a sentinel because the shell echoes the command that
/// produced it: the echo carries the `%s` and the output carries the value, so
/// the format string is what tells them apart. The raw stream is read rather
/// than the grid, so a value longer than the terminal is whole rather than
/// wrapped.
fn printed(seen: &str, label: &str) -> Option<String> {
    let opening = format!("{label}[");
    seen.match_indices(&opening)
        .filter_map(|(at, _)| {
            let rest = &seen[at + opening.len()..];
            rest.find(']').map(|end| rest[..end].to_owned())
        })
        .find(|value| !value.contains("%s"))
}

/// What the same shell, started by hand as a login shell, comes out with.
///
/// The reference the whole feature is measured against: not "did Crook pass
/// -l", but "is the environment inside Crook the environment the person gets
/// from their own terminal". Everything is held equal but the launcher — the
/// same binary, the same `HOME`, the same inherited `PATH` — and `ZDOTDIR` is
/// cleared because a developer who has one set would otherwise be comparing
/// their own zsh files against the test's.
fn by_hand(program: &Path, home: &TempDir, line: &str) -> Option<String> {
    let mut command = crate::process::command(&program.to_string_lossy());
    command.args(["-l", "-i", "-c", line]);
    command.env_remove("ZDOTDIR");
    command.env_remove("CROOK_SCRATCH");
    for (key, value) in home_environment(home) {
        command.env(key, value);
    }
    let output = command.output().ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The one line that prints `$PATH`, and the label [`printed`] finds it by.
const PRINT_PATH: &str = "printf 'CROOK_PATH[%s]\\n' \"$PATH\"";

/// See [`PRINT_PATH`].
const PATH_LABEL: &str = "CROOK_PATH";

#[test]
fn test_a_zsh_started_by_crook_has_the_path_a_zsh_started_by_hand_has() {
    let home_files = &[
        // Login-shell files and rc files both, because a PATH that matches on
        // one and not the other is the bug: it is the .zprofile half that a
        // non-login shell silently drops, and on macOS /etc/zprofile with it,
        // which is where path_helper builds PATH out of /etc/paths at all.
        (".zprofile", "export PATH=\"/crook-zprofile:$PATH\"\n"),
        (".zshrc", "export PATH=\"/crook-zshrc:$PATH\"\n"),
    ];
    let Some(ran) = (RealShell {
        name: "zsh",
        home_files,
        command: PRINT_PATH,
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    let inside = printed(&ran.seen, PATH_LABEL).unwrap_or_else(|| {
        panic!(
            "the shell should have printed its PATH, wrote {:?}",
            ran.seen
        )
    });
    let outside = by_hand(
        &installed("zsh").expect("zsh is installed"),
        &ran.home,
        PRINT_PATH,
    )
    .and_then(|out| printed(&out, PATH_LABEL))
    .expect("a zsh started by hand should print its PATH");

    assert!(
        inside.starts_with("/crook-zshrc:/crook-zprofile:"),
        "both halves of the user's own startup have to have run, in order; \
         got {inside:?}"
    );
    assert_eq!(
        inside, outside,
        "a PATH that differs from the one the same shell gives outside Crook is \
         a different set of tools and a different version of each of them, on \
         every machine"
    );
}

#[test]
fn test_a_bash_started_by_crook_has_the_path_a_bash_started_by_hand_has() {
    let home_files = &[
        // The arrangement nearly every ~/.bash_profile on earth has, and the
        // one that makes double-sourcing ~/.bashrc easy to do by accident.
        (
            ".bash_profile",
            "export PATH=\"/crook-bash-profile:$PATH\"\n             [ -f \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n",
        ),
        (".bashrc", "export PATH=\"/crook-bashrc:$PATH\"\n"),
    ];
    let Some(ran) = (RealShell {
        name: "bash",
        home_files,
        command: PRINT_PATH,
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    let inside = printed(&ran.seen, PATH_LABEL).unwrap_or_else(|| {
        panic!(
            "the shell should have printed its PATH, wrote {:?}",
            ran.seen
        )
    });
    let outside = by_hand(
        &installed("bash").expect("bash is installed"),
        &ran.home,
        PRINT_PATH,
    )
    .and_then(|out| printed(&out, PATH_LABEL))
    .expect("a bash started by hand should print its PATH");

    assert!(
        inside.starts_with("/crook-bashrc:/crook-bash-profile:"),
        "bash cannot be given -l and an rc file at once, so its rc file runs \
         bash's login sequence itself — and this is the proof that it does, in \
         the right order; got {inside:?}"
    );
    assert_eq!(
        inside, outside,
        "the emulated login sequence has to land on the same PATH as a real \
         one, or Crook's bash is a bash nobody else has"
    );
}

/// How many times `seen` says a file was sourced.
fn sourced(seen: &str, file: &str) -> usize {
    seen.matches(&format!("[sourced {file}]")).count()
}

/// A line for a startup file to print, so that [`sourced`] can count it.
fn announce(file: &str) -> String {
    format!("printf '[sourced {file}]\\n'\n")
}

#[test]
fn test_a_real_zsh_reads_each_of_the_users_startup_files_exactly_once() {
    let files = [".zshenv", ".zprofile", ".zshrc", ".zlogin"];
    let home_files: Vec<(&str, String)> = files.iter().map(|f| (*f, announce(f))).collect();
    let home_files: Vec<(&str, &str)> = home_files
        .iter()
        .map(|(name, contents)| (*name, contents.as_str()))
        .collect();

    let Some(ran) = (RealShell {
        name: "zsh",
        home_files: &home_files,
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    for file in files {
        assert_eq!(
            sourced(&ran.seen, file),
            1,
            "a login zsh reads all four, and Crook's stubs stand in front of all \
             four — {file} should have been sourced once and was not. Twice is \
             the dangerous answer: a .zshrc run twice is a prompt framework \
             installed twice. What the shell wrote was {:?}",
            ran.seen
        );
    }
}

#[test]
fn test_a_real_bash_reads_the_first_profile_it_finds_and_no_other() {
    let profile = format!(
        "{}[ -f \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n",
        announce(".bash_profile")
    );
    let home_files = [
        (".bash_profile", profile.as_str()),
        (".bash_login", "printf '[sourced .bash_login]\n'\n"),
        (".profile", "printf '[sourced .profile]\n'\n"),
        (".bashrc", "printf '[sourced .bashrc]\n'\n"),
    ];

    let Some(ran) = (RealShell {
        name: "bash",
        home_files: &home_files,
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    assert_eq!(
        sourced(&ran.seen, ".bash_profile"),
        1,
        "what the shell wrote was {:?}",
        ran.seen
    );
    assert_eq!(
        sourced(&ran.seen, ".bashrc"),
        1,
        "the profile sourced it, and Crook must not source it again — that is \
         the double source this whole arrangement is arranged around. What the \
         shell wrote was {:?}",
        ran.seen
    );
    for skipped in [".bash_login", ".profile"] {
        assert_eq!(
            sourced(&ran.seen, skipped),
            0,
            "bash reads the FIRST of the three that exists and stops; {skipped} \
             is one no login shell of theirs has ever run"
        );
    }
}

#[test]
fn test_a_real_bash_with_no_profile_at_all_still_gets_its_bashrc() {
    let home_files = [(".bashrc", "printf '[sourced .bashrc]\n'\n")];
    let Some(ran) = (RealShell {
        name: "bash",
        home_files: &home_files,
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    assert_eq!(
        sourced(&ran.seen, ".bashrc"),
        1,
        "bash's own login path would have read nothing here, and a person on \
         Linux whose terminal has always started a non-login shell has exactly \
         this home. Turning login on must not empty their shell. What it wrote \
         was {:?}",
        ran.seen
    );
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
fn test_a_real_bash_keeps_its_marks_when_the_users_own_files_trap_debug() {
    // The risk the login change introduces on bash, and it is bash 3.2's:
    // the user's files are now reached through ~/.bash_profile, one `source`
    // deeper than before, and 3.2 unwinds a DEBUG trap set inside a nested
    // source when one was already installed. Crook's own trap is set by the rc
    // file itself, after these — but a person's bash-preexec is set from in
    // here, and the C mark is what would quietly disappear.
    let home_files = [
        (
            ".bash_profile",
            "[ -f \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n",
        ),
        (
            ".bashrc",
            "CROOK_TEST_DEBUG=0\n             __crook_test_debug() { CROOK_TEST_DEBUG=$((CROOK_TEST_DEBUG+1)); }\n             trap '__crook_test_debug' DEBUG\n",
        ),
    ];
    let Some(ran) = (RealShell {
        name: "bash",
        home_files: &home_files,
        command: "printf 'CROOK_TRAPS[%s]\\n' \"$CROOK_TEST_DEBUG\"",
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    assert_four_marks("bash", &ran.seen);
    let traps = printed(&ran.seen, "CROOK_TRAPS").unwrap_or_default();
    assert!(
        traps.parse::<u32>().is_ok_and(|count| count > 0),
        "the user's own DEBUG trap was thrown away — Crook chains onto whatever \
         it finds rather than replacing it, and a lost trap is a lost \
         bash-preexec, atuin or ble.sh. It reported {traps:?}"
    );
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

/// The one line that prints `$HISTFILE`, and the label [`printed`] finds it by.
const PRINT_HISTFILE: &str = "printf 'CROOK_HISTFILE[%s]\\n' \"$HISTFILE\"";

/// See [`PRINT_HISTFILE`].
const HISTFILE_LABEL: &str = "CROOK_HISTFILE";

#[test]
fn test_a_real_zsh_writes_its_history_where_a_zsh_started_by_hand_writes_it() {
    // The failure this stands against is silent and total. macOS ships an
    // /etc/zshrc that runs before Crook's stub and sets
    // HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history — with ZDOTDIR being the scratch
    // directory the pane deletes when it closes. The pane then starts with an
    // empty history and throws away everything typed into it, on the platform
    // the whole login change was written for.
    let Some(ran) = (RealShell {
        name: "zsh",
        command: PRINT_HISTFILE,
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    let inside = printed(&ran.seen, HISTFILE_LABEL).unwrap_or_else(|| {
        panic!(
            "the shell should have printed its HISTFILE, wrote {:?}",
            ran.seen
        )
    });
    let outside = by_hand(
        &installed("zsh").expect("zsh is installed"),
        &ran.home,
        PRINT_HISTFILE,
    )
    .and_then(|out| printed(&out, HISTFILE_LABEL))
    .expect("a zsh started by hand should print its HISTFILE");

    assert_eq!(
        inside, outside,
        "the history has to go where this person's history goes. A HISTFILE \
         inside Crook's scratch directory is deleted with the pane: up-arrow \
         shows nothing from yesterday, and nothing typed today survives"
    );
    assert!(
        inside.starts_with(&ran.home.path().display().to_string()),
        "and specifically inside the person's own home rather than the scratch \
         directory Session::drop removes; got {inside:?}"
    );
}

#[test]
fn test_a_real_zsh_shows_the_users_own_zshenv_the_zdotdir_it_expects() {
    // `ZDOTDIR="${ZDOTDIR:-$HOME/zcfg}"` is a common line in a .zshenv, and it
    // takes the wrong branch against a ZDOTDIR that somebody else set. Crook
    // sets one, so without care this person loses .zprofile, .zshrc and
    // .zlogin — every alias, the prompt, the PATH, the plugins — with nothing
    // on screen saying why.
    let moved: Vec<(String, String)> = [".zprofile", ".zshrc", ".zlogin"]
        .iter()
        .map(|file| (format!("zcfg/{file}"), announce(file)))
        .collect();
    let mut home_files: Vec<(&str, &str)> = vec![(
        ".zshenv",
        "export ZDOTDIR=\"${ZDOTDIR:-$HOME/zcfg}\"\nprintf '[sourced .zshenv]\\n'\n",
    )];
    home_files.extend(
        moved
            .iter()
            .map(|(name, contents)| (name.as_str(), contents.as_str())),
    );

    let Some(ran) = (RealShell {
        name: "zsh",
        home_files: &home_files,
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    for file in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
        assert_eq!(
            sourced(&ran.seen, file),
            1,
            "the user's own .zshenv moved ZDOTDIR, and every later file has to \
             be read from where it moved it to — {file} was not. What the shell \
             wrote was {:?}",
            ran.seen
        );
    }
}

/// The one line that says whether `ZDOTDIR` is set at all, and its label.
const PRINT_ZDOTDIR: &str = "printf 'CROOK_ZDOTDIR_SET[%s]\\n' \"${ZDOTDIR+set}\"";

/// See [`PRINT_ZDOTDIR`].
const ZDOTDIR_LABEL: &str = "CROOK_ZDOTDIR_SET";

#[test]
fn test_a_real_zsh_leaves_zdotdir_the_way_it_found_it() {
    // What the pane is left holding after startup, which is what a nested zsh
    // and an `exec zsh` inside it will see. A ZDOTDIR exported where the person
    // had none is the same wrong branch as above, one shell later.
    let Some(ran) = (RealShell {
        name: "zsh",
        command: PRINT_ZDOTDIR,
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    let inside = printed(&ran.seen, ZDOTDIR_LABEL)
        .unwrap_or_else(|| panic!("the shell should have answered, wrote {:?}", ran.seen));
    assert_eq!(
        inside, "",
        "this home has no ZDOTDIR of its own, so the prompt must not have one \
         either — `${{ZDOTDIR:-...}}` in anything the person runs from here \
         would take the branch for a shell that does"
    );
}

#[test]
fn test_a_real_bash_runs_the_logout_files_a_login_bash_runs() {
    // bash cannot be given --rcfile and -l at once, so Crook's rc file plays
    // the login shell. The half that only shows on the way out is easy to
    // forget: ~/.bash_logout is where `ssh-agent -k` and `history -a` live, and
    // `logout` is what a person types to close the pane.
    let home_files = [(".bash_logout", "printf '[sourced .bash_logout]\\n'\ntrue\n")];
    let Some(ran) = (RealShell {
        name: "bash",
        home_files: &home_files,
        command: "logout",
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    assert!(
        !ran.seen.contains("not login shell"),
        "`logout` closes a pane in every other terminal on the machine; bash \
         answers ``not login shell: use `exit''` unless it is put back. What \
         the shell wrote was {:?}",
        ran.seen
    );
    assert_eq!(
        sourced(&ran.seen, ".bash_logout"),
        1,
        "a login bash reads ~/.bash_logout on its way out, and this one has to \
         as well. What the shell wrote was {:?}",
        ran.seen
    );
}

#[test]
fn test_a_real_bash_keeps_an_exit_trap_the_users_own_files_installed() {
    // The logout emulation is a trap, and a trap that replaced the person's own
    // would be a worse bug than the one it fixes.
    let home_files = [
        (
            ".bash_profile",
            "trap 'printf \"[sourced their-exit-trap]\\n\"' EXIT\n",
        ),
        (".bash_logout", "printf '[sourced .bash_logout]\\n'\n"),
    ];
    let Some(ran) = (RealShell {
        name: "bash",
        home_files: &home_files,
        command: "exit",
        ..RealShell::default()
    })
    .run() else {
        return;
    };

    assert_eq!(
        sourced(&ran.seen, "their-exit-trap"),
        1,
        "Crook chains onto the EXIT trap it finds rather than replacing it. \
         What the shell wrote was {:?}",
        ran.seen
    );
    assert_eq!(
        sourced(&ran.seen, ".bash_logout"),
        1,
        "and still runs the logout file. What the shell wrote was {:?}",
        ran.seen
    );
}

#[test]
fn test_a_shell_crook_has_no_switch_for_is_still_a_login_shell() {
    // The argv[0] convention, end to end, on the one shell every Unix has.
    // `/bin/sh` is Shell::Other — no marks, no arguments — and a login sh is
    // what reads ~/.profile. Without argv[0] it reads nothing, which is the
    // whole bug for everyone on tcsh, ksh, dash or a wrapper script.
    if cfg!(windows) {
        return;
    }
    let shell = Path::new("/bin/sh");
    if !shell.is_file() {
        return;
    }

    let home = TempDir::new("sh");
    fs::write(home.path().join(".profile"), announce(".profile"))
        .expect("a startup file should be writable");

    let launch = plain(shell, true);
    let mut options = TerminalOptions {
        size: TerminalSize::new(80, 24),
        program: launch.program.clone(),
        ..TerminalOptions::default()
    };
    options.environment.extend(launch.environment);
    options.environment.extend(home_environment(&home));

    let mut terminal = Terminal::spawn(options).expect("a shell should start on a pty");
    let output = read_on_a_thread(
        terminal
            .take_reader()
            .expect("a fresh terminal has its reader"),
    );

    let mut seen = String::new();
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline && sourced(&seen, ".profile") == 0 {
        let Ok(chunk) = output.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        else {
            break;
        };
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    let _ = terminal.write(b"exit\r");

    assert_eq!(
        sourced(&seen, ".profile"),
        1,
        "/bin/sh takes no -l that Crook is willing to pass it, so the only way \
         it reads the person's profile is argv[0] — the same convention \
         login(1) uses. What it wrote was {seen:?}"
    );
}

#[test]
fn test_a_shell_that_is_no_longer_installed_still_opens_a_pane() {
    // $SHELL outliving the shell it names — a Homebrew or Nix package removed,
    // a chsh to a path that moved. Every other terminal falls back to the
    // password database and keeps working; a pane that will not open is the
    // worst answer available.
    if cfg!(windows) {
        return;
    }
    let launch = plain(Path::new("/nowhere/a-shell-that-was-uninstalled"), true);
    let mut options = TerminalOptions {
        size: TerminalSize::new(80, 24),
        program: launch.program.clone(),
        ..TerminalOptions::default()
    };
    options.environment.extend(launch.environment);

    let terminal = Terminal::spawn(options);
    assert!(
        terminal.is_ok(),
        "a shell that is not there has to fall back to the one the password \
         database names, not to a pane with an error string in it: {:?}",
        terminal.err()
    );
}

#[test]
fn test_the_login_default_is_the_one_this_desktops_own_terminal_uses() {
    assert_eq!(
        login_by_default(),
        cfg!(not(target_os = "linux")),
        "macOS terminals start login shells and /etc/zprofile is the only thing \
         that runs path_helper; GNOME Terminal and Konsole start non-login \
         shells and Linux configurations put PATH in ~/.bashrc to match"
    );
    assert_eq!(Options::default().login, login_by_default());
}

#[test]
fn test_the_standing_of_a_shell_is_read_from_its_name_and_the_opt_out() {
    // What the Shell page prints, decided from the same two facts the launch
    // reads: the program's file name, and whether the environment opted out.
    let zsh = Standing::of(PathBuf::from("/bin/zsh"), false);
    assert_eq!(zsh.marks, Marks::Installed(Shell::Zsh));
    assert_eq!(zsh.program, PathBuf::from("/bin/zsh"));

    assert_eq!(
        Standing::of(PathBuf::from("/usr/local/bin/fish"), false).marks,
        Marks::Installed(Shell::Fish)
    );

    // `/bin/sh` is not bash even where it is: see `Shell::of`.
    assert_eq!(
        Standing::of(PathBuf::from("/bin/sh"), false).marks,
        Marks::NoneFor("sh".to_owned())
    );
    assert_eq!(
        Standing::of(PathBuf::from("/opt/homebrew/bin/nu"), false).marks,
        Marks::NoneFor("nu".to_owned())
    );

    // The opt-out beats a shell that has a snippet, which is the case a person
    // set the variable in a profile and forgot: the page has to say so.
    assert_eq!(
        Standing::of(PathBuf::from("/bin/zsh"), true).marks,
        Marks::OptedOut
    );
}
