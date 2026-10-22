//! The card an agent's question is answered on.
//!
//! # Why a card at all
//!
//! Claude Code asks before it runs a command, in a dialog in its own tab, and
//! runs its `PermissionRequest` hook as that dialog opens. The hook —
//! `crook --agent ask claude` — hands the window the whole request and holds
//! on for an answer, while the dialog stays up beside it; see
//! [`crate::control::ask`]. A person with ten agents in ten tabs does not
//! want to visit each to read what it asks. This card is where they can read
//! it whole, and answer Allow once or Deny, without leaving the tab they are
//! in. Whichever answer comes first wins: Claude Code drops a hook's answer
//! that comes after the person answered in its dialog, and takes the first
//! the hook prints if they have not.
//!
//! # The card
//!
//! It is the question [`closing`](super::closing) asks, put to a different
//! use, and it keeps that question's rules, because they are the rules for a
//! card whose one wrong press cannot be taken back. The same face — header,
//! notes, hairline, a row of buttons, drawn by the same functions in
//! [`tab_menu`](super::tab_menu) — wider, since what it shows is a command.
//!
//! What it says is read on every frame from the question the hook handed the
//! window and from the strip, never from what the agent reported: a status's
//! message is whatever the agent chose to say, and the command is shown only
//! as it will run, through [`visible`], in the terminal's family, wrapped at
//! any character and scrolled rather than cut. It names the tab, as it is
//! called now, and the directory. It says what Claude Code says the command
//! does, as Claude Code's words. It says first, above the command, when the
//! command would run outside Claude Code's sandbox, and below it when it would
//! run in the background or with a time limit. It says the agent is asking in
//! its own tab too, and how long ago it asked — the age as of the frame, with
//! no timer to move it — and, once the pane has said it is running again,
//! that the question may already have been answered there. A question from
//! inside tmux says so, names tmux's pane, and says in how many other tabs
//! tmux is attached as well; its tab is the one holding tmux's client.
//!
//! **What is drawn is what runs.** The command is broken into rows by how
//! wide the shaper draws each grapheme, not by a width table, which calls
//! `⚠` one column where the colour-emoji font draws two; a row is never
//! broken inside a grapheme or a written-out character, never wider than the
//! box, and one the shaper still draws wider is cut at its edge and keeps
//! Allow waiting, since its end was never on screen. Every row reads left to
//! right, its characters in the order they run whatever letters it holds: a
//! shaper would reorder what lies between right-to-left letters with them.
//! A character no installed face has is written out, as everything else that
//! would not print as itself is — a combining mark that does not compose
//! with what it is on, a character that looks like a quote, a `#` or a `;`
//! — and everything written out is muted, so a command cannot spell a
//! `⟨U+XXXX⟩` out to pass for one.
//!
//! **Text that is not ASCII is allowed only as written.** None of that is a
//! guarantee: no list of characters that can pass for a quote, a `#` or a
//! `<<` is ever whole, and a font can draw a private-use point as anything.
//! So a request any text of which — its command, what its agent says, its
//! path, its change, the directory — is not ASCII says so under the box, and
//! Allow waits until it has been shown as written, every such character
//! written out and muted so that what is drawn is ASCII and marks nothing
//! can pass for, and read to its end so. Showing it so starts the card's
//! wait again. Once it has been read so, it stays allowed shown either way. A row the box
//! broke has a muted `↳` in a gutter before it, and a line of the command's
//! own has none, so a newline and a break never look alike. A tab a line
//! starts with is drawn as a muted `→`, and the tabs and spaces a line ends
//! in as `→` and `·`: a tab-indented `EOF` ends a `<<-` heredoc and a space
//! after a `\` ends a command; drawn blank, each reads as its harmless twin.
//!
//! **A change to a file is shown as the change.** An edit's old text is drawn
//! as removed lines and its new text as added ones, in the colours the
//! Changes column draws a diff in — see `changes_panel::diff_colour` — under
//! the file's path, written out whole and broken at any character rather
//! than cut. A tab in it is drawn as a muted `→` and a space a line ends in
//! as a muted `·`, as a diff viewer marks them. A write is its content, all
//! of it added, under a line saying whether it creates the file or replaces
//! one, and how many lines and bytes that one held, as the hook found it
//! when the agent asked — the window reads no file, and says that what is
//! there may change before the agent writes. A path that is not under the
//! directory the agent works in, by its text or, as the hook found it,
//! through a symbolic link on the way, is warned about above the change.
//! The change is the box a command is, and keeps its rules: the lines on
//! screen are the only ones built, and Allow waits for the last of them to
//! have been seen.
//!
//! **Not now is the default.** Escape presses it, a press off the card
//! presses it, and so do Enter and Space until the keyboard is moved off it.
//! It is drawn filled at the trailing edge, where the close question keeps
//! Cancel; Allow and Deny are outlined beside it. Not now answers nothing:
//! the agent's own dialog is still up, and the card can be opened again.
//! Tab, Shift-Tab and the arrows stop at the ends rather than going round, so
//! that Allow, at the leading edge, is never one key from the default, and a
//! button the keyboard has just been moved to waits [`SETTLE`] before Enter
//! or Space presses it: Tab and Enter in one breath answer nothing.
//!
//! **Nothing typed ahead can answer.** Allow, Deny and Go to tab wait for the
//! keyboard to have been still for [`SETTLE`] — the close question's second —
//! after the card came up, after the window took the keyboard, and after any
//! key the card has no use for, which is somebody typing at something else.
//! Until then Tab, the arrows, and Enter or Space on any of the three are
//! [`AskAction::TooSoon`]: taken, and the wait starts again. Go to waits as
//! well, because it hands the keyboard to Claude Code's dialog, whose own
//! default is Yes. And however the card closes — Go to, an answer sent, or
//! Escape, Enter or Space on Not now — what has the keyboard after it is kept
//! from it until it has been still for the same second, so the key that
//! closed the card, held until it repeats, does not answer the dialog in the
//! pane under it. Escape still closes the card at once. The scroll keys move
//! the command and start the wait again: a person still reading has not
//! decided.
//!
//! **Nor anything pressed ahead.** For the same second the three are drawn
//! with no handler at all — a disabled control in this application has none —
//! and one one-shot wait, not a timer, makes them live and the card is drawn
//! again. A button that turns live is drawn with mouse state of its own, so a
//! press begun while it was inert cannot be finished on it.
//!
//! **Nor anything not shown.** A command longer than its box has room for
//! keeps Allow inert, for keys and the pointer alike, until its last line has
//! been on screen — drawn inside the window, which the paint checks, and not
//! only inside the box, which is all the layout knows. A window too short for
//! the card shortens it, and the box gives up lines to the rest and scrolls
//! through the others, so that a short window cannot leave the end of the
//! command below its edge, read. Deny needs no such thing: a no is never the
//! dangerous answer.
//!
//! **The pane under it hears nothing**, as under the close question, and for
//! its reason: a reflexive `ctrl-c` to be rid of the card would interrupt the
//! agent that is asking. Only one popup is up at a time: opening the card
//! takes every menu and plugin panel down, and the close question beats it —
//! a close takes the card down, and the card does not open over a close.
//!
//! # The press
//!
//! Every press carries the question's number and its digest as the card read
//! them, and is carried out in one place, `Workspace::answer_ask`, which looks
//! again at everything: that the card is up on that question, that its digest
//! is the registry's, that the question is still held with its hook waiting
//! and is one this window may answer, that the time gates have passed, and,
//! for Allow, that the command was read to its end. Anything else sends
//! nothing, and the card turns to say the request changed or ended. None of
//! it is a command, an answer or a name the host knows — see
//! [`WorkspaceAction::Asking`] — so no binding, palette row, `--action`,
//! plugin, notification or socket can say it.
//!
//! An answer sent closes the card, back to the list of every question when
//! it was opened from there — see [`answer_list`](super::answer_list). It
//! never moves on to another question,
//! and it never claims the agent did as it was told: the row's status is left
//! to the agent's next report, which is the truth about what happened. A
//! confined session, for one, drops a hook's allow and keeps its dialog up.
//!
//! # When the question ends under it
//!
//! Its hook went, its time ran out, its turn ended, its pane closed, the
//! agent asked something newer, the command ran after a yes in the agent's
//! own tab, or letting agents ask in Crook was turned off: the card turns to
//! say Claude Code is no longer waiting on it, and why, and leaves only Go to
//! tab — still waiting its second — and Close.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crookui_core::AppContext;
use crookui_core::elements::{MouseStateHandle, WINDOW_INSET};
use crookui_core::event::{DispatchedEvent, Event, Keystroke, Modifiers};
use crookui_core::fonts::FamilyId;
use crookui_core::geometry::Point;
use crookui_core::prelude::*;
use crookui_core::presenter::{LayoutContext, PaintContext};
use crookui_core::text_layout::Line;

use crate::agent::request::{
    AskRequest, Mark, Replaced, as_written, command_as_written, command_lines, lexically_inside,
    visible,
};
use crate::control::ask::{Ask, AskId, Asks, Cause, Choice, How};
use crate::keybindings::Recording;
use crate::tab::PaneId;
use crate::theme::theme;

use super::action::{AskAction, AskButton, Pressed, WorkspaceAction};
use super::closing::SETTLE;
use super::tab_menu::{LABEL_SIZE, ROW_INSET, button, divider, header, note};
use super::view::Workspace;
use super::window_room::WindowRoom;

/// How wide the card is: room for a command line of seventy-odd columns.
const CARD_WIDTH: f32 = 480.;

/// The least the card is narrowed to in a window narrower than it.
const CARD_LEAST_WIDTH: f32 = 300.;

/// The least the card is shortened to in a window shorter than it, which
/// still leaves its command a line or two.
const CARD_LEAST_HEIGHT: f32 = 200.;

/// The card's corner radius, which is every menu's in this application.
const CARD_RADIUS: f32 = 6.;

/// How many lines of the command the card shows at once, at most: fewer in a
/// window too short for the card to have them all. The rest scrolls, inside
/// the box, and the buttons below it never move.
pub(super) const BODY_LINES: usize = 12;

/// How many columns a line of the command is wrapped at when the box has no
/// width to measure against, which only a flex measuring its children asks.
const FALLBACK_COLUMNS: usize = 80;

/// How wide the thumb beside an overflowing command is, and the room kept
/// for it.
const THUMB_WIDTH: f32 = 4.;

/// The mark in the box's gutter beside a row that goes on with the line
/// above it rather than starting one: a line the box broke, which a newline
/// in the command would otherwise draw exactly as. Not `↪`, which is an
/// emoji, and drawn as one in colour by the font it falls back to.
const CONTINUED: &str = "↳";

/// Why the card is no longer about a question that can be answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Withdrawn {
    /// The question ended, for this reason.
    Ended(Cause),
    /// A press found the question was not what the card was opened on, or
    /// was over: nothing was sent.
    Changed,
}

impl Withdrawn {
    /// What the card says about it, in a sentence.
    pub(super) fn sentence(self, agent: &str) -> String {
        match self {
            Self::Changed => "This request changed or ended, and nothing was sent.".to_owned(),
            Self::Ended(Cause::AnsweredInAgent) => {
                format!("It was answered in {agent}'s own tab, and the command has run.")
            }
            // Not that the command ran: a "No, and tell Claude what to do
            // differently" sets the agent to work again as a Yes does.
            Self::Ended(Cause::AtWorkAgain) => {
                format!("It was answered in {agent}'s own tab, and {agent} is at work again.")
            }
            Self::Ended(Cause::HookGone) => format!(
                "Its hook stopped waiting: it was answered in {agent}'s own tab, or {agent} \
                 stopped asking."
            ),
            Self::Ended(Cause::Expired) => "Its time to wait for an answer ran out.".to_owned(),
            Self::Ended(Cause::PaneClosed) => "Its tab closed.".to_owned(),
            Self::Ended(Cause::CommandEnded) => format!("{agent} is no longer running in its tab."),
            Self::Ended(Cause::TurnEnded) => format!("{agent}'s turn ended."),
            Self::Ended(Cause::AskedAgain) => format!("{agent} asked something newer."),
            Self::Ended(Cause::WindowClosed) => "The window is closing.".to_owned(),
            Self::Ended(Cause::AnsweringOff) => {
                "Letting agents ask in Crook was turned off.".to_owned()
            }
        }
    }
}

/// What the card looks like this frame.
pub(super) enum Face<'a> {
    /// The question it was opened on, still held and waiting.
    Live(&'a Ask),
    /// It is over, or was never what the card thought.
    Withdrawn(Withdrawn),
}

/// Where the command's box is scrolled to, and what its last layout found.
///
/// Shared between the card and the element that draws the box, since it is
/// the layout that learns how many lines a command wraps to at the width the
/// card came out at, and the card that has to know whether the end has been
/// on screen.
#[derive(Default)]
pub(super) struct Body {
    /// The first line shown.
    top: Cell<usize>,
    /// How many lines the command wrapped to, at the last layout; zero before
    /// the first.
    lines: Cell<usize>,
    /// How many of them the box had room to show, at the last layout: at
    /// most [`BODY_LINES`], and fewer in a short window; zero before the
    /// first.
    shown: Cell<usize>,
    /// Whether its last line has been on screen.
    read_to_end: Cell<bool>,
    /// Whether a row of it was drawn wider than the box, which a row is
    /// never wrapped to: its end was cut off at the box's edge, unseen, so
    /// the command has not been read whatever the box shows. Once, for good.
    too_wide: Cell<bool>,
    /// How wide each grapheme the command holds is drawn, by the family it
    /// was measured in: what the rows are broken by. Kept, since a frame asks
    /// for every one of them.
    widths:
        RefCell<std::collections::HashMap<FamilyId, std::collections::HashMap<String, Grapheme>>>,
    /// The command as last broken into rows, and what it was broken for.
    laid: RefCell<Option<Rc<Laid>>>,
    /// The rows drawn so far of [`Self::laid`], shaped, by their number:
    /// shaping a row is the dear part of drawing one, and a row drawn on one
    /// frame is drawn on the next.
    shaped: RefCell<std::collections::HashMap<usize, Rc<Line>>>,
    /// Whether what the card shows of the request has text in it that is
    /// not ASCII, so that Allow waits for it to have been read as written.
    exact_needed: Cell<bool>,
    /// Whether the box shows the request as written: every character that
    /// is not ASCII written out.
    exact: Cell<bool>,
    /// Whether the request, shown as written, has been on screen to its end.
    exact_read: Cell<bool>,
    /// Whether the last layout broke the command by a guess at how wide
    /// some of it is, having run out of the time a frame may spend measuring:
    /// the rows are not yet the ones it will be drawn in, and it has not been
    /// read until they are.
    measuring: Cell<bool>,
    /// Whether a frame has learned something about the box since the card
    /// was last built: how many lines the command wraps to, how many the box
    /// shows, or that the end was on screen. The card that frame drew was
    /// built before it knew — Allow inert, the line under the box saying
    /// nothing — and the next has to be asked for; see
    /// `Workspace::frame_drawn`.
    news: Cell<bool>,
}

impl Body {
    /// Whether the command has been read to its end: its last line has been
    /// on screen, which is never before it has been laid out once, and no
    /// row of it was ever drawn past the box's edge.
    pub(super) fn read_to_end(&self) -> bool {
        self.read_to_end.get()
            && !self.too_wide.get()
            && !self.measuring.get()
            && (!self.exact_needed.get() || self.exact_read.get())
    }

    /// Whether Allow waits for the request to be shown as written and read
    /// to its end so, and has not yet had it.
    fn waits_for_exact(&self) -> bool {
        self.exact_needed.get() && !self.exact_read.get()
    }

    /// Whether the request, shown as written, has been read to its end.
    #[cfg(test)]
    pub(super) fn exact_read(&self) -> bool {
        self.exact_read.get()
    }

    /// Whether the last layout broke the command by a guess.
    #[cfg(test)]
    pub(super) fn is_measuring(&self) -> bool {
        self.measuring.get()
    }

    /// Whether a row was drawn wider than the box.
    fn too_wide(&self) -> bool {
        self.too_wide.get()
    }

    /// Writes down that a row was drawn wider than the box.
    fn note_too_wide(&self) {
        if !self.too_wide.replace(true) {
            self.news.set(true);
        }
    }

    /// How many lines the box shows at once.
    fn shown(&self) -> usize {
        match self.shown.get() {
            0 => BODY_LINES,
            shown => shown,
        }
    }

    /// The largest first line that still fills the box.
    fn last_top(&self) -> usize {
        self.lines.get().saturating_sub(self.shown())
    }

    /// Puts `top` first, kept inside the command. Whether it moved.
    ///
    /// Whether that shows the end is the paint's to say, which is the one
    /// that knows where the box landed in the window.
    fn scroll_to(&self, top: usize) -> bool {
        let top = top.min(self.last_top());
        let moved = top != self.top.get();
        self.top.set(top);
        moved
    }

    /// Moves by `lines`, down for positive. Whether it moved.
    fn scroll_by(&self, lines: isize) -> bool {
        self.scroll_to(self.top.get().saturating_add_signed(lines))
    }

    /// Writes down that the end has been seen, when the box shows it and
    /// `on_screen`, which the paint says: whether the last line was drawn
    /// inside the window and inside whatever clips the card.
    fn note_the_end(&self, on_screen: bool) {
        if !on_screen || !self.shows_the_end() || self.measuring.get() {
            return;
        }
        let first = !self.read_to_end.replace(true);
        let first_exact = self.exact.get() && !self.exact_read.replace(true);
        if first || first_exact {
            self.news.set(true);
        }
    }

    /// Takes in how many lines the command wraps to and how many the box
    /// shows, as a layout found them.
    fn measured(&self, lines: usize, shown: usize) {
        let was = (self.lines.replace(lines), self.shown.replace(shown));
        if was != (lines, shown) {
            self.news.set(true);
        }
    }

    /// Whether a frame has learned anything since this was last asked, which
    /// it then forgets.
    pub(super) fn take_news(&self) -> bool {
        self.news.replace(false)
    }

    /// Whether the box, scrolled where it is, shows the command's last line.
    fn shows_the_end(&self) -> bool {
        self.lines.get() > 0 && self.top.get() + self.shown() >= self.lines.get()
    }
}

/// The mouse state of each of the card's buttons, and whether each was last
/// drawn with a handler.
#[derive(Default)]
struct Handles {
    allow: MouseStateHandle,
    deny: MouseStateHandle,
    go_to: MouseStateHandle,
    not_now: MouseStateHandle,
    as_written: MouseStateHandle,
    /// Allow, Deny and Go to, as last drawn.
    live: [bool; 3],
}

impl Handles {
    /// The handle for `button`, fresh when it is about to be drawn live after
    /// a frame it was drawn inert in — so a press begun on the inert one,
    /// whose state remembers it, cannot be let go of on the live one.
    fn for_button(&mut self, button: AskButton, live: bool) -> MouseStateHandle {
        let index = match button {
            AskButton::Allow => 0,
            AskButton::Deny => 1,
            AskButton::GoTo => 2,
            AskButton::NotNow => return self.not_now.clone(),
            AskButton::AsWritten => return self.as_written.clone(),
        };
        let handle = match button {
            AskButton::Allow => &mut self.allow,
            AskButton::Deny => &mut self.deny,
            _ => &mut self.go_to,
        };
        if live && !self.live[index] {
            *handle = MouseStateHandle::default();
        }
        self.live[index] = live;
        handle.clone()
    }
}

/// The card, while it is up.
pub(super) struct AskCard {
    /// The question it was opened on, and stays on.
    ask: AskId,
    /// That question's digest when the card opened: what every press carries.
    digest: [u8; 32],
    /// The pane it was asked in.
    pane: PaneId,
    /// The button the keyboard is on, which Enter and Space press and which
    /// is drawn filled. Not now, until Tab or an arrow moves it.
    chosen: AskButton,
    /// When a key may first reach Allow, Deny or Go to: [`SETTLE`] after the
    /// card came up, the window last took the keyboard, or the last key that
    /// came too soon or meant nothing to the card.
    ///
    /// A cell for the close question's reason: the keystroke being asked
    /// about is what moves it, and the window asks about each one once.
    settles_at: Cell<Instant>,
    /// When Enter or Space may first press the button the keyboard is on:
    /// [`SETTLE`] after Tab or an arrow last moved it there, so that a move
    /// and a press in one breath — Tab, Enter — answer nothing.
    press_settles_at: Instant,
    /// When the pointer may first reach them: [`SETTLE`] after the card came
    /// up or its face changed, or the window last took the keyboard.
    pointer_live_at: Instant,
    /// Whether the one-shot wait for [`Self::pointer_live_at`] has come home
    /// and drawn them live.
    pointer_live: bool,
    /// The command's box.
    body: Rc<Body>,
    /// The buttons' mouse state.
    handles: RefCell<Handles>,
    /// Why it is no longer about a question that can be answered, once a
    /// press has found so or the window has heard the question end.
    withdrawn: Option<Withdrawn>,
    /// Whether the pane has said it is running since the card came up: the
    /// agent may have been answered in its own tab.
    back_to_work: bool,
}

impl AskCard {
    /// A card about `ask`, coming up at `now`.
    pub(super) fn about(ask: &Ask, now: Instant) -> Self {
        let body = Rc::new(Body::default());
        body.exact_needed
            .set(!ask.request.is_ascii() || !ask.cwd.is_ascii());
        Self {
            ask: ask.id,
            digest: ask.digest,
            pane: ask.pane,
            chosen: AskButton::default(),
            settles_at: Cell::new(now + SETTLE),
            press_settles_at: now,
            pointer_live_at: now + SETTLE,
            pointer_live: false,
            body,
            handles: RefCell::default(),
            withdrawn: None,
            back_to_work: false,
        }
    }

    /// Shows the command as written, or as drawn again. The box is laid out
    /// afresh; what of the command has been read stays read, since it is the
    /// same command.
    pub(super) fn toggle_as_written(&mut self) {
        self.body.exact.set(!self.body.exact.get());
        *self.body.laid.borrow_mut() = None;
        self.body.shaped.borrow_mut().clear();
        self.body.top.set(0);
    }

    /// Whether the request is shown as written.
    pub(super) fn is_as_written(&self) -> bool {
        self.body.exact.get()
    }

    /// The question it is about.
    pub(super) fn ask(&self) -> AskId {
        self.ask
    }

    /// Its digest, as the card read it.
    pub(super) fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// The pane it was asked in.
    pub(super) fn pane(&self) -> PaneId {
        self.pane
    }

    /// The button the keyboard is on.
    #[cfg(test)]
    pub(super) fn chosen(&self) -> AskButton {
        self.chosen
    }

    /// Puts the keyboard on `button`, at `now`, which starts the wait before
    /// Enter or Space may press it.
    pub(super) fn choose(&mut self, button: AskButton, now: Instant) {
        self.chosen = button;
        self.press_settles_at = now + SETTLE;
    }

    /// The command's box.
    pub(super) fn body(&self) -> &Body {
        &self.body
    }

    /// Starts the keyboard's wait for [`SETTLE`] again, from `now`.
    pub(super) fn unsettle(&self, now: Instant) {
        self.settles_at.set(now + SETTLE);
    }

    /// Whether the keyboard has been still long enough, by `now`, to move.
    pub(super) fn has_settled(&self, now: Instant) -> bool {
        now >= self.settles_at.get()
    }

    /// Whether it has been still long enough, by `now`, to press the button
    /// it is on: settled, and on that button for [`SETTLE`].
    pub(super) fn press_has_settled(&self, now: Instant) -> bool {
        self.has_settled(now) && now >= self.press_settles_at
    }

    /// Makes the pointer wait [`SETTLE`] again from `now`, and hands back the
    /// moment it may reach the buttons, which the one-shot wait that makes
    /// them live is for.
    pub(super) fn hold_the_pointer(&mut self, now: Instant) -> Instant {
        self.pointer_live = false;
        self.pointer_live_at = now + SETTLE;
        self.pointer_live_at
    }

    /// Lets the pointer reach the buttons, when the wait that came home is
    /// the one the card is still waiting on. Whether it did.
    pub(super) fn free_the_pointer(&mut self, waited_for: Instant) -> bool {
        if self.pointer_live || waited_for != self.pointer_live_at {
            return false;
        }
        self.pointer_live = true;
        true
    }

    /// Whether the pointer may reach Allow, Deny and Go to, at `now`.
    pub(super) fn pointer_is_live(&self, now: Instant) -> bool {
        self.pointer_live && now >= self.pointer_live_at
    }

    /// When the pointer may first reach them.
    pub(super) fn pointer_live_at(&self) -> Instant {
        self.pointer_live_at
    }

    /// Turns the card to the face of a question that is over.
    pub(super) fn withdraw(&mut self, why: Withdrawn) {
        self.withdrawn = Some(why);
        self.chosen = AskButton::NotNow;
    }

    /// Whether it has turned to that face.
    pub(super) fn is_withdrawn(&self) -> bool {
        self.withdrawn.is_some()
    }

    /// Notes that its pane said it is running again, and whether that is
    /// news.
    pub(super) fn back_to_work(&mut self) -> bool {
        !std::mem::replace(&mut self.back_to_work, true)
    }

    /// What it looks like, by what `asks` holds now.
    pub(super) fn face<'a>(&self, asks: &'a Asks) -> Face<'a> {
        if let Some(why) = self.withdrawn {
            return Face::Withdrawn(why);
        }
        match asks.get(self.ask) {
            Some(ask) if ask.digest == self.digest => Face::Live(ask),
            Some(_) => Face::Withdrawn(Withdrawn::Changed),
            None => Face::Withdrawn(ended(asks, self.ask)),
        }
    }

    /// Says the command has been on screen to its end, for a window that
    /// draws no frame — a test's, with no presenter — and so never lays the
    /// box out.
    #[cfg(all(test, unix))]
    pub(super) fn read_to_its_end(&self) {
        self.body.lines.set(self.body.lines.get().max(1));
        self.body.read_to_end.set(true);
    }

    /// Lets the keyboard and the pointer reach the buttons from now on, as a
    /// second of stillness would — which a test says this way rather than by
    /// sleeping through it.
    #[cfg(test)]
    pub(super) fn settle(&mut self) {
        let now = Instant::now();
        self.settles_at.set(now);
        self.press_settles_at = now;
        self.pointer_live_at = now;
        self.pointer_live = true;
    }
}

/// Why question `id` is over, as `asks` remembers it.
pub(super) fn ended(asks: &Asks, id: AskId) -> Withdrawn {
    match asks.outcome(id).map(|outcome| outcome.how) {
        Some(How::Ended(cause)) => Withdrawn::Ended(cause),
        // Answered from somewhere this card did not see, or forgotten: either
        // way not something the card can say more about.
        Some(How::Answered(_)) => Withdrawn::Changed,
        None => Withdrawn::Ended(Cause::HookGone),
    }
}

/// The buttons the card has, in the order they sit, leading edge first.
fn buttons(face: &Face<'_>) -> &'static [AskButton] {
    match face {
        Face::Live(ask) if ask.cannot_answer.is_none() => &[
            AskButton::Allow,
            AskButton::Deny,
            AskButton::GoTo,
            AskButton::NotNow,
        ],
        Face::Live(_) | Face::Withdrawn(_) => &[AskButton::GoTo, AskButton::NotNow],
    }
}

/// Whether the card says the request holds text that is not ASCII, and
/// offers to show it as written: a card still waiting, on a request of any
/// kind, any text of which — command, what its agent says, path, change,
/// working directory — is not all ASCII.
fn offers_as_written(card: &AskCard, face: &Face<'_>) -> bool {
    matches!(face, Face::Live(_)) && card.body.exact_needed.get()
}

/// The buttons the keyboard can stand on: Show as written first, when the
/// card offers it, then the row's; Allow only once the command has been
/// read to its end, Go to only while its pane is open.
fn reachable(card: &AskCard, face: &Face<'_>, pane_open: bool) -> Vec<AskButton> {
    offers_as_written(card, face)
        .then_some(AskButton::AsWritten)
        .into_iter()
        .chain(buttons(face).iter().copied())
        .filter(|button| match button {
            AskButton::Allow => card.body.read_to_end(),
            AskButton::GoTo => pane_open,
            _ => true,
        })
        .collect()
}

/// What a press of `button` on the card means, when it means anything but
/// Not now.
fn pressing(card: &AskCard, button: AskButton, by: Pressed) -> AskAction {
    let (ask, digest) = (card.ask, card.digest);
    match button {
        AskButton::NotNow => AskAction::NotNow,
        AskButton::AsWritten => AskAction::AsWritten,
        AskButton::GoTo => AskAction::GoTo { ask, digest, by },
        AskButton::Allow => AskAction::Answer {
            ask,
            digest,
            decision: Choice::Allow,
            by,
        },
        AskButton::Deny => AskAction::Answer {
            ask,
            digest,
            decision: Choice::Deny,
            by,
        },
    }
}

/// What a keystroke means while the card is up, at `now`, with the card's
/// face read off `asks` and `pane_open` whether its pane is still open.
///
/// Shaped like [`closing::action_for`](super::closing::action_for), with two
/// differences. Every key is taken — a key the card has no use for starts
/// the wait again and goes nowhere, since a palette or a pane under a card
/// that answers an agent is the last place a stray key should land — but for
/// a modifier held on its own, which is how Shift-Tab is reached. And the
/// scroll keys move the command and start the wait again.
pub(super) fn action_for(
    card: &AskCard,
    asks: &Asks,
    pane_open: bool,
    keystroke: &Keystroke,
    now: Instant,
) -> Option<WorkspaceAction> {
    if Recording::is_a_modifier(keystroke) {
        return None;
    }
    let face = card.face(asks);
    let reachable = reachable(card, &face, pane_open);
    let modifiers = keystroke.modifiers;
    let bare = modifiers.is_empty();
    let shifted = modifiers
        == Modifiers {
            shift: true,
            ..Modifiers::default()
        };
    let chosen = if reachable.contains(&card.chosen) {
        card.chosen
    } else {
        AskButton::NotNow
    };
    let at = reachable
        .iter()
        .position(|button| *button == chosen)
        .unwrap_or(reachable.len() - 1);
    let page = card.body.shown() as isize - 1;
    // Taken whether it moved or not: an arrow at the end of the command must
    // not fall through to whatever is under the card.
    let scrolled = |_moved: bool| {
        card.unsettle(now);
        Some(AskAction::Scrolled.into())
    };
    let action = match keystroke.key.as_str() {
        "escape" if bare => return Some(AskAction::NotNow.into()),
        "enter" | "space" if bare => match chosen {
            AskButton::NotNow => return Some(AskAction::NotNow.into()),
            button => pressing(card, button, Pressed::Keys),
        },
        // Never round from the end: Not now is the last button and the
        // default, and Tab wrapping from it would put the keyboard on Allow.
        "tab" if bare => AskAction::Choose(reachable[(at + 1).min(reachable.len() - 1)]),
        "tab" if shifted => AskAction::Choose(reachable[at.saturating_sub(1)]),
        "left" if bare => AskAction::Choose(reachable[at.saturating_sub(1)]),
        "right" if bare => AskAction::Choose(reachable[(at + 1).min(reachable.len() - 1)]),
        "up" if bare => return scrolled(card.body.scroll_by(-1)),
        "down" if bare => return scrolled(card.body.scroll_by(1)),
        "pageup" if bare => return scrolled(card.body.scroll_by(-page)),
        "pagedown" if bare => return scrolled(card.body.scroll_by(page)),
        "home" if bare => return scrolled(card.body.scroll_to(0)),
        "end" if bare => return scrolled(card.body.scroll_to(usize::MAX)),
        _ => {
            card.unsettle(now);
            return Some(AskAction::TooSoon.into());
        }
    };
    let settled = match action {
        AskAction::Choose(_) => card.has_settled(now),
        _ => card.press_has_settled(now),
    };
    if settled {
        return Some(action.into());
    }
    card.unsettle(now);
    Some(AskAction::TooSoon.into())
}

/// "40 s", "2 min 5 s": a span short enough to say to the second.
pub(super) fn duration(span: Duration) -> String {
    let seconds = span.as_secs();
    match (seconds / 60, seconds % 60) {
        (0, seconds) => format!("{seconds} s"),
        (minutes, 0) => format!("{minutes} min"),
        (minutes, seconds) => format!("{minutes} min {seconds} s"),
    }
}

/// The card's first line.
pub(super) fn title(agent: &str, face: &Face<'_>) -> String {
    match face {
        Face::Live(ask) => match ask.request {
            AskRequest::Command { .. } => format!("{agent} wants to run a command"),
            AskRequest::Edit { .. } => format!("{agent} wants to edit a file"),
            AskRequest::Write { .. } => format!("{agent} wants to write a file"),
        },
        Face::Withdrawn(_) => format!("{agent} is no longer waiting on this"),
    }
}

/// The whole card, over a modal underlay that answers a press off it with
/// Not now.
pub(super) fn render(workspace: &Workspace, card: &AskCard) -> Box<dyn Element> {
    let fonts = workspace.fonts();
    let ui = fonts.ui;
    let now = Instant::now();
    let face = card.face(workspace.asks());
    let strip = workspace.tabs();
    let pane = strip.pane(card.pane);
    let agent = match &face {
        Face::Live(ask) => ask.agent.name(),
        Face::Withdrawn(_) => "Claude Code",
    };

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header(title(agent, &face), ui));

    // The tab as it is called now, read off the strip, and where the agent
    // says it works: the two things that say which of ten agents this is.
    let tab = strip
        .panes()
        .find(|(_, open)| open.id() == card.pane)
        .and_then(|(tab, _)| strip.get(tab))
        .map(|tab| tab.name().to_owned());
    match &face {
        Face::Live(ask) => {
            let cwd =
                crate::git::user_friendly_path(std::path::Path::new(&ask.cwd), workspace.home());
            let cwd = if card.is_as_written() {
                as_written(&cwd)
            } else {
                visible(&cwd)
            };
            let place = match tab {
                Some(tab) => format!("{} · {cwd}", visible(&tab)),
                None => cwd,
            };
            column.add_child(one_line(place, ui));
            live_face(&mut column, workspace, card, ask, now, ui);
        }
        Face::Withdrawn(why) => {
            if let Some(tab) = tab {
                column.add_child(one_line(visible(&tab), ui));
            }
            column.add_child(divider());
            column.add_child(note(why.sentence(agent), ui));
        }
    }
    column.add_child(button_row(card, &face, pane.is_some(), now, ui));

    let card_element = WindowRoom::new(
        ConstrainedBox::new(
            Container::new(column.finish())
                .with_background_color(theme().surface_raised)
                .with_border(Border::all(1.).with_border_color(theme().overlay_2))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)))
                .with_vertical_padding(6.)
                .finish(),
        )
        .with_width(CARD_WIDTH)
        .finish(),
    )
    .with_width_inset(WINDOW_INSET, CARD_LEAST_WIDTH)
    .with_height_inset(WINDOW_INSET, CARD_LEAST_HEIGHT)
    .finish();

    // Placed as the close question is, centred and a third of the way down.
    let centred = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(card_element)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .finish();
    let placed = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(centred)
        .with_child(Expanded::new(2., Empty::new().finish()).finish())
        .finish();

    Dismiss::new(placed)
        // The rest of the window is inert while it is up: a row pressed
        // behind it could be a way to change what it is about unseen.
        .modal()
        .on_dismiss(|ctx, _| ctx.dispatch_typed_action(WorkspaceAction::from(AskAction::NotNow)))
        .finish()
}

/// What the card says about a question still waiting: the warning above the
/// command, the command, the agent's words, the other warnings, and where
/// the question stands.
fn live_face(
    column: &mut Flex,
    workspace: &Workspace,
    card: &AskCard,
    ask: &Ask,
    now: Instant,
    ui: FamilyId,
) {
    let agent = ask.agent.name();
    let monospace = workspace.fonts().monospace;
    let exact = card.is_as_written();
    let shown = |text: &str| {
        if exact {
            as_written(text)
        } else {
            visible(text)
        }
    };
    match &ask.request {
        AskRequest::Command {
            command,
            note: said,
            timeout_ms,
            background,
            outside_sandbox,
        } => {
            if *outside_sandbox {
                column.add_child(warning(
                    format!(
                        "Outside {agent}'s sandbox: it would run with none of the sandbox's limits."
                    ),
                    ui,
                ));
            }
            let (text, muted) = if exact {
                command_as_written(command)
            } else {
                command_lines(command)
            };
            let lines = vec![Shown {
                text,
                color: theme().text_primary,
                muted,
            }];
            column.add_child(body_box(lines, card, monospace));
            column.add_child(extent(&card.body, ui));
            if card.body.exact_needed.get() {
                column.add_child(as_written_line(card, ui));
            }
            if let Some(said) = said {
                column.add_child(note(format!("{agent} says: {}", shown(said)), ui));
            }
            if *background {
                column.add_child(note(
                    format!("In the background: {agent} goes on without waiting for it."),
                    ui,
                ));
            }
            if let Some(timeout) = timeout_ms {
                column.add_child(note(
                    format!(
                        "With a time limit of {}.",
                        duration(Duration::from_millis(*timeout))
                    ),
                    ui,
                ));
            }
        }
        AskRequest::Edit { path, .. } | AskRequest::Write { path, .. } => {
            if let Some(outside) = outside_warning(&ask.request, &ask.cwd) {
                column.add_child(warning(outside.to_owned(), ui));
            }
            column.add_child(file_path(
                shown(&crate::git::user_friendly_path(
                    std::path::Path::new(path),
                    workspace.home(),
                )),
                monospace,
            ));
            if let AskRequest::Write { replaces, .. } = &ask.request {
                column.add_child(note(
                    what_a_write_does(*replaces, ask.request.change_lines(), agent),
                    ui,
                ));
            }
            column.add_child(body_box(change_lines(&ask.request, exact), card, monospace));
            column.add_child(extent(&card.body, ui));
            if card.body.exact_needed.get() {
                column.add_child(as_written_line(card, ui));
            }
        }
    }
    column.add_child(divider());
    let age = duration(now.saturating_duration_since(ask.asked_at));
    match &ask.inside {
        None => column.add_child(one_line(
            format!("{agent} is asking in its tab too · asked {age} ago"),
            ui,
        )),
        Some(inside) => {
            column.add_child(one_line(
                format!(
                    "{agent} is asking in its {} pane {} too · asked {age} ago",
                    inside.kind.word(),
                    inside.pane
                ),
                ui,
            ));
            column.add_child(note(format!("Asked from {}.", inside.said()), ui));
        }
    }
    if card.back_to_work {
        column.add_child(note(
            "Its tab has gone back to work; this may already be answered there.",
            ui,
        ));
    }
    if let Some(cannot) = ask.cannot_answer {
        column.add_child(note(cannot.sentence(), ui));
    }
}

/// The box the command or the change is drawn in.
///
/// The one part of the card that gives up height in a short window: what
/// the rest leaves is the lines it shows, and it scrolls through the others.
fn body_box(lines: Vec<Shown>, card: &AskCard, monospace: FamilyId) -> Box<dyn Element> {
    Shrinkable::new(
        1.,
        Container::new(CommandBody::new(lines, card.body.clone(), monospace).finish())
            .with_horizontal_padding(ROW_INSET)
            .with_margin_bottom(6.)
            .finish(),
    )
    .finish()
}

/// The warning a change to a file is shown under when the file is not
/// under `cwd`, or `None` when it is: by its text, which the window reads,
/// or through a link, which only the hook could look for.
pub(super) fn outside_warning(request: &AskRequest, cwd: &str) -> Option<&'static str> {
    let path = request.path()?;
    if !lexically_inside(path, cwd) {
        Some(
            "Outside its working directory: this file is not under the directory the agent \
             works in.",
        )
    } else if request.outside(cwd) {
        Some(
            "Outside its working directory: a symbolic link on the way to this file leads out \
             of the directory the agent works in, or could not be followed.",
        )
    } else {
        None
    }
}

/// What a write does to the file, as its hook found the file when `agent`
/// asked, putting in `lines` lines: creates it, or replaces one of so many
/// lines and bytes — and that this was then, not now.
pub(super) fn what_a_write_does(replaces: Option<Replaced>, lines: usize, agent: &str) -> String {
    let counted = |count: u64, one: &str, many: &str| match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    };
    let these = match lines {
        0 => "nothing".to_owned(),
        1 => "this 1 line".to_owned(),
        lines => format!("these {lines} lines"),
    };
    let done = match replaces {
        None => format!("Creates this file, with {these}: there was none when {agent} asked."),
        Some(Replaced { bytes: 0, .. }) => format!("Replaces an empty file with {these}."),
        Some(Replaced {
            lines: held,
            bytes,
            more,
        }) => format!(
            "Replaces a file of {}{} ({}) with {these}.",
            if more { "at least " } else { "" },
            counted(held, "line", "lines"),
            counted(bytes, "byte", "bytes"),
        ),
    };
    format!("{done} Measured when {agent} asked; what is there may change before it writes.")
}

/// A change to a file as the box draws it: each line behind the mark a diff
/// gives it, in the colour the Changes column draws that mark in, its marks
/// for white space muted; and for a change with no text at all, one line
/// saying so, in no mark's colour.
pub(super) fn change_lines(request: &AskRequest, exact: bool) -> Vec<Shown> {
    let change = if exact {
        request.change_as_written()
    } else {
        request.change()
    };
    let lines: Vec<Shown> = change
        .into_iter()
        .map(|line| {
            let (mark, prefix) = match line.mark {
                Mark::Removed => (b'-', "- "),
                Mark::Added => (b'+', "+ "),
                Mark::Heading => (b'@', "@@ "),
            };
            Shown {
                text: format!("{prefix}{}", line.text),
                color: super::changes_panel::diff_colour(Some(mark)),
                muted: line
                    .muted
                    .into_iter()
                    .map(|mark| mark.start + prefix.len()..mark.end + prefix.len())
                    .collect(),
            }
        })
        .collect();
    if lines.is_empty() {
        return vec![Shown {
            text: "(empty)".to_owned(),
            color: theme().text_muted,
            muted: Vec::new(),
        }];
    }
    lines
}

/// The file a change is to, whole: in the terminal's family, broken at any
/// character, never cut — the end of a path is the part that names the
/// file — and left to right, as the box's rows are: see [`shape`].
fn file_path(path: String, monospace: FamilyId) -> Box<dyn Element> {
    Container::new(
        crookui_core::elements::Paragraph::new(path, monospace, LABEL_SIZE)
            .with_color(theme().text_primary)
            .breaking_anywhere()
            .left_to_right()
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(6.)
    .finish()
}

/// A line under a request with text in it that is not ASCII, saying so,
/// and a button that shows it as written — every such character written
/// out as `⟨U+XXXX⟩`, muted — or as drawn again.
///
/// A shell's syntax is ASCII, and so is nearly everything a person reads a
/// command or a change for. Text that is not can be drawn to pass for it —
/// a letter that reorders a line, a character a font draws as a quote, a
/// `#`, a `<<` — and no list of such characters is ever whole. Shown as
/// written, what is drawn is printable ASCII and muted marks, which nothing
/// can pass for. So Allow waits until the request has been shown as written
/// and read to its end so; the card's own wait then applies as ever. Once
/// it has, it stays allowed shown either way: it is the same request, and
/// it has been read as what it is.
fn as_written_line(card: &AskCard, ui: FamilyId) -> Box<dyn Element> {
    let chosen = card.chosen == AskButton::AsWritten;
    let state = card
        .handles
        .borrow_mut()
        .for_button(AskButton::AsWritten, true);
    let exact = card.is_as_written();
    let said = match (exact, card.body.waits_for_exact()) {
        (true, true) => "Shown as written · read it to its end to allow",
        (true, false) => "Shown as written: what is not ASCII written out",
        (false, true) => "Contains text that is not ASCII · Show as written to allow",
        (false, false) => "Contains text that is not ASCII",
    };
    let label = if exact {
        "Show as drawn"
    } else {
        "Show as written"
    };
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(
                Expanded::new(
                    1.,
                    Text::new(said, ui, super::tab_menu::PATH_SIZE)
                        .with_color(theme().text_muted)
                        .with_ellipsis(Cut::End)
                        .finish(),
                )
                .finish(),
            )
            .with_child(button(
                state,
                label,
                chosen,
                Some(WorkspaceAction::from(AskAction::AsWritten)),
                ui,
            ))
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(6.)
    .finish()
}

/// One line under the command saying how much of it is shown, and, while
/// Allow waits on it, that the end has to be read first.
///
/// Always one line, whatever it says — a [`Text`] cut at the card's edge, and
/// a space before the box has been laid out — so that nothing below it moves
/// when it learns how long the command is or when the end comes into view:
/// a button that slid under a pointer the moment it turned live would be a
/// press nobody aimed.
fn extent(body: &Body, ui: FamilyId) -> Box<dyn Element> {
    let lines = body.lines.get();
    let top = body.top.get();
    let said = match lines {
        _ if body.measuring.get() => "Measuring the command…".to_owned(),
        _ if body.too_wide() => {
            "A line is drawn wider than this card, so Allow waits: answer in its tab".to_owned()
        }
        0 => " ".to_owned(),
        1 => "1 line".to_owned(),
        lines if lines <= body.shown() => format!("{lines} lines"),
        lines => {
            let shown = format!(
                "Lines {}–{} of {lines}",
                top + 1,
                (top + body.shown()).min(lines)
            );
            if body.read_to_end() {
                shown
            } else {
                format!("{shown} · scroll to the end to allow it")
            }
        }
    };
    Container::new(
        Text::new(said, ui, super::tab_menu::PATH_SIZE)
            .with_color(theme().text_muted)
            .with_ellipsis(Cut::End)
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(6.)
    .finish()
}

/// A note that can change while the card is up — the tab's name, which a
/// rename changes, and how long ago the agent asked, which every frame
/// changes — on one line, cut with a mark where the card runs out.
///
/// A [`note`] wraps, and one of these wrapping onto a second line would push
/// the buttons down under a pointer on its way to them, with nothing to say
/// they had moved and no wait started again.
///
/// Left to right, as the box's rows are — see [`shape`] — since what it
/// names, a tab and a directory, is read in order.
fn one_line(text: String, ui: FamilyId) -> Box<dyn Element> {
    let (text, _) = crookui_core::text_layout::left_to_right(&text);
    Container::new(
        Text::new(text, ui, super::tab_menu::PATH_SIZE)
            .with_color(theme().text_muted)
            .with_ellipsis(Cut::End)
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(4.)
    .finish()
}

/// A line the card says before anything else about the command, in the
/// colour a row says something has gone wrong in.
fn warning(text: String, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        crookui_core::elements::Paragraph::new(text, ui, LABEL_SIZE)
            .with_color(theme().usage_critical)
            .with_style(crookui_core::fonts::Properties {
                weight: crookui_core::fonts::Weight::Semibold,
                ..Default::default()
            })
            .with_line_height_ratio(1.4)
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(6.)
    .finish()
}

/// The buttons: Allow once and Deny outlined at the leading edge, Go to tab,
/// and Not now — or Close, on a question that is over — filled at the
/// trailing edge.
///
/// Whichever the keyboard is on is drawn filled, as the one Enter presses.
/// Allow, Deny and Go to are drawn with no handler until the pointer's wait
/// is over, and Allow until an overflowing command has been read to its end.
fn button_row(
    card: &AskCard,
    face: &Face<'_>,
    pane_open: bool,
    now: Instant,
    ui: FamilyId,
) -> Box<dyn Element> {
    let pointer = card.pointer_is_live(now);
    let chosen = if reachable(card, face, pane_open).contains(&card.chosen) {
        card.chosen
    } else {
        AskButton::NotNow
    };
    let mut handles = card.handles.borrow_mut();
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(8.);
    for button_kind in buttons(face) {
        let live = match button_kind {
            AskButton::NotNow | AskButton::AsWritten => true,
            AskButton::Allow => pointer && card.body.read_to_end(),
            AskButton::Deny => pointer,
            AskButton::GoTo => pointer && pane_open,
        };
        let label = match (button_kind, face) {
            (AskButton::Allow, _) => "Allow once",
            (AskButton::Deny, _) => "Deny",
            (AskButton::GoTo, _) => "Go to tab",
            (AskButton::NotNow, Face::Live(_)) => "Not now",
            (AskButton::NotNow, Face::Withdrawn(_)) => "Close",
            (AskButton::AsWritten, _) => "Show as written",
        };
        let action =
            live.then(|| WorkspaceAction::from(pressing(card, *button_kind, Pressed::Pointer)));
        row.add_child(
            Expanded::new(
                1.,
                button(
                    handles.for_button(*button_kind, live),
                    label,
                    chosen == *button_kind,
                    action,
                    ui,
                ),
            )
            .finish(),
        );
    }
    Container::new(row.finish())
        .with_horizontal_padding(ROW_INSET)
        .with_margin_top(10.)
        .finish()
}

/// One run of the box's text, in one colour: the whole command, or one line
/// of a change.
pub(super) struct Shown {
    /// The text, written out through [`visible`]; a command's has its
    /// newlines in it.
    pub(super) text: String,
    /// What it is drawn in.
    pub(super) color: crookui_core::geometry::Color,
    /// Where in [`Self::text`] the marks drawn muted are, in bytes: the
    /// marks for a change's tabs, a command's leading ones, and the tabs and
    /// spaces either ends a line in.
    pub(super) muted: Vec<std::ops::Range<usize>>,
}

/// The command or the change, broken into rows where the box's width runs
/// out and shown [`BODY_LINES`] at a time, or as many as the height it is
/// given has room for.
///
/// Built here rather than from a [`Scrollable`], which lays out the whole of
/// what it scrolls: a command can be as long as the socket's line, and the
/// card would build every line of it on every frame. This breaks it by how
/// wide each of its graphemes is drawn — measured once each, by the shaper
/// that draws them, not counted — and shapes the rows on screen, once each,
/// and writes into the [`Body`] how many rows there are and whether the last
/// is showing.
///
/// Counting would not do. A character a width table calls one column can be
/// drawn two by the colour-emoji font it falls back to — `⚠` is — and a row
/// of them counted to fit would run past the box's edge, the rest of the
/// command cut off with it, unseen. Nor would measuring characters one at a
/// time: `#` and a keycap mark are one and nothing apart, and two cells
/// together. A row is never broken inside a grapheme, a flag or a conjunct,
/// or inside a mark or a written-out character, and never wider than the box
/// it is drawn in; and should the shaper still draw one wider, it is cut at
/// the edge and the [`Body`] remembers, which keeps Allow waiting: what was
/// not drawn was not read.
///
/// A character no installed face has a glyph for is written out, muted, as
/// `⟨U+XXXX⟩`: every such one is drawn as the same box, which says nothing
/// about which it is. And every row reads left to right, its characters in
/// the order they run — see [`shape`].
///
/// Each row starts after a gutter. A row that goes on with the line before
/// it has [`CONTINUED`] there, muted, and a row that starts a line has
/// nothing, so a line the box broke never reads as a line of its own.
struct CommandBody {
    text: Vec<Shown>,
    body: Rc<Body>,
    family: FamilyId,
    lines: Option<Box<dyn Element>>,
    line_height: f32,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl CommandBody {
    fn new(text: Vec<Shown>, body: Rc<Body>, family: FamilyId) -> Self {
        Self {
            text,
            body,
            family,
            lines: None,
            line_height: 0.,
            size: None,
            origin: None,
        }
    }
}

/// How long one layout may spend shaping graphemes it has not met before.
///
/// A grapheme in the box's own face is read off that face's tables, in
/// microseconds; one that falls back is shaped, and the shaper searches the
/// faces for it. A command of thousands of those would hold the window that
/// long. Past this, the layout guesses — [`GUESSED_CELLS`] — and the next
/// frame measures on, Allow waiting until none is a guess.
const MEASURING_BUDGET: Duration = Duration::from_millis(8);

/// How many cells a grapheme not yet measured is taken to be wide, for each
/// character in it: wide, so that a row broken by a guess is no wider than
/// the box once measured.
const GUESSED_CELLS: f32 = 2.;

/// The line style the box's text is shaped in.
fn line_style() -> crookui_core::fonts::LineStyle {
    use crookui_core::fonts::{DEFAULT_TOP_BOTTOM_RATIO, DEFAULT_UI_LINE_HEIGHT_RATIO, LineStyle};
    LineStyle {
        font_size: LABEL_SIZE,
        line_height_ratio: DEFAULT_UI_LINE_HEIGHT_RATIO,
        baseline_ratio: DEFAULT_TOP_BOTTOM_RATIO,
        fixed_width_tab_size: None,
    }
}

/// `text` shaped as one row of the box, in `family`, with `runs` its styles,
/// or one of the default's when `runs` is empty.
///
/// Left to right whatever letter it starts with, and with no character
/// moved but within a run of right-to-left letters: a shaper would take
/// the row's direction from its first strong letter, and reorder what lies
/// between two right-to-left ones with them — so that `: ع ; rm -rf $HOME
/// # ع` is drawn with `rm` behind the `#`, where a shell runs it. See
/// [`crookui_core::text_layout::layout_left_to_right`].
fn shape(
    text: &str,
    family: FamilyId,
    runs: &[(std::ops::Range<usize>, crookui_core::fonts::StyleAndFont)],
    ctx: &mut LayoutContext,
) -> Line {
    use crookui_core::fonts::{Properties, StyleAndFont, TextStyle};
    let whole = [(
        0..text.len(),
        StyleAndFont::new(family, Properties::default(), TextStyle::default()),
    )];
    crookui_core::text_layout::layout_left_to_right(
        ctx.text_layout,
        text,
        line_style(),
        if runs.is_empty() { &whole } else { runs },
        f32::INFINITY,
    )
}

/// One piece of a run of the box's text that a row is never broken inside:
/// a grapheme, or a whole mark or written-out character, or a newline.
#[derive(Clone, Debug)]
struct Piece {
    /// Where in the run's text it is, in bytes.
    range: std::ops::Range<usize>,
    /// What it is drawn as, when that is not its text: a grapheme some
    /// character of which no face has, its characters written out.
    drawn: Option<String>,
    /// Whether it is drawn muted.
    muted: bool,
    /// Whether it is a newline, which ends a row and is not drawn.
    newline: bool,
    /// How wide it is drawn.
    width: f32,
}

/// Where the pieces of `text` are, in bytes, and which are muted: each of
/// `muted` whole, every other grapheme on its own.
fn pieces(text: &str, muted: &[std::ops::Range<usize>]) -> Vec<(std::ops::Range<usize>, bool)> {
    use unicode_segmentation::UnicodeSegmentation;
    let mut pieces: Vec<(std::ops::Range<usize>, bool)> = Vec::new();
    let mut marks = muted.iter().peekable();
    for (at, grapheme) in text.grapheme_indices(true) {
        while marks.next_if(|mark| mark.end <= at).is_some() {}
        let mark = marks.peek().filter(|mark| mark.start <= at);
        match (pieces.last_mut(), mark) {
            // The rest of the mark the last piece started.
            (Some((range, true)), Some(mark)) if range.start >= mark.start => {
                range.end = at + grapheme.len();
            }
            (_, mark) => pieces.push((at..at + grapheme.len(), mark.is_some())),
        }
    }
    pieces
}

/// The command broken into rows, and what for.
struct Laid {
    /// The family and the room it was broken for.
    key: BrokenFor,
    /// Each run's pieces.
    pieces: Vec<Vec<Piece>>,
    /// The rows.
    rows: Vec<Wrapped>,
    /// Whether some piece's width was guessed.
    guessed: bool,
}

/// The family and the room, in pixels as bits, a command's rows were
/// broken for.
type BrokenFor = (FamilyId, u32);

/// One row of the box: which run of its text it is of, which of that run's
/// pieces it holds, and whether it goes on with the line before it rather
/// than starting one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wrapped {
    run: usize,
    start: usize,
    end: usize,
    continued: bool,
}

/// Where each row of a run starts and ends, by piece, given how wide each
/// piece is and whether it is a newline, broken where the next piece would
/// take it past `room` — and whether the row goes on with the line before
/// it rather than starting one. Every line of it is a row at least, an empty
/// one included, and no row is wider than `room` but one with one piece.
pub(super) fn wrap(pieces: &[(f32, bool)], room: f32) -> Vec<(usize, usize, bool)> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut continued = false;
    let mut width = 0.;
    for (at, &(wide, newline)) in pieces.iter().enumerate() {
        if newline {
            rows.push((start, at, continued));
            start = at + 1;
            continued = false;
            width = 0.;
            continue;
        }
        if width + wide > room && at > start {
            rows.push((start, at, continued));
            start = at;
            continued = true;
            width = 0.;
        }
        width += wide;
    }
    rows.push((start, pieces.len(), continued));
    rows
}

/// How wide graphemes are drawn, from what is kept of them, from the box's
/// own face, or shaped, until the layout's time to shape runs out.
struct Measuring<'a, 'c> {
    family: FamilyId,
    ctx: &'a mut LayoutContext<'c>,
    known: &'a mut std::collections::HashMap<String, Grapheme>,
    until: Instant,
    cell: f32,
    guessed: bool,
}

/// What is kept of one grapheme.
#[derive(Clone, Copy)]
struct Grapheme {
    /// How wide it is drawn.
    width: f32,
    /// Whether some face has a glyph for each of its characters; when not,
    /// its width is not what it is drawn as, which is written out.
    covered: bool,
}

impl Measuring<'_, '_> {
    /// `grapheme`, measured, or `None` when the time to shape it is gone.
    fn grapheme(&mut self, grapheme: &str) -> Option<Grapheme> {
        if let Some(known) = self.known.get(grapheme) {
            return Some(*known);
        }
        let layout = self.ctx.text_layout;
        let mut characters = grapheme.chars();
        let alone = match (characters.next(), characters.next()) {
            (Some(character), None) => Some(character),
            _ => None,
        };
        let in_face = |character: char| {
            layout.advance_in_face(
                self.family,
                crookui_core::fonts::Properties::default(),
                LABEL_SIZE,
                character,
            )
        };
        // Asked before it is shaped: a character no face has is the one a
        // shaper takes longest over, searching every face for it, and it is
        // drawn written out, not as itself. Not yet known — the table of
        // which faces have what still being read — is a width not yet known.
        let (mut covered, mut known) = (true, true);
        for character in grapheme.chars() {
            let covers = match in_face(character) {
                Some(_) => Some(true),
                None => layout.covers(character),
            };
            match covers {
                Some(true) => {}
                Some(false) => covered = false,
                None => known = false,
            }
        }
        let measured = match alone.and_then(in_face) {
            Some(width) => Grapheme {
                width,
                covered: true,
            },
            None if !covered => Grapheme {
                width: 0.,
                covered: false,
            },
            None if !known || Instant::now() >= self.until => {
                self.guessed = true;
                return None;
            }
            None => Grapheme {
                width: shape(grapheme, self.family, &[], self.ctx).width,
                covered: true,
            },
        };
        self.known.insert(grapheme.to_owned(), measured);
        Some(measured)
    }

    /// How wide `text` is drawn: each grapheme of it, guessed when need be.
    fn text(&mut self, text: &str) -> f32 {
        use unicode_segmentation::UnicodeSegmentation;
        text.graphemes(true)
            .map(|grapheme| match self.grapheme(grapheme) {
                Some(measured) => measured.width,
                None => self.guess(grapheme),
            })
            .sum()
    }

    /// The width taken for `grapheme` until it is measured.
    fn guess(&self, grapheme: &str) -> f32 {
        GUESSED_CELLS * self.cell * grapheme.chars().count() as f32
    }

    /// The piece of `text` at `range`, measured, and drawn written out where
    /// no face has a character of it.
    fn piece(&mut self, text: &str, range: std::ops::Range<usize>, muted: bool) -> Piece {
        let piece = &text[range.clone()];
        if piece == "\n" {
            return Piece {
                range,
                drawn: None,
                muted,
                newline: true,
                width: 0.,
            };
        }
        if muted {
            let width = self.text(piece);
            return Piece {
                range,
                drawn: None,
                muted,
                newline: false,
                width,
            };
        }
        match self.grapheme(piece) {
            Some(Grapheme { covered: false, .. }) => {
                let layout = self.ctx.text_layout;
                let drawn: String = piece
                    .chars()
                    .map(|character| {
                        let in_face = layout
                            .advance_in_face(
                                self.family,
                                crookui_core::fonts::Properties::default(),
                                LABEL_SIZE,
                                character,
                            )
                            .is_some();
                        if in_face || layout.covers(character) != Some(false) {
                            character.to_string()
                        } else {
                            format!("⟨U+{:04X}⟩", u32::from(character))
                        }
                    })
                    .collect();
                let width = self.text(&drawn);
                Piece {
                    range,
                    drawn: Some(drawn),
                    muted: true,
                    newline: false,
                    width,
                }
            }
            Some(Grapheme { width, .. }) => Piece {
                range,
                drawn: None,
                muted,
                newline: false,
                width,
            },
            None => Piece {
                range,
                drawn: None,
                muted,
                newline: false,
                width: self.guess(piece),
            },
        }
    }
}

/// The text of `row`, as drawn, and where in it what is muted is, in bytes.
fn row_text(text: &[Shown], laid: &Laid, row: &Wrapped) -> (String, Vec<std::ops::Range<usize>>) {
    let source = &text[row.run].text;
    let mut drawn = String::new();
    let mut muted: Vec<std::ops::Range<usize>> = Vec::new();
    for piece in &laid.pieces[row.run][row.start..row.end] {
        let start = drawn.len();
        drawn.push_str(
            piece
                .drawn
                .as_deref()
                .unwrap_or(&source[piece.range.clone()]),
        );
        if piece.muted {
            match muted.last_mut() {
                Some(last) if last.end == start => last.end = drawn.len(),
                _ => muted.push(start..drawn.len()),
            }
        }
    }
    // A line with nothing on it is still a line of the command.
    if drawn.is_empty() {
        drawn.push(' ');
    }
    (drawn, muted)
}

/// `text` shaped as a row in `family`, its `muted` ranges in the muted
/// colour and the rest in whatever it is painted in.
fn shape_row(
    text: &str,
    muted: &[std::ops::Range<usize>],
    family: FamilyId,
    ctx: &mut LayoutContext,
) -> Line {
    use crookui_core::fonts::{Properties, StyleAndFont, TextStyle};
    let style = |color: Option<crookui_core::geometry::Color>| {
        StyleAndFont::new(
            family,
            Properties::default(),
            TextStyle {
                foreground_color: color,
                ..TextStyle::default()
            },
        )
    };
    let mut runs = Vec::new();
    let mut at = 0;
    for mark in muted {
        if mark.start > at {
            runs.push((at..mark.start, style(None)));
        }
        runs.push((mark.clone(), style(Some(theme().text_muted))));
        at = mark.end;
    }
    if at < text.len() {
        runs.push((at..text.len(), style(None)));
    }
    shape(text, family, &runs, ctx)
}

/// One shaped row, painted in one colour but for the runs it names its own
/// for, and never wider than `room`: a glyph past it is not drawn.
struct Drawn {
    line: Rc<Line>,
    color: crookui_core::geometry::Color,
    room: f32,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Element for Drawn {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        _: &mut LayoutContext,
        _: &AppContext,
    ) -> Vector2F {
        let size = constraint.apply(vec2f(self.line.width.min(self.room), self.line.height()));
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, _: &AppContext) {
        let size = self
            .size
            .expect("a line was painted before it was laid out");
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.line.paint(
            RectF::new(origin, vec2f(size.x().min(self.room), size.y())),
            self.color,
            ctx.scene,
        );
    }

    fn dispatch_event(
        &mut self,
        _: &DispatchedEvent,
        _: &mut EventContext,
        _: &AppContext,
    ) -> bool {
        false
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

impl CommandBody {
    /// The command broken into rows in `room`, as kept from the last layout
    /// when that was for this family and room and guessed at nothing, and
    /// broken again, measuring on, when not.
    fn laid(&self, room: f32, cell: f32, ctx: &mut LayoutContext) -> Rc<Laid> {
        let key = (self.family, room.to_bits());
        if let Some(laid) = &*self.body.laid.borrow()
            && laid.key == key
            && !laid.guessed
        {
            return laid.clone();
        }
        let mut widths = self.body.widths.borrow_mut();
        let known = widths.entry(self.family).or_default();
        let mut measuring = Measuring {
            family: self.family,
            ctx,
            known,
            until: Instant::now() + MEASURING_BUDGET,
            cell,
            guessed: false,
        };
        let mut all = Vec::with_capacity(self.text.len());
        let mut rows = Vec::new();
        for (run, shown) in self.text.iter().enumerate() {
            let run_pieces: Vec<Piece> = pieces(&shown.text, &shown.muted)
                .into_iter()
                .map(|(range, muted)| measuring.piece(&shown.text, range, muted))
                .collect();
            let measured: Vec<(f32, bool)> = run_pieces
                .iter()
                .map(|piece| (piece.width, piece.newline))
                .collect();
            rows.extend(
                wrap(&measured, room)
                    .into_iter()
                    .map(|(start, end, continued)| Wrapped {
                        run,
                        start,
                        end,
                        continued,
                    }),
            );
            all.push(run_pieces);
        }
        let laid = Rc::new(Laid {
            key,
            pieces: all,
            rows,
            guessed: measuring.guessed,
        });
        // A frame that guessed asks for the next, which measures on: the
        // window draws only when asked, and a card left on its guess would
        // say it is measuring, with Allow waiting, for good.
        if self.body.measuring.replace(laid.guessed) != laid.guessed || laid.guessed {
            self.body.news.set(true);
        }
        // The rows drawn of the last breaking are not this one's.
        self.body.shaped.borrow_mut().clear();
        *self.body.laid.borrow_mut() = Some(laid.clone());
        laid
    }

    /// Row `index` of `laid`, shaped, as kept or shaped now — and the box
    /// told, when it is drawn wider than `room`.
    fn shaped_row(
        &self,
        laid: &Laid,
        index: usize,
        room: f32,
        ctx: &mut LayoutContext,
    ) -> Rc<Line> {
        if let Some(line) = self.body.shaped.borrow().get(&index) {
            return line.clone();
        }
        let (text, muted) = row_text(&self.text, laid, &laid.rows[index]);
        let line = Rc::new(shape_row(&text, &muted, self.family, ctx));
        // Half a pixel for the rounding of a sum of advances.
        if line.width > room + 0.5 {
            self.body.note_too_wide();
        }
        self.body.shaped.borrow_mut().insert(index, line.clone());
        line
    }
}

impl Element for CommandBody {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        // One cell's advance, off the face the lines are drawn in, for the
        // gutter and for a box with no width to measure against.
        let cell = ctx
            .text_layout
            .advance_in_face(
                self.family,
                crookui_core::fonts::Properties::default(),
                LABEL_SIZE,
                '0',
            )
            .unwrap_or_else(|| shape("0", self.family, &[], ctx).width)
            .max(1.);
        self.line_height = line_style().font_size * line_style().line_height_ratio;
        let mark = Rc::new(shape(CONTINUED, self.family, &[], ctx));
        let gutter = (mark.width.max(cell) + cell / 2.).ceil();
        let width = constraint.max.x();
        let room = if width.is_finite() {
            (width - THUMB_WIDTH * 2. - gutter).max(cell)
        } else {
            FALLBACK_COLUMNS as f32 * cell
        };

        // As many lines as the card has room for under the window's edge, so
        // that a short window scrolls the command rather than running the
        // card's end, the command's with it, off the bottom of the window.
        let height = constraint.max.y();
        let fits = if height.is_finite() {
            ((height / self.line_height).floor() as usize).clamp(1, BODY_LINES)
        } else {
            BODY_LINES
        };
        let laid = self.laid(room, cell, ctx);
        self.body.measured(laid.rows.len(), fits);
        let top = self.body.top.get().min(self.body.last_top());
        self.body.top.set(top);

        let mut lines = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start);
        let end = (top + fits).min(laid.rows.len());
        for index in top..end {
            let row = laid.rows[index];
            let gutter_mark: Box<dyn Element> = if row.continued {
                Drawn {
                    line: mark.clone(),
                    color: theme().text_muted,
                    room: gutter,
                    size: None,
                    origin: None,
                }
                .finish()
            } else {
                Empty::new().finish()
            };
            let line = self.shaped_row(&laid, index, room, ctx);
            lines.add_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Start)
                    .with_child(ConstrainedBox::new(gutter_mark).with_width(gutter).finish())
                    .with_child(
                        Drawn {
                            line,
                            color: self.text[row.run].color,
                            room,
                            size: None,
                            origin: None,
                        }
                        .finish(),
                    )
                    .finish(),
            );
        }
        let mut lines = lines.finish();
        let shown = lines.layout(
            SizeConstraint {
                min: Vector2F::zero(),
                max: vec2f(width, f32::INFINITY),
            },
            ctx,
            app,
        );
        self.lines = Some(lines);
        let size = constraint.apply(vec2f(
            if width.is_finite() { width } else { shown.x() },
            shown.y(),
        ));
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        let size = self
            .size
            .expect("a command was painted before it was laid out");
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        if let Some(lines) = self.lines.as_mut() {
            lines.paint(origin, ctx, app);
        }
        // The end has been read only once its line was where a person could
        // see it: inside the window, and inside whatever clips the card. The
        // layout knows the box shows it, and not where the box landed; a
        // card taller than a short window has its last rows past the edge.
        if self.body.shows_the_end() {
            let last = self.body.lines.get() - 1 - self.body.top.get();
            let line = RectF::new(
                origin + vec2f(0., last as f32 * self.line_height),
                vec2f(size.x(), self.line_height),
            );
            // Whole, give or take a rounding: half a line cut off is a line
            // somebody may not have read.
            let whole = |seen: Option<RectF>| {
                seen.is_some_and(|seen| {
                    seen.width() >= line.width() - 0.5 && seen.height() >= line.height() - 0.5
                })
            };
            let in_the_window =
                whole(RectF::new(Vector2F::zero(), ctx.window_size).intersection(line));
            let unclipped = whole(ctx.scene.visible_rect(
                Point::from_vec2f(line.origin(), ctx.scene.z_index()),
                line.size(),
            ));
            self.body.note_the_end(in_the_window && unclipped);
        }
        // A thumb, as a list's, so that a box that does not show all of the
        // command says so before anybody scrolls it.
        let total = self.body.lines.get();
        let shown = self.body.shown();
        if total > shown {
            let share = shown as f32 / total as f32;
            let length = (size.y() * share).max(12.).min(size.y());
            let travel = size.y() - length;
            let at = travel * self.body.top.get() as f32 / self.body.last_top().max(1) as f32;
            ctx.scene
                .draw_rect_without_hit_recording(RectF::new(
                    origin + vec2f(size.x() - THUMB_WIDTH, at),
                    vec2f(THUMB_WIDTH, length),
                ))
                .with_background(theme().overlay_2)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(THUMB_WIDTH / 2.)));
        }
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        _: &AppContext,
    ) -> bool {
        let Event::ScrollWheel {
            position, delta, ..
        } = event.raw_event()
        else {
            return false;
        };
        let (Some(origin), Some(size)) = (self.origin, self.size) else {
            return false;
        };
        let over = ctx
            .visible_rect(origin, size)
            .is_some_and(|shown| shown.contains_point(*position))
            && !ctx.is_covered(Point::from_vec2f(*position, origin.z_index()));
        if !over {
            return false;
        }
        // A wheel reports positive-up; the lines count down.
        let pixels = -delta.to_pixels(self.line_height).y();
        let lines = (pixels / self.line_height).round() as isize;
        let lines = if lines == 0 {
            pixels.signum() as isize
        } else {
            lines
        };
        if self.body.scroll_by(lines) {
            ctx.dispatch_typed_action(WorkspaceAction::from(AskAction::Scrolled));
        }
        // Taken at an end as well: the window under a modal card scrolls no
        // more than it hovers.
        true
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    /// The rows `text`, with `muted` its marks, breaks into in `room`
    /// cells, each piece as wide as `advance` says.
    fn broken_marked<'a>(
        text: &'a str,
        muted: &[std::ops::Range<usize>],
        room: usize,
        advance: impl Fn(&str) -> usize,
    ) -> Vec<(&'a str, bool)> {
        let pieces = pieces(text, muted);
        let measured: Vec<(f32, bool)> = pieces
            .iter()
            .map(|(range, _)| {
                let piece = &text[range.clone()];
                (advance(piece) as f32, piece == "\n")
            })
            .collect();
        wrap(&measured, room as f32)
            .into_iter()
            .map(|(start, end, continued)| {
                let from = pieces.get(start).map_or(text.len(), |piece| piece.0.start);
                let to = pieces[..end]
                    .last()
                    .map_or(from, |piece| piece.0.end.max(from));
                (&text[from..to], continued)
            })
            .collect()
    }

    /// The same, with no marks.
    fn broken(text: &str, room: usize, advance: impl Fn(&str) -> usize) -> Vec<(&str, bool)> {
        broken_marked(text, &[], room, advance)
    }

    /// How many cells a terminal's width table counts a piece as.
    fn columns(piece: &str) -> usize {
        piece.width()
    }

    #[test]
    fn a_command_wraps_at_the_room_it_has_and_keeps_its_empty_lines() {
        assert_eq!(
            broken("abcdef", 4, columns),
            [("abcd", false), ("ef", true)]
        );
        assert_eq!(
            broken("ab\n\ncd", 4, columns),
            [("ab", false), ("", false), ("cd", false)]
        );
        assert_eq!(broken("", 4, columns), [("", false)]);
        // A wide character takes two cells, and is never split.
        assert_eq!(
            broken("ab漢字", 4, columns),
            [("ab漢", false), ("字", true)]
        );
        // A row the box broke goes on with the line; one after a newline
        // starts one, however full the row before it was.
        assert_eq!(
            broken("abcd\nefgh", 4, columns),
            [("abcd", false), ("efgh", false)]
        );
        assert_eq!(
            broken("abcdefgh", 4, columns),
            [("abcd", false), ("efgh", true)]
        );
    }

    #[test]
    fn a_row_is_broken_by_how_wide_its_graphemes_are_drawn() {
        // `⚠` is one column to a width table and two cells as the
        // colour-emoji font draws it: a row of them counted would run past
        // the box's edge and take the rest of the command with it.
        let drawn = |piece: &str| if piece == "⚠" { 2 } else { piece.width() };
        let command = "echo ⚠⚠⚠⚠ ; rm";
        assert_eq!(
            broken(command, 8, drawn),
            [("echo ⚠", false), ("⚠⚠⚠ ;", true), (" rm", true)]
        );
        // Counted, the first row would hold all four, drawn twelve wide.
        assert_eq!(broken(command, 8, columns)[0].0, "echo ⚠⚠⚠");
        // A combining mark stays with what it is drawn over.
        assert_eq!(
            broken("abce\u{301}fg", 4, columns),
            [("abce\u{301}", false), ("fg", true)]
        );
        // A keycap is one piece, measured whole: `#` and its mark are one
        // cell and none apart, and two together.
        let keycap = |piece: &str| {
            if piece == "#\u{20e3}" {
                2
            } else {
                piece.width()
            }
        };
        assert_eq!(
            broken("ab#\u{20e3}c", 3, keycap),
            [("ab", false), ("#\u{20e3}c", true)]
        );
        // So is a flag, never broken between its two letters.
        assert_eq!(
            broken("ab\u{1f1fa}\u{1f1f8}", 3, |piece: &str| piece.width()),
            [("ab", false), ("\u{1f1fa}\u{1f1f8}", true)]
        );
    }

    #[test]
    fn a_written_out_character_is_never_broken() {
        let text = "echo a⟨U+200B⟩b";
        let token = 6..text.len() - 1;
        assert_eq!(&text[token.clone()], "⟨U+200B⟩");
        assert_eq!(
            broken_marked(text, &[token], 10, columns),
            [("echo a", false), ("⟨U+200B⟩b", true)]
        );
        // Unmarked, the same characters are text, and broken anywhere.
        assert_eq!(broken(text, 10, columns)[0].0, "echo a⟨U+2");
    }

    #[test]
    fn the_end_is_seen_only_once_the_box_shows_it() {
        let body = Body::default();
        assert!(!body.read_to_end(), "not before it is laid out");
        body.lines.set(BODY_LINES + 5);
        body.note_the_end(true);
        assert!(!body.read_to_end());
        assert!(body.scroll_by(4));
        body.note_the_end(true);
        assert!(!body.read_to_end(), "a line short of the end");
        assert!(body.scroll_by(1));
        assert!(!body.read_to_end(), "scrolled there and not yet drawn");
        // Drawn where the window does not reach, it has not been seen.
        body.note_the_end(false);
        assert!(!body.read_to_end());
        body.note_the_end(true);
        assert!(body.read_to_end());
        // And it stays seen: scrolled back up, the end was on screen once.
        assert!(body.scroll_to(0));
        body.note_the_end(true);
        assert!(body.read_to_end());
        assert!(!body.scroll_by(-1), "nothing above the top");
    }

    #[test]
    fn a_box_with_room_for_fewer_lines_scrolls_by_what_it_shows() {
        let body = Body::default();
        body.lines.set(10);
        body.shown.set(4);
        assert_eq!(body.last_top(), 6);
        assert!(!body.shows_the_end());
        assert!(body.scroll_to(usize::MAX));
        assert_eq!(body.top.get(), 6);
        assert!(body.shows_the_end());
    }
}
