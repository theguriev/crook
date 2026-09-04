//! What Crook was showing when it was last closed, and the file it is
//! remembered in.
//!
//! # Why these types exist at all
//!
//! Nothing here is a live type. [`Tab`](crate::tab::Tab) holds a
//! [`PaneGroup`](crate::tab::PaneGroup) holding [`Pane`](crate::tab::Pane)s
//! holding [`AgentSession`](crate::tab::AgentSession)s, and every one of them
//! carries identity — a [`PaneId`](crate::tab::PaneId) minted from a
//! process-wide counter — that means nothing in a later process. Beside them,
//! in the workspace rather than in the model, sit mouse states, drag gestures,
//! scroll offsets and terminal handles, none of which is a fact about what was
//! open.
//!
//! So a snapshot is a separate shape, and it is deliberately thin: a title, a
//! directory, a share of a split. Serialising the live types would mean
//! deriving `Serialize` on everything they reach and then remembering, every
//! time one of them grew a field, whether that field was a fact about the work
//! or a fact about a pointer. Warp draws the line in the same place and for the
//! same reason.
//!
//! # What is not remembered, and why
//!
//! **Not the scrollback, and not what any command printed.** A terminal
//! session is a process; the process is gone. What comes back is a shell in
//! the directory the old one was in, which is the useful half and the only
//! honest one — a window that redrew yesterday's output over a shell that had
//! never run any of it would be lying about the state of the machine.
//!
//! **Not the settings pane.** It is a pane, so it *could* be, and it should not
//! be: it is a thing somebody opened to change a setting, and a window that
//! came back with it still open would be answering a question nobody asked
//! twice.
//!
//! # Nothing here can cost a person their window
//!
//! Exactly the rule [`crate::settings`] follows. Every failure — no
//! configuration directory, no file, a file that cannot be read, JSON that is
//! not the shape this build expects — is one line in the log and a fresh
//! window with one tab in it. A session file is a convenience, and a
//! convenience that could refuse to start would be a bug.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::settings::{atomic_write, config_directory};
use crate::tab::{Pane, PaneGroup, SplitAxis, Tab, TabStrip};

/// The file the last session is remembered in.
const SESSION_FILE: &str = "session.json";

/// The most tabs and panes a session file is allowed to restore.
///
/// A file this build did not write — hand-edited, or truncated by a full disk
/// and then repaired by something else — must not be able to make Crook open
/// ten thousand ptys before drawing its first frame. It is a bound rather than
/// a validation because there is no correct number: any file naming more than
/// this is one nobody meant.
const MAX_TABS: usize = 64;
/// See [`MAX_TABS`].
const MAX_PANES: usize = 16;

/// The window as it was, and every tab in it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// The tabs, in bar order.
    pub tabs: Vec<TabSnapshot>,
    /// Which of them was active, as a position in `tabs`.
    ///
    /// A position rather than an id, because an id is a fact about the process
    /// that wrote it. Out of range is clamped rather than refused: it names
    /// the tab that would have been selected, and the first tab is a better
    /// answer than no window.
    pub active: usize,
    /// How big the window was, in logical pixels.
    ///
    /// `None` for a session written before there was a window to measure —
    /// which is what a `--snapshot` run leaves — and for a file that predates
    /// this key.
    pub window: Option<[f32; 2]>,
}

/// One tab: what it was called, and the panes it held.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabSnapshot {
    /// The name the tab was created with. Not its derived title: that came
    /// from a shell that no longer exists.
    pub name: String,
    /// Whether the panes were divided left-to-right rather than top-to-bottom.
    pub horizontal: bool,
    /// Which pane had the keyboard, as a position in `panes`.
    pub focused: usize,
    /// The panes, in render order.
    pub panes: Vec<PaneSnapshot>,
}

/// One pane: where its shell was, and how much of the split it took.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneSnapshot {
    /// The name the session was created with.
    pub title: String,
    /// Where the shell was working, as the shell last reported it.
    ///
    /// This is the whole point of the file: a window that comes back with its
    /// shells in the directories they were in is a window somebody can carry
    /// on working in.
    pub working_directory: Option<PathBuf>,
    /// The share of the split this pane took, from a dragged divider.
    pub flex: f32,
}

impl Session {
    /// Whether there is anything worth restoring.
    ///
    /// A session with no tabs is one written by something that had none, and
    /// restoring it would be opening a window with no panes — which the strip
    /// refuses to represent anyway.
    pub fn is_empty(&self) -> bool {
        self.tabs.iter().all(|tab| tab.panes.is_empty())
    }

    /// The window size to open at, when the file named one that makes sense.
    ///
    /// A size of zero, a negative one or a `NaN` is a file this build did not
    /// write; the caller's own default is a better answer than a window
    /// nobody can see.
    pub fn window_size(&self) -> Option<[f32; 2]> {
        let size = self.window?;
        (size[0].is_finite() && size[0] >= 1. && size[1].is_finite() && size[1] >= 1.)
            .then_some(size)
    }

    /// Records what a strip is showing.
    ///
    /// The settings pane is left out, and with it any tab that held nothing
    /// else — see the module docs.
    pub fn of(strip: &TabStrip, window: Option<[f32; 2]>) -> Self {
        let mut tabs = Vec::with_capacity(strip.len());
        let mut active = 0;

        for tab in strip.iter() {
            let panes: Vec<_> = tab
                .panes()
                .iter()
                .filter(|pane| !pane.is_settings())
                .map(PaneSnapshot::of)
                .collect();
            if panes.is_empty() {
                continue;
            }

            if strip.is_active(tab.id()) {
                active = tabs.len();
            }
            // The focused pane's position among the ones that were *kept*,
            // which is not its position in the tab once a settings pane has
            // been dropped out of the middle.
            let focused = tab
                .panes()
                .iter()
                .filter(|pane| !pane.is_settings())
                .position(|pane| tab.panes().is_focused(pane.id()))
                .unwrap_or(0);

            tabs.push(TabSnapshot {
                name: tab.name().to_owned(),
                horizontal: tab.panes().axis() == SplitAxis::Horizontal,
                focused,
                panes,
            });
        }

        Self {
            tabs,
            active,
            window,
        }
    }

    /// Builds the strip this session describes, or `None` when it describes
    /// nothing.
    ///
    /// Every id in the result is minted fresh. Nothing about a previous
    /// process's identities is carried across, which is what makes a restored
    /// window indistinguishable from one somebody opened by hand.
    pub fn restore(&self) -> Option<TabStrip> {
        let mut strip = TabStrip::empty();
        let mut opened: usize = 0;

        for snapshot in self.tabs.iter().take(MAX_TABS) {
            let Some(tab) = snapshot.restore() else {
                continue;
            };
            strip.adopt(tab);
            opened += 1;
        }
        if opened == 0 {
            return None;
        }

        // Clamped rather than refused: the index names the tab that would have
        // been selected, and the first one is a better answer than no window.
        strip.select_index(self.active.min(opened.saturating_sub(1)));
        Some(strip)
    }

    /// Reads the session file, defaulting past anything unusable.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let Ok(text) = fs::read_to_string(path) else {
            // The ordinary case on a first run, so not worth a line.
            return Self::default();
        };

        match serde_json::from_str(&text) {
            Ok(session) => session,
            Err(error) => {
                log::warn!("could not read {}: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Reads the per-user session file.
    pub fn for_user() -> Self {
        match user_session_path() {
            Some(path) => Self::load(path),
            None => Self::default(),
        }
    }

    /// Writes the session to `path`, atomically.
    ///
    /// Blocking, and belongs on the background pool for the reason
    /// [`Settings::save_blocking`](crate::settings::Settings::save_blocking)
    /// does: it creates a directory, writes a file and renames it.
    pub fn save_blocking(&self, path: &Path) -> Result<()> {
        let mut json =
            serde_json::to_string_pretty(self).context("could not serialize the session")?;
        json.push('\n');

        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)
                .with_context(|| format!("could not create {}", directory.display()))?;
        }
        atomic_write(path, json.as_bytes())
    }
}

impl TabSnapshot {
    /// Builds the tab this describes, or `None` when it has no panes.
    fn restore(&self) -> Option<Tab> {
        let mut panes = self.panes.iter().take(MAX_PANES);
        let first = panes.next()?;

        let mut tab = Tab::new(if self.name.is_empty() {
            first.title.clone()
        } else {
            self.name.clone()
        });
        first.apply_to_first(tab.panes_mut());

        let direction = if self.horizontal {
            crate::tab::Direction::Right
        } else {
            crate::tab::Direction::Down
        };
        for pane in panes {
            // Splitting always inserts beside the *focused* pane and focuses
            // the new one, so splitting repeatedly in one direction rebuilds
            // the row in order without this having to know how the group
            // stores it.
            tab.panes_mut().split(direction, pane.title.clone());
            pane.apply_to_focused(tab.panes_mut());
        }

        let ids: Vec<_> = tab.panes().iter().map(Pane::id).collect();
        if let Some(id) = ids.get(self.focused.min(ids.len().saturating_sub(1))) {
            tab.panes_mut().focus(*id);
        }
        Some(tab)
    }
}

impl PaneSnapshot {
    /// Records one pane.
    fn of(pane: &Pane) -> Self {
        let session = pane.session();
        Self {
            title: session.map_or_else(|| pane.title().to_owned(), |s| s.title.clone()),
            working_directory: session.and_then(|s| s.working_directory.clone()),
            flex: pane.flex(),
        }
    }

    /// Writes this snapshot's facts onto a group's only pane.
    fn apply_to_first(&self, group: &mut PaneGroup) {
        let Some(id) = group.iter().next().map(Pane::id) else {
            return;
        };
        self.apply(group, id);
    }

    /// The same, onto whichever pane a split has just focused.
    fn apply_to_focused(&self, group: &mut PaneGroup) {
        let id = group.focused_id();
        self.apply(group, id);
    }

    fn apply(&self, group: &mut PaneGroup, id: crate::tab::PaneId) {
        if let Some(pane) = group.get_mut(id) {
            pane.set_flex(self.flex);
            if let Some(session) = pane.session_mut() {
                // Only a directory that still exists. A repository moved or
                // deleted between two launches would otherwise start a shell
                // in a directory that is not there, which most shells answer
                // by starting in `/` — a worse answer than the one Crook was
                // started from.
                if self
                    .working_directory
                    .as_ref()
                    .is_some_and(|path| path.is_dir())
                {
                    session.working_directory = self.working_directory.clone();
                }
            }
        }
    }
}

/// Where the per-user session file lives.
pub fn user_session_path() -> Option<PathBuf> {
    Some(config_directory()?.join(SESSION_FILE))
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
