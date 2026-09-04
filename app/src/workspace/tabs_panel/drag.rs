//! Picking a row of the panel up and carrying it.
//!
//! Warp's gesture, and it is not the one a list usually gets. There is no line
//! drawn in a gap and no drop decided at the end. Warp's
//! `Draggable::paint` stops painting the row where the layout put it and
//! paints it on an overlay layer at `mouse_position + mouse_offset` — the row
//! leaves the list and follows the pointer, above everything and outside every
//! clip — and `Workspace::on_tab_drag` reorders the strip *while the hand is
//! still moving*, one step per event. `DropTab` then resolves nothing at all,
//! because by the time the button comes up everything the gesture meant has
//! already happened.
//!
//! # Why a line was the wrong promise
//!
//! A line in a gap says where a row *would* land. That is a promise about the
//! future, and it costs a list two things a person can feel. The rows have to
//! part to make room for it, so the list moves under the hand — and moves the
//! very boundary the hand is aiming at, which is why the old code had to
//! answer the pointer against a layout with the line taken back out of it. And
//! the row being carried stays where it was, so the one thing on screen that
//! ought to be under the pointer is the one thing that never moves.
//!
//! Carrying the row instead says the same thing in the present tense: the list
//! *is* in the order it will be in, the row *is* where it is going, and the
//! hole left in the layout is exactly the slot it will drop into. Nothing has
//! to be promised, so nothing can be promised wrongly.
//!
//! # Which group the row is in is a band, not a row
//!
//! The question a grouped list gets wrong is not "between which two rows" —
//! it is "in which group", and answering it with *the row under the pointer*
//! is what makes a group impossible to leave. Under a group's last member is
//! still that member's row; under that is the next block's first row, which is
//! somebody else's group; and a group at the end of the list has nothing under
//! it at all. There is no pointer position that means "out of this group and
//! into none", which is precisely the bug this rewrite is here to fix.
//!
//! Warp asks the group's own box instead. `target_group_at_axis` takes the
//! carried row's middle and tests it against the whole block's rectangle,
//! inset by [`LEADING_EDGE_MARGIN`] at the top and [`TRAILING_EDGE_MARGIN`] at
//! the bottom: inside the band the row belongs to that group, outside it the
//! row belongs to none. The band is a fact about the block, so it exists for
//! every group whatever is drawn above or below it — and the margins, being
//! smaller than a row, cannot be reached without leaving the group's rows.
//!
//! # Why the geometry is written down rather than asked for
//!
//! An element knows its own box and nothing else's. "Which block is that
//! height inside" is a question about all of them, so every row records the box
//! it painted into one shared list and every group records its block's, in
//! paint order, and the grip that owns the gesture reads them when the pointer
//! moves. It is the trick [`RowGeometry`](super::geometry::RowGeometry)
//! already uses for the scroll target and
//! [`PaneExtent`](crate::pane_split::PaneExtent) for a divider, for the same
//! reason: the number is produced by a paint pass and wanted by an event three
//! frames later.
//!
//! Window coordinates, not content ones, and the boxes are the ones the layout
//! chose — a row that is being carried records the slot it came from, never
//! the place it is being painted. The hole is what the next step is measured
//! against. That is a promise about *these* boxes only: the row's own subtree
//! is painted at the pointer and anything inside it that writes down where it
//! was drawn — [`geometry::Tracked`](super::geometry::Tracked), which is how a
//! selection is scrolled to — writes down the place in the air. Nothing reads
//! that while a row is being carried, and
//! [`Workspace::scroll_row_into_view`](crate::workspace::view::Workspace) says
//! so out loud: a list that scrolled to a row travelling with the hand would
//! chase it off its own end.
//!
//! # Why the step is decided by a pure function
//!
//! [`advance`] is the whole gesture's judgement — which neighbour has been
//! passed, which group's band the row is in, and whether either is a change —
//! and it takes two lists of boxes and two numbers. So the cases that are
//! actually hard, and that every list gets wrong at least once (leaving a group
//! at the end of the list, hopping a folded block, a member that is its group's
//! first row), are tested here in microseconds with no window and no pointer.
//!
//! What it cannot get wrong is the invariant:
//! [`TabStrip::apply`](crate::tab::TabStrip::apply) clamps the group and the
//! neighbour it names against each other, so a step computed from a coarse
//! gesture can be wrong about *where* and still cannot produce a strip the
//! panel is unable to draw.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::elements::MouseStateHandle;
use crookui_core::event::{DispatchedEvent, Event, MouseButton};
use crookui_core::geometry::{Point, Vector2F, ZIndex, vec2f};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};
use crookui_core::scene::ClipBounds;

use crate::tab::{GroupId, TabAction, TabId};

use super::super::action::WorkspaceAction;

/// How far the pointer has to travel before a press becomes a drag.
///
/// Warp's `DEFAULT_DRAG_THRESHOLD`. Below it the gesture is still a click, and
/// a click on a row selects it. A list whose rows moved on the first stray
/// pixel would be a list nobody could click; one that waited too long would
/// feel stuck.
///
/// Measured on the vertical, where Warp measures the straight line — and the
/// difference is the one Warp feature Crook does not have. There, sideways is
/// a direction a tab can go: far enough and it tears off into a window of its
/// own, so the threshold has to notice it. Here the row is locked to the
/// column, so a press that travelled six pixels sideways and none downwards
/// has asked for nothing that can be drawn: the row would lift, land back in
/// the same slot, and swallow the click that was actually made. A gesture is
/// measured on the axis it is allowed to move on.
const THRESHOLD: f32 = 5.;

/// How far inside a group's top edge the band that means "in this group"
/// begins, and how far short of its bottom edge it ends.
///
/// Warp's `LEADING_EDGE_MARGIN` and `TRAILING_EDGE_MARGIN`, from
/// `target_group_at_axis`, and taken off the same thing: the box the block
/// painted into, heading and padding and all. They are not symmetric because
/// the block is not — four is a slice off the heading, eight is the strip of
/// empty column the block keeps under its last member.
///
/// What matters about both is that they are smaller than the padding they cut
/// into, so neither can be reached while the carried row's middle is still
/// over one of the group's own rows. In
/// [`Panes`](crate::settings::Granularity::Panes) that padding is paid twice —
/// a member's own box already ends
/// [`GROUP_BODY_BOTTOM_PADDING`](super::GROUP_BODY_BOTTOM_PADDING) below its
/// last row, and the block adds as much again — so the line a row leaves a
/// group at sits eight pixels below the last row rather than level with it.
/// Warp's block has one of those paddings and its line is level; a quarter of
/// a row of extra travel is a dead zone, and a dead zone on the way out of a
/// group is not the wrong thing to have.
const LEADING_EDGE_MARGIN: f32 = 4.;
/// See [`LEADING_EDGE_MARGIN`].
const TRAILING_EDGE_MARGIN: f32 = 8.;

/// What a press on one of the panel's boxes picks up.
///
/// It is also what that box records about itself every frame, which is why it
/// carries more than an identity: a row knows the group it is in, and a block
/// knows the row it begins with and whether its members are folded away.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Grip {
    /// A tab's row, and the group it is currently in.
    Tab {
        /// The tab the row stands for.
        tab: TabId,
        /// The group it is in, if any.
        group: Option<GroupId>,
    },
    /// A whole group's block — its heading and, unless they are folded away,
    /// its members — which moves as one thing.
    Group {
        /// The group the block draws.
        group: GroupId,
        /// The row it begins with, which is the row anything landing in front
        /// of the block lands in front of.
        first: TabId,
        /// Whether its members are folded away. A folded group is one row on
        /// screen and cannot be dropped into, so it is hopped over instead.
        collapsed: bool,
    },
}

impl Grip {
    /// What a drag started on this box is carrying.
    ///
    /// The identity the gesture is held by, and deliberately narrower than the
    /// grip: a row's group changes *while it is being carried*, and a gesture
    /// held by anything that can change under it is a gesture that lets go by
    /// itself halfway across the panel.
    fn carries(self) -> Carried {
        match self {
            Self::Tab { tab, .. } => Carried::Tab(tab),
            Self::Group { group, .. } => Carried::Group(group),
        }
    }
}

/// What a drag in progress is carrying.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Carried {
    /// One tab, which can join a group, leave one, or pass its neighbours.
    Tab(TabId),
    /// A whole group, which only ever passes whole blocks.
    Group(GroupId),
}

/// One row the panel drew, as a carried row meets it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Slot {
    /// The tab it stands for.
    pub(crate) tab: TabId,
    /// The group it is in, if any.
    pub(crate) group: Option<GroupId>,
    /// Its top edge, in window coordinates.
    pub(crate) top: f32,
    /// Its bottom edge.
    pub(crate) bottom: f32,
}

/// One group's block: everything the group draws, taken together.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Extent {
    /// The group the block draws.
    pub(crate) group: GroupId,
    /// The row it begins with.
    pub(crate) first: TabId,
    /// Whether its members are folded away.
    pub(crate) collapsed: bool,
    /// The block's top edge, in window coordinates.
    pub(crate) top: f32,
    /// Its bottom edge, past the padding under the last member.
    pub(crate) bottom: f32,
}

impl Extent {
    /// Whether a carried row's middle at `height` is inside this group.
    ///
    /// A folded group is never inside anything: its members are not drawn, so
    /// a row dropped into it would vanish behind a chevron.
    fn holds(&self, height: f32) -> bool {
        !self.collapsed
            && height >= self.top + LEADING_EDGE_MARGIN
            && height <= self.bottom - TRAILING_EDGE_MARGIN
    }
}

/// The gesture in progress: what it carries, where it began, and whether it
/// has travelled far enough to be a drag at all.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Gesture {
    carried: Carried,
    /// Where the press landed, so that [`THRESHOLD`] is measured from it.
    from: Vector2F,
    /// The box's own origin minus that press. Adding it to the pointer is what
    /// keeps the row under the part of itself that was picked up, instead of
    /// snapping its corner to the pointer.
    offset: Vector2F,
    /// How tall the box is, so that its middle can be answered for while it is
    /// out of the layout.
    height: f32,
    /// Where the pointer is now, in the window.
    at: Vector2F,
    /// Whether it has passed the threshold. Until it does, this is a click
    /// that has not been let go of yet and nothing has moved.
    dragging: bool,
}

impl Gesture {
    /// Where the carried box is now: its top edge, and its bottom.
    fn box_at(&self) -> Range<f32> {
        let top = self.at.y() + self.offset.y();
        top..top + self.height
    }
}

/// Everything the panel's drags share: the boxes of the last frame, and the
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
    extents: Vec<Extent>,
    gesture: Option<Gesture>,
}

impl PanelDrag {
    /// A panel with nothing being carried and nothing drawn yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Starts a frame: forgets the boxes the last one drew.
    ///
    /// Cleared rather than overwritten, because a box that is no longer drawn
    /// is a row that has closed — and one left behind would be a place the
    /// carried row could still be answered against.
    fn begin(&self) {
        let mut inner = self.0.borrow_mut();
        inner.slots.clear();
        inner.extents.clear();
    }

    /// Records one row, in paint order.
    ///
    /// Except a row of a block that is in the air: it was painted at the
    /// pointer along with the rest of its block, so the box it would record is
    /// not a place in the list at all. Its own block's [`Extent`] is recorded
    /// where the layout put it, which is what the gesture is measured against.
    fn record(&self, slot: Slot) {
        let mut inner = self.0.borrow_mut();
        let lifted = inner
            .gesture
            .filter(|gesture| gesture.dragging)
            .and_then(|gesture| match gesture.carried {
                Carried::Group(group) => Some(group),
                Carried::Tab(_) => None,
            });
        if lifted.is_some() && lifted == slot.group {
            return;
        }
        inner.slots.push(slot);
    }

    /// Records one group's block.
    fn record_extent(&self, extent: Extent) {
        self.0.borrow_mut().extents.push(extent);
    }

    /// Takes note of a press, which is not yet a drag.
    fn press(&self, carried: Carried, at: Vector2F, origin: Vector2F, height: f32) {
        self.0.borrow_mut().gesture = Some(Gesture {
            carried,
            from: at,
            offset: origin - at,
            height,
            at,
            dragging: false,
        });
    }

    /// Whether the gesture in progress began on this box.
    fn holds(&self, carried: Carried) -> bool {
        self.0
            .borrow()
            .gesture
            .is_some_and(|gesture| gesture.carried == carried)
    }

    /// What is being carried, once the gesture is one.
    pub(crate) fn carrying(&self) -> Option<Carried> {
        let inner = self.0.borrow();
        let gesture = inner.gesture.filter(|gesture| gesture.dragging)?;
        Some(gesture.carried)
    }

    /// Where the top edge of `carried` is right now, if it is the thing being
    /// carried.
    ///
    /// The one number [`Handle`] needs to paint a row somewhere other than
    /// where the layout put it.
    fn lifted(&self, carried: Carried) -> Option<f32> {
        let inner = self.0.borrow();
        let gesture = inner
            .gesture
            .filter(|gesture| gesture.dragging && gesture.carried == carried)?;
        // Whole pixels, unlike the number the steps are decided from: the
        // glyph cache keys on a glyph's subpixel offset, so a row carried at
        // 41.3 and then at 41.7 rasterises its whole text twice on the way. A
        // gesture is a few hundred of those.
        Some(gesture.box_at().start.round())
    }

    /// Moves the gesture to `at`, answering with whether the panel has to be
    /// drawn again and with the one step the strip should take.
    ///
    /// A press that has not passed the threshold moves nothing on screen, so
    /// it asks for no frame: that is what keeps a click that wobbles by a
    /// pixel off the GPU.
    fn moved_to(&self, at: Vector2F) -> (bool, Option<TabAction>) {
        let mut inner = self.0.borrow_mut();
        let Some(mut gesture) = inner.gesture else {
            return (false, None);
        };
        gesture.at = at;
        gesture.dragging = gesture.dragging || (at.y() - gesture.from.y()).abs() > THRESHOLD;
        inner.gesture = Some(gesture);
        if !gesture.dragging {
            return (false, None);
        }
        let step = advance(
            &inner.slots,
            &inner.extents,
            gesture.carried,
            gesture.box_at(),
            at.y(),
        );
        (true, step)
    }

    /// Ends the gesture, answering with whether it was a drag at all.
    ///
    /// There is nothing else to answer with: a carried row has been landing
    /// where it is all the way across the panel, so letting go of it performs
    /// no move. What the caller still needs to know is that this release is
    /// not also a click.
    fn release(&self) -> bool {
        let dragged = self.carrying().is_some();
        self.0.borrow_mut().gesture = None;
        dragged
    }

    /// Ends the gesture without doing anything with it.
    pub(crate) fn cancel(&self) {
        self.0.borrow_mut().gesture = None;
    }

    /// The box the `index`th row of the list was painted into, and the box the
    /// `index`th group's block was, on the last frame.
    ///
    /// A drag is a state that exists only while a button is held down, which
    /// makes it the third thing an unattended run cannot hold still — after a
    /// hovered row and a selection dragged across a shell's output. `--carry`
    /// holds it by pressing on a row and never letting go, and to press on a
    /// row it has to know where one is. It finds out the way the gesture
    /// itself does: from the boxes the last frame wrote down.
    pub(crate) fn row(&self, index: usize) -> Option<Range<f32>> {
        let inner = self.0.borrow();
        let slot = inner.slots.get(index)?;
        Some(slot.top..slot.bottom)
    }

    /// See [`Self::row`].
    pub(crate) fn block(&self, index: usize) -> Option<Range<f32>> {
        let inner = self.0.borrow();
        let extent = inner.extents.get(index)?;
        Some(extent.top..extent.bottom)
    }
}

/// One thing the list drew that a carried row can be measured against: one of
/// the carried row's own siblings, a row that is in no group, or a whole block
/// the carried row is not part of.
///
/// A block the row does not belong to is **one** step, however many rows it
/// draws. Passing it means passing all of them, and it has to: the strip
/// refuses to put an outsider between two of a group's members, so a step that
/// named one of them would be answered by the clamp with "the nearer end of
/// the run" — which can be the end the row came from, and then the row is
/// wedged there for as long as the pointer stays where it is. It is also the
/// only reading that makes sense of a folded block, whose members are not on
/// screen to be passed one at a time. Warp says the same thing twice, in
/// `neighbor_drag_rect` and `group_swap_threshold_rect`.
///
/// The ordinary way into a group is not this at all — it is the band, which
/// the pointer crosses long before it reaches any member's middle. This is
/// what answers a hand that moved a whole block's worth of pixels between two
/// events.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Step {
    /// The row anything landing in front of this step lands in front of.
    head: TabId,
    top: f32,
    bottom: f32,
}

impl Step {
    fn middle(&self) -> f32 {
        (self.top + self.bottom) / 2.
    }
}

/// One block of the list: a group with everything it draws, or a tab that is
/// in no group.
///
/// What a carried *group* is measured against, for the reason a group only
/// ever lands between blocks.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Block {
    head: TabId,
    group: Option<GroupId>,
    top: f32,
    bottom: f32,
}

impl Block {
    fn middle(&self) -> f32 {
        (self.top + self.bottom) / 2.
    }
}

/// The list as a row carried out of `own` meets it, in order.
fn steps(slots: &[Slot], extents: &[Extent], own: Option<GroupId>) -> Vec<Step> {
    let mut steps: Vec<Step> = slots
        .iter()
        .filter(|slot| slot.group.is_none() || slot.group == own)
        .map(|slot| Step {
            head: slot.tab,
            top: slot.top,
            bottom: slot.bottom,
        })
        .chain(
            extents
                .iter()
                .filter(|extent| Some(extent.group) != own)
                .map(|extent| Step {
                    head: extent.first,
                    top: extent.top,
                    bottom: extent.bottom,
                }),
        )
        .collect();
    steps.sort_by(|one, other| one.top.total_cmp(&other.top));
    steps
}

/// The blocks of the list, in order.
fn blocks(slots: &[Slot], extents: &[Extent]) -> Vec<Block> {
    let mut blocks: Vec<Block> = extents
        .iter()
        .map(|extent| Block {
            head: extent.first,
            group: Some(extent.group),
            top: extent.top,
            bottom: extent.bottom,
        })
        .chain(
            slots
                .iter()
                .filter(|slot| slot.group.is_none())
                .map(|slot| Block {
                    head: slot.tab,
                    group: None,
                    top: slot.top,
                    bottom: slot.bottom,
                }),
        )
        .collect();
    blocks.sort_by(|one, other| one.top.total_cmp(&other.top));
    blocks
}

/// The one step the strip should take, for a box carried to `box_at` with the
/// pointer at `pointer`.
///
/// `None` when the arrangement on screen is already the one the hand is
/// asking for, which is what it answers on almost every frame of a drag: a
/// gesture that travels the height of the panel is a few dozen of these and
/// four or five steps.
///
/// One move, never two — but the one move goes as far as the hand has. A
/// change of group and a change of place are two different questions about the
/// same pointer, and answering both from one frame's boxes would decide the
/// second against a list the first has already moved; the group is answered
/// first and the frame ends there, which is what Warp's `on_tab_drag` does
/// when it returns after a regrouping. Within one question, though, the answer
/// is the whole distance: several pointer positions are answered against one
/// painted frame, so a move worth one place would be a move worth one place
/// per *frame*.
pub(crate) fn advance(
    slots: &[Slot],
    extents: &[Extent],
    carried: Carried,
    box_at: Range<f32>,
    pointer: f32,
) -> Option<TabAction> {
    match carried {
        Carried::Tab(tab) => tab_step(slots, extents, tab, (box_at.start + box_at.end) / 2.),
        Carried::Group(group) => group_step(
            slots,
            extents,
            group,
            (box_at.start + box_at.end) / 2.,
            pointer,
        ),
    }
}

/// Where a carried tab whose middle is at `height` goes next: into a group,
/// out of one, or past the neighbour it has reached.
///
/// The group comes first and the neighbour second, which is Warp's order and
/// is the one that keeps the two from fighting: crossing a group's edge is
/// also crossing the row nearest that edge, and answering it as a reorder
/// would leave the row in a group whose band it has left.
fn tab_step(slots: &[Slot], extents: &[Extent], tab: TabId, height: f32) -> Option<TabAction> {
    let row = slots.iter().find(|slot| slot.tab == tab)?;
    let held = row.group;
    let target = extents
        .iter()
        .find(|extent| extent.holds(height))
        .map(|extent| extent.group);

    if target != held {
        return regroup(slots, extents, tab, row, held, target, height);
    }

    // Against every neighbour's middle, and all the way: the row lands in
    // front of the first one the hand has not passed. Middles rather than
    // edges is what makes the answer reversible by the same movement that
    // caused it — a row that has just moved above another sits above that
    // row's middle, and has to be carried back past it to come down again.
    //
    // *All the way* is not Warp's `calculate_updated_tab_index_vertical`,
    // which moves one place per answer, and the difference is not cosmetic.
    // These boxes are the ones the last frame painted, and a hand moving
    // faster than the display reports several positions against the same
    // frame; every one of them names the same neighbour, and naming a row by
    // identity makes the second answer a no-op. One place per *frame* is what
    // that comes to — flick a row down a list of forty and it arrives three
    // rows behind the hand and is dropped there. (Warp's own swap walks
    // further than one place per frame by accident: it looks its neighbours up
    // by *index*, so after a swap the same index names a different row.)
    let steps = steps(slots, extents, held);
    let index = steps.iter().position(|step| step.head == tab)?;
    let before = steps
        .iter()
        .filter(|step| step.head != tab)
        .find(|step| height < step.middle())
        .map(|step| step.head);

    // Where it already is: the row it would land in front of is the row that
    // already follows it, and the end of the list is where a row with nothing
    // after it already is.
    if before == steps.get(index + 1).map(|step| step.head) {
        return None;
    }
    Some(TabAction::MoveTab {
        tab,
        group: held,
        before,
    })
}

/// The move that takes `tab` from the group it is in to the one its middle is
/// now inside — or out of every group, when its middle is inside none.
///
/// The row it names is always one the strip can honour without splitting a
/// group in two: joining names the group's own first row or its end, and
/// leaving names the row above the whole block or the row below it. A member
/// pulled out of the middle of a group therefore comes out at the group's near
/// edge rather than staying between two of its neighbours, which is the one
/// arrangement the panel could not draw.
fn regroup(
    slots: &[Slot],
    extents: &[Extent],
    tab: TabId,
    row: &Slot,
    held: Option<GroupId>,
    target: Option<GroupId>,
    height: f32,
) -> Option<TabAction> {
    let Some(joining) = target else {
        let left = held?;
        let extent = extents.iter().find(|extent| extent.group == left)?;
        let below = steps(slots, extents, held)
            .into_iter()
            .find(|step| step.top >= extent.bottom)
            .map(|step| step.head);
        // Out of the top of the band, or out of the bottom of it. Landing in
        // front of a member that stays behind is landing in front of the whole
        // group once the strip has clamped it, which is what puts the row
        // above the heading rather than between two members.
        let before = if height < extent.top + LEADING_EDGE_MARGIN {
            slots
                .iter()
                .find(|slot| slot.group == Some(left) && slot.tab != tab)
                .map(|slot| slot.tab)
                .or(below)
        } else {
            below
        };
        return Some(TabAction::MoveTab {
            tab,
            group: None,
            before,
        });
    };

    let extent = extents.iter().find(|extent| extent.group == joining)?;
    // Entering from above lands at the group's first row and from below at its
    // last, so that a row joining a group appears at the edge it arrived at
    // rather than jumping the whole block. Warp's `hop_tab_to_index`, which
    // picks `first - 1` or `last + 1` for the same reason.
    let before = (row.top < extent.top).then_some(extent.first);
    Some(TabAction::MoveTab {
        tab,
        group: Some(joining),
        before,
    })
}

/// Where a carried group goes next: past the block above it, or past the one
/// below.
///
/// Two anchors rather than one, which is Warp's `on_group_drag`: a block is
/// several rows tall, and measuring both ends of the gesture from its middle
/// makes an expanded group refuse to move up until half of it is already past
/// the block above. The pointer leads going up — it is the end of the block a
/// person is looking at — and the block's own middle leads going down.
fn group_step(
    slots: &[Slot],
    extents: &[Extent],
    group: GroupId,
    middle: f32,
    pointer: f32,
) -> Option<TabAction> {
    let blocks = blocks(slots, extents);
    let index = blocks.iter().position(|block| block.group == Some(group))?;
    // A folded block is one row tall, so the pointer and the middle are the
    // same place and the asymmetry has nothing to say. Warp spells this out
    // rather than letting the numbers coincide, and so does this.
    let collapsed = extents
        .iter()
        .any(|extent| extent.group == group && extent.collapsed);
    let leading = if collapsed { middle } else { pointer };

    // All the way, for the reason a row goes all the way: several pointer
    // positions are answered against one painted frame, and an answer that
    // moved the block one place would move it one place per frame.
    if let Some(above) = blocks[..index]
        .iter()
        .position(|block| leading < block.middle())
    {
        return Some(TabAction::MoveGroup {
            group,
            before: Some(blocks[above].head),
        });
    }
    if let Some(below) = blocks[index + 1..]
        .iter()
        .rposition(|block| middle > block.middle())
    {
        let below = index + 1 + below;
        return Some(TabAction::MoveGroup {
            group,
            before: blocks.get(below + 1).map(|after| after.head),
        });
    }
    None
}

/// Wraps the list, clearing the boxes the last frame recorded.
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
    /// Clears the boxes and draws `child`.
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
        self.drag.begin();
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

/// Wraps one row or one group's block: records where it was drawn, turns a
/// press that travels into a drag, and paints what is being carried under the
/// pointer instead of where the layout put it.
///
/// **It never takes the press itself.** A press is half a click and half a
/// drag, and only the pointer's next move says which — so this notes where the
/// press landed and hands the event on to the row underneath, whose
/// [`Hoverable`](crookui_core::elements::Hoverable) still claims it and still
/// fires on release. What the drag takes is the *moves*, which nothing else in
/// the panel wants.
///
/// A block's handle wraps its member rows' handles, and both note the press.
/// The inner one wins because it notes it second, which is what lets a member
/// be dragged out of a group whose heading is draggable too — Warp says the
/// same thing with `with_defer_to_handled_child_mouse_down`.
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
    /// own, or the block's — and it is here for one case: a press that becomes
    /// a drag never reaches that element's release, so the press it is still
    /// holding has to be taken away by hand. It is the same call
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
        // Where the layout put it, even when that is not where it is being
        // drawn: the hole a carried row leaves is the slot it will drop into,
        // and every step of the gesture is measured against it.
        match self.grip {
            Grip::Tab { tab, group } => self.drag.record(Slot {
                tab,
                group,
                top: origin.y(),
                bottom: origin.y() + height,
            }),
            Grip::Group {
                group,
                first,
                collapsed,
            } => self.drag.record_extent(Extent {
                group,
                first,
                collapsed,
                top: origin.y(),
                bottom: origin.y() + height,
            }),
        }

        match self.drag.lifted(self.grip.carries()) {
            // Warp's `Draggable::paint`: on an overlay layer, so the row is
            // above every other row and outside the list's clip, and at the
            // pointer's height with the layout's own x — the column is one
            // tab wide and a row that could be carried out of it would be
            // promising a place there is nowhere to put.
            Some(top) => {
                ctx.scene.start_overlay_layer(ClipBounds::None);
                // A drag image is not a thing to click on. Every layer above a
                // point is asked whether it covers it, and a row painted at
                // the pointer covers the pointer by construction — so without
                // this the list under it stops answering the wheel for the
                // whole gesture, and so does everything else the hand passes
                // over. Warp keeps its hit rect and pays for it elsewhere;
                // this is the same claim said once.
                ctx.scene.set_active_layer_click_through();
                self.child.paint(vec2f(origin.x(), top), ctx, app);
                ctx.scene.stop_layer();
            }
            None => self.child.paint(origin, ctx, app),
        }
        self.child_max_z_index = Some(ctx.scene.max_active_z_index());
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let carried = self.grip.carries();
        match event.raw_event() {
            Event::MouseDown {
                button: MouseButton::Left,
                position,
                click_count: 1,
                ..
            } if self.takes(*position, ctx) => {
                let (Some(origin), Some(size)) = (self.origin, self.size) else {
                    return self.child.dispatch_event(event, ctx, app);
                };
                self.drag.press(carried, *position, origin.xy(), size.y());
            }

            // Not hit-tested, for the reason a divider's moves are not: the
            // pointer has left the row by the first pixel of the gesture, and
            // what keeps every other row out of this is that the gesture names
            // the box it began on.
            Event::MouseDragged {
                button: MouseButton::Left,
                position,
                ..
            } if self.drag.holds(carried) => {
                let (redraw, step) = self.drag.moved_to(*position);
                if let Some(step) = step {
                    ctx.dispatch_typed_action(WorkspaceAction::Tab(step));
                }
                if redraw {
                    ctx.notify();
                }
                return true;
            }

            Event::MouseUp {
                button: MouseButton::Left,
                ..
            } if self.drag.holds(carried) => {
                if self.drag.release() {
                    // A gesture that travelled is not also a click. Letting
                    // this reach the child would fold a group away every time
                    // its heading was dragged, and select whichever row a tab
                    // was carried over — so the press is taken back off the
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

        // A row being carried is not a row being pointed at. Warp's
        // `Draggable` stops calling its child for the duration of a drag, and
        // it has to: the row is under the pointer for the whole gesture, so
        // every hover, every press and every release it would otherwise
        // collect belongs to whatever the row is being carried *over*.
        //
        // Unhandled rather than handled, which is Warp's answer too: the
        // events are not this row's to take, and claiming them would stop the
        // panel's own scroll from ever seeing a wheel turned over a row that
        // happens to be in the air.
        if self.drag.lifted(carried).is_some() {
            return false;
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
    use super::*;

    /// The rows of a panel holding a loose tab, then a group of two, then
    /// another loose tab. Twenty pixels a row, and the group's block runs from
    /// its heading's top to eight past its last member — the padding a person
    /// drags into on the way out of it.
    ///
    /// ```text
    ///   0  tabs[0]
    ///  20  heading of `group`      ┐
    ///  40  tabs[1]                 │ the block, 20..108
    ///  60  tabs[2]                 │
    ///  80  (the group's padding)   ┘
    ///  88  tabs[3]
    /// ```
    struct Panel {
        slots: Vec<Slot>,
        extents: Vec<Extent>,
        tabs: Vec<TabId>,
        group: GroupId,
    }

    fn panel() -> Panel {
        let tabs: Vec<TabId> = (0..4).map(|_| TabId::next()).collect();
        let group = GroupId::next();
        Panel {
            slots: vec![
                Slot {
                    tab: tabs[0],
                    group: None,
                    top: 0.,
                    bottom: 20.,
                },
                Slot {
                    tab: tabs[1],
                    group: Some(group),
                    top: 40.,
                    bottom: 60.,
                },
                Slot {
                    tab: tabs[2],
                    group: Some(group),
                    top: 60.,
                    bottom: 80.,
                },
                Slot {
                    tab: tabs[3],
                    group: None,
                    top: 88.,
                    bottom: 108.,
                },
            ],
            extents: vec![Extent {
                group,
                first: tabs[1],
                collapsed: false,
                top: 20.,
                bottom: 88.,
            }],
            tabs,
            group,
        }
    }

    impl Panel {
        /// The step a row carried to `middle` asks for.
        fn carry(&self, tab: TabId, middle: f32) -> Option<TabAction> {
            advance(
                &self.slots,
                &self.extents,
                Carried::Tab(tab),
                middle - 10. ..middle + 10.,
                middle,
            )
        }
    }

    #[test]
    fn a_row_lands_where_the_hand_is_and_not_one_place_nearer() {
        let panel = panel();

        assert_eq!(
            panel.carry(panel.tabs[3], 5.),
            Some(TabAction::MoveTab {
                tab: panel.tabs[3],
                group: None,
                before: Some(panel.tabs[0])
            }),
            "carried from the last row to above the first, it lands above the \
             first — a whole group and a row are between them, and both are \
             behind the hand"
        );
        assert_eq!(
            panel.carry(panel.tabs[0], 15.),
            None,
            "a row that has not reached its neighbour's middle has not moved"
        );
    }

    #[test]
    fn a_row_carried_into_a_groups_band_joins_it() {
        let panel = panel();

        assert_eq!(
            panel.carry(panel.tabs[0], 50.),
            Some(TabAction::MoveTab {
                tab: panel.tabs[0],
                group: Some(panel.group),
                before: Some(panel.tabs[1])
            }),
            "arriving from above it lands at the group's first row"
        );
        assert_eq!(
            panel.carry(panel.tabs[3], 70.),
            Some(TabAction::MoveTab {
                tab: panel.tabs[3],
                group: Some(panel.group),
                before: None
            }),
            "arriving from below it lands at the group's end"
        );
    }

    #[test]
    fn the_bands_margins_are_the_way_out_of_a_group() {
        let panel = panel();

        // The last eight pixels of the block are the padding under its last
        // member: the strip of the group that is not one of its rows.
        assert_eq!(
            panel.carry(panel.tabs[2], 81.),
            Some(TabAction::MoveTab {
                tab: panel.tabs[2],
                group: None,
                before: Some(panel.tabs[3])
            }),
            "carried into the padding under the group it should leave it"
        );
        assert_eq!(
            panel.carry(panel.tabs[1], 22.),
            Some(TabAction::MoveTab {
                tab: panel.tabs[1],
                group: None,
                before: Some(panel.tabs[2])
            }),
            "carried into the top of the heading it should leave it, above the block"
        );
        assert_eq!(
            panel.carry(panel.tabs[1], 45.),
            None,
            "inside the band, over its own row, it has not moved at all"
        );
    }

    #[test]
    fn a_member_of_the_last_group_can_still_be_taken_out_of_it() {
        // The case the panel could not do at all when the group under the
        // pointer was the row under the pointer: with nothing drawn below the
        // block, there was no position that meant "out".
        let mut panel = panel();
        panel.slots.pop();

        assert_eq!(
            panel.carry(panel.tabs[2], 100.),
            Some(TabAction::MoveTab {
                tab: panel.tabs[2],
                group: None,
                before: None
            }),
            "carried below the last block it should leave the group and land last"
        );
    }

    #[test]
    fn a_member_taken_out_of_the_middle_of_a_group_leaves_it_whole() {
        // Three members, and the one in the middle is carried out of the top
        // of the band. Naming the row it was between would split the group
        // into two runs with one identity; naming the group's own first row
        // puts it above the whole block instead.
        let mut panel = panel();
        let extra = TabId::next();
        panel.slots.insert(
            3,
            Slot {
                tab: extra,
                group: Some(panel.group),
                top: 80.,
                bottom: 100.,
            },
        );
        panel.slots[3].top = 80.;
        panel.extents[0].bottom = 108.;

        assert_eq!(
            panel.carry(panel.tabs[2], 22.),
            Some(TabAction::MoveTab {
                tab: panel.tabs[2],
                group: None,
                before: Some(panel.tabs[1])
            }),
        );
    }

    #[test]
    fn a_row_that_stops_in_a_bands_margin_passes_the_whole_block() {
        // The four pixels above a band are inside the block's box and outside
        // its group, so a row that stops there is not joining and has to be
        // placed by the neighbour rule instead — and the neighbour rule has to
        // answer with a position the strip will honour. Naming a *member* of a
        // group the row does not belong to is answered by the clamp with the
        // nearer end of that group's run, which for a group of three is the
        // end the row came from: the row would be wedged under a block it was
        // carried above until the hand went away and came back. So a block a
        // row is not part of is one step, and the row it names is the block's
        // first.
        let mut panel = panel();
        let extra = TabId::next();
        panel.slots.insert(
            3,
            Slot {
                tab: extra,
                group: Some(panel.group),
                top: 80.,
                bottom: 100.,
            },
        );
        panel.slots[4].top = 108.;
        panel.slots[4].bottom = 128.;
        panel.extents[0].bottom = 108.;

        assert_eq!(
            panel.carry(panel.tabs[3], 22.),
            Some(TabAction::MoveTab {
                tab: panel.tabs[3],
                group: None,
                before: Some(panel.tabs[1])
            }),
            "it landed in front of the block rather than inside it"
        );
    }

    #[test]
    fn a_press_that_travels_sideways_is_still_a_click() {
        // The threshold is a distance up or down the column, because up and
        // down the column is the only way a row can go. A press that slid
        // sideways and let go would otherwise lift a row, put it back in the
        // slot it came from, and eat the click that was actually made.
        let drag = PanelDrag::new();
        let tab = TabId::next();
        let carried = Carried::Tab(tab);
        drag.press(carried, vec2f(40., 100.), vec2f(0., 90.), 20.);

        assert_eq!(drag.moved_to(vec2f(140., 100.)), (false, None));
        assert!(
            drag.carrying().is_none(),
            "sideways is not a direction here"
        );
        assert_eq!(drag.moved_to(vec2f(140., 104.)), (false, None));
        assert!(drag.moved_to(vec2f(40., 106.)).0);
        assert_eq!(drag.carrying(), Some(carried));

        // And it stays a drag: a hand that comes back to where it pressed has
        // not turned the gesture back into a click.
        assert!(drag.moved_to(vec2f(40., 100.)).0);
        assert_eq!(drag.carrying(), Some(carried));
        assert!(
            drag.release(),
            "a gesture that travelled is not also a click"
        );
        assert!(drag.carrying().is_none());
    }

    #[test]
    fn a_carried_row_is_painted_where_it_was_picked_up_by() {
        // The offset is frozen at the press, so the row keeps the part of
        // itself that the hand took hold of. A row that snapped its own corner
        // to the pointer would jump on the first pixel of every drag.
        let drag = PanelDrag::new();
        let tab = TabId::next();
        let carried = Carried::Tab(tab);
        drag.press(carried, vec2f(40., 100.), vec2f(0., 90.), 20.);
        drag.moved_to(vec2f(40., 200.));

        assert_eq!(drag.lifted(carried), Some(190.));
        assert_eq!(
            drag.lifted(Carried::Tab(TabId::next())),
            None,
            "only the row that was picked up is in the air"
        );
    }

    #[test]
    fn nothing_is_carried_anywhere_in_an_empty_panel() {
        assert_eq!(
            advance(&[], &[], Carried::Tab(TabId::next()), 0. ..20., 10.),
            None
        );
        assert_eq!(
            advance(&[], &[], Carried::Group(GroupId::next()), 0. ..20., 10.),
            None
        );
    }

    #[test]
    fn the_band_is_the_blocks_box_less_a_margin_at_each_end() {
        // The two numbers the whole grouping gesture rests on, and both of
        // them are a strip of the block that is not one of its rows: four off
        // the heading's own padding, and the eight the block keeps under its
        // last member. A person leaving a group drags into one of them.
        let extent = panel().extents[0];

        assert!(!extent.holds(23.9), "above the band is out of the group");
        assert!(extent.holds(24.), "the band starts four inside the top");
        assert!(extent.holds(80.), "and ends eight short of the bottom");
        assert!(!extent.holds(80.1), "below the band is out of the group");
    }

    #[test]
    fn a_folded_block_carried_up_is_measured_from_its_middle() {
        // The two anchors are for a block several rows tall: the hand leads
        // going up because the top of the block is what a person is aiming
        // with, and the block's own middle leads going down. A folded block is
        // one row, so there is no gap between the two to exploit and Warp
        // measures both ends from the middle. Said here rather than left to
        // the numbers coinciding, because they only coincide by an accident of
        // where the hand happened to grab.
        let mut panel = panel();
        panel.slots.retain(|slot| slot.group.is_none());
        panel.extents[0].collapsed = true;
        panel.extents[0].bottom = 40.;

        let carried = Carried::Group(panel.group);
        assert_eq!(
            advance(&panel.slots, &panel.extents, carried, 20. ..40., 5.),
            None,
            "the hand is past the row above, and a folded block is not"
        );

        panel.extents[0].collapsed = false;
        assert_eq!(
            advance(&panel.slots, &panel.extents, carried, 20. ..40., 5.),
            Some(TabAction::MoveGroup {
                group: panel.group,
                before: Some(panel.tabs[0])
            }),
            "expanded, the same hand in the same place moves the block"
        );
    }

    #[test]
    fn a_folded_group_is_one_step_and_cannot_be_joined() {
        let mut panel = panel();
        panel.slots.retain(|slot| slot.group.is_none());
        panel.extents[0].collapsed = true;
        panel.extents[0].bottom = 40.;
        panel.slots[1].top = 40.;
        panel.slots[1].bottom = 60.;

        assert_eq!(
            panel.carry(panel.tabs[3], 25.),
            Some(TabAction::MoveTab {
                tab: panel.tabs[3],
                group: None,
                before: Some(panel.tabs[1])
            }),
            "a row carried over a folded group hops the whole block, not into it"
        );
    }

    #[test]
    fn a_carried_group_passes_whole_blocks() {
        let panel = panel();

        assert_eq!(
            advance(
                &panel.slots,
                &panel.extents,
                Carried::Group(panel.group),
                20. ..88.,
                5.,
            ),
            Some(TabAction::MoveGroup {
                group: panel.group,
                before: Some(panel.tabs[0])
            }),
            "the pointer leads going up: past the first row's middle is past the block"
        );
        assert_eq!(
            advance(
                &panel.slots,
                &panel.extents,
                Carried::Group(panel.group),
                70. ..138.,
                70.,
            ),
            Some(TabAction::MoveGroup {
                group: panel.group,
                before: None
            }),
            "the block's own middle leads going down, and past the last block is the end"
        );
        assert_eq!(
            advance(
                &panel.slots,
                &panel.extents,
                Carried::Group(panel.group),
                20. ..88.,
                15.,
            ),
            None,
            "the block's middle is long past the row above it, and the block has \
             not moved: going up it is the pointer that decides"
        );
    }
}
