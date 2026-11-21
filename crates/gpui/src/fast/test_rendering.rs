//! Letting an ordinary headless test render its windows to images.
//!
//! A `#[gpui::test]` context has no renderer: it draws a window's scene but
//! rasterizes nothing, and uses a text system that measures nothing. A test
//! opts in with `#[gpui::test(render)]` or `cx.enable_rendering(..)`, which
//! gives it a real text system and the platform's offscreen renderer (Metal
//! on macOS, wgpu elsewhere, or a CPU renderer where there is no GPU or when
//! `GPUI_TEST_RENDERER=cpu` is set). No window opens and no screen-recording
//! permission is needed. Tests that do not opt in are unchanged.
//!
//! Once on, `cx.screenshot("name")` renders the window's current frame to a
//! PNG and returns its path, and `cx.screenshot_element("name", role, label)`
//! writes only the bounds of the element found by role and name through the
//! accessibility tree (see [`crate::fast::ui_test`]).
//!
//! The renderer comes from `gpui_platform`, which this crate cannot depend
//! on, so `#[gpui::test(render)]` expands to
//! `cx.enable_rendering(gpui_platform::test_rendering())`.
//!
//! See `docs/ui-testing.md`.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::Arc,
};

use accesskit::Role;
use anyhow::{Context as _, Result, anyhow, bail};
use image::RgbaImage;

use crate::{
    AssetSource, Bounds, DevicePixels, Pixels, PlatformHeadlessRenderer, PlatformTextSystem,
    Scene, SharedString, Size, SvgRenderer, TestAppContext, TextSystem, VisualTestContext, point,
    size,
};

/// The device pixels an element's `bounds` cover in a window of `size` (the
/// whole pixel an edge falls in), or `None` when none of them are in the window.
pub(crate) fn element_region(
    bounds: Bounds<Pixels>,
    scale: f32,
    window: Size<DevicePixels>,
) -> Option<Bounds<DevicePixels>> {
    let (width, height) = (window.width.0.max(0) as u32, window.height.0.max(0) as u32);
    let left = ((bounds.left().0 * scale).floor().max(0.) as u32).min(width);
    let top = ((bounds.top().0 * scale).floor().max(0.) as u32).min(height);
    let right = ((bounds.right().0 * scale).ceil().max(0.) as u32).min(width);
    let bottom = ((bounds.bottom().0 * scale).ceil().max(0.) as u32).min(height);
    if right <= left || bottom <= top {
        return None;
    }
    Some(Bounds {
        origin: point(DevicePixels(left as i32), DevicePixels(top as i32)),
        size: size(
            DevicePixels((right - left) as i32),
            DevicePixels((bottom - top) as i32),
        ),
    })
}

/// Draws `region` of a test window's frame with its offscreen renderer.
pub(crate) fn render_region(
    renderer: &mut Option<Box<dyn PlatformHeadlessRenderer>>,
    size: Size<DevicePixels>,
    scene: &Scene,
    region: Bounds<DevicePixels>,
) -> Result<RgbaImage> {
    match renderer {
        Some(renderer) => renderer.render_scene_region_to_image(scene, size, region),
        None => bail!("render_to_image not available: no HeadlessRenderer configured"),
    }
}

/// Draws `region` of a frame of the test window `window` with its renderer.
pub(crate) fn render_test_window_region(
    window: &crate::TestWindow,
    scene: &Scene,
    region: Bounds<DevicePixels>,
) -> Result<RgbaImage> {
    let scale_factor = crate::PlatformWindow::scale_factor(window);
    let mut state = window.0.lock();
    let size = state.bounds.size.to_device_pixels(scale_factor);
    render_region(&mut state.renderer, size, scene, region)
}

impl crate::Window {
    /// Like `render_to_image`, for `region` of the frame only.
    pub fn render_region_to_image(&self, region: Bounds<DevicePixels>) -> Result<RgbaImage> {
        self.platform_window
            .render_region_to_image(&self.rendered_frame.scene, region)
    }
}

/// What a test needs to render its windows: a real text system and a factory
/// for the platform's offscreen renderer. `gpui_platform::test_rendering()`
/// builds the one for the current platform.
pub struct TestRendering {
    text_system: Arc<dyn PlatformTextSystem>,
    renderer_factory: Box<dyn Fn() -> Result<Option<Box<dyn PlatformHeadlessRenderer>>>>,
}

impl TestRendering {
    /// `renderer_factory` is called once for every window the test opens.
    pub fn new(
        text_system: Arc<dyn PlatformTextSystem>,
        renderer_factory: impl Fn() -> Result<Option<Box<dyn PlatformHeadlessRenderer>>> + 'static,
    ) -> Self {
        Self {
            text_system,
            renderer_factory: Box::new(renderer_factory),
        }
    }
}

type RendererFactory = Box<dyn Fn() -> Result<Option<Box<dyn PlatformHeadlessRenderer>>>>;

/// What the test platform renders with: the offscreen renderer its windows
/// get (given when it is built, or by [`TestAppContext::enable_rendering`])
/// and the text system a test swapped in.
pub(crate) struct Installed {
    text_system: RefCell<Option<Arc<dyn PlatformTextSystem>>>,
    renderer_factory: RefCell<Option<RendererFactory>>,
}

impl Installed {
    pub(crate) fn new(renderer_factory: Option<RendererFactory>) -> Self {
        Self {
            text_system: RefCell::new(None),
            renderer_factory: RefCell::new(renderer_factory),
        }
    }

    pub(crate) fn text_system(installed: &Self) -> Option<Arc<dyn PlatformTextSystem>> {
        installed.text_system.borrow().clone()
    }

    pub(crate) fn renderer(
        installed: &Self,
    ) -> Result<Option<Box<dyn PlatformHeadlessRenderer>>> {
        match installed.renderer_factory.borrow().as_ref() {
            Some(factory) => factory(),
            None => Ok(None),
        }
    }
}

thread_local! {
    /// SVG paths this thread tried to paint and found no asset for.
    static MISSING_SVGS: RefCell<Vec<SharedString>> = const { RefCell::new(Vec::new()) };
}

/// Called when painting an SVG finds no asset at its path, so a screenshot
/// can say so instead of showing an empty space.
pub(crate) fn note_missing_svg(path: &SharedString) {
    MISSING_SVGS.with(|missing| {
        let mut missing = missing.borrow_mut();
        if !missing.contains(path) {
            missing.push(path.clone());
        }
    });
}

fn take_missing_svgs() -> Vec<SharedString> {
    MISSING_SVGS.with(|missing| std::mem::take(&mut *missing.borrow_mut()))
}

impl TestAppContext {
    /// Lets this test's windows render to images: gives the context a real
    /// text system and the offscreen renderer in `rendering`, so
    /// [`VisualTestContext::screenshot`] works. Call it before opening a
    /// window. `#[gpui::test(render)]` does this at the start of the test.
    pub fn enable_rendering(&mut self, rendering: TestRendering) {
        assert!(
            !self.app.borrow().windows.values().any(Option::is_some),
            "enable_rendering must be called before the test opens a window"
        );
        let TestRendering {
            text_system,
            renderer_factory,
        } = rendering;
        let installed = &self.test_platform.fast_rendering;
        *installed.text_system.borrow_mut() = Some(text_system.clone());
        *installed.renderer_factory.borrow_mut() = Some(renderer_factory);
        let text_system = Arc::new(TextSystem::new(text_system));
        self.app.borrow_mut().text_system = text_system.clone();
        self.text_system = text_system;
        take_missing_svgs();
    }

    /// Whether [`Self::enable_rendering`] was called.
    pub fn is_rendering(&self) -> bool {
        self.test_platform
            .fast_rendering
            .renderer_factory
            .borrow()
            .is_some()
    }

    /// Sets where `svg().path(..)` and `img(..)` find their assets. Without
    /// one, an SVG icon paints nothing, and a screenshot of a window that
    /// has one fails saying which path is missing.
    pub fn set_asset_source(&mut self, asset_source: impl AssetSource) {
        let asset_source: Arc<dyn AssetSource> = Arc::new(asset_source);
        let mut app = self.app.borrow_mut();
        app.svg_renderer = SvgRenderer::new(asset_source.clone());
        app.asset_source = asset_source;
    }
}

/// The folder screenshots go in: `GPUI_SCREENSHOT_DIR` if set, else
/// `gpui-screenshots` in the cargo target directory.
pub fn screenshot_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("GPUI_SCREENSHOT_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    target_dir().join("gpui-screenshots")
}

/// The cargo target directory: the nearest folder above the running test
/// binary that cargo tagged with a `CACHEDIR.TAG`.
fn target_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors().skip(1) {
            if dir.join("CACHEDIR.TAG").is_file() {
                return dir.to_path_buf();
            }
        }
    }
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    std::env::temp_dir()
}

/// `name` made safe to be a file name: anything but letters, digits, `-`,
/// `_`, `.` and spaces becomes `_`, a leading `.` gets a `_` before it, and a
/// trailing `.png` is dropped.
fn file_stem(name: &str) -> Result<String> {
    let name = name.strip_suffix(".png").unwrap_or(name);
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if stem.trim_matches(|c| c == '.' || c == ' ').is_empty() {
        bail!("a screenshot needs a name, got {name:?}");
    }
    // Not a hidden file, and never `..`.
    Ok(if stem.starts_with('.') {
        format!("_{stem}")
    } else {
        stem
    })
}

fn screenshot_path(test_name: Option<&str>, name: &str) -> Result<PathBuf> {
    let folder = file_stem(test_name.unwrap_or("test"))?;
    Ok(screenshot_dir()
        .join(folder)
        .join(format!("{}.png", file_stem(name)?)))
}

fn save(image: &RgbaImage, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating the screenshot folder {}", parent.display()))?;
    }
    image
        .save_with_format(path, image::ImageFormat::Png)
        .with_context(|| format!("writing the screenshot {}", path.display()))
}

impl VisualTestContext {
    /// Renders the window's current frame to an image the size of the window
    /// in device pixels (its size times its scale factor).
    ///
    /// Fails when the test did not enable rendering, when the platform has
    /// no offscreen renderer, or when the frame paints an SVG whose asset
    /// the test did not supply (see [`TestAppContext::set_asset_source`]).
    pub fn screenshot_image(&mut self) -> Result<RgbaImage> {
        self.render_frame(None)
    }

    /// The frame, or with `element` (its bounds, role and name) only the
    /// element's bounds, drawn without drawing the rest of the window.
    fn render_frame(
        &mut self,
        element: Option<(Bounds<Pixels>, Role, &str)>,
    ) -> Result<RgbaImage> {
        if !self.is_rendering() {
            bail!(
                "this test can't take a screenshot: rendering is not enabled. Use \
                 `#[gpui::test(render)]`, or call `cx.enable_rendering(..)` before \
                 opening the window"
            );
        }
        // Draw the frame again, so a missing asset is noted by this frame and
        // not left over from an earlier one.
        take_missing_svgs();
        self.update(|window, _| window.refresh());
        crate::fast::ui_test::settle(self);
        let image = self
            .update(|window, _| match element {
                None => window.render_to_image(),
                Some((bounds, role, label)) => {
                    let scale = window.scale_factor();
                    let size = window.bounds().size.to_device_pixels(scale);
                    let Some(region) = element_region(bounds, scale, size) else {
                        bail!(
                            "the {role:?} named {label:?} is outside the window or has no size \
                             (bounds {bounds:?}), so there is nothing to crop to"
                        );
                    };
                    window.render_region_to_image(region)
                }
            })
            .map_err(|error| {
                if error.to_string().contains("no HeadlessRenderer configured") {
                    anyhow!(
                        "this platform has no offscreen renderer to take a screenshot with"
                    )
                } else {
                    error
                }
            })?;
        let missing = take_missing_svgs();
        if !missing.is_empty() {
            let paths = missing
                .iter()
                .map(|path| format!("{path:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "the window paints an SVG with no asset to load, so the screenshot would \
                 miss it: {paths}. Give the test an asset source that has {} with \
                 `cx.set_asset_source(..)`",
                if missing.len() == 1 { "it" } else { "them" }
            );
        }
        Ok(image)
    }

    /// Renders the window's current frame and writes it as
    /// `<folder>/<test name>/<name>.png`, returning the path. The folder is
    /// `$GPUI_SCREENSHOT_DIR`, or `gpui-screenshots` in the cargo target
    /// directory. See [`Self::screenshot_image`] for when it fails.
    pub fn screenshot(&mut self, name: &str) -> Result<PathBuf> {
        let path = screenshot_path(self.test_function_name(), name)?;
        let image = self.screenshot_image()?;
        save(&image, &path)?;
        Ok(path)
    }

    /// Like [`Self::screenshot_image`], cut down to the bounds of the one
    /// element with `role` named exactly `label` in the accessibility tree.
    /// Fails, naming the role and label, when there is none or more than
    /// one such element.
    pub fn screenshot_element_image(&mut self, role: Role, label: &str) -> Result<RgbaImage> {
        let bounds = self
            .try_bounds_by_role(role, label)
            .map_err(|message| anyhow!("can't take a screenshot of the element: {message}"))?;
        self.render_frame(Some((bounds, role, label)))
    }

    /// Like [`Self::screenshot`], cut down to the bounds of the one element
    /// with `role` named exactly `label` in the accessibility tree.
    pub fn screenshot_element(&mut self, name: &str, role: Role, label: &str) -> Result<PathBuf> {
        let path = screenshot_path(self.test_function_name(), name)?;
        let image = self.screenshot_element_image(role, label)?;
        save(&image, &path)?;
        Ok(path)
    }
}
