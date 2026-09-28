//! What can be checked about posting on Linux without posting anything.
//!
//! No test here runs `notify-send`: one that did would put a banner on the
//! screen of whoever runs the suite. What is tested is everything before the
//! spawn — what the program is told, and what happens when it is not there —
//! which is where the mistakes are.

use super::*;

fn notice() -> Notice {
    Notice {
        title: "Crook — port the tab bar".to_owned(),
        body: "run rm -rf build?".to_owned(),
    }
}

#[test]
fn a_notification_names_crook_and_the_entry_a_desktop_knows_it_by() {
    let arguments = arguments(&notice());

    assert!(arguments.contains(&"--app-name=Crook".to_owned()));
    assert!(arguments.contains(&"--hint=string:desktop-entry:crook".to_owned()));
}

#[test]
fn a_question_that_starts_with_a_dash_is_text_and_not_a_flag() {
    // An agent's words go after `--`, where `notify-send` reads nothing as a
    // flag: without it "-u critical" in a question would be an urgency.
    let arguments = arguments(&Notice {
        title: "-t 0".to_owned(),
        body: "-u critical".to_owned(),
    });

    assert_eq!(
        arguments[arguments.len() - 3..],
        ["--".to_owned(), "-t 0".to_owned(), "-u critical".to_owned()]
    );
}

#[test]
fn no_flag_is_passed_that_an_older_notify_send_would_refuse() {
    // One unknown flag and the whole command fails, which is a notification
    // that never arrives and says nothing about why.
    let arguments = arguments(&notice());
    let flags: Vec<&String> = arguments
        .iter()
        .take_while(|argument| *argument != "--")
        .collect();

    for newer in [
        "--print-id",
        "--replace-id",
        "--wait",
        "--action",
        "--transient",
    ] {
        assert!(
            !flags.iter().any(|flag| flag.starts_with(newer)),
            "{newer} is passed: {flags:?}"
        );
    }
    assert_eq!(flags.len(), 2, "{flags:?}");
}

#[test]
fn markup_in_an_agents_question_is_shown_rather_than_read() {
    // A link in a banner with Crook's name on it is the thing to refuse, and
    // a bare ampersand is markup that does not parse.
    let arguments = arguments(&Notice {
        title: "Crook — a <b>tab</b>".to_owned(),
        body: r#"<a href="https://x.invalid">log in</a> && go"#.to_owned(),
    });

    assert_eq!(
        arguments.last().map(String::as_str),
        Some(r#"&lt;a href="https://x.invalid"&gt;log in&lt;/a&gt; &amp;&amp; go"#)
    );
    assert_eq!(
        arguments[arguments.len() - 2],
        "Crook — a <b>tab</b>",
        "the title is never read as markup, so it is left as it was"
    );
}

#[test]
fn a_backslash_in_an_agents_question_is_shown_as_written() {
    // `notify-send` reads the body, and not the title, through
    // `g_strcompress`: an unescaped `\n` is a line break again, `\d` loses
    // its backslash and `\0` ends the body there. Each one is doubled, which
    // `g_strcompress` turns back into the one that was written.
    let arguments = arguments(&Notice {
        title: r"Crook — C:\new".to_owned(),
        body: r#"Allow grep -E "\bfoo\b" in C:\Users\me\0?"#.to_owned(),
    });

    assert_eq!(
        arguments.last().map(String::as_str),
        Some(r#"Allow grep -E "\\bfoo\\b" in C:\\Users\\me\\0?"#)
    );
    assert_eq!(
        arguments[arguments.len() - 2],
        r"Crook — C:\new",
        "the title is passed on as it is, so it is left as it was"
    );
}

#[test]
fn a_program_that_is_not_installed_is_tried_once() {
    let missing = AtomicBool::new(false);

    assert_eq!(
        deliver("crook-test-no-such-notify-send", &notice(), &missing),
        Delivery::Missing
    );
    assert!(missing.load(Ordering::Relaxed), "and it is remembered");
    assert_eq!(
        deliver("crook-test-no-such-notify-send", &notice(), &missing),
        Delivery::Skipped,
        "the second question starts nothing"
    );
}

#[test]
fn a_notifier_that_found_nothing_hands_the_pool_nothing() {
    // The main thread's half of the same rule: once a pool task has found the
    // program missing, posting schedules no task at all.
    let notifier = NotifySend::running("crook-test-no-such-notify-send");
    notifier.missing.store(true, Ordering::Relaxed);
    let before = Arc::strong_count(&notifier.missing);

    // The pool's one worker is held, so a task handed the flag would still be
    // queued holding it when it is counted, however quickly it would run.
    let pool = Background::new(1);
    let (release, held) = std::sync::mpsc::channel::<()>();
    let (busy, working) = std::sync::mpsc::channel::<()>();
    pool.spawn(async move {
        let _ = busy.send(());
        let _ = held.recv();
    })
    .detach();
    working.recv().expect("the worker picked the task up");

    notifier.post(notice(), &pool);

    assert_eq!(
        Arc::strong_count(&notifier.missing),
        before,
        "a task was handed the flag"
    );
    let _ = release.send(());
}
