use crook_terminal::CellSide;

use super::*;

/// The block ids a test needs: two of them, in order, out of a real terminal
/// so that the ordering is the one the list actually hands out.
fn ids() -> (crook_terminal::BlockId, crook_terminal::BlockId) {
    let mut emulator =
        crook_terminal::Emulator::new(crook_terminal::TerminalSize::new(20, 5), 100, {
            crook_terminal::Palette::default()
        });
    emulator.advance(b"\x1b]133;A\x07one\x1b]133;D;0\x07\x1b]133;A\x07two");
    let first = emulator.blocks().first().expect("one block closed").id;
    (first, emulator.live_block().id)
}

/// An anchor in a block, on the left of a cell.
fn at(block: crook_terminal::BlockId, row: usize, column: usize) -> Anchor {
    Anchor::new(block, row, column, CellSide::Left)
}

#[test]
fn test_a_press_with_no_drag_behind_it_selects_nothing() {
    // What makes a plain click on the output let go of the last selection
    // rather than leave a one-cell highlight where it landed.
    let (block, _) = ids();
    let selection = PaneSelection::new();

    selection.press(
        SelectionKind::Simple,
        at(block, 0, 0),
        true,
        20,
        Cells::List,
    );
    assert!(selection.has_selection());

    selection.press(
        SelectionKind::Simple,
        at(block, 0, 4),
        false,
        20,
        Cells::List,
    );
    assert!(
        !selection.has_selection(),
        "a click left something selected"
    );
    assert!(selection.is_dragging(), "and the press is still open");
}

#[test]
fn test_a_drag_only_selects_once_it_covers_cells() {
    let (block, _) = ids();
    let selection = PaneSelection::new();

    selection.press(
        SelectionKind::Simple,
        at(block, 0, 0),
        false,
        20,
        Cells::List,
    );
    assert!(!selection.drag(at(block, 0, 0), false) || !selection.has_selection());
    assert!(selection.drag(at(block, 0, 4), true));
    assert_eq!(
        Some(Selection::new(
            SelectionKind::Simple,
            at(block, 0, 0),
            at(block, 0, 4)
        )),
        selection.selection()
    );
}

#[test]
fn test_a_drag_with_no_press_behind_it_is_not_this_panes_business() {
    // Only the pane the press landed on has a gesture, which is what keeps a
    // drag that began in another pane out of this one's selection.
    let (block, _) = ids();
    let untouched = PaneSelection::new();
    assert!(!untouched.drag(at(block, 0, 4), true));
    assert!(!untouched.has_selection());
    assert!(!untouched.release(), "a pane with no press has no release");
}

#[test]
fn test_the_element_handed_a_clone_is_looking_at_the_same_selection() {
    // The whole reason it is an `Rc`: the tree that saw the press has been
    // thrown away by the time the drag arrives.
    let (block, _) = ids();
    let kept = PaneSelection::new();
    let handed_to_the_element = kept.clone();

    handed_to_the_element.press(
        SelectionKind::Simple,
        at(block, 0, 0),
        false,
        20,
        Cells::List,
    );
    handed_to_the_element.drag(at(block, 0, 4), true);
    assert!(kept.is_dragging());
    assert!(kept.has_selection());
}

#[test]
fn test_a_reflow_lets_go_of_a_selection_made_at_another_width() {
    // The cells a selection named hold other text once the rows have been
    // re-wrapped under it, and there is no honest way to re-anchor that.
    let (block, _) = ids();
    let selection = PaneSelection::new();
    selection.press(
        SelectionKind::Simple,
        at(block, 0, 0),
        true,
        20,
        Cells::List,
    );

    assert!(!selection.reflowed(20), "the same width changed nothing");
    assert!(selection.has_selection());
    assert!(selection.reflowed(40));
    assert!(!selection.has_selection());
    assert!(!selection.reflowed(60), "and there is nothing left to drop");
}

#[test]
fn test_a_selection_spans_the_blocks_its_anchors_name() {
    let (first, live) = ids();
    let selection = PaneSelection::new();
    selection.press(
        SelectionKind::Simple,
        at(first, 0, 0),
        false,
        20,
        Cells::List,
    );
    selection.drag(at(live, 0, 2), true);

    let selection = selection.selection().expect("something is selected");
    assert_eq!(first, selection.anchor.block);
    assert_eq!(live, selection.head.block);
}

#[test]
fn test_a_change_of_surface_lets_go_of_a_selection_made_on_the_other_one() {
    // A block's rows are numbered from its own first and a grid's from the
    // oldest line of the scrollback, so the same anchor names two different
    // rows. A pane that falls back to the grid mid-drag — a command printing
    // past the top of the viewport does it — takes the selection with it.
    let (block, _) = ids();
    let selection = PaneSelection::new();
    selection.press(
        SelectionKind::Simple,
        at(block, 0, 0),
        true,
        20,
        Cells::List,
    );

    assert!(!selection.resurfaced(Cells::List), "the same surface");
    assert!(selection.has_selection_in(Cells::List));
    assert!(
        !selection.has_selection_in(Cells::Grid),
        "the grid cannot draw a selection made on the list"
    );

    assert!(selection.resurfaced(Cells::Grid));
    assert!(!selection.has_selection(), "and it is let go of outright");
    assert!(
        !selection.is_dragging(),
        "half a gesture in each space is a region with one end in each"
    );
}
