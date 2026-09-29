//! The question a close asks before it ends agents that are still working.
//!
//! # Why a close asks at all
//!
//! Every agent in Crook is a child of the window. A pane's pty hangs its
//! child up when it goes and kills it a quarter of a second later — see
//! `crook_terminal::pty` — so closing the window, a tab, a group or a pane is
//! also ending whatever was running there: a conversation halfway through a
//! turn, a build, an `ssh` session. The title bar's close, the desktop's, a
//! row's × and `cmd-w` all used to do that without a word, and in a terminal
//! whose work is agents one stray press was the end of an afternoon's
//! context.
//!
//! What counts as working is
//! [`AgentSession::is_working`](crate::tab::AgentSession::is_working), which
//! says why it is two facts rather than one. A close that would end nothing
//! working goes at once, as it always did: a window of shells at their
//! prompts is not something anybody needs to be asked about, and a question
//! that came up on every close would be one a person learns to press through.
//!
//! # The question
//!
//! A card in the middle of the window, in the face the worktree removal asks
//! with — its header, its notes, its hairline and its pair of buttons, drawn
//! by the same functions in [`tab_menu`](super::tab_menu). It says how many
//! are working, names up to three of them, and says what the close does to
//! them.
//!
//! **Cancel is the default.** Enter presses it, Escape presses it, and so does
//! a press anywhere off the card. The button that ends something must never
//! be what a person gets by pressing Enter to be rid of a box they did not
//! read, which is the rule every desktop's guidelines give for a destructive
//! question — and the opposite of the worktree confirmation, whose Enter
//! removes. That question is asked by a pointer aimed at one checkout; this
//! one can be asked by the desktop closing a window nobody was looking at,
//! while somebody's hands were on the keyboard. The default button sits at
//! the trailing edge, where the worktree face keeps its own.
//!
//! **The keyboard can still say yes.** Tab and Shift-Tab move it between the
//! two buttons, the left arrow onto the one that ends them and the right
//! arrow back onto Cancel, and Enter or Space presses whichever it is on. The
//! button it is on is the one drawn filled — the face the worktree question
//! gives the button its Enter presses — so the fill moves with the keyboard
//! and says, before anything is pressed, what Enter will do. Without it a
//! person with no pointer could never close a pane running `ssh` or a busy
//! agent short of turning the question off; with it, ending one still takes
//! a deliberate move first.
//!
//! **But not with keys typed before it was read.** A close chord pressed by
//! mistake mid-word leaves hands still typing, and Tab is every shell's
//! completion key while a Space or an Enter is what follows one: `lo<Tab> -l`
//! would be a move onto End and a press of it. So the way to End opens only
//! once the keyboard has been still for [`SETTLE`] — after the card comes up,
//! after the window takes the keyboard (a desktop can hand it over mid-word,
//! see below), and after any key the card has no use for, since that is
//! somebody typing at something else. Until then Tab, the arrows, and Enter
//! or Space on End are taken and do nothing, and each one starts the wait
//! again. Escape, and Enter or Space on Cancel, work at once: a way out never
//! waits. Firefox holds the buttons of its install and download prompts for a
//! moment (`security.dialog_enable_delay`) against the same typed-ahead
//! press.
//!
//! **The panes under it hear nothing.** Any other popup leaves the focused
//! pane its interrupt, end and suspend keys (see `Keys::Signals`), because a
//! menu with no Escape must not make a `sleep 600` uninterruptible. This one
//! has an Escape, and it is asking whether to end the very program those keys
//! reach: a reflexive `ctrl-c` to get out of the card would interrupt the
//! agent's turn it is protecting, and a second one, or `ctrl-d` at its
//! prompt, would end it. So the focused pane gets no keys at all while it is
//! up — see `body::panel`.
//!
//! **A close from the desktop brings the window forward.** A taskbar's
//! "Close window" or a window manager's close can reach a window that is
//! minimised or on another workspace, and a card drawn there would be a close
//! that seemed to do nothing. A window close that asks therefore restores a
//! minimised window and, if the window does not have the keyboard, asks the
//! desktop for the person's attention — see
//! [`WindowControls::bring_forward`](crate::window_controls::WindowControls::bring_forward).
//! It asks rather than takes: a close can come from a script while somebody
//! is typing in another application, and a window that seized the keyboard
//! would be handed the rest of what they typed. Where restoring it, or the
//! desktop's answer to the request, hands it the keyboard anyway, the
//! keyboard's wait starts again when it does.
//!
//! It holds the close it is waiting to do and the panes that were working
//! when it asked, and nothing else about them: the names are read off the
//! strip on every frame, so an agent that renames its work while the card is
//! up is named as it is now. A pane whose shell ends while it is up is taken
//! off it, since there is nothing left there to end, and a question with
//! nobody left on it goes — see [`Question::forget_closed`].
//!
//! # What cannot ask
//!
//! Anything that ends the process from outside it. `SIGTERM`, a logout that
//! kills rather than closes, a power cut: none of them runs a line of Crook
//! first, so there is nobody to put a card up. On macOS the application
//! menu's Quit — and `cmd-q`, its key equivalent, which AppKit takes before
//! the window sees a key — is the same case for a different reason: winit's
//! menu sends `terminate:`, and winit answers that with
//! `applicationWillTerminate:` and no `applicationShouldTerminate:`, so there
//! is no moment in which to say no. Taking it over needs a menu of Crook's own
//! and an Objective-C dependency the rest of the window has no use for yet.
//! The window's own close — the traffic light, `alt-f4`, a window manager's
//! close — does arrive, as winit's `CloseRequested`, and it is routed here
//! through the window delegate rather than ending the event loop.
//!
//! A run nobody is sitting at never sees the card either. The headless
//! snapshot's quit is a no-op, so its frames are drawn and written whatever
//! is up; a windowed run with a frame budget exits through its own budget
//! and answers a desktop's close at once. See `Shell::close_requested`.

use std::cell::Cell;
use std::time::{Duration, Instant};

use crookui_core::elements::{MouseStateHandle, WINDOW_INSET};
use crookui_core::event::{Keystroke, Modifiers};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::keybindings::Recording;
use crate::tab::{Pane, PaneId, TabAction, TabStrip};
use crate::theme::theme;

use super::action::{EndingAction, EndingButton, WorkspaceAction};
use super::tab_menu::{ROW_INSET, button, divider, header, note};
use super::view::Workspace;
use super::window_room::WindowRoom;

/// How wide the card is.
///
/// Wider than the worktree menu's 260, because the two buttons share the row
/// half and half and "End them and close" has to fit in a half.
const CARD_WIDTH: f32 = 320.;

/// The least the card is narrowed to in a window narrower than it.
const CARD_LEAST_WIDTH: f32 = 240.;

/// The card's corner radius, which is every menu's in this application.
const CARD_RADIUS: f32 = 6.;

/// How many working panes the card names before it counts the rest.
///
/// Three: enough to recognise which work it means, few enough that the card
/// is read rather than scanned. The count in the header is the number that
/// matters, and it is always the whole of it.
const NAMED: usize = 3;

/// How long the keyboard has to have been still before a key can reach the
/// button that ends them.
///
/// A second: longer than the gap between two keys of somebody still typing,
/// so hands that carry on after a stray close keep the way to End shut, and
/// about what reading the card's first line takes anyway. What it costs
/// somebody who meant it is a Tab pressed inside that second doing nothing;
/// the card says how many are working and names them, and that is worth the
/// second it takes to read.
const SETTLE: Duration = Duration::from_secs(1);

/// What a close that stopped to ask is waiting to do.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum Close {
    /// Quit, which ends every pane in the window.
    Window,
    /// One of the strip's own closes — a tab, a pane or a group — held
    /// until it is answered and then applied exactly as it was asked.
    Strip(TabAction),
}

impl Close {
    /// The close a strip action is, if it is one.
    ///
    /// Three of them, and they are every way a gesture takes panes away. A
    /// shell that exits closes its pane through [`TabAction::ClosePane`] as
    /// well, but not through here: it is already gone, and there is nothing
    /// left to ask about.
    pub(super) fn of(action: TabAction) -> Option<Self> {
        matches!(
            action,
            TabAction::Close(_) | TabAction::ClosePane(_) | TabAction::CloseGroup(_)
        )
        .then_some(Self::Strip(action))
    }

    /// Every pane this close would end, in the order the strip holds them.
    fn ends(self, strip: &TabStrip) -> Vec<&Pane> {
        match self {
            Self::Window => strip.panes().map(|(_, pane)| pane).collect(),
            Self::Strip(TabAction::Close(tab)) => strip
                .get(tab)
                .into_iter()
                .flat_map(|tab| tab.panes().iter())
                .collect(),
            Self::Strip(TabAction::ClosePane(pane)) => strip.pane(pane).into_iter().collect(),
            Self::Strip(TabAction::CloseGroup(group)) => strip
                .members(group)
                .flat_map(|tab| tab.panes().iter())
                .collect(),
            Self::Strip(_) => Vec::new(),
        }
    }

    /// The panes among those that are working, which is who the question is
    /// about.
    pub(super) fn working(self, strip: &TabStrip) -> Vec<PaneId> {
        self.ends(strip)
            .into_iter()
            .filter(|pane| pane.session().is_working())
            .map(Pane::id)
            .collect()
    }

    /// Whether this close takes every pane in the window, which is quitting
    /// however it was asked for.
    ///
    /// The strip never empties: the last tab closing — by its ×, by its last
    /// pane, by its group's — takes the window instead. A card that said
    /// "close" over a press that was about to quit would be wrong about the
    /// one thing it is there to say.
    fn quits(self, strip: &TabStrip) -> bool {
        self == Self::Window || self.ends(strip).len() == strip.panes().count()
    }
}

/// The question, while it is up.
pub(super) struct Question {
    /// The close it is holding.
    pub(super) close: Close,
    /// The panes that were working when it asked, in strip order.
    working: Vec<PaneId>,
    /// Cancel's mouse state. Fresh with every question, like [`Self::end`],
    /// so a card never comes up with a button lit under a pointer that has
    /// since gone somewhere else.
    cancel: MouseStateHandle,
    /// The mouse state of the button that ends them.
    end: MouseStateHandle,
    /// The button the keyboard is on, which is the one Enter and Space press
    /// and the one drawn filled. Cancel, until Tab or an arrow moves it.
    chosen: EndingButton,
    /// When a key may first move the keyboard onto the button that ends them,
    /// or press it: [`SETTLE`] after the card came up, after the window last
    /// took the keyboard, or after the last key that was typed too soon or
    /// meant nothing to the card, whichever was latest.
    ///
    /// A cell because the keystroke being asked about is what moves it, and
    /// the window asks about each keystroke exactly once — the same terms the
    /// half-typed chord sequence is written down on.
    settles_at: Cell<Instant>,
}

impl Question {
    /// Asks about `close`, if anything it would end is working.
    ///
    /// `None` is the ordinary answer: nothing is working, and the close goes
    /// at once. `now` is when it comes up, which is where the keyboard's wait
    /// for [`SETTLE`] starts.
    pub(super) fn about(close: Close, strip: &TabStrip, now: Instant) -> Option<Self> {
        let working = close.working(strip);
        (!working.is_empty()).then(|| Self {
            close,
            working,
            cancel: MouseStateHandle::default(),
            end: MouseStateHandle::default(),
            chosen: EndingButton::default(),
            settles_at: Cell::new(now + SETTLE),
        })
    }

    /// Starts the keyboard's wait for [`SETTLE`] again, from `now`.
    pub(super) fn unsettle(&self, now: Instant) {
        self.settles_at.set(now + SETTLE);
    }

    /// Whether the keyboard has been still long enough, by `now`, for a key
    /// to reach the button that ends them.
    fn has_settled(&self, now: Instant) -> bool {
        now >= self.settles_at.get()
    }

    /// Lets the keyboard reach End from now on, as though it had been still
    /// for [`SETTLE`] — which a test says this way rather than by sleeping
    /// through it.
    #[cfg(test)]
    pub(super) fn settle(&self) {
        self.settles_at.set(Instant::now());
    }

    /// The button the keyboard is on.
    pub(super) fn chosen(&self) -> EndingButton {
        self.chosen
    }

    /// Puts the keyboard on `button`.
    pub(super) fn choose(&mut self, button: EndingButton) {
        self.chosen = button;
    }

    /// The panes it is asking about.
    pub(super) fn working(&self) -> &[PaneId] {
        &self.working
    }

    /// Takes off the panes that have closed since it asked, and says whether
    /// anybody is left.
    ///
    /// A pane goes from under the card when its shell exits. What it was
    /// running has ended already, so the card has one fewer thing to warn
    /// about; one that has nothing left to warn about is a question whose
    /// premise is gone, and it is taken down rather than left saying "0
    /// agents".
    pub(super) fn forget_closed(&mut self, strip: &TabStrip) -> bool {
        self.working.retain(|pane| strip.pane(*pane).is_some());
        !self.working.is_empty()
    }
}

/// What a keystroke means while the question is up, at `now`.
///
/// Escape is Cancel wherever the keyboard is. Enter and Space press the
/// button it is on, which starts as Cancel: Enter is Cancel rather than
/// nothing then, because a key that did nothing under a question would read
/// as a window that had stopped answering. Tab and Shift-Tab move it to the
/// other button, and the arrows move it the way the buttons sit — the one
/// that ends them on the left, Cancel on the right.
///
/// Every key but the ways to Cancel waits for the keyboard to settle first.
/// Until it has, Tab, the arrows, and Enter or Space on End are
/// [`EndingAction::TooSoon`] — taken, so that they reach nothing under the
/// card either, and nothing done — and each of them starts the wait again.
/// So does any other key: it falls through as it would over any popup, but it
/// is somebody typing at something that is not the card. A modifier held on
/// its own is neither, since Shift is how Shift-Tab is reached. The pane under
/// the card is not listening to any of them. See the [module](self).
pub(super) fn action_for(
    question: &Question,
    keystroke: &Keystroke,
    now: Instant,
) -> Option<WorkspaceAction> {
    if Recording::is_a_modifier(keystroke) {
        return None;
    }
    let modifiers = keystroke.modifiers;
    let bare = modifiers.is_empty();
    let shifted = modifiers
        == Modifiers {
            shift: true,
            ..Modifiers::default()
        };
    let action = match keystroke.key.as_str() {
        "escape" if bare => return Some(EndingAction::Cancel.into()),
        "enter" | "space" if bare => match question.chosen() {
            EndingButton::Cancel => return Some(EndingAction::Cancel.into()),
            EndingButton::End => EndingAction::End,
        },
        "tab" if bare || shifted => EndingAction::Choose(question.chosen().other()),
        "left" if bare => EndingAction::Choose(EndingButton::End),
        "right" if bare => EndingAction::Choose(EndingButton::Cancel),
        _ => {
            question.unsettle(now);
            return None;
        }
    };
    if question.has_settled(now) {
        return Some(action.into());
    }
    question.unsettle(now);
    Some(EndingAction::TooSoon.into())
}

/// The card's first line.
pub(super) fn title(count: usize) -> String {
    match count {
        1 => "1 agent is still working".to_owned(),
        many => format!("{many} agents are still working"),
    }
}

/// What the close does to them, in one sentence.
pub(super) fn consequence(close: Close, quits: bool, count: usize) -> String {
    let closing = match close {
        _ if quits => "Quitting Crook",
        Close::Strip(TabAction::Close(_)) => "Closing the tab",
        Close::Strip(TabAction::ClosePane(_)) => "Closing the pane",
        Close::Strip(TabAction::CloseGroup(_)) => "Closing the group",
        Close::Window | Close::Strip(_) => "Quitting Crook",
    };
    let them = if count == 1 { "it" } else { "them" };
    format!("{closing} ends {them}.")
}

/// The label on the button that ends them.
pub(super) fn end_label(quits: bool, count: usize) -> &'static str {
    match (quits, count) {
        (true, 1) => "End it and quit",
        (true, _) => "End them and quit",
        (false, 1) => "End it and close",
        (false, _) => "End them and close",
    }
}

/// The names the card gives, and how many it leaves to the count.
///
/// Each pane by its own name — what a person called it, else what the agent
/// calls its work, else what it is running. A pane that is working nearly
/// always has one of the three, and it is the name the panel beside the card
/// leads its row with unless the person asked the rows to lead with the
/// directory or the branch instead.
pub(super) fn names(strip: &TabStrip, working: &[PaneId]) -> (Vec<String>, usize) {
    let open: Vec<&Pane> = working
        .iter()
        .filter_map(|pane| strip.pane(*pane))
        .collect();
    let named = open
        .iter()
        .take(NAMED)
        .map(|pane| pane.title().to_owned())
        .collect();
    (named, open.len().saturating_sub(NAMED))
}

/// The whole card, over a modal underlay that answers a press off it with
/// Cancel.
pub(super) fn render(workspace: &Workspace, question: &Question) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let strip = workspace.tabs();
    let count = question.working().len();
    let quits = question.close.quits(strip);
    let (named, more) = names(strip, question.working());

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header(title(count), ui));
    for name in &named {
        column.add_child(note(name, ui));
    }
    if more > 0 {
        column.add_child(note(format!("and {more} more."), ui));
    }
    // The worktree face's seam between the names and the sentence about
    // them, for its reason: without it the list runs into the sentence in
    // the same size and the same tone.
    column.add_child(divider());
    column.add_child(note(consequence(question.close, quits, count), ui));
    column.add_child(buttons(question, end_label(quits, count), ui));

    let card = WindowRoom::new(
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
    .finish();

    // Centred across the window by a spacer each side, as the palette is,
    // and a third of the way down rather than half: a question sits where
    // the eye already is, above the middle, and the palette's own card
    // starts in the same band.
    let centred = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(card)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .finish();
    let placed = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(centred)
        .with_child(Expanded::new(2., Empty::new().finish()).finish())
        .finish();

    Dismiss::new(placed)
        // The rest of the window is inert while it is up. A tab that could
        // be clicked behind a question about closing it would be a way to
        // answer the question without reading it.
        .modal()
        .on_dismiss(|ctx, _| ctx.dispatch_typed_action(WorkspaceAction::from(EndingAction::Cancel)))
        .finish()
}

/// The button that ends them, and Cancel at the trailing edge.
///
/// Whichever the keyboard is on is drawn filled, as the one Enter presses:
/// Cancel, until Tab or an arrow moves it.
fn buttons(question: &Question, label: &str, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(
                Expanded::new(
                    1.,
                    button(
                        question.end.clone(),
                        label,
                        question.chosen() == EndingButton::End,
                        Some(EndingAction::End.into()),
                        ui,
                    ),
                )
                .finish(),
            )
            .with_child(
                Expanded::new(
                    1.,
                    button(
                        question.cancel.clone(),
                        "Cancel",
                        question.chosen() == EndingButton::Cancel,
                        Some(EndingAction::Cancel.into()),
                        ui,
                    ),
                )
                .finish(),
            )
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_top(10.)
    .finish()
}
