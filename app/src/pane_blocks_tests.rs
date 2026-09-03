//! The scroll rules, stated one per test, and the seek that finds an item.

use crook_terminal::LiveBlock;

use super::*;

/// The id of the block a fresh session opens, which is the only one this
/// module can name without a terminal to hand out others.
fn a_block() -> BlockId {
    LiveBlock::default().id
}

/// A list with `content` lines of blocks in a `viewport`-line box.
fn measured(content: f32, viewport: f32) -> PaneBlocks {
    let list = PaneBlocks::new();
    list.measured(content, viewport);
    list
}

#[test]
fn a_list_that_fits_has_nothing_to_scroll() {
    let list = measured(10., 24.);

    assert_eq!(list.max_offset(), 0.);
    assert!(!list.is_scrollable());
    assert_eq!(
        list.offset(),
        0.,
        "the top, because that is also the bottom"
    );
}

#[test]
fn following_the_bottom_re_resolves_rather_than_pinning_a_number() {
    // The pitfall this enum exists for: a float that was the maximum a moment
    // ago leaves a person one line short of every line that arrives after it.
    let list = measured(100., 24.);
    assert_eq!(list.offset(), 76.);

    list.measured(140., 24.);
    assert_eq!(
        list.offset(),
        116.,
        "a growing block is followed with no auto-scroll step"
    );
}

#[test]
fn the_wheel_leaves_the_bottom_and_landing_on_it_again_returns() {
    let list = measured(100., 24.);

    assert!(list.apply(ScrollCause::Wheel(-30.)), "scrolled up");
    assert_eq!(list.position(), ScrollPosition::Fixed(46.));

    // Overshooting the end must re-enter the mode, not pin the number that was
    // the end when the wheel was turned.
    assert!(list.apply(ScrollCause::Wheel(1000.)));
    assert_eq!(list.position(), ScrollPosition::FollowBottom);

    list.measured(160., 24.);
    assert_eq!(list.offset(), 136., "and it is following again");
}

#[test]
fn the_wheel_cannot_scroll_above_the_top() {
    let list = measured(100., 24.);

    list.apply(ScrollCause::Wheel(-1000.));
    assert_eq!(list.position(), ScrollPosition::Fixed(0.));
    assert!(
        !list.apply(ScrollCause::Wheel(-10.)),
        "already at the top; a wheel that moves nothing repaints nothing"
    );
}

#[test]
fn running_a_command_and_typing_both_return_to_the_live_block() {
    for cause in [ScrollCause::Submit, ScrollCause::KeyToPty] {
        let list = measured(100., 24.);
        list.apply(ScrollCause::Wheel(-50.));
        assert_eq!(list.position(), ScrollPosition::Fixed(26.));

        assert!(list.apply(cause), "{cause:?} should have moved the list");
        assert_eq!(list.position(), ScrollPosition::FollowBottom);
    }
}

#[test]
fn a_resize_keeps_the_mode_until_the_offset_no_longer_fits() {
    let list = measured(100., 24.);
    list.apply(ScrollCause::Wheel(-40.));
    assert_eq!(list.position(), ScrollPosition::Fixed(36.));

    // Taller pane, same content: the offset still has content under it.
    list.measured(100., 40.);
    assert_eq!(list.position(), ScrollPosition::Fixed(36.));

    // Now it does not, so the list gives way rather than sitting on a
    // position with nothing at it.
    list.measured(50., 40.);
    assert_eq!(list.position(), ScrollPosition::FollowBottom);
}

#[test]
fn the_seek_finds_the_item_a_line_falls_inside_and_clamps_at_both_ends() {
    let mut heights = Heights::default();
    heights.sync((1, 3, 0), [4., 6., 2.].into_iter(), 5.);

    assert_eq!(heights.len(), 4, "three finished blocks and the live one");
    assert_eq!(heights.total(), 17.);
    assert_eq!(heights.start(2), 10.);
    assert_eq!(heights.height(1), 6.);

    assert_eq!(heights.seek(0.), 0);
    assert_eq!(heights.seek(3.9), 0);
    assert_eq!(
        heights.seek(4.),
        1,
        "a boundary belongs to the item below it"
    );
    assert_eq!(heights.seek(11.5), 2);
    assert_eq!(heights.seek(12.), 3, "the live block");

    assert_eq!(heights.seek(-100.), 0, "clamped rather than refused");
    assert_eq!(heights.seek(1e6), 3);
}

#[test]
fn only_the_live_block_is_re_summed_when_nothing_else_changed() {
    let mut heights = Heights::default();
    heights.sync((7, 2, 0), [4., 6.].into_iter(), 5.);
    assert_eq!(heights.total(), 15.);

    // The same finished list, a taller live block: the sums behind it are the
    // ones already there.
    heights.sync((7, 2, 0), std::iter::empty(), 9.);
    assert_eq!(heights.total(), 19.);
    assert_eq!(heights.start(2), 10., "the finished sums did not move");

    // A block finished, so the identity moved and the whole thing is rebuilt.
    heights.sync((8, 3, 0), [4., 6., 9.].into_iter(), 1.);
    assert_eq!(heights.total(), 20.);
    assert_eq!(heights.len(), 4);
}

#[test]
fn hovering_reports_only_the_changes() {
    let list = PaneBlocks::new();
    assert_eq!(list.hovered(), None);

    let block = a_block();
    assert!(
        list.hover(Some(block), false),
        "the pointer entered a block"
    );
    assert!(!list.hover(Some(block), false), "and did not move off it");
    assert!(list.hover(Some(block), true), "and then onto its control");
    assert!(list.is_on_control());

    assert!(list.hover(None, true), "the pointer left the list");
    assert!(
        !list.is_on_control(),
        "a control with no block under it is not hovered"
    );
}

#[test]
fn a_control_is_clicked_only_where_it_was_pressed() {
    let list = PaneBlocks::new();
    assert_eq!(list.release_control(), None, "no press, no click");

    let block = a_block();
    list.press_control(block);
    assert_eq!(list.release_control(), Some(block));
    assert_eq!(list.release_control(), None, "and the press is spent");
}
