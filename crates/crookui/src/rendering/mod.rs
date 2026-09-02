//! The wgpu renderer: everything between a
//! [`Scene`](crookui_core::scene::Scene) and pixels.
//!
//! A frame is two passes over the scene. The first walks every
//! [`Layer`](crookui_core::scene::Layer) and appends instance data into one
//! `Vec` per pipeline; those become exactly two GPU buffers for the whole
//! frame. The second opens a single render pass and, per layer, sets a scissor
//! rect and issues one instanced draw per pipeline over a range of that buffer.
//! A two-hundred-layer scene therefore costs two buffer allocations, not four
//! hundred.
//!
//! Everything is an instanced unit quad. Rounded corners, gradients and borders
//! are fragment-shader SDF math over `[0,1]²`; glyphs are the same quad
//! sampling an atlas. There is no tessellation, no path rasterizer and no depth
//! buffer — z order is layer order, painter's algorithm.
//!
//! # Where things live
//!
//! * `resources` — instance, adapter, device, queue, swapchain, and the
//!   uniform and unit-quad buffers every pipeline shares.
//! * `rect` and `glyph` — the two pipelines, each owning its shader module, its
//!   instance layout and its per-frame buffer.
//! * `glyph_cache` and [`atlas`] — where a rasterized glyph goes and how it is
//!   found again.
//! * `frame` — the two passes above.
//! * [`offscreen`] — the same renderer with a texture instead of a window,
//!   which is how it is verified without a display.
//!
//! # Coordinates
//!
//! Scene coordinates are *logical*. The CPU multiplies by
//! [`Scene::scale_factor`](crookui_core::scene::Scene::scale_factor) exactly
//! once, while building instance data, so everything the GPU sees is physical
//! pixels with the origin at the top-left. `src/shaders/README.md` is the
//! binding contract for the buffers those instances land in.

mod frame;
mod glyph;
mod glyph_cache;
mod rect;
mod resources;
mod util;

pub mod atlas;
pub mod offscreen;

use std::error;
use std::fmt;

pub use frame::{Frame, Renderer};
pub use offscreen::{Offscreen, render_scene_to_rgba};
pub use resources::{
    GetSurfaceTextureError, Resources, SurfaceConfigureError, WindowResources, init_wgpu_instance,
    reset_wgpu_instance,
};
pub use util::{create_buffer_init, with_error_scope};

/// Everything that can go wrong while drawing a frame.
///
/// wgpu reports validation problems through an error scope rather than as a
/// return value, so every call site that could produce one is wrapped in
/// [`with_error_scope`] and the result lands here. That is what turns silent
/// GPU corruption into a value the event loop can branch on — specifically,
/// into [`Self::requires_renderer_recreation`].
#[derive(Debug)]
pub enum Error {
    /// The GPU device went away: a driver reset, a GPU hot-unplug, a laptop
    /// switching between its integrated and discrete adapters.
    DeviceLost,

    /// A buffer could not be mapped for writing.
    BufferMap(wgpu::MapRangeError),

    /// The next swapchain texture could not be acquired.
    Surface(GetSurfaceTextureError),

    /// The swapchain could not be configured at the requested size.
    SurfaceConfigure(SurfaceConfigureError),

    /// A wgpu validation error that is none of the above.
    Unknown(wgpu::Error),
}

impl Error {
    /// Whether recovering means throwing the device away and building a new one.
    ///
    /// A lost device is a normal event, not a crash: the window keeps its
    /// `winit` handle and gets a fresh [`WindowResources`] and [`Renderer`]
    /// built against it. Anything unrecognised is treated the same way,
    /// because a renderer in an unknown state renders nothing until it is
    /// replaced.
    pub fn requires_renderer_recreation(&self) -> bool {
        matches!(
            self,
            Self::DeviceLost
                | Self::SurfaceConfigure(_)
                | Self::Unknown(_)
                | Self::Surface(GetSurfaceTextureError::Lost)
        )
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceLost => write!(f, "the GPU device was lost"),
            Self::BufferMap(error) => write!(f, "failed to map a buffer range: {error}"),
            Self::Surface(error) => write!(f, "failed to acquire a surface texture: {error}"),
            Self::SurfaceConfigure(error) => write!(f, "{error}"),
            Self::Unknown(error) => write!(f, "{error}"),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::DeviceLost => None,
            Self::BufferMap(error) => Some(error),
            Self::Surface(error) => Some(error),
            Self::SurfaceConfigure(error) => Some(error),
            Self::Unknown(error) => Some(error),
        }
    }
}

impl From<wgpu::MapRangeError> for Error {
    fn from(error: wgpu::MapRangeError) -> Self {
        Self::BufferMap(error)
    }
}

impl From<GetSurfaceTextureError> for Error {
    fn from(error: GetSurfaceTextureError) -> Self {
        Self::Surface(error)
    }
}

impl From<SurfaceConfigureError> for Error {
    fn from(error: SurfaceConfigureError) -> Self {
        Self::SurfaceConfigure(error)
    }
}

impl From<wgpu::Error> for Error {
    fn from(error: wgpu::Error) -> Self {
        for cause in anyhow::Chain::new(&error) {
            if let Some(wgpu::wgc::device::DeviceError::Lost) =
                cause.downcast_ref::<wgpu::wgc::device::DeviceError>()
            {
                return Self::DeviceLost;
            }

            // Several of wgpu's nested device errors are `#[error(transparent)]`,
            // which forwards `source()` to a `DeviceError::Lost` that wraps
            // nothing — so the chain walk above never reaches it through a
            // `SurfaceError`. Checking that shape explicitly is what makes a
            // device lost inside `present()` recoverable.
            if let Some(wgpu::wgc::present::SurfaceError::Device(
                wgpu::wgc::device::DeviceError::Lost,
            )) = cause.downcast_ref::<wgpu::wgc::present::SurfaceError>()
            {
                return Self::DeviceLost;
            }
        }

        Self::Unknown(error)
    }
}
