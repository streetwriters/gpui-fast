//! How much texture memory a window's scroll layer tiles may keep.
//!
//! The renderer crates include this file by path, as it is not part of
//! `gpui` itself.

const BYTES_PER_PIXEL: u64 = 4;
/// Tiles worth this many windows are kept for scrolling back over.
const WINDOWS_KEPT: u64 = 3;
/// A small window still keeps its viewport's tiles, which straddle more tiles
/// than it spans.
const FLOOR: u64 = 16 << 20;
const CEILING: u64 = 64 << 20;

/// The tile memory budget of a window `width` by `height` device pixels.
pub(crate) fn for_window(width: u32, height: u32) -> u64 {
    (u64::from(width) * u64::from(height) * BYTES_PER_PIXEL * WINDOWS_KEPT).clamp(FLOOR, CEILING)
}

#[cfg(test)]
mod tests {
    use super::for_window;

    #[test]
    fn the_budget_follows_the_window_between_a_floor_and_a_ceiling() {
        assert_eq!(for_window(640, 480), 16 << 20);
        assert_eq!(for_window(2800, 1800), 60_480_000);
        assert_eq!(for_window(5600, 3200), 64 << 20);
        assert_eq!(for_window(0, 0), 16 << 20);
    }
}
