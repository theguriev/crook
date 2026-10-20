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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::event::{DispatchedEvent, Keystroke, Modifiers};
use crookui_core::geometry::{Point, RectF, Vector2F, vec2f};
use crookui_core::prelude::{Border, CornerRadius, MouseStateHandle, Radius};
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
    /// A character typed while the keyboard was off the box: it goes back to
    /// the box, and the character with it, so typing to search works from
    /// anywhere on the page.
    Type(char),
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
/// binding or a pane any more than the close question's may. And so is a
/// character, which goes back to the box — see [`Key::Type`].
pub(crate) fn key_for(place: Place, keystroke: &Keystroke) -> Option<Key> {
    let modifiers = keystroke.modifiers;
    let bare = modifiers.is_empty();
    let shifted = modifiers
        == Modifiers {
            shift: true,
            ..Modifiers::default()
        };
    let typing = !modifiers.ctrl && !modifiers.cmd && !modifiers.alt;
    let mut chars = keystroke.key.chars();
    if let (Some(character), None) = (chars.next(), chars.next())
        && typing
        && !character.is_control()
    {
        // In the box the field types it itself.
        return (place != Place::Search).then_some(Key::Type(character));
    }
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
    /// Where it was painted, from the top of the page's content; `None`
    /// until it has been.
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
}

impl Inner {
    /// Settles which control the keyboard is on against the list the last
    /// layout recorded: the one it was on if that is still there, or
    /// whatever has taken its place.
    fn resolve(&mut self) {
        if self.place != Place::Control {
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
        inner.on.is_none() || inner.index == 0
    }

    /// Steps the keyboard `by` controls, clamped to the ends, and reports
    /// whether it moved.
    pub(crate) fn step(&self, by: isize) -> bool {
        let mut inner = self.0.borrow_mut();
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

    /// What the keys do to the control the keyboard is on.
    pub(crate) fn keys(&self) -> Option<Keys> {
        let mut inner = self.0.borrow_mut();
        inner.resolve();
        let on = inner.on?;
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
        let on = inner.on?;
        let stop = inner.stops.iter().find(|stop| stop.id == on)?;
        Some((stop.top? - RING_REACH, stop.height + 2. * RING_REACH))
    }

    /// Whether the keyboard is on this control.
    fn is_on(&self, id: StopId) -> bool {
        let inner = self.0.borrow();
        inner.place == Place::Control && inner.on == Some(id)
    }

    /// Starts a layout's list.
    fn begin(&self) {
        self.0.borrow_mut().stops.clear();
    }

    /// Adds one control to it, in the order it was laid out.
    ///
    /// Once: a flex may lay a child out more than once in a frame, and the
    /// second time is the same control.
    fn record(&self, id: StopId, keys: Keys) {
        let mut inner = self.0.borrow_mut();
        if inner.stops.iter().any(|stop| stop.id == id) {
            return;
        }
        inner.stops.push(Recorded {
            id,
            keys,
            top: None,
            height: 0.,
        });
    }

    /// Writes down where one control was painted.
    fn painted(&self, id: StopId, window_top: f32, height: f32) {
        let mut inner = self.0.borrow_mut();
        let top = window_top - inner.content_top;
        if let Some(stop) = inner.stops.iter_mut().find(|stop| stop.id == id) {
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

thread_local! {
    /// The page being laid out or painted, while one is. See the
    /// [module](self).
    static PAGE: RefCell<Option<Focus>> = const { RefCell::new(None) };
}

/// Runs `f` with `focus` as the page a [`Stop`] reports to, and puts back
/// whatever was there before.
fn within<T>(focus: &Focus, f: impl FnOnce() -> T) -> T {
    let before = PAGE.with(|page| page.replace(Some(focus.clone())));
    let result = f();
    PAGE.with(|page| *page.borrow_mut() = before);
    result
}

/// The page whose controls are being laid out or painted, if any.
fn current() -> Option<Focus> {
    PAGE.with(|page| page.borrow().clone())
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
    child: Box<dyn Element>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl Page {
    pub(crate) fn new(focus: Focus, child: Box<dyn Element>) -> Self {
        Self {
            focus,
            child,
            size: None,
            origin: None,
        }
    }
}

impl Element for Page {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.focus.begin();
        let child = &mut self.child;
        let size = within(&self.focus, || child.layout(constraint, ctx, app));
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.focus.begin_paint(origin.y());
        let child = &mut self.child;
        within(&self.focus, || child.paint(origin, ctx, app));
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

/// Whether a [`Stop`] is one of the page's controls, or a ring that is lit or
/// not by somebody who already knows.
enum Lit {
    /// A control: added to the page's list, and lit when the keyboard is on it.
    Control(StopId, Keys),
    /// Lit or not, and in nobody's list: the rail's row, which is one stop
    /// for the whole rail.
    Fixed(bool),
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
    /// A control known by `state`, which `keys` press, rounded by `radius`.
    pub(crate) fn control(
        state: &MouseStateHandle,
        keys: Keys,
        radius: Radius,
        child: Box<dyn Element>,
    ) -> Box<dyn Element> {
        Box::new(Self {
            lit: Lit::Control(StopId::of(state), keys),
            radius,
            within: false,
            child,
            size: None,
            origin: None,
        })
    }

    /// A control as wide as the page, whose ring is drawn on its own edge,
    /// rounded by `radius` as the control is.
    pub(crate) fn row(
        state: &MouseStateHandle,
        keys: Keys,
        radius: f32,
        child: Box<dyn Element>,
    ) -> Box<dyn Element> {
        Box::new(Self {
            lit: Lit::Control(StopId::of(state), keys),
            radius: Radius::Pixels(radius),
            within: true,
            child,
            size: None,
            origin: None,
        })
    }

    /// A ring on a row's own edge, rounded by `radius` as the row is, that is
    /// lit when `lit` says so.
    pub(crate) fn fixed(lit: bool, radius: f32, child: Box<dyn Element>) -> Box<dyn Element> {
        Box::new(Self {
            lit: Lit::Fixed(lit),
            radius: Radius::Pixels(radius),
            within: true,
            child,
            size: None,
            origin: None,
        })
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
        if let Lit::Control(id, keys) = self.lit
            && !keys.is_inert()
            && let Some(page) = current()
        {
            page.record(id, keys);
        }
        self.size = Some(size);
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        let size = self.size.unwrap_or_default();
        self.child.paint(origin, ctx, app);

        let lit = match self.lit {
            Lit::Fixed(lit) => lit,
            Lit::Control(id, _) => current().is_some_and(|page| {
                page.painted(id, origin.y(), size.y());
                page.is_on(id)
            }),
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
    fn a_character_off_the_box_is_typed_into_it_and_in_the_box_is_the_box_s() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            key_for(Place::Search, &stroke("a", Modifiers::default())),
            None
        );
        assert_eq!(
            key_for(Place::Control, &stroke("a", Modifiers::default())),
            Some(Key::Type('a'))
        );
        assert_eq!(
            key_for(Place::Rail, &stroke("A", shift)),
            Some(Key::Type('A'))
        );
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(key_for(Place::Control, &stroke("a", ctrl)), None, "a chord");
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
