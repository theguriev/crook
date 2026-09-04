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

use crookui_core::geometry::RectF;

use crate::clipboard::Clipboard;
use crate::completion::{self, Completions};
use crate::editor::{Editor, Motion, Selection};
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
    /// What the shell offered that Tab has not typed yet, if anything.
    offered: Option<Offer>,
}

/// The candidates a Tab turned up, and which of them is being offered.
///
/// **There is no list on screen.** A shell prints its candidates in columns
/// and every terminal that has tried to improve on that has ended up with a
/// panel: a second surface, in a second type, that appears under whatever a
/// person is reading. So the answer is offered the way the history is — one
/// candidate at a time, in dim ink after the caret, in the line itself — and
/// Tab steps through the rest.
#[derive(Debug)]
struct Offer {
    /// The word the shell was asked about.
    ///
    /// What says whether the answer is still about the line on screen: typing
    /// on past it narrows the same answer, and anything else means the shell
    /// was asked a question nobody is asking any more.
    stem: String,
    /// What it answered.
    answer: Completions,
    /// Which of the candidates that still match is being shown.
    at: usize,
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

impl Default for TextInput {
    fn default() -> Self {
        Self::new()
    }
}

impl TextInput {
    /// An empty input that starts with the shell's own history behind it: the
    /// field a pane composes in.
    ///
    /// Every other field in the window — the palette's, the settings rail's,
    /// the search box above the tabs — is [`Self::new`], because none of them
    /// is a command line and a person's shell history has nothing to say about
    /// what they are filtering. See [`crate::shell_history`].
    pub fn for_pane() -> Self {
        let input = Self::new();
        input.edit(|editor| editor.seed_history(crate::shell_history::user_history().to_vec()));
        input
    }

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
        completion.offered = None;
        completion.asked
    }

    /// Applies an answer, reporting whether anything on screen changed.
    ///
    /// The three outcomes every shell's Tab has, with the third one moved into
    /// the line: one candidate is typed whole, several type as much as they
    /// agree on, and what is still ambiguous after that is *offered* — the
    /// first of the candidates in dim ink after the caret, with Tab stepping
    /// to the next. No list, on purpose: see [`Offer`].
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

        let stem = completion::word_at_end(&self.line_to_caret()).to_owned();
        let typed = answer.insertion(&stem).is_some_and(|whole| {
            self.edit(|editor| editor.insert(&whole[stem.len()..]));
            true
        });

        // Against the word as it stands *after* that insertion, which is the
        // word an offer has to add to: Tab on `Car` with `Cargo.toml` and
        // `Cargo.lock` types `Cargo.` and what is left to offer is the rest of
        // one of the two.
        let line = self.line_to_caret();
        let word = completion::word_at_end(&line);
        let offered = if answer.matching(word).len() > 1 {
            Some(Offer {
                stem,
                answer,
                at: 0,
            })
        } else {
            None
        };

        let mut completion = self.0.completion.borrow_mut();
        // A frame is owed for the insertion as much as for the offer: a Tab
        // that typed the rest of a word and left nothing ambiguous has still
        // changed the line.
        let changed = typed || offered.is_some() || completion.offered.is_some();
        completion.offered = offered;
        changed
    }

    /// Steps to the next candidate the shell offered, reporting whether there
    /// was one to step through.
    ///
    /// It wraps. A person pressing Tab through four candidates has no reason
    /// to be stopped at the fourth and made to press something else.
    pub fn cycle_completion(&self, forward: bool) -> bool {
        let line = self.line_to_caret();
        let word = completion::word_at_end(&line);
        let mut completion = self.0.completion.borrow_mut();
        let Some(offer) = completion.offered.as_mut() else {
            return false;
        };
        if !word.starts_with(&offer.stem) {
            return false;
        }

        let total = offer.answer.matching(word).len();
        if total < 2 {
            return false;
        }
        offer.at = if forward {
            (offer.at + 1) % total
        } else {
            (offer.at + total - 1) % total
        };
        drop(completion);
        // The caret is solid while a person is stepping through candidates,
        // for the same reason it is solid while they type.
        self.edit(|_| ());
        true
    }

    /// Forgets what the shell offered, reporting whether there was anything.
    ///
    /// The request number moves on with it, so an answer already in flight
    /// cannot put back an offer that has been overtaken — by a submitted line,
    /// or by the interrupt that threw the line away.
    pub fn forget_completions(&self) -> bool {
        let mut completion = self.0.completion.borrow_mut();
        let had = completion.offered.take().is_some();
        completion.asked += 1;
        had
    }

    /// What would be added to the line if the suggestion were taken, drawn
    /// after the caret in dim ink.
    ///
    /// Two sources, and the order is what makes them one feature. A candidate
    /// the shell offered comes first: Tab is a question somebody has just
    /// asked, and the answer to it outranks anything recalled. Otherwise it is
    /// the newest command in the history that starts with this line — zsh's
    /// `autosuggestions` and Warp's ghost text, which are the same thing.
    ///
    /// Only ever at the very end of the line, because text after the caret is
    /// text the suggestion would be standing in front of.
    pub fn suggestion(&self) -> Option<String> {
        self.offered_completion()
            .or_else(|| self.0.editor.borrow().suggestion().map(str::to_owned))
    }

    /// The tail of the candidate being offered, if one still answers the word
    /// under the caret.
    fn offered_completion(&self) -> Option<String> {
        let editor = self.0.editor.borrow();
        if !editor.selection().is_empty() || editor.caret() != editor.text().len() {
            return None;
        }
        drop(editor);

        let line = self.line_to_caret();
        let word = completion::word_at_end(&line);
        let completion = self.0.completion.borrow();
        let offer = completion.offered.as_ref()?;
        if !word.starts_with(&offer.stem) {
            return None;
        }

        let matching = offer.answer.matching(word);
        let candidate = matching.get(offer.at.min(matching.len().checked_sub(1)?))?;
        candidate
            .strip_prefix(word)
            .filter(|rest| !rest.is_empty())
            .map(str::to_owned)
    }

    /// Types the suggestion, all of it or one word, reporting whether there
    /// was one.
    ///
    /// The right arrow at the end of a line, which is otherwise a keystroke
    /// that does nothing at all — there is nowhere further right to go — and
    /// the word arrow for the same reason.
    pub fn accept_suggestion(&self, whole: bool) -> bool {
        let Some(suggestion) = self.suggestion() else {
            return false;
        };
        let taken = if whole {
            suggestion
        } else {
            first_word(&suggestion).to_owned()
        };
        self.edit(|editor| editor.insert(&taken));
        // What was offered has been typed, and what is left of the answer is
        // about a word that is now whole.
        self.forget_completions();
        true
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
    ///
    /// **Three keys mean something else while a suggestion stands after the
    /// caret**, and that is decided here rather than in
    /// [`crate::input_keys`]. Which keystrokes reach the field at all is a
    /// question about the *pane* — a selection, the alternate screen, a signal
    /// — and what the field then does with one is a question about the field,
    /// exactly as the bare Up key being a line or a history entry has always
    /// been the editor's rule and not the keymap's.
    ///
    /// Right takes the whole suggestion, the word arrow takes one word of it,
    /// and the line-end chord takes it whole as well. All three would
    /// otherwise do nothing at all: a suggestion only ever stands at the end
    /// of the line, and there is nowhere further right to go from there.
    pub fn apply(&self, intent: Intent, clipboard: &Clipboard) -> Option<String> {
        match intent {
            Intent::Move(Motion::Right | Motion::LineEnd) if self.suggestion().is_some() => {
                self.accept_suggestion(true);
                return None;
            }
            Intent::Move(Motion::WordRight) if self.suggestion().is_some() => {
                self.accept_suggestion(false);
                return None;
            }
            _ => {}
        }

        let submitted = self.edit(|editor| apply(intent, clipboard, editor));
        if submitted.is_some() {
            // The line has gone to the shell, so candidates for a word in it
            // are candidates for nothing.
            self.forget_completions();
        }
        submitted
    }

    /// Throws the half-written line away: what Ctrl-C means, on the field's
    /// side of the same keystroke the shell is interrupted by.
    pub fn abandon(&self) {
        self.edit(Editor::clear);
        self.forget_completions();
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
        // Neither is an edit. Tab's question goes to the shell through a
        // channel of its own — the element that saw the keystroke sends it,
        // being the only thing holding the terminal to ask — and stepping
        // through what came back is the field's, above.
        Complete | CompleteBackwards => {}
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

/// The first word of a suggestion, with the space that follows it.
///
/// What the word arrow takes: a suggestion is a whole command line and taking
/// `git commit --amend --no-edit` when what was wanted was `git commit` is why
/// every shell that offers one offers this too. The space comes with the word
/// so that the next press starts on the next one.
fn first_word(suggestion: &str) -> &str {
    let leading = suggestion.len() - suggestion.trim_start_matches(' ').len();
    match suggestion[leading..].find(' ') {
        Some(at) => &suggestion[..leading + at + 1],
        None => suggestion,
    }
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

    /// The candidates a shell might have answered with.
    fn answer(candidates: &[&str]) -> Completions {
        Completions {
            candidates: candidates.iter().map(|c| (*c).to_owned()).collect(),
            truncated: false,
        }
    }

    /// Asks, and answers, in one step: the round trip a Tab makes.
    fn answered(input: &TextInput, candidates: &[&str]) {
        let serial = input.ask_for_completions();
        input.take_completions(serial, answer(candidates));
    }

    #[test]
    fn tab_types_what_is_certain_and_offers_the_rest_after_the_caret() {
        // A shell's Tab, with its third case moved into the line: what every
        // candidate agrees on is typed, and the ambiguity that is left is one
        // candidate standing after the caret rather than a list under it.
        let input = holding("cargo C");
        answered(&input, &["Cargo.toml", "Cargo.lock"]);

        assert_eq!(input.editor().text(), "cargo Cargo.");
        assert_eq!(input.suggestion().as_deref(), Some("toml"));
    }

    #[test]
    fn tab_again_steps_to_the_next_candidate_and_wraps() {
        let input = holding("cargo C");
        answered(&input, &["Cargo.toml", "Cargo.lock"]);

        assert!(input.cycle_completion(true));
        assert_eq!(input.suggestion().as_deref(), Some("lock"));
        assert!(input.cycle_completion(true));
        assert_eq!(
            input.suggestion().as_deref(),
            Some("toml"),
            "four candidates and four presses come back to the first"
        );
        assert!(input.cycle_completion(false));
        assert_eq!(input.suggestion().as_deref(), Some("lock"), "and back");
        assert_eq!(
            input.editor().text(),
            "cargo Cargo.",
            "none of which typed anything"
        );
    }

    #[test]
    fn typing_on_narrows_what_is_offered_and_a_new_word_drops_it() {
        let input = holding("car");
        answered(&input, &["cargo", "carbon", "cartridge"]);
        assert_eq!(input.suggestion().as_deref(), Some("go"));

        input.edit(|editor| editor.insert("t"));
        assert_eq!(
            input.suggestion().as_deref(),
            Some("ridge"),
            "the candidates the word has ruled out are not stepped through"
        );

        input.edit(|editor| editor.insert(" b"));
        assert_eq!(
            input.suggestion(),
            None,
            "a new word is a question this answer cannot answer"
        );
        assert!(
            !input.cycle_completion(true),
            "and there is nothing left to step through"
        );
    }

    #[test]
    fn a_candidate_the_shell_offered_outranks_one_the_history_remembers() {
        // Tab is a question somebody has just asked. What comes back to it
        // outranks anything recalled.
        let input = holding("car");
        input.edit(|editor| editor.seed_history(vec!["carbon --dry-run".to_owned()]));
        assert_eq!(input.suggestion().as_deref(), Some("bon --dry-run"));

        answered(&input, &["cargo", "cartridge"]);
        assert_eq!(input.suggestion().as_deref(), Some("go"));
    }

    #[test]
    fn the_right_arrow_takes_what_is_offered_and_the_word_arrow_takes_one_word() {
        let clipboard = Clipboard::new();
        let input = holding("git ");
        input.edit(|editor| editor.seed_history(vec!["git commit --amend".to_owned()]));

        assert_eq!(input.suggestion().as_deref(), Some("commit --amend"));
        input.apply(Intent::Move(Motion::WordRight), &clipboard);
        assert_eq!(
            input.editor().text(),
            "git commit ",
            "one word, with the space that follows it"
        );

        input.apply(Intent::Move(Motion::Right), &clipboard);
        assert_eq!(input.editor().text(), "git commit --amend");
        assert_eq!(
            input.suggestion(),
            None,
            "and there is nothing left to suggest"
        );
    }

    #[test]
    fn taking_a_candidate_leaves_nothing_to_step_through() {
        let clipboard = Clipboard::new();
        let input = holding("car");
        answered(&input, &["cargo", "carbon"]);

        input.apply(Intent::Move(Motion::Right), &clipboard);
        assert_eq!(input.editor().text(), "cargo");
        assert!(
            !input.cycle_completion(true),
            "the word is whole, and the rest of the answer was about half of it"
        );
    }

    #[test]
    fn a_line_that_has_gone_to_the_shell_forgets_what_was_offered() {
        let clipboard = Clipboard::new();
        let input = holding("car");
        answered(&input, &["cargo", "carbon"]);

        assert_eq!(
            input.apply(Intent::Submit, &clipboard).as_deref(),
            Some("car"),
            "what was offered was never in the line, so Enter sends what was typed"
        );
        assert_eq!(input.suggestion(), None);
    }

    #[test]
    fn a_suggestion_stands_only_at_the_end_of_a_line_nobody_is_selecting() {
        let input = holding("git ");
        input.edit(|editor| editor.seed_history(vec!["git status".to_owned()]));
        assert_eq!(input.suggestion().as_deref(), Some("status"));

        input.edit(|editor| editor.move_caret(Motion::Left));
        assert_eq!(
            input.suggestion(),
            None,
            "text after the caret is what a suggestion would be standing in front of"
        );

        input.edit(|editor| editor.select_all());
        assert_eq!(input.suggestion(), None);
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
