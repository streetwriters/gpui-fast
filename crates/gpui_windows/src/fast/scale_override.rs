//! A window's scale factor as a test run asks for it.
//!
//! A runner whose display is 1x cannot run a test that compares pixels at 2x.
//! `RN_GPUI_TEST_SCALE_FACTOR` names the scale every window reports and draws
//! at. The window is then that many times larger in device pixels than its
//! logical size, so it may not fit the display: the largest size the system
//! allows a window is lifted while the override is set.

use std::sync::OnceLock;

use windows::Win32::{Foundation::LPARAM, UI::WindowsAndMessaging::MINMAXINFO};

pub(crate) fn apply(measured: f32) -> f32 {
    requested().unwrap_or(measured)
}

pub(crate) fn lift_max_track_size(lparam: LPARAM) -> bool {
    if requested().is_none() {
        return false;
    }
    const LARGEST: i32 = 16384;
    unsafe {
        let info = &mut *(lparam.0 as *mut MINMAXINFO);
        info.ptMaxTrackSize.x = LARGEST;
        info.ptMaxTrackSize.y = LARGEST;
    }
    true
}

fn requested() -> Option<f32> {
    static REQUESTED: OnceLock<Option<f32>> = OnceLock::new();
    *REQUESTED.get_or_init(|| parse(std::env::var("RN_GPUI_TEST_SCALE_FACTOR").ok().as_deref()))
}

fn parse(value: Option<&str>) -> Option<f32> {
    value
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|scale| scale.is_finite() && *scale > 0.0)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn only_a_positive_number_is_a_scale() {
        assert_eq!(parse(Some("2")), Some(2.0));
        assert_eq!(parse(Some("1.5")), Some(1.5));
        assert_eq!(parse(Some("0")), None);
        assert_eq!(parse(Some("two")), None);
        assert_eq!(parse(None), None);
    }
}
