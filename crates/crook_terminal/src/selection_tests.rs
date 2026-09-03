use super::*;

/// A span over one run of cells, the way a plain drag leaves one.
fn span(start: (i32, usize), end: (i32, usize)) -> SelectionSpan {
    SelectionSpan {
        start: GridPoint::new(start.0, start.1),
        end: GridPoint::new(end.0, end.1),
        block: false,
    }
}

#[test]
fn test_a_span_runs_to_the_end_of_a_line_and_on_to_the_next() {
    // What tells a run of text from a rectangle: the middle lines of a
    // multi-line selection are selected edge to edge, whatever columns the
    // two ends happen to be in.
    let selected = span((0, 4), (2, 2));

    assert!(!selected.contains(GridPoint::new(0, 3)), "before the start");
    assert!(selected.contains(GridPoint::new(0, 4)));
    assert!(
        selected.contains(GridPoint::new(0, 79)),
        "to the line's end"
    );
    assert!(
        selected.contains(GridPoint::new(1, 0)),
        "and all of the next"
    );
    assert!(selected.contains(GridPoint::new(1, 79)));
    assert!(selected.contains(GridPoint::new(2, 2)));
    assert!(!selected.contains(GridPoint::new(2, 3)), "past the end");
    assert!(
        !selected.contains(GridPoint::new(3, 0)),
        "past the last line"
    );
}

#[test]
fn test_a_block_span_takes_the_same_columns_out_of_every_line() {
    let selected = SelectionSpan {
        block: true,
        ..span((0, 4), (2, 6))
    };

    assert!(selected.contains(GridPoint::new(1, 4)));
    assert!(selected.contains(GridPoint::new(1, 6)));
    assert!(
        !selected.contains(GridPoint::new(1, 7)),
        "a block never runs to the end of a line"
    );
    assert!(!selected.contains(GridPoint::new(1, 3)));
}

#[test]
fn test_a_span_of_one_cell_holds_exactly_that_cell() {
    let selected = span((-3, 10), (-3, 10));

    assert!(selected.contains(GridPoint::new(-3, 10)));
    assert!(!selected.contains(GridPoint::new(-3, 9)));
    assert!(!selected.contains(GridPoint::new(-3, 11)));
    assert!(
        !selected.contains(GridPoint::new(-4, 10)),
        "a line above is history, not the same line"
    );
}

#[test]
fn test_a_grid_point_survives_the_round_trip_through_the_emulator_types() {
    // Scrollback is negative and the conversion has to keep the sign: a
    // selection four screens back that came home as line 4 would highlight
    // the live output instead.
    for point in [
        GridPoint::new(0, 0),
        GridPoint::new(12, 40),
        GridPoint::new(-137, 3),
    ] {
        let converted: alacritty_terminal::index::Point = point.into();
        assert_eq!(point, GridPoint::from(converted));
    }
}

#[test]
fn test_every_gesture_maps_to_the_selection_the_emulator_has_for_it() {
    assert_eq!(SelectionType::Simple, SelectionKind::Simple.into());
    assert_eq!(SelectionType::Block, SelectionKind::Block.into());
    assert_eq!(SelectionType::Semantic, SelectionKind::Semantic.into());
    assert_eq!(SelectionType::Lines, SelectionKind::Lines.into());
    assert_eq!(Side::Left, CellSide::Left.into());
    assert_eq!(Side::Right, CellSide::Right.into());
}
