//! Giving the instance buffers' memory back after a spike. See
//! [`buffer_shrink`](super::buffer_shrink) for the policy.

use crate::metal_renderer::InstanceBufferPool;

/// What the pool's buffers start as, and shrink no lower than.
const INITIAL_SIZE: usize = 2 * 1024 * 1024;

/// Records the bytes a drawn frame wrote and, when the buffers have been
/// mostly idle for long enough, makes the pool hand out smaller ones. Buffers
/// still in flight are dropped as they return. The passes drawing an effect
/// group are part of the frame that draws them and are not counted.
pub(crate) fn note_frame(pool: &mut InstanceBufferPool, used: usize) {
    if crate::fast::effects::in_group_pass() {
        return;
    }
    if let Some(size) = pool.shrink.frame(used, pool.buffer_size, INITIAL_SIZE) {
        log::info!("decreased instance buffer size to {size}");
        pool.reset(size);
    }
}

#[cfg(test)]
mod tests {
    use super::INITIAL_SIZE;
    use crate::metal_renderer::InstanceBufferPool;
    use crate::metal_renderer::MetalRenderer;
    use gpui::{Bounds, DevicePixels, ScaledPixels, Scene, point, size};
    use objc2_metal::MTLDevice as _;
    use parking_lot::Mutex;
    use std::sync::Arc;

    fn scene_of(quads: usize) -> Scene {
        let mut scene = Scene::default();
        for i in 0..quads {
            let bounds = Bounds {
                origin: point(
                    ScaledPixels((i % 400) as f32 * 2.),
                    ScaledPixels((i / 400 % 300) as f32 * 2.),
                ),
                size: size(ScaledPixels(2.), ScaledPixels(2.)),
            };
            scene.insert_primitive(gpui::Quad {
                order: 0,
                border_style_and_transform: 0,
                bounds,
                content_mask: gpui::ClipMask::new(bounds),
                background: gpui::rgb(0x3366ff).into(),
                border_color: gpui::Hsla::default(),
                corner_radii: gpui::Corners::default(),
                border_widths: gpui::Edges::default(),
            });
        }
        scene.finish();
        scene
    }

    #[test]
    fn the_pool_shrinks_after_a_spike_and_stays_small() {
        let size = size(DevicePixels(256), DevicePixels(256));
        let pool = Arc::new(Mutex::new(InstanceBufferPool::default()));
        let mut renderer = MetalRenderer::new_headless(pool.clone()).unwrap();
        renderer
            .render_scene_to_image(&scene_of(200_000), size)
            .unwrap();
        assert!(pool.lock().buffer_size > INITIAL_SIZE);
        let normal = scene_of(200);
        for _ in 0..crate::fast::buffer_shrink::QUIET_FRAMES + 2 {
            renderer.render_scene_to_image(&normal, size).unwrap();
        }
        assert_eq!(pool.lock().buffer_size, INITIAL_SIZE);
    }

    /// `cargo test -p gpui_apple instance_buffer_cost --release -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check"]
    fn instance_buffer_cost() {
        fn footprint_mb() -> String {
            let out = std::process::Command::new("footprint")
                .arg(std::process::id().to_string())
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            out.lines()
                .find(|l| l.contains("Footprint:"))
                .unwrap_or("?")
                .trim()
                .to_string()
        }
        let size = size(DevicePixels(1024), DevicePixels(768));
        let normal = scene_of(2000);
        let huge = scene_of(200_000);
        let pool = Arc::new(Mutex::new(InstanceBufferPool::default()));
        let mut renderer = MetalRenderer::new_headless(pool.clone()).unwrap();
        let allocated =
            |renderer: &MetalRenderer| renderer.device.currentAllocatedSize() / (1 << 20);
        let capacity = || pool.lock().buffer_size >> 20;
        println!(
            "start: allocated {} MB, pool {} MB, {}",
            allocated(&renderer),
            capacity(),
            footprint_mb()
        );
        renderer.render_scene_to_image(&huge, size).unwrap();
        println!(
            "after one huge frame: allocated {} MB, pool {} MB, {}",
            allocated(&renderer),
            capacity(),
            footprint_mb()
        );
        let start = std::time::Instant::now();
        for _ in 0..300 {
            renderer.render_scene_to_image(&normal, size).unwrap();
        }
        println!(
            "after 300 normal frames: allocated {} MB, pool {} MB, {}, {:.2} ms/frame",
            allocated(&renderer),
            capacity(),
            footprint_mb(),
            start.elapsed().as_secs_f64() * 1000. / 300.
        );
        // Alternate 60 huge and 60 normal frames: count capacity changes.
        let mut changes = 0;
        let mut last = capacity();
        let start = std::time::Instant::now();
        for block in 0..10 {
            let scene = if block % 2 == 0 { &huge } else { &normal };
            for _ in 0..60 {
                renderer.render_scene_to_image(scene, size).unwrap();
                if capacity() != last {
                    changes += 1;
                    last = capacity();
                }
            }
        }
        println!(
            "alternating 60/60 x10: {changes} capacity changes, {:.2} ms/frame, allocated {} MB",
            start.elapsed().as_secs_f64() * 1000. / 600.,
            allocated(&renderer)
        );
    }
}
