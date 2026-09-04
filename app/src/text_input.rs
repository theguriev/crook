//! A text field's state, as the view holds it between frames.
//!
//! The element tree is thrown away and rebuilt every time a view re-renders,
//! so nothing that has to survive a keystroke can live in it — and an editor is
//! nothing but state that survives keystrokes. It lives here instead, behind an
//! [`Rc`], and the workspace keeps one per field and hands a clone to the
//! element each frame. That is the same arrangement
//! [`MouseStateHandle`](crookui_core::elements::MouseStateHandle) uses, for the
//! same reason.
//!
//! There are two kinds of field: the composer under every pane, one per pane,
//! and the settings page's search box. Nothing here is a pane's — the file was
//! called `pane_input.rs` while a pane was the only thing that could hold one —
//! except [`TextInput::apply`]\'s [`Submit`](crate::input_keys::Intent::Submit)
//! arm, which hands back a line for somebody else to send, and
//! [`TextInput::abandon`]. A field with nowhere to send a line simply does not
//! ask for that intent.
//!
//! What is kept is the editor, the drag a press started, when the person using
//! it last did something — which is the whole of the caret blink, because a
//! caret that is solid for half a second after every keystroke is a caret
//! nobody loses — and whether this field is the one the keyboard belongs to,
//! which is the one fact the element beside it cannot work out for itself.

use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::clipboard::Clipboard;
use crate::editor::{Editor, Selection};
use crate::input_keys::Intent;

/// How long the caret stays solid, and then hidden, before it flips again.
///
/// The interval every text field has blinked at since the Macintosh, near
/// enough. It is also how often the workspace's blink chain wakes up, so a
/// shorter one is a real cost rather than a preference.
pub const CARET_PHASE: Duration = Duration::from_millis(530);

/// One text field: its editor, and what the mouse is doing to it.
///
/// Cheap to clone — it is an [`Rc`] — because the element that draws it takes
/// one every frame.
#[derive(Clone)]
pub struct TextInput(Rc<Inner>);

struct Inner {
    editor: RefCell<Editor>,
    /// The selection gesture in progress, if a button is down.
    drag: Cell<Option<Drag>>,
    /// When this input was last used, which is what the caret blinks against.
    active_since: Cell<Instant>,
    /// Whether this field is the one the keyboard belongs to.
    ///
    /// Here rather than in the element, because the element is a frame old by
    /// the time a keystroke reaches it: focus moves, or a pane closes, and the
    /// tree that says which field was listening is the one built *before* that
    /// happened. The workspace sets this the moment focus changes, and the
    /// element asks the input rather than trusting what it was built with —
    /// which is what stops a keystroke arriving in that gap from landing in a
    /// field nothing can draw or read back.
    has_keys: Cell<bool>,
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

impl Default for TextInput {
    fn default() -> Self {
        Self::new()
    }
}

impl TextInput {
    /// An empty input with nothing typed into it.
    pub fn new() -> Self {
        Self(Rc::new(Inner {
            editor: RefCell::new(Editor::new()),
            drag: Cell::new(None),
            active_since: Cell::new(Instant::now()),
            has_keys: Cell::new(false),
        }))
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
    fn holding(text: &str) -> TextInput {
        let input = TextInput::new();
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
