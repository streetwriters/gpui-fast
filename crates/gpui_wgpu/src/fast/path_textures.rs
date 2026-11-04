//! The window's path intermediate textures, held only while paths are drawn
//! and sized to the frame's paths.
//!
//! Upstream keeps a window-sized texture and a multisample one from the first
//! frame. Here they are allocated for the first frame with paths, cover the
//! union of the paths' bounds rounded out to `BUCKET` pixels, and are released
//! after `IDLE_FRAMES` frames without paths. Planning moves the vertices into
//! the region and gives each path sprite the region, which the path shader
//! maps the screen position into. Layer tiles and effect groups have textures
//! of their own that cover their whole size, and a sprite without a region
//! says so.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Bounds, DevicePixels, ScaledPixels, Scene, Size, point, size};

use crate::wgpu_renderer::{GlobalParams, PathRasterizationVertex, PathSprite, WgpuRendererCore};

/// The granularity, in device pixels, of the region's origin and size.
const BUCKET: i32 = 256;

/// Frames without paths after which the textures are released.
const IDLE_FRAMES: u32 = 120;

/// Whether the adapter saves memory on transient attachments.
static TRANSIENT_MSAA: AtomicBool = AtomicBool::new(false);

pub(crate) fn note_adapter(info: &wgpu::AdapterInfo) {
    TRANSIENT_MSAA.store(info.transient_saves_memory == Some(true), Ordering::Relaxed);
}

/// The extra usage of the multisample texture, whose contents are resolved
/// and never read.
pub(crate) fn msaa_usage() -> wgpu::TextureUsages {
    if TRANSIENT_MSAA.load(Ordering::Relaxed) {
        wgpu::TextureUsages::TRANSIENT_ATTACHMENT
    } else {
        wgpu::TextureUsages::empty()
    }
}

/// What a pass does with a multisample attachment it resolves.
pub(crate) fn store_op(resolved: bool) -> wgpu::StoreOp {
    if resolved {
        wgpu::StoreOp::Discard
    } else {
        wgpu::StoreOp::Store
    }
}

#[derive(Default)]
pub(crate) struct PathTextures {
    region: Bounds<DevicePixels>,
    idle_frames: u32,
}

fn contains(outer: Bounds<DevicePixels>, inner: Bounds<DevicePixels>) -> bool {
    outer.origin.x <= inner.origin.x
        && outer.origin.y <= inner.origin.y
        && outer.right() >= inner.right()
        && outer.bottom() >= inner.bottom()
}

fn union(a: Bounds<DevicePixels>, b: Bounds<DevicePixels>) -> Bounds<DevicePixels> {
    let origin = point(a.origin.x.min(b.origin.x), a.origin.y.min(b.origin.y));
    let (right, bottom) = (a.right().max(b.right()), a.bottom().max(b.bottom()));
    Bounds {
        origin,
        size: size(right - origin.x, bottom - origin.y),
    }
}

/// The union of the clipped bounds of the paths drawn in the scene's pass,
/// which include those of its rounded clip scopes.
fn collect_bounds(scene: &Scene, covered: &mut Option<Bounds<ScaledPixels>>) {
    for path in &scene.paths {
        let bounds = path.clipped_bounds();
        *covered = Some(covered.map_or(bounds, |covered| covered.union(&bounds)));
    }
    for sprite in &scene.polychrome_sprites {
        if let Some(frame) = crate::fast::rounded_clip::group(scene, sprite.tile.texture_id)
            && let Some(child) = frame.content.scene()
        {
            collect_bounds(child, covered);
        }
    }
}

/// The device pixels the scene's paths can touch, within the viewport.
fn needed(scene: &Scene, viewport: Size<DevicePixels>) -> Option<Bounds<DevicePixels>> {
    let mut bounds = None;
    collect_bounds(scene, &mut bounds);
    let bounds: Bounds<ScaledPixels> = bounds?;
    let left = (bounds.origin.x.0.floor() as i32).clamp(0, viewport.width.0 - 1);
    let top = (bounds.origin.y.0.floor() as i32).clamp(0, viewport.height.0 - 1);
    // Paths wholly outside the viewport still need a texture to be encoded.
    let right = (bounds.right().0.ceil() as i32).clamp(left + 1, viewport.width.0);
    let bottom = (bounds.bottom().0.ceil() as i32).clamp(top + 1, viewport.height.0);
    Some(Bounds {
        origin: point(DevicePixels(left), DevicePixels(top)),
        size: size(DevicePixels(right - left), DevicePixels(bottom - top)),
    })
}

fn bucketed(region: Bounds<DevicePixels>, viewport: Size<DevicePixels>) -> Bounds<DevicePixels> {
    let left = region.origin.x.0.div_euclid(BUCKET) * BUCKET;
    let top = region.origin.y.0.div_euclid(BUCKET) * BUCKET;
    let right = (region.right().0 + BUCKET - 1).div_euclid(BUCKET) * BUCKET;
    let bottom = (region.bottom().0 + BUCKET - 1).div_euclid(BUCKET) * BUCKET;
    let right = right.min(viewport.width.0.max(region.right().0));
    let bottom = bottom.min(viewport.height.0.max(region.bottom().0));
    Bounds {
        origin: point(DevicePixels(left), DevicePixels(top)),
        size: size(DevicePixels(right - left), DevicePixels(bottom - top)),
    }
}

/// The region the textures would first be allocated with for the scene.
#[cfg(test)]
pub(crate) fn region_for(
    scene: &Scene,
    viewport: Size<DevicePixels>,
) -> Option<Bounds<DevicePixels>> {
    needed(scene, viewport).map(|needed| bucketed(needed, viewport))
}

impl WgpuRendererCore {
    /// Makes the textures cover the scene's paths, allocating them for the
    /// first scene with paths, and counts the frames without.
    pub(crate) fn prepare_path_textures(&mut self, scene: &Scene, viewport: Size<DevicePixels>) {
        let Some(needed) = needed(scene, viewport) else {
            let state = &mut self.fast_path_textures;
            state.idle_frames += 1;
            if state.idle_frames >= IDLE_FRAMES {
                self.resources.invalidate_intermediate_textures();
                self.fast_path_textures.idle_frames = 0;
            }
            return;
        };
        self.fast_path_textures.idle_frames = 0;
        let held = self.path_region();
        if held.is_some_and(|held| contains(held, needed)) {
            return;
        }
        let wanted = held.map_or(needed, |held| union(held, needed));
        let region = bucketed(wanted, viewport);
        self.ensure_intermediate_textures(region.size);
        self.fast_path_textures.region = region;
    }

    /// The pixels the held textures cover, if there are textures.
    pub(crate) fn path_region(&self) -> Option<Bounds<DevicePixels>> {
        let texture = self.resources.path_intermediate_texture.as_ref()?;
        let held = size(
            DevicePixels(texture.width() as i32),
            DevicePixels(texture.height() as i32),
        );
        let region = self.fast_path_textures.region;
        Some(if region.size == held {
            region
        } else {
            Bounds {
                origin: point(DevicePixels(0), DevicePixels(0)),
                size: held,
            }
        })
    }
}

/// The path globals rasterize at the size of the textures.
pub(crate) fn path_globals(renderer: &WgpuRendererCore, globals: GlobalParams) -> GlobalParams {
    match renderer.path_region() {
        Some(region) => GlobalParams {
            viewport_size: [region.size.width.0 as f32, region.size.height.0 as f32],
            ..globals
        },
        None => globals,
    }
}

/// Moves the path vertices of a scene's pass into `region` and tells its
/// sprites where the texture is. Without a region the texture covers the
/// pass's whole target.
pub(crate) fn to_region(
    vertices: &mut [PathRasterizationVertex],
    sprites: &mut [PathSprite],
    region: Option<Bounds<DevicePixels>>,
) {
    let Some(region) = region else {
        return;
    };
    let (dx, dy) = (region.origin.x.0 as f32, region.origin.y.0 as f32);
    for vertex in vertices {
        vertex.xy_position.x.0 -= dx;
        vertex.xy_position.y.0 -= dy;
        for bounds in [&mut vertex.bounds, &mut vertex.rounded_bounds] {
            bounds.origin.x.0 -= dx;
            bounds.origin.y.0 -= dy;
        }
    }
    for sprite in sprites {
        sprite.region = Bounds {
            origin: point(ScaledPixels(dx), ScaledPixels(dy)),
            size: size(
                ScaledPixels(region.size.width.0 as f32),
                ScaledPixels(region.size.height.0 as f32),
            ),
        };
    }
}

#[cfg(all(not(target_family = "wasm"), any(test, feature = "test-support")))]
impl crate::wgpu_renderer::WgpuHeadlessRenderer {
    /// The bytes of the window path textures the renderer holds, the
    /// multisample one counted in full.
    pub fn path_texture_bytes(&self) -> u64 {
        let resources = &self.core.resources;
        [
            &resources.path_intermediate_texture,
            &resources.path_msaa_texture,
        ]
        .into_iter()
        .flatten()
        .map(|texture| {
            let texels = u64::from(texture.width()) * u64::from(texture.height());
            let bytes = u64::from(texture.format().block_copy_size(None).unwrap_or(4));
            texels * bytes * u64::from(texture.sample_count())
        })
        .sum()
    }
}
