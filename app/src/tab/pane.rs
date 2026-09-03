//! The panes inside one tab: what a pane is, and the rules for splitting them.
//!
//! A Crook tab used to be exactly one agent session, and that is why this
//! module exists. "View as: Panes | Tabs" is not a setting anybody can watch
//! move until a tab can hold more than one thing to show — both modes would
//! draw the same bar, and the control would be a lie.
//!
//! What is here is Warp's `PaneBranch` (`app/src/pane_group/tree.rs:132`) with
//! the nesting taken out: a flat ordered vector along one axis, plus Warp's
//! `focus_state.focused_pane_id` and its `pane_history` MRU. That is faithful
//! rather than a simplification, because Warp's own same-axis fast path
//! (`PaneBranch::split`, tree.rs:927) already collapses the common case to
//! exactly this — three "split right"s in Warp produce one horizontal branch
//! with three children, not a right-leaning tree — and every list-shaped
//! reader of the tree (`pane_ids()`, the vertical tab rows, prev/next
//! navigation) only ever sees the flattened order anyway.
//!
//! What a flat vector cannot express is the nested case, and
//! [`PaneGroup::split`] says exactly what it does instead of pretending.

use std::sync::atomic::{AtomicU64, Ordering};

use super::{AgentSession, AgentStatus};

/// A pane's identity, stable for as long as the pane exists.
///
/// Minted from a process-wide counter and never reused, exactly like
/// [`TabId`](super::TabId), so an action naming a pane that has since been
/// closed resolves to nothing rather than to whichever pane took its slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(u64);

impl PaneId {
    /// Mints an id no other pane in this process will ever have.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Which way a group's panes are laid out. Warp's `SplitDirection`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SplitAxis {
    /// Side by side, in a row. What a group that has never been split reports,
    /// and what "split right" makes of it.
    #[default]
    Horizontal,
    /// Stacked, in a column.
    Vertical,
}

/// Where a new pane goes, relative to the focused one. Warp's `Direction`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Before the focused pane, in a row.
    Left,
    /// After the focused pane, in a row.
    Right,
    /// Before the focused pane, in a column.
    Up,
    /// After the focused pane, in a column.
    Down,
}

impl Direction {
    /// The axis a split in this direction divides along.
    pub fn axis(self) -> SplitAxis {
        match self {
            Self::Left | Self::Right => SplitAxis::Horizontal,
            Self::Up | Self::Down => SplitAxis::Vertical,
        }
    }

    /// Whether the new pane goes before the one it was split off, rather than
    /// after it.
    fn is_leading(self) -> bool {
        matches!(self, Self::Left | Self::Up)
    }
}

/// What a pane is showing.
///
/// A pane used to be a session and nothing else. The settings page is the
/// second thing a pane can be, and it is a *pane* rather than a modal because
/// that is Warp's design and it is the better one: settings open the way a
/// shell opens, in a tab of their own, listed in the strip beside the work
/// they configure, splittable next to it, closed by the same close button and
/// the same `cmd/ctrl-w`.
///
/// The cost is this enum, and it is paid honestly rather than hidden behind a
/// session with empty fields: everything that reads a pane now says what it
/// does when there is no session to read — [`Pane::session`] returns an
/// `Option`, [`Pane::status`] returns an `Option`, and the row renderers draw
/// a gear and one line instead of resolving a fact table that has nothing in
/// it.
///
/// The other arm carries the session a shell runs under — the terminal itself
/// lives in a model keyed by [`PaneId`], not here, so this stays a thing that
/// can be tested with no window and no process.
#[derive(Debug)]
pub enum PaneContent {
    /// One session: what a tab opened with `cmd/ctrl-t` holds, and what the
    /// shell in that tab reports its title and working directory into.
    Agent(AgentSession),
    /// The settings page. There is at most one in a window — `TabStrip::apply`
    /// navigates to the existing one rather than opening a second — and it
    /// holds no state of its own: which page the rail has selected and how far
    /// it is scrolled are the view's, keyed by nothing, which is what makes
    /// them survive closing the tab and opening it again.
    Settings,
}

/// What the strip and the panel call the settings pane.
pub const SETTINGS_TITLE: &str = "Settings";

/// One pane: an identity, and what it shows.
///
/// Both fields are private for the reason [`Tab`](super::Tab)'s are: a public
/// `id` is settable through the `&mut Pane` the agent-progress path hands out,
/// and two panes sharing an id makes every lookup here resolve to the wrong
/// session.
#[derive(Debug)]
pub struct Pane {
    id: PaneId,
    content: PaneContent,
}

impl Pane {
    /// A pane over a new session named `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: PaneId::next(),
            content: PaneContent::Agent(AgentSession::new(title)),
        }
    }

    /// A pane showing the settings page.
    pub fn settings() -> Self {
        Self {
            id: PaneId::next(),
            content: PaneContent::Settings,
        }
    }

    /// This pane's identity, for as long as it is open.
    pub fn id(&self) -> PaneId {
        self.id
    }

    /// What it is showing.
    pub fn content(&self) -> &PaneContent {
        &self.content
    }

    /// The agent session behind it, or `None` for the settings pane.
    pub fn session(&self) -> Option<&AgentSession> {
        match &self.content {
            PaneContent::Agent(session) => Some(session),
            PaneContent::Settings => None,
        }
    }

    /// Whether this is the settings pane.
    pub fn is_settings(&self) -> bool {
        matches!(self.content, PaneContent::Settings)
    }

    /// The session, for reporting the agent's progress into it.
    ///
    /// Crate-private on purpose: mutating a session changes what the tab bar
    /// draws, so the only way in from outside is `Workspace::update_session`,
    /// which notifies in the same call. `None` for the settings pane, which is
    /// what makes a report addressed to a pane that has been replaced by one
    /// fail rather than land somewhere.
    pub(crate) fn session_mut(&mut self) -> Option<&mut AgentSession> {
        match &mut self.content {
            PaneContent::Agent(session) => Some(session),
            PaneContent::Settings => None,
        }
    }

    /// What to print for this pane, in the bar and in its panel.
    pub fn title(&self) -> &str {
        match &self.content {
            PaneContent::Agent(session) => session.display_title(),
            PaneContent::Settings => SETTINGS_TITLE,
        }
    }

    /// What the agent in it is doing, or `None` when there is no agent.
    ///
    /// The settings pane is not idle, not running and not failed; it is not an
    /// agent. An `Option` rather than an `Idle` default because the two look
    /// different on screen — a status dot against a gear — and a default here
    /// would put a grey dot beside the settings row and call it accurate.
    pub fn status(&self) -> Option<AgentStatus> {
        match &self.content {
            PaneContent::Agent(session) => Some(session.status),
            PaneContent::Settings => None,
        }
    }
}

/// What a change did to a group, so the tab layer knows whether it survived.
///
/// The first two variants carry the same "did anything move" question
/// [`TabEffect`](super::TabEffect) answers for the strip, and for the same
/// reason: a legal action that changed nothing must not cost a frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PaneEffect {
    /// Nothing moved.
    Unchanged,
    /// The group changed and has to be drawn again.
    Changed,
    /// The last pane was closed. The group refuses to empty itself, so the tab
    /// goes instead — Warp's `Event::Exited`, which its workspace turns into
    /// `close_tab` (`app/src/workspace/view.rs:16046`).
    GroupEmptied,
}

/// The panes of one tab, in order, with one of them focused.
///
/// The vector is private and only [`Self::split`] and [`Self::close`] change
/// it, which is what keeps `focused` resident without a second invariant kept
/// somewhere else.
#[derive(Debug)]
pub struct PaneGroup {
    panes: Vec<Pane>,
    /// Which way the panes are divided. Set by the first split and fixed after
    /// it; see [`Self::split`].
    axis: SplitAxis,
    /// The focused pane's *identity*, never its position — the same reason
    /// [`TabStrip`](super::TabStrip) stores an identity for the active tab.
    focused: PaneId,
    /// Most-recently-focused first. Warp's `pane_history`, and it is what
    /// picks the successor when the focused pane is closed.
    mru: Vec<PaneId>,
}

impl PaneGroup {
    /// A group of one pane, over a new session named `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self::of(Pane::new(title))
    }

    /// A group of one pane, showing the settings page.
    pub fn settings() -> Self {
        Self::of(Pane::settings())
    }

    /// A group of exactly one pane, focused.
    fn of(pane: Pane) -> Self {
        let id = pane.id();
        Self {
            panes: vec![pane],
            axis: SplitAxis::default(),
            focused: id,
            mru: vec![id],
        }
    }

    /// How many panes there are. Never zero.
    pub fn len(&self) -> usize {
        self.panes.len()
    }

    /// Always false: the group refuses to empty itself. Here because a `len`
    /// without an `is_empty` is a trap for the next reader.
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    /// Every pane, in render order — left to right, or top to bottom.
    ///
    /// Warp needs both `pane_ids()` and `visible_pane_ids()` because a pane
    /// can be hidden for five different reasons (a move, a job, a temporary
    /// replacement, an undoable close, a child agent). With one kind of pane
    /// and no undo the two lists are the same one, and this is it.
    pub fn iter(&self) -> impl Iterator<Item = &Pane> {
        self.panes.iter()
    }

    /// The pane with this id, if it is still open.
    pub fn get(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id() == id)
    }

    /// The pane with this id, for reporting agent progress into it.
    ///
    /// Crate-private for the same reason as [`Pane::session_mut`].
    pub(crate) fn get_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|pane| pane.id() == id)
    }

    /// The focused pane.
    ///
    /// `Option` rather than an `expect`, exactly as `TabStrip::active` is: the
    /// group really is never empty and `repair` really does keep the
    /// focused id resident, but an accessor that panics when they are both
    /// wrong is how a UI ends up with a special case in a different file.
    pub fn focused(&self) -> Option<&Pane> {
        self.get(self.focused)
    }

    /// The focused pane's id.
    pub fn focused_id(&self) -> PaneId {
        self.focused
    }

    /// Whether a pane is the focused one. Exactly one is, ever.
    pub fn is_focused(&self, id: PaneId) -> bool {
        self.focused == id
    }

    /// Where a pane sits in render order.
    pub fn index_of(&self, id: PaneId) -> Option<usize> {
        self.panes.iter().position(|pane| pane.id() == id)
    }

    /// Which way the panes are divided.
    pub fn axis(&self) -> SplitAxis {
        self.axis
    }

    /// Whether the tab is showing more than one pane.
    ///
    /// Warp's `in_split_pane`, derived rather than stored. Warp keeps it in
    /// its focus state and has to recompute it in `handle_pane_count_change`
    /// on every add, close, move, hide and reveal; a group that has collapsed
    /// back to one pane has to be indistinguishable from one that never split,
    /// and deriving it is the only way that cannot come apart.
    pub fn is_split(&self) -> bool {
        self.panes.len() > 1
    }

    /// Panes in most-recently-focused order, the focused one first.
    pub fn mru(&self) -> &[PaneId] {
        &self.mru
    }

    /// Inserts a new pane named `title` beside the focused one, and focuses
    /// it. Warp focuses the new pane on every split path a person can reach.
    ///
    /// The first split is what decides the group's axis. After that the axis
    /// is fixed, and a `direction` across it keeps only its before-or-after
    /// sense: `Down` on a row inserts to the right of the focused pane rather
    /// than below it. Warp would nest a perpendicular branch inside that one
    /// cell and get a 2×2; a flat vector cannot represent that, and pretending
    /// otherwise would be worse than saying so. The upgrade path, if nested
    /// splits are ever wanted, is to grow this type into Warp's `PaneNode`
    /// enum — this is its one-branch case, so nothing here is thrown away.
    pub fn split(&mut self, direction: Direction, title: impl Into<String>) -> PaneEffect {
        if !self.is_split() {
            self.axis = direction.axis();
        }

        let pane = Pane::new(title);
        let id = pane.id();
        let at = match self.index_of(self.focused) {
            Some(index) if direction.is_leading() => index,
            Some(index) => index + 1,
            // Unreachable: `repair` keeps the focused id resident. Appending
            // beats dropping the pane on the floor.
            None => self.panes.len(),
        };

        self.panes.insert(at, pane);
        self.repair(Some(id));
        PaneEffect::Changed
    }

    /// Removes a pane, and reports [`PaneEffect::GroupEmptied`] rather than
    /// removing the last one.
    ///
    /// The group never represents itself as empty, not even briefly. That is
    /// Warp's rule — `close_pane` on the last visible pane emits `Exited` and
    /// leaves the pane in place — and it is what keeps "and then the tab goes,
    /// and then the window does" written down in exactly one place.
    pub fn close(&mut self, id: PaneId) -> PaneEffect {
        let Some(index) = self.index_of(id) else {
            return PaneEffect::Unchanged;
        };

        if !self.is_split() {
            return PaneEffect::GroupEmptied;
        }

        self.panes.remove(index);
        // `repair` prefers whoever is at the front of the MRU list once the
        // closed pane is gone, so closing the focused pane needs no successor
        // computed here and closing any other needs nothing at all.
        self.repair(None);
        PaneEffect::Changed
    }

    /// Focuses a pane.
    pub fn focus(&mut self, id: PaneId) -> PaneEffect {
        if self.focused == id || self.get(id).is_none() {
            return PaneEffect::Unchanged;
        }
        self.repair(Some(id));
        PaneEffect::Changed
    }

    /// The single place `focused` is ever assigned, and the single place the
    /// MRU list is pruned.
    ///
    /// It picks by identity, in order of preference: the pane the caller asked
    /// for, the pane that was already focused, the most recently focused
    /// survivor, and finally the first pane. No branch of it knows what a
    /// removal did to anyone's position, which is exactly why none of them can
    /// get it wrong.
    fn repair(&mut self, preferred: Option<PaneId>) {
        let open: Vec<PaneId> = self.panes.iter().map(Pane::id).collect();
        self.mru.retain(|id| open.contains(id));

        let Some(chosen) = preferred
            .filter(|id| self.get(*id).is_some())
            .or_else(|| self.get(self.focused).map(Pane::id))
            .or_else(|| self.mru.first().copied())
            .or_else(|| self.panes.first().map(Pane::id))
        else {
            debug_assert!(false, "the group emptied itself, which `close` forbids");
            return;
        };

        self.focused = chosen;
        self.mru.retain(|id| *id != chosen);
        self.mru.insert(0, chosen);
    }
}
