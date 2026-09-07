//! Recording a chord, and writing it down.
//!
//! The page beside this file has printed keybindings and refused to change one
//! since it was written, and its own doc said why: "recording a chord needs a
//! control that takes over the keyboard and the settings page has no text
//! input and no popup". Both halves of that are now things the application
//! has — a plugin may claim every keystroke while a surface of its own is up,
//! and `window.overlay` is where a surface goes — so the sentence is no longer
//! true and this is what it was standing in for.
//!
//! # It takes the whole keyboard, on purpose
//!
//! A recorder that let chords through would be a recorder that cannot record
//! them: `cmd-w` is the thing being bound and also the thing that closes the
//! pane. So while it is up every keystroke is claimed — before the bindings,
//! which is what [`Host::claim_surface`](crate::plugin::Host::claim_surface)
//! is — and exactly one of them is not recorded: Escape on its own, which is
//! the way out. Anybody who wants Escape *bound* to something can write it in
//! the file by hand, which is the same escape hatch every editor with this
//! control has.
//!
//! # What it writes
//!
//! Two lines, appended: a removal of every chord the command has, and the new
//! chord. Not a rewrite of the file — a file somebody keeps comments in is a
//! file a tool has no business reformatting, and the two-line form says
//! exactly what happened in the vocabulary
//! [`keybindings`](crate::keybindings) already documents. The last matching
//! rule wins, so what a person reads at the bottom of their own file is what
//! their keyboard does.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use crookui_core::elements::Padding;
use crookui_core::event::Keystroke;
use crookui_core::prelude::*;

use crook_plugin::ActionName;

use crate::keybindings;
use crate::plugin::Showing;
use crate::settings;
use crate::theme::theme;
use crate::workspace::Fonts;

/// How wide the card is. The palette's, less the room a list needs.
const CARD_WIDTH: f32 = 420.;

/// How far down the window it starts, which is the palette's too: a surface
/// that grows downwards from a fixed point does not move under the pointer.
const TOP: f32 = 140.;

/// The inset inside the card.
const PADDING: f32 = 16.;

/// The type sizes on it.
const TITLE_SIZE: f32 = 14.;
const BODY_SIZE: f32 = 12.;
const CHORD_SIZE: f32 = 20.;

/// The names of the three actions the keyboard resolves to while it is up.
pub(super) struct Names {
    /// A chord was pressed.
    pub(super) record: ActionName,
    /// Escape was.
    pub(super) cancel: ActionName,
    /// Something that is not a chord was: a key that is only a modifier.
    ///
    /// It does nothing, and it exists so that the keystroke is *claimed*
    /// rather than let through — a bare `shift` on its way to `shift-cmd-p`
    /// must not reach the shell underneath.
    pub(super) ignore: ActionName,
}

/// The keys that are only a modifier, which are not a chord on their own.
///
/// A person reaching for `ctrl-shift-k` presses three keys, and the first two
/// of them are not what they are binding.
const MODIFIERS: [&str; 8] = [
    "shift", "ctrl", "control", "alt", "option", "cmd", "super", "meta",
];

/// The recorder.
pub(super) struct Recorder {
    open: Cell<bool>,
    showing: RefCell<Option<Showing>>,
    names: Names,
    /// The command being rebound, and what a person calls it.
    subject: RefCell<Option<(ActionName, String)>>,
    /// The chord last pressed, waiting for the action that writes it down.
    pressed: RefCell<Option<Keystroke>>,
    /// What went wrong with the last attempt to write, if anything did.
    problem: RefCell<Option<String>>,
    fonts: Fonts,
}

impl Recorder {
    /// A recorder that is not up.
    pub(super) fn new(names: Names, fonts: Fonts) -> Self {
        Self {
            open: Cell::new(false),
            showing: RefCell::new(None),
            names,
            subject: RefCell::new(None),
            pressed: RefCell::new(None),
            problem: RefCell::new(None),
            fonts,
        }
    }

    /// Hands it the flag the workspace reads.
    pub(super) fn armed_by(&self, showing: Showing) {
        *self.showing.borrow_mut() = Some(showing);
    }

    /// Puts it up for one command.
    pub(super) fn open(&self, command: ActionName, title: String) {
        *self.subject.borrow_mut() = Some((command, title));
        self.problem.replace(None);
        self.pressed.replace(None);
        self.open.set(true);
        if let Some(showing) = self.showing.borrow().as_ref() {
            showing.set(true);
        }
    }

    /// Takes it down.
    pub(super) fn close(&self) {
        self.open.set(false);
        self.pressed.replace(None);
        if let Some(showing) = self.showing.borrow().as_ref() {
            showing.set(false);
        }
    }

    /// Whether it is up.
    pub(super) fn is_open(&self) -> bool {
        self.open.get()
    }

    /// What this keystroke means while it is up.
    ///
    /// Recording happens *here*, in the claim, which is the one place every
    /// keystroke passes through before anything else in the window sees it.
    /// The action it names then writes down what was recorded, so the keys and
    /// the writing stay on the same path every other binding is on.
    pub(super) fn claims(&self, keystroke: &Keystroke) -> Option<ActionName> {
        if !self.open.get() {
            return None;
        }
        if keystroke.key == "escape" && keystroke.modifiers.is_empty() {
            return Some(self.names.cancel.clone());
        }
        if MODIFIERS.contains(&keystroke.key.as_str()) {
            // Not a chord yet: somebody is still reaching for the key they
            // mean. Claimed all the same — returning `None` here would let a
            // bare modifier fall through to the pane underneath.
            return Some(self.names.ignore.clone());
        }
        self.pressed.replace(Some(keystroke.clone()));
        Some(self.names.record.clone())
    }

    /// The command being rebound and the chord that was pressed for it.
    pub(super) fn recorded(&self) -> Option<(ActionName, String)> {
        let keystroke = self.pressed.borrow().clone()?;
        let (command, _) = self.subject.borrow().clone()?;
        Some((command, keybindings::format_chord(&keystroke)))
    }

    /// Says why the last attempt did not work, and stays up saying it.
    pub(super) fn failed(&self, why: String) {
        self.problem.replace(Some(why));
        self.pressed.replace(None);
    }
}

/// The card, or nothing at all while the recorder is down.
pub(super) fn render(recorder: &Rc<Recorder>) -> Box<dyn Element> {
    if !recorder.is_open() {
        return Empty::new().finish();
    }
    let ui = recorder.fonts.ui;
    let Some((command, title)) = recorder.subject.borrow().clone() else {
        return Empty::new().finish();
    };

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(
            Text::new(format!("Change the keys for {title}"), ui, TITLE_SIZE)
                .with_color(theme().text_primary)
                .finish(),
        )
        .with_child(
            Container::new(
                Text::new(command.to_string(), ui, BODY_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            )
            .with_margin_top(6.)
            .finish(),
        )
        .with_child(
            Container::new(
                Text::new("Press the keys you want.", ui, CHORD_SIZE)
                    .with_color(theme().accent)
                    .finish(),
            )
            .with_margin_top(PADDING)
            .finish(),
        );

    if let Some(problem) = recorder.problem.borrow().as_ref() {
        column.add_child(
            Container::new(
                Text::new(problem.clone(), ui, BODY_SIZE)
                    .with_color(theme().usage_critical)
                    .finish(),
            )
            .with_margin_top(PADDING)
            .finish(),
        );
    }

    column.add_child(
        Container::new(
            Text::new("Escape to leave it as it is", ui, BODY_SIZE)
                .with_color(theme().text_muted)
                .finish(),
        )
        .with_margin_top(PADDING)
        .finish(),
    );

    let card = ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme().surface)
            .with_border(Border::all(1.).with_border_color(theme().overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
            .with_padding(Padding {
                top: PADDING,
                bottom: PADDING,
                left: PADDING,
                right: PADDING,
            })
            .finish(),
    )
    .with_width(CARD_WIDTH)
    .finish();

    // Centred by the element tree rather than by the anchor, for the reason
    // the palette says: an anchor's offset is a constant and a window's width
    // is not.
    let centred = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(card)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .finish();

    Container::new(centred).with_margin_top(TOP).finish()
}

/// Binds `chord` to `command` in the person's own keybindings file, and says
/// where it wrote it.
///
/// Appended rather than rewritten: see this module's own doc. The removal
/// before it is what makes this a *change* rather than a second chord for the
/// same command.
pub(super) fn bind(command: &ActionName, chord: &str) -> Result<PathBuf, String> {
    let path = keybindings::user_keybindings_path()
        .ok_or_else(|| String::from("there is no configuration directory to write into"))?;

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let text = append(&existing, command, chord)?;

    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|why| why.to_string())?;
    }
    settings::atomic_write(&path, text.as_bytes()).map_err(|why| why.to_string())?;
    Ok(path)
}

/// The file with the two lines added, or a reason it cannot have them.
///
/// The insertion point is the last `]` in the file, which is the end of the
/// list every keybindings file is. A file that is not one — no `]`, or a
/// document that does not parse — is left alone and reported: a tool that
/// "fixed" somebody's file by replacing it would be a tool nobody leaves
/// comments in.
fn append(existing: &str, command: &ActionName, chord: &str) -> Result<String, String> {
    let lines = format!(
        "  {{ \"command\": \"-{command}\" }},\n  {{ \"key\": \"{chord}\", \"command\": \"{command}\" }}\n"
    );

    let trimmed = existing.trim();
    if trimmed.is_empty() {
        return Ok(format!("[\n{lines}]\n"));
    }

    let closed = trimmed
        .rfind(']')
        .ok_or_else(|| String::from("the keybindings file is not a list"))?;
    let (before, after) = trimmed.split_at(closed);
    // A list with something already in it needs a comma; one that is only
    // `[` and `]` — or `[` and a comment — does not.
    let separator = if before
        .trim_start()
        .trim_start_matches('[')
        .trim()
        .is_empty()
    {
        ""
    } else {
        ","
    };

    Ok(format!(
        "{}{separator}\n{lines}{}\n",
        before.trim_end(),
        after
    ))
}

#[cfg(test)]
#[path = "recorder_tests.rs"]
mod tests;
