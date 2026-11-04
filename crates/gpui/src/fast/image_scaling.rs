//! Large images decoded at the size they are shown, and kept as encoded
//! bytes, not as pixels.
//!
//! Upstream's image loader decodes every image at its source resolution, holds
//! the pixels in the asset cache and uploads them to the atlas as they are: a
//! 12 megapixel photo shown at 200x150 costs 48 MB of RAM and of atlas. Here a
//! source of more than `LAZY_MIN_PIXELS` is loaded as a *lazy* image: its
//! dimensions and a handle to its encoded bytes, no pixels. What a view shows
//! is a *variant* of it, decoded and resized on a background thread to the
//! size the view needs, rounded up to `STEP` pixels. A variant is its own
//! small `RenderImage` with an `ImageId` of its own, so the atlas keys and
//! `Window::paint_image` are upstream's. Once the variant is in the atlas the
//! cache keeps only a pixel-less `RenderImage` with the same id; if the atlas
//! loses it, it is decoded again.
//!
//! Small, animated and caller-supplied images are not lazy and are drawn as
//! before.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    thread::ThreadId,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use image::{Frame, RgbaImage, imageops::FilterType};
use smallvec::SmallVec;

use crate::{
    App, Bounds, DefiniteLength, DevicePixels, EntityId, ImageCacheError, ImageId, Length,
    ObjectFit, Pixels, RenderImage, Resource, Size, Window, WindowId, size,
};

/// A source with more decoded pixels than this is loaded lazily: 4 MB of
/// pixels, which decodes in about 15 ms. Icons, avatars and thumbnails stay
/// eager, so they appear with their view and need no second step.
pub(crate) const LAZY_MIN_PIXELS: u64 = 1_000_000;

/// The long edge of the variant the loader decodes ahead of the first layout.
const PREVIEW_EDGE: f32 = 1024.0;

/// Variants are made in multiples of this many pixels on the long edge.
const STEP: u32 = 64;

/// A variant at most this many times larger than the wanted size is shown in
/// its place, rather than decoding another.
const REUSE_RATIO: f32 = 1.5;

/// The bytes of variants, at 4 bytes a pixel, held before the least recently
/// used ones that are not in use are released.
const BUDGET: usize = 128 << 20;

/// A variant not shown for this long is released.
pub(crate) const IDLE: Duration = Duration::from_secs(2);

/// A variant shown this recently is in use and not released for the budget.
const RECENT: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub(crate) enum Source {
    Path(Arc<Path>),
    Bytes(Arc<[u8]>),
}

/// What a `RenderImage` without pixels stands for.
#[derive(Clone)]
pub(crate) enum Lazy {
    /// A source image, known by its size and where its encoded bytes are.
    Source {
        size: Size<DevicePixels>,
        source: Source,
    },
    /// A variant whose pixels are in the atlas.
    Released { size: Size<DevicePixels> },
}

impl Lazy {
    fn size(&self) -> Size<DevicePixels> {
        match self {
            Lazy::Source { size, .. } | Lazy::Released { size } => *size,
        }
    }
}

pub(crate) fn lazy_size(lazy: &Option<Lazy>) -> Option<Size<DevicePixels>> {
    lazy.as_ref().map(Lazy::size)
}

pub(crate) fn is_lazy(lazy: &Option<Lazy>) -> bool {
    lazy.is_some()
}

pub(crate) fn frame_count(lazy: &Option<Lazy>, frames: usize) -> usize {
    if lazy.is_some() { 1 } else { frames }
}

/// Reads the dimensions of an encoded still image, as it is shown (rotated by
/// its orientation), without decoding it. `None` for formats that can be
/// animated or are not rasters.
fn dimensions(bytes: &[u8]) -> Option<Size<DevicePixels>> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if matches!(reader.format()?, image::ImageFormat::Gif) {
        return None;
    }
    let mut decoder = reader.into_decoder().ok()?;
    if image::ImageDecoder::dimensions(&decoder) == (0, 0) {
        return None;
    }
    if is_animated(bytes) {
        return None;
    }
    let (mut width, mut height) = image::ImageDecoder::dimensions(&decoder);
    let orientation = image::ImageDecoder::orientation(&mut decoder).ok()?;
    if matches!(
        orientation,
        image::metadata::Orientation::Rotate90
            | image::metadata::Orientation::Rotate270
            | image::metadata::Orientation::Rotate90FlipH
            | image::metadata::Orientation::Rotate270FlipH
    ) {
        std::mem::swap(&mut width, &mut height);
    }
    Some(size(
        DevicePixels(width as i32),
        DevicePixels(height as i32),
    ))
}

fn is_animated(bytes: &[u8]) -> bool {
    matches!(image::guess_format(bytes), Ok(image::ImageFormat::WebP))
        && image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
            .is_ok_and(|decoder| decoder.has_animation())
}

/// The lazy image for an image the loader has read, if it is a big still one.
/// `path` is where it was read from, when from a file.
pub(crate) fn lazy_image(
    bytes: &[u8],
    path: Option<&Path>,
    resource: &Resource,
) -> Option<Arc<RenderImage>> {
    let size = dimensions(bytes)?;
    if (size.width.0.max(0) as u64) * (size.height.0.max(0) as u64) <= LAZY_MIN_PIXELS {
        return None;
    }
    let source = match path {
        Some(path) => Source::Path(path.into()),
        None => Source::Bytes(bytes.into()),
    };
    let image = lazy_render_image(
        None,
        Lazy::Source {
            size,
            source: source.clone(),
        },
    );
    // The loader decodes the variant for the first frame after the load, as
    // the eager loader decodes the image: at the size the view announced
    // when it asked for the image, or else a preview, which the variant at
    // the display size follows and replaces.
    let hint = registry()
        .lock()
        .unwrap()
        .hints
        .get(&crate::hash(resource))
        .copied();
    let first = match hint {
        Some(hint) => {
            let bounds = Bounds {
                origin: crate::point(Pixels(0.), Pixels(0.)),
                size: hint.size,
            };
            let shown = fit_from(hint.fit).get_bounds(bounds, size).size;
            target_size(size, shown, hint.scale)
        }
        None => target_size(size, size_in_pixels(PREVIEW_EDGE), 1.0),
    };
    let _permit = DecodePermit::acquire();
    if let Ok(pixels) = build_from(bytes, first) {
        registry()
            .lock()
            .unwrap()
            .preseed(image.id, &source, first, pixels, Instant::now());
    }
    Some(image)
}

/// Holds one of the `max_jobs` places for decoding that the loader's threads
/// share with nobody else, so they do not all hold a full-size image at once.
struct DecodePermit;

static PERMITS: (Mutex<usize>, std::sync::Condvar) = (Mutex::new(0), std::sync::Condvar::new());

impl DecodePermit {
    fn acquire() -> Self {
        let mut taken = PERMITS.0.lock().unwrap();
        while *taken >= max_jobs() {
            taken = PERMITS.1.wait(taken).unwrap();
        }
        *taken += 1;
        Self
    }
}

impl Drop for DecodePermit {
    fn drop(&mut self) {
        *PERMITS.0.lock().unwrap() -= 1;
        PERMITS.1.notify_one();
    }
}

/// A square of `edge` pixels, as the box an image is fitted into.
fn size_in_pixels(edge: f32) -> Size<Pixels> {
    size(Pixels(edge), Pixels(edge))
}

fn lazy_render_image(id: Option<ImageId>, lazy: Lazy) -> Arc<RenderImage> {
    let mut image = RenderImage::new(SmallVec::new());
    if let Some(id) = id {
        image.id = id;
    }
    image.fast_lazy = Some(lazy);
    Arc::new(image)
}

/// The size of the variant to show an image of `source` size in `bounds`.
pub(crate) fn target_size(
    source: Size<DevicePixels>,
    bounds: Size<Pixels>,
    scale: f32,
) -> Size<DevicePixels> {
    let wanted_width = (f32::from(bounds.width) * scale).ceil().max(1.0);
    let wanted_height = (f32::from(bounds.height) * scale).ceil().max(1.0);
    let (source_width, source_height) =
        (source.width.0.max(1) as f32, source.height.0.max(1) as f32);
    // The factor to shrink the source by, never above one: no upscaling.
    let factor = (wanted_width / source_width)
        .max(wanted_height / source_height)
        .min(1.0);
    let (width, height) = (source_width * factor, source_height * factor);
    let long = width.max(height);
    let stepped = ((long / STEP as f32).ceil() * STEP as f32).min(source_width.max(source_height));
    let factor = stepped / source_width.max(source_height);
    size(
        DevicePixels(((source_width * factor).round() as i32).clamp(1, source.width.0.max(1))),
        DevicePixels(((source_height * factor).round() as i32).clamp(1, source.height.0.max(1))),
    )
}

type Key = (ImageId, Size<DevicePixels>);

/// How big a view will show an image, announced before the image has loaded.
#[derive(Clone, Copy)]
struct Hint {
    size: Size<Pixels>,
    fit: u8,
    scale: f32,
}

fn fit_code(fit: &ObjectFit) -> u8 {
    match fit {
        ObjectFit::Fill => 0,
        ObjectFit::Contain => 1,
        ObjectFit::Cover => 2,
        ObjectFit::ScaleDown => 3,
        ObjectFit::None => 4,
    }
}

fn fit_from(code: u8) -> ObjectFit {
    match code {
        0 => ObjectFit::Fill,
        1 => ObjectFit::Contain,
        2 => ObjectFit::Cover,
        3 => ObjectFit::ScaleDown,
        _ => ObjectFit::None,
    }
}

/// Forwarded to by `Img::request_layout` before it asks for the image: a view
/// of a definite size tells the loader what size to decode the first variant at.
pub(crate) fn hint(
    source: &crate::ImageSource,
    style_size: &Size<Length>,
    fit: &ObjectFit,
    window: &Window,
) {
    let absolute = |length: &Length| match length {
        Length::Definite(DefiniteLength::Absolute(length)) => {
            Some(length.to_pixels(window.rem_size()))
        }
        _ => None,
    };
    let (crate::ImageSource::Resource(resource), Some(width), Some(height)) = (
        source,
        absolute(&style_size.width),
        absolute(&style_size.height),
    ) else {
        return;
    };
    let scale = window.scale_factor();
    registry().lock().unwrap().hints.insert(
        crate::hash(resource),
        Hint {
            size: size(width, height),
            fit: fit_code(fit),
            scale,
        },
    );
}

enum State {
    Pending,
    Ready {
        /// The pixels, until they are handed out to be uploaded.
        pixels: Option<Arc<RenderImage>>,
        released: Arc<RenderImage>,
    },
    Failed(String),
}

struct Entry {
    id: ImageId,
    source: Source,
    bytes: usize,
    last_used: Instant,
    state: State,
}

struct Job {
    source_id: ImageId,
    target: Size<DevicePixels>,
    source: Source,
    variant: ImageId,
}

#[derive(Default)]
pub(crate) struct Variants {
    entries: HashMap<Key, Entry>,
    by_variant: HashMap<ImageId, Key>,
    queue: VecDeque<Job>,
    active: usize,
    waiting: HashMap<ImageId, HashSet<EntityId>>,
    requested: HashSet<ImageId>,
    missing: Vec<ImageId>,
    hints: HashMap<u64, Hint>,
    bytes: usize,
    /// The variants each window's latest frame paints, drawn or drawn again
    /// from the frame before, which are not released while they are on
    /// screen however long ago they were selected.
    on_screen: HashMap<(ThreadId, WindowId), HashSet<ImageId>>,
}

fn registry() -> &'static Mutex<Variants> {
    static REGISTRY: OnceLock<Mutex<Variants>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

fn long_edge(size: Size<DevicePixels>) -> f32 {
    size.width.0.max(size.height.0) as f32
}

impl Variants {
    /// The variant to show for `source` at `target`, or none yet; and queues
    /// the decode of `target` unless it, or a variant to be shown in its
    /// place, exists.
    fn select(
        &mut self,
        source_id: ImageId,
        source: &Source,
        target: Size<DevicePixels>,
        view: Option<EntityId>,
        now: Instant,
    ) -> Option<Arc<RenderImage>> {
        self.requested.insert(source_id);
        if let Some(view) = view {
            self.waiting.entry(source_id).or_default().insert(view);
        }
        let ready = |entry: &Entry| matches!(entry.state, State::Ready { .. });
        let exact = (source_id, target);
        let best: Option<Key> = if self.entries.get(&exact).is_some_and(ready) {
            Some(exact)
        } else {
            // The smallest ready variant not smaller than wanted and within
            // the reuse ratio.
            let wanted = long_edge(target);
            let mut reusable = None::<(Key, f32)>;
            let mut largest = None::<(Key, f32)>;
            for (key, entry) in &self.entries {
                if key.0 != source_id || !ready(entry) {
                    continue;
                }
                let edge = long_edge(key.1);
                if edge >= wanted
                    && edge <= wanted * REUSE_RATIO
                    && reusable.is_none_or(|(_, smallest)| edge < smallest)
                {
                    reusable = Some((*key, edge));
                }
                if largest.is_none_or(|(_, largest)| edge > largest) {
                    largest = Some((*key, edge));
                }
            }
            if reusable.is_none() && !self.entries.contains_key(&exact) {
                let variant = ImageId(next_id());
                self.entries.insert(
                    exact,
                    Entry {
                        id: variant,
                        source: source.clone(),
                        bytes: 0,
                        last_used: now,
                        state: State::Pending,
                    },
                );
                self.by_variant.insert(variant, exact);
                self.queue.push_back(Job {
                    source_id,
                    target,
                    source: source.clone(),
                    variant,
                });
            }
            reusable.or(largest).map(|(key, _)| key)
        };
        let entry = self.entries.get_mut(&best?)?;
        entry.last_used = now;
        match &mut entry.state {
            State::Ready { pixels, released } => {
                Some(pixels.take().unwrap_or_else(|| released.clone()))
            }
            _ => None,
        }
    }

    /// Adds a variant decoded by the loader.
    fn preseed(
        &mut self,
        source_id: ImageId,
        source: &Source,
        target: Size<DevicePixels>,
        buffer: RgbaImage,
        now: Instant,
    ) {
        let job = Job {
            source_id,
            target,
            source: source.clone(),
            variant: ImageId(next_id()),
        };
        self.entries.insert(
            (source_id, target),
            Entry {
                id: job.variant,
                source: source.clone(),
                bytes: 0,
                last_used: now,
                state: State::Pending,
            },
        );
        self.by_variant.insert(job.variant, (source_id, target));
        self.complete(&job, Ok(buffer), now);
    }

    fn failure(&self, source_id: ImageId) -> Option<String> {
        self.entries
            .iter()
            .find(|(key, entry)| key.0 == source_id && matches!(entry.state, State::Failed(_)))
            .and_then(|(_, entry)| match &entry.state {
                State::Failed(message) => Some(message.clone()),
                _ => None,
            })
    }

    fn has_ready(&self, source_id: ImageId) -> bool {
        self.entries
            .iter()
            .any(|(key, entry)| key.0 == source_id && matches!(entry.state, State::Ready { .. }))
    }

    /// Stores a finished decode and returns the views that waited for it.
    fn complete(&mut self, job: &Job, result: Result<RgbaImage>, now: Instant) -> Vec<EntityId> {
        let key = (job.source_id, job.target);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = now;
            match result {
                Ok(buffer) => {
                    let (width, height) = buffer.dimensions();
                    let size = size(DevicePixels(width as i32), DevicePixels(height as i32));
                    entry.bytes = width as usize * height as usize * 4;
                    self.bytes += entry.bytes;
                    entry.state = State::Ready {
                        pixels: Some(Arc::new({
                            let mut image =
                                RenderImage::new(SmallVec::from_elem(Frame::new(buffer), 1));
                            image.id = job.variant;
                            image
                        })),
                        released: lazy_render_image(Some(job.variant), Lazy::Released { size }),
                    };
                }
                Err(error) => entry.state = State::Failed(format!("{error:#}")),
            }
        }
        self.waiting
            .get(&job.source_id)
            .map(|views| views.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Releases variants unused for `IDLE`, and the least recently used of
    /// those not in use while over `BUDGET`. Returns what to remove from the
    /// atlases.
    fn evict(&mut self, now: Instant) -> Vec<Arc<RenderImage>> {
        let mut doomed: Vec<Key> = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                !matches!(entry.state, State::Pending)
                    && now.saturating_duration_since(entry.last_used) > IDLE
                    && !self.is_on_screen(entry.id)
            })
            .map(|(key, _)| *key)
            .collect();
        let mut over = self
            .bytes
            .saturating_sub(doomed.iter().map(|key| self.entries[key].bytes).sum());
        if over > BUDGET {
            let mut idle: Vec<(Instant, Key)> = self
                .entries
                .iter()
                .filter(|(key, entry)| {
                    matches!(entry.state, State::Ready { .. })
                        && !doomed.contains(key)
                        && now.saturating_duration_since(entry.last_used) > RECENT
                        && !self.is_on_screen(entry.id)
                })
                .map(|(key, entry)| (entry.last_used, *key))
                .collect();
            idle.sort_by_key(|(used, _)| *used);
            for (_, key) in idle {
                if over <= BUDGET {
                    break;
                }
                over -= self.entries[&key].bytes;
                doomed.push(key);
            }
        }
        doomed
            .into_iter()
            .filter_map(|key| self.remove(key))
            .collect()
    }

    fn is_on_screen(&self, variant: ImageId) -> bool {
        self.on_screen
            .values()
            .any(|shown| shown.contains(&variant))
    }

    fn remove(&mut self, key: Key) -> Option<Arc<RenderImage>> {
        let entry = self.entries.remove(&key)?;
        self.by_variant.remove(&entry.id);
        self.bytes -= entry.bytes;
        match entry.state {
            State::Ready { released, .. } => Some(released),
            _ => None,
        }
    }

    fn variant_ids(&self, source_id: ImageId) -> Vec<ImageId> {
        self.entries
            .iter()
            .filter(|(key, _)| key.0 == source_id)
            .map(|(_, entry)| entry.id)
            .collect()
    }
}

fn next_id() -> usize {
    // `RenderImage::new` draws ids from one counter; take one from it.
    RenderImage::new(SmallVec::new()).id.0
}

/// Decodes `source` and shrinks it to `target`, in BGRA order. An image
/// without transparency is shrunk as RGB, which saves the 4/3 copy at full size.
fn build(source: &Source, target: Size<DevicePixels>) -> Result<RgbaImage> {
    let bytes: std::borrow::Cow<[u8]> = match source {
        Source::Path(path) => std::fs::read(path)
            .with_context(|| format!("reading {}", path.display()))?
            .into(),
        Source::Bytes(bytes) => (&**bytes).into(),
    };
    build_from(&bytes, target)
}

/// `build` from encoded bytes.
fn build_from(bytes: &[u8], target: Size<DevicePixels>) -> Result<RgbaImage> {
    let mut decoder = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()?
        .into_decoder()
        .context("creating image decoder")?;
    let orientation =
        image::ImageDecoder::orientation(&mut decoder).context("reading decoder's orientation")?;
    let mut decoded = image::DynamicImage::from_decoder(decoder).context("decoding image")?;
    decoded.apply_orientation(orientation);
    let (width, height) = (target.width.0 as u32, target.height.0 as u32);
    let mut out = if decoded.color().has_alpha() {
        let buffer = decoded.into_rgba8();
        if width < buffer.width() || height < buffer.height() {
            shrink_with_alpha(buffer, width, height)
        } else {
            buffer
        }
    } else {
        let buffer = decoded.into_rgb8();
        let buffer = if width < buffer.width() || height < buffer.height() {
            shrink(buffer, width, height)
        } else {
            buffer
        };
        image::DynamicImage::ImageRgb8(buffer).into_rgba8()
    };
    for pixel in out.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(out)
}

/// Shrinks to `width` x `height`: an exact average over whole blocks down to
/// less than twice the size at most, then Lanczos3.
fn shrink<P>(
    buffer: image::ImageBuffer<P, Vec<u8>>,
    width: u32,
    height: u32,
) -> image::ImageBuffer<P, Vec<u8>>
where
    P: image::Pixel<Subpixel = u8> + 'static,
{
    let factor = (buffer.width() / width.max(1)).min(buffer.height() / height.max(1));
    if factor >= 2 {
        let averaged = block_average(&buffer, factor);
        drop(buffer);
        image::imageops::resize(&averaged, width, height, FilterType::Lanczos3)
    } else {
        image::imageops::resize(&buffer, width, height, FilterType::Lanczos3)
    }
}

/// `shrink` for an image with transparency, whose colours are premultiplied
/// while averaging unless every pixel is opaque.
fn shrink_with_alpha(mut buffer: RgbaImage, width: u32, height: u32) -> RgbaImage {
    let transparent = buffer.pixels().any(|pixel| pixel.0[3] != 255);
    if !transparent {
        return shrink(buffer, width, height);
    }
    for pixel in buffer.pixels_mut() {
        let alpha = u32::from(pixel.0[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    let mut resized = shrink(buffer, width, height);
    for pixel in resized.pixels_mut() {
        let alpha = u32::from(pixel.0[3]);
        if alpha != 0 && alpha != 255 {
            for channel in &mut pixel.0[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    resized
}

fn block_average<P>(
    buffer: &image::ImageBuffer<P, Vec<u8>>,
    factor: u32,
) -> image::ImageBuffer<P, Vec<u8>>
where
    P: image::Pixel<Subpixel = u8> + 'static,
{
    let channels = usize::from(P::CHANNEL_COUNT);
    let factor = factor as usize;
    let (width, height) = (
        buffer.width() as usize / factor,
        buffer.height() as usize / factor,
    );
    let stride = buffer.width() as usize * channels;
    let area = (factor * factor) as u32;
    let source = buffer.as_raw();
    let mut out = vec![0u8; width * height * channels];
    let mut sums = vec![0u32; width * channels];
    for (y, out_row) in out.chunks_exact_mut(width * channels).enumerate() {
        sums.fill(0);
        for dy in 0..factor {
            let row = &source[(y * factor + dy) * stride..][..width * factor * channels];
            for (x, block) in row.chunks_exact(factor * channels).enumerate() {
                for pixel in block.chunks_exact(channels) {
                    for (sum, channel) in sums[x * channels..][..channels].iter_mut().zip(pixel) {
                        *sum += u32::from(*channel);
                    }
                }
            }
        }
        for (value, sum) in out_row.iter_mut().zip(&sums) {
            *value = ((sum + area / 2) / area) as u8;
        }
    }
    image::ImageBuffer::from_raw(width as u32, height as u32, out)
        .expect("the buffer holds width x height pixels")
}

/// Decodes in flight at once. Each holds the image at full size, and the
/// system allocator keeps what freed buffers took, so more only buys speed.
fn max_jobs() -> usize {
    std::thread::available_parallelism()
        .map_or(2, |cores| cores.get() - 2)
        .clamp(2, 6)
}

/// Starts queued decodes, up to `max_jobs` at once.
fn pump(cx: &mut App) {
    loop {
        let job = {
            let mut registry = registry().lock().unwrap();
            if registry.active >= max_jobs() {
                return;
            }
            let Some(job) = registry.queue.pop_front() else {
                return;
            };
            registry.active += 1;
            job
        };
        let task = {
            let source = job.source.clone();
            let target = job.target;
            cx.background_executor()
                .spawn(async move { build(&source, target) })
        };
        cx.spawn(async move |cx| {
            let result = task.await;
            cx.update(|cx| {
                let views = {
                    let mut registry = registry().lock().unwrap();
                    registry.active -= 1;
                    registry.complete(&job, result, Instant::now())
                };
                for view in views {
                    cx.notify(view);
                }
                pump(cx);
            });
        })
        .detach();
    }
}

/// Forwarded to by `Img::paint`: the image to paint for `data`, shown in
/// `image_bounds`. `data` itself unless it is lazy; `None` while the
/// variant to show for it is decoded.
pub(crate) fn display(
    data: Arc<RenderImage>,
    image_bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    let Some(Lazy::Source {
        size: source_size,
        source,
    }) = &data.fast_lazy
    else {
        return Some(data);
    };
    let target = target_size(*source_size, image_bounds.size, window.scale_factor());
    let now = Instant::now();
    let (shown, released) = {
        let mut registry = registry().lock().unwrap();
        let shown = registry.select(
            data.id,
            source,
            target,
            window.rendered_entity_stack.last().copied(),
            now,
        );
        let thread = std::thread::current().id();
        registry
            .on_screen
            .retain(|(from, id), _| *from != thread || cx.windows.contains_key(*id));
        (shown, registry.evict(now))
    };
    if let Some(shown) = &shown {
        window.next_frame.fast_images.push(shown.id);
    }
    for image in released {
        cx.drop_image(image, Some(window));
    }
    pump(cx);
    shown
}

/// Forwarded to by `Img::request_layout`: a lazy image whose first variant
/// is not ready yet is still loading, and one that failed to decode failed.
pub(crate) fn gate(
    loaded: Option<Result<Arc<RenderImage>, ImageCacheError>>,
) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
    let Some(Ok(data)) = &loaded else {
        return loaded;
    };
    if !matches!(data.fast_lazy, Some(Lazy::Source { .. })) {
        return loaded;
    }
    let registry = registry().lock().unwrap();
    if let Some(message) = registry.failure(data.id) {
        return Some(Err(ImageCacheError::Other(Arc::new(anyhow::anyhow!(
            message
        )))));
    }
    if registry.requested.contains(&data.id) && !registry.has_ready(data.id) {
        return None;
    }
    loaded
}

/// Forwarded to by `Window::paint_image` when the atlas has no tile for a
/// variant whose pixels were released.
pub(crate) fn missing(data: &RenderImage) {
    if matches!(data.fast_lazy, Some(Lazy::Released { .. })) {
        registry().lock().unwrap().missing.push(data.id);
    }
}

/// Forwarded to by `Img::paint` after it paints: decodes again the variants
/// the atlas lost.
pub(crate) fn after_paint(window: &Window, cx: &mut App) {
    let jobs = {
        let mut registry = registry().lock().unwrap();
        let ids = std::mem::take(&mut registry.missing);
        for id in ids {
            let Some(key) = registry.by_variant.get(&id).copied() else {
                continue;
            };
            let Some(entry) = registry.entries.get_mut(&key) else {
                continue;
            };
            entry.state = State::Pending;
            let bytes = std::mem::take(&mut entry.bytes);
            registry.bytes -= bytes;
            if let Some(view) = window.rendered_entity_stack.last().copied() {
                registry.waiting.entry(key.0).or_default().insert(view);
            }
            let source = registry.entries[&key].source.clone();
            registry.queue.push_back(Job {
                source_id: key.0,
                target: key.1,
                source,
                variant: id,
            });
        }
        !registry.queue.is_empty()
    };
    if jobs {
        pump(cx);
    }
}

/// Carries into the frame being drawn the variants the rendered frame's paint
/// of `range` showed, where drawing that paint again, or compositing the layer
/// that holds it, does not paint them.
pub(crate) fn carry(window: &mut Window, range: &std::ops::Range<crate::PaintIndex>) {
    let shown =
        window.rendered_frame.fast_images[range.start.fast.images..range.end.fast.images].to_vec();
    window.next_frame.fast_images.extend(shown);
}

/// Forwarded to by `finish_retained_frame`: the variants the frame just
/// drawn paints, which includes those of the views it drew again from the
/// frame before without painting them.
pub(crate) fn finish_frame(window: &Window) {
    let shown: HashSet<ImageId> = window.next_frame.fast_images.iter().copied().collect();
    let key = (std::thread::current().id(), window.handle.window_id());
    let mut registry = registry().lock().unwrap();
    if shown.is_empty() {
        registry.on_screen.remove(&key);
    } else {
        registry.on_screen.insert(key, shown);
    }
}

/// Forwarded to by `Window::drop_image`: the ids of the variants to remove
/// from the atlas along with the source.
pub(crate) fn variant_ids(data: &RenderImage) -> Vec<ImageId> {
    match data.fast_lazy {
        Some(Lazy::Source { .. }) => registry().lock().unwrap().variant_ids(data.id),
        _ => Vec::new(),
    }
}

/// The variants the calling thread's windows paint, for tests.
#[cfg(test)]
pub(crate) fn on_screen_ids() -> Vec<ImageId> {
    let thread = std::thread::current().id();
    registry()
        .lock()
        .unwrap()
        .on_screen
        .iter()
        .filter(|((from, _), _)| *from == thread)
        .flat_map(|(_, shown)| shown.iter().copied())
        .collect()
}

/// Every variant's id, for tests that clear an atlas.
#[cfg(test)]
pub(crate) fn all_variant_ids() -> Vec<ImageId> {
    registry()
        .lock()
        .unwrap()
        .by_variant
        .keys()
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        BUDGET, Hint, IDLE, Key, LAZY_MIN_PIXELS, Lazy, RECENT, Source, State, Variants, fit_code,
        lazy_image, registry, target_size,
    };
    use crate::{DevicePixels, EntityId, ImageId, Pixels, Size, px, size};
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    fn device(width: i32, height: i32) -> Size<DevicePixels> {
        size(DevicePixels(width), DevicePixels(height))
    }

    fn logical(width: f32, height: f32) -> Size<Pixels> {
        size(px(width), px(height))
    }

    fn long_edge(size: Size<DevicePixels>) -> i32 {
        size.width.0.max(size.height.0)
    }

    #[test]
    fn the_target_is_the_display_size_in_steps_and_never_above_the_source() {
        let source = device(4000, 3000);
        // 200x150 points on a 2x display: 400x300 pixels, long edge 400 -> 448.
        assert_eq!(
            target_size(source, logical(200., 150.), 2.),
            device(448, 336)
        );
        // The aspect ratio is the source's, whichever box asked.
        let target = target_size(source, logical(200., 100.), 2.);
        assert_eq!(target.width.0 * 3, target.height.0 * 4);
        // A display larger than the source is not upscaled.
        assert_eq!(target_size(source, logical(5000., 4000.), 2.), source);
        assert_eq!(target_size(source, logical(2000., 1500.), 2.), source);
        // 1x and 2x ask for different sizes: a scale factor change decodes again.
        assert_ne!(
            target_size(source, logical(200., 150.), 1.),
            target_size(source, logical(200., 150.), 2.)
        );
    }

    #[test]
    fn object_fit_cover_asks_for_the_overflowing_image_not_the_box() {
        let source = device(4000, 3000);
        // A 100x100 box covered by a 4:3 image: the image is 133x100.
        let cover = target_size(source, logical(133.4, 100.), 2.);
        let contain = target_size(source, logical(100., 75.), 2.);
        assert!(long_edge(cover) > long_edge(contain));
        assert_eq!(cover, device(320, 240));
        assert_eq!(contain, device(256, 192));
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([10, 200, 90, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        bytes
    }

    #[test]
    fn only_sources_above_the_threshold_are_lazy() {
        assert_eq!(LAZY_MIN_PIXELS, 1_000_000);
        assert!(lazy_image(&png(1000, 1000), None, &resource()).is_none());
        assert!(lazy_image(&png(200, 150), None, &resource()).is_none());
        let big = lazy_image(&png(1001, 1000), None, &resource()).expect("above the threshold");
        assert_eq!(big.size(0), device(1001, 1000));
        assert_eq!(big.frame_count(), 1);
        assert!(big.as_bytes(0).is_none());
        assert!(matches!(big.fast_lazy, Some(Lazy::Source { .. })));
        // Garbage and non-images stay with upstream's path.
        assert!(lazy_image(b"not an image", None, &resource()).is_none());
    }

    fn resource() -> crate::Resource {
        crate::Resource::Path(std::path::PathBuf::from("/nonexistent/threshold.png").into())
    }

    fn source() -> (ImageId, Source) {
        (ImageId(1_000_000), Source::Bytes(Arc::from(&b""[..])))
    }

    fn ready(registry: &mut Variants, key: Key, bytes: usize, now: Instant) {
        let entry = registry
            .entries
            .get_mut(&key)
            .expect("the variant was requested");
        entry.bytes = bytes;
        registry.bytes += bytes;
        let id = entry.id;
        entry.last_used = now;
        entry.state = State::Ready {
            pixels: None,
            released: super::lazy_render_image(Some(id), Lazy::Released { size: key.1 }),
        };
    }

    #[test]
    fn a_larger_variant_within_the_ratio_is_reused_and_others_decode() {
        let (id, src) = source();
        let view = Some(EntityId::from(1u64));
        let now = Instant::now();
        let mut registry = Variants::default();
        let wanted = device(448, 336);
        assert!(registry.select(id, &src, wanted, view, now).is_none());
        assert_eq!(registry.queue.len(), 1);
        ready(&mut registry, (id, wanted), 448 * 336 * 4, now);

        // The same size is a hit; so is a smaller one up to 1.5x below.
        assert!(registry.select(id, &src, wanted, view, now).is_some());
        assert!(
            registry
                .select(id, &src, device(320, 240), view, now)
                .is_some()
        );
        assert_eq!(registry.queue.len(), 1);

        // A smaller wanted size that makes the variant more than 1.5x too large
        // decodes anew, and the existing one is shown until it is ready.
        assert!(
            registry
                .select(id, &src, device(256, 192), view, now)
                .is_some()
        );
        assert_eq!(registry.queue.len(), 2);
        // A larger one too.
        assert!(
            registry
                .select(id, &src, device(640, 480), view, now)
                .is_some()
        );
        assert_eq!(registry.queue.len(), 3);
        // Asking again does not queue it twice.
        registry.select(id, &src, device(640, 480), view, now);
        assert_eq!(registry.queue.len(), 3);
    }

    #[test]
    fn a_view_announcing_its_size_gets_the_first_variant_at_it() {
        let resource =
            crate::Resource::Path(std::path::PathBuf::from("/nonexistent/hinted.png").into());
        registry().lock().unwrap().hints.insert(
            crate::hash(&resource),
            Hint {
                size: logical(100., 100.),
                fit: fit_code(&crate::ObjectFit::Fill),
                scale: 2.,
            },
        );
        let image = lazy_image(&png(1600, 1200), None, &resource).expect("lazy");
        let sizes: Vec<_> = registry()
            .lock()
            .unwrap()
            .entries
            .keys()
            .filter(|key| key.0 == image.id)
            .map(|key| key.1)
            .collect();
        // A 100 pt box at 2x is 200 px; the 4:3 source fills it at 266 wide,
        // in steps of 64: 320x240.
        assert_eq!(sizes, vec![device(320, 240)]);
    }

    #[test]
    fn pixels_are_handed_out_once() {
        let (id, src) = source();
        let view = Some(EntityId::from(1u64));
        let now = Instant::now();
        let mut registry = Variants::default();
        let wanted = device(64, 48);
        registry.select(id, &src, wanted, view, now);
        let key = (id, wanted);
        let variant = registry.entries[&key].id;
        let job = registry.queue.pop_front().unwrap();
        let buffer = image::RgbaImage::from_pixel(64, 48, image::Rgba([1, 2, 3, 255]));
        registry.complete(&job, Ok(buffer), now);
        let first = registry.select(id, &src, wanted, view, now).unwrap();
        assert_eq!(first.id, variant);
        assert!(first.as_bytes(0).is_some());
        let second = registry.select(id, &src, wanted, view, now).unwrap();
        assert_eq!(second.id, variant);
        assert!(second.as_bytes(0).is_none());
        assert_eq!(second.size(0), wanted);
    }

    #[test]
    fn variants_unused_for_a_while_go_and_those_in_use_stay_under_the_budget() {
        let view = Some(EntityId::from(1u64));
        let start = Instant::now();
        let mut registry = Variants::default();
        let sources: Vec<(ImageId, Source)> = (0..4)
            .map(|index| {
                (
                    ImageId(2_000_000 + index),
                    Source::Bytes(Arc::from(&b""[..])),
                )
            })
            .collect();
        let big = BUDGET / 2;
        for (id, src) in &sources {
            let wanted = device(1000, 1000);
            registry.select(*id, src, wanted, view, start);
            ready(&mut registry, (*id, wanted), big, start);
        }
        assert_eq!(registry.bytes, 4 * big);

        // Everything was used just now: over budget, but nothing may go.
        assert!(registry.evict(start).is_empty());

        // Two are used again; the other two have been idle past RECENT and go,
        // oldest first, until the budget holds.
        let later = start + RECENT + Duration::from_millis(100);
        for (id, src) in sources.iter().take(2) {
            assert!(
                registry
                    .select(*id, src, device(1000, 1000), view, later)
                    .is_some()
            );
        }
        let released = registry.evict(later);
        assert_eq!(released.len(), 2);
        assert_eq!(registry.bytes, 2 * big);
        assert!(
            registry
                .entries
                .contains_key(&(sources[0].0, device(1000, 1000)))
        );
        assert!(
            registry
                .entries
                .contains_key(&(sources[1].0, device(1000, 1000)))
        );

        // Idle for IDLE: released whatever the budget.
        let idle = later + IDLE + Duration::from_millis(1);
        assert_eq!(registry.evict(idle).len(), 2);
        assert!(registry.entries.is_empty());
        assert_eq!(registry.bytes, 0);
    }

    #[test]
    fn a_variant_one_window_still_paints_is_not_released_when_another_stops() {
        let (window_a, window_b) = (Some(EntityId::from(1u64)), Some(EntityId::from(2u64)));
        let start = Instant::now();
        let mut registry = Variants::default();
        let (id, src) = source();
        let wanted = device(1000, 1000);
        registry.select(id, &src, wanted, window_a, start);
        ready(&mut registry, (id, wanted), 1000, start);
        // Both windows paint it; then A stops. B keeps painting it, so the
        // variant is not idle however long A has been away.
        registry.select(id, &src, wanted, window_b, start);
        let mut now = start;
        for _ in 0..10 {
            now += IDLE / 2;
            assert!(registry.select(id, &src, wanted, window_b, now).is_some());
            assert!(registry.evict(now).is_empty());
        }
        // Once B stops too, it goes after IDLE.
        assert_eq!(
            registry.evict(now + IDLE + Duration::from_millis(1)).len(),
            1
        );
    }

    #[test]
    fn a_variant_on_screen_is_kept_until_the_frame_stops_painting_it() {
        let view = Some(EntityId::from(1u64));
        let start = Instant::now();
        let mut registry = Variants::default();
        let (id, src) = source();
        let wanted = device(1000, 1000);
        registry.select(id, &src, wanted, view, start);
        ready(&mut registry, (id, wanted), 1000, start);
        let variant = registry.entries[&(id, wanted)].id;
        let window = (std::thread::current().id(), crate::WindowId::from(1u64));
        registry.on_screen.insert(window, [variant].into());

        let idle = start + IDLE * 3;
        assert!(registry.evict(idle).is_empty());
        // The window's next frame paints something else.
        registry.on_screen.remove(&window);
        assert_eq!(registry.evict(idle).len(), 1);
    }

    #[test]
    fn a_failed_decode_is_reported() {
        let (id, src) = source();
        let view = Some(EntityId::from(1u64));
        let now = Instant::now();
        let mut registry = Variants::default();
        registry.select(id, &src, device(64, 64), view, now);
        let job = registry.queue.pop_front().unwrap();
        registry.complete(&job, Err(anyhow::anyhow!("corrupt")), now);
        assert_eq!(registry.failure(id).as_deref(), Some("corrupt"));
    }

    #[test]
    fn shrinking_keeps_the_colours_and_the_transparency() {
        let mut opaque = image::RgbaImage::from_pixel(400, 300, image::Rgba([10, 20, 30, 255]));
        let small = super::shrink_with_alpha(opaque.clone(), 40, 30);
        assert_eq!(small.dimensions(), (40, 30));
        assert!(small.pixels().all(|pixel| pixel.0 == [10, 20, 30, 255]));

        // A transparent half does not bleed its colour into the opaque half.
        for y in 0..300 {
            for x in 0..200 {
                opaque.put_pixel(x, y, image::Rgba([255, 0, 0, 0]));
            }
        }
        let small = super::shrink_with_alpha(opaque, 40, 30);
        assert_eq!(small.get_pixel(30, 15).0, [10, 20, 30, 255]);
        assert_eq!(small.get_pixel(5, 15).0[3], 0);
    }
}
