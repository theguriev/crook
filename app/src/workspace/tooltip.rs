//! The line that says what an icon-only control does, once the pointer has
//! rested on it.
//!
//! An icon is a word somebody has to have learned, and most of Crook's small
//! controls are only an icon: the `+` under the tabs, the copy square and the
//! dots on a block, the find bar's switches and steps, the ×s that close a
//! column or a row. Each of them now names itself — the action's name, and the
//! chord that runs it when one does — in a one-line card under it.
//!
//! # When it comes up, and when it goes
//!
//! **Half a second after the pointer stops on the control,** and not before:
//! a pointer travelling across the panel to somewhere else crosses a dozen
//! controls, and a card under each of them in turn would be a flicker that
//! follows the hand. That is the delay every desktop's tooltip has, and
//! [`TIP_DELAY`] is the number most of them use.
//!
//! **At once, while one is warm.** Moving from a control whose card is up
//! straight onto its neighbour — along the find bar, across the block's two
//! squares — shows the neighbour's card on arrival, as long as the move
//! takes less than [`TIP_WARM`]. Somebody reading the controls one by one has
//! already waited once; making them wait again for each would punish exactly
//! the person the cards are for.
//!
//! **Gone on any press, scroll or key,** and kept away until the pointer
//! leaves the control and comes back. Each of those says the person knows
//! what they are doing now, and a card that stayed up over what a press just
//! changed would be describing the frame before.
//!
//! # It takes nothing
//!
//! The card is painted into a click-through overlay layer, so it covers no
//! hit and no pointer shape: a press on whatever it is drawn over reaches
//! that, as though it were not there — and puts the card away, as every
//! press does. It never takes the keyboard either; there is nothing in it to
//! focus.
//!
//! There is no fade. A card that faded in would need a frame drawn for every
//! step of the fade, and the one thing the motion setting asks — less of it —
//! is what a card that simply appears already does; so it needs no branch on
//! [`crate::motion`] to respect it.
//!
//! # Where it is
//!
//! Under the control, centred on it, four pixels clear — above instead when
//! the window has no room below — and slid sideways to stay [`WINDOW_INSET`]
//! inside the window. One line: the name in the menus' body size, then the
//! chord in the caption size and the muted colour the menus print a chord in,
//! so a card reads as a row cut out of one of them.

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crookui_core::AppContext;
use crookui_core::element::{Element, SizeConstraint};
use crookui_core::elements::{Padding, WINDOW_INSET};
use crookui_core::event::{DispatchedEvent, Event};
use crookui_core::fonts::FamilyId;
use crookui_core::geometry::{Point, RectF, Vector2F, ZIndex, vec2f};
use crookui_core::prelude::*;
use crookui_core::presenter::{LayoutContext, PaintContext};
use crookui_core::scene::ClipBounds;

use crate::theme::theme;

/// How long the pointer has to rest on a control before its card comes up.
pub(crate) const TIP_DELAY: Duration = Duration::from_millis(500);

/// How soon after one card went away the next comes up without the wait.
///
/// Long enough to cross the gap between two neighbouring controls at any
/// speed a person reads at, short enough that a pointer that left for the
/// output and came back much later waits again.
pub(crate) const TIP_WARM: Duration = Duration::from_millis(300);

/// The air between the control and its card.
///
/// The four pixels every menu keeps from what opened it.
const TIP_GAP: f32 = 4.;

/// The size the name is set in: a menu row's label.
const LABEL_SIZE: f32 = crate::style::text::BODY;

/// The size the chord is set in: a menu row's chord.
const CHORD_SIZE: f32 = crate::style::text::CAPTION;

/// The space between the name and the chord.
const CHORD_GAP: f32 = 10.;

/// The card's corner radius, which is every menu's.
const TIP_RADIUS: f32 = crate::style::radius::MEDIUM;

/// The card's padding: a menu row's five above and below, and a little less
/// than a row's inset at the sides, since a card has no column to line up with.
const TIP_PADDING: Padding = Padding {
    top: 4.,
    bottom: 5.,
    left: 8.,
    right: 8.,
};

/// Which control a card belongs to.
///
/// A name for the kind of control — `"new-tab"`, `"find-case"` — and which one
/// of them it is, for the kinds a window draws more than one of: a close
/// button on every row, a find bar on every pane. The name is also what
/// `--tooltip` asks for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct TipKey {
    control: &'static str,
    which: u64,
}

impl TipKey {
    /// The one control of this kind.
    pub(crate) const fn new(control: &'static str) -> Self {
        Self { control, which: 0 }
    }

    /// The control of this kind that belongs to `owner` — a pane, a row.
    pub(crate) fn of(self, owner: impl Hash) -> Self {
        let mut hasher = DefaultHasher::new();
        owner.hash(&mut hasher);
        Self {
            which: hasher.finish(),
            ..self
        }
    }
}

/// The control the pointer is on, as the cards see it.
#[derive(Copy, Clone, Debug)]
struct Resting {
    key: TipKey,
    /// When its card comes up, or `None` once a press, a scroll or a key has
    /// put it away for the rest of this visit.
    shows_at: Option<Instant>,
}

/// What the cards remember between frames.
#[derive(Debug, Default)]
struct State {
    resting: Option<Resting>,
    /// When the last card that was up went away because the pointer left its
    /// control: what makes the next one warm.
    cooled_at: Option<Instant>,
    /// The moment a frame has to be drawn at for a card to come up, taken
    /// once by the window after the frame that set it.
    wake: Option<Instant>,
    /// The control `--tooltip` asked for, whose card is up whatever the
    /// pointer is doing.
    pinned: Option<String>,
}

/// The window's cards: which control the pointer is resting on, and since
/// when.
///
/// One per window, shared by every [`Tooltip`] the frame builds — the elements
/// are thrown away each frame and this is what outlives them, as a
/// [`MouseStateHandle`] outlives a
/// [`Hoverable`]. Only one card is ever up, so one record is the whole of it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tips(Rc<RefCell<State>>);

impl Tips {
    /// The pointer arrived on `key`'s control at `now`.
    pub(crate) fn enter(&self, key: TipKey, now: Instant) {
        let mut state = self.0.borrow_mut();
        if state.resting.is_some_and(|resting| resting.key == key) {
            return;
        }
        // Warm if a card is up right now — the move onto this control reached
        // it before the move off the last one did — or went down a moment
        // ago.
        let up = state
            .resting
            .is_some_and(|resting| resting.shows_at.is_some_and(|at| at <= now));
        let warm = up
            || state
                .cooled_at
                .is_some_and(|at| now.saturating_duration_since(at) <= TIP_WARM);
        let shows_at = if warm { now } else { now + TIP_DELAY };
        if !warm {
            state.wake = Some(shows_at);
        }
        state.resting = Some(Resting {
            key,
            shows_at: Some(shows_at),
        });
    }

    /// The pointer left `key`'s control at `now`.
    pub(crate) fn leave(&self, key: TipKey, now: Instant) {
        let mut state = self.0.borrow_mut();
        let Some(resting) = state.resting.filter(|resting| resting.key == key) else {
            // A leave for a control the pointer has already been counted off:
            // the move that reached the next one was heard first.
            return;
        };
        if resting.shows_at.is_some_and(|at| at <= now) {
            state.cooled_at = Some(now);
        }
        state.resting = None;
    }

    /// Puts the card away for a press, a scroll or a key, until the pointer
    /// leaves the control it is on and comes back. Whether that changed what
    /// a frame would draw.
    pub(crate) fn put_away(&self) -> bool {
        let mut state = self.0.borrow_mut();
        // Not warm afterwards: the next control waits its own half second.
        state.cooled_at = None;
        match state.resting.as_mut() {
            Some(resting) if resting.shows_at.is_some() => {
                resting.shows_at = None;
                true
            }
            _ => false,
        }
    }

    /// Whether `key`'s card is up at `now`.
    pub(crate) fn shows(&self, key: TipKey, now: Instant) -> bool {
        let state = self.0.borrow();
        state.pinned.as_deref() == Some(key.control)
            || state.resting.is_some_and(|resting| {
                resting.key == key && resting.shows_at.is_some_and(|at| at <= now)
            })
    }

    /// Whether the pointer is counted as on `key`'s control.
    fn is_resting_on(&self, key: TipKey) -> bool {
        self.0
            .borrow()
            .resting
            .is_some_and(|resting| resting.key == key)
    }

    /// The moment a frame laid out since the last call needs drawing again
    /// at, once: what the one-shot wait is started for.
    pub(crate) fn take_wake(&self) -> Option<Instant> {
        self.0.borrow_mut().wake.take()
    }

    /// Keeps the card on every control called `control` up, whatever the
    /// pointer does: what `--tooltip` asks for, since no unattended run can
    /// hold a pointer still on anything.
    pub(crate) fn pin(&self, control: String) {
        self.0.borrow_mut().pinned = Some(control);
    }

    /// Whether `--tooltip` asked for the cards on controls called `control`.
    pub(crate) fn is_pinned(&self, control: &str) -> bool {
        self.0.borrow().pinned.as_deref() == Some(control)
    }

    /// Lets the card the pointer is resting on come up now, as half a second
    /// of keeping still would — which a test says this way rather than by
    /// sleeping through it.
    #[cfg(test)]
    pub(crate) fn ripen(&self) {
        if let Some(resting) = self.0.borrow_mut().resting.as_mut() {
            resting.shows_at = resting.shows_at.map(|_| Instant::now());
        }
    }
}

/// What one card says: the action's name, and the chord that runs it.
#[derive(Clone, Debug)]
pub(crate) struct Tip {
    label: String,
    chord: Option<String>,
    ui: FamilyId,
}

impl Tip {
    /// A card saying `label`, then `chord` when there is one.
    pub(crate) fn new(label: impl Into<String>, chord: Option<String>, ui: FamilyId) -> Self {
        Self {
            label: label.into(),
            chord,
            ui,
        }
    }

    /// The card itself.
    fn face(&self) -> Box<dyn Element> {
        let mut words = Flex::row()
            .with_main_axis_size(MainAxisSize::Min)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(CHORD_GAP)
            .with_child(
                Text::new(self.label.clone(), self.ui, LABEL_SIZE)
                    .with_color(theme().text_primary)
                    .finish(),
            );
        if let Some(chord) = &self.chord {
            words.add_child(
                Text::new(chord.clone(), self.ui, CHORD_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            );
        }
        Container::new(words.finish())
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_1))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(TIP_RADIUS)))
            .with_padding(TIP_PADDING)
            .finish()
    }
}

/// Where a card of `size` goes for a control at `control`, `clearance` past
/// the control's own edge, in a window of `window`.
///
/// Below and centred; above when below would run past the window's bottom
/// inset; slid sideways to stay inside the side insets, the left winning for
/// a card wider than the window.
pub(crate) fn place(control: RectF, size: Vector2F, clearance: f32, window: Vector2F) -> RectF {
    let drop = TIP_GAP + clearance;
    let below = control.max_y() + drop;
    let top = if below + size.y() > window.y() - WINDOW_INSET {
        (control.min_y() - drop - size.y()).max(WINDOW_INSET)
    } else {
        below
    };
    let centred = control.min_x() + (control.width() - size.x()) / 2.;
    let left = centred
        .min(window.x() - WINDOW_INSET - size.x())
        .max(WINDOW_INSET);
    RectF::new(vec2f(left, top), size)
}

/// A card for one control, before there is a control to wrap: what a
/// renderer that has no [`Workspace`](super::view::Workspace) to hand — a row's
/// close button, built inside its row's closure — is given instead.
#[derive(Clone, Debug)]
pub(crate) struct Named {
    pub(crate) tips: Tips,
    pub(crate) key: TipKey,
    pub(crate) tip: Tip,
}

impl Named {
    /// `child`, naming itself.
    pub(crate) fn wrap(self, child: Box<dyn Element>) -> Tooltip {
        Tooltip::new(child, self.tips, self.key, self.tip)
    }
}

/// A card laid out for one frame, ready to be painted beside its control.
///
/// For a control that is painted rather than built — a block's squares are
/// drawn by the list itself — which lays this out in its own `layout` and
/// paints it in its own `paint`, where it knows where the control is.
pub(crate) struct LaidTip {
    face: Box<dyn Element>,
    size: Vector2F,
}

impl LaidTip {
    /// `tip`'s card, measured against the window.
    pub(crate) fn lay_out(tip: &Tip, ctx: &mut LayoutContext, app: &AppContext) -> Self {
        let mut face = tip.face();
        let size = face.layout(
            SizeConstraint::new(Vector2F::zero(), ctx.window_size),
            ctx,
            app,
        );
        Self { face, size }
    }

    /// Paints the card beside `control`, in a click-through layer over
    /// everything painted so far.
    pub(crate) fn paint_beside(
        &mut self,
        control: RectF,
        clearance: f32,
        ctx: &mut PaintContext,
        app: &AppContext,
    ) {
        let at = place(control, self.size, clearance, ctx.window_size);
        ctx.scene.start_overlay_layer(ClipBounds::None);
        ctx.scene.set_active_layer_click_through();
        self.face.paint(at.origin(), ctx, app);
        ctx.scene.stop_layer();
    }
}

/// Whether an event is one that puts a card away: a press, a scroll, a key.
pub(crate) fn puts_tips_away(event: &Event) -> bool {
    matches!(
        event,
        Event::MouseDown { .. } | Event::ScrollWheel { .. } | Event::KeyDown { .. }
    )
}

/// A control that names itself after the pointer has rested on it.
///
/// Wraps the control, asks nothing of its layout and hands it every event
/// first, so the control behaves exactly as it did; what this adds is the
/// hover it watches for itself — the same test a [`Hoverable`] makes, so the
/// two agree on when the pointer is on it — and the card it paints beside the
/// control when [`Tips`] says the card is up.
pub(crate) struct Tooltip {
    child: Box<dyn Element>,
    tips: Tips,
    key: TipKey,
    tip: Tip,
    /// How far past the control's own edge the card hangs, beyond the usual
    /// gap: the find bar's controls sit inside a bar, and a card over the
    /// bar's own edge would cover the field it is about.
    clearance: f32,
    /// Whether the keyboard is on the control, which shows its card at once.
    focused: bool,
    laid: Option<LaidTip>,
    origin: Option<Point>,
    child_max_z_index: Option<ZIndex>,
}

impl Tooltip {
    /// `child`, naming itself with `tip`.
    pub(crate) fn new(child: Box<dyn Element>, tips: Tips, key: TipKey, tip: Tip) -> Self {
        Self {
            child,
            tips,
            key,
            tip,
            clearance: 0.,
            focused: false,
            laid: None,
            origin: None,
            child_max_z_index: None,
        }
    }

    /// Hangs the card `clearance` further from the control.
    pub(crate) fn with_clearance(mut self, clearance: f32) -> Self {
        self.clearance = clearance;
        self
    }

    /// Shows the card while the keyboard's ring is on the control, as well as
    /// under a pointer that has rested there.
    pub(crate) fn with_focus(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether `position` is over the control and not covered by anything.
    fn is_over(&self, position: Vector2F, ctx: &EventContext) -> bool {
        let (Some(origin), Some(size), Some(z_index)) =
            (self.origin, self.child.size(), self.child_max_z_index)
        else {
            return false;
        };
        ctx.visible_rect(origin, size)
            .is_some_and(|visible| visible.contains_point(position))
            && !ctx.is_covered(Point::from_vec2f(position, z_index))
    }
}

impl Element for Tooltip {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let size = self.child.layout(constraint, ctx, app);
        self.laid = (self.focused || self.tips.shows(self.key, Instant::now()))
            .then(|| LaidTip::lay_out(&self.tip, ctx, app));
        size
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        self.child.paint(origin, ctx, app);
        self.child_max_z_index = Some(ctx.scene.max_active_z_index());
        if let (Some(laid), Some(size)) = (self.laid.as_mut(), self.child.size()) {
            laid.paint_beside(RectF::new(origin, size), self.clearance, ctx, app);
        }
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let handled = self.child.dispatch_event(event, ctx, app);
        let raw = event.raw_event();
        if let Event::MouseMoved { position, .. } = raw {
            let over = self.is_over(*position, ctx);
            let resting = self.tips.is_resting_on(self.key);
            if over && !resting {
                self.tips.enter(self.key, Instant::now());
                ctx.notify();
            } else if !over && resting {
                self.tips.leave(self.key, Instant::now());
                ctx.notify();
            }
        } else if puts_tips_away(raw) && self.tips.is_resting_on(self.key) && self.tips.put_away() {
            ctx.notify();
        }
        handled
    }
}
