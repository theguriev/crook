//! The two shapes a sandboxed plugin describes and the host drives.
//!
//! Everything else in [`render`](super::render) is a translation: a node comes
//! in, an element goes out, and nothing is remembered between one frame and
//! the next. These two cannot be that. A picker has a field somebody is typing
//! into, a row the keyboard is on and a list scrolled somewhere; a menu is up
//! or it is not. That state has to live somewhere, and it cannot live in the
//! guest — a plugin that held it would need the keyboard, and the whole of
//! what this tier is worth rests on a plugin not having one.
//!
//! So the host holds it, in a [`Held`] per plugin. The plugin says what can
//! be chosen; the host says what was chosen, in the argument of the action the
//! plugin named. That is the same bargain as
//! [`Meter`](crook_plugin_api::Node::Meter) — describe the reading, not the
//! pixels — carried as far as it goes.
//!
//! # One of each, per plugin
//!
//! A plugin has one picker and one menu up at a time, and that is not a
//! limitation anybody will meet: a panel is modal, so opening a second one
//! dismisses the first, and a plugin with two chips has two panels of which a
//! person can see one. What it buys is that the four keys have one thing to
//! act on and no way of naming which — see [`Held::claims`].
//!
//! # The keys are actions, like everything else
//!
//! Escape, Enter and the arrows are claimed through
//! [`Host::claim_panel`](crate::plugin::Host::claim_panel), which names an
//! action rather than doing anything. So they end at `WorkspaceAction::Run`
//! exactly as a chord out of somebody's `keybindings.json` does, and a person
//! who wants ctrl-n to move the selection can bind it. The palette's
//! arrangement, and the reason it is worth copying twice.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use std::rc::Rc;

use crookui_core::elements::{MouseStateHandle, Padding, ScrollStateHandle};
use crookui_core::event::Keystroke;
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crook_plugin::ActionName;
use crook_plugin_api::{MenuItem, Row};

use crate::clipboard::Clipboard;
use crate::plugin::{ActionId, Showing, Voice};
use crate::text_input::TextInput;
use crate::theme::theme;
use crate::workspace::{Fonts, TextField, WorkspaceAction};

/// One row's height, which is what makes scrolling to a row arithmetic.
///
/// The palette's, because it is the same row: a label a person is picking out
/// of a list, at the interface's own size.
const ROW_HEIGHT: f32 = 30.;

/// How many rows are shown before the list scrolls.
const VISIBLE_ROWS: f32 = 8.;

/// The gap between the field and the list under it.
const GAP: f32 = 8.;

/// The type size in a row, and in a menu entry.
const TEXT_SIZE: f32 = 12.;

/// The icon in front of a row's label, and the gap after it.
const ICON_SIZE: f32 = 13.;
const ICON_GAP: f32 = 8.;

/// A row's own inset, and its corner.
const ROW_PADDING: f32 = 7.;
const ROW_RADIUS: f32 = 5.;

/// What a menu is drawn as: its width, its inset and its corner.
const MENU_WIDTH: f32 = 200.;
const MENU_PADDING: f32 = 4.;
const MENU_RADIUS: f32 = 8.;

/// The gap between the thing pressed and the menu that opened.
const MENU_OFFSET: f32 = 4.;

/// One thing in a plugin's chrome that the pointer can be over.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum Control {
    /// The field at the top of a picker.
    Query,
    /// One row of the list, by its place in what is being shown.
    ///
    /// By position rather than by key, for the palette's reason: the list is
    /// re-filtered on every keystroke, and a handle kept per key would be a
    /// map that only grows.
    Row(usize),
    /// One entry of a menu.
    Entry(usize),
    /// What a menu is opened from.
    Opener,
}

/// The names the four keys resolve to.
///
/// Held rather than built, because a claim runs on every keystroke the window
/// sees and formatting four strings to answer "no" is a cost with nothing on
/// the other side of it.
pub(crate) const NEXT: &str = "crook/plugins/picker-next";
/// See [`NEXT`].
pub(crate) const PREVIOUS: &str = "crook/plugins/picker-previous";
/// See [`NEXT`].
pub(crate) const CHOOSE: &str = "crook/plugins/picker-choose";
/// See [`NEXT`].
pub(crate) const CLOSE: &str = "crook/plugins/picker-close";

thread_local! {
    /// Whatever plugin has something up, and the flag the workspace reads.
    ///
    /// One of each, because one is all there can be: a panel is modal, so
    /// opening a second dismisses the first, and the keys therefore have
    /// exactly one thing to act on and no way of naming which. A thread local
    /// rather than something the host holds because this *is* per thread —
    /// there is one foreground and everything here runs on it — and because
    /// the alternative is four actions and a key claim per installed plugin,
    /// on every plugin's card, whether or not it has ever drawn a picker.
    static OPEN: RefCell<Option<Rc<Held>>> = const { RefCell::new(None) };
    static SHOWING: RefCell<Option<Showing>> = const { RefCell::new(None) };
}

/// Hands the tier the flag the workspace reads. Called once, by the plugin
/// that registers the four keys.
pub(crate) fn armed_by(showing: Showing) {
    SHOWING.with(|held| *held.borrow_mut() = Some(showing));
}

/// Says that this is the one with something up.
///
/// The one before it is *not* told: a panel is modal, so the press that opened
/// this one has already dismissed that one through its own underlay, and a
/// second telling would be the host shutting a panel nobody touched.
fn mark_open(held: &Rc<Held>) {
    OPEN.with(|open| *open.borrow_mut() = Some(Rc::clone(held)));
}

/// Whatever has something up, for the four keys to act on.
pub(crate) fn open() -> Option<Rc<Held>> {
    OPEN.with(|open| open.borrow().clone())
}

/// Takes down whatever is up, without telling the plugin.
///
/// What [`Host::take_panels_down`](crate::plugin::Host::take_panels_down)
/// calls: the host letting go of the keyboard, not the plugin being told its
/// panel is gone. A panel still open in a plugin comes back with the pane that
/// drew it, which is what a person who switched tabs and switched back means.
pub(crate) fn shut_open() {
    if let Some(held) = open() {
        held.shut();
    }
}

/// What one of these keys does while something is up.
///
/// Escape takes down whichever of the two is up — a menu first, since it is
/// the one drawn over the other — and the rest belong to a picker and are left
/// alone while there is none. A keystroke this returns `None` for falls
/// through to the bindings and then to the pane, which is what keeps `cmd+t`
/// working over an open panel.
pub(crate) fn claims(keystroke: &Keystroke) -> Option<ActionName> {
    if !keystroke.modifiers.is_empty() {
        return None;
    }
    let held = open()?;
    let picking = held.showing_a_picker();
    let name = match keystroke.key.as_str() {
        "escape" => CLOSE,
        "up" if picking => PREVIOUS,
        "down" if picking => NEXT,
        "enter" if picking => CHOOSE,
        _ => return None,
    };
    ActionName::parse(name).ok()
}

/// What the last frame drew, so that a key pressed on the next one knows what
/// it is acting on.
#[derive(Default)]
struct Shown {
    /// The key of every row being offered, in the order it is offered.
    keys: Vec<String>,
    /// The action a chosen row runs.
    choose: Option<ActionId>,
    /// The action that takes the panel holding it down — the `dismiss` of the
    /// [`Anchored`](crook_plugin_api::Node::Anchored) this picker is inside.
    dismiss: Option<ActionId>,
}

/// Everything the host is holding on one plugin's behalf.
///
/// Named for what it is rather than for what is in it: the picker's field and
/// selection, the menu that is up, the flag that says the keyboard is here.
/// `render::Chrome` is the other half of the same idea and is deliberately not
/// this — that one is what a *node* is drawn with and travels down the tree by
/// value; this is what a *plugin* is holding and outlives every frame.
pub(crate) struct Held {
    /// Where what the thing that was pressed had to say is left.
    ///
    /// The host's own, shared with every other control that stands for a
    /// *thing* — the Keyboard Shortcuts page's Change buttons say into the
    /// same one — so a plugin's action and one of Crook's own are told what
    /// they are about in exactly one way. Said by whatever was clicked and
    /// taken by the action a moment later, which works because a dispatched
    /// action is applied after the whole tree has seen the event: one press,
    /// one thing said. See [`Host::voice`](crate::plugin::Host::voice).
    voice: Voice,
    /// Whether a picker or a menu of this plugin's is up.
    open: Cell<bool>,
    /// Whether that has changed since anybody last asked.
    ///
    /// What tells the contribution that drew it to re-sync the keyboard: the
    /// fact is discovered while building a tree, and the thing that has to act
    /// on it holds the workspace.
    moved: Cell<bool>,
    /// What has been typed into the picker's field.
    query: TextInput,
    /// Which row the keyboard is on.
    selected: Cell<usize>,
    scroll: ScrollStateHandle,
    controls: RefCell<HashMap<Control, MouseStateHandle>>,
    /// What the query was when the list was last worked out; see
    /// [`Held::settle`].
    last_query: RefCell<String>,
    shown: RefCell<Shown>,
    /// The menu that is up, by the entries it holds.
    ///
    /// The entries themselves are the identity: a plugin draws its menus from
    /// what it was told, so two of them are the same menu exactly when they
    /// offer the same things — and a node has no other name to be known by.
    menu: RefCell<Option<Vec<MenuItem>>>,
}

impl Held {
    /// A plugin's chrome, with nothing up.
    pub(crate) fn new(voice: Voice) -> Self {
        Self {
            voice,
            open: Cell::new(false),
            moved: Cell::new(false),
            query: TextInput::new(),
            selected: Cell::new(0),
            scroll: ScrollStateHandle::default(),
            controls: RefCell::new(HashMap::new()),
            last_query: RefCell::new(String::new()),
            shown: RefCell::new(Shown::default()),
            menu: RefCell::new(None),
        }
    }

    /// Whether the last frame drew a picker rather than only a menu.
    fn showing_a_picker(&self) -> bool {
        self.shown.borrow().choose.is_some()
    }

    /// Says what the thing that was pressed had to say.
    pub(crate) fn say(&self, what: impl Into<String>) {
        self.voice.say(what);
    }

    /// Marks something of this plugin's as up, or not, and says whether that
    /// is a change.
    ///
    /// The answer is what tells the caller the keyboard has moved and a
    /// re-sync is owed. Setting it is the whole of how a panel drawn by a
    /// plugin takes the keys away from the pane it is drawn in.
    fn set_open(&self, open: bool) -> bool {
        if self.open.get() == open {
            return false;
        }
        self.open.set(open);
        self.moved.set(true);
        self.query.set_has_keys(open && self.showing_a_picker());
        SHOWING.with(|showing| {
            if let Some(showing) = showing.borrow().as_ref() {
                showing.set(open);
            }
        });
        // Nothing is up any more, and the keys have to stop acting on this.
        // Cleared by identity rather than unconditionally: a panel that was
        // dismissed *because* another opened must not take the new one down.
        if !open {
            OPEN.with(|held| {
                let mine = held
                    .borrow()
                    .as_ref()
                    .is_some_and(|open| std::ptr::eq(Rc::as_ptr(open), self));
                if mine {
                    held.borrow_mut().take();
                }
            });
        }
        true
    }

    /// Whether the keyboard has moved since this was last asked.
    pub(super) fn keyboard_moved(&self) -> bool {
        self.moved.replace(false)
    }

    /// Lets go of the keyboard after one of the plugin's own actions has run.
    ///
    /// What a plugin's action did to its own panel is the plugin's business
    /// and the host cannot see it — a picker that chose a row may have shut
    /// itself, walked into a directory, or done nothing at all. So the host
    /// lets go and finds out by drawing: the next frame puts a picker back on
    /// screen and takes the keys again, and one that does not leaves them with
    /// the pane, which is where they belong.
    pub(super) fn released(&self) {
        if self.menu.borrow().is_none() {
            self.set_open(false);
        }
    }

    /// Takes everything of this plugin's down.
    ///
    /// What [`Host::take_panels_down`](crate::plugin::Host::take_panels_down)
    /// calls, and what the plugin's own close action calls. It does not tell
    /// the guest: the panel is the guest's state and this is the host letting
    /// go of the keyboard, so a panel that is still open in the plugin comes
    /// back with the pane that drew it, which is what a person who switched
    /// tabs and switched back means.
    pub(crate) fn shut(&self) {
        self.menu.replace(None);
        self.shown.replace(Shown::default());
        self.set_open(false);
    }

    /// Moves the selection, wrapping at both ends.
    pub(crate) fn move_selection(&self, by: isize) {
        let len = self.shown.borrow().keys.len();
        if len == 0 {
            self.selected.set(0);
            return;
        }
        let index = self.selected.get() as isize;
        self.selected
            .set((index + by).rem_euclid(len as isize) as usize);
        self.scroll_selection_into_view();
    }

    /// Keeps the row the keyboard is on inside the list.
    ///
    /// Arithmetic, because rows are a fixed height — the trade the palette,
    /// the Themes panel and the tabs panel all name.
    fn scroll_selection_into_view(&self) {
        let top = self.selected.get() as f32 * ROW_HEIGHT;
        let scroll = self.scroll.clone();
        let mut scroll = scroll.lock();

        let offset = scroll.offset();
        let viewport = scroll.viewport();
        if top < offset {
            scroll.scroll_to(top);
        } else if top + ROW_HEIGHT > offset + viewport {
            scroll.scroll_to(top + ROW_HEIGHT - viewport);
        }
    }

    /// The action a chosen row runs, and what to hand it.
    pub(crate) fn chosen(&self) -> Option<(ActionId, String)> {
        let shown = self.shown.borrow();
        let key = shown.keys.get(self.selected.get())?;
        Some((shown.choose?, key.clone()))
    }

    /// The action that takes down whatever this plugin has up.
    pub(crate) fn dismissal(&self) -> Option<ActionId> {
        self.shown.borrow().dismiss
    }

    /// Records the action a panel is dismissed by, while it is being built.
    pub(super) fn hangs_off(&self, dismiss: Option<ActionId>) {
        self.shown.borrow_mut().dismiss = dismiss;
    }

    /// Opens the menu holding `items`, and says whether anything changed.
    pub(super) fn open_menu(self: &Rc<Self>, items: &[MenuItem]) -> bool {
        mark_open(self);
        self.menu.replace(Some(items.to_vec()));
        self.set_open(true);
        true
    }

    /// Whether the menu that is up is this one.
    fn menu_is(&self, items: &[MenuItem]) -> bool {
        self.menu.borrow().as_deref() == Some(items)
    }

    /// Takes the menu down, leaving anything else alone.
    pub(super) fn close_menu(&self) {
        self.menu.replace(None);
        if !self.showing_a_picker() {
            self.set_open(false);
        }
    }

    /// The mouse state for one control, made the first time it is drawn.
    fn control(&self, control: Control) -> MouseStateHandle {
        self.controls
            .borrow_mut()
            .entry(control)
            .or_default()
            .clone()
    }

    /// Puts the selection somewhere that exists, given what is being shown.
    ///
    /// The palette's rule, and its reasoning: a query that *changed* puts the
    /// keyboard back on the first row, because the first row is the best match
    /// for what was just typed; a list that changed for any other reason only
    /// clamps, so the selection stays where the person put it.
    fn settle(&self, query: &str, len: usize) {
        let changed = {
            let mut last = self.last_query.borrow_mut();
            let changed = *last != query;
            if changed {
                *last = query.to_owned();
            }
            changed
        };

        if changed {
            self.selected.set(0);
            self.scroll.lock().scroll_to_top();
            return;
        }
        if len == 0 {
            self.selected.set(0);
        } else if self.selected.get() >= len {
            self.selected.set(len - 1);
        }
    }
}

/// The picker: a field, and the rows that match what is in it.
///
/// The filtering is the host's, and it is substring rather than fuzzy for the
/// reason the palette's is: a fuzzy match over a short list answers every
/// query with a row, and the answer stops meaning anything. Every term has to
/// match, and they need not match the same part — this is a person narrowing a
/// list, not writing a pattern.
pub(super) fn picker(
    chrome: &Rc<Held>,
    placeholder: &str,
    rows: &[Row],
    choose: Option<ActionId>,
    fonts: Fonts,
    clipboard: &Clipboard,
) -> Box<dyn Element> {
    let typed = chrome.query.editor().text().to_lowercase();
    let terms: Vec<&str> = typed.split_whitespace().collect();
    let matching: Vec<&Row> = rows
        .iter()
        .filter(|row| {
            let label = row.label.to_lowercase();
            terms.iter().all(|term| label.contains(term))
        })
        .collect();

    chrome.settle(&typed, matching.len());
    {
        let mut shown = chrome.shown.borrow_mut();
        shown.keys = matching.iter().map(|row| row.key.clone()).collect();
        shown.choose = choose;
    }
    // A picker that is on screen is a picker with the keyboard. Said here
    // rather than by whatever put the panel up, because the panel is the
    // guest's own state and this is the only place the host finds out what is
    // in it. The caller re-syncs when this answers `true`.
    mark_open(chrome);
    chrome.set_open(true);
    chrome.query.set_has_keys(true);

    let field = TextField::new(
        chrome.query.clone(),
        clipboard.clone(),
        fonts,
        chrome.control(Control::Query),
        placeholder.to_owned(),
    )
    .with_icon(Lucide::Search)
    .finish();

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(Container::new(field).with_margin_bottom(GAP).finish());

    if matching.is_empty() {
        column.add_child(
            Container::new(
                Text::new("Nothing matches that.", fonts.ui, TEXT_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            )
            .with_uniform_padding(ROW_PADDING)
            .finish(),
        );
        return column.finish();
    }

    let mut list = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for (index, row) in matching.iter().enumerate() {
        list.add_child(offered(
            row,
            index == chrome.selected.get(),
            choose,
            chrome,
            chrome.control(Control::Row(index)),
            fonts.ui,
        ));
    }

    column.add_child(
        ConstrainedBox::new(
            Scrollable::new(chrome.scroll.clone(), list.finish())
                .with_scrollbar(theme().overlay_3)
                .finish(),
        )
        // As tall as it needs to be, up to eight rows: a picker with three
        // things in it is a small panel rather than a tall one with a hole in
        // the bottom.
        .with_max_height(ROW_HEIGHT * VISIBLE_ROWS)
        .finish(),
    );
    column.finish()
}

/// One row of a picker.
fn offered(
    row: &Row,
    selected: bool,
    choose: Option<ActionId>,
    chrome: &Rc<Held>,
    state: MouseStateHandle,
    ui: FamilyId,
) -> Box<dyn Element> {
    let mut line = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center);
    if let Some(icon) = super::render::mark(&row.icon, row.tone, ICON_SIZE) {
        line.add_child(Container::new(icon).with_margin_right(ICON_GAP).finish());
    }
    line.add_child(
        Text::new(row.label.clone(), ui, TEXT_SIZE)
            .with_color(super::render::colour(row.tone))
            .finish(),
    );

    let key = row.key.clone();
    let chrome = chrome.clone();
    ConstrainedBox::new(
        Hoverable::new(state, move |mouse| {
            // The palette's three states, and its colours: a selected row is
            // a raised ground rather than a filled accent one, because the
            // text on it is the plugin's own tone and a tone chosen to read on
            // the panel does not read on an accent.
            let background = match (selected, mouse.is_hovered()) {
                (true, _) => theme().overlay_2,
                (false, true) => theme().overlay_1,
                (false, false) => Color::TRANSPARENT,
            };
            Container::new(line.finish())
                .with_background_color(background)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(ROW_RADIUS)))
                .with_padding(Padding {
                    top: ROW_PADDING,
                    bottom: ROW_PADDING,
                    left: ROW_PADDING,
                    right: ROW_PADDING,
                })
                .finish()
        })
        .on_click(move |_, ctx, _| {
            // Said before the action is dispatched and taken when it runs,
            // which is safe for the reason the field it is kept in says: an
            // action is applied after the whole tree has seen the event, and
            // one press presses one row.
            chrome.say(key.clone());
            if let Some(action) = choose {
                ctx.dispatch_typed_action(WorkspaceAction::Run(action));
            }
        })
        .finish(),
    )
    .with_height(ROW_HEIGHT)
    .finish()
}

/// Something with a menu on its secondary click.
///
/// The menu is the host's chrome around the plugin's words, for the reason a
/// panel is: a plugin that had to draw one would be a plugin whose menu is the
/// wrong shape beside the one a tab opens.
pub(super) fn menued(
    content: Box<dyn Element>,
    items: &[MenuItem],
    chrome: &Rc<Held>,
    action: &dyn Fn(&str) -> Option<ActionId>,
    ui: FamilyId,
) -> Box<dyn Element> {
    // Wrapped rather than laid over: a `Hoverable` builds its own child and
    // records the hit rect around it, so what the secondary click lands on is
    // exactly what the plugin drew, whatever shape that turned out to be.
    // Nothing about the content changes under the pointer — how a chip looks
    // is the plugin's business and this is here for the press alone.
    let opener = {
        let chrome = chrome.clone();
        let items = items.to_vec();
        Hoverable::new(chrome.control(Control::Opener), move |_| content).on_right_click(
            move |_, ctx, _| {
                chrome.open_menu(&items);
                ctx.notify();
            },
        )
    };
    let mut stack = Stack::new().with_child(opener.finish());

    if !chrome.menu_is(items) {
        return stack.finish();
    }

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
    for (index, item) in items.iter().enumerate() {
        column.add_child(entry(
            item,
            action(&item.action),
            chrome,
            chrome.control(Control::Entry(index)),
            ui,
        ));
    }

    let menu = ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_1))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(MENU_RADIUS)))
            .with_uniform_padding(MENU_PADDING)
            .finish(),
    )
    .with_width(MENU_WIDTH)
    .finish();

    let closing = chrome.clone();
    stack.add_anchored_overlay_child(
        Dismiss::new(menu)
            .modal()
            .on_dismiss(move |ctx, _| {
                closing.close_menu();
                ctx.notify();
            })
            .finish(),
        AnchorTo {
            parent: Corner::BottomLeft,
            child: Corner::TopLeft,
            offset: vec2f(0., MENU_OFFSET),
            keep_on_screen: true,
            keep_clear_of_parent: false,
        },
    );
    stack.finish()
}

/// One entry of a menu.
fn entry(
    item: &MenuItem,
    action: Option<ActionId>,
    chrome: &Rc<Held>,
    state: MouseStateHandle,
    ui: FamilyId,
) -> Box<dyn Element> {
    let label = item.label.clone();
    let argument = item.argument.clone();
    let chrome = chrome.clone();

    Hoverable::new(state, move |mouse| {
        Container::new(
            Text::new(label.clone(), ui, TEXT_SIZE)
                .with_color(theme().text_primary)
                .finish(),
        )
        .with_background_color(if mouse.is_hovered() {
            theme().overlay_1
        } else {
            Color::TRANSPARENT
        })
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(ROW_RADIUS)))
        .with_padding(Padding {
            top: ROW_PADDING,
            bottom: ROW_PADDING,
            left: ROW_PADDING,
            right: ROW_PADDING,
        })
        .finish()
    })
    .on_click(move |_, ctx, _| {
        chrome.say(argument.clone());
        chrome.close_menu();
        if let Some(action) = action {
            ctx.dispatch_typed_action(WorkspaceAction::Run(action));
        }
        ctx.notify();
    })
    .finish()
}
