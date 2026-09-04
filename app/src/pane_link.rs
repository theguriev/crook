//! The link under the pointer in one pane's output.
//!
//! Kept by the workspace, per pane, for exactly the reason
//! [`PaneSelection`](crate::pane_selection::PaneSelection) is: the element tree
//! is thrown away and rebuilt on every frame, and this is a fact about where
//! the pointer is that has to survive between two of them. The move that finds
//! a link and the frame that underlines it are different frames.
//!
//! # Why a link is only live while a modifier is held
//!
//! Because the pointer is already spoken for. Dragging across the output
//! selects it, and a terminal where a click on a URL opened a browser instead
//! of placing a selection would be a terminal you cannot copy a URL out of.
//! Every terminal resolves this the same way: hold the platform's own chord
//! key and links light up; let go and the pointer goes back to selecting. So
//! this is set only while that key is down, and cleared the moment it comes up
//! — which is what makes the underline appear and disappear under a pointer
//! that never moved.

use std::cell::RefCell;
use std::rc::Rc;

/// Where a link is, in the coordinates of the surface that found it.
///
/// The two surfaces number their rows differently — a grid row is a row of the
/// viewport, a block row is a row of one finished command — so the row is
/// stored beside a tag saying which it is. Without it, scrolling the list would
/// leave an underline on whichever row happened to inherit the number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    /// Which surface, and which of its rows.
    pub row: LinkRow,
    /// The first cell of the row the link occupies.
    pub start: usize,
    /// How many cells it covers.
    pub len: usize,
    /// The URL itself, which is what a click opens.
    pub uri: String,
}

/// Which row of which surface a link was found on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LinkRow {
    /// A row of the live viewport, as the grid draws it.
    Viewport(usize),
    /// A row of one finished block: its index in the list, and the row within
    /// it.
    Block {
        /// Which block, by its position in the pane's history.
        index: usize,
        /// Which of that block's rows.
        row: usize,
    },
}

/// The link under the pointer in one pane, if there is one.
///
/// Cheap to clone — it is an [`Rc`] — because the elements that draw a pane's
/// output take one every frame.
#[derive(Clone, Default)]
pub struct PaneLink(Rc<RefCell<Option<LinkSpan>>>);

impl PaneLink {
    /// A pane with no link under the pointer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The link under the pointer, if there is one.
    pub fn get(&self) -> Option<LinkSpan> {
        self.0.borrow().clone()
    }

    /// The link on a given row, if the one under the pointer is on it.
    ///
    /// What the paint path asks: it is drawing one row and wants to know
    /// whether any of its cells are underlined.
    pub fn on(&self, row: LinkRow) -> Option<LinkSpan> {
        self.0.borrow().clone().filter(|link| link.row == row)
    }

    /// The URL a click would open.
    pub fn uri(&self) -> Option<String> {
        self.0.borrow().as_ref().map(|link| link.uri.clone())
    }

    /// Puts a link under the pointer, reporting whether the frame changed.
    ///
    /// `None` clears it. The answer is what decides whether a pointer move is
    /// worth a repaint — a pointer travelling along one link produces a move
    /// per pixel and exactly one frame.
    pub fn set(&self, link: Option<LinkSpan>) -> bool {
        let mut held = self.0.borrow_mut();
        if *held == link {
            return false;
        }
        *held = link;
        true
    }

    /// Whether there is a link under the pointer at all.
    pub fn is_some(&self) -> bool {
        self.0.borrow().is_some()
    }

    /// The link, if it is on a viewport row this grid still draws.
    ///
    /// The bound matters: a grid that shrank between the move that found the
    /// link and the frame that draws it would otherwise underline a row
    /// outside its own box.
    pub fn on_viewport_rows(&self, rows: usize) -> Option<LinkSpan> {
        self.0
            .borrow()
            .clone()
            .filter(|link| matches!(link.row, LinkRow::Viewport(row) if row < rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(row: LinkRow) -> LinkSpan {
        LinkSpan {
            row,
            start: 4,
            len: 19,
            uri: "https://example.com".to_owned(),
        }
    }

    #[test]
    fn a_link_is_only_offered_to_the_row_it_was_found_on() {
        // The two surfaces number their rows differently. Without the tag, a
        // list scrolled by one row would underline whichever row inherited the
        // number.
        let link = PaneLink::new();
        link.set(Some(span(LinkRow::Viewport(3))));

        assert!(link.on(LinkRow::Viewport(3)).is_some());
        assert!(link.on(LinkRow::Viewport(4)).is_none());
        assert!(
            link.on(LinkRow::Block { index: 0, row: 3 }).is_none(),
            "row 3 of a block is not row 3 of the viewport"
        );
    }

    #[test]
    fn setting_the_same_link_again_is_not_a_repaint() {
        // A pointer travelling along one link produces a move per pixel. Every
        // one of them finds the same link, and exactly one of them is worth a
        // frame.
        let link = PaneLink::new();

        assert!(link.set(Some(span(LinkRow::Viewport(1)))));
        assert!(!link.set(Some(span(LinkRow::Viewport(1)))));
        assert!(link.set(Some(span(LinkRow::Viewport(2)))));
        assert!(link.set(None));
        assert!(!link.set(None));
    }

    #[test]
    fn the_element_that_is_handed_a_clone_is_looking_at_the_same_link() {
        let kept = PaneLink::new();
        let handed_to_the_element = kept.clone();

        handed_to_the_element.set(Some(span(LinkRow::Viewport(0))));

        assert!(kept.is_some());
        assert_eq!(kept.uri().as_deref(), Some("https://example.com"));
    }
}
