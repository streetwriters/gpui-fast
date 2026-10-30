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
    scene.group_scenes = subs;
}
