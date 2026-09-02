//! Two wrappers that make wgpu's failure modes into values.
//!
//! wgpu reports validation errors out-of-band, through an error scope, and its
//! stock `create_buffer_init` panics rather than returning when a device is
//! lost mid-map. Both are fine for a sample and wrong for an application:
//! without these, a lost GPU shows up as console spew and a black window
//! instead of as an [`Error`] the event loop can act on.
//!
//! Ported from Warp's `crates/warpui/src/rendering/wgpu/renderer/util.rs` (MIT).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wgpu::util::BufferInitDescriptor;
use wgpu::{Buffer, BufferAddress, BufferDescriptor, COPY_BUFFER_ALIGNMENT, Device};

use super::Error;

/// Runs `callback` inside a validation error scope, returning whatever it
/// produced alongside the first error wgpu recorded.
///
/// The returned error must be acted on: encoding work that failed validation
/// and then presenting anyway makes wgpu complain about presenting without
/// submitting, which buries the real error.
#[must_use]
pub fn with_error_scope<T>(device: &Device, callback: impl FnOnce() -> T) -> (T, Option<Error>) {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let value = callback();
    // On native backends this future is already resolved by the time `pop`
    // returns, so blocking on it never actually parks the thread.
    let error = pollster::block_on(scope.pop());
    (value, error.map(Into::into))
}

/// Creates a buffer and fills it with `descriptor.contents`.
///
/// This is [`wgpu::util::DeviceExt::create_buffer_init`] with two differences
/// that matter: a device lost between creation and mapping returns
/// [`Error::DeviceLost`] instead of panicking, and the size is rounded up to a
/// non-zero multiple of [`COPY_BUFFER_ALIGNMENT`], which Vulkan requires.
pub fn create_buffer_init(
    device: &Device,
    device_lost: &Arc<AtomicBool>,
    descriptor: &BufferInitDescriptor<'_>,
) -> Result<Buffer, Error> {
    if descriptor.contents.is_empty() {
        return create_buffer(
            device,
            &BufferDescriptor {
                label: descriptor.label,
                size: 0,
                usage: descriptor.usage,
                mapped_at_creation: false,
            },
        );
    }

    let unpadded_size = descriptor.contents.len() as BufferAddress;
    let align_mask = COPY_BUFFER_ALIGNMENT - 1;
    let padded_size = ((unpadded_size + align_mask) & !align_mask).max(COPY_BUFFER_ALIGNMENT);

    let buffer = create_buffer(
        device,
        &BufferDescriptor {
            label: descriptor.label,
            size: padded_size,
            usage: descriptor.usage,
            mapped_at_creation: true,
        },
    )?;

    // A device that died while the buffer was being created leaves a mapping
    // that is invalid to write through, so check before touching it.
    if device_lost.load(Ordering::SeqCst) {
        return Err(Error::DeviceLost);
    }

    let slice = buffer.slice(..);
    let mut mapping = match slice.get_mapped_range_mut() {
        Ok(mapping) => mapping,
        Err(error) => {
            log::warn!("failed to map a wgpu buffer range: {error}");
            buffer.unmap();
            return Err(error.into());
        }
    };
    mapping
        .slice(..unpadded_size as usize)
        .copy_from_slice(descriptor.contents);
    drop(mapping);
    buffer.unmap();

    Ok(buffer)
}

fn create_buffer(device: &Device, descriptor: &BufferDescriptor<'_>) -> Result<Buffer, Error> {
    match with_error_scope(device, || device.create_buffer(descriptor)) {
        (buffer, None) => Ok(buffer),
        (_, Some(error)) => {
            log::warn!("failed to create a wgpu buffer: {error}");
            Err(error)
        }
    }
}
