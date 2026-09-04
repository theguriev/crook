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

use crook_terminal::{Modifiers, MouseButton, MouseEventKind, MouseModes, Rows, SelectionKind};
use crookui_core::event::Event;
use crookui_core::presenter::EventContext;

use crate::clipboard::Clipboard;
use crate::input_keys::{self, Platform, Route};
use crate::pane_selection::PaneSelection;
use crate::selection::{Anchor, Blocks, Cells, Selection};
use crate::tab::PaneId;
use crate::terminal_keys;
use crate::terminal_model::TerminalHandle;
use crate::text_input::TextInput;

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

/// What the output needs to be selectable: which pane this is, the selection
/// the workspace keeps for it, where its cells are, and somewhere for a copy
/// to go.
pub struct Mouse {
    pane: PaneId,
    gesture: PaneSelection,
    clipboard: Clipboard,
    cells: Cells,
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
    input: Option<TextInput>,

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
    pub fn with_input(mut self, input: TextInput) -> Self {
        self.input = Some(input);
        self
    }

    /// Makes the output selectable: `gesture` is this pane's selection, which
    /// outlives the frame, `cells` is where its text is read back from,
    /// `clipboard` is where a copy goes, and `pane` is who the release is
    /// dispatched for.
    pub fn with_selection(
        mut self,
        pane: PaneId,
        gesture: PaneSelection,
        clipboard: Clipboard,
        cells: Cells,
    ) -> Self {
        self.mouse = Some(Mouse {
            pane,
            gesture,
            clipboard,
            cells,
        });
        self
    }

    /// This pane's selection, when the output is selectable at all.
    pub fn pane_selection(&self) -> Option<&PaneSelection> {
        self.mouse.as_ref().map(|mouse| &mouse.gesture)
    }

    /// Lets go of a selection the frame being laid out cannot honour.
    ///
    /// Two things change what an anchor means and neither of them is
    /// something the anchor can be moved through. A resize that changes the
    /// column count re-wraps every row, so the cells it named hold other text;
    /// a change of surface renumbers the rows outright, because a block counts
    /// its own from zero and a grid counts from the oldest line of the
    /// scrollback. Both are known here, in layout, and nowhere earlier.
    pub fn laid_out(&self, columns: usize) {
        let Some(mouse) = self.mouse.as_ref() else {
            return;
        };
        mouse.gesture.resurfaced(mouse.cells);
        mouse.gesture.reflowed(columns);
    }

    /// The terminal behind the output, when there is one.
    pub fn handle(&self) -> Option<&TerminalHandle> {
        self.handle.as_ref()
    }

    /// How much of the keyboard this output takes.
    pub fn keys(&self) -> Keys {
        self.keys
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
            grid_has_selection: can_copy && self.has_selection(),
        };
        let route = input_keys::route(keystroke, chars, pane, Platform::current());

        if route == Route::CopyOutput {
            return if self.copy_selection(ctx) {
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
    fn copy_selection(&self, ctx: &mut EventContext) -> bool {
        let Some(mouse) = self.mouse.as_ref() else {
            return false;
        };
        if !self
            .selected_text()
            .is_some_and(|copied| mouse.clipboard.write(&copied))
        {
            log::warn!("the selection could not be put on the clipboard; letting go of it anyway");
        }
        self.release_selection(ctx);
        true
    }

    /// Whether anything is selected in this pane's output, on the surface this
    /// element is drawing.
    ///
    /// A field read rather than a walk of the blocks: the selection is only
    /// ever stored once it covers cells, which is what lets the one keystroke
    /// that must never be wrong about this — the one that stops a running
    /// command — be settled by a `bool`. Scoped to the surface for the same
    /// reason it is a `bool` at all: a selection made on the list while the
    /// pane has since fallen back to the grid is a highlight that is not on
    /// screen, and an interrupt spent copying one of those is an interrupt
    /// nobody asked for.
    pub fn has_selection(&self) -> bool {
        self.mouse
            .as_ref()
            .is_some_and(|mouse| mouse.gesture.has_selection_in(mouse.cells))
    }

    /// What a copy would take, or `None` when nothing is selected.
    ///
    /// The blocks are asked for at the moment of the copy rather than kept
    /// from the frame that built this: a command may have finished since, and
    /// a block that has been harvested holds exactly the rows it did while it
    /// was open.
    pub fn selected_text(&self) -> Option<String> {
        let mouse = self.mouse.as_ref()?;
        let handle = self.handle.as_ref()?;
        let selection = mouse.gesture.selection_in(mouse.cells)?;
        let snapshot = handle.snapshot();

        match mouse.cells {
            Cells::List => {
                let blocks = handle.blocks();
                selection.text(&Blocks::list(&blocks, &snapshot))
            }
            // The rows a drag covered may have scrolled out of the viewport,
            // and a snapshot only ever holds the viewport — so they come back
            // out of the emulator. A screenful of slack at each end covers the
            // rows a double or triple click grows onto: a line folded over
            // more than a whole screen, triple-clicked, copies the screenful
            // around the click rather than all of it, which is a limit of this
            // surface and not of the list.
            Cells::Grid => {
                let slack = snapshot.rows;
                let (first, last) = (
                    selection
                        .anchor
                        .row
                        .min(selection.head.row)
                        .saturating_sub(slack),
                    selection.anchor.row.max(selection.head.row) + slack,
                );
                let (rows, at) = handle.harvest_rows(first, last);
                selection.text(&Blocks::one(
                    selection.anchor.block,
                    Rows::Stored(&rows),
                    at,
                ))
            }
        }
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
    ///
    /// `covers` is whether that already selects cells, which a double or
    /// triple click does and a single click never does. Only the caller can
    /// answer it: the blocks are its, not this.
    pub fn press(
        &self,
        kind: SelectionKind,
        at: Anchor,
        covers: bool,
        columns: usize,
        ctx: &mut EventContext,
    ) -> bool {
        let Some(mouse) = self.mouse.as_ref() else {
            return false;
        };
        mouse.gesture.press(kind, at, covers, columns, mouse.cells);
        ctx.notify();
        true
    }

    /// Drags the open end of the selection to a cell.
    pub fn drag(&self, at: Anchor, covers: bool, ctx: &mut EventContext) -> bool {
        let Some(mouse) = self.mouse.as_ref() else {
            return false;
        };
        if !mouse.gesture.is_dragging() {
            return false;
        }
        if mouse.gesture.drag(at, covers) {
            ctx.notify();
        }
        true
    }

    /// The press this pane has open, and what it is taking at a time.
    pub fn pressed(&self) -> Option<(SelectionKind, Anchor)> {
        self.mouse
            .as_ref()
            .and_then(|mouse| mouse.gesture.pressed())
    }

    /// What is selected in this pane's output, in the space this element
    /// draws.
    ///
    /// `None` for a selection taken on the other surface: its rows are
    /// numbered against a picture this element is not the one drawing, and the
    /// pane is about to let go of it. See
    /// [`PaneSelection::resurfaced`](crate::pane_selection::PaneSelection::resurfaced).
    pub fn selection(&self) -> Option<Selection> {
        self.mouse
            .as_ref()
            .and_then(|mouse| mouse.gesture.selection_in(mouse.cells))
    }

    /// Whether a press on this pane's output has not been released yet.
    pub fn is_dragging(&self) -> bool {
        self.mouse
            .as_ref()
            .is_some_and(|mouse| mouse.gesture.is_dragging())
    }

    /// Ends the gesture, reporting whether this pane had one.
    pub fn release(&self) -> bool {
        self.mouse
            .as_ref()
            .is_some_and(|mouse| mouse.gesture.release())
    }

    /// Which mouse reports the program in this pane has asked for.
    ///
    /// [`MouseModes::NONE`] whenever there is no terminal, which is what makes
    /// a detached output — a test, a pane whose shell has gone — behave as one
    /// nobody is reading the pointer in.
    pub fn mouse_modes(&self) -> MouseModes {
        self.handle
            .as_ref()
            .map_or(MouseModes::NONE, TerminalHandle::mouse_modes)
    }

    /// Hands a pointer gesture to the program, reporting whether it took it.
    ///
    /// `false` for every gesture the modes in force do not cover, which is what
    /// leaves a press to start a selection instead. A gesture reported to a
    /// program repaints nothing by itself: what the program does about it
    /// arrives as output, and that is what draws the next frame.
    pub fn report_mouse(
        &self,
        kind: MouseEventKind,
        button: Option<MouseButton>,
        row: usize,
        column: usize,
        modifiers: Modifiers,
    ) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| handle.send_mouse(kind, button, row, column, modifiers))
    }

    /// Says a press was given to a program, so the moves that follow are
    /// reports rather than a selection being dragged out.
    pub fn begin_reporting(&self) -> bool {
        self.mouse
            .as_ref()
            .is_some_and(|mouse| mouse.gesture.begin_reporting())
    }

    /// Whether the button that is down belongs to a program reading the mouse.
    pub fn is_reporting(&self) -> bool {
        self.mouse
            .as_ref()
            .is_some_and(|mouse| mouse.gesture.is_reporting())
    }

    /// Ends a reported press, reporting whether there was one.
    pub fn end_reporting(&self) -> bool {
        self.mouse
            .as_ref()
            .is_some_and(|mouse| mouse.gesture.end_reporting())
    }

    /// Sends the wheel to a full-screen program as arrow keys, for one that
    /// asked for `?1007` and never asked for the mouse.
    pub fn alternate_scroll(&self, lines: i32) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|handle| handle.send_alternate_scroll(lines))
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
