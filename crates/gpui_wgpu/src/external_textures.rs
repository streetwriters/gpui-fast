//! Painting [`gpui::ExternalTexture`]s: GPU textures produced outside the
//! sprite atlas (an imported dma-buf, or a test image) that the renderer
//! samples directly, with no CPU copy.
//!
//! The renderer keeps a bind group per texture it has painted, dropped once
//! the app has released its handle, and one parameter uniform per paint in
//! the frame: the same texture can be painted more than once (a video shown
//! in two places), and every draw needs its own bounds.

use crate::fast::pass_state::PassState;
use crate::wgpu_renderer::{SurfaceParams, WgpuBindGroupLayouts, WgpuPipelines};
use collections::FxHashMap;
use gpui::{ExternalTexture, ExternalTextureId, PaintExternalTexture, Size};
use std::any::Any;
use std::ops::Range;
use std::sync::Arc;

/// A standalone GPU texture produced outside the sprite atlas, wrapped in a
/// [`gpui::ExternalTexture`] so the renderer samples it directly. Produced by
/// [`external_texture_from_rgba`] and by dma-buf imports.
pub struct SurfaceTexture {
    /// The texture itself. Its format matches the bytes that were uploaded:
    /// `Bgra8Unorm` for [`external_texture_from_rgba`] (the surface target and
    /// the sprite atlas are non-sRGB too, so values pass through unchanged).
    pub texture: wgpu::Texture,
    /// `(width, height)` in pixels.
    pub size: (u32, u32),
}

/// Uploads `bgra` pixels (`size.0 * size.1 * 4` bytes, BGRA order — the same
/// layout as WebKit's ARGB/XRGB buffers and GPUI's `RenderImage`) into a
/// standalone GPU texture wrapped in a [`gpui::ExternalTexture`].
///
/// This is the verification affordance for the external-texture paint path: a
/// synthetic image exercises exactly the sampling and compositing code that an
/// imported dma-buf uses, on machines whose driver cannot import one.
///
/// The texture is `Bgra8Unorm`, not `Bgra8UnormSrgb`: the WGPU renderer picks a
/// non-sRGB surface and uploads its sprite atlas as `Bgra8Unorm`, so the whole
/// pipeline carries sRGB-encoded values unmodified. An sRGB texture here would
/// be linearized on sample and come out darker than everything next to it.
pub fn external_texture_from_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: (u32, u32),
    bgra: &[u8],
) -> ExternalTexture {
    let (width, height) = (size.0.max(1), size.1.max(1));
    assert_eq!(
        bgra.len(),
        width as usize * height as usize * 4,
        "external texture has {width}x{height} pixels but {} bytes of data",
        bgra.len(),
    );

    let extent = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("external_texture"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bgra,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        extent,
    );

    ExternalTexture::new(
        Size { width, height },
        Arc::new(SurfaceTexture { texture, size }),
    )
}

/// The renderer's state for painting external textures, kept across frames.
#[derive(Default)]
pub(crate) struct ExternalTextures {
    entries: FxHashMap<ExternalTextureId, Entry>,
    /// One per external-texture paint of the frame being drawn, at the
    /// paint's index in `Scene::external_textures`; recycled across frames.
    slots: Vec<ParamSlot>,
}

/// The bind group that samples one [`SurfaceTexture`], and the payload `Arc`
/// used to drop it once the app has released its handle.
struct Entry {
    payload: Arc<dyn Any + Send + Sync>,
    texture: wgpu::BindGroup,
}

/// One `SurfaceParams` uniform and the bind group that exposes it at group 1.
struct ParamSlot {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl ExternalTextures {
    /// Prepares every paint in `paints` (a finished scene's
    /// `external_textures`) to be drawn by [`Self::draw`]: creates the bind
    /// groups of textures painted for the first time, writes each paint's
    /// parameters, and drops the textures the app no longer holds.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layouts: &WgpuBindGroupLayouts,
        sampler: &wgpu::Sampler,
        paints: &[PaintExternalTexture],
    ) {
        self.entries
            .retain(|_, entry| Arc::strong_count(&entry.payload) > 1);
        for paint in paints {
            self.ensure_entry(device, layouts, sampler, &paint.texture);
        }
        while self.slots.len() < paints.len() {
            self.slots.push(ParamSlot::new(device, layouts));
        }
        for (paint, slot) in paints.iter().zip(&self.slots) {
            let params = SurfaceParams {
                bounds: paint.bounds.into(),
                content_mask: paint.content_mask.bounds.into(),
            };
            queue.write_buffer(&slot.buffer, 0, bytemuck::bytes_of(&params));
        }
    }

    /// Creates the bind group of `texture` if it has none yet. Handles whose
    /// payload is not a [`SurfaceTexture`] (another renderer's) are skipped.
    fn ensure_entry(
        &mut self,
        device: &wgpu::Device,
        layouts: &WgpuBindGroupLayouts,
        sampler: &wgpu::Sampler,
        texture: &ExternalTexture,
    ) {
        if self.entries.contains_key(&texture.id()) {
            return;
        }
        let Some(surface) = texture.downcast::<SurfaceTexture>() else {
            return;
        };
        let view = surface
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("external_texture_bind_group"),
            layout: &layouts.texture,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        self.entries.insert(
            texture.id(),
            Entry {
                payload: texture.payload().clone(),
                texture: bind_group,
            },
        );
    }

    /// Draws the paints at `range` of the scene last passed to
    /// [`Self::prepare`].
    pub(crate) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        bound: &mut PassState,
        pipelines: &WgpuPipelines,
        globals: &wgpu::BindGroup,
        paints: &[PaintExternalTexture],
        range: Range<usize>,
    ) {
        for index in range {
            let (Some(paint), Some(slot)) = (paints.get(index), self.slots.get(index)) else {
                continue;
            };
            let Some(entry) = self.entries.get(&paint.texture.id()) else {
                continue;
            };
            bound.set_pipeline(pass, &pipelines.surfaces_rgba);
            bound.set_bind_group(pass, 0, globals);
            bound.set_bind_group(pass, 1, &slot.bind_group);
            bound.set_bind_group(pass, 2, &entry.texture);
            pass.draw(0..4, 0..1);
        }
    }
}

impl ParamSlot {
    fn new(device: &wgpu::Device, layouts: &WgpuBindGroupLayouts) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("external_texture_params"),
            size: size_of::<SurfaceParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("external_texture_params_bind_group"),
            layout: &layouts.surface_rgba,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self { buffer, bind_group }
    }
}
