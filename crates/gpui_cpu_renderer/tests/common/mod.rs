//! A scene with every kind of large primitive the CPU renderer draws, for
//! tests that compare one rendering of it with another.

#![allow(dead_code)]

use gpui::{
    BorderStyle, Bounds, ClipMask, ColorFilter, Corners, DevicePixels, Edges, Hsla, PaddedBool32,
    Path, PlatformHeadlessRenderer, Quad, Radians, ScaledPixels, Scene, Shadow,
    TransformationMatrix, Underline, linear_color_stop, linear_gradient, point, px, rgb, size,
    solid_background,
};
use gpui_cpu_renderer::CpuHeadlessRenderer;

pub const WIDTH: i32 = 400;
pub const HEIGHT: i32 = 300;

fn sp(value: f32) -> ScaledPixels {
    ScaledPixels(value)
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(sp(x), sp(y)),
        size: size(sp(width), sp(height)),
    }
}

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

fn path(points: &[(f32, f32)], curve: Option<((f32, f32), (f32, f32))>, hex: u32) -> Path<ScaledPixels> {
    let mut path = Path::new(point(px(points[0].0), px(points[0].1)));
    for &(x, y) in &points[1..] {
        path.line_to(point(px(x), px(y)));
    }
    if let Some(((x, y), (control_x, control_y))) = curve {
        path.curve_to(point(px(x), px(y)), point(px(control_x), px(control_y)));
    }
    path.content_mask = ClipMask::new(Bounds {
        origin: point(px(-1000.), px(-1000.)),
        size: size(px(2000.), px(2000.)),
    });
    path.color = solid_background(color(hex));
    path.scale(1.)
}

/// The scene: a gradient backdrop, cards with radii, per-side and dashed
/// borders, shadows (outer and inset), underlines, paths, and quads under a
/// rotation and a colour filter.
pub fn scene() -> Scene {
    let mut scene = Scene::default();

    let mut backdrop = quad(rect(0., 0., WIDTH as f32, HEIGHT as f32), 0);
    backdrop.background = linear_gradient(
        35.,
        linear_color_stop(color(0xf2f4f8), 0.),
        linear_color_stop(color(0x6a8bd6), 1.),
    );
    scene.insert_primitive(backdrop);

    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: sp(12.),
        bounds: rect(30., 30., 140., 90.),
        corner_radii: Corners::all(sp(10.)),
        content_mask: open(),
        color: Hsla::black().opacity(0.5),
        element_bounds: rect(30., 30., 140., 90.),
        element_corner_radii: Corners::all(sp(10.)),
        inset: 0,
        transform_index: 0,
    });
    let mut card = quad(rect(30., 30., 140., 90.), 0xffffff);
    card.corner_radii = Corners::all(sp(10.));
    card.border_color = color(0x3355aa);
    card.border_widths = Edges {
        top: sp(1.),
        right: sp(3.),
        bottom: sp(5.),
        left: sp(2.),
    };
    scene.insert_primitive(card);

    let mut dashed = quad(rect(200., 30., 160., 90.), 0xfff3d6);
    dashed.border_color = color(0xcc4400);
    dashed.border_widths = Edges::all(sp(3.));
    dashed.corner_radii = Corners::all(sp(24.));
    dashed.border_style_and_transform =
        Quad::pack_border_style(BorderStyle::Dashed, gpui::TransformIndex::IDENTITY);
    scene.insert_primitive(dashed);

    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: sp(6.),
        bounds: rect(30., 150., 140., 110.),
        corner_radii: Corners::all(sp(16.)),
        content_mask: open(),
        color: Hsla::black(),
        element_bounds: rect(30., 150., 140., 110.),
        element_corner_radii: Corners::all(sp(16.)),
        inset: 1,
        transform_index: 0,
    });
    let mut masked = quad(rect(30., 150., 140., 110.), 0xe8f5e9);
    masked.corner_radii = Corners::all(sp(16.));
    masked.content_mask = ClipMask::new(rect(40., 160., 120., 90.));
    scene.insert_primitive(masked);

    scene.insert_primitive(Underline {
        order: 0,
        transform_index: 0,
        bounds: rect(200., 140., 160., 8.),
        content_mask: open(),
        color: color(0xff0000),
        thickness: sp(2.),
        wavy: PaddedBool32::from(true),
    });
    scene.insert_primitive(Underline {
        order: 0,
        transform_index: 0,
        bounds: rect(200., 156., 160., 4.),
        content_mask: open(),
        color: color(0x0000ff),
        thickness: sp(1.),
        wavy: PaddedBool32::from(false),
    });

    scene.insert_primitive(path(
        &[(200., 280.), (260., 170.), (320., 280.)],
        None,
        0x008800,
    ));
    scene.insert_primitive(path(&[(330., 280.)], Some(((390., 280.), (360., 170.))), 0xaa00aa));

    let mut halve_red = ColorFilter::IDENTITY;
    halve_red.rows[0][0] = 0.5;
    let filter = scene.push_color_filter(halve_red);
    let rotation = scene.push_transform(
        TransformationMatrix::unit()
            .translate(point(sp(300.), sp(210.)))
            .rotate(Radians(0.4)),
    );
    let mut turned = quad(rect(-40., -25., 80., 50.), 0xff3322);
    turned.corner_radii = Corners::all(sp(8.));
    turned.border_color = color(0x000000);
    turned.border_widths = Edges::all(sp(2.));
    scene.insert_primitive(turned);
    scene.pop_transform(rotation);
    scene.pop_transform(filter);

    scene.finish();
    scene
}

fn shadow(
    bounds: Bounds<ScaledPixels>,
    blur: f32,
    radii: Corners<ScaledPixels>,
    inset: u32,
) -> Shadow {
    Shadow {
        order: 0,
        blur_radius: sp(blur),
        bounds,
        corner_radii: radii,
        content_mask: open(),
        color: Hsla::black().opacity(0.6),
        element_bounds: bounds,
        element_corner_radii: radii,
        inset,
        transform_index: 0,
    }
}

/// The corners of `scene()` leave out: patterns and gradients of every kind,
/// shadows with no blur, inset ones, ones cut off by the frame or too small
/// for their blur, unequal radii, solid quads of every sort (opaque and not,
/// whole pixels or not, cut by a plain mask and by a rounded one, off the
/// frame, under a transform).
pub fn scene_extras() -> Scene {
    let mut scene = Scene::default();
    scene.insert_primitive(quad(rect(0., 0., WIDTH as f32, HEIGHT as f32), 0xf7f7fa));

    scene.insert_primitive(quad(rect(10.3, 12.7, 90.4, 50.2), 0x336699));
    let mut half = quad(rect(60.6, 30.2, 70.1, 40.9), 0xff8800);
    half.background = solid_background(color(0xff8800).opacity(0.5));
    scene.insert_primitive(half);
    scene.insert_primitive(quad(rect(0., 80., 12., 6.), 0x010203));
    scene.insert_primitive(quad(rect(-20.5, 250.5, 60., 70.), 0x44aa44));
    scene.insert_primitive(quad(rect(380.5, -10., 40., 40.), 0xaa4444));

    let mut cut = quad(rect(120.2, 14.6, 100.7, 60.1), 0x9955cc);
    cut.content_mask = ClipMask::new(rect(130.4, 20.5, 70.3, 80.));
    scene.insert_primitive(cut);
    let mut rounded_mask = quad(rect(120.2, 90.6, 100.7, 60.1), 0x2277bb);
    let mut mask = ClipMask::new(rect(125.5, 95.5, 90., 50.));
    mask.rounded_bounds = rect(125.5, 95.5, 90., 50.);
    mask.corner_radii = Corners::all(sp(14.));
    rounded_mask.content_mask = mask;
    scene.insert_primitive(rounded_mask);

    let mut stripes = quad(rect(240., 12., 70., 60.), 0);
    stripes.background = gpui::pattern_slash(color(0x884488), 4., 3.);
    scene.insert_primitive(stripes);
    let mut checks = quad(rect(320., 12.5, 60., 60.), 0);
    checks.background = gpui::checkerboard(color(0x22aa55), 6.);
    scene.insert_primitive(checks);

    let mut oklab = quad(rect(10., 100., 100., 70.), 0);
    oklab.background = linear_gradient(
        200.,
        linear_color_stop(color(0xff0066), 0.),
        linear_color_stop(color(0x00ccff), 1.),
    )
    .color_space(gpui::ColorSpace::Oklab);
    scene.insert_primitive(oklab);
    let mut wide = quad(rect(10., 180., 160., 30.), 0);
    wide.background = linear_gradient(
        90.,
        linear_color_stop(color(0x222222), 0.2),
        linear_color_stop(color(0xeeeeee), 0.8),
    );
    scene.insert_primitive(wide);
    let mut tall = quad(rect(180., 160., 30., 100.), 0);
    tall.background = linear_gradient(
        0.,
        linear_color_stop(color(0xffcc00).opacity(0.7), 0.),
        linear_color_stop(color(0x0033ff), 1.),
    );
    tall.corner_radii = Corners::all(sp(6.));
    scene.insert_primitive(tall);

    scene.insert_primitive(shadow(
        rect(240., 90., 60., 40.),
        0.,
        Corners::all(sp(8.)),
        0,
    ));
    scene.insert_primitive(shadow(
        rect(320., 90., 60., 40.),
        9.,
        Corners::default(),
        0,
    ));
    scene.insert_primitive(shadow(
        rect(240., 150., 60., 50.),
        5.,
        Corners::all(sp(12.)),
        1,
    ));
    scene.insert_primitive(shadow(
        rect(-10., -12., 50., 40.),
        8.,
        Corners::all(sp(10.)),
        0,
    ));
    scene.insert_primitive(shadow(
        rect(330., 160., 6., 6.),
        20.,
        Corners::all(sp(2.)),
        0,
    ));
    scene.insert_primitive(shadow(
        rect(250., 220., 90., 50.),
        10.,
        Corners {
            top_left: sp(22.),
            top_right: sp(2.),
            bottom_right: sp(14.),
            bottom_left: sp(0.),
        },
        0,
    ));
    let mut deep = quad(rect(250., 220., 90., 50.), 0xffffff);
    deep.corner_radii = Corners {
        top_left: sp(22.),
        top_right: sp(2.),
        bottom_right: sp(14.),
        bottom_left: sp(0.),
    };
    scene.insert_primitive(deep);

    let mut dim = ColorFilter::IDENTITY;
    dim.rows[1][1] = 0.4;
    let filter = scene.push_color_filter(dim);
    let rotation = scene.push_transform(
        TransformationMatrix::unit()
            .translate(point(sp(90.), sp(260.)))
            .rotate(Radians(-0.3)),
    );
    scene.insert_primitive(quad(rect(-30.5, -15.5, 60., 30.), 0x88cc88));
    scene.insert_primitive(shadow(
        rect(-20., -10., 40., 20.),
        6.,
        Corners::all(sp(4.)),
        0,
    ));
    scene.pop_transform(rotation);
    scene.pop_transform(filter);

    scene.finish();
    scene
}

pub fn frame_size() -> gpui::Size<DevicePixels> {
    size(DevicePixels(WIDTH), DevicePixels(HEIGHT))
}

/// The scene drawn whole.
pub fn render() -> image::RgbaImage {
    CpuHeadlessRenderer::new()
        .render_scene_to_image(&scene(), frame_size())
        .expect("the scene renders")
}

/// `scene_extras()` drawn whole.
pub fn render_extras() -> image::RgbaImage {
    CpuHeadlessRenderer::new()
        .render_scene_to_image(&scene_extras(), frame_size())
        .expect("the scene renders")
}

/// An FNV-1a hash of an image's size and bytes, stable across Rust versions.
pub fn fingerprint(image: &image::RgbaImage) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in image
        .width()
        .to_le_bytes()
        .into_iter()
        .chain(image.height().to_le_bytes())
        .chain(image.as_raw().iter().copied())
    {
        hash = (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
