//! The header, driven through a real presenter.
//!
//! These run the actual view tree — `Workspace::render`, layout, paint, hit
//! testing — against a stub shaper, so they cover the wiring a unit test of
//! the strip cannot: that a click lands on the tab under it, that the close
//! button closes its own tab, and that revealing that button does not move
//! the bar out from under the cursor.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crookui_core::event::{Event, Keystroke, Modifiers, MouseButton, ScrollDelta};
use crookui_core::executor::{Background, LocalQueue};
use crookui_core::fonts::{FamilyId, FontId, LineStyle, StyleAndFont};
use crookui_core::geometry::{RectF, Vector2F, vec2f};
use crookui_core::icons::Mark;
use crookui_core::platform::TextLayoutSystem;
use crookui_core::prelude::*;
use crookui_core::scene::{CornerRadius, Radius, Rect, Scene};
use crookui_core::text_layout::{Glyph, Line, Run};
use crookui_core::{App, Presenter, WindowId};

use crook_plugin::ActionName;

use crate::Channel;
use crate::git::{DiffStats, GitFacts, Head};
use crate::keybindings::Recording;
use crate::platform_insets::{ControlLayout, WindowChrome};
use crate::settings::{
    Density, GeneralOptions, Granularity, PrimaryInfo, Settings, Subtitle, TabOptions,
};
use crate::tab::{
    AgentSession, AgentStatus, Direction, GroupId, Pane, PaneId, Tab, TabAction, TabId,
};
use crate::terminal_font::{CELL_FONT_SIZE, CellFont};
use crate::theme::theme;
use crate::window_controls::{Recorder, Request, WindowState};

use super::{
    BlockAction, Fonts, Opening, OptionsAction, QuitRequest, SettingsAction, TabMenuAction,
    ThemeAction, Workspace, WorkspaceAction, WorktreeAction, tab_options_menu, tabs_panel,
};

/// Big enough that two tabs both reach their maximum width, so the geometry
/// the assertions below reason about is not a division of leftovers.
const WINDOW: Vector2F = vec2f(1024., 640.);

/// A shaper with no fonts: every character is a square half its font size.
/// Real metrics come from a real backend; what these tests need is only that
/// widths add up.
struct StubShaper;

impl TextLayoutSystem for StubShaper {
    fn layout_line(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        _: f32,
    ) -> Line {
        let advance = line_style.font_size * 0.5;
        let glyphs: Vec<_> = text
            .char_indices()
            .enumerate()
            .map(|(position, (index, character))| Glyph {
                id: character as u32,
                position_along_baseline: vec2f(position as f32 * advance, 0.),
                index,
                width: advance,
            })
            .collect();
        let width = glyphs.len() as f32 * advance;

        Line {
            width,
            runs: vec![Run {
                font_id: FontId(0),
                glyphs,
                styles: style_runs
                    .first()
                    .map(|(_, style_and_font)| style_and_font.style)
                    .unwrap_or_default(),
                width,
            }],
            font_size: line_style.font_size,
            line_height_ratio: line_style.line_height_ratio,
            baseline_ratio: line_style.baseline_ratio,
            ascent: line_style.font_size * 0.8,
            descent: line_style.font_size * 0.2,
        }
    }
}

/// The fetcher most tests want: a refusal, so what a test can see is the
/// frame between a press and the answer.
fn no_network() -> crate::plugins::store::model::Fetcher {
    Arc::new(|_| Err(String::from("a test reaches no network")))
}

struct Harness {
    /// The queue standing in for the event loop. Kept, rather than dropped into
    /// the executor, because a terminal's output comes home on it: a test with
    /// a shell in it has to pump the queue by hand.
    queue: Arc<LocalQueue>,
    app: App,
    presenter: Presenter,
    window_id: WindowId,
    workspace: ViewHandle<Workspace>,
    quit_requests: Rc<Cell<usize>>,
    /// The window the header is the title bar of, which only remembers what it
    /// was asked. A drag leaves no mark on a frame, so this is the only thing
    /// that can be looked at afterwards.
    window: Rc<Recorder>,
    /// The store's own state, when the window carries a store — the only
    /// way a test reaches the store's model.
    store: Option<Rc<crate::plugins::store::StoreState>>,
}

impl Harness {
    /// A window with `tabs` tabs, the last of which is active, and one frame
    /// already drawn so there is something to hit-test against.
    ///
    /// Ephemeral: a test run must not read, and must not rewrite, the tab
    /// options of whoever is running it.
    fn new(tabs: usize) -> Self {
        Self::with_settings(tabs, Settings::ephemeral())
    }

    /// The same. Kept as a name because a great many tests say `panel` to mean
    /// "the window Crook opens in", and that is now the only window there is.
    fn panel(tabs: usize) -> Self {
        Self::new(tabs)
    }

    /// The same, on settings that know where they would be written.
    ///
    /// The only way to test what a save would put in the file: `Settings`
    /// carries the path, and `Workspace::save_settings` returns before doing
    /// anything at all when there is none.
    fn with_settings(tabs: usize, settings: Settings) -> Self {
        Self::with_plugins(tabs, settings, crate::plugins::defaults())
    }

    /// The same, with a different set of plugins — which is how a test gives
    /// the window one that is not in the box.
    fn with_plugins(
        tabs: usize,
        settings: Settings,
        plugins: Vec<Box<dyn crate::plugin::Plugin>>,
    ) -> Self {
        Self::with_withdrawn(tabs, settings, plugins, Default::default())
    }

    /// The same, with some of them withdrawn by the registry — which the
    /// window is told about rather than reading, so that a test can say it
    /// without a store, a network or a file.
    ///
    /// With no plugins directory: a window opened this way installs nothing
    /// and removes nothing, and says so. The `sandboxed` tests, which do
    /// both, open theirs on a scratch.
    fn with_withdrawn(
        tabs: usize,
        settings: Settings,
        plugins: Vec<Box<dyn crate::plugin::Plugin>>,
        withdrawn: std::collections::BTreeMap<String, String>,
    ) -> Self {
        Self::with_opening(
            tabs,
            Opening {
                settings,
                channel: Channel::Dev,
                plugins,
                withdrawn,
                heard: Default::default(),
                plugins_directory: None,
            },
        )
    }

    /// The same, with a Store reading `cache` and the window hearing that
    /// cache at the opening, the way a real window hears the cached index at
    /// startup.
    fn with_store(
        tabs: usize,
        opening: Opening,
        cache: crate::plugins::store::cache::Cache,
    ) -> Self {
        Self::with_store_fetching(tabs, opening, cache, no_network())
    }

    /// The same, with the store answering every module fetch with `fetch`
    /// — which is how a test lets a download land, and everything after a
    /// landing run, with no socket opened.
    fn with_store_fetching(
        tabs: usize,
        mut opening: Opening,
        cache: crate::plugins::store::cache::Cache,
        fetch: crate::plugins::store::model::Fetcher,
    ) -> Self {
        use crate::plugins::store::index;

        opening.heard = index::Heard::offered(cache.read().as_ref().map(|cached| &cached.index));
        let state = crate::plugins::store::hermetic(&mut opening.plugins, Some(cache), fetch)
            .expect("the store is among the plugins the window opens with");

        let mut harness = Self::opened(tabs, opening);
        harness.store = Some(state);
        harness
    }

    /// The whole of it: a window opened with exactly this — which is how a
    /// test says what the registry offers, the way the Store would tell the
    /// window, without a network or a file, and where the plugins that are
    /// files live, which is never the directory of whoever is running the
    /// tests.
    ///
    /// Exactly this but for the store. The one in the box reads the real
    /// list of whoever is running the tests, and the first time its model
    /// spoke — a face decoded off that list, a download landing — the
    /// window would hear their offers in place of what the test said. The
    /// store this window carries has read nothing; a test that wants a
    /// registry opens with [`Self::with_store`].
    fn with_opening(tabs: usize, mut opening: Opening) -> Self {
        let state = crate::plugins::store::hermetic(&mut opening.plugins, None, no_network());

        let mut harness = Self::opened(tabs, opening);
        harness.store = state;
        harness
    }

    /// A window opened with `opening` as it stands, store and all.
    fn opened(tabs: usize, opening: Opening) -> Self {
        assert!(
            opening.plugins_directory.as_deref().is_none_or(
                |directory| Some(directory) != crate::plugins::wasm::directory().as_deref()
            ),
            "a test opened a window on the plugins directory of whoever is running it"
        );
        let queue = LocalQueue::new();
        // Two, not one, and for the reason `crate::PARKED_WORKERS` exists: a
        // task that waits on a timer holds its worker for the whole cycle, so
        // a pool of one has nothing left to run anything else on. A harness
        // with a single worker passed for as long as no test started such a
        // chain — and then failed, twenty seconds at a time and in a test
        // about a shell title, the day the terminal model grew one.
        //
        // Two rather than the application's `PARKED_WORKERS + 1`: a test
        // starts at most the model's own chain, and a pool of five per harness
        // is five OS threads per test for workers nothing ever schedules onto.
        let mut app = App::new(queue.foreground(), Arc::new(Background::new(2)));

        let quit_requests = Rc::new(Cell::new(0));
        let quit: QuitRequest = {
            let requests = quit_requests.clone();
            Rc::new(move || requests.set(requests.get() + 1))
        };

        let window = Rc::new(Recorder::default());
        let fonts = Fonts {
            ui: FamilyId(0),
            monospace: FamilyId(0),
        };
        // Measured with no font backend, exactly as the stub shaper above lays
        // text out with none: a cell is half the font size, and a character is
        // its own glyph id.
        let cell_font = CellFont::headless(CELL_FONT_SIZE);
        let (window_id, workspace) = app
            .add_window(|ctx| Workspace::new(fonts, cell_font, opening, quit, window.clone(), ctx));

        let mut harness = Self {
            queue,
            app,
            presenter: Presenter::new(window_id, Arc::new(StubShaper)),
            window_id,
            workspace,
            quit_requests,
            window,
            store: None,
        };

        for _ in 1..tabs {
            harness.dispatch_action(TabAction::New);
        }
        harness.frame();
        harness
    }

    fn frame(&mut self) -> Rc<Scene> {
        self.frame_sized(WINDOW)
    }

    /// The same, in a window of another size.
    ///
    /// The window is a parameter of the frame rather than of the harness
    /// because it is a parameter of `build_scene`: nothing in the workspace
    /// remembers how big the last one was, so a test can draw a wide frame and
    /// a narrow one against the same state.
    fn frame_sized(&mut self, window: Vector2F) -> Rc<Scene> {
        let window_id = self.window_id;
        let presenter = &mut self.presenter;
        self.app.update(|ctx| {
            let invalidation = ctx.take_all_invalidations_for_window(window_id);
            presenter.invalidate(invalidation, ctx);
            presenter.build_scene(window, 1., ctx)
        })
    }

    /// Arms the first row's detail card, the way `--hover` does.
    fn hover_first_row(&mut self) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| workspace.hover_first_row(ctx));
        });
    }

    fn dispatch_action(&mut self, action: TabAction) {
        self.dispatch_workspace_action(WorkspaceAction::Tab(action));
    }

    /// Writes one option, the way a click on the menu does.
    fn dispatch_option(&mut self, action: OptionsAction) {
        self.dispatch_workspace_action(WorkspaceAction::Options(action));
    }

    fn dispatch_workspace_action(&mut self, action: WorkspaceAction) {
        let chain = [self.workspace.id()];
        self.app
            .dispatch_typed_action(self.window_id, &chain, &action);
    }

    fn dispatch(&mut self, event: Event) {
        let window_id = self.window_id;
        let presenter = &mut self.presenter;
        self.app
            .update(|ctx| ctx.dispatch_window_event(window_id, event, presenter));
    }

    fn move_to(&mut self, position: Vector2F) {
        self.dispatch(Event::MouseMoved {
            position,
            modifiers: Modifiers::default(),
            is_synthetic: false,
        });
    }

    fn click(&mut self, position: Vector2F, button: MouseButton) {
        self.dispatch(Event::MouseDown {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
        self.dispatch(Event::MouseUp {
            button,
            position,
            modifiers: Modifiers::default(),
        });
    }

    /// Reports agent progress into a session, the way the agent runtime will.
    fn update_session(&mut self, id: PaneId, report: impl FnOnce(&mut AgentSession)) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                assert!(workspace.update_session(id, ctx, report), "no such pane");
            });
        });
    }

    /// Switches what the bar draws a row for, the way the options menu does.
    fn set_granularity(&mut self, granularity: Granularity) {
        self.dispatch_option(OptionsAction::SetGranularity(granularity));
    }

    /// The options as they stand.
    fn options(&self) -> TabOptions {
        self.workspace
            .read(&self.app, |workspace, _| workspace.options())
    }

    /// Where this layout says the window's own controls land.
    fn window_insets(&self) -> crate::platform_insets::LayoutInsets {
        self.workspace
            .read(&self.app, |workspace, _| workspace.window_insets())
    }

    /// Lays the header out for another platform's window controls, the way
    /// `--controls` does.
    ///
    /// The only way to look at the macOS reservation from anywhere else: the
    /// traffic lights are the one thing still painted over Crook's header, and
    /// a machine that is not a Mac can still lay the room out for them.
    fn override_controls(&mut self, layout: ControlLayout) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                workspace.override_control_layout(layout, ctx)
            });
        });
    }

    /// Puts the window into a state it can only be put into from outside —
    /// the macOS green button, a compositor, a desktop shortcut — and asks for
    /// the frame that follows.
    fn set_window_state(&mut self, state: WindowState) {
        self.window.state.set(state);
        let workspace = &self.workspace;
        self.app
            .update(|ctx| workspace.update(ctx, |_, ctx| ctx.notify()));
    }

    /// What the header has asked of the window.
    fn window_requests(&self) -> Vec<Request> {
        self.window.requests()
    }

    /// One press and release at `position`, as the `count`-th click of a
    /// series.
    fn click_times(&mut self, position: Vector2F, count: u32) {
        self.dispatch(Event::MouseDown {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: count,
        });
        self.dispatch(Event::MouseUp {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
        });
    }

    /// Starts in a density the command line asked for, the way `--density`
    /// does.
    fn override_density(&mut self, density: Density) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                workspace.override_density(density, ctx)
            });
        });
    }

    /// Starts on a granularity the command line asked for, the way
    /// `--granularity` does.
    fn override_granularity(&mut self, granularity: Granularity) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                workspace.override_granularity(granularity, ctx)
            });
        });
    }

    /// The options as they would be written to the file.
    fn saved_options(&self) -> TabOptions {
        self.workspace
            .read(&self.app, |workspace, _| workspace.settings().tab_options())
    }

    fn is_menu_open(&self) -> bool {
        self.workspace
            .read(&self.app, |workspace, _| workspace.is_options_menu_open())
    }

    /// Shows the settings section, the way the sidebar's own button does.
    fn open_settings_page(&mut self) {
        self.show_section(crate::plugins::settings::SETTINGS_SECTION);
    }

    /// Shows the section with this key, the way its button does.
    fn show_section(&mut self, key: &str) {
        let section = self
            .workspace
            .read(&self.app, |workspace, _| {
                workspace.host().sidebar_section_id(key)
            })
            .unwrap_or_else(|| panic!("no sidebar section is called {key:?}"));
        self.dispatch_workspace_action(WorkspaceAction::ShowSection(Some(section)));
        self.frame();
    }

    /// Shows the Plugins section, the way its button does.
    fn show_plugins(&mut self) {
        self.show_section("crook/plugins/section");
    }

    /// Goes back to the tab list.
    fn show_tabs(&mut self) {
        self.dispatch_workspace_action(WorkspaceAction::ShowSection(None));
        self.frame();
    }

    /// Runs `change` against the workspace, the way an action handler does.
    fn workspace_update(
        &mut self,
        change: impl FnOnce(&mut Workspace, &mut ViewContext<Workspace>),
    ) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| change(workspace, ctx));
        });
    }

    /// Opens the Themes panel, the way the settings row does.
    fn open_theme_panel(&mut self) {
        self.dispatch_workspace_action(WorkspaceAction::Theme(ThemeAction::OpenPanel));
    }

    /// Closes it, the way the × does.
    fn close_theme_panel(&mut self) {
        self.dispatch_workspace_action(WorkspaceAction::Theme(ThemeAction::ClosePanel));
    }

    fn is_theme_panel_open(&self) -> bool {
        self.workspace
            .read(&self.app, |workspace, _| workspace.is_theme_panel_open())
    }

    /// Starts making a theme, the way the `+` does.
    fn start_creating(&mut self) {
        self.dispatch_workspace_action(WorkspaceAction::Theme(ThemeAction::StartCreating));
    }

    fn cancel_creating(&mut self) {
        self.dispatch_workspace_action(WorkspaceAction::Theme(ThemeAction::CancelCreating));
    }

    fn create_theme(&mut self) {
        self.dispatch_workspace_action(WorkspaceAction::Theme(ThemeAction::Create));
    }

    fn is_creating(&self) -> bool {
        self.workspace
            .read(&self.app, |workspace, _| workspace.is_creating_theme())
    }

    /// The name of the theme the panel's keyboard row is on.
    fn selected_theme_name(&self) -> String {
        self.workspace.read(&self.app, |workspace, _| {
            let panel = workspace.theme_panel();
            workspace
                .themes()
                .get(panel.selected)
                .map(|theme| theme.name.clone())
                .unwrap_or_default()
        })
    }

    /// Every theme the panel would list.
    fn theme_names(&self) -> Vec<String> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .themes()
                .iter()
                .map(|theme| theme.name.clone())
                .collect()
        })
    }

    /// Points the themes folder at a directory of this test's own.
    fn set_themes_directory(&mut self, directory: PathBuf) {
        self.workspace_update(|workspace, _| workspace.set_themes_directory(directory));
    }

    /// Whether the focused pane's field is listening to the keyboard.
    fn pane_takes_keys(&self) -> bool {
        let Some(pane) = self.focused_pane_id() else {
            return false;
        };
        self.workspace
            .read(&self.app, |workspace, _| workspace.pane_takes_keys(pane))
    }

    /// Pumps the queue until `settled` is true or `patience` runs out.
    ///
    /// For the background chains that have no completion to wait on: a poll
    /// that will re-arm itself for as long as the window is open.
    fn settle_for(
        &mut self,
        patience: std::time::Duration,
        mut settled: impl FnMut(&mut Self) -> bool,
    ) {
        let deadline = std::time::Instant::now() + patience;
        loop {
            self.queue.run_until_parked();
            if settled(self) || std::time::Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The name of the theme in force.
    fn theme_name(&self) -> String {
        self.workspace
            .read(&self.app, |workspace, _| workspace.theme_name().to_owned())
    }

    /// What the rail row of the selected settings page says.
    fn settings_section(&self) -> String {
        self.workspace
            .read(&self.app, |workspace, _| workspace.settings_page_title())
            .unwrap_or_default()
    }

    /// Types `text` into whatever has the keyboard, a key at a time.
    ///
    /// Through `press`, so the bindings get first refusal exactly as they do
    /// in a window — which is the half of "the search box takes typing" worth
    /// asserting.
    fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            self.press(
                &character.to_lowercase().to_string(),
                Modifiers::default(),
                &character.to_string(),
            );
        }
    }

    /// What is in the settings page's search box.
    fn search_text(&self) -> String {
        self.workspace
            .read(&self.app, |workspace, _| workspace.settings_search_text())
    }

    /// What is in the panel's search box.
    fn panel_search_text(&self) -> String {
        self.workspace
            .read(&self.app, |workspace, _| workspace.panel_search_text())
    }

    /// Whether that box, rather than the pane, is what typing reaches.
    fn panel_search_takes_keys(&self) -> bool {
        self.workspace
            .read(&self.app, |workspace, _| workspace.search_takes_keys())
    }

    /// Puts the keyboard in it with the chord, from wherever it is.
    fn press_search_chord(&mut self) {
        self.press("k", platform_chord(), "k");
        self.frame();
    }

    /// Presses the box, which is the other way in.
    fn click_panel_search(&mut self) {
        let box_ = panel_search_box(&self.frame());
        self.click(center(box_), MouseButton::Left);
        self.frame();
    }

    /// Whether the menu is asking about removing a checkout.
    fn worktree_menu_is_confirming(&self) -> bool {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.worktree_menu_is_confirming()
        })
    }

    /// Whether the menu is asking about removing every free checkout.
    fn worktree_menu_is_tidying(&self) -> bool {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.worktree_menu_is_tidying()
        })
    }

    /// How many checkouts that question would take, once it has looked in all
    /// of them.
    fn worktrees_going(&self) -> Option<usize> {
        self.workspace
            .read(&self.app, |workspace, _| workspace.worktrees_going())
    }

    /// How far the sweep has got, while it is looking or removing.
    fn worktrees_swept(&self) -> Option<(usize, usize)> {
        self.workspace
            .read(&self.app, |workspace, _| workspace.worktrees_swept())
    }

    /// Whether the sweep is finishing the one in flight and no more.
    fn worktree_sweep_is_stopping(&self) -> bool {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.worktree_sweep_is_stopping()
        })
    }

    /// Whether the menu is waiting on git.
    fn worktree_menu_is_busy(&self) -> bool {
        self.workspace
            .read(&self.app, |workspace, _| workspace.worktree_menu_is_busy())
    }

    /// How far into his bite the pirate is.
    fn worktree_menu_chomp(&self) -> usize {
        self.workspace
            .read(&self.app, |workspace, _| workspace.worktree_menu_chomp())
    }

    /// Whether the menu is making a worktree.
    fn worktree_menu_is_creating(&self) -> bool {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.worktree_menu_is_creating()
        })
    }

    /// How many worktrees the menu has read, if it has finished reading.
    fn worktrees_listed(&self) -> Option<usize> {
        self.workspace
            .read(&self.app, |workspace, _| workspace.worktrees_listed())
    }

    /// What one of the worktree menu's controls dispatches.
    fn dispatch_worktree(&mut self, action: WorktreeAction) {
        self.dispatch_workspace_action(WorkspaceAction::Worktree(action));
    }

    /// Opens a tab's context menu over one of its rows, the way a secondary
    /// press on that row does.
    fn open_tab_menu_on(&mut self, tab: TabId, pane: PaneId) {
        self.dispatch_workspace_action(WorkspaceAction::TabMenu(TabMenuAction::Open { tab, pane }));
    }

    /// The row that menu is up on, if it is up.
    fn tab_menu_row(&self) -> Option<PaneId> {
        self.workspace
            .read(&self.app, |workspace, _| workspace.tab_context_menu().pane)
    }

    /// Runs a plugin's named action — which is what a menu entry does, and a
    /// palette row, and a chord.
    fn run_command(&mut self, name: &str) {
        let action = ActionName::parse(name).expect("a literal that parses");
        let id = self
            .workspace
            .read(&self.app, |workspace, _| workspace.host().action(&action))
            .unwrap_or_else(|| panic!("nothing is registered as {name}"));
        self.dispatch_workspace_action(WorkspaceAction::Run(id));
    }

    /// Runs a named action *about* something — which is what a button on a
    /// card does, and `--action "name subject"` on the command line.
    fn run_about(&mut self, name: &str, subject: &str) {
        let action = ActionName::parse(name).expect("a literal that parses");
        let (id, subject) = self.workspace.read(&self.app, |workspace, _| {
            let id = workspace
                .host()
                .action(&action)
                .unwrap_or_else(|| panic!("nothing is registered as {name}"));
            (id, workspace.subject(subject))
        });
        self.dispatch_workspace_action(WorkspaceAction::RunAbout(id, subject));
    }

    /// What a tab is called, whatever its panes are called.
    fn tab_name(&self, tab: TabId) -> String {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .tabs()
                .get(tab)
                .map(|tab| tab.name().to_owned())
                .unwrap_or_default()
        })
    }

    /// Whether the keyboard is in a field some plugin owns.
    fn a_plugin_field_has_keys(&self) -> bool {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.host().a_field_has_keys()
        })
    }

    /// Whether it is in a pane's composer instead.
    fn a_pane_field_has_keys(&self) -> bool {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.tabs().panes().any(|(_, pane)| {
                workspace
                    .input(pane.id())
                    .is_some_and(|input| input.has_keys())
            })
        })
    }

    /// Every entry in a tab's context menu, as `owner/entry`, in drawing order.
    fn tab_menu_entries(&self) -> Vec<String> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .host()
                .slots()
                .contributors(crate::plugins::tabs::TAB_MENU_ENTRIES)
                .into_iter()
                .map(|(owner, entry)| format!("{owner}/{entry}"))
                .collect()
        })
    }

    /// Shows the page whose rail row says `title`, the way a click on the rail
    /// does.
    fn select_settings_section(&mut self, title: &str) {
        let page = self
            .workspace
            .read(&self.app, |workspace, _| {
                workspace.settings_page_named(title)
            })
            .unwrap_or_else(|| panic!("no settings page is called {title:?}"));
        self.dispatch_workspace_action(WorkspaceAction::Settings(SettingsAction::Select(page)));
    }

    /// Whether any popup is up, which is the one question five parts of the
    /// window ask before deciding what a key or a hover means.
    fn a_popup_is_open(&self) -> bool {
        self.workspace
            .read(&self.app, |workspace, _| workspace.a_popup_is_open())
    }

    fn is_settings_page_open(&self) -> bool {
        self.workspace
            .read(&self.app, |workspace, _| workspace.is_settings_page_open())
    }

    /// The options that are not the tab strip's.
    fn general(&self) -> GeneralOptions {
        self.workspace
            .read(&self.app, |workspace, _| workspace.general())
    }

    /// Whether the next shell opened in this window would be a login shell.
    fn shell_login(&self) -> bool {
        self.workspace
            .read(&self.app, |workspace, app| workspace.shell_login(app))
    }

    /// Presses the one text field the showing section drew.
    fn click_section_field(&mut self) {
        let boxes = settings_field_boxes(&self.frame());
        assert_eq!(boxes.len(), 1, "one field on this section");
        self.click(center(boxes[0]), MouseButton::Left);
        self.frame();
    }

    /// Clicks the row for the plugin called `name` in the Plugins page's list.
    ///
    /// By the text on the row, found where it was drawn: the list is filtered
    /// and reordered by what is registered, so its rows have no fixed places.
    fn click_plugin(&mut self, name: &str) {
        let scene = self.frame();
        // The column the list is in, taken from its own field: the card is to
        // the right of it and shares baselines with it, so a line built from
        // both columns is a line neither of them drew.
        let column = settings_field_boxes(&scene)
            .into_iter()
            .next()
            .expect("the Plugins section has a field of its own");

        // The row's name, or the name with the word a row may end in — the
        // version an update would bring, "installed" — run on after it: the
        // two share a baseline, so they are one line to `text_lines`.
        let lines = text_lines(&scene, |at| {
            at.x() >= column.min_x() && at.x() <= column.max_x()
        });
        let row = lines
            .iter()
            .find(|(_, line)| line.trim() == name)
            .or_else(|| lines.iter().find(|(_, line)| line.trim().starts_with(name)))
            .unwrap_or_else(|| panic!("no row in the plugin list says {name:?}"));

        self.click(row.0 + vec2f(4., 4.), MouseButton::Left);
        self.frame();
    }

    /// Which plugin the store is downloading, straight from its model.
    fn store_downloading(&self) -> Option<crook_plugin::PluginId> {
        let model = self.store_model().expect("the store has built");
        model.read(&self.app, |model, _| model.downloading().cloned())
    }

    /// The plugins waiting behind the download in flight, in order.
    fn store_queued(&self) -> Vec<crook_plugin::PluginId> {
        let model = self.store_model().expect("the store has built");
        model.read(&self.app, |model, _| model.queued())
    }

    /// The store's model, or `None` while the store has not built — which
    /// is what a store switched off comes to.
    fn store_model(&self) -> Option<ModelHandle<crate::plugins::store::model::StoreModel>> {
        self.store
            .as_ref()
            .expect("the window was opened with a store of its own")
            .model()
    }

    /// Presses the button in the sidebar that says `label` — the one under
    /// a section's list, where the Store's look and both lists' update-all
    /// are.
    fn click_sidebar_button(&mut self, label: &str) {
        let scene = self.frame();
        let at = word_in(&scene, panel_box(&scene), label);
        self.click(at + vec2f(4., 4.), MouseButton::Left);
        self.frame();
    }

    /// Presses the button on the page beside the list that says `label`.
    ///
    /// By its text, found where it was drawn, the way [`click_plugin`] finds
    /// a row: a card's buttons have no fixed places either. The word's own
    /// first glyph rather than the line's, because a button shares its line
    /// with the label at the other end of the row — "On this machine" and
    /// "Remove" are one line to [`page_line`], and pressing where that line
    /// starts presses the label.
    ///
    /// [`click_plugin`]: Self::click_plugin
    fn click_page_button(&mut self, label: &str) {
        let scene = self.frame();
        let at = page_word(&scene, label);
        self.click(at + vec2f(4., 4.), MouseButton::Left);
        self.frame();
    }

    /// The binding being recorded on the Keyboard Shortcuts page, if one is.
    fn recording(&self) -> Option<Recording> {
        self.workspace
            .read(&self.app, |workspace, _| workspace.recording())
    }

    /// Reads a keybindings file out of `text` and puts it in force.
    fn bind(&mut self, text: &str) {
        let directory = Scratch::new();
        let path = directory.path().join("keybindings.json");
        fs::write(&path, text).expect("a scratch keybindings file");
        let keybindings = crate::keybindings::Keybindings::load(&path);
        assert!(!keybindings.is_empty(), "nothing in {text:?} was a binding");

        self.workspace_update(|workspace, ctx| workspace.set_keybindings(keybindings, ctx));
    }

    /// Turns the wheel over the middle of the settings card.
    ///
    /// Positive is away from the user, so a negative count moves the page
    /// down. A count far past the end is how a test says "the bottom of the
    /// page" without depending on how tall the page happens to be: the offset
    /// clamps, and one more click changes nothing.
    fn scroll_settings_page(&mut self, lines: f32) {
        let position = center(settings_pane_box(&self.frame()));
        self.dispatch(Event::ScrollWheel {
            position,
            delta: ScrollDelta::Lines(vec2f(0., lines)),
            modifiers: Modifiers::default(),
        });
    }

    /// What a keystroke means to the workspace, if it means anything.
    fn action_for(&self, key: &str, modifiers: Modifiers) -> Option<WorkspaceAction> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.action_for(&Keystroke::new(key, modifiers))
        })
    }

    /// Presses a key and applies whatever it is bound to, which is what the
    /// window's own key handler does.
    fn press_key(&mut self, key: &str, modifiers: Modifiers) -> bool {
        match self.action_for(key, modifiers) {
            Some(action) => {
                self.dispatch_workspace_action(action);
                true
            }
            None => false,
        }
    }

    /// Puts known git facts in front of the renderer.
    ///
    /// The gather chain is never started in a test, so this is the only way a
    /// row has a branch or a diff count to print — and the only way the
    /// assertion does not depend on the repository the test runs in.
    fn record_git(&mut self, pane: PaneId, branch: &str, diff: Option<DiffStats>) {
        self.record_git_facts(pane, branch, diff, false);
    }

    /// The same, for a pane whose directory is a linked worktree.
    fn record_git_facts(
        &mut self,
        pane: PaneId,
        branch: &str,
        diff: Option<DiffStats>,
        worktree: bool,
    ) {
        let directory = self.workspace.read(&self.app, |workspace, _| {
            workspace
                .tabs()
                .pane(pane)
                .and_then(|pane| pane.session().working_directory.clone())
                .expect("every seeded session has a working directory")
        });
        let facts = GitFacts {
            branch: Some(Head::Branch(branch.to_owned())),
            diff,
            worktree,
        };

        let workspace = &self.workspace;
        self.app.update(|ctx| {
            let git = workspace.read(ctx, |workspace, _| workspace.git().clone());
            git.update(ctx, |model, ctx| model.record(directory, facts, ctx));
        });
    }

    /// Whether the window has been told it needs another frame.
    fn needs_a_frame(&self) -> bool {
        let window_id = self.window_id;
        self.app.read(|ctx| ctx.has_window_invalidations(window_id))
    }

    fn tab_ids(&self) -> Vec<TabId> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.tabs().iter().map(Tab::id).collect()
        })
    }

    fn active_id(&self) -> TabId {
        self.workspace
            .read(&self.app, |workspace, _| workspace.tabs().active_id())
    }

    /// The group a tab is folded into, if it is in one.
    fn group_of(&self, tab: TabId) -> Option<GroupId> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.tabs().get(tab).and_then(Tab::group)
        })
    }

    /// The tabs of a group, in strip order.
    fn members_of(&self, group: GroupId) -> Vec<TabId> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.tabs().members(group).map(Tab::id).collect()
        })
    }

    /// Where the menu lists the checkout under `root`, if it lists one.
    fn worktree_index_under(&self, root: &Path) -> Option<usize> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .tab_menu()
                .worktrees()
                .iter()
                .position(|worktree| worktree.path.starts_with(root))
        })
    }

    /// The panes of one tab, in render order.
    fn panes_of(&self, tab: TabId) -> Vec<PaneId> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .tabs()
                .get(tab)
                .map(|tab| tab.panes().iter().map(Pane::id).collect())
                .unwrap_or_default()
        })
    }

    /// Every pane in the window, in bar order.
    fn pane_ids(&self) -> Vec<PaneId> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .tabs()
                .panes()
                .map(|(_, pane)| pane.id())
                .collect()
        })
    }

    /// The panes of the active tab, in render order.
    fn active_pane_ids(&self) -> Vec<PaneId> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .tabs()
                .active()
                .map(|tab| tab.panes().iter().map(Pane::id).collect())
                .unwrap_or_default()
        })
    }

    fn focused_pane_id(&self) -> Option<PaneId> {
        self.workspace
            .read(&self.app, |workspace, _| workspace.tabs().focused_pane_id())
    }

    /// Opens a shell in every pane, the way the window delegate does at
    /// startup. Returns false when the machine has no shell to open.
    ///
    /// **With no command marks.** A shell that reports no boundaries keeps the
    /// whole session in one open block, which is the state a drag across the
    /// output can select in — see `block_list`, where selection is still the
    /// emulator's and therefore reaches the open block only. It is also a real
    /// state a person is in, on any shell the integration has no snippet for.
    /// The tests that are *about* blocks ask for marks with
    /// [`Self::start_terminals_with_marks`].
    fn start_terminals(&mut self) -> bool {
        self.start_shells(false)
    }

    /// The same, with the command marks a block list is built out of.
    fn start_terminals_with_marks(&mut self) -> bool {
        self.start_shells(true)
    }

    fn start_shells(&mut self, marks: bool) -> bool {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                workspace.set_shell_marks(marks, ctx);
                // **Not a login shell**, though a real pane is one. A login
                // shell reads the startup files of whoever is running the
                // suite, and a `~/.zprofile` that prints a greeting — or an
                // ASCII pirate — puts thirty rows on screen before the first
                // prompt and moves every row these tests count. The login
                // shell is proved where it can be proved honestly, against a
                // home directory the test owns: see `shell_integration::tests`.
                workspace.set_shell_login(false, ctx);
                workspace.start_terminals(ctx);
            });
        });

        let Some(pane) = self.focused_pane_id() else {
            return false;
        };
        let started = self.workspace.read(&self.app, |workspace, app| {
            workspace.terminal(pane, app).is_some()
        });
        if !started {
            eprintln!("skipped: no shell could be started here");
        }
        started
    }

    /// Types into a pane's shell without going through the keyboard.
    fn type_into(&mut self, pane: PaneId, text: &str) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| workspace.type_into(pane, text, ctx));
        });
    }

    /// Puts text in a pane's field without sending it, the way `--type` does.
    fn type_field(&mut self, pane: PaneId, text: &str) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                workspace.type_into_input(pane, text, ctx);
            });
        });
    }

    /// Pumps the queue for `patience`, for an assertion about something that
    /// must *not* happen.
    ///
    /// A `wait_for` proves an event arrived; only a wait with no condition can
    /// prove one did not, and it has to be long enough that a pty echo would
    /// have come back if it were coming.
    fn settle(&mut self, patience: std::time::Duration) {
        let deadline = std::time::Instant::now() + patience;
        while std::time::Instant::now() < deadline {
            self.queue.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// What is in a pane's input field, unsent.
    fn field_text(&self, pane: PaneId) -> String {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .input(pane)
                .map(|input| input.editor().text().to_owned())
                .unwrap_or_default()
        })
    }

    /// Where the caret is in a pane's input field.
    fn field_caret(&self, pane: PaneId) -> usize {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .input(pane)
                .map_or(0, |input| input.editor().caret())
        })
    }

    /// What is selected in a pane's input field.
    fn field_selection(&self, pane: PaneId) -> String {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .input(pane)
                .map(|input| input.editor().selected_text().to_owned())
                .unwrap_or_default()
        })
    }

    /// Whether a pane's shell has taken the whole screen.
    fn alt_screen(&self, pane: PaneId) -> bool {
        self.workspace.read(&self.app, |workspace, app| {
            workspace
                .terminal(pane, app)
                .is_some_and(|(_, snapshot)| snapshot.alt_screen)
        })
    }

    /// How many columns a pane's grid was laid out at.
    fn terminal_columns(&self, pane: PaneId) -> usize {
        self.workspace
            .read(&self.app, |workspace, app| {
                workspace.terminal(pane, app).map(|(_, grid)| grid.columns)
            })
            .unwrap_or_default()
    }

    /// What is selected in a pane's output, which is what a copy would take.
    fn terminal_selection(&self, pane: PaneId) -> Option<String> {
        self.workspace.read(&self.app, |workspace, app| {
            workspace.terminal_selection(pane, app)
        })
    }

    /// Selects the first occurrence of `text` in a pane's output, the way
    /// `--select-output` does.
    fn select_in_output(&mut self, pane: PaneId, text: &str) -> bool {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                workspace.select_in_output(pane, text, ctx)
            })
        })
    }

    /// Presses the left button at a point, without releasing it.
    fn hold(&mut self, position: Vector2F, click_count: u32) {
        self.dispatch(Event::MouseDown {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count,
        });
    }

    /// Holds the button down with Alt, which is what asks for a rectangle.
    fn hold_alt(&mut self, position: Vector2F, click_count: u32) {
        self.dispatch(Event::MouseDown {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            click_count,
        });
    }

    /// Drags to a point with the button still down.
    fn drag_to(&mut self, position: Vector2F) {
        self.dispatch(Event::MouseDragged {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
        });
    }

    /// Lets the button go where it is.
    fn let_go(&mut self, position: Vector2F) {
        self.dispatch(Event::MouseUp {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
        });
    }

    /// What a pane's shell is showing.
    fn terminal_text(&self, pane: PaneId) -> String {
        self.workspace
            .read(&self.app, |workspace, app| {
                workspace.terminal_text(pane, app)
            })
            .unwrap_or_default()
    }

    /// Presses a key exactly as the window delegate does: the bindings first,
    /// and only what they decline reaches the element tree — which is where a
    /// focused pane's grid picks it up.
    ///
    /// This *is* the line between Crook's keyboard and the shell's, so a test
    /// that dispatched straight into the tree would be testing the wrong half.
    fn press(&mut self, key: &str, modifiers: Modifiers, chars: &str) {
        let keystroke = Keystroke::new(key, modifiers);
        let bound = self
            .workspace
            .read(&self.app, |workspace, _| workspace.action_for(&keystroke));

        if let Some(action) = bound {
            self.dispatch_workspace_action(action);
            return;
        }
        self.dispatch(Event::KeyDown {
            keystroke,
            chars: chars.to_owned(),
        });
    }

    /// Pumps the queue until `settled` is true, or fails after
    /// [`SHELL_TIMEOUT`].
    fn wait_for(&mut self, what: &str, mut settled: impl FnMut(&mut Self) -> bool) {
        let deadline = std::time::Instant::now() + SHELL_TIMEOUT;
        loop {
            self.queue.run_until_parked();
            if settled(self) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{what}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn quit_requests(&self) -> usize {
        self.quit_requests.get()
    }

    /// Where a pane's session says it is working.
    fn working_directory(&self, pane: PaneId) -> Option<PathBuf> {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .tabs()
                .pane(pane)
                .and_then(|pane| pane.session().working_directory.clone())
        })
    }

    /// What the strip would print as a pane's title.
    fn pane_title(&self, pane: PaneId) -> String {
        self.workspace.read(&self.app, |workspace, _| {
            workspace
                .tabs()
                .pane(pane)
                .map(|pane| pane.title().to_owned())
                .unwrap_or_default()
        })
    }

    /// The branch the strip would print for a pane, which is looked up by the
    /// session's working directory.
    fn branch_shown(&self, pane: PaneId) -> Option<Head> {
        self.workspace.read(&self.app, |workspace, app| {
            let session = workspace.tabs().pane(pane)?.session();
            workspace.git_facts(session, app)?.branch.clone()
        })
    }
}

/// How long a test waits for a shell to do as it was told.
///
/// It bounds a *failure*, never a pass: a test that is going to succeed does so
/// in a second. What makes it generous is that the thing being waited for is a
/// real shell starting and sourcing somebody's rc files, on a machine running
/// as many of these tests at once as it has cores.
///
/// The way to make a shell test reliable is not to raise this: it is to wait
/// for the shell to say something *before* typing at it, so that starting up
/// and doing as it was told are two waits rather than one. Every test here
/// that types does that.
const SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The modifier that means "this is an application command" on this platform.
///
/// Command on macOS and Control-*Shift* everywhere else, which is the shape
/// the tty forces: a bare `ctrl-d` off macOS has to be able to end an input,
/// so nothing of Crook's own can live on one. See [`crate::input_keys`].
fn platform_chord() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            cmd: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        }
    }
}

/// The chord that opens the settings, which is not the one the tab bindings
/// use.
///
/// `input_keys` spends Ctrl-Shift on the tabs everywhere but macOS, precisely
/// so the field keeps plain Ctrl — and the settings chord is the exception it
/// names: the same `cmd-,` / `ctrl-,` every application has, with no Shift,
/// because it has no field gesture to stay out of the way of.
fn settings_chord() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            cmd: true,
            ..Default::default()
        }
    } else {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }
}

/// The tabs, by their boxes, top to bottom.
///
/// The panel is the only place tabs live, so this is [`panel_rows`] under the
/// name every test that clicks a tab already uses. It was a strip across the
/// header once; the strip is gone and the tests it was written for are about
/// the tabs rather than about where they were.
fn tab_boxes(scene: &Scene) -> Vec<RectF> {
    panel_rows(scene)
}

/// The close buttons that are currently drawn, by their rounded boxes.
///
/// Bounded by size as well as by radius: a row, the options menu's info note
/// and the hover detail card are all 4px-rounded too, and only a close button
/// is a 16px square. A clipped one is narrower, never wider.
fn close_boxes(scene: &Scene) -> Vec<RectF> {
    rects_rounded_by(scene, Radius::Pixels(4.))
        .into_iter()
        .filter(|bounds| bounds.width() <= super::CLOSE_BUTTON_SIZE + 0.5)
        .collect()
}

/// The composers that are currently drawn, by the box each one paints.
///
/// There is no box, and that is exactly how one is found: the composer paints
/// a rect with no fill, no radius and no side borders — one top edge and
/// nothing else, which is the whole of its chrome. A settings page's theme
/// cards carry the same top-only edge, so the second half of the test is that
/// a composer runs the full width of a pane: it is a direct child of the
/// pane's column, and its rule is full-bleed for exactly that reason.
fn composer_boxes(scene: &Scene) -> Vec<RectF> {
    let panels = panel_boxes(scene);
    visible_rects(scene)
        .filter(|(rect, _)| {
            rect.background == Fill::None
                && rect.corner_radius == CornerRadius::default()
                && rect.border.top
                && !rect.border.left
                && !rect.border.bottom
                && !rect.border.right
        })
        .map(|(_, bounds)| bounds)
        .filter(|bounds| {
            panels.iter().any(|panel| {
                (panel.min_x() - bounds.min_x()).abs() < 0.5
                    && (panel.max_x() - bounds.max_x()).abs() < 0.5
            })
        })
        .collect()
}

/// The button that opens another tab, by its rounded box.
fn new_tab_box(scene: &Scene) -> RectF {
    let boxes = rects_rounded_by(scene, Radius::Pixels(5.));
    assert_eq!(boxes.len(), 1, "exactly one new-tab button per frame");
    boxes[0]
}

fn rects_rounded_by(scene: &Scene, radius: Radius) -> Vec<RectF> {
    visible_rects(scene)
        .filter(|(rect, _)| rect.corner_radius.get_top_left() == radius)
        .map(|(_, bounds)| bounds)
        .collect()
}

/// Every rect the frame will actually draw, paired with the part of it its
/// layer's scissor leaves on screen. What is painted outside a clip is neither
/// drawn nor clickable, so the tests reason about the visible part only.
fn visible_rects(scene: &Scene) -> impl Iterator<Item = (&Rect, RectF)> {
    scene.layers().flat_map(|layer| {
        layer.rects.iter().filter_map(|rect| {
            let visible = match layer.clip_bounds {
                Some(clip) => clip.intersection(rect.bounds)?,
                None => rect.bounds,
            };
            Some((rect, visible))
        })
    })
}

fn center(bounds: RectF) -> Vector2F {
    bounds.origin() + bounds.size().scale(0.5)
}

/// Everything the frame says, as one string.
///
/// The stub shaper numbers each glyph by its character, so a frame can be read
/// back and searched. That is what makes "the title line shows the branch" an
/// assertion about pixels rather than about a field.
fn frame_text(scene: &Scene) -> String {
    text_where(scene, |_| true)
}

/// What the tab list says, without the body beside it.
///
/// The body prints the session's title and its working directory too, so a
/// whole-frame search cannot tell "the row stopped showing this" from "the row
/// never showed it". The tabs are in the panel, so the cut is by x rather than
/// by y — everything left of the panel's right edge is the list's.
fn strip_text(scene: &Scene) -> String {
    panel_text(scene)
}

/// The frame's text, one line at a time, with where each line was drawn.
///
/// Glyphs grouped by the row they sit on and sorted along it, which is what
/// turns "the frame says X" into "X is *here*" — the only way to click on
/// something a list drew without knowing how tall its rows are or how many
/// came before it.
///
/// A scene has no occlusion, so a line found here may be behind something.
/// Callers that care filter by position first.
fn text_lines(scene: &Scene, keep: impl Fn(Vector2F) -> bool) -> Vec<(Vector2F, String)> {
    let mut rows: HashMap<i32, Vec<(f32, char)>> = HashMap::new();
    for glyph in scene.layers().flat_map(|layer| layer.glyphs.iter()) {
        let Some(character) = char::from_u32(glyph.glyph_key.glyph_id) else {
            continue;
        };
        // Filtered *before* grouping, not after: two columns of a page share
        // baselines, and a line built from both is a line neither of them
        // drew.
        if !keep(glyph.position) {
            continue;
        }
        // Rounded, because two glyphs on one line may differ in the last bits
        // of their baseline after a scale.
        rows.entry(glyph.position.y().round() as i32)
            .or_default()
            .push((glyph.position.x(), character));
    }

    let mut lines: Vec<(Vector2F, String)> = rows
        .into_iter()
        .map(|(y, mut glyphs)| {
            glyphs.sort_by(|left, right| left.0.total_cmp(&right.0));
            let x = glyphs.first().map_or(0., |(x, _)| *x);
            let text: String = glyphs.into_iter().map(|(_, character)| character).collect();
            (vec2f(x, y as f32), text)
        })
        .collect();
    lines.sort_by(|left, right| left.0.y().total_cmp(&right.0.y()));
    lines
}

fn text_where(scene: &Scene, keep: impl Fn(Vector2F) -> bool) -> String {
    scene
        .layers()
        .flat_map(|layer| layer.glyphs.iter())
        .filter(|glyph| keep(glyph.position))
        .filter_map(|glyph| char::from_u32(glyph.glyph_key.glyph_id))
        .collect()
}

/// The options popup, by its ground.
fn menu_box(scene: &Scene) -> RectF {
    let boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                && rect.background == Fill::Solid(theme().surface_raised)
        })
        .map(|(_, bounds)| bounds)
        .collect();

    assert_eq!(boxes.len(), 1, "exactly one options popup per frame");
    boxes[0]
}

/// The popup's clickable option rows, top to bottom.
///
/// Found by shape rather than by colour, because an unhovered row is
/// transparent: every check row is the popup's full content width and exactly
/// `max(16, 12 * 1.2) + 2 * 2` tall, and nothing else in the popup is.
fn menu_option_boxes(scene: &Scene) -> Vec<RectF> {
    let popup = menu_box(scene);
    let mut rows: Vec<RectF> = visible_rects(scene)
        .map(|(_, bounds)| bounds)
        .filter(|bounds| {
            popup.contains_point(center(*bounds))
                && (bounds.height() - 20.).abs() < 0.5
                && bounds.width() > popup.width() - 4.
        })
        .collect();
    rows.sort_by(|left, right| left.min_y().total_cmp(&right.min_y()));
    rows
}

/// The two halves of each segmented control, top to bottom then left to right.
///
/// Found by shape: a segment is half the track, so it is far narrower than an
/// option row and far wider than the check slot or the info dot.
fn menu_segment_boxes(scene: &Scene) -> Vec<RectF> {
    let popup = menu_box(scene);
    let mut segments: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| rect.corner_radius.get_top_left() == Radius::Pixels(4.))
        .map(|(_, bounds)| bounds)
        .filter(|bounds| {
            popup.contains_point(center(*bounds)) && (60. ..100.).contains(&bounds.width())
        })
        .collect();
    segments.sort_by(|left, right| {
        left.min_y()
            .total_cmp(&right.min_y())
            .then(left.min_x().total_cmp(&right.min_x()))
    });
    segments
}

/// The info dot on "Show: PR link", by the icon it is.
fn info_dot_box(scene: &Scene) -> RectF {
    let dots = icons_in(scene, menu_box(scene), Lucide::Info);

    assert_eq!(dots.len(), 1, "exactly one info dot in the popup");
    assert!(
        (dots[0].width() - tab_options_menu::INFO_DOT_SIZE).abs() < 0.5,
        "the info dot is {} wide",
        dots[0].width()
    );
    dots[0]
}

/// The note the info dot opens.
///
/// Every `surface_raised` panel with a 4px radius, which while the menu is up
/// is the note and nothing else: the popup itself is 6px-rounded, and opening
/// the menu takes down any hover card. Deliberately not filtered by width —
/// the width is what the test is about.
fn info_notes(scene: &Scene) -> Vec<RectF> {
    detail_cards(scene)
}

/// Which option rows carry a check, by their index in `menu_option_boxes`.
fn checked_rows(scene: &Scene) -> Vec<usize> {
    menu_option_boxes(scene)
        .into_iter()
        .enumerate()
        .filter(|(_, row)| !icons_in(scene, *row, Lucide::Check).is_empty())
        .map(|(index, _)| index)
        .collect()
}

/// Every Lucide icon the frame draws, in paint order.
///
/// Marks that are drawings rather than icons — a sandboxed plugin's pirate —
/// are not in here: they are two layers apiece and nothing that counts icons
/// means to count them.
fn icons_of(scene: &Scene) -> Vec<Lucide> {
    scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .filter_map(|drawn| match drawn.icon_key.mark {
            Mark::Icon(icon) => Some(icon),
            Mark::Art(_) => None,
        })
        .collect()
}

/// Every icon of one kind painted inside `bounds`.
fn icons_in(scene: &Scene, bounds: RectF, icon: Lucide) -> Vec<RectF> {
    scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .filter(|drawn| drawn.icon_key.mark == Mark::Icon(icon))
        .filter(|drawn| bounds.contains_point(center(drawn.bounds)))
        .map(|drawn| drawn.bounds)
        .collect()
}

/// Where a row's label starts: its leftmost glyph.
///
/// Every glyph in the row is part of the label now — the check beside it is an
/// icon rather than a codepoint, so it is not in this list to be filtered out.
fn row_label_x(scene: &Scene, row: RectF) -> f32 {
    scene
        .layers()
        .flat_map(|layer| layer.glyphs.iter())
        .filter(|glyph| row.contains_point(glyph.position))
        .map(|glyph| glyph.position.x())
        .fold(f32::INFINITY, f32::min)
}

/// A point in the room the list leaves under the `+`, which is what the
/// options menu's secondary press is aimed at.
///
/// Below the band rather than on it. The band answers the same press, but it
/// paints a box of its own and so covers the ground exactly there — pressing
/// it would prove the band's handler works and say nothing about the room.
/// Ninety pixels off the foot of the panel clears the row of section buttons,
/// and it is far enough down to be outside the menu this opens: that menu
/// hangs from the *top* of the list area, so a point just under a short list
/// would be under the popup as well and the second press of a toggle would
/// land on the popup instead of on the ground.
fn empty_list_space(scene: &Scene) -> Vector2F {
    let panel = panel_box(scene);
    let plus = plus_box(scene);
    let point = vec2f(center(panel).x(), panel.max_y() - 90.);

    assert!(
        point.y() > plus.max_y(),
        "the list leaves no room under the {plus:?} to press"
    );
    point
}

/// The chips a row draws, by their pill boxes.
fn chip_boxes(scene: &Scene) -> Vec<RectF> {
    rects_rounded_by(scene, Radius::Pixels(3.))
}

/// The hover detail card, by its box.
fn detail_cards(scene: &Scene) -> Vec<RectF> {
    visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(4.)
                && rect.background == Fill::Solid(theme().surface_raised)
        })
        .map(|(_, bounds)| bounds)
        .collect()
}

#[test]
fn a_window_opens_on_one_tab_and_that_tab_is_active() {
    let harness = Harness::new(1);

    assert_eq!(harness.tab_ids().len(), 1);
    assert_eq!(harness.active_id(), harness.tab_ids()[0]);
}

#[test]
fn clicking_a_tab_selects_the_tab_that_is_under_the_pointer() {
    let mut harness = Harness::new(3);
    let ids = harness.tab_ids();
    let boxes = tab_boxes(&harness.frame());
    assert_eq!(boxes.len(), 3, "one box per tab");

    // Left of centre, well clear of the close button's slot.
    let target = boxes[0].origin() + vec2f(40., boxes[0].height() / 2.);
    harness.click(target, MouseButton::Left);

    assert_eq!(harness.active_id(), ids[0]);
    assert_eq!(harness.tab_ids(), ids, "selecting closes nothing");
}

#[test]
fn the_close_button_closes_its_own_tab() {
    let mut harness = Harness::new(2);
    let ids = harness.tab_ids();
    let scene = harness.frame();

    // Only the active tab draws one until a pointer arrives, which is the
    // second tab here: a new tab takes the selection.
    let buttons = close_boxes(&scene);
    assert_eq!(buttons.len(), 1);
    assert!(tab_boxes(&scene)[1].contains_point(center(buttons[0])));

    harness.click(center(buttons[0]), MouseButton::Left);

    assert_eq!(harness.tab_ids(), [ids[0]]);
    assert_eq!(harness.active_id(), ids[0]);
}

#[test]
fn middle_clicking_a_tab_closes_it_wherever_the_pointer_is_on_it() {
    let mut harness = Harness::new(2);
    let ids = harness.tab_ids();
    let boxes = tab_boxes(&harness.frame());

    harness.click(
        boxes[0].origin() + vec2f(40., boxes[0].height() / 2.),
        MouseButton::Middle,
    );

    assert_eq!(harness.tab_ids(), [ids[1]]);
}

#[test]
fn revealing_a_close_button_does_not_move_the_bar_under_the_cursor() {
    let mut harness = Harness::new(3);
    let before = tab_boxes(&harness.frame());

    // Hovering an inactive tab is what makes its close button appear. The
    // slot it appears in is reserved on every tab in every state, so nothing
    // may move.
    harness.move_to(before[0].origin() + vec2f(40., before[0].height() / 2.));
    let scene = harness.frame();

    assert_eq!(close_boxes(&scene).len(), 2, "hovered tab and active tab");
    assert_eq!(tab_boxes(&scene), before);
}

#[test]
fn closing_the_last_tab_asks_the_shell_to_quit_instead_of_emptying_the_strip() {
    let mut harness = Harness::new(1);
    let ids = harness.tab_ids();

    harness.dispatch_action(TabAction::Close(ids[0]));

    assert_eq!(harness.quit_requests.get(), 1);
    assert_eq!(harness.tab_ids(), ids);
}

#[test]
fn the_first_tab_clears_the_window_controls_this_platform_draws() {
    // The panel owns the top-left corner, and it spends the reservation on a
    // strip of nothing above its first row rather than on room beside it —
    // see `platform_insets` — so what clears the traffic lights is the row's
    // *top*. Asked of this platform's own layout and of the one that
    // reserves the most, so the arithmetic is checked wherever this runs.
    let mut harness = Harness::new(1);
    let insets = harness.window_insets();
    let first = tab_boxes(&harness.frame())[0];
    let reserved = if insets.panel_left > 0. {
        crate::workspace::tabs_panel::TITLE_STRIP_HEIGHT
    } else {
        0.
    };
    assert!(
        first.min_y() >= reserved,
        "the first row starts at {} but {reserved} is reserved above it for window controls",
        first.min_y()
    );

    harness.override_controls(ControlLayout::MacOs);
    let lowered = tab_boxes(&harness.frame())[0];
    assert!(
        lowered.min_y()
            >= first
                .min_y()
                .max(crate::workspace::tabs_panel::TITLE_STRIP_HEIGHT),
        "under the traffic lights the first row starts at {}, which is not below the strip",
        lowered.min_y()
    );
}

#[test]
fn the_header_reserves_the_left_corner_this_platform_puts_its_controls_in_and_no_more() {
    // Crook's header *is* the title bar, so something is over it — and only at
    // one end. Reserving at both would leave a hole at whichever end this
    // platform's controls are not, which is the failure nobody sees because it
    // is always the end they are not looking at.
    //
    // Only the left end is measurable from a release binary: nothing in the
    // box is pinned to the right of this row, so the reservation at that end
    // is checked against what a plugin pins there — see
    // `sandboxed::what_a_plugin_pins_to_the_header_clears_the_right_corner`.
    let mut harness = Harness::new(1);
    let insets = harness.window_insets();
    let scene = harness.frame();
    let first = tab_boxes(&scene)[0];

    assert!(
        first.min_x() >= insets.header_left,
        "the first tab starts at {} inside a {} reservation",
        first.min_x(),
        insets.header_left
    );
    // The reservation and the header's own 8px padding, and nothing else.
    assert!(
        first.min_x() < insets.header_left + 24.,
        "the first tab starts at {}, further in than the reservation of {}",
        first.min_x(),
        insets.header_left
    );
}

#[test]
fn fullscreen_gives_back_the_room_the_traffic_lights_were_using() {
    // macOS moves its traffic lights into the menu-bar overlay in fullscreen.
    // The room reserved for them has to go with them or the header ends in a
    // 64px hole — and nothing tells the application it happened, which is why
    // the state is read on the render path rather than remembered.
    // Asked of the same call the renderer makes rather than measured off a
    // control, because the reservation is at the *left* of the panel's control
    // bar and everything in that bar is right-aligned: the room is real, and
    // nothing is drawn in it to move.
    let mut harness = Harness::new(1);
    harness.override_controls(ControlLayout::MacOs);
    assert_eq!(
        harness.window_insets().panel_left,
        ControlLayout::MacOs
            .insets(WindowChrome::Client, false)
            .left,
        "the panel does not reserve the corner the traffic lights are in"
    );

    harness.set_window_state(WindowState { fullscreen: true });

    assert_eq!(
        harness.window_insets().panel_left,
        0.,
        "fullscreen did not give back what the traffic lights had"
    );
}

#[test]
fn pressing_the_header_where_nothing_is_picks_the_window_up() {
    // The gesture that makes this row a title bar. It is a *press*, not a
    // click: the window manager takes over the pointer from the press onward,
    // so waiting for the release would mean waiting for one that never comes.
    let mut harness = Harness::new(1);
    let scene = harness.frame();
    let empty = empty_header_point(&scene);

    harness.dispatch(Event::MouseDown {
        button: MouseButton::Left,
        position: empty,
        modifiers: Modifiers::default(),
        click_count: 1,
    });

    assert_eq!(harness.window_requests(), vec![Request::Drag]);
}

#[test]
fn pressing_something_in_the_title_bar_does_not_pick_the_window_up() {
    // "Empty" is whatever the row's children did not claim, and this is the
    // half of that which fails silently: a title bar that dragged the window
    // from its own controls would make every tab unclickable, and the tab
    // would still be highlighted while the window moved.
    //
    // The controls are the panel's — the header row itself holds only what a
    // plugin pinned to it, which on a release build is nothing — and the
    // panel's own bar is title bar too, so they are what this rule is about.
    let mut harness = Harness::new(2);
    let scene = harness.frame();
    let tab = center(tab_boxes(&scene)[0]);
    let plus = center(plus_box(&scene));

    harness.click(tab, MouseButton::Left);
    harness.click(plus, MouseButton::Left);

    assert!(
        harness.window_requests().is_empty(),
        "a press on a control dragged the window: {:?}",
        harness.window_requests()
    );
}

#[test]
fn a_press_that_dismisses_the_options_menu_does_not_pick_the_window_up() {
    // The menu's underlay is modal and it is *inside* the header, so a press
    // outside the menu reaches the drag region on its way to nowhere. Taking
    // it as empty space would move the window every time the menu was
    // dismissed, which is the one gesture that has to leave the window alone.
    let mut harness = Harness::new(1);
    harness.dispatch_option(OptionsAction::TogglePopup);
    let scene = harness.frame();
    let empty = empty_header_point(&scene);

    harness.click_times(empty, 1);

    assert!(
        !harness.is_menu_open(),
        "the press did not dismiss the menu"
    );
    assert!(
        harness.window_requests().is_empty(),
        "dismissing the menu dragged the window"
    );
}

#[test]
fn double_clicking_the_header_maximises_the_window() {
    let mut harness = Harness::new(1);
    let scene = harness.frame();
    let empty = empty_header_point(&scene);

    harness.click_times(empty, 1);
    harness.click_times(empty, 2);

    // The first press is still a drag: the window manager decides what a drag
    // of zero pixels was, and every desktop that has this gesture starts one
    // the same way.
    assert_eq!(
        harness.window_requests(),
        vec![Request::Drag, Request::ToggleMaximized]
    );
}

#[test]
fn the_panel_owns_the_top_left_corner_of_the_window_in_the_vertical_layout() {
    // The layout Crook opens in. The panel's top row is the window's top-left
    // corner, so it is what the traffic lights are painted over and what that
    // end of the window is dragged by — and the header, which is no longer in
    // that corner, owes neither. The control bar that used to be that row is
    // gone; what is left is the reservation itself, which exists only where
    // something is painted over it.
    let mut harness = Harness::panel(1);
    harness.override_controls(ControlLayout::MacOs);
    let insets = harness.window_insets();
    assert_eq!(insets.header_left, 0., "the header kept a corner it lost");
    assert!(insets.panel_left > 0., "the panel took no reservation");

    let scene = harness.frame();
    let field = panel_search_box(&scene);
    assert!(
        field.min_y() >= tabs_panel::TITLE_STRIP_HEIGHT,
        "the search box starts at {} and the traffic lights reach {}",
        field.min_y(),
        tabs_panel::TITLE_STRIP_HEIGHT
    );

    // The strip itself: above the search box, inside the room the lights are
    // painted in.
    harness.dispatch(Event::MouseDown {
        button: MouseButton::Left,
        position: vec2f(insets.panel_left / 2., tabs_panel::TITLE_STRIP_HEIGHT / 2.),
        modifiers: Modifiers::default(),
        click_count: 1,
    });

    assert_eq!(harness.window_requests(), vec![Request::Drag]);
}

#[test]
fn a_platform_that_paints_nothing_over_the_panel_gets_no_strip_at_all() {
    // The other half, and the one the person asking for the control bar to go
    // was looking at: with nothing over the corner there is nothing to reserve,
    // so the panel starts with its search box and the row is not drawn at all.
    let mut harness = Harness::panel(1);
    harness.override_controls(ControlLayout::Freedesktop);
    assert_eq!(harness.window_insets().panel_left, 0.);

    let field = panel_search_box(&harness.frame());
    assert!(
        field.min_y() < tabs_panel::TITLE_STRIP_HEIGHT,
        "the search box starts at {}, which is a strip's worth down a panel        that reserved nothing",
        field.min_y()
    );
}

/// The header row, by the ground it puts across the top of the window.
///
/// Found the way a person finds it rather than by what is in it: it starts at
/// the very top and it runs the whole width, and nothing else does both. It
/// used to be found by the rule under it, until the rule was taken out — which
/// is why this looks for the ground instead. What a release binary draws
/// *inside* the row is nothing at all — the one place it has is filled by a
/// plugin installed from a file — so a helper that looked for a control would
/// find the row only on the builds that had one.
fn header_box(scene: &Scene) -> RectF {
    let right = far_edge(scene);
    let boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, bounds)| {
            rect.background == Fill::Solid(theme().surface) && is_the_header(bounds, right)
        })
        .map(|(_, bounds)| bounds)
        .collect();

    assert_eq!(boxes.len(), 1, "exactly one header per frame");
    boxes[0]
}

/// Whether a box is where the header is: against the top of the frame, and
/// reaching its right edge.
///
/// Two rules, because either alone catches something else. The tabs panel is
/// down the left side and starts at the top too, so "at the top" is not
/// enough; the header starts where the panel ends and is therefore not the
/// full width, so "as wide as the frame" is wrong. Touching the top *and* the
/// far edge is the one shape nothing else has.
///
/// The far edge comes from the scene rather than from a constant: a frame is
/// taken at whatever size a test wants one at, and a helper that compared
/// against the usual size would quietly answer "no header" in the tests that
/// take a small one.
fn is_the_header(bounds: &RectF, right: f32) -> bool {
    bounds.min_y() < 0.5 && bounds.max_x() >= right - 0.5
}

/// How far the frame reaches.
fn far_edge(scene: &Scene) -> f32 {
    visible_rects(scene)
        .map(|(_, bounds)| bounds.max_x())
        .fold(0., f32::max)
}

/// A point in the header where nothing is drawn.
///
/// The row holds one item at most — whatever a plugin pinned to the right of
/// it — and a release binary pins nothing, so its middle is empty space by
/// construction. Derived from the row's own bounds rather than measured off
/// something drawn in it, because what is drawn in it is not this build's
/// decision to make: a test about picking the window up must not start
/// failing the day somebody installs a plugin.
fn empty_header_point(scene: &Scene) -> Vector2F {
    center(header_box(scene))
}

/// Enough tabs that each one is far narrower than its own contents.
const CROWDED: usize = 30;

#[test]
fn clicking_a_crowded_tab_selects_it_and_closes_nothing() {
    let mut harness = Harness::new(CROWDED);
    let ids = harness.tab_ids();
    let tabs = tab_boxes(&harness.frame());
    let middle = |tab: RectF| tab.origin() + vec2f(2., tab.height() / 2.);

    // Hovering a tab is what makes it draw a close button at all. Unclipped,
    // that button is painted — and hit-tested — over the left half of the tab
    // to its right, so the click below would land on it.
    harness.move_to(middle(tabs[2]));
    let tabs = tab_boxes(&harness.frame());

    harness.click(middle(tabs[3]), MouseButton::Left);

    assert_eq!(harness.tab_ids(), ids, "clicking a tab closed one");
    assert_eq!(harness.active_id(), ids[3]);
}

#[test]
fn the_new_tab_button_opens_a_tab_without_closing_the_active_one() {
    // Crowded on purpose: the `+` sits under a list long enough to scroll, and
    // it is outside the scroll area for exactly that reason — a `+` that went
    // with the content would be unreachable at the tab count a person most
    // wants another tab at.
    let mut harness = Harness::new(CROWDED);
    let ids = harness.tab_ids();
    let button = new_tab_box(&harness.frame());

    harness.click(center(button), MouseButton::Left);

    let after = harness.tab_ids();
    assert_eq!(after.len(), ids.len() + 1, "the new tab replaced one");
    assert!(
        ids.iter().all(|id| after.contains(id)),
        "opening a tab closed another"
    );
}

#[test]
fn reporting_agent_progress_into_a_session_repaints_the_tab_that_shows_it() {
    // The path an agent runtime uses to rename its own tab and to report that
    // it has stopped. A mutation that did not notify would leave the old title
    // and an idle dot on screen — with nothing reporting an error — until some
    // unrelated click happened to rebuild the frame.
    let mut harness = Harness::new(2);
    let id = harness.pane_ids()[0];
    let before = glyph_count(&harness.frame());
    assert!(!harness.needs_a_frame(), "the frame is up to date");

    harness.update_session(id, |session| {
        session.derived_title = Some("port the tab bar".to_owned());
        session.status = AgentStatus::Failed;
    });

    assert!(
        harness.needs_a_frame(),
        "a renamed session left the window believing it was up to date"
    );
    assert!(
        glyph_count(&harness.frame()) > before,
        "the frame does not show the longer title"
    );
}

#[test]
fn an_action_that_changes_nothing_does_not_ask_for_a_frame() {
    // A held-down cmd-alt-left on the leftmost tab: every repeat dispatches
    // `MoveLeft`, and every one of them used to rebuild, lay out and paint the
    // whole window for a bar that cannot move.
    let mut harness = Harness::new(1);
    harness.frame();
    assert!(!harness.needs_a_frame());

    harness.dispatch_action(TabAction::MoveLeft);
    harness.dispatch_action(TabAction::MoveRight);
    harness.dispatch_action(TabAction::Select(harness.active_id()));
    assert!(
        !harness.needs_a_frame(),
        "an action that moved nothing asked for a repaint"
    );

    harness.dispatch_action(TabAction::New);
    assert!(harness.needs_a_frame(), "opening a tab has to repaint");
}

/// How many glyphs the frame draws, as a stand-in for "what it says".
fn glyph_count(scene: &Scene) -> usize {
    scene.layers().map(|layer| layer.glyphs.len()).sum()
}

/// The body's panes, in render order.
///
/// A pane has no chrome left to find it by — no corner radius, no border, no
/// margin — and its fill is the terminal's own ground, which for a grid that
/// has not been told otherwise is the same `surface` the header and the tabs
/// panel are painted in. So it is found by what it is *not*: filled like a
/// terminal, and neither bordered (the header's underline, the panel's right
/// edge) nor rounded (the well a pane composes its next command in — see
/// [`composer_boxes`] for that one).
///
/// The one other thing that matches is the grid's own ground, painted inside
/// the pane it belongs to, so a rect contained in another is dropped. Without
/// that a shell test would count every pane twice.
fn panel_boxes(scene: &Scene) -> Vec<RectF> {
    let right = far_edge(scene);
    let candidates: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, bounds)| {
            rect.background == Fill::Solid(theme().surface)
                && rect.border == Border::default()
                && rect.corner_radius == CornerRadius::default()
                // The header is drawn on the same ground and, since the rule
                // under it was taken out, in the same shape. It is told apart
                // the way [`header_box`] tells it apart, so that the two
                // helpers cannot disagree about which rect is which.
                && !is_the_header(bounds, right)
        })
        .map(|(_, bounds)| bounds)
        .collect();
    candidates
        .iter()
        .filter(|bounds| {
            !candidates
                .iter()
                .any(|other| other != *bounds && contains(*other, **bounds))
        })
        .copied()
        .collect()
}

/// Whether `outer` covers every corner of `inner`.
fn contains(outer: RectF, inner: RectF) -> bool {
    outer.min_x() <= inner.min_x()
        && outer.min_y() <= inner.min_y()
        && outer.max_x() >= inner.max_x()
        && outer.max_y() >= inner.max_y()
}

#[test]
fn the_bar_draws_a_chip_per_pane_or_a_chip_per_tab_as_it_is_told() {
    // The setting somebody can watch move. It only moves at all because a tab
    // can now hold more than one pane.
    let mut harness = Harness::new(2);
    assert_eq!(tab_boxes(&harness.frame()).len(), 2);

    harness.dispatch_action(TabAction::Split(Direction::Right));
    assert_eq!(tab_boxes(&harness.frame()).len(), 3, "Panes view");

    harness.set_granularity(Granularity::Tabs);
    assert_eq!(tab_boxes(&harness.frame()).len(), 2, "Tabs view");
}

#[test]
fn splitting_the_active_tab_puts_a_second_panel_in_its_body() {
    let mut harness = Harness::new(1);
    let before = panel_boxes(&harness.frame());
    assert_eq!(before.len(), 1, "an unsplit tab is one panel");

    harness.dispatch_action(TabAction::Split(Direction::Right));

    let after = panel_boxes(&harness.frame());
    assert_eq!(after.len(), 2);
    assert!(
        after[0].max_x() <= after[1].min_x(),
        "a horizontal split stacked its panels instead of putting them side by side"
    );
    assert!(
        after[0].min_y() == after[1].min_y(),
        "a horizontal split's panels do not start at the same height"
    );
}

#[test]
fn a_pane_fills_its_share_of_the_body_and_not_just_the_cells_it_can_draw() {
    // A grid measures to whole cells and stops short of the remainder. While
    // every pane was a rounded card with a twelve-pixel gutter, that ragged
    // edge was hidden; now that the panes meet the window's edges, a pane that
    // sized itself to its content would leave a strip down the right and along
    // the bottom that looks like the terminal and does not focus it when
    // clicked.
    let mut harness = Harness::new(1);
    let panes = panel_boxes(&harness.frame());
    assert_eq!(panes.len(), 1, "an unsplit tab is one pane");
    let pane = panes[0];

    assert_eq!(
        pane.min_x(),
        panel_box(&harness.frame()).max_x(),
        "the pane starts where the panel ends"
    );
    assert_eq!(
        pane.max_x(),
        WINDOW.x(),
        "the pane stops short of the window's right edge"
    );
    assert_eq!(
        pane.max_y(),
        WINDOW.y(),
        "the pane stops short of the window's bottom edge"
    );
    assert!(
        pane.min_y() > 0.,
        "the pane should start below the header, not at the top of the window"
    );

    // And the whole of it is a click target, including the corner a grid of
    // whole cells cannot reach.
    harness.dispatch_action(TabAction::Split(Direction::Right));
    let panes = panel_boxes(&harness.frame());
    let first = harness.active_pane_ids()[0];
    harness.click(
        panes[0].origin() + panes[0].size() - vec2f(2., 2.),
        MouseButton::Left,
    );
    assert_eq!(
        Some(first),
        harness.focused_pane_id(),
        "a click in the bottom-right corner of a pane did not focus it"
    );
}

#[test]
fn a_vertical_split_stacks_its_panels() {
    let mut harness = Harness::new(1);

    harness.dispatch_action(TabAction::Split(Direction::Down));

    let panels = panel_boxes(&harness.frame());
    assert_eq!(panels.len(), 2);
    assert!(
        panels[0].max_y() <= panels[1].min_y(),
        "a vertical split put its panels side by side instead of stacking them"
    );
}

#[test]
fn clicking_a_panel_focuses_the_pane_it_draws() {
    // Warp wraps every leaf of its tree in the same handler
    // (`Activate(pane_id, ActivationReason::Click)`), which is what makes the
    // body itself a way to choose which pane a keystroke goes to.
    let mut harness = Harness::new(1);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    let panes = harness.active_pane_ids();
    assert_eq!(
        harness.focused_pane_id(),
        Some(panes[1]),
        "a split focuses the new pane"
    );

    let panels = panel_boxes(&harness.frame());
    harness.click(center(panels[0]), MouseButton::Left);

    assert_eq!(harness.focused_pane_id(), Some(panes[0]));
    assert_eq!(
        harness.active_pane_ids(),
        panes,
        "clicking a panel closed a pane"
    );
}

#[test]
fn clicking_a_chip_focuses_its_pane_and_brings_its_tab_forward() {
    let mut harness = Harness::new(2);
    let tabs = harness.tab_ids();
    harness.dispatch_action(TabAction::Split(Direction::Right));
    let split = harness.active_pane_ids();
    harness.dispatch_action(TabAction::Select(tabs[0]));

    // The second tab's first pane: the third chip in bar order is the split
    // tab's second pane, so the second chip is the one to aim at.
    let chips = tab_boxes(&harness.frame());
    assert_eq!(chips.len(), 3);
    harness.click(
        chips[1].origin() + vec2f(40., chips[1].height() / 2.),
        MouseButton::Left,
    );

    assert_eq!(harness.active_id(), tabs[1]);
    assert_eq!(harness.focused_pane_id(), Some(split[0]));
}

#[test]
fn the_close_button_on_a_pane_chip_closes_the_pane_and_leaves_its_tab_open() {
    let mut harness = Harness::new(1);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    let panes = harness.active_pane_ids();
    let tabs = harness.tab_ids();

    // Only the selected chip draws a close button until a pointer arrives,
    // and a split focuses the new pane.
    let buttons = close_boxes(&harness.frame());
    assert_eq!(buttons.len(), 1);
    harness.click(center(buttons[0]), MouseButton::Left);

    assert_eq!(harness.active_pane_ids(), [panes[0]]);
    assert_eq!(harness.tab_ids(), tabs, "closing a pane closed its tab");
    assert_eq!(harness.quit_requests.get(), 0);

    // And now the tab's last pane, which takes the tab, which takes the
    // window: the rule lives in one place and runs all the way out.
    harness.dispatch_action(TabAction::ClosePane(panes[0]));
    assert_eq!(harness.quit_requests.get(), 1);
}

#[test]
fn a_close_button_in_tabs_view_closes_the_whole_tab_it_stands_for() {
    // The row is the tab there, so its button is the tab's — which is what
    // Warp's tab-group header button does. Closing only the focused pane would
    // leave the tab on the bar under a different name and look like nothing
    // happened.
    let mut harness = Harness::new(2);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    let tabs = harness.tab_ids();
    harness.set_granularity(Granularity::Tabs);

    let buttons = close_boxes(&harness.frame());
    assert_eq!(buttons.len(), 1, "only the selected row draws one");
    harness.click(center(buttons[0]), MouseButton::Left);

    assert_eq!(harness.tab_ids(), [tabs[0]]);
    assert_eq!(harness.pane_ids().len(), 1);
}

// --- the options menu --------------------------------------------------------

/// A session's name, place and repository, so a row has something to print.
const TITLE: &str = "port the tab bar";
const DIRECTORY: &str = "/opt/crook-tests/app/src";
const BRANCH: &str = "eugen/tab-options";
const PULL_REQUEST: &str = "https://github.com/warpdotdev/warp/pull/14876";

/// Twelve added lines and three removed, so the chip has both tokens.
fn seeded_diff() -> DiffStats {
    DiffStats {
        files_changed: 2,
        lines_added: 12,
        lines_removed: 3,
    }
}

impl Harness {
    /// Gives a pane a known name, place and repository.
    ///
    /// Everything a row can print comes from here, so an assertion about what
    /// the frame says never depends on where the test happens to be running.
    fn seed(&mut self, pane: PaneId, diff: Option<DiffStats>) {
        self.update_session(pane, |session| {
            session.derived_title = Some(TITLE.to_owned());
            session.working_directory = Some(PathBuf::from(DIRECTORY));
        });
        self.record_git(pane, BRANCH, diff);
    }

    /// A harness with one seeded row and the frame already drawn.
    fn seeded() -> Self {
        let mut harness = Self::new(1);
        let pane = harness.pane_ids()[0];
        harness.seed(pane, Some(seeded_diff()));
        harness.frame();
        harness
    }

    /// What the strip says, with the options set to `options`.
    fn text_with(&mut self, options: TabOptions) -> String {
        self.set_options(options);
        strip_text(&self.frame())
    }

    fn set_options(&mut self, options: TabOptions) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| workspace.set_options(options, ctx));
        });
    }
}

#[test]
fn the_empty_space_under_the_list_opens_the_menu_and_a_click_outside_closes_it() {
    let mut harness = Harness::seeded_panel();
    assert!(!harness.is_menu_open());

    let ground = empty_list_space(&harness.frame());
    harness.click(ground, MouseButton::Right);
    assert!(
        harness.is_menu_open(),
        "a secondary press on the empty space under the list opened nothing"
    );

    let popup = menu_box(&harness.frame());
    // Well clear of the popup, and over the body — which must not react.
    let outside = vec2f(20., WINDOW.y() - 20.);
    assert!(!popup.contains_point(outside));
    harness.click(outside, MouseButton::Left);

    assert!(!harness.is_menu_open(), "a click outside left the menu up");
}

#[test]
fn pressing_the_empty_space_again_closes_the_menu_once_rather_than_twice() {
    // The ground is under the modal underlay while the menu is up, so its own
    // handler never fires and the press goes through the dismiss path — which
    // takes the secondary button as well as the primary one, for exactly this
    // gesture. Letting both run would toggle twice in one press and leave the
    // menu looking frozen open.
    let mut harness = Harness::seeded_panel();
    let ground = empty_list_space(&harness.frame());

    harness.click(ground, MouseButton::Right);
    assert!(harness.is_menu_open());
    harness.frame();

    harness.click(ground, MouseButton::Right);
    assert!(
        !harness.is_menu_open(),
        "pressing the empty space again toggled twice"
    );
}

#[test]
fn clicking_an_option_changes_it_and_leaves_the_menu_up() {
    // The popup is a persistent preferences panel: change the title field,
    // turn two chips off, and only then click away.
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::TogglePopup);

    let rows = menu_option_boxes(&harness.frame());
    // Compact, so: three "Pane title as" rows, two "Additional metadata" rows,
    // and "Show details on hover".
    assert_eq!(rows.len(), 6, "the Compact menu draws six option rows");

    // "Working Directory", the second of the three title options.
    harness.click(center(rows[1]), MouseButton::Left);
    assert!(harness.is_menu_open(), "an option click closed the menu");
    assert_eq!(
        PrimaryInfo::WorkingDirectory,
        harness.options().primary_info
    );

    // And again, on a different section, without reopening anything.
    let rows = menu_option_boxes(&harness.frame());
    harness.click(center(rows[5]), MouseButton::Left);
    assert!(harness.is_menu_open());
    assert!(
        !harness.options().show_details_on_hover,
        "the last row did not toggle the detail card"
    );
}

#[test]
fn the_menu_takes_the_clicks_that_would_otherwise_reach_the_window_under_it() {
    let mut harness = Harness::new(1);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    let panes = harness.active_pane_ids();
    let focused = harness.focused_pane_id();
    harness.dispatch_option(OptionsAction::TogglePopup);

    let scene = harness.frame();
    let popup = menu_box(&scene);
    let rows = menu_option_boxes(&scene);
    // The menu is anchored inside the panel, so what it covers is the panel's
    // own rows — which are the clickable thing underneath it.
    assert!(
        tab_boxes(&scene).iter().any(|row| overlaps(*row, popup)),
        "the popup at {popup:?} covers no row, so there is nothing to occlude"
    );

    harness.click(center(rows[0]), MouseButton::Left);

    assert_eq!(PrimaryInfo::Command, harness.options().primary_info);
    assert_eq!(
        harness.focused_pane_id(),
        focused,
        "a click on the menu focused the pane underneath it"
    );
    assert_eq!(harness.active_pane_ids(), panes);
}

#[test]
fn the_menu_offers_additional_metadata_in_compact_and_show_in_expanded() {
    // Two arms of one conditional, which is why no state of the menu has both.
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::TogglePopup);

    let compact = frame_text(&harness.frame());
    assert!(compact.contains("Additional metadata"));
    assert!(!compact.contains("Diff stats"));
    assert!(!compact.contains("PR link"));

    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));

    let expanded = frame_text(&harness.frame());
    assert!(!expanded.contains("Additional metadata"));
    assert!(expanded.contains("Diff stats"));
    assert!(expanded.contains("PR link"));
    assert!(
        expanded.contains("Show details on hover"),
        "the one option every state of the menu shows went missing"
    );
}

#[test]
fn the_menu_never_offers_a_subtitle_that_repeats_the_title() {
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::TogglePopup);
    harness.dispatch_option(OptionsAction::SetPrimaryInfo(PrimaryInfo::Branch));

    let text = frame_text(&harness.frame());
    // "Branch" is still the third title option; what must be gone is the
    // second copy of it under "Additional metadata".
    assert_eq!(
        text.matches("Branch").count(),
        1,
        "the menu offers Branch as both the title and the subtitle"
    );
    assert!(text.contains("Command / Conversation"));
    assert!(text.contains("Working Directory"));
}

// --- what the options do to a row -------------------------------------------

#[test]
fn density_decides_how_much_of_a_row_there_is() {
    let mut harness = Harness::seeded();
    let options = harness.options();

    harness.set_options(TabOptions {
        density: Density::Compact,
        ..options
    });
    let compact = harness.frame();
    let compact_row = tab_boxes(&compact)[0];
    assert!(
        chip_boxes(&compact).is_empty(),
        "a Compact row drew a chip, which is exactly why the menu hides the toggles there"
    );

    harness.set_options(TabOptions {
        density: Density::Expanded,
        ..options
    });
    let expanded = harness.frame();
    let expanded_row = tab_boxes(&expanded)[0];
    assert_eq!(
        chip_boxes(&expanded).len(),
        1,
        "an Expanded row drew no diff chip for a repository with changes"
    );
    assert!(
        expanded_row.height() > compact_row.height(),
        "Expanded is {} tall and Compact is {}",
        expanded_row.height(),
        compact_row.height()
    );
}

#[test]
fn pane_title_as_decides_which_fact_leads_the_row() {
    let mut harness = Harness::seeded();
    let options = TabOptions {
        density: Density::Compact,
        ..harness.options()
    };

    let command = harness.text_with(TabOptions {
        primary_info: PrimaryInfo::Command,
        ..options
    });
    assert!(command.contains(TITLE));
    assert!(
        !command.contains(DIRECTORY),
        "the working directory has no line in this mode"
    );
    assert!(
        command.find(TITLE) < command.find(BRANCH),
        "the branch was drawn above the title"
    );

    let directory = harness.text_with(TabOptions {
        primary_info: PrimaryInfo::WorkingDirectory,
        ..options
    });
    assert!(directory.contains(DIRECTORY));
    assert!(!directory.contains(TITLE));

    let branch = harness.text_with(TabOptions {
        primary_info: PrimaryInfo::Branch,
        ..options
    });
    assert!(branch.contains(BRANCH));
    assert!(
        branch.find(BRANCH) < branch.find(TITLE),
        "Branch mode did not put the branch on the title line"
    );
}

#[test]
fn additional_metadata_decides_what_a_compact_row_says_underneath() {
    let mut harness = Harness::seeded();
    let options = TabOptions {
        density: Density::Compact,
        primary_info: PrimaryInfo::Command,
        ..harness.options()
    };

    let with_branch = harness.text_with(TabOptions {
        subtitle: Subtitle::Branch,
        ..options
    });
    assert!(with_branch.contains(BRANCH));
    assert!(!with_branch.contains(DIRECTORY));

    let with_directory = harness.text_with(TabOptions {
        subtitle: Subtitle::WorkingDirectory,
        ..options
    });
    assert!(with_directory.contains(DIRECTORY));
    assert!(!with_directory.contains(BRANCH));
}

#[test]
fn the_show_toggles_add_and_remove_chips_without_resizing_the_row() {
    let mut harness = Harness::seeded();
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.pull_request = Some(PULL_REQUEST.to_owned());
    });
    let options = TabOptions {
        density: Density::Expanded,
        ..harness.options()
    };

    harness.set_options(options);
    let both = harness.frame();
    let full_height = tab_boxes(&both)[0].height();
    assert_eq!(chip_boxes(&both).len(), 2);
    assert!(frame_text(&both).contains("+12"));
    assert!(frame_text(&both).contains("PR #14876"));

    harness.set_options(TabOptions {
        show_pr_link: false,
        ..options
    });
    let one = harness.frame();
    assert_eq!(chip_boxes(&one).len(), 1);
    assert!(!frame_text(&one).contains("PR #14876"));

    harness.set_options(TabOptions {
        show_pr_link: false,
        show_diff_stats: false,
        ..options
    });
    let none = harness.frame();
    assert!(chip_boxes(&none).is_empty());

    // The metadata line is height-locked precisely so that toggling a chip
    // never reflows the strip.
    assert_eq!(tab_boxes(&one)[0].height(), full_height);
    assert_eq!(tab_boxes(&none)[0].height(), full_height);
}

#[test]
fn show_details_on_hover_opens_a_card_over_the_row_and_closes_with_the_option() {
    let mut harness = Harness::seeded();
    assert!(detail_cards(&harness.frame()).is_empty());

    let row = tab_boxes(&harness.frame())[0];
    harness.move_to(row.origin() + vec2f(40., row.height() / 2.));

    let scene = harness.frame();
    let cards = detail_cards(&scene);
    assert_eq!(cards.len(), 1, "hovering a row opened no detail card");
    assert!(
        cards[0].min_x() >= row.max_x() || cards[0].min_y() >= row.max_y(),
        "the card covers the row it describes"
    );
    assert_eq!(cards[0].width(), 320., "Warp's sidecar width");
    assert!(
        cards[0].max_x() <= WINDOW.x() && cards[0].max_y() <= WINDOW.y(),
        "the card at {:?} hangs outside the window",
        cards[0]
    );
    // What the row could not fit: the whole path, and the chips, whether or
    // not the row's own toggles are on.
    let text = frame_text(&scene);
    assert!(text.contains(DIRECTORY));
    assert!(text.contains("+12"));

    harness.dispatch_option(OptionsAction::ToggleShowDetailsOnHover);
    assert!(
        detail_cards(&harness.frame()).is_empty(),
        "turning the option off left the card that was up on screen"
    );
}

#[test]
fn the_hover_card_shows_the_chips_the_row_was_told_to_hide() {
    // Warp's asymmetry, kept: those two settings govern the row, and the card
    // is what the row could not fit.
    let mut harness = Harness::seeded();
    harness.set_options(TabOptions {
        density: Density::Expanded,
        show_diff_stats: false,
        ..harness.options()
    });

    let row = tab_boxes(&harness.frame())[0];
    harness.move_to(row.origin() + vec2f(40., row.height() / 2.));

    let scene = harness.frame();
    assert_eq!(detail_cards(&scene).len(), 1);
    assert_eq!(
        chip_boxes(&scene).len(),
        1,
        "the row kept its chip, or the card lost it"
    );
}

#[test]
fn an_option_set_to_what_it_already_was_does_not_ask_for_a_frame() {
    let mut harness = Harness::seeded();
    harness.frame();
    assert!(!harness.needs_a_frame());

    // Clicking the selected half of a segmented control is the ordinary way to
    // do this, and it must not put the window on a repaint loop.
    harness.dispatch_option(OptionsAction::SetGranularity(Granularity::Panes));
    assert!(!harness.needs_a_frame());

    harness.dispatch_option(OptionsAction::SetGranularity(Granularity::Tabs));
    assert!(harness.needs_a_frame());
}

#[test]
fn an_option_is_written_to_the_settings_that_get_saved() {
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));
    harness.dispatch_option(OptionsAction::ToggleShowPrLink);

    let saved = harness.workspace.read(&harness.app, |workspace, _| {
        workspace.settings().tab_options()
    });

    assert_eq!(Density::Expanded, saved.density);
    assert!(!saved.show_pr_link);
    assert_eq!(saved, harness.options());
}

// --- the menu as a person uses it, not as an action stream -------------------

#[test]
fn clicking_a_view_as_segment_changes_what_the_strip_draws_a_row_for() {
    // The other option tests write `TabOptions` straight into the workspace;
    // this one goes through the pixels, so it also covers that a segment is
    // where it looks like it is and that a click on it reaches the strip.
    let mut harness = Harness::new(1);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    assert_eq!(tab_boxes(&harness.frame()).len(), 2, "Panes view");

    harness.dispatch_option(OptionsAction::TogglePopup);
    let segments = menu_segment_boxes(&harness.frame());
    assert_eq!(segments.len(), 4, "two segmented controls, two halves each");

    // "Tabs", the right half of the first control.
    harness.click(center(segments[1]), MouseButton::Left);

    assert_eq!(Granularity::Tabs, harness.options().granularity);
    assert_eq!(tab_boxes(&harness.frame()).len(), 1, "Tabs view");
    assert!(harness.is_menu_open(), "a segment click closed the menu");
}

#[test]
fn clicking_a_density_segment_changes_how_much_of_a_row_there_is() {
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::TogglePopup);
    let compact = tab_boxes(&harness.frame())[0].height();

    // "Expanded", the right half of the second control.
    let segments = menu_segment_boxes(&harness.frame());
    harness.click(center(segments[3]), MouseButton::Left);

    assert_eq!(Density::Expanded, harness.options().density);
    let scene = harness.frame();
    assert!(
        tab_boxes(&scene)[0].height() > compact,
        "Expanded is {} tall and Compact was {compact}",
        tab_boxes(&scene)[0].height()
    );
    assert_eq!(
        chip_boxes(&scene).len(),
        1,
        "the diff chip Compact has nowhere to put did not appear"
    );
}

#[test]
fn the_info_note_fits_inside_the_menu_it_belongs_to() {
    // `Text` never wraps and `Stack` lays an anchored child out against the
    // whole window, so the unbroken sentence measured about twice the popup's
    // width and hung over the body on both sides of it.
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));
    harness.dispatch_option(OptionsAction::TogglePopup);

    let scene = harness.frame();
    assert!(info_notes(&scene).is_empty(), "the note is up unhovered");
    let dot = info_dot_box(&scene);
    let popup = menu_box(&scene);

    harness.move_to(center(dot));
    let scene = harness.frame();
    let notes = info_notes(&scene);
    assert_eq!(notes.len(), 1, "hovering the info dot opened no note");
    let note = notes[0];

    assert!(
        note.min_x() >= popup.min_x() && note.max_x() <= popup.max_x(),
        "the note at {note:?} runs outside the popup at {popup:?}"
    );
    assert!(
        note.max_y() <= dot.min_y(),
        "the note at {note:?} was drawn below the dot at {dot:?}, over the \
         rows underneath it"
    );
}

#[test]
fn the_info_note_covers_the_rows_it_is_drawn_over() {
    // An overlay covers only what was painted before it, which is why the note
    // hangs above the dot: below it, it floated over the "Diff stats" row
    // while that row went on hit-testing as if nothing were there.
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));
    harness.dispatch_option(OptionsAction::TogglePopup);

    let dot = info_dot_box(&harness.frame());
    harness.move_to(center(dot));

    let scene = harness.frame();
    let note = info_notes(&scene)[0];
    let covered: Vec<RectF> = menu_option_boxes(&scene)
        .into_iter()
        .filter(|row| note.contains_point(center(*row)))
        .collect();
    assert!(
        !covered.is_empty(),
        "the note covers no row, so there is nothing to occlude"
    );

    let before = harness.options();
    harness.click(center(covered[0]), MouseButton::Left);
    assert_eq!(
        before,
        harness.options(),
        "a click inside the note reached the row underneath it"
    );
}

#[test]
fn a_dot_that_stopped_being_drawn_stops_believing_it_is_hovered() {
    // The dot's mouse state outlives the element, and clicking the dot is
    // claimed by the row underneath — which turns "PR link" off and takes the
    // dot away before it can ever see the pointer leave.
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));
    harness.dispatch_option(OptionsAction::TogglePopup);

    let dot = info_dot_box(&harness.frame());
    harness.move_to(center(dot));
    assert_eq!(info_notes(&harness.frame()).len(), 1);

    // The press lands on the dot and the row claims it, so "PR link" goes off
    // and the dot goes with it.
    harness.click(center(dot), MouseButton::Left);
    assert!(!harness.options().show_pr_link);
    assert!(info_notes(&harness.frame()).is_empty());

    // Back on, from the row's label rather than from the dot: the pointer is
    // nowhere near where the dot will be.
    let rows = menu_option_boxes(&harness.frame());
    let pr_link = rows[3];
    harness.click(
        pr_link.origin() + vec2f(60., pr_link.height() / 2.),
        MouseButton::Left,
    );

    assert!(harness.options().show_pr_link);
    assert!(
        info_notes(&harness.frame()).is_empty(),
        "the note reopened by itself, with the pointer 26px from the dot"
    );
}

// --- what the command line may and may not change ---------------------------

/// A settings file in a directory of this test's own, deleted with it.
///
/// The point of a real path: `Workspace::save_settings` returns without doing
/// anything when there is none, so a test on ephemeral settings cannot see
/// what a save would write — and what a save writes is the whole question for
/// a command-line override.
struct Scratch {
    directory: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // macOS's temporary directory is a symlink into `/private`, and git
        // prints the real path — so a test comparing what git lists with what
        // it made has to start from the real one. Nowhere else is the
        // temporary directory a link, and on Windows the canonical form is
        // the `\\?\` spelling nothing else here writes.
        let temp = std::env::temp_dir();
        let temp = if cfg!(target_os = "macos") {
            fs::canonicalize(&temp).unwrap_or(temp)
        } else {
            temp
        };
        Self {
            directory: temp.join(format!("crook-tab-options-{}-{serial}", std::process::id())),
        }
    }

    /// The directory itself, for a test that wants to put themes in it.
    fn path(&self) -> &Path {
        // Created on demand: a themes folder that does not exist yet is the
        // ordinary state, and every reader here defaults past a missing one —
        // but a test that writes into it needs it to be there.
        let _ = fs::create_dir_all(&self.directory);
        &self.directory
    }

    /// Settings that would be written here. The file does not exist yet, and
    /// `Settings::load` defaults past a missing one.
    fn settings(&self) -> Settings {
        Settings::load(self.directory.join("settings.json"))
    }

    /// The file, once a save has written `needle` into it.
    ///
    /// Waits, because the save is handed to the background pool, and waits for
    /// the *content* rather than for the file to appear: a test that clicks
    /// twice queues two saves, and the first one to land is not the one it is
    /// asking about. Asserting on `Settings::tab_options` instead would stop
    /// one step short of the bytes, and the bytes are what the next launch
    /// reads.
    fn written_containing(&self, needle: &str) -> String {
        let path = self.directory.join("settings.json");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut last = String::new();
        loop {
            last = std::fs::read_to_string(&path).unwrap_or(last);
            if last.contains(needle) {
                return last;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no save put {needle} into {}; it holds {last}",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// The file once it has stopped changing.
    ///
    /// Two reads in a row that agree, rather than a wait for a value: a save
    /// that lands and is then overwritten by an earlier one passes
    /// [`Scratch::written_containing`] if the poll happens to fall in the
    /// window between the two writes, and the whole question here is what the
    /// file holds *afterwards*.
    fn settled(&self) -> String {
        let path = self.directory.join("settings.json");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut before = None;
        loop {
            let now = std::fs::read_to_string(&path).ok();
            if now.is_some() && now == before {
                return now.unwrap_or_default();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{} never stopped changing",
                path.display()
            );
            before = now;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn a_density_the_command_line_asked_for_is_never_written_to_the_settings_file() {
    // `--density expanded` is a way to look at a frame, not a way to change
    // what the next launch does — and every menu click writes the *whole*
    // options snapshot back, so this cannot be arranged by not saving once.
    let scratch = Scratch::new();
    let mut harness = Harness::with_settings(1, scratch.settings());
    assert_eq!(Density::Compact, harness.saved_options().density);

    harness.override_density(Density::Expanded);
    assert_eq!(
        Density::Expanded,
        harness.options().density,
        "the override never reached the renderer"
    );

    // Any click will do. The point is that a click saves the *whole* options
    // snapshot, so one that has nothing to do with density still carries it.
    harness.dispatch_option(OptionsAction::ToggleShowDetailsOnHover);

    let written = scratch.written_containing("\"show_details_on_hover\": false");
    assert!(
        written.contains("\"view_mode\": \"compact\""),
        "the command line's density reached the settings file, so the next \
         launch with no flags starts Expanded; the file says {written}"
    );
    assert_eq!(Density::Compact, harness.saved_options().density);
}

#[test]
fn choosing_the_density_already_on_screen_still_ends_the_override() {
    // The case `set_options` cannot see: with `--density expanded` the menu
    // already shows Expanded selected, so clicking it changes no value — and
    // without a special case the file would go on saying Compact for ever.
    let scratch = Scratch::new();
    let mut harness = Harness::with_settings(1, scratch.settings());
    harness.override_density(Density::Expanded);

    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));

    scratch.written_containing("\"view_mode\": \"expanded\"");
    assert_eq!(harness.options(), harness.saved_options());
}

#[test]
fn choosing_a_different_density_ends_the_override_too() {
    let scratch = Scratch::new();
    let mut harness = Harness::with_settings(1, scratch.settings());
    harness.override_density(Density::Expanded);

    harness.dispatch_option(OptionsAction::SetDensity(Density::Compact));
    assert_eq!(Density::Compact, harness.saved_options().density);

    // And the next click saves the density that is on screen rather than
    // reaching back for the one the file happened to hold.
    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));
    harness.dispatch_option(OptionsAction::ToggleShowPrLink);
    assert_eq!(Density::Expanded, harness.saved_options().density);
    scratch.written_containing("\"view_mode\": \"expanded\"");
}

/// How many times [`two_choices_in_a_row_leave_the_second_one_in_the_file`]
/// plays out its race.
///
/// The failure it is looking for is decided by the scheduler, not by the code
/// path: before the fix, the earlier of two saves overwrote the later one in
/// roughly two rounds out of five on the machine this was written on, so
/// thirty-two rounds miss it about seven times in a hundred million. After the
/// fix it cannot happen at all, so this is not a flaky test in either
/// direction — it is a coin the fix stops flipping.
const RACE_ROUNDS: usize = 32;

#[test]
fn two_choices_in_a_row_leave_the_second_one_in_the_file() {
    // Two clicks, each of which asks for a save of the whole options snapshot,
    // and the two saves run on different workers of the background pool. Both
    // used to be allowed to write — each looked at the save counter before the
    // other had been asked for — and which of the two `rename`s landed last
    // was up to the scheduler. When it was the first, the file settled on the
    // state before the second click and stayed there: the option a person had
    // just chosen was on screen, in memory and in `saved_options`, and simply
    // not in the file the next launch reads.
    //
    // Every other test here waits for the file to *contain* what it asked for,
    // which a lost write can still satisfy for the instant before it is
    // overwritten. This one asks what the file holds once it has stopped
    // moving.
    for round in 0..RACE_ROUNDS {
        let scratch = Scratch::new();
        let mut harness = Harness::with_settings(1, scratch.settings());

        harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));
        harness.dispatch_option(OptionsAction::SetSubtitle(Subtitle::WorkingDirectory));

        let settled = scratch.settled();
        assert!(
            settled.contains("\"compact_subtitle\": \"working_directory\""),
            "round {round}: the save from the first click overwrote the one \
             from the second, so the file holds {settled}"
        );
        // And the first click is still in it: the second save carries the whole
        // snapshot, so a file that lost the first choice would mean the two
        // saves had been ordered by dropping one of them rather than by
        // sequencing them.
        assert!(
            settled.contains("\"view_mode\": \"expanded\""),
            "round {round}: the last save did not carry the earlier choice; \
             the file holds {settled}"
        );
        assert_eq!(harness.options(), harness.saved_options());
    }
}

#[test]
fn every_mark_in_the_chrome_is_an_icon_rather_than_a_codepoint() {
    // The rule this file exists to keep: a mark is geometry Crook ships, not a
    // character it hopes the machine has a font for. The codepoints below are
    // the ones that used to be drawn here, and a `Text` carrying any of them
    // is a mark that will be a different shape on somebody else's machine —
    // or, for the settings mark, a colour emoji.
    let mut harness = Harness::seeded();

    // The tab list first: the branch mark is a row's, and rows are only drawn
    // while the tabs are the section showing.
    let tabs = icons_of(&harness.frame());
    for expected in [Lucide::Plus, Lucide::X, Lucide::GitBranch] {
        assert!(
            tabs.contains(&expected),
            "the tab list draws no {expected:?}, only {tabs:?}"
        );
    }

    harness.open_settings_page();
    let scene = harness.frame();
    let drawn = icons_of(&scene);
    assert!(
        drawn.contains(&Lucide::Settings),
        "the settings draw no settings mark, only {drawn:?}"
    );

    let text = frame_text(&scene);
    for codepoint in ['\u{00d7}', '\u{2715}', '\u{2699}', '\u{2713}', '\u{FF0B}'] {
        assert!(
            !text.contains(codepoint),
            "{codepoint:?} is still being drawn as text"
        );
    }
}

/// A git repository in `directory`, or `None` where git is not installed.
///
/// One commit, because `git worktree list` on a repository with no commits at
/// all answers about a checkout with an unborn HEAD — a real state, and not
/// the one this is testing.
fn scratch_repository(directory: &Path) -> Option<PathBuf> {
    fs::create_dir_all(directory).ok()?;
    let run = |args: &[&str]| {
        crate::process::command("git")
            .args(args)
            .current_dir(directory)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()
            .filter(std::process::ExitStatus::success)
    };

    run(&["init", "--quiet"])?;
    run(&["config", "user.email", "crook@example.invalid"])?;
    run(&["config", "user.name", "crook"])?;
    fs::write(directory.join("README"), "worktree test\n").ok()?;
    run(&["add", "-A"])?;
    run(&["commit", "--quiet", "-m", "one"])?;
    Some(directory.to_path_buf())
}

/// The worktree row whose branch label begins with `prefix`, by its box.
///
/// The rows are the menu's only hoverable full-width bands, so a row is found
/// by the glyphs on it and returned as the band they sit in.
fn worktree_row_saying(scene: &Scene, prefix: &str) -> RectF {
    let menu = worktree_menu_box(scene).expect("the menu is not up");
    let baseline = scene
        .layers()
        .flat_map(|layer| layer.glyphs.iter())
        .filter(|glyph| menu.contains_point(glyph.position))
        .map(|glyph| glyph.position.y())
        .find(|y| {
            text_where(scene, |position| {
                menu.contains_point(position) && (position.y() - y).abs() < 0.5
            })
            .contains(prefix)
        })
        .unwrap_or_else(|| panic!("no row of the menu begins with {prefix:?}"));

    RectF::new(
        vec2f(menu.min_x() + 8., baseline - 3.),
        vec2f(menu.width() - 16., 6.),
    )
}

/// The × a hovered worktree row offers, by its 16px square.
///
/// Drawn only while the pointer is on a removable row, so the frame it is
/// found in has to be one taken with that row hovered.
fn worktree_remove_cross(scene: &Scene) -> RectF {
    let menu = worktree_menu_box(scene).expect("the menu is not up");
    scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .filter(|icon| icon.icon_key.mark == Mark::Icon(Lucide::X))
        .map(|icon| icon.bounds)
        .find(|bounds| menu.contains_point(center(*bounds)))
        .expect("no row offers a ×")
}

/// The worktree menu the active tab opens, by its popup box.
///
/// Found the way the options menu is: the one surface-raised, 6px-rounded box
/// that is wide enough to be it. The options menu is 200 wide and this is 260,
/// which is what tells the two apart in a frame that could hold either.
fn worktree_menu_box(scene: &Scene) -> Option<RectF> {
    visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                && rect.background == Fill::Solid(theme().surface_raised)
                && (rect.bounds.width() - super::tab_menu::MENU_WIDTH).abs() < 0.5
        })
        .map(|(_, bounds)| bounds)
        .next()
}

/// Whether the worktree menu says `needle` anywhere on it.
///
/// The window behind the popup says "main" too, so every question about what
/// this menu says has to be asked inside the box it is drawn in.
fn worktree_menu_says(scene: &Scene, needle: &str) -> bool {
    let menu = worktree_menu_box(scene).expect("the menu is not up");
    text_where(scene, |position| menu.contains_point(position)).contains(needle)
}

/// Every rect painted in `fill`, which for a tab colour is its stripe.
fn stripes_of(scene: &Scene, fill: Color) -> Vec<RectF> {
    visible_rects(scene)
        .filter(|(rect, _)| rect.background == Fill::Solid(fill))
        .map(|(_, bounds)| bounds)
        .collect()
}

/// The popup a tab's secondary press opens, by its box.
///
/// Told apart from the worktree list hanging off it by its width, which is the
/// only thing about the two columns that differs: a worktree row carries a
/// path and needs the extra 28px, and everything else — the radius, the
/// ground, the inset — is deliberately shared so that the pair reads as one
/// menu.
fn tab_menu_box(scene: &Scene) -> Option<RectF> {
    visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                && rect.background == Fill::Solid(theme().surface_raised)
                && (rect.bounds.width() - super::tab_context_menu::MENU_WIDTH).abs() < 0.5
        })
        .map(|(_, bounds)| bounds)
        .next()
}

/// The entry of that menu whose label begins with `prefix`, by its band.
///
/// `worktree_row_saying`'s shape, against the other menu: the entries are the
/// popup's only full-width bands, so one is found by the glyphs on it.
fn tab_menu_row_saying(scene: &Scene, prefix: &str) -> RectF {
    let menu = tab_menu_box(scene).expect("no tab menu is up");
    let baseline = scene
        .layers()
        .flat_map(|layer| layer.glyphs.iter())
        .filter(|glyph| menu.contains_point(glyph.position))
        .map(|glyph| glyph.position.y())
        .find(|y| {
            text_where(scene, |position| {
                menu.contains_point(position) && (position.y() - y).abs() < 0.5
            })
            .contains(prefix)
        })
        .unwrap_or_else(|| panic!("no entry of the menu begins with {prefix:?}"));

    RectF::new(
        vec2f(menu.min_x() + 8., baseline - 3.),
        vec2f(menu.width() - 16., 6.),
    )
}

/// Whether that menu is offering an entry beginning with `prefix`.
fn tab_menu_offers(scene: &Scene, prefix: &str) -> bool {
    let Some(menu) = tab_menu_box(scene) else {
        return false;
    };
    text_where(scene, |position| menu.contains_point(position)).contains(prefix)
}

#[test]
fn right_clicking_a_tab_opens_its_menu() {
    // The gesture, and the whole of why it is this one: it is the button a
    // context menu opens on everywhere else. The left one used to do it — on
    // the row you were already in, where the click was otherwise free — and
    // that made the menu something you opened by accident on the way to the
    // tab you were already in.
    let mut harness = Harness::seeded();
    let scene = harness.frame();
    assert!(
        tab_menu_box(&scene).is_none(),
        "the menu was up before anything was clicked"
    );

    let tab = tab_boxes(&scene)[0];
    harness.click(center(tab), MouseButton::Right);

    assert!(
        tab_menu_box(&harness.frame()).is_some(),
        "a right press on a row did not open its menu"
    );
}

#[test]
fn the_worktrees_entry_opens_the_list_beside_the_menu() {
    // The submenu, through the pointer: the entry that used to *be* this
    // gesture is now one row of what it opens. Beside rather than below,
    // because below a menu row is the next menu row.
    let mut harness = Harness::seeded();
    let pane = harness.pane_ids()[0];
    harness.record_git(pane, BRANCH, None);

    let tab = tab_boxes(&harness.frame())[0];
    harness.click(center(tab), MouseButton::Right);

    let scene = harness.frame();
    let menu = tab_menu_box(&scene).expect("the menu did not open");
    harness.click(
        center(tab_menu_row_saying(&scene, "Worktrees")),
        MouseButton::Left,
    );

    let scene = harness.frame();
    let list = worktree_menu_box(&scene).expect("the entry opened no worktree list");
    assert!(
        tab_menu_box(&scene).is_some(),
        "opening the submenu took the menu it hangs off down with it"
    );
    assert!(
        list.min_x() >= menu.min_x(),
        "the list opened at {} and the menu it hangs off starts at {}",
        list.min_x(),
        menu.min_x()
    );
}

#[test]
fn left_clicking_the_tab_you_are_already_in_opens_nothing() {
    // The other half of moving the gesture: the primary button focuses and
    // does nothing else, on the active row as much as on any other.
    let mut harness = Harness::seeded();
    let tab = tab_boxes(&harness.frame())[0];

    harness.click(center(tab), MouseButton::Left);

    assert!(
        tab_menu_box(&harness.frame()).is_none(),
        "a left click on the active row opened its menu"
    );
}

#[test]
fn right_clicking_it_again_takes_the_menu_down() {
    let mut harness = Harness::seeded();
    let tab = tab_boxes(&harness.frame())[0];

    harness.click(center(tab), MouseButton::Right);
    harness.frame();
    harness.click(center(tab), MouseButton::Right);

    assert!(
        tab_menu_box(&harness.frame()).is_none(),
        "a second right press on the same row left the menu up"
    );
}

#[test]
fn a_tab_outside_a_repository_is_offered_no_worktrees() {
    // "If it is under git, there should be worktree options" — and if it is
    // not, that entry is not there. This used to cost the whole gesture: the
    // menu *was* the worktrees, so a tab outside a repository opened nothing
    // at all. Now the rule costs exactly the row it was ever about, and the
    // other four entries are as true of this tab as of any.
    let mut harness = Harness::new(1);
    let scene = harness.frame();
    let tab = tab_boxes(&scene)[0];

    harness.click(center(tab), MouseButton::Right);

    let scene = harness.frame();
    assert!(
        tab_menu_box(&scene).is_some(),
        "a tab with no repository behind it opened no menu at all"
    );
    assert!(
        !tab_menu_offers(&scene, "Worktrees"),
        "a tab with no repository behind it was offered its worktrees"
    );
    assert!(
        tab_menu_offers(&scene, "Close tab"),
        "the entries that are about any tab went with the one that is not"
    );
}

#[test]
fn clicking_a_tab_that_is_not_the_active_one_still_just_selects_it() {
    // The primary button is how a person changes tabs, and it must stay that
    // on every row.
    let mut harness = Harness::seeded();
    harness.dispatch_action(TabAction::New);
    let pane = harness.pane_ids()[0];
    harness.record_git(pane, BRANCH, None);
    let scene = harness.frame();

    let first = tab_boxes(&scene)[0];
    harness.click(center(first), MouseButton::Left);
    let scene = harness.frame();

    assert!(
        tab_menu_box(&scene).is_none(),
        "clicking away from the active tab opened a menu instead of selecting"
    );
    assert_eq!(
        harness.focused_pane_id(),
        Some(pane),
        "and it did not select the tab that was clicked"
    );
}

#[test]
fn right_clicking_a_tab_that_is_not_the_active_one_opens_its_menu() {
    // A context menu is about the row it landed on, not about the row you
    // happen to be in — the restriction to the active tab was there only
    // because the gesture shared the primary button with selecting.
    let mut harness = Harness::seeded();
    harness.dispatch_action(TabAction::New);
    let inactive = harness.pane_ids()[0];
    harness.record_git(inactive, BRANCH, None);
    let focused = harness.focused_pane_id();
    let scene = harness.frame();

    let first = tab_boxes(&scene)[0];
    harness.click(center(first), MouseButton::Right);

    assert!(
        tab_menu_box(&harness.frame()).is_some(),
        "a right press on an inactive tab opened no menu"
    );
    assert_eq!(
        harness.focused_pane_id(),
        focused,
        "and it changed tabs, which a context menu does not do"
    );
}

#[test]
fn the_menu_takes_the_keyboard_away_from_the_pane_under_it() {
    // The rule every popup in this window obeys: while one is up the grid
    // keeps only the three signal keys, and no field has the keyboard. A menu
    // that let typing through would be typing into a shell nobody can see.
    let mut harness = Harness::seeded();
    let tab = tab_boxes(&harness.frame())[0];

    harness.click(center(tab), MouseButton::Right);
    harness.frame();

    assert!(
        harness.a_popup_is_open(),
        "the menu is up but nothing in the window knows it"
    );
}

#[test]
fn the_menu_reads_the_repository_the_click_landed_on() {
    // The click, the background read, and the answer arriving — the half of
    // the path the state-level test below does not go through a pointer for.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(scratch.path()) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };

    let mut harness = Harness::seeded();
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);

    let tab = tab_boxes(&harness.frame())[0];
    harness.click(center(tab), MouseButton::Right);
    let scene = harness.frame();
    harness.click(
        center(tab_menu_row_saying(&scene, "Worktrees")),
        MouseButton::Left,
    );
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });

    assert_eq!(
        harness.worktrees_listed(),
        Some(1),
        "a fresh repository has one checkout"
    );
}

#[test]
fn the_worktree_list_is_walked_and_opened_with_the_keyboard() {
    // The list was the deepest pointer-only thing in the window: opening a
    // checkout was a press on a row, and the × that offers to remove one was
    // drawn on hover, so neither had a key at all.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(scratch.path()) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };

    let mut harness = Harness::seeded();
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);

    let tab = harness
        .workspace
        .read(&harness.app, |workspace, _| workspace.tabs().active_id());
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });
    harness.frame();

    let selected = |harness: &Harness| {
        harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.tab_menu().selected)
    };
    assert_eq!(selected(&harness), None, "the list opens with nothing lit");

    // Down from nothing is the first row, and a clamped list stays on it.
    assert!(harness.press_key("down", Modifiers::default()));
    assert_eq!(selected(&harness), Some(0));
    assert!(harness.press_key("up", Modifiers::default()));
    assert_eq!(selected(&harness), Some(0), "the top is as far as up goes");

    // The one checkout a fresh repository has is its main one, which is
    // exactly the row the × is never drawn on — so Delete must not open a
    // question git is certain to refuse.
    assert!(harness.press_key("delete", Modifiers::default()));
    assert!(
        !harness.worktree_menu_is_confirming(),
        "Delete asked about a checkout the × is not drawn on"
    );

    // Enter on the checkout this tab is already in brings it forward, which
    // closes the menu the way the row's own press does.
    assert!(harness.press_key("enter", Modifiers::default()));
    assert!(
        harness
            .workspace
            .read(&harness.app, |workspace, _| !workspace
                .worktree_menu_is_open()),
        "Enter left the menu standing"
    );
}

#[test]
fn a_tabs_menu_is_the_entries_its_plugins_put_in_it() {
    // The shell knows no entry by name, so this list is the whole of what a
    // tab's menu is — and it comes from two plugins rather than one, which is
    // the fact the slot exists to make true. `crook/worktrees` is last because
    // it asked for the band after the one "Close tab" is alone in.
    let harness = Harness::seeded();

    assert_eq!(
        harness.tab_menu_entries(),
        [
            "crook/tabs/pin-tab",
            "crook/tabs/new-group-with-tab",
            "crook/tabs/copy-pane-title",
            "crook/tabs/copy-working-directory",
            "crook/tabs/rename-tab",
            "crook/tabs/rename-pane",
            "crook/tabs/close-tab",
            "crook/worktrees/menu",
            "crook/tabs/color",
        ],
        "the menu is not the entries its plugins contributed, in band order"
    );
}

#[test]
fn a_secondary_press_opens_the_menu_on_the_row_it_was_made_on() {
    // The menu used to be about the tab and opened on the focused pane's row
    // alone. Half its entries now name a *pane*, so which row was pressed is a
    // fact it has to keep — and a second press on the same row closes it,
    // which is what anything opened by being pressed does.
    let mut harness = Harness::seeded();
    harness.dispatch_action(TabAction::Split(Direction::Right));
    harness.frame();

    let tab = harness.active_id();
    let panes = harness.pane_ids();
    let (first, second) = (panes[0], panes[1]);

    harness.open_tab_menu_on(tab, second);
    assert_eq!(harness.tab_menu_row(), Some(second));

    harness.open_tab_menu_on(tab, first);
    assert_eq!(
        harness.tab_menu_row(),
        Some(first),
        "pressing another row moved the menu rather than opening a second one"
    );

    harness.open_tab_menu_on(tab, first);
    assert_eq!(
        harness.tab_menu_row(),
        None,
        "pressing the row whose menu is up did not close it"
    );
}

#[test]
fn the_menu_is_about_the_row_it_was_opened_on_rather_than_the_focused_one() {
    // What "Copy pane title" copies. A split leaves the second pane focused,
    // so a menu opened on the first row that answered with the focused pane
    // would copy the wrong one — and the test would still pass with no menu
    // open at all, which is why the fallback is checked here too.
    let mut harness = Harness::seeded();
    harness.dispatch_action(TabAction::Split(Direction::Right));
    harness.frame();

    let tab = harness.active_id();
    let first = harness.pane_ids()[0];
    let focused = harness.focused_pane_id().expect("a split tab has a focus");
    assert_ne!(first, focused, "the split did not move the focus");

    harness.open_tab_menu_on(tab, first);
    assert_eq!(
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace.menu_target().map(|(_, pane)| pane)
        }),
        Some(first)
    );

    harness.dispatch_workspace_action(WorkspaceAction::TabMenu(TabMenuAction::Close));
    assert_eq!(
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace.menu_target().map(|(_, pane)| pane)
        }),
        Some(focused),
        "with no menu up a command means the tab a person is looking at"
    );
}

#[test]
fn close_tab_closes_the_tab_its_menu_is_on() {
    // The entry is a named action, so it is reachable from the palette and
    // from a chord as well as from the row — and all three have to mean the
    // same tab. A handler that closed `active_id` would be right twice and
    // wrong on the gesture the entry actually exists for.
    let mut harness = Harness::seeded();
    harness.dispatch_action(TabAction::New);
    harness.frame();

    let tabs = harness.tab_ids();
    let (first, active) = (tabs[0], harness.active_id());
    assert_ne!(first, active, "the new tab is the active one");

    let pane = harness.workspace.read(&harness.app, |workspace, _| {
        workspace
            .tabs()
            .get(first)
            .map(|tab| tab.panes().focused_id())
            .expect("the first tab is still open")
    });
    harness.open_tab_menu_on(first, pane);
    harness.run_command("crook/tabs/close-tab");
    harness.frame();

    assert_eq!(
        harness.tab_ids(),
        [active],
        "the entry closed the active tab rather than the one its menu was on"
    );
    assert!(
        !harness.a_popup_is_open(),
        "the menu outlived the tab it was open on"
    );
}

#[test]
fn escape_takes_the_submenu_down_before_the_menu() {
    // One key, one step back, twice — the rule the worktree menu already
    // followed inside itself, extended over the menu that now holds it. A
    // first press that closed both would throw away a menu a person had only
    // wanted to back out of one level of.
    let mut harness = Harness::seeded();
    let tab = harness.active_id();
    let pane = harness
        .focused_pane_id()
        .expect("the seeded tab has a pane");
    harness.record_git(pane, BRANCH, None);
    harness.frame();

    harness.open_tab_menu_on(tab, pane);
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    assert!(
        harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.tab_menu().is_open()),
        "the submenu did not open"
    );

    assert!(harness.press_key("escape", Modifiers::default()));
    assert_eq!(
        harness.tab_menu_row(),
        Some(pane),
        "the first Escape took the menu down as well as its submenu"
    );

    assert!(harness.press_key("escape", Modifiers::default()));
    assert_eq!(
        harness.tab_menu_row(),
        None,
        "the second Escape did nothing"
    );
    assert!(!harness.a_popup_is_open());
}

#[test]
fn a_rename_takes_the_keyboard_and_gives_it_back() {
    // The whole of why the host has a field registry. Nothing in the window's
    // own source names this field: `sync_input_keys` asks the host, the host
    // asks the plugin that claimed it, and the plugin answers from state the
    // workspace has never heard of.
    let mut harness = Harness::seeded();
    let tab = harness.active_id();
    let pane = harness
        .focused_pane_id()
        .expect("the seeded tab has a pane");
    let was = harness.tab_name(tab);

    harness.open_tab_menu_on(tab, pane);
    harness.run_command("crook/tabs/rename-tab");
    // The field is only in the tree once the menu has been drawn with it, and
    // a keystroke lands in an element rather than in a state.
    harness.frame();

    harness.type_text("release work");
    assert!(
        harness.press_key("enter", Modifiers::default()),
        "enter was not claimed by the field being typed into"
    );

    assert_eq!(harness.tab_name(tab), "release work");
    assert_ne!(was, "release work", "the tab was already called that");
    assert!(
        !harness.a_popup_is_open(),
        "committing a rename left the menu up"
    );
}

#[test]
fn escape_abandons_a_rename_and_leaves_the_menu_standing() {
    // One step back, which is the rule everywhere else in this menu: the first
    // Escape undoes the gesture that opened the field, not the one that opened
    // the menu the field is in.
    let mut harness = Harness::seeded();
    let tab = harness.active_id();
    let pane = harness
        .focused_pane_id()
        .expect("the seeded tab has a pane");
    let was = harness.tab_name(tab);

    harness.open_tab_menu_on(tab, pane);
    harness.run_command("crook/tabs/rename-tab");
    // The field is only in the tree once the menu has been drawn with it, and
    // a keystroke lands in an element rather than in a state.
    harness.frame();
    harness.type_text("nope");
    assert!(harness.press_key("escape", Modifiers::default()));

    assert_eq!(harness.tab_name(tab), was, "escape renamed it anyway");
    assert_eq!(
        harness.tab_menu_row(),
        Some(pane),
        "escape took the menu down as well as the field"
    );
}

#[test]
fn a_person_s_name_for_a_pane_beats_the_one_its_agent_chose() {
    // The precedence, through the gesture. An agent that renames its work
    // every few turns would otherwise take the name back within the minute,
    // and somebody who typed one would have no way to make it stick.
    let mut harness = Harness::seeded();
    let tab = harness.active_id();
    let pane = harness
        .focused_pane_id()
        .expect("the seeded tab has a pane");

    harness.open_tab_menu_on(tab, pane);
    harness.run_command("crook/tabs/rename-pane");
    harness.frame();
    harness.type_text("the long build");
    assert!(harness.press_key("enter", Modifiers::default()));
    assert_eq!(harness.pane_title(pane), "the long build");

    harness.update_session(pane, |session| {
        session.derived_title = Some("summarising the diff".to_owned());
    });
    assert_eq!(
        harness.pane_title(pane),
        "the long build",
        "the agent took the name back"
    );
}

#[test]
fn emptying_the_field_puts_back_the_name_it_started_with() {
    // A person who clears the box is asking for the name they had before they
    // touched it, not for a row with no name — which is what a tab whose name
    // is the empty string would be.
    let mut harness = Harness::seeded();
    let tab = harness.active_id();
    let pane = harness
        .focused_pane_id()
        .expect("the seeded tab has a pane");
    let born_as = harness.tab_name(tab);

    harness.open_tab_menu_on(tab, pane);
    harness.run_command("crook/tabs/rename-tab");
    // The field is only in the tree once the menu has been drawn with it, and
    // a keystroke lands in an element rather than in a state.
    harness.frame();
    harness.type_text("something else");
    assert!(harness.press_key("enter", Modifiers::default()));
    assert_eq!(harness.tab_name(tab), "something else");

    harness.open_tab_menu_on(tab, pane);
    harness.run_command("crook/tabs/rename-tab");
    harness.frame();
    // The name is selected when the field opens, so one Backspace empties it.
    // Through `press` rather than `press_key`: a key the field takes is one
    // nothing in the window claimed, which is what `press_key` reports on.
    harness.press("backspace", Modifiers::default(), "");
    assert!(harness.press_key("enter", Modifiers::default()));

    assert_eq!(
        harness.tab_name(tab),
        born_as,
        "an emptied field left the tab with no name of its own"
    );
}

#[test]
fn a_colour_puts_a_stripe_on_the_rows_of_the_tab_that_has_one() {
    // On the leading edge rather than on the status disc, which is already
    // saying what the agent is doing: a disc that carried a colour as well
    // would be a red tab and a failed agent telling the same story with the
    // same pixels.
    let mut harness = Harness::seeded_panel();
    let tab = harness.active_id();
    let stripe = theme().terminal.bright[crate::tab::TabColor::Magenta.index()];
    assert!(
        stripes_of(&harness.frame(), stripe).is_empty(),
        "a row was already wearing the colour nothing has been given"
    );

    harness.dispatch_action(TabAction::SetColor {
        tab,
        color: Some(crate::tab::TabColor::Magenta),
    });

    let painted = stripes_of(&harness.frame(), stripe);
    assert_eq!(
        painted.len(),
        1,
        "the colour did not land on exactly one row"
    );
    assert!(
        painted[0].height() > painted[0].width(),
        "the mark is {}x{} — a stripe is taller than it is wide",
        painted[0].width(),
        painted[0].height()
    );
}

#[test]
fn a_tabs_menu_offers_to_unpin_a_tab_that_is_pinned() {
    // One entry and one action for both directions, because they are one
    // gesture. The label says what pressing it will do rather than what is
    // true: "Pinned" would be a row a person has to work out the verb for.
    let mut harness = Harness::seeded();
    let tab = harness.active_id();
    let pane = harness
        .focused_pane_id()
        .expect("the seeded tab has a pane");

    harness.open_tab_menu_on(tab, pane);
    assert!(tab_menu_offers(&harness.frame(), "Pin tab"));

    harness.run_command("crook/tabs/pin-tab");
    harness.open_tab_menu_on(tab, pane);

    let scene = harness.frame();
    assert!(
        tab_menu_offers(&scene, "Unpin tab"),
        "a pinned tab was still being offered a pin"
    );
    assert!(harness.workspace.read(&harness.app, |workspace, _| {
        workspace
            .tabs()
            .get(tab)
            .is_some_and(crate::tab::Tab::is_pinned)
    }));
}

#[test]
fn the_worktrees_row_stays_while_its_list_is_up() {
    // What says a tab is in a repository is a model the background pool fills
    // in, so the frame that opens the list can be a frame that has not been
    // told about the repository yet. An entry that vanished under its own
    // submenu would leave a column hanging off nothing — and the row a person
    // pressed would be the one thing missing from the menu they pressed it in.
    // A tab with a directory and no git facts, which is exactly the state the
    // gather chain leaves behind between asking and answering.
    let mut harness = Harness::new(1);
    let tab = harness.active_id();
    let pane = harness.focused_pane_id().expect("the tab has a pane");
    let directory = std::env::current_dir().expect("a working directory");
    harness.update_session(pane, |session| {
        session.working_directory = Some(directory);
    });
    harness.frame();

    harness.open_tab_menu_on(tab, pane);
    assert!(
        !tab_menu_offers(&harness.frame(), "Worktrees"),
        "a tab nothing has said is in a repository was offered its worktrees"
    );

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    assert!(
        tab_menu_offers(&harness.frame(), "Worktrees"),
        "the row the list hangs off went missing under it"
    );
}

#[test]
fn closing_the_menu_takes_its_submenu_with_it() {
    // The submenu is drawn *inside* this popup rather than as one of its own,
    // so a worktree list left standing over a menu that has gone would hang
    // off nothing — and nothing in the window could reach it to dismiss it.
    let mut harness = Harness::seeded();
    let tab = harness.active_id();
    let pane = harness
        .focused_pane_id()
        .expect("the seeded tab has a pane");
    harness.record_git(pane, BRANCH, None);
    harness.frame();

    harness.open_tab_menu_on(tab, pane);
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.dispatch_workspace_action(WorkspaceAction::TabMenu(TabMenuAction::Close));

    assert!(
        !harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.tab_menu().is_open()),
        "the worktree list outlived the menu that opened it"
    );
    assert!(!harness.a_popup_is_open());
}

#[test]
fn opening_a_tabs_menu_closes_the_options_menu() {
    // Two popups are never up at once, and the reason is not tidiness: a modal
    // underlay covers only the layers painted before it, so the second one
    // would float above the first one's underlay while that underlay ate the
    // press meant to dismiss it. See `a_popup_is_open`.
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::TogglePopup);
    assert!(harness.a_popup_is_open(), "the options menu did not open");

    let tab = harness.active_id();
    let pane = harness
        .focused_pane_id()
        .expect("the seeded tab has a pane");
    harness.open_tab_menu_on(tab, pane);

    assert_eq!(harness.tab_menu_row(), Some(pane));
    assert!(
        !harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.menu().open),
        "the options menu is still up behind a tab's menu"
    );
}

#[test]
fn closing_the_tab_a_menu_is_open_on_takes_the_menu_with_it() {
    // `tab_menu.tab` *is* the open flag, so a tab that closed under its own
    // menu would leave a popup nothing can dismiss — and a window where
    // `a_popup_is_open` is true forever is a window where no pane ever gets
    // the keyboard again.
    let mut harness = Harness::seeded();
    harness.dispatch_action(TabAction::New);
    let tab = harness.active_id();
    let pane = harness.focused_pane_id().expect("the new tab has a pane");
    harness.record_git(pane, BRANCH, None);
    harness.frame();

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    assert!(harness.a_popup_is_open(), "the menu did not open");

    harness.dispatch_action(TabAction::Close(tab));
    harness.frame();

    assert!(
        !harness.a_popup_is_open(),
        "the menu outlived the tab it was open on"
    );
}

#[test]
fn the_cross_on_a_row_asks_about_removing_it_rather_than_opening_it() {
    // The × is a descendant of the row, and a `Hoverable` runs its own click
    // handler whether or not a child already handled the release. Without a
    // guard the × dispatches `AskRemove` and the row dispatches `Show` on top
    // of it: the confirmation is unreachable, and a tab opens in the very
    // checkout somebody was asking to delete. The theme panel paid for this
    // lesson once already.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };
    let store = scratch.path().join("store");

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| workspace.set_worktrees_directory(store.clone()));
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);
    harness.frame();

    // A second checkout to have something removable: the main one never is.
    harness.dispatch_worktree(WorktreeAction::OpenMenu(harness.active_id()));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    let before = harness.pane_ids().len();
    harness.dispatch_worktree(WorktreeAction::Create);
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.pane_ids().len() > before
    });

    // The pane that opened on it has to go before the × will be offered: a
    // checkout somebody is working in is exactly the one that must not be
    // removable, and that rule is what this test would otherwise trip over.
    let opened = harness
        .focused_pane_id()
        .expect("the split focused its pane");
    harness.dispatch_action(TabAction::ClosePane(opened));

    // Back on the first tab's menu, with two checkouts in it now.
    let first = harness.tab_ids()[0];
    harness.dispatch_action(TabAction::Select(first));
    harness.dispatch_worktree(WorktreeAction::OpenMenu(first));
    harness.wait_for("the repository to be read again", |harness| {
        harness.worktrees_listed() == Some(2)
    });

    // The × is drawn only while the pointer is on a removable row, so the row
    // has to be hovered before there is anything to click.
    let scene = harness.frame();
    harness.move_to(center(worktree_row_saying(&scene, "worktree/")));
    let scene = harness.frame();
    let cross = worktree_remove_cross(&scene);

    // Onto the × itself before pressing it, which is what a pointer does and
    // what the guard reads: the row declines a press the × is hovering over,
    // exactly as a tab declines one its close button is under.
    harness.move_to(center(cross));
    harness.frame();

    let panes = harness.pane_ids().len();
    harness.click(center(cross), MouseButton::Left);

    assert!(
        harness.worktree_menu_is_confirming(),
        "the × did not open the confirmation"
    );
    assert_eq!(
        harness.pane_ids().len(),
        panes,
        "the row's own click fired too and opened a pane"
    );
}

#[test]
fn making_a_worktree_checks_it_out_and_opens_a_tab_in_the_group() {
    // The whole feature with a real repository and a real `git worktree add`
    // at the end of it: the menu reads the repository, the creator names a
    // branch nothing is using, git checks it out, and a *tab* opens whose
    // shell would start there — folded under one heading with the tab the menu
    // was opened on, and never a second pane inside it. Belonging together and
    // sharing a rectangle are two different claims and only the first is
    // true.
    //
    // Asserted against the workspace's own state rather than against pixels.
    // A glyph under a popup is still in the scene — the tab behind this menu
    // says "main" too — so a frame cannot answer "has the menu read the
    // repository" without answering "is that word the row's or the menu's".
    // The clicks that reach these controls are covered by the tests above.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };
    let store = scratch.path().join("store");

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| workspace.set_worktrees_directory(store.clone()));
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);
    harness.frame();

    harness.dispatch_worktree(WorktreeAction::OpenMenu(harness.active_id()));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });
    assert_eq!(
        harness.worktrees_listed(),
        Some(1),
        "a fresh repository has one checkout and the menu should say so"
    );

    let tab = harness.active_id();
    let tabs = harness.tab_ids().len();
    let before = harness.pane_ids().len();
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    assert!(
        harness.worktree_menu_is_creating(),
        "the creator did not open"
    );

    harness.dispatch_worktree(WorktreeAction::Create);
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.pane_ids().len() > before
    });

    assert_eq!(
        harness.tab_ids().len(),
        tabs + 1,
        "the checkout did not open a tab"
    );
    assert_eq!(
        harness.panes_of(tab).len(),
        1,
        "the tab the menu was opened on was split instead of grouped"
    );
    let group = harness
        .group_of(tab)
        .expect("the tab the menu was opened on was not put in a group");
    assert_eq!(
        harness.members_of(group),
        harness.tab_ids(),
        "both checkouts are folded under one heading, in strip order"
    );

    // A pane of the new tab, in a directory that is really there, which git
    // really knows is a worktree of the repository the menu was opened on.
    let opened = harness
        .workspace
        .read(&harness.app, |workspace, _| workspace.pane_directories())
        .into_iter()
        .map(|(_, directory)| directory)
        .find(|directory| directory.starts_with(&store))
        .expect("no pane was opened in the new checkout");
    assert!(opened.is_dir(), "{} was not checked out", opened.display());

    let listed = crate::git::worktree::list(&repository).expect("the repository still lists");
    assert!(
        listed.iter().any(|worktree| worktree.path == opened),
        "git does not know about the checkout that was made: {listed:?}"
    );
}

#[test]
fn the_creator_answers_enter_with_its_button_and_escape_with_cancel() {
    // The popup's two keys, on the face a person types into. The branch field
    // eats both by itself — Escape empties it, Enter does nothing — so this is
    // the test that says `Workspace::action_for` gets them first: without that
    // claim the dialog can only be finished with a pointer.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };
    let store = scratch.path().join("store");

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| workspace.set_worktrees_directory(store.clone()));
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);
    harness.frame();

    harness.dispatch_worktree(WorktreeAction::OpenMenu(harness.active_id()));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });

    // Escape backs out of the creator without taking the menu with it, which
    // is what the Cancel button beside it does.
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    assert!(
        harness.press_key("escape", Modifiers::default()),
        "escape was not claimed by the creator"
    );
    assert!(
        !harness.worktree_menu_is_creating(),
        "escape did not leave the creator"
    );
    assert!(
        harness.a_popup_is_open(),
        "escape closed the whole menu instead of stepping back one face"
    );

    // And Enter is the Create button: a name is already in the field, so this
    // is the whole of making a worktree from the keyboard.
    let before = harness.pane_ids().len();
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    assert!(
        harness.press_key("enter", Modifiers::default()),
        "enter was not claimed by the creator"
    );
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.pane_ids().len() > before
    });

    let opened = harness
        .workspace
        .read(&harness.app, |workspace, _| workspace.pane_directories())
        .into_iter()
        .map(|(_, directory)| directory)
        .find(|directory| directory.starts_with(&store))
        .expect("enter did not check anything out");
    assert!(opened.is_dir(), "{} was not checked out", opened.display());
}

#[test]
fn a_plugins_field_takes_the_keyboard_from_the_pane_under_it() {
    // The whole of what the field registry buys. Nothing in the window's own
    // source names this field any more: `sync_input_keys` asks the host, the
    // host asks the plugin that claimed it, and the plugin answers from a
    // state the workspace happens to hold today and will not always.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| {
        workspace.set_worktrees_directory(scratch.path().join("store"));
    });
    let tab = harness.active_id();
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);
    harness.frame();

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });
    assert!(
        !harness.a_plugin_field_has_keys(),
        "a field nobody is typing into already had the keyboard"
    );

    harness.dispatch_worktree(WorktreeAction::StartCreating);

    assert!(
        harness.a_plugin_field_has_keys(),
        "the creator opened and the plugin's field did not take the keyboard"
    );
    assert!(
        !harness.a_pane_field_has_keys(),
        "a pane was still listening while a field over it was being typed into"
    );

    harness.dispatch_worktree(WorktreeAction::Cancel);

    assert!(
        !harness.a_plugin_field_has_keys(),
        "leaving the creator left the keyboard in a field nothing draws"
    );
}

#[test]
fn escape_takes_the_menu_down_and_enter_removes_the_checkout() {
    // The other two answers. Escape on the list is the press outside it, and
    // Enter on the confirmation is its Remove — the face a person got to by
    // pressing an × and reading a question has a keyboard as well as a
    // pointer.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };
    let store = scratch.path().join("store");

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| workspace.set_worktrees_directory(store.clone()));
    let tab = harness.active_id();
    let first = harness.pane_ids()[0];
    harness.update_session(first, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(first, "main", None);
    harness.frame();

    // A second checkout, so there is a row with an × behind it — the main one
    // is never removable.
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    harness.dispatch_worktree(WorktreeAction::Create);
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.pane_ids().len() > 1
    });
    let made = harness
        .focused_pane_id()
        .expect("the split focused its pane");
    harness.dispatch_action(TabAction::ClosePane(made));

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("both checkouts to be read", |harness| {
        harness.worktrees_listed() == Some(2)
    });
    let index = harness
        .worktree_index_under(&store)
        .expect("the checkout that was made is not in the menu");
    harness.dispatch_worktree(WorktreeAction::AskRemove(index));
    assert!(
        harness.worktree_menu_is_confirming(),
        "the confirmation did not open"
    );

    assert!(
        harness.press_key("escape", Modifiers::default()),
        "escape was not claimed by the confirmation"
    );
    assert!(
        !harness.worktree_menu_is_confirming(),
        "escape did not leave the confirmation"
    );
    assert!(
        crate::git::worktree::list(&repository)
            .expect("the repository still lists")
            .len()
            == 2,
        "backing out of the confirmation removed the checkout anyway"
    );

    // And from the list itself, one more press puts the list away — leaving
    // the menu it hangs off standing, because that is one step back and not
    // two. See `escape_takes_the_submenu_down_before_the_menu`.
    assert!(
        harness.press_key("escape", Modifiers::default()),
        "escape was not claimed by the list"
    );
    assert_eq!(
        harness.tab_menu_row(),
        Some(first),
        "escape took the menu down as well as the list inside it"
    );
    assert!(
        harness.press_key("escape", Modifiers::default()),
        "escape was not claimed by the menu the list was in"
    );
    assert!(
        !harness.a_popup_is_open(),
        "escape did not take the menu down"
    );

    // Then the same question again, answered with the other key. A clean
    // checkout is one git removes without a second question, so this press is
    // the whole of it.
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("both checkouts to be read", |harness| {
        harness.worktrees_listed() == Some(2)
    });
    let index = harness
        .worktree_index_under(&store)
        .expect("the checkout that was made is not in the menu");
    harness.dispatch_worktree(WorktreeAction::AskRemove(index));
    assert!(
        harness.press_key("enter", Modifiers::default()),
        "enter was not claimed by the confirmation"
    );
    harness.wait_for("the checkout to go", |_| {
        crate::git::worktree::list(&repository)
            .map(|worktrees| worktrees.len() == 1)
            .unwrap_or(false)
    });
}

/// A checkout of the repository `tab` is in, made through the menu the way a
/// person makes one and then walked away from.
///
/// The pane it opened in is closed again, because a checkout a tab is working
/// in is not free — which is the rule these tests set up rather than the one
/// they are about.
fn spare_checkout(harness: &mut Harness, tab: TabId, store: &Path, made: &[PathBuf]) -> PathBuf {
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });

    let before = harness.pane_ids().len();
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    harness.dispatch_worktree(WorktreeAction::Create);
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.pane_ids().len() > before
    });

    let opened = harness
        .focused_pane_id()
        .expect("the checkout did not open a pane");
    let path = harness
        .working_directory(opened)
        .expect("the pane that opened does not know where it is");
    assert!(
        path.starts_with(store) && !made.contains(&path),
        "{} is not a checkout this call made",
        path.display()
    );

    harness.dispatch_action(TabAction::ClosePane(opened));
    harness.dispatch_action(TabAction::Select(tab));
    path
}

#[test]
fn tidying_up_takes_the_free_checkouts_and_leaves_the_work_alone() {
    // The whole of the sweep against a real repository: three checkouts
    // nobody is in, one of them with a file in it that git will refuse over,
    // and one press that has to take exactly the other two. What makes this
    // worth a real `git worktree remove` rather than a stub is the refusal —
    // "which of these will git actually let go" is a question only git
    // answers, and the face is built entirely around not guessing it.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };
    let store = scratch.path().join("store");

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| workspace.set_worktrees_directory(store.clone()));
    let tab = harness.active_id();
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);
    harness.frame();

    let mut free: Vec<PathBuf> = Vec::new();
    for _ in 0..3 {
        let made = spare_checkout(&mut harness, tab, &store, &free);
        free.push(made);
    }
    fs::write(free[1].join("scratch.txt"), "half a thought\n").expect("the checkout is there");

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("every checkout to be read", |harness| {
        harness.worktrees_listed() == Some(4)
    });
    assert!(
        worktree_menu_says(&harness.frame(), "3 free checkouts"),
        "the list did not offer the three checkouts nothing is working in"
    );

    harness.dispatch_worktree(WorktreeAction::AskTidy);
    assert!(
        harness.worktree_menu_is_tidying(),
        "the row did not open its question"
    );
    // The question opens saying how many it is looking in, before any of
    // them has answered.
    assert_eq!(harness.worktrees_swept(), Some((0, 3)));
    harness.wait_for("the checkouts to be looked in", |harness| {
        harness.worktrees_going().is_some()
    });
    assert_eq!(
        harness.worktrees_going(),
        Some(2),
        "the checkout with a file in it was counted as one that would go"
    );

    harness.dispatch_worktree(WorktreeAction::Tidy);
    // And the button opens the removal saying how many are going, so that
    // the face has a count to move the pirate along from the first frame.
    assert_eq!(harness.worktrees_swept(), Some((0, 2)));
    harness.wait_for("the free checkouts to go", |_| {
        crate::git::worktree::list(&repository)
            .map(|worktrees| worktrees.len() == 2)
            .unwrap_or(false)
    });
    // Back on the list, freshly read, with nothing in flight.
    harness.wait_for("the list to be read again", |harness| {
        harness.worktrees_listed() == Some(2)
    });
    assert_eq!(harness.worktrees_swept(), None);

    assert!(
        !free[0].exists() && !free[2].exists(),
        "a checkout git let go was left on disk"
    );
    assert!(
        free[1].is_dir(),
        "the checkout with work in it was removed anyway"
    );
    let left = crate::git::worktree::list(&repository).expect("the repository still lists");
    assert!(
        left.iter().any(|worktree| worktree.path == free[1]),
        "git no longer knows about the checkout that was kept: {left:?}"
    );
}

#[test]
fn stop_spares_the_checkouts_after_the_one_in_flight() {
    // Six checkouts with a build in each is minutes of `git worktree remove`,
    // and the way out of that is a Stop that means it: the one git is
    // deleting finishes — a kill halfway through leaves a checkout neither
    // there nor gone — and nothing after it starts. Pressed before the first
    // has landed, so exactly one goes.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };
    let store = scratch.path().join("store");

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| workspace.set_worktrees_directory(store.clone()));
    let tab = harness.active_id();
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);
    harness.frame();

    let mut free: Vec<PathBuf> = Vec::new();
    for _ in 0..3 {
        let made = spare_checkout(&mut harness, tab, &store, &free);
        free.push(made);
    }

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("every checkout to be read", |harness| {
        harness.worktrees_listed() == Some(4)
    });
    harness.dispatch_worktree(WorktreeAction::AskTidy);
    harness.wait_for("the checkouts to be looked in", |harness| {
        harness.worktrees_going() == Some(3)
    });

    harness.dispatch_worktree(WorktreeAction::Tidy);
    // Escape, which is Cancel, which during a sweep is Stop. Dispatched
    // before the queue is pumped, so the first removal has not landed yet
    // whether or not git has finished it.
    harness.dispatch_worktree(WorktreeAction::Cancel);
    assert!(
        harness.worktree_sweep_is_stopping(),
        "Cancel during a sweep walked back to a list that is about to be wrong"
    );
    assert!(
        harness.worktree_menu_is_tidying(),
        "the face was taken down with a removal still in flight"
    );

    harness.wait_for(
        "the sweep to stop and the list to be read again",
        |harness| harness.worktrees_listed().is_some() && !harness.worktree_menu_is_tidying(),
    );
    let left = crate::git::worktree::list(&repository).expect("the repository still lists");
    assert_eq!(
        left.len(),
        3,
        "the stop did not spare the two behind the one in flight: {left:?}"
    );
    assert!(
        !free[0].exists(),
        "the checkout in flight was not let finish"
    );
    assert!(
        free[1].is_dir() && free[2].is_dir(),
        "a checkout after the stop was removed"
    );
    assert!(
        worktree_menu_says(&harness.frame(), "Stopped after 1 of 3"),
        "the list does not say where the sweep stopped"
    );
}

#[test]
fn the_pirate_chews_while_git_is_out_and_rests_when_it_is_back() {
    // The indicator is the pirate, and the pirate has to move: a frame that
    // does not change for as long as git takes is exactly the hang the
    // indicator exists to distinguish itself from. And he has to stop, because
    // a chain still ticking under a list nobody is waiting on is an idle
    // heartbeat, which this window does not have.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };

    let mut harness = Harness::seeded();
    let tab = harness.active_id();
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);
    harness.frame();

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    assert!(
        harness.worktree_menu_is_busy(),
        "a menu that has not read the repository yet is not waiting on anything"
    );
    // Long enough for the bite to have moved whether or not git has answered:
    // the read is milliseconds and the frame is a tenth of a second, so this
    // is what proves the chain runs on its own clock rather than on git's.
    harness.settle_for(std::time::Duration::from_secs(2), |harness| {
        harness.worktree_menu_chomp() > 0
    });
    let moved = harness.worktree_menu_chomp() > 0 || !harness.worktree_menu_is_busy();
    assert!(moved, "the pirate stood still while git was out");

    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });
    assert!(!harness.worktree_menu_is_busy());
    harness.wait_for("the bite to end on a whole face", |harness| {
        harness.worktree_menu_chomp() == 0
    });
    // And it stays there: nothing is re-arming him.
    harness.settle(crate::pirate::FRAME * 3);
    assert_eq!(
        harness.worktree_menu_chomp(),
        0,
        "the pirate chews at nothing"
    );
}

#[test]
fn nothing_offers_to_tidy_a_checkout_a_tab_is_working_in() {
    // The row is absent rather than inert, so its absence is the whole of what
    // says "there is nothing here to remove" — and a checkout somebody has
    // open must not be counted, or the offer becomes one that fails when it is
    // taken up. Three states of one repository, in the order they happen.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };
    let store = scratch.path().join("store");

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| workspace.set_worktrees_directory(store.clone()));
    let tab = harness.active_id();
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(pane, "main", None);
    harness.frame();

    // One checkout, which is the main one and is never removable.
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });
    assert!(
        !worktree_menu_says(&harness.frame(), "free checkout"),
        "a repository with only its main checkout was offered a tidy-up"
    );

    // A second one, with the tab it opened in still there.
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    harness.dispatch_worktree(WorktreeAction::Create);
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.pane_ids().len() > 1
    });
    let opened = harness
        .focused_pane_id()
        .expect("the checkout did not open a pane");

    harness.dispatch_action(TabAction::Select(tab));
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("both checkouts to be read", |harness| {
        harness.worktrees_listed() == Some(2)
    });
    assert!(
        !worktree_menu_says(&harness.frame(), "free checkout"),
        "a checkout a tab is working in was offered for removal"
    );

    // And once nothing is working in it, it is exactly what this row is for.
    harness.dispatch_worktree(WorktreeAction::CloseMenu);
    harness.dispatch_action(TabAction::ClosePane(opened));
    harness.dispatch_action(TabAction::Select(tab));
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("both checkouts to be read again", |harness| {
        harness.worktrees_listed() == Some(2)
    });
    assert!(
        worktree_menu_says(&harness.frame(), "1 free checkout"),
        "the checkout nothing is working in any more was not offered"
    );
}

#[test]
fn enter_does_not_remove_a_checkout_git_has_already_refused() {
    // The second question, which the keyboard does not answer. git declines to
    // throw away work nobody asked it to throw away, and the button that then
    // says "Remove anyway" is the one destructive thing in this menu that
    // stays a click: a person who pressed Enter and was answered with a
    // warning must not be able to delete what it warns about by pressing the
    // same key again.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };
    let store = scratch.path().join("store");

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| workspace.set_worktrees_directory(store.clone()));
    let tab = harness.active_id();
    let first = harness.pane_ids()[0];
    harness.update_session(first, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(first, "main", None);
    harness.frame();

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    harness.dispatch_worktree(WorktreeAction::Create);
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.pane_ids().len() > 1
    });
    let made = harness
        .focused_pane_id()
        .expect("the split focused its pane");
    harness.dispatch_action(TabAction::ClosePane(made));

    // The work git will refuse over: one file that is in the checkout and in
    // nothing else.
    let checkout = crate::git::worktree::list(&repository)
        .expect("the repository lists")
        .into_iter()
        .map(|worktree| worktree.path)
        .find(|path| path.starts_with(&store))
        .expect("nothing was checked out under the store");
    std::fs::write(checkout.join("unsaved.txt"), "work").expect("the checkout is writable");

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("both checkouts to be read", |harness| {
        harness.worktrees_listed() == Some(2)
    });
    let index = harness
        .worktree_index_under(&store)
        .expect("the checkout that was made is not in the menu");
    harness.dispatch_worktree(WorktreeAction::AskRemove(index));
    assert!(
        harness.press_key("enter", Modifiers::default()),
        "enter was not claimed by the confirmation"
    );
    harness.wait_for("git to refuse over the work in there", |harness| {
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace.worktree_menu_was_refused()
        })
    });

    assert_eq!(
        harness.action_for("enter", Modifiers::default()),
        None,
        "enter is bound to the button that deletes work git refused to delete"
    );
    assert_eq!(
        crate::git::worktree::list(&repository)
            .expect("the repository still lists")
            .len(),
        2,
        "the checkout went with a second press of the same key"
    );
    assert!(
        checkout.join("unsaved.txt").exists(),
        "the work in the checkout went with it"
    );
}

#[test]
fn showing_a_checkout_opens_it_in_the_group_and_never_twice() {
    // The other half of the same rule, on the path that opens a checkout that
    // already exists. Two things it has to get right, and the second is what
    // the first one costs: a checkout opens as a tab in the *group* the tab
    // that asked belongs to, so "is this one already open" is asked of every
    // pane in the window rather than of the tabs' focused ones — the branch is
    // very often in a tab beside the row somebody is looking at, and a menu
    // that missed it would put a second agent in the same checkout.
    let scratch = Scratch::new();
    let Some(repository) = scratch_repository(&scratch.path().join("repo")) else {
        eprintln!("skipped: no git here to make a repository with");
        return;
    };
    let store = scratch.path().join("store");

    let mut harness = Harness::seeded();
    harness.workspace_update(|workspace, _| workspace.set_worktrees_directory(store.clone()));
    let tab = harness.active_id();
    let first = harness.pane_ids()[0];
    harness.update_session(first, |session| {
        session.working_directory = Some(repository.clone());
    });
    harness.record_git(first, "main", None);
    harness.frame();

    // A second checkout to show. Made through the menu, because that is the
    // only thing that makes one.
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("the repository to be read", |harness| {
        harness.worktrees_listed().is_some()
    });
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    harness.dispatch_worktree(WorktreeAction::Create);
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.pane_ids().len() > 1
    });

    // Away again, so the row has a checkout nothing is working in to show.
    let made = harness
        .focused_pane_id()
        .expect("the new tab focused its pane");
    harness.dispatch_action(TabAction::ClosePane(made));
    assert_eq!(harness.tab_ids().len(), 1, "the tab did not close");

    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("both checkouts to be read", |harness| {
        harness.worktrees_listed() == Some(2)
    });
    let index = harness
        .worktree_index_under(&store)
        .expect("the checkout that was made is not in the menu");

    let tabs = harness.tab_ids().len();
    harness.dispatch_worktree(WorktreeAction::Show(index));

    assert_eq!(
        harness.tab_ids().len(),
        tabs + 1,
        "the checkout did not open a tab"
    );
    assert_eq!(
        harness.panes_of(tab).len(),
        1,
        "the tab its menu was opened on was split instead of grouped"
    );
    let group = harness
        .group_of(tab)
        .expect("the checkout did not join the tab that asked for it");
    assert_eq!(harness.members_of(group).len(), 2);
    let opened = harness.focused_pane_id().expect("a tab has a focused pane");
    assert_ne!(opened, first, "the pane it opened is not the focused one");

    // Back on the first pane, so the checkout is open in a pane nobody is
    // looking at — which is the case the old tab-shaped lookup got wrong.
    harness.dispatch_action(TabAction::FocusPane(first));
    harness.dispatch_worktree(WorktreeAction::OpenMenu(tab));
    harness.wait_for("both checkouts to be read again", |harness| {
        harness.worktrees_listed() == Some(2)
    });
    harness.dispatch_worktree(WorktreeAction::Show(index));

    assert_eq!(
        harness.tab_ids().len(),
        tabs + 1,
        "a second agent was opened in a checkout that was already open"
    );
    assert_eq!(
        harness.focused_pane_id(),
        Some(opened),
        "showing an open checkout did not bring its pane forward"
    );
}

#[test]
fn a_branch_leads_with_the_branch_icon() {
    // The subtitle under a seeded row is its branch, and what says it is a
    // branch rather than a path is the mark in front of it. Warp draws
    // `UiIcon::GitBranch` there; this used to draw three rectangles.
    let mut harness = Harness::seeded();
    let scene = harness.frame();
    let row = tab_boxes(&scene)[0];

    let marks = icons_in(&scene, row, Lucide::GitBranch);
    assert_eq!(marks.len(), 1, "the row draws no branch mark");

    let branch_x = scene
        .layers()
        .flat_map(|layer| layer.glyphs.iter())
        .filter(|glyph| row.contains_point(glyph.position))
        .filter(|glyph| glyph.position.y() > marks[0].min_y())
        .map(|glyph| glyph.position.x())
        .fold(f32::INFINITY, f32::min);

    assert!(
        marks[0].max_x() <= branch_x,
        "the mark at {:?} is not in front of the branch at {branch_x}",
        marks[0]
    );
}

#[test]
fn the_density_control_shows_one_icon_per_density() {
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::TogglePopup);
    let scene = harness.frame();

    let menu = menu_box(&scene);
    assert_eq!(icons_in(&scene, menu, Lucide::Menu).len(), 1, "Compact");
    assert_eq!(
        icons_in(&scene, menu, Lucide::LayoutGrid).len(),
        1,
        "Expanded"
    );
}

#[test]
fn the_menu_marks_the_chosen_option_and_reserves_the_slot_on_the_others() {
    // The check slot is 16x16 whether or not it holds a check, so a label
    // never shifts when the selection moves — and the label is `text_primary`
    // selected or not. Dimming the unselected rows is the obvious instinct and
    // it is not what Warp does; the glyph is the only difference.
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::TogglePopup);

    let scene = harness.frame();
    let rows = menu_option_boxes(&scene);
    assert_eq!(rows.len(), 6, "the Compact menu draws six option rows");
    // "Command / Conversation", "Branch" as the subtitle, and the hover row.
    assert_eq!(checked_rows(&scene), vec![0, 3, 5]);

    let checked = row_label_x(&scene, rows[0]);
    let unchecked = row_label_x(&scene, rows[1]);
    assert!(
        checked.is_finite(),
        "no label glyph was found inside the row, so the comparison below          would pass whatever the layout did"
    );
    assert_eq!(
        checked, unchecked,
        "the label moved with the check, so the slot is not reserved"
    );

    // "Working Directory": the mark moves within its own section and nowhere
    // else, because the three sections are three independent lists.
    harness.click(center(rows[1]), MouseButton::Left);
    let scene = harness.frame();
    assert_eq!(checked_rows(&scene), vec![1, 3, 5]);
    assert_eq!(
        row_label_x(&scene, menu_option_boxes(&scene)[0]),
        checked,
        "deselecting a row moved its label"
    );

    // The hover row is the one toggle in this state, and it un-checks.
    harness.click(center(rows[5]), MouseButton::Left);
    assert_eq!(checked_rows(&harness.frame()), vec![1, 3]);
}

#[test]
fn every_show_toggle_carries_its_own_check() {
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));
    harness.dispatch_option(OptionsAction::TogglePopup);

    let scene = harness.frame();
    let rows = menu_option_boxes(&scene);
    // Three "Pane title as", "PR link", "Diff stats", and the hover row.
    assert_eq!(rows.len(), 6, "the Expanded menu draws six option rows");
    assert_eq!(checked_rows(&scene), vec![0, 3, 4, 5], "every Show is on");

    harness.click(center(rows[4]), MouseButton::Left);
    assert!(!harness.options().show_diff_stats);
    assert_eq!(checked_rows(&harness.frame()), vec![0, 3, 5]);
}

// --- the vertical tabs panel -------------------------------------------------

/// The panel's own box, by its ground and its width.
///
/// The header is painted in the same colour, so the width and the left edge
/// are what tell them apart — and in the vertical layout the header starts
/// beside the panel rather than above it, so their boxes never overlap.
fn panel_box(scene: &Scene) -> RectF {
    let boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.background == Fill::Solid(theme().surface)
                && (rect.bounds.width() - tabs_panel::PANEL_WIDTH).abs() < 0.5
                && rect.bounds.min_x() < 1.
        })
        .map(|(_, bounds)| bounds)
        .collect();

    assert_eq!(boxes.len(), 1, "exactly one tabs panel per frame");
    boxes[0]
}

/// The panel's search box, by the one thing only a field is: its own rounded
/// ground, exactly [`text_field::HEIGHT`] tall, inside the panel.
fn panel_search_box(scene: &Scene) -> RectF {
    let panel = panel_box(scene);
    let boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, bounds)| {
            rect.background == Fill::Solid(theme().overlay_1)
                && (bounds.height() - crate::workspace::text_field::HEIGHT).abs() < 0.5
                && bounds.max_x() <= panel.max_x()
        })
        .map(|(_, bounds)| bounds)
        .collect();

    assert_eq!(boxes.len(), 1, "expected one search box, got {boxes:?}");
    boxes[0]
}

/// Every row the panel painted, as (what it drew, what the clip left).
///
/// A row is the only 4px-rounded box inside the panel wider than a button: a
/// close button is 16 across, the `+` is rounded by five, and the hover card
/// hangs outside the panel entirely.
fn panel_row_rects(scene: &Scene) -> Vec<(RectF, RectF)> {
    let panel = panel_box(scene);
    let mut rows: Vec<(RectF, RectF)> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(4.)
                && rect.bounds.width() > 100.
                && rect.bounds.min_x() >= panel.min_x()
                && rect.bounds.max_x() <= panel.max_x()
        })
        .map(|(rect, visible)| (rect.bounds, visible))
        .collect();
    rows.sort_by(|left, right| left.0.min_y().total_cmp(&right.0.min_y()));
    rows
}

/// The rows a person can actually see and click.
fn panel_rows(scene: &Scene) -> Vec<RectF> {
    panel_row_rects(scene)
        .into_iter()
        .filter(|(painted, visible)| (visible.height() - painted.height()).abs() < 0.5)
        .map(|(painted, _)| painted)
        .collect()
}

/// What the panel says, without the header or the body beside it.
fn panel_text(scene: &Scene) -> String {
    let panel = panel_box(scene);
    text_where(scene, |position| position.x() < panel.max_x())
}

/// The rows carrying the selected row's 1px outline.
fn selected_panel_rows(scene: &Scene) -> Vec<RectF> {
    let panel = panel_box(scene);
    visible_rects(scene)
        .filter(|(rect, _)| {
            rect.border.color == Fill::Solid(theme().overlay_3)
                && rect.bounds.width() > 100.
                && rect.bounds.max_x() <= panel.max_x()
        })
        .map(|(_, bounds)| bounds)
        .collect()
}

impl Harness {
    /// A panel with one seeded row and the frame already drawn.
    fn seeded_panel() -> Self {
        let mut harness = Self::panel(1);
        let pane = harness.pane_ids()[0];
        harness.seed(pane, Some(seeded_diff()));
        harness.frame();
        harness
    }

    /// The panel's rows, at these options.
    fn panel_rows_with(&mut self, options: TabOptions) -> Vec<RectF> {
        self.set_options(options);
        panel_rows(&self.frame())
    }
}

#[test]
fn a_fresh_workspace_opens_with_the_tabs_in_a_panel() {
    // The one place Crook departs from Warp, asserted through the pixels: the
    // tabs are in a panel and the header holds none of them. Warp makes this a
    // setting; here it is the window.
    let mut harness = Harness::panel(2);
    let scene = harness.frame();

    assert_eq!(
        0.,
        panel_box(&scene).min_x(),
        "the panel is not at the edge"
    );
    assert_eq!(panel_rows(&scene).len(), 2, "one row per tab");
}

#[test]
fn the_panel_carries_a_search_box_above_the_list_and_the_plus_below_it() {
    // Telegram's arrangement with Warp's browser at the other end of it: the
    // field across the whole column, then the list, then the `+` in the space
    // the list leaves. Asserted as an order rather than as coordinates — what
    // matters is that the field is over every row and the `+` under every row,
    // not what any of the three happen to measure.
    let mut harness = Harness::panel(2);
    let scene = harness.frame();

    let field = panel_search_box(&scene);
    let rows = panel_rows(&scene);

    assert!(
        field.max_y() <= plus_box(&scene).min_y(),
        "the box is under the `+` it belongs above"
    );
    assert!(
        plus_box(&scene).min_y() >= rows[rows.len() - 1].max_y(),
        "the `+` is not under the last row of the list"
    );
    assert!(
        field.max_y() <= rows[0].min_y(),
        "the box overlaps the first row of the list"
    );
    assert!(
        field.width() > tabs_panel::PANEL_WIDTH - 26.,
        "the box does not take the width of the column: {field:?}"
    );
}

#[test]
fn the_box_takes_no_keys_until_it_is_asked_for() {
    // The rule that makes a search box possible in a window whose whole point
    // is a shell: the tabs and a pane are on screen together, so the box has to
    // stay out of the way of typing until somebody puts the keyboard in it.
    let mut harness = Harness::panel(2);

    harness.type_text("ab");
    assert_eq!(
        harness.panel_search_text(),
        "",
        "the box collected what was typed into a session"
    );
    assert!(
        harness.pane_takes_keys(),
        "the pane lost the keyboard to it"
    );

    harness.press_search_chord();
    assert!(harness.panel_search_takes_keys(), "the chord found nothing");
    assert!(
        !harness.pane_takes_keys(),
        "the pane and the box both have the keyboard"
    );

    harness.type_text("cd");
    assert_eq!(harness.panel_search_text(), "cd");
}

#[test]
fn typing_in_the_box_filters_the_list_and_leaves_the_strip_alone() {
    // The whole feature, and the one line of it that is easy to get wrong: a
    // filter is a thing the *list* does. The tab that is active stays active
    // and every tab is still open — a person looking for one tab has not asked
    // to close the others.
    let mut harness = Harness::panel(3);
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.derived_title = Some("kettle".to_owned());
    });

    harness.click_panel_search();
    harness.type_text("kettle");

    let scene = harness.frame();
    assert_eq!(
        panel_rows(&scene).len(),
        1,
        "the filter kept the wrong rows"
    );
    let text = panel_text(&scene);
    assert!(
        text.contains("kettle") && !text.contains("agent 2"),
        "the list is not the one row that matches: {text:?}"
    );

    assert_eq!(harness.tab_ids().len(), 3, "the search closed a tab");
    assert_eq!(
        harness.active_id(),
        harness.tab_ids()[2],
        "the search moved the active tab"
    );
}

#[test]
fn a_row_is_found_by_the_directory_it_prints() {
    // A row says two things — what is running and where — and both of them are
    // what somebody types. The path is matched as the row abbreviates it and
    // not as the row *cuts* it: a directory that could not be found because
    // the column was too narrow to print its middle is a search nobody trusts
    // twice.
    let mut harness = Harness::panel(2);
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.working_directory = Some(PathBuf::from("/tmp/zebra"));
    });

    harness.click_panel_search();
    harness.type_text("zebra");

    assert_eq!(
        panel_rows(&harness.frame()).len(),
        1,
        "the row standing in /tmp/zebra was not the one kept"
    );
}

#[test]
fn a_tab_is_found_by_a_pane_its_row_does_not_name() {
    // `Tabs` granularity draws one row per tab and that row names the focused
    // pane, silently dropping the rest. A filter that searched only what the
    // row prints would hide the tab that is running what somebody is looking
    // for, which is the one thing a search must not do.
    let mut harness = Harness::panel(2);
    let hidden = harness.active_pane_ids()[0];
    harness.dispatch_action(TabAction::Split(Direction::Right));
    harness.update_session(hidden, |session| {
        session.derived_title = Some("kettle".to_owned());
    });
    // The focused pane is named too, and named something else. Both panes sit
    // in the same directory here, and a row with no name of its own is now
    // called after that directory — so without this the two rows would read
    // the same and the assertion below could not tell them apart.
    let focused = harness.focused_pane_id().expect("the split focused a pane");
    harness.update_session(focused, |session| {
        session.derived_title = Some("cocoa".to_owned());
    });
    harness.set_granularity(Granularity::Tabs);

    harness.click_panel_search();
    harness.type_text("kettle");

    let scene = harness.frame();
    assert_eq!(
        panel_rows(&scene).len(),
        1,
        "the tab running it was filtered out"
    );
    // The row that survived names the tab's *focused* pane, which is the one
    // the split made and not the one the query found — so the match came from
    // a pane the list never printed. ("kettle" is in the panel either way: the
    // box prints what was typed into it, which is why the row is read for
    // `cocoa` rather than searched for the absence of `kettle`.)
    let text = panel_text(&scene);
    assert!(
        text.contains("cocoa"),
        "the tab kept is not the one running it: {text:?}"
    );
}

#[test]
fn enter_selects_the_top_match_and_gives_the_keyboard_back() {
    // Telegram's Enter, and the reason the box is worth a chord: three
    // letters, one key, and you are in that tab with the keyboard back in the
    // shell. The query goes with it — what it was for has happened.
    let mut harness = Harness::panel(3);
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.derived_title = Some("kettle".to_owned());
    });
    harness.press_search_chord();
    harness.type_text("kettle");

    harness.press("enter", Modifiers::default(), "");

    assert_eq!(
        harness.active_id(),
        harness.tab_ids()[0],
        "Enter did not open the tab that matched"
    );
    assert_eq!(
        harness.panel_search_text(),
        "",
        "the query outlived its use"
    );
    assert!(
        !harness.panel_search_takes_keys() && harness.pane_takes_keys(),
        "the box kept the keyboard after it was finished with"
    );
    assert_eq!(
        panel_rows(&harness.frame()).len(),
        3,
        "the list is still cut"
    );
}

#[test]
fn clicking_a_row_that_was_searched_for_ends_the_search() {
    // The other half of Enter, and the one that is easy to leave out: a person
    // who found their tab with the mouse is just as finished as one who found
    // it with a key, and a box that kept the keyboard would collect the first
    // command they typed into the tab they had just opened.
    let mut harness = Harness::panel(3);
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.derived_title = Some("kettle".to_owned());
    });
    harness.press_search_chord();
    harness.type_text("kettle");

    let row = panel_rows(&harness.frame())[0];
    harness.click(center(row), MouseButton::Left);
    harness.frame();

    assert_eq!(
        harness.active_id(),
        harness.tab_ids()[0],
        "the row is inert"
    );
    assert_eq!(harness.panel_search_text(), "");
    assert!(
        !harness.panel_search_takes_keys() && harness.pane_takes_keys(),
        "the box kept the keyboard after the row was clicked"
    );
    assert_eq!(panel_rows(&harness.frame()).len(), 3);
}

#[test]
fn escape_empties_the_box_and_gives_the_keyboard_back() {
    // One press, not two. In the settings rail Escape only empties the box,
    // because there is nowhere else on that screen for the keyboard to go;
    // here there is a shell behind it, and a box that held on would eat the
    // next command typed.
    let mut harness = Harness::panel(2);
    harness.press_search_chord();
    harness.type_text("kettle");
    assert_eq!(harness.panel_search_text(), "kettle");

    harness.press("escape", Modifiers::default(), "");

    assert_eq!(harness.panel_search_text(), "");
    assert!(harness.pane_takes_keys(), "the pane never got the keyboard");
    assert_eq!(
        panel_rows(&harness.frame()).len(),
        2,
        "the list is still cut"
    );
}

#[test]
fn a_query_that_empties_the_panel_says_so() {
    // The one state in which the panel is genuinely empty — the strip refuses
    // to empty itself and closes the window instead — so it says what the
    // filter did rather than what a window with no tabs would say.
    let mut harness = Harness::panel(2);
    harness.click_panel_search();
    harness.type_text("zzzz");

    let scene = harness.frame();
    assert!(panel_rows(&scene).is_empty());
    assert!(
        panel_text(&scene).contains("No tabs match your search."),
        "an empty panel with no explanation in it: {:?}",
        panel_text(&scene)
    );
}

#[test]
fn the_query_goes_when_the_sidebar_shows_something_else() {
    // The same rule the settings rail's box follows, for the same reason: a
    // filter that survived a trip through the settings would bring a person
    // back to a sidebar showing four tabs out of forty, which reads as broken
    // rather than as filtered. The keyboard goes with it — clicking a section
    // button is being finished with the box.
    let mut harness = Harness::panel(3);
    harness.click_panel_search();
    harness.type_text("kettle");

    harness.show_plugins();
    harness.show_tabs();

    assert_eq!(harness.panel_search_text(), "");
    assert!(!harness.panel_search_takes_keys());
    assert_eq!(panel_rows(&harness.frame()).len(), 3);
}

#[test]
fn the_chord_comes_back_to_the_tabs_from_another_section() {
    // One gesture rather than two. The box is drawn over the tabs and nowhere
    // else, so a chord pressed on the settings has to bring the tabs back
    // before it can put the keyboard anywhere.
    let mut harness = Harness::panel(2);
    harness.open_settings_page();

    harness.press_search_chord();

    assert!(
        !harness.is_settings_page_open(),
        "the sidebar is still showing the settings"
    );
    assert!(harness.panel_search_takes_keys());
    harness.type_text("kettle");
    assert_eq!(harness.panel_search_text(), "kettle");
}

#[test]
fn the_panel_draws_a_row_per_pane_or_a_row_per_tab_as_it_is_told() {
    // Warp's `pane_ids_for_display_granularity`, seen from the front. It is a
    // real difference now that a tab holds a pane group: `Tabs` silently drops
    // every pane but the focused one, with no count and no expander.
    let mut harness = Harness::panel(2);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    assert_eq!(harness.pane_ids().len(), 3);

    assert_eq!(panel_rows(&harness.frame()).len(), 3, "a row per pane");

    harness.set_granularity(Granularity::Tabs);
    assert_eq!(panel_rows(&harness.frame()).len(), 2, "a row per tab");
}

#[test]
fn a_split_tab_names_itself_above_its_rows_in_panes_and_nowhere_else() {
    // Warp's `should_show_tab_group_header`. Its first two clauses are
    // permanently false in Crook, so what is left is the third — and a header
    // above every single-pane tab would just repeat the row under it.
    let mut harness = Harness::panel(2);
    let names = harness.workspace.read(&harness.app, |workspace, _| {
        workspace
            .tabs()
            .iter()
            .map(|tab| tab.name().to_owned())
            .collect::<Vec<_>>()
    });
    // The tab's name is also its first pane's session title, so a row prints
    // it too. Renaming the session — which is what an agent does the moment it
    // has called its work something — leaves the name to the header alone.
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.derived_title = Some(TITLE.to_owned());
    });

    assert!(
        !panel_text(&harness.frame()).contains(&names[0]),
        "a single-pane tab drew a header repeating what its only row already \
         says"
    );

    harness.dispatch_action(TabAction::Select(harness.tab_ids()[0]));
    harness.dispatch_action(TabAction::Split(Direction::Right));
    assert!(
        panel_text(&harness.frame()).contains(&names[0]),
        "a tab holding two panes drew no header, so neither row says which \
         tab it belongs to"
    );

    harness.set_granularity(Granularity::Tabs);
    assert!(
        !panel_text(&harness.frame()).contains(&names[0]),
        "Tabs granularity drew a group header, which is what \
         `uses_outer_group_container == false` exists to suppress"
    );
}

#[test]
fn panes_granularity_insets_and_separates_its_tabs_and_tabs_granularity_spaces_them() {
    // The chrome inverts between the granularities, and getting only the row
    // count right leaves the panel looking wrong in both.
    let mut harness = Harness::panel(3);

    let panes = panel_rows(&harness.frame());
    let panes_gaps: Vec<f32> = panes
        .windows(2)
        .map(|two| two[1].min_y() - two[0].max_y())
        .collect();

    harness.set_granularity(Granularity::Tabs);
    let tabs = panel_rows(&harness.frame());
    let tabs_gaps: Vec<f32> = tabs
        .windows(2)
        .map(|two| two[1].min_y() - two[0].max_y())
        .collect();

    // Panes: 8px of the tab's own bottom padding and 8px of the next tab's
    // top padding — no border between them and no gap in the list column at
    // all.
    for gap in &panes_gaps {
        assert!(
            (*gap - 16.).abs() < 0.5,
            "Panes put {gap} between two tabs, not 8 + 8"
        );
    }
    // Tabs: the list column's own 4px spacing and nothing else.
    for gap in &tabs_gaps {
        assert!(
            (*gap - 4.).abs() < 0.5,
            "Tabs put {gap} between two tabs, not the column's 4px spacing"
        );
    }
    // A row is the same width in both — Warp pads each `Panes` tab by 8 and
    // the whole `Tabs` column by 8, which lands in the same place. What
    // differs is the box behind it: in `Panes` the lift belongs to the tab and
    // covers the header, the inset and every row it holds; in `Tabs` the tab
    // *is* its row and the two boxes coincide.
    assert_eq!(tabs[0].width(), panes[0].width());
    let active_row = *tabs.last().expect("three tabs, three rows");
    let lifted = lifted_tab_box(&harness.frame());
    assert!(
        (lifted.min_x() - active_row.min_x()).abs() < 0.01
            && (lifted.min_y() - active_row.min_y()).abs() < 0.01
            && (lifted.width() - active_row.width()).abs() < 0.01
            && (lifted.height() - active_row.height()).abs() < 0.01,
        "a Tabs tab drew {lifted:?} around a row of {active_row:?}, so it is \
         not the bare container the mode calls for"
    );

    harness.set_granularity(Granularity::Panes);
    let lifted = lifted_tab_box(&harness.frame());
    let row = *panel_rows(&harness.frame())
        .last()
        .expect("three tabs, three rows");
    assert!(
        lifted.width() > row.width() && lifted.height() > row.height(),
        "a Panes tab's container is {lifted:?} and its only row {row:?}, so \
         the outer group container is not there"
    );
}

/// The active tab's lifted container, by its ground.
fn lifted_tab_box(scene: &Scene) -> RectF {
    let panel = panel_box(scene);
    let boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.background == Fill::Solid(theme().overlay_1)
                && rect.bounds.width() > 100.
                && rect.bounds.max_x() <= panel.max_x()
                // The search box is painted on the same ground and takes the
                // same width. Nothing else in the panel is exactly a field
                // tall, which is also how `panel_search_box` finds it.
                && (rect.bounds.height() - crate::workspace::text_field::HEIGHT).abs() >= 0.5
        })
        .map(|(_, bounds)| bounds)
        .collect();

    assert_eq!(boxes.len(), 1, "exactly one tab is active");
    boxes[0]
}

#[test]
fn each_density_gives_a_panel_row_the_height_its_lines_add_up_to() {
    // Warp's arithmetic at a 1.2 line-height ratio, plus the 8px padding on
    // all four sides and the 1px border Crook keeps in every state so the list
    // does not move when the selection does.
    let mut harness = Harness::seeded_panel();
    let options = harness.options();

    for granularity in [Granularity::Panes, Granularity::Tabs] {
        let compact = harness.panel_rows_with(TabOptions {
            granularity,
            density: Density::Compact,
            ..options
        });
        // max(24, 12*1.2 + 1 + 10*1.2) + 16 + 2.
        assert!(
            (compact[0].height() - 45.4).abs() < 0.1,
            "{granularity:?}/Compact is {} tall, not 45.4",
            compact[0].height()
        );
        assert!(
            chip_boxes(&harness.frame()).is_empty(),
            "{granularity:?}/Compact drew a chip, which is exactly why the \
             menu hides the two Show toggles there"
        );

        let expanded = harness.panel_rows_with(TabOptions {
            granularity,
            density: Density::Expanded,
            ..options
        });
        // max(24, 12*1.2 + 2 + 12*1.2 + 2 + 14) + 16 + 2.
        assert!(
            (expanded[0].height() - 64.8).abs() < 0.1,
            "{granularity:?}/Expanded is {} tall, not 64.8",
            expanded[0].height()
        );
        assert_eq!(
            chip_boxes(&harness.frame()).len(),
            1,
            "{granularity:?}/Expanded drew no diff chip for a repository with \
             changes"
        );
    }
}

#[test]
fn a_compact_row_with_nothing_to_put_underneath_loses_the_line_rather_than_blanking_it() {
    let mut harness = Harness::seeded_panel();
    let pane = harness.pane_ids()[0];
    // No directory means no branch lookup either, so "Additional metadata"
    // has nothing at all to print.
    harness.update_session(pane, |session| session.working_directory = None);

    let rows = panel_rows(&harness.frame());
    // The 24px icon is the floor once the second line has gone.
    assert!(
        (rows[0].height() - 42.).abs() < 0.1,
        "a subtitle-less row is {} tall, not the icon plus the padding",
        rows[0].height()
    );
}

#[test]
fn an_expanded_row_is_the_same_height_whether_or_not_it_has_chips() {
    // The metadata line is pinned to 14px on purpose: letting it size to
    // content means every arriving diff stat reflows the whole list.
    let mut harness = Harness::seeded_panel();
    let options = TabOptions {
        density: Density::Expanded,
        ..harness.options()
    };

    let with_chips = harness.panel_rows_with(options)[0].height();
    let without = harness.panel_rows_with(TabOptions {
        show_diff_stats: false,
        show_pr_link: false,
        ..options
    })[0]
        .height();

    assert_eq!(with_chips, without, "turning the chips off resized the row");
}

/// Far more tabs than 608px of list can hold.
const OVERFLOWING: usize = 40;

#[test]
fn a_list_longer_than_the_panel_is_clipped_instead_of_painting_over_the_body() {
    // The clip half of `Scrollable`, on its own: whatever is off the end of
    // the list is neither drawn over the body nor hit-tested there. What is
    // past the fold is a scroll away, which the next test is about.
    let mut harness = Harness::panel(OVERFLOWING);
    let scene = harness.frame();
    let panel = panel_box(&scene);

    let painted = panel_row_rects(&scene);
    let visible = panel_rows(&scene);

    assert!(
        visible.len() < OVERFLOWING,
        "all {OVERFLOWING} rows fit, so this test is no longer measuring an \
         overflowing list"
    );
    // The number `tabs_panel`'s module docs quote as what one screenful of
    // this panel holds, asserted so the figure written down there cannot
    // quietly stop being true. The stub shaper is deterministic — every glyph
    // is half its font size and every line is 1.2x — so this is arithmetic,
    // not a font's opinion.
    // One fewer where the panel starts with the strip it reserves for the
    // traffic lights — a client-decorated macOS window — which is the strip
    // taking the height of a row.
    let fits = if harness.window_insets().panel_left > 0. {
        7
    } else {
        8
    };
    assert_eq!(
        visible.len(),
        fits,
        "the default combination fits {} tabs, and the module docs say {fits}",
        visible.len()
    );
    for (_, on_screen) in &painted {
        assert!(
            on_screen.max_y() <= panel.max_y() + 0.5,
            "a row reaches {} and the panel ends at {}",
            on_screen.max_y(),
            panel.max_y()
        );
        assert!(
            on_screen.max_x() <= panel.max_x() + 0.5,
            "a row reaches past the panel and into the body"
        );
    }

    // One target per row: clicking every row the panel shows reaches as many
    // tabs as there are rows, and never the same tab twice.
    let mut reachable: Vec<TabId> = Vec::new();
    for row in &visible {
        harness.click(
            row.origin() + vec2f(40., row.height() / 2.),
            MouseButton::Left,
        );
        let active = harness.active_id();
        if !reachable.contains(&active) {
            reachable.push(active);
        }
    }

    assert_eq!(
        reachable.len(),
        visible.len(),
        "clicking {} rows reached {} tabs, so two rows share a target",
        visible.len(),
        reachable.len()
    );
    assert!(
        reachable.len() < OVERFLOWING,
        "every tab fitted on one screenful, so this is not an overflowing list"
    );

    // The other extreme, and the other figure the docs quote — one fewer
    // under the traffic lights, for the same strip.
    harness.set_options(TabOptions {
        density: Density::Expanded,
        ..harness.options()
    });
    let fits = if harness.window_insets().panel_left > 0. {
        5
    } else {
        6
    };
    assert_eq!(
        panel_rows(&harness.frame()).len(),
        fits,
        "Panes/Expanded fits a different number than the module docs say"
    );
}

#[test]
fn the_wheel_reaches_the_tabs_that_are_past_the_bottom_of_the_panel() {
    // What the panel gained when `crookui_core` gained a `Scrollable`: before
    // it, the rows past the fold had no mouse target at all and the module
    // docs called that a ceiling. The last tab of forty is as far past it as a
    // row gets.
    let mut harness = Harness::panel(OVERFLOWING);
    let last = *harness.tab_ids().last().expect("forty tabs");
    harness.dispatch_action(TabAction::Select(harness.tab_ids()[0]));

    let panel = panel_box(&harness.frame());
    let position = center(panel);
    for _ in 0..40 {
        harness.dispatch(Event::ScrollWheel {
            position,
            delta: ScrollDelta::Lines(vec2f(0., -3.)),
            modifiers: Modifiers::default(),
        });
    }

    let scene = harness.frame();
    let rows = panel_rows(&scene);
    let bottom = rows.last().expect("the panel still draws rows");
    harness.click(
        bottom.origin() + vec2f(40., bottom.height() / 2.),
        MouseButton::Left,
    );

    assert_eq!(
        last,
        harness.active_id(),
        "scrolling to the end of the list did not put the last tab under the \
         pointer"
    );

    // And the rows are still confined to the panel: a scrollable that had
    // stopped clipping would paint the list over the body it sits beside.
    let panel = panel_box(&harness.frame());
    for (_, on_screen) in panel_row_rects(&harness.frame()) {
        assert!(
            on_screen.max_y() <= panel.max_y() + 0.5 && on_screen.min_y() >= panel.min_y() - 0.5,
            "a scrolled row escaped the panel"
        );
    }
}

#[test]
fn exactly_one_row_in_the_panel_is_drawn_as_the_selected_one() {
    // `is_selected = is_active_tab && is_focused`, a conjunction. In Panes the
    // active tab's container is lifted while only its focused pane's row is
    // selected, and painting every row of that tab as selected would lose the
    // distinction the mode exists for.
    let mut harness = Harness::panel(2);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    harness.dispatch_action(TabAction::Split(Direction::Down));

    let scene = harness.frame();
    assert_eq!(
        panel_rows(&scene).len(),
        4,
        "three panes in one tab, plus one"
    );
    assert_eq!(
        selected_panel_rows(&scene).len(),
        1,
        "the active tab drew {} selected rows",
        selected_panel_rows(&scene).len()
    );
}

#[test]
fn clicking_a_panel_row_focuses_the_pane_it_stands_for() {
    let mut harness = Harness::panel(3);
    let ids = harness.tab_ids();
    let panes = harness.pane_ids();
    let rows = panel_rows(&harness.frame());
    assert_eq!(rows.len(), 3);

    // Left of centre, well clear of the close button's slot.
    harness.click(
        rows[0].origin() + vec2f(40., rows[0].height() / 2.),
        MouseButton::Left,
    );

    assert_eq!(harness.active_id(), ids[0]);
    assert_eq!(harness.focused_pane_id(), Some(panes[0]));
    assert_eq!(harness.tab_ids(), ids, "selecting closed something");
}

/// The panel's tabs, as a group of the first two and every other tab loose.
///
/// Made the way the only thing that makes a group makes one: a worktree opened
/// from a tab. `NewInGroupOf` is what that dispatches once git has finished.
impl Harness {
    fn grouped_panel(tabs: usize) -> (Self, GroupId) {
        let mut harness = Self::panel(tabs);
        let first = harness.tab_ids()[0];
        harness.dispatch_action(TabAction::NewInGroupOf(first));
        let group = harness
            .group_of(first)
            .expect("the tab was not put in a group");
        harness.frame();
        (harness, group)
    }
}

/// Where a group's heading is: the chevron that folds it away is the only
/// thing on it that is neither text nor another row.
fn panel_heading(scene: &Scene) -> RectF {
    let panel = panel_box(scene);
    let chevrons: Vec<RectF> = [Lucide::ChevronDown, Lucide::ChevronRight]
        .into_iter()
        .flat_map(|icon| icons_in(scene, panel, icon))
        .collect();

    assert_eq!(chevrons.len(), 1, "expected one heading, got {chevrons:?}");
    chevrons[0]
}

/// How many moves a test gesture is made of. See [`Harness::drag`].
const STEPS: usize = 24;

/// The row a drag is carrying, if the panel is drawing one: the one row whose
/// middle is at the pointer rather than in the list's own flow.
fn carried_row(scene: &Scene, pointer: Vector2F) -> Option<RectF> {
    panel_rows(scene)
        .into_iter()
        .find(|row| (center(*row).y() - pointer.y()).abs() < 1.)
}

impl Harness {
    /// Picks a point up and puts it down at another, the way a hand does:
    /// press, travel, release.
    ///
    /// A frame after every event, because the window draws one after every
    /// event — and a drag *changes what the next event will be answered
    /// against*, since the line it draws moves the rows under it. A gesture
    /// tested against the frame it started on is a gesture no hand ever makes.
    fn drag(&mut self, from: Vector2F, to: Vector2F) {
        // The pointer arrives before it presses. That is not decoration: a
        // hovered row puts its detail card up, which makes the row a `Stack`,
        // which changes the layer it paints into — and a press hit-tested
        // against the wrong layer is a press the panel refuses. See
        // [`drag::Handle::takes`].
        self.move_to(from);
        self.frame();
        self.hold(from, 1);
        self.frame();
        // A hand does not arrive in one jump, and this list reorders itself
        // under it: every position is answered against the frame the last one
        // produced, and a change of group ends a frame before a change of
        // place begins. A gesture made in two leaps would skip the states a
        // real one passes through, which is where the interesting failures
        // are — and it is not a thing a hand can do.
        for step in 1..=STEPS {
            self.drag_to(from + (to - from) * (step as f32 / STEPS as f32));
            self.frame();
        }
        self.let_go(to);
        self.frame();
    }
}

/// A point inside a row, clear of its close button.
fn inside(row: RectF) -> Vector2F {
    row.origin() + vec2f(40., row.height() / 2.)
}

#[test]
fn a_row_can_be_picked_up_with_its_hover_card_up() {
    // Every row a person drags is a row they have just hovered, so this is
    // not an edge: it is the only way the gesture is ever made.
    let (mut harness, group) = Harness::grouped_panel(3);
    let loose = *harness.tab_ids().last().expect("three tabs and a worktree");
    let rows = panel_rows(&harness.frame());
    assert!(
        harness.options().show_details_on_hover,
        "the card is on by default, which is what makes this the ordinary case"
    );

    let onto = rows[0].origin() + vec2f(40., rows[0].height() * 0.75);
    harness.drag(inside(rows[3]), onto);

    assert_eq!(harness.group_of(loose), Some(group));
}

#[test]
fn a_row_carried_to_the_bottom_of_a_group_joins_it_at_its_end() {
    // The gap under a group's last member is also the gap above the block
    // beneath it, so a list that decides the group from the nearest row puts a
    // tab aimed there outside the group it was aimed into. The group's own box
    // is what settles it: that gap is inside the box, so it is inside the
    // group, and the row it names is the group's end.
    let (mut harness, group) = Harness::grouped_panel(2);
    let members = harness.members_of(group);
    let loose = *harness.tab_ids().last().expect("three tabs and a worktree");
    let rows = panel_rows(&harness.frame());
    let last_member = rows[1];

    harness.drag(
        inside(*rows.last().expect("three rows")),
        last_member.origin() + vec2f(40., last_member.height() - 1.),
    );

    assert_eq!(harness.members_of(group).len(), members.len() + 1);
    assert_eq!(
        harness.members_of(group).last(),
        Some(&loose),
        "it joined the group somewhere other than its end"
    );
}

#[test]
fn a_pointer_held_still_leaves_the_list_where_it_is() {
    // The list moves under the hand now, so it has to be able to stop. Every
    // step is decided against the frame the last one produced, and a pair of
    // steps that each undo the other is a list that runs at the frame rate
    // under a hand that is holding perfectly still — which is exactly what a
    // hand does while it aims at a boundary.
    //
    // A step towards the pointer is allowed; a second one after the list has
    // settled is not.
    let (mut harness, _) = Harness::grouped_panel(3);
    let rows = panel_rows(&harness.frame());
    let from = inside(rows[3]);
    harness.move_to(from);
    harness.frame();
    harness.hold(from, 1);
    harness.frame();

    // Every boundary in the list, and every pixel of both margins of the
    // group's band — the two four- and eight-pixel strips are exactly where a
    // pair of steps that undid each other would hide.
    for step in 0..120 {
        let at = vec2f(from.x(), rows[0].min_y() + step as f32);
        for _ in 0..5 {
            harness.drag_to(at);
            harness.frame();
        }
        let settled = harness.tab_ids();
        harness.drag_to(at);
        harness.frame();
        assert_eq!(
            harness.tab_ids(),
            settled,
            "the list moved under a pointer that did not, at y {}",
            at.y()
        );
    }
}

#[test]
fn dragging_a_row_onto_a_group_folds_it_into_it() {
    // What the panel is for, end to end: a tab picked up with the pointer and
    // dropped on a group belongs to that group afterwards.
    let (mut harness, group) = Harness::grouped_panel(3);
    let loose = *harness.tab_ids().last().expect("three tabs and a worktree");
    let rows = panel_rows(&harness.frame());
    let members = harness.members_of(group).len();

    // From the last row onto the lower half of the group's first member,
    // which is the gap between the two members.
    let onto = rows[0].origin() + vec2f(40., rows[0].height() * 0.75);
    harness.drag(inside(rows[3]), onto);

    assert_eq!(harness.group_of(loose), Some(group));
    assert_eq!(harness.members_of(group).len(), members + 1);
    assert_eq!(
        harness.tab_ids().len(),
        4,
        "a drag closed or opened something"
    );
}

#[test]
fn dragging_a_member_out_of_a_group_leaves_it() {
    let (mut harness, group) = Harness::grouped_panel(2);
    let member = harness.members_of(group)[1];
    let rows = panel_rows(&harness.frame());

    // Onto the top of the last row, which is below the group entirely.
    let last = *rows.last().expect("three rows");
    harness.drag(inside(rows[1]), last.origin() + vec2f(40., 2.));

    assert_eq!(harness.group_of(member), None);
    assert_eq!(harness.members_of(group).len(), 1);
}

#[test]
fn carrying_a_member_past_the_bottom_of_its_group_takes_it_out() {
    // The gesture a person actually makes, and the one the panel used to
    // refuse: pull the row down until it is below the last row of the group.
    // The strip of padding under that row belongs to the block and to nothing
    // else, so it is the one place that can mean "out of here" without
    // meaning "into whatever is next".
    let (mut harness, group) = Harness::grouped_panel(3);
    let member = harness.members_of(group)[1];
    let rows = panel_rows(&harness.frame());
    let last_member = rows[1];

    // Past the bottom of the last member, into the strip of padding the block
    // keeps under it.
    harness.drag(
        inside(last_member),
        last_member.origin() + vec2f(40., last_member.height() + 12.),
    );

    assert_eq!(harness.group_of(member), None);
    assert_eq!(harness.members_of(group).len(), 1);
    assert_eq!(
        harness.tab_ids().len(),
        4,
        "a drag closed or opened something"
    );
}

#[test]
fn a_member_of_the_last_group_can_still_be_carried_out_of_it() {
    // The case a panel whose groups are decided by the row under the pointer
    // cannot do at all: with nothing drawn below the block there is no row to
    // aim at that could mean "out of this group". The group's own box is what
    // says it instead, so the empty column under the last block says it.
    let (mut harness, group) = Harness::grouped_panel(1);
    let member = harness.members_of(group)[1];
    let rows = panel_rows(&harness.frame());
    let last = *rows.last().expect("a group of two");

    harness.drag(inside(last), last.origin() + vec2f(40., last.height() * 2.));

    assert_eq!(harness.group_of(member), None);
    assert_eq!(harness.members_of(group).len(), 1);
    assert_eq!(
        harness.tab_ids().len(),
        2,
        "a drag closed or opened something"
    );
}

#[test]
fn a_row_carried_past_the_bottom_of_the_list_lands_last() {
    // The end of the list is a place with no row under it, so it is the one
    // position a rule written in terms of neighbours has to be told about.
    let mut harness = Harness::panel(3);
    let first = harness.tab_ids()[0];
    let rows = panel_rows(&harness.frame());
    let last = *rows.last().expect("three rows");

    harness.drag(inside(rows[0]), last.origin() + vec2f(40., last.height()));

    assert_eq!(
        harness.tab_ids().last(),
        Some(&first),
        "the row did not land at the end: {:?}",
        harness.tab_ids()
    );
}

#[test]
fn a_row_flicked_the_length_of_the_list_arrives_with_the_hand() {
    // One event, where the gesture helper sends twenty-four. A hand reports
    // its position far more often than the display draws, and every one of
    // those positions is answered against the same painted list — so a rule
    // that moved the row one place per answer would move it one place per
    // *frame*, and a flick down a long list would put the row down three rows
    // short of where it was let go.
    let mut harness = Harness::panel(6);
    let first = harness.tab_ids()[0];
    let rows = panel_rows(&harness.frame());
    let last = *rows.last().expect("six rows");
    let from = inside(rows[0]);

    harness.move_to(from);
    harness.frame();
    harness.hold(from, 1);
    harness.frame();
    let to = last.origin() + vec2f(40., last.height());
    harness.drag_to(to);
    harness.frame();
    harness.let_go(to);
    harness.frame();

    assert_eq!(
        harness.tab_ids().last(),
        Some(&first),
        "the row was left behind by the hand: {:?}",
        harness.tab_ids()
    );
}

#[test]
fn the_wheel_still_reaches_the_list_under_a_carried_row() {
    // The carried row is painted at the pointer, on a layer above everything,
    // and every element under a point asks whether a layer above it covers
    // that point. A drag image that answered "yes" would take the wheel away
    // for the whole gesture — from the one list a person dragging a row
    // through forty tabs has to be able to scroll.
    let mut harness = Harness::panel(OVERFLOWING);
    let rows = panel_rows(&harness.frame());
    let from = inside(rows[1]);

    harness.move_to(from);
    harness.frame();
    harness.hold(from, 1);
    harness.drag_to(from + vec2f(0., 8.));
    let carried = panel_rows(&harness.frame());

    harness.dispatch(Event::ScrollWheel {
        position: from,
        delta: ScrollDelta::Lines(vec2f(0., -3.)),
        modifiers: Modifiers::default(),
    });

    let scrolled = panel_rows(&harness.frame());
    assert_ne!(
        scrolled.first().map(|row| row.min_y()),
        carried.first().map(|row| row.min_y()),
        "the list did not scroll under the row being carried"
    );
}

#[test]
fn a_filtered_list_keeps_its_filter_while_a_row_is_being_carried() {
    // Every step of a drag asks the strip to move a row, and the strip ending
    // the search is what every other way of asking it for something means.
    // Here it would put the filtered-out tabs back into the list *under the
    // hand*, halfway through the one gesture whose whole meaning is which rows
    // it is passing.
    let mut harness = Harness::panel(3);
    for (index, pane) in harness.pane_ids().into_iter().enumerate() {
        harness.update_session(pane, |session| {
            session.derived_title = Some(format!("kettle {index}"));
        });
    }
    let odd = harness.pane_ids()[1];
    harness.update_session(odd, |session| {
        session.derived_title = Some("teapot".to_owned());
    });

    harness.click_panel_search();
    harness.type_text("kettle");
    let rows = panel_rows(&harness.frame());
    assert_eq!(rows.len(), 2, "the filter kept the wrong rows");

    harness.drag(inside(rows[1]), inside(rows[0]) - vec2f(0., 4.));

    let scene = harness.frame();
    assert_eq!(
        panel_rows(&scene).len(),
        2,
        "the filter was dropped mid-gesture: {:?}",
        panel_text(&scene)
    );
    assert!(
        panel_text(&scene).contains("kettle 2"),
        "the list is not the filtered one any more"
    );
}

#[test]
fn dragging_a_heading_moves_the_whole_group() {
    let (mut harness, group) = Harness::grouped_panel(3);
    let members = harness.members_of(group);
    let scene = harness.frame();
    let rows = panel_rows(&scene);
    let heading = panel_heading(&scene);

    // Past the last row, which is the one gap under everything.
    let last = *rows.last().expect("four rows");
    harness.drag(center(heading), last.origin() + vec2f(40., last.height()));

    let order = harness.tab_ids();
    assert_eq!(
        &order[order.len() - members.len()..],
        members.as_slice(),
        "the group did not land at the end, in one piece: {order:?}"
    );
    assert_eq!(
        harness.members_of(group),
        members,
        "the block lost or gained a member on the way"
    );
}

#[test]
fn a_carried_row_follows_the_pointer_and_leaves_its_slot_behind() {
    // What a drag draws, and all of it: the row is taken out of the list and
    // painted under the hand, and the room it was taking stays taken so that
    // the list shows the slot it will drop into.
    let mut harness = Harness::panel(3);
    let order = harness.tab_ids();
    let rows = panel_rows(&harness.frame());
    let from = inside(rows[2]);

    harness.move_to(from);
    harness.frame();
    harness.hold(from, 1);
    assert_eq!(
        panel_rows(&harness.frame()).len(),
        rows.len(),
        "a press that has not travelled has picked nothing up"
    );

    // Up by more than the threshold and by less than half a row: far enough to
    // be a drag, not far enough to have reached a neighbour's middle.
    let at = from - vec2f(0., 8.);
    harness.drag_to(at);
    let scene = harness.frame();

    carried_row(&scene, at).expect("no row was drawn at the pointer");
    // Two boxes more than there are rows: the hole the row came out of, left
    // open at its old place, and the ground the row is carried on, which is
    // what stops the words on it landing on the words underneath.
    assert_eq!(
        panel_rows(&scene).len(),
        rows.len() + 2,
        "the slot the row came from was closed up instead of left where it was"
    );
    assert_eq!(
        harness.tab_ids(),
        order,
        "the list reordered itself for a gesture that had not passed a row"
    );

    harness.let_go(at);
    let scene = harness.frame();
    assert!(
        carried_row(&scene, at).is_none(),
        "the carried row outlived the gesture"
    );
    assert_eq!(
        panel_rows(&scene).len(),
        rows.len(),
        "the list did not go back to being a list of rows"
    );
}

#[test]
fn a_row_carried_out_over_the_body_still_only_moves_in_its_column() {
    // The panel is one tab wide and there is nowhere else in the window to put
    // a tab, so the row is locked to the column and the hand's height is all
    // of the gesture that means anything. Warp says the same thing with
    // `DragAxis::VerticalOnly`, and says it for the same reason whenever a tab
    // cannot be dragged out into a window of its own.
    let (mut harness, group) = Harness::grouped_panel(2);
    let member = harness.members_of(group)[1];
    let rows = panel_rows(&harness.frame());
    let panel = panel_box(&harness.frame());

    // Level with the top of the first row, but out over the terminal.
    let outside = vec2f(WINDOW.x() - 40., rows[0].min_y() + 2.);
    harness.drag(inside(rows[1]), outside);

    let carried = harness.frame();
    assert!(
        panel_rows(&carried)
            .iter()
            .all(|row| row.max_x() <= panel.max_x()),
        "a row was drawn outside the panel"
    );
    assert_eq!(
        harness.tab_ids().first(),
        Some(&member),
        "the row did not follow the pointer's height"
    );
}

#[test]
fn a_press_that_does_not_travel_still_selects_the_row() {
    // The whole risk of putting a drag on a row: the click has to survive it.
    let (mut harness, _) = Harness::grouped_panel(2);
    let rows = panel_rows(&harness.frame());
    let panes = harness.pane_ids();

    harness.hold(inside(rows[2]), 1);
    // A pixel of wobble, which every hand has — and a slide across the row,
    // which a thumb on a trackpad has. Sideways is not a direction a row in a
    // one-tab-wide column can go, so a gesture that only went that way asked
    // for nothing and is still the click it started as.
    harness.drag_to(inside(rows[2]) + vec2f(0., 1.));
    harness.drag_to(inside(rows[2]) + vec2f(30., 2.));
    harness.let_go(inside(rows[2]) + vec2f(30., 2.));

    assert_eq!(harness.focused_pane_id(), Some(panes[2]));
}

#[test]
fn clicking_a_heading_folds_the_group_away_and_back() {
    let (mut harness, _) = Harness::grouped_panel(2);
    let scene = harness.frame();
    let heading = panel_heading(&scene);
    let rows = panel_rows(&scene).len();

    harness.click(center(heading), MouseButton::Left);

    let folded = panel_rows(&harness.frame());
    assert_eq!(
        folded.len(),
        rows - 2,
        "the group's members did not fold away"
    );
    assert_eq!(
        harness.pane_ids().len(),
        rows,
        "folding a group away closed something"
    );

    let folded_heading = panel_heading(&harness.frame());
    harness.click(center(folded_heading), MouseButton::Left);
    assert_eq!(panel_rows(&harness.frame()).len(), rows);
}

#[test]
fn dragging_a_heading_does_not_also_fold_the_group() {
    // A press on a heading is a click and half a drag, and only the pointer's
    // next move says which. One gesture must not be both.
    let (mut harness, group) = Harness::grouped_panel(3);
    let scene = harness.frame();
    let heading = panel_heading(&scene);
    let last = *panel_rows(&scene).last().expect("four rows");

    harness.drag(center(heading), last.origin() + vec2f(40., last.height()));

    assert!(
        !harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .tabs()
                .group(group)
                .is_some_and(crate::tab::TabGroup::is_collapsed)
        }),
        "the drag folded the group away as well as moving it"
    );
}

#[test]
fn the_heading_closes_every_tab_in_the_group() {
    let (mut harness, group) = Harness::grouped_panel(3);
    let members = harness.members_of(group);
    let survivors: Vec<TabId> = harness
        .tab_ids()
        .into_iter()
        .filter(|tab| !members.contains(tab))
        .collect();

    // The cross only draws while the pointer is on the heading, exactly as a
    // row's does.
    let heading = panel_heading(&harness.frame());
    harness.move_to(center(heading));
    let scene = harness.frame();
    let crosses = icons_in(&scene, panel_box(&scene), Lucide::X);
    let cross = crosses
        .iter()
        .find(|bounds| bounds.min_y() < heading.max_y() && bounds.max_y() > heading.min_y())
        .copied()
        .expect("the heading drew no close button while it was hovered");

    harness.click(center(cross), MouseButton::Left);

    assert_eq!(harness.tab_ids(), survivors);
    assert!(harness.group_of(survivors[0]).is_none());
}

#[test]
fn right_clicking_a_panel_row_opens_the_menu_too() {
    // The panel and the strip answer the same gesture, because the rule is
    // about the tab rather than about how the tab is drawn.
    let mut harness = Harness::seeded_panel();
    let row = panel_rows(&harness.frame())[0];

    harness.click(
        row.origin() + vec2f(40., row.height() / 2.),
        MouseButton::Right,
    );

    assert!(
        tab_menu_box(&harness.frame()).is_some(),
        "a right press on a panel row opened no menu"
    );
}

#[test]
fn a_panel_rows_close_button_closes_the_pane_it_names() {
    let mut harness = Harness::panel(1);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    let panes = harness.pane_ids();

    let buttons = close_boxes(&harness.frame());
    assert_eq!(buttons.len(), 1, "only the selected row draws one at rest");
    harness.click(center(buttons[0]), MouseButton::Left);

    assert_eq!(harness.pane_ids().len(), 1, "the pane did not close");
    assert_eq!(harness.tab_ids().len(), 1, "closing a pane closed its tab");
    assert!(!harness.pane_ids().contains(&panes[1]));
}

#[test]
fn the_options_menu_opens_from_the_panel_and_stays_inside_it() {
    // The whole of why the anchor names a corner: right edges aligned, because
    // a 200px menu hung leftwards off a 248px column opens across the body,
    // and `keep_on_screen` would not pull it back — the window has plenty of
    // room to its right.
    let mut harness = Harness::seeded_panel();
    let panel = panel_box(&harness.frame());

    let ground = empty_list_space(&harness.frame());
    harness.click(ground, MouseButton::Right);
    assert!(harness.is_menu_open());

    let menu = menu_box(&harness.frame());
    assert!(
        menu.min_x() >= panel.min_x() && menu.max_x() <= panel.max_x() + 0.5,
        "the menu spans {} to {} and the panel {} to {}",
        menu.min_x(),
        menu.max_x(),
        panel.min_x(),
        panel.max_x()
    );
    assert!(
        panel.contains_point(center(menu)),
        "the menu at {menu:?} is not inside the panel at {panel:?}"
    );
}

#[test]
fn the_band_under_the_list_lights_up_whole_rather_than_around_its_mark() {
    // The `+` is the end of the list, so it is as wide as a row and it lights
    // like one. A highlight the size of the glyph would be a smaller target
    // than the one in the bar this replaced, and it would read as a mark
    // somebody left there rather than as somewhere to press.
    let mut harness = Harness::seeded_panel();
    let scene = harness.frame();
    let plus = plus_box(&scene);
    let last = *panel_rows(&scene).last().expect("a row");

    assert!(
        (plus.width() - last.width()).abs() < 0.5,
        "the `+` is {plus:?} and a row is {last:?}; the two do not line up"
    );

    // Its left edge, as far from the glyph in the middle as the band allows.
    harness.move_to(vec2f(plus.min_x() + 4., center(plus).y()));
    let scene = harness.frame();

    let lit: Vec<RectF> = visible_rects(&scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(5.)
                && rect.background == Fill::Solid(theme().overlay_1)
        })
        .map(|(_, bounds)| bounds)
        .collect();

    assert_eq!(
        lit,
        vec![plus],
        "hovering the edge of the band lit {lit:?} rather than the whole of it"
    );
}

#[test]
fn the_panel_and_the_settings_rail_put_their_search_box_in_the_same_place() {
    // Two sidebars in one column, so one box. Copied rather than chosen: a
    // field a few pixels wider or higher in one section than in the other is
    // the kind of difference nobody can name and everybody can see when the
    // buttons at the foot of the panel switch between them.
    let mut harness = Harness::seeded_panel();
    let tabs = panel_search_box(&harness.frame());

    harness.open_settings_page();
    let rail = panel_search_box(&harness.frame());

    assert_eq!(
        tabs, rail,
        "the tab list puts its search box at {tabs:?} and the settings rail       puts the same box at {rail:?}"
    );
}

#[test]
fn a_sidebar_section_gets_neither_the_plus_nor_the_menu_the_list_carries() {
    // Both belong to the tab list, and the tab list is one section of the
    // sidebar. `tabs_panel::render` draws the chrome for every section, so a
    // `+` added there rather than to the list itself would offer a new agent
    // tab from the middle of the settings rail.
    let mut harness = Harness::seeded_panel();
    harness.open_settings_page();
    let scene = harness.frame();

    assert!(
        rects_rounded_by(&scene, Radius::Pixels(5.)).is_empty(),
        "the settings rail is showing and the tab list's `+` is still drawn"
    );

    let panel = panel_box(&scene);
    harness.click(
        vec2f(center(panel).x(), panel.max_y() - 90.),
        MouseButton::Right,
    );
    assert!(
        !harness.is_menu_open(),
        "a press on the settings rail opened the tab list's options menu"
    );
}

#[test]
fn a_secondary_press_on_a_row_opens_that_row_s_menu_and_not_the_list_s() {
    // Both menus answer the same button, one layer apart. The row wins because
    // the ground is a sibling *under* the list rather than a wrapper around it:
    // a stack asks its topmost child first and stops at the one that claims the
    // press, and the ground is covered by every rect a row painted.
    let mut harness = Harness::seeded_panel();
    let row = panel_rows(&harness.frame())[0];

    harness.click(
        row.origin() + vec2f(40., row.height() / 2.),
        MouseButton::Right,
    );

    assert!(
        tab_menu_box(&harness.frame()).is_some(),
        "the row's own menu did not open"
    );
    assert!(
        !harness.is_menu_open(),
        "a press on a row opened the list's options menu as well"
    );
}

#[test]
fn the_hover_card_opens_beside_a_panel_row_rather_than_below_it() {
    // Below a panel row is where the next rows are. Warp opens its sidecar on
    // the side away from the panel for exactly that reason.
    let mut harness = Harness::seeded_panel();
    let rows = panel_rows(&harness.frame());
    let panel = panel_box(&harness.frame());

    harness.move_to(center(rows[0]));
    let scene = harness.frame();
    let cards = detail_cards(&scene);

    assert_eq!(cards.len(), 1, "hovering a row opened no card");
    assert!(
        cards[0].min_x() >= panel.max_x(),
        "the card starts at {} and the panel ends at {}",
        cards[0].min_x(),
        panel.max_x()
    );
    assert!(
        cards[0].min_y() < rows[0].max_y(),
        "the card was hung below the row, over the rows underneath it"
    );
}

// --- what the adversarial review found -------------------------------------

/// The `overlay_2` hairlines inside `card`, which are what divide its sections.
fn card_dividers(scene: &Scene, card: RectF) -> Vec<RectF> {
    visible_rects(scene)
        .filter(|(rect, _)| {
            rect.background == Fill::Solid(theme().overlay_2)
                && (rect.bounds.height() - 1.).abs() < 0.01
                && card.contains_point(rect.bounds.origin())
        })
        .map(|(_, bounds)| bounds)
        .collect()
}

#[test]
fn the_hover_card_never_slides_back_over_the_row_that_opened_it() {
    // A 320px card beside a 248px panel needs 559px of window, and the window
    // can be dragged to 480. Slid back to fit, the card lands on its own row:
    // the row stops hit-testing as hovered, so the card is torn down and
    // re-armed frame after frame, and the close button under it is
    // unreachable. Warp narrows its sidecar and then lets it clip off the
    // window edge rather than move back across the panel.
    const NARROW: Vector2F = vec2f(480., 640.);

    let mut harness = Harness::seeded_panel();
    let scene = harness.frame_sized(NARROW);
    let row = panel_rows(&scene)[0];

    harness.move_to(center(row));
    let scene = harness.frame_sized(NARROW);
    let cards = detail_cards(&scene);
    assert_eq!(cards.len(), 1, "hovering a row opened no card");
    assert!(
        cards[0].min_x() >= row.max_x(),
        "the card at {:?} was slid back over the row at {row:?} in a {}px \
         window",
        cards[0],
        NARROW.x()
    );

    // Still hovered, which is the consequence rather than the geometry: the
    // pointer has not moved and the card is still up on the next frame.
    let scene = harness.frame_sized(NARROW);
    assert_eq!(
        detail_cards(&scene).len(),
        1,
        "the card took the pointer off its own row and tore itself down"
    );
}

#[test]
fn a_tabs_view_card_describes_every_pane_of_the_tab_it_stands_for() {
    // `Tabs` granularity drops a tab's other panes from the list entirely —
    // no count, no expander — so Warp's sidecar target follows the
    // granularity: one section per visible pane, and this is the only place
    // those panes surface.
    let mut harness = Harness::panel(1);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    let panes = harness.active_pane_ids();
    assert_eq!(panes.len(), 2, "the split opened no second pane");
    for pane in &panes {
        harness.seed(*pane, Some(seeded_diff()));
    }

    let row = panel_rows(&harness.frame())[0];
    harness.move_to(center(row));
    let scene = harness.frame();
    let one_pane = detail_cards(&scene)[0];
    assert!(
        card_dividers(&scene, one_pane).is_empty(),
        "a Panes card is one pane and needs no divider"
    );

    harness.set_granularity(Granularity::Tabs);
    let rows = panel_rows(&harness.frame());
    assert_eq!(rows.len(), 1, "Tabs drew more than one row for one tab");
    harness.move_to(center(rows[0]));
    let scene = harness.frame();
    let whole_tab = detail_cards(&scene)[0];

    assert_eq!(
        card_dividers(&scene, whole_tab).len(),
        1,
        "the card for a two-pane tab has no hairline in it, so it is still \
         describing one pane"
    );
    // A card is its 1px border, then its sections with a hairline between
    // them: a one-section card is `2 + section`, and a two-section card is
    // `2 + section + 1 + section`.
    let section = one_pane.height() - 2.;
    assert!(
        (whole_tab.height() - (2. + 2. * section + 1.)).abs() < 0.5,
        "the tab's card is {whole_tab:?} and one pane's is {one_pane:?}, which \
         is not two sections and a hairline"
    );
}

#[test]
fn clicking_a_split_tabs_heading_selects_it_without_moving_the_focus_inside_it() {
    // Warp's `render_group_header` dispatches `ActivateTab`. Its *container*
    // takes right-clicks and middle-clicks and no left-click at all, which is
    // why the 8px inset and the gaps between rows stay inert here too.
    let mut harness = Harness::panel(2);
    let tabs = harness.tab_ids();
    harness.dispatch_action(TabAction::Select(tabs[0]));
    harness.dispatch_action(TabAction::Split(Direction::Right));
    let split_panes = harness.active_pane_ids();
    harness.dispatch_action(TabAction::Select(tabs[1]));

    let scene = harness.frame();
    let rows = panel_rows(&scene);
    assert_eq!(rows.len(), 3, "two tabs, three panes, three rows");
    assert_eq!(harness.active_id(), tabs[1]);

    // The heading's own band: its bottom padding is what separates it from the
    // first row, and the body's top padding is zero when it is there.
    let heading = vec2f(rows[0].min_x() + 10., rows[0].min_y() - 2.);
    harness.click(heading, MouseButton::Left);

    assert_eq!(
        harness.active_id(),
        tabs[0],
        "clicking the heading of a split tab left the other tab active, so \
         the only way to reach it with a mouse is to click one of its rows"
    );
    assert_eq!(
        harness.focused_pane_id(),
        Some(split_panes[1]),
        "the heading re-targeted which pane is focused; that is what a row \
         does, and the two signals are meant to stay separate"
    );
}

#[test]
fn a_row_armed_before_the_menu_opened_puts_no_card_over_the_menus_underlay() {
    // A card and the menu are both anchored overlays, and the later of the two
    // covers the earlier. The menu is added last on the ground under the list,
    // so it wins for any card the same frame builds — but a card armed before
    // and left armed would come back on a later frame, land in a later overlay
    // layer, and cover the modal underlay whose whole job is to catch the press
    // that dismisses. `--menu --hover` applies the two in that order.
    let mut harness = Harness::seeded_panel();
    let row = panel_rows(&harness.frame())[0];

    harness.dispatch_option(OptionsAction::TogglePopup);
    harness.hover_first_row();

    let scene = harness.frame();
    assert!(harness.is_menu_open(), "the menu did not open");
    assert!(
        detail_cards(&scene).is_empty(),
        "a card is up under the menu that froze the window"
    );

    // Where the card would have been, which is outside the menu and so a
    // dismiss.
    let beside = vec2f(row.max_x() + 100., center(row).y());
    assert!(!menu_box(&scene).contains_point(beside));
    harness.click(beside, MouseButton::Left);
    assert!(
        !harness.is_menu_open(),
        "a click outside the menu did not close it, so something painted \
         after it swallowed the press"
    );
}

#[test]
fn choosing_one_option_does_not_carry_another_command_line_override_into_the_file() {
    // `persisted` is the single place an override is stripped out of a save,
    // and the early-save branch is the one write path that can skip it. It is
    // also permanent if it does: the flag stays set, so every later save reads
    // the poisoned value back out of the file and writes it again.
    let scratch = Scratch::new();
    let mut harness = Harness::with_settings(1, scratch.settings());
    harness.override_granularity(Granularity::Tabs);
    harness.override_density(Density::Expanded);

    // The density that is already on screen: the one click that takes the
    // early-save branch, and the case that branch exists for.
    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));

    let written = scratch.written_containing("\"view_mode\": \"expanded\"");
    assert!(
        written.contains("\"display_granularity\": \"panes\""),
        "choosing a density adopted `--granularity tabs` as well, so the next \
         launch with no flags opens showing tabs; the file holds {written}"
    );
    assert_eq!(Granularity::Panes, harness.saved_options().granularity);
    assert_eq!(
        Granularity::Tabs,
        harness.options().granularity,
        "the save changed what is on screen"
    );
}

/// Where the settings are drawn: the window, minus the sidebar and the header.
///
/// A synthetic box rather than a painted one. The settings are a *section*
/// now: the rail is in the sidebar and the page is the window's whole body,
/// which paints no surface of its own — so what bounds the page is the two
/// things beside it.
fn settings_pane_box(scene: &Scene) -> RectF {
    let panel = panel_box(scene);
    RectF::new(
        vec2f(panel.max_x(), 0.),
        vec2f(WINDOW.x() - panel.max_x(), WINDOW.y()),
    )
}

/// The card's switches, top to bottom, by the round track they are painted on.
fn settings_switch_boxes(scene: &Scene) -> Vec<RectF> {
    let pane = settings_pane_box(scene);
    let mut switches: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| rect.corner_radius.get_top_left() == Radius::Percentage(50.))
        .map(|(_, bounds)| bounds)
        .filter(|bounds| pane.contains_point(center(*bounds)) && (bounds.width() - 28.).abs() < 0.5)
        .collect();
    switches.sort_by(|left, right| left.min_y().total_cmp(&right.min_y()));
    switches
}

/// The boxes a card makes a decision in, top to bottom: the Enabled box, the
/// permission box, the plugin's own row.
///
/// Found by the box's own ground — `overlay_1`, rounded by
/// [`widgets::ANSWER_RADIUS`] — inside the page beside the list. A hovered row
/// is `overlay_1` too and is rounded by six; the field is bordered and is in
/// the sidebar.
fn answer_boxes(scene: &Scene) -> Vec<RectF> {
    let pane = settings_pane_box(scene);
    let mut boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.background == Fill::Solid(theme().overlay_1)
                && rect.corner_radius.get_top_left()
                    == Radius::Pixels(crate::workspace::settings_page::widgets::ANSWER_RADIUS)
        })
        .map(|(_, bounds)| bounds)
        .filter(|bounds| pane.contains_point(center(*bounds)))
        .collect();
    boxes.sort_by(|left, right| left.min_y().total_cmp(&right.min_y()));
    boxes
}

/// The lines of text on the page beside the list, top to bottom, with where
/// each starts.
fn page_lines(scene: &Scene) -> Vec<(Vector2F, String)> {
    let pane = settings_pane_box(scene);
    text_lines(scene, |at| pane.contains_point(at))
}

/// The one line of the page that says `phrase`, and where it starts.
///
/// A point four pixels under the baseline of the first glyph — inside the
/// line's own box, and inside any padding a box gives its text — which is
/// what a test asks "is this line inside that box" with.
fn page_line(scene: &Scene, phrase: &str) -> (Vector2F, String) {
    let (at, line) = page_lines(scene)
        .into_iter()
        .find(|(_, line)| line.contains(phrase))
        .unwrap_or_else(|| panic!("no line on the page says {phrase:?}: {}", frame_text(scene)));
    (at + vec2f(4., 4.), line)
}

/// Where the first glyph of `word` is, on the first line of the page that
/// says it.
fn page_word(scene: &Scene, word: &str) -> Vector2F {
    word_in(scene, settings_pane_box(scene), word)
}

/// The same, on the first line inside `bounds` that says it.
fn word_in(scene: &Scene, bounds: RectF, word: &str) -> Vector2F {
    let mut rows: HashMap<i32, Vec<(f32, char)>> = HashMap::new();
    for glyph in scene.layers().flat_map(|layer| layer.glyphs.iter()) {
        let Some(character) = char::from_u32(glyph.glyph_key.glyph_id) else {
            continue;
        };
        if !bounds.contains_point(glyph.position) {
            continue;
        }
        rows.entry(glyph.position.y().round() as i32)
            .or_default()
            .push((glyph.position.x(), character));
    }
    let mut lines: Vec<(i32, Vec<(f32, char)>)> = rows.into_iter().collect();
    lines.sort_by_key(|(y, _)| *y);
    for (y, mut glyphs) in lines {
        glyphs.sort_by(|left, right| left.0.total_cmp(&right.0));
        let text: String = glyphs.iter().map(|(_, character)| *character).collect();
        if let Some(offset) = text.find(word) {
            let index = text[..offset].chars().count();
            return vec2f(glyphs[index].0, y as f32);
        }
    }
    panic!("no line in {bounds:?} says {word:?}: {}", frame_text(scene));
}

/// The colour the line of the page that says `phrase` is set in.
///
/// The first glyph's, which is the line's: a line is one `Text` or one line
/// of one `Paragraph`, and either has one colour.
fn page_line_color(scene: &Scene, phrase: &str) -> Color {
    let (at, _) = page_line(scene, phrase);
    let at = at - vec2f(4., 4.);
    scene
        .layers()
        .flat_map(|layer| layer.glyphs.iter())
        .find(|glyph| glyph.position.x() == at.x() && glyph.position.y().round() == at.y())
        .map(|glyph| glyph.color)
        .unwrap_or_else(|| panic!("no glyph starts the line that says {phrase:?}"))
}

/// Whether the frame says `phrase`, wherever the paragraph wrapped.
///
/// [`frame_text`] joins the lines it found with nothing between them, so a
/// sentence that wrapped comes back with two of its words run together.
/// Comparing both sides with their spaces taken out is what lets a test name
/// a phrase without also knowing the width the card came out at.
fn says(scene: &Scene, phrase: &str) -> bool {
    let bare = |text: &str| text.split_whitespace().collect::<String>();
    bare(&frame_text(scene)).contains(&bare(phrase))
}

/// The text fields anywhere in the frame, left to right.
///
/// Found by the field's own ground: a rounded box in `overlay_1` exactly
/// [`text_field::HEIGHT`] tall, **with a border**.
///
/// The border is what tells a field from a row of the sidebar, and it is not
/// decoration in this filter. A hovered row is drawn in `overlay_1` too, and a
/// row of a section's list is 26.4 tall against the field's 26 — inside the
/// half-pixel slack this has to allow. Only the field is stroked, in every one
/// of its three states, so only the field is counted.
fn settings_field_boxes(scene: &Scene) -> Vec<RectF> {
    let mut fields: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, bounds)| {
            rect.background == Fill::Solid(theme().overlay_1)
                && rect.border != Border::default()
                && (bounds.height() - crate::workspace::text_field::HEIGHT).abs() < 0.5
        })
        .map(|(_, bounds)| bounds)
        .collect();
    fields.sort_by(|left, right| left.min_x().total_cmp(&right.min_x()));
    fields
}

/// Whether two boxes share any pixels.
fn overlaps(left: RectF, right: RectF) -> bool {
    left.min_x() < right.max_x()
        && right.min_x() < left.max_x()
        && left.min_y() < right.max_y()
        && right.min_y() < left.max_y()
}

/// The `+` button's box, by the only 5px-rounded rect in the frame.
///
/// Five is what keeps it out of every other helper here: a row, a card and a
/// close button are all rounded by four, and the `+` has been the one thing in
/// the frame rounded by five since it was in the control bar.
fn plus_box(scene: &Scene) -> RectF {
    let boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| rect.corner_radius.get_top_left() == Radius::Pixels(5.))
        .map(|(_, bounds)| bounds)
        .collect();
    assert_eq!(boxes.len(), 1, "expected one + button, got {boxes:?}");
    boxes[0]
}

/// The buttons on the settings page itself, in reading order.
///
/// Found the way the rail's rows are — a six-pixel box — and told apart from
/// them by being on the page rather than in the sidebar. On the Keyboard
/// Shortcuts page that is two per row: the chord, and the button that puts it
/// back.
fn settings_page_button_boxes(scene: &Scene) -> Vec<RectF> {
    let panel = panel_box(scene);
    let mut buttons: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| rect.corner_radius.get_top_left() == Radius::Pixels(6.))
        .filter(|(rect, _)| rect.border != Border::default())
        .map(|(_, bounds)| bounds)
        .filter(|bounds| center(*bounds).x() > panel.max_x())
        .collect();
    buttons.sort_by(|left, right| {
        left.min_y()
            .total_cmp(&right.min_y())
            .then(left.min_x().total_cmp(&right.min_x()))
    });
    buttons
}

/// The rail's page buttons, top to bottom.
///
/// Found by their rounded box *and* by being in the sidebar, because the page
/// beside them rounds its one button by the same six pixels. The rail is the
/// sidebar's body while the settings are showing.
fn settings_rail_boxes(scene: &Scene) -> Vec<RectF> {
    let panel = panel_box(scene);
    let mut rows: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| rect.corner_radius.get_top_left() == Radius::Pixels(6.))
        // The search box above them is the same shape, and is outlined where a
        // page button never is.
        .filter(|(rect, _)| rect.border == Border::default())
        .map(|(_, bounds)| bounds)
        .filter(|bounds| center(*bounds).x() < panel.max_x())
        .collect();
    rows.sort_by(|left, right| left.min_y().total_cmp(&right.min_y()));
    rows
}

/// The one button on the page, by its outline.
///
/// `None` while the button is drawn in its disabled state, which is exactly
/// what "there is nothing to reset" looks like: the outline is what goes.
fn settings_button_box(scene: &Scene) -> Option<RectF> {
    let pane = settings_pane_box(scene);
    visible_rects(scene)
        // By the border's *colour*: a disabled button keeps its stroke and
        // paints it in nothing at all, which is the whole of how the reset
        // control says there is nothing to reset.
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                && rect.border.color == Fill::Solid(theme().overlay_3)
        })
        .map(|(_, bounds)| bounds)
        // In the page rather than in the sidebar, which the two are now on
        // opposite sides of.
        .find(|bounds| pane.contains_point(center(*bounds)))
}

/// The "Current theme" row on the settings page.
///
/// By its ten-pixel radius, which is the one thing on that page shaped like a
/// card and rounded like a pane rather than like a tab.
fn settings_theme_row(scene: &Scene) -> RectF {
    let pane = settings_pane_box(scene);
    let rows: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(10.) && rect.border.width >= 1.
        })
        .map(|(_, bounds)| bounds)
        .filter(|bounds| pane.contains_point(center(*bounds)))
        .collect();

    assert_eq!(rows.len(), 1, "exactly one current-theme row on the page");
    rows[0]
}

/// The theme cards in the Themes panel, top to bottom.
///
/// Found by the one thing only a card is: a rounded box painted in a theme's
/// *terminal* background, at the panel's own card width. Every other rounded
/// box in the window is painted in the palette in force.
fn theme_cards(scene: &Scene) -> Vec<RectF> {
    let mut cards: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                && rect.border.width >= 1.
                // Within two pixels of the panel's card: a bordered container
                // paints its box a pixel narrower than the width it was
                // constrained to on each side.
                && (rect.bounds.width() - 190.).abs() < 2.5
        })
        .map(|(_, bounds)| bounds)
        .collect();
    cards.sort_by(|left, right| left.min_y().total_cmp(&right.min_y()));
    cards
}

/// The creator's two buttons, left to right: Cancel, then Create.
fn creator_buttons(scene: &Scene) -> Vec<RectF> {
    let mut buttons: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                && rect.border.width >= 1.
                && (rect.bounds.height() - 27.2).abs() < 1.
        })
        .map(|(_, bounds)| bounds)
        .collect();
    buttons.sort_by(|left, right| left.min_x().total_cmp(&right.min_x()));
    buttons
}

/// The five swatches the creator offers, left to right.
///
/// Filled as well as 40 tall. A swatch *is* a colour, so it always has a fill,
/// and height on its own caught an unpainted container in the tabs panel
/// behind this the day that container changed height.
fn creator_swatches(scene: &Scene) -> Vec<RectF> {
    let mut swatches: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| rect.background != Fill::None)
        .map(|(_, bounds)| bounds)
        .filter(|bounds| (bounds.height() - 40.).abs() < 0.5)
        .collect();
    swatches.sort_by(|left, right| left.min_x().total_cmp(&right.min_x()));
    swatches
}

#[test]
fn the_settings_page_shows_the_theme_in_force_and_opens_the_panel() {
    // Warp's shape: the settings page does not list themes, it shows *the*
    // theme and leads to the panel. A list of themes on a settings page is a
    // list you look at instead of your work.
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    let themes = Scratch::new();
    harness.set_themes_directory(themes.path().to_owned());
    harness.open_settings_page();

    let scene = harness.frame();
    assert!(
        frame_text(&scene).contains("Crook Dark"),
        "the settings page does not say which theme is in force"
    );
    assert!(!harness.is_theme_panel_open());

    let row = settings_theme_row(&scene);
    harness.click(center(row), MouseButton::Left);
    assert!(
        harness.is_theme_panel_open(),
        "clicking the current theme did not open the panel"
    );
}

#[test]
fn choosing_a_theme_in_the_panel_repaints_saves_and_reaches_the_shells() {
    // The whole of what applying a theme has to do. The last of the three is
    // the one that is easy to miss: a grid resolves its colours through a
    // palette handed to it when its shell started.
    let scratch = Scratch::new();
    let themes = Scratch::new();
    let mut harness = Harness::with_settings(1, scratch.settings());
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    // Pointed at an empty folder of this test's own: a list read from the real
    // one would assert something about the machine the test runs on, and a
    // theme file there called "Crook Light" would make this fail for a reason
    // that has nothing to do with the panel.
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    let cards = theme_cards(&harness.frame());
    // The cards that are *visible*: the list scrolls, and every theme that
    // ships has a row whether or not the window is tall enough to show it.
    assert!(
        cards.len() >= 2,
        "the panel drew {} cards, so there is nothing to click",
        cards.len()
    );

    // The second card is the light one, and it is the one that proves a theme
    // reaches everything: nothing about a dark theme replacing another dark
    // theme is visible in a test.
    harness.click(center(cards[1]), MouseButton::Left);

    assert_eq!(harness.theme_name(), "Crook Light");
    assert!(crate::theme::theme().is_light);
    assert_eq!(
        crate::terminal_model::crook_palette().background,
        crook_terminal::Rgb::new(
            crate::theme::theme().terminal.background.r,
            crate::theme::theme().terminal.background.g,
            crate::theme::theme().terminal.background.b,
        ),
        "the palette a shell is drawn through is not the theme's"
    );

    let written = scratch.written_containing("\"theme\"");
    assert!(
        written.contains("\"theme\": \"Crook Light\""),
        "the chosen theme did not reach the settings file: {written}"
    );
}

#[test]
fn the_arrow_keys_browse_the_panel_and_the_shell_never_sees_them() {
    // Warp's arrow keys move the selection *and* apply, so browsing is
    // browsing the real thing. The half that is easy to get wrong is the other
    // one: with a pane focused, a bare Up is the shell's history key, and one
    // keystroke that moved the selection *and* recalled a line would be worse
    // than either.
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    let themes = Scratch::new();
    harness.set_themes_directory(themes.path().to_owned());
    harness.open_theme_panel();
    harness.frame();

    assert!(harness.press_key("down", Modifiers::default()));
    assert_eq!(harness.theme_name(), "Crook Light");
    assert!(harness.press_key("down", Modifiers::default()));
    assert_eq!(harness.theme_name(), "Midnight");

    // At the end it stops rather than wrapping — and it still *takes* the
    // key, which is the point: a Down that fell through to the shell at the
    // bottom of the list would recall a line of history from a panel the
    // person is looking at.
    let last = harness.theme_names().last().cloned().expect("themes");
    for _ in 0..harness.theme_names().len() + 2 {
        assert!(harness.press_key("down", Modifiers::default()));
    }
    assert_eq!(harness.theme_name(), last);
    assert!(harness.press_key("down", Modifiers::default()));
    assert_eq!(harness.theme_name(), last);

    // Back to the top the same way.
    for _ in 0..harness.theme_names().len() + 2 {
        assert!(harness.press_key("up", Modifiers::default()));
    }
    assert_eq!(harness.theme_name(), "Crook Dark");

    // The pane's field is not listening while the panel is up, which is the
    // other half of the same door.
    assert!(
        !harness.pane_takes_keys(),
        "the shell's field is still listening while the panel owns the arrows"
    );
}

#[test]
fn escape_and_enter_both_put_the_panel_away() {
    // Nothing is uncommitted — moving the selection has already applied and
    // saved — so confirm and dismiss are the same gesture with two keys.
    for key in ["escape", "enter"] {
        let mut harness = Harness::new(1);
        let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
        let themes = Scratch::new();
        harness.set_themes_directory(themes.path().to_owned());
        harness.open_theme_panel();
        assert!(harness.is_theme_panel_open());

        assert!(harness.press_key(key, Modifiers::default()));
        assert!(
            !harness.is_theme_panel_open(),
            "{key} did not close the panel"
        );
        assert!(
            harness.pane_takes_keys(),
            "{key} closed the panel and left the shell without its keyboard"
        );
    }
}

#[test]
fn a_chord_still_reaches_the_window_while_the_panel_is_up() {
    // The panel takes the *unmodified* keys it uses and nothing else: a chord
    // is a window command wherever the pointer is, and a panel that swallowed
    // `cmd/ctrl-t` would be a panel you had to close to open a tab.
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    let themes = Scratch::new();
    harness.set_themes_directory(themes.path().to_owned());
    harness.open_theme_panel();

    let before = harness.tab_ids().len();
    assert!(harness.press_key("t", platform_chord()));
    assert_eq!(harness.tab_ids().len(), before + 1);
    assert!(
        harness.is_theme_panel_open(),
        "opening a tab closed the panel"
    );
}

#[test]
fn the_creator_builds_a_theme_from_the_one_in_force_and_writes_it_to_the_folder() {
    // Warp's creator makes a theme out of a photograph; Crook has no image
    // decoder, so it makes one out of the palette in force — five candidate
    // colours, one click, everything else decided. What it produces is a file
    // in the themes folder, in the same format as one downloaded from
    // anywhere else.
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    harness.start_creating();
    let scene = harness.frame();

    let swatches = creator_swatches(&scene);
    assert_eq!(swatches.len(), 5, "five candidates, five swatches");

    // The draft is live: picking a background repaints the window rather than
    // a preview card.
    let before = crate::theme::theme();
    harness.click(center(swatches[3]), MouseButton::Left);
    assert_ne!(crate::theme::theme(), before, "the draft is not on screen");

    harness.create_theme();

    assert!(
        !harness.is_creating(),
        "the creator stayed open after saving"
    );
    assert_eq!(
        harness.theme_name(),
        "Crook Dark variant",
        "the theme that was just made is not the one in force"
    );

    let written: Vec<PathBuf> = fs::read_dir(themes.path())
        .expect("the themes folder should be readable")
        .flatten()
        .map(|entry: fs::DirEntry| entry.path())
        .collect();
    assert_eq!(written.len(), 1, "one theme should have been written");
    let read_back = crate::theme::load_themes_in(themes.path());
    assert_eq!(read_back.len(), 1);
    assert_eq!(read_back[0].name, "Crook Dark variant");

    // And it is in the list, from a file.
    let listed = harness.theme_names();
    assert!(
        listed.contains(&"Crook Dark variant".to_owned()),
        "the new theme is not in the panel's list: {listed:?}"
    );
}

#[test]
fn cancelling_the_creator_puts_back_the_theme_it_opened_over() {
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    harness.start_creating();
    harness.frame();
    assert_ne!(crate::theme::theme(), crate::theme::DARK);

    harness.cancel_creating();

    assert!(!harness.is_creating());
    assert_eq!(
        crate::theme::theme(),
        crate::theme::DARK,
        "cancelling left the draft on screen"
    );
    assert_eq!(
        fs::read_dir(themes.path())
            .expect("readable")
            .flatten()
            .count(),
        0,
        "cancelling wrote a theme anyway"
    );
}

#[test]
fn a_theme_dropped_into_the_folder_is_in_the_panel_the_next_time_it_opens() {
    // Warp watches its themes directory; Crook re-reads it on the gesture that
    // precedes choosing a theme, which is opening a surface that lists them.
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    let before = harness.theme_names().len();
    assert_eq!(
        before,
        crate::theme::BUILTIN.len(),
        "only the built-ins to start"
    );

    fs::write(themes.path().join("my_own.yaml"), theme_file_text()).expect("writable");
    harness.close_theme_panel();
    harness.open_theme_panel();

    let names = harness.theme_names();
    assert_eq!(
        names.len(),
        before + 1,
        "the new theme is not listed: {names:?}"
    );
    assert!(names.contains(&"My Own".to_owned()));
}

#[test]
fn a_theme_file_edited_while_the_panel_is_open_is_re_read_and_re_applied() {
    // The half of a filesystem watcher that matters: somebody is editing a
    // theme in one window and looking at Crook in the other. The panel being
    // open is what the poll is gated on, because that is when it is happening.
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    let path = themes.path().join("my_own.yaml");
    fs::write(&path, theme_file_text()).expect("writable");
    harness.open_theme_panel();
    harness.workspace_update(|workspace, ctx| workspace.set_theme("My Own", ctx));

    let before = crate::theme::theme().terminal.background;
    assert_eq!(harness.theme_name(), "My Own");

    // The same theme, one colour different, saved under the same name.
    fs::write(&path, theme_file_text().replace("#2e3440", "#101010")).expect("writable");
    harness.settle_for(std::time::Duration::from_secs(5), |harness| {
        let _ = harness;
        crate::theme::theme().terminal.background != before
    });

    assert_ne!(
        crate::theme::theme().terminal.background,
        before,
        "the edit never reached the window"
    );
    assert_eq!(
        harness.theme_name(),
        "My Own",
        "it is still the same theme, re-read"
    );
}

#[test]
fn the_themes_folder_is_not_polled_while_the_panel_is_closed() {
    // An application that is idle by design stays idle: the chain ends at the
    // first tick that finds the panel gone.
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    let before = harness.theme_names().len();
    fs::write(themes.path().join("my_own.yaml"), theme_file_text()).expect("writable");

    // Two poll intervals, which is one more than it takes for a poll that was
    // running to have noticed. Longer would only make the suite slower.
    harness.settle_for(super::view::THEMES_POLL * 2, |_| false);

    assert_eq!(
        harness.theme_names().len(),
        before,
        "the list moved with nobody looking at it"
    );
}

/// A theme file in Warp's format, for the tests that drop one in.
fn theme_file_text() -> String {
    let mut text = String::from(
        "accent: '#88c0d0'\nbackground: '#2e3440'\nforeground: '#d8dee9'\nterminal_colors:\n",
    );
    for block in ["normal", "bright"] {
        text.push_str(&format!("  {block}:\n"));
        for name in [
            "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        ] {
            text.push_str(&format!("    {name}: '#88c0d0'\n"));
        }
    }
    text
}

#[test]
fn a_theme_this_machine_does_not_have_is_refused_rather_than_swapped_for_another() {
    // A theme file can be deleted between two launches, and the name in the
    // settings file is then a name nothing answers to. Quietly adopting a
    // different theme would lose the choice: put the file back and it comes
    // back.
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);

    let before = harness.theme_name();
    harness.workspace_update(|workspace, ctx| workspace.set_theme("Nothing At All", ctx));

    assert_eq!(harness.theme_name(), before);
    assert_eq!(crate::theme::theme(), crate::theme::DARK);
}

#[test]
fn the_settings_chord_shows_the_settings_section_and_a_second_press_keeps_it() {
    // A second press is a navigation to the settings, never a toggle away
    // from them: somebody pressing the chord twice means "settings", and a
    // toggle would answer "the thing you were looking at".
    let mut harness = Harness::new(2);
    let tabs = harness.tab_ids();
    assert!(!harness.is_settings_page_open());

    assert!(harness.press_key(",", settings_chord()));
    assert!(harness.is_settings_page_open());

    assert!(harness.press_key(",", settings_chord()));
    assert!(
        harness.is_settings_page_open(),
        "the second press toggled away"
    );

    // And the tabs are untouched: showing the settings opens nothing and
    // closes nothing.
    assert_eq!(harness.tab_ids(), tabs);
    assert!(
        frame_text(&harness.frame()).contains("Appearance"),
        "the settings are not on screen"
    );
}

#[test]
fn opening_the_settings_page_from_the_options_menu_takes_the_menu_down() {
    // The popup is a menu about the strip. It stays up through every option
    // click on purpose, but this entry navigates away from what it is about.
    let mut harness = Harness::seeded();
    harness.dispatch_option(OptionsAction::TogglePopup);
    assert!(harness.is_menu_open());

    harness.open_settings_page();
    assert!(harness.is_settings_page_open());
    assert!(
        !harness.is_menu_open(),
        "the menu survived the page opening"
    );
}

#[test]
fn typing_in_the_rail_filters_the_page_and_the_rail_together() {
    // The whole feature, through the real keyboard: keys the bindings decline
    // reach the box, and one query narrows both halves of the page at once.
    // Filtering the rail is not decoration — a rail still listing four pages
    // beside a page holding three rows would say the opposite of what the box
    // is for.
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    harness.frame();

    harness.type_text("chip");
    assert_eq!(harness.search_text(), "chip");

    let scene = harness.frame();
    let text = frame_text(&scene);
    assert!(
        text.contains("Show the PR link chip") && text.contains("Show the diff stats chip"),
        "the rows that say `chip` are not on the page: {text:?}"
    );
    assert!(
        !text.contains("Tab placement"),
        "a row that says nothing about chips survived the filter"
    );
    assert!(
        text.contains("Appearance (3)"),
        "the rail does not count what it found: {text:?}"
    );
    assert!(
        !text.contains("About"),
        "a page with nothing in it is still listed"
    );
}

#[test]
fn a_row_is_found_by_a_word_that_is_not_written_on_it() {
    // The keywords, which are the difference between a search that works and
    // one that only finds what somebody already knew to call it. Nothing on
    // the "View as" row says "split".
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    harness.frame();
    harness.type_text("split");

    assert!(
        frame_text(&harness.frame()).contains("View as"),
        "the row nobody calls by its name was not found"
    );
}

#[test]
fn a_query_that_empties_the_page_shows_the_first_page_that_has_something() {
    // The rail's selection does not move — clearing the box has to put you
    // back where you were — so which page is *shown* is worked out from the
    // query instead.
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    harness.frame();
    harness.type_text("clipboard");

    let text = frame_text(&harness.frame());
    assert!(
        text.contains("Copy what is selected"),
        "the page with the answers is not the one on screen: {text:?}"
    );
    assert_eq!(
        harness.settings_section(),
        "Appearance",
        "the rail moved its own selection"
    );
}

#[test]
fn a_query_that_finds_nothing_says_so() {
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    harness.frame();
    harness.type_text("zzz");

    let text = frame_text(&harness.frame());
    assert!(
        text.contains("No settings match your search."),
        "an empty page with no explanation on it: {text:?}"
    );
    assert!(
        settings_rail_boxes(&harness.frame()).len() <= 1,
        "a rail with no pages in it should hold nothing but the box"
    );
}

#[test]
fn escape_empties_the_box_and_puts_the_page_back() {
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    harness.frame();
    harness.type_text("chip");
    harness.frame();

    harness.press("escape", Modifiers::default(), "");
    assert_eq!(harness.search_text(), "");
    assert!(
        frame_text(&harness.frame()).contains("View as"),
        "the page did not come back"
    );
}

#[test]
fn the_query_goes_when_the_page_does() {
    // The page's *section* outlives the pane on purpose — closing the tab and
    // opening it again comes back to where you were. A filter must not: a page
    // that came back showing three rows out of thirty would read as broken
    // rather than as filtered, and the box that explains it is in a rail
    // somebody has to look at to find.
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    harness.type_text("chip");

    harness.show_tabs();
    harness.open_settings_page();

    assert_eq!(harness.search_text(), "");
    assert!(frame_text(&harness.frame()).contains("View as"));
}

#[test]
fn the_box_takes_no_keys_while_a_session_is_the_focused_pane() {
    // The one rule that keeps the box from eating a shell's typing: it has the
    // keyboard when the focused pane is the page it is part of, and never
    // otherwise. Going back to the tabs must stop it collecting keystrokes.
    let mut harness = Harness::new(2);
    harness.open_settings_page();
    harness.type_text("ab");
    assert_eq!(harness.search_text(), "ab");

    harness.show_tabs();
    harness.type_text("cd");

    assert_eq!(
        harness.search_text(),
        "",
        "the search box collected what was typed into a session"
    );
}

#[test]
fn the_rail_switches_pages_and_the_pane_shows_the_one_it_names() {
    let mut harness = Harness::new(1);
    harness.open_settings_page();

    let rail = settings_rail_boxes(&harness.frame());
    assert_eq!(rail.len(), 4, "four pages in the rail");

    // The third: Keyboard Shortcuts.
    harness.click(center(rail[2]), MouseButton::Left);
    assert_eq!("Keyboard Shortcuts", harness.settings_section());

    let text = frame_text(&harness.frame());
    assert!(
        text.contains("New agent tab"),
        "the Keyboard Shortcuts page did not come up: {text}"
    );
    assert!(
        !text.contains("Tab placement"),
        "the Appearance page is still on screen"
    );
}

#[test]
fn the_page_and_the_scroll_position_outlive_leaving_the_section() {
    // Coming back to the settings comes back to where you were. The state is
    // the workspace's rather than the section's, which is what makes that true
    // with nothing to keep alive.
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    harness.select_settings_section("About");

    harness.show_tabs();
    assert!(!harness.is_settings_page_open());

    harness.open_settings_page();
    assert_eq!(
        "About",
        harness.settings_section(),
        "reopening the settings went back to the first page"
    );
}

#[test]
fn a_switch_on_the_page_writes_the_option_the_options_menu_writes() {
    let mut harness = Harness::new(1);
    harness.open_settings_page();

    // The page is taller than the pane and the switches are at the bottom of
    // it. Scrolling past the end lands on the last pixel of content, which is
    // what makes this independent of how tall the page happens to be.
    harness.scroll_settings_page(-100.);
    let scene = harness.frame();

    let switches = settings_switch_boxes(&scene);
    // The detail card's is the last switch on the page, whichever others have
    // scrolled into view above it — the page has grown a switch twice now, and
    // a fixed index would have to be corrected each time.
    let detail_card = *switches.last().expect("the page has switches on it");

    assert!(harness.options().show_details_on_hover);
    harness.click(center(detail_card), MouseButton::Left);
    assert!(
        !harness.options().show_details_on_hover,
        "the switch did not write the option"
    );
    assert!(
        harness.is_settings_page_open(),
        "a click on a control closed the page"
    );
}

#[test]
fn a_switch_the_density_has_made_inert_is_drawn_and_does_nothing() {
    // Warp's third way with an irrelevant setting, and the one the page takes:
    // the row stays, greyed, with no handler. The options menu takes the other —
    // it drops the two rows entirely — and both are right for their surface.
    let mut harness = Harness::new(1);
    assert_eq!(Density::Compact, harness.options().density);
    harness.open_settings_page();
    harness.scroll_settings_page(-100.);

    let switches = settings_switch_boxes(&harness.frame());
    let before = harness.options();

    // The two chip switches are the third and second from the end: the detail
    // card's is last, and whatever else the page has grown is above them.
    // Counted from the end rather than the start for the reason the test above
    // is: a fixed index has to be corrected every time a switch is added.
    let inert = [switches.len() - 3, switches.len() - 2];
    for index in inert {
        harness.click(center(switches[index]), MouseButton::Left);
    }

    assert_eq!(
        before,
        harness.options(),
        "a compact row has no chips, so its chip switches must not be clickable"
    );
}

#[test]
fn the_reset_button_puts_every_tab_option_back_and_then_goes_quiet() {
    let mut harness = Harness::new(1);
    harness.dispatch_option(OptionsAction::SetPrimaryInfo(PrimaryInfo::Branch));
    harness.dispatch_option(OptionsAction::ToggleShowDiffStats);
    assert_ne!(TabOptions::default(), harness.options());

    harness.open_settings_page();
    harness.scroll_settings_page(-100.);

    let button = settings_button_box(&harness.frame()).expect("the reset button should be drawn");
    harness.click(center(button), MouseButton::Left);

    assert_eq!(
        TabOptions::default(),
        harness.options(),
        "reset left an option where it was"
    );

    // And now it is the page's modified indicator, saying that nothing has
    // been changed: still drawn, still inert, and without its outline.
    harness.scroll_settings_page(-100.);
    assert!(
        settings_button_box(&harness.frame()).is_none(),
        "the reset button kept its outline with nothing left to reset"
    );
}

#[test]
fn turning_the_login_shell_off_reaches_the_thing_that_opens_shells() {
    // The switch is worth nothing on its own: what matters is that it arrives
    // at the model that starts the shell, before the next one is started. A
    // setting written to a file and read by nobody is the shape of bug this
    // catches.
    // Which way the switch starts is the platform's answer and not one to
    // hard-code: a login shell is what every terminal on macOS gives you and
    // what none of them give you on Linux, and `login_by_default` is where
    // that is argued. What this test is about is the wire between the switch
    // and the model, which is the same either way round.
    let out_of_the_box = crate::shell_integration::login_by_default();
    let mut harness = Harness::new(1);
    assert_eq!(harness.general().login_shell, out_of_the_box);
    assert_eq!(
        harness.shell_login(),
        out_of_the_box,
        "the model was told at startup"
    );

    harness.open_settings_page();
    harness.select_settings_section("Shell");

    let switches = settings_switch_boxes(&harness.frame());
    assert_eq!(switches.len(), 1, "one switch on the shell page");
    harness.click(center(switches[0]), MouseButton::Left);

    assert_eq!(harness.general().login_shell, !out_of_the_box);
    assert_eq!(
        harness.shell_login(),
        !out_of_the_box,
        "the switch wrote the file and left the shells alone"
    );
}

/// The shell in a pane, driven through the real workspace.
///
/// These are the only tests in this file that start a process. They are worth
/// the cost: everything between a shell writing a byte and a tab knowing about
/// it — a reader thread, a wake, a model, a subscription, a session, the git
/// model's directory list — has no meaning without one at the end of it, and a
/// double would only assert that the double was called. The same goes for the
/// field: what it is for is reaching a shell.
mod shells {
    use std::time::Duration;

    use crook_terminal::BlockState;

    use super::*;
    use crate::clipboard::Clipboard;

    /// How long the field is watched for keystrokes leaking into the pty.
    ///
    /// A pty echoes what is written to it as fast as the kernel can hand it
    /// back, so anything that has not arrived in a tenth of a second was never
    /// sent. Doubled, because a loaded build machine is not a fast one.
    const NO_LEAK_PATIENCE: Duration = Duration::from_millis(200);

    /// The pane every test here works in.
    fn one_shell(harness: &mut Harness) -> Option<PaneId> {
        if !harness.start_terminals() {
            return None;
        }
        harness.focused_pane_id()
    }

    /// A pane whose shell reports command boundaries, or `None` on a machine
    /// where none could be started.
    fn marked_shell(harness: &mut Harness) -> Option<PaneId> {
        if !harness.start_terminals_with_marks() {
            return None;
        }
        harness.focused_pane_id()
    }

    /// Waits for the shell to report that it is *at* a prompt.
    ///
    /// Not merely for it to have printed something. A pty echoes what is
    /// written to it whether or not a child has read it yet, and a command
    /// submitted into the block that is still open from before the first
    /// prompt is a command the shell never reports a boundary for.
    fn await_prompt(harness: &mut Harness, pane: PaneId) {
        harness.wait_for("the shell never reached a prompt", |harness| {
            harness
                .workspace
                .read(&harness.app, |workspace, app| {
                    let (_, snapshot) = workspace.terminal(pane, app)?;
                    Some(snapshot.live_block.state == BlockState::AtPrompt)
                })
                .unwrap_or_default()
        });
        harness.frame();
    }

    /// The panel of the one pane, which is the ground it paints that is not a
    /// field.
    fn panel_of_the_pane(harness: &mut Harness) -> RectF {
        let scene = harness.frame();
        let fields = composer_boxes(&scene);
        panel_boxes(&scene)
            .into_iter()
            .find(|panel| !fields.iter().any(|field| field.origin() == panel.origin()))
            .expect("a pane draws a panel")
    }

    /// The point in the window where a cell of the one pane's output is drawn.
    ///
    /// `across` is how far into the cell horizontally, in cells: a tenth is the
    /// left half of it, which is the side a selection starts before, and nine
    /// tenths is the right half, which is the side it ends after.
    ///
    /// `row` is a viewport row either way, because the pane's shell reports no
    /// command marks here and the open block therefore holds the whole
    /// session. Where those rows are drawn is not the same on both surfaces:
    /// the grid puts row zero at the top of the pane, and the list bottom-
    /// aligns the open block against the composer, so the composer is measured
    /// rather than assumed.
    fn grid_cell(harness: &mut Harness, row: usize, column: usize, across: f32) -> Vector2F {
        let panel = panel_of_the_pane(harness);
        let cell = CellFont::headless(CELL_FONT_SIZE).metrics();

        let top = if draws_blocks(harness) {
            let scene = harness.frame();
            let composer = composer_boxes(&scene)
                .first()
                .copied()
                .expect("a pane on the normal screen draws a composer");
            composer.min_y() - live_rows(harness) as f32 * cell.height
        } else {
            panel.min_y() + crate::workspace::body::GRID_VERTICAL_PADDING
        };

        vec2f(panel.min_x() + crate::workspace::body::GUTTER, top)
            + vec2f(
                (column as f32 + across) * cell.width,
                (row as f32 + 0.5) * cell.height,
            )
    }

    /// Whether the one pane is drawing its output as a list of blocks rather
    /// than as one grid.
    fn draws_blocks(harness: &Harness) -> bool {
        let Some(pane) = harness.focused_pane_id() else {
            return false;
        };
        harness
            .workspace
            .read(&harness.app, |workspace, app| {
                let (_, snapshot) = workspace.terminal(pane, app)?;
                Some(
                    crate::pane_surface::of(&snapshot, std::time::Instant::now()).surface
                        == crate::pane_surface::Surface::Blocks,
                )
            })
            .unwrap_or_default()
    }

    /// The middle of the composer's first text row, which is where a click
    /// aimed at the line being composed lands.
    fn composer_text(harness: &mut Harness) -> Vector2F {
        let scene = harness.frame();
        let composer = composer_boxes(&scene)
            .first()
            .copied()
            .expect("a pane on the normal screen draws a composer");
        let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
        vec2f(
            composer.min_x() + crate::workspace::body::GUTTER + cell.width * 0.5,
            composer.max_y() - crate::workspace::body::COMPOSER_PADDING_BOTTOM - cell.height * 0.5,
        )
    }

    /// How many rows the one pane's open block holds.
    fn live_rows(harness: &Harness) -> usize {
        let Some(pane) = harness.focused_pane_id() else {
            return 0;
        };
        harness
            .workspace
            .read(&harness.app, |workspace, app| {
                let (_, snapshot) = workspace.terminal(pane, app)?;
                let live = &snapshot.live_block;
                (live.bottom_row >= live.top_row)
                    .then(|| (live.bottom_row - live.top_row.max(0)) as usize + 1)
            })
            .unwrap_or_default()
    }

    /// Drags a selection from the left of one cell to the right of another.
    fn drag_across(harness: &mut Harness, from: (usize, usize), to: (usize, usize)) {
        let start = grid_cell(harness, from.0, from.1, 0.1);
        let end = grid_cell(harness, to.0, to.1, 0.9);
        harness.hold(start, 1);
        harness.drag_to(end);
        harness.let_go(end);
    }

    /// Runs a command in the pane and waits for what it prints, then reports
    /// the row the marker landed on.
    ///
    /// The marker must be something the *command* cannot contain, because a
    /// pty echoes what is typed into it: the tests here print in upper case
    /// and fold it down, so the line the shell wrote is the only one on screen
    /// that matches.
    fn run_and_find(harness: &mut Harness, command: &str, marker: &str) -> usize {
        harness.type_into(harness_pane(harness), &format!("{command}\n"));
        let pane = harness_pane(harness);
        harness.wait_for("the shell never printed the marker", |harness| {
            harness.terminal_text(pane).contains(marker)
        });
        harness.frame();
        harness
            .terminal_text(pane)
            .lines()
            .position(|line| line.contains(marker))
            .expect("the marker is on screen")
    }

    /// The pane every test here works in, once it is open.
    fn harness_pane(harness: &Harness) -> PaneId {
        harness.focused_pane_id().expect("the window has a pane")
    }

    #[test]
    fn a_drag_across_the_output_selects_the_cells_it_covered_and_copy_takes_them() {
        // **The whole feature, end to end.** A real shell prints a line, a
        // pointer drags across part of it, and what a copy would take is
        // exactly the characters those cells were drawn from — no padding to
        // the end of the row, no line the drag never reached.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        let row = run_and_find(
            &mut harness,
            "echo 'ALPHA BETA GAMMA' | tr A-Z a-z",
            "alpha beta",
        );
        let column = harness
            .terminal_text(pane)
            .lines()
            .nth(row)
            .and_then(|line| line.find("beta"))
            .expect("the marker is on that row");

        drag_across(&mut harness, (row, column), (row, column + 3));
        assert_eq!(harness.terminal_selection(pane).as_deref(), Some("beta"));

        // And the copy chord takes it and lets go of it, which is the only
        // sign a copy happened at all.
        harness.press("c", platform_chord(), "c");
        assert_eq!(
            harness.terminal_selection(pane),
            None,
            "copying left the highlight on screen"
        );
    }

    #[test]
    fn a_selection_across_a_wrapped_line_copies_back_as_the_one_line_it_is() {
        // A line too long for the pane is one line of text on two rows, and
        // the fold is somewhere the terminal put it rather than something the
        // shell printed. Copying it with a newline in it would break the
        // command it came from.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        // Five hundred characters the command that prints them does not
        // contain, so the row they land on is unambiguous however wide the
        // pane turns out to be.
        let long: String = (1..=200).map(|number| number.to_string()).collect();
        let row = run_and_find(&mut harness, "printf '%s' $(seq 1 200); echo", "123456789");
        let columns = harness.terminal_columns(pane);
        assert!(
            long.len() > columns,
            "the line has to be long enough to wrap"
        );

        // From the start of the wrapped line to a cell on the row below it,
        // which is the fold the copy must not put a newline at.
        drag_across(&mut harness, (row, 0), (row + 1, 4));
        let copied = harness
            .terminal_selection(pane)
            .expect("something is selected");
        assert_eq!(
            copied,
            long[..columns + 5],
            "the fold the terminal put in a long line came back as a break in the text"
        );
    }

    #[test]
    fn a_double_click_takes_a_word_and_a_triple_click_takes_the_line() {
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        let row = run_and_find(
            &mut harness,
            "echo 'ONE TWO THREE' | tr A-Z a-z",
            "one two three",
        );
        let column = harness
            .terminal_text(pane)
            .lines()
            .nth(row)
            .and_then(|line| line.find("two"))
            .expect("the marker is on that row");

        let middle = grid_cell(&mut harness, row, column + 1, 0.5);
        harness.hold(middle, 2);
        harness.let_go(middle);
        assert_eq!(harness.terminal_selection(pane).as_deref(), Some("two"));

        harness.hold(middle, 3);
        harness.let_go(middle);
        // The line, and not a line break after it. A selection is the cells it
        // covers, and a newline is not a cell: one only ever appears *between*
        // two rows. That is also what makes "select this block and copy" and
        // the block's own copy control hand back the same string.
        assert_eq!(
            harness.terminal_selection(pane).as_deref(),
            Some("one two three"),
            "a triple click takes the whole line the pointer is on"
        );
    }

    #[test]
    fn a_plain_click_on_the_output_lets_go_of_what_was_selected() {
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        let row = run_and_find(&mut harness, "echo 'LETTING GO' | tr A-Z a-z", "letting go");
        drag_across(&mut harness, (row, 0), (row, 6));
        assert!(harness.terminal_selection(pane).is_some());

        let elsewhere = grid_cell(&mut harness, row, 0, 0.1);
        harness.hold(elsewhere, 1);
        harness.let_go(elsewhere);
        assert_eq!(
            harness.terminal_selection(pane),
            None,
            "a click with no drag behind it left a one-cell highlight"
        );
    }

    #[test]
    fn typing_and_clicking_into_the_field_let_go_of_the_output_selection() {
        // Two of the four clearing rules, and the reason for both: a highlight
        // nobody is aiming at any more is the next copy taking the wrong
        // thing.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        let row = run_and_find(&mut harness, "echo 'CLEAR ME' | tr A-Z a-z", "clear me");

        drag_across(&mut harness, (row, 0), (row, 4));
        assert!(harness.terminal_selection(pane).is_some());
        type_line(&mut harness, "e");
        assert_eq!(
            harness.terminal_selection(pane),
            None,
            "typing into the field left the output highlighted"
        );
        assert_eq!(harness.field_text(pane), "e", "and the key still landed");

        drag_across(&mut harness, (row, 0), (row, 4));
        assert!(harness.terminal_selection(pane).is_some());
        // The composer's first text row. Its box carries twenty pixels of
        // bottom padding — the pane's own bottom inset — so the middle of the
        // box is under the line rather than on it.
        let at = composer_text(&mut harness);
        harness.click(at, MouseButton::Left);
        assert_eq!(
            harness.terminal_selection(pane),
            None,
            "clicking into the field left the output highlighted"
        );
    }

    #[test]
    fn the_shell_printing_does_not_let_go_of_a_selection() {
        // The one thing that must *not* clear it. Output arriving is exactly
        // when somebody is reading what is already on screen, and a highlight
        // that vanished every time a build printed a line would be unusable.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        let row = run_and_find(&mut harness, "echo 'KEEP ME' | tr A-Z a-z", "keep me");

        drag_across(&mut harness, (row, 0), (row, 6));
        assert_eq!(harness.terminal_selection(pane).as_deref(), Some("keep me"));

        harness.type_into(pane, "echo 'AND MORE' | tr A-Z a-z\n");
        harness.wait_for("the shell never printed again", |harness| {
            harness.terminal_text(pane).contains("and more")
        });
        assert_eq!(
            harness.terminal_selection(pane).as_deref(),
            Some("keep me"),
            "output scrolling underneath a selection took it away"
        );
    }

    #[test]
    fn each_pane_keeps_its_own_selection_and_a_closed_one_takes_its_with_it() {
        // The split comes first on purpose: a split *resizes* the pane it
        // divides, and reflowing a screen into a different number of columns
        // is the one thing that genuinely invalidates a selection — the cells
        // it named hold other text afterwards. Moving the focus does not.
        let mut harness = Harness::panel(1);
        let Some(first) = one_shell(&mut harness) else {
            return;
        };
        harness.dispatch_action(TabAction::Split(Direction::Right));
        harness.frame();
        let second = harness.focused_pane_id().expect("the split focused a pane");
        assert_ne!(first, second);

        harness.dispatch_action(TabAction::FocusPane(first));
        harness.frame();
        let row = run_and_find(&mut harness, "echo 'FIRST PANE' | tr A-Z a-z", "first pane");
        drag_across(&mut harness, (row, 0), (row, 4));
        assert_eq!(harness.terminal_selection(first).as_deref(), Some("first"));

        harness.dispatch_action(TabAction::FocusPane(second));
        harness.frame();
        assert_eq!(
            harness.terminal_selection(first).as_deref(),
            Some("first"),
            "focusing another pane threw away this one's selection"
        );
        assert_eq!(
            harness.terminal_selection(second),
            None,
            "the other pane inherited a selection nobody made in it"
        );

        harness.dispatch_action(TabAction::ClosePane(first));
        harness.frame();
        assert_eq!(
            harness.terminal_selection(first),
            None,
            "a closed pane left a selection behind it"
        );
    }

    #[test]
    fn ctrl_c_copies_a_selection_and_the_next_one_interrupts() {
        // **The collision that makes or breaks the terminal.** With something
        // selected `ctrl-c` is the copy a person just dragged out; with
        // nothing selected it is the interrupt it has always been — and
        // because copying lets go, the second press is always the interrupt.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        harness.type_into(pane, "printf 'g%s\n' o; sleep 30\n");
        harness.wait_for("the shell never started the command", |harness| {
            harness.terminal_text(pane).contains("go")
        });
        harness.frame();
        let row = harness
            .terminal_text(pane)
            .lines()
            .position(|line| line.trim() == "go")
            .expect("the marker is on screen");

        drag_across(&mut harness, (row, 0), (row, 1));
        assert_eq!(harness.terminal_selection(pane).as_deref(), Some("go"));

        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        harness.press("c", ctrl, "c");
        assert_eq!(
            harness.terminal_selection(pane),
            None,
            "the copy did not let go of the selection"
        );
        harness.settle(NO_LEAK_PATIENCE);
        assert!(
            !harness.terminal_text(pane).contains("^C"),
            "a copy interrupted the command it was copying from"
        );

        // And now the shell is still waiting on `sleep 30`, so only an
        // interrupt can let the next command run.
        harness.press("c", ctrl, "c");
        harness.type_into(pane, "printf 'ba%s\n' ck\n");
        harness.wait_for("the second ctrl-c never reached the shell", |harness| {
            harness.terminal_text(pane).contains("back")
        });
    }

    #[test]
    fn a_drag_past_the_bottom_edge_scrolls_the_output_under_the_pointer() {
        // What makes a selection able to run past the screen it began on.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        harness.type_into(pane, "for i in $(seq 1 200); do echo line-$i; done\n");
        harness.wait_for("the shell never filled the scrollback", |harness| {
            harness.terminal_text(pane).contains("line-200")
        });
        harness.frame();

        // Back into the history, and then a drag that leaves the bottom of the
        // pane: the viewport has to follow the pointer down.
        let over_the_grid = grid_cell(&mut harness, 1, 1, 0.5);
        harness.dispatch(Event::ScrollWheel {
            position: over_the_grid,
            delta: ScrollDelta::Lines(vec2f(0., 20.)),
            modifiers: Modifiers::default(),
        });
        let scrolled_back = harness.terminal_text(pane);
        assert!(
            !scrolled_back.contains("line-200"),
            "the wheel did not reach the scrollback"
        );

        let panel = panel_of_the_pane(&mut harness);
        let start = grid_cell(&mut harness, 0, 0, 0.1);
        let below = vec2f(panel.max_x() - 1., panel.max_y() + 200.);
        harness.hold(start, 1);
        harness.drag_to(below);
        harness.let_go(below);

        assert_ne!(
            harness.terminal_text(pane),
            scrolled_back,
            "a drag past the bottom of the pane scrolled nothing"
        );
        let selected = harness
            .terminal_selection(pane)
            .expect("the drag selected something");
        assert!(
            selected.lines().count() > 1,
            "a drag that scrolled the screen selected one line: {selected:?}"
        );
    }

    #[test]
    fn the_output_can_be_selected_by_name_the_way_a_headless_run_does() {
        // `--select-output`, which is the only way a picture can show a
        // selection: nobody is holding a button down while a PNG is rendered.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        run_and_find(&mut harness, "printf 'FIND\\nME\\n' | tr A-Z a-z", "find");

        assert!(harness.select_in_output(pane, "find\nme"));
        assert_eq!(
            harness.terminal_selection(pane).as_deref(),
            Some("find\nme"),
            "the cells named by their text are not the cells that were selected"
        );
        assert!(
            !harness.select_in_output(pane, "not on this screen"),
            "text the screen does not show cannot be selected"
        );
    }

    #[test]
    fn a_release_over_the_tabs_panel_still_ends_the_gesture() {
        // The button can come up anywhere. Over the tabs panel it comes up in a
        // layer painted *after* the grid, and an occlusion test on the release
        // would drop it — leaving the pane with a press it thinks is still
        // down, so that every later drag anywhere in the window, from any other
        // press or from none at all, would rewrite this pane's selection.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        let row = run_and_find(
            &mut harness,
            "echo 'ALPHA BETA GAMMA' | tr A-Z a-z",
            "alpha beta",
        );
        let column = harness
            .terminal_text(pane)
            .lines()
            .nth(row)
            .and_then(|line| line.find("alpha"))
            .expect("the marker is on that row");

        let over_the_panel = center(panel_rows(&harness.frame())[0]);
        let start = grid_cell(&mut harness, row, column, 0.1);
        let end = grid_cell(&mut harness, row, column + 4, 0.9);
        harness.hold(start, 1);
        harness.drag_to(end);
        harness.let_go(over_the_panel);
        assert_eq!(harness.terminal_selection(pane).as_deref(), Some("alpha"));

        // A drag with no press of its own behind it. The gesture is over, so
        // this pane has nothing to do with it.
        let elsewhere = grid_cell(&mut harness, row, column + 12, 0.9);
        harness.drag_to(elsewhere);
        assert_eq!(
            harness.terminal_selection(pane).as_deref(),
            Some("alpha"),
            "a drag this pane was never pressed for rewrote its selection"
        );
    }

    #[test]
    fn a_drag_that_leaves_the_pane_keeps_taking_the_selection_with_it() {
        // The other half of the same rule, and the documented behaviour it
        // protects: dragging past the edge of the pane is how a selection is
        // taken to the end of a line. The tabs panel is beside the pane and
        // paints over it in the layer order, so a drag that crosses onto it
        // must still reach the grid that opened the gesture.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        let row = run_and_find(
            &mut harness,
            "echo 'ALPHA BETA GAMMA' | tr A-Z a-z",
            "alpha beta",
        );
        let column = harness
            .terminal_text(pane)
            .lines()
            .nth(row)
            .and_then(|line| line.find("beta"))
            .expect("the marker is on that row");

        // The panel's own rows, which is the part of it that records a hit and
        // so the part that occludes what is painted before it.
        let over_the_panel = center(panel_rows(&harness.frame())[0]);
        let start = grid_cell(&mut harness, row, column, 0.1);
        harness.hold(start, 1);
        harness.drag_to(over_the_panel);
        harness.let_go(over_the_panel);

        let selected = harness
            .terminal_selection(pane)
            .expect("a drag that crossed onto the panel selected nothing at all");
        assert!(
            selected.contains("alpha"),
            "a drag off the pane stopped where the panel starts: {selected:?}"
        );
    }

    #[test]
    fn the_copy_chord_leaves_the_half_written_command_line_alone() {
        // **The two elements, one keystroke.** The grid and the field are
        // siblings, and a `Flex` hands the same `ctrl-c` to both — the grid
        // first. Both route it by asking whether anything is selected in the
        // output, and a grid that let go of the selection as it copied would
        // change the answer under the field: the field would read the chord as
        // the interrupt it is with nothing selected, and throw away the command
        // the person was in the middle of writing.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        let row = run_and_find(&mut harness, "echo 'COPY ME' | tr A-Z a-z", "copy me");

        type_line(&mut harness, "echo");
        assert_eq!(harness.field_text(pane), "echo");

        drag_across(&mut harness, (row, 0), (row, 3));
        assert_eq!(harness.terminal_selection(pane).as_deref(), Some("copy"));

        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        harness.press("c", ctrl, "c");
        assert_eq!(
            harness.field_text(pane),
            "echo",
            "a copy off the output abandoned the line being written under it"
        );
        assert_eq!(
            harness.terminal_selection(pane),
            None,
            "and it still let go of what it copied"
        );

        // The line survived and is still the line: sending it runs it.
        harness.press("enter", Modifiers::default(), "\r");
        harness.wait_for("the line the copy spared never ran", |harness| {
            harness.terminal_text(pane).contains("echo\r\n")
                || harness
                    .terminal_text(pane)
                    .lines()
                    .filter(|line| line.contains("echo"))
                    .count()
                    > 1
        });
    }

    #[test]
    fn with_a_selection_in_both_halves_of_the_pane_the_output_is_what_is_copied() {
        // Rule 1 of the routing policy, on the one machine that can prove it:
        // the output's selection outranks the field's, so the clipboard holds
        // what was dragged out of the shell rather than what the field had.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        let Some((clipboard, _system)) = working_clipboard(&harness) else {
            return;
        };
        let row = run_and_find(&mut harness, "echo 'COPY ME' | tr A-Z a-z", "copy me");

        type_line(&mut harness, "abcd");
        harness.press("a", platform_chord(), "a");
        assert_eq!(harness.field_selection(pane), "abcd");

        drag_across(&mut harness, (row, 0), (row, 3));
        assert_eq!(harness.terminal_selection(pane).as_deref(), Some("copy"));

        harness.press("c", platform_chord(), "c");
        assert_eq!(
            clipboard.read().as_deref(),
            Some("copy"),
            "the field overwrote the clipboard the output had just been copied to"
        );
    }

    /// The system clipboard, when this machine has one that works.
    ///
    /// A headless runner and an X session with nothing serving the selection
    /// both have none, and there is nothing to assert about a copy on a machine
    /// where a copy cannot happen — [`crate::clipboard`] says so at length.
    fn working_clipboard(harness: &Harness) -> Option<(Clipboard, MutexGuard<'static, ()>)> {
        // **One at a time.** These are the tests that put text on the
        // *machine's* clipboard, and on macOS that is one `NSPasteboard`
        // shared by every thread in the process: two of them reading and
        // writing it at once crashes inside the Objective-C runtime, which
        // arrives as a segmentation fault in whichever test happened to be
        // holding it. The lock orders them and costs a few milliseconds.
        static SYSTEM: Mutex<()> = Mutex::new(());
        // The lock guards an ordering rather than an invariant, so a test that
        // panicked while holding it left nothing for the next one to be
        // careful of.
        let held = SYSTEM.lock().unwrap_or_else(PoisonError::into_inner);

        let clipboard = harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.clipboard().clone());
        clipboard.write("crook: nothing was copied over this");
        (clipboard.read().as_deref() == Some("crook: nothing was copied over this"))
            .then_some((clipboard, held))
    }

    /// Types `text` a key at a time, the way a person does.
    fn type_line(harness: &mut Harness, text: &str) {
        for character in text.chars() {
            harness.press(
                &character.to_lowercase().to_string(),
                Modifiers::default(),
                &character.to_string(),
            );
        }
    }

    #[test]
    fn tab_completes_a_path_the_shell_can_see_and_crook_cannot() {
        // **The whole feature, end to end, and the point of doing it this
        // way.** The completion is the shell's: it is computed by the shell in
        // the pane, against the directory that shell is in, by the shell's own
        // machinery. Crook writes the question, sends a key, and splices the
        // answer back into a line the shell has never seen.
        let directory = Scratch::new();
        fs::write(directory.path().join("distinctive-name.txt"), "").expect("writable");

        let mut harness = Harness::panel(1);
        let Some(pane) = marked_shell(&mut harness) else {
            return;
        };
        harness.frame();
        await_prompt(&mut harness, pane);

        // Into the directory the file is in, so the answer can only have come
        // from the shell: nothing in Crook knows where that shell is.
        type_line(&mut harness, &format!("cd {}", directory.path().display()));
        harness.press("enter", Modifiers::default(), "");
        await_prompt(&mut harness, pane);

        type_line(&mut harness, "cat disti");
        harness.press("tab", Modifiers::default(), "\t");

        harness.wait_for("the shell never completed the path", |harness| {
            harness.field_text(pane).contains("distinctive-name.txt")
        });
    }

    #[test]
    fn a_pane_whose_pty_is_held_open_by_a_background_process_still_closes() {
        // End-of-file on the pty master is what the reader learns a session is
        // over by, and something other than the shell can hold the far end
        // open. Before the child was asked directly, this pane sat there
        // showing a dead shell until somebody closed it by hand.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        harness.wait_for("the shell never printed anything", |harness| {
            !harness.terminal_text(pane).trim().is_empty()
        });

        // A process that outlives the shell and keeps the slave descriptor.
        type_line(&mut harness, "sleep 30 &");
        harness.press("enter", Modifiers::default(), "");
        harness.wait_for("the background job never started", |harness| {
            harness.terminal_text(pane).contains(char::is_numeric)
        });

        type_line(&mut harness, "exit");
        harness.press("enter", Modifiers::default(), "");

        // The shell is gone; the pty is not. The window closes because the
        // *child* was asked, not because anything reached end of file.
        harness.wait_for("the pane never noticed its shell had exited", |harness| {
            harness.quit_requests() > 0
        });
    }

    #[test]
    fn a_command_typed_into_the_field_reaches_the_shell_when_it_is_sent() {
        // **The whole feature, end to end.** Keys the bindings declined go into
        // the field and nowhere else; the pty hears nothing at all until Enter;
        // and then the shell echoes the line and runs it, so the grid above
        // shows prompt, command and output exactly as it always did.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        type_line(&mut harness, "echo typed-it");
        assert_eq!(harness.field_text(pane), "echo typed-it");

        harness.settle(NO_LEAK_PATIENCE);
        assert!(
            !harness.terminal_text(pane).contains("typed-it"),
            "the keystrokes reached the pty as well as the field"
        );

        harness.press("enter", Modifiers::default(), "\r");
        assert_eq!(
            harness.field_text(pane),
            "",
            "a sent line leaves the field, for the next one"
        );
        harness.wait_for("the shell never ran what the field sent it", |harness| {
            harness.terminal_text(pane).contains("typed-it")
        });
    }

    #[test]
    fn a_sent_line_can_be_recalled_from_the_history_and_sent_again() {
        // Per pane, and it is the field's own history rather than the shell's:
        // the shell never saw the line until it was sent, so it has nothing to
        // recall it with.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        type_line(&mut harness, "echo recalled");
        harness.press("enter", Modifiers::default(), "\r");
        harness.press("up", Modifiers::default(), "");

        assert_eq!(harness.field_text(pane), "echo recalled");
        assert_eq!(harness.field_caret(pane), "echo recalled".len());
    }

    #[test]
    fn the_signal_keys_reach_the_shell_while_the_field_has_the_rest() {
        // Rule three of the routing policy, against a real pty: a command that
        // is already running has to stay interruptible, whatever is half
        // written in the field.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        // The marker is printed by the command rather than named in it, so
        // finding it in the grid is evidence the shell *ran* the line rather
        // than evidence the tty echoed it.
        harness.type_into(pane, "printf 'g%s\\n' o; sleep 30\n");
        harness.wait_for("the shell never started the command", |harness| {
            harness.terminal_text(pane).contains("go")
        });

        let ctrl = Modifiers {
            ctrl: true,
            ..Default::default()
        };
        harness.press("c", ctrl, "c");

        // A shell still waiting on `sleep 30` cannot run this, so the marker
        // arriving at all is the interrupt having landed.
        harness.type_into(pane, "printf 'ba%s\\n' ck\n");
        harness.wait_for("ctrl-c never reached the shell", |harness| {
            harness.terminal_text(pane).contains("back")
        });
    }

    #[test]
    fn a_program_that_takes_the_screen_takes_the_keys_and_the_field_goes_away() {
        // Rule two: vim, `top` and `less` drive every cell and read every key,
        // so there is no line to compose and nowhere to compose it.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        assert_eq!(
            composer_boxes(&harness.frame()).len(),
            1,
            "a pane on the normal screen draws a field"
        );

        // The escape every full-screen program starts with.
        harness.type_into(pane, "printf '\\033[?1049h'\n");
        harness.wait_for("the shell never took the alt screen", |harness| {
            harness.alt_screen(pane)
        });

        assert!(
            composer_boxes(&harness.frame()).is_empty(),
            "the field outlived the screen it belongs to"
        );
        type_line(&mut harness, "q");
        assert_eq!(
            harness.field_text(pane),
            "",
            "a key was typed into a field nobody can see"
        );
    }

    #[test]
    fn clicking_in_the_field_puts_the_caret_where_it_was_aimed() {
        // The mouse path through the real tree: a press at a point, hit-tested
        // against what was painted, landing on the boundary nearest it.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.type_field(pane, "echo hello");

        let scene = harness.frame();
        let field = composer_boxes(&scene)[0];
        let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
        // Column zero, which is the output's column zero: the gutter is the
        // composer's own padding and there is no prompt glyph past it.
        let text_left = field.min_x() + crate::workspace::body::GUTTER;
        let middle = field.max_y() - crate::workspace::body::COMPOSER_PADDING_BOTTOM
            + cell.height * 0.5
            - cell.height;

        harness.click(
            vec2f(text_left + cell.width * 5., middle),
            MouseButton::Left,
        );
        assert_eq!(harness.field_caret(pane), 5, "the boundary before `hello`");

        harness.click(
            vec2f(text_left + cell.width * 5.6, middle),
            MouseButton::Left,
        );
        assert_eq!(
            harness.field_caret(pane),
            6,
            "past the middle of a cell is the boundary after it"
        );

        // And a double click takes the word under it.
        harness.dispatch(Event::MouseDown {
            button: MouseButton::Left,
            position: vec2f(text_left + cell.width * 6., middle),
            modifiers: Modifiers::default(),
            click_count: 2,
        });
        assert_eq!(harness.field_selection(pane), "hello");
    }

    #[test]
    fn a_binding_opens_a_tab_instead_of_typing_a_letter_into_the_field() {
        // The line this whole feature turns on. `cmd-t` — `ctrl-t` off macOS —
        // is Crook's, and a field that swallowed it would leave the window with
        // no way to open a tab; a field that let it through *as well* would put
        // a stray `t` in somebody's command line.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        assert_eq!(harness.tab_ids().len(), 1);
        harness.press("q", Modifiers::default(), "q");
        harness.press("t", platform_chord(), "t");

        assert_eq!(harness.tab_ids().len(), 2, "the binding did not open a tab");
        assert_eq!(
            harness.field_text(pane),
            "q",
            "the binding was typed into the field as well as opening a tab"
        );
    }

    #[test]
    fn a_split_gives_the_new_pane_a_field_of_its_own() {
        // Two panes of one tab are two places to work: what is half written in
        // one of them has nothing to do with the other, and neither has the
        // other's history.
        let mut harness = Harness::panel(1);
        let Some(first) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        type_line(&mut harness, "first");

        harness.dispatch_action(TabAction::Split(Direction::Right));
        harness.frame();
        let second = harness.focused_pane_id().expect("the split focused a pane");
        assert_ne!(first, second);

        type_line(&mut harness, "second");
        assert_eq!(harness.field_text(second), "second");
        assert_eq!(
            harness.field_text(first),
            "first",
            "the split pane took the line the other one was composing"
        );
        assert_eq!(
            composer_boxes(&harness.frame()).len(),
            2,
            "each panel draws its own field"
        );
    }

    #[test]
    fn a_shell_that_renames_itself_renames_its_row_and_moves_where_git_is_read() {
        // OSC 0 and OSC 7, which is what makes the strip's "Command /
        // Conversation" and "Working Directory" say something true rather than
        // repeating the name the pane was born with.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        assert_eq!(harness.pane_title(pane), "agent 1");

        // Wait for the shell to *be there* before typing at it. Starting up and
        // doing as it was told are then two waits of their own rather than one
        // that has to cover both — which is the difference between a test that
        // is reliable on a loaded machine and one that is not.
        harness.frame();
        harness.wait_for("the shell never printed anything", |harness| {
            !harness.terminal_text(pane).trim().is_empty()
        });

        // **The command does not return to a prompt**, and that is the whole
        // of why `read` is on the end of it. A great many people's shells set
        // the terminal's title from a `precmd`: the printf sets it, the prompt
        // that follows sets it straight back, and what this test then sees
        // depends on whether the machine was fast enough to look in between.
        // It passed on an idle laptop and failed under a full suite, which is
        // the worst way for a test to be wrong. A shell waiting on a line it
        // will never get draws no second prompt, so the title it was told to
        // set is the title it has.
        harness.type_into(
            pane,
            "printf '\\033]0;deploy the release\\007\\033]7;file:///tmp\\007'; read\n",
        );
        harness.wait_for("the shell's title never reached the strip", |harness| {
            harness.pane_title(pane) == "deploy the release"
        });
        assert_eq!(harness.working_directory(pane), Some(PathBuf::from("/tmp")));

        // And the branch chip follows it: the git facts a row prints are looked
        // up by the session's directory, so a `cd` the shell reported has to
        // move the lookup with it.
        harness.record_git(pane, "shell-moved-here", None);
        assert_eq!(
            harness.branch_shown(pane),
            Some(Head::Branch("shell-moved-here".to_owned())),
            "the row is still reading the repository the pane started in"
        );
    }

    #[test]
    fn a_program_in_a_marked_shell_reports_its_status_to_the_strip() {
        // The whole chain, with a real shell: the sequence `crook --agent`
        // writes, through the pty, the emulator, the model's subscription and
        // into the session the row is drawn from — and then taken back by the
        // shell's own `D` when the command ends.
        let mut harness = Harness::panel(1);
        let Some(pane) = marked_shell(&mut harness) else {
            return;
        };
        await_prompt(&mut harness, pane);

        let status = |harness: &Harness| {
            harness.workspace.read(&harness.app, |workspace, _| {
                workspace
                    .tabs()
                    .pane(pane)
                    .map(|pane| pane.session().status)
            })
        };
        assert_eq!(status(&harness), Some(AgentStatus::Idle));

        harness.type_into(
            pane,
            "printf '\\033]6340;needs-input;port the tab bar\\007'; read\n",
        );
        harness.wait_for("the status never reached the strip", |harness| {
            status(harness) == Some(AgentStatus::NeedsInput)
        });
        assert_eq!(harness.pane_title(pane), "port the tab bar");

        // Ending the command — `read` gets its line — is the shell's `D`,
        // which takes a waiting status back to idle.
        harness.type_into(pane, "\n");
        harness.wait_for("the command ending never took the status back", |harness| {
            status(harness) == Some(AgentStatus::Idle)
        });
    }

    #[test]
    fn a_shell_that_exits_closes_its_tab_and_takes_its_terminal_with_it() {
        // The ordinary case, and the one that closes the loop: the pane goes
        // through `TabAction::ClosePane`, the strip's change comes back round
        // through `sync_terminals`, and the session is ended rather than left
        // holding a pty nothing reads.
        let mut harness = Harness::panel(2);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        assert_eq!(harness.tab_ids().len(), 2);

        harness.type_into(pane, "exit\n");
        harness.wait_for("the shell exited and its tab stayed open", |harness| {
            harness.tab_ids().len() == 1
        });

        assert_eq!(harness.quit_requests(), 0, "one tab was left to show");
        assert_eq!(
            harness.terminal_text(pane),
            String::new(),
            "the closed pane's terminal outlived the pane"
        );
    }

    #[test]
    fn a_shell_that_exits_closes_its_pane_and_the_window_with_the_last_one() {
        // Requirement seven, and it goes through `TabAction::ClosePane` — the
        // same path `cmd-w` takes — rather than a second way to close things.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        assert_eq!(harness.quit_requests(), 0);

        harness.type_into(pane, "exit\n");
        harness.wait_for("the shell exited and its window stayed open", |harness| {
            harness.quit_requests() > 0
        });
    }

    /// The modifier that interrupts, ends and suspends.
    fn ctrl() -> Modifiers {
        Modifiers {
            ctrl: true,
            ..Default::default()
        }
    }

    #[test]
    fn ctrl_c_interrupts_the_shell_and_takes_the_half_written_line_with_it() {
        // The gesture whose entire meaning is "forget this". The shell prints
        // `^C` and a fresh prompt, and a field still holding `rm -rf` above
        // that prompt would be the interrupt only pretending to have worked.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        harness.type_into(pane, "printf 'g%s\\n' o; sleep 30\n");
        harness.wait_for("the shell never started the command", |harness| {
            harness.terminal_text(pane).contains("go")
        });

        type_line(&mut harness, "rm -rf important");
        assert_eq!(harness.field_text(pane), "rm -rf important");

        harness.press("c", ctrl(), "c");
        assert_eq!(
            harness.field_text(pane),
            "",
            "the abandoned line outlived the interrupt that abandoned it"
        );

        // And the interrupt still landed: a shell waiting on `sleep 30` cannot
        // run this.
        harness.type_into(pane, "printf 'ba%s\\n' ck\n");
        harness.wait_for("ctrl-c never reached the shell", |harness| {
            harness.terminal_text(pane).contains("back")
        });
    }

    #[test]
    fn ctrl_d_over_a_written_line_deletes_a_character_rather_than_closing_the_pane() {
        // The shell's own reader is empty now — the line lives in the field —
        // so an unconditional Ctrl-D would be an end of file every time, and
        // the pane would close under the fingers of anybody who pressed it out
        // of habit.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        type_line(&mut harness, "echo hi");
        harness.press("home", Modifiers::default(), "");
        harness.press("d", ctrl(), "d");

        assert_eq!(harness.field_text(pane), "cho hi");
        harness.settle(NO_LEAK_PATIENCE);
        assert_eq!(
            harness.quit_requests(),
            0,
            "the shell read an end of file and the window went with it"
        );
    }

    #[test]
    fn ctrl_d_on_an_empty_line_still_ends_the_shell() {
        // The other half of the same rule, and the reason the first half
        // cannot simply swallow the key: an empty line is where Ctrl-D means
        // what it has always meant.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        assert_eq!(harness.field_text(pane), "");

        // Wait for the shell to reach its own prompt before sending the end of
        // input. A `^D` written into a pty the shell has not started reading
        // yet is read during its startup, where it is not an end of file at
        // all — which is the difference between this test passing and hanging
        // for its whole budget.
        harness.wait_for("the shell never printed anything", |harness| {
            !harness.terminal_text(pane).trim().is_empty()
        });

        harness.press("d", ctrl(), "d");
        harness.wait_for("the shell never read an end of file", |harness| {
            harness.quit_requests() > 0
        });
    }

    #[test]
    fn a_pane_that_closed_hands_the_keyboard_to_the_one_that_is_left() {
        // A keystroke can arrive between a pane closing and the next frame,
        // and the tree it arrives in is the one built *before* the close: it
        // still holds the closed pane's field, still marked as the listening
        // one. Those keys used to land in an editor nothing could draw or read
        // back — and Enter would have written the line to a shell that had
        // already ended.
        let mut harness = Harness::panel(1);
        let Some(first) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        harness.dispatch_action(TabAction::Split(Direction::Right));
        harness.frame();
        let second = harness.focused_pane_id().expect("the split focused a pane");
        assert_ne!(first, second);

        // No frame between the close and the keys, which is the whole case.
        harness.dispatch_action(TabAction::ClosePane(second));
        type_line(&mut harness, "kept");

        assert_eq!(
            harness.field_text(first),
            "kept",
            "the keys went into the pane that had just closed"
        );
    }

    #[test]
    fn the_options_menu_cannot_stop_a_running_command_from_being_interrupted() {
        // The menu is modal and takes the typing away, which is right. What it
        // must not take away is the one key a terminal cannot live without:
        // the menu does not close on Escape, so an uninterruptible command
        // would leave somebody hunting for the mouse.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();

        harness.type_into(pane, "printf 'g%s\\n' o; sleep 30\n");
        harness.wait_for("the shell never started the command", |harness| {
            harness.terminal_text(pane).contains("go")
        });

        harness.dispatch_option(OptionsAction::TogglePopup);
        harness.frame();
        harness.press("q", Modifiers::default(), "q");
        assert_eq!(
            harness.field_text(pane),
            "",
            "the modal menu was typed through"
        );

        harness.press("c", ctrl(), "c");
        harness.type_into(pane, "printf 'ba%s\\n' ck\n");
        harness.wait_for("ctrl-c never reached the shell", |harness| {
            harness.terminal_text(pane).contains("back")
        });
    }

    #[test]
    fn the_selection_chord_selects_in_the_field_instead_of_switching_tabs() {
        // Both platforms' most-used selection gesture, and on both of them the
        // tab bindings used to eat it: there was no way to select to the end
        // of a line at all.
        let mut harness = Harness::panel(2);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        let active = harness.active_id();

        type_line(&mut harness, "echo hello");
        let chord = Modifiers {
            shift: true,
            ..platform_chord()
        };
        harness.press("left", chord, "");

        // macOS selects to the start of the line; everywhere else the same
        // chord is the one that selects by word.
        let expected = if cfg!(target_os = "macos") {
            "echo hello"
        } else {
            "hello"
        };
        assert_eq!(harness.field_selection(pane), expected);
        assert_eq!(harness.active_id(), active, "the tab moved instead");
    }

    #[test]
    fn only_the_composer_draws_a_caret_while_the_composer_has_the_keys() {
        // Two cursors in one pane — the shell's at its prompt and the
        // composer's under it — say nothing about which of them is listening,
        // and the steady one is the wrong one. The block list therefore draws
        // no cursor at all, and the composer's is a bar in the terminal's own
        // cursor colour.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.type_field(pane, "echo hello");

        let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
        let scene = harness.frame();
        let carets: Vec<_> = visible_rects(&scene)
            .filter(|(rect, bounds)| {
                rect.background == Fill::Solid(theme().terminal.cursor)
                    && bounds.width() < cell.width
                    && bounds.height() <= cell.height
            })
            .collect();
        assert_eq!(carets.len(), 1, "one caret, in the composer");

        assert!(
            !visible_rects(&scene).any(|(rect, bounds)| {
                rect.border.width > 0.
                    && (bounds.width() - cell.width).abs() < 0.5
                    && (bounds.height() - cell.height).abs() < 0.5
            }),
            "the shell's own cursor was drawn as well, hollow or otherwise"
        );
    }

    #[test]
    fn an_unmarked_shell_keeps_the_composer_on_a_row_of_its_own() {
        // The fallback, in the real tree. Nothing said where this prompt ends
        // — and nothing guesses, because there is no pattern in a prompt to
        // find — so the field keeps the row under the output it has always
        // had, on the same ground, in the same font and at the same gutter.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.frame();
        if !draws_blocks(&harness) {
            return;
        }
        harness.type_field(pane, "echo hello");

        let panel = panel_of_the_pane(&mut harness);
        let scene = harness.frame();
        let composer = composer_boxes(&scene)
            .first()
            .copied()
            .expect("a pane on the normal screen draws a composer");

        let typed: Vec<Vector2F> = scene
            .layers()
            .flat_map(|layer| layer.glyphs.iter())
            .filter(|glyph| glyph.glyph_key.glyph_id == u32::from('h'))
            .map(|glyph| glyph.position)
            .filter(|at| at.y() >= composer.min_y())
            .collect();
        assert!(
            !typed.is_empty(),
            "the line being typed is not inside the composer's own box"
        );

        let baseline = typed[0].y();
        let row = text_where(&scene, |at| (at.y() - baseline).abs() < 0.5);
        assert_eq!(
            row, "echo hello",
            "the field's row holds the line and nothing the shell printed"
        );
        let left = scene
            .layers()
            .flat_map(|layer| layer.glyphs.iter())
            .filter(|glyph| (glyph.position.y() - baseline).abs() < 0.5)
            .map(|glyph| glyph.position.x())
            .fold(f32::INFINITY, f32::min);
        assert!(
            (left - (panel.min_x() + crate::workspace::body::GUTTER)).abs() < 0.5,
            "column zero here is column zero in the output above"
        );
    }

    #[test]
    fn a_field_too_tall_for_its_pane_gives_way_instead_of_painting_over_it() {
        // The field grows downwards into the grid. In a short window a
        // six-line command used to be laid out past the panel it sits in —
        // over the border, over the pane below and off the bottom of the
        // window — with the grid above squeezed to no rows at all.
        // A shell that reports its boundaries, so that the pane is drawing the
        // block list and not the grid: an un-integrated shell whose one open
        // block has scrolled past the top of the viewport is drawn as a plain
        // grid, and a grid has no composer to measure. See `pane_surface::of`.
        let mut harness = Harness::panel(1);
        let Some(pane) = marked_shell(&mut harness) else {
            return;
        };
        await_prompt(&mut harness, pane);
        harness.type_field(pane, "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight");

        let scene = harness.frame_sized(vec2f(600., 200.));
        let panel = panel_boxes(&scene)[0];
        let field = composer_boxes(&scene)[0];
        let cell = CellFont::headless(CELL_FONT_SIZE).metrics();

        assert!(
            field.max_y() <= panel.max_y() + 0.5,
            "the field reaches {} and the panel ends at {}",
            field.max_y(),
            panel.max_y()
        );
        assert!(
            field.min_y() - panel.min_y() >= cell.height,
            "the output was squeezed to nothing to make room for the composer"
        );
    }

    /// The surface a shell with command marks gets: one item per command,
    /// hoverable, copyable, and with a rule under it only when something is
    /// cut off.
    ///
    /// These are the only tests that ask for the integration, and they are
    /// worth a real shell for the reason the rest of this module is: the whole
    /// point of a block is that the *shell* said where it ended.
    mod blocks {
        use super::*;
        use crate::pane_blocks::{PaneBlocks, ScrollPosition};

        /// Runs `command` through the composer and waits for the block it
        /// makes, reporting how many blocks the pane then has.
        ///
        /// Through the composer, not the pty: submitting is half of what marks
        /// a boundary, and a write that went round it would prove the shell's
        /// half only.
        ///
        /// The wait is for a *closed* block carrying this command, not merely
        /// for the count to grow: the shell prints its next prompt in the same
        /// breath, and a wait that stopped at the first change would read a
        /// block that had not been given its exit status yet.
        fn run(harness: &mut Harness, pane: PaneId, command: &str) -> usize {
            await_prompt(harness, pane);
            harness.type_field(pane, command);
            harness.press("enter", Modifiers::default(), "\r");

            let wanted = command.to_owned();
            harness.wait_for("the command never became a block", move |harness| {
                harness
                    .workspace
                    .read(&harness.app, |workspace, app| {
                        let blocks = workspace.terminal_blocks(pane, app)?;
                        Some(
                            blocks
                                .iter()
                                .any(|block| block.command.as_deref() == Some(wanted.as_str())),
                        )
                    })
                    .unwrap_or_default()
            });
            harness.frame();
            block_count(harness, pane)
        }

        fn block_count(harness: &Harness, pane: PaneId) -> usize {
            harness
                .workspace
                .read(&harness.app, |workspace, app| {
                    Some(workspace.terminal_blocks(pane, app)?.len())
                })
                .unwrap_or_default()
        }

        /// The text of one finished block, oldest first.
        fn block_text(harness: &Harness, pane: PaneId, index: usize) -> String {
            harness
                .workspace
                .read(&harness.app, |workspace, app| {
                    let blocks = workspace.terminal_blocks(pane, app)?;
                    Some(blocks.get(index)?.rows.to_text())
                })
                .unwrap_or_default()
        }

        /// Hovers the block at `index` and returns the frame that follows.
        fn hover_block(harness: &mut Harness, pane: PaneId, index: usize) -> Rc<Scene> {
            harness.workspace_update(|workspace, ctx| {
                assert!(
                    workspace.hover_block(pane, index, ctx),
                    "there is no block {index} to hover"
                );
            });
            harness.frame()
        }

        /// The controls drawn on a block in a frame, left to right, by their
        /// rounded plate.
        ///
        /// Bounded to the pane, because the button that opens another tab is
        /// the same square with the same radius in the header above it. Sorted
        /// by where they are rather than by the order they were painted in:
        /// which is the copy square and which is the menu is a thing about the
        /// picture.
        fn block_controls(scene: &Scene, panel: RectF) -> Vec<RectF> {
            let mut controls: Vec<RectF> = visible_rects(scene)
                .filter(|(rect, bounds)| {
                    rect.corner_radius.get_top_left() == Radius::Pixels(5.)
                        && (bounds.width() - bounds.height()).abs() < 0.5
                        && bounds.width() > 20.
                        && bounds.width() < 32.
                })
                .map(|(_, bounds)| bounds)
                .filter(|bounds| panel.contains_point(center(*bounds)))
                .collect();
            controls.sort_by(|left, right| left.min_x().total_cmp(&right.min_x()));
            controls
        }

        /// Opens the menu on the block at `index`, the way pressing its dots
        /// does, and returns the frame that follows.
        fn open_block_menu(harness: &mut Harness, pane: PaneId, index: usize) -> Rc<Scene> {
            harness.workspace_update(|workspace, ctx| {
                assert!(
                    workspace.open_block_menu_at(pane, index, ctx),
                    "there is no block {index} to open a menu on"
                );
            });
            harness.frame()
        }

        /// The popup the block menu is drawn in, by its width and its corner.
        fn block_menu_box(scene: &Scene) -> RectF {
            visible_rects(scene)
                .filter(|(rect, bounds)| {
                    rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                        && (bounds.width() - 260.).abs() < 1.
                })
                .map(|(_, bounds)| bounds)
                .next()
                .expect("no block menu is up")
        }

        /// Clicks the entry of the block menu that says `label`.
        ///
        /// Found inside the popup's own box rather than anywhere in the frame:
        /// the menu is drawn over a shell's output, and a line built from both
        /// would be a line neither of them drew.
        fn click_menu_entry(harness: &mut Harness, label: &str) {
            let scene = harness.frame();
            let popup = block_menu_box(&scene);
            let row = text_lines(&scene, |at| {
                at.x() >= popup.min_x() && at.x() <= popup.max_x()
            })
            .into_iter()
            .find(|(_, line)| line.trim() == label)
            .unwrap_or_else(|| panic!("no entry of the block menu says {label:?}"));
            harness.click(row.0 + vec2f(4., 4.), MouseButton::Left);
            harness.frame();
        }

        #[test]
        fn a_command_becomes_a_block_with_a_divider_above_it() {
            // The whole feature, end to end: a real shell says where its
            // command started and finished, and the pane draws one item per
            // command with a hairline between them.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ALPHA") == 0 {
                return;
            }
            run(&mut harness, pane, "echo BETA");

            assert!(
                block_text(&harness, pane, 0).contains("ALPHA"),
                "the first command's output is in the first block"
            );
            assert!(
                !block_text(&harness, pane, 0).contains("BETA"),
                "and the next command's is not"
            );

            // One rule per boundary: above the second block and above the open
            // one, and never along the very top of the pane. The open block
            // has to have drawn its prompt first — an item that has printed
            // nothing has no height and therefore no edge to rule.
            await_prompt(&mut harness, pane);
            let scene = harness.frame();
            let panel = panel_of_the_pane(&mut harness);
            let rules: Vec<_> = visible_rects(&scene)
                .filter(|(rect, bounds)| {
                    rect.background == Fill::Solid(theme().overlay_2)
                        && (bounds.width() - panel.width()).abs() < 0.5
                        && bounds.height() <= 1.5
                })
                .collect();
            assert!(
                rules.len() >= 2,
                "two blocks and an open one need two dividers, not {}",
                rules.len()
            );
            assert!(
                rules
                    .iter()
                    .all(|(_, bounds)| bounds.min_y() > panel.min_y() + 0.5),
                "a rule along the top of the pane separates the pane from nothing"
            );
        }

        #[test]
        fn a_failed_command_is_washed_in_the_theme_s_red() {
            // Failure is visible without reading: scanning a long session for
            // what broke is a glance rather than a search.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "false") == 0 {
                return;
            }

            let scene = harness.frame();
            let red = theme().terminal.normal[1];
            let stripe = visible_rects(&scene)
                .find(|(rect, bounds)| rect.background == Fill::Solid(red) && bounds.width() < 8.)
                .map(|(_, bounds)| bounds);
            assert!(
                stripe.is_some(),
                "a block that failed carries a stripe down its left edge"
            );
            assert!(
                visible_rects(&scene).any(|(rect, _)| {
                    matches!(rect.background, Fill::Solid(fill)
                        if (fill.r, fill.g, fill.b) == (red.r, red.g, red.b) && fill.a < 255)
                }),
                "and a wash over the whole of it"
            );
        }

        #[test]
        fn hovering_a_block_reveals_a_copy_control_that_takes_that_block_and_no_other() {
            // What scrollback cannot do: one click, and exactly that command
            // and its output — no neighbour's text, no over-selection, no
            // trailing blank rows.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ALPHA") == 0 {
                return;
            }
            run(&mut harness, pane, "echo BETA");

            let panel = panel_of_the_pane(&mut harness);
            assert!(
                block_controls(&harness.frame(), panel).is_empty(),
                "nothing is hovered, so no control is drawn"
            );

            let scene = hover_block(&mut harness, pane, 0);
            let controls = block_controls(&scene, panel);
            assert_eq!(
                controls.len(),
                2,
                "two controls, on the hovered block: the copy square and the dots"
            );
            assert_eq!(
                icons_in(&scene, controls[0], Lucide::Copy).len(),
                1,
                "the left plate is empty: it says nothing about what it does"
            );
            assert_eq!(
                icons_in(&scene, controls[1], Lucide::EllipsisVertical).len(),
                1,
                "the right plate is empty: nothing says a menu opens there"
            );

            let Some((clipboard, _system)) = working_clipboard(&harness) else {
                return;
            };
            let at = center(controls[0]);
            harness.click(at, MouseButton::Left);

            let copied = clipboard.read().unwrap_or_default();
            assert!(
                copied.contains("ALPHA"),
                "the block that was copied: {copied:?}"
            );
            assert!(
                !copied.contains("BETA"),
                "its neighbour came with it: {copied:?}"
            );
            assert_eq!(
                copied,
                copied.trim_end(),
                "the block's trailing blank rows came with it"
            );
        }

        /// Every plugin a release binary carries except the one that owns the
        /// block menu.
        fn without_the_menu() -> Vec<Box<dyn crate::plugin::Plugin>> {
            let blocks = crook_plugin::PluginId::parse("crook/blocks").expect("a literal");
            let mut plugins = crate::plugins::defaults();
            plugins.retain(|plugin| plugin.manifest().id != blocks);
            plugins
        }

        /// A plugin that is not in the box and puts one entry in the menu.
        ///
        /// The proof the slot is a seam rather than a way of writing Crook's
        /// own four groups down: this one is a stranger to every module the
        /// menu is built from, and it reaches the same list.
        struct Probe;

        impl crate::plugin::Plugin for Probe {
            fn manifest(&self) -> &'static crook_plugin::Manifest {
                static MANIFEST: std::sync::OnceLock<crook_plugin::Manifest> =
                    std::sync::OnceLock::new();
                MANIFEST.get_or_init(|| crook_plugin::Manifest {
                    schema: crook_plugin::Manifest::SCHEMA,
                    id: crook_plugin::PluginId::parse("eugen/probe").expect("a literal"),
                    name: "Probe",
                    description: "A plugin that exists to be looked at.",
                    version: "0.1.0",
                    tier: crook_plugin::Tier::Native,
                    capabilities: &[],
                })
            }

            fn build(
                &mut self,
                host: &mut crate::plugin::Host,
                _: &mut ViewContext<Workspace>,
            ) -> Result<(), crate::plugin::BuildError> {
                host.contribute(
                    crate::plugins::blocks::BLOCK_MENU,
                    "probe",
                    5,
                    |workspace, _| {
                        Text::new("Probe this block", workspace.fonts().ui, 12.)
                            .with_color(theme().text_primary)
                            .finish()
                    },
                );
                Ok(())
            }
        }

        #[test]
        fn what_a_plugin_puts_in_the_menu_is_drawn_in_it() {
            // `block.menu` is the first surface in the application that is
            // about the work rather than about the window, and this is the
            // whole claim: a plugin nothing in the menu's own modules knows
            // about is drawn in it, in its own group, under a rule.
            let mut plugins = crate::plugins::defaults();
            plugins.push(Box::new(Probe));
            let mut harness = Harness::with_plugins(1, Settings::ephemeral(), plugins);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ALPHA") == 0 {
                return;
            }

            let scene = open_block_menu(&mut harness, pane, 0);
            let popup = block_menu_box(&scene);
            let lines: Vec<String> = text_lines(&scene, |at| {
                at.x() >= popup.min_x() && at.x() <= popup.max_x()
            })
            .into_iter()
            .map(|(_, line)| line.trim().to_owned())
            .collect();

            assert!(
                lines.iter().any(|line| line == "Probe this block"),
                "the plugin's entry is not in the menu: {lines:?}"
            );
            // At order 5, which is between Crook's own copy group and the
            // facts under it — a plugin can land between two of them rather
            // than only at the ends.
            let probe = lines.iter().position(|line| line == "Probe this block");
            let facts = lines
                .iter()
                .position(|line| line == "Copy working directory");
            assert!(
                probe < facts,
                "the slot's order did not decide where it went: {lines:?}"
            );
        }

        #[test]
        fn a_sandboxed_plugin_reads_the_block_its_entry_was_pressed_in() {
            // The second tier reaching one command, end to end: a `.wasm` file
            // in a directory describes two entries and never says what a menu
            // row looks like; a person presses one; the plugin asks what the
            // command printed, which only a press may ask; and asks for
            // something to go on the clipboard, which only a press may do.
            let scratch = crate::plugins::wasm::tests::Scratch::new("blockmenu");
            crate::plugins::wasm::tests::install(
                scratch.path(),
                "probe",
                &crate::plugins::wasm::tests::wasm_in_a_block_menu("eugen/probe"),
            );
            let mut settings = Settings::ephemeral();
            settings.set_granted(
                "eugen/probe",
                vec![String::from("block.read"), String::from("clipboard")],
            );
            let mut plugins = crate::plugins::defaults();
            plugins.extend(crate::plugins::wasm::installed(scratch.path()));
            let mut harness = Harness::with_plugins(1, settings, plugins);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ALPHA") == 0 {
                return;
            }
            let Some((clipboard, _system)) = working_clipboard(&harness) else {
                return;
            };

            let scene = open_block_menu(&mut harness, pane, 0);
            let popup = block_menu_box(&scene);
            let lines: Vec<String> = text_lines(&scene, |at| {
                at.x() >= popup.min_x() && at.x() <= popup.max_x()
            })
            .into_iter()
            .map(|(_, line)| line.trim().to_owned())
            .collect();
            assert!(
                lines.iter().any(|line| line == "Copy as Markdown"),
                "the guest's entries are not in the menu: {lines:?}"
            );
            assert!(
                lines.iter().any(|line| line == "Nothing to do here"),
                "an entry whose action nothing answers to is drawn, not dropped: {lines:?}"
            );

            click_menu_entry(&mut harness, "Copy as Markdown");
            assert!(
                !harness.a_popup_is_open(),
                "a sandboxed plugin's entry left the menu up"
            );

            // Two requests round the loop — what the command printed, then the
            // clipboard — each served by the window rather than by the pool.
            let fence = crate::plugins::wasm::tests::FENCE;
            harness.wait_for("the plugin never copied anything", move |harness| {
                harness.frame();
                clipboard.read().as_deref() == Some(fence)
            });
        }

        #[test]
        fn a_block_carries_no_dots_when_nothing_puts_anything_in_the_menu() {
            // Switching the plugin that owns the menu off has to take its
            // control with it. A button that opened an empty popup would be
            // the plugin still on screen after it was gone.
            let mut harness = Harness::with_plugins(1, Settings::ephemeral(), without_the_menu());
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ALPHA") == 0 {
                return;
            }

            let panel = panel_of_the_pane(&mut harness);
            let scene = hover_block(&mut harness, pane, 0);
            let controls = block_controls(&scene, panel);
            assert_eq!(controls.len(), 1, "the copy square, and nothing beside it");
            assert!(
                icons_in(&scene, controls[0], Lucide::EllipsisVertical).is_empty(),
                "the dots are drawn with no menu behind them"
            );

            harness.workspace_update(|workspace, ctx| {
                assert!(
                    !workspace.open_block_menu_at(pane, 0, ctx),
                    "a menu with nothing in it opened anyway"
                );
            });
            assert!(!harness.a_popup_is_open());
        }

        #[test]
        fn the_dots_open_a_menu_that_copies_the_command_and_the_output_apart() {
            // The whole of what a block menu is for: the two halves of a block
            // that a pointer cannot take apart. A drag over the output catches
            // the prompt at one end and the shell's next line at the other,
            // and these two are exact because the marks said where the command
            // ended.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ALPHA") == 0 {
                return;
            }
            let Some((clipboard, _system)) = working_clipboard(&harness) else {
                return;
            };

            open_block_menu(&mut harness, pane, 0);
            assert!(
                harness.a_popup_is_open(),
                "the menu is a popup, and the window has to know one is up"
            );
            click_menu_entry(&mut harness, "Copy command");

            let copied = clipboard.read().unwrap_or_default();
            assert_eq!(copied, "echo ALPHA", "the command line, and only it");
            assert!(
                !harness.a_popup_is_open(),
                "an entry that has run leaves the menu up"
            );

            open_block_menu(&mut harness, pane, 0);
            click_menu_entry(&mut harness, "Copy output");
            let copied = clipboard.read().unwrap_or_default();
            assert_eq!(
                copied, "ALPHA",
                "what the command printed, without the prompt or the line it was typed on"
            );
        }

        #[test]
        fn the_menu_brings_the_block_it_is_open_on_to_the_top_of_the_pane() {
            // A block taller than the window is read from its own first row,
            // and finding that row by turning the wheel is how somebody scrolls
            // past it twice.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "seq 1 200") == 0 {
                return;
            }
            run(&mut harness, pane, "echo AFTER");

            let at_the_end = harness.workspace.read(&harness.app, |workspace, _| {
                workspace.pane_blocks(pane).map(PaneBlocks::position)
            });
            assert!(
                matches!(at_the_end, Some(ScrollPosition::FollowBottom)),
                "a session that has just run a command follows its end: {at_the_end:?}"
            );

            open_block_menu(&mut harness, pane, 0);
            click_menu_entry(&mut harness, "Scroll to top of block");

            let moved = harness.workspace.read(&harness.app, |workspace, _| {
                workspace.pane_blocks(pane).map(PaneBlocks::position)
            });
            assert!(
                matches!(moved, Some(ScrollPosition::Fixed(lines)) if lines.abs() < 0.5),
                "the first block starts at the top of the list, so the list is at line zero: \
                 {moved:?}"
            );
        }

        #[test]
        fn running_a_command_again_puts_it_back_in_the_field_and_sends_nothing() {
            // A menu that ran a command would be a menu that runs the `rm`
            // somebody opened it to read. What it does instead is type.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ALPHA") == 0 {
                return;
            }
            let before = block_count(&harness, pane);

            open_block_menu(&mut harness, pane, 0);
            click_menu_entry(&mut harness, "Run again");

            let field = harness.workspace.read(&harness.app, |workspace, _| {
                workspace
                    .input(pane)
                    .map(|input| input.editor().text().to_owned())
            });
            assert_eq!(
                field.as_deref(),
                Some("echo ALPHA"),
                "the command is in the field, waiting for an Enter that is the person's"
            );
            assert_eq!(
                block_count(&harness, pane),
                before,
                "nothing was sent, so no block was made"
            );
        }

        #[test]
        fn escape_takes_the_block_menu_down() {
            // Every modal popup in the window answers Escape, and the workspace
            // claims the key before the pane under it can type one.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ALPHA") == 0 {
                return;
            }

            open_block_menu(&mut harness, pane, 0);
            assert_eq!(
                harness.action_for("escape", Modifiers::default()),
                Some(WorkspaceAction::Block(BlockAction::CloseMenu)),
            );

            harness.press("escape", Modifiers::default(), "\x1b");
            assert!(!harness.a_popup_is_open(), "Escape left the menu up");
        }

        #[test]
        fn the_rule_above_the_composer_appears_only_when_something_is_cut_off() {
            // Warp's rule, and the reason the seam is invisible until it means
            // something: while the list is following its own end there is
            // nothing below the fold to separate the composer from.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ALPHA") == 0 {
                return;
            }

            let seam = |harness: &mut Harness| {
                let scene = harness.frame();
                composer_boxes(&scene)
                    .first()
                    .map(|composer| composer.height())
                    .expect("a pane on the normal screen draws a composer")
            };

            let flush = seam(&mut harness);

            // Enough output to overflow the pane, then a scroll up: now a
            // block really is cut off underneath.
            run(&mut harness, pane, "seq 1 200");
            harness.workspace_update(|workspace, ctx| {
                assert!(
                    workspace.scroll_blocks(pane, -20., ctx),
                    "the list had nothing to scroll"
                );
            });

            assert!(
                seam(&mut harness) > flush,
                "the rule takes a pixel out of the composer's box, and it is not there when the \
                 list is at its own end"
            );
        }

        #[test]
        fn only_the_rows_in_view_are_painted_however_long_the_block_is() {
            // The whole reason this is not a `Scrollable`: a frame costs a
            // screenful, not a session.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "seq 1 400") == 0 {
                return;
            }

            let scene = harness.frame();
            let digits = scene.layers().flat_map(|layer| layer.glyphs.iter()).count();
            assert!(
                digits < 2000,
                "a four-hundred-line block painted {digits} glyphs; only the rows in view \
                 should have been drawn"
            );
        }

        #[test]
        fn a_finished_block_keeps_the_colours_it_printed_in() {
            // The store keeps every cell's background and every underline, and
            // a block that lost them when its command ended would turn `git
            // diff`, `grep --color`, `ls` and a powerlevel10k prompt — which is
            // almost entirely background — into flat text one prompt later.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "printf '\\033[41mRED\\033[0m\\n'") == 0 {
                return;
            }

            let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
            let red = theme().terminal.normal[1];
            let scene = harness.frame();
            let fills: Vec<_> = visible_rects(&scene)
                .filter(|(rect, bounds)| {
                    rect.background == Fill::Solid(red)
                        && (bounds.height() - cell.height).abs() < 1.
                        && bounds.width() > cell.width
                })
                .collect();
            assert!(
                !fills.is_empty(),
                "the finished block was painted without the background it printed on"
            );
        }

        #[test]
        fn running_a_command_brings_the_list_back_to_its_own_end() {
            // Enter means the person is done reading history. Without this the
            // view stays where it was scrolled to and nothing on screen answers
            // the command they just ran.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "seq 1 200") == 0 {
                return;
            }

            harness.workspace_update(|workspace, ctx| {
                assert!(
                    workspace.scroll_blocks(pane, -40., ctx),
                    "the list had nothing to scroll"
                );
            });
            harness.frame();
            let scrolled = harness.workspace.read(&harness.app, |workspace, _| {
                workspace.pane_blocks(pane).map(PaneBlocks::position)
            });
            assert!(
                matches!(scrolled, Some(ScrollPosition::Fixed(_))),
                "the list did not stay where it was scrolled to: {scrolled:?}"
            );

            run(&mut harness, pane, "echo AFTER");
            let after = harness.workspace.read(&harness.app, |workspace, _| {
                workspace.pane_blocks(pane).map(PaneBlocks::position)
            });
            assert_eq!(
                after,
                Some(ScrollPosition::FollowBottom),
                "the command ran and the view never came back to it"
            );
        }

        #[test]
        fn a_block_taller_than_the_pane_still_offers_its_copy_control() {
            // The control used to be pinned to the block's own top edge, which
            // for the blocks most worth copying is a point above the window:
            // painted outside the clip, and clickable nowhere.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "seq 1 400") == 0 {
                return;
            }

            let panel = panel_of_the_pane(&mut harness);
            let scene = hover_block(&mut harness, pane, 0);
            assert_eq!(
                block_controls(&scene, panel).len(),
                2,
                "a block whose top has scrolled out of view drew no controls"
            );
        }

        #[test]
        fn the_control_follows_the_list_when_it_moves_under_a_still_pointer() {
            // A wheel, and a command finishing, both put a different block
            // under a pointer that has not moved. A control left on the block
            // that *was* there is an affordance pointing at output a click
            // would not copy.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            if run(&mut harness, pane, "echo ONE") == 0 {
                return;
            }
            run(&mut harness, pane, "seq 1 200");

            let panel = panel_of_the_pane(&mut harness);
            let near_the_top = vec2f(panel.min_x() + panel.width() / 2., panel.min_y() + 8.);
            harness.move_to(near_the_top);
            harness.frame();
            let over_the_tall_one = harness.workspace.read(&harness.app, |workspace, _| {
                workspace.pane_blocks(pane).and_then(PaneBlocks::hovered)
            });

            harness.workspace_update(|workspace, ctx| {
                workspace.scroll_blocks(pane, -1000., ctx);
            });
            harness.frame();
            let over_the_first = harness.workspace.read(&harness.app, |workspace, _| {
                workspace.pane_blocks(pane).and_then(PaneBlocks::hovered)
            });

            assert!(over_the_tall_one.is_some() && over_the_first.is_some());
            assert_ne!(
                over_the_tall_one, over_the_first,
                "the list scrolled to its top and the control stayed on the block that had \
                 been under the pointer"
            );
        }

        /// Where the shell said its prompt ends, or `None` when it has not
        /// said — which is the unmarked case and has its own test.
        fn prompt_end(harness: &Harness, pane: PaneId) -> Option<crook_terminal::PromptEnd> {
            harness.workspace.read(&harness.app, |workspace, app| {
                workspace.terminal(pane, app)?.1.live_block.prompt_end
            })
        }

        /// The baseline the composer's first row is drawn on when it continues
        /// the prompt: the row *above* the field's own box, which the list
        /// painted the prompt into.
        fn prompt_row_baseline(scene: &Scene) -> f32 {
            let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
            let composer = composer_boxes(scene)
                .first()
                .copied()
                .expect("a pane on the normal screen draws a composer");
            composer.min_y() - cell.height + cell.baseline
        }

        #[test]
        fn a_line_typed_at_the_prompt_shares_its_row_and_still_reaches_the_shell() {
            // **The whole of the change, end to end, against a real shell.**
            // The shell says where its prompt ends, the line being typed
            // starts in that very cell on that very row — and pressing Enter
            // still runs it and still makes it a block of its own.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            await_prompt(&mut harness, pane);

            let Some(prompt) = prompt_end(&harness, pane) else {
                return;
            };
            type_line(&mut harness, "echo INLINE");

            let panel = panel_of_the_pane(&mut harness);
            let scene = harness.frame();
            let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
            let baseline = prompt_row_baseline(&scene);

            let row = text_where(&scene, |at| (at.y() - baseline).abs() < 0.5);
            assert!(
                row.ends_with("echo INLINE"),
                "the line being typed is not on the prompt's row: {row:?}"
            );
            assert!(
                row.len() > "echo INLINE".len(),
                "the prompt should be on that row too, and it is only {row:?}"
            );

            // In the cell the shell named, and not a column either side of it.
            let start =
                panel.min_x() + crate::workspace::body::GUTTER + prompt.column as f32 * cell.width;
            let first: Vec<char> = scene
                .layers()
                .flat_map(|layer| layer.glyphs.iter())
                .filter(|glyph| (glyph.position.y() - baseline).abs() < 0.5)
                .filter(|glyph| (glyph.position.x() - start).abs() < 0.5)
                .filter_map(|glyph| char::from_u32(glyph.glyph_key.glyph_id))
                .collect();
            assert_eq!(
                first,
                vec!['e'],
                "the first character of the line is not in the cell OSC 133 B named"
            );

            // And Enter still does what Enter did.
            harness.press("enter", Modifiers::default(), "\r");
            harness.wait_for("the command never became a block", |harness| {
                harness
                    .workspace
                    .read(&harness.app, |workspace, app| {
                        let blocks = workspace.terminal_blocks(pane, app)?;
                        Some(
                            blocks
                                .iter()
                                .any(|block| block.command.as_deref() == Some("echo INLINE")),
                        )
                    })
                    .unwrap_or_default()
            });
            harness.frame();

            let last = block_count(&harness, pane) - 1;
            let text = block_text(&harness, pane, last);
            assert!(
                text.contains("INLINE"),
                "the command and its output are not in the block it made: {text:?}"
            );
            assert_eq!(
                harness.field_text(pane),
                "",
                "the field kept the line it sent"
            );
        }

        #[test]
        fn the_prompt_s_row_is_the_list_s_on_the_left_and_the_field_s_on_the_right() {
            // The row belongs to two elements, and both are handed every
            // press. A click past the prompt puts the caret in the line being
            // typed; a click on the prompt itself is output, and must leave
            // that caret exactly where it was.
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            harness.frame();
            await_prompt(&mut harness, pane);

            let Some(prompt) = prompt_end(&harness, pane) else {
                return;
            };
            harness.type_field(pane, "echo hello");

            let panel = panel_of_the_pane(&mut harness);
            let scene = harness.frame();
            let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
            let middle = prompt_row_baseline(&scene) - cell.baseline + cell.height * 0.5;
            let start =
                panel.min_x() + crate::workspace::body::GUTTER + prompt.column as f32 * cell.width;

            harness.click(vec2f(start + cell.width * 5., middle), MouseButton::Left);
            assert_eq!(
                harness.field_caret(pane),
                5,
                "the boundary before `hello`, five cells into the line"
            );

            harness.click(
                vec2f(
                    panel.min_x() + crate::workspace::body::GUTTER + cell.width * 0.5,
                    middle,
                ),
                MouseButton::Left,
            );
            assert_eq!(
                harness.field_caret(pane),
                5,
                "a click on the prompt moved a caret that is not in the prompt"
            );
        }

        /// Selecting text across the list: the commands that have finished as
        /// well as the one still open.
        ///
        /// Every test here aims at the pixels a frame actually painted, and
        /// that is the point: a finished block's rows left the emulator when
        /// its command ended, so a test that named cells of the grid would be
        /// testing the very thing that stopped working.
        mod selecting {
            use super::*;

            /// Every row of glyphs a frame painted, top to bottom: the
            /// baseline it sits on, its characters left to right, and where
            /// each of them starts.
            ///
            /// A blank cell draws no glyph, so a row's text is its non-blank
            /// characters run together — which is why the markers below are
            /// single words.
            fn painted_rows(scene: &Scene) -> Vec<(f32, String, Vec<f32>)> {
                let mut rows: Vec<(f32, Vec<(f32, char)>)> = Vec::new();
                for glyph in scene.layers().flat_map(|layer| layer.glyphs.iter()) {
                    let Some(character) = char::from_u32(glyph.glyph_key.glyph_id) else {
                        continue;
                    };
                    let at = glyph.position;
                    match rows.iter_mut().find(|(y, _)| (y - at.y()).abs() < 0.5) {
                        Some((_, cells)) => cells.push((at.x(), character)),
                        None => rows.push((at.y(), vec![(at.x(), character)])),
                    }
                }

                rows.sort_by(|left, right| left.0.total_cmp(&right.0));
                rows.into_iter()
                    .map(|(y, mut cells)| {
                        cells.sort_by(|left, right| left.0.total_cmp(&right.0));
                        (
                            y,
                            cells.iter().map(|(_, c)| *c).collect(),
                            cells.iter().map(|(x, _)| *x).collect(),
                        )
                    })
                    .collect()
            }

            /// Where the *last* occurrence of `text` was painted: the pen
            /// position of its first character and of its last.
            ///
            /// The last rather than the first, because the block under test is
            /// usually the newest one and the command that made it is on
            /// screen above it.
            fn painted(scene: &Scene, text: &str) -> Option<(Vector2F, Vector2F)> {
                painted_rows(scene)
                    .into_iter()
                    .rev()
                    .find_map(|(y, row, xs)| {
                        let at = row.rfind(text)?;
                        let first = row[..at].chars().count();
                        let last = first + text.chars().count() - 1;
                        Some((vec2f(*xs.get(first)?, y), vec2f(*xs.get(last)?, y)))
                    })
            }

            /// The point a press aimed at a painted cell lands on: `across` of
            /// the way into it, and half way down it.
            fn aim(pen: Vector2F, across: f32) -> Vector2F {
                let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
                vec2f(
                    pen.x() + across * cell.width,
                    pen.y() - cell.baseline + cell.height * 0.5,
                )
            }

            /// Drags from the start of the last `from` on screen to the end of
            /// the last `to`, and reports what the pane then has selected.
            fn drag(harness: &mut Harness, pane: PaneId, from: &str, to: &str) -> Option<String> {
                let scene = harness.frame();
                let start = aim(
                    painted(&scene, from)
                        .unwrap_or_else(|| panic!("{from:?} is not on screen"))
                        .0,
                    0.1,
                );
                let end = aim(
                    painted(&scene, to)
                        .unwrap_or_else(|| panic!("{to:?} is not on screen"))
                        .1,
                    0.9,
                );

                harness.hold(start, 1);
                harness.drag_to(end);
                harness.let_go(end);
                harness.terminal_selection(pane)
            }

            /// Puts two files with unmistakable names in a directory of their
            /// own and moves the shell into it, so that the listing under test
            /// is short enough to be on screen and says what it is.
            fn scratch_directory(harness: &mut Harness, pane: PaneId) -> bool {
                run(harness, pane, "cd \"$(mktemp -d)\" && touch ZULU YANKEE") > 0
            }

            #[test]
            fn dragging_across_a_finished_command_s_output_copies_exactly_those_cells() {
                // **The bug, in the words it was reported in: run `ls -la`,
                // then go to select what it printed, and find that you
                // cannot.** The command's rows were harvested out of the
                // emulator the moment it ended, and the selection was still in
                // the emulator — so everything a person had already run had
                // become unselectable, which is the whole reason to look at a
                // terminal.
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if !scratch_directory(&mut harness, pane) {
                    return;
                }
                run(&mut harness, pane, "ls -la");
                await_prompt(&mut harness, pane);

                // It really is a finished block: its rows are in the store,
                // with the command that made them.
                let last = block_count(&harness, pane) - 1;
                assert_eq!(
                    Some("ls -la"),
                    harness
                        .workspace
                        .read(&harness.app, |workspace, app| {
                            let blocks = workspace.terminal_blocks(pane, app)?;
                            Some(blocks.get(last)?.command.clone())
                        })
                        .flatten()
                        .as_deref(),
                    "the newest block is not the listing"
                );
                assert!(block_text(&harness, pane, last).contains("ZULU"));

                assert_eq!(
                    Some("ZULU".to_owned()),
                    drag(&mut harness, pane, "ZULU", "ZULU"),
                    "a drag across a finished block's output selected something else"
                );

                // And the copy chord takes it and lets go of it, which is the
                // only sign a copy happened at all.
                harness.press("c", platform_chord(), "c");
                assert_eq!(None, harness.terminal_selection(pane));
            }

            #[test]
            fn with_nothing_selected_ctrl_c_still_interrupts_the_shell() {
                // **The rule that must survive the whole of this**, on the
                // surface the whole of this changed. Off macOS the copy chord
                // is also SIGINT, and the collision is settled by the
                // selection existing rather than by the key: with something
                // selected it copies and *lets go*, so the very next press is
                // the interrupt it has always been.
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if run(&mut harness, pane, "echo MARKER") == 0 {
                    return;
                }
                await_prompt(&mut harness, pane);

                // A command that will not end on its own, so only an interrupt
                // can let the next one run.
                harness.type_field(pane, "sleep 30");
                harness.press("enter", Modifiers::default(), "\r");
                harness.settle(NO_LEAK_PATIENCE);

                let ctrl = Modifiers {
                    ctrl: true,
                    ..Default::default()
                };
                assert_eq!(
                    Some("MARKER".to_owned()),
                    drag(&mut harness, pane, "MARKER", "MARKER"),
                    "the finished block above the running command is selectable"
                );
                harness.press("c", ctrl, "c");
                assert_eq!(
                    None,
                    harness.terminal_selection(pane),
                    "the copy did not let go of the selection"
                );

                // Now nothing is selected, so this one is the interrupt.
                harness.press("c", ctrl, "c");
                harness.type_into(pane, "echo AFTER\n");
                harness.wait_for("the second ctrl-c never reached the shell", |harness| {
                    harness
                        .workspace
                        .read(&harness.app, |workspace, app| {
                            let blocks = workspace.terminal_blocks(pane, app)?;
                            Some(
                                blocks
                                    .iter()
                                    .any(|block| block.rows.to_text().contains("AFTER")),
                            )
                        })
                        .unwrap_or_default()
                });
            }

            #[test]
            fn a_selection_spanning_three_blocks_takes_the_ends_partly_and_the_middle_whole() {
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if run(&mut harness, pane, "echo AAAA") == 0 {
                    return;
                }
                run(&mut harness, pane, "echo BBBB");
                run(&mut harness, pane, "echo CCCC");
                await_prompt(&mut harness, pane);

                let copied =
                    drag(&mut harness, pane, "AAAA", "CCCC").expect("three blocks are selected");
                assert!(
                    copied.starts_with("AAAA"),
                    "the first block is not taken from the press onwards: {copied:?}"
                );
                assert!(
                    copied.ends_with("CCCC"),
                    "the last block is not taken up to the release: {copied:?}"
                );
                assert!(
                    copied.contains("echo BBBB") && copied.contains("\nBBBB"),
                    "the block between them is not whole: {copied:?}"
                );
                assert!(
                    copied.lines().count() >= 4,
                    "three blocks in {copied:?} is not enough lines"
                );
            }

            #[test]
            fn a_selection_inside_the_open_block_still_works() {
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if run(&mut harness, pane, "echo LIVE") == 0 {
                    return;
                }
                // A command that keeps its block open, so the rows dragged
                // across are the grid's rather than a store's.
                harness.type_field(pane, "printf 'OPE%s\\n' N; sleep 30");
                harness.press("enter", Modifiers::default(), "\r");
                harness.wait_for("the command never printed", |harness| {
                    harness.terminal_text(pane).contains("OPEN")
                });
                harness.frame();

                assert_eq!(
                    Some("OPEN".to_owned()),
                    drag(&mut harness, pane, "OPEN", "OPEN")
                );
                harness.press("c", Modifiers::default(), "\u{3}");
            }

            #[test]
            fn a_double_click_takes_a_word_of_a_finished_block_and_a_triple_click_its_line() {
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if run(&mut harness, pane, "echo WORD LINE") == 0 {
                    return;
                }
                await_prompt(&mut harness, pane);

                let scene = harness.frame();
                let at = aim(
                    painted(&scene, "WORD").expect("the output is on screen").0,
                    0.5,
                );
                harness.hold(at, 2);
                harness.let_go(at);
                assert_eq!(Some("WORD".to_owned()), harness.terminal_selection(pane));

                harness.hold(at, 3);
                harness.let_go(at);
                assert_eq!(
                    Some("WORD LINE".to_owned()),
                    harness.terminal_selection(pane),
                    "a triple click takes the whole row of a finished block"
                );
            }

            #[test]
            fn more_output_arriving_does_not_move_a_selection() {
                // Nothing about a finished block moves when another command
                // runs after it: it is a different item of the list, not more
                // rows pushed under the same cells.
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if run(&mut harness, pane, "echo KEEPME") == 0 {
                    return;
                }
                await_prompt(&mut harness, pane);
                assert_eq!(
                    Some("KEEPME".to_owned()),
                    drag(&mut harness, pane, "KEEPME", "KEEPME")
                );

                // Written straight to the shell rather than typed: typing is
                // one of the rules that *does* let go of a selection, and what
                // is under test here is the one thing that must not.
                harness.type_into(pane, "echo LATER\n");
                // For the *block* rather than for the text on screen: closing
                // one harvests its rows and clears the grid above the next, so
                // a wait that watched the viewport could miss the output
                // between it arriving and being taken out.
                harness.wait_for("the shell never ran the second command", |harness| {
                    harness
                        .workspace
                        .read(&harness.app, |workspace, app| {
                            let blocks = workspace.terminal_blocks(pane, app)?;
                            Some(
                                blocks
                                    .iter()
                                    .any(|block| block.rows.to_text().contains("LATER")),
                            )
                        })
                        .unwrap_or_default()
                });
                harness.frame();
                assert_eq!(
                    Some("KEEPME".to_owned()),
                    harness.terminal_selection(pane),
                    "output arriving underneath a selection took it away"
                );
            }

            #[test]
            fn scrolling_the_list_does_not_move_a_selection() {
                // The anchors name a block and a row of it, so the wheel moves
                // the list under the highlight and neither of them minds.
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if run(&mut harness, pane, "echo SCROLLME") == 0 {
                    return;
                }
                for _ in 0..8 {
                    run(&mut harness, pane, "echo FILLER");
                }
                await_prompt(&mut harness, pane);

                // Back up the list far enough for the marker to be on screen,
                // then select it and scroll away again.
                harness.workspace_update(|workspace, ctx| {
                    workspace.scroll_blocks(pane, -100., ctx);
                });
                let Some(copied) = drag(&mut harness, pane, "SCROLLME", "SCROLLME") else {
                    panic!("the marker never came back into view");
                };
                assert_eq!("SCROLLME", copied);

                harness.workspace_update(|workspace, ctx| {
                    workspace.scroll_blocks(pane, 100., ctx);
                });
                harness.frame();
                assert_eq!(
                    Some("SCROLLME".to_owned()),
                    harness.terminal_selection(pane),
                    "scrolling the list moved the selection off its text"
                );
            }

            #[test]
            fn a_selection_across_a_folded_line_in_a_finished_block_copies_as_one_line() {
                // The fold is a place the terminal put a line too long for the
                // pane rather than something the shell printed, and the store
                // a finished block's rows live in has to remember that.
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                let columns = harness.terminal_columns(pane);
                let digits: String = (100..200).map(|number| number.to_string()).collect();
                let long = format!("FOLDSTART{digits}FOLDEND");
                assert!(
                    long.len() > columns,
                    "the line has to be long enough to fold"
                );
                if run(
                    &mut harness,
                    pane,
                    // Markers no prompt prints. `HEAD` and `TAIL` were the
                    // obvious choice and the wrong one: `drag` takes the LAST
                    // occurrence on screen, and a prompt showing a detached
                    // HEAD — which is every prompt during a rebase — supplies
                    // one below the block, so the drag copied the gap between
                    // the two instead of the fold.
                    "printf 'FOLDSTART'; printf '%s' $(seq 100 199); printf 'FOLDEND\\n'",
                ) == 0
                {
                    return;
                }
                await_prompt(&mut harness, pane);

                let copied = drag(&mut harness, pane, "FOLDSTART", "FOLDEND")
                    .expect("the folded line is on screen");
                assert_eq!(long, copied, "the fold came back as a break in the text");

                // And the block's own copy control says the same thing. Two
                // ways of copying one block that disagree about where its
                // lines end is a wrapped path pasted as three commands.
                let Some((clipboard, _system)) = working_clipboard(&harness) else {
                    return;
                };
                let panel = panel_of_the_pane(&mut harness);
                let scene = hover_block(&mut harness, pane, 0);
                let controls = block_controls(&scene, panel);
                assert_eq!(controls.len(), 2, "the controls, on the hovered block");
                harness.click(center(controls[0]), MouseButton::Left);

                let whole = clipboard.read().unwrap_or_default();
                assert!(
                    whole.contains(&long),
                    "the control broke the folded line the drag ran on: {whole:?}"
                );
            }

            #[test]
            fn a_selection_goes_when_the_pane_falls_back_to_the_grid() {
                // A command printing past the top of the viewport takes the
                // pane from the list to the grid, and the two number their
                // rows differently — a block from its own first row, a grid
                // from the oldest line of the scrollback. Reading the anchors
                // against the surface that did not make them copies text
                // nobody selected; keeping them while the list is not on
                // screen leaves `ctrl-c` claimed by a highlight nobody can
                // see. So the pane lets go.
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if run(&mut harness, pane, "echo MARKER") == 0 {
                    return;
                }
                await_prompt(&mut harness, pane);
                assert_eq!(
                    Some("MARKER".to_owned()),
                    drag(&mut harness, pane, "MARKER", "MARKER")
                );

                // Written straight to the pty: typing into the composer is one
                // of the rules that lets go of a selection by itself, and what
                // is under test is the surface changing.
                harness.type_into(pane, "seq 1 400; sleep 30\n");
                // Four hundred lines into a pane forty rows tall: the block
                // this command opened starts a long way above the viewport,
                // which is what `pane_surface::of` falls back to the grid on.
                // For a number only the *output* can contain: the command line
                // itself is echoed on screen, so waiting for "400" would match
                // the moment the shell drew what was typed at it.
                harness.wait_for("the command never overflowed the viewport", |harness| {
                    harness.terminal_text(pane).contains("399")
                });
                harness.frame();

                assert_eq!(
                    None,
                    harness.terminal_selection(pane),
                    "a selection the pane is no longer drawing is still being copied"
                );

                // Which is the whole point: the interrupt is not spent on it.
                let ctrl = Modifiers {
                    ctrl: true,
                    ..Default::default()
                };
                harness.press("c", ctrl, "c");
                harness.type_into(pane, "echo AFTER\n");
                harness.wait_for("the ctrl-c never reached the shell", |harness| {
                    harness
                        .workspace
                        .read(&harness.app, |workspace, app| {
                            let blocks = workspace.terminal_blocks(pane, app)?;
                            Some(
                                blocks
                                    .iter()
                                    .any(|block| block.rows.to_text().contains("AFTER")),
                            )
                        })
                        .unwrap_or_default()
                });
            }

            /// The rectangles the highlight put in one pane, as they were
            /// asked for rather than as the clip left them.
            ///
            /// Deliberately unclipped: a highlight that runs past the pane is
            /// exactly what is under test below, and the clip would hide it.
            fn highlight(scene: &Scene, panel: RectF) -> Vec<RectF> {
                scene
                    .layers()
                    .flat_map(|layer| layer.rects.iter())
                    .filter(|rect| rect.background == Fill::Solid(crate::theme::theme().selection))
                    .map(|rect| rect.bounds)
                    .filter(|bounds| panel.contains_point(bounds.origin()))
                    .collect()
            }

            #[test]
            fn an_alt_drag_across_two_blocks_bridges_as_the_column_it_is() {
                // The highlight bridges the padding between two blocks so that
                // a selection running through it reads as one selection. A
                // rectangle runs through it as a column — a full-width bar
                // across the gap would be the one part of an alt-drag that
                // took whole rows.
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if run(&mut harness, pane, "echo ALPHAALPHA") == 0 {
                    return;
                }
                run(&mut harness, pane, "echo BRAVOBRAVO");
                await_prompt(&mut harness, pane);

                let scene = harness.frame();
                let start = aim(
                    painted(&scene, "ALPHAALPHA")
                        .expect("the first block is on screen")
                        .0,
                    0.1,
                );
                let end = aim(
                    painted(&scene, "BRAVOBRAVO")
                        .expect("the second block is on screen")
                        .0,
                    0.9,
                );
                harness.hold_alt(start, 1);
                harness.drag_to(end);
                harness.let_go(end);

                let panel = panel_of_the_pane(&mut harness);
                let cell = CellFont::headless(CELL_FONT_SIZE).metrics().width;
                let bands = highlight(&harness.frame(), panel);
                assert!(!bands.is_empty(), "the rectangle painted nothing");
                for band in bands {
                    assert!(
                        band.width() <= cell * 2.,
                        "a one-column rectangle painted a {}px band across the gap",
                        band.width()
                    );
                }
            }

            #[test]
            fn a_highlight_stops_where_the_pane_stops_drawing_the_row() {
                // A finished block keeps the width it was harvested at, so a
                // pane narrowed since holds rows it cannot draw. The
                // highlight is the picture, so it stops with the glyphs; the
                // copy still takes the whole row, because those cells are the
                // block's text.
                let mut harness = Harness::panel(1);
                let Some(pane) = marked_shell(&mut harness) else {
                    return;
                };
                harness.frame();
                if run(&mut harness, pane, "printf 'ONEONE\\nTWOTWO\\n'") == 0 {
                    return;
                }
                await_prompt(&mut harness, pane);

                harness.dispatch_action(TabAction::Split(Direction::Right));
                harness.frame();
                let panel = panel_of_the_pane(&mut harness);
                assert_eq!(
                    Some("ONEONE\nTWOTWO".to_owned()),
                    drag(&mut harness, pane, "ONEONE", "TWOTWO"),
                    "the block harvested at the old width is still selectable"
                );

                for band in highlight(&harness.frame(), panel) {
                    assert!(
                        band.max_x() <= panel.max_x() + 0.5,
                        "the highlight runs {}px past the pane it is in",
                        band.max_x() - panel.max_x()
                    );
                }
            }
        }
    }

    /// The find bar over a pane's output: what it counts, where it steps, and
    /// what it does to the keyboard.
    mod find {
        use super::*;
        use crate::tab::AgentStatus;

        /// Opens the bar and leaves `query` in it, on a marked shell that has
        /// printed `line` as one finished command. Returns the pane, or `None`
        /// on a machine where no shell could be started.
        fn searching(harness: &mut Harness, line: &str, query: &str) -> Option<PaneId> {
            let pane = marked_shell(harness)?;
            await_prompt(harness, pane);
            harness.type_into(pane, &format!("printf '{line}'; echo\n"));
            // Wait for the command to become a finished block, which is what
            // the find walks — not for the output on the grid, which is
            // harvested off it the moment the prompt comes back. `await_prompt`
            // will not do: the shell is already at one when this starts, so it
            // would return before the command had run at all.
            harness.wait_for("the command never became a block", |harness| {
                harness.workspace.read(&harness.app, |workspace, app| {
                    workspace.terminal_blocks(pane, app).is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            block
                                .command
                                .as_deref()
                                .is_some_and(|command| command.contains(line))
                        })
                    })
                })
            });

            harness.run_command("crook/window/find");
            let find = harness
                .workspace
                .read(&harness.app, |workspace, _| workspace.find(pane).cloned())
                .expect("the pane has a find bar");
            assert!(find.is_open(), "the chord opened it");
            find.input().edit(|editor| editor.set_text(query));
            Some(pane)
        }

        fn matches(harness: &Harness, pane: PaneId) -> usize {
            harness.workspace.read(&harness.app, |workspace, app| {
                workspace.find_matches(pane, app).len()
            })
        }

        fn current(harness: &Harness, pane: PaneId) -> Option<usize> {
            harness.workspace.read(&harness.app, |workspace, app| {
                let total = workspace.find_matches(pane, app).len();
                workspace.find(pane).and_then(|find| find.clamped(total))
            })
        }

        fn step(harness: &mut Harness, pane: PaneId, forward: bool) {
            harness.dispatch_workspace_action(WorkspaceAction::Find {
                pane,
                action: crate::workspace::action::FindAction::Step { forward },
            });
        }

        #[test]
        fn it_counts_the_matches_and_steps_round_them() {
            let mut harness = Harness::panel(1);
            // `beta` is in the echoed command line once and in the output
            // once: two matches, and a query nothing has to guess about.
            let Some(pane) = searching(&mut harness, "alpha beta gamma", "beta") else {
                return;
            };

            assert_eq!(2, matches(&harness, pane));
            assert_eq!(Some(0), current(&harness, pane));

            let scene = harness.frame();
            assert!(
                frame_text(&scene).contains("1/2"),
                "the bar counts the current match out of the total"
            );

            step(&mut harness, pane, true);
            assert_eq!(Some(1), current(&harness, pane));
            step(&mut harness, pane, true);
            assert_eq!(
                Some(0),
                current(&harness, pane),
                "forward wraps round the end"
            );
            step(&mut harness, pane, false);
            assert_eq!(Some(1), current(&harness, pane), "back wraps the other way");
        }

        #[test]
        fn a_query_that_matches_nothing_says_so() {
            let mut harness = Harness::panel(1);
            let Some(pane) = searching(&mut harness, "alpha beta gamma", "nowhere") else {
                return;
            };

            assert_eq!(0, matches(&harness, pane));
            assert_eq!(None, current(&harness, pane));
            let scene = harness.frame();
            assert!(
                frame_text(&scene).contains("results"),
                "an open bar with a query nothing matches says so"
            );
        }

        #[test]
        fn the_bar_takes_the_keyboard_and_gives_it_back() {
            let mut harness = Harness::panel(1);
            let Some(pane) = searching(&mut harness, "alpha beta gamma", "beta") else {
                return;
            };

            let field_has_keys = harness.workspace.read(&harness.app, |workspace, _| {
                workspace.find(pane).unwrap().input().has_keys()
            });
            assert!(field_has_keys, "the open bar is where the keyboard is");
            assert!(!harness.pane_takes_keys(), "so the shell under it is not");

            harness.dispatch_workspace_action(WorkspaceAction::Find {
                pane,
                action: crate::workspace::action::FindAction::Close,
            });
            let still_open = harness.workspace.read(&harness.app, |workspace, _| {
                workspace.find(pane).unwrap().is_open()
            });
            assert!(!still_open, "Escape closed it");
            assert!(
                harness.pane_takes_keys(),
                "and the shell has the keyboard back"
            );
        }

        #[test]
        fn a_full_screen_program_has_no_find_bar() {
            let mut harness = Harness::panel(1);
            let Some(pane) = marked_shell(&mut harness) else {
                return;
            };
            await_prompt(&mut harness, pane);
            // The alternate screen: one grid, not a list of commands. The
            // doubled backslash is the shell's: its printf is what turns \033
            // into an ESC, so what is typed at it is a backslash and not an
            // escape this Rust string already resolved.
            harness.type_into(pane, "printf '\\033[?1049h'\n");
            harness.wait_for("the alternate screen never came up", |harness| {
                harness.alt_screen(pane)
            });

            harness.run_command("crook/window/find");
            let opened = harness.workspace.read(&harness.app, |workspace, _| {
                workspace.find(pane).unwrap().is_open()
            });
            assert!(!opened, "Ctrl-F is the program's key, not a find bar's");
            // Silence the unused import on machines with no shell.
            let _ = AgentStatus::Idle;
        }
    }

    /// Stepping through the finished blocks with the keyboard.
    mod block_keyboard {
        use super::*;
        use crook_terminal::BlockId;

        /// Runs three echo commands and returns the pane with three finished
        /// blocks behind it, or `None` where no shell could be started.
        fn three_commands(harness: &mut Harness) -> Option<PaneId> {
            let pane = marked_shell(harness)?;
            await_prompt(harness, pane);
            for word in ["first", "second", "third"] {
                harness.type_into(pane, &format!("echo {word}\n"));
                harness.wait_for("a command never became a block", |harness| {
                    harness.workspace.read(&harness.app, |workspace, app| {
                        workspace.terminal_blocks(pane, app).is_some_and(|blocks| {
                            blocks.iter().any(|block| {
                                block.command.as_deref() == Some(&format!("echo {word}"))
                            })
                        })
                    })
                });
            }
            Some(pane)
        }

        fn ids(harness: &Harness, pane: PaneId) -> Vec<BlockId> {
            harness.workspace.read(&harness.app, |workspace, app| {
                workspace
                    .terminal_blocks(pane, app)
                    .map(|blocks| blocks.iter().map(|block| block.id).collect())
                    .unwrap_or_default()
            })
        }

        fn selected(harness: &Harness, pane: PaneId) -> Option<BlockId> {
            harness.workspace.read(&harness.app, |workspace, _| {
                workspace.pane_blocks(pane).and_then(|view| view.selected())
            })
        }

        /// Whether the list is on its own end rather than scrolled off it.
        fn is_following(harness: &Harness, pane: PaneId) -> bool {
            harness.workspace.read(&harness.app, |workspace, _| {
                workspace
                    .pane_blocks(pane)
                    .is_some_and(|view| !view.is_cut_off())
            })
        }

        fn up(harness: &mut Harness) {
            harness.run_command("crook/window/select-block-up");
        }
        fn down(harness: &mut Harness) {
            harness.run_command("crook/window/select-block-down");
        }

        #[test]
        fn stepping_up_walks_the_blocks_from_the_prompt_and_stops_at_the_top() {
            let mut harness = Harness::panel(1);
            let Some(pane) = three_commands(&mut harness) else {
                return;
            };
            let blocks = ids(&harness, pane);
            assert!(blocks.len() >= 3, "the three commands each left a block");
            let top = blocks[0];
            let last = blocks[blocks.len() - 1];
            let second_last = blocks[blocks.len() - 2];
            assert_eq!(
                None,
                selected(&harness, pane),
                "the keyboard starts at the prompt"
            );

            up(&mut harness);
            assert_eq!(
                Some(last),
                selected(&harness, pane),
                "up from the prompt is the last block"
            );
            up(&mut harness);
            assert_eq!(Some(second_last), selected(&harness, pane));
            // All the way to the top, however many blocks the shell's own
            // start-up left in front of the three.
            for _ in 0..blocks.len() {
                up(&mut harness);
            }
            assert_eq!(
                Some(top),
                selected(&harness, pane),
                "the top is as far as up goes"
            );
        }

        #[test]
        fn stepping_down_returns_to_the_prompt_and_stays_there() {
            let mut harness = Harness::panel(1);
            let Some(pane) = three_commands(&mut harness) else {
                return;
            };
            let blocks = ids(&harness, pane);
            let last = blocks[blocks.len() - 1];
            let second_last = blocks[blocks.len() - 2];

            up(&mut harness); // last
            up(&mut harness); // second last
            assert_eq!(Some(second_last), selected(&harness, pane));

            down(&mut harness);
            assert_eq!(Some(last), selected(&harness, pane));
            down(&mut harness);
            assert_eq!(
                None,
                selected(&harness, pane),
                "down off the last block is the prompt"
            );
            down(&mut harness);
            assert_eq!(None, selected(&harness, pane), "and it stays there");
        }

        #[test]
        fn escape_and_copy_are_the_selections_only_when_there_is_one() {
            let mut harness = Harness::panel(1);
            let Some(pane) = three_commands(&mut harness) else {
                return;
            };

            // With nothing selected, Escape and the copy chord are nobody's
            // here — they fall through to the pane.
            assert_eq!(None, harness.action_for("escape", Modifiers::default()));
            let copy = Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            };
            assert_eq!(None, harness.action_for("c", copy));

            up(&mut harness);
            assert!(selected(&harness, pane).is_some());

            // Now Escape clears it and the copy chord copies it.
            assert!(matches!(
                harness.action_for("escape", Modifiers::default()),
                Some(WorkspaceAction::Block(
                    crate::workspace::action::BlockAction::ClearSelection(_)
                ))
            ));
            assert!(matches!(
                harness.action_for("c", copy),
                Some(WorkspaceAction::Block(
                    crate::workspace::action::BlockAction::CopySelection(_)
                ))
            ));

            harness.dispatch_workspace_action(WorkspaceAction::Block(
                crate::workspace::action::BlockAction::ClearSelection(pane),
            ));
            assert_eq!(None, selected(&harness, pane), "Escape let go of it");
        }
        #[test]
        fn a_block_command_acts_on_the_selected_block_with_no_menu_open() {
            // The whole of why `block_target` exists. Every entry of the block
            // menu used to read what it needed off the menu's own cache, so a
            // chord for one of them was a chord that did nothing until a
            // pointer had opened the menu first.
            let mut harness = Harness::panel(1);
            let Some(pane) = three_commands(&mut harness) else {
                return;
            };
            up(&mut harness);
            let block = selected(&harness, pane).expect("a block is selected");
            assert!(
                harness
                    .workspace
                    .read(&harness.app, |workspace, _| !workspace
                        .block_menu()
                        .is_open()),
                "no menu should be open"
            );

            let command = harness.workspace.read(&harness.app, |workspace, app| {
                workspace
                    .terminal_blocks(pane, app)
                    .and_then(|blocks| {
                        blocks
                            .iter()
                            .find(|finished| finished.id == block)
                            .and_then(|finished| finished.command.clone())
                    })
                    .unwrap_or_default()
            });
            assert!(!command.is_empty(), "the block has no command line");

            harness.run_command("crook/window/rerun-block");

            assert!(
                harness.field_text(pane).contains(&command),
                "the composer holds {:?} rather than {command:?}",
                harness.field_text(pane)
            );
        }

        #[test]
        fn paging_moves_the_list_and_the_ends_come_back_to_following_it() {
            let mut harness = Harness::panel(1);
            let Some(pane) = three_commands(&mut harness) else {
                return;
            };
            // A window short enough that three commands do not fit in it, so
            // there is something for a page to move. Measured by drawing:
            // `PaneBlocks` learns its viewport and its content from layout,
            // and a list nothing has laid out has nothing to scroll.
            harness.frame_sized(vec2f(1024., 220.));

            let list = |harness: &Harness| {
                harness.workspace.read(&harness.app, |workspace, _| {
                    workspace
                        .pane_blocks(pane)
                        .map(|view| (view.offset(), view.viewport(), view.is_scrollable()))
                })
            };
            let Some((bottom, viewport, scrollable)) = list(&harness) else {
                return;
            };
            if !scrollable {
                eprintln!("skipped: three echoes still fit in a short window");
                return;
            }

            harness.run_command("crook/window/page-up");
            let paged = list(&harness).expect("a list").0;
            // A screenful less the overlap, or the whole of what there is to
            // move — the clamp is half of what the arithmetic has to get
            // right, and a short scrollback is the ordinary case for three
            // echo commands.
            let expected = (viewport - crate::pane_blocks::PAGE_OVERLAP).min(bottom);
            assert!(
                (bottom - paged - expected).abs() < 0.01,
                "a page moved {} lines of a {viewport}-line box with {bottom} to move",
                bottom - paged
            );

            harness.run_command("crook/window/page-down");
            assert!(
                is_following(&harness, pane),
                "paging back onto the end did not start following it again"
            );

            harness.run_command("crook/window/scroll-to-top");
            assert_eq!(list(&harness).expect("a list").0, 0.);

            harness.run_command("crook/window/scroll-to-bottom");
            assert!(
                is_following(&harness, pane),
                "scroll-to-bottom left the list off its own end"
            );
        }
    }
}

#[test]
fn clicking_create_writes_the_theme() {
    // The button, clicked — not the action, dispatched. Every control in the
    // panel is wrapped in a `Hoverable`, a `Hoverable` claims the press it
    // sees whether or not it has a click handler, and a flex hands every event
    // to every child: a preview card that shared the button's mouse state
    // swallowed the press and the button never fired. A test that dispatches
    // the action cannot see that, which is why this one aims at pixels.
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    harness.start_creating();
    let scene = harness.frame();

    let buttons = creator_buttons(&scene);
    assert_eq!(buttons.len(), 2, "Cancel and Create");
    harness.click(center(buttons[1]), MouseButton::Left);

    assert!(
        !harness.is_creating(),
        "clicking Create left the creator open"
    );
    assert_eq!(
        fs::read_dir(themes.path())
            .expect("readable")
            .flatten()
            .count(),
        1,
        "clicking Create wrote no theme"
    );
    assert_eq!(harness.theme_name(), "Crook Dark variant");
}

#[test]
fn clicking_cancel_puts_the_theme_back_and_writes_nothing() {
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    harness.start_creating();
    let buttons = creator_buttons(&harness.frame());
    harness.click(center(buttons[0]), MouseButton::Left);

    assert!(!harness.is_creating());
    assert_eq!(crate::theme::theme(), crate::theme::DARK);
    assert_eq!(
        fs::read_dir(themes.path())
            .expect("readable")
            .flatten()
            .count(),
        0
    );
}

#[test]
fn the_arrow_keys_do_nothing_while_the_creator_is_up() {
    // The list is not on screen, so a key that quietly chose *and saved* a
    // theme behind the creator would leave the settings file naming a theme
    // the window is not in — and cancelling would then restore a third one.
    let scratch = Scratch::new();
    let themes = Scratch::new();
    let mut harness = Harness::with_settings(1, scratch.settings());
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    harness.start_creating();
    let draft = crate::theme::theme();

    assert!(harness.press_key("down", Modifiers::default()));
    assert_eq!(
        crate::theme::theme(),
        draft,
        "an arrow key changed the theme from behind the creator"
    );

    harness.cancel_creating();
    assert_eq!(crate::theme::theme(), crate::theme::DARK);
}

#[test]
fn escape_while_the_creator_is_up_puts_the_theme_back() {
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    harness.start_creating();
    assert_ne!(crate::theme::theme(), crate::theme::DARK);

    assert!(harness.press_key("escape", Modifiers::default()));

    assert!(!harness.is_theme_panel_open());
    assert!(!harness.is_creating());
    assert_eq!(
        crate::theme::theme(),
        crate::theme::DARK,
        "escape left the unsaved draft painted on the window"
    );
}

#[test]
fn re_opening_the_panel_while_creating_puts_the_theme_back_too() {
    // The settings page's row dispatches `OpenPanel` whether or not the panel
    // is already up, so this is one click away at any moment.
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    harness.start_creating();
    harness.open_theme_panel();

    assert!(!harness.is_creating());
    assert_eq!(
        crate::theme::theme(),
        crate::theme::DARK,
        "the abandoned draft is still on screen"
    );
}

#[test]
fn the_panel_opens_on_the_theme_that_is_on_screen() {
    // Warp's chooser opens on the theme you are in. Opening at the top of the
    // list would move a person's theme the first time they pressed Down.
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard =
        crate::theme::ThemeGuard::new(crate::theme::named("Midnight").expect("a built-in"));
    harness.set_themes_directory(themes.path().to_owned());

    harness.open_theme_panel();
    assert_eq!(harness.selected_theme_name(), "Midnight");

    // And one press moves to the row beside it, not to the second row of the
    // list.
    assert!(harness.press_key("down", Modifiers::default()));
    assert_eq!(harness.theme_names()[3], harness.theme_name());
}

#[test]
fn the_row_height_the_panel_scrolls_by_is_the_height_it_draws() {
    // Scrolling to a row is arithmetic — the selected row's index times a
    // constant — so a constant that drifted from what is drawn would scroll to
    // the wrong place rather than fail. This is what pins it.
    let themes = Scratch::new();
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.set_themes_directory(themes.path().to_owned());
    harness.open_theme_panel();

    let cards = theme_cards(&harness.frame());
    assert!(cards.len() >= 2, "two cards are needed to measure the gap");

    let drawn = cards[1].min_y() - cards[0].min_y();
    assert!(
        (drawn - crate::workspace::theme_panel::ROW_HEIGHT).abs() < 0.5,
        "rows are drawn {drawn} apart and scrolled by {}",
        crate::workspace::theme_panel::ROW_HEIGHT
    );
}

/// The tabs panel scrolling to the row a selection landed on.
mod panel_autoscroll {
    use super::*;

    /// How far the panel's list is scrolled.
    fn offset(harness: &Harness) -> f32 {
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace.panel_scroll().lock().offset()
        })
    }

    /// A panel with enough tabs that the list is longer than the window.
    fn crowded() -> Harness {
        let mut harness = Harness::panel(1);
        for _ in 0..30 {
            harness.dispatch_action(TabAction::New);
        }
        harness.frame();
        harness
    }

    #[test]
    fn selecting_a_tab_off_the_bottom_brings_its_row_into_view() {
        // Without this, the keyboard moves the selection to a row nobody can
        // see and the panel looks as though the chord did nothing.
        let mut harness = crowded();
        let first = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tabs().iter().next().expect("a first tab").id()
        });

        harness.dispatch_action(TabAction::Select(first));
        harness.frame();
        assert_eq!(offset(&harness), 0., "the first row is at the top");

        let last = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tabs().iter().last().expect("a last tab").id()
        });
        harness.dispatch_action(TabAction::Select(last));
        harness.frame();

        assert!(
            offset(&harness) > 0.,
            "the last row was selected and the list never moved"
        );
    }

    #[test]
    fn a_row_already_in_view_does_not_move_the_list() {
        // A selection that scrolled every time would fight the wheel: reading
        // down the list and clicking what you find would jump it.
        let mut harness = crowded();
        let last = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tabs().iter().last().expect("a last tab").id()
        });
        harness.dispatch_action(TabAction::Select(last));
        harness.frame();
        let settled = offset(&harness);

        // The same tab again, and then a frame: nothing has moved.
        harness.dispatch_action(TabAction::Select(last));
        harness.frame();

        assert_eq!(offset(&harness), settled);
    }
}

/// Restoring a window: the strip comes back, and everything the workspace
/// keeps beside it comes back into step.
mod restoring {
    use super::*;
    use crate::session::Session;

    #[test]
    fn a_restored_strip_brings_the_per_pane_state_with_it() {
        // The whole risk of replacing the strip wholesale: mouse states, drag
        // gestures, scroll offsets and input fields are all keyed by pane id,
        // and every id in a restored strip is new. Nothing in `restore` knows
        // what that state is — `sync_interactions` does — so this is what
        // proves the seam holds.
        let mut source = Harness::new(1);
        source.dispatch_action(TabAction::Split(Direction::Right));
        source.dispatch_action(TabAction::New);
        let session = source.workspace.read(&source.app, |workspace, _| {
            Session::of(workspace.tabs(), None)
        });

        let mut harness = Harness::new(1);
        let strip = session.restore().expect("there was something to restore");
        harness.workspace_update(|workspace, ctx| workspace.restore(strip, ctx));

        let panes = harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .tabs()
                .panes()
                .map(|(_, pane)| pane.id())
                .collect::<Vec<_>>()
        });
        assert_eq!(panes.len(), 3, "two tabs, the first split in two");

        for pane in panes {
            assert!(
                harness
                    .workspace
                    .read(&harness.app, |workspace, _| workspace
                        .interaction(pane)
                        .is_some()),
                "{pane:?} came back with no mouse state"
            );
        }

        // And the frame draws, which is the other half of the same claim.
        harness.frame();
    }

    #[test]
    fn the_pane_that_had_the_keyboard_has_it_again() {
        let mut source = Harness::new(1);
        source.dispatch_action(TabAction::Split(Direction::Right));
        let first = source.workspace.read(&source.app, |workspace, _| {
            workspace
                .tabs()
                .panes()
                .map(|(_, pane)| pane.id())
                .next()
                .expect("a first pane")
        });
        source.dispatch_action(TabAction::FocusPane(first));

        let session = source.workspace.read(&source.app, |workspace, _| {
            Session::of(workspace.tabs(), None)
        });

        let mut harness = Harness::new(1);
        let strip = session.restore().expect("restored");
        harness.workspace_update(|workspace, ctx| workspace.restore(strip, ctx));
        harness.frame();

        let focused = harness.focused_pane_id().expect("a focused pane");
        let panes = harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .tabs()
                .panes()
                .map(|(_, pane)| pane.id())
                .collect::<Vec<_>>()
        });
        assert_eq!(Some(focused), panes.first().copied());
        assert!(
            harness.pane_takes_keys(),
            "the restored pane's field was never told it has the keyboard"
        );
    }
}

/// Following the desktop's light or dark setting.
mod system_theme {
    use super::*;

    /// A harness on settings that know where they would be written, so the
    /// pair is really read back rather than kept in memory.
    fn harness(themes: &Scratch, settings: &Scratch) -> Harness {
        let mut harness = Harness::with_settings(1, settings.settings());
        harness.set_themes_directory(themes.path().to_owned());
        harness
    }

    /// Chooses a theme by name, the way a row of the panel does.
    fn choose(harness: &mut Harness, name: &str) {
        let name = name.to_owned();
        harness.workspace_update(|workspace, ctx| workspace.set_theme(&name, ctx));
    }

    fn follow(harness: &mut Harness, on: bool) {
        harness.workspace_update(|workspace, ctx| workspace.set_follow_system_theme(on, ctx));
    }

    fn desktop_is_dark(harness: &mut Harness, dark: bool) {
        harness.workspace_update(|workspace, ctx| workspace.set_system_dark(dark, ctx));
    }

    #[test]
    fn the_desktop_is_ignored_until_it_is_being_followed() {
        // A terminal whose chosen theme changed colour at sunset without being
        // asked would be a surprise, which is why the flag is off by default.
        let themes = Scratch::new();
        let files = Scratch::new();
        let mut harness = harness(&themes, &files);
        let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);

        choose(&mut harness, "Midnight");
        desktop_is_dark(&mut harness, false);

        assert_eq!(harness.theme_name(), "Midnight");
    }

    #[test]
    fn turning_it_on_applies_the_half_the_desktop_is_in() {
        // A switch that changed nothing until the next sunset would look
        // broken.
        let themes = Scratch::new();
        let files = Scratch::new();
        let mut harness = harness(&themes, &files);
        let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);

        desktop_is_dark(&mut harness, false);
        follow(&mut harness, true);

        assert_eq!(harness.theme_name(), crate::theme::DEFAULT_LIGHT_NAME);
    }

    #[test]
    fn the_desktop_moving_moves_the_theme_with_it() {
        let themes = Scratch::new();
        let files = Scratch::new();
        let mut harness = harness(&themes, &files);
        let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);

        follow(&mut harness, true);
        desktop_is_dark(&mut harness, false);
        assert_eq!(harness.theme_name(), crate::theme::DEFAULT_LIGHT_NAME);

        desktop_is_dark(&mut harness, true);
        assert_eq!(harness.theme_name(), crate::theme::DEFAULT_NAME);
    }

    #[test]
    fn choosing_a_theme_while_following_sets_only_the_half_in_force() {
        // The other half is somebody's choice for the other half, and
        // replacing it would silently throw it away.
        let themes = Scratch::new();
        let files = Scratch::new();
        let mut harness = harness(&themes, &files);
        let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);

        follow(&mut harness, true);
        desktop_is_dark(&mut harness, true);
        choose(&mut harness, "Midnight");

        let (light, dark) = harness.workspace.read(&harness.app, |workspace, _| {
            (
                workspace.settings().light_theme().to_owned(),
                workspace.settings().dark_theme().to_owned(),
            )
        });
        assert_eq!(dark, "Midnight");
        assert_eq!(
            light,
            crate::theme::DEFAULT_LIGHT_NAME,
            "the light half was left as it was"
        );

        // And going light comes back to it rather than to Midnight.
        desktop_is_dark(&mut harness, false);
        assert_eq!(harness.theme_name(), crate::theme::DEFAULT_LIGHT_NAME);
    }
}

/// Zooming: one number that every measurement in the window comes from.
mod text_size {
    use super::*;
    use crate::settings::{DEFAULT_FONT_SIZE, FONT_SIZE_STEP, MAX_FONT_SIZE};

    fn size(harness: &Harness) -> f32 {
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace.cell_font().font_size()
        })
    }

    /// The chord, sent the way the window delegate sends a bound keystroke.
    ///
    /// [`settings_chord`] rather than [`platform_chord`], because the zoom
    /// chords are the same exception the settings chord is: they carry no
    /// Shift. Off macOS the Shift is don't-care on `=` and `-` — it is what
    /// reaches the `+` and `_` printed on those keys — but *not* on `0`, where
    /// Shift is `)` and nothing is bound. A helper that sent Shift could only
    /// ever test two of the three, which is what it was doing.
    fn zoom(harness: &mut Harness, key: &str) -> bool {
        harness.press_key(key, settings_chord())
    }

    #[test]
    fn the_chords_change_the_font_every_grid_is_measured_with() {
        let mut harness = Harness::new(1);
        assert_eq!(size(&harness), DEFAULT_FONT_SIZE);

        assert!(zoom(&mut harness, "="));
        assert_eq!(size(&harness), DEFAULT_FONT_SIZE + FONT_SIZE_STEP);

        assert!(zoom(&mut harness, "-"));
        assert_eq!(size(&harness), DEFAULT_FONT_SIZE);
    }

    #[test]
    fn a_bigger_cell_is_a_wider_cell() {
        // The point of the whole feature: the cell is what a pane's columns
        // and rows are its box divided by, so a pty resizes because the font
        // did and nothing has to tell it.
        let mut harness = Harness::new(1);
        let before = harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.cell_font().metrics());

        assert!(zoom(&mut harness, "="));
        let after = harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.cell_font().metrics());

        assert!(after.width > before.width, "{after:?} vs {before:?}");
        assert!(after.height >= before.height);
    }

    #[test]
    fn the_reset_chord_goes_back_to_the_size_a_fresh_install_opens_at() {
        let mut harness = Harness::new(1);
        for _ in 0..4 {
            zoom(&mut harness, "=");
        }
        assert_ne!(size(&harness), DEFAULT_FONT_SIZE);

        assert!(zoom(&mut harness, "0"));
        assert_eq!(size(&harness), DEFAULT_FONT_SIZE);
    }

    #[test]
    fn holding_the_chord_down_stops_at_the_end_of_the_range() {
        // Key repeat reaches the end of the range in a second, so the end has
        // to be an answer rather than an accident — and the frame that changed
        // nothing must not repaint.
        let mut harness = Harness::new(1);
        for _ in 0..200 {
            zoom(&mut harness, "=");
        }

        assert_eq!(size(&harness), MAX_FONT_SIZE);
        harness.frame();
    }
}

/// The bell: what a shell asks for that only the tab strip can answer.
///
/// Driven through `apply_terminal_update`, which is the exact call the
/// subscription in `Workspace::new` makes when a session's own thread reports
/// something.
mod the_bell {
    use super::*;
    use crate::terminal_model::TerminalUpdate;

    /// Applies one update the way the model's subscription does.
    fn report(harness: &mut Harness, update: TerminalUpdate) {
        harness.workspace_update(|workspace, ctx| {
            workspace.apply_terminal_update(&update, ctx);
        });
    }

    fn status_of(harness: &Harness, pane: PaneId) -> Option<AgentStatus> {
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tabs().pane(pane).map(Pane::status)
        })
    }

    /// The pane of the tab that is *not* active.
    fn background_of(harness: &Harness) -> PaneId {
        let active = harness.focused_pane_id().expect("the window has a pane");
        harness
            .workspace
            .read(&harness.app, |workspace, _| {
                workspace
                    .tabs()
                    .panes()
                    .map(|(_, pane)| pane.id())
                    .find(|id| *id != active)
            })
            .expect("two tabs have two panes")
    }

    #[test]
    fn a_bell_in_a_pane_nobody_is_looking_at_asks_for_attention() {
        let mut harness = Harness::new(2);
        let ringing = background_of(&harness);

        assert_eq!(status_of(&harness, ringing), Some(AgentStatus::Idle));
        report(
            &mut harness,
            TerminalUpdate::Bell {
                pane: ringing,
                while_running: false,
            },
        );

        assert_eq!(
            status_of(&harness, ringing),
            Some(AgentStatus::NeedsInput),
            "a bell in a background pane is the one thing that says look here"
        );
    }

    #[test]
    fn a_bell_in_the_pane_with_the_keyboard_is_not_an_interruption() {
        // Bash rings this on an ambiguous Tab completion. Painting somebody's
        // own row amber while they type in it would be worse than silence.
        let mut harness = Harness::new(1);
        let focused = harness.focused_pane_id().expect("the window has a pane");

        report(
            &mut harness,
            TerminalUpdate::Bell {
                pane: focused,
                while_running: false,
            },
        );

        assert_eq!(status_of(&harness, focused), Some(AgentStatus::Idle));
    }

    #[test]
    fn looking_at_a_pane_is_what_quiets_it() {
        let mut harness = Harness::new(2);
        let ringing = background_of(&harness);

        report(
            &mut harness,
            TerminalUpdate::Bell {
                pane: ringing,
                while_running: false,
            },
        );
        assert_eq!(status_of(&harness, ringing), Some(AgentStatus::NeedsInput));

        harness.dispatch_action(TabAction::FocusPane(ringing));

        assert_eq!(
            status_of(&harness, ringing),
            Some(AgentStatus::Idle),
            "the bell was answered by looking at the pane that rang"
        );
    }

    #[test]
    fn a_failed_pane_is_not_quieted_by_being_looked_at() {
        // Only the status a bell sets is cleared by attention. Failure is a
        // fact about the work, and looking at it does not undo it.
        let mut harness = Harness::new(2);
        let failed = background_of(&harness);
        harness.workspace_update(|workspace, ctx| {
            workspace.update_session(failed, ctx, |session| {
                session.status = AgentStatus::Failed;
            });
        });

        harness.dispatch_action(TabAction::FocusPane(failed));

        assert_eq!(status_of(&harness, failed), Some(AgentStatus::Failed));
    }
}

/// The agent's own report: what a program in a pane says it is doing, and
/// what the strip remembers about whether anybody saw it.
mod the_agent {
    use super::*;
    use crate::terminal_model::TerminalUpdate;

    fn report(harness: &mut Harness, pane: PaneId, status: AgentStatus, title: Option<&str>) {
        let update = TerminalUpdate::Agent {
            pane,
            status,
            title: title.map(str::to_owned),
        };
        harness.workspace_update(|workspace, ctx| {
            workspace.apply_terminal_update(&update, ctx);
        });
    }

    fn session_of(harness: &Harness, pane: PaneId) -> (AgentStatus, bool, Option<String>) {
        harness.workspace.read(&harness.app, |workspace, _| {
            let session = workspace
                .tabs()
                .pane(pane)
                .expect("the pane is open")
                .session();
            (
                session.status,
                session.attention,
                session.derived_title.clone(),
            )
        })
    }

    /// The pane of the tab that is *not* active.
    fn background_of(harness: &Harness) -> PaneId {
        let active = harness.focused_pane_id().expect("the window has a pane");
        harness
            .workspace
            .read(&harness.app, |workspace, _| {
                workspace
                    .tabs()
                    .panes()
                    .map(|(_, pane)| pane.id())
                    .find(|id| *id != active)
            })
            .expect("two tabs have two panes")
    }

    #[test]
    fn what_the_agent_says_is_what_the_row_shows() {
        let mut harness = Harness::new(1);
        let pane = harness.focused_pane_id().expect("the window has a pane");

        report(&mut harness, pane, AgentStatus::Running, None);
        assert_eq!(
            (AgentStatus::Running, false, None),
            session_of(&harness, pane),
            "the pane with the keyboard is being looked at, so nothing asks for a look"
        );

        report(&mut harness, pane, AgentStatus::NeedsInput, None);
        assert_eq!(AgentStatus::NeedsInput, session_of(&harness, pane).0);
    }

    #[test]
    fn a_title_in_the_report_is_the_agents_name_for_its_work() {
        let mut harness = Harness::new(1);
        let pane = harness.focused_pane_id().expect("the window has a pane");

        report(
            &mut harness,
            pane,
            AgentStatus::Running,
            Some("port the tab bar"),
        );

        assert_eq!(
            Some("port the tab bar".to_owned()),
            session_of(&harness, pane).2
        );
        let shown = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tabs().active().map(|tab| tab.title().to_owned())
        });
        assert_eq!(Some("port the tab bar".to_owned()), shown);
    }

    #[test]
    fn an_agent_stopping_where_nobody_is_looking_asks_for_a_look() {
        let mut harness = Harness::new(2);
        let away = background_of(&harness);

        report(&mut harness, away, AgentStatus::Running, None);
        assert_eq!(
            (AgentStatus::Running, false, None),
            session_of(&harness, away),
            "an agent getting on with it is the one change nobody needs to see"
        );

        report(&mut harness, away, AgentStatus::NeedsInput, None);
        assert_eq!(
            (AgentStatus::NeedsInput, true, None),
            session_of(&harness, away)
        );

        harness.dispatch_action(TabAction::FocusPane(away));
        assert_eq!(
            (AgentStatus::NeedsInput, false, None),
            session_of(&harness, away),
            "looking answers the request for a look, and not the agent's question"
        );
    }

    #[test]
    fn an_agent_going_back_to_work_takes_its_request_back() {
        let mut harness = Harness::new(2);
        let away = background_of(&harness);

        report(&mut harness, away, AgentStatus::Failed, None);
        assert!(session_of(&harness, away).1);

        report(&mut harness, away, AgentStatus::Running, None);
        assert_eq!(
            (AgentStatus::Running, false, None),
            session_of(&harness, away)
        );
    }

    #[test]
    fn a_bell_keeps_a_running_agent_running_and_still_asks_for_a_look() {
        // The dot says what the agent said; the ring is remembered beside it.
        let mut harness = Harness::new(2);
        let away = background_of(&harness);
        report(&mut harness, away, AgentStatus::Running, None);

        harness.workspace_update(|workspace, ctx| {
            workspace.apply_terminal_update(
                &TerminalUpdate::Bell {
                    pane: away,
                    while_running: true,
                },
                ctx,
            );
        });

        assert_eq!(
            (AgentStatus::Running, true, None),
            session_of(&harness, away)
        );
        let shown = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tabs().pane(away).map(Pane::status)
        });
        assert_eq!(Some(AgentStatus::Running), shown);
    }

    #[test]
    fn the_chord_goes_round_the_waiting_panes_in_the_panels_order() {
        let mut harness = Harness::new(4);
        let tabs = harness.tab_ids();
        let first = harness.panes_of(tabs[0])[0];
        let third = harness.panes_of(tabs[2])[0];
        assert_eq!(
            harness.active_id(),
            tabs[3],
            "the last tab opened is active"
        );

        harness.run_command("crook/tabs/next-waiting");
        assert_eq!(
            harness.active_id(),
            tabs[3],
            "nothing is waiting, so nothing moves"
        );

        report(&mut harness, third, AgentStatus::NeedsInput, None);
        report(&mut harness, first, AgentStatus::Failed, None);
        let count = harness.workspace.read(&harness.app, |workspace, _| {
            crate::plugins::tabs::waiting_count(workspace.tabs())
        });
        assert_eq!(2, count);

        // Round the end of the list: the first tab comes before the third.
        harness.run_command("crook/tabs/next-waiting");
        assert_eq!(harness.active_id(), tabs[0]);
        assert_eq!(
            (AgentStatus::Failed, false, None),
            session_of(&harness, first),
            "arriving is what answers the request for a look"
        );

        harness.run_command("crook/tabs/next-waiting");
        assert_eq!(harness.active_id(), tabs[2]);

        // The third is still waiting — an agent's question is not answered
        // by a glance — but it is the one being looked at, so the chord has
        // nowhere left to go and stays put.
        harness.run_command("crook/tabs/next-waiting");
        assert_eq!(harness.active_id(), tabs[2]);

        // Leaving it makes it waiting again, from the strip's point of view.
        harness.dispatch_action(TabAction::Select(tabs[3]));
        harness.run_command("crook/tabs/next-waiting");
        assert_eq!(harness.active_id(), tabs[2]);
    }

    #[test]
    fn the_header_counts_the_waiting_panes_and_says_nothing_at_zero() {
        let mut harness = Harness::new(2);
        let away = background_of(&harness);

        let scene = harness.frame();
        assert!(
            !frame_text(&scene).contains(" waiting"),
            "a count of zero is not information"
        );

        report(&mut harness, away, AgentStatus::NeedsInput, None);
        let scene = harness.frame();
        assert!(frame_text(&scene).contains("1 waiting"));

        harness.dispatch_action(TabAction::FocusPane(away));
        let scene = harness.frame();
        assert!(
            !frame_text(&scene).contains(" waiting"),
            "the one waiting pane is the one being looked at"
        );
    }

    #[test]
    fn only_a_pane_nobody_is_looking_at_is_waiting() {
        let mut harness = Harness::new(2);
        let away = background_of(&harness);
        let here = harness.focused_pane_id().expect("the window has a pane");
        report(&mut harness, away, AgentStatus::NeedsInput, None);
        report(&mut harness, here, AgentStatus::NeedsInput, None);

        let waiting = harness.workspace.read(&harness.app, |workspace, _| {
            let waits = |pane: PaneId, active: bool| {
                workspace
                    .tabs()
                    .pane(pane)
                    .unwrap()
                    .session()
                    .is_waiting(active)
            };
            (waits(away, false), waits(here, true))
        });

        assert_eq!((true, false), waiting);
    }
}

// ---------------------------------------------------------------- ADVERSARIAL
/// The title bar's two halves, checked against each other: every control in
/// the header still answers a click, and every gap between them still picks
/// the window up.
#[cfg(test)]
mod title_bar_hit_testing {
    use super::*;

    fn press(harness: &mut Harness, position: Vector2F, count: u32) {
        harness.dispatch(Event::MouseDown {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: count,
        });
    }

    #[test]
    fn every_control_in_the_header_still_answers_a_click() {
        let mut harness = Harness::new(2);

        // The empty space under the list opens the options menu.
        let scene = harness.frame();
        harness.click(empty_list_space(&scene), MouseButton::Right);
        assert!(
            harness.is_menu_open(),
            "the empty space stopped opening the menu"
        );
        harness.dispatch_option(OptionsAction::TogglePopup);

        // The `+` opens a tab.
        let scene = harness.frame();
        let before = tab_boxes(&scene).len();
        harness.click(center(plus_box(&scene)), MouseButton::Left);
        let scene = harness.frame();
        assert_eq!(
            tab_boxes(&scene).len(),
            before + 1,
            "the + stopped opening tabs"
        );

        // A tab focuses its pane.
        let tabs = tab_boxes(&scene);
        harness.click(center(tabs[0]), MouseButton::Left);
        let scene = harness.frame();
        assert_eq!(
            tab_boxes(&scene)
                .iter()
                .position(|bounds| *bounds == tabs[0]),
            Some(0),
            "the first tab moved when it was clicked"
        );

        // And none of the three dragged the window.
        assert!(
            harness.window_requests().is_empty(),
            "a control in the panel dragged the window: {:?}",
            harness.window_requests()
        );
    }

    #[test]
    fn the_controls_and_the_tabs_swallow_a_double_click_rather_than_maximising() {
        for target in ["tab", "plus"] {
            let mut harness = Harness::new(2);
            let scene = harness.frame();
            let at = match target {
                "tab" => center(tab_boxes(&scene)[0]),
                _ => center(plus_box(&scene)),
            };

            harness.click_times(at, 1);
            harness.click_times(at, 2);

            assert!(
                harness.window_requests().is_empty(),
                "double clicking the {target} asked the window for {:?}",
                harness.window_requests()
            );
        }
    }

    #[test]
    fn every_gap_between_the_header_controls_still_picks_the_window_up() {
        // The `+` is the panel's; what is left in this row is whatever a
        // plugin pinned to the right of it, and nothing else — so on a build
        // with no such plugin the whole row is gap, and the points below are
        // read off the row rather than off anything drawn in it.
        let scene = Harness::new(2).frame();
        let panel = panel_box(&scene);
        let header = header_box(&scene);
        let row = center(header).y();

        // Just right of the panel, the middle of the row, and its top edge.
        let gaps = [
            vec2f(panel.max_x() + 8., row),
            vec2f(center(header).x(), row),
            vec2f(center(header).x(), header.min_y() + 2.),
        ];

        for gap in gaps {
            let mut harness = Harness::new(2);
            harness.frame();
            press(&mut harness, gap, 1);
            assert_eq!(
                harness.window_requests(),
                vec![Request::Drag],
                "the gap at {gap:?} did not pick the window up"
            );
        }
    }

    #[test]
    fn the_top_right_corner_of_the_window_picks_it_up_on_every_platform() {
        // The corner three caption buttons used to be in. Crook draws none on
        // any platform now, so the header runs into it and it is title bar
        // like the rest of the row — a press there moves the window rather
        // than closing it. Checked for all three layouts because a cluster
        // coming back anywhere would be a corner that stops dragging.
        for layout in [
            ControlLayout::MacOs,
            ControlLayout::Windows,
            ControlLayout::Freedesktop,
        ] {
            let mut harness = Harness::new(1);
            harness.override_controls(layout);
            harness.frame();

            press(&mut harness, vec2f(WINDOW.x() - 2., 2.), 1);

            assert_eq!(
                harness.window_requests(),
                vec![Request::Drag],
                "{layout:?} did not pick the window up by its top-right corner"
            );
        }
    }

    #[test]
    fn the_third_press_in_a_row_still_picks_the_window_up() {
        // The second press of a series maximises and no other one does.
        // `InputState::count_click` goes on counting — 1, 2, 3, 4 for as long
        // as the presses stay inside half a second and four pixels of each
        // other — so a `click_count >= 2` rule made every press after a double
        // click another maximise: reach straight for the title bar to move the
        // window you have just maximised, and it restores and stays where it
        // is instead of following the pointer.
        let mut harness = Harness::new(1);
        let scene = harness.frame();
        let empty = empty_header_point(&scene);

        harness.click_times(empty, 1);
        harness.click_times(empty, 2);
        harness.click_times(empty, 3);

        assert_eq!(
            harness.window_requests(),
            vec![Request::Drag, Request::ToggleMaximized, Request::Drag],
            "the press after a double click was not a drag"
        );
    }
}

#[test]
fn a_chord_bound_to_a_plugins_action_reaches_the_plugin() {
    // The end-to-end of a named action, and the thing that was impossible
    // before there was one: `crook/window/new-tab` is registered by a plugin,
    // named in a file the application does not compile, and reached by a chord
    // this build has no arm for — `shift+cmd+u` is nobody's.
    let mut harness = Harness::new(1);
    assert_eq!(harness.tab_ids().len(), 1);

    harness.bind(r#"[{ "key": "shift+cmd+u", "command": "crook/window/new-tab" }]"#);
    harness.press(
        "u",
        Modifiers {
            cmd: true,
            shift: true,
            ..Modifiers::default()
        },
        "",
    );

    assert_eq!(
        harness.tab_ids().len(),
        2,
        "the chord did not reach the plugin's action"
    );
}

#[test]
fn a_chord_bound_to_an_action_nothing_answers_to_does_nothing() {
    // A keybindings file written for a plugin that is not installed, which is
    // the ordinary state of any file somebody copied from a friend. It costs
    // that one chord and nothing else.
    let mut harness = Harness::new(1);

    harness.bind(r#"[{ "key": "shift+cmd+u", "command": "eugen/not-installed/go" }]"#);

    assert_eq!(
        harness.action_for(
            "u",
            Modifiers {
                cmd: true,
                shift: true,
                ..Modifiers::default()
            }
        ),
        None
    );
    harness.press(
        "u",
        Modifiers {
            cmd: true,
            shift: true,
            ..Modifiers::default()
        },
        "",
    );
    assert_eq!(
        harness.tab_ids().len(),
        1,
        "a chord nothing answers to did something anyway"
    );
}

#[test]
fn the_shortcuts_page_lists_what_a_plugin_registered_and_the_chord_that_reaches_it() {
    // Where somebody finds out a command's name, which is the only way they
    // can bind it. The page cannot have been written with this row in it: the
    // name belongs to a plugin, and so does the chord.
    let mut harness = Harness::new(1);
    harness.bind(r#"[{ "key": "shift+cmd+u", "command": "crook/window/new-tab" }]"#);
    harness.open_settings_page();
    let rail = settings_rail_boxes(&harness.frame());
    harness.click(center(rail[2]), MouseButton::Left);
    assert_eq!("Keyboard Shortcuts", harness.settings_section());

    let text = frame_text(&harness.frame());

    assert!(
        text.contains("crook/window/new-tab"),
        "the page does not name the command: {text}"
    );
    assert!(
        text.contains("shift+cmd+u"),
        "the page does not say what reaches it: {text}"
    );
}

/// Points the window's keybindings at a file of its own and hands back the
/// path, so a test can read what an edit made on the page wrote.
///
/// The point of a real path, exactly as it is [`Scratch`]'s: `Keybindings::bind`
/// writes nothing when there is nowhere to write, so a test on ephemeral
/// bindings cannot see what a recording would have saved — and what it saves is
/// the whole question.
fn keybindings_in(harness: &mut Harness, scratch: &Scratch) -> PathBuf {
    let path = scratch.path().join("keybindings.json");
    let keybindings = crate::keybindings::Keybindings::load(&path);
    harness.workspace_update(|workspace, ctx| workspace.set_keybindings(keybindings, ctx));
    path
}

/// The keybindings file, once a save has written `needle` into it.
///
/// Waits, for the reason [`Scratch::written_containing`] waits: the write is
/// handed to the background pool.
fn keybindings_written(path: &Path, needle: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last = String::new();
    loop {
        last = fs::read_to_string(path).unwrap_or(last);
        if last.contains(needle) {
            return last;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{needle:?} was never written to {}: {last:?}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The keybindings file, once a save has taken `needle` out of it.
///
/// The other half of [`keybindings_written`], for an edit whose effect is a
/// line no longer being there.
fn keybindings_without(path: &Path, needle: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last = String::new();
    loop {
        last = fs::read_to_string(path).unwrap_or(last);
        if !last.contains(needle) {
            return last;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{needle:?} was never taken out of {}: {last:?}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Starts recording a chord for `command`, the way clicking its row does.
fn record(harness: &mut Harness, command: &str) {
    let name = crate::plugin::ActionName::parse(command).expect("a literal that parses");
    let id = harness
        .workspace
        .read(&harness.app, |workspace, _| workspace.host().action(&name))
        .unwrap_or_else(|| panic!("nothing answers to {command}"));
    harness.dispatch_workspace_action(SettingsAction::RecordBinding(id).into());
    harness.frame();
}

#[test]
fn a_chord_recorded_on_the_page_replaces_the_shipped_one_and_is_written_down() {
    // The whole gesture, end to end: click a chord, press the keys, press
    // Enter. What comes out is a window that answers to the new chord, does
    // not answer to the old one, and a file that says so for the next launch.
    let mut harness = Harness::new(1);
    let scratch = Scratch::new();
    let path = keybindings_in(&mut harness, &scratch);
    harness.open_settings_page();

    record(&mut harness, "crook/window/new-tab");
    let ctrl_alt = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::default()
    };
    harness.press("n", ctrl_alt, "");
    harness.press("enter", Modifiers::default(), "");

    assert_eq!(harness.tab_ids().len(), 1, "the recording opened a tab");
    assert_eq!(
        harness.action_for("n", ctrl_alt),
        Some(WorkspaceAction::Tab(TabAction::New))
    );
    assert_eq!(
        harness.action_for(
            "t",
            Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            }
        ),
        None,
        "the shipped chord still opens a tab"
    );
    let written = keybindings_written(&path, "ctrl+alt+n");
    assert!(
        written.contains("-crook/window/new-tab"),
        "the shipped chord was not taken away: {written}"
    );
}

#[test]
fn clicking_the_chord_on_a_row_hands_it_the_keyboard() {
    // The gesture itself, through the hit test: a click on the chord starts a
    // recording, and the very next chord goes into it rather than doing what
    // it is bound to. `ctrl+shift+d` splits a pane, and here it splits
    // nothing.
    let mut harness = Harness::new(1);
    let scratch = Scratch::new();
    keybindings_in(&mut harness, &scratch);
    harness.open_settings_page();
    harness.select_settings_section("Keyboard Shortcuts");

    let buttons = settings_page_button_boxes(&harness.frame());
    harness.click(center(buttons[0]), MouseButton::Left);

    assert_eq!(
        harness.recording().as_ref().map(Recording::command),
        Some(&crate::plugin::ActionName::parse("crook/window/new-tab").expect("a literal"))
    );

    harness.press(
        "d",
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        },
        "",
    );

    assert_eq!(
        harness.pane_ids().len(),
        1,
        "the recorded chord split a pane"
    );
    assert_eq!(
        harness.recording().map(|recording| recording.chord()),
        Some("ctrl+shift+d".to_owned())
    );
}

#[test]
fn a_row_that_is_recording_says_so_and_says_what_the_chord_is_already_for() {
    // The two things the row has to say while the keyboard belongs to it: how
    // to finish, and that the chord being pressed is one somebody else has.
    // The second is VSCode's warning, and without it the last rule quietly
    // wins and a person finds out the next time they reach for the chord.
    let mut harness = Harness::new(1);
    let scratch = Scratch::new();
    keybindings_in(&mut harness, &scratch);
    harness.open_settings_page();
    harness.select_settings_section("Keyboard Shortcuts");

    record(&mut harness, "crook/window/new-tab");
    let text = frame_text(&harness.frame());
    assert!(
        text.contains("press a chord"),
        "the row does not say it is recording: {text}"
    );

    harness.press(
        "d",
        Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        },
        "",
    );
    let text = frame_text(&harness.frame());

    assert!(
        text.contains("ctrl+shift+d"),
        "the row does not show what was pressed: {text}"
    );
    assert!(
        text.contains("Split to the right"),
        "the row does not say what the chord is already for: {text}"
    );
}

#[test]
fn a_recording_can_take_a_chord_the_pane_would_otherwise_have_eaten() {
    // "Any key on any action" is only true if the recorder is asked *first*.
    // ctrl-l is the shell's own — nothing in the window is bound to it, so
    // every keystroke of it reaches the pty — and it is bindable all the same.
    let mut harness = Harness::new(1);
    let scratch = Scratch::new();
    let path = keybindings_in(&mut harness, &scratch);
    harness.open_settings_page();

    record(&mut harness, "crook/window/split-right");
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    harness.press("l", ctrl, "\u{c}");
    harness.press("enter", Modifiers::default(), "");

    keybindings_written(&path, "ctrl+l");
    assert_eq!(
        harness.action_for("l", ctrl),
        Some(WorkspaceAction::Tab(TabAction::Split(Direction::Right)))
    );
}

#[test]
fn escape_leaves_the_binding_exactly_as_it_was() {
    // Nothing is written until a recording is kept, so reaching for a chord,
    // seeing it is the wrong one and pressing Escape costs nothing.
    let mut harness = Harness::new(1);
    let scratch = Scratch::new();
    let path = keybindings_in(&mut harness, &scratch);
    harness.open_settings_page();

    record(&mut harness, "crook/window/new-tab");
    harness.press(
        "n",
        Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::default()
        },
        "",
    );
    harness.press("escape", Modifiers::default(), "");

    assert_eq!(
        harness.action_for("t", tab_chord()),
        Some(WorkspaceAction::Tab(TabAction::New)),
        "the shipped chord was lost to a recording nobody kept"
    );
    assert!(!path.exists(), "a cancelled recording wrote a file");
}

#[test]
fn a_recording_that_is_never_finished_does_not_keep_the_keyboard() {
    // The one way this feature could cost somebody their window: a recorder
    // holding every keystroke with nothing on screen to say so. Leaving the
    // settings ends it, and the next chord works.
    let mut harness = Harness::new(1);
    let scratch = Scratch::new();
    keybindings_in(&mut harness, &scratch);
    harness.open_settings_page();
    record(&mut harness, "crook/window/new-tab");

    harness.show_tabs();

    assert_eq!(
        harness.action_for("t", tab_chord()),
        Some(WorkspaceAction::Tab(TabAction::New))
    );
}

#[test]
fn a_command_unbound_on_the_page_gives_the_chord_back_and_a_reset_takes_it_again() {
    let mut harness = Harness::new(1);
    let scratch = Scratch::new();
    let path = keybindings_in(&mut harness, &scratch);
    harness.open_settings_page();
    let name = crate::plugin::ActionName::parse("crook/window/new-tab").expect("a literal");
    let id = harness
        .workspace
        .read(&harness.app, |workspace, _| workspace.host().action(&name))
        .expect("the window's own command");
    let shipped = tab_chord();

    harness.dispatch_workspace_action(SettingsAction::UnbindCommand(id).into());
    harness.frame();

    assert_eq!(harness.action_for("t", shipped), None);
    keybindings_written(&path, "-crook/window/new-tab");

    harness.dispatch_workspace_action(SettingsAction::ResetBinding(id).into());
    harness.frame();

    assert_eq!(
        harness.action_for("t", shipped),
        Some(WorkspaceAction::Tab(TabAction::New))
    );
    // Waited for by what the reset's save takes *out*: the file already had a
    // `[` before it, so a wait for one returns whatever was there last — which
    // on a slower machine was still the unbind.
    keybindings_without(&path, "crook/window/new-tab");
}

#[test]
fn a_sequence_takes_two_keystrokes_and_neither_of_them_reaches_the_shell() {
    // VSCode's chord mode, end to end. The first keystroke is consumed with
    // nothing to show for it — that is the point, since the alternative is a
    // stray `ctrl+k` typed into the command line while somebody reaches for
    // the second half — and the second one opens the tab.
    let mut harness = Harness::new(1);
    harness.bind(r#"[{ "key": "ctrl+k ctrl+t", "command": "crook/window/new-tab" }]"#);

    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    assert_eq!(harness.tab_ids().len(), 1);

    // Asked once, like the window delegate asks: the answer is "the window
    // took it", and the window is now waiting for the rest of the sequence.
    assert_eq!(
        harness.action_for("k", ctrl),
        Some(WorkspaceAction::Chord),
        "the first half of the sequence was not consumed"
    );
    assert_eq!(harness.tab_ids().len(), 1, "half a chord opened a tab");

    harness.press("t", ctrl, "");
    assert_eq!(harness.tab_ids().len(), 2, "the sequence did not complete");
}

#[test]
fn a_sequence_nothing_completes_ends_and_costs_the_key_that_ended_it() {
    // The other half of chord mode, and the one that has to be deliberate: a
    // keystroke that finishes nothing is swallowed rather than passed on,
    // because it was typed as part of a chord.
    let mut harness = Harness::new(1);
    harness.bind(r#"[{ "key": "ctrl+k ctrl+t", "command": "crook/window/new-tab" }]"#);

    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    harness.press("k", ctrl, "");

    assert_eq!(
        harness.action_for("j", ctrl),
        Some(WorkspaceAction::Chord),
        "the key that ended the chord went on to the pane"
    );

    // And the window is out of chord mode: `ctrl+t` on its own is nobody's
    // chord again, rather than the second half of the abandoned sequence.
    assert_eq!(harness.action_for("t", ctrl), None);
    assert_eq!(harness.tab_ids().len(), 1);
}

#[test]
fn a_command_taken_off_a_chord_gives_the_chord_back() {
    // The only way to unbind, and the reason it matters here rather than in
    // an editor: the chord goes back to the shell, which is what somebody
    // whose shell wants that key is asking for.
    let mut harness = Harness::new(1);
    let chord = tab_chord();
    assert!(harness.action_for("t", chord).is_some());

    harness.bind(&format!(
        r#"[{{ "key": "{}", "command": "-crook/window/new-tab" }}]"#,
        if cfg!(target_os = "macos") {
            "cmd+t"
        } else {
            "ctrl+shift+t"
        }
    ));

    assert_eq!(
        harness.action_for("t", chord),
        None,
        "the chord is still the window's"
    );
    assert!(
        harness.action_for("w", close_chord()).is_some(),
        "removing one binding took another with it"
    );
}

#[test]
fn a_when_clause_decides_whether_a_chord_is_in_force() {
    // The context comes from the window rather than from the file, so this is
    // the whole of what a clause is worth: the same chord means one thing on
    // the settings page and nothing in a shell.
    let mut harness = Harness::new(1);
    harness.bind(
        r#"[{
            "key": "ctrl+alt+shift+n",
            "command": "crook/window/new-tab",
            "when": "settingsFocused"
        }]"#,
    );
    let chord = Modifiers {
        ctrl: true,
        alt: true,
        shift: true,
        ..Modifiers::default()
    };

    assert_eq!(
        harness.action_for("n", chord),
        None,
        "the binding fired with its condition false"
    );

    harness.open_settings_page();

    assert!(
        harness.action_for("n", chord).is_some(),
        "the binding did not fire with its condition true"
    );
}

/// The chord that opens a tab on this platform.
fn tab_chord() -> Modifiers {
    Modifiers {
        cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        shift: !cfg!(target_os = "macos"),
        ..Default::default()
    }
}

/// The chord that closes a pane on this platform, which is the same shape.
fn close_chord() -> Modifiers {
    tab_chord()
}

/// What the palette's field says while nothing has been typed, which is how a
/// test tells an open card from a closed one.
const PALETTE_PLACEHOLDER: &str = "Search commands, tabs and settings";

/// The chord `crook/palette` asks for: the one every editor uses, plus the
/// Shift that keeps Crook's chords off a bare Ctrl-letter away from macOS.
fn palette_chord() -> Modifiers {
    Modifiers {
        cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        shift: true,
        ..Default::default()
    }
}

#[test]
fn the_palette_opens_on_the_chord_a_plugin_asked_for() {
    // A chord no `input_keys` table knows about, reaching a surface no
    // `Workspace` field holds. Both halves are the plugin's.
    let mut harness = Harness::new(1);
    assert!(!frame_text(&harness.frame()).contains(PALETTE_PLACEHOLDER));

    harness.press("p", palette_chord(), "");
    let text = frame_text(&harness.frame());

    assert!(
        text.contains(PALETTE_PLACEHOLDER),
        "the palette did not come up: {text}"
    );
    // Every command `crook/window` registered, listed by its title and by the
    // name a person would bind.
    assert!(text.contains("New agent tab"), "{text}");
    assert!(text.contains("crook/window/new-tab"), "{text}");
}

#[test]
fn the_palette_lists_what_it_is_told_and_not_its_own_keys() {
    // The distinction between an action and a command: the arrows and the
    // Escape are reachable by name and are not rows in a list of things to do.
    let mut harness = Harness::new(1);
    harness.press("p", palette_chord(), "");

    let text = frame_text(&harness.frame());

    assert!(text.contains("Command palette"), "{text}");
    assert!(
        !text.contains("crook/palette/next"),
        "the palette offered its own arrow key as a command: {text}"
    );
    assert!(
        !text.contains("crook/palette/run"),
        "the palette offered its own Enter as a command: {text}"
    );
}

#[test]
fn typing_narrows_the_palette() {
    let mut harness = Harness::new(1);
    harness.press("p", palette_chord(), "");
    harness.frame();
    harness.type_text("split");
    let text = frame_text(&harness.frame());

    assert!(text.contains("Split to the right"), "{text}");
    assert!(
        !text.contains("New agent tab"),
        "the list did not narrow: {text}"
    );
}

#[test]
fn a_query_that_matches_nothing_says_so_rather_than_showing_everything() {
    let mut harness = Harness::new(1);
    harness.press("p", palette_chord(), "");
    harness.frame();
    harness.type_text("zzzz");
    let text = frame_text(&harness.frame());

    assert!(text.contains("Nothing matches that."), "{text}");
    assert!(!text.contains("New agent tab"), "{text}");
}

#[test]
fn enter_runs_what_is_selected_and_takes_the_palette_down() {
    let mut harness = Harness::new(1);
    assert_eq!(harness.tab_ids().len(), 1);

    harness.press("p", palette_chord(), "");
    harness.frame();
    harness.type_text("new agent");
    harness.frame();
    harness.press("enter", Modifiers::default(), "");
    let text = frame_text(&harness.frame());

    assert_eq!(
        harness.tab_ids().len(),
        2,
        "the command did not run: {text}"
    );
    assert!(
        !text.contains(PALETTE_PLACEHOLDER),
        "the palette stayed up: {text}"
    );
}

#[test]
fn the_arrows_move_the_selection_and_enter_runs_the_row_they_are_on() {
    let mut harness = Harness::new(1);
    harness.press("p", palette_chord(), "");
    harness.frame();
    // Two rows, in title order: "Split downwards" then "Split to the right".
    harness.type_text("split");
    harness.frame();
    harness.press("down", Modifiers::default(), "");
    harness.frame();
    harness.press("enter", Modifiers::default(), "");
    harness.frame();

    assert_eq!(
        harness.pane_ids().len(),
        2,
        "the second row of the list did not run"
    );
}

#[test]
fn escape_takes_the_palette_down_and_gives_the_keyboard_back() {
    // The half that is not visible: while a surface is up nothing under it is
    // typing into a shell, and when it goes down the pane has to get the
    // keyboard back — which is `sync_input_keys`, reached from a plugin.
    let mut harness = Harness::new(1);
    let pane = harness.pane_ids()[0];
    let takes_keys = |harness: &Harness| {
        harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.pane_takes_keys(pane))
    };
    assert!(takes_keys(&harness));

    harness.press("p", palette_chord(), "");
    assert!(
        !takes_keys(&harness),
        "the pane was still typing under an open palette"
    );

    harness.press("escape", Modifiers::default(), "");
    let text = frame_text(&harness.frame());

    assert!(!text.contains(PALETTE_PLACEHOLDER), "{text}");
    assert!(takes_keys(&harness), "the pane never got the keyboard back");
}

#[test]
fn a_chord_the_window_owns_still_works_over_an_open_palette() {
    // A surface claims the bare keys it uses and nothing else, so the window's
    // own chords keep working — which is the rule the Themes panel already
    // states for its arrow keys.
    let mut harness = Harness::new(1);
    harness.press("p", palette_chord(), "");

    harness.press("t", platform_chord(), "");

    assert_eq!(harness.tab_ids().len(), 2);
}

#[test]
fn clicking_a_row_runs_it() {
    let mut harness = Harness::new(1);
    harness.press("p", palette_chord(), "");
    harness.frame();
    harness.type_text("new agent");
    let scene = harness.frame();
    let row = palette_selected_row(&scene);

    harness.click(center(row), MouseButton::Left);

    assert_eq!(harness.tab_ids().len(), 2);
}

/// The band the palette's selected row is drawn in.
///
/// Found by what the selection *is* — the only wide `overlay_2` band with the
/// row radius on it — rather than by the glyphs on the row, because a scene
/// has no occlusion and the text of the window behind the palette is in it
/// too.
fn palette_selected_row(scene: &Scene) -> RectF {
    let rows: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, bounds)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                && rect.background == Fill::Solid(theme().overlay_2)
                && bounds.width() > 400.
        })
        .map(|(_, bounds)| bounds)
        .collect();

    assert_eq!(rows.len(), 1, "exactly one selected palette row per frame");
    rows[0]
}
/// Opens the palette on the list of keys, with `query` typed after the sigil.
///
/// Through the chord and then the field, because that is the entry path this
/// feature ships: `crook/palette/keys` spends no chord of its own, and a `?`
/// typed at the front of the query is the whole of how a person gets here.
fn open_keys(harness: &mut Harness, query: &str) {
    harness.press("p", palette_chord(), "");
    harness.frame();
    harness.type_text(&format!("?{query}"));
}

/// Every place the frame draws `text` as a line of its own.
///
/// Whole lines rather than a search over the frame, because that is the only
/// way a heading can be asked about: `Window commands` names a plugin *and* is
/// four words inside `Turn the Window commands plugin on or off`, and
/// `Worktrees` is a heading the fold exists to prevent *and* the title of the
/// one command that plugin registers. A `contains` over the frame cannot tell
/// any of those apart.
fn drawn_lines(scene: &Scene, text: &str) -> Vec<Vector2F> {
    text_lines(scene, |_| true)
        .into_iter()
        .filter(|(_, line)| line == text)
        .map(|(at, _)| at)
        .collect()
}

/// Where the frame draws `text` as a line of its own, if it does anywhere.
fn drawn_line(scene: &Scene, text: &str) -> Option<Vector2F> {
    drawn_lines(scene, text).first().copied()
}

/// What the selected row is called.
///
/// The topmost line inside the band, which is the title: a row's keys and its
/// action name are set two pixels lower, in the secondary size, so they group
/// as a line of their own.
fn palette_selected_title(scene: &Scene) -> String {
    let band = palette_selected_row(scene);
    text_lines(scene, |at| band.contains_point(at))
        .into_iter()
        .next()
        .map(|(_, line)| line)
        .unwrap_or_default()
}

/// The chord the shipped table binds `command` to, as a row's cap prints it.
///
/// Read out of the table rather than written down again. A test about what a
/// row says must not carry a second copy of what the window is bound to, and
/// this way the assertion is the same true sentence on a Mac, where the two
/// chords below are spelled `cmd+t` and `cmd+f`.
fn shipped_chord(command: &str) -> &'static str {
    let table = if cfg!(target_os = "macos") {
        crate::keybindings::DEFAULTS_MAC
    } else {
        crate::keybindings::DEFAULTS_OTHER
    };

    table
        .iter()
        .find(|(_, bound)| *bound == command)
        .map(|(keys, _)| *keys)
        .unwrap_or_else(|| panic!("nothing in the shipped table binds {command}"))
}

#[test]
fn a_question_mark_turns_the_palette_into_a_list_of_keys() {
    // One character, and the launcher is a keymap: headings over the rows and
    // a word on every command no key reaches. Neither of those can come from
    // the list of things to run, which is what makes them the mode's
    // signature.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "");
    let scene = harness.frame();

    assert!(
        drawn_line(&scene, "Window commands").is_some(),
        "the list is not grouped: {}",
        frame_text(&scene)
    );
    assert!(
        frame_text(&scene).contains("not bound"),
        "the list says nothing about the commands no key reaches: {}",
        frame_text(&scene)
    );
}

#[test]
fn the_list_of_things_to_run_says_nothing_about_what_is_not_bound() {
    // The mode boundary, in one place. The launcher lists every command there
    // is and none of the keymap's furniture: no heading, and no `not bound` on
    // the forty rows that have no chord — which is the noise the sigil exists
    // to keep off the fast path.
    let mut harness = Harness::new(1);
    harness.press("p", palette_chord(), "");
    let scene = harness.frame();
    let text = frame_text(&scene);

    assert!(text.contains("New agent tab"), "{text}");
    assert!(
        drawn_line(&scene, "Window commands").is_none(),
        "the launcher grew a heading: {text}"
    );
    assert!(
        !text.contains("not bound"),
        "the launcher started reporting what is not bound: {text}"
    );
}

#[test]
fn the_keys_are_grouped_by_the_plugin_that_registered_the_command() {
    // Registration order, which is load order, which is the order the settings
    // rail is already in. Asserted by where the headings landed rather than by
    // what the frame says, because both names are also words in a row.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "");
    let scene = harness.frame();

    let window = drawn_line(&scene, "Window commands").expect("the first plugin's heading");
    let tabs = drawn_line(&scene, "Tabs").expect("the second plugin's heading");
    assert!(
        window.y() < tabs.y(),
        "the groups are not in load order: {window:?} then {tabs:?}"
    );

    let under = text_lines(&scene, |_| true)
        .into_iter()
        .find(|(at, line)| at.y() > tabs.y() && line.contains("crook/tabs/"))
        .map(|(at, _)| at);
    assert!(
        under.is_some(),
        "nothing the Tabs plugin registered is under its heading: {}",
        frame_text(&scene)
    );
}

#[test]
fn the_plugins_that_registered_one_command_share_a_heading() {
    // The fold: seven plugins register a single command each, and seven
    // headings over seven rows would be a fence rather than a list. `Worktrees`
    // is the test's own point — it is the name of a plugin *and* the title of
    // the one command it registers, so a heading of its own would put the word
    // on the frame twice.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "");
    let scene = harness.frame();

    assert!(
        drawn_line(&scene, "Elsewhere").is_some(),
        "the small plugins claimed no shared heading: {}",
        frame_text(&scene)
    );
    assert_eq!(
        drawn_lines(&scene, "Worktrees").len(),
        1,
        "`Worktrees` is drawn as a heading as well as a row: {}",
        frame_text(&scene)
    );
}

#[test]
fn the_arrows_step_over_a_group_heading() {
    // `close` matches two commands the window registered and two the tabs did,
    // so the list is a group of two, a heading, and a group of two. One row is
    // 34px and a heading is 28, and the selection moving by 62 is the heading
    // being passed over rather than landed on.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "close");
    let inside_a_group = palette_selected_row(&harness.frame()).min_y();

    harness.press("down", Modifiers::default(), "");
    let next = palette_selected_row(&harness.frame()).min_y();
    assert_eq!(next - inside_a_group, 34.);

    harness.press("down", Modifiers::default(), "");
    let across = palette_selected_row(&harness.frame()).min_y();
    assert_eq!(across - next, 34. + 28.);
}

#[test]
fn a_heading_is_not_a_row_that_can_be_run() {
    // Enter means one thing in both lists, and the row it acts on is the row
    // the band is on. `shell` puts one command under `Plugins`, one under
    // `Elsewhere` and two keys a pane eats under `In a pane`, so a single Down
    // crosses a heading — and if the arrows had landed on it, Enter would be a
    // no-op with nothing to show for it.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "shell");
    harness.frame();

    harness.press("down", Modifiers::default(), "");
    let scene = harness.frame();
    assert_eq!(palette_selected_title(&scene), "Settings: Shell");

    harness.press("enter", Modifiers::default(), "");
    let text = frame_text(&harness.frame());

    assert_eq!(harness.settings_section(), "Shell", "{text}");
    assert!(
        text.contains("Start a login shell"),
        "the row the band was on did not run: {text}"
    );
}

#[test]
fn the_arrows_step_over_a_key_a_pane_eats() {
    // Navigationally a fact is a heading that happens to be 34px tall: there
    // is nothing for Enter to mean on `Send the line to the shell`, so the
    // arrows walk past it and wrap to the top of the list instead.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "shell");
    let scene = harness.frame();
    let first = palette_selected_title(&scene);
    assert!(
        frame_text(&scene).contains("Send the line to the shell"),
        "there is no key a pane eats to step over: {}",
        frame_text(&scene)
    );

    harness.press("down", Modifiers::default(), "");
    harness.frame();
    harness.press("down", Modifiers::default(), "");

    assert_eq!(
        palette_selected_title(&harness.frame()),
        first,
        "the selection landed in the keys a pane eats instead of wrapping"
    );
}

#[test]
fn every_chord_is_printed_and_not_only_the_first() {
    // A launcher's row says whether there is a faster way; the keymap's row
    // says what all of them are. Both caps on one line, asserted on that line
    // rather than on the frame, because the frame holds every other row's
    // chords too.
    let mut harness = Harness::new(1);
    harness.bind(r#"[{ "key": "ctrl+alt+n", "command": "crook/window/new-tab" }]"#);
    open_keys(&mut harness, "new agent");
    let scene = harness.frame();

    let (_, row) = text_lines(&scene, |_| true)
        .into_iter()
        .find(|(_, line)| line.contains("crook/window/new-tab"))
        .expect("the row for the command the binding names");
    assert!(row.contains(shipped_chord("crook/window/new-tab")), "{row}");
    assert!(row.contains("ctrl+alt+n"), "{row}");
}

#[test]
#[cfg(not(target_os = "macos"))]
fn one_key_that_prints_two_characters_is_one_chord() {
    // `ctrl+shift+]` is bound under both spellings, because a keyboard reports
    // the shifted character and Crook cannot know which one arrives. The
    // settings page prints both, on purpose — it explains why a key does what
    // it does. A row of the keymap is a list of keys, and that is one key.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "focus the next pane");
    let scene = harness.frame();

    let (_, row) = text_lines(&scene, |_| true)
        .into_iter()
        .find(|(_, line)| line.contains("crook/window/focus-next-pane"))
        .expect("the row for the command the chord reaches");
    assert!(row.contains("ctrl+shift+]"), "{row}");
    assert!(
        !frame_text(&scene).contains("ctrl+shift+}"),
        "the shifted twin is printed as a second key: {}",
        frame_text(&scene)
    );
}

#[test]
fn typing_a_chord_finds_what_it_runs() {
    // The reverse lookup, and the half of «на какую кнопку забиндено» a list of
    // titles cannot answer: press the keys into the box and be told what they
    // do. It is why the chord text is in the haystack.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, shipped_chord("crook/window/find"));
    let text = frame_text(&harness.frame());

    assert!(text.contains("Find in output"), "{text}");
    assert!(
        !text.contains("New agent tab"),
        "the list did not narrow to what the chord reaches: {text}"
    );
}

#[test]
fn typing_unbound_narrows_to_what_has_no_key() {
    // The list of things worth binding, which is a query rather than a filter
    // control. `unbound` is not a word any row prints — the row says
    // `not bound` — so the query is answered by what a row *lacks*.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "unbound");
    let text = frame_text(&harness.frame());

    assert!(text.contains("not bound"), "{text}");
    assert!(
        !text.contains("New agent tab"),
        "a command with a key survived the query: {text}"
    );
    assert!(
        !text.contains("No command matches that."),
        "the query for the unbound matched nothing: {text}"
    );
}

#[test]
fn tab_turns_the_list_over_and_keeps_the_row_it_was_on() {
    // The whole argument for a mode kept in the query rather than in a flag: a
    // search survives the switch, and so does the row a person had walked to.
    // The two lists put `Split to the left` at two different places — third in
    // the launcher's alphabet, fifth in a group that puts the bound rows first
    // — so the row is kept by name and not by index.
    let mut harness = Harness::new(1);
    harness.press("p", palette_chord(), "");
    harness.frame();
    harness.type_text("split");
    harness.frame();
    harness.press("down", Modifiers::default(), "");
    harness.frame();
    harness.press("down", Modifiers::default(), "");
    assert_eq!(
        palette_selected_title(&harness.frame()),
        "Split to the left"
    );

    harness.press("tab", Modifiers::default(), "");
    let scene = harness.frame();

    assert_eq!(palette_selected_title(&scene), "Split to the left");
    assert!(
        drawn_line(&scene, "Window commands").is_some(),
        "tab did not turn the card over: {}",
        frame_text(&scene)
    );
}

#[test]
fn a_chord_the_window_owns_still_works_over_the_list_of_keys() {
    // The claim grew a key — `tab`, for the switch — and this is the invariant
    // that growing it must not have broken: the surface takes bare keys and
    // nothing else, so everything in anybody's keybindings file keeps working
    // over an open card.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "");
    harness.frame();

    harness.press("t", platform_chord(), "");

    assert_eq!(harness.tab_ids().len(), 2);
}

#[test]
fn typing_a_key_the_pane_eats_says_what_it_does() {
    // The query the scope of this list was decided for. `ctrl+c` is bound to
    // nothing and is the key people ask about most, because what it does
    // belongs to the program in the pane rather than to a keybinding — and a
    // list of keys that answered it with silence would be answering the wrong
    // question well.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "ctrl+c");
    let scene = harness.frame();
    let text = frame_text(&scene);

    assert!(text.contains("Interrupt, suspend, end the input"), "{text}");
    assert!(
        drawn_line(&scene, "In a pane").is_some(),
        "the keys the pane answers to are not a block of their own: {text}"
    );
}

#[test]
fn a_key_the_pane_eats_is_not_a_row_that_can_be_run() {
    // `sigint` is a keyword on that row and a word on no other, so the list is
    // a heading and one fact and there is nothing for the keyboard to be on.
    // Enter over it does what Enter over an empty list does: takes the card
    // down and runs nothing.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "sigint");
    let text = frame_text(&harness.frame());
    assert!(text.contains("Interrupt, suspend, end the input"), "{text}");

    harness.press("enter", Modifiers::default(), "");
    let text = frame_text(&harness.frame());

    assert_eq!(harness.pane_ids().len(), 1, "something ran: {text}");
    assert_eq!(harness.tab_ids().len(), 1, "something ran: {text}");
    assert!(!text.contains("esc to close"), "the card stayed up: {text}");
}

#[test]
fn a_query_the_list_of_keys_cannot_answer_says_so_in_its_own_words() {
    // The two lists fail differently because they hold different things. The
    // launcher can only be short of a command; the list of keys can be short
    // of a key, and telling somebody who typed a chord that no *command*
    // matches answers a question they did not ask.
    let mut harness = Harness::new(1);
    open_keys(&mut harness, "zzzqqq");
    let text = frame_text(&harness.frame());

    assert!(text.contains("No key or command matches that."), "{text}");
    assert!(
        !text.contains("No command matches that."),
        "the list of keys borrowed the launcher's sentence: {text}"
    );
}

/// Opens the palette and types `query` into it.
///
/// Through the chord and then the field, because that is the entry path a
/// person has: every list is reached by a sigil typed into the one box.
fn open_palette(harness: &mut Harness, query: &str) {
    harness.press("p", palette_chord(), "");
    harness.frame();
    harness.type_text(query);
}

#[test]
fn each_kind_of_answer_gets_a_heading_when_there_is_another_kind() {
    // The rule the everything list is built on: a heading is drawn when there
    // is something to tell apart. `agent` is answered by a command and by a
    // tab, so both blocks are named; `split` is answered by commands alone,
    // and naming the only list there is would be a fence rather than a label.
    let mut harness = Harness::new(2);
    open_palette(&mut harness, "agent");
    let scene = harness.frame();

    assert!(
        drawn_line(&scene, "Commands").is_some(),
        "the commands were not named: {}",
        frame_text(&scene)
    );
    assert!(
        drawn_line(&scene, "Tabs").is_some(),
        "the tabs were not named: {}",
        frame_text(&scene)
    );
    assert!(drawn_line(&scene, "New agent tab").is_some());

    harness.press("escape", Modifiers::default(), "");
    harness.frame();
    open_palette(&mut harness, "minimise");
    let scene = harness.frame();

    assert!(drawn_line(&scene, "Minimise the window").is_some());
    assert!(
        drawn_line(&scene, "Commands").is_none(),
        "a list of nothing but commands grew a heading: {}",
        frame_text(&scene)
    );
}

#[test]
fn enter_on_a_tab_goes_to_that_tab() {
    // A row that is not a command at all: it carries an action and the tab it
    // is about, and Enter says the one before running the other.
    let mut harness = Harness::new(3);
    let tabs = harness.tab_ids();
    assert_eq!(harness.active_id(), tabs[2]);

    open_palette(&mut harness, "@agent 1");
    let scene = harness.frame();
    assert!(
        drawn_line(&scene, "New agent tab").is_none(),
        "the sigil did not narrow the card to the tabs: {}",
        frame_text(&scene)
    );

    harness.press("enter", Modifiers::default(), "");
    let text = frame_text(&harness.frame());

    assert_eq!(harness.active_id(), tabs[0], "{text}");
    assert!(!text.contains(PALETTE_PLACEHOLDER), "{text}");
}

#[test]
fn the_tab_a_person_is_in_says_so() {
    let mut harness = Harness::new(2);
    open_palette(&mut harness, "@");
    let text = frame_text(&harness.frame());

    // Counted in the frame rather than looked for as a line of its own: a
    // note is set beside the words it is about, on their baseline.
    assert_eq!(
        text.matches("you are here").count(),
        1,
        "one row is the tab on screen, and only one: {text}"
    );
}

#[test]
fn a_settings_row_opens_the_page_it_lives_on() {
    // The third kind of row, and the one that has somewhere to arrive: a page
    // is thirty rows tall, so the row's own words go into the rail's box on
    // the way — which is what puts the row a person asked for on screen.
    let mut harness = Harness::new(1);
    open_palette(&mut harness, "login shell");
    let scene = harness.frame();
    assert!(
        drawn_line(&scene, "Start a login shell").is_some(),
        "the settings were not searched: {}",
        frame_text(&scene)
    );

    harness.press("enter", Modifiers::default(), "");
    let scene = harness.frame();
    let text = frame_text(&scene);

    assert!(!text.contains(PALETTE_PLACEHOLDER), "{text}");
    assert!(
        drawn_line(&scene, "Start a login shell").is_some(),
        "the page it lives on is not on screen: {text}"
    );
    assert_eq!(
        harness
            .workspace
            .read(&harness.app, |workspace, _| workspace
                .settings_search_text()),
        "Start a login shell",
        "the row's own words are what put it on screen: {text}"
    );
}

#[test]
fn a_command_is_not_answered_twice_over() {
    // Every command is also a row of the Keyboard Shortcuts page, and that
    // page is one of the settings. Listing it here would answer `split` with
    // the same five titles twice — which teaches a person that half of what
    // the palette says is noise.
    let mut harness = Harness::new(1);
    open_palette(&mut harness, "split");
    let scene = harness.frame();

    assert_eq!(
        drawn_lines(&scene, "Split to the right").len(),
        1,
        "{}",
        frame_text(&scene)
    );
}

#[test]
fn what_the_pane_eats_is_not_offered_as_something_to_run() {
    // The mode boundary again, for the rows the addendum added. The launcher
    // lists what can be run, and none of these can be: a list that offered
    // `Interrupt, suspend, end the input` as a command would be offering a row
    // Enter cannot act on.
    let mut harness = Harness::new(1);
    harness.press("p", palette_chord(), "");
    harness.frame();
    harness.type_text("ctrl+c");
    let scene = harness.frame();
    let text = frame_text(&scene);

    // Not "nothing matches": a chord's spelling is searchable, and on a Mac
    // `ctrl+c` is inside `ctrl+cmd+left`, which moves a tab. What must not
    // match is the row for the key itself.
    assert!(
        !text.contains("Interrupt, suspend, end the input"),
        "the launcher offered a key the pane eats: {text}"
    );
    assert!(
        drawn_line(&scene, "In a pane").is_none(),
        "the launcher grew the keymap's last group: {text}"
    );
}

#[test]
fn the_settings_rail_lists_the_pages_the_plugins_contributed() {
    // Through the real presenter, because the rail's order is worked out from
    // what is registered and the assertion worth making is about pixels.
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    let scene = harness.frame();

    let rail = settings_rail_boxes(&scene);
    assert_eq!(rail.len(), 4, "four pages in the rail");
    // Top to bottom, which is the `order` each plugin asked for.
    assert_eq!(harness.settings_section(), "Appearance");

    harness.click(center(rail[1]), MouseButton::Left);
    assert_eq!(harness.settings_section(), "Shell");
    assert!(
        frame_text(&harness.frame()).contains("Start a login shell"),
        "the Shell page did not come up"
    );
}

#[test]
fn a_settings_page_can_be_reached_by_the_name_on_its_rail_row() {
    // What `--settings shell` resolves through, and the only name a person
    // ever sees: the key is `owner/entry` and nobody types that.
    let harness = Harness::new(1);

    let (found, missing) = harness.workspace.read(&harness.app, |workspace, _| {
        (
            workspace.settings_page_named("shell"),
            workspace.settings_page_named("nonesuch"),
        )
    });

    assert!(found.is_some(), "the Shell page is not reachable by name");
    assert!(missing.is_none());
}

mod sandboxed {
    use super::*;
    use crate::picture::tests::{header_only, icon_png, preview_png};
    use crate::plugins::wasm::tests::{
        Scratch, install, manifest, wasm, wasm_asking, wasm_at, wasm_carrying, wasm_saying,
    };
    use crate::workspace::settings_page::widgets;
    use crook_plugin_api::Capability;

    /// A window opened on a scratch directory: its plugins are the ones in
    /// the box plus whatever is installed there, and it is also where the
    /// window installs to and removes from, so a test that does either does
    /// it to the scratch. `withdrawn` and `heard` are what the registry
    /// says, the way the window would read it off the cached index.
    fn opening(
        scratch: &Scratch,
        withdrawn: std::collections::BTreeMap<String, String>,
        heard: crate::plugins::store::index::Heard,
    ) -> Opening {
        let mut plugins = crate::plugins::defaults();
        plugins.extend(crate::plugins::wasm::installed(scratch.path()));
        Opening {
            settings: Settings::ephemeral(),
            channel: Channel::Dev,
            plugins,
            withdrawn,
            heard,
            plugins_directory: Some(scratch.path().to_path_buf()),
        }
    }

    /// A harness on a scratch directory, with the registry saying nothing.
    fn harness(scratch: &Scratch) -> Harness {
        Harness::with_opening(1, opening(scratch, Default::default(), Default::default()))
    }

    /// Every picture the frame draws, as the box each lands in.
    fn images(scene: &Scene) -> Vec<RectF> {
        scene
            .layers()
            .flat_map(|layer| layer.images.iter())
            .map(|image| image.bounds)
            .collect()
    }

    /// The rooms the card holds for pictures that have not landed: box fill,
    /// square-cornered, unbordered, between the card's edges — which is none
    /// of the answer boxes (rounded), the hovered rows (in the list) or the
    /// field (bordered). Unclipped, as [`images`] is, because the card
    /// scrolls and a room below the window's foot is still a room.
    fn reserved(scene: &Scene) -> Vec<RectF> {
        let pane = settings_pane_box(scene);
        scene
            .layers()
            .flat_map(|layer| layer.rects.iter())
            .filter(|rect| {
                rect.background == Fill::Solid(theme().overlay_1)
                    && rect.corner_radius == CornerRadius::default()
                    && rect.border.width == 0.
            })
            .map(|rect| rect.bounds)
            .filter(|bounds| bounds.min_x() >= pane.min_x() && bounds.max_x() <= pane.max_x())
            .collect()
    }

    /// The plugin every test here is about.
    fn probe() -> crook_plugin::PluginId {
        crook_plugin::PluginId::parse("eugen/probe").expect("a literal that parses")
    }

    /// Whether the sidebar's list has a row reading `name`.
    fn listed(scene: &Scene, name: &str) -> bool {
        let column = settings_field_boxes(scene)
            .into_iter()
            .next()
            .expect("the Plugins section has a field of its own");
        text_lines(scene, |at| {
            at.x() >= column.min_x() && at.x() <= column.max_x()
        })
        .into_iter()
        .any(|(_, line)| line.trim() == name)
    }

    #[test]
    fn a_plugin_installed_while_the_window_is_open_has_a_row_a_switch_and_an_allow() {
        // What installing from the store comes to on this page. The row, the
        // switch and Allow are one action each for the whole page, run about
        // the plugin, and the switch is registered again for the new list —
        // so a plugin that arrived after `load` is one somebody can click on,
        // switch, and answer for, rather than a row that does nothing until a
        // restart.
        let scratch = Scratch::new("arrived");
        let mut harness = harness(&scratch);
        let module = wasm("eugen/probe", "header.right", 10);
        harness.workspace_update(|workspace, ctx| {
            workspace
                .install_plugin(&module, |_| Ok(()), ctx)
                .expect("it should install");
        });
        assert!(
            scratch.path().join("eugen.probe").is_dir(),
            "the module was not written where the window installs to"
        );

        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();
        assert!(
            says(&scene, "Installed, sandboxed"),
            "{}",
            frame_text(&scene)
        );

        // Allow writes the plugin's keys.
        harness.click_page_button("Allow");
        let granted = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.settings().granted_to("eugen/probe").to_vec()
        });
        assert_eq!(granted, ["tabs.read"]);

        // And the switch is live: it takes the plugin out.
        let switch = settings_switch_boxes(&harness.frame())[0];
        harness.click(center(switch), MouseButton::Left);
        harness.frame();
        let loaded = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.host().is_loaded(&probe())
        });
        assert!(
            !loaded,
            "the switch on a plugin that arrived mid-session is dead"
        );
    }

    #[test]
    fn allowing_after_an_update_writes_what_the_new_version_asks_for() {
        // The bug this page's actions were rewritten around. Allow used to
        // capture the capability list when the page loaded, so after an
        // update that asked for more it wrote the old list — and the card
        // went on saying "asking for more than you allowed" forever. What is
        // written now is read off the manifest at the press.
        let scratch = Scratch::new("escalated");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let mut harness = harness(&scratch);
        harness.workspace_update(|workspace, ctx| {
            workspace.set_plugin_granted(&probe(), vec![String::from("tabs.read")], ctx);
        });

        let mut newer = manifest("eugen/probe");
        newer.version = String::from("0.2.0");
        newer.capabilities = vec![
            Capability::ReadTabs,
            Capability::Network(vec![String::from("example.com")]),
        ];
        let module = wasm_saying(&newer, "header.right", 10);
        harness.workspace_update(|workspace, ctx| {
            workspace
                .install_plugin(&module, |_| Ok(()), ctx)
                .expect("the update should install");
        });

        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();
        assert!(
            says(&scene, "It is asking for more than you allowed"),
            "{}",
            frame_text(&scene)
        );

        harness.click_page_button("Allow");

        let granted = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.settings().granted_to("eugen/probe").to_vec()
        });
        assert_eq!(granted, ["tabs.read", "net:example.com"]);
        assert!(
            says(&harness.frame(), "What it is allowed to do"),
            "the card still says the plugin is asking for more"
        );
    }

    #[test]
    fn a_plugin_granted_allow_cannot_allow_itself() {
        // The escalation `Request::Run` would otherwise be. Allow resolves
        // the subject and the list at the press, so a guest holding
        // `run:crook/plugins/allow` that names its own id would be granted
        // whatever its newest version asks for — the network, here — with
        // nobody reading the card. The host refuses a guest every action a
        // person answers, by name, and the grant is what it was.
        //
        // The module is on disk before the window opens, because a store
        // refuses to install one that asks for an answer (the test after
        // this one); what is on disk from before that rule is loaded and
        // run, and this is the door that has to stay shut for it.
        let scratch = Scratch::new("self-allow");
        let allow = String::from("crook/plugins/allow");
        let request = crook_plugin_api::Request::Run {
            name: allow.clone(),
            argument: String::from("eugen/probe"),
        };
        // The version that asks for more than the grant — the network — so
        // the card is escalated and the new line refused until a person
        // allows it.
        let mut escalating = manifest("eugen/probe");
        escalating.version = String::from("0.2.0");
        escalating.capabilities = vec![
            Capability::RunCommands(vec![allow.clone()]),
            Capability::Network(vec![String::from("evil.example")]),
        ];
        install(
            scratch.path(),
            "eugen.probe",
            &wasm_asking(&escalating, "header.right", 10, &request),
        );
        let granted = vec![format!("run:{allow}")];
        let mut opening = opening(&scratch, Default::default(), Default::default());
        opening.settings.set_granted("eugen/probe", granted.clone());
        let mut harness = Harness::with_opening(1, opening);
        harness.show_plugins();
        harness.click_plugin("Probe");
        assert!(says(
            &harness.frame(),
            "It is asking for more than you allowed"
        ));

        // The guest, pressed, asks the host to run Allow about itself. The
        // request is served by the observer on the frames after the press;
        // a refusal lands nothing to wait for, so the queue is given the
        // turns a served request would have taken.
        harness.run_command("eugen/probe/poke");
        for _ in 0..4 {
            harness.queue.run_until_parked();
            harness.frame();
        }

        let now = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.settings().granted_to("eugen/probe").to_vec()
        });
        assert_eq!(now, granted, "the plugin allowed itself the network");
        assert!(
            says(&harness.frame(), "It is asking for more than you allowed"),
            "{}",
            frame_text(&harness.frame())
        );
    }

    #[test]
    fn a_module_asking_to_run_an_answer_is_not_installed() {
        // The same door from the other side: the card never asks "Use
        // Crook's own crook/plugins/allow" with an Allow under it, because a
        // module that wants it is refused before it is written, with the
        // reason.
        let scratch = Scratch::new("asks-an-answer");
        let mut asking = manifest("eugen/probe");
        asking.capabilities = vec![Capability::RunCommands(vec![String::from(
            "crook/store/install",
        )])];
        let mut harness = harness(&scratch);
        let mut outcome = Ok(());
        harness.workspace_update(|workspace, ctx| {
            outcome = workspace
                .install_plugin(&wasm_saying(&asking, "header.right", 10), |_| Ok(()), ctx)
                .map(|_| ());
        });
        assert_eq!(
            outcome,
            Err(String::from(
                "it asks to run crook/store/install, which is answered by a person, on the \
                 card, and never by a plugin"
            ))
        );
        assert!(
            !scratch.path().join("eugen.probe").exists(),
            "the module was written anyway"
        );
    }

    #[test]
    fn remove_on_the_card_takes_the_plugin_off_this_machine() {
        // Directory, grant, switch, row, palette command: all of it goes,
        // because a plugin that was removed is one somebody is done with. The
        // card falls back to the first row, the way it does for anything the
        // list no longer has.
        let scratch = Scratch::new("removed");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let mut harness = harness(&scratch);
        harness.workspace_update(|workspace, ctx| {
            workspace.set_plugin_granted(&probe(), vec![String::from("tabs.read")], ctx);
            workspace.toggle_plugin(&probe(), ctx);
        });
        harness.show_plugins();
        harness.click_plugin("Probe");
        assert!(says(&harness.frame(), "On this machine"));

        harness.click_page_button("Remove");

        assert!(
            !scratch.path().join("eugen.probe").exists(),
            "the plugin's directory is still there"
        );
        let scene = harness.frame();
        assert!(!listed(&scene, "Probe"), "the row is still in the list");
        // Said under the list, which is what lost a row — the card has
        // moved to another plugin's and must not say it there — and let go
        // of when the next row is chosen.
        assert!(
            says(
                &scene,
                "Probe is off this machine, and so is what it was allowed to do."
            ),
            "{}",
            frame_text(&scene)
        );
        let panel = panel_box(&scene);
        assert!(
            text_lines(&scene, |at| at.x() < panel.max_x())
                .iter()
                .any(|(_, line)| line.contains("Probe is off this machine")),
            "the sentence is not under the list: {}",
            frame_text(&scene)
        );
        harness.workspace.read(&harness.app, |workspace, _| {
            assert!(workspace.settings().granted_to("eugen/probe").is_empty());
            assert!(workspace.settings().disabled_plugins().is_empty());
            let toggle = ActionName::parse("crook/plugins/toggle-eugen-probe").expect("a literal");
            assert!(
                !workspace
                    .host()
                    .commands()
                    .iter()
                    .any(|(_, name, _)| *name == toggle),
                "the palette still offers a switch for a plugin that is gone"
            );
            assert!(workspace.host().action(&toggle).is_none());
        });
        // The first row is Window's, and the card followed the list.
        assert!(says(&scene, "crook/window"), "{}", frame_text(&scene));

        harness.click_plugin("Plugins");
        assert!(
            !says(&harness.frame(), "is off this machine"),
            "the sentence outlived the next choice"
        );
    }

    #[test]
    fn a_module_with_an_icon_draws_it_in_its_row_and_before_its_title() {
        // The face travels inside the module, and it is drawn twice: twelve
        // pixels tall in the list, and eighteen beside the name at the top of
        // the card — the one picture the page draws without being asked.
        let scratch = Scratch::new("icon");
        let icon = icon_png(64);
        install(
            scratch.path(),
            "eugen.probe",
            &wasm_carrying(
                &manifest("eugen/probe"),
                "header.right",
                10,
                &[("crook.icon", &icon)],
            ),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();

        let scene = harness.frame();
        let panel = panel_box(&scene);
        let in_rows: Vec<RectF> = images(&scene)
            .into_iter()
            .filter(|bounds| bounds.max_x() <= panel.max_x())
            .collect();
        assert_eq!(in_rows.len(), 1, "one plugin carries an icon");
        assert!(
            (in_rows[0].height() - widgets::ROW_ICON).abs() < 0.5,
            "a row's icon is {} tall",
            in_rows[0].height()
        );
        // A row with an icon is the same height as every other row.
        let rows = settings_rail_boxes(&scene);
        assert!(
            rows.iter()
                .all(|row| (row.height() - rows[0].height()).abs() < 0.5),
            "the rows are not one height: {rows:?}"
        );

        harness.click_plugin("Probe");
        let scene = harness.frame();
        let pane = settings_pane_box(&scene);
        let (title, _) = page_line(&scene, "Probe");
        let mark = images(&scene)
            .into_iter()
            .find(|bounds| pane.contains_point(center(*bounds)))
            .expect("the card draws the icon before its title");
        assert!(
            (mark.height() - widgets::TITLE_MARK).abs() < 0.5,
            "the title's mark is {} tall",
            mark.height()
        );
        assert!(mark.max_x() < title.x(), "the mark is not before the title");
        assert!(
            mark.min_y() < title.y() && title.y() < mark.max_y() + 4.,
            "the mark at {mark:?} is not on the title's line at {title:?}"
        );
    }

    #[test]
    fn a_fixture_with_an_icon_draws_it_in_its_row_like_a_module_does() {
        // The one route a picture of the Plugins page with a face on a row
        // takes on a machine with none of the six plugins: `--plugin-fixture`
        // names a PNG beside itself, and the row draws it at the row's own
        // size through the same path a module's face takes.
        let scratch = Scratch::new("fixture-face");
        std::fs::write(scratch.path().join("face.png"), icon_png(32))
            .expect("the icon should be writable");
        let path = scratch.path().join("fixture.json");
        std::fs::write(
            &path,
            r#"{"icon": "face.png", "header.right": {"Text": {"text": "62%", "size": "Small", "tone": "Primary"}}}"#,
        )
        .expect("the fixture should be writable");
        let mut opening = opening(&scratch, Default::default(), Default::default());
        opening.plugins.push(Box::new(
            crate::plugins::wasm::fixture::Fixture::read(&path).expect("it should read"),
        ));
        let mut harness = Harness::with_opening(1, opening);
        harness.show_plugins();

        let scene = harness.frame();
        let panel = panel_box(&scene);
        let in_rows: Vec<RectF> = images(&scene)
            .into_iter()
            .filter(|bounds| bounds.max_x() <= panel.max_x())
            .collect();
        assert_eq!(in_rows.len(), 1, "the fixture's face is in its row");
        assert!(
            (in_rows[0].height() - widgets::ROW_ICON).abs() < 0.5,
            "a row's icon is {} tall",
            in_rows[0].height()
        );
    }

    #[test]
    fn a_module_with_previews_offers_them_and_draws_them_on_a_press() {
        // Counted on the card and decoded only when asked: six screenshots
        // are megabytes of pixels, and a card is drawn for every plugin
        // somebody scrolls past. What lands is drawn at its captured size
        // halved, held to the card's measure, with the caption under it.
        let scratch = Scratch::new("previews");
        let wide = preview_png(1200, 200);
        let tall = preview_png(400, 300);
        install(
            scratch.path(),
            "eugen.probe",
            &wasm_carrying(
                &manifest("eugen/probe"),
                "header.right",
                10,
                &[
                    ("crook.preview.1", &wide),
                    ("crook.caption.1", b"The chip in the header"),
                    ("crook.preview.2", &tall),
                ],
            ),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();
        assert!(says(&scene, "2 pictures inside"), "{}", frame_text(&scene));
        assert!(says(&scene, "Show pictures"));
        assert!(
            images(&scene).is_empty(),
            "nothing is drawn before the press"
        );

        harness.click_page_button("Show pictures");
        // Nothing has pumped the queue, so this is the frame between the
        // press and the landing: the button is dead and says so, and the
        // room for each picture is drawn in the box fill.
        let waiting = harness.frame();
        assert!(says(&waiting, "Opening"), "{}", frame_text(&waiting));
        let reserved = reserved(&waiting);
        assert_eq!(reserved.len(), 2, "one room per picture: {reserved:?}");

        harness.wait_for("the previews to be decoded", |harness| {
            images(&harness.frame()).len() == 2
        });

        let scene = harness.frame();
        let pane = settings_pane_box(&scene);
        let drawn = images(&scene);
        // The wide one is 600 logical and the card is 560, so it was fitted
        // — and the room reserved for it was fitted the same way, or the
        // caption and everything under it would have moved when it landed.
        assert_eq!(
            reserved, drawn,
            "the pictures did not land in the rooms reserved for them"
        );
        // Between the card's edges; the second may be below the window's
        // foot, where the card scrolls to.
        assert!(
            drawn
                .iter()
                .all(|bounds| bounds.min_x() >= pane.min_x() && bounds.max_x() <= pane.max_x()),
            "a preview was drawn outside the card: {drawn:?}"
        );
        assert!(
            drawn.iter().all(|bounds| bounds.width() <= 560.),
            "a preview is wider than the card's measure: {drawn:?}"
        );
        // The second is the captured size halved: 200 by 150.
        assert!(
            drawn
                .iter()
                .any(|bounds| (bounds.width() - 200.).abs() < 0.5
                    && (bounds.height() - 150.).abs() < 0.5),
            "no preview is drawn at half its captured size: {drawn:?}"
        );
        assert!(
            says(&scene, "The chip in the header"),
            "the caption is missing"
        );
        assert!(
            !says(&scene, "Opening"),
            "the button still says the pictures are being opened"
        );
        // And it is not live either: the pictures are on the card, and a
        // press that changed nothing would read as broken.
        assert!(says(&scene, "Shown"), "{}", frame_text(&scene));
        assert!(!says(&scene, "Show pictures"), "{}", frame_text(&scene));
    }

    #[test]
    fn a_preview_that_will_not_decode_does_not_shift_the_others_onto_its_size() {
        // The module's reader stops at the header, so a corrupt preview is
        // counted, offered, and only found out on the press. The picture
        // after it is drawn at its own captured size — 200 by 150 — and not
        // stretched into the 600-by-100 the missing one was captured at.
        let scratch = Scratch::new("corrupt-preview");
        let corrupt = header_only(1200, 200);
        let good = preview_png(400, 300);
        install(
            scratch.path(),
            "eugen.probe",
            &wasm_carrying(
                &manifest("eugen/probe"),
                "header.right",
                10,
                &[
                    ("crook.preview.1", &corrupt),
                    ("crook.caption.1", b"The one that is missing"),
                    ("crook.preview.2", &good),
                    ("crook.caption.2", b"The one that is there"),
                ],
            ),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();
        harness.click_plugin("Probe");
        assert!(says(&harness.frame(), "2 pictures inside"));

        harness.click_page_button("Show pictures");
        harness.wait_for("the previews to be decoded", |harness| {
            !says(&harness.frame(), "Opening")
        });

        let scene = harness.frame();
        let drawn = images(&scene);
        assert_eq!(
            drawn.len(),
            1,
            "the corrupt picture is one fewer: {drawn:?}"
        );
        assert!(
            (drawn[0].width() - 200.).abs() < 0.5 && (drawn[0].height() - 150.).abs() < 0.5,
            "the picture that decoded is not at its own size: {drawn:?}"
        );
        assert!(
            says(&scene, "The one that is there"),
            "{}",
            frame_text(&scene)
        );
        assert!(
            !says(&scene, "The one that is missing"),
            "the missing picture's caption was drawn under the other one"
        );
        // The row says so, rather than "2 pictures inside" above one
        // picture with no word about the other.
        assert!(
            says(&scene, "1 of 2 pictures could be drawn"),
            "{}",
            frame_text(&scene)
        );
    }

    #[test]
    fn a_dev_plugin_is_not_something_remove_could_take_off_the_machine() {
        // The module somebody is writing runs from wherever it was built and
        // is not in the plugins directory, so the card offers no Remove for
        // it: a Remove that deleted a directory the plugin was never in would
        // delete nothing and say it had.
        //
        // Nor is it something an update could be written over, however far
        // ahead the registry is: the Store would install a copy into the
        // plugins directory, and the next build would carry the dev copy
        // back over it. So neither the count at the foot of the list nor the
        // card offers one — the card still shows the row in the Store.
        let scratch = Scratch::new("dev");
        let mut opening = opening(&scratch, Default::default(), registry_offering("0.2.0"));
        opening.plugins.push(Box::new(
            crate::plugins::wasm::opened(&wasm("eugen/probe", "header.right", 10))
                .expect("it should open"),
        ));
        let mut harness = Harness::with_opening(1, opening);
        harness.show_plugins();
        harness.click_plugin("Probe");

        let scene = harness.frame();
        assert!(says(&scene, "Installed, sandboxed"));
        assert!(!says(&scene, "On this machine"), "{}", frame_text(&scene));
        assert!(!says(&scene, "Remove"), "{}", frame_text(&scene));
        assert!(!says(&scene, "Update to"), "{}", frame_text(&scene));
        assert!(
            !says(&scene, "update in the registry"),
            "{}",
            frame_text(&scene)
        );
        assert!(says(&scene, "Show in Store"), "{}", frame_text(&scene));
        assert_eq!(
            probe_row(&scene),
            "Probe",
            "the row ends in the offered version"
        );
        harness.workspace.read(&harness.app, |workspace, _| {
            assert!(
                workspace.updates().is_empty(),
                "a dev plugin counts as an update"
            );
        });

        // And a Remove named by hand from the command line is refused on the
        // card, which has no box for the refusal to sit in but a switch, so
        // it sits under the switch.
        harness.run_about("crook/plugins/remove", "eugen/probe");
        let scene = harness.frame();
        assert!(
            says(
                &scene,
                "Probe was not removed: is not installed on this machine"
            ),
            "{}",
            frame_text(&scene)
        );
        let switch = answer_boxes(&scene)[0];
        let (refusal, _) = page_line(&scene, "was not removed");
        assert!(
            refusal.y() > switch.max_y(),
            "the refusal at {refusal:?} is not under the switch's box {switch:?}"
        );
        assert_eq!(
            page_line_color(&scene, "was not removed"),
            theme().usage_critical,
            "a refusal is a warning"
        );
    }

    #[test]
    fn a_native_plugin_the_registry_lists_is_neither_an_update_nor_offered_one() {
        // A registry row naming one of Crook's own — a mistake, or a
        // stranger's index. The binary is updated by the About page's own
        // sentence: the count skips it and its card has no machine box.
        let index = crate::plugins::store::index::parse(
            br#"{"schema": 1, "plugins": [
             {"id": "crook/window", "name": "Window", "description": "d",
              "versions": [{"version": "99.0.0", "abi": 8, "url": "https://x.invalid/w.wasm",
                            "sha256": "aa"}]}]}"#,
        )
        .expect("the test index parses");
        let heard = crate::plugins::store::index::Heard::offered(Some(&index));
        let scratch = Scratch::new("native-offered");
        let mut harness = Harness::with_opening(1, opening(&scratch, Default::default(), heard));
        harness.show_plugins();
        harness.click_plugin("Window");

        let scene = harness.frame();
        let text = frame_text(&scene);
        assert!(says(&scene, "crook/window"), "{text}");
        assert!(!says(&scene, "Update to"), "{text}");
        assert!(!says(&scene, "Show in Store"), "{text}");
        assert!(!says(&scene, "update in the registry"), "{text}");
        harness.workspace.read(&harness.app, |workspace, _| {
            assert!(
                workspace.updates().is_empty(),
                "a native counts as an update"
            );
        });
    }

    /// What the registry says about the probe, for a test: one row offering
    /// `version`, which asks to reach `example.com`.
    fn registry_offering(version: &str) -> crate::plugins::store::index::Heard {
        let index = crate::plugins::store::index::parse(
            format!(
                r#"{{"schema": 1, "plugins": [
                 {{"id": "eugen/probe", "name": "Probe", "description": "d",
                  "versions": [{{"version": "{version}", "abi": 8, "url": "https://x.invalid/p.wasm",
                                "sha256": "aa", "capabilities": ["net:example.com"],
                                "asks": ["Reach example.com"]}}]}}]}}"#
            )
            .as_bytes(),
        )
        .expect("the test index parses");
        crate::plugins::store::index::Heard {
            offers: crate::plugins::store::index::offers(&index),
            busy: Vec::new(),
        }
    }

    /// The probe's card, with the registry saying `heard` and the probe's
    /// version withdrawn for `withdrawn`, if it is.
    fn card_hearing(
        name: &str,
        heard: crate::plugins::store::index::Heard,
        withdrawn: Option<&str>,
    ) -> (Scratch, Harness) {
        let scratch = Scratch::new(name);
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let withdrawn = withdrawn
            .map(|why| [(String::from("eugen/probe"), why.to_owned())].into())
            .unwrap_or_default();
        let mut harness = Harness::with_opening(1, opening(&scratch, withdrawn, heard));
        harness.show_plugins();
        harness.click_plugin("Probe");
        (scratch, harness)
    }

    /// The line of the list that names the probe, with whatever word ends it.
    fn probe_row(scene: &Scene) -> String {
        let column = settings_field_boxes(scene)
            .into_iter()
            .next()
            .expect("the Plugins section has a field of its own");
        // The name and the word after it are set in two sizes and can sit
        // on two baselines, so the row is read by its y rather than as one
        // line: everything in the column within a row's height of the name.
        let lines = text_lines(scene, |at| {
            at.x() >= column.min_x() && at.x() <= column.max_x()
        });
        let (at, _) = lines
            .iter()
            .find(|(_, line)| line.trim().starts_with("Probe"))
            .expect("the list has a row for the probe");
        lines
            .iter()
            .filter(|(other, _)| (other.y() - at.y()).abs() < 8.)
            .map(|(_, line)| line.trim())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_card_says_when_the_registry_is_ahead_and_what_the_update_would_ask() {
        // Before anybody has opened the Store: the row ends in the version
        // an update would bring, and the card's box says the registry has a
        // newer one, offers it, and — since the new version asks for a host
        // nothing here has allowed — says so above the button, with the ask.
        let (_scratch, mut harness) = card_hearing("ahead", registry_offering("0.2.0"), None);
        let scene = harness.frame();
        let text = frame_text(&scene);

        assert!(says(&scene, "A newer version is in the registry"), "{text}");
        assert!(says(&scene, "Version 0.1.0"), "{text}");
        assert!(says(&scene, "Update to 0.2.0"), "{text}");
        // Nothing is allowed yet, so nothing after the update will be marked
        // new — and the sentence must not promise it. What is true is that
        // 0.2.0 asks past what 0.1.0 asks, and the list says what.
        assert!(
            says(&scene, "It asks for more than this version does"),
            "{text}"
        );
        assert!(!says(&scene, "marked new"), "{text}");
        assert!(says(&scene, "Reach example.com"), "{text}");
        assert!(says(&scene, "In the registry"), "{text}");
        assert!(says(&scene, "Show in Store"), "{text}");
        assert!(says(&scene, "On this machine"), "{text}");
        // The footnote is about the rows that are there: both of them.
        assert!(says(&scene, "Updating keeps what you allowed"), "{text}");
        assert!(says(&scene, "Removing takes it off this machine"), "{text}");
        assert!(
            probe_row(&scene).contains("0.2.0"),
            "the row does not end in the version: {:?}",
            probe_row(&scene)
        );

        // Allowed what runs today and no more: the update asks past the
        // grant, and after it the new line will be marked.
        harness.workspace_update(|workspace, ctx| {
            workspace.set_plugin_granted(&probe(), vec![String::from("tabs.read")], ctx);
        });
        let scene = harness.frame();
        let text = frame_text(&scene);
        assert!(
            says(&scene, "It asks for more than you have allowed"),
            "{text}"
        );
        assert!(says(&scene, "marked new"), "{text}");

        // And once the ask is within the grant, the box says that instead.
        harness.workspace_update(|workspace, ctx| {
            workspace.set_plugin_granted(
                &probe(),
                vec![String::from("tabs.read"), String::from("net:example.com")],
                ctx,
            );
        });
        let scene = harness.frame();
        assert!(
            says(&scene, "0.2.0 asks for nothing you have not allowed."),
            "{}",
            frame_text(&scene)
        );
    }

    #[test]
    fn an_update_asking_what_the_running_version_asks_is_not_an_escalation() {
        // The ordinary update: the same capability list as the version that
        // is running, and nothing allowed yet. The box used to call it "more
        // than you have allowed", list the asks with open dots, and promise
        // lines marked new — directly above the permissions box saying the
        // same list again. It says the one true thing instead, and lists
        // nothing.
        let mut heard = registry_offering("0.2.0");
        let release = heard.offers[0]
            .release
            .as_mut()
            .expect("the probe is offered");
        release.capabilities = vec![String::from("tabs.read")];
        release.asks = vec![String::from("See what your tabs are called")];
        let (_scratch, mut harness) = card_hearing("same-asks", heard, None);
        let scene = harness.frame();
        let text = frame_text(&scene);

        assert!(
            says(&scene, "0.2.0 asks for what this version asks for"),
            "{text}"
        );
        assert!(says(&scene, "none of it is allowed yet"), "{text}");
        assert!(!says(&scene, "It asks for more"), "{text}");
        // The ask is on the card once — in the permissions box — not twice.
        assert_eq!(
            page_lines(&scene)
                .iter()
                .filter(|(_, line)| line.contains("See what your tabs are called"))
                .count(),
            1,
            "{text}"
        );

        // And a release asking nothing at all says that, in those words.
        let mut heard = registry_offering("0.3.0");
        let release = heard.offers[0]
            .release
            .as_mut()
            .expect("the probe is offered");
        release.capabilities.clear();
        release.asks.clear();
        let (_scratch, mut harness) = card_hearing("asks-nothing", heard, None);
        let scene = harness.frame();
        assert!(
            says(&scene, "0.3.0 asks for nothing."),
            "{}",
            frame_text(&scene)
        );
    }

    #[test]
    fn a_registry_that_is_behind_offers_no_update() {
        // The state a yank leaves behind, and the state a person on a
        // development build is in: the newest the registry has is older than
        // what is running, and that is not an update. The row ends in
        // nothing, and the box offers the Store and Remove and no more.
        let (_scratch, mut harness) = card_hearing("behind", registry_offering("0.0.9"), None);
        let scene = harness.frame();
        let text = frame_text(&scene);

        assert!(!says(&scene, "Update to"), "{text}");
        assert!(!says(&scene, "newer version"), "{text}");
        assert!(says(&scene, "Show in Store"), "{text}");
        assert!(says(&scene, "On this machine"), "{text}");
        assert_eq!(probe_row(&scene), "Probe");
    }

    #[test]
    fn a_version_taken_back_is_offered_its_replacement_whatever_its_number() {
        // The case a comparison gets wrong. 0.0.9 is older than the 0.1.0
        // that is running, and it is still what the card offers: the running
        // one was withdrawn, and "the registry is behind you" would be a card
        // telling somebody to stay on a version somebody took back.
        let (_scratch, mut harness) = card_hearing(
            "replaced",
            registry_offering("0.0.9"),
            Some("it read the wrong file"),
        );
        let scene = harness.frame();
        let text = frame_text(&scene);

        assert!(says(&scene, "The registry has a replacement"), "{text}");
        assert!(says(&scene, "Install 0.0.9"), "{text}");
        assert!(says(&scene, "Taken back: it read the wrong file"), "{text}");
        assert!(
            probe_row(&scene).contains("0.0.9"),
            "the row does not end in the replacement: {:?}",
            probe_row(&scene)
        );
        // The note over the switch points at that box, and not at the Store:
        // one card, one place to go for one thing.
        assert!(says(&scene, "The box under this one"), "{text}");
        assert!(!says(&scene, "Remove there"), "{text}");
        let (note, _) = page_line(&scene, "The box under this one");
        let (replacement, _) = page_line(&scene, "The registry has a replacement");
        assert!(
            note.y() < replacement.y(),
            "the note at {note:?} is not over the box it points at, at {replacement:?}"
        );
    }

    #[test]
    fn a_version_taken_back_with_nothing_in_its_place_says_so() {
        // The only version there is was withdrawn. The note over the switch
        // must not send anybody to a replacement that is not there: the box
        // under it has Show in Store and Remove, and the note says which.
        let index = crate::plugins::store::index::parse(
            br#"{"schema": 1, "plugins": [
             {"id": "eugen/probe", "name": "Probe", "description": "d",
              "versions": [{"version": "0.1.0", "abi": 8, "url": "https://x.invalid/p.wasm",
                            "sha256": "aa", "yanked": "it played the wrong sound"}]}]}"#,
        )
        .expect("the test index parses");
        let heard = crate::plugins::store::index::Heard::offered(Some(&index));
        let (_scratch, mut harness) =
            card_hearing("nothing-instead", heard, Some("it played the wrong sound"));
        let scene = harness.frame();
        let text = frame_text(&scene);

        assert!(says(&scene, "Withdrawn from the registry"), "{text}");
        assert!(
            says(&scene, "Nothing is offered in its place yet"),
            "{text}"
        );
        assert!(!says(&scene, "what the registry offers instead"), "{text}");
        assert!(!says(&scene, "Install 0.1.0"), "{text}");
        assert!(!says(&scene, "replacement"), "{text}");
        assert!(says(&scene, "Show in Store"), "{text}");
        assert!(says(&scene, "Remove"), "{text}");
        // A box with Remove and no update has a footnote about removing and
        // not one about updating.
        assert!(says(&scene, "Removing takes it off this machine"), "{text}");
        assert!(!says(&scene, "Updating keeps"), "{text}");
    }

    #[test]
    fn a_subject_already_interned_is_found_without_a_scan() {
        // The Store interns one subject per row on every frame it is showing,
        // and a registry can list twenty thousand rows: a lookup that scanned
        // everything ever said would make each of those frames cost the
        // square of the list — measured at a third of a second per frame in
        // release. Twenty thousand lookups is a few milliseconds through a
        // map, and hundreds of them even on a slow machine, which is what
        // the bound allows.
        let harness = Harness::new(1);
        let names: Vec<String> = (0..20_000).map(|n| format!("owner/plugin-{n}")).collect();
        harness.workspace.read(&harness.app, |workspace, _| {
            let first: Vec<_> = names.iter().map(|name| workspace.subject(name)).collect();
            let started = std::time::Instant::now();
            let again: Vec<_> = names.iter().map(|name| workspace.subject(name)).collect();
            let took = started.elapsed();
            assert_eq!(first, again, "the same text is the same subject");
            assert!(
                took < std::time::Duration::from_millis(500),
                "twenty thousand lookups took {took:?}"
            );
        });
    }

    #[test]
    fn a_chord_bound_to_allow_grants_nothing() {
        // A chord says nothing about which plugin it means, and an action
        // that guessed — the card that happens to be showing, say — would be
        // a way to allow something unread. It logs and does nothing.
        let (_scratch, mut harness) = probe_card("chord");
        harness.run_command("crook/plugins/allow");

        let granted = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.settings().granted_to("eugen/probe").to_vec()
        });
        assert!(granted.is_empty(), "a chord allowed {granted:?}");
        assert!(says(&harness.frame(), "Not allowed"));
    }

    #[test]
    fn what_a_sandboxed_plugin_describes_is_what_the_window_draws() {
        // The whole of the second tier, end to end: a `.wasm` file in a
        // directory, run in an interpreter, describing a row it never painted
        // — and the window draws it in the theme in force, in Crook's own
        // fonts, with Crook's own icons.
        //
        // At order -1, which is the front of `header.right`: the slot is
        // `Single`, nothing a release binary carries fills it, and a plugin
        // installed from a file is the whole of what that row shows.
        let scratch = Scratch::new("draws");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", -1),
        );
        let mut harness = harness(&scratch);

        let text = frame_text(&harness.frame());

        assert!(text.contains("from a sandbox"), "{text}");
        assert!(text.contains("probed"), "the badge is missing: {text}");
        assert!(text.contains("Poke"), "the button is missing: {text}");
        // And the icon it named by string is drawn as one of Crook's own.
        assert!(
            icons_of(&harness.frame()).contains(&Lucide::GitBranch),
            "the icon it asked for was not drawn"
        );
    }

    #[test]
    fn a_chip_a_plugin_pins_to_a_pane_is_drawn_with_the_pane_it_is_about() {
        // The other place a plugin may put a chip, and the one that is not the
        // header: `pane.chips` is drawn by the pane the keyboard is in —
        // beside the line being composed while there is one, and over the
        // pane's own corner while a program has the screen. This starts a real
        // shell, because a pane with nothing running in it draws neither.
        let scratch = Scratch::new("pane-chips");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "pane.chips", 10),
        );
        let mut harness = harness(&scratch);
        if !harness.start_terminals() {
            return;
        }
        harness.frame();

        let text = frame_text(&harness.frame());

        assert!(text.contains("from a sandbox"), "{text}");
        assert!(
            icons_of(&harness.frame()).contains(&Lucide::GitBranch),
            "the icon it asked for was not drawn"
        );
    }

    #[test]
    fn a_version_the_registry_took_back_is_carried_and_not_run() {
        // A yank is honoured on the launch after it is published and on a
        // machine that is offline, because it is read off the copy of the
        // index this machine already has. What it must not be is silent: the
        // row stays, the switch is dead, and the card says the sentence
        // whoever withdrew it wrote.
        let scratch = Scratch::new("withdrawn");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let withdrawn = std::collections::BTreeMap::from([(
            String::from("eugen/probe"),
            String::from("it read a file it had no business reading"),
        )]);
        let mut harness =
            Harness::with_opening(1, opening(&scratch, withdrawn, Default::default()));

        // Not running: what it contributes to the header is not on screen.
        assert!(
            !frame_text(&harness.frame()).contains("from a sandbox"),
            "a withdrawn plugin should not be drawing anything"
        );

        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();
        let text = frame_text(&scene);

        assert!(text.contains("Withdrawn from the registry"), "{text}");
        assert!(
            text.contains("it read a file it had no business reading"),
            "the registry's own sentence is missing: {text}"
        );
        assert!(text.contains("switched off"), "{text}");

        // And the sentence is in the box with the switch it explains, above
        // it: a warning under a heading four sections down is a warning about
        // a control somebody has already given up on.
        let switch = settings_switch_boxes(&scene)[0];
        let enabled = answer_boxes(&scene)
            .into_iter()
            .find(|boxed| boxed.contains_point(center(switch)))
            .expect("the switch is in a box");
        let (at, _) = page_line(&scene, "Withdrawn from the registry");
        assert!(
            enabled.contains_point(at),
            "the warning is not in the switch's box"
        );
        assert!(at.y() < switch.min_y(), "the warning is under the switch");
        // And it is a warning: the one colour the Store already speaks them in.
        assert_eq!(
            page_line_color(&scene, "Withdrawn from the registry"),
            theme().usage_critical
        );
    }

    #[test]
    fn a_sandboxed_plugin_is_on_the_plugins_page_beside_the_ones_in_the_box() {
        let scratch = Scratch::new("listed");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();

        // In the list beside the ones in the box, and its card says where it
        // came from — which is the one thing a person needs to tell an
        // installed plugin from a built-in one.
        assert!(frame_text(&harness.frame()).contains("Probe"));
        harness.click_plugin("Probe");
        let text = frame_text(&harness.frame());

        assert!(
            text.contains("A plugin that exists to be looked at."),
            "{text}"
        );
        assert!(text.contains("Installed, sandboxed"), "{text}");
        assert!(text.contains("eugen/probe"), "{text}");
    }

    /// The probe's card, open, with nothing allowed.
    fn probe_card(name: &str) -> (Scratch, Harness) {
        let scratch = Scratch::new(name);
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();
        harness.click_plugin("Probe");
        (scratch, harness)
    }

    #[test]
    fn the_terms_and_the_button_share_a_box() {
        // What the card exists for: a person reads the list before answering.
        // The list, the state and the button are one rectangle, and the
        // paragraph about the mechanism is under it rather than between the
        // terms and the control.
        let (_scratch, mut harness) = probe_card("one-box");
        let scene = harness.frame();

        let (term, _) = page_line(&scene, "See what your tabs are called");
        let (answer, _) = page_line(&scene, "Not allowed");
        let asked = answer_boxes(&scene)
            .into_iter()
            .find(|boxed| boxed.contains_point(term))
            .expect("the terms are in a box");
        assert!(
            asked.contains_point(answer),
            "the answer is not in the terms' box"
        );

        let (explanation, _) = page_line(&scene, "None of this is allowed yet");
        assert!(
            explanation.y() > asked.max_y(),
            "the explanation is inside the box, between the terms and the button"
        );
    }

    #[test]
    fn the_enabled_box_and_the_answer_row_are_one_shape() {
        // A switch is 16 tall and a button 23, and the box around each used
        // to take its height from whichever it held: 36 and 43, two boxes
        // that promise to be one shape. Both rows are held to one lane now.
        let (_scratch, mut harness) = probe_card("one-shape");
        let scene = harness.frame();

        let boxes = answer_boxes(&scene);
        assert_eq!(
            boxes.len(),
            3,
            "the switch's box, the machine's box and the terms' box"
        );
        let (enabled_label, _) = page_line(&scene, "Enabled");
        let (answer_label, _) = page_line(&scene, "Not allowed");
        let enabled = boxes[0];
        let asked = boxes[2];
        assert!(enabled.contains_point(enabled_label));
        assert!(asked.contains_point(answer_label));

        // The label sits the same distance above the foot of either box: the
        // row is the same row.
        let above_switch = enabled.max_y() - enabled_label.y();
        let above_button = asked.max_y() - answer_label.y();
        assert!(
            (above_switch - above_button).abs() < 0.5,
            "the switch's row is {above_switch} tall under its label and the button's is \
             {above_button}"
        );
        // Forty-four, and pointedly not forty: `CONTROL_LANE` says why.
        assert!(
            (enabled.height() - 44.).abs() < 0.5,
            "the Enabled box is {} tall",
            enabled.height()
        );
    }

    #[test]
    fn no_box_on_the_card_is_a_text_field() {
        // The tests find a field by "overlay_1, a border, about 26 tall", and
        // a card is a stack of overlay_1 boxes. None of them may be bordered:
        // one that was would be pressed as the search field.
        let (_scratch, mut harness) = probe_card("no-field");
        let scene = harness.frame();

        assert_eq!(
            settings_field_boxes(&scene).len(),
            1,
            "something on the card looks like a text field"
        );
    }

    #[test]
    fn a_plugin_that_was_allowed_keeps_its_terms_and_its_revoke_in_one_box() {
        // The state the owner's own example was in. Every line is in force
        // and says so by its dot, not by a word at the end of each.
        let scratch = Scratch::new("allowed");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let mut harness = harness(&scratch);
        let probe = crook_plugin::PluginId::parse("eugen/probe").expect("a literal that parses");
        harness.workspace_update(|workspace, ctx| {
            workspace.set_plugin_granted(&probe, vec![String::from("tabs.read")], ctx);
        });
        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();

        assert!(
            says(&scene, "What it is allowed to do"),
            "{}",
            frame_text(&scene)
        );
        assert!(!says(&scene, "\u{2014} allowed"), "{}", frame_text(&scene));
        let (term, _) = page_line(&scene, "See what your tabs are called");
        let (revoke, _) = page_line(&scene, "Revoke");
        let asked = answer_boxes(&scene)
            .into_iter()
            .find(|boxed| boxed.contains_point(term))
            .expect("the terms are in a box");
        assert!(
            asked.contains_point(revoke),
            "Revoke is not in the terms' box"
        );
    }

    #[test]
    fn a_long_capability_sentence_wraps_inside_the_box() {
        // A manifest names hosts and paths, and eight of them are wider than
        // the box. The sentence wraps at the box's inner edge, and its second
        // line starts under the first line's text rather than under the dot —
        // which is what a paragraph in a flexible child of a row is for, and
        // the reason `widgets::item` may have one.
        let hosts: Vec<String> = (0..8)
            .map(|n| format!("service-{n}.example-registry.com"))
            .collect();
        let mut asking = manifest("eugen/probe");
        asking.capabilities = vec![Capability::Network(hosts.clone())];
        let scratch = Scratch::new("wrapped");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm_saying(&asking, "header.right", 10),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();

        let sentence = Capability::Network(hosts).sentence();
        assert!(
            says(&scene, &sentence),
            "the sentence was cut off: {}",
            frame_text(&scene)
        );

        let lines = page_lines(&scene);
        let first = lines
            .iter()
            .position(|(_, line)| line.starts_with("Reach "))
            .expect("the sentence starts a line");
        let (start, _) = lines[first];
        let (next, continued) = &lines[first + 1];
        assert!(
            !continued.contains("Not allowed"),
            "the sentence fitted on one line, so nothing wrapped: {continued}"
        );
        assert!(
            (next.x() - start.x()).abs() < 0.5,
            "the second line starts at {} and the first at {}",
            next.x(),
            start.x()
        );
        let (answer, _) = page_line(&scene, "Not allowed");
        assert!(
            next.y() < answer.y(),
            "the sentence ran past the box's foot"
        );

        // Beside the dot, inside the box: the sentence starts a dot and a gap
        // in from the heading, which sits at the box's own inset.
        let (heading, _) = page_line(&scene, "What it wants to be allowed to do");
        let asked = answer_boxes(&scene)
            .into_iter()
            .find(|boxed| boxed.contains_point(heading))
            .expect("the heading is in a box");
        assert!(
            asked.contains_point(start + vec2f(4., 4.)),
            "the sentence is outside the box"
        );
        assert!(
            asked.contains_point(*next + vec2f(4., 4.)),
            "the second line is outside the box"
        );
        let inset = start.x() - (heading.x() - 4.);
        assert!(
            (inset - (6. + 8.)).abs() < 0.5,
            "the text sits {inset} in from the heading, not a 6px dot and an 8px gap"
        );
    }

    #[test]
    fn a_line_the_grant_does_not_cover_is_marked_new() {
        // The card where the list has grown: a version that asks for a host
        // on top of what was allowed. The line in force goes quiet behind a
        // filled dot and the new one is the only lit text in the list — the
        // word "new" stays for a reading that cannot see the contrast.
        let mut asking = manifest("eugen/probe");
        asking.capabilities = vec![
            Capability::ReadTabs,
            Capability::Network(vec![String::from("api.example.com")]),
        ];
        let scratch = Scratch::new("escalated");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm_saying(&asking, "header.right", 10),
        );
        let mut harness = harness(&scratch);
        let probe = crook_plugin::PluginId::parse("eugen/probe").expect("a literal that parses");
        harness.workspace_update(|workspace, ctx| {
            workspace.set_plugin_granted(&probe, vec![String::from("tabs.read")], ctx);
        });
        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();

        assert!(
            says(&scene, "It is asking for more than you allowed"),
            "{}",
            frame_text(&scene)
        );
        assert!(
            says(&scene, "Reach api.example.com \u{2014} new"),
            "{}",
            frame_text(&scene)
        );
        // The covered line carries no word: the dot and the grey say it.
        assert!(!says(&scene, "\u{2014} allowed"), "{}", frame_text(&scene));
        assert_eq!(
            page_line_color(&scene, "See what your tabs are called"),
            theme().text_muted,
            "the line already in force is not quiet"
        );
        assert_eq!(
            page_line_color(&scene, "Reach api.example.com"),
            theme().text_primary,
            "the new line is not lit"
        );
        assert!(says(&scene, "Partly allowed"), "{}", frame_text(&scene));
    }

    #[test]
    fn a_plugin_nobody_answered_for_says_so_under_its_own_controls() {
        // The dead Play button, end to end. The probe draws its own controls
        // on its own card and asks for a capability nobody has granted, so
        // every request those controls make is refused before it reaches
        // anything — and the card used to draw them live and say nothing,
        // which is what makes a working button read as a broken one.
        let scratch = Scratch::new("stalled");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "plugins.card.status", 10),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();

        assert!(
            says(&scene, "refused rather than broken"),
            "the card drew the plugin's own controls without saying they cannot work: {}",
            frame_text(&scene)
        );

        // In the box the controls are in, under them, under the caption that
        // says whose they are: a line of prose floating between two boxes
        // belongs to whichever one the reader guesses.
        let (caption, _) = page_line(&scene, "What it is doing");
        let (row, _) = page_line(&scene, "from a sandbox");
        // By the note's first words: the phrase above is on its second line
        // in this window, and a line is one line.
        let (note, _) = page_line(&scene, "Nothing this plugin asks for");
        let status = answer_boxes(&scene)
            .into_iter()
            .find(|boxed| boxed.contains_point(row))
            .expect("the plugin's row is in a box");
        assert!(
            status.contains_point(caption),
            "the caption is outside the box"
        );
        assert!(status.contains_point(note), "the note is outside the box");
        assert!(
            caption.y() < row.y() && row.y() < note.y(),
            "caption, row and note are not in that order"
        );
    }

    #[test]
    fn a_plugin_that_drew_its_own_controls_is_not_listed_twice() {
        // Its own row says what it can be asked to do, in the shape it chose.
        // The card counting the same commands underneath is the longest
        // section on the page saying what the row above it already said.
        let scratch = Scratch::new("counted");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "plugins.card.status", 10),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();

        assert!(
            says(&scene, "1 command, which the command palette lists."),
            "the list was not counted: {}",
            frame_text(&scene)
        );
        assert!(
            !says(&scene, "Poke the probe"),
            "the command is listed under the controls that already offer it: {}",
            frame_text(&scene)
        );
    }

    #[test]
    fn a_plugin_that_drew_nothing_of_its_own_still_gets_the_whole_list() {
        // The same plugin and the same command, contributed somewhere else.
        // Nothing on this card offers it, so the card does — which is what
        // keeps the section from being hidden by a rule about chips.
        let scratch = Scratch::new("listed-in-full");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();
        harness.click_plugin("Probe");
        let scene = harness.frame();

        assert!(says(&scene, "Poke the probe"), "{}", frame_text(&scene));
        assert!(
            !says(&scene, "which the command palette lists"),
            "{}",
            frame_text(&scene)
        );
    }

    #[test]
    fn a_panel_the_plugin_has_already_shut_leaves_the_screen() {
        // The one thing a press does that a dismissal does not: notify. A
        // guest's state is inside the module, so nothing out here can tell
        // that running an action changed what it draws — and a plugin's panel
        // is exactly that state. The dismissal ran, the guest shut the panel,
        // and the frame went on drawing it, modal underlay and all, over the
        // plugin's own controls. Every press after that was eaten by a menu
        // that was not there.
        let scratch = Scratch::new("dismissed");
        install(
            scratch.path(),
            "eugen.probe",
            &crate::plugins::wasm::tests::wasm_with_a_panel(
                "eugen/probe",
                "plugins.card.status",
                10,
            ),
        );
        let mut harness = harness(&scratch);
        harness.show_plugins();
        harness.click_plugin("Probe");

        let scene = harness.frame();
        let chip = scene
            .layers()
            .flat_map(|layer| layer.icons.iter())
            .find(|drawn| drawn.icon_key.mark == Mark::Icon(Lucide::ChevronDown))
            .expect("the plugin's own chip should have been drawn")
            .bounds;
        harness.click(chip.origin() + chip.size() / 2., MouseButton::Left);
        assert!(
            says(&harness.frame(), "the panel is up"),
            "the chip did not open the panel, so this proves nothing"
        );

        // A press in the corner, which is what shuts a menu. The guest hears
        // it — the dismissal names an action and the action ran — so what is
        // being asked here is only whether anybody drew the answer.
        harness.click(vec2f(1000., 100.), MouseButton::Left);
        let scene = harness.frame();
        assert!(
            !says(&scene, "the panel is up"),
            "a panel the plugin shut is still on screen: {}",
            frame_text(&scene)
        );
    }

    #[test]
    fn a_sandboxed_plugins_action_is_reachable_by_name_like_any_other() {
        // Prefixed by the host with the plugin's own id, so a guest cannot
        // claim an action belonging to anybody else however it spells its own.
        let scratch = Scratch::new("action");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 10),
        );
        let mut harness = harness(&scratch);

        harness.press("p", palette_chord(), "");
        harness.frame();
        harness.type_text("poke");
        let text = frame_text(&harness.frame());

        assert!(text.contains("Poke the probe"), "{text}");
        assert!(text.contains("eugen/probe/poke"), "{text}");
    }

    #[test]
    fn what_a_plugin_pins_to_the_header_clears_the_right_corner_this_platform_reserves() {
        // The other end of
        // `the_header_reserves_the_left_corner_this_platform_puts_its_controls_in_and_no_more`.
        // Nothing a release binary carries goes in this slot, so the only way
        // to see the reservation honoured is to put something there — which is
        // also the honest statement of the rule: the room is kept for whatever
        // fills the slot, whoever wrote it.
        let scratch = Scratch::new("corner");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "header.right", 0),
        );
        let mut harness = harness(&scratch);
        let insets = harness.window_insets();
        let scene = harness.frame();
        let edge = pinned_right_edge(&scene);

        assert!(
            WINDOW.x() - edge >= insets.header_right,
            "what the plugin pinned runs into the {} reserved on the right",
            insets.header_right
        );
        // The reservation and the header's own 10px padding, and nothing else.
        assert!(
            WINDOW.x() - edge < insets.header_right + 24.,
            "what the plugin pinned stops {} short of the right edge",
            WINDOW.x() - edge
        );
    }

    /// The right edge of the last thing a plugin drew in the header row.
    ///
    /// The row paints its own ground and its own rule, and both run the whole
    /// width of it; anything narrower is content, and the right edge of the
    /// content is what a reservation at that end has to clear.
    fn pinned_right_edge(scene: &Scene) -> f32 {
        let header = header_box(scene);
        visible_rects(scene)
            .map(|(_, drawn)| drawn)
            .filter(|drawn| contains(header, *drawn) && drawn.width() < header.width())
            .map(|drawn| drawn.max_x())
            .max_by(f32::total_cmp)
            .expect("the plugin drew something in the header")
    }

    #[test]
    fn a_contribution_to_a_slot_this_build_does_not_have_is_refused_and_nothing_else() {
        // A plugin written against a Crook with a slot this one does not have
        // should be missing that one contribution, not missing entirely.
        let scratch = Scratch::new("unknown-slot");
        install(
            scratch.path(),
            "eugen.probe",
            &wasm("eugen/probe", "somewhere.else", 0),
        );
        let mut harness = harness(&scratch);

        let text = frame_text(&harness.frame());

        assert!(!text.contains("probed"), "it drew somewhere: {text}");
        // Still loaded, still listed, still offering its action.
        assert!(
            harness
                .workspace
                .read(&harness.app, |workspace, _| workspace.host().is_loaded(
                    &crate::plugin::PluginId::parse("eugen/probe").expect("a literal")
                )),
            "the plugin was thrown away over one contribution"
        );
    }

    /// The Store, opened on a scratch index: what its rows and its card say
    /// about the plugins on this machine, and what a press there does.
    mod store_section {
        use super::*;
        use crate::plugins::store::cache::Cache;
        use crate::plugins::store::index::Busy;

        /// The Store's key on the sidebar.
        const STORE: &str = "crook/store/section";

        /// A registry listing the probe at `version`, asking to reach
        /// `example.com`, with the `extra` fields on the plugin's row and
        /// the version's — an icon, previews, a size — written into the
        /// scratch cache.
        fn listing(scratch: &Scratch, version: &str, extra: (&str, &str)) -> Cache {
            let (on_the_plugin, on_the_version) = extra;
            let cache = Cache::at(scratch.path());
            cache
                .write(
                    format!(
                        r#"{{"schema": 1, "plugins": [
                         {{"id": "eugen/probe", "name": "Probe", "description": "d",
                          "repository": "https://example.com/probe", {on_the_plugin}
                          "versions": [{{"version": "{version}", "abi": 8,
                                        "url": "https://github.com/theguriev/crook-plugins/releases/download/index/eugen.probe-{version}.wasm",
                                        "sha256": "aa", {on_the_version}
                                        "capabilities": ["net:example.com"],
                                        "asks": ["Reach example.com"]}}]}}]}}"#
                    )
                    .as_bytes(),
                    None,
                )
                .expect("the test index parses");
            cache
        }

        /// The rooms and pictures inside the panel — the list's — rather
        /// than the card's.
        fn in_panel(scene: &Scene, boxes: Vec<RectF>) -> Vec<RectF> {
            let panel = panel_box(scene);
            boxes
                .into_iter()
                .filter(|bounds| bounds.max_x() <= panel.max_x())
                .collect()
        }

        #[test]
        fn a_row_with_a_face_in_the_list_draws_it_twelve_pixels_tall() {
            // The face rides in the list as base64 and is decoded on the
            // pool: nothing in the row before it lands, then the icon in
            // the row's box at the row's own size, and the rows one height.
            let plugins = Scratch::new("store-face");
            let index = Scratch::new("store-face-index");
            let icon = {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(icon_png(64))
            };
            let cache = listing(&index, "1.0.0", (&format!(r#""icon": "{icon}","#), ""));
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );
            harness.show_section(STORE);

            harness.wait_for("the icon to be decoded", |harness| {
                !in_panel(&harness.frame(), images(&harness.frame())).is_empty()
            });
            let scene = harness.frame();
            let in_rows = in_panel(&scene, images(&scene));
            assert_eq!(in_rows.len(), 1, "one plugin carries a face");
            assert!(
                (in_rows[0].height() - widgets::ROW_ICON).abs() < 0.5,
                "a row's icon is {} tall",
                in_rows[0].height()
            );
            let rows = settings_rail_boxes(&scene);
            assert!(
                rows.iter()
                    .all(|row| (row.height() - rows[0].height()).abs() < 0.5),
                "the rows are not one height: {rows:?}"
            );

            // And the same face, eighteen pixels tall, before the card's
            // title.
            let pane = settings_pane_box(&scene);
            let mark = images(&scene)
                .into_iter()
                .find(|bounds| pane.contains_point(center(*bounds)))
                .expect("the card draws the icon before its title");
            assert!((mark.height() - widgets::TITLE_MARK).abs() < 0.5);
        }

        #[test]
        fn a_version_taken_back_is_offered_its_replacement_on_the_store_card_too() {
            // The same answer the plugin's own card gives, from the same
            // `change`: the row ends in the replacement, the label says why
            // the running one was taken back, and the button is live — a
            // press starts the fetch, which the card says on the next frame.
            let plugins = Scratch::new("store-replaced");
            let index = Scratch::new("store-replaced-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm("eugen/probe", "header.right", 10),
            );
            let cache = listing(&index, "0.0.9", ("", ""));
            let withdrawn = [(
                String::from("eugen/probe"),
                String::from("it read the wrong file"),
            )]
            .into();
            let mut harness =
                Harness::with_store(1, opening(&plugins, withdrawn, Default::default()), cache);
            harness.show_section(STORE);
            harness.click_plugin("Probe");
            let scene = harness.frame();
            let text = frame_text(&scene);

            assert!(says(&scene, "Taken back: it read the wrong file"), "{text}");
            assert!(says(&scene, "Install 0.0.9"), "{text}");
            assert!(!says(&scene, "Installed"), "{text}");
            assert!(
                probe_row(&scene).contains("0.0.9"),
                "the row does not end in the replacement: {:?}",
                probe_row(&scene)
            );

            harness.click_page_button("Install 0.0.9");
            let scene = harness.frame();
            assert_eq!(harness.store_downloading(), Some(probe()));
            assert!(says(&scene, "Getting it"), "{}", frame_text(&scene));
            assert!(says(&scene, "Downloading"), "{}", frame_text(&scene));
        }

        #[test]
        fn a_registry_that_is_behind_offers_no_update_on_the_store_card() {
            let plugins = Scratch::new("store-behind");
            let index = Scratch::new("store-behind-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm("eugen/probe", "header.right", 10),
            );
            let cache = listing(&index, "0.0.9", ("", ""));
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );
            harness.show_section(STORE);
            let scene = harness.frame();
            let text = frame_text(&scene);

            assert!(says(&scene, "Version 0.1.0"), "{text}");
            assert!(says(&scene, "Installed"), "{text}");
            assert!(!says(&scene, "Update to"), "{text}");
            assert!(!says(&scene, "Install 0.0.9"), "{text}");
            assert!(says(&scene, "On this machine"), "{text}");
            assert!(!says(&scene, "update in the registry"), "{text}");
        }

        #[test]
        fn update_all_takes_every_update_the_registry_has_one_module_at_a_time() {
            // Plan test (6): the Plugins list's foot counts the updates and
            // offers them all, the card offers the one, and pressing either
            // is the store's own fetch — one in flight, the rest waiting,
            // and every surface hearing it on the next frame.
            let plugins = Scratch::new("store-update-all");
            let index = Scratch::new("store-update-all-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm("eugen/probe", "header.right", 10),
            );
            let cache = listing(&index, "0.2.0", ("", ""));
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );
            harness.show_plugins();
            harness.click_plugin("Probe");
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "A newer version is in the registry"), "{text}");
            assert!(says(&scene, "Update to 0.2.0"), "{text}");
            assert!(!says(&scene, "The Store is switched off"), "{text}");
            assert!(says(&scene, "1 update in the registry"), "{text}");
            assert!(says(&scene, "Update all"), "{text}");
            assert_eq!(harness.store_downloading(), None);

            harness.click_sidebar_button("Update all");
            assert_eq!(harness.store_downloading(), Some(probe()));
            let scene = harness.frame();
            let heard = harness
                .workspace
                .read(&harness.app, |workspace, _| workspace.heard().clone());
            assert_eq!(heard.busy(&probe()), Some(Busy::Downloading));
            assert!(says(&scene, "Getting it"), "{}", frame_text(&scene));
            // Remove is dead while the fetch is in flight: a removal that
            // landed under it would be undone by the download landing.
            assert_eq!(
                page_line_color(&scene, "On this machine"),
                theme().text_muted,
                "Remove is live under a download: {}",
                frame_text(&scene)
            );
            // And so is this list's own Update all, since everything it
            // counts is already on its way.
            harness.click_sidebar_button("Update all");
            assert!(
                harness.store_queued().is_empty(),
                "a dead button queued a second fetch"
            );

            // The Store says the same of the same plugin, and its own
            // update-all is dead while everything is already on its way.
            harness.show_section(STORE);
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "Downloading"), "{text}");
            assert!(says(&scene, "Getting it"), "{text}");
            assert!(says(&scene, "1 update in the registry"), "{text}");
            harness.click_sidebar_button("Update all");
            assert!(
                harness.store_queued().is_empty(),
                "a dead button queued a second fetch"
            );
            assert_eq!(harness.store_downloading(), Some(probe()));
        }

        #[test]
        fn the_update_button_on_the_plugins_card_is_the_stores_fetch() {
            // The one control the page was asked for, pressed where it was
            // asked for: the card's own Update runs the Store's action about
            // this plugin, and the Store starts fetching it.
            let plugins = Scratch::new("plugins-update");
            let index = Scratch::new("plugins-update-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm("eugen/probe", "header.right", 10),
            );
            let cache = listing(&index, "0.2.0", ("", ""));
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );
            harness.show_plugins();
            harness.click_plugin("Probe");
            assert_eq!(harness.store_downloading(), None);

            harness.click_page_button("Update to 0.2.0");
            assert_eq!(harness.store_downloading(), Some(probe()));
            assert!(says(&harness.frame(), "Getting it"));
        }

        #[test]
        fn with_the_store_switched_off_the_card_says_so_and_the_list_counts_nothing() {
            // The card still says the registry is ahead, because that is
            // true of the plugin; what it cannot do is fetch, and the button
            // is dead with the reason under the row. The list's foot offers
            // nothing: a count is worth a line only beside a way to act on
            // it.
            let plugins = Scratch::new("store-off");
            let index = Scratch::new("store-off-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm("eugen/probe", "header.right", 10),
            );
            let cache = listing(&index, "0.2.0", ("", ""));
            let mut opening = opening(&plugins, Default::default(), Default::default());
            opening.settings.set_plugin_disabled("crook/store", true);
            let mut harness = Harness::with_store(1, opening, cache);
            harness.show_plugins();
            harness.click_plugin("Probe");
            let scene = harness.frame();
            let text = frame_text(&scene);

            assert!(says(&scene, "A newer version is in the registry"), "{text}");
            assert!(says(&scene, "Update to 0.2.0"), "{text}");
            assert!(
                says(&scene, "The Store is switched off, and it is what fetches."),
                "{text}"
            );
            assert!(!says(&scene, "update in the registry"), "{text}");
            assert!(!says(&scene, "Update all"), "{text}");
            // The Store's row is still a fact about the plugin; the button
            // to it is as dead as the update.
            assert!(says(&scene, "Show in Store"), "{text}");
            assert_eq!(
                page_line_color(&scene, "In the registry"),
                theme().text_muted
            );
            assert_eq!(page_line_color(&scene, "Version 0.1.0"), theme().text_muted);

            harness.click_page_button("Update to 0.2.0");
            assert!(
                harness.store_model().is_none(),
                "a store switched off has a model"
            );
        }

        #[test]
        fn an_update_lands_is_installed_and_runs_and_the_next_in_the_queue_starts() {
            // The whole path the feature is for, through the pool: the
            // fetcher answers with the module the index promised, the
            // completion lands it, the observer checks, writes and carries
            // it, the switch is registered again for the new version, the
            // grant is kept — and the completion is what starts the next
            // download waiting behind it.
            let plugins = Scratch::new("store-lands");
            let index = Scratch::new("store-lands-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm("eugen/probe", "header.right", 10),
            );
            let mut other = manifest("eugen/other");
            other.name = String::from("Other");
            install(
                plugins.path(),
                "eugen.other",
                &wasm_saying(&other, "header.right", 11),
            );
            let newer_probe = wasm_at("eugen/probe", "0.2.0");
            other.version = String::from("0.2.0");
            let newer_other = wasm_saying(&other, "header.right", 11);
            let row = |id: &str, name: &str, module: &[u8]| {
                format!(
                    r#"{{"id": "{id}", "name": "{name}", "description": "d",
                     "versions": [{{"version": "0.2.0", "abi": 8,
                                   "url": "{}{}-0.2.0.wasm",
                                   "sha256": "{}", "bytes": {},
                                   "capabilities": ["tabs.read"],
                                   "asks": ["See what your tabs are called"]}}]}}"#,
                    crate::plugins::store::fetch::assets_prefix(),
                    id.replace('/', "."),
                    crate::plugins::store::fetch::sha256_hex(module),
                    module.len()
                )
            };
            let cache = Cache::at(index.path());
            cache
                .write(
                    format!(
                        r#"{{"schema": 1, "plugins": [{}, {}]}}"#,
                        row("eugen/probe", "Probe", &newer_probe),
                        row("eugen/other", "Other", &newer_other)
                    )
                    .as_bytes(),
                    None,
                )
                .expect("the test index parses");

            // Answered by URL, and checked the way the real fetch checks
            // what arrived against what the list promised.
            let modules: Vec<(String, Vec<u8>)> = vec![
                (String::from("eugen.probe-0.2.0.wasm"), newer_probe),
                (String::from("eugen.other-0.2.0.wasm"), newer_other),
            ];
            let fetch: crate::plugins::store::model::Fetcher = Arc::new(move |release| {
                let (_, module) = modules
                    .iter()
                    .find(|(name, _)| release.url.ends_with(name.as_str()))
                    .ok_or_else(|| format!("nothing is served at {}", release.url))?;
                crate::plugins::store::fetch::checked(module.clone(), release)
            });
            let mut opening = opening(&plugins, Default::default(), Default::default());
            opening
                .settings
                .set_granted("eugen/probe", vec![String::from("tabs.read")]);
            let mut harness = Harness::with_store_fetching(1, opening, cache, fetch);
            harness.show_plugins();
            // By name rather than by a click on its row: the row is the last
            // of sixteen, and on a macOS window the strip the panel reserves
            // for the traffic lights pushes it under the fold, where a click
            // lands on nothing. Which card is up is not what this test is
            // about.
            harness.run_about("crook/plugins/show", "eugen/probe");
            let scene = harness.frame();
            assert!(
                says(&scene, "2 updates in the registry"),
                "{}",
                frame_text(&scene)
            );
            assert!(says(&scene, "Update to 0.2.0"), "{}", frame_text(&scene));

            harness.click_sidebar_button("Update all");
            assert!(harness.store_downloading().is_some());
            assert_eq!(
                harness.store_queued().len(),
                1,
                "one in flight, one waiting"
            );

            harness.wait_for("both updates to land", |harness| {
                harness.frame();
                harness.store_downloading().is_none()
                    && plugins
                        .path()
                        .join("eugen.other/0.2.0/plugin.wasm")
                        .is_file()
            });
            harness.wait_for("the window to draw the new version", |harness| {
                says(&harness.frame(), "0.2.0")
            });

            assert!(
                plugins
                    .path()
                    .join("eugen.probe/0.2.0/plugin.wasm")
                    .is_file()
            );
            assert!(harness.store_queued().is_empty());
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "eugen/probe \u{b7} 0.2.0"), "{text}");
            assert!(!says(&scene, "Update to 0.2.0"), "{text}");
            assert!(!says(&scene, "update in the registry"), "{text}");
            assert!(!says(&scene, "updates in the registry"), "{text}");
            harness.workspace.read(&harness.app, |workspace, _| {
                assert_eq!(
                    workspace.settings().granted_to("eugen/probe"),
                    ["tabs.read"],
                    "the grant did not survive the update"
                );
                assert!(workspace.host().is_loaded(&probe()), "0.2.0 is not running");
                let toggle =
                    ActionName::parse("crook/plugins/toggle-eugen-probe").expect("a literal");
                assert!(
                    workspace.host().action(&toggle).is_some(),
                    "the switch was not registered again for the new version"
                );
                let other = crook_plugin::PluginId::parse("eugen/other").expect("a literal");
                assert!(
                    workspace.host().is_loaded(&other),
                    "0.2.0 of the other is not running"
                );
            });
        }

        #[test]
        fn the_update_button_on_the_store_card_is_the_same_fetch() {
            let plugins = Scratch::new("store-update");
            let index = Scratch::new("store-update-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm("eugen/probe", "header.right", 10),
            );
            let cache = listing(&index, "0.2.0", ("", ""));
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );
            harness.show_section(STORE);
            harness.click_plugin("Probe");
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "0.1.0 installed, 0.2.0 out"), "{text}");
            assert!(says(&scene, "Update to 0.2.0"), "{text}");

            harness.click_page_button("Update to 0.2.0");
            assert_eq!(harness.store_downloading(), Some(probe()));
            assert!(says(&harness.frame(), "Getting it"));
        }

        #[test]
        fn fetching_the_pictures_of_a_plugin_not_here_says_what_it_costs_and_holds_their_room() {
            // The one button in the store that downloads without installing.
            // Before the press the note says it is the plugin itself being
            // fetched, and how big; after it the button is dead and the room
            // for each picture is drawn at the size the index gave, halved.
            let plugins = Scratch::new("store-fetch-pictures");
            let index = Scratch::new("store-fetch-pictures-index");
            let cache = listing(
                &index,
                "1.0.0",
                (
                    "",
                    r#""bytes": 367410, "previews": [{"width": 640, "height": 128}, {"width": 400, "height": 300}],"#,
                ),
            );
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );
            harness.show_section(STORE);
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "What it looks like"), "{text}");
            assert!(says(&scene, "2 pictures inside"), "{text}");
            assert!(says(&scene, "Fetch pictures"), "{text}");
            assert!(
                says(&scene, "Seeing them is fetching the plugin itself — 367 KB"),
                "{text}"
            );
            assert!(
                reserved(&scene).is_empty(),
                "nothing is reserved before the press"
            );

            harness.click_page_button("Fetch pictures");
            let waiting = harness.frame();
            assert!(says(&waiting, "Fetching"), "{}", frame_text(&waiting));
            assert!(
                !says(&waiting, "Fetch pictures"),
                "{}",
                frame_text(&waiting)
            );
            let mut rooms = reserved(&waiting);
            rooms.sort_by(|left, right| left.min_y().total_cmp(&right.min_y()));
            assert_eq!(rooms.len(), 2, "one room per picture: {rooms:?}");
            assert!(
                (rooms[0].width() - 320.).abs() < 0.5 && (rooms[0].height() - 64.).abs() < 0.5,
                "the first room is not the capture halved: {:?}",
                rooms[0]
            );
            assert!(
                (rooms[1].width() - 200.).abs() < 0.5 && (rooms[1].height() - 150.).abs() < 0.5,
                "the second room is not the capture halved: {:?}",
                rooms[1]
            );
            assert_eq!(
                harness.store_downloading(),
                None,
                "looking is not installing"
            );

            // And a fetch that brings nothing says so in the sentence a
            // download that brings nothing says it in, under the plugin's
            // name — not the fetch's own words after the name, which read as
            // "Probe http status: 404" — with the button live again.
            harness.wait_for("the refusal to land", |harness| {
                says(&harness.frame(), "did not arrive")
            });
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(
                says(&scene, "Probe did not arrive: a test reaches no network"),
                "{text}"
            );
            assert!(says(&scene, "Fetch pictures"), "{text}");
            assert!(!says(&scene, "Fetching"), "{text}");
            assert!(reserved(&scene).is_empty(), "the rooms outlived the fetch");
        }

        #[test]
        fn show_in_store_turns_the_card_to_the_plugin_named_whatever_the_field_says() {
            // The card is resolved against the rows the field lets through,
            // and a row the field hides is a card about the first row it
            // does not. Asked for the probe while the field hides it, the
            // store empties the field and turns to the probe — whether the
            // asking is a plugin running the action while the store is
            // showing, or a person pressing Show in Store on the Plugins
            // page, which leaves the store's field behind on the way.
            let plugins = Scratch::new("store-show");
            let index = Scratch::new("store-show-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm("eugen/probe", "header.right", 10),
            );
            let cache = Cache::at(index.path());
            cache
                .write(
                    br#"{"schema": 1, "plugins": [
                     {"id": "eugen/probe", "name": "Probe", "description": "d",
                      "repository": "https://example.com/probe",
                      "versions": [{"version": "0.1.0", "abi": 8,
                                    "url": "https://github.com/theguriev/crook-plugins/releases/download/index/eugen.probe-0.1.0.wasm",
                                    "sha256": "aa"}]},
                     {"id": "eugen/zebra", "name": "Zebra", "description": "d",
                      "repository": "https://example.com/zebra",
                      "versions": [{"version": "1.0.0", "abi": 8,
                                    "url": "https://github.com/theguriev/crook-plugins/releases/download/index/eugen.zebra-1.0.0.wasm",
                                    "sha256": "aa"}]}]}"#,
                    None,
                )
                .expect("the test index parses");
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );

            harness.show_section(STORE);
            harness.click_section_field();
            harness.type_text("zebra");
            let scene = harness.frame();
            assert!(says(&scene, "eugen/zebra"), "{}", frame_text(&scene));
            assert!(!says(&scene, "eugen/probe"), "{}", frame_text(&scene));

            harness.run_about("crook/store/show", "eugen/probe");
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "eugen/probe"), "{text}");
            assert!(!says(&scene, "eugen/zebra"), "{text}");
            assert!(says(&scene, "Zebra"), "the field was not emptied: {text}");

            harness.click_section_field();
            harness.type_text("zebra");
            assert!(!says(&harness.frame(), "eugen/probe"));
            harness.show_plugins();
            harness.click_plugin("Probe");
            harness.click_page_button("Show in Store");
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "eugen/probe"), "{text}");
            assert!(!says(&scene, "eugen/zebra"), "{text}");
            assert!(says(&scene, "Zebra"), "the field was not emptied: {text}");
        }

        #[test]
        fn a_window_a_test_opens_carries_a_store_that_has_read_nothing() {
            // Every harness, not only one opened with a store: the one in
            // the box reads the list of whoever is running the tests, and
            // the first face decoded off it would have the window hearing
            // their offers in place of what the test said.
            let mut harness = Harness::new(1);
            let state = harness.store.clone().expect("the window carries a store");
            let model = state.model().expect("the store has built");
            let (offers, fetched) = model.read(&harness.app, |model, _| {
                (model.offers().len(), model.fetched())
            });
            assert_eq!(offers, 0, "the store read somebody's list");
            assert_eq!(fetched, None);

            harness.show_section(STORE);
            let scene = harness.frame();
            assert!(says(&scene, "Nothing here yet"), "{}", frame_text(&scene));
            assert!(says(&scene, "Never looked"), "{}", frame_text(&scene));
        }

        #[test]
        fn the_pictures_of_a_plugin_already_here_are_shown_from_its_own_module() {
            // Installed, and the module carries previews: no note about
            // fetching, "Show" rather than "Fetch", and the pictures drawn
            // out of the module on this machine once the pool has decoded
            // them.
            let plugins = Scratch::new("store-show-pictures");
            let index = Scratch::new("store-show-pictures-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm_carrying(
                    &manifest("eugen/probe"),
                    "header.right",
                    10,
                    &[
                        ("crook.preview.1", &preview_png(400, 300)),
                        ("crook.caption.1", b"The chip in the header"),
                    ],
                ),
            );
            let cache = listing(&index, "0.1.0", ("", ""));
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );
            harness.show_section(STORE);
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "1 picture inside"), "{text}");
            assert!(says(&scene, "Show picture"), "{text}");
            assert!(
                !says(&scene, "Show pictures"),
                "one picture, one word: {text}"
            );
            assert!(!says(&scene, "fetching the plugin itself"), "{text}");
            assert!(
                !says(&scene, "the version on this machine"),
                "the registry offers the version that is running, so its pictures are its \
                 own: {text}"
            );

            harness.click_page_button("Show picture");
            assert!(says(&harness.frame(), "Opening"));
            harness.wait_for("the picture to be decoded", |harness| {
                let scene = harness.frame();
                let pane = settings_pane_box(&scene);
                images(&scene)
                    .into_iter()
                    .any(|bounds| pane.contains_point(center(bounds)))
            });
            let scene = harness.frame();
            let pane = settings_pane_box(&scene);
            let drawn: Vec<RectF> = images(&scene)
                .into_iter()
                .filter(|bounds| pane.contains_point(center(*bounds)))
                .collect();
            assert!(
                drawn
                    .iter()
                    .any(|bounds| (bounds.width() - 200.).abs() < 0.5
                        && (bounds.height() - 150.).abs() < 0.5),
                "no picture is drawn at half its captured size: {drawn:?}"
            );
            assert!(
                says(&scene, "The chip in the header"),
                "{}",
                frame_text(&scene)
            );
            assert_eq!(harness.store_downloading(), None);
            // Landed: the button says so and is dead, since a press would
            // change nothing but spend a decode.
            assert!(says(&scene, "Shown"), "{}", frame_text(&scene));
            assert!(!says(&scene, "Show picture"), "{}", frame_text(&scene));
        }

        #[test]
        fn the_pictures_of_the_version_here_are_named_when_the_card_offers_another() {
            // The card offers 0.2.0 and the pictures are 0.1.0's, out of the
            // module on this machine: somebody deciding on the update is
            // told which version they are looking at, and how many the
            // offered one lists. The note under the fetch names the verb
            // the card's own button uses.
            let plugins = Scratch::new("store-old-pictures");
            let index = Scratch::new("store-old-pictures-index");
            install(
                plugins.path(),
                "eugen.probe",
                &wasm_carrying(
                    &manifest("eugen/probe"),
                    "header.right",
                    10,
                    &[("crook.preview.1", &preview_png(400, 300))],
                ),
            );
            let cache = listing(
                &index,
                "0.2.0",
                (
                    "",
                    r#""previews": [{"width": 640, "height": 128}, {"width": 400, "height": 300}],"#,
                ),
            );
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );
            harness.show_section(STORE);
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "Update to 0.2.0"), "{text}");
            assert!(says(&scene, "1 picture inside"), "{text}");
            assert!(
                says(
                    &scene,
                    "The pictures of 0.1.0, the version on this machine; 0.2.0 lists 2."
                ),
                "{text}"
            );
        }

        #[test]
        fn a_list_of_sizes_past_the_rule_is_held_to_it_before_a_room_is_laid_out() {
            // The index is a mirror, and a mirror somebody else wrote: seven
            // sizes where a module may carry six, and sizes no picture can
            // have, are held to the module's own rule before the card lays
            // out a room for each — or a hostile list is three thousand
            // rooms a frame for as long as the fetch takes.
            let plugins = Scratch::new("store-many-sizes");
            let index = Scratch::new("store-many-sizes-index");
            let sizes: Vec<String> = std::iter::once(String::from(
                r#"{"width": 4294967295, "height": 0}, {"width": 0, "height": 4294967295}"#,
            ))
            .chain((0..7).map(|_| String::from(r#"{"width": 400, "height": 300}"#)))
            .collect();
            let cache = listing(
                &index,
                "1.0.0",
                ("", &format!(r#""previews": [{}],"#, sizes.join(", "))),
            );
            let mut harness = Harness::with_store(
                1,
                opening(&plugins, Default::default(), Default::default()),
                cache,
            );
            harness.show_section(STORE);
            let scene = harness.frame();
            let text = frame_text(&scene);
            assert!(says(&scene, "6 pictures inside"), "{text}");
            assert!(says(&scene, "the same file Install fetches"), "{text}");

            harness.click_page_button("Fetch pictures");
            let waiting = harness.frame();
            assert_eq!(
                reserved(&waiting).len(),
                6,
                "a room per picture the rule allows: {}",
                frame_text(&waiting)
            );
        }
    }
}

/// The Plugins page: a list on the left, a card on the right.
mod plugins_page {
    use super::*;

    /// A harness showing the Plugins section of the sidebar.
    fn harness() -> Harness {
        let mut harness = Harness::new(1);
        harness.show_plugins();
        harness
    }

    #[test]
    fn a_native_plugin_has_no_box_about_this_machine() {
        // A native plugin is the binary: it is not in a registry, it is not
        // a file in the plugins directory, and the About page is what says
        // whether the binary is behind. Its card has the switch and nothing
        // that could update or remove it.
        let mut harness = harness();
        let scene = harness.frame();

        assert!(says(&scene, "crook/window"), "{}", frame_text(&scene));
        assert_eq!(answer_boxes(&scene).len(), 1, "the switch's box, alone");
        assert!(!says(&scene, "On this machine"));
        assert!(!says(&scene, "In the registry"));
        assert!(!says(&scene, "Remove"));
    }

    #[test]
    fn a_remove_named_by_hand_for_a_native_plugin_is_refused_on_its_card() {
        // The card never offers Remove for one of Crook's own, so this is
        // the command line naming it. Nothing is removed, and the card says
        // so under its switch — the one box a native card has — rather than
        // in a log line nobody reads.
        let mut harness = harness();
        harness.run_about("crook/plugins/remove", "crook/window");

        let scene = harness.frame();
        assert!(
            says(&scene, "was not removed: is one of Crook's own"),
            "{}",
            frame_text(&scene)
        );
        let boxes = answer_boxes(&scene);
        assert_eq!(boxes.len(), 1, "the refusal conjured a box");
        let (refusal, _) = page_line(&scene, "was not removed");
        assert!(
            refusal.y() > boxes[0].max_y(),
            "the refusal is not under the switch"
        );
        assert_eq!(
            page_line_color(&scene, "was not removed"),
            theme().usage_critical
        );

        // And it is about the plugin it was about: another card does not
        // carry it.
        harness.click_plugin("Tabs");
        assert!(!says(&harness.frame(), "was not removed"));
    }

    #[test]
    fn the_card_says_the_state_before_the_id() {
        // The state is what the card is opened for, and it used to be the
        // last word of the facts line, after the id and the version.
        let mut harness = harness();
        let (_, line) = page_line(&harness.frame(), "crook/window");

        let state = line.find("running").expect("the facts line says the state");
        let id = line
            .find("crook/window")
            .expect("the facts line says the id");
        assert!(state < id, "the state comes after the id: {line}");

        // Lit, where the rest of the line is not: the state is the one word
        // on the line the card is opened for.
        let scene = harness.frame();
        assert_eq!(page_line_color(&scene, "running"), theme().text_primary);
    }

    #[test]
    fn a_slot_is_named_once_however_much_is_in_it() {
        // Eight entries in one menu used to be eight lines each starting with
        // the menu's name. The slot is one fact; what is in it goes under it.
        let mut harness = harness();
        harness.click_plugin("Tabs");
        let scene = harness.frame();

        let named = page_lines(&scene)
            .into_iter()
            .filter(|(_, line)| line.contains("tab.menu.entries"))
            .count();
        assert_eq!(
            named,
            1,
            "the slot is named more than once: {}",
            frame_text(&scene)
        );
        // The last entries, not the first: a line that was cut off at the
        // measure would still start with pin-tab.
        assert!(
            says(&scene, "close-tab, color"),
            "the last entries were cut off: {}",
            frame_text(&scene)
        );
    }

    #[test]
    fn a_plugin_that_holds_the_page_says_so_above_its_switch() {
        // The sentence is about the switch, so it sits with the switch — in
        // the same box, above it — and not at the end of the description.
        let mut harness = harness();
        harness.click_plugin("Plugins");
        let scene = harness.frame();

        let switch = settings_switch_boxes(&scene)[0];
        let enabled = answer_boxes(&scene)
            .into_iter()
            .find(|boxed| boxed.contains_point(center(switch)))
            .expect("the switch is in a box");
        let (at, _) = page_line(&scene, "It is what draws the page");
        assert!(
            enabled.contains_point(at),
            "the reason is not in the switch's box"
        );
        assert!(
            at.y() < switch.min_y(),
            "the reason is under the switch it explains"
        );

        let (_, description) = page_line(&scene, "What this build is made of");
        assert!(
            !description.contains("It is what draws"),
            "the reason is still in the description: {description}"
        );
    }

    #[test]
    fn the_page_is_a_list_of_every_plugin_beside_a_card_about_one() {
        let mut harness = harness();
        let text = frame_text(&harness.frame());

        // Every plugin the binary carries is in the list, whether or not it
        // is running: a switch you cannot see is a switch you cannot turn
        // back on.
        for name in [
            "Window commands",
            "Header",
            "Shell settings",
            "Command palette",
            "About",
        ] {
            assert!(text.contains(name), "{name} is not in the list: {text}");
        }

        // And the card is about the first of them, rather than empty: a card
        // saying "choose something" is a card explaining an interface instead
        // of being one.
        assert!(text.contains("crook/window"), "no card: {text}");
        assert!(text.contains("Built in"), "{text}");
        assert!(text.contains("Enabled"), "{text}");
    }

    #[test]
    fn clicking_a_row_shows_that_plugin() {
        let mut harness = harness();

        harness.click_plugin("Command palette");
        let text = frame_text(&harness.frame());

        assert!(text.contains("crook/palette"), "{text}");
        assert!(
            text.contains("Everything the window and its plugins can be asked to do"),
            "the card does not describe it: {text}"
        );
        // What it puts on screen, asked of the host rather than of the plugin.
        assert!(text.contains("window.overlay"), "{text}");
        assert!(text.contains("crook/palette/open"), "{text}");
    }

    #[test]
    fn the_sidebar_field_narrows_the_list() {
        let mut harness = harness();
        harness.click_section_field();
        harness.type_text("palette");
        let text = frame_text(&harness.frame());

        assert!(text.contains("Command palette"), "{text}");
        assert!(
            !text.contains("Window commands"),
            "the list did not narrow: {text}"
        );
        // And the sidebar is the plugin list rather than the settings rail:
        // they are two sections, not two halves of one screen.
        assert!(
            !frame_text(&harness.frame()).contains("Appearance"),
            "the settings rail is in the sidebar while the plugins are showing"
        );
    }

    #[test]
    fn a_query_that_matches_nothing_says_so() {
        let mut harness = harness();
        harness.click_section_field();
        harness.type_text("zzzz");

        assert!(frame_text(&harness.frame()).contains("No plugin matches that."));
    }

    #[test]
    fn a_plugin_is_found_by_its_id_and_by_where_it_came_from() {
        // The two things somebody types that are not the name: the id they
        // read in a keymap, and the word for a tier.
        let mut harness = harness();
        harness.click_section_field();
        harness.type_text("crook/palette");
        assert!(frame_text(&harness.frame()).contains("Command palette"));
    }

    #[test]
    fn what_a_section_was_typed_into_does_not_outlive_leaving_it() {
        // The same rule the settings rail's own box follows: a filter that
        // came back with the section would be a list that had silently lost
        // most of itself.
        let mut harness = harness();
        harness.click_section_field();
        harness.type_text("palette");
        assert!(!frame_text(&harness.frame()).contains("Window commands"));

        harness.show_tabs();
        harness.show_plugins();

        assert!(
            frame_text(&harness.frame()).contains("Window commands"),
            "the query outlived the section"
        );
    }

    #[test]
    fn the_switch_on_the_card_takes_the_plugin_out_of_the_window() {
        // The switch is not a preference: throwing it takes the plugin's
        // registrations back out, so what it contributed stops being in the
        // window at all rather than being drawn disabled.
        let mut harness = Harness::new(1);
        harness.show_plugins();
        harness.click_plugin("Shell settings");

        let switches = settings_switch_boxes(&harness.frame());
        assert_eq!(switches.len(), 1, "one switch, on the card");
        harness.click(center(switches[0]), MouseButton::Left);

        let text = frame_text(&harness.frame());
        assert!(
            text.contains("switched off"),
            "the card does not say it is off: {text}"
        );
        // And the Shell page went with it, because that page was the plugin's
        // too. Asked of the host: the sidebar is showing the plugin list
        // rather than the settings rail.
        assert!(
            harness
                .workspace
                .read(&harness.app, |workspace, _| workspace
                    .host()
                    .settings_page_id("crook/shell/page"))
                .is_none(),
            "the page outlived the plugin that added it"
        );
    }

    #[test]
    fn the_switch_that_would_take_the_switch_away_is_inert() {
        // A one-way door whose way back is editing a JSON file. The card says
        // so rather than offering it.
        let mut harness = harness();

        harness.click_plugin("Plugins");
        let text = frame_text(&harness.frame());

        assert!(
            text.contains("It is what draws the page"),
            "the card does not say why its switch is inert: {text}"
        );
    }
}

/// The one shape both sidebar sections are drawn in.
///
/// The Settings section and the Plugins section each wrote their own two
/// halves once, and drifted: see `workspace::section`, which is now the only
/// place either shape is written down. These pin the parts of it that a
/// screenshot would otherwise be the only record of.
mod section_layout {
    use super::*;

    #[test]
    fn a_page_with_less_room_than_the_measure_shrinks_to_it() {
        // The bug: the measure was applied by a flex row of two spacers, and a
        // flex measures an inflexible child *free along the main axis* — so
        // the column came back 560 wide however little room it had been given.
        // Docking the Themes panel leaves the body 528, and every control down
        // the right of a settings row was laid out past the edge of the window
        // and clipped away by the scroll.
        let mut harness = Harness::new(1);
        harness.open_settings_page();
        assert!(
            !settings_switch_boxes(&harness.frame()).is_empty(),
            "the Appearance page should draw switches to begin with"
        );

        harness.open_theme_panel();
        let scene = harness.frame();
        let pane = settings_pane_box(&scene);
        let switch = *settings_switch_boxes(&scene)
            .first()
            .expect("the page's switches went off the window with the panel up");

        // Clear of the page's own inset, not merely inside the window: a rect
        // that ran past the edge would have been *clipped* to it by the
        // scroll, and so would have read as fitting.
        assert!(
            switch.max_x() <= pane.max_x() - 20.,
            "a switch reaches {} in a body that ends at {}, which leaves less \
             than the page's own padding",
            switch.max_x(),
            pane.max_x()
        );
    }

    #[test]
    fn both_sections_draw_their_lists_the_same_way() {
        // Rows inset by ten pixels and rows inset by eight, one list that
        // scrolled and one that could not. They are one row now, and this is
        // what says so without a picture — for the Store's list too, which
        // used to draw a row of its own with a face's worth of room missing.
        let scratch = Scratch::new();
        let cache = crate::plugins::store::cache::Cache::at(scratch.path().join("store"));
        cache
            .write(
                br#"{"schema": 1, "plugins": [{"id": "eugen/probe", "name": "Probe", "description": "d",
                     "versions": [{"version": "1.0.0", "abi": 8, "url": "https://x.invalid/p.wasm", "sha256": "aa"}]}]}"#,
                None,
            )
            .expect("the test index parses");
        let mut harness = Harness::with_store(
            1,
            Opening {
                settings: Settings::ephemeral(),
                channel: Channel::Dev,
                plugins: crate::plugins::defaults(),
                withdrawn: Default::default(),
                heard: Default::default(),
                plugins_directory: None,
            },
            cache,
        );
        harness.open_settings_page();
        let rail = settings_rail_boxes(&harness.frame());
        harness.show_plugins();
        let list = settings_rail_boxes(&harness.frame());
        harness.show_section("crook/store/section");
        let store = settings_rail_boxes(&harness.frame());

        let rail = *rail.first().expect("the settings rail has rows");
        let list = *list.first().expect("the plugins list has rows");
        let store = *store.first().expect("the store's list has rows");

        for (what, row) in [("a list row", list), ("a store row", store)] {
            assert!(
                (rail.height() - row.height()).abs() < 0.5,
                "a rail row is {} tall and {what} {}",
                rail.height(),
                row.height()
            );
            assert!(
                (rail.min_x() - row.min_x()).abs() < 0.5,
                "a rail row starts at {} and {what} at {}",
                rail.min_x(),
                row.min_x()
            );
            assert!(
                (rail.min_y() - row.min_y()).abs() < 0.5,
                "the first rail row is at {} and the first {what} at {}",
                rail.min_y(),
                row.min_y()
            );
        }
    }

    #[test]
    fn a_pages_title_stays_put_while_the_page_scrolls() {
        // The settings page pinned its heading and the plugins card scrolled
        // its own away. The frame pins both: a body a hundred pixels tall
        // should not have to be scrolled to find out what it is a page of.
        //
        // Both sections, because the point is that there is one answer — and
        // each is checked against a line further down the same page, so a page
        // that simply refused to scroll could not pass.
        for section in ["Settings", "Plugins"] {
            let mut harness = Harness::new(1);
            let (title, below) = match section {
                "Settings" => {
                    harness.open_settings_page();
                    ("Appearance", "Follow the desktop")
                }
                _ => {
                    harness.show_plugins();
                    // The command's *name*, which is the row's description
                    // and has a line to itself. Not its title: that is the
                    // label, and a label shares a baseline with the Run
                    // button beside it, so the line holds both.
                    ("Window commands", "crook/window/close-pane")
                }
            };

            let scene = harness.frame();
            let title_before = title_line(&scene, title);
            let below_before = title_line(&scene, below);

            harness.scroll_settings_page(-20.);
            let scene = harness.frame();
            let title_after = title_line(&scene, title);
            let below_after = title_line(&scene, below);

            assert!(
                below_before.y() - below_after.y() > 1.,
                "{section}: the page did not scroll, so this proves nothing"
            );
            assert!(
                (title_before.y() - title_after.y()).abs() < 0.5,
                "{section}: the title moved from {} to {} when the page scrolled",
                title_before.y(),
                title_after.y()
            );
        }
    }

    #[test]
    fn a_plugin_that_is_running_says_so_with_the_pointer_elsewhere() {
        // The regression this test exists for: the two lists were given one
        // colour rule, and it was the rail's — lit while selected or hovered,
        // muted otherwise. That is right for a rail of pages and wrong for a
        // list of plugins, which is read for what is *running* by somebody
        // whose pointer is nowhere near it. Every unselected row went muted,
        // and the only thing left saying a plugin was on was a six-pixel dot.
        let mut harness = Harness::new(1);
        harness.show_plugins();
        let scene = harness.frame();
        let panel = panel_box(&scene);

        // "Header" is the second row and is loaded, so it is neither the
        // selected row nor under a pointer this test never moves.
        let color = label_color(&scene, &panel, "Header");
        assert_eq!(
            color,
            theme().text_primary,
            "a running plugin's name is drawn in {color:?} with nothing \
             pointing at it, which is how a switched-off one is drawn"
        );
    }

    /// The colour the glyphs of one line in the sidebar are tinted.
    fn label_color(scene: &Scene, panel: &RectF, label: &str) -> Color {
        let line = text_lines(scene, |position| position.x() < panel.max_x())
            .into_iter()
            .find(|(_, line)| line.trim() == label)
            .unwrap_or_else(|| panic!("no row in the sidebar reading {label:?}"))
            .0;

        scene
            .layers()
            .flat_map(|layer| layer.glyphs.iter())
            .find(|glyph| {
                (glyph.position.y() - line.y()).abs() < 0.5
                    && (glyph.position.x() - line.x()).abs() < 0.5
            })
            .map(|glyph| glyph.color)
            .expect("the line the text came from has glyphs")
    }

    /// Where the page's own heading is drawn, which is the copy beside the
    /// list rather than the one in it.
    fn title_line(scene: &Scene, title: &str) -> Vector2F {
        let pane = settings_pane_box(scene);
        text_lines(scene, |position| position.x() > pane.min_x())
            .into_iter()
            .find(|(_, line)| line.trim() == title)
            .unwrap_or_else(|| panic!("no line on the page reading {title:?}"))
            .0
    }
}

/// The Themes panel's place in the window: a second sidebar, beside the first.
mod theme_panel_placement {
    use super::*;

    #[test]
    fn the_panel_is_drawn_whatever_section_is_showing() {
        // The bug this test exists for: the panel was composed inside the
        // tabs' own branch of the render, so opening the chooser from the
        // Appearance page set the flag and drew nothing at all. It is composed
        // once now, beside the sidebar, for every section.
        for section in [None, Some("Settings"), Some("Plugins")] {
            let mut harness = Harness::new(1);
            match section {
                Some("Settings") => harness.open_settings_page(),
                Some("Plugins") => harness.show_plugins(),
                _ => {}
            }

            harness.open_theme_panel();
            let scene = harness.frame();

            assert!(
                !theme_cards(&scene).is_empty(),
                "the panel drew no theme cards with {section:?} showing"
            );
            let sidebar = panel_box(&scene);
            for card in theme_cards(&scene) {
                assert!(
                    card.min_x() >= sidebar.max_x() - 0.5,
                    "a theme card at {card:?} is over the first sidebar, which \
                     ends at {}",
                    sidebar.max_x()
                );
            }
        }
    }

    #[test]
    fn the_panel_stands_beside_the_sidebar_rather_than_over_it() {
        // The whole of what this arrangement is for: the sidebar keeps what it
        // was showing, and the panel is a column of its own next to it.
        let mut harness = Harness::new(1);
        harness.open_settings_page();
        assert!(frame_text(&harness.frame()).contains("Appearance"));

        harness.open_theme_panel();
        let text = frame_text(&harness.frame());

        assert!(text.contains("Themes"), "the panel is not up: {text}");
        assert!(
            text.contains("Shell"),
            "the settings rail lost its place to the panel: {text}"
        );
        // And the page it was opened from is still there, narrower.
        assert!(text.contains("Follow the desktop"), "{text}");
    }

    #[test]
    fn the_tab_list_keeps_its_rows_and_its_search_box() {
        // The tabs are a section like the others, and the panel does not take
        // their sidebar either.
        let mut harness = Harness::new(2);
        let before = panel_rows(&harness.frame()).len();
        assert!(before > 0);

        harness.open_theme_panel();
        let scene = harness.frame();

        assert_eq!(panel_rows(&scene).len(), before, "the tab list went away");
        assert!(!theme_cards(&scene).is_empty(), "the panel drew nothing");
        assert!(
            panel_search_box(&scene).max_x() <= panel_box(&scene).max_x() + 0.5,
            "the search box left the sidebar"
        );
    }

    #[test]
    fn the_panel_suspends_the_search_box_rather_than_ending_it() {
        // The box stays on screen, because the sidebar is not covered any
        // more — but the panel takes the keyboard whole while it is up, so a
        // letter typed at an open chooser is not quietly filed into a filter
        // behind it. The wish outlives the panel: closing it gives the box
        // back, query and keyboard together.
        let mut harness = Harness::panel(2);
        harness.press_search_chord();
        harness.type_text("kettle");

        harness.open_theme_panel();
        assert!(
            !harness.panel_search_takes_keys(),
            "the box kept the keyboard with the panel up"
        );
        panel_search_box(&harness.frame());

        harness.close_theme_panel();
        assert!(
            harness.panel_search_takes_keys(),
            "the box never got the keyboard back"
        );
        assert_eq!(harness.panel_search_text(), "kettle");
    }

    #[test]
    fn the_work_is_pushed_aside_and_gets_its_width_back() {
        // Warp's docked chooser: it pushes the terminal over rather than
        // covering it, so a theme is judged against real output — and the
        // width it took is the width the work gets back when it closes.
        let mut harness = Harness::new(1);
        let wide = panel_boxes(&harness.frame())[0];

        harness.open_theme_panel();
        let squeezed = panel_boxes(&harness.frame())[0];
        assert!(
            (wide.width() - squeezed.width() - tabs_panel::PANEL_WIDTH).abs() < 1.,
            "the pane went from {} to {} wide, which is not the panel's {}",
            wide.width(),
            squeezed.width(),
            tabs_panel::PANEL_WIDTH
        );
        assert!(
            squeezed.min_x() >= wide.min_x() + tabs_panel::PANEL_WIDTH - 1.,
            "the panel covered the work instead of pushing it aside"
        );

        harness.dispatch_workspace_action(WorkspaceAction::Theme(ThemeAction::ClosePanel));
        let text = frame_text(&harness.frame());
        assert!(!text.contains("Change your current theme."), "{text}");
        assert!((panel_boxes(&harness.frame())[0].width() - wide.width()).abs() < 1.);
    }
}

/// A plugin that takes both marks on a tab row, for the tests below.
///
/// Native, because what is being tested is the *slot* rather than the sandbox:
/// a contribution from a `.wasm` file arrives at the same registry through
/// `plugins::wasm`, and putting a wasm module in this file would test the
/// interpreter and the panel at once.
struct TestMarks;

/// What the mark contribution draws, so the assertions can find it.
const MARK_ICON: Lucide = Lucide::Check;

/// And the badge, which it puts only on the rows that are worktrees.
const BADGE_ICON: Lucide = Lucide::Info;

/// The title of the one row this plugin declines to draw a mark on.
const UNMARKED: &str = "left alone";

impl crate::plugin::Plugin for TestMarks {
    fn manifest(&self) -> &'static crate::plugin::Manifest {
        static MANIFEST: std::sync::OnceLock<crate::plugin::Manifest> = std::sync::OnceLock::new();
        MANIFEST.get_or_init(|| crate::plugin::Manifest {
            schema: crate::plugin::Manifest::SCHEMA,
            id: crate::plugin::PluginId::parse("eugen/marks").expect("a literal that parses"),
            name: "Marks",
            description: "Takes both marks on a tab row.",
            version: "0.1.0",
            tier: crate::plugin::Tier::Native,
            capabilities: &[],
        })
    }

    fn build(
        &mut self,
        host: &mut crate::plugin::Host,
        _: &mut ViewContext<Workspace>,
    ) -> Result<(), crate::plugin::BuildError> {
        host.contribute_row(
            crate::plugins::tabs::TAB_ROW_MARK,
            "mark",
            0,
            |_, row, _| {
                // One row declined, which is the case the disc has to survive:
                // "nothing to say about this one" must leave the row as it was
                // rather than empty it.
                (row.title != UNMARKED).then(|| {
                    Icon::new(MARK_ICON, 16.)
                        .with_color(theme().accent)
                        .finish()
                })
            },
        );
        host.contribute_row(
            crate::plugins::tabs::TAB_ROW_BADGE,
            "badge",
            0,
            |_, row, _| {
                row.git.is_some_and(|facts| facts.worktree).then(|| {
                    Icon::new(BADGE_ICON, 8.)
                        .with_color(theme().accent)
                        .finish()
                })
            },
        );
        Ok(())
    }
}

impl Harness {
    /// A window whose tab rows a plugin has taken the marks on.
    fn with_marks(tabs: usize) -> Self {
        let mut plugins = crate::plugins::defaults();
        plugins.push(Box::new(TestMarks));
        Self::with_plugins(tabs, Settings::ephemeral(), plugins)
    }
}

/// The discs the panel is drawing: round, and the size `crook/tabs` draws its
/// status disc at.
fn status_discs(scene: &Scene) -> Vec<RectF> {
    let panel = panel_box(scene);
    let diameter = crate::plugins::tabs::MARK_SIZE * 0.76;
    rects_rounded_by(scene, Radius::Percentage(50.))
        .into_iter()
        .filter(|bounds| panel.contains_point(center(*bounds)))
        .filter(|bounds| (bounds.width() - diameter).abs() < 0.5)
        .collect()
}

/// The rings round a badge: round, and smaller than half the mark's box.
fn badge_rings(scene: &Scene) -> Vec<RectF> {
    let panel = panel_box(scene);
    let ring = crate::plugins::tabs::MARK_SIZE * 0.46;
    rects_rounded_by(scene, Radius::Percentage(50.))
        .into_iter()
        .filter(|bounds| panel.contains_point(center(*bounds)))
        .filter(|bounds| (bounds.width() - ring).abs() < 0.5)
        .collect()
}

#[test]
fn a_plugin_can_draw_the_mark_at_the_head_of_every_row() {
    // The slot, end to end: what the panel paints where the status disc was is
    // what a plugin said to paint there, on every row and not just the active
    // one.
    let mut harness = Harness::with_marks(3);

    let scene = harness.frame();

    assert_eq!(icons_in(&scene, panel_box(&scene), MARK_ICON).len(), 3);
    assert!(
        status_discs(&scene).is_empty(),
        "the disc was drawn under the mark that replaced it"
    );
}

#[test]
fn a_row_the_plugin_declines_keeps_the_disc_it_had() {
    // The reason the disc is the host's answer to an empty slot rather than a
    // contribution of its own: a plugin marking *some* rows must leave the
    // others looking exactly as they did, and `Slots::one` never asks the next
    // contributor when the first declines.
    let mut harness = Harness::with_marks(2);
    let pane = harness.pane_ids()[0];
    harness.update_session(pane, |session| {
        session.derived_title = Some(UNMARKED.to_owned());
    });

    let scene = harness.frame();

    let discs = status_discs(&scene);
    assert_eq!(icons_in(&scene, panel_box(&scene), MARK_ICON).len(), 1);
    assert_eq!(discs.len(), 1, "the declined row lost its disc");
    assert!(
        tab_boxes(&scene)[0].contains_point(center(discs[0])),
        "the disc came back on the wrong row"
    );
}

/// What every row a plugin was asked about said it was, in panel order.
type Ordinals = Rc<RefCell<Vec<usize>>>;

/// A plugin that draws nothing and writes down which row it was asked about.
struct TestOrdinals(Ordinals);

impl crate::plugin::Plugin for TestOrdinals {
    fn manifest(&self) -> &'static crate::plugin::Manifest {
        static MANIFEST: std::sync::OnceLock<crate::plugin::Manifest> = std::sync::OnceLock::new();
        MANIFEST.get_or_init(|| crate::plugin::Manifest {
            schema: crate::plugin::Manifest::SCHEMA,
            id: crate::plugin::PluginId::parse("eugen/ordinals").expect("a literal that parses"),
            name: "Ordinals",
            description: "Writes down which row it is drawing.",
            version: "0.1.0",
            tier: crate::plugin::Tier::Native,
            capabilities: &[],
        })
    }

    fn build(
        &mut self,
        host: &mut crate::plugin::Host,
        _: &mut ViewContext<Workspace>,
    ) -> Result<(), crate::plugin::BuildError> {
        let seen = self.0.clone();
        host.contribute_row(
            crate::plugins::tabs::TAB_ROW_MARK,
            "mark",
            0,
            move |_, row, _| {
                seen.borrow_mut().push(row.nth);
                // Declined, so the panel looks exactly as it did: this plugin
                // is here to be asked, not to draw.
                None
            },
        );
        Ok(())
    }
}

#[test]
fn every_tab_in_one_directory_is_still_a_row_of_its_own() {
    // The bug a mark per tab had: `AgentSession::new` seeds every session with
    // the process's own directory, so three new tabs are three rows in one
    // place — and a plugin told only *where* a row is working could not tell
    // them apart, which is one emoji drawn three times down the panel.
    let seen = Ordinals::default();
    let mut plugins = crate::plugins::defaults();
    plugins.push(Box::new(TestOrdinals(seen.clone())));
    // Drawn by the harness's own first frame; a second would be answered out
    // of the element cache, since nothing has been invalidated since.
    let _harness = Harness::with_plugins(3, Settings::ephemeral(), plugins);

    let seen = seen.borrow();

    assert_eq!(*seen, vec![0, 1, 2], "three tabs, one row");
}

#[test]
fn a_badge_is_drawn_on_the_corner_of_the_mark_it_belongs_to() {
    // Warp hangs its status ring off the bottom-right of the same 24px box,
    // and this is where the two plugins the panel was opened up for meet: one
    // draws the mark, the other says one more thing about the same tab without
    // taking the first one's place.
    let mut harness = Harness::with_marks(2);
    let panes = harness.pane_ids();
    harness.seed(panes[1], None);
    harness.record_git_facts(panes[1], "side", None, true);

    let scene = harness.frame();

    let rings = badge_rings(&scene);
    assert_eq!(rings.len(), 1, "the badge went on more rows than one");
    assert_eq!(icons_in(&scene, panel_box(&scene), BADGE_ICON).len(), 1);

    let row = tab_boxes(&scene)[1];
    let ring = rings[0];
    assert!(
        row.contains_point(center(ring)),
        "{ring:?} is not on {row:?}"
    );
    // Down and to the right of the mark it sits on, which is the whole of what
    // makes it a badge rather than a second mark beside the first.
    let mark = icons_in(&scene, row, MARK_ICON)[0];
    assert!(
        center(ring).x() > center(mark).x() && center(ring).y() > center(mark).y(),
        "the badge at {ring:?} is not on the corner of the mark at {mark:?}"
    );
}

/// Reaching the panes, the tabs and the block menu with nothing but the
/// keyboard.
///
/// The window's commands are one table read three ways — the palette runs one,
/// a chord names one, and the Keyboard Shortcuts page binds one — so these go
/// in through `run_command`, which is the palette's path, and through
/// `press_key`, which is the chord's. A command that declines is the case
/// worth most of the cases here: `Workspace::command` answering `None` is what
/// sends the keystroke on to the shell, and a chord that swallowed a key it
/// could do nothing with would be a terminal that eats input.
mod from_the_keyboard {
    use super::*;

    /// The active tab's panes, in render order.
    fn panes(harness: &Harness) -> Vec<PaneId> {
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .tabs()
                .active()
                .expect("a tab is active")
                .panes()
                .iter()
                .map(Pane::id)
                .collect()
        })
    }

    fn focused(harness: &Harness) -> PaneId {
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .tabs()
                .focused_pane_id()
                .expect("a pane is focused")
        })
    }

    fn active_tab(harness: &Harness) -> TabId {
        harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.tabs().active_id())
    }

    fn tabs_in_order(harness: &Harness) -> Vec<TabId> {
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tabs().iter().map(Tab::id).collect()
        })
    }

    #[test]
    fn the_focus_commands_step_along_the_split() {
        let mut harness = Harness::panel(1);
        harness.run_command("crook/window/split-right");
        harness.run_command("crook/window/split-right");
        let panes = panes(&harness);
        assert_eq!(panes.len(), 3);
        assert_eq!(focused(&harness), panes[2], "a split focuses what it made");

        harness.run_command("crook/window/focus-pane-left");
        assert_eq!(focused(&harness), panes[1]);
        harness.run_command("crook/window/focus-pane-left");
        assert_eq!(focused(&harness), panes[0]);
        harness.run_command("crook/window/focus-pane-right");
        assert_eq!(focused(&harness), panes[1]);
    }

    #[test]
    fn a_focus_chord_across_the_axis_reaches_the_shell_instead() {
        // The rule the whole pane family rests on. A row of panes has nothing
        // above it, so the chord must decline rather than be swallowed —
        // otherwise `ctrl+shift+up` would stop meaning anything in vim the
        // moment somebody split a tab sideways.
        let mut harness = Harness::panel(1);
        harness.run_command("crook/window/split-right");

        let modifiers =
            if crate::input_keys::Platform::current() == crate::input_keys::Platform::Mac {
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Default::default()
                }
            } else {
                Modifiers {
                    alt: true,
                    ..Default::default()
                }
            };

        assert!(
            !harness.press_key("up", modifiers),
            "a direction the split has no pane in was consumed"
        );
        assert!(
            harness.press_key("left", modifiers),
            "the direction it does have was not"
        );
    }

    #[test]
    fn nothing_in_the_pane_family_fires_on_a_tab_that_was_never_split() {
        let mut harness = Harness::panel(1);
        let alone = focused(&harness);

        for command in [
            "crook/window/focus-pane-left",
            "crook/window/focus-pane-right",
            "crook/window/focus-next-pane",
            "crook/window/focus-previous-pane",
            "crook/window/grow-pane",
            "crook/window/shrink-pane",
            "crook/window/even-panes",
        ] {
            harness.run_command(command);
            assert_eq!(focused(&harness), alone, "{command}");
        }
    }

    #[test]
    fn cycling_comes_back_round_where_the_directions_stop() {
        let mut harness = Harness::panel(1);
        harness.run_command("crook/window/split-right");
        let panes = panes(&harness);

        harness.run_command("crook/window/focus-next-pane");
        assert_eq!(focused(&harness), panes[0], "past the end is the start");
        harness.run_command("crook/window/focus-previous-pane");
        assert_eq!(focused(&harness), panes[1]);
    }

    #[test]
    fn splitting_leftwards_puts_the_new_pane_in_front_of_the_old_one() {
        // The half of the pair that had a model and no name until now.
        let mut harness = Harness::panel(1);
        let first = focused(&harness);

        harness.run_command("crook/window/split-left");

        let panes = panes(&harness);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[1], first, "the pane that was there moved along");
        assert_eq!(focused(&harness), panes[0], "the new one has the keyboard");
    }

    #[test]
    fn the_digits_pick_a_tab_by_its_place_in_the_strip() {
        let mut harness = Harness::panel(4);
        let tabs = tabs_in_order(&harness);

        harness.run_command("crook/window/select-tab-1");
        assert_eq!(active_tab(&harness), tabs[0]);
        harness.run_command("crook/window/select-tab-3");
        assert_eq!(active_tab(&harness), tabs[2]);
        harness.run_command("crook/window/select-last-tab");
        assert_eq!(active_tab(&harness), tabs[3]);
    }

    #[test]
    fn a_digit_past_the_end_of_the_strip_reaches_the_shell() {
        // Four tabs and a chord for the eighth. Every browser leaves that key
        // to whatever is under it rather than selecting the last tab, because
        // a person pressing it has counted and is wrong about something.
        let mut harness = Harness::panel(4);
        let before = active_tab(&harness);

        harness.run_command("crook/window/select-tab-8");

        assert_eq!(active_tab(&harness), before);
    }

    #[test]
    fn the_block_commands_decline_at_the_prompt() {
        // Nothing is selected until somebody steps off the prompt, and each of
        // these is about *the* block. Consumed, they would take the copy chord
        // away from a shell for a command nobody could aim.
        let harness = Harness::panel(1);

        for command in [
            "crook/window/copy-block",
            "crook/window/copy-block-command",
            "crook/window/copy-block-output",
            "crook/window/rerun-block",
            "crook/window/open-block-menu",
            "crook/window/scroll-to-block-top",
        ] {
            let action = ActionName::parse(command).expect("a literal that parses");
            let resolved = harness.workspace.read(&harness.app, |workspace, _| {
                crate::plugins::window::binding_for(&action)
                    .and_then(|binding| workspace.command(binding))
            });
            assert!(resolved.is_none(), "{command} did not decline");
        }
    }

    #[test]
    fn every_command_in_the_table_is_registered() {
        // The promise every unbound command rests on. `register_command` is
        // what puts one in the palette and on the Keyboard Shortcuts page, so
        // a name in `window::COMMANDS` that the host does not carry would be a
        // command no keyboard can reach at all — not by a chord, since most of
        // them ship without one, and not by name either.
        //
        // Asked of the host rather than of the table, because the table is
        // where the name came from: `binding_for` is a search of that same
        // array and cannot fail for anything in it.
        let harness = Harness::panel(1);
        let registered: Vec<String> = harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .host()
                .commands()
                .iter()
                .map(|(_, action, _)| action.to_string())
                .collect()
        });

        for (name, title, _) in crate::plugins::window::COMMANDS {
            let action = format!("crook/window/{name}");
            assert!(
                registered.contains(&action),
                "{action} ({title}) is in the table and nothing registered it"
            );
        }
    }

    #[test]
    fn the_pane_family_declines_rather_than_swallowing_on_an_unsplit_tab() {
        // `nothing_in_the_pane_family_fires...` watches the focus, which three
        // of these never move even when they act. This watches the thing that
        // matters: `Workspace::command` answering `None`, which is what sends
        // the keystroke on to the shell.
        let harness = Harness::panel(1);

        for name in [
            "focus-pane-left",
            "focus-pane-right",
            "focus-pane-up",
            "focus-pane-down",
            "focus-next-pane",
            "focus-previous-pane",
            "grow-pane",
            "shrink-pane",
            "even-panes",
        ] {
            let action = ActionName::parse(&format!("crook/window/{name}")).expect("a literal");
            let resolved = harness.workspace.read(&harness.app, |workspace, _| {
                crate::plugins::window::binding_for(&action)
                    .and_then(|binding| workspace.command(binding))
            });
            assert!(resolved.is_none(), "{name} did not decline on one pane");
        }
    }

    #[test]
    fn the_menu_answers_the_arrow_keys_themselves() {
        // Through `press_key`, which is the chord's own path: the arms in
        // `tab_menu_action_for` are what a person actually presses, and the
        // test above them drives the actions directly.
        let mut harness = Harness::panel(1);
        harness.run_command("crook/tabs/open-menu");
        harness.frame();

        let selected = |harness: &Harness| {
            harness.workspace.read(&harness.app, |workspace, _| {
                workspace.tab_context_menu().selected_action()
            })
        };
        assert!(selected(&harness).is_none());

        assert!(harness.press_key("down", Modifiers::default()));
        assert!(selected(&harness).is_some(), "Down selected nothing");

        assert!(harness.press_key("escape", Modifiers::default()));
        assert!(
            harness.tab_menu_row().is_none(),
            "Escape left the menu standing"
        );
    }

    #[test]
    fn a_rename_field_keeps_the_arrows_the_menu_would_otherwise_take() {
        // The guard in `tab_menu_action_for`, which nothing else covers. While
        // a row has become a field, its arrows belong to the caret in it — a
        // selection that moved under somebody editing a tab's name would be
        // the menu answering a key aimed at the text.
        let mut harness = Harness::panel(1);
        harness.run_command("crook/tabs/open-menu");
        harness.frame();
        harness.run_command("crook/tabs/rename-tab");
        harness.frame();
        assert!(
            harness.a_plugin_field_has_keys(),
            "the rename field did not take the keyboard"
        );

        let before = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tab_context_menu().selected_action()
        });
        harness.press_key("down", Modifiers::default());
        let after = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tab_context_menu().selected_action()
        });

        assert_eq!(before, after, "the menu moved under a caret");
    }

    #[test]
    fn every_settings_page_can_be_opened_by_name() {
        // Registered by `Host::add_settings_page` rather than by each plugin,
        // so this is also the check that a page a stranger's plugin adds gets
        // one for free.
        let mut harness = Harness::panel(1);
        let commands: Vec<String> = harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .host()
                .commands()
                .iter()
                .map(|(_, action, _)| action.to_string())
                .collect()
        });

        for page in ["appearance", "shell", "shortcuts", "about"] {
            let expected = format!("crook/{page}/open-page");
            assert!(
                commands.contains(&expected),
                "{expected} is not a command; the page list is {commands:?}"
            );
        }

        harness.run_command("crook/shortcuts/open-page");
        assert!(
            harness
                .workspace
                .read(&harness.app, |workspace, _| workspace
                    .is_settings_page_open()),
            "opening a page by name did not show the settings"
        );
    }

    #[test]
    fn the_tab_menu_can_be_opened_walked_and_run_without_a_pointer() {
        let mut harness = Harness::panel(2);
        assert!(harness.tab_menu_row().is_none());

        harness.run_command("crook/tabs/open-menu");
        assert!(harness.tab_menu_row().is_some(), "the menu did not open");
        harness.frame();

        // Down from nothing is the first row, and it is the pin entry —
        // band 0, which is the top of the menu.
        harness.dispatch_workspace_action(TabMenuAction::MoveSelection(1).into());
        let first = harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tab_context_menu().selected_action()
        });
        assert!(first.is_some(), "nothing was selected by the first arrow");

        let pinned_before = harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .tabs()
                .active()
                .expect("a tab is active")
                .is_pinned()
        });
        harness.dispatch_workspace_action(TabMenuAction::RunSelected.into());
        let pinned_after = harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .tabs()
                .active()
                .expect("a tab is active")
                .is_pinned()
        });
        assert_ne!(pinned_before, pinned_after, "Enter ran no entry");
    }

    #[test]
    fn a_menu_row_prints_the_chord_that_reaches_it() {
        // The whole of why the chord is not passed in: the key a row is built
        // with is the name of the action it runs, so a binding written this
        // morning is on the row this afternoon with nothing told about it.
        let mut harness = Harness::panel(1);
        harness.run_command("crook/tabs/open-menu");
        assert!(
            !frame_text(&harness.frame()).contains("ctrl+alt+9"),
            "the chord was printed before anything was bound to it"
        );

        harness.bind(r#"[{ "key": "ctrl+alt+9", "command": "crook/tabs/close-tab" }]"#);
        let text = frame_text(&harness.frame());
        assert!(
            text.contains("ctrl+alt+9"),
            "the menu does not print the chord it was given: {text}"
        );
    }

    #[test]
    fn a_palette_row_prints_the_chord_that_reaches_it() {
        let mut harness = Harness::panel(1);
        harness.run_command("crook/palette/open");

        let text = frame_text(&harness.frame());
        // Whichever platform this runs on, the chord for a new tab is shipped
        // and the palette lists it.
        let expected = match crate::input_keys::Platform::current() {
            crate::input_keys::Platform::Mac => "cmd+t",
            crate::input_keys::Platform::Other => "ctrl+shift+t",
        };
        assert!(
            text.contains("New agent tab"),
            "the palette did not open: {text}"
        );
        assert!(
            text.contains(expected),
            "the palette does not print {expected}: {text}"
        );
    }

    #[test]
    fn the_menus_selection_does_not_outlive_the_menu() {
        // An action Enter could still find with nothing on screen would be a
        // key that acts on a row nobody can see.
        let mut harness = Harness::panel(1);
        harness.run_command("crook/tabs/open-menu");
        harness.frame();
        harness.dispatch_workspace_action(TabMenuAction::MoveSelection(1).into());
        assert!(harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tab_context_menu().selected_action().is_some()
        }));

        harness.dispatch_workspace_action(TabMenuAction::Close.into());

        assert!(harness.workspace.read(&harness.app, |workspace, _| {
            workspace.tab_context_menu().selected_action().is_none()
        }));
    }
}

mod what_a_tab_is_called {
    use super::*;
    use crate::terminal_model::TerminalUpdate;

    fn report(harness: &mut Harness, update: TerminalUpdate) {
        harness.workspace_update(|workspace, ctx| {
            workspace.apply_terminal_update(&update, ctx);
        });
    }

    fn name_of(harness: &Harness, pane: PaneId) -> Option<String> {
        harness.workspace.read(&harness.app, |workspace, _| {
            workspace
                .tabs()
                .pane(pane)
                .and_then(|pane| pane.session().name().map(str::to_owned))
        })
    }

    /// The whole point, end to end through the update the model sends: a tab
    /// with no name of its own is called after what it is running, and stops
    /// being called that the moment it stops running it.
    #[test]
    fn a_tab_is_called_after_what_it_is_running() {
        let mut harness = Harness::panel(1);
        let pane = harness.active_pane_ids()[0];
        assert_eq!(name_of(&harness, pane), None, "a fresh shell is unnamed");

        report(
            &mut harness,
            TerminalUpdate::Running(pane, Some("cargo test".to_owned())),
        );
        assert_eq!(name_of(&harness, pane), Some("cargo test".to_owned()));

        report(&mut harness, TerminalUpdate::Running(pane, None));
        assert_eq!(
            name_of(&harness, pane),
            None,
            "back to a prompt, and back to being about its directory"
        );
    }

    /// What the shell calls the window outranks what it happens to be running:
    /// an agent reporting its task is the better name, and the reason the
    /// reporting exists at all.
    #[test]
    fn a_reported_title_outranks_the_command() {
        let mut harness = Harness::panel(1);
        let pane = harness.active_pane_ids()[0];

        report(
            &mut harness,
            TerminalUpdate::Running(pane, Some("claude".to_owned())),
        );
        report(
            &mut harness,
            TerminalUpdate::Title(pane, Some("AG-2517 UI simplify".to_owned())),
        );
        assert_eq!(
            name_of(&harness, pane),
            Some("AG-2517 UI simplify".to_owned())
        );
    }
}
