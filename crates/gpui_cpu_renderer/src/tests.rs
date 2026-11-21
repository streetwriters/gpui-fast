//! Per-primitive tests of the CPU renderer: each builds a small `Scene` by hand,
//! renders it and checks the colour at a few sample points. The window is
//! cleared to opaque black, so a pixel that stays `[0, 0, 0, 255]` was not
//! drawn on.

use std::borrow::Cow;

use gpui::{
    AtlasKey, AtlasTile, BorderStyle, Bounds, ClipMask, ColorFilter, Corners, DevicePixels, Edges,
    Hsla, ImageId, MonochromeSprite, PaddedBool32, Path, PlatformHeadlessRenderer,
    PolychromeSprite, Quad, Radians, RenderImageParams, RenderSvgParams, ScaledPixels, Scene,
    Shadow, TransformationMatrix, Underline, linear_color_stop, linear_gradient, point, px, rgb,
    rgba, size, solid_background,
};

use crate::CpuHeadlessRenderer;

const BLACK: [u8; 4] = [0, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn sp(value: f32) -> ScaledPixels {
    ScaledPixels(value)
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(sp(x), sp(y)),
        size: size(sp(width), sp(height)),
    }
}

/// A mask that clips nothing.
fn open() -> ClipMask<ScaledPixels> {
    ClipMask::new(rect(-1000., -1000., 2000., 2000.))
}

fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn quad(bounds: Bounds<ScaledPixels>, hex: u32) -> Quad {
    Quad {
        bounds,
        content_mask: open(),
        background: solid_background(color(hex)),
        ..Default::default()
    }
}

struct Frame {
    renderer: CpuHeadlessRenderer,
    scene: Scene,
}

impl Frame {
    fn new() -> Self {
        Self {
            renderer: CpuHeadlessRenderer::new(),
            scene: Scene::default(),
        }
    }

    fn add(&mut self, primitive: impl Into<gpui::Primitive>) -> &mut Self {
        self.scene.insert_primitive(primitive);
        self
    }

    fn render(&mut self, width: i32, height: i32) -> Image {
        self.scene.finish();
        let image = self
            .renderer
            .render_scene_to_image(
                &self.scene,
                size(DevicePixels(width), DevicePixels(height)),
            )
            .expect("the scene renders");
        Image(image)
    }

    /// Puts pixels in the renderer's atlas, as the text system does.
    fn upload(&self, key: AtlasKey, width: i32, height: i32, bytes: &[u8]) -> AtlasTile {
        self.renderer
            .sprite_atlas()
            .get_or_insert_with(key, &mut || {
                Ok(Some((
                    size(DevicePixels(width), DevicePixels(height)),
                    Cow::Borrowed(bytes),
                )))
            })
            .expect("the atlas takes the tile")
            .expect("a tile came back")
    }
}

struct Image(image::RgbaImage);

impl Image {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        self.0.get_pixel(x, y).0
    }

    #[track_caller]
    fn expect(&self, x: u32, y: u32, expected: [u8; 4], tolerance: u8) {
        let actual = self.at(x, y);
        let close = actual
            .iter()
            .zip(expected)
            .all(|(a, e)| a.abs_diff(e) <= tolerance);
        assert!(
            close,
            "pixel ({x}, {y}) is {actual:?}, expected {expected:?} (±{tolerance})"
        );
    }
}

fn svg_key(name: &str) -> AtlasKey {
    AtlasKey::Svg(RenderSvgParams {
        path: name.to_string().into(),
        size: size(DevicePixels(4), DevicePixels(4)),
    })
}

fn image_key(id: usize) -> AtlasKey {
    AtlasKey::Image(RenderImageParams {
        image_id: ImageId(id),
        frame_index: 0,
    })
}

#[test]
fn the_window_starts_opaque_black() {
    let image = Frame::new().render(8, 6);
    assert_eq!((image.0.width(), image.0.height()), (8, 6));
    image.expect(0, 0, BLACK, 0);
    image.expect(7, 5, BLACK, 0);
}

#[test]
fn an_empty_window_cannot_be_rendered() {
    let mut frame = Frame::new();
    frame.scene.finish();
    for (width, height) in [(0, 10), (10, 0), (-4, 10)] {
        let result = frame
            .renderer
            .render_scene_to_image(&frame.scene, size(DevicePixels(width), DevicePixels(height)));
        assert!(result.is_err(), "{width}x{height} should not render");
    }
}

#[test]
fn a_solid_quad_fills_its_bounds_and_nothing_else() {
    let mut frame = Frame::new();
    frame.add(quad(rect(10., 5., 20., 10.), 0xff0000));
    let image = frame.render(60, 30);
    image.expect(10, 5, RED, 1);
    image.expect(29, 14, RED, 1);
    image.expect(9, 5, BLACK, 0);
    image.expect(10, 4, BLACK, 0);
    image.expect(30, 14, BLACK, 0);
    image.expect(29, 15, BLACK, 0);
}

#[test]
fn a_translucent_quad_blends_over_what_is_below() {
    let mut frame = Frame::new();
    frame.add(quad(rect(0., 0., 20., 20.), 0xffffff));
    let mut top = quad(rect(5., 5., 10., 10.), 0);
    top.background = solid_background(Hsla::from(rgba(0xff000080)));
    frame.add(top);
    let image = frame.render(20, 20);
    image.expect(2, 2, WHITE, 0);
    // Half red over white.
    image.expect(10, 10, [255, 127, 127, 255], 2);
}

#[test]
fn quads_are_drawn_in_order() {
    let mut frame = Frame::new();
    frame.add(quad(rect(0., 0., 20., 20.), 0xff0000));
    frame.add(quad(rect(10., 10., 20., 20.), 0x00ff00));
    let image = frame.render(30, 30);
    image.expect(5, 5, RED, 1);
    image.expect(15, 15, GREEN, 1);
    image.expect(25, 25, GREEN, 1);
    image.expect(25, 5, BLACK, 0);
}

#[test]
fn corner_radii_cut_the_corners() {
    let mut frame = Frame::new();
    let mut rounded = quad(rect(10., 10., 40., 40.), 0xff0000);
    rounded.corner_radii = Corners::all(sp(20.));
    frame.add(rounded);
    let image = frame.render(60, 60);
    image.expect(30, 30, RED, 1);
    // Half a pixel inside the arc, so almost fully covered.
    image.expect(30, 10, RED, 4);
    image.expect(10, 30, RED, 4);
    // The corner pixels are outside the arc.
    image.expect(10, 10, BLACK, 0);
    image.expect(49, 49, BLACK, 0);
    // On the arc at 45 degrees: (20 - 20 / sqrt 2) = 5.86 from each side.
    image.expect(16, 16, RED, 40);
    image.expect(12, 12, BLACK, 0);
}

#[test]
fn a_linear_gradient_runs_from_one_stop_to_the_other() {
    let mut frame = Frame::new();
    let mut gradient = quad(rect(0., 0., 100., 10.), 0);
    gradient.background = linear_gradient(
        90.,
        linear_color_stop(color(0xff0000), 0.),
        linear_color_stop(color(0x0000ff), 1.),
    );
    frame.add(gradient);
    let image = frame.render(100, 10);
    image.expect(0, 5, [252, 0, 3, 255], 6);
    image.expect(99, 5, [3, 0, 252, 255], 6);
    let middle = image.at(50, 5);
    assert!(
        middle[0].abs_diff(128) < 12 && middle[2].abs_diff(128) < 12 && middle[1] < 8,
        "the middle of a red to blue gradient is purple, got {middle:?}"
    );
}

#[test]
fn borders_have_a_width_per_side() {
    let mut frame = Frame::new();
    let mut bordered = quad(rect(10., 10., 60., 40.), 0xffffff);
    bordered.border_color = color(0xff0000);
    bordered.border_widths = Edges {
        top: sp(2.),
        right: sp(0.),
        bottom: sp(8.),
        left: sp(4.),
    };
    frame.add(bordered);
    let image = frame.render(80, 60);
    // Inside each border, then just past it.
    image.expect(11, 30, RED, 2);
    image.expect(13, 30, RED, 2);
    image.expect(14, 30, WHITE, 2);
    image.expect(40, 10, RED, 2);
    image.expect(40, 11, RED, 2);
    image.expect(40, 12, WHITE, 2);
    image.expect(40, 49, RED, 2);
    image.expect(40, 42, RED, 2);
    image.expect(40, 41, WHITE, 2);
    // No border on the right.
    image.expect(69, 30, WHITE, 2);
    image.expect(68, 30, WHITE, 2);
}

#[test]
fn a_dashed_border_has_gaps() {
    let mut frame = Frame::new();
    let mut dashed = quad(rect(10., 10., 100., 40.), 0);
    dashed.background = solid_background(Hsla::transparent_black());
    dashed.border_color = color(0xff0000);
    dashed.border_widths = Edges::all(sp(2.));
    dashed.border_style_and_transform =
        Quad::pack_border_style(BorderStyle::Dashed, gpui::TransformIndex::IDENTITY);
    frame.add(dashed);
    let image = frame.render(120, 60);
    let row: Vec<[u8; 4]> = (30..90).map(|x| image.at(x, 11)).collect();
    let drawn = row.iter().filter(|pixel| pixel[0] > 200).count();
    let gaps = row.iter().filter(|pixel| pixel[0] < 40).count();
    assert!(drawn >= 8, "a dashed top edge has dashes, got {row:?}");
    assert!(gaps >= 8, "a dashed top edge has gaps, got {row:?}");
    // A solid border of the same size has none.
    let mut frame = Frame::new();
    let mut solid = quad(rect(10., 10., 100., 40.), 0);
    solid.background = solid_background(Hsla::transparent_black());
    solid.border_color = color(0xff0000);
    solid.border_widths = Edges::all(sp(2.));
    frame.add(solid);
    let image = frame.render(120, 60);
    assert!((30..90).all(|x| image.at(x, 11)[0] > 200));
}

#[test]
fn a_content_mask_clips_to_its_rectangle() {
    let mut frame = Frame::new();
    let mut clipped = quad(rect(0., 0., 40., 20.), 0xff0000);
    clipped.content_mask = ClipMask::new(rect(10., 5., 20., 10.));
    frame.add(clipped);
    let image = frame.render(40, 20);
    image.expect(10, 5, RED, 1);
    image.expect(29, 14, RED, 1);
    image.expect(9, 10, BLACK, 0);
    image.expect(30, 10, BLACK, 0);
    image.expect(20, 4, BLACK, 0);
    image.expect(20, 15, BLACK, 0);
}

#[test]
fn a_rounded_content_mask_rounds_what_it_clips() {
    let mut frame = Frame::new();
    let mut clipped = quad(rect(0., 0., 40., 40.), 0xff0000);
    clipped.content_mask = ClipMask {
        bounds: rect(0., 0., 40., 40.),
        rounded_bounds: rect(0., 0., 40., 40.),
        corner_radii: Corners::all(sp(20.)),
    };
    frame.add(clipped);
    let image = frame.render(40, 40);
    image.expect(20, 20, RED, 1);
    image.expect(0, 0, BLACK, 0);
    image.expect(39, 0, BLACK, 0);
}

#[test]
fn a_shadow_fades_out_from_the_element() {
    let mut frame = Frame::new();
    frame.add(quad(rect(0., 0., 100., 100.), 0xffffff));
    frame.add(Shadow {
        order: 0,
        blur_radius: sp(8.),
        bounds: rect(30., 30., 40., 40.),
        corner_radii: Corners::default(),
        content_mask: open(),
        color: Hsla::black(),
        element_bounds: rect(30., 30., 40., 40.),
        element_corner_radii: Corners::default(),
        inset: 0,
        transform_index: 0,
    });
    let image = frame.render(100, 100);
    // Fully shadowed in the middle, half at the edge, and gone past three blur
    // radii.
    assert!(image.at(50, 50)[0] < 25, "{:?}", image.at(50, 50));
    let edge = image.at(30, 50)[0];
    assert!((90..170).contains(&edge), "the edge is half shadowed, got {edge}");
    let near = image.at(24, 50)[0];
    let far = image.at(10, 50)[0];
    assert!(edge < near && near < far, "{edge} < {near} < {far}");
    image.expect(2, 50, WHITE, 0);
    image.expect(50, 2, WHITE, 0);
}

#[test]
fn an_inset_shadow_darkens_the_inside_of_the_edges() {
    let mut frame = Frame::new();
    frame.add(quad(rect(0., 0., 80., 80.), 0xffffff));
    frame.add(Shadow {
        order: 0,
        blur_radius: sp(4.),
        bounds: rect(10., 10., 60., 60.),
        corner_radii: Corners::default(),
        content_mask: open(),
        color: Hsla::black(),
        element_bounds: rect(10., 10., 60., 60.),
        element_corner_radii: Corners::default(),
        inset: 1,
        transform_index: 0,
    });
    let image = frame.render(80, 80);
    let edge = image.at(11, 40)[0];
    assert!(edge < 200, "inside the edge is shadowed, got {edge}");
    image.expect(40, 40, WHITE, 3);
    // Outside the element there is no shadow.
    image.expect(5, 40, WHITE, 0);
}

#[test]
fn a_shadow_is_clipped_by_its_content_mask() {
    let mut frame = Frame::new();
    frame.add(quad(rect(0., 0., 100., 100.), 0xffffff));
    frame.add(Shadow {
        order: 0,
        blur_radius: sp(8.),
        bounds: rect(30., 30., 40., 40.),
        corner_radii: Corners::default(),
        content_mask: ClipMask::new(rect(0., 0., 50., 100.)),
        color: Hsla::black(),
        element_bounds: rect(30., 30., 40., 40.),
        element_corner_radii: Corners::default(),
        inset: 0,
        transform_index: 0,
    });
    let image = frame.render(100, 100);
    assert!(image.at(45, 50)[0] < 25);
    image.expect(55, 50, WHITE, 0);
}

fn underline(bounds: Bounds<ScaledPixels>, wavy: bool) -> Underline {
    Underline {
        order: 0,
        transform_index: 0,
        bounds,
        content_mask: open(),
        color: color(0xff0000),
        thickness: sp(2.),
        wavy: PaddedBool32::from(wavy),
    }
}

#[test]
fn a_straight_underline_is_a_line() {
    let mut frame = Frame::new();
    frame.add(underline(rect(10., 10., 30., 4.), false));
    let image = frame.render(50, 30);
    let lit = |x: u32| (0..30).filter(|&y| image.at(x, y)[0] > 128).count();
    // A straight underline fills its bounds; only a wavy one uses its thickness.
    assert_eq!(lit(25), 4, "the bounds are 4 px tall");
    assert_eq!(lit(5), 0);
    assert_eq!(lit(45), 0);
}

#[test]
fn a_wavy_underline_moves_up_and_down() {
    let mut frame = Frame::new();
    frame.add(underline(rect(10., 10., 60., 8.), true));
    let image = frame.render(80, 30);
    let centre_of = |x: u32| -> Option<f32> {
        let (mut total, mut weight) = (0., 0.);
        for y in 0..30 {
            let amount = image.at(x, y)[0] as f32 / 255.;
            total += amount * y as f32;
            weight += amount;
        }
        (weight > 0.5).then(|| total / weight)
    };
    let centres: Vec<f32> = (12..68).filter_map(centre_of).collect();
    assert!(centres.len() > 40, "the wave is drawn along its width");
    let high = centres.iter().cloned().fold(f32::MAX, f32::min);
    let low = centres.iter().cloned().fold(f32::MIN, f32::max);
    assert!(low - high > 2., "the line moves by {} px", low - high);
    assert_eq!(image.at(5, 14), BLACK);
}

#[test]
fn a_monochrome_sprite_is_the_tile_tinted_with_the_sprite_colour() {
    let mut frame = Frame::new();
    let tile = frame.upload(svg_key("full"), 4, 4, &[255; 16]);
    let half = frame.upload(svg_key("half"), 4, 4, &[128; 16]);
    for (tile, x) in [(tile, 5.), (half, 25.)] {
        frame.add(MonochromeSprite {
            order: 0,
            transform_index: 0,
            tile,
            bounds: rect(x, 5., 8., 8.),
            content_mask: open(),
            color: color(0x00ff00),
        });
    }
    let image = frame.render(40, 20);
    image.expect(5, 5, GREEN, 1);
    image.expect(12, 12, GREEN, 1);
    image.expect(4, 8, BLACK, 0);
    image.expect(13, 8, BLACK, 0);
    // Half coverage of green over black.
    image.expect(28, 8, [0, 128, 0, 255], 2);
}

#[test]
fn a_monochrome_sprite_follows_the_shape_of_its_tile() {
    let mut frame = Frame::new();
    // A 2x2 tile with only the top left texel covered.
    let tile = frame.upload(svg_key("corner"), 2, 2, &[255, 0, 0, 0]);
    frame.add(MonochromeSprite {
        order: 0,
        transform_index: 0,
        tile,
        bounds: rect(0., 0., 20., 20.),
        content_mask: open(),
        color: color(0x0000ff),
    });
    let image = frame.render(20, 20);
    image.expect(1, 1, BLUE, 1);
    image.expect(18, 18, BLACK, 1);
    image.expect(18, 1, BLACK, 1);
}

#[test]
fn a_sprite_is_clipped_by_its_content_mask() {
    let mut frame = Frame::new();
    let tile = frame.upload(svg_key("clipped"), 4, 4, &[255; 16]);
    frame.add(MonochromeSprite {
        order: 0,
        transform_index: 0,
        tile,
        bounds: rect(0., 0., 20., 20.),
        content_mask: ClipMask::new(rect(0., 0., 10., 20.)),
        color: color(0x00ff00),
    });
    let image = frame.render(20, 20);
    image.expect(5, 5, GREEN, 1);
    image.expect(15, 5, BLACK, 0);
}

fn red_bgra(side: usize) -> Vec<u8> {
    [0, 0, 255, 255].repeat(side * side)
}

fn polychrome(tile: AtlasTile, bounds: Bounds<ScaledPixels>) -> PolychromeSprite {
    PolychromeSprite {
        order: 0,
        transform_index: 0,
        tile,
        grayscale: PaddedBool32::from(false),
        opacity: 1.,
        bounds,
        content_mask: open(),
        corner_radii: Corners::default(),
    }
}

#[test]
fn a_polychrome_sprite_shows_the_colours_of_its_tile() {
    let mut frame = Frame::new();
    let tile = frame.upload(image_key(1), 2, 2, &red_bgra(2));
    frame.add(polychrome(tile, rect(5., 5., 10., 10.)));
    let image = frame.render(20, 20);
    image.expect(10, 10, RED, 1);
    image.expect(4, 10, BLACK, 0);
}

#[test]
fn a_polychrome_sprite_can_be_grayscale_and_translucent() {
    let mut frame = Frame::new();
    let tile = frame.upload(image_key(2), 2, 2, &red_bgra(2));
    let mut gray = polychrome(tile, rect(0., 0., 10., 10.));
    gray.grayscale = PaddedBool32::from(true);
    frame.add(gray);
    let mut faded = polychrome(tile, rect(10., 0., 10., 10.));
    faded.opacity = 0.5;
    frame.add(faded);
    let image = frame.render(20, 10);
    // 0.2126 of 255.
    image.expect(5, 5, [54, 54, 54, 255], 2);
    image.expect(15, 5, [128, 0, 0, 255], 2);
}

#[test]
fn a_polychrome_sprite_honours_its_corner_radii() {
    let mut frame = Frame::new();
    let tile = frame.upload(image_key(3), 2, 2, &red_bgra(2));
    let mut rounded = polychrome(tile, rect(10., 10., 20., 20.));
    rounded.corner_radii = Corners::all(sp(8.));
    frame.add(rounded);
    let image = frame.render(40, 40);
    image.expect(20, 20, RED, 1);
    image.expect(20, 10, RED, 1);
    image.expect(10, 10, BLACK, 0);
    image.expect(29, 29, BLACK, 0);
}

#[test]
fn a_straight_alpha_tile_is_blended_by_its_alpha() {
    let mut frame = Frame::new();
    // Half transparent blue, as BGRA.
    let tile = frame.upload(image_key(4), 2, 2, &[255, 0, 0, 128].repeat(4));
    frame.add(polychrome(tile, rect(0., 0., 10., 10.)));
    let image = frame.render(10, 10);
    image.expect(5, 5, [0, 0, 128, 255], 2);
}

fn triangle(corners: [(f32, f32); 3], hex: u32) -> Path<ScaledPixels> {
    let mut path = Path::new(point(px(corners[0].0), px(corners[0].1)));
    path.line_to(point(px(corners[1].0), px(corners[1].1)));
    path.line_to(point(px(corners[2].0), px(corners[2].1)));
    path.content_mask = ClipMask::new(Bounds {
        origin: point(px(-1000.), px(-1000.)),
        size: size(px(2000.), px(2000.)),
    });
    path.color = solid_background(color(hex));
    path.scale(1.)
}

#[test]
fn a_path_is_filled() {
    let mut frame = Frame::new();
    frame.add(triangle([(10., 10.), (50., 10.), (10., 50.)], 0xff0000));
    let image = frame.render(60, 60);
    image.expect(15, 15, RED, 1);
    image.expect(30, 15, RED, 1);
    image.expect(45, 45, BLACK, 0);
    image.expect(55, 15, BLACK, 0);
}

#[test]
fn a_curved_path_is_filled_up_to_its_curve() {
    let mut frame = Frame::new();
    let mut path = Path::new(point(px(10.), px(40.)));
    path.curve_to(point(px(50.), px(40.)), point(px(30.), px(0.)));
    path.content_mask = ClipMask::new(Bounds {
        origin: point(px(-1000.), px(-1000.)),
        size: size(px(2000.), px(2000.)),
    });
    path.color = solid_background(color(0xff0000));
    frame.add(path.scale(1.));
    let image = frame.render(60, 50);
    // Under the parabola, whose apex is at y = 20.
    image.expect(30, 30, RED, 1);
    image.expect(30, 38, RED, 1);
    image.expect(30, 15, BLACK, 0);
    image.expect(12, 20, BLACK, 0);
    // The edge is antialiased: somewhere along it a pixel is partly covered.
    let partial = (10..50)
        .flat_map(|x| (0..40).map(move |y| (x, y)))
        .any(|(x, y)| {
            let pixel = image.at(x, y);
            pixel[0] > 40 && pixel[0] < 215
        });
    assert!(partial, "the curve's edge is antialiased");
}

#[test]
fn touching_triangles_of_a_path_leave_no_seam() {
    let mut frame = Frame::new();
    let mut path = Path::new(point(px(10.), px(10.)));
    path.line_to(point(px(40.), px(10.)));
    path.line_to(point(px(40.), px(40.)));
    path.line_to(point(px(10.), px(40.)));
    path.content_mask = ClipMask::new(Bounds {
        origin: point(px(-1000.), px(-1000.)),
        size: size(px(2000.), px(2000.)),
    });
    // Translucent, so a pixel drawn twice would show.
    path.color = solid_background(Hsla::from(rgba(0xff000080)));
    frame.add(path.scale(1.));
    let image = frame.render(50, 50);
    let first = image.at(15, 15);
    for (x, y) in [(20, 20), (25, 25), (30, 30), (15, 35)] {
        assert_eq!(image.at(x, y), first, "pixel ({x}, {y}) was drawn twice");
    }
    assert!(first[0] > 100 && first[0] < 150, "half red, got {first:?}");
}

#[test]
fn a_path_is_clipped_by_its_content_mask() {
    let mut frame = Frame::new();
    let mut path = Path::new(point(px(10.), px(10.)));
    path.line_to(point(px(50.), px(10.)));
    path.line_to(point(px(10.), px(50.)));
    path.content_mask = ClipMask::new(Bounds {
        origin: point(px(0.), px(0.)),
        size: size(px(20.), px(60.)),
    });
    path.color = solid_background(color(0xff0000));
    frame.add(path.scale(1.));
    let image = frame.render(60, 60);
    image.expect(15, 15, RED, 1);
    image.expect(30, 15, BLACK, 0);
}

#[test]
fn a_transform_moves_what_is_drawn_under_it() {
    let mut frame = Frame::new();
    let previous = frame.scene.push_transform(
        TransformationMatrix::unit().translate(point(sp(20.), sp(30.))),
    );
    frame.add(quad(rect(0., 0., 10., 10.), 0xff0000));
    frame.scene.pop_transform(previous);
    let image = frame.render(50, 50);
    image.expect(25, 35, RED, 1);
    image.expect(5, 5, BLACK, 0);
}

#[test]
fn a_rotation_turns_what_is_drawn_under_it() {
    let mut frame = Frame::new();
    let previous = frame
        .scene
        .push_transform(TransformationMatrix::unit().rotate(Radians(std::f32::consts::FRAC_PI_2)));
    // Turned a quarter, (x, y) lands on (-y, x): x in 0..10 and y in 0..20.
    frame.add(quad(rect(0., -10., 20., 10.), 0xff0000));
    frame.scene.pop_transform(previous);
    let image = frame.render(30, 30);
    image.expect(5, 15, RED, 1);
    image.expect(15, 5, BLACK, 0);
}

#[test]
fn a_colour_filter_recolours_what_is_drawn_under_it() {
    let mut frame = Frame::new();
    let mut halve_red = ColorFilter::IDENTITY;
    halve_red.rows[0][0] = 0.5;
    let previous = frame.scene.push_color_filter(halve_red);
    frame.add(quad(rect(0., 0., 10., 10.), 0xff0000));
    frame.scene.pop_transform(previous);
    frame.add(quad(rect(10., 0., 10., 10.), 0xff0000));
    let image = frame.render(20, 10);
    image.expect(5, 5, [128, 0, 0, 255], 2);
    image.expect(15, 5, RED, 1);
}

#[test]
fn a_frame_does_not_leak_into_the_next() {
    let mut frame = Frame::new();
    frame.add(quad(rect(0., 0., 10., 10.), 0xff0000));
    frame.render(10, 10).expect(5, 5, RED, 1);
    let mut next = Scene::default();
    next.insert_primitive(quad(rect(0., 0., 10., 10.), 0x00ff00));
    next.finish();
    let image = frame
        .renderer
        .render_scene_to_image(&next, size(DevicePixels(10), DevicePixels(10)))
        .unwrap();
    assert_eq!(image.get_pixel(5, 5).0, GREEN);
}

#[test]
fn the_atlas_keeps_what_it_is_given() {
    let frame = Frame::new();
    let key = svg_key("kept");
    let tile = frame.upload(key.clone(), 4, 4, &[7; 16]);
    let again = frame.upload(key.clone(), 4, 4, &[9; 16]);
    assert_eq!(tile.texture_id, again.texture_id);
    // The pixels of the first upload are the ones kept, not the second's.
    let texture = frame.renderer.atlas.texture(tile.texture_id).expect("the texture is held");
    assert_eq!((texture.width, texture.height), (4, 4));
    for (x, y) in [(0, 0), (3, 0), (0, 3), (3, 3), (2, 1)] {
        assert_eq!(texture.texel(x, y), [0., 0., 0., 7. / 255.], "texel ({x}, {y})");
    }
    // A texel past the edge repeats the edge's.
    assert_eq!(texture.texel(9, -4), texture.texel(3, 0));
    // A colour tile keeps its channels, blue first.
    let colour = frame.upload(image_key(30), 2, 1, &[10, 20, 30, 255, 40, 50, 60, 128]);
    let texture = frame.renderer.atlas.texture(colour.texture_id).unwrap();
    assert_eq!(texture.texel(0, 0), [30. / 255., 20. / 255., 10. / 255., 1.]);
    assert_eq!(texture.texel(1, 0), [60. / 255., 50. / 255., 40. / 255., 128. / 255.]);
    // Between two texel centres it is the mean of both.
    let middle = texture.sample_linear(1., 0.5);
    assert!((middle[0] - 45. / 255.).abs() < 1e-4, "{middle:?}");

    assert!(frame.renderer.sprite_atlas().contains(&key));
    frame.renderer.sprite_atlas().remove(&key);
    assert!(!frame.renderer.sprite_atlas().contains(&key));
    assert!(frame.renderer.atlas.texture(tile.texture_id).is_none());
    // The other tile is still there.
    assert!(frame.renderer.atlas.texture(colour.texture_id).is_some());
}

#[test]
fn a_checkerboard_alternates_its_squares() {
    let mut frame = Frame::new();
    let mut board = quad(rect(0., 0., 40., 20.), 0);
    board.background = gpui::checkerboard(color(0xffffff), 10.);
    frame.add(board);
    let image = frame.render(40, 20);
    // The first square is left clear, the next is coloured, and so on.
    image.expect(5, 5, BLACK, 0);
    image.expect(15, 5, WHITE, 1);
    image.expect(25, 5, BLACK, 0);
    image.expect(35, 5, WHITE, 1);
    image.expect(5, 15, WHITE, 1);
    image.expect(15, 15, BLACK, 0);
}

#[test]
fn a_slash_pattern_draws_diagonal_stripes() {
    let mut frame = Frame::new();
    let mut stripes = quad(rect(0., 0., 40., 40.), 0);
    stripes.background = gpui::pattern_slash(color(0xffffff), 2., 6.);
    frame.add(stripes);
    let image = frame.render(40, 40);
    // A stripe runs along x + y = 8 k - 1, up and to the right; the pixels
    // between two stripes are clear.
    for (x, y) in [(7, 0), (3, 4), (0, 7), (15, 0), (10, 5), (20, 11)] {
        image.expect(x, y, WHITE, 2);
    }
    for (x, y) in [(3, 1), (2, 2), (10, 2), (20, 14), (1, 1)] {
        image.expect(x, y, BLACK, 0);
    }
}

#[test]
fn an_oklab_gradient_mixes_its_stops_differently_from_srgb() {
    let render = |space: gpui::ColorSpace| {
        let mut frame = Frame::new();
        let mut gradient = quad(rect(0., 0., 100., 10.), 0);
        gradient.background = linear_gradient(
            90.,
            linear_color_stop(color(0xff0000), 0.),
            linear_color_stop(color(0x0000ff), 1.),
        )
        .color_space(space);
        frame.add(gradient);
        frame.render(100, 10)
    };
    let srgb = render(gpui::ColorSpace::Srgb);
    let oklab = render(gpui::ColorSpace::Oklab);
    // The ends are the stops either way (a little off: the sRGB curve is steep
    // near zero, so a half percent of blue lifts the green).
    oklab.expect(0, 5, [252, 0, 3, 255], 20);
    oklab.expect(99, 5, [3, 0, 252, 255], 20);
    // Red to blue passes through (140, 83, 162) in Oklab, but only (128, 0, 128) in sRGB.
    oklab.expect(50, 5, [140, 83, 162, 255], 14);
    assert!(srgb.at(50, 5)[1] < 10, "{:?}", srgb.at(50, 5));
}

#[test]
fn a_border_follows_a_rounded_corner() {
    let mut frame = Frame::new();
    let mut bordered = quad(rect(10., 10., 40., 40.), 0xffffff);
    bordered.corner_radii = Corners::all(sp(12.));
    bordered.border_color = color(0x0000ff);
    bordered.border_widths = Edges::all(sp(4.));
    frame.add(bordered);
    let image = frame.render(60, 60);
    // Inside, the fill.
    image.expect(30, 30, WHITE, 1);
    image.expect(19, 19, WHITE, 8);
    // The straight stretches of the ring.
    image.expect(30, 10, BLUE, 2);
    image.expect(30, 13, BLUE, 2);
    image.expect(10, 30, BLUE, 2);
    image.expect(49, 30, BLUE, 2);
    image.expect(30, 49, BLUE, 2);
    // The ring turns the corner: 9.2 from the arc's centre, between the inner
    // radius (8) and the outer one (12).
    image.expect(15, 15, BLUE, 8);
    image.expect(44, 44, BLUE, 8);
    // Outside the arc nothing is drawn, border or not.
    image.expect(10, 10, BLACK, 0);
    image.expect(12, 12, BLACK, 0);
    image.expect(49, 10, BLACK, 0);
}

/// A rounded clip group the way the core hands one over: a sprite whose texture
/// id is in the clip range stands for `child`, which a geometry frame
/// accompanies with the vertical radii.
fn rounded_clip(
    bounds: Bounds<ScaledPixels>,
    radius: f32,
    child: impl FnOnce(&mut Scene),
) -> Scene {
    let mut content = Scene::default();
    child(&mut content);
    content.layers.frames.push(gpui::LayerFrame {
        key: gpui::LayerKey(u32::MAX),
        generation: 0,
        background: gpui::Rgba {
            r: radius,
            g: radius,
            b: radius,
            a: radius,
        },
        tile_size: 0,
        content: Scene::default().into(),
        dirty_tiles: Vec::new(),
    });
    content.finish();
    let key = gpui::LayerKey(0x0100_0000);
    let mut scene = Scene::default();
    scene.layers.frames.push(gpui::LayerFrame {
        key,
        generation: 0,
        background: gpui::Rgba::default(),
        tile_size: 0,
        content: content.into(),
        dirty_tiles: Vec::new(),
    });
    scene.insert_primitive(PolychromeSprite {
        order: 0,
        transform_index: 0,
        grayscale: PaddedBool32::from(false),
        opacity: 1.,
        bounds,
        content_mask: ClipMask::new(bounds),
        corner_radii: Corners::all(sp(radius)),
        tile: AtlasTile {
            texture_id: gpui::AtlasTextureId {
                index: 0xF100_0000,
                kind: gpui::AtlasTextureKind::Polychrome,
            },
            tile_id: gpui::TileId(0),
            padding: 0,
            bounds: Default::default(),
        },
    });
    scene
}

fn render_scene(frame: &mut Frame, scene: Scene, width: i32, height: i32) -> Image {
    let mut scene = scene;
    scene.finish();
    Image(
        frame
            .renderer
            .render_scene_to_image(&scene, size(DevicePixels(width), DevicePixels(height)))
            .expect("the scene renders"),
    )
}

#[test]
fn a_rounded_clip_group_rounds_what_overflows_it() {
    let mut frame = Frame::new();
    // A white square much bigger than the 40 x 40 clip it is drawn through.
    let scene = rounded_clip(rect(10., 10., 40., 40.), 16., |child| {
        child.insert_primitive(quad(rect(0., 0., 100., 100.), 0xffffff));
    });
    let image = render_scene(&mut frame, scene, 80, 80);
    image.expect(30, 30, WHITE, 1);
    image.expect(30, 10, WHITE, 8);
    image.expect(10, 30, WHITE, 8);
    // Rounded: the corner pixels of the clip's rectangle are left clear.
    image.expect(10, 10, BLACK, 0);
    image.expect(11, 11, BLACK, 0);
    image.expect(49, 49, BLACK, 0);
    // Inside the arc, though: (15.5, 15.5) is 14.8 from its centre, under 16.
    image.expect(15, 15, WHITE, 8);
    // Outside the rectangle the overflow is cut off.
    image.expect(5, 30, BLACK, 0);
    image.expect(60, 30, BLACK, 0);
    image.expect(30, 70, BLACK, 0);
}

#[test]
fn a_rounded_clip_group_with_square_corners_clips_to_its_rectangle() {
    let mut frame = Frame::new();
    let scene = rounded_clip(rect(10., 10., 40., 40.), 0., |child| {
        child.insert_primitive(quad(rect(0., 0., 100., 100.), 0x00ff00));
    });
    let image = render_scene(&mut frame, scene, 80, 80);
    image.expect(10, 10, GREEN, 1);
    image.expect(49, 49, GREEN, 1);
    image.expect(9, 10, BLACK, 0);
    image.expect(50, 49, BLACK, 0);
}

/// A scroll layer's tiles, composited the way the core hands them over.
fn scrolled_layer(translation_x: f32) -> Scene {
    let mut content = Scene::default();
    // Content in the layer's own space: a red square at (70, 10), which is in
    // the second 64-pixel tile, and a green one at (10, 70), in the row below.
    content.insert_primitive(quad(rect(70., 10., 20., 20.), 0xff0000));
    content.insert_primitive(quad(rect(10., 70., 20., 20.), 0x00ff00));
    content.finish();
    let frame = gpui::LayerFrame {
        key: gpui::LayerKey(7),
        generation: 1,
        background: gpui::rgba(0xe0e0e0ff),
        tile_size: 64,
        content: content.into(),
        dirty_tiles: Vec::new(),
    };
    let viewport = ClipMask::new(rect(0., 0., 100., 128.));
    let mut scene = Scene::default();
    for coord in [
        gpui::TileCoord { x: 0, y: 0 },
        gpui::TileCoord { x: 1, y: 0 },
        gpui::TileCoord { x: 0, y: 1 },
    ] {
        let tile = frame.tile_bounds(coord);
        scene.insert_primitive(PolychromeSprite {
            order: 0,
            transform_index: 0,
            grayscale: PaddedBool32::from(false),
            opacity: 1.,
            bounds: Bounds {
                origin: point(tile.origin.x + sp(translation_x), tile.origin.y),
                size: tile.size,
            },
            content_mask: viewport,
            corner_radii: Corners::default(),
            tile: AtlasTile {
                texture_id: gpui::layer_tile_texture_id(frame.key),
                tile_id: gpui::layer_tile_id(coord),
                padding: 0,
                bounds: Bounds {
                    origin: point(DevicePixels(0), DevicePixels(0)),
                    size: size(DevicePixels(64), DevicePixels(64)),
                },
            },
        });
    }
    scene.layers.frames.push(frame);
    scene
}

#[test]
fn a_scroll_layer_shows_the_tiles_its_content_is_cut_into() {
    let mut frame = Frame::new();
    // Scrolled 60 to the left: the second tile starts at x = 4.
    let image = render_scene(&mut frame, scrolled_layer(-60.), 100, 128);
    // The layer's background shows where the content is empty ...
    image.expect(1, 1, [224, 224, 224, 255], 1);
    image.expect(40, 50, [224, 224, 224, 255], 1);
    // ... and the content where it is: the red square is at x = 10..30 now.
    image.expect(20, 20, RED, 1);
    image.expect(9, 20, [224, 224, 224, 255], 1);
    image.expect(31, 20, [224, 224, 224, 255], 1);
    // The tile row below holds the green square, which its first tile shows
    // at x = -50..-30, out of the window; the 4 pixels of that tile that are
    // in it are background.
    image.expect(2, 80, [224, 224, 224, 255], 1);
    // Past the second tile, and where the row below has no tile, nothing was
    // composited.
    image.expect(80, 20, BLACK, 0);
    image.expect(20, 80, BLACK, 0);
}

/// What a crop of `scene` shows, and how many layer tiles were drawn for it.
fn cropped_layer(scene: &Scene, region: Bounds<DevicePixels>) -> (image::RgbaImage, usize) {
    let renderer = CpuHeadlessRenderer::new();
    let mut canvas =
        crate::fast::crop::canvas(size(DevicePixels(100), DevicePixels(128)), region)
            .expect("the crop is in the frame");
    let mut drawing = crate::Drawing {
        atlas: &renderer.atlas,
        canvas: &mut canvas,
        clips: Vec::new(),
        tiles: std::collections::HashMap::new(),
        spans: true,
    };
    drawing.scene(scene);
    let tiles = drawing.tiles.len();
    (canvas.into_image(), tiles)
}

#[test]
fn a_crop_of_a_scroll_layer_draws_only_the_tiles_it_shows() {
    let mut scene = scrolled_layer(-60.);
    scene.finish();
    let whole = render_scene(&mut Frame::new(), scrolled_layer(-60.), 100, 128).0;
    let region = |x, y, width, height| Bounds {
        origin: point(DevicePixels(x), DevicePixels(y)),
        size: size(DevicePixels(width), DevicePixels(height)),
    };
    // Tiles cover x = -60..4 and 4..68 in the first row, -60..4 in the second
    // (y = 64..128). A crop is the frame's pixels whichever tiles it draws.
    for (x, y, width, height, tiles) in [
        // Only the second tile of the first row.
        (10, 10, 20, 20, 1),
        // The edge between the first two tiles of the row, nothing below.
        (0, 0, 30, 30, 2),
        // The row below: only its tile, which is 4 pixels wide here.
        (0, 70, 10, 20, 1),
        // Past every tile: nothing to draw.
        (80, 20, 10, 10, 0),
        // The whole frame needs them all.
        (0, 0, 100, 128, 3),
    ] {
        let (crop, drawn) = cropped_layer(&scene, region(x, y, width, height));
        let expected =
            image::imageops::crop_imm(&whole, x as u32, y as u32, width as u32, height as u32)
                .to_image();
        assert!(
            crop == expected,
            "the crop ({x}, {y}) {width}x{height} differs from the whole frame cropped"
        );
        assert_eq!(
            drawn, tiles,
            "the crop ({x}, {y}) {width}x{height} drew {drawn} tiles"
        );
    }
}

/// A scene of many opaque one-colour quads at fractional bounds, each under a
/// mask that is a plain rectangle, a rounded one or none, some translucent,
/// bordered or rounded (which take the shader), some under a transform or a
/// colour filter, and one inside a rounded clip.
fn spanned_scene(seed: u64) -> Scene {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let mut frame = Frame::new();
    for index in 0..240 {
        let (x, y) = (-20. + next() * 140., -20. + next() * 140.);
        let (width, height) = (next() * 60., next() * 60.);
        let bounds = rect(x, y, width, height);
        let hex = (next() * 16_777_215.) as u32;
        let mut primitive = quad(bounds, hex);
        let mask = rect(
            -10. + next() * 100.,
            -10. + next() * 100.,
            next() * 100.,
            next() * 100.,
        );
        primitive.content_mask = match index % 6 {
            0 => open(),
            1 => ClipMask {
                bounds: mask,
                rounded_bounds: mask,
                corner_radii: Corners::all(sp(next() * 20.)),
            },
            _ => ClipMask::new(mask),
        };
        match index % 7 {
            0 => primitive.corner_radii = Corners::all(sp(next() * 10.)),
            1 => {
                primitive.border_widths = Edges::all(sp(next() * 3.));
                primitive.border_color = color(0x00ff00);
            }
            2 => primitive.background = solid_background(Hsla { a: next(), ..color(hex) }),
            _ => {}
        }
        let previous = match index % 11 {
            0 => Some(
                frame
                    .scene
                    .push_transform(TransformationMatrix::unit().translate(point(
                        sp(next() * 10.),
                        sp(next() * 10.),
                    ))),
            ),
            1 => {
                let mut halve_red = ColorFilter::IDENTITY;
                halve_red.rows[0][0] = next();
                Some(frame.scene.push_color_filter(halve_red))
            }
            _ => None,
        };
        frame.add(primitive);
        if let Some(previous) = previous {
            frame.scene.pop_transform(previous);
        }
    }
    let mut scene = frame.scene;
    scene.finish();
    scene
}

#[test]
fn filling_an_opaque_quad_in_spans_draws_what_the_shader_draws() {
    for seed in 1..=6 {
        let mut images = Vec::new();
        for spans in [false, true] {
            let mut renderer = CpuHeadlessRenderer::new();
            renderer.spans = spans;
            let scene = spanned_scene(seed);
            let image = renderer
                .render_scene_to_image(&scene, size(DevicePixels(120), DevicePixels(120)))
                .expect("the scene renders");
            images.push(image.into_raw());
        }
        assert!(
            images[0] == images[1],
            "seed {seed}: the spans drew something else than the shader"
        );
    }
}

#[test]
fn the_rounded_clip_is_the_same_with_and_without_spans() {
    let mut images = Vec::new();
    for spans in [false, true] {
        let mut frame = Frame::new();
        frame.renderer.spans = spans;
        let scene = rounded_clip(rect(5.3, 7.1, 70.4, 60.2), 12., |scene| {
            scene.insert_primitive(quad(rect(0., 0., 90.7, 40.5), 0x3366cc));
            scene.insert_primitive(quad(rect(10.5, 30.25, 40.5, 70.), 0xcc6633));
        });
        images.push(render_scene(&mut frame, scene, 100, 100).0.into_raw());
    }
    assert!(images[0] == images[1]);
}

#[test]
fn a_scroll_layer_tile_is_copied_pixel_for_pixel() {
    let mut frame = Frame::new();
    let image = render_scene(&mut frame, scrolled_layer(0.), 100, 128);
    // Unscrolled, the red square is where the content has it, in the second
    // tile, edges included, and the green one in the tile below.
    image.expect(70, 10, RED, 0);
    image.expect(89, 29, RED, 0);
    image.expect(69, 10, [224, 224, 224, 255], 0);
    image.expect(90, 10, [224, 224, 224, 255], 0);
    image.expect(70, 30, [224, 224, 224, 255], 0);
    image.expect(10, 70, GREEN, 0);
    image.expect(29, 89, GREEN, 0);
    image.expect(30, 89, [224, 224, 224, 255], 0);
}
