//! The window's path intermediate textures, held at their full size only
//! while paths are drawn and sized to the frame's paths.
//!
//! Upstream creates a window-sized texture and a multisample one with the
//! swap chain's buffers. Here resource creation makes one pixel placeholders,
//! and the first frame with paths replaces them with textures that cover the
//! union of the paths' bounds, rounded out to `BUCKET` pixels. They are
//! replaced by placeholders again after `IDLE_FRAMES` frames without paths.
//! Rasterization moves the vertices into the region, and each path sprite
//! carries it for the shader to map the screen position into the texture.
//! Targets whose textures cover their whole size keep a zero region, which
//! the shader reads as the viewport.

use anyhow::{Context as _, Result};
use gpui::{Bounds, DevicePixels, ScaledPixels, Scene, Size, point, size};
use windows::Win32::Graphics::Direct3D11::{D3D11_TEXTURE2D_DESC, ID3D11Texture2D};

use crate::directx_renderer::{
    DirectXRenderer, PathRasterizationSprite, PathSprite,
    create_path_intermediate_msaa_texture_and_view, create_path_intermediate_texture,
};

/// The size, in device pixels, of the textures while no path is drawn.
pub(crate) const PLACEHOLDER: (u32, u32) = (1, 1);

/// The granularity, in device pixels, of the region's origin and size.
const BUCKET: i32 = 256;

/// Frames without paths after which the textures are replaced by placeholders.
const IDLE_FRAMES: u32 = 120;

#[derive(Default)]
pub(crate) struct PathTextures {
    region: Bounds<DevicePixels>,
    idle_frames: u32,
}

impl PathTextures {
    /// The pixels the texture covers, given its size: a texture of another
    /// size than the last allocated, such as a placeholder, starts at the origin.
    pub(crate) fn region_of(&self, texture: &ID3D11Texture2D) -> Bounds<DevicePixels> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        let held = size(
            DevicePixels(desc.Width as i32),
            DevicePixels(desc.Height as i32),
        );
        if self.region.size == held {
            self.region
        } else {
            Bounds {
                origin: point(DevicePixels(0), DevicePixels(0)),
                size: held,
            }
        }
    }
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

/// Makes the window's path textures cover the scene's paths, replacing the
/// placeholders for the first scene with paths, and counts the frames without.
/// Forwarded to by `DirectXRenderer::render`.
pub(crate) fn prepare(renderer: &mut DirectXRenderer, scene: &Scene) -> Result<()> {
    let viewport = size(
        DevicePixels(renderer.width as i32),
        DevicePixels(renderer.height as i32),
    );
    let Some(needed) = needed(scene, viewport) else {
        let state = &mut renderer.fast_frame.path_textures;
        state.idle_frames += 1;
        if state.idle_frames >= IDLE_FRAMES {
            state.idle_frames = 0;
            replace(renderer, bucketed_placeholder())?;
        }
        return Ok(());
    };
    renderer.fast_frame.path_textures.idle_frames = 0;
    let held = {
        let resources = renderer.resources.as_ref().context("resources missing")?;
        renderer
            .fast_frame
            .path_textures
            .region_of(&resources.path_intermediate_texture)
    };
    if contains(held, needed) {
        return Ok(());
    }
    let region = bucketed(union(held, needed), viewport);
    replace(renderer, region)
}

fn bucketed_placeholder() -> Bounds<DevicePixels> {
    Bounds {
        origin: point(DevicePixels(0), DevicePixels(0)),
        size: size(
            DevicePixels(PLACEHOLDER.0 as i32),
            DevicePixels(PLACEHOLDER.1 as i32),
        ),
    }
}

/// Replaces the window's path textures with ones covering `region`.
fn replace(renderer: &mut DirectXRenderer, region: Bounds<DevicePixels>) -> Result<()> {
    let device = &renderer.devices.as_ref().context("devices missing")?.device;
    let (width, height) = (region.size.width.0 as u32, region.size.height.0 as u32);
    let (intermediate, srv) = create_path_intermediate_texture(device, width, height)?;
    let (msaa, msaa_view) = create_path_intermediate_msaa_texture_and_view(device, width, height)?;
    let resources = renderer.resources.as_mut().context("resources missing")?;
    resources.path_intermediate_texture = intermediate;
    resources.path_intermediate_srv = srv;
    resources.path_intermediate_msaa_texture = msaa;
    resources.path_intermediate_msaa_view = msaa_view;
    renderer.fast_frame.path_textures.region = region;
    Ok(())
}

/// The part of the window the path textures cover.
fn window_region(renderer: &DirectXRenderer) -> Option<Bounds<DevicePixels>> {
    let resources = renderer.resources.as_ref()?;
    Some(
        renderer
            .fast_frame
            .path_textures
            .region_of(&resources.path_intermediate_texture),
    )
}

/// `move_vertices` for upstream's own path drawing, which draws into the
/// window's textures.
pub(crate) fn shift_vertices(vertices: &mut [PathRasterizationSprite], renderer: &DirectXRenderer) {
    move_vertices(vertices, window_region(renderer));
}

/// `set_regions` for upstream's own path drawing.
pub(crate) fn set_regions(sprites: &mut [PathSprite], renderer: &DirectXRenderer) {
    set_sprite_regions(sprites, window_region(renderer));
}

/// Moves a pass's path vertices into `region`.
pub(crate) fn move_vertices(
    vertices: &mut [PathRasterizationSprite],
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
}

/// Tells path sprites where their texture is.
pub(crate) fn set_sprite_regions(sprites: &mut [PathSprite], region: Option<Bounds<DevicePixels>>) {
    let Some(region) = region else {
        return;
    };
    for sprite in sprites {
        sprite.region = Bounds {
            origin: point(
                ScaledPixels(region.origin.x.0 as f32),
                ScaledPixels(region.origin.y.0 as f32),
            ),
            size: size(
                ScaledPixels(region.size.width.0 as f32),
                ScaledPixels(region.size.height.0 as f32),
            ),
        };
    }
}
