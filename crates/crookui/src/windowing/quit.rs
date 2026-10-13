//! macOS's Quit, made to ask before it ends work that is still going.
//!
//! The application menu's Quit, `cmd-q` — its key equivalent, which AppKit
//! takes before the window sees a key — the Dock icon's Quit, and a logout,
//! restart or shutdown all reach a Mac application the same way: AppKit's
//! `terminate:`. That asks the application's delegate
//! `applicationShouldTerminate:` and, unless the answer is no, tells it
//! `applicationWillTerminate:` and ends the process from inside AppKit. None
//! of them is a window closing, so none of them is winit's `CloseRequested`,
//! and winit's delegate implements only the second message. Left that way,
//! every one of them ended every pane at once, with no moment in which the
//! window could ask whether to — while the window's own close, a click away,
//! did ask.
//!
//! So the first message is added to winit's delegate class here, at run
//! time, the way winit itself puts its own `sendEvent:` on the application's
//! class. It answers from a [`Guard`]:
//!
//! - **Nothing to ask about**, and the quit goes on exactly as it always did:
//!   AppKit ends the process through `applicationWillTerminate:`, which winit
//!   turns into the loop's exit, and [`run`](super::run) never returns. A
//!   logout with nothing working is not held up by a single extra step.
//! - **Something to ask about**, and the quit is refused —
//!   `NSTerminateCancel` — and handed to the window as its own close, through
//!   [`WindowDelegate::close_requested`](super::WindowDelegate::close_requested)
//!   on the event loop's next turn, which puts the question up and brings the
//!   window forward as a desktop's close does. A logout is cancelled with it,
//!   which is what Terminal and iTerm2 do over a running process. Ending them
//!   from the card then leaves the way a window close does, through the event
//!   loop rather than `terminate:`, so nothing is asked twice and nothing
//!   loops.
//!
//! # Why the answer is one written down earlier
//!
//! AppKit wants it before the method returns, and the one thing that knows —
//! the application, behind [`WindowDelegate`](super::WindowDelegate) — is
//! held by winit's event loop for the whole of the run, where a method
//! AppKit calls cannot reach it. So the loop asks it every time it goes idle
//! ([`set_asks`]), which is after every batch of events, and the method reads
//! what it said. Nothing ticks for it: an idle loop asks nothing.
//!
//! What it can miss is a change within the batch the quit itself arrived in:
//! a command that started in the same few milliseconds as `cmd-q` is quit
//! over without a question, which is what every quit did before. The other
//! way round costs nothing, since a refused quit is asked about afresh when
//! it reaches the window, and a close with nothing working there goes at
//! once.
//!
//! # Why refuse rather than wait
//!
//! AppKit has a third answer, `NSTerminateLater`, which holds the quit — and
//! a logout — open until the application replies. It would let a logout over
//! nothing go on without being cancelled first, but the reply would have to
//! come from the application, which is only ever reached inside one of
//! winit's callbacks, and a "yes" ends the process right there: AppKit says
//! `applicationWillTerminate:`, winit hands its handler the loop's exit while
//! that handler is still running, and winit panics rather than run it twice
//! at once. Refusing keeps both of AppKit's paths as they were: the one that
//! ends the process is the one it always used, and the one that does not is
//! the window close Crook already has.
//!
//! # Why not a menu of Crook's own
//!
//! winit can leave its default menu out (`with_default_menu(false)`), and a
//! menu of Crook's own could send its Quit anywhere. But only `cmd-q` and the
//! menu item go through a menu: the Dock's Quit and a logout send
//! `terminate:` to the application directly, and only its delegate hears
//! them.

use std::cell::Cell;

#[cfg(target_os = "macos")]
use objc2::runtime::{AnyClass, AnyObject, Sel};
#[cfg(target_os = "macos")]
use objc2_app_kit::NSApplicationTerminateReply;

/// What macOS's Quit is answered with: whether quitting would ask, as the
/// application last said, and who to hand a refused quit to.
pub(super) struct Guard {
    /// Whether a quit would stop to ask, as of the last time the event loop
    /// went idle.
    asks: Cell<bool>,
    /// Hands a refused quit to the window, as a close of its own.
    ask: Box<dyn Fn()>,
}

impl Guard {
    /// A guard that lets every quit through until it is told otherwise, and
    /// hands one it refuses to `ask`.
    pub(super) fn new(ask: impl Fn() + 'static) -> Self {
        Self {
            asks: Cell::new(false),
            ask: Box::new(ask),
        }
    }

    /// Writes down whether a quit would stop to ask.
    pub(super) fn set_asks(&self, asks: bool) {
        self.asks.set(asks);
    }

    /// AppKit's question: whether the application may end now.
    ///
    /// Yes when there is nothing to ask about. Otherwise no, and the window
    /// is asked instead — every time: a second `cmd-q` over the question is a
    /// second close, which asks about itself as a second close always does.
    pub(super) fn may_quit(&self) -> bool {
        if !self.asks.get() {
            return true;
        }
        (self.ask)();
        false
    }
}

#[cfg(target_os = "macos")]
thread_local! {
    /// The guard the delegate's method answers from.
    ///
    /// Per thread because AppKit calls the method on the main thread and the
    /// event loop runs there, and a static would have to be `Sync` for a
    /// boxed closure holding the event loop's proxy. Set once: winit lets a
    /// process build one event loop.
    static GUARD: std::cell::OnceCell<Guard> = const { std::cell::OnceCell::new() };
}

/// The method added to winit's delegate class, as AppKit calls it: the
/// delegate, the selector, and the application asking.
///
/// The application as a bare pointer, since nothing here reads it and a
/// reference would make a nil one — which is what a test has to send —
/// undefined behaviour.
#[cfg(target_os = "macos")]
type ShouldTerminate =
    extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> NSApplicationTerminateReply;

/// Teaches winit's application delegate `applicationShouldTerminate:`, and
/// hands every quit it refuses to `ask`.
///
/// After the event loop is built, which is when winit makes its delegate and
/// hands it to the application, and before it runs. Where the delegate
/// cannot be taught — off the main thread, with no delegate, or a winit that
/// has learned the method itself — the quit goes on as it did before, and
/// the log says so.
#[cfg(target_os = "macos")]
pub(super) fn install(ask: impl Fn() + 'static) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(main_thread) = MainThreadMarker::new() else {
        log::warn!("the Quit question was set up off the main thread; Quit will not ask");
        return;
    };
    let application = NSApplication::sharedApplication(main_thread);
    let Some(delegate) = application.delegate() else {
        log::warn!("the application has no delegate to answer Quit; Quit will not ask");
        return;
    };

    // Set before the method exists, so that no call can find it missing.
    if GUARD.with(|guard| guard.set(Guard::new(ask))).is_err() {
        log::warn!("the Quit question was set up twice; the first one stands");
        return;
    }

    let class = AsRef::<AnyObject>::as_ref(&*delegate).class();
    if !teach(class) {
        log::warn!(
            "{} already answers applicationShouldTerminate:; Quit will not ask",
            class.name().to_string_lossy()
        );
        return;
    }

    // Handed over again, because the application may have written down which
    // optional messages its delegate answers when it was handed it, and this
    // one was added after. Through nothing, so that handing over the same
    // object is not taken as no change.
    application.setDelegate(None);
    application.setDelegate(Some(&*delegate));
}

/// Adds `applicationShouldTerminate:` to `class`, answered by
/// [`should_terminate`], and says whether it did.
///
/// Not over a class that answers it already, whose own answer is left alone:
/// the runtime adds a method only to a class with none of its own by that
/// name.
#[cfg(target_os = "macos")]
fn teach(class: &AnyClass) -> bool {
    use std::ffi::CString;
    use std::ptr;

    use objc2::runtime::Imp;
    use objc2::{Encode, ffi, sel};

    // Spelled out of the types rather than written as "Q@:@", so a reply
    // type that changed width would change the encoding with it.
    let types = CString::new(format!(
        "{}{}{}{}",
        NSApplicationTerminateReply::ENCODING,
        <&AnyObject>::ENCODING,
        Sel::ENCODING,
        <*mut AnyObject>::ENCODING,
    ))
    .expect("a type encoding has no NUL in it");

    let method: ShouldTerminate = should_terminate;
    // SAFETY: an `Imp` is any method's function pointer, called by the
    // runtime with the arguments the encoding names, which are the ones
    // `should_terminate` takes.
    let imp = unsafe { std::mem::transmute::<ShouldTerminate, Imp>(method) };
    // SAFETY: `class` is a registered class, and `types` is the encoding of
    // `imp`'s return and arguments, self and the selector included.
    unsafe {
        ffi::class_addMethod(
            ptr::from_ref(class).cast_mut(),
            sel!(applicationShouldTerminate:),
            imp,
            types.as_ptr(),
        )
    }
    .as_bool()
}

/// Writes down whether a quit would stop to ask, for the next one AppKit
/// sends. Nothing before [`install`].
#[cfg(target_os = "macos")]
pub(super) fn set_asks(asks: bool) {
    // `try_with` because the loop's last word comes as the process ends, when
    // a thread's locals may already be gone; a quit then has nobody to ask.
    let _ = GUARD.try_with(|guard| {
        if let Some(guard) = guard.get() {
            guard.set_asks(asks);
        }
    });
}

/// `applicationShouldTerminate:`, as winit's delegate now answers it.
///
/// Nothing in it can panic, since an unwind would cross AppKit's frames. A
/// guard that was never set, or is gone, lets the quit through, as AppKit did
/// before there was one.
#[cfg(target_os = "macos")]
extern "C-unwind" fn should_terminate(
    _: &AnyObject,
    _: Sel,
    _: *mut AnyObject,
) -> NSApplicationTerminateReply {
    let may_quit = GUARD
        .try_with(|guard| guard.get().is_none_or(Guard::may_quit))
        .unwrap_or(true);
    if may_quit {
        NSApplicationTerminateReply::TerminateNow
    } else {
        NSApplicationTerminateReply::TerminateCancel
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    /// A guard, and how many times it has handed a quit to the window.
    fn guard() -> (Guard, Rc<Cell<usize>>) {
        let asked = Rc::new(Cell::new(0));
        let counter = Rc::clone(&asked);
        (Guard::new(move || counter.set(counter.get() + 1)), asked)
    }

    #[test]
    fn a_quit_with_nothing_to_ask_about_goes_at_once() {
        let (guard, asked) = guard();
        assert!(guard.may_quit(), "a quit before anything was said was held");

        guard.set_asks(true);
        guard.set_asks(false);
        assert!(guard.may_quit(), "a quit over nothing was held");
        assert_eq!(asked.get(), 0, "the window was asked about nothing");
    }

    #[test]
    fn a_quit_with_something_to_ask_about_is_refused_and_handed_to_the_window() {
        let (guard, asked) = guard();
        guard.set_asks(true);

        assert!(!guard.may_quit(), "the quit went with something working");
        assert_eq!(asked.get(), 1, "the window was never asked");

        assert!(!guard.may_quit(), "a second quit went over the question");
        assert_eq!(asked.get(), 2, "a second quit did not ask again");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_delegate_taught_the_question_answers_it_as_the_guard_says() {
        use std::ptr;

        use objc2::rc::Retained;
        use objc2::runtime::{ClassBuilder, NSObject};
        use objc2::{ClassType, msg_send};

        // A class of the test's own in place of winit's delegate, which
        // exists only once an event loop does, and only one per process.
        let class = ClassBuilder::new(c"CrookQuitTestDelegate", NSObject::class())
            .expect("no other class has the name")
            .register();
        assert!(teach(class), "the class was not taught the question");
        assert!(
            !teach(class),
            "a class that answers already was taught over"
        );

        let (guard, asked) = guard();
        assert!(GUARD.with(|cell| cell.set(guard)).is_ok());
        // SAFETY: `new` is NSObject's, and the class adds no state to it.
        let delegate: Retained<AnyObject> = unsafe { msg_send![class, new] };
        let quit = || -> NSApplicationTerminateReply {
            // SAFETY: the method was added above with this signature, and
            // never reads the application it is handed.
            unsafe {
                msg_send![&*delegate, applicationShouldTerminate: ptr::null_mut::<AnyObject>()]
            }
        };

        assert_eq!(
            quit(),
            NSApplicationTerminateReply::TerminateNow,
            "a quit over nothing was refused"
        );
        set_asks(true);
        assert_eq!(
            quit(),
            NSApplicationTerminateReply::TerminateCancel,
            "a quit over working agents went"
        );
        assert_eq!(asked.get(), 1, "the refused quit never reached the window");
    }
}
