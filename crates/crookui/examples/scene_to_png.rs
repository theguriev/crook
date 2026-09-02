//! Renders the demo scene headlessly and writes it to a PNG.
//!
//! This is the whole renderer without a window: no event loop, no swapchain, no
//! display. Run it to look at a frame, or to check on a machine with no monitor
//! that the GPU path works at all.
//!
//! ```text
//! cargo run -p crookui --example scene_to_png -- /tmp/crook-scene.png
//! ```

mod common;

use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use anyhow::{Context, Result};
use crookui::{CosmicFontDb, render_scene_to_rgba};

/// Physical pixels per logical pixel. Two, because that is where subpixel
/// positioning and the glyph atlas are actually exercised.
const SCALE_FACTOR: f32 = 2.;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let path = env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("/tmp/crook-scene.png"), PathBuf::from);

    let font_db = CosmicFontDb::new().context("no usable system fonts")?;
    let demo = common::Demo::new(&font_db)?;
    let scene = demo.scene(common::SIZE, SCALE_FACTOR);

    let (pixels, width, height) = render_scene_to_rgba(&scene, common::SIZE, &font_db)
        .context("failed to render the scene")?;

    let file =
        File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .context("failed to write the PNG header")?
        .write_image_data(&pixels)
        .context("failed to write the PNG body")?;

    println!("wrote {} ({width}x{height})", path.display());
    Ok(())
}
