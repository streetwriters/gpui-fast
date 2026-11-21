//! What a capture costs: a tester-sized window (1024x768 points at scale 2)
//! with a screen's worth of bordered, rounded and shadowed boxes, drawn whole
//! and as the crop of one box, on one thread (what the renderer cost before it
//! drew in bands) and on every core.
//!
//! Run with
//! `cargo test -p gpui_cpu_renderer --release --features runtime_shaders --test timing -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use gpui::{
    BorderStyle, Bounds, ClipMask, Corners, DevicePixels, Edges, Hsla, PlatformHeadlessRenderer,
    Quad, ScaledPixels, Scene, Shadow, point, rgb, size, solid_background,
};
use gpui_cpu_renderer::CpuHeadlessRenderer;

const SCALE: f32 = 2.;
const WIDTH: i32 = 2048;
const HEIGHT: i32 = 1536;

fn sp(value: f32) -> ScaledPixels {
    ScaledPixels(value)
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(sp(x), sp(y)),
        size: size(sp(width), sp(height)),
    }
}

fn open() -> ClipMask<ScaledPixels> {
    ClipMask::new(rect(-1000., -1000., 5000., 5000.))
}

fn quad(bounds: Bounds<ScaledPixels>, hex: u32) -> Quad {
    Quad {
        bounds,
        content_mask: open(),
        background: solid_background(Hsla::from(rgb(hex))),
        ..Default::default()
    }
}

/// A window of white with a grid of boxes 100 points wide and 50 high: side
/// borders, outlines with radii, a dashed one, a rounded clip, a shadow.
fn screen() -> Scene {
    let mut scene = Scene::default();
    scene.insert_primitive(quad(rect(0., 0., WIDTH as f32, HEIGHT as f32), 0xffffff));
    for row in 0..10 {
        for column in 0..8 {
            let (x, y) = (
                (24 + column * 120) as f32 * SCALE,
                (24 + row * 72) as f32 * SCALE,
            );
            let bounds = rect(x, y, 100. * SCALE, 50. * SCALE);
            if (row + column) % 5 == 0 {
                scene.insert_primitive(Shadow {
                    order: 0,
                    blur_radius: sp(8. * SCALE),
                    bounds,
                    corner_radii: Corners::all(sp(8. * SCALE)),
                    content_mask: open(),
                    color: Hsla::black().opacity(0.3),
                    element_bounds: bounds,
                    element_corner_radii: Corners::all(sp(8. * SCALE)),
                    inset: 0,
                    transform_index: 0,
                });
            }
            let mut boxed = quad(bounds, 0xf4f4f8);
            boxed.border_color = Hsla::from(rgb(0xff0000));
            match (row + column) % 4 {
                0 => {
                    boxed.border_widths = Edges {
                        top: sp(0.),
                        right: sp(0.),
                        bottom: sp(2. * SCALE),
                        left: sp(0.),
                    };
                }
                1 => {
                    boxed.border_widths = Edges::all(sp(3. * SCALE));
                    boxed.corner_radii = Corners::all(sp(16. * SCALE));
                }
                2 => {
                    boxed.border_widths = Edges::all(sp(2. * SCALE));
                    boxed.corner_radii = Corners::all(sp(12. * SCALE));
                    boxed.border_style_and_transform = Quad::pack_border_style(
                        BorderStyle::Dashed,
                        gpui::TransformIndex::IDENTITY,
                    );
                }
                _ => {
                    boxed.corner_radii = Corners::all(sp(20. * SCALE));
                    boxed.content_mask = ClipMask::new(rect(
                        x + 8. * SCALE,
                        y + 6. * SCALE,
                        84. * SCALE,
                        38. * SCALE,
                    ));
                }
            }
            scene.insert_primitive(boxed);
        }
    }
    scene.finish();
    scene
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn time(
    threads: usize,
    runs: usize,
    capture: impl Fn(&mut CpuHeadlessRenderer, &Scene) + Send + Sync,
) -> Duration {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| {
            let scene = screen();
            let mut renderer = CpuHeadlessRenderer::new();
            capture(&mut renderer, &scene);
            median(
                (0..runs)
                    .map(|_| {
                        let start = Instant::now();
                        capture(&mut renderer, &scene);
                        start.elapsed()
                    })
                    .collect(),
            )
        })
}

/// One of the tester's e2e specs, reduced to what it asks the renderer for: a
/// window of 1024x768 points at scale 2, and one capture of an element's
/// slot for each `shot`, `across`, `red` or `blue` call the spec makes.
struct Spec {
    name: &'static str,
    /// Captures the spec makes (counted in its `.e2e.ts`).
    captures: usize,
    /// The size of an element's slot, in points.
    slot: (f32, f32),
    /// Draws the element number `i` of the spec's screen into its slot.
    element: fn(&mut Scene, Bounds<ScaledPixels>, usize),
}

/// `border-bottom`, `border-bottom-radius`, `border-bottom-dashed`: a white
/// box with a red edge on its bottom only, rounded or dashed in turn.
fn side_borders(scene: &mut Scene, slot: Bounds<ScaledPixels>, i: usize) {
    let mut boxed = quad(slot, 0xffffff);
    boxed.border_color = Hsla::from(rgb(0xff0000));
    boxed.border_widths = Edges {
        top: sp(0.),
        right: sp(0.),
        bottom: sp(2. * SCALE),
        left: sp(0.),
    };
    match i % 3 {
        1 => boxed.corner_radii = Corners::all(sp(12. * SCALE)),
        2 => {
            boxed.border_style_and_transform =
                Quad::pack_border_style(BorderStyle::Dashed, gpui::TransformIndex::IDENTITY)
        }
        _ => {}
    }
    scene.insert_primitive(boxed);
}

/// `outline-square`, `outline-rounded`, `outline-dashed`: a ring drawn
/// around a view, with no fill, square, rounded or dashed in turn.
fn outline_radius(scene: &mut Scene, slot: Bounds<ScaledPixels>, i: usize) {
    scene.insert_primitive(quad(slot, 0xffffff));
    let ring = Bounds {
        origin: point(slot.origin.x + sp(14. * SCALE), slot.origin.y + sp(14. * SCALE)),
        size: size(slot.size.width - sp(28. * SCALE), slot.size.height - sp(28. * SCALE)),
    };
    let mut outline = quad(ring, 0xffffff);
    outline.background = solid_background(Hsla::transparent_black());
    outline.border_color = Hsla::from(rgb(0xff0000));
    outline.border_widths = Edges::all(sp(3. * SCALE));
    match i % 3 {
        1 => outline.corner_radii = Corners::all(sp(16. * SCALE)),
        2 => {
            outline.border_style_and_transform =
                Quad::pack_border_style(BorderStyle::Dashed, gpui::TransformIndex::IDENTITY)
        }
        _ => {}
    }
    scene.insert_primitive(outline);
}

/// `clip-radius-hidden`, `clip-rect`, ...: a red view cut by a rounded
/// clip, as large as the slot.
fn rounded_clip(scene: &mut Scene, slot: Bounds<ScaledPixels>, i: usize) {
    let mut red = quad(slot, 0xff0000);
    let mut mask = ClipMask::new(slot);
    if i % 3 != 1 {
        mask.rounded_bounds = slot;
        mask.corner_radii = Corners::all(sp(100. * SCALE));
    }
    red.content_mask = mask;
    scene.insert_primitive(red);
}

const SPECS: [Spec; 3] = [
    Spec { name: "side-borders", captures: 13, slot: (100., 50.), element: side_borders },
    Spec { name: "outline-radius", captures: 25, slot: (100., 100.), element: outline_radius },
    Spec { name: "rounded-clip", captures: 7, slot: (200., 200.), element: rounded_clip },
];

/// The spec's screen, and the slots its captures cycle through.
fn spec_screen(spec: &Spec) -> (Scene, Vec<Bounds<DevicePixels>>) {
    let mut scene = Scene::default();
    scene.insert_primitive(quad(rect(0., 0., WIDTH as f32, HEIGHT as f32), 0xffffff));
    let (width, height) = spec.slot;
    let columns = (1024. / (width + 20.)) as usize;
    let rows = (768. / (height + 20.)) as usize;
    let mut slots = Vec::new();
    for index in 0..columns * rows {
        let (x, y) = (
            (10. + (index % columns) as f32 * (width + 20.)) * SCALE,
            (10. + (index / columns) as f32 * (height + 20.)) * SCALE,
        );
        (spec.element)(&mut scene, rect(x, y, width * SCALE, height * SCALE), index);
        slots.push(Bounds {
            origin: point(DevicePixels(x as i32), DevicePixels(y as i32)),
            size: size(
                DevicePixels((width * SCALE) as i32),
                DevicePixels((height * SCALE) as i32),
            ),
        });
    }
    scene.finish();
    (scene, slots)
}

/// All of a spec's captures, in `threads` threads: of the whole frame (what
/// a capture cost before elements were cropped) or of the element only.
fn time_spec(spec: &Spec, threads: usize, cropped: bool) -> Duration {
    let frame = size(DevicePixels(WIDTH), DevicePixels(HEIGHT));
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap()
        .install(|| {
            let (scene, slots) = spec_screen(spec);
            let mut renderer = CpuHeadlessRenderer::new();
            let mut run = || {
                let start = Instant::now();
                for capture in 0..spec.captures {
                    let slot = slots[capture % slots.len()];
                    let image = if cropped {
                        renderer.render_scene_region_to_image(&scene, frame, slot)
                    } else {
                        renderer.render_scene_to_image(&scene, frame)
                    };
                    std::hint::black_box(image.unwrap());
                }
                start.elapsed()
            };
            run();
            median((0..9).map(|_| run()).collect())
        })
}

#[test]
#[ignore = "timing, run by hand in release mode"]
fn what_the_tester_specs_cost() {
    let cores = std::thread::available_parallelism().unwrap().get();
    println!("CPU time of every capture a tester spec makes, 1024x768 points at scale 2, {cores} cores");
    println!("(the scene is drawn and the pixels are cropped, without the PNG encode and the IPC)");
    for spec in &SPECS {
        let before = time_spec(spec, 1, false);
        let threaded = time_spec(spec, cores, false);
        let cropped = time_spec(spec, 1, true);
        let both = time_spec(spec, cores, true);
        println!(
            "{:<15} {:>2} captures: whole frame on one thread {before:>9.2?} | whole frame on {cores} threads {threaded:>9.2?} | element on one thread {cropped:>9.2?} | element on {cores} threads {both:>9.2?} | {:.1}x faster than before",
            spec.name,
            spec.captures,
            before.as_secs_f64() / both.as_secs_f64()
        );
    }
}

#[test]
#[ignore = "timing, run by hand in release mode"]
fn what_a_capture_costs() {
    let frame = size(DevicePixels(WIDTH), DevicePixels(HEIGHT));
    let one_box = Bounds {
        origin: point(DevicePixels(24 * 2 + 120 * 2 * 2), DevicePixels(24 * 2 + 72 * 2 * 3)),
        size: size(DevicePixels(200), DevicePixels(100)),
    };
    let cores = std::thread::available_parallelism().unwrap().get();
    let whole = |renderer: &mut CpuHeadlessRenderer, scene: &Scene| {
        renderer.render_scene_to_image(scene, frame).unwrap();
    };
    let crop = |renderer: &mut CpuHeadlessRenderer, scene: &Scene| {
        renderer
            .render_scene_region_to_image(scene, frame, one_box)
            .unwrap();
    };
    let before = time(1, 15, whole);
    let threaded = time(cores, 15, whole);
    let cropped = time(1, 15, crop);
    let both = time(cores, 15, crop);
    println!("{WIDTH}x{HEIGHT} frame, crop of {:?}, {cores} cores", one_box.size);
    println!("whole frame, one thread (as before): {before:?}");
    println!("whole frame, {cores} threads:          {threaded:?}");
    println!("crop, one thread:                    {cropped:?}");
    println!("crop, {cores} threads (what a test asks for): {both:?}");
    println!(
        "speed-up of an element capture over before: {:.1}x",
        before.as_secs_f64() / both.as_secs_f64()
    );
}
