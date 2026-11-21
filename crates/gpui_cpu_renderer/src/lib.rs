//! A software renderer for headless tests, on tiny-skia's pixel buffer.
//!
//! `#[gpui::test(render)]` and `cx.screenshot(..)` normally draw with Metal or
//! wgpu, which need a GPU. This renderer draws the same `Scene` on the CPU by
//! evaluating the Metal fragment shaders (`gpui_apple/src/shaders.metal`) one
//! pixel at a time, in draw order, so layout, colours and text match the GPU's
//! output to within antialiasing. It uses the same sprite atlas contract and
//! text system as the GPU renderers.
//!
//! Scroll layers are drawn as the GPU renderers do: each tile is rasterized
//! into a texture of its own and composited. It does not draw surfaces,
//! external textures, dynamic textures or effect groups (opacity, blur, drop
//! shadow, blend modes); `docs/ui-testing.md` lists what is left out. Anything
//! skipped is logged once per process.

mod atlas;
mod fast;
mod shade;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, bail};
use gpui::{
    AtlasTextureId, Bounds, ClipMask, DevicePixels, Path, PlatformAtlas, PlatformHeadlessRenderer,
    PolychromeSprite, PrimitiveBatch, Scene, ScaledPixels, Size, TransformIndex,
    TransformationMatrix,
};
use tiny_skia::{Color, Pixmap};

pub use atlas::CpuAtlas;
use shade::{Fill, Rgba, RoundedClip, V2, v2};

/// The first texture index of the sprites that stand for a rounded clip.
const ROUNDED_CLIP_TEXTURE_BASE: u32 = 0xF100_0000;
/// The first texture index of effect groups, scroll-layer tiles and dynamic
/// textures: ones that are not tiles of the atlas.
const VIRTUAL_TEXTURE_BASE: u32 = 0xE000_0000;

/// What the renderer left out, said once per process.
fn note_unsupported(what: &'static str) {
    static NOTED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let noted = NOTED.get_or_init(Default::default);
    let first = noted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(what);
    if first {
        log::warn!("the CPU test renderer does not draw {what}");
    }
}

/// A `PlatformHeadlessRenderer` that draws on the CPU.
pub struct CpuHeadlessRenderer {
    atlas: Arc<CpuAtlas>,
    /// Whether the interior of an opaque one-colour primitive is written as
    /// spans of pixels. Always, but for the tests that check the spans against
    /// drawing every pixel.
    spans: bool,
}

static RENDERERS_CREATED: AtomicUsize = AtomicUsize::new(0);

/// How many CPU renderers this process has made: how a test tells that a frame
/// was drawn on the CPU and not by a GPU.
pub fn renderers_created() -> usize {
    RENDERERS_CREATED.load(Ordering::Relaxed)
}

impl CpuHeadlessRenderer {
    pub fn new() -> Self {
        RENDERERS_CREATED.fetch_add(1, Ordering::Relaxed);
        Self {
            atlas: Arc::new(CpuAtlas::default()),
            spans: true,
        }
    }
}

impl CpuHeadlessRenderer {
    fn draw(&mut self, scene: &Scene, mut canvas: Canvas) -> Result<image::RgbaImage> {
        let mut drawing = Drawing {
            atlas: &self.atlas,
            canvas: &mut canvas,
            clips: Vec::new(),
            tiles: HashMap::new(),
            spans: self.spans,
        };
        drawing.scene(scene);
        Ok(canvas.into_image())
    }
}

impl Default for CpuHeadlessRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformHeadlessRenderer for CpuHeadlessRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> Result<image::RgbaImage> {
        let (width, height) = (size.width.0, size.height.0);
        if width <= 0 || height <= 0 {
            bail!("cannot render a {width}x{height} window");
        }
        self.draw(scene, Canvas::new(width as u32, height as u32)?)
    }

    /// Draws only the pixels of `region`: the image is the same as the whole
    /// frame's `region`, and costs what the region does.
    fn render_scene_region_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
        region: Bounds<DevicePixels>,
    ) -> Result<image::RgbaImage> {
        let canvas = crate::fast::crop::canvas(size, region)?;
        self.draw(scene, canvas)
    }

    /// Frames that are never read back are not drawn: a test window hands the
    /// scene over again when it is asked for a screenshot.
    fn render_scene(&mut self, _scene: &Scene, _size: Size<DevicePixels>) -> Result<()> {
        Ok(())
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }
}

/// The frame: tiny-skia's premultiplied RGBA8 pixmap, cleared to opaque black as
/// the Metal headless renderer's is. It holds the pixels of the frame inside
/// `left..left + width` by `top..top + height`, which is all of it unless only
/// a crop of the frame was asked for: the primitives are still placed by the
/// frame's coordinates, and the ones outside the crop are not drawn.
pub(crate) struct Canvas {
    pub(crate) pixmap: Pixmap,
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0., 1.) * 255. + 0.5) as u8
}

/// Blends a straight colour over a pixel's four bytes, as the GPU pipelines do:
/// `source * alpha + destination * (1 - alpha)`.
fn blend_pixel(pixel: &mut [u8], color: Rgba) {
    let alpha = color[3].min(1.);
    if !(alpha > 0.) {
        return;
    }
    for channel in 0..3 {
        let below = pixel[channel] as f32 / 255.;
        pixel[channel] = to_byte(color[channel].clamp(0., 1.) * alpha + below * (1. - alpha));
    }
    let below = pixel[3] as f32 / 255.;
    pixel[3] = to_byte(alpha + below * (1. - alpha));
}

/// Blends a premultiplied colour over a pixel's four bytes.
fn blend_premultiplied_pixel(pixel: &mut [u8], color: Rgba) {
    let alpha = color[3].min(1.);
    if !(alpha > 0.) {
        return;
    }
    for channel in 0..3 {
        let below = pixel[channel] as f32 / 255.;
        pixel[channel] = to_byte(color[channel] + below * (1. - alpha));
    }
    let below = pixel[3] as f32 / 255.;
    pixel[3] = to_byte(alpha + below * (1. - alpha));
}

/// How much of a point the enclosing rounded clips let through.
fn clip_coverage(clips: &[RoundedClip], device: V2) -> f32 {
    clips
        .iter()
        .fold(1., |coverage, clip| coverage.min(clip.coverage(device)))
}

impl Canvas {
    fn new(width: u32, height: u32) -> Result<Self> {
        Self::crop(0, 0, width, height)
    }

    /// A canvas of the `width` by `height` pixels of the frame from (`left`, `top`).
    fn crop(left: i32, top: i32, width: u32, height: u32) -> Result<Self> {
        let Some(mut pixmap) = Pixmap::new(width, height) else {
            bail!("cannot allocate a {width}x{height} frame");
        };
        pixmap.fill(Color::BLACK);
        Ok(Self {
            pixmap,
            left,
            top,
            width: width as i32,
            height: height as i32,
        })
    }

    fn into_image(self) -> image::RgbaImage {
        let (width, height) = (self.width as u32, self.height as u32);
        let mut data = self.pixmap.take();
        for pixel in data.chunks_exact_mut(4) {
            let alpha = pixel[3];
            if alpha != 0 && alpha != 255 {
                for channel in &mut pixel[..3] {
                    *channel = ((*channel as f32 * 255. / alpha as f32) + 0.5).min(255.) as u8;
                }
            }
        }
        image::RgbaImage::from_raw(width, height, data).expect("a pixmap's size matches")
    }

    /// The columns of the canvas whose centres lie in `[low, high)`.
    fn x_span(&self, low: f32, high: f32) -> (i32, i32) {
        Self::span(low, high, self.left, self.left + self.width)
    }

    /// The rows of the canvas whose centres lie in `[low, high)`.
    fn y_span(&self, low: f32, high: f32) -> (i32, i32) {
        Self::span(low, high, self.top, self.top + self.height)
    }

    /// The pixel range inside `[min, max)` whose centres lie in `[low, high)`.
    fn span(low: f32, high: f32, min: i32, max: i32) -> (i32, i32) {
        let start = ((low - 0.5).ceil().max(min as f32)).min(max as f32) as i32;
        let end = ((high - 0.5).ceil().max(min as f32)).min(max as f32) as i32;
        (start, end.max(start))
    }
}

/// A primitive's transform: where device pixels land in the space its bounds
/// and content mask are in.
struct Placement {
    matrix: TransformationMatrix,
    inverse: Option<TransformationMatrix>,
    identity: bool,
}

impl Placement {
    fn new(scene: &Scene, index: TransformIndex) -> Self {
        let identity = index.is_identity();
        let matrix = scene.transforms.get(index);
        Self {
            matrix,
            inverse: if identity { None } else { matrix.inverse() },
            identity,
        }
    }

    /// The position of a device point in the primitive's own space.
    fn local(&self, device: V2) -> Option<V2> {
        if self.identity {
            return Some(device);
        }
        let inverse = self.inverse.as_ref()?;
        let m = inverse.rotation_scale;
        let t = inverse.translation;
        Some(v2(
            device.x * m[0][0] + device.y * m[0][1] + t[0],
            device.x * m[1][0] + device.y * m[1][1] + t[1],
        ))
    }

    /// The device-space box of the primitive's box `low..high`.
    fn device_box(&self, low: V2, high: V2) -> (V2, V2) {
        if self.identity {
            return (low, high);
        }
        let covered = self.matrix.transform_bounds(&Bounds {
            origin: gpui::point(ScaledPixels(low.x), ScaledPixels(low.y)),
            size: gpui::size(ScaledPixels(high.x - low.x), ScaledPixels(high.y - low.y)),
        });
        (
            shade::origin(&covered),
            shade::origin(&covered) + shade::size(&covered),
        )
    }

    /// How far past a primitive's bounds a transformed one covers, so that an
    /// edge antialiased in the fragment stage is not cut off.
    fn margin(&self) -> f32 {
        if self.identity {
            return 0.;
        }
        let m = self.matrix.rotation_scale;
        let determinant = (m[0][0] * m[1][1] - m[0][1] * m[1][0]).abs();
        (1.5 / determinant.max(1e-4).sqrt()).min(16.)
    }
}

struct Drawing<'a> {
    atlas: &'a CpuAtlas,
    canvas: &'a mut Canvas,
    /// The rounded clips of the groups being drawn, outermost first.
    clips: Vec<RoundedClip>,
    /// The layer tiles rasterized so far, by the layer's frame and the tile.
    tiles: HashMap<(usize, gpui::TileCoord), Arc<atlas::Texture>>,
    spans: bool,
}

fn grown(bounds: &Bounds<ScaledPixels>, margin: f32) -> (V2, V2) {
    let low = shade::origin(bounds) - V2::splat(margin);
    let high = shade::origin(bounds) + shade::size(bounds) + V2::splat(margin);
    (low, high)
}

impl Drawing<'_> {

    /// Draws `scene` in draw order.
    fn scene(&mut self, scene: &Scene) {
        for batch in scene.batches() {
            match batch {
                PrimitiveBatch::Shadows(range) => {
                    for shadow in &scene.shadows[range] {
                        self.shadow(scene, shadow);
                    }
                }
                PrimitiveBatch::Quads(range) => {
                    for quad in &scene.quads[range] {
                        self.quad(scene, quad);
                    }
                }
                PrimitiveBatch::Paths(range) => {
                    for path in &scene.paths[range] {
                        self.path(path);
                    }
                }
                PrimitiveBatch::Underlines(range) => {
                    for underline in &scene.underlines[range] {
                        self.underline(scene, underline);
                    }
                }
                PrimitiveBatch::MonochromeSprites { texture_id, range } => {
                    for sprite in &scene.monochrome_sprites[range] {
                        self.monochrome_sprite(scene, texture_id, sprite);
                    }
                }
                PrimitiveBatch::SubpixelSprites { texture_id, range } => {
                    for sprite in &scene.subpixel_sprites[range] {
                        self.subpixel_sprite(scene, texture_id, sprite);
                    }
                }
                PrimitiveBatch::PolychromeSprites { texture_id, range } => {
                    self.polychrome_sprites(scene, texture_id, range);
                }
                PrimitiveBatch::Surfaces(_) => note_unsupported("surfaces"),
                PrimitiveBatch::ExternalTextures(_) => note_unsupported("external textures"),
            }
        }
    }

    /// Runs `shade` for each pixel of a primitive: the ones whose centre is
    /// inside its bounds grown by `margin`, inside the hard rectangle of its
    /// content mask. The result is straight RGBA, to which the transform's
    /// colour filter and the enclosing rounded clips are applied before it is
    /// blended.
    fn raster(
        &mut self,
        scene: &Scene,
        bounds: &Bounds<ScaledPixels>,
        margin: f32,
        transform: TransformIndex,
        mask: &ClipMask<ScaledPixels>,
        shade: impl Fn(V2) -> Rgba + Sync,
    ) {
        self.raster_rows(
            scene,
            bounds,
            margin,
            transform,
            mask,
            None,
            |_| (),
            |_, position| shade(position),
        );
    }

    /// [`Self::raster`] for a shader that has work to do once for each row of
    /// pixels: `prepare` gets the y of a row's pixel centres, in the
    /// primitive's space, and `shade` the result with each pixel's position.
    /// Under a transform no two pixels of a row share a y in that space, and
    /// `prepare` runs for each pixel.
    ///
    /// `opaque` promises that `shade` returns that colour, with an alpha of
    /// one or more, for every pixel that gets as far as it. Where nothing is
    /// then done to the colour (no transform, no colour filter, no rounded
    /// clip), the row's pixels are written as a span.
    fn raster_rows<Row>(
        &mut self,
        scene: &Scene,
        bounds: &Bounds<ScaledPixels>,
        margin: f32,
        transform: TransformIndex,
        mask: &ClipMask<ScaledPixels>,
        opaque: Option<Rgba>,
        prepare: impl Fn(f32) -> Row + Sync,
        shade: impl Fn(&Row, V2) -> Rgba + Sync,
    ) {
        let placement = Placement::new(scene, transform);
        if !placement.identity && placement.inverse.is_none() {
            return;
        }
        let margin = margin.max(placement.margin());
        let (low, high) = grown(bounds, margin);
        let (device_low, device_high) = placement.device_box(low, high);
        let (x0, x1) = self.canvas.x_span(device_low.x, device_high.x);
        let (y0, y1) = self.canvas.y_span(device_low.y, device_high.y);
        let filter = scene.transforms.filter(transform);
        let filtered = !filter.is_identity();
        let clips = &self.clips;
        // What an opaque colour comes to once blended: the pixel below does
        // not show, since the colour's alpha is one.
        let span_bytes = opaque
            .filter(|_| self.spans && placement.identity && !filtered && clips.is_empty())
            .map(|color| {
                let mut bytes = [0; 4];
                blend_pixel(&mut bytes, color);
                bytes
            });
        let x_inside = |x: i32| {
            let centre = x as f32 + 0.5;
            !(centre < low.x || centre >= high.x) && shade::mask_contains_x(centre, mask)
        };
        let y_inside = |y: i32| {
            let centre = y as f32 + 0.5;
            !(centre < low.y || centre >= high.y) && shade::mask_contains_y(centre, mask)
        };
        let mask_x = (
            shade::origin(&mask.bounds).x,
            shade::origin(&mask.bounds).x + shade::size(&mask.bounds).x,
        );
        crate::fast::bands::for_each_row(self.canvas, (x0, x1), (y0, y1), |y, row| {
            if let Some(bytes) = span_bytes {
                if !y_inside(y) {
                    return;
                }
                let estimate = (
                    (low.x.max(mask_x.0) - 0.5).ceil(),
                    (high.x.min(mask_x.1) - 0.5).ceil(),
                );
                if let Some(columns) = crate::fast::spans::columns((x0, x1), estimate, x_inside) {
                    crate::fast::spans::fill(row, x0, columns, bytes);
                    return;
                }
            }
            let mut prepared = None;
            for (pixel, x) in row.chunks_exact_mut(4).zip(x0..x1) {
                let device = v2(x as f32 + 0.5, y as f32 + 0.5);
                let Some(local) = placement.local(device) else {
                    continue;
                };
                if local.x < low.x || local.x >= high.x || local.y < low.y || local.y >= high.y {
                    continue;
                }
                if !shade::inside_mask_rect(local, mask) {
                    continue;
                }
                let mut color = if placement.identity {
                    shade(prepared.get_or_insert_with(|| prepare(local.y)), local)
                } else {
                    shade(&prepare(local.y), local)
                };
                if filtered {
                    color = shade::apply_color_filter(&filter, color);
                }
                color[3] *= clip_coverage(clips, device);
                blend_pixel(pixel, color);
            }
        });
    }

    fn quad(&mut self, scene: &Scene, quad: &gpui::Quad) {
        let fill = Fill::new(&quad.background, quad.bounds);
        let border = shade::hsla_to_rgba(quad.border_color);
        self.raster_rows(
            scene,
            &quad.bounds,
            0.,
            quad.transform_index(),
            &quad.content_mask,
            shade::one_opaque_colour(quad, &fill),
            |_| (),
            |_, position| shade::quad_color(quad, &fill, border, position),
        );
    }

    fn shadow(&mut self, scene: &Scene, shadow: &gpui::Shadow) {
        let color = shade::hsla_to_rgba(shadow.color);
        let (bounds, margin) = if shadow.inset != 0 {
            (shadow.element_bounds, 0.)
        } else {
            (shadow.bounds, 3. * shadow.blur_radius.0)
        };
        self.raster_rows(
            scene,
            &bounds,
            margin,
            TransformIndex(shadow.transform_index),
            &shadow.content_mask,
            None,
            |y| shade::ShadowRow::new(shadow, y),
            |row, position| {
                let alpha = row.alpha(shadow, position.x);
                [color[0], color[1], color[2], color[3] * alpha]
            },
        );
    }

    fn underline(&mut self, scene: &Scene, underline: &gpui::Underline) {
        let color = shade::hsla_to_rgba(underline.color);
        self.raster(
            scene,
            &underline.bounds,
            0.,
            TransformIndex(underline.transform_index),
            &underline.content_mask,
            |position| {
                let alpha = shade::underline_alpha(underline, position);
                [color[0], color[1], color[2], color[3] * alpha]
            },
        );
    }

    /// Where a sprite's bounds put a point in its tile, in texels.
    fn tile_position(
        bounds: &Bounds<ScaledPixels>,
        tile: &gpui::AtlasTile,
        position: V2,
    ) -> (f32, f32) {
        let unit = (position - shade::origin(bounds)) / shade::size(bounds);
        (
            tile.bounds.origin.x.0 as f32 + unit.x * tile.bounds.size.width.0 as f32,
            tile.bounds.origin.y.0 as f32 + unit.y * tile.bounds.size.height.0 as f32,
        )
    }

    fn monochrome_sprite(
        &mut self,
        scene: &Scene,
        texture_id: AtlasTextureId,
        sprite: &gpui::MonochromeSprite,
    ) {
        let Some(texture) = self.atlas.texture(texture_id) else {
            return;
        };
        let color = shade::hsla_to_rgba(sprite.color);
        self.raster(
            scene,
            &sprite.bounds,
            0.,
            TransformIndex(sprite.transform_index),
            &sprite.content_mask,
            |position| {
                let (x, y) = Self::tile_position(&sprite.bounds, &sprite.tile, position);
                let coverage = texture.sample_linear(x, y)[3];
                let mut color = color;
                color[3] *= coverage * shade::content_mask_alpha(position, &sprite.content_mask);
                color
            },
        );
    }

    /// Subpixel sprites carry a coverage per colour channel, so they blend each
    /// channel by its own.
    fn subpixel_sprite(
        &mut self,
        scene: &Scene,
        texture_id: AtlasTextureId,
        sprite: &gpui::SubpixelSprite,
    ) {
        let Some(texture) = self.atlas.texture(texture_id) else {
            return;
        };
        let color = shade::hsla_to_rgba(sprite.color);
        let placement = Placement::new(scene, TransformIndex(sprite.transform_index));
        if !placement.identity {
            // A rotated glyph has no meaningful subpixel layout.
            self.raster(
                scene,
                &sprite.bounds,
                0.,
                TransformIndex(sprite.transform_index),
                &sprite.content_mask,
                |position| {
                    let (x, y) = Self::tile_position(&sprite.bounds, &sprite.tile, position);
                    let mut color = color;
                    color[3] *= texture.sample_linear(x, y)[3]
                        * shade::content_mask_alpha(position, &sprite.content_mask);
                    color
                },
            );
            return;
        }
        let (low, high) = grown(&sprite.bounds, 0.);
        let (x0, x1) = self.canvas.x_span(low.x, high.x);
        let (y0, y1) = self.canvas.y_span(low.y, high.y);
        let clips = &self.clips;
        crate::fast::bands::for_each_row(self.canvas, (x0, x1), (y0, y1), |y, row| {
            for (pixel, x) in row.chunks_exact_mut(4).zip(x0..x1) {
                let position = v2(x as f32 + 0.5, y as f32 + 0.5);
                if !shade::inside_mask_rect(position, &sprite.content_mask) {
                    continue;
                }
                let (tx, ty) = Self::tile_position(&sprite.bounds, &sprite.tile, position);
                let coverage = texture.sample_linear(tx, ty);
                let strength = color[3]
                    * shade::content_mask_alpha(position, &sprite.content_mask)
                    * clip_coverage(clips, position);
                for channel in 0..3 {
                    let below = pixel[channel] as f32 / 255.;
                    let alpha = coverage[channel] * strength;
                    pixel[channel] = to_byte(color[channel] * alpha + below * (1. - alpha));
                }
                let below = pixel[3] as f32 / 255.;
                let alpha = coverage[3] * strength;
                pixel[3] = to_byte(alpha + below * (1. - alpha));
            }
        });
    }

    fn polychrome_sprites(
        &mut self,
        scene: &Scene,
        texture_id: AtlasTextureId,
        range: std::ops::Range<usize>,
    ) {
        if let Some(frame) = rounded_clip_group(scene, texture_id) {
            self.rounded_clip_group(scene, frame, range);
            return;
        }
        if texture_id.index >= gpui::LAYER_TILE_TEXTURE_BASE {
            self.layer_tiles(scene, range);
            return;
        }
        if texture_id.index >= VIRTUAL_TEXTURE_BASE {
            note_unsupported("effect groups and dynamic textures");
            return;
        }
        let Some(texture) = self.atlas.texture(texture_id) else {
            return;
        };
        for sprite in &scene.polychrome_sprites[range] {
            if sprite.mode() >= 4 {
                note_unsupported("blurs and blend modes");
                continue;
            }
            self.polychrome_sprite(scene, &texture, sprite);
        }
    }

    /// The sprites of a scroll layer are its tiles, composited where the
    /// layer is scrolled to. Each tile is rasterized the way the GPU renderers
    /// do: its part of the layer's content, drawn over the layer's background
    /// in a texture of the tile's size, which the sprite then copies.
    fn layer_tiles(&mut self, scene: &Scene, range: std::ops::Range<usize>) {
        for sprite in &scene.polychrome_sprites[range] {
            let Some((key, coord)) = gpui::decode_layer_tile(sprite.tile.texture_id, sprite.tile.tile_id)
            else {
                continue;
            };
            let Some(frame) = scene
                .layers
                .frames
                .iter()
                .find(|frame| frame.key == key && frame.tile_size != 0)
            else {
                note_unsupported("a layer tile whose layer the frame does not carry");
                continue;
            };
            let identity = (frame as *const gpui::LayerFrame as usize, coord);
            let texture = match self.tiles.get(&identity) {
                Some(texture) => texture.clone(),
                None => {
                    // A tile whose sprite is off the canvas (a crop of the
                    // frame) is not drawn: the sprite would draw nothing.
                    if !self.reaches_canvas(scene, &sprite.bounds, TransformIndex(sprite.transform_index)) {
                        continue;
                    }
                    let Some(texture) = self.rasterize_tile(frame, coord) else {
                        continue;
                    };
                    self.tiles.insert(identity, texture.clone());
                    texture
                }
            };
            self.polychrome_sprite(scene, &texture, sprite);
        }
    }

    /// Whether a primitive with these bounds covers any pixel of the canvas.
    fn reaches_canvas(&self, scene: &Scene, bounds: &Bounds<ScaledPixels>, transform: TransformIndex) -> bool {
        let placement = Placement::new(scene, transform);
        if !placement.identity && placement.inverse.is_none() {
            return false;
        }
        let (low, high) = grown(bounds, placement.margin());
        let (device_low, device_high) = placement.device_box(low, high);
        let (x0, x1) = self.canvas.x_span(device_low.x, device_high.x);
        let (y0, y1) = self.canvas.y_span(device_low.y, device_high.y);
        x0 < x1 && y0 < y1
    }

    /// One tile of a layer, drawn into a texture of its own.
    fn rasterize_tile(
        &self,
        frame: &gpui::LayerFrame,
        coord: gpui::TileCoord,
    ) -> Option<Arc<atlas::Texture>> {
        let side = frame.tile_size;
        let mut canvas = Canvas::new(side, side).ok()?;
        let background = frame.background;
        canvas.pixmap.fill(Color::from_rgba(
            background.r.clamp(0., 1.),
            background.g.clamp(0., 1.),
            background.b.clamp(0., 1.),
            background.a.clamp(0., 1.),
        )?);
        let tile_scene = frame.tile_scene(coord);
        Drawing {
            atlas: self.atlas,
            canvas: &mut canvas,
            clips: Vec::new(),
            tiles: HashMap::new(),
            spans: self.spans,
        }
        .scene(&tile_scene);
        // The atlas keeps its pixels as blue, green, red, alpha.
        let mut data = canvas.pixmap.take();
        for pixel in data.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Some(Arc::new(atlas::Texture {
            kind: gpui::AtlasTextureKind::Polychrome,
            width: side as usize,
            height: side as usize,
            data,
        }))
    }

    fn polychrome_sprite(&mut self, scene: &Scene, texture: &atlas::Texture, sprite: &PolychromeSprite) {
        let mode = sprite.mode();
        let grayscale = sprite.grayscale.is_set();
        self.raster(
            scene,
            &sprite.bounds,
            0.,
            TransformIndex(sprite.transform_index),
            &sprite.content_mask,
            |position| {
                let (x, y) = Self::tile_position(&sprite.bounds, &sprite.tile, position);
                let mut color = texture.sample_linear(x, y);
                if mode >= 2 {
                    let alpha = color[3].max(1e-5);
                    for channel in &mut color[..3] {
                        *channel /= alpha;
                    }
                } else if grayscale {
                    let gray = 0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2];
                    color[..3].fill(gray);
                }
                let distance = shade::quad_sdf(position, &sprite.bounds, &sprite.corner_radii);
                color[3] *= sprite.opacity
                    * (0.5 - distance).clamp(0., 1.)
                    * shade::content_mask_alpha(position, &sprite.content_mask);
                color
            },
        );
    }

    /// The sprites of a rounded clip group stand for a child scene drawn through
    /// a rounded rectangle with elliptical corners: the horizontal radii are the
    /// sprite's, the vertical ones the background of the child's geometry frame.
    fn rounded_clip_group(
        &mut self,
        scene: &Scene,
        frame: &gpui::LayerFrame,
        range: std::ops::Range<usize>,
    ) {
        let Some(child) = frame.content.scene() else {
            return;
        };
        let Some(geometry) = child
            .layers
            .frames
            .iter()
            .find(|frame| frame.key.0 == u32::MAX)
        else {
            return;
        };
        let vertical = geometry.background;
        for sprite in &scene.polychrome_sprites[range] {
            let bounds = sprite.bounds;
            let radii = sprite.corner_radii;
            self.clips.push(RoundedClip {
                min: shade::origin(&bounds),
                max: shade::origin(&bounds) + shade::size(&bounds),
                horizontal: [
                    radii.top_left.0,
                    radii.top_right.0,
                    radii.bottom_right.0,
                    radii.bottom_left.0,
                ],
                vertical: [vertical.r, vertical.g, vertical.b, vertical.a],
            });
            self.scene(child);
            self.clips.pop();
        }
    }

    /// `path_rasterization_fragment` and `path_sprite_fragment`: the triangles of
    /// the path are rasterized into premultiplied coverage, curves antialiased
    /// by their quadratic `s * s - t`, then composited through the clips.
    fn path(&mut self, path: &Path<ScaledPixels>) {
        let mask = &path.content_mask;
        let low = shade::origin(&path.bounds).max(shade::origin(&mask.bounds));
        let high = (shade::origin(&path.bounds) + shade::size(&path.bounds))
            .min(shade::origin(&mask.bounds) + shade::size(&mask.bounds));
        let (x0, x1) = self.canvas.x_span(low.x, high.x);
        let (y0, y1) = self.canvas.y_span(low.y, high.y);
        let (width, height) = ((x1 - x0) as usize, (y1 - y0) as usize);
        if width == 0 || height == 0 {
            return;
        }
        let fill = Fill::new(&path.color, path.bounds);
        let mut coverage = vec![[0f32; 4]; width * height];
        for triangle in path.vertices.chunks_exact(3) {
            let corner = |index: usize| {
                let vertex = &triangle[index];
                (
                    v2(vertex.xy_position.x.0, vertex.xy_position.y.0),
                    v2(vertex.st_position.x, vertex.st_position.y),
                )
            };
            let ((a, sa), (mut b, mut sb), (mut c, mut sc)) = (corner(0), corner(1), corner(2));
            let mut area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            if area == 0. || !area.is_finite() {
                continue;
            }
            if area < 0. {
                std::mem::swap(&mut b, &mut c);
                std::mem::swap(&mut sb, &mut sc);
                area = -area;
            }
            let edge = |from: V2, to: V2, point: V2| {
                (to.x - from.x) * (point.y - from.y) - (to.y - from.y) * (point.x - from.x)
            };
            // A pixel centre on an edge belongs to exactly one of the two
            // triangles that share it.
            let owns = |from: V2, to: V2| {
                let (dx, dy) = (to.x - from.x, to.y - from.y);
                dy < 0. || (dy == 0. && dx > 0.)
            };
            let (own_ab, own_bc, own_ca) = (owns(a, b), owns(b, c), owns(c, a));
            // The weights of the vertices are `edge(b, c) / area` and so on, so
            // their derivatives are constants, which are what `dfdx` and `dfdy`
            // return.
            let derivative = |sa: f32, sb: f32, sc: f32, along_x: bool| {
                let (wa, wb, wc) = if along_x {
                    (-(c.y - b.y), -(a.y - c.y), -(b.y - a.y))
                } else {
                    (c.x - b.x, a.x - c.x, b.x - a.x)
                };
                (wa * sa + wb * sb + wc * sc) / area
            };
            let (ds_dx, ds_dy) = (derivative(sa.x, sb.x, sc.x, true), derivative(sa.x, sb.x, sc.x, false));
            let (dt_dx, dt_dy) = (derivative(sa.y, sb.y, sc.y, true), derivative(sa.y, sb.y, sc.y, false));
            let solid = v2(ds_dx, ds_dy).length() < 0.001;

            let min_x = a.x.min(b.x).min(c.x);
            let max_x = a.x.max(b.x).max(c.x);
            let min_y = a.y.min(b.y).min(c.y);
            let max_y = a.y.max(b.y).max(c.y);
            let (tx0, tx1) = Canvas::span(min_x, max_x, x0, x1);
            let (ty0, ty1) = Canvas::span(min_y, max_y, y0, y1);
            for y in ty0..ty1 {
                for x in tx0..tx1 {
                    let point = v2(x as f32 + 0.5, y as f32 + 0.5);
                    let (e_ab, e_bc, e_ca) = (edge(a, b, point), edge(b, c, point), edge(c, a, point));
                    let inside = (e_ab > 0. || (e_ab == 0. && own_ab))
                        && (e_bc > 0. || (e_bc == 0. && own_bc))
                        && (e_ca > 0. || (e_ca == 0. && own_ca));
                    if !inside {
                        continue;
                    }
                    let (wa, wb, wc) = (e_bc / area, e_ca / area, e_ab / area);
                    let alpha = if solid {
                        1.
                    } else {
                        let s = wa * sa.x + wb * sb.x + wc * sc.x;
                        let t = wa * sa.y + wb * sb.y + wc * sc.y;
                        let gradient = v2(2. * s * ds_dx - dt_dx, 2. * s * ds_dy - dt_dy);
                        let distance = (s * s - t) / gradient.length();
                        (0.5 - distance).clamp(0., 1.)
                    };
                    let color = fill.at(point);
                    let alpha = alpha
                        * shade::rounded_clip_alpha(point, &mask.rounded_bounds, &mask.corner_radii)
                        * color[3];
                    let source = [
                        color[0] * alpha,
                        color[1] * alpha,
                        color[2] * alpha,
                        alpha,
                    ];
                    let below = &mut coverage[(y - y0) as usize * width + (x - x0) as usize];
                    for channel in 0..4 {
                        below[channel] = source[channel] + below[channel] * (1. - source[3]);
                    }
                }
            }
        }
        let clips = &self.clips;
        crate::fast::bands::for_each_row(self.canvas, (x0, x1), (y0, y1), |y, row| {
            for (pixel, x) in row.chunks_exact_mut(4).zip(x0..x1) {
                let mut color = coverage[(y - y0) as usize * width + (x - x0) as usize];
                if color[3] <= 0. {
                    continue;
                }
                let device = v2(x as f32 + 0.5, y as f32 + 0.5);
                let clip = clip_coverage(clips, device);
                for channel in &mut color {
                    *channel *= clip;
                }
                blend_premultiplied_pixel(pixel, color);
            }
        });
    }
}

/// The layer frame behind a sprite that stands for a rounded clip group.
fn rounded_clip_group(scene: &Scene, texture: AtlasTextureId) -> Option<&gpui::LayerFrame> {
    if texture.index < ROUNDED_CLIP_TEXTURE_BASE {
        return None;
    }
    let key = texture.index - gpui::LAYER_TILE_TEXTURE_BASE;
    scene
        .layers
        .frames
        .iter()
        .find(|frame| frame.key.0 == key && frame.tile_size == 0)
}
