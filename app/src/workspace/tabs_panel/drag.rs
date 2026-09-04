//! Picking a row of the panel up and putting it down somewhere else.
//!
//! Two gestures, one mechanism: a tab dragged between rows — into a group, out
//! of one, or past its neighbours — and a whole group dragged as a block.
//! Warp's `Draggable` detaches what it carries into an overlay that follows
//! the pointer; there is no such element here, and a list does not need one.
//! What a person has to be told is *where the thing will land*, and that is a
//! line drawn between two rows. Finder's sidebar, VS Code's explorer and every
//! reorderable list in a settings page say it the same way.
//!
//! # Why the geometry is written down rather than asked for
//!
//! An element knows its own box and nothing else's. "Which row is the pointer
//! over" is a question about all of them, so every row and every heading
//! records the box it painted into one shared list, in paint order, and the
//! grip that owns the gesture reads that list when the pointer moves. It is
//! the trick [`RowGeometry`](super::geometry::RowGeometry) already uses for
//! the scroll target and [`PaneExtent`](crate::pane_split::PaneExtent) for a
//! divider, for the same reason: the number is produced by a paint pass and
//! wanted by an event three frames later.
//!
//! Window coordinates, not content ones. A drop is answered against where the
//! pointer *is*, and the pointer is in the window.
//!
//! The boxes are read back as the list would have drawn them with no line in
//! it, which is not a detail: a line that takes room moves the rows under it,
//! and a drop decided against rows the drop itself moved un-decides itself
//! every other frame. See [`Inner::unparted`].
//!
//! # Why the drop is decided by a pure function
//!
//! [`drop_action`] is the whole gesture's judgement — which gap, whose group,
//! and whether any of it is a change — and it takes a list of slots and a
//! number. So the cases that are actually hard, and that every list gets wrong
//! at least once (the gap between two groups, the space under the last row,
//! the top half of a heading), are tested here in microseconds with no window
//! and no pointer.
//!
//! What it cannot get wrong is the invariant:
//! [`TabStrip::apply`](crate::tab::TabStrip::apply) clamps the group and the
//! neighbour it names against each other, so a target computed from a coarse
//! gesture can be wrong about *where* and still cannot produce a strip the
//! panel is unable to draw.
//!
//! It has one job that clamp cannot do for it, though, and getting it wrong is
//! invisible rather than wrong: the target has to be a target the *panel* can
//! draw. The panel draws a line above a member of the group being joined, or
//! at that group's end, or above a whole block — so a drop that joins a group
//! must name one of that group's own rows, and a block that lands somewhere
//! must name the row a block begins with. Both are said here rather than left
//! to the clamp, because a drop the strip performs correctly and the panel
//! could not promise is a drop nobody asked for.

use std::cell::RefCell;
use std::rc::Rc;

use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::elements::MouseStateHandle;
use crookui_core::event::{DispatchedEvent, Event, MouseButton};
use crookui_core::geometry::{Point, Vector2F, ZIndex};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};

use crate::tab::{GroupId, TabAction, TabId};

use super::super::action::WorkspaceAction;

/// How far the pointer has to travel before a press becomes a drag.
///
/// Below it the gesture is still a click, and a click on a row selects it. A
/// list whose rows moved on the first stray pixel would be a list nobody could
/// click; one that waited too long would feel stuck. Four is what every file
/// manager uses.
const THRESHOLD: f32 = 4.;

/// What a press on one of the panel's slots picks up.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Grip {
    /// A tab's row, and the group it is currently in.
    Tab {
        /// The tab the row stands for.
        tab: TabId,
        /// The group it is in, if any.
        group: Option<GroupId>,
    },
    /// A group's heading, which carries the whole block.
    Heading {
        /// The group the heading names.
        group: GroupId,
        /// Whether its members are folded away, so there are no member rows
        /// under it to aim between.
        collapsed: bool,
    },
}

impl Grip {
    /// What a drag started on this box is carrying.
    fn carries(self) -> Carried {
        match self {
            Self::Tab { tab, .. } => Carried::Tab(tab),
            Self::Heading { group, .. } => Carried::Group(group),
        }
    }
}

/// What a drag in progress is carrying.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Carried {
    /// One tab, which can be dropped into a group, out of one, or between any
    /// two rows.
    Tab(TabId),
    /// A whole group, which only ever lands between blocks.
    Group(GroupId),
}

/// One box the panel drew, as the pointer meets it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Slot {
    /// What was drawn there.
    pub(crate) grip: Grip,
    /// Its top edge, in window coordinates.
    pub(crate) top: f32,
    /// Its bottom edge.
    pub(crate) bottom: f32,
}

impl Slot {
    /// Where the box stops being its own top half.
    fn middle(&self) -> f32 {
        (self.top + self.bottom) / 2.
    }
}

/// The gesture in progress: what it carries, where it began, and whether it
/// has travelled far enough to be a drag at all.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Gesture {
    grip: Grip,
    /// Where the press landed, so that [`THRESHOLD`] is measured from it.
    from: f32,
    /// Where the pointer is now, in the window.
    at: Vector2F,
    /// Whether it has passed the threshold. Until it does, this is a click
    /// that has not been let go of yet and the panel draws nothing.
    dragging: bool,
}

/// Everything the panel's drags share: the slots of the last frame, and the
/// one gesture there can be.
///
/// Cheap to clone — it is an [`Rc`] — because every row takes one every frame,
/// and one cell is also what makes "only one row can be dragged at a time" a
/// fact rather than a rule somebody has to keep.
#[derive(Clone, Default)]
pub(crate) struct PanelDrag(Rc<RefCell<Inner>>);

#[derive(Default)]
struct Inner {
    slots: Vec<Slot>,
    /// How wide the list was, so that a drop outside the panel can be refused.
    width: std::ops::Range<f32>,
    /// Where the last frame parted to make room for the line, and by how
    /// much. See [`Self::unparted`].
    parting: Option<(f32, f32)>,
    gesture: Option<Gesture>,
}

impl Inner {
    /// The slots as the list would have drawn them with no line in it.
    ///
    /// The line is drawn *between* two rows, and the room it takes pushes
    /// every row under it down. Answering the pointer against the boxes that
    /// were actually painted therefore feeds the answer back into itself: the
    /// line appears, the row under the pointer moves out from under it, the
    /// gap it named stops being the one the pointer is in, the line goes away,
    /// the row comes back — a list that strobes at the frame rate anywhere
    /// within the parting's height of a boundary, which is where a person
    /// aiming at that boundary holds the pointer.
    ///
    /// So the question is asked of the list that has no line in it. Every box
    /// under the parting comes back up by its height, which is exactly the
    /// layout of the frame before the drag began, and the answer becomes a
    /// function of the pointer alone. It is the same reason a scrollbar is
    /// sized from the content rather than from itself.
    fn unparted(&self) -> Vec<Slot> {
        let Some((top, height)) = self.parting else {
            return self.slots.clone();
        };
        self.slots
            .iter()
            .map(|slot| {
                if slot.top >= top {
                    Slot {
                        top: slot.top - height,
                        bottom: slot.bottom - height,
                        ..*slot
                    }
                } else {
                    *slot
                }
            })
            .collect()
    }
}

impl PanelDrag {
    /// A panel with nothing being dragged and nothing drawn yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Starts a frame: forgets the slots the last one drew, and records how
    /// wide the list is.
    ///
    /// Cleared rather than overwritten, because a box that is no longer drawn
    /// is a row that has closed — and one left behind would be a place the
    /// pointer could still be answered against.
    fn begin(&self, width: std::ops::Range<f32>) {
        let mut inner = self.0.borrow_mut();
        inner.slots.clear();
        inner.parting = None;
        inner.width = width;
    }

    /// Records one box, in paint order.
    fn record(&self, grip: Grip, top: f32, bottom: f32) {
        self.0.borrow_mut().slots.push(Slot { grip, top, bottom });
    }

    /// Records the room the line took, so the rows under it can be put back
    /// where they would have been. See [`Inner::unparted`].
    fn record_parting(&self, top: f32, height: f32) {
        self.0.borrow_mut().parting = Some((top, height));
    }

    /// Takes note of a press, which is not yet a drag.
    fn press(&self, grip: Grip, at: Vector2F) {
        self.0.borrow_mut().gesture = Some(Gesture {
            grip,
            from: at.y(),
            at,
            dragging: false,
        });
    }

    /// Whether the gesture in progress began on this box.
    fn holds(&self, grip: Grip) -> bool {
        self.0
            .borrow()
            .gesture
            .is_some_and(|gesture| gesture.grip == grip)
    }

    /// Moves the gesture to `at`, reporting whether the panel has to be drawn
    /// again.
    ///
    /// A press that has not passed the threshold moves nothing on screen, so
    /// it asks for no frame: that is what keeps a click that wobbles by a
    /// pixel off the GPU.
    fn moved_to(&self, at: Vector2F) -> bool {
        let mut inner = self.0.borrow_mut();
        let Some(gesture) = inner.gesture.as_mut() else {
            return false;
        };
        let was = gesture.dragging;
        gesture.at = at;
        gesture.dragging = was || (at.y() - gesture.from).abs() >= THRESHOLD;
        gesture.dragging
    }

    /// What is being dragged, once the gesture is one.
    pub(crate) fn carrying(&self) -> Option<Carried> {
        let inner = self.0.borrow();
        let gesture = inner.gesture.filter(|gesture| gesture.dragging)?;
        Some(gesture.grip.carries())
    }

    /// Where a drop right now would put it.
    ///
    /// `None` when nothing is being dragged, and when the drop would change
    /// nothing — which is what leaves the line undrawn while the pointer is
    /// still over the row it picked up.
    pub(crate) fn pending(&self) -> Option<TabAction> {
        let inner = self.0.borrow();
        let gesture = inner.gesture.filter(|gesture| gesture.dragging)?;
        // Outside the panel is nowhere to put a tab. A drag that wandered over
        // the terminal and was let go there has to be a drag that did nothing:
        // the alternative is a list that reorders itself because somebody let
        // go of the mouse in the wrong half of the window, and there is no
        // line drawn out there to have warned them.
        if !inner.width.contains(&gesture.at.x()) {
            return None;
        }
        drop_action(&inner.unparted(), gesture.grip.carries(), gesture.at.y())
    }

    /// Ends the gesture, answering with whether it was a drag at all and with
    /// the drop it landed on.
    ///
    /// The two are separate answers: a drag that ended over the row it started
    /// on has moved nothing and still must not be read as a click.
    fn release(&self) -> (bool, Option<TabAction>) {
        let pending = self.pending();
        let dragged = self.carrying().is_some();
        self.0.borrow_mut().gesture = None;
        (dragged, pending)
    }

    /// Ends the gesture without doing anything with it.
    pub(crate) fn cancel(&self) {
        self.0.borrow_mut().gesture = None;
    }
}

/// The drop `y` names, for what is being carried.
///
/// Reads as three questions, in order: which box is the pointer in, is it in
/// that box's top half or its bottom, and what lies on the far side of the
/// boundary that answer names. A pointer above every box lands before the
/// first row; one below every box lands at the end, which is the only way to
/// put a tab past everything.
pub(crate) fn drop_action(slots: &[Slot], carried: Carried, y: f32) -> Option<TabAction> {
    let first = slots.first()?;
    let last = slots.last()?;

    // The gap the pointer is aiming at, as the box it is over and which half.
    let (index, after) = if y < first.top {
        (0, false)
    } else if y >= last.bottom {
        (slots.len() - 1, true)
    } else {
        let index = slots
            .iter()
            .position(|held| y >= held.top && y < held.bottom)
            .unwrap_or(slots.len() - 1);
        (index, y >= slots[index].middle())
    };

    match carried {
        Carried::Tab(tab) => tab_drop(slots, tab, index, after),
        Carried::Group(group) => group_drop(slots, group, index, after),
    }
}

/// Where a dragged tab lands: which group it joins, and which row it lands in
/// front of.
///
/// The `before` it names may belong to another group entirely — a drop under a
/// group's last member names the first row of whatever is beneath it — and
/// that is deliberate. Naming the *next row in the list* is a fact about the
/// pointer; deciding which side of a group boundary it means is
/// [`TabStrip`](crate::tab::TabStrip)'s clamp, which is the only place that
/// can be sure.
fn tab_drop(slots: &[Slot], tab: TabId, index: usize, after: bool) -> Option<TabAction> {
    let (group, before) = match slots[index].grip {
        // Over a row: above it, or above whatever comes next.
        Grip::Tab { tab: over, group } => {
            if !after {
                (group, Some(over))
            } else {
                (group, next_tab(slots, index))
            }
        }
        // Over a heading: above the whole group, or into it. A folded group
        // has no member rows to aim between, so the bottom half of its heading
        // is the only way into it and means its end.
        Grip::Heading { group, collapsed } => {
            let first = next_tab(slots, index);
            match (after, collapsed) {
                (false, _) => (None, first),
                (true, false) => (Some(group), first),
                (true, true) => (Some(group), None),
            }
        }
    };

    // A drop into a group can only name one of that group's own rows. The gap
    // under a group's last member is also the gap above whatever follows it,
    // so the row below it — which is what the pointer names — belongs to the
    // next block, and `before` then points outside the group being joined.
    // The strip clamps that back into the group's run, so the tab does land
    // where it was aimed; what nothing can do with it is *draw* it, because
    // there is no member row to draw a line above. The one gap it can only
    // ever mean is the group's end, so it is named as the group's end here —
    // the same slot, said in the terms the panel has a line for.
    let before = match (group, before) {
        (Some(joining), Some(named)) if !is_member(slots, named, joining) => None,
        _ => before,
    };

    // Dropping a tab back where it already is: the row above the gap is the
    // tab itself, or the row below it is.
    if before == Some(tab) {
        return None;
    }
    if let Grip::Tab {
        tab: over,
        group: held,
    } = slots[index].grip
        && over == tab
        && group == held
    {
        return None;
    }

    Some(TabAction::MoveTab { tab, group, before })
}

/// Where a dragged group lands: in front of the first tab of the block it was
/// dropped on, or at the end.
///
/// A group only ever lands between blocks, so every gap inside a block reads
/// as the boundary above it. The strip snaps `before` to its block's start for
/// the same reason, which is what makes a coarse answer here a safe one.
fn group_drop(slots: &[Slot], group: GroupId, index: usize, after: bool) -> Option<TabAction> {
    let before = if after {
        next_tab(slots, index)
    } else {
        // The block this box belongs to starts at its heading, or at the row
        // itself when the row is in no group.
        match slots[index].grip {
            Grip::Tab { tab, .. } => Some(tab),
            Grip::Heading { .. } => next_tab(slots, index),
        }
    };
    // Said as the block it lands in front of rather than as the row: the row
    // the pointer named can be the middle of a group, and a block does not go
    // between two of another block's members. The strip snaps it to the same
    // place; saying it here is what gives the panel a line to draw, which is
    // drawn above a *block*.
    let before = before.map(|before| block_start(slots, before));

    // The two gaps its own block already occupies: above its first member,
    // and under its last one. Neither is a move, and a line drawn in either
    // would be promising one.
    let already_there = match before {
        Some(before) => slots.iter().any(|held| {
            matches!(held.grip, Grip::Tab { tab, group: Some(held) } if tab == before && held == group)
        }),
        None => last_group(slots) == Some(group),
    };
    if already_there {
        return None;
    }

    Some(TabAction::MoveGroup { group, before })
}

/// Whether a row of the list is one of `group`'s members.
fn is_member(slots: &[Slot], tab: TabId, group: GroupId) -> bool {
    slots.iter().any(|held| {
        matches!(held.grip, Grip::Tab { tab: held, group: Some(joined) } if held == tab && joined == group)
    })
}

/// The row the block holding `tab` begins with: its group's first member, or
/// `tab` itself when it is in no group.
fn block_start(slots: &[Slot], tab: TabId) -> TabId {
    let group = slots.iter().find_map(|held| match held.grip {
        Grip::Tab { tab: held, group } if held == tab => Some(group),
        _ => None,
    });
    match group.flatten() {
        Some(group) => first_member(slots, group).unwrap_or(tab),
        None => tab,
    }
}

/// A group's first member, as the list drew it.
fn first_member(slots: &[Slot], group: GroupId) -> Option<TabId> {
    slots.iter().find_map(|held| match held.grip {
        Grip::Tab {
            tab,
            group: Some(joined),
        } if joined == group => Some(tab),
        _ => None,
    })
}

/// The group the last row of the list is in, if it is in one.
fn last_group(slots: &[Slot]) -> Option<GroupId> {
    slots.iter().rev().find_map(|held| match held.grip {
        Grip::Tab { group, .. } => Some(group),
        Grip::Heading { .. } => None,
    })?
}

/// The next tab row after `index`, if the list has one.
fn next_tab(slots: &[Slot], index: usize) -> Option<TabId> {
    slots[index + 1..].iter().find_map(|held| match held.grip {
        Grip::Tab { tab, .. } => Some(tab),
        Grip::Heading { .. } => None,
    })
}

/// Whether the insertion line goes above this block, between it and whatever
/// is above it.
pub(crate) fn line_above_block(pending: Option<TabAction>, first: TabId) -> bool {
    match pending {
        Some(TabAction::MoveTab {
            group: None,
            before,
            ..
        }) => before == Some(first),
        Some(TabAction::MoveGroup { before, .. }) => before == Some(first),
        _ => false,
    }
}

/// Whether it goes above this row, inside the group holding it.
pub(crate) fn line_above_member(pending: Option<TabAction>, group: GroupId, member: TabId) -> bool {
    matches!(
        pending,
        Some(TabAction::MoveTab { group: Some(joining), before, .. })
            if joining == group && before == Some(member)
    )
}

/// Whether it goes under a group's last member, inside the group.
pub(crate) fn line_ends_group(pending: Option<TabAction>, group: GroupId) -> bool {
    matches!(
        pending,
        Some(TabAction::MoveTab { group: Some(joining), before: None, .. }) if joining == group
    )
}

/// Whether it goes under everything.
pub(crate) fn line_at_end(pending: Option<TabAction>) -> bool {
    matches!(
        pending,
        Some(TabAction::MoveTab {
            group: None,
            before: None,
            ..
        }) | Some(TabAction::MoveGroup { before: None, .. })
    )
}

/// Wraps the list, clearing the slots the last frame recorded.
///
/// One of these sits outside every row, for the reason
/// [`geometry::Content`](super::geometry::Content) does: the rows record
/// against it, so it has to run first.
pub(crate) struct Frame {
    drag: PanelDrag,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Frame {
    /// Clears the slots and draws `child`.
    pub(crate) fn new(drag: PanelDrag, child: Box<dyn Element>) -> Self {
        Self {
            drag,
            child,
            size: None,
            origin: None,
        }
    }
}

impl Element for Frame {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        // Before the children, because every one of them records against it.
        let width = self.size.map_or(0., |size| size.x());
        self.drag.begin(origin.x()..origin.x() + width);
        self.child.paint(origin, ctx, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// Wraps the line the list parts to make room for: records the room it took,
/// so that the rows under it can be answered where they would have been.
///
/// See [`Inner::unparted`] for why a drop must not be decided against a layout
/// the drop itself moved.
pub(crate) struct Parting {
    drag: PanelDrag,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Parting {
    /// Makes `child` the room a frame gave the line.
    pub(crate) fn new(drag: PanelDrag, child: Box<dyn Element>) -> Self {
        Self {
            drag,
            child,
            size: None,
            origin: None,
        }
    }
}

impl Element for Parting {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let height = self.size.map_or(0., |size| size.y());
        self.drag.record_parting(origin.y(), height);
        self.child.paint(origin, ctx, app);
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// Wraps one row or one heading: records where it was drawn, and turns a press
/// that travels into a drag.
///
/// **It never takes the press itself.** A press is half a click and half a
/// drag, and only the pointer's next move says which — so this notes where the
/// press landed and hands the event on to the row underneath, whose
/// [`Hoverable`](crookui_core::elements::Hoverable) still claims it and still
/// fires on release. What the drag takes is the *moves*, which nothing else in
/// the panel wants.
pub(crate) struct Handle {
    grip: Grip,
    drag: PanelDrag,
    /// The mouse state of whatever drew the box, so that a press that turned
    /// into a drag can be taken back off it.
    pressed: MouseStateHandle,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
    /// The topmost layer the child reached, which is what a press is
    /// hit-tested against. [`Hoverable`](crookui_core::elements::Hoverable)
    /// and [`Scrollable`](crookui_core::elements::Scrollable) keep the same
    /// number for the same reason. See [`Self::takes`].
    child_max_z_index: Option<ZIndex>,
}

impl Handle {
    /// Makes `child` a thing that can be picked up, as `grip`.
    ///
    /// `pressed` is the mouse state of the element that drew it — the row's
    /// own, or the heading's — and it is here for one case: a press that
    /// becomes a drag never reaches that element's release, so the press it is
    /// still holding has to be taken away by hand. It is the same call
    /// [`row`](super::row) makes when a close button stops being drawn, for
    /// the same reason.
    pub(crate) fn new(
        grip: Grip,
        drag: PanelDrag,
        pressed: MouseStateHandle,
        child: Box<dyn Element>,
    ) -> Self {
        Self {
            grip,
            drag,
            pressed,
            child,
            size: None,
            origin: None,
            child_max_z_index: None,
        }
    }

    /// Whether a press at `position` landed on this box and nothing is over
    /// it.
    ///
    /// Against the topmost layer the child reached rather than the one this
    /// element painted into, which is
    /// [`Hoverable`](crookui_core::elements::Hoverable)'s rule and has to be:
    /// a row that draws itself above this element is still the row, not
    /// something covering it. A row does exactly that whenever it is a
    /// [`Stack`](crookui_core::elements::Stack) — which is every row with its
    /// hover card up, and therefore every row a pointer has arrived at. Asking
    /// about the layer *this* painted into found the row's own paint on top of
    /// the press and refused it, so with the card switched on — its default —
    /// no row in the panel could be picked up at all.
    fn takes(&self, position: Vector2F, ctx: &EventContext) -> bool {
        let (Some(origin), Some(size), Some(top)) =
            (self.origin, self.size, self.child_max_z_index)
        else {
            return false;
        };
        ctx.visible_rect(origin, size)
            .is_some_and(|visible| visible.contains_point(position))
            && !ctx.is_covered(Point::from_vec2f(position, top))
    }
}

impl Element for Handle {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let height = self.size.map_or(0., |size| size.y());
        self.drag.record(self.grip, origin.y(), origin.y() + height);
        self.child.paint(origin, ctx, app);
        self.child_max_z_index = Some(ctx.scene.max_active_z_index());
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        match event.raw_event() {
            Event::MouseDown {
                button: MouseButton::Left,
                position,
                click_count: 1,
                ..
            } if self.takes(*position, ctx) => self.drag.press(self.grip, *position),

            // Not hit-tested, for the reason a divider's moves are not: the
            // pointer has left the row by the first pixel of the gesture, and
            // what keeps every other row out of this is that the gesture names
            // the box it began on.
            Event::MouseDragged {
                button: MouseButton::Left,
                position,
                ..
            } if self.drag.holds(self.grip) => {
                if self.drag.moved_to(*position) {
                    ctx.notify();
                }
                return true;
            }

            Event::MouseUp {
                button: MouseButton::Left,
                ..
            } if self.drag.holds(self.grip) => {
                let (dragged, action) = self.drag.release();
                if let Some(action) = action {
                    ctx.dispatch_typed_action(WorkspaceAction::Tab(action));
                }
                if dragged {
                    // A gesture that travelled is not also a click. Letting
                    // this reach the child would fold a group away every time
                    // its heading was dragged, and select whichever row a tab
                    // was dropped on — so the press is taken back off the
                    // child instead, because the release it was waiting for is
                    // not coming.
                    self.pressed.lock().reset_interaction_state();
                    ctx.notify();
                    return true;
                }
                // Otherwise on to the child: a press that never travelled is a
                // click, and the row is what answers one.
            }

            _ => {}
        }

        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

#[cfg(test)]
mod tests {
    use crookui_core::geometry::vec2f;

    use super::*;

    /// Three rows of 20px: an ungrouped tab, then a group of two.
    fn panel() -> (Vec<Slot>, Vec<TabId>, GroupId) {
        let tabs: Vec<TabId> = (0..3).map(|_| TabId::next()).collect();
        let group = GroupId::next();
        let slots = vec![
            Slot {
                grip: Grip::Tab {
                    tab: tabs[0],
                    group: None,
                },
                top: 0.,
                bottom: 20.,
            },
            Slot {
                grip: Grip::Heading {
                    group,
                    collapsed: false,
                },
                top: 20.,
                bottom: 40.,
            },
            Slot {
                grip: Grip::Tab {
                    tab: tabs[1],
                    group: Some(group),
                },
                top: 40.,
                bottom: 60.,
            },
            Slot {
                grip: Grip::Tab {
                    tab: tabs[2],
                    group: Some(group),
                },
                top: 60.,
                bottom: 80.,
            },
        ];
        (slots, tabs, group)
    }

    #[test]
    fn a_tab_dropped_on_the_upper_half_of_a_row_lands_above_it() {
        let (slots, tabs, group) = panel();

        assert_eq!(
            drop_action(&slots, Carried::Tab(tabs[0]), 45.),
            Some(TabAction::MoveTab {
                tab: tabs[0],
                group: Some(group),
                before: Some(tabs[1])
            })
        );
    }

    #[test]
    fn a_tab_dropped_on_the_lower_half_of_a_row_lands_under_it() {
        let (slots, tabs, group) = panel();

        assert_eq!(
            drop_action(&slots, Carried::Tab(tabs[0]), 55.),
            Some(TabAction::MoveTab {
                tab: tabs[0],
                group: Some(group),
                before: Some(tabs[2])
            })
        );
    }

    #[test]
    fn a_tab_dropped_under_the_last_row_lands_at_the_end_of_its_group() {
        // The gap under the last member is inside the group, not past it: the
        // strip clamps `before: None` to the end of the group it names.
        let (slots, tabs, group) = panel();

        assert_eq!(
            drop_action(&slots, Carried::Tab(tabs[0]), 75.),
            Some(TabAction::MoveTab {
                tab: tabs[0],
                group: Some(group),
                before: None
            })
        );
    }

    #[test]
    fn a_tab_dropped_below_everything_lands_at_the_end_of_the_list() {
        let (slots, tabs, group) = panel();

        assert_eq!(
            drop_action(&slots, Carried::Tab(tabs[1]), 400.),
            Some(TabAction::MoveTab {
                tab: tabs[1],
                group: Some(group),
                before: None
            }),
            "under the last member is still inside the group it is over"
        );
    }

    /// The panel of [`panel`], with a loose row under the group as well: the
    /// arrangement where the gap below the group's last member is also the gap
    /// above something else.
    fn panel_with_a_row_under_the_group() -> (Vec<Slot>, Vec<TabId>, GroupId) {
        let (mut slots, mut tabs, group) = panel();
        let under = TabId::next();
        tabs.push(under);
        slots.push(Slot {
            grip: Grip::Tab {
                tab: under,
                group: None,
            },
            top: 80.,
            bottom: 100.,
        });
        (slots, tabs, group)
    }

    #[test]
    fn a_drop_under_a_group_s_last_member_is_the_group_s_end() {
        // The row below that gap belongs to the next block, and a drop that
        // named it would be asking to join a group in front of a tab the group
        // does not contain: the strip clamps it back into the group, and the
        // panel has no line to draw for it in the meantime. The gap has one
        // meaning, so it is given the name of that meaning.
        let (slots, tabs, group) = panel_with_a_row_under_the_group();

        assert_eq!(
            drop_action(&slots, Carried::Tab(tabs[0]), 75.),
            Some(TabAction::MoveTab {
                tab: tabs[0],
                group: Some(group),
                before: None
            })
        );
    }

    #[test]
    fn a_group_dropped_inside_another_block_lands_above_that_block() {
        // A block goes between blocks, never between two of another block's
        // members — so the gap under a group's first member names the group,
        // not the member under it.
        let (slots, tabs, group) = panel_with_a_row_under_the_group();
        let moving = GroupId::next();

        assert_eq!(
            drop_action(&slots, Carried::Group(moving), 55.),
            Some(TabAction::MoveGroup {
                group: moving,
                before: Some(tabs[1])
            }),
            "the block starts at its first member, whichever member was named"
        );
        assert_eq!(
            drop_action(&slots, Carried::Group(group), 55.),
            None,
            "and its own block is where it already is"
        );
    }

    #[test]
    fn the_top_of_a_heading_is_above_the_whole_group() {
        // The one gap that has to be reachable, and the one every list gets
        // wrong: without it there is no way to put a tab between the row above
        // a group and the group itself.
        let (slots, tabs, _) = panel();

        assert_eq!(
            drop_action(&slots, Carried::Tab(tabs[2]), 25.),
            Some(TabAction::MoveTab {
                tab: tabs[2],
                group: None,
                before: Some(tabs[1])
            })
        );
    }

    #[test]
    fn the_bottom_of_a_heading_is_the_top_of_the_group() {
        let (slots, tabs, group) = panel();

        assert_eq!(
            drop_action(&slots, Carried::Tab(tabs[0]), 35.),
            Some(TabAction::MoveTab {
                tab: tabs[0],
                group: Some(group),
                before: Some(tabs[1])
            })
        );
    }

    #[test]
    fn a_folded_group_takes_a_drop_at_its_end() {
        // Its members are hidden, so there is nothing to aim between: the
        // whole lower half of the heading means "into it".
        let (mut slots, tabs, group) = panel();
        slots.truncate(2);
        slots[1].grip = Grip::Heading {
            group,
            collapsed: true,
        };

        assert_eq!(
            drop_action(&slots, Carried::Tab(tabs[0]), 35.),
            Some(TabAction::MoveTab {
                tab: tabs[0],
                group: Some(group),
                before: None
            })
        );
    }

    #[test]
    fn a_tab_dropped_where_it_already_is_asks_for_nothing() {
        let (slots, tabs, _) = panel();

        assert_eq!(drop_action(&slots, Carried::Tab(tabs[0]), 5.), None);
        assert_eq!(drop_action(&slots, Carried::Tab(tabs[0]), 15.), None);
        assert_eq!(
            drop_action(&slots, Carried::Tab(tabs[2]), 75.),
            None,
            "the last member, dropped under itself"
        );
    }

    #[test]
    fn a_group_dropped_on_its_own_block_asks_for_nothing() {
        let (slots, _, group) = panel();

        for y in [25., 35., 45., 55., 65., 75.] {
            assert_eq!(
                drop_action(&slots, Carried::Group(group), y),
                None,
                "at {y}"
            );
        }
    }

    #[test]
    fn a_group_dragged_over_a_row_lands_on_that_row_s_side_of_it() {
        let (slots, tabs, group) = panel();

        assert_eq!(
            drop_action(&slots, Carried::Group(group), 5.),
            Some(TabAction::MoveGroup {
                group,
                before: Some(tabs[0])
            }),
            "above the row it is over"
        );
        assert_eq!(
            drop_action(&slots, Carried::Group(group), 15.),
            None,
            "and under it, which is where the group already begins"
        );
    }

    #[test]
    fn nothing_is_dropped_on_an_empty_panel() {
        assert_eq!(drop_action(&[], Carried::Group(GroupId::next()), 10.), None);
    }

    #[test]
    fn a_press_that_does_not_travel_is_not_a_drag() {
        let drag = PanelDrag::new();
        let grip = Grip::Tab {
            tab: TabId::next(),
            group: None,
        };

        drag.press(grip, vec2f(0., 100.));
        assert!(
            !drag.moved_to(vec2f(0., 102.)),
            "two pixels is a click that wobbled"
        );
        assert_eq!(drag.carrying(), None);

        assert!(drag.moved_to(vec2f(0., 110.)));
        assert_eq!(drag.carrying(), Some(grip.carries()));

        // And once it is a drag it stays one, however far back the pointer
        // comes: a gesture that un-dragged itself would fire the click it was
        // never going to be.
        assert!(drag.moved_to(vec2f(0., 100.)));
        assert_eq!(drag.release(), (true, None));
    }

    #[test]
    fn only_the_row_that_was_pressed_is_being_dragged() {
        let drag = PanelDrag::new();
        let (pressed, other) = (
            Grip::Tab {
                tab: TabId::next(),
                group: None,
            },
            Grip::Tab {
                tab: TabId::next(),
                group: None,
            },
        );

        drag.press(pressed, Vector2F::default());

        assert!(drag.holds(pressed));
        assert!(!drag.holds(other));
    }
}
