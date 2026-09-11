//! What a module carries besides code: its icon, its previews, their captions.
//!
//! A plugin's pictures are custom sections of the `.wasm`, put there by
//! `crook_plugin_api::icon!` and `preview!` and read here off the parsed
//! module — before anything in it runs, and without a byte of them ever
//! entering the guest's memory. That is what a custom section is for: the
//! interpreter parses past it, so the pictures cost no memory ceiling, no
//! fuel and no ABI.
//!
//! # One rule, read twice
//!
//! The rule is [`crook_plugin_api::pictures`]: what a section is called, what
//! a PNG in it may measure. This file applies it and answers with a sentence
//! for whichever part fails — `crook.icon is 300×200 and an icon is square` —
//! which is the sentence a registry refuses a build with, and the sentence a
//! host logs when it draws a plugin without the picture. A picture problem is
//! never a [`Problem`](crate::Problem): the module still opens, and whether a
//! plugin with a bad icon is refused or merely faceless is the caller's
//! policy, not this crate's.
//!
//! # No decoder
//!
//! A PNG's size is in its first twenty-four bytes and whether it is animated
//! is a chunk name, so both are read by hand rather than by pulling a decoder
//! into a crate every plugin host links. What is checked here is what a
//! *registry* needs to refuse a picture; whether the pixels decode is the
//! host's question, asked when it draws.

use crook_plugin_api::pictures::{
    CAPTION_SECTION_PREFIX, ICON_SECTION, MAX_CAPTION_CHARS, MAX_ICON_BYTES, MAX_ICON_EDGE,
    MAX_PREVIEW_BYTES, MAX_PREVIEW_EDGE, MAX_PREVIEWS, MIN_ICON_EDGE, PREVIEW_SECTION_PREFIX,
};
use wasmi::{Engine, Module};

/// The eight bytes every PNG starts with.
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// The pictures a module carries, each checked against the rule.
///
/// Empty for a module that carries none, which every plugin built before the
/// rule existed is — so `Default` is what a module with no sections reads as,
/// and nothing about it is an error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pictures {
    /// The icon's PNG, when there is one.
    pub icon: Option<Vec<u8>>,
    /// The previews, in the order of their numbers; a gap in the numbering
    /// is not a gap here.
    pub previews: Vec<Preview>,
}

/// One preview: a PNG, its size read off its header, and the line under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    /// The PNG's bytes.
    pub png: Vec<u8>,
    /// Its width in pixels, as its header says.
    pub width: u32,
    /// Its height in pixels, as its header says.
    pub height: u32,
    /// What the caption section of the same number said, if there was one.
    pub caption: Option<String>,
}

impl Pictures {
    /// Gathers the picture sections of a parsed module and checks every rule.
    ///
    /// The error is the sentence a registry refuses the build with, naming
    /// the section and what is wrong with it. The first thing wrong is the
    /// thing reported, in the order the sections are in the module; a caption
    /// with no preview of its number is found last, once every section has
    /// been seen. Sections with other names are somebody else's — `name`,
    /// `producers`, a debugger's — and are passed over.
    pub fn read(module: &Module) -> Result<Self, String> {
        let mut icon = None;
        let mut previews: [Option<Preview>; MAX_PREVIEWS] = Default::default();
        let mut captions: [Option<String>; MAX_PREVIEWS] = Default::default();

        for section in module.custom_sections() {
            let name = section.name();
            let data = section.data();
            if name == ICON_SECTION {
                if icon.is_some() {
                    return Err(twice(name));
                }
                checked_icon(data)?;
                icon = Some(data.to_vec());
            } else if let Some(number) = name.strip_prefix(PREVIEW_SECTION_PREFIX) {
                let slot = slot(name, number)?;
                if previews[slot].is_some() {
                    return Err(twice(name));
                }
                let (width, height) = checked_preview(name, data)?;
                previews[slot] = Some(Preview {
                    png: data.to_vec(),
                    width,
                    height,
                    caption: None,
                });
            } else if let Some(number) = name.strip_prefix(CAPTION_SECTION_PREFIX) {
                let slot = slot(name, number)?;
                if captions[slot].is_some() {
                    return Err(twice(name));
                }
                captions[slot] = Some(checked_caption(name, data)?);
            }
        }

        for (slot, caption) in captions.into_iter().enumerate() {
            let Some(caption) = caption else {
                continue;
            };
            match &mut previews[slot] {
                // An empty caption is a picture with nothing under it, which
                // is what no caption is.
                Some(preview) => preview.caption = Some(caption).filter(|text| !text.is_empty()),
                None => {
                    return Err(format!(
                        "{CAPTION_SECTION_PREFIX}{} names no picture",
                        slot + 1
                    ));
                }
            }
        }

        Ok(Self {
            icon,
            previews: previews.into_iter().flatten().collect(),
        })
    }

    /// The same read, from the bytes of a module nobody has opened.
    ///
    /// Parsing only: the module is never instantiated, so this is safe to
    /// call on an artifact that has not been checked against anything yet,
    /// and costs what parsing costs. The error for something that is not a
    /// module at all is wasmi's own sentence, prefixed.
    pub fn read_bytes(wasm: &[u8]) -> Result<Self, String> {
        let module = Module::new(&Engine::default(), wasm)
            .map_err(|why| format!("not a WebAssembly module: {why}"))?;
        Self::read(&module)
    }

    /// Whether the module carried nothing at all.
    pub fn is_empty(&self) -> bool {
        self.icon.is_none() && self.previews.is_empty()
    }
}

/// The size a PNG's header says it is, or `None` for bytes that do not start
/// like a PNG.
///
/// The signature, then the IHDR chunk, which the specification puts first
/// and fixes at thirteen bytes: width and height are the first eight of
/// them. A zero on either side is not a PNG either — the specification says
/// so — and answering it as a size would send the caller on to say "0 px a
/// side" about something that is not a picture.
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let header = png.get(..24)?;
    if &header[..8] != PNG_SIGNATURE || &header[8..16] != b"\0\0\0\x0dIHDR" {
        return None;
    }
    let width = u32::from_be_bytes(header[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(header[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

/// Whether a PNG is an APNG: an `acTL` chunk before the first `IDAT`.
///
/// That is where the specification requires the chunk to be, so the walk
/// stops at the first image data — or at the end, or at a chunk whose stated
/// length runs past the bytes, which is a file that is not what it says and
/// is at any rate not animated in a way anything would play.
pub fn is_apng(png: &[u8]) -> bool {
    if !png.starts_with(PNG_SIGNATURE) {
        return false;
    }
    let mut at = PNG_SIGNATURE.len();
    while let Some(header) = at.checked_add(8).and_then(|end| png.get(at..end)) {
        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
        match &header[4..8] {
            b"acTL" => return true,
            b"IDAT" | b"IEND" => return false,
            _ => {}
        }
        // Length, name, data, CRC.
        let Some(next) = at.checked_add(12).and_then(|next| next.checked_add(length)) else {
            return false;
        };
        at = next;
    }
    false
}

/// The sentence for a section the module holds twice.
///
/// The macros cannot write one twice — a second `icon!` is a second static
/// of the same name, which does not compile — so this is a module assembled
/// some other way, and a reader that took the first, or the last, would be
/// guessing which its author meant.
fn twice(name: &str) -> String {
    format!("{name} is in the module twice")
}

/// Which preview `number` — the part of a section name after its prefix —
/// is for, counted from zero.
///
/// One digit, `1` to [`MAX_PREVIEWS`], and nothing else: `01` and `+1` would
/// name the same preview as `1` and be two sections for one picture.
fn slot(name: &str, number: &str) -> Result<usize, String> {
    let wrong = || format!("{name} is not a number from 1 to {MAX_PREVIEWS}");
    let [digit] = number.as_bytes() else {
        return Err(wrong());
    };
    let number = usize::from(digit.wrapping_sub(b'0'));
    if !(1..=MAX_PREVIEWS).contains(&number) {
        return Err(wrong());
    }
    Ok(number - 1)
}

/// The icon's rule: a PNG, under the byte limit, square, inside the edge
/// range, and still.
fn checked_icon(data: &[u8]) -> Result<(), String> {
    let Some((width, height)) = png_size(data) else {
        return Err(format!("{ICON_SECTION} is not a PNG"));
    };
    if data.len() > MAX_ICON_BYTES {
        return Err(format!(
            "{ICON_SECTION} is {} and the limit is {}",
            kib(data.len()),
            kib(MAX_ICON_BYTES)
        ));
    }
    if width != height {
        return Err(format!(
            "{ICON_SECTION} is {width}×{height} and an icon is square"
        ));
    }
    if !(MIN_ICON_EDGE..=MAX_ICON_EDGE).contains(&width) {
        return Err(format!(
            "{ICON_SECTION} is {width} px a side and an icon is {MIN_ICON_EDGE} to {MAX_ICON_EDGE}"
        ));
    }
    if is_apng(data) {
        return Err(format!(
            "{ICON_SECTION} is animated, and an icon holds still"
        ));
    }
    Ok(())
}

/// A preview's rule: a PNG, under the byte limit, no side past the edge, and
/// still — the host's decoder refuses an animation, so a registry that let
/// one through would publish a preview nobody sees.
fn checked_preview(name: &str, data: &[u8]) -> Result<(u32, u32), String> {
    let Some((width, height)) = png_size(data) else {
        return Err(format!("{name} is not a PNG"));
    };
    if data.len() > MAX_PREVIEW_BYTES {
        return Err(format!(
            "{name} is {} and the limit is {}",
            kib(data.len()),
            kib(MAX_PREVIEW_BYTES)
        ));
    }
    if width.max(height) > MAX_PREVIEW_EDGE {
        return Err(format!(
            "{name} is {width}×{height} and a side is at most {MAX_PREVIEW_EDGE}"
        ));
    }
    if is_apng(data) {
        return Err(format!("{name} is animated, and a preview holds still"));
    }
    Ok((width, height))
}

/// A caption's rule: text, one line, inside the length.
fn checked_caption(name: &str, data: &[u8]) -> Result<String, String> {
    let Ok(text) = std::str::from_utf8(data) else {
        return Err(format!("{name} is not text"));
    };
    let count = text.chars().count();
    if count > MAX_CAPTION_CHARS {
        return Err(format!(
            "{name} is {count} characters and a caption is at most {MAX_CAPTION_CHARS}"
        ));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("{name} is not one line"));
    }
    Ok(text.to_owned())
}

/// `bytes` in whole KiB, rounded up.
///
/// Up, so that a picture one byte over a limit is never described with the
/// limit's own number: "is 32 KiB and the limit is 32 KiB" would be a
/// sentence that contradicts itself.
fn kib(bytes: usize) -> String {
    format!("{} KiB", bytes.div_ceil(1024))
}
