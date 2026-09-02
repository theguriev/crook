//! Draws the demo scene in a real window, through the winit event loop.
//!
//! The same [`Scene`](crookui_core::scene::Scene) `scene_to_png` writes to a
//! file, on a swapchain instead of a texture, rebuilt whenever the window is
//! resized or its scale factor changes.
//!
//! ```text
//! cargo run -p crookui --example window                # until closed
//! cargo run -p crookui --example window -- --frames 3  # exits after 3 frames
//! ```
//!
//! `--frames` is what makes this runnable unattended: CI, and any check that
//! wants to know the window path still reaches `present` without a validation
//! error, cannot wait for a human to close a window.

mod common;

use std::env;
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use crookui::{CosmicFontDb, Proxy, WindowDelegate, WindowOptions};
use crookui_core::event::Event;
use crookui_core::geometry::Vector2F;
use crookui_core::scene::Scene;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let frame_budget = frame_budget()?;
    let font_db = CosmicFontDb::new().context("no usable system fonts")?;
    let demo = common::Demo::new(&font_db)?;

    let options = WindowOptions {
        title: "Crook — renderer demo".to_owned(),
        size: common::SIZE,
        ..Default::default()
    };

    crookui::run(options, Box::new(font_db), |platform| {
        Box::new(Demo {
            demo,
            proxy: platform.proxy.clone(),
            frames_drawn: 0,
            frame_budget,
        })
    })
}

/// Reads `--frames N`, or `None` for "run until the window is closed".
fn frame_budget() -> Result<Option<u32>> {
    let mut args = env::args().skip(1);
    let Some(flag) = args.next() else {
        return Ok(None);
    };
    if flag != "--frames" {
        bail!("unrecognised argument {flag}; the only one is `--frames N`");
    }

    let count = args
        .next()
        .context("`--frames` needs a count")?
        .parse()
        .context("`--frames` takes a number")?;

    Ok(Some(count))
}

struct Demo {
    demo: common::Demo,
    proxy: Proxy,
    frames_drawn: u32,
    frame_budget: Option<u32>,
}

impl WindowDelegate for Demo {
    fn build_scene(&mut self, size: Vector2F, scale_factor: f32) -> Rc<Scene> {
        Rc::new(self.demo.scene(size, scale_factor))
    }

    fn handle_event(&mut self, event: Event) -> bool {
        // The demo has no hover, focus or selection, so no input changes what
        // is on screen and nothing here ever asks for a redraw. Resizes do not
        // come through this path: the event loop reconfigures and repaints on
        // its own.
        log::debug!("{event:?}");
        false
    }

    fn frame_drawn(&mut self) {
        self.frames_drawn += 1;
        log::info!("frame {} drawn", self.frames_drawn);

        let Some(budget) = self.frame_budget else {
            return;
        };

        if self.frames_drawn >= budget {
            self.proxy.exit();
        } else {
            self.proxy.request_redraw();
        }
    }
}
