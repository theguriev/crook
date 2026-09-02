//! Executable specification for the strip.
//!
//! Every case here is one that a tabbed terminal gets wrong at least once:
//! closing on either side of the selection, closing the selection itself,
//! closing the last one, and moving a tab that is already at the end it was
//! asked to move towards.

use super::*;

/// A strip of `count` tabs, with the ids in bar order.
fn strip(count: usize) -> (TabStrip, Vec<TabId>) {
    let mut strip = TabStrip::new();
    while strip.len() < count {
        strip.apply(TabAction::New);
    }
    let ids = strip.iter().map(Tab::id).collect();
    (strip, ids)
}

fn order(strip: &TabStrip) -> Vec<TabId> {
    strip.iter().map(Tab::id).collect()
}

#[test]
fn a_new_strip_has_one_tab_and_it_is_active() {
    let strip = TabStrip::new();

    assert_eq!(strip.len(), 1);
    assert_eq!(strip.active().map(Tab::id), Some(strip.active_id()));
    assert_eq!(strip.mru(), [strip.active_id()]);
}

#[test]
fn a_new_tab_lands_after_the_active_one_and_takes_the_selection() {
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[0]));

    strip.apply(TabAction::New);

    let opened = strip.active_id();
    assert_eq!(order(&strip), [ids[0], opened, ids[1], ids[2]]);
}

#[test]
fn selecting_moves_the_tab_to_the_front_of_the_mru_list() {
    let (mut strip, ids) = strip(3);

    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Select(ids[2]));

    assert!(strip.is_active(ids[2]));
    assert_eq!(strip.mru(), [ids[2], ids[0], ids[1]]);
}

#[test]
fn selecting_a_tab_that_is_gone_changes_nothing() {
    let (mut strip, ids) = strip(2);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Close(ids[1]));

    strip.apply(TabAction::Select(ids[1]));

    assert!(strip.is_active(ids[0]));
    assert_eq!(order(&strip), [ids[0]]);
}

#[test]
fn closing_a_tab_before_the_active_one_leaves_the_selection_alone() {
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[2]));

    assert_eq!(strip.apply(TabAction::Close(ids[0])), TabEffect::Changed);

    assert!(strip.is_active(ids[2]));
    assert_eq!(order(&strip), [ids[1], ids[2]]);
}

#[test]
fn closing_a_tab_after_the_active_one_leaves_the_selection_alone() {
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[0]));

    strip.apply(TabAction::Close(ids[2]));

    assert!(strip.is_active(ids[0]));
    assert_eq!(order(&strip), [ids[0], ids[1]]);
}

#[test]
fn closing_the_active_tab_falls_back_to_the_one_used_before_it() {
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Select(ids[2]));

    strip.apply(TabAction::Close(ids[2]));

    assert!(strip.is_active(ids[0]));
    assert_eq!(strip.mru(), [ids[0], ids[1]]);
}

#[test]
fn closing_the_last_tab_asks_for_the_window_instead_of_emptying_the_strip() {
    let (mut strip, ids) = strip(1);

    assert_eq!(
        strip.apply(TabAction::Close(ids[0])),
        TabEffect::CloseWindow
    );

    assert_eq!(strip.len(), 1);
    assert!(strip.active().is_some());
}

#[test]
fn the_mru_list_never_names_a_closed_tab() {
    let (mut strip, ids) = strip(4);
    for id in &ids {
        strip.apply(TabAction::Select(*id));
    }

    strip.apply(TabAction::Close(ids[1]));
    strip.apply(TabAction::Close(ids[3]));

    assert_eq!(strip.mru(), [ids[2], ids[0]]);
    assert!(strip.is_active(ids[2]));
}

#[test]
fn moving_right_swaps_with_the_next_tab_and_keeps_the_selection() {
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[0]));

    strip.apply(TabAction::MoveRight);

    assert_eq!(order(&strip), [ids[1], ids[0], ids[2]]);
    assert!(strip.is_active(ids[0]));
}

#[test]
fn moving_left_swaps_with_the_previous_tab_and_keeps_the_selection() {
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[2]));

    strip.apply(TabAction::MoveLeft);

    assert_eq!(order(&strip), [ids[0], ids[2], ids[1]]);
    assert!(strip.is_active(ids[2]));
}

#[test]
fn moving_past_either_end_does_nothing() {
    let (mut strip, ids) = strip(3);

    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::MoveLeft);
    assert_eq!(order(&strip), ids);

    strip.apply(TabAction::Select(ids[2]));
    strip.apply(TabAction::MoveRight);
    assert_eq!(order(&strip), ids);
}

#[test]
fn an_action_that_moves_nothing_reports_that_it_moved_nothing() {
    // What a held-down shortcut turns into. A caller that repainted on every
    // action would rebuild and re-paint the window at key-repeat rate for a
    // bar that is already where it is being asked to go.
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[0]));

    assert_eq!(strip.apply(TabAction::MoveLeft), TabEffect::Unchanged);
    assert_eq!(
        strip.apply(TabAction::Select(ids[0])),
        TabEffect::Unchanged,
        "selecting the tab that is already selected"
    );
    assert_eq!(strip.apply(TabAction::MoveRight), TabEffect::Changed);

    strip.apply(TabAction::Close(ids[2]));
    assert_eq!(
        strip.apply(TabAction::Close(ids[2])),
        TabEffect::Unchanged,
        "a second close of the same tab, as a stale click would send"
    );
}

#[test]
fn every_tab_a_strip_opens_gets_a_name_no_other_tab_has_had() {
    // Naming from `len()` repeats as soon as a tab in the middle is closed,
    // and the title is also the body panel's heading: two live agent sessions
    // would be indistinguishable in both places.
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Close(ids[1]));
    strip.apply(TabAction::New);

    let titles: Vec<&str> = strip.iter().map(Tab::title).collect();
    assert_eq!(titles, ["agent 1", "agent 3", "agent 4"]);
}

#[test]
fn reporting_progress_into_a_session_cannot_touch_its_tabs_identity() {
    // `get_mut` is the documented agent-progress path, so whatever it hands
    // out is reachable from ordinary code. What it must not hand out is a
    // settable `id`: two tabs sharing one makes `index_of` resolve a close to
    // the wrong session, and a session carrying a second id of its own is one
    // assignment away from disagreeing with its tab's.
    let (mut strip, ids) = strip(3);
    let tab = strip.get_mut(ids[1]).expect("still open");

    tab.session_mut().title = "renamed".to_owned();
    tab.session_mut().status = AgentStatus::Failed;

    assert_eq!(order(&strip), ids);
    assert_eq!(strip.index_of(ids[1]), Some(1));
    assert_eq!(strip.get(ids[1]).map(Tab::id), Some(ids[1]));
}

#[test]
fn a_tab_that_was_moved_is_still_reachable_by_the_id_an_action_captured() {
    // The point of addressing tabs by identity: an action minted before a
    // reorder still names the tab a person was pointing at, not whoever
    // inherited its slot.
    let (mut strip, ids) = strip(3);
    let doomed = ids[2];
    strip.apply(TabAction::Select(doomed));
    strip.apply(TabAction::MoveLeft);
    strip.apply(TabAction::MoveLeft);

    strip.apply(TabAction::Close(doomed));

    assert_eq!(order(&strip), [ids[0], ids[1]]);
}

#[test]
fn a_derived_title_replaces_the_one_the_session_was_created_with() {
    let mut strip = TabStrip::new();
    let id = strip.active_id();
    assert_eq!(strip.get(id).map(Tab::title), Some("agent 1"));

    strip
        .get_mut(id)
        .expect("just created")
        .session_mut()
        .derived_title = Some("port the tab bar".to_owned());

    assert_eq!(strip.get(id).map(Tab::title), Some("port the tab bar"));
}

#[test]
fn the_mru_list_holds_every_open_tab_once_with_the_active_one_at_its_head() {
    // The rule that closing the active tab returns you to the one you were on
    // before rests entirely on this: a duplicate entry, a missing tab or a
    // head that is not the active tab all pick the wrong successor, and none
    // of the three is visible until the moment a tab is closed.
    let (mut strip, ids) = strip(5);

    for action in [
        TabAction::Select(ids[3]),
        TabAction::Select(ids[0]),
        TabAction::MoveRight,
        TabAction::Close(ids[2]),
        TabAction::New,
        TabAction::Select(ids[4]),
        TabAction::MoveLeft,
        TabAction::Close(ids[4]),
        TabAction::Select(ids[2]),
    ] {
        strip.apply(action);

        let mut open = order(&strip);
        let mut listed = strip.mru().to_vec();
        assert_eq!(
            strip.mru().first(),
            Some(&strip.active_id()),
            "after {action:?}"
        );

        open.sort();
        listed.sort();
        assert_eq!(
            listed, open,
            "the MRU list disagrees with the strip after {action:?}"
        );
        assert!(strip.active().is_some(), "after {action:?}");
    }
}
