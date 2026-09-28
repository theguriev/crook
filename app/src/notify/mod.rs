//! Telling a person who is in another application that a pane wants them.
//!
//! The half of attention that leaves the window. Inside it a waiting pane is
//! an amber row, a count on a chip and a count in front of the title, and a
//! window behind another one also asks the desktop to point at it
//! (`Proxy::request_attention`) — which says "this window" and nothing else.
//! A notification says which tab, and what its agent is asking, to somebody
//! reading a browser on another workspace.
//!
//! # Deciding is the workspace's, posting is a [`Notifier`]'s
//!
//! What is worth a notification is decided where the facts are: the
//! workspace knows whether the window is in front and what each pane's row
//! says, so it is the workspace that sees a row turn to needs-input — on by
//! default — or to failed, or a long command end, both off by default, and
//! only while the window is behind something else (see
//! `Workspace::tell_the_desktop`). What that decision needs and no workspace
//! does is here: the occasions ([`Occasion`]), what a notification says
//! ([`Notice`]), and how often one pane may post ([`Cooldown`]).
//!
//! Posting is behind a trait so that a test hands the workspace a list to post
//! into and a snapshot posts nowhere. The workspace holds [`Silent`] until the
//! window that runs on a desktop hands it [`for_this_desktop`]'s — the same
//! rule the plugins directory follows: a window that found a real notifier
//! for itself would be one every test posted from.
//!
//! # Linux only, in this build
//!
//! There a notification goes to the desktop's notification service over the
//! session bus, by way of `notify-send` — see [`linux`]. macOS delivers
//! notifications only to a signed application bundle, after asking the
//! person, and Windows has never opened a window; both post nothing
//! ([`Silent`]) until each is its own piece of work, and the Notifications
//! page says so rather than offering switches that do nothing.
//!
//! Nothing here reaches a network. The session bus is a socket on this
//! machine, and what crosses it is the tab's name and the agent's question.
//!
//! # A click does not bring the pane forward
//!
//! `notify-send` hears a click only with `--action`, and `--action` implies
//! `--wait`: a process held open per notification for as long as the
//! notification lives, which on a desktop that keeps them in a tray is until
//! somebody clears it, and which nothing kills when Crook quits. A click
//! heard would also have nowhere useful to go. Switching to the pane is
//! easy, but the window would stay behind the application the person is in:
//! winit's `focus_window` does nothing on Wayland, and it has no way to be
//! handed the activation token a server sends with a click. So the title
//! names the tab instead, and the notification carries the `desktop-entry`
//! hint that lets a server tie it to Crook's own entry where one is
//! installed.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crookui_core::executor::Background;

use crate::tab::{AgentSession, AgentStatus, Attention, PaneId, StatusSource};

pub mod linux;

#[cfg(test)]
mod tests;

/// What the title of every notification starts with.
///
/// The name alone, whichever build is running: a person reading a banner is
/// asking which application interrupted them, and "Crook (dev)" is an answer
/// about something else.
pub const APPLICATION: &str = "Crook";

/// How long a pane that posted a notification stays quiet.
///
/// Thirty seconds. An agent whose hooks flap — needs-input, then running on
/// the next tool call, then needs-input again — would otherwise post a banner
/// per flap, and a person who stepped away comes back to a stack of copies of
/// one question. The first one is still on the screen or in the tray saying
/// the pane wants them, so the ones after it within the half-minute say
/// nothing new; and quiet is per pane, so a second agent stopping in another
/// tab is never held up by the first. It is the bell's rule one level up:
/// `terminal_model` drops a bell while the one before it has not been taken
/// (#352), and this drops a notification while the one before it is fresh
/// and nobody has come to the pane since: looking at the pane ends its
/// quiet at once ([`Cooldown::seen`]).
pub const QUIET: Duration = Duration::from_secs(30);

/// How long a command has to have run for its end to be worth a notification.
///
/// Ten seconds. Below it are the commands a person types and watches — a
/// `git status`, an `ls`, a quick test — which are over before anyone has
/// switched away; above it are the builds and the test runs somebody switches
/// away *from*, which is the case this occasion is for. Longer than the
/// two-second sound `crook-dziling` rings, because a banner costs a person
/// more than a chime does.
pub const LONG_COMMAND: Duration = Duration::from_secs(10);

/// How much of an agent's question a notification carries, in characters.
///
/// A banner is a few lines wide and the row shows one: enough for "run `rm
/// -rf build`?" or a sentence about which file, and short enough that a
/// question the agent wrote as a paragraph is still a banner and not a page.
/// The whole of it is on the row.
pub const MESSAGE_CHARS: usize = 120;

/// What a pane did that a person may want to hear about while they are away.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Occasion {
    /// The pane's row turned to needs-input: its agent stopped to ask, or
    /// said it was done while nobody was looking, or a program rang the bell
    /// in a pane that had nothing else to say. The row shows all three the
    /// same way, amber, because each is a pane waiting on a person — and the
    /// bell is how Codex asks in a terminal it does not recognise.
    NeedsInput,
    /// Its agent said it stopped because something went wrong.
    Failed,
    /// A command that ran for at least [`LONG_COMMAND`] ended.
    LongCommand,
}

/// One notification: the line in bold, and the text under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// `Crook — <tab>`, always: see [`title`].
    pub title: String,
    /// What happened, in the pane's own words where it had some.
    pub body: String,
}

impl Notice {
    /// A pane named `name` whose row turned to needs-input, with `session`
    /// as it stands now.
    ///
    /// The body is the agent's question when it asked one — the message the
    /// row shows in place of its second line, cut short — and otherwise what
    /// the row's "Why this status" would say turned it amber: the agent
    /// stopped to ask without saying what, the bell rang, the agent said it
    /// was done, or the command it was running ended without its saying so.
    pub fn needs_input(name: &str, session: &AgentSession) -> Self {
        let question = session
            .message
            .as_deref()
            .map(cut)
            .filter(|question| !question.is_empty());
        let body = match question {
            Some(question) => question,
            None if session.status == AgentStatus::NeedsInput => "Waiting for you".to_owned(),
            None if session.attention == Some(Attention::Bell) => "Rang the bell".to_owned(),
            None => match session.source {
                StatusSource::CommandEnded(_) => "Its command ended".to_owned(),
                StatusSource::Agent(_) | StatusSource::NoReport => {
                    "Done, waiting for a prompt".to_owned()
                }
            },
        };
        Self {
            title: title(name),
            body,
        }
    }

    /// A pane named `name` whose agent said it failed.
    pub fn failed(name: &str) -> Self {
        Self {
            title: title(name),
            body: "Stopped: something went wrong".to_owned(),
        }
    }

    /// A pane named `name` whose command ended with `exit` after `took`.
    pub fn finished(name: &str, exit: Option<i32>, took: Duration) -> Self {
        let took = spoken(took);
        Self {
            title: title(name),
            body: match exit {
                Some(0) | None => format!("A command finished after {took}"),
                Some(status) => format!("A command exited {status} after {took}"),
            },
        }
    }
}

/// A notification's title: Crook, and the tab it is about.
///
/// Always both. A notification names the application because the desktop
/// shows it among everyone else's, and it names the tab because any program
/// in a pane can make its row say it needs input, and a banner whose text is
/// a program's own words must say whose words they are.
pub fn title(name: &str) -> String {
    match name.trim() {
        "" => APPLICATION.to_owned(),
        name => format!("{APPLICATION} — {name}"),
    }
}

/// An agent's question as a notification carries it: on one line, and at
/// most [`MESSAGE_CHARS`] long.
///
/// One line because the row shows it on one, and an agent's message is
/// written for that row; runs of whitespace — a newline, an indent — become
/// one space. Cut at a character rather than a byte, so a question in any
/// script is cut between two letters, and marked with an ellipsis so a cut
/// question does not read as a finished one.
pub fn cut(message: &str) -> String {
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= MESSAGE_CHARS {
        return line;
    }
    let mut kept: String = line.chars().take(MESSAGE_CHARS - 1).collect();
    kept.truncate(kept.trim_end().len());
    kept.push('…');
    kept
}

/// A duration the way a person says one: `42s`, `3m 12s`, `1h 4m`.
fn spoken(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, minutes) => format!("{minutes}m {seconds}s"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

/// Which panes have posted lately, so that a pane which flaps posts once.
///
/// Per pane, and nothing global: one agent asking over and over is the spam
/// this is for, and five agents each stopping once is five things a person
/// wants to know. A pane's quiet ends after [`QUIET`], or sooner when
/// somebody looks at the pane ([`Self::seen`]).
#[derive(Debug, Default)]
pub struct Cooldown {
    /// When each pane last posted, for as long as that is within [`QUIET`].
    posted: HashMap<PaneId, Instant>,
}

impl Cooldown {
    /// Whether `pane` may post at `now` — and, when it may, that it just did.
    ///
    /// Entries are forgotten as they fall out of the window, which is what
    /// keeps a pane that closed from holding one for the rest of the session.
    pub fn admits(&mut self, pane: PaneId, now: Instant) -> bool {
        self.posted
            .retain(|_, posted| now.saturating_duration_since(*posted) < QUIET);
        if self.posted.contains_key(&pane) {
            return false;
        }
        self.posted.insert(pane, now);
        true
    }

    /// Ends `pane`'s quiet, because somebody is looking at it.
    ///
    /// The quiet stands in for a person who has not seen the banner yet.
    /// Once they have come to the pane, whatever it says next is news however
    /// soon it comes: the agent they just answered asking its next question
    /// is the rhythm of a permission prompt, not a flap.
    pub fn seen(&mut self, pane: PaneId) {
        self.posted.remove(&pane);
    }
}

/// Where a [`Notice`] goes: the desktop, nowhere, or a test's list.
pub trait Notifier {
    /// Posts `notice`, without waiting for anything to take it.
    ///
    /// Called on the main thread from the middle of applying what a pane
    /// reported, so whatever takes time — starting a process, waiting on a
    /// bus — goes to `pool`.
    fn post(&self, notice: Notice, pool: &Background);
}

/// The notifier that posts nothing.
///
/// What a workspace holds until it is told otherwise — a test, a snapshot —
/// and what a platform this build does not post on gets.
pub struct Silent;

impl Notifier for Silent {
    fn post(&self, _: Notice, _: &Background) {}
}

/// Whether this build posts notifications on the platform it is running on.
///
/// What the Notifications page asks before it offers a switch: a switch that
/// can change nothing here is drawn without a handler, as every control with
/// nothing to do is.
pub fn posts_here() -> bool {
    cfg!(target_os = "linux")
}

/// The notifier for the desktop this is running on.
///
/// Asked at runtime rather than compiled out, so that the Linux half is built
/// and its tests run on every platform's CI: what it does before the spawn is
/// where the mistakes are, and none of it is Linux's.
pub fn for_this_desktop() -> Rc<dyn Notifier> {
    if posts_here() {
        Rc::new(linux::NotifySend::new())
    } else {
        Rc::new(Silent)
    }
}
