//! Transport rounded clip groups through the existing renderer layer graph.
//! No public primitive or ClipMask layout changes. Texture ids in
//! 0xF1000000.. identify clip groups, not atlas sprites or scroll tiles. Such a
//! group's LayerFrame has tile_size=0 and contains its drawable scene plus
//! a geometry LayerFrame (key u32::MAX). Horizontal radii are on the group
//! sprite and vertical radii occupy that frame's background RGBA, in
//! TL/TR/BR/BL order. This frame is metadata and is never composited.
//! Renderers evaluate the independent padding edges in colour fragments;
//! rounded groups do not allocate intermediate render targets.

use crate::fast::transform::push_rebuilt_layer;
use std::mem;

use crate::fast::rounded_clip::{ClipOperation, Shape};
use crate::{
    AtlasTextureId, AtlasTextureKind, AtlasTile, ColorFilter, ClipMask, LayerContent,
    LayerFrame, LayerKey, PolychromeSprite, Rgba, ScaledPixels, Scene, TileId, TransformIndex,
    TransformationMatrix,
    scene::{PaintOperation, Primitive},
};

const CLIP_TEXTURE_BASE: u32 = 0xF100_0000;
const GEOMETRY_KEY: LayerKey = LayerKey(u32::MAX);

/// Materialize private paint scopes as existing, renderer-readable layer
/// objects. Preserve the original operation stream for retained paint ranges.
///
/// What an effect group draws is drawn into the group's texture, so a scope
/// opened inside a group is a scene of that group, and the sprite that
/// composites the scope is a primitive of the group. A scope's scene holds
/// the group's primitives plainly: the group's texture is drawn from it.
pub(crate) fn compile(scene: &mut Scene) {
    if scene.fast_rounded_clip.scopes.is_empty() || scene.fast_rounded_clip.compiled {
        return;
    }
    let operations = mem::take(&mut scene.paint_operations);
    let scope_groups = scope_groups(&operations, &scene.transforms);
    let layers: Vec<LayerFrame> = scene
        .layers
        .frames
        .iter()
        .filter(|frame| frame.tile_size != 0)
        .cloned()
        .collect();
    let transforms = scene.transforms.clone();
    let new_scene = || {
        let mut scene = Scene::default();
        scene.layers.frames = layers.clone();
        scene.transforms = transforms.clone();
        scene
    };
    let mut root = new_scene();
    root.groups = scene.groups.clone();
    // The scopes being painted, with the group each is in.
    let mut open: Vec<(u32, Scene)> = Vec::new();
    let mut shapes = Vec::new();
    // Whether each begun scope repeats one active, and its group.
    let mut begun: Vec<(bool, u32)> = Vec::new();
    let mut next_scope = 0;
    // The scene a primitive of `group` is inserted into: the innermost scope
    // of the group, or the frame's.
    fn target<'a>(
        open: &'a mut [(u32, Scene)],
        root: &'a mut Scene,
        group: u32,
    ) -> (bool, &'a mut Scene) {
        match open.iter_mut().rev().find(|(scope, _)| *scope == group) {
            Some((_, scene)) => (false, scene),
            None => (true, root),
        }
    }
    for (position, operation) in operations.iter().enumerate() {
        match operation {
            PaintOperation::FastRoundedClip(ClipOperation::Begin(shape)) => {
                let already_active = shapes.contains(shape);
                let group = scope_groups[next_scope];
                next_scope += 1;
                begun.push((already_active, group));
                shapes.push(*shape);
                if !already_active {
                    open.push((group, new_scene()));
                }
            }
            PaintOperation::FastRoundedClip(ClipOperation::End) => {
                let shape = shapes.pop().expect("balanced rounded clips");
                let (redundant, group) = begun.pop().expect("rounded clip ownership");
                if redundant {
                    continue;
                }
                let (_, mut child) = open.pop().expect("rounded clip scene");
                if child.is_empty() {
                    continue;
                }
                child.layers.frames.push(geometry(shape));
                child.finish();
                let (in_frame, parent) = target(&mut open, &mut root, group);
                let key = LayerKey(0x0100_0000 + parent.layers.frames.len() as u32);
                parent.layers.frames.push(LayerFrame {
                    key,
                    generation: 0,
                    background: Rgba::default(),
                    tile_size: 0,
                    content: LayerContent::from(child),
                    dirty_tiles: Vec::new(),
                });
                let transform = if in_frame {
                    parent.transforms.intern_in_group(
                        TransformationMatrix::unit(),
                        ColorFilter::IDENTITY,
                        group,
                    )
                } else {
                    TransformIndex::IDENTITY
                };
                parent.insert_under(
                    PolychromeSprite {
                        order: 0,
                        transform_index: 0,
                        grayscale: false.into(),
                        opacity: 1.,
                        bounds: shape.bounds,
                        content_mask: ClipMask::new(shape.bounds),
                        corner_radii: shape.radii.map(|radius| radius.width),
                        tile: AtlasTile {
                            texture_id: AtlasTextureId {
                                index: CLIP_TEXTURE_BASE + key.0 - 0x0100_0000,
                                kind: AtlasTextureKind::Polychrome,
                            },
                            tile_id: TileId(0),
                            padding: 0,
                            bounds: Default::default(),
                        },
                    }
                    .into(),
                    transform,
                    false,
                );
            }
            PaintOperation::Primitive(primitive) => {
                let group = primitive_group(primitive, &scene.transforms);
                let index = primitive.transform_index();
                let (in_frame, target) = target(&mut open, &mut root, group);
                let index = match (in_frame, group, primitive) {
                    (true, group, Primitive::Path(_)) if group != 0 => {
                        target.transforms.intern_in_group(
                            TransformationMatrix::unit(),
                            ColorFilter::IDENTITY,
                            group,
                        )
                    }
                    (false, group, Primitive::Path(_)) if group != 0 => TransformIndex::IDENTITY,
                    (false, group, _) if group != 0 => target.transforms.intern_in_group(
                        target.transforms.get(index),
                        target.transforms.filter(index),
                        0,
                    ),
                    _ => index,
                };
                target.insert_under(primitive.clone(), index, false);
            }
            // Its primitives keep their transforms, and the scope scenes have
            // none current.
            PaintOperation::StartLayer(bounds) => {
                let rest = &operations[position + 1..];
                let scene = match open.last_mut() {
                    Some((_, scope)) => scope,
                    None => &mut root,
                };
                push_rebuilt_layer(scene, *bounds, rest, &transforms);
            }
            PaintOperation::EndLayer => match open.last_mut() {
                Some((_, scope)) => scope.pop_layer(),
                None => root.pop_layer(),
            },
        }
    }
    assert!(
        shapes.is_empty() && open.is_empty(),
        "balanced rounded clips"
    );
    let mut compiled = root;
    compiled.groups = mem::take(&mut scene.groups);
    compiled.paint_operations = operations;
    compiled.fast_rounded_clip = mem::take(&mut scene.fast_rounded_clip);
    compiled.fast_rounded_clip.compiled = true;
    compiled.sort_scratch = mem::take(&mut scene.sort_scratch);
    *scene = compiled;
}

fn primitive_group(primitive: &Primitive, transforms: &crate::TransformTable) -> u32 {
    match primitive {
        Primitive::Path(path) => path.group,
        primitive => transforms.group(primitive.transform_index()),
    }
}

/// The effect group each scope is opened in, in the order the scopes begin:
/// that of the sprite compositing a group inside it, which is a primitive of
/// the scope's own group, or else that of any primitive of the scope, or of a
/// scope inside it.
fn scope_groups(operations: &[PaintOperation], transforms: &crate::TransformTable) -> Vec<u32> {
    struct Open {
        slot: usize,
        composite: Option<u32>,
        any: Option<u32>,
    }
    let mut groups = Vec::new();
    let mut open: Vec<Open> = Vec::new();
    for operation in operations {
        match operation {
            PaintOperation::FastRoundedClip(ClipOperation::Begin(_)) => {
                open.push(Open {
                    slot: groups.len(),
                    composite: None,
                    any: None,
                });
                groups.push(0);
            }
            PaintOperation::FastRoundedClip(ClipOperation::End) => {
                let done = open.pop().expect("balanced rounded clips");
                let group = done.composite.or(done.any).unwrap_or(0);
                groups[done.slot] = group;
                if let Some(parent) = open.last_mut() {
                    parent.any.get_or_insert(group);
                }
            }
            PaintOperation::Primitive(primitive) => {
                if let Some(scope) = open.last_mut() {
                    let group = primitive_group(primitive, transforms);
                    let composite = matches!(
                        primitive,
                        Primitive::PolychromeSprite(sprite)
                            if crate::decode_effect_group(sprite.tile.texture_id).is_some()
                    );
                    if composite {
                        scope.composite.get_or_insert(group);
                    }
                    scope.any.get_or_insert(group);
                }
            }
            _ => {}
        }
    }
    groups
}

/// Gives `group`'s scene the clip scopes its sprites composite, moved by
/// `(dx, dy)` with the rest of what it draws.
pub(crate) fn move_clips(group: &mut Scene, frames: &[LayerFrame], dx: f32, dy: f32) {
    let delta = crate::point(ScaledPixels(dx), ScaledPixels(dy));
    for sprite in &group.polychrome_sprites {
        let index = sprite.tile.texture_id.index;
        if index < CLIP_TEXTURE_BASE || sprite.tile.texture_id.kind != AtlasTextureKind::Polychrome
        {
            continue;
        }
        let key = LayerKey(0x0100_0000 + index - CLIP_TEXTURE_BASE);
        if let Some(frame) = frames.iter().find(|frame| frame.key == key) {
            group.layers.frames.push(translated_frame(frame, delta));
        }
    }
}

fn translated_frame(frame: &LayerFrame, delta: crate::Point<ScaledPixels>) -> LayerFrame {
    let mut moved = frame.clone();
    if let Some(content) = frame.content.scene() {
        moved.content = LayerContent::from(translated_scene(content, delta));
    }
    moved
}

fn translated_scene(scene: &Scene, delta: crate::Point<ScaledPixels>) -> Scene {
    let mut moved = Scene::default();
    moved.transforms = scene.transforms.clone();
    moved.layers.frames = scene
        .layers
        .frames
        .iter()
        .map(|frame| {
            if frame.key.0 >= 0x0100_0000 && frame.key != GEOMETRY_KEY {
                translated_frame(frame, delta)
            } else {
                frame.clone()
            }
        })
        .collect();
    for operation in &scene.paint_operations {
        match operation {
            PaintOperation::Primitive(primitive) => moved.insert_under(
                crate::fast::layers::scene::translate_primitive(primitive, delta),
                primitive.transform_index(),
                false,
            ),
            PaintOperation::StartLayer(bounds) => moved.push_layer(crate::Bounds {
                origin: bounds.origin + delta,
                size: bounds.size,
            }),
            PaintOperation::EndLayer => moved.pop_layer(),
            PaintOperation::FastRoundedClip(_) => {}
        }
    }
    moved.finish();
    moved
}

fn geometry(shape: Shape) -> LayerFrame {
    let r = shape.radii;
    LayerFrame {
        key: GEOMETRY_KEY,
        generation: 0,
        // Private analytic geometry: horizontal radii are on the group sprite;
        // this non-composited frame carries the corresponding vertical radii.
        background: Rgba {
            r: r.top_left.height.0,
            g: r.top_right.height.0,
            b: r.bottom_right.height.0,
            a: r.bottom_left.height.0,
        },
        tile_size: 0,
        content: Scene::default().into(),
        dirty_tiles: Vec::new(),
    }
}
