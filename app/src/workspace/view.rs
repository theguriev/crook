//! [`Workspace`]: the state behind the window, and the one place it changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crook_terminal::Snapshot;
use crookui_core::elements::MouseStateHandle;
use crookui_core::event::Keystroke;
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::clipboard::Clipboard;
use crate::editor::Selection;
use crate::git::GitFacts;
use crate::git_model::GitModel;
use crate::input_keys::{self, Binding, Platform};
use crate::pane_input::{CARET_PHASE, PaneInput};
use crate::platform_insets::{LayoutInsets, TabsPlacement, layout_insets};
use crate::settings::{Density, GeneralOptions, Granularity, Layout, Settings, TabOptions};
use crate::tab::{AgentSession, Direction, PaneId, Tab, TabAction, TabEffect, TabId, TabStrip};
use crate::terminal_font::CellFont;
use crate::terminal_model::{TerminalHandle, TerminalModel, TerminalUpdate};
use crate::theme::creator::Draft;
use crate::theme::{Available, theme};
use crate::usage_model::UsageModel;
use crate::{Channel, WINDOW_CHROME};

use super::action::{OptionsAction, SettingsAction, ThemeAction, WorkspaceAction};
use super::settings_page::{Section, SettingsState};
use super::theme_panel::{Mode, ThemePanelState};
use super::usage_chip::UsageChip;
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
    layout: bool,
}

impl Overridden {
    /// Only the density.
    const DENSITY: Self = Self {
        density: true,
        granularity: false,
        layout: false,
    };
    /// Only the granularity.
    const GRANULARITY: Self = Self {
        density: false,
        granularity: true,
        layout: false,
    };
    /// Only the layout.
    const LAYOUT: Self = Self {
        density: false,
        granularity: false,
        layout: true,
    };

    /// Whether nothing at all is overridden.
    fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// Marks every flag `also` marks, leaving the rest alone.
    fn mark(&mut self, also: Self) {
        self.density |= also.density;
        self.granularity |= also.granularity;
        self.layout |= also.layout;
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
        self.layout &= !chosen.layout;
        *self != before
    }
}

/// The window's root view.
pub struct Workspace {
    tabs: TabStrip,
    fonts: Fonts,
    /// The faces and the cell every pane's grid is drawn with, resolved once at
    /// startup because measuring one is a search through the font database.
    cell_font: CellFont,
    usage: ModelHandle<UsageModel>,
    chip: ViewHandle<UsageChip>,
    git: ModelHandle<GitModel>,
    /// The shells behind the panes.
    terminals: ModelHandle<TerminalModel>,
    /// The command line being composed in each pane.
    ///
    /// One per pane and never one per tab: a split gives the new pane a field
    /// of its own, with its own undo stack and its own history, which is what
    /// makes two panes of the same tab two places to work rather than one.
    inputs: HashMap<PaneId, PaneInput>,
    /// The system clipboard every field copies to and pastes from. One for the
    /// window: see [`Clipboard`].
    clipboard: Clipboard,
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
    /// How many settings saves have been asked for.
    ///
    /// Each save task carries the number it was asked at and does nothing if a
    /// later one has been asked for since. Browsing themes with the arrow keys
    /// asks for one per keystroke, which is exactly the case `save_settings`
    /// said it would need this for.
    ///
    /// An `Arc<AtomicU64>` rather than a `Cell`, because the check happens on
    /// the worker that is about to write.
    save_generation: Arc<AtomicU64>,
    /// How far the tabs panel's list has been scrolled.
    ///
    /// On the workspace rather than inside the panel module for the reason
    /// every mouse state is: the element tree is rebuilt on every render, and
    /// a scroll offset that lived in it would snap back to the top on the
    /// frame the scroll itself caused.
    panel_scroll: ScrollStateHandle,
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
}

impl Workspace {
    /// Builds the workspace, its usage chip, its git model, and one tab to
    /// start in.
    pub fn new(
        fonts: Fonts,
        cell_font: CellFont,
        settings: Settings,
        channel: Channel,
        quit: QuitRequest,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let usage = UsageModel::handle(ctx);

        // The canonical model-changed-so-repaint bridge. The chip observes the
        // same model for itself; this is what keeps the header honest when the
        // reading changes the chip's width and the row around it has to be
        // laid out again.
        ctx.observe(&usage, |_, _, ctx| ctx.notify());

        let chip = ctx.add_view(|ctx| UsageChip::new(fonts, ctx));

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

        let options = settings.tab_options();
        let mut workspace = Self {
            tabs: TabStrip::new(),
            fonts,
            cell_font,
            usage,
            chip,
            git,
            terminals,
            inputs: HashMap::new(),
            clipboard: Clipboard::new(),
            interactions: HashMap::new(),
            tab_chrome: HashMap::new(),
            settings,
            channel,
            options,
            overridden: Overridden::default(),
            menu: MenuState::default(),
            page: SettingsState::default(),
            panel: ThemePanelState::default(),
            themes: crate::theme::available(),
            theme_before_draft: None,
            themes_directory: crate::theme::user_themes_directory(),
            save_generation: Arc::new(AtomicU64::new(0)),
            panel_scroll: ScrollStateHandle::default(),
            hovered_row: None,
            home: std::env::home_dir(),
            new_tab: MouseStateHandle::default(),
            quit,
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
        self.save_settings(ctx);
        self.sync_palette(ctx);
        ctx.notify();
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
    pub(super) fn usage(&self) -> &ModelHandle<UsageModel> {
        &self.usage
    }

    /// The settings page's state.
    pub(super) fn settings_page(&self) -> &SettingsState {
        &self.page
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

    /// Which page of the settings the rail has selected.
    ///
    /// Public for the snapshot path and the tests: which page is showing is
    /// not something a renderer asks for — it reads
    /// [`Self::settings_page`] — and it is the one piece of the page's state
    /// worth asserting from outside.
    pub fn settings_section(&self) -> Section {
        self.page.section
    }

    /// How far the tabs panel's list has been scrolled.
    pub(super) fn panel_scroll(&self) -> ScrollStateHandle {
        self.panel_scroll.clone()
    }

    /// Whether the settings page is open — which is to say, whether a pane is
    /// holding it.
    ///
    /// Asked of the strip rather than of a flag beside it: the pane *is* the
    /// page, and a second answer kept here would be one more thing to keep
    /// true through every close.
    pub fn is_settings_page_open(&self) -> bool {
        self.tabs.settings_pane().is_some()
    }

    /// Opens the settings page at `section`, for a run that was asked to start
    /// on it.
    ///
    /// The same two steps a click on the menu entry and a click on the rail
    /// take, in that order, so a snapshot of the page is a snapshot of the
    /// real thing rather than of a second code path.
    pub fn open_settings_page(&mut self, section: Section, ctx: &mut ViewContext<Self>) {
        self.apply_settings(SettingsAction::Select(section), ctx);
        if self.apply(TabAction::OpenSettings, ctx) == TabEffect::CloseWindow {
            // Unreachable: opening a tab never empties the strip.
            (self.quit)();
        }
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
            layout: options.layout != self.options.layout,
        });

        if options.layout != self.options.layout {
            // The whole window is about to be rebuilt somewhere else, so every
            // control the pointer was on is about to stop existing without
            // ever seeing a hover-out. Left alone, a row that was hovered in
            // the strip comes back hovered in the panel with the pointer
            // nowhere near it — the same trap the close button and the info
            // dot each close for themselves.
            self.forget_hover_state();
        }

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
    /// job that one does not: the usage chip's switch is also the poll's, so
    /// the model is told before the file is written. A person who turns the
    /// chip off has said they do not want Crook talking to the network, and
    /// waiting for a background save to land before acting on that would be
    /// the wrong order to do two things in.
    fn set_general(&mut self, general: GeneralOptions, ctx: &mut ViewContext<Self>) {
        if general == self.settings.general() {
            return;
        }

        self.settings.set_general(general);
        self.usage.update(ctx, |model, ctx| {
            model.set_wanted(general.show_usage_chip, ctx);
        });
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

    /// Starts in a layout the command line asked for, without adopting it.
    ///
    /// The same contract again, and the one that matters most: `--layout
    /// horizontal` is how somebody looks at the strip once, and it must not
    /// quietly become the layout their next launch opens in.
    pub fn override_layout(&mut self, layout: Layout, ctx: &mut ViewContext<Self>) {
        self.override_with(ctx, Overridden::LAYOUT, |options| options.layout = layout);
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

        if options.layout != self.options.layout {
            self.forget_hover_state();
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
        if self.overridden.layout {
            options.layout = saved.layout;
        }
        options
    }

    /// Where the window's own controls land, given where the tabs are.
    ///
    /// The one call a renderer makes about window chrome. It answers "how
    /// much" and "which element owes it" together, so no view can get the
    /// second half right on the platform it was written on and wrong on the
    /// other two.
    ///
    /// Fullscreen is `false` because the windowing layer exposes no way to
    /// enter it and no way to ask — and under native chrome the answer is the
    /// same either way.
    pub(super) fn window_insets(&self) -> LayoutInsets {
        let placement = match self.options.layout {
            Layout::Vertical => TabsPlacement::LeftPanel,
            Layout::Horizontal => TabsPlacement::Header,
        };
        layout_insets(placement, WINDOW_CHROME, false)
    }

    /// Drops every mouse state the frame about to be replaced was holding.
    ///
    /// Called when the layout changes, which is the one edit that throws away
    /// the whole element tree rather than a row of it.
    fn forget_hover_state(&mut self) {
        self.hovered_row = None;
        for interaction in self.interactions.values() {
            interaction.chip.lock().reset_interaction_state();
            interaction.close.lock().reset_interaction_state();
            interaction.body.lock().reset_interaction_state();
        }
        for chrome in self.tab_chrome.values() {
            chrome.container.lock().reset_interaction_state();
            chrome.header.lock().reset_interaction_state();
        }
        self.new_tab.lock().reset_interaction_state();
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
    /// one wait outstanding and nothing to cancel. It is the third thing in the
    /// process that parks a background worker — see [`crate::PARKED_WORKERS`] —
    /// and, like the other two, it is deliberately not started by a test or by
    /// the headless snapshot, both of which want a frame rather than a
    /// heartbeat.
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
        if self.menu.open {
            return false;
        }
        let Some(pane) = self.tabs.focused_pane_id() else {
            return false;
        };
        self.terminal(pane, app)
            .is_some_and(|(_, snapshot)| input_keys::shows_input(snapshot.alt_screen))
    }

    /// The command line being composed in a pane.
    pub(super) fn input(&self, pane: PaneId) -> Option<&PaneInput> {
        self.inputs.get(&pane)
    }

    /// The system clipboard every field copies to and pastes from.
    pub(super) fn clipboard(&self) -> &Clipboard {
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

        // `None` when the pane holds the settings page rather than a session.
        // A report addressed to it is a report for a session that has been
        // closed, and it fails the same way: nothing written, `false`
        // returned.
        let Some(session) = pane.session_mut() else {
            return false;
        };
        report(session);
        // A report can move the session somewhere else, and the git facts a
        // row shows are looked up by directory — which is what makes the branch
        // chip follow a shell's `cd`.
        self.sync_git(ctx);
        ctx.notify();
        true
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
    fn apply_terminal_update(&mut self, update: &TerminalUpdate, ctx: &mut ViewContext<Self>) {
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
        // Opening the settings is one of the two gestures that can precede
        // choosing a theme — the other is opening the panel — and therefore
        // one of the two moments worth walking the themes directory. Here
        // rather than in the action handler, because every way of opening the
        // page comes through this one call.
        if action == TabAction::OpenSettings {
            self.refresh_themes();
        }

        let effect = self.tabs.apply(action);
        self.sync_interactions();
        self.sync_git(ctx);
        self.sync_terminals(ctx);

        // An action that changed nothing repaints nothing: holding down
        // cmd-alt-left on the leftmost tab must not put the window on a
        // key-repeat render loop.
        if effect == TabEffect::Changed {
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

        let tab = match input_keys::binding(keystroke, Platform::current())? {
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
            // The sidebar chord every editor uses for the same gesture: move
            // the list of things you are working on out of the way, or back.
            Binding::ToggleLayout => {
                return Some(WorkspaceAction::Options(OptionsAction::ToggleLayout));
            }
            // The binding every application on all three platforms uses for
            // this, and the one Warp binds `ShowSettings` to. A tab action
            // rather than a settings one, because what it opens is a tab —
            // and because a second press must navigate to the page rather
            // than toggle it away, which is what `OpenSettings` does and a
            // toggle could not.
            Binding::OpenSettings => TabAction::OpenSettings,
        };

        Some(WorkspaceAction::Tab(tab))
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

    pub(super) fn chip(&self) -> &ViewHandle<UsageChip> {
        &self.chip
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
            && !self.menu.open
            && self.hovered_row == Some(pane)
            // The card says what a row had no room for, and the settings row
            // has nothing behind its one line. `detail_panes` drops the pane
            // as well; without this the card would still open, empty.
            && self.tabs.pane(pane).is_some_and(|pane| !pane.is_settings())
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
        self.sync_input_keys();

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
    fn sync_input_keys(&self) {
        // The menu is modal and the Themes panel owns the arrow keys, so
        // neither leaves the keyboard with a pane.
        let listening = (!self.menu.open && !self.panel.open)
            .then(|| self.tabs.focused_pane_id())
            .flatten();
        for (id, input) in &self.inputs {
            input.set_has_keys(Some(*id) == listening);
        }
    }

    /// Tells the git model which directories the strip is showing.
    fn sync_git(&mut self, ctx: &mut ViewContext<Self>) {
        let directories: Vec<PathBuf> = self
            .tabs
            .panes()
            .filter_map(|(_, pane)| pane.session()?.working_directory.clone())
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
    ///
    /// The settings pane is not one of them. Nothing draws a grid for it and
    /// nothing can type into it, so a shell opened here would be a process
    /// running for a pane that cannot show it — started when the page opens,
    /// killed when the tab closes, and visible to nobody in between.
    fn open_panes(&self) -> Vec<(PaneId, Option<PathBuf>)> {
        self.tabs
            .panes()
            .filter(|(_, pane)| !pane.is_settings())
            .map(|(_, pane)| {
                (
                    pane.id(),
                    pane.session()
                        .and_then(|session| session.working_directory.clone()),
                )
            })
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
            layout: matches!(
                action,
                OptionsAction::ToggleLayout | OptionsAction::SetLayout(_)
            ),
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
            OptionsAction::ToggleLayout => options.layout = options.layout.toggled(),
            OptionsAction::SetLayout(layout) => options.layout = layout,
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
            SettingsAction::Select(section) => {
                if self.page.section == section {
                    return;
                }
                self.page.section = section;
                // A page is a different set of controls at a different set of
                // positions. Both of the things that survive a section change
                // would otherwise be wrong: the scroll offset belongs to the
                // page that was showing, and every control the pointer was
                // over is about to stop existing without a hover-out.
                self.page.scroll.lock().scroll_to_top();
                self.page.forget_hover_state();
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
        }
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
    /// temporary, so two of them racing is a question of which lands last
    /// rather than of a half-written file. Two clicks a millisecond apart could
    /// in principle land out of order and persist the earlier state; the fix
    /// for that is one save task the model owns rather than one per click, and
    /// it is not worth the machinery until an option can be changed from
    /// somewhere other than a person's hand.
    fn save_settings(&self, ctx: &mut ViewContext<Self>) {
        if self.settings.path().is_none() {
            // An ephemeral run, or a machine with no configuration directory.
            // Both were reported when the settings were loaded.
            return;
        }

        // **Only the last one asked for lands.** This function's own comment
        // used to say that two clicks a millisecond apart could persist the
        // earlier state, and that the fix was worth having when an option
        // could be changed from somewhere other than a person's hand. Browsing
        // themes with the arrow keys is that: one save per keystroke, each on
        // its own background task, racing each other to the same file. A
        // generation number makes the race decidable — a task that finds a
        // later one has been asked for since simply does nothing.
        let generation = self.save_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let latest = self.save_generation.clone();
        let settings = self.settings.clone();

        ctx.background()
            .spawn(async move {
                // Checked on the worker, immediately before the write: a task
                // that has been overtaken has nothing to do, and the one that
                // was asked for last is the one holding the state a person can
                // see.
                if latest.load(Ordering::Relaxed) != generation {
                    return;
                }
                if let Err(error) = settings.save_blocking() {
                    log::warn!("could not save the settings: {error:#}");
                }
            })
            .detach();
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
        // The header and the body, which sit one above the other in both
        // layouts. What changes is whether the header holds the tabs.
        //
        // The Themes panel goes *between* the tabs and the work in both, which
        // is where Warp puts its chooser: a docked sibling that pushes the
        // terminal aside rather than a modal that covers it, so a theme is
        // judged against a running shell.
        let stacked = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(header_toolbar::render(self, app))
            .with_child(Expanded::new(1., self.beside_panel(body::render(self, app), app)).finish())
            .finish();

        let content = match self.options.layout {
            // With the tabs in a strip the header spans the window, so the
            // panel sits beside the body under it — otherwise it would push
            // the strip sideways and take the window's top-left corner from
            // it.
            Layout::Horizontal => stacked,
            // The panel is the full height of the window and the header starts
            // beside it, not above it. That is what puts the panel's control
            // bar in the window's top-left corner — where a client-decorated
            // macOS window draws its traffic lights — and it is why
            // `window_insets` has a `panel_left` at all.
            Layout::Vertical => Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                .with_child(tabs_panel::render(self, app))
                .with_child(Expanded::new(1., stacked).finish())
                .finish(),
        };

        Container::new(content)
            .with_background_color(theme().ground)
            .finish()
    }
}

impl Workspace {
    /// `work` with the Themes panel beside it, when the panel is up.
    ///
    /// One place, called from the one point both layouts share, so the panel
    /// cannot end up on a different side of the window depending on where the
    /// tabs are.
    fn beside_panel(&self, work: Box<dyn Element>, app: &AppContext) -> Box<dyn Element> {
        if !self.panel.open {
            return work;
        }

        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(super::theme_panel::render(self, app))
            .with_child(Expanded::new(1., work).finish())
            .finish()
    }
}

impl TypedActionView for Workspace {
    type Action = WorkspaceAction;

    fn handle_action(&mut self, action: &WorkspaceAction, ctx: &mut ViewContext<Self>) {
        match *action {
            WorkspaceAction::Tab(action) => {
                // The one tab action the options menu itself dispatches, and
                // the menu's job is done the moment it does: it is a popup
                // about the strip, and this puts a page over the body.
                // The menu is a popup about the strip, and its job is done
                // the moment its own entry puts a page over the body.
                if action == TabAction::OpenSettings {
                    self.close_menu();
                }
                if self.apply(action, ctx) == TabEffect::CloseWindow {
                    (self.quit)();
                }
            }
            WorkspaceAction::Options(action) => self.apply_option(action, ctx),
            WorkspaceAction::Settings(action) => self.apply_settings(action, ctx),
            WorkspaceAction::Theme(action) => self.apply_theme_action(action, ctx),
            WorkspaceAction::HoverRow { pane, entered } => self.hover_row(pane, entered, ctx),
        }
    }
}
