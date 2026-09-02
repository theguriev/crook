//! Byte-offset translation across the paragraph join that shaping performs.
//!
//! Warp's file of this name maps byte offsets to *char* offsets, because
//! `warpui_core` indexes glyphs and style runs by char while cosmic-text
//! reports bytes. `crookui_core` indexes both by byte — see
//! [`crookui_core::text_layout::Glyph::index`] — so that conversion does not
//! exist here. What survives is the one index space text layout still creates
//! for itself.
//!
//! [`cosmic_text::ShapeLine`] wants a single paragraph: it asserts that every
//! paragraph in its input runs in the same direction, and a tab title carrying
//! a newline out of agent output is exactly the value that breaks that
//! assumption. Warp flattens the input by joining the paragraphs with a
//! zero-width space (U+200B), which shapes to nothing, and so does this. The
//! separator being one byte (`\n`) while the joiner is three shifts every
//! offset past the first newline, and undoing that shift is this module's whole
//! job: the caller's style runs and the [`Glyph::index`] values handed back to
//! it both speak in offsets into the *original* string.
//!
//! [`Glyph::index`]: crookui_core::text_layout::Glyph::index

use std::borrow::Cow;

use cosmic_text::BidiParagraphs;

/// What paragraphs are joined with. Zero width, so it costs no advance, and
/// three bytes, which is why the offsets need mapping at all.
const JOINER: &str = "\u{200b}";

/// One paragraph's position in both index spaces.
#[derive(Copy, Clone, Debug)]
struct Paragraph {
    /// Offset of the paragraph in the shaped string.
    shaped_start: usize,
    /// Offset of the same paragraph in the source string.
    source_start: usize,
    /// Byte length, identical in both spaces.
    len: usize,
}

/// A string prepared for shaping, plus the map back to the caller's offsets.
///
/// Construction is O(n) in the length of the text and allocates only when the
/// text actually spans more than one paragraph; the overwhelmingly common
/// single-paragraph case borrows the input and maps offsets to themselves.
pub(super) struct StrIndexMap<'a> {
    text: Cow<'a, str>,

    /// Ascending by both starts. Empty means the identity map.
    paragraphs: Vec<Paragraph>,
}

impl<'a> StrIndexMap<'a> {
    /// Flattens `text` to a single paragraph and records how to get back.
    pub(super) fn new(text: &'a str) -> Self {
        let mut paragraphs = Vec::new();
        let mut shaped = String::new();

        for source in BidiParagraphs::new(text) {
            // `BidiParagraphs` yields subslices of `text`, so the distance
            // between the two pointers is the paragraph's source offset. The
            // iterator reports no offsets of its own and rebuilding its
            // paragraph-splitting rules here would be a second, divergent
            // definition of where a paragraph ends.
            let source_start = source.as_ptr() as usize - text.as_ptr() as usize;

            if !paragraphs.is_empty() {
                shaped.push_str(JOINER);
            }
            paragraphs.push(Paragraph {
                shaped_start: shaped.len(),
                source_start,
                len: source.len(),
            });
            shaped.push_str(source);
        }

        let single_paragraph = match paragraphs.as_slice() {
            [] => true,
            [only] => only.source_start == 0 && only.len == text.len(),
            _ => false,
        };
        if single_paragraph {
            return Self {
                text: Cow::Borrowed(text),
                paragraphs: Vec::new(),
            };
        }

        Self {
            text: Cow::Owned(shaped),
            paragraphs,
        }
    }

    /// The string to hand the shaper.
    pub(super) fn shaped_text(&self) -> &str {
        &self.text
    }

    /// Translates an offset the shaper reported into the caller's string.
    ///
    /// An offset landing inside a joiner — which no glyph can, since the joiner
    /// shapes away, but which a clamped style-run end can — resolves to the end
    /// of the paragraph before it.
    pub(super) fn source_index(&self, shaped_index: usize) -> usize {
        let Some(paragraph) = self.paragraph_by(|p| p.shaped_start, shaped_index) else {
            return shaped_index.min(self.text.len());
        };

        let offset_in_paragraph = (shaped_index - paragraph.shaped_start).min(paragraph.len);
        paragraph.source_start + offset_in_paragraph
    }

    /// Translates an offset in the caller's string into the shaped string.
    ///
    /// A source offset that falls on a paragraph separator resolves to the end
    /// of the paragraph that separator terminates, so a style run covering a
    /// newline keeps covering the text on both sides of it.
    pub(super) fn shaped_index(&self, source_index: usize) -> usize {
        let Some(paragraph) = self.paragraph_by(|p| p.source_start, source_index) else {
            return source_index.min(self.text.len());
        };

        let offset_in_paragraph = (source_index - paragraph.source_start).min(paragraph.len);
        paragraph.shaped_start + offset_in_paragraph
    }

    /// The last paragraph whose start in the `key` space is at or before
    /// `index`, or `None` when this is the identity map.
    fn paragraph_by(&self, key: impl Fn(&Paragraph) -> usize, index: usize) -> Option<Paragraph> {
        if self.paragraphs.is_empty() {
            return None;
        }

        let after = self
            .paragraphs
            .partition_point(|paragraph| key(paragraph) <= index);
        Some(self.paragraphs[after.saturating_sub(1)])
    }
}
