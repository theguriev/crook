//! The output of shaping a string, and how it reaches the [`Scene`].
//!
//! Shaping itself needs a font backend, so it happens in the platform crate
//! behind [`crate::platform::TextLayoutSystem`]. What lives here is the half
//! that does not: the positioned-glyph data a shaper produces, and the walk
//! that turns it into draw commands.
//!
//! The contract between the two halves is that a [`Line`] is self-contained —
//! it carries its own ascent, descent and font size, so painting it needs no
//! font metrics and no cache lookup.

use crate::fonts::{FontId, GlyphId, TextStyle};
use crate::geometry::{Color, RectF, Vector2F, vec2f};
use crate::scene::Scene;

/// One positioned glyph within a [`Run`].
#[derive(Clone, Debug)]
pub struct Glyph {
    /// The glyph index in the run's face.
    pub id: GlyphId,

    /// Where the glyph's left edge meets the baseline, relative to the line's
    /// origin.
    pub position_along_baseline: Vector2F,

    /// Byte offset into the source string where the character this glyph came
    /// from starts. Neither injective nor monotonic: ligatures collapse
    /// several characters into one glyph, and bidi reorders them.
    pub index: usize,

    /// The glyph's advance in logical pixels.
    pub width: f32,
}

/// Consecutive glyphs that share one face and one style.
///
/// A run breaks whenever either changes — which is why the shaper, not the
/// caller, decides where runs begin: font fallback can split a single styled
/// range across several faces.
#[derive(Clone, Debug)]
pub struct Run {
    /// The face every glyph in this run came from.
    pub font_id: FontId,
    /// The glyphs, in visual order.
    pub glyphs: Vec<Glyph>,
    /// How the run should be painted.
    pub styles: TextStyle,
    /// The sum of the run's advances.
    pub width: f32,
}

/// A single shaped line of text.
#[derive(Clone, Debug)]
pub struct Line {
    /// The total advance width in logical pixels.
    pub width: f32,
    /// The runs making up the line, in visual order.
    pub runs: Vec<Run>,
    /// The em size the line was shaped at.
    pub font_size: f32,
    /// Line height as a multiple of `font_size`.
    pub line_height_ratio: f32,
    /// Where the baseline sits within the leftover line height.
    pub baseline_ratio: f32,
    /// Maximum rise above the baseline, in logical pixels.
    pub ascent: f32,
    /// Maximum drop below the baseline, in logical pixels, as a positive
    /// number — the opposite sign convention from [`crate::fonts::Metrics`].
    pub descent: f32,
}

impl Default for Line {
    fn default() -> Self {
        Self {
            width: 0.,
            runs: Vec::new(),
            font_size: 0.,
            line_height_ratio: crate::fonts::DEFAULT_UI_LINE_HEIGHT_RATIO,
            baseline_ratio: crate::fonts::DEFAULT_TOP_BOTTOM_RATIO,
            ascent: 0.,
            descent: 0.,
        }
    }
}

/// Where the baseline sits inside a line box, measured from its top.
///
/// The text box (ascent plus descent) is centered in the line box, and the
/// baseline is one ascent below the top of that.
pub fn default_compute_baseline_position(
    font_size: f32,
    line_height_ratio: f32,
    ascent: f32,
    descent: f32,
) -> f32 {
    let line_height = font_size * line_height_ratio;
    let text_height = ascent + descent;
    let padding_top = (line_height - text_height) / 2.;
    padding_top + ascent
}

/// How thick an underline is drawn, as a fraction of the font size.
const UNDERLINE_THICKNESS_RATIO: f32 = 0.06;

/// How far below the baseline an underline is drawn, as a fraction of the
/// descent.
const UNDERLINE_OFFSET_RATIO: f32 = 0.4;

impl Line {
    /// The height of the line box in logical pixels.
    pub fn height(&self) -> f32 {
        self.font_size * self.line_height_ratio
    }

    /// Where the baseline sits, measured down from the top of the line box.
    pub fn baseline_position(&self) -> f32 {
        default_compute_baseline_position(
            self.font_size,
            self.line_height_ratio,
            self.ascent,
            self.descent,
        )
    }

    /// The x offset of the glyph at `index` characters into the line, or the
    /// line's width when `index` is past the end.
    pub fn x_for_index(&self, index: usize) -> f32 {
        self.runs
            .iter()
            .flat_map(|run| run.glyphs.iter())
            .find(|glyph| glyph.index >= index)
            .map(|glyph| glyph.position_along_baseline.x())
            .unwrap_or(self.width)
    }

    /// Pushes this line's glyphs into `scene`, laid out inside `bounds`.
    ///
    /// `bounds` is the *line box*: its origin is the top-left of the line, not
    /// the baseline, and its width is how much room the glyphs have. Glyphs
    /// that would overflow are dropped rather than drawn outside — a tab title
    /// too long for its tab stops at the edge.
    ///
    /// `default_color` paints every run that does not name its own foreground.
    pub fn paint(&self, bounds: RectF, default_color: Color, scene: &mut Scene) {
        let line_origin = bounds.origin() + vec2f(0., self.baseline_position());
        let available_width = bounds.width();

        for run in &self.runs {
            let color = run.styles.foreground_color.unwrap_or(default_color);

            let visible_width = run
                .glyphs
                .iter()
                .take_while(|glyph| Self::fits(glyph, available_width))
                .map(|glyph| glyph.width)
                .sum::<f32>();
            if visible_width <= 0. {
                continue;
            }

            let run_start = run
                .glyphs
                .first()
                .map(|glyph| glyph.position_along_baseline.x())
                .unwrap_or_default();

            // Backgrounds and underlines are rects behind and under the run, so
            // they have to be drawn before its glyphs to avoid covering them.
            if let Some(background) = run.styles.background_color {
                scene
                    .draw_rect_without_hit_recording(RectF::new(
                        bounds.origin() + vec2f(run_start, 0.),
                        vec2f(visible_width, self.height()),
                    ))
                    .with_background(background);
            }

            if let Some(underline) = run.styles.underline_color {
                let offset = self.descent * UNDERLINE_OFFSET_RATIO;
                let thickness = (self.font_size * UNDERLINE_THICKNESS_RATIO).max(1.);
                scene
                    .draw_rect_without_hit_recording(RectF::new(
                        line_origin + vec2f(run_start, offset),
                        vec2f(visible_width, thickness),
                    ))
                    .with_background(underline);
            }

            for glyph in &run.glyphs {
                if !Self::fits(glyph, available_width) {
                    break;
                }
                scene.draw_glyph(
                    line_origin + glyph.position_along_baseline,
                    glyph.id,
                    run.font_id,
                    self.font_size,
                    color,
                );
            }
        }
    }

    fn fits(glyph: &Glyph, available_width: f32) -> bool {
        // Half a pixel of slack, so a line painted at exactly the width it
        // measured does not lose its last glyph to accumulated float error.
        const TOLERANCE: f32 = 0.5;

        glyph.position_along_baseline.x() + glyph.width <= available_width + TOLERANCE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::GlyphKey;
    use crate::geometry::Point;
    use crate::scene::ClipBounds;

    fn line(glyph_widths: &[f32], styles: TextStyle) -> Line {
        let mut x = 0.;
        let glyphs = glyph_widths
            .iter()
            .enumerate()
            .map(|(index, width)| {
                let glyph = Glyph {
                    id: index as u32,
                    position_along_baseline: vec2f(x, 0.),
                    index,
                    width: *width,
                };
                x += width;
                glyph
            })
            .collect();

        Line {
            width: x,
            runs: vec![Run {
                font_id: FontId(0),
                glyphs,
                styles,
                width: x,
            }],
            font_size: 10.,
            ascent: 8.,
            descent: 2.,
            ..Default::default()
        }
    }

    fn painted_glyphs(line: &Line, bounds: RectF) -> Vec<GlyphKey> {
        let mut scene = Scene::new(1.);
        line.paint(bounds, Color::WHITE, &mut scene);
        scene
            .layers()
            .flat_map(|layer| layer.glyphs.iter())
            .map(|glyph| glyph.glyph_key)
            .collect()
    }

    #[test]
    fn glyphs_sit_on_the_baseline_below_the_line_top() {
        let line = line(&[5.], TextStyle::default());
        let mut scene = Scene::new(1.);
        line.paint(
            RectF::new(vec2f(100., 200.), vec2f(50., 12.)),
            Color::WHITE,
            &mut scene,
        );

        let glyph = scene.layers().flat_map(|l| l.glyphs.iter()).next().unwrap();
        assert_eq!(glyph.position.x(), 100.);
        // Line box 12px, text box 10px, so 1px of padding above the ascent.
        assert_eq!(glyph.position.y(), 200. + 1. + 8.);
    }

    #[test]
    fn glyphs_past_the_available_width_are_dropped() {
        let line = line(&[5., 5., 5.], TextStyle::default());
        assert_eq!(
            painted_glyphs(&line, RectF::new(Vector2F::zero(), vec2f(11., 12.))).len(),
            2
        );
        assert_eq!(
            painted_glyphs(&line, RectF::new(Vector2F::zero(), vec2f(15., 12.))).len(),
            3
        );
    }

    #[test]
    fn a_backgrounded_run_draws_a_rect_that_is_click_through() {
        let styles = TextStyle {
            background_color: Some(Color::BLACK),
            ..Default::default()
        };
        let mut scene = Scene::new(1.);
        let under_the_text = Point::from_vec2f(vec2f(1., 1.), scene.z_index());
        scene.start_layer(ClipBounds::None);
        line(&[5., 5.], styles).paint(
            RectF::new(Vector2F::zero(), vec2f(20., 12.)),
            Color::WHITE,
            &mut scene,
        );
        scene.stop_layer();

        assert_eq!(scene.layers().flat_map(|l| l.rects.iter()).count(), 1);
        assert!(
            !scene.is_covered(under_the_text),
            "text decoration must not swallow clicks meant for what is under it"
        );
    }
}
