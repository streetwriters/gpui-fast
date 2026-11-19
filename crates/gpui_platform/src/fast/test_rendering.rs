//! What `#[gpui::test(render)]` and `cx.enable_rendering(..)` need to render a
//! test's windows: the current platform's text system, built once per process
//! and shared by every test, and a factory for its offscreen renderer.
//!
//! The renderer is the platform's GPU one (Metal, wgpu). Where that can't be
//! made (no GPU, as in a sandbox or on much of Linux CI) the software renderer
//! in `gpui_cpu_renderer` takes over. `GPUI_TEST_RENDERER=cpu` picks it even when a
//! GPU works, so screenshots come out the same on every machine.

use std::sync::{Arc, OnceLock};

use gpui::PlatformHeadlessRenderer;

/// The environment variable that forces a renderer; `cpu` is the one value.
pub const RENDERER_ENV: &str = "GPUI_TEST_RENDERER";

fn cpu_forced() -> bool {
    forces_cpu(std::env::var(RENDERER_ENV).ok().as_deref())
}

fn forces_cpu(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case("cpu"))
}

fn cpu_renderer() -> Box<dyn PlatformHeadlessRenderer> {
    Box::new(gpui_cpu_renderer::CpuHeadlessRenderer::new())
}

/// Which renderer `choose` picked, and why.
#[derive(Debug, PartialEq, Eq)]
enum Chosen {
    Gpu,
    CpuForced,
    CpuFallback,
}

/// The offscreen renderer: the CPU one when `forced`, otherwise what `gpu`
/// makes, falling back to the CPU one when that can't be made. `gpu` is not
/// called at all when forced.
fn choose(
    forced: bool,
    gpu: impl FnOnce() -> anyhow::Result<Option<Box<dyn PlatformHeadlessRenderer>>>,
) -> (Chosen, Box<dyn PlatformHeadlessRenderer>) {
    if forced {
        return (Chosen::CpuForced, cpu_renderer());
    }
    match gpu() {
        Ok(Some(renderer)) => (Chosen::Gpu, renderer),
        Ok(None) => {
            log::info!("no GPU renderer for this platform; rendering tests on the CPU");
            (Chosen::CpuFallback, cpu_renderer())
        }
        Err(error) => {
            log::info!("no GPU renderer ({error:#}); rendering tests on the CPU");
            (Chosen::CpuFallback, cpu_renderer())
        }
    }
}

fn headless_renderer() -> anyhow::Result<Option<Box<dyn PlatformHeadlessRenderer>>> {
    Ok(Some(
        choose(cpu_forced(), crate::current_headless_renderer).1,
    ))
}

#[cfg(test)]
mod tests {
    use super::{Chosen, choose, forces_cpu};
    use gpui::{DevicePixels, PlatformAtlas, PlatformHeadlessRenderer, Scene, Size};
    use std::sync::Arc;

    #[test]
    fn only_cpu_forces_the_cpu_renderer() {
        assert!(forces_cpu(Some("cpu")));
        assert!(forces_cpu(Some(" CPU\n")));
        assert!(!forces_cpu(None));
        assert!(!forces_cpu(Some("")));
        assert!(!forces_cpu(Some("gpu")));
        assert!(!forces_cpu(Some("cpus")));
    }

    /// Stands for a GPU renderer: it draws nothing.
    struct Gpu;

    impl PlatformHeadlessRenderer for Gpu {
        fn render_scene_to_image(
            &mut self,
            _: &Scene,
            _: Size<DevicePixels>,
        ) -> anyhow::Result<image::RgbaImage> {
            Ok(image::RgbaImage::new(1, 1))
        }

        fn render_scene(&mut self, _: &Scene, _: Size<DevicePixels>) -> anyhow::Result<()> {
            Ok(())
        }

        fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
            unreachable!("not asked for")
        }
    }

    /// Whether this is the CPU renderer: asked for a 2x2 frame it gives one,
    /// where the stand-in always gives its 1x1 image.
    fn is_cpu(renderer: &mut dyn PlatformHeadlessRenderer) -> bool {
        let mut scene = Scene::default();
        scene.finish();
        let size = gpui::size(DevicePixels(2), DevicePixels(2));
        renderer.render_scene_to_image(&scene, size).unwrap().dimensions() == (2, 2)
    }

    #[test]
    fn the_cpu_renderer_takes_over_when_the_gpu_one_fails() {
        let (chosen, mut renderer) = choose(false, || Err(anyhow::anyhow!("no Metal device")));
        assert_eq!(chosen, Chosen::CpuFallback);
        assert!(is_cpu(renderer.as_mut()));
    }

    #[test]
    fn the_cpu_renderer_takes_over_when_the_platform_has_no_gpu_one() {
        let (chosen, mut renderer) = choose(false, || Ok(None));
        assert_eq!(chosen, Chosen::CpuFallback);
        assert!(is_cpu(renderer.as_mut()));
    }

    #[test]
    fn a_working_gpu_renderer_is_kept() {
        let (chosen, mut renderer) = choose(false, || Ok(Some(Box::new(Gpu))));
        assert_eq!(chosen, Chosen::Gpu);
        // The stand-in answers with its 1x1 image, the CPU renderer with a 2x2.
        assert!(!is_cpu(renderer.as_mut()));
    }

    #[test]
    fn forcing_the_cpu_renderer_never_asks_for_the_gpu_one() {
        let (chosen, mut renderer) = choose(true, || -> anyhow::Result<_> {
            panic!("the GPU renderer was asked for")
        });
        assert_eq!(chosen, Chosen::CpuForced);
        assert!(is_cpu(renderer.as_mut()));
    }

}

pub fn test_rendering() -> gpui::TestRendering {
    gpui::TestRendering::new(text_system(), headless_renderer)
}

/// Test rendering on the CPU renderer whatever the machine has.
pub fn cpu_test_rendering() -> gpui::TestRendering {
    gpui::TestRendering::new(text_system(), || Ok(Some(cpu_renderer())))
}

/// Test rendering on the platform's GPU renderer only: no fallback, so a
/// machine without a GPU gets the error. For comparing it with the CPU one.
pub fn gpu_test_rendering() -> gpui::TestRendering {
    gpui::TestRendering::new(text_system(), crate::current_headless_renderer)
}

fn text_system() -> Arc<dyn gpui::PlatformTextSystem> {
    static TEXT_SYSTEM: OnceLock<Arc<dyn gpui::PlatformTextSystem>> = OnceLock::new();
    TEXT_SYSTEM
        .get_or_init(|| {
            // A whole macOS platform can only be made on the main thread, which
            // a test is not on.
            #[cfg(target_os = "macos")]
            {
                Arc::new(gpui_apple::AppleTextSystem::new())
            }
            #[cfg(not(target_os = "macos"))]
            {
                crate::current_platform(true).text_system()
            }
        })
        .clone()
}
