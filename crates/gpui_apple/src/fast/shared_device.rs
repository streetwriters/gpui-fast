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
    sync::{Arc, Weak},
};

use anyhow::Result;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::{MTLBuffer, MTLDevice, MTLGPUFamily, MTLRenderPipelineState};

use crate::metal_atlas::MetalAtlas;

type PipelineState = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

/// The compiled state every window of a device draws with.
#[derive(Clone)]
pub(crate) struct Gpu {
    pub(crate) unit_vertices: Retained<ProtocolObject<dyn MTLBuffer>>,
    pub(crate) paths_rasterization_pipeline_state: PipelineState,
    pub(crate) path_sprites_pipeline_state: PipelineState,
    pub(crate) shadows_pipeline_state: PipelineState,
    pub(crate) quads_pipeline_state: PipelineState,
    pub(crate) underlines_pipeline_state: PipelineState,
    pub(crate) monochrome_sprites_pipeline_state: PipelineState,
    pub(crate) polychrome_sprites_pipeline_state: PipelineState,
    pub(crate) surfaces_pipeline_state: PipelineState,
}

struct Entry {
    registry_id: u64,
    gpu: Option<Gpu>,
    atlas: Weak<MetalAtlas>,
}

thread_local! {
    static DEVICES: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// The entry of `device`, made if it has none.
fn with_entry<R>(device: &ProtocolObject<dyn MTLDevice>, f: impl FnOnce(&mut Entry) -> R) -> R {
    let registry_id = device.registryID();
    DEVICES.with(|devices| {
        let mut devices = devices.borrow_mut();
        let index = devices
            .iter()
            .position(|entry| entry.registry_id == registry_id)
            .unwrap_or_else(|| {
                devices.push(Entry {
                    registry_id,
                    gpu: None,
                    atlas: Weak::new(),
                });
                devices.len() - 1
            });
        f(&mut devices[index])
    })
}

/// The pipelines of `device`, built by `build` for the first window on it.
pub(crate) fn gpu(
    device: &ProtocolObject<dyn MTLDevice>,
    build: impl FnOnce() -> Result<Gpu>,
) -> Result<Gpu> {
    if let Some(gpu) = with_entry(device, |entry| entry.gpu.clone()) {
        return Ok(gpu);
    }
    let gpu = build()?;
    with_entry(device, |entry| entry.gpu = Some(gpu.clone()));
    Ok(gpu)
}

/// The sprite atlas of the windows drawing on `device`, made when no window
/// holds one.
pub(crate) fn atlas(device: &Retained<ProtocolObject<dyn MTLDevice>>) -> Arc<MetalAtlas> {
    with_entry(device, |entry| {
        entry.atlas.upgrade().unwrap_or_else(|| {
            let supports_shared_storage =
                cfg!(target_os = "ios") || device.supportsFamily(MTLGPUFamily::Apple1);
            #[expect(
                clippy::arc_with_non_send_sync,
                reason = "PlatformWindow::sprite_atlas requires Arc, not Send or Sync"
            )]
            let atlas = Arc::new(MetalAtlas::new(device.clone(), supports_shared_storage));
            entry.atlas = Arc::downgrade(&atlas);
            atlas
        })
    })
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
