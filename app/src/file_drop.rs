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
//! cmd.exe, and PowerShell. This module only picks the spelling for whoever
//! is reading, and leaves out what cannot be typed.

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
/// A path that is not text, or holds a control character, is left out
/// rather than typed as something else. Typing it lossily would name a file
/// that is not there, and a line break in a path a program has not asked for
/// bracketed paste for is the Enter key.
pub fn line(paths: &[PathBuf], shell: Option<&Path>) -> Option<String> {
    let spelling = Spelling::of(shell);
    let words: Vec<String> = paths
        .iter()
        .filter_map(|path| {
            let typed = path
                .to_str()
                .filter(|text| !text.chars().any(char::is_control));
            if typed.is_none() {
                log::warn!("a dropped file whose path cannot be typed was left out: {path:?}");
            }
            typed
        })
        .map(|path| spelling.spell(path))
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

    fn spell(self, path: &str) -> String {
        match self {
            Self::Cmd => format!("\"{path}\""),
            Self::PowerShell => powershell(path),
            Self::Host => quote(path),
        }
    }
}

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
        let expected: String = dropped.iter().map(|path| format!("{path}\n")).collect();

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
                line(&paths(&dropped), Some(Path::new(shell))).expect("every path can be typed");
            let Ok(output) = crate::process::command(shell)
                .args(flags)
                .arg(format!("{print} {typed}"))
                .output()
            else {
                // Not on this machine.
                continue;
            };
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                expected,
                "{shell} read {typed:?} as something else"
            );
        }
    }
}
