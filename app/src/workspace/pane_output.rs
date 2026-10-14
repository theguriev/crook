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
//! while the shell prints underneath it. A word an input method is composing
//! for the program is a [`PaneComposition`], which the workspace keeps too.
//! What this owns is the routing.

use std::rc::Rc;

use crook_terminal::{
    BlockId, Key, Modifiers, MouseButton, MouseEventKind, MouseModes, Rows, SelectionKind,
};
use crookui_core::event::{Event, Ime, Keystroke};
use crookui_core::presenter::EventContext;

use crate::clipboard::{self, Clipboard};
use crate::input_keys::{self, Platform, Route};
use crate::pane_composition::PaneComposition;
use crate::pane_selection::PaneSelection;
use crate::selection::{Anchor, Blocks, Cells, Filter, Selection};
use crate::settings::OptionAsMeta;
use crate::tab::PaneId;
use crate::terminal_keys::{self, Alt};
use crate::terminal_model::TerminalHandle;
use crate::text_input::TextInput;

use super::action::WorkspaceAction;

/// What a pane's output does with the keys that reach the window.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Keys {
    /// Nothing: another pane is the focused one, or the question a close asks
    /// is up (see [`closing`](super::closing)).
    None,
    /// Only the keys that interrupt, end and suspend a command.
    ///
    /// What a focused pane still takes while a modal menu is open over the
    /// window. The menu freezes everything under it, and a `sleep 30` that
    /// could not be interrupted until somebody found the mouse would be the
    /// menu taking away the one key a terminal must never lose. Except under
    /// the question a close asks, which has an Escape and is asking whether
    /// to end the program these keys would reach: there the pane gets
    /// [`Keys::None`].
    Signals,
    /// The keys of [`Keys::Signals`], and the copy chord over a selection in
    /// the output.
    ///
    /// What the focused pane's output keeps while that pane's find bar has
    /// the keyboard. The field takes every key it has a use for before the
    /// output hears it, and the rest used to fall through to the program: a
    /// word an input method composed, a chord the field means nothing by, a
    /// paste chord it does not paste on (`ctrl-shift-v` on macOS) — every one
    /// of them, over an agent or an `ssh` session with no composer to catch
    /// them. Unlike under a menu, the bar hides nothing of the output, so a
    /// selection made in it is still in view, and the copy chord over one —
    /// the platform's, or `ctrl-c` — still copies it rather than interrupting
    /// the program. The field leaves the platform's chord to the output
    /// whenever it has nothing selected of its own to copy.
    SignalsAndCopy,
    /// Everything the routing rule gives the shell.
    All,
}

impl Keys {
    /// Whether a keystroke the routing rule gives the program — typed, or a
    /// paste — is handed on to it with this much of the keyboard.
    ///
    /// Everything, with the whole of it; with less, only the three keys a
    /// running command must keep hearing. A paste is not one of them, and is
    /// asked here like any other key: under a menu, or under a find bar whose
    /// field had no use for the chord, the clipboard going into the program
    /// would be the one key of all of them that reached it.
    fn hands_on(self, keystroke: &Keystroke) -> bool {
        match self {
            Keys::All => true,
            Keys::Signals | Keys::SignalsAndCopy => input_keys::is_signal(keystroke),
            Keys::None => false,
        }
    }
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

    /// Whether the window this output is in has the desktop's keyboard.
    ///
    /// The cursor's business and not the routing's: a window behind another
    /// application is sent no keys to route, and the one that comes back to
    /// the front types where it left off with `keys` exactly as they were.
    /// See [`Self::draws_focus`].
    window_focused: bool,

    /// The line being composed under this output, when there is a composer.
    ///
    /// **`None` is a fact about the keyboard, not only about Ctrl-D.** A pane
    /// with no field is a pane whose keys are the program's — see rule 2 of
    /// [`input_keys::route`] — so this is attached exactly when
    /// [`crate::pane_surface`] said a composer is drawn, and never on an alt
    /// screen, an overflowing block or a command that has been running long
    /// enough to take the field away.
    ///
    /// The line itself is read at the moment a key arrives rather than baked
    /// in when the frame was built, because it decides what Ctrl-D means: an
    /// end of input on an empty line, and a delete over a written one.
    input: Option<TextInput>,

    /// What an input method is composing into the program, which the output
    /// draws at the program's cursor. See [`Self::composition`].
    composition: Option<PaneComposition>,

    /// The pointer gesture that selects text out of this output.
    mouse: Option<Mouse>,

    /// The system clipboard, when this output is attached to one.
    ///
    /// Not the mouse's, though a copy is the gesture that made one necessary:
    /// pasting into a full-screen program is done with no pointer anywhere
    /// near it, and a clipboard reachable only through a selection would make
    /// the paste chord depend on whether anything had ever been selected.
    clipboard: Option<Clipboard>,

    /// What Option means to the program on a Mac. See [`OptionAsMeta`].
    option_as_meta: OptionAsMeta,

    /// The block the list shows filtered, and the rows of it that it shows.
    /// See [`BlockList::with_filter`](super::BlockList::with_filter).
    ///
    /// Here because a copy is: a drag over the filtered block names the
    /// block's own rows at its two ends, and only this says which of the rows
    /// between them were on screen.
    filter: Option<(BlockId, Rc<Vec<usize>>)>,
}

impl Output {
    /// An output that is driven by nothing: no shell to type into, no gesture
    /// to select with.
    pub fn detached() -> Self {
        Self {
            handle: None,
            keys: Keys::None,
            // In front until told otherwise, as the workspace's own bit is: a
            // window that never hears about its focus draws as it always did.
            window_focused: true,
            input: None,
            composition: None,
            mouse: None,
            clipboard: None,
            option_as_meta: OptionAsMeta::default(),
            filter: None,
        }
    }

    /// Attaches the terminal the output came from, and says how much of the
    /// keyboard it takes.
    pub fn with_terminal(mut self, handle: TerminalHandle, keys: Keys) -> Self {
        self.handle = Some(handle);
        self.keys = keys;
        self
    }

    /// Says what Option means to the program on a Mac, which is the person's
    /// setting and not the output's to decide.
    pub fn with_option_as_meta(mut self, option_as_meta: OptionAsMeta) -> Self {
        self.option_as_meta = option_as_meta;
        self
    }

    /// Says whether the window has the desktop's keyboard. See
    /// [`Self::draws_focus`].
    pub fn with_window_focused(mut self, focused: bool) -> Self {
        self.window_focused = focused;
        self
    }

    /// Says which block the list shows filtered, and which of its rows it
    /// shows, so that a copy takes those rows and no others.
    pub fn with_filter(mut self, filter: Option<(BlockId, Rc<Vec<usize>>)>) -> Self {
        self.filter = filter;
        self
    }

    /// The block shown filtered and the rows of it that are shown, when the
    /// list is filtering one.
    pub fn filter(&self) -> Option<Filter<'_>> {
        self.filter.as_ref().map(|(block, kept)| Filter {
            block: *block,
            kept,
        })
    }

    /// Attaches the composer under this output: the field a key that is not
    /// the program's goes into, whose line decides what Ctrl-D means.
    ///
    /// Called only where one is drawn. See [`Output::input`].
    pub fn with_input(mut self, input: TextInput) -> Self {
        self.input = Some(input);
        self
    }

    /// Attaches the pane's input method composition, which the output keeps
    /// and draws while the program has the keyboard.
    pub fn with_composition(mut self, composition: PaneComposition) -> Self {
        self.composition = Some(composition);
        self
    }

    /// The pane's composition, when this output is where an input method
    /// composes: it has every key and no composer is drawn under it.
    ///
    /// Asked of the composition as well as of the frame, because the element
    /// an input method's event reaches was built before whatever last moved
    /// the keyboard. See [`PaneComposition::is_listening`].
    pub fn composition(&self) -> Option<&PaneComposition> {
        if self.keys != Keys::All || self.input.is_some() {
            return None;
        }
        self.composition
            .as_ref()
            .filter(|composition| composition.is_listening())
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
            cells,
        });
        self.clipboard = Some(clipboard);
        self
    }

    /// Which pane this output belongs to, when it belongs to one.
    ///
    /// What an element dispatching an action about this pane needs: a control
    /// on a block names the pane its list is drawing, and the workspace is
    /// where the answer to it lives.
    pub fn pane(&self) -> Option<PaneId> {
        self.mouse.as_ref().map(|mouse| mouse.pane)
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

    /// Whether the program's cursor here is drawn as the one typing goes to —
    /// in the shape the program asked for — rather than as the outline of a
    /// terminal without the keyboard.
    ///
    /// Every key, and a window in front. A pane with the keyboard in a window
    /// behind another application is one nothing typed reaches either, and
    /// its cursor drawn filled made every Crook window on a desktop look as
    /// though it were the one being typed into.
    pub fn draws_focus(&self) -> bool {
        self.keys == Keys::All && self.window_focused
    }

    /// The typed keystroke, if this pane is the one that should have it and
    /// the shell is the half of the pane it belongs to.
    ///
    /// The whole policy is [`input_keys::route`]: a selection in the output
    /// owns the copy chord, a pane with no composer gives the program every
    /// key, and under a composer the shell has only the ones that interrupt,
    /// end and suspend while the field has the rest.
    ///
    /// A modal menu suspends the selection's claim rather than the menu's own
    /// filter below: while one is up, the three keys a running command has to
    /// keep hearing mean what they always mean, and `ctrl-c` cannot be spent
    /// on the clipboard by a selection nobody can see the pointer on any more.
    /// An output with nowhere to copy *to* reports the same, so the interrupt
    /// is never taken by a chord that could not have answered it.
    pub fn type_key(&self, event: &Event, ctx: &mut EventContext) -> Typed {
        let Event::KeyDown {
            keystroke,
            chars,
            origin,
        } = event
        else {
            return match event {
                Event::Ime(ime) => self.compose(ime),
                _ => Typed::Ignored,
            };
        };
        if self.keys == Keys::None {
            return Typed::Ignored;
        }
        let Some(handle) = self.handle.as_ref() else {
            return Typed::Ignored;
        };

        // Everything but the whole keyboard leaves the program the signals
        // alone — see [`Keys::hands_on`]; only a menu takes the selection's
        // claim on `ctrl-c` as well.
        let can_copy = self.keys != Keys::Signals && self.mouse.is_some();
        let pane = input_keys::Pane {
            // Which screen this is was answered when the frame was built, by
            // the one function that answers it: a composer was attached, or
            // there is none to attach.
            composer: self.input.is_some(),
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
        // The program owns the keyboard, so this is the only way the clipboard
        // reaches it — with the whole of the keyboard, and only then. Under a
        // find bar this is a chord the field had no use for, `ctrl-shift-v` on
        // macOS, meant for the query and not for the agent under it.
        if route == Route::PasteToShell {
            if !self.keys.hands_on(keystroke) {
                return Typed::Ignored;
            }
            let text = self
                .clipboard
                .as_ref()
                .and_then(Clipboard::read)
                .filter(|text| !text.is_empty());
            if let Some(text) = text {
                return if handle.paste(&text) {
                    Typed::SentToPty
                } else {
                    log::debug!("nothing was pasted into the shell");
                    Typed::Handled
                };
            }
            // No text: a screenshot, most likely, and a terminal has no way to
            // send a program an image. Claude Code and Codex read the clipboard
            // themselves on Ctrl-V, so the chord a person pastes with becomes
            // the one those programs paste on, and the image arrives as it
            // would have from Ctrl-V — instead of Cmd-V doing nothing and
            // Ctrl-V being a second paste key to remember.
            let control = Modifiers {
                control: true,
                ..Modifiers::default()
            };
            return if handle.send_key(Key::Char('v'), control) {
                Typed::SentToPty
            } else {
                Typed::Handled
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
        // A modal menu, or a find bar, takes the rest away: everything but the
        // three keys a running command has to keep hearing.
        if !self.keys.hands_on(keystroke) {
            return Typed::Ignored;
        }

        let alt = Alt::on(Platform::current(), self.option_as_meta);
        let Some((key, modifiers)) = terminal_keys::key_for(keystroke, chars, origin, alt) else {
            return Typed::Ignored;
        };
        if handle.send_key(key, modifiers) {
            Typed::SentToPty
        } else {
            Typed::Ignored
        }
    }

    /// Keeps what an input method is composing for the program that owns the
    /// keyboard, or sends it what it committed.
    ///
    /// A preedit is held and drawn, never written: it is not text yet, and a
    /// program handed every guess on the way to a word would have typed all of
    /// them. Handled rather than sent, so the frame is redrawn with it and
    /// nothing that follows typing — the list returning to its end — happens
    /// for a word that has not been typed.
    fn compose(&self, ime: &Ime) -> Typed {
        let composition = self.composition();
        let changed = match ime {
            Ime::Commit(text) => {
                // The composition is over whether or not its text reaches
                // anyone: what was drawn belonged to the word that has just
                // been committed, and left up it would be drawn twice once the
                // program echoes it.
                let cleared = composition.is_some_and(PaneComposition::clear);
                return match self.commit(text) {
                    Typed::Ignored if cleared => Typed::Handled,
                    typed => typed,
                };
            }
            // A composition starting means anything left over from the last
            // one is stale, whatever ended it.
            Ime::Enabled | Ime::Disabled => composition.is_some_and(PaneComposition::clear),
            Ime::Preedit { text, cursor } => {
                composition.is_some_and(|composition| composition.set(text, *cursor))
            }
        };
        if changed {
            Typed::Handled
        } else {
            Typed::Ignored
        }
    }

    /// Sends what an input method composed to the program that owns the
    /// keyboard.
    ///
    /// Text an IME commits — Japanese, Chinese or Korean, and on macOS a dead
    /// key's accent, `⌥e` then `e` — arrives as one event *instead of* the key
    /// presses that made it, and only the composer read that event. Under a
    /// composer that is right: the field has the line. With no composer — vim,
    /// `less`, Claude Code — nothing else read it, and what somebody composed
    /// reached no program at all. So it goes to the pty as typed text, the way
    /// every terminal hands a full-screen program a composed character. Only
    /// with the whole keyboard: a modal menu, or a find bar being typed into,
    /// leaves the program the signals and nothing else, and a composed word is
    /// not a signal.
    fn commit(&self, text: &str) -> Typed {
        if self.keys != Keys::All || self.input.is_some() || text.is_empty() {
            return Typed::Ignored;
        }
        match self.handle.as_ref() {
            Some(handle) if handle.write(text) => Typed::SentToPty,
            _ => Typed::Ignored,
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
    ///
    /// And on the copy there was nothing to make: a selection the find bar's
    /// filter shows none of, which [`Self::selected_text`] says is `None`. The
    /// clipboard keeps what it held, and the highlight nobody could see is let
    /// go of all the same.
    fn copy_selection(&self, ctx: &mut EventContext) -> bool {
        if self.mouse.is_none() {
            return false;
        }
        match self.selected_text() {
            // Not a failure, so not the warning below: that line is what
            // somebody tracing a clipboard that will not open goes looking
            // for, and a filter narrowed after a drag is not one.
            None => log::debug!("the selection shows nothing to copy; letting go of it"),
            Some(copied) => {
                if !self
                    .clipboard
                    .as_ref()
                    .is_some_and(|clipboard| clipboard.write(&copied))
                {
                    log::warn!(
                        "the selection could not be put on the clipboard; letting go of it anyway"
                    );
                }
            }
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

    /// What a copy would take, or `None` when there is nothing to take:
    /// nothing is selected, or the find bar's filter hides every row of what
    /// is — a drag the query was narrowed under, or one let go of in the rows
    /// hidden between two shown lines. See
    /// [`Region::text_with`](crate::selection::Region::text_with).
    ///
    /// The blocks are asked for at the moment of the copy rather than kept
    /// from the frame that built this: a command may have finished since, and
    /// a block that has been harvested holds exactly the rows it did while it
    /// was open. The filter is the frame's, because it is what that frame
    /// drew — and what the highlight being copied was painted over.
    pub fn selected_text(&self) -> Option<String> {
        let mouse = self.mouse.as_ref()?;
        let handle = self.handle.as_ref()?;
        let selection = mouse.gesture.selection_in(mouse.cells)?;
        let snapshot = handle.snapshot();

        match mouse.cells {
            Cells::List => {
                let blocks = handle.blocks();
                selection.text_with(&Blocks::list(&blocks, &snapshot), self.filter())
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
        let Some(clipboard) = self.clipboard.as_ref() else {
            return false;
        };
        if clipboard.write(text) {
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

    /// Ends the gesture, reporting whether this pane had one, and offers what
    /// it selected as PRIMARY where the desktop keeps one.
    ///
    /// Only a gesture that ends holding a selection offers anything: a plain
    /// click lets go of the highlight and leaves PRIMARY with whichever
    /// program holds it, the way every X11 terminal does. See
    /// [`crate::clipboard`].
    pub fn release(&self) -> bool {
        if !self
            .mouse
            .as_ref()
            .is_some_and(|mouse| mouse.gesture.release())
        {
            return false;
        }
        // Asked before the text is, because the text is not free: on the grid
        // it is read back out of the emulator's history, and with PRIMARY off
        // or absent nothing would be done with it.
        if clipboard::primary_in_use()
            && let (Some(clipboard), Some(selected)) =
                (self.clipboard.as_ref(), self.selected_text())
        {
            clipboard.write_primary(&selected);
        }
        true
    }

    /// Asks for PRIMARY to be pasted into this pane, for a middle press the
    /// program here did not take, reporting whether the press was answered.
    ///
    /// Dispatched rather than read here, because a read waits on whichever
    /// program holds PRIMARY and this runs on the thread that draws; see
    /// [`WorkspaceAction::PastePrimary`]. `false` where there is no PRIMARY to
    /// paste, or the person switched it off, which leaves the press doing what
    /// it did before there was any: nothing.
    pub fn paste_primary(&self, ctx: &mut EventContext) -> bool {
        let Some(pane) = self.pane() else {
            return false;
        };
        if !clipboard::primary_in_use() {
            return false;
        }
        ctx.dispatch_typed_action(WorkspaceAction::PastePrimary(pane));
        true
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

#[cfg(test)]
mod tests {
    use crookui_core::event::Modifiers;

    use super::*;

    fn ctrl_shift() -> Modifiers {
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        }
    }

    /// A pane whose program holds the screen: an agent, `ssh`, a REPL.
    fn program() -> input_keys::Pane {
        input_keys::Pane {
            composer: false,
            line_is_empty: true,
            grid_has_selection: false,
        }
    }

    /// On macOS `ctrl-shift-v` is a paste chord for a program and nothing to
    /// a text field, so a find bar over an agent let it through to the
    /// output, which pasted the clipboard into the agent's prompt rather than
    /// the query. Asked of macOS's keymap on every machine, because the field
    /// off macOS takes the chord itself and the window's own tests can only
    /// show the gap on a Mac.
    #[test]
    fn a_paste_chord_the_find_field_has_no_use_for_stays_out_of_the_program() {
        let paste = Keystroke::new("v", ctrl_shift());
        assert_eq!(
            None,
            input_keys::intent(&paste, "", Platform::Mac),
            "the field leaves the chord to whatever is under it"
        );
        assert_eq!(
            Route::PasteToShell,
            input_keys::route(&paste, "", program(), Platform::Mac),
            "and the routing rule gives it to the program as a paste"
        );
        assert!(
            !Keys::SignalsAndCopy.hands_on(&paste),
            "a paste reached the program under the find bar"
        );
        assert!(
            !Keys::Signals.hands_on(&paste),
            "a paste reached the program under a menu"
        );
        assert!(Keys::All.hands_on(&paste), "the whole keyboard pastes");
    }

    /// What is left to the program under a menu or a find bar is the three
    /// keys a running command has to keep hearing, and no other.
    #[test]
    fn under_a_menu_or_a_find_bar_only_the_signals_are_handed_on() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        for keys in [Keys::Signals, Keys::SignalsAndCopy] {
            for signal in ["c", "d", "z"] {
                assert!(
                    keys.hands_on(&Keystroke::new(signal, ctrl)),
                    "{keys:?} kept ctrl-{signal} from the program"
                );
            }
            assert!(!keys.hands_on(&Keystroke::new("f5", Modifiers::default())));
            assert!(!keys.hands_on(&Keystroke::new("a", ctrl)));
        }
        assert!(!Keys::None.hands_on(&Keystroke::new("c", ctrl)));
    }
}
