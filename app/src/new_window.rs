//! Another window, which means another Crook.
//!
//! A Crook process has one window. `crookui::run` opens it and returns when
//! it closes, so a second window is a second process: this binary started
//! again. Before this, starting Crook again from a launcher, a shell or `open
//! -n` was the only way to get a second window, and on macOS a click on the
//! Dock brings the running window forward instead. So a Mac had no way to
//! open one from inside Crook.
//!
//! The window is started with `--working-directory` set to the directory the
//! person was working in. That makes it a launcher's window as far as
//! [`Overrides::launched`](crate::Overrides::launched) is concerned. It opens fresh, never writes
//! the session file, and never takes the lock that says which window keeps
//! it. So the window the command was pressed in keeps its tabs, whichever
//! window that is, and nobody is told about a session they did not ask for.
//!
//! Nothing waits for the window. The child is reaped on a thread of its own
//! when it closes, hours later or never — see [`crate::process::reap`] — and
//! on Unix it is put in a process group of its own, so a Ctrl-C or a hangup
//! aimed at the shell the first Crook was started from does not close the
//! window it opened.

use std::ffi::OsString;
use std::io;
use std::path::Path;

/// The arguments that open another window whose shells start in
/// `directory`.
pub(crate) fn arguments(directory: &Path) -> Vec<OsString> {
    vec!["--working-directory".into(), directory.into()]
}

/// Opens another window whose shells start in `directory`, and does not wait
/// for it.
///
/// An error when there is no binary to start or the system refused to start
/// it. Nothing else can be reported from here: a window that opens and then
/// fails says so in its own log.
pub fn open(directory: &Path) -> io::Result<()> {
    let binary = crate::shell_integration::running_binary().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Crook cannot tell where its own binary is",
        )
    })?;
    let arguments = arguments(directory);

    #[cfg(test)]
    if OPENED
        .with_borrow_mut(|opened| {
            opened.as_mut().map(|opened| {
                opened.push(
                    std::iter::once(OsString::from(binary))
                        .chain(arguments.iter().cloned())
                        .collect(),
                )
            })
        })
        .is_some()
    {
        return Ok(());
    }

    let mut launcher = crate::process::command(binary);
    launcher
        .args(&arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        launcher.process_group(0);
    }
    let child = launcher.spawn()?;
    crate::process::reap(child, "window");
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// The command lines this thread has asked to open, once
    /// [`record_opens`] asked for them to be written down rather than run.
    static OPENED: std::cell::RefCell<Option<Vec<Vec<OsString>>>> =
        const { std::cell::RefCell::new(None) };
}

/// From now on, writes down the windows this thread opens instead of opening
/// them.
///
/// For a test that presses New window. What it checks is that the press got
/// here with the arguments it should have. What it must not do is open a
/// window on the desktop of whoever runs the suite, from a test binary that
/// would run its tests again instead. Kept per thread, because a press in the
/// window's test harness runs on the test's own thread and the suite runs its
/// tests in parallel.
#[cfg(test)]
pub(crate) fn record_opens() {
    OPENED.with_borrow_mut(|opened| *opened = Some(Vec::new()));
}

/// What this thread has opened since [`record_opens`], each as the program
/// followed by its arguments.
#[cfg(test)]
pub(crate) fn opened() -> Vec<Vec<OsString>> {
    OPENED.with_borrow(|opened| opened.clone().unwrap_or_default())
}
