//! The Metal state every window on a device shares: the compiled pipelines,
//! the command queue and the sprite atlas. A window's own state (drawable,
//! path textures, tile cache, instance buffers) stays in its renderer.
//!
//! Windows of one device draw from one atlas, so the glyphs and images they
//! have in common are rasterized and uploaded once, and the pipeline states
//! are compiled once. The pipelines and queue are kept for the thread's life;
//! the atlas, which holds textures, only while a window uses it.

use std::{
    cell::RefCell,
    mem, ptr,
    sync::{Arc, Weak},
};

use anyhow::{Context as _, Result};
use gpui::point;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::{
    MTLBuffer, MTLCommandQueue, MTLDevice, MTLGPUFamily, MTLPixelFormat, MTLRenderPipelineState,
    MTLResourceOptions,
};

#[cfg(not(feature = "runtime_shaders"))]
use crate::metal_renderer::SHADERS_METALLIB;
#[cfg(feature = "runtime_shaders")]
use crate::metal_renderer::SHADERS_SOURCE_FILE;
use crate::{
    metal_atlas::MetalAtlas,
    metal_renderer::{
        PATH_SAMPLE_COUNT, PointF, build_path_rasterization_pipeline_state,
        build_path_sprite_pipeline_state, build_pipeline_state,
    },
};

type PipelineState = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

/// What a renderer takes from its device's shared state.
pub(crate) struct Shared {
    pub(crate) unit_vertices: Retained<ProtocolObject<dyn MTLBuffer>>,
    pub(crate) paths_rasterization_pipeline_state: PipelineState,
    pub(crate) path_sprites_pipeline_state: PipelineState,
    pub(crate) shadows_pipeline_state: PipelineState,
    pub(crate) quads_pipeline_state: PipelineState,
    pub(crate) underlines_pipeline_state: PipelineState,
    pub(crate) monochrome_sprites_pipeline_state: PipelineState,
    pub(crate) polychrome_sprites_pipeline_state: PipelineState,
    pub(crate) surfaces_pipeline_state: PipelineState,
    pub(crate) command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pub(crate) sprite_atlas: Arc<MetalAtlas>,
}

/// A device's shared state but for the atlas, kept as long as the thread.
struct Gpu {
    unit_vertices: Retained<ProtocolObject<dyn MTLBuffer>>,
    paths_rasterization_pipeline_state: PipelineState,
    path_sprites_pipeline_state: PipelineState,
    shadows_pipeline_state: PipelineState,
    quads_pipeline_state: PipelineState,
    underlines_pipeline_state: PipelineState,
    monochrome_sprites_pipeline_state: PipelineState,
    polychrome_sprites_pipeline_state: PipelineState,
    surfaces_pipeline_state: PipelineState,
    command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
}

impl Gpu {
    fn with_atlas(&self, sprite_atlas: Arc<MetalAtlas>) -> Shared {
        Shared {
            unit_vertices: self.unit_vertices.clone(),
            paths_rasterization_pipeline_state: self.paths_rasterization_pipeline_state.clone(),
            path_sprites_pipeline_state: self.path_sprites_pipeline_state.clone(),
            shadows_pipeline_state: self.shadows_pipeline_state.clone(),
            quads_pipeline_state: self.quads_pipeline_state.clone(),
            underlines_pipeline_state: self.underlines_pipeline_state.clone(),
            monochrome_sprites_pipeline_state: self.monochrome_sprites_pipeline_state.clone(),
            polychrome_sprites_pipeline_state: self.polychrome_sprites_pipeline_state.clone(),
            surfaces_pipeline_state: self.surfaces_pipeline_state.clone(),
            command_queue: self.command_queue.clone(),
            sprite_atlas,
        }
    }
}

struct Entry {
    registry_id: u64,
    gpu: Gpu,
    atlas: Weak<MetalAtlas>,
}

thread_local! {
    static DEVICES: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// The state shared by the windows drawing on `device`.
pub(crate) fn get(device: &Retained<ProtocolObject<dyn MTLDevice>>) -> Result<Shared> {
    let registry_id = device.registryID();
    DEVICES.with(|devices| {
        let mut devices = devices.borrow_mut();
        if let Some(entry) = devices
            .iter_mut()
            .find(|entry| entry.registry_id == registry_id)
        {
            let atlas = entry.atlas.upgrade().unwrap_or_else(|| {
                let atlas = new_atlas(device);
                entry.atlas = Arc::downgrade(&atlas);
                atlas
            });
            return Ok(entry.gpu.with_atlas(atlas));
        }
        let (gpu, atlas) = build(device)?;
        let shared = gpu.with_atlas(atlas.clone());
        devices.push(Entry {
            registry_id,
            atlas: Arc::downgrade(&atlas),
            gpu,
        });
        Ok(shared)
    })
}

fn supports_shared_storage(device: &ProtocolObject<dyn MTLDevice>) -> bool {
    cfg!(target_os = "ios") || device.supportsFamily(MTLGPUFamily::Apple1)
}

fn new_atlas(device: &Retained<ProtocolObject<dyn MTLDevice>>) -> Arc<MetalAtlas> {
    #[expect(
        clippy::arc_with_non_send_sync,
        reason = "PlatformWindow::sprite_atlas requires Arc, not Send or Sync"
    )]
    Arc::new(MetalAtlas::new(
        device.clone(),
        supports_shared_storage(device),
    ))
}

fn build(device: &Retained<ProtocolObject<dyn MTLDevice>>) -> Result<(Gpu, Arc<MetalAtlas>)> {
    let device = device.clone();
    #[cfg(feature = "runtime_shaders")]
    let library = device
        .newLibraryWithSource_options_error(
            &NSString::from_str(SHADERS_SOURCE_FILE),
            Some(&objc2_metal::MTLCompileOptions::new()),
        )
        .context("building Metal library")?;
    #[cfg(not(feature = "runtime_shaders"))]
    let library = device
        .newLibraryWithData_error(&dispatch2::DispatchData::from_static_bytes(
            SHADERS_METALLIB,
        ))
        .context("loading Metal library")?;

    fn to_float2_bits(point: PointF) -> u64 {
        let mut output = point.y.to_bits() as u64;
        output <<= 32;
        output |= point.x.to_bits() as u64;
        output
    }

    // Shared memory can be used only if CPU and GPU share the same memory space.
    // https://developer.apple.com/documentation/metal/setting-resource-storage-modes
    // iOS does not support managed resources. Its simulator may report a
    // non-unified host GPU even though resources must still use shared storage.
    let is_unified_memory = cfg!(target_os = "ios") || device.hasUnifiedMemory();
    // Apple GPU families support memoryless textures, which can significantly reduce
    // memory usage by keeping render targets in on-chip tile memory instead of
    // allocating backing store in system memory.
    // https://developer.apple.com/documentation/metal/mtlgpufamily
    let _is_apple_gpu = device.supportsFamily(MTLGPUFamily::Apple1);

    let unit_vertices = [
        to_float2_bits(point(0., 0.)),
        to_float2_bits(point(1., 0.)),
        to_float2_bits(point(0., 1.)),
        to_float2_bits(point(0., 1.)),
        to_float2_bits(point(1., 0.)),
        to_float2_bits(point(1., 1.)),
    ];
    // SAFETY: Metal copies the live array using its exact byte length.
    let unit_vertices = unsafe {
        device.newBufferWithBytes_length_options(
            ptr::NonNull::from(&unit_vertices).cast(),
            mem::size_of_val(&unit_vertices),
            if is_unified_memory {
                MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined
            } else {
                MTLResourceOptions::StorageModeManaged
            },
        )
    }
    .context("allocating unit vertex buffer")?;

    let paths_rasterization_pipeline_state = build_path_rasterization_pipeline_state(
        &device,
        &library,
        "paths_rasterization",
        "path_rasterization_vertex",
        "path_rasterization_fragment",
        MTLPixelFormat::BGRA8Unorm,
        PATH_SAMPLE_COUNT,
    )?;
    let path_sprites_pipeline_state = build_path_sprite_pipeline_state(
        &device,
        &library,
        "path_sprites",
        "path_sprite_vertex",
        "path_sprite_fragment",
        MTLPixelFormat::BGRA8Unorm,
    )?;
    let shadows_pipeline_state = build_pipeline_state(
        &device,
        &library,
        "shadows",
        "shadow_vertex",
        "shadow_fragment",
        MTLPixelFormat::BGRA8Unorm,
    )?;
    let quads_pipeline_state = build_pipeline_state(
        &device,
        &library,
        "quads",
        "quad_vertex",
        "quad_fragment",
        MTLPixelFormat::BGRA8Unorm,
    )?;
    let underlines_pipeline_state = build_pipeline_state(
        &device,
        &library,
        "underlines",
        "underline_vertex",
        "underline_fragment",
        MTLPixelFormat::BGRA8Unorm,
    )?;
    let monochrome_sprites_pipeline_state = build_pipeline_state(
        &device,
        &library,
        "monochrome_sprites",
        "monochrome_sprite_vertex",
        "monochrome_sprite_fragment",
        MTLPixelFormat::BGRA8Unorm,
    )?;
    let polychrome_sprites_pipeline_state = build_pipeline_state(
        &device,
        &library,
        "polychrome_sprites",
        "polychrome_sprite_vertex",
        "polychrome_sprite_fragment",
        MTLPixelFormat::BGRA8Unorm,
    )?;
    let surfaces_pipeline_state = build_pipeline_state(
        &device,
        &library,
        "surfaces",
        "surface_vertex",
        "surface_fragment",
        MTLPixelFormat::BGRA8Unorm,
    )?;

    let command_queue = crate::fast::dynamic_textures::command_queue(&device)?;
    Ok((
        Gpu {
            unit_vertices,
            paths_rasterization_pipeline_state,
            path_sprites_pipeline_state,
            shadows_pipeline_state,
            quads_pipeline_state,
            underlines_pipeline_state,
            monochrome_sprites_pipeline_state,
            polychrome_sprites_pipeline_state,
            surfaces_pipeline_state,
            command_queue,
        },
        new_atlas(&device),
    ))
}

#[cfg(test)]
mod tests {
    use crate::metal_renderer::MetalRenderer;

    #[test]
    fn renderers_on_a_device_share_its_atlas_and_pipelines() {
        let first = MetalRenderer::new_headless(Default::default()).unwrap();
        let second = MetalRenderer::new_headless(Default::default()).unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &first.sprite_atlas,
            &second.sprite_atlas
        ));
        assert!(std::ptr::eq(&*first.command_queue, &*second.command_queue));
        assert!(std::ptr::eq(
            &*first.polychrome_sprites_pipeline_state,
            &*second.polychrome_sprites_pipeline_state
        ));
    }

    #[test]
    fn the_atlas_is_made_anew_once_no_renderer_holds_it() {
        let first = MetalRenderer::new_headless(Default::default()).unwrap();
        let old = std::sync::Arc::downgrade(&first.sprite_atlas);
        let pipelines = &*first.polychrome_sprites_pipeline_state as *const _;
        drop(first);
        assert!(old.upgrade().is_none());
        let second = MetalRenderer::new_headless(Default::default()).unwrap();
        assert!(old.upgrade().is_none());
        assert!(std::ptr::eq(
            &*second.polychrome_sprites_pipeline_state,
            pipelines
        ));
    }
}
