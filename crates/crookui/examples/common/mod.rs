//! The scene both examples draw.
//!
//! One hand-built [`Scene`] exercising everything Crook v1 puts on screen: a
//! dark window ground, a row of tab-shaped rounded rects with text inside, a
//! clipped content panel in a monospace face, and a pill-shaped usage chip with
//! a gradient fill. Nothing here uses the element tree — the point is to test
//! the renderer against a scene whose every coordinate is written down, so a
//! wrong pixel is a renderer bug and never a layout one.

use anyhow::Result;
use crookui::{CosmicFontDb, CosmicTextLayout};
use crookui_core::fonts::{FamilyId, LineStyle, Properties, StyleAndFont, TextStyle, Weight};
use crookui_core::geometry::{Color, RectF, Vector2F, vec2f};
use crookui_core::platform::TextLayoutSystem as _;
use crookui_core::scene::{Border, ClipBounds, CornerRadius, Fill, Radius, Scene};
use crookui_core::text_layout::Line;

/// The logical size the demo is composed for.
pub const SIZE: Vector2F = vec2f(720., 200.);

const GROUND: Color = Color::hex(0x14_16_1a);
const PANEL: Color = Color::hex(0x1a_1d_24);
const PANEL_BORDER: Color = Color::hex(0x25_2a_34);
const TAB_ACTIVE: Color = Color::hex(0x1f_24_30);
const TAB_ACTIVE_BORDER: Color = Color::hex(0x3a_41_52);
const TAB_INACTIVE: Color = Color::hex(0x17_1a_20);
const TAB_INACTIVE_BORDER: Color = Color::hex(0x23_27_2f);
const LABEL_ACTIVE: Color = Color::hex(0xe8_eb_f0);
const LABEL_INACTIVE: Color = Color::hex(0x8a_93_a3);
const BODY: Color = Color::hex(0x9f_a8_b8);
const ACCENT: Color = Color::hex(0x8b_5c_f6);
const CHIP_TEXT: Color = Color::hex(0xd4_c6_ff);

const TAB_TITLES: [&str; 3] = ["agent: refactor", "agent: tests", "zsh"];

/// How one piece of text in the demo is set.
#[derive(Copy, Clone)]
struct Label {
    family: FamilyId,
    font_size: f32,
    weight: Weight,
    color: Color,
}

/// The fonts and families the demo scene needs, resolved once.
pub struct Demo {
    text_layout: CosmicTextLayout,
    ui: FamilyId,
    monospace: FamilyId,
}

impl Demo {
    /// Resolves the two families the scene draws with.
    ///
    /// Both come from the system font database, so this fails only on a machine
    /// with no usable fonts at all.
    pub fn new(font_db: &CosmicFontDb) -> Result<Self> {
        Ok(Self {
            text_layout: font_db.text_layout(),
            ui: font_db.default_ui_family()?,
            monospace: font_db.default_monospace_family()?,
        })
    }

    /// Composes the demo scene for a window of `size` logical pixels.
    ///
    /// Everything is anchored to the left, the top or the right edge, so the
    /// scene stays sensible as a window is dragged wider — which is what makes
    /// it worth repainting on resize in the windowed example.
    pub fn scene(&self, size: Vector2F, scale_factor: f32) -> Scene {
        let mut scene = Scene::new(scale_factor);

        scene
            .draw_rect_without_hit_recording(RectF::new(Vector2F::zero(), size))
            .with_background(GROUND);

        self.tab_row(&mut scene, size);
        self.chip(&mut scene, size);
        self.panel(&mut scene, size);

        scene
    }

    /// Three tabs, the first active: different fills, different borders, and
    /// only the top corners rounded, which is what makes a rect read as a tab.
    fn tab_row(&self, scene: &mut Scene, size: Vector2F) {
        const TOP: f32 = 14.;
        const HEIGHT: f32 = 34.;
        const GAP: f32 = 8.;
        const LEFT: f32 = 16.;

        // The chip and its margin are reserved before the tabs are measured, so
        // widening the window grows the tabs and never overlaps the chip.
        let available = size.x() - LEFT - CHIP_WIDTH - CHIP_MARGIN * 2.;
        let width = ((available - GAP * 2.) / 3.).max(72.);

        for (index, title) in TAB_TITLES.iter().enumerate() {
            let active = index == 0;
            let bounds = RectF::new(
                vec2f(LEFT + index as f32 * (width + GAP), TOP),
                vec2f(width, HEIGHT),
            );

            let (fill, border) = if active {
                (TAB_ACTIVE, TAB_ACTIVE_BORDER)
            } else {
                (TAB_INACTIVE, TAB_INACTIVE_BORDER)
            };

            scene
                .draw_rect_with_hit_recording(bounds)
                .with_background(fill)
                .with_border(Border::all(1.).with_border_color(border))
                .with_corner_radius(CornerRadius::with_top(Radius::Pixels(8.)));

            self.centered_label(
                scene,
                title,
                inset(bounds, 10.),
                Label {
                    family: self.ui,
                    font_size: 13.,
                    weight: if active {
                        Weight::Semibold
                    } else {
                        Weight::Normal
                    },
                    color: if active { LABEL_ACTIVE } else { LABEL_INACTIVE },
                },
            );
        }
    }

    /// The usage chip: a pill with a gradient fill, an accent border and a
    /// percentage in it.
    fn chip(&self, scene: &mut Scene, size: Vector2F) {
        let bounds = RectF::new(
            vec2f(size.x() - CHIP_MARGIN - CHIP_WIDTH, 20.),
            vec2f(CHIP_WIDTH, 22.),
        );

        scene
            .draw_rect_with_hit_recording(bounds)
            .with_background(Fill::Gradient {
                start: vec2f(0., 0.),
                end: vec2f(1., 0.),
                start_color: Color::hex(0x2c_21_40),
                end_color: Color::hex(0x3f_2b_63),
            })
            .with_border(Border::all(1.).with_border_color(ACCENT.with_alpha(160)))
            .with_corner_radius(CornerRadius::with_all(Radius::Percentage(50.)));

        self.centered_label(
            scene,
            "42%",
            bounds,
            Label {
                family: self.ui,
                font_size: 11.,
                weight: Weight::Semibold,
                color: CHIP_TEXT,
            },
        );
    }

    /// The content area, with its text in its own clipped layer so an
    /// overlong line stops at the panel edge instead of running into the
    /// window.
    fn panel(&self, scene: &mut Scene, size: Vector2F) {
        let bounds = RectF::new(vec2f(16., 60.), vec2f(size.x() - 32., size.y() - 76.));

        scene
            .draw_rect_with_hit_recording(bounds)
            .with_background(PANEL)
            .with_border(Border::all(1.).with_border_color(PANEL_BORDER))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)));

        let text = inset(bounds, 14.);
        scene.start_layer(ClipBounds::BoundedBy(text));
        let line = self.line(
            "$ crook run --agent refactor  # 3 tools, 12.4k tokens",
            Label {
                family: self.monospace,
                font_size: 13.,
                weight: Weight::Normal,
                color: BODY,
            },
        );
        line.paint(
            RectF::new(text.origin(), vec2f(text.width(), line.height())),
            BODY,
            scene,
        );
        scene.stop_layer();
    }

    /// Draws `text` centered in `bounds`, both horizontally and vertically.
    fn centered_label(&self, scene: &mut Scene, text: &str, bounds: RectF, label: Label) {
        let line = self.line(text, label);
        let width = line.width.min(bounds.width());
        let origin = bounds.origin()
            + vec2f(
                (bounds.width() - width) / 2.,
                (bounds.height() - line.height()) / 2.,
            );

        line.paint(
            RectF::new(origin, vec2f(width, line.height())),
            label.color,
            scene,
        );
    }

    /// Shapes one line, unwrapped: the demo lays out its own boxes and never
    /// asks the shaper to break anything.
    fn line(&self, text: &str, label: Label) -> Line {
        let style = StyleAndFont::new(
            label.family,
            Properties {
                weight: label.weight,
                ..Default::default()
            },
            TextStyle::default(),
        );

        self.text_layout.layout_line(
            text,
            LineStyle {
                font_size: label.font_size,
                ..Default::default()
            },
            &[(0..text.len(), style)],
            f32::INFINITY,
        )
    }
}

const CHIP_WIDTH: f32 = 62.;
const CHIP_MARGIN: f32 = 16.;

fn inset(bounds: RectF, by: f32) -> RectF {
    RectF::new(
        bounds.origin() + Vector2F::splat(by),
        bounds.size() - Vector2F::splat(by * 2.),
    )
}
