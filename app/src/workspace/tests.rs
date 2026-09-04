//! The header, driven through a real presenter.
//!
//! These run the actual view tree — `Workspace::render`, layout, paint, hit
//! testing — against a stub shaper, so they cover the wiring a unit test of
//! the strip cannot: that a click lands on the tab under it, that the close
//! button closes its own tab, and that revealing that button does not move
//! the bar out from under the cursor.

use std::cell::Cell;
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
use crookui_core::platform::TextLayoutSystem;
use crookui_core::prelude::*;
use crookui_core::scene::{CornerRadius, Radius, Rect, Scene};
use crookui_core::text_layout::{Glyph, Line, Run};
use crookui_core::{AddSingletonModel as _, App, Presenter, WindowId};

use crate::Channel;
use crate::git::{DiffStats, GitFacts, Head};
use crate::platform_insets::{ControlLayout, WindowChrome};
use crate::settings::{
    Density, GeneralOptions, Granularity, PrimaryInfo, Settings, Subtitle, TabOptions,
};
use crate::tab::{AgentSession, AgentStatus, Direction, Pane, PaneId, Tab, TabAction, TabId};
use crate::terminal_font::{CELL_FONT_SIZE, CellFont};
use crate::theme::theme;
use crate::usage_model::UsageModel;
use crate::window_controls::{Recorder, Request, WindowState};

use super::{
    Fonts, Opening, OptionsAction, QuitRequest, SettingsAction, ThemeAction, Workspace,
    WorkspaceAction, WorktreeAction, controls, tab_options_menu, tabs_panel,
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
        app.update(|ctx| ctx.add_singleton_model(UsageModel::new));

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
        let (window_id, workspace) = app.add_window(|ctx| {
            Workspace::new(
                fonts,
                cell_font,
                Opening {
                    settings,
                    channel: Channel::Dev,
                    plugins,
                },
                quit,
                window.clone(),
                ctx,
            )
        });

        let mut harness = Self {
            queue,
            app,
            presenter: Presenter::new(window_id, Arc::new(StubShaper)),
            window_id,
            workspace,
            quit_requests,
            window,
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

    /// Draws another platform's window controls, the way `--controls` does.
    ///
    /// The only way to look at two thirds of this: the caption buttons are
    /// Crook's own drawing on Windows and Linux, so a machine running neither
    /// can still lay them out and measure them.
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

    /// Whether the menu is asking about removing a checkout.
    fn worktree_menu_is_confirming(&self) -> bool {
        self.workspace.read(&self.app, |workspace, _| {
            workspace.worktree_menu_is_confirming()
        })
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

        let row = text_lines(&scene, |at| {
            at.x() >= column.min_x() && at.x() <= column.max_x()
        })
        .into_iter()
        .find(|(_, line)| line.trim() == name)
        .unwrap_or_else(|| panic!("no row in the plugin list says {name:?}"));

        self.click(row.0 + vec2f(4., 4.), MouseButton::Left);
        self.frame();
    }

    /// Whether somebody clicked the chip and is still waiting.
    fn usage_is_busy_for_user(&self) -> bool {
        self.workspace.read(&self.app, |workspace, ctx| {
            workspace.usage().as_ref(ctx).is_busy_for_user()
        })
    }

    /// Reads a keymap out of `text` and puts it in force.
    fn bind(&mut self, text: &str) {
        let directory = Scratch::new();
        let path = directory.path().join("keymap.json");
        fs::write(&path, text).expect("a scratch keymap");
        let keymap = crate::keymap::Keymap::load(&path);
        assert!(!keymap.is_empty(), "nothing in {text:?} was a binding");

        self.workspace_update(|workspace, _| workspace.set_keymap(keymap));
    }

    /// Whether the usage poll chain is meant to be running.
    fn usage_is_wanted(&self) -> bool {
        self.workspace.read(&self.app, |workspace, ctx| {
            workspace.usage().as_ref(ctx).is_wanted()
        })
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
/// Bounded by size as well as by radius: the options gear, its menu's info
/// note and the hover detail card are all 4px-rounded too, and only a close
/// button is a 16px square. A clipped one is narrower, never wider.
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
/// is the note and nothing else: the popup itself is 6px-rounded, the gear's
/// tooltip is suppressed while its menu is open, and opening the menu takes
/// down any hover card. Deliberately not filtered by width — the width is what
/// the test is about.
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

/// Every icon the frame draws, in paint order.
fn icons_of(scene: &Scene) -> Vec<Lucide> {
    scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .map(|drawn| drawn.icon_key.icon)
        .collect()
}

/// Every icon of one kind painted inside `bounds`.
fn icons_in(scene: &Scene, bounds: RectF, icon: Lucide) -> Vec<RectF> {
    scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .filter(|drawn| drawn.icon_key.icon == icon)
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

/// The gear that opens the popup, by its 16x16 icon slot's box.
fn gear_box(scene: &Scene) -> RectF {
    let boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(4.)
                && (rect.bounds.width() - 20.).abs() < 0.5
                && (rect.bounds.height() - 20.).abs() < 0.5
        })
        .map(|(_, bounds)| bounds)
        .collect();

    assert_eq!(boxes.len(), 1, "exactly one options gear per frame");
    boxes[0]
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
    let mut harness = Harness::new(1);
    let insets = crate::platform_insets::window_control_insets(crate::WINDOW_CHROME, false);
    let boxes = tab_boxes(&harness.frame());

    assert!(
        boxes[0].min_x() >= insets.left,
        "the first tab starts at {} but {} is reserved for window controls",
        boxes[0].min_x(),
        insets.left
    );
    assert!(
        boxes[0].max_x() <= WINDOW.x() - insets.right,
        "the last header item runs into the window controls on the right"
    );
}

#[test]
fn the_header_reserves_the_corner_this_platform_puts_its_controls_in_and_no_other() {
    // Crook's header *is* the title bar, so something is over it — and only at
    // one end. Reserving at both would leave a hole at whichever end this
    // platform's controls are not, which is the failure nobody sees because it
    // is always the end they are not looking at.
    let mut harness = Harness::new(1);
    let insets = harness.window_insets();
    let scene = harness.frame();
    let first = tab_boxes(&scene)[0];
    let chip = pill_box(&scene);

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
    assert!(
        WINDOW.x() - chip.max_x() >= insets.header_right,
        "the chip runs into the {} reserved on the right",
        insets.header_right
    );
    assert!(
        WINDOW.x() - chip.max_x() < insets.header_right + 24.,
        "the chip stops {} short of the right edge",
        WINDOW.x() - chip.max_x()
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

    harness.set_window_state(WindowState {
        fullscreen: true,
        ..WindowState::default()
    });

    assert_eq!(
        harness.window_insets().panel_left,
        0.,
        "fullscreen did not give back what the traffic lights had"
    );
}

/// The caption buttons Crook draws for a window with no frame, by their boxes.
///
/// Found by size rather than by fill: a button that is not being pointed at
/// draws no fill at all on Windows, and the point of the geometry is that the
/// three of them are the width that was reserved whether or not anyone is
/// pointing at one.
fn caption_boxes(scene: &Scene, size: Vector2F) -> Vec<RectF> {
    let mut boxes: Vec<RectF> = visible_rects(scene)
        .map(|(_, bounds)| bounds)
        .filter(|bounds| {
            (bounds.width() - size.x()).abs() < 0.5 && (bounds.height() - size.y()).abs() < 0.5
        })
        .collect();
    boxes.sort_by(|left, right| left.min_x().total_cmp(&right.min_x()));
    boxes
}

#[test]
fn the_caption_buttons_fill_exactly_the_end_of_the_header_that_was_reserved() {
    // Neither platform this draws for can be run here, so what is checked is
    // the arithmetic that is wrong on both when it is wrong: three buttons
    // that reach the corner of the window and start where the reservation
    // starts. A cluster narrower than its reservation is a hole in the header;
    // a wider one sits on the usage chip.
    for (layout, button) in [
        (ControlLayout::Windows, vec2f(45., 30.)),
        (ControlLayout::Freedesktop, vec2f(30., 30.)),
    ] {
        let mut harness = Harness::new(1);
        harness.override_controls(layout);
        let reserved = layout.insets(WindowChrome::Client, false).right;
        let scene = harness.frame();
        let buttons = caption_boxes(&scene, button);

        assert_eq!(
            buttons.len(),
            3,
            "{layout:?} drew {} controls",
            buttons.len()
        );
        assert!(
            buttons[0].min_x() >= WINDOW.x() - reserved,
            "{layout:?} started its controls {} left of the {reserved} it reserved",
            WINDOW.x() - reserved - buttons[0].min_x()
        );
        assert!(
            WINDOW.x() - buttons[2].max_x() <= 8.,
            "{layout:?} left {} between the close button and the window's edge",
            WINDOW.x() - buttons[2].max_x()
        );
        // Nothing the header drew of its own may reach into that end.
        assert!(
            pill_box(&scene).max_x() <= WINDOW.x() - reserved,
            "{layout:?} drew the usage chip under its own close button"
        );
    }
}

#[test]
fn a_caption_button_lights_up_under_the_pointer_and_asks_the_window_when_it_is_clicked() {
    let mut harness = Harness::new(1);
    harness.override_controls(ControlLayout::Windows);
    let scene = harness.frame();
    let buttons = caption_boxes(&scene, vec2f(45., 30.));

    harness.move_to(center(buttons[0]));
    let hovered = harness.frame();
    assert_eq!(
        fills_of(&hovered, theme().overlay_2)
            .into_iter()
            .filter(|bounds| (bounds.width() - 45.).abs() < 0.5)
            .count(),
        1,
        "the button under the pointer did not light up"
    );

    // In order, left to right, the way every desktop that has these draws
    // them: minimise, then maximise, then close.
    harness.click(center(buttons[0]), MouseButton::Left);
    assert_eq!(harness.window_requests(), vec![Request::Minimize]);
    harness.click(center(buttons[1]), MouseButton::Left);
    assert_eq!(
        harness.window_requests(),
        vec![Request::Minimize, Request::ToggleMaximized]
    );

    harness.click(center(buttons[2]), MouseButton::Left);
    assert_eq!(harness.quit_requests.get(), 1, "close did not close");
    assert_eq!(
        harness.window_requests().len(),
        2,
        "closing the window is a quit, not a fourth thing to ask a window"
    );
}

/// Everything painted in one colour, by its box.
fn fills_of(scene: &Scene, color: Color) -> Vec<RectF> {
    visible_rects(scene)
        .filter(|(rect, _)| rect.background == Fill::Solid(color))
        .map(|(_, bounds)| bounds)
        .collect()
}

#[test]
fn pressing_the_header_where_nothing_is_picks_the_window_up() {
    // The gesture that makes this row a title bar. It is a *press*, not a
    // click: the window manager takes over the pointer from the press onward,
    // so waiting for the release would mean waiting for one that never comes.
    let mut harness = Harness::new(1);
    let scene = harness.frame();
    let empty = vec2f(pill_box(&scene).min_x() - 30., center(pill_box(&scene)).y());

    harness.dispatch(Event::MouseDown {
        button: MouseButton::Left,
        position: empty,
        modifiers: Modifiers::default(),
        click_count: 1,
    });

    assert_eq!(harness.window_requests(), vec![Request::Drag]);
}

#[test]
fn pressing_something_in_the_header_does_not_pick_the_window_up() {
    // "Empty" is whatever the row's children did not claim, and this is the
    // half of that which fails silently: a header that dragged the window from
    // its own controls would make every tab unclickable, and the tab would
    // still be highlighted while the window moved.
    let mut harness = Harness::new(2);
    let scene = harness.frame();
    let tab = center(tab_boxes(&scene)[0]);
    let chip = center(pill_box(&scene));

    harness.click(tab, MouseButton::Left);
    harness.click(chip, MouseButton::Left);

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
    let empty = vec2f(
        pill_box(&scene).min_x() - 30.,
        center(tab_boxes(&scene)[0]).y(),
    );

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
    let empty = vec2f(pill_box(&scene).min_x() - 30., center(pill_box(&scene)).y());

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
    // The layout Crook opens in. The panel's control bar is the window's
    // top-left corner, so it is what the traffic lights are painted over and
    // what that end of the window is dragged by — and the header, which is no
    // longer in that corner, owes neither.
    let mut harness = Harness::panel(1);
    harness.override_controls(ControlLayout::MacOs);
    let insets = harness.window_insets();
    assert_eq!(insets.header_left, 0., "the header kept a corner it lost");
    assert!(insets.panel_left > 0., "the panel took no reservation");

    let scene = harness.frame();
    let gear = gear_box(&scene);
    assert!(
        gear.min_x() >= insets.panel_left,
        "the panel's gear is at {} under a {} reservation",
        gear.min_x(),
        insets.panel_left
    );

    // The empty half of the control bar: left of the gear, right of the room
    // the traffic lights are painted in.
    harness.dispatch(Event::MouseDown {
        button: MouseButton::Left,
        position: vec2f((insets.panel_left + gear.min_x()) / 2., center(gear).y()),
        modifiers: Modifiers::default(),
        click_count: 1,
    });

    assert_eq!(harness.window_requests(), vec![Request::Drag]);
}

/// The usage chip's pill, by its fully-rounded box.
fn pill_box(scene: &Scene) -> RectF {
    let boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| rect.corner_radius.get_top_left() == Radius::Percentage(50.))
        .map(|(_, bounds)| bounds)
        // The status dots are round too, and much smaller.
        .filter(|bounds| bounds.width() > 40.)
        .collect();

    assert_eq!(boxes.len(), 1, "exactly one usage chip per frame");
    boxes[0]
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
    let mut harness = Harness::new(CROWDED);
    let ids = harness.tab_ids();
    let button = new_tab_box(&harness.frame());

    // Its left edge: the first place the active tab's close button reaches
    // once the strip is crowded enough for it to escape.
    harness.click(
        button.origin() + vec2f(2., button.height() / 2.),
        MouseButton::Left,
    );

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
/// edge) nor rounded (the usage chip, and the well a pane composes its next
/// command in — see [`composer_boxes`] for that one).
///
/// The one other thing that matches is the grid's own ground, painted inside
/// the pane it belongs to, so a rect contained in another is dropped. Without
/// that a shell test would count every pane twice.
fn panel_boxes(scene: &Scene) -> Vec<RectF> {
    let candidates: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.background == Fill::Solid(theme().surface)
                && rect.border == Border::default()
                && rect.corner_radius == CornerRadius::default()
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
fn the_gear_opens_the_menu_and_a_click_outside_closes_it() {
    let mut harness = Harness::seeded();
    assert!(!harness.is_menu_open());

    let gear = gear_box(&harness.frame());
    harness.click(center(gear), MouseButton::Left);
    assert!(harness.is_menu_open(), "the gear did not open the menu");

    let popup = menu_box(&harness.frame());
    // Well clear of the popup, and over the body — which must not react.
    let outside = vec2f(20., WINDOW.y() - 20.);
    assert!(!popup.contains_point(outside));
    harness.click(outside, MouseButton::Left);

    assert!(!harness.is_menu_open(), "a click outside left the menu up");
}

#[test]
fn re_clicking_the_gear_closes_the_menu_once_rather_than_twice() {
    // The gear is under the modal underlay while the menu is up, so its own
    // handler never fires and the press goes through the dismiss path. Letting
    // both run would toggle twice in one click and leave the menu looking
    // frozen open.
    let mut harness = Harness::seeded();
    let gear = gear_box(&harness.frame());

    harness.click(center(gear), MouseButton::Left);
    assert!(harness.is_menu_open());
    harness.frame();

    harness.click(center(gear), MouseButton::Left);
    assert!(
        !harness.is_menu_open(),
        "re-clicking the gear toggled twice"
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
fn the_gear_says_what_it_does_while_the_pointer_is_on_it_and_the_menu_is_down() {
    let mut harness = Harness::seeded();
    let gear = gear_box(&harness.frame());
    assert!(!frame_text(&harness.frame()).contains(controls::GEAR_TOOLTIP));

    harness.move_to(center(gear));
    let hovered = harness.frame();
    assert!(
        frame_text(&hovered).contains(controls::GEAR_TOOLTIP),
        "hovering the gear named nothing; the only way to find out what it \
         does is to click it"
    );
    // The tooltip is an anchored overlay, which contributes nothing to the
    // size of the stack it hangs off — a button that grew or moved when it was
    // named would slide out from under the pointer that named it.
    assert_eq!(gear, gear_box(&hovered), "the tooltip moved the gear");

    // Suppressed while the menu is up: the popup hangs off this button, and a
    // tooltip left there would sit between the gear and its own menu.
    harness.dispatch_option(OptionsAction::TogglePopup);
    assert!(
        !frame_text(&harness.frame()).contains(controls::GEAR_TOOLTIP),
        "the tooltip stayed up under the menu it opened"
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
        Self {
            directory: std::env::temp_dir()
                .join(format!("crook-tab-options-{}-{serial}", std::process::id())),
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
    // or, for the gear, a colour emoji.
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
        "the settings draw no gear, only {drawn:?}"
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
        .filter(|icon| icon.icon_key.icon == Lucide::X)
        .map(|icon| icon.bounds)
        .find(|bounds| menu.contains_point(center(*bounds)))
        .expect("no row offers a ×")
}

/// The worktree menu the active tab opens, by its popup box.
///
/// Found the way the options menu is: the one surface-raised, 6px-rounded box
/// that is wide enough to be it. The gear's menu is 200 wide and this is 260,
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

#[test]
fn right_clicking_a_tab_opens_its_worktree_menu() {
    // The gesture, and the whole of why it is this one: it is the button a
    // context menu opens on everywhere else. The left one used to do it — on
    // the row you were already in, where the click was otherwise free — and
    // that made the menu something you opened by accident on the way to the
    // tab you were already in.
    let mut harness = Harness::seeded();
    let scene = harness.frame();
    assert!(
        worktree_menu_box(&scene).is_none(),
        "the menu was up before anything was clicked"
    );

    let tab = tab_boxes(&scene)[0];
    harness.click(center(tab), MouseButton::Right);

    assert!(
        worktree_menu_box(&harness.frame()).is_some(),
        "a right press on a row did not open its menu"
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
        worktree_menu_box(&harness.frame()).is_none(),
        "a left click on the active row opened the worktree menu"
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
        worktree_menu_box(&harness.frame()).is_none(),
        "a second right press on the same row left the menu up"
    );
}

#[test]
fn a_tab_outside_a_repository_opens_no_menu() {
    // "If it is under git, there should be worktree options" — and if it is
    // not, the press does nothing. A menu that opened everywhere and was empty
    // half the time would teach people not to reach for it.
    let mut harness = Harness::new(1);
    let scene = harness.frame();
    let tab = tab_boxes(&scene)[0];

    harness.click(center(tab), MouseButton::Right);

    assert!(
        worktree_menu_box(&harness.frame()).is_none(),
        "a tab with no repository behind it opened a worktree menu"
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
        worktree_menu_box(&scene).is_none(),
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
        worktree_menu_box(&harness.frame()).is_some(),
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
    let before = harness.tab_ids().len();
    harness.dispatch_worktree(WorktreeAction::Create);
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.tab_ids().len() > before
    });

    // The tab that opened on it has to go before the × will be offered: a
    // checkout somebody is working in is exactly the one that must not be
    // removable, and that rule is what this test would otherwise trip over.
    harness.dispatch_action(TabAction::Close(harness.active_id()));

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

    let tabs = harness.tab_ids().len();
    harness.click(center(cross), MouseButton::Left);

    assert!(
        harness.worktree_menu_is_confirming(),
        "the × did not open the confirmation"
    );
    assert_eq!(
        harness.tab_ids().len(),
        tabs,
        "the row's own click fired too and opened a tab"
    );
}

#[test]
fn making_a_worktree_checks_it_out_and_opens_a_tab_in_it() {
    // The whole feature with a real repository and a real `git worktree add`
    // at the end of it: the menu reads the repository, the creator names a
    // branch nothing is using, git checks it out, and a tab opens whose shell
    // would start there.
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

    let before = harness.tab_ids().len();
    harness.dispatch_worktree(WorktreeAction::StartCreating);
    assert!(
        harness.worktree_menu_is_creating(),
        "the creator did not open"
    );

    harness.dispatch_worktree(WorktreeAction::Create);
    harness.wait_for("the worktree to be checked out", |harness| {
        harness.tab_ids().len() > before
    });

    // A tab, in a directory that is really there, which git really knows is a
    // worktree of the repository the menu was opened on.
    let opened = harness
        .workspace
        .read(&harness.app, |workspace, _| workspace.tab_directories())
        .into_iter()
        .map(|(_, directory)| directory)
        .find(|directory| directory.starts_with(&store))
        .expect("no tab was opened in the new checkout");
    assert!(opened.is_dir(), "{} was not checked out", opened.display());

    let listed = crate::git::worktree::list(&repository).expect("the repository still lists");
    assert!(
        listed.iter().any(|worktree| worktree.path == opened),
        "git does not know about the checkout that was made: {listed:?}"
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

/// Every row the panel painted, as (what it drew, what the clip left).
///
/// A row is the only 4px-rounded box inside the panel wider than a button: the
/// gear is 20 across, a close button 16, and the hover card hangs outside the
/// panel entirely.
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

    // Panes: 8px of the tab's own bottom padding, a hairline, and 8px of the
    // next tab's top padding — no gap in the list column at all.
    for gap in &panes_gaps {
        assert!(
            (*gap - 17.).abs() < 0.5,
            "Panes put {gap} between two tabs, not 8 + 1 + 8"
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
    assert_eq!(
        visible.len(),
        8,
        "the default combination fits {} tabs, and the module docs say eight",
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

    // The other extreme, and the other figure the docs quote.
    harness.set_options(TabOptions {
        density: Density::Expanded,
        ..harness.options()
    });
    assert_eq!(
        panel_rows(&harness.frame()).len(),
        6,
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

#[test]
fn right_clicking_a_panel_row_opens_the_worktree_menu_too() {
    // The panel and the strip answer the same gesture, because the rule is
    // about the tab rather than about how the tab is drawn.
    let mut harness = Harness::seeded_panel();
    let row = panel_rows(&harness.frame())[0];

    harness.click(
        row.origin() + vec2f(40., row.height() / 2.),
        MouseButton::Right,
    );

    assert!(
        worktree_menu_box(&harness.frame()).is_some(),
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
    // The whole of why the anchor is a parameter: the strip's gear hangs its
    // menu from the left edge, and the same rule in a 248px column would open
    // a 200px menu across the body.
    let mut harness = Harness::seeded_panel();
    let panel = panel_box(&harness.frame());

    let gear = gear_box(&harness.frame());
    assert!(
        panel.contains_point(center(gear)),
        "the gear is not in the panel"
    );
    harness.click(center(gear), MouseButton::Left);
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
        menu.min_y() >= gear.max_y(),
        "the menu opened over its own gear"
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
fn the_gear_tooltip_is_a_label_beside_the_gear_and_not_a_bar_down_the_window() {
    // An anchored overlay child is laid out against the whole window, and
    // `Align` returns `constraint.max` on every finite axis — so an `Align`
    // inside the tooltip measured 88 by the window's *height*: an 88px bar
    // from the top of the window to the bottom, straight down the tab list,
    // with the label stranded in the middle of it and every element under it
    // reporting itself covered.
    let mut harness = Harness::seeded_panel();
    let scene = harness.frame();
    let gear = gear_box(&scene);
    let plus = new_tab_box(&scene);
    let row = panel_rows(&scene)[0];

    harness.move_to(center(gear));
    let scene = harness.frame();
    let tooltips = detail_cards(&scene);
    assert_eq!(tooltips.len(), 1, "hovering the gear named nothing");
    let tooltip = tooltips[0];

    assert!(
        tooltip.height() < 2. * gear.height(),
        "the tooltip is {tooltip:?}, which is a bar rather than a label"
    );
    assert!(
        tooltip.min_y() >= gear.max_y(),
        "the tooltip at {tooltip:?} is not below the gear at {gear:?}"
    );
    // It floats over the top of the list while it is up, which is what a
    // tooltip does. What it must not do is run past it: the bar reached the
    // bottom of the window and covered every row on the way.
    assert!(
        tooltip.max_y() < row.max_y(),
        "the tooltip at {tooltip:?} outlasts the row at {row:?} it is drawn          over"
    );
    assert!(
        tooltip
            .intersection(plus)
            .is_none_or(|overlap| overlap.is_empty()),
        "the tooltip at {tooltip:?} covers the new-tab button at {plus:?}"
    );
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
    // The gear moved into the control bar, which paints *before* the list, so
    // a card opened from a row afterwards lands in a later overlay layer than
    // the menu — and covers the modal underlay whose whole job is to catch the
    // press that dismisses. `--menu --hover` applies the two in that order.
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

/// The text fields on the settings pane, left to right.
///
/// Found by the field's own ground, which nothing else on the page paints:
/// a rounded box in `overlay_1` exactly [`text_field::HEIGHT`] tall.
fn settings_field_boxes(scene: &Scene) -> Vec<RectF> {
    let mut fields: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, bounds)| {
            rect.background == Fill::Solid(theme().overlay_1)
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
/// It lives in the panel's control bar, which is the window's top-left corner
/// — so it is also what moves when the traffic lights come and go.
fn plus_box(scene: &Scene) -> RectF {
    let boxes: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| rect.corner_radius.get_top_left() == Radius::Pixels(5.))
        .map(|(_, bounds)| bounds)
        .collect();
    assert_eq!(boxes.len(), 1, "expected one + button, got {boxes:?}");
    boxes[0]
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
                && rect.border.color == Fill::Solid(theme().border)
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
fn creator_swatches(scene: &Scene) -> Vec<RectF> {
    let mut swatches: Vec<RectF> = visible_rects(scene)
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
fn opening_the_settings_page_from_the_gear_menu_takes_the_menu_down() {
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
    assert_eq!(rail.len(), 5, "five pages in the rail");

    // The fourth: Keys.
    harness.click(center(rail[3]), MouseButton::Left);
    assert_eq!("Keys", harness.settings_section());

    let text = frame_text(&harness.frame());
    assert!(
        text.contains("Tabs and panes"),
        "the Keys page did not come up: {text}"
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
fn a_switch_on_the_page_writes_the_option_the_gear_menu_writes() {
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
    // the row stays, greyed, with no handler. The gear menu takes the other —
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
fn turning_the_usage_chip_off_takes_the_pill_out_of_the_header_and_stops_the_poll() {
    let mut harness = Harness::new(1);
    assert!(harness.general().show_usage_chip);
    assert!(
        frame_text(&harness.frame()).contains("claude"),
        "the chip should be in the header to start with"
    );

    harness.open_settings_page();
    harness.select_settings_section("Usage");

    let switches = settings_switch_boxes(&harness.frame());
    assert_eq!(switches.len(), 1, "one switch on the usage page");
    harness.click(center(switches[0]), MouseButton::Left);

    assert!(!harness.general().show_usage_chip);
    assert!(
        !harness.usage_is_wanted(),
        "a hidden chip must not go on polling"
    );
    assert!(
        !frame_text(&harness.frame()).contains("claude"),
        "the chip is still in the header"
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

        harness.type_into(
            pane,
            "printf '\\033]0;deploy the release\\007\\033]7;file:///tmp\\007'\n",
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

        /// The copy controls drawn in a frame, by their rounded plate.
        ///
        /// Bounded to the pane, because the button that opens another tab is
        /// the same square with the same radius in the header above it.
        fn copy_controls(scene: &Scene, panel: RectF) -> Vec<RectF> {
            visible_rects(scene)
                .filter(|(rect, bounds)| {
                    rect.corner_radius.get_top_left() == Radius::Pixels(5.)
                        && (bounds.width() - bounds.height()).abs() < 0.5
                        && bounds.width() > 20.
                        && bounds.width() < 32.
                })
                .map(|(_, bounds)| bounds)
                .filter(|bounds| panel.contains_point(center(*bounds)))
                .collect()
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
                copy_controls(&harness.frame(), panel).is_empty(),
                "nothing is hovered, so no control is drawn"
            );

            let scene = hover_block(&mut harness, pane, 0);
            let controls = copy_controls(&scene, panel);
            assert_eq!(controls.len(), 1, "one control, on the hovered block");
            assert_eq!(
                icons_in(&scene, controls[0], Lucide::Copy).len(),
                1,
                "the control's plate is empty: it says nothing about what it does"
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
                copy_controls(&scene, panel).len(),
                1,
                "a block whose top has scrolled out of view drew no control"
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
                let controls = copy_controls(&scene, panel);
                assert_eq!(controls.len(), 1, "one control, on the hovered block");
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
        report(&mut harness, TerminalUpdate::Bell(ringing));

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

        report(&mut harness, TerminalUpdate::Bell(focused));

        assert_eq!(status_of(&harness, focused), Some(AgentStatus::Idle));
    }

    #[test]
    fn looking_at_a_pane_is_what_quiets_it() {
        let mut harness = Harness::new(2);
        let ringing = background_of(&harness);

        report(&mut harness, TerminalUpdate::Bell(ringing));
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

// ---------------------------------------------------------------- ADVERSARIAL
/// The title bar's two halves, checked against each other: every control in
/// the header still answers a click, and every gap between them still picks
/// the window up.
#[cfg(test)]
mod title_bar_hit_testing {
    use super::*;

    /// The header's own box: the full-width surface rect at the top.
    fn header_box(scene: &Scene) -> RectF {
        fills_of(scene, theme().surface)
            .into_iter()
            .find(|bounds| bounds.min_y() == 0. && bounds.width() > 512.)
            .expect("the header paints its own surface")
    }

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

        // The gear opens its menu.
        let scene = harness.frame();
        harness.click(center(gear_box(&scene)), MouseButton::Left);
        assert!(harness.is_menu_open(), "the gear stopped opening the menu");
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

        // And none of the four dragged the window.
        assert!(
            harness.window_requests().is_empty(),
            "a control in the header dragged the window: {:?}",
            harness.window_requests()
        );
    }

    #[test]
    fn the_chip_and_the_tabs_swallow_a_double_click_rather_than_maximising() {
        for target in ["tab", "chip", "gear", "plus"] {
            let mut harness = Harness::new(2);
            let scene = harness.frame();
            let at = match target {
                "tab" => center(tab_boxes(&scene)[0]),
                "chip" => center(pill_box(&scene)),
                "gear" => center(gear_box(&scene)),
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
        // The `+` and the gear are the panel's; what is left in this row is
        // whatever a plugin pinned to the right of it and the caption cluster.
        let scene = Harness::new(2).frame();
        let panel = panel_box(&scene);
        let chip = pill_box(&scene);
        let row = center(chip).y();

        // Between the panel and the chip, just left of the chip, and above it.
        let gaps = [
            vec2f((panel.max_x() + chip.min_x()) / 2., row),
            vec2f(chip.min_x() - 8., row),
            vec2f(center(chip).x(), 2.),
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
        let empty = vec2f(pill_box(&scene).min_x() - 30., center(pill_box(&scene)).y());

        harness.click_times(empty, 1);
        harness.click_times(empty, 2);
        harness.click_times(empty, 3);

        assert_eq!(
            harness.window_requests(),
            vec![Request::Drag, Request::ToggleMaximized, Request::Drag],
            "the press after a double click was not a drag"
        );
    }

    #[test]
    fn the_caption_cluster_starts_at_the_top_of_the_window() {
        // The buttons belong to the window, not to the row they are drawn in:
        // every desktop that draws them puts them hard against the top of the
        // window, and the top-right corner is where a person throws the
        // pointer to close one. Hung from the header's bottom edge instead —
        // which is what the row's own `CrossAxisAlignment::End` does to them —
        // they leave a strip of inert header above the close button.
        for (layout, button) in [
            (ControlLayout::Windows, vec2f(45., 30.)),
            (ControlLayout::Freedesktop, vec2f(30., 30.)),
        ] {
            let mut harness = Harness::new(1);
            harness.override_controls(layout);
            let scene = harness.frame();
            let header = header_box(&scene);
            let buttons = caption_boxes(&scene, button);

            assert_eq!(
                buttons[0].min_y(),
                0.,
                "{layout:?} starts its caption buttons {} below the top of the window, \
                 inside a header {} tall",
                buttons[0].min_y(),
                header.height()
            );
        }
    }

    #[test]
    fn the_top_right_corner_of_the_window_closes_it_rather_than_dragging_it() {
        // The one above as a gesture: two pixels in from the top-right corner
        // is the close button on Windows, and nothing there may pick the
        // window up instead.
        let mut harness = Harness::new(1);
        harness.override_controls(ControlLayout::Windows);
        harness.frame();

        press(&mut harness, vec2f(WINDOW.x() - 2., 2.), 1);

        assert!(
            harness.window_requests().is_empty(),
            "the top-right corner of the window asked for {:?}",
            harness.window_requests()
        );
    }

    #[test]
    fn the_resize_border_keeps_out_of_every_caption_button() {
        // The corner the border is told to leave alone has to be the cluster
        // itself. `App::handle_resize_border` runs *before* the element tree
        // sees a press, so any part of a button outside that corner is a
        // button that resizes the window instead of doing what it says — on
        // Windows the last five columns of the close button and the top five
        // rows of all three, which is exactly where a corner-aimed pointer
        // lands. The other half of this — that nothing inside the corner is an
        // edge — is `chrome.rs`'s own test.
        for (layout, button) in [
            (ControlLayout::Windows, vec2f(45., 30.)),
            (ControlLayout::Freedesktop, vec2f(30., 30.)),
        ] {
            let mut harness = Harness::new(1);
            harness.override_controls(layout);
            let scene = harness.frame();
            let area = super::super::caption_area(layout, WindowChrome::Client)
                .expect("{layout:?} draws its own controls");
            let kept = RectF::new(vec2f(WINDOW.x() - area.x(), 0.), area);

            for bounds in caption_boxes(&scene, button) {
                let inside = bounds.min_x() >= kept.min_x()
                    && bounds.max_x() <= kept.max_x()
                    && bounds.min_y() >= kept.min_y()
                    && bounds.max_y() <= kept.max_y();
                assert!(
                    inside,
                    "{layout:?} draws a caption button at {bounds:?}, outside the {kept:?} \
                     the resize border was told to keep out of"
                );
            }
        }
    }
}

#[test]
fn a_chord_bound_to_a_plugins_action_reaches_the_plugin() {
    // The end-to-end of a named action, and the thing that was impossible
    // before there was one: `crook/usage/refresh` is registered by a plugin,
    // named in a file the application does not compile, and reached by a chord
    // this build has no arm for.
    let mut harness = Harness::new(1);
    assert!(!harness.usage_is_busy_for_user());

    harness.bind(r#"{"cmd-shift-u": "crook/usage/refresh"}"#);
    harness.press(
        "u",
        Modifiers {
            cmd: true,
            shift: true,
            ..Modifiers::default()
        },
        "",
    );

    assert!(
        harness.usage_is_busy_for_user(),
        "the chord did not reach the plugin's action"
    );
}

#[test]
fn a_chord_bound_to_an_action_nothing_answers_to_does_nothing() {
    // A keymap written for a plugin that is not installed, which is the
    // ordinary state of any keymap somebody copied from a friend. It costs
    // that one chord and nothing else.
    let mut harness = Harness::new(1);

    harness.bind(r#"{"cmd-shift-u": "eugen/not-installed/go"}"#);

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
    assert!(!harness.usage_is_busy_for_user());
}

#[test]
fn the_keys_page_lists_what_a_plugin_registered_and_the_chord_that_reaches_it() {
    // Where somebody finds out an action's name, which is the only way they
    // can bind it. The page cannot have been written with this row in it: the
    // name belongs to a plugin.
    let mut harness = Harness::new(1);
    harness.bind(r#"{"cmd-shift-u": "crook/usage/refresh"}"#);
    harness.open_settings_page();
    let rail = settings_rail_boxes(&harness.frame());
    harness.click(center(rail[3]), MouseButton::Left);
    assert_eq!("Keys", harness.settings_section());

    let text = frame_text(&harness.frame());

    assert!(
        text.contains("crook/usage/refresh"),
        "the Keys page does not name the action: {text}"
    );
    assert!(
        text.contains("cmd-shift-u"),
        "the Keys page does not say what reaches it: {text}"
    );
}

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
    assert!(!frame_text(&harness.frame()).contains("Run a command"));

    harness.press("p", palette_chord(), "");
    let text = frame_text(&harness.frame());

    assert!(
        text.contains("Run a command"),
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

    assert!(text.contains("No command matches that."), "{text}");
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
        !text.contains("Run a command"),
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

    assert!(!text.contains("Run a command"), "{text}");
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

#[test]
fn the_settings_rail_lists_the_pages_the_plugins_contributed() {
    // Through the real presenter, because the rail's order is worked out from
    // what is registered and the assertion worth making is about pixels.
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    let scene = harness.frame();

    let rail = settings_rail_boxes(&scene);
    assert_eq!(rail.len(), 5, "five pages in the rail");
    // Top to bottom, which is the `order` each plugin asked for.
    assert_eq!(harness.settings_section(), "Appearance");

    harness.click(center(rail[2]), MouseButton::Left);
    assert_eq!(harness.settings_section(), "Usage");
    assert!(
        frame_text(&harness.frame()).contains("Show the usage chip"),
        "the Usage page did not come up"
    );
}

#[test]
fn a_settings_page_can_be_reached_by_the_name_on_its_rail_row() {
    // What `--settings usage` resolves through, and the only name a person
    // ever sees: the key is `owner/entry` and nobody types that.
    let harness = Harness::new(1);

    let (found, missing) = harness.workspace.read(&harness.app, |workspace, _| {
        (
            workspace.settings_page_named("usage"),
            workspace.settings_page_named("nonesuch"),
        )
    });

    assert!(found.is_some(), "the Usage page is not reachable by name");
    assert!(missing.is_none());
}

mod sandboxed {
    use super::*;
    use crate::plugins::wasm::tests::{Scratch, install, wasm};

    /// A harness whose plugins are the ones in the box plus whatever is in a
    /// scratch directory.
    fn harness(scratch: &Scratch) -> Harness {
        let mut plugins = crate::plugins::defaults();
        plugins.extend(crate::plugins::wasm::installed(scratch.path()));
        Harness::with_plugins(1, Settings::ephemeral(), plugins)
    }

    #[test]
    fn what_a_sandboxed_plugin_describes_is_what_the_window_draws() {
        // The whole of the second tier, end to end: a `.wasm` file in a
        // directory, run in an interpreter, describing a row it never painted
        // — and the window draws it in the theme in force, in Crook's own
        // fonts, with Crook's own icons.
        //
        // At order -1 so it wins `header.right`, which is a `Single` slot the
        // usage chip is already in. That is the whole of what "change
        // practically everything" has to mean for a store plugin: it can
        // *replace* something Crook ships.
        let scratch = Scratch::new("draws");
        install(
            scratch.path(),
            "probe",
            &wasm("eugen/probe", "header.right", -1),
        );
        let mut harness = harness(&scratch);

        let text = frame_text(&harness.frame());

        assert!(text.contains("from a sandbox"), "{text}");
        assert!(text.contains("probed"), "the badge is missing: {text}");
        assert!(text.contains("Poke"), "the button is missing: {text}");
        assert!(
            !text.contains("claude"),
            "the plugin did not win the slot: {text}"
        );
        // And the icon it named by string is drawn as one of Crook's own.
        assert!(
            icons_of(&harness.frame()).contains(&Lucide::GitBranch),
            "the icon it asked for was not drawn"
        );
    }

    #[test]
    fn a_sandboxed_plugin_is_on_the_plugins_page_beside_the_ones_in_the_box() {
        let scratch = Scratch::new("listed");
        install(
            scratch.path(),
            "probe",
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

    #[test]
    fn a_sandboxed_plugins_action_is_reachable_by_name_like_any_other() {
        // Prefixed by the host with the plugin's own id, so a guest cannot
        // claim an action belonging to anybody else however it spells its own.
        let scratch = Scratch::new("action");
        install(
            scratch.path(),
            "probe",
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
    fn a_contribution_to_a_slot_this_build_does_not_have_is_refused_and_nothing_else() {
        // A plugin written against a Crook with a slot this one does not have
        // should be missing that one contribution, not missing entirely.
        let scratch = Scratch::new("unknown-slot");
        install(
            scratch.path(),
            "probe",
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
    fn the_page_is_a_list_of_every_plugin_beside_a_card_about_one() {
        let mut harness = harness();
        let text = frame_text(&harness.frame());

        // Every plugin the binary carries is in the list, whether or not it
        // is running: a switch you cannot see is a switch you cannot turn
        // back on.
        for name in [
            "Window commands",
            "Header",
            "Usage chip",
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

        harness.click_plugin("Usage chip");
        let text = frame_text(&harness.frame());

        assert!(text.contains("crook/usage"), "{text}");
        assert!(
            text.contains("How much of the Claude Code"),
            "the card does not describe it: {text}"
        );
        // What it puts on screen, asked of the host rather than of the plugin.
        assert!(text.contains("header.right"), "{text}");
        assert!(text.contains("crook/usage/refresh"), "{text}");
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
        let mut harness = Harness::new(1);
        assert!(frame_text(&harness.frame()).contains("claude"));
        harness.show_plugins();
        harness.click_plugin("Usage chip");

        let switches = settings_switch_boxes(&harness.frame());
        assert_eq!(switches.len(), 1, "one switch, on the card");
        harness.click(center(switches[0]), MouseButton::Left);

        let text = frame_text(&harness.frame());
        assert!(
            !text.contains("claude"),
            "the chip outlived the plugin: {text}"
        );
        assert!(
            text.contains("switched off"),
            "the card does not say it is off: {text}"
        );
        // And the Usage page went with it, because that page was the
        // plugin's too. Asked of the host: the sidebar is showing the plugin
        // list rather than the settings rail.
        assert!(
            harness
                .workspace
                .read(&harness.app, |workspace, _| workspace
                    .host()
                    .settings_page_id("crook/usage/page"))
                .is_none()
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
