//! Pixel tests of scroll layers: the same content drawn directly and through
//! layer tiles must come out byte for byte the same.
//!
//! [`Harness`] draws scenes without a window: it owns the GPU objects the
//! renderer would, built by the renderer's own constructors, and records
//! through `fast::frame` as the renderer does, into a texture it reads back.

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;
use gpui::{
    AtlasKey, AtlasTile, Bounds, ContentMask, Corners, DevicePixels, Edges, FontId, GlyphId, Hsla,
    ImageId, LayerFrame, LayerKey, MonochromeSprite, Path, PlatformAtlas, PolychromeSprite, Quad,
    RenderGlyphParams, RenderImageParams, Rgba, ScaledPixels, Scene, SceneLayers, Shadow, Size,
    SubpixelSprite, TileCoord, TransformationMatrix, Underline, layer_tile_id,
    layer_tile_texture_id, linear_color_stop, linear_gradient, point, px, rgba, size,
};

use crate::WgpuAtlas;
use crate::fast::frame::{FrameHost, FrameState, FrameTarget};
use crate::fast::layers::TileCache;
use crate::wgpu_renderer::{
    GammaParams, GlobalParams, RenderingParameters, WgpuBindGroupLayouts, WgpuPipelines,
    WgpuRendererCore,
};

#[test]
fn harness_renders_a_quad_at_the_right_pixels() {
    let Some(mut harness) = Harness::new() else {
        eprintln!("skipped: no wgpu adapter");
        return;
    };
    let mut scene = Scene::default();
    scene.insert_primitive(quad(sp(4., 4., 8., 8.), Hsla::from(rgba(0xff0000ff))));
    scene.finish();
    let pixels = harness.render(&scene, device_size(32, 32), rgba(0xffffffff));
    assert_eq!(pixel(&pixels, 32, 8, 8), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 32, 1, 1), [255, 255, 255, 255]);
}

const TILE: u32 = 512;

#[test]
fn a_rasterized_tile_equals_the_same_content_drawn_directly() {
    let Some(mut harness) = Harness::new() else {
        eprintln!("skipped: no wgpu adapter");
        return;
    };
    let direct = harness.render(
        &content(&harness, (0., 0.), no_mask().bounds, Paths::With),
        device_size(1024, 1024),
        background(),
    );
    let tiles = [(0, 0), (1, 0), (0, 1), (1, 1)];
    let content = content(&harness, (0., 0.), no_mask().bounds, Paths::With);
    let assembled = rasterize_and_assemble(&mut harness, content, &tiles);
    assert_same_pixels(&assembled, &direct, 1024);
}

#[test]
fn tiles_of_negative_coordinates_rasterize_like_positive_ones() {
    let Some(mut harness) = Harness::new() else {
        eprintln!("skipped: no wgpu adapter");
        return;
    };
    let direct = harness.render(
        &content(&harness, (0., 0.), no_mask().bounds, Paths::With),
        device_size(1024, 1024),
        background(),
    );
    let tiles = [(-1, -1), (0, -1), (-1, 0), (0, 0)];
    let shifted = content(
        &harness,
        (-(TILE as f32), -(TILE as f32)),
        no_mask().bounds,
        Paths::With,
    );
    let assembled = rasterize_and_assemble(&mut harness, shifted, &tiles);
    assert_same_pixels(&assembled, &direct, 1024);
}

#[test]
fn a_composited_layer_equals_direct_drawing() {
    let Some(mut harness) = Harness::new() else {
        eprintln!("skipped: no wgpu adapter");
        return;
    };
    composite_and_compare(&mut harness, (37., -91.), Paths::Without);
}

/// Paths are drawn exactly when the content moved by an even number of
/// device pixels since its tiles were rasterized. At an odd translation an
/// antialiased path edge pixel can come out one level apart: the path
/// shader's `dpdx` and `dpdy` are differences within 2×2 pixel quads,
/// which then pair other pixels.
#[test]
fn paths_composite_exactly_at_even_translations() {
    let Some(mut harness) = Harness::new() else {
        eprintln!("skipped: no wgpu adapter");
        return;
    };
    composite_and_compare(&mut harness, (38., -92.), Paths::With);
}

/// Paths something draws over stay out of the tiles: the core draws them,
/// and what it draws over them, over the composited tiles
/// (`gpui::fast::layers::overlay`), as without a layer. That holds at odd
/// translations too, as the paths are rasterized where they show.
#[test]
fn covered_paths_drawn_over_the_tiles_equal_direct_drawing() {
    let Some(mut harness) = Harness::new() else {
        eprintln!("skipped: no wgpu adapter");
        return;
    };
    for translation in [(37., -91.), (38., -92.), (3., 5.), (101., -233.)] {
        composite_and_compare(&mut harness, translation, Paths::Covered);
    }
}

/// Draws a window with a viewport over a scroll layer at `translation`, once
/// from the content directly and once from the layer's tiles, twice (the
/// second frame from the cached tiles), and compares the pixels.
fn composite_and_compare(harness: &mut Harness, translation: (f32, f32), paths: Paths) {
    let window = device_size(700, 500);
    let viewport = sp(40., 30., 600., 400.);
    let panel = quad(sp(0., 0., 700., 500.), Hsla::from(background()));
    let scrollbar = quad(sp(620., 40., 12., 120.), Hsla::from(rgba(0x888888ff)));

    let mut direct = Scene::default();
    direct.insert_primitive(panel);
    add_content(&mut direct, harness, translation, viewport, paths);
    if paths == Paths::Covered {
        add_covered_paths(&mut direct, harness, translation, viewport);
    }
    direct.insert_primitive(scrollbar);
    direct.finish();
    let expected = harness.render(&direct, window, rgba(0x000000ff));

    let key = LayerKey(5);
    let tiles = [coord(0, 0), coord(1, 0), coord(0, 1), coord(1, 1)];
    let layer = layer_frame(
        key,
        1,
        content(harness, (0., 0.), no_mask().bounds, paths),
        &tiles,
    );
    let mut composited = Scene::default();
    composited.insert_primitive(panel);
    composited.push_layer(viewport);
    for tile in tiles {
        let bounds = layer.tile_bounds(tile);
        composited.insert_primitive(PolychromeSprite {
            order: 0,
            transform_index: 0,
            grayscale: false.into(),
            opacity: 1.,
            bounds: sp(
                bounds.origin.x.0 + translation.0,
                bounds.origin.y.0 + translation.1,
                TILE as f32,
                TILE as f32,
            ),
            content_mask: ContentMask::new(viewport),
            corner_radii: Corners::default(),
            tile: AtlasTile {
                texture_id: layer_tile_texture_id(key),
                tile_id: layer_tile_id(tile),
                padding: 0,
                bounds: Bounds {
                    origin: point(DevicePixels(0), DevicePixels(0)),
                    size: device_size(TILE as i32, TILE as i32),
                },
            },
        });
    }
    composited.pop_layer();
    if paths == Paths::Covered {
        add_covered_paths(&mut composited, harness, translation, viewport);
    }
    composited.insert_primitive(scrollbar);
    composited.layers.frames.push(layer);
    composited.finish();
    let actual = harness.render(&composited, window, rgba(0x000000ff));
    assert_same_pixels(&actual, &expected, 700);

    // A frame that only scrolled draws the cached tiles again.
    let actual = harness.render(&composited, window, rgba(0x000000ff));
    assert_same_pixels(&actual, &expected, 700);
}

#[test]
fn dirty_tiles_are_rasterized_once_they_are_shown() {
    let mut cache = TileCache::default();
    let key = LayerKey(3);
    let all = [coord(0, 0), coord(1, 0), coord(0, 1), coord(1, 1)];
    let mut rasterized = 0;
    let mut frame =
        |cache: &mut TileCache, generation, dirty: &[TileCoord], shown: &[TileCoord]| {
            let layers = scene_layers(key, generation, dirty);
            let planned = cache.begin_frame(&layers, shown.iter().map(|tile| (key, *tile)));
            rasterized += planned.len();
            planned
                .into_iter()
                .map(|(_, tile)| tile)
                .collect::<Vec<_>>()
        };

    let [a, b, c, _] = all;
    // A new layer: only the dirty tiles shown. The others wait until they are.
    let mut shown = vec![a, b];
    shown.sort();
    assert_eq!(frame(&mut cache, 1, &all, &[a, b]), shown);
    // The same generation again: nothing.
    assert_eq!(frame(&mut cache, 1, &all, &[a, b]), vec![]);
    // The next generation dirties a held tile that is not shown: it waits.
    assert_eq!(frame(&mut cache, 2, &[b, c], &[a]), vec![]);
    // Shown again, it is rasterized, and only it.
    assert_eq!(frame(&mut cache, 2, &[b, c], &[a, b]), vec![b]);
    // A dirty tile never shown before is rasterized once it is.
    assert_eq!(frame(&mut cache, 3, &[], &[a, b, c]), vec![c]);
    // A skipped generation may have dirtied any tile: shown tiles again.
    assert_eq!(frame(&mut cache, 5, &[], &[a]), vec![a]);
    // A shown tile the cache never had.
    assert_eq!(frame(&mut cache, 5, &[], &[coord(2, 0)]), vec![coord(2, 0)]);
    assert_eq!(rasterized, 6);
}

#[test]
fn tiles_are_evicted_least_recently_composited_first_within_the_budget() {
    const SIDE: u32 = 16;
    let tile_bytes = (SIDE * SIDE * 4) as u64;
    let key = LayerKey(4);
    let mut cache = TileCache::default();
    let layers = SceneLayers {
        frames: vec![LayerFrame {
            tile_size: SIDE,
            ..layer_frame(key, 1, Scene::default(), &[])
        }],
    };
    let frame = |cache: &mut TileCache, shown: &[TileCoord], budget: u64| {
        cache.begin_frame(&layers, shown.iter().map(|tile| (key, *tile)));
        cache.evict_to_budget(budget);
    };
    let (a, b, c, d, e) = (
        coord(0, 0),
        coord(1, 0),
        coord(2, 0),
        coord(3, 0),
        coord(4, 0),
    );

    frame(&mut cache, &[a, b, c], 3 * tile_bytes);
    frame(&mut cache, &[b, c, d], 3 * tile_bytes);
    assert!(
        !cache.holds(key, a),
        "the least recently composited tile goes"
    );
    assert!(cache.holds(key, b) && cache.holds(key, c) && cache.holds(key, d));

    frame(&mut cache, &[c], 3 * tile_bytes);
    frame(&mut cache, &[d, e], tile_bytes);
    assert!(!cache.holds(key, b) && !cache.holds(key, c));
    assert!(
        cache.holds(key, d) && cache.holds(key, e),
        "tiles composited this frame stay, over budget or not"
    );
}

#[test]
fn resize_releases_every_tile() {
    let Some(mut harness) = Harness::new() else {
        eprintln!("skipped: no wgpu adapter");
        return;
    };
    let key = LayerKey(1);
    let tiles = [(0, 0), (1, 0), (0, 1), (1, 1)];
    let content = content(&harness, (0., 0.), no_mask().bounds, Paths::With);
    rasterize_and_assemble(&mut harness, content, &tiles);
    assert!(harness.state().layers.holds(key, coord(0, 0)));

    // What `WgpuRenderer::update_drawable_size` calls.
    TileCache::clear(&mut harness.state().layers);
    assert!(harness.state().layers.is_empty());

    // The next frame rasterizes the tiles it composites again.
    let content = content_again(&harness);
    let assembled = rasterize_and_assemble(&mut harness, content, &tiles);
    let direct = harness.render(
        &content_again(&harness),
        device_size(1024, 1024),
        background(),
    );
    assert_same_pixels(&assembled, &direct, 1024);
}

fn content_again(harness: &Harness) -> Scene {
    content(harness, (0., 0.), no_mask().bounds, Paths::With)
}

// --- layer helpers --- //

fn coord(x: i32, y: i32) -> TileCoord {
    TileCoord { x, y }
}

fn background() -> Rgba {
    rgba(0x336699ff)
}

fn layer_frame(key: LayerKey, generation: u64, content: Scene, dirty: &[TileCoord]) -> LayerFrame {
    LayerFrame {
        key,
        generation,
        background: background(),
        tile_size: TILE,
        content: content.into(),
        dirty_tiles: dirty.to_vec(),
    }
}

fn scene_layers(key: LayerKey, generation: u64, dirty: &[TileCoord]) -> SceneLayers {
    SceneLayers {
        frames: vec![layer_frame(key, generation, Scene::default(), dirty)],
    }
}

/// Draws a frame that rasterizes `tiles` of `content` and puts the tiles
/// together into one image, `tiles[0]` at its top left.
fn rasterize_and_assemble(harness: &mut Harness, content: Scene, tiles: &[(i32, i32)]) -> Vec<u8> {
    let key = LayerKey(1);
    let coords: Vec<TileCoord> = tiles.iter().map(|&(x, y)| coord(x, y)).collect();
    // The frame composites the tiles, where they were painted: tiles are
    // rasterized once a frame shows them.
    let layer = layer_frame(key, 1, content, &coords);
    let mut frame = Scene::default();
    for &tile in &coords {
        let bounds = layer.tile_bounds(tile);
        frame.insert_primitive(PolychromeSprite {
            order: 0,
            transform_index: 0,
            grayscale: false.into(),
            opacity: 1.,
            bounds,
            content_mask: ContentMask::new(bounds),
            corner_radii: Corners::default(),
            tile: AtlasTile {
                texture_id: layer_tile_texture_id(key),
                tile_id: layer_tile_id(tile),
                padding: 0,
                bounds: Bounds {
                    origin: point(DevicePixels(0), DevicePixels(0)),
                    size: device_size(TILE as i32, TILE as i32),
                },
            },
        });
    }
    frame.layers.frames.push(layer);
    frame.finish();
    harness.render(&frame, device_size(16, 16), background());

    let (min_x, min_y) = (tiles[0].0, tiles[0].1);
    let side = TILE as usize;
    let width = 2 * side;
    let mut image = vec![0; width * width * 4];
    for tile in coords {
        let texture = harness
            .state()
            .layers
            .tile_texture(key, tile)
            .expect("tile rasterized")
            .clone();
        let pixels = read_back(
            harness.device(),
            harness.queue(),
            &texture,
            harness.format(),
        );
        let x0 = (tile.x - min_x) as usize * side;
        let y0 = (tile.y - min_y) as usize * side;
        for y in 0..side {
            let from = y * side * 4;
            let to = ((y0 + y) * width + x0) * 4;
            image[to..to + side * 4].copy_from_slice(&pixels[from..from + side * 4]);
        }
    }
    image
}

fn assert_same_pixels(actual: &[u8], expected: &[u8], width: usize) {
    assert_eq!(actual.len(), expected.len());
    let differing: Vec<(usize, usize)> = (0..actual.len() / 4)
        .filter(|i| actual[i * 4..i * 4 + 4] != expected[i * 4..i * 4 + 4])
        .map(|i| (i % width, i / width))
        .collect();
    if differing.is_empty() {
        return;
    }
    let (min_x, max_x) = differing
        .iter()
        .fold((usize::MAX, 0), |(lo, hi), (x, _)| (lo.min(*x), hi.max(*x)));
    let (min_y, max_y) = differing
        .iter()
        .fold((usize::MAX, 0), |(lo, hi), (_, y)| (lo.min(*y), hi.max(*y)));
    panic!(
        "{} pixels differ, within x {min_x}..={max_x}, y {min_y}..={max_y}; first at {:?}: {:?} instead of {:?}",
        differing.len(),
        differing[0],
        pixel(actual, width, differing[0].0, differing[0].1),
        pixel(expected, width, differing[0].0, differing[0].1),
    );
}

/// Content of every primitive kind, spread over the four tiles of
/// (0, 0)..(1024, 1024) and across their edges, moved by `offset` and
/// clipped to `clip`.
fn content(
    harness: &Harness,
    offset: (f32, f32),
    clip: Bounds<ScaledPixels>,
    paths: Paths,
) -> Scene {
    let mut scene = Scene::default();
    add_content(&mut scene, harness, offset, clip, paths);
    scene.finish();
    scene
}

/// Inserts [`content`]'s primitives into `scene`.
#[derive(Clone, Copy, PartialEq)]
enum Paths {
    With,
    Without,
    /// No path in the tiles; [`add_covered_paths`] over them.
    Covered,
}

fn add_content(
    scene: &mut Scene,
    harness: &Harness,
    offset: (f32, f32),
    clip: Bounds<ScaledPixels>,
    paths: Paths,
) {
    let (ox, oy) = offset;
    let at = |x: f32, y: f32, w: f32, h: f32| sp(x + ox, y + oy, w, h);
    let mask = |bounds: Bounds<ScaledPixels>| ContentMask::new(bounds.intersect(&clip));

    scene.insert_primitive(Quad {
        content_mask: mask(clip),
        ..quad(at(100., 100., 200., 150.), Hsla::from(rgba(0xcc3322ff)))
    });
    scene.insert_primitive(Quad {
        bounds: at(450.5, 60., 150., 120.),
        content_mask: mask(at(0., 0., 1024., 1024.)),
        background: Hsla::from(rgba(0xeeeeeeff)).into(),
        border_color: Hsla::from(rgba(0x222222ff)),
        corner_radii: Corners::all(ScaledPixels(12.)),
        border_widths: Edges::all(ScaledPixels(3.)),
        ..Default::default()
    });
    // The shadow's tail crosses the tile edge its bounds stop short of.
    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: ScaledPixels(8.),
        bounds: at(380., 420., 120., 70.),
        corner_radii: Corners::all(ScaledPixels(6.)),
        content_mask: mask(clip),
        color: Hsla::from(rgba(0x00000080)),
        element_bounds: at(380., 420., 120., 70.),
        element_corner_radii: Corners::all(ScaledPixels(6.)),
        inset: 0,
        transform_index: 0,
    });
    scene.insert_primitive(Quad {
        bounds: at(600., 600., 300., 200.),
        content_mask: mask(clip),
        background: linear_gradient(
            45.,
            linear_color_stop(rgba(0xff0000ff), 0.),
            linear_color_stop(rgba(0x0000ffff), 1.),
        ),
        corner_radii: Corners::all(ScaledPixels(20.)),
        ..Default::default()
    });
    scene.insert_primitive(Underline {
        order: 0,
        transform_index: 0,
        bounds: at(50., 505., 900., 8.),
        content_mask: mask(clip),
        color: Hsla::from(rgba(0xffcc00ff)),
        thickness: ScaledPixels(2.),
        wavy: true.into(),
    });
    // A clipped quad whose mask crosses a tile edge.
    scene.insert_primitive(Quad {
        bounds: at(700., 200., 200., 200.),
        content_mask: mask(at(490., 250., 300., 100.)),
        background: Hsla::from(rgba(0x44aa44ff)).into(),
        ..Default::default()
    });

    if paths == Paths::With {
        add_path(scene, offset, clip);
    }

    let mono = glyph_tile(harness, 1, false);
    scene.insert_primitive(MonochromeSprite {
        order: 0,
        transform_index: 0,
        bounds: at(505., 700., 16., 16.),
        content_mask: mask(clip),
        color: Hsla::from(rgba(0xffffffff)),
        tile: mono,
    });
    if !harness.dual_source_blending {
        eprintln!("no dual-source blending: subpixel sprites left out");
    } else {
        let subpixel = glyph_tile(harness, 2, true);
        scene.insert_primitive(SubpixelSprite {
            order: 0,
            transform_index: 0,
            bounds: at(520., 505., 16., 16.),
            content_mask: mask(clip),
            color: Hsla::from(rgba(0x000000ff)),
            tile: subpixel,
        });
    }
    scene.insert_primitive(PolychromeSprite {
        order: 0,
        transform_index: 0,
        grayscale: false.into(),
        opacity: 1.,
        bounds: at(300., 500., 20., 20.),
        content_mask: mask(clip),
        corner_radii: Corners::all(ScaledPixels(4.)),
        tile: image_tile(harness),
    });
}

/// A path of lines and a curve across a tile edge.
fn add_path(scene: &mut Scene, (ox, oy): (f32, f32), clip: Bounds<ScaledPixels>) {
    let mut path = Path::new(point(px(480. + ox), px(300. + oy)));
    path.line_to(point(px(560.5 + ox), px(330. + oy)));
    path.curve_to(
        point(px(500. + ox), px(560. + oy)),
        point(px(600. + ox), px(450. + oy)),
    );
    path.line_to(point(px(480. + ox), px(300. + oy)));
    path.content_mask = ContentMask::new(clip.map(|c| px(c.0)));
    path.color = Hsla::from(rgba(0x8800ffcc)).into();
    scene.insert_primitive(path.scale(1.));
}

/// What GPUI Kit draws for a table's rounded frame: paths filling corner
/// notches, a bordered quad and a glyph drawn over them, and a path nothing
/// covers, moved by `offset` and clipped to `clip`, in drawing order.
fn add_covered_paths(
    scene: &mut Scene,
    harness: &Harness,
    (ox, oy): (f32, f32),
    clip: Bounds<ScaledPixels>,
) {
    let mask = ContentMask::new(clip);
    for (x, y, dx, dy) in [
        (200., 330., 1., 1.),
        (330.5, 330., -1., 1.),
        (200., 410.25, 1., -1.),
    ] {
        let corner = point(px(x + ox), px(y + oy));
        let mut path = Path::new(corner);
        path.line_to(corner + point(px(9. * dx), px(0.)));
        path.curve_to(corner + point(px(0.), px(9. * dy)), corner);
        path.line_to(corner);
        path.content_mask = ContentMask::new(clip.map(|c| px(c.0)));
        path.color = Hsla::from(rgba(0x3366ccff)).into();
        scene.insert_primitive(path.scale(1.));
    }
    scene.insert_primitive(Quad {
        bounds: sp(200. + ox, 330. + oy, 130.5, 80.25),
        content_mask: mask,
        background: Hsla::from(rgba(0x00000000)).into(),
        border_color: Hsla::from(rgba(0x222222ff)),
        corner_radii: Corners::all(ScaledPixels(9.)),
        border_widths: Edges::all(ScaledPixels(1.)),
        ..Default::default()
    });
    scene.insert_primitive(MonochromeSprite {
        order: 0,
        transform_index: 0,
        bounds: sp(204. + ox, 334. + oy, 16., 16.),
        content_mask: mask,
        color: Hsla::from(rgba(0xff8800ff)),
        tile: glyph_tile(harness, 1, false),
    });
    let mut path = Path::new(point(px(250. + ox), px(360. + oy)));
    path.line_to(point(px(290.5 + ox), px(372. + oy)));
    path.curve_to(
        point(px(255. + ox), px(400. + oy)),
        point(px(300. + ox), px(395. + oy)),
    );
    path.line_to(point(px(250. + ox), px(360. + oy)));
    path.content_mask = ContentMask::new(clip.map(|c| px(c.0)));
    path.color = Hsla::from(rgba(0x8800ffcc)).into();
    scene.insert_primitive(path.scale(1.));
}

/// A 16×16 glyph of a made-up font, uploaded to the harness's atlas: a
/// monochrome one, or a subpixel one.
fn glyph_tile(harness: &Harness, glyph: u32, subpixel: bool) -> AtlasTile {
    let key = AtlasKey::Glyph(RenderGlyphParams {
        font_id: FontId(9_999),
        glyph_id: GlyphId(glyph),
        font_size: px(12.),
        subpixel_variant: point(0, 0),
        scale_factor: 1.,
        is_emoji: false,
        subpixel_rendering: subpixel,
        dilation: 0,
    });
    let bytes_per_pixel = if subpixel { 4 } else { 1 };
    let bytes: Vec<u8> = (0..16 * 16 * bytes_per_pixel)
        .map(|i| ((i * 37) % 256) as u8)
        .collect();
    harness
        .atlas
        .get_or_insert_with(key, &mut || {
            Ok(Some((device_size(16, 16), Cow::Owned(bytes.clone()))))
        })
        .expect("glyph uploaded")
        .expect("glyph tile")
}

/// A 20×20 image uploaded to the harness's atlas.
fn image_tile(harness: &Harness) -> AtlasTile {
    let key = AtlasKey::Image(RenderImageParams {
        image_id: ImageId(9_999),
        frame_index: 0,
    });
    let bytes: Vec<u8> = (0..20 * 20)
        .flat_map(|i: u32| {
            [
                (i * 7 % 256) as u8,
                (i * 13 % 256) as u8,
                (i * 3 % 256) as u8,
                255,
            ]
        })
        .collect();
    harness
        .atlas
        .get_or_insert_with(key, &mut || {
            Ok(Some((device_size(20, 20), Cow::Owned(bytes.clone()))))
        })
        .expect("image uploaded")
        .expect("image tile")
}

// --- helpers --- //

pub(super) fn sp(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(w), ScaledPixels(h)),
    }
}

/// A content mask that clips nothing the tests draw.
pub(super) fn no_mask() -> ContentMask<ScaledPixels> {
    ContentMask::new(sp(-10_000., -10_000., 20_000., 20_000.))
}

pub(super) fn quad(bounds: Bounds<ScaledPixels>, color: Hsla) -> Quad {
    Quad {
        bounds,
        content_mask: no_mask(),
        background: color.into(),
        ..Default::default()
    }
}

pub(super) fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    size(DevicePixels(width), DevicePixels(height))
}

/// The RGBA bytes of the pixel at (`x`, `y`) of a `width`-wide readback.
pub(super) fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let at = (y * width + x) * 4;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

const INITIAL_INSTANCE_CAPACITY: u64 = 2 * 1024 * 1024;

/// Draws scenes into textures without a surface, through the renderer's
/// pipelines and frame recording.
pub(super) struct Harness {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    format: wgpu::TextureFormat,
    atlas: WgpuAtlas,
    pipelines: WgpuPipelines,
    bind_group_layouts: WgpuBindGroupLayouts,
    atlas_sampler: wgpu::Sampler,
    rendering_params: RenderingParameters,
    globals_buffer: wgpu::Buffer,
    transform_buffer: wgpu::Buffer,
    path_globals_offset: u64,
    gamma_offset: u64,
    globals_bind_group: wgpu::BindGroup,
    path_globals_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    instance_alignment: u64,
    path_targets: Option<PathTargets>,
    state: FrameState,
    dual_source_blending: bool,
}

/// The path intermediate texture, and its multisampled twin, of one size.
struct PathTargets {
    size: Size<DevicePixels>,
    _intermediate: wgpu::Texture,
    intermediate_view: wgpu::TextureView,
    _msaa: Option<wgpu::Texture>,
    msaa_view: Option<wgpu::TextureView>,
}

impl Harness {
    /// A harness on the first adapter wgpu offers without a surface, or
    /// `None` when there is none (the tests then skip).
    pub(super) fn new() -> Option<Harness> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL | wgpu::Backends::METAL,
            flags: wgpu::InstanceFlags::default(),
            backend_options: wgpu::BackendOptions::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            display: None,
        });
        let adapter = gpui::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok()?;
        let (device, queue, dual_source_blending, color_texture_format) =
            gpui::block_on(crate::WgpuContext::create_device(&adapter)).ok()?;
        let (device, queue) = (Arc::new(device), Arc::new(queue));

        // The surface's choice when it offers both.
        let usages = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let format = [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
        ]
        .into_iter()
        .find(|format| {
            adapter
                .get_texture_format_features(*format)
                .allowed_usages
                .contains(usages)
        })?;

        let rendering_params = RenderingParameters::new(&adapter, format);
        let bind_group_layouts = WgpuRendererCore::create_bind_group_layouts(&device, false);
        let pipelines = WgpuRendererCore::create_pipelines(
            &device,
            &bind_group_layouts,
            format,
            wgpu::CompositeAlphaMode::Opaque,
            rendering_params.path_sample_count,
            dual_source_blending,
            false,
        );
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let uniform_alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let globals_size = size_of::<GlobalParams>() as u64;
        let gamma_size = size_of::<GammaParams>() as u64;
        let path_globals_offset = globals_size.next_multiple_of(uniform_alignment);
        let gamma_offset = (path_globals_offset + globals_size).next_multiple_of(uniform_alignment);
        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals_buffer"),
            size: gamma_offset + gamma_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let transform_buffer = crate::wgpu_renderer::create_transform_buffer(&device);
        let globals_bind_group = |label: &str, offset: u64| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &bind_group_layouts.globals,
                entries: &crate::wgpu_renderer::globals_entries(
                    (&globals_buffer, offset, globals_size),
                    (&globals_buffer, gamma_offset, gamma_size),
                    Some(&transform_buffer),
                ),
            })
        };
        let path_globals_bind_group =
            globals_bind_group("path_globals_bind_group", path_globals_offset);
        let globals_bind_group = globals_bind_group("globals_bind_group", 0);

        let instance_alignment = device.limits().min_storage_buffer_offset_alignment as u64;
        let instance_buffer = create_instance_buffer(&device, INITIAL_INSTANCE_CAPACITY);
        let atlas = WgpuAtlas::new(device.clone(), queue.clone(), color_texture_format);

        Some(Harness {
            device,
            queue,
            format,
            atlas,
            pipelines,
            bind_group_layouts,
            atlas_sampler,
            rendering_params,
            globals_buffer,
            transform_buffer,
            path_globals_offset,
            gamma_offset,
            globals_bind_group,
            path_globals_bind_group,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
            instance_alignment,
            path_targets: None,
            state: FrameState::default(),
            dual_source_blending,
        })
    }

    pub(super) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub(super) fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub(super) fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    pub(super) fn state(&mut self) -> &mut FrameState {
        &mut self.state
    }

    /// Draws `scene` into a `size` texture cleared to `clear`, as the
    /// renderer draws a frame, and returns its pixels as RGBA bytes, row by row.
    pub(super) fn render(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
        clear: Rgba,
    ) -> Vec<u8> {
        let (width, height) = (size.width.0 as u32, size.height.0 as u32);
        self.write_globals(width, height);
        self.ensure_path_targets(size);
        self.atlas.before_frame();

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("harness_target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let clear = wgpu::Color {
            r: clear.r as f64,
            g: clear.g as f64,
            b: clear.b as f64,
            a: clear.a as f64,
        };
        crate::fast::globals::write_transforms(
            &mut Default::default(),
            &self.queue,
            &self.transform_buffer,
            &scene.transforms,
        );
        crate::fast::frame::record_into(self, scene, &view, clear).expect("frame recorded");
        read_back(&self.device, &self.queue, &texture, self.format)
    }

    fn write_globals(&self, width: u32, height: u32) {
        let globals = GlobalParams {
            viewport_size: [width as f32, height as f32],
            premultiplied_alpha: 0,
            pad: 0,
        };
        let gamma = GammaParams {
            gamma_ratios: self.rendering_params.gamma_ratios,
            grayscale_enhanced_contrast: self.rendering_params.grayscale_enhanced_contrast,
            subpixel_enhanced_contrast: self.rendering_params.subpixel_enhanced_contrast,
            is_bgr: 0,
            _pad: 0,
        };
        self.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));
        self.queue.write_buffer(
            &self.globals_buffer,
            self.path_globals_offset,
            bytemuck::bytes_of(&globals),
        );
        self.queue.write_buffer(
            &self.globals_buffer,
            self.gamma_offset,
            bytemuck::bytes_of(&gamma),
        );
    }

    fn ensure_path_targets(&mut self, size: Size<DevicePixels>) {
        if self
            .path_targets
            .as_ref()
            .is_some_and(|targets| targets.size == size)
        {
            return;
        }
        let (width, height) = (size.width.0 as u32, size.height.0 as u32);
        let (intermediate, intermediate_view) =
            WgpuRendererCore::create_path_intermediate(&self.device, self.format, width, height);
        let (msaa, msaa_view) = WgpuRendererCore::create_msaa_if_needed(
            &self.device,
            self.format,
            width,
            height,
            self.rendering_params.path_sample_count,
        )
        .unzip();
        self.path_targets = Some(PathTargets {
            size,
            _intermediate: intermediate,
            intermediate_view,
            _msaa: msaa,
            msaa_view,
        });
    }
}

impl FrameHost for Harness {
    fn frame_state(&mut self) -> &mut FrameState {
        &mut self.state
    }

    fn instance_data_alignment(&self) -> u64 {
        self.instance_alignment.max(1)
    }

    fn reserve_instance_data(&mut self, size: u64) -> Result<()> {
        if size > self.instance_capacity {
            self.instance_capacity = size.next_power_of_two();
            self.instance_buffer = create_instance_buffer(&self.device, self.instance_capacity);
        }
        Ok(())
    }

    fn target(&self) -> Result<FrameTarget<'_>> {
        let path_targets = self.path_targets.as_ref();
        Ok(FrameTarget {
            device: &self.device,
            queue: &self.queue,
            pipelines: &self.pipelines,
            bind_group_layouts: &self.bind_group_layouts,
            atlas: &self.atlas,
            atlas_sampler: &self.atlas_sampler,
            globals_bind_group: &self.globals_bind_group,
            path_globals_bind_group: &self.path_globals_bind_group,
            path_intermediate_view: path_targets.map(|targets| &targets.intermediate_view),
            path_msaa_view: path_targets.and_then(|targets| targets.msaa_view.as_ref()),
            instance_buffer: &self.instance_buffer,
            globals_buffer: &self.globals_buffer,
            transform_buffer: &self.transform_buffer,
            gamma_offset: self.gamma_offset,
            gamma_size: size_of::<GammaParams>() as u64,
            format: self.format,
            path_sample_count: self.rendering_params.path_sample_count,
            premultiplied_alpha: false,
        })
    }
}

fn create_instance_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instance_buffer"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The pixels of `texture` as RGBA bytes, row by row.
pub(super) fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
) -> Vec<u8> {
    let (width, height) = (texture.width(), texture.height());
    let row = width * 4;
    let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("harness_readback"),
        size: (padded_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("harness_readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("readback mapped")
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device polled");
    let mapped = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((row * height) as usize);
    for y in 0..height {
        let start = (y * padded_row) as usize;
        pixels.extend_from_slice(&mapped[start..start + row as usize]);
    }
    drop(mapped);
    buffer.unmap();
    if format == wgpu::TextureFormat::Bgra8Unorm {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
    pixels
}

mod transforms {
    use super::*;
    use gpui::{Corners, Radians, TransformationMatrix};

    const SIZE: usize = 200;

    fn rotation_about(degrees: f32, cx: f32, cy: f32) -> TransformationMatrix {
        TransformationMatrix::unit()
            .translate(point(ScaledPixels(cx), ScaledPixels(cy)))
            .rotate(Radians(degrees.to_radians()))
            .translate(point(ScaledPixels(-cx), ScaledPixels(-cy)))
    }

    fn red_quad(
        scene: &mut Scene,
        matrix: TransformationMatrix,
        bounds: Bounds<ScaledPixels>,
        corner: f32,
        mask: Option<Bounds<ScaledPixels>>,
    ) {
        let previous = scene.push_transform(matrix);
        let mut quad = quad(bounds, Hsla::from(rgba(0xff0000ff)));
        quad.corner_radii = Corners::all(ScaledPixels(corner));
        if let Some(mask) = mask {
            quad.content_mask = ContentMask::new(mask);
        }
        scene.insert_primitive(quad);
        scene.pop_transform(previous);
    }

    fn render(harness: &mut Harness, mut scene: Scene) -> Vec<u8> {
        scene.finish();
        harness.render(
            &scene,
            device_size(SIZE as i32, SIZE as i32),
            rgba(0xffffffff),
        )
    }

    fn is_red(pixels: &[u8], x: usize, y: usize) -> bool {
        let [r, g, b, a] = pixel(pixels, SIZE, x, y);
        r > 200 && g < 60 && b < 60 && a > 200
    }

    #[test]
    fn a_rotated_quad_covers_the_rotated_rectangle() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        red_quad(
            &mut scene,
            rotation_about(45., 100., 100.),
            sp(50., 50., 100., 100.),
            0.,
            None,
        );
        let pixels = render(&mut harness, scene);
        assert!(is_red(&pixels, 100, 100));
        assert!(is_red(&pixels, 100, 45));
        assert!(is_red(&pixels, 155, 100));
        assert!(!is_red(&pixels, 54, 54));
        assert!(!is_red(&pixels, 146, 146));
    }

    #[test]
    fn a_rotated_quad_keeps_its_rounded_corners_exact() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        red_quad(
            &mut scene,
            rotation_about(90., 100., 100.),
            sp(50., 50., 100., 100.),
            30.,
            None,
        );
        let pixels = render(&mut harness, scene);
        assert!(is_red(&pixels, 100, 100));
        assert!(!is_red(&pixels, 53, 53));
        assert!(is_red(&pixels, 70, 70));
    }

    #[test]
    fn a_transformed_clip_is_the_rotated_rectangle() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        red_quad(
            &mut scene,
            rotation_about(90., 100., 100.),
            sp(50., 50., 100., 100.),
            0.,
            Some(sp(50., 50., 100., 40.)),
        );
        let pixels = render(&mut harness, scene);
        assert!(is_red(&pixels, 140, 100));
        assert!(!is_red(&pixels, 60, 100));
        assert!(!is_red(&pixels, 100, 60));
    }

    #[test]
    fn a_translated_and_mirrored_quad_lands_where_the_matrix_puts_it() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let matrix = TransformationMatrix::unit()
            .translate(point(ScaledPixels(20.), ScaledPixels(10.)))
            .scale(size(-1., 1.));
        red_quad(
            &mut scene,
            matrix,
            sp(-140., 50., 40., 40.),
            0.,
            Some(sp(-200., 0., 200., 200.)),
        );
        let pixels = render(&mut harness, scene);
        assert!(is_red(&pixels, 140, 80));
        assert!(!is_red(&pixels, 60, 80));
    }

    fn is_painted(pixels: &[u8], x: usize, y: usize) -> bool {
        pixel(pixels, SIZE, x, y) != [255, 255, 255, 255]
    }

    #[test]
    fn a_rotated_image_covers_the_rotated_rectangle() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let previous = scene.push_transform(rotation_about(45., 100., 100.));
        scene.insert_primitive(PolychromeSprite {
            order: 0,
            transform_index: 0,
            grayscale: false.into(),
            opacity: 1.,
            bounds: sp(50., 50., 100., 100.),
            content_mask: no_mask(),
            corner_radii: Corners::default(),
            tile: image_tile(&harness),
        });
        scene.pop_transform(previous);
        let pixels = render(&mut harness, scene);
        assert!(is_painted(&pixels, 100, 100));
        assert!(is_painted(&pixels, 100, 45));
        assert!(is_painted(&pixels, 155, 100));
        assert!(!is_painted(&pixels, 54, 54));
        assert!(!is_painted(&pixels, 146, 146));
    }

    fn painted_near(pixels: &[u8], x: usize, y: usize) -> bool {
        (y - 3..=y + 3).any(|y| (x - 3..=x + 3).any(|x| is_painted(pixels, x, y)))
    }

    #[test]
    fn a_rotated_glyph_sprite_covers_the_rotated_rectangle() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let previous = scene.push_transform(rotation_about(45., 100., 100.));
        scene.insert_primitive(MonochromeSprite {
            order: 0,
            transform_index: 0,
            bounds: sp(50., 50., 100., 100.),
            content_mask: no_mask(),
            color: Hsla::from(rgba(0xff0000ff)),
            tile: glyph_tile(&harness, 1, false),
        });
        scene.pop_transform(previous);
        let pixels = render(&mut harness, scene);
        assert!(painted_near(&pixels, 100, 100));
        assert!(painted_near(&pixels, 100, 45));
        assert!(painted_near(&pixels, 155, 100));
        assert!(!painted_near(&pixels, 54, 54));
        assert!(!painted_near(&pixels, 146, 146));
    }

    #[test]
    fn a_rotated_shadow_covers_the_rotated_rectangle() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let previous = scene.push_transform(rotation_about(45., 100., 100.));
        scene.insert_primitive(Shadow {
            order: 0,
            blur_radius: ScaledPixels(0.),
            bounds: sp(50., 50., 100., 100.),
            corner_radii: Corners::default(),
            content_mask: no_mask(),
            color: Hsla::from(rgba(0xff0000ff)),
            element_bounds: sp(50., 50., 100., 100.),
            element_corner_radii: Corners::default(),
            inset: 0,
            transform_index: 0,
        });
        scene.pop_transform(previous);
        let pixels = render(&mut harness, scene);
        assert!(is_red(&pixels, 100, 100));
        assert!(is_red(&pixels, 100, 45));
        assert!(!is_red(&pixels, 54, 54));
    }

    #[test]
    fn a_dithered_shadow_stays_zero_outside_its_blur() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        scene.insert_primitive(Shadow {
            order: 0,
            blur_radius: ScaledPixels(10.),
            bounds: sp(80., 80., 40., 40.),
            corner_radii: Corners::default(),
            content_mask: no_mask(),
            color: Hsla::from(rgba(0xff0000ff)),
            element_bounds: sp(80., 80., 40., 40.),
            element_corner_radii: Corners::default(),
            inset: 0,
            transform_index: 0,
        });
        let pixels = render(&mut harness, scene);
        for (x, y) in [(2, 2), (197, 3), (3, 197), (100, 20)] {
            assert_eq!(
                pixel(&pixels, 200, x, y),
                [255, 255, 255, 255],
                "({x}, {y})"
            );
        }
        assert!(is_red(&pixels, 100, 100));
    }

    fn grayscale() -> gpui::ColorFilter {
        let row = [0.2126, 0.7152, 0.0722, 0., 0.];
        gpui::ColorFilter {
            rows: [row, row, row, [0., 0., 0., 1., 0.]],
        }
    }

    #[test]
    fn a_color_filter_recolours_what_is_drawn_under_it() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let previous = scene.push_color_filter(grayscale());
        red_quad(
            &mut scene,
            TransformationMatrix::unit(),
            sp(50., 50., 100., 100.),
            0.,
            None,
        );
        scene.pop_transform(previous);
        red_quad(
            &mut scene,
            TransformationMatrix::unit(),
            sp(0., 0., 20., 20.),
            0.,
            None,
        );
        let pixels = render(&mut harness, scene);
        let [r, g, b, a] = pixel(&pixels, SIZE, 100, 100);
        assert!(
            r.abs_diff(54) < 6 && g.abs_diff(54) < 6 && b.abs_diff(54) < 6,
            "{r} {g} {b}"
        );
        assert_eq!(a, 255);
        assert!(is_red(&pixels, 5, 5));
    }

    #[test]
    fn a_color_filter_composes_with_a_transform() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let previous = scene.push_color_filter(grayscale());
        red_quad(
            &mut scene,
            rotation_about(45., 100., 100.),
            sp(50., 50., 100., 100.),
            0.,
            None,
        );
        scene.pop_transform(previous);
        let pixels = render(&mut harness, scene);
        let [r, g, b, _] = pixel(&pixels, SIZE, 100, 45);
        assert!(
            r.abs_diff(g) < 3 && g.abs_diff(b) < 3 && r > 40,
            "{r} {g} {b}"
        );
        assert!(!is_red(&pixels, 100, 100));
    }
}

mod groups {
    use super::*;

    const SIZE: usize = 200;

    fn render(harness: &mut Harness, mut scene: Scene) -> Vec<u8> {
        scene.finish();
        harness.render(
            &scene,
            device_size(SIZE as i32, SIZE as i32),
            rgba(0xffffffff),
        )
    }

    fn near(actual: [u8; 4], expected: [u8; 3]) -> bool {
        actual[..3]
            .iter()
            .zip(expected)
            .all(|(a, e)| a.abs_diff(e) <= 4)
    }

    #[test]
    fn two_overlapping_children_fade_as_one_picture() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let scope = scene.begin_effect_group(0.5);
        scene.insert_primitive(quad(sp(20., 20., 100., 100.), Hsla::from(rgba(0xff0000ff))));
        scene.insert_primitive(quad(sp(60., 60., 100., 100.), Hsla::from(rgba(0x0000ffff))));
        scene.end_effect_group(scope, sp(0., 0., 200., 200.));
        let pixels = render(&mut harness, scene);
        // Half red on white; half blue on white where the children overlap,
        // with no red in it.
        assert!(near(pixel(&pixels, SIZE, 30, 30), [255, 128, 128]));
        assert!(near(pixel(&pixels, SIZE, 90, 90), [128, 128, 255]));
        assert!(near(pixel(&pixels, SIZE, 150, 150), [128, 128, 255]));
        assert!(near(pixel(&pixels, SIZE, 10, 10), [255, 255, 255]));
    }

    #[test]
    fn a_group_in_a_group_fades_twice() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let outer = scene.begin_effect_group(0.5);
        let inner = scene.begin_effect_group(0.5);
        scene.insert_primitive(quad(sp(50., 50., 100., 100.), Hsla::from(rgba(0x000000ff))));
        scene.end_effect_group(inner, sp(0., 0., 200., 200.));
        scene.end_effect_group(outer, sp(0., 0., 200., 200.));
        let pixels = render(&mut harness, scene);
        assert!(near(pixel(&pixels, SIZE, 100, 100), [191, 191, 191]));
    }

    #[test]
    fn a_path_in_a_group_is_faded_with_it() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let scope = scene.begin_effect_group(0.5);
        let mut path = Path::new(point(px(100.), px(100.)));
        path.line_to(point(px(180.), px(100.)));
        path.line_to(point(px(100.), px(180.)));
        path.line_to(point(px(100.), px(100.)));
        path.content_mask = ContentMask::new(Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(200.), px(200.)),
        });
        path.color = Hsla::from(rgba(0x00ff00ff)).into();
        scene.insert_primitive(path.scale(1.));
        scene.insert_primitive(quad(sp(90., 90., 30., 30.), Hsla::from(rgba(0xff0000ff))));
        scene.end_effect_group(scope, sp(0., 0., 200., 200.));
        let pixels = render(&mut harness, scene);
        assert!(near(pixel(&pixels, SIZE, 110, 150), [128, 255, 128]));
        assert!(near(pixel(&pixels, SIZE, 100, 100), [255, 128, 128]));
        assert!(near(pixel(&pixels, SIZE, 170, 170), [255, 255, 255]));
    }

    #[test]
    fn a_scroll_layer_in_a_group_is_faded_with_it() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let key = LayerKey(9);
        let tile = coord(0, 0);
        let mut content = Scene::default();
        content.insert_primitive(quad(
            sp(0., 0., TILE as f32, TILE as f32),
            Hsla::from(rgba(0xff0000ff)),
        ));
        content.finish();
        let layer = layer_frame(key, 1, content, &[tile]);
        let viewport = sp(20., 20., 100., 100.);
        let mut scene = Scene::default();
        let scope = scene.begin_effect_group(0.5);
        scene.push_layer(viewport);
        scene.insert_primitive(PolychromeSprite {
            order: 0,
            transform_index: 0,
            grayscale: false.into(),
            opacity: 1.,
            bounds: sp(0., 0., TILE as f32, TILE as f32),
            content_mask: ContentMask::new(viewport),
            corner_radii: Corners::default(),
            tile: AtlasTile {
                texture_id: layer_tile_texture_id(key),
                tile_id: layer_tile_id(tile),
                padding: 0,
                bounds: Bounds {
                    origin: point(DevicePixels(0), DevicePixels(0)),
                    size: device_size(TILE as i32, TILE as i32),
                },
            },
        });
        scene.pop_layer();
        scene.end_effect_group(scope, sp(0., 0., 200., 200.));
        scene.layers.frames.push(layer);
        let pixels = render(&mut harness, scene);
        assert!(near(pixel(&pixels, SIZE, 60, 60), [255, 128, 128]));
        assert!(near(pixel(&pixels, SIZE, 10, 10), [255, 255, 255]));
    }

    #[test]
    fn a_rotated_quad_in_a_group_is_where_it_would_be_without_one() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let draw = |harness: &mut Harness, grouped: bool| {
            let mut scene = Scene::default();
            let scope = grouped.then(|| scene.begin_effect_group(0.999));
            let previous = scene.push_transform(
                TransformationMatrix::unit()
                    .translate(point(ScaledPixels(100.), ScaledPixels(100.)))
                    .rotate(gpui::Radians(45f32.to_radians()))
                    .translate(point(ScaledPixels(-100.), ScaledPixels(-100.))),
            );
            scene.insert_primitive(quad(sp(70., 70., 60., 60.), Hsla::from(rgba(0xff0000ff))));
            scene.pop_transform(previous);
            if let Some(scope) = scope {
                scene.end_effect_group(scope, sp(0., 0., 200., 200.));
            }
            render(harness, scene)
        };
        let (plain, grouped) = (draw(&mut harness, false), draw(&mut harness, true));
        for (x, y) in [(100, 100), (100, 62), (140, 100), (70, 70), (100, 150)] {
            let (a, b) = (pixel(&plain, SIZE, x, y), pixel(&grouped, SIZE, x, y));
            assert!(
                a.iter().zip(&b).all(|(a, b)| a.abs_diff(*b) <= 3),
                "{x},{y} {a:?} {b:?}"
            );
        }
    }

    #[test]
    fn a_blurred_group_spreads_its_edges() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let scope = scene.begin_effect_group_with(1.0, gpui::GroupEffect::Blur { sigma: 4.0 });
        scene.insert_primitive(quad(sp(80., 80., 40., 40.), Hsla::from(rgba(0x000000ff))));
        scene.end_effect_group(scope, sp(0., 0., 200., 200.));
        let pixels = render(&mut harness, scene);
        let at = |x, y| pixel(&pixels, SIZE, x, y)[0];
        assert!(at(100, 100) < 5, "{}", at(100, 100));
        assert!(at(80, 100).abs_diff(128) < 24, "edge {}", at(80, 100));
        assert!(
            at(76, 100) > 165 && at(76, 100) < 245,
            "outside {}",
            at(76, 100)
        );
        assert!(at(60, 100) > 251, "far {}", at(60, 100));
        assert!(at(76, 100).abs_diff(at(123, 100)) < 12);
    }

    #[test]
    fn a_drop_shadow_is_drawn_beneath_the_group_offset() {
        let Some(mut harness) = Harness::new() else {
            eprintln!("skipped: no wgpu adapter");
            return;
        };
        let mut scene = Scene::default();
        let scope = scene.begin_effect_group_with(
            1.0,
            gpui::GroupEffect::DropShadow {
                dx: 12.0,
                dy: 12.0,
                sigma: 2.0,
                color: [0.0, 0.0, 1.0, 1.0],
            },
        );
        scene.insert_primitive(quad(sp(40., 40., 60., 60.), Hsla::from(rgba(0xff0000ff))));
        scene.end_effect_group(scope, sp(0., 0., 200., 200.));
        let pixels = render(&mut harness, scene);
        assert!(near(pixel(&pixels, SIZE, 70, 70), [255, 0, 0]));
        assert!(near(pixel(&pixels, SIZE, 105, 105), [0, 0, 255]));
        assert!(near(pixel(&pixels, SIZE, 30, 30), [255, 255, 255]));
    }
}
