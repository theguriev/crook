//! What a plugin is allowed to ask for, and where a path actually points.
//!
//! The two halves of this file are the two ways a grant can be wrong. One is
//! a request that names something nobody allowed — the ordinary case, and the
//! one a plugin is expected to hit on its first run. The other is a request
//! that names something a person *did* allow while meaning something else,
//! which is the case worth writing tests about.

use super::*;

use std::sync::Arc;

use crookui_core::executor::{Background, LocalQueue};
use crookui_core::{App, ModelHandle};

use crook_plugin::PluginId;
use crook_plugin_api::{Bound, Cell as Field, Key, Method, Request, Table, Tallied};
use crook_wasm::{Fuel, Sandbox};

/// What the pirate asks for, as it asks for it.
fn fetch(url: &str) -> Request {
    Request::Fetch {
        method: Method::Get,
        url: url.to_owned(),
        headers: vec![("authorization".into(), "Bearer …".into())],
        body: None,
    }
}

fn read(path: &str) -> Request {
    Request::ReadFile {
        path: path.to_owned(),
    }
}

#[test]
fn nothing_is_allowed_to_a_plugin_nobody_has_answered_for() {
    // The state every plugin installs in. A manifest asking for something is
    // not a person allowing it.
    let refusal = allowed(&[], &fetch("https://api.anthropic.com/api/oauth/usage"))
        .expect_err("an ungranted plugin should be refused");

    // And the refusal is the sentence the dialog used, so the plugin can say
    // what to go and allow rather than "something went wrong".
    assert_eq!(refusal, "Reach api.anthropic.com");
}

#[test]
fn a_granted_host_is_reached_and_its_neighbours_are_not() {
    let granted = vec![String::from("net:api.anthropic.com")];

    assert!(
        allowed(
            &granted,
            &fetch("https://api.anthropic.com/api/oauth/usage")
        )
        .is_ok()
    );
    // A different host on the same domain is a different host.
    assert!(allowed(&granted, &fetch("https://evil.anthropic.com/")).is_err());
    // And so is one that merely starts the same way.
    assert!(allowed(&granted, &fetch("https://api.anthropic.com.evil.example/")).is_err());
}

#[test]
fn a_url_that_hides_its_host_behind_a_name_somebody_granted() {
    // `https://api.anthropic.com@evil.example/` is a request to *evil.example*
    // with a username of `api.anthropic.com`. A check that read the front of
    // the authority would hand it a permission somebody gave to Anthropic.
    assert_eq!(
        host_of("https://api.anthropic.com@evil.example/x"),
        Ok(String::from("evil.example"))
    );
    assert!(
        allowed(
            &[String::from("net:api.anthropic.com")],
            &fetch("https://api.anthropic.com@evil.example/x")
        )
        .is_err()
    );
}

#[test]
fn a_host_is_the_authority_without_its_port_and_in_one_case() {
    assert_eq!(
        host_of("https://API.Anthropic.com:443/api/oauth/usage?x=1"),
        Ok(String::from("api.anthropic.com"))
    );
}

#[test]
fn only_https_is_reached_at_all() {
    // Not a capability question: there is no capability that grants sending a
    // token over a cleartext connection.
    assert!(host_of("http://api.anthropic.com/").is_err());
    assert!(host_of("file:///etc/passwd").is_err());
    assert!(host_of("https:///nothing").is_err());
}

#[test]
fn a_granted_path_is_the_path_that_was_granted() {
    let granted = vec![String::from("file:~/.claude/.credentials.json")];

    assert!(allowed(&granted, &read("~/.claude/.credentials.json")).is_ok());
    assert!(allowed(&granted, &read("~/.ssh/id_ed25519")).is_err());
    // Not a prefix, and not a directory: the grant is the text of one file.
    assert!(allowed(&granted, &read("~/.claude/")).is_err());
}

#[test]
fn a_path_that_can_walk_is_not_a_path_this_resolves() {
    // The grant is the text of the path, so a path that can walk out of it is
    // a grant that means something other than what a person read.
    assert_eq!(resolve("~/.claude/../.ssh/id_ed25519"), None);
    assert_eq!(resolve("/etc/../etc/passwd"), None);
    assert_eq!(resolve(".."), None);
}

#[test]
fn a_tilde_is_the_home_directory_and_nothing_else_is_expanded() {
    let home = dirs::home_dir().expect("this machine has a home directory");

    assert_eq!(
        resolve("~/.claude/.credentials.json"),
        Some(home.join(".claude/.credentials.json"))
    );
    // Only a leading `~/`. A file actually called `~` is a file called `~`.
    assert_eq!(
        resolve("/tmp/~/thing"),
        Some(std::path::PathBuf::from("/tmp/~/thing"))
    );
}

/// A runtime over a real module, on a pool with exactly one worker.
///
/// One worker is the whole point: it makes "how many workers does a plugin
/// hold?" a question a test can answer, because the second one to be held is
/// the one that never comes back.
fn on_one_worker() -> (App, Arc<LocalQueue>, Arc<Background>, ModelHandle<Runtime>) {
    let background = Arc::new(Background::new(1));
    let queue = LocalQueue::new();
    let mut app = App::new(queue.foreground(), background.clone());

    let module = crate::plugins::wasm::tests::wasm("eugen/probe", "header.right", 0);
    let (sandbox, _) = Sandbox::open(&module, Fuel::default()).expect("the test module opens");
    let runtime = app.update(|ctx| {
        ctx.add_model(|_| {
            Runtime::new(
                PluginId::parse("eugen/probe").expect("a literal that parses"),
                Rc::new(RefCell::new(sandbox)),
                Rc::new(Cell::new(0)),
                Vec::new(),
            )
        })
    });

    (app, queue, background, runtime)
}

#[test]
fn changing_its_mind_about_when_parks_one_worker_and_not_six() {
    // The bug this defends against, which took an hour of a person's session
    // to show itself: a shorter wait used to start a *second* chain and drop
    // the first task. Dropping cancels the callback and not the sleep, so a
    // worker sat inside `thread::sleep` for the rest of the minute. Six clicks
    // was six workers held, and everything else that needed one — the git
    // gather, a settings save, this plugin's own next tick — queued behind
    // them and then came back by itself, which is what made it so hard to see.
    let (mut app, queue, background, runtime) = on_one_worker();

    app.update(|ctx| {
        runtime.update(ctx, |runtime, ctx| {
            for _ in 0..6 {
                runtime.wait(Duration::from_secs(60), ctx);
            }
        })
    });
    queue.run_until_parked();

    // One worker, and a minute's wait parked on it. If more than one wait were
    // parked, this probe would never be run at all.
    let (ran, was_run) = std::sync::mpsc::channel();
    background
        .spawn(async move {
            let _ = ran.send(());
        })
        .detach();

    assert!(
        was_run.recv_timeout(Duration::from_secs(5)).is_ok(),
        "the pool's only worker is held by a wait nobody can wake"
    );
}

#[test]
fn a_wait_that_is_poked_ends_early_rather_than_at_its_own_time() {
    // The other half: poking has to actually shorten the wait, or a plugin
    // that asked for a minute and then asked for a frame would still be drawn
    // a minute later — which is a mark that stands still when it should bite.
    let (mut app, queue, _background, runtime) = on_one_worker();

    app.update(|ctx| {
        runtime.update(ctx, |runtime, ctx| {
            runtime.wait(Duration::from_secs(60), ctx);
            runtime.wait(Duration::from_millis(1), ctx);
        })
    });

    // The parked wait returns as soon as it is poked, so the tick it books
    // arrives now rather than in a minute. `run_until_parked` runs whatever
    // the wait's completion put on the foreground queue.
    let mut ticked = false;
    for _ in 0..50 {
        if queue.run_until_parked() > 0 {
            ticked = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    assert!(ticked, "a poked wait did not come back inside a second");
}

/// A runtime over a module with somewhere to put an answer, allowed to ask
/// where a pane is working — which is a deed, served by the window.
fn allowed_to_ask_where() -> (App, Arc<LocalQueue>, ModelHandle<Runtime>) {
    runtime_allowed(&["cwd.read"])
}

/// The same runtime, allowed `granted`.
fn runtime_allowed(granted: &[&str]) -> (App, Arc<LocalQueue>, ModelHandle<Runtime>) {
    let queue = LocalQueue::new();
    let mut app = App::new(queue.foreground(), Arc::new(Background::new(1)));

    let module = crate::plugins::wasm::tests::wasm_asking_where("eugen/probe", 0, 0, 0);
    let (sandbox, _) = Sandbox::open(&module, Fuel::default()).expect("the test module opens");
    let runtime = app.update(|ctx| {
        ctx.add_model(|_| {
            Runtime::new(
                PluginId::parse("eugen/probe").expect("a literal that parses"),
                Rc::new(RefCell::new(sandbox)),
                Rc::new(Cell::new(0)),
                granted.iter().copied().map(String::from).collect(),
            )
        })
    });

    (app, queue, runtime)
}

/// Asks where the pane is once per ticket, as a guest's calls would, on
/// `gesture`'s behalf.
fn ask_where(
    app: &mut App,
    runtime: &ModelHandle<Runtime>,
    tickets: std::ops::Range<u32>,
    gesture: Gesture,
) {
    app.update(|ctx| {
        runtime.update(ctx, |runtime, ctx| {
            for ticket in tickets {
                runtime.start(ticket, Request::Where, gesture, ctx);
            }
        })
    });
}

/// Asks for something to be put on the clipboard once per ticket, out of a
/// press: a deed that carries text, which only a press may ask for.
fn ask_to_copy(app: &mut App, runtime: &ModelHandle<Runtime>, tickets: std::ops::Range<u32>) {
    app.update(|ctx| {
        runtime.update(ctx, |runtime, ctx| {
            for ticket in tickets {
                let text = format!("copied {ticket}");
                runtime.start(ticket, Request::Copy { text }, Gesture::Pressed, ctx);
            }
        })
    });
}

/// The tickets one of the window's turns hands the observer: everything
/// `deeds` hands out, inside one update, before it hands out nothing — which
/// is what the observer's loop takes.
fn one_turn(app: &mut App, runtime: &ModelHandle<Runtime>) -> Vec<u32> {
    app.update(|ctx| {
        runtime.update(ctx, |runtime, ctx| {
            let mut turn = Vec::new();
            loop {
                let deeds = runtime.deeds(ctx);
                if deeds.is_empty() {
                    return turn;
                }
                turn.extend(deeds.into_iter().map(|(ticket, _)| ticket));
            }
        })
    })
}

/// Turn after turn, until one hands out nothing.
fn served_in_turns(app: &mut App, runtime: &ModelHandle<Runtime>) -> Vec<Vec<u32>> {
    let mut turns = Vec::new();
    loop {
        let turn = one_turn(app, runtime);
        if turn.is_empty() {
            return turns;
        }
        turns.push(turn);
        assert!(
            turns.len() < 100,
            "the turns never ran out of things to hand out"
        );
    }
}

/// How many deeds are waiting to be served.
fn waiting(app: &App, runtime: &ModelHandle<Runtime>) -> usize {
    runtime.read(app, |runtime, _| runtime.deeds.len())
}

#[test]
fn deeds_past_a_turn_wait_for_the_next_and_none_is_lost_or_served_twice() {
    // What the budget must not do. `deeds` drains, and a deed taken out and
    // not served is a ticket a guest waits on for the rest of the session —
    // so what one turn has no room for stays where it was, in the order it
    // was asked, and the next turn starts with it, ahead of what the answers
    // in between asked for.
    let (mut app, _queue, runtime) = allowed_to_ask_where();
    let full = DEEDS_PER_TURN;
    ask_where(&mut app, &runtime, 1..full + 9, Gesture::None);

    let first = one_turn(&mut app, &runtime);
    ask_where(&mut app, &runtime, full + 9..2 * full + 17, Gesture::None);
    let rest = served_in_turns(&mut app, &runtime);

    assert_eq!(first, (1..full + 1).collect::<Vec<_>>());
    assert_eq!(
        rest.iter().map(Vec::len).collect::<Vec<_>>(),
        [full as usize, 16],
        "the turns were not a turn's worth each"
    );
    assert_eq!(
        [first, rest.concat()].concat(),
        (1..2 * full + 17).collect::<Vec<_>>(),
        "a ticket was dropped, served twice or served out of order"
    );
}

#[test]
fn a_queue_found_empty_is_not_the_end_of_a_turn() {
    // Two plugins that run each other, as this one sees it: each thing it
    // asks for is served at once, so its queue is empty every time it is
    // looked at, and the next thing arrives in the same turn because the other
    // plugin asked for it. A budget that started again whenever the queue was
    // empty was never spent, and the window never got its turn.
    let (mut app, _queue, runtime) = allowed_to_ask_where();
    let asked = DEEDS_PER_TURN + 8;

    let handed = app.update(|ctx| {
        runtime.update(ctx, |runtime, ctx| {
            let mut handed = Vec::new();
            for ticket in 1..asked + 1 {
                runtime.start(ticket, Request::Where, Gesture::None, ctx);
                handed.extend(runtime.deeds(ctx).into_iter().map(|(ticket, _)| ticket));
                assert!(runtime.deeds(ctx).is_empty(), "a ticket was handed twice");
            }
            handed
        })
    });

    assert_eq!(
        handed,
        (1..DEEDS_PER_TURN + 1).collect::<Vec<_>>(),
        "one turn was handed more than a turn's worth"
    );
    assert_eq!(
        waiting(&app, &runtime),
        8,
        "what the turn had no room for was not left waiting"
    );
    assert_eq!(
        one_turn(&mut app, &runtime),
        (DEEDS_PER_TURN + 1..asked + 1).collect::<Vec<_>>(),
        "the next turn did not start with what was left"
    );
}

#[test]
fn a_burst_that_ends_is_forgiven_the_turns_it_took() {
    // "In a row" is the whole rule: a turn that ends with nothing left
    // waiting ended a chain, and a chain that ends was not the loop. A plugin
    // whose bursts are each a little more than a turn, more times than a
    // plugin may overrun, is served every one of them.
    let (mut app, _queue, runtime) = allowed_to_ask_where();
    let burst = DEEDS_PER_TURN + 1;

    for round in 0..=OVERRUNS_ALLOWED {
        let first = round * burst + 1;
        ask_where(&mut app, &runtime, first..first + burst, Gesture::None);

        let served = served_in_turns(&mut app, &runtime).concat();

        assert_eq!(
            served,
            (first..first + burst).collect::<Vec<_>>(),
            "burst {round} was not served whole"
        );
    }
}

#[test]
fn a_plugin_still_asking_after_its_turns_run_out_stops_being_served() {
    // The loop's end, as the observer sees a guest that asks again from every
    // answer: each turn is spent with something still waiting, because the
    // answers asked for as much again. The turn that runs out for the
    // sixteenth time in a row is the last it is served, and what it asks for
    // afterwards is not kept for anybody.
    let (mut app, _queue, runtime) = allowed_to_ask_where();
    let full = DEEDS_PER_TURN;
    ask_where(&mut app, &runtime, 1..full + 2, Gesture::None);

    let mut served = Vec::new();
    let mut next = full + 2;
    for _ in 0..OVERRUNS_ALLOWED + 3 {
        served.extend(one_turn(&mut app, &runtime));
        ask_where(&mut app, &runtime, next..next + full, Gesture::None);
        next += full;
    }

    assert_eq!(
        served,
        (1..OVERRUNS_ALLOWED * full + 1).collect::<Vec<_>>(),
        "it was not served its sixteen turns and then stopped"
    );
    assert_eq!(
        waiting(&app, &runtime),
        0,
        "what it asked for afterwards was kept"
    );
}

#[test]
fn a_plugin_with_more_to_copy_waiting_than_two_turns_stops_being_served_at_once() {
    // A turn bounds what is served and not what is kept. A guest that asks
    // for thirty-two from every answer leaves a thousand behind after one
    // turn, and a copy or a line to type may be a megabyte each, so what
    // carries text has a ceiling: as many as may wait are kept, a turn served
    // makes room for as many again, one more is the plugin caught, and
    // nothing it had waiting is kept or remembered as pressed after that.
    let (mut app, _queue, runtime) = runtime_allowed(&["cwd.read", "clipboard"]);
    let most = MAX_WAITING as u32;
    ask_to_copy(&mut app, &runtime, 1..most + 1);
    assert_eq!(
        waiting(&app, &runtime),
        MAX_WAITING,
        "as many as may wait were not all kept"
    );

    let served = one_turn(&mut app, &runtime);
    let next = most + 1 + served.len() as u32;
    ask_to_copy(&mut app, &runtime, most + 1..next);
    // A question carries nothing, so it does not take a copy's room.
    ask_where(&mut app, &runtime, next..next + 1, Gesture::Pressed);
    assert_eq!(
        waiting(&app, &runtime),
        MAX_WAITING + 1,
        "what a turn served did not make room for as much again"
    );

    ask_to_copy(&mut app, &runtime, next + 1..next + 2);

    assert_eq!(waiting(&app, &runtime), 0, "what was waiting was kept");
    assert_eq!(
        runtime.read(&app, |runtime, _| runtime.pressed.clone()),
        served,
        "a ticket nobody will answer is still remembered as pressed"
    );
    assert!(
        one_turn(&mut app, &runtime).is_empty(),
        "it was still served"
    );
    ask_where(&mut app, &runtime, next + 2..next + 3, Gesture::None);
    assert_eq!(
        waiting(&app, &runtime),
        0,
        "what it asked for afterwards was kept"
    );
}

#[test]
fn a_burst_of_questions_bigger_than_may_wait_is_served_whole() {
    // Where the pane is carries nothing to keep, and it is what an event may
    // ask the window for — and events come in batches, every one delivered
    // before the first thing they asked for is served. A plugin that asks
    // once from each of a burst has as many waiting as the burst, none of
    // them raised by an answer, and they are served a turn at a time rather
    // than taken for a queue that is growing.
    let (mut app, _queue, runtime) = allowed_to_ask_where();
    let full = DEEDS_PER_TURN as usize;
    let burst = 2 * MAX_WAITING as u32 + 1;
    ask_where(&mut app, &runtime, 1..burst + 1, Gesture::None);
    assert_eq!(
        waiting(&app, &runtime),
        burst as usize,
        "some of the burst was not kept"
    );

    let turns = served_in_turns(&mut app, &runtime);

    assert_eq!(
        turns.iter().map(Vec::len).collect::<Vec<_>>(),
        [full, full, full, full, 1],
        "the burst was not served a turn's worth at a time"
    );
    assert_eq!(
        turns.concat(),
        (1..burst + 1).collect::<Vec<_>>(),
        "a ticket was dropped, served twice or served out of order"
    );
}

/// A directory of line-delimited JSON, written for one test.
fn transcripts(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "crook-scan-{}-{}-{:?}",
        std::process::id(),
        name,
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("one")).expect("a scratch directory");
    fs::create_dir_all(root.join("two")).expect("a scratch directory");

    // Two turns worth keeping, one line that is not a turn, and one that is
    // not JSON at all — which is what a file something is still writing looks
    // like when it is read half way through a line.
    fs::write(
        root.join("one/a.jsonl"),
        concat!(
            r#"{"type":"user","message":{"content":"hello"}}"#, "\n",
            r#"{"type":"assistant","timestamp":"2026-09-04T18:00:00Z","message":{"id":"m1","model":"claude-opus-5","usage":{"output_tokens":12}}}"#, "\n",
            r#"{"type":"assistant","timestamp":"2026-09-04T19:00:00Z","message":{"id":"m2","model":"claude-fable-5-1","usage":{"output_tokens":34}}}"#, "\n",
            r#"{"type":"assistant","usage":"#, "\n",
        ),
    )
    .expect("a scratch file");
    fs::write(
        root.join("two/b.jsonl"),
        format!(
            "{}\n",
            r#"{"type":"assistant","timestamp":"2026-09-05T09:00:00Z","message":{"id":"m3","model":"claude-haiku-4-5","usage":{"output_tokens":5}}}"#
        ),
    )
    .expect("a scratch file");
    // Not a transcript, and never opened.
    fs::write(root.join("one/notes.txt"), "\"usage\"\n").expect("a scratch file");

    root
}

/// What a tally of that directory asks for: turns per model, and per hour.
fn tally_of(root: &Path) -> Request {
    Request::Tally {
        root: root.to_string_lossy().into_owned(),
        extension: ".jsonl".into(),
        touched_since: 0,
        containing: "\"usage\"".into(),
        at_least: Vec::new(),
        distinct_by: vec!["message.id".into()],
        tables: vec![
            Table {
                by: vec![Key {
                    field: "message.model".into(),
                    prefix: None,
                }],
                sum: vec!["message.usage.output_tokens".into()],
            },
            Table {
                by: vec![Key {
                    field: "timestamp".into(),
                    // Thirteen characters of an RFC 3339 stamp is its hour,
                    // which is how a plugin asks for "by the hour" without the
                    // host knowing what a date is.
                    prefix: Some(13),
                }],
                sum: vec!["message.usage.output_tokens".into()],
            },
        ],
    }
}

/// The rows of one table, sorted so a test can name them.
fn rows_of(tables: &[Vec<Tallied>], table: usize) -> Vec<(String, f64, u64)> {
    let mut rows: Vec<(String, f64, u64)> = tables[table]
        .iter()
        .map(|row| {
            let key = match row.key.first() {
                Some(Field::Text(text)) => text.clone(),
                other => format!("{other:?}"),
            };
            (key, row.sums[0], row.lines)
        })
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

#[test]
fn a_tally_counts_where_it_reads_and_hands_back_the_answer() {
    // The whole point of the request: a hundred megabytes of transcripts is a
    // hundred megabytes wherever it is read, and handing a sandbox the *lines*
    // was measured at ninety thousand instructions each.
    let root = transcripts("counted");

    let Answer::Counted { tables, lines } = perform(tally_of(&root)) else {
        panic!("a tally answers with tables");
    };

    assert_eq!(lines, 3);
    assert_eq!(
        rows_of(&tables, 0),
        vec![
            (String::from("claude-fable-5-1"), 34., 1),
            (String::from("claude-haiku-4-5"), 5., 1),
            (String::from("claude-opus-5"), 12., 1),
        ]
    );
}

#[test]
fn a_prefix_is_how_a_plugin_asks_for_by_the_hour() {
    let root = transcripts("hours");

    let Answer::Counted { tables, .. } = perform(tally_of(&root)) else {
        panic!("a tally answers with tables");
    };

    assert_eq!(
        rows_of(&tables, 1),
        vec![
            (String::from("2026-09-04T18"), 12., 1),
            (String::from("2026-09-04T19"), 34., 1),
            (String::from("2026-09-05T09"), 5., 1),
        ]
    );
}

#[test]
fn a_line_written_twice_is_counted_once() {
    let root = transcripts("twice");
    // The same turn again, in another session's file — which is what a resumed
    // conversation leaves on disk.
    fs::write(
        root.join("two/c.jsonl"),
        format!(
            "{}\n",
            r#"{"type":"assistant","timestamp":"2026-09-04T18:00:00Z","message":{"id":"m1","model":"claude-opus-5","usage":{"output_tokens":12}}}"#
        ),
    )
    .expect("a scratch file");
    fs::write(
        root.join("one/a.jsonl"),
        format!(
            "{}\n",
            r#"{"type":"assistant","timestamp":"2026-09-04T18:00:00Z","message":{"id":"m1","model":"claude-opus-5","usage":{"output_tokens":12}}}"#
        ),
    )
    .expect("a scratch file");

    let Answer::Counted { lines, tables } = perform(tally_of(&root)) else {
        panic!("a tally answers with tables");
    };

    assert_eq!(lines, 2, "the file with no id in it is counted too");
    assert_eq!(
        rows_of(&tables, 0)
            .iter()
            .find(|(model, _, _)| model == "claude-opus-5")
            .map(|(_, sum, _)| *sum),
        Some(12.),
        "the same turn was added twice"
    );
}

#[test]
fn a_line_below_the_floor_is_not_counted() {
    // `touched_since` skips files, which is what makes the walk cheap; a file
    // touched an hour ago can still hold lines from a month ago, and without
    // this they would quietly be in the total.
    let root = transcripts("floor");
    let Request::Tally {
        root: at,
        extension,
        containing,
        distinct_by,
        tables,
        ..
    } = tally_of(&root)
    else {
        panic!("that is a tally");
    };

    let Answer::Counted { lines, .. } = perform(Request::Tally {
        root: at,
        extension,
        touched_since: 0,
        containing,
        at_least: vec![Bound {
            field: "timestamp".into(),
            at_least: "2026-09-05T00:00:00Z".into(),
        }],
        distinct_by,
        tables,
    }) else {
        panic!("a tally answers with tables");
    };

    assert_eq!(lines, 1, "only the turn on the 5th is above the floor");
}

#[test]
fn walking_a_directory_is_a_different_thing_to_agree_to_than_reading_a_file() {
    let root = transcripts("granted");
    let tally = tally_of(&root);
    let inside = root.join("one/a.jsonl").to_string_lossy().into_owned();

    // Allowing every file in the directory does not allow walking it: a person
    // agreeing to "read this file" has pictured one file.
    let file_by_file = vec![format!("file:{inside}")];
    assert!(allowed(&file_by_file, &tally).is_err());

    // And the sentence a person is asked to agree to says what it means rather
    // than spelling a pattern.
    let refusal = allowed(&[], &tally).expect_err("nothing is allowed yet");
    assert!(refusal.starts_with("Read everything under "), "{refusal}");

    let granted = vec![format!("file:{}/**", root.to_string_lossy())];
    assert!(allowed(&granted, &tally).is_ok());

    // And the grant names this directory, not one above it: the crate's docs
    // tell a plugin author exactly that, so a host that started accepting a
    // parent's `/**` would be widening what people agreed to behind them.
    let parent = root.parent().expect("the transcripts are in a directory");
    let above = vec![format!("file:{}/**", parent.to_string_lossy())];
    assert!(allowed(&above, &tally).is_err());
}

/// A week of this machine's own transcripts, counted.
///
/// Ignored, and it has to be: what it walks is whatever Claude Code has
/// written on the machine it runs on, so it proves nothing on a build server
/// and everything on the one machine where the numbers can be checked against
/// what a person knows they did. Run it by name when the tally changes.
///
/// It is the only test that can answer the question the request exists for —
/// is a hundred megabytes of transcripts affordable — because a fixture with
/// four lines in it cannot.
#[test]
#[ignore = "walks this machine's own Claude Code transcripts"]
fn a_week_of_this_machine_is_counted_where_it_is_read() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let root = home.join(".claude/projects");
    if !root.is_dir() {
        return;
    }

    let week = 7 * 86_400_000_i64;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default();
    let sums: Vec<String> = [
        "message.usage.input_tokens",
        "message.usage.output_tokens",
        "message.usage.cache_creation_input_tokens",
        "message.usage.cache_read_input_tokens",
    ]
    .iter()
    .map(|field| String::from(*field))
    .collect();
    let by = |subject: &str, prefix: Option<u32>| {
        vec![
            Key {
                field: "type".into(),
                prefix: None,
            },
            Key {
                field: subject.into(),
                prefix,
            },
        ]
    };

    let at = std::time::Instant::now();
    let answer = perform(Request::Tally {
        root: root.to_string_lossy().into_owned(),
        extension: ".jsonl".into(),
        touched_since: now - week,
        containing: "\"usage\"".into(),
        at_least: Vec::new(),
        distinct_by: vec!["message.id".into(), "requestId".into()],
        tables: vec![
            Table {
                by: by("message.model", None),
                sum: sums.clone(),
            },
            Table {
                by: by("timestamp", Some(13)),
                sum: sums.clone(),
            },
            Table {
                by: by("sessionId", None),
                sum: Vec::new(),
            },
        ],
    });
    let took = at.elapsed();

    let Answer::Counted { tables, lines } = answer else {
        panic!("a tally answers with tables");
    };
    let rows: usize = tables.iter().map(Vec::len).sum();
    println!(
        "{lines} turns counted into {rows} rows in {took:?} ({} models, {} hours, {} sessions)",
        tables[0].len(),
        tables[1].len(),
        tables[2].len()
    );

    // What crosses the boundary is the answer, not the reading: a plugin that
    // was handed the lines instead was measured at ninety thousand
    // instructions each, which for a week is forty seconds of interpreter.
    assert!(rows < lines as usize / 20, "{rows} rows for {lines} turns");
    // And the walk is one a person waits through once, not one they notice.
    assert!(took < std::time::Duration::from_secs(20), "{took:?}");
}

#[test]
fn a_sound_is_refused_to_a_plugin_nobody_has_answered_for() {
    // The whole of the dead Play button, in one line. The press lands, the
    // guest runs, it asks for its sound — and the asking stops here, because
    // nobody ever answered the question its card is still asking.
    let refusal = allowed(
        &[],
        &Request::PlaySound {
            wav: Vec::new(),
            volume: 70,
        },
    )
    .expect_err("an ungranted plugin should be refused its sound");

    assert_eq!(refusal, "Play a sound");
}

/// A listing of one directory, as the picker asks for it.
fn list(path: &str) -> Request {
    Request::List {
        path: path.to_owned(),
    }
}

/// One command typed, as choosing a row asks for it.
fn typed(template: &str, argument: &str) -> Request {
    Request::Type {
        template: template.to_owned(),
        argument: argument.to_owned(),
    }
}

#[test]
fn a_listing_is_allowed_anywhere_under_a_root_somebody_granted() {
    // The one grant that is not a string comparison: what was allowed is a
    // root and what is asked for is somewhere under it.
    let granted = vec![String::from("list:~")];

    assert!(allowed(&granted, &list("~/Work/crook")).is_ok());
    assert!(allowed(&granted, &list("~")).is_ok());
    // And the same directory written the way the host resolves it, because
    // `~/Work` and `/home/…/Work` are one directory and a plugin that asked
    // for either asked for what it was allowed.
    if let Some(home) = dirs::home_dir() {
        let inside = home.join("Work").to_string_lossy().into_owned();
        assert!(allowed(&granted, &list(&inside)).is_ok());
    }
}

#[test]
fn a_listing_outside_every_granted_root_is_refused_by_the_sentence_it_wanted() {
    let granted = vec![String::from("list:~/Work")];

    let refusal =
        allowed(&granted, &list("/etc")).expect_err("nothing granted /etc and it is not under ~");

    assert_eq!(refusal, "See the names of the files in /etc");

    // And "under" is by whole directory: a sibling that only *starts* with
    // the root's name is somewhere else, which a comparison of the text would
    // have let through.
    assert!(allowed(&granted, &list("~/Workshop")).is_err());
    assert!(allowed(&granted, &list("~/Work-secrets/keys")).is_err());
    if let Some(home) = dirs::home_dir() {
        let sibling = home.join("Workshop").to_string_lossy().into_owned();
        assert!(allowed(&granted, &list(&sibling)).is_err());
    }
}

#[test]
fn a_listing_cannot_walk_out_of_the_root_it_was_granted() {
    // The same rule `resolve` applies to a file: a path that can walk is a
    // grant that means something other than what it says. Refused rather than
    // normalised, so there is nothing to get subtly wrong.
    let granted = vec![String::from("list:~/Work")];

    assert!(allowed(&granted, &list("~/Work/../.ssh")).is_err());
}

#[test]
fn only_the_exact_command_somebody_allowed_may_be_typed() {
    // A template is the *shape* of the command, and it is the shape a person
    // agreed to. A plugin that was allowed to `cd` may not `git push`, however
    // it spells it.
    let granted = vec![String::from("type:cd {}")];

    assert!(allowed(&granted, &typed("cd {}", "/tmp")).is_ok());
    assert_eq!(
        allowed(&granted, &typed("git push {}", "--force")).expect_err("nobody allowed that one"),
        "Type into your shell, and run: git push \u{2026}"
    );
}

#[test]
fn a_command_of_crooks_own_is_allowed_by_name_and_no_other() {
    let granted = vec![String::from("run:crook/shortcuts/rebind")];
    let run = |name: &str| Request::Run {
        name: name.to_owned(),
        argument: String::new(),
    };

    assert!(allowed(&granted, &run("crook/shortcuts/rebind")).is_ok());
    assert!(allowed(&granted, &run("crook/window/close-window")).is_err());
}

#[test]
fn the_requests_that_need_a_press_are_the_ones_that_act_or_are_about_a_press() {
    // What makes typing into somebody's shell an acceptable thing for a
    // stranger's plugin to be able to do: it only happens when a person did
    // something. The clipboard is the same argument, and so is reading a
    // block — that answer is *about* the menu an entry was pressed in, and
    // there is no such menu when nobody pressed anything.
    assert!(only_from_a_gesture(&typed("cd {}", "/tmp")));
    assert!(only_from_a_gesture(&Request::Run {
        name: String::from("crook/window/new-tab"),
        argument: String::new(),
    }));
    assert!(only_from_a_gesture(&Request::Copy {
        text: String::from("anything"),
    }));
    assert!(only_from_a_gesture(&Request::Output));

    // Ordinary reading is not on the list: a chip that says where you are has
    // to be able to ask on a timer.
    assert!(!only_from_a_gesture(&Request::Where));
    assert!(!only_from_a_gesture(&list("~")));
    assert!(!only_from_a_gesture(&Request::Commands));
}

#[test]
fn reading_a_block_and_writing_the_clipboard_are_asked_for_like_anything_else() {
    // Neither reaches a socket or a file, and neither is therefore free: what
    // a plugin may read about the command a person ran it on, and what it may
    // put on their clipboard, are answers a person gives once and the host
    // checks every time.
    let nothing: Vec<String> = Vec::new();

    assert_eq!(
        allowed(&nothing, &Request::Output).expect_err("nobody has allowed it"),
        "Read the command you run it on, and what it printed"
    );
    assert_eq!(
        allowed(
            &nothing,
            &Request::Copy {
                text: String::from("anything"),
            }
        )
        .expect_err("nobody has allowed it"),
        "Read and change your clipboard"
    );

    let granted = vec![String::from("block.read")];
    assert!(allowed(&granted, &Request::Output).is_ok());
    assert!(
        allowed(
            &granted,
            &Request::Copy {
                text: String::from("anything"),
            }
        )
        .is_err(),
        "one grant is not the other"
    );
}

#[test]
fn a_listing_answers_with_the_names_and_nothing_about_them() {
    let scratch = super::super::tests::Scratch::new("listing");
    std::fs::create_dir_all(scratch.path().join("app")).expect("a directory should be creatable");
    std::fs::write(scratch.path().join("Cargo.toml"), b"[package]\n")
        .expect("a file should be writable");

    let answer = list_directory(&scratch.path().to_string_lossy());

    let Answer::Listed(entries) = answer else {
        panic!("a listing should answer with names, not {answer:?}");
    };
    // Directories first, then files, each by name: the order every file
    // picker has used since the first one.
    assert_eq!(
        entries,
        vec![
            Entry {
                name: String::from("app"),
                directory: true
            },
            Entry {
                name: String::from("Cargo.toml"),
                directory: false
            },
        ]
    );
}

/// Claude Code's credentials file is the keychain item on macOS, however the
/// path was spelled, and no other file is.
#[cfg(target_os = "macos")]
#[test]
fn claude_codes_credentials_are_read_from_the_keychain_on_macos() {
    let file = resolve("~/.claude/.credentials.json").expect("there is a home directory");
    assert_eq!(keychain_item_for(&file), Some("Claude Code-credentials"));
    assert_eq!(
        keychain_item_for(&resolve("~/.claude/settings.json").expect("home")),
        None
    );
    assert_eq!(keychain_item_for(Path::new("/etc/hosts")), None);
}

/// Against the real keychain, where Claude Code is signed in on this machine:
/// what is served is the JSON the file would hold, not the file.
#[cfg(target_os = "macos")]
#[test]
fn the_keychain_item_is_served_as_the_file_would_be() {
    let file = resolve("~/.claude/.credentials.json").expect("there is a home directory");
    let Some(bytes) = from_the_keychain(&file) else {
        eprintln!("skipped: Claude Code is not signed in on this machine");
        return;
    };
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("the item is JSON");
    assert!(
        json.get("claudeAiOauth").is_some(),
        "the shape the file has"
    );
    assert!(matches!(
        super::read("~/.claude/.credentials.json"),
        Answer::Read { .. }
    ));
}
