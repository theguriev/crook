//! Opening a changed file in the editor a person has named, at the line that
//! changed.
//!
//! `$VISUAL`, then `$EDITOR` — the order git and every Unix tool read them in —
//! split on whitespace into a program and its arguments, the way `EDITOR="code
//! -w"` is written. Not through a shell: a shell would make the variable a
//! command line, and a path with a space or a `$` in it would be one too.
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
//! Editors do not agree on how to be told one. `+N path` is the old
//! convention and the default here — gVim, Emacs and gedit read it; the VS
//! Code family takes `--goto path:N`; Sublime Text and Zed take `path:N`; the
//! JetBrains IDEs take `--line N path`.

use std::ffi::OsString;
use std::path::Path;

/// Editors that draw in the terminal they were started from, and so have
/// nothing to draw in when started from here.
///
/// By the program's own name, lower-cased and without `.exe`.
const TERMINAL_EDITORS: [&str; 17] = [
    "vi", "vim", "nvim", "view", "ex", "ed", "nano", "pico", "micro", "hx", "helix", "kak", "ne",
    "joe", "jed", "mg", "vis",
];

/// Arguments that put an editor that could open a window in the terminal
/// instead: Emacs's and `emacsclient`'s.
const TERMINAL_FLAGS: [&str; 4] = ["-nw", "--no-window-system", "-t", "--tty"];

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
        let mut words = named.split_whitespace().map(str::to_owned);
        let program = words.next()?;
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

    /// A program's name without its directory, its `.exe` or its capitals.
    fn name_of(program: &str) -> String {
        let file = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| program.to_owned());
        let lower = file.to_lowercase();
        lower
            .strip_suffix(".exe")
            .map(str::to_owned)
            .unwrap_or(lower)
    }

    /// The program and every argument, to open `path` at `line`.
    pub(crate) fn command_line(&self, path: &Path, line: u32) -> (String, Vec<OsString>) {
        let mut arguments: Vec<OsString> = self.arguments.iter().map(OsString::from).collect();
        let at = |separator: &str| {
            let mut spelled = path.as_os_str().to_owned();
            spelled.push(format!("{separator}{line}"));
            spelled
        };
        match self.name().as_str() {
            "code" | "code-insiders" | "codium" | "vscodium" | "cursor" | "windsurf" => {
                arguments.push("--goto".into());
                arguments.push(at(":"));
            }
            "subl" | "sublime_text" | "zed" | "zeditor" => arguments.push(at(":")),
            "idea" | "pycharm" | "webstorm" | "clion" | "goland" | "rustrover" | "rider"
            | "phpstorm" | "studio" => {
                arguments.push("--line".into());
                arguments.push(line.to_string().into());
                arguments.push(path.as_os_str().to_owned());
            }
            _ => {
                arguments.push(format!("+{line}").into());
                arguments.push(path.as_os_str().to_owned());
            }
        }
        (self.program.clone(), arguments)
    }

    /// Starts the editor on `path` at `line`, and does not wait for it.
    ///
    /// Nothing is handed to it on stdin and nothing it prints is read: an
    /// editor with a window of its own has no use for either, and one that
    /// wrote to a pipe nobody reads would block on it. It is waited for on a
    /// thread of its own, so it is reaped when it closes rather than left a
    /// zombie for as long as Crook runs — a thread rather than the background
    /// pool, because an editor window lives for hours and a pool worker held
    /// that long is one every git read is waiting for.
    pub(crate) fn open(&self, path: &Path, line: u32) -> std::io::Result<()> {
        let (program, arguments) = self.command_line(path, line);
        let mut child = crate::process::command(&program)
            .args(arguments)
            .current_dir(path.parent().unwrap_or(path))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}
