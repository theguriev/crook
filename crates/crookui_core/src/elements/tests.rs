//! Layout, paint and hit-testing, exercised through a real presenter.
//!
//! The harness is the point: an [`App`], a [`Presenter`] and a stub shaper are
//! enough to run a whole frame and inspect the [`Scene`] that comes out. No
//! GPU, no window, no font file.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use super::*;
use crate::core::{
    App, AppContext, Entity, TypedActionView, View, ViewContext, ViewHandle, WindowId,
};
use crate::element::{Element, ParentElement};
use crate::event::{Event, Modifiers, MouseButton, ScrollDelta};
use crate::fonts::{FamilyId, FontId, LineStyle, StyleAndFont};
use crate::geometry::{Color, RectF, Vector2F, vec2f};
use crate::icons::Lucide;
use crate::platform::TextLayoutSystem;
use crate::presenter::Presenter;
use crate::scene::{Border, Rect, Scene};
use crate::text_layout::{Glyph, Line, Run};

/// A shaper with no fonts: every character is a square half its font size.
///
/// Real metrics come from a real backend; what the element layer needs from a
/// shaper is that widths add up, which this delivers exactly.
struct StubShaper;

const STUB_ADVANCE_RATIO: f32 = 0.5;

impl TextLayoutSystem for StubShaper {
    fn layout_line(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        _: f32,
    ) -> Line {
        let advance = line_style.font_size * STUB_ADVANCE_RATIO;
        let styles = style_runs
            .first()
            .map(|(_, style_and_font)| style_and_font.style)
            .unwrap_or_default();

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
                styles,
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

/// What a click did, so a test can assert on it.
#[derive(Debug, PartialEq, Eq)]
enum TestAction {
    Clicked,
    Closed,
    Menued,
}

/// How a [`TestView`] renders itself.
type BuildElement = Box<dyn Fn(&TestView) -> Box<dyn Element>>;

/// A view that renders whatever the test told it to.
struct TestView {
    build: BuildElement,
    mouse: MouseStateHandle,
    /// The scroll offset a [`Scrollable`] test hands back each render, kept
    /// here for the reason every other handle is: an element tree does not
    /// survive a repaint, and scrolling causes one.
    scroll: ScrollStateHandle,
    actions: Vec<TestAction>,
}

impl TestView {
    fn new(build: impl 'static + Fn(&TestView) -> Box<dyn Element>) -> Self {
        Self {
            build: Box::new(build),
            mouse: MouseStateHandle::default(),
            scroll: ScrollStateHandle::default(),
            actions: Vec::new(),
        }
    }
}

impl Entity for TestView {
    type Event = ();
}

impl View for TestView {
    fn ui_name() -> &'static str {
        "TestView"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        (self.build)(self)
    }
}

impl TypedActionView for TestView {
    type Action = TestAction;

    fn handle_action(&mut self, action: &TestAction, _: &mut ViewContext<Self>) {
        self.actions.push(match action {
            TestAction::Clicked => TestAction::Clicked,
            TestAction::Closed => TestAction::Closed,
            TestAction::Menued => TestAction::Menued,
        });
    }
}

/// An app with one window whose root view renders `build`.
struct Harness {
    app: App,
    presenter: Presenter,
    window_id: WindowId,
    root: ViewHandle<TestView>,
}

impl Harness {
    fn new(build: impl 'static + Fn(&TestView) -> Box<dyn Element>) -> Self {
        let mut app = App::new(
            crate::executor::LocalQueue::new().foreground(),
            Arc::new(crate::executor::Background::new(1)),
        );
        let (window_id, root) = app.add_window(|_| TestView::new(build));
        let presenter = Presenter::new(window_id, Arc::new(StubShaper));

        Self {
            app,
            presenter,
            window_id,
            root,
        }
    }

    /// Renders the dirty views and builds a frame at `size`.
    fn build_scene(&mut self, size: Vector2F) -> Rc<Scene> {
        let window_id = self.window_id;
        let presenter = &mut self.presenter;
        self.app.update(|ctx| {
            let invalidation = ctx.take_all_invalidations_for_window(window_id);
            presenter.invalidate(invalidation, ctx);
            presenter.build_scene(size, 1., ctx)
        })
    }

    fn dispatch(&mut self, event: Event) {
        let window_id = self.window_id;
        let presenter = &mut self.presenter;
        self.app
            .update(|ctx| ctx.dispatch_window_event(window_id, event, presenter));
    }

    fn press(&mut self, position: Vector2F, button: MouseButton) {
        self.dispatch(Event::MouseDown {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
    }

    fn release(&mut self, position: Vector2F, button: MouseButton) {
        self.dispatch(Event::MouseUp {
            button,
            position,
            modifiers: Modifiers::default(),
        });
    }

    /// Turns the wheel `lines` clicks, positive being away from the user.
    fn scroll(&mut self, position: Vector2F, lines: f32) {
        self.dispatch(Event::ScrollWheel {
            position,
            delta: ScrollDelta::Lines(vec2f(0., lines)),
            modifiers: Modifiers::default(),
        });
    }

    /// The scroll state the root view is holding.
    fn scroll_state(&self) -> ScrollStateHandle {
        self.root.read(&self.app, |view, _| view.scroll.clone())
    }
}

/// Every rect in the frame, bottom layer first.
fn rects(scene: &Scene) -> Vec<Rect> {
    scene
        .layers()
        .flat_map(|layer| layer.rects.iter())
        .cloned()
        .collect()
}

/// A fixed-size box that paints one rect, for reading positions back out.
fn marker(width: f32, height: f32) -> Box<dyn Element> {
    ConstrainedBox::new(
        Container::new(Empty::new().finish())
            .with_background_color(Color::WHITE)
            .finish(),
    )
    .with_width(width)
    .with_height(height)
    .finish()
}

#[test]
fn a_row_places_children_left_to_right_with_spacing() {
    let mut harness = Harness::new(|_| {
        Flex::row()
            .with_spacing(4.)
            .with_child(marker(20., 10.))
            .with_child(marker(30., 10.))
            .finish()
    });

    let scene = harness.build_scene(vec2f(100., 50.));
    let bounds: Vec<_> = rects(&scene).iter().map(|rect| rect.bounds).collect();

    assert_eq!(
        bounds,
        [
            RectF::new(vec2f(0., 0.), vec2f(20., 10.)),
            RectF::new(vec2f(24., 0.), vec2f(30., 10.)),
        ]
    );
}

#[test]
fn an_expanded_spacer_pushes_the_next_child_to_the_far_edge() {
    let mut harness = Harness::new(|_| {
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(marker(20., 10.))
            .with_child(Expanded::new(1., Empty::new().finish()).finish())
            .with_child(marker(30., 10.))
            .finish()
    });

    let scene = harness.build_scene(vec2f(100., 50.));
    let bounds: Vec<_> = rects(&scene).iter().map(|rect| rect.bounds).collect();

    assert_eq!(bounds[0].origin(), Vector2F::zero());
    assert_eq!(
        bounds[1],
        RectF::new(vec2f(70., 0.), vec2f(30., 10.)),
        "the last child should end flush with the right edge"
    );
}

#[test]
fn a_column_asked_not_to_overflow_lays_no_child_out_past_its_end() {
    // A child that is not flexible is measured free along the main axis, so
    // one that asks for more than the column has is given it, laid out past
    // the bottom edge and painted over whatever comes after — including the
    // window's own end. That is the default, and a list with a clip below it
    // depends on it; a column that *is* the space says so instead.
    let mut harness = Harness::new(|_| {
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_no_overflow()
            .with_child(marker(20., 400.))
            .with_child(marker(30., 400.))
            .finish()
    });

    let scene = harness.build_scene(vec2f(100., 50.));
    let bounds: Vec<_> = rects(&scene).iter().map(|rect| rect.bounds).collect();

    assert_eq!(
        bounds[0],
        RectF::new(Vector2F::zero(), vec2f(20., 50.)),
        "the first child took the column and no more"
    );
    assert_eq!(
        bounds[1],
        RectF::new(vec2f(0., 50.), vec2f(30., 0.)),
        "and the second was measured against what was left of it, which was \
         nothing"
    );
}

#[test]
fn a_container_grows_by_its_padding_and_border() {
    let mut harness = Harness::new(|_| {
        Container::new(marker(10., 10.))
            .with_uniform_padding(5.)
            .with_border(Border::all(2.).with_border_color(Color::WHITE))
            .with_uniform_margin(1.)
            .finish()
    });

    let scene = harness.build_scene(vec2f(100., 50.));
    let bounds: Vec<_> = rects(&scene).iter().map(|rect| rect.bounds).collect();

    assert_eq!(
        bounds[0],
        RectF::new(vec2f(1., 1.), vec2f(24., 24.)),
        "the box is inset by the margin and grown by padding and border"
    );
    assert_eq!(
        bounds[1].origin(),
        vec2f(8., 8.),
        "the child sits inside the margin, border and padding"
    );
}

#[test]
fn align_centers_a_child_in_the_space_it_was_given() {
    let mut harness = Harness::new(|_| Align::new(marker(10., 10.)).finish());

    let scene = harness.build_scene(vec2f(100., 50.));
    assert_eq!(rects(&scene)[0].bounds.origin(), vec2f(45., 20.));
}

#[test]
fn text_measures_to_its_advances_and_paints_one_glyph_per_character() {
    let family = FamilyId::new();
    let mut harness = Harness::new(move |_| {
        Flex::row()
            .with_child(Text::new("abc", family, 10.).finish())
            .finish()
    });

    let scene = harness.build_scene(vec2f(100., 50.));
    let glyphs: Vec<_> = scene
        .layers()
        .flat_map(|layer| layer.glyphs.iter())
        .collect();

    assert_eq!(glyphs.len(), 3);
    assert_eq!(glyphs[0].position.x(), 0.);
    assert_eq!(glyphs[1].position.x(), 5.);
    // Line box 12, text box 10, so 1 of padding above an ascent of 8.
    assert_eq!(glyphs[0].position.y(), 9.);
}

#[test]
fn text_wider_than_its_box_is_cut_off_rather_than_wrapped() {
    let family = FamilyId::new();
    let mut harness = Harness::new(move |_| {
        ConstrainedBox::new(Text::new("abcdefgh", family, 10.).finish())
            .with_width(12.)
            .finish()
    });

    let scene = harness.build_scene(vec2f(100., 50.));
    let glyph_count = scene.layers().flat_map(|layer| layer.glyphs.iter()).count();

    assert_eq!(glyph_count, 2, "only whole glyphs inside the box are drawn");
}

#[test]
fn an_icon_fills_the_square_it_asked_for() {
    let mut harness = Harness::new(|_| {
        Flex::row()
            .with_child(Icon::new(Lucide::X, 16.).with_color(Color::WHITE).finish())
            .finish()
    });

    let scene = harness.build_scene(vec2f(100., 50.));
    let icons: Vec<_> = scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .collect();

    assert_eq!(icons.len(), 1);
    assert_eq!(
        icons[0].bounds,
        RectF::new(Vector2F::zero(), vec2f(16., 16.))
    );
    assert_eq!(icons[0].icon_key.icon, Lucide::X);
    assert_eq!(icons[0].icon_key.size, 16.);
}

#[test]
fn an_icon_squeezed_by_its_parent_is_centred_rather_than_stretched() {
    // The rasterizer draws a square, so a squeezed icon has to become a
    // smaller square: stretching it would ask for a mask that no longer
    // matches Lucide's proportions, and cropping it would cut the stroke.
    let mut harness = Harness::new(|_| {
        ConstrainedBox::new(Icon::new(Lucide::Settings, 24.).finish())
            .with_width(10.)
            .with_height(24.)
            .finish()
    });

    let scene = harness.build_scene(vec2f(100., 50.));
    let icon = scene
        .layers()
        .flat_map(|layer| layer.icons.iter())
        .next()
        .expect("the icon should have painted");

    assert_eq!(
        icon.bounds,
        RectF::new(vec2f(0., 7.), vec2f(10., 10.)),
        "a 10 x 24 box holds a 10 x 10 icon, centred"
    );
    assert_eq!(
        icon.icon_key.size, 10.,
        "and the mask is rasterized at the size it is drawn at"
    );
}

#[test]
fn a_press_and_release_inside_a_hoverable_dispatches_its_action() {
    let mut harness = Harness::new(|view| {
        Hoverable::new(view.mouse.clone(), |_| marker(50., 20.))
            .on_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Clicked))
            .on_middle_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Closed))
            .finish()
    });
    harness.build_scene(vec2f(100., 50.));

    harness.press(vec2f(10., 10.), MouseButton::Left);
    harness.release(vec2f(10., 10.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(view.actions, [TestAction::Clicked])
    });

    // A release outside the element is not a click on it.
    harness.press(vec2f(10., 10.), MouseButton::Left);
    harness.release(vec2f(80., 10.), MouseButton::Left);
    harness
        .root
        .read(&harness.app, |view, _| assert_eq!(view.actions.len(), 1));

    harness.press(vec2f(10., 10.), MouseButton::Middle);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(view.actions, [TestAction::Clicked, TestAction::Closed]);
    });
}

#[test]
fn a_right_press_inside_a_hoverable_dispatches_its_own_action() {
    // On the press, like the middle button and like every context menu: a
    // handler that waited for the release would open the menu under a pointer
    // that has already been let go.
    let mut harness = Harness::new(|view| {
        Hoverable::new(view.mouse.clone(), |_| marker(50., 20.))
            .on_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Clicked))
            .on_right_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Menued))
            .finish()
    });
    harness.build_scene(vec2f(100., 50.));

    harness.press(vec2f(10., 10.), MouseButton::Right);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(view.actions, [TestAction::Menued]);
    });

    // A right press outside is not on this element, and the left button goes
    // on meaning what it meant.
    harness.press(vec2f(80., 10.), MouseButton::Right);
    harness.press(vec2f(10., 10.), MouseButton::Left);
    harness.release(vec2f(10., 10.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(view.actions, [TestAction::Menued, TestAction::Clicked]);
    });
}

#[test]
fn hovering_a_hoverable_updates_the_state_its_view_owns() {
    let mut harness =
        Harness::new(|view| Hoverable::new(view.mouse.clone(), |_| marker(50., 20.)).finish());
    harness.build_scene(vec2f(100., 50.));

    let mouse = harness
        .root
        .read(&harness.app, |view, _| view.mouse.clone());
    assert!(!mouse.lock().is_hovered());

    harness.dispatch(Event::MouseMoved {
        position: vec2f(10., 10.),
        modifiers: Modifiers::default(),
        is_synthetic: false,
    });
    assert!(mouse.lock().is_hovered());

    harness.dispatch(Event::MouseMoved {
        position: vec2f(90., 10.),
        modifiers: Modifiers::default(),
        is_synthetic: false,
    });
    assert!(!mouse.lock().is_hovered());
}

#[test]
fn laying_out_a_child_view_is_what_builds_the_responder_chain() {
    let mut app = App::new(
        crate::executor::LocalQueue::new().foreground(),
        Arc::new(crate::executor::Background::new(1)),
    );
    let (window_id, root) = app.add_window(|_| TestView::new(|_| Empty::new().finish()));

    // Deliberately parentless: only layout can discover where it belongs.
    let child = app.add_view(window_id, |_| TestView::new(|_| marker(10., 10.)));
    let child_id = child.id();
    root.update(&mut app, |view, ctx| {
        view.build = Box::new(move |_| ChildView::<TestView>::with_id(child_id).finish());
        ctx.notify();
    });

    app.read(|ctx| {
        assert_eq!(
            ctx.view_ancestors(window_id, child.id()),
            [child.id()],
            "before layout the child has no ancestors"
        );
    });

    let mut presenter = Presenter::new(window_id, Arc::new(StubShaper));
    app.update(|ctx| {
        let invalidation = ctx.take_all_invalidations_for_window(window_id);
        presenter.invalidate(invalidation, ctx);
        presenter.build_scene(vec2f(100., 50.), 1., ctx);
    });

    app.read(|ctx| {
        assert_eq!(
            ctx.view_ancestors(window_id, child.id()),
            [root.id(), child.id()]
        );
    });
}

#[test]
fn a_clip_confines_what_its_child_paints_and_what_that_child_can_be_clicked_on() {
    // Two 30-wide markers in a 40-wide box: the second one overflows, and the
    // half of it that is outside must be neither drawn nor clickable. Hit
    // testing resolves against painted geometry, so without the clip that
    // overflow would be a live control sitting on top of its neighbour.
    let mut harness = Harness::new(|view| {
        ConstrainedBox::new(
            Clipped::new(
                Flex::row()
                    .with_child(marker(30., 20.))
                    .with_child(
                        Hoverable::new(view.mouse.clone(), |_| marker(30., 20.))
                            .on_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Clicked))
                            .finish(),
                    )
                    .finish(),
            )
            .finish(),
        )
        .with_width(40.)
        .with_height(20.)
        .finish()
    });

    let scene = harness.build_scene(vec2f(100., 50.));
    assert_eq!(scene.layer_count(), 2, "the clip is a layer of its own");
    assert_eq!(
        scene.visible_rect(
            crate::geometry::Point::from_vec2f(vec2f(30., 0.), crate::geometry::ZIndex::Normal(1)),
            vec2f(30., 20.)
        ),
        Some(RectF::new(vec2f(30., 0.), vec2f(10., 20.))),
        "the overflowing half of the second marker is clipped away"
    );

    harness.press(vec2f(50., 10.), MouseButton::Left);
    harness.release(vec2f(50., 10.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(
            view.actions,
            [],
            "a click outside the clip reached the child"
        );
    });

    harness.press(vec2f(35., 10.), MouseButton::Left);
    harness.release(vec2f(35., 10.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(view.actions, [TestAction::Clicked]);
    });
}

/// Hangs an overlay 10 in from the stack's upper-left, so it overlaps
/// whatever the stack painted there.
const OVER_THE_STACK: AnchorTo = AnchorTo {
    parent: Corner::TopLeft,
    child: Corner::TopLeft,
    offset: vec2f(10., 10.),
    keep_on_screen: false,
    keep_clear_of_parent: false,
};

#[test]
fn an_overlay_child_paints_last_however_early_it_was_added() {
    // The overlay goes in *first*, so everything about the frame that follows
    // is the layer jump rather than paint order: a menu is emitted wherever
    // its trigger lives, which is usually before the content it must cover.
    let mut harness = Harness::new(move |view| {
        Stack::new()
            .with_anchored_overlay_child(marker(50., 20.), OVER_THE_STACK)
            .with_child(
                Hoverable::new(view.mouse.clone(), |_| marker(100., 50.))
                    .on_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Clicked))
                    .finish(),
            )
            .finish()
    });

    let scene = harness.build_scene(vec2f(200., 100.));
    let bounds: Vec<_> = rects(&scene).iter().map(|rect| rect.bounds).collect();

    assert_eq!(
        bounds,
        [
            RectF::new(vec2f(0., 0.), vec2f(100., 50.)),
            RectF::new(vec2f(10., 10.), vec2f(50., 20.)),
        ],
        "the overlay is drawn after the child added after it"
    );

    harness.press(vec2f(20., 20.), MouseButton::Left);
    harness.release(vec2f(20., 20.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(
            view.actions,
            [],
            "a click on the menu must not reach the button underneath it"
        );
    });

    harness.press(vec2f(80., 40.), MouseButton::Left);
    harness.release(vec2f(80., 40.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(view.actions, [TestAction::Clicked], "the rest still works");
    });
}

#[test]
fn an_anchored_child_is_laid_out_against_the_window_not_against_the_stack() {
    // A 40 by 28 toolbar button with a menu hung under it. Handing the menu
    // the stack's own constraint would lay it out 28 tall and squash every row
    // of it into nothing.
    let mut harness = Harness::new(|_| {
        ConstrainedBox::new(
            Stack::new()
                .with_anchored_overlay_child(
                    ConstrainedBox::new(
                        Container::new(Empty::new().finish())
                            .with_background_color(Color::WHITE)
                            .finish(),
                    )
                    .with_width(50.)
                    .with_height(80.)
                    .finish(),
                    AnchorTo::below(vec2f(0., 4.)),
                )
                .finish(),
        )
        .with_width(40.)
        .with_height(28.)
        .finish()
    });

    let scene = harness.build_scene(vec2f(200., 200.));

    assert_eq!(
        rects(&scene)[0].bounds,
        RectF::new(vec2f(0., 32.), vec2f(50., 80.)),
        "80 tall, and hung 4 below the button's bottom-left corner"
    );
}

#[test]
fn a_press_outside_a_dismiss_closes_it_and_one_on_it_does_not() {
    let mut harness = Harness::new(move |view| {
        Stack::new()
            .with_child(
                Hoverable::new(view.mouse.clone(), |_| marker(200., 100.))
                    .on_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Clicked))
                    .finish(),
            )
            .with_anchored_overlay_child(
                Dismiss::new(marker(50., 20.))
                    .on_dismiss(|ctx, _| ctx.dispatch_typed_action(TestAction::Closed))
                    .finish(),
                OVER_THE_STACK,
            )
            .finish()
    });
    harness.build_scene(vec2f(200., 100.));

    harness.press(vec2f(20., 20.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(
            view.actions,
            [],
            "a press on the menu is not a press outside"
        );
    });

    // Not modal, so the click that dismisses is also a click on the window.
    harness.press(vec2f(150., 80.), MouseButton::Left);
    harness.release(vec2f(150., 80.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(view.actions, [TestAction::Closed, TestAction::Clicked]);
    });
}

#[test]
fn a_modal_dismiss_keeps_the_click_that_closed_it_from_reaching_the_frame() {
    let mut harness = Harness::new(move |view| {
        Stack::new()
            .with_child(
                Hoverable::new(view.mouse.clone(), |_| marker(200., 100.))
                    .on_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Clicked))
                    .finish(),
            )
            .with_anchored_overlay_child(
                Dismiss::new(marker(50., 20.))
                    .modal()
                    .on_dismiss(|ctx, _| ctx.dispatch_typed_action(TestAction::Closed))
                    .finish(),
                OVER_THE_STACK,
            )
            .finish()
    });
    harness.build_scene(vec2f(200., 100.));

    harness.press(vec2f(150., 80.), MouseButton::Left);
    harness.release(vec2f(150., 80.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(
            view.actions,
            [TestAction::Closed],
            "the button under the modal saw neither the press nor the release"
        );
    });
}

#[test]
fn an_overlay_that_records_no_hit_rect_lets_clicks_through_to_what_it_covers() {
    // The trap this whole mechanism has: occlusion is decided by hit rects,
    // and only a Container draws one. A menu built out of bare labels floats
    // above the frame and is invisible to every click that lands on it.
    let family = FamilyId::new();
    let mut harness = Harness::new(move |view| {
        Stack::new()
            .with_child(
                Hoverable::new(view.mouse.clone(), |_| marker(100., 50.))
                    .on_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Clicked))
                    .finish(),
            )
            .with_anchored_overlay_child(Text::new("Density", family, 10.).finish(), OVER_THE_STACK)
            .finish()
    });
    harness.build_scene(vec2f(200., 100.));

    harness.press(vec2f(20., 15.), MouseButton::Left);
    harness.release(vec2f(20., 15.), MouseButton::Left);
    harness.root.read(&harness.app, |view, _| {
        assert_eq!(view.actions, [TestAction::Clicked]);
    });
}

#[test]
fn a_scrollable_keeps_the_box_it_was_given_however_tall_its_child_is() {
    let mut harness =
        Harness::new(|view| Scrollable::new(view.scroll.clone(), marker(80., 400.)).finish());

    let scene = harness.build_scene(vec2f(100., 100.));

    let state = *harness.scroll_state().lock();
    assert_eq!(
        state.max_offset(),
        300.,
        "the child asked for 400 in a 100px box, so 300 of it is off screen"
    );
    assert!(state.is_scrollable());
    assert_eq!(
        rects(&scene)[0].bounds,
        RectF::new(Vector2F::zero(), vec2f(80., 400.)),
        "the child is painted at its own height and clipped, not squashed into the box"
    );
}

#[test]
fn the_wheel_moves_the_content_and_stops_at_the_end() {
    let mut harness =
        Harness::new(|view| Scrollable::new(view.scroll.clone(), marker(80., 400.)).finish());
    harness.build_scene(vec2f(100., 100.));

    // Positive-up at the wheel, so one click away from the user moves the
    // content up by one line.
    harness.scroll(vec2f(50., 50.), -2.);
    let scene = harness.build_scene(vec2f(100., 100.));

    assert_eq!(harness.scroll_state().lock().offset(), 40.);
    assert_eq!(
        rects(&scene)[0].bounds.origin(),
        vec2f(0., -40.),
        "the child paints above the box by exactly the offset"
    );

    harness.scroll(vec2f(50., 50.), -100.);
    harness.build_scene(vec2f(100., 100.));
    assert_eq!(
        harness.scroll_state().lock().offset(),
        300.,
        "a wheel past the end lands on the last pixel of content, not beyond it"
    );
}

#[test]
fn a_wheel_over_content_that_fits_is_left_for_something_else_to_handle() {
    let mut harness =
        Harness::new(|view| Scrollable::new(view.scroll.clone(), marker(80., 40.)).finish());
    harness.build_scene(vec2f(100., 100.));

    harness.scroll(vec2f(50., 50.), -3.);

    let state = *harness.scroll_state().lock();
    assert!(!state.is_scrollable());
    assert_eq!(state.offset(), 0.);
}

#[test]
fn a_window_that_grew_taller_clamps_the_offset_back_into_its_content() {
    let mut harness =
        Harness::new(|view| Scrollable::new(view.scroll.clone(), marker(80., 400.)).finish());
    harness.build_scene(vec2f(100., 100.));

    harness.scroll(vec2f(50., 50.), -20.);
    harness.build_scene(vec2f(100., 100.));
    assert_eq!(harness.scroll_state().lock().offset(), 300.);

    // The same content in a box three times taller has only 40px to hide, and
    // an offset of 300 would leave the list showing nothing at all.
    harness.build_scene(vec2f(100., 360.));
    assert_eq!(harness.scroll_state().lock().offset(), 40.);
}

#[test]
fn the_thumb_is_painted_only_while_there_is_something_to_scroll() {
    let mut harness = Harness::new(|view| {
        Scrollable::new(view.scroll.clone(), marker(80., 400.))
            .with_scrollbar(Color::WHITE)
            .finish()
    });

    let scene = harness.build_scene(vec2f(100., 100.));
    let thumb = rects(&scene)[1].bounds;
    assert_eq!(
        thumb.origin(),
        vec2f(74., 2.),
        "the thumb rides the right edge of the box the scrollable settled on — \
         the child's 80px, not the window's 100 — inset, and starts at the top"
    );
    assert_eq!(
        thumb.size(),
        vec2f(4., 24.),
        "a quarter of the content is on screen, so the thumb is at its floor"
    );

    let mut harness = Harness::new(|view| {
        Scrollable::new(view.scroll.clone(), marker(80., 40.))
            .with_scrollbar(Color::WHITE)
            .finish()
    });
    let scene = harness.build_scene(vec2f(100., 100.));
    assert_eq!(
        rects(&scene).len(),
        1,
        "a list that fits paints its child and no thumb"
    );
}

#[test]
fn a_control_scrolled_out_of_the_box_is_neither_drawn_nor_clickable() {
    let mut harness = Harness::new(|view| {
        Scrollable::new(
            view.scroll.clone(),
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_child(marker(80., 100.))
                .with_child(
                    Hoverable::new(view.mouse.clone(), |_| marker(80., 40.))
                        .on_click(|_, ctx, _| ctx.dispatch_typed_action(TestAction::Clicked))
                        .finish(),
                )
                .finish(),
        )
        .finish()
    });
    harness.build_scene(vec2f(100., 100.));

    // The button is at y 100..140, entirely below a 100px box.
    harness.press(vec2f(40., 120.), MouseButton::Left);
    harness.release(vec2f(40., 120.), MouseButton::Left);
    assert!(
        harness
            .root
            .read(&harness.app, |view, _| view.actions.is_empty()),
        "a click below the clip must not reach the button hanging out of it"
    );

    // Scrolled up by 60, the button occupies y 40..80 and is over the box.
    harness.scroll(vec2f(50., 50.), -3.);
    harness.build_scene(vec2f(100., 100.));
    harness.press(vec2f(40., 60.), MouseButton::Left);
    harness.release(vec2f(40., 60.), MouseButton::Left);
    assert_eq!(
        harness
            .root
            .read(&harness.app, |view, _| view.actions.len()),
        1,
        "the same button, scrolled into view, takes the click"
    );
}
