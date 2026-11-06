//! Clipping and ordering content drawn under a transform.
//!
//! A content mask is in the space it was pushed in, and primitives are clipped
//! in their own space, so the mask in force when a transform begins is carried
//! into the transformed space. A layer is ordered by where it lands on screen,
//! as its primitives are; one copied from another scene keeps the transforms
//! its primitives had there.

use crate::{
    Bounds, ContentMask, Corners, PaintOperation, Pixels, ScaledPixels, Scene, TransformIndex,
    TransformTable, TransformationMatrix, Window, point,
};

/// Pushes the content mask in force, mapped into the space `matrix` transforms
/// from. Returns whether a mask was pushed, for [`pop_content_mask`].
pub(crate) fn push_content_mask(window: &mut Window, matrix: &TransformationMatrix) -> bool {
    let Some(inverse) = matrix.inverse() else {
        return false;
    };
    let mask = map_content_mask(&window.content_mask(), &inverse);
    window.content_mask_stack.push(mask);
    true
}

pub(crate) fn pop_content_mask(window: &mut Window, pushed: bool) {
    if pushed {
        window.content_mask_stack.pop();
    }
}

/// `mask` mapped by `matrix`. Exact for translation and scale; under a rotation
/// the mask is the box around the mapped one and loses its rounded corners.
fn map_content_mask(
    mask: &ContentMask<Pixels>,
    matrix: &TransformationMatrix,
) -> ContentMask<Pixels> {
    let map = |bounds: &Bounds<Pixels>| {
        let corners = [
            bounds.origin,
            bounds.top_right(),
            bounds.bottom_right(),
            bounds.bottom_left(),
        ]
        .map(|corner| matrix.transform_logical_point(corner));
        let (mut min, mut max) = (corners[0], corners[0]);
        for corner in &corners[1..] {
            min = point(min.x.min(corner.x), min.y.min(corner.y));
            max = point(max.x.max(corner.x), max.y.max(corner.y));
        }
        Bounds::from_corners(min, max)
    };
    let [[a, b], [c, d]] = matrix.rotation_scale;
    let axis_aligned = b == 0.0 && c == 0.0;
    if !mask.is_rounded() || !axis_aligned {
        return ContentMask::new(map(&mask.bounds));
    }
    let scale = (a.abs() + d.abs()) / 2.0;
    let radii = &mask.corner_radii;
    ContentMask {
        bounds: map(&mask.bounds),
        rounded_bounds: map(&mask.rounded_bounds),
        corner_radii: Corners {
            top_left: radii.top_left * scale,
            top_right: radii.top_right * scale,
            bottom_right: radii.bottom_right * scale,
            bottom_left: radii.bottom_left * scale,
        },
    }
}

/// Where a layer pushed now under the scene's current transform lands.
pub(crate) fn layer_screen_bounds(
    scene: &Scene,
    bounds: Bounds<ScaledPixels>,
) -> Bounds<ScaledPixels> {
    if scene.current_transform.is_identity() {
        bounds
    } else {
        scene
            .transforms
            .get(scene.current_transform)
            .transform_bounds(&bounds)
    }
}

/// Pushes a layer copied from another scene whose transforms are `transforms`,
/// where `rest` follows its start. Its primitives keep the transforms they had
/// there rather than taking `scene`'s current one, so it is ordered by where
/// they land.
pub(crate) fn push_rebuilt_layer(
    scene: &mut Scene,
    bounds: Bounds<ScaledPixels>,
    rest: &[PaintOperation],
    transforms: &TransformTable,
) {
    let screen_bounds = first_transform_in_layer(rest)
        .filter(|transform| !transform.is_identity())
        .map_or(bounds, |transform| {
            transforms.get(transform).transform_bounds(&bounds)
        });
    let order = scene.primitive_bounds.insert(screen_bounds);
    scene.layer_stack.push(order);
    scene
        .paint_operations
        .push(PaintOperation::StartLayer(bounds));
}

/// The transform of the first primitive in the layer `operations` begin inside
/// of, or `None` for a layer with none.
fn first_transform_in_layer(operations: &[PaintOperation]) -> Option<TransformIndex> {
    let mut depth = 0usize;
    for operation in operations {
        match operation {
            PaintOperation::Primitive(primitive) => return Some(primitive.transform_index()),
            PaintOperation::StartLayer(_) => depth += 1,
            PaintOperation::EndLayer if depth == 0 => return None,
            PaintOperation::EndLayer => depth -= 1,
            PaintOperation::FastRoundedClip(_) => {}
        }
    }
    None
}
