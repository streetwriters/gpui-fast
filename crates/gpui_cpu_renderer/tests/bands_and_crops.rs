//! Drawing across threads and drawing a crop must give the pixels that drawing
//! the whole scene on one thread does. Both scenes of `common` are used: the
//! first has the large primitives, the second the opaque solid quads that are
//! filled in spans, with fractional edges, cut by masks, off the frame and
//! under a transform.

mod common;

use gpui::{Bounds, DevicePixels, PlatformHeadlessRenderer, Scene, point, size};
use gpui_cpu_renderer::CpuHeadlessRenderer;

/// Both scenes, by name.
fn scenes() -> [(&'static str, fn() -> Scene); 2] {
    [("scene", common::scene), ("scene_extras", common::scene_extras)]
}

fn render(scene: fn() -> Scene) -> image::RgbaImage {
    CpuHeadlessRenderer::new()
        .render_scene_to_image(&scene(), common::frame_size())
        .expect("the scene renders")
}

fn in_pool<T: Send>(threads: usize, work: impl FnOnce() -> T + Send) -> T {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("a pool")
        .install(work)
}

fn region(x: i32, y: i32, width: i32, height: i32) -> Bounds<DevicePixels> {
    Bounds {
        origin: point(DevicePixels(x), DevicePixels(y)),
        size: size(DevicePixels(width), DevicePixels(height)),
    }
}

fn region_of(scene: fn() -> Scene, region: Bounds<DevicePixels>) -> anyhow::Result<image::RgbaImage> {
    CpuHeadlessRenderer::new().render_scene_region_to_image(&scene(), common::frame_size(), region)
}

#[test]
fn bands_on_many_threads_are_the_pixels_of_one_thread() {
    for (name, scene) in scenes() {
        let one = in_pool(1, || render(scene));
        for threads in [2, 3, 8] {
            let many = in_pool(threads, || render(scene));
            assert!(
                many == one,
                "{name} drawn on {threads} threads differs from drawing on one"
            );
        }
    }
}

#[test]
fn a_crop_is_the_same_pixels_as_the_whole_frame_cropped() {
    for (name, scene) in scenes() {
        let whole = in_pool(1, || render(scene));
        // Inside, on each edge, a single pixel, a sliver, and the whole frame.
        // The slivers and the small crops cut the quads of `scene_extras` mid
        // span, at the pixel its fractional edges end on.
        for (x, y, width, height) in [
            (40, 40, 100, 60),
            (0, 0, 64, 48),
            (336, 252, 64, 48),
            (0, 0, 400, 300),
            (199, 123, 1, 1),
            (0, 140, 400, 3),
            (205, 0, 2, 300),
            (31, 151, 77, 101),
            (10, 12, 91, 51),
            (11, 13, 89, 49),
            (60, 30, 71, 41),
            (125, 95, 91, 51),
            (381, 0, 19, 30),
            (0, 250, 40, 50),
        ] {
            let crop = in_pool(4, || region_of(scene, region(x, y, width, height)))
                .expect("the crop renders");
            let expected = image::imageops::crop_imm(
                &whole,
                x as u32,
                y as u32,
                width as u32,
                height as u32,
            )
            .to_image();
            assert!(
                crop == expected,
                "{name}: the crop ({x}, {y}) {width}x{height} differs from the whole frame cropped"
            );
        }
    }
}

#[test]
fn a_crop_that_hangs_over_the_frame_is_cut_to_it() {
    for (name, scene) in scenes() {
        let whole = render(scene);
        let crop = region_of(scene, region(350, 250, 200, 200)).expect("the crop renders");
        assert_eq!(crop.dimensions(), (50, 50));
        assert!(
            crop == image::imageops::crop_imm(&whole, 350, 250, 50, 50).to_image(),
            "{name}: the crop over the bottom right differs"
        );
        let crop = region_of(scene, region(-30, -20, 80, 60)).expect("the crop renders");
        assert_eq!(crop.dimensions(), (50, 40));
        assert!(
            crop == image::imageops::crop_imm(&whole, 0, 0, 50, 40).to_image(),
            "{name}: the crop over the top left differs"
        );
    }
}

#[test]
fn a_crop_outside_the_frame_or_without_size_is_an_error() {
    for (x, y, width, height) in [(400, 0, 10, 10), (0, 300, 10, 10), (10, 10, 0, 5), (-50, 0, 20, 20)] {
        assert!(
            region_of(common::scene, region(x, y, width, height)).is_err(),
            "({x}, {y}) {width}x{height} has no pixels in the frame"
        );
    }
}

#[test]
fn a_crop_is_drawn_without_drawing_the_rest() {
    // A crop of a corner costs far less than the frame: both are timed
    // many times over, so a scheduling hiccup cannot flip the comparison.
    let mut renderer = CpuHeadlessRenderer::new();
    let scene = common::scene();
    let time = |region: Option<Bounds<DevicePixels>>, renderer: &mut CpuHeadlessRenderer| {
        let start = std::time::Instant::now();
        for _ in 0..20 {
            match region {
                Some(region) => {
                    renderer
                        .render_scene_region_to_image(&scene, common::frame_size(), region)
                        .unwrap();
                }
                None => {
                    renderer
                        .render_scene_to_image(&scene, common::frame_size())
                        .unwrap();
                }
            }
        }
        start.elapsed()
    };
    let whole = time(None, &mut renderer);
    let corner = time(Some(region(0, 0, 16, 16)), &mut renderer);
    assert!(
        corner * 4 < whole,
        "a 16x16 crop took {corner:?}, the whole frame {whole:?}"
    );
}
