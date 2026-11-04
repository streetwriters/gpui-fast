//! Giving the pipelines' instance buffers back after a spike. See
//! [`buffer_shrink`](super::buffer_shrink) for the policy.
//!
//! A frame uploads to some of the buffers more than once, a batch at a time,
//! so each buffer's use is noted per upload and judged once per frame, before
//! the frame's first upload.

use anyhow::{Context as _, Result};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;

use crate::directx_renderer::{DirectXRenderer, PipelineState, create_buffer, create_buffer_view};

pub(crate) fn begin_frame(renderer: &mut DirectXRenderer) -> Result<()> {
    let device = renderer
        .devices
        .as_ref()
        .context("devices missing")?
        .device
        .clone();
    let pipelines = &mut renderer.pipelines;
    shrink(&device, &mut pipelines.shadow_pipeline)?;
    shrink(&device, &mut pipelines.quad_pipeline)?;
    shrink(&device, &mut pipelines.path_rasterization_pipeline)?;
    shrink(&device, &mut pipelines.path_sprite_pipeline)?;
    shrink(&device, &mut pipelines.underline_pipeline)?;
    shrink(&device, &mut pipelines.mono_sprites)?;
    shrink(&device, &mut pipelines.subpixel_sprites)?;
    shrink(&device, &mut pipelines.poly_sprites)
}

/// Replaces the buffer by a smaller one between two frames. The driver keeps
/// the old one until the GPU is done with it.
fn shrink<T>(device: &ID3D11Device, pipeline: &mut PipelineState<T>) -> Result<()> {
    if let Some(size) = pipeline.fast_use.end_frame(pipeline.buffer_size) {
        log::debug!(
            "Updating {} buffer size from {} to {}",
            pipeline.label,
            pipeline.buffer_size,
            size
        );
        let buffer = create_buffer(device, std::mem::size_of::<T>(), size)?;
        pipeline.view = create_buffer_view(device, &buffer)?;
        pipeline.buffer = buffer;
        pipeline.buffer_size = size;
    }
    Ok(())
}
