//! The prose on a settings page, folded behind "More" where it runs long.
//!
//! # Why fold at all
//!
//! Nearly every row on a page has a line under its label, and the owner's
//! sentences under them are the setting as much as the switch is — a switch
//! called "login shell" means nothing without them. But a page of rows each
//! carrying three to five lines of muted text is a page whose labels cannot
//! be found by eye, and the "How it works" paragraphs at the foot of a page
//! were longer than the page above them. So the words stay exactly as they
//! were written, and only how many of them are on screen at once changes:
//! a description keeps the whole sentences that fit in about two lines, and
//! the rest waits behind an inline "More" that opens it in place.
//!
//! The decision is made on the text rather than on its layout. Whether a
//! paragraph wraps onto a third line is known only once it has been laid out,
//! which is after the element that would draw the button has been built, and
//! a button that appeared and vanished as the window was resized would move
//! the rows under it. [`FOLD_AT`] is two lines at the page's own measure,
//! which is the width almost every page is read at.
//!
//! # Why it does not lose a search
//!
//! The search already matches a row's whole description, folded or not. What
//! it must not do is answer with a row whose matching words are out of sight:
//! while a query is typed, a fold whose hidden part holds one of its terms is
//! drawn open, and drawn without a "Less", since pressing it could not close
//! what the query is holding open.
//!
//! # How a widget finds out
//!
//! The way [`super::focus`] does: through a slot that
//! [`super::render`] fills for exactly as long as it builds the pages. The
//! widgets that draw a description are free functions handed a row's words
//! and nothing that knows about a window, and threading one more argument
//! through every page in every plugin would be forty call sites saying the
//! same thing. Outside that slot — a plugin's card is drawn from the same
//! widgets — nothing is folded, which is what those surfaces drew before.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crookui_core::elements::MouseStateHandle;
use crookui_core::fonts::{Properties, Weight};
use crookui_core::prelude::*;

use super::focus::{self, Keys, Stop};
use super::search::Query;
use super::widgets::DESCRIPTION_SIZE;
use crate::theme::theme;
use crate::workspace::action::{SettingsAction, WorkspaceAction};

/// How many characters of prose are shown before the rest is folded away.
///
/// Two lines of a description at the page's measure: the page is 560 wide, a
/// description stops 96 short of its edge, and the interface face at 11px
/// advances about 5.4 a character — 86 to the line, so 170 is two lines with
/// a word's slack for where they break.
pub(crate) const FOLD_AT: usize = 170;

/// A piece of prose split where it folds: what is always shown, and what
/// waits behind "More".
///
/// The shown part is every whole sentence from the start that fits in
/// [`FOLD_AT`], and never less than the first sentence however long it is —
/// a sentence cut in the middle would say something its writer did not. A
/// text that fits, or that is one sentence, does not fold at all.
pub(crate) fn fold(text: &str) -> (&str, Option<&str>) {
    if text.chars().count() <= FOLD_AT {
        return (text, None);
    }
    let mut cut = None;
    for end in sentence_ends(text) {
        if cut.is_some() && text[..end].chars().count() > FOLD_AT {
            break;
        }
        cut = Some(end);
    }
    match cut {
        Some(end) if end < text.len() => (&text[..end], Some(text[end..].trim_start())),
        _ => (text, None),
    }
}

/// The byte offsets just past each sentence's closing mark, in order.
///
/// A full stop, a question mark or an exclamation mark, then a space, then
/// something a sentence starts with: a capital, a digit or an opening quote.
/// That leaves alone the dots inside a version, a path or `~/.zprofile`,
/// which are what the prose on these pages is full of.
fn sentence_ends(text: &str) -> impl Iterator<Item = usize> + '_ {
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    (0..characters.len()).filter_map(move |at| {
        let (offset, mark) = characters[at];
        let space = characters.get(at + 1).map(|(_, c)| *c);
        let next = characters.get(at + 2).map(|(_, c)| *c);
        let starts =
            |c: char| c.is_uppercase() || c.is_ascii_digit() || "\"\u{201c}\u{2018}".contains(c);
        (matches!(mark, '.' | '?' | '!') && space == Some(' ') && next.is_some_and(starts))
            .then_some(offset + mark.len_utf8())
    })
}

/// A name for a fold that fits in a `Copy` action.
///
/// [`WorkspaceAction`] is `Copy`, so a fold is named by a hash of
/// its words rather than by the words. Two rows with the same words share a
/// fold, which is the right answer: they say the same thing.
pub(crate) fn id_of(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Which folds are open, and the mouse state of each one's button, for as
/// long as the window is open.
///
/// Kept beside the page's other state rather than on disk: which paragraph
/// somebody opened is where they were reading, not a preference, and a page
/// they come back to in the same session is the page they left.
#[derive(Default)]
pub(crate) struct Prose {
    /// The folds somebody opened.
    open: RefCell<HashSet<u64>>,
    /// The buttons' mouse states, by the fold they open: see
    /// [`SettingsState::control`](super::SettingsState::control) for why a
    /// handle is made once and kept.
    controls: RefCell<HashMap<u64, MouseStateHandle>>,
}

impl Prose {
    /// Opens a fold, or closes an open one.
    pub(crate) fn toggle(&self, id: u64) {
        let mut open = self.open.borrow_mut();
        if !open.remove(&id) {
            open.insert(id);
        }
    }

    /// Forgets every hover: the same reset
    /// [`SettingsState::forget_hover_state`](super::SettingsState) gives the
    /// page's other controls when it closes.
    pub(crate) fn forget_hover_state(&self) {
        for state in self.controls.borrow().values() {
            state.lock().reset_interaction_state();
        }
    }

    /// The mouse state of the button of the fold named `id`.
    fn control(&self, id: u64) -> MouseStateHandle {
        self.controls.borrow_mut().entry(id).or_default().clone()
    }
}

/// What the widgets are told while a page is being built.
struct Reading {
    prose: Rc<Prose>,
    query: Query,
}

thread_local! {
    /// Filled by [`reading`] while the pages are built; see the module.
    static READING: RefCell<Option<Reading>> = const { RefCell::new(None) };
}

/// Runs `build` with the page's folds and query where its widgets can see
/// them, putting back whatever was there before.
pub(crate) fn reading<T>(prose: Rc<Prose>, query: &Query, build: impl FnOnce() -> T) -> T {
    let reading = Reading {
        prose,
        query: query.clone(),
    };
    let before = READING.with_borrow_mut(|slot| slot.replace(reading));
    let built = build();
    READING.with_borrow_mut(|slot| *slot = before);
    built
}

/// The prose under a row or in a note, folded where a page is being read.
///
/// `draw` sets the words in whatever a caller's prose is set in — a row's
/// description, a note — so the fold is the one shape for both.
pub(crate) fn folded(
    text: &str,
    ui: crookui_core::fonts::FamilyId,
    draw: impl Fn(String) -> Box<dyn Element>,
) -> Box<dyn Element> {
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    READING.with_borrow(|reading| {
        let Some(reading) = reading else {
            column.add_child(draw(text.to_owned()));
            return;
        };
        let (shown, hidden) = fold(text);
        let id = id_of(text);
        let found = hidden.is_some_and(|hidden| reading.query.finds_in(hidden));
        let open = hidden.is_none() || found || reading.prose.open.borrow().contains(&id);

        column.add_child(draw(if open { text } else { shown }.to_owned()));
        if hidden.is_some() && !found {
            column.add_child(more(open, id, reading.prose.control(id), ui));
        }
    });
    column.finish()
}

/// The inline button that opens a fold, or closes it.
///
/// A word and a chevron in the label's colour rather than the description's,
/// so it reads as the one thing in the paragraph that does something; the
/// accent on hover is the colour a link is everywhere. It is a stop of its
/// own, so Enter and Space open it from the keyboard.
fn more(
    open: bool,
    id: u64,
    state: MouseStateHandle,
    ui: crookui_core::fonts::FamilyId,
) -> Box<dyn Element> {
    let action: WorkspaceAction = SettingsAction::ToggleProse(id).into();
    let stop = state.clone();
    let control = Hoverable::new(state, move |mouse| {
        let color = if mouse.is_hovered() {
            theme().accent
        } else {
            theme().text_primary
        };
        Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new(if open { "Less" } else { "More" }, ui, DESCRIPTION_SIZE)
                    .with_color(color)
                    .with_style(Properties {
                        weight: Weight::Semibold,
                        ..Properties::default()
                    })
                    .finish(),
            )
            .with_child(
                Container::new(
                    Icon::new(
                        if open {
                            Lucide::ChevronUp
                        } else {
                            Lucide::ChevronDown
                        },
                        DESCRIPTION_SIZE,
                    )
                    .with_color(color)
                    .finish(),
                )
                .with_margin_left(2.)
                .finish(),
            )
            .finish()
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(action))
    .finish();

    Container::new(
        Align::new(Stop::control(
            &stop,
            Keys::press(Some(action)),
            focus::ring_radius(2.),
            control,
        ))
        .left()
        .finish(),
    )
    // Enough air that the focus ring, drawn outside the word, clears the
    // last line of the paragraph above it.
    .with_margin_top(MORE_GAP)
    .finish()
}

/// The gap between a folded paragraph and its "More".
const MORE_GAP: f32 = 6.;

#[cfg(test)]
#[path = "prose_tests.rs"]
mod tests;
