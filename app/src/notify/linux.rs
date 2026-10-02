//! Posting a notification on a Linux desktop, by running `notify-send`.
//!
//! A Linux desktop's notifications are `org.freedesktop.Notifications` on the
//! session bus, and every desktop that shows them at all answers it — GNOME's
//! and KDE's shells, and the daemons a tiling window manager is paired with:
//! dunst, mako, swaync. Talking D-Bus from Rust means a crate, and neither
//! fits: `zbus` is a whole async bus client for one method call, and `dbus`
//! binds libdbus, a system library of the kind the workspace refuses (see
//! `docs/architecture.md`). `notify-send` is libnotify's command for exactly
//! this call, it is installed wherever a notification service is, and a
//! program in another process cannot wedge the window however long the bus
//! takes to answer. It is the bargain `plugins::wasm::sound` makes with
//! `pw-play`, made the same way: started on the pool through
//! [`crate::process::command`], waited on by a thread of its own.
//!
//! # The flags every `notify-send` has
//!
//! The app name, one hint and the two texts, and nothing newer. The flags
//! that print or replace a notification's id, and the ones that wait for a
//! click, are later additions, and a `notify-send` from before them refuses
//! the whole command over a flag it does not know — which is a notification
//! that silently never arrives on a long-term-support desktop. So the per-pane
//! coalescing is Crook's own, in [`Cooldown`](super::Cooldown), rather than
//! the server replacing one banner with the next; and see the module above for
//! why there is no click.
//!
//! # Not installed
//!
//! A machine without `notify-send` — a minimal install, a container — is told
//! so once, in the log, and then left alone: nothing is started again for the
//! rest of the session, so a missing program costs one failed spawn rather
//! than one per question.

use std::io;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crookui_core::executor::Background;

use super::{APPLICATION, Notice, Notifier};

/// The program that posts.
const PROGRAM: &str = "notify-send";

/// The name of the desktop entry a notification says it came from.
///
/// The specification's `desktop-entry` hint, which is how a server finds an
/// application's own name and icon for a banner, groups its notifications,
/// and ties them to its window. `crook`, because that is the one name a Linux
/// desktop knows Crook by: the Wayland app id and X11 class crookui gives the
/// window, and the `crook.desktop` a package installs.
const DESKTOP_ENTRY: &str = "crook";

/// Posts through `notify-send`.
pub struct NotifySend {
    /// What to run: [`PROGRAM`], except in a test that needs a program that
    /// is not there.
    program: &'static str,
    /// Whether the program turned out not to be installed.
    ///
    /// Shared with the pool tasks, which are what find out: once one has, the
    /// main thread stops handing them anything.
    missing: Arc<AtomicBool>,
}

impl NotifySend {
    /// A notifier that runs `notify-send` from `PATH`.
    pub fn new() -> Self {
        Self::running(PROGRAM)
    }

    /// The same, running `program` in its place.
    fn running(program: &'static str) -> Self {
        Self {
            program,
            missing: Arc::default(),
        }
    }
}

impl Default for NotifySend {
    fn default() -> Self {
        Self::new()
    }
}

impl Notifier for NotifySend {
    fn post(&self, notice: Notice, pool: &Background) {
        if self.missing.load(Ordering::Relaxed) {
            return;
        }
        let program = self.program;
        let missing = self.missing.clone();
        pool.spawn(async move {
            deliver(program, &notice, &missing);
        })
        .detach();
    }
}

/// What became of one notification.
#[derive(Debug, PartialEq, Eq)]
enum Delivery {
    /// The program started; the thread waiting on it has it now.
    Started,
    /// The program is not installed, and nothing will be tried again.
    Missing,
    /// It was already known not to be installed, so nothing was tried.
    Skipped,
    /// It is installed and would not start, which is worth a line in the
    /// debug log and not a reason to stop trying.
    Failed,
}

/// Starts `program` for `notice`, unless it is known not to be installed.
///
/// On the pool: starting a process is a fork, which is milliseconds of a
/// window's main thread that it has no reason to spend.
fn deliver(program: &str, notice: &Notice, missing: &AtomicBool) -> Delivery {
    if missing.load(Ordering::Relaxed) {
        return Delivery::Skipped;
    }
    let started = crate::process::command(program)
        .args(arguments(notice))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match started {
        Ok(child) => {
            // Not waited on here: the bus answers or gives up within its
            // reply timeout, and `notify-send` exits either way — it holds
            // nothing open without `--wait`, which is never passed. A
            // non-zero exit is most often no notification daemon on the bus,
            // which the missing banner has already said.
            crate::process::reap(child, PROGRAM);
            Delivery::Started
        }
        Err(why) if why.kind() == io::ErrorKind::NotFound => {
            // The swap is what makes it once: two questions in flight can both
            // find the program missing, and only the first to say so is heard.
            if !missing.swap(true, Ordering::Relaxed) {
                log::warn!(
                    "there is no {program} on this machine, so Crook posts no desktop \
                     notifications; it comes with libnotify"
                );
            }
            Delivery::Missing
        }
        Err(why) => {
            log::debug!("{program} could not be started: {why}");
            Delivery::Failed
        }
    }
}

/// What `notify-send` is told for `notice`.
///
/// `--` before the two texts, because they are a pane's own words — an
/// agent's question can start with a dash — and after it nothing is read as
/// a flag.
fn arguments(notice: &Notice) -> Vec<String> {
    vec![
        format!("--app-name={APPLICATION}"),
        format!("--hint=string:desktop-entry:{DESKTOP_ENTRY}"),
        "--".to_owned(),
        notice.title.clone(),
        escaped(&notice.body),
    ]
}

/// The body with the three characters markup gives a meaning written as the
/// entities for them, and every backslash doubled.
///
/// The specification lets a server read a body as a small subset of HTML,
/// and the ones in use do. The body is a program's words, so its `<a href>`
/// would be a link in a banner with Crook's name on it, and its bare `&` —
/// "build && test" — is markup that does not parse. The title is never read
/// as markup and is left alone. A server that does not read markup shows the
/// entities as written, which is the rarer and the cheaper of the two
/// mistakes.
///
/// The backslashes are `notify-send`'s own: it reads the body, and not the
/// title, through `g_strcompress`, which takes C escapes out of it. A
/// question with a Windows path or a regex in it — `C:\new`, `\bfoo\b` —
/// would arrive with a line break, a lost backslash, or cut short at a
/// `\0`; `\\` is the one escape that gives back the backslash that was
/// written.
fn escaped(body: &str) -> String {
    let mut escaped = String::with_capacity(body.len());
    for character in body.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\\' => escaped.push_str(r"\\"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
#[path = "linux_tests.rs"]
mod tests;
