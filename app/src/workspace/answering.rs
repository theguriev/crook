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
//! that the question may already have been answered there.
//!
//! **Not now is the default.** Escape presses it, a press off the card
//! presses it, and so do Enter and Space until the keyboard is moved off it.
//! It is drawn filled at the trailing edge, where the close question keeps
//! Cancel; Allow and Deny are outlined beside it. Not now answers nothing:
//! the agent's own dialog is still up, and the card can be opened again.
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
//! An answer sent closes the card. It never moves on to another question,
//! and it never claims the agent did as it was told: the row's status is left
//! to the agent's next report, which is the truth about what happened. A
//! confined session, for one, drops a hook's allow and keeps its dialog up.
//!
//! # When the question ends under it
//!
//! Its hook went, its time ran out, its turn ended, its pane closed, the
//! agent asked something newer: the card turns to say Claude Code is no
//! longer waiting on it, and why, and leaves only Go to tab — still waiting
//! its second — and Close.

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
use unicode_width::UnicodeWidthChar;

use crate::agent::request::{AskRequest, visible};
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
    /// Whether a frame has learned something about the box since the card
    /// was last built: how many lines the command wraps to, how many the box
    /// shows, or that the end was on screen. The card that frame drew was
    /// built before it knew — Allow inert, the line under the box saying
    /// nothing — and the next has to be asked for; see
    /// `Workspace::frame_drawn`.
    news: Cell<bool>,
}

impl Body {
    /// Whether the command's last line has been on screen, which is never
    /// before it has been laid out once.
    pub(super) fn read_to_end(&self) -> bool {
        self.read_to_end.get()
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
        if on_screen && self.shows_the_end() && !self.read_to_end.replace(true) {
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
        Self {
            ask: ask.id,
            digest: ask.digest,
            pane: ask.pane,
            chosen: AskButton::default(),
            settles_at: Cell::new(now + SETTLE),
            pointer_live_at: now + SETTLE,
            pointer_live: false,
            body: Rc::default(),
            handles: RefCell::default(),
            withdrawn: None,
            back_to_work: false,
        }
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

    /// Puts the keyboard on `button`.
    pub(super) fn choose(&mut self, button: AskButton) {
        self.chosen = button;
    }

    /// The command's box.
    pub(super) fn body(&self) -> &Body {
        &self.body
    }

    /// Starts the keyboard's wait for [`SETTLE`] again, from `now`.
    pub(super) fn unsettle(&self, now: Instant) {
        self.settles_at.set(now + SETTLE);
    }

    /// Whether the keyboard has been still long enough, by `now`.
    pub(super) fn has_settled(&self, now: Instant) -> bool {
        now >= self.settles_at.get()
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

/// The buttons the keyboard can stand on: Allow only once the command has
/// been read to its end, Go to only while its pane is open.
fn reachable(card: &AskCard, face: &Face<'_>, pane_open: bool) -> Vec<AskButton> {
    buttons(face)
        .iter()
        .copied()
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
    let step = |by: isize| {
        let count = reachable.len() as isize;
        reachable[(at as isize + by).rem_euclid(count) as usize]
    };
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
        "tab" if bare => AskAction::Choose(step(1)),
        "tab" if shifted => AskAction::Choose(step(-1)),
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
    if card.has_settled(now) {
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
        Face::Live(_) => format!("{agent} wants to run a command"),
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
            let place = match tab {
                Some(tab) => format!("{} · {}", visible(&tab), visible(&cwd)),
                None => visible(&cwd),
            };
            column.add_child(note(place, ui));
            live_face(&mut column, workspace, card, ask, now, ui);
        }
        Face::Withdrawn(why) => {
            if let Some(tab) = tab {
                column.add_child(note(visible(&tab), ui));
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
    let AskRequest::Command {
        command,
        note: said,
        timeout_ms,
        background,
        outside_sandbox,
    } = &ask.request;
    if *outside_sandbox {
        column.add_child(warning(
            format!("Outside {agent}'s sandbox: it would run with none of the sandbox's limits."),
            ui,
        ));
    }
    // The one part of the card that gives up height in a short window: what
    // the rest leaves is the lines it shows, and it scrolls through the
    // others.
    column.add_child(
        Shrinkable::new(
            1.,
            Container::new(
                CommandBody::new(
                    visible(command),
                    card.body.clone(),
                    workspace.fonts().monospace,
                )
                .finish(),
            )
            .with_horizontal_padding(ROW_INSET)
            .with_margin_bottom(6.)
            .finish(),
        )
        .finish(),
    );
    column.add_child(extent(&card.body, ui));
    if let Some(said) = said {
        column.add_child(note(format!("{agent} says: {}", visible(said)), ui));
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
    column.add_child(divider());
    column.add_child(note(
        format!(
            "{agent} is asking in its tab too · asked {} ago",
            duration(now.saturating_duration_since(ask.asked_at))
        ),
        ui,
    ));
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
            AskButton::NotNow => true,
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

/// The command, wrapped at whichever column the box's width runs out at and
/// shown [`BODY_LINES`] at a time, or as many as the height it is given has
/// room for.
///
/// Built here rather than from a [`Scrollable`], which lays out the whole of
/// what it scrolls: a command can be as long as the socket's line, and the
/// card would build every line of it on every frame. This wraps it — string
/// work, no shaping — and builds the lines on screen, and writes into the
/// [`Body`] how many lines there are and whether the last is showing.
struct CommandBody {
    text: String,
    body: Rc<Body>,
    family: FamilyId,
    lines: Option<Box<dyn Element>>,
    line_height: f32,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl CommandBody {
    fn new(text: String, body: Rc<Body>, family: FamilyId) -> Self {
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

/// Where each line of `text` starts and ends, in bytes, wrapped at `columns`
/// display columns: every line of it at least one, an empty one included.
pub(super) fn wrap(text: &str, columns: usize) -> Vec<(usize, usize)> {
    let columns = columns.max(1);
    let mut lines = Vec::new();
    let mut start = 0;
    let mut width = 0;
    for (at, character) in text.char_indices() {
        if character == '\n' {
            lines.push((start, at));
            start = at + 1;
            width = 0;
            continue;
        }
        let wide = character.width().unwrap_or(0);
        if width + wide > columns && at > start {
            lines.push((start, at));
            start = at;
            width = 0;
        }
        width += wide;
    }
    lines.push((start, text.len()));
    lines
}

impl Element for CommandBody {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        // One cell's advance, measured off the family the lines are drawn in,
        // so the wrap is where the box's edge is.
        let mut probe = Text::new("0".repeat(16), self.family, LABEL_SIZE).finish();
        let measured = probe.layout(
            SizeConstraint {
                min: Vector2F::zero(),
                max: vec2f(f32::INFINITY, f32::INFINITY),
            },
            ctx,
            app,
        );
        let advance = (measured.x() / 16.).max(1.);
        self.line_height = measured.y().max(1.);
        let width = constraint.max.x();
        let columns = if width.is_finite() {
            (((width - THUMB_WIDTH * 2.) / advance).floor() as usize).max(1)
        } else {
            FALLBACK_COLUMNS
        };

        // As many lines as the card has room for under the window's edge, so
        // that a short window scrolls the command rather than running the
        // card's end, the command's with it, off the bottom of the window.
        let room = constraint.max.y();
        let fits = if room.is_finite() {
            ((room / self.line_height).floor() as usize).clamp(1, BODY_LINES)
        } else {
            BODY_LINES
        };
        let wrapped = wrap(&self.text, columns);
        self.body.measured(wrapped.len(), fits);
        let top = self.body.top.get().min(self.body.last_top());
        self.body.top.set(top);

        let mut lines = Flex::column()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Start);
        for (start, end) in wrapped.iter().skip(top).take(fits) {
            let line = &self.text[*start..*end];
            // A line with nothing on it is still a line of the command.
            let line = if line.is_empty() { " " } else { line };
            lines.add_child(
                Text::new(line.to_owned(), self.family, LABEL_SIZE)
                    .with_color(theme().text_primary)
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

    #[test]
    fn a_command_wraps_at_the_columns_it_has_and_keeps_its_empty_lines() {
        assert_eq!(wrap("abcdef", 4), [(0, 4), (4, 6)]);
        assert_eq!(wrap("ab\n\ncd", 4), [(0, 2), (3, 3), (4, 6)]);
        assert_eq!(wrap("", 4), [(0, 0)]);
        // A wide character takes two columns, and is never split.
        let text = "ab漢字";
        let lines: Vec<&str> = wrap(text, 4)
            .into_iter()
            .map(|(start, end)| &text[start..end])
            .collect();
        assert_eq!(lines, ["ab漢", "字"]);
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
