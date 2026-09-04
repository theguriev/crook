//! Where the panel's rows ended up, so that one of them can be scrolled to.
//!
//! A [`Scrollable`](crookui_core::elements::Scrollable) can be told to scroll
//! to an offset. What it cannot be asked is "where is the row for pane 7?" —
//! it holds one child and knows nothing about what is inside it, and the rows
//! are not a fixed height anyway: the density changes them, the granularity
//! changes how many there are, and a group header sits above each tab's.
//!
//! So the rows write down where they were drawn, and this is where they write
//! it. It is the same trick [`PaneExtent`](crate::pane_split::PaneExtent) uses
//! for a divider, and it is here for the same reason: the number is produced
//! by the paint pass and wanted by a keystroke three frames later.
//!
//! # Why offsets from the content rather than from the window
//!
//! A row's paint origin is in window coordinates, and so is the scrollable's.
//! The offset a `scroll_to` takes is neither: it is measured from the top of
//! the *content*, which is the window origin of the content minus however far
//! it has already been scrolled. Subtracting one from the other here — both
//! recorded in the same paint pass — leaves exactly that, and leaves it
//! correct whatever the panel's own chrome above the list happens to measure.

use std::cell::RefCell;
use std::rc::Rc;

use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::DispatchedEvent;
use crookui_core::geometry::{Point, Vector2F};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};
use std::collections::HashMap;

use crate::tab::PaneId;

/// Where one row was drawn, in content coordinates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct RowBox {
    /// The row's top edge, measured from the top of the list's content.
    pub(crate) top: f32,
    /// How tall it is.
    pub(crate) height: f32,
}

/// Where every row of the panel was drawn.
///
/// Cheap to clone — it is an [`Rc`] — because the elements that draw the rows
/// take one every frame.
#[derive(Clone, Default)]
pub(crate) struct RowGeometry(Rc<RefCell<Inner>>);

#[derive(Default)]
struct Inner {
    /// Where the list's content began, in window coordinates, on the frame
    /// being recorded.
    content_top: f32,
    /// The rows, keyed by the pane each stands for.
    rows: HashMap<PaneId, RowBox>,
}

impl RowGeometry {
    /// A panel nothing has drawn yet.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Starts a frame: forgets the rows the last one drew and records where
    /// the content begins.
    ///
    /// Cleared rather than overwritten, because a row that is no longer drawn
    /// is a pane that has closed — and an entry left behind for it would be a
    /// scroll target that no longer exists.
    pub(crate) fn begin(&self, content_top: f32) {
        let mut inner = self.0.borrow_mut();
        inner.content_top = content_top;
        inner.rows.clear();
    }

    /// Records one row, given where it was painted in window coordinates.
    pub(crate) fn record(&self, pane: PaneId, window_top: f32, height: f32) {
        let mut inner = self.0.borrow_mut();
        let top = window_top - inner.content_top;
        inner.rows.insert(pane, RowBox { top, height });
    }

    /// Where a row is, if it was drawn on the last frame.
    pub(crate) fn get(&self, pane: PaneId) -> Option<RowBox> {
        self.0.borrow().rows.get(&pane).copied()
    }
}

/// The offset a viewport has to be scrolled to for `row` to be inside it, or
/// `None` when it already is.
///
/// The nearer edge wins, which is what every list does: a row above the
/// viewport comes to the top and one below it comes to the bottom, so a
/// selection walked down the list moves the view by one row at a time rather
/// than jumping the row to the middle.
pub(crate) fn scroll_for(row: RowBox, offset: f32, viewport: f32) -> Option<f32> {
    if row.top < offset {
        return Some(row.top);
    }
    if row.top + row.height <= offset + viewport {
        return None;
    }

    // Past the bottom. A row that *fits* comes up until its last pixel is at
    // the bottom edge; one taller than the viewport cannot, and showing its
    // bottom would push its beginning off the top — so it shows its top and
    // the rest is the wheel's business. A row already showing its top is
    // therefore already as visible as it can be.
    let target = if row.height >= viewport {
        row.top
    } else {
        row.top + row.height - viewport
    };
    (target != offset).then_some(target)
}

/// Wraps one row, recording where it was painted.
///
/// A wrapper rather than a line inside the row's own renderer because the row
/// is built out of containers, hover handlers and flexes, and none of them
/// reports its own box to anything: the outermost element is the only one that
/// knows the whole row's extent.
pub(crate) struct Tracked {
    pane: PaneId,
    geometry: RowGeometry,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Tracked {
    /// Records `child`'s box into `geometry` under `pane`.
    pub(crate) fn new(pane: PaneId, geometry: RowGeometry, child: Box<dyn Element>) -> Self {
        Self {
            pane,
            geometry,
            child,
            size: None,
            origin: None,
        }
    }
}

impl Element for Tracked {
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
        self.geometry
            .record(self.pane, origin.y(), self.size.map_or(0., |size| size.y()));
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

/// Wraps the list's content, recording where it begins.
///
/// One of these sits inside the scrollable and outside every row, which is
/// what makes a row's offset measurable from the content rather than from the
/// window.
pub(crate) struct Content {
    geometry: RowGeometry,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Content {
    /// Records where `child` begins, and clears the rows the last frame drew.
    pub(crate) fn new(geometry: RowGeometry, child: Box<dyn Element>) -> Self {
        Self {
            geometry,
            child,
            size: None,
            origin: None,
        }
    }
}

impl Element for Content {
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
        self.geometry.begin(origin.y());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(top: f32, height: f32) -> RowBox {
        RowBox { top, height }
    }

    #[test]
    fn a_row_already_in_view_scrolls_nowhere() {
        assert_eq!(scroll_for(row(40., 30.), 0., 200.), None);
        assert_eq!(scroll_for(row(40., 30.), 40., 30.), None, "exactly filling");
    }

    #[test]
    fn the_nearer_edge_wins() {
        // A selection walked down the list moves the view a row at a time
        // rather than jumping the row to the middle.
        assert_eq!(scroll_for(row(10., 30.), 100., 200.), Some(10.));
        assert_eq!(scroll_for(row(300., 30.), 0., 200.), Some(130.));
    }

    #[test]
    fn a_row_taller_than_the_viewport_shows_its_top() {
        // Scrolling to its bottom would push its own beginning off the top,
        // which is the half a person is reading.
        assert_eq!(
            scroll_for(row(0., 400.), 0., 200.),
            None,
            "its top is already at the top"
        );
        assert_eq!(scroll_for(row(300., 400.), 0., 200.), Some(300.));
        assert_eq!(scroll_for(row(10., 400.), 20., 200.), Some(10.));
    }

    #[test]
    fn an_offset_is_measured_from_the_content_rather_than_the_window() {
        // The panel has chrome above its list, and the list may already be
        // scrolled. Both are in the window origin the content reports, and
        // neither belongs in a `scroll_to`.
        let geometry = RowGeometry::new();
        let pane = PaneId::next();

        geometry.begin(120.);
        geometry.record(pane, 200., 44.);

        assert_eq!(geometry.get(pane), Some(row(80., 44.)));
    }

    #[test]
    fn a_row_that_stopped_being_drawn_stops_being_a_target() {
        // Its pane has closed. An entry left behind would be a scroll target
        // that does not exist.
        let geometry = RowGeometry::new();
        let (kept, closed) = (PaneId::next(), PaneId::next());

        geometry.begin(0.);
        geometry.record(kept, 0., 44.);
        geometry.record(closed, 44., 44.);
        assert!(geometry.get(closed).is_some());

        geometry.begin(0.);
        geometry.record(kept, 0., 44.);

        assert!(geometry.get(kept).is_some());
        assert_eq!(geometry.get(closed), None);
    }
}
