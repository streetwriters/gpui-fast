//! What `#[gpui::test(render)]` and `cx.enable_rendering(..)` need to render a
//! test's windows: the current platform's text system, built once per process
//! and shared by every test, and a factory for its offscreen renderer.

use std::sync::{Arc, OnceLock};

pub fn test_rendering() -> gpui::TestRendering {
    static TEXT_SYSTEM: OnceLock<Arc<dyn gpui::PlatformTextSystem>> = OnceLock::new();
    let text_system = TEXT_SYSTEM
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
        .clone();
    gpui::TestRendering::new(text_system, crate::current_headless_renderer)
}
