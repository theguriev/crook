//! Where a keystroke stops being the input field's and becomes the shell's.
//!
//! [`crate::terminal_keys`] is the other half of this line: it says what a key
//! *encodes* as once it has been decided that the shell should have it. This
//! module makes that decision, and it makes it in one place — [`route`] — so
//! that the whole policy can be read at once.
//!
//! Crook's own chords are not here. They are keybindings like any other, in
//! [`crate::keybindings`], where a person's own file can reach them. The
//! danger that used to keep them in this file is still real — a chord the
//! window consumes never reaches [`route`], so two tables can quietly take the
//! same keys away from each other — and it is answered by a test rather than
//! by proximity: `no_shipped_binding_takes_a_chord_the_input_field_needs`
//! routes every shipped binding through this module and insists the field has
//! no use for it.
//!
//! # The platform split
//!
//! Everything below the routing rule is a keymap, and a keymap is where the
//! platforms genuinely differ:
//!
//! * **macOS** takes its bindings from Cocoa. Word movement is Alt-Left and
//!   Alt-Right, the ends of a line are Cmd-Left and Cmd-Right, the ends of the
//!   text are Cmd-Up and Cmd-Down, and the clipboard is on Command. Control is
//!   left to the emacs bindings every macOS text field also has — Ctrl-A,
//!   Ctrl-E, Ctrl-K — which are the ones a terminal person already has in
//!   their fingers. Crook's own chords are Command's, which is where a macOS
//!   application's chords live and where nothing the field wants can be.
//! * **Everywhere else** word movement is Ctrl-Left and Ctrl-Right, the ends
//!   of a line are Home and End, and the ends of the text are Ctrl-Home and
//!   Ctrl-End. Crook's own chords are Ctrl-*Shift*, because plain Ctrl-letter
//!   belongs to the tty: Ctrl-C interrupts, Ctrl-D ends input and Ctrl-W
//!   erases a word, and an application that took them would be an application
//!   nobody could run a program in. Every terminal emulator on Linux arrived
//!   at the same arrangement.
//!
//! The one place the keymap is decided by something other than convention is
//! Ctrl-C and Ctrl-Z off macOS. They are the signal keys — see [`route`] — so
//! the clipboard cannot have them, and copy and undo take the Shift variants.
//!
//! The platform is a *parameter* rather than a `cfg!` inside the mapping,
//! because a keymap that can only be tested on the machine it was written on
//! is a keymap with one half untested.

use crookui_core::event::{Keystroke, Modifiers};

use crate::editor::Motion;

/// Which keymap to read a keystroke against.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Platform {
    /// macOS, where the command key carries the application chords.
    Mac,
    /// Linux and Windows, where Control-Shift does.
    Other,
}

impl Platform {
    /// The keymap this build is for.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

/// What the pane a keystroke arrived at is doing.
///
/// Everything [`route`] needs to know beyond the keystroke itself, which is
/// two facts: who owns the screen, and whether there is a line being composed
/// at all.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    /// Whether the program on the far end has taken the whole screen.
    pub alt_screen: bool,
    /// Whether the field is empty. What makes Ctrl-D an end of input rather
    /// than a delete — see [`route`].
    pub line_is_empty: bool,
    /// Whether there is a selection in the output above the field. What makes
    /// the copy chord mean the *screen* rather than the line — rule 1 of
    /// [`route`], and the only rule that can take a key away from the shell.
    pub grid_has_selection: bool,
}

/// What the input field should do about a keystroke.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    /// Type this text at the caret.
    Insert(String),
    /// Break the line without sending it: a multi-line command.
    Newline,
    /// Send the line to the shell.
    Submit,
    /// Delete backwards by a grapheme cluster.
    Backspace,
    /// Delete forwards by a grapheme cluster.
    DeleteForward,
    /// Delete back to the start of the previous word.
    DeleteWordLeft,
    /// Delete forward to the end of the next word.
    DeleteWordRight,
    /// Delete back to the start of the line.
    DeleteToLineStart,
    /// Delete forward to the end of the line.
    DeleteToLineEnd,
    /// Move the caret, dropping any selection.
    Move(Motion),
    /// Move the head of the selection, leaving its anchor.
    Extend(Motion),
    /// The bare Up key: a line up, or the previous command.
    HistoryUp,
    /// The bare Down key: a line down, or the next command.
    HistoryDown,
    /// Select the whole line.
    SelectAll,
    /// Copy the selection to the system clipboard.
    Copy,
    /// Cut the selection to the system clipboard.
    Cut,
    /// Insert what is on the system clipboard.
    Paste,
    /// Step back one edit.
    Undo,
    /// Step forward one undone edit.
    Redo,
    /// Ask the shell what the word before the caret could become, or step to
    /// the next candidate it already offered.
    ///
    /// Not an edit: nothing changes until the shell answers, and the answer
    /// arrives frames later on a channel of its own. See
    /// [`crate::completion`].
    Complete,
    /// Step *back* through the candidates: Shift-Tab.
    ///
    /// It asks nothing. There has to be an answer to step through, and a
    /// Shift-Tab with none is a keystroke about a list that was never asked
    /// for.
    CompleteBackwards,
}

/// Where a keystroke goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Straight to the pty, encoded by [`crate::terminal_keys`].
    Raw,
    /// To the pty, *and* the half-written line goes with it: Ctrl-C.
    ///
    /// The one gesture whose entire meaning is "forget this". The shell prints
    /// `^C` and a fresh prompt, and a field still holding the abandoned line
    /// above that prompt would be the interrupt only pretending to have
    /// worked.
    Interrupt,
    /// To the input field's editor.
    Edit(Intent),
    /// Copy what is selected in the output, and let go of it.
    ///
    /// The grid's, not the field's: there are two selections on screen and
    /// this is the one that outranks the other. See rule 1 of [`route`].
    CopyOutput,
    /// Nowhere. Nothing is typed and nothing is sent.
    Ignored,
}

impl Route {
    /// Whether the pty gets this keystroke.
    pub fn reaches_the_shell(&self) -> bool {
        matches!(self, Self::Raw | Self::Interrupt)
    }
}

/// One of the window's own commands: something it does before any pane sees
/// the keystroke.
///
/// The chord that reaches one is not here — see [`crate::keybindings`], where
/// a person's file can move it. This is the *set*, which is closed because
/// each of these is a thing the workspace itself knows how to do;
/// [`crate::plugins::window`] puts a name and a title on every one of them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// Open a tab.
    NewTab,
    /// Close the focused pane, and its tab with the last one.
    ClosePane,
    /// Split the focused pane to the right.
    SplitRight,
    /// Split the focused pane downwards.
    SplitDown,
    /// Select the tab before the active one.
    PreviousTab,
    /// Select the tab after the active one.
    NextTab,
    /// Move the active tab one place towards the start.
    MoveTabLeft,
    /// Move the active tab one place towards the end.
    MoveTabRight,
    /// Put the keyboard in the search box above the tabs.
    SearchTabs,
    /// Open the settings page.
    OpenSettings,
    /// Make the terminal's text bigger.
    ZoomIn,
    /// Make it smaller.
    ZoomOut,
    /// Put it back to the size a fresh install opens at.
    ZoomReset,
}

/// **The whole keyboard policy of a pane, in one function.**
///
/// 0. Crook's own bindings never reach here. `Workspace::action_for` consumes
///    what the keybindings name, in the window delegate, before any element
///    sees the event — which is what makes `cmd-t` open a tab everywhere rather
///    than typing a `t`.
/// 1. **A selection in the output owns the copy chord while it exists.** There
///    are two selections on a pane — the one dragged out of the shell's output
///    and the one in the field below it — and only one `cmd-c`. The output's
///    wins, which is Warp's rule and the one a person expects: they have just
///    dragged a highlight across something and pressed copy, and the invisible
///    empty selection in a field they were not looking at is not what they
///    meant.
///
///    Off macOS that chord is `ctrl-c`, which is also SIGINT, and **that
///    collision is settled by the selection existing rather than by the key**:
///    with nothing selected `ctrl-c` interrupts, exactly as it always has, so
///    a runaway command is never more than one keystroke from being stopped.
///    With something selected it copies — and *releases* the selection, so the
///    very next `ctrl-c` interrupts. That release is the whole safety of this
///    rule, and it is also the only feedback a copy has: the highlight going
///    away is how a person knows it happened. The other three clearing rules —
///    typing, clicking elsewhere, closing the pane — are the element's, in
///    [`crate::workspace::terminal_element`].
///
///    A modal menu is the one thing that suspends this: while one is up the
///    pane reports no selection at all, so the three keys a running command
///    has to keep hearing still mean what they always mean.
/// 2. **The alt screen belongs to the program.** vim, `top` and `less` drive
///    every cell of the screen and read every key themselves, so on the alt
///    screen everything goes raw to the pty — and the input field is not even
///    drawn. See [`crate::pane_surface`], which is where the visible half of
///    this rule lives. Rule 1 is deliberately above this one: text
///    on a full-screen program's screen is still text somebody selected with
///    the mouse, and there is no reason they cannot copy it.
/// 3. **The signal keys always reach the shell.** Ctrl-C interrupts and
///    abandons the line with it, Ctrl-Z suspends, and Ctrl-D ends the input —
///    but only on an empty line, exactly as it does in a shell. The field
///    holds the line the shell's own reader used to hold, so a Ctrl-D typed
///    out of habit over a half-written command has to be the delete it is in
///    every line editor rather than an end of file that closes the pane.
/// 4. Everything else on the normal screen belongs to the input field, and a
///    keystroke the keymap has no meaning for does nothing at all rather than
///    leaking into the shell.
pub fn route(keystroke: &Keystroke, chars: &str, pane: Pane, platform: Platform) -> Route {
    if pane.grid_has_selection && copies_the_output(keystroke, platform) {
        return Route::CopyOutput;
    }
    if pane.alt_screen {
        return Route::Raw;
    }
    match signal(keystroke) {
        Some(Signal::Interrupt) => return Route::Interrupt,
        Some(Signal::Suspend) => return Route::Raw,
        // The one signal the field can outrank, and only by holding a line for
        // it to delete a character out of.
        Some(Signal::EndOfInput) if pane.line_is_empty => return Route::Raw,
        Some(Signal::EndOfInput) => return Route::Edit(Intent::DeleteForward),
        None => {}
    }
    match intent(keystroke, chars, platform) {
        Some(intent) => Route::Edit(intent),
        None => Route::Ignored,
    }
}

/// Whether this is one of the three keys that interrupt, end and suspend.
///
/// Rule 3 seen from the outside, for the one caller that has to apply it
/// without routing anything: a pane with a modal menu over it takes no typing
/// at all, and still cannot be allowed to stop a running command from being
/// interrupted.
pub fn is_signal(keystroke: &Keystroke) -> bool {
    signal(keystroke).is_some()
}

/// One of the three keys the tty reserves.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Signal {
    /// Ctrl-C: interrupt the foreground command.
    Interrupt,
    /// Ctrl-D: end of input.
    EndOfInput,
    /// Ctrl-Z: suspend the foreground command.
    Suspend,
}

/// Whether this keystroke asks for whatever is selected on screen.
///
/// Every chord that means "copy" on this platform, including the ones the
/// field would otherwise get and the one the shell would otherwise get. Rule 1
/// of [`route`] only consults it when there *is* something selected, which is
/// what keeps `ctrl-c` a signal the rest of the time.
///
/// Alt is excluded on both platforms because no copy chord has it, and a
/// `ctrl-alt-c` is somebody's window-manager binding leaking through rather
/// than a request to copy.
fn copies_the_output(keystroke: &Keystroke, platform: Platform) -> bool {
    let modifiers = keystroke.modifiers;
    if keystroke.key != "c" || modifiers.alt {
        return false;
    }
    match platform {
        // Cmd-C is the copy chord; Ctrl-C is here as well because a terminal
        // person reaches for it, and on this one key it costs nothing — the
        // interrupt is what it does whenever there is nothing to copy.
        Platform::Mac => modifiers.cmd != modifiers.ctrl && !modifiers.shift,
        // Ctrl-C and Ctrl-Shift-C, which are the interrupt and the copy: with
        // a selection on screen both copy, and the Shift is no longer the
        // thing a person has to remember.
        Platform::Other => modifiers.ctrl && !modifiers.cmd,
    }
}

/// Which signal a keystroke is, if it is one.
///
/// Shift is excluded because off macOS the Shift variants are the clipboard's
/// — that is the whole reason they are.
fn signal(keystroke: &Keystroke) -> Option<Signal> {
    let modifiers = keystroke.modifiers;
    if !modifiers.ctrl || modifiers.cmd || modifiers.alt || modifiers.shift {
        return None;
    }
    match keystroke.key.as_str() {
        "c" => Some(Signal::Interrupt),
        "d" => Some(Signal::EndOfInput),
        "z" => Some(Signal::Suspend),
        _ => None,
    }
}

/// What the editor should do about a keystroke, or `None` for one it has no
/// meaning for.
///
/// The keymap without the pane: [`route`] above is the whole keyboard policy
/// of a *pane*, and its first three rules — a selection owns the copy chord,
/// the alternate screen owns everything, a signal reaches the shell — are all
/// about a shell. A field with no shell under it wants none of them and all of
/// this: word movement, the line ends, the clipboard chords, undo, and the
/// emacs bindings macOS puts in every text field.
///
/// Callers outside a pane must still answer for the intents that assume one.
/// [`Intent::Submit`], [`Intent::Newline`], [`Intent::HistoryUp`] and
/// [`Intent::HistoryDown`] all mean "a line, and a place to send it"; a field
/// that has neither has to decide what those keys mean before it gets here.
pub fn intent(keystroke: &Keystroke, chars: &str, platform: Platform) -> Option<Intent> {
    let modifiers = keystroke.modifiers;
    if let Some(intent) = named_intent(&keystroke.key, modifiers, platform) {
        return Some(intent);
    }
    if let Some(intent) = chord_intent(&keystroke.key, modifiers, platform) {
        return Some(intent);
    }
    typed_intent(&keystroke.key, modifiers, chars, platform)
}

/// What the keys that have a name rather than a character do.
fn named_intent(key: &str, modifiers: Modifiers, platform: Platform) -> Option<Intent> {
    match key {
        // Shift-Enter is the only way to get a second line into a command, so
        // it is the one that must not be sent.
        "enter" if modifiers.shift => Some(Intent::Newline),
        "enter" if plain(modifiers) => Some(Intent::Submit),

        // Tab used to do nothing at all, because the shell had never seen the
        // partial line and had nothing to complete. It has now: the line goes
        // to the shell through a channel of its own and the answer comes back
        // the same way. A tab *character* is still not typed — no terminal has
        // ever let one into a command line, and the field measures in cells.
        // Backwards through what Tab turned up, and nothing where there is
        // nothing: see [`Intent::CompleteBackwards`]. Before the arm below it,
        // because `plain` allows Shift.
        "tab" if plain(modifiers) && modifiers.shift => Some(Intent::CompleteBackwards),
        "tab" if plain(modifiers) => Some(Intent::Complete),

        "backspace" => Some(match () {
            _ if line_chord(modifiers, platform) => Intent::DeleteToLineStart,
            _ if modifiers.alt || modifiers.ctrl => Intent::DeleteWordLeft,
            _ if modifiers.cmd => return None,
            _ => Intent::Backspace,
        }),
        "delete" => Some(match () {
            _ if line_chord(modifiers, platform) => Intent::DeleteToLineEnd,
            _ if modifiers.alt || modifiers.ctrl => Intent::DeleteWordRight,
            _ if modifiers.cmd => return None,
            _ => Intent::DeleteForward,
        }),

        "left" | "right" => {
            let forward = key == "right";
            let motion = if word_chord(modifiers, platform) {
                pick(forward, Motion::WordRight, Motion::WordLeft)
            } else if line_chord(modifiers, platform) {
                pick(forward, Motion::LineEnd, Motion::LineStart)
            } else if plain(modifiers) {
                pick(forward, Motion::Right, Motion::Left)
            } else {
                return None;
            };
            Some(movement(motion, modifiers))
        }

        "up" | "down" => {
            let forward = key == "down";
            if buffer_chord(modifiers, platform) {
                return Some(movement(
                    pick(forward, Motion::BufferEnd, Motion::BufferStart),
                    modifiers,
                ));
            }
            if !plain(modifiers) {
                return None;
            }
            // The bare arrows reach for the history, and only from the first
            // and last lines — which is the editor's rule, not this one's.
            Some(match (modifiers.shift, forward) {
                (true, true) => Intent::Extend(Motion::Down),
                (true, false) => Intent::Extend(Motion::Up),
                (false, true) => Intent::HistoryDown,
                (false, false) => Intent::HistoryUp,
            })
        }

        "home" | "end" => {
            let forward = key == "end";
            let motion = if buffer_chord(modifiers, platform) {
                pick(forward, Motion::BufferEnd, Motion::BufferStart)
            } else if plain(modifiers) {
                pick(forward, Motion::LineEnd, Motion::LineStart)
            } else {
                return None;
            };
            Some(movement(motion, modifiers))
        }

        _ => None,
    }
}

/// What the platform's application chords do.
fn chord_intent(key: &str, modifiers: Modifiers, platform: Platform) -> Option<Intent> {
    let shift = modifiers.shift;
    match platform {
        Platform::Mac => {
            if modifiers.cmd && !modifiers.ctrl && !modifiers.alt {
                return match (key, shift) {
                    ("a", false) => Some(Intent::SelectAll),
                    ("c", false) => Some(Intent::Copy),
                    ("x", false) => Some(Intent::Cut),
                    ("v", false) => Some(Intent::Paste),
                    ("z", false) => Some(Intent::Undo),
                    ("z", true) => Some(Intent::Redo),
                    _ => None,
                };
            }
            // Cocoa's own emacs bindings, which every macOS text field honours
            // and every shell's line editor honoured before it.
            if modifiers.ctrl && !modifiers.cmd && !modifiers.alt && !shift {
                return emacs_intent(key);
            }
            None
        }
        Platform::Other => {
            if !modifiers.ctrl || modifiers.cmd || modifiers.alt {
                return None;
            }
            match (key, shift) {
                ("a", _) => Some(Intent::SelectAll),
                // Ctrl-C is SIGINT and Ctrl-Z is SIGTSTP, so copy and undo
                // take the Shift variants — the same arrangement every
                // terminal emulator on Linux arrived at.
                ("c", true) => Some(Intent::Copy),
                ("z", true) => Some(Intent::Undo),
                ("x", _) => Some(Intent::Cut),
                ("v", _) => Some(Intent::Paste),
                ("y", _) => Some(Intent::Redo),
                _ if shift => None,
                _ => emacs_intent(key),
            }
        }
    }
}

/// The readline bindings both platforms honour, on Control.
///
/// Ctrl-W is here rather than in a table of its own because it is the reason
/// the werase character exists: it is what a terminal person presses to take
/// back the last word, and off macOS it used to close the pane instead.
fn emacs_intent(key: &str) -> Option<Intent> {
    match key {
        "a" => Some(Intent::Move(Motion::LineStart)),
        "e" => Some(Intent::Move(Motion::LineEnd)),
        "k" => Some(Intent::DeleteToLineEnd),
        "u" => Some(Intent::DeleteToLineStart),
        "w" => Some(Intent::DeleteWordLeft),
        _ => None,
    }
}

/// The text a key press produces, when it produces any.
///
/// On macOS Alt is a *composing* modifier — Alt-O types `ø` — and the platform
/// has already applied it to `chars`, so it is let through there and nowhere
/// else: on X11, Wayland and Windows the text of Alt-F is still `f`, and
/// typing an `f` is the one thing readline's Alt-F must not do. Control and
/// Command never type on either platform: a key held with either is a chord
/// that the tables above have already had their chance at.
fn typed_intent(
    key: &str,
    modifiers: Modifiers,
    chars: &str,
    platform: Platform,
) -> Option<Intent> {
    if modifiers.ctrl || modifiers.cmd || (modifiers.alt && platform == Platform::Other) {
        return None;
    }

    // The space bar is reported by name; every other typing key is reported by
    // the text it produced.
    let typed = if key == "space" && chars.is_empty() {
        " "
    } else {
        chars
    };

    // Tab, Escape and Backspace all produce text, and all of it is a control
    // character that would go into the line as an unprintable cell.
    let types = !typed.is_empty() && !typed.chars().any(char::is_control);
    types.then(|| Intent::Insert(typed.to_owned()))
}

/// A movement, extending the selection when Shift is held.
fn movement(motion: Motion, modifiers: Modifiers) -> Intent {
    if modifiers.shift {
        Intent::Extend(motion)
    } else {
        Intent::Move(motion)
    }
}

/// One of two values, by direction. Reads better at the call site than a
/// two-armed `if` inside an expression.
fn pick<T>(forward: bool, ahead: T, behind: T) -> T {
    if forward { ahead } else { behind }
}

/// Whether nothing but Shift is held.
fn plain(modifiers: Modifiers) -> bool {
    !modifiers.ctrl && !modifiers.cmd && !modifiers.alt
}

/// Whether these modifiers mean "by word" on this platform.
fn word_chord(modifiers: Modifiers, platform: Platform) -> bool {
    match platform {
        Platform::Mac => modifiers.alt && !modifiers.cmd && !modifiers.ctrl,
        Platform::Other => modifiers.ctrl && !modifiers.cmd && !modifiers.alt,
    }
}

/// Whether these modifiers mean "to the end of the line" on this platform.
///
/// Nothing off macOS: there the ends of a line are Home and End, and a
/// Ctrl-Left that also meant "line start" would take word movement away.
fn line_chord(modifiers: Modifiers, platform: Platform) -> bool {
    platform == Platform::Mac && modifiers.cmd && !modifiers.ctrl && !modifiers.alt
}

/// Whether these modifiers mean "to the end of the text" on this platform.
fn buffer_chord(modifiers: Modifiers, platform: Platform) -> bool {
    match platform {
        Platform::Mac => modifiers.cmd && !modifiers.ctrl && !modifiers.alt,
        Platform::Other => modifiers.ctrl && !modifiers.cmd && !modifiers.alt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke::new(key, modifiers)
    }

    fn none() -> Modifiers {
        Modifiers::default()
    }

    fn shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..Modifiers::default()
        }
    }

    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Modifiers::default()
        }
    }

    fn ctrl_shift() -> Modifiers {
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        }
    }

    fn cmd() -> Modifiers {
        Modifiers {
            cmd: true,
            ..Modifiers::default()
        }
    }

    fn cmd_shift() -> Modifiers {
        Modifiers {
            shift: true,
            ..cmd()
        }
    }

    fn cmd_alt() -> Modifiers {
        Modifiers { alt: true, ..cmd() }
    }

    fn alt() -> Modifiers {
        Modifiers {
            alt: true,
            ..Modifiers::default()
        }
    }

    /// A pane on the normal screen with a line half written in its field.
    fn composing() -> Pane {
        Pane {
            alt_screen: false,
            line_is_empty: false,
            grid_has_selection: false,
        }
    }

    /// A pane on the normal screen whose field is empty.
    fn empty() -> Pane {
        Pane {
            alt_screen: false,
            line_is_empty: true,
            grid_has_selection: false,
        }
    }

    /// A pane with a selection dragged out of the output above its field.
    fn selected(pane: Pane) -> Pane {
        Pane {
            grid_has_selection: true,
            ..pane
        }
    }

    /// What the editor is asked to do with this key on this platform.
    fn edit(key: &str, modifiers: Modifiers, platform: Platform) -> Option<Intent> {
        match route(&keystroke(key, modifiers), "", composing(), platform) {
            Route::Edit(intent) => Some(intent),
            _ => None,
        }
    }

    #[test]
    fn the_alt_screen_takes_every_key_before_the_editor_sees_one() {
        // vim is reading the arrows, and a field that swallowed them would
        // leave the cursor stuck in the corner.
        let vim = Pane {
            alt_screen: true,
            line_is_empty: true,
            grid_has_selection: false,
        };
        for platform in [Platform::Mac, Platform::Other] {
            for (key, modifiers) in [("left", none()), ("a", none()), ("enter", none())] {
                assert_eq!(
                    route(&keystroke(key, modifiers), "a", vim, platform),
                    Route::Raw,
                    "{key} was taken from a full-screen program"
                );
            }
        }
    }

    #[test]
    fn the_signal_keys_reach_the_shell_from_either_screen() {
        // The rule a running command depends on: `ctrl-c` interrupts it
        // whatever the field is doing.
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(
                route(&keystroke("c", ctrl()), "", composing(), platform),
                Route::Interrupt
            );
            assert_eq!(
                route(&keystroke("z", ctrl()), "", composing(), platform),
                Route::Raw
            );
            assert_eq!(
                route(&keystroke("d", ctrl()), "", empty(), platform),
                Route::Raw
            );
        }
    }

    #[test]
    fn ctrl_c_takes_the_half_written_line_with_it() {
        // Not `Raw`: the shell prints `^C` and a fresh prompt, and a field
        // still holding the abandoned command would be a lie about what just
        // happened.
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(
                route(&keystroke("c", ctrl()), "", composing(), platform),
                Route::Interrupt
            );
            assert!(
                Route::Interrupt.reaches_the_shell(),
                "and the shell hears it"
            );
        }
    }

    #[test]
    fn a_selection_in_the_output_takes_the_copy_chord_from_the_field() {
        // Rule 1. Two selections, one `cmd-c`: the one somebody just dragged
        // across the output wins over the one in a field they were not
        // looking at.
        assert_eq!(
            route(
                &keystroke("c", cmd()),
                "",
                selected(composing()),
                Platform::Mac
            ),
            Route::CopyOutput
        );
        assert_eq!(
            route(&keystroke("c", cmd()), "", composing(), Platform::Mac),
            Route::Edit(Intent::Copy),
            "with nothing selected in the output it is the field's copy again"
        );

        assert_eq!(
            route(
                &keystroke("c", ctrl_shift()),
                "",
                selected(composing()),
                Platform::Other
            ),
            Route::CopyOutput
        );
        assert_eq!(
            route(
                &keystroke("c", ctrl_shift()),
                "",
                composing(),
                Platform::Other
            ),
            Route::Edit(Intent::Copy)
        );
    }

    #[test]
    fn ctrl_c_interrupts_with_nothing_selected_and_copies_with_something() {
        // **The collision that matters.** A terminal whose interrupt key was
        // spent on the clipboard would be a terminal nobody could run a
        // program in, so the selection has to be what decides — and copying
        // lets go of it, which is why the second `ctrl-c` always interrupts.
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(
                route(&keystroke("c", ctrl()), "", composing(), platform),
                Route::Interrupt,
                "with nothing selected `ctrl-c` is the signal it has always been"
            );
            assert_eq!(
                route(&keystroke("c", ctrl()), "", selected(composing()), platform),
                Route::CopyOutput
            );
            assert!(
                !Route::CopyOutput.reaches_the_shell(),
                "a copy must not also interrupt the command it copied from"
            );

            // And nothing else the shell reserves is taken by a selection
            // being on screen.
            assert_eq!(
                route(&keystroke("z", ctrl()), "", selected(composing()), platform),
                Route::Raw
            );
            assert_eq!(
                route(&keystroke("d", ctrl()), "", selected(empty()), platform),
                Route::Raw
            );
        }
    }

    #[test]
    fn a_selection_can_be_copied_off_a_full_screen_programs_screen() {
        // Rule 1 sits above rule 2 on purpose: what vim has drawn is still
        // text somebody dragged a pointer across. Every other key on that
        // screen is still vim's, including `ctrl-c` once the selection is
        // gone.
        let vim = Pane {
            alt_screen: true,
            line_is_empty: true,
            grid_has_selection: true,
        };
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(
                route(&keystroke("c", ctrl()), "", vim, platform),
                Route::CopyOutput
            );
            assert_eq!(
                route(&keystroke("c", ctrl()), "", selected(vim), platform),
                Route::CopyOutput
            );
            assert_eq!(
                route(&keystroke("x", ctrl()), "", vim, platform),
                Route::Raw,
                "every other key on the alt screen is still the program's"
            );
            assert_eq!(
                route(
                    &keystroke("c", ctrl()),
                    "",
                    Pane {
                        grid_has_selection: false,
                        ..vim
                    },
                    platform
                ),
                Route::Raw,
                "and with nothing selected `ctrl-c` reaches the program"
            );
        }
    }

    #[test]
    fn nothing_but_a_copy_chord_is_taken_by_a_selection() {
        // The rule has to be narrow, or a selection nobody remembered making
        // would start eating keys. Only the letter `c`, only with the copy
        // modifiers of this platform.
        let pane = selected(composing());
        for (key, modifiers) in [("v", cmd()), ("x", cmd()), ("c", none()), ("c", alt())] {
            assert_ne!(
                route(&keystroke(key, modifiers), "c", pane, Platform::Mac),
                Route::CopyOutput,
                "{key} was taken by a selection"
            );
        }
        for (key, modifiers) in [("c", none()), ("c", shift()), ("v", ctrl()), ("x", ctrl())] {
            assert_ne!(
                route(&keystroke(key, modifiers), "c", pane, Platform::Other),
                Route::CopyOutput,
                "{key} was taken by a selection"
            );
        }
        assert_eq!(
            route(&keystroke("c", cmd_alt()), "", pane, Platform::Mac),
            Route::Ignored,
            "a window manager's chord is not a request to copy"
        );
    }

    #[test]
    fn ctrl_d_ends_the_input_only_on_an_empty_line() {
        // The shell's own reader has nothing in it any more — the line lives
        // in the field — so an unconditional Ctrl-D would be an end of file
        // every time, and the pane would close under somebody's fingers.
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(
                route(&keystroke("d", ctrl()), "", empty(), platform),
                Route::Raw,
                "an empty line is where Ctrl-D means end of file"
            );
            assert_eq!(
                route(&keystroke("d", ctrl()), "", composing(), platform),
                Route::Edit(Intent::DeleteForward),
                "and over a written line it is the delete every line editor has"
            );
        }
    }

    #[test]
    fn typing_goes_into_the_field_rather_than_the_pty() {
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(
                route(&keystroke("a", none()), "a", composing(), platform),
                Route::Edit(Intent::Insert("a".to_owned()))
            );
            assert_eq!(
                route(&keystroke("a", shift()), "A", composing(), platform),
                Route::Edit(Intent::Insert("A".to_owned())),
                "the platform already applied the layout, and its text is the truth"
            );
            assert_eq!(
                route(&keystroke("space", none()), " ", composing(), platform),
                Route::Edit(Intent::Insert(" ".to_owned()))
            );
        }
    }

    #[test]
    fn alt_composes_a_character_on_macos_and_types_nothing_anywhere_else() {
        // winit reports the text of Alt-F as `f` on X11, Wayland and Windows,
        // where Alt is not a composing modifier at all — and typing an `f` is
        // the one thing readline's Alt-F must not do.
        assert_eq!(
            route(&keystroke("o", alt()), "\u{f8}", composing(), Platform::Mac),
            Route::Edit(Intent::Insert("\u{f8}".to_owned())),
            "macOS applied the modifier and handed over the character it made"
        );
        assert_eq!(
            route(&keystroke("f", alt()), "f", composing(), Platform::Other),
            Route::Ignored
        );
    }

    #[test]
    fn a_key_that_produces_only_a_control_character_types_nothing() {
        // Escape produces text and has no cell to sit in. Neither has Tab,
        // which is why it asks the shell a question instead of typing one.
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(
                route(&keystroke("tab", none()), "\t", composing(), platform),
                Route::Edit(Intent::Complete),
                "a tab character has never been typeable into a command line"
            );
            assert_eq!(
                route(&keystroke("tab", shift()), "\t", composing(), platform),
                Route::Edit(Intent::CompleteBackwards),
                "and Shift-Tab steps back through what it turned up"
            );
            assert_eq!(
                route(
                    &keystroke("escape", none()),
                    "\u{1b}",
                    composing(),
                    platform
                ),
                Route::Ignored
            );
            assert_eq!(
                route(&keystroke("f5", none()), "", composing(), platform),
                Route::Ignored
            );
        }
    }

    #[test]
    fn enter_sends_the_line_and_shift_enter_lengthens_it() {
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(edit("enter", none(), platform), Some(Intent::Submit));
            assert_eq!(edit("enter", shift(), platform), Some(Intent::Newline));
        }
    }

    #[test]
    fn word_movement_is_alt_on_macos_and_control_everywhere_else() {
        assert_eq!(
            edit("left", alt(), Platform::Mac),
            Some(Intent::Move(Motion::WordLeft))
        );
        assert_eq!(
            edit("right", ctrl(), Platform::Other),
            Some(Intent::Move(Motion::WordRight))
        );
        // And each platform's word chord is the other's nothing-in-particular.
        assert_eq!(edit("left", ctrl(), Platform::Mac), None);
        assert_eq!(edit("left", alt(), Platform::Other), None);
    }

    #[test]
    fn the_ends_of_a_line_are_the_command_key_on_macos_and_home_and_end_elsewhere() {
        assert_eq!(
            edit("left", cmd(), Platform::Mac),
            Some(Intent::Move(Motion::LineStart))
        );
        assert_eq!(
            edit("end", none(), Platform::Other),
            Some(Intent::Move(Motion::LineEnd))
        );
        assert_eq!(
            edit("home", none(), Platform::Mac),
            Some(Intent::Move(Motion::LineStart)),
            "a Mac keyboard with a Home key still has one"
        );
    }

    #[test]
    fn the_ends_of_the_text_are_the_platform_chord_with_a_vertical_key() {
        assert_eq!(
            edit("up", cmd(), Platform::Mac),
            Some(Intent::Move(Motion::BufferStart))
        );
        assert_eq!(
            edit("end", ctrl(), Platform::Other),
            Some(Intent::Move(Motion::BufferEnd))
        );
    }

    #[test]
    fn shift_extends_whatever_the_movement_was() {
        let shifted_word = Modifiers {
            shift: true,
            ..alt()
        };
        assert_eq!(
            edit("left", shifted_word, Platform::Mac),
            Some(Intent::Extend(Motion::WordLeft))
        );
        assert_eq!(
            edit("up", shift(), Platform::Other),
            Some(Intent::Extend(Motion::Up)),
            "shift-up extends by a line and never reaches for the history"
        );
    }

    #[test]
    fn the_selection_chords_a_text_field_lives_on_are_the_fields() {
        // Both platforms' most-used selection gesture, and on both of them it
        // used to switch tabs instead.
        assert_eq!(
            edit("left", cmd_shift(), Platform::Mac),
            Some(Intent::Extend(Motion::LineStart))
        );
        assert_eq!(
            edit("right", cmd_shift(), Platform::Mac),
            Some(Intent::Extend(Motion::LineEnd))
        );
        assert_eq!(
            edit("left", ctrl_shift(), Platform::Other),
            Some(Intent::Extend(Motion::WordLeft))
        );
        assert_eq!(
            edit("right", ctrl_shift(), Platform::Other),
            Some(Intent::Extend(Motion::WordRight))
        );
    }

    #[test]
    fn the_bare_arrows_walk_the_history() {
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(edit("up", none(), platform), Some(Intent::HistoryUp));
            assert_eq!(edit("down", none(), platform), Some(Intent::HistoryDown));
        }
    }

    #[test]
    fn the_clipboard_is_on_command_on_macos() {
        assert_eq!(edit("a", cmd(), Platform::Mac), Some(Intent::SelectAll));
        assert_eq!(edit("c", cmd(), Platform::Mac), Some(Intent::Copy));
        assert_eq!(edit("x", cmd(), Platform::Mac), Some(Intent::Cut));
        assert_eq!(edit("v", cmd(), Platform::Mac), Some(Intent::Paste));
        assert_eq!(edit("z", cmd(), Platform::Mac), Some(Intent::Undo));
        assert_eq!(edit("z", cmd_shift(), Platform::Mac), Some(Intent::Redo));
    }

    #[test]
    fn off_macos_copy_and_undo_take_the_shift_variants_the_signal_keys_left_them() {
        // The one place the keymap is decided by the routing rule rather than
        // by convention: `ctrl-c` cannot be copy, because it has to interrupt.
        assert_eq!(edit("c", ctrl(), Platform::Other), None, "ctrl-c went raw");
        assert_eq!(edit("c", ctrl_shift(), Platform::Other), Some(Intent::Copy));
        assert_eq!(edit("z", ctrl(), Platform::Other), None, "ctrl-z went raw");
        assert_eq!(edit("z", ctrl_shift(), Platform::Other), Some(Intent::Undo));
        assert_eq!(edit("y", ctrl(), Platform::Other), Some(Intent::Redo));

        // And the two that were never signals keep the plain chord.
        assert_eq!(edit("a", ctrl(), Platform::Other), Some(Intent::SelectAll));
        assert_eq!(edit("v", ctrl(), Platform::Other), Some(Intent::Paste));
        assert_eq!(edit("x", ctrl(), Platform::Other), Some(Intent::Cut));
    }

    #[test]
    fn the_emacs_bindings_are_control_on_both_platforms() {
        assert_eq!(
            edit("a", ctrl(), Platform::Mac),
            Some(Intent::Move(Motion::LineStart)),
            "on macOS the clipboard is on Command, so Control is free for these"
        );
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(
                edit("e", ctrl(), platform),
                Some(Intent::Move(Motion::LineEnd))
            );
            assert_eq!(edit("k", ctrl(), platform), Some(Intent::DeleteToLineEnd));
            assert_eq!(edit("u", ctrl(), platform), Some(Intent::DeleteToLineStart));
            assert_eq!(
                edit("w", ctrl(), platform),
                Some(Intent::DeleteWordLeft),
                "the werase character, which off macOS used to close the pane"
            );
        }
    }

    #[test]
    fn deleting_by_word_and_by_line_follows_the_same_split_as_moving() {
        assert_eq!(
            edit("backspace", alt(), Platform::Mac),
            Some(Intent::DeleteWordLeft)
        );
        assert_eq!(
            edit("backspace", cmd(), Platform::Mac),
            Some(Intent::DeleteToLineStart)
        );
        assert_eq!(
            edit("backspace", ctrl(), Platform::Other),
            Some(Intent::DeleteWordLeft)
        );
        assert_eq!(
            edit("delete", ctrl(), Platform::Other),
            Some(Intent::DeleteWordRight)
        );
        for platform in [Platform::Mac, Platform::Other] {
            assert_eq!(edit("backspace", none(), platform), Some(Intent::Backspace));
            assert_eq!(
                edit("delete", none(), platform),
                Some(Intent::DeleteForward)
            );
        }
    }
}
