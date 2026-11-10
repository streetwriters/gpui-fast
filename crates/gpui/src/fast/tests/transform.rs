//! A layer is ordered by where it lands on screen: painted under a transform,
//! copied from the frame before, or rebuilt around a rounded clip, its
//! primitives are drawn over what they were painted over.

use crate::fast::rounded_clip::{ClipOperation, Shape, record};
use crate::{
    Bounds, ClipMask, Corners, Quad, ScaledPixels, Scene, TransformationMatrix, point, size,
};

fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(w), ScaledPixels(h)),
    }
}

fn quad(bounds: Bounds<ScaledPixels>) -> Quad {
    Quad {
        bounds,
        content_mask: ClipMask::new(self::bounds(-1000., -1000., 4000., 4000.)),
        ..Default::default()
    }
}

/// A background, and over it a layer holding one quad, painted 500 px to the
/// right of where a transform moves them on screen: off the window, where
/// nothing else is.
fn paint_moved_panel(scene: &mut Scene) {
    let previous = scene.push_transform(TransformationMatrix {
        rotation_scale: [[1., 0.], [0., 1.]],
        translation: [-500., 0.],
    });
    scene.insert_primitive(quad(bounds(500., 0., 100., 40.)));
    scene.push_layer(bounds(510., 10., 20., 20.));
    scene.insert_primitive(quad(bounds(510., 10., 20., 20.)));
    scene.pop_layer();
    scene.pop_transform(previous);
}

/// The ordering of the background and of the quad in the layer.
fn orders(scene: &Scene) -> (u32, u32) {
    let order = |bounds: Bounds<ScaledPixels>| {
        scene
            .quads
            .iter()
            .find(|quad| quad.bounds == bounds)
            .expect("painted quad")
            .order
    };
    (
        order(bounds(500., 0., 100., 40.)),
        order(bounds(510., 10., 20., 20.)),
    )
}

#[test]
fn a_layer_painted_under_a_transform_is_drawn_over_its_background() {
    let mut scene = Scene::default();
    paint_moved_panel(&mut scene);
    scene.finish();
    let (background, layer) = orders(&scene);
    assert!(layer > background, "layer {layer}, background {background}");
}

#[test]
fn a_replayed_layer_is_drawn_over_its_background() {
    let mut previous = Scene::default();
    paint_moved_panel(&mut previous);
    let mut scene = Scene::default();
    scene.replay(0..previous.len(), &previous);
    scene.finish();
    let (background, layer) = orders(&scene);
    assert!(layer > background, "layer {layer}, background {background}");
}

#[test]
fn a_layer_rebuilt_around_a_rounded_clip_is_drawn_over_its_background() {
    let mut scene = Scene::default();
    paint_moved_panel(&mut scene);
    let clip = Shape {
        bounds: bounds(300., 300., 50., 50.),
        radii: Corners {
            top_left: size(ScaledPixels(8.), ScaledPixels(8.)),
            top_right: size(ScaledPixels(8.), ScaledPixels(8.)),
            bottom_right: size(ScaledPixels(8.), ScaledPixels(8.)),
            bottom_left: size(ScaledPixels(8.), ScaledPixels(8.)),
        },
    };
    record(&mut scene, ClipOperation::Begin(clip));
    scene.insert_primitive(quad(bounds(300., 300., 50., 50.)));
    record(&mut scene, ClipOperation::End);
    scene.finish();
    let (background, layer) = orders(&scene);
    assert!(layer > background, "layer {layer}, background {background}");
}

fn moved_by(x: f32, y: f32) -> TransformationMatrix {
    TransformationMatrix {
        rotation_scale: [[1., 0.], [0., 1.]],
        translation: [x, y],
    }
}

/// A row 1000 px down its list, as a recycling list places it: laid out at
/// the top and moved by a transform, with a text layer.
fn moved_row(matrix: TransformationMatrix) -> Scene {
    let mut scene = Scene::default();
    let previous = scene.push_transform(matrix);
    scene.insert_primitive(quad(bounds(0., 0., 100., 40.)));
    scene.push_layer(bounds(10., 10., 50., 20.));
    scene.insert_primitive(quad(bounds(10., 10., 50., 20.)));
    scene.pop_layer();
    scene.pop_transform(previous);
    scene
}

#[test]
fn layer_content_moved_by_a_translation_lands_where_it_moves_to() {
    let (content, tileable) = crate::fast::layers::paint::translated_scene(
        &moved_row(moved_by(0., 1000.)),
        point(ScaledPixels(0.), ScaledPixels(-100.)),
    );
    assert!(tileable);
    let row = content
        .quads
        .iter()
        .find(|quad| quad.bounds.size.width == ScaledPixels(100.))
        .expect("row");
    assert_eq!(row.bounds, bounds(0., 900., 100., 40.));
    assert_eq!(row.transform_index().0, 0);
    let layer = content
        .paint_operations
        .iter()
        .find_map(|operation| match operation {
            crate::PaintOperation::StartLayer(bounds) => Some(*bounds),
            _ => None,
        });
    assert_eq!(layer, Some(bounds(10., 910., 50., 20.)));
}

#[test]
fn layer_content_under_a_scale_keeps_it_and_cannot_be_tiled() {
    let (content, tileable) = crate::fast::layers::paint::translated_scene(
        &moved_row(TransformationMatrix {
            rotation_scale: [[2., 0.], [0., 2.]],
            translation: [0., 1000.],
        }),
        point(ScaledPixels(0.), ScaledPixels(-100.)),
    );
    assert!(!tileable);
    let row = &content.quads[0];
    let matrix = content.transforms.get(row.transform_index());
    assert_eq!(matrix.rotation_scale, [[2., 0.], [0., 2.]]);
    assert_eq!(matrix.translation, [0., 900.]);
}
