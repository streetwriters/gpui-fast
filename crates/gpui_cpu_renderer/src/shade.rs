//! The fragment shaders of `gpui_apple/src/shaders.metal`, evaluated for one
//! pixel at a time on the CPU. Positions are in device pixels, colours are
//! straight (not premultiplied) RGBA in `0.0..=1.0`.
//!
//! The gradient and shadow dither of the Metal shaders is ported too, so
//! the noise is the same where the arithmetic is. Against Metal the two
//! renderers' screenshots differ by at most 3/255 per channel.
//!
//! This is a third copy of the shader maths, after `shaders.metal` and
//! `gpui_wgpu/src/shaders.wgsl`: when either changes (a new background kind,
//! a different shadow falloff), change this to match. The `cpu_vs_gpu` test in
//! `tests/` measures each element of a scene against the GPU renderer and fails
//! on a machine with a GPU when one has drifted.

use std::ops::{Add, Div, Mul, Sub};

use gpui::{
    Background, Bounds, ClipMask, ColorFilter, Corners, FillParts, Hsla, Quad, ScaledPixels,
    Shadow, Underline,
};

pub(crate) type Rgba = [f32; 4];

/// A two component vector, to keep the ports of the shader code readable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct V2 {
    pub x: f32,
    pub y: f32,
}

pub(crate) const fn v2(x: f32, y: f32) -> V2 {
    V2 { x, y }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, other: V2) -> V2 {
        v2(self.x + other.x, self.y + other.y)
    }
}

impl Sub for V2 {
    type Output = V2;
    fn sub(self, other: V2) -> V2 {
        v2(self.x - other.x, self.y - other.y)
    }
}

impl Mul<f32> for V2 {
    type Output = V2;
    fn mul(self, factor: f32) -> V2 {
        v2(self.x * factor, self.y * factor)
    }
}

impl Mul<V2> for V2 {
    type Output = V2;
    fn mul(self, other: V2) -> V2 {
        v2(self.x * other.x, self.y * other.y)
    }
}

impl Div<f32> for V2 {
    type Output = V2;
    fn div(self, divisor: f32) -> V2 {
        v2(self.x / divisor, self.y / divisor)
    }
}

impl Div<V2> for V2 {
    type Output = V2;
    fn div(self, other: V2) -> V2 {
        v2(self.x / other.x, self.y / other.y)
    }
}

impl V2 {
    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    fn abs(self) -> V2 {
        v2(self.x.abs(), self.y.abs())
    }

    fn dot(self, other: V2) -> f32 {
        self.x * other.x + self.y * other.y
    }

    pub fn splat(value: f32) -> V2 {
        v2(value, value)
    }

    pub fn max(self, other: V2) -> V2 {
        v2(self.x.max(other.x), self.y.max(other.y))
    }

    pub fn min(self, other: V2) -> V2 {
        v2(self.x.min(other.x), self.y.min(other.y))
    }
}

/// `fract` of the shading languages: `x - floor(x)`.
fn fract(value: f32) -> f32 {
    value - value.floor()
}

fn saturate(value: f32) -> f32 {
    value.clamp(0., 1.)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix4(a: Rgba, b: Rgba, t: f32) -> Rgba {
    [
        mix(a[0], b[0], t),
        mix(a[1], b[1], t),
        mix(a[2], b[2], t),
        mix(a[3], b[3], t),
    ]
}

pub(crate) fn origin(bounds: &Bounds<ScaledPixels>) -> V2 {
    v2(bounds.origin.x.0, bounds.origin.y.0)
}

pub(crate) fn size(bounds: &Bounds<ScaledPixels>) -> V2 {
    v2(bounds.size.width.0, bounds.size.height.0)
}

pub(crate) fn hsla_to_rgba(hsla: Hsla) -> Rgba {
    let h = hsla.h * 6.0;
    let (s, l, a) = (hsla.s, hsla.l, hsla.a);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if (0.0..1.0).contains(&h) {
        (c, x, 0.0)
    } else if (1.0..2.0).contains(&h) {
        (x, c, 0.0)
    } else if (2.0..3.0).contains(&h) {
        (0.0, c, x)
    } else if (3.0..4.0).contains(&h) {
        (0.0, x, c)
    } else if (4.0..5.0).contains(&h) {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [r + m, g + m, b + m, a]
}

fn srgb_to_oklab(color: Rgba) -> Rgba {
    let [r, g, b, a] = [
        color[0].powf(2.2),
        color[1].powf(2.2),
        color[2].powf(2.2),
        color[3],
    ];
    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
        a,
    ]
}

fn oklab_to_srgb(color: Rgba) -> Rgba {
    let l_ = color[0] + 0.3963377774 * color[1] + 0.2158037573 * color[2];
    let m_ = color[0] - 0.1055613458 * color[1] - 0.0638541728 * color[2];
    let s_ = color[0] - 0.0894841775 * color[1] - 1.2914855480 * color[2];
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let linear = [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ];
    let encode = |value: f32| value.max(0.).powf(1.0 / 2.2);
    [
        encode(linear[0]),
        encode(linear[1]),
        encode(linear[2]),
        color[3],
    ]
}

/// A fill prepared for a primitive's bounds, with the colour conversions done
/// once.
pub(crate) struct Fill {
    parts: FillParts,
    solid: Rgba,
    color0: Rgba,
    color1: Rgba,
    bounds: Bounds<ScaledPixels>,
}

impl Fill {
    pub fn new(background: &Background, bounds: Bounds<ScaledPixels>) -> Self {
        let parts = background.fill_parts();
        let mut solid = [0.; 4];
        let mut color0 = [0.; 4];
        let mut color1 = [0.; 4];
        match parts.tag {
            1 => {
                color0 = hsla_to_rgba(parts.stops[0].color);
                color1 = hsla_to_rgba(parts.stops[1].color);
                if parts.oklab {
                    color0 = srgb_to_oklab(color0);
                    color1 = srgb_to_oklab(color1);
                }
            }
            _ => solid = hsla_to_rgba(parts.solid),
        }
        Self {
            parts,
            solid,
            color0,
            color1,
            bounds,
        }
    }

    /// `fill_color` of the Metal shaders.
    pub fn at(&self, position: V2) -> Rgba {
        let bounds = &self.bounds;
        let (bounds_origin, bounds_size) = (origin(bounds), size(bounds));
        match self.parts.tag {
            0 => self.solid,
            1 => {
                let angle = self.parts.angle_or_pattern;
                let radians = ((angle % 360.0) - 90.0) * (std::f32::consts::PI / 180.0);
                let mut direction = v2(radians.cos(), radians.sin());
                if bounds_size.x > bounds_size.y {
                    direction.y *= bounds_size.y / bounds_size.x;
                } else {
                    direction.x *= bounds_size.x / bounds_size.y;
                }
                let half_size = bounds_size / 2.;
                let center = bounds_origin + half_size;
                let center_to_point = position - center;
                let mut t = center_to_point.dot(direction) / direction.length();
                if direction.x.abs() > direction.y.abs() {
                    t = (t + half_size.x) / bounds_size.x;
                } else {
                    t = (t + half_size.y) / bounds_size.y;
                }
                let [stop0, stop1] = self.parts.stops;
                t = (t - stop0.percentage) / (stop1.percentage - stop0.percentage);
                let t = if t.is_nan() { 0. } else { t.clamp(0., 1.) };
                let mixed = mix4(self.color0, self.color1, t);
                let color = if self.parts.oklab {
                    oklab_to_srgb(mixed)
                } else {
                    mixed
                };
                // Metal's dither: triangular noise from two `sin` hashes.
                let seed = (position - bounds_origin) * 0.618_034;
                let r1 = fract(seed.dot(v2(12.9898, 78.233)).sin() * 43758.5453);
                let r2 = fract(seed.dot(v2(39.3460, 11.135)).sin() * 24634.6345);
                let tri = r1 + r2 - 1.0;
                [
                    color[0] + tri * 2.0 / 255.0,
                    color[1] + tri * 2.0 / 255.0,
                    color[2] + tri * 2.0 / 255.0,
                    color[3] + tri * 3.0 / 255.0,
                ]
            }
            2 => {
                let packed = self.parts.angle_or_pattern;
                let pattern_width = (packed / 65535.0) / 255.0;
                let pattern_interval = (packed % 65535.0) / 255.0;
                let pattern_height = pattern_width + pattern_interval;
                let stripe_angle = std::f32::consts::FRAC_PI_4;
                let pattern_period = pattern_height * stripe_angle.sin();
                let relative = position - bounds_origin;
                let (s, c) = stripe_angle.sin_cos();
                // float2x2(c, -s, s, c) is column major: x' = c*x + s*y.
                let rotated_x = c * relative.x + s * relative.y;
                let pattern = rotated_x % pattern_period;
                let distance = pattern.min(pattern_period - pattern)
                    - pattern_period * (pattern_width / pattern_height) / 2.0;
                let mut color = self.solid;
                color[3] *= saturate(0.5 - distance);
                color
            }
            _ => {
                let size = self.parts.angle_or_pattern;
                let relative = position - bounds_origin;
                let x_index = (relative.x / size).floor();
                let y_index = (relative.y / size).floor();
                let should_be_colored = (x_index + y_index) % 2.0;
                let mut color = self.solid;
                color[3] *= saturate(should_be_colored);
                color
            }
        }
    }
}

fn pick_corner_radius(center_to_point: V2, radii: &Corners<ScaledPixels>) -> f32 {
    match (center_to_point.x < 0., center_to_point.y < 0.) {
        (true, true) => radii.top_left.0,
        (true, false) => radii.bottom_left.0,
        (false, true) => radii.top_right.0,
        (false, false) => radii.bottom_right.0,
    }
}

fn quad_sdf_impl(corner_center_to_point: V2, corner_radius: f32) -> f32 {
    if corner_radius == 0.0 {
        corner_center_to_point.x.max(corner_center_to_point.y)
    } else {
        let inset = corner_center_to_point.max(V2::splat(0.)).length()
            + corner_center_to_point.x.max(corner_center_to_point.y).min(0.0);
        inset - corner_radius
    }
}

pub(crate) fn quad_sdf(
    point: V2,
    bounds: &Bounds<ScaledPixels>,
    radii: &Corners<ScaledPixels>,
) -> f32 {
    let half_size = size(bounds) / 2.0;
    let center = origin(bounds) + half_size;
    let center_to_point = point - center;
    let corner_radius = pick_corner_radius(center_to_point, radii);
    let corner_to_point = center_to_point.abs() - half_size;
    let corner_center_to_point = corner_to_point + V2::splat(corner_radius);
    quad_sdf_impl(corner_center_to_point, corner_radius)
}

fn no_radii(radii: &Corners<ScaledPixels>) -> bool {
    radii.top_left.0 == 0.
        && radii.top_right.0 == 0.
        && radii.bottom_right.0 == 0.
        && radii.bottom_left.0 == 0.
}

pub(crate) fn rounded_clip_alpha(
    position: V2,
    bounds: &Bounds<ScaledPixels>,
    radii: &Corners<ScaledPixels>,
) -> f32 {
    if no_radii(radii) {
        return 1.;
    }
    saturate(0.5 - quad_sdf(position, bounds, radii))
}

pub(crate) fn content_mask_alpha(position: V2, mask: &ClipMask<ScaledPixels>) -> f32 {
    rounded_clip_alpha(position, &mask.rounded_bounds, &mask.corner_radii)
}

/// Whether `position` is inside the hard rectangle of a content mask.
pub(crate) fn inside_mask_rect(position: V2, mask: &ClipMask<ScaledPixels>) -> bool {
    let (o, s) = (origin(&mask.bounds), size(&mask.bounds));
    position.x - o.x >= 0.
        && o.x + s.x - position.x >= 0.
        && position.y - o.y >= 0.
        && o.y + s.y - position.y >= 0.
}

fn over(below: Rgba, above: Rgba) -> Rgba {
    let alpha = above[3] + below[3] * (1.0 - above[3]);
    if alpha <= 0. {
        return [0.; 4];
    }
    let channel = |index: usize| {
        (above[index] * above[3] + below[index] * below[3] * (1.0 - above[3])) / alpha
    };
    [channel(0), channel(1), channel(2), alpha]
}

fn quarter_ellipse_sdf(point: V2, radii: V2) -> f32 {
    let circle = point / radii;
    (circle.length() - 1.0) * (radii.x + radii.y) * -0.5
}

fn corner_dash_velocity(dv1: f32, dv2: f32) -> f32 {
    if dv1 == 0.0 {
        dv2
    } else if dv2 == 0.0 {
        dv1
    } else {
        dv1.min(dv2)
    }
}

fn dash_alpha(t: f32, period: f32, length: f32, dash_velocity: f32, antialias: f32) -> f32 {
    let half_period = period / 2.0;
    let half_length = length / 2.0;
    let centered = ((t + half_period - half_length) % period) - half_period;
    let signed_distance = centered.abs() - half_length;
    saturate(antialias - signed_distance / dash_velocity)
}

/// `quad_fragment_body`: the colour of a quad at `frag_pos`, before the colour
/// filter. `border_color` is the quad's border colour as RGBA.
pub(crate) fn quad_color(quad: &Quad, fill: &Fill, border_color: Rgba, frag_pos: V2) -> Rgba {
    let mask_alpha = content_mask_alpha(frag_pos, &quad.content_mask);
    let background = fill.at(frag_pos);
    let scale_alpha = |color: Rgba, alpha: f32| [color[0], color[1], color[2], color[3] * alpha];

    let radii = &quad.corner_radii;
    let widths = &quad.border_widths;
    let unrounded = no_radii(radii);
    if widths.top.0 == 0.
        && widths.left.0 == 0.
        && widths.right.0 == 0.
        && widths.bottom.0 == 0.
        && unrounded
        && quad.transform_index().0 == 0
    {
        return scale_alpha(background, mask_alpha);
    }

    let quad_size = size(&quad.bounds);
    let half_size = quad_size / 2.0;
    let point = frag_pos - origin(&quad.bounds);
    let center_to_point = point - half_size;
    let antialias = 0.5;

    let corner_radius = pick_corner_radius(center_to_point, radii);
    let border = v2(
        if center_to_point.x < 0.0 { widths.left.0 } else { widths.right.0 },
        if center_to_point.y < 0.0 { widths.top.0 } else { widths.bottom.0 },
    );
    let reduced_border = v2(
        if border.x == 0.0 { -antialias } else { border.x },
        if border.y == 0.0 { -antialias } else { border.y },
    );
    let corner_to_point = center_to_point.abs() - half_size;
    let corner_center_to_point = corner_to_point + V2::splat(corner_radius);
    let is_near_rounded_corner = corner_center_to_point.x >= 0.0 && corner_center_to_point.y >= 0.0;
    let straight_border_inner_corner_to_point = corner_to_point + reduced_border;
    let is_beyond_inner_straight_border = straight_border_inner_corner_to_point.x > 0.0
        || straight_border_inner_corner_to_point.y > 0.0;
    let is_within_inner_straight_border = straight_border_inner_corner_to_point.x < -antialias
        && straight_border_inner_corner_to_point.y < -antialias;
    if is_within_inner_straight_border && !is_near_rounded_corner {
        return scale_alpha(background, mask_alpha);
    }

    let outer_sdf = quad_sdf_impl(corner_center_to_point, corner_radius);
    let inner_sdf = if corner_center_to_point.x <= 0.0 || corner_center_to_point.y <= 0.0 {
        -straight_border_inner_corner_to_point
            .x
            .max(straight_border_inner_corner_to_point.y)
    } else if is_beyond_inner_straight_border {
        -1.0
    } else if reduced_border.x == reduced_border.y {
        -(outer_sdf + reduced_border.x)
    } else {
        let ellipse_radii = (V2::splat(corner_radius) - reduced_border).max(V2::splat(0.));
        quarter_ellipse_sdf(corner_center_to_point, ellipse_radii)
    };

    let border_sdf = inner_sdf.max(outer_sdf);
    let mut color = background;
    if border_sdf < antialias {
        let mut border_color = border_color;
        if (quad.border_style_and_transform & 0xff) == 1 {
            border_color[3] *= dashed_border_alpha(
                quad,
                quad_size,
                point,
                center_to_point,
                corner_center_to_point,
                corner_radius,
                is_near_rounded_corner,
                unrounded,
                antialias,
            );
        }
        let blended_border = over(background, border_color);
        color = mix4(background, blended_border, saturate(antialias - inner_sdf));
    }
    scale_alpha(color, saturate(antialias - outer_sdf) * mask_alpha)
}

/// The dash pattern of a dashed border at one point, as a factor of the border
/// colour's alpha.
#[allow(clippy::too_many_arguments)]
fn dashed_border_alpha(
    quad: &Quad,
    size: V2,
    point: V2,
    center_to_point: V2,
    corner_center_to_point: V2,
    corner_radius: f32,
    is_near_rounded_corner: bool,
    unrounded: bool,
    antialias: f32,
) -> f32 {
    let dash_length_per_width = 2.0;
    let dash_gap_per_width = 1.0;
    let dash_period_per_width = dash_length_per_width + dash_gap_per_width;
    let dv_numerator = 1.0 / dash_period_per_width;
    let widths = &quad.border_widths;
    let radii = &quad.corner_radii;

    let t;
    let max_t;
    let dash_velocity;
    if unrounded {
        let is_horizontal = corner_center_to_point.x < corner_center_to_point.y;
        let dashed_border = v2(
            widths.bottom.0.max(widths.top.0),
            widths.right.0.max(widths.left.0),
        );
        let border_width = if is_horizontal { dashed_border.x } else { dashed_border.y };
        dash_velocity = dv_numerator / border_width;
        t = (if is_horizontal { point.x } else { point.y }) * dash_velocity;
        max_t = (if is_horizontal { size.x } else { size.y }) * dash_velocity;
    } else {
        let (r_tr, r_br, r_bl, r_tl) = (
            radii.top_right.0,
            radii.bottom_right.0,
            radii.bottom_left.0,
            radii.top_left.0,
        );
        let (w_t, w_r, w_b, w_l) = (widths.top.0, widths.right.0, widths.bottom.0, widths.left.0);
        let velocity = |width: f32| if width <= 0.0 { 0.0 } else { dv_numerator / width };
        let (dv_t, dv_r, dv_b, dv_l) = (velocity(w_t), velocity(w_r), velocity(w_b), velocity(w_l));

        let s_t = (size.x - r_tl - r_tr) * dv_t;
        let s_r = (size.y - r_tr - r_br) * dv_r;
        let s_b = (size.x - r_br - r_bl) * dv_b;
        let s_l = (size.y - r_bl - r_tl) * dv_l;

        let cdv_tr = corner_dash_velocity(dv_t, dv_r);
        let cdv_br = corner_dash_velocity(dv_b, dv_r);
        let cdv_bl = corner_dash_velocity(dv_b, dv_l);
        let cdv_tl = corner_dash_velocity(dv_t, dv_l);

        let quarter = std::f32::consts::FRAC_PI_2;
        let c_tr = r_tr * quarter * cdv_tr;
        let c_br = r_br * quarter * cdv_br;
        let c_bl = r_bl * quarter * cdv_bl;
        let c_tl = r_tl * quarter * cdv_tl;

        let upto_tr = s_t;
        let upto_r = upto_tr + c_tr;
        let upto_br = upto_r + s_r;
        let upto_b = upto_br + c_br;
        let upto_bl = upto_b + s_b;
        let upto_l = upto_bl + c_bl;
        let upto_tl = upto_l + s_l;
        max_t = upto_tl + c_tl;

        if is_near_rounded_corner {
            let radians = corner_center_to_point.y.atan2(corner_center_to_point.x);
            let corner_t = radians * corner_radius;
            if center_to_point.x >= 0.0 {
                if center_to_point.y < 0.0 {
                    dash_velocity = cdv_tr;
                    t = upto_r - corner_t * dash_velocity;
                } else {
                    dash_velocity = cdv_br;
                    t = upto_br + corner_t * dash_velocity;
                }
            } else if center_to_point.y >= 0.0 {
                dash_velocity = cdv_bl;
                t = upto_l - corner_t * dash_velocity;
            } else {
                dash_velocity = cdv_tl;
                t = upto_tl + corner_t * dash_velocity;
            }
        } else {
            let is_horizontal = corner_center_to_point.x < corner_center_to_point.y;
            if is_horizontal {
                if center_to_point.y < 0.0 {
                    dash_velocity = dv_t;
                    t = (point.x - r_tl) * dash_velocity;
                } else {
                    dash_velocity = dv_b;
                    t = upto_bl - (point.x - r_bl) * dash_velocity;
                }
            } else if center_to_point.x < 0.0 {
                dash_velocity = dv_l;
                t = upto_tl - (point.y - r_tl) * dash_velocity;
            } else {
                dash_velocity = dv_r;
                t = upto_r + (point.y - r_tr) * dash_velocity;
            }
        }
    }

    let dash_length = dash_length_per_width / dash_period_per_width;
    let max_t = max_t - if unrounded { dash_length } else { 0.0 };
    if max_t >= 1.0 {
        let dash_count = max_t.floor();
        let dash_period = max_t / dash_count;
        dash_alpha(t, dash_period, dash_length, dash_velocity, antialias)
    } else if unrounded {
        let dash_gap = max_t - dash_length;
        if dash_gap > 0.0 {
            let dash_period = dash_length + dash_gap;
            dash_alpha(t, dash_period, dash_length, dash_velocity, antialias)
        } else {
            1.0
        }
    } else {
        1.0
    }
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    (-(x * x) / (2. * sigma * sigma)).exp() / ((2. * std::f32::consts::PI).sqrt() * sigma)
}

fn erf(x: f32) -> f32 {
    let sign = if x > 0. {
        1.
    } else if x < 0. {
        -1.
    } else {
        0.
    };
    let a = x.abs();
    let r1 = 1. + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    sign - sign / (r2 * r2)
}

fn blur_along_x(x: f32, y: f32, sigma: f32, corner: f32, half_size: V2) -> f32 {
    let delta = (half_size.y - corner - y.abs()).min(0.);
    let curved = half_size.x - corner + (corner * corner - delta * delta).max(0.).sqrt();
    let scale = 0.5f32.sqrt() / sigma;
    let low = 0.5 + 0.5 * erf((x - curved) * scale);
    let high = 0.5 + 0.5 * erf((x + curved) * scale);
    high - low
}

/// `shadow_fragment_body` without the colour: the shadow's alpha at `frag_pos`.
pub(crate) fn shadow_alpha(shadow: &Shadow, frag_pos: V2) -> f32 {
    let half_size = size(&shadow.bounds) / 2.;
    let center = origin(&shadow.bounds) + half_size;
    let point = frag_pos - center;
    let corner_radius = pick_corner_radius(point, &shadow.corner_radii);
    let blur = shadow.blur_radius.0;

    let mut alpha;
    if blur == 0. {
        alpha = saturate(0.5 - quad_sdf(frag_pos, &shadow.bounds, &shadow.corner_radii));
    } else {
        let low = point.y - half_size.y;
        let high = point.y + half_size.y;
        let start = (-3. * blur).clamp(low.min(high), low.max(high));
        let end = (3. * blur).clamp(low.min(high), low.max(high));
        let step = (end - start) / 4.;
        let mut y = start + step * 0.5;
        alpha = 0.;
        for _ in 0..4 {
            alpha += blur_along_x(point.x, point.y - y, blur, corner_radius, half_size)
                * gaussian(y, blur)
                * step;
            y += step;
        }
    }
    if shadow.inset != 0 {
        alpha = 1. - alpha;
        let element_distance = quad_sdf(
            frag_pos,
            &shadow.element_bounds,
            &shadow.element_corner_radii,
        );
        alpha *= saturate(0.5 - element_distance);
    }
    // Metal's dither: interleaved gradient noise, triangular over about +-1/255.
    let p = frag_pos - origin(&shadow.bounds);
    let weights = v2(0.067_110_56, 0.005_837_15);
    let a = fract(52.982_918_9 * fract(p.dot(weights)));
    let b = fract(52.982_918_9 * fract((p + V2::splat(0.5)).dot(weights)));
    alpha = (alpha + (a + b - 1.0) / 255.0 * (alpha * 2.0).min(1.0)).max(0.0);
    alpha * content_mask_alpha(frag_pos, &shadow.content_mask)
}

/// `underline_fragment_body` without the colour: the underline's alpha.
pub(crate) fn underline_alpha(underline: &Underline, frag_pos: V2) -> f32 {
    let mask_alpha = content_mask_alpha(frag_pos, &underline.content_mask);
    if !underline.wavy.is_set() {
        return mask_alpha;
    }
    let wave_frequency = 2.0;
    let wave_height_ratio = 0.8;
    let thickness = underline.thickness.0;
    let height = underline.bounds.size.height.0;
    let half_thickness = thickness * 0.5;
    let st = (frag_pos - origin(&underline.bounds)) / height - v2(0., 0.5);
    let frequency = (std::f32::consts::PI * wave_frequency * thickness) / height;
    let amplitude = (thickness * wave_height_ratio) / height;
    let sine = (st.x * frequency).sin() * amplitude;
    let d_sine = (st.x * frequency).cos() * amplitude * frequency;
    let distance = (st.y - sine) / (1. + d_sine * d_sine).sqrt();
    let distance_in_pixels = distance * height;
    let from_top = distance_in_pixels - half_thickness;
    let from_bottom = distance_in_pixels + half_thickness;
    saturate(0.5 - (-from_bottom).max(from_top)) * mask_alpha
}

/// `apply_color_filter`.
pub(crate) fn apply_color_filter(filter: &ColorFilter, color: Rgba) -> Rgba {
    if color[3] <= 0.0 {
        return color;
    }
    let m = &filter.rows;
    let mut out = [0.; 4];
    for (row, out) in m.iter().zip(out.iter_mut()) {
        *out = saturate(
            row[0] * color[0] + row[1] * color[1] + row[2] * color[2] + row[3] * color[3] + row[4],
        );
    }
    out
}

/// One rounded clip of `fast_rounded_clip_coverage`: elliptical radii, in device
/// pixels.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RoundedClip {
    pub min: V2,
    pub max: V2,
    /// Top left, top right, bottom right, bottom left.
    pub horizontal: [f32; 4],
    pub vertical: [f32; 4],
}

impl RoundedClip {
    pub fn coverage(&self, position: V2) -> f32 {
        let extent = self.max - self.min;
        if extent.x <= 0. || extent.y <= 0. {
            return 0.;
        }
        let local = position - self.min;
        let mut distance = (-local.x)
            .max(-local.y)
            .max((local.x - extent.x).max(local.y - extent.y));
        for corner in 0..4 {
            let radius = v2(self.horizontal[corner], self.vertical[corner]);
            if radius.x <= 0. || radius.y <= 0. {
                continue;
            }
            let right = corner == 1 || corner == 2;
            let bottom = corner >= 2;
            let inward = v2(
                if right { extent.x - local.x } else { local.x },
                if bottom { extent.y - local.y } else { local.y },
            );
            if inward.x < radius.x && inward.y < radius.y {
                let q = (inward - radius) / radius;
                let length_q = q.length();
                let gradient = (q / radius).length() / length_q.max(0.000001);
                distance = distance.max((length_q - 1.) / gradient.max(0.000001));
            }
        }
        (0.5 - distance).clamp(0., 1.)
    }
}
