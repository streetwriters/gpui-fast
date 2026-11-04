//! Path intermediate textures that exist only while paths are drawn, sized to
//! the paths of the frame.
//!
//! Upstream keeps a texture as large as the window, plus a multisample one,
//! from the first frame whether or not the scene has paths. Here they are
//! allocated for the first frame with paths, cover the union of the frame's
//! path bounds rounded out to `BUCKET` pixels, and are released after
//! `IDLE_FRAMES` drawn frames without paths. The region the textures cover
//! is kept in the coordinates of the pass that allocated it: rasterization
//! subtracts its origin from the vertices, and compositing maps the screen
//! position into the texture by it.

use std::{mem, ptr};

use anyhow::Result;
use gpui::{Bounds, DevicePixels, ScaledPixels, Scene, Size, point, size};
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLRenderCommandEncoder, MTLTexture};

use crate::metal_renderer::{MetalRenderer, PathRasterizationVertex, SpriteInputIndex};

/// The granularity, in device pixels, of the region's origin and size.
const BUCKET: i32 = 256;

/// Drawn frames without paths after which the textures are released.
const IDLE_FRAMES: u32 = 120;

#[derive(Default)]
pub(crate) struct PathTextures {
    pub(crate) region: Bounds<DevicePixels>,
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

/// The device pixels the scene's paths can touch, within the viewport.
fn needed(scene: &Scene, viewport: Size<DevicePixels>) -> Option<Bounds<DevicePixels>> {
    let mut covered: Option<Bounds<ScaledPixels>> = None;
    for path in &scene.paths {
        let bounds = path.clipped_bounds();
        covered = Some(covered.map_or(bounds, |covered| covered.union(&bounds)));
    }
    let covered = covered?;
    let left = (covered.origin.x.0.floor() as i32).max(0);
    let top = (covered.origin.y.0.floor() as i32).max(0);
    let right = (covered.right().0.ceil() as i32).min(viewport.width.0);
    let bottom = (covered.bottom().0.ceil() as i32).min(viewport.height.0);
    // Paths wholly outside the viewport still need a texture to be encoded.
    let (right, bottom) = if right > left && bottom > top {
        (right, bottom)
    } else {
        (
            left.min(viewport.width.0 - 1) + 1,
            top.min(viewport.height.0 - 1) + 1,
        )
    };
    let (left, top) = (left.min(right - 1), top.min(bottom - 1));
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

impl MetalRenderer {
    /// Makes the textures cover the scene's paths, allocating them if the
    /// scene is the first with paths, and counts the frames without.
    pub(crate) fn prepare_path_textures(
        &mut self,
        scene: &Scene,
        viewport: Size<DevicePixels>,
    ) -> Result<()> {
        if viewport.width.0 <= 0 || viewport.height.0 <= 0 {
            return Ok(());
        }
        let Some(needed) = needed(scene, viewport) else {
            // A group's pass draws within a frame that is counted once.
            if !crate::fast::effects::in_group_pass() {
                let state = &mut self.fast_path_textures;
                state.idle_frames += 1;
                if state.idle_frames >= IDLE_FRAMES {
                    self.release_path_textures();
                }
            }
            return Ok(());
        };
        self.fast_path_textures.idle_frames = 0;
        let held = self.path_region();
        if held.is_some_and(|held| contains(held, needed)) {
            return Ok(());
        }
        let wanted = held.map_or(needed, |held| union(held, needed));
        let region = bucketed(wanted, viewport);
        self.update_path_intermediate_textures(region.size)?;
        self.fast_path_textures.region = region;
        Ok(())
    }

    pub(crate) fn release_path_textures(&mut self) {
        self.path_intermediate_texture = None;
        self.path_intermediate_msaa_texture = None;
        self.fast_path_textures.idle_frames = 0;
    }

    /// The pixels the held textures cover. Textures that upstream's
    /// `update_path_intermediate_textures` allocated for a whole viewport
    /// start at the origin.
    pub(crate) fn path_region(&self) -> Option<Bounds<DevicePixels>> {
        let texture = self.path_intermediate_texture.as_ref()?;
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

/// Moves `vertices` into the textures' region and returns the size to
/// rasterize them at.
pub(crate) fn to_region(
    renderer: &MetalRenderer,
    vertices: &mut [PathRasterizationVertex],
    viewport: Size<DevicePixels>,
) -> Size<DevicePixels> {
    let Some(region) = renderer.path_region() else {
        return viewport;
    };
    for vertex in vertices {
        shift_vertex(vertex, region.origin);
    }
    region.size
}

pub(crate) fn shift_vertex(
    vertex: &mut PathRasterizationVertex,
    origin: gpui::Point<DevicePixels>,
) {
    let (dx, dy) = (origin.x.0 as f32, origin.y.0 as f32);
    vertex.xy_position.x.0 -= dx;
    vertex.xy_position.y.0 -= dy;
    for bounds in [&mut vertex.bounds, &mut vertex.rounded_bounds] {
        bounds.origin.x.0 -= dx;
        bounds.origin.y.0 -= dy;
    }
}

/// Gives the path sprite shader the region its texture covers.
pub(crate) fn bind_region(
    renderer: &MetalRenderer,
    encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
) {
    let region = renderer.path_region().unwrap_or_default();
    let region = [
        region.origin.x.0 as f32,
        region.origin.y.0 as f32,
        region.size.width.0 as f32,
        region.size.height.0 as f32,
    ];
    // SAFETY: The bytes are copied from a live array into the shader's float4 slot.
    unsafe {
        encoder.setVertexBytes_length_atIndex(
            ptr::NonNull::from(&region).cast(),
            mem::size_of_val(&region),
            SpriteInputIndex::PathRegion as usize,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{IDLE_FRAMES, MetalRenderer};
    use gpui::{Bounds, DevicePixels, ScaledPixels, Scene, Size, point, size};
    use gpui::{ContentMask, Corners, Hsla, Pixels, Quad, px, rgba};
    use objc2_metal::MTLTexture;

    const VIEWPORT: (i32, i32) = (1200, 900);

    fn viewport() -> Size<DevicePixels> {
        size(DevicePixels(VIEWPORT.0), DevicePixels(VIEWPORT.1))
    }

    fn mask(rounded: bool) -> ContentMask<Pixels> {
        let bounds = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(VIEWPORT.0 as f32), px(VIEWPORT.1 as f32)),
        };
        if rounded {
            ContentMask {
                bounds,
                rounded_bounds: Bounds {
                    origin: point(px(20.), px(20.)),
                    size: size(px(VIEWPORT.0 as f32 - 40.), px(VIEWPORT.1 as f32 - 40.)),
                },
                corner_radii: Corners::all(px(60.)),
            }
        } else {
            ContentMask::new(bounds)
        }
    }

    fn path_at(scene: &mut Scene, x: f32, y: f32, side: f32, gradient: bool, rounded: bool) {
        let mut path = gpui::Path::new(point(px(x), px(y)));
        path.line_to(point(px(x + side), px(y + side * 0.3)));
        path.curve_to(
            point(px(x + side * 0.2), px(y + side)),
            point(px(x + side), px(y + side)),
        );
        path.line_to(point(px(x), px(y)));
        path.content_mask = mask(rounded);
        path.color = if gradient {
            gpui::linear_gradient(
                35.,
                gpui::linear_color_stop(Hsla::from(rgba(0xff0066ff)), 0.),
                gpui::linear_color_stop(Hsla::from(rgba(0x0066ffcc)), 1.),
            )
        } else {
            Hsla::from(rgba(0x8800ffcc)).into()
        };
        scene.insert_primitive(path.scale(1.));
    }

    fn quad(scene: &mut Scene) {
        scene.insert_primitive(Quad {
            bounds: Bounds {
                origin: point(ScaledPixels(300.), ScaledPixels(300.)),
                size: size(ScaledPixels(500.), ScaledPixels(200.)),
            },
            content_mask: mask(false).scale(1.),
            background: Hsla::from(rgba(0x3366ccff)).into(),
            ..Default::default()
        });
    }

    fn renderer() -> MetalRenderer {
        MetalRenderer::new_headless(Default::default()).expect("Metal device")
    }

    /// Renders the scene with the textures sized to the paths, and with
    /// textures as large as the viewport as upstream allocates them.
    fn assert_matches_full_textures(mut scene: Scene) -> Bounds<DevicePixels> {
        scene.finish();
        let mut sized = renderer();
        let mut full = renderer();
        full.update_path_intermediate_textures(viewport()).unwrap();
        let sized_image = sized.render_offscreen(&scene, viewport()).unwrap();
        let full_image = full.render_offscreen(&scene, viewport()).unwrap();
        assert!(sized_image == full_image, "images differ");
        sized.path_region().unwrap()
    }

    #[test]
    fn a_path_at_the_bottom_right_edge_renders_as_with_full_textures() {
        for (gradient, rounded) in [(false, false), (true, false), (true, true)] {
            let mut scene = Scene::default();
            quad(&mut scene);
            path_at(&mut scene, 1150., 850., 44., gradient, rounded);
            let region = assert_matches_full_textures(scene);
            assert!(region.size.width.0 < VIEWPORT.0 && region.size.height.0 < VIEWPORT.1);
            assert_eq!(region.right().0, VIEWPORT.0);
            assert_eq!(region.bottom().0, VIEWPORT.1);
        }
    }

    #[test]
    fn far_apart_path_batches_render_as_with_full_textures() {
        let mut scene = Scene::default();
        path_at(&mut scene, 24., 18., 60., true, false);
        quad(&mut scene);
        path_at(&mut scene, 1100., 800., 60., false, true);
        path_at(&mut scene, 600., 420., 90., true, true);
        assert_matches_full_textures(scene);
    }

    #[test]
    fn a_path_crossing_the_viewport_edge_renders_as_with_full_textures() {
        let mut scene = Scene::default();
        quad(&mut scene);
        path_at(&mut scene, 1180., 880., 100., true, false);
        assert_matches_full_textures(scene);
    }

    #[test]
    fn paths_in_effect_groups_render_as_with_full_textures() {
        for effect in [None, Some(gpui::GroupEffect::Blur { sigma: 3.0 })] {
            let mut scene = Scene::default();
            quad(&mut scene);
            path_at(&mut scene, 1100., 820., 70., true, false);
            let scope = match effect {
                Some(effect) => scene.begin_effect_group_with(0.7, effect),
                None => scene.begin_effect_group(0.7),
            };
            path_at(&mut scene, 700., 480., 90., true, true);
            quad(&mut scene);
            scene.end_effect_group(
                scope,
                Bounds {
                    origin: point(ScaledPixels(600.), ScaledPixels(400.)),
                    size: size(ScaledPixels(400.), ScaledPixels(300.)),
                },
            );
            path_at(&mut scene, 1000., 820., 50., false, false);
            assert_matches_full_textures(scene);
        }
    }

    #[test]
    fn textures_follow_the_paths_and_are_released_when_idle() {
        let mut renderer = renderer();
        let mut without = Scene::default();
        quad(&mut without);
        without.finish();
        let mut icon = Scene::default();
        path_at(&mut icon, 700., 520., 24., false, false);
        icon.finish();

        renderer.render_offscreen(&without, viewport()).unwrap();
        assert!(renderer.path_intermediate_texture.is_none());

        renderer.render_offscreen(&icon, viewport()).unwrap();
        let texture = renderer.path_intermediate_texture.clone().unwrap();
        assert_eq!((texture.width(), texture.height()), (256, 256));
        assert_eq!(renderer.path_region().unwrap().origin.x.0, 512);

        renderer.render_offscreen(&icon, viewport()).unwrap();
        assert!(std::ptr::eq(
            &*texture,
            renderer.path_intermediate_texture.as_deref().unwrap()
        ));

        let mut corner = Scene::default();
        path_at(&mut corner, 20., 20., 24., false, false);
        corner.finish();
        renderer.render_offscreen(&corner, viewport()).unwrap();
        let grown = renderer.path_intermediate_texture.clone().unwrap();
        assert!(grown.width() >= 768 && grown.height() >= 512);

        for _ in 0..IDLE_FRAMES - 1 {
            renderer.render_offscreen(&without, viewport()).unwrap();
        }
        assert!(renderer.path_intermediate_texture.is_some());
        renderer.render_offscreen(&without, viewport()).unwrap();
        assert!(renderer.path_intermediate_texture.is_none());
        assert!(renderer.path_intermediate_msaa_texture.is_none());
    }
}
