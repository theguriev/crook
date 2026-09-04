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
