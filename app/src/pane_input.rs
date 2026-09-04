//! One pane's command input, as the view holds it between frames.
//!
//! The element tree is thrown away and rebuilt every time a view re-renders,
//! so nothing that has to survive a keystroke can live in it — and an editor is
//! nothing but state that survives keystrokes. It lives here instead, behind an
//! [`Rc`], and the workspace keeps one per pane and hands a clone to the
//! element each frame. That is the same arrangement
//! [`MouseStateHandle`](crookui_core::elements::MouseStateHandle) uses, for the
//! same reason.
//!
//! What is kept is the editor, the drag a press started, when the person using
//! it last did something — which is the whole of the caret blink, because a
//! caret that is solid for half a second after every keystroke is a caret
//! nobody loses — and whether this field is the one the keyboard belongs to,
//! which is the one fact the element beside it cannot work out for itself.

use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crookui_core::geometry::RectF;

use crate::clipboard::Clipboard;
use crate::completion::Completions;
use crate::editor::{Editor, Selection};
use crate::input_keys::Intent;

/// How long the caret stays solid, and then hidden, before it flips again.
///
/// The interval every text field has blinked at since the Macintosh, near
/// enough. It is also how often the workspace's blink chain wakes up, so a
/// shorter one is a real cost rather than a preference.
pub const CARET_PHASE: Duration = Duration::from_millis(530);

/// One pane's input field: its editor, and what the mouse is doing to it.
///
/// Cheap to clone — it is an [`Rc`] — because the element that draws it takes
/// one every frame.
#[derive(Clone)]
pub struct PaneInput(Rc<Inner>);

struct Inner {
    editor: RefCell<Editor>,
    /// The selection gesture in progress, if a button is down.
    drag: Cell<Option<Drag>>,
    /// When this input was last used, which is what the caret blinks against.
    active_since: Cell<Instant>,
    /// Whether this pane's field is the one the keyboard belongs to.
    ///
    /// Here rather than in the element, because the element is a frame old by
    /// the time a keystroke reaches it: focus moves, or a pane closes, and the
    /// tree that says which field was listening is the one built *before* that
    /// happened. The workspace sets this the moment focus changes, and the
    /// element asks the input rather than trusting what it was built with —
    /// which is what stops a keystroke arriving in that gap from landing in a
    /// field nothing can draw or read back.
    has_keys: Cell<bool>,

    /// What an input method is composing, and where its own caret sits inside
    /// it.
    ///
    /// **Not in the editor**, and that is the whole design. A preedit is not
    /// text: it is replaced wholesale by the next one, it can be abandoned
    /// without leaving anything behind, and it must never reach the undo
    /// history or a submitted line. Keeping it beside the editor means every
    /// existing operation — a copy, a submit, `is_empty` — goes on answering
    /// about what was actually typed, and only the drawing has to know.
    preedit: RefCell<Preedit>,

    /// The completion request this field is waiting for an answer to, and what
    /// the last answer said.
    ///
    /// The serial is what makes a stale answer discardable: pressing Tab twice
    /// quickly leaves two requests outstanding, the shell answers both, and
    /// only the second is about the line on screen. It counts up and never
    /// resets, so no answer can be mistaken for a later one.
    completion: RefCell<CompletionState>,

    /// Where the caret was last painted, in window coordinates.
    ///
    /// Written by the element that draws the field and read by the window,
    /// which puts the input method's candidate list beside it. It has to come
    /// from the paint path: the caret's position is the result of wrapping the
    /// line at the width the field was given, and nothing else in the
    /// application knows either.
    caret_rect: Cell<Option<RectF>>,
}

/// What this field has asked the shell, and what it last heard back.
#[derive(Debug, Default)]
struct CompletionState {
    /// The number of the last request sent. Zero before any.
    asked: u64,
    /// The candidates the last *matching* answer carried, and the word they
    /// were for.
    ///
    /// Kept so the list can be drawn under the field, and dropped the moment
    /// the line changes: a list of what `car` could become is nonsense under a
    /// line that now says `cargo b`.
    showing: Option<(String, Completions)>,
}

/// The text an input method is composing, before it becomes text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preedit {
    /// The string being composed. Empty means there is no composition.
    text: String,
    /// Where the input method's own caret sits within [`Self::text`], as a
    /// byte offset. Always on a character boundary.
    caret: usize,
}

impl Preedit {
    /// Whether nothing is being composed.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The string being composed.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// How far into the composition the input method's caret is, in bytes.
    pub fn caret(&self) -> usize {
        self.caret
    }
}

/// A selection being dragged out: where it started, and in what units.
#[derive(Copy, Clone)]
struct Drag {
    /// The offset the press landed on, which the drag selects away from.
    anchor: usize,
    granularity: Granularity,
}

/// What one press selects at a time.
///
/// A drag keeps the granularity of the press that began it, so dragging after
/// a double click selects whole words — which is what every text field does and
/// what makes a double-click-and-drag worth having.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Granularity {
    /// One grapheme cluster: a single click.
    Character,
    /// A word: a double click.
    Word,
    /// A whole line: a triple click.
    Line,
}

impl Default for PaneInput {
    fn default() -> Self {
        Self::new()
    }
}

impl PaneInput {
    /// An empty input with nothing typed into it.
    pub fn new() -> Self {
        Self(Rc::new(Inner {
            editor: RefCell::new(Editor::new()),
            drag: Cell::new(None),
            active_since: Cell::new(Instant::now()),
            has_keys: Cell::new(false),
            preedit: RefCell::new(Preedit::default()),
            completion: RefCell::new(CompletionState::default()),
            caret_rect: Cell::new(None),
        }))
    }

    /// What an input method is composing in this field, if anything.
    pub fn preedit(&self) -> Ref<'_, Preedit> {
        self.0.preedit.borrow()
    }

    /// The line as far as the caret, which is the question a completion
    /// answers.
    ///
    /// The prefix rather than the whole line and an offset, because that is
    /// what every shell's completion takes — `complete -C` in fish and
    /// `compgen` in bash both complete the end of what they are given.
    pub fn line_to_caret(&self) -> String {
        let editor = self.0.editor.borrow();
        editor.text()[..editor.caret()].to_owned()
    }

    /// Records that a completion has been asked for, and returns its number.
    ///
    /// Counts up and never resets, so an answer to a request two keystrokes
    /// ago cannot be mistaken for the answer to this one.
    pub fn ask_for_completions(&self) -> u64 {
        let mut completion = self.0.completion.borrow_mut();
        completion.asked += 1;
        completion.showing = None;
        completion.asked
    }

    /// Applies an answer, reporting whether anything on screen changed.
    ///
    /// Three outcomes, and they are the three every shell's Tab has. One
    /// candidate is inserted whole. Several insert as much as they agree on.
    /// An answer that adds nothing to what is typed is *shown* instead, which
    /// is the only useful thing left to do with it.
    ///
    /// An answer whose number is not the one this field is waiting for is
    /// dropped: it is about a line that has since been typed past.
    pub fn take_completions(&self, serial: u64, answer: Completions) -> bool {
        {
            let completion = self.0.completion.borrow();
            if completion.asked != serial {
                return false;
            }
        }

        let word = crate::completion::word_at_end(&self.line_to_caret()).to_owned();
        if let Some(whole) = answer.insertion(&word) {
            let addition = whole[word.len()..].to_owned();
            self.0.completion.borrow_mut().showing = None;
            self.edit(|editor| editor.insert(&addition));
            return true;
        }

        let showing = (answer.candidates.len() > 1).then_some((word, answer));
        let mut completion = self.0.completion.borrow_mut();
        if completion.showing == showing {
            return false;
        }
        completion.showing = showing;
        true
    }

    /// The candidates to draw under the field, if any are still relevant.
    ///
    /// Dropped the moment the word under the caret is no longer the one they
    /// were for: a list of what `car` could become is nonsense under a line
    /// that now says `cargo b`.
    pub fn showing_completions(&self) -> Option<Completions> {
        let completion = self.0.completion.borrow();
        let (word, answer) = completion.showing.as_ref()?;
        (crate::completion::word_at_end(&self.line_to_caret()) == word).then(|| answer.clone())
    }

    /// Whether an input method is mid-composition here.
    ///
    /// The one question the rest of the field asks: while this is true a click
    /// does not move the caret, because the offsets the pointer resolves
    /// against are offsets into a string that includes a preedit the editor
    /// has never heard of.
    pub fn is_composing(&self) -> bool {
        !self.0.preedit.borrow().is_empty()
    }

    /// Replaces what the input method is composing.
    ///
    /// `caret` is clamped onto a character boundary of `text`, because an
    /// input method reporting a range this build does not understand must not
    /// be able to panic the field. Reports whether anything changed, which is
    /// what decides if the frame is worth redrawing.
    pub fn set_preedit(&self, text: &str, caret: usize) -> bool {
        let mut caret = caret.min(text.len());
        while caret > 0 && !text.is_char_boundary(caret) {
            caret -= 1;
        }

        let mut preedit = self.0.preedit.borrow_mut();
        let replacement = Preedit {
            text: text.to_owned(),
            caret,
        };
        if *preedit == replacement {
            return false;
        }
        *preedit = replacement;
        drop(preedit);
        // The caret is solid while somebody is composing, for the same reason
        // it is solid while somebody is typing: it is being looked at.
        self.0.active_since.set(Instant::now());
        true
    }

    /// Abandons any composition, reporting whether there was one.
    ///
    /// What an input method that gave up produces, and what a commit does
    /// before it inserts: in both cases what was on screen belongs to nothing.
    pub fn clear_preedit(&self) -> bool {
        let had = !self.0.preedit.borrow().is_empty();
        if had {
            *self.0.preedit.borrow_mut() = Preedit::default();
        }
        had
    }

    /// Where the caret was last painted, in window coordinates.
    pub fn caret_rect(&self) -> Option<RectF> {
        self.0.caret_rect.get()
    }

    /// Records where the caret has just been painted, so the window can put an
    /// input method's candidate list beside it.
    pub fn set_caret_rect(&self, rect: Option<RectF>) {
        self.0.caret_rect.set(rect);
    }

    /// Whether the keyboard belongs to this field.
    pub fn has_keys(&self) -> bool {
        self.0.has_keys.get()
    }

    /// Says whether the keyboard belongs to this field from now on.
    ///
    /// The workspace's to call, and it calls it for every field it keeps
    /// whenever focus could have moved — including for the field of a pane it
    /// is about to forget, which is the one case an element cannot notice on
    /// its own.
    pub fn set_has_keys(&self, has_keys: bool) {
        self.0.has_keys.set(has_keys);
    }

    /// The editor, to read: what a renderer paints from.
    pub fn editor(&self) -> Ref<'_, Editor> {
        self.0.editor.borrow()
    }

    /// Changes the editor, and restarts the caret's blink so that the caret is
    /// solid at the moment somebody is looking for it.
    pub fn edit<T>(&self, change: impl FnOnce(&mut Editor) -> T) -> T {
        self.0.active_since.set(Instant::now());
        change(&mut self.0.editor.borrow_mut())
    }

    /// Applies one keyboard intent, returning the line to send when it was
    /// Enter and `None` otherwise.
    pub fn apply(&self, intent: Intent, clipboard: &Clipboard) -> Option<String> {
        self.edit(|editor| apply(intent, clipboard, editor))
    }

    /// Throws the half-written line away: what Ctrl-C means, on the field's
    /// side of the same keystroke the shell is interrupted by.
    pub fn abandon(&self) {
        self.edit(Editor::clear);
    }

    /// Whether the caret is drawn at this instant.
    pub fn caret_is_visible(&self) -> bool {
        let phase = CARET_PHASE.as_millis();
        self.0.active_since.get().elapsed().as_millis() % (phase * 2) < phase
    }

    /// Starts a selection at `offset`, selecting a character, a word or a line
    /// according to how many clicks the press is part of.
    pub fn press(&self, offset: usize, click_count: u32) {
        let granularity = match click_count {
            1 => Granularity::Character,
            2 => Granularity::Word,
            _ => Granularity::Line,
        };

        self.edit(|editor| match granularity {
            Granularity::Character => editor.set_caret(offset),
            Granularity::Word => editor.select_word_at(offset),
            Granularity::Line => editor.select_line_at(offset),
        });
        // The anchor is the offset pressed rather than the edge of what it
        // selected: a word or line drag re-reads the segment holding it on
        // every move, so naming a position inside that segment is enough.
        self.0.drag.set(Some(Drag {
            anchor: offset,
            granularity,
        }));
    }

    /// Drags the selection out to `offset`, reporting whether a press was in
    /// progress to drag at all.
    pub fn drag_to(&self, offset: usize) -> bool {
        let Some(drag) = self.0.drag.get() else {
            return false;
        };

        self.edit(|editor| match drag.granularity {
            Granularity::Character => editor.set_selection(Selection::new(drag.anchor, offset)),
            Granularity::Word => extend_by(editor, drag.anchor, offset, Editor::select_word_at),
            Granularity::Line => extend_by(editor, drag.anchor, offset, Editor::select_line_at),
        });
        true
    }

    /// Ends the gesture, reporting whether there was one.
    pub fn release(&self) -> bool {
        self.0.drag.replace(None).is_some()
    }
}

/// Selects from the segment holding `anchor` to the segment holding `head`.
///
/// `select` is one of the editor's segment selections, so this is the same
/// query run twice: once for the end that stays put and once for the end under
/// the pointer.
fn extend_by(editor: &mut Editor, anchor: usize, head: usize, select: fn(&mut Editor, usize)) {
    select(editor, anchor);
    let from = editor.selection();
    select(editor, head);
    let to = editor.selection();

    let selection = if head >= anchor {
        Selection::new(from.start(), to.end())
    } else {
        Selection::new(from.end(), to.start())
    };
    editor.set_selection(selection);
}

/// Does what one keyboard intent means, and hands back the line when the intent
/// was to send one.
///
/// A free function rather than a method on [`Editor`], because half of these
/// are the clipboard's business and the editor deliberately knows nothing about
/// a machine it might be running on.
fn apply(intent: Intent, clipboard: &Clipboard, editor: &mut Editor) -> Option<String> {
    use crate::input_keys::Intent::*;

    match intent {
        Insert(text) => editor.insert(&text),
        // Not an edit, and deliberately nothing here. The line goes to the
        // shell and the answer comes back frames later on a channel of its
        // own; the element that saw the keystroke is what sends the question,
        // because it is the only thing holding the terminal to ask.
        Complete => {}
        Newline => editor.insert_newline(),
        Submit => return Some(editor.submit()),
        Backspace => editor.backspace(),
        DeleteForward => editor.delete_forward(),
        DeleteWordLeft => editor.delete_word_left(),
        DeleteWordRight => editor.delete_word_right(),
        DeleteToLineStart => editor.delete_to_line_start(),
        DeleteToLineEnd => editor.delete_to_line_end(),
        Move(motion) => editor.move_caret(motion),
        Extend(motion) => editor.extend_selection(motion),
        HistoryUp => editor.up(),
        HistoryDown => editor.down(),
        SelectAll => editor.select_all(),
        Copy => {
            if let Some(copied) = editor.copy() {
                clipboard.write(&copied);
            }
        }
        Cut => {
            if let Some(cut) = editor.cut() {
                clipboard.write(&cut);
            }
        }
        // Nothing on the clipboard and no clipboard at all are the same thing
        // from here: there is nothing to insert either way.
        Paste => {
            if let Some(pasted) = clipboard.read() {
                editor.paste(&pasted);
            }
        }
        Undo => {
            editor.undo();
        }
        Redo => {
            editor.redo();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Motion;

    /// An input holding `text`, with the caret at the end.
    fn holding(text: &str) -> PaneInput {
        let input = PaneInput::new();
        input.edit(|editor| editor.set_text(text));
        input
    }

    #[test]
    fn a_press_puts_the_caret_where_it_landed_and_a_drag_selects_away_from_it() {
        let input = holding("echo hello");
        input.press(4, 1);
        assert_eq!(input.editor().caret(), 4);
        assert!(input.editor().selection().is_empty());

        assert!(input.drag_to(9));
        assert_eq!(input.editor().selected_text(), " hell");
        // Dragging back past the anchor selects the other way round.
        assert!(input.drag_to(0));
        assert_eq!(input.editor().selected_text(), "echo");

        assert!(input.release());
        assert!(
            !input.drag_to(2),
            "a drag with no press behind it selects nothing"
        );
    }

    #[test]
    fn a_double_click_selects_a_word_and_dragging_keeps_selecting_them_whole() {
        let input = holding("one two three");
        input.press(5, 2);
        assert_eq!(input.editor().selected_text(), "two");

        input.drag_to(9);
        assert_eq!(
            input.editor().selected_text(),
            "two three",
            "a word drag never stops halfway through one"
        );
    }

    #[test]
    fn a_triple_click_selects_the_line() {
        let input = holding("first\nsecond");
        input.press(8, 3);
        assert_eq!(input.editor().selected_text(), "second");
    }

    #[test]
    fn enter_hands_back_the_line_and_leaves_the_field_empty() {
        let input = holding("echo hi");
        let clipboard = Clipboard::new();

        assert_eq!(
            input.apply(Intent::Submit, &clipboard),
            Some("echo hi".to_owned())
        );
        assert!(input.editor().is_empty());
        assert_eq!(input.editor().history(), ["echo hi"]);
    }

    #[test]
    fn every_other_intent_sends_nothing_to_the_shell() {
        let input = holding("echo hi");
        let clipboard = Clipboard::new();

        for intent in [
            Intent::Insert("!".to_owned()),
            Intent::Newline,
            Intent::Backspace,
            Intent::Move(Motion::LineStart),
            Intent::SelectAll,
            Intent::Undo,
        ] {
            assert_eq!(input.apply(intent, &clipboard), None);
        }
    }

    #[test]
    fn cutting_nothing_leaves_the_line_and_the_clipboard_alone() {
        // The guard that matters: a cut with nothing selected must not reach
        // the clipboard at all, or an idle `cmd-x` would wipe whatever somebody
        // had copied somewhere else.
        let input = holding("echo hi");
        input.edit(|editor| editor.set_caret(0));

        assert_eq!(input.apply(Intent::Cut, &Clipboard::new()), None);
        assert_eq!(input.editor().text(), "echo hi");
    }

    #[test]
    fn abandoning_the_line_leaves_no_trace_of_it() {
        // The field's half of Ctrl-C. Not a command that ran, so nothing about
        // it belongs in the history either.
        let input = holding("rm -rf important");
        input.abandon();

        assert!(input.editor().is_empty());
        assert!(
            input.editor().history().is_empty(),
            "an abandoned line is not a command that ran"
        );
    }

    #[test]
    fn a_field_holds_the_keyboard_only_while_it_is_given_it() {
        // What a closed or unfocused pane's field is told, and what the
        // element tree — a frame behind by the time a key arrives — asks
        // instead of trusting what it was built with.
        let input = holding("echo hi");
        assert!(
            !input.has_keys(),
            "a field starts with nothing focused on it"
        );

        input.set_has_keys(true);
        assert!(input.has_keys());
        input.set_has_keys(false);
        assert!(!input.has_keys());
    }

    #[test]
    fn the_caret_is_solid_the_moment_the_field_is_used() {
        let input = holding("x");
        assert!(
            input.caret_is_visible(),
            "a caret that blinked out under somebody's fingers is a lost caret"
        );
    }
}
