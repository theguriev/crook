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
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use crookui_core::executor::Task;
use crookui_core::prelude::*;

use crook_plugin::PluginId;
use crook_plugin_api::{Answer, Capability, Method, Request};
use crook_wasm::Sandbox;

use super::GIVE_UP_AFTER;

/// How long one request may take before it is given up on.
///
/// The same fifteen seconds the usage poller used when it was in the box. A
/// plugin waiting on a socket costs nobody a frame — the work is on the pool —
/// but a request with no ceiling is a background thread held for the rest of
/// the session.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

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

/// The most a plugin may be handed back from one request.
///
/// A megabyte, which is the sandbox's own ceiling on an answer: a bigger one
/// could not be delivered anyway, and finding that out after reading two
/// hundred megabytes into memory would be finding it out too late.
const MAX_ANSWER: u64 = 1 << 20;

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
    /// The tick that is coming, held so that the next one can take its place.
    ///
    /// Held rather than detached, and that is what lets a plugin change its
    /// mind about *when*. A guest asks for one timer at a time and cannot take
    /// an answer back, so a chip that wants to be woken every hundred
    /// milliseconds while somebody is waiting — the bite in the usage chip's
    /// mark — would otherwise be stuck with the minute it asked for before the
    /// click. Dropping the task cancels the wake-up, so the newest ask is the
    /// only one that lands.
    ///
    /// The sleep itself runs on to its end on the pool: a thread that is
    /// sleeping cannot be interrupted, only ignored when it wakes.
    waiting: Option<Task<()>>,
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
            waiting: None,
        }
    }

    /// Takes everything the guest asked for and starts it.
    ///
    /// Called after every call that can reach a context, which is what makes
    /// "ask, and be answered later" a loop rather than a single shot: a
    /// delivery pumps again, so a plugin that reads a file and then fetches
    /// what the file authorised it to fetch needs no special case.
    pub(super) fn pump(&mut self, ctx: &mut ModelContext<Self>) {
        let (asked, timer) = {
            // Already running: the guest reached back into itself, which it
            // cannot do through this API. Nothing to take.
            let Ok(mut sandbox) = self.sandbox.try_borrow_mut() else {
                return;
            };
            (sandbox.asked(), sandbox.timer())
        };

        for (ticket, request) in asked {
            self.start(ticket, request, ctx);
        }
        if let Some(after) = timer {
            self.wait(after, ctx);
        }
    }

    /// Checks one request against the grant and starts it if it is inside.
    fn start(&mut self, ticket: u32, request: Request, ctx: &mut ModelContext<Self>) {
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

        // A refusal goes round the pool exactly as the work would, and that is
        // not ceremony: answering it here would mean `deliver` running inside
        // `pump`, and a guest that answers a refusal by asking again would
        // take the host's stack down with it. Off the foreground and back is
        // what makes the loop a loop rather than a recursion.
        let working = ctx.background().spawn(async move {
            match refused {
                Some(sentence) => Answer::Refused(sentence),
                None => perform(request),
            }
        });
        ctx.spawn(working, move |runtime, answer, ctx| {
            runtime.answer(ticket, answer, ctx);
        })
        .detach();
    }

    /// Hands one answer to the guest, and takes whatever that made it ask for.
    fn answer(&mut self, ticket: u32, answer: Answer, ctx: &mut ModelContext<Self>) {
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
                self.later(move |runtime, ctx| runtime.answer(ticket, answer, ctx), ctx);
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

        self.pump(ctx);
        // The reading changed, so whatever is drawing it has to be asked
        // again. This is the bridge every model-backed feature in Crook has.
        ctx.notify();
    }

    /// Sleeps for as long as the guest asked and then ticks it.
    ///
    /// A sleep on the pool rather than a timer of the foreground's own, which
    /// is what everything else here that waits does: there is no foreground
    /// timer, and a plugin is not the place to introduce one.
    fn wait(&mut self, after: Duration, ctx: &mut ModelContext<Self>) {
        let sleeping = ctx.background().spawn(async move {
            std::thread::sleep(after);
        });
        // Assigned rather than detached: whatever was coming is dropped here,
        // and dropping it cancels it. See the field.
        self.waiting = Some(ctx.spawn(sleeping, |runtime, (), ctx| runtime.tick(ctx)));
    }

    /// Does something to this runtime a moment from now.
    ///
    /// A sleep on the pool, like every other wait here. What it is for is the
    /// one case a borrow of the guest can fail — see [`RETRY_AFTER`].
    fn later<F>(&mut self, what: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut Self, &mut ModelContext<Self>) + 'static,
    {
        let sleeping = ctx.background().spawn(async move {
            std::thread::sleep(RETRY_AFTER);
        });
        ctx.spawn(sleeping, move |runtime, (), ctx| what(runtime, ctx))
            .detach();
    }

    /// Tells the guest its wait is over.
    fn tick(&mut self, ctx: &mut ModelContext<Self>) {
        // Whatever asked for this one has been answered; anything the guest
        // asks for while it is being ticked replaces it below.
        self.waiting = None;
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
                self.later(Runtime::tick, ctx);
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

        self.pump(ctx);
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
fn perform(request: Request) -> Answer {
    match request {
        Request::Fetch {
            method,
            url,
            headers,
            body,
        } => fetch(method, &url, &headers, body),
        Request::ReadFile { path } => read(&path),
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

    match std::fs::read(&resolved) {
        Ok(bytes) if bytes.len() as u64 > MAX_ANSWER => Answer::Failed(format!(
            "{} is {} bytes and the limit is {MAX_ANSWER}",
            resolved.display(),
            bytes.len()
        )),
        Ok(bytes) => Answer::Read { bytes },
        Err(why) => Answer::Failed(why.to_string()),
    }
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
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|home| home.join(rest)),
        None => Some(PathBuf::from(path)),
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
