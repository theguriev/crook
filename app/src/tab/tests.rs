//! Executable specification for the strip.
//!
//! Every case here is one that a tabbed terminal gets wrong at least once:
//! closing on either side of the selection, closing the selection itself,
//! closing the last one, and moving a tab that is already at the end it was
//! asked to move towards.

use super::*;
use crate::git::Head;

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
fn a_new_tab_opens_where_the_focused_pane_is_working() {
    let mut strip = TabStrip::new();
    let pane = strip.focused_pane_id().expect("a pane has the keyboard");
    let here = PathBuf::from("/somewhere/the/shell/went");
    strip
        .pane_mut(pane)
        .expect("the pane is open")
        .session_mut()
        .working_directory = Some(here.clone());

    strip.apply(TabAction::New);

    let opened = focused_pane(&strip, strip.active_id());
    assert_ne!(opened, pane);
    let session = strip.pane(opened).expect("the new pane is open").session();
    assert_eq!(session.working_directory, Some(here));
}

/// Says the shell in a tab's focused pane has gone to `directory`, and
/// answers it.
fn focused_pane_went_to(strip: &mut TabStrip, tab: TabId, directory: &str) -> PathBuf {
    let directory = PathBuf::from(directory);
    let pane = focused_pane(strip, tab);
    strip
        .pane_mut(pane)
        .expect("the pane is open")
        .session_mut()
        .working_directory = Some(directory.clone());
    directory
}

/// Where a pane is working, as its row and its pty will read it.
fn directory_of(strip: &TabStrip, pane: PaneId) -> Option<PathBuf> {
    strip
        .pane(pane)
        .expect("the pane is open")
        .session()
        .working_directory
        .clone()
}

#[test]
fn a_split_opens_where_the_focused_pane_is_working() {
    let mut strip = TabStrip::new();
    let tab = strip.active_id();
    let pane = focused_pane(&strip, tab);
    let here = focused_pane_went_to(&mut strip, tab, "/somewhere/the/shell/went");

    strip.apply(TabAction::Split(Direction::Right));

    let opened = focused_pane(&strip, tab);
    assert_ne!(opened, pane);
    assert_eq!(directory_of(&strip, opened), Some(here));
}

/// The far side of an `ssh`, in a directory this machine has too.
fn on_buildbox() -> Remote {
    Remote {
        host: "buildbox.internal.example.com".to_owned(),
        directory: PathBuf::from("/home/eugen/work/app"),
    }
}

#[test]
fn a_tab_or_a_split_from_a_pane_on_another_machine_opens_where_a_new_tab_would() {
    // This machine may well have a directory at the path the far side
    // reported, and it is not the one the shell is in. A shell started here
    // starts where one opened from nowhere does.
    let mut strip = TabStrip::new();
    let tab = strip.active_id();
    let pane = focused_pane(&strip, tab);
    let fresh = AgentSession::new("fresh").working_directory;
    strip
        .pane_mut(pane)
        .expect("the pane is open")
        .session_mut()
        .set_remote(on_buildbox());

    strip.apply(TabAction::New);
    let opened = focused_pane(&strip, strip.active_id());
    assert_eq!(directory_of(&strip, opened), fresh);

    strip.apply(TabAction::Select(tab));
    strip.apply(TabAction::Split(Direction::Right));
    let split = focused_pane(&strip, tab);
    assert_ne!(split, pane);
    assert_eq!(directory_of(&strip, split), fresh);
}

#[test]
fn a_shell_on_another_machine_leaves_its_pane_no_directory_here_until_it_comes_back() {
    let mut session = AgentSession::new("agent 1");
    let path = PathBuf::from("/home/eugen/work/app");
    let home = Some(Path::new("/home/eugen"));
    session.set_local_directory(path.clone());
    assert_eq!(session.shown_path(home).as_deref(), Some("~/work/app"));

    session.set_remote(on_buildbox());
    assert_eq!(session.working_directory, None);
    assert_eq!(session.shown_directory(), Some(path.as_path()));
    assert_eq!(
        session.shown_path(home).as_deref(),
        Some("/home/eugen/work/app"),
        "this machine's home says nothing of where the far side keeps its own"
    );
    assert_eq!(session.directory_label(home).as_deref(), Some("app"));

    session.set_local_directory(path.clone());
    assert_eq!(session.remote, None);
    assert_eq!(session.working_directory, Some(path));
}

mod remote_hosts {
    use super::super::remote::{names_of, names_this_machine, short_host};
    use super::*;

    #[test]
    fn a_report_naming_this_machine_or_none_is_this_machine_s() {
        let directory = PathBuf::from("/tmp");
        let here = gethostname::gethostname().to_string_lossy().into_owned();
        let shouted = here.to_uppercase();
        let mut local = vec![None, Some("localhost"), Some("LocalHost")];
        // A machine with no name has nothing for a shell to say but nothing.
        if !here.is_empty() {
            local.extend([Some(here.as_str()), Some(shouted.as_str())]);
        }
        for host in local {
            assert_eq!(
                Remote::reported(host, directory.clone()),
                None,
                "{host:?} was taken for another machine"
            );
        }

        assert_eq!(
            Remote::reported(Some("buildbox.invalid"), directory.clone()),
            Some(Remote {
                host: "buildbox.invalid".to_owned(),
                directory,
            })
        );
    }

    #[test]
    fn a_name_nothing_knows_is_asked_about_again_and_every_name_is_kept() {
        let mut known = Vec::new();
        let renamed = || vec!["laptop-2.lan".to_owned()];
        let unasked = || -> Vec<String> { panic!("asked about a name it knew") };

        assert!(names_this_machine("laptop", &mut known, || vec![
            "laptop".to_owned()
        ]));
        assert!(names_this_machine("LAPTOP", &mut known, unasked));
        // The machine took another name on another network, and a shell
        // started since says that one.
        assert!(names_this_machine("laptop-2.lan", &mut known, renamed));
        // One started before the move still says the old one.
        assert!(names_this_machine("laptop", &mut known, unasked));
        // Another machine is asked about, and is not this one.
        assert!(!names_this_machine("buildbox", &mut known, renamed));
        assert_eq!(known, ["laptop", "laptop-2.lan"]);

        assert!(names_this_machine("localhost", &mut Vec::new(), unasked));
    }

    #[test]
    fn a_name_is_this_machine_s_by_its_first_label_but_an_address_is_not() {
        // A startup file that set `HOST=$(hostname -s)`, and a Mac that is
        // `eugens-mbp.local` to itself: taken for another machine, a shell
        // here would lose every git fact on its row.
        let mut known = vec!["Eugens-MBP.local".to_owned()];
        let nothing_new = || vec!["Eugens-MBP.local".to_owned()];
        assert!(names_this_machine("eugens-mbp", &mut known, nothing_new));
        assert!(names_this_machine(
            "eugens-mbp.lan",
            &mut known,
            nothing_new
        ));
        assert!(!names_this_machine(
            "buildbox.local",
            &mut known,
            nothing_new
        ));

        let mut known = vec!["10.0.0.5".to_owned()];
        let nothing_new = || vec!["10.0.0.5".to_owned()];
        assert!(names_this_machine("10.0.0.5", &mut known, nothing_new));
        assert!(!names_this_machine("10.0.0.7", &mut known, nothing_new));
        assert!(!names_this_machine("10", &mut known, nothing_new));
    }

    #[test]
    fn an_address_that_means_this_machine_to_whoever_says_it_is_this_machine() {
        // `HOST=0.0.0.0` for a dev server, and Debian's `127.0.1.1` for the
        // machine's own name: whatever shell wrote one, it is a shell here.
        let unasked = || -> Vec<String> { panic!("asked about an address that means here") };
        for host in [
            "127.0.0.1",
            "127.0.1.1",
            "::1",
            "[::1]",
            "0.0.0.0",
            "::",
            "[::]",
        ] {
            assert!(
                names_this_machine(host, &mut Vec::new(), unasked),
                "{host} was taken for another machine"
            );
        }
        let elsewhere = || vec!["laptop".to_owned()];
        for host in ["10.0.0.7", "[fe80::1]", "128.0.0.1"] {
            assert!(
                !names_this_machine(host, &mut Vec::new(), elsewhere),
                "{host} was taken for this machine"
            );
        }
    }

    #[test]
    fn the_names_crook_was_started_with_are_this_machine_s() {
        // Every shell Crook starts inherits `$HOST` and `$HOSTNAME`, and an
        // integration that writes one into its URL writes whatever it holds:
        // a conda environment's target triple, or the name the machine had
        // at login. Neither is another machine.
        let inherited = |variable: &str| match variable {
            "HOST" => Some("x86_64-apple-darwin13.4.0".to_owned()),
            "HOSTNAME" => Some(String::new()),
            _ => None,
        };
        let names = names_of("laptop".to_owned(), inherited);
        assert_eq!(names, ["laptop", "x86_64-apple-darwin13.4.0"]);

        let mut known = Vec::new();
        let ask = || names_of("laptop".to_owned(), inherited);
        assert!(names_this_machine(
            "x86_64-apple-darwin13.4.0",
            &mut known,
            ask
        ));
        assert!(!names_this_machine("buildbox", &mut known, ask));
    }

    #[test]
    fn a_host_is_shortened_the_way_a_prompt_shortens_it() {
        for (host, short) in [
            ("buildbox", "buildbox"),
            ("buildbox.internal.example.com", "buildbox"),
            ("eugen@buildbox.lan", "eugen@buildbox"),
            ("first.last@buildbox.lan", "first.last@buildbox"),
            // An address is not a name with a domain after it.
            ("10.0.0.5", "10.0.0.5"),
            ("eugen@10.0.0.5", "eugen@10.0.0.5"),
            ("::1", "::1"),
            ("[fe80::1]", "[fe80::1]"),
            (".hidden", ".hidden"),
        ] {
            assert_eq!(short_host(host), short, "{host}");
        }
    }
}

#[test]
fn a_new_group_with_a_tab_that_is_not_active_opens_where_that_tab_is_working() {
    // A tab opened into the group of one the keyboard is not in belongs with
    // that tab, and so does its directory. A worktree writes its checkout
    // over this; the tab menu says which pane, below.
    let (mut strip, ids) = strip(2);
    assert!(strip.is_active(ids[1]));
    let there = focused_pane_went_to(&mut strip, ids[0], "/where/the/clicked/tab/works");
    focused_pane_went_to(&mut strip, ids[1], "/where/the/active/tab/works");

    strip.apply(TabAction::NewInGroupOf(ids[0]));

    let opened = strip.active_id();
    assert!(!ids.contains(&opened));
    assert_eq!(
        directory_of(&strip, focused_pane(&strip, opened)),
        Some(there)
    );
}

#[test]
fn a_new_group_from_a_pane_its_tab_has_not_focused_opens_where_that_pane_is_working() {
    // The tab menu's "New group with tab" on a pane's own row, which under
    // `Panes` is every pane of a split tab: the row pressed, not the pane its
    // tab has focused, and not the active tab either.
    let (mut strip, ids) = strip(2);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Split(Direction::Right));
    let focused = focused_pane(&strip, ids[0]);
    let pressed = panes_of(&strip, ids[0])
        .into_iter()
        .find(|pane| *pane != focused)
        .expect("a split tab has a second pane");
    let there = PathBuf::from("/where/the/pressed/row/works");
    strip
        .pane_mut(pressed)
        .expect("the pane is open")
        .session_mut()
        .working_directory = Some(there.clone());
    focused_pane_went_to(&mut strip, ids[0], "/where/the/tab/has/focus");
    strip.apply(TabAction::Select(ids[1]));
    focused_pane_went_to(&mut strip, ids[1], "/where/the/active/tab/works");

    strip.apply(TabAction::NewInGroupFrom(pressed));

    let opened = strip.active_id();
    assert!(!ids.contains(&opened));
    let group = |tab| strip.get(tab).and_then(Tab::group);
    assert!(group(opened).is_some());
    assert_eq!(
        group(opened),
        group(ids[0]),
        "it joined the pressed row's tab"
    );
    assert_eq!(
        directory_of(&strip, focused_pane(&strip, opened)),
        Some(there)
    );
}

#[test]
fn a_tab_opened_beside_one_that_is_not_active_opens_where_that_tab_is_working() {
    // Both ways `crook tab new` opens one, so that whether it joins the
    // group is not also a choice of where its shell starts. What a caller
    // with no directory of its own gets: `open_worker_tab` writes the
    // asking pane's own over this when it has one.
    for grouped in [false, true] {
        let (mut strip, ids) = strip(2);
        let there = focused_pane_went_to(&mut strip, ids[0], "/where/the/asking/tab/works");
        focused_pane_went_to(&mut strip, ids[1], "/where/the/active/tab/works");

        strip.apply(TabAction::NewBeside {
            tab: ids[0],
            grouped,
        });

        let opened = order(&strip)
            .into_iter()
            .find(|tab| !ids.contains(tab))
            .expect("a tab opened");
        assert_eq!(
            directory_of(&strip, focused_pane(&strip, opened)),
            Some(there),
            "a tab opened beside another with grouped: {grouped}"
        );
    }
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
fn the_tab_used_before_is_the_one_left_last_and_there_is_none_when_alone() {
    let (mut strip, ids) = strip(3);
    strip.apply(TabAction::Select(ids[0]));
    strip.apply(TabAction::Select(ids[2]));
    assert_eq!(strip.used_before(), Some(ids[0]));

    strip.apply(TabAction::Select(ids[0]));
    assert_eq!(
        strip.used_before(),
        Some(ids[2]),
        "going back swaps the two"
    );

    strip.apply(TabAction::Close(ids[2]));
    assert_eq!(
        strip.used_before(),
        Some(ids[1]),
        "and a close falls through"
    );

    strip.apply(TabAction::Close(ids[1]));
    assert_eq!(strip.used_before(), None);
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
fn a_session_is_working_while_its_agent_runs_or_waits_or_its_shell_runs_a_command() {
    let mut session = AgentSession::new("agent 1");
    assert!(!session.is_working(), "a shell at its prompt");

    for (status, working) in [
        (AgentStatus::Running, true),
        (AgentStatus::NeedsInput, true),
        (AgentStatus::Failed, false),
        (AgentStatus::Idle, false),
    ] {
        session.status = status;
        assert_eq!(session.is_working(), working, "{status:?}");
    }

    // A command with no agent report behind it: a build, an ssh session, an
    // agent started without the hooks.
    session.running_command = Some("cargo build".to_owned());
    assert!(session.is_working(), "a command the shell is running");
    session.running_command = None;

    // The bell makes the dot say "waiting", and the shell under it is still
    // only a prompt.
    session.attention = Some(Attention::Bell);
    assert_eq!(session.shown_status(), AgentStatus::NeedsInput);
    assert!(!session.is_working(), "a prompt that rang");
}

#[test]
fn a_session_said_again_later_says_the_same_and_one_said_another_way_does_not() {
    // What decides whether a report is a frame: an agent saying running
    // again around every tool it calls has changed only when it said so.
    let mut said = AgentSession::new("agent 1");
    said.status = AgentStatus::Running;
    said.source = StatusSource::Agent(Instant::now());
    let mut again = said.clone();
    again.source = StatusSource::Agent(Instant::now() + std::time::Duration::from_secs(1));
    assert!(again.says_the_same_as(&said));

    // Where it came from is what "Why this status" says, so a change of it
    // is a change, however close the instant.
    let mut titled = said.clone();
    titled.source = StatusSource::Title(Instant::now());
    assert!(!titled.says_the_same_as(&said));

    fn changed(said: &AgentSession, change: impl FnOnce(&mut AgentSession)) -> AgentSession {
        let mut changed = said.clone();
        change(&mut changed);
        changed
    }
    for (what, changed) in [
        (
            "the name",
            changed(&said, |session| {
                session.derived_title = Some("port the tab bar".to_owned());
            }),
        ),
        (
            "the status",
            changed(&said, |session| session.status = AgentStatus::Idle),
        ),
        (
            "the question",
            changed(&said, |session| {
                session.message = Some("Run the tests?".to_owned());
            }),
        ),
        (
            "a look",
            changed(&said, |session| session.attention = Some(Attention::Bell)),
        ),
        ("the mark", changed(&said, |session| session.marked = true)),
        (
            "the command",
            changed(&said, |session| {
                session.running_command = Some("cargo test".to_owned());
            }),
        ),
    ] {
        assert!(!changed.says_the_same_as(&said), "{what} changed");
    }
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
fn a_split_across_the_branchs_axis_nests_in_the_focused_pane() {
    // "Split downwards" on the right half of a row stacks two panes in that
    // half and leaves the left one the full height of the tab — it used to
    // add a third column.
    let (mut strip, ids) = strip(1);
    let tab = ids[0];
    strip.apply(TabAction::Split(Direction::Right));
    let row = panes_of(&strip, tab);

    strip.apply(TabAction::Split(Direction::Down));

    let group = strip.get(tab).expect("open").panes();
    let below = group.focused_id();
    assert_eq!(
        group.layout(),
        Layout::Split {
            axis: SplitAxis::Horizontal,
            flex: DEFAULT_FLEX,
            children: vec![
                Layout::Pane(row[0]),
                Layout::Split {
                    axis: SplitAxis::Vertical,
                    flex: DEFAULT_FLEX,
                    children: vec![Layout::Pane(row[1]), Layout::Pane(below)],
                },
            ],
        }
    );
}

#[test]
fn a_split_along_the_branchs_axis_joins_it_beside_the_focused_pane() {
    // tmux's and Warp's rule: three "split right"s are a row of four, not
    // halves of halves, and a leading direction goes before the pane.
    let (mut strip, ids) = strip(1);
    let tab = ids[0];
    strip.apply(TabAction::Split(Direction::Down));
    strip.apply(TabAction::Split(Direction::Up));

    let group = strip.get(tab).expect("open").panes();
    assert_eq!(group.root_axis(), Some(SplitAxis::Vertical));
    assert_eq!(group.len(), 3);
    assert_eq!(
        group.index_of(group.focused_id()),
        Some(1),
        "a leading direction still inserts before the pane it was split off"
    );
    assert!(
        matches!(group.layout(), Layout::Split { children, .. } if children.len() == 3),
        "one column of three, not a column inside a column"
    );
}

#[test]
fn a_pane_put_back_beside_the_only_one_left_takes_back_the_way_its_split_went() {
    // Closed out of a column, and the tab split into a row and back to one
    // pane since: the pane left says nothing about which way the closed one
    // was, so the place kept for the one coming back says it.
    let (mut strip, ids) = strip(1);
    let tab = ids[0];
    strip.apply(TabAction::Split(Direction::Down));
    let column = panes_of(&strip, tab);
    let place = strip
        .get(tab)
        .expect("open")
        .panes()
        .place_of(column[1])
        .expect("in the split");
    strip.apply(TabAction::ClosePane(column[1]));
    strip.apply(TabAction::Split(Direction::Right));
    let row = panes_of(&strip, tab);
    strip.apply(TabAction::ClosePane(row[1]));
    assert_eq!(strip.get(tab).expect("open").panes().root_axis(), None);

    let pane = Pane::new("agent 2");
    let id = pane.id();
    strip.reopen_pane(tab, pane, &place);

    let group = strip.get(tab).expect("open").panes();
    assert_eq!(
        group.root_axis(),
        Some(SplitAxis::Vertical),
        "it came back in a column"
    );
    assert_eq!(panes_of(&strip, tab), [row[0], id]);
    assert_eq!(group.focused_id(), id);
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

/// Giving one pane the whole tab, with the split kept underneath.
///
/// The cases are the ones tmux's `zoom-pane` settled: what is drawn, what is
/// kept, and what ends a zoom — which is anything that would change what the
/// kept split means.
mod zooming {
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

    fn group(strip: &TabStrip) -> &PaneGroup {
        strip.active().expect("a tab is active").panes()
    }

    /// The panes the body would draw, in render order.
    fn visible(strip: &TabStrip) -> Vec<PaneId> {
        group(strip).visible().map(Pane::id).collect()
    }

    fn flexes(strip: &TabStrip) -> Vec<f32> {
        group(strip).iter().map(Pane::flex).collect()
    }

    #[test]
    fn zooming_hides_every_pane_but_the_focused_one() {
        let (mut strip, panes) = row(3);
        strip.apply(TabAction::FocusPane(panes[1]));

        assert_eq!(TabEffect::Changed, strip.apply(TabAction::ZoomPane));

        assert!(group(&strip).is_zoomed());
        assert_eq!(visible(&strip), vec![panes[1]]);
        assert_eq!(
            panes_of(&strip, strip.active_id()),
            panes,
            "the hidden panes are still open"
        );
        assert!(group(&strip).is_visible(panes[1]));
        assert!(!group(&strip).is_visible(panes[0]));
    }

    #[test]
    fn zooming_again_restores_the_split_exactly() {
        // The weights are the layout, and a zoom must not touch them: what
        // comes back is the split as it was dragged, not an even one.
        let (mut strip, panes) = row(3);
        strip.apply(TabAction::ResizePanes {
            before: panes[0],
            after: panes[1],
            leading: 0.75,
        });
        let before = flexes(&strip);

        strip.apply(TabAction::ZoomPane);
        assert_eq!(TabEffect::Changed, strip.apply(TabAction::ZoomPane));

        assert!(!group(&strip).is_zoomed());
        assert_eq!(visible(&strip), panes);
        assert_eq!(flexes(&strip), before);
        assert_eq!(group(&strip).focused_id(), panes[2]);
    }

    #[test]
    fn a_tab_that_was_never_split_cannot_be_zoomed() {
        // The same refusal a nudge makes: there is nothing to hide, and the
        // action has to report that it did nothing so the chord goes on to
        // the shell.
        let (mut strip, _) = row(1);

        assert_eq!(TabEffect::Unchanged, strip.apply(TabAction::ZoomPane));
        assert!(!group(&strip).is_zoomed());
    }

    #[test]
    fn closing_the_zoomed_pane_shows_the_split_again() {
        let (mut strip, panes) = row(3);
        strip.apply(TabAction::ZoomPane);

        strip.apply(TabAction::ClosePane(panes[2]));

        assert!(!group(&strip).is_zoomed());
        assert_eq!(visible(&strip), vec![panes[0], panes[1]]);
    }

    #[test]
    fn closing_a_hidden_pane_ends_the_zoom_too() {
        // A hidden pane's shell can exit on its own. The split the zoom was
        // keeping is not the split any more, so the person gets to see what
        // is left rather than finding out on the next toggle.
        let (mut strip, panes) = row(3);
        strip.apply(TabAction::ZoomPane);

        strip.apply(TabAction::ClosePane(panes[0]));

        assert!(!group(&strip).is_zoomed());
        assert_eq!(visible(&strip), vec![panes[1], panes[2]]);
        assert_eq!(group(&strip).focused_id(), panes[2]);
    }

    #[test]
    fn moving_the_focus_ends_the_zoom() {
        // Focusing a hidden pane is the one way to reach one without the
        // chord — a click on its row — and a zoom that followed the focus
        // would show a pane a person has not seen the context of.
        let (mut strip, panes) = row(3);
        strip.apply(TabAction::ZoomPane);

        strip.apply(TabAction::FocusPane(panes[0]));

        assert!(!group(&strip).is_zoomed());
        assert_eq!(visible(&strip), panes);
        assert_eq!(group(&strip).focused_id(), panes[0]);
    }

    #[test]
    fn focusing_the_zoomed_pane_itself_changes_nothing() {
        // A click on the one pane that is showing is not a request to see
        // the others.
        let (mut strip, panes) = row(2);
        strip.apply(TabAction::ZoomPane);

        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::FocusPane(panes[1]))
        );
        assert!(group(&strip).is_zoomed());
    }

    #[test]
    fn splitting_a_zoomed_pane_ends_the_zoom() {
        let (mut strip, _) = row(2);
        strip.apply(TabAction::ZoomPane);

        strip.apply(TabAction::Split(Direction::Right));

        assert!(!group(&strip).is_zoomed());
        assert_eq!(visible(&strip).len(), 3, "the new pane is among them");
    }

    #[test]
    fn a_zoom_is_the_active_tabs_and_another_tab_keeps_its_own() {
        // One bool per group, so a zoom in one tab says nothing about the
        // tab beside it — and selecting the other tab does not end it.
        let (mut strip, _) = row(2);
        let zoomed = strip.active_id();
        strip.apply(TabAction::ZoomPane);

        strip.apply(TabAction::New);
        assert_eq!(TabEffect::Unchanged, strip.apply(TabAction::ZoomPane));

        strip.apply(TabAction::Select(zoomed));
        assert!(group(&strip).is_zoomed(), "the tab came back as it was");
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
    fn a_tab_opened_beside_another_takes_no_focus_and_goes_last_in_the_order_of_use() {
        // What `crook tab new` opens: the person is looking at the third tab,
        // the pane that asked is in the first, and the new tab goes beside
        // the first without moving anybody's keyboard.
        let (mut strip, ids) = strip(3);
        strip.apply(TabAction::Select(ids[2]));
        let used = strip.mru().to_vec();

        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::NewBeside {
                tab: ids[0],
                grouped: false,
            })
        );
        let now = order(&strip);
        assert_eq!(now.len(), 4);
        let opened = now[1];
        assert!(!ids.contains(&opened), "beside the tab that asked: {now:?}");
        assert_eq!(strip.get(opened).and_then(Tab::group), None);
        assert_eq!(
            strip.active_id(),
            ids[2],
            "the person's tab is still active"
        );
        assert_eq!(
            strip.mru(),
            [&used[..], &[opened]].concat(),
            "nobody has used it, so it is the last tab to go back to"
        );

        // In a group, the same: it joins, and it is not selected.
        strip.apply(TabAction::NewBeside {
            tab: ids[0],
            grouped: true,
        });
        let group = strip
            .get(ids[0])
            .and_then(Tab::group)
            .expect("a group was made");
        let members: Vec<TabId> = strip.members(group).map(Tab::id).collect();
        assert_eq!(members.len(), 2, "{members:?}");
        assert_eq!(members[0], ids[0]);
        assert_eq!(
            strip.active_id(),
            ids[2],
            "the person's tab is still active"
        );
        assert_eq!(strip.mru().last(), Some(&members[1]));
        assert_eq!(
            strip.mru().len(),
            strip.len(),
            "every tab is in the order of use"
        );

        // Beside a tab that has closed there is nowhere to put one.
        strip.apply(TabAction::Close(ids[1]));
        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::NewBeside {
                tab: ids[1],
                grouped: false,
            })
        );
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
        // Aimed a tab past the group's end, so the clamp has to move it:
        // before the tab straight after the group, it would have landed at
        // the end with no clamp at all.
        let (mut strip, ids, group) = grouped(4);
        let outsider = *ids.last().expect("four tabs");

        strip.apply(TabAction::MoveTab {
            tab: outsider,
            group: Some(group),
            before: Some(ids[3]),
        });

        assert_eq!(shape(&strip), vec![(Some(group), 3), (None, 1), (None, 1)]);
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

        // The second group dropped on the first one's second member: above
        // the whole of it is somewhere else than it was, so the snap is what
        // puts it there. The other way round, `first` is already above
        // `second`, and a drop that refused did as well as one that landed.
        let first_member = strip.members(first).map(Tab::id).nth(1).expect("two");
        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::MoveGroup {
                group: second,
                before: Some(first_member),
            })
        );

        assert_eq!(shape(&strip), vec![(Some(second), 2), (Some(first), 2)]);
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
    // And the group is still one block where it was. `members` reads a group
    // in strip order whether or not its tabs are together, so it could not
    // see a member lifted to the front of the list and away from the other.
    assert_eq!(
        order(&strip),
        [ids[0], members[1], members[0], ids[2]],
        "pinning took the member out of its group's place"
    );
}

#[test]
fn pinning_the_only_tab_left_in_a_group_leaves_the_group_where_it_is() {
    // What is left of a worktree group once one of its two checkouts is
    // closed. Lifted out to be put back, the tab left its group with no run
    // at all, and the block it was put back into was the ungrouped run above.
    let mut strip = TabStrip::new();
    strip.apply(TabAction::New);
    strip.apply(TabAction::New);
    let ids: Vec<TabId> = strip.iter().map(Tab::id).collect();
    strip.apply(TabAction::NewInGroupOf(ids[2]));
    let group = strip
        .get(ids[2])
        .and_then(Tab::group)
        .expect("the two of them made a group");
    let other = strip
        .members(group)
        .map(Tab::id)
        .find(|id| *id != ids[2])
        .expect("the group has a second member");
    strip.apply(TabAction::Close(other));
    assert!(
        strip.group(group).is_some(),
        "the group went with its second tab"
    );
    assert_eq!(order(&strip), ids, "closing the second member moved things");

    strip.apply(TabAction::TogglePin(ids[2]));
    assert!(strip.get(ids[2]).is_some_and(Tab::is_pinned));
    assert_eq!(order(&strip), ids, "pinning its only tab moved the group");
    assert_eq!(strip.get(ids[2]).and_then(Tab::group), Some(group));

    strip.apply(TabAction::TogglePin(ids[2]));
    assert_eq!(order(&strip), ids, "unpinning its only tab moved the group");
}

#[test]
fn a_group_closed_between_two_runs_leaves_one_run_with_its_pins_in_front() {
    // Two ungrouped runs, each with a pin at its front, and a group between
    // them. Closing the group joins the runs; each pin stays pinned and at
    // the front of the one run they are now in.
    let mut strip = TabStrip::new();
    for _ in 0..4 {
        strip.apply(TabAction::New);
    }
    let ids: Vec<TabId> = strip.iter().map(Tab::id).collect();
    let [a, b, c, d, e] = [ids[0], ids[1], ids[2], ids[3], ids[4]];
    strip.apply(TabAction::NewInGroupOf(c));
    let group = strip
        .get(c)
        .and_then(Tab::group)
        .expect("the two of them made a group");
    strip.apply(TabAction::TogglePin(a));
    strip.apply(TabAction::TogglePin(d));
    assert_eq!(
        order(&strip)[..2],
        [a, b],
        "the first run is not as this expects"
    );
    assert_eq!(
        order(&strip)[4..],
        [d, e],
        "the second run is not as this expects"
    );

    assert_eq!(
        strip.apply(TabAction::CloseGroup(group)),
        TabEffect::Changed
    );
    assert_eq!(
        order(&strip),
        [a, d, b, e],
        "the pinned tab of the second run was left under an unpinned one"
    );

    // And the run moves the way a run with its pins in front does.
    strip.apply(TabAction::Select(e));
    assert_eq!(
        strip.apply(TabAction::MoveLeft),
        TabEffect::Changed,
        "an unpinned tab could not move past an unpinned one"
    );
    assert_eq!(order(&strip), [a, d, e, b]);
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

#[test]
fn a_tab_opened_from_a_pinned_one_lands_after_the_pins() {
    let mut strip = TabStrip::new();
    strip.apply(TabAction::New);
    strip.apply(TabAction::New);
    let ids: Vec<TabId> = strip.iter().map(Tab::id).collect();
    strip.apply(TabAction::TogglePin(ids[0]));
    strip.apply(TabAction::TogglePin(ids[1]));
    strip.apply(TabAction::Select(ids[0]));

    strip.apply(TabAction::New);

    let order: Vec<TabId> = strip.iter().map(Tab::id).collect();
    assert_eq!(&order[..2], &ids[..2], "a new tab got in among the pins");
    assert!(!strip.get(order[2]).is_some_and(Tab::is_pinned));
    assert_eq!(
        strip.active().map(Tab::id),
        Some(order[2]),
        "the new tab is not the active one"
    );
}

#[test]
fn a_hop_does_not_cross_the_line_between_pinned_and_unpinned() {
    let mut strip = TabStrip::new();
    strip.apply(TabAction::New);
    let ids: Vec<TabId> = strip.iter().map(Tab::id).collect();
    strip.apply(TabAction::TogglePin(ids[0]));

    // The unpinned tab, one hop from above the pinned one.
    strip.apply(TabAction::Select(ids[1]));
    assert_eq!(strip.apply(TabAction::MoveLeft), TabEffect::Unchanged);
    // The pinned tab, one hop from below the unpinned one.
    strip.apply(TabAction::Select(ids[0]));
    assert_eq!(strip.apply(TabAction::MoveRight), TabEffect::Unchanged);

    assert_eq!(strip.iter().map(Tab::id).collect::<Vec<_>>(), ids);
}

#[test]
fn a_lone_member_moved_within_its_own_group_cannot_split_another() {
    // `a` is the only member of its group, so once it is lifted out to move
    // the group has no run to clamp the drop into — and the drop named a row
    // in the middle of `b`'s group.
    let (mut strip, ids) = strip(2);
    let (a, b) = (ids[0], ids[1]);
    strip.apply(TabAction::NewInGroupOf(a));
    let own = strip.get(a).and_then(Tab::group).expect("a is grouped");
    let c = order(&strip)[1];
    strip.apply(TabAction::MoveTab {
        tab: c,
        group: None,
        before: None,
    });
    strip.apply(TabAction::NewInGroupOf(b));
    let other = strip.get(b).and_then(Tab::group).expect("b is grouped");
    let d = order(&strip)[2];
    assert_eq!(order(&strip), vec![a, b, d, c]);

    strip.apply(TabAction::MoveTab {
        tab: a,
        group: Some(own),
        before: Some(d),
    });

    let blocks: Vec<Option<GroupId>> = strip.blocks().iter().map(|block| block.group).collect();
    assert_eq!(
        blocks.iter().filter(|group| **group == Some(other)).count(),
        1,
        "`b`'s group was split in two: {blocks:?}"
    );
    assert_eq!(
        strip.get(a).and_then(Tab::group),
        Some(own),
        "and `a` is still in its own"
    );
}

// --- a pull request's chip and branch ------------------------------------------

#[test]
fn a_pull_request_on_github_is_a_number_and_anywhere_else_names_its_host() {
    let label = |url: &str| pull_request_label(url).expect("an address has a label");
    assert_eq!(
        label("https://github.com/theguriev/crook/pull/398"),
        "PR #398"
    );
    assert_eq!(
        label("https://GitHub.com/o/r/pull/7/files?diff=split#top"),
        "PR #7"
    );
    // Another forge's spelling, and a host that is not github.com, say where
    // they are: the chip is not the whole address, and it is not a lie.
    assert_eq!(
        label("https://gitlab.com/group/project/-/merge_requests/42"),
        "gitlab.com #42"
    );
    assert_eq!(
        label("https://github.example.com/o/r/pull/12"),
        "github.example.com #12"
    );
    assert_eq!(
        label("https://gitea.example.org/o/r/pulls"),
        "gitea.example.org"
    );
    assert_eq!(pull_request_label("  "), None);
}

#[test]
fn an_address_dressed_as_github_is_labelled_with_the_host_it_opens() {
    // Any program's output can write the sequence, so a look-alike host and
    // a `github.com@` in front of another one are the two ways a planted
    // link would pass for the agent's: each is labelled with the host the
    // browser would actually be sent to.
    assert_eq!(
        pull_request_label("https://github.com.attacker.example/o/r/pull/12").as_deref(),
        Some("github.com.attacker.example #12")
    );
    assert_eq!(
        pull_request_label("https://github.com@attacker.example/o/r/pull/12").as_deref(),
        Some("attacker.example #12")
    );
    // A browser ends an https host at a `\` as it does at a `/`, so an
    // `@github.com` after one is in the path, and the host is before it.
    assert_eq!(
        pull_request_label("https://attacker.example\\@github.com/o/r/pull/12").as_deref(),
        Some("attacker.example #12")
    );
    assert_eq!(
        pull_request_label("https://github.com\\@attacker.example/o/r/pull/12").as_deref(),
        Some("PR #12")
    );
    // And `/pull/` in the query is not in the path.
    assert_eq!(
        pull_request_label("https://github.com/o/r/issues?q=/pull/12").as_deref(),
        Some("github.com")
    );
}

#[test]
fn a_pull_request_goes_with_another_branch_and_stays_through_a_detached_head() {
    // Paths that name no directory, so that the repository is told apart by
    // its path alone and never resolved.
    let at = |repository: &str, head: Head| BranchAtWork {
        repository: PathBuf::from(format!("/nowhere/crook-tab-tests/{repository}/.git")),
        head,
    };
    let feat = Head::Branch("feat".to_owned());
    let detached = Head::Detached {
        short: "1a22cb9".to_owned(),
        full: "1a22cb92d4e5f60718293a4b5c6d7e8f90123456".to_owned(),
    };
    let pull_request = PullRequest::new(
        "https://github.com/o/r/pull/1".to_owned(),
        Some(at("app", feat.clone())),
    );

    assert!(!pull_request.is_left_for(Some(&at("app", feat.clone()))));
    assert!(pull_request.is_left_for(Some(&at("app", Head::Branch("main".to_owned())))));
    // Out of the repository is no branch at all.
    assert!(pull_request.is_left_for(None));
    // A commit checked out to look at, or a bisect: no other branch named,
    // and nothing would report the pull request again when feat comes back.
    assert!(!pull_request.is_left_for(Some(&at("app", detached.clone()))));

    // Another repository is other work whatever its `HEAD` says: a submodule
    // checked out detached, or a branch that has the same name.
    assert!(pull_request.is_left_for(Some(&at("lib", detached))));
    assert!(pull_request.is_left_for(Some(&at("other", feat.clone()))));
    // And a pull request reported outside any repository has gone once the
    // pane is in one.
    let nowhere = PullRequest::new("https://github.com/o/r/pull/1".to_owned(), None);
    assert!(!nowhere.is_left_for(None));
    assert!(nowhere.is_left_for(Some(&at("app", feat))));
}

#[test]
fn close_others_keeps_the_tab_it_was_asked_about_and_the_pinned_ones() {
    let (mut strip, ids) = strip(5);
    strip.apply(TabAction::TogglePin(ids[3]));
    let pinned = ids[3];
    let order_before = order(&strip);
    assert_eq!(
        order_before[0], pinned,
        "pinning moved the tab to the front"
    );

    assert_eq!(
        strip.apply(TabAction::CloseOthers(ids[1])),
        TabEffect::Changed
    );

    assert_eq!(order(&strip), [pinned, ids[1]]);
}

#[test]
fn close_below_and_above_take_the_unpinned_tabs_on_their_side() {
    let (mut strip, ids) = strip(5);
    assert_eq!(
        strip.closed_by(TabAction::CloseBelow(ids[2])),
        [ids[3], ids[4]]
    );
    assert_eq!(
        strip.closed_by(TabAction::CloseAbove(ids[2])),
        [ids[0], ids[1]]
    );

    strip.apply(TabAction::CloseBelow(ids[2]));
    assert_eq!(order(&strip), &ids[..3]);

    strip.apply(TabAction::CloseAbove(ids[2]));
    assert_eq!(order(&strip), [ids[2]]);
}

#[test]
fn a_close_of_several_with_nothing_to_take_changes_nothing() {
    // The bottom tab has nothing below it, and a strip of one has no others.
    // Neither may reach `close`, which would close the window for the last
    // tab rather than leave it alone.
    let (mut strip, ids) = strip(2);
    assert_eq!(
        strip.apply(TabAction::CloseBelow(ids[1])),
        TabEffect::Unchanged
    );
    assert_eq!(
        strip.apply(TabAction::CloseAbove(ids[0])),
        TabEffect::Unchanged
    );
    strip.apply(TabAction::Close(ids[0]));
    assert_eq!(
        strip.apply(TabAction::CloseOthers(ids[1])),
        TabEffect::Unchanged
    );
    assert_eq!(order(&strip), [ids[1]]);
}

/// Splits that nest: a 2×2, an agent beside two stacked watchers, and every
/// operation that has to walk the tree to get them right.
mod nesting {
    use super::*;

    /// The active tab's pane group.
    fn group(strip: &TabStrip) -> &PaneGroup {
        strip.active().expect("a tab is active").panes()
    }

    /// A 2×2, and its panes as `[top left, top right, bottom left, bottom
    /// right]`.
    ///
    /// Made the way a person makes one: split right, split the right half
    /// down, go back to the left half and split it down too.
    fn grid() -> (TabStrip, [PaneId; 4]) {
        let mut strip = TabStrip::new();
        let top_left = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Right));
        let top_right = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Down));
        let bottom_right = group(&strip).focused_id();
        strip.apply(TabAction::FocusPane(top_left));
        strip.apply(TabAction::Split(Direction::Down));
        let bottom_left = group(&strip).focused_id();
        (strip, [top_left, top_right, bottom_left, bottom_right])
    }

    /// An agent on the left, and two watchers stacked on its right.
    fn agent_and_watchers() -> (TabStrip, [PaneId; 3]) {
        let mut strip = TabStrip::new();
        let agent = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Right));
        let upper = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Down));
        let lower = group(&strip).focused_id();
        (strip, [agent, upper, lower])
    }

    fn column(children: Vec<Layout>) -> Layout {
        Layout::Split {
            axis: SplitAxis::Vertical,
            flex: DEFAULT_FLEX,
            children,
        }
    }

    fn row(children: Vec<Layout>) -> Layout {
        Layout::Split {
            axis: SplitAxis::Horizontal,
            flex: DEFAULT_FLEX,
            children,
        }
    }

    fn flex_of(strip: &TabStrip, pane: PaneId) -> f32 {
        group(strip).get(pane).expect("open").flex()
    }

    #[test]
    fn four_splits_make_a_two_by_two() {
        let (strip, [a, b, c, d]) = grid();

        assert_eq!(
            group(&strip).layout(),
            row(vec![
                column(vec![Layout::Pane(a), Layout::Pane(c)]),
                column(vec![Layout::Pane(b), Layout::Pane(d)]),
            ])
        );
        assert_eq!(
            panes_of(&strip, strip.active_id()),
            [a, c, b, d],
            "render order goes down the first column before the second"
        );
    }

    #[test]
    fn the_keyboard_moves_across_a_two_by_two_by_where_the_panes_are() {
        let (mut strip, [a, b, c, d]) = grid();
        strip.apply(TabAction::FocusPane(a));

        assert_eq!(group(&strip).neighbour(Direction::Right), Some(b));
        assert_eq!(group(&strip).neighbour(Direction::Down), Some(c));
        assert_eq!(group(&strip).neighbour(Direction::Left), None);
        assert_eq!(group(&strip).neighbour(Direction::Up), None);

        strip.apply(TabAction::FocusPane(d));
        assert_eq!(group(&strip).neighbour(Direction::Left), Some(c));
        assert_eq!(group(&strip).neighbour(Direction::Up), Some(b));
        assert_eq!(group(&strip).neighbour(Direction::Right), None);
        assert_eq!(group(&strip).neighbour(Direction::Down), None);
    }

    #[test]
    fn a_pane_at_the_corner_is_nobodys_neighbour() {
        // The top-left and bottom-right panes of a 2×2 share one point. A
        // direction that answered with the diagonal would land on a pane
        // nobody pointed at.
        let (mut strip, [a, _, _, d]) = grid();
        strip.apply(TabAction::FocusPane(a));

        for direction in [
            Direction::Left,
            Direction::Right,
            Direction::Up,
            Direction::Down,
        ] {
            assert_ne!(group(&strip).neighbour(direction), Some(d), "{direction:?}");
        }
    }

    #[test]
    fn of_two_panes_beside_the_focused_one_the_last_used_is_the_one_moved_to() {
        // tmux's answer, and the one that makes left-then-right a round trip.
        let (mut strip, [agent, upper, lower]) = agent_and_watchers();

        strip.apply(TabAction::FocusPane(lower));
        strip.apply(TabAction::FocusPane(agent));
        assert_eq!(group(&strip).neighbour(Direction::Right), Some(lower));

        strip.apply(TabAction::FocusPane(upper));
        assert_eq!(group(&strip).neighbour(Direction::Left), Some(agent));
        strip.apply(TabAction::FocusPane(agent));
        assert_eq!(group(&strip).neighbour(Direction::Right), Some(upper));
    }

    #[test]
    fn a_pane_the_full_height_of_the_tab_is_beside_both_halves_of_the_other_column() {
        let (mut strip, [agent, upper, lower]) = agent_and_watchers();

        for watcher in [upper, lower] {
            strip.apply(TabAction::FocusPane(watcher));
            assert_eq!(group(&strip).neighbour(Direction::Left), Some(agent));
        }
        strip.apply(TabAction::FocusPane(upper));
        assert_eq!(group(&strip).neighbour(Direction::Down), Some(lower));
    }

    #[test]
    fn closing_a_corner_gives_its_column_to_the_pane_left_in_it() {
        let (mut strip, [a, b, c, d]) = grid();
        strip.apply(TabAction::ResizePanes {
            before: a,
            after: b,
            leading: 0.25,
        });
        let column_share = group(&strip).root_flexes();

        strip.apply(TabAction::ClosePane(c));

        assert_eq!(
            group(&strip).layout(),
            row(vec![
                Layout::Pane(a),
                Layout::Split {
                    axis: SplitAxis::Vertical,
                    flex: column_share[1],
                    children: vec![Layout::Pane(b), Layout::Pane(d)],
                },
            ]),
            "no branch is left holding one pane, and the other column kept its share"
        );
        assert!(
            (flex_of(&strip, a) - column_share[0]).abs() < 1e-5,
            "the pane took its column's share: {} for {column_share:?}",
            flex_of(&strip, a)
        );
    }

    #[test]
    fn a_branch_left_dividing_its_parents_way_is_folded_into_it() {
        // A row with a column in it, and a row in that column. Closing the
        // column's other pane leaves the inner row standing where the column
        // was — a row in a row — so its panes join the outer row, sharing
        // what the column had in the proportions they had.
        let mut strip = TabStrip::new();
        let a = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Right));
        let b = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Down));
        let c = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Right));
        let d = group(&strip).focused_id();
        strip.apply(TabAction::ResizePanes {
            before: c,
            after: d,
            leading: 0.75,
        });

        strip.apply(TabAction::ClosePane(b));

        assert_eq!(
            group(&strip).layout(),
            row(vec![Layout::Pane(a), Layout::Pane(c), Layout::Pane(d)])
        );
        let (c_flex, d_flex) = (flex_of(&strip, c), flex_of(&strip, d));
        assert!(
            (c_flex + d_flex - DEFAULT_FLEX).abs() < 1e-5,
            "the pair took the column's share between them: {c_flex} + {d_flex}"
        );
        assert!((c_flex / d_flex - 3.).abs() < 1e-4, "{c_flex} : {d_flex}");
    }

    #[test]
    fn a_drag_between_two_small_shares_keeps_what_the_pair_had() {
        // A column squeezed to its floor and then folded into the row beside
        // it hands its two panes a share under the floor each. A floor fixed
        // in the units of the pair would push them apart and take room from
        // every other pane in the row.
        let mut strip = TabStrip::new();
        let a = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Right));
        let b = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Down));
        let c = group(&strip).focused_id();
        strip.apply(TabAction::Split(Direction::Right));
        let d = group(&strip).focused_id();
        strip.apply(TabAction::ResizePanes {
            before: a,
            after: b,
            leading: 1.,
        });
        strip.apply(TabAction::ClosePane(b));
        let pair = flex_of(&strip, c) + flex_of(&strip, d);

        strip.apply(TabAction::ResizePanes {
            before: c,
            after: d,
            leading: 0.,
        });

        let after = flex_of(&strip, c) + flex_of(&strip, d);
        assert!((after - pair).abs() < 1e-5, "{pair} became {after}");
        assert!(flex_of(&strip, c) > 0.);
    }

    #[test]
    fn a_divider_inside_a_column_moves_only_that_column() {
        let (mut strip, [a, b, c, d]) = grid();

        assert_eq!(
            TabEffect::Changed,
            strip.apply(TabAction::ResizePanes {
                before: b,
                after: d,
                leading: 0.75,
            })
        );

        assert!((flex_of(&strip, b) - 1.5).abs() < 1e-5);
        assert!((flex_of(&strip, d) - 0.5).abs() < 1e-5);
        assert_eq!(flex_of(&strip, a), 1., "the other column never moved");
        assert_eq!(flex_of(&strip, c), 1.);
        assert_eq!(group(&strip).root_flexes(), vec![1., 1.]);
    }

    #[test]
    fn two_dividers_after_the_same_pane_are_told_apart_by_the_pane_after_them() {
        // The row's divider and the left column's divider both have the
        // top-left pane before them; only what comes after says which one a
        // drag is on.
        let (mut strip, [a, b, c, _]) = grid();

        strip.apply(TabAction::ResizePanes {
            before: a,
            after: b,
            leading: 0.75,
        });
        assert!((group(&strip).root_flexes()[0] - 1.5).abs() < 1e-5);
        assert_eq!(flex_of(&strip, a), 1., "the column's own divider stayed");

        strip.apply(TabAction::ResizePanes {
            before: a,
            after: c,
            leading: 0.25,
        });
        assert!((flex_of(&strip, a) - 0.5).abs() < 1e-5);
        assert!(
            (group(&strip).root_flexes()[0] - 1.5).abs() < 1e-5,
            "the row's divider stayed"
        );
    }

    #[test]
    fn a_pair_from_two_different_branches_names_no_divider() {
        let (mut strip, [_, _, c, d]) = grid();

        assert_eq!(
            TabEffect::Unchanged,
            strip.apply(TabAction::ResizePanes {
                before: c,
                after: d,
                leading: 0.75,
            }),
            "the two bottom panes are side by side, but in two columns"
        );
    }

    #[test]
    fn the_keyboard_resizes_the_innermost_split_the_pane_is_in() {
        let (mut strip, [agent, upper, lower]) = agent_and_watchers();
        strip.apply(TabAction::FocusPane(upper));

        strip.apply(TabAction::NudgePane { grow: true });

        assert!(flex_of(&strip, upper) > 1., "the watcher grew");
        assert!(flex_of(&strip, lower) < 1., "against the one under it");
        assert_eq!(group(&strip).root_flexes(), vec![1., 1.]);
        assert_eq!(flex_of(&strip, agent), 1.);
    }

    #[test]
    fn evening_out_reaches_every_depth() {
        let (mut strip, [a, b, c, d]) = grid();
        strip.apply(TabAction::ResizePanes {
            before: a,
            after: b,
            leading: 0.8,
        });
        strip.apply(TabAction::ResizePanes {
            before: b,
            after: d,
            leading: 0.2,
        });

        assert_eq!(TabEffect::Changed, strip.apply(TabAction::EvenPanes));

        assert_eq!(group(&strip).root_flexes(), vec![1., 1.]);
        for pane in [a, b, c, d] {
            assert_eq!(flex_of(&strip, pane), 1.);
        }
        assert_eq!(TabEffect::Unchanged, strip.apply(TabAction::EvenPanes));
    }

    #[test]
    fn a_zoom_of_a_nested_pane_keeps_the_whole_tree_under_it() {
        let (mut strip, [a, b, _, d]) = grid();
        strip.apply(TabAction::ResizePanes {
            before: b,
            after: d,
            leading: 0.7,
        });
        strip.apply(TabAction::FocusPane(d));
        let kept = group(&strip).layout();
        let shares: Vec<f32> = group(&strip).iter().map(Pane::flex).collect();

        strip.apply(TabAction::ZoomPane);
        let shown: Vec<PaneId> = group(&strip).visible().map(Pane::id).collect();
        assert_eq!(shown, [d]);
        assert!(!group(&strip).is_visible(a));

        strip.apply(TabAction::ZoomPane);
        assert_eq!(group(&strip).layout(), kept);
        let back: Vec<f32> = group(&strip).iter().map(Pane::flex).collect();
        assert_eq!(back, shares);
    }

    #[test]
    fn walking_out_of_a_zoom_lands_on_a_pane_that_is_drawn() {
        let (mut strip, [_, b, _, d]) = grid();
        strip.apply(TabAction::FocusPane(d));
        strip.apply(TabAction::ZoomPane);

        let up = group(&strip).neighbour(Direction::Up).expect("b is above");
        assert_eq!(up, b);
        strip.apply(TabAction::FocusPane(up));

        assert!(!group(&strip).is_zoomed());
        assert!(group(&strip).is_visible(b));
    }

    #[test]
    fn a_corner_closed_and_put_back_restores_the_two_by_two_exactly() {
        let (mut strip, [a, b, c, _]) = grid();
        let tab = strip.active_id();
        strip.apply(TabAction::ResizePanes {
            before: a,
            after: b,
            leading: 0.3,
        });
        strip.apply(TabAction::ResizePanes {
            before: a,
            after: c,
            leading: 0.6,
        });
        let before = group(&strip).layout();
        let shares: Vec<f32> = group(&strip).iter().map(Pane::flex).collect();
        let place = group(&strip).place_of(c).expect("in the split");

        strip.apply(TabAction::ClosePane(c));
        let pane = Pane::new("agent 3");
        let back = pane.id();
        pane_with_flex(pane, shares[1], |pane| {
            strip.reopen_pane(tab, pane, &place);
        });

        let after = group(&strip).layout();
        assert_eq!(
            format!("{after:?}"),
            format!("{before:?}").replace(&format!("{c:?}"), &format!("{back:?}")),
            "the tree came back as it was, with the new pane where the old one was"
        );
        let restored: Vec<f32> = group(&strip).iter().map(Pane::flex).collect();
        assert_eq!(restored, shares, "every share came back too");
        assert_eq!(group(&strip).focused_id(), back);
    }

    #[test]
    fn a_pane_put_back_after_the_split_moved_on_goes_beside_its_neighbour() {
        // A pane opened since makes the old shape a stranger's: rebuilding it
        // would throw the new pane about. The closed one goes back on its
        // old neighbour's side instead.
        let (mut strip, [agent, upper, lower]) = agent_and_watchers();
        let tab = strip.active_id();
        let place = group(&strip).place_of(lower).expect("in the split");
        strip.apply(TabAction::ClosePane(lower));
        strip.apply(TabAction::FocusPane(agent));
        strip.apply(TabAction::Split(Direction::Down));
        let new = group(&strip).focused_id();

        let pane = Pane::new("agent 9");
        let back = pane.id();
        strip.reopen_pane(tab, pane, &place);

        assert_eq!(
            group(&strip).layout(),
            row(vec![
                column(vec![Layout::Pane(agent), Layout::Pane(new)]),
                column(vec![Layout::Pane(upper), Layout::Pane(back)]),
            ])
        );
    }

    /// Hands `pane` over with `flex` as its share, the way a closed pane's
    /// snapshot restores one.
    fn pane_with_flex(mut pane: Pane, flex: f32, put: impl FnOnce(Pane)) {
        pane.set_flex(flex);
        put(pane);
    }

    /// A tiny deterministic generator, so a failing sequence is the same
    /// sequence every run.
    struct Steps(u64);

    impl Steps {
        fn next(&mut self, below: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % below as u64) as usize
        }
    }

    /// Everything the module docs promise of a tree, checked from outside.
    fn assert_in_shape(group: &PaneGroup, open: &[PaneId], step: &str) {
        fn walk(node: &Node, parent: Option<SplitAxis>, seen: &mut Vec<PaneId>, step: &str) {
            assert!(
                node.flex().is_finite() && node.flex() > 0.,
                "{step}: a share of {}",
                node.flex()
            );
            match node {
                Node::Leaf(pane) => seen.push(pane.id()),
                Node::Branch(branch) => {
                    assert!(
                        branch.children().len() >= 2,
                        "{step}: a branch of {}",
                        branch.children().len()
                    );
                    assert_ne!(
                        Some(branch.axis()),
                        parent,
                        "{step}: a branch inside one dividing the same way"
                    );
                    for child in branch.children() {
                        walk(child, Some(branch.axis()), seen, step);
                    }
                }
            }
        }

        let mut seen = Vec::new();
        walk(group.root(), None, &mut seen, step);
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), seen.len(), "{step}: a pane is in it twice");
        let mut expected = open.to_vec();
        expected.sort();
        assert_eq!(
            sorted, expected,
            "{step}: the panes in it are not the open ones"
        );
        assert!(
            seen.contains(&group.focused_id()),
            "{step}: the focus is on a pane that is not there"
        );
        assert!(
            group.is_visible(group.focused_id()),
            "{step}: the focus is on a pane nobody can see"
        );
        let mut used = group.mru().to_vec();
        used.sort();
        assert_eq!(
            used, expected,
            "{step}: the order of use lost or kept a pane"
        );
        assert_eq!(group.mru()[0], group.focused_id());
    }

    #[test]
    fn any_run_of_splits_closes_resizes_zooms_and_reopens_leaves_the_tree_in_shape() {
        // From the audit that found the flat group's one fixed axis: what a
        // person does to a split in any order, a few thousand times over.
        for seed in 1..=40_u64 {
            let mut steps = Steps(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut strip = TabStrip::new();
            let tab = strip.active_id();
            let mut open = panes_of(&strip, tab);
            let mut closed: Vec<(PanePlace, f32)> = Vec::new();

            for turn in 0..150 {
                let pick = |steps: &mut Steps, open: &[PaneId]| open[steps.next(open.len())];
                let direction = [
                    Direction::Left,
                    Direction::Right,
                    Direction::Up,
                    Direction::Down,
                ][steps.next(4)];
                let step = match steps.next(9) {
                    0 | 1 if open.len() < 12 => {
                        strip.apply(TabAction::Split(direction));
                        format!("split {direction:?}")
                    }
                    2 if open.len() > 1 => {
                        let pane = pick(&mut steps, &open);
                        let group = group(&strip);
                        let place = group.place_of(pane).expect("open");
                        let flex = group.get(pane).expect("open").flex();
                        strip.apply(TabAction::ClosePane(pane));
                        closed.push((place, flex));
                        format!("close {pane:?}")
                    }
                    3 => match closed.pop() {
                        Some((place, flex)) => {
                            let mut pane = Pane::new("back");
                            pane.set_flex(flex);
                            strip.reopen_pane(tab, pane, &place);
                            "reopen".to_owned()
                        }
                        None => continue,
                    },
                    4 => {
                        let pane = pick(&mut steps, &open);
                        strip.apply(TabAction::FocusPane(pane));
                        format!("focus {pane:?}")
                    }
                    5 => {
                        if let Some(next) = group(&strip).neighbour(direction) {
                            strip.apply(TabAction::FocusPane(next));
                        }
                        format!("walk {direction:?}")
                    }
                    6 => {
                        let panes = panes_of(&strip, tab);
                        let (before, after) = (pick(&mut steps, &panes), pick(&mut steps, &panes));
                        let leading = steps.next(101) as f32 / 100.;
                        strip.apply(TabAction::ResizePanes {
                            before,
                            after,
                            leading,
                        });
                        format!("resize {before:?} {after:?} {leading}")
                    }
                    7 => {
                        strip.apply(TabAction::NudgePane {
                            grow: steps.next(2) == 0,
                        });
                        "nudge".to_owned()
                    }
                    _ => {
                        strip.apply(TabAction::ZoomPane);
                        "zoom".to_owned()
                    }
                };
                open = panes_of(&strip, tab);
                assert_in_shape(
                    group(&strip),
                    &open,
                    &format!("seed {seed} turn {turn}: {step}"),
                );
            }
        }
    }
}
