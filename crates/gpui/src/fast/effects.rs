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
    Path, PathId, PolychromeSprite, ScaledPixels, Scene, Shadow, TransformIndex, TransformTable,
    Underline, scene::MonochromeSprite, scene::Quad, scene::SubpixelSprite,
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
    // Entries that move something are copied for each group, moved to the
    // group's texture, into the one table every scene of the frame shares:
    // the renderers upload only the frame's own.
    let mut table = std::mem::take(&mut scene.transforms);
    for (sub, group) in subs.iter_mut().zip(&scene.groups) {
        let origin = group.bounds.origin;
        shift(sub, &mut table, -origin.x.0, -origin.y.0);
    }
    for sub in &mut subs {
        sub.transforms = table.clone();
    }
    scene.transforms = table;
    scene.group_scenes = subs;
}

/// Moves what `scene` draws by `(dx, dy)`, so that it can be drawn into a
/// texture whose top-left corner is the group's. A primitive under a matrix
/// gets a copy of it that moves by `(dx, dy)` after its own transform; one
/// under none, and every path, is moved itself.
fn shift(scene: &mut Scene, table: &mut TransformTable, dx: f32, dy: f32) {
    let mut moved: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let by = |bounds: &mut crate::Bounds<ScaledPixels>| {
        bounds.origin.x.0 += dx;
        bounds.origin.y.0 += dy;
    };
    // The index a primitive is drawn under in the group's texture, or `None`
    // when it is moved itself.
    let mut index_in_texture = |index: TransformIndex| -> Option<TransformIndex> {
        if table.is_unit(index) {
            return None;
        }
        let new = *moved.entry(index.0).or_insert_with(|| {
            let mut matrix = table.get(index);
            matrix.translation[0] += dx;
            matrix.translation[1] += dy;
            table
                .intern_in_group(matrix, table.filter(index), table.group(index))
                .0
        });
        Some(TransformIndex(new))
    };
    macro_rules! shift_all {
        ($field:ident) => {
            for item in &mut scene.$field {
                match index_in_texture(TransformIndex(item.transform_index)) {
                    Some(index) => item.transform_index = index.0,
                    None => {
                        by(&mut item.bounds);
                        by(&mut item.content_mask.bounds);
                    }
                }
            }
        };
    }
    shift_all!(shadows);
    shift_all!(underlines);
    shift_all!(monochrome_sprites);
    shift_all!(subpixel_sprites);
    shift_all!(polychrome_sprites);
    for quad in &mut scene.quads {
        match index_in_texture(quad.transform_index()) {
            Some(index) => quad.set_transform_index(index),
            None => {
                by(&mut quad.bounds);
                by(&mut quad.content_mask.bounds);
            }
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
}
