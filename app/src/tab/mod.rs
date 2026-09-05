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

mod group;
mod pane;

#[cfg(test)]
mod tests;

pub use group::{GroupId, TabGroup};
pub use pane::{Direction, Pane, PaneEffect, PaneGroup, PaneId, SplitAxis};

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
    /// What a person called it, if they have said.
    ///
    /// Beats the agent's own name and never the other way round, which is the
    /// whole of what renaming means: an agent that renames its work every few
    /// turns would otherwise take the name back within the minute, and a
    /// person who typed one would have no way to make it stick. Warp draws the
    /// same line with `custom_title` over its own derived one.
    ///
    /// `None` is "nobody has said", and it is what a rename to nothing goes
    /// back to — a person who empties the field is asking for the name they
    /// had before they touched it, not for a row with no name.
    pub custom_title: Option<String>,
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
            custom_title: None,
            status: AgentStatus::default(),
            working_directory: std::env::current_dir().ok(),
            pull_request: None,
        }
    }

    /// What the tab bar should print: what a person called it, else the
    /// agent's own name for its work, else the name it was created with.
    pub fn display_title(&self) -> &str {
        self.custom_title
            .as_deref()
            .or(self.derived_title.as_deref())
            .unwrap_or(&self.title)
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
/// The name is the tab's own and does not move as focus moves inside it,
/// because the panel's group header is what names a tab whose rows name its
/// panes. Deriving that header from the focused pane would rewrite the heading
/// every time someone clicked a row underneath it. It is Warp's `custom_title`
/// now that there is a rename flow — see [`Tab::set_name`] — and the name it
/// was born with is kept beside it so that taking a rename back has something
/// to go back to.
#[derive(Debug)]
pub struct Tab {
    id: TabId,
    name: String,
    /// The name it was opened with, which is what a rename undone returns to.
    ///
    /// Kept rather than derived, because there is nothing to derive it from: a
    /// tab's panes are renamed and split and closed independently of it, and
    /// by the time somebody empties the rename field the session this was
    /// taken from may not be in the tab any more.
    born_as: String,
    panes: PaneGroup,
    /// The group this tab belongs to, if it is in one.
    ///
    /// Warp's `TabData::group_id`, and it is one `Option` rather than a list
    /// of members held by the group for the reason [`group`] gives: the tabs
    /// are already an ordered vector, and a second list of the same tabs is a
    /// second answer to what order they are in.
    ///
    /// Private and written only by [`TabStrip`], which is the one thing that
    /// can keep a group's members contiguous while writing it.
    group: Option<GroupId>,
}

impl Tab {
    /// A tab over one new session named `title`.
    pub fn new(title: impl Into<String>) -> Self {
        let title = title.into();
        Self {
            id: TabId::next(),
            name: title.clone(),
            born_as: title.clone(),
            panes: PaneGroup::new(title),
            group: None,
        }
    }

    /// This tab's identity, for as long as it is open.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The group this tab is in, if it is in one.
    pub fn group(&self) -> Option<GroupId> {
        self.group
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

    /// Renames it, or takes a name back.
    ///
    /// `None` restores the name it was born with, which is where a rename to
    /// an empty field lands. Crate-private like everything else that changes
    /// what the strip draws: the way in from outside is `TabStrip::apply`.
    pub(crate) fn set_name(&mut self, name: Option<String>) {
        self.name = name.unwrap_or_else(|| self.born_as.clone());
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

    /// What the agent in the focused pane is doing.
    pub fn status(&self) -> Option<AgentStatus> {
        self.panes.focused().map(Pane::status)
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
    /// Open a tab in the same group as `tab`, making a group of the two when
    /// it has none, and select it.
    ///
    /// What a worktree opens into, and the whole reason groups exist. The
    /// checkout beside the one a person asked from belongs *with* it: same
    /// repository, one branch over. Splitting the tab said that by putting two
    /// agents in one rectangle, which is a different claim — that they are two
    /// halves of one screen — and it is the wrong one.
    NewInGroupOf(TabId),
    /// Put a tab somewhere else in the list: into a group, out of one, or at
    /// another place among its neighbours.
    ///
    /// The one action a drop dispatches. `before` names the tab it lands in
    /// front of and `None` means last; `group` is the group it joins and
    /// `None` means none. The two are clamped against each other rather than
    /// trusted — see [`TabStrip::slot_for`] — so a target computed from a
    /// pointer position cannot break a group's contiguity however coarse the
    /// geometry that produced it.
    MoveTab {
        /// The tab being moved.
        tab: TabId,
        /// The group it lands in, or `None` to leave whatever group it is in.
        group: Option<GroupId>,
        /// The tab it lands in front of, or `None` for the end.
        before: Option<TabId>,
    },
    /// Move a whole group's block of tabs in front of `before`, or to the end.
    ///
    /// A block only ever lands between blocks: `before` is snapped to the
    /// start of whatever block holds it, because a group dropped into the
    /// middle of another group is the one arrangement the panel cannot draw.
    MoveGroup {
        /// The group being moved.
        group: GroupId,
        /// The tab whose block it lands in front of, or `None` for the end.
        before: Option<TabId>,
    },
    /// Fold a group's members away behind its heading, or show them again.
    ToggleGroup(GroupId),
    /// Close every tab of a group. When they are all of them, the window goes.
    CloseGroup(GroupId),
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
    /// The groups any of those tabs belong to.
    ///
    /// Holds no membership and no order of its own — both are the tabs' — so
    /// there is nothing here to keep in step with the vector above. A group
    /// nobody is in is not a group: [`Self::prune`] drops it the moment its
    /// last member leaves, which is what stops an empty heading from
    /// outliving the work it named.
    groups: Vec<TabGroup>,
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
            groups: Vec::new(),
            active: id,
            mru: vec![id],
            opened: 1,
        }
    }

    /// An empty strip, for a caller that is about to fill it.
    ///
    /// **The one shape this type otherwise refuses to be in**, and it exists
    /// for exactly one caller: [`crate::session`], which builds a strip out of
    /// a file and cannot start from a strip that already has a tab nobody
    /// asked for. It is `pub(crate)` and paired with [`Self::adopt`]; a strip
    /// left empty draws no rows and answers `None` to everything, which is why
    /// the restoring path throws one away rather than opening a window with
    /// it.
    pub(crate) fn empty() -> Self {
        Self {
            tabs: Vec::new(),
            groups: Vec::new(),
            active: TabId::next(),
            mru: Vec::new(),
            opened: 0,
        }
    }

    /// Appends a tab built elsewhere, making it the active one.
    ///
    /// The counter moves with it, so the first tab a person opens after a
    /// restore is named after the ones that came back rather than repeating
    /// one of their names.
    pub(crate) fn adopt(&mut self, tab: Tab) {
        let id = tab.id();
        self.tabs.push(tab);
        self.opened += 1;
        self.repair(Some(id));
    }

    /// Selects a tab by its position in the bar.
    ///
    /// Out of range selects nothing, which leaves whatever `adopt` last made
    /// active. Positions are the session file's vocabulary and nothing else's:
    /// every other caller names a tab by identity, for the reason the module
    /// docs give at length.
    pub(crate) fn select_index(&mut self, index: usize) {
        if let Some(id) = self.tabs.get(index).map(Tab::id) {
            self.repair(Some(id));
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

    /// Every group with a tab in it.
    pub fn groups(&self) -> impl Iterator<Item = &TabGroup> {
        self.groups.iter()
    }

    /// The group with this id, if it still holds a tab.
    pub fn group(&self, id: GroupId) -> Option<&TabGroup> {
        self.groups.iter().find(|group| group.id() == id)
    }

    /// The tabs of a group, in strip order. Contiguous, always.
    pub fn members(&self, group: GroupId) -> impl Iterator<Item = &Tab> {
        self.tabs
            .iter()
            .filter(move |tab| tab.group() == Some(group))
    }

    /// The strip as the panel draws it: each group's members under the group,
    /// and every ungrouped tab on its own.
    ///
    /// This is the one place the contiguity invariant is *spent* rather than
    /// kept. Because members are contiguous, one walk in strip order produces
    /// the blocks in strip order with no sorting and no second pass — and a
    /// panel built from it cannot draw a tab twice or leave one out, whatever
    /// a drag did a frame ago.
    pub fn blocks(&self) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::with_capacity(self.tabs.len());

        for tab in &self.tabs {
            match blocks.last_mut() {
                Some(block) if block.group.is_some() && block.group == tab.group() => {
                    block.tabs.push(tab.id());
                }
                _ => blocks.push(Block {
                    group: tab.group(),
                    tabs: vec![tab.id()],
                }),
            }
        }

        blocks
    }

    /// Renames a group.
    ///
    /// Crate-private and outside [`Self::apply`], for the reason
    /// [`Self::adopt`] is: an action is `Copy` and a name is a `String`. The
    /// vector is not touched, so nothing here can break the invariant `apply`
    /// exists to keep.
    pub(crate) fn rename_group(&mut self, id: GroupId, name: impl Into<String>) {
        if let Some(group) = self.groups.iter_mut().find(|group| group.id() == id) {
            group.set_name(name);
        }
    }

    /// Rebuilds a group a session file described, out of tabs already adopted.
    ///
    /// The restoring path's counterpart to [`Self::adopt`], and it has the
    /// same shape for the same reason: a file cannot be trusted to have
    /// written a group whose members were contiguous, so this gathers them
    /// rather than assuming them. A group naming no tab that came back is not
    /// created at all.
    pub(crate) fn adopt_group(&mut self, name: String, collapsed: bool, members: &[TabId]) {
        let members: Vec<TabId> = members
            .iter()
            .copied()
            .filter(|id| self.get(*id).is_some())
            .collect();
        let Some(&first) = members.first() else {
            return;
        };

        let mut group = TabGroup::new(name);
        group.set_collapsed(collapsed);
        let id = group.id();
        self.groups.push(group);

        for member in &members {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id() == *member) {
                tab.group = Some(id);
            }
        }

        // Gathered at the first member's place, so a hand-edited file that
        // scattered them cannot produce a strip the panel is unable to draw.
        let at = self.index_of(first).unwrap_or(self.tabs.len());
        let moving: Vec<Tab> = {
            let mut moving = Vec::with_capacity(members.len());
            let mut index = 0;
            while index < self.tabs.len() {
                if self.tabs[index].group() == Some(id) {
                    moving.push(self.tabs.remove(index));
                } else {
                    index += 1;
                }
            }
            moving
        };
        let at = at.min(self.tabs.len());
        for (offset, tab) in moving.into_iter().enumerate() {
            self.tabs.insert(at + offset, tab);
        }
        self.repair(None);
    }

    /// The half-open range of the vector a group's members occupy.
    ///
    /// `None` for a group with nothing in it, which is a group about to be
    /// pruned. Correct *because* members are contiguous: the first and last
    /// positions bound every member between them.
    fn run_of(&self, group: GroupId) -> Option<std::ops::Range<usize>> {
        let first = self
            .tabs
            .iter()
            .position(|tab| tab.group() == Some(group))?;
        let last = self
            .tabs
            .iter()
            .rposition(|tab| tab.group() == Some(group))?;
        Some(first..last + 1)
    }

    /// Where a tab joining `group` in front of `before` actually goes.
    ///
    /// The clamp the drop targets are trusted through. A pointer between two
    /// rows says two things that can disagree — which gap it is in, and which
    /// group's band it is over — and the honest resolution is to believe the
    /// group and move the gap, because the group is what the person is aiming
    /// at and the gap is only how far their hand got. So:
    ///
    /// * joining a group, the slot is pulled into that group's run;
    /// * joining none, a slot *inside* somebody's run is pushed out to its
    ///   nearer end, rather than splitting them.
    ///
    /// Either way the result is a position no group's contiguity survives by
    /// luck.
    fn slot_for(&self, group: Option<GroupId>, before: Option<TabId>) -> usize {
        let raw = before
            .and_then(|id| self.index_of(id))
            .unwrap_or(self.tabs.len());

        match group {
            // Into a group that still has members: inside its run, wherever
            // in it the pointer got to. A group whose only member is the tab
            // being moved has no run left, and the raw slot is as good an
            // answer as there is.
            Some(group) => match self.run_of(group) {
                Some(run) => raw.clamp(run.start, run.end),
                None => raw,
            },
            // Into no group: never between two members of one. The nearer end
            // wins, so a drop just past a group's first member lands above the
            // group rather than teleporting to the bottom of it.
            None => match self.tabs.get(raw).and_then(Tab::group) {
                Some(landed_in) => match self.run_of(landed_in) {
                    Some(run) if raw > run.start => {
                        if raw - run.start <= run.end - raw {
                            run.start
                        } else {
                            run.end
                        }
                    }
                    _ => raw,
                },
                None => raw,
            },
        }
    }

    /// Moves one tab, joining `group` and landing in front of `before`.
    fn move_tab(&mut self, tab: TabId, group: Option<GroupId>, before: Option<TabId>) -> TabEffect {
        let Some(from) = self.index_of(tab) else {
            return TabEffect::Unchanged;
        };
        // A group that has been pruned since the frame the drag started on is
        // not a group to join. Naming no group is always legal.
        if group.is_some_and(|id| self.group(id).is_none()) {
            return TabEffect::Unchanged;
        }
        let left = self.tabs[from].group();
        if before == Some(tab) {
            // "In front of itself" is where it already is.
            return TabEffect::Unchanged;
        }

        let mut moving = self.tabs.remove(from);
        moving.group = group;
        let at = self.slot_for(group, before);
        let unchanged = at == from && left == group;
        self.tabs.insert(at, moving);

        if unchanged {
            return TabEffect::Unchanged;
        }
        if let Some(left) = left.filter(|left| Some(*left) != group) {
            self.prune(left);
        }
        self.repair(None);
        TabEffect::Changed
    }

    /// Moves a whole group's block in front of `before`'s block.
    fn move_group(&mut self, group: GroupId, before: Option<TabId>) -> TabEffect {
        let Some(run) = self.run_of(group) else {
            return TabEffect::Unchanged;
        };
        // A block dropped on itself has not moved, and a block dropped on one
        // of its own members would be asked to land inside itself.
        if before.is_some_and(|id| self.tabs[run.clone()].iter().any(|tab| tab.id() == id)) {
            return TabEffect::Unchanged;
        }

        // Snapped to the start of whatever block holds `before`: a group only
        // ever lands between blocks.
        let at = match before.and_then(|id| self.index_of(id)) {
            Some(index) => match self.tabs[index].group().and_then(|id| self.run_of(id)) {
                Some(landed_in) => landed_in.start,
                None => index,
            },
            None => self.tabs.len(),
        };
        // Both ends of its own run are where it already is: dropping a block
        // just above itself and dropping it just below itself are the same
        // arrangement, and reporting either as a change repaints the window
        // for nothing.
        if at == run.start || at == run.end {
            return TabEffect::Unchanged;
        }

        let moving: Vec<Tab> = self.tabs.drain(run.clone()).collect();
        // Everything after the run has shifted left by its length.
        let at = if at > run.start {
            at - moving.len()
        } else {
            at
        };
        for (offset, tab) in moving.into_iter().enumerate() {
            self.tabs.insert(at + offset, tab);
        }
        self.repair(None);
        TabEffect::Changed
    }

    /// Opens a tab in `anchor`'s group, making one of the two when it has no
    /// group yet.
    fn new_in_group_of(&mut self, anchor: TabId) -> TabEffect {
        let Some(index) = self.index_of(anchor) else {
            return TabEffect::Unchanged;
        };

        let group = match self.tabs[index].group() {
            Some(group) => group,
            None => {
                // Named after the tab it was made around, which is the only
                // name there is at this point and a better one than "New
                // Group": the heading says which piece of work the checkouts
                // under it belong to. Its *displayed* title, so a tab whose
                // agent has named its own work lends the group that name
                // rather than the "agent 3" nobody chose. A caller that knows
                // the repository renames it — see [`Self::rename_group`].
                let anchor = &self.tabs[index];
                let name = match anchor.title() {
                    "" => anchor.name().to_owned(),
                    title => title.to_owned(),
                };
                let group = TabGroup::new(name);
                let id = group.id();
                self.groups.push(group);
                self.tabs[index].group = Some(id);
                id
            }
        };

        self.opened += 1;
        let mut tab = Tab::new(format!("agent {}", self.opened));
        tab.group = Some(group);
        let id = tab.id();
        // After the group's last member. A worktree is the newest thing in the
        // group, and the alternative — beside the tab it was asked from — puts
        // it in the middle of checkouts made before it.
        let at = self.run_of(group).map_or(index + 1, |run| run.end);
        self.tabs.insert(at, tab);
        self.repair(Some(id));
        TabEffect::Changed
    }

    /// Closes every tab of a group.
    fn close_group(&mut self, group: GroupId) -> TabEffect {
        let members: Vec<TabId> = self.members(group).map(Tab::id).collect();
        if members.is_empty() {
            return TabEffect::Unchanged;
        }
        // Closing every tab there is closes the window, and it says so once
        // rather than closing tabs until `close` refuses and leaves a group
        // half gone.
        if members.len() >= self.tabs.len() {
            return TabEffect::CloseWindow;
        }

        for member in members {
            self.close(member);
        }
        TabEffect::Changed
    }

    /// The group a tab hopped into `to` lands in: the one *both* of its new
    /// neighbours are in, and otherwise none.
    ///
    /// Called with the tab already lifted out of the vector, so `to` is a gap
    /// between two tabs that are staying put. Landing between two members of
    /// one group is the only way into a group and the only way to stay in one;
    /// every other gap is outside every group, which is exactly the reading
    /// that keeps a run unbroken.
    fn neighbouring_group(&self, to: usize) -> Option<GroupId> {
        let before = to
            .checked_sub(1)
            .and_then(|index| self.tabs.get(index))
            .and_then(Tab::group);
        let after = self.tabs.get(to).and_then(Tab::group);
        (before == after).then_some(before).flatten()
    }

    /// Drops a group nobody is in any more.
    fn prune(&mut self, group: GroupId) {
        if self.members(group).next().is_none() {
            self.groups.retain(|held| held.id() != group);
        }
    }

    /// Applies an action. The only way the strip changes.
    pub fn apply(&mut self, action: TabAction) -> TabEffect {
        match action {
            TabAction::New => {
                self.opened += 1;
                let tab = Tab::new(format!("agent {}", self.opened));
                let id = tab.id();
                // After the active tab, which is where a person who just
                // branched off what they were doing expects to find it — and
                // past the rest of its group when it is in one, because a tab
                // that belongs to nothing cannot be dropped into the middle of
                // tabs that belong together. Warp's `clamp_to_unpinned_region`
                // is the same move for its own reason.
                let at = match self.index_of(self.active) {
                    Some(index) => match self.tabs[index].group().and_then(|id| self.run_of(id)) {
                        Some(run) => run.end,
                        None => index + 1,
                    },
                    None => self.tabs.len(),
                };
                self.tabs.insert(at, tab);
                self.repair(Some(id));
                TabEffect::Changed
            }

            TabAction::NewInGroupOf(anchor) => self.new_in_group_of(anchor),

            TabAction::MoveTab { tab, group, before } => self.move_tab(tab, group, before),

            TabAction::MoveGroup { group, before } => self.move_group(group, before),

            TabAction::ToggleGroup(id) => {
                let Some(group) = self.groups.iter_mut().find(|group| group.id() == id) else {
                    return TabEffect::Unchanged;
                };
                let collapsed = group.is_collapsed();
                group.set_collapsed(!collapsed);
                TabEffect::Changed
            }

            TabAction::CloseGroup(group) => self.close_group(group),

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

        let closed = self.tabs.remove(index);
        // A heading with nothing under it is not a group. Pruned here rather
        // than by whoever called `close`, because this is the one place a tab
        // ever leaves the vector.
        if let Some(group) = closed.group() {
            self.prune(group);
        }
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
        let mut tab = self.tabs.remove(from);
        // A tab hopped past the end of its group leaves it, and one hopped
        // into somebody else's joins theirs. That is what the keyboard has to
        // mean once there are groups: the alternative is a shortcut that can
        // put a tab between two members of a group it is not in, which is the
        // one arrangement the panel cannot draw.
        let left = tab.group();
        tab.group = self.neighbouring_group(to);
        let joined = tab.group();
        self.tabs.insert(to, tab);
        if let Some(left) = left.filter(|left| Some(*left) != joined) {
            self.prune(left);
        }
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

/// One run of the strip the panel draws as a unit: a group with its members,
/// or a single tab that is in no group.
///
/// Warp's `render_groups` builds the same thing inline and calls the halves
/// `grouped` and `ungrouped`; naming it makes the panel's loop one `match`
/// instead of a state machine, and makes "the blocks are in strip order" a
/// thing [`TabStrip::blocks`] can be tested for without a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The group these tabs belong to, or `None` for a lone ungrouped tab.
    pub group: Option<GroupId>,
    /// Its tabs, in strip order. Never empty; exactly one when `group` is
    /// `None`.
    pub tabs: Vec<TabId>,
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
