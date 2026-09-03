//! Turning an icon's outline into a coverage mask.
//!
//! Two steps: flatten the cubics into straight segments, then ask every pixel
//! how far it is from the nearest of them. There is no path filling here, no
//! winding rule, and no stroke-to-outline conversion — the three things that
//! make a vector renderer big — and the reason is that Lucide's icons are all
//! **strokes with round caps and round joins**.
//!
//! # Why distance, rather than a rasterizer
//!
//! The usual way to draw a stroke is to build the outline of the stroke — a
//! quadrilateral per segment, a wedge or an arc per join, a cap at each end —
//! and fill that with a scanline rasterizer under a winding rule. Every one of
//! those pieces exists to answer the question "is this pixel within half a
//! stroke of the path", and for a round join and a round cap that question
//! *is* the distance to the path. So the distance is what is computed, and the
//! joins and caps come out exactly round because a round join is not
//! approximated here — it is what a distance field of a polyline already is.
//!
//! Coverage from that distance is the standard exact-for-a-straight-band
//! overlap: the pixel spans half a pixel either side of its centre, the stroke
//! spans half a stroke either side of the path, and the coverage is how much
//! of the first lies inside the second. It is exact for a straight run at any
//! width — including a stroke thinner than a pixel, which a 16px icon has —
//! and off by a hair only where the path curves inside one pixel.

use crate::fonts::{Canvas, RasterFormat};
use crate::geometry::{Vector2F, vec2f};

use super::{GRID, Lucide, Segment};

/// How far a flattened curve may sit from the curve it replaces, in pixels.
///
/// A third of the distance at which a difference in coverage would round to a
/// different byte, which is where "smaller is pointless" begins.
const FLATNESS: f32 = 0.03;

/// How many times a curve may be halved before the flatness test is taken on
/// trust. Reached only by a curve with a cusp in it, which Lucide has none of.
const MAX_DEPTH: u32 = 10;

/// Below this length two points are the same point.
const EPSILON: f32 = 1e-6;

/// Draws `icon` into a `size` x `size` coverage mask.
///
/// `stroke_width` is in the 24-unit grid, not in pixels: it is scaled with the
/// icon the way `lucide-react` scales it, so proportions hold at every size.
///
/// The result is [`RasterFormat::A8`] — one coverage byte per pixel, the
/// format a mask is. Widening it to whatever the atlas holds is the
/// renderer's business.
pub fn rasterize(icon: Lucide, size: u32, stroke_width: f32) -> Canvas {
    let edge = size as usize;
    let scale = size as f32 / GRID;
    let half = (stroke_width * scale / 2.).max(0.);
    let mut coverage = vec![0u8; edge * edge];

    // Distances first, coverage second: a pixel's ink is decided by the
    // *nearest* segment, and a segment at a time is the cheap way round —
    // each one touches only the pixels within reach of it.
    let mut distances = vec![f32::INFINITY; edge * edge];
    let reach = half + 1.;

    for (start, end) in flatten(icon.outline(), scale) {
        let low = start.min(end) - Vector2F::splat(reach);
        let high = start.max(end) + Vector2F::splat(reach);
        let (first_x, last_x) = span(low.x(), high.x(), edge);
        let (first_y, last_y) = span(low.y(), high.y(), edge);

        for y in first_y..last_y {
            for x in first_x..last_x {
                let center = vec2f(x as f32 + 0.5, y as f32 + 0.5);
                let distance = &mut distances[y * edge + x];
                *distance = distance.min(distance_to_segment(center, start, end));
            }
        }
    }

    for (byte, distance) in coverage.iter_mut().zip(distances) {
        // How much of the pixel — half a pixel either side of its centre —
        // lies inside the stroke, which is half a stroke either side of the
        // path. Exact for a straight run, at any stroke width.
        let overlap = half.min(distance + 0.5) - (-half).max(distance - 0.5);
        *byte = (overlap.clamp(0., 1.) * 255.).round() as u8;
    }

    Canvas {
        pixels: coverage,
        size: (size, size),
        row_stride: edge,
        format: RasterFormat::A8,
    }
}

/// The half-open range of pixel columns — or rows — that `low..high` touches.
fn span(low: f32, high: f32, edge: usize) -> (usize, usize) {
    let first = (low.floor().max(0.) as usize).min(edge);
    let last = (high.ceil().max(0.) as usize).min(edge);
    (first, last)
}

/// An outline as straight segments in pixel space.
///
/// A subpath that draws nothing — a lone move — still becomes one zero-length
/// segment, because a round cap on a zero-length subpath is a dot, and that is
/// how `info` draws the dot on its `i`.
fn flatten(outline: &[Segment], scale: f32) -> Vec<(Vector2F, Vector2F)> {
    let mut segments = Vec::new();
    let mut point = Vector2F::zero();
    let mut start = Vector2F::zero();
    let mut drew = true;

    for segment in outline {
        match *segment {
            Segment::Move(x, y) => {
                if !drew {
                    segments.push((point, point));
                }
                point = vec2f(x, y) * scale;
                start = point;
                drew = false;
            }
            Segment::Line(x, y) => {
                let end = vec2f(x, y) * scale;
                segments.push((point, end));
                point = end;
                drew = true;
            }
            Segment::Cubic(x1, y1, x2, y2, x, y) => {
                let end = vec2f(x, y) * scale;
                flatten_cubic(
                    &mut segments,
                    point,
                    vec2f(x1, y1) * scale,
                    vec2f(x2, y2) * scale,
                    end,
                    0,
                );
                point = end;
                drew = true;
            }
            Segment::Close => {
                segments.push((point, start));
                point = start;
                drew = true;
            }
        }
    }

    if !drew {
        segments.push((point, point));
    }

    segments
}

/// Halves a cubic until each piece is within [`FLATNESS`] of its chord.
fn flatten_cubic(
    segments: &mut Vec<(Vector2F, Vector2F)>,
    p0: Vector2F,
    p1: Vector2F,
    p2: Vector2F,
    p3: Vector2F,
    depth: u32,
) {
    let flat = distance_to_line(p1, p0, p3).max(distance_to_line(p2, p0, p3)) <= FLATNESS;
    if flat || depth >= MAX_DEPTH {
        segments.push((p0, p3));
        return;
    }

    // de Casteljau at the midpoint.
    let mid = |a: Vector2F, b: Vector2F| (a + b) * 0.5;
    let p01 = mid(p0, p1);
    let p12 = mid(p1, p2);
    let p23 = mid(p2, p3);
    let p012 = mid(p01, p12);
    let p123 = mid(p12, p23);
    let center = mid(p012, p123);

    flatten_cubic(segments, p0, p01, p012, center, depth + 1);
    flatten_cubic(segments, center, p123, p23, p3, depth + 1);
}

/// How far `point` is from the segment `start`-`end`.
fn distance_to_segment(point: Vector2F, start: Vector2F, end: Vector2F) -> f32 {
    let along = end - start;
    let length_squared = along.x() * along.x() + along.y() * along.y();
    if length_squared < EPSILON * EPSILON {
        return length(point - start);
    }

    let offset = point - start;
    let t = ((offset.x() * along.x() + offset.y() * along.y()) / length_squared).clamp(0., 1.);
    length(offset - along * t)
}

/// How far `point` is from the infinite line through `start` and `end`, which
/// is the flatness test and not the same question as the one above.
fn distance_to_line(point: Vector2F, start: Vector2F, end: Vector2F) -> f32 {
    let along = end - start;
    let span = length(along);
    if span < EPSILON {
        return length(point - start);
    }

    let offset = point - start;
    (offset.x() * along.y() - offset.y() * along.x()).abs() / span
}

/// A vector's length.
fn length(vector: Vector2F) -> f32 {
    vector.x().hypot(vector.y())
}
