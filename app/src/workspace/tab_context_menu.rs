//! The menu a tab's secondary press opens, and the slot its entries come from.
//!
//! # What this file is, and what it deliberately is not
//!
//! It is the *shell*: a popup, the rows a slot puts in it, the hairlines
//! between the groups those rows fall into, and the one place a submenu may
//! hang. It knows no entry by name. "Close tab" is not written here, and
//! neither is "Worktrees…" — they are contributions, from
//! [`crook/tabs`](crate::plugins::tabs) and
//! [`crook/worktrees`](crate::plugins::worktrees), and the shell could not
//! name one of them if it wanted to.
//!
//! That split is the whole point of the file. The gesture — secondary button,
//! on a row, over a tab — is the most valuable unclaimed surface in the
//! panel, and there is exactly one of it. Before this, the worktree menu had
//! it outright: a menu about git, opened by a gesture that reads as "about
//! this tab", which is why a person looking for "close tab" found a list of
//! checkouts. The answer is not to bolt Warp's nine entries onto that menu's
//! column. It is to make the gesture open a *place*, and to let the worktree
//! menu be one of the things in it — the first of them, and the one that
//! proved the place was general, because it was written before the slot
//! existed and had to be fitted into it afterwards.
//!
//! # Where the entries come from
//!
//! [`TAB_MENU_ENTRIES`](crate::plugins::tabs::TAB_MENU_ENTRIES) is a
//! [`Cardinality::List`](crook_plugin::Cardinality::List) slot, declared by the
//! plugin that owns the tabs. A contribution is what every UI contribution in
//! this application is — a function from `&Workspace` to an element — so an
//! entry decides for itself whether it is drawn at all, whether it is inert,
//! and what it looks like when it is. The worktree entry uses all three: it is
//! absent outside a git repository, rather than present and dead.
//!
//! Nothing is passed to a contribution saying which tab the menu is on,
//! because nothing needs to be: the menu is only ever open on one tab and
//! [`Workspace::tab_menu_target`] is that tab. A slot whose contributions took
//! an argument would be a second kind of slot, and one every other surface in
//! the application would then have to explain not having.
//!
//! # Groups, without a second vocabulary for them
//!
//! Warp's menu is nine entries and five groups, and the rules between them
//! carry real meaning: they are what says that renaming a tab and closing it
//! are not the same kind of act. A slot hands its renderer a flat, ordered
//! list, so the seams have to be inferred from something — and the something
//! is already there. An entry's `order` is divided by [`BAND`]: neighbours
//! that land in different bands get a hairline between them.
//!
//! One number does both jobs, which is what makes this worth doing rather than
//! adding a `separator: bool` to every contribution: a plugin says *where* its
//! entry goes and *what it belongs with* in the same breath, and — because the
//! rule is about neighbours rather than about entries — no plugin can draw a
//! line across somebody else's group. A stranger's plugin that asks for band 3
//! lands among the copy entries whether or not it has read this paragraph.
//!
//! # One underlay, and therefore one submenu that is not a popup
//!
//! [`Workspace::a_popup_is_open`](super::view::Workspace::a_popup_is_open)
//! explains why two popups are never up at once: a modal underlay covers only
//! the layers painted before it, so a second popup floats above the first
//! one's underlay while that underlay eats the press meant to dismiss it. A
//! submenu is exactly the shape that argument forbids — so it is not a second
//! popup. It is an anchored child of *this* popup's own stack, painted above
//! it and inside its hit rect, which is why a press on the worktree list does
//! not dismiss the menu that opened it and a press outside both takes the
//! whole thing down in one gesture.

use std::cell::RefCell;
use std::collections::HashMap;

use crookui_core::element::SizeConstraint;
use crookui_core::elements::MouseStateHandle;
use crookui_core::event::DispatchedEvent;
use crookui_core::geometry::Point;
use crookui_core::icons::Lucide;
use crookui_core::prelude::*;
use crookui_core::presenter::{EventContext, LayoutContext, PaintContext};

use crate::tab::{PaneId, TabId};
use crate::theme::theme;

use super::action::{TabMenuAction, WorkspaceAction};
use super::view::Workspace;

/// How wide the popup is.
///
/// Warp's own tab menu, and wide enough for the longest entry Crook ships —
/// "Copy working directory" — with room for a plugin's sentence beside it.
pub(super) const MENU_WIDTH: f32 = 232.;

/// The inset around every row. The worktree menu's, because the two columns
/// are read as one and a submenu that indented differently would look loose.
const ROW_INSET: f32 = 12.;

/// The popup's corner radius, which is every other menu's in this application.
const MENU_RADIUS: f32 = 6.;

/// The size of an entry's label.
const LABEL_SIZE: f32 = 12.;

/// The size of the chevron on the one row that has one.
const CHEVRON_SIZE: f32 = 12.;

/// How many `order`s make one group.
///
/// A hundred, so that "the fourth group, third entry" is `403` and reads as
/// what it is. See this file's own doc for why the band is a division of the
/// order rather than a field of its own.
pub(crate) const BAND: i32 = 100;

/// Whether a menu is up and on what.
///
/// The tab *and* the pane, which are two facts here and one everywhere else in
/// the panel. A row under `Panes` granularity stands for a pane and its menu
/// must be able to copy that pane's title; the tab it belongs to is what
/// closing and grouping act on. Carrying only the tab would make "Rename pane"
/// mean the focused pane rather than the one that was clicked.
#[derive(Default)]
pub(crate) struct TabContextMenuState {
    /// The tab whose menu is up. `None` is closed — "which tab" *is* the open
    /// flag, for the reason [`TabMenuState`](super::tab_menu::TabMenuState)
    /// says: a bool beside it is a second fact that can disagree with this one.
    pub(super) tab: Option<TabId>,
    /// The pane whose row was pressed.
    pub(super) pane: Option<PaneId>,
    /// One mouse state per entry, made on that entry's first frame.
    ///
    /// Keyed by a string rather than by an enum of the entries, because the
    /// entries are however many plugins contributed and cannot be written down
    /// here. The Themes panel's controls are keyed the same way and for the
    /// same reason.
    controls: RefCell<HashMap<String, MouseStateHandle>>,
}

impl TabContextMenuState {
    /// Whether the menu is up.
    pub(crate) fn is_open(&self) -> bool {
        self.tab.is_some()
    }

    /// The mouse state for one entry.
    pub(crate) fn control(&self, key: &str) -> MouseStateHandle {
        self.controls
            .borrow_mut()
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    /// Forgets every hover and press the menu was holding.
    ///
    /// Called when it closes: every row is about to stop existing without
    /// seeing a hover-out, and the next opening would come back with an entry
    /// lit under a pointer that is somewhere else.
    pub(crate) fn forget_hover_state(&self) {
        for state in self.controls.borrow().values() {
            state.lock().reset_interaction_state();
        }
    }
}

/// What a contribution returns when it has nothing to put in the menu.
///
/// Not [`Empty`], and the difference is the hairlines. The shell puts a rule
/// between two neighbours in different bands, so a contribution that answered
/// "nothing" with an element that merely laid out to nothing would still be
/// counted as its band's row — and a worktree entry that was absent because
/// there is no repository would leave a rule at the foot of the menu with
/// nothing under it.
///
/// So the answer says what it is, through the channel
/// [`Element::parent_data`] exists for: an untyped word from a child to the
/// particular parent that knows how to read it, which is how an anchored child
/// tells a [`Stack`] it is anchored. The menu is that parent here, and this is
/// the one word it understands.
pub(crate) fn nothing() -> Box<dyn Element> {
    Box::new(NotARow {
        child: Empty::new().finish(),
    })
}

/// The carrier [`nothing`] hands back: an empty child, and a `parent_data`
/// that says so.
struct NotARow {
    child: Box<dyn Element>,
}

/// The word itself. Its type is the whole message, so it has no fields.
struct Absent;

impl Element for NotARow {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.child.layout(constraint, ctx, app)
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app);
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
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn parent_data(&self) -> Option<&dyn std::any::Any> {
        Some(&Absent)
    }
}

/// Whether a contribution said it had nothing to draw.
fn is_absent(row: &dyn Element) -> bool {
    row.parent_data()
        .is_some_and(|data| data.downcast_ref::<Absent>().is_some())
}

/// One entry, as every contributor draws one.
///
/// The shell offers this rather than leaving each plugin to build a row,
/// because a menu whose rows were each somebody's own idea of a menu row is a
/// menu that looks like a list of plugins. What a contribution chooses is the
/// label and what pressing it does; the inset, the type size, the hover colour
/// and the height are the menu's.
///
/// **No icons**, and that is Warp's menu rather than an omission: nine text
/// rows in a narrow column, where an icon per row would be nine glyphs a
/// person has to learn to skip. It is also the honest answer to what Crook has
/// — thirteen icons, none of which means "rename" or "group", and a fourteenth
/// costs path data in `crookui_core::icons::data` rather than a name.
///
/// `key` is the entry's mouse state, so it must be unique within the menu:
/// `owner/entry` is what every contribution already has to hand.
pub(crate) fn entry(
    workspace: &Workspace,
    key: &str,
    label: impl Into<String>,
    action: WorkspaceAction,
) -> Box<dyn Element> {
    build_row(
        workspace,
        key,
        label.into(),
        Chevron::None,
        true,
        Some(action),
    )
}

/// An entry that opens a submenu: the same row with a chevron after it.
///
/// The chevron is the one icon in the menu, and it earns its place by being
/// the promise that pressing this does not *do* anything — the single thing a
/// person must be able to tell about a row before they press it.
///
/// `open` says the submenu is showing, and the row is then lit whether or not
/// the pointer is on it. A submenu hanging off a row that looks like every
/// other row is a column that appears to have come from nowhere, and the
/// pointer has by then left the row on its way into what it opened.
pub(crate) fn submenu_entry(
    workspace: &Workspace,
    key: &str,
    label: impl Into<String>,
    open: bool,
    action: WorkspaceAction,
) -> Box<dyn Element> {
    build_row(
        workspace,
        key,
        label.into(),
        Chevron::Showing(open),
        true,
        Some(action),
    )
}

/// Whether a row has a chevron, and whether what it opens is open.
#[derive(Copy, Clone)]
enum Chevron {
    /// No submenu: an entry that does something when it is pressed.
    None,
    /// A submenu, showing or not.
    Showing(bool),
}

/// An entry that is there and cannot be pressed.
///
/// For a thing that is genuinely *this tab's* and momentarily impossible — a
/// copy whose pane has not reported a directory yet. An entry that is merely
/// irrelevant should not be contributed at all: a menu half greyed out is a
/// menu that has to be read twice.
pub(crate) fn inert_entry(
    workspace: &Workspace,
    key: &str,
    label: impl Into<String>,
) -> Box<dyn Element> {
    build_row(workspace, key, label.into(), Chevron::None, false, None)
}

/// Every entry, live or not, with or without a chevron.
fn build_row(
    workspace: &Workspace,
    key: &str,
    label: String,
    chevron: Chevron,
    live: bool,
    action: Option<WorkspaceAction>,
) -> Box<dyn Element> {
    let ui = workspace.fonts().ui;
    let state = workspace.tab_context_menu().control(key);

    let row = Hoverable::new(state, move |mouse| {
        let mut line = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Text::new(label.clone(), ui, LABEL_SIZE)
                    .with_color(if live {
                        theme().text_primary
                    } else {
                        theme().text_muted
                    })
                    .finish(),
            );

        let showing = match chevron {
            Chevron::None => false,
            Chevron::Showing(open) => {
                line.add_child(Expanded::new(1., Empty::new().finish()).finish());
                line.add_child(
                    Icon::new(Lucide::ChevronRight, CHEVRON_SIZE)
                        .with_color(theme().text_muted)
                        .finish(),
                );
                open
            }
        };

        Container::new(line.finish())
            .with_background_color(if showing || (live && mouse.is_hovered()) {
                theme().overlay_1
            } else {
                Color::TRANSPARENT
            })
            .with_horizontal_padding(ROW_INSET)
            .with_vertical_padding(6.)
            .finish()
    });

    // A dead control has no click handler at all rather than a handler that
    // returns early, which is the rule everywhere else in this application.
    match action.filter(|_| live) {
        Some(action) => row
            .on_click(move |_, ctx, _| ctx.dispatch_typed_action(action))
            .finish(),
        None => row.finish(),
    }
}

/// The whole popup: the entries a slot put in it, and whatever hangs off one.
pub(super) fn render(workspace: &Workspace, app: &AppContext) -> Box<dyn Element> {
    let slot = crate::plugins::tabs::TAB_MENU_ENTRIES;
    let rows = workspace
        .host()
        .slots()
        .map(slot, |build| build(workspace, app));
    let bands = workspace.host().slots().orders(slot);

    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch);

    let mut last: Option<i32> = None;
    for (row, order) in rows.into_iter().zip(bands) {
        // A contribution with nothing to say is not a row, and must not be a
        // band either: counting it would put a rule under the last entry of a
        // menu whose final band drew nothing. See `nothing`.
        if is_absent(row.as_ref()) {
            continue;
        }
        let band = order.div_euclid(BAND);
        if last.is_some_and(|previous| previous != band) {
            column.add_child(divider());
        }
        last = Some(band);
        column.add_child(row);
    }

    let popup = ConstrainedBox::new(
        Container::new(column.finish())
            .with_background_color(theme().surface_raised)
            .with_border(Border::all(1.).with_border_color(theme().overlay_2))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(MENU_RADIUS)))
            .with_vertical_padding(6.)
            .finish(),
    )
    .with_width(MENU_WIDTH)
    .finish();

    // The one submenu there is, and it is not a popup. See this file's doc.
    if !workspace.tab_menu().is_open() {
        return popup;
    }
    let mut stack = Stack::new().with_child(popup);
    stack.add_anchored_overlay_child(super::tab_menu::render(workspace), BESIDE);
    stack.finish()
}

/// Where a submenu lands: off the popup's top-right corner, overlapping it by
/// a hair so the pointer crossing between the two never leaves both.
const BESIDE: AnchorTo = AnchorTo {
    parent: Corner::TopRight,
    child: Corner::TopLeft,
    offset: vec2f(-2., 0.),
    keep_on_screen: true,
    keep_clear_of_parent: false,
};

/// The hairline between two groups of entries.
fn divider() -> Box<dyn Element> {
    Container::new(
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(theme().overlay_2)
                .finish(),
        )
        .with_height(1.)
        .finish(),
    )
    .with_margin_top(4.)
    .with_margin_bottom(4.)
    .finish()
}

/// What a row's secondary press dispatches.
pub(super) fn open(tab: TabId, pane: PaneId) -> WorkspaceAction {
    WorkspaceAction::TabMenu(TabMenuAction::Open { tab, pane })
}
