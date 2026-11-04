#[cfg(not(target_family = "wasm"))]
mod bench {
    use gpui::*;
    use gpui_fast as gpui;
    use std::time::Instant;

    const WIDTH: i32 = 2560;
    const HEIGHT: i32 = 1600;

    fn quad(scene: &mut Scene) {
        scene.insert_primitive(Quad {
            bounds: Bounds {
                origin: point(ScaledPixels(100.), ScaledPixels(100.)),
                size: size(ScaledPixels(800.), ScaledPixels(600.)),
            },
            content_mask: ContentMask::new(Bounds {
                origin: point(ScaledPixels(0.), ScaledPixels(0.)),
                size: size(ScaledPixels(WIDTH as f32), ScaledPixels(HEIGHT as f32)),
            }),
            background: Hsla::from(rgba(0x3366ccff)).into(),
            ..Default::default()
        });
    }

    fn path_at(scene: &mut Scene, x: f32, y: f32, side: f32) {
        let mut path = Path::new(point(px(x), px(y)));
        path.line_to(point(px(x + side), px(y + side * 0.3)));
        path.curve_to(
            point(px(x + side * 0.2), px(y + side)),
            point(px(x + side), px(y + side)),
        );
        path.line_to(point(px(x), px(y)));
        path.content_mask = ContentMask::new(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(WIDTH as f32), px(HEIGHT as f32)),
        });
        path.color = Hsla::from(rgba(0x8800ffcc)).into();
        scene.insert_primitive(path.scale(1.));
    }

    fn scene_for(state: &str) -> Scene {
        let mut scene = Scene::default();
        quad(&mut scene);
        match state {
            "icon" | "freed" => path_at(&mut scene, 1200., 800., 24.),
            "spread" => {
                for (x, y) in [
                    (20., 20.),
                    (2500., 20.),
                    (20., 1540.),
                    (2500., 1540.),
                    (1250., 780.),
                ] {
                    path_at(&mut scene, x, y, 24.);
                }
            }
            "heavy" => {
                for i in 0..400 {
                    path_at(
                        &mut scene,
                        (i % 40) as f32 * 62.,
                        (i / 40) as f32 * 150. + 10.,
                        40.,
                    );
                }
            }
            _ => {}
        }
        scene.finish();
        scene
    }

    /// Draws each state at 2560x1600 on the headless wgpu renderer and prints
    /// the bytes its window path textures hold and the median CPU time of a
    /// frame, one line per state.
    pub fn run() {
        let extent = size(DevicePixels(WIDTH), DevicePixels(HEIGHT));
        for state in ["none", "icon", "spread", "heavy", "freed"] {
            let mut renderer = gpui_wgpu::WgpuHeadlessRenderer::new().expect("a wgpu adapter");
            let scene = scene_for(state);
            let empty = scene_for("none");
            let frames = if state == "freed" { 140 } else { 60 };
            let mut samples = Vec::new();
            for frame in 0..frames {
                let scene = if state == "freed" && frame >= 5 {
                    &empty
                } else {
                    &scene
                };
                let start = Instant::now();
                PlatformHeadlessRenderer::render_scene(&mut renderer, scene, extent)
                    .expect("frame drawn");
                if frame >= 10 {
                    samples.push(start.elapsed().as_secs_f64() * 1e3);
                }
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "state={state} path_texture_bytes={} cpu_ms={:.3}",
                renderer.path_texture_bytes(),
                samples[samples.len() / 2]
            );
        }
    }
}

fn main() {
    #[cfg(not(target_family = "wasm"))]
    bench::run();
}
