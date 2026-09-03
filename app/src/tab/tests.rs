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

/// The pane a tab is currently showing.
fn focused_pane(strip: &TabStrip, tab: TabId) -> PaneId {
    strip
        .get(tab)
        .expect("the tab is open")
        .panes()
        .focused_id()
}

/// Every pane a tab holds, in render order.
fn panes_of(strip: &TabStrip, tab: TabId) -> Vec<PaneId> {
    strip
        .get(tab)
        .expect("the tab is open")
        .panes()
        .iter()
        .map(Pane::id)
        .collect()
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
fn reporting_progress_into_a_session_cannot_touch_its_panes_identity() {
    // `pane_mut` is the documented agent-progress path, so whatever it hands
    // out is reachable from ordinary code. What it must not hand out is a
    // settable `id`: two panes sharing one makes every lookup resolve a close
    // to the wrong session, and a session carrying a second id of its own is
    // one assignment away from disagreeing with its pane's.
    let (mut strip, ids) = strip(3);
    let pane = focused_pane(&strip, ids[1]);
    let session = strip
        .pane_mut(pane)
        .expect("still open")
        .session_mut()
        .expect("an agent pane");

    session.title = "renamed".to_owned();
    session.status = AgentStatus::Failed;

    assert_eq!(order(&strip), ids);
    assert_eq!(strip.index_of(ids[1]), Some(1));
    assert_eq!(strip.tab_of(pane), Some(ids[1]));
    assert_eq!(strip.pane(pane).map(Pane::id), Some(pane));
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

    let pane = focused_pane(&strip, id);
    strip
        .pane_mut(pane)
        .expect("just created")
        .session_mut()
        .expect("an agent pane")
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

#[test]
fn a_split_tab_draws_a_row_per_pane_and_still_a_single_row_per_tab() {
    // The whole of "View as: Panes | Tabs", and the reason a tab had to be
    // able to hold more than one session before the control could mean
    // anything: with one pane per tab both modes return the same rows.
    let (mut strip, ids) = strip(2);
    strip.apply(TabAction::Select(ids[0]));

    assert_eq!(
        strip.rows(Granularity::Panes),
        strip.rows(Granularity::Tabs),
        "before any split the two views are the same bar"
    );

    strip.apply(TabAction::Split(Direction::Right));

    assert_eq!(strip.rows(Granularity::Panes).len(), 3);
    assert_eq!(strip.rows(Granularity::Tabs).len(), 2);
    assert_eq!(
        strip.rows(Granularity::Panes),
        [
            (ids[0], panes_of(&strip, ids[0])[0]),
            (ids[0], panes_of(&strip, ids[0])[1]),
            (ids[1], focused_pane(&strip, ids[1])),
        ],
        "rows come out in bar order, with a tab's panes in render order"
    );
}

#[test]
fn the_single_row_of_a_tab_names_whichever_pane_is_focused() {
    // In `Tabs` view the row is a *pane*, not a summary of a tab, so it has to
    // follow focus. A row that stayed on the first pane would be Warp's
    // flagged-off `Summary` mode wearing the default mode's name.
    let (mut strip, ids) = strip(2);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Split(Direction::Right));

    let panes = panes_of(&strip, ids[0]);
    assert_eq!(strip.rows(Granularity::Tabs)[0], (ids[0], panes[1]));

    strip.apply(TabAction::FocusPane(panes[0]));

    assert_eq!(strip.rows(Granularity::Tabs)[0], (ids[0], panes[0]));
}

#[test]
fn exactly_one_row_in_the_whole_bar_is_the_selected_one() {
    // Warp's `is_selected = is_active_tab && is_focused`. Ported as two
    // independent highlights — an active-tab tint on every row of the active
    // tab, plus a focused-pane marker — it gives a bar where three rows look
    // chosen.
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Split(Direction::Right));
    strip.apply(TabAction::Split(Direction::Down));
    strip.apply(TabAction::Select(ids[1]));
    strip.apply(TabAction::Split(Direction::Right));

    for granularity in [Granularity::Panes, Granularity::Tabs] {
        let selected: Vec<(TabId, PaneId)> = strip
            .rows(granularity)
            .into_iter()
            .filter(|(tab, pane)| {
                strip.is_active(*tab)
                    && strip
                        .get(*tab)
                        .is_some_and(|open| open.panes().is_focused(*pane))
            })
            .collect();

        assert_eq!(
            selected,
            [(ids[1], focused_pane(&strip, ids[1]))],
            "{granularity:?} does not have exactly one selected row"
        );
    }
}

#[test]
fn closing_a_tabs_last_pane_closes_the_tab_and_the_last_of_those_takes_the_window() {
    // The rule the group and the strip share, stated once each: neither ever
    // empties itself, so a close runs out through the tab and then out through
    // the window.
    let (mut strip, ids) = strip(2);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Split(Direction::Right));
    let panes = panes_of(&strip, ids[0]);

    assert_eq!(
        strip.apply(TabAction::ClosePane(panes[1])),
        TabEffect::Changed
    );
    assert_eq!(order(&strip), ids, "closing a pane closed its tab");

    assert_eq!(
        strip.apply(TabAction::ClosePane(panes[0])),
        TabEffect::Changed
    );
    assert_eq!(
        order(&strip),
        [ids[1]],
        "the tab did not go with its last pane"
    );

    let last = focused_pane(&strip, ids[1]);
    assert_eq!(
        strip.apply(TabAction::ClosePane(last)),
        TabEffect::CloseWindow
    );
    assert_eq!(strip.len(), 1, "the strip emptied itself");
    assert_eq!(panes_of(&strip, ids[1]), [last], "the group emptied itself");
}

#[test]
fn a_group_that_collapses_back_to_one_pane_is_a_group_that_never_split() {
    // Warp's `BranchRemoveResult::Collapse`: two panes down to one leaves a
    // bare leaf, with no divider and no active-pane indicator. Here it falls
    // out of `is_split` being derived from the count rather than stored — the
    // flag Warp has to recompute on every add, close, move, hide and reveal.
    let (mut strip, ids) = strip(1);
    let tab = ids[0];
    assert!(!strip.get(tab).expect("open").panes().is_split());

    strip.apply(TabAction::Split(Direction::Down));
    let panes = panes_of(&strip, tab);
    assert!(strip.get(tab).expect("open").panes().is_split());

    strip.apply(TabAction::ClosePane(panes[1]));

    let group = strip.get(tab).expect("open").panes();
    assert_eq!(group.len(), 1);
    assert!(!group.is_split());
    assert_eq!(group.focused_id(), panes[0]);
    assert_eq!(group.mru(), [panes[0]]);
}

#[test]
fn focusing_a_pane_in_another_tab_brings_that_tab_forward_with_it() {
    // What a click on any row of the bar does. The pane is focused first and
    // its tab activated second — Warp's ordering, because activating the tab
    // first re-focuses whichever pane already held input focus.
    let (mut strip, ids) = strip(2);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Split(Direction::Right));
    let panes = panes_of(&strip, ids[0]);
    strip.apply(TabAction::Select(ids[1]));

    assert_eq!(
        strip.apply(TabAction::FocusPane(panes[0])),
        TabEffect::Changed
    );

    assert!(strip.is_active(ids[0]));
    assert_eq!(focused_pane(&strip, ids[0]), panes[0]);
}

#[test]
fn an_action_naming_a_pane_that_is_gone_changes_nothing() {
    let (mut strip, ids) = strip(2);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Split(Direction::Right));
    let panes = panes_of(&strip, ids[0]);
    strip.apply(TabAction::ClosePane(panes[1]));

    assert_eq!(
        strip.apply(TabAction::ClosePane(panes[1])),
        TabEffect::Unchanged,
        "a second close of the same pane, as a stale click would send"
    );
    assert_eq!(
        strip.apply(TabAction::FocusPane(panes[1])),
        TabEffect::Unchanged
    );
    assert_eq!(
        strip.apply(TabAction::FocusPane(panes[0])),
        TabEffect::Unchanged,
        "focusing the pane that is already focused, in the tab already active"
    );
    assert_eq!(strip.tab_of(panes[1]), None);
}

#[test]
fn a_split_across_the_groups_axis_appends_along_it_rather_than_nesting() {
    // A flat vector cannot hold Warp's perpendicular branch, so it does not
    // pretend to: the first split fixes the axis, and a later direction across
    // it keeps only its before-or-after sense.
    let (mut strip, ids) = strip(1);
    strip.apply(TabAction::Split(Direction::Down));
    assert_eq!(
        strip.get(ids[0]).expect("open").panes().axis(),
        SplitAxis::Vertical
    );

    strip.apply(TabAction::Split(Direction::Left));

    let group = strip.get(ids[0]).expect("open").panes();
    assert_eq!(
        group.axis(),
        SplitAxis::Vertical,
        "the axis moved under a pane"
    );
    assert_eq!(group.len(), 3);
    assert_eq!(
        group.index_of(group.focused_id()),
        Some(1),
        "a leading direction still inserts before the pane it was split off"
    );
}

#[test]
fn every_session_a_window_opens_gets_a_name_no_other_session_has_had() {
    // A pane and a tab are the same kind of row in `Panes` view, so a name
    // that repeats across them is exactly as ambiguous as one that repeats
    // between two tabs — in the bar, and in the body panel's heading.
    let (mut strip, ids) = strip(2);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Split(Direction::Right));
    strip.apply(TabAction::New);
    strip.apply(TabAction::Split(Direction::Down));

    let names: Vec<&str> = strip.panes().map(|(_, pane)| pane.title()).collect();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();

    assert_eq!(
        unique.len(),
        names.len(),
        "two sessions share a name: {names:?}"
    );
    assert_eq!(names.len(), 5);
}

#[test]
fn opening_the_settings_puts_them_in_a_tab_after_the_active_one() {
    // Where a new tab goes, because it *is* a new tab: after the one somebody
    // was looking at when they asked for it.
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[0]));

    assert_eq!(TabEffect::Changed, strip.apply(TabAction::OpenSettings));

    let order = order(&strip);
    assert_eq!(order.len(), 4);
    assert_eq!(order[0], ids[0]);
    let settings = order[1];
    assert_eq!(strip.active_id(), settings, "the new tab was not selected");
    assert_eq!(
        strip.get(settings).map(Tab::title),
        Some(SETTINGS_TITLE),
        "the tab does not name itself"
    );
    assert_eq!(
        strip.get(settings).and_then(Tab::status),
        None,
        "the settings pane reported an agent status"
    );
}

#[test]
fn there_is_never_more_than_one_settings_pane_in_a_window() {
    // Warp keeps at most one per window and navigates to it; a second press of
    // `cmd/ctrl-,` must bring the page forward rather than open another copy
    // of it, and a second copy would also be a second thing writing the same
    // settings file.
    let (mut strip, ids) = strip(2);
    strip.apply(TabAction::OpenSettings);
    let settings = strip.active_id();
    let (settings_tab, settings_pane) = strip.settings_pane().expect("just opened");
    assert_eq!(settings_tab, settings);

    strip.apply(TabAction::Select(ids[0]));
    assert_eq!(TabEffect::Changed, strip.apply(TabAction::OpenSettings));

    assert_eq!(order(&strip).len(), 3, "a second settings tab was opened");
    assert_eq!(strip.active_id(), settings);
    assert_eq!(
        strip.settings_pane(),
        Some((settings_tab, settings_pane)),
        "the pane holding the page was replaced"
    );

    // And asking for it while it is already in front changes nothing at all,
    // which is what keeps a held-down binding off the GPU.
    assert_eq!(TabEffect::Unchanged, strip.apply(TabAction::OpenSettings));
}

#[test]
fn a_settings_tab_does_not_consume_an_agent_name() {
    // The counter names agent sessions. Opening settings between two new tabs
    // must not skip a number, or the strip reads as though a session had been
    // opened and closed.
    let mut strip = TabStrip::new();
    strip.apply(TabAction::OpenSettings);
    strip.apply(TabAction::New);

    let names: Vec<&str> = strip.iter().map(Tab::name).collect();
    assert_eq!(names, ["agent 1", SETTINGS_TITLE, "agent 2"]);
}

#[test]
fn closing_the_settings_pane_of_a_split_tab_leaves_the_session_behind() {
    // A settings pane is a pane: it can be split next to a session, and
    // closing it is closing one pane of a tab rather than closing the tab.
    let mut strip = TabStrip::new();
    strip.apply(TabAction::OpenSettings);
    let tab = strip.active_id();
    strip.apply(TabAction::Split(Direction::Right));
    let (_, settings_pane) = strip.settings_pane().expect("still open");

    assert_eq!(panes_of(&strip, tab).len(), 2);
    assert_eq!(
        TabEffect::Changed,
        strip.apply(TabAction::ClosePane(settings_pane))
    );

    assert_eq!(strip.settings_pane(), None);
    assert_eq!(
        panes_of(&strip, tab).len(),
        1,
        "the tab should have kept the session it was split with"
    );
}
