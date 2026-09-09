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

use crook_plugin::ActionName;

use crate::tab::{PaneId, TabId};
use crate::text_input::TextInput;
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

/// The size the chord beside an entry is printed at.
///
/// Smaller than the label, because it is the answer to a question nobody asked
/// yet: a person reads the row, and the chord is what they find when they
/// wonder whether there is a faster way to do it again.
const CHORD_SIZE: f32 = 10.5;

/// The diameter of one colour swatch.
const SWATCH_SIZE: f32 = 16.;

/// The gap between the ring a chosen swatch wears and the swatch itself.
const SWATCH_RING: f32 = 2.;

/// The gap between two swatches, measured between their rings.
const SWATCH_GAP: f32 = 4.;

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
    /// The pressable rows the last frame drew, in the order it drew them.
    ///
    /// Written while the menu is being built and read by the keyboard, and
    /// there is no other way the two could agree about what "the next row" is:
    /// the entries come from a slot, so nothing outside a render knows how
    /// many there are, which of them a contribution declined to draw this
    /// frame, or what any of them does. `PaneBlocks` keeps the block menu's
    /// corner the same way and for the same reason.
    ///
    /// Only the rows a *press* would do something to: an inert entry and the
    /// row a rename is being typed into are both skipped, because Enter on
    /// either of them would be a key that looks aimed and does nothing.
    rows: RefCell<Vec<(String, WorkspaceAction)>>,
    /// The row the keyboard is standing on, by key.
    ///
    /// By key rather than by position, for the reason every other identity in
    /// this application is not an index: the list is rebuilt every frame and a
    /// contribution that stops having something to say takes its row with it,
    /// so a remembered number would come back pointing at its neighbour.
    selected: RefCell<Option<String>>,
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

    /// Whether the keyboard is standing on this row.
    pub(crate) fn is_selected(&self, key: &str) -> bool {
        self.selected.borrow().as_deref() == Some(key)
    }

    /// Starts a frame's list of pressable rows.
    fn begin_rows(&self) {
        self.rows.borrow_mut().clear();
    }

    /// Records one, in the order it was drawn.
    fn add_row(&self, key: &str, action: WorkspaceAction) {
        self.rows.borrow_mut().push((key.to_owned(), action));
    }

    /// Steps the keyboard `by` rows, or onto an end of the menu from nothing,
    /// and reports whether it moved.
    ///
    /// Clamped rather than wrapped, which is the worktree list's rule and the
    /// same reasoning: a short list read top to bottom, where an arrow that
    /// jumped from the last row to the first is one nobody can press twice
    /// with confidence.
    pub(crate) fn move_selection(&self, by: isize) -> bool {
        let rows = self.rows.borrow();
        if rows.is_empty() {
            return false;
        }
        let last = rows.len() as isize - 1;
        let at = self
            .selected
            .borrow()
            .as_deref()
            .and_then(|key| rows.iter().position(|(row, _)| row == key));
        let next = match at {
            Some(at) => (at as isize + by).clamp(0, last),
            None if by < 0 => last,
            None => 0,
        };
        let next = rows[next as usize].0.clone();
        let mut selected = self.selected.borrow_mut();
        let moved = selected.as_deref() != Some(next.as_str());
        *selected = Some(next);
        moved
    }

    /// What pressing the selected row would dispatch.
    ///
    /// `None` when nothing is selected, and also when the row that was
    /// selected is not in the frame that was last drawn — a contribution can
    /// stop having something to say between one frame and the next, and
    /// running what it *used* to do would be acting on a row nobody can see.
    pub(crate) fn selected_action(&self) -> Option<WorkspaceAction> {
        let selected = self.selected.borrow();
        let key = selected.as_deref()?;
        self.rows
            .borrow()
            .iter()
            .find(|(row, _)| row == key)
            .map(|(_, action)| *action)
    }

    /// Forgets every hover and press the menu was holding.
    ///
    /// Called when it closes: every row is about to stop existing without
    /// seeing a hover-out, and the next opening would come back with an entry
    /// lit under a pointer that is somewhere else. The keyboard's row goes
    /// with them, and so does the list it indexes — a menu that is not up must
    /// not have an action Enter could still find.
    pub(crate) fn forget_hover_state(&self) {
        for state in self.controls.borrow().values() {
            state.lock().reset_interaction_state();
        }
        self.selected.borrow_mut().take();
        self.rows.borrow_mut().clear();
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

/// An entry that is being typed into, in place of the row it replaces.
///
/// Warp renames a tab by turning its row into a field, and so does this: the
/// entry a person pressed becomes the thing they type the new name into, in
/// the column they pressed it in, rather than a dialog somewhere else that has
/// to say which tab it is about.
///
/// The field belongs to the plugin that claimed it — see
/// [`Host::claim_field`](crate::plugin::Host::claim_field) — and what decides
/// whether the keyboard is in it is that claim rather than anything here. This
/// only draws it.
pub(crate) fn field_entry(
    workspace: &Workspace,
    key: &str,
    field: &TextInput,
    placeholder: &'static str,
) -> Box<dyn Element> {
    Container::new(
        super::text_field::TextField::new(
            field.clone(),
            workspace.clipboard().clone(),
            workspace.fonts(),
            workspace.tab_context_menu().control(key),
            placeholder,
        )
        .finish(),
    )
    .with_horizontal_padding(ROW_INSET)
    .with_vertical_padding(2.)
    .finish()
}

/// A row of colour swatches, with the one in force ringed.
///
/// The only entry in the menu that is not a line of text, and the only one
/// that is a row of controls rather than one — which is why the shell offers
/// it rather than leaving a plugin to build it out of the primitives. Warp's
/// menu ends the same way, and the first swatch is the one with a line through
/// it: taking a colour off is a colour, not a separate entry.
///
/// `colors` is a run of `(key, fill, chosen, action)` — the shell resolves no
/// colour of its own, because which six a tab may be is the tab model's
/// business and not the popup's.
pub(crate) fn swatch_entry(
    workspace: &Workspace,
    swatches: Vec<(String, Option<Color>, bool, WorkspaceAction)>,
) -> Box<dyn Element> {
    let mut row = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_spacing(SWATCH_GAP);

    for (key, fill, chosen, action) in swatches {
        let state = workspace.tab_context_menu().control(&key);
        row.add_child(
            Hoverable::new(state, move |mouse| {
                // Ringed when it is the one in force, and again under the
                // pointer: the ring is what a swatch has instead of a hover
                // background, because a background behind a disc reads as a
                // second, squarer swatch.
                let ring = if chosen {
                    theme().text_primary
                } else if mouse.is_hovered() {
                    theme().text_muted
                } else {
                    Color::TRANSPARENT
                };
                Container::new(
                    ConstrainedBox::new(
                        Container::new(match fill {
                            Some(_) => Empty::new().finish(),
                            // The swatch that takes a colour off, drawn as the
                            // absence it is rather than as a seventh colour.
                            None => Icon::new(Lucide::X, SWATCH_SIZE * 0.6)
                                .with_color(theme().text_muted)
                                .finish(),
                        })
                        .with_background_color(fill.unwrap_or(Color::TRANSPARENT))
                        .with_border(Border::all(1.).with_border_color(match fill {
                            Some(_) => Color::TRANSPARENT,
                            None => theme().overlay_3,
                        }))
                        .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                        .finish(),
                    )
                    .with_width(SWATCH_SIZE)
                    .with_height(SWATCH_SIZE)
                    .finish(),
                )
                .with_uniform_padding(SWATCH_RING)
                .with_border(Border::all(1.).with_border_color(ring))
                .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)))
                .finish()
            })
            .on_click(move |_, ctx, _| ctx.dispatch_typed_action(action))
            .finish(),
        );
    }

    Container::new(row.finish())
        .with_horizontal_padding(ROW_INSET)
        .with_vertical_padding(4.)
        .finish()
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
    let menu = workspace.tab_context_menu();
    let state = menu.control(key);
    if live && let Some(action) = action {
        menu.add_row(key, action);
    }
    // The keyboard's row is lit exactly as the pointer's is. There is no
    // second treatment for it, because a person who arrived by the arrows and
    // a person who arrived by the pointer are looking at the same question.
    let picked = menu.is_selected(key);

    // The chord that reaches this entry, if one does. The key a row is built
    // with *is* the name of the action it runs — see `contribute` in the tabs
    // plugin — so the menu can print exactly what the Keyboard Shortcuts page
    // prints without being told a thing, and a person who rebound it reads
    // their own chord here. The first of them, because a row is one line and
    // the zoom's four spellings would fill it.
    let chord = ActionName::parse(key)
        .ok()
        .and_then(|name| workspace.keybindings().chords_for(&name).into_iter().next());

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

        // One spacer, whatever ends up after it: a chord, a chevron, or the
        // chord and then the chevron. Two would divide the gap between them
        // and leave the chord adrift in the middle of the row.
        if chord.is_some() || matches!(chevron, Chevron::Showing(_)) {
            line.add_child(Expanded::new(1., Empty::new().finish()).finish());
        }
        if let Some(chord) = &chord {
            line.add_child(
                Text::new(chord.clone(), ui, CHORD_SIZE)
                    .with_color(theme().text_muted)
                    .finish(),
            );
        }

        let showing = match chevron {
            Chevron::None => false,
            Chevron::Showing(open) => {
                line.add_child(
                    Icon::new(Lucide::ChevronRight, CHEVRON_SIZE)
                        .with_color(theme().text_muted)
                        .finish(),
                );
                open
            }
        };

        Container::new(line.finish())
            .with_background_color(if showing || picked || (live && mouse.is_hovered()) {
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
    // Before the contributions run, because each of them records its own row
    // as it builds it. See `TabContextMenuState::rows`.
    workspace.tab_context_menu().begin_rows();
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
