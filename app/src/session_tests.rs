//! What comes back, and what a file nobody meant cannot do.
//!
//! Every case here runs with no window, no GPU and no shell: a session is a
//! shape, and restoring one is arithmetic over a strip.

use super::*;
use crate::tab::{Direction, TabAction};

/// A strip of one tab, split `panes` ways to the right.
fn split(panes: usize) -> TabStrip {
    let mut strip = TabStrip::new();
    for _ in 1..panes {
        strip.apply(TabAction::Split(Direction::Right));
    }
    strip
}

/// The titles of every pane of every tab, in bar order.
fn titles(strip: &TabStrip) -> Vec<Vec<String>> {
    strip
        .iter()
        .map(|tab| {
            tab.panes()
                .iter()
                .map(|pane| pane.title().to_owned())
                .collect()
        })
        .collect()
}

/// A scratch directory nothing else is using.
fn scratch(name: &str) -> PathBuf {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "crook-session-{name}-{}-{serial}",
        std::process::id()
    ));
    let _ = fs::create_dir_all(&directory);
    directory
}

#[test]
fn test_a_window_of_tabs_and_splits_comes_back_the_shape_it_was() {
    let mut strip = split(3);
    strip.apply(TabAction::New);
    strip.apply(TabAction::Split(Direction::Down));
    let before = titles(&strip);

    let restored = Session::of(&strip, None)
        .restore()
        .expect("there was something to restore");

    assert_eq!(titles(&restored), before);
    assert_eq!(restored.len(), 2);
    assert_eq!(
        restored.iter().next().expect("a first tab").panes().len(),
        3
    );
}

#[test]
fn test_a_rename_comes_back_and_the_name_it_replaced_comes_back_with_it() {
    // Both halves, because they are kept in two different places: a tab's name
    // is the snapshot's own field and a pane's is one beside the title it
    // overrides. The name a tab was *born* with is neither — it is rebuilt on
    // the way in, and without it a tab that came back renamed would have
    // nowhere to go when the rename was taken back.
    let mut strip = split(1);
    let tab = strip.iter().next().expect("a first tab").id();
    let pane = strip
        .iter()
        .next()
        .expect("a first tab")
        .panes()
        .focused_id();
    let born_as = strip.get(tab).expect("the tab is open").name().to_owned();

    strip
        .get_mut(tab)
        .expect("the tab is open")
        .set_name(Some("release work".to_owned()));
    strip
        .pane_mut(pane)
        .expect("the pane is open")
        .session_mut()
        .custom_title = Some("the long build".to_owned());

    let mut restored = Session::of(&strip, None)
        .restore()
        .expect("there was something to restore");
    let back = restored.iter().next().expect("a first tab").id();

    assert_eq!(restored.get(back).expect("the tab").name(), "release work");
    assert_eq!(
        restored
            .get(back)
            .expect("the tab")
            .panes()
            .focused()
            .expect("a focused pane")
            .title(),
        "the long build"
    );

    restored.get_mut(back).expect("the tab").set_name(None);
    assert_eq!(
        restored.get(back).expect("the tab").name(),
        born_as,
        "a rename taken back after a restart had nowhere to go"
    );
}

#[test]
fn test_every_identity_is_minted_fresh() {
    // A restored window has to be indistinguishable from one somebody opened
    // by hand. An id from a previous process means nothing in this one, and
    // one that collided with a live id would make every lookup resolve to the
    // wrong pane.
    let strip = split(2);
    let old: Vec<_> = strip.panes().map(|(_, pane)| pane.id()).collect();

    let restored = Session::of(&strip, None).restore().expect("restored");
    let new: Vec<_> = restored.panes().map(|(_, pane)| pane.id()).collect();

    assert_eq!(old.len(), new.len());
    for id in &new {
        assert!(!old.contains(id), "{id:?} came from the previous process");
    }
}

#[test]
fn test_the_active_tab_and_the_focused_pane_come_back() {
    let mut strip = split(3);
    strip.apply(TabAction::New);
    strip.apply(TabAction::New);

    // The middle tab, and its first pane rather than the one a split focused.
    let middle = strip.iter().nth(1).expect("three tabs").id();
    strip.apply(TabAction::Select(middle));
    let first_of_the_first = strip
        .iter()
        .next()
        .expect("a first tab")
        .panes()
        .iter()
        .next()
        .expect("a first pane")
        .id();
    strip.apply(TabAction::FocusPane(first_of_the_first));

    // Focusing a pane activates its tab, so put the middle one back.
    let middle = strip.iter().nth(1).expect("three tabs").id();
    strip.apply(TabAction::Select(middle));

    let restored = Session::of(&strip, None).restore().expect("restored");

    assert_eq!(
        restored.index_of(restored.active_id()),
        Some(1),
        "the second tab was the active one"
    );
    let first = restored.iter().next().expect("a first tab");
    assert_eq!(
        first.panes().index_of(first.panes().focused_id()),
        Some(0),
        "the first pane of the first tab had the keyboard"
    );
}

#[test]
fn test_a_dragged_divider_survives() {
    let mut strip = split(2);
    let panes: Vec<_> = strip.panes().map(|(_, pane)| pane.id()).collect();
    strip.apply(TabAction::ResizePanes {
        before: panes[0],
        after: panes[1],
        leading: 0.75,
    });

    let restored = Session::of(&strip, None).restore().expect("restored");
    let flexes: Vec<_> = restored.panes().map(|(_, pane)| pane.flex()).collect();

    assert!((flexes[0] - 1.5).abs() < 1e-5, "{flexes:?}");
    assert!((flexes[1] - 0.5).abs() < 1e-5);
}

#[test]
fn test_the_split_axis_survives() {
    let mut strip = TabStrip::new();
    strip.apply(TabAction::Split(Direction::Down));
    assert_eq!(
        strip.active().expect("a tab").panes().axis(),
        SplitAxis::Vertical
    );

    let restored = Session::of(&strip, None).restore().expect("restored");
    assert_eq!(
        restored.active().expect("a tab").panes().axis(),
        SplitAxis::Vertical
    );
}

#[test]
fn test_a_working_directory_that_is_gone_is_not_restored() {
    // A repository moved or deleted between two launches. Starting a shell in
    // a directory that is not there is answered by most shells with `/`, which
    // is a worse answer than the directory Crook was started from.
    let strip = TabStrip::new();
    let mut session = Session::of(&strip, None);
    session.tabs[0].panes[0].working_directory = Some(PathBuf::from("/nowhere/at/all"));

    let restored = session.restore().expect("restored");
    let directory = restored
        .panes()
        .next()
        .map(|(_, pane)| pane.session())
        .and_then(|session| session.working_directory.clone());

    assert_ne!(directory.as_deref(), Some(Path::new("/nowhere/at/all")));
}

#[test]
fn test_a_directory_that_is_still_there_is_restored() {
    let directory = scratch("directory");
    let strip = TabStrip::new();
    let mut session = Session::of(&strip, None);
    session.tabs[0].panes[0].working_directory = Some(directory.clone());

    let restored = session.restore().expect("restored");
    let restored_directory = restored
        .panes()
        .next()
        .map(|(_, pane)| pane.session())
        .and_then(|session| session.working_directory.clone());

    assert_eq!(restored_directory.as_deref(), Some(directory.as_path()));
}

#[test]
fn test_a_file_that_describes_nothing_opens_a_fresh_window() {
    assert!(Session::default().is_empty());
    assert!(Session::default().restore().is_none());

    // A tab with no panes is not a tab. It cannot be represented by the strip
    // and must not become one.
    let session = Session {
        tabs: vec![TabSnapshot::default()],
        ..Session::default()
    };
    assert!(session.is_empty());
    assert!(session.restore().is_none());
}

#[test]
fn test_a_file_nobody_meant_cannot_open_ten_thousand_ptys() {
    let session = Session {
        tabs: (0..MAX_TABS * 4)
            .map(|_| TabSnapshot {
                name: "agent".to_owned(),
                panes: (0..MAX_PANES * 4)
                    .map(|_| PaneSnapshot {
                        title: "agent".to_owned(),
                        ..PaneSnapshot::default()
                    })
                    .collect(),
                ..TabSnapshot::default()
            })
            .collect(),
        ..Session::default()
    };

    let restored = session.restore().expect("restored");
    assert_eq!(restored.len(), MAX_TABS);
    for (_, _) in restored.panes() {}
    assert_eq!(
        restored.active().expect("a tab").panes().len(),
        MAX_PANES,
        "a tab took more panes than the bound allows"
    );
}

#[test]
fn test_an_active_index_past_the_end_selects_the_last_tab_rather_than_nothing() {
    let mut strip = TabStrip::new();
    strip.apply(TabAction::New);
    let mut session = Session::of(&strip, None);
    session.active = 99;

    let restored = session.restore().expect("restored");
    assert_eq!(restored.index_of(restored.active_id()), Some(1));
}

#[test]
fn test_a_flex_a_layout_could_not_use_falls_back_to_an_even_share() {
    // The number comes out of a file. A zero, a negative or a `NaN` is a pane
    // a flex layout gives no space to — a pane nobody can see or get back.
    for written in [0., -1., f32::NAN, f32::INFINITY] {
        let strip = TabStrip::new();
        let mut session = Session::of(&strip, None);
        session.tabs[0].panes[0].flex = written;

        let restored = session.restore().expect("restored");
        let flex = restored
            .panes()
            .next()
            .map(|(_, pane)| pane.flex())
            .expect("a pane");
        assert!(
            flex.is_finite() && flex > 0.,
            "a stored {written} resolved to {flex}"
        );
    }
}

#[test]
fn test_a_session_survives_the_round_trip_through_a_file() {
    let directory = scratch("round-trip");
    let path = directory.join("session.json");

    let mut strip = split(2);
    strip.apply(TabAction::New);
    let session = Session::of(&strip, Some([1280., 800.]));
    session.save_blocking(&path).expect("writable");

    let reread = Session::load(&path);
    assert_eq!(reread, session);
    assert_eq!(reread.window_size(), Some([1280., 800.]));
    assert_eq!(titles(&reread.restore().expect("restored")), titles(&strip));
}

#[test]
fn test_a_file_that_cannot_be_read_is_a_fresh_window_and_a_line_in_the_log() {
    let directory = scratch("unreadable");

    // Missing: the ordinary first run.
    assert!(Session::load(directory.join("nothing.json")).is_empty());

    // Present and not JSON at all.
    let path = directory.join("broken.json");
    fs::write(&path, "{ not json").expect("writable");
    assert!(Session::load(&path).is_empty());

    // JSON, and not the shape this build expects.
    fs::write(&path, "[1, 2, 3]").expect("writable");
    assert!(Session::load(&path).is_empty());

    // JSON of the right shape with a key this build has never heard of, which
    // is what a *newer* Crook's file looks like.
    fs::write(&path, r#"{"tabs": [], "tomorrows_key": 7}"#).expect("writable");
    assert!(Session::load(&path).is_empty());
}

#[test]
fn test_a_window_size_a_person_could_not_see_is_refused() {
    for size in [[0., 0.], [-100., 200.], [f32::NAN, 600.]] {
        let session = Session {
            window: Some(size),
            ..Session::default()
        };
        assert_eq!(session.window_size(), None, "{size:?}");
    }
}

/// The blocks a strip draws, as each group's name and its tabs' names.
fn shape(strip: &TabStrip) -> Vec<(Option<String>, Vec<String>)> {
    strip
        .blocks()
        .into_iter()
        .map(|block| {
            (
                block
                    .group
                    .and_then(|id| strip.group(id))
                    .map(|group| group.name().to_owned()),
                block
                    .tabs
                    .iter()
                    .filter_map(|id| strip.get(*id))
                    .map(|tab| tab.name().to_owned())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn test_a_group_comes_back_with_its_name_its_members_and_its_fold() {
    let mut strip = TabStrip::new();
    strip.apply(TabAction::New);
    let first = strip.iter().next().expect("a first tab").id();
    strip.apply(TabAction::NewInGroupOf(first));
    let group = strip.get(first).and_then(Tab::group).expect("grouped");
    strip.rename_group(group, "crook");
    strip.apply(TabAction::ToggleGroup(group));
    let before = shape(&strip);

    let restored = Session::of(&strip, None)
        .restore()
        .expect("there was something to restore");

    assert_eq!(shape(&restored), before);
    assert_eq!(
        restored.groups().count(),
        1,
        "the group came back once, or not at all"
    );
    assert!(
        restored.groups().next().expect("a group").is_collapsed(),
        "a group folded away came back open"
    );
}

#[test]
fn test_a_file_that_scattered_a_group_gathers_it_again() {
    // A session file is a file a person can edit, and members that are not
    // contiguous are a strip the panel cannot draw. Gathered at the first
    // member's place rather than refused: a window is better than no window.
    let session = Session {
        tabs: vec![
            TabSnapshot {
                name: "one".to_owned(),
                panes: vec![PaneSnapshot::default()],
                group: Some(0),
                ..TabSnapshot::default()
            },
            TabSnapshot {
                name: "two".to_owned(),
                panes: vec![PaneSnapshot::default()],
                group: None,
                ..TabSnapshot::default()
            },
            TabSnapshot {
                name: "three".to_owned(),
                panes: vec![PaneSnapshot::default()],
                group: Some(0),
                ..TabSnapshot::default()
            },
        ],
        groups: vec![GroupSnapshot {
            name: "scattered".to_owned(),
            collapsed: false,
        }],
        ..Session::default()
    };

    let restored = session.restore().expect("three tabs");

    assert_eq!(
        shape(&restored),
        vec![
            (
                Some("scattered".to_owned()),
                vec!["one".to_owned(), "three".to_owned()]
            ),
            (None, vec!["two".to_owned()]),
        ]
    );
}

#[test]
fn test_a_group_naming_no_tab_that_came_back_is_not_created() {
    let session = Session {
        tabs: vec![TabSnapshot {
            name: "alone".to_owned(),
            panes: vec![PaneSnapshot::default()],
            group: Some(9),
            ..TabSnapshot::default()
        }],
        groups: vec![GroupSnapshot {
            name: "empty".to_owned(),
            collapsed: false,
        }],
        ..Session::default()
    };

    let restored = session.restore().expect("one tab");

    assert_eq!(restored.groups().count(), 0);
    assert_eq!(shape(&restored), vec![(None, vec!["alone".to_owned()])]);
}
