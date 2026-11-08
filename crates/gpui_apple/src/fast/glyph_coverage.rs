//! The coverage curve Skia applies to glyph masks, so text weighs what Chromium's does.
//!
//! Skia's mask gamma raises coverage for light text and lowers it slightly for
//! dark text, depending on the text colour's luminance quantised to three bits.
//! Core Graphics' own font smoothing thickens strokes by a different amount, so
//! glyphs are rasterised unsmoothed and this curve is applied to the mask.
//! `GAMMA` and `CONTRAST` are fitted to Chromium's output on macOS for dark and
//! light Inter at 13 and 15 points.

use core_graphics::context::CGContext;
use gpui::{Hsla, Rgba};
use std::sync::OnceLock;

const GAMMA: f32 = 1.7;
const CONTRAST: f32 = 0.8;
const LEVELS: usize = 8;

/// The level of a glyph whose coverage is left as rasterised.
const UNADJUSTED: u8 = LEVELS as u8;

/// The luminance level, 0 to 7, a glyph painted in `color` uses, or the
/// unadjusted level when the user turned font smoothing off.
pub(crate) fn level_for_color(color: Hsla, smoothing: bool) -> u8 {
    if !smoothing {
        return UNADJUSTED;
    }
    let rgba: Rgba = color.into();
    let luminance = 0.2126 * rgba.r + 0.7152 * rgba.g + 0.0722 * rgba.b;
    ((luminance.clamp(0.0, 1.0) * 255.0).round() as u32 >> 5) as u8
}

/// Fills glyphs with plain black and no font smoothing, so the mask holds raw coverage.
pub(crate) fn begin_unsmoothed(cx: &CGContext) {
    cx.set_gray_fill_color(0.0, 1.0);
}

/// Raises or lowers the coverage of an alpha-only glyph bitmap.
pub(crate) fn apply(level: u8, mask: &mut [u8]) {
    if level >= UNADJUSTED {
        return;
    }
    let table = &tables()[usize::from(level)];
    for coverage in mask {
        *coverage = table[usize::from(*coverage)];
    }
}

fn tables() -> &'static [[u8; 256]; LEVELS] {
    static TABLES: OnceLock<[[u8; 256]; LEVELS]> = OnceLock::new();
    TABLES.get_or_init(|| std::array::from_fn(|level| table(level as u32)))
}

fn table(level: u32) -> [u8; 256] {
    let source = ((level << 5) | (level << 2) | (level >> 1)) as f32 / 255.0;
    let destination = 1.0 - source;
    let linear_source = source.powf(GAMMA);
    let linear_destination = destination.powf(GAMMA);
    let contrast = CONTRAST * linear_destination;
    std::array::from_fn(|index| {
        let coverage = index as f32 / 255.0;
        if (source - destination).abs() < 1.0 / 256.0 {
            return index as u8;
        }
        let boosted = coverage + (1.0 - coverage) * contrast * coverage;
        let linear = linear_source * boosted + (1.0 - boosted) * linear_destination;
        let out = linear.powf(1.0 / GAMMA);
        let result = (out - destination) / (source - destination);
        (result.clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_and_no_coverage_are_unchanged() {
        for level in 0..LEVELS as u8 {
            let mut mask = [0, 255];
            apply(level, &mut mask);
            assert_eq!(mask, [0, 255], "level {level}");
        }
    }

    #[test]
    fn light_text_gains_coverage_and_dark_text_barely_changes() {
        let mut light = [128];
        apply(7, &mut light);
        let mut dark = [128];
        apply(0, &mut dark);
        assert!(light[0] > 160, "{}", light[0]);
        assert!(dark[0].abs_diff(128) <= 4, "{}", dark[0]);
    }

    #[test]
    fn the_curve_never_decreases() {
        for level in 0..LEVELS {
            let table = &tables()[level];
            assert!(
                table.windows(2).all(|pair| pair[0] <= pair[1]),
                "level {level}"
            );
        }
    }

    #[test]
    fn white_and_near_white_text_share_a_level() {
        let white = Hsla::from(Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        });
        let grey = Hsla::from(Rgba {
            r: 0.95,
            g: 0.95,
            b: 0.95,
            a: 1.0,
        });
        assert_eq!(level_for_color(white, true), 7);
        assert_eq!(level_for_color(grey, true), 7);
        assert_eq!(level_for_color(white, false), UNADJUSTED);
    }
}
