//! The DirectX state every window on a device shares: the sprite atlas and
//! the compiled shaders. A window's own state (swap chain, path textures,
//! tile cache, instance buffers) stays in its renderer.
//!
//! Windows of one device draw from one atlas, so the glyphs and images they
//! have in common are rasterized and uploaded once. The atlas is kept only
//! while a window uses it; the shaders, which hold no textures, for the
//! thread's life.

use std::{
    cell::RefCell,
    sync::{Arc, Weak},
};

use anyhow::Result;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11PixelShader, ID3D11VertexShader};

use crate::{
    directx_atlas::DirectXAtlas,
    directx_renderer::{DirectXRendererDevices, shader_resources::ShaderModule},
};

struct AtlasEntry {
    device: ID3D11Device,
    atlas: Weak<DirectXAtlas>,
}

struct ShaderEntry {
    device: ID3D11Device,
    module: ShaderModule,
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
}

thread_local! {
    static ATLASES: RefCell<Vec<AtlasEntry>> = const { RefCell::new(Vec::new()) };
    static SHADERS: RefCell<Vec<ShaderEntry>> = const { RefCell::new(Vec::new()) };
}

/// The atlas of the windows drawing on `devices`' device.
pub(crate) fn atlas(devices: &DirectXRendererDevices) -> Arc<DirectXAtlas> {
    ATLASES.with(|atlases| {
        let mut atlases = atlases.borrow_mut();
        atlases.retain(|entry| entry.atlas.strong_count() > 0);
        if let Some(atlas) = atlases
            .iter()
            .find(|entry| entry.device == devices.device)
            .and_then(|entry| entry.atlas.upgrade())
        {
            return atlas;
        }
        let atlas = Arc::new(DirectXAtlas::new(&devices.device, &devices.device_context));
        atlases.push(AtlasEntry {
            device: devices.device.clone(),
            atlas: Arc::downgrade(&atlas),
        });
        atlas
    })
}

/// Points `atlas` at the recreated device and empties it, once however many
/// windows share it recover.
pub(crate) fn recover_atlas(atlas: &Arc<DirectXAtlas>, devices: &DirectXRendererDevices) {
    let recovered = ATLASES.with(|atlases| {
        let mut atlases = atlases.borrow_mut();
        let entry = atlases.iter_mut().find(|entry| {
            entry
                .atlas
                .upgrade()
                .is_some_and(|a| Arc::ptr_eq(&a, atlas))
        });
        match entry {
            Some(entry) if entry.device == devices.device => true,
            Some(entry) => {
                entry.device = devices.device.clone();
                false
            }
            None => false,
        }
    });
    if !recovered {
        atlas.handle_device_lost(&devices.device, &devices.device_context);
    }
}

/// The shaders of `module` on `device`, compiled by `build` the first time.
pub(crate) fn shaders(
    device: &ID3D11Device,
    module: ShaderModule,
    build: impl FnOnce() -> Result<(ID3D11VertexShader, ID3D11PixelShader)>,
) -> Result<(ID3D11VertexShader, ID3D11PixelShader)> {
    SHADERS.with(|shaders| {
        let mut shaders = shaders.borrow_mut();
        if let Some(entry) = shaders
            .iter()
            .find(|entry| entry.module == module && entry.device == *device)
        {
            return Ok((entry.vertex.clone(), entry.fragment.clone()));
        }
        // Shaders of a lost device are of no use to anyone.
        shaders.retain(|entry| entry.device == *device);
        let (vertex, fragment) = build()?;
        shaders.push(ShaderEntry {
            device: device.clone(),
            module,
            vertex: vertex.clone(),
            fragment: fragment.clone(),
        });
        Ok((vertex, fragment))
    })
}
