//! Making a theme, out of a theme.
//!
//! # Warp's algorithm, with the image taken out
//!
//! Warp's theme creator is image-driven: pick a photograph, k-means its pixels
//! in CIE L\*a\*b\* into five clusters, sort them by lightness, and let the
//! person choose which of the five is the background. Everything after that is
//! decided for them — the foreground is pure black or pure white, whichever
//! contrasts better; the accent is whichever of the four remaining clusters is
//! perceptually furthest from the background *and* the foreground; and the
//! sixteen ANSI colours come from one of two fixed palettes, chosen by which
//! foreground won.
//!
//! Crook cannot decode an image — `png` is in the workspace to *write* a
//! snapshot, the scene has no texture primitive, and there is no file picker —
//! so the first step is the one that changes: **the five candidates are
//! clustered out of a theme you already have** rather than out of a photograph.
//! Every step after it is Warp's, for the reason Warp's is good: the person
//! makes one choice, and the palette that results is internally consistent
//! because nothing about it was chosen by hand.
//!
//! That also makes the feature answer a question people actually have about a
//! terminal, which "turn this photo into a theme" only accidentally does: *I
//! like this theme but I want it darker / lighter / built around that green.*
//!
//! # What is different, and deliberately
//!
//! **Two distances, as Warp has two.** The clustering measures with plain
//! Euclidean distance in L\*a\*b\* — which is what k-means in Lab means, and
//! what Warp's clustering crate does. The accent is picked with **CIEDE2000**,
//! which is what Warp's accent picker uses, and the difference is not academic:
//! against a black background and white text, of a bright red, a dark red and
//! a nearly-black red, Euclidean distance picks the *brightest* and CIEDE2000
//! picks the middle one. Warp pins that case in a test of its own, because an
//! accent that hugs either end of the palette is the wrong answer, and
//! CIEDE2000's compression of differences at high chroma is precisely what
//! produces the right one.
//!
//! **A deterministic start rather than a seed.** Warp seeds its k-means with
//! zero so a photograph always yields the same theme. Farthest-point
//! initialisation gets the same property without a random number generator at
//! all: the first centroid is the darkest colour, and each next one is
//! whichever colour is furthest from those already chosen.

use crookui_core::geometry::Color;

use super::builtin::{ANSI_BRIGHT, ANSI_NORMAL, LIGHT};
use super::{TerminalColors, Theme};

/// How many candidate colours a person chooses their background from.
///
/// Warp's k, and its five swatches. Fewer than five and a palette with a
/// distinct accent in it loses the accent; more and the row of swatches stops
/// being a glance.
pub const CANDIDATES: usize = 5;

/// How many times the clustering moves its centroids.
///
/// Warp's twenty. Over eighteen colours it converges in three or four.
const ITERATIONS: usize = 20;

/// A draft theme: what the creator is holding while somebody chooses.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// The five colours to choose a background from, darkest first.
    pub candidates: [Color; CANDIDATES],
    /// Which of them is the background.
    pub chosen: usize,
    /// The palette that choice produces.
    pub theme: Theme,
}

impl Draft {
    /// Starts a draft from the theme `source`.
    ///
    /// The initial background is the darkest candidate, which is Warp's
    /// default: index 0 of a list sorted by lightness.
    pub fn new(source: &Theme) -> Self {
        let candidates = candidates(source);
        Self {
            candidates,
            chosen: 0,
            theme: assemble(candidates, 0),
        }
    }

    /// Chooses a different background, and rebuilds everything that follows
    /// from it.
    pub fn choose(&mut self, index: usize) {
        let index = index.min(CANDIDATES - 1);
        if index == self.chosen {
            return;
        }
        self.chosen = index;
        self.theme = assemble(self.candidates, index);
    }
}

/// The five colours a theme is built from, darkest first.
///
/// Clustered out of the source's own eighteen: its background, its foreground
/// and the sixteen a program can ask for by number. Sorted by lightness, so
/// index 0 is the darkest — the same order Warp's swatches are in, and the
/// same default background.
pub fn candidates(source: &Theme) -> [Color; CANDIDATES] {
    let mut colors: Vec<Lab> = Vec::with_capacity(18);
    colors.push(Lab::of(source.terminal.background));
    colors.push(Lab::of(source.terminal.foreground));
    for color in source
        .terminal
        .normal
        .iter()
        .chain(source.terminal.bright.iter())
    {
        colors.push(Lab::of(*color));
    }

    let mut centroids = initial_centroids(&colors);
    for _ in 0..ITERATIONS {
        if !step(&colors, &mut centroids) {
            break;
        }
    }

    // Ascending by lightness: Warp sorts its clusters the same way, and it is
    // what makes "index 0" mean "the darkest one" rather than "whichever the
    // clustering happened to settle first".
    centroids.sort_by(|left, right| left.l.total_cmp(&right.l));

    let mut chosen = [Color::BLACK; CANDIDATES];
    for (slot, centroid) in chosen.iter_mut().zip(centroids) {
        *slot = centroid.to_color();
    }
    chosen
}

/// Where the clustering starts.
///
/// Farthest-point initialisation: the darkest colour, then repeatedly whichever
/// colour is furthest from everything chosen so far. Deterministic, which is
/// what a seeded random start buys Warp, and it puts the starting centroids
/// where the answer wants them rather than where a seed happens to land.
fn initial_centroids(colors: &[Lab]) -> Vec<Lab> {
    let mut centroids: Vec<Lab> = Vec::with_capacity(CANDIDATES);

    let darkest = colors
        .iter()
        .copied()
        .min_by(|left, right| left.l.total_cmp(&right.l))
        .unwrap_or(Lab {
            l: 0.,
            a: 0.,
            b: 0.,
        });
    centroids.push(darkest);

    while centroids.len() < CANDIDATES {
        let furthest = colors
            .iter()
            .copied()
            .max_by(|left, right| {
                nearest_distance(*left, &centroids).total_cmp(&nearest_distance(*right, &centroids))
            })
            .unwrap_or(darkest);
        centroids.push(furthest);
    }

    centroids
}

/// One Lloyd iteration. Returns whether anything moved.
fn step(colors: &[Lab], centroids: &mut [Lab]) -> bool {
    let mut sums = vec![(0., 0., 0., 0usize); centroids.len()];

    for color in colors {
        let nearest = centroids
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                distance(*color, **left).total_cmp(&distance(*color, **right))
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        let slot = &mut sums[nearest];
        slot.0 += f64::from(color.l);
        slot.1 += f64::from(color.a);
        slot.2 += f64::from(color.b);
        slot.3 += 1;
    }

    let mut moved = false;
    for (centroid, (l, a, b, count)) in centroids.iter_mut().zip(sums) {
        // A centroid nothing landed on keeps its place rather than collapsing
        // onto another: five swatches with two the same is a worse answer than
        // five with one that no colour voted for.
        if count == 0 {
            continue;
        }
        let next = Lab {
            l: (l / count as f64) as f32,
            a: (a / count as f64) as f32,
            b: (b / count as f64) as f32,
        };
        if distance(next, *centroid) > 0.01 {
            moved = true;
        }
        *centroid = next;
    }
    moved
}

/// The palette that a chosen background implies.
///
/// Warp's four steps, in Warp's order.
fn assemble(candidates: [Color; CANDIDATES], chosen: usize) -> Theme {
    let background = candidates[chosen];
    let foreground = foreground_for(background);
    let accent = accent_for(background, foreground, candidates, chosen);
    let (normal, bright) = ansi_for(foreground);

    Theme::derived(
        accent,
        TerminalColors {
            foreground,
            background,
            // Warp's generated themes omit the cursor entirely and fall back to
            // the accent. Written out here because `TerminalColors` has no
            // notion of an absent colour.
            cursor: accent,
            normal,
            bright,
        },
    )
}

/// Black or white, whichever reads better on `background`.
///
/// Warp's `pick_foreground_color`, and it is a two-way choice: nothing else is
/// ever produced. The contrast ratio is WCAG 2.0's — relative luminance with
/// the sRGB gamma curve undone, plus the 0.05 offset that keeps the ratio
/// finite at black.
pub fn foreground_for(background: Color) -> Color {
    let black = Color::hex(0x000000);
    let white = Color::hex(0xffffff);

    if contrast(background, black) > contrast(background, white) {
        black
    } else {
        // A tie goes to white, as it does in Warp: mid-grey reads as a dark
        // theme, which is what a terminal usually wants to be.
        white
    }
}

/// The candidate furthest from both the background and the text.
///
/// Warp's `pick_accent_color_from_options`: the summed CIEDE2000 distance from
/// each of the two reference colours, and the largest sum wins. The chosen
/// background is not a candidate; the foreground never was one, it is only a
/// reference point.
fn accent_for(
    background: Color,
    foreground: Color,
    candidates: [Color; CANDIDATES],
    chosen: usize,
) -> Color {
    let reference = [Lab::of(background), Lab::of(foreground)];

    candidates
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != chosen)
        .map(|(_, candidate)| *candidate)
        .max_by(|left, right| {
            let score = |color: Color| {
                let lab = Lab::of(color);
                reference
                    .iter()
                    .map(|point| ciede2000(lab, *point))
                    .sum::<f32>()
            };
            score(*left).total_cmp(&score(*right))
        })
        // Unreachable with five candidates and one of them excluded.
        .unwrap_or(foreground)
}

/// The sixteen a generated theme is given.
///
/// Warp picks one of two fixed palettes by which foreground won, and these are
/// Crook's own two: the xterm values every terminal agrees on for a dark
/// theme, and the darkened set `Crook Light` carries for a light one — the
/// standard bright colours are chosen to sit on black and are unreadable on
/// white.
pub fn ansi_for(foreground: Color) -> ([Color; 8], [Color; 8]) {
    if luminance(foreground) < 128 {
        (LIGHT.terminal.normal, LIGHT.terminal.bright)
    } else {
        (ANSI_NORMAL, ANSI_BRIGHT)
    }
}

/// The WCAG 2.0 contrast ratio between two colours, 1 to 21.
fn contrast(left: Color, right: Color) -> f32 {
    let (lighter, darker) = {
        let (left, right) = (relative_luminance(left), relative_luminance(right));
        if left > right {
            (left, right)
        } else {
            (right, left)
        }
    };
    (lighter + 0.05) / (darker + 0.05)
}

/// WCAG relative luminance: the sRGB gamma curve undone, then weighted.
fn relative_luminance(color: Color) -> f32 {
    let channel = |value: u8| {
        let value = f32::from(value) / 255.;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };

    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

/// The same integer luma the theme module infers light from dark with.
fn luminance(color: Color) -> u8 {
    ((2126 * u32::from(color.r) + 7152 * u32::from(color.g) + 722 * u32::from(color.b)) / 10000)
        as u8
}

/// A colour in CIE L\*a\*b\*, where distance means what the eye means by it.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Lab {
    l: f32,
    a: f32,
    b: f32,
}

impl Lab {
    /// sRGB to L\*a\*b\*, through linear RGB and XYZ under a D65 white point.
    fn of(color: Color) -> Self {
        let linear = |value: u8| {
            let value = f32::from(value) / 255.;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (linear(color.r), linear(color.g), linear(color.b));

        // sRGB primaries against D65, and the white point they normalise to.
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;

        let f = |value: f32| {
            if value > 0.008856 {
                value.cbrt()
            } else {
                7.787 * value + 16. / 116.
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));

        Self {
            l: 116. * fy - 16.,
            a: 500. * (fx - fy),
            b: 200. * (fy - fz),
        }
    }

    /// Back to a colour, by inverting the same two steps.
    fn to_color(self) -> Color {
        let fy = (self.l + 16.) / 116.;
        let fx = fy + self.a / 500.;
        let fz = fy - self.b / 200.;

        let inverse = |value: f32| {
            let cubed = value * value * value;
            if cubed > 0.008856 {
                cubed
            } else {
                (value - 16. / 116.) / 7.787
            }
        };
        let (x, y, z) = (inverse(fx) * 0.95047, inverse(fy), inverse(fz) * 1.08883);

        let linear_r = 3.2406 * x - 1.5372 * y - 0.4986 * z;
        let linear_g = -0.9689 * x + 1.8758 * y + 0.0415 * z;
        let linear_b = 0.0557 * x - 0.2040 * y + 1.0570 * z;

        let channel = |value: f32| {
            let value = value.clamp(0., 1.);
            let value = if value <= 0.0031308 {
                value * 12.92
            } else {
                1.055 * value.powf(1. / 2.4) - 0.055
            };
            (value * 255.).round().clamp(0., 255.) as u8
        };

        Color::rgb(channel(linear_r), channel(linear_g), channel(linear_b))
    }
}

/// ΔE76: the straight-line distance between two colours in L\*a\*b\*.
///
/// What the clustering measures with, because that is what k-means in Lab
/// means. The accent picker uses [`ciede2000`] instead — see the module docs
/// for why the two are different questions.
fn distance(left: Lab, right: Lab) -> f32 {
    let l = left.l - right.l;
    let a = left.a - right.a;
    let b = left.b - right.b;
    (l * l + a * a + b * b).sqrt()
}

/// ΔE2000: the CIE's 2000 revision of perceptual distance.
///
/// The formula as published (Sharma, Wu and Dalal's formulation, which is the
/// one every implementation follows), with the standard weighting factors
/// `kL = kC = kH = 1`. It exists because ΔE76 badly overstates differences in
/// saturated colours and understates them in the blues; the three `S` terms
/// scale the lightness, chroma and hue differences by where in the space they
/// were measured, and `Rt` is the rotation that fixes the blue region.
///
/// This is the one piece of maths in Crook copied from a specification rather
/// than derived — it is a standard, and an approximation of it would produce
/// accents that are subtly wrong in exactly the way the standard exists to
/// prevent.
fn ciede2000(left: Lab, right: Lab) -> f32 {
    const POW_25_7: f64 = 6_103_515_625.; // 25^7

    let (l1, a1, b1) = (f64::from(left.l), f64::from(left.a), f64::from(left.b));
    let (l2, a2, b2) = (f64::from(right.l), f64::from(right.a), f64::from(right.b));

    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let c_bar = (c1 + c2) / 2.;

    let g = 0.5 * (1. - (c_bar.powi(7) / (c_bar.powi(7) + POW_25_7)).sqrt());
    let (a1p, a2p) = ((1. + g) * a1, (1. + g) * a2);
    let c1p = (a1p * a1p + b1 * b1).sqrt();
    let c2p = (a2p * a2p + b2 * b2).sqrt();

    // Hue angles in degrees, zero for a colour with no chroma at all.
    let hue = |a: f64, b: f64| {
        if a == 0. && b == 0. {
            0.
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.)
        }
    };
    let (h1p, h2p) = (hue(a1p, b1), hue(a2p, b2));

    let delta_l = l2 - l1;
    let delta_c = c2p - c1p;
    let delta_h = if c1p * c2p == 0. {
        0.
    } else {
        let difference = h2p - h1p;
        if difference.abs() <= 180. {
            difference
        } else if difference > 180. {
            difference - 360.
        } else {
            difference + 360.
        }
    };
    let delta_upper_h = 2. * (c1p * c2p).sqrt() * (delta_h.to_radians() / 2.).sin();

    let l_bar = (l1 + l2) / 2.;
    let c_bar_p = (c1p + c2p) / 2.;
    let h_bar_p = if c1p * c2p == 0. {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180. {
        (h1p + h2p) / 2.
    } else if h1p + h2p < 360. {
        (h1p + h2p + 360.) / 2.
    } else {
        (h1p + h2p - 360.) / 2.
    };

    let t = 1. - 0.17 * (h_bar_p - 30.).to_radians().cos()
        + 0.24 * (2. * h_bar_p).to_radians().cos()
        + 0.32 * (3. * h_bar_p + 6.).to_radians().cos()
        - 0.20 * (4. * h_bar_p - 63.).to_radians().cos();

    let s_l = 1. + (0.015 * (l_bar - 50.).powi(2)) / (20. + (l_bar - 50.).powi(2)).sqrt();
    let s_c = 1. + 0.045 * c_bar_p;
    let s_h = 1. + 0.015 * c_bar_p * t;

    let delta_theta = 30. * (-((h_bar_p - 275.) / 25.).powi(2)).exp();
    let r_c = 2. * (c_bar_p.powi(7) / (c_bar_p.powi(7) + POW_25_7)).sqrt();
    let r_t = -(2. * delta_theta).to_radians().sin() * r_c;

    let lightness = delta_l / s_l;
    let chroma = delta_c / s_c;
    let hue_term = delta_upper_h / s_h;

    ((lightness * lightness + chroma * chroma + hue_term * hue_term + r_t * chroma * hue_term)
        .max(0.)
        .sqrt()) as f32
}

/// How far `color` is from the nearest of `centroids`.
fn nearest_distance(color: Lab, centroids: &[Lab]) -> f32 {
    centroids
        .iter()
        .map(|centroid| distance(color, *centroid))
        .fold(f32::INFINITY, f32::min)
}

#[cfg(test)]
#[path = "creator_tests.rs"]
mod tests;
