//! Effect groups on Metal: each group's primitives are drawn into a texture
//! of their own before the frame's main pass, innermost group first, and the
//! sprite the scene composites it with reads that texture. See
//! `gpui::fast::effects`.

use std::cell::Cell;

use anyhow::Result;
use gpui::{DevicePixels, GROUP_CONTENT, Scene, Size};

use crate::metal_renderer::MetalRenderer;

thread_local! {
    /// Set while a group's texture is being drawn, which starts transparent.
    static GROUP_PASS: Cell<bool> = const { Cell::new(false) };
}

/// Whether a group's texture is being drawn, which starts transparent.
pub(crate) fn in_group_pass() -> bool {
    GROUP_PASS.with(Cell::get)
}

/// Draws every effect group of `scene` into its texture. The command buffers
/// are committed here, ahead of the frame's own, so the textures are ready
/// when its pass composites them.
pub(crate) fn render_groups(
    renderer: &mut MetalRenderer,
    scene: &Scene,
    _viewport_size: Size<DevicePixels>,
) -> Result<()> {
    // A group's own pass has no groups, and must not recycle its siblings'.
    if in_group_pass() {
        return Ok(());
    }
    renderer.recycle_group_textures();
    if scene.group_scenes.is_empty() {
        return Ok(());
    }
    for (index, sub) in scene.group_scenes.iter().enumerate().rev() {
        let group = index as u32 + 1;
        let bounds = scene.groups[index].bounds;
        let (width, height) = (bounds.size.width.0 as i32, bounds.size.height.0 as i32);
        if width <= 0 || height <= 0 {
            continue;
        }
        let size = gpui::size(DevicePixels(width), DevicePixels(height));
        let content = renderer.group_texture(width as u64, height as u64);
        renderer.effect_stats.groups_rasterized += 1;
        GROUP_PASS.with(|pass| pass.set(true));
        let drawn = renderer.render_frame(sub, &content, size);
        GROUP_PASS.with(|pass| pass.set(false));
        drawn?.commit();
        renderer
            .effect_group_textures
            .insert((group, GROUP_CONTENT), content);
        for pass in &scene.group_blurs[index] {
            let target = match renderer.effect_group_textures.get(&(group, pass.to)) {
                Some(texture) => texture.clone(),
                None => {
                    let texture = renderer.group_texture(width as u64, height as u64);
                    renderer
                        .effect_group_textures
                        .insert((group, pass.to), texture.clone());
                    texture
                }
            };
            GROUP_PASS.with(|flag| flag.set(true));
            let drawn = renderer.render_frame(&pass.scene, &target, size);
            GROUP_PASS.with(|flag| flag.set(false));
            drawn?.commit();
        }
    }
    Ok(())
}
