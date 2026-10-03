//! Pictures a program prints into the output: iTerm2's inline images.
//!
//! `imgcat`, `yazi`'s previewer, `chafa -f iterm`, `viu` and a great many
//! scripts speak iTerm2's sequence, which WezTerm, Konsole and VS Code's
//! terminal read too:
//!
//! ```text
//! OSC 1337 ; File = [args] : <base64 file> BEL
//! ```
//!
//! `args` is a `;`-separated list of `key=value`, of which four are read:
//! `inline=1`, without which the file is a download and not a picture;
//! `width` and `height`, each a count of cells, `Npx`, `N%` of the pane, or
//! `auto`; and `preserveAspectRatio=0`, which stretches the picture to the
//! box instead of fitting it inside. The same file may arrive in pieces —
//! `MultipartFile=[args]`, any number of `FilePart=<base64>`, then `FileEnd`
//! — which is what iTerm2's own `imgcat` sends inside tmux.
//!
//! # What happens to one
//!
//! The picture is placed at the cursor and the cursor is moved past it, down
//! to its last row and along to the column after it, the way iTerm2 moves it;
//! so the rows it covers are rows of the grid like any other, scrolled and
//! harvested with the block it was printed in. The block keeps the picture
//! beside its text (see [`crate::Block::images`]) and the renderer draws it
//! over the rows it covers.
//!
//! # The bytes came from anything that can print
//!
//! So a picture is believed only within limits: PNG alone, whose size is read
//! from its header before anything is reserved for it — this crate decodes
//! no pixels, the renderer does, and a format it cannot decode would be a
//! hole in the output with nothing in it — a file of at most [`MAX_FILE`]
//! bytes, at most [`MAX_SIDE`] pixels a side, and never on the alternate
//! screen, whose program owns every cell. A picture past any of them is
//! dropped and the stream goes on as if it had not been there.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;

/// The most a picture's file may be, decoded.
pub const MAX_FILE: usize = 16 << 20;

/// The most a picture may be on either side, in pixels.
pub const MAX_SIDE: u32 = 4096;

/// The cell size assumed when the pane has not said what its cells are.
const FALLBACK_CELL: (u32, u32) = (8, 16);

/// One picture a program printed: the PNG it sent, and how big it says it is.
///
/// Compared by identity. Two prints of the same file are two pictures, and a
/// renderer's cache keyed on [`Self::id`] decodes each once.
pub struct InlineImage {
    id: u64,
    png: Vec<u8>,
    size: (u32, u32),
}

impl InlineImage {
    /// A picture of `png`'s bytes, if they are a PNG within the limits.
    pub fn new(png: Vec<u8>) -> Option<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        if png.len() > MAX_FILE {
            return None;
        }
        let size = png_size(&png)?;
        if size.0 == 0 || size.1 == 0 || size.0 > MAX_SIDE || size.1 > MAX_SIDE {
            return None;
        }
        Some(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            png,
            size,
        })
    }

    /// The identity a renderer keys its decoded copy on.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The file, as the program sent it.
    pub fn png(&self) -> &[u8] {
        &self.png
    }

    /// Its width and height in pixels, as its header says.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

impl PartialEq for InlineImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for InlineImage {}

impl fmt::Debug for InlineImage {
    // Not the bytes: a snapshot's debug print with a megabyte in it is a
    // log line nobody can read.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InlineImage")
            .field("id", &self.id)
            .field("size", &self.size)
            .field("bytes", &self.png.len())
            .finish()
    }
}

/// Where a picture is drawn, in cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePlacement {
    /// The picture.
    pub image: Arc<InlineImage>,
    /// The row its top is on: a viewport row on a [`crate::LiveBlock`], which
    /// may be negative or past the screen as its rows are, and a row of the
    /// block on a finished [`crate::Block`].
    pub row: i32,
    /// The column its left edge is on.
    pub column: usize,
    /// How many columns it covers.
    pub columns: usize,
    /// How many rows it covers.
    pub rows: usize,
    /// Whether it is stretched to fill those cells rather than fitted inside
    /// them with its shape kept: `preserveAspectRatio=0`.
    pub stretch: bool,
}

/// A picture a program asked for, before it has been given cells.
#[derive(Debug)]
pub(crate) struct Requested {
    pub(crate) image: InlineImage,
    pub(crate) width: Dimension,
    pub(crate) height: Dimension,
    pub(crate) stretch: bool,
}

/// One side of the box a program asked a picture to be drawn in.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum Dimension {
    /// The picture's own size.
    Auto,
    /// A count of cells.
    Cells(u32),
    /// A count of pixels.
    Pixels(u32),
    /// A share of the pane, in percent.
    Percent(u32),
}

impl Dimension {
    fn parse(value: &str) -> Self {
        let number = |digits: &str| digits.trim().parse::<u32>().ok();
        if let Some(pixels) = value.strip_suffix("px").and_then(number) {
            Self::Pixels(pixels)
        } else if let Some(percent) = value.strip_suffix('%').and_then(number) {
            Self::Percent(percent.min(100))
        } else {
            number(value).map_or(Self::Auto, Self::Cells)
        }
    }

    /// This side in pixels: `natural` for `auto`, `cell` per cell, or a share
    /// of `pane`.
    fn pixels(self, natural: u32, cell: u32, pane: u32) -> Option<u32> {
        match self {
            Self::Auto => Some(natural),
            Self::Cells(cells) => Some(cells.saturating_mul(cell)),
            Self::Pixels(pixels) => Some(pixels),
            Self::Percent(percent) => Some(pane.saturating_mul(percent) / 100),
        }
        .filter(|pixels| *pixels > 0)
    }
}

/// The cells a picture covers: `(columns, rows)`.
///
/// The box the program asked for, with an `auto` side worked out from the
/// other so the picture keeps its shape, and the picture fitted inside it
/// unless it asked to be stretched. Then made to fit the pane, narrowed to
/// its width and shortened to its height, still keeping its shape: a picture
/// wider than the pane would be cut off at its edge, and one taller than the
/// screen could never be seen whole.
pub(crate) fn cells(
    requested: &Requested,
    cell: (u16, u16),
    pane: (usize, usize),
) -> (usize, usize) {
    let cell = match cell {
        (0, _) | (_, 0) => FALLBACK_CELL,
        (width, height) => (u32::from(width), u32::from(height)),
    };
    let pane_pixels = (
        (pane.0 as u32).saturating_mul(cell.0).max(1),
        (pane.1 as u32).saturating_mul(cell.1).max(1),
    );
    let (natural_width, natural_height) = requested.image.size();
    let (natural_width, natural_height) = (f64::from(natural_width), f64::from(natural_height));

    let asked_width = requested
        .width
        .pixels(natural_width as u32, cell.0, pane_pixels.0);
    let asked_height = requested
        .height
        .pixels(natural_height as u32, cell.1, pane_pixels.1);
    let (mut width, mut height) = match (requested.width, requested.height) {
        (Dimension::Auto, Dimension::Auto) => (natural_width, natural_height),
        (_, Dimension::Auto) => {
            let width = f64::from(asked_width.unwrap_or(1));
            (width, width * natural_height / natural_width)
        }
        (Dimension::Auto, _) => {
            let height = f64::from(asked_height.unwrap_or(1));
            (height * natural_width / natural_height, height)
        }
        _ => {
            let (box_width, box_height) = (
                f64::from(asked_width.unwrap_or(1)),
                f64::from(asked_height.unwrap_or(1)),
            );
            if requested.stretch {
                (box_width, box_height)
            } else {
                let scale = (box_width / natural_width).min(box_height / natural_height);
                (natural_width * scale, natural_height * scale)
            }
        }
    };

    let shrink = (f64::from(pane_pixels.0) / width)
        .min(f64::from(pane_pixels.1) / height)
        .min(1.);
    width *= shrink;
    height *= shrink;

    let columns = (width / f64::from(cell.0)).ceil().max(1.) as usize;
    let rows = (height / f64::from(cell.1)).ceil().max(1.) as usize;
    (columns.min(pane.0.max(1)), rows.min(pane.1.max(1)))
}

/// Reads OSC 1337 file transfers, a piece at a time when they come in pieces.
#[derive(Default)]
pub(crate) struct Reader {
    /// A `MultipartFile` that has begun: its arguments, and the base64 that
    /// has arrived for it so far.
    multipart: Option<(String, Vec<u8>)>,
}

impl Reader {
    /// Reads one OSC 1337, and returns a picture if it finished one.
    pub(crate) fn read(&mut self, params: &[&[u8]]) -> Option<Requested> {
        // `vte` cut the sequence at every `;`, and the arguments are separated
        // by them, so they are put back together first.
        let body = params.get(1..)?.join(&b';');
        if let Some(rest) = body.strip_prefix(b"File=") {
            self.multipart = None;
            let colon = rest.iter().position(|byte| *byte == b':')?;
            let (args, data) = rest.split_at(colon);
            return request(str::from_utf8(args).ok()?, &data[1..]);
        }
        if let Some(args) = body.strip_prefix(b"MultipartFile=") {
            self.multipart = Some((String::from_utf8_lossy(args).into_owned(), Vec::new()));
            return None;
        }
        if let Some(part) = body.strip_prefix(b"FilePart=") {
            let (_, data) = self.multipart.as_mut()?;
            // Base64 is a third bigger than what it carries, and a stream that
            // never sends `FileEnd` is not let grow past the largest file a
            // picture may be.
            if data.len() + part.len() > MAX_FILE / 3 * 4 + 4 {
                self.multipart = None;
                return None;
            }
            data.extend_from_slice(part);
            return None;
        }
        if body == b"FileEnd" {
            let (args, data) = self.multipart.take()?;
            return request(&args, &data);
        }
        None
    }
}

/// A picture out of a file transfer's arguments and its base64, if it is an
/// inline one this crate can place.
fn request(args: &str, data: &[u8]) -> Option<Requested> {
    let mut inline = false;
    let mut width = Dimension::Auto;
    let mut height = Dimension::Auto;
    let mut stretch = false;
    for arg in args.split(';') {
        let Some((key, value)) = arg.split_once('=') else {
            continue;
        };
        match key {
            "inline" => inline = value == "1",
            "width" => width = Dimension::parse(value),
            "height" => height = Dimension::parse(value),
            "preserveAspectRatio" => stretch = value == "0",
            _ => {}
        }
    }
    if !inline || data.len() > MAX_FILE / 3 * 4 + 4 {
        return None;
    }
    // Whitespace is dropped first: a script that wraps its base64 at 76
    // columns, as `base64` does by default, sends newlines inside it.
    let data: Vec<u8> = data
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    let png = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .ok()?;
    Some(Requested {
        image: InlineImage::new(png)?,
        width,
        height,
        stretch,
    })
}

/// A PNG's width and height, out of its header, or `None` for anything that
/// is not a PNG.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || !bytes.starts_with(SIGNATURE) || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
