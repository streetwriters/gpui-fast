//! A layer is ordered by where it lands on screen: painted under a transform,
//! copied from the frame before, or rebuilt around a rounded clip, its
//! primitives are drawn over what they were painted over.

use crate::fast::rounded_clip::{ClipOperation, Shape, record};
use crate::{
    Bounds, ContentMask, Corners, Quad, ScaledPixels, Scene, TransformationMatrix, point, size,
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
        content_mask: ContentMask::new(self::bounds(-1000., -1000., 4000., 4000.)),
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
