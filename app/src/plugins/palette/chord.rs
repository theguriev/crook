//! Every distinct key that reaches a command, out of every rule that does.
//!
//! # A list of keys is not a list of rules
//!
//! A [`Keystroke`]'s key is the character the platform *reports*, with Shift
//! already applied to it. So a chord table that wants `ctrl+shift+]` to fire
//! has to name `ctrl+shift+}` as well, and the zoom answers to four spellings
//! of one physical key on Linux and six on macOS. Those extra lines are a
//! workaround for what the platform reports, not four ways to zoom in; a
//! surface that printed one cap per rule would be printing the workaround.
//!
//! The fold is a **display** decision and it lives here rather than in
//! [`Keybindings::chords_for`](crate::keybindings::Keybindings::chords_for),
//! because the two surfaces that print chords are answering different
//! questions. The Keyboard Shortcuts page prints every rule: its job is to
//! explain *why* a key does what it does, and a rule that is genuinely in
//! force must stay visible in an audit view — collapsing there would delete
//! one. The palette prints every key a person can press, because its job is to
//! tell them *which one*.
//!
//! Nothing here changes what a key does, and nothing here is a keymap change
//! wearing a display change's clothes: [`Keybindings::effective`] still
//! returns what it returned, and the rules this folds together are all still
//! in force.
//!
//! [`Keybindings::effective`]: crate::keybindings::Keybindings::effective

use crookui_core::event::{Keystroke, Modifiers};

use crate::keybindings::{self, Rule};

use super::rows::Chord;

/// What a rule's key sequence reduces to, once what the platform spelled into
/// it has been taken back out: the thing two rules have to agree on to be two
/// spellings of one key. See [`bare`].
type Pressed = Vec<(Modifiers, String)>;

/// Every distinct key that reaches a command, out of every rule that does.
///
/// `rules` are the rules naming one command, in consult order — weakest layer
/// first — so the key this build ships comes out before a key somebody added,
/// and a key keeps the place its first spelling had.
///
/// Two rules are one key when the sequences they reduce to are equal; see
/// [`bare`] for what "reduce" leaves out. Which of a key's two spellings goes
/// on the cap is ranked, not taken from the file's order alone: the character
/// printed on the keycap, then the fewest modifiers, then the earliest line.
/// On a layout where that key does not shift-change, the spelling this keeps
/// is not the one that fires — both are bound, neither is knowably the answer,
/// and the settings page prints both either way.
pub(super) fn collapse(rules: &[&Rule]) -> Vec<Chord> {
    let mut keys: Vec<(Pressed, Vec<&Rule>)> = Vec::new();

    for rule in rules {
        let pressed: Pressed = rule.keys.iter().map(bare).collect();
        match keys.iter_mut().find(|(known, _)| *known == pressed) {
            Some((_, spellings)) => spellings.push(*rule),
            None => keys.push((pressed, vec![*rule])),
        }
    }

    keys.iter().map(|(_, spellings)| key(spellings)).collect()
}

/// The physical key a keystroke is, with what the platform spelled into it
/// taken back out.
///
/// Shift comes off only on a key that prints two characters: that Shift is
/// already spelled into the key the platform reports, and is not a Shift a
/// person also holds. On `a` or `pageup` it is a chord of its own, and
/// `shift+pageup` is not `pageup`.
fn bare(keystroke: &Keystroke) -> (Modifiers, String) {
    let mut modifiers = keystroke.modifiers;
    if keybindings::has_twin(&keystroke.key) {
        modifiers.shift = false;
    }
    (modifiers, keybindings::unshifted(&keystroke.key).to_owned())
}

/// One key, as the row prints it: the spelling kept, and its clause.
///
/// `spellings` are the rules that all reduce to this key, in consult order and
/// never empty.
fn key(spellings: &[&Rule]) -> Chord {
    // `min_by_key` returns the first of equal ranks, which is what makes the
    // position in the file the last tiebreak without its being a term.
    let kept = spellings
        .iter()
        .min_by_key(|rule| (shifted(rule), held(rule)))
        .expect("a key is only made by putting a rule into it");

    // `None` the moment *any* rule spelling this key is unconditional: the
    // last rule that *applies* is the one that wins, so an unconditional
    // fallback under a conditional override still fires, and a row saying
    // "when panelOpen" over a key that always works would be a lie. Otherwise
    // it is the strongest member's, which is the rule in force when its clause
    // holds.
    let only_when = if spellings.iter().any(|rule| rule.when.is_none()) {
        None
    } else {
        spellings
            .last()
            .and_then(|rule| rule.when.as_ref())
            .map(|clause| clause.names_once().join(", "))
    };

    Chord {
        text: kept.chord(),
        only_when,
    }
}

/// How many of a rule's keystrokes name the shifted half of a two-character
/// key, which is how many of them are not what is printed on the keycap.
fn shifted(rule: &Rule) -> usize {
    rule.keys
        .iter()
        .filter(|keystroke| keybindings::unshifted(&keystroke.key) != keystroke.key)
        .count()
}

/// How many modifiers a rule's whole sequence holds down.
fn held(rule: &Rule) -> usize {
    rule.keys
        .iter()
        .map(|keystroke| {
            let modifiers = keystroke.modifiers;
            usize::from(modifiers.ctrl)
                + usize::from(modifiers.shift)
                + usize::from(modifiers.alt)
                + usize::from(modifiers.cmd)
        })
        .sum()
}
