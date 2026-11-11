//! Windows that report themselves active while a test driver runs the app.
//!
//! A test run keeps its windows unmapped, or behind a window manager that never
//! focuses them, and focus and blur events reach an element only while its
//! window is active. While the test driver is on (`RN_GPUI_TEST_DRIVER`) and
//! the run has not asked for real activation (`RN_GPUI_BACKGROUND=0`), every
//! window reports itself active to GPUI.

use std::sync::OnceLock;

/// Whether a window is active: it is whenever [`reports_active`] says so, and
/// otherwise when the window system says `focused`.
pub(crate) fn apply(focused: bool) -> bool {
    reports_active() || focused
}

fn reports_active() -> bool {
    static REPORTS: OnceLock<bool> = OnceLock::new();
    *REPORTS.get_or_init(|| {
        resolve(
            std::env::var("RN_GPUI_TEST_DRIVER").ok().as_deref(),
            std::env::var("RN_GPUI_BACKGROUND").ok().as_deref(),
        )
    })
}

fn resolve(driver: Option<&str>, background: Option<&str>) -> bool {
    let driver = driver.is_some_and(|value| !matches!(value, "" | "0"));
    let background = !matches!(background, Some("" | "0" | "false" | "no"));
    driver && background
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn the_driver_turns_it_on_and_a_run_that_wants_real_activation_turns_it_off() {
        assert!(resolve(Some("1"), None));
        assert!(resolve(Some("1"), Some("1")));
        assert!(!resolve(Some("1"), Some("0")));
        assert!(!resolve(None, None));
        assert!(!resolve(Some("0"), None));
    }
}
