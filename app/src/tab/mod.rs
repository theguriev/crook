//! The tab model: what a tab is, and every legal mutation of the strip.
//!
//! Nothing here draws, and nothing here names a UI type. That is the point:
//! the cases that actually break a tabbed terminal — closing the active tab,
//! closing the one before it, moving the first tab further left — are tested
//! in microseconds with no window, no GPU and no fonts.
//!
//! Two decisions shape everything below, and both are answers to bugs Warp is
//! living with today.
//!
//! **Actions carry a [`TabId`], never a position.** Warp's tab actions carry a
//! `usize` captured in a render closure, and its drag handler carries a
//! twenty-five line comment about what follows: a mutation between render and
//! event delivery leaves that index pointing at an innocent neighbour. One
//! `index_of` lookup over a vector of fewer than twenty entries removes the
//! entire class of bug, and it costs nothing at this size.
//!
//! **The vector is private and there is exactly one mutation path.** Warp's is
//! `pub(crate)` and is read and written directly across 29k lines of one file,
//! which is how it grew four separately-written and mutually-inconsistent
//! repairs of the active index. Here every mutation ends in one private
//! helper that re-finds the active tab *by identity*.
//!
//! A tab holds a [`PaneGroup`] rather than a session, so the same two rules
//! hold one level down: the `pane` module is the strip's shape again, over
//! panes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::settings::Granularity;

mod pane;

#[cfg(test)]
mod tests;

pub use pane::{
    Direction, Pane, PaneContent, PaneEffect, PaneGroup, PaneId, SETTINGS_TITLE, SplitAxis,
};

/// A tab's identity, stable for as long as the tab exists.
///
/// Ids are minted from a process-wide counter and never reused, so an action
/// naming a tab that has since been closed resolves to nothing rather than to
/// whoever moved into its slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u64);

impl TabId {
    /// Mints an id no other tab in this process will ever have.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// What the agent behind a tab is doing.
///
/// A tab of agents wants a status at a glance far more than a tab of shells
/// does, so this drives a dot in the tab bar. It is derived once, when the
/// agent reports, and never recomputed by the renderer: Warp's indicator
/// ladder walks the whole conversation on every tab on every frame.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AgentStatus {
    /// Waiting for a prompt.
    #[default]
    Idle,
    /// Working, with no attention needed.
    Running,
    /// Stopped, waiting on a person — an approval or an answer.
    NeedsInput,
    /// Stopped because something went wrong.
    Failed,
}

impl AgentStatus {
    /// A word for this status, for the tab body's status line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::NeedsInput => "needs input",
            Self::Failed => "failed",
        }
    }
}

/// The agent session a pane is a window onto.
///
/// Warp's tab points at a `ViewHandle<PaneGroup>` whose panes hold views,
/// which is why its tab struct has no title and every accessor threads a
/// context. A Crook pane is exactly one agent, so the session is owned inline
/// and read directly.
///
/// It deliberately has no id of its own. Carrying one would mean carrying the
/// invariant that it equals its pane's, and a session is reachable as `&mut`
/// from the agent-progress path — so the invariant would be one field
/// assignment away from being false, and `index_of` would then resolve to the
/// wrong pane. One identity, owned by [`Pane`], cannot disagree with itself.
#[derive(Debug)]
pub struct AgentSession {
    /// The name the session was created with. Never empty.
    pub title: String,
    /// What the agent has called its own work, once it has called it
    /// something. This is what makes a tab of agents rename itself with no
    /// rename plumbing at all.
    pub derived_title: Option<String>,
    /// What the agent is doing right now.
    pub status: AgentStatus,
    /// Where the agent is working.
    ///
    /// Seeded from the process's own directory, because that is where a
    /// session started from Crook actually runs and there is nothing else true
    /// to say yet. An agent runtime that chooses a directory sets this, and
    /// every git fact a row shows is looked up by it.
    ///
    /// `None` only when the process has no readable working directory at all —
    /// a directory deleted out from under it — in which case a row falls back
    /// to the session title.
    pub working_directory: Option<PathBuf>,
    /// The pull request this session's work belongs to, as a URL.
    ///
    /// **Nothing populates this yet, and that is deliberate.** Warp's PR link
    /// comes out of `gh pr view` — a subprocess with a five-second timeout, an
    /// authentication state and a whole failure taxonomy — and Crook has no
    /// forge integration to put behind it. The field exists so the row and the
    /// "Show: PR link" toggle are written against real data rather than a
    /// placeholder: the toggle governs whether the slot appears *when there is
    /// a link*, and today there never is. The menu says so on screen rather
    /// than leaving a dead chip to be discovered.
    pub pull_request: Option<String>,
}

impl AgentSession {
    /// A fresh idle session named `title`, working where Crook is.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            derived_title: None,
            status: AgentStatus::default(),
            working_directory: std::env::current_dir().ok(),
            pull_request: None,
        }
    }

    /// What the tab bar should print: the agent's own name for its work, or
    /// the name the session was created with until it has one.
    pub fn display_title(&self) -> &str {
        self.derived_title.as_deref().unwrap_or(&self.title)
    }

    /// What a pull-request chip says: `PR #123`, or the raw URL when the number
    /// cannot be read out of it.
    ///
    /// Warp's `github_pr_display_text_from_url`, rule for rule — split on
    /// `/pull/`, take everything up to the next delimiter, require it to be a
    /// positive run of digits. Showing the URL when that fails beats showing
    /// nothing: a link nobody can label is still a link somebody can follow.
    pub fn pull_request_label(&self) -> Option<String> {
        let url = self.pull_request.as_deref()?.trim();
        if url.is_empty() {
            return None;
        }

        let number = url
            .rsplit_once("/pull/")
            .map(|(_, tail)| tail.split(['/', '?', '#']).next().unwrap_or_default())
            .filter(|number| !number.is_empty())
            .filter(|number| number.bytes().all(|byte| byte.is_ascii_digit()))
            .filter(|number| number.parse::<u64>().is_ok_and(|number| number > 0));

        Some(match number {
            Some(number) => format!("PR #{number}"),
            None => url.to_owned(),
        })
    }
}

/// One tab: an identity and the panes it shows.
///
/// Deliberately tiny. It owns no hover state, no drag state and no active
/// flag — hover belongs to the view that renders it and is handed back each
/// frame, and activeness belongs to the strip, so "two tabs are active" is not
/// a representable state. This is Warp's `TabData`, which is likewise a pane
/// group and a handful of visual scraps.
///
/// Both fields are private, and that is load-bearing rather than tidy: a
/// public `id` is settable through the `&mut Tab` the agent-progress path
/// hands out, and two tabs sharing an id makes `index_of` resolve a close to
/// somebody else's tab.
///
/// The name is the tab's own and is fixed at birth. Warp's `custom_title` is
/// a rename a person performs, which Crook has no flow for; what a tab needs
/// even without one is a name that does not move as focus moves inside it,
/// because the panel's group header is what names a tab whose rows name its
/// panes. Deriving that header from the focused pane would rewrite the heading
/// every time someone clicked a row underneath it.
#[derive(Debug)]
pub struct Tab {
    id: TabId,
    name: String,
    panes: PaneGroup,
}

impl Tab {
    /// A tab over one new session named `title`.
    pub fn new(title: impl Into<String>) -> Self {
        let title = title.into();
        Self {
            id: TabId::next(),
            name: title.clone(),
            panes: PaneGroup::new(title),
        }
    }

    /// A tab over the settings page.
    ///
    /// Named the same as its pane, so the strip says "Settings" whether it is
    /// drawing tabs or panes and whether or not the tab has since been split.
    pub fn settings() -> Self {
        Self {
            id: TabId::next(),
            name: SETTINGS_TITLE.to_owned(),
            panes: PaneGroup::settings(),
        }
    }

    /// This tab's identity, for as long as it is open.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// What the tab is called, whatever its panes are called.
    ///
    /// The name of the session it was opened for, which stays put when that
    /// session is split away, closed, or renamed by its agent. Warp's
    /// `should_show_tab_group_header` puts this above a tab's rows exactly
    /// when the rows cannot speak for the tab — for Crook, when the tab holds
    /// more than one pane.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The panes it holds, in render order. Never empty.
    pub fn panes(&self) -> &PaneGroup {
        &self.panes
    }

    /// The panes, for splitting, closing and focusing them.
    ///
    /// Crate-private for the same reason as [`Pane::session_mut`]: everything
    /// here is drawn, so the way in from outside is `TabStrip::apply`.
    pub(crate) fn panes_mut(&mut self) -> &mut PaneGroup {
        &mut self.panes
    }

    /// What to print in the tab bar for the tab as a whole: the focused pane's
    /// title.
    ///
    /// This one line *is* `Tabs` granularity. Warp's row in that mode names
    /// the tab's focused pane and reads every field off it — title, status,
    /// working directory, branch, PR link, diff stats. A tab-level aggregate
    /// would be a different Warp feature (`VerticalTabsTabItemMode::Summary`,
    /// which is flagged off) rather than this one.
    pub fn title(&self) -> &str {
        self.panes.focused().map_or("", Pane::title)
    }

    /// What the agent in the focused pane is doing, or `None` when the
    /// focused pane holds no agent.
    pub fn status(&self) -> Option<AgentStatus> {
        self.panes.focused().and_then(Pane::status)
    }
}

/// Everything that can happen to the strip, from a click, a key or a menu.
///
/// The whole vocabulary, addressed by identity. `MoveLeft` and `MoveRight`
/// need no argument because there is only one tab a person can mean by them:
/// the one they are looking at.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum TabAction {
    /// Open a tab after the active one and select it.
    New,
    /// Close a tab, whether or not it is the active one.
    Close(TabId),
    /// Make a tab the active one.
    Select(TabId),
    /// Move the active tab one slot towards the start.
    MoveLeft,
    /// Move the active tab one slot towards the end.
    MoveRight,
    /// Split the active tab's focused pane. Warp's
    /// `PaneGroupAction::Add(Direction)`.
    Split(Direction),
    /// Close one pane. When it is its tab's last pane the tab closes, and when
    /// that was the last tab the window does. Warp's `PaneGroupAction::Remove`
    /// → `Event::Exited` → `Workspace::close_tab`.
    ClosePane(PaneId),
    /// Focus a pane, activating its tab. Warp's
    /// `WorkspaceAction::FocusPane(PaneViewLocator)`.
    FocusPane(PaneId),
    /// Move the divider between two adjacent panes of the active tab.
    ///
    /// `leading` is the share of the pair that goes to the first of them, from
    /// zero to one. A ratio rather than pixels, because only the element that
    /// drew the two panes knows how wide they were — and only the group knows
    /// what a share means once other panes are beside them.
    ResizePanes {
        /// The pane on the left, or above.
        before: PaneId,
        /// The pane on the right, or below.
        after: PaneId,
        /// The share of the two of them that `before` takes.
        leading: f32,
    },
    /// Give every pane of the active tab an equal share again.
    EvenPanes,
    /// Show the settings page: focus the pane already holding it, or open a
    /// tab for it.
    ///
    /// One action for both halves, because "open settings" is one gesture and
    /// a caller that had to ask whether the page was already up would be a
    /// second copy of the rule. Warp's `Workspace::open_settings_pane` makes
    /// the same choice for the same reason, against a per-window manager that
    /// holds at most one settings pane.
    OpenSettings,
}

/// What the shell must do after an action was applied.
///
/// The distinction between the first two variants is what keeps a held-down
/// shortcut off the GPU: an action can be perfectly legal and still change
/// nothing — moving the first tab further left, selecting the tab that is
/// already selected, closing a tab that a click closed a moment ago — and the
/// strip is the only thing that knows which. A caller that repainted on every
/// action would rebuild, lay out and paint the whole window at key-repeat rate
/// for a bar that did not move.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TabEffect {
    /// The action was a no-op. Nothing moved, so nothing needs redrawing.
    Unchanged,
    /// The strip changed and has to be drawn again.
    Changed,
    /// The last tab was closed. The strip refuses to empty itself, so the
    /// window goes instead — which is also what keeps every accessor here
    /// honest without a second invariant kept in a second file.
    CloseWindow,
}

/// The tabs of one window, in order, with one of them active.
///
/// The vector is private and the only way to change it is [`Self::apply`].
#[derive(Debug)]
pub struct TabStrip {
    tabs: Vec<Tab>,
    /// The active tab's *identity*. Storing a position here is what forces
    /// every mutation to patch it; storing an identity means most mutations
    /// need no repair at all.
    active: TabId,
    /// Most-recently-used first, so closing the tab you are on returns you to
    /// the one you were on before rather than to whichever tab happens to sit
    /// to its right. Long-lived agent sessions make that the better answer;
    /// browsers pick the neighbour because their tabs are disposable.
    mru: Vec<TabId>,
    /// How many sessions this strip has ever opened, which is what the next
    /// one is named after — a tab's, or a pane's within a tab. Naming from
    /// `len()` instead repeats a name as soon as one in the middle is closed,
    /// and two agent sessions called "agent 3" are indistinguishable in the
    /// bar *and* in the body panel's heading. One counter for both, because in
    /// `Panes` view a pane and a tab are the same kind of row and a name that
    /// repeats across them is just as ambiguous.
    opened: u64,
}

impl Default for TabStrip {
    fn default() -> Self {
        Self::new()
    }
}

impl TabStrip {
    /// A strip with one tab in it, which is that tab's active.
    pub fn new() -> Self {
        let tab = Tab::new("agent 1");
        let id = tab.id();
        Self {
            tabs: vec![tab],
            active: id,
            mru: vec![id],
            opened: 1,
        }
    }

    /// How many tabs there are. Never zero.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Always false: the strip refuses to empty itself. Here because a `len`
    /// without an `is_empty` is a trap for the next reader.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Every tab, in bar order.
    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter()
    }

    /// The tab with this id, if it is still open.
    pub fn get(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id() == id)
    }

    /// The tab with this id, for reporting agent progress into it.
    ///
    /// Crate-private for the same reason as [`Tab::session_mut`].
    pub(crate) fn get_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id() == id)
    }

    /// The active tab.
    ///
    /// `Option` rather than an `expect`: the strip really is never empty and
    /// [`Self::repair`] really does keep the active id resident, but an
    /// accessor that panics when they are both wrong is how Warp ended up with
    /// two of them, guarded by a special case in a different file behind a
    /// feature flag.
    pub fn active(&self) -> Option<&Tab> {
        self.get(self.active)
    }

    /// The active tab's id.
    pub fn active_id(&self) -> TabId {
        self.active
    }

    /// Whether a tab is the active one.
    pub fn is_active(&self, id: TabId) -> bool {
        self.active == id
    }

    /// Where a tab sits in bar order.
    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id() == id)
    }

    /// Tabs in most-recently-used order, active first.
    pub fn mru(&self) -> &[TabId] {
        &self.mru
    }

    /// Every pane in the window, in bar order, with the tab holding it.
    pub fn panes(&self) -> impl Iterator<Item = (TabId, &Pane)> {
        self.tabs
            .iter()
            .flat_map(|tab| tab.panes().iter().map(|pane| (tab.id(), pane)))
    }

    /// Where the settings page is, if it is open at all.
    ///
    /// The window's one settings pane, named by both ids because everything
    /// that wants it wants both: the renderer needs the pane, and
    /// [`TabAction::OpenSettings`] needs the tab to bring forward. Searched
    /// rather than remembered — a cached id would be a second thing to keep
    /// true through every close, and the strip holds a handful of tabs.
    pub fn settings_pane(&self) -> Option<(TabId, PaneId)> {
        self.panes()
            .find(|(_, pane)| pane.is_settings())
            .map(|(tab, pane)| (tab, pane.id()))
    }

    /// The pane with this id, wherever it is.
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.tabs.iter().find_map(|tab| tab.panes().get(id))
    }

    /// The pane with this id, for reporting agent progress into it.
    ///
    /// Crate-private for the same reason as [`Pane::session_mut`]. This is the
    /// agent-progress path now that a session belongs to a pane rather than to
    /// a tab.
    pub(crate) fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.tabs
            .iter_mut()
            .find_map(|tab| tab.panes_mut().get_mut(id))
    }

    /// The tab holding this pane, if it is still open.
    pub fn tab_of(&self, pane: PaneId) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|tab| tab.panes().get(pane).is_some())
            .map(Tab::id)
    }

    /// The one pane a person is working in: the active tab's focused one.
    pub fn focused_pane_id(&self) -> Option<PaneId> {
        self.active().map(|tab| tab.panes().focused_id())
    }

    /// Every row the bar should draw at this granularity, as the tab it
    /// belongs to and the pane it stands for.
    ///
    /// The direct port of Warp's `pane_ids_for_display_granularity`
    /// (`app/src/workspace/view/vertical_tabs.rs:6460`), lifted from one tab
    /// to the whole strip so the bar has a single thing to iterate. A row is a
    /// pane in both modes; what changes is how many of a tab's panes get one.
    pub fn rows(&self, granularity: Granularity) -> Vec<(TabId, PaneId)> {
        let mut rows = Vec::with_capacity(self.tabs.len());

        for tab in &self.tabs {
            let panes = tab.panes();
            match granularity {
                Granularity::Panes => {
                    rows.extend(panes.iter().map(|pane| (tab.id(), pane.id())));
                }
                // Warp falls back to the first pane, and so does this. It is
                // unreachable — `repair` keeps the focused id resident — but a
                // tab with no row at all would vanish from the bar with no way
                // left to click it back.
                Granularity::Tabs => {
                    let pane = panes.focused().or_else(|| panes.iter().next());
                    rows.extend(pane.map(|pane| (tab.id(), pane.id())));
                }
            }
        }

        rows
    }

    /// Applies an action. The only way the strip changes.
    pub fn apply(&mut self, action: TabAction) -> TabEffect {
        match action {
            TabAction::New => {
                self.opened += 1;
                let tab = Tab::new(format!("agent {}", self.opened));
                let id = tab.id();
                // After the active tab, which is where a person who just
                // branched off what they were doing expects to find it.
                let at = self
                    .index_of(self.active)
                    .map_or(self.tabs.len(), |i| i + 1);
                self.tabs.insert(at, tab);
                self.repair(Some(id));
                TabEffect::Changed
            }

            TabAction::Close(id) => self.close(id),

            TabAction::Select(id) => self.select(id),

            TabAction::MoveLeft => self.hop(-1),
            TabAction::MoveRight => self.hop(1),

            TabAction::Split(direction) => {
                // A split opens an agent session, so it is named out of the
                // same counter a new tab is, and the counter only advances
                // once there is a pane wearing the name.
                let title = format!("agent {}", self.opened + 1);
                let active = self.active;
                let Some(tab) = self.get_mut(active) else {
                    // Unreachable: `repair` keeps the active id resident.
                    return TabEffect::Unchanged;
                };

                tab.panes_mut().split(direction, title);
                self.opened += 1;
                TabEffect::Changed
            }

            TabAction::ClosePane(id) => {
                let Some(tab) = self.tab_holding_mut(id) else {
                    return TabEffect::Unchanged;
                };
                let tab_id = tab.id();

                match tab.panes_mut().close(id) {
                    PaneEffect::Unchanged => TabEffect::Unchanged,
                    PaneEffect::Changed => TabEffect::Changed,
                    // The group refuses to empty itself exactly as the strip
                    // does, so closing a tab's last pane is closing the tab —
                    // and `close` is still the only place that knows the last
                    // tab takes the window with it.
                    PaneEffect::GroupEmptied => self.close(tab_id),
                }
            }

            TabAction::OpenSettings => {
                if let Some((_, pane)) = self.settings_pane() {
                    // Already open: this is a navigation, not a second page.
                    // Through the ordinary focus path, so activating its tab,
                    // the MRU and the "did anything move" answer are all the
                    // ones a click on its row would have produced.
                    return self.apply(TabAction::FocusPane(pane));
                }

                let tab = Tab::settings();
                let id = tab.id();
                let at = self
                    .index_of(self.active)
                    .map_or(self.tabs.len(), |i| i + 1);
                self.tabs.insert(at, tab);
                self.repair(Some(id));
                // Deliberately not touching `opened`: that counter names agent
                // sessions, and a settings tab is not one. Opening settings
                // between two new-tab chords must not skip a number.
                TabEffect::Changed
            }

            TabAction::FocusPane(id) => {
                let Some(tab) = self.tab_holding_mut(id) else {
                    return TabEffect::Unchanged;
                };
                let tab_id = tab.id();

                // The pane first, its tab second. Warp's ordering, and its
                // comment says why (`app/src/workspace/view.rs:5878`):
                // activating the tab first re-focuses whichever pane already
                // held input focus, and the pane that was asked for loses it
                // again. Crook's panes hold no input focus yet, so the wrong
                // order would look right today and break the day they do.
                let focused = tab.panes_mut().focus(id);
                let selected = self.select(tab_id);

                if focused == PaneEffect::Changed || selected == TabEffect::Changed {
                    TabEffect::Changed
                } else {
                    TabEffect::Unchanged
                }
            }

            // Both of these are about the *active* tab's split, because a
            // divider is a thing on screen and only the active tab has any.
            TabAction::ResizePanes {
                before,
                after,
                leading,
            } => match self.get_mut(self.active) {
                Some(tab) => pane_effect(tab.panes_mut().resize(before, after, leading)),
                None => TabEffect::Unchanged,
            },

            TabAction::EvenPanes => match self.get_mut(self.active) {
                Some(tab) => pane_effect(tab.panes_mut().even_out()),
                None => TabEffect::Unchanged,
            },
        }
    }

    fn select(&mut self, id: TabId) -> TabEffect {
        if self.active == id || self.get(id).is_none() {
            return TabEffect::Unchanged;
        }
        self.repair(Some(id));
        TabEffect::Changed
    }

    fn tab_holding_mut(&mut self, pane: PaneId) -> Option<&mut Tab> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.panes().get(pane).is_some())
    }

    fn close(&mut self, id: TabId) -> TabEffect {
        let Some(index) = self.index_of(id) else {
            return TabEffect::Unchanged;
        };

        // The strip is non-empty by construction, and this is the whole of it:
        // the last tab is never removed, the window is closed instead.
        if self.tabs.len() == 1 {
            return TabEffect::CloseWindow;
        }

        self.tabs.remove(index);
        // `repair` prefers whoever is at the front of the MRU list once the
        // closed tab is gone, so closing the active tab needs no successor
        // computed here and closing any other tab needs nothing at all.
        self.repair(None);
        TabEffect::Changed
    }

    fn hop(&mut self, offset: isize) -> TabEffect {
        let Some(from) = self.index_of(self.active) else {
            return TabEffect::Unchanged;
        };
        let Some(to) = from
            .checked_add_signed(offset)
            .filter(|to| *to < self.len())
        else {
            // Already at the end it was asked to move towards.
            return TabEffect::Unchanged;
        };

        // Remove-and-insert rather than swap, so the same code moves a tab one
        // slot or ten. Nothing needs repairing afterwards, because `active`
        // names the tab that just moved rather than the slot it was in.
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.repair(None);
        TabEffect::Changed
    }

    /// The single place `active` is ever assigned, and the single place the
    /// MRU list is pruned.
    ///
    /// It picks by identity, in order of preference: the tab the caller asked
    /// for, the tab that was already active, the most recently used survivor,
    /// and finally the first tab. No branch of it knows what a removal or a
    /// reorder did to anyone's position, which is exactly why none of them can
    /// get it wrong.
    fn repair(&mut self, preferred: Option<TabId>) {
        let open: Vec<TabId> = self.tabs.iter().map(Tab::id).collect();
        self.mru.retain(|id| open.contains(id));

        let Some(chosen) = preferred
            .filter(|id| self.get(*id).is_some())
            .or_else(|| self.get(self.active).map(Tab::id))
            .or_else(|| self.mru.first().copied())
            .or_else(|| self.tabs.first().map(Tab::id))
        else {
            debug_assert!(false, "the strip emptied itself, which `close` forbids");
            return;
        };

        self.active = chosen;
        self.mru.retain(|id| *id != chosen);
        self.mru.insert(0, chosen);
    }
}

/// What a change inside one tab's panes means to the strip.
///
/// Only for the changes that cannot empty a group. `GroupEmptied` is the
/// closing path's, where it means "and then the tab goes", and it is handled
/// there rather than mapped here — a resize that reported it would be a bug,
/// and reading it as `Unchanged` is the answer that changes nothing.
fn pane_effect(effect: PaneEffect) -> TabEffect {
    match effect {
        PaneEffect::Changed => TabEffect::Changed,
        PaneEffect::Unchanged | PaneEffect::GroupEmptied => TabEffect::Unchanged,
    }
}
