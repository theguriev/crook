//! [`Workspace`]: the state behind the window, and the one place it changes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use crookui_core::elements::MouseStateHandle;
use crookui_core::event::{Keystroke, Modifiers};
use crookui_core::fonts::FamilyId;
use crookui_core::prelude::*;

use crate::git::GitFacts;
use crate::git_model::GitModel;
use crate::settings::{Density, Settings, TabOptions};
use crate::tab::{AgentSession, Direction, PaneId, Tab, TabAction, TabEffect, TabId, TabStrip};
use crate::theme::THEME;
use crate::usage_model::UsageModel;

use super::action::{OptionsAction, WorkspaceAction};
use super::usage_chip::UsageChip;
use super::{body, header_toolbar};

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

/// The window's root view.
pub struct Workspace {
    tabs: TabStrip,
    fonts: Fonts,
    usage: ModelHandle<UsageModel>,
    chip: ViewHandle<UsageChip>,
    git: ModelHandle<GitModel>,
    interactions: HashMap<PaneId, PaneInteraction>,
    settings: Settings,
    /// The options, kept beside [`Self::settings`] rather than read out of it
    /// on every access. A renderer reads this dozens of times per frame and
    /// wants a `Copy` snapshot, not a borrow of the thing a save is cloning.
    options: TabOptions,
    /// Whether [`Self::options`]'s density came from the command line rather
    /// than from the file.
    ///
    /// An override is a way to look at a frame, so it must not become what the
    /// next launch does — and every menu click writes the *whole* options
    /// snapshot back, so "never saved" cannot be arranged by simply not saving
    /// it once. [`Self::persisted`] is what keeps the promise; clicking a
    /// density in the menu is a real choice and clears this.
    density_is_overridden: bool,
    menu: MenuState,
    /// The row the pointer is on, if the detail card is armed.
    hovered_row: Option<PaneId>,
    new_tab: MouseStateHandle,
    quit: QuitRequest,
}

impl Workspace {
    /// Builds the workspace, its usage chip, its git model, and one tab to
    /// start in.
    pub fn new(
        fonts: Fonts,
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

        let options = settings.tab_options();
        let mut workspace = Self {
            tabs: TabStrip::new(),
            fonts,
            usage,
            chip,
            git,
            interactions: HashMap::new(),
            settings,
            options,
            density_is_overridden: false,
            menu: MenuState::default(),
            hovered_row: None,
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

        // A density that actually moved is a choice somebody made, and it ends
        // the command line's override: from here on the file learns it.
        if options.density != self.options.density {
            self.density_is_overridden = false;
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
    /// snapshot back, so [`Self::density_is_overridden`] is what keeps the
    /// override out of the saves that follow.
    pub fn override_density(&mut self, density: Density, ctx: &mut ViewContext<Self>) {
        if self.options.density == density {
            // Already what the file says. Nothing to keep out of it, and
            // claiming an override here would suppress a later real choice.
            return;
        }
        self.options.density = density;
        self.density_is_overridden = true;
        ctx.notify();
    }

    /// The options as the file should hold them.
    ///
    /// Everything the menu wrote, with an overridden density replaced by the
    /// one already on disk — which [`Self::override_density`] never touched,
    /// precisely so there is something to put back here.
    fn persisted(&self) -> TabOptions {
        if !self.density_is_overridden {
            return self.options;
        }
        TabOptions {
            density: self.settings.tab_options().density,
            ..self.options
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

    /// Starts the usage poll chain. Call once, after the window exists.
    pub fn start_usage_poll(&self, ctx: &mut ViewContext<Self>) {
        self.usage.update(ctx, |model, ctx| model.start(ctx));
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
        // row shows are looked up by directory.
        self.sync_git(ctx);
        ctx.notify();
        true
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

        // An action that changed nothing repaints nothing: holding down
        // cmd-alt-left on the leftmost tab must not put the window on a
        // key-repeat render loop.
        if effect == TabEffect::Changed {
            ctx.notify();
        }
        effect
    }

    /// The tab action a keystroke means, if it means one.
    ///
    /// Bindings produce exactly the values the mouse produces — there is no
    /// second code path for the keyboard, which is what stops a shortcut from
    /// drifting away from the button beside it.
    pub fn action_for(&self, keystroke: &Keystroke) -> Option<TabAction> {
        if !is_platform_chord(keystroke.modifiers) {
            return None;
        }

        let shift = keystroke.modifiers.shift;
        let alt = keystroke.modifiers.alt;
        match (keystroke.key.as_str(), shift, alt) {
            ("t", false, false) => Some(TabAction::New),
            // Warp's `pane_group:close_current_session`: the pane goes, and
            // the tab only goes with it when it was the tab's last one.
            ("w", false, false) => self.tabs.focused_pane_id().map(TabAction::ClosePane),
            ("d", false, false) => Some(TabAction::Split(Direction::Right)),
            ("d", true, false) => Some(TabAction::Split(Direction::Down)),
            ("left", true, false) => self.neighbour(-1).map(TabAction::Select),
            ("right", true, false) => self.neighbour(1).map(TabAction::Select),
            ("left", false, true) => Some(TabAction::MoveLeft),
            ("right", false, true) => Some(TabAction::MoveRight),
            _ => None,
        }
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

    /// Whether this row should be showing its detail card.
    pub(super) fn shows_details_for(&self, pane: PaneId) -> bool {
        self.options.show_details_on_hover && self.hovered_row == Some(pane)
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

    /// Applies one option, or opens and closes the menu.
    fn apply_option(&mut self, action: OptionsAction, ctx: &mut ViewContext<Self>) {
        let mut options = self.options;
        // Clicking a density is a real choice even when it names the density
        // already on screen, which is the one case `set_options` cannot see:
        // `--density expanded` followed by a click on "Expanded" changes no
        // value, and without this the file would go on saying Compact.
        let adopts_density = matches!(action, OptionsAction::SetDensity(_));

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
        }

        if adopts_density && self.density_is_overridden {
            self.density_is_overridden = false;
            if options == self.options {
                // `set_options` would return before writing anything, and the
                // choice would be lost. One save, here, and nothing repaints
                // because nothing on screen moved.
                self.settings.set_tab_options(options);
                self.save_settings(ctx);
                return;
            }
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
        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Max)
                .with_child(header_toolbar::render(self, app))
                .with_child(Expanded::new(1., body::render(self)).finish())
                .finish(),
        )
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
