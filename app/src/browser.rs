//! Handing a URL to whatever the machine opens URLs with.
//!
//! Three commands, one per platform, and nothing else. The crates that do this
//! are small and good, and they are still a dependency for a `Command::new`
//! and a match on `cfg!(target_os)` — which is the same trade `crate::process`
//! already made for `CREATE_NO_WINDOW`, and the same one the whole workspace
//! makes about build scripts.
//!
//! Nothing waits for the child. A browser cold-starting takes seconds, and the
//! frame that dispatched the click must not be one of them.
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

use std::io;

use crate::process::command;

/// The schemes a printed link is allowed to open.
///
/// The same list `crook_terminal::url` recognises, and deliberately so: this
/// is the second half of one decision, and a scheme that could be *found* but
/// not opened would be a link that underlines and then does nothing.
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

    if let Err(error) = spawn(url) {
        log::warn!("could not open {url:?}: {error}");
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

/// Starts the platform's handler and does not wait for it.
fn spawn(url: &str) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut launcher = {
        let mut launcher = command("open");
        launcher.arg(url);
        launcher
    };

    #[cfg(target_os = "windows")]
    let mut launcher = {
        // `start` is a builtin of the shell rather than a program, so it needs
        // one. The empty string is the window title `start` takes as its first
        // quoted argument — without it a quoted URL becomes the title and
        // nothing opens.
        let mut launcher = command("cmd");
        launcher.args(["/C", "start", "", url]);
        launcher
    };

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut launcher = {
        let mut launcher = command("xdg-open");
        launcher.arg(url);
        launcher
    };

    launcher
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scheme_the_terminal_finds_is_one_this_will_open() {
        // The two lists are one decision. A scheme that could be found but not
        // opened would be a link that underlines under the pointer and then
        // does nothing when it is clicked.
        for scheme in OPENABLE {
            assert!(
                is_openable(&format!("{scheme}example")),
                "{scheme} is recognised and would not be opened"
            );
        }
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
    fn a_scheme_in_capitals_is_still_the_scheme() {
        assert!(is_openable("HTTPS://example.com"));
        assert!(is_openable("MailTo:someone@example.com"));
    }
}
