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
//!
//! # PRIMARY, on Linux
//!
//! An X11 desktop keeps a second selection beside the clipboard. Selecting
//! text anywhere puts it there, with no chord, and a middle click anywhere
//! pastes it; every Linux terminal does both, and a Linux hand reaches for it
//! without thinking. So the end of a drag here offers what it selected as
//! PRIMARY — [`Clipboard::write_primary`] — and a middle press in a pane
//! pastes it, through [`Clipboard::read_primary`]. The clipboard proper is
//! left alone by both: selecting is not copying, and `cmd-v` still pastes
//! what was last copied.
//!
//! **Only X11's.** `arboard` is built without its Wayland backend (see the
//! workspace manifest), so a Wayland session reaches PRIMARY through XWayland,
//! and how well that meets a native Wayland program's selection is the
//! compositor's bridge. macOS and Windows have nothing of the kind, and there
//! both calls do nothing — see [`HAS_PRIMARY`].
//!
//! **Read off the thread that draws.** Writing PRIMARY is taking ownership of
//! it and returns at once; reading it is a request to whichever program owns
//! it, answered when that program gets round to it. `arboard` waits four
//! seconds for one that never does — once for each of the six text formats it
//! asks for in turn, so about half a minute in all. So a read is handed out as
//! something to run on the pool rather than done here, and the workspace keeps
//! one out at a time for each time Crook takes PRIMARY — see
//! [`primary_taken`].
//!
//! **Never the machine's in a test.** PRIMARY is every selection a person
//! makes on their desktop, and a test run makes hundreds: a suite that wrote
//! the real one would paste its own output into whatever the person running
//! it middle-clicked next. A test build keeps PRIMARY in memory, one per
//! clipboard and shared by its clones, which is also what lets a test on a
//! machine with no display see what a drag offered.
//!
//! Which leaves the few lines that do reach the desktop's PRIMARY — the
//! `machine_primary` module, and the selection it names to `arboard` —
//! unreached by the suite. One test does reach them, and it is ignored unless
//! asked for by name, on a Linux desktop:
//! `cargo test -p crook --lib -- --ignored the_desktop_s_primary`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Whether this platform's desktop keeps a PRIMARY selection beside the
/// clipboard: X11's, or Wayland's through XWayland.
///
/// The setting that turns it off is shown only where this is true, and
/// [`primary_in_use`] answers `false` wherever it is not.
pub const HAS_PRIMARY: bool = cfg!(all(unix, not(target_os = "macos")));

thread_local! {
    /// Whether the window on this thread uses PRIMARY: the setting "Middle-click
    /// pastes the selection".
    ///
    /// Per thread, as the theme in force is, and for the same two reasons. It
    /// is the window's — one window to a process, drawn on one thread — and it
    /// reaches every clipboard on it, the command palette's own included,
    /// without being handed to each of the dozen fields that select. And the
    /// window's test harness runs on the test's own thread, so a test that
    /// turns it off turns it off for no other test.
    static PRIMARY_ON: Cell<bool> = const { Cell::new(true) };

    /// How many times a selection on this thread's window has taken PRIMARY.
    /// See [`primary_taken`].
    static PRIMARY_TAKEN: Cell<u64> = const { Cell::new(0) };
}

/// Says whether the window on this thread uses PRIMARY, which is what the
/// setting "Middle-click pastes the selection" writes.
pub fn use_primary(on: bool) {
    PRIMARY_ON.set(on);
}

/// Whether a selection made on this thread's window is offered as PRIMARY and
/// a middle press pastes it: a desktop that has one, with the setting on.
pub fn primary_in_use() -> bool {
    HAS_PRIMARY && PRIMARY_ON.get()
}

/// How many times a selection on this thread's window has taken PRIMARY: a
/// number that moves each time one does, through any clipboard on the window,
/// the command palette's own included.
///
/// What tells a middle press whether a read already on its way still asks the
/// right program. A read is put to whoever held PRIMARY when it began, and if
/// Crook has taken it since, that answer — late, from a program that may have
/// stopped answering — is not what is selected now, while a read begun now is
/// answered at once from what Crook holds. Per thread, as [`use_primary`] is,
/// and for the same reasons. Another program taking PRIMARY does not move it:
/// nothing tells Crook.
pub fn primary_taken() -> u64 {
    PRIMARY_TAKEN.get()
}

/// The system clipboard, as an input uses one.
///
/// Cheap to clone — it is an [`Rc`] — so every pane can hold the same one.
#[derive(Clone, Default)]
pub struct Clipboard(Rc<Shared>);

/// What every clone of one clipboard shares.
#[derive(Default)]
struct Shared {
    state: RefCell<State>,
    /// PRIMARY, in a test build, which never touches the machine's. See the
    /// module's last section.
    #[cfg(test)]
    primary: RefCell<Option<String>>,
    /// Whether the program holding that PRIMARY has stopped answering, and
    /// what a read put to it waits on. See [`Clipboard::select_and_hang`].
    #[cfg(test)]
    hung: RefCell<Option<std::sync::Arc<Unanswered>>>,
}

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

    /// Puts `text` on the clipboard, reporting whether it got there.
    ///
    /// `false` is a machine with no clipboard, or one whose clipboard was busy
    /// — never a reason to show anybody an error, and never a reason for the
    /// caller to retry.
    pub fn write(&self, text: &str) -> bool {
        self.with(|clipboard| clipboard.set_text(text.to_owned()))
            .is_some()
    }

    /// Offers `text` as PRIMARY, which is what a selection that has just been
    /// made does on Linux, reporting whether it got there.
    ///
    /// `false`, and nothing written, wherever [`primary_in_use`] is `false`
    /// and for an empty selection — a click that selected nothing takes
    /// nothing away from the program that holds PRIMARY now, which is how
    /// every X11 program behaves. Never the clipboard proper.
    pub fn write_primary(&self, text: &str) -> bool {
        if text.is_empty() || !primary_in_use() {
            return false;
        }
        let taken = self.put_on_primary(text);
        if taken {
            PRIMARY_TAKEN.set(PRIMARY_TAKEN.get().wrapping_add(1));
        }
        taken
    }

    /// A read of PRIMARY, to be run off the thread that draws, or `None`
    /// where there is none to read: a desktop without one, the setting off,
    /// or a machine whose clipboard could not be opened.
    ///
    /// Run it on the pool; see the module's section on why. Its answer is
    /// `None` for a PRIMARY nobody holds, one holding something that is not
    /// text, and an owner that never answered.
    pub fn read_primary(&self) -> Option<impl FnOnce() -> Option<String> + Send + 'static> {
        if !primary_in_use() {
            return None;
        }
        self.primary_reader()
    }

    /// What is on PRIMARY, as a test sees it.
    #[cfg(test)]
    pub fn primary(&self) -> Option<String> {
        self.0.primary.borrow().clone()
    }

    /// Puts `text` on PRIMARY the way another program selecting it would:
    /// whatever the setting says, which is not another program's to read.
    #[cfg(test)]
    pub fn select_elsewhere(&self, text: &str) {
        *self.0.primary.borrow_mut() = Some(text.to_owned());
        *self.0.hung.borrow_mut() = None;
    }

    /// Puts `text` on PRIMARY the way [`Self::select_elsewhere`] does, from a
    /// program that then stops answering — frozen, or suspended with Ctrl-Z.
    ///
    /// A read begun while it holds PRIMARY waits until the [`HungOwner`]
    /// handed back answers or is dropped, and then has `text`, which is what
    /// such a program says when it is resumed: it answers what it was asked
    /// before it stopped, whoever has taken PRIMARY since. A read begun once
    /// somebody else has taken it is answered by them.
    #[cfg(test)]
    pub fn select_and_hang(&self, text: &str) -> HungOwner {
        self.select_elsewhere(text);
        let unanswered = std::sync::Arc::<Unanswered>::default();
        *self.0.hung.borrow_mut() = Some(unanswered.clone());
        HungOwner(unanswered)
    }

    /// Takes PRIMARY with `text`.
    #[cfg(not(test))]
    fn put_on_primary(&self, text: &str) -> bool {
        self.with(|clipboard| machine_primary::write(clipboard, text))
            .is_some()
    }

    /// The same, in memory.
    #[cfg(test)]
    fn put_on_primary(&self, text: &str) -> bool {
        self.select_elsewhere(text);
        true
    }

    /// The read [`Self::read_primary`] hands out.
    #[cfg(not(test))]
    fn primary_reader(&self) -> Option<impl FnOnce() -> Option<String> + Send + 'static> {
        // Opened here first, and not only to learn whether there is a
        // clipboard at all. The read opens a handle of its own on the pool,
        // and on X11 every handle in a process is one shared connection that
        // the last one to go tears down — so a read made while this one was
        // still unopened would bring the connection up and take it down again
        // for every middle click.
        self.with(|_| Ok(()))?;
        Some(machine_primary::read)
    }

    /// The same, from memory: what PRIMARY held at the press, as a read that
    /// answered at once would have found — or, from an owner that has stopped
    /// answering, once it answers.
    #[cfg(test)]
    fn primary_reader(&self) -> Option<impl FnOnce() -> Option<String> + Send + 'static> {
        let held = self.primary();
        let hung = self.0.hung.borrow().clone();
        Some(move || {
            if let Some(hung) = hung {
                hung.wait();
            }
            held
        })
    }

    /// Runs `use_clipboard` against the open clipboard, opening it if this is
    /// the first time and reporting `None` if it cannot be opened.
    fn with<T>(
        &self,
        use_clipboard: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
    ) -> Option<T> {
        let mut state = self.0.state.borrow_mut();
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

/// A program holding PRIMARY in a test build that has stopped answering, from
/// [`Clipboard::select_and_hang`]: the reads put to it wait until this
/// answers, or is dropped.
///
/// Dropped answers too, so that a test failing before it answers does not
/// leave a worker of the pool waiting forever — which would hold up the
/// harness's shutdown, since the pool joins its workers.
#[cfg(test)]
pub struct HungOwner(std::sync::Arc<Unanswered>);

#[cfg(test)]
impl HungOwner {
    /// Answers every read put to it so far, and any put to it from now on.
    pub fn answer(self) {
        drop(self);
    }
}

#[cfg(test)]
impl Drop for HungOwner {
    fn drop(&mut self) {
        let mut answered = self
            .0
            .answered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *answered = true;
        self.0.woken.notify_all();
    }
}

/// Whether a [`HungOwner`] has answered yet, and where the reads put to it
/// wait for it to.
#[cfg(test)]
#[derive(Default)]
struct Unanswered {
    answered: std::sync::Mutex<bool>,
    woken: std::sync::Condvar,
}

#[cfg(test)]
impl Unanswered {
    /// Blocks until the owner answers.
    fn wait(&self) {
        let answered = self
            .answered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _answered = self
            .woken
            .wait_while(answered, |answered| !*answered)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

/// The machine's PRIMARY, where the desktop keeps one.
///
/// Compiled into a test build too, for the one test that is run by hand
/// against a real desktop; nothing else in a test build calls it.
#[cfg(all(unix, not(target_os = "macos")))]
mod machine_primary {
    use arboard::{GetExtLinux, LinuxClipboardKind, SetExtLinux};

    /// Takes PRIMARY with `text`. Returns at once: the selection is served
    /// to whoever asks by `arboard`'s own thread, which is what X11 means by
    /// owning it.
    pub(super) fn write(
        clipboard: &mut arboard::Clipboard,
        text: &str,
    ) -> Result<(), arboard::Error> {
        clipboard
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text.to_owned())
    }

    /// Asks whoever holds PRIMARY for it as text. Blocks until they answer,
    /// or, when they never do, for four seconds for each text format asked
    /// for: about half a minute.
    pub(super) fn read() -> Option<String> {
        let read = arboard::Clipboard::new().and_then(|mut clipboard| {
            clipboard
                .get()
                .clipboard(LinuxClipboardKind::Primary)
                .text()
        });
        match read {
            Ok(text) => Some(text),
            // Nobody holds it, or what they hold is not text: a middle click
            // with nothing to paste, which pastes nothing.
            Err(error) => {
                log::debug!("PRIMARY could not be read: {error}");
                None
            }
        }
    }
}

/// The same, where there is none — never reached, because [`HAS_PRIMARY`]
/// is `false` here and [`primary_in_use`] with it, but the calls above it are
/// compiled on every platform.
#[cfg(not(any(test, all(unix, not(target_os = "macos")))))]
mod machine_primary {
    /// Takes nothing.
    pub(super) fn write(_: &mut arboard::Clipboard, _: &str) -> Result<(), arboard::Error> {
        Err(arboard::Error::ClipboardNotSupported)
    }

    /// Reads nothing.
    pub(super) fn read() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_is_offered_as_primary_on_linux_and_nowhere_else() {
        let clipboard = Clipboard::new();
        assert_eq!(clipboard.write_primary("zq7xv"), HAS_PRIMARY);
        assert_eq!(
            clipboard.primary().as_deref(),
            HAS_PRIMARY.then_some("zq7xv")
        );
    }

    /// The desktop's own PRIMARY, which the rest of the suite never touches:
    /// written as the end of a drag writes it, read back as a middle click
    /// reads it, and the clipboard proper not written at all — the one thing
    /// here that would catch the wrong selection named to `arboard`.
    ///
    /// Ignored, because it takes the selection of whoever runs it. Run it by
    /// hand on a Linux desktop, X11 or Wayland through XWayland.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    #[ignore = "takes the desktop's PRIMARY: run it by hand on a Linux desktop"]
    fn the_desktop_s_primary_is_written_and_read_and_the_clipboard_is_not() {
        let marker = format!("crook primary {}", std::process::id());
        let mut clipboard = arboard::Clipboard::new().expect("this desktop has no clipboard");
        machine_primary::write(&mut clipboard, &marker).expect("PRIMARY could not be taken");

        // On another thread with a handle of its own, as the pool reads it.
        let read = std::thread::spawn(machine_primary::read)
            .join()
            .expect("the read does not panic");
        assert_eq!(
            read.as_deref(),
            Some(marker.as_str()),
            "PRIMARY does not hold what was written to it"
        );
        assert_ne!(
            clipboard.get_text().ok().as_deref(),
            Some(marker.as_str()),
            "the clipboard proper was written"
        );
    }

    #[test]
    fn an_empty_selection_takes_nothing_from_whoever_holds_primary() {
        let clipboard = Clipboard::new();
        clipboard.select_elsewhere("held by another program");
        assert!(!clipboard.write_primary(""));
        assert_eq!(
            clipboard.primary().as_deref(),
            Some("held by another program")
        );
    }

    #[test]
    fn with_the_setting_off_primary_is_neither_offered_nor_read() {
        let clipboard = Clipboard::new();
        clipboard.select_elsewhere("held by another program");

        use_primary(false);
        assert!(!clipboard.write_primary("zq7xv"));
        assert!(clipboard.read_primary().is_none());
        assert_eq!(
            clipboard.primary().as_deref(),
            Some("held by another program")
        );

        use_primary(true);
        let read = clipboard.read_primary();
        assert_eq!(read.is_some(), HAS_PRIMARY);
        assert_eq!(
            read.and_then(|read| read()).as_deref(),
            HAS_PRIMARY.then_some("held by another program")
        );
    }

    #[test]
    fn a_read_of_primary_can_be_run_on_another_thread() {
        // The whole point of handing it out: the pool is where it runs.
        let clipboard = Clipboard::new();
        clipboard.select_elsewhere("zq7xv");
        let answer = clipboard.read_primary().map(|read| {
            std::thread::spawn(read)
                .join()
                .expect("the read does not panic")
        });
        assert_eq!(
            answer.as_ref().map(Option::as_deref),
            HAS_PRIMARY.then_some(Some("zq7xv"))
        );
    }
}
