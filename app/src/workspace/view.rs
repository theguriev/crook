//! [`Workspace`]: the state behind the window, and the one place it changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// How often the themes folder is re-read while the Themes panel is open.
///
/// Fast enough that saving a theme file and looking at the window feels
/// immediate, slow enough that a folder of fifty themes is a directory walk
/// somebody would have to go looking for in a profiler. It costs nothing at
/// all while the panel is closed — see [`Workspace::watch_themes`].
pub(super) const THEMES_POLL: Duration = Duration::from_millis(750);

use crook_terminal::{Rows, Snapshot};
use crookui_core::elements::MouseStateHandle;
use crookui_core::event::Keystroke;
use crookui_core::fonts::FamilyId;
use crookui_core::geometry::RectF;
use crookui_core::prelude::*;

use crate::clipboard::Clipboard;
use crate::editor::Selection;
use crate::git::GitFacts;
use crate::git_model::GitModel;
use crate::input_keys::{self, Binding, Platform};
use crate::keymap::{Bound, Keymap};
use crate::pane_blocks::PaneBlocks;
use crate::pane_link::PaneLink;
use crate::pane_selection::PaneSelection;
use crate::pane_split::{DividerDrag, PaneExtent};
use crate::pane_surface;
use crate::platform_insets::{ControlLayout, LayoutInsets, WindowChrome};
use crate::plugin::{ActionId, Host, PageId, PluginId, SectionId};
use crate::plugins::settings::SETTINGS_SECTION;
use crate::selection::{Blocks, Cells};
use crate::settings::{
    DEFAULT_FONT_SIZE, Density, FONT_SIZE_STEP, GeneralOptions, Granularity, Settings, TabOptions,
};
use crate::tab::{
    AgentSession, AgentStatus, Direction, Pane, PaneId, Tab, TabAction, TabEffect, TabId, TabStrip,
};
use crate::terminal_font::CellFont;
use crate::terminal_model::{BlockHistory, TerminalHandle, TerminalModel, TerminalUpdate};
use crate::text_input::{CARET_PHASE, TextInput};
use crate::theme::creator::Draft;
use crate::theme::{Available, theme};
use crate::usage_model::UsageModel;
use crate::window_controls::WindowHandle;
use crate::{Channel, WINDOW_CHROME};

use super::action::{
    OptionsAction, SearchAction, SettingsAction, ThemeAction, WindowAction, WorkspaceAction,
    WorktreeAction,
};
use super::settings_page::SettingsState;
use super::tab_menu::{Contents, Mode as WorktreeMode, TabMenuState};
use super::tabs_panel::geometry::RowGeometry;
use super::tabs_panel::search::SearchState;
use super::theme_panel::{Mode, ThemePanelState};
use super::{body, header_toolbar, tabs_panel};

/// The two font families the interface is set in, resolved once at startup.
///
/// Passed down rather than looked up, because resolving a family is a search
/// through the system font database and a view renders many times a second.
#[derive(Copy, Clone)]
pub struct Fonts {
    /// Labels, buttons, the chip.
    pub ui: FamilyId,
    /// Anything that stands in for terminal output.
    pub monospace: FamilyId,
}

/// What the workspace calls when the last tab is closed.
///
/// A callback rather than a window handle, so nothing in this module names the
/// windowing layer: the headless snapshot path passes a no-op and gets the
/// same code path a real window gets.
pub type QuitRequest = Rc<dyn Fn()>;

/// What the mouse is doing to one pane, kept across renders.
///
/// The element tree is thrown away every time the view re-renders, so hover
/// and press state cannot live in it. It lives here, keyed by identity, and is
/// handed back to the elements each frame.
///
/// Keyed by [`PaneId`] and not [`TabId`], because in `Panes` view one tab
/// draws several chips: sharing one entry between them would light them all up
/// together, and the close-button guard in the chip's click handler would
/// swallow clicks meant for a sibling.
pub(super) struct PaneInteraction {
    /// The pane's chip in the bar.
    pub(super) chip: MouseStateHandle,
    /// That chip's close button.
    pub(super) close: MouseStateHandle,
    /// The pane's panel in the body.
    pub(super) body: MouseStateHandle,
    /// The selection gesture in the pane's output. See [`PaneSelection`].
    pub(super) selection: PaneSelection,
    /// The link under the pointer in the pane's output. See [`PaneLink`].
    pub(super) links: PaneLink,
    /// How many pixels the pane last measured along the split's axis. See
    /// [`PaneExtent`].
    pub(super) extent: PaneExtent,
    /// Where the pane's block list is scrolled to and what the pointer is
    /// over. See [`PaneBlocks`].
    pub(super) blocks: PaneBlocks,
}

/// What the mouse is doing to one tab's chrome in the panel.
///
/// Tab-level rather than pane-level: in `Panes` granularity one tab draws
/// several rows, and the container lifts while the pointer is over any of
/// them. One entry for several rows, not one per row.
#[derive(Default)]
pub(super) struct TabInteraction {
    /// The box around all of the tab's rows.
    pub(super) container: MouseStateHandle,
    /// The tab's name above them, which only a split tab draws.
    pub(super) header: MouseStateHandle,
}

/// The options menu: whether it is up, and what the mouse is doing to each of
/// its controls.
///
/// One handle per control and never a shared one. Warp learned this the hard
/// way: it used to render the popup in two element trees, and because both
/// copies read the same handle, the first copy's mouse-up took the click count
/// and left `None` for the second — every click on every item silently
/// dropped. Crook's `Hoverable` takes the count the same way, so the bug ports
/// over exactly. One tree, one handle per row.
#[derive(Default)]
pub(super) struct MenuState {
    /// Whether the popup is up.
    pub(super) open: bool,
    /// The gear that opens it.
    pub(super) gear: MouseStateHandle,
    /// "View as: Panes".
    pub(super) panes: MouseStateHandle,
    /// "View as: Tabs".
    pub(super) tabs: MouseStateHandle,
    /// "Density: Compact".
    pub(super) compact: MouseStateHandle,
    /// "Density: Expanded".
    pub(super) expanded: MouseStateHandle,
    /// "Pane title as: Command / Conversation".
    pub(super) primary_command: MouseStateHandle,
    /// "Pane title as: Working Directory".
    pub(super) primary_directory: MouseStateHandle,
    /// "Pane title as: Branch".
    pub(super) primary_branch: MouseStateHandle,
    /// The first "Additional metadata" row, whichever value it holds.
    pub(super) subtitle_first: MouseStateHandle,
    /// The second "Additional metadata" row.
    pub(super) subtitle_second: MouseStateHandle,
    /// "Show: PR link".
    pub(super) pr_link: MouseStateHandle,
    /// The info dot on "Show: PR link".
    pub(super) pr_link_info: MouseStateHandle,
    /// "Show: Diff stats".
    pub(super) diff_stats: MouseStateHandle,
    /// "Show details on hover".
    pub(super) details_on_hover: MouseStateHandle,
    /// The row that opens the settings page.
    pub(super) settings: MouseStateHandle,
}

impl MenuState {
    /// Drops every hover and press the popup was holding.
    ///
    /// Called when something other than a click on the gear takes the menu
    /// down — today, the settings page opening over it. Every row is about to
    /// stop existing without seeing a hover-out, and the next time the menu
    /// opens the row the pointer happened to be on would come back lit.
    ///
    /// Written out one handle at a time, like the struct itself, because the
    /// fields are what stops two controls from sharing one state; a loop here
    /// would need a collection, and a collection is the thing this type exists
    /// not to be.
    fn forget_hover_state(&self) {
        for state in [
            &self.gear,
            &self.panes,
            &self.tabs,
            &self.compact,
            &self.expanded,
            &self.primary_command,
            &self.primary_directory,
            &self.primary_branch,
            &self.subtitle_first,
            &self.subtitle_second,
            &self.pr_link,
            &self.pr_link_info,
            &self.diff_stats,
            &self.details_on_hover,
            &self.settings,
        ] {
            state.lock().reset_interaction_state();
        }
    }
}

/// Which options the command line put on screen without adopting them.
///
/// An override is a way to look at a frame, so it must never become what the
/// next launch does — and every menu click writes the *whole* options snapshot
/// back, so "never saved" cannot be arranged by simply not saving it once.
/// [`Workspace::persisted`] is what keeps the promise, and this is what it
/// reads: one flag per option a flag can set, cleared the moment a person
/// chooses that option for themselves.
///
/// One struct rather than three booleans on [`Workspace`], because the three
/// obey the same three rules and the second and third were added by copying
/// the first. A fourth overridable option adds a field here and nothing else.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct Overridden {
    density: bool,
    granularity: bool,
}

impl Overridden {
    /// Only the density.
    const DENSITY: Self = Self {
        density: true,
        granularity: false,
    };
    /// Only the granularity.
    const GRANULARITY: Self = Self {
        density: false,
        granularity: true,
    };

    /// Whether nothing at all is overridden.
    fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// Marks every flag `also` marks, leaving the rest alone.
    fn mark(&mut self, also: Self) {
        self.density |= also.density;
        self.granularity |= also.granularity;
    }

    /// Clears every flag `chosen` marks, and says whether that changed
    /// anything.
    ///
    /// The answer is what tells [`Workspace::apply_option`] that a click it is
    /// about to discard as a no-op was in fact a choice worth saving.
    fn clear(&mut self, chosen: Self) -> bool {
        let before = *self;
        self.density &= !chosen.density;
        self.granularity &= !chosen.granularity;
        *self != before
    }
}

/// What makes the last write asked for the one the file ends up holding.
///
/// **A number on its own is not enough, and that is the whole of this type.**
/// Every write is a background task, and a task that has been overtaken has
/// nothing to do — so each carries the number it was asked at and looks at the
/// last number asked for before writing. That much was here before. What it
/// could not do is *order* the two writes it does allow: a task that looked
/// while it was still the latest is free to be descheduled between the look
/// and its `rename`, and the later task, which looked afterwards and was also
/// the latest when it did, can then land first and be overwritten by the
/// earlier snapshot. The file settles on the state before the last click and
/// stays there — which is exactly what a person sees as "the setting did not
/// stick", and what a test that reads the file sees as the value it asked for
/// never arriving.
///
/// So the look and the write are one step, under [`SaveOrder::writing`]. A
/// task only writes while holding that lock and only while it is still the
/// last one asked for, and both facts are true at the same instant. Any task
/// asked for earlier than one that has already written finds a larger number
/// when its turn at the lock comes and does nothing; any task asked for later
/// waits at the lock until the write in progress is finished and then
/// overwrites it. Either way the bytes on disk are the last snapshot asked
/// for.
///
/// The lock is per file, not per process: one of these belongs to the settings
/// and another to the session, because they are two files and neither has to
/// wait on the other.
#[derive(Debug, Default)]
struct SaveOrder {
    /// How many writes have been asked for.
    ///
    /// Only ever increases, which is what lets a task compare its own number
    /// with it and know whether it has been overtaken.
    asked_for: AtomicU64,
    /// Held across the decision *and* the write.
    ///
    /// A `()` because it guards an order rather than a value: the state being
    /// protected is the file, and the file is not something this type can
    /// hold.
    writing: Mutex<()>,
}

impl SaveOrder {
    /// Records that a write has been asked for and hands back its number.
    ///
    /// Called on the thread the change was made on, *before* the task is
    /// spawned, so that a write asked for later is already counted by the time
    /// an earlier task reaches the lock.
    fn ask(&self) -> u64 {
        self.asked_for.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Runs `write` unless a later write has been asked for since, with every
    /// other write to the same file held off until it returns.
    ///
    /// Blocking, on both counts: the lock is waited for and `write` is a file
    /// being written. This belongs on the background pool.
    ///
    /// A poisoned lock is taken anyway. It guards no data — see
    /// [`SaveOrder::writing`] — so the only thing a panicking writer leaves
    /// behind is a file that may not have been written, and refusing every
    /// later save because of it would turn one lost write into all of them.
    fn write_if_last(&self, asked_at: u64, write: impl FnOnce()) {
        let _ordered = self.writing.lock().unwrap_or_else(PoisonError::into_inner);
        if self.asked_for.load(Ordering::Relaxed) != asked_at {
            return;
        }
        write();
    }
}

/// What one window is opened with, as one argument.
///
/// Three values that arrive together, are read once each, and travel from
/// whoever built them to [`Workspace::new`] with nothing in between looking at
/// them. Passing them separately put that function one argument over clippy's
/// limit, and grouping them says something true: they are the opening, not
/// three unrelated parameters. `Launch` in `crate::lib` is the same answer to
/// the same question one layer up.
pub struct Opening {
    /// The options this window starts with.
    pub settings: Settings,
    /// Which build is running, for the About page.
    pub channel: Channel,
    /// Every plugin this window carries: the ones in the box, then the ones a
    /// person installed.
    ///
    /// A list rather than something read here, because what is in it is not
    /// one thing — a list in the source and a directory somebody has to have
    /// gone and looked in — and a window that read a directory would be a
    /// window no test could give a plugin to.
    pub plugins: Vec<Box<dyn crate::plugin::Plugin>>,
}

/// The window's root view.
pub struct Workspace {
    tabs: TabStrip,
    /// One mouse state per section button, made the first time it is drawn.
    section_buttons: std::cell::RefCell<HashMap<String, MouseStateHandle>>,
    /// Every text field a section brought with it, in the order they were
    /// first drawn: which section it belongs to, its own name, and the editor
    /// behind it.
    ///
    /// Held here rather than by the sections, because `sync_input_keys` has to
    /// be able to take the keyboard *away* from every one of them and it can
    /// only reach what the workspace holds.
    fields: std::cell::RefCell<Vec<(String, String, TextInput)>>,
    /// Which field of each section was pressed last.
    ///
    /// Per section, because a section is one screen: the field a person was
    /// typing into in the settings is not a claim about the plugins. A section
    /// with nothing recorded gives the keyboard to the first field it drew,
    /// which for every section Crook ships is its only one.
    focused_field: std::cell::RefCell<HashMap<String, String>>,
    /// Which section of the sidebar is showing, by its `owner/entry` key, or
    /// `None` for the tabs.
    ///
    /// A key rather than an id, for the reason the settings page's is: the
    /// sections come from a slot, so a key whose section has gone — the plugin
    /// was switched off while it was showing — reads as `None` and the window
    /// goes back to the tabs.
    section: Option<String>,
    fonts: Fonts,
    /// The faces and the cell every pane's grid is drawn with, resolved once at
    /// startup because measuring one is a search through the font database.
    cell_font: CellFont,
    usage: ModelHandle<UsageModel>,
    git: ModelHandle<GitModel>,
    /// The shells behind the panes.
    terminals: ModelHandle<TerminalModel>,
    /// The command line being composed in each pane.
    ///
    /// One per pane and never one per tab: a split gives the new pane a field
    /// of its own, with its own undo stack and its own history, which is what
    /// makes two panes of the same tab two places to work rather than one.
    inputs: HashMap<PaneId, TextInput>,
    /// The system clipboard every field copies to and pastes from. One for the
    /// window: see [`Clipboard`].
    clipboard: Clipboard,

    /// The divider drag in progress, shared by every divider in the window so
    /// that only one can be dragged at a time. See
    /// [`DividerDrag`](crate::pane_split::DividerDrag).
    divider_drag: DividerDrag,

    /// Makes the last session save the one that lands. See
    /// [`Self::save_session`] and [`SaveOrder`].
    session_saves: Arc<SaveOrder>,

    /// How big the window was when it was last laid out, in logical pixels.
    ///
    /// Written by the delegate, which is the only thing told: the size is an
    /// argument to `build_scene` and nothing in the view tree ever sees it. A
    /// cell rather than a field so that recording it costs no `update` and
    /// therefore no frame.
    window_size: Rc<std::cell::Cell<Vector2F>>,

    /// The bindings a person wrote down, consulted before Crook's own.
    ///
    /// Read once, at startup, like the theme and the font family: a keymap
    /// re-read mid-session would change what a key does between the press and
    /// the release. See [`crate::keymap`].
    keymap: Keymap,

    /// Whether the desktop is set to dark, as of the last thing the window
    /// said about it.
    ///
    /// Dark until told otherwise, which is what a terminal has always been and
    /// what a desktop that will not answer is taken as. It is one bit rather
    /// than a theme because the *resolution* is
    /// [`Settings::theme_for`](crate::settings::Settings::theme_for), and
    /// keeping a resolved theme here as well would be a second copy of an
    /// answer that already has one.
    system_is_dark: bool,
    interactions: HashMap<PaneId, PaneInteraction>,
    /// What the mouse is doing to each tab's chrome in the panel.
    ///
    /// Keyed by [`TabId`] and separate from [`Self::interactions`] because it
    /// is a tab-level affordance: in `Panes` granularity the container lifts
    /// while the pointer is over *any* of the tab's rows, which is one piece
    /// of state for several rows rather than one per row.
    tab_chrome: HashMap<TabId, TabInteraction>,
    settings: Settings,
    /// Which build this is, for the settings page's About section. Carried
    /// rather than looked up: nothing else in the view layer knows which
    /// binary started it, and the alternative is a second global.
    channel: Channel,
    /// The options, kept beside [`Self::settings`] rather than read out of it
    /// on every access. A renderer reads this dozens of times per frame and
    /// wants a `Copy` snapshot, not a borrow of the thing a save is cloning.
    options: TabOptions,
    /// Which of [`Self::options`] came from the command line rather than from
    /// the file. See [`Overridden`].
    overridden: Overridden,
    menu: MenuState,
    /// The settings page: whether it is up, which page it is on, and what the
    /// mouse is doing to each of its controls.
    page: SettingsState,
    /// Every plugin that loaded, and everything they registered.
    ///
    /// Built before the workspace, because a contribution is a closure over
    /// `&Workspace` rather than anything captured — so the registries can be
    /// filled without a workspace to fill them from.
    host: Host,
    /// The menu a tab opens, which is about worktrees.
    tab_menu: TabMenuState,
    /// The Themes panel, which is the other surface that lists themes.
    panel: ThemePanelState,
    /// Every theme that can be chosen, as of the last time a surface that
    /// lists them was opened.
    ///
    /// One list for both surfaces. Reading it walks a directory, so it is not
    /// read on the render path; refreshing it on the gestures that can precede
    /// choosing a theme is the same trade the settings page already made, and
    /// keeping *one* of it is what stops the panel and the page from
    /// disagreeing about what exists.
    themes: Vec<Available>,
    /// The theme that was in force when the creator opened, to put back if it
    /// is cancelled.
    theme_before_draft: Option<String>,
    /// Where themes are read from and written to.
    ///
    /// Carried rather than asked for each time, because a test must be able to
    /// point it somewhere else: a list read from the real folder would assert
    /// something about the machine running the test, and a creator test would
    /// write a theme into the folder of whoever ran it. `Settings` has the
    /// same seam for the same reason.
    themes_directory: Option<PathBuf>,
    /// Where a worktree Crook makes is checked out.
    ///
    /// Resolved once, at startup, for the reason the themes folder is: it is a
    /// property of the machine rather than of a frame. `None` on a machine
    /// with no data directory, where the creator has nowhere to put one and
    /// says so rather than guessing.
    worktrees_directory: Option<PathBuf>,
    /// Makes the last settings save the one the file ends up holding.
    ///
    /// Browsing themes with the arrow keys asks for one save per keystroke,
    /// which is exactly the case `save_settings` said it would need this for.
    ///
    /// An `Arc<SaveOrder>` rather than a `Cell`, because the deciding and the
    /// writing both happen on the worker. See [`SaveOrder`].
    saves: Arc<SaveOrder>,
    /// How far the tabs panel's list has been scrolled.
    ///
    /// On the workspace rather than inside the panel module for the reason
    /// every mouse state is: the element tree is rebuilt on every render, and
    /// a scroll offset that lived in it would snap back to the top on the
    /// frame the scroll itself caused.
    panel_scroll: ScrollStateHandle,
    /// Where the panel drew each of its rows on the last frame, so that a
    /// selection made with the keyboard can be scrolled to. See
    /// [`RowGeometry`](super::tabs_panel::geometry::RowGeometry).
    panel_rows: RowGeometry,
    /// The box above the list: what has been typed into it, and whether the
    /// keyboard is its.
    ///
    /// Beside the panel's scroll offset and its row geometry rather than
    /// inside the panel module, for the reason both of those are: the element
    /// tree is rebuilt on every render, so anything a keystroke changes has to
    /// outlive the element that saw it.
    panel_search: SearchState,
    /// The row the pointer is on, if the detail card is armed.
    hovered_row: Option<PaneId>,
    /// The home directory, resolved once.
    ///
    /// Every row abbreviates its working directory against it, and
    /// `std::env::home_dir` is a `getpwuid_r` fallback on Unix and a shell-API
    /// call on Windows — a syscall per row per frame if it is asked on the
    /// render path. It does not change while the process runs.
    home: Option<PathBuf>,
    new_tab: MouseStateHandle,
    quit: QuitRequest,
    /// The window this is drawn in, for the header to move and maximise.
    window: WindowHandle,
    /// Where this build's platform puts a window's controls.
    ///
    /// A field rather than [`ControlLayout::host`] read at the point of use,
    /// so `--controls` can lay this window out as another platform's. What is
    /// left of that difference is macOS's reservation for its traffic lights,
    /// which is invisible on the other two platforms and can only be looked at
    /// by asking for it.
    control_layout: ControlLayout,
}

impl Workspace {
    /// Builds the workspace, its git model, every plugin in the box, and one
    /// tab to start in.
    pub fn new(
        fonts: Fonts,
        cell_font: CellFont,
        opening: Opening,
        quit: QuitRequest,
        window: WindowHandle,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let Opening {
            settings,
            channel,
            plugins,
        } = opening;
        let usage = UsageModel::handle(ctx);

        let git = ctx.add_model(GitModel::new);
        // A branch that arrives, or a diff count that changes, repaints the
        // strip that prints it — and only when it actually changed, because
        // the model checks before it notifies.
        ctx.observe(&git, |_, _, ctx| ctx.notify());

        let terminals = ctx.add_model(TerminalModel::new);
        // Two channels, and they carry different things. The observation is
        // "a grid changed, draw it again", which the model raises at most once
        // per pane per frame interval. The subscription is the handful of
        // things a shell does that the *strip* has to act on: rename itself,
        // move, or finish.
        ctx.observe(&terminals, |_, _, ctx| ctx.notify());
        ctx.subscribe_to_model(&terminals, |workspace, _, update, ctx| {
            workspace.apply_terminal_update(update, ctx);
        });
        // Before any shell can be opened, because a shell reads its startup
        // files once and the model's own default would otherwise decide the
        // first pane of every session for a person who turned this off.
        let login_shell = settings.general().login_shell;
        terminals.update(ctx, |model, _| model.set_shell_login(login_shell));

        // Last, and before anything is placed on the workspace: every plugin
        // builds here, and one that owns a model or a view makes it with this
        // context. Nothing it registers can reach the workspace yet — the
        // contributions are closures, and they are not called until there is a
        // frame to draw.
        //
        // The list arrives rather than being read here, because what is in it
        // is not one thing: the plugins in the box are a list in the source,
        // and the ones a person installed are a directory somebody has to have
        // gone and looked in. A window that read a directory would be a window
        // no test could give a plugin to.
        let host = crate::plugin::load(plugins, settings.disabled_plugins(), fonts, ctx);

        let options = settings.tab_options();
        let settings_path = settings.path().map(Path::to_owned);
        let mut workspace = Self {
            tabs: TabStrip::new(),
            section: None,
            section_buttons: std::cell::RefCell::new(HashMap::new()),
            fields: std::cell::RefCell::new(Vec::new()),
            focused_field: std::cell::RefCell::new(HashMap::new()),
            fonts,
            cell_font,
            usage,
            git,
            terminals,
            inputs: HashMap::new(),
            clipboard: Clipboard::new(),
            divider_drag: DividerDrag::new(),
            session_saves: Arc::default(),
            // Blocking, and deliberately: one small file, read once, on the
            // same startup path the settings are read on. A run with no
            // settings file to write is an ephemeral one — a test, the
            // headless snapshot — and must not read the keymap of whoever is
            // running it either.
            keymap: if settings_path.is_some() {
                Keymap::for_user()
            } else {
                Keymap::new()
            },
            window_size: Rc::new(std::cell::Cell::new(Vector2F::zero())),
            system_is_dark: true,
            interactions: HashMap::new(),
            tab_chrome: HashMap::new(),
            settings,
            channel,
            options,
            overridden: Overridden::default(),
            menu: MenuState::default(),
            page: SettingsState::default(),
            host,
            tab_menu: TabMenuState::default(),
            panel: ThemePanelState::default(),
            themes: crate::theme::available(),
            theme_before_draft: None,
            themes_directory: crate::theme::user_themes_directory(),
            worktrees_directory: worktree_store(),
            saves: Arc::default(),
            panel_scroll: ScrollStateHandle::default(),
            panel_rows: RowGeometry::new(),
            panel_search: SearchState::default(),
            hovered_row: None,
            home: std::env::home_dir(),
            new_tab: MouseStateHandle::default(),
            quit,
            window,
            control_layout: ControlLayout::host(),
        };
        workspace.sync_interactions();
        workspace.sync_git(ctx);
        workspace
    }

    /// The tabs, read-only. Changing them goes through [`Self::apply`].
    pub fn tabs(&self) -> &TabStrip {
        &self.tabs
    }

    /// The families the interface is set in.
    pub fn fonts(&self) -> Fonts {
        self.fonts
    }

    /// The faces and the cell a terminal grid is drawn with.
    pub fn cell_font(&self) -> &CellFont {
        &self.cell_font
    }

    /// Everything the options menu writes, as a snapshot.
    ///
    /// The one thing every renderer reads. Nothing derived from it is cached
    /// anywhere, which is what makes a click on the menu visible in the very
    /// next frame.
    pub fn options(&self) -> TabOptions {
        self.options
    }

    /// The settings, including where they are saved.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Everything the settings page writes that is not a tab option.
    ///
    /// Read straight out of [`Self::settings`] rather than mirrored beside it,
    /// the way `options` is. The mirror exists because every renderer reads
    /// the tab options dozens of times a frame and wants a `Copy` snapshot
    /// rather than a borrow of the thing a save is cloning; one switch on one
    /// page does not earn a second copy to keep in step.
    pub fn general(&self) -> GeneralOptions {
        self.settings.general()
    }

    /// The name of the theme in force.
    ///
    /// The palette on screen rather than the name in the settings file, when
    /// the two differ — which they do under `--theme`, an override that is
    /// deliberately never saved. A panel that outlined the *saved* theme while
    /// a different one was on screen would be telling a person their own
    /// window is wrong.
    pub fn theme_name(&self) -> &str {
        let showing = theme();
        self.themes
            .iter()
            .find(|available| available.theme == showing)
            .map(|available| available.name.as_str())
            .unwrap_or_else(|| self.settings.theme())
    }

    /// Puts a theme on screen, remembers it, and repaints everything.
    ///
    /// Everything: the chrome reads [`theme`](crate::theme::theme) on the next
    /// render, and every shell already running is handed the new palette,
    /// because a grid resolves its colours through the one it was started
    /// with. A theme that stopped at the edge of the terminal would be the
    /// least useful half of a theme.
    ///
    /// A name this machine has no theme for is ignored with a warning rather
    /// than falling back: the file may be one directory away from being put
    /// back, and silently adopting a different theme would lose the choice.
    pub fn set_theme(&mut self, name: &str, ctx: &mut ViewContext<Self>) {
        let known = self
            .themes
            .iter()
            .find(|available| available.name == name)
            .map(|available| available.theme);
        let Some(palette) = known.or_else(|| crate::theme::named(name)) else {
            log::warn!("no theme called {name:?} on this machine; keeping the current one");
            return;
        };

        crate::theme::set_theme(palette);
        self.settings.set_theme(name);
        // And into whichever half of the desktop pair is in force, so that a
        // theme chosen while following the system is the one it comes back to
        // the next time the desktop is in this state. Writing both halves
        // would be worse than writing neither: it would silently replace the
        // theme somebody had chosen for the *other* half.
        self.settings.set_system_theme(self.system_is_dark, name);
        self.save_settings(ctx);
        self.sync_palette(ctx);
        ctx.notify();
    }

    /// Records what the desktop is set to, and follows it if it is being
    /// followed.
    ///
    /// Called once when the window opens and again whenever the setting moves.
    /// The resolution itself is [`Settings::theme_for`] — a pure function of
    /// the flag, the pair of names and this one bit — so nothing here decides
    /// anything a test would need a desktop to reproduce.
    pub fn set_system_dark(&mut self, dark: bool, ctx: &mut ViewContext<Self>) {
        self.system_is_dark = dark;
        if !self.general().use_system_theme {
            return;
        }

        let wanted = self.settings.theme_for(dark).to_owned();
        if wanted == self.settings.theme() {
            return;
        }
        self.set_theme(&wanted, ctx);
    }

    /// Whether the desktop is set to dark, as of the last thing the window
    /// said.
    pub fn system_is_dark(&self) -> bool {
        self.system_is_dark
    }

    /// Turns following the desktop on or off.
    ///
    /// Turning it on applies whichever half the desktop is currently in, which
    /// is the only way the switch can be honest: a toggle that changed nothing
    /// until the next sunset would look broken.
    pub fn set_follow_system_theme(&mut self, follow: bool, ctx: &mut ViewContext<Self>) {
        let mut general = self.general();
        if general.use_system_theme == follow {
            return;
        }
        general.use_system_theme = follow;
        self.set_general(general, ctx);

        if follow {
            let wanted = self.settings.theme_for(self.system_is_dark).to_owned();
            self.set_theme(&wanted, ctx);
        }
    }

    /// Hands the terminals the palette the theme in force resolves to.
    fn sync_palette(&self, ctx: &mut ViewContext<Self>) {
        let palette = crate::terminal_model::crook_palette();
        self.terminals
            .update(ctx, |model, ctx| model.set_palette(palette, ctx));
    }

    /// Which build this is: `dev` or `stable`.
    pub fn channel(&self) -> &'static str {
        self.channel.name()
    }

    /// The usage model, for the settings page's live reading.
    /// The usage model, whose poll this workspace starts and stops.
    ///
    /// Public for the tests, which is the honest reason: what the workspace
    /// still owns of the usage feature is the poll's switch — `crook/usage`
    /// owns the chip and the page — and a test asserting that a hidden chip
    /// stops polling has to be able to ask.
    pub fn usage(&self) -> &ModelHandle<UsageModel> {
        &self.usage
    }

    /// The settings page's state.
    pub(crate) fn settings_page(&self) -> &SettingsState {
        &self.page
    }

    /// How many worktrees the menu has read, or `None` while it is still
    /// reading or has nothing to read.
    ///
    /// For a test, which otherwise has to infer this from pixels — and the
    /// window behind a popup is still in the scene, so "the menu does not say
    /// it is reading" is not the same question.
    pub fn worktrees_listed(&self) -> Option<usize> {
        match &self.tab_menu.contents {
            Contents::Ready(worktrees) => Some(worktrees.len()),
            _ => None,
        }
    }

    /// Whether the menu is asking about removing a checkout. For a test.
    pub fn worktree_menu_is_confirming(&self) -> bool {
        matches!(self.tab_menu.mode, WorktreeMode::Removing { .. })
    }

    /// Whether the menu is making a worktree. For a test.
    pub fn worktree_menu_is_creating(&self) -> bool {
        self.tab_menu.mode == WorktreeMode::Creating
    }

    /// The plugins, and the slots and actions they registered.
    /// Runs a plugin's named action.
    ///
    /// The registry is cloned out of the host before the handler is called,
    /// because the handler takes `&mut Workspace` and the host is a field of
    /// it. What the handler sees is the workspace after everything the
    /// keystroke or the click did to it, which is the same thing every arm of
    /// `handle_action` sees.
    ///
    /// An id whose plugin has been disabled resolves to nothing and this does
    /// nothing — see [`ActionId`].
    /// Switches one plugin off, or back on, and remembers the answer.
    ///
    /// Both halves matter. The host is changed now, so the next frame is drawn
    /// without whatever the plugin was contributing — or with it — and the
    /// settings file is written, so the answer survives a restart. A switch
    /// that only did the first would be a switch that lies the next time the
    /// window opens.
    pub fn toggle_plugin(&mut self, plugin: &PluginId, ctx: &mut ViewContext<Self>) {
        let turning_off = self.host.is_loaded(plugin);
        self.settings
            .set_plugin_disabled(plugin.as_str(), turning_off);
        self.save_settings(ctx);

        if turning_off {
            self.host.unload(plugin);
        } else {
            self.host.enable(plugin, ctx);
        }
        // A plugin's surface may have been what was holding the keyboard.
        self.sync_input_keys();
        ctx.notify();
    }

    pub fn run_action(&mut self, id: ActionId, ctx: &mut ViewContext<Self>) {
        let Some(name) = self.host.action_name(id).cloned() else {
            return;
        };
        let actions = self.host.actions().clone();
        actions.with(&name, |handler| handler(self, ctx));
    }

    /// The person's own bindings, for the page that prints them.
    pub(crate) fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    /// Replaces them, which is what re-reading `keymap.json` does.
    ///
    /// The bindings are read once at startup today, so this has one caller and
    /// it is a test. It is not a test-only method: a keymap that can be
    /// replaced is what "you changed the file, here it is" needs, and there is
    /// nothing about swapping the table that has to wait for that.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap;
    }

    /// The plugins, and everything they registered.
    ///
    /// `pub` because a plugin's contribution is handed `&Workspace` and the
    /// registries are what it was contributing to — a palette that could not
    /// ask what commands exist is a palette with a hard-coded list.
    pub fn host(&self) -> &Host {
        &self.host
    }

    /// The menu a tab opens, which is about worktrees.
    pub(super) fn tab_menu(&self) -> &TabMenuState {
        &self.tab_menu
    }

    /// Every pane in the window, with the directory it is in.
    ///
    /// Every pane rather than every tab's focused one, and that is what makes
    /// "this checkout is already open" a true answer: a worktree opened from a
    /// tab now lands in a pane *beside* the one that asked for it, so the tab
    /// whose row a person is looking at is very often not the pane the branch
    /// is in. Asking the focused pane only would offer to remove a checkout an
    /// agent is working in, and would open a second pane on it a moment later.
    pub(super) fn pane_directories(&self) -> Vec<(PaneId, PathBuf)> {
        self.tabs
            .panes()
            .filter_map(|(_, pane)| Some((pane.id(), pane.session().working_directory.clone()?)))
            .collect()
    }

    /// Whether any popup is up.
    ///
    /// One question, asked in five places, because the answer is what decides
    /// whether a pane has the keyboard, whether a hover card may open, whether
    /// a caret blinks and whether the grid takes a key. Two popups are never
    /// up at once — opening either closes the other — and the reason is not
    /// tidiness: a modal underlay covers only the layers painted *before* it,
    /// so the second one's popup would float above the first one's underlay
    /// while the first one's underlay swallowed the press meant to dismiss it.
    pub(super) fn a_popup_is_open(&self) -> bool {
        self.menu.open || self.tab_menu.is_open()
    }

    /// The Themes panel's state.
    pub(super) fn theme_panel(&self) -> &ThemePanelState {
        &self.panel
    }

    /// Whether the Themes panel is up.
    pub fn is_theme_panel_open(&self) -> bool {
        self.panel.open
    }

    /// Opens the Themes panel, for a run that was asked to start on it.
    ///
    /// The same two actions a click on the settings row and a click on the `+`
    /// send, so a snapshot of the panel is a snapshot of the real thing.
    pub fn open_theme_panel(&mut self, creating: bool, ctx: &mut ViewContext<Self>) {
        self.apply_theme_action(ThemeAction::OpenPanel, ctx);
        if creating {
            self.apply_theme_action(ThemeAction::StartCreating, ctx);
        }
    }

    /// Whether it is showing the creator.
    pub fn is_creating_theme(&self) -> bool {
        self.panel.mode == Mode::Creating
    }

    /// Whether one pane's field is listening to the keyboard.
    ///
    /// The answer `sync_input_keys` last wrote, read back — which is what a
    /// test asking "did the panel take the keyboard" wants to know.
    pub fn pane_takes_keys(&self, pane: PaneId) -> bool {
        self.inputs.get(&pane).is_some_and(|input| input.has_keys())
    }

    /// Every theme that can be chosen.
    ///
    /// The list both surfaces read. Refreshed when one of them opens rather
    /// than on the render path: reading it walks a directory, and a panel that
    /// did so while a pointer moved over it would `readdir` sixty times a
    /// second.
    pub(super) fn themes(&self) -> &[Available] {
        &self.themes
    }

    /// Points the themes folder somewhere else.
    ///
    /// For a test, and for a run that must not read or write the folder of
    /// whoever started it.
    /// Puts the worktrees Crook makes somewhere else.
    ///
    /// A test's, and only a test's: creating one writes a whole checkout to
    /// disk, and a suite that wrote into the data directory of whoever ran it
    /// would leave real repositories lying about on their machine.
    pub fn set_worktrees_directory(&mut self, directory: PathBuf) {
        self.worktrees_directory = Some(directory);
    }

    pub fn set_themes_directory(&mut self, directory: PathBuf) {
        self.themes_directory = Some(directory);
        self.refresh_themes();
    }

    /// Re-reads the themes folder, and keeps the keyboard's row pointing at
    /// the theme in force.
    fn refresh_themes(&mut self) {
        self.themes = match self.themes_directory.as_deref() {
            Some(directory) => crate::theme::available_in(directory),
            None => crate::theme::available(),
        };
        self.select_theme_in_force();
    }

    /// Re-reads the themes folder while the panel is open, so a file edited in
    /// an editor takes effect in the window beside it.
    ///
    /// **A poll, and only while somebody is looking.** Warp watches its themes
    /// directory with a filesystem watcher; that is a dependency, a thread and
    /// a per-platform API for a folder that changes when a person is editing a
    /// theme — which is exactly when the panel is open. Closed, this costs
    /// nothing at all: the chain ends at the first tick that finds the panel
    /// gone, and an application that is idle by design stays idle.
    ///
    /// The read happens on the background pool, because it is a directory walk
    /// and a parse per file, and neither belongs on the thread that draws. The
    /// wait and the read are one task, so this is one of the chains
    /// [`crate::PARKED_WORKERS`] counts — the only one that comes and goes with
    /// a panel rather than running for the life of the window.
    fn watch_themes(&self, ctx: &mut ViewContext<Self>) {
        if !self.panel.open {
            return;
        }

        let directory = self.themes_directory.clone();
        let reading = ctx.background().spawn(async move {
            std::thread::sleep(THEMES_POLL);
            match directory.as_deref() {
                Some(directory) => crate::theme::available_in(directory),
                None => crate::theme::available(),
            }
        });

        ctx.spawn(reading, |workspace, themes, ctx| {
            workspace.adopt_themes(themes, ctx);
            workspace.watch_themes(ctx);
        })
        .detach();
    }

    /// Takes a freshly read themes folder, re-applying the theme in force if
    /// its own file is what changed.
    ///
    /// By *name*, which is the only way an edit can be noticed: the palette in
    /// force is the old one, so looking the theme up by palette would find the
    /// row it used to be and conclude nothing had happened.
    fn adopt_themes(&mut self, themes: Vec<crate::theme::Available>, ctx: &mut ViewContext<Self>) {
        // The creator paints a draft on the window. Re-applying anything under
        // it would replace a palette somebody is in the middle of choosing.
        if !self.panel.open || self.panel.mode == Mode::Creating || self.themes == themes {
            return;
        }

        self.themes = themes;
        let name = self.settings.theme().to_owned();
        if let Some(edited) = self
            .themes
            .iter()
            .find(|available| available.name == name)
            .map(|available| available.theme)
            && edited != theme()
        {
            crate::theme::set_theme(edited);
            self.sync_palette(ctx);
        }

        self.select_theme_in_force();
        ctx.notify();
    }

    /// Puts the keyboard's row on the theme that is on screen.
    ///
    /// By palette rather than by name, for the reason [`Self::theme_name`]
    /// answers by palette: under `--theme` the saved name is not what a person
    /// is looking at, and the row they would arrow away from has to be the one
    /// they can see.
    fn select_theme_in_force(&mut self) {
        let showing = theme();
        self.panel.selected = self
            .themes
            .iter()
            .position(|available| available.theme == showing)
            .unwrap_or(0);
    }

    /// Which page of the settings the rail has selected, by its key.
    ///
    /// Public for the snapshot path and the tests: which page is showing is
    /// not something a renderer asks for — it reads
    /// [`Self::settings_page`] — and it is the one piece of the page's state
    /// worth asserting from outside.
    pub fn settings_page_key(&self) -> Option<String> {
        let id = self.page.selected(&self.host)?;
        self.host.settings_page_key(id).map(str::to_owned)
    }

    /// What the rail row of the selected page says.
    pub fn settings_page_title(&self) -> Option<String> {
        let id = self.page.selected(&self.host)?;
        self.host.settings_page_title(id)
    }

    /// The page whose rail row says `title`, however it is spelled.
    ///
    /// What `--settings appearance` resolves through, and what a test names a
    /// page by. The title rather than the key, because the title is what is
    /// written on the rail and the key is `owner/entry` — nobody types that,
    /// and a plugin's page should be as reachable as one of Crook's.
    pub fn settings_page_named(&self, title: &str) -> Option<PageId> {
        self.host
            .settings_pages()
            .into_iter()
            .find(|(_, name)| name.eq_ignore_ascii_case(title))
            .map(|(id, _)| id)
    }

    /// How far the tabs panel's list has been scrolled.
    pub(super) fn panel_scroll(&self) -> ScrollStateHandle {
        self.panel_scroll.clone()
    }

    /// Where the panel's rows were drawn on the last frame.
    pub(super) fn panel_rows(&self) -> RowGeometry {
        self.panel_rows.clone()
    }

    /// The search box above the tab list.
    pub(crate) fn panel_search(&self) -> &SearchState {
        &self.panel_search
    }

    /// Whether that box is on screen at all.
    ///
    /// It belongs to the tabs, so it is drawn when the tabs are. The Themes
    /// panel does not come into it: it is a column of its own beside the
    /// sidebar, and the tab list and its box stay where they were while it is
    /// up. Asked in two places and answered in one: the panel draws the box by
    /// it, and [`Self::search_takes_keys`] refuses the keyboard to a box
    /// nobody can see.
    pub(super) fn panel_search_is_showing(&self) -> bool {
        self.section.is_none()
    }

    /// Whether the keyboard is the search box's rather than the pane's.
    ///
    /// The wish is [`SearchState::is_focused`]; this is the wish granted. Every
    /// other clause is a thing that has taken the keyboard away from the pane
    /// as well, so this is the one place where "the box is being typed into"
    /// and "the pane is being typed into" are settled against each other — and
    /// they are settled by asking one question, which is why they cannot both
    /// be true.
    ///
    /// A menu or a panel opening suspends the wish rather than ending it, so
    /// closing one a person opened by accident gives them back the box they
    /// were typing in. The Themes panel is in that list even though it no
    /// longer covers the box — it is a column beside the sidebar now — because
    /// it takes the keyboard whole while it is up, arrow keys and letters
    /// together; see [`Self::action_for`]. Leaving the section is the one
    /// thing that ends the wish, in [`Self::show_section`]: the box is gone,
    /// not covered.
    pub(super) fn search_takes_keys(&self) -> bool {
        self.panel_search.is_focused()
            && self.panel_search_is_showing()
            && !self.panel.open
            && !self.a_popup_is_open()
            && !self.host.a_surface_is_up()
    }

    /// A text field belonging to one section, made the first time it is drawn.
    ///
    /// The index it hands back is what an action carries, because
    /// [`WorkspaceAction`] is `Copy` and a field is named by two strings.
    pub(crate) fn field(&self, section: &str, name: &str) -> (usize, TextInput) {
        let mut fields = self.fields.borrow_mut();
        if let Some(index) = fields
            .iter()
            .position(|(owner, known, _)| owner == section && known == name)
        {
            return (index, fields[index].2.clone());
        }
        let input = TextInput::new();
        fields.push((section.to_owned(), name.to_owned(), input.clone()));
        let index = fields.len() - 1;
        drop(fields);

        // Told now rather than at the next `sync_input_keys`. A field is
        // registered while its section renders, which is *after* the change
        // that showed the section — so a field left to wait would be inert
        // until something else moved the focus, and the first thing typed
        // after switching sections would go nowhere.
        input.set_has_keys(
            self.section.as_deref() == Some(section)
                && self.field_with_keys().as_deref() == Some(name),
        );
        (index, input)
    }

    /// Moves the keyboard to one of them.
    fn focus_field(&self, index: usize) {
        let fields = self.fields.borrow();
        let Some((section, name, _)) = fields.get(index) else {
            return;
        };
        self.focused_field
            .borrow_mut()
            .insert(section.clone(), name.clone());
    }

    /// Which field of the section that is showing has the keyboard.
    ///
    /// The one pressed last, or the first that section drew — which is the
    /// only rule a person can predict with no focus ring to look at, and the
    /// only one that does not leave a section's single field inert until it is
    /// clicked.
    fn field_with_keys(&self) -> Option<String> {
        let showing = self.section.clone()?;
        let recorded = self.focused_field.borrow().get(&showing).cloned();
        recorded.or_else(|| {
            self.fields
                .borrow()
                .iter()
                .find(|(section, _, _)| *section == showing)
                .map(|(_, name, _)| name.clone())
        })
    }

    /// Empties every field, which is what leaving a section does.
    fn clear_fields(&self) {
        for (_, _, input) in self.fields.borrow().iter() {
            input.edit(crate::editor::Editor::clear);
        }
        self.focused_field.borrow_mut().clear();
    }

    /// The mouse state for one of the sidebar's section buttons, made the
    /// first time it is drawn.
    ///
    /// Keyed by the section's own key so that a plugin switched off and back
    /// on gets its state back rather than a neighbour's — the same rule the
    /// settings page's controls follow, for the same reason.
    pub(super) fn section_button(&self, key: &str) -> MouseStateHandle {
        self.section_buttons
            .borrow_mut()
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    /// Brings the focused pane's row into view in the tabs panel.
    ///
    /// The gap that used to be written down in `tabs_panel`'s module docs:
    /// selecting a tab from the keyboard moved the selection whether or not
    /// its row was on screen. What it needed was a scrollable that can be told
    /// to make a particular child visible, and what that needs is somewhere
    /// for the children to say where they ended up — which is
    /// [`RowGeometry`](super::tabs_panel::geometry::RowGeometry).
    ///
    /// Against the row boxes the *last* frame recorded, which is right: the
    /// rows do not move when the selection does, so a frame that has not been
    /// drawn yet would report the same offsets. A row that was never drawn —
    /// the first selection of a session, before any frame — scrolls nowhere,
    /// and the next selection finds it.
    fn scroll_row_into_view(&self) {
        let Some(pane) = self.tabs.focused_pane_id() else {
            return;
        };
        let Some(row) = self.panel_rows.get(pane) else {
            return;
        };

        let mut scroll = self.panel_scroll.lock();
        if let Some(offset) =
            tabs_panel::geometry::scroll_for(row, scroll.offset(), scroll.viewport())
        {
            scroll.scroll_to(offset);
        }
    }

    /// Whether the sidebar is showing the settings.
    pub fn is_settings_page_open(&self) -> bool {
        self.showing_section().is_some()
            && self.showing_section() == self.host.sidebar_section_id(SETTINGS_SECTION)
    }

    /// Shows the settings, at `page`, for a run that was asked to start there.
    ///
    /// The same two steps a click on the sidebar's button and a click on the
    /// rail take, in that order, so a snapshot of the page is a snapshot of
    /// the real thing rather than of a second code path.
    pub fn open_settings_page(&mut self, page: Option<PageId>, ctx: &mut ViewContext<Self>) {
        if let Some(page) = page {
            self.apply_settings(SettingsAction::Select(page), ctx);
        }
        let settings = self.host.sidebar_section_id(SETTINGS_SECTION);
        self.show_section(settings, ctx);
    }

    /// The section whose button says `title`, however it is spelled.
    ///
    /// The title rather than the key, because the title is what is written on
    /// the button and the key is `owner/entry` — nobody types that.
    pub fn section_named(&self, title: &str) -> Option<SectionId> {
        self.host
            .sidebar_sections()
            .into_iter()
            .find(|(_, name, _)| name.eq_ignore_ascii_case(title))
            .map(|(id, _, _)| id)
    }

    /// Shows one section of the sidebar, or the tabs.
    pub fn show_section(&mut self, section: Option<SectionId>, ctx: &mut ViewContext<Self>) {
        let key = section.and_then(|id| self.host.sidebar_section_key(id).map(str::to_owned));
        if self.section == key {
            return;
        }
        self.section = key;
        // The sidebar and the window are both about to be replaced, so every
        // control the pointer was on is about to stop existing without ever
        // seeing a hover-out — and a field on the section being left must not
        // go on holding the keyboard.
        self.forget_section_state();
        self.clear_fields();
        // The panel's box goes with them, and for the same reason: it filters
        // a list that is about to be replaced, and coming back to the tabs to
        // find four of forty is a sidebar that reads as broken rather than as
        // filtered. The keyboard goes back with it, because a person who
        // clicked their way out of the box was finished with it — anything
        // that merely covers it is handled by `search_takes_keys` instead.
        self.panel_search.clear();
        self.panel_search.set_focused(false);
        self.sync_input_keys();
        ctx.notify();
    }

    /// Which section the sidebar is showing, if it is not showing the tabs.
    pub fn showing_section(&self) -> Option<SectionId> {
        self.section
            .as_deref()
            .and_then(|key| self.host.sidebar_section_id(key))
    }

    /// Drops what the section being left was holding.
    fn forget_section_state(&mut self) {
        self.hovered_row = None;
        self.page.forget_hover_state();
        for interaction in self.interactions.values() {
            interaction.chip.lock().reset_interaction_state();
            interaction.close.lock().reset_interaction_state();
            interaction.body.lock().reset_interaction_state();
        }
        for chrome in self.tab_chrome.values() {
            chrome.container.lock().reset_interaction_state();
            chrome.header.lock().reset_interaction_state();
        }
    }

    /// What is in the settings page's search box, for a test to read back.
    pub fn settings_search_text(&self) -> String {
        self.field(SETTINGS_SECTION, "search")
            .1
            .editor()
            .text()
            .to_owned()
    }

    /// What is in the panel's search box, for a test to read back.
    pub fn panel_search_text(&self) -> String {
        self.panel_search.input().editor().text().to_owned()
    }

    /// Types `query` into the settings page's search box, for a run that was
    /// asked to start with something searched for.
    ///
    /// Straight into the editor rather than through a keystroke each: what the
    /// box does with a keystroke is insert a character, and a snapshot wants
    /// the state that leaves rather than the path to it.
    pub fn type_into_settings_search(&mut self, query: &str, ctx: &mut ViewContext<Self>) {
        self.field(SETTINGS_SECTION, "search")
            .1
            .edit(|editor| editor.set_text(query));
        ctx.notify();
    }

    /// Types `query` into the panel's search box, for a run that was told to.
    ///
    /// With the keyboard in it, unlike the settings page's: that box has the
    /// keyboard by virtue of its section being on screen, and this one only
    /// ever has it because somebody put it there — a snapshot of a filtered
    /// list with an unfocused box would be a picture of a state that cannot
    /// happen.
    pub fn type_into_panel_search(&mut self, query: &str, ctx: &mut ViewContext<Self>) {
        self.show_section(None, ctx);
        self.panel_search
            .input()
            .edit(|editor| editor.set_text(query));
        self.panel_search.set_focused(true);
        self.sync_input_keys();
        ctx.notify();
    }

    /// Everything the menu on a tab does.
    ///
    /// Every arm that asks git anything does it on the background pool and
    /// lands the answer through `ctx.spawn`. Nothing here blocks the thread
    /// that draws — the rule the git layer is built on — which is why the menu
    /// has a state for "reading" at all.
    fn apply_worktree(&mut self, action: WorktreeAction, ctx: &mut ViewContext<Self>) {
        match action {
            WorktreeAction::OpenMenu(tab) => self.open_tab_menu(tab, ctx),
            WorktreeAction::CloseMenu => self.close_tab_menu(ctx),

            WorktreeAction::Show(index) => {
                let Some(worktree) = self.tab_menu.worktrees().get(index) else {
                    return;
                };
                let path = worktree.path.clone();
                // By the same longest-match rule the badges use, and for the
                // same reason: a linked worktree is very often *inside* the
                // main checkout — Crook's own are — so a tab in the nested one
                // is under both paths, and a plain prefix would have the main
                // checkout's row bring forward a tab that is somewhere else
                // entirely.
                let worktrees = self.tab_menu.worktrees();
                let existing = self
                    .pane_directories()
                    .into_iter()
                    .find(|(_, directory)| {
                        super::tab_menu::holding(worktrees, Some(directory)) == Some(index)
                    })
                    .map(|(pane, _)| pane);

                let opened_on = self.tab_menu.tab;
                self.close_tab_menu(ctx);
                match existing {
                    // Already open: bring it forward rather than opening a
                    // second pane on the same checkout. Two agents in one
                    // worktree is the thing this whole feature exists to stop.
                    // By pane, because that is where a checkout lives now, and
                    // focusing one activates the tab holding it.
                    Some(pane) => {
                        self.apply(TabAction::FocusPane(pane), ctx);
                    }
                    // Beside the tab that asked, so the branch keeps the
                    // company of the checkout it came from.
                    None => match opened_on {
                        Some(tab) => {
                            self.open_pane_in(tab, path, ctx);
                        }
                        None => {
                            self.open_tab_in(path, ctx);
                        }
                    },
                }
            }

            WorktreeAction::StartCreating => {
                // Not until the repository has been read. The name offered has
                // to be one no existing worktree is using, and where the
                // checkout goes is derived from what the repository is called
                // — both of which are answers the list carries. Opening the
                // creator over a list that had not arrived would offer a name
                // chosen against nothing and then refuse to use it.
                if !matches!(self.tab_menu.contents, Contents::Ready(_)) {
                    return;
                }

                // Pre-filled with a name nothing is using, so the shortest way
                // through is to press the button. herdr does the same, and the
                // reason is that a person who has not decided on a name yet
                // still wants the worktree.
                //
                // Both lists, because the branches are the half that decides:
                // `remove` never deletes a branch, so a name offered against
                // the worktrees alone comes back the moment its checkout goes
                // and `add` refuses it every time after that.
                let branch = crate::git::worktree::suggested_branch(
                    self.tab_menu.worktrees(),
                    &self.tab_menu.branches,
                );
                self.tab_menu.branch.edit(|editor| {
                    editor.set_text(&branch);
                    editor.select_all();
                });
                self.tab_menu.problem = None;
                self.tab_menu.mode = WorktreeMode::Creating;
                self.tab_menu.forget_hover_state();
                self.sync_input_keys();
                ctx.notify();
            }

            WorktreeAction::Create => self.create_worktree(ctx),
            WorktreeAction::AskRemove(index) => self.ask_about_removing(index, ctx),
            WorktreeAction::Remove { force } => self.remove_worktree(force, ctx),

            WorktreeAction::Cancel => {
                self.tab_menu.mode = WorktreeMode::Listing;
                self.tab_menu.problem = None;
                self.tab_menu.forget_hover_state();
                self.sync_input_keys();
                ctx.notify();
            }
        }
    }

    /// Opens the menu on a tab, and reads the repository behind it.
    ///
    /// Pressing again on the tab whose menu is already up closes it, which is
    /// the gear's rule and the one a person expects of anything that opens by
    /// being clicked. In practice the modal underlay gets that press first and
    /// dismisses on it; this is what makes the toggle right anyway, for the
    /// keyboard and for anything else that dispatches the action.
    fn open_tab_menu(&mut self, tab: TabId, ctx: &mut ViewContext<Self>) {
        if self.tab_menu.tab == Some(tab) {
            self.close_tab_menu(ctx);
            return;
        }

        let directory = self
            .tabs
            .get(tab)
            .and_then(|tab| tab.panes().focused())
            .and_then(|pane| pane.session().working_directory.clone());
        let Some(directory) = directory else {
            return;
        };

        // Two popups are never up at once. See `a_popup_is_open`.
        self.close_menu();

        self.tab_menu.tab = Some(tab);
        self.tab_menu.pane_directory = Some(directory.clone());
        self.tab_menu.mode = WorktreeMode::Listing;
        self.tab_menu.contents = Contents::Reading;
        self.tab_menu.branches.clear();
        self.tab_menu.problem = None;
        self.tab_menu.working = false;
        self.tab_menu.store = self.worktrees_directory.clone();
        self.tab_menu.forget_hover_state();
        self.sync_input_keys();
        ctx.notify();

        let reading = ctx.background().spawn(async move {
            let worktrees = crate::git::worktree::list(&directory)?;
            // Best effort, and second, because the two answers are not worth
            // the same. The listing *is* the menu, and failing to read it is a
            // message where the rows would be; the branches only feed the name
            // the creator offers, and a suggestion made without them is worse
            // than one made with them and far better than no menu at all.
            let branches = crate::git::worktree::branches(&directory).unwrap_or_default();
            // Named, because the branches are read with their own error
            // swallowed and nothing else in the block says what this one is.
            Ok::<_, crate::git::worktree::Error>((worktrees, branches))
        });

        ctx.spawn(reading, move |workspace, listed, ctx| {
            // The menu may have been taken down, or opened on another tab,
            // while git was being asked. The answer belongs to the tab it was
            // asked for and to no other.
            if workspace.tab_menu.tab != Some(tab) {
                return;
            }
            workspace.tab_menu.contents = match listed {
                Ok((worktrees, branches)) => {
                    workspace.tab_menu.repository = worktrees
                        .first()
                        .and_then(|worktree| worktree.path.file_name())
                        .map(|name| name.to_string_lossy().into_owned());
                    workspace.tab_menu.branches = branches;
                    Contents::Ready(worktrees)
                }
                Err(problem) => Contents::Failed(problem.to_string()),
            };
            ctx.notify();
        })
        .detach();
    }

    /// Opens the menu on the active tab and reads the repository *now*, for a
    /// run that was asked to start with it up.
    ///
    /// Blocking, and only here: a snapshot draws one frame and would otherwise
    /// photograph the menu saying it is still reading. Every other way in goes
    /// through [`Self::open_tab_menu`] and its background read.
    pub fn open_tab_menu_for_snapshot(&mut self, ctx: &mut ViewContext<Self>) {
        let tab = self.tabs.active_id();

        // The demo's directories are invented — they name a checkout of Crook
        // that is not on this machine — and a menu about a repository that
        // does not exist is a picture of an error message. The focused pane is
        // pointed at the directory the process is actually running in, which
        // is a repository whenever anybody is looking at this flag.
        if let Some(directory) = std::env::current_dir().ok().filter(|path| path.is_dir())
            && let Some(pane) = self.tabs.focused_pane_id()
        {
            self.update_session(pane, ctx, |session| {
                session.working_directory = Some(directory);
            });
        }

        self.open_tab_menu(tab, ctx);
        if !self.tab_menu.is_open() {
            return;
        }

        let Some(directory) = self.tab_menu.pane_directory.clone() else {
            return;
        };
        self.tab_menu.contents = match crate::git::worktree::list(&directory) {
            Ok(worktrees) => {
                self.tab_menu.repository = worktrees
                    .first()
                    .and_then(|worktree| worktree.path.file_name())
                    .map(|name| name.to_string_lossy().into_owned());
                self.tab_menu.branches =
                    crate::git::worktree::branches(&directory).unwrap_or_default();
                Contents::Ready(worktrees)
            }
            Err(problem) => Contents::Failed(problem.to_string()),
        };
        ctx.notify();
    }

    /// Puts the menu into its creator, for a run that was asked to start there.
    pub fn start_creating_worktree(&mut self, ctx: &mut ViewContext<Self>) {
        self.apply_worktree(WorktreeAction::StartCreating, ctx);
    }

    /// Takes the menu down.
    fn close_tab_menu(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.tab_menu.is_open() {
            return;
        }

        self.tab_menu.tab = None;
        self.tab_menu.mode = WorktreeMode::Listing;
        self.tab_menu.contents = Contents::Reading;
        self.tab_menu.branches.clear();
        self.tab_menu.problem = None;
        self.tab_menu.working = false;
        self.tab_menu.forget_hover_state();
        self.sync_input_keys();
        ctx.notify();
    }

    /// Makes the worktree the creator describes, and opens a pane in it.
    fn create_worktree(&mut self, ctx: &mut ViewContext<Self>) {
        if self.tab_menu.working {
            return;
        }

        let branch = self.tab_menu.branch.editor().text().trim().to_owned();
        let (Some(repository), Some(path)) = (
            self.tab_menu.pane_directory.clone(),
            super::tab_menu::checkout_for(&self.tab_menu),
        ) else {
            // The one thing the creator can be missing is a name, and the
            // field says so more usefully than a sentence would.
            self.tab_menu.problem = Some("A worktree needs a branch name.".to_owned());
            ctx.notify();
            return;
        };

        self.tab_menu.working = true;
        self.tab_menu.problem = None;
        ctx.notify();

        let opened_on = self.tab_menu.tab;
        let made = ctx.background().spawn({
            let path = path.clone();
            async move { crate::git::worktree::add(&repository, &path, &branch, None) }
        });

        ctx.spawn(made, move |workspace, made, ctx| {
            // The menu this was asked for may have been taken down, or opened
            // on another tab, while git was checking a working tree out. The
            // checkout still happened and the tab still opens — that is what
            // was asked for — but the *dialog's* state belongs to the menu
            // that asked, and writing "working = false" or a failure into a
            // different one is writing into somebody else's question.
            let answering = workspace.tab_menu.tab == opened_on;
            if answering {
                workspace.tab_menu.working = false;
            }
            match made {
                // Creating one opens it, which is the whole point: a worktree
                // nobody is working in is a directory.
                Ok(()) => {
                    if answering {
                        workspace.close_tab_menu(ctx);
                    }
                    // Beside the tab it was asked for, which is what
                    // `open_pane_in` falls back out of if that tab has closed
                    // in the meantime.
                    match opened_on {
                        Some(tab) => workspace.open_pane_in(tab, path, ctx),
                        None => workspace.open_tab_in(path, ctx),
                    };
                }
                Err(problem) => {
                    if answering {
                        workspace.tab_menu.problem = Some(problem.to_string());
                        ctx.notify();
                    } else {
                        log::warn!("a worktree nobody is waiting for failed: {problem}");
                    }
                }
            }
        })
        .detach();
    }

    /// Asks about removing one, and counts what is in it while it asks.
    fn ask_about_removing(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(worktree) = self.tab_menu.worktrees().get(index) else {
            return;
        };
        let path = worktree.path.clone();

        self.tab_menu.mode = WorktreeMode::Removing {
            index,
            local: None,
            refused: false,
        };
        self.tab_menu.problem = None;
        self.tab_menu.forget_hover_state();
        ctx.notify();

        let counting = ctx
            .background()
            .spawn(async move { crate::git::worktree::local_work(&path) });

        ctx.spawn(counting, move |workspace, counted, ctx| {
            // Only into the question that asked it. A count that arrived after
            // the person had gone back to the list would put a line under a
            // heading that is no longer there.
            if let WorktreeMode::Removing {
                index: asking,
                local,
                ..
            } = &mut workspace.tab_menu.mode
                && *asking == index
            {
                *local = counted.ok();
                ctx.notify();
            }
        })
        .detach();
    }

    /// Removes the checkout the confirmation is about.
    fn remove_worktree(&mut self, force: bool, ctx: &mut ViewContext<Self>) {
        let WorktreeMode::Removing { index, .. } = self.tab_menu.mode else {
            return;
        };
        if self.tab_menu.working {
            return;
        }
        let (Some(worktree), Some(repository)) = (
            self.tab_menu.worktrees().get(index),
            self.tab_menu.pane_directory.clone(),
        ) else {
            return;
        };
        let path = worktree.path.clone();

        self.tab_menu.working = true;
        self.tab_menu.problem = None;
        ctx.notify();

        let asked_about = index;
        let opened_on = self.tab_menu.tab;
        let removed = ctx.background().spawn({
            let path = path.clone();
            async move { crate::git::worktree::remove(&repository, &path, force) }
        });

        ctx.spawn(removed, move |workspace, removed, ctx| {
            if workspace.tab_menu.tab != opened_on {
                if let Err(problem) = removed {
                    log::warn!("a removal nobody is waiting for failed: {problem}");
                }
                return;
            }
            workspace.tab_menu.working = false;
            match removed {
                Ok(()) => {
                    // Back to the list, which has to be read again: the thing
                    // it was listing is gone.
                    if let Some(tab) = workspace.tab_menu.tab {
                        workspace.tab_menu.tab = None;
                        workspace.open_tab_menu(tab, ctx);
                    }
                }
                // The one refusal that is a question rather than an error:
                // there is work in there, and the person can still say yes.
                // Marked on the confirmation that *asked* — `refused` is what
                // turns the button into the one that deletes anyway, and
                // setting it on whichever confirmation happens to be up would
                // arm that button over a checkout git never objected to.
                Err(crate::git::worktree::Error::HoldsLocalWork { .. }) => {
                    if let WorktreeMode::Removing { index, refused, .. } =
                        &mut workspace.tab_menu.mode
                        && *index == asked_about
                    {
                        *refused = true;
                    }
                    ctx.notify();
                }
                Err(problem) => {
                    workspace.tab_menu.problem = Some(problem.to_string());
                    ctx.notify();
                }
            }
        })
        .detach();
    }

    /// Whether the options menu is up.
    pub fn is_options_menu_open(&self) -> bool {
        self.menu.open
    }

    /// The model the strip reads its git facts out of.
    ///
    /// Exposed so a snapshot and a test can put known facts in front of the
    /// renderer with [`GitModel::record`]. Neither of them starts the gather
    /// chain, and a frame with no branch on any row would show nothing of what
    /// the options do.
    pub fn git(&self) -> &ModelHandle<GitModel> {
        &self.git
    }

    /// Replaces the options, saves them, and repaints what they changed.
    ///
    /// The one write path. Every menu click comes through here — so "the
    /// options changed" and "the file needs writing" and "the window is dirty"
    /// are one statement rather than three. What reaches the file is
    /// [`Self::persisted`] rather than `options`, which is the whole of how a
    /// command-line density stays out of it.
    pub fn set_options(&mut self, options: TabOptions, ctx: &mut ViewContext<Self>) {
        // An option set to what it already was repaints nothing. Clicking the
        // selected half of a segmented control is the ordinary way to do that.
        if options == self.options {
            return;
        }

        // An option that actually moved is a choice somebody made, and it ends
        // the command line's override of that option: from here on the file
        // learns it.
        self.overridden.clear(Overridden {
            density: options.density != self.options.density,
            granularity: options.granularity != self.options.granularity,
        });

        self.options = options;
        let persisted = self.persisted();
        self.settings.set_tab_options(persisted);

        // Turning the card off takes down the one that is up, the way Warp's
        // settings-changed handler clears its sidecar rather than waiting for
        // a hover-out that may never come.
        if !options.show_details_on_hover {
            self.hovered_row = None;
        }

        let wants_diff = wants_diff_stats(options);
        self.git
            .update(ctx, |model, _| model.set_diff_stats_wanted(wants_diff));

        self.save_settings(ctx);
        ctx.notify();
    }

    /// Replaces the options that are not the tab strip's, saves them, and
    /// starts or stops whatever they gate.
    ///
    /// The mirror of [`Self::set_options`] for the other group, and it has one
    /// job that one does not: two of these switches are also models', so the
    /// models are told before the file is written. A person who turns the chip
    /// off has said they do not want Crook talking to the network, and waiting
    /// for a background save to land before acting on that would be the wrong
    /// order to do two things in.
    fn set_general(&mut self, general: GeneralOptions, ctx: &mut ViewContext<Self>) {
        if general == self.settings.general() {
            return;
        }

        self.settings.set_general(general);
        self.usage.update(ctx, |model, ctx| {
            model.set_wanted(general.show_usage_chip, ctx);
        });
        // The shells already running keep the startup they were given; there is
        // no way to read a file into a shell that has drawn its prompt. This
        // decides the next one opened.
        self.terminals
            .update(ctx, |model, _| model.set_shell_login(general.login_shell));
        self.save_settings(ctx);
        ctx.notify();
    }

    /// Starts in a density the command line asked for, without adopting it.
    ///
    /// The density goes into [`Self::options`], where every renderer reads it,
    /// and never into [`Self::settings`], which is what gets written. That is
    /// only half the promise: every menu click writes the *whole* options
    /// snapshot back, so [`Overridden`] is what keeps the override out of the
    /// saves that follow.
    pub fn override_density(&mut self, density: Density, ctx: &mut ViewContext<Self>) {
        self.override_with(ctx, Overridden::DENSITY, |options| {
            options.density = density
        });
    }

    /// Starts in a granularity the command line asked for, without adopting
    /// it. The same contract as [`Self::override_density`].
    pub fn override_granularity(&mut self, granularity: Granularity, ctx: &mut ViewContext<Self>) {
        self.override_with(ctx, Overridden::GRANULARITY, |options| {
            options.granularity = granularity
        });
    }

    /// Puts one option on screen and records that the file did not ask for it.
    fn override_with(
        &mut self,
        ctx: &mut ViewContext<Self>,
        flag: Overridden,
        set: impl FnOnce(&mut TabOptions),
    ) {
        let mut options = self.options;
        set(&mut options);
        if options == self.options {
            // Already what the file says. Nothing to keep out of it, and
            // claiming an override here would suppress a later real choice.
            return;
        }

        self.options = options;
        self.overridden.mark(flag);
        ctx.notify();
    }

    /// The options as the file should hold them.
    ///
    /// Everything the menu wrote, with every overridden option replaced by the
    /// value already on disk — which the `override_*` methods never touched,
    /// precisely so there is something to put back here.
    fn persisted(&self) -> TabOptions {
        if self.overridden.is_empty() {
            return self.options;
        }

        let saved = self.settings.tab_options();
        let mut options = self.options;
        if self.overridden.density {
            options.density = saved.density;
        }
        if self.overridden.granularity {
            options.granularity = saved.granularity;
        }
        options
    }

    /// Where the window's own controls land.
    ///
    /// The one call a renderer makes about window chrome. It answers "how
    /// much" and "which element owes it" together, so no view can get the
    /// second half right on the platform it was written on and wrong on the
    /// other two. Crook draws no controls of its own, so what this reserves is
    /// only ever room for the platform's — the traffic lights on macOS.
    ///
    /// Fullscreen is asked of the window itself on every frame, because macOS
    /// takes the traffic lights away there and the room reserved for them has
    /// to go with them. See [`WindowControls`](crate::window_controls).
    pub(super) fn window_insets(&self) -> LayoutInsets {
        self.control_layout
            .insets(WINDOW_CHROME, self.window.state().fullscreen)
            .split()
    }

    /// Who draws this window's controls.
    ///
    /// [`WINDOW_CHROME`](crate::WINDOW_CHROME), handed out from here so that a
    /// renderer asks the workspace about its window rather than reaching for a
    /// constant halfway down an element tree.
    pub(super) fn window_chrome(&self) -> WindowChrome {
        WINDOW_CHROME
    }

    /// Lays the header out for another platform's window controls, the way
    /// `--controls` asks.
    ///
    /// A way to look at a frame, like `--theme` and `--layout`: it changes
    /// what this build draws, never what it is. Nothing else moves — the
    /// window is still the one the real platform opened, and its own controls
    /// are still wherever that platform put them — so this is for a picture of
    /// a title bar, not for using one.
    pub fn override_control_layout(&mut self, layout: ControlLayout, ctx: &mut ViewContext<Self>) {
        self.control_layout = layout;
        ctx.notify();
    }

    /// Does what the header was asked to do as a title bar.
    ///
    /// Nothing here notifies, and that is not an oversight: none of these
    /// changes anything Crook draws. What they change is the *window*, and the
    /// frame that has to follow — the room macOS's traffic lights give back in
    /// fullscreen — comes back through `Shell`, which watches the window's own
    /// state between frames. Repainting here would draw the state that was
    /// asked for a moment before the window manager decided whether to give
    /// it.
    fn apply_window_action(&self, action: WindowAction) {
        match action {
            WindowAction::Drag => self.window.start_drag(),
            WindowAction::ToggleMaximized => self.window.toggle_maximized(),
            WindowAction::Minimize => self.window.minimize(),
            // The same request the last tab closing makes. There is one way to
            // end the process, and a title bar's close button is not a second
            // one.
            WindowAction::Close => (self.quit)(),
        }
    }

    /// Opens the options menu, for a run that was asked to start with it up.
    pub fn open_options_menu(&mut self, ctx: &mut ViewContext<Self>) {
        if self.menu.open {
            return;
        }
        self.menu.open = true;
        ctx.notify();
    }

    /// Arms the detail card on the first row, for a run that was asked to start
    /// with it up.
    ///
    /// The same path a pointer arriving on the row takes, so a snapshot of it
    /// is a snapshot of the real thing rather than of a second code path.
    pub fn hover_first_row(&mut self, ctx: &mut ViewContext<Self>) {
        let Some((_, pane)) = self.tabs.rows(self.options.granularity).first().copied() else {
            // Unreachable: the strip refuses to empty itself.
            return;
        };
        self.hover_row(pane, true, ctx);
    }

    /// Starts the usage poll chain, if anything is going to show what it
    /// reads. Call once, after the window exists.
    ///
    /// Gated on the same switch the chip is, and gated *here* rather than at
    /// the call site: "nothing displays the reading" and "do not fetch the
    /// reading" have to be one statement, or a build that hides the chip goes
    /// on polling forever because somebody added a second entry point.
    pub fn start_usage_poll(&self, ctx: &mut ViewContext<Self>) {
        let wanted = self.general().show_usage_chip;
        self.usage
            .update(ctx, |model, ctx| model.set_wanted(wanted, ctx));
    }

    /// Opens a shell in every pane, and in every pane opened from now on.
    ///
    /// Call once, after the window exists. Separate from [`Self::new`] for the
    /// reason the poll chains are separate from it: a headless snapshot and a
    /// test render this very view tree, and neither should leave a shell
    /// running to do it.
    pub fn start_terminals(&self, ctx: &mut ViewContext<Self>) {
        let panes = self.open_panes();
        self.terminals
            .update(ctx, |model, ctx| model.start(&panes, ctx));
    }

    /// Says whether shells opened from now on report command boundaries.
    ///
    /// Call before [`Self::start_terminals`]. Off is not a broken state — it
    /// is what a shell Crook has no integration for already does — and it is
    /// what a test asks for when it wants the whole session in one open block
    /// so that a drag across the output has something to select.
    pub fn set_shell_marks(&self, enabled: bool, ctx: &mut ViewContext<Self>) {
        self.terminals
            .update(ctx, |model, _| model.set_shell_marks(enabled));
    }

    /// Says whether shells opened from now on are login shells.
    ///
    /// Normally the setting decides, and it is applied when the window is
    /// built. This is the seam a test uses to stand a shell up in a world it
    /// controls: a login shell reads the startup files of whoever is running
    /// the suite, and a test whose result depends on what a developer's
    /// `~/.zprofile` prints is not a test.
    pub fn set_shell_login(&self, login: bool, ctx: &mut ViewContext<Self>) {
        self.terminals
            .update(ctx, |model, _| model.set_shell_login(login));
    }

    /// Whether the next shell opened will be a login shell.
    pub fn shell_login(&self, app: &AppContext) -> bool {
        self.terminals.as_ref(app).shell_login()
    }

    /// Writes to a pane's shell directly, going round the input field.
    ///
    /// **Not the path a command takes.** A person's line is composed in the
    /// field and sent by Enter — see [`Self::type_into_input`] and the element
    /// that owns it — and `--run` types it exactly that way, keystroke by
    /// keystroke. This is the raw write underneath, and it exists for the tests
    /// that have to put a shell in a state the field cannot ask for: an escape
    /// that takes the alt screen, a command that has to be running already.
    pub fn type_into(&self, pane: PaneId, text: &str, ctx: &mut ViewContext<Self>) {
        self.terminals
            .update(ctx, |model, _| model.type_into(pane, text));
    }

    /// Starts the caret blink. Call once, after the window exists.
    ///
    /// A chain rather than a timer, for the reason the poll chains are one: the
    /// next round is started by the previous one finishing, so there is exactly
    /// one wait outstanding and nothing to cancel. It is one of the things in
    /// the process that park a background worker — see
    /// [`crate::PARKED_WORKERS`] for the list — and, like the others, it is
    /// deliberately not started by a test or by the headless snapshot, both of
    /// which want a frame rather than a heartbeat.
    pub fn start_caret_blink(&self, ctx: &mut ViewContext<Self>) {
        self.blink_caret(ctx);
    }

    /// Waits out one half of the blink and repaints if a caret is on screen.
    fn blink_caret(&self, ctx: &mut ViewContext<Self>) {
        let waiting = ctx
            .background()
            .spawn(async { std::thread::sleep(CARET_PHASE) });

        ctx.spawn(waiting, |workspace, (), ctx| {
            // Only when something would actually be drawn differently. A window
            // whose focused pane is running vim has no caret of its own, and
            // repainting it twice a second for nobody is exactly the kind of
            // idle tick the rest of this application does not have.
            if workspace.shows_a_caret(ctx) {
                ctx.notify();
            }
            workspace.blink_caret(ctx);
        })
        .detach();
    }

    /// Whether any field on screen is drawing a caret.
    fn shows_a_caret(&self, app: &AppContext) -> bool {
        if self.a_popup_is_open() {
            // Except the branch field inside the menu that is up, which is the
            // one caret a popup can carry.
            return self.tab_menu.branch.has_keys();
        }
        let Some(pane) = self.tabs.focused_pane_id() else {
            return false;
        };
        // The settings page's search box is the other field that blinks, and
        // it is the one field that is not a pane's — so it is asked about
        // separately, in the one state it can have the keyboard in.
        if self
            .fields
            .borrow()
            .iter()
            .any(|(_, _, input)| input.has_keys())
        {
            return true;
        }
        self.terminal(pane, app).is_some_and(|(_, snapshot)| {
            pane_surface::of(&snapshot, std::time::Instant::now()).composer
        })
    }

    /// How many pixels a pane last measured along the axis its split divides.
    ///
    /// Written by the element that lays it out and read by the divider beside
    /// it, which is the only way a drag in pixels can become the share the
    /// pane group keeps. A pane with no entry yet reports zero, which is the
    /// state a divider reads as "nothing to drag against".
    pub(super) fn pane_extent(&self, pane: PaneId) -> PaneExtent {
        self.interactions
            .get(&pane)
            .map_or_else(PaneExtent::new, |interaction| interaction.extent.clone())
    }

    /// The divider drag in progress anywhere in the window.
    ///
    /// One for the window rather than one per divider, which is what makes
    /// "only one divider can be dragged at a time" a fact rather than a rule.
    pub(super) fn divider_drag(&self) -> &DividerDrag {
        &self.divider_drag
    }

    /// Where the caret of the field with the keyboard was last painted.
    ///
    /// What the window puts an input method's candidate list beside. `None`
    /// when no field has the keyboard — a full-screen program is up, or the
    /// settings page is the focused pane — and when the caret has been
    /// scrolled out of a field taller than its box.
    pub fn caret_rect(&self) -> Option<RectF> {
        let pane = self.tabs.focused_pane_id()?;
        let input = self.inputs.get(&pane)?;
        input.has_keys().then(|| input.caret_rect()).flatten()
    }

    /// The command line being composed in a pane.
    pub(super) fn input(&self, pane: PaneId) -> Option<&TextInput> {
        self.inputs.get(&pane)
    }

    /// Where a pane's block list is scrolled to, and what the pointer is over.
    pub(super) fn pane_blocks(&self, pane: PaneId) -> Option<&PaneBlocks> {
        self.interactions.get(&pane).map(|state| &state.blocks)
    }

    /// Scrolls a pane's block list by `lines`, positive being further down,
    /// and reports whether anything moved.
    ///
    /// For `--scroll-blocks`: a list scrolled off its own bottom is what draws
    /// the rule above the composer, and no unattended run can turn a wheel.
    pub fn scroll_blocks(&self, pane: PaneId, lines: f32, ctx: &mut ViewContext<Self>) -> bool {
        let Some(view) = self.pane_blocks(pane) else {
            return false;
        };
        let moved = view.apply(crate::pane_blocks::ScrollCause::Wheel(lines));
        ctx.notify();
        moved
    }

    /// Puts the pointer over one finished block, so that its copy control is
    /// drawn, and reports whether there was a block at that index.
    ///
    /// For `--hover-block`, and for the same reason: a hover only exists while
    /// somebody is holding a pointer still.
    pub fn hover_block(&self, pane: PaneId, index: usize, ctx: &mut ViewContext<Self>) -> bool {
        let (Some(view), Some(history)) = (self.pane_blocks(pane), self.terminal_blocks(pane, ctx))
        else {
            return false;
        };
        let Some(block) = history.get(index) else {
            return false;
        };
        view.hover(Some(block.id), false);
        ctx.notify();
        true
    }

    /// The commands that have finished in a pane, oldest first.
    pub(super) fn terminal_blocks(
        &self,
        pane: PaneId,
        app: &AppContext,
    ) -> Option<Arc<BlockHistory>> {
        self.terminals.as_ref(app).blocks(pane)
    }

    /// The system clipboard every field copies to and pastes from.
    pub(crate) fn clipboard(&self) -> &Clipboard {
        &self.clipboard
    }

    /// Types into a pane's field without sending it, as if a person had.
    ///
    /// The other half of [`Self::type_into`], and the one the command line's
    /// `--type` uses: this leaves the text in the field for a picture to be
    /// taken of, where `--run` sends it to the shell.
    pub fn type_into_input(&self, pane: PaneId, text: &str, ctx: &mut ViewContext<Self>) {
        let Some(input) = self.inputs.get(&pane) else {
            log::warn!("nothing to type into: pane {pane:?} has no field");
            return;
        };
        input.edit(|editor| editor.insert(text));
        ctx.notify();
    }

    /// Selects the first occurrence of `text` in a pane's field, reporting
    /// whether it was there to select.
    ///
    /// For `--select`: a selection is a state nobody can be dragging out while
    /// a headless frame is being rendered.
    pub fn select_in_input(&self, pane: PaneId, text: &str, ctx: &mut ViewContext<Self>) -> bool {
        let Some(input) = self.inputs.get(&pane) else {
            return false;
        };

        let found = input.edit(|editor| {
            let Some(at) = editor.text().find(text) else {
                return false;
            };
            editor.set_selection(Selection::new(at, at + text.len()));
            true
        });
        ctx.notify();
        found
    }

    /// What a pane's shell is showing, for a caller that wants the text rather
    /// than the pixels.
    pub fn terminal_text(&self, pane: PaneId, app: &AppContext) -> Option<String> {
        Some(self.terminals.as_ref(app).snapshot(pane)?.text())
    }

    /// What is selected in a pane's output, or `None` when nothing is.
    ///
    /// Resolved against the blocks rather than read out of the emulator: a
    /// selection names a block and a row of it, and all but the newest of
    /// those left the grid when their commands ended.
    ///
    /// Against the space the gesture was made in rather than the surface the
    /// pane is showing now. The two are the same for as long as a selection
    /// lives — a pane that changes surface lets go of it — and asking the
    /// selection is what makes that a fact rather than a hope.
    pub fn terminal_selection(&self, pane: PaneId, app: &AppContext) -> Option<String> {
        let interaction = self.interaction(pane)?;
        let selection = interaction.selection.selection()?;
        let model = self.terminals.as_ref(app);
        let snapshot = model.snapshot(pane)?;

        if interaction.selection.cells() == Cells::Grid {
            let handle = model.handle(pane)?;
            let slack = snapshot.rows;
            let (first, last) = (
                selection
                    .anchor
                    .row
                    .min(selection.head.row)
                    .saturating_sub(slack),
                selection.anchor.row.max(selection.head.row) + slack,
            );
            let (rows, at) = handle.harvest_rows(first, last);
            return selection.text(&Blocks::one(
                selection.anchor.block,
                Rows::Stored(&rows),
                at,
            ));
        }
        let blocks = model.blocks(pane)?;
        selection.text(&Blocks::list(&blocks, &snapshot))
    }

    /// Selects the first occurrence of `text` in a pane's output, reporting
    /// whether the screen was showing it.
    ///
    /// The output's half of [`Self::select_in_input`], and it exists for the
    /// same reason: a selection is a state somebody is holding a button down
    /// to be in, and nobody is holding anything down in a headless run. See
    /// `--select-output`.
    pub fn select_in_output(&self, pane: PaneId, text: &str, ctx: &mut ViewContext<Self>) -> bool {
        self.select_in_output_through(pane, text, text, ctx)
    }

    /// Selects from the first occurrence of `from` to the first occurrence of
    /// `to`, reporting whether the output was showing both.
    ///
    /// Two markers because one string cannot name a region that crosses a
    /// block boundary without spelling out the prompt between them, and a
    /// prompt is whatever `PS1` was. See `--select-through`.
    pub fn select_in_output_through(
        &self,
        pane: PaneId,
        from: &str,
        to: &str,
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let model = self.terminals.as_ref(ctx);
        let (Some(blocks), Some(snapshot)) = (model.blocks(pane), model.snapshot(pane)) else {
            return false;
        };
        let cells = if Self::grid_surface(&snapshot) {
            Cells::Grid
        } else {
            Cells::List
        };
        let addressed = match cells {
            Cells::Grid => Blocks::grid(&snapshot, snapshot.live_block.id),
            Cells::List => Blocks::list(&blocks, &snapshot),
        };
        let Some(found) = addressed.find_through(from, to) else {
            return false;
        };
        let Some(interaction) = self.interaction(pane) else {
            return false;
        };

        interaction.selection.select(found, snapshot.columns, cells);
        ctx.notify();
        true
    }

    /// Lets go of what is selected in a pane's output.
    ///
    /// Dispatched by the grid rather than done where it is decided, and
    /// [`WorkspaceAction::ReleaseSelection`] says why: the grid and the field
    /// under it route one keystroke against one answer, and this is what runs
    /// after both of them have had it.
    fn release_selection(&mut self, pane: PaneId, ctx: &mut ViewContext<Self>) {
        let Some(interaction) = self.interaction(pane) else {
            return;
        };
        if interaction.selection.clear() {
            ctx.notify();
        }
    }

    /// Whether a pane showing `snapshot` draws one grid rather than a list of
    /// blocks, which is the one thing a selection is resolved differently for.
    fn grid_surface(snapshot: &Snapshot) -> bool {
        pane_surface::of(snapshot, std::time::Instant::now()).surface == pane_surface::Surface::Grid
    }

    /// Starts the git gather chain. Call once, after the window exists.
    ///
    /// Separate from [`Self::new`] for the reason the usage poll is: a
    /// headless snapshot and a test render the real view tree without ever
    /// spawning a subprocess or parking a worker thread.
    pub fn start_git_poll(&self, ctx: &mut ViewContext<Self>) {
        let wants_diff = wants_diff_stats(self.options);
        self.git.update(ctx, |model, ctx| {
            model.set_diff_stats_wanted(wants_diff);
            model.start(ctx);
        });
    }

    /// Reports the agent's progress into one session, and repaints the chip
    /// that shows it. Returns whether the pane is still open.
    ///
    /// A closure rather than a returned `&mut AgentSession`, because the
    /// repaint has to be part of the same call: a caller that renamed a
    /// session and did not notify would leave the old title on screen with
    /// nothing reporting an error, until some unrelated click happened to
    /// rebuild the frame.
    ///
    /// Addressed by [`PaneId`], because a session belongs to a pane. Taking a
    /// [`TabId`] would mean writing into whichever pane of that tab happens to
    /// be focused when the agent reports — a race between a person clicking
    /// and a background task finishing.
    pub fn update_session(
        &mut self,
        id: PaneId,
        ctx: &mut ViewContext<Self>,
        report: impl FnOnce(&mut AgentSession),
    ) -> bool {
        let Some(pane) = self.tabs.pane_mut(id) else {
            return false;
        };

        report(pane.session_mut());
        // A report can move the session somewhere else, and the git facts a
        // row shows are looked up by directory — which is what makes the branch
        // chip follow a shell's `cd`.
        self.sync_git(ctx);
        ctx.notify();
        true
    }

    /// Records that a pane's shell rang the bell.
    ///
    /// A bell is a program saying "look at me", so it becomes the one status
    /// that means exactly that — and only in a pane nobody is looking at. The
    /// pane with the keyboard is already being looked at, and a shell that
    /// rings on every ambiguous Tab completion would otherwise paint its own
    /// row amber while somebody typed in it.
    ///
    /// It is cleared by looking: [`Self::attend`] runs on every focus change.
    fn ring(&mut self, pane: PaneId, ctx: &mut ViewContext<Self>) -> bool {
        if self.tabs.focused_pane_id() == Some(pane) {
            // Not "nothing to write into" — the pane is there and the bell was
            // heard. Reporting `true` is what keeps this out of the log line
            // that means a pane has gone.
            return true;
        }
        self.update_session(pane, ctx, |session| {
            session.status = AgentStatus::NeedsInput;
        })
    }

    /// Clears the attention a bell asked for, now that the pane has it.
    ///
    /// Only [`AgentStatus::NeedsInput`] is cleared, and only ever back to
    /// [`AgentStatus::Idle`]: a pane that failed stays failed until something
    /// says otherwise, and looking at a running command does not stop it.
    fn attend(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(pane) = self.tabs.focused_pane_id() else {
            return;
        };
        let rang = self
            .tabs
            .pane(pane)
            .map(Pane::session)
            .is_some_and(|session| session.status == AgentStatus::NeedsInput);
        if !rang {
            return;
        }
        self.update_session(pane, ctx, |session| session.status = AgentStatus::Idle);
    }

    /// Asks a pane's shell what the word before its caret could become.
    ///
    /// The workspace rather than the field, because the question needs the
    /// terminal *model*: the line goes into a file in the session's own
    /// scratch, and only the model knows where that is.
    fn request_completions(&self, pane: PaneId, ctx: &mut ViewContext<Self>) {
        let Some(input) = self.inputs.get(&pane) else {
            return;
        };

        let serial = input.ask_for_completions();
        let line = input.line_to_caret();
        let asked = self.terminals.update(ctx, |model, _| {
            model.request_completions(pane, serial, &line)
        });

        if !asked {
            // A shell with no integration binds nothing, so nothing will ever
            // answer. Saying so once beats a field that looks as though it is
            // thinking.
            log::debug!("pane {pane:?} has no shell that can answer a completion");
        }
        // The list that was showing has gone either way — `ask_for_completions`
        // dropped it — so the frame is worth drawing.
        ctx.notify();
    }

    /// Applies what a pane's shell did.
    ///
    /// A title and a working directory go into the session, which is what makes
    /// the strip's "Command / Conversation" and "Working Directory" say
    /// something true — and, through [`Self::sync_git`], what makes the branch
    /// and diff chips follow a `cd`.
    ///
    /// A shell that ended closes its pane through [`TabAction::ClosePane`],
    /// which is the same path `cmd-w` takes: the pane goes, its tab goes with
    /// it if it was the last pane, and the window goes if that was the last
    /// tab. There is deliberately no second way to close anything.
    pub(super) fn apply_terminal_update(
        &mut self,
        update: &TerminalUpdate,
        ctx: &mut ViewContext<Self>,
    ) {
        let reported = match update {
            TerminalUpdate::Title(pane, title) => {
                let title = title.clone();
                self.update_session(*pane, ctx, |session| session.derived_title = title)
            }
            TerminalUpdate::WorkingDirectory(pane, directory) => {
                let directory = directory.clone();
                self.update_session(*pane, ctx, |session| {
                    session.working_directory = Some(directory);
                })
            }
            TerminalUpdate::Closed(pane) => {
                if self.apply(TabAction::ClosePane(*pane), ctx) == TabEffect::CloseWindow {
                    (self.quit)();
                }
                true
            }
            // Straight onto the window's one clipboard, which is the same one
            // `cmd-c` writes: a program that asked for its text to be copied
            // means the clipboard a person will paste from, not a second one.
            TerminalUpdate::ClipboardStore(_, text) => {
                if !self.clipboard.write(text) {
                    log::debug!("a shell asked to write the clipboard, and there is none");
                }
                true
            }
            TerminalUpdate::Bell(pane) => self.ring(*pane, ctx),
            TerminalUpdate::Completions(pane, serial, answer) => {
                let Some(input) = self.inputs.get(pane) else {
                    return;
                };
                if input.take_completions(*serial, answer.clone()) {
                    ctx.notify();
                }
                true
            }
        };

        if !reported {
            // The pane closed between the shell saying something and the main
            // thread hearing it. Nothing to write it into, and nothing wrong.
            log::debug!("a terminal reported {update:?} for a pane that has gone");
        }
    }

    /// Applies a tab action, brings the per-tab mouse state back in step, and
    /// repaints if the strip actually moved.
    ///
    /// Every mutation of the strip goes through here, which is what lets the
    /// interaction map be keyed by identity without ever leaking an entry for
    /// a tab that has been closed — and what makes "the strip changed" and
    /// "the window is dirty" the same statement rather than two.
    pub fn apply(&mut self, action: TabAction, ctx: &mut ViewContext<Self>) -> TabEffect {
        let effect = self.tabs.apply(action);
        self.settle(effect, ctx)
    }

    /// Opens a tab whose shell starts in `directory`.
    ///
    /// The directory is written onto the new session *before* the shells are
    /// synced, and that ordering is the whole of this function. Syncing is the
    /// moment a pty's working directory is decided, and it is the only moment
    /// it can be: Crook records where a shell says it is and never drives it,
    /// so a directory set afterwards would relabel the row while the shell sat
    /// in the old place. It is the same order [`crate::session`] restores in.
    pub fn open_tab_in(&mut self, directory: PathBuf, ctx: &mut ViewContext<Self>) -> TabEffect {
        let effect = self.tabs.apply(TabAction::New);

        // `New` inserts after the active tab and makes it active, so the
        // focused pane is the one it just made.
        if let Some(pane) = self.tabs.focused_pane_id()
            && let Some(pane) = self.tabs.pane_mut(pane)
        {
            pane.session_mut().working_directory = Some(directory);
        }

        self.settle(effect, ctx)
    }

    /// Opens a pane whose shell starts in `directory`, inside `tab`.
    ///
    /// What the worktree menu opens into. A worktree opened from a tab belongs
    /// *with* that tab: it is the same repository, one checkout over, and the
    /// panel says so by drawing the two under one group header — which is a
    /// header that appears the moment the second pane arrives, so there is no
    /// group to make first and none to tidy away when one of them closes. The
    /// alternative, a tab of its own, puts the branch somewhere else in the
    /// list with nothing left to say where it came from.
    ///
    /// [`TabAction::Split`] is about the active tab, so the tab named here
    /// becomes the active one first. That is not a workaround: a person who
    /// asked a tab for a worktree is about to be looking at it, and the split
    /// focuses what it made.
    ///
    /// A tab that closed while git was checking the worktree out gets the tab
    /// this used to open every time. The checkout happened and it is still
    /// what was asked for; only the place to put it has gone.
    pub fn open_pane_in(
        &mut self,
        tab: TabId,
        directory: PathBuf,
        ctx: &mut ViewContext<Self>,
    ) -> TabEffect {
        if self.tabs.get(tab).is_none() {
            return self.open_tab_in(directory, ctx);
        }

        self.tabs.apply(TabAction::Select(tab));
        let effect = self.tabs.apply(TabAction::Split(Direction::Right));

        // The directory before the shells are synced, for the reason
        // `open_tab_in` writes it there: syncing is the moment a pty's
        // working directory is decided.
        if let Some(pane) = self.tabs.focused_pane_id()
            && let Some(pane) = self.tabs.pane_mut(pane)
        {
            pane.session_mut().working_directory = Some(directory);
        }

        self.settle(effect, ctx)
    }

    /// Everything that happens after the strip has moved, whatever moved it.
    fn settle(&mut self, effect: TabEffect, ctx: &mut ViewContext<Self>) -> TabEffect {
        self.sync_interactions();
        self.sync_git(ctx);
        self.sync_terminals(ctx);
        // After the strip has moved, so "which pane is being looked at" is the
        // answer for the state the frame is about to draw. Every action comes
        // through here, which is what makes looking at a pane the one and only
        // thing that quiets its bell — and what brings the row it selected
        // into view whichever gesture selected it.
        self.attend(ctx);
        self.scroll_row_into_view();

        // An action that changed nothing repaints nothing: holding down
        // cmd-alt-left on the leftmost tab must not put the window on a
        // key-repeat render loop — and writes no session file either.
        if effect == TabEffect::Changed {
            self.save_session(ctx);
            ctx.notify();
        }
        effect
    }

    /// What a keystroke means, if it means anything.
    ///
    /// Bindings produce exactly the values the mouse produces — there is no
    /// second code path for the keyboard, which is what stops a shortcut from
    /// drifting away from the button beside it. That is also why this returns
    /// a [`WorkspaceAction`] rather than a [`TabAction`]: one binding moves
    /// the tabs themselves rather than a tab, and giving it its own dispatch
    /// path would be exactly the second code path.
    pub fn action_for(&self, keystroke: &Keystroke) -> Option<WorkspaceAction> {
        // **The panel takes the keyboard while it is up.** Before the bindings
        // and before anything a pane would see: Warp's chooser moves its
        // selection with the arrow keys, and a keystroke that both moved the
        // selection and recalled a line of shell history would be worse than
        // either. `sync_input_keys` closes the other half of the same door by
        // taking the keyboard away from the focused pane's field.
        if self.panel.open
            && let Some(action) = self.panel_action_for(keystroke)
        {
            return Some(action);
        }

        // A plugin's floating surface, while one is up. Before the bindings
        // for the same reason the panel is: a palette that is showing owns its
        // arrow keys, and a keystroke that both moved its selection and did
        // something to the window would be worse than either. A surface that
        // does not claim this keystroke lets it fall through, so `cmd-t` still
        // opens a tab over an open palette.
        if let Some(action) = self.host.keys_for(keystroke) {
            return Some(WorkspaceAction::Run(action));
        }

        // **The search box owns its two ways out while it is being typed
        // into**, and it owns them here rather than in the field because the
        // field has nowhere to hand the keyboard back to: see
        // [`tabs_panel::search`]. Before the bindings, like the panel above —
        // and after them nothing would be left, since Escape and Enter are
        // nobody's chord and would fall through to the pane's shell.
        if self.search_takes_keys()
            && let Some(action) = self.search_action_for(keystroke)
        {
            return Some(action);
        }

        // The person's own table first, and only where it has something to
        // say: a chord it does not mention keeps Crook's binding, and one it
        // binds to nothing has none at all — which is how a chord is given
        // back to a shell or an editor that wants it. Nothing here reaches
        // what a *pane* does with a key; see `crate::keymap`.
        let bound = match self.keymap.binding(keystroke) {
            Some(binding) => binding?,
            None => match input_keys::binding(keystroke, Platform::current()) {
                Some(binding) => Bound::Builtin(binding),
                // Last, so that a plugin cannot take a chord from the window
                // or from the person's own file by loading first.
                None => {
                    return self.host.suggested_for(keystroke).map(WorkspaceAction::Run);
                }
            },
        };

        match bound {
            Bound::Builtin(binding) => self.command(binding),
            // A plugin's action, resolved now rather than when the file was
            // read: which plugins are loaded is a question with a different
            // answer at every moment. A name nothing answers to is a chord
            // that does nothing, and is not passed on to the pane — a person
            // who bound a chord meant to take it away from the shell.
            Bound::Named(name) => self.host.action(&name).map(WorkspaceAction::Run),
        }
    }

    /// What one of Crook's own commands does, right now.
    ///
    /// Split out of [`Self::action_for`] so that the keyboard is not the only
    /// way to reach it: `crook/window` registers every one of these under a
    /// name, and its handlers come back through here. One implementation, two
    /// entry points — a palette entry and a chord cannot drift apart, which is
    /// the same rule the mouse and the keyboard already follow.
    ///
    /// `None` where the command does not apply — closing a pane when there is
    /// no focused one. From a chord that means the keystroke goes on to the
    /// shell, which is what a person pressing a chord the window has no use
    /// for expects.
    pub fn command(&self, binding: Binding) -> Option<WorkspaceAction> {
        let tab = match binding {
            Binding::NewTab => TabAction::New,
            // Warp's `pane_group:close_current_session`: the pane goes, and
            // the tab only goes with it when it was the tab's last one.
            Binding::ClosePane => TabAction::ClosePane(self.tabs.focused_pane_id()?),
            Binding::SplitRight => TabAction::Split(Direction::Right),
            Binding::SplitDown => TabAction::Split(Direction::Down),
            Binding::PreviousTab => TabAction::Select(self.neighbour(-1)?),
            Binding::NextTab => TabAction::Select(self.neighbour(1)?),
            Binding::MoveTabLeft => TabAction::MoveLeft,
            Binding::MoveTabRight => TabAction::MoveRight,
            // Telegram's chord for Telegram's box, and it works from anywhere
            // in the window — including from a section that is not the tabs,
            // which is what makes it one gesture rather than two. See
            // `apply_search`.
            Binding::SearchTabs => return Some(SearchAction::Focus.into()),
            // The sidebar chord every editor uses for the same gesture: move
            // the list of things you are working on out of the way, or back.
            // The binding every application on all three platforms uses for
            // this, and the one Warp binds `ShowSettings` to. A second press
            // navigates to the settings rather than toggling them away, which
            // is what a person pressing it twice means.
            Binding::OpenSettings => {
                return Some(WorkspaceAction::ShowSection(
                    self.host.sidebar_section_id(SETTINGS_SECTION),
                ));
            }
            // The zoom chords are not tab actions: they change the font the
            // whole window is drawn in, and the size lives in the settings
            // beside the theme.
            Binding::ZoomIn => {
                return Some(
                    SettingsAction::SetFontSize(self.general().zoomed(FONT_SIZE_STEP)).into(),
                );
            }
            Binding::ZoomOut => {
                return Some(
                    SettingsAction::SetFontSize(self.general().zoomed(-FONT_SIZE_STEP)).into(),
                );
            }
            Binding::ZoomReset => {
                return Some(SettingsAction::SetFontSize(DEFAULT_FONT_SIZE).into());
            }
        };

        Some(WorkspaceAction::Tab(tab))
    }

    /// What a keystroke means to the search box, if it means anything.
    ///
    /// Two keys, both unmodified, and neither of them is a chord: every other
    /// key typed while the box has the keyboard is the box's own and is left
    /// to it. Escape and Enter are taken away from it because both end with
    /// the keyboard somewhere else, which is a thing an element cannot do.
    fn search_action_for(&self, keystroke: &Keystroke) -> Option<WorkspaceAction> {
        if !keystroke.modifiers.is_empty() {
            return None;
        }

        let action = match keystroke.key.as_str() {
            "escape" => SearchAction::Dismiss,
            "enter" => SearchAction::Accept,
            _ => return None,
        };
        Some(action.into())
    }

    /// What a keystroke means to the Themes panel, if it means anything.
    ///
    /// Only unmodified keys, and only the four the panel actually uses: a
    /// chord is a window command wherever the pointer is, and swallowing one
    /// here would make `cmd/ctrl-t` stop opening a tab while a panel happened
    /// to be showing.
    fn panel_action_for(&self, keystroke: &Keystroke) -> Option<WorkspaceAction> {
        if !keystroke.modifiers.is_empty() {
            return None;
        }

        let action = match keystroke.key.as_str() {
            "up" => ThemeAction::MoveSelection(-1),
            "down" => ThemeAction::MoveSelection(1),
            // Enter and Escape both close it. Nothing is uncommitted — moving
            // the selection has already applied and saved, as it does in Warp
            // — so "confirm" and "dismiss" are the same gesture with two keys,
            // and the creator's Escape is the one that puts something back.
            "enter" | "escape" => ThemeAction::ClosePanel,
            _ => return None,
        };
        Some(WorkspaceAction::Theme(action))
    }

    pub(super) fn menu(&self) -> &MenuState {
        &self.menu
    }

    pub(super) fn new_tab_state(&self) -> MouseStateHandle {
        self.new_tab.clone()
    }

    pub(super) fn interaction(&self, id: PaneId) -> Option<&PaneInteraction> {
        self.interactions.get(&id)
    }

    /// What the mouse is doing to one tab's chrome in the panel.
    pub(super) fn tab_chrome(&self, id: TabId) -> Option<&TabInteraction> {
        self.tab_chrome.get(&id)
    }

    /// Whether this row should be showing its detail card.
    ///
    /// Never while the options menu is up. Warp tears its sidecar down when
    /// the row's tab opens a menu, and here it also keeps an invariant the
    /// overlay layers depend on: the menu is anchored inside the control bar,
    /// which paints *before* the list, so a card opened from a row afterwards
    /// would land in a later overlay layer and cover the menu's own modal
    /// underlay — the press that should dismiss the menu would be swallowed by
    /// the card instead.
    pub(super) fn shows_details_for(&self, pane: PaneId) -> bool {
        self.options.show_details_on_hover
            && !self.a_popup_is_open()
            && self.hovered_row == Some(pane)
    }

    /// The home directory every row abbreviates its path against.
    pub(super) fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// The terminal running in a pane, and the grid it is showing.
    ///
    /// Both or neither: an element that had a handle and no snapshot would have
    /// nothing to paint, and one that had a snapshot and no handle could not
    /// resize the pty it was measuring.
    pub(super) fn terminal(
        &self,
        pane: PaneId,
        app: &AppContext,
    ) -> Option<(TerminalHandle, Arc<Snapshot>)> {
        let terminals = self.terminals.as_ref(app);
        Some((terminals.handle(pane)?, terminals.snapshot(pane)?))
    }

    /// Why a pane has no terminal, when the attempt failed rather than never
    /// having been made.
    pub(super) fn terminal_failure<'a>(
        &self,
        pane: PaneId,
        app: &'a AppContext,
    ) -> Option<&'a str> {
        self.terminals.as_ref(app).failure(pane)
    }

    /// What is known about the repository a session sits in.
    ///
    /// A map lookup on a model the background pool fills in. Nothing on the
    /// render path walks a directory tree or spawns `git`.
    pub(super) fn git_facts<'a>(
        &self,
        session: &AgentSession,
        app: &'a AppContext,
    ) -> Option<&'a GitFacts> {
        self.git
            .as_ref(app)
            .facts(session.working_directory.as_deref()?)
    }

    /// The tab `offset` slots away from the active one, wrapping at both ends.
    fn neighbour(&self, offset: isize) -> Option<TabId> {
        let count = self.tabs.len() as isize;
        let index = self.tabs.index_of(self.tabs.active_id())? as isize;
        let wrapped = (index + offset).rem_euclid(count) as usize;
        self.tabs.iter().nth(wrapped).map(Tab::id)
    }

    fn sync_interactions(&mut self) {
        let tabs: Vec<TabId> = self.tabs.iter().map(Tab::id).collect();
        for id in &tabs {
            self.tab_chrome.entry(*id).or_default();
        }
        // A closed tab's entry would otherwise outlive it, and the next tab to
        // reuse nothing at all would still be paying for the map.
        self.tab_chrome.retain(|id, _| tabs.contains(id));

        let open: Vec<PaneId> = self.tabs.panes().map(|(_, pane)| pane.id()).collect();

        for id in &open {
            self.interactions
                .entry(*id)
                .or_insert_with(|| PaneInteraction {
                    chip: MouseStateHandle::default(),
                    close: MouseStateHandle::default(),
                    body: MouseStateHandle::default(),
                    selection: PaneSelection::new(),
                    links: PaneLink::new(),
                    extent: PaneExtent::new(),
                    blocks: PaneBlocks::new(),
                });
            self.inputs.entry(*id).or_default();
        }
        self.interactions.retain(|id, _| open.contains(id));
        // A closed pane's half-written command line goes with it. Keeping it
        // would mean a later pane inheriting somebody else's history the first
        // time an id was reused.
        self.inputs.retain(|id, input| {
            // The element tree that drew this field is still holding a clone
            // of it, and a keystroke queued behind the close would land in an
            // editor nothing can draw or read back — and Enter would write the
            // line to a shell that has already ended. Taking the keyboard away
            // is what the tree cannot work out for itself.
            let open = open.contains(id);
            if !open {
                input.set_has_keys(false);
            }
            open
        });
        // A menu is open *on a tab*, and nothing else clears it when that tab
        // goes: `tab_menu.tab` is the open flag, so a tab closed under an open
        // menu leaves a popup nothing can dismiss — and with it a window where
        // `a_popup_is_open` is true forever, which is a window where no pane
        // ever gets the keyboard again. This is the same reaping the line
        // below does for a hover card anchored to a row that no longer paints.
        if self
            .tab_menu
            .tab
            .is_some_and(|tab| self.tabs.get(tab).is_none())
        {
            self.tab_menu.tab = None;
            self.tab_menu.mode = super::tab_menu::Mode::Listing;
            self.tab_menu.contents = Contents::Reading;
            self.tab_menu.problem = None;
            self.tab_menu.working = false;
            self.tab_menu.forget_hover_state();
        }

        self.sync_input_keys();

        // The query lives exactly as long as the section it filters. Which
        // page the rail has selected deliberately outlives it — coming back to
        // the settings comes back to where you were — but a filter must not: a
        // settings page that came back showing four rows out of thirty would
        // read as broken rather than as filtered, and the box that explains
        // why is at the top of a rail somebody has to look at.
        if self.section.is_none() {
            self.clear_fields();
        }

        // A row that has gone cannot receive the hover-out that would clear
        // this, and a card anchored to a row that no longer paints would hang
        // in the frame with nothing under it.
        if self
            .hovered_row
            .is_some_and(|hovered| !open.contains(&hovered))
        {
            self.hovered_row = None;
        }
    }

    /// Tells every field whether the keyboard is its.
    ///
    /// Called wherever focus could have moved — a pane focused, closed or
    /// split, a tab selected, the modal menu opened — because the element that
    /// asks is a frame behind: it was built before whatever moved the focus,
    /// and by the time a keystroke reaches it, what it was told is history.
    ///
    /// Reachable from a plugin, because a plugin's surface going up or down is
    /// one of the things that moves the focus and nothing else would say so.
    pub fn sync_input_keys(&self) {
        // The menu is modal and the Themes panel owns the arrow keys, so
        // neither leaves the keyboard with a pane.
        // A plugin's surface counts exactly as an open menu does: while a
        // palette is up nothing under it is typing into a shell.
        // The search box counts exactly as an open menu does, and for the same
        // reason: while a person is typing into it they are not typing into a
        // shell. It is the only one of these that is on screen *beside* a pane
        // rather than over it, which is why it is a wish that has to be
        // granted rather than a surface that is simply up.
        let listening = (!self.a_popup_is_open()
            && !self.panel.open
            && !self.host.a_surface_is_up()
            && !self.search_takes_keys())
        .then(|| self.tabs.focused_pane_id())
        .flatten();
        for (id, input) in &self.inputs {
            input.set_has_keys(Some(*id) == listening);
        }

        // The settings page's search box, which is the one field that is not a
        // pane's. It has the keyboard whenever the focused pane is the page it
        // is part of, which is Warp's rule — there the search field is what
        // the settings pane focuses when it opens — and it is the only rule
        // available: there is nothing else on that page that takes a key, so
        // "focus is somewhere else on the page" is not a state that exists.
        //
        // Note which question this asks. `listening` is the *focused* pane,
        // and the settings page draws no field of its own through `inputs`, so
        // the two never both have the keyboard.
        // The worktree menu's branch field, which is the other keyboard a
        // popup can hold and the only one that is not a pane's or the settings
        // page's.
        self.tab_menu
            .branch
            .set_has_keys(self.tab_menu.mode == WorktreeMode::Creating);

        // The panel's own box, which is not one of the section fields below:
        // see `search_takes_keys` for why it is asked a different question.
        self.panel_search
            .input()
            .set_has_keys(self.search_takes_keys());

        // A section's fields take the keyboard while that section is showing,
        // which is also when no pane is: the sidebar's sections replace the
        // panes rather than sitting beside them. Exactly one of them is being
        // typed into — see `field_with_keys`.
        let showing = self.section.clone();
        let focused = self.field_with_keys();
        for (section, name, input) in self.fields.borrow().iter() {
            input.set_has_keys(
                showing.as_deref() == Some(section.as_str()) && focused.as_deref() == Some(name),
            );
        }
    }

    /// Tells the git model which directories the strip is showing.
    fn sync_git(&mut self, ctx: &mut ViewContext<Self>) {
        let directories: Vec<PathBuf> = self
            .tabs
            .panes()
            .filter_map(|(_, pane)| pane.session().working_directory.clone())
            .collect();
        self.git
            .update(ctx, |model, ctx| model.track(directories, ctx));
    }

    /// Opens a shell for every pane that has none, and ends the ones whose
    /// panes have gone.
    ///
    /// Called from [`Self::apply`] like [`Self::sync_git`], so that "a pane
    /// exists" and "a shell is running in it" are one statement rather than two
    /// that can disagree. Before [`Self::start_terminals`] it opens nothing.
    fn sync_terminals(&mut self, ctx: &mut ViewContext<Self>) {
        let panes = self.open_panes();
        self.terminals
            .update(ctx, |model, ctx| model.sync(&panes, ctx));
    }

    /// The open panes that want a shell, and where each of them is working.
    fn open_panes(&self) -> Vec<(PaneId, Option<PathBuf>)> {
        self.tabs
            .panes()
            .map(|(_, pane)| (pane.id(), pane.session().working_directory.clone()))
            .collect()
    }

    /// Applies one option, or opens and closes the menu.
    fn apply_option(&mut self, action: OptionsAction, ctx: &mut ViewContext<Self>) {
        let mut options = self.options;
        // Choosing an option is a real choice even when it names the value
        // already on screen, which is the one case `set_options` cannot see:
        // `--density expanded` followed by a click on "Expanded" changes no
        // value, and without this the file would go on saying Compact.
        let chosen = Overridden {
            density: matches!(action, OptionsAction::SetDensity(_)),
            granularity: matches!(action, OptionsAction::SetGranularity(_)),
        };

        match action {
            OptionsAction::TogglePopup => {
                self.menu.open = !self.menu.open;
                // The menu freezes the window under it, so a row that was
                // hovered when it opened would never see its hover-out and
                // would keep a card on screen behind the menu.
                if self.menu.open {
                    self.hovered_row = None;
                }
                // The menu is modal: while it is up the field takes nothing.
                self.sync_input_keys();
                ctx.notify();
                return;
            }
            OptionsAction::SetGranularity(granularity) => options.granularity = granularity,
            OptionsAction::SetDensity(density) => options.density = density,
            OptionsAction::SetPrimaryInfo(primary_info) => options.primary_info = primary_info,
            OptionsAction::SetSubtitle(subtitle) => options.subtitle = subtitle,
            OptionsAction::ToggleShowPrLink => options.show_pr_link = !options.show_pr_link,
            OptionsAction::ToggleShowDiffStats => {
                options.show_diff_stats = !options.show_diff_stats;
            }
            OptionsAction::ToggleShowDetailsOnHover => {
                options.show_details_on_hover = !options.show_details_on_hover;
            }
        }

        if self.overridden.clear(chosen) && options == self.options {
            // `set_options` would return before writing anything, and the
            // choice would be lost. One save, here, and nothing repaints
            // because nothing on screen moved.
            //
            // Through [`Self::persisted`] like every other write, and for the
            // same reason: `clear` above ended the override on the option that
            // was just chosen, and every *other* command-line override is
            // still in force. Writing `options` verbatim would adopt those
            // into the file, permanently — the flag stays set, so every later
            // save reads the poisoned value back out and writes it again.
            self.settings.set_tab_options(self.persisted());
            self.save_settings(ctx);
            return;
        }

        // Deliberately not closing the menu. Warp's popup is a persistent
        // preferences panel — change the title field, turn two chips off, and
        // only then click away.
        self.set_options(options, ctx);
    }

    /// Empties the panel's search box and gives the keyboard back.
    ///
    /// What [`SearchAction::Dismiss`] does, reached from the other direction:
    /// there, a person said they were finished; here, they did something that
    /// says it. Silent when there was no search on, so the ordinary business
    /// of the strip costs nothing and asks for no frame of its own — the
    /// action that called this is about to ask for one.
    fn stop_searching(&mut self) {
        if !self.panel_search.is_focused() && self.panel_search.is_empty() {
            return;
        }
        self.panel_search.clear();
        self.panel_search.set_focused(false);
        self.sync_input_keys();
    }

    /// Moves the keyboard into the panel's search box, or out of it.
    ///
    /// Both ways out empty the box, and that is the rule the whole feature
    /// hangs on: the query lives for exactly as long as a person is looking
    /// for something. Nothing in here touches the strip's own state — the
    /// active tab, the focused pane — except the one `Select` that is the
    /// point of `Accept`.
    fn apply_search(&mut self, action: SearchAction, ctx: &mut ViewContext<Self>) {
        match action {
            SearchAction::Focus => {
                // The chord can arrive from another section, and the box is
                // not drawn there. Showing the tabs first is what makes one
                // press enough; from a click it is already true and
                // `show_section` returns having done nothing.
                self.show_section(None, ctx);
                self.panel_search.set_focused(true);
            }
            SearchAction::Dismiss => {
                self.panel_search.clear();
                self.panel_search.set_focused(false);
            }
            SearchAction::Accept => {
                // Read before the box is emptied, because emptying it is what
                // decides there is no match at all.
                let chosen = tabs_panel::search::first_match(self, ctx);
                self.panel_search.clear();
                self.panel_search.set_focused(false);
                if let Some(tab) = chosen {
                    // Through the ordinary path, so Enter on a match is the
                    // click on that row it is meant to stand in for — the same
                    // scroll, the same focus, the same save.
                    self.apply(TabAction::Select(tab), ctx);
                }
            }
        }

        self.sync_input_keys();
        ctx.notify();
    }

    /// Switches the page the rail has selected, or does one of the two things
    /// only the settings page can do.
    ///
    /// Opening and closing are not here: those are [`TabAction::OpenSettings`]
    /// and the ordinary close of a pane, because the page is a pane. Every
    /// other control on it dispatches an [`OptionsAction`] and lands in
    /// [`Self::apply_option`] beside the gear menu's clicks, which is why this
    /// handles three actions rather than fifteen.
    fn apply_settings(&mut self, action: SettingsAction, ctx: &mut ViewContext<Self>) {
        match action {
            SettingsAction::Select(page) => {
                let key = self.host.settings_page_key(page).map(str::to_owned);
                if self.page.page == key {
                    return;
                }
                self.page.page = key;
                // The fields belong to the page that drew them, and the next
                // page may have none at all.
                // A page is a different set of controls at a different set of
                // positions. Both of the things that survive a section change
                // would otherwise be wrong: the scroll offset belongs to the
                // page that was showing, and every control the pointer was
                // over is about to stop existing without a hover-out.
                self.page.scroll.lock().scroll_to_top();
                self.page.forget_hover_state();
                ctx.notify();
            }
            SettingsAction::FocusField(field) => {
                let Some(index) = field else {
                    return;
                };
                self.focus_field(index);
                self.sync_input_keys();
                ctx.notify();
            }
            SettingsAction::ToggleUsageChip => {
                let mut general = self.general();
                general.show_usage_chip = !general.show_usage_chip;
                self.set_general(general, ctx);
            }
            SettingsAction::ResetTabOptions => {
                // Through `set_options` like every other write, so the reset
                // ends the command line's overrides exactly as clicking each
                // control by hand would.
                self.set_options(TabOptions::default(), ctx);
            }
            SettingsAction::SetFontSize(size) => self.set_font_size(size, ctx),
            SettingsAction::ToggleFollowSystemTheme => {
                let follow = !self.general().use_system_theme;
                self.set_follow_system_theme(follow, ctx);
            }
            SettingsAction::ToggleLoginShell => {
                let mut general = self.general();
                general.login_shell = !general.login_shell;
                self.set_general(general, ctx);
            }
            SettingsAction::ToggleRestoreSession => {
                let mut general = self.general();
                general.restore_session = !general.restore_session;
                self.set_general(general, ctx);
                // Turning it on writes the file now rather than at the next
                // tab action, so a person who switches it on and closes the
                // window gets what they asked for.
                self.save_session(ctx);
            }
        }
    }

    /// Sets the terminal's type size and re-measures every grid in the window.
    ///
    /// The size is a fact about the *font*, not about a pane, so the whole
    /// window changes at once — which is also why the ptys follow without
    /// anything here telling them: a pane's columns and rows are its box
    /// divided by a cell, so the next layout measures a different grid and
    /// `PaneSizer` reports it.
    ///
    /// A font that will not re-measure at the new size leaves the old one in
    /// place. That is a family whose metrics have gone — a font uninstalled
    /// mid-session — and the honest answer to it is the size that was working
    /// a moment ago rather than a window that stops drawing.
    pub fn set_font_size(&mut self, size: f32, ctx: &mut ViewContext<Self>) {
        let mut general = self.general();
        general.font_size = size;
        let size = general.font_size();
        if (self.cell_font.font_size() - size).abs() < f32::EPSILON {
            return;
        }

        match self.cell_font.resized(size) {
            Ok(font) => self.cell_font = font,
            Err(error) => {
                log::warn!("could not set the terminal font to {size}: {error:#}");
                return;
            }
        }
        // After the font, because the save is what makes the size outlive the
        // process and there is no point remembering one that could not be
        // applied.
        self.set_general(general, ctx);
    }

    /// Everything the Themes panel does.
    fn apply_theme_action(&mut self, action: ThemeAction, ctx: &mut ViewContext<Self>) {
        match action {
            ThemeAction::OpenPanel => {
                // Opening is also the moment to re-read the folder: a theme
                // dropped in while Crook was running is in the list.
                self.refresh_themes();
                if self.panel.open && self.panel.mode == Mode::Choosing {
                    // Already showing the list. Still worth the refresh above,
                    // and the selection below, but nothing else moves.
                    self.select_theme_in_force();
                    self.scroll_selection_into_view();
                    ctx.notify();
                    return;
                }
                // A creator that was up is *cancelled*, not abandoned: its
                // draft is painted on the window, and leaving it there would
                // mean a palette nobody chose, that nothing can name and no
                // file holds.
                self.cancel_draft();
                self.panel.open = true;
                // Warp's chooser opens on the theme you are in. A panel that
                // opened at the top of the list with the selection somewhere
                // else would move a person's theme the first time they pressed
                // Down.
                self.select_theme_in_force();
                self.scroll_selection_into_view();
                self.panel.forget_hover_state();
                self.sync_input_keys();
                self.watch_themes(ctx);
                ctx.notify();
            }
            ThemeAction::ClosePanel => {
                if !self.panel.open {
                    return;
                }
                // A creator left open is cancelled rather than kept: the draft
                // is on screen, and a panel that came back holding a theme
                // nobody had chosen would be applying it.
                self.cancel_draft();
                self.panel.open = false;
                self.panel.forget_hover_state();
                self.sync_input_keys();
                ctx.notify();
            }
            ThemeAction::Choose(index) => {
                let Some(name) = self.themes.get(index).map(|theme| theme.name.clone()) else {
                    log::error!("the themes panel asked for {index}, which is not in its list");
                    return;
                };
                self.panel.selected = index;
                self.set_theme(&name, ctx);
                // Clicking a row does not scroll, but arrowing onto one does,
                // and both go through here so the two cannot drift.
                self.scroll_selection_into_view();
                ctx.notify();
            }
            ThemeAction::MoveSelection(delta) => {
                // Not while the creator is up: the list is not on screen, and
                // a key that quietly chose and *saved* a theme behind it would
                // leave the settings file naming a theme the window is not in.
                if self.panel.mode == Mode::Creating || self.themes.is_empty() {
                    return;
                }
                let last = self.themes.len() as isize - 1;
                let next = (self.panel.selected as isize + delta).clamp(0, last) as usize;
                if next == self.panel.selected {
                    return;
                }
                self.apply_theme_action(ThemeAction::Choose(next), ctx);
            }
            ThemeAction::StartCreating => {
                // The theme *in force*, which is not always the saved one:
                // under `--theme` they differ, and cancelling back to the
                // saved one would change the window rather than restore it.
                self.theme_before_draft = Some(self.theme_name().to_owned());
                let draft = Draft::new(&theme());
                crate::theme::set_theme(draft.theme);
                self.panel.draft = Some(draft);
                self.panel.mode = Mode::Creating;
                self.panel.forget_hover_state();
                self.sync_palette(ctx);
                ctx.notify();
            }
            ThemeAction::CancelCreating => {
                self.cancel_draft();
                self.sync_palette(ctx);
                ctx.notify();
            }
            ThemeAction::PickBackground(index) => {
                let Some(draft) = self.panel.draft.as_mut() else {
                    return;
                };
                draft.choose(index);
                // Applied to the window rather than to a preview card: a
                // palette is judged against real output, which is the whole
                // reason the creator is in a panel beside a shell.
                crate::theme::set_theme(draft.theme);
                self.sync_palette(ctx);
                ctx.notify();
            }
            ThemeAction::Create => self.save_draft(ctx),
        }
    }

    /// Writes the draft into the themes folder and chooses it.
    fn save_draft(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(draft) = self.panel.draft.clone() else {
            return;
        };
        let Some(directory) = self.themes_directory.clone() else {
            log::warn!("no configuration directory, so a theme cannot be written anywhere");
            return;
        };

        // Named after the theme it was built from, because that is the only
        // thing about it a person did not choose by clicking: Warp fills its
        // name field from the image's file name for the same reason.
        let name = self.unused_theme_name();
        match crate::theme::write_theme(&directory, &name, &draft.theme) {
            Ok(path) => log::info!("wrote {name:?} to {}", path.display()),
            Err(error) => {
                log::warn!("could not write the theme: {error:#}");
                return;
            }
        }

        self.theme_before_draft = None;
        self.panel.draft = None;
        self.panel.mode = Mode::Choosing;
        self.panel.forget_hover_state();
        self.refresh_themes();
        self.set_theme(&name, ctx);
        // After `set_theme`, because `refresh_themes` points the selection at
        // whatever was in force *before* the new theme existed.
        self.select_theme_in_force();
        self.scroll_selection_into_view();
        ctx.notify();
    }

    /// A name for a new theme that nothing on this machine is using.
    fn unused_theme_name(&self) -> String {
        let base = format!("{} variant", self.settings.theme());
        let taken = |candidate: &str| self.themes.iter().any(|theme| theme.name == candidate);

        if !taken(&base) {
            return base;
        }
        for serial in 2.. {
            let candidate = format!("{base} {serial}");
            if !taken(&candidate) {
                return candidate;
            }
        }
        base
    }

    /// Puts back the theme the creator was opened over, if it is open.
    fn cancel_draft(&mut self) {
        if self.panel.draft.take().is_none() {
            return;
        }
        self.panel.mode = Mode::Choosing;
        self.panel.forget_hover_state();

        // Back to the *name* that was in force, through the same lookup a
        // click uses: the draft was never saved, so nothing has to be undone
        // except what is on screen.
        // Looked up in the list this workspace is showing rather than through
        // `theme::named`, which reads the real themes folder: a test points
        // the folder somewhere else, and a cancel that fell back to the
        // machine's own themes would put back a palette from outside the test.
        if let Some(name) = self.theme_before_draft.take() {
            let palette = self
                .themes
                .iter()
                .find(|available| available.name == name)
                .map(|available| available.theme)
                .or_else(|| crate::theme::named(&name));
            if let Some(palette) = palette {
                crate::theme::set_theme(palette);
            }
        }
    }

    /// Keeps the row the keyboard is on inside the list.
    ///
    /// Rows are a fixed height, so this is arithmetic. Warp asks its scrollable
    /// to bring a saved child position into view; `Scrollable` has no such
    /// call, and at a fixed row height it does not need one — which is the
    /// same trade `tabs_panel` names for its own list.
    fn scroll_selection_into_view(&self) {
        let row = super::theme_panel::ROW_HEIGHT;
        let top = self.panel.selected as f32 * row;
        let mut scroll = self.panel.scroll.lock();

        let offset = scroll.offset();
        let viewport = scroll.viewport();
        if top < offset {
            scroll.scroll_to(top);
        } else if top + row > offset + viewport {
            scroll.scroll_to(top + row - viewport);
        }
    }

    /// Takes the options menu down, and forgets what the mouse was doing to
    /// it.
    ///
    /// Called when something other than the gear closes it — today, its own
    /// "Settings…" entry, which navigates away from the strip the menu is
    /// about. Every row is about to stop existing without seeing a hover-out,
    /// and the next time the menu opens the row the pointer happened to be on
    /// would come back lit.
    fn close_menu(&mut self) {
        if !self.menu.open {
            return;
        }
        self.menu.open = false;
        self.menu.forget_hover_state();
    }

    /// Arms or disarms the detail card.
    fn hover_row(&mut self, pane: PaneId, entered: bool, ctx: &mut ViewContext<Self>) {
        // Read live at hover time rather than at render time, which is where
        // Warp reads it: switching the option on puts a card under the pointer
        // on the next movement rather than needing a fresh entry.
        if !self.options.show_details_on_hover {
            return;
        }

        let next = if entered {
            Some(pane)
        } else if self.hovered_row == Some(pane) {
            None
        } else {
            // A leave for a row that is not the one being tracked. The pointer
            // has already arrived somewhere else, and honouring this would
            // take down the card that just came up.
            return;
        };

        if self.hovered_row == next {
            return;
        }
        self.hovered_row = next;
        ctx.notify();
    }

    /// Writes the settings, off the main thread.
    ///
    /// A click must not wait on a directory being created, a file being
    /// written, `fsync` returning and a rename landing — so it does not: the
    /// settings are cheap to clone, and the clone is what the background pool
    /// gets. A save that fails says so in the log and changes nothing on
    /// screen, because the option itself has already been applied.
    ///
    /// Each save carries a complete snapshot and writes through its own
    /// temporary, so two of them racing is never a half-written file — and
    /// [`SaveOrder`] is what decides which of the two the file keeps.
    fn save_settings(&self, ctx: &mut ViewContext<Self>) {
        if self.settings.path().is_none() {
            // An ephemeral run, or a machine with no configuration directory.
            // Both were reported when the settings were loaded.
            return;
        }

        // **Only the last one asked for lands.** Browsing themes with the
        // arrow keys asks for one save per keystroke, each on its own
        // background task, racing the others to the same file — and a click on
        // one option followed by a click on another is the same race with two
        // runners. [`SaveOrder`] is the whole of the answer.
        let asked_at = self.saves.ask();
        let saves = self.saves.clone();
        let settings = self.settings.clone();

        ctx.background()
            .spawn(async move {
                saves.write_if_last(asked_at, || {
                    if let Err(error) = settings.save_blocking() {
                        log::warn!("could not save the settings: {error:#}");
                    }
                });
            })
            .detach();
    }

    /// Writes what the window is showing, so the next one can come back to it.
    ///
    /// **On a change rather than on the way out**, because there is no reliable
    /// way out: a window closed by the window manager, a process killed, a
    /// machine that lost power — none of them runs a shutdown path, and a
    /// session file written only at exit is one that is missing exactly when
    /// somebody wanted it. Every mutation of the strip goes through
    /// [`Self::apply`], which is where this is called from, and those are rare
    /// enough — a tab opened, a pane closed, a split — that a small file per
    /// gesture is not worth debouncing.
    ///
    /// Ordered by the same [`SaveOrder`] the settings save uses, for the same
    /// reason: two gestures a millisecond apart must not race each other to the
    /// file with the earlier one winning.
    fn save_session(&self, ctx: &mut ViewContext<Self>) {
        if !self.general().restore_session {
            return;
        }
        let Some(path) = crate::session::user_session_path() else {
            return;
        };
        // An ephemeral run — a test, the headless snapshot — has nowhere to
        // write its *settings*, and must not write a session file into the
        // real one's place either.
        if self.settings.path().is_none() {
            return;
        }

        let asked_at = self.session_saves.ask();
        let saves = self.session_saves.clone();
        let session = crate::session::Session::of(&self.tabs, self.window_size());

        ctx.background()
            .spawn(async move {
                saves.write_if_last(asked_at, || {
                    if let Err(error) = session.save_blocking(&path) {
                        log::warn!("could not save the session: {error:#}");
                    }
                });
            })
            .detach();
    }

    /// How big the window was when it was last laid out, if it has been.
    fn window_size(&self) -> Option<[f32; 2]> {
        let size = self.window_size.get();
        (size.x() >= 1. && size.y() >= 1.).then(|| [size.x(), size.y()])
    }

    /// Records the window's size, from the frame that is being built.
    ///
    /// Set by the delegate rather than reached for, because the size is an
    /// argument to `build_scene` and nothing in the view tree is told it.
    pub fn window_size_cell(&self) -> Rc<std::cell::Cell<Vector2F>> {
        self.window_size.clone()
    }

    /// Replaces the strip with the one a previous session described.
    ///
    /// Everything the workspace keeps *beside* the strip — mouse states, drag
    /// gestures, scroll offsets, input fields — is keyed by pane id and is
    /// brought back into step by `sync_interactions`, exactly as it is after
    /// any other mutation. Nothing here has to know what that state is.
    pub fn restore(&mut self, strip: TabStrip, ctx: &mut ViewContext<Self>) {
        self.tabs = strip;
        self.sync_interactions();
        self.sync_git(ctx);
        self.sync_input_keys();
        ctx.notify();
    }
}

/// Whether anything on screen would print a diff stat.
///
/// Warp's `needs_git_status_for_chip_ui` rule: do not pay for a subprocess when
/// nothing displays its answer. The hover card counts, because it draws the
/// chips whether or not the row's toggle is on.
fn wants_diff_stats(options: TabOptions) -> bool {
    (options.show_diff_stats && matches!(options.density, Density::Expanded))
        || options.show_details_on_hover
}

impl Entity for Workspace {
    type Event = ();
}

impl View for Workspace {
    fn ui_name() -> &'static str {
        "Workspace"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        // The panel is the full height of the window and the header starts
        // beside it, not above it. That is what puts the panel's control bar
        // in the window's top-left corner — where a client-decorated macOS
        // window draws its traffic lights — and it is why `window_insets` has
        // a `panel_left` at all.
        //
        // What the sidebar holds and what the window holds are one answer,
        // asked once: a section builds both halves together, because its list
        // and its detail are two views of the same state. The tabs are the
        // window's own section and the only one no plugin contributes.
        let (sidebar, body) = match self.showing_section() {
            Some(id) => self
                .host
                .build_sidebar_section(id, self, app)
                .unwrap_or_else(|| (Empty::new().finish(), Empty::new().finish())),
            None => (tabs_panel::tab_list(self, app), body::render(self, app)),
        };

        let main = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(header_toolbar::render(self, app))
            .with_child(Expanded::new(1., body).finish())
            .finish();

        // The Themes panel is a *second* sidebar, docked between the first
        // and the work — Warp's arrangement, and the only one in which the
        // sidebar keeps whatever it was showing while a theme is chosen.
        // Composed here rather than inside a section's branch of the render:
        // that is what makes it the same panel from the tabs, the settings and
        // the plugins, instead of one section's panel and nothing anywhere
        // else.
        let mut content = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(tabs_panel::render(self, sidebar));
        if self.panel.open {
            content.add_child(super::theme_panel::render(self, app));
        }
        content.add_child(Expanded::new(1., main).finish());
        let content = content.finish();

        let window = Container::new(content)
            .with_background_color(theme().ground)
            .finish();

        // Anything a plugin floats over the whole window. Anchored children of
        // a stack whose box *is* the window, so a palette is laid out against
        // the window rather than against whatever it happens to hang off, and
        // painted into an overlay layer above every menu the chrome opened.
        //
        // Built even when there is nothing showing: a contribution that has
        // nothing to say returns `Empty`, which lays out to nothing and paints
        // nothing, and the alternative is the workspace knowing which plugin's
        // surface is up.
        let overlays = self
            .host
            .slots()
            .map(crate::plugins::window::WINDOW_OVERLAY, |build| {
                build(self, app)
            });
        if overlays.is_empty() {
            return window;
        }

        let mut stack = Stack::new().with_child(window);
        for overlay in overlays {
            stack.add_anchored_overlay_child(overlay, OVERLAY_ANCHOR);
        }
        stack.finish()
    }
}

/// Where a plugin's floating surface lands: the window's own top-left corner,
/// with the surface's top-left on it.
///
/// The offset is nothing, so a contribution is placed against the window and
/// puts itself where it wants inside that — which is the only way a surface
/// can be centred, since an anchor's offset is a constant and the window's
/// width is not.
const OVERLAY_ANCHOR: AnchorTo = AnchorTo {
    parent: Corner::TopLeft,
    child: Corner::TopLeft,
    offset: Vector2F::zero(),
    keep_on_screen: false,
    keep_clear_of_parent: false,
};

/// Where Crook keeps the checkouts it makes.
///
/// Its own store, under the user's data directory, and neither of the two
/// places a person would put one by hand. *Inside* the repository is a trap
/// git will not stop you falling into: a checkout under the working tree shows
/// up in `status`, in every build, and in every recursive search of the
/// project. *Beside* it means writing into a directory that belongs to whoever
/// laid the project out, and a tool that scatters siblings around somebody
/// else's `~/Work` is a tool they stop trusting.
///
/// Grouped by repository, so one store serves however many of them a person
/// works on. herdr's `~/.herdr/worktrees/<repo>/<branch>`, in the place this
/// platform keeps data rather than in a dotfile of our own.
fn worktree_store() -> Option<PathBuf> {
    dirs::data_dir().map(|directory| directory.join("crook").join("worktrees"))
}

impl TypedActionView for Workspace {
    type Action = WorkspaceAction;

    fn handle_action(&mut self, action: &WorkspaceAction, ctx: &mut ViewContext<Self>) {
        match *action {
            WorkspaceAction::Tab(action) => {
                // Anything asked of the strip ends the search, and this is the
                // only place that can say so for *every* way of asking: a click
                // on a row, the `+`, a close button, a split, a chord. The
                // person found what they were looking for — or stopped looking
                // — and both mean the same two things. The keyboard is the
                // half that has to happen: a box that kept it after a row was
                // clicked would collect the first command typed into the tab it
                // opened, which is the trap this whole box is arranged around.
                self.stop_searching();
                if self.apply(action, ctx) == TabEffect::CloseWindow {
                    (self.quit)();
                }
            }
            WorkspaceAction::ShowSection(section) => {
                // The menu is a popup about the tab list, and its job is done
                // the moment its own entry puts something else in the sidebar.
                self.close_menu();
                if section
                    .is_some_and(|id| self.host.sidebar_section_key(id) == Some(SETTINGS_SECTION))
                {
                    // One of the two gestures that can precede choosing a
                    // theme — the other is opening the panel — and therefore
                    // one of the two moments worth walking the themes folder.
                    self.refresh_themes();
                }
                self.show_section(section, ctx);
            }
            WorkspaceAction::Options(action) => self.apply_option(action, ctx),
            WorkspaceAction::Settings(action) => self.apply_settings(action, ctx),
            WorkspaceAction::Search(action) => self.apply_search(action, ctx),
            WorkspaceAction::Theme(action) => self.apply_theme_action(action, ctx),
            WorkspaceAction::Window(action) => self.apply_window_action(action),
            WorkspaceAction::Worktree(action) => self.apply_worktree(action, ctx),
            WorkspaceAction::HoverRow { pane, entered } => self.hover_row(pane, entered, ctx),
            WorkspaceAction::ReleaseSelection(pane) => self.release_selection(pane, ctx),
            WorkspaceAction::Complete(pane) => self.request_completions(pane, ctx),
            WorkspaceAction::Run(id) => self.run_action(id, ctx),
        }
    }
}
