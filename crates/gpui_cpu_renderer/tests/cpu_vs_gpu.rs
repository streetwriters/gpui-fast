//! The same scene rendered by the platform's GPU renderer and by the CPU one,
//! side by side: both PNGs, a diff image, and how far apart they are.
//!
//! Run with `cargo test -p gpui_cpu_renderer --features runtime_shaders --test cpu_vs_gpu -- --nocapture`
//! (`runtime_shaders` only matters on macOS, where it compiles the Metal shaders
//! at run time, so no Xcode toolchain is needed).
//! Where there's no GPU the test says so and passes without comparing.
//! The images and `report.txt` land in `<screenshots>/cpu_and_gpu_render_the_same_scene/`.
//!
//! The CPU renderer's shader maths (`src/shade.rs`) is a third copy of the Metal
//! and wgpu shaders'. This is the check that it hasn't drifted from them: each
//! element below is measured on its own, and on a machine with a GPU the test
//! fails when one of them is further from the GPU's drawing than its budget.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{
    AssetSource, BoxShadow, ColorSpace, Context, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext,
    Window, black, checkerboard, div, hsla, linear_color_stop, linear_gradient, pattern_slash,
    point, px, rgb, size, svg,
};

const DISC: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><circle cx="12" cy="12" r="12" fill="black"/></svg>"#;

struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok((path == "icons/disc.svg").then(|| Cow::Borrowed(DISC.as_bytes())))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// A pixel counts as different when a channel is further than this from the
/// other renderer's (out of 255).
const THRESHOLD: u8 = 16;

/// What a typical interface is made of: a card with a shadow, a border and
/// rounded corners, gradients, per-side borders, a dashed border, a disc icon,
/// text in several sizes and a wavy underline; and below those, the less common
/// things: a slash pattern, a checkerboard, an inset shadow, a rounded
/// `overflow_hidden` clip and a scrolled list.
struct Scene;

impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xf2f4f8))
            .child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(20.))
                    .w(px(160.))
                    .h(px(100.))
                    .rounded(px(14.))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0x3b6fe0))
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 0., 0.35),
                        offset: point(px(0.), px(6.)),
                        blur_radius: px(16.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .p(px(10.))
                    .text_size(px(16.))
                    .text_color(black())
                    .child("Card title"),
            )
            .child(
                div()
                    .absolute()
                    .left(px(200.))
                    .top(px(20.))
                    .w(px(160.))
                    .h(px(40.))
                    .rounded(px(8.))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(rgb(0xe03030), 0.),
                        linear_color_stop(rgb(0x3050e0), 1.),
                    )),
            )
            .child(
                div()
                    .absolute()
                    .left(px(200.))
                    .top(px(76.))
                    .w(px(160.))
                    .h(px(44.))
                    .border_color(rgb(0x30a050))
                    .border_t(px(1.))
                    .border_l(px(4.))
                    .border_b(px(8.))
                    .border_r(px(2.))
                    .bg(rgb(0xe6f5ea)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(140.))
                    .w(px(100.))
                    .h(px(50.))
                    .rounded(px(6.))
                    .border_2()
                    .border_dashed()
                    .border_color(rgb(0x8040c0)),
            )
            .child(
                svg()
                    .path("icons/disc.svg")
                    .absolute()
                    .left(px(140.))
                    .top(px(146.))
                    .size(px(36.))
                    .text_color(rgb(0xd07020)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(200.))
                    .top(px(140.))
                    .flex()
                    .flex_col()
                    .text_color(rgb(0x202020))
                    .child(div().text_size(px(24.)).child("Quick brown fox"))
                    .child(div().text_size(px(12.)).child("jumps over the lazy dog"))
                    .child(
                        div()
                            .text_size(px(14.))
                            .underline()
                            .text_decoration_wavy()
                            .text_decoration_color(rgb(0xe03030))
                            .child("wavy underline"),
                    ),
            )
            // Second row: patterns and a gradient mixed in Oklab.
            .child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(250.))
                    .w(px(100.))
                    .h(px(40.))
                    .bg(pattern_slash(rgb(0x3050e0), 2., 6.)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(130.))
                    .top(px(250.))
                    .w(px(60.))
                    .h(px(40.))
                    .bg(checkerboard(rgb(0x30a050), 10.)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(200.))
                    .top(px(250.))
                    .w(px(160.))
                    .h(px(40.))
                    .rounded(px(8.))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(rgb(0x30e0a0), 0.),
                        linear_color_stop(rgb(0xe030c0), 1.),
                    )
                    .color_space(ColorSpace::Oklab)),
            )
            // Third row: an inset shadow, a rounded clip and a scrolled list.
            .child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(310.))
                    .w(px(100.))
                    .h(px(50.))
                    .rounded(px(10.))
                    .bg(rgb(0xffffff))
                    .shadow(vec![BoxShadow {
                        color: hsla(0., 0., 0., 0.5),
                        offset: point(px(0.), px(4.)),
                        blur_radius: px(10.),
                        spread_radius: px(0.),
                        inset: true,
                    }]),
            )
            .child(
                div()
                    .absolute()
                    .left(px(140.))
                    .top(px(310.))
                    .w(px(50.))
                    .h(px(50.))
                    .rounded(px(25.))
                    .overflow_hidden()
                    .bg(rgb(0xdddddd))
                    .child(
                        div()
                            .absolute()
                            .left(px(-10.))
                            .top(px(15.))
                            .w(px(80.))
                            .h(px(20.))
                            .bg(rgb(0xe03030)),
                    ),
            )
            .child(
                div()
                    .id("list")
                    .absolute()
                    .left(px(200.))
                    .top(px(310.))
                    .w(px(160.))
                    .h(px(50.))
                    .overflow_y_scroll()
                    .bg(rgb(0xffffff))
                    .text_size(px(12.))
                    .text_color(rgb(0x202020))
                    .child(div().h(px(25.)).bg(rgb(0xfde0e0)).child("first row"))
                    .child(div().h(px(25.)).bg(rgb(0xe0fde0)).child("second row"))
                    .child(div().h(px(25.)).bg(rgb(0xe0e0fd)).child("third row")),
            )
    }
}

const WINDOW: (f32, f32) = (400., 380.);

fn open_scene(cx: &mut TestAppContext, rendering: gpui::TestRendering) -> VisualTestContext {
    cx.enable_rendering(rendering);
    cx.set_asset_source(Icons);
    let window = cx.open_window(size(px(WINDOW.0), px(WINDOW.1)), |_, _| Scene);
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    cx
}

/// How far two images of the same size are from each other.
struct Difference {
    pixels: usize,
    mean: f64,
    max: u8,
    over_threshold: usize,
    diff: image::RgbaImage,
}

fn compare(a: &image::RgbaImage, b: &image::RgbaImage) -> Difference {
    assert_eq!(a.dimensions(), b.dimensions());
    let (width, height) = a.dimensions();
    let mut diff = image::RgbaImage::new(width, height);
    let (mut total, mut max, mut over) = (0u64, 0u8, 0usize);
    for (x, y, pa) in a.enumerate_pixels() {
        let pb = b.get_pixel(x, y);
        let deltas: [u8; 4] = std::array::from_fn(|ix| pa.0[ix].abs_diff(pb.0[ix]));
        total += deltas.iter().map(|d| *d as u64).sum::<u64>();
        let worst = *deltas.iter().max().unwrap();
        max = max.max(worst);
        if worst > THRESHOLD {
            over += 1;
        }
        // White where the renderers agree, darker red the further apart.
        let shade = 255 - worst;
        diff.put_pixel(x, y, image::Rgba([255, shade, shade, 255]));
    }
    let pixels = (width * height) as usize;
    Difference {
        pixels,
        mean: total as f64 / (pixels * 4) as f64,
        max,
        over_threshold: over,
        diff,
    }
}

/// The share of an element's pixels that may be over the threshold before the
/// test fails. On Metal every element measures 0%; this leaves room for another
/// GPU's rounding but not for a shader that has drifted.
const BUDGET: f64 = 0.01;

/// An element of the scene and where it is, in logical pixels: x0, y0, x1, y1.
struct Region {
    name: &'static str,
    area: [u32; 4],
}

const REGIONS: [Region; 13] = [
    Region { name: "card with shadow", area: [0, 0, 195, 140] },
    Region { name: "gradient", area: [200, 20, 360, 60] },
    Region { name: "per-side borders", area: [200, 76, 360, 120] },
    Region { name: "dashed border", area: [20, 140, 120, 190] },
    Region { name: "icon", area: [140, 146, 176, 182] },
    Region { name: "text", area: [200, 140, 400, 240] },
    Region { name: "slash pattern", area: [20, 250, 120, 290] },
    Region { name: "checkerboard", area: [130, 250, 190, 290] },
    Region { name: "Oklab gradient", area: [200, 250, 360, 290] },
    Region { name: "inset shadow", area: [20, 310, 120, 360] },
    Region { name: "rounded clip", area: [140, 310, 190, 360] },
    Region { name: "scrolled list", area: [200, 310, 360, 360] },
    Region { name: "empty background", area: [380, 0, 400, 130] },
];

/// The difference per element: how many of its pixels differ at all, how many
/// are over the threshold, and by how much. Returns the report and the names
/// of the elements over their budget.
fn breakdown(a: &image::RgbaImage, b: &image::RgbaImage, scale: u32) -> (String, Vec<String>) {
    let mut out = String::from(
        "by element (pixels differing at all / pixels, over threshold, mean, max):\n",
    );
    let mut over_budget = Vec::new();
    for Region { name, area: [x0, y0, x1, y1] } in REGIONS {
        let (mut differing, mut over, mut pixels, mut total, mut max) =
            (0usize, 0usize, 0usize, 0u64, 0u8);
        for y in y0 * scale..y1 * scale {
            for x in x0 * scale..x1 * scale {
                let (pa, pb) = (a.get_pixel(x, y), b.get_pixel(x, y));
                let deltas: [u8; 4] = std::array::from_fn(|ix| pa.0[ix].abs_diff(pb.0[ix]));
                let worst = *deltas.iter().max().unwrap();
                pixels += 1;
                total += deltas.iter().map(|d| *d as u64).sum::<u64>();
                max = max.max(worst);
                differing += (worst > 0) as usize;
                over += (worst > THRESHOLD) as usize;
            }
        }
        let share = over as f64 / pixels as f64;
        if share > BUDGET {
            over_budget.push(format!(
                "{name}: {:.2}% over the threshold, budget {:.0}%",
                share * 100.,
                BUDGET * 100.
            ));
        }
        out += &format!(
            "  {name}: {differing} / {pixels}, {over} over, mean {:.3}, max {max}\n",
            total as f64 / (pixels * 4) as f64
        );
    }
    (out, over_budget)
}

#[gpui::test]
fn cpu_and_gpu_render_the_same_scene(gpu: &mut TestAppContext, cpu: &mut TestAppContext) {
    let mut cpu = open_scene(cpu, gpui_platform::cpu_test_rendering());
    let cpu_path = cpu.screenshot("cpu").unwrap();
    let cpu_image = cpu.screenshot_image().unwrap();

    if !matches!(gpui_platform::current_headless_renderer(), Ok(Some(_))) {
        eprintln!("no GPU renderer here, so nothing to compare the CPU one with");
        return;
    }
    let mut gpu = open_scene(gpu, gpui_platform::gpu_test_rendering());
    gpu.screenshot("gpu").unwrap();
    let gpu_image = gpu.screenshot_image().unwrap();

    let difference = compare(&gpu_image, &cpu_image);
    let dir = cpu_path.parent().unwrap();
    difference.diff.save(dir.join("difference.png")).unwrap();
    let (elements, over_budget) = breakdown(&gpu_image, &cpu_image, 2);
    let report = format!(
        "{}x{} pixels\nmean difference per channel: {:.3} / 255\nmax difference: {} / 255\npixels with a channel more than {THRESHOLD}/255 apart: {} of {} ({:.2}%)\n{elements}",
        gpu_image.width(),
        gpu_image.height(),
        difference.mean,
        difference.max,
        difference.over_threshold,
        difference.pixels,
        100. * difference.over_threshold as f64 / difference.pixels as f64,
    );
    std::fs::write(dir.join("report.txt"), &report).unwrap();
    eprintln!("{report}");

    let share = difference.over_threshold as f64 / difference.pixels as f64;
    assert!(
        share < 0.05,
        "the renderers disagree on {:.1}% of the pixels\n{report}",
        share * 100.
    );
    assert!(
        over_budget.is_empty(),
        "elements further from the GPU's drawing than their budget:\n{}\n{report}",
        over_budget.join("\n")
    );
}
