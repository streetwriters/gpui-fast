//! `#[gpui::test(render)]`: ordinary headless tests that render a window to a PNG.
//!
//! Run with `cargo test -p gpui_platform --features test-support --test screenshots`.
//! The PNGs land in `target/gpui-screenshots/<test name>/` (or `$GPUI_SCREENSHOT_DIR`).

use std::{borrow::Cow, path::Path, process::Command, time::Instant};

use anyhow::Result;
use gpui::{
    AssetSource, Context, InteractiveElement, IntoElement, ParentElement, Render, Role,
    SharedString, StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext, Window,
    div, px, rgb, size, svg,
};

/// A disc that fills the icon's square but for its corners, so the image
/// shows the icon's shape and not just a coloured box.
const SQUARE_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><circle cx="12" cy="12" r="12" fill="black"/></svg>"#;

struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok((path == "icons/disc.svg").then(|| Cow::Borrowed(SQUARE_ICON.as_bytes())))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

const WINDOW: (f32, f32) = (240., 160.);

/// Text, a coloured quad, an icon and a button, each at a known place (in
/// logical pixels):
///
/// - red quad: left 10, top 10, 80 x 40
/// - blue 32 x 32 icon: left 110, top 10
/// - "Hello" in black 32px text: left 10, top 62
/// - green "Save" button: left 130, top 110, 90 x 36
struct Sample;

impl Render for Sample {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .child(
                div()
                    .absolute()
                    .left(px(10.))
                    .top(px(10.))
                    .w(px(80.))
                    .h(px(40.))
                    .bg(rgb(0xe03030)),
            )
            .child(
                svg()
                    .path("icons/disc.svg")
                    .absolute()
                    .left(px(110.))
                    .top(px(10.))
                    .size(px(32.))
                    .text_color(rgb(0x2050e0)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(10.))
                    .top(px(62.))
                    .text_size(px(32.))
                    .text_color(rgb(0x000000))
                    .child("Hello"),
            )
            .child(
                div()
                    .id("save")
                    .absolute()
                    .left(px(130.))
                    .top(px(110.))
                    .w(px(90.))
                    .h(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(0x30a050))
                    .text_color(rgb(0xffffff))
                    .on_click(|_, _, _| {})
                    .child("Save"),
            )
    }
}

fn open_sample(cx: &mut TestAppContext) -> VisualTestContext {
    cx.set_asset_source(Icons);
    let window = cx.open_window(size(px(WINDOW.0), px(WINDOW.1)), |_, _| Sample);
    let cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    cx
}

/// The pixel at logical position (x, y), for a screenshot taken at `scale`.
fn pixel(image: &image_like::Rgba, scale: f32, x: f32, y: f32) -> [u8; 4] {
    image.get((x * scale) as u32, (y * scale) as u32)
}

fn assert_close(actual: [u8; 4], expected: [u8; 3], what: &str) {
    let close = actual[3] == 255
        && actual[..3]
            .iter()
            .zip(expected)
            .all(|(a, e)| a.abs_diff(e) <= 3);
    assert!(close, "{what}: expected rgb{expected:?}, got {actual:?}");
}

/// Reads the size from a PNG's header, so the test needs no image decoder.
fn png_size(path: &Path) -> (u32, u32) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{path:?} is not a PNG");
    let be = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    (be(16), be(20))
}

/// How many pixels in the logical rectangle differ from `background`.
fn inked(
    image: &image_like::Rgba,
    scale: f32,
    from: (f32, f32),
    to: (f32, f32),
    background: [u8; 3],
) -> usize {
    let mut count = 0;
    for y in (from.1 * scale) as u32..(to.1 * scale) as u32 {
        for x in (from.0 * scale) as u32..(to.0 * scale) as u32 {
            let p = image.get(x, y);
            if p[..3].iter().zip(background).any(|(a, b)| a.abs_diff(b) > 40) {
                count += 1;
            }
        }
    }
    count
}

/// A thin wrapper so the test names no image crate type.
mod image_like {
    pub struct Rgba {
        width: u32,
        data: Vec<u8>,
    }

    impl Rgba {
        pub fn new(width: u32, data: Vec<u8>) -> Self {
            Self { width, data }
        }

        pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
            let at = ((y * self.width + x) * 4) as usize;
            self.data[at..at + 4].try_into().unwrap()
        }
    }
}

fn shot(cx: &mut VisualTestContext) -> (image_like::Rgba, u32, u32) {
    let image = cx.screenshot_image().unwrap();
    let (w, h) = (image.width(), image.height());
    (image_like::Rgba::new(w, image.into_raw()), w, h)
}

#[gpui::test(render)]
fn sample_view_renders_to_png(cx: &mut TestAppContext) {
    let mut cx = open_sample(cx);
    let path = cx.screenshot("sample").unwrap();
    assert!(path.ends_with("sample_view_renders_to_png/sample.png"), "{path:?}");

    // The window is 240 x 160 logical pixels at the test window's scale factor.
    let scale = cx.update(|window, _| window.scale_factor());
    assert_eq!(scale, 2.0);
    let expected = ((WINDOW.0 * scale) as u32, (WINDOW.1 * scale) as u32);
    assert_eq!(png_size(&path), expected);

    let (image, w, h) = shot(&mut cx);
    assert_eq!((w, h), expected);
    assert_close(pixel(&image, scale, 50., 30.), [0xe0, 0x30, 0x30], "red quad");
    assert_close(pixel(&image, scale, 126., 26.), [0x20, 0x50, 0xe0], "blue icon");
    assert_close(pixel(&image, scale, 111., 11.), [0xff, 0xff, 0xff], "icon corner, outside its disc");
    assert_close(pixel(&image, scale, 125., 55.), [0xff, 0xff, 0xff], "background");
    assert_close(pixel(&image, scale, 135., 114.), [0x30, 0xa0, 0x50], "button, away from its text");
    assert!(
        inked(&image, scale, (10., 62.), (100., 100.), [0xff; 3]) > 100,
        "the text \"Hello\" painted nothing"
    );
    assert!(
        inked(&image, scale, (130., 110.), (220., 146.), [0x30, 0xa0, 0x50]) > 20,
        "the button's label painted nothing"
    );

    // Forced to the CPU renderer by the environment, a CPU renderer drew this
    // (the test below runs this one that way).
    if std::env::var("GPUI_TEST_RENDERER").is_ok_and(|value| value.eq_ignore_ascii_case("cpu")) {
        assert!(
            gpui_cpu_renderer::renderers_created() > 0,
            "GPUI_TEST_RENDERER=cpu, yet no CPU renderer was made"
        );
    }
}

#[gpui::test(render)]
fn size_follows_the_scale_factor(cx: &mut TestAppContext) {
    let mut cx = open_sample(cx);
    for scale in [1.0_f32, 1.5, 3.0] {
        cx.simulate_scale_factor_change(scale);
        cx.run_until_parked();
        let (image, w, h) = shot(&mut cx);
        assert_eq!(
            (w, h),
            ((WINDOW.0 * scale).round() as u32, (WINDOW.1 * scale).round() as u32),
            "scale {scale}"
        );
        assert_close(pixel(&image, scale, 50., 30.), [0xe0, 0x30, 0x30], "red quad");
        assert_close(pixel(&image, scale, 126., 26.), [0x20, 0x50, 0xe0], "blue icon");
    }
}

#[gpui::test(render)]
fn crop_to_an_element_by_role_and_name(cx: &mut TestAppContext) {
    let mut cx = open_sample(cx);
    let path = cx.screenshot_element("save-button", Role::Button, "Save").unwrap();
    assert!(path.ends_with("crop_to_an_element_by_role_and_name/save-button.png"));
    // 90 x 36 logical pixels at scale 2.
    assert_eq!(png_size(&path), (180, 72));

    let image = cx.screenshot_element_image(Role::Button, "Save").unwrap();
    assert_eq!((image.width(), image.height()), (180, 72));
    let image = image_like::Rgba::new(image.width(), image.into_raw());
    assert_close(image.get(4, 4), [0x30, 0xa0, 0x50], "the button's corner");
    assert!(inked(&image, 2., (0., 0.), (90., 36.), [0x30, 0xa0, 0x50]) > 20, "label");
}

#[gpui::test(render)]
fn an_element_crop_is_the_same_pixels_as_the_frame_cropped(cx: &mut TestAppContext) {
    let mut cx = open_sample(cx);
    let crop = cx.screenshot_element_image(Role::Button, "Save").unwrap();
    let frame = cx.screenshot_image().unwrap();
    // The "Save" button is at left 130, top 110, 90 x 36 (logical pixels): its
    // crop is exactly that place of the frame, and not the same pixels
    // anywhere else, so a crop that is placed or sized wrongly fails.
    let scale = frame.width() as f32 / WINDOW.0;
    let (left, top, width, height) = (
        (130. * scale) as u32,
        (110. * scale) as u32,
        (90. * scale) as u32,
        (36. * scale) as u32,
    );
    assert_eq!(
        crop.dimensions(),
        (width, height),
        "the crop is not the button's size"
    );
    let expected = image::imageops::crop_imm(&frame, left, top, width, height).to_image();
    assert!(
        crop == expected,
        "the crop is not the pixels of the frame at the button's place ({left}, {top})"
    );
    // The place has the button's colours, so the comparison is not of two
    // blank images: the green of its background in a corner and the white of
    // its label in the middle.
    assert_close(crop.get_pixel(2, 2).0, [0x30, 0xa0, 0x50], "the button's corner");
    assert!(
        crop.pixels().any(|pixel| pixel.0[..3] == [255, 255, 255]),
        "the button's label is not in the crop"
    );
    // A crop one pixel off is a different image: the whole frame has no
    // other place with these pixels next to the one asked for.
    let shifted = image::imageops::crop_imm(&frame, left + 1, top, width, height).to_image();
    assert!(crop != shifted, "a crop one pixel to the right looks the same");
}

#[gpui::test(render)]
fn cropping_to_a_missing_element_names_it(cx: &mut TestAppContext) {
    let mut cx = open_sample(cx);
    let error = cx
        .screenshot_element("nothing", Role::Button, "Delete")
        .unwrap_err()
        .to_string();
    assert!(error.contains("Button named \"Delete\""), "{error}");
    assert!(error.contains("accessibility tree"), "{error}");
}

#[gpui::test(render)]
fn a_missing_icon_asset_fails_the_screenshot(cx: &mut TestAppContext) {
    // No asset source: the icon would be left out of the image.
    let window = cx.open_window(size(px(WINDOW.0), px(WINDOW.1)), |_, _| Sample);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    let error = cx.screenshot("never-written").unwrap_err().to_string();
    assert!(error.contains("icons/disc.svg"), "{error}");
    assert!(error.contains("set_asset_source"), "{error}");

    // Giving it the asset fixes it.
    cx.set_asset_source(Icons);
    cx.screenshot("never-written").unwrap();
}

#[gpui::test]
fn a_test_that_did_not_opt_in_says_so(cx: &mut TestAppContext) {
    assert!(!cx.is_rendering());
    let window = cx.open_window(size(px(WINDOW.0), px(WINDOW.1)), |_, _| Sample);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    let error = cx.screenshot("sample").unwrap_err().to_string();
    assert!(error.contains("rendering is not enabled"), "{error}");
}

#[gpui::test]
fn enable_rendering_by_hand(cx: &mut TestAppContext) {
    cx.enable_rendering(gpui_platform::test_rendering());
    assert!(cx.is_rendering());
    let mut cx = open_sample(cx);
    cx.screenshot("by-hand").unwrap();
}

#[gpui::test(render)]
fn names_cannot_escape_the_folder(cx: &mut TestAppContext) {
    let mut cx = open_sample(cx);
    let path = cx.screenshot("../../etc/passwd").unwrap();
    assert!(path.ends_with("names_cannot_escape_the_folder/_.._.._etc_passwd.png"), "{path:?}");
    assert!(cx.screenshot("").is_err());
    assert!(cx.screenshot("..").is_err());
}

/// Many windows in one test, each with its own renderer.
#[gpui::test(render)]
fn many_windows_in_one_test(cx: &mut TestAppContext) {
    cx.set_asset_source(Icons);
    for ix in 0..12 {
        let window = cx.open_window(size(px(WINDOW.0), px(WINDOW.1)), |_, _| Sample);
        let mut window_cx = VisualTestContext::from_window(window.into(), cx);
        window_cx.run_until_parked();
        let path = window_cx.screenshot(&format!("window-{ix}")).unwrap();
        assert_eq!(png_size(&path), (480, 320));
    }
}

// The next few tests do the same, so cargo's test threads render at the same
// time, each with its own GPU resources.
fn render_in_parallel(cx: &mut TestAppContext, name: &str) {
    let mut cx = open_sample(cx);
    for ix in 0..5 {
        let (image, w, h) = shot(&mut cx);
        assert_eq!((w, h), (480, 320));
        assert_close(pixel(&image, 2., 50., 30.), [0xe0, 0x30, 0x30], "red quad");
        cx.screenshot(&format!("{name}-{ix}")).unwrap();
    }
}

#[gpui::test(render)]
fn parallel_a(cx: &mut TestAppContext) {
    render_in_parallel(cx, "a");
}

#[gpui::test(render)]
fn parallel_b(cx: &mut TestAppContext) {
    render_in_parallel(cx, "b");
}

#[gpui::test(render)]
fn parallel_c(cx: &mut TestAppContext) {
    render_in_parallel(cx, "c");
}

#[gpui::test(render)]
fn parallel_d(cx: &mut TestAppContext) {
    render_in_parallel(cx, "d");
}

/// What a screenshot costs; `--nocapture` prints it, and the docs quote it.
#[gpui::test(render)]
fn what_a_screenshot_costs(cx: &mut TestAppContext) {
    let start = Instant::now();
    let mut cx = open_sample(cx);
    let setup = start.elapsed();
    let first = Instant::now();
    shot(&mut cx);
    let first = first.elapsed();
    let rest = Instant::now();
    for _ in 0..20 {
        shot(&mut cx);
    }
    let rest = rest.elapsed() / 20;
    eprintln!("render setup {setup:?}, first screenshot {first:?}, later ones {rest:?} each");
    let written = Instant::now();
    cx.screenshot("cost").unwrap();
    eprintln!("a screenshot with the PNG written: {:?}", written.elapsed());
    assert!(rest.as_secs() < 2, "a render takes {rest:?}");
}

/// `GPUI_SCREENSHOT_DIR` moves the PNGs. Set in a child process running one
/// test, so the other tests' environment is untouched.
#[test]
fn the_folder_can_be_chosen_by_env_var() {
    let dir = std::env::temp_dir().join(format!("gpui-screenshots-env-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "sample_view_renders_to_png", "--test-threads=1"])
        .env("GPUI_SCREENSHOT_DIR", &dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let path = dir.join("sample_view_renders_to_png/sample.png");
    assert_eq!(png_size(&path), (480, 320));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `GPUI_TEST_RENDERER=cpu` renders on the CPU, GPU or not. Set in a child
/// process running one test, so the other tests keep their renderer.
#[test]
fn the_cpu_renderer_can_be_forced_by_env_var() {
    let dir = std::env::temp_dir().join(format!("gpui-screenshots-cpu-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "sample_view_renders_to_png", "--test-threads=1"])
        .env("GPUI_TEST_RENDERER", "cpu")
        .env("GPUI_SCREENSHOT_DIR", &dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    // The child ran the one test (a filter that matched nothing also succeeds),
    // and that test checks a CPU renderer was made.
    assert!(stdout.contains("1 passed"), "the child ran no test:\n{stdout}");
    let path = dir.join("sample_view_renders_to_png/sample.png");
    assert_eq!(png_size(&path), (480, 320));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui::test(render)]
fn the_default_folder_is_under_the_target_dir(cx: &mut TestAppContext) {
    let mut cx = open_sample(cx);
    let path = cx.screenshot("default-folder").unwrap();
    if std::env::var_os("GPUI_SCREENSHOT_DIR").is_none() {
        let exe = std::env::current_exe().unwrap();
        let target = exe.ancestors().find(|dir| dir.join("CACHEDIR.TAG").is_file()).unwrap();
        assert!(path.starts_with(target.join("gpui-screenshots")), "{path:?}");
    }
}
