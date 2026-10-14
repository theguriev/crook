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
            Runtime::working_on(
                Arc::new(Background::new(1)),
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
    let runtime = runtime_in(&mut app, granted, &Arc::new(Background::new(1)));
    (app, queue, runtime)
}

/// One more such runtime in `app`, allowed `granted`, whose requests are
/// worked on in `pool` — which is the plugins' pool, standing in for
/// [`requests`] so that a test can watch it.
fn runtime_in(app: &mut App, granted: &[&str], pool: &Arc<Background>) -> ModelHandle<Runtime> {
    let module = crate::plugins::wasm::tests::wasm_asking_where("eugen/probe", 0, 0, 0);
    let (sandbox, _) = Sandbox::open(&module, Fuel::default()).expect("the test module opens");
    app.update(|ctx| {
        ctx.add_model(|_| {
            Runtime::working_on(
                pool.clone(),
                PluginId::parse("eugen/probe").expect("a literal that parses"),
                Rc::new(RefCell::new(sandbox)),
                Rc::new(Cell::new(0)),
                granted.iter().copied().map(String::from).collect(),
            )
        })
    })
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

/// How soon "at once" has to be, for a test on a runner that is busy.
///
/// What these tests tell apart is a task started now from one started when a
/// fetch gives up, which is [`REQUEST_TIMEOUT`] — fifteen seconds. A frame
/// is what a person would notice, and an idle worker answers inside one, but
/// a test binary running every other test beside this one on a loaded runner
/// cannot promise a frame; it can promise a second.
const PROMPTLY: Duration = Duration::from_secs(1);

/// A host on this machine that takes every connection and never says a
/// word: a fetch to it waits out the whole of [`REQUEST_TIMEOUT`], which is a
/// host that drops packets without needing the network to have one.
///
/// Dropped, it closes the listener and everything it accepted, and every
/// fetch still waiting on it fails at once — which is what lets a pool
/// holding them be dropped at the end of a test rather than fifteen seconds
/// after it, and why every test makes its hosts after its pools: what is
/// made last is dropped first.
struct Silent {
    listener: std::net::TcpListener,
    held: Vec<std::net::TcpStream>,
}

impl Silent {
    fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("a port on this machine to listen on");
        listener
            .set_nonblocking(true)
            .expect("a listener that can be asked without waiting");
        Self {
            listener,
            held: Vec::new(),
        }
    }

    /// A fetch from it. The grant is the host, so `net:127.0.0.1` allows
    /// every one of these whatever its port.
    ///
    /// Without [`fetch`]'s header, whose `…` is not a byte a header may
    /// carry: that fetch fails as soon as it has connected, before it can
    /// wait on anything, and a test of what a hanging fetch holds would pass
    /// with nothing hanging.
    fn fetch(&self) -> Request {
        let port = self
            .listener
            .local_addr()
            .expect("a bound listener has an address")
            .port();
        Request::Fetch {
            method: Method::Get,
            url: format!("https://127.0.0.1:{port}/usage"),
            headers: Vec::new(),
            body: None,
        }
    }

    /// Whether a fetch reaches it within `within`, holding the connection
    /// open — and silent — if one does.
    fn reached(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            match self.listener.accept() {
                Ok((connection, _)) => {
                    self.held.push(connection);
                    return true;
                }
                Err(why) if why.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(why) => panic!("the listener broke: {why}"),
            }
        }
    }

    /// Hangs up on every fetch it is holding, which fails each at once.
    fn hang_up(&mut self) {
        self.held.clear();
    }
}

/// Asks for `requests` in turn, on nobody's behalf, as a tick would, with
/// tickets counting up from `first`.
fn ask_for(app: &mut App, runtime: &ModelHandle<Runtime>, first: u32, requests: Vec<Request>) {
    app.update(|ctx| {
        runtime.update(ctx, |runtime, ctx| {
            for (ticket, request) in (first..).zip(requests) {
                runtime.start(ticket, request, Gesture::None, ctx);
            }
        })
    });
}

/// The tickets a runtime has queued, and how many of its requests are on
/// the pool.
fn queued_and_running(app: &App, runtime: &ModelHandle<Runtime>) -> (Vec<u32>, usize) {
    runtime.read(app, |runtime, _| {
        let queued = runtime.queued.iter().map(|(ticket, _)| *ticket).collect();
        (queued, runtime.running)
    })
}

/// Asserts that none of a runtime's requests has been answered: that the
/// fetches a test set hanging are hanging.
///
/// Without it a test of what a hanging fetch holds passes just as well with
/// fetches that fail at once and hold nothing. A tenth of a second is long
/// enough for one that fails to have come back, and a hanging one takes
/// [`REQUEST_TIMEOUT`].
fn still_hanging(queue: &LocalQueue, app: &App, runtime: &ModelHandle<Runtime>, asked: usize) {
    std::thread::sleep(Duration::from_millis(100));
    queue.run_until_parked();
    let (queued, running) = queued_and_running(app, runtime);
    assert_eq!(
        queued.len() + running,
        asked,
        "fetches were answered, so they were not hanging and nothing here waited behind them"
    );
}

/// Runs what lands on the foreground until `done`, or for five seconds.
fn until(queue: &LocalQueue, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        queue.run_until_parked();
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_plugin_whose_fetches_hang_leaves_the_windows_pool_to_the_window() {
    // What was measured on a machine with six workers: as many fetches to a
    // host that drops packets as there were workers, then one more task
    // standing in for a settings save, and the save ran fourteen point eight
    // seconds later. Every request a plugin made was a task on the window's
    // own pool, and a fetch holds its worker for as long as it waits. Here
    // the window has one worker, so a single fetch on it is enough to show
    // it — and a save that waits behind none of them is the fix.
    let queue = LocalQueue::new();
    let window = Arc::new(Background::new(1));
    let mut app = App::new(queue.foreground(), window.clone());
    let plugins = Arc::new(Background::new(REQUEST_WORKERS));
    let runtime = runtime_in(&mut app, &["net:127.0.0.1"], &plugins);
    let mut silent = Silent::new();

    ask_for(&mut app, &runtime, 1, vec![silent.fetch(); 8]);
    assert!(
        silent.reached(Duration::from_secs(5)),
        "no fetch was started at all, so nothing here is hanging"
    );

    let (ran, was_run) = std::sync::mpsc::channel();
    window
        .spawn(async move {
            let _ = ran.send(());
        })
        .detach();

    assert!(
        was_run.recv_timeout(PROMPTLY).is_ok(),
        "a task on the window's pool waited behind a plugin's fetches"
    );
    still_hanging(&queue, &app, &runtime, 8);
}

#[test]
fn one_plugins_hanging_fetches_leave_room_for_another_plugins_requests() {
    // The plugins' pool is shared between them, so it is the same story one
    // level down: a plugin that asks for more fetches than the pool has
    // workers, to a host that never answers, used to hold every worker, and
    // the next plugin's request waited fifteen seconds behind it. At most
    // two of a plugin's requests are on the pool; the rest wait their turn
    // in its own queue, not in front of everybody else's.
    let queue = LocalQueue::new();
    let mut app = App::new(queue.foreground(), Arc::new(Background::new(1)));
    let plugins = Arc::new(Background::new(REQUEST_WORKERS));
    let greedy = runtime_in(&mut app, &["net:127.0.0.1"], &plugins);
    let other = runtime_in(&mut app, &["net:127.0.0.1"], &plugins);
    let mut hanging = Silent::new();
    let mut asked = Silent::new();

    ask_for(&mut app, &greedy, 1, vec![hanging.fetch(); 8]);
    assert!(
        hanging.reached(Duration::from_secs(5)),
        "none of the greedy plugin's fetches was started"
    );
    ask_for(&mut app, &other, 1, vec![asked.fetch()]);

    assert!(
        asked.reached(PROMPTLY),
        "another plugin's fetch waited behind the first one's"
    );
    still_hanging(&queue, &app, &greedy, 8);
    assert_eq!(
        queued_and_running(&app, &greedy),
        ((3..9).collect(), RUNNING_AT_ONCE),
        "the greedy plugin had more than its share of the pool"
    );
}

#[test]
fn a_plugins_queued_requests_are_each_started_in_the_order_asked_as_one_finishes() {
    // What the budget must not do, which is the rule the deeds follow too: a
    // request waiting is a ticket a guest is waiting on, so none is dropped,
    // and they go in the order they were asked. Five fetches, each to a host
    // of its own, so that which one a connection arrives at says which
    // request was started.
    let queue = LocalQueue::new();
    let mut app = App::new(queue.foreground(), Arc::new(Background::new(1)));
    let plugins = Arc::new(Background::new(REQUEST_WORKERS));
    let runtime = runtime_in(&mut app, &["net:127.0.0.1"], &plugins);
    let mut hosts: Vec<Silent> = (0..5).map(|_| Silent::new()).collect();

    ask_for(
        &mut app,
        &runtime,
        1,
        hosts.iter().map(Silent::fetch).collect(),
    );
    assert!(
        hosts[0].reached(Duration::from_secs(5)),
        "the first was not started"
    );
    assert!(
        hosts[1].reached(Duration::from_secs(5)),
        "the second was not started"
    );
    still_hanging(&queue, &app, &runtime, 5);
    assert_eq!(
        queued_and_running(&app, &runtime),
        (vec![3, 4, 5], RUNNING_AT_ONCE),
        "more than a plugin's share was started, or the rest were not kept"
    );

    // Each that is answered makes room for exactly the oldest still waiting.
    for (finished, next) in [(0, 2), (1, 3), (2, 4)] {
        hosts[finished].hang_up();
        assert!(
            until(&queue, || hosts[next].reached(Duration::ZERO)),
            "fetch {} was not started once fetch {} was answered",
            next + 1,
            finished + 1
        );
        let (waiting, _) = queued_and_running(&app, &runtime);
        assert_eq!(
            waiting,
            (next as u32 + 2..6).collect::<Vec<_>>(),
            "something other than the oldest was started"
        );
    }

    for host in &mut hosts {
        host.hang_up();
    }
    assert!(
        until(&queue, || queued_and_running(&app, &runtime)
            == (Vec::new(), 0)),
        "not every request was answered"
    );
}

#[test]
fn a_plugin_built_for_a_window_works_on_the_plugins_pool_and_not_the_windows() {
    // Half of keeping a plugin's hanging fetches off the window's pool is
    // which pool a built plugin is handed, and the window's would compile:
    // it is the pool a model is built beside. So the runtime is built the
    // way a plugin's is, and asked which it has.
    let queue = LocalQueue::new();
    let window = Arc::new(Background::new(1));
    let mut app = App::new(queue.foreground(), window.clone());
    let module = crate::plugins::wasm::tests::wasm_asking_where("eugen/probe", 0, 0, 0);
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

    runtime.read(&app, |runtime, _| {
        assert!(
            !Arc::ptr_eq(&runtime.pool, &window),
            "a plugin's requests are worked on beside the window's saves"
        );
        assert!(
            Arc::ptr_eq(&runtime.pool, &requests()),
            "a plugin's requests are worked on somewhere other than the plugins' pool"
        );
    });
}

/// A sound that is not a wav, so that one let through fails before a
/// player starts and a test that plays it makes no noise.
fn quiet_sound() -> Request {
    Request::PlaySound {
        wav: b"not a wav".to_vec(),
        volume: 0,
    }
}

/// When each sound a runtime still counts against its limit was played.
fn rung(app: &App, runtime: &ModelHandle<Runtime>) -> Vec<Instant> {
    runtime.read(app, |runtime, _| {
        runtime
            .chimes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .played()
    })
}

#[test]
fn a_sound_past_what_a_plugin_may_play_in_a_second_is_answered_and_not_played() {
    // A sound is a player started and a person interrupted, and nothing
    // limited how many a plugin could ask for. Past the limit it is not
    // dropped — the guest would wait on the ticket for the rest of the
    // session — but answered with why, and never handed to a player.
    let chimes = Mutex::new(Chimes::default());

    for index in 0..SOUNDS_PER_SECOND + 2 {
        let Answer::Failed(why) = ring(quiet_sound(), &chimes) else {
            panic!("sound {index} was played, and it was not a wav");
        };
        assert_eq!(
            why.contains("sounds a second"),
            index >= SOUNDS_PER_SECOND,
            "sound {index} was answered: {why}"
        );
    }
}

#[test]
fn a_sound_is_counted_when_it_is_played_and_not_when_it_was_asked_for() {
    // Counted when asked for, a sound waiting for a worker was a sound that
    // had already been let through: a plugin asking four a second while the
    // pool was busy for fifteen seconds had sixty played back to back when
    // it came free. Here the pool's only worker is held while more than a
    // second's worth are asked for, and let go.
    let queue = LocalQueue::new();
    let mut app = App::new(queue.foreground(), Arc::new(Background::new(1)));
    let plugins = Arc::new(Background::new(1));
    let runtime = runtime_in(&mut app, &["sound.play"], &plugins);
    let (release, held) = std::sync::mpsc::channel::<()>();
    plugins
        .spawn(async move {
            let _ = held.recv();
        })
        .detach();

    ask_for(
        &mut app,
        &runtime,
        1,
        vec![quiet_sound(); SOUNDS_PER_SECOND + 2],
    );
    queue.run_until_parked();
    assert_eq!(
        rung(&app, &runtime),
        Vec::new(),
        "sounds were counted as played while nothing could play them"
    );

    let released = Instant::now();
    release.send(()).expect("the worker is holding on");
    // One worker, first come first served: when this has run, every sound
    // asked for before it has been through the limit.
    let (behind, through) = std::sync::mpsc::channel();
    plugins
        .spawn(async move {
            let _ = behind.send(());
        })
        .detach();
    through
        .recv_timeout(Duration::from_secs(5))
        .expect("the pool came free");

    let played = rung(&app, &runtime);
    assert_eq!(
        played.len(),
        SOUNDS_PER_SECOND,
        "more than a second's worth was played the moment the pool came free"
    );
    assert!(
        played.iter().all(|at| *at >= released),
        "a sound was counted before the pool could play it"
    );
}

#[test]
fn a_plugins_sound_is_played_while_its_fetches_hang() {
    // A sound waiting its turn behind its plugin's own fetches to a host that
    // drops packets rang fifteen seconds after what it was about. It is a
    // write and a spawn, so it does not take a turn: it is played beside
    // them.
    let queue = LocalQueue::new();
    let mut app = App::new(queue.foreground(), Arc::new(Background::new(1)));
    let plugins = Arc::new(Background::new(REQUEST_WORKERS));
    let runtime = runtime_in(&mut app, &["net:127.0.0.1", "sound.play"], &plugins);
    let mut silent = Silent::new();

    ask_for(&mut app, &runtime, 1, vec![silent.fetch(); 4]);
    assert!(
        silent.reached(Duration::from_secs(5)),
        "none of the plugin's fetches was started"
    );
    ask_for(&mut app, &runtime, 5, vec![quiet_sound()]);

    let asked = Instant::now();
    assert!(
        until(&queue, || rung(&app, &runtime).len() == 1),
        "the sound waited behind the fetches"
    );
    assert!(
        asked.elapsed() < PROMPTLY,
        "the sound took {:?} to be played",
        asked.elapsed()
    );
    still_hanging(&queue, &app, &runtime, 4);
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
fn a_line_that_is_not_utf8_costs_itself_and_not_the_rest_of_the_file() {
    let root = transcripts("bytes");
    let mut file = b"\xff\xfe not text, and \"usage\" in it\n".to_vec();
    file.extend_from_slice(
        concat!(
            r#"{"type":"assistant","timestamp":"2026-09-06T10:00:00Z","message":{"id":"m4","model":"claude-opus-5","usage":{"output_tokens":7}}}"#,
            "\n"
        )
        .as_bytes(),
    );
    fs::write(root.join("two/d.jsonl"), file).expect("a scratch file");

    let Answer::Counted { lines, .. } = perform(tally_of(&root)) else {
        panic!("a tally answers with tables");
    };

    assert_eq!(lines, 4, "the turn after the broken line was not counted");
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
fn a_walk_past_its_deadline_says_it_gave_up_rather_than_answering_a_low_total() {
    // A fetch has a socket's timeout to end it, and a walk had nothing: a
    // granted directory holding a file of a hundred gigabytes was a worker
    // held for as long as reading it took. What is past the deadline is not
    // a total — a count that stopped part way is a week with fewer turns in
    // it than it had — so the answer is that it gave up.
    let root = transcripts("deadline");
    let Request::Tally {
        containing,
        at_least,
        distinct_by,
        tables,
        ..
    } = tally_of(&root)
    else {
        panic!("that is a tally");
    };
    let later = Instant::now() + Duration::from_secs(60);

    assert_eq!(
        walk(&root, ".jsonl", 0, Instant::now()),
        None,
        "a walk out of time went on looking"
    );
    let files = walk(&root, ".jsonl", 0, later).expect("a minute is time enough for three files");

    let Answer::Failed(why) = add_up(
        &files,
        &containing,
        &at_least,
        &distinct_by,
        &tables,
        Instant::now(),
    ) else {
        panic!("counting out of time answered with a total");
    };
    assert!(why.contains("given up"), "unhelpful answer: {why}");
    // The last file alone, which is where a total that had stopped part way
    // used to be handed over as though it were the whole: nothing came after
    // it to notice the clock.
    let last = &files[files.len() - 1..];
    assert!(
        matches!(
            add_up(
                last,
                &containing,
                &at_least,
                &distinct_by,
                &tables,
                Instant::now()
            ),
            Answer::Failed(_)
        ),
        "the last file, read out of time, was answered with a total"
    );
    assert!(
        matches!(
            add_up(&files, &containing, &at_least, &distinct_by, &tables, later),
            Answer::Counted { lines: 3, .. }
        ),
        "the same files did not add up in time"
    );
}

/// What [`next_line`] makes of `text`, a line at a time, keeping at most
/// `cap` of each: what it found, and the line when it was whole.
fn lines_of(text: &[u8], cap: usize) -> Vec<(Line, Option<String>)> {
    // A buffer smaller than a line, so that reading past one takes more
    // than one look.
    let mut reader = std::io::BufReader::with_capacity(4, text);
    let later = Instant::now() + Duration::from_secs(60);
    let mut bytes = Vec::new();
    let mut found = Vec::new();
    loop {
        let line = next_line(&mut reader, &mut bytes, cap, later).expect("a slice can be read");
        assert!(
            bytes.len() <= cap,
            "{} bytes of one line were held under a ceiling of {cap}",
            bytes.len()
        );
        let whole = (line == Line::Whole).then(|| String::from_utf8_lossy(&bytes).into_owned());
        let end = line == Line::End;
        found.push((line, whole));
        if end {
            return found;
        }
    }
}

#[test]
fn a_line_longer_than_a_walk_holds_is_read_past_and_the_next_one_is_read() {
    // A line was read whole however long it was. One past the ceiling is
    // skipped as a line that is not JSON is, and the line after it is read
    // as though nothing had happened.
    assert_eq!(
        lines_of(b"short\nthis one is far too long\nnext\nlast", 8),
        vec![
            (Line::Whole, Some(String::from("short\n"))),
            (Line::TooLong, None),
            (Line::Whole, Some(String::from("next\n"))),
            (Line::Whole, Some(String::from("last"))),
            (Line::End, None),
        ]
    );
    // Exactly as long as the ceiling, with or without its newline, fits.
    assert_eq!(
        lines_of(b"1234567\n12345678", 8),
        vec![
            (Line::Whole, Some(String::from("1234567\n"))),
            (Line::Whole, Some(String::from("12345678"))),
            (Line::End, None),
        ]
    );
}

#[test]
fn a_line_with_no_end_is_given_up_on_at_the_deadline_and_not_held() {
    // A disk image in a granted directory is a line with no end: read with
    // nothing but `read_until`, it went into memory whole whatever the
    // deadline said. An endless one here, so that only the clock can stop it.
    let mut endless = std::io::BufReader::new(std::io::repeat(b'x'));
    let mut bytes = Vec::new();

    let outcome = next_line(
        &mut endless,
        &mut bytes,
        1024,
        Instant::now() + Duration::from_millis(50),
    );

    assert!(
        matches!(outcome, Err(Stopped::OutOfTime)),
        "a line with no end came to {outcome:?}"
    );
    assert!(bytes.len() <= 1024, "{} bytes of it were held", bytes.len());
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

/// A keychain that answers with a dialog nobody is there to dismiss does not
/// hold the request for good: the program asked is killed at the deadline.
#[cfg(target_os = "macos")]
#[test]
fn a_keychain_question_that_never_comes_back_is_given_up_on() {
    let started = std::time::Instant::now();
    let answer = output_within("/bin/sleep", &["30"], Duration::from_millis(200));
    assert_eq!(
        answer.map_err(|why| why.kind()),
        Err(std::io::ErrorKind::TimedOut)
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it waited the sleep out"
    );

    assert_eq!(
        output_within("/bin/echo", &["secret"], Duration::from_secs(10)).expect("echo ran"),
        (true, b"secret\n".to_vec())
    );
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
