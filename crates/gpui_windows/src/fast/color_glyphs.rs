//! Tests of DirectWrite's colour glyph rasterization against the real device.

#[cfg(test)]
mod tests {
    use crate::direct_write::DirectWriteTextSystem;
    use crate::directx_devices::DirectXDevices;
    use anyhow::Result;
    use gpui::{Font, PlatformTextSystem, RenderGlyphParams, point, px};
    use std::time::Instant;

    /// Up to `count` distinct emoji glyphs of Segoe UI Emoji at `size`.
    fn emoji_params(
        text_system: &DirectWriteTextSystem,
        count: usize,
        size: f32,
    ) -> Result<Vec<(RenderGlyphParams, gpui::Bounds<gpui::DevicePixels>)>> {
        let font_id = text_system.font_id(&Font {
            family: "Segoe UI Emoji".into(),
            ..Default::default()
        })?;
        let mut seen = std::collections::HashSet::new();
        let mut all = Vec::new();
        let ranges = [
            0x1F300..0x1F650,
            0x1F680..0x1F700,
            0x1F900..0x1FA00,
            0x1FA70..0x1FB00,
            0x2600..0x2700,
        ];
        for ch in ranges.into_iter().flatten().filter_map(char::from_u32) {
            let Some(glyph_id) = text_system.glyph_for_char(font_id, ch) else {
                continue;
            };
            if !seen.insert(glyph_id.0) {
                continue;
            }
            let params = RenderGlyphParams {
                font_id,
                glyph_id,
                font_size: px(size),
                subpixel_variant: point(0u8, 0u8),
                scale_factor: 1.0,
                is_emoji: true,
                subpixel_rendering: false,
                dilation: 0,
            };
            let Ok(bounds) = text_system.glyph_raster_bounds(&params) else {
                continue;
            };
            if bounds.size.width.0 > 0 && bounds.size.height.0 > 0 {
                all.push((params, bounds));
            }
            if all.len() == count {
                break;
            }
        }
        Ok(all)
    }

    /// `cargo test -p gpui_windows --release color_glyph_cost -- --ignored --nocapture`
    ///
    /// `EMOJI_BYTES=<path>` writes every glyph's bitmap there, to compare
    /// two builds byte for byte.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn color_glyph_cost() -> Result<()> {
        let devices = DirectXDevices::new()?;
        let text_system = DirectWriteTextSystem::new(Some(&devices))?;
        let params = emoji_params(&text_system, 500, 24.)?;
        println!("glyphs: {}", params.len());
        // One pass to warm the font and shader caches.
        for (params, bounds) in params.iter().take(20) {
            text_system.rasterize_glyph(params, *bounds)?;
        }
        let mut times = Vec::new();
        let mut bytes = Vec::new();
        for (params, bounds) in &params {
            let start = Instant::now();
            let (_, rasterized) = text_system.rasterize_glyph(params, *bounds)?;
            times.push(start.elapsed().as_secs_f64() * 1e3);
            bytes.extend_from_slice(&rasterized);
        }
        let total: f64 = times.iter().sum();
        times.sort_by(f64::total_cmp);
        println!(
            "rasterize {} glyphs: total {:.1} ms, median {:.3} ms, p95 {:.3} ms, max {:.3} ms",
            times.len(),
            total,
            times[times.len() / 2],
            times[times.len() * 95 / 100],
            times[times.len() - 1]
        );
        if let Ok(path) = std::env::var("EMOJI_BYTES") {
            std::fs::write(path, &bytes)?;
        }
        Ok(())
    }
}
