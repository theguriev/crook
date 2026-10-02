//! Handing a URL to whatever the machine opens URLs with — and a folder of
//! Crook's own to the file manager, which is the same three commands.
//!
//! Three commands, one per platform, and nothing else. The crates that do this
//! are small and good, and they are still a dependency for a `Command::new`
//! and a match on `cfg!(target_os)` — which is the same trade `crate::process`
//! already made for `CREATE_NO_WINDOW`, and the same one the whole workspace
//! makes about build scripts.
//!
//! Nothing on the frame waits for the child. A browser cold-starting takes
//! seconds, and the frame that dispatched the click must not be one of them.
//! A thread of its own waits instead, so the launcher is reaped when it exits
//! rather than left a zombie for as long as Crook runs — one per link
//! anybody ever clicked.
//!
//! # What is refused
//!
//! Only what the terminal itself put on screen reaches here, but "on screen"
//! includes whatever a program printed, and a program's output is not
//! trustworthy: a `curl` of somebody else's server, a log line quoting a
//! request, a filename in a repository. So the scheme is checked against a
//! list rather than passed through. The one that matters is that nothing can
//! ask the platform to open a scheme a handler has been registered for —
//! `ms-msdt:`, `search-ms:`, an application's own registered scheme — by
//! printing it into somebody's terminal.

use std::ffi::OsStr;
use std::io;
use std::path::Path;

use crate::process::command;

/// The schemes a printed link is allowed to open.
///
/// The same list `crook_terminal::url::SCHEMES` recognises, and deliberately
/// so: this is the second half of one decision, and a scheme that could be
/// *found* but not opened would be a link that underlines and then does
/// nothing. A test holds the two lists equal.
const OPENABLE: [&str; 8] = [
    "https://", "http://", "ftps://", "ftp://", "file://", "ssh://", "git://", "mailto:",
];

/// Opens `url` with the platform's own handler, reporting whether the attempt
/// was made.
///
/// `false` means the URL was refused before anything was started — a scheme
/// that is not on the list — which is never worth showing anybody an error
/// for. A handler that fails *after* starting is the platform's business and
/// is not reported at all: there is nothing useful to say about somebody's
/// browser failing to launch, and nothing to be done about it here.
pub fn open(url: &str) -> bool {
    if !is_openable(url) {
        log::debug!("refusing to open {url:?}: not a scheme a printed link may open");
        return false;
    }

    if let Err(error) = spawn(url.as_ref()) {
        log::warn!("could not open {url:?}: {error}");
    }
    true
}

/// Shows `folder` in the platform's file manager, reporting whether the
/// attempt was made.
///
/// For the folders Crook itself writes into — its logs and crash reports —
/// and never for anything a program printed, which is what [`open`] and its
/// list of schemes are for. A path rather than a `file://` URL, which would
/// need every space and `%` in a home directory escaped to mean the same
/// folder. Only a folder that exists is handed over: `explorer` given a
/// program runs it, and `open` given an application launches it, so a path
/// that is not a folder is refused here rather than trusted there.
pub fn open_folder(folder: &Path) -> bool {
    if !folder.is_dir() {
        log::debug!("refusing to open {}: not a folder", folder.display());
        return false;
    }

    if let Err(error) = spawn(folder.as_os_str()) {
        log::warn!("could not open {}: {error}", folder.display());
    }
    true
}

/// Whether a URL is one a printed link may open.
fn is_openable(url: &str) -> bool {
    // Case-insensitively, because a scheme is, and because the alternative is
    // a filter `HTTPS://` walks straight through.
    let lowered = url.to_ascii_lowercase();
    OPENABLE.iter().any(|scheme| lowered.starts_with(scheme))
}

/// Starts the platform's handler, and leaves a thread waiting on it.
///
/// A thread rather than the background pool, for `changes_panel::launch`'s
/// reason: `xdg-open` given a URL with no browser running can stay until the
/// browser it started exits, and a pool worker held that long is one every
/// git read is waiting for.
fn spawn(url: &OsStr) -> io::Result<()> {
    #[cfg(test)]
    if OPENED
        .with_borrow_mut(|opened| {
            opened
                .as_mut()
                .map(|opened| opened.push(url.to_string_lossy().into_owned()))
        })
        .is_some()
    {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    let mut launcher = {
        let mut launcher = command("open");
        launcher.arg(url);
        launcher
    };

    #[cfg(target_os = "windows")]
    let mut launcher = {
        // `explorer`, not `cmd /C start`. `start` is a shell builtin, so it
        // needs `cmd`, and `cmd` re-parses its command line: a URL a program
        // printed can carry an `&` — legal in a query string, and something
        // `crook_terminal::url` keeps inside a link — which `cmd` reads as a
        // command separator and runs what follows. `std` only quotes an
        // argument that holds a space, and a URL holds none, so the `&` would
        // reach `cmd` bare. `explorer` is a program: it is handed the URL as
        // one argument through `CreateProcess`, with no shell between them to
        // split it, and opens it with the same handler `start` would have.
        let mut launcher = command("explorer");
        launcher.arg(url);
        launcher
    };

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut launcher = {
        let mut launcher = command("xdg-open");
        launcher.arg(url);
        launcher
    };

    let mut child = launcher
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let reaper = std::thread::Builder::new()
        .name("crook-opener".to_owned())
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(why) = reaper {
        log::debug!("nothing could wait on the opener: {why}");
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// What this thread has asked to open, once [`record_opens`] asked for it
    /// to be written down rather than handed to a browser.
    static OPENED: std::cell::RefCell<Option<Vec<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// From now on, writes down what this thread opens instead of opening it.
///
/// For a test that presses a link: the thing it asserts is that the press
/// reached here with the address it should have, and the thing it must not
/// do is start the browser of whoever runs the suite. A thread's worth,
/// because a press in the window's test harness is dispatched on the test's
/// own thread and the suite runs its tests in parallel.
#[cfg(test)]
pub(crate) fn record_opens() {
    OPENED.with_borrow_mut(|opened| *opened = Some(Vec::new()));
}

/// What this thread has opened since [`record_opens`].
#[cfg(test)]
pub(crate) fn opened() -> Vec<String> {
    OPENED.with_borrow(|opened| opened.clone().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scheme_the_terminal_finds_is_one_this_will_open() {
        // The two lists are one decision, kept in two crates. A scheme the
        // terminal recognises but this refuses is a link that underlines
        // under the pointer and does nothing when it is clicked; one this
        // would open but the terminal never finds is a scheme no printed link
        // can carry. So the sets must be equal — and checked against the
        // terminal's own list, not against this one, which would only prove
        // it equals itself.
        let mut found = crook_terminal::url::SCHEMES.to_vec();
        let mut opens = OPENABLE.to_vec();
        found.sort_unstable();
        opens.sort_unstable();
        assert_eq!(
            found, opens,
            "the schemes the terminal finds and the ones the browser opens have drifted apart"
        );
    }

    #[test]
    fn a_scheme_nobody_printed_on_purpose_is_refused() {
        // A program's output is not trustworthy: this is what stops a `curl`
        // of somebody else's server from asking the platform to open a scheme
        // an application has registered a handler for.
        for url in [
            "ms-msdt:/id",
            "search-ms:query=x",
            "javascript:alert(1)",
            "data:text/html,<script>",
            "vscode://file/etc/passwd",
            "",
            "example.com",
        ] {
            assert!(!is_openable(url), "{url:?} should not be openable");
            assert!(!open(url), "{url:?} was handed to the platform");
        }
    }

    #[test]
    fn a_query_string_is_a_url_and_is_not_refused() {
        // The `&` a link carries is defended against by the launcher on
        // Windows — a program is handed the URL whole rather than a shell
        // re-parsing it — not by refusing the character here, which would
        // turn away most of the addresses anyone actually clicks.
        assert!(is_openable("https://example.com/search?q=rust&hl=en"));
        assert!(is_openable("https://example.com/a?x=1&y=2"));
    }

    #[test]
    fn a_path_that_is_not_a_folder_is_never_handed_to_the_platform() {
        // `explorer` given a program runs it, and `open` given an application
        // launches it: the one thing the folder opener must never be is a way
        // to start a file.
        let this_binary = std::env::current_exe().expect("the test binary knows where it is");
        let nowhere = std::env::temp_dir().join("crook-browser-a-folder-that-is-not-there");

        assert!(!open_folder(&this_binary), "a program was handed over");
        assert!(!open_folder(&nowhere), "a missing folder was handed over");
    }

    #[test]
    fn a_scheme_in_capitals_is_still_the_scheme() {
        assert!(is_openable("HTTPS://example.com"));
        assert!(is_openable("MailTo:someone@example.com"));
    }
}
