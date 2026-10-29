// --- subpixel sprites --- //

struct SubpixelSprite {
    order: u32,
    transform_index: u32,
    bounds: Bounds,
    content_mask: ContentMask,
    color: Hsla,
    tile: AtlasTile,
    transformation: TransformationMatrix,
}
@group(1) @binding(0) var<storage, read> b_subpixel_sprites: array<SubpixelSprite>;

struct SubpixelSpriteOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) tile_position: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(3) clip_distances: vec4<f32>,
    @location(4) @interpolate(flat) sprite_id: u32,
    @location(5) local_position: vec2<f32>,
}

struct SubpixelSpriteFragmentOutput {
    @location(0) @blend_src(0) foreground: vec4<f32>,
    @location(0) @blend_src(1) alpha: vec4<f32>,
}

@vertex
fn vs_subpixel_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> SubpixelSpriteOutput {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = b_subpixel_sprites[instance_id];

    var out = SubpixelSpriteOutput();
    let local_position = apply_transform(sprite.transformation, unit_vertex * sprite.bounds.size + sprite.bounds.origin);
    out.position = to_device_position_impl(apply_transform(transform_at(sprite.transform_index), local_position));
    out.local_position = local_position;
    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.color = hsla_to_rgba(sprite.color);
    out.clip_distances = distance_from_clip_rect_impl(local_position, sprite.content_mask.bounds);
    out.sprite_id = instance_id;
    return out;
}

@fragment
fn fs_subpixel_sprite(input: SubpixelSpriteOutput) -> SubpixelSpriteFragmentOutput {
    var sample = textureSample(t_sprite, s_sprite, input.tile_position).rgb;
    if (gamma_params.is_bgr != 0u) {
        sample = sample.bgr;
    }
    let alpha_corrected = apply_contrast_and_gamma_correction3(sample, input.color.rgb, gamma_params.subpixel_enhanced_contrast, gamma_params.gamma_ratios);

    // Alpha clip after using the derivatives.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return SubpixelSpriteFragmentOutput(vec4<f32>(0.0), vec4<f32>(0.0));
    }

    var out = SubpixelSpriteFragmentOutput();
    let sprite = b_subpixel_sprites[input.sprite_id];
    // The filter recolours the text colour; the coverage stays as sampled.
    out.foreground = vec4<f32>(apply_color_filter(sprite.transform_index, vec4<f32>(input.color.rgb, 1.0)).rgb, 1.0);
    let frag_pos = fragment_position(input.position, input.local_position, sprite.transform_index);
    let mask_alpha = content_mask_alpha(frag_pos, sprite.content_mask);
    out.alpha = vec4<f32>(input.color.a * alpha_corrected * mask_alpha, 1.0);
    return out;
}
