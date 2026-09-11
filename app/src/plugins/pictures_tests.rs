//! What the host makes of a module's pictures, and what a bad one costs.

use super::*;
use crate::picture::tests::{header_only, icon_png, preview_png};

/// A preview as the host keeps it, from `png`, captioned `caption`.
fn preview(png: Vec<u8>, width: u32, height: u32, caption: Option<&str>) -> Preview {
    Preview {
        png: Arc::new(png),
        width,
        height,
        caption: caption.map(str::to_owned),
    }
}

#[test]
fn a_preview_that_will_not_decode_costs_no_other_its_size() {
    // A header the module's reader accepts and no pixels behind it: the
    // registry's check stops at the header, so this is what a corrupt
    // picture looks like by the time it reaches the host. The picture after
    // it must still be drawn at its own captured size, with its own caption
    // — a list paired up by position would hand it the missing one's.
    let previews = [
        preview(header_only(1200, 200), 1200, 200, Some("The wide one")),
        preview(
            preview_png(400, 300),
            400,
            300,
            Some("The one that decodes"),
        ),
    ];

    let decoded = Pictures::decode_previews(&previews);

    assert_eq!(decoded.len(), 1, "the corrupt picture is one fewer");
    assert_eq!((decoded[0].width, decoded[0].height), (400, 300));
    assert_eq!(decoded[0].caption.as_deref(), Some("The one that decodes"));
    assert_eq!(decoded[0].bitmap.size(), (400, 300));
}

#[test]
fn a_module_with_an_icon_it_cannot_draw_is_a_plugin_with_no_face() {
    // The module has opened by the time its icon is decoded, and a face
    // Crook cannot draw is a line in the log, not a plugin refused.
    let id = PluginId::parse("eugen/probe").expect("a literal that parses");
    let raw = crook_wasm::Pictures {
        icon: Some(header_only(64, 64)),
        previews: vec![crook_wasm::Preview {
            png: preview_png(8, 8),
            width: 8,
            height: 8,
            caption: None,
        }],
    };

    let pictures = Pictures::from_module(raw, &id);

    assert!(pictures.icon.is_none());
    assert_eq!(pictures.count(), 1, "the previews are kept regardless");

    let drawn = Pictures::from_module(
        crook_wasm::Pictures {
            icon: Some(icon_png(64)),
            previews: Vec::new(),
        },
        &id,
    );
    assert_eq!(
        drawn.count(),
        0,
        "the icon is a face, not a picture to open"
    );
    assert_eq!(drawn.icon.map(|icon| icon.size()), Some((64, 64)));
}
