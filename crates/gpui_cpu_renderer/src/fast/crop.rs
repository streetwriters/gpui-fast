//! Drawing only the region of the frame a test asked for.

use anyhow::{Result, bail};
use gpui::{Bounds, DevicePixels, Size};

use crate::Canvas;

/// A canvas for `region` of a frame of `size`, the region cut down to the frame.
pub(crate) fn canvas(size: Size<DevicePixels>, region: Bounds<DevicePixels>) -> Result<Canvas> {
    let left = region.origin.x.0.clamp(0, size.width.0);
    let top = region.origin.y.0.clamp(0, size.height.0);
    let right = (region.origin.x.0.saturating_add(region.size.width.0)).clamp(left, size.width.0);
    let bottom =
        (region.origin.y.0.saturating_add(region.size.height.0)).clamp(top, size.height.0);
    if right <= left || bottom <= top {
        bail!(
            "cannot render {region:?} of a {}x{} window: it is outside it or has no size",
            size.width.0,
            size.height.0
        );
    }
    Canvas::crop(left, top, (right - left) as u32, (bottom - top) as u32)
}
