//! What the guest may call, and what it leaves behind when it does.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crook_plugin_api::{Action, Contribution, Registered, Request};

/// How many answers one plugin may be waiting on at once.
///
/// Fuel bounds how long a call runs and this bounds what one can *leave
/// behind*: a guest looping on the request import spends its own budget, but
/// every request it manages to raise is a string the host is holding until
/// somebody answers it. Thirty-two is far more than a plugin has reason to
/// have outstanding and far less than a way to run a machine out of memory.
const MAX_ASKED: usize = 32;

/// What a plugin registered while it was building, and what it has asked for
/// since.
///
/// Shared between the host functions — which are called from inside the guest
/// and hold no `&mut` to anything of the host's — and the [`Sandbox`] that
/// reads it afterwards.
///
/// [`Sandbox`]: crate::Sandbox
#[derive(Clone, Default)]
pub struct Registry(Rc<RefCell<Inner>>);

/// The shared half.
#[derive(Default)]
struct Inner {
    /// Contributions and actions, which are a build's whole output.
    registered: Registered,
    /// Requests raised and not yet handed to the host, with the tickets their
    /// answers will carry.
    asked: Vec<(u32, Request)>,
    /// The last ticket handed out. Never reused inside one session, so an
    /// answer that arrives after the plugin stopped caring is an answer to a
    /// ticket nothing is waiting on rather than to somebody else's.
    tickets: u32,
    /// How long the guest asked to be left alone for.
    timer: Option<Duration>,
    /// How far this machine's time is from UTC, in minutes east.
    ///
    /// Set by the host rather than worked out here: a time zone is a table
    /// that ships with an operating system, and this crate is four
    /// dependencies with no clock among them.
    timezone: i32,
}

impl Registry {
    /// An empty one.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a contribution.
    pub(crate) fn contribute(&self, slot: String, entry: String, order: i32) {
        self.0
            .borrow_mut()
            .registered
            .contributions
            .push(Contribution { slot, entry, order });
    }

    /// Records an action.
    pub(crate) fn register_action(&self, name: String, title: Option<String>) {
        self.0
            .borrow_mut()
            .registered
            .actions
            .push(Action { name, title });
    }

    /// Everything registered so far.
    pub fn taken(&self) -> Registered {
        self.0.borrow().registered.clone()
    }

    /// Forgets it, which is what a build that failed leaves behind.
    ///
    /// The requests go too. A plugin whose build gave up half way has not
    /// established what it is, and doing the network call it asked for on the
    /// way would be doing a thing on behalf of a plugin that does not exist.
    pub(crate) fn clear(&self) {
        *self.0.borrow_mut() = Inner::default();
    }

    /// Records a request and hands back the ticket its answer will carry.
    ///
    /// Zero when there is no room, which the guest reads as "not asked" — a
    /// number rather than a trap, because a plugin that asks for too much at
    /// once has a bug in its own scheduling and not a reason to lose a frame.
    pub(crate) fn ask(&self, request: Request) -> u32 {
        let mut inner = self.0.borrow_mut();
        if inner.asked.len() >= MAX_ASKED {
            return 0;
        }
        inner.tickets += 1;
        let ticket = inner.tickets;
        inner.asked.push((ticket, request));
        ticket
    }

    /// Takes what has been asked for, leaving nothing behind.
    pub(crate) fn asked(&self) -> Vec<(u32, Request)> {
        std::mem::take(&mut self.0.borrow_mut().asked)
    }

    /// Records that the guest would like to be ticked.
    ///
    /// The last one wins rather than the shortest: a plugin that asks twice in
    /// one call has changed its mind, and a host that kept the earlier answer
    /// would be scheduling on a decision the plugin has already replaced.
    pub(crate) fn wants_ticking_in(&self, after: Duration) {
        self.0.borrow_mut().timer = Some(after);
    }

    /// Takes the timer the guest asked for, if it asked for one.
    pub(crate) fn timer(&self) -> Option<Duration> {
        self.0.borrow_mut().timer.take()
    }

    /// Tells it what this machine's offset from UTC is.
    pub(crate) fn set_timezone(&self, minutes: i32) {
        self.0.borrow_mut().timezone = minutes;
    }

    /// What it was last told.
    pub(crate) fn timezone(&self) -> i32 {
        self.0.borrow().timezone
    }
}
