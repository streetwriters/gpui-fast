//! The sprite atlas of the CPU renderer. Like `gpui::HeadlessAtlas` it gives
//! every tile a texture of its own, but it keeps the pixels so the renderer can
//! sample them. The text system and the image cache fill it exactly as they
//! fill the Metal and wgpu atlases, so glyphs look the same.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Result, ensure};
use gpui::{
    AtlasBackend, AtlasKey, AtlasState, AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds,
    DevicePixels, PlatformAtlas, Point, Size, TileId,
};

/// The pixels of one tile.
pub(crate) struct Texture {
    pub kind: AtlasTextureKind,
    pub width: usize,
    pub height: usize,
    /// One byte per pixel (coverage) for monochrome tiles, otherwise four, in
    /// the order the GPU atlases take them: blue, green, red, alpha.
    pub data: Vec<u8>,
}

impl Texture {
    fn bytes_per_pixel(&self) -> usize {
        match self.kind {
            AtlasTextureKind::Monochrome => 1,
            AtlasTextureKind::Polychrome | AtlasTextureKind::Subpixel => 4,
        }
    }

    /// The texel at `(x, y)`, clamped to the edge, as RGBA in `0.0..=1.0`. A
    /// monochrome texel is `[0, 0, 0, coverage]`. A subpixel texel holds the
    /// coverage of each colour channel, and the mean of the three as its alpha.
    pub fn texel(&self, x: i32, y: i32) -> [f32; 4] {
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.height as i32 - 1) as usize;
        let index = (y * self.width + x) * self.bytes_per_pixel();
        match self.kind {
            AtlasTextureKind::Monochrome => [0., 0., 0., self.data[index] as f32 / 255.],
            AtlasTextureKind::Polychrome => {
                let bgra = &self.data[index..index + 4];
                [
                    bgra[2] as f32 / 255.,
                    bgra[1] as f32 / 255.,
                    bgra[0] as f32 / 255.,
                    bgra[3] as f32 / 255.,
                ]
            }
            AtlasTextureKind::Subpixel => {
                let bgra = &self.data[index..index + 4];
                let (b, g, r) = (
                    bgra[0] as f32 / 255.,
                    bgra[1] as f32 / 255.,
                    bgra[2] as f32 / 255.,
                );
                [r, g, b, (r + g + b) / 3.]
            }
        }
    }

    /// Samples at a point in texel units, with linear filtering and the texel
    /// centres at half integers, as the GPU samplers do.
    pub fn sample_linear(&self, x: f32, y: f32) -> [f32; 4] {
        let (x, y) = (x - 0.5, y - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let a = self.texel(x0, y0);
        let b = self.texel(x0 + 1, y0);
        let c = self.texel(x0, y0 + 1);
        let d = self.texel(x0 + 1, y0 + 1);
        let mut out = [0.; 4];
        for channel in 0..4 {
            let top = a[channel] + (b[channel] - a[channel]) * fx;
            let bottom = c[channel] + (d[channel] - c[channel]) * fx;
            out[channel] = top + (bottom - top) * fy;
        }
        out
    }
}

#[derive(Default)]
pub(crate) struct CpuAtlasBackend {
    next_id: u32,
    textures: HashMap<u32, Arc<Texture>>,
}

impl AtlasBackend for CpuAtlasBackend {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile> {
        ensure!(
            size.width.0 > 0 && size.height.0 > 0,
            "atlas dimensions must be positive"
        );
        let (width, height) = (size.width.0 as usize, size.height.0 as usize);
        let bytes_per_pixel = match kind {
            AtlasTextureKind::Monochrome => 1,
            AtlasTextureKind::Polychrome | AtlasTextureKind::Subpixel => 4,
        };
        let length = width * height * bytes_per_pixel;
        ensure!(
            bytes.len() >= length,
            "atlas upload byte length is {}, expected at least {length}",
            bytes.len()
        );
        self.next_id += 1;
        let texture_id = self.next_id;
        self.next_id += 1;
        let tile_id = self.next_id;
        self.textures.insert(
            texture_id,
            Arc::new(Texture {
                kind,
                width,
                height,
                data: bytes[..length].to_vec(),
            }),
        );
        Ok(AtlasTile {
            texture_id: AtlasTextureId {
                index: texture_id,
                kind,
            },
            tile_id: TileId(tile_id),
            padding: 0,
            bounds: Bounds {
                origin: Point::default(),
                size,
            },
        })
    }

    fn remove(&mut self, tile: AtlasTile) {
        self.textures.remove(&tile.texture_id.index);
    }
}

/// A sprite atlas whose tiles live in memory.
#[derive(Default)]
pub struct CpuAtlas(Mutex<AtlasState<CpuAtlasBackend>>);

impl CpuAtlas {
    /// The pixels of the tile texture `id`, if the atlas still holds it.
    pub(crate) fn texture(&self, id: AtlasTextureId) -> Option<Arc<Texture>> {
        let state = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.backend.textures.get(&id.index).cloned()
    }
}

impl PlatformAtlas for CpuAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let mut state = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.get_or_insert_with(key, build)
    }

    fn remove(&self, key: &AtlasKey) {
        let mut state = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.remove(key);
    }

    fn contains(&self, key: &AtlasKey) -> bool {
        let state = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.contains(key)
    }
}
