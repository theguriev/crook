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
    let session = strip.pane_mut(pane).expect("still open").session_mut();

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

/// Dragging the boundary between two panes, which is arithmetic on weights and
/// needs no window to check.
mod resizing {
    use super::*;

    /// The weights of the active tab's panes, in render order.
    fn flexes(strip: &TabStrip) -> Vec<f32> {
        strip
            .active()
            .expect("there is an active tab")
            .panes()
            .iter()
            .map(Pane::flex)
            .collect()
    }

    /// A strip whose one tab holds `count` panes, and their ids in order.
    fn split(count: usize) -> (TabStrip, Vec<PaneId>) {
        let mut strip = TabStrip::new();
        for _ in 1..count {
            strip.apply(TabAction::Split(Direction::Right));
        }
        let tab = strip.active_id();
        let panes = panes_of(&strip, tab);
        (strip, panes)
    }

    #[test]
    fn a_fresh_split_divides_evenly() {
        let (strip, _) = split(3);
        assert_eq!(flexes(&strip), vec![1., 1., 1.]);
    }

    #[test]
    fn a_drag_moves_weight_between_the_pair_and_nobody_else() {
        // The property that makes a three-way split behave: dragging the first
        // divider must not move the third pane at all.
        let (mut strip, panes) = split(3);

        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::ResizePanes {
                before: panes[0],
                after: panes[1],
                leading: 0.75,
            })
        );

        let flexes = flexes(&strip);
        assert!((flexes[0] - 1.5).abs() < 1e-5, "three quarters of the pair");
        assert!((flexes[1] - 0.5).abs() < 1e-5);
        assert_eq!(flexes[2], 1., "the pane beyond the divider never moved");
        assert!(
            (flexes[0] + flexes[1] - 2.).abs() < 1e-5,
            "the pair keeps the share it had between them"
        );
    }

    #[test]
    fn a_pane_cannot_be_dragged_away_to_nothing() {
        // A divider that could take a pane to zero would put itself on top of
        // its neighbour, and the pane behind it would have no edge left to
        // grab it back by.
        let (mut strip, panes) = split(2);

        strip.apply(TabAction::ResizePanes {
            before: panes[0],
            after: panes[1],
            leading: 0.,
        });
        let squeezed = flexes(&strip);
        assert!(squeezed[0] > 0., "the pane is still on screen");
        assert!(squeezed[1] < 2.);

        strip.apply(TabAction::ResizePanes {
            before: panes[0],
            after: panes[1],
            leading: 1.,
        });
        assert!(flexes(&strip)[1] > 0.);
    }

    #[test]
    fn a_pair_that_is_not_adjacent_is_refused() {
        // An action is a value and can arrive after the panes it names have
        // moved or closed. A divider knows its neighbours; the group checks.
        let (mut strip, panes) = split(3);

        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::ResizePanes {
                before: panes[0],
                after: panes[2],
                leading: 0.5,
            }),
            "there is no divider between the first pane and the third"
        );
        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::ResizePanes {
                before: panes[1],
                after: panes[0],
                leading: 0.5,
            }),
            "the pair is ordered, and this one is back to front"
        );
        assert_eq!(flexes(&strip), vec![1., 1., 1.]);
    }

    #[test]
    fn a_drag_that_changes_nothing_repaints_nothing() {
        // A pointer held still produces a move per frame, and every one of
        // them asks for the ratio it already has.
        let (mut strip, panes) = split(2);
        let resize = TabAction::ResizePanes {
            before: panes[0],
            after: panes[1],
            leading: 0.5,
        };

        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(resize),
            "an even split asked to stay even"
        );
    }

    #[test]
    fn evening_out_is_the_way_back() {
        let (mut strip, panes) = split(3);
        strip.apply(TabAction::ResizePanes {
            before: panes[0],
            after: panes[1],
            leading: 0.9,
        });
        assert_ne!(flexes(&strip), vec![1., 1., 1.]);

        assert_eq!(TabEffect::Changed, strip.apply(TabAction::EvenPanes));
        assert_eq!(flexes(&strip), vec![1., 1., 1.]);

        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::EvenPanes),
            "a split that is already even has not changed"
        );
    }

    #[test]
    fn a_closed_pane_leaves_the_survivors_their_weights() {
        // Three equal panes that lose one leave two weights of 1, and the
        // layout divides what is left between them. A fraction would have had
        // to be renormalised here; a weight does not.
        let (mut strip, panes) = split(3);
        strip.apply(TabAction::ResizePanes {
            before: panes[0],
            after: panes[1],
            leading: 0.75,
        });

        strip.apply(TabAction::ClosePane(panes[2]));

        let flexes = flexes(&strip);
        assert_eq!(flexes.len(), 2);
        assert!((flexes[0] - 1.5).abs() < 1e-5);
        assert!((flexes[1] - 0.5).abs() < 1e-5);
    }
}

/// Walking a split with the keyboard, and sizing it from there.
///
/// A pane group is one vector along one axis, so half of what "left, right, up
/// and down" could mean is a direction the group cannot answer — and the
/// cases here are mostly about that half, because a command that answered it
/// anyway would move the keyboard somewhere nobody pointed and eat the chord
/// on the way.
mod walking {
    use super::*;

    /// A row of `count` panes, and their ids in render order.
    fn row(count: usize) -> (TabStrip, Vec<PaneId>) {
        let mut strip = TabStrip::new();
        for _ in 1..count {
            strip.apply(TabAction::Split(Direction::Right));
        }
        let panes = panes_of(&strip, strip.active_id());
        (strip, panes)
    }

    /// The active tab's pane group.
    fn group(strip: &TabStrip) -> &PaneGroup {
        strip.active().expect("a tab is active").panes()
    }

    fn flexes(strip: &TabStrip) -> Vec<f32> {
        group(strip).iter().map(Pane::flex).collect()
    }

    #[test]
    fn the_neighbour_along_the_axis_is_the_next_pane_either_way() {
        let (mut strip, panes) = row(3);
        strip.apply(TabAction::FocusPane(panes[1]));

        assert_eq!(group(&strip).neighbour(Direction::Left), Some(panes[0]));
        assert_eq!(group(&strip).neighbour(Direction::Right), Some(panes[2]));
    }

    #[test]
    fn a_direction_across_the_axis_names_no_pane() {
        // The whole reason `neighbour` is an `Option`. A row of panes has
        // nothing above or below it, and `Workspace::command` turns this
        // `None` into a keystroke that reaches the shell.
        let (mut strip, panes) = row(3);
        strip.apply(TabAction::FocusPane(panes[1]));

        assert_eq!(group(&strip).neighbour(Direction::Up), None);
        assert_eq!(group(&strip).neighbour(Direction::Down), None);
    }

    #[test]
    fn the_ends_of_a_split_have_no_neighbour_beyond_them() {
        // A split focuses the pane it just made, so the row opens with the
        // keyboard on the last of them.
        let (mut strip, panes) = row(2);
        assert_eq!(group(&strip).focused_id(), panes[1]);
        assert_eq!(group(&strip).neighbour(Direction::Right), None);
        assert_eq!(group(&strip).neighbour(Direction::Left), Some(panes[0]));

        strip.apply(TabAction::FocusPane(panes[0]));
        assert_eq!(group(&strip).neighbour(Direction::Left), None);
        assert_eq!(group(&strip).neighbour(Direction::Right), Some(panes[1]));
    }

    #[test]
    fn a_tab_that_was_never_split_has_no_neighbour_in_any_direction() {
        let (strip, _) = row(1);

        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            assert_eq!(group(&strip).neighbour(direction), None, "{direction:?}");
        }
        assert_eq!(group(&strip).cycled(1), None);
        assert_eq!(group(&strip).cycled(-1), None);
    }

    #[test]
    fn cycling_wraps_where_the_directions_stop() {
        // The difference between the two gestures, and the reason both exist:
        // "the next one" comes back round, and "the one to my right" does not.
        let (mut strip, panes) = row(3);
        strip.apply(TabAction::FocusPane(panes[2]));

        assert_eq!(group(&strip).cycled(1), Some(panes[0]));
        assert_eq!(group(&strip).neighbour(Direction::Right), None);

        strip.apply(TabAction::FocusPane(panes[0]));
        assert_eq!(group(&strip).cycled(-1), Some(panes[2]));
    }

    #[test]
    fn a_column_answers_up_and_down_instead() {
        let mut strip = TabStrip::new();
        strip.apply(TabAction::Split(Direction::Down));
        let panes = panes_of(&strip, strip.active_id());
        strip.apply(TabAction::FocusPane(panes[0]));

        assert_eq!(group(&strip).neighbour(Direction::Down), Some(panes[1]));
        assert_eq!(group(&strip).neighbour(Direction::Right), None);
    }

    #[test]
    fn growing_takes_from_the_pane_after_the_focused_one() {
        let (mut strip, panes) = row(3);
        strip.apply(TabAction::FocusPane(panes[0]));

        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::NudgePane { grow: true })
        );

        let flexes = flexes(&strip);
        assert!(flexes[0] > 1., "the focused pane took room");
        assert!(flexes[1] < 1., "its neighbour gave it up");
        assert_eq!(flexes[2], 1., "the pane beyond the divider never moved");
        assert!(
            (flexes[0] + flexes[1] - 2.).abs() < 1e-5,
            "the pair keeps the share it had between them"
        );
    }

    #[test]
    fn the_last_pane_grows_against_the_divider_before_it() {
        // The asymmetry `nudge` documents: the last pane has no divider after
        // it, and a version that only ever looked forwards would leave it the
        // one pane in the split that cannot be made wider.
        let (mut strip, panes) = row(2);
        strip.apply(TabAction::FocusPane(panes[1]));

        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::NudgePane { grow: true })
        );

        let flexes = flexes(&strip);
        assert!(flexes[1] > 1., "the focused pane took room");
        assert!(flexes[0] < 1.);
    }

    #[test]
    fn shrinking_is_growing_the_other_way_round() {
        let (mut strip, panes) = row(2);
        strip.apply(TabAction::FocusPane(panes[0]));
        strip.apply(TabAction::NudgePane { grow: true });
        let grown = flexes(&strip);

        strip.apply(TabAction::NudgePane { grow: false });

        let back = flexes(&strip);
        assert!(back[0] < grown[0]);
        assert!((back[0] - 1.).abs() < 1e-5, "one step each way is no step");
    }

    #[test]
    fn a_tab_with_one_pane_cannot_be_resized() {
        let (mut strip, _) = row(1);

        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::NudgePane { grow: true })
        );
    }

    #[test]
    fn a_pane_cannot_be_nudged_past_the_minimum() {
        // `resize` clamps at MIN_FLEX, and the step is a share of the pair, so
        // a chord held down settles rather than driving a pane to nothing.
        let (mut strip, panes) = row(2);
        strip.apply(TabAction::FocusPane(panes[0]));

        for _ in 0..200 {
            strip.apply(TabAction::NudgePane { grow: false });
        }

        let flexes = flexes(&strip);
        // The exact floor is `pane::MIN_FLEX`, which is private; what has to
        // hold here is that neither pane was driven to nothing.
        assert!(flexes[0] > 0.05, "{flexes:?}");
        assert!(flexes[1] > 0.05, "{flexes:?}");
    }

    #[test]
    fn the_pane_at_a_position_is_the_one_the_panel_draws_there() {
        let (strip, panes) = row(3);

        assert_eq!(group(&strip).at(0), Some(panes[0]));
        assert_eq!(group(&strip).at(2), Some(panes[2]));
        assert_eq!(group(&strip).at(3), None, "past the end names nothing");
    }
}

/// The strip's groups, as the panel would draw them: one entry per block,
/// naming its group's tabs.
mod groups {
    use super::*;

    /// A strip of `count` tabs where the first two are one group, with the
    /// group's id.
    fn grouped(count: usize) -> (TabStrip, Vec<TabId>, GroupId) {
        let (mut strip, ids) = strip(count);
        strip.apply(TabAction::NewInGroupOf(ids[0]));
        let group = strip
            .get(ids[0])
            .expect("the anchor is open")
            .group()
            .expect("the anchor was put in a group");
        let ids = order(&strip);
        (strip, ids, group)
    }

    /// Every block, as its group's name (or `-`) and the positions of its tabs.
    fn shape(strip: &TabStrip) -> Vec<(Option<GroupId>, usize)> {
        strip
            .blocks()
            .into_iter()
            .map(|block| (block.group, block.tabs.len()))
            .collect()
    }

    #[test]
    fn a_worktree_opens_a_tab_in_the_group_rather_than_a_pane_in_the_tab() {
        // The whole feature in one case: the second checkout is a tab beside
        // the first, under one heading — not a second pane inside it.
        let (mut strip, ids) = strip(1);

        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::NewInGroupOf(ids[0]))
        );

        assert_eq!(strip.len(), 2, "a tab, not a split");
        assert_eq!(
            strip.get(ids[0]).expect("open").panes().len(),
            1,
            "the tab it was opened from was not split"
        );
        let group = strip.get(ids[0]).expect("open").group().expect("grouped");
        assert_eq!(
            strip.members(group).map(Tab::id).collect::<Vec<_>>(),
            order(&strip),
            "both tabs are in it, in strip order"
        );
        assert_eq!(strip.active_id(), order(&strip)[1], "and it is selected");
    }

    #[test]
    fn the_group_is_named_after_the_tab_it_was_made_around() {
        let (strip, ids, group) = grouped(1);

        assert_eq!(
            strip.group(group).map(TabGroup::name),
            strip.get(ids[0]).map(Tab::name)
        );
    }

    #[test]
    fn a_second_worktree_joins_the_group_that_is_already_there() {
        let (mut strip, ids, group) = grouped(1);

        strip.apply(TabAction::NewInGroupOf(ids[0]));

        assert_eq!(strip.groups().count(), 1, "one group, not two");
        assert_eq!(strip.members(group).count(), 3);
        assert_eq!(shape(&strip), vec![(Some(group), 3)]);
    }

    #[test]
    fn a_new_tab_lands_past_the_group_rather_than_inside_it() {
        // `New` inserts after the active tab, and the active tab is the
        // group's newest member. Landing there would put an ungrouped tab
        // between two members, which is the one arrangement the panel cannot
        // draw.
        let (mut strip, _, group) = grouped(1);

        strip.apply(TabAction::New);

        assert_eq!(shape(&strip), vec![(Some(group), 2), (None, 1)]);
        assert_eq!(strip.get(strip.active_id()).and_then(Tab::group), None);
    }

    #[test]
    fn a_tab_dragged_onto_a_group_joins_it() {
        let (mut strip, ids, group) = grouped(3);
        let outsider = *ids.last().expect("three tabs");

        strip.apply(TabAction::MoveTab {
            tab: outsider,
            group: Some(group),
            before: None,
        });

        assert_eq!(shape(&strip), vec![(Some(group), 3), (None, 1)]);
        assert_eq!(
            strip.members(group).map(Tab::id).last(),
            Some(outsider),
            "at the end of the group, which is where it was dropped"
        );
    }

    #[test]
    fn a_tab_dragged_out_of_a_group_leaves_it() {
        let (mut strip, ids, group) = grouped(2);
        let member = ids[1];

        strip.apply(TabAction::MoveTab {
            tab: member,
            group: None,
            before: None,
        });

        assert_eq!(strip.get(member).and_then(Tab::group), None);
        assert_eq!(strip.members(group).count(), 1);
        assert_eq!(order(&strip).last(), Some(&member));
    }

    #[test]
    fn a_group_that_loses_its_last_member_is_gone() {
        let (mut strip, ids, group) = grouped(1);

        for member in [ids[0], ids[1]] {
            strip.apply(TabAction::MoveTab {
                tab: member,
                group: None,
                before: None,
            });
        }

        assert!(strip.group(group).is_none());
        assert_eq!(strip.groups().count(), 0);
    }

    #[test]
    fn closing_the_last_member_takes_the_group_with_it() {
        let (mut strip, ids, group) = grouped(2);
        let members: Vec<TabId> = strip.members(group).map(Tab::id).collect();

        for member in members {
            strip.apply(TabAction::Close(member));
        }

        assert_eq!(strip.groups().count(), 0);
        assert_eq!(order(&strip), vec![ids[2]]);
    }

    #[test]
    fn a_drop_between_two_members_of_a_group_it_is_not_joining_lands_outside_it() {
        // The clamp: a pointer in the gap between two members says two things
        // that disagree, and the group wins. Anything else splits the run.
        let (mut strip, ids, group) = grouped(3);
        let outsider = *ids.last().expect("three tabs");
        let second_member = strip.members(group).map(Tab::id).nth(1).expect("two");

        strip.apply(TabAction::MoveTab {
            tab: outsider,
            group: None,
            before: Some(second_member),
        });

        assert_eq!(
            shape(&strip),
            vec![(None, 1), (Some(group), 2), (None, 1)],
            "above the group, not inside it"
        );
    }

    #[test]
    fn a_drop_into_a_group_lands_inside_it_however_far_the_pointer_got() {
        // The other half of the clamp: the group is believed and the gap is
        // moved, so a target computed from coarse geometry cannot break a run.
        let (mut strip, ids, group) = grouped(3);
        let outsider = *ids.last().expect("three tabs");

        strip.apply(TabAction::MoveTab {
            tab: outsider,
            group: Some(group),
            before: Some(ids[2]),
        });

        assert_eq!(shape(&strip), vec![(Some(group), 3), (None, 1)]);
    }

    #[test]
    fn a_group_is_dragged_as_one_block() {
        let (mut strip, ids, group) = grouped(3);
        let last = *ids.last().expect("three tabs");
        let members: Vec<TabId> = strip.members(group).map(Tab::id).collect();

        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::MoveGroup {
                group,
                before: None
            })
        );

        assert_eq!(shape(&strip), vec![(None, 1), (None, 1), (Some(group), 2)]);
        assert_eq!(
            order(&strip)[0],
            ids[2],
            "the tabs it passed kept their order"
        );
        assert_eq!(order(&strip)[1], last);
        assert_eq!(
            strip.members(group).map(Tab::id).collect::<Vec<_>>(),
            members,
            "and the block kept its own"
        );
    }

    #[test]
    fn a_group_dropped_where_it_already_is_has_not_moved() {
        let (mut strip, ids, group) = grouped(3);

        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::MoveGroup {
                group,
                before: Some(ids[2])
            })
        );
    }

    #[test]
    fn a_group_dropped_on_another_group_lands_above_the_whole_of_it() {
        let (mut strip, ids, first) = grouped(2);
        let last = *ids.last().expect("two tabs");
        strip.apply(TabAction::NewInGroupOf(last));
        let second = strip.get(last).and_then(Tab::group).expect("grouped");
        let second_member = strip.members(second).map(Tab::id).nth(1).expect("two");

        strip.apply(TabAction::MoveGroup {
            group: first,
            before: Some(second_member),
        });

        assert_eq!(shape(&strip), vec![(Some(first), 2), (Some(second), 2)]);
    }

    #[test]
    fn folding_a_group_away_leaves_its_members_in_the_strip() {
        let (mut strip, _, group) = grouped(1);

        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::ToggleGroup(group))
        );

        assert!(strip.group(group).expect("open").is_collapsed());
        assert_eq!(strip.len(), 2, "folded away is not closed");

        strip.apply(TabAction::ToggleGroup(group));
        assert!(!strip.group(group).expect("open").is_collapsed());
    }

    #[test]
    fn closing_a_group_closes_its_tabs() {
        let (mut strip, ids, group) = grouped(2);
        let survivor = *ids.last().expect("two tabs");

        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::CloseGroup(group))
        );

        assert_eq!(order(&strip), vec![survivor]);
        assert_eq!(strip.groups().count(), 0);
    }

    #[test]
    fn closing_the_only_group_there_is_closes_the_window() {
        // The strip refuses to empty itself, and it says so once rather than
        // closing tabs until `close` refuses and leaves half a group behind.
        let (mut strip, _, group) = grouped(1);

        assert_eq!(
            TabEffect::CloseWindow,
            strip.apply(TabAction::CloseGroup(group))
        );
        assert_eq!(strip.len(), 2, "nothing was closed on the way out");
    }

    #[test]
    fn moving_the_active_tab_past_the_end_of_its_group_takes_it_out() {
        let (mut strip, ids, group) = grouped(2);
        let member = strip.members(group).map(Tab::id).nth(1).expect("two");
        strip.apply(TabAction::Select(member));

        strip.apply(TabAction::MoveRight);

        assert_eq!(strip.get(member).and_then(Tab::group), None);
        assert_eq!(shape(&strip), vec![(Some(group), 1), (None, 1), (None, 1)]);
        let _ = ids;
    }

    #[test]
    fn an_action_naming_a_group_that_is_gone_changes_nothing() {
        let (mut strip, ids, group) = grouped(1);
        for member in strip.members(group).map(Tab::id).collect::<Vec<_>>() {
            strip.apply(TabAction::MoveTab {
                tab: member,
                group: None,
                before: None,
            });
        }

        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::MoveTab {
                tab: ids[0],
                group: Some(group),
                before: None
            })
        );
        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::ToggleGroup(group))
        );
        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::CloseGroup(group))
        );
        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::MoveGroup {
                group,
                before: None
            })
        );
    }
}

#[test]
fn pinning_moves_a_tab_to_the_front_of_its_own_block() {
    // Not to the front of the *list*, which is Warp's rule and the one Crook
    // cannot have: a group is a contiguous block that says two checkouts are
    // one piece of work, and pinning that lifted a member out of the middle
    // would be pinning that takes a group apart.
    let mut strip = TabStrip::new();
    strip.apply(TabAction::New);
    strip.apply(TabAction::New);
    let ids: Vec<TabId> = strip.iter().map(Tab::id).collect();
    strip.apply(TabAction::NewInGroupOf(ids[1]));
    let group = strip
        .get(ids[1])
        .and_then(Tab::group)
        .expect("the two of them made a group");
    let members: Vec<TabId> = strip.members(group).map(Tab::id).collect();
    assert_eq!(members.len(), 2);

    strip.apply(TabAction::TogglePin(members[1]));

    assert_eq!(
        strip.members(group).map(Tab::id).collect::<Vec<_>>(),
        [members[1], members[0]],
        "the pinned member did not come to the front of its group"
    );
    assert!(
        strip.get(members[1]).is_some_and(Tab::is_pinned),
        "it did not come back pinned"
    );
    assert_eq!(strip.len(), 4, "pinning lost or gained a tab");
}

#[test]
fn an_unpinned_tab_cannot_be_dropped_above_a_pinned_one() {
    // The clamp, which is pinning's whole enforcement: a drop is a pointer
    // position, and a pointer that stopped halfway up a block of pinned rows
    // is not somebody asking to unpin anything.
    let mut strip = TabStrip::new();
    strip.apply(TabAction::New);
    strip.apply(TabAction::New);
    let ids: Vec<TabId> = strip.iter().map(Tab::id).collect();

    strip.apply(TabAction::TogglePin(ids[0]));
    let pinned = ids[0];
    let last = ids[2];

    strip.apply(TabAction::MoveTab {
        tab: last,
        group: None,
        before: Some(pinned),
    });

    assert_eq!(
        strip.iter().next().map(Tab::id),
        Some(pinned),
        "an unpinned tab was dropped above the pinned one"
    );
}

#[test]
fn unpinning_leaves_a_tab_first_among_the_ones_that_are_not_pinned() {
    // The shortest move that satisfies the rule, rather than back where it
    // came from: nothing remembers where it came from, and a tab that jumped
    // to the bottom of the list on being unpinned would be a gesture nobody
    // would use twice.
    let mut strip = TabStrip::new();
    strip.apply(TabAction::New);
    strip.apply(TabAction::New);
    let ids: Vec<TabId> = strip.iter().map(Tab::id).collect();

    strip.apply(TabAction::TogglePin(ids[0]));
    strip.apply(TabAction::TogglePin(ids[1]));
    assert_eq!(
        strip.iter().map(Tab::id).collect::<Vec<_>>(),
        [ids[0], ids[1], ids[2]],
        "two pins did not leave the two of them at the front, in order"
    );

    strip.apply(TabAction::TogglePin(ids[0]));

    assert_eq!(
        strip.iter().map(Tab::id).collect::<Vec<_>>(),
        [ids[1], ids[0], ids[2]],
        "the unpinned tab did not land first among the unpinned"
    );
    assert!(!strip.get(ids[0]).is_some_and(Tab::is_pinned));
}

#[test]
fn a_colour_is_a_name_a_theme_resolves_rather_than_a_number() {
    // Both directions, because the file carries the name: a colour written by
    // one build and read by another has to survive the enum being reordered,
    // and a name nothing matches is a tab with no colour rather than a refusal.
    for color in TabColor::ALL {
        assert_eq!(TabColor::named(color.name()), Some(color));
    }
    assert_eq!(TabColor::named("chartreuse"), None);
}
