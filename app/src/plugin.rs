//! What a plugin is, in this application.
//!
//! [`crook_plugin`] holds the vocabulary — identities, manifests, the two
//! registries and the guard that takes a contribution back out — and knows
//! nothing about Crook. This is the other half: what a contribution *is* here
//! (a closure that builds an element from the workspace), what a handler is (a
//! closure that reaches the workspace), and the [`Host`] that hands both to a
//! plugin while it builds.
//!
//! # Everything Crook does is meant to arrive through here
//!
//! Not as a slogan. An API the application itself does not use is an API
//! nobody has built anything with, and the way to find out whether a slot is
//! the right shape is to move something real onto it and see what breaks. The
//! order features move is in `docs/plugins.md`; each one that lands is a
//! surface a stranger's plugin can reach for the same reason Crook's own can.
//!
//! # What a plugin may not do
//!
//! Panic during `build`, and nothing else is forbidden yet — a native plugin is
//! compiled into the binary and reviewed as the binary. The rule that governs
//! it is the one `settings.rs` states for its own file and that everything
//! since has kept: **nothing here may cost a person their window**. A plugin
//! that fails to build is skipped by name, with one line in the log, and the
//! window opens without whatever it was contributing.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use crookui_core::event::Keystroke;
use crookui_core::prelude::*;
use crookui_core::{AppContext, Element};

pub use crook_plugin::{
    ActionName, Actions, Cardinality, Complaint, EntryId, Manifest, PluginId, Registration, SlotId,
    Slots, Tier,
};

use crook_plugin_api::Event;

use crate::keybindings::{Rule, Source, rule_from};
use crate::plugins::tabs::TabRow;
use crate::text_input::TextInput;
use crate::workspace::{Category, Fonts, Workspace};

/// A section of the sidebar: a button at its foot, and what the window shows
/// while it is chosen.
///
/// Declared by `crook/window`, which owns the sidebar; contributed to by
/// whichever plugin the section belongs to.
pub const SIDEBAR_SECTION: SlotId = SlotId::new("sidebar.section");

/// Where a page of the settings goes.
///
/// Declared by `crook/settings`, which owns the rail; contributed to by
/// whichever plugin the page belongs to, which for the five Crook ships is one
/// plugin each.
pub const SETTINGS_PAGE: SlotId = SlotId::new("settings.page");

/// What a plugin contributes to a slot: something that can build an element
/// out of the workspace, every frame.
///
/// A closure taking `&Workspace` rather than a value captured when the plugin
/// built, because `View::render` is immutable and runs again for every frame:
/// a contribution that captured what it wanted to draw would be drawing the
/// state of the window at the moment the plugin loaded.
pub type UiContribution = Box<dyn Fn(&Workspace, &AppContext) -> Box<dyn Element>>;

/// What a plugin contributes to a slot that is drawn once per row.
///
/// Two differences from [`UiContribution`], and both are the row's doing.
///
/// It is handed the row it is being drawn on, because a slot drawn seven times
/// asks the same plugin seven questions and "which one is this" is the whole
/// of what distinguishes them.
///
/// And it may answer `None`, which [`UiContribution`] has no need for: the
/// header's slot is empty or it is not, whereas a mark per tab is something a
/// plugin may want on *some* rows — the worktrees, the failures — and nowhere
/// else. `None` means "as it was": the host draws whatever it would have drawn
/// with no plugin there at all, rather than a hole where a mark goes.
pub(crate) type RowContribution =
    Box<dyn Fn(&Workspace, &TabRow<'_>, &AppContext) -> Option<Box<dyn Element>>>;

/// What answers to an [`ActionName`].
///
/// `&mut Workspace` and its context, which is what every existing handler in
/// `Workspace::handle_action` already receives — a named action is the same
/// thing an enum variant was, addressable by people who cannot add a variant.
pub type ActionHandler = Box<dyn Fn(&mut Workspace, &mut ViewContext<Workspace>)>;

/// What one page of the settings is made of, built fresh every frame.
///
/// A `Vec<Category>` rather than an element, because the settings page's
/// search filters *rows* — which rows survive decides which category is first
/// and therefore which one draws no divider above itself. A page that handed
/// back an element would have to be searched by looking at pixels.
pub(crate) type SettingsContribution = Box<dyn Fn(&Workspace, &AppContext) -> Vec<Category>>;

/// One section of the sidebar.
///
/// The two halves are built together rather than by two closures, because a
/// section's list and its detail are two views of one answer: the settings
/// page works out which page is showing from a query that filters both, and
/// the Plugins page's card is about whatever its list has selected. Two
/// builders would work it out twice and could disagree.
pub(crate) struct SidebarSection {
    /// What the button says.
    pub(crate) title: String,
    /// What it draws.
    pub(crate) icon: Lucide,
    /// The sidebar's body and the window's, in that order.
    pub(crate) build: SectionContribution,
}

/// What a section draws: the sidebar's body, then the window's.
pub(crate) type SectionContribution =
    Box<dyn Fn(&Workspace, &AppContext) -> (Box<dyn Element>, Box<dyn Element>)>;

/// One sidebar section, as something `Copy`.
///
/// The same trick [`PageId`] plays, for the same reason.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SectionId(usize);

/// One page of the settings: what the rail calls it, and what is on it.
pub(crate) struct SettingsPage {
    /// What the rail row and the page's heading both say.
    pub(crate) title: String,
    /// What is on it.
    pub(crate) build: SettingsContribution,
}

/// One settings page, as something `Copy`.
///
/// The same trick [`ActionId`] plays and for the same reason: a page is named
/// by `owner/entry`, `SettingsAction` is `Copy`, and a `String` is not. The
/// key is the identity — an id whose page has been disabled resolves to a key
/// no page answers to, and the rail falls back to the first page there is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageId(usize);

/// Whether a plugin's floating surface is up.
///
/// A palette, a modal, anything that covers the window and has to take the
/// keyboard away from the pane under it. The plugin holds one of these and
/// raises it while its surface is showing; the workspace reads it in
/// `sync_input_keys`, and the host only consults that surface's key claim
/// while it is raised.
///
/// A shared flag rather than a question asked of the plugin, because the thing
/// that knows is a view the host cannot reach and the thing that asks is a
/// render that holds no `&mut`.
#[derive(Clone, Default)]
pub struct Showing(Rc<Cell<bool>>);

impl Showing {
    /// Says whether the surface is up.
    pub fn set(&self, showing: bool) {
        self.0.set(showing);
    }

    /// Whether it is.
    pub fn get(&self) -> bool {
        self.0.get()
    }
}

/// One surface that hangs off a place in the interface: whose it is, whether
/// it is up, and how to take it down. See [`Host::claim_panel`].
type Panel = (PluginId, Showing, Rc<dyn Fn()>);

/// What a surface does with a keystroke while it is up.
///
/// It names an action rather than doing anything, which is what keeps one
/// dispatch path: a key a palette claims and a key somebody bound in their own
/// file both end as [`WorkspaceAction::Run`](crate::workspace::WorkspaceAction).
/// A surface that returns `None` lets the keystroke go on to the bindings and
/// then to the pane.
pub type KeyClaim = Box<dyn Fn(&Keystroke) -> Option<ActionName>>;

/// What a plugin does when something it is watching happens.
///
/// Shaped like [`ActionHandler`] and dispatched like one, because it is the
/// same thing arriving for a different reason: something outside the plugin
/// happened, and the plugin gets the workspace and a context to answer it
/// with. Behind an [`Rc`] so a dispatch can take a handle and let go of the
/// host before it calls anything — a watcher that reached back into the host
/// while the host was lending out its list would be a borrow inside a borrow.
pub type EventWatcher = Rc<dyn Fn(&mut Workspace, &Event, &mut ViewContext<Workspace>)>;

/// Which kind of happening a watcher was registered for.
///
/// The registry is keyed by this rather than filtered by the watcher, so that
/// a plugin refused a grant registers nothing for that kind and hears nothing
/// at all — instead of being handed events it is not allowed and having them
/// dropped somewhere further in, where nobody reading the dispatch could tell
/// that a grant was what decided it.
///
/// One variant per [`Event`], and it stays that way: a watcher asks for the
/// thing it wants to hear, never for "events".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    /// A command in a pane finished — [`Event::CommandFinished`].
    Commands,
    /// A program in a pane rang the bell — [`Event::Bell`].
    Bells,
}
/// What decides whether a plugin's field is the one the keyboard belongs to.
///
/// Asked every time anything could have changed the answer, and it must be
/// cheap and must not look at anything but the workspace: it runs inside
/// [`Workspace::sync_input_keys`](crate::workspace::Workspace::sync_input_keys),
/// which is called from the middle of applying an action.
pub type FieldClaim = Box<dyn Fn(&Workspace) -> bool>;

/// A registered action, as something `Copy`.
///
/// [`WorkspaceAction`](crate::workspace::WorkspaceAction) is compared by value
/// in a dozen places and has to stay `Copy`; an [`ActionName`] is a `String`.
/// So a chord and a menu entry carry this instead — an index into the order
/// actions were registered in, which [`Host`] keeps.
///
/// The **name** is still the identity. Two ids that resolve to one name run one
/// handler, and an id whose plugin has been disabled resolves to a name nothing
/// answers to, which is a chord that does nothing rather than a dangling
/// pointer into a table.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActionId(usize);

/// Everything a plugin may register, and the record of who registered what.
///
/// A plugin never keeps its own guards: [`Host`] holds them, filed under the
/// plugin that made them, so that "disable this plugin" is one `retain` and
/// every surface it touched goes back to what it was. That is the same
/// arrangement Cordis reaches for with a fibre state machine, and here it is
/// a vector of [`Registration`]s being dropped.
pub struct Host {
    /// The families a plugin needs before there is a workspace to ask.
    ///
    /// A contribution is handed `&Workspace` and can ask it anything; a plugin
    /// *building* an entity cannot, because the workspace does not exist until
    /// every plugin has built. This is the short list of what has to arrive
    /// the other way, and it is short on purpose.
    fonts: Fonts,
    /// What each plugin has been allowed to do, by `owner/name`.
    ///
    /// Here for the reason [`Host::fonts`] is: a plugin that asks for
    /// something while it is *building* cannot ask the workspace, because the
    /// workspace does not exist until every plugin has built. This is the
    /// short list of what has to arrive the other way.
    grants: BTreeMap<String, Vec<String>>,
    slots: Slots<UiContribution>,
    /// The slots drawn once per row of the tab panel, which are their own
    /// registry because what a contribution to them *is* is different: it is
    /// asked about a row, and it may decline that row.
    rows: Slots<RowContribution>,
    /// The settings pages, which are a slot of their own because what a
    /// contribution to them *is* is different: rows to be searched rather than
    /// an element to be drawn.
    pages: Slots<SettingsPage>,
    /// The sidebar's sections, which are a slot of their own for the reason
    /// the settings pages are: what a contribution to them *is* is different.
    sections: Slots<SidebarSection>,
    /// Every section key that has ever been registered, in order; the index is
    /// a [`SectionId`]. Appended to and never removed from, as
    /// [`Host::action_names`] is.
    section_keys: Vec<String>,
    /// Every settings page key that has ever been registered, in order; the
    /// index is a [`PageId`]. Appended to and never removed from, for the
    /// reason [`Host::action_names`] is.
    page_keys: Vec<String>,
    actions: Actions<ActionHandler>,
    /// Every action name that has ever been registered here, in order.
    ///
    /// Appended to and never removed from, because an [`ActionId`] is an index
    /// into it and an index that moved would be a chord that started meaning
    /// something else. Disabling a plugin takes its handler out of the
    /// registry, not its name out of here — which is what makes the id safe to
    /// hold on to.
    action_names: Vec<ActionName>,
    /// The actions that are meant to be *offered*, with what to call them.
    ///
    /// Every action is reachable by name; a command is one a person should be
    /// able to find without knowing the name. The arrow keys a palette binds
    /// to itself are actions and not commands, which is the whole of the
    /// distinction.
    commands: Vec<(PluginId, ActionName, String)>,
    /// The chords a plugin asked for, as the weakest layer of the
    /// keybindings.
    ///
    /// The weakest, so a plugin cannot take `cmd+t` away from the tabs by
    /// loading first, and a person's own file wins over both. They are
    /// [`Rule`]s like every other binding — see [`crate::keybindings`] — which
    /// is what lets a person remove one by name.
    suggested: Vec<(PluginId, Rule)>,
    /// The floating surfaces plugins own, and what each does with a keystroke
    /// while it is up.
    surfaces: Vec<(PluginId, Showing, KeyClaim)>,
    /// Who asked to be told when something happens, and what about.
    ///
    /// A list rather than a slot: nobody *owns* the fact that a command ended
    /// or that a bell rang, every watcher of that kind hears it, and the order
    /// they hear it in is load order because there is nothing better to sort
    /// it by.
    watchers: Vec<(PluginId, Watch, EventWatcher)>,
    /// The text fields plugins own, by `owner/name`, and when each of them is
    /// the one the keyboard belongs to.
    ///
    /// A field is the one thing a surface could not do for itself. A plugin
    /// could put a popup on screen, draw a box in it and claim Escape, and
    /// still have nowhere for a keystroke to land — because which field is
    /// listening is a fact the element tree cannot work out and the workspace
    /// used to answer by naming, in source, every field there was. There were
    /// two, and neither was a plugin's.
    fields: Vec<(PluginId, String, TextInput, FieldClaim)>,
    /// The surfaces among those that hang off a *place* rather than over the
    /// window, and how to take each one down. See [`Host::claim_panel`].
    panels: Vec<Panel>,
    /// Whose registrations are being made right now. Set around each plugin's
    /// `build` so a plugin cannot register in another's name by accident.
    building: Option<PluginId>,
    kept: Vec<(PluginId, Registration)>,
    /// Every plugin the binary carries, kept so that one switched off can be
    /// switched back on without restarting.
    ///
    /// A plugin object is not its registrations: `build` is what makes those,
    /// and it can be run again. What it costs is that the objects outlive
    /// being disabled, which for a `struct Usage;` is nothing.
    plugins: Vec<Box<dyn Plugin>>,
    /// What every plugin the binary carries says about itself, in load order,
    /// whether or not it is loaded.
    ///
    /// Separate from `plugins` because it is filled *before* the second pass
    /// and `plugins` cannot be: `ready` runs with the objects lent out, and a
    /// plugin whose whole job is to offer a switch per plugin has to be able
    /// to ask how many there are while it does.
    carried: Vec<&'static Manifest>,
    /// The plugins that built, in the order they did.
    loaded: Vec<&'static Manifest>,
    /// The ones that did not, and what went wrong.
    refused: Vec<(PluginId, String)>,
    /// What the thing that invoked the next action had to say to it.
    ///
    /// An action is a `Fn(&mut Workspace, &mut ViewContext)` and takes no
    /// argument, because a chord and a palette row have nothing to say. Some
    /// callers do — a row chosen out of a picker is a row with a key, a menu
    /// entry is an entry about something — and this is where that is left for
    /// the handler to take. Set immediately before the action is dispatched
    /// and taken when it runs, which is safe because a dispatched action is
    /// applied after the whole tree has seen the event: one press, one thing
    /// said, one action.
    said: Voice,
    /// What the command whose menu an entry was pressed in printed.
    ///
    /// Beside [`Host::said`] and for a related reason — something the press
    /// knows and the handler needs — but written once per press rather than
    /// per dispatch, because it can be a megabyte. See
    /// [`Host::set_pressed_output`].
    pressed_output: Rc<std::cell::RefCell<Option<String>>>,
}

/// A place to leave something for the next action that runs to take.
///
/// A handle rather than a field, because the thing that says something is
/// usually an element's click handler: it holds no workspace and no host, only
/// what it captured when it was built. So it captures one of these.
#[derive(Clone, Default)]
pub struct Voice(Rc<RefCell<String>>);

impl Voice {
    /// Leaves something for the next action.
    pub fn say(&self, what: impl Into<String>) {
        *self.0.borrow_mut() = what.into();
    }

    /// Takes it, leaving nothing behind.
    pub fn taken(&self) -> String {
        std::mem::take(&mut self.0.borrow_mut())
    }
}

impl Host {
    /// A host with nothing registered.
    pub fn new(fonts: Fonts, grants: BTreeMap<String, Vec<String>>) -> Self {
        Self {
            fonts,
            grants,
            pressed_output: Rc::default(),
            slots: Slots::new(),
            rows: Slots::new(),
            pages: Slots::new(),
            page_keys: Vec::new(),
            sections: Slots::new(),
            section_keys: Vec::new(),
            actions: Actions::new(),
            action_names: Vec::new(),
            commands: Vec::new(),
            suggested: Vec::new(),
            surfaces: Vec::new(),
            watchers: Vec::new(),
            fields: Vec::new(),
            panels: Vec::new(),
            building: None,
            kept: Vec::new(),
            plugins: Vec::new(),
            carried: Vec::new(),
            loaded: Vec::new(),
            refused: Vec::new(),
            said: Voice::default(),
        }
    }

    /// Leaves something for the next action to be run to take.
    ///
    /// See the field. `&self` rather than `&mut self` because the thing that
    /// says it is usually an element handler, which holds neither.
    pub fn say(&self, what: impl Into<String>) {
        self.said.say(what);
    }

    /// Writes down what the command whose menu an entry was just pressed in
    /// printed, for a plugin allowed to ask.
    ///
    /// **Written at the press and not before.** A subject is built on every
    /// frame a menu is open and can afford to carry a command line; output can
    /// be a megabyte, so it is copied once, when somebody has actually pressed
    /// something, and the plugin asks for it with `Request::Output`.
    ///
    /// **And not cleared afterwards**, which is the ordering the whole thing
    /// turns on: a menu closes as its entry runs, and what the entry asked for
    /// is served after that — so a cell emptied on the way out would be a
    /// plugin told there was no block a moment after it was shown one. What
    /// keeps it honest instead is that the request may only be raised out of
    /// an action a person caused, so nothing can read it except the press that
    /// filled it.
    pub(crate) fn set_pressed_output(&self, text: Option<String>) {
        *self.pressed_output.borrow_mut() = text;
    }

    /// What that was, for whoever is serving the request.
    pub(crate) fn pressed_output(&self) -> Option<String> {
        self.pressed_output.borrow().clone()
    }

    /// The handle an element holds to be able to say something.
    pub fn voice(&self) -> Voice {
        self.said.clone()
    }

    /// Takes it, leaving nothing behind.
    ///
    /// Taken rather than read, so an action reached by a chord a moment later
    /// is not handed what somebody clicked before it.
    pub fn said(&self) -> String {
        self.said.taken()
    }

    /// The plugin whose registrations are being made.
    ///
    /// Outside a `build` there is none, and registering then is a bug in the
    /// host rather than in a plugin — so it is named after the host.
    fn who(&self) -> PluginId {
        self.building
            .clone()
            .unwrap_or_else(|| PluginId::parse("crook/host").expect("a literal that parses"))
    }

    /// Declares a slot this plugin owns and will render.
    pub fn declare_slot(&mut self, slot: SlotId, cardinality: Cardinality) {
        let who = self.who();
        let registration = self.slots.declare(&who, slot, cardinality);
        self.kept.push((who, registration));
    }

    /// Contributes something to a slot somebody declares.
    ///
    /// `order` places it among the others: lower is earlier, and two entries
    /// with the same order keep the order their plugins loaded in.
    pub fn contribute(
        &mut self,
        slot: SlotId,
        entry: impl Into<String>,
        order: i32,
        build: impl Fn(&Workspace, &AppContext) -> Box<dyn Element> + 'static,
    ) {
        let who = self.who();
        let registration = self.slots.contribute(
            &who,
            slot,
            EntryId::new(entry),
            order,
            Box::new(build) as UiContribution,
        );
        self.kept.push((who, registration));
    }

    /// Declares a slot that is drawn once per row of the tab panel.
    ///
    /// Separate from [`declare_slot`](Self::declare_slot) because the two
    /// registries hold different things, and a plugin contributing to the
    /// wrong one is told so by name rather than by drawing nothing: a row slot
    /// is not somewhere an ordinary contribution can go, since an ordinary
    /// contribution has no way to ask which row it is on.
    pub fn declare_row_slot(&mut self, slot: SlotId, cardinality: Cardinality) {
        let who = self.who();
        let registration = self.rows.declare(&who, slot, cardinality);
        self.kept.push((who, registration));
    }

    /// Contributes something drawn per row to a slot somebody declares.
    pub fn contribute_row(
        &mut self,
        slot: SlotId,
        entry: impl Into<String>,
        order: i32,
        build: impl Fn(&Workspace, &TabRow<'_>, &AppContext) -> Option<Box<dyn Element>> + 'static,
    ) {
        let who = self.who();
        let registration = self.rows.contribute(
            &who,
            slot,
            EntryId::new(entry),
            order,
            Box::new(build) as RowContribution,
        );
        self.kept.push((who, registration));
    }

    /// Registers something the application can be asked to do by name.
    ///
    /// The id it hands back is what a chord or a menu entry carries; it is also
    /// obtainable later from the name, so a plugin that only wants the action
    /// to exist may throw it away.
    pub fn register_action(
        &mut self,
        action: ActionName,
        handler: impl Fn(&mut Workspace, &mut ViewContext<Workspace>) + 'static,
    ) -> ActionId {
        let who = self.who();
        let registration =
            self.actions
                .register(&who, action.clone(), Box::new(handler) as ActionHandler);
        self.kept.push((who, registration));

        match self.action_names.iter().position(|known| *known == action) {
            Some(index) => ActionId(index),
            None => {
                self.action_names.push(action);
                ActionId(self.action_names.len() - 1)
            }
        }
    }

    /// Adds a page to the settings.
    ///
    /// `entry` is this page's name within the plugin that adds it, so the key
    /// a person writes on the command line and the key the state remembers is
    /// `owner/entry` — stable across restarts and across every other plugin
    /// they have installed.
    pub(crate) fn add_settings_page(
        &mut self,
        entry: &str,
        title: impl Into<String>,
        order: i32,
        build: impl Fn(&Workspace, &AppContext) -> Vec<Category> + 'static,
    ) -> PageId {
        let who = self.who();
        let key = format!("{who}/{entry}");
        let registration = self.pages.contribute(
            &who,
            SETTINGS_PAGE,
            EntryId::new(entry),
            order,
            SettingsPage {
                title: title.into(),
                build: Box::new(build) as SettingsContribution,
            },
        );
        self.kept.push((who, registration));

        match self.page_keys.iter().position(|known| *known == key) {
            Some(index) => PageId(index),
            None => {
                self.page_keys.push(key);
                PageId(self.page_keys.len() - 1)
            }
        }
    }

    /// Adds a section to the sidebar.
    ///
    /// `entry` names it within the plugin, so the key a person's state
    /// remembers is `owner/entry` — stable across restarts and across every
    /// other plugin they have installed.
    pub(crate) fn add_sidebar_section(
        &mut self,
        entry: &str,
        title: impl Into<String>,
        icon: Lucide,
        order: i32,
        build: impl Fn(&Workspace, &AppContext) -> (Box<dyn Element>, Box<dyn Element>) + 'static,
    ) -> SectionId {
        let who = self.who();
        let key = format!("{who}/{entry}");
        let registration = self.sections.contribute(
            &who,
            SIDEBAR_SECTION,
            EntryId::new(entry),
            order,
            SidebarSection {
                title: title.into(),
                icon,
                build: Box::new(build) as SectionContribution,
            },
        );
        self.kept.push((who, registration));

        match self.section_keys.iter().position(|known| *known == key) {
            Some(index) => SectionId(index),
            None => {
                self.section_keys.push(key);
                SectionId(self.section_keys.len() - 1)
            }
        }
    }

    /// Declares the sidebar slot. Called by the plugin that owns the sidebar.
    pub(crate) fn declare_sidebar_slot(&mut self) {
        let who = self.who();
        let registration = self
            .sections
            .declare(&who, SIDEBAR_SECTION, Cardinality::List);
        self.kept.push((who, registration));
    }

    /// Every section there is, in the order their buttons are drawn.
    pub fn sidebar_sections(&self) -> Vec<(SectionId, String, Lucide)> {
        let titles = self.sections.map(SIDEBAR_SECTION, |section| {
            (section.title.clone(), section.icon)
        });
        self.sections
            .contributors(SIDEBAR_SECTION)
            .into_iter()
            .zip(titles)
            .filter_map(|((owner, entry), (title, icon))| {
                let id = self.sidebar_section_id(&format!("{owner}/{entry}"))?;
                Some((id, title, icon))
            })
            .collect()
    }

    /// The id for a section key, if a section answers to it right now.
    pub fn sidebar_section_id(&self, key: &str) -> Option<SectionId> {
        self.section_index(key)?;
        self.section_keys
            .iter()
            .position(|known| known == key)
            .map(SectionId)
    }

    /// What a section id is called: `owner/entry`.
    pub fn sidebar_section_key(&self, id: SectionId) -> Option<&str> {
        self.section_keys.get(id.0).map(String::as_str)
    }

    /// Builds one section's two halves, without building the others.
    pub(crate) fn build_sidebar_section(
        &self,
        id: SectionId,
        workspace: &Workspace,
        app: &AppContext,
    ) -> Option<(Box<dyn Element>, Box<dyn Element>)> {
        let index = self.section_index(self.sidebar_section_key(id)?)?;
        self.sections.at(SIDEBAR_SECTION, index, |section| {
            (section.build)(workspace, app)
        })
    }

    /// Where a section sits in the bar right now.
    fn section_index(&self, key: &str) -> Option<usize> {
        self.sections
            .contributors(SIDEBAR_SECTION)
            .into_iter()
            .position(|(owner, entry)| format!("{owner}/{entry}") == key)
    }

    /// Declares the settings slot itself. Called by the plugin that owns it.
    pub fn declare_settings_slot(&mut self) {
        let who = self.who();
        let registration = self.pages.declare(&who, SETTINGS_PAGE, Cardinality::List);
        self.kept.push((who, registration));
    }

    /// Every settings page there is, in rail order.
    pub fn settings_pages(&self) -> Vec<(PageId, String)> {
        let titles = self.pages.map(SETTINGS_PAGE, |page| page.title.clone());
        self.pages
            .contributors(SETTINGS_PAGE)
            .into_iter()
            .zip(titles)
            .filter_map(|((owner, entry), title)| {
                Some((self.settings_page_id(&format!("{owner}/{entry}"))?, title))
            })
            .collect()
    }

    /// The id for a page key, if a page answers to it right now.
    pub fn settings_page_id(&self, key: &str) -> Option<PageId> {
        self.page_index(key)?;
        self.page_keys
            .iter()
            .position(|known| known == key)
            .map(PageId)
    }

    /// What a page id is called: `owner/entry`.
    pub fn settings_page_key(&self, id: PageId) -> Option<&str> {
        self.page_keys.get(id.0).map(String::as_str)
    }

    /// Builds one page's rows, without building the others.
    pub(crate) fn build_settings_page(
        &self,
        id: PageId,
        workspace: &Workspace,
        app: &AppContext,
    ) -> Option<Vec<Category>> {
        let index = self.page_index(self.settings_page_key(id)?)?;
        self.pages
            .at(SETTINGS_PAGE, index, |page| (page.build)(workspace, app))
    }

    /// What one page's rail row says.
    pub fn settings_page_title(&self, id: PageId) -> Option<String> {
        let index = self.page_index(self.settings_page_key(id)?)?;
        self.pages
            .at(SETTINGS_PAGE, index, |page| page.title.clone())
    }

    /// Where a page sits in the rail right now, or `None` if no page answers
    /// to that key any more.
    fn page_index(&self, key: &str) -> Option<usize> {
        self.pages
            .contributors(SETTINGS_PAGE)
            .into_iter()
            .position(|(owner, entry)| format!("{owner}/{entry}") == key)
    }

    /// Registers an action *and* offers it under a title.
    ///
    /// The title is what a palette prints, so it is a sentence a person would
    /// recognise rather than the name — "New agent tab", not
    /// `crook/window/new-tab`.
    pub fn register_command(
        &mut self,
        action: ActionName,
        title: impl Into<String>,
        handler: impl Fn(&mut Workspace, &mut ViewContext<Workspace>) + 'static,
    ) -> ActionId {
        let who = self.who();
        self.commands.push((who, action.clone(), title.into()));
        self.register_action(action, handler)
    }

    /// Asks for a chord to reach an action, if nothing else claims it.
    ///
    /// A *default*, not a binding: it is the weakest layer of the keybindings,
    /// so a person's own file wins and so does every one of the window's own
    /// chords — a plugin cannot take `cmd+t` from the tabs. The chord is
    /// written the way a `keybindings.json` line writes one, and one that does
    /// not parse is dropped with a line in the log, exactly as that line would
    /// be.
    pub fn suggest_binding(&mut self, chord: &str, action: ActionName) {
        let who = self.who();
        match rule_from(chord, action, Source::Plugin) {
            Some(rule) => self.suggested.push((who, rule)),
            None => log::warn!("{who} asked for {chord:?}, which is not a chord"),
        }
    }

    /// Registers a floating surface, and what it does with a keystroke.
    ///
    /// The flag it hands back is how the plugin says the surface is up; while
    /// it is down the claim is never consulted and the surface counts for
    /// nothing.
    pub fn claim_surface(
        &mut self,
        keys: impl Fn(&Keystroke) -> Option<ActionName> + 'static,
    ) -> Showing {
        let who = self.who();
        let showing = Showing::default();
        self.surfaces
            .push((who, showing.clone(), Box::new(keys) as KeyClaim));
        showing
    }

    /// Asks to be told when something of `kind` happens in a pane.
    ///
    /// Whether the plugin is *allowed* to hear it is not checked here: this is
    /// the registration, and the grant is read where every other grant is —
    /// by the thing that registers, while it builds. A native plugin has no
    /// grant to read because a native plugin is the binary.
    ///
    /// A plugin that wants two kinds registers twice, which is what keeps a
    /// grant it was refused from reaching a watcher at all.
    pub fn watch(&mut self, kind: Watch, watch: EventWatcher) {
        let who = self.who();
        self.watchers.push((who, kind, watch));
    }

    /// Handles for every watcher of `kind`, so a dispatch can call them with
    /// the host no longer borrowed.
    pub fn watchers(&self, kind: Watch) -> Vec<EventWatcher> {
        self.watchers
            .iter()
            .filter(|(_, registered, _)| *registered == kind)
            .map(|(_, _, watch)| Rc::clone(watch))
            .collect()
    }

    /// Registers a surface that hangs off a *place* in the interface rather
    /// than floating over the window, and how to take it down.
    ///
    /// The same keyboard bargain [`Host::claim_surface`] makes, and one rule
    /// more. A palette floats over everything: it is still on screen after a
    /// tab switch and may go on owning its arrow keys. A panel hung under a
    /// chip in a pane is drawn by that pane and stops being drawn when the
    /// window's attention moves — and a key claim left standing then is a
    /// keyboard whose owner is nowhere on screen. So the workspace takes these
    /// down at every point it moves the attention, and `close` is how the
    /// plugin holding one finds out.
    pub fn claim_panel(
        &mut self,
        keys: impl Fn(&Keystroke) -> Option<ActionName> + 'static,
        close: impl Fn() + 'static,
    ) -> Showing {
        let showing = self.claim_surface(keys);
        let who = self.who();
        self.panels
            .push((who, showing.clone(), Rc::new(close) as Rc<dyn Fn()>));
        showing
    }

    /// Takes down every panel that is up, and says whether any was.
    ///
    /// The answer is what tells the caller a re-sync is owed: taking a panel
    /// down gives the keyboard back to a pane, and nothing else would say so.
    pub fn take_panels_down(&self) -> bool {
        let mut any = false;
        for (_, showing, close) in &self.panels {
            if showing.get() {
                close();
                showing.set(false);
                any = true;
            }
        }
        any
    }

    /// Whether any plugin's surface is up.
    ///
    /// What takes the keyboard away from the focused pane, the same way an
    /// open menu or the Themes panel does.
    pub fn a_surface_is_up(&self) -> bool {
        self.surfaces.iter().any(|(_, showing, _)| showing.get())
    }

    /// Registers a text field this plugin owns, and hands it back.
    ///
    /// The host makes the field rather than taking one, because a plugin
    /// builds before the workspace exists and there is nothing to take one
    /// from; what it gets is an [`Rc`](std::rc::Rc) it keeps and hands to an
    /// element every frame, which is how every field in this application is
    /// held.
    ///
    /// `wants_keys` is asked whenever anything could have changed the answer.
    /// The first field to say yes gets the keyboard and the panes do not, in
    /// registration order — two fields wanting it at once is a bug somewhere
    /// else, and this only decides which of them hears the next letter.
    ///
    /// The name is the field's, under this plugin: `owner/name`, so something
    /// that has to *draw* a field it does not own can find it the way it finds
    /// an action.
    pub fn claim_field(
        &mut self,
        name: &str,
        wants_keys: impl Fn(&Workspace) -> bool + 'static,
    ) -> TextInput {
        let who = self.who();
        let field = TextInput::new();
        self.fields.push((
            who.clone(),
            format!("{who}/{name}"),
            field.clone(),
            Box::new(wants_keys) as FieldClaim,
        ));
        field
    }

    /// One plugin's field, by its `owner/name`.
    pub fn field(&self, name: &str) -> Option<&TextInput> {
        self.fields
            .iter()
            .find(|(_, known, _, _)| known == name)
            .map(|(_, _, field, _)| field)
    }

    /// Whether one of them has the keyboard right now.
    ///
    /// The answer to "is there a caret on screen": a plugin's field blinks
    /// exactly as the window's own do, and the window cannot know how many
    /// there are to ask.
    pub fn a_field_has_keys(&self) -> bool {
        self.fields.iter().any(|(_, _, field, _)| field.has_keys())
    }

    /// Tells every plugin's field whether the keyboard is its, and says
    /// whether one of them took it.
    ///
    /// The loop is here rather than in the workspace because the registry is
    /// here, and because "the first claimant wins" is a rule about the
    /// registry rather than about the window. A `true` answer is a pane that
    /// must stop listening — see
    /// [`Workspace::sync_input_keys`](crate::workspace::Workspace::sync_input_keys).
    pub fn sync_fields(&self, workspace: &Workspace) -> bool {
        let mut taken = false;
        for (_, _, field, wants) in &self.fields {
            let has_keys = !taken && wants(workspace);
            field.set_has_keys(has_keys);
            taken |= has_keys;
        }
        taken
    }

    /// What a surface that is up makes of this keystroke.
    ///
    /// The first surface to claim it wins, in load order. Two modal surfaces
    /// up at once is a bug somewhere else; this is only deciding which of them
    /// hears the Escape.
    pub fn keys_for(&self, keystroke: &Keystroke) -> Option<ActionId> {
        let name = self
            .surfaces
            .iter()
            .filter(|(_, showing, _)| showing.get())
            .find_map(|(_, _, claim)| claim(keystroke))?;
        self.action(&name)
    }

    /// Every chord the plugins that are loaded asked for.
    ///
    /// Handed to the keybindings once the plugins have built, which is the
    /// only moment this is knowable: a plugin can be switched off, and one
    /// that failed to load has already had its rules taken back out.
    pub fn suggested_rules(&self) -> Vec<Rule> {
        self.suggested
            .iter()
            .map(|(_, rule)| rule.clone())
            .collect()
    }

    /// Every action offered under a title, with who owns it.
    pub fn commands(&self) -> &[(PluginId, ActionName, String)] {
        &self.commands
    }

    /// What an action is called, where it has a title.
    pub fn title_of(&self, action: &ActionName) -> Option<&str> {
        self.commands
            .iter()
            .find(|(_, name, _)| name == action)
            .map(|(_, _, title)| title.as_str())
    }

    /// The id for a name, if anything answers to it right now.
    ///
    /// `None` for a name nothing is registered under — a chord bound to a
    /// plugin that is not installed, or one spelled wrong — which is the case
    /// the keymap is built around: it does nothing, and it does not stop the
    /// chords around it working.
    pub fn action(&self, name: &ActionName) -> Option<ActionId> {
        if !self.actions.contains(name) {
            return None;
        }
        self.action_names
            .iter()
            .position(|known| known == name)
            .map(ActionId)
    }

    /// What an id is called.
    pub fn action_name(&self, id: ActionId) -> Option<&ActionName> {
        self.action_names.get(id.0)
    }

    /// The font families, for a plugin building something that draws text
    /// before the workspace exists.
    pub fn fonts(&self) -> Fonts {
        self.fonts
    }

    /// What this plugin has been allowed to do.
    ///
    /// Empty for one nobody has answered for, which is what every plugin
    /// installs as: a manifest asking for something is not a person allowing
    /// it. What the keys mean is
    /// [`Capability::keys`](crook_plugin_api::Capability::keys).
    pub fn granted(&self, plugin: &PluginId) -> &[String] {
        self.grants
            .get(plugin.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The slot of that name, if one has been declared.
    ///
    /// For a plugin that names a slot with a *string* — which every plugin
    /// outside the binary does. A native plugin names a [`SlotId`] and cannot
    /// name one that does not exist; the answer to a name nothing declares is
    /// "no", and the contribution is refused rather than filed under a slot
    /// invented on the spot.
    pub fn slot_named(&self, name: &str) -> Option<SlotId> {
        self.slots
            .declared()
            .into_iter()
            .find(|slot| slot.as_str() == name)
    }

    /// The slots, for the renderers that draw them.
    pub fn slots(&self) -> &Slots<UiContribution> {
        &self.slots
    }

    /// The row slot of that name, if this build has one.
    ///
    /// Looked up separately from [`slot_named`](Self::slot_named), so that a
    /// plugin contributing to `tab.row.mark` reaches the registry that can ask
    /// it about a row and a plugin contributing to `header.right` reaches the
    /// one that cannot.
    pub fn row_slot_named(&self, name: &str) -> Option<SlotId> {
        self.rows
            .declared()
            .into_iter()
            .find(|slot| slot.as_str() == name)
    }

    /// The row slots, for the panel that draws them.
    pub(crate) fn rows(&self) -> &Slots<RowContribution> {
        &self.rows
    }

    /// The actions, for whatever dispatches one.
    pub fn actions(&self) -> &Actions<ActionHandler> {
        &self.actions
    }

    /// Every plugin that built, in the order it did.
    pub fn loaded(&self) -> &[&'static Manifest] {
        &self.loaded
    }

    /// Every plugin that did not, and why.
    pub fn refused(&self) -> &[(PluginId, String)] {
        &self.refused
    }

    /// Everything the registries have to complain about.
    pub fn audit(&self) -> Vec<Complaint> {
        let mut complaints = self.slots.audit();
        complaints.extend(self.rows.audit());
        complaints.extend(self.actions.audit());
        complaints
    }

    /// Every plugin the binary carries, whether or not it is loaded.
    ///
    /// What the Plugins page lists: a switched-off plugin has to be visible,
    /// or there is no way to switch it back on.
    pub fn available(&self) -> &[&'static Manifest] {
        &self.carried
    }

    /// Whether this plugin is loaded right now.
    pub fn is_loaded(&self, plugin: &PluginId) -> bool {
        self.loaded.iter().any(|manifest| manifest.id == *plugin)
    }

    /// Records what a plugin is allowed to do, for the next time it builds.
    ///
    /// Only for the next time: a plugin reads its grant once, while building,
    /// and holds what it read. So this is half of the answer and
    /// [`Self::unload`] followed by [`Self::enable`] is the other half — which
    /// is exactly what switching a plugin off and on again does, and is why a
    /// new permission needs no restart. Nothing of the plugin's previous life
    /// survives that, which is what makes it correct rather than merely
    /// convenient.
    pub fn set_granted(&mut self, plugin: &PluginId, keys: Vec<String>) {
        if keys.is_empty() {
            self.grants.remove(plugin.as_str());
        } else {
            self.grants.insert(plugin.as_str().to_owned(), keys);
        }
    }

    /// Builds a plugin that is not loaded, and does nothing to one that is.
    ///
    /// The other half of [`Self::unload`], and the reason the plugin objects
    /// are kept: switching one back on runs its `build` again, which makes its
    /// entities and its registrations afresh. It is not a resumption — nothing
    /// of the previous life survives — and that is what makes it correct: a
    /// plugin that was off saw nothing happen while it was off, so there is no
    /// state it could have been holding.
    pub fn enable(&mut self, plugin: &PluginId, ctx: &mut ViewContext<Workspace>) {
        if self.is_loaded(plugin) {
            return;
        }
        // Taken out and put back, because `build_one` needs the host and the
        // plugin at the same time and both live here.
        let mut plugins = std::mem::take(&mut self.plugins);
        if let Some(found) = plugins
            .iter_mut()
            .find(|carried| carried.manifest().id == *plugin)
        {
            self.build_one(found.as_mut(), ctx);
            self.ready_one(found.as_mut(), ctx);
        }
        self.plugins = plugins;
    }

    /// Takes back everything one plugin registered.
    ///
    /// Which is the whole of what disabling a plugin does: the guards drop, the
    /// registries forget, and the next frame draws the window that was there
    /// before the plugin loaded.
    pub fn unload(&mut self, plugin: &PluginId) {
        self.kept.retain(|(by, _)| by != plugin);
        self.commands.retain(|(by, _, _)| by != plugin);
        self.suggested.retain(|(by, _)| by != plugin);
        self.surfaces.retain(|(by, _, _)| by != plugin);
        // A field goes out with its plugin, and it has to go out *emptied*:
        // the registry is the only thing holding it, but the workspace may
        // have asked it a moment ago whether it had the keyboard, and a field
        // nothing can draw must not answer yes to that again.
        for (by, _, field, _) in &self.fields {
            if by == plugin {
                field.set_has_keys(false);
            }
        }
        self.fields.retain(|(by, _, _, _)| by != plugin);
        self.panels.retain(|(by, _, _)| by != plugin);
        self.loaded.retain(|manifest| &manifest.id != plugin);
    }

    /// Runs one plugin's `ready`, filing what it registers under its name.
    fn ready_one(&mut self, plugin: &mut dyn Plugin, ctx: &mut ViewContext<Workspace>) {
        let manifest = plugin.manifest();
        if !self.is_loaded(&manifest.id) {
            return;
        }
        self.building = Some(manifest.id.clone());
        let outcome = plugin.ready(self, ctx);
        self.building = None;

        if let Err(problem) = outcome {
            // The same rule `build_one` follows, and it has to be: a plugin
            // that gave up halfway through its second step has left the same
            // half-built surface as one that gave up in its first.
            self.unload(&manifest.id);
            log::warn!(
                "the plugin {} did not finish loading: {problem}",
                manifest.id
            );
            self.refused.push((manifest.id.clone(), problem));
        }
    }

    /// Builds one plugin, filing everything it registers under its own name.
    fn build_one(&mut self, plugin: &mut dyn Plugin, ctx: &mut ViewContext<Workspace>) {
        let manifest = plugin.manifest();
        self.building = Some(manifest.id.clone());
        let outcome = plugin.build(self, ctx);
        self.building = None;

        match outcome {
            Ok(()) => self.loaded.push(manifest),
            Err(problem) => {
                // Everything it managed to register before it gave up goes
                // back out, so a half-built plugin never leaves half a
                // contribution on screen.
                self.unload(&manifest.id);
                log::warn!("the plugin {} did not load: {problem}", manifest.id);
                self.refused.push((manifest.id.clone(), problem));
            }
        }
    }
}

/// Why a plugin did not load.
pub type BuildError = String;

/// One plugin.
///
/// Deliberately two methods. The manifest is *data* — the store reads it to
/// list a plugin and the settings page reads it to describe one, neither of
/// which should have to run any of it — and `build` is everything else.
pub trait Plugin {
    /// What this plugin says about itself.
    fn manifest(&self) -> &'static Manifest;

    /// Registers everything it contributes, and builds whatever it owns.
    ///
    /// Called once, from inside `Workspace::new` and therefore *before* there
    /// is a workspace — which is why the context is the workspace's own rather
    /// than something narrower: a plugin that owns a model or a view makes it
    /// here, keeps the handle, and captures it in the contributions that draw
    /// it. Nothing is reachable through `ctx` that the workspace itself could
    /// be asked for, because there is not one yet.
    ///
    /// Returning an error is how a plugin declines to load — the host takes
    /// back whatever it registered first and says so by name; it is never how
    /// a plugin reports something a person should act on, which is what the log
    /// and the plugins page are for.
    fn build(
        &mut self,
        host: &mut Host,
        ctx: &mut ViewContext<Workspace>,
    ) -> Result<(), BuildError>;

    /// Registers whatever depends on *every other* plugin having built.
    ///
    /// Run after the last `build`, in load order, on every plugin that loaded.
    /// Bevy calls the same step `finish`, and the reason both have one is the
    /// same: a plugin that wants to know what is registered cannot ask during
    /// `build`, because half of it has not been registered yet. The Plugins
    /// page's switches are the case here — one per plugin the binary carries,
    /// and it cannot know how many that is until they have all arrived.
    ///
    /// Nothing about it is different from `build` otherwise: what it registers
    /// is filed under the same plugin and taken back by the same `unload`.
    fn ready(
        &mut self,
        _host: &mut Host,
        _ctx: &mut ViewContext<Workspace>,
    ) -> Result<(), BuildError> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "plugin_tests.rs"]
mod tests;

/// Builds every plugin in `plugins`, in order, into a fresh host.
///
/// Order is load order, and load order is what settles two entries that asked
/// for the same place in a slot. It is a list in the source rather than
/// anything resolved at runtime: a dependency here is a `use`, and a plugin
/// that will not compile is a build failure with a name on it rather than a
/// window that comes up silently missing a feature.
pub fn load(
    plugins: Vec<Box<dyn Plugin>>,
    disabled: &[String],
    grants: BTreeMap<String, Vec<String>>,
    fonts: Fonts,
    ctx: &mut ViewContext<Workspace>,
) -> Host {
    let mut host = Host::new(fonts, grants);
    let mut plugins = plugins;
    host.carried = plugins.iter().map(|plugin| plugin.manifest()).collect();
    for plugin in &mut plugins {
        // A plugin somebody switched off is carried and not built, which is
        // the whole of what "off" means: its `build` never runs, so it
        // registers nothing and makes nothing. The bytes are in the binary
        // either way — see `docs/plugins.md` on why there is no "carried but
        // switched off" tier.
        if disabled
            .iter()
            .any(|off| *off == plugin.manifest().id.as_str())
        {
            continue;
        }
        host.build_one(plugin.as_mut(), ctx);
    }
    // Second pass, once everything that is going to register has. See
    // `Plugin::ready`.
    for plugin in &mut plugins {
        host.ready_one(plugin.as_mut(), ctx);
    }
    host.plugins = plugins;

    for complaint in host.audit() {
        log::warn!("{complaint}");
    }

    host
}
