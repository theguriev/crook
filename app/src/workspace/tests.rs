//! The header, driven through a real presenter.
//!
//! These run the actual view tree — `Workspace::render`, layout, paint, hit
//! testing — against a stub shaper, so they cover the wiring a unit test of
//! the strip cannot: that a click lands on the tab under it, that the close
//! button closes its own tab, and that revealing that button does not move
//! the bar out from under the cursor.

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

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
use crate::settings::{
    Density, GeneralOptions, Granularity, Layout, PrimaryInfo, Settings, Subtitle, TabOptions,
};
use crate::tab::{AgentSession, AgentStatus, Direction, Pane, PaneId, Tab, TabAction, TabId};
use crate::terminal_font::{CELL_FONT_SIZE, CellFont};
use crate::theme::theme;
use crate::usage_model::UsageModel;

use super::{
    Fonts, OptionsAction, QuitRequest, Section, SettingsAction, Workspace, WorkspaceAction,
    controls, settings_page, tab_options_menu, tabs_panel,
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

/// Settings that live only as long as the test, holding `layout`.
///
/// Written into the settings rather than set on the workspace afterwards, so
/// that `saved_options` and `options` agree from the first frame — a harness
/// whose file disagreed with its screen would make every override test lie.
fn ephemeral_settings(layout: Layout) -> Settings {
    let mut settings = Settings::ephemeral();
    settings.set_tab_options(TabOptions {
        layout,
        ..TabOptions::default()
    });
    settings
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
}

impl Harness {
    /// A window with `tabs` tabs in the horizontal strip, the last of which is
    /// active, and one frame already drawn so there is something to hit-test
    /// against.
    ///
    /// The layout is asked for rather than inherited, because it is *not* the
    /// one Crook opens in: the default is the panel, and everything below this
    /// point is about the strip. [`Harness::panel`] is the other half.
    ///
    /// Ephemeral: a test run must not read, and must not rewrite, the tab
    /// options of whoever is running it.
    fn new(tabs: usize) -> Self {
        Self::with_settings(tabs, ephemeral_settings(Layout::Horizontal))
    }

    /// The same, in the layout Crook actually opens in.
    fn panel(tabs: usize) -> Self {
        Self::with_settings(tabs, Settings::ephemeral())
    }

    /// The same, on settings that know where they would be written.
    ///
    /// The only way to test what a save would put in the file: `Settings`
    /// carries the path, and `Workspace::save_settings` returns before doing
    /// anything at all when there is none.
    fn with_settings(tabs: usize, settings: Settings) -> Self {
        let queue = LocalQueue::new();
        let mut app = App::new(queue.foreground(), Arc::new(Background::new(1)));
        app.update(|ctx| ctx.add_singleton_model(UsageModel::new));

        let quit_requests = Rc::new(Cell::new(0));
        let quit: QuitRequest = {
            let requests = quit_requests.clone();
            Rc::new(move || requests.set(requests.get() + 1))
        };

        let fonts = Fonts {
            ui: FamilyId(0),
            monospace: FamilyId(0),
        };
        // Measured with no font backend, exactly as the stub shaper above lays
        // text out with none: a cell is half the font size, and a character is
        // its own glyph id.
        let cell_font = CellFont::headless(CELL_FONT_SIZE);
        let (window_id, workspace) = app
            .add_window(|ctx| Workspace::new(fonts, cell_font, settings, Channel::Dev, quit, ctx));

        let mut harness = Self {
            queue,
            app,
            presenter: Presenter::new(window_id, Arc::new(StubShaper)),
            window_id,
            workspace,
            quit_requests,
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

    /// Moves the tabs between the panel and the strip, the way the keybinding
    /// does.
    fn toggle_layout(&mut self) {
        self.dispatch_option(OptionsAction::ToggleLayout);
    }

    /// Starts in a layout the command line asked for, the way `--layout` does.
    fn override_layout(&mut self, layout: Layout) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| workspace.override_layout(layout, ctx));
        });
    }

    /// Where this layout says the window's own controls land.
    fn window_insets(&self) -> crate::platform_insets::LayoutInsets {
        self.workspace
            .read(&self.app, |workspace, _| workspace.window_insets())
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

    /// The options as they would be written to the file.
    fn saved_options(&self) -> TabOptions {
        self.workspace
            .read(&self.app, |workspace, _| workspace.settings().tab_options())
    }

    fn is_menu_open(&self) -> bool {
        self.workspace
            .read(&self.app, |workspace, _| workspace.is_options_menu_open())
    }

    /// Opens the settings tab, or brings it forward — the keystroke's action,
    /// sent the way the gear menu's entry sends it.
    fn open_settings_page(&mut self) {
        self.dispatch_action(TabAction::OpenSettings);
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

    /// The name of the theme in force.
    fn theme_name(&self) -> String {
        self.workspace
            .read(&self.app, |workspace, _| workspace.theme_name().to_owned())
    }

    /// Which page of the settings the rail has selected.
    fn settings_section(&self) -> Section {
        self.workspace
            .read(&self.app, |workspace, _| workspace.settings_section())
    }

    /// Shows a different page, the way a click on the rail does.
    fn select_settings_section(&mut self, section: Section) {
        self.dispatch_workspace_action(WorkspaceAction::Settings(SettingsAction::Select(section)));
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
                .and_then(|pane| pane.session()?.working_directory.clone())
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
    fn start_terminals(&mut self) -> bool {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| workspace.start_terminals(ctx));
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
                .and_then(|pane| pane.session()?.working_directory.clone())
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
            let session = workspace.tabs().pane(pane)?.session()?;
            workspace.git_facts(session, app)?.branch.clone()
        })
    }
}

/// How long a test waits for a shell to do as it was told.
const SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

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

/// The tabs, by their rounded-top boxes, in bar order.
fn tab_boxes(scene: &Scene) -> Vec<RectF> {
    rects_rounded_by(scene, Radius::Pixels(8.))
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

/// The input fields that are currently drawn, by their rounded boxes.
fn field_boxes(scene: &Scene) -> Vec<RectF> {
    rects_rounded_by(scene, Radius::Pixels(crate::workspace::body::FIELD_RADIUS))
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

/// What the strip says, without the body panel underneath it.
///
/// The body prints the session's title and its working directory too, so a
/// whole-frame search cannot tell "the row stopped showing this" from "the row
/// never showed it".
fn strip_text(scene: &Scene) -> String {
    let bottom = tab_boxes(scene)
        .iter()
        .map(|tab| tab.max_y())
        .fold(0., f32::max);
    text_where(scene, |position| position.y() <= bottom)
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

/// The info dot on "Show: PR link", by its round box.
fn info_dot_box(scene: &Scene) -> RectF {
    let popup = menu_box(scene);
    let dots: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Percentage(50.)
                && (rect.bounds.width() - tab_options_menu::INFO_DOT_SIZE).abs() < 0.5
        })
        .map(|(_, bounds)| bounds)
        .filter(|bounds| popup.contains_point(center(*bounds)))
        .collect();

    assert_eq!(dots.len(), 1, "exactly one info dot in the popup");
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
        .filter(|(_, row)| row_text(scene, *row).contains('\u{2713}'))
        .map(|(index, _)| index)
        .collect()
}

/// What one option row says, check glyph included.
fn row_text(scene: &Scene, row: RectF) -> String {
    text_where(scene, |position| {
        row.contains_point(position) || row.contains_point(position - vec2f(0., 1.))
    })
}

/// Where a row's label starts: the leftmost glyph that is not the check.
fn row_label_x(scene: &Scene, row: RectF) -> f32 {
    scene
        .layers()
        .flat_map(|layer| layer.glyphs.iter())
        .filter(|glyph| row.contains_point(glyph.position))
        .filter(|glyph| glyph.glyph_key.glyph_id != u32::from('\u{2713}'))
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
fn a_natively_decorated_window_leaves_no_gap_for_controls_it_does_not_draw() {
    // The window manager draws Crook's controls in a bar of its own, above the
    // client area. Reserving for them anyway costs 136px of header on Windows
    // and 116 on Linux — a hole nobody developing on macOS would ever see, and
    // a first tab pushed 64px in for macOS itself.
    let mut harness = Harness::new(1);
    let scene = harness.frame();
    let first = tab_boxes(&scene)[0];
    let chip = pill_box(&scene);

    // The header's own padding, and nothing else.
    assert!(
        first.min_x() < 24.,
        "the first tab starts at {}, which is a window-control reservation",
        first.min_x()
    );
    assert!(
        WINDOW.x() - chip.max_x() < 24.,
        "the chip stops {} short of the right edge",
        WINDOW.x() - chip.max_x()
    );
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
fn a_tab_too_narrow_for_its_close_button_does_not_put_it_over_its_neighbour() {
    let mut harness = Harness::new(CROWDED);
    let scene = harness.frame();
    let tabs = tab_boxes(&scene);
    assert_eq!(tabs.len(), CROWDED);

    // Nothing a tab draws may end up outside the tab that drew it: an escaped
    // close button hit-tests where it paints, so it would be a live "close my
    // neighbour" button sitting on the neighbour.
    for button in close_boxes(&scene) {
        let inside_a_tab = tabs
            .iter()
            .any(|tab| tab.min_x() <= button.min_x() && button.max_x() <= tab.max_x());
        assert!(
            inside_a_tab,
            "a close button is drawn at {button:?}, outside every tab"
        );
    }
}

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
fn the_strip_never_runs_out_of_the_header_however_many_tabs_there_are() {
    // Far past the point where a tab is narrower than its own padding, which
    // is where a container that reports the size it wanted rather than the
    // size it was given pushes every later sibling off the end of the window.
    let mut harness = Harness::new(60);
    let scene = harness.frame();
    let tabs = tab_boxes(&scene);
    let button = new_tab_box(&scene);
    let chip = pill_box(&scene);

    let last = tabs.last().expect("60 tabs");
    assert!(
        last.max_x() <= button.min_x(),
        "the last tab ends at {} and the new-tab button starts at {}",
        last.max_x(),
        button.min_x()
    );
    assert!(
        button.max_x() <= chip.min_x(),
        "the new-tab button ends at {} and the usage chip starts at {}",
        button.max_x(),
        chip.min_x()
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

/// The chips drawn as the selected one, by their active fill.
fn selected_chips(scene: &Scene) -> Vec<RectF> {
    visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(8.)
                && rect.background == Fill::Solid(theme().tab_active)
        })
        .map(|(_, bounds)| bounds)
        .collect()
}

/// The body's panes, in render order.
///
/// A pane has no chrome left to find it by — no corner radius, no border, no
/// margin — and its fill is the terminal's own ground, which for a grid that
/// has not been told otherwise is the same `surface` the header and the tabs
/// panel are painted in. So it is found by what it is *not*: filled like a
/// terminal, and neither bordered (the header's underline, the panel's right
/// edge) nor rounded (the usage chip, and the well a pane composes its next
/// command in — see [`field_boxes`] for that one).
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
fn exactly_one_chip_in_the_whole_bar_is_drawn_as_the_selected_one() {
    // Warp's `is_selected = is_active_tab && is_focused`, read off the pixels.
    // Tinting every row of the active tab and marking the focused pane on top
    // of that is the easy mistake, and it gives a bar where three chips look
    // chosen.
    let mut harness = Harness::new(2);
    harness.dispatch_action(TabAction::Split(Direction::Right));
    harness.dispatch_action(TabAction::Split(Direction::Down));

    for granularity in [Granularity::Panes, Granularity::Tabs] {
        harness.set_granularity(granularity);
        let scene = harness.frame();

        assert_eq!(
            selected_chips(&scene).len(),
            1,
            "{granularity:?} draws more than one selected chip"
        );
    }
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
        0.,
        "the pane starts at the window's left edge"
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
    let panels = panel_boxes(&scene);
    assert!(
        panels
            .iter()
            .any(|panel| panel.contains_point(center(popup))),
        "the popup does not overhang a panel, so there is nothing to occlude"
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
        cards[0].min_y() >= row.max_y(),
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
    // The one place Crook's defaults are not Warp's, asserted through the
    // pixels rather than through the settings struct: a default that never
    // reached the renderer would still pass `settings.rs`'s test.
    let mut harness = Harness::panel(2);
    let scene = harness.frame();

    assert_eq!(Layout::Vertical, harness.options().layout);
    assert_eq!(
        0.,
        panel_box(&scene).min_x(),
        "the panel is not at the edge"
    );
    assert_eq!(panel_rows(&scene).len(), 2, "one row per tab");
    assert!(
        tab_boxes(&scene).is_empty(),
        "the header drew tab items while the panel was up; the two layouts \
         are mutually exclusive and both would be taking the same clicks"
    );
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
        9,
        "the default combination fits {} tabs, and the module docs say nine",
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
        7,
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

// --- moving the tabs between the two layouts ---------------------------------

#[test]
fn the_layout_keybinding_moves_the_tabs_and_the_gear_with_them() {
    let mut harness = Harness::panel(2);
    let panel_gear = gear_box(&harness.frame());
    assert!(panel_box(&harness.frame()).contains_point(center(panel_gear)));

    harness.toggle_layout();

    let scene = harness.frame();
    assert_eq!(Layout::Horizontal, harness.options().layout);
    assert_eq!(tab_boxes(&scene).len(), 2, "the strip drew no tabs");
    assert!(
        visible_rects(&scene).all(|(rect, _)| (rect.bounds.width() - tabs_panel::PANEL_WIDTH)
            .abs()
            > 0.5
            || rect.background != Fill::Solid(theme().surface)),
        "the panel is still painted beside the strip"
    );
    // The gear went with the tabs: it is now at the far end of the header
    // rather than at the top of a panel that no longer exists.
    assert!(gear_box(&scene).min_x() > tabs_panel::PANEL_WIDTH);

    harness.toggle_layout();
    assert_eq!(Layout::Vertical, harness.options().layout);
    assert!(tab_boxes(&harness.frame()).is_empty());
}

#[test]
fn the_sidebar_chord_is_what_moves_the_tabs() {
    let harness = Harness::panel(1);
    let action = harness.workspace.read(&harness.app, |workspace, _| {
        workspace.action_for(&Keystroke::new("b", platform_chord()))
    });

    assert_eq!(
        Some(WorkspaceAction::Options(OptionsAction::ToggleLayout)),
        action,
        "the sidebar chord is in --help's KEYS list and is bound to nothing"
    );
}

#[test]
fn the_layout_the_command_line_asked_for_is_never_written_to_the_settings_file() {
    // `--layout horizontal` is a way to look at a frame. The file says
    // vertical, and a menu click — which saves the *whole* options snapshot —
    // must not carry the override into it.
    let scratch = Scratch::new();
    let mut harness = Harness::with_settings(1, scratch.settings());
    assert_eq!(Layout::Vertical, harness.saved_options().layout);

    harness.override_layout(Layout::Horizontal);
    assert_eq!(
        Layout::Horizontal,
        harness.options().layout,
        "the override never reached the renderer"
    );
    assert_eq!(tab_boxes(&harness.frame()).len(), 1, "the strip is not up");

    harness.dispatch_option(OptionsAction::ToggleShowDetailsOnHover);

    let written = scratch.written_containing("\"show_details_on_hover\": false");
    assert!(
        written.contains("\"layout\": \"vertical\""),
        "the command line's layout reached the settings file, so the next \
         launch with no flags opens horizontal; the file says {written}"
    );
    assert_eq!(Layout::Vertical, harness.saved_options().layout);
}

#[test]
fn choosing_the_layout_ends_the_override_the_way_choosing_a_density_does() {
    let scratch = Scratch::new();
    let mut harness = Harness::with_settings(1, scratch.settings());
    harness.override_layout(Layout::Horizontal);

    // The toggle always moves the value, so unlike the density there is no
    // "already on screen" case — what has to happen is that the save carries
    // the new layout rather than reaching back for the file's.
    harness.toggle_layout();
    assert_eq!(Layout::Vertical, harness.options().layout);
    harness.toggle_layout();

    scratch.written_containing("\"layout\": \"horizontal\"");
    assert_eq!(harness.options(), harness.saved_options());
}

#[test]
fn the_layout_decides_which_element_owes_the_window_controls() {
    // The reservation follows the two top corners of the window, and which
    // element owns each corner is what the layout changes. `platform_insets`
    // holds the per-platform table; this is the one line that picks a column
    // out of it, and it is invisible on a natively decorated window — which is
    // every window Crook opens today.
    use crate::platform_insets::{TabsPlacement, layout_insets};

    let mut harness = Harness::panel(1);
    assert_eq!(
        layout_insets(TabsPlacement::LeftPanel, crate::WINDOW_CHROME, false),
        harness.window_insets()
    );

    harness.toggle_layout();
    assert_eq!(
        layout_insets(TabsPlacement::Header, crate::WINDOW_CHROME, false),
        harness.window_insets()
    );
}

#[test]
fn moving_the_tabs_makes_every_control_forget_the_pointer() {
    // The whole element tree is replaced, so nothing the pointer was on ever
    // sees a hover-out. Left alone, a row hovered in the panel comes back
    // hovered in the strip with the pointer nowhere near it — and its close
    // button then swallows the row's next click.
    let mut harness = Harness::seeded_panel();
    let rows = panel_rows(&harness.frame());
    harness.move_to(center(rows[0]));
    assert_eq!(detail_cards(&harness.frame()).len(), 1, "no card to lose");

    harness.toggle_layout();

    let scene = harness.frame();
    assert!(
        detail_cards(&scene).is_empty(),
        "a card the pointer opened in the panel is still up in the strip"
    );
    assert_eq!(
        close_boxes(&scene).len(),
        1,
        "a row other than the selected one still believes it is hovered"
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
    harness.override_layout(Layout::Horizontal);
    harness.override_density(Density::Expanded);

    // The density that is already on screen: the one click that takes the
    // early-save branch, and the case that branch exists for.
    harness.dispatch_option(OptionsAction::SetDensity(Density::Expanded));

    let written = scratch.written_containing("\"view_mode\": \"expanded\"");
    assert!(
        written.contains("\"layout\": \"vertical\""),
        "choosing a density adopted `--layout horizontal` as well, so the \
         next launch with no flags opens in the strip; the file holds {written}"
    );
    assert_eq!(Layout::Vertical, harness.saved_options().layout);
    assert_eq!(
        Layout::Horizontal,
        harness.options().layout,
        "the save changed what is on screen"
    );
}

/// The settings pane's panel.
///
/// The settings page is a pane, so it has no chrome of its own: what bounds it
/// is the body panel every pane gets. In a window whose active tab is the
/// settings tab there is exactly one panel, and this is it.
fn settings_pane_box(scene: &Scene) -> RectF {
    let panels = panel_boxes(scene);
    assert_eq!(
        panels.len(),
        1,
        "the settings tab holds one pane, so its body has one panel"
    );
    panels[0]
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

/// The rail's page buttons, top to bottom.
///
/// Found by their rounded box *and* by being in the rail's column, because the
/// page beside them rounds its one button by the same six pixels.
fn settings_rail_boxes(scene: &Scene) -> Vec<RectF> {
    let pane = settings_pane_box(scene);
    let mut rows: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| rect.corner_radius.get_top_left() == Radius::Pixels(6.))
        .map(|(_, bounds)| bounds)
        .filter(|bounds| {
            pane.contains_point(center(*bounds))
                && center(*bounds).x() < pane.min_x() + settings_page::RAIL_WIDTH
        })
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
        .find(|bounds| {
            pane.contains_point(center(*bounds))
                && center(*bounds).x() > pane.min_x() + settings_page::RAIL_WIDTH
        })
}

/// The theme cards on the settings page, top to bottom.
///
/// Found by the one thing only a card is: a rounded box painted in a theme's
/// *terminal* background. Every other rounded box on the page is painted in
/// the palette in force, and a card is the one element in Crook drawn in a
/// palette that is not.
fn theme_cards(scene: &Scene) -> Vec<RectF> {
    let pane = settings_pane_box(scene);
    let mut cards: Vec<RectF> = visible_rects(scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                && rect.border.width >= 1.
                && (rect.bounds.width() - 152.).abs() < 0.5
        })
        .map(|(_, bounds)| bounds)
        .filter(|bounds| pane.contains_point(center(*bounds)))
        .collect();
    cards.sort_by(|left, right| left.min_y().total_cmp(&right.min_y()));
    cards
}

#[test]
fn choosing_a_theme_repaints_the_window_saves_it_and_reaches_the_shells() {
    // The whole of what applying a theme has to do. The last of the three is
    // the one that is easy to miss: a grid resolves its colours through a
    // palette handed to it when its shell started, so a theme that stopped at
    // the edge of the terminal would be the least useful half of a theme.
    let scratch = Scratch::new();
    let mut harness = Harness::with_settings(1, scratch.settings());
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);

    harness.open_settings_page();
    let cards = theme_cards(&harness.frame());
    assert_eq!(cards.len(), 3, "three built-in themes, three cards");

    // The second card is the light one, and it is the one that proves a theme
    // reaches everything: nothing about a dark theme replacing another dark
    // theme is visible in a test.
    harness.click(center(cards[1]), MouseButton::Left);

    assert_eq!(harness.theme_name(), "Crook Light");
    assert!(
        crate::theme::theme().is_light,
        "the palette every view reads did not change"
    );
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
fn the_card_of_the_theme_in_force_is_the_marked_one() {
    let mut harness = Harness::new(1);
    let _guard = crate::theme::ThemeGuard::new(crate::theme::DARK);
    harness.open_settings_page();

    // A selected card is outlined at two pixels and the rest at one, which is
    // Warp's way of marking the chosen one *on* the preview rather than beside
    // it.
    let scene = harness.frame();
    let widths: Vec<f32> = visible_rects(&scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Pixels(6.)
                && (rect.bounds.width() - 152.).abs() < 0.5
        })
        .map(|(rect, _)| rect.border.width)
        .collect();

    assert_eq!(
        widths.iter().filter(|width| **width > 1.5).count(),
        1,
        "exactly one card should be drawn as the chosen one: {widths:?}"
    );
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
fn the_settings_page_opens_in_a_tab_and_the_binding_brings_that_tab_forward() {
    // Warp's `open_settings_pane`: one settings pane per window. A second
    // press is a navigation to it, never a second page and never a toggle —
    // which is also why the keystroke resolves to a `TabAction` rather than to
    // anything the settings page owns.
    let mut harness = Harness::new(2);
    let before = harness.tab_ids();
    assert!(!harness.is_settings_page_open());

    assert_eq!(
        Some(WorkspaceAction::Tab(TabAction::OpenSettings)),
        harness.action_for(",", settings_chord())
    );
    assert!(harness.press_key(",", settings_chord()));

    let with_settings = harness.tab_ids();
    assert_eq!(with_settings.len(), before.len() + 1, "no tab was opened");
    assert!(harness.is_settings_page_open());
    let settings_tab = harness.active_id();
    assert!(
        !before.contains(&settings_tab),
        "the settings page went into a tab that already existed"
    );
    assert!(
        frame_text(&harness.frame()).contains("Settings"),
        "the strip does not say which tab it is"
    );

    // Somewhere else, then back: the same tab, brought forward.
    harness.dispatch_action(TabAction::Select(before[0]));
    assert!(harness.press_key(",", settings_chord()));
    assert_eq!(settings_tab, harness.active_id());
    assert_eq!(
        with_settings,
        harness.tab_ids(),
        "the binding opened a second settings tab"
    );
}

#[test]
fn the_settings_pane_closes_the_way_every_other_pane_does() {
    // No close button of its own and no Escape binding: the row's ×, a middle
    // click and the close chord (`cmd-w`, `ctrl-shift-w` off macOS) already
    // close a pane, and the settings pane is not special enough to have a
    // fourth way.
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    let settings_tab = harness.active_id();
    let pane = harness
        .focused_pane_id()
        .expect("the settings pane is focused");

    assert_eq!(None, harness.action_for("escape", Modifiers::default()));

    harness.dispatch_action(TabAction::ClosePane(pane));
    assert!(
        !harness.is_settings_page_open(),
        "closing the pane left the page open"
    );
    assert!(
        !harness.tab_ids().contains(&settings_tab),
        "the settings pane was the tab's last one, so the tab should have gone with it"
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
fn the_settings_row_leads_with_a_gear_and_says_nothing_a_session_would() {
    // The row stands for something that is not an agent: no status colour, no
    // working directory, no branch. The last of those is a trap the fact table
    // walks straight into — with "Pane title as: Branch" every arm falls back
    // to the command, and the compact subtitle *is* the command, so a row
    // built through it would read "Settings" over "Settings".
    let mut harness = Harness::new(1);
    harness.dispatch_option(OptionsAction::SetPrimaryInfo(PrimaryInfo::Branch));
    harness.open_settings_page();

    let scene = harness.frame();
    let settings_row = tab_boxes(&scene)
        .into_iter()
        .max_by(|left, right| left.min_x().total_cmp(&right.min_x()))
        .expect("the strip draws the settings tab");
    let text = text_where(&scene, |position| {
        settings_row.contains_point(position + vec2f(0., -4.))
    });

    assert_eq!(
        text.matches("Settings").count(),
        1,
        "the settings row prints its name {} times: {text:?}",
        text.matches("Settings").count()
    );

    // No status dot: every colour a dot can be is an agent state, and the
    // settings pane is not an agent in a fifth one.
    let dots: Vec<RectF> = visible_rects(&scene)
        .filter(|(rect, _)| {
            rect.corner_radius.get_top_left() == Radius::Percentage(50.)
                && (rect.bounds.width() - super::STATUS_DOT_SIZE).abs() < 0.5
        })
        .map(|(_, bounds)| bounds)
        .filter(|bounds| settings_row.contains_point(center(*bounds)))
        .collect();
    assert!(dots.is_empty(), "the settings row drew a status dot");
}

#[test]
fn the_rail_switches_pages_and_the_pane_shows_the_one_it_names() {
    let mut harness = Harness::new(1);
    harness.open_settings_page();

    let rail = settings_rail_boxes(&harness.frame());
    assert_eq!(rail.len(), 4, "four pages in the rail");

    // The third: Keys.
    harness.click(center(rail[2]), MouseButton::Left);
    assert_eq!(Section::Keys, harness.settings_section());

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
fn the_page_and_the_scroll_position_outlive_the_tab_they_were_in() {
    // What Warp's per-window pane manager buys by holding its view handle
    // through a close: coming back to settings comes back to where you were.
    // Here the state is the workspace's rather than the pane's, which is the
    // same guarantee with nothing to keep alive.
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    harness.select_settings_section(Section::About);
    let pane = harness
        .focused_pane_id()
        .expect("the settings pane is focused");

    harness.dispatch_action(TabAction::ClosePane(pane));
    assert!(!harness.is_settings_page_open());

    harness.open_settings_page();
    assert_eq!(
        Section::About,
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
    assert_eq!(
        switches.len(),
        3,
        "PR link, diff stats and the detail card, in that order"
    );

    assert!(harness.options().show_details_on_hover);
    harness.click(center(switches[2]), MouseButton::Left);
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
    harness.click(center(switches[0]), MouseButton::Left);

    assert_eq!(
        before,
        harness.options(),
        "a compact row has no chips, so its chip switch must not be clickable"
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
    harness.select_settings_section(Section::Usage);

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
fn a_settings_pane_can_be_split_beside_a_session() {
    // The reason it is a pane at all: Warp's settings can sit next to the
    // thing being configured. Nothing here special-cases the split — a split
    // opens an agent session beside whatever pane was focused, and the
    // settings pane is a pane.
    let mut harness = Harness::new(1);
    harness.open_settings_page();
    harness.dispatch_action(TabAction::Split(Direction::Right));

    let panes = harness.active_pane_ids();
    assert_eq!(panes.len(), 2, "the settings tab did not split");
    assert!(
        harness.is_settings_page_open(),
        "splitting took the settings page away"
    );
    assert_eq!(
        panel_boxes(&harness.frame()).len(),
        2,
        "the body should draw the settings page and the session side by side"
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

    /// The panel of the one pane, which is the ground it paints that is not a
    /// field.
    fn panel_of_the_pane(harness: &mut Harness) -> RectF {
        let scene = harness.frame();
        let fields = field_boxes(&scene);
        panel_boxes(&scene)
            .into_iter()
            .find(|panel| !fields.iter().any(|field| field.origin() == panel.origin()))
            .expect("a pane draws a panel")
    }

    /// The point in the window where a cell of the one pane's grid is drawn.
    ///
    /// `across` is how far into the cell horizontally, in cells: a tenth is the
    /// left half of it, which is the side a selection starts before, and nine
    /// tenths is the right half, which is the side it ends after.
    fn grid_cell(harness: &mut Harness, row: usize, column: usize, across: f32) -> Vector2F {
        let panel = panel_of_the_pane(harness);
        let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
        panel.origin()
            + vec2f(
                crate::workspace::body::GRID_PADDING,
                crate::workspace::body::GRID_PADDING,
            )
            + vec2f(
                (column as f32 + across) * cell.width,
                (row as f32 + 0.5) * cell.height,
            )
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
        assert_eq!(
            harness.terminal_selection(pane).as_deref(),
            Some("one two three\n"),
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
        let field = field_boxes(&harness.frame())[0];
        harness.click(center(field), MouseButton::Left);
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
        let Some(clipboard) = working_clipboard(&harness) else {
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
    fn working_clipboard(harness: &Harness) -> Option<Clipboard> {
        let clipboard = harness
            .workspace
            .read(&harness.app, |workspace, _| workspace.clipboard().clone());
        clipboard.write("crook: nothing was copied over this");
        (clipboard.read().as_deref() == Some("crook: nothing was copied over this"))
            .then_some(clipboard)
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
            field_boxes(&harness.frame()).len(),
            1,
            "a pane on the normal screen draws a field"
        );

        // The escape every full-screen program starts with.
        harness.type_into(pane, "printf '\\033[?1049h'\n");
        harness.wait_for("the shell never took the alt screen", |harness| {
            harness.alt_screen(pane)
        });

        assert!(
            field_boxes(&harness.frame()).is_empty(),
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
    fn the_settings_pane_is_the_one_pane_that_gets_no_field() {
        // Where the two halves of the body meet. A split settings tab draws
        // both kinds of pane in the same frame: the session keeps the field it
        // composes into, and the settings page — no shell, and every control on
        // it a click — has nothing under it to type in.
        let mut harness = Harness::new(1);
        harness.open_settings_page();
        harness.dispatch_action(TabAction::Split(Direction::Right));
        if !harness.start_terminals() {
            return;
        }

        assert_eq!(
            harness.active_pane_ids().len(),
            2,
            "the settings page and the session should be side by side"
        );
        // The settings page's own controls share the field's corner radius, so
        // the fill is what tells a field from a switch: the field is the well
        // in the pane's own ground, and nothing on the settings page is.
        let scene = harness.frame();
        let fields: Vec<RectF> = visible_rects(&scene)
            .filter(|(rect, _)| {
                rect.corner_radius.get_top_left()
                    == Radius::Pixels(crate::workspace::body::FIELD_RADIUS)
                    && rect.background == Fill::Solid(theme().ground)
            })
            .map(|(_, bounds)| bounds)
            .collect();
        assert_eq!(
            fields.len(),
            1,
            "either the settings pane was given a field or the session lost one"
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
        let field = field_boxes(&scene)[0];
        let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
        // Inside the border and the padding, then past the prompt.
        let text_left =
            field.min_x() + 1. + crate::workspace::body::FIELD_PADDING + cell.width * 2.;
        let middle = field.min_y() + 1. + crate::workspace::body::FIELD_PADDING + cell.height * 0.5;

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
            field_boxes(&harness.frame()).len(),
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
    fn only_the_field_draws_a_caret_while_the_field_has_the_keys() {
        // Two filled block cursors in one pane — the shell's at its prompt and
        // the field's under it — say nothing about which of them is listening,
        // and the steady one is the wrong one.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.type_field(pane, "echo hello");

        let cell = CellFont::headless(CELL_FONT_SIZE).metrics();
        let scene = harness.frame();
        let carets: Vec<_> = visible_rects(&scene)
            .filter(|(rect, bounds)| {
                rect.background == Fill::Solid(theme().accent)
                    && (bounds.width() - cell.width).abs() < 0.5
                    && (bounds.height() - cell.height).abs() < 0.5
            })
            .collect();
        assert_eq!(carets.len(), 1, "one caret, in the field");

        assert!(
            visible_rects(&scene).any(|(rect, bounds)| {
                rect.background == Fill::None
                    && rect.border.width > 0.
                    && (bounds.width() - cell.width).abs() < 0.5
            }),
            "the shell's own cursor is not drawn at all, hollow or otherwise"
        );
    }

    #[test]
    fn a_field_too_tall_for_its_pane_gives_way_instead_of_painting_over_it() {
        // The field grows downwards into the grid. In a short window a
        // six-line command used to be laid out past the panel it sits in —
        // over the border, over the pane below and off the bottom of the
        // window — with the grid above squeezed to no rows at all.
        let mut harness = Harness::panel(1);
        let Some(pane) = one_shell(&mut harness) else {
            return;
        };
        harness.type_field(pane, "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight");

        let scene = harness.frame_sized(vec2f(600., 200.));
        let panel = panel_boxes(&scene)[0];
        let field = field_boxes(&scene)[0];
        let cell = CellFont::headless(CELL_FONT_SIZE).metrics();

        assert!(
            field.max_y() <= panel.max_y() + 0.5,
            "the field reaches {} and the panel ends at {}",
            field.max_y(),
            panel.max_y()
        );
        assert!(
            field.min_y() - panel.min_y() >= cell.height,
            "the grid was squeezed to nothing to make room for the field"
        );
    }
}
