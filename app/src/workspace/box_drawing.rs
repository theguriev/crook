//! Line and block characters, drawn from the cell rather than taken from a
//! font.
//!
//! A font's `│` is a glyph like any other: drawn at the font's size and
//! stopping wherever its designer stopped it. A grid's row is taller than that
//! — 1.2 times the font size, rounded up — so in most monospace families a
//! column of `│` is a dashed line with a gap at every row, and so is a column
//! of `█`. Every box an agent draws its interface in is made of these: Claude
//! Code's input box and frames, Codex's, Gemini's trust box, Copilot's
//! scrollbar and OpenCode's input bar, and `htop`'s meters besides. Ghostty,
//! kitty, WezTerm, foot and Alacritty all draw them themselves, from the
//! cell's geometry, and so does this.
//!
//! **Everything is a rectangle on the device-pixel grid.** The renderer does
//! not anti-alias a rectangle's straight edges: one whose edge falls between
//! two pixels covers whichever pixel centres it happens to, so a light line
//! would be one pixel wide in one column and two in the next. Every edge here
//! is snapped to a device pixel, and the cell's own edges are snapped the same
//! way from the same numbers, so the bottom of one row's `│` is exactly the
//! top of the next one's and nothing is left between them. The cell's edges
//! go to the pixel the renderer would start an unsnapped rectangle at there,
//! so a line also agrees with the cell backgrounds, washes and cursor beside
//! it, which are not snapped.
//!
//! What is drawn here:
//!
//! * the lines of U+2500 to U+257F — light, heavy and double, every corner,
//!   tee and cross of them, the half lines and the dashed lines;
//! * the rounded corners `╭╮╯╰`, as quarter circles: a rectangle stroked on
//!   two sides with the corner between them rounded, which the renderer
//!   already draws smooth;
//! * the blocks of U+2580 to U+259F — halves, eighths and quadrants, and the
//!   three shades as the ink at a quarter, a half and three quarters of its
//!   opacity;
//! * braille, U+2800 to U+28FF, as square dots spaced as evenly from one row
//!   to the next as within a cell, so a graph drawn in it reads as one.
//!
//! What is not: the diagonals `╱╲╳` and the powerline separators. A slope is
//! not something a rectangle can draw, and the font's glyph is a better
//! answer than a staircase of them.

use crookui_core::geometry::{Color, RectF, vec2f};
use crookui_core::scene::{Border, CornerRadius, Radius, Scene};

use super::terminal_element::RULE_THICKNESS_RATIO;

/// Draws `character` into `cell`, in `ink`, when it is one of the characters
/// this module draws, and says whether it was.
///
/// `cell` is the whole cell, in logical pixels, and `font_size` the size the
/// grid is set at, which the thickness of a line is measured from. Nothing is
/// drawn for a `false`, which leaves the character to the font.
pub(super) fn paint(
    character: char,
    cell: RectF,
    font_size: f32,
    ink: Color,
    scene: &mut Scene,
) -> bool {
    let Some(shape) = Shape::of(character) else {
        return false;
    };
    Canvas::new(cell, font_size, ink, scene).draw(shape);
    true
}

/// What a character is, as far as drawing it goes.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Shape {
    /// Lines from the centre to the edges: every straight line, corner, tee
    /// and cross, and the half lines.
    Lines(Arms),
    /// A straight line broken into this many dashes.
    Dashes(Arms, i32),
    /// A light corner turned round a quarter circle rather than a right
    /// angle.
    Arc(Arms),
    /// A solid rectangle, its edges given in eighths of the cell: left and
    /// right, then top and bottom.
    Block((i32, i32), (i32, i32)),
    /// The whole cell, in the ink at this many quarters of its opacity.
    Shade(u8),
    /// Some of the cell's four quarters, as a set of [`UPPER_LEFT`] and the
    /// rest.
    Quadrants(u8),
    /// The braille dots of this pattern, bit `n` being dot `n + 1`, which is
    /// how Unicode numbers them.
    Braille(u8),
}

impl Shape {
    /// The shape `character` is drawn as, or `None` for one the font draws.
    fn of(character: char) -> Option<Self> {
        let code = u32::from(character);
        Some(match code {
            0x2504..=0x250B => Self::Dashes(arms(code), if code < 0x2508 { 3 } else { 4 }),
            0x254C..=0x254F => Self::Dashes(arms(code), 2),
            0x256D..=0x2570 => Self::Arc(arms(code)),
            0x2571..=0x2573 => return None,
            0x2500..=0x257F => Self::Lines(arms(code)),
            0x2580 => Self::Block((0, 8), (0, 4)),
            0x2581..=0x2587 => Self::Block((0, 8), (8 - (code - 0x2580) as i32, 8)),
            0x2588 => Self::Block((0, 8), (0, 8)),
            0x2589..=0x258F => Self::Block((0, (0x2590 - code) as i32), (0, 8)),
            0x2590 => Self::Block((4, 8), (0, 8)),
            0x2591..=0x2593 => Self::Shade((code - 0x2590) as u8),
            0x2594 => Self::Block((0, 8), (0, 1)),
            0x2595 => Self::Block((7, 8), (0, 8)),
            0x2596..=0x259F => Self::Quadrants(QUADRANTS[(code - 0x2596) as usize]),
            0x2800..=0x28FF => Self::Braille((code - 0x2800) as u8),
            _ => return None,
        })
    }
}

/// The upper-left quarter of a cell, in a [`Shape::Quadrants`] set.
const UPPER_LEFT: u8 = 1;
/// The upper-right quarter.
const UPPER_RIGHT: u8 = 2;
/// The lower-left quarter.
const LOWER_LEFT: u8 = 4;
/// The lower-right quarter.
const LOWER_RIGHT: u8 = 8;

/// The quarters each of U+2596 to U+259F fills, in code point order.
const QUADRANTS: [u8; 10] = [
    LOWER_LEFT,                             // ▖
    LOWER_RIGHT,                            // ▗
    UPPER_LEFT,                             // ▘
    UPPER_LEFT | LOWER_LEFT | LOWER_RIGHT,  // ▙
    UPPER_LEFT | LOWER_RIGHT,               // ▚
    UPPER_LEFT | UPPER_RIGHT | LOWER_LEFT,  // ▛
    UPPER_LEFT | UPPER_RIGHT | LOWER_RIGHT, // ▜
    UPPER_RIGHT,                            // ▝
    UPPER_RIGHT | LOWER_LEFT,               // ▞
    UPPER_RIGHT | LOWER_LEFT | LOWER_RIGHT, // ▟
];

/// How heavy a line is on one side of a character.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Weight {
    None,
    Light,
    Heavy,
    /// Two light strokes with a light stroke's width between them.
    Double,
}

/// The four arms of a line character, from its centre out to each edge.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Arms {
    up: Weight,
    right: Weight,
    down: Weight,
    left: Weight,
}

impl Arms {
    /// The arm on one side.
    fn on(self, side: Side) -> Weight {
        match side {
            Side::Up => self.up,
            Side::Right => self.right,
            Side::Down => self.down,
            Side::Left => self.left,
        }
    }
}

/// The arms of a character from U+2500 to U+257F.
fn arms(code: u32) -> Arms {
    let [up, right, down, left] = ARMS[(code - 0x2500) as usize].map(|letter| match letter {
        b'l' => Weight::Light,
        b'h' => Weight::Heavy,
        b'd' => Weight::Double,
        _ => Weight::None,
    });
    Arms {
        up,
        right,
        down,
        left,
    }
}

/// The arms of every character from U+2500 to U+257F: up, right, down and
/// left, each light (`l`), heavy (`h`), double (`d`) or absent (`.`).
///
/// Read off the characters' Unicode names, which spell exactly this out —
/// `BOX DRAWINGS DOWN LIGHT AND RIGHT HEAVY` is `.hl.`. The dashed lines carry
/// the arms of the line they break up, the arcs those of the corner they
/// round, and the diagonals none, since they are left to the font. Every two
/// lines of entries sit under a comment naming their sixteen characters,
/// which is how the table is checked against them, and why it is not left to
/// the formatter to reflow.
#[rustfmt::skip]
const ARMS: [[u8; 4]; 128] = [
    // ─ ━ │ ┃ ┄ ┅ ┆ ┇ ┈ ┉ ┊ ┋ ┌ ┍ ┎ ┏
    *b".l.l", *b".h.h", *b"l.l.", *b"h.h.", *b".l.l", *b".h.h", *b"l.l.", *b"h.h.",
    *b".l.l", *b".h.h", *b"l.l.", *b"h.h.", *b".ll.", *b".hl.", *b".lh.", *b".hh.",
    // ┐ ┑ ┒ ┓ └ ┕ ┖ ┗ ┘ ┙ ┚ ┛ ├ ┝ ┞ ┟
    *b"..ll", *b"..lh", *b"..hl", *b"..hh", *b"ll..", *b"lh..", *b"hl..", *b"hh..",
    *b"l..l", *b"l..h", *b"h..l", *b"h..h", *b"lll.", *b"lhl.", *b"hll.", *b"llh.",
    // ┠ ┡ ┢ ┣ ┤ ┥ ┦ ┧ ┨ ┩ ┪ ┫ ┬ ┭ ┮ ┯
    *b"hlh.", *b"hhl.", *b"lhh.", *b"hhh.", *b"l.ll", *b"l.lh", *b"h.ll", *b"l.hl",
    *b"h.hl", *b"h.lh", *b"l.hh", *b"h.hh", *b".lll", *b".llh", *b".hll", *b".hlh",
    // ┰ ┱ ┲ ┳ ┴ ┵ ┶ ┷ ┸ ┹ ┺ ┻ ┼ ┽ ┾ ┿
    *b".lhl", *b".lhh", *b".hhl", *b".hhh", *b"ll.l", *b"ll.h", *b"lh.l", *b"lh.h",
    *b"hl.l", *b"hl.h", *b"hh.l", *b"hh.h", *b"llll", *b"lllh", *b"lhll", *b"lhlh",
    // ╀ ╁ ╂ ╃ ╄ ╅ ╆ ╇ ╈ ╉ ╊ ╋ ╌ ╍ ╎ ╏
    *b"hlll", *b"llhl", *b"hlhl", *b"hllh", *b"hhll", *b"llhh", *b"lhhl", *b"hhlh",
    *b"lhhh", *b"hlhh", *b"hhhl", *b"hhhh", *b".l.l", *b".h.h", *b"l.l.", *b"h.h.",
    // ═ ║ ╒ ╓ ╔ ╕ ╖ ╗ ╘ ╙ ╚ ╛ ╜ ╝ ╞ ╟
    *b".d.d", *b"d.d.", *b".dl.", *b".ld.", *b".dd.", *b"..ld", *b"..dl", *b"..dd",
    *b"ld..", *b"dl..", *b"dd..", *b"l..d", *b"d..l", *b"d..d", *b"ldl.", *b"dld.",
    // ╠ ╡ ╢ ╣ ╤ ╥ ╦ ╧ ╨ ╩ ╪ ╫ ╬ ╭ ╮ ╯
    *b"ddd.", *b"l.ld", *b"d.dl", *b"d.dd", *b".dld", *b".ldl", *b".ddd", *b"ld.d",
    *b"dl.l", *b"dd.d", *b"ldld", *b"dldl", *b"dddd", *b".ll.", *b"..ll", *b"l..l",
    // ╰ ╱ ╲ ╳ ╴ ╵ ╶ ╷ ╸ ╹ ╺ ╻ ╼ ╽ ╾ ╿
    *b"ll..", *b"....", *b"....", *b"....", *b"...l", *b"l...", *b".l..", *b"..l.",
    *b"...h", *b"h...", *b".h..", *b"..h.", *b".h.l", *b"l.h.", *b".l.h", *b"h.l.",
];

/// One side of a cell, and the arm a line character has on it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Side {
    Up,
    Right,
    Down,
    Left,
}

impl Side {
    /// Every side, clockwise from the top.
    const ALL: [Self; 4] = [Self::Up, Self::Right, Self::Down, Self::Left];

    /// Whether an arm on this side is a vertical line.
    fn is_vertical(self) -> bool {
        matches!(self, Self::Up | Self::Down)
    }

    /// Whether an arm on this side runs in from the cell's top or left edge,
    /// rather than out to its bottom or right one.
    fn leads(self) -> bool {
        matches!(self, Self::Up | Self::Left)
    }

    /// The side across the centre from this one.
    fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Right => Self::Left,
            Self::Down => Self::Up,
            Self::Left => Self::Right,
        }
    }

    /// The two sides at right angles to this one, left before right and up
    /// before down: the order a double line's two strokes come in, so each
    /// stroke is paired with the arm on its own side.
    fn across(self) -> [Self; 2] {
        if self.is_vertical() {
            [Self::Left, Self::Right]
        } else {
            [Self::Up, Self::Down]
        }
    }
}

/// Where strokes of each weight run across one axis of a cell, centred on
/// it, in device pixels from its edge.
#[derive(Copy, Clone, Debug)]
struct Bands {
    light: (i32, i32),
    heavy: (i32, i32),
}

impl Bands {
    /// The bands across an axis `extent` pixels long, for a light stroke
    /// `light` pixels thick and a heavy one twice that.
    fn centred(extent: i32, light: i32) -> Self {
        let centred = |thickness: i32| {
            let start = (extent - thickness).div_euclid(2);
            (start, start + thickness)
        };
        Self {
            light: centred(light),
            heavy: centred(light * 2),
        }
    }

    /// The span the strokes of a line of `weight` take: for a double line,
    /// both of its strokes and the gap between them.
    fn of(self, weight: Weight) -> (i32, i32) {
        match weight {
            Weight::Heavy => self.heavy,
            Weight::Double => {
                let [first, second] = self.double();
                (first.0, second.1)
            }
            Weight::None | Weight::Light => self.light,
        }
    }

    /// The two strokes of a double line, either side of where a light one
    /// runs, so the gap between them is a light line's width.
    fn double(self) -> [(i32, i32); 2] {
        let (start, end) = self.light;
        let thickness = end - start;
        [(start - thickness, start), (end, end + thickness)]
    }
}

/// The strokes an arm meets in the middle of its cell — as the span they take
/// along its axis — and whether it goes through them or stops where they
/// begin.
///
/// `across` is the pair of arms at right angles to it, and `straight` whether
/// the arm opposite is the same weight, so that the two of them are one line
/// through the centre.
///
/// An arm goes through wherever it turns into another or crosses one, since
/// stopping short would cut a notch out of the corner. It stops at them where
/// they are one line running straight past and this arm ends there — the stem
/// of a `┬` or a `╤`: through a double line it would fill the gap in, and a
/// heavy stem through a light line shows as a heavy patch on it.
fn meeting(across: [Weight; 2], straight: bool, along: Bands) -> ((i32, i32), bool) {
    if across.contains(&Weight::Heavy) {
        return (along.heavy, true);
    }
    if across[0] == across[1] && across[0] != Weight::None && !straight {
        return (along.of(across[0]), false);
    }
    let widest = if across.contains(&Weight::Double) {
        Weight::Double
    } else {
        Weight::Light
    };
    (along.of(widest), true)
}

/// `numerator / denominator` of `extent`, rounded to a whole device pixel.
///
/// Every division of the cell goes through this one rounding, so an edge two
/// shapes share — the bottom of `▀` and the top of `▄`, the middle of a
/// quadrant — is the same pixel in both.
fn part(extent: i32, numerator: i32, denominator: i32) -> i32 {
    (2 * extent * numerator + denominator).div_euclid(2 * denominator)
}

/// The device pixel an edge at `device` device pixels lands on: the first one
/// whose centre is at or past it.
///
/// Which is where the renderer starts an unsnapped rectangle whose top or left
/// edge is there, and stops one whose bottom or right edge is: a pixel is
/// covered when its centre is inside, and a centre exactly on the edge counts
/// for the top and left edges and not for the bottom and right ones. A cell
/// background, a wash or a cursor is painted unsnapped, so snapping to this
/// makes a line cover the very pixels its cell's background does. Rounding
/// would not: an edge exactly on a half pixel rounds up, past a pixel the next
/// row's background then paints over, and that is a gap in the line.
fn snap_edge(device: f32) -> f32 {
    (device - 0.5).ceil()
}

/// One cell on the device-pixel grid, and the scene its rectangles go into.
struct Canvas<'a> {
    scene: &'a mut Scene,
    ink: Color,
    /// Device pixels per logical pixel.
    scale: f32,
    /// The cell's top-left corner, in device pixels: whole ones, because it
    /// is rounded.
    origin: (f32, f32),
    /// The cell's width, in device pixels.
    width: i32,
    /// The cell's height, in device pixels.
    height: i32,
    /// How thick a light line is, in device pixels.
    light: i32,
}

impl<'a> Canvas<'a> {
    /// The cell `cell` covers on `scene`'s device pixels.
    fn new(cell: RectF, font_size: f32, ink: Color, scene: &'a mut Scene) -> Self {
        let scale = scene.scale_factor();
        // Each edge snapped on its own, rather than the corner snapped and the
        // size added to it: the edge this cell shares with the next one is
        // then the same number from both sides, which is the whole point.
        let snap = |logical: f32| snap_edge(logical * scale);
        let (left, top) = (snap(cell.min_x()), snap(cell.min_y()));
        Self {
            width: (snap(cell.max_x()) - left) as i32,
            height: (snap(cell.max_y()) - top) as i32,
            // The weight an underline is drawn at, so a box and a link under
            // it match. Never under a pixel, or a small size would draw none.
            light: ((font_size * RULE_THICKNESS_RATIO * scale).round() as i32).max(1),
            origin: (left, top),
            scale,
            ink,
            scene,
        }
    }

    /// Draws `shape` across the cell.
    fn draw(&mut self, shape: Shape) {
        let (width, height) = (self.width, self.height);
        match shape {
            Shape::Lines(arms) => self.lines(arms),
            Shape::Dashes(arms, count) => self.dashes(arms, count),
            Shape::Arc(arms) => self.arc(arms),
            Shape::Block(x, y) => self.fill(
                (part(width, x.0, 8), part(width, x.1, 8)),
                (part(height, y.0, 8), part(height, y.1, 8)),
                self.ink,
            ),
            Shape::Shade(quarters) => {
                let alpha = u16::from(self.ink.a) * u16::from(quarters) / 4;
                let shade = self.ink.with_alpha(alpha as u8);
                self.fill((0, width), (0, height), shade);
            }
            Shape::Quadrants(set) => {
                let (middle_x, middle_y) = (part(width, 1, 2), part(height, 1, 2));
                for (quarter, x, y) in [
                    (UPPER_LEFT, (0, middle_x), (0, middle_y)),
                    (UPPER_RIGHT, (middle_x, width), (0, middle_y)),
                    (LOWER_LEFT, (0, middle_x), (middle_y, height)),
                    (LOWER_RIGHT, (middle_x, width), (middle_y, height)),
                ] {
                    if set & quarter != 0 {
                        self.fill(x, y, self.ink);
                    }
                }
            }
            Shape::Braille(dots) => self.braille(dots),
        }
    }

    /// Draws each arm from its edge to where it meets the others.
    fn lines(&mut self, arms: Arms) {
        let columns = Bands::centred(self.width, self.light);
        let rows = Bands::centred(self.height, self.light);
        for side in Side::ALL {
            let weight = arms.on(side);
            if weight == Weight::None {
                continue;
            }
            // `along` is where the strokes at right angles to this arm lie on
            // its own axis, and `beside` where its own strokes lie across it.
            let (along, beside, extent) = if side.is_vertical() {
                (rows, columns, self.height)
            } else {
                (columns, rows, self.width)
            };
            let across = side.across().map(|side| arms.on(side));
            let (band, through) = meeting(across, weight == arms.on(side.opposite()), along);
            let reach = |band: (i32, i32), through: bool| match (side.leads(), through) {
                (true, true) => (0, band.1),
                (true, false) => (0, band.0),
                (false, true) => (band.0, extent),
                (false, false) => (band.1, extent),
            };

            if weight != Weight::Double {
                self.stroke(side, reach(band, through), beside.of(weight));
                continue;
            }
            for (stroke, facing) in beside.double().into_iter().zip(across) {
                // Two double lines turning into each other: this stroke is
                // on the inside of the turn, and it stops where the other
                // line's gap begins, so the corner is two nested right angles
                // rather than a grid.
                let span = if facing == Weight::Double {
                    reach(along.light, false)
                } else {
                    reach(band, through)
                };
                self.stroke(side, span, stroke);
            }
        }
    }

    /// Draws a straight line as `count` dashes.
    fn dashes(&mut self, arms: Arms, count: i32) {
        let (side, extent, beside) = if arms.up == Weight::None {
            (
                Side::Left,
                self.width,
                Bands::centred(self.height, self.light),
            )
        } else {
            (
                Side::Up,
                self.height,
                Bands::centred(self.width, self.light),
            )
        };
        let width = beside.of(arms.on(side));
        // A third of each dash's share is gap, and the gap is split between
        // the dash's two ends, so the gap across a cell boundary is the same
        // as the gaps inside a cell and a column of them is evenly dashed.
        let gap = part(extent, 1, 3 * count).max(1);
        for dash in 0..count {
            let span = (
                part(extent, dash, count) + gap / 2,
                part(extent, dash + 1, count) - (gap - gap / 2),
            );
            self.stroke(side, span, width);
        }
    }

    /// Draws a rounded corner: a rectangle from the middle of the cell out to
    /// the two edges the corner reaches, stroked along the two sides that lie
    /// on the cell's centre lines and rounded where they meet.
    fn arc(&mut self, arms: Arms) {
        let (right, down) = (arms.right != Weight::None, arms.down != Weight::None);
        let columns = Bands::centred(self.width, self.light);
        let rows = Bands::centred(self.height, self.light);
        let x = if right {
            (columns.light.0, self.width)
        } else {
            (0, columns.light.1)
        };
        let y = if down {
            (rows.light.0, self.height)
        } else {
            (0, rows.light.1)
        };

        // As round as the renderer lets a corner be, which is half the
        // shorter side: past that its stroke turns itself inside out.
        let radius = Radius::Pixels((x.1 - x.0).min(y.1 - y.0) as f32 / 2. / self.scale);
        let corner = match (down, right) {
            (true, true) => CornerRadius::with_top_left(radius),
            (true, false) => CornerRadius::with_top_right(radius),
            (false, true) => CornerRadius::with_bottom_left(radius),
            (false, false) => CornerRadius::with_bottom_right(radius),
        };
        let border = Border::new(self.light as f32 / self.scale)
            .with_sides(down, right, !down, !right)
            .with_border_color(self.ink);
        let bounds = self.bounds(x, y);
        self.scene
            .draw_rect_without_hit_recording(bounds)
            .with_border(border)
            .with_corner_radius(corner);
    }

    /// Draws a braille pattern, each dot a square in the middle of its eighth
    /// of the cell.
    fn braille(&mut self, dots: u8) {
        // Down the left column, then down the right, then the bottom row,
        // which Unicode added last.
        const PLACES: [(i32, i32); 8] = [
            (0, 0),
            (0, 1),
            (0, 2),
            (1, 0),
            (1, 1),
            (1, 2),
            (0, 3),
            (1, 3),
        ];
        let (width, height) = (self.width, self.height);
        let size = part(width, 1, 4).min(part(height, 1, 8)).max(1);
        for (bit, (column, row)) in PLACES.into_iter().enumerate() {
            if dots & (1 << bit) == 0 {
                continue;
            }
            let (left, right) = (part(width, column, 2), part(width, column + 1, 2));
            let (top, bottom) = (part(height, row, 4), part(height, row + 1, 4));
            let x = left + (right - left - size).div_euclid(2);
            let y = top + (bottom - top - size).div_euclid(2);
            self.fill((x, x + size), (y, y + size), self.ink);
        }
    }

    /// Fills one stroke of the arm on `side`: `span` along the arm and
    /// `width` across it.
    fn stroke(&mut self, side: Side, span: (i32, i32), width: (i32, i32)) {
        if side.is_vertical() {
            self.fill(width, span, self.ink);
        } else {
            self.fill(span, width, self.ink);
        }
    }

    /// Fills the rectangle from `x.0` to `x.1` and `y.0` to `y.1`, in device
    /// pixels from the cell's corner, cut to the cell.
    fn fill(&mut self, x: (i32, i32), y: (i32, i32), color: Color) {
        let x = (x.0.max(0), x.1.min(self.width));
        let y = (y.0.max(0), y.1.min(self.height));
        if x.0 >= x.1 || y.0 >= y.1 {
            return;
        }
        let bounds = self.bounds(x, y);
        self.scene
            .draw_rect_without_hit_recording(bounds)
            .with_background(color);
    }

    /// A rectangle given in device pixels from the cell's corner, as the
    /// logical one the scene takes.
    fn bounds(&self, x: (i32, i32), y: (i32, i32)) -> RectF {
        let point = |x: i32, y: i32| {
            vec2f(
                (self.origin.0 + x as f32) / self.scale,
                (self.origin.1 + y as f32) / self.scale,
            )
        };
        RectF::from_points(point(x.0, y.0), point(x.1, y.1))
    }
}

#[cfg(test)]
#[path = "box_drawing_tests.rs"]
mod tests;
