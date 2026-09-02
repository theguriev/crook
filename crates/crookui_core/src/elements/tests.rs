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
use crate::event::{Event, Modifiers, MouseButton};
use crate::fonts::{FamilyId, FontId, LineStyle, StyleAndFont};
use crate::geometry::{Color, RectF, Vector2F, vec2f};
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
}

/// How a [`TestView`] renders itself.
type BuildElement = Box<dyn Fn(&TestView) -> Box<dyn Element>>;

/// A view that renders whatever the test told it to.
struct TestView {
    build: BuildElement,
    mouse: MouseStateHandle,
    actions: Vec<TestAction>,
}

impl TestView {
    fn new(build: impl 'static + Fn(&TestView) -> Box<dyn Element>) -> Self {
        Self {
            build: Box::new(build),
            mouse: MouseStateHandle::default(),
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
            crate::geometry::Point::from_vec2f(vec2f(30., 0.), crate::geometry::ZIndex(1)),
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
