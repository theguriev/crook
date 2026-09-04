//! Turning a mark's outline into a coverage mask.
//!
//! An icon is two steps: flatten the cubics into straight segments, then ask
//! every pixel how far it is from the nearest of them. There is no
//! stroke-to-outline conversion — the thing that makes a vector renderer big —
//! and the reason is that Lucide's icons are all **strokes with round caps and
//! round joins**.
//!
//! [`Art`] is the other half, and it does need a fill, which is
//! [`fill`] below: one pass over the edges and one along each row, no crossing
//! list and no winding rule to configure. The two meet in [`paint`], which
//! unions a layer's fills and strokes and cuts the result to the layer beneath
//! it. Nothing else in the crate distinguishes the two kinds of mark.
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

use super::{Art, GRID, Mark, Segment};

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

/// Draws `mark` into a `size` x `size` coverage mask.
///
/// `stroke_width` is in the 24-unit grid, not in pixels: it is scaled with the
/// icon the way `lucide-react` scales it, so proportions hold at every size.
/// A [`Mark::Art`] ignores it and uses the widths its own geometry carries.
///
/// The result is [`RasterFormat::A8`] — one coverage byte per pixel, the
/// format a mask is. Widening it to whatever the atlas holds is the
/// renderer's business.
pub fn rasterize(mark: impl Into<Mark>, size: u32, stroke_width: f32) -> Canvas {
    let edge = size as usize;
    let scale = size as f32 / GRID;

    let coverage = match mark.into() {
        Mark::Icon(icon) => stroke(icon.outline(), stroke_width, scale, edge),
        Mark::Art(art) => paint(art, scale, edge),
    };

    Canvas {
        pixels: coverage,
        size: (size, size),
        row_stride: edge,
        format: RasterFormat::A8,
    }
}

/// One layer of a drawing: its fills and its strokes unioned, then cut to the
/// layer it is drawn on.
///
/// Recursive through the clip, which is the head at the bottom of every chain
/// and has none of its own — so this bottoms out in two steps, and rasterizes
/// the same face twice for a frame whose ink is asked for after it. Both are
/// cached by the atlas afterwards; doing it again is the cheaper half of
/// keeping the clip a layer rather than a shape nobody can see.
fn paint(art: Art, scale: f32, edge: usize) -> Vec<u8> {
    let painting = art.painting();
    let mut coverage = vec![0u8; edge * edge];

    for outline in painting.fills {
        union(&mut coverage, &fill(outline, scale, edge));
    }
    for (outline, width) in painting.strokes {
        union(&mut coverage, &stroke(outline, *width, scale, edge));
    }
    if let Some(clip) = painting.clip {
        clip_to(&mut coverage, &paint(clip, scale, edge));
    }

    coverage
}

/// The coverage of `outline` stroked at `stroke_width` grid units.
fn stroke(outline: &[Segment], stroke_width: f32, scale: f32, edge: usize) -> Vec<u8> {
    let half = (stroke_width * scale / 2.).max(0.);
    let mut coverage = vec![0u8; edge * edge];

    // Distances first, coverage second: a pixel's ink is decided by the
    // *nearest* segment, and a segment at a time is the cheap way round —
    // each one touches only the pixels within reach of it.
    let mut distances = vec![f32::INFINITY; edge * edge];
    let reach = half + 1.;

    for (start, end) in flatten(outline, scale) {
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

    coverage
}

/// The coverage of `outline` filled, every subpath closed.
///
/// Signed area rather than scanline crossings: each edge deposits, in the
/// pixels of the row it crosses, the *change* in how much of the row is inside
/// the shape, signed by the direction the edge runs. Running those along a row
/// gives the coverage of every pixel in it, antialiased exactly, in one pass
/// over the edges and one over the row — and it needs no sorted crossing list,
/// no winding rule spelled out, and no special case where two subpaths
/// overlap, which is the whole reason a filled mark costs so little here.
///
/// The magnitude is clamped rather than taken modulo: a shape wound over
/// itself is opaque, not transparent. Nothing here has a hole in it, and the
/// day something does it will want an explicit second layer rather than a
/// winding rule nobody can see in the geometry.
fn fill(outline: &[Segment], scale: f32, edge: usize) -> Vec<u8> {
    // Two columns wider than the mask, so that an edge which leaves the box
    // to the right has somewhere to put the area that balances it — including
    // the one that runs down the border itself, whose balance lands a column
    // further right again. Nothing reads the spare columns; what they buy is
    // that the deposit cannot land in the next row, where the running sum
    // below would spread it across a whole line of the mask.
    let width = edge + 2;
    let mut areas = vec![0f32; width * edge];

    for (start, end) in flatten(&closed(outline), scale) {
        accumulate(&mut areas, width, edge, start, end);
    }

    let mut coverage = vec![0u8; edge * edge];
    for y in 0..edge {
        let mut inside = 0f32;
        for x in 0..edge {
            inside += areas[y * width + x];
            coverage[y * edge + x] = (inside.abs().min(1.) * 255.).round() as u8;
        }
    }

    coverage
}

/// `outline` with every subpath closed, which is what a fill needs and what
/// the geometry is not obliged to say.
fn closed(outline: &[Segment]) -> Vec<Segment> {
    let mut segments = Vec::with_capacity(outline.len() + 1);
    let mut open = false;

    for segment in outline {
        if matches!(segment, Segment::Move(..)) && open {
            segments.push(Segment::Close);
        }
        open = !matches!(segment, Segment::Close);
        segments.push(*segment);
    }
    if open {
        segments.push(Segment::Close);
    }

    segments
}

/// Deposits one edge's signed area into the rows it crosses.
///
/// Per row: the edge crosses it as a trapezoid, and what lands in a pixel is
/// how much further inside the shape that pixel is than the one to its left.
/// A horizontal edge changes nothing about what is inside and is skipped —
/// which is also why the two ends of a closed subpath cancel exactly.
fn accumulate(areas: &mut [f32], width: usize, edge: usize, p0: Vector2F, p1: Vector2F) {
    if (p0.y() - p1.y()).abs() < EPSILON {
        return;
    }

    // Downwards, remembering which way it was: the sign is the winding.
    let (direction, top, bottom) = if p0.y() < p1.y() {
        (1., p0, p1)
    } else {
        (-1., p1, p0)
    };
    let dxdy = (bottom.x() - top.x()) / (bottom.y() - top.y());
    let at = |y: f32| top.x() + (y - top.y()) * dxdy;

    let (first, last) = span(top.y(), bottom.y(), edge);
    for y in first..last {
        let from = (y as f32).max(top.y());
        let to = ((y + 1) as f32).min(bottom.y());
        if to <= from {
            continue;
        }

        // Clamped into the mask: an edge to the left of the box makes
        // everything to its right inside, which is column zero's business,
        // and one to the right of it lands in the spare column.
        let limit = edge as f32;
        let (left, right) = {
            let (a, b) = (at(from).clamp(0., limit), at(to).clamp(0., limit));
            if a <= b { (a, b) } else { (b, a) }
        };
        let height = (to - from) * direction;
        let row = y * width;

        let first_column = left.floor();
        let last_column = right.ceil();
        if last_column <= first_column + 1. {
            // Within one column: the whole change happens here, split with
            // the column to the right by where the crossing's midpoint sits.
            let across = 0.5 * (left + right) - first_column;
            areas[row + first_column as usize] += height * (1. - across);
            areas[row + first_column as usize + 1] += height * across;
            continue;
        }

        // Across several: the crossing is a ramp, and each column takes the
        // slice of it that falls inside the column. The two ends are partial
        // triangles and everything between them is the same full step.
        let slope = (right - left).recip();
        let head = 0.5 * slope * (first_column + 1. - left).powi(2);
        let tail = 0.5 * slope * (right - (last_column - 1.)).powi(2);

        areas[row + first_column as usize] += height * head;
        if last_column == first_column + 2. {
            areas[row + first_column as usize + 1] += height * (1. - head - tail);
        } else {
            let second = slope * (first_column + 1.5 - left);
            areas[row + first_column as usize + 1] += height * (second - head);
            for column in (first_column as usize + 2)..(last_column as usize - 1) {
                areas[row + column] += height * slope;
            }
            let before_last = second + (last_column - first_column - 3.) * slope;
            areas[row + last_column as usize - 1] += height * (1. - before_last - tail);
        }
        areas[row + last_column as usize] += height * tail;
    }
}

/// Draws `layer` over `into`, both being coverage of the same colour.
fn union(into: &mut [u8], layer: &[u8]) {
    for (under, over) in into.iter_mut().zip(layer) {
        *under = (*under).max(*over);
    }
}

/// Cuts `into` to `mask`, which is what a layer clipped to the one below it is.
fn clip_to(into: &mut [u8], mask: &[u8]) {
    for (ink, keep) in into.iter_mut().zip(mask) {
        *ink = ((*ink as u16 * *keep as u16 + 127) / 255) as u8;
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
