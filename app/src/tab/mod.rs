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

use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
mod tests;

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

/// The agent session a tab is a window onto.
///
/// Warp's tab points at a `ViewHandle<PaneGroup>` — a split tree of terminal
/// panes — which is why its tab struct has no title and every accessor threads
/// a context. A Crook tab is exactly one agent, so the session is owned inline
/// and read directly.
///
/// It deliberately has no id of its own. Carrying one would mean carrying the
/// invariant that it equals its tab's, and a session is reachable as `&mut`
/// from the agent-progress path — so the invariant would be one field
/// assignment away from being false, and `index_of` would then resolve to the
/// wrong tab. One identity, owned by [`Tab`], cannot disagree with itself.
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
}

impl AgentSession {
    /// A fresh idle session named `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            derived_title: None,
            status: AgentStatus::default(),
        }
    }

    /// What the tab bar should print: the agent's own name for its work, or
    /// the name the session was created with until it has one.
    pub fn display_title(&self) -> &str {
        self.derived_title.as_deref().unwrap_or(&self.title)
    }
}

/// One tab: an identity and the session it shows.
///
/// Deliberately tiny. It owns no hover state, no drag state and no active
/// flag — hover belongs to the view that renders it and is handed back each
/// frame, and activeness belongs to the strip, so "two tabs are active" is not
/// a representable state.
///
/// Both fields are private, and that is load-bearing rather than tidy: a
/// public `id` is settable through the `&mut Tab` the agent-progress path
/// hands out, and two tabs sharing an id makes `index_of` resolve a close to
/// somebody else's tab.
#[derive(Debug)]
pub struct Tab {
    id: TabId,
    session: AgentSession,
}

impl Tab {
    /// A tab over a new session named `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: TabId::next(),
            session: AgentSession::new(title),
        }
    }

    /// This tab's identity, for as long as it is open.
    pub fn id(&self) -> TabId {
        self.id
    }

    /// The agent session behind it.
    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    /// The session, for reporting the agent's progress into it.
    pub fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    /// What to print in the tab bar.
    pub fn title(&self) -> &str {
        self.session.display_title()
    }

    /// What the agent is doing.
    pub fn status(&self) -> AgentStatus {
        self.session.status
    }
}

/// Everything that can happen to the strip, from a click, a key or a menu.
///
/// The whole vocabulary, addressed by identity. `MoveLeft` and `MoveRight`
/// need no argument because there is only one tab a person can mean by them:
/// the one they are looking at.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
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
    /// How many tabs this strip has ever opened, which is what the next one is
    /// named after. Naming from `len()` instead repeats a name as soon as a
    /// tab in the middle is closed, and two agent sessions called "agent 3"
    /// are indistinguishable in the bar *and* in the body panel's heading.
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
    pub fn get_mut(&mut self, id: TabId) -> Option<&mut Tab> {
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

            TabAction::Select(id) => {
                if self.active == id || self.get(id).is_none() {
                    return TabEffect::Unchanged;
                }
                self.repair(Some(id));
                TabEffect::Changed
            }

            TabAction::MoveLeft => self.hop(-1),
            TabAction::MoveRight => self.hop(1),
        }
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
