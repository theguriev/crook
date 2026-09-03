//! The system clipboard, opened once and never complained about.
//!
//! Every pane's input shares one of these. That is not a nicety: opening a
//! clipboard connects to the window server on macOS, to X11 or Wayland on
//! Linux and to the OLE subsystem on Windows, which is far too much to pay per
//! keystroke — and on Windows the clipboard is a global lock that a second
//! handle can be refused outright.
//!
//! Opening is deferred to the first copy or paste, and a failure is recorded
//! rather than retried: a machine with no clipboard at all — a headless CI
//! runner, a session with no display — must not attempt a connection on every
//! `cmd-v`, and must not show anybody an error for a key they pressed. There is
//! nothing to be done about it and nothing worth saying, so the failure is a
//! line in the log and "no clipboard" everywhere else.
//!
//! One footnote, on X11 only: a selection there is owned by the process that
//! set it, so text copied out of Crook stops being available the moment Crook
//! exits. That is the protocol, not a bug, and the workaround — a daemon
//! thread that keeps serving the selection after the process would otherwise
//! be done — is a cost that belongs to a clipboard manager rather than to a
//! terminal.

use std::cell::RefCell;
use std::rc::Rc;

/// The system clipboard, as an input uses one.
///
/// Cheap to clone — it is an [`Rc`] — so every pane can hold the same one.
#[derive(Clone, Default)]
pub struct Clipboard(Rc<RefCell<State>>);

/// Whether the clipboard has been opened, and how that went.
#[derive(Default)]
enum State {
    /// Nothing has been copied or pasted yet.
    #[default]
    Unopened,
    /// Open, and usable.
    Open(arboard::Clipboard),
    /// It could not be opened, and will not be tried again.
    Unavailable,
}

impl Clipboard {
    /// A clipboard that has not been opened yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// What is on the clipboard, or `None` when there is nothing there and
    /// when there is no clipboard to ask.
    pub fn read(&self) -> Option<String> {
        self.with(|clipboard| clipboard.get_text())
    }

    /// Puts `text` on the clipboard, or does nothing at all when there is no
    /// clipboard to put it on.
    pub fn write(&self, text: &str) {
        self.with(|clipboard| clipboard.set_text(text.to_owned()));
    }

    /// Runs `use_clipboard` against the open clipboard, opening it if this is
    /// the first time and reporting `None` if it cannot be opened.
    fn with<T>(
        &self,
        use_clipboard: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
    ) -> Option<T> {
        let mut state = self.0.borrow_mut();
        if matches!(*state, State::Unopened) {
            *state = match arboard::Clipboard::new() {
                Ok(clipboard) => State::Open(clipboard),
                Err(error) => {
                    log::warn!("there is no clipboard on this machine: {error}");
                    State::Unavailable
                }
            };
        }

        let State::Open(clipboard) = &mut *state else {
            return None;
        };
        match use_clipboard(clipboard) {
            Ok(outcome) => Some(outcome),
            // An empty clipboard, a clipboard holding an image, another
            // process holding the Windows lock: all of them mean "not now"
            // rather than "never", so the handle is kept.
            Err(error) => {
                log::debug!("the clipboard could not be used: {error}");
                None
            }
        }
    }
}
