//! The list of every question the window holds: the other face of the
//! popup an agent's question is answered on.
//!
//! # Why a list
//!
//! A person with ten agents in ten tabs wants the questions one gesture away
//! without leaving the tab they are in, and without visiting the tabs in turn
//! to find which is asking what. "Answer waiting agent", the header's "to
//! answer" and its chord open this when more than one question is held, and
//! the card in [`answering`](super::answering) when one is.
//!
//! # What a row can do
//!
//! **Open its card, and nothing else.** A row has no Allow and no Deny, and
//! neither its Enter nor its press is an answer: either opens the card about
//! that question, which starts as every card starts — fresh mouse state, the
//! keyboard's [`SETTLE`](super::closing::SETTLE) begun and the pointer gate
//! shut — so a press or a key that opened it cannot go on to answer on it.
//! Escape on the card comes back here, and Escape here closes.
//!
//! **Nothing moves by itself.** The rows stand in the order the window took
//! the questions, oldest first, and a new one is added at the end, under
//! the rows already there rather than above them. A row whose question ends
//! goes, and the keyboard moves to the row after it. An answer sent on a card
//! opened from here brings the list back and opens nothing; the next card is
//! a person's choice, and the keys of the one that closed are kept from the
//! list until the keyboard has been still, so a held Enter does not open it.
//! A question arriving opens nothing either: the list is drawn again with one
//! more row, if it is up.
//!
//! Below the rows, read-only, is how the last [`EARLIER`] questions ended,
//! newest first. A list with no question left in it closes.
//!
//! Every key is the list's while it is up, as every key is the card's: Up and
//! Down move between the rows, Enter opens one, Escape closes, and anything
//! else goes nowhere, so a key meant for a pane does not reach the pane of an
//! agent that is asking.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Instant;

use crookui_core::elements::WINDOW_INSET;
use crookui_core::event::Keystroke;
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::agent::request::visible;
use crate::control::ask::{Ask, AskId, Asks, Cause, Choice, How, Outcome};
use crate::keybindings::Recording;
use crate::theme::theme;

use super::action::{AskAction, WorkspaceAction};
use super::answering::duration;
use super::tab_menu::{LABEL_SIZE, PATH_SIZE, ROW_INSET, divider, header, note};
use super::view::Workspace;
use super::window_room::WindowRoom;

/// How many ended questions the list shows under its rows.
pub(super) const EARLIER: usize = 20;

/// How wide the list is: a summary of fifty-odd characters.
const LIST_WIDTH: f32 = 420.;

/// The least the list is narrowed to in a window narrower than it.
const LIST_LEAST_WIDTH: f32 = 220.;

/// The least the list is shortened to in a window shorter than it.
const LIST_LEAST_HEIGHT: f32 = 160.;

/// The list's corner radius, which is the card's and every menu's.
const LIST_RADIUS: f32 = 6.;

/// How tall a row is, its two lines and the air around them. Fixed, so that
/// the keyboard's row can be scrolled into view without measuring anything.
const ROW_HEIGHT: f32 = 40.;

/// How tall the list's scrolled part may be, at most: room for six rows and
/// a few of the earlier ones. A shorter window has less, and the rest scroll.
const LIST_HEIGHT: f32 = 360.;

/// The list, while it is up — or under the card it opened, until that card
/// goes and the list is drawn again.
pub(super) struct AskList {
    /// The row the keyboard was last on, by its question; see
    /// [`Self::selected`] for what it is while that question is over.
    selected: Cell<Option<AskId>>,
    /// The rows' mouse state, by question: none kept from one showing of the
    /// list to the next, so that a press begun on the card that was over it
    /// cannot be finished on a row.
    handles: RefCell<HashMap<AskId, MouseStateHandle>>,
    /// Where its rows and the earlier ones are scrolled to.
    scroll: ScrollStateHandle,
}

impl AskList {
    /// A list, with the keyboard on its first row.
    pub(super) fn new() -> Self {
        Self {
            selected: Cell::new(None),
            handles: RefCell::default(),
            scroll: ScrollStateHandle::default(),
        }
    }

    /// The row the keyboard is on, by `asks` as they are now: the one it was
    /// put on while that question is held, the row after it once it is over,
    /// or the last row when none comes after, and the first before any.
    pub(super) fn selected(&self, asks: &Asks) -> Option<AskId> {
        let held = rows(asks);
        match self.selected.get() {
            Some(kept) if held.iter().any(|ask| ask.id == kept) => Some(kept),
            Some(kept) => held
                .iter()
                .find(|ask| ask.id > kept)
                .or(held.last())
                .map(|ask| ask.id),
            None => held.first().map(|ask| ask.id),
        }
    }

    /// Writes down the row the keyboard is on now, by `asks`: what keeps it
    /// on the row after one whose question ended, rather than on whichever
    /// question is asked next after that.
    pub(super) fn follow(&self, asks: &Asks) {
        self.selected.set(self.selected(asks));
    }

    /// Puts the keyboard on question `id`'s row, and scrolls the row into
    /// view.
    pub(super) fn pick(&self, id: AskId, asks: &Asks) {
        let Some(at) = rows(asks).iter().position(|ask| ask.id == id) else {
            return;
        };
        self.selected.set(Some(id));
        let top = at as f32 * ROW_HEIGHT;
        let mut scroll = self.scroll.lock();
        let viewport = match scroll.viewport() {
            measured if measured > 0. => measured.min(LIST_HEIGHT),
            _ => LIST_HEIGHT,
        };
        let offset = scroll.offset();
        if top < offset {
            scroll.scroll_to(top);
        } else if top + ROW_HEIGHT > offset + viewport {
            scroll.scroll_to(top + ROW_HEIGHT - viewport);
        }
    }

    /// Forgets the rows' mouse state, for the list coming back from under
    /// the card it opened.
    pub(super) fn forget_hover_state(&self) {
        self.handles.borrow_mut().clear();
    }

    /// The mouse state of question `id`'s row.
    fn handle(&self, id: AskId) -> MouseStateHandle {
        self.handles.borrow_mut().entry(id).or_default().clone()
    }
}

/// The list's rows: every question held, in the order the window took them.
///
/// By number, which is the order the window took them in — a question's is
/// handed out as it is held, and its `asked_at` is that moment — and which a
/// question arriving later can never come before.
pub(super) fn rows(asks: &Asks) -> Vec<&Ask> {
    let mut held: Vec<&Ask> = asks.live().collect();
    held.sort_by_key(|ask| ask.id);
    held
}

/// What a keystroke means while the list is up.
///
/// Every key is taken, as on the card, but for a modifier held on its own:
/// Up and Down move, stopping at the ends; Enter opens the row's card;
/// Escape closes; anything else is [`AskAction::TooSoon`], and goes nowhere.
pub(super) fn action_for(
    list: &AskList,
    asks: &Asks,
    keystroke: &Keystroke,
) -> Option<WorkspaceAction> {
    if Recording::is_a_modifier(keystroke) {
        return None;
    }
    let bare = keystroke.modifiers.is_empty();
    let held = rows(asks);
    let at = list
        .selected(asks)
        .and_then(|id| held.iter().position(|ask| ask.id == id));
    let action = match (keystroke.key.as_str(), at) {
        ("escape", _) if bare => AskAction::NotNow,
        ("up", Some(at)) if bare => AskAction::Pick(held[at.saturating_sub(1)].id),
        ("down", Some(at)) if bare => AskAction::Pick(held[(at + 1).min(held.len() - 1)].id),
        ("enter", Some(at)) if bare => AskAction::Open(held[at].id),
        _ => AskAction::TooSoon,
    };
    Some(action.into())
}

/// The whole list, over a modal underlay that closes it when pressed.
pub(super) fn render(workspace: &Workspace, list: &AskList) -> Box<dyn Element> {
    let fonts = workspace.fonts();
    let ui = fonts.ui;
    let now = Instant::now();
    let asks = workspace.asks();
    let held = rows(asks);
    let selected = list.selected(asks);

    let mut body = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for ask in &held {
        body.add_child(row(
            workspace,
            ask,
            selected == Some(ask.id),
            list.handle(ask.id),
            now,
            fonts.monospace,
            ui,
        ));
    }
    let ended: Vec<&Outcome> = asks.history().collect();
    let earlier: Vec<&Outcome> = ended.into_iter().rev().take(EARLIER).collect();
    if !earlier.is_empty() {
        body.add_child(divider());
        body.add_child(header("Earlier", ui));
        for outcome in earlier {
            body.add_child(one_line(earlier_line(outcome, now), ui));
        }
    }

    let count = held.len();
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(header(
            match count {
                1 => "1 request waiting, oldest first".to_owned(),
                count => format!("{count} requests waiting, oldest first"),
            },
            ui,
        ))
        // The one part that gives up height in a short window, and scrolls
        // instead: the header above and the hint below never move.
        .with_child(
            Shrinkable::new(
                1.,
                ConstrainedBox::new(
                    Scrollable::new(list.scroll.clone(), body.finish())
                        .with_scrollbar(theme().overlay_3)
                        .finish(),
                )
                .with_max_height(LIST_HEIGHT)
                .finish(),
            )
            .finish(),
        );
    column.add_child(divider());
    column.add_child(note(
        "Up and Down to choose, Enter to open its card, Escape to close.",
        ui,
    ));

    let list_element = WindowRoom::new(
        ConstrainedBox::new(
            Container::new(column.finish())
                .with_background_color(theme().surface_raised)
                .with_border(Border::all(1.).with_border_color(theme().overlay_2))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(LIST_RADIUS)))
                .with_vertical_padding(6.)
                .finish(),
        )
        .with_width(LIST_WIDTH)
        .finish(),
    )
    .with_width_inset(WINDOW_INSET, LIST_LEAST_WIDTH)
    .with_height_inset(WINDOW_INSET, LIST_LEAST_HEIGHT)
    .finish();

    // Where the card is placed, so that a row's card comes up where the list
    // was and the eye does not have to go looking for it.
    let centred = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(list_element)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .finish();
    let placed = Flex::column()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(Expanded::new(1., Empty::new().finish()).finish())
        .with_child(centred)
        .with_child(Expanded::new(2., Empty::new().finish()).finish())
        .finish();

    Dismiss::new(placed)
        .modal()
        .on_dismiss(|ctx, _| ctx.dispatch_typed_action(WorkspaceAction::from(AskAction::NotNow)))
        .finish()
}

/// One question's row: who asks and in which tab, what it asks, how long
/// ago, and, for one this window can only show, that it is read-only — in
/// words, since a colour alone says nothing to somebody who cannot tell it.
///
/// Its press opens the question's card, and that is its one handler.
fn row(
    workspace: &Workspace,
    ask: &Ask,
    selected: bool,
    hover: MouseStateHandle,
    now: Instant,
    monospace: FamilyId,
    ui: FamilyId,
) -> Box<dyn Element> {
    // The tab as it is called now, read off the strip as the card reads it.
    let strip = workspace.tabs();
    let tab = strip
        .panes()
        .find(|(_, open)| open.id() == ask.pane)
        .and_then(|(tab, _)| strip.get(tab))
        .map(|tab| visible(tab.name()));
    let who = match tab {
        Some(tab) => format!("{} · {tab}", ask.agent.name()),
        None => ask.agent.name().to_owned(),
    };
    let mut after = format!(
        " · {}",
        duration(now.saturating_duration_since(ask.asked_at))
    );
    if ask.cannot_answer.is_some() {
        after.push_str(" · read-only");
    }
    let summary = ask.summary.clone();
    let open = WorkspaceAction::from(AskAction::Open(ask.id));

    Hoverable::new(hover, move |mouse| {
        let ground = match (selected, mouse.is_hovered()) {
            (true, _) => theme().overlay_2,
            (false, true) => theme().overlay_1,
            (false, false) => Color::TRANSPARENT,
        };
        // The name and the tab give way first, cut with a mark; the age and
        // the read-only mark after them are never cut off.
        let first = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    1.,
                    Text::new(who.clone(), ui, LABEL_SIZE)
                        .with_color(theme().text_primary)
                        .with_ellipsis(Cut::End)
                        .finish(),
                )
                .finish(),
            )
            .with_child(
                Text::new(after.clone(), ui, PATH_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            )
            .finish();
        let second = Text::new(summary.clone(), monospace, PATH_SIZE)
            .with_color(theme().text_muted)
            .with_ellipsis(Cut::End)
            .finish();
        ConstrainedBox::new(
            Container::new(
                Flex::column()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::Center)
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(3.)
                    .with_child(first)
                    .with_child(second)
                    .finish(),
            )
            .with_background_color(ground)
            .with_horizontal_padding(ROW_INSET)
            .finish(),
        )
        .with_height(ROW_HEIGHT)
        .finish()
    })
    .on_click(move |_, ctx, _| ctx.dispatch_typed_action(open))
    .finish()
}

/// What the list says about a question that ended: how, who asked, the tab
/// as it was called then, what it asked and how long ago, in that order, so
/// that how it ended is the part a narrow list never cuts.
fn earlier_line(outcome: &Outcome, now: Instant) -> String {
    let how = match outcome.how {
        How::Answered(Choice::Allow) => "Allow sent",
        How::Answered(Choice::Deny) => "Deny sent",
        How::Ended(cause) => ended(cause),
    };
    let mut line = format!("{how} · {}", outcome.agent.name());
    if !outcome.tab_title.is_empty() {
        line.push_str(&format!(" · {}", visible(&outcome.tab_title)));
    }
    format!(
        "{line} · {} · {} ago",
        outcome.summary,
        duration(now.saturating_duration_since(outcome.ended_at))
    )
}

/// How a question nobody answered in Crook ended, in the fewest words.
fn ended(cause: Cause) -> &'static str {
    match cause {
        Cause::AnsweredInAgent => "Answered in its tab",
        Cause::HookGone => "Hook stopped waiting",
        Cause::Expired => "Timed out",
        Cause::PaneClosed => "Tab closed",
        Cause::CommandEnded => "Agent exited",
        Cause::TurnEnded => "Turn ended",
        Cause::AskedAgain => "Asked again",
        Cause::WindowClosed => "Window closed",
        Cause::AnsweringOff => "Answering turned off",
    }
}

/// A line that never wraps, cut with a mark where the list runs out.
fn one_line(text: String, ui: FamilyId) -> Box<dyn Element> {
    Container::new(
        Text::new(text, ui, PATH_SIZE)
            .with_color(theme().text_muted)
            .with_ellipsis(Cut::End)
            .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_margin_bottom(4.)
    .finish()
}

/// Questions to hold for a picture of the list and the card, read from the
/// JSON file `crook-dev --ask-fixture` names: the same questions on every
/// machine, with no agent asking and no hook waiting.
///
/// ```json
/// {
///   "asks": [
///     { "tab": "crook", "command": "cargo test --workspace", "note": "Run the tests" },
///     { "tab": "site", "command": "rm -rf dist", "read_only": true }
///   ],
///   "earlier": [{ "tab": "api", "command": "make lint", "ended": "allowed" }]
/// }
/// ```
///
/// Each `tab` is a tab by that name, the first the window's own and the rest
/// opened for it, and each question is asked in the pane it shows. `ended`
/// says how one of the earlier ones went: `allowed`, `denied`, or the word a
/// pass carries as its why, `expired` and the rest.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskFixture {
    /// The questions still held, in the order they are asked.
    #[serde(default)]
    asks: Vec<FixtureAsk>,
    /// The questions that ended, in the order they did.
    #[serde(default)]
    earlier: Vec<FixtureAsk>,
}

/// One question of an [`AskFixture`].
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureAsk {
    /// The tab it is asked in, by name.
    tab: String,
    /// The command it asks to run.
    command: String,
    /// What the agent says the command does.
    #[serde(default)]
    note: Option<String>,
    /// Whether this window can only show it.
    #[serde(default)]
    read_only: bool,
    /// Whether it would run outside the agent's sandbox.
    #[serde(default)]
    outside_sandbox: bool,
    /// How it ended, for one of the earlier ones.
    #[serde(default)]
    ended: Option<String>,
}

impl AskFixture {
    /// Reads the fixture at `path`, or says why it is not one.
    pub fn read(path: &std::path::Path) -> anyhow::Result<Self> {
        use anyhow::Context as _;
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let fixture: Self = serde_json::from_str(&text)
            .with_context(|| format!("{} is not a fixture of questions", path.display()))?;
        if let Some(word) = fixture
            .earlier
            .iter()
            .filter_map(|ask| ask.ended.as_deref())
            .find(|word| how(word).is_none())
        {
            anyhow::bail!("{}: {word:?} is not how a question ends", path.display());
        }
        Ok(fixture)
    }

    /// Holds its questions in `workspace`, opening and naming the tabs they
    /// are asked in, and ends the earlier ones as they ended. Opens nothing
    /// else: the list and the card are put up the way a person puts them up,
    /// with `--action crook/tabs/answer-waiting`.
    pub fn seed(&self, workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>) {
        use crate::agent::request::AskRequest;
        use crate::control::ask::{Delivery, Found};
        use crate::control::protocol::{AgentAsk, AskingAgent, Verb};
        use crate::control::watch::Feed;
        use crate::tab::TabAction;

        let mut tabs: Vec<(String, crate::tab::PaneId)> = Vec::new();
        let asked = self.earlier.iter().chain(&self.asks).enumerate();
        for (index, fixture) in asked {
            let pane = match tabs.iter().find(|(name, _)| *name == fixture.tab) {
                Some((_, pane)) => *pane,
                None => {
                    if !tabs.is_empty() {
                        workspace.handle_action(&WorkspaceAction::Tab(TabAction::New), ctx);
                    }
                    let Some(tab) = workspace.tabs().active().map(|tab| tab.id()) else {
                        return;
                    };
                    workspace.rename_tab(tab, Some(fixture.tab.clone()), ctx);
                    let Some(pane) = workspace.tabs().focused_pane_id() else {
                        return;
                    };
                    tabs.push((fixture.tab.clone(), pane));
                    pane
                }
            };
            let cwd = workspace
                .home()
                .map(|home| home.join("work").join(&fixture.tab))
                .unwrap_or_else(|| std::path::PathBuf::from(&fixture.tab));
            let asking = AgentAsk {
                agent: AskingAgent::Claude,
                session: format!("fixture-{index}"),
                cwd: cwd.to_string_lossy().into_owned(),
                tty: 0,
                wait: 300,
                request: AskRequest::Command {
                    command: fixture.command.clone(),
                    note: fixture.note.clone(),
                    timeout_ms: None,
                    background: false,
                    outside_sandbox: fixture.outside_sandbox,
                },
            };
            let digest = asking.digest();
            let feed = std::sync::Arc::new(Feed::for_verb(&Verb::AgentAsk(asking.clone())));
            let found = Found {
                pane,
                cannot_answer: fixture
                    .read_only
                    .then_some(crate::control::ask::CannotAnswer::NotFromThisTab),
                tab_title: fixture.tab.clone(),
            };
            let id = match workspace
                .asks_mut()
                .add(asking, found, feed, Instant::now())
            {
                Ok(id) => id,
                Err(refused) => {
                    log::warn!(
                        "the fixture's question in {:?} was refused: {refused:?}",
                        fixture.tab
                    );
                    continue;
                }
            };
            match fixture.ended.as_deref().and_then(how) {
                Some(How::Answered(choice)) => {
                    if workspace.asks_mut().deliver(id, &digest, choice) != Delivery::Delivered {
                        log::warn!("the fixture's answer in {:?} was not taken", fixture.tab);
                    }
                }
                Some(How::Ended(cause)) => workspace.asks_mut().end(id, cause),
                None => {}
            }
        }
        workspace.asks_changed(ctx);
    }
}

/// How a question ended, by the word a fixture says it with.
fn how(word: &str) -> Option<How> {
    const CAUSES: [Cause; 9] = [
        Cause::HookGone,
        Cause::Expired,
        Cause::PaneClosed,
        Cause::CommandEnded,
        Cause::TurnEnded,
        Cause::AskedAgain,
        Cause::AnsweredInAgent,
        Cause::WindowClosed,
        Cause::AnsweringOff,
    ];
    match word {
        "allowed" => Some(How::Answered(Choice::Allow)),
        "denied" => Some(How::Answered(Choice::Deny)),
        word => CAUSES
            .into_iter()
            .find(|cause| cause.word() == word)
            .map(How::Ended),
    }
}
