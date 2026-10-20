//! The panes inside one tab: what a pane is, and the rules for splitting them.
//!
//! A Crook tab used to be exactly one agent session, and that is why this
//! module exists. "View as: Panes | Tabs" is not a setting anybody can watch
//! move until a tab can hold more than one thing to show — both modes would
//! draw the same bar, and the control would be a lie.
//!
//! A tab's panes are a tree, Warp's `PaneNode` (`app/src/pane_group/tree.rs`)
//! and tmux's layout cells: a [`Node`] is one pane, or a [`Branch`] dividing
//! its cell among two or more children along one axis. That is what lets a
//! tab hold a 2×2, or an agent beside two watchers stacked on top of each
//! other — neither of which a single row or a single column can draw.
//!
//! Two rules keep the tree in one shape for each arrangement, so that no
//! reader has to tell apart two trees that draw the same thing:
//!
//! - **No branch has one child.** A close that leaves a branch with one child
//!   puts the child in its place, with the branch's share of the parent.
//! - **No branch has a child dividing along its own axis.** Such a child is
//!   folded into its parent, its children taking its share between them in
//!   the proportions they had. So three "split right"s make one row of four
//!   rather than a right-leaning tree, which is Warp's same-axis fast path
//!   (`PaneBranch::split`) and tmux's, and is what makes a divider dragged in
//!   a row of four move only the two panes beside it.
//!
//! Every list-shaped reader — the panel's rows, the session file's pane list,
//! the shells, cycling with `focus-next-pane` — sees the leaves in render
//! order, depth first: left to right in a row, top to bottom in a column.
//! What only the tree can answer is spatial, and [`PaneGroup::neighbour`]
//! answers it from the laid-out cells rather than from that order.
//!
//! The one thing here Warp does not have is tmux's zoom: one bool that gives
//! the focused pane the whole body and hides the rest without touching the
//! split they are in. [`PaneGroup::visible`] is the list the body draws, and
//! it is the only reader of the group that ever sees fewer panes than there
//! are.

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

    /// The bare number, for the places a pane has to be named outside the
    /// process: a sandboxed plugin, and the shell in the pane through
    /// `CROOK_PANE_ID` — each gets something it can key state on and nothing
    /// it could reach the pane with, and the two agree on the name.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// Which way a branch divides its cell. Warp's `SplitDirection`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SplitAxis {
    /// Side by side, in a row. What "split right" makes.
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

    /// The direction along `axis` that goes before, or after.
    fn along(axis: SplitAxis, leading: bool) -> Self {
        match (axis, leading) {
            (SplitAxis::Horizontal, true) => Self::Left,
            (SplitAxis::Horizontal, false) => Self::Right,
            (SplitAxis::Vertical, true) => Self::Up,
            (SplitAxis::Vertical, false) => Self::Down,
        }
    }
}

/// One pane: an identity, and what it shows.
///
/// Both fields are private for the reason [`Tab`](super::Tab)'s are: a public
/// `id` is settable through the `&mut Pane` the agent-progress path hands out,
/// and two panes sharing an id makes every lookup here resolve to the wrong
/// session.
#[derive(Debug)]
pub struct Pane {
    id: PaneId,
    session: AgentSession,
    /// How large a share of the branch it is in this pane takes.
    ///
    /// One for a pane nobody has resized, which is what makes a fresh split an
    /// even one. A divider dragged between two panes moves weight from one to
    /// the other and leaves every other pane's alone — the two of them are the
    /// only ones whose extent the drag changed.
    ///
    /// A weight rather than a fraction, because that is what a flex layout
    /// takes and because it survives a pane being closed: three equal panes
    /// that lose one leave two weights of 1, and the layout divides what is
    /// left between them.
    flex: f32,
}

/// The share a pane nobody has resized takes.
pub const DEFAULT_FLEX: f32 = 1.;

/// How much of a pair of panes one press of the resize chords moves.
///
/// A share of the two rather than a count of pixels, for the reason
/// [`Pane::flex`] is one: only the layout knows how wide a pane came out, and
/// a keyboard resize runs before any of it. Five per cent is small enough to
/// aim with and large enough to be worth a keystroke.
pub const RESIZE_STEP: f32 = 0.05;

/// The smallest share a pane may be dragged down to.
///
/// A divider that could take a pane to zero would make it disappear with no
/// way to get it back — the divider would be on top of its neighbour, and the
/// pane behind it would have no edge left to grab.
const MIN_FLEX: f32 = 0.08;

impl Pane {
    /// A pane over a new session named `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: PaneId::next(),
            session: AgentSession::new(title),
            flex: DEFAULT_FLEX,
        }
    }

    /// How large a share of the branch it is in this pane takes.
    pub fn flex(&self) -> f32 {
        self.flex
    }

    /// Restores a share a previous session recorded.
    ///
    /// Clamped and defaulted past anything unusable, because the number comes
    /// out of a file: a zero, a negative or a `NaN` would be a pane a flex
    /// layout gives no space to, which is a pane nobody can see or get back.
    pub(crate) fn set_flex(&mut self, flex: f32) {
        self.flex = if flex.is_finite() && flex >= MIN_FLEX {
            flex
        } else {
            DEFAULT_FLEX
        };
    }

    /// This pane's identity, for as long as it is open.
    pub fn id(&self) -> PaneId {
        self.id
    }

    /// The agent session behind it.
    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    /// The session, for reporting the agent's progress into it.
    ///
    /// Crate-private on purpose: mutating a session changes what the tab bar
    /// draws, so the only way in from outside is `Workspace::update_session`,
    /// which notifies in the same call.
    pub(crate) fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    /// What to print for this pane, in the panel and in its own body.
    pub fn title(&self) -> &str {
        self.session.display_title()
    }

    /// What the agent in it is doing, as the row's dot says it.
    pub fn status(&self) -> AgentStatus {
        self.session.shown_status()
    }
}

/// One cell of a tab's split: a pane filling it, or a branch dividing it.
///
/// The variants are public so the body and the session file can walk the
/// tree, and nothing outside this module can build or change one: every
/// change goes through [`PaneGroup`], which is what keeps the two rules in
/// the module docs true.
#[derive(Debug)]
pub enum Node {
    /// A pane, filling the cell.
    ///
    /// Boxed because a pane is many times the size of a branch, and every
    /// branch would otherwise be paying for one.
    Leaf(Box<Pane>),
    /// Two or more cells side by side, or stacked.
    Branch(Branch),
}

/// Two or more cells dividing one along an axis.
#[derive(Debug)]
pub struct Branch {
    axis: SplitAxis,
    /// The branch's own share of the branch it is in, the way
    /// [`Pane::flex`] is a pane's.
    flex: f32,
    /// In render order; never fewer than two, and none of them a branch
    /// along `axis`.
    children: Vec<Node>,
}

impl Branch {
    /// Which way the children are divided.
    pub fn axis(&self) -> SplitAxis {
        self.axis
    }

    /// The cells, in render order.
    pub fn children(&self) -> &[Node] {
        &self.children
    }
}

impl Node {
    /// What a cell is turned into for the moment it is taken out of the tree
    /// and put back. Never left anywhere a reader can see it.
    fn placeholder() -> Self {
        Self::Branch(Branch {
            axis: SplitAxis::default(),
            flex: DEFAULT_FLEX,
            children: Vec::new(),
        })
    }

    /// This cell's share of the branch it is in.
    pub fn flex(&self) -> f32 {
        match self {
            Self::Leaf(pane) => pane.flex,
            Self::Branch(branch) => branch.flex,
        }
    }

    fn set_flex(&mut self, flex: f32) {
        match self {
            Self::Leaf(pane) => pane.flex = flex,
            Self::Branch(branch) => branch.flex = flex,
        }
    }

    /// The first pane in the cell, in render order.
    ///
    /// What names a divider: the two cells either side of one start with two
    /// different panes, and no other pair of adjacent cells anywhere in the
    /// tree starts with the same two — a cell's first pane is the first pane
    /// of every cell that starts it, and only one of those has a cell before
    /// it in a branch. See [`PaneGroup::resize`].
    pub fn first_pane(&self) -> PaneId {
        match self {
            Self::Leaf(pane) => pane.id(),
            Self::Branch(branch) => branch.children.first().map_or(PaneId(0), Node::first_pane),
        }
    }

    /// The last pane in the cell, in render order.
    fn last_pane(&self) -> PaneId {
        match self {
            Self::Leaf(pane) => pane.id(),
            Self::Branch(branch) => branch.children.last().map_or(PaneId(0), Node::last_pane),
        }
    }

    fn collect<'a>(&'a self, into: &mut Vec<&'a Pane>) {
        match self {
            Self::Leaf(pane) => into.push(pane),
            Self::Branch(branch) => {
                for child in &branch.children {
                    child.collect(into);
                }
            }
        }
    }

    fn into_panes(self, into: &mut Vec<Pane>) {
        match self {
            Self::Leaf(pane) => into.push(*pane),
            Self::Branch(branch) => {
                for child in branch.children {
                    child.into_panes(into);
                }
            }
        }
    }

    fn find(&self, id: PaneId) -> Option<&Pane> {
        match self {
            Self::Leaf(pane) => (pane.id() == id).then_some(&**pane),
            Self::Branch(branch) => branch.children.iter().find_map(|child| child.find(id)),
        }
    }

    fn find_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        match self {
            Self::Leaf(pane) => (pane.id() == id).then_some(&mut **pane),
            Self::Branch(branch) => branch
                .children
                .iter_mut()
                .find_map(|child| child.find_mut(id)),
        }
    }

    /// The child indices from this cell down to the pane, or `None` when the
    /// pane is not in it.
    fn path_to(&self, id: PaneId) -> Option<Vec<usize>> {
        match self {
            Self::Leaf(pane) => (pane.id() == id).then(Vec::new),
            Self::Branch(branch) => branch.children.iter().enumerate().find_map(|(at, child)| {
                let mut path = child.path_to(id)?;
                path.insert(0, at);
                Some(path)
            }),
        }
    }

    fn at(&self, path: &[usize]) -> &Node {
        match (path.split_first(), self) {
            (Some((&at, rest)), Self::Branch(branch)) if at < branch.children.len() => {
                branch.children[at].at(rest)
            }
            (_, node) => node,
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> &mut Node {
        let Some((&at, rest)) = path.split_first() else {
            return self;
        };
        if !matches!(self, Self::Branch(branch) if at < branch.children.len()) {
            return self;
        }
        match self {
            Self::Branch(branch) => branch.children[at].at_mut(rest),
            node => node,
        }
    }

    /// The branch holding the divider between a cell starting with `before`
    /// and the next one starting with `after`, and the first cell's index.
    fn divider_mut(&mut self, before: PaneId, after: PaneId) -> Option<(&mut Branch, usize)> {
        let Self::Branch(branch) = self else {
            return None;
        };
        if let Some(at) = branch
            .children
            .windows(2)
            .position(|pair| pair[0].first_pane() == before && pair[1].first_pane() == after)
        {
            return Some((branch, at));
        }
        branch
            .children
            .iter_mut()
            .find_map(|child| child.divider_mut(before, after))
    }

    fn is_even(&self) -> bool {
        (self.flex() - DEFAULT_FLEX).abs() < f32::EPSILON
            && match self {
                Self::Leaf(_) => true,
                Self::Branch(branch) => branch.children.iter().all(Node::is_even),
            }
    }

    fn even_out(&mut self) {
        self.set_flex(DEFAULT_FLEX);
        if let Self::Branch(branch) = self {
            for child in &mut branch.children {
                child.even_out();
            }
        }
    }

    fn layout(&self) -> Layout {
        match self {
            Self::Leaf(pane) => Layout::Pane(pane.id()),
            Self::Branch(branch) => Layout::Split {
                axis: branch.axis,
                flex: branch.flex,
                children: branch.children.iter().map(Node::layout).collect(),
            },
        }
    }

    /// Lays the cell out over `cell`, writing down where every pane in it
    /// lands.
    ///
    /// Each boundary is computed once and handed to both cells beside it, so
    /// two neighbours meet at exactly the same number — which is what lets
    /// [`Cell::borders`] ask for touching edges rather than nearly touching
    /// ones.
    fn lay_out(&self, cell: Cell, into: &mut Vec<(PaneId, Cell)>) {
        let branch = match self {
            Self::Leaf(pane) => {
                into.push((pane.id(), cell));
                return;
            }
            Self::Branch(branch) => branch,
        };
        let total: f32 = branch.children.iter().map(Node::flex).sum();
        let (start, end) = cell.span(branch.axis);
        let mut taken = 0.;
        let mut from = start;
        for (at, child) in branch.children.iter().enumerate() {
            taken += child.flex();
            let to = if at + 1 == branch.children.len() || total <= 0. {
                end
            } else {
                start + (end - start) * taken / total
            };
            child.lay_out(cell.with_span(branch.axis, from, to), into);
            from = to;
        }
    }
}

/// Puts the tree back into the module's one shape for its arrangement — see
/// the module docs — or `None` for a cell with no panes left in it.
///
/// Bottom up, so a child is already in shape when its parent looks at it: a
/// branch left with one child hands that child its own share, and a child
/// dividing along its parent's axis is folded in, its children scaled to
/// take exactly the share it had.
///
/// Except at the root, whose share is of nothing: the cell left there keeps
/// its own, as a pane left alone in its tab always kept the share it had.
fn normalized(node: Node, root: bool) -> Option<Node> {
    let Branch {
        axis,
        flex,
        children,
    } = match node {
        Node::Leaf(pane) => return Some(Node::Leaf(pane)),
        Node::Branch(branch) => branch,
    };

    let mut kept = Vec::with_capacity(children.len());
    for child in children
        .into_iter()
        .filter_map(|child| normalized(child, false))
    {
        match child {
            Node::Branch(inner) if inner.axis == axis => {
                let total: f32 = inner.children.iter().map(Node::flex).sum();
                let scale = if total > 0. { inner.flex / total } else { 1. };
                for mut grandchild in inner.children {
                    grandchild.set_flex(grandchild.flex() * scale);
                    kept.push(grandchild);
                }
            }
            child => kept.push(child),
        }
    }

    if kept.len() > 1 {
        return Some(Node::Branch(Branch {
            axis,
            flex,
            children: kept,
        }));
    }
    kept.pop().map(|mut only| {
        if !root {
            only.set_flex(flex);
        }
        only
    })
}

/// A share read out of a file, or out of a layout kept since: anything a flex
/// layout could not give room to becomes the default. See [`Pane::set_flex`].
fn sane_flex(flex: f32) -> f32 {
    if flex.is_finite() && flex > 0. {
        flex
    } else {
        DEFAULT_FLEX
    }
}

/// Builds the cells `layout` describes out of `panes`, taking each pane it
/// names out of the list. A pane it names that is not there, or names twice,
/// is left out; the caller normalises what is left.
fn assemble(layout: &Layout, panes: &mut Vec<Pane>) -> Option<Node> {
    match layout {
        Layout::Pane(id) => {
            let at = panes.iter().position(|pane| pane.id() == *id)?;
            Some(Node::Leaf(Box::new(panes.remove(at))))
        }
        Layout::Split {
            axis,
            flex,
            children,
        } => Some(Node::Branch(Branch {
            axis: *axis,
            flex: sane_flex(*flex),
            children: children
                .iter()
                .filter_map(|child| assemble(child, panes))
                .collect(),
        })),
    }
}

/// Where a pane sits in a unit square that stands for the body.
///
/// A unit square rather than pixels, because whether one pane is beside
/// another does not depend on how many pixels either got — only on where the
/// shares put their edges.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Cell {
    x: (f32, f32),
    y: (f32, f32),
}

impl Cell {
    const WHOLE: Self = Self {
        x: (0., 1.),
        y: (0., 1.),
    };

    fn span(self, axis: SplitAxis) -> (f32, f32) {
        match axis {
            SplitAxis::Horizontal => self.x,
            SplitAxis::Vertical => self.y,
        }
    }

    fn with_span(self, axis: SplitAxis, from: f32, to: f32) -> Self {
        match axis {
            SplitAxis::Horizontal => Self {
                x: (from, to),
                ..self
            },
            SplitAxis::Vertical => Self {
                y: (from, to),
                ..self
            },
        }
    }

    /// Whether `other` lies directly beyond this cell's edge in `direction`:
    /// its near edge is this one's far edge, and the two overlap across it.
    ///
    /// Touching at a corner is not bordering. In a 2×2 the pane diagonally
    /// opposite shares one point with this one, and "right" never means
    /// "right and down".
    fn borders(self, other: Self, direction: Direction) -> bool {
        const TOUCH: f32 = 1e-4;
        let (edge, near, across, theirs) = match direction {
            Direction::Right => (self.x.1, other.x.0, self.y, other.y),
            Direction::Left => (self.x.0, other.x.1, self.y, other.y),
            Direction::Down => (self.y.1, other.y.0, self.x, other.x),
            Direction::Up => (self.y.0, other.y.1, self.x, other.x),
        };
        let overlap = across.1.min(theirs.1) - across.0.max(theirs.0);
        (edge - near).abs() < TOUCH && overlap > TOUCH
    }
}

/// The shape of a tab's split, by pane identity, with no panes in it.
///
/// What a session file is turned into on the way back in, and what a closed
/// pane keeps of the split it was closed out of — see [`PanePlace`]. A leaf's
/// share is the pane's own, so only a branch carries one here.
#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    /// The cell this pane fills.
    Pane(PaneId),
    /// Cells divided along `axis`, with this share of the branch they are in.
    Split {
        /// Which way the children are divided.
        axis: SplitAxis,
        /// The branch's share of the one it is in.
        flex: f32,
        /// The cells, in render order.
        children: Vec<Layout>,
    },
}

impl Layout {
    fn contains(&self, id: PaneId) -> bool {
        match self {
            Self::Pane(pane) => *pane == id,
            Self::Split { children, .. } => children.iter().any(|child| child.contains(id)),
        }
    }

    /// The same shape, with one pane standing where another was.
    fn renamed(&self, from: PaneId, to: PaneId) -> Self {
        match self {
            Self::Pane(pane) => Self::Pane(if *pane == from { to } else { *pane }),
            Self::Split {
                axis,
                flex,
                children,
            } => Self::Split {
                axis: *axis,
                flex: *flex,
                children: children
                    .iter()
                    .map(|child| child.renamed(from, to))
                    .collect(),
            },
        }
    }
}

/// Where a pane was in its tab's split, for putting it back there.
///
/// The whole split as it was, rather than an index or a neighbour, because
/// in a tree "where it was" is a position in every branch above it: a pane
/// closed out of the corner of a 2×2 collapses its column into one pane, and
/// only the shape from before says the column was there to be split again.
/// Restored whole — every share as it was — when no pane has been opened in
/// the tab since; otherwise the split has moved on and the pane goes back
/// beside the neighbour it had, see [`PaneGroup::reopen`].
#[derive(Clone, Debug)]
pub struct PanePlace {
    layout: Layout,
    pane: PaneId,
    /// Every pane's share at the moment of the close. A close moves shares —
    /// the branch it collapsed hands its share to the cell left in it — so
    /// the shape alone does not put them back.
    flexes: Vec<(PaneId, f32)>,
    /// The pane it was beside in its branch, and which side of it it was on.
    beside: Option<(PaneId, Direction)>,
}

impl PanePlace {
    /// Hands a pane's new id to this place, for a pane that was closed after
    /// this one and has come back first under a new one.
    ///
    /// Without it the pane coming back is a stranger to the split this place
    /// kept, and [`PaneGroup::reopen`] takes it for a pane opened since and
    /// gives up the shape it could have restored whole.
    pub(crate) fn renamed(&mut self, was: PaneId, is: PaneId) {
        self.layout = self.layout.renamed(was, is);
        for (pane, _) in &mut self.flexes {
            if *pane == was {
                *pane = is;
            }
        }
        if let Some((anchor, _)) = &mut self.beside
            && *anchor == was
        {
            *anchor = is;
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

/// The panes of one tab, as a tree of splits, with one of them focused.
///
/// The tree is private and only this type changes it, which is what keeps
/// `focused` resident and the tree in shape without a second invariant kept
/// somewhere else.
#[derive(Debug)]
pub struct PaneGroup {
    root: Node,
    /// The focused pane's *identity*, never its position — the same reason
    /// [`TabStrip`](super::TabStrip) stores an identity for the active tab.
    focused: PaneId,
    /// Most-recently-focused first. Warp's `pane_history`, and it is what
    /// picks the successor when the focused pane is closed, and which of two
    /// panes beside the focused one a direction goes to.
    mru: Vec<PaneId>,
    /// Whether the focused pane has the whole body to itself for now.
    ///
    /// tmux's `zoom-pane`. One bool rather than a second layout, because the
    /// split is *kept*: every weight above stays what it was, and the layout
    /// simply stops drawing the other panes until this is cleared. It is
    /// cleared by anything that would change what the kept layout means —
    /// a split, a close, the focus moving — so that a zoom never outlives the
    /// arrangement it was a zoom of. See [`Self::toggle_zoom`].
    zoomed: bool,
}

impl PaneGroup {
    /// A group of one pane, over a new session named `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self::of(Pane::new(title))
    }

    /// A group of exactly one pane, focused.
    fn of(pane: Pane) -> Self {
        let id = pane.id();
        Self {
            root: Node::Leaf(Box::new(pane)),
            focused: id,
            mru: vec![id],
            zoomed: false,
        }
    }

    /// The group `layout` describes, out of `panes`, or `None` for no panes.
    ///
    /// For a session file, which is a list of panes and a shape naming them.
    /// The shape is checked rather than trusted, because it is a file: a pane
    /// it names twice or not at all, or a branch left with nothing in it, is
    /// mended the way a close would mend it, and a pane it leaves out goes
    /// after the rest in a row rather than being lost.
    ///
    /// The last pane is focused and the order of use is the reverse of the
    /// render order, which is exactly what splitting the panes off one by one
    /// left behind — and what a file written before the tree was kept still
    /// restores to.
    pub(crate) fn from_layout(mut panes: Vec<Pane>, layout: &Layout) -> Option<Self> {
        let placed = assemble(layout, &mut panes);
        let root = match placed {
            Some(placed) if panes.is_empty() => placed,
            placed => {
                let mut children: Vec<Node> = placed.into_iter().collect();
                children.extend(panes.into_iter().map(|pane| Node::Leaf(Box::new(pane))));
                Node::Branch(Branch {
                    axis: SplitAxis::Horizontal,
                    flex: DEFAULT_FLEX,
                    children,
                })
            }
        };
        let root = normalized(root, true)?;

        let mut leaves = Vec::new();
        root.collect(&mut leaves);
        let mut mru: Vec<PaneId> = leaves.iter().map(|pane| pane.id()).collect();
        mru.reverse();
        let focused = *mru.first()?;
        Some(Self {
            root,
            focused,
            mru,
            zoomed: false,
        })
    }

    /// Every pane, in render order.
    fn panes(&self) -> Vec<&Pane> {
        let mut panes = Vec::new();
        self.root.collect(&mut panes);
        panes
    }

    /// How many panes there are. Never zero.
    pub fn len(&self) -> usize {
        self.panes().len()
    }

    /// Every pane, in render order — depth first, so left to right in a row
    /// and top to bottom in a column.
    ///
    /// Warp needs both `pane_ids()` and `visible_pane_ids()` because a pane
    /// can be hidden for five different reasons (a move, a job, a temporary
    /// replacement, an undoable close, a child agent). With one kind of pane
    /// and no undo the two lists are the same one, and this is it.
    pub fn iter(&self) -> impl Iterator<Item = &Pane> {
        self.panes().into_iter()
    }

    /// The panes the body draws, in render order: every one of them, or
    /// only the focused one while the group is zoomed.
    ///
    /// The one reader that differs from [`Self::iter`], and the only one that
    /// should: the panel's rows, the session file and the shells all want
    /// every pane, because a hidden pane is still open, still running and
    /// still the way back out of the zoom.
    pub fn visible(&self) -> impl Iterator<Item = &Pane> {
        self.panes()
            .into_iter()
            .filter(move |pane| !self.zoomed || pane.id() == self.focused)
    }

    /// Whether the body is drawing this pane right now.
    pub fn is_visible(&self, id: PaneId) -> bool {
        self.get(id).is_some() && (!self.zoomed || self.focused == id)
    }

    /// Whether the focused pane has the whole body to itself.
    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    /// The whole split, for the body to draw and the session file to keep.
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// The shape of the split, with no panes in it.
    #[cfg(test)]
    pub fn layout(&self) -> Layout {
        self.root.layout()
    }

    /// The shares of the outermost split's cells, or the one pane's own.
    #[cfg(test)]
    pub fn root_flexes(&self) -> Vec<f32> {
        match &self.root {
            Node::Leaf(pane) => vec![pane.flex],
            Node::Branch(branch) => branch.children.iter().map(Node::flex).collect(),
        }
    }

    /// Which way the outermost split divides the tab, or `None` for a tab of
    /// one pane.
    #[cfg(test)]
    pub fn root_axis(&self) -> Option<SplitAxis> {
        match &self.root {
            Node::Leaf(_) => None,
            Node::Branch(branch) => Some(branch.axis),
        }
    }

    /// The pane with this id, if it is still open.
    pub fn get(&self, id: PaneId) -> Option<&Pane> {
        self.root.find(id)
    }

    /// The pane with this id, for reporting agent progress into it.
    ///
    /// Crate-private for the same reason as [`Pane::session_mut`].
    pub(crate) fn get_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.root.find_mut(id)
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
        self.panes().iter().position(|pane| pane.id() == id)
    }

    /// Whether the tab is showing more than one pane.
    ///
    /// Warp's `in_split_pane`, derived rather than stored. Warp keeps it in
    /// its focus state and has to recompute it in `handle_pane_count_change`
    /// on every add, close, move, hide and reveal; a group that has collapsed
    /// back to one pane has to be indistinguishable from one that never split,
    /// and deriving it is the only way that cannot come apart.
    pub fn is_split(&self) -> bool {
        matches!(self.root, Node::Branch(_))
    }

    /// Panes in most-recently-focused order, the focused one first.
    #[cfg(test)]
    pub fn mru(&self) -> &[PaneId] {
        &self.mru
    }

    /// Splits the focused pane along `direction`'s axis, puts a new pane named
    /// `title` on `direction`'s side of it, and focuses the new one. Warp
    /// focuses the new pane on every split path a person can reach.
    ///
    /// Always the *focused* pane, along the axis asked for: "split down" on
    /// the right half of a row splits that half into two stacked panes and
    /// leaves the left one alone. When the focused pane's branch already
    /// divides along that axis, the new pane joins it beside the focused one
    /// instead of nesting a branch of one more direction inside it — tmux's
    /// and Warp's rule, and the one that makes three "split right"s a row of
    /// four even panes rather than halves of halves.
    pub fn split(&mut self, direction: Direction, title: impl Into<String>) -> PaneEffect {
        let pane = Pane::new(title);
        let id = pane.id();
        self.insert_beside(self.focused, direction, Node::Leaf(Box::new(pane)));
        // A zoom is of one arrangement, and this is a different one: the new
        // pane has to be seen to be used, and it is the one being focused.
        self.zoomed = false;
        self.repair(Some(id));
        PaneEffect::Changed
    }

    /// Puts `node` on `direction`'s side of `anchor`: into the anchor's
    /// branch when that divides the same way, or into a new branch that
    /// takes the anchor's place and its share.
    fn insert_beside(&mut self, anchor: PaneId, direction: Direction, node: Node) {
        let axis = direction.axis();
        let leading = direction.is_leading();
        // Unreachable without one, since `repair` keeps the focused id
        // resident and a kept neighbour is checked; beside the whole split
        // beats dropping the pane on the floor.
        let path = self.root.path_to(anchor).unwrap_or_default();

        if let Some((&at, parent)) = path.split_last()
            && let Node::Branch(branch) = self.root.at_mut(parent)
            && branch.axis == axis
        {
            branch
                .children
                .insert(if leading { at } else { at + 1 }, node);
            return;
        }

        let cell = self.root.at_mut(&path);
        let mut old = std::mem::replace(cell, Node::placeholder());
        let flex = old.flex();
        old.set_flex(DEFAULT_FLEX);
        let children = if leading {
            vec![node, old]
        } else {
            vec![old, node]
        };
        *cell = Node::Branch(Branch {
            axis,
            flex,
            children,
        });
        // Only the unreachable fallback above can have nested a branch inside
        // one along its own axis; this puts it back in shape if it did.
        self.normalize();
    }

    /// Where a pane is in the split, for [`Self::reopen`] to put it back.
    pub(crate) fn place_of(&self, id: PaneId) -> Option<PanePlace> {
        let path = self.root.path_to(id)?;
        let beside = path.split_last().and_then(|(&at, parent)| {
            let Node::Branch(branch) = self.root.at(parent) else {
                return None;
            };
            // The cell before it if there is one, which it went after; the
            // cell after it otherwise, which it went before. The pane of that
            // cell that touched it.
            Some(match at.checked_sub(1) {
                Some(previous) => (
                    branch.children[previous].last_pane(),
                    Direction::along(branch.axis, false),
                ),
                None => (
                    branch.children.get(1)?.first_pane(),
                    Direction::along(branch.axis, true),
                ),
            })
        });
        Some(PanePlace {
            layout: self.root.layout(),
            pane: id,
            flexes: self.iter().map(|pane| (pane.id(), pane.flex)).collect(),
            beside,
        })
    }

    /// Puts a closed pane back where `place` says it was, and focuses it.
    ///
    /// When every pane open now was in the split it was closed out of —
    /// nothing has been split off since, whatever else was closed — the
    /// split comes back as it was then, shares and all, less the panes that
    /// have gone since. Otherwise the arrangement has moved on and that shape
    /// would throw the new panes about, so the pane goes back beside the
    /// neighbour it had, on the side it was on — or, with that neighbour gone
    /// too, to the right of the focused pane.
    pub(crate) fn reopen(&mut self, pane: Pane, place: &PanePlace) {
        let id = pane.id();
        let layout = place.layout.renamed(place.pane, id);
        if self.iter().all(|open| layout.contains(open.id())) {
            let mut panes = Vec::new();
            std::mem::replace(&mut self.root, Node::placeholder()).into_panes(&mut panes);
            panes.push(pane);
            for pane in &mut panes {
                if let Some((_, flex)) = place.flexes.iter().find(|(kept, _)| *kept == pane.id) {
                    pane.flex = *flex;
                }
            }
            let mut root = assemble(&layout, &mut panes).unwrap_or_else(Node::placeholder);
            // Unreachable, since every pane was checked to be in the layout;
            // a pane in a row at the end beats a pane dropped.
            if !panes.is_empty() {
                let mut children = vec![root];
                children.extend(panes.into_iter().map(|pane| Node::Leaf(Box::new(pane))));
                root = Node::Branch(Branch {
                    axis: SplitAxis::Horizontal,
                    flex: DEFAULT_FLEX,
                    children,
                });
            }
            self.root = root;
            self.normalize();
        } else {
            let (anchor, direction) = place
                .beside
                .filter(|(anchor, _)| self.get(*anchor).is_some())
                .unwrap_or((self.focused, Direction::Right));
            self.insert_beside(anchor, direction, Node::Leaf(Box::new(pane)));
        }
        // As a split does: the zoom was of the arrangement before this one,
        // and the pane coming back has to be seen to be used.
        self.zoomed = false;
        self.repair(Some(id));
    }

    /// Removes a pane, and reports [`PaneEffect::GroupEmptied`] rather than
    /// removing the last one.
    ///
    /// The group never represents itself as empty, not even briefly. That is
    /// Warp's rule — `close_pane` on the last visible pane emits `Exited` and
    /// leaves the pane in place — and it is what keeps "and then the tab goes,
    /// and then the window does" written down in exactly one place.
    ///
    /// A branch the close leaves with one cell gives that cell its place and
    /// its share — see the module docs — so closing a corner of a 2×2 leaves
    /// a pane the height of the tab beside the column that is still split.
    pub fn close(&mut self, id: PaneId) -> PaneEffect {
        let Some(path) = self.root.path_to(id) else {
            return PaneEffect::Unchanged;
        };
        let Some((&at, parent)) = path.split_last() else {
            return PaneEffect::GroupEmptied;
        };
        if let Node::Branch(branch) = self.root.at_mut(parent) {
            branch.children.remove(at);
        }
        self.normalize();
        // Whichever pane went: the zoomed one, whose successor a person has
        // to be able to see the split around, or a hidden one, whose going
        // has changed the layout being kept under the zoom.
        self.zoomed = false;
        // `repair` prefers whoever is at the front of the MRU list once the
        // closed pane is gone, so closing the focused pane needs no successor
        // computed here and closing any other needs nothing at all.
        self.repair(None);
        PaneEffect::Changed
    }

    /// Puts the tree back in shape after a change that may have bent it.
    fn normalize(&mut self) {
        let root = std::mem::replace(&mut self.root, Node::placeholder());
        match normalized(root, true) {
            Some(root) => self.root = root,
            None => debug_assert!(false, "the group emptied itself, which `close` forbids"),
        }
    }

    /// Moves the divider between two adjacent cells of one branch.
    ///
    /// The divider is named by the first pane of the cell on each side of it,
    /// which names exactly one divider in the whole tree — see
    /// [`Node::first_pane`] — at whatever depth it is. `leading` is the share
    /// of the pair that goes to the first of them, from zero to one; the two
    /// keep the weight they had between them, so every other cell in the
    /// branch is untouched and the branch still adds up to what the layout
    /// was already dividing.
    ///
    /// The pair is looked up rather than trusted: a divider knows its own
    /// neighbours, but an action is a value and can arrive after the panes it
    /// names have moved or closed.
    pub fn resize(&mut self, before: PaneId, after: PaneId, leading: f32) -> PaneEffect {
        let Some((branch, first)) = self.root.divider_mut(before, after) else {
            return PaneEffect::Unchanged;
        };
        let second = first + 1;

        let total = branch.children[first].flex() + branch.children[second].flex();
        // No smaller than the pair can give both of its cells: folding a
        // branch into its parent scales its cells' shares down with it, and a
        // fixed floor above half of what a pair holds would make every drag
        // between them grow the pair at its neighbours' expense.
        let floor = MIN_FLEX.min(total / 2.);
        let leading = (total * leading).clamp(floor, total - floor);
        let trailing = total - leading;

        // A drag that would not move anything by a pixel is not a change, and
        // reporting one would repaint the window at pointer-move rate.
        if (branch.children[first].flex() - leading).abs() < f32::EPSILON
            && (branch.children[second].flex() - trailing).abs() < f32::EPSILON
        {
            return PaneEffect::Unchanged;
        }

        branch.children[first].set_flex(leading);
        branch.children[second].set_flex(trailing);
        PaneEffect::Changed
    }

    /// Gives every cell, at every depth, an equal share of its branch again.
    ///
    /// What a double click on a divider does, and the only way back to an even
    /// split once one has been dragged.
    pub fn even_out(&mut self) -> PaneEffect {
        if self.root.is_even() {
            return PaneEffect::Unchanged;
        }
        self.root.even_out();
        PaneEffect::Changed
    }

    /// The pane beside the focused one in `direction`, if there is one.
    ///
    /// Spatial, from where the shares put every pane: the panes whose near
    /// edge is the focused pane's far edge in that direction and that overlap
    /// it across. Of two or more — the agent with two watchers stacked to its
    /// right — the one used most recently, which is tmux's answer and the
    /// one that makes left-then-right come back to where it started. `None`
    /// where the focused pane is at the edge of the tab, which is what lets
    /// the chord fall through to the shell instead — the same bargain
    /// [`Workspace::command`](crate::workspace::Workspace::command) makes
    /// everywhere else.
    ///
    /// Of the whole split, zoomed or not: [`Self::focus`] ends a zoom, so the
    /// pane it lands on is drawn.
    pub fn neighbour(&self, direction: Direction) -> Option<PaneId> {
        let mut cells = Vec::new();
        self.root.lay_out(Cell::WHOLE, &mut cells);
        let (_, from) = cells.iter().find(|(id, _)| *id == self.focused)?;
        let beside: Vec<PaneId> = cells
            .iter()
            .filter(|(id, cell)| *id != self.focused && from.borders(*cell, direction))
            .map(|(id, _)| *id)
            .collect();
        self.mru
            .iter()
            .copied()
            .find(|id| beside.contains(id))
            .or_else(|| beside.first().copied())
    }

    /// The pane `delta` places along from the focused one in render order,
    /// wrapping round.
    ///
    /// Wrapping, unlike [`Self::neighbour`], because this is the "show me the
    /// next one" gesture rather than a direction: a person cycling through two
    /// panes with one chord expects the second press to come back, and a cycle
    /// that stopped at the end would need a second chord to be useful.
    pub fn cycled(&self, delta: isize) -> Option<PaneId> {
        if !self.is_split() {
            return None;
        }
        let panes = self.panes();
        let len = panes.len() as isize;
        let index = self.index_of(self.focused)? as isize;
        let next = (index + delta).rem_euclid(len);
        panes.get(next as usize).map(|pane| pane.id())
    }

    /// The pane at `index` in render order.
    #[cfg(test)]
    pub fn at(&self, index: usize) -> Option<PaneId> {
        self.panes().get(index).map(|pane| pane.id())
    }

    /// Moves the divider beside the focused pane, giving it more room or less.
    ///
    /// The keyboard's half of [`Self::resize`], and it takes a *step* rather
    /// than a share because a chord has no pointer position to derive one
    /// from. Which divider it moves is decided the only way it can be: one of
    /// the focused pane's own branch — the innermost split it is in — the one
    /// after it, or for the last cell, which has none, the one before it.
    /// That asymmetry is what makes the last pane growable at all.
    pub fn nudge(&mut self, grow: bool) -> PaneEffect {
        let Some(path) = self.root.path_to(self.focused) else {
            return PaneEffect::Unchanged;
        };
        let Some((&index, parent)) = path.split_last() else {
            return PaneEffect::Unchanged;
        };
        let Node::Branch(branch) = self.root.at(parent) else {
            return PaneEffect::Unchanged;
        };
        let (first, second) = if index + 1 < branch.children.len() {
            (index, index + 1)
        } else if index > 0 {
            (index - 1, index)
        } else {
            return PaneEffect::Unchanged;
        };

        let (one, two) = (&branch.children[first], &branch.children[second]);
        let total = one.flex() + two.flex();
        // Growing the *trailing* cell of the pair is shrinking the leading
        // one, and `resize` only speaks in the leading one's share.
        let towards_leading = grow == (first == index);
        let step = RESIZE_STEP * if towards_leading { 1. } else { -1. };
        let leading = (one.flex() / total + step).clamp(0., 1.);

        let (before, after) = (one.first_pane(), two.first_pane());
        self.resize(before, after, leading)
    }

    /// Focuses a pane.
    ///
    /// Focusing a pane the zoom is hiding is the one way to reach it without
    /// a chord — a click on its row in the panel — so the zoom ends here
    /// rather than following the focus to a pane a person has not seen the
    /// context of. The same rule with a chord: `focus-next-pane` on a zoomed
    /// tab shows the split again, with the next pane focused in it.
    pub fn focus(&mut self, id: PaneId) -> PaneEffect {
        if self.focused == id || self.get(id).is_none() {
            return PaneEffect::Unchanged;
        }
        self.zoomed = false;
        self.repair(Some(id));
        PaneEffect::Changed
    }

    /// Gives the focused pane the whole body, or the split back.
    ///
    /// Refused on a tab that was never split, the way [`Self::nudge`] is:
    /// there is nothing to hide, and a chord that is consumed for a zoom of
    /// one pane over itself is a chord taken from the shell for nothing.
    pub fn toggle_zoom(&mut self) -> PaneEffect {
        if !self.is_split() {
            return PaneEffect::Unchanged;
        }
        self.zoomed = !self.zoomed;
        PaneEffect::Changed
    }

    /// Restores a zoom a previous session recorded.
    ///
    /// Through the same refusal [`Self::toggle_zoom`] makes, so a file that
    /// says a single pane was zoomed restores a tab that is not.
    pub(crate) fn set_zoomed(&mut self, zoomed: bool) {
        if zoomed != self.zoomed {
            self.toggle_zoom();
        }
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
        let open: Vec<PaneId> = self.iter().map(Pane::id).collect();
        self.mru.retain(|id| open.contains(id));

        let Some(chosen) = preferred
            .filter(|id| open.contains(id))
            .or_else(|| open.contains(&self.focused).then_some(self.focused))
            .or_else(|| self.mru.first().copied())
            .or_else(|| open.first().copied())
        else {
            debug_assert!(false, "the group emptied itself, which `close` forbids");
            return;
        };

        self.focused = chosen;
        self.mru.retain(|id| *id != chosen);
        self.mru.insert(0, chosen);
    }
}
