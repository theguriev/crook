//! Opening a changed file in the editor a person has named, at the line that
//! changed when the editor is one known to take a line.
//!
//! `$VISUAL`, then `$EDITOR` — the order git and every Unix tool read them in —
//! split into a program and its arguments the way a shell splits the value,
//! which is how git runs it: `EDITOR="code -w"` is two words, and
//! `EDITOR="'/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl' -w"`
//! is two as well. Split, not run through a shell: nothing in the value is
//! expanded, and no shell is needed on a platform that has none.
//!
//! # Only an editor with a window of its own
//!
//! The editor is started *detached* — no terminal, nothing on its stdin — so
//! it has to be one that opens a window. `$EDITOR=nvim` is the common case
//! and the one this cannot serve: started with no terminal, Neovim does not
//! exit, it waits for one for ever, and a click that did nothing on screen
//! would leave a process behind for every press. So an editor known to need
//! a terminal is not offered at all, and the action is absent rather than a
//! button that does nothing. Opening one in a tab of Crook's own is the
//! answer for those, and it waits on Crook being able to start a command in a
//! new tab — which is not something a tab can do yet.
//!
//! A wrapper script is taken at its word: nothing here can see what it runs.
//!
//! # The line
//!
//! Editors do not agree on how to be told one. The VS Code family takes
//! `--goto path:N`; Sublime Text and Zed take `path:N`; the JetBrains IDEs,
//! Kate and TextMate take `--line N path`; Notepad++ takes `-nN path`; gVim,
//! Emacs, gedit and its forks and BBEdit take the old `+N path`. Anything
//! else is handed the path alone and opens at the top: a line argument an
//! editor does not read is one it opens as a second file, called `+12`, and
//! a wrapper script — Omarchy's `omarchy-launch-editor`, say, which starts VS
//! Code — has no family this can see.

use std::ffi::OsString;
use std::path::Path;

/// Editors that draw in the terminal they were started from, and so have
/// nothing to draw in when started from here.
///
/// By the program's own name, lower-cased and without the extension Windows
/// runs it by.
const TERMINAL_EDITORS: [&str; 17] = [
    "vi", "vim", "nvim", "view", "ex", "ed", "nano", "pico", "micro", "hx", "helix", "kak", "ne",
    "joe", "jed", "mg", "vis",
];

/// Arguments that put an editor that could open a window in the terminal
/// instead: Emacs's and `emacsclient`'s.
const TERMINAL_FLAGS: [&str; 4] = ["-nw", "--no-window-system", "-t", "--tty"];

/// What Windows runs a program by as well as `.exe`: the scripts `code`,
/// `cursor` and `windsurf` are installed as on `PATH`. Rust's own search
/// tries `.exe` alone, so these are tried after it.
const WINDOWS_SCRIPTS: [&str; 2] = ["cmd", "bat"];

/// The editor a person has named, as a program and the arguments they gave
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Editor {
    program: String,
    arguments: Vec<String>,
}

impl Editor {
    /// The editor this process's environment names, if it names one this can
    /// start.
    pub(crate) fn from_environment() -> Option<Self> {
        let visual = std::env::var("VISUAL").ok();
        let editor = std::env::var("EDITOR").ok();
        Self::from_variables(visual.as_deref(), editor.as_deref())
    }

    /// The editor two values of `$VISUAL` and `$EDITOR` name.
    ///
    /// The first that is set to anything at all decides, whether or not it
    /// names an editor this can start: somebody who set `$VISUAL=vim` has
    /// said which editor they want, and falling through to an `$EDITOR` they
    /// set for some other tool would open one they did not ask for.
    pub(crate) fn from_variables(visual: Option<&str>, editor: Option<&str>) -> Option<Self> {
        let named = [visual, editor]
            .into_iter()
            .flatten()
            .find(|value| !value.trim().is_empty())?;
        let mut words = words(named, cfg!(not(windows))).into_iter();
        let program = words.next().filter(|program| !program.is_empty())?;
        let arguments: Vec<String> = words.collect();

        let name = Self::name_of(&program);
        let in_a_terminal = TERMINAL_EDITORS.contains(&name.as_str())
            || (matches!(name.as_str(), "emacs" | "emacsclient")
                && arguments
                    .iter()
                    .any(|argument| TERMINAL_FLAGS.contains(&argument.as_str())));
        if in_a_terminal {
            return None;
        }
        Some(Self { program, arguments })
    }

    /// What to call it: the program's own name, without its directory.
    pub(crate) fn name(&self) -> String {
        Self::name_of(&self.program)
    }

    /// A program's name without its directory, its capitals, or the
    /// extension Windows runs it by.
    fn name_of(program: &str) -> String {
        let file = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| program.to_owned());
        let lower = file.to_lowercase();
        [".exe", ".cmd", ".bat", ".com"]
            .into_iter()
            .find_map(|extension| lower.strip_suffix(extension))
            .map(str::to_owned)
            .unwrap_or(lower)
    }

    /// The program and every argument, to open `path` at `line` — or at the
    /// top, for an editor not known to take a line.
    pub(crate) fn command_line(&self, path: &Path, line: u32) -> (String, Vec<OsString>) {
        let mut arguments: Vec<OsString> = self.arguments.iter().map(OsString::from).collect();
        let at = |separator: &str| {
            let mut spelled = path.as_os_str().to_owned();
            spelled.push(format!("{separator}{line}"));
            spelled
        };
        match self.name().as_str() {
            "code" | "code-insiders" | "code-oss" | "codium" | "vscodium" | "cursor"
            | "windsurf" => {
                arguments.push("--goto".into());
                arguments.push(at(":"));
            }
            "subl" | "sublime_text" | "zed" | "zeditor" => arguments.push(at(":")),
            "idea" | "idea64" | "pycharm" | "pycharm64" | "webstorm" | "webstorm64" | "clion"
            | "clion64" | "goland" | "goland64" | "rustrover" | "rustrover64" | "rider"
            | "rider64" | "phpstorm" | "phpstorm64" | "studio" | "studio64" | "kate" | "kwrite"
            | "mate" => {
                arguments.push("--line".into());
                arguments.push(line.to_string().into());
                arguments.push(path.as_os_str().to_owned());
            }
            "notepad++" => {
                arguments.push(format!("-n{line}").into());
                arguments.push(path.as_os_str().to_owned());
            }
            "gvim" | "mvim" | "emacs" | "emacsclient" | "gedit" | "xed" | "pluma" | "bbedit" => {
                arguments.push(format!("+{line}").into());
                arguments.push(path.as_os_str().to_owned());
            }
            _ => arguments.push(path.as_os_str().to_owned()),
        }
        (self.program.clone(), arguments)
    }

    /// Starts the editor on `path` at `line`, and does not wait for it.
    ///
    /// Nothing is handed to it on stdin and nothing it prints is read: an
    /// editor with a window of its own has no use for either, and one that
    /// wrote to a pipe nobody reads would block on it. It is reaped when it
    /// closes — see [`crate::process::reap`].
    pub(crate) fn open(&self, path: &Path, line: u32) -> std::io::Result<()> {
        let (program, arguments) = self.command_line(path, line);
        let mut missing = None;
        for spelling in spellings(&program, cfg!(windows)) {
            let started = crate::process::command(&spelling)
                .args(&arguments)
                .current_dir(path.parent().unwrap_or(path))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match started {
                Ok(child) => {
                    crate::process::reap(child, "editor");
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(missing.unwrap_or_else(|| std::io::ErrorKind::NotFound.into()))
    }
}

/// A command line split into words the way a POSIX shell splits one, with
/// nothing expanded: whitespace between words, a single-quoted run taken as
/// it is, a double-quoted one taken as it is but for a backslash before `"`,
/// `\`, `$` or a backtick. Outside quotes a backslash takes the next
/// character as it is where `backslash_escapes` — not on Windows, where it is
/// how a path is written and `C:\Windows\notepad.exe` has to survive
/// unquoted. A quote left open runs to the end, which is the reading a
/// person who forgot to close it meant.
fn words(line: &str, backslash_escapes: bool) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    // A word can be nothing but quotes — `''` — and is still a word.
    let mut in_word = false;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\'' => {
                in_word = true;
                for quoted in characters.by_ref() {
                    if quoted == '\'' {
                        break;
                    }
                    word.push(quoted);
                }
            }
            '"' => {
                in_word = true;
                while let Some(quoted) = characters.next() {
                    match quoted {
                        '"' => break,
                        '\\' if backslash_escapes
                            && characters
                                .peek()
                                .is_some_and(|next| matches!(next, '"' | '\\' | '$' | '`')) =>
                        {
                            word.extend(characters.next());
                        }
                        other => word.push(other),
                    }
                }
            }
            '\\' if backslash_escapes => {
                in_word = true;
                word.extend(characters.next());
            }
            space if space.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            other => {
                in_word = true;
                word.push(other);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// The names to try starting `program` by, in order.
///
/// Its own, always. On Windows a bare name — no directory, no extension —
/// is also tried as the scripts [`WINDOWS_SCRIPTS`] names, after Rust's own
/// search has looked for it as an `.exe`: `code` on Windows is `code.cmd`.
fn spellings(program: &str, windows: bool) -> Vec<String> {
    let mut names = vec![program.to_owned()];
    let bare = Path::new(program).extension().is_none();
    if windows && bare {
        names.extend(
            WINDOWS_SCRIPTS
                .into_iter()
                .map(|extension| format!("{program}.{extension}")),
        );
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(line: &str) -> Vec<String> {
        words(line, true)
    }

    #[test]
    fn a_value_is_split_the_way_a_shell_splits_it() {
        assert_eq!(split("code -w"), ["code", "-w"]);
        assert_eq!(split("  code\t -w  "), ["code", "-w"]);
        assert_eq!(
            split("'/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl' -w"),
            [
                "/Applications/Sublime Text.app/Contents/SharedSupport/bin/subl",
                "-w"
            ]
        );
        assert_eq!(
            split(r#""/opt/My Editor/bin/edit" --flag="a b""#),
            ["/opt/My Editor/bin/edit", "--flag=a b"]
        );
        assert_eq!(split(r"/opt/My\ Editor/edit"), ["/opt/My Editor/edit"]);
        assert_eq!(split(r#""say \"hi\"""#), [r#"say "hi""#]);
        assert_eq!(split("edit ''"), ["edit", ""]);
        assert_eq!(split("'never closed"), ["never closed"]);
    }

    #[test]
    fn a_windows_path_keeps_its_backslashes() {
        assert_eq!(
            words(r"C:\Windows\notepad.exe", false),
            [r"C:\Windows\notepad.exe"]
        );
        assert_eq!(
            words(
                r#""C:\Program Files\Notepad++\notepad++.exe" -multiInst"#,
                false
            ),
            [r"C:\Program Files\Notepad++\notepad++.exe", "-multiInst"]
        );
        // Inside double quotes a backslash before an ordinary character is
        // itself on every platform, so a quoted Windows path survives the
        // Unix rules too.
        assert_eq!(
            words(r#""C:\Program Files\Editor\edit.exe""#, true),
            [r"C:\Program Files\Editor\edit.exe"]
        );
    }

    #[test]
    fn a_bare_name_on_windows_is_tried_as_its_scripts_too() {
        assert_eq!(spellings("code", true), ["code", "code.cmd", "code.bat"]);
        assert_eq!(spellings("code.cmd", true), ["code.cmd"]);
        assert_eq!(spellings("code", false), ["code"]);
    }

    #[test]
    fn a_script_on_windows_is_named_without_its_extension() {
        let editor = Editor::from_variables(Some("code.cmd"), None).expect("an editor");
        assert_eq!(editor.name(), "code");
        let (_, arguments) = editor.command_line(Path::new("main.rs"), 7);
        assert_eq!(arguments, [OsString::from("--goto"), "main.rs:7".into()]);
    }
}
