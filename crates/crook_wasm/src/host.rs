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

/// How many contributions and actions one plugin may leave behind, together.
///
/// The same reasoning as [`MAX_ASKED`], for the other thing a guest can make
/// the host hold onto. Fuel bounds how long `build` runs, but the host
/// functions it calls are native and unmetered, so a guest looping on
/// `contribute` grows a host `Vec` its own budget never pays for — and these
/// imports are reachable from every later call too, none of which clears what
/// they left. Two hundred and fifty-six is far more than a plugin has reason
/// to register and far less than a way to run a machine out of memory.
const MAX_REGISTERED: usize = 256;

/// Whether a plugin has registered everything [`MAX_REGISTERED`] allows.
///
/// Counts contributions and actions together: the ceiling is on what the host
/// holds, and it holds both.
fn registers_full(registered: &Registered) -> bool {
    registered.contributions.len() + registered.actions.len() >= MAX_REGISTERED
}

/// The soonest a plugin can ask to be ticked again.
///
/// The sibling of the other two ceilings here: fuel bounds how long a call
/// runs, [`MAX_ASKED`] and [`MAX_REGISTERED`] bound what a call leaves behind,
/// and this bounds how *often* a plugin can make the host come back to it. A
/// zero — or sub-frame — interval spins the tick chain with no wait between
/// wakes: the host wakes the guest, it asks to be woken again at once, and a
/// core is pegged for a plugin that re-arms on every tick. Floored to a frame,
/// the soonest a plugin is woken is the soonest a host has any use for.
const MIN_TICK: Duration = Duration::from_millis(16);

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

    /// Records a contribution, unless the plugin has already registered as
    /// much as it is allowed to leave behind.
    pub(crate) fn contribute(&self, slot: String, entry: String, order: i32) {
        let mut inner = self.0.borrow_mut();
        if registers_full(&inner.registered) {
            return;
        }
        inner
            .registered
            .contributions
            .push(Contribution { slot, entry, order });
    }

    /// Records an action, under the same ceiling as [`contribute`].
    ///
    /// [`contribute`]: Self::contribute
    pub(crate) fn register_action(&self, name: String, title: Option<String>) {
        let mut inner = self.0.borrow_mut();
        if registers_full(&inner.registered) {
            return;
        }
        inner.registered.actions.push(Action { name, title });
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

    /// Records that the guest would like to be ticked, no sooner than a frame.
    ///
    /// The last one wins rather than the shortest: a plugin that asks twice in
    /// one call has changed its mind, and a host that kept the earlier answer
    /// would be scheduling on a decision the plugin has already replaced.
    ///
    /// Floored at [`MIN_TICK`]: a zero interval would ask the host to wake the
    /// guest with no wait at all, and a plugin that re-arms on every tick would
    /// spin a core rather than be drawn.
    pub(crate) fn wants_ticking_in(&self, after: Duration) {
        self.0.borrow_mut().timer = Some(after.max(MIN_TICK));
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
