//! What a notification says, and how often one pane may post one.
//!
//! Pure: no workspace, no desktop, and instants made up rather than waited
//! for. Whether the workspace posts at the right moments is tested where the
//! moments are, in `workspace::tests`, with a notifier that keeps a list.

use super::*;

/// A session in the state a row is amber in, as the workspace leaves it.
fn session(
    status: AgentStatus,
    message: Option<&str>,
    attention: Option<Attention>,
) -> AgentSession {
    let mut session = AgentSession::new("agent 1");
    session.status = status;
    session.message = message.map(str::to_owned);
    session.attention = attention;
    session.source = StatusSource::Agent(Instant::now());
    session
}

#[test]
fn a_notification_names_crook_and_the_tab() {
    let asking = session(
        AgentStatus::NeedsInput,
        Some("run rm -rf build?"),
        Some(Attention::StatusChange),
    );
    let notice = Notice::needs_input("port the tab bar", &asking);

    assert_eq!(notice.title, "Crook — port the tab bar");
    assert_eq!(notice.body, "run rm -rf build?");
}

#[test]
fn a_tab_with_no_name_is_still_crook() {
    assert_eq!(title("   "), "Crook");
}

#[test]
fn with_no_question_the_body_says_what_turned_the_row_amber() {
    let body = |session: &AgentSession| Notice::needs_input("a", session).body;
    let changed = Some(Attention::StatusChange);

    assert_eq!(
        body(&session(AgentStatus::NeedsInput, None, changed)),
        "Waiting for you"
    );
    // A question of nothing but whitespace is no question.
    assert_eq!(
        body(&session(AgentStatus::NeedsInput, Some(" \n "), changed)),
        "Waiting for you"
    );
    assert_eq!(
        body(&session(AgentStatus::Idle, None, Some(Attention::Bell))),
        "Rang the bell"
    );
    assert_eq!(
        body(&session(AgentStatus::Idle, None, changed)),
        "Done, waiting for a prompt"
    );

    let mut ended = session(AgentStatus::Idle, None, changed);
    ended.source = StatusSource::CommandEnded(Instant::now());
    assert_eq!(body(&ended), "Its command ended");
}

#[test]
fn a_long_question_is_cut_short_on_one_line() {
    let question = format!("run\n  {} now?", "x".repeat(400));
    let body = Notice::needs_input(
        "a",
        &session(AgentStatus::NeedsInput, Some(&question), None),
    )
    .body;

    assert_eq!(body.chars().count(), MESSAGE_CHARS);
    assert!(body.starts_with("run xxx"), "{body}");
    assert!(body.ends_with('…'), "{body}");
    assert!(!body.contains('\n'), "{body}");
}

#[test]
fn a_question_is_cut_between_two_letters_in_any_script() {
    // Four bytes a character, so a cut by bytes would land inside one.
    let question = "🦀".repeat(MESSAGE_CHARS * 2);
    let body = cut(&question);

    assert_eq!(body.chars().count(), MESSAGE_CHARS);
    assert!(
        body.chars()
            .rev()
            .skip(1)
            .all(|character| character == '🦀')
    );
}

#[test]
fn a_question_that_fits_is_left_whole() {
    let question = "y".repeat(MESSAGE_CHARS);
    assert_eq!(cut(&question), question);
}

#[test]
fn a_finished_command_says_how_long_and_how_it_ended() {
    assert_eq!(
        Notice::finished("build", Some(0), Duration::from_secs(72)).body,
        "A command finished after 1m 12s"
    );
    assert_eq!(
        Notice::finished("build", Some(101), Duration::from_secs(12)).body,
        "A command exited 101 after 12s"
    );
    assert_eq!(
        Notice::finished("build", None, Duration::from_secs(3 * 3600 + 5 * 60 + 9)).body,
        "A command finished after 3h 5m"
    );
}

#[test]
fn a_pane_that_posted_is_quiet_until_the_quiet_is_over() {
    let mut cooldown = Cooldown::default();
    let pane = PaneId::next();
    let start = Instant::now();

    assert!(cooldown.admits(pane, start));
    assert!(!cooldown.admits(pane, start + Duration::from_secs(1)));
    assert!(!cooldown.admits(pane, start + QUIET - Duration::from_millis(1)));
    assert!(
        cooldown.admits(pane, start + QUIET),
        "a pane asking again after the quiet is heard again"
    );
    assert!(
        !cooldown.admits(pane, start + QUIET + Duration::from_secs(1)),
        "and that starts a quiet of its own"
    );
}

#[test]
fn one_pane_being_quiet_holds_up_no_other() {
    let mut cooldown = Cooldown::default();
    let (first, second) = (PaneId::next(), PaneId::next());
    let now = Instant::now();

    assert!(cooldown.admits(first, now));
    assert!(cooldown.admits(second, now));
    assert!(!cooldown.admits(first, now));
}

#[test]
fn a_refused_post_does_not_lengthen_the_quiet() {
    // Counted from the notification that was posted, not from the last one
    // that was asked for: an agent flapping every second would otherwise be
    // quiet for as long as it kept flapping.
    let mut cooldown = Cooldown::default();
    let pane = PaneId::next();
    let start = Instant::now();

    assert!(cooldown.admits(pane, start));
    for second in 1..QUIET.as_secs() {
        assert!(!cooldown.admits(pane, start + Duration::from_secs(second)));
    }
    assert!(cooldown.admits(pane, start + QUIET));
}

#[test]
fn a_pane_somebody_looked_at_is_heard_again_at_once() {
    let mut cooldown = Cooldown::default();
    let (seen, unseen) = (PaneId::next(), PaneId::next());
    let start = Instant::now();
    assert!(cooldown.admits(seen, start));
    assert!(cooldown.admits(unseen, start));

    cooldown.seen(seen);

    let soon = start + Duration::from_secs(5);
    assert!(cooldown.admits(seen, soon), "its next stop is news");
    assert!(!cooldown.admits(unseen, soon), "and no other pane's is");
}

#[test]
fn panes_quiet_for_long_enough_are_forgotten() {
    let mut cooldown = Cooldown::default();
    let start = Instant::now();
    for _ in 0..8 {
        cooldown.admits(PaneId::next(), start);
    }

    cooldown.admits(PaneId::next(), start + QUIET);

    assert_eq!(cooldown.posted.len(), 1, "the closed panes are still held");
}

#[test]
fn a_mac_binary_outside_an_app_bundle_posts_no_banner() {
    // `cargo run`'s binary and the one `script/install` puts on PATH: macOS
    // delivers notifications to an application bundle and nothing else, and
    // asking for its notification center without one throws rather than
    // failing. Such a copy has the dock's bounce and badge instead.
    assert_eq!(Service::of("macos", None), Service::OutsideTheApp);
    assert_eq!(Service::of("macos", Some("")), Service::OutsideTheApp);
    assert!(!Service::OutsideTheApp.posts());
    // Nor is leave to badge asked for: the center it would be asked of is the
    // one that throws.
    assert!(!Service::OutsideTheApp.badges_with_leave());
}

#[test]
fn crook_app_posts_through_notification_center() {
    assert_eq!(
        Service::of("macos", Some("com.theguriev.crook")),
        Service::NotificationCenter
    );
    assert!(Service::NotificationCenter.posts());
    // And its dock badge is drawn only once the same leave has been asked for.
    assert!(Service::NotificationCenter.badges_with_leave());
}

#[test]
fn linux_posts_through_notify_send_and_windows_nowhere_yet() {
    assert_eq!(Service::of("linux", None), Service::NotifySend);
    assert!(Service::NotifySend.posts());

    // A bundle is macOS's idea, and says nothing about anywhere else.
    assert_eq!(
        Service::of("windows", Some("com.theguriev.crook")),
        Service::Nowhere
    );
    assert!(!Service::Nowhere.posts());
    assert!(!Service::NotifySend.badges_with_leave());
    assert!(!Service::Nowhere.badges_with_leave());
}
