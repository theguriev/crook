//! The header, driven through a real presenter.
//!
//! These run the actual view tree — `Workspace::render`, layout, paint, hit
//! testing — against a stub shaper, so they cover the wiring a unit test of
//! the strip cannot: that a click lands on the tab under it, that the close
//! button closes its own tab, and that revealing that button does not move
//! the bar out from under the cursor.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use crookui_core::event::{Event, Modifiers, MouseButton};
use crookui_core::executor::{Background, LocalQueue};
use crookui_core::fonts::{FamilyId, FontId, LineStyle, StyleAndFont};
use crookui_core::geometry::{RectF, Vector2F, vec2f};
use crookui_core::platform::TextLayoutSystem;
use crookui_core::prelude::*;
use crookui_core::scene::{Radius, Rect, Scene};
use crookui_core::text_layout::{Glyph, Line, Run};
use crookui_core::{AddSingletonModel as _, App, Presenter, WindowId};

use crate::tab::{AgentSession, AgentStatus, Tab, TabAction, TabId};
use crate::usage_model::UsageModel;

use super::{Fonts, QuitRequest, Workspace};

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
    app: App,
    presenter: Presenter,
    window_id: WindowId,
    workspace: ViewHandle<Workspace>,
    quit_requests: Rc<Cell<usize>>,
}

impl Harness {
    /// A window with `tabs` tabs, the last of which is active, and one frame
    /// already drawn so there is something to hit-test against.
    fn new(tabs: usize) -> Self {
        let mut app = App::new(LocalQueue::new().foreground(), Arc::new(Background::new(1)));
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
        let (window_id, workspace) = app.add_window(|ctx| Workspace::new(fonts, quit, ctx));

        let mut harness = Self {
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
        let window_id = self.window_id;
        let presenter = &mut self.presenter;
        self.app.update(|ctx| {
            let invalidation = ctx.take_all_invalidations_for_window(window_id);
            presenter.invalidate(invalidation, ctx);
            presenter.build_scene(WINDOW, 1., ctx)
        })
    }

    fn dispatch_action(&mut self, action: TabAction) {
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
    fn update_session(&mut self, id: TabId, report: impl FnOnce(&mut AgentSession)) {
        let workspace = &self.workspace;
        self.app.update(|ctx| {
            workspace.update(ctx, |workspace, ctx| {
                assert!(workspace.update_session(id, ctx, report), "no such tab");
            });
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
}

/// The tabs, by their rounded-top boxes, in bar order.
fn tab_boxes(scene: &Scene) -> Vec<RectF> {
    rects_rounded_by(scene, Radius::Pixels(8.))
}

/// The close buttons that are currently drawn, by their rounded boxes.
fn close_boxes(scene: &Scene) -> Vec<RectF> {
    rects_rounded_by(scene, Radius::Pixels(4.))
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
    let id = harness.tab_ids()[0];
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
