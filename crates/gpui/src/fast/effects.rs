//! Effect groups: a subtree painted into a texture of its own and composited
//! as one image, so group opacity (and later blur, shadow and blend modes) act
//! on what the subtree painted as a whole.
//!
//! A group's primitives carry the group id in their transform table entry
//! (paths, which have no entry, in `Scene::path_groups`). [`Scene::finish`]
//! sorts the scene as it always did, then [`split_groups`] moves each group's
//! primitives, in the same order, into a scene of its own. The renderer draws
//! those scenes into textures before the frame's main pass, innermost group
//! first, and the sprite [`Scene::end_effect_group`] inserted composites each.

use crate::{
    Path, PathId, PolychromeSprite, ScaledPixels, Scene, Shadow, Underline,
    scene::MonochromeSprite, scene::Quad, scene::SubpixelSprite,
};

/// Moves the primitives of every effect group out of `scene` into
/// `scene.group_scenes`.
pub(crate) fn split_groups(scene: &mut Scene) {
    if scene.groups.is_empty() || !scene.group_scenes.is_empty() {
        return;
    }
    let count = scene.groups.len();
    let mut subs: Vec<Scene> = (0..count)
        .map(|_| {
            let mut sub = Scene::default();
            sub.transforms = scene.transforms.clone();
            sub
        })
        .collect();
    let table = &scene.transforms;

    macro_rules! split {
        ($field:ident, $index:expr) => {{
            let all = std::mem::take(&mut scene.$field);
            for item in all {
                let group = table.group($index(&item));
                if group == 0 {
                    scene.$field.push(item);
                } else {
                    subs[group as usize - 1].$field.push(item);
                }
            }
        }};
    }
    split!(shadows, |s: &Shadow| crate::TransformIndex(
        s.transform_index
    ));
    split!(quads, |q: &Quad| q.transform_index());
    split!(underlines, |u: &Underline| crate::TransformIndex(
        u.transform_index
    ));
    split!(monochrome_sprites, |s: &MonochromeSprite| {
        crate::TransformIndex(s.transform_index)
    });
    split!(subpixel_sprites, |s: &SubpixelSprite| {
        crate::TransformIndex(s.transform_index)
    });
    split!(polychrome_sprites, |s: &PolychromeSprite| {
        crate::TransformIndex(s.transform_index)
    });

    // Paths carry no table index but their own group. The ids index `paths`,
    // so each scene renumbers its own.
    let all: Vec<Path<ScaledPixels>> = std::mem::take(&mut scene.paths);
    for mut path in all {
        let group = path.group;
        let target = if group == 0 {
            &mut *scene
        } else {
            &mut subs[group as usize - 1]
        };
        path.id = PathId(target.paths.len());
        target.paths.push(path);
    }
    for (sub, group) in subs.iter_mut().zip(&scene.groups) {
        let origin = group.bounds.origin;
        shift(sub, -origin.x.0, -origin.y.0);
    }
    scene.group_scenes = subs;
}

/// Moves what `scene` draws by `(dx, dy)`, so that it can be drawn into a
/// texture whose top-left corner is the group's. A primitive under a matrix
/// has its matrix moved; one under none, and every path, is moved itself.
fn shift(scene: &mut Scene, dx: f32, dy: f32) {
    let table = scene.transforms.clone();
    let moved = |index: crate::TransformIndex| table.is_unit(index);
    let by = |bounds: &mut crate::Bounds<ScaledPixels>| {
        bounds.origin.x.0 += dx;
        bounds.origin.y.0 += dy;
    };
    for shadow in &mut scene.shadows {
        if moved(crate::TransformIndex(shadow.transform_index)) {
            by(&mut shadow.bounds);
            by(&mut shadow.content_mask.bounds);
        }
    }
    for quad in &mut scene.quads {
        if moved(quad.transform_index()) {
            by(&mut quad.bounds);
            by(&mut quad.content_mask.bounds);
        }
    }
    for underline in &mut scene.underlines {
        if moved(crate::TransformIndex(underline.transform_index)) {
            by(&mut underline.bounds);
            by(&mut underline.content_mask.bounds);
        }
    }
    for sprite in &mut scene.monochrome_sprites {
        if moved(crate::TransformIndex(sprite.transform_index)) {
            by(&mut sprite.bounds);
            by(&mut sprite.content_mask.bounds);
        }
    }
    for sprite in &mut scene.subpixel_sprites {
        if moved(crate::TransformIndex(sprite.transform_index)) {
            by(&mut sprite.bounds);
            by(&mut sprite.content_mask.bounds);
        }
    }
    for sprite in &mut scene.polychrome_sprites {
        if moved(crate::TransformIndex(sprite.transform_index)) {
            by(&mut sprite.bounds);
            by(&mut sprite.content_mask.bounds);
        }
    }
    for path in &mut scene.paths {
        by(&mut path.bounds);
        by(&mut path.content_mask.bounds);
        for vertex in &mut path.vertices {
            vertex.xy_position.x.0 += dx;
            vertex.xy_position.y.0 += dy;
            by(&mut vertex.content_mask.bounds);
        }
    }
    scene.transforms.shift_entries(dx, dy);
}
