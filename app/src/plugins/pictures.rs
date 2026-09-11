//! What a plugin looks like, as the host holds it.
//!
//! `crook_wasm` reads a module's pictures out as PNG bytes and checks them
//! against the rule; this is the shape they take once they are the host's. The
//! icon is decoded on the spot — it is a few kilobytes, it is drawn on every
//! frame the list is up, and a row cannot wait for it — and the previews are
//! kept as the bytes they arrived as, decoded only when somebody asks to see
//! them: a card of six screenshots is megabytes of pixels for a card most
//! people never scroll to.

use std::sync::Arc;

use crookui_core::image::Bitmap;

use crook_plugin::PluginId;

use crate::picture::{self, Limits};

/// One plugin's previews, decoded.
///
/// What a card draws once somebody asks to see them, and what the pool
/// hands back.
pub type Decoded = Vec<Picture>;

/// One preview a card can draw: the pixels, the size it was captured at,
/// and its caption.
///
/// The captured size rides with the pixels rather than being looked up in
/// the module's list by position, because a preview that will not decode is
/// left out of this list — and a picture that took its size from the entry
/// before it would be drawn stretched into a stranger's box. An `Arc` for
/// the pixels, because the same ones are drawn on every frame the card is
/// up and copied on none of them.
#[derive(Clone, Debug)]
pub struct Picture {
    /// The pixels, held to a thousand on the long edge.
    pub bitmap: Arc<Bitmap>,
    /// What the header said, in pixels as captured — which is what decides
    /// how big it is drawn, not how many pixels are held now.
    pub width: u32,
    /// See [`width`](Self::width).
    pub height: u32,
    /// One line under it, when the module gave one.
    pub caption: Option<String>,
}

/// A plugin's face and what it looks like.
#[derive(Clone, Debug, Default)]
pub struct Pictures {
    /// Its icon, decoded, or `None` for a plugin that carries none or one
    /// Crook could not draw.
    pub icon: Option<Arc<Bitmap>>,
    /// Its previews, still encoded, in the order the module numbered them.
    pub previews: Vec<Preview>,
}

/// One preview as it travels: the PNG, the size the header says, and the
/// caption under it.
///
/// The size rides beside the bytes so a card can reserve the room before the
/// pixels are decoded, and the bytes are behind an `Arc` so the decode can
/// happen on the pool without copying a screenshot to get there.
#[derive(Clone, Debug)]
pub struct Preview {
    /// The PNG as the module carried it.
    pub png: Arc<Vec<u8>>,
    /// What the header says, in pixels as captured.
    pub width: u32,
    /// See [`width`](Self::width).
    pub height: u32,
    /// One line under it, when the module gave one.
    pub caption: Option<String>,
}

impl Pictures {
    /// What a module carried, as the host keeps it.
    ///
    /// The icon is decoded here and now. One that will not decode is a line
    /// in the log and no icon — the module has already opened, and a face
    /// Crook cannot draw is not a reason to refuse a plugin somebody chose.
    pub fn from_module(raw: crook_wasm::Pictures, id: &PluginId) -> Self {
        let icon = raw
            .icon
            .and_then(|png| match picture::decode(&png, Limits::ICON) {
                Ok(bitmap) => Some(Arc::new(bitmap)),
                Err(why) => {
                    log::warn!("{id} carries an icon Crook cannot draw: it {why}");
                    None
                }
            });
        let previews = raw
            .previews
            .into_iter()
            .map(|preview| Preview {
                png: Arc::new(preview.png),
                width: preview.width,
                height: preview.height,
                caption: preview.caption,
            })
            .collect();
        Self { icon, previews }
    }

    /// How many pictures there are to look at.
    ///
    /// The previews, not the icon: the icon is the plugin's face beside its
    /// name, and "N pictures inside" is what a card offers to open.
    pub fn count(&self) -> usize {
        self.previews.len()
    }

    /// Decodes previews for a card, each with its captured size and caption.
    ///
    /// Pool work: a preview is up to two thousand pixels a side, and six of
    /// them decoded on the thread that draws would be a frame nobody sees for
    /// a while. One that will not decode is a line in the log and a picture
    /// fewer, for the reason the icon is — and costs the others nothing,
    /// since each carries its own size.
    pub fn decode_previews(previews: &[Preview]) -> Decoded {
        previews
            .iter()
            .enumerate()
            .filter_map(
                |(index, preview)| match picture::decode_preview(&preview.png) {
                    Ok(bitmap) => Some(Picture {
                        bitmap: Arc::new(bitmap),
                        width: preview.width,
                        height: preview.height,
                        caption: preview.caption.clone(),
                    }),
                    Err(why) => {
                        log::warn!("preview {} cannot be drawn: it {why}", index + 1);
                        None
                    }
                },
            )
            .collect()
    }
}

#[cfg(test)]
#[path = "pictures_tests.rs"]
mod tests;
