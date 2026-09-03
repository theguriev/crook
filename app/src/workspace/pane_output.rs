//! The half of a pane's output that is the same whether it is drawn as a grid
//! or as a list of blocks: which keys it takes, where a copy goes, and the
//! gesture that selects text out of it.
//!
//! Two elements draw a pane's output — [`TerminalElement`](super::TerminalElement)
//! for a full-screen program and [`BlockList`](super::BlockList) for
//! everything else — and both have to answer the same questions about the
//! keyboard. Answering them twice is how a pane ends up interrupting a command
//! on one surface and copying the clipboard on the other, so both hold one of
//! these and the answers live here.
//!
//! **Nothing here is state that outlives a frame.** The gesture is a
//! [`PaneSelection`], which the workspace keeps; the selection itself is the
//! emulator's, because that is the only place it stays anchored to its text
//! while the shell prints underneath it. What this owns is the routing.

use crook_terminal::{CellSide, SelectionKind, ViewportPoint};
use crookui_core::event::Event;
use crookui_core::presenter::EventContext;

use crate::clipboard::Clipboard;
use crate::input_keys::{self, Platform, Route};
use crate::pane_input::PaneInput;
use crate::pane_selection::PaneSelection;
use crate::tab::PaneId;
use crate::terminal_keys;
use crate::terminal_model::TerminalHandle;

use super::action::WorkspaceAction;

/// What a pane's output does with the keys that reach the window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Keys {
    /// Nothing: some other pane is the focused one.
    None,
    /// Only the keys that interrupt, end and suspend a command.
    ///
    /// What a focused pane still takes while a modal menu is open over the
    /// window. The menu freezes everything under it, and a `sleep 30` that
    /// could not be interrupted until somebody found the mouse would be the
    /// menu taking away the one key a terminal must never lose.
    Signals,
    /// Everything the routing rule gives the shell.
    All,
}

/// What became of a keystroke offered to a pane's output.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    /// Not this element's, or a key with no encoding. The caller carries on
    /// looking for somebody to take it.
    Ignored,
    /// Taken, and nothing was sent to the shell — a copy, or a key the field
    /// below is about to route for itself.
    Handled,
    /// Taken, and bytes went to the pty. What returns a scrolled-back surface
    /// to the live output.
    SentToPty,
}

/// What the output needs to be selectable: which pane this is, the gesture the
/// workspace keeps for it, and somewhere for a copy to go.
pub struct Mouse {
    pane: PaneId,
    gesture: PaneSelection,
    clipboard: Clipboard,
}

/// The keyboard and the selection of one pane's output.
pub struct Output {
    /// The terminal behind the output, when there is a live one.
    ///
    /// `None` draws a snapshot and nothing else — no typing, no selection —
    /// which is what a test does, and what a pane whose shell has gone would
    /// do.
    handle: Option<TerminalHandle>,

    /// How much of the keyboard this pane's output takes.
    ///
    /// Decided by the workspace rather than here, because it is the workspace
    /// that knows which pane is focused and whether a menu is up over it. See
    /// [`crate::terminal_keys`] for the other half of the same line.
    keys: Keys,

    /// The line being composed under this output, when there is a composer.
    ///
    /// Read at the moment a key arrives rather than baked in when the frame
    /// was built, because it decides what Ctrl-D means: an end of input on an
    /// empty line, and a delete over a written one.
    input: Option<PaneInput>,

    /// The pointer gesture that selects text out of this output, and where a
    /// copy of it goes.
    mouse: Option<Mouse>,
}

impl Output {
    /// An output that is driven by nothing: no shell to type into, no gesture
    /// to select with.
    pub fn detached() -> Self {
        Self {
            handle: None,
            keys: Keys::None,
            input: None,
            mouse: None,
        }
    }

    /// Attaches the terminal the output came from, and says how much of the
    /// keyboard it takes.
    pub fn with_terminal(mut self, handle: TerminalHandle, keys: Keys) -> Self {
        self.handle = Some(handle);
        self.keys = keys;
        self
    }

    /// Attaches the composer under this output, whose line decides what Ctrl-D
    /// means.
    pub fn with_input(mut self, input: PaneInput) -> Self {
        self.input = Some(input);
        self
    }

    /// Makes the output selectable: `gesture` is the press this pane has open,
    /// which outlives the frame, `clipboard` is where a copy goes, and `pane`
    /// is who the release is dispatched for.
    pub fn with_selection(
        mut self,
        pane: PaneId,
        gesture: PaneSelection,
        clipboard: Clipboard,
    ) -> Self {
        self.mouse = Some(Mouse {
            pane,
            gesture,
            clipboard,
        });
        self
    }

    /// The terminal behind the output, when there is one.
    pub fn handle(&self) -> Option<&TerminalHandle> {
        self.handle.as_ref()
    }

    /// How much of the keyboard this output takes.
    pub fn keys(&self) -> Keys {
        self.keys
    }

    /// Whether anything can be selected out of this output at all.
    pub fn is_selectable(&self) -> bool {
        self.mouse.is_some()
    }

    /// The typed keystroke, if this pane is the one that should have it and
    /// the shell is the half of the pane it belongs to.
    ///
    /// The whole policy is [`input_keys::route`]: a selection in the output
    /// owns the copy chord, on the alt screen the program has every key, on
    /// the normal screen the shell has only the ones that interrupt, end and
    /// suspend, and the composer below has the rest.
    ///
    /// A modal menu suspends the selection's claim rather than the menu's own
    /// filter below: while one is up, the three keys a running command has to
    /// keep hearing mean what they always mean, and `ctrl-c` cannot be spent
    /// on the clipboard by a selection nobody can see the pointer on any more.
    /// An output with nowhere to copy *to* reports the same, so the interrupt
    /// is never taken by a chord that could not have answered it.
    pub fn type_key(&self, event: &Event, alt_screen: bool, ctx: &mut EventContext) -> Typed {
        let Event::KeyDown { keystroke, chars } = event else {
            return Typed::Ignored;
        };
        if self.keys == Keys::None {
            return Typed::Ignored;
        }
        let Some(handle) = self.handle.as_ref() else {
            return Typed::Ignored;
        };

        let modal = self.keys == Keys::Signals;
        let can_copy = !modal && self.mouse.is_some();
        let pane = input_keys::Pane {
            alt_screen,
            line_is_empty: self
                .input
                .as_ref()
                .is_none_or(|input| input.editor().is_empty()),
            grid_has_selection: can_copy && handle.has_selection(),
        };
        let route = input_keys::route(keystroke, chars, pane, Platform::current());

        if route == Route::CopyOutput {
            return if self.copy_selection(handle, ctx) {
                Typed::Handled
            } else {
                Typed::Ignored
            };
        }
        // **Typing releases the selection**, whichever half of the pane the
        // key belongs to: a line going into the composer under a highlight
        // nobody is aiming at any more is the same stale highlight as one left
        // over a screen that has scrolled. A key the keymap has no meaning for
        // changes nothing and so releases nothing.
        if route != Route::Ignored && pane.grid_has_selection {
            self.release_selection(ctx);
        }

        if !route.reaches_the_shell() {
            return Typed::Ignored;
        }
        // A modal menu takes the rest away: everything but the three keys a
        // running command has to keep hearing.
        if modal && !input_keys::is_signal(keystroke) {
            return Typed::Ignored;
        }

        let Some((key, modifiers)) = terminal_keys::key_for(keystroke, chars) else {
            return Typed::Ignored;
        };
        if handle.send_key(key, modifiers) {
            Typed::SentToPty
        } else {
            Typed::Ignored
        }
    }

    /// Puts what is selected in the output on the clipboard and lets go of it.
    ///
    /// Letting go is not tidiness. Off macOS this chord is also SIGINT, and
    /// the selection is the only thing standing between a person and an
    /// interrupt key that no longer interrupts — so the very next press of it
    /// does. It is also the only sign a copy happened at all: the highlight
    /// goes away.
    ///
    /// It goes away even on the copy that could not be made — a machine with
    /// no clipboard at all, or one whose clipboard another process was
    /// holding. **That is deliberate, and it is the lesser of two bad
    /// answers.** Keeping the selection would be more truthful about the copy,
    /// and it would also hand the same selection the next press of this chord,
    /// and the one after it: on a machine where the clipboard never opens,
    /// that is a terminal whose interrupt never works again. A copy that did
    /// not happen costs a highlight and a line in the log; one that disarmed
    /// the interrupt would cost the pane.
    fn copy_selection(&self, handle: &TerminalHandle, ctx: &mut EventContext) -> bool {
        let Some(mouse) = self.mouse.as_ref() else {
            return false;
        };
        if !handle
            .selection_text()
            .is_some_and(|copied| mouse.clipboard.write(&copied))
        {
            log::warn!("the selection could not be put on the clipboard; letting go of it anyway");
        }
        self.release_selection(ctx);
        true
    }

    /// Puts `text` on the clipboard, reporting whether it got there.
    ///
    /// What a block's copy control hands over. Nothing is released: a copy
    /// aimed at one block is not a copy of whatever the pointer had selected.
    pub fn copy(&self, text: &str) -> bool {
        let Some(mouse) = self.mouse.as_ref() else {
            return false;
        };
        if mouse.clipboard.write(text) {
            return true;
        }
        log::warn!("a block could not be put on the clipboard");
        false
    }

    /// Asks for the selection to be let go of, once this keystroke is done
    /// with.
    ///
    /// Dispatched rather than done here, and [`WorkspaceAction::ReleaseSelection`]
    /// carries the reason: the composer under this output routes the same
    /// keystroke against the same question a moment later, and an output that
    /// had already changed the answer would have it route as though nothing
    /// were selected.
    pub fn release_selection(&self, ctx: &mut EventContext) {
        let Some(mouse) = self.mouse.as_ref() else {
            return;
        };
        ctx.dispatch_typed_action(WorkspaceAction::ReleaseSelection(mouse.pane));
    }

    /// Starts a selection at a cell, in the units the click count asks for: a
    /// character, a word, or a whole line.
    ///
    /// Alt makes it a block, which is how a column is taken out of aligned
    /// output — `ls -l`, a table, a diff — without the rest of every line
    /// coming with it.
    pub fn press(
        &self,
        at: ViewportPoint,
        side: CellSide,
        kind: SelectionKind,
        ctx: &mut EventContext,
    ) -> bool {
        let (Some(mouse), Some(handle)) = (self.mouse.as_ref(), self.handle.as_ref()) else {
            return false;
        };
        handle.start_selection(kind, at, side);
        mouse.gesture.begin();
        ctx.notify();
        true
    }

    /// Drags the open end of the selection to a cell, scrolling the emulator's
    /// own viewport by `scroll` lines first.
    pub fn drag(
        &self,
        at: ViewportPoint,
        side: CellSide,
        scroll: i32,
        ctx: &mut EventContext,
    ) -> bool {
        let (Some(mouse), Some(handle)) = (self.mouse.as_ref(), self.handle.as_ref()) else {
            return false;
        };
        if !mouse.gesture.is_dragging() {
            return false;
        }
        handle.drag_selection(at, side, scroll);
        ctx.notify();
        true
    }

    /// Whether a press on this pane's output has not been released yet.
    pub fn is_dragging(&self) -> bool {
        self.mouse
            .as_ref()
            .is_some_and(|mouse| mouse.gesture.is_dragging())
    }

    /// Ends the gesture, reporting whether this pane had one.
    pub fn release(&self) -> bool {
        self.mouse.as_ref().is_some_and(|mouse| mouse.gesture.end())
    }
}

/// Which selection a click count and the Alt key ask for.
///
/// A press that is never dragged anywhere leaves an *empty* simple selection,
/// which is no selection at all — so a plain click on the output is also how
/// the last one is let go of.
pub fn selection_kind(click_count: u32, alt: bool) -> SelectionKind {
    match (click_count, alt) {
        (1, true) => SelectionKind::Block,
        (1, _) => SelectionKind::Simple,
        (2, _) => SelectionKind::Semantic,
        _ => SelectionKind::Lines,
    }
}
