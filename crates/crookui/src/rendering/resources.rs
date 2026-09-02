//! The GPU objects a renderer is built on, and the swapchain it presents to.
//!
//! Split in two because the renderer has two callers. [`Resources`] is the
//! surfaceless half — adapter, device, queue, the uniform buffer and the shared
//! unit quad — and is all
//! [`offscreen`](super::offscreen) needs. [`WindowResources`] adds the
//! swapchain, and owns the two policies that make a window survive contact with
//! a real compositor: how the surface is configured, and what to do when
//! acquiring its next texture fails.
//!
//! Distilled from Warp's `crates/warpui/src/rendering/wgpu/resources.rs` (MIT).
//! Warp's 985 lines are mostly adapter sorting and per-driver blocklists
//! accumulated from shipping to millions of Linux desktops; none of that is
//! carried here. What is carried verbatim is the part that is not folklore: the
//! four surface-configuration decisions, the downlevel limits, and the
//! acquire-retry policy.

use std::cell::RefCell;
use std::error;
use std::fmt;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use crookui_core::geometry::Vector2F;
use parking_lot::Mutex;
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::wgt::WgpuHasDisplayHandle;

/// The process-wide [`wgpu::Instance`].
///
/// wgpu forbids creating a surface from a display handle other than the one its
/// instance was built with, so there can only be one instance per process and
/// it has to exist before the first window does.
static WGPU_INSTANCE: Mutex<Option<Arc<wgpu::Instance>>> = Mutex::new(None);

/// Builds the process-wide instance against `display`.
///
/// Call this once, from the event loop, before opening a window — winit's
/// `owned_display_handle()` is the handle to pass. Calling it again replaces
/// the instance, which is what device-lost recovery does: some NVIDIA drivers
/// deadlock when resources are rebuilt on the instance that lost them.
pub fn init_wgpu_instance(display: Box<dyn WgpuHasDisplayHandle>) {
    *WGPU_INSTANCE.lock() = Some(create_instance(Some(display)));
}

/// Drops the process-wide instance and builds a fresh one against `display`.
pub fn reset_wgpu_instance(display: Box<dyn WgpuHasDisplayHandle>) {
    // The lock is not reentrant, so the old instance has to be released before
    // `init_wgpu_instance` takes it again.
    WGPU_INSTANCE.lock().take();
    init_wgpu_instance(display);
}

/// The process-wide instance, built without a display handle if none exists.
///
/// A display-less instance can still enumerate adapters and open a device —
/// everything [`Resources::new`] needs. It just cannot make a surface, which is
/// why the window path calls [`init_wgpu_instance`] first.
fn instance() -> Arc<wgpu::Instance> {
    WGPU_INSTANCE
        .lock()
        .get_or_insert_with(|| create_instance(None))
        .clone()
}

fn create_instance(display: Option<Box<dyn WgpuHasDisplayHandle>>) -> Arc<wgpu::Instance> {
    Arc::new(wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::all()),
        flags: wgpu::InstanceFlags::empty(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions {
            dx12: wgpu::Dx12BackendOptions {
                // Fxc is part of Windows. DynamicDxc would mean shipping
                // dxcompiler.dll and dxil.dll next to the binary, and Crook
                // ships one file with no build script and no side-car DLLs.
                shader_compiler: wgpu::Dx12Compiler::Fxc,
                ..Default::default()
            },
            ..Default::default()
        },
        display,
    }))
}

/// A device, its queue, and the buffers every pipeline binds.
pub struct Resources {
    /// The open logical device.
    pub device: wgpu::Device,

    /// Its command queue.
    pub queue: wgpu::Queue,

    /// The physical adapter the device was opened on.
    pub adapter: wgpu::Adapter,

    /// Set by wgpu's device-lost callback.
    ///
    /// Losing a device is asynchronous, so a buffer can be created against a
    /// device that has already died; [`create_buffer_init`](super::create_buffer_init)
    /// checks this flag before writing through the mapping it just got.
    pub device_lost: Arc<AtomicBool>,

    uniforms: Uniforms,
    quad: Quad,
}

impl Resources {
    /// Opens a device on an adapter that can present to `compatible_surface`.
    ///
    /// Pass `None` when rendering to a texture: any adapter will do, and
    /// constraining the choice would fail on a machine with no display.
    pub fn new(compatible_surface: Option<&wgpu::Surface<'static>>) -> Result<Self> {
        let instance = instance();

        pollster::block_on(async {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface,
                    apply_limit_buckets: false,
                })
                .await
                .context("no usable GPU adapter was found")?;

            let info = adapter.get_info();
            log::info!(
                "rendering with {:?} {:?} ({})",
                info.backend,
                info.device_type,
                info.name
            );

            // Downlevel limits everywhere except texture size, which is raised
            // to whatever this adapter can do because real displays exceed the
            // 2048px downlevel default. Staying downlevel otherwise is what
            // keeps one set of WGSL running on Metal, Vulkan, D3D12 and GL.
            // Both shaders are written to fit inside these; see
            // `src/shaders/README.md` for the per-limit budget.
            let limits =
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());

            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("Crook device"),
                    required_limits: limits,
                    ..Default::default()
                })
                .await
                .context("failed to open a logical device")?;

            let device_lost = Arc::new(AtomicBool::new(false));
            let flag = device_lost.clone();
            device.set_device_lost_callback(move |reason, message| {
                flag.store(true, Ordering::SeqCst);
                log::warn!("the GPU device was lost ({reason:?}): {message}");
            });

            let uniforms = Uniforms::new(&device);
            let quad = Quad::new(&device);

            Ok(Self {
                device,
                queue,
                adapter,
                device_lost,
                uniforms,
                quad,
            })
        })
    }

    /// The layout of bind group 0, which both pipelines declare.
    pub fn uniform_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.uniforms.bind_group_layout
    }

    /// Binds group 0 and the shared unit quad, and points the shaders at a
    /// target of `drawable_size` physical pixels.
    ///
    /// Called once per render pass, before any pipeline is set: both vertex
    /// shaders divide by this viewport size to reach normalized device
    /// coordinates, so it must be the size of the texture being drawn into,
    /// never the window's logical size.
    pub fn configure_render_pass(
        &self,
        render_pass: &mut wgpu::RenderPass<'_>,
        drawable_size: Vector2F,
    ) {
        self.queue.write_buffer(
            &self.uniforms.buffer,
            0,
            bytemuck::cast_slice(&[ShaderUniforms::new(drawable_size)]),
        );
        render_pass.set_bind_group(0, &self.uniforms.bind_group, &[]);
        render_pass.set_vertex_buffer(QUAD_VERTEX_SLOT, self.quad.vertices.slice(..));
        render_pass.set_index_buffer(self.quad.indices.slice(..), wgpu::IndexFormat::Uint16);
    }
}

/// A window's swapchain, and the device chosen to drive it.
pub struct WindowResources {
    /// The device this surface presents from.
    pub resources: Resources,

    surface: wgpu::Surface<'static>,
    config: RefCell<wgpu::SurfaceConfiguration>,
}

impl WindowResources {
    /// Creates a surface for `window`, picks an adapter that can present to it,
    /// and configures the swapchain at `initial_size` physical pixels.
    ///
    /// `initial_size` must be non-zero on both axes; a zero-sized swapchain is
    /// a validation error on every backend.
    pub fn new(
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        initial_size: Vector2F,
    ) -> Result<Self> {
        let surface = instance()
            .create_surface(window)
            .context("failed to create a surface for the window")?;
        let resources = Resources::new(Some(&surface))?;

        let config = create_surface_config(&resources.adapter, &surface, initial_size)
            .ok_or_else(|| anyhow!("the selected adapter cannot present to this window"))?;
        pollster::block_on(configure_surface(&surface, &resources.device, &config))?;

        Ok(Self {
            resources,
            surface,
            config: RefCell::new(config),
        })
    }

    /// The texture format the swapchain was configured with, which is what the
    /// pipelines must declare as their color target.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.borrow().format
    }

    /// The swapchain's size in physical pixels.
    pub fn size(&self) -> Vector2F {
        let config = self.config.borrow();
        Vector2F::new(config.width as f32, config.height as f32)
    }

    /// Reconfigures the swapchain at `size` physical pixels.
    ///
    /// A zero-sized request is ignored rather than rejected: a minimized window
    /// reports zero on some platforms and is expected to come back.
    pub fn update_size(&self, size: Vector2F) -> Result<(), SurfaceConfigureError> {
        if size.x() <= 0. || size.y() <= 0. {
            return Ok(());
        }

        let mut config = self.config.borrow_mut();
        config.width = size.x() as u32;
        config.height = size.y() as u32;
        pollster::block_on(configure_surface(
            &self.surface,
            &self.resources.device,
            &config,
        ))
    }

    /// Acquires the next texture to draw into.
    ///
    /// Failure here is routine, not exceptional — a window gets occluded by
    /// Mission Control, a resize races the compositor — so the two outcomes are
    /// treated differently. `Timeout`, `Validation` and `Occluded` mean skip
    /// this frame and try again on the next one. `Lost` and `Outdated` mean the
    /// swapchain itself is stale: reconfigure it and retry exactly once, so a
    /// persistently broken surface reports an error instead of spinning.
    pub fn get_surface_texture(&self) -> Result<wgpu::SurfaceTexture, GetSurfaceTextureError> {
        let error = match next_texture(&self.surface) {
            Ok(texture) => return Ok(texture),
            Err(error) => error,
        };

        log::warn!("failed to acquire the next swapchain texture: {error}");
        match error {
            GetSurfaceTextureError::Timeout
            | GetSurfaceTextureError::Validation
            | GetSurfaceTextureError::Occluded
            | GetSurfaceTextureError::Configuration(_) => Err(error),

            GetSurfaceTextureError::Lost | GetSurfaceTextureError::Outdated => {
                pollster::block_on(configure_surface(
                    &self.surface,
                    &self.resources.device,
                    &self.config.borrow(),
                ))
                .map_err(GetSurfaceTextureError::Configuration)?;

                next_texture(&self.surface).inspect_err(|error| {
                    log::warn!("failed to recreate the swapchain: {error}");
                })
            }
        }
    }
}

/// Why acquiring the next swapchain texture failed.
#[derive(Debug)]
pub enum GetSurfaceTextureError {
    /// The compositor did not hand one over in time.
    Timeout,
    /// The window is not visible, so there is nothing to present to.
    Occluded,
    /// The swapchain no longer matches the surface, usually after a resize.
    Outdated,
    /// The swapchain is gone, usually because the device is.
    Lost,
    /// wgpu rejected the request.
    Validation,
    /// Reconfiguring the swapchain after `Lost` or `Outdated` failed too.
    Configuration(SurfaceConfigureError),
}

impl fmt::Display for GetSurfaceTextureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(f, "timed out waiting for the next surface texture"),
            Self::Occluded => write!(f, "the window is occluded and cannot be presented to"),
            Self::Outdated => write!(f, "the surface configuration is outdated"),
            Self::Lost => write!(f, "the surface was lost"),
            Self::Validation => write!(f, "the surface rejected the request"),
            Self::Configuration(error) => write!(f, "{error}"),
        }
    }
}

impl error::Error for GetSurfaceTextureError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            _ => None,
        }
    }
}

/// A swapchain that could not be configured with the requested settings.
#[derive(Debug)]
pub struct SurfaceConfigureError {
    /// What wgpu objected to.
    pub source: wgpu::Error,
    /// The configuration that was rejected.
    pub config: wgpu::SurfaceConfiguration,
}

impl fmt::Display for SurfaceConfigureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to configure the surface: {}\n\nrequested configuration: {:#?}",
            self.source, self.config
        )
    }
}

impl error::Error for SurfaceConfigureError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(&self.source)
    }
}

fn next_texture(
    surface: &wgpu::Surface<'_>,
) -> Result<wgpu::SurfaceTexture, GetSurfaceTextureError> {
    match surface.get_current_texture() {
        // Suboptimal still presents; it just means the compositor would prefer
        // a different configuration, which the next resize will give it.
        wgpu::CurrentSurfaceTexture::Success(texture)
        | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Ok(texture),
        wgpu::CurrentSurfaceTexture::Timeout => Err(GetSurfaceTextureError::Timeout),
        wgpu::CurrentSurfaceTexture::Occluded => Err(GetSurfaceTextureError::Occluded),
        wgpu::CurrentSurfaceTexture::Outdated => Err(GetSurfaceTextureError::Outdated),
        wgpu::CurrentSurfaceTexture::Lost => Err(GetSurfaceTextureError::Lost),
        wgpu::CurrentSurfaceTexture::Validation => Err(GetSurfaceTextureError::Validation),
    }
}

/// The four decisions that make a window look right, kept verbatim from Warp.
fn create_surface_config(
    adapter: &wgpu::Adapter,
    surface: &wgpu::Surface<'_>,
    size: Vector2F,
) -> Option<wgpu::SurfaceConfiguration> {
    let mut config = surface.get_default_config(adapter, size.x() as u32, size.y() as u32)?;
    let caps = surface.get_capabilities(adapter);

    // Blend in the same space the colors were authored in. An sRGB surface
    // format would have the GPU encode on write, which makes every
    // antialiased edge and every semi-transparent fill land in the wrong place.
    config.format = config.format.remove_srgb_suffix();

    // A terminal is judged on keystroke-to-pixel latency, so tear rather than
    // wait for the next vblank.
    config.present_mode = wgpu::PresentMode::AutoNoVsync;

    // Lets a frame be copied back out — the window-capture path, and the same
    // usage `offscreen` puts on its own texture.
    if caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
        config.usage |= wgpu::TextureUsages::COPY_SRC;
    }

    // Most permissive alpha mode first. PostMultiplied is what makes a
    // transparent window work under native Wayland; D3D12 advertises it but
    // composites it wrong, so it is skipped there.
    config.alpha_mode = if caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::PostMultiplied)
        && adapter.get_info().backend != wgpu::Backend::Dx12
    {
        wgpu::CompositeAlphaMode::PostMultiplied
    } else if caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
    {
        wgpu::CompositeAlphaMode::PreMultiplied
    } else if caps
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::Inherit)
    {
        wgpu::CompositeAlphaMode::Inherit
    } else {
        wgpu::CompositeAlphaMode::Auto
    };

    Some(config)
}

async fn configure_surface(
    surface: &wgpu::Surface<'_>,
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> Result<(), SurfaceConfigureError> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    surface.configure(device, config);
    match scope.pop().await {
        Some(source) => Err(SurfaceConfigureError {
            source,
            config: config.clone(),
        }),
        None => Ok(()),
    }
}

/// The vertex buffer slot the shared quad is bound to. Slot 1 is per-instance
/// data, and belongs to whichever pipeline is drawing.
const QUAD_VERTEX_SLOT: u32 = 0;

/// Two triangles over the unit quad's four corners.
pub(super) const QUAD_INDICES: &[u16] = &[0, 1, 2, 2, 3, 1];

/// The unit quad, in `[0,1]²`. Not normalized device coordinates: both vertex
/// shaders map these corners into an instance's `bounds` rect.
const QUAD_VERTICES: &[Vertex] = &[
    Vertex { position: [0., 0.] },
    Vertex { position: [1., 0.] },
    Vertex { position: [0., 1.] },
    Vertex { position: [1., 1.] },
];

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
}

impl Vertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];
}

/// The layout of vertex slot 0, which every pipeline declares first.
pub(super) fn quad_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &Vertex::ATTRIBUTES,
    }
}

struct Quad {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
}

impl Quad {
    fn new(device: &wgpu::Device) -> Self {
        Self {
            vertices: device.create_buffer_init(&BufferInitDescriptor {
                label: Some("Unit quad vertices"),
                contents: bytemuck::cast_slice(QUAD_VERTICES),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            indices: device.create_buffer_init(&BufferInitDescriptor {
                label: Some("Unit quad indices"),
                contents: bytemuck::cast_slice(QUAD_INDICES),
                usage: wgpu::BufferUsages::INDEX,
            }),
        }
    }
}

/// The single uniform both shaders read, at group 0 binding 0.
///
/// GLES and WebGL require a uniform buffer binding to be a multiple of 16
/// bytes, and WGSL lays the matching struct out the same way, so the padding is
/// part of the wire format rather than an artifact of this struct.
#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ShaderUniforms {
    viewport_size: [f32; 2],
    padding: [f32; 2],
}

impl ShaderUniforms {
    fn new(viewport_size: Vector2F) -> Self {
        Self {
            viewport_size: [viewport_size.x(), viewport_size.y()],
            padding: [0., 0.],
        }
    }
}

struct Uniforms {
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
}

impl Uniforms {
    fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Uniform bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // Only the vertex stage reads it, in both shaders.
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        mem::size_of::<ShaderUniforms>() as wgpu::BufferAddress
                    ),
                },
                count: None,
            }],
        });

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Uniform buffer"),
            size: mem::size_of::<ShaderUniforms>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Uniform bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        Self {
            bind_group_layout,
            bind_group,
            buffer,
        }
    }
}
