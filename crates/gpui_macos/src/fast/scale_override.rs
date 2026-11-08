//! A window's scale factor as a test run asks for it.
//!
//! A window on a 1x display draws at 1x, so a test that compares pixels at 2x
//! (against a browser capture at a real 2x device, say) cannot run on one.
//! `RN_GPUI_TEST_SCALE_FACTOR` names the scale every window reports and draws
//! at: layout, the drawable and the layer's contents scale all follow it, and
//! the system scales the layer down to the display.

use std::sync::OnceLock;

/// The scale factor a window reports: the one the environment asks for, if it
/// asks for one, else `measured`.
pub(crate) fn apply(measured: f32) -> f32 {
    requested().unwrap_or(measured)
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
        assert_eq!(parse(Some("-1")), None);
        assert_eq!(parse(Some("two")), None);
        assert_eq!(parse(None), None);
    }
}
