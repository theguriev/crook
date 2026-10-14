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
//! `crook tab new`'s words go through, so there is one spelling to get right
//! and it is proven against the shells a pane runs. This module only picks
//! the spelling for whoever is reading, and leaves out what cannot be typed.

use std::path::{Path, PathBuf};

use crate::plugins::wasm::quote;

/// The line a drop of `paths` types: each path as one quoted word, in the
/// order they were dropped, with a space between two and none around them.
/// `None` when not one of them can be typed.
///
/// `shell` is the shell that will read the line, when it goes into a pane's
/// composer to be run there; `None` when it goes to a program instead. Both
/// get [`quote`]'s spelling, which off Windows is POSIX's — single quotes,
/// and a quote inside them closed, escaped and reopened — with a backslash
/// taken out of the quotes as well, so fish reads it as sh, bash and zsh do.
/// That is also right for a program that is not a shell. One reading a path
/// out of its input reads shell words, and one that *is* a shell — the far
/// side of an `ssh`, or a shell nobody installed the integration for, neither
/// of which has a composer — may be any of the four. On Windows it is
/// PowerShell's spelling, which a POSIX reader reads back the same but for a
/// `'` in a path. The one exception is cmd.exe, Windows' own default, which
/// reads a single quote as a letter: a composer over it gets double quotes,
/// which cannot appear in a Windows path and so need no escaping inside.
///
/// A path that is not text, or holds a control character, is left out
/// rather than typed as something else. Typing it lossily would name a file
/// that is not there, and a line break in a path a program has not asked for
/// bracketed paste for is the Enter key.
pub fn line(paths: &[PathBuf], shell: Option<&Path>) -> Option<String> {
    let cmd = shell.is_some_and(is_cmd);
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
        .map(|path| {
            if cmd {
                format!("\"{path}\"")
            } else {
                quote(path)
            }
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Whether `shell` is cmd.exe, by the program's name as `ComSpec` names it.
///
/// Split on both separators by hand rather than by [`Path::file_name`], which
/// splits on the platform's own: a Windows path read anywhere else is one
/// long name.
fn is_cmd(shell: &Path) -> bool {
    let program = shell.to_string_lossy();
    let name = program.rsplit(['/', '\\']).next().unwrap_or_default();
    name.eq_ignore_ascii_case("cmd") || name.eq_ignore_ascii_case("cmd.exe")
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
    fn a_composer_over_cmd_gets_double_quotes() {
        let dropped = paths(&[r"C:\Users\me\Screen Shot.png", r"C:\Users\me\a.log"]);

        for shell in [r"C:\Windows\system32\cmd.exe", "CMD.EXE"] {
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
        // file agrees with: each shell is handed the line as the arguments of
        // a `printf` that prints one argument a line, and has to print the
        // paths. The fish break-out `quote` was fixed for is among them.
        let dropped = [
            "/tmp/Screen Shot 2026-10-14 at 09.41.07.png",
            "/tmp/it's here.log",
            "/tmp/x\\' ; echo PWNED ; echo \\'",
            "/tmp/$HOME `id` *.png",
        ];
        let typed = line(&paths(&dropped), None).expect("every path can be typed");
        let expected: String = dropped.iter().map(|path| format!("{path}\n")).collect();

        for (shell, flags) in [
            ("sh", &["-c"][..]),
            ("bash", &["-c"][..]),
            ("zsh", &["-f", "-c"][..]),
            ("fish", &["--no-config", "-c"][..]),
        ] {
            let Ok(output) = crate::process::command(shell)
                .args(flags)
                .arg(format!("printf '%s\\n' {typed}"))
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
