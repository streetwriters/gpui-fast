//! Colour glyphs, composited on the CPU.
//!
//! DirectWrite hands back each colour layer of a glyph as an alpha mask. The
//! layers used to be uploaded as textures, drawn over each other on the GPU,
//! and read back with a blocking copy, which cost a round trip to the GPU for
//! every new glyph. Compositing the masks over each other takes a few
//! microseconds on the CPU, with the arithmetic of the shader that drew them
//! (`color_text_raster.hlsl`) and the render target's 8-bit rounding after
//! each layer.

use anyhow::Result;
use gpui::{Bounds, DevicePixels, RenderGlyphParams, Rgba, point, size};
use std::mem::ManuallyDrop;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_GLYPH_IMAGE_FORMATS_COLR, DWRITE_GLYPH_IMAGE_FORMATS_TRUETYPE, DWRITE_GLYPH_OFFSET,
    DWRITE_GLYPH_RUN, DWRITE_GRID_FIT_MODE_DEFAULT, DWRITE_MATRIX, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC, DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    DWRITE_TEXTURE_ALIASED_1x1,
};
use windows::core::BOOL;
use windows_numerics::Vector2;

use crate::direct_write::{DirectWriteComponents, FontInfo};
use crate::directx_renderer::DirectXRenderer;

/// One colour layer of a glyph: its alpha mask, `bounds` big, at `bounds`'
/// origin in the glyph's bitmap, drawn in `color`.
pub(crate) struct Layer {
    pub(crate) color: Rgba,
    pub(crate) bounds: Bounds<i32>,
    pub(crate) mask: Vec<u8>,
}

fn saturate(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

/// `apply_contrast_and_gamma_correction` of `alpha_correction.hlsl`.
fn corrected_alpha(sample: f32, color: [f32; 3], contrast: f32, gamma: [f32; 4]) -> f32 {
    let brightness = color[0] * 0.30 + color[1] * 0.59 + color[2] * 0.11;
    let enhanced = contrast * saturate(4.0 * (0.75 - brightness));
    let contrasted = sample * (enhanced + 1.0) / (sample * enhanced + 1.0);
    let adjustment = gamma[0] * brightness + gamma[1];
    let correction = adjustment * contrasted + (gamma[2] * brightness + gamma[3]);
    contrasted + contrasted * (1.0 - contrasted) * correction
}

fn to_unorm(value: f32) -> u8 {
    (saturate(value) * 255.0 + 0.5) as u8
}

/// Draws `layers` over each other into a transparent `size` bitmap of
/// premultiplied BGRA, as the render target held them, then returns it with
/// straight alpha.
pub(crate) fn composite(
    layers: &[Layer],
    size: (usize, usize),
    gamma: [f32; 4],
    contrast: f32,
) -> Vec<u8> {
    let (width, height) = size;
    let mut bitmap = vec![0u8; width * height * 4];
    for layer in layers {
        let color = [layer.color.r, layer.color.g, layer.color.b];
        let (left, top) = (layer.bounds.origin.x, layer.bounds.origin.y);
        let (layer_width, layer_height) = (
            layer.bounds.size.width as usize,
            layer.bounds.size.height as usize,
        );
        for y in 0..layer_height {
            let row = top + y as i32;
            if row < 0 || row as usize >= height {
                continue;
            }
            for x in 0..layer_width {
                let column = left + x as i32;
                if column < 0 || column as usize >= width {
                    continue;
                }
                let sample = layer.mask[y * layer_width + x];
                if sample == 0 {
                    continue;
                }
                let alpha =
                    corrected_alpha(sample as f32 / 255.0, color, contrast, gamma) * layer.color.a;
                let source = [
                    to_unorm(color[2] * alpha),
                    to_unorm(color[1] * alpha),
                    to_unorm(color[0] * alpha),
                    to_unorm(alpha),
                ];
                let inverse = 1.0 - source[3] as f32 / 255.0;
                let at = (row as usize * width + column as usize) * 4;
                for channel in 0..4 {
                    let behind = bitmap[at + channel] as f32 / 255.0;
                    bitmap[at + channel] =
                        to_unorm(source[channel] as f32 / 255.0 + behind * inverse);
                }
            }
        }
    }
    // Convert from premultiplied to straight alpha
    for chunk in bitmap.chunks_exact_mut(4) {
        let b = chunk[0] as f32;
        let g = chunk[1] as f32;
        let r = chunk[2] as f32;
        let a = chunk[3] as f32;
        if a > 0.0 {
            let inv_a = 255.0 / a;
            chunk[0] = (b * inv_a).clamp(0.0, 255.0) as u8;
            chunk[1] = (g * inv_a).clamp(0.0, 255.0) as u8;
            chunk[2] = (r * inv_a).clamp(0.0, 255.0) as u8;
        }
    }
    bitmap
}

/// The colour glyph of `params` in `glyph_bounds`, straight-alpha BGRA, with
/// its layers composited on the CPU.
pub(crate) fn rasterize(
    components: &DirectWriteComponents,
    font: &FontInfo,
    params: &RenderGlyphParams,
    glyph_bounds: Bounds<DevicePixels>,
) -> Result<Vec<u8>> {
    let bitmap_size = glyph_bounds.size;
    let subpixel_shift = params
        .subpixel_variant
        .map(|v| v as f32 / gpui::SUBPIXEL_VARIANTS_X as f32);
    let baseline_origin_x = subpixel_shift.x / params.scale_factor;
    let baseline_origin_y = subpixel_shift.y / params.scale_factor;
    let transform = DWRITE_MATRIX {
        m11: params.scale_factor,
        m12: 0.0,
        m21: 0.0,
        m22: params.scale_factor,
        dx: 0.0,
        dy: 0.0,
    };
    let glyph_id = [params.glyph_id.0 as u16];
    let advance = [glyph_bounds.size.width.0 as f32];
    let offset = [DWRITE_GLYPH_OFFSET {
        advanceOffset: -glyph_bounds.origin.x.0 as f32 / params.scale_factor,
        ascenderOffset: glyph_bounds.origin.y.0 as f32 / params.scale_factor,
    }];
    let glyph_run = DWRITE_GLYPH_RUN {
        fontFace: ManuallyDrop::new(Some(unsafe { std::ptr::read(&***font.font_face) })),
        fontEmSize: params.font_size.as_f32(),
        glyphCount: 1,
        glyphIndices: glyph_id.as_ptr(),
        glyphAdvances: advance.as_ptr(),
        glyphOffsets: offset.as_ptr(),
        isSideways: BOOL(0),
        bidiLevel: 0,
    };
    let color_enumerator = unsafe {
        components.factory.TranslateColorGlyphRun(
            Vector2::new(baseline_origin_x, baseline_origin_y),
            &glyph_run,
            None,
            DWRITE_GLYPH_IMAGE_FORMATS_COLR,
            DWRITE_MEASURING_MODE_NATURAL,
            Some(&transform),
            0,
        )
    }?;

    let mut layers = Vec::new();
    loop {
        let color_run = unsafe { color_enumerator.GetCurrentRun() }?;
        let color_run = unsafe { &*color_run };
        let image_format = color_run.glyphImageFormat & !DWRITE_GLYPH_IMAGE_FORMATS_TRUETYPE;
        if image_format == DWRITE_GLYPH_IMAGE_FORMATS_COLR {
            let analysis = unsafe {
                components.factory.CreateGlyphRunAnalysis(
                    &color_run.Base.glyphRun as *const _,
                    Some(&transform),
                    DWRITE_RENDERING_MODE1_NATURAL_SYMMETRIC,
                    DWRITE_MEASURING_MODE_NATURAL,
                    DWRITE_GRID_FIT_MODE_DEFAULT,
                    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                    baseline_origin_x,
                    baseline_origin_y,
                )
            }?;
            let layer_bounds =
                unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1) }?;
            let layer_size = size(
                layer_bounds.right - layer_bounds.left,
                layer_bounds.bottom - layer_bounds.top,
            );
            if layer_size.width > 0 && layer_size.height > 0 {
                let mut mask = vec![0u8; (layer_size.width * layer_size.height) as usize];
                unsafe {
                    analysis.CreateAlphaTexture(
                        DWRITE_TEXTURE_ALIASED_1x1,
                        &layer_bounds,
                        &mut mask,
                    )
                }?;
                let color = color_run.Base.runColor;
                layers.push(Layer {
                    color: Rgba {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                        a: color.a,
                    },
                    bounds: gpui::bounds(point(layer_bounds.left, layer_bounds.top), layer_size),
                    mask,
                });
            }
        }
        let has_next = unsafe { color_enumerator.MoveNext() }
            .map(|e| e.as_bool())
            .unwrap_or(false);
        if !has_next {
            break;
        }
    }

    let font_info = DirectXRenderer::get_font_info();
    Ok(composite(
        &layers,
        (bitmap_size.width.0 as usize, bitmap_size.height.0 as usize),
        font_info.gamma_ratios,
        font_info.grayscale_enhanced_contrast,
    ))
}

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

    /// The glyph as the GPU compositing it used to be drawn.
    fn on_the_gpu(
        text_system: &DirectWriteTextSystem,
        params: &RenderGlyphParams,
        bounds: gpui::Bounds<gpui::DevicePixels>,
    ) -> Result<Vec<u8>> {
        text_system
            .state
            .read()
            .rasterize_color(&text_system.components, params, bounds)
    }

    #[test]
    fn the_cpu_composite_matches_the_gpu_one() -> Result<()> {
        let devices = DirectXDevices::new()?;
        let text_system = DirectWriteTextSystem::new(Some(&devices))?;
        let glyphs = emoji_params(&text_system, 500, 24.)?;
        assert!(glyphs.len() > 100);
        let mut largest = 0u8;
        let (mut differing, mut total) = (0usize, 0usize);
        for (params, bounds) in &glyphs {
            let (_, cpu) = text_system.rasterize_glyph(params, *bounds)?;
            let gpu = on_the_gpu(&text_system, params, *bounds)?;
            assert_eq!(cpu.len(), gpu.len());
            for (cpu, gpu) in cpu.iter().zip(&gpu) {
                total += 1;
                let difference = cpu.abs_diff(*gpu);
                differing += usize::from(difference != 0);
                largest = largest.max(difference);
            }
        }
        println!("largest difference {largest}; {differing} of {total} channel values differ");
        assert!(largest <= 2, "largest difference {largest}");
        Ok(())
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
