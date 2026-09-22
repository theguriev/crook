//! The tree a group's members hang from.
//!
//! Warp indents a group's members and stops there, and an indent is a relation
//! only to somebody who already knows to look for one: the rows under a
//! heading are the same rows as the ones beside it, moved twelve pixels.
//! herdr — the tool the worktree menu was modelled on — draws the relation
//! instead, as a stem down the heading's side with an elbow into each member,
//! and this is that stem in the twelve pixels the indent had already spent. So
//! nothing moves, and the gap that was empty now says what it was left for.
//!
//! # The node is the row's own mark
//!
//! herdr ends each elbow in a disc coloured by the session's state, and there
//! is none here. A row already carries that disc at its head — in
//! [`TAB_ROW_MARK`](crate::plugins::tabs::TAB_ROW_MARK), which a plugin may
//! have taken — so a second one four pixels to its left would be the same fact
//! twice, in the host's colours beside whatever the plugin chose. The elbow
//! arrives at the mark and lets the mark be the node.
//!
//! # Why an element and not two containers
//!
//! A line down the side of a box has to be as tall as the box, and nothing in
//! this tier can say "as tall as my sibling": a container is as tall as its
//! child, and an [`Empty`](crookui_core::elements::Empty) offered an unbounded
//! height answers with the minimum rather than with the room a member has
//! taken. The rail is therefore the box itself — it lays the member out inside
//! the gutter it keeps, and paints two rectangles of its own into it.

use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::DispatchedEvent;
use crookui_core::geometry::{Point, RectF, Vector2F, vec2f};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};

use crate::plugins::tabs::MARK_SIZE;
use crate::settings::Granularity;
use crate::theme::theme;

use super::MEMBER_INDENT;

/// How thick a line is: a hairline, like every other line in the panel.
const THICKNESS: f32 = 1.;

/// How far in the stem runs, from the left edge of the group's block.
///
/// The heading's own left padding, so the line comes down from under the
/// chevron that folds the group rather than from the edge of the panel — the
/// stem and the heading are one shape, and a line hugging the panel's edge
/// reads as a border somebody drew by accident. It stays clear of the members
/// themselves, which begin four pixels further in.
const STEM: f32 = super::GROUP_HORIZONTAL_PADDING;

/// Where a member's elbow meets it: how far down its block, and how far into
/// it, both measured from the block's top-left corner.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct Meeting {
    /// How far down the elbow runs.
    pub(super) node: f32,
    /// How far in it reaches, from the left edge of the gutter.
    pub(super) reach: f32,
}

/// Where the rail meets a member.
///
/// Down as far as the middle of the mark at the head of its first row, and in
/// as far as the box that mark is drawn in — so the line arrives at the disc
/// that says what the session is doing rather than stopping in the gap beside
/// it, which is a tree with its branches cut short. It stops at the box and
/// not at the disc inside it because the mark may be a plugin's icon, and
/// nothing in the host knows how much of its box a plugin chose to fill.
///
/// Which pixel either of them is depends on what the granularity wraps the row
/// in, and on nothing else: a row is 8px of padding around a 24px mark in both
/// densities, which is why no density appears here.
pub(super) fn meets(granularity: Granularity, names_itself: bool) -> Meeting {
    let row = super::row::ROW_PADDING;
    match granularity {
        Granularity::Tabs => Meeting {
            node: row + MARK_SIZE / 2.,
            reach: MEMBER_INDENT + row,
        },
        // A `Panes` tab that names itself puts that name where its first row
        // would have been, and the name is what the member *is* — so the elbow
        // stops at the name rather than reaching past it to the first of the
        // panes under it. Half the type rather than half the line box it is
        // set in, which is a pixel high: the line box is the font's, and a
        // rail measured off a font moves when somebody loads another one.
        Granularity::Panes if names_itself => Meeting {
            node: super::GROUP_HEADER_VERTICAL_PADDING + super::GROUP_HEADER_SIZE / 2.,
            reach: MEMBER_INDENT + super::GROUP_HORIZONTAL_PADDING,
        },
        Granularity::Panes => Meeting {
            node: super::GROUP_HORIZONTAL_PADDING + row + MARK_SIZE / 2.,
            reach: MEMBER_INDENT + super::GROUP_HORIZONTAL_PADDING + row,
        },
    }
}

/// The two lines one member's rail is drawn out of, in the rail's own
/// coordinates: the stem down the gutter, and the elbow into the member.
///
/// `continues` is the gap to the next member, and `None` on the last one —
/// which is the whole difference between a `├` and a `└`. The gap is part of
/// it because the stem is drawn per member and the list puts room between
/// them in `Tabs` granularity: a stem that stopped at each member's own bottom
/// edge would be a dashed line down a list of cards.
///
/// A node below the member is brought back to it. Nothing should produce one —
/// the offsets above are smaller than any row — but a rail whose elbow hangs
/// in the gap under the last member would be drawn on top of whatever is
/// there, and clamping is cheaper than trusting the arithmetic.
fn lines(height: f32, meeting: Meeting, continues: Option<f32>) -> [RectF; 2] {
    let node = meeting.node.min(height);
    let stem = match continues {
        Some(gap) => height + gap,
        None => node,
    };

    [
        RectF::new(vec2f(STEM, 0.), vec2f(THICKNESS, stem)),
        RectF::new(vec2f(STEM, node), vec2f(meeting.reach - STEM, THICKNESS)),
    ]
}

/// One member of a group, with the rail that ties it to the heading.
///
/// It owns the indent as well as the lines: the member used to be moved by a
/// padding on the column all of them shared, and a gutter drawn by one element
/// and reserved by another is two numbers that have to agree.
pub(super) struct Rail {
    child: Box<dyn Element>,
    meeting: Meeting,
    continues: Option<f32>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Rail {
    /// Hangs `child` off the rail, met where [`meets`] says.
    ///
    /// `continues` is the gap to the member under this one, and `None` when
    /// this is the last of them.
    pub(super) fn new(child: Box<dyn Element>, meeting: Meeting, continues: Option<f32>) -> Self {
        Self {
            child,
            meeting,
            continues,
            size: None,
            origin: None,
        }
    }
}

impl Element for Rail {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let gutter = vec2f(MEMBER_INDENT, 0.);
        let child = SizeConstraint {
            min: (constraint.min - gutter).max(Vector2F::zero()),
            max: (constraint.max - gutter).max(Vector2F::zero()),
        };

        let size = self.child.layout(child, ctx, app) + gutter;
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let size = self
            .size
            .expect("a rail was painted before it was laid out");

        self.child
            .paint(origin + vec2f(MEMBER_INDENT, 0.), ctx, app);

        // After the member, and the elbow ends inside it: the last few pixels
        // of the line cross the member's own left padding to reach its mark,
        // which is what joins the two — a line stopping at the edge of the
        // gutter reads as a tree with its branches cut off. Painted first it
        // would disappear under the fill a hovered, selected or waiting row
        // grows. Nothing here is hit-tested: the rail is decoration, and a
        // press in it belongs to whatever is behind it.
        for line in lines(size.y(), self.meeting, self.continues) {
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(origin + line.origin(), line.size()))
                .with_background(theme().border);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A row is 40px tall in `Compact`, which is the density the panel opens
    /// in; the numbers below are that row unless they say otherwise.
    const ROW: f32 = 40.;

    /// Where a `Tabs` member is met, which is the simplest of the three.
    fn meeting() -> Meeting {
        meets(Granularity::Tabs, false)
    }

    #[test]
    fn the_last_member_s_stem_stops_at_its_own_elbow() {
        let [stem, elbow] = lines(ROW, meeting(), None);

        assert_eq!(stem.height(), meeting().node, "the stem ran past the elbow");
        assert_eq!(stem.max_y(), elbow.min_y(), "the corner has a gap in it");
    }

    #[test]
    fn a_member_with_one_under_it_carries_the_stem_over_the_gap() {
        let [stem, _] = lines(ROW, meeting(), Some(4.));

        // The member, and the room the list leaves before the next one: a stem
        // that stopped at 40 would be a dashed line down a list of cards.
        assert_eq!(stem.height(), 44.);
    }

    #[test]
    fn the_elbow_leaves_the_stem_and_reaches_into_the_member() {
        let [stem, elbow] = lines(ROW, meeting(), None);

        assert_eq!(elbow.min_x(), stem.min_x());
        assert!(
            elbow.max_x() > MEMBER_INDENT,
            "the elbow stopped at the gutter instead of reaching the mark"
        );
        assert_eq!(elbow.max_x(), meeting().reach);
    }

    #[test]
    fn a_node_under_the_member_is_brought_back_onto_it() {
        let short = 12.;
        let [stem, elbow] = lines(short, meeting(), None);

        assert_eq!(elbow.min_y(), short, "the elbow hangs below the member");
        assert_eq!(stem.height(), short);
    }

    #[test]
    fn the_elbow_meets_the_mark_at_the_head_of_the_first_row() {
        // A row's own 8px of padding, then half of its 24px mark.
        assert_eq!(meets(Granularity::Tabs, false).node, 20.);
        // And in `Panes`, the 8px the tab insets its rows by first.
        assert_eq!(meets(Granularity::Panes, false).node, 28.);
        // Which is also what the elbow has to cross to arrive at the mark.
        assert_eq!(
            meets(Granularity::Panes, false).reach - meets(Granularity::Tabs, false).reach,
            8.
        );
    }

    #[test]
    fn a_member_that_names_itself_is_met_at_the_name() {
        let named = meets(Granularity::Panes, true);

        assert!(
            named.node < meets(Granularity::Panes, false).node,
            "the elbow reached past the name to the rows under it"
        );
        // The name is a `Tabs` member's whole block: nothing above it.
        assert!(named.node < meets(Granularity::Tabs, false).node);
        assert!(
            named.reach < meets(Granularity::Panes, false).reach,
            "the elbow reached past the name it stops at"
        );
    }

    #[test]
    fn a_name_is_only_a_thing_in_panes() {
        assert_eq!(
            meets(Granularity::Tabs, true),
            meets(Granularity::Tabs, false),
            "a `Tabs` member has no header to aim at"
        );
    }
}
