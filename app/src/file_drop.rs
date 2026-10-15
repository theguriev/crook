//! What a file dropped on a pane types: its path, spelled so that whatever
//! reads it reads the path back.
//!
//! Every terminal does the same thing with a drop, which is to type the path
//! of each file and leave the rest to what is listening — a shell at its
//! prompt, which takes it as an argument, or an agent, which takes the path of
//! a picture as the picture. Typed *quoted*, because the file most often
//! dropped is a screenshot, and every desktop names those with spaces.
//!
//! The quoting is the host's own, [`quote`], the one a plugin's argument and
//! `crook tab new`'s words go through, so there is one spelling to get right:
//! off Windows it is proven against sh, bash, zsh and fish, by those shells.
//! Two readers get a spelling of their own, because that one is not theirs:
//! cmd.exe, and PowerShell. Neither reports its commands, so a program started
//! from either reads what was spelled for the shell, and their spellings also
//! leave out a name that a shell started from them would run part of. This
//! module only picks the spelling for whoever is reading, and leaves out what
//! cannot be typed.

use std::path::{Path, PathBuf};

use crate::plugins::wasm::quote;

/// The line a drop of `paths` types: each path as one quoted word, in the
/// order they were dropped, with a space between two and none around them.
/// `None` when not one of them can be typed.
///
/// `shell` is the shell that will read the line — the one a pane's composer
/// hands it to, or the pane's shell at its prompt when there is no composer —
/// and `None` when a program running in the pane reads it instead. The
/// spelling is that reader's:
///
/// * **cmd.exe**, Windows' own default, reads a single quote as a letter. It
///   gets double quotes, which cannot appear in a Windows path and so need no
///   escaping inside.
/// * **PowerShell** — `pwsh`, or Windows' own `powershell` — ends a
///   single-quoted string at a curly single quote as well as a straight one:
///   `‘`, `’`, `‚` and `‛`. It gets single quotes with every one of those five
///   doubled inside them, which it reads as the one character. `Bob’s
///   notes.txt` is an ordinary name for a file, and `x’; Start-Process calc;
///   ’.png` is one that must not run anything.
/// * **Any other shell, and a program**, get [`quote`]'s spelling. Off Windows
///   that is POSIX's — single quotes, and a quote inside them closed, escaped
///   and reopened — with a backslash taken out of the quotes as well, so fish
///   reads it as sh, bash and zsh do. That is also right for a program that
///   is not a shell: one reading a path out of its input reads shell words.
///   On Windows it is PowerShell's for a straight quote, which a POSIX reader
///   reads back the same but for a `'` in a path.
///
/// **cmd.exe and PowerShell may not be what reads their spelling.** Neither
/// reports command marks, so Crook never sees a program start from either:
/// one on the main screen — `wsl`, `ssh`, a nested bash or pwsh, an agent —
/// reads the line spelled for the shell, from the composer that stays until
/// the first screenful and at the prompt after it alike. So their spellings
/// leave out a path that a shell started from them would run part of, or read
/// past its closing quote into the next name:
///
/// * **From cmd's double quotes**, a path holding `$`, a backtick, `!`, `%`,
///   or a straight or curly double quote, or ending in a backslash. Every
///   other character, `&` and `’` among them, reads as itself inside double
///   quotes to cmd, bash, zsh, fish and PowerShell alike.
/// * **From PowerShell's single quotes**, a path with a backslash before a
///   straight quote, or ending in one, which fish reads as an escaped quote.
///   The doubled quotes cost a program something too: one that reads quotes
///   as a shell other than PowerShell does reads `’’` for `’` and nothing for
///   `''`, and so names a file that is not there, but runs nothing. A cmd.exe
///   started from PowerShell is the reader this does not cover: it takes no
///   single quote as a quote, and covering it would mean leaving out every
///   name with an `&` in it.
///
/// A path that is not text, or holds a control character, is left out
/// rather than typed as something else. Typing it lossily would name a file
/// that is not there, and a line break in a path a program has not asked for
/// bracketed paste for is the Enter key.
pub fn line(paths: &[PathBuf], shell: Option<&Path>) -> Option<String> {
    let spelling = Spelling::of(shell);
    let words: Vec<String> = paths
        .iter()
        .filter_map(|path| {
            let Some(text) = path
                .to_str()
                .filter(|text| !text.chars().any(char::is_control))
            else {
                log::warn!("a dropped file whose path cannot be typed was left out: {path:?}");
                return None;
            };
            if spelling.leaves_out(text) {
                log::warn!(
                    "a dropped file whose name a shell started from {spelling:?} could run \
                     was left out: {path:?}"
                );
                return None;
            }
            Some(spelling.spell(text))
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// How a path is quoted, by who reads it: see [`line()`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spelling {
    /// Double quotes, for cmd.exe.
    Cmd,
    /// Single quotes with PowerShell's five single quotes doubled.
    PowerShell,
    /// [`quote`]'s.
    Host,
}

impl Spelling {
    /// The spelling `shell` reads, or a program's for `None`.
    fn of(shell: Option<&Path>) -> Self {
        match shell.map(program_name).as_deref() {
            Some("cmd") => Self::Cmd,
            Some("pwsh" | "powershell") => Self::PowerShell,
            _ => Self::Host,
        }
    }

    /// Whether a shell started from this spelling's reader would read `path`,
    /// spelled this way, as more than the one word or as something to run:
    /// see [`line()`]. Only cmd's and PowerShell's spellings leave anything
    /// out. Any other shell's is the one a program gets anyway, so a program
    /// started from it is given what it would have been given regardless.
    fn leaves_out(self, path: &str) -> bool {
        // Right before the closing quote, a backslash is bash's and fish's
        // escape for the quote in double quotes, and fish's in single ones:
        // the string goes on, and the next name dropped is outside of it.
        let escapes_the_close = path.ends_with('\\');
        match self {
            Self::Cmd => escapes_the_close || path.contains(CMD_EXPANDS),
            Self::PowerShell => escapes_the_close || path.contains("\\'"),
            Self::Host => false,
        }
    }

    fn spell(self, path: &str) -> String {
        match self {
            Self::Cmd => format!("\"{path}\""),
            Self::PowerShell => powershell(path),
            Self::Host => quote(path),
        }
    }
}

/// Every character that cmd.exe, or a shell started from it, does not read
/// as itself inside double quotes: `$`, which bash, zsh, fish and PowerShell
/// expand there; the backtick, which bash and zsh run and PowerShell escapes
/// with; `!`, which an interactive bash or zsh expands from its history; `%`,
/// which cmd expands itself; and the double quotes, straight and the three
/// curly ones PowerShell ends a string at.
const CMD_EXPANDS: [char; 8] = ['$', '`', '!', '%', '"', '\u{201C}', '\u{201D}', '\u{201E}'];

/// Every character that ends a single-quoted string in PowerShell: the
/// straight quote, and the four curly ones its tokenizer counts as the same.
const POWERSHELL_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];

/// `path` as one PowerShell word: in single quotes, inside which nothing is
/// expanded, with each of [`POWERSHELL_QUOTES`] doubled — which PowerShell
/// reads as that one character rather than as the end of the string.
fn powershell(path: &str) -> String {
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('\'');
    for character in path.chars() {
        if POWERSHELL_QUOTES.contains(&character) {
            quoted.push(character);
        }
        quoted.push(character);
    }
    quoted.push('\'');
    quoted
}

/// The program `shell` names, in lower case and without an `.exe`, the way
/// `ComSpec` or a person's setting may spell it.
///
/// Split on both separators by hand rather than by [`Path::file_name`], which
/// splits on the platform's own: a Windows path read anywhere else is one
/// long name.
fn program_name(shell: &Path) -> String {
    let program = shell.to_string_lossy();
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match name.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn each_path_is_one_quoted_word_and_a_space_goes_between_two() {
        let dropped = paths(&["/tmp/Screen Shot 1.png", "/tmp/it's here.log"]);

        // Spelled out per platform rather than built with `quote`, which
        // would be the function agreeing with itself.
        let expected = match cfg!(windows) {
            true => "'/tmp/Screen Shot 1.png' '/tmp/it''s here.log'",
            false => "'/tmp/Screen Shot 1.png' '/tmp/it'\\''s here.log'",
        };
        assert_eq!(
            line(&dropped, Some(Path::new("/bin/zsh"))).as_deref(),
            Some(expected)
        );
        // A program reads the same words a shell would.
        assert_eq!(line(&dropped, None).as_deref(), Some(expected));
    }

    #[test]
    fn cmd_gets_double_quotes_and_a_program_under_it_does_not() {
        let dropped = paths(&[r"C:\Users\me\Screen Shot.png", r"C:\Users\me\a.log"]);

        for shell in [r"C:\Windows\system32\cmd.exe", "CMD.EXE", "/opt/odd/cmd"] {
            assert_eq!(
                line(&dropped, Some(Path::new(shell))).as_deref(),
                Some(r#""C:\Users\me\Screen Shot.png" "C:\Users\me\a.log""#),
                "{shell}"
            );
        }
        // A program under it is not cmd reading the line.
        assert_ne!(
            line(&dropped, None).as_deref(),
            Some(r#""C:\Users\me\Screen Shot.png" "C:\Users\me\a.log""#)
        );
    }

    #[test]
    fn powershell_gets_every_one_of_its_single_quotes_doubled() {
        // A name a download is given every day, one with every quote
        // PowerShell ends a string at, and one made to break out at the first
        // of them and run something.
        let dropped = paths(&[
            r"C:\Users\me\Downloads\Bob’s notes.txt",
            "C:\\it's \u{2018}a\u{2019} \u{201A}b\u{201B}.log",
            r"C:\Users\me\Downloads\x’; Start-Process calc; ’.png",
        ]);
        let expected = concat!(
            r"'C:\Users\me\Downloads\Bob’’s notes.txt' ",
            "'C:\\it''s \u{2018}\u{2018}a\u{2019}\u{2019} \u{201A}\u{201A}b\u{201B}\u{201B}.log' ",
            r"'C:\Users\me\Downloads\x’’; Start-Process calc; ’’.png'",
        );

        for shell in [
            "pwsh",
            "/usr/bin/pwsh",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        ] {
            assert_eq!(
                line(&dropped, Some(Path::new(shell))).as_deref(),
                Some(expected),
                "{shell}"
            );
        }
    }

    #[test]
    fn cmd_leaves_out_a_name_a_shell_started_from_it_would_run_part_of() {
        // Inside double quotes each of these is something other than the
        // path, to cmd itself (`%COMSPEC%`) or to a shell started from it,
        // which reads the same line since cmd reports nothing: the readback
        // test below has them run.
        let cmd = Some(Path::new(r"C:\Windows\system32\cmd.exe"));
        for path in [
            r"C:\Users\me\Downloads\a$(touch PWNED).png",
            r"C:\Users\me\Downloads\a`touch PWNED`.png",
            r"C:\Users\me\Downloads\x!#; touch PWNED #.png",
            r"C:\Users\me\Downloads\a%COMSPEC%.png",
            "C:\\Users\\me\\Downloads\\a\u{201C}; Start-Process calc; \u{201D}.png",
            "C:\\Users\\me\\Downloads\\a\u{201E}b.png",
            "/tmp/a\"b.png",
            r"D:\",
        ] {
            assert_eq!(line(&paths(&[path]), cmd), None, "{path}");
        }

        // One name at a time, and the rest of the drop still typed: `&`, `’`
        // and what else cmd runs or splits on outside quotes are all read as
        // themselves inside them.
        let dropped = paths(&[
            r"C:\Users\me\Downloads\a$(touch PWNED).png",
            r"C:\Users\me\Pictures\Bob’s & Ann's (1) ^ #;.png",
        ]);
        assert_eq!(
            line(&dropped, cmd).as_deref(),
            Some(r#""C:\Users\me\Pictures\Bob’s & Ann's (1) ^ #;.png""#)
        );
        // A full-screen program under cmd gets a program's spelling, single
        // quotes, inside which none of them is expanded.
        assert!(line(&dropped[..1], None).is_some());
    }

    #[test]
    fn powershell_leaves_out_a_name_fish_would_read_past_its_closing_quote() {
        // In single quotes fish reads `\'` as a quote inside the string, so a
        // backslash before a quote, or at the end before the closing one,
        // keeps its string open where PowerShell's has ended.
        let pwsh = Some(Path::new("pwsh"));
        for path in [r"C:\Users\me\'; echo PWNED #.png", r"D:\"] {
            assert_eq!(line(&paths(&[path]), pwsh), None, "{path}");
        }
        // Every Windows path is full of backslashes; only those two places
        // matter.
        assert_eq!(
            line(&paths(&[r"C:\Users\me\it's\a.png"]), pwsh).as_deref(),
            Some(r"'C:\Users\me\it''s\a.png'")
        );
    }

    #[test]
    fn a_path_with_a_control_character_in_it_is_left_out() {
        assert_eq!(
            line(&paths(&["/tmp/one\ntwo", "/tmp/a.png"]), None).as_deref(),
            Some("'/tmp/a.png'")
        );
        assert_eq!(line(&paths(&["/tmp/one\rtwo"]), None), None);
        assert_eq!(line(&[], None), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_text_is_left_out() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let latin1 = PathBuf::from(OsStr::from_bytes(b"/tmp/caf\xe9.png"));
        assert_eq!(
            line(&[latin1, PathBuf::from("/tmp/a.png")], None).as_deref(),
            Some("'/tmp/a.png'")
        );
    }

    #[cfg(unix)]
    #[test]
    fn every_shell_on_this_machine_reads_the_line_back_as_the_paths() {
        // The proof that the line is the paths, rather than a spelling this
        // file agrees with: each shell is handed the line it would be typed
        // as the arguments of a command that prints one argument a line, and
        // has to print the paths. The fish break-out `quote` was fixed for is
        // among them, and so is a PowerShell one through a curly quote.
        let dropped = [
            "/tmp/Screen Shot 2026-10-14 at 09.41.07.png",
            "/tmp/it's here.log",
            "/tmp/x\\' ; echo PWNED ; echo \\'",
            "/tmp/$HOME `id` *.png",
            "/tmp/Bob’s x’; echo PWNED; ’.png",
        ];

        for (shell, flags, print) in [
            ("sh", &["-c"][..], "printf '%s\\n'"),
            ("bash", &["-c"][..], "printf '%s\\n'"),
            ("zsh", &["-f", "-c"][..], "printf '%s\\n'"),
            ("fish", &["--no-config", "-c"][..], "printf '%s\\n'"),
            (
                "pwsh",
                &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"][..],
                "Write-Output",
            ),
        ] {
            let typed =
                line(&paths(&dropped), Some(Path::new(shell))).expect("a path can be typed");
            let Ok(output) = crate::process::command(shell)
                .args(flags)
                .arg(format!("{print} {typed}"))
                .output()
            else {
                // Not on this machine.
                continue;
            };
            // PowerShell's spelling leaves the fish break-out out, which a
            // fish started from PowerShell would read past its quote: see the
            // test below.
            let expected: String = dropped
                .iter()
                .filter(|path| shell != "pwsh" || !path.contains("\\'"))
                .map(|path| format!("{path}\n"))
                .collect();
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                expected,
                "{shell} read {typed:?} as something else"
            );
        }
    }

    /// What a shell made of a line: what it printed, what it said on stderr,
    /// and whether it exited cleanly.
    #[cfg(unix)]
    struct Printed {
        stdout: String,
        stderr: String,
        success: bool,
    }

    #[cfg(unix)]
    impl Printed {
        /// Whether the shell failed without printing a thing — which is what
        /// a shell that never got as far as the line looks like, and also
        /// what one that could not parse it looks like.
        fn failed_silently(&self) -> bool {
            !self.success && self.stdout.is_empty()
        }

        /// Whether the shell said it could not parse the line: bash's
        /// `syntax error` and `unexpected EOF`, zsh's `parse error` and
        /// `unmatched`, fish's unbalanced quotes, PowerShell's `ParserError`.
        /// Read generously, because a misreading taken for a shell that would
        /// not start is a failure passed over.
        fn could_not_parse(&self) -> bool {
            let said = self.stderr.to_lowercase();
            [
                "syntax error",
                "parse error",
                "parsererror",
                "unexpected",
                "unmatched",
                "unterminated",
                "not balanced",
                "missing the terminator",
            ]
            .iter()
            .any(|sign| said.contains(sign))
        }
    }

    /// What `shell` makes of `words` as the arguments of a command that
    /// prints each on a line of its own — `printf`, or PowerShell's
    /// `Write-Output` — or `None` when it is not on this machine, or will not
    /// run on it.
    ///
    /// A shell that fails without printing anything is given the line once
    /// more: pwsh on a CI runner has failed to start that way, cold, and its
    /// silence was read as a misreading of every name. One that fails so
    /// twice, saying something other than that it could not parse the line,
    /// and cannot print a plain word either, does not run here, and is passed
    /// over. One that could not parse the line, or can print a plain word and
    /// not these, misread them, and is handed back for the test to fail on,
    /// with what it said.
    #[cfg(unix)]
    fn printed_by(shell: &str, words: &str) -> Option<Printed> {
        let mut printed = run_printing(shell, words)?;
        if printed.failed_silently() {
            printed = run_printing(shell, words)?;
        }
        if printed.failed_silently()
            && !printed.could_not_parse()
            && run_printing(shell, "word").is_none_or(|plain| plain.stdout != "word\n")
        {
            eprintln!(
                "skipped: {shell} runs nothing here; it said {:?}",
                printed.stderr
            );
            return None;
        }
        Some(printed)
    }

    /// One run of `shell` printing `words`, as [`printed_by`] describes, or
    /// `None` when it is not on this machine.
    ///
    /// bash is typed the line, as a person's bash is, rather than handed it
    /// with `-c`, so that it expands `!` from its history as it does there.
    #[cfg(unix)]
    fn run_printing(shell: &str, words: &str) -> Option<Printed> {
        use std::io::Write as _;
        use std::process::Stdio;

        let printf = format!("printf '%s\\n' {words}");
        let output = match shell {
            "bash" => {
                let mut bash = crate::process::command("bash")
                    .args(["--norc", "--noprofile", "-i"])
                    // So that nothing typed here lands in a history file.
                    .env("HISTFILE", "")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .ok()?;
                bash.stdin
                    .take()?
                    .write_all(format!("{printf}\nexit\n").as_bytes())
                    .expect("bash reads what it is typed");
                bash.wait_with_output()
            }
            "pwsh" => crate::process::command("pwsh")
                .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
                .arg(format!("Write-Output {words}"))
                .output(),
            "zsh" => crate::process::command("zsh")
                .args(["-f", "-c", &printf])
                .output(),
            "fish" => crate::process::command("fish")
                .args(["--no-config", "-c", &printf])
                .output(),
            _ => crate::process::command(shell)
                .args(["-c", &printf])
                .output(),
        }
        .ok()?;
        Some(Printed {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
        })
    }

    #[cfg(unix)]
    #[test]
    fn no_shell_started_from_cmd_or_powershell_runs_any_part_of_a_name() {
        // cmd.exe and PowerShell report no commands, so the line spelled for
        // either may be read instead by a shell started from it: `wsl`, `ssh`,
        // a nested bash or pwsh. Each shell on this machine is handed both
        // spellings of each name, followed by one that runs `echo PWNED` the
        // moment its quotes are lost, and has to print the names it was given
        // and nothing else. Each name here is one that breaks out of one of
        // the two spellings, in one of these shells, unless it is left out —
        // all but the first two, which are there to be typed.
        //
        // (name, left out of cmd's spelling, left out of PowerShell's)
        let names = [
            ("/tmp/Screen Shot 2026-10-14 at 09.41.07.png", false, false),
            ("/tmp/Bob’s & it's #1.png", false, false),
            // bash, sh, zsh, fish and pwsh in double quotes.
            ("/tmp/a$(echo PWNED).png", true, false),
            // bash, sh and zsh in double quotes.
            ("/tmp/a`echo PWNED`.png", true, false),
            // bash's history, in double quotes: `!#` repeats the line so far,
            // and the quote in it closes the string.
            ("/tmp/x!#; echo PWNED #.png", true, false),
            // pwsh ends a double-quoted string at a curly double quote.
            ("/tmp/a\u{201C}; echo PWNED #\u{201D}.png", true, false),
            // fish in single quotes.
            ("/tmp/c\\'; echo PWNED #.png", false, true),
            // bash, sh and zsh in double quotes, and fish in either.
            ("/tmp/dir\\", true, true),
        ];
        let then = "/tmp/b; echo PWNED #.png";

        for (name, out_of_cmd, out_of_powershell) in names {
            for (spelling, left_out) in [("cmd", out_of_cmd), ("pwsh", out_of_powershell)] {
                let reader = Some(Path::new(spelling));
                let typed =
                    line(&paths(&[name, then]), reader).expect("the last name can be typed");
                let kept: Vec<&str> = [name, then]
                    .into_iter()
                    .filter(|path| line(&paths(&[path]), reader).is_some())
                    .collect();
                let expected: String = kept.iter().map(|path| format!("{path}\n")).collect();
                for shell in ["sh", "bash", "zsh", "fish", "pwsh"] {
                    let Some(printed) = printed_by(shell, &typed) else {
                        continue;
                    };
                    assert!(
                        !printed.stdout.lines().any(|line| line == "PWNED"),
                        "{shell} ran part of {name:?} spelled for {spelling} as {typed:?}"
                    );
                    // Every one of them reads a double-quoted string as itself
                    // once what it would expand is left out. PowerShell's
                    // doubled quotes are read as the name by PowerShell alone.
                    if spelling == "cmd" || shell == "pwsh" {
                        assert_eq!(
                            printed.stdout, expected,
                            "{shell} read {typed:?}, spelled for {spelling}, as something else; \
it said {:?}",
                            printed.stderr
                        );
                    }
                }
                assert_eq!(
                    !kept.contains(&name),
                    left_out,
                    "{name:?} spelled for {spelling}"
                );
            }
        }
    }
}
