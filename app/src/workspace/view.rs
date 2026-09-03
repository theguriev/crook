//! [`Workspace`]: the state behind the window, and the one place it changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use crook_terminal::Snapshot;
use crookui_core::elements::MouseStateHandle;
use crookui_core::event::{Keystroke, Modifiers};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::WINDOW_CHROME;
use crate::git::GitFacts;
use crate::git_model::GitModel;
use crate::platform_insets::{LayoutInsets, TabsPlacement, layout_insets};
use crate::settings::{Density, Granularity, Layout, Settings, TabOptions};
use crate::tab::{AgentSession, Direction, PaneId, Tab, TabAction, TabEffect, TabId, TabStrip};
use crate::terminal_font::CellFont;
use crate::terminal_model::{TerminalHandle, TerminalModel, TerminalUpdate};
use crate::theme::THEME;
use crate::usage_model::UsageModel;

use super::action::{OptionsAction, WorkspaceAction};
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
    interactions: HashMap<PaneId, PaneInteraction>,
    /// What the mouse is doing to each tab's chrome in the panel.
    ///
    /// Keyed by [`TabId`] and separate from [`Self::interactions`] because it
    /// is a tab-level affordance: in `Panes` granularity the container lifts
    /// while the pointer is over *any* of the tab's rows, which is one piece
    /// of state for several rows rather than one per row.
    tab_chrome: HashMap<TabId, TabInteraction>,
    settings: Settings,
    /// The options, kept beside [`Self::settings`] rather than read out of it
    /// on every access. A renderer reads this dozens of times per frame and
    /// wants a `Copy` snapshot, not a borrow of the thing a save is cloning.
    options: TabOptions,
    /// Which of [`Self::options`] came from the command line rather than from
    /// the file. See [`Overridden`].
    overridden: Overridden,
    menu: MenuState,
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
            interactions: HashMap::new(),
            tab_chrome: HashMap::new(),
            settings,
            options,
            overridden: Overridden::default(),
            menu: MenuState::default(),
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

    /// Starts the usage poll chain. Call once, after the window exists.
    pub fn start_usage_poll(&self, ctx: &mut ViewContext<Self>) {
        self.usage.update(ctx, |model, ctx| model.start(ctx));
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

    /// Types into a pane's shell, as if a person had.
    ///
    /// The one way in from outside, and it exists for the command line's
    /// `--run`: a way to prove the whole loop — pty, emulator, reader thread,
    /// repaint — works, in a run nobody is sitting in front of.
    pub fn type_into(&self, pane: PaneId, text: &str, ctx: &mut ViewContext<Self>) {
        self.terminals
            .update(ctx, |model, _| model.type_into(pane, text));
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

        report(pane.session_mut());
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
        if !is_platform_chord(keystroke.modifiers) {
            return None;
        }

        let shift = keystroke.modifiers.shift;
        let alt = keystroke.modifiers.alt;
        let tab = match (keystroke.key.as_str(), shift, alt) {
            ("t", false, false) => TabAction::New,
            // Warp's `pane_group:close_current_session`: the pane goes, and
            // the tab only goes with it when it was the tab's last one.
            ("w", false, false) => TabAction::ClosePane(self.tabs.focused_pane_id()?),
            ("d", false, false) => TabAction::Split(Direction::Right),
            ("d", true, false) => TabAction::Split(Direction::Down),
            ("left", true, false) => TabAction::Select(self.neighbour(-1)?),
            ("right", true, false) => TabAction::Select(self.neighbour(1)?),
            ("left", false, true) => TabAction::MoveLeft,
            ("right", false, true) => TabAction::MoveRight,
            // The sidebar chord every editor uses for the same gesture: move
            // the list of things you are working on out of the way, or back.
            ("b", false, false) => {
                return Some(WorkspaceAction::Options(OptionsAction::ToggleLayout));
            }
            _ => return None,
        };

        Some(WorkspaceAction::Tab(tab))
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
        self.options.show_details_on_hover && !self.menu.open && self.hovered_row == Some(pane)
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
        }
        self.interactions.retain(|id, _| open.contains(id));

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

    /// The open panes and where each of them is working.
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
            layout: matches!(action, OptionsAction::ToggleLayout),
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

        let settings = self.settings.clone();
        ctx.background()
            .spawn(async move {
                if let Err(error) = settings.save_blocking() {
                    log::warn!("could not save the tab options: {error:#}");
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
        let stacked = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(header_toolbar::render(self, app))
            .with_child(Expanded::new(1., body::render(self, app)).finish())
            .finish();

        let content = match self.options.layout {
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
            .with_background_color(THEME.ground)
            .finish()
    }
}

impl TypedActionView for Workspace {
    type Action = WorkspaceAction;

    fn handle_action(&mut self, action: &WorkspaceAction, ctx: &mut ViewContext<Self>) {
        match *action {
            WorkspaceAction::Tab(action) => {
                if self.apply(action, ctx) == TabEffect::CloseWindow {
                    (self.quit)();
                }
            }
            WorkspaceAction::Options(action) => self.apply_option(action, ctx),
            WorkspaceAction::HoverRow { pane, entered } => self.hover_row(pane, entered, ctx),
        }
    }
}

/// Whether these modifiers are the platform's "this is an application command"
/// chord: Command on macOS, Control everywhere else.
fn is_platform_chord(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.cmd && !modifiers.ctrl
    } else {
        modifiers.ctrl && !modifiers.cmd
    }
}
