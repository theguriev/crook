//! The half of a sandboxed plugin that is not in the sandbox.
//!
//! A plugin that can only describe what it already knows is a plugin that
//! knows nothing: the usage chip has to read a file and reach a host, and the
//! whole point of the second tier is that it may do neither. So it *asks*, and
//! this is what happens to the asking.
//!
//! # Why a model, and not a few closures
//!
//! Because of one type. A contribution is a
//! [`Fn(&Workspace, &AppContext)`](crate::plugin::UiContribution): it may read
//! the world and build elements, and it may not start work or ask for a
//! repaint. Something has to hold a context that can, and in this application
//! the thing that holds one is an entity. So each sandboxed plugin gets a
//! model of its own — the same shape the usage chip's poller had when it was
//! in the box, which is not a coincidence: this is that shape, with the plugin
//! moved to the other side of a boundary.
//!
//! **A request raised while describing waits.** The guest may call the request
//! import from anywhere, but only `build`, an action, a tick and a delivery
//! run somewhere that can start work; anything asked for during a render is
//! taken on the next of those. A plugin that wants to poll asks from its tick,
//! which is what a tick is for.
//!
//! # Three of the things it may ask for are not this model's to answer
//!
//! A fetch and a file are work: they belong on the pool and come back here.
//! Where the active pane is, what Crook can be asked to do, typing a line into
//! a shell and running one of Crook's own commands are none of them work —
//! they are questions about, and changes to, the window this plugin is drawn
//! in, and this model cannot see it. So those wait in [`Runtime::deeds`] and
//! are served by the observer in `wasm::mod`, which is handed the workspace
//! the moment anything here notifies.
//!
//! **They are served a turn at a time.** Served on the thread that draws, and
//! answered there, so a guest that asks again from every answer would be
//! served for as long as it kept asking — and the window with it. Past
//! [`DEEDS_PER_TURN`] what is left waits here, untouched, for a turn booked
//! after the window has drawn. A plugin whose turns run out
//! [`OVERRUNS_ALLOWED`] times in a row, or that has more than [`MAX_WAITING`]
//! things to type, run or copy waiting at once, stops being served at all.
//!
//! **And two of them may only happen because somebody pressed something.**
//! A plugin that could type into a shell from a timer is a plugin that types
//! while nobody is looking, so a request that *changes* something is taken
//! only out of a [`crook_run`](crook_wasm::exports::RUN) a person caused. It
//! is not refused — the grant is not the thing being failed — it comes back
//! as [`Answer::Failed`] saying so.
//!
//! # Nothing a plugin asks for happens because it asked
//!
//! Every request is checked against what a person granted before it is
//! started, and the check is here rather than in [`crook_wasm`] — the sandbox
//! deliberately cannot tell an allowed request from a refused one. A request
//! outside the grant is not an error and does not count against the plugin: it
//! comes back as [`Answer::Refused`] carrying the sentence the permission
//! dialog uses, so the plugin can say "allow me to reach api.anthropic.com"
//! rather than "something went wrong".

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use crookui_core::executor::Task;
use crookui_core::prelude::*;

use crook_plugin::PluginId;
use crook_plugin_api::{
    Answer, Bound, Capability, Cell as Field, Entry, Event, Method, Request, Table, Tallied,
};
use crook_wasm::Sandbox;

use super::GIVE_UP_AFTER;

/// How long one request may take before it is given up on.
///
/// The same fifteen seconds the usage poller used when it was in the box. A
/// plugin waiting on a socket costs nobody a frame — the work is on the pool —
/// but a request with no ceiling is a background thread held for the rest of
/// the session.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait before ticking a plugin that has just failed.
///
/// A failure must not end the clock. Everything a plugin does happens because
/// a tick happened, and a tick only happens because the last one booked the
/// next — so a `return` on the failing path is a plugin that stops for good
/// after one bad minute, with the number it last drew still on screen looking
/// current. That is the shape of "it worked and then it quietly stopped", and
/// it took one transient error to cause. A second is long enough that three
/// failures in a row are three seconds rather than three frames, and short
/// enough that a person does not see the gap.
const RECOVER_AFTER: Duration = Duration::from_secs(1);

/// How many refusals in a row a plugin gets before it stops being asked at
/// all for the rest of the session.
///
/// A refusal is not a failure — it is the ordinary state of a plugin nobody
/// has answered for yet, and it must not count against
/// [`GIVE_UP_AFTER`]. But a plugin that answers every refusal by asking again
/// is a loop, and a loop that spends a background task per turn is a machine
/// with a fan on. Sixteen is far more than a plugin has reason to ask before
/// somebody allows it, and small enough that the loop stops being free.
const REFUSALS_ALLOWED: u32 = 16;

/// How long to wait before trying again to hand a guest something while it is
/// busy being something else.
///
/// About a frame. This should not happen — every call into a guest is made on
/// the thread that draws, and none of them is re-entrant — but "should not"
/// is not a guarantee, and the cost of being wrong is a plugin that waits for
/// an answer that was quietly thrown away and never polls again. Retrying is
/// cheap; a chip frozen on a number from an hour ago is not.
const RETRY_AFTER: Duration = Duration::from_millis(16);

/// How many deeds one plugin is handed in one of the window's turns.
///
/// A deed is served on the thread that draws, and its answer is delivered
/// there too, so a guest that asks again from inside every answer is a loop
/// with nothing in it that waits — and serving until nothing was left waiting
/// was a window that never came back and had to be killed, over a plugin with
/// one ordinary grant. Nothing else in that loop is finite: each deed is
/// answered before the next is asked, so the sandbox's ceiling on what a guest
/// may have outstanding never fills.
///
/// A turn is [`AppContext::turns`]: one update and everything its effects
/// set off. Not one call of the observer, because each answer notifies this
/// model and the effects queue would call it again; and not "until the queue
/// is found empty", because a plugin that runs another plugin's action, and is
/// run by it, finds its own queue empty after every answer while the chain
/// goes on through the other one.
///
/// Thirty-two because that is as many requests as the sandbox holds for a
/// guest at once, so everything any one call raised is served in the turn
/// that raised it, and only a chain — answers that keep asking — ever spans
/// turns. The plugins that exist ask for two at most: where the pane is and
/// what Crook can do, from `build`, or what a command printed and then the
/// clipboard, one after the other.
pub(super) const DEEDS_PER_TURN: u32 = 32;

/// How many turns in a row a plugin's deeds may run out of room: the one that
/// makes it this many is the last it is served until Crook is restarted.
///
/// The rule [`REFUSALS_ALLOWED`] is, for the same kind of loop. A burst bigger
/// than a turn is carried on with in the next one, and a chain that ends — a
/// turn that ends with nothing left waiting — is forgiven its count, but a
/// plugin still asking after sixteen full turns is asking because it asks, and
/// each turn it is given is thirty-two calls into it on the thread that draws.
const OVERRUNS_ALLOWED: u32 = 16;

/// How many deeds that carry text — a line to type, a command to run and what
/// to hand it, something to copy — one plugin may have waiting at once; one
/// more and it stops being served until Crook is restarted.
///
/// A turn bounds what is *served*, and nothing about that bounds what is
/// *kept*: each answer in a turn may ask for as many things as the sandbox
/// holds for a guest at once, so a plugin that asks for thirty-two from every
/// answer leaves a thousand behind after one turn and sixteen thousand by the
/// time its turns run out — and each of these may be a megabyte. Twice a
/// turn's worth is what a full turn leaves when every answer in it asks for
/// two things more, which is the most any plugin that exists asks for from
/// one call; a queue of these longer than that is growing, not being served.
/// Only a press, or what came of one, may ask for them, and a press is one
/// call, so it raises a turn's worth at most.
///
/// **The rest are not counted: they carry nothing to keep, and a burst of
/// them is the window's doing.** Where the pane is and what Crook can do are
/// all an event may ask the window for, and events come in batches: every
/// command a pane reports finished in one read is an event of its own in one
/// update, and all of them are delivered before the first thing they asked for
/// is served. A plugin asking once from each has as many waiting as there
/// were commands without a single answer having asked for anything, and a
/// ceiling on those would stop it and drop every ticket it held unanswered.
/// What they cost waiting is an entry each, and a plugin that keeps asking for
/// them from its answers is the one [`OVERRUNS_ALLOWED`] stops — as is one
/// handed more than sixteen turns' worth in one update, which that rule cannot
/// tell from a loop.
pub(super) const MAX_WAITING: usize = 2 * DEEDS_PER_TURN as usize;

/// How long the window is left to itself before a plugin's waiting deeds are
/// carried on with.
///
/// About a frame, and a sleep on the pool rather than a task queued straight
/// back onto the foreground: on X11 and macOS the event loop runs every task
/// it has been handed before it draws anything, so a task that queued the
/// next one at once would run in the same pass, and the turn this is for
/// would never come. Booked only while deeds are waiting, so it is not a
/// clock — a plugin with nothing left to be served books nothing.
const RESUME_AFTER: Duration = Duration::from_millis(16);

/// How many files one walk will look at before it stops.
///
/// A bound on the walk rather than a guess at what is reasonable: a granted
/// directory is somebody's, and a plugin asking to walk one should not be able
/// to turn a home directory with a million files in it into a walk that never
/// ends.
const FILES_PER_WALK: usize = 20_000;

/// The most a plugin may be handed back from one request.
///
/// A megabyte, which is the sandbox's own ceiling on an answer: a bigger one
/// could not be delivered anyway, and finding that out after reading two
/// hundred megabytes into memory would be finding it out too late.
const MAX_ANSWER: u64 = 1 << 20;

/// How many names one [`Request::List`] answers with.
///
/// A directory is one directory and never a tree, so this is a bound on the
/// unusual rather than on the ordinary: `/nix/store` and a `node_modules` that
/// got away from somebody are both real, and neither should be a megabyte
/// copied into a plugin's memory for a list a person scrolls ten rows of.
const MAX_NAMES: usize = 2048;

/// Why the guest is being pumped.
///
/// The difference between a plugin that answers a click and a plugin that acts
/// on its own, which is the whole of what makes typing into somebody's shell
/// an acceptable thing for a stranger's plugin to be able to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Gesture {
    /// A person pressed something and this is what came of it.
    Pressed,
    /// Anything else: a build, a timer, an answer landing.
    None,
}

/// One sandboxed plugin's runtime: what it asked for, and what came back.
pub(super) struct Runtime {
    /// Whose requests these are, for the log line when one is refused.
    id: PluginId,
    /// Shared with the contributions, which call into the guest to draw. Only
    /// ever borrowed for the length of one call.
    sandbox: Rc<RefCell<Sandbox>>,
    /// Shared with them too: a plugin that fails everywhere is switched off
    /// everywhere. See [`GIVE_UP_AFTER`].
    failures: Rc<Cell<u32>>,
    /// What a person allowed, as [`Capability::keys`] writes it down.
    granted: Vec<String>,
    /// How many things in a row it has asked for and not been allowed. See
    /// [`REFUSALS_ALLOWED`].
    refusals: u32,
    /// Which tickets were raised out of a press, so that the answer to one is
    /// still that press's business.
    ///
    /// A chain is one gesture. "Ask what the command printed, and when it
    /// comes back put it on the clipboard" is two requests and one thing a
    /// person did — and a delivery that forgot which would refuse the second
    /// half of every plugin that reads before it acts. What it must *not*
    /// become is a gesture that outlives the chain: an entry is remembered
    /// only until its answer lands, and a tick or an event pumps as itself.
    pressed: Vec<u32>,
    /// The tick that is coming.
    waiting: Option<Task<()>>,
    /// What pokes the wait awake, while there is one to poke.
    ///
    /// **This is the whole of why a plugin parks one worker and not several.**
    /// A guest holds one timer and cannot take an answer back, so a chip that
    /// wants waking every hundred milliseconds while somebody watches the bite
    /// has to be able to shorten the minute it asked for before the click. The
    /// obvious way — start a new wait and drop the old task — is wrong in a way
    /// that does not show up for an hour: dropping the task cancels the
    /// *callback*, and a worker already inside `thread::sleep` cannot be
    /// interrupted, so it holds its place in the pool for the rest of the
    /// minute. Six clicks in a minute is six workers held, and the pool is
    /// sized for five parked chains and a spare. Everything that waits on a
    /// worker — the git gather, a settings save, this plugin's own next tick —
    /// queues behind them, and then it all comes back by itself, which is what
    /// makes it so hard to catch.
    ///
    /// So a shorter wait *pokes* the chain that is already parked. It returns
    /// early, the guest is ticked, and it books whatever it wants next. The
    /// deleted usage poller carried this same design and said why in the same
    /// words; this is that lesson, relearned the expensive way.
    wake: Option<Sender<()>>,
    /// What it asked for that only the workspace can answer, waiting for
    /// somewhere that holds one. See this module's own doc.
    ///
    /// Every one of these has already been checked against the grant: what is
    /// waiting is the *doing*, and a request nobody allowed never gets here.
    deeds: Vec<(u32, Request)>,
    /// How many of `deeds` carry text, which is what [`MAX_WAITING`] counts.
    carrying: usize,
    /// Which of the window's turns `served` is counting. See
    /// [`DEEDS_PER_TURN`].
    turn: u64,
    /// How many deeds have been handed out in that turn.
    served: u32,
    /// Whether that turn has run out of room with deeds still waiting.
    overran: bool,
    /// How many turns in a row have. See [`OVERRUNS_ALLOWED`].
    overruns: u32,
    /// Whether the turn that carries on with what is still waiting has been
    /// booked — once, however often the observer asks in the meantime.
    resuming: bool,
    /// Whether this plugin was caught asking the window for things without
    /// end, so that what it asks the window for is neither kept nor served
    /// for the rest of the session. See [`OVERRUNS_ALLOWED`] and
    /// [`MAX_WAITING`].
    stopped: bool,
}

impl Entity for Runtime {
    type Event = ();
}

impl Runtime {
    /// A runtime for a plugin that has just been built.
    pub(super) fn new(
        id: PluginId,
        sandbox: Rc<RefCell<Sandbox>>,
        failures: Rc<Cell<u32>>,
        granted: Vec<String>,
    ) -> Self {
        Self {
            id,
            sandbox,
            failures,
            granted,
            refusals: 0,
            pressed: Vec::new(),
            waiting: None,
            wake: None,
            deeds: Vec::new(),
            carrying: 0,
            turn: 0,
            served: 0,
            overran: false,
            overruns: 0,
            resuming: false,
            stopped: false,
        }
    }

    /// What is waiting for a workspace, taken, as far as this turn goes.
    ///
    /// Drained rather than read, for the reason the sandbox drains what a
    /// guest asked for: a deed served twice is a line typed into a shell
    /// twice, and the second one would be the host's fault.
    ///
    /// **And taken no further than [`DEEDS_PER_TURN`].** What is past it stays
    /// here, in the order it was asked, for the turn that is booked to carry
    /// on with it: a ticket taken and not served is a guest waiting for the
    /// rest of the session, so the budget decides what is *taken* and never
    /// what is dropped. Once the turn is spent the answer is nothing at all,
    /// which is what ends the observer's loop.
    pub(super) fn deeds(&mut self, ctx: &mut ModelContext<Self>) -> Vec<(u32, Request)> {
        if self.stopped {
            return Vec::new();
        }
        let turn = ctx.turns();
        if turn != self.turn {
            // The last turn this plugin was looked at in is over. One that
            // ended with nothing waiting ended a chain, and however long the
            // chain ran it was not the loop: the loop always has something
            // waiting when its turn runs out.
            if !self.overran {
                self.overruns = 0;
            }
            self.turn = turn;
            self.served = 0;
            self.overran = false;
        }
        if self.deeds.is_empty() {
            return Vec::new();
        }

        let room = DEEDS_PER_TURN.saturating_sub(self.served) as usize;
        if room == 0 {
            self.ran_out(ctx);
            return Vec::new();
        }
        let taken: Vec<(u32, Request)> = self.deeds.drain(..room.min(self.deeds.len())).collect();
        self.served += taken.len() as u32;
        self.carrying -= taken
            .iter()
            .filter(|(_, request)| carries_text(request))
            .count();
        taken
    }

    /// Books the turn that carries on with what is still waiting — or, for a
    /// plugin whose turns have run out [`OVERRUNS_ALLOWED`] times in a row,
    /// stops serving it.
    fn ran_out(&mut self, ctx: &mut ModelContext<Self>) {
        // Counted once a turn, however often the observer asks in it.
        if !self.overran {
            self.overran = true;
            self.overruns += 1;
            if self.overruns >= OVERRUNS_ALLOWED {
                self.stop(&format!(
                    "has asked the window for more than {DEEDS_PER_TURN} things a turn, \
                     {OVERRUNS_ALLOWED} turns in a row,"
                ));
                return;
            }
        }
        if self.resuming {
            return;
        }

        self.resuming = true;
        self.later(
            RESUME_AFTER,
            |runtime, ctx| {
                runtime.resuming = false;
                // The observer is what serves them, and it runs when this
                // model notifies; nothing else would. It runs in a turn of its
                // own, which is what gives it a turn's worth again.
                ctx.notify();
            },
            ctx,
        );
    }

    /// Stops serving this plugin's deeds for the rest of the session, and says
    /// so, once.
    ///
    /// What is waiting is dropped, as a request past [`REFUSALS_ALLOWED`] is:
    /// answering it would be delivering to the loop, which asks again. Its
    /// tickets go from `pressed` too — in one pass, because a queue this is
    /// called on can hold thousands — since none of them is ever going to be
    /// answered.
    fn stop(&mut self, why: &str) {
        log::warn!(
            "{} {why} and will not be served again until Crook is restarted",
            self.id
        );
        self.stopped = true;
        self.carrying = 0;
        let dropped: HashSet<u32> = self.deeds.drain(..).map(|(ticket, _)| ticket).collect();
        self.pressed.retain(|ticket| !dropped.contains(ticket));
    }

    /// Takes everything the guest asked for and starts it.
    ///
    /// Called after every call that can reach a context, which is what makes
    /// "ask, and be answered later" a loop rather than a single shot: a
    /// delivery pumps again, so a plugin that reads a file and then fetches
    /// what the file authorised it to fetch needs no special case.
    /// Tells the guest what this machine's offset from UTC is.
    ///
    /// Before every pumped call rather than once, because an offset changes
    /// when the clocks go back and when a laptop is opened in another country
    /// — and a plugin drawing a chart of days against the wrong one is wrong
    /// in a way nobody would think to check.
    pub(super) fn set_timezone(&self) {
        let minutes = chrono::Local::now().offset().local_minus_utc() / 60;
        if let Ok(mut sandbox) = self.sandbox.try_borrow_mut() {
            sandbox.set_timezone(minutes);
        }
    }

    pub(super) fn pump(&mut self, gesture: Gesture, ctx: &mut ModelContext<Self>) {
        self.set_timezone();

        let (asked, timer) = {
            // Already running: the guest reached back into itself, which it
            // cannot do through this API. Nothing to take.
            let Ok(mut sandbox) = self.sandbox.try_borrow_mut() else {
                return;
            };
            (sandbox.asked(), sandbox.timer())
        };

        for (ticket, request) in asked {
            self.start(ticket, request, gesture, ctx);
        }
        if let Some(after) = timer {
            self.wait(after, ctx);
        }
    }

    /// Checks one request against the grant and starts it if it is inside.
    fn start(
        &mut self,
        ticket: u32,
        request: Request,
        gesture: Gesture,
        ctx: &mut ModelContext<Self>,
    ) {
        // A plugin that asks for things without exporting anywhere to put the
        // answer is a plugin nothing can be done for. Said once, here, rather
        // than after the work is done.
        if !self.sandbox.borrow().takes_answers() {
            log::warn!(
                "{} asked for something and exports nowhere to deliver it",
                self.id
            );
            return;
        }

        let refused = allowed(&self.granted, &request).err();
        if let Some(sentence) = refused.as_ref() {
            self.refusals += 1;
            if self.refusals > REFUSALS_ALLOWED {
                if self.refusals == REFUSALS_ALLOWED + 1 {
                    log::warn!(
                        "{} has asked for {REFUSALS_ALLOWED} things it was not allowed and \
                         will not be asked again until Crook is restarted",
                        self.id
                    );
                }
                return;
            }
            log::info!("{} was not allowed to: {sentence}", self.id);
        } else {
            self.refusals = 0;
        }

        // Asked for on nobody's behalf. Not a refusal — the grant is not the
        // thing being failed — and not counted against one either: a plugin
        // that got this wrong has a bug rather than a permission it is
        // missing.
        let declined = (refused.is_none()
            && only_from_a_gesture(&request)
            && gesture == Gesture::None)
            .then(|| {
                String::from(
                    "that only happens when somebody presses something, and nobody pressed anything",
                )
            });

        let deed = refused.is_none() && declined.is_none() && needs_the_workspace(&request);
        // Dropped, the way a refusal past its allowance is: this plugin was
        // caught asking the window for things without end. See
        // [`OVERRUNS_ALLOWED`].
        if deed && self.stopped {
            return;
        }
        let carrying = deed && carries_text(&request);
        if carrying && self.carrying >= MAX_WAITING {
            self.stop(&format!(
                "has asked the window to type, run or copy more than {MAX_WAITING} things \
                 at once,"
            ));
            return;
        }

        // Written down before anything can answer it: what this ticket's
        // answer may go on to ask for is decided by what raised it. See the
        // field.
        if gesture == Gesture::Pressed {
            self.pressed.push(ticket);
        }

        // Allowed, invited, and about the window rather than about the world:
        // it waits for somewhere that holds one. Notified, because the thing
        // that serves it is an observer of this model and nothing else would
        // say there is anything to serve.
        if deed {
            self.deeds.push((ticket, request));
            self.carrying += usize::from(carrying);
            ctx.notify();
            return;
        }

        // A refusal goes round the pool exactly as the work would, and that is
        // not ceremony: answering it here would mean `deliver` running inside
        // `pump`, and a guest that answers a refusal by asking again would
        // take the host's stack down with it. Off the foreground and back is
        // what makes the loop a loop rather than a recursion.
        let working = ctx.background().spawn(async move {
            match (refused, declined) {
                (Some(sentence), _) => Answer::Refused(sentence),
                (None, Some(why)) => Answer::Failed(why),
                (None, None) => perform(request),
            }
        });
        ctx.spawn(working, move |runtime, answer, ctx| {
            runtime.answer(ticket, answer, ctx);
        })
        .detach();
    }

    /// Tells the guest something happened, and takes whatever that made it ask
    /// for.
    ///
    /// The pump afterwards is the whole point: a plugin hears that a command
    /// finished and answers by asking to play a sound, and without it that
    /// request would sit in the registry until something else woke the
    /// runtime up.
    pub(super) fn notify(&mut self, event: &Event, ctx: &mut ModelContext<Self>) {
        if self.failures.get() >= GIVE_UP_AFTER {
            return;
        }
        // Said nothing about: a plugin that watches without exporting
        // `crook_event` is one the host has nowhere to tell, and it registered
        // the watch itself, so this is its own bug and not worth a line per
        // command somebody runs.
        if !self.sandbox.borrow().takes_events() {
            return;
        }

        let sandbox = Rc::clone(&self.sandbox);
        let outcome = match sandbox.try_borrow_mut() {
            Ok(mut sandbox) => sandbox.event(event),
            Err(_) => {
                // Dropped rather than retried, which is the opposite of what
                // an answer does — and the difference is that nothing is
                // waiting on this. A ticket left unanswered strands a plugin
                // forever; a missed event is one chime, and a queue of them
                // would ring after the thing they were about.
                log::warn!("{} was told of an event while it was running", self.id);
                return;
            }
        };

        match outcome {
            Ok(()) => self.failures.set(0),
            Err(problem) => {
                self.give_up_on(&problem.to_string());
                return;
            }
        }

        // An event is something that happened, not somebody pressing
        // something: whatever it makes the plugin ask for, it may not type.
        self.pump(Gesture::None, ctx);
    }

    /// Hands one answer to the guest, and takes whatever that made it ask for.
    ///
    /// Reachable from `wasm::mod` as well as from here: a deed the workspace
    /// served comes back through the same door as a fetch off the pool, so a
    /// guest cannot tell which of its requests took a detour.
    pub(super) fn answer(&mut self, ticket: u32, answer: Answer, ctx: &mut ModelContext<Self>) {
        if self.failures.get() >= GIVE_UP_AFTER {
            return;
        }

        // Through a handle of its own, not through `self`: an arm that reaches
        // back into the runtime while the match still holds the borrow is a
        // borrow of `self` inside a borrow of `self`.
        let sandbox = Rc::clone(&self.sandbox);
        let outcome = match sandbox.try_borrow_mut() {
            Ok(mut sandbox) => sandbox.deliver(ticket, &answer),
            Err(_) => {
                // Handed back rather than dropped. A guest is waiting on this
                // ticket and will wait for the rest of the session if nobody
                // ever answers it. See [`RETRY_AFTER`].
                log::warn!("{} was answered while it was running", self.id);
                self.later(
                    RETRY_AFTER,
                    move |runtime, ctx| runtime.answer(ticket, answer, ctx),
                    ctx,
                );
                return;
            }
        };

        match outcome {
            Ok(()) => self.failures.set(0),
            Err(problem) => {
                // Counted, and then the clock is wound again anyway: the guest
                // is waiting on this ticket and will go on waiting, so the
                // only thing that can free it is a tick. See [`RECOVER_AFTER`].
                self.give_up_on(&problem.to_string());
                self.recover(ctx);
                return;
            }
        }

        // Whatever the answer made it ask for, under the gesture that started
        // the chain: an answer to something a press asked for is still what
        // came of that press, and a plugin that reads before it acts would
        // otherwise be refused its second half every time. Anything else — a
        // tick, an event, a build — pumps as itself.
        let gesture = match self.pressed.iter().position(|raised| *raised == ticket) {
            Some(index) => {
                self.pressed.swap_remove(index);
                Gesture::Pressed
            }
            None => Gesture::None,
        };
        self.pump(gesture, ctx);
        // The reading changed, so whatever is drawing it has to be asked
        // again. This is the bridge every model-backed feature in Crook has.
        ctx.notify();
    }

    /// Winds the clock again after something went wrong.
    ///
    /// Not the wait the guest asked for — it did not get to ask — but one
    /// short enough to be a recovery and long enough not to be a spin. A
    /// plugin that has run out of tries is left alone.
    fn recover(&mut self, ctx: &mut ModelContext<Self>) {
        if self.failures.get() >= GIVE_UP_AFTER {
            return;
        }
        self.wait(RECOVER_AFTER, ctx);
    }

    /// Sleeps for as long as the guest asked and then ticks it.
    ///
    /// A sleep on the pool rather than a timer of the foreground's own, which
    /// is what everything else here that waits does: there is no foreground
    /// timer, and a plugin is not the place to introduce one.
    fn wait(&mut self, after: Duration, ctx: &mut ModelContext<Self>) {
        // A chain is already parked, so this is the guest asking to be woken
        // sooner than it will be. Poke it: it returns early, the guest is
        // ticked, and it books what it wants next. Starting a second chain
        // here is the bug this field exists to name.
        if let Some(wake) = &self.wake {
            let _ = wake.send(());
            return;
        }

        let (wake, wakeup) = mpsc::channel();
        self.wake = Some(wake);
        // The wait is the whole of the background task, so the pool holds one
        // worker for one chain — and `recv_timeout` is what makes that worker
        // interruptible, which `thread::sleep` is not.
        let sleeping = ctx.background().spawn(async move {
            let _ = wakeup.recv_timeout(after);
        });
        self.waiting = Some(ctx.spawn(sleeping, |runtime, (), ctx| runtime.tick(ctx)));
    }

    /// Does something to this runtime a moment from now.
    ///
    /// A sleep on the pool, like every other wait here. What it is for is the
    /// one case a borrow of the guest can fail — see [`RETRY_AFTER`] — and the
    /// turn the window takes between two batches of one plugin's deeds — see
    /// [`RESUME_AFTER`].
    fn later<F>(&mut self, after: Duration, what: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut Self, &mut ModelContext<Self>) + 'static,
    {
        let sleeping = ctx.background().spawn(async move {
            std::thread::sleep(after);
        });
        ctx.spawn(sleeping, move |runtime, (), ctx| what(runtime, ctx))
            .detach();
    }

    /// Tells the guest its wait is over.
    fn tick(&mut self, ctx: &mut ModelContext<Self>) {
        // The chain has ended. Anything the guest asks for below starts a new
        // one rather than poking this one, which is no longer there.
        self.waiting = None;
        self.wake = None;
        if self.failures.get() >= GIVE_UP_AFTER {
            return;
        }

        // See `answer`: a handle of its own, so that the arm below is not a
        // borrow of `self` inside one.
        let sandbox = Rc::clone(&self.sandbox);
        let outcome = match sandbox.try_borrow_mut() {
            Ok(mut sandbox) => sandbox.tick(),
            // The guest is drawing. Tried again rather than dropped: a guest
            // asks for one timer at a time, so a tick that never arrives is a
            // plugin that stops polling for good.
            Err(_) => {
                self.later(RETRY_AFTER, Runtime::tick, ctx);
                return;
            }
        };

        match outcome {
            Ok(()) => self.failures.set(0),
            Err(problem) => {
                // The same rule `answer` follows, and the more important half
                // of it: this *is* the clock, so a `return` here is the end of
                // everything the plugin was going to do.
                self.give_up_on(&problem.to_string());
                self.recover(ctx);
                return;
            }
        }

        self.pump(Gesture::None, ctx);
        ctx.notify();
    }

    /// Counts a failure, and says so on the one that stops it being asked.
    fn give_up_on(&self, why: &str) {
        let count = self.failures.get() + 1;
        self.failures.set(count);
        log::warn!("{}: {why}", self.id);
        if count >= GIVE_UP_AFTER {
            log::warn!(
                "{} has failed {count} times and will not be asked again",
                self.id
            );
        }
    }
}

/// Whether a request is inside what a person granted.
///
/// The error is the sentence the permission dialog used, verbatim, because it
/// is the sentence the plugin has to be able to tell somebody to go and allow.
pub(super) fn allowed(granted: &[String], request: &Request) -> Result<(), String> {
    let wanted = match request {
        Request::Fetch { url, .. } => Capability::Network(vec![host_of(url)?]),
        Request::ReadFile { path } => Capability::ReadFiles(vec![path.clone()]),
        // Walking a directory is a different thing to agree to than reading a
        // file, and it is asked for differently: the grant has to name the
        // directory *and* say it means everything under it.
        Request::Tally { root, .. } => Capability::ReadFiles(vec![format!("{root}/**")]),
        Request::PlaySound { .. } => Capability::PlaySound,
        // The one grant that is not a string comparison: what was allowed is a
        // *root* and what is being asked for is somewhere under it.
        Request::List { path } => return under_a_root(granted, path),
        Request::Where | Request::Repository { .. } => Capability::ReadWorkingDirectory,
        Request::Type { template, .. } => Capability::TypeCommands(vec![template.clone()]),
        Request::Run { name, .. } => Capability::RunCommands(vec![name.clone()]),
        Request::Commands => Capability::ReadCommands,
        Request::Output => Capability::ReadBlock,
        Request::Copy { .. } => Capability::Clipboard,
    };

    if wanted
        .keys()
        .iter()
        .all(|key| granted.iter().any(|allowed| allowed == key))
    {
        Ok(())
    } else {
        Err(wanted.sentence())
    }
}

/// Whether a directory is under one of the roots a person allowed.
///
/// The comparison is on the *resolved* paths rather than on the text, because
/// `~/Work` and `/home/eugen/Work` are the same directory and a plugin that
/// asked for one on a grant written as the other is asking for what it was
/// allowed. [`resolve`] refuses a path holding `..` outright, which is what
/// makes "under a root" mean it — a path that can walk is a grant that means
/// something other than what it says.
fn under_a_root(granted: &[String], path: &str) -> Result<(), String> {
    let refusal = || Capability::ListDirectories(vec![path.to_owned()]).sentence();
    let Some(wanted) = resolve(path) else {
        return Err(refusal());
    };

    let inside = granted
        .iter()
        .filter_map(|key| key.strip_prefix("list:"))
        .filter_map(resolve)
        .any(|root| wanted.starts_with(&root));

    inside.then_some(()).ok_or_else(refusal)
}

/// Whether only the workspace can answer this.
///
/// Where the active pane is and what Crook can be asked to do are questions
/// about the window; typing a line and running a command are changes to it.
/// None of the four is work, and this model cannot see a window — see the
/// module's own doc.
fn needs_the_workspace(request: &Request) -> bool {
    matches!(
        request,
        Request::Where
            | Request::Commands
            | Request::Type { .. }
            | Request::Run { .. }
            | Request::Output
            | Request::Copy { .. }
    )
}

/// Whether this is a deed with text in it for the window to keep until it is
/// served, which is what [`MAX_WAITING`] counts.
///
/// Every one of them is also one [`only_from_a_gesture`] names; what a command
/// printed is the one of those that is asked for with nothing in it.
fn carries_text(request: &Request) -> bool {
    matches!(
        request,
        Request::Type { .. } | Request::Run { .. } | Request::Copy { .. }
    )
}

/// Whether this is a request that may only be raised out of an action a person
/// caused.
///
/// Two kinds are. The ones that *change* something — typing a line, running a
/// command, writing the clipboard — because a plugin that could do those on a
/// timer is a plugin that does them while nobody is looking. And the one that
/// is *about what was pressed*: what a command printed is an answer about the
/// menu an entry was run in, and there is no such menu when nobody ran one.
///
/// Ordinary reading is not on the list: a chip that says which branch you are
/// on has to be able to ask on a timer, and asking is what a grant already
/// answered for.
fn only_from_a_gesture(request: &Request) -> bool {
    matches!(
        request,
        Request::Type { .. } | Request::Run { .. } | Request::Copy { .. } | Request::Output
    )
}

/// The host a URL names, which is the thing a person granted or did not.
///
/// Parsed here rather than with a URL crate, and narrowly: only `https`, and
/// the authority is everything before the first `/`, `?` or `#`. Userinfo is
/// dropped from the front — `https://api.anthropic.com@evil.example/` is a
/// request to *evil.example*, and a check that read the front of the authority
/// would grant it on a permission somebody gave to Anthropic.
pub(super) fn host_of(url: &str) -> Result<String, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| String::from("Reach something over https"))?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    let host = authority.split(':').next().unwrap_or_default();

    if host.is_empty() {
        return Err(String::from("Reach a host that is named"));
    }
    Ok(host.to_ascii_lowercase())
}

/// Does what was asked, on the pool.
///
/// Everything [`needs_the_workspace`] answers `true` to is served in
/// `wasm::mod` instead and never arrives here; reaching one of those arms
/// would be this file disagreeing with itself, so it says so rather than
/// answering something plausible.
fn perform(request: Request) -> Answer {
    match request {
        Request::Fetch {
            method,
            url,
            headers,
            body,
        } => fetch(method, &url, &headers, body),
        Request::ReadFile { path } => read(&path),
        Request::Tally {
            root,
            extension,
            touched_since,
            containing,
            at_least,
            distinct_by,
            tables,
        } => tally(
            &root,
            &extension,
            touched_since,
            &containing,
            &at_least,
            &distinct_by,
            &tables,
        ),
        Request::PlaySound { wav, volume } => super::sound::play(&wav, volume),
        Request::List { path } => list_directory(&path),
        Request::Repository { path } => repository(&path),
        // Everything `needs_the_workspace` answers `true` to is served in
        // `wasm::mod` instead and never arrives here; reaching one of these
        // arms would be this file disagreeing with itself, so it says so
        // rather than answering something plausible.
        Request::Where
        | Request::Commands
        | Request::Type { .. }
        | Request::Run { .. }
        | Request::Output
        | Request::Copy { .. } => {
            Answer::Failed(String::from("that is not something the pool can do"))
        }
    }
}

/// Walks a directory of line-delimited JSON and hands back what it adds up to.
///
/// Everything expensive happens here rather than on the other side of the
/// boundary, and in this order, because each step is what makes the next one
/// affordable: a file nobody has touched is never opened, a line without the
/// needle is never parsed, a line parsed is looked at once, and what crosses
/// is the totals. The first shape of this handed the *lines* over and was
/// measured at ninety thousand instructions each inside the guest — forty
/// seconds for a week, which no page size fixes.
fn tally(
    root: &str,
    extension: &str,
    touched_since: i64,
    containing: &str,
    at_least: &[Bound],
    distinct_by: &[String],
    tables: &[Table],
) -> Answer {
    let Some(resolved) = resolve(root) else {
        return Answer::Failed(String::from("that is not a directory this can read"));
    };

    let mut seen: HashSet<u64> = HashSet::new();
    let mut counters: Vec<HashMap<String, Tallied>> =
        tables.iter().map(|_| HashMap::new()).collect();
    let mut lines = 0;

    for file in walk(&resolved, extension, touched_since) {
        if let Err(why) = count(
            &file,
            containing,
            at_least,
            distinct_by,
            tables,
            &mut seen,
            &mut counters,
            &mut lines,
        ) {
            // A file that cannot be read costs itself and not the walk: a
            // directory somebody else is writing into loses files while it is
            // being read, and that is not a reason to answer with nothing.
            log::debug!("{}: {why}", file.display());
        }
    }

    Answer::Counted {
        tables: counters
            .into_iter()
            .map(|counter| counter.into_values().collect())
            .collect(),
        lines,
    }
}

/// Folds one file into the running totals.
#[allow(clippy::too_many_arguments)]
fn count(
    path: &Path,
    containing: &str,
    at_least: &[Bound],
    distinct_by: &[String],
    tables: &[Table],
    seen: &mut HashSet<u64>,
    counters: &mut [HashMap<String, Tallied>],
    lines: &mut u64,
) -> std::io::Result<()> {
    use std::io::BufRead as _;

    let file = fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        // The whole of what makes walking a hundred megabytes cheap: a line
        // without the needle is never handed to a JSON parser.
        if !line.contains(containing) {
            continue;
        }
        let Ok(object) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };

        if !at_least.iter().all(|bound| clears(&object, bound)) {
            continue;
        }

        // A line missing part of its identity is counted rather than dropped:
        // an identity that is not there cannot say a line repeats anything.
        if !distinct_by.is_empty() {
            let mut identity = String::new();
            let mut whole = true;
            for field in distinct_by {
                match pick(&object, field) {
                    Field::Nothing => whole = false,
                    found => {
                        identity.push_str(&render(&found));
                        identity.push('\u{1f}');
                    }
                }
            }
            if whole && !seen.insert(hash_of(&identity)) {
                continue;
            }
        }

        *lines += 1;
        for (table, counter) in tables.iter().zip(counters.iter_mut()) {
            let key: Vec<Field> = table
                .by
                .iter()
                .map(|column| shortened(pick(&object, &column.field), column.prefix))
                .collect();
            let at: String = key.iter().map(render).collect::<Vec<_>>().join("\u{1f}");

            let row = counter.entry(at).or_insert_with(|| Tallied {
                key,
                sums: vec![0.; table.sum.len()],
                lines: 0,
            });
            for (index, field) in table.sum.iter().enumerate() {
                if let Field::Number(number) = pick(&object, field) {
                    row.sums[index] += number;
                }
            }
            row.lines += 1;
        }
    }
}

/// Whether a line's field sorts at or after a floor.
fn clears(object: &serde_json::Value, bound: &Bound) -> bool {
    match pick(object, &bound.field) {
        Field::Text(text) => text.as_str() >= bound.at_least.as_str(),
        // A number compared against text, or a field that is not there.
        // Not cleared: a floor nothing can be measured against is a floor.
        _ => false,
    }
}

/// The first `prefix` characters of a cell, or all of it.
fn shortened(cell: Field, prefix: Option<u32>) -> Field {
    match (cell, prefix) {
        (Field::Text(text), Some(prefix)) => {
            Field::Text(text.chars().take(prefix as usize).collect())
        }
        (cell, _) => cell,
    }
}

/// A cell as the text a key is built from.
fn render(cell: &Field) -> String {
    match cell {
        Field::Nothing => String::new(),
        Field::Text(text) => text.clone(),
        Field::Number(number) => number.to_string(),
    }
}

/// A hash of an identity, kept instead of the identity itself.
///
/// Forty thousand identities of eighty characters each is three megabytes held
/// for the length of a walk, against sixty-four bits apiece for the same
/// answer. What it costs is a collision every few billion — which would count
/// one turn as a repeat of another, in a total measured in millions.
fn hash_of(identity: &str) -> u64 {
    use std::hash::{BuildHasher as _, RandomState};

    // A hasher built once per process rather than per call, because a hash
    // that changed between two lines of the same walk would say every line was
    // new.
    static HASHER: std::sync::OnceLock<RandomState> = std::sync::OnceLock::new();
    HASHER.get_or_init(RandomState::new).hash_one(identity)
}

/// Every file worth opening, in an order that is the same on every walk.
///
/// Sorted, because a cursor is an ordinal into this list and a list that came
/// back in a different order would be a page that skipped half a directory and
/// read another half twice.
fn walk(root: &Path, extension: &str, touched_since: i64) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut looking = vec![root.to_path_buf()];

    while let Some(directory) = looking.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            if found.len() >= FILES_PER_WALK {
                break;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => looking.push(path),
                Ok(kind) if kind.is_file() => {
                    if !path.to_string_lossy().ends_with(extension) {
                        continue;
                    }
                    if touched_since > 0 && !touched_at_or_after(&path, touched_since) {
                        continue;
                    }
                    found.push(path);
                }
                _ => {}
            }
        }
    }

    found.sort();
    found
}

/// Whether a file has been written to since an instant.
///
/// Only ever used to *skip*: a file's modification time is the last turn in
/// it, so one that is older than the window cannot hold a line inside it —
/// while a file that is newer may hold turns far older, which is why nothing
/// here treats the time as the line's.
fn touched_at_or_after(path: &Path, millis: i64) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|facts| facts.modified()) else {
        // Unknowable is kept rather than skipped: a file whose time cannot be
        // read is a file whose lines have to be looked at.
        return true;
    };
    match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX) >= millis,
        // Before the epoch, which is a clock nobody should trust to skip on.
        Err(_) => true,
    }
}

/// One field out of one line, by its dotted path.
fn pick(object: &serde_json::Value, field: &str) -> Field {
    let mut at = object;
    for step in field.split('.') {
        match at.get(step) {
            Some(next) => at = next,
            None => return Field::Nothing,
        }
    }

    match at {
        serde_json::Value::Null => Field::Nothing,
        serde_json::Value::String(text) => Field::Text(text.clone()),
        serde_json::Value::Number(number) => match number.as_f64() {
            Some(number) => Field::Number(number),
            None => Field::Nothing,
        },
        // A shape this vocabulary has no cell for, handed over as it reads
        // rather than dropped: a plugin that asked for it knows what it is.
        other => Field::Text(other.to_string()),
    }
}

/// The names in one directory, if it is under a root the grant named.
///
/// Directories first and then files, each by name, which is the order every
/// file picker has used since the first one: the thing a person is walking
/// down is a directory, and a list that interleaves them is a list they have
/// to read twice.
///
/// Names alone. Not a size, not a time, not a permission bit — the capability
/// says "see the names of the files", and a host that also handed over the
/// rest would be a host whose permission dialog lied.
pub(super) fn list_directory(path: &str) -> Answer {
    let Some(resolved) = resolve(path) else {
        return Answer::Failed(String::from("that is not a path this can read"));
    };

    let entries = match std::fs::read_dir(&resolved) {
        Ok(entries) => entries,
        Err(why) => return Answer::Failed(why.to_string()),
    };

    let mut found: Vec<Entry> = Vec::new();
    for entry in entries.flatten() {
        if found.len() >= MAX_NAMES {
            break;
        }
        let Ok(name) = entry.file_name().into_string() else {
            // A name that is not UTF-8 cannot cross the wire, and a plugin
            // handed a lossy version of it would be a plugin asking to `cd`
            // somewhere that does not exist.
            continue;
        };
        // Followed rather than not: a symlink to a directory is a directory to
        // everybody who is about to walk into it.
        let directory = entry
            .metadata()
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        found.push(Entry { name, directory });
    }

    found.sort_by(|left, right| {
        right
            .directory
            .cmp(&left.directory)
            .then_with(|| left.name.cmp(&right.name))
    });
    Answer::Listed(found)
}

/// What the repository at a path is, read out of its files.
fn repository(path: &str) -> Answer {
    let Some(resolved) = resolve(path) else {
        return Answer::Failed(String::from("that is not a path this can read"));
    };

    let Some(layout) = crate::git::discover(&resolved) else {
        // Not in a repository is an ordinary answer and not a failure: most
        // directories are not in one, and a plugin that was told "failed"
        // would have to guess which kind of nothing it was handed.
        return Answer::Repository {
            head: None,
            branches: Vec::new(),
        };
    };

    Answer::Repository {
        head: crate::git::read_head(&layout.git_dir).map(|head| head.label().to_owned()),
        branches: crate::git::branches_in(&layout),
    }
}

/// One HTTP request, and whatever came back.
///
/// A status is *not* a failure: a 401 is an answer about a session and only
/// the plugin knows whether that is a problem, so it is handed over as it
/// stands. What is a failure is not reaching the host at all.
fn fetch(method: Method, url: &str, headers: &[(String, String)], body: Option<Vec<u8>>) -> Answer {
    let agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent();

    let sent = match method {
        Method::Get => {
            let mut request = agent.get(url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            request.call()
        }
        Method::Post => {
            let mut request = agent.post(url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            request.send(&body.unwrap_or_default()[..])
        }
    };

    let mut response = match sent {
        Ok(response) => response,
        Err(why) => return Answer::Failed(why.to_string()),
    };

    let status = response.status().as_u16();
    match response
        .body_mut()
        .with_config()
        .limit(MAX_ANSWER)
        .read_to_vec()
    {
        Ok(body) => Answer::Fetched { status, body },
        Err(why) => Answer::Failed(why.to_string()),
    }
}

/// One file, if it is where the grant said it was.
fn read(path: &str) -> Answer {
    let Some(resolved) = resolve(path) else {
        return Answer::Failed(String::from("that is not a path this can read"));
    };

    #[cfg(target_os = "macos")]
    if let Some(bytes) = from_the_keychain(&resolved) {
        return Answer::Read { bytes };
    }

    match fs::read(&resolved) {
        Ok(bytes) if bytes.len() as u64 > MAX_ANSWER => Answer::Failed(format!(
            "{} is {} bytes and the limit is {MAX_ANSWER}",
            resolved.display(),
            bytes.len()
        )),
        Ok(bytes) => Answer::Read { bytes },
        Err(why) => Answer::Failed(why.to_string()),
    }
}

/// Files a program writes to the keychain instead on macOS, and the
/// keychain item it writes them to.
///
/// Claude Code keeps its sign-in in `~/.claude/.credentials.json` on Linux and
/// Windows, and in the login keychain on macOS. The file can still be on a Mac
/// — left by an older Claude Code, never written to again — holding a token
/// that expired months ago, so a plugin granted the file read a session that
/// was always "expired" while Claude Code was signed in the whole time. The
/// item holds the same JSON the file would, so serving it in the file's place
/// is the grant meaning what it says: Claude Code's credentials, where that
/// platform keeps them.
#[cfg(target_os = "macos")]
const IN_THE_KEYCHAIN: &[(&str, &str)] =
    &[("~/.claude/.credentials.json", "Claude Code-credentials")];

/// The keychain item standing in for `resolved`, if it is one of
/// [`IN_THE_KEYCHAIN`].
#[cfg(target_os = "macos")]
fn keychain_item_for(resolved: &Path) -> Option<&'static str> {
    IN_THE_KEYCHAIN
        .iter()
        .find(|(file, _)| resolve(file).as_deref() == Some(resolved))
        .map(|(_, service)| *service)
}

/// What the keychain holds in place of `resolved`, or `None` to read the file.
///
/// Through `/usr/bin/security`, the tool Claude Code writes the item with,
/// which is what the item's access list already trusts: reading it this way
/// asks nobody anything. Anything short of an answer — no such item, a locked
/// keychain, a tool that failed — falls back to the file rather than failing
/// the read, so a Mac whose Claude Code does use the file keeps working.
///
/// Under [`REQUEST_TIMEOUT`], like every other request a plugin makes. A
/// keychain that is locked, or an item whose access list does not trust
/// `security` after all, is answered with a dialog — and `security` waits on
/// the dialog for as long as nobody answers it, holding a pool worker the
/// whole time. Past the deadline it is killed, which takes its dialog with
/// it, and the file is read instead.
#[cfg(target_os = "macos")]
fn from_the_keychain(resolved: &Path) -> Option<Vec<u8>> {
    let service = keychain_item_for(resolved)?;
    let (success, mut bytes) = output_within(
        "/usr/bin/security",
        &["find-generic-password", "-s", service, "-w"],
        REQUEST_TIMEOUT,
    )
    .map_err(|why| log::debug!("could not ask the keychain for {service:?}: {why}"))
    .ok()?;
    if !success {
        log::debug!("the keychain has no {service:?}; reading the file");
        return None;
    }
    // `-w` ends the secret with a newline of its own.
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes.pop();
    }
    (!bytes.is_empty() && bytes.len() as u64 <= MAX_ANSWER).then_some(bytes)
}

/// Runs `program` with `args` and nothing on its stdin, and waits for it up
/// to `timeout`, answering whether it succeeded and what it printed.
///
/// stdout is read on a thread of its own, so a program that prints more than
/// a pipe holds cannot wedge the wait; stderr goes nowhere. Past `timeout` the
/// program is killed and reaped, and that is an error.
#[cfg(target_os = "macos")]
fn output_within(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> std::io::Result<(bool, Vec<u8>)> {
    use std::io::Read as _;
    use std::process::Stdio;

    let mut child = crate::process::command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let (sender, printed) = std::sync::mpsc::channel();
    if let Some(mut stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            let _ = sender.send(bytes);
        });
    }

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("no answer within {}s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    // The program is gone, so what it printed is already in the pipe; a
    // second is for a reader somebody else's descriptor is holding open.
    let bytes = printed
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    Ok((status.success(), bytes))
}

/// Where a granted path actually is.
///
/// `~/` is the person's home directory and is the only thing expanded. A path
/// with `..` anywhere in it is refused rather than resolved: the grant is the
/// text of the path, so a path that can walk is a grant that means something
/// other than what it says.
pub(super) fn resolve(path: &str) -> Option<PathBuf> {
    if path.split('/').any(|part| part == "..") {
        return None;
    }
    // `~` on its own is the home directory itself, which a *root* is far more
    // likely to be than a file: a grant written `~` that resolved to a
    // directory literally called `~` would refuse everything and say nothing
    // about why.
    if path == "~" {
        return dirs::home_dir();
    }
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|home| home.join(rest)),
        None => Some(PathBuf::from(path)),
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
