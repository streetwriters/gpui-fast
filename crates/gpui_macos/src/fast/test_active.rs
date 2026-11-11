//! Windows that report themselves active and visible while a test driver runs the app.
//!
//! A background test run never makes a window key, and focus and blur events
//! reach an element only while its window is active. While the test driver is
//! on (`RN_GPUI_TEST_DRIVER`) and the run has not asked for real activation
//! (`RN_GPUI_BACKGROUND=0`), every window reports itself active to GPUI from
//! the moment it opens, without the application being activated.
//!
//! A window other windows cover is occluded, and GPUI stops drawing it, so a
//! driver that acts on it in the background would read stale pixels and a
//! stale accessibility tree. In the same mode a window also counts as visible,
//! so it keeps drawing while covered.
//!
//! The run must not take the user's focus or pointer either: in this mode a
//! window opens behind the user's windows, and activating the application or
//! a window does nothing. A window brought in front of the user's work catches
//! the user's next click, and an activated application their typing.

use std::sync::OnceLock;

/// Whether a window is active: it is whenever [`reports_active`] says so, and
/// otherwise when the system says `key`.
pub(crate) fn apply(key: bool) -> bool {
    reports_active() || key
}

/// Whether a window counts as visible: always in the mode above, otherwise when
/// the system says `occlusion_visible`.
pub(crate) fn visible(occlusion_visible: bool) -> bool {
    reports_active() || occlusion_visible
}

/// Whether the app runs in the background under a test driver: its windows
/// open behind the user's and it never activates.
pub(crate) fn in_background() -> bool {
    reports_active()
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
