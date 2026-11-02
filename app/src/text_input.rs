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
//!
//! The blink stops, lit, in two cases: [`CARET_REST`] after the field was
//! last used, and for good while the window keeps still — the Appearance
//! page's "Motion", which [`let_carets_blink`] puts in force. Both are
//! answered here, where the caret is drawn from, so that no field can blink
//! where another holds still.

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

/// How long a caret goes on blinking after its field was last used, before
/// it holds still, lit.
///
/// GNOME's `cursor-blink-timeout`, whose default it is; kitty and Alacritty
/// stop theirs too, after fifteen seconds and five. A blink is for finding
/// the caret again after looking away, and a caret nobody has typed at for
/// ten seconds is not being looked for — it is a pulse at the edge of the
/// eye of somebody reading something else.
pub const CARET_REST: Duration = Duration::from_secs(10);

thread_local! {
    /// Whether a caret on this thread's window blinks at all: the Appearance
    /// page's "Motion", as the workspace resolves it.
    ///
    /// Per thread, as the theme in force is and as
    /// [`crate::clipboard::use_primary`] is, and for the same reasons: it is
    /// the window's, and it reaches every field on it — a plugin's own
    /// included — without being handed to the dozen places a field is drawn.
    static BLINKS: Cell<bool> = const { Cell::new(true) };

    /// When a field on this thread's window was last used, which is the
    /// latest of every field's own [`Inner::active_since`].
    ///
    /// What lets the blink chain ask "is any caret still blinking" without
    /// finding the field that has the keyboard: only that field is typed in,
    /// so the latest use is nearly always its own. Never earlier than any
    /// field's own, which is the half that matters — a caret that can still
    /// be in the dark half of its blink is a caret the chain is still
    /// repainting for.
    static LAST_USED: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Says whether a caret on this thread's window blinks, which is what the
/// "Motion" setting resolves to. A caret that does not is drawn solid.
pub fn let_carets_blink(blink: bool) {
    BLINKS.set(blink);
}

/// Whether a caret on this thread's window may be blinking now: carets blink
/// at all, and a field was used within [`CARET_REST`].
///
/// What the workspace's blink chain asks before it repaints: once it says
/// no, every caret on the window is solid, and another frame would draw the
/// same one.
pub fn a_caret_blinks() -> bool {
    BLINKS.get()
        && LAST_USED
            .get()
            .is_some_and(|used| used.elapsed() < CARET_REST)
}

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

    /// Where this field's pane is and what it ran last, which ranks the
    /// suggestion after the caret — see [`crate::recall`]. `None` for every
    /// field that is not a pane's command line: a palette's search box has no
    /// directory, and the commands a person ran are nothing to suggest in it.
    context: RefCell<Option<crate::recall::Context>>,

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
    /// The line up to the caret when it was sent, which is what the shell
    /// is answering about.
    asked_about: String,
    /// What the shell offered that Tab has not typed yet, if anything.
    offered: Option<Offer>,
}

/// The candidates a Tab turned up, and which of them is selected.
///
/// **Two things on screen, and the line comes first.** The selected candidate
/// is offered the way the history is — in dim ink after the caret, in the line
/// itself — and the rest stand in a small list over the line, in the
/// terminal's own type and on its cell grid, with what the shell said about
/// each. One candidate at a time was the first design, and it cannot say how
/// many there are, which one is which, or that `cherry` and `cherry-pick` are
/// different commands; a list shown only while there is a choice to make, and
/// gone the moment there is not, can. See [`Menu`].
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
    /// Which of the candidates that still match is selected, as of the word
    /// in [`Self::seen`].
    at: usize,
    /// The word as it stood when [`Self::at`] was chosen. Typing on changes
    /// what the list holds and what order it is in, so a selection made
    /// against one word means nothing against the next: the top of the new
    /// list is selected instead, the way every filtering list behaves.
    seen: String,
}

/// How many candidates the list shows at once.
///
/// Enough to choose among without reading; a longer answer scrolls under the
/// selection.
pub const MENU_ROWS: usize = 8;

/// What the list over the line shows, for the field to draw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    /// The candidates in view, at most [`MENU_ROWS`] of them.
    pub items: Vec<MenuItem>,
    /// Which of [`Self::items`] is selected.
    pub selected: usize,
    /// The number of the selected candidate in the whole list, from zero.
    pub position: usize,
    /// How many candidates still match, in view or not.
    pub total: usize,
    /// Whether the shell offered more than it was allowed to send.
    pub truncated: bool,
    /// Where in the line the word being completed begins, in bytes: what the
    /// list lines up under.
    pub word_start: usize,
}

/// One row of the [`Menu`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// The candidate, as it would go on the line.
    pub candidate: String,
    /// What the shell said about it.
    pub description: Option<String>,
    /// The characters the typed word matched, by character index.
    pub matched: Vec<usize>,
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
        *input.0.context.borrow_mut() = Some(crate::recall::Context::default());
        input
    }

    /// Says where this field's pane now is, for the suggestion to rank by.
    pub fn set_directory(&self, directory: Option<std::path::PathBuf>) {
        if let Some(context) = self.0.context.borrow_mut().as_mut() {
            context.directory = directory;
        }
    }

    /// Says what this field's pane ran last, for the suggestion to rank by.
    pub fn set_previous(&self, command: Option<String>) {
        if let Some(context) = self.0.context.borrow_mut().as_mut() {
            context.previous = command;
        }
    }

    /// Where this field's pane is and what it ran last, if it is a pane's.
    pub fn context(&self) -> Option<crate::recall::Context> {
        self.0.context.borrow().clone()
    }

    /// An empty input with nothing typed into it.
    pub fn new() -> Self {
        let input = Self(Rc::new(Inner {
            editor: RefCell::new(Editor::new()),
            drag: Cell::new(None),
            active_since: Cell::new(Instant::now()),
            has_keys: Cell::new(false),
            preedit: RefCell::new(Preedit::default()),
            completion: RefCell::new(CompletionState::default()),
            context: RefCell::new(None),
            caret_rect: Cell::new(None),
        }));
        // A field starts its blink lit, and the window's clock has to know,
        // or a caret drawn in the field's dark half would be one the blink
        // chain thought was at rest.
        input.used();
        input
    }

    /// Restarts the caret's blink: the field is being used.
    fn used(&self) {
        let now = Instant::now();
        self.0.active_since.set(now);
        LAST_USED.set(Some(now));
    }

    /// Puts the field's last use `by` further back, as though nobody had
    /// touched it since — and the window's with it, as the field a test has
    /// just used is the last one used.
    ///
    /// For a test of what a caret does half a blink, or ten seconds, after
    /// it was used, which would otherwise have to sleep for it.
    #[cfg(test)]
    pub(crate) fn backdate(&self, by: Duration) {
        let then = self
            .0
            .active_since
            .get()
            .checked_sub(by)
            .expect("the clock goes back that far");
        self.0.active_since.set(then);
        LAST_USED.set(Some(then));
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
        let line = self.line_to_caret();
        let mut completion = self.0.completion.borrow_mut();
        completion.asked_about = line;
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
    /// dropped: it is about a line that has since been typed past. So is one
    /// that arrives after the line itself changed — typing, a paste, the
    /// caret moved — which the number alone cannot see, since only a Tab, an
    /// Enter or an abandoned line moves it on: the shell answered about a
    /// word that is no longer the one at the caret, and `build/` completed
    /// for `echo bu` was typed into a line that had become `bu`.
    pub fn take_completions(&self, serial: u64, answer: Completions) -> bool {
        {
            let completion = self.0.completion.borrow();
            if completion.asked != serial || completion.asked_about != self.line_to_caret() {
                return false;
            }
        }

        let stem = completion::word_at_end(&self.line_to_caret()).to_owned();
        let typed = answer.insertion(&stem).is_some_and(|whole| {
            self.edit(|editor| {
                // In place of the stem, not after it. The match ignores case,
                // so the letters typed may be the wrong case for the name the
                // shell returned — `car` for `Cargo.toml`; appending the tail
                // would leave `cargo.toml`, a path that is not there, where
                // replacing the stem writes the name the shell actually gave.
                let caret = editor.caret();
                editor.set_selection(Selection::new(caret - stem.len(), caret));
                editor.insert(&whole);
            });
            true
        });

        // Against the word as it stands *after* that insertion, which is the
        // word an offer has to add to: Tab on `Car` with `Cargo.toml` and
        // `Cargo.lock` types `Cargo.` and what is left to offer is the rest of
        // one of the two.
        let line = self.line_to_caret();
        let word = completion::word_at_end(&line);
        let (count, exact) = {
            let matching = answer.matching(word);
            (
                matching.len(),
                matching.first().is_some_and(|&one| one == word),
            )
        };
        // One candidate, and the word is it: the completion is finished, and
        // what a shell's Tab types then is a space, so the next word can be
        // typed straight away. Also when the word was already whole before the
        // Tab — that Tab was asking whether anything is left, and nothing is.
        let closed = count == 1 && exact && completion::closes_word(word) && self.close_word();
        let offered = if count > 1 {
            Some(Offer {
                // The word as it now stands, in the shell's case: a later
                // keystroke extends this, not the mixed-case fragment that was
                // first typed and has since been replaced.
                stem: word.to_owned(),
                answer,
                at: 0,
                seen: word.to_owned(),
            })
        } else {
            None
        };

        let mut completion = self.0.completion.borrow_mut();
        // A frame is owed for the insertion as much as for the offer: a Tab
        // that typed the rest of a word and left nothing ambiguous has still
        // changed the line.
        let changed = typed || closed || offered.is_some() || completion.offered.is_some();
        completion.offered = offered;
        changed
    }

    /// Steps to the next candidate the shell offered, reporting whether there
    /// was one to step through.
    ///
    /// It wraps. A person pressing Tab through four candidates has no reason
    /// to be stopped at the fourth and made to press something else.
    pub fn cycle_completion(&self, forward: bool) -> bool {
        let Some((total, at)) = self.with_offer(|ranked, at| (ranked.len(), at)) else {
            return false;
        };
        if total < 2 {
            return false;
        }
        let line = self.line_to_caret();
        let mut completion = self.0.completion.borrow_mut();
        let Some(offer) = completion.offered.as_mut() else {
            return false;
        };
        offer.at = if forward {
            (at + 1) % total
        } else {
            (at + total - 1) % total
        };
        completion::word_at_end(&line).clone_into(&mut offer.seen);
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
    /// a command from the history that starts with this line — zsh's
    /// `autosuggestions` and Warp's ghost text, which are the same thing — and
    /// in a pane's field the one that fits where the pane is: ran in this
    /// directory, after what the pane just ran, and not one that failed. See
    /// [`crate::recall`].
    ///
    /// Only ever at the very end of the line, because text after the caret is
    /// text the suggestion would be standing in front of.
    pub fn suggestion(&self) -> Option<String> {
        self.offered_completion().or_else(|| {
            let editor = self.0.editor.borrow();
            match self.0.context.borrow().as_ref() {
                Some(context) => crate::recall::with_user(|recall| {
                    editor.suggestion_in(recall, context).map(str::to_owned)
                }),
                None => editor.suggestion().map(str::to_owned),
            }
        })
    }

    /// Runs `look` over the candidates still on offer for the word at the
    /// caret, best first, and the index of the selected one — or returns
    /// `None` when nothing is on offer.
    ///
    /// Nothing is while the caret is anywhere but the end of the line, while
    /// text is selected, once the word has been edited back past what the
    /// shell was asked about, and when the word rules every candidate out.
    fn with_offer<T>(&self, look: impl FnOnce(&[completion::Match<'_>], usize) -> T) -> Option<T> {
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

        let ranked = offer.answer.ranked(word);
        if ranked.is_empty() {
            return None;
        }
        let at = if offer.seen == word {
            offer.at.min(ranked.len() - 1)
        } else {
            0
        };
        Some(look(&ranked, at))
    }

    /// The tail of the selected candidate, if it extends the word under the
    /// caret.
    ///
    /// Only one that *starts* with the word: a candidate the list found by
    /// letters scattered through it has no tail to draw after the caret, and
    /// stands selected in the list alone.
    fn offered_completion(&self) -> Option<String> {
        let line = self.line_to_caret();
        let word = completion::word_at_end(&line);
        self.with_offer(|ranked, at| {
            ranked[at]
                .candidate
                .strip_prefix(word)
                .filter(|rest| !rest.is_empty())
                .map(str::to_owned)
        })
        .flatten()
    }

    /// Whether the list of what the shell offered is up.
    pub fn has_offer(&self) -> bool {
        self.with_offer(|_, _| ()).is_some()
    }

    /// What the list over the line shows, or `None` when it is not up.
    ///
    /// The window of [`MENU_ROWS`] that holds the selection: from the top
    /// until the selection would fall off its bottom, and then following it.
    pub fn menu(&self) -> Option<Menu> {
        let line = self.line_to_caret();
        let word_start = line.len() - completion::word_at_end(&line).len();
        let truncated = self
            .0
            .completion
            .borrow()
            .offered
            .as_ref()
            .is_some_and(|offer| offer.answer.truncated);
        self.with_offer(|ranked, at| {
            let first = (at + 1).saturating_sub(MENU_ROWS);
            let items = ranked
                .iter()
                .skip(first)
                .take(MENU_ROWS)
                .map(|found| MenuItem {
                    candidate: found.candidate.to_owned(),
                    description: found.description.map(str::to_owned),
                    matched: found.matched.clone(),
                })
                .collect();
            Menu {
                items,
                selected: at - first,
                position: at,
                total: ranked.len(),
                truncated,
                word_start,
            }
        })
    }

    /// Puts the selected candidate on the line in place of the word under the
    /// caret, and puts the list away. Reports whether there was one.
    ///
    /// In place of the word rather than after it, because the candidate may
    /// not start with it — the list matches letters anywhere in a name — and
    /// because the case typed may not be the case of the name.
    pub fn accept_offer(&self) -> bool {
        let Some(candidate) = self.with_offer(|ranked, at| ranked[at].candidate.to_owned()) else {
            return false;
        };
        let line = self.line_to_caret();
        let word = completion::word_at_end(&line).len();
        self.edit(|editor| {
            let caret = editor.caret();
            editor.set_selection(Selection::new(caret - word, caret));
            editor.insert(&candidate);
        });
        if completion::closes_word(&candidate) {
            self.close_word();
        }
        self.forget_completions();
        true
    }

    /// Types the space after a finished completion, unless the line already
    /// has one right after the caret. Reports whether it typed one.
    ///
    /// A space already standing there — Tab on a word in the middle of a line
    /// — is the one that would have been typed; a second would leave two.
    fn close_word(&self) -> bool {
        let follows = {
            let editor = self.0.editor.borrow();
            editor.text()[editor.caret()..].chars().next()
        };
        if follows.is_some_and(char::is_whitespace) {
            return false;
        }
        self.edit(|editor| editor.insert(" "));
        true
    }

    /// Selects the candidate at `position` in the whole list and takes it, as
    /// Enter would: a press on its row. Reports whether there was one there.
    pub fn choose_offer(&self, position: usize) -> bool {
        let Some(total) = self.with_offer(|ranked, _| ranked.len()) else {
            return false;
        };
        if position >= total {
            return false;
        }
        let line = self.line_to_caret();
        if let Some(offer) = self.0.completion.borrow_mut().offered.as_mut() {
            offer.at = position;
            completion::word_at_end(&line).clone_into(&mut offer.seen);
        }
        self.accept_offer()
    }

    /// Puts the list away without taking anything from it, reporting whether
    /// it was up: Escape.
    pub fn dismiss_offer(&self) -> bool {
        if !self.has_offer() {
            return false;
        }
        self.forget_completions();
        self.edit(|_| ());
        true
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

    /// Whether the selected candidate is exactly the word already typed.
    fn offer_is_typed(&self) -> bool {
        let line = self.line_to_caret();
        let word = completion::word_at_end(&line);
        self.with_offer(|ranked, at| ranked[at].candidate == word)
            .unwrap_or(false)
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
        self.used();
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
        // A caret the keyboard has just come to starts its blink lit, as one
        // just typed at does: arriving in the dark half of a blink, or at
        // rest after a while unused, it would be a caret somebody has to
        // wait for or look twice to find.
        if has_keys && !self.0.has_keys.get() {
            self.used();
        }
        self.0.has_keys.set(has_keys);
    }

    /// The editor, to read: what a renderer paints from.
    pub fn editor(&self) -> Ref<'_, Editor> {
        self.0.editor.borrow()
    }

    /// Changes the editor, and restarts the caret's blink so that the caret is
    /// solid at the moment somebody is looking for it.
    pub fn edit<T>(&self, change: impl FnOnce(&mut Editor) -> T) -> T {
        self.used();
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
        // While the list is up, the keys every list answers to: Up and Down
        // move through it, and Enter and Right take what is selected. Enter
        // on a word that already *is* the selected candidate has nothing to
        // take, and runs the line as it would have.
        if self.has_offer() {
            match intent {
                Intent::HistoryUp => {
                    self.cycle_completion(false);
                    return None;
                }
                Intent::HistoryDown => {
                    self.cycle_completion(true);
                    return None;
                }
                Intent::Submit if !self.offer_is_typed() => {
                    self.accept_offer();
                    return None;
                }
                Intent::Move(Motion::Right | Motion::LineEnd | Motion::WordRight) => {
                    self.accept_offer();
                    return None;
                }
                _ => {}
            }
        }
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

    /// Inserts PRIMARY at the caret, for a middle press: it arrives when its
    /// owner answers rather than with a keystroke.
    ///
    /// Added to the line, unlike the paste chord's clipboard, and never put
    /// over what is selected in it, as a terminal's middle click — xterm's,
    /// VTE's — pastes at the cursor and leaves the selection be. A GtkEntry
    /// would paste over a selection clicked inside it, and the composer does
    /// not follow it there: the selection is very often where PRIMARY came
    /// from, since selecting in the field is what offers it, and a paste over
    /// it would write the word over itself and look like nothing happened.
    /// See [`Editor::paste_at_caret`].
    pub fn paste_primary(&self, text: &str) {
        self.edit(|editor| editor.paste_at_caret(text));
    }

    /// Throws the half-written line away: what Ctrl-C means, on the field's
    /// side of the same keystroke the shell is interrupted by.
    pub fn abandon(&self) {
        self.edit(Editor::clear);
        self.forget_completions();
    }

    /// Whether the caret is drawn at this instant.
    ///
    /// Always, where carets keep still and once the field has gone
    /// [`CARET_REST`] unused: a caret that holds still holds still lit.
    pub fn caret_is_visible(&self) -> bool {
        let idle = self.0.active_since.get().elapsed();
        if !BLINKS.get() || idle >= CARET_REST {
            return true;
        }
        let phase = CARET_PHASE.as_millis();
        idle.as_millis() % (phase * 2) < phase
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

    /// Ends the gesture, reporting whether there was one, and offers what it
    /// selected as PRIMARY where the desktop keeps one.
    ///
    /// At the end rather than at every step of the drag: PRIMARY is what a
    /// middle click in another program pastes, and a selection is not chosen
    /// until the button comes up. A press that selected nothing offers nothing
    /// — clicking to place the caret is not taking PRIMARY away from whoever
    /// holds it. See [`crate::clipboard`].
    pub fn release(&self, clipboard: &Clipboard) -> bool {
        if self.0.drag.replace(None).is_none() {
            return false;
        }
        if let Some(selected) = self.editor().copy() {
            clipboard.write_primary(&selected);
        }
        true
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
        Complete | CompleteBackwards | AskCompletions => {}
        // Nor this: the element that saw it clears the pane, and the line
        // stays as it was, which is readline's own Ctrl-L.
        ClearBlocks => {}
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
            ..Completions::default()
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
    fn tab_completes_across_a_case_difference_and_corrects_it() {
        // fish's completion ignores case, so a lowercase `c` comes back as
        // `Cargo.`. Typing the tail onto `c` would leave `cargo.`, a name that
        // is not there; the stem is replaced with the shell's own case, and
        // the suggestion that follows reads off the corrected word.
        let input = holding("cargo c");
        answered(&input, &["Cargo.toml", "Cargo.lock"]);

        assert_eq!(input.editor().text(), "cargo Cargo.");
        assert_eq!(input.suggestion().as_deref(), Some("toml"));

        // And a single case-mismatched candidate completes whole.
        let one = holding("read");
        answered(&one, &["README.md"]);
        assert_eq!(one.editor().text(), "README.md ");
    }

    #[test]
    fn a_completion_that_leaves_nothing_to_choose_is_followed_by_a_space() {
        // What every shell's Tab does with a single candidate: the word is
        // done, so the next one can be typed straight away.
        let input = holding("git chec");
        answered(&input, &["checkout"]);
        assert_eq!(input.editor().text(), "git checkout ");

        // A word that was already whole asks whether anything is left, and
        // the answer is the space.
        let whole = holding("cargo build");
        answered(&whole, &["build"]);
        assert_eq!(whole.editor().text(), "cargo build ");
    }

    #[test]
    fn a_name_that_asks_for_more_is_left_open() {
        // A directory goes on into its files, an option into its value, a
        // user into a host: a space there would have to be deleted again.
        for (typed, candidate) in [
            ("cd sr", "src/"),
            ("git log --form", "--format="),
            ("ssh gi", "git@"),
            ("scp serv", "server:"),
        ] {
            let input = holding(typed);
            answered(&input, &[candidate]);
            let line = input.editor().text().to_owned();
            assert!(line.ends_with(candidate), "{typed}: {line:?}");
        }
    }

    #[test]
    fn a_tab_in_the_middle_of_a_line_does_not_double_the_space_after_it() {
        let input = holding("git chec main");
        input.edit(|editor| editor.set_selection(Selection::caret(8)));
        answered(&input, &["checkout"]);
        assert_eq!(input.editor().text(), "git checkout main");
    }

    #[test]
    fn several_candidates_that_still_differ_get_no_space() {
        // `cargo` is typed, but `cargo-fmt` is still a choice: a space would
        // throw it away.
        let input = holding("carg");
        answered(&input, &["cargo", "cargo-fmt"]);
        assert_eq!(input.editor().text(), "cargo");
        assert!(input.has_offer());
    }

    #[test]
    fn an_answer_about_a_line_that_has_since_changed_is_dropped() {
        // Tab on `echo bu`, and before the shell answers the line is typed
        // over. The answer is about `bu` as an argument; typed into the new
        // line it would put `build/` where a command goes.
        let input = holding("echo bu");
        let clipboard = Clipboard::new();
        let serial = input.ask_for_completions();
        input.apply(Intent::SelectAll, &clipboard);
        input.apply(Intent::Insert("bu".to_owned()), &clipboard);

        assert!(!input.take_completions(serial, answer(&["build/"])));
        assert_eq!(input.editor().text(), "bu");

        // A line still as it was asked about takes its answer.
        let serial = input.ask_for_completions();
        assert!(input.take_completions(serial, answer(&["build/"])));
        assert_eq!(input.editor().text(), "build/");
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
        assert_eq!(input.editor().text(), "cargo ");
        assert!(
            !input.cycle_completion(true),
            "the word is whole, and the rest of the answer was about half of it"
        );
    }

    #[test]
    fn enter_over_the_list_takes_the_selection_and_then_runs_the_line() {
        let clipboard = Clipboard::new();
        let input = holding("car");
        answered(&input, &["cargo", "carbon"]);
        input.apply(Intent::HistoryDown, &clipboard);

        assert_eq!(
            input.apply(Intent::Submit, &clipboard),
            None,
            "Enter with a list up chooses from it, the way every list's does"
        );
        assert_eq!(input.editor().text(), "carbon ");
        assert!(!input.has_offer(), "and the list has gone with the choice");
        assert_eq!(
            input.apply(Intent::Submit, &clipboard).as_deref(),
            Some("carbon "),
            "the next Enter runs it"
        );
    }

    #[test]
    fn enter_on_a_word_that_is_already_the_selection_runs_the_line() {
        let clipboard = Clipboard::new();
        let input = holding("ca");
        answered(&input, &["cat", "cat-file", "cargo"]);
        input.edit(|editor| editor.insert("t"));
        assert!(input.has_offer(), "the premise: `cat` still lists two");

        assert_eq!(
            input.apply(Intent::Submit, &clipboard).as_deref(),
            Some("cat"),
            "there is nothing to take, so Enter does what it would have"
        );
        assert_eq!(
            input.suggestion(),
            None,
            "and what was offered is forgotten"
        );
    }

    #[test]
    fn up_and_down_move_through_the_list_and_not_the_history() {
        let clipboard = Clipboard::new();
        let input = holding("car");
        answered(&input, &["cargo", "carbon", "cartridge"]);

        input.apply(Intent::HistoryDown, &clipboard);
        input.apply(Intent::HistoryDown, &clipboard);
        assert_eq!(input.menu().map(|menu| menu.position), Some(2));
        input.apply(Intent::HistoryUp, &clipboard);
        assert_eq!(input.suggestion().as_deref(), Some("bon"));
        assert_eq!(input.editor().text(), "car", "the history was not walked");
    }

    #[test]
    fn typing_filters_the_list_and_selects_its_top_again() {
        let clipboard = Clipboard::new();
        let input = holding("git checkout ");
        answered(
            &input,
            &["main", "feat/completion-menu", "fix/card-pip-fallback"],
        );
        input.apply(Intent::HistoryDown, &clipboard);
        input.apply(Intent::HistoryDown, &clipboard);

        input.edit(|editor| editor.insert("fcp"));
        let menu = input.menu().expect("the letters are in two of the names");
        assert_eq!(menu.items[0].candidate, "fix/card-pip-fallback");
        assert_eq!(
            menu.selected, 0,
            "a new word is a new list, chosen from the top"
        );
        assert_eq!(
            input.suggestion(),
            None,
            "a name found by scattered letters has no tail to draw after them"
        );

        assert!(input.accept_offer());
        assert_eq!(
            input.editor().text(),
            "git checkout fix/card-pip-fallback ",
            "taken in place of the letters, not after them"
        );
    }

    #[test]
    fn escape_puts_the_list_away_and_leaves_the_line() {
        let input = holding("car");
        answered(&input, &["cargo", "carbon"]);

        assert!(input.dismiss_offer());
        assert!(!input.has_offer());
        assert_eq!(input.suggestion(), None);
        assert_eq!(input.editor().text(), "car");
        assert!(
            !input.dismiss_offer(),
            "a second Escape has nothing to put away"
        );
    }

    #[test]
    fn the_list_scrolls_to_keep_the_selection_in_view() {
        let clipboard = Clipboard::new();
        let input = holding("f");
        let names: Vec<String> = (0..20).map(|index| format!("file-{index:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        answered(&input, &names);

        let menu = input.menu().expect("twenty files");
        assert_eq!(
            (menu.items.len(), menu.total, menu.selected),
            (MENU_ROWS, 20, 0)
        );
        for _ in 0..10 {
            input.apply(Intent::HistoryDown, &clipboard);
        }
        let menu = input.menu().expect("still up");
        assert_eq!(menu.position, 10);
        assert_eq!(menu.items[menu.selected].candidate, "file-10");
        assert_eq!(
            menu.selected,
            MENU_ROWS - 1,
            "the selection is the last row in view"
        );
        assert_eq!(menu.word_start, 0);
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

        assert!(input.release(&Clipboard::new()));
        assert!(
            !input.drag_to(2),
            "a drag with no press behind it selects nothing"
        );
    }

    #[test]
    fn the_end_of_a_drag_offers_what_it_selected_as_primary_on_linux() {
        // Offered once the button comes up rather than on every step of the
        // drag: PRIMARY is what a middle click elsewhere pastes, and the
        // selection is not finished being chosen until then.
        let clipboard = Clipboard::new();
        let input = holding("echo hello");
        input.press(5, 1);
        input.drag_to(10);
        assert_eq!(clipboard.primary(), None, "offered before the drag ended");

        assert!(input.release(&clipboard));
        assert_eq!(
            clipboard.primary().as_deref(),
            crate::clipboard::HAS_PRIMARY.then_some("hello")
        );
    }

    #[test]
    fn a_click_that_selects_nothing_leaves_primary_to_whoever_holds_it() {
        let clipboard = Clipboard::new();
        clipboard.select_elsewhere("held by another program");
        let input = holding("echo hello");
        input.press(5, 1);
        assert!(input.release(&clipboard));
        assert_eq!(
            clipboard.primary().as_deref(),
            Some("held by another program")
        );
    }

    #[test]
    fn a_double_click_offers_the_word_it_took() {
        let clipboard = Clipboard::new();
        let input = holding("one two three");
        input.press(5, 2);
        assert!(input.release(&clipboard));
        assert_eq!(
            clipboard.primary().as_deref(),
            crate::clipboard::HAS_PRIMARY.then_some("two")
        );
    }

    #[test]
    fn a_middle_click_after_a_double_click_pastes_the_word_a_second_time() {
        // The word is selected and it is PRIMARY, so pasting PRIMARY over the
        // selection wrote it over itself and nothing visibly happened. The
        // composer follows the terminal — xterm and VTE paste at the cursor
        // and leave the selection — rather than GtkEntry, which replaces a
        // selection clicked inside.
        let input = holding("ls foo");
        input.press(4, 2);
        assert!(input.release(&Clipboard::new()));
        assert_eq!(input.editor().selected_text(), "foo");

        input.paste_primary("foo");
        assert_eq!(input.editor().text(), "ls foofoo");
        assert!(input.editor().selection().is_empty());
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

    /// A little way into a half of the blink, so that a test asking about it
    /// at once is still in that half on a loaded machine.
    const INTO_A_HALF: Duration = Duration::from_millis(100);

    #[test]
    fn a_caret_goes_dark_half_a_blink_after_the_field_was_used() {
        // What every test below departs from: the caret does blink.
        let input = holding("x");
        input.backdate(CARET_PHASE + INTO_A_HALF);
        assert!(!input.caret_is_visible(), "the caret never went out");
        assert!(a_caret_blinks());
    }

    #[test]
    fn a_caret_left_alone_holds_still_lit_and_stops_asking_for_frames() {
        let input = holding("x");
        // Ten seconds and a little: the dark half of a blink, had it gone on.
        input.backdate(CARET_REST + INTO_A_HALF);
        assert!(
            input.caret_is_visible(),
            "a caret nobody has used for ten seconds went on blinking"
        );
        assert!(
            !a_caret_blinks(),
            "the blink chain would go on repainting a caret at rest"
        );

        input.edit(|editor| editor.set_text("xy"));
        assert!(a_caret_blinks(), "typing again did not start the blink");
    }

    #[test]
    fn a_caret_that_may_not_blink_is_solid() {
        let input = holding("x");
        input.backdate(CARET_PHASE + INTO_A_HALF);
        let_carets_blink(false);
        let (visible, blinks) = (input.caret_is_visible(), a_caret_blinks());
        let_carets_blink(true);

        assert!(visible, "a caret asked to keep still went dark");
        assert!(
            !blinks,
            "the blink chain would repaint a caret that keeps still"
        );
    }

    #[test]
    fn a_field_the_keyboard_comes_to_starts_its_blink_lit() {
        // A caret that arrived in the dark half of a blink, or at rest after
        // a long while unused, is a caret a person has to wait to find.
        let input = holding("x");
        input.backdate(CARET_PHASE + INTO_A_HALF);
        input.set_has_keys(true);
        assert!(input.caret_is_visible());

        // Told again, it is not news, and the blink goes on where it was.
        input.backdate(CARET_PHASE + INTO_A_HALF);
        input.set_has_keys(true);
        assert!(!input.caret_is_visible());
    }
}
