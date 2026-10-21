//! Where the keyboard is on the settings page, and the ring that says so.
//!
//! # Three places, one of them a list
//!
//! The keyboard is in one of three places while the settings are showing:
//! the search box, the rail, or one of the controls on the page. The box is
//! where it starts, because typing is what a person arriving here does
//! first; the rail is one stop, whose arrows move between pages the way a
//! list's do; and every control that would do something if it were clicked
//! is a stop of its own, in the order the page draws them.
//!
//! Tab walks the three in that order and Shift-Tab walks back, and neither
//! wraps — the rule the close question, the agent's card and the tab menu
//! already follow, for their reason: a short list read top to bottom, where
//! a key that jumped from the end to the start is one nobody presses twice
//! with confidence. Up and Down move inside a place, and Down from the box
//! goes straight to the first control, which is the palette's gesture: type
//! what you are looking for, step down onto it, press it. Escape walks back
//! the other way, one place at a time — a control to the box, the box to an
//! empty box, and an empty box out of the settings.
//!
//! # Why the page writes the list down while it is laid out
//!
//! For the reason [`TabContextMenuState`](super::super::tab_context_menu)
//! keeps its rows: the controls come from plugins, the search leaves out
//! whichever rows it did not find, and a change of value can add or take
//! away a row — so nothing outside a frame knows what "the next control" is.
//! The menu records its rows while it is built. This records them while the
//! page is *laid out*, because that is the first moment the search's verdict
//! is in the tree: a row the query filtered away is never laid out, and so
//! never becomes a stop. [`Page`] wraps the page and starts the list;
//! [`Stop`] wraps one control and adds it.
//!
//! The two find each other through a thread-local slot that [`Page`] fills
//! for exactly as long as its own layout and paint run. That is the one way a
//! control built by a free function in `widgets` — which is handed a mouse
//! state and an action, and nothing that knows about a page — can reach the
//! list without every page in every plugin threading one through. Outside a
//! page the slot is empty and a [`Stop`] is a plain wrapper, which is what the
//! same widgets on a plugin's card are.
//!
//! # Which control, by identity
//!
//! A control is known by its mouse state, which is made once per control name
//! the first time the page asks for it and never dropped (see
//! [`SettingsState::control`](super::SettingsState::control)). So the handle's
//! address is a name no other control has, and it is the same on every frame.
//!
//! When the control the keyboard was on is not in the frame that was laid out
//! — the search left it out, or pressing it took its row away — the keyboard
//! goes to whatever is now in its place in the list, which is the nearest
//! thing to "where it was" a list rebuilt from scratch can mean. The ring is
//! drawn where that lands on the same frame, because the list is complete by
//! the time anything is painted.
//!
//! # Every section of the sidebar, not only the settings
//!
//! The Plugins and the Store sections are the same three places: their field
//! is the box, their list is the rail, and the card beside it is the page.
//! So each section has a [`Focus`] of its own, and [`section`](super::super::section)'s
//! two frames put the elements here around what a section hands them: the
//! list in [`Within::list`], whose rows ([`section::row`](super::super::section::row))
//! write themselves down as the rail's pages do, what the sidebar ends in in
//! [`Within::aside`], and the card in a [`Page`]. A section changes nothing about
//! its widgets to be walked: a switch on a plugin's card is a stop because it
//! is laid out inside a page, exactly as one on the Appearance page is.
//!
//! What the sidebar ends in — the Store's "Look for plugins", the Plugins
//! list's "Update all" — is part of the controls, after the card's: Tab from
//! a row goes onto its card, so the card's controls are the ones the
//! keyboard arrives on, and End is the way to the foot of the list. With
//! nothing listed, the foot is all there is, and Tab from the box goes
//! straight to it.
//!
//! Enter on a row of plugins or offers does nothing more than the row: it is
//! chosen already, and its card opens with Install or the plugin's switch
//! first, so an Enter that went onto the card would make a second Enter an
//! install or a plugin switched off. Getting onto a card takes a Tab.
//!
//! # What a plugin draws is never a stop
//!
//! A card's "What it is doing" box is drawn by the plugin, and nothing in it
//! may become something the keyboard presses: a stop runs whatever action
//! its control was built with, and a plugin that drew a host's button —
//! Allow, Remove, Install — inside its own box would have made a key press
//! the host's answer. [`Within::sealed`] lays out and paints what is inside it with
//! no page to report to, so anything in it is a plain wrapper. The plugin's
//! own controls keep the pointer, and whatever keys its own surfaces claim.
//!
//! # A card that becomes another card
//!
//! A press can change what the card is *about*: Remove takes the plugin off
//! the list, and the card is then about the next one, whose controls sit at
//! the places the old one's did. Taking "whatever is now in its place" there
//! would put the keyboard on the next plugin's Remove, and a second Enter —
//! somebody who thought the first one had not landed — would remove that one
//! too. So a [`Page`] says whom it is about, and when that changes under a
//! keyboard on one of its controls, the keyboard goes back to the list,
//! where the new card's subject is the lit row. The same happens when the
//! control the keyboard was on leaves a card that is still about the same
//! plugin: the Store's Remove goes, and Install takes its place.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::{DispatchedEvent, Keystroke, Modifiers};
use crookui_core::geometry::{Point, RectF, Vector2F, vec2f};
use crookui_core::prelude::{Border, CornerRadius, MouseStateHandle, Radius, ScrollStateHandle};
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};

use crate::theme::theme;
use crate::workspace::action::WorkspaceAction;

/// How thick the ring is.
const RING_WIDTH: f32 = 2.;

/// The air between a control and its ring, so that the ring reads as being
/// *around* the control rather than as a border it has grown.
const RING_GAP: f32 = 2.5;

/// How far outside a control its ring reaches.
///
/// No room is kept for it at the top and the bottom of the page: every page
/// starts with a category's heading and every row ends in its own gap, so
/// there is always more than this between a control and the scroll's edge.
const RING_REACH: f32 = RING_WIDTH + RING_GAP;

/// A ring's corners, for a control rounded by `radius`: the control's own
/// curve, carried out to the ring, so the two are concentric.
pub(crate) fn ring_radius(radius: f32) -> Radius {
    Radius::Pixels(radius + RING_REACH)
}

/// Where the keyboard is.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Place {
    /// In the search box, which is where it starts.
    #[default]
    Search,
    /// On the rail, on the page that is showing.
    Rail,
    /// On one of the page's controls.
    Control,
}

/// What a key means on the settings page, before anything has been done
/// about it.
///
/// The action carries this rather than what it does, because what it does
/// depends on the list the last frame recorded, which the key handler reads
/// and the action applies — see [`Workspace::settings_key`](super::super::view::Workspace).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// Tab.
    Next,
    /// Shift-Tab.
    Previous,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    /// Enter or Space.
    Press,
    Escape,
}

/// What a keystroke means where the keyboard is, if it means anything.
///
/// In the box only the keys the box has no use for: Left, Right, Home, End
/// and Space are its caret and its text, and Up would be a key that does
/// nothing anywhere. Enter goes down onto the first control, as Down does,
/// because a field with no Enter of its own would otherwise send it on to
/// whatever is listening behind the page.
///
/// Everywhere else every one of them is taken, whether or not it moves
/// anything: an arrow at the end of the list must not fall through to a
/// binding or a pane any more than the close question's may.
///
/// A character is never taken here, wherever the keyboard is. The box keeps
/// the keys for text the whole time the page has them — it is only drawn as
/// though it had not while the ring is elsewhere — so a letter, a shifted
/// one, a numpad digit, an AltGr character and an input method's word all
/// reach it as they would any field, with the text the key really typed,
/// and the edit puts the keyboard back in it. See
/// [`SettingsAction::SearchEdited`](crate::workspace::SettingsAction::SearchEdited).
pub(crate) fn key_for(place: Place, keystroke: &Keystroke) -> Option<Key> {
    let modifiers = keystroke.modifiers;
    let bare = modifiers.is_empty();
    let shifted = modifiers
        == Modifiers {
            shift: true,
            ..Modifiers::default()
        };
    let key = match keystroke.key.as_str() {
        "tab" if bare => Key::Next,
        "tab" if shifted => Key::Previous,
        _ if !bare => return None,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "enter" | "space" => Key::Press,
        "escape" => Key::Escape,
        _ => return None,
    };
    if place == Place::Search {
        let taken = match key {
            Key::Next | Key::Previous | Key::Down | Key::Escape => true,
            Key::Press => keystroke.key == "enter",
            _ => false,
        };
        if !taken {
            return None;
        }
    }
    Some(key)
}

/// What Enter, Left and Right do to one control.
///
/// Left and Right are for the controls that have a direction: a switch, whose
/// knob they push; a segmented control, whose neighbour they choose; and a
/// stepper, which they step. Everything else is pressed, and only pressed.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct Keys {
    pub(crate) press: Option<WorkspaceAction>,
    pub(crate) left: Option<WorkspaceAction>,
    pub(crate) right: Option<WorkspaceAction>,
}

impl Keys {
    /// Keys that only press.
    pub(crate) fn press(action: Option<WorkspaceAction>) -> Self {
        Self {
            press: action,
            ..Self::default()
        }
    }

    /// Whether no key would do anything, which is a control the keyboard
    /// skips: a greyed one, like the menus' inert entries.
    fn is_inert(&self) -> bool {
        self.press.is_none() && self.left.is_none() && self.right.is_none()
    }
}

/// A control's name for the keyboard: its mouse state's address.
///
/// See the [module](self) for why that is a name.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct StopId(usize);

impl StopId {
    pub(crate) fn of(state: &MouseStateHandle) -> Self {
        Self(Arc::as_ptr(state) as *const () as usize)
    }
}

/// One control the last frame laid out.
struct Recorded {
    id: StopId,
    keys: Keys,
    /// Whether it is at the foot of the sidebar rather than on the page:
    /// after every one of the page's in the list, and in no scroll.
    aside: bool,
    /// Where it was painted, from the top of the page's content; `None`
    /// until it has been.
    top: Option<f32>,
    height: f32,
}

/// One row of the list in the sidebar — a page of the rail, a plugin, an
/// offer — as the last frame laid it out.
struct Row {
    id: StopId,
    /// What choosing it does: the same action a click on it sends.
    choose: Option<WorkspaceAction>,
    /// Whether it is the row the page beside the list is about.
    selected: bool,
    /// Where it was painted, from the top of the list's content.
    top: Option<f32>,
    height: f32,
}

#[derive(Default)]
struct Inner {
    place: Place,
    /// The control the keyboard is on, when it is on one — or `None` for
    /// "whichever is at `index`", which is what arriving on the page from the
    /// palette asks for before the page it arrives on has been laid out.
    on: Option<StopId>,
    /// Where `on` was in the list, which is where the keyboard goes when it
    /// is no longer there.
    index: usize,
    stops: Vec<Recorded>,
    /// Where the page's content began on the last paint, in window
    /// coordinates.
    content_top: f32,
    /// Whether something has changed the page since `stops` was recorded.
    ///
    /// Keys can arrive faster than frames, and a key read against a list
    /// that no longer matches the page would press what the page *was*: a
    /// switch's Right recorded while it was off, pressed a second time once
    /// it is on, turns it back off; Enter after Down on the rail presses the
    /// control of the page that was showing before. So once anything has
    /// changed the page, presses do nothing and moves count places, until the
    /// next layout writes the list down again.
    stale: bool,
    /// Whether the page holds the keyboard at all: not while the Themes
    /// panel, a popup or the Changes column's comment field has it. The ring
    /// is drawn only while it does.
    holds_keys: bool,
    /// The control the ring was drawn on in the last frame, if it was drawn
    /// whole on screen.
    ///
    /// A press lands only there. The keyboard can be moved onto a control
    /// faster than a frame draws the ring around it — Tab from a Store row
    /// goes onto its Install, and an Enter at once would install what nobody
    /// was shown the ring on — or the wheel can scroll it out of sight, and a
    /// key that presses something must press what the person is looking at.
    shown: Option<StopId>,
    /// The list's rows, as the last frame laid them out.
    rows: Vec<Row>,
    /// The row the keyboard has chosen since the list was last laid out, if
    /// it has: a second Down before the frame goes one further than the
    /// first, rather than to the same row again.
    chose: Option<usize>,
    /// Where the list's content began on the last paint, in window
    /// coordinates.
    list_top: f32,
    /// Who the page was about when it was last laid out. See the
    /// [module](self).
    subject: Option<String>,
    /// The two scrolls a step brings what it lands on into, as the section
    /// last handed them over: the list's and the page's.
    list_scroll: Option<ScrollStateHandle>,
    page_scroll: Option<ScrollStateHandle>,
}

impl Inner {
    /// Settles which control the keyboard is on against the list the last
    /// layout recorded: the one it was on if that is still there, or
    /// whatever has taken its place.
    fn resolve(&mut self) {
        if self.place != Place::Control || self.stale {
            return;
        }
        if let Some(at) = self
            .on
            .and_then(|on| self.stops.iter().position(|stop| stop.id == on))
        {
            self.index = at;
            return;
        }
        match self.stops.len() {
            0 => self.on = None,
            count => {
                self.index = self.index.min(count - 1);
                self.on = Some(self.stops[self.index].id);
            }
        }
    }
}

/// Where the keyboard is on the settings page, and every control the last
/// frame offered it.
///
/// Cheap to clone — it is an [`Rc`] — because the element that records into
/// it takes one every frame, as [`RowGeometry`](super::super::tabs_panel::geometry::RowGeometry)'s
/// do.
#[derive(Clone, Default)]
pub(crate) struct Focus(Rc<RefCell<Inner>>);

impl Focus {
    /// Where the keyboard is.
    pub(crate) fn place(&self) -> Place {
        self.0.borrow().place
    }

    /// Puts the keyboard in the box or on the rail.
    pub(crate) fn put(&self, place: Place) {
        let mut inner = self.0.borrow_mut();
        inner.place = place;
        inner.on = None;
        inner.index = 0;
    }

    /// Puts the keyboard on the first control the page draws, which is
    /// settled when it is next laid out if it has not been yet.
    pub(crate) fn first_control(&self) {
        self.put(Place::Control);
        self.0.borrow_mut().resolve();
    }

    /// Puts the keyboard on the last control.
    pub(crate) fn last_control(&self) {
        self.control_at(usize::MAX);
    }

    /// Puts the keyboard on the control at `at`, or the last one where there
    /// are fewer.
    pub(crate) fn control_at(&self, at: usize) {
        let mut inner = self.0.borrow_mut();
        inner.place = Place::Control;
        inner.on = None;
        inner.index = at;
        inner.resolve();
    }

    /// Whether the last frame had a control to put the keyboard on.
    pub(crate) fn has_controls(&self) -> bool {
        !self.0.borrow().stops.is_empty()
    }

    /// Whether the keyboard is on the first control, or on none.
    pub(crate) fn is_on_first(&self) -> bool {
        let mut inner = self.0.borrow_mut();
        inner.resolve();
        inner.index == 0 || (inner.on.is_none() && !inner.stale)
    }

    /// Says the page has changed since its controls were written down. See
    /// `Inner::stale`.
    pub(crate) fn changed(&self) {
        self.0.borrow_mut().stale = true;
    }

    /// Starts the frame being built, saying whether the page holds the
    /// keyboard in it.
    ///
    /// The frame writes every list down afresh: the controls, the rows, and
    /// the control the ring is drawn on. Afresh rather than part by part,
    /// because a part the frame does not draw — a card with nothing to be
    /// about, a foot of the list with nothing to say — lays nothing out, and
    /// what it recorded last time would otherwise stay, live actions and
    /// all, for the keyboard to land on with no ring to show it.
    pub(crate) fn begin_frame(&self, holds: bool) {
        let mut inner = self.0.borrow_mut();
        inner.holds_keys = holds;
        inner.shown = None;
        inner.stops.clear();
        inner.rows.clear();
        inner.chose = None;
        inner.stale = false;
    }

    /// Whether the ring around the rail is drawn: the keyboard is on it and
    /// the page holds the keyboard.
    pub(crate) fn rail_is_lit(&self) -> bool {
        let inner = self.0.borrow();
        inner.place == Place::Rail && inner.holds_keys
    }

    /// Whether the last frame's list had a row to put the keyboard on.
    pub(crate) fn has_rows(&self) -> bool {
        !self.0.borrow().rows.is_empty()
    }

    /// Moves the list's choice by `key` — Up and Down one row, Home and End
    /// to the ends, clamped — and hands back what choosing the row it lands
    /// on does, when it lands on another.
    ///
    /// From the row chosen since the last frame when there is one, so two
    /// Downs before a frame go two rows; otherwise from the row the page is
    /// about.
    pub(crate) fn choose(&self, key: Key) -> Option<WorkspaceAction> {
        let mut inner = self.0.borrow_mut();
        let last = inner.rows.len().checked_sub(1)?;
        let at = inner
            .chose
            .or_else(|| inner.rows.iter().position(|row| row.selected));
        let next = match (key, at) {
            (Key::Home, _) => 0,
            (Key::End, _) => last,
            (Key::Up, Some(at)) => at.saturating_sub(1),
            (Key::Down, Some(at)) => (at + 1).min(last),
            (Key::Up | Key::Down, None) => 0,
            _ => return None,
        };
        if Some(next) == at {
            return None;
        }
        inner.chose = Some(next);
        inner.rows[next].choose
    }

    /// Where the row the keyboard has chosen was painted, from the top of the
    /// list's content.
    fn box_of_row(&self) -> Option<(f32, f32)> {
        let inner = self.0.borrow();
        let at = inner
            .chose
            .or_else(|| inner.rows.iter().position(|row| row.selected))?;
        let row = inner.rows.get(at)?;
        Some((row.top?, row.height))
    }

    /// Hands over the scroll the list is in, and the one the page is in, as
    /// the section builds them.
    pub(crate) fn set_scrolls(
        &self,
        list: Option<ScrollStateHandle>,
        page: Option<ScrollStateHandle>,
    ) {
        let mut inner = self.0.borrow_mut();
        if list.is_some() {
            inner.list_scroll = list;
        }
        if page.is_some() {
            inner.page_scroll = page;
        }
    }

    /// Brings whatever the keyboard is on into its scroll's view, against
    /// where the last frame drew it: the row it chose, or the control. The
    /// tab list's rule — the nearer edge wins, so a walk moves the view one
    /// thing at a time.
    pub(crate) fn scroll_into_view(&self) {
        let (target, scroll) = match self.place() {
            Place::Search => return,
            Place::Rail => (self.box_of_row(), self.0.borrow().list_scroll.clone()),
            Place::Control => (self.box_of_focused(), self.0.borrow().page_scroll.clone()),
        };
        let (Some((top, height)), Some(scroll)) = (target, scroll) else {
            return;
        };
        let mut scroll = scroll.lock();
        let row = crate::workspace::tabs_panel::geometry::RowBox { top, height };
        if let Some(offset) = crate::workspace::tabs_panel::geometry::scroll_for(
            row,
            scroll.offset(),
            scroll.viewport(),
        ) {
            scroll.scroll_to(offset);
        }
    }

    /// Steps the keyboard `by` controls, clamped to the ends, and reports
    /// whether it moved.
    pub(crate) fn step(&self, by: isize) -> bool {
        let mut inner = self.0.borrow_mut();
        if inner.stale {
            // By place: the controls under the old list's names may not be
            // the ones the page will draw, and the place is settled against
            // what it does draw when it next lays out.
            let last = inner.stops.len().saturating_sub(1) as isize;
            let next = (inner.index as isize + by).clamp(0, last) as usize;
            let moved = next != inner.index;
            inner.index = next;
            inner.on = None;
            return moved;
        }
        inner.resolve();
        let Some(on) = inner.on else {
            return false;
        };
        let last = inner.stops.len() as isize - 1;
        let next = (inner.index as isize + by).clamp(0, last) as usize;
        inner.index = next;
        inner.on = Some(inner.stops[next].id);
        inner.on != Some(on)
    }

    /// What the keys do to the control the keyboard is on — nothing until
    /// the ring has been drawn on it. See `Inner::shown`.
    pub(crate) fn keys(&self) -> Option<Keys> {
        let mut inner = self.0.borrow_mut();
        if inner.stale {
            return None;
        }
        inner.resolve();
        let on = inner.on?;
        if inner.shown != Some(on) {
            return None;
        }
        inner
            .stops
            .iter()
            .find(|stop| stop.id == on)
            .map(|stop| stop.keys)
    }

    /// Where the control the keyboard is on was painted, from the top of the
    /// page's content, with its ring.
    pub(crate) fn box_of_focused(&self) -> Option<(f32, f32)> {
        let inner = self.0.borrow();
        if inner.stale {
            return None;
        }
        let on = inner.on?;
        let stop = inner
            .stops
            .iter()
            .find(|stop| stop.id == on && !stop.aside)?;
        Some((stop.top? - RING_REACH, stop.height + 2. * RING_REACH))
    }

    /// Whether the keyboard is on this control.
    fn is_on(&self, id: StopId) -> bool {
        let inner = self.0.borrow();
        inner.place == Place::Control && inner.holds_keys && inner.on == Some(id)
    }

    /// Writes down that the ring was drawn on this control — and whether it
    /// was whole on screen, without which it does not count as shown.
    fn ringed(&self, id: StopId, whole: bool) {
        self.0.borrow_mut().shown = whole.then_some(id);
    }

    /// Starts a layout's list, which is the page as it is now — about
    /// `subject`, when the page says whom it is about.
    fn begin(&self, subject: Option<&str>) {
        let mut inner = self.0.borrow_mut();
        inner.stops.retain(|stop| stop.aside);
        inner.stale = false;
        // A card that is about somebody else now: the keyboard goes back to
        // the list rather than onto whatever stands where its control stood.
        // See the module.
        let moved = match (&inner.subject, subject) {
            (Some(was), Some(now)) => was != now,
            _ => false,
        };
        // Only from a control of the card that went: the keyboard put on
        // "the first control of whatever comes" — Tab after Down, before the
        // frame — is where it was asked to be.
        if moved && inner.place == Place::Control && inner.on.is_some() {
            inner.place = Place::Rail;
            inner.on = None;
            inner.index = 0;
        }
        inner.subject = subject.map(str::to_owned);
    }

    /// Hands the keyboard back to the list when the control it was on is not
    /// on the card any more — a press that took it away: Install becoming
    /// "Getting it…", Remove leaving with the plugin it removed.
    ///
    /// On a card, not on a settings page, where the neighbour takes it. A
    /// card's neighbours are Install, Remove, Allow and Update, and the one
    /// that moved into the place of a control just pressed is the one a
    /// second press — somebody who did not see the first land — would find:
    /// a Remove pressed and Install where it was would put the plugin back.
    fn lost_on_a_card(&self) {
        let mut inner = self.0.borrow_mut();
        let Some(on) = inner.on else {
            return;
        };
        if inner.place == Place::Control && !inner.stops.iter().any(|stop| stop.id == on) {
            inner.place = Place::Rail;
            inner.on = None;
            inner.index = 0;
        }
    }

    /// Starts the list of what the sidebar ends in.
    fn begin_aside(&self) {
        self.0.borrow_mut().stops.retain(|stop| !stop.aside);
    }

    /// Adds one control to it, in the order it was laid out: the page's
    /// before the sidebar's foot.
    ///
    /// Once: a flex may lay a child out more than once in a frame, and the
    /// second time is the same control.
    fn record(&self, id: StopId, keys: Keys, aside: bool) {
        let mut inner = self.0.borrow_mut();
        if inner.stops.iter().any(|stop| stop.id == id) {
            return;
        }
        let stop = Recorded {
            id,
            keys,
            aside,
            top: None,
            height: 0.,
        };
        match aside {
            true => inner.stops.push(stop),
            false => {
                let at = inner.stops.iter().filter(|stop| !stop.aside).count();
                inner.stops.insert(at, stop);
            }
        }
    }

    /// Starts a layout's list of rows.
    fn begin_list(&self) {
        let mut inner = self.0.borrow_mut();
        inner.rows.clear();
        inner.chose = None;
    }

    /// Adds one row to it, in the order it was laid out — once, as a control
    /// is.
    fn record_row(&self, id: StopId, choose: Option<WorkspaceAction>, selected: bool) {
        let mut inner = self.0.borrow_mut();
        if inner.rows.iter().any(|row| row.id == id) {
            return;
        }
        inner.rows.push(Row {
            id,
            choose,
            selected,
            top: None,
            height: 0.,
        });
    }

    /// Writes down where one row was painted.
    fn painted_row(&self, id: StopId, window_top: f32, height: f32) {
        let mut inner = self.0.borrow_mut();
        let top = window_top - inner.list_top;
        if let Some(row) = inner.rows.iter_mut().find(|row| row.id == id) {
            row.top = Some(top);
            row.height = height;
        }
    }

    /// Starts a paint of the list.
    fn begin_list_paint(&self, list_top: f32) {
        self.0.borrow_mut().list_top = list_top;
    }

    /// Writes down where one control was painted.
    fn painted(&self, id: StopId, window_top: f32, height: f32) {
        let mut inner = self.0.borrow_mut();
        let top = window_top - inner.content_top;
        if let Some(stop) = inner
            .stops
            .iter_mut()
            .find(|stop| stop.id == id && !stop.aside)
        {
            stop.top = Some(top);
            stop.height = height;
        }
    }

    /// Starts a paint: settles the keyboard against the list the layout just
    /// recorded, and where the content begins.
    fn begin_paint(&self, content_top: f32) {
        let mut inner = self.0.borrow_mut();
        inner.content_top = content_top;
        inner.resolve();
    }
}

/// Which part of a section is being laid out or painted.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Region {
    /// The page beside the list, whose controls are stops.
    Page,
    /// What the sidebar ends in under its list, whose controls are stops
    /// after the page's.
    Aside,
    /// The list, whose rows are the rail's.
    List,
}

thread_local! {
    /// The section being laid out or painted, and which part of it, while
    /// one is. See the [module](self).
    static SCOPE: RefCell<Option<(Focus, Region)>> = const { RefCell::new(None) };
}

/// Runs `f` with `scope` as where a [`Stop`] reports to — `None` for
/// nowhere — and puts back whatever was there before.
fn within<T>(scope: Option<(&Focus, Region)>, f: impl FnOnce() -> T) -> T {
    let scope = scope.map(|(focus, region)| (focus.clone(), region));
    let before = SCOPE.with(|slot| slot.replace(scope));
    let result = f();
    SCOPE.with(|slot| *slot.borrow_mut() = before);
    result
}

/// The section whose part is being laid out or painted, and which part.
fn current() -> Option<(Focus, Region)> {
    SCOPE.with(|slot| slot.borrow().clone())
}

/// Draws a ring around `bounds`.
///
/// Without hit recording: the ring is an indicator, and a press on it belongs
/// to whatever is under it.
fn draw_ring(ctx: &mut PaintContext, bounds: RectF, radius: Radius, within: bool) {
    let reach = if within { 0. } else { RING_REACH };
    let outer = RectF::new(
        bounds.origin() - vec2f(reach, reach),
        bounds.size() + vec2f(2. * reach, 2. * reach),
    );
    ctx.scene
        .draw_rect_without_hit_recording(outer)
        .with_border(Border::all(RING_WIDTH).with_border_color(theme().accent))
        .with_corner_radius(CornerRadius::with_all(radius));
}

/// Wraps the page's rows: starts the list of controls each layout, and
/// settles the keyboard against it before each paint.
///
/// Inside the scroll, around the rows and nothing else, so that a control's
/// offset is measured from the top of what scrolls. It takes no room of its
/// own.
pub(crate) struct Page {
    focus: Focus,
    /// Whom the page is about, for a page that is about somebody: a plugin's
    /// card, an offer's. See the [module](self).
    subject: Option<String>,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Page {
    pub(crate) fn new(focus: Focus, child: Box<dyn Element>) -> Self {
        Self {
            focus,
            subject: None,
            child,
            size: None,
            origin: None,
        }
    }

    /// The same page, about `subject`.
    pub(crate) fn about(mut self, subject: Option<String>) -> Self {
        self.subject = subject;
        self
    }
}

impl Element for Page {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.focus.begin(self.subject.as_deref());
        let child = &mut self.child;
        let size = within(Some((&self.focus, Region::Page)), || {
            child.layout(constraint, ctx, app)
        });
        if self.subject.is_some() {
            self.focus.lost_on_a_card();
        }
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.focus.begin_paint(origin.y());
        let child = &mut self.child;
        within(Some((&self.focus, Region::Page)), || {
            child.paint(origin, ctx, app)
        });
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// Whether a [`Stop`] is one of the page's controls, a row of the list, or a
/// ring that is lit or not by somebody who already knows.
enum Lit {
    /// A control: added to the page's list, and lit when the keyboard is on it.
    Control(StopId, Keys),
    /// A row of the sidebar's list: added to the list's rows, and lit when it
    /// is the selected one and the keyboard is on the list.
    Row {
        id: StopId,
        choose: Option<WorkspaceAction>,
        selected: bool,
    },
    /// Lit or not, and in nobody's list: what a surface that keeps its own
    /// list of stops draws its ring with — the Changes column's — told
    /// whether the ring it drew was whole on screen, which is when that
    /// surface may let a press land.
    Fixed(bool, Rc<dyn Fn(bool)>),
}

/// One control the keyboard can stand on, and the ring drawn around it when
/// it does.
///
/// The ring is painted outside the control's box and takes no room, so moving
/// the keyboard moves nothing on the page.
pub(crate) struct Stop {
    lit: Lit,
    radius: Radius,
    /// Whether the ring is drawn on the control's own edge rather than
    /// around it: for a row as wide as the page, whose ring outside it would
    /// be cut by the edge of the scroll.
    within: bool,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Stop {
    fn boxed(lit: Lit, radius: Radius, within: bool, child: Box<dyn Element>) -> Box<dyn Element> {
        Box::new(Self {
            lit,
            radius,
            within,
            child,
            size: None,
            origin: None,
        })
    }

    /// A control known by `state`, which `keys` press, rounded by `radius`.
    pub(crate) fn control(
        state: &MouseStateHandle,
        keys: Keys,
        radius: Radius,
        child: Box<dyn Element>,
    ) -> Box<dyn Element> {
        Self::boxed(Lit::Control(StopId::of(state), keys), radius, false, child)
    }

    /// A control as wide as the page, whose ring is drawn on its own edge,
    /// rounded by `radius` as the control is.
    pub(crate) fn row(
        state: &MouseStateHandle,
        keys: Keys,
        radius: f32,
        child: Box<dyn Element>,
    ) -> Box<dyn Element> {
        Self::boxed(
            Lit::Control(StopId::of(state), keys),
            Radius::Pixels(radius),
            true,
            child,
        )
    }

    /// A row of the sidebar's list, known by `state`, which `choose` chooses,
    /// rounded by `radius`: lit while it is `selected` and the keyboard is on
    /// the list.
    pub(crate) fn list_row(
        state: &MouseStateHandle,
        choose: Option<WorkspaceAction>,
        selected: bool,
        radius: f32,
        child: Box<dyn Element>,
    ) -> Box<dyn Element> {
        Self::boxed(
            Lit::Row {
                id: StopId::of(state),
                choose,
                selected,
            },
            Radius::Pixels(radius),
            true,
            child,
        )
    }

    /// A ring rounded by `radius`, lit when `lit` says so — around the child,
    /// or on its own edge when `within` — telling `seen` after each paint of
    /// a lit ring whether the whole of it was on screen.
    pub(crate) fn fixed(
        lit: bool,
        radius: Radius,
        within: bool,
        seen: Rc<dyn Fn(bool)>,
        child: Box<dyn Element>,
    ) -> Box<dyn Element> {
        Self::boxed(Lit::Fixed(lit, seen), radius, within, child)
    }
}

impl Element for Stop {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        match (&self.lit, current()) {
            (Lit::Control(id, keys), Some((page, region @ (Region::Page | Region::Aside))))
                if !keys.is_inert() =>
            {
                page.record(*id, *keys, region == Region::Aside);
            }
            (
                Lit::Row {
                    id,
                    choose,
                    selected,
                },
                Some((page, Region::List)),
            ) => page.record_row(*id, *choose, *selected),
            _ => {}
        }
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let size = self.size.unwrap_or_default();
        self.child.paint(origin, ctx, app);

        // Whether the control is whole inside the clip it is painted in: a
        // ring scrolled out of sight, or cut by the scroll's edge, is a ring
        // nobody is looking at, and a press must not land under it.
        let whole = || {
            let point = Point::from_vec2f(origin, ctx.scene.z_index());
            ctx.scene
                .visible_rect(point, size)
                .is_some_and(|shown| (shown.height() - size.y()).abs() < 0.5)
        };
        let lit = match (&self.lit, current()) {
            (Lit::Fixed(lit, seen), _) => {
                if *lit {
                    seen(whole());
                }
                *lit
            }
            (Lit::Control(id, _), Some((page, Region::Page | Region::Aside))) => {
                page.painted(*id, origin.y(), size.y());
                let on = page.is_on(*id);
                if on {
                    page.ringed(*id, whole());
                }
                on
            }
            (Lit::Row { id, selected, .. }, Some((page, Region::List))) => {
                page.painted_row(*id, origin.y(), size.y());
                *selected && page.rail_is_lit()
            }
            _ => false,
        };
        if lit {
            draw_ring(ctx, RectF::new(origin, size), self.radius, self.within);
        }
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }
}

/// What a section is laid out in, other than its page: its list, what the
/// sidebar ends in, or something a plugin drew that must report to nobody.
enum Part {
    /// Starts the list of rows each layout. Inside the list's scroll, so a
    /// row's offset is measured from the top of what scrolls.
    List(Focus),
    /// Starts the list of controls at the sidebar's foot each layout.
    Aside(Focus),
    /// Lays out and paints with no section to report to. See the
    /// [module](self).
    Sealed,
}

/// One of those parts around `child`, taking no room of its own.
pub(crate) struct Within {
    part: Part,
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Within {
    fn boxed(part: Part, child: Box<dyn Element>) -> Box<dyn Element> {
        Box::new(Self {
            part,
            child,
            size: None,
            origin: None,
        })
    }

    /// The sidebar's list, whose rows are the rail's.
    pub(crate) fn list(focus: &Focus, child: Box<dyn Element>) -> Box<dyn Element> {
        Self::boxed(Part::List(focus.clone()), child)
    }

    /// What the sidebar ends in, whose controls come after the page's.
    pub(crate) fn aside(focus: &Focus, child: Box<dyn Element>) -> Box<dyn Element> {
        Self::boxed(Part::Aside(focus.clone()), child)
    }

    /// Something a plugin drew, which nothing the keyboard presses may be in.
    pub(crate) fn sealed(child: Box<dyn Element>) -> Box<dyn Element> {
        Self::boxed(Part::Sealed, child)
    }

    /// Where a [`Stop`] inside reports to, as [`within`] takes it.
    fn scope(&self) -> Option<(Focus, Region)> {
        match &self.part {
            Part::List(focus) => Some((focus.clone(), Region::List)),
            Part::Aside(focus) => Some((focus.clone(), Region::Aside)),
            Part::Sealed => None,
        }
    }
}

impl Element for Within {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        match &self.part {
            Part::List(focus) => focus.begin_list(),
            Part::Aside(focus) => focus.begin_aside(),
            Part::Sealed => {}
        }
        let scope = self.scope();
        let child = &mut self.child;
        let size = within(
            scope.as_ref().map(|(focus, region)| (focus, *region)),
            || child.layout(constraint, ctx, app),
        );
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        if let Part::List(focus) = &self.part {
            focus.begin_list_paint(origin.y());
        }
        let scope = self.scope();
        let child = &mut self.child;
        within(
            scope.as_ref().map(|(focus, region)| (focus, *region)),
            || child.paint(origin, ctx, app),
        );
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        self.child.dispatch_event(event, ctx, app)
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

    fn stroke(key: &str, modifiers: Modifiers) -> Keystroke {
        Keystroke::new(key, modifiers)
    }

    #[test]
    fn the_box_keeps_its_caret_keys_and_its_space() {
        for key in ["left", "right", "home", "end", "space", "up"] {
            assert_eq!(
                key_for(Place::Search, &stroke(key, Modifiers::default())),
                None,
                "{key} is the box's"
            );
            assert!(
                key_for(Place::Control, &stroke(key, Modifiers::default())).is_some(),
                "{key} is the page's on a control"
            );
        }
        assert_eq!(
            key_for(Place::Search, &stroke("enter", Modifiers::default())),
            Some(Key::Press)
        );
    }

    #[test]
    fn a_character_is_the_box_s_wherever_the_keyboard_is() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        for place in [Place::Search, Place::Rail, Place::Control] {
            assert_eq!(key_for(place, &stroke("a", Modifiers::default())), None);
            assert_eq!(key_for(place, &stroke("a", shift)), None);
            assert_eq!(
                key_for(place, &stroke("backspace", Modifiers::default())),
                None
            );
        }
    }

    #[test]
    fn a_chord_is_nobody_s_but_shift_tab() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let cmd = Modifiers {
            cmd: true,
            ..Modifiers::default()
        };
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        for place in [Place::Search, Place::Rail, Place::Control] {
            assert_eq!(key_for(place, &stroke("tab", ctrl)), None);
            assert_eq!(key_for(place, &stroke("down", cmd)), None);
            assert_eq!(key_for(place, &stroke("enter", shift)), None);
            assert_eq!(key_for(place, &stroke("tab", shift)), Some(Key::Previous));
        }
    }
}
