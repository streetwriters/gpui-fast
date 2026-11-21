//! Cutting a region out of a whole rendered frame, for a headless renderer
//! that has no cheaper way to draw only the region. It lives apart from
//! `test_rendering` because `PlatformHeadlessRenderer`, whose default method
//! calls it, is also built for benchmarks, which have no `test_rendering`.

use anyhow::{Result, bail};
use image::RgbaImage;

use crate::{Bounds, DevicePixels};

/// `region` of a whole frame, for a renderer that draws the whole frame.
pub(crate) fn crop_image(image: RgbaImage, region: Bounds<DevicePixels>) -> Result<RgbaImage> {
    let left = (region.origin.x.0.max(0) as u32).min(image.width());
    let top = (region.origin.y.0.max(0) as u32).min(image.height());
    let right = ((region.origin.x.0 + region.size.width.0).max(0) as u32).min(image.width());
    let bottom = ((region.origin.y.0 + region.size.height.0).max(0) as u32).min(image.height());
    if right <= left || bottom <= top {
        bail!("{region:?} is outside the {}x{} frame", image.width(), image.height());
    }
    Ok(image::imageops::crop_imm(&image, left, top, right - left, bottom - top).to_image())
}
