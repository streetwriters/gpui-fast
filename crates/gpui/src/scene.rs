// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTextureKind, AtlasTile, Background, Bounds, ContentMask, Corners, Edges,
    ExternalTexture, Hsla, Pixels, Point, Radians, ScaledPixels, Size, TileId,
    bounds_tree::BoundsTree, point,
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
};

#[allow(non_camel_case_types, unused)]
#[expect(missing_docs)]
pub type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

#[expect(missing_docs)]
pub type DrawOrder = u32;

/// A boolean stored as a `u32` so that GPU-facing structs contain no
/// compiler-inserted padding bytes, which would be undefined behavior to
/// reinterpret as `&[u8]` when writing instance buffers. Guaranteed to be
/// `0` or `1` by construction; shaders read it as a `u32`/`uint`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct PaddedBool32(pub(crate) u32);

impl From<bool> for PaddedBool32 {
    fn from(value: bool) -> Self {
        PaddedBool32(value as u32)
    }
}

/// What [`Scene::begin_effect_group`] returns for [`Scene::end_effect_group`].
#[derive(Clone, Copy, Debug)]
pub struct EffectGroupScope {
    previous: TransformIndex,
    id: u32,
}

/// An index into a scene's transform table. Index 0 is the identity.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TransformIndex(pub u32);

impl TransformIndex {
    /// The identity transform, which every scene's table starts with.
    pub const IDENTITY: TransformIndex = TransformIndex(0);

    /// Whether this is the identity.
    pub fn is_identity(self) -> bool {
        self.0 == 0
    }
}

/// A colour matrix applied to what a primitive paints, in the form of an SVG
/// `feColorMatrix`: each row gives one of red, green, blue and alpha as a
/// weighted sum of the four and an offset, on unpremultiplied colour.
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct ColorFilter {
    /// Rows for red, green, blue and alpha; columns r, g, b, a, offset.
    pub rows: [[f32; 5]; 4],
}

impl ColorFilter {
    /// The filter that leaves colour alone.
    pub const IDENTITY: ColorFilter = ColorFilter {
        rows: [
            [1., 0., 0., 0., 0.],
            [0., 1., 0., 0., 0.],
            [0., 0., 1., 0., 0.],
            [0., 0., 0., 1., 0.],
        ],
    };

    /// Whether this leaves colour alone.
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// The filter that applies `first`, then `self`.
    pub fn after(&self, first: &ColorFilter) -> ColorFilter {
        let mut rows = [[0.; 5]; 4];
        for (row, out) in self.rows.iter().zip(rows.iter_mut()) {
            for column in 0..5 {
                let mut sum = 0.;
                for k in 0..4 {
                    sum += row[k] * first.rows[k][column];
                }
                if column == 4 {
                    sum += row[4];
                }
                out[column] = sum;
            }
        }
        ColorFilter { rows }
    }

    fn bits(&self) -> [u32; 20] {
        let mut bits = [0; 20];
        for (i, value) in self.rows.iter().flatten().enumerate() {
            bits[i] = value.to_bits();
        }
        bits
    }
}

impl Default for ColorFilter {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// The transforms and colour filters a scene's primitives are drawn under,
/// uploaded to the GPU once per frame. Entries are absolute: each is the
/// composition of every transform (and filter) from the window down to the
/// primitive, so a shader reads one.
#[derive(Debug, Clone)]
pub struct TransformTable {
    entries: Vec<TransformationMatrix>,
    filters: Vec<ColorFilter>,
    /// The effect group each entry's primitives belong to; 0 for none. Not
    /// uploaded: the renderer draws a group's primitives into the group's own
    /// texture.
    groups: Vec<u32>,
    lookup: std::collections::HashMap<[u32; 27], u32>,
    filtered: bool,
}

impl Default for TransformTable {
    fn default() -> Self {
        Self {
            entries: vec![TransformationMatrix::unit()],
            filters: vec![ColorFilter::IDENTITY],
            groups: vec![0],
            lookup: std::collections::HashMap::new(),
            filtered: false,
        }
    }
}

impl TransformTable {
    /// The entries, with the identity first.
    pub fn entries(&self) -> &[TransformationMatrix] {
        &self.entries
    }

    /// The colour filters, one per entry.
    pub fn filters(&self) -> &[ColorFilter] {
        &self.filters
    }

    /// Whether any entry has a colour filter.
    pub fn has_filters(&self) -> bool {
        self.filtered
    }

    /// The matrix at `index`; the identity for an index the table does not hold.
    pub fn get(&self, index: TransformIndex) -> TransformationMatrix {
        self.entries
            .get(index.0 as usize)
            .copied()
            .unwrap_or_else(TransformationMatrix::unit)
    }

    /// The colour filter at `index`; the identity for an index the table does
    /// not hold.
    pub fn filter(&self, index: TransformIndex) -> ColorFilter {
        self.filters
            .get(index.0 as usize)
            .copied()
            .unwrap_or(ColorFilter::IDENTITY)
    }

    /// The effect group `index` belongs to; 0 for none.
    /// Whether the entry at `index` moves nothing.
    pub(crate) fn is_unit(&self, index: TransformIndex) -> bool {
        self.entries[index.0 as usize] == TransformationMatrix::unit()
    }

    pub fn group(&self, index: TransformIndex) -> u32 {
        self.groups.get(index.0 as usize).copied().unwrap_or(0)
    }

    /// The index of `matrix`, adding it if the table does not hold it yet.
    pub fn intern(&mut self, matrix: TransformationMatrix) -> TransformIndex {
        self.intern_with(matrix, ColorFilter::IDENTITY)
    }

    /// The index of `matrix` with `filter`, adding it if the table does not
    /// hold it yet.
    pub fn intern_with(
        &mut self,
        matrix: TransformationMatrix,
        filter: ColorFilter,
    ) -> TransformIndex {
        self.intern_in_group(matrix, filter, 0)
    }

    /// Like [`Self::intern_with`], for primitives of effect group `group`.
    pub fn intern_in_group(
        &mut self,
        matrix: TransformationMatrix,
        filter: ColorFilter,
        group: u32,
    ) -> TransformIndex {
        if matrix == TransformationMatrix::unit() && filter.is_identity() && group == 0 {
            return TransformIndex::IDENTITY;
        }
        let mut key = [0u32; 27];
        key[..6].copy_from_slice(&matrix.bits());
        key[6..26].copy_from_slice(&filter.bits());
        key[26] = group;
        if let Some(index) = self.lookup.get(&key) {
            return TransformIndex(*index);
        }
        let index = self.entries.len() as u32;
        self.entries.push(matrix);
        self.filters.push(filter);
        self.groups.push(group);
        self.filtered |= !filter.is_identity();
        self.lookup.insert(key, index);
        TransformIndex(index)
    }

    fn clear(&mut self) {
        self.entries.truncate(1);
        self.filters.truncate(1);
        self.groups.truncate(1);
        self.lookup.clear();
        self.filtered = false;
    }
}

#[derive(Default)]
#[expect(missing_docs)]
pub struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    pub(crate) primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<DrawOrder>,
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<Path<ScaledPixels>>,
    pub underlines: Vec<Underline>,
    pub monochrome_sprites: Vec<MonochromeSprite>,
    pub subpixel_sprites: Vec<SubpixelSprite>,
    pub polychrome_sprites: Vec<PolychromeSprite>,
    pub surfaces: Vec<PaintSurface>,
    pub external_textures: Vec<PaintExternalTexture>,
    pub(crate) sort_scratch: crate::fast::scene::SortScratch,
    pub layers: crate::fast::layers::scene::SceneLayers,
    pub transforms: TransformTable,
    current_transform: TransformIndex,
    /// The effect groups of the frame, by id - 1. See [`Scene::begin_effect_group`].
    pub groups: Vec<EffectGroup>,
    /// What each group draws, split out of the scene by [`Scene::finish`]:
    /// finished scenes that share this one's transform table.
    pub group_scenes: Vec<Scene>,
    /// The blur passes of each group in `group_scenes`, in order.
    pub group_blurs: Vec<Vec<BlurPass>>,
}

/// A subtree painted into a texture of its own and composited with `opacity`.
#[derive(Clone, Debug)]
pub struct EffectGroup {
    /// What the subtree painted, in device pixels.
    pub bounds: Bounds<ScaledPixels>,
    /// How opaque the composited group is.
    pub opacity: f32,
    /// What is done to the texture before it is composited.
    pub effect: GroupEffect,
    /// How the composited picture is combined with what is beneath it.
    pub blend: BlendMode,
}

/// How a group's picture is combined with the backdrop beneath it, as in
/// CSS compositing's `mix-blend-mode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(missing_docs)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
    PlusLighter,
}

/// What an effect group does to its picture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum GroupEffect {
    /// Nothing: the picture is composited as it is.
    #[default]
    None,
    /// Nothing, but the content is painted as a group of its own, so that
    /// blend modes inside it see only what is inside it.
    Isolate,
    /// A gaussian blur of standard deviation `sigma`, in device pixels.
    Blur {
        /// The standard deviation.
        sigma: f32,
    },
    /// A blurred silhouette of the picture, in `color`, drawn `(dx, dy)` away
    /// beneath it.
    DropShadow {
        /// Device pixels to the right.
        dx: f32,
        /// Device pixels down.
        dy: f32,
        /// The standard deviation of the blur.
        sigma: f32,
        /// The shadow's straight RGBA colour.
        color: [f32; 4],
    },
}

impl GroupEffect {
    /// How far past the picture's bounds the effect reaches, in device pixels.
    fn margin(&self) -> f32 {
        match self {
            GroupEffect::None | GroupEffect::Isolate => 0.0,
            GroupEffect::Blur { sigma } | GroupEffect::DropShadow { sigma, .. } => {
                (sigma * 3.0).ceil()
            }
        }
    }

    /// The blur's standard deviation, if the picture is blurred.
    pub fn sigma(&self) -> Option<f32> {
        match self {
            GroupEffect::None | GroupEffect::Isolate => None,
            GroupEffect::Blur { sigma } | GroupEffect::DropShadow { sigma, .. } => {
                (*sigma > 0.0).then_some(*sigma)
            }
        }
    }
}

/// A pass over an effect group's texture that blurs it along one axis: a
/// scene of one sprite that reads texture `from` of the group and is drawn
/// into texture `to`.
pub struct BlurPass {
    /// The slot of the group texture read.
    pub from: u8,
    /// The slot drawn into.
    pub to: u8,
    /// A finished scene of the one sprite.
    pub scene: Scene,
}

/// The slot of a group's texture holding what the group painted.
pub const GROUP_CONTENT: u8 = 0;
/// The slot holding the blurred picture of a group with a drop shadow.
pub const GROUP_BLURRED: u8 = 1;
/// The slot a blur's first pass draws into.
pub const GROUP_SCRATCH: u8 = 2;

/// The first texture index of effect groups; below the scroll layer tiles.
pub const EFFECT_GROUP_TEXTURE_BASE: u32 = 0xE000_0000;

/// The texture id of `slot` of effect group `group`'s textures.
pub fn effect_group_texture_id(group: u32, slot: u8) -> AtlasTextureId {
    AtlasTextureId {
        index: EFFECT_GROUP_TEXTURE_BASE + group * 4 + slot as u32,
        kind: AtlasTextureKind::Polychrome,
    }
}

/// The group and slot a sprite's texture stands for, if it is one.
pub fn decode_effect_group(texture: AtlasTextureId) -> Option<(u32, u8)> {
    (texture.kind == AtlasTextureKind::Polychrome
        && (EFFECT_GROUP_TEXTURE_BASE..crate::fast::layers::scene::LAYER_TILE_TEXTURE_BASE)
            .contains(&texture.index))
    .then(|| {
        let offset = texture.index - EFFECT_GROUP_TEXTURE_BASE;
        (offset / 4, (offset % 4) as u8)
    })
}

/// The low byte of a sprite's `grayscale` word when its texture holds
/// premultiplied colour, which the shader divides by alpha before blending.
pub const PREMULTIPLIED_SPRITE: u32 = 2;
/// The low byte of a sprite that blurs its texture horizontally; the rest of
/// the word is the standard deviation in sixteenths of a pixel.
pub const BLUR_HORIZONTAL_SPRITE: u32 = 4;
/// Like [`BLUR_HORIZONTAL_SPRITE`], vertically.
pub const BLUR_VERTICAL_SPRITE: u32 = 5;
/// The low byte of a sprite that is combined with the backdrop under it by a
/// blend mode; the rest of the word is the mode's index. The texture holds
/// premultiplied colour.
pub const BLEND_SPRITE: u32 = 6;

#[expect(missing_docs)]
impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.subpixel_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
        self.external_textures.clear();
        self.transforms.clear();
        self.current_transform = TransformIndex::IDENTITY;
        self.groups.clear();
        self.group_scenes.clear();
        self.group_blurs.clear();
        crate::fast::layers::scene::SceneLayers::clear(&mut self.layers);
    }

    /// The transform primitives inserted now are drawn under.
    pub fn current_transform(&self) -> TransformIndex {
        self.current_transform
    }

    /// Draws what is inserted next under `matrix` applied in the current
    /// transform's space. Returns the previous index for [`Self::pop_transform`].
    pub fn push_transform(&mut self, matrix: TransformationMatrix) -> TransformIndex {
        let previous = self.current_transform;
        let composed = self.transforms.get(previous).compose(matrix);
        let filter = self.transforms.filter(previous);
        let group = self.transforms.group(previous);
        self.current_transform = self.transforms.intern_in_group(composed, filter, group);
        previous
    }

    /// Colours what is inserted next through `filter`, after any filter already
    /// in force. Returns the previous index for [`Self::pop_transform`].
    pub fn push_color_filter(&mut self, filter: ColorFilter) -> TransformIndex {
        let previous = self.current_transform;
        let combined = self.transforms.filter(previous).after(&filter);
        let matrix = self.transforms.get(previous);
        let group = self.transforms.group(previous);
        self.current_transform = self.transforms.intern_in_group(matrix, combined, group);
        previous
    }

    /// Restores the transform [`Self::push_transform`] returned.
    pub fn pop_transform(&mut self, previous: TransformIndex) {
        self.current_transform = previous;
    }

    /// Starts an effect group: what is inserted until [`Self::end_effect_group`]
    /// is painted into a texture of its own and composited with `opacity`, so
    /// overlapping content inside it is blended as one image.
    pub fn begin_effect_group(&mut self, opacity: f32) -> EffectGroupScope {
        self.begin_effect_group_with(opacity, GroupEffect::None)
    }

    /// Like [`Self::begin_effect_group`], doing `effect` to the picture before
    /// it is composited.
    pub fn begin_effect_group_with(
        &mut self,
        opacity: f32,
        effect: GroupEffect,
    ) -> EffectGroupScope {
        self.begin_effect_group_blended(opacity, effect, BlendMode::Normal)
    }

    /// Like [`Self::begin_effect_group_with`], combining the picture with the
    /// backdrop by `blend` when it is composited.
    pub fn begin_effect_group_blended(
        &mut self,
        opacity: f32,
        effect: GroupEffect,
        blend: BlendMode,
    ) -> EffectGroupScope {
        let previous = self.current_transform;
        self.groups.push(EffectGroup {
            bounds: Bounds::default(),
            opacity,
            effect,
            blend,
        });
        let id = self.groups.len() as u32;
        // The colour filter in force outside colours the composited picture,
        // not what is painted into it.
        self.current_transform = self.transforms.intern_in_group(
            self.transforms.get(previous),
            ColorFilter::IDENTITY,
            id,
        );
        EffectGroupScope { previous, id }
    }

    /// Ends the group `scope` started and inserts what composites it, above
    /// everything the group painted and what overlaps its bounds. The group's
    /// texture covers its bounds, grown by what the effect reaches, rounded
    /// out to whole device pixels and clipped to `window`, so it is never
    /// larger than the window nor sampled at another scale than the device's.
    pub fn end_effect_group(&mut self, scope: EffectGroupScope, window: Bounds<ScaledPixels>) {
        self.current_transform = scope.previous;
        let record = &self.groups[scope.id as usize - 1];
        let (opacity, effect, blend) = (record.opacity, record.effect, record.blend);
        if record.bounds.is_empty() {
            return;
        }
        let margin = effect.margin();
        let drawn = record
            .bounds
            .dilate(ScaledPixels(margin))
            .intersect(&window);
        let bounds = Bounds::from_corners(
            point(
                ScaledPixels(drawn.origin.x.0.floor()),
                ScaledPixels(drawn.origin.y.0.floor()),
            ),
            point(
                ScaledPixels((drawn.origin.x.0 + drawn.size.width.0).ceil()),
                ScaledPixels((drawn.origin.y.0 + drawn.size.height.0).ceil()),
            ),
        );
        self.groups[scope.id as usize - 1].bounds = bounds;
        if bounds.is_empty() {
            return;
        }
        let group = self.transforms.group(scope.previous);
        let outer = self.transforms.filter(scope.previous);
        let sprite = |slot: u8, bounds: Bounds<ScaledPixels>, opacity: f32| PolychromeSprite {
            order: 0,
            transform_index: 0,
            grayscale: PaddedBool32(PREMULTIPLIED_SPRITE),
            opacity,
            bounds,
            content_mask: ContentMask::new(bounds),
            corner_radii: Corners::default(),
            tile: AtlasTile {
                texture_id: effect_group_texture_id(scope.id, slot),
                tile_id: TileId(scope.id),
                padding: 0,
                bounds: Bounds {
                    origin: Point::default(),
                    size: crate::size(
                        crate::DevicePixels(bounds.size.width.0 as i32),
                        crate::DevicePixels(bounds.size.height.0 as i32),
                    ),
                },
            },
        };
        if let GroupEffect::DropShadow { dx, dy, color, .. } = effect {
            // The shadow is the blurred picture, in one colour: a matrix that
            // sends every colour to `color` and keeps the alpha.
            let tint = ColorFilter {
                rows: [
                    [0., 0., 0., 0., color[0]],
                    [0., 0., 0., 0., color[1]],
                    [0., 0., 0., 0., color[2]],
                    [0., 0., 0., color[3], 0.],
                ],
            };
            let transform = self.transforms.intern_in_group(
                TransformationMatrix::unit(),
                outer.after(&tint),
                group,
            );
            let moved = Bounds {
                origin: point(
                    ScaledPixels(bounds.origin.x.0 + dx),
                    ScaledPixels(bounds.origin.y.0 + dy),
                ),
                size: bounds.size,
            };
            let mut shadow = sprite(GROUP_BLURRED, moved, opacity);
            shadow.tile.bounds.origin = Point::default();
            self.insert_under(shadow.into(), transform, false);
        }
        let transform = self
            .transforms
            .intern_in_group(TransformationMatrix::unit(), outer, group);
        let mut content = sprite(GROUP_CONTENT, bounds, opacity);
        if blend != BlendMode::Normal {
            content.grayscale = PaddedBool32(BLEND_SPRITE | (blend as u32) << 8);
        }
        self.insert_under(content.into(), transform, false);
    }

    /// The passes that draw the frame's effect groups, in the order they
    /// must run: innermost group first, and for each group the pass that
    /// paints it and then the passes that blur it. Each is the group, the slot
    /// of its texture the pass draws into, and the scene it draws.
    pub fn effect_passes(&self) -> impl Iterator<Item = (u32, u8, &Scene)> + '_ {
        self.group_scenes
            .iter()
            .enumerate()
            .rev()
            .flat_map(|(index, painted)| {
                let id = index as u32 + 1;
                std::iter::once((id, GROUP_CONTENT, painted)).chain(
                    self.group_blurs
                        .get(index)
                        .into_iter()
                        .flatten()
                        .map(move |pass| (id, pass.to, &pass.scene)),
                )
            })
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.primitive_bounds.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        self.insert_under(primitive.into(), self.current_transform, true);
    }

    /// Inserts `primitive` under the transform at `transform`. A path is
    /// transformed on the CPU when `transform_paths` is set; one replayed from
    /// an earlier scene already is.
    fn insert_under(
        &mut self,
        mut primitive: Primitive,
        transform: TransformIndex,
        transform_paths: bool,
    ) {
        let local_bounds = primitive
            .bounds()
            .intersect(&primitive.content_mask().bounds);

        if local_bounds.is_empty() {
            return;
        }
        // Order and damage are decided by where the primitive lands on screen.
        let clipped_bounds = if transform.is_identity() {
            local_bounds
        } else {
            self.transforms
                .get(transform)
                .transform_bounds(&local_bounds)
        };
        primitive.set_transform_index(transform);
        let group = self.transforms.group(transform);
        if group != 0 {
            let record = &mut self.groups[group as usize - 1];
            record.bounds = if record.bounds.is_empty() {
                clipped_bounds
            } else {
                record.bounds.union(&clipped_bounds)
            };
        }
        if transform_paths && !transform.is_identity() {
            if let Primitive::Path(path) = &mut primitive {
                path.apply_transform(&self.transforms.get(transform));
            }
        }

        let order = self
            .layer_stack
            .last()
            .copied()
            .unwrap_or_else(|| self.primitive_bounds.insert(clipped_bounds));
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(*shadow);
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(*quad);
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                path.group = group;
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(*underline);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(*sprite);
            }
            Primitive::SubpixelSprite(sprite) => {
                sprite.order = order;
                self.subpixel_sprites.push(*sprite);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(*sprite);
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
            Primitive::ExternalTexture(texture) => {
                texture.order = order;
                self.external_textures.push(texture.clone());
            }
        }
        self.paint_operations
            .push(PaintOperation::Primitive(primitive));
    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        crate::fast::layers::paint::replay_layers(self, range.clone(), prev_scene);
        let mut carried = (TransformIndex::IDENTITY, TransformIndex::IDENTITY);
        // The previous frame's effect groups, by their id in this one.
        let mut regrouped: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        // The groups whose pictures are composited inside the range. Any other
        // group a replayed primitive was in encloses the range, and is
        // whichever group encloses it now.
        let enclosed: std::collections::HashSet<u32> = prev_scene.paint_operations[range.clone()]
            .iter()
            .filter_map(|operation| match operation {
                PaintOperation::Primitive(Primitive::PolychromeSprite(sprite)) => {
                    decode_effect_group(sprite.tile.texture_id).map(|(group, _)| group)
                }
                _ => None,
            })
            .collect();
        let enclosing = self.transforms.group(self.current_transform);
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    // The previous scene's table holds absolute transforms and
                    // filters, so they are carried over, not composed with the
                    // current ones.
                    let mut primitive = primitive.clone();
                    let index = primitive.transform_index();
                    let old_group = match &primitive {
                        Primitive::Path(path) => path.group,
                        _ => prev_scene.transforms.group(index),
                    };
                    let mut regroup = |scene: &mut Scene, old: u32| {
                        *regrouped.entry(old).or_insert_with(|| {
                            let old = &prev_scene.groups[old as usize - 1];
                            // The group's final bounds, which the replayed
                            // primitives lie within: the sprite that
                            // composites it was made with them.
                            scene.groups.push(EffectGroup {
                                bounds: old.bounds,
                                opacity: old.opacity,
                                effect: old.effect,
                                blend: old.blend,
                            });
                            scene.groups.len() as u32
                        })
                    };
                    let group = if enclosed.contains(&old_group) {
                        regroup(self, old_group)
                    } else {
                        enclosing
                    };
                    // The sprite that composites a group points at the group's
                    // texture, which is numbered anew.
                    if let Primitive::PolychromeSprite(sprite) = &mut primitive {
                        if let Some((own, slot)) = decode_effect_group(sprite.tile.texture_id) {
                            let renumbered = regroup(self, own);
                            sprite.tile.texture_id = effect_group_texture_id(renumbered, slot);
                            sprite.tile.tile_id = TileId(renumbered);
                        }
                    }
                    if group == 0 && index != carried.0 || group != 0 {
                        carried = (
                            index,
                            self.transforms.intern_in_group(
                                prev_scene.transforms.get(index),
                                prev_scene.transforms.filter(index),
                                group,
                            ),
                        );
                    }
                    self.insert_under(primitive, carried.1, false);
                }
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::EndLayer => self.pop_layer(),
            }
        }
    }

    pub fn finish(&mut self) {
        crate::fast::effects::split_groups(self);
        crate::fast::scene::sort_in_drawing_order(self);
        for sub in &mut self.group_scenes {
            crate::fast::scene::sort_in_drawing_order(sub);
        }
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub fn batches(&self) -> impl Iterator<Item = PrimitiveBatch> + '_ {
        BatchIterator {
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            subpixel_sprites_start: 0,
            subpixel_sprites_iter: self.subpixel_sprites.iter().peekable(),
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
            external_textures_start: 0,
            external_textures_iter: self.external_textures.iter().peekable(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    SubpixelSprite,
    PolychromeSprite,
    Surface,
    ExternalTexture,
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
}

#[derive(Clone)]
#[expect(missing_docs)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    SubpixelSprite(SubpixelSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
    ExternalTexture(PaintExternalTexture),
}

#[expect(missing_docs)]
impl Primitive {
    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::SubpixelSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
            Primitive::ExternalTexture(texture) => &texture.bounds,
        }
    }

    /// The transform the primitive is drawn under. Paths, surfaces and
    /// external textures carry none: a path is transformed when inserted.
    pub fn transform_index(&self) -> TransformIndex {
        match self {
            Primitive::Shadow(shadow) => TransformIndex(shadow.transform_index),
            Primitive::Quad(quad) => quad.transform_index(),
            Primitive::Underline(underline) => TransformIndex(underline.transform_index),
            Primitive::MonochromeSprite(sprite) => TransformIndex(sprite.transform_index),
            Primitive::SubpixelSprite(sprite) => TransformIndex(sprite.transform_index),
            Primitive::PolychromeSprite(sprite) => TransformIndex(sprite.transform_index),
            Primitive::Path(_) | Primitive::Surface(_) | Primitive::ExternalTexture(_) => {
                TransformIndex::IDENTITY
            }
        }
    }

    fn set_transform_index(&mut self, index: TransformIndex) {
        match self {
            Primitive::Shadow(shadow) => shadow.transform_index = index.0,
            Primitive::Quad(quad) => quad.set_transform_index(index),
            Primitive::Underline(underline) => underline.transform_index = index.0,
            Primitive::MonochromeSprite(sprite) => sprite.transform_index = index.0,
            Primitive::SubpixelSprite(sprite) => sprite.transform_index = index.0,
            Primitive::PolychromeSprite(sprite) => sprite.transform_index = index.0,
            Primitive::Path(_) => {}
            Primitive::Surface(_) | Primitive::ExternalTexture(_) => {
                if !index.is_identity() {
                    log::debug!(
                        "a surface or external texture is drawn axis-aligned under a transform"
                    );
                }
            }
        }
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::SubpixelSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
            Primitive::ExternalTexture(texture) => &texture.content_mask,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    subpixel_sprites_start: usize,
    subpixel_sprites_iter: Peekable<slice::Iter<'a, SubpixelSprite>>,
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
    external_textures_start: usize,
    external_textures_iter: Peekable<slice::Iter<'a, PaintExternalTexture>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.subpixel_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::SubpixelSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
            (
                self.external_textures_iter.peek().map(|t| t.order),
                PrimitiveKind::ExternalTexture,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(shadows_start..shadows_end))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(quads_start..quads_end))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(paths_start..paths_end))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(underlines_start..underlines_end))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::SubpixelSprite => {
                let texture_id = self.subpixel_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.subpixel_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.subpixel_sprites_iter.next();
                while self
                    .subpixel_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.subpixel_sprites_start = sprites_end;
                Some(PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(surfaces_start..surfaces_end))
            }
            PrimitiveKind::ExternalTexture => {
                let start = self.external_textures_start;
                let mut end = start + 1;
                self.external_textures_iter.next();
                while self
                    .external_textures_iter
                    .next_if(|texture| (texture.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    end += 1;
                }
                self.external_textures_start = end;
                Some(PrimitiveBatch::ExternalTextures(start..end))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
#[allow(missing_docs)]
pub enum PrimitiveBatch {
    Shadows(Range<usize>),
    Quads(Range<usize>),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    SubpixelSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    Surfaces(Range<usize>),
    ExternalTextures(Range<usize>),
}

impl PrimitiveBatch {
    #[expect(missing_docs)]
    pub fn label(&self) -> String {
        match self {
            Self::Shadows(range) => format!("shadows ({})", range.len()),
            Self::Quads(range) => format!("quads ({})", range.len()),
            Self::Paths(range) => format!("paths ({})", range.len()),
            Self::Underlines(range) => format!("underlines ({})", range.len()),
            Self::MonochromeSprites { texture_id, range } => {
                format!(
                    "monochrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::SubpixelSprites { texture_id, range } => {
                format!(
                    "subpixel sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::PolychromeSprites { texture_id, range } => {
                format!(
                    "polychrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::Surfaces(range) => format!("surfaces ({})", range.len()),
            Self::ExternalTextures(range) => format!("external textures ({})", range.len()),
        }
    }
}

#[derive(Default, Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Quad {
    pub order: DrawOrder,
    /// The border style in the low 8 bits and the transform index in the
    /// high 24. Use [`Quad::border_style`] and [`Quad::transform_index`].
    pub border_style_and_transform: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl Quad {
    /// The word packing a border style with a transform index.
    pub fn pack_border_style(style: BorderStyle, transform: TransformIndex) -> u32 {
        (style as u32) | (transform.0 << 8)
    }

    /// The style of the border.
    pub fn border_style(&self) -> BorderStyle {
        match self.border_style_and_transform & 0xff {
            1 => BorderStyle::Dashed,
            _ => BorderStyle::Solid,
        }
    }

    /// Sets the style of the border.
    pub fn set_border_style(&mut self, style: BorderStyle) {
        self.border_style_and_transform = Self::pack_border_style(style, self.transform_index());
    }

    /// The transform the quad is drawn under.
    pub fn transform_index(&self) -> TransformIndex {
        TransformIndex(self.border_style_and_transform >> 8)
    }

    /// Sets the transform the quad is drawn under.
    pub fn set_transform_index(&mut self, index: TransformIndex) {
        self.border_style_and_transform = Self::pack_border_style(self.border_style(), index);
    }
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Underline {
    pub order: DrawOrder,
    pub transform_index: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: PaddedBool32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub element_bounds: Bounds<ScaledPixels>,
    pub element_corner_radii: Corners<ScaledPixels>,
    /// 0 = drop shadow (rendered outside the element), 1 = inset shadow (rendered inside).
    pub inset: u32,
    pub transform_index: u32,
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(mut self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// The matrix as the bits of its six numbers, for use as a key.
    pub fn bits(&self) -> [u32; 6] {
        [
            self.rotation_scale[0][0].to_bits(),
            self.rotation_scale[0][1].to_bits(),
            self.rotation_scale[1][0].to_bits(),
            self.rotation_scale[1][1].to_bits(),
            self.translation[0].to_bits(),
            self.translation[1].to_bits(),
        ]
    }

    /// Whether this is the unit matrix.
    pub fn is_unit(&self) -> bool {
        *self == Self::unit()
    }

    /// Where the matrix puts a point.
    pub fn transform_point(&self, point: Point<ScaledPixels>) -> Point<ScaledPixels> {
        let (x, y) = (point.x.0, point.y.0);
        crate::point(
            ScaledPixels(
                self.rotation_scale[0][0] * x + self.rotation_scale[0][1] * y + self.translation[0],
            ),
            ScaledPixels(
                self.rotation_scale[1][0] * x + self.rotation_scale[1][1] * y + self.translation[1],
            ),
        )
    }

    /// The smallest axis-aligned rectangle holding `bounds` after the matrix.
    pub fn transform_bounds(&self, bounds: &Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
        let corners = [
            bounds.origin,
            bounds.top_right(),
            bounds.bottom_left(),
            bounds.bottom_right(),
        ];
        let mapped = corners.map(|corner| self.transform_point(corner));
        let min_x = mapped.iter().map(|p| p.x.0).fold(f32::INFINITY, f32::min);
        let min_y = mapped.iter().map(|p| p.y.0).fold(f32::INFINITY, f32::min);
        let max_x = mapped
            .iter()
            .map(|p| p.x.0)
            .fold(f32::NEG_INFINITY, f32::max);
        let max_y = mapped
            .iter()
            .map(|p| p.y.0)
            .fold(f32::NEG_INFINITY, f32::max);
        Bounds {
            origin: crate::point(ScaledPixels(min_x), ScaledPixels(min_y)),
            size: crate::size(ScaledPixels(max_x - min_x), ScaledPixels(max_y - min_y)),
        }
    }

    /// The matrix that undoes this one, or `None` when it flattens the plane.
    pub fn inverse(&self) -> Option<TransformationMatrix> {
        let [[a, b], [c, d]] = self.rotation_scale;
        let determinant = a * d - b * c;
        if determinant.abs() < f32::EPSILON || !determinant.is_finite() {
            return None;
        }
        let (ia, ib, ic, id) = (
            d / determinant,
            -b / determinant,
            -c / determinant,
            a / determinant,
        );
        Some(TransformationMatrix {
            rotation_scale: [[ia, ib], [ic, id]],
            translation: [
                -(ia * self.translation[0] + ib * self.translation[1]),
                -(ic * self.translation[0] + id * self.translation[1]),
            ],
        })
    }

    /// `matrix` applied about the point `(x, y)`: the point stays where it is.
    pub fn about(x: f32, y: f32, matrix: TransformationMatrix) -> TransformationMatrix {
        let toward = TransformationMatrix {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [x, y],
        };
        let back = TransformationMatrix {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [-x, -y],
        };
        toward.compose(matrix).compose(back)
    }

    /// Where the matrix puts a point in logical pixels.
    pub fn transform_logical_point(&self, point: Point<Pixels>) -> Point<Pixels> {
        let (x, y) = (point.x.0, point.y.0);
        crate::point(
            Pixels(
                self.rotation_scale[0][0] * x + self.rotation_scale[0][1] * y + self.translation[0],
            ),
            Pixels(
                self.rotation_scale[1][0] * x + self.rotation_scale[1][1] * y + self.translation[1],
            ),
        )
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct MonochromeSprite {
    pub order: DrawOrder,
    pub transform_index: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct SubpixelSprite {
    pub order: DrawOrder,
    pub transform_index: u32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
}

impl From<SubpixelSprite> for Primitive {
    fn from(sprite: SubpixelSprite) -> Self {
        Primitive::SubpixelSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PolychromeSprite {
    pub order: DrawOrder,
    pub transform_index: u32,
    pub grayscale: PaddedBool32,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
}

impl PolychromeSprite {
    /// The low byte of the sprite's flag word: how its texture is read.
    pub fn mode(&self) -> u32 {
        self.grayscale.0 & 0xff
    }
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub image_buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

/// An external GPU texture painted at `bounds`, sampled directly by the
/// renderer (no atlas upload).
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct PaintExternalTexture {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub texture: ExternalTexture,
}

impl From<PaintExternalTexture> for Primitive {
    fn from(texture: PaintExternalTexture) -> Self {
        Primitive::ExternalTexture(texture)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[expect(missing_docs)]
pub struct PathId(pub usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub id: PathId,
    pub order: DrawOrder,
    pub bounds: Bounds<P>,
    pub content_mask: ContentMask<P>,
    pub vertices: Vec<PathVertex<P>>,
    pub color: Background,
    /// The effect group the path is drawn in; 0 for none.
    pub group: u32,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
}

impl Path<ScaledPixels> {
    /// Moves every vertex by `matrix` and clips to the transformed masks'
    /// bounding boxes.
    pub(crate) fn apply_transform(&mut self, matrix: &TransformationMatrix) {
        for vertex in &mut self.vertices {
            vertex.xy_position = matrix.transform_point(vertex.xy_position);
            vertex.content_mask.bounds = matrix.transform_bounds(&vertex.content_mask.bounds);
        }
        self.bounds = matrix.transform_bounds(&self.bounds);
        self.content_mask.bounds = matrix.transform_bounds(&self.content_mask.bounds);
    }
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            group: 0,
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
        }
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            group: 0,
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            vertices: self
                .vertices
                .iter()
                .map(|vertex| vertex.scale(factor))
                .collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        self.vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    #[expect(missing_docs)]
    pub fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub xy_position: Point<P>,
    pub st_position: Point<f32>,
    pub content_mask: ContentMask<P>,
}

#[expect(missing_docs)]
impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}

#[cfg(test)]
mod transform_tests {
    use super::*;
    use crate::{Corners, Edges, point, size};

    fn quad_at(x: f32, y: f32) -> Quad {
        Quad {
            order: 0,
            border_style_and_transform: Quad::pack_border_style(
                BorderStyle::Dashed,
                TransformIndex::IDENTITY,
            ),
            bounds: Bounds {
                origin: point(ScaledPixels(x), ScaledPixels(y)),
                size: size(ScaledPixels(10.), ScaledPixels(10.)),
            },
            content_mask: ContentMask::new(Bounds {
                origin: point(ScaledPixels(-1000.), ScaledPixels(-1000.)),
                size: size(ScaledPixels(2000.), ScaledPixels(2000.)),
            }),
            background: Background::default(),
            border_color: Hsla::default(),
            corner_radii: Corners::default(),
            border_widths: Edges::default(),
        }
    }

    fn rotation(degrees: f32) -> TransformationMatrix {
        TransformationMatrix::unit().rotate(Radians(degrees.to_radians()))
    }

    fn translation(x: f32, y: f32) -> TransformationMatrix {
        TransformationMatrix::unit().translate(point(ScaledPixels(x), ScaledPixels(y)))
    }

    fn halve_red() -> ColorFilter {
        let mut filter = ColorFilter::IDENTITY;
        filter.rows[0][0] = 0.5;
        filter
    }

    #[test]
    fn color_filters_nest_and_restore() {
        let mut scene = Scene::default();
        assert!(!scene.transforms.has_filters());
        let outer = scene.push_color_filter(halve_red());
        let inner = scene.push_color_filter(halve_red());
        let filter = scene.transforms.filter(scene.current_transform());
        assert_eq!(filter.rows[0][0], 0.25);
        scene.pop_transform(inner);
        assert_eq!(
            scene.transforms.filter(scene.current_transform()).rows[0][0],
            0.5
        );
        scene.pop_transform(outer);
        assert!(scene.current_transform().is_identity());
        assert!(scene.transforms.has_filters());
        scene.clear();
        assert!(!scene.transforms.has_filters());
        assert_eq!(scene.transforms.filters().len(), 1);
    }

    #[test]
    fn a_filter_survives_a_transform_pushed_inside_it() {
        let mut scene = Scene::default();
        let filtered = scene.push_color_filter(halve_red());
        let moved = scene.push_transform(translation(4., 0.));
        let index = scene.current_transform();
        assert_eq!(scene.transforms.filter(index).rows[0][0], 0.5);
        assert_eq!(scene.transforms.get(index), translation(4., 0.));
        scene.pop_transform(moved);
        scene.pop_transform(filtered);
    }

    #[test]
    fn the_table_starts_with_the_identity_and_resets_on_clear() {
        let mut scene = Scene::default();
        assert_eq!(scene.transforms.entries(), &[TransformationMatrix::unit()]);
        let previous = scene.push_transform(rotation(30.));
        assert_eq!(scene.transforms.entries().len(), 2);
        scene.pop_transform(previous);
        scene.clear();
        assert_eq!(scene.transforms.entries(), &[TransformationMatrix::unit()]);
        assert!(scene.current_transform().is_identity());
    }

    #[test]
    fn equal_matrices_share_an_entry_and_nesting_composes() {
        let mut scene = Scene::default();
        let outer = scene.push_transform(translation(10., 0.));
        let inner = scene.push_transform(translation(0., 5.));
        let composed = scene.transforms.get(scene.current_transform());
        assert_eq!(composed.translation, [10., 5.]);
        scene.pop_transform(inner);
        let again = scene.push_transform(translation(0., 5.));
        assert_eq!(scene.transforms.entries().len(), 3);
        scene.pop_transform(again);
        scene.pop_transform(outer);
        assert!(scene.current_transform().is_identity());
    }

    #[test]
    fn primitives_are_stamped_with_the_current_transform() {
        let mut scene = Scene::default();
        scene.insert_primitive(quad_at(0., 0.));
        let previous = scene.push_transform(rotation(45.));
        scene.insert_primitive(quad_at(0., 0.));
        scene.pop_transform(previous);
        scene.insert_primitive(quad_at(0., 0.));
        let indices: Vec<u32> = scene
            .quads
            .iter()
            .map(|quad| quad.transform_index().0)
            .collect();
        assert_eq!(indices, vec![0, 1, 0]);
        // The border style survives the stamping.
        assert!(
            scene
                .quads
                .iter()
                .all(|q| q.border_style() == BorderStyle::Dashed)
        );
    }

    #[test]
    fn a_quad_packs_its_style_and_index_into_one_word() {
        let mut quad = quad_at(0., 0.);
        quad.set_transform_index(TransformIndex(0xabcdef));
        assert_eq!(quad.border_style(), BorderStyle::Dashed);
        assert_eq!(quad.transform_index(), TransformIndex(0xabcdef));
        quad.set_border_style(BorderStyle::Solid);
        assert_eq!(quad.transform_index(), TransformIndex(0xabcdef));
        assert_eq!(quad.border_style_and_transform & 0xff, 0);
    }

    #[test]
    fn a_rotated_primitive_is_ordered_by_where_it_lands() {
        let mut scene = Scene::default();
        // A 10 px quad at the origin, turned a quarter about (5, 5), still
        // covers (0..10); turned 45° its corners reach past 10 along the axes.
        let about = TransformationMatrix::about(5., 5., rotation(45.));
        let previous = scene.push_transform(about);
        scene.insert_primitive(quad_at(0., 0.));
        scene.pop_transform(previous);
        // This one lies where only the rotated corner reaches.
        scene.insert_primitive(quad_at(11., 4.));
        let orders: Vec<u32> = scene.quads.iter().map(|quad| quad.order).collect();
        assert!(orders[1] > orders[0], "{orders:?}");
    }

    #[test]
    fn replay_carries_absolute_transforms_into_the_new_table() {
        let mut previous_scene = Scene::default();
        let before = previous_scene.push_transform(rotation(20.));
        previous_scene.insert_primitive(quad_at(0., 0.));
        previous_scene.pop_transform(before);
        previous_scene.insert_primitive(quad_at(30., 0.));

        let mut scene = Scene::default();
        // An unrelated transform is already in this table.
        let unrelated = scene.push_transform(translation(99., 99.));
        scene.pop_transform(unrelated);
        scene.replay(0..previous_scene.len(), &previous_scene);
        let matrices: Vec<TransformationMatrix> = scene
            .quads
            .iter()
            .map(|quad| scene.transforms.get(quad.transform_index()))
            .collect();
        assert_eq!(matrices[0], rotation(20.));
        assert!(matrices[1].is_unit());
        // Replaying under another transform does not compose with it.
        let mut replayed = Scene::default();
        let current = replayed.push_transform(translation(5., 5.));
        replayed.replay(0..previous_scene.len(), &previous_scene);
        replayed.pop_transform(current);
        assert_eq!(
            replayed.transforms.get(replayed.quads[0].transform_index()),
            rotation(20.)
        );
    }

    #[test]
    fn the_inverse_undoes_the_matrix_and_a_flattened_one_has_none() {
        let matrix = TransformationMatrix::about(30., 40., rotation(33.))
            .compose(translation(7., -3.))
            .scale(size(2., 3.));
        let inverse = matrix
            .inverse()
            .expect("a rotation and scale is invertible");
        let start = point(ScaledPixels(12.), ScaledPixels(-8.));
        let back = inverse.transform_point(matrix.transform_point(start));
        assert!((back.x.0 - start.x.0).abs() < 1e-3 && (back.y.0 - start.y.0).abs() < 1e-3);
        assert!(
            TransformationMatrix::unit()
                .scale(size(0., 1.))
                .inverse()
                .is_none()
        );
    }

    #[test]
    fn a_path_is_transformed_when_it_is_inserted() {
        let mut scene = Scene::default();
        let mut path = Path::new(point(crate::px(0.), crate::px(0.)));
        path.line_to(point(crate::px(10.), crate::px(0.)));
        path.line_to(point(crate::px(10.), crate::px(10.)));
        let scaled = Path {
            group: 0,
            id: path.id,
            order: path.order,
            bounds: path.bounds.scale(1.0),
            content_mask: ContentMask::new(Bounds {
                origin: point(ScaledPixels(-100.), ScaledPixels(-100.)),
                size: size(ScaledPixels(300.), ScaledPixels(300.)),
            }),
            vertices: path.vertices.iter().map(|v| v.scale(1.0)).collect(),
            color: path.color,
            start: path.start.scale(1.0),
            current: path.current.scale(1.0),
            contour_count: path.contour_count,
        };
        let previous = scene.push_transform(translation(100., 50.));
        scene.insert_primitive(scaled);
        scene.pop_transform(previous);
        let first = scene.paths[0].vertices[0].xy_position;
        assert_eq!((first.x.0, first.y.0), (100., 50.));
    }
}
